//! Go standard-library behaviour that dstore observes (go1.26.5), so texts come out byte for byte:
//!
//! - `strconv` and `unicode` (`quote`, `strconv`), `strings` (`strings`);
//! - `time` (`time`), `encoding/json` v1 (`json`), `encoding/hex` (`hex`), `encoding/base32` (`base32`);
//! - `syscall` errno texts and `*fs.PathError` (`errno`), `path`/`path/filepath` (`path`), `os` (`os`);
//! - `fmt` verbs and `errors.Join` (`fmt`), `context` (`ctx`) and `log/slog` (`slog`).
//!
//! Spec: PORTING.md §4.1.
#![allow(dead_code, unused_variables)] // L0 stubs: remove once the modules are implemented
#![deny(unsafe_op_in_unsafe_fn)]

pub mod base32;
pub mod ctx;
pub mod errno;
mod errno_tables;
pub mod fmt;
pub mod hex;
pub mod json;
pub mod os;
pub mod path;
pub mod quote;
pub mod slog;
pub mod strconv;
pub mod strings;
mod tables;
pub mod time;
