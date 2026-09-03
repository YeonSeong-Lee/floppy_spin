#![forbid(unsafe_code)]

pub mod runtime;

pub use floppy_core::session::SessionDigest;
pub use runtime::{
    AppRuntime, ExitReason, FrameView, PlaybackCursor, PresentationEvent, PresentationEvents,
    RuntimeConfig, RuntimeEffects, RuntimeError, ShutdownEffects,
};
