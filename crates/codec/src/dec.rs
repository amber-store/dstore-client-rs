//! Pass 2: per-field decoding (fxamacker `decode.go` subset) and `unmarshal`.

use crate::{DecodeError, Struct};

/// Pass-2 cursor over input that pass 1 already validated.
pub struct Dec<'a> {
    data: &'a [u8],
    off: usize,
}

impl<'a> Dec<'a> {
    pub fn peek_major(&self) -> u8 {
        todo!()
    }

    /// `f6` / `f7` consumed → true.
    pub fn take_null(&mut self) -> bool {
        todo!()
    }

    /// D2.
    pub fn tag_preamble(&mut self) -> Result<(), DecodeError> {
        todo!()
    }

    /// D3/D4/D8.
    pub fn read_uint(&mut self, go_type: &'static str, max: u64) -> Result<u64, DecodeError> {
        todo!()
    }

    /// D3/D4/D5/D8.
    pub fn read_int(
        &mut self,
        go_type: &'static str,
        min: i64,
        max: i64,
    ) -> Result<i64, DecodeError> {
        todo!()
    }

    pub fn read_bool(&mut self, go_type: &'static str) -> Result<bool, DecodeError> {
        todo!()
    }

    pub fn read_f64(&mut self, go_type: &'static str) -> Result<f64, DecodeError> {
        todo!()
    }

    /// D7, chunks, UTF-8 check.
    pub fn read_text(&mut self, go_type: &'static str) -> Result<String, DecodeError> {
        todo!()
    }

    /// D6, D9 (array of uint8), D3 bignum.
    pub fn read_bytes(&mut self, go_type: &'static str) -> Result<Vec<u8>, DecodeError> {
        todo!()
    }

    pub fn read_array<T>(
        &mut self,
        go_type: &'static str,
        elem: impl FnMut(&mut Dec<'a>) -> Result<T, DecodeError>,
    ) -> Result<Vec<T>, DecodeError> {
        todo!()
    }

    /// D10: key dispatch; `field` returns false for unknown keys (the value is skipped); duplicates of
    /// a matched key are skipped unexamined.
    pub fn read_map_struct(
        &mut self,
        go_type: &'static str,
        field: impl FnMut(&mut Dec<'a>, u64) -> Result<bool, DecodeError>,
    ) -> Result<(), DecodeError> {
        todo!()
    }

    pub fn skip(&mut self) {
        todo!()
    }
}

/// `codec.Unmarshal`: pass 1, then pass 2; a top-level null gives `T::default()`.
pub fn unmarshal<T: Struct>(b: &[u8]) -> Result<T, DecodeError> {
    todo!()
}
