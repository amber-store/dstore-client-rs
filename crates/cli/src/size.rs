//! `cmd/dstore/size.go` (`parseSize`) and `main.go:209-227` (`defaultPackSize`, `packSize`).

use std::os::unix::ffi::OsStrExt;

use dstore_gocli::Context;
use dstore_gocompat::quote::quote;
use dstore_gocompat::strconv::parse_int;
use dstore_gocompat::strings::{to_lower, trim_space};

/// `defaultPackSize` (`main.go:209`), `node.DefaultSegmentSize`: the `--pack-size` default.
pub(crate) const DEFAULT_PACK_SIZE: &str = "2Gi";

/// `parseSize`: a byte count such as `1048576`, `512Mi` or `2Gi`. The suffixes K, M, G and T are binary
/// whether or not they carry an `i`, and one trailing `b` (any case) is ignored.
pub fn parse_size(s: &str) -> Result<i64, String> {
    parse_size_bytes(s.as_bytes())
}

/// `parseSize` over Go string bytes: a `--pack-size` value may be invalid UTF-8, which `%q` renders as
/// `\xNN`.
fn parse_size_bytes(s: &[u8]) -> Result<i64, String> {
    let s = trim_space(s);
    let digits = s.iter().take_while(|b| b.is_ascii_digit()).count();
    if digits == 0 {
        return Err(format!(
            "bad size {}: want a number with an optional Ki/Mi/Gi/Ti suffix",
            quote(s)
        ));
    }
    let (number, unit_text) = s.split_at(digits);
    // `number` holds ASCII digits only, so it is valid UTF-8 and ParseInt can only report a range error.
    let n = parse_int(&String::from_utf8_lossy(number), 10, 64)
        .map_err(|e| format!("bad size {}: {e}", quote(s)))?;
    let lower = to_lower(unit_text);
    let unit = lower.strip_suffix(b"b").unwrap_or(&lower);
    let shift: u32 = match unit {
        b"" => 0,
        b"k" | b"ki" => 10,
        b"m" | b"mi" => 20,
        b"g" | b"gi" => 30,
        b"t" | b"ti" => 40,
        _ => {
            return Err(format!(
                "bad size {}: unknown unit {} (want Ki, Mi, Gi or Ti)",
                quote(s),
                quote(unit_text)
            ));
        }
    };
    if n > i64::MAX >> shift {
        return Err(format!("bad size {}: too large", quote(s)));
    }
    Ok(n << shift)
}

/// `packSize`: `--pack-size` / `$DSTORE_PACK_SIZE`, trimmed; an empty value (an exported but empty
/// `$DSTORE_PACK_SIZE`) means the default.
pub fn pack_size(c: &Context) -> Result<i64, String> {
    let raw = c.os_string("pack-size");
    let mut v = trim_space(raw.as_bytes());
    if v.is_empty() {
        v = DEFAULT_PACK_SIZE.as_bytes();
    }
    let n = parse_size_bytes(v).map_err(|e| format!("--pack-size: {e}"))?;
    if n <= 0 {
        return Err(format!("--pack-size: {n} is not a positive size"));
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `size_test.go` `TestParseSize`: the good table.
    #[test]
    fn parse_size_good() {
        let good: &[(&str, i64)] = &[
            ("0", 0),
            ("1024", 1024),
            ("512Ki", 512 << 10),
            ("256Mi", 256 << 20),
            ("2Gi", 2 << 30),
            ("1Ti", 1 << 40),
            ("2gi", 2 << 30),
            ("2GiB", 2 << 30),
            ("2G", 2 << 30),
            ("2GB", 2 << 30),
            (" 2Gi ", 2 << 30),
        ];
        for &(input, want) in good {
            assert_eq!(parse_size(input), Ok(want), "parseSize({input:?})");
        }
    }

    /// `size_test.go` `TestParseSize`: the bad table.
    #[test]
    fn parse_size_bad() {
        for input in ["", "Gi", "2X", "2.5Gi", "-1", "1e3", "9999999999Ti"] {
            assert!(parse_size(input).is_err(), "parseSize({input:?}) = ok");
        }
    }

    /// cli.md §5.4 texts.
    #[test]
    fn parse_size_texts() {
        let cases: &[(&str, &str)] = &[
            (
                "",
                r#"bad size "": want a number with an optional Ki/Mi/Gi/Ti suffix"#,
            ),
            (
                "-1",
                r#"bad size "-1": want a number with an optional Ki/Mi/Gi/Ti suffix"#,
            ),
            (
                "2X",
                r#"bad size "2X": unknown unit "X" (want Ki, Mi, Gi or Ti)"#,
            ),
            (
                "2.5Gi",
                r#"bad size "2.5Gi": unknown unit ".5Gi" (want Ki, Mi, Gi or Ti)"#,
            ),
            (
                "2 Gi",
                r#"bad size "2 Gi": unknown unit " Gi" (want Ki, Mi, Gi or Ti)"#,
            ),
            ("8388608Ti", r#"bad size "8388608Ti": too large"#),
            (
                "99999999999999999999",
                r#"bad size "99999999999999999999": strconv.ParseInt: parsing "99999999999999999999": value out of range"#,
            ),
        ];
        for &(input, want) in cases {
            assert_eq!(
                parse_size(input),
                Err(want.to_string()),
                "parseSize({input:?})"
            );
        }
        assert_eq!(parse_size("8388607Ti"), Ok(9223370937343148032));
        assert_eq!(parse_size("2kb"), Ok(2048));
        assert_eq!(parse_size("2b"), Ok(2));
    }

    /// Invalid UTF-8 reaches `%q` as `\xNN`, as Go quotes the string's bytes.
    #[test]
    fn parse_size_invalid_utf8() {
        assert_eq!(
            parse_size_bytes(b"2\xff"),
            Err(r#"bad size "2\xff": unknown unit "\xff" (want Ki, Mi, Gi or Ti)"#.to_string())
        );
        assert_eq!(
            parse_size_bytes(b"\xff"),
            Err(
                r#"bad size "\xff": want a number with an optional Ki/Mi/Gi/Ti suffix"#.to_string()
            )
        );
    }

    /// `strings.ToLower` is Unicode: KELVIN SIGN lowers to `k`.
    #[test]
    fn parse_size_unicode_unit() {
        assert_eq!(parse_size("2\u{212a}"), Ok(2 << 10));
    }

    /// `size_test.go` `TestPackSizeFlag`: `packSize` inside an app over `nodeFlags()` (here the `serve` definition
    /// of the command table), with `$DSTORE_PACK_SIZE` from `getenv` instead of `t.Setenv`.
    #[test]
    fn pack_size_flag() {
        use std::ffi::OsString;

        let app = crate::app();
        let run = |env: &str, args: &[&str]| -> Result<i64, String> {
            let argv: Vec<OsString> = ["dstore", "serve"]
                .iter()
                .chain(args)
                .map(OsString::from)
                .collect();
            let getenv = |name: &str| (name == "DSTORE_PACK_SIZE").then(|| OsString::from(env));
            let mut stdout = Vec::new();
            match dstore_gocli::dispatch(&app, argv, &mut stdout, &getenv) {
                Ok(Some((_, ctx))) => pack_size(&ctx),
                other => panic!(
                    "dispatch {args:?}: {:?}, stdout {:?}",
                    other.map(|found| found.is_some()),
                    String::from_utf8_lossy(&stdout)
                ),
            }
        };
        assert_eq!(run("", &[]), Ok(2 << 30), "default");
        assert_eq!(
            run("", &["--pack-size", "512Mi"]),
            Ok(512 << 20),
            "--pack-size 512Mi"
        );
        assert_eq!(run("1Gi", &[]), Ok(1 << 30), "DSTORE_PACK_SIZE=1Gi");
        for bad in ["0", "x", "1.5Gi"] {
            assert!(
                run("", &["--pack-size", bad]).is_err(),
                "--pack-size {bad}: want an error"
            );
        }
    }
}
