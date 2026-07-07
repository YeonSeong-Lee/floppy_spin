//! FLOPPY SPIN procedural audio core (SPEC §8, game_design.md §8/§5) — M6-A.
//!
//! Synthesis only. This crate renders deterministic i16 mono samples from a
//! pure function of `(mixer/tracker state, triggers)`; it never touches wall
//! clock, threads, or `HashMap` (SPEC §5). The Windows `waveOut` playback
//! ring is a later milestone and lives in `platform::win32`, not here.
//!
//! ## Determinism (SPEC §5 / C5)
//! - No libm transcendentals anywhere in this crate: `sin`/`cos` are only
//!   reached through `floppy_core::fixmath::{sin,cos}` (never aliased, so the
//!   workspace `no_libm` source scan can see the qualified calls), used
//!   exactly once to build the [`osc`] sine lookup table lazily. Everything
//!   else is integer phase accumulators, linear interpolation-free table
//!   reads, and fixed-point-style integer/float arithmetic using only
//!   `+ - * /` and the IEEE-exact `.floor()/.abs()/.min()/.max()/.clamp()`
//!   family (see `floppy_core/tests/no_libm.rs`, which scans this crate's
//!   `src/` too).
//! - `.round()` and `.fract()` are in that same banned list (they are exact
//!   per IEEE-754 but banned anyway by the shared scanner's blocklist), so
//!   this crate always reaches the equivalent behavior via `as` truncating
//!   casts or explicit integer division instead — documented at each call
//!   site.
//! - No `HashMap`, no wall-clock reads, no thread IDs. `Mixer`/`Tracker`
//!   render is a pure function of their own state plus the samples/events
//!   handed to them by the caller.
//!
//! ## Module map
//! - [`osc`]: `Waveform` + the per-voice oscillator (u32 phase accumulator).
//! - [`env`]: linear-segment ADSR envelope, sample-counted.
//! - [`mixer`]: the 16-voice `Mixer`, voice stealing, soft-clip, final i16 mix.
//! - [`sfx`]: the `Sfx` enum (waveform recipes) and `BattleEvent` → SFX wiring.
//! - [`tracker`]: the 4-channel music tracker (menu + battle themes).

#![forbid(unsafe_code)]

pub mod env;
pub mod mixer;
pub mod osc;
pub mod sfx;
pub mod tracker;

/// Fixed mono sample rate (SPEC §8). The platform layer duplicates to stereo.
pub const SAMPLE_RATE: u32 = 44_100;

// Re-exports: the handful of types a caller (game loop / platform layer)
// actually needs to wire this crate up, without reaching into submodules.
pub use env::AdsrParams;
pub use mixer::{Mixer, VoiceParams};
pub use osc::Waveform;
pub use sfx::{on_event, play, Sfx};
pub use tracker::{SongId, Tracker};

/// Convert a millisecond duration to a whole number of samples at
/// [`SAMPLE_RATE`], truncating. `.round()` is on the workspace's banned-call
/// list (see module docs above) even though it would be exactly rounded, so
/// every ms→samples conversion in this crate goes through this one integer
/// division instead of ever calling a float rounding method.
pub(crate) const fn ms_to_samples(ms: u32) -> u32 {
    (ms * SAMPLE_RATE) / 1000
}

/// Build an [`AdsrParams`] from millisecond timings via [`ms_to_samples`].
/// Shared by `sfx.rs`'s one-shot recipes and `tracker.rs`'s per-instrument
/// envelopes so both convert ms -> samples exactly the same way.
pub(crate) fn adsr_ms(attack_ms: u32, decay_ms: u32, sustain: f32, release_ms: u32) -> AdsrParams {
    AdsrParams {
        attack: ms_to_samples(attack_ms),
        decay: ms_to_samples(decay_ms),
        sustain,
        release: ms_to_samples(release_ms),
    }
}
