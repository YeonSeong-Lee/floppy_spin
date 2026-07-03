//! Platform boundary: ALL unsafe/FFI lives below this module (SPEC C8).
#[cfg(windows)]
pub mod win32;
