//! Pass 1: well-formedness with fxamacker's limits and texts (`valid.go`).

use crate::{CborType, DecodeError};

/// fxamacker `defaultMaxNestedLevels`.
const MAX_NESTED_LEVELS: usize = 32;
/// fxamacker `defaultMaxArrayElements`.
const MAX_ARRAY_ELEMENTS: u64 = 131072;
/// fxamacker `defaultMaxMapPairs`.
const MAX_MAP_PAIRS: u64 = 131072;

/// fxamacker `Valid` as `DecOptions{}` configures it: max nesting 32, 131072 array elements and map
/// pairs, extraneous-data check.
pub fn well_formed(b: &[u8]) -> Result<(), DecodeError> {
    if b.is_empty() {
        return Err(DecodeError::Eof);
    }
    let mut v = Validator { data: b, off: 0 };
    v.item(0)?;
    if v.off != b.len() {
        return Err(DecodeError::Extraneous {
            n: b.len() - v.off,
            index: v.off,
        });
    }
    Ok(())
}

/// fxamacker `decoder` during `wellformed(false, false)`.
struct Validator<'a> {
    data: &'a [u8],
    off: usize,
}

/// A head read by `wellformedHead`.
struct Head {
    ty: CborType,
    ai: u8,
    val: u64,
}

impl Validator<'_> {
    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.off)
    }

    fn peek(&self) -> Option<u8> {
        self.data.get(self.off).copied()
    }

    /// `wellformedInternal`.
    fn item(&mut self, depth: usize) -> Result<(), DecodeError> {
        let h = self.head()?;
        match h.ty {
            CborType::ByteString | CborType::TextString => {
                if h.ai == 31 {
                    return self.indefinite_string(h.ty, depth);
                }
                // Go: int(val) < 0 detects a length beyond int64.
                if h.val > i64::MAX as u64 {
                    return Err(DecodeError::StrLenOverflow { ty: h.ty, n: h.val });
                }
                if (self.remaining() as u64) < h.val {
                    return Err(DecodeError::UnexpectedEof);
                }
                self.off += h.val as usize;
                Ok(())
            }
            CborType::Array | CborType::Map => {
                let depth = depth + 1;
                if depth > MAX_NESTED_LEVELS {
                    return Err(DecodeError::MaxNested);
                }
                if h.ai == 31 {
                    return self.indefinite_container(h.ty, depth);
                }
                if h.val > i64::MAX as u64 {
                    return Err(DecodeError::LenOverflow { ty: h.ty, n: h.val });
                }
                if h.ty == CborType::Array {
                    if h.val > MAX_ARRAY_ELEMENTS {
                        return Err(DecodeError::MaxArray);
                    }
                } else if h.val > MAX_MAP_PAIRS {
                    return Err(DecodeError::MaxMap);
                }
                let items = if h.ty == CborType::Map {
                    h.val * 2
                } else {
                    h.val
                };
                for _ in 0..items {
                    self.item(depth)?;
                }
                Ok(())
            }
            CborType::Tag => {
                // Scan nested tag numbers iteratively; every tag after the first adds a level.
                let mut depth = depth;
                loop {
                    let Some(next) = self.peek() else {
                        // A tag number must be followed by tag content.
                        return Err(DecodeError::UnexpectedEof);
                    };
                    if CborType::of_initial_byte(next) != CborType::Tag {
                        break;
                    }
                    self.head()?;
                    depth += 1;
                    if depth > MAX_NESTED_LEVELS {
                        return Err(DecodeError::MaxNested);
                    }
                }
                self.item(depth)
            }
            // Integers and primitives: the head is the whole item.
            _ => Ok(()),
        }
    }

    /// `wellformedIndefiniteString`.
    fn indefinite_string(&mut self, ty: CborType, depth: usize) -> Result<(), DecodeError> {
        loop {
            let Some(next) = self.peek() else {
                return Err(DecodeError::UnexpectedEof);
            };
            if next == 0xff {
                self.off += 1;
                return Ok(());
            }
            let chunk = CborType::of_initial_byte(next);
            if chunk != ty {
                return Err(DecodeError::ChunkType { chunk, ty });
            }
            if next & 0x1f == 31 {
                return Err(DecodeError::ChunkIndef(ty));
            }
            self.item(depth)?;
        }
    }

    /// `wellformedIndefiniteArrayOrMap`.
    fn indefinite_container(&mut self, ty: CborType, depth: usize) -> Result<(), DecodeError> {
        let mut i: u64 = 0;
        loop {
            let Some(next) = self.peek() else {
                return Err(DecodeError::UnexpectedEof);
            };
            if next == 0xff {
                self.off += 1;
                break;
            }
            self.item(depth)?;
            i += 1;
            if ty == CborType::Array {
                if i > MAX_ARRAY_ELEMENTS {
                    return Err(DecodeError::MaxArray);
                }
            } else if i.is_multiple_of(2) && i / 2 > MAX_MAP_PAIRS {
                return Err(DecodeError::MaxMap);
            }
        }
        if ty == CborType::Map && i % 2 == 1 {
            return Err(DecodeError::UnexpectedBreak);
        }
        Ok(())
    }

    /// `wellformedHead`. Floats of every width, NaN and Inf are acceptable with `DecOptions{}`.
    fn head(&mut self) -> Result<Head, DecodeError> {
        let Some(first) = self.peek() else {
            return Err(DecodeError::UnexpectedEof);
        };
        self.off += 1;
        let ty = CborType::of_initial_byte(first);
        let ai = first & 0x1f;
        let size = match ai {
            0..=23 => {
                return Ok(Head {
                    ty,
                    ai,
                    val: u64::from(ai),
                });
            }
            24 => 1,
            25 => 2,
            26 => 4,
            27 => 8,
            31 => {
                return match ty {
                    CborType::PositiveInteger | CborType::NegativeInteger | CborType::Tag => {
                        Err(DecodeError::InvalidAi { ai, ty })
                    }
                    // 0xff (break code) outside an indefinite-length item.
                    CborType::Primitives => Err(DecodeError::UnexpectedBreak),
                    _ => Ok(Head {
                        ty,
                        ai,
                        val: u64::from(ai),
                    }),
                };
            }
            // 28, 29, 30
            _ => return Err(DecodeError::InvalidAi { ai, ty }),
        };
        let Some(arg) = self.data.get(self.off..self.off + size) else {
            return Err(DecodeError::UnexpectedEof);
        };
        let val = arg.iter().fold(0u64, |v, &b| v << 8 | u64::from(b));
        self.off += size;
        if ai == 24 && ty == CborType::Primitives && val < 32 {
            return Err(DecodeError::InvalidSimple(val as u8));
        }
        Ok(Head { ty, ai, val })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        let s: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
        s.chunks(2)
            .map(|p| {
                let digit = |c: u8| (c as char).to_digit(16).map(|d| d as u8).unwrap_or(0);
                digit(p[0]) << 4 | digit(p[1])
            })
            .collect()
    }

    fn check(input: &str) -> Result<(), DecodeError> {
        well_formed(&hex(input))
    }

    #[test]
    fn empty_and_truncated() {
        assert_eq!(check(""), Err(DecodeError::Eof));
        for truncated in [
            "18",
            "19ff",
            "1a0000",
            "1b00000000000000",
            "41",
            "5f",
            "5f41",
            "81",
            "a1",
            "a100",
            "c6",
            "d9d9",
            "9f",
            "bf00",
            "f9",
            "fa0000",
            "fb00",
        ] {
            assert_eq!(
                check(truncated),
                Err(DecodeError::UnexpectedEof),
                "input {truncated}"
            );
        }
    }

    #[test]
    fn additional_information() {
        for major in 0u8..8 {
            for ai in [28u8, 29, 30] {
                let b = [major << 5 | ai];
                assert_eq!(
                    well_formed(&b),
                    Err(DecodeError::InvalidAi {
                        ai,
                        ty: CborType::of_initial_byte(b[0])
                    })
                );
            }
        }
        for b in [0x1f, 0x3f, 0xdf] {
            assert_eq!(
                well_formed(&[b]),
                Err(DecodeError::InvalidAi {
                    ai: 31,
                    ty: CborType::of_initial_byte(b)
                })
            );
        }
        assert_eq!(check("ff"), Err(DecodeError::UnexpectedBreak));
        assert_eq!(check("81ff"), Err(DecodeError::UnexpectedBreak));
        for v in 0u8..32 {
            assert_eq!(well_formed(&[0xf8, v]), Err(DecodeError::InvalidSimple(v)));
        }
        assert_eq!(check("f820"), Ok(()));
    }

    #[test]
    fn lengths() {
        assert_eq!(
            check("5b8000000000000000"),
            Err(DecodeError::StrLenOverflow {
                ty: CborType::ByteString,
                n: 1 << 63
            })
        );
        assert_eq!(
            check("7bffffffffffffffff"),
            Err(DecodeError::StrLenOverflow {
                ty: CborType::TextString,
                n: u64::MAX
            })
        );
        assert_eq!(check("5b7fffffffffffffff"), Err(DecodeError::UnexpectedEof));
        assert_eq!(
            check("9b8000000000000000"),
            Err(DecodeError::LenOverflow {
                ty: CborType::Array,
                n: 1 << 63
            })
        );
        assert_eq!(
            check("bb8000000000000000"),
            Err(DecodeError::LenOverflow {
                ty: CborType::Map,
                n: 1 << 63
            })
        );
        // Counts are checked right after the head, before any element.
        assert_eq!(check("9a00020000"), Err(DecodeError::UnexpectedEof));
        assert_eq!(check("9a00020001"), Err(DecodeError::MaxArray));
        assert_eq!(check("ba00020000"), Err(DecodeError::UnexpectedEof));
        assert_eq!(check("ba00020001"), Err(DecodeError::MaxMap));
        assert_eq!(check("a1049a00020001"), Err(DecodeError::MaxArray));
        assert_eq!(check("9b7fffffffffffffff"), Err(DecodeError::MaxArray));
    }

    #[test]
    fn indefinite_limits() {
        let mut arr = vec![0x9f];
        arr.extend(std::iter::repeat_n(0x00, 131072));
        arr.push(0xff);
        assert_eq!(well_formed(&arr), Ok(()));
        arr.insert(1, 0x00);
        assert_eq!(well_formed(&arr), Err(DecodeError::MaxArray));

        let mut map = vec![0xbf];
        map.extend(std::iter::repeat_n(0x00, 2 * 131072));
        map.push(0xff);
        assert_eq!(well_formed(&map), Ok(()));
        // An odd item count is a break error, even beyond the pair limit.
        map.insert(1, 0x00);
        assert_eq!(well_formed(&map), Err(DecodeError::UnexpectedBreak));
        map.insert(1, 0x00);
        assert_eq!(well_formed(&map), Err(DecodeError::MaxMap));
        assert_eq!(check("bf00ff"), Err(DecodeError::UnexpectedBreak));
    }

    #[test]
    fn indefinite_strings() {
        assert_eq!(check("5f41614162ff"), Ok(()));
        assert_eq!(check("5fff"), Ok(()));
        assert_eq!(
            check("5f6161ff"),
            Err(DecodeError::ChunkType {
                chunk: CborType::TextString,
                ty: CborType::ByteString
            })
        );
        assert_eq!(
            check("7f00ff"),
            Err(DecodeError::ChunkType {
                chunk: CborType::PositiveInteger,
                ty: CborType::TextString
            })
        );
        assert_eq!(
            check("5f5fffff"),
            Err(DecodeError::ChunkIndef(CborType::ByteString))
        );
        assert_eq!(
            check("5f5cff"),
            Err(DecodeError::InvalidAi {
                ai: 28,
                ty: CborType::ByteString
            })
        );
    }

    #[test]
    fn nesting_boundaries() {
        let nested = |prefix: &[u8], item: u8, n: usize, content: &[u8]| {
            let mut b = prefix.to_vec();
            b.extend(std::iter::repeat_n(item, n));
            b.extend_from_slice(content);
            well_formed(&b)
        };
        // Under a top-level map: 31 nested arrays pass, 32 fail.
        assert_eq!(nested(&[0xa1, 0x09], 0x81, 31, &[0x01]), Ok(()));
        assert_eq!(
            nested(&[0xa1, 0x09], 0x81, 32, &[0x01]),
            Err(DecodeError::MaxNested)
        );
        // Tags: the first tag of a chain adds no level.
        assert_eq!(nested(&[0xa1, 0x09], 0xc0, 32, &[0x01]), Ok(()));
        assert_eq!(
            nested(&[0xa1, 0x09], 0xc0, 33, &[0x01]),
            Err(DecodeError::MaxNested)
        );
        assert_eq!(
            nested(&[0xa1, 0x09], 0xc0, 1_000_000, &[0x01]),
            Err(DecodeError::MaxNested)
        );
        assert_eq!(nested(&[], 0x81, 32, &[0x01]), Ok(()));
        assert_eq!(nested(&[], 0x81, 33, &[0x01]), Err(DecodeError::MaxNested));
        assert_eq!(nested(&[], 0xc6, 33, &[0x01]), Ok(()));
        assert_eq!(nested(&[], 0xc6, 34, &[0x01]), Err(DecodeError::MaxNested));
        // A deep hostile document fails cleanly.
        assert_eq!(nested(&[], 0x9f, 100_000, &[]), Err(DecodeError::MaxNested));
    }

    #[test]
    fn extraneous_data() {
        assert_eq!(
            check("a0a0"),
            Err(DecodeError::Extraneous { n: 1, index: 1 })
        );
        assert_eq!(
            check("a1000501"),
            Err(DecodeError::Extraneous { n: 1, index: 3 })
        );
        // Extraneous data is only checked after the item is well-formed.
        assert_eq!(
            check("5c00"),
            Err(DecodeError::InvalidAi {
                ai: 28,
                ty: CborType::ByteString
            })
        );
        assert_eq!(
            check("f93e00fb3fb999999999999a"),
            Err(DecodeError::Extraneous { n: 9, index: 3 })
        );
    }
}
