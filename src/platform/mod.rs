//! Platform boundary: ALL unsafe/FFI lives below this module (SPEC C8).
//!
//! Exactly one backend is compiled per target and re-exported as `backend`, so
//! `main.rs` names no platform: it imports `platform::backend` and the two
//! backends expose an identical API. `win32` is the shipped one (SPEC C2);
//! `macos` is the cfg-gated safe dev backend (SPEC §2), not a ship gate.
#[cfg(windows)]
pub mod win32;
#[cfg(windows)]
pub use win32 as backend;

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "macos")]
pub use macos as backend;
