//! `strconv.Quote` (`strconv/quote.go`, `isprint.go`) and `unicode.IsSpace`, go1.26.5.

/// `strconv.Quote` over the bytes of a Go string: `\xNN` for each invalid UTF-8 byte.
pub fn quote(s: &[u8]) -> String {
    todo!()
}

/// `strconv.IsPrint`.
pub fn is_print(r: char) -> bool {
    todo!()
}

/// `unicode.IsSpace`.
pub fn is_space(r: char) -> bool {
    todo!()
}
