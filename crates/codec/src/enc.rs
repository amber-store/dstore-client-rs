//! Canonical encoding (`CanonicalEncOptions`): shortest heads, ascending keys, shortest exact floats.

/// A CBOR output buffer.
pub struct Enc {
    buf: Vec<u8>,
}

impl Enc {
    pub fn new() -> Enc {
        todo!()
    }

    pub fn into_bytes(self) -> Vec<u8> {
        todo!()
    }

    /// Shortest head (= `amber_store_core::cbor::append_head`).
    pub fn head(&mut self, major: u8, n: u64) {
        todo!()
    }

    pub fn uint(&mut self, v: u64) {
        todo!()
    }

    /// Major type 1 with -1-v when v < 0.
    pub fn int(&mut self, v: i64) {
        todo!()
    }

    pub fn bool(&mut self, v: bool) {
        todo!()
    }

    /// `0xf6`.
    pub fn null(&mut self) {
        todo!()
    }

    pub fn bytes(&mut self, v: &[u8]) {
        todo!()
    }

    pub fn text(&mut self, v: &str) {
        todo!()
    }

    /// E11: NaN `f97e00`, ±Inf `f97c00`/`f9fc00`, then float16/32/64, the shortest exact form.
    pub fn f64_canonical(&mut self, v: f64) {
        todo!()
    }
}

impl Default for Enc {
    fn default() -> Enc {
        Enc::new()
    }
}

/// A value that encodes itself canonically.
pub trait Encode {
    fn encode(&self, e: &mut Enc);
}

/// `codec.Marshal`.
pub fn marshal<T: Encode + ?Sized>(v: &T) -> Vec<u8> {
    todo!()
}
