//! Per-voice oscillator: an integer `u32` phase accumulator plus a waveform
//! shape. All five shapes are pure functions of the current phase (or, for
//! noise, of an LFSR stepped from the phase), so a voice's output is a pure
//! function of `(waveform, phase, phase_inc)` — no per-sample transcendental
//! calls, no per-sample division (SPEC §5 determinism, perf budget in the
//! milestone brief).
//!
//! ## Phase accumulator
//! `phase: u32` wraps naturally (`wrapping_add`) at `2^32`, representing one
//! full waveform period. `phase_inc` is computed **once per frequency change**
//! (not per sample) as
//! `phase_inc = freq_hz * 2^32 / SAMPLE_RATE`,
//! in `f64` (so the multiply doesn't lose bits for the audio-rate frequencies
//! this crate ever uses — up to a few kHz against a 44.1 kHz rate) and then
//! truncated to `u32` via `as` (an exact, deterministic truncating cast, not
//! `.round()`, which is on the workspace's banned-call list — see
//! `crate` docs). Per sample, advancing the oscillator is one `u32` add.
//!
//! ## Sine table
//! A 1024-entry `f32` table is built lazily (`OnceLock`) by calling
//! `floppy_core::fixmath::sin` once per entry at construction time — the one
//! place in this crate a trig function is evaluated, and it's a qualified
//! `fixmath::` call so the workspace's `no_libm` source scan (which also
//! greps this crate) accepts it. Playback indexes the table directly by the
//! **top 10 bits of phase, with no interpolation**: at 1024 entries over a
//! 44.1 kHz sample rate that aliases audibly at higher pitches, which is
//! intentional — it's the same lo-fi "digital" sine character as early
//! wavetable chiptune hardware, not a bug.
//!
//! ## Noise
//! A 15-bit Fibonacci LFSR (`taps` at bit 14 and bit 13, XORed into the new
//! bit 0 — polynomial `x^15 + x^14 + 1`, a primitive trinomial, so the
//! sequence has maximal period `2^15 - 1 = 32767` and never revisits the
//! all-zero state) is stepped once every time the phase accumulator wraps
//! around, i.e. at a rate of `freq_hz` steps/second — "the divided rate
//! derived from freq" the milestone brief asks for. Every voice trigger
//! reseeds the LFSR to the same fixed nonzero constant (`1`), never from a
//! global RNG, so a given SFX/note is bit-identical on every play (SPEC §5).

use std::sync::OnceLock;

use crate::SAMPLE_RATE;

/// Oscillator waveform shape. `Square`'s `duty` is a coarse `0..=255`
/// fraction of the period spent "high": `phase < (duty << 24)` is the high
/// half of the cycle, so `duty = 128` is as close to an exact 50% duty cycle
/// as an 8-bit fraction gets (128/256 = 50.0% exactly), and `duty = 64` is
/// 25% exactly — the two values the in-crate duty test pins against.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Waveform {
    Square { duty: u8 },
    Saw,
    Triangle,
    Sine,
    Noise,
}

const SINE_TABLE_LEN: usize = 1024;
static SINE_TABLE: OnceLock<[f32; SINE_TABLE_LEN]> = OnceLock::new();

fn sine_table() -> &'static [f32; SINE_TABLE_LEN] {
    SINE_TABLE.get_or_init(|| {
        let mut table = [0.0f32; SINE_TABLE_LEN];
        let tau = 2.0 * std::f32::consts::PI;
        for (i, slot) in table.iter_mut().enumerate() {
            let theta = (i as f32) * tau / (SINE_TABLE_LEN as f32);
            *slot = floppy_core::fixmath::sin(theta);
        }
        table
    })
}

/// Fixed, non-RNG reseed value for the noise LFSR (SPEC §5 policy: no global
/// RNG state anywhere near a "pure function of state" audio path). Any
/// nonzero 15-bit value gives the maximal-length sequence; `1` is simplest.
const NOISE_SEED: u16 = 1;
const NOISE_MASK: u16 = 0x7FFF;

/// One Fibonacci-LFSR step: feedback = bit14 XOR bit13, shifted into bit0.
/// Polynomial `x^15 + x^14 + 1`; empirically verified maximal (period
/// 32767) by the in-crate test below.
fn lfsr_step(state: u16) -> u16 {
    let bit14 = (state >> 14) & 1;
    let bit13 = (state >> 13) & 1;
    let feedback = bit14 ^ bit13;
    ((state << 1) | feedback) & NOISE_MASK
}

/// A single oscillator: phase accumulator + waveform shape + (for `Noise`)
/// LFSR state.
#[derive(Clone, Copy, Debug)]
pub struct Osc {
    waveform: Waveform,
    phase: u32,
    phase_inc: u32,
    freq_hz: f32,
    noise_state: u16,
}

impl Osc {
    pub fn new(waveform: Waveform) -> Self {
        Osc {
            waveform,
            phase: 0,
            phase_inc: 0,
            freq_hz: 0.0,
            noise_state: NOISE_SEED,
        }
    }

    /// The frequency last passed to [`Osc::set_freq`] (kept alongside the
    /// derived `phase_inc` so callers — the mixer's per-block sweep update —
    /// can compute `freq + sweep_hz_per_s * dt` without maintaining their own
    /// shadow copy of the oscillator's frequency).
    pub fn freq(&self) -> f32 {
        self.freq_hz
    }

    pub fn waveform(&self) -> Waveform {
        self.waveform
    }

    pub fn set_waveform(&mut self, waveform: Waveform) {
        self.waveform = waveform;
    }

    /// Recompute `phase_inc` for a new frequency. One-time `f64` multiply/
    /// divide (documented in the module docs above); never called per-sample
    /// in the hot render loop — only on trigger and on the mixer's per-block
    /// (every 64 samples) sweep update.
    pub fn set_freq(&mut self, freq_hz: f32) {
        let freq_hz = freq_hz.max(0.0);
        self.freq_hz = freq_hz;
        let inc = (freq_hz as f64) * (4_294_967_296.0 / SAMPLE_RATE as f64);
        // Truncating cast (not `.round()`, which is banned outside fixmath —
        // see crate docs): deterministic, and the sub-Hz bias it introduces
        // is inaudible at audio-buffer lengths.
        self.phase_inc = inc as u32;
    }

    /// Reset phase to 0 and reseed noise to the fixed constant — called on
    /// every voice (re)trigger so a given SFX/note always starts from
    /// exactly the same waveform state (SPEC §5 determinism).
    pub fn retrigger(&mut self) {
        self.phase = 0;
        self.noise_state = NOISE_SEED;
    }

    /// Produce the next sample (`f32` in `[-1, 1]`) and advance the phase.
    pub fn next_sample(&mut self) -> f32 {
        let out = match self.waveform {
            Waveform::Square { duty } => {
                let threshold = (duty as u32) << 24;
                if self.phase < threshold {
                    1.0
                } else {
                    -1.0
                }
            }
            Waveform::Saw => {
                let t = (self.phase >> 8) as f32 * (1.0 / 16_777_216.0);
                t * 2.0 - 1.0
            }
            Waveform::Triangle => {
                let t = (self.phase >> 8) as f32 * (1.0 / 16_777_216.0);
                1.0 - 4.0 * (t - 0.5).abs()
            }
            Waveform::Sine => {
                // Top 10 bits of phase index the 1024-entry table directly —
                // no interpolation (module docs: intentional lo-fi aliasing).
                let idx = (self.phase >> 22) as usize;
                sine_table()[idx]
            }
            Waveform::Noise => {
                if self.noise_state & 1 == 1 {
                    1.0
                } else {
                    -1.0
                }
            }
        };

        let (new_phase, wrapped) = self.phase.overflowing_add(self.phase_inc);
        self.phase = new_phase;
        if wrapped && matches!(self.waveform, Waveform::Noise) {
            self.noise_state = lfsr_step(self.noise_state);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test 1 (square duty): at duty 128 (50%), exactly half of one period's
    /// samples are positive; at duty 64 (25%), exactly a quarter are.
    /// `phase_inc` is chosen so the period divides the sample count exactly,
    /// so this is an exact count, not an approximation.
    #[test]
    fn square_duty_50_and_25_percent() {
        for (duty, expected_positive_frac) in [(128u8, 0.5f64), (64u8, 0.25f64)] {
            let mut osc = Osc::new(Waveform::Square { duty });
            // One period = 2^32 in phase units. Pick phase_inc = 2^32 / N for
            // an N that divides evenly, so N samples cover exactly one period.
            let n: u32 = 1024;
            osc.phase_inc = u32::MAX / n + 1; // 2^32 / n, exact since n is a power of 2
            let mut positive = 0u32;
            for _ in 0..n {
                if osc.next_sample() > 0.0 {
                    positive += 1;
                }
            }
            let expected = (n as f64 * expected_positive_frac) as u32;
            assert_eq!(
                positive, expected,
                "duty={duty} expected {expected} positive samples out of {n}, got {positive}"
            );
        }
    }

    /// Test 2 (LFSR): the 15-bit sequence has period exactly 32767 and never
    /// visits the all-zero state.
    #[test]
    fn lfsr_period_is_32767_and_never_zero() {
        let mut state = NOISE_SEED;
        let start = state;
        for step in 1..=32767u32 {
            state = lfsr_step(state);
            assert_ne!(state, 0, "LFSR hit the all-zero state at step {step}");
            if state == start {
                assert_eq!(step, 32767, "LFSR period was {step}, expected 32767");
                return;
            }
        }
        panic!("LFSR did not return to its start state within 32767 steps");
    }

    #[test]
    fn saw_ramps_from_negative_one_to_positive_one() {
        let mut osc = Osc::new(Waveform::Saw);
        osc.phase_inc = 1 << 20; // small step, many samples per period
        let first = osc.next_sample();
        assert!((-1.0..-0.99).contains(&first));
    }

    #[test]
    fn triangle_folds_symmetrically() {
        let mut osc = Osc::new(Waveform::Triangle);
        osc.phase = 0;
        osc.phase_inc = 0;
        assert!((osc.next_sample() - (-1.0)).abs() < 1e-6);
        osc.phase = 1u32 << 31; // halfway through the period
        osc.phase_inc = 0;
        assert!((osc.next_sample() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn sine_table_matches_fixmath_sin_at_zero_phase() {
        let mut osc = Osc::new(Waveform::Sine);
        osc.phase = 0;
        osc.phase_inc = 0;
        assert!((osc.next_sample() - 0.0).abs() < 1e-3);
    }
}
