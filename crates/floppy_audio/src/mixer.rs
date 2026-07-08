//! The 16-voice mixer: allocation/stealing, per-block frequency sweeps,
//! summing, master gain, soft-clip, and the final `f32 -> i16` conversion.
//!
//! ## Voice budget (documented split of the 16 voices)
//! - `0..SFX_POOL_SIZE` (0..11): the generic one-shot SFX pool. [`Mixer::trigger`]
//!   allocates *only* from this range.
//! - `SPIN_HUM_VOICE` (11): a single voice dedicated to the spin-hum loop
//!   (`sfx::Sfx::SpinHum`), addressed only via [`Mixer::trigger_at`] /
//!   [`Mixer::retune_voice`] — it never enters the generic pool, so a run of
//!   one-shot SFX can never steal or be stolen by the hum.
//! - `MUSIC_VOICE_BASE..MUSIC_VOICE_BASE+NUM_MUSIC_CHANNELS` (12..16): the
//!   tracker's 4 channels, addressed only via [`Mixer::trigger_at`]. These
//!   are **never** visible to [`Mixer::trigger`]'s allocator, in either its
//!   first-fit or its steal path — a full reservation, not just steal-time
//!   protection, per the milestone brief ("reserve voices 12..16 for the
//!   tracker, never stolen by SFX").
//!
//! ## Voice stealing
//! When the SFX pool is full, [`Mixer::trigger`] evicts the voice with the
//! lowest `priority`; ties break to the *oldest* (smallest trigger-time
//! `age` stamp, i.e. `sample_clock` at the moment it was triggered); further
//! ties (identical priority *and* age — only possible if two voices were
//! triggered on the exact same sample) break to the lowest voice index,
//! simply because the scan below only replaces its current-best candidate on
//! a strict improvement, so the first (lowest-index) voice found at a given
//! `(priority, age)` is never displaced by an equal one found later.
//!
//! ## Soft-clip
//! `y = x*(27+x*x)/(27+9*x*x)` for `x` clamped to `[-3, 3]` first (so the
//! formula's own saturation at `|x|=3 -> |y|=1` handles "clamped beyond"
//! automatically — no separate branch needed). This is a standard
//! polynomial/rational soft saturator: smoothly compresses toward `±1`
//! instead of hard-clipping, which is kinder on a chiptune-style mix with
//! several voices summed at once.

use crate::env::{AdsrParams, Env};
use crate::osc::{Osc, Waveform};

/// Size of the generic SFX allocation pool (voices `0..SFX_POOL_SIZE`).
pub const SFX_POOL_SIZE: usize = 11;
/// The single voice dedicated to the spin-hum loop (see module docs).
pub const SPIN_HUM_VOICE: usize = 11;
/// First index of the 4 tracker-reserved voices.
pub const MUSIC_VOICE_BASE: usize = 12;
/// Number of tracker channels (pulse1, pulse2, bass, drums).
pub const NUM_MUSIC_CHANNELS: usize = 4;
/// Total voice count (SPEC §8).
pub const NUM_VOICES: usize = 16;

const _: () = assert!(MUSIC_VOICE_BASE + NUM_MUSIC_CHANNELS == NUM_VOICES);

/// Parameters to start (or restart) one voice.
#[derive(Clone, Copy, Debug)]
pub struct VoiceParams {
    pub waveform: Waveform,
    pub freq_hz: f32,
    /// Hz/second; applied once per 64-sample block (see [`Mixer::render`]),
    /// not per sample — cheap and still plenty smooth for chiptune SFX.
    pub sweep_hz_per_s: f32,
    pub adsr: AdsrParams,
    pub gain: f32,
    /// Steal priority: **higher = more important = survives longer**. Ties
    /// during a steal break to the oldest voice (see module docs).
    pub priority: u8,
}

#[derive(Clone, Copy, Debug)]
struct Voice {
    osc: Osc,
    env: Env,
    gain: f32,
    sweep_hz_per_s: f32,
    priority: u8,
    age: u64,
    active: bool,
}

impl Voice {
    fn silent() -> Self {
        Voice {
            osc: Osc::new(Waveform::Square { duty: 128 }),
            env: Env::new(),
            gain: 0.0,
            sweep_hz_per_s: 0.0,
            priority: 0,
            age: 0,
            active: false,
        }
    }

    fn start(&mut self, params: VoiceParams, age: u64) {
        self.osc.set_waveform(params.waveform);
        self.osc.set_freq(params.freq_hz);
        self.osc.retrigger();
        self.env.trigger(params.adsr);
        self.gain = params.gain;
        self.sweep_hz_per_s = params.sweep_hz_per_s;
        self.priority = params.priority;
        self.age = age;
        self.active = true;
    }

    fn apply_sweep_block(&mut self, block_samples: u32) {
        if self.sweep_hz_per_s == 0.0 {
            return;
        }
        let dt = block_samples as f32 / crate::SAMPLE_RATE as f32;
        let new_freq = (self.osc.freq() + self.sweep_hz_per_s * dt).max(0.0);
        self.osc.set_freq(new_freq);
    }

    fn render_sample(&mut self) -> f32 {
        let osc_val = self.osc.next_sample();
        let env_val = self.env.tick();
        if !self.env.is_active() {
            self.active = false;
        }
        osc_val * env_val * self.gain
    }
}

/// The 16-voice mixer (SPEC §8).
pub struct Mixer {
    voices: [Voice; NUM_VOICES],
    sample_clock: u64,
    /// Master gain applied before soft-clip.
    pub master: f32,
    /// Group gain applied to voices `0..MUSIC_VOICE_BASE` (the SFX pool +
    /// spin-hum voice) at mix time — see [`Mixer::set_group_gains`].
    sfx_gain: f32,
    /// Group gain applied to voices `MUSIC_VOICE_BASE..NUM_VOICES` (the
    /// tracker's 4 channels) at mix time — see [`Mixer::set_group_gains`].
    music_gain: f32,
}

/// How often (in samples) frequency sweeps are re-applied — see module docs
/// on [`VoiceParams::sweep_hz_per_s`].
const SWEEP_BLOCK_SAMPLES: u32 = 64;

/// Fixed i16 scale factor. Soft-clip already bounds its output to `[-1, 1]`,
/// so `32767.0` (not `32768.0`) keeps the conversion symmetric and never
/// needs the negative extra headroom `i16::MIN` would allow.
const I16_SCALE: f32 = 32767.0;

impl Default for Mixer {
    fn default() -> Self {
        Mixer::new()
    }
}

impl Mixer {
    pub fn new() -> Self {
        Mixer {
            voices: [Voice::silent(); NUM_VOICES],
            sample_clock: 0,
            master: 0.85,
            sfx_gain: 1.0,
            music_gain: 1.0,
        }
    }

    /// Volume-settings hook (SPEC §7's `music_vol`/`sfx_vol`, integer scale
    /// zero to ten; main.rs converts to a `f32` fraction): scales voices
    /// below `MUSIC_VOICE_BASE` (the SFX pool plus the spin-hum voice) by
    /// `sfx`, and voices from `MUSIC_VOICE_BASE` up to `NUM_VOICES` (the
    /// tracker's four channels) by `music`, applied per-sample in
    /// [`Mixer::render`] before `master`/soft-clip. A pure `f32` multiply
    /// keyed only on voice index — no wall-clock, no branching on anything
    /// but the fixed group boundary — so this stays fully deterministic
    /// (SPEC §5) and reproducible in headless WAV renders. Defaults to
    /// `1.0`/`1.0` (see [`Mixer::new`]), so every pre-existing test/caller
    /// that never calls this is unaffected (multiplying by `1.0` is an exact
    /// no-op per IEEE-754).
    pub fn set_group_gains(&mut self, sfx: f32, music: f32) {
        self.sfx_gain = sfx;
        self.music_gain = music;
    }

    pub fn sample_clock(&self) -> u64 {
        self.sample_clock
    }

    /// `true` if voice `index` currently has an active envelope. Panics if
    /// `index >= NUM_VOICES` (programmer error, not a runtime data path).
    pub fn voice_active(&self, index: usize) -> bool {
        self.voices[index].active
    }

    /// Test-only introspection: the frequency currently loaded into a
    /// voice's oscillator, used to confirm *which* voice a steal actually
    /// landed on (an "active" bool alone can't distinguish "still the old
    /// occupant" from "freshly retriggered").
    #[cfg(test)]
    fn voice_freq(&self, index: usize) -> f32 {
        self.voices[index].osc.freq()
    }

    /// Allocate a voice for a one-shot SFX from the pool `0..SFX_POOL_SIZE`
    /// only (see module docs — voice 11 and 12..16 are never touched here).
    /// First fit among currently-idle pool voices; if all are busy, steals
    /// the lowest-priority (ties: oldest, then lowest index) voice.
    pub fn trigger(&mut self, params: VoiceParams) {
        let age = self.sample_clock;
        if let Some(idx) = (0..SFX_POOL_SIZE).find(|&i| !self.voices[i].active) {
            self.voices[idx].start(params, age);
            return;
        }
        let mut victim = 0usize;
        let mut best_priority = self.voices[0].priority;
        let mut best_age = self.voices[0].age;
        for i in 1..SFX_POOL_SIZE {
            let v = &self.voices[i];
            if v.priority < best_priority || (v.priority == best_priority && v.age < best_age) {
                victim = i;
                best_priority = v.priority;
                best_age = v.age;
            }
        }
        self.voices[victim].start(params, age);
    }

    /// Directly (re)trigger a specific voice, bypassing the pool allocator —
    /// the only way to reach the dedicated hum voice ([`SPIN_HUM_VOICE`]) or
    /// the tracker's reserved voices (`MUSIC_VOICE_BASE..`).
    pub fn trigger_at(&mut self, index: usize, params: VoiceParams) {
        let age = self.sample_clock;
        self.voices[index].start(params, age);
    }

    /// Retune a currently-sounding voice to a new frequency *without*
    /// resetting its phase or envelope — used by the spin-hum SFX to glide
    /// with RPM instead of clicking on every update (see `sfx.rs`).
    pub fn retune_voice(&mut self, index: usize, freq_hz: f32) {
        self.voices[index].osc.set_freq(freq_hz);
    }

    /// Explicitly enter a voice's envelope into `Release` (see
    /// [`Env::note_off`]) — used by the spin-hum SFX to fade out cleanly
    /// when RPM reaches 0, instead of holding its sustain forever.
    pub fn release_voice(&mut self, index: usize) {
        self.voices[index].env.note_off();
    }

    /// Render `out.len()` samples, deterministically summing all 16 voices
    /// in index order 0..16, applying master gain, soft-clipping, and
    /// truncating to i16.
    pub fn render(&mut self, out: &mut [i16]) {
        for slot in out.iter_mut() {
            if self.sample_clock > 0 && self.sample_clock.is_multiple_of(SWEEP_BLOCK_SAMPLES as u64)
            {
                for v in self.voices.iter_mut() {
                    if v.active {
                        v.apply_sweep_block(SWEEP_BLOCK_SAMPLES);
                    }
                }
            }

            let mut sum = 0.0f32;
            for (idx, v) in self.voices.iter_mut().enumerate() {
                if v.active {
                    let group_gain = if idx < MUSIC_VOICE_BASE {
                        self.sfx_gain
                    } else {
                        self.music_gain
                    };
                    sum += v.render_sample() * group_gain;
                }
            }
            sum *= self.master;
            let clipped = soft_clip(sum);
            // Truncating cast: soft_clip already bounds |clipped| <= 1.0, so
            // the product is within [-32767, 32767], well inside i16 range —
            // this is a plain deterministic cast, not a saturating rescue.
            *slot = (clipped * I16_SCALE) as i16;

            self.sample_clock += 1;
        }
    }
}

/// Rational soft-clip: `y = x*(27+x*x)/(27+9*x*x)`, `x` clamped to `[-3,3]`
/// first (see module docs for why that's sufficient to bound `|y| <= 1`).
pub fn soft_clip(x: f32) -> f32 {
    let xc = x.clamp(-3.0, 3.0);
    xc * (27.0 + xc * xc) / (27.0 + 9.0 * xc * xc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::AdsrParams;

    fn params(priority: u8) -> VoiceParams {
        params_freq(priority, 440.0)
    }

    fn params_freq(priority: u8, freq_hz: f32) -> VoiceParams {
        VoiceParams {
            waveform: Waveform::Square { duty: 128 },
            freq_hz,
            sweep_hz_per_s: 0.0,
            adsr: AdsrParams {
                attack: 4,
                decay: 4,
                sustain: 1.0,
                release: 1_000_000, // long: stays active for the whole test
            },
            gain: 0.5,
            priority,
        }
    }

    /// Test 4 (mixer stealing + reservation): 17 total voice-starts across
    /// the whole 16-voice budget — 4 tracker channel starts + 1 hum start
    /// (both via `trigger_at`, i.e. never candidates for the generic
    /// allocator) + 13 generic `trigger()` calls into the 11-voice SFX pool
    /// — end with exactly 16 active voices (every voice in the mixer), the
    /// 13th pool call forced to steal a specific, deterministic victim, and
    /// the 5 reserved voices (11 hum + 12..16 music) confirmed untouched by
    /// the steal.
    #[test]
    fn stealing_is_deterministic_and_reserved_voices_are_never_touched() {
        let mut mixer = Mixer::new();

        // 4 music channels + 1 hum voice: direct, reserved.
        for i in MUSIC_VOICE_BASE..NUM_VOICES {
            mixer.trigger_at(i, params(255));
        }
        mixer.trigger_at(SPIN_HUM_VOICE, params(255));

        // Fill the 11-voice SFX pool with ascending priorities 0..10, so
        // voice 0 (priority 0) is unambiguously the lowest-priority, oldest
        // voice in the pool once it's full.
        for i in 0..SFX_POOL_SIZE {
            mixer.trigger(params(i as u8));
        }
        for i in 0..NUM_VOICES {
            assert!(mixer.voice_active(i), "voice {i} should be active");
        }

        // 13th generic trigger (all pool voices busy): must steal voice 0
        // (lowest priority, 0, in the pool), not any reserved voice.
        mixer.trigger(params_freq(200, 12345.0));
        assert_eq!(
            mixer.voice_freq(0),
            12345.0,
            "steal should land on voice 0 (priority 0)"
        );

        // Reserved voices were never candidates: still exactly the voices we
        // put there, still active, still 16 active total.
        for i in MUSIC_VOICE_BASE..NUM_VOICES {
            assert!(mixer.voice_active(i));
        }
        assert!(mixer.voice_active(SPIN_HUM_VOICE));
        let active_count = (0..NUM_VOICES).filter(|&i| mixer.voice_active(i)).count();
        assert_eq!(active_count, 16);
    }

    /// Tie-break rule: equal priority breaks to oldest (smallest trigger-time
    /// `age`), then to lowest index. Fill the pool in order (voice `i` is
    /// always strictly older than voice `i+1`, since `age` is stamped from
    /// `sample_clock` and we advance the clock between triggers) with equal
    /// priority and distinct, checkable frequencies; the first contended
    /// steal must land on voice 0 (oldest), the second on voice 1
    /// (next-oldest), confirmed by inspecting each voice's retuned
    /// frequency rather than just its `active` flag.
    #[test]
    fn tie_break_prefers_oldest_then_lowest_index() {
        let mut mixer = Mixer::new();
        let mut buf = [0i16; 1];
        for i in 0..SFX_POOL_SIZE {
            mixer.trigger(params_freq(50, 100.0 + i as f32));
            mixer.render(&mut buf); // advance sample_clock so ages differ
        }

        mixer.trigger(params_freq(50, 999.0));
        assert_eq!(
            mixer.voice_freq(0),
            999.0,
            "first steal should evict voice 0 (oldest)"
        );

        mixer.render(&mut buf);
        mixer.trigger(params_freq(50, 888.0));
        assert_eq!(
            mixer.voice_freq(1),
            888.0,
            "second steal should evict voice 1 (next-oldest, now that voice 0 is freshest)"
        );
    }

    /// Test 5 (soft-clip): driving all 16 voices at an absurd gain (10x)
    /// never overflows i16, checked via a widening i32 cast so the check
    /// itself can't silently rely on `i16`'s own wraparound.
    #[test]
    fn soft_clip_never_overflows_i16_even_at_absurd_gain() {
        let mut mixer = Mixer::new();
        mixer.master = 10.0;
        for i in 0..NUM_VOICES {
            let mut p = params(200);
            p.gain = 10.0;
            mixer.trigger_at(i, p);
        }
        let mut buf = [0i16; 2_048];
        mixer.render(&mut buf);
        for &s in buf.iter() {
            let widened = s as i32;
            assert!(widened.abs() <= i16::MAX as i32, "sample {s} overflowed");
        }
    }

    /// [`Mixer::set_group_gains`]: zeroing the SFX group must silence a
    /// pool-allocated voice while a tracker-channel voice (index
    /// `>= MUSIC_VOICE_BASE`) at the same gain keeps sounding, and vice
    /// versa — confirms the split is keyed on voice index at exactly
    /// `MUSIC_VOICE_BASE`, not some blanket master-gain effect.
    #[test]
    fn group_gains_scale_only_their_own_voice_range() {
        let mut mixer = Mixer::new();
        mixer.trigger(params(200)); // lands in the SFX pool (index 0).
        mixer.trigger_at(MUSIC_VOICE_BASE, params(200)); // a tracker channel.

        mixer.set_group_gains(0.0, 1.0);
        let mut buf = [0i16; 4];
        mixer.render(&mut buf);
        // Voice 0 (SFX) contributes nothing; voice MUSIC_VOICE_BASE (music)
        // still sounds, so the mix isn't silent.
        assert!(buf.iter().any(|&s| s != 0), "music group was wrongly muted");

        // Fresh mixer, opposite split: SFX audible, music muted.
        let mut mixer2 = Mixer::new();
        mixer2.trigger(params(200));
        mixer2.set_group_gains(1.0, 0.0);
        let mut buf_sfx_only = [0i16; 4];
        mixer2.render(&mut buf_sfx_only);
        assert!(
            buf_sfx_only.iter().any(|&s| s != 0),
            "SFX group was wrongly muted"
        );

        let mut mixer3 = Mixer::new();
        mixer3.trigger_at(MUSIC_VOICE_BASE, params(200));
        mixer3.set_group_gains(1.0, 0.0);
        let mut buf_music_muted = [0i16; 4];
        mixer3.render(&mut buf_music_muted);
        assert!(
            buf_music_muted.iter().all(|&s| s == 0),
            "music group should be fully silent at gain 0.0"
        );
    }

    #[test]
    fn trigger_never_allocates_reserved_voices_when_pool_contended() {
        let mut mixer = Mixer::new();
        // Hammer the generic allocator far past 16 total triggers; reserved
        // voices must stay untouched (still inactive — nothing ever put
        // sound there) the whole time.
        for _ in 0..64 {
            mixer.trigger(params(10));
        }
        for i in SPIN_HUM_VOICE..NUM_VOICES {
            assert!(
                !mixer.voice_active(i),
                "voice {i} (reserved) was touched by the generic SFX allocator"
            );
        }
    }
}
