//! Linear-segment ADSR envelope, counted in samples (not seconds/ms — the
//! caller converts once via [`crate::ms_to_samples`]). Every segment's
//! per-sample delta is precomputed once (on trigger, or on `note_off` for the
//! release delta) so `tick()` is a single add plus a countdown decrement —
//! no division and no transcendentals in the per-sample path (perf budget in
//! the milestone brief).
//!
//! Segment boundaries snap the value to the *exact* target (`1.0` at the end
//! of attack, `sustain` at the end of decay, `0.0` at the end of release)
//! instead of letting the accumulated float adds drift — this is what makes
//! "reaches 1.0 exactly at end of attack" and "release reaches exactly 0" a
//! precise guarantee rather than an approximation.

/// ADSR timing/level parameters. `attack`/`decay`/`release` are sample
/// counts; `sustain` is the held level in `0..=1`. A segment length of `0`
/// means "skip immediately to the next stage" (no divide-by-zero: the
/// per-sample delta for a zero-length segment is never computed or used).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdsrParams {
    pub attack: u32,
    pub decay: u32,
    pub sustain: f32,
    pub release: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnvState {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
    Done,
}

#[derive(Clone, Copy, Debug)]
pub struct Env {
    params: AdsrParams,
    state: EnvState,
    value: f32,
    attack_delta: f32,
    decay_delta: f32,
    release_delta: f32,
    remaining: u32,
}

impl Default for Env {
    fn default() -> Self {
        Env::new()
    }
}

impl Env {
    pub fn new() -> Self {
        Env {
            params: AdsrParams {
                attack: 0,
                decay: 0,
                sustain: 1.0,
                release: 0,
            },
            state: EnvState::Idle,
            value: 0.0,
            attack_delta: 0.0,
            decay_delta: 0.0,
            release_delta: 0.0,
            remaining: 0,
        }
    }

    pub fn state(&self) -> EnvState {
        self.state
    }

    pub fn value(&self) -> f32 {
        self.value
    }

    /// A voice using this envelope is still producing sound (or ramping
    /// toward doing so) and should not be reused by the mixer's allocator.
    pub fn is_active(&self) -> bool {
        !matches!(self.state, EnvState::Idle | EnvState::Done)
    }

    /// (Re)start the envelope from `Attack` (or straight into `Decay`/
    /// `Sustain` if `attack`/`decay` are 0) with new params.
    pub fn trigger(&mut self, params: AdsrParams) {
        self.params = params;
        self.value = 0.0;
        if params.attack == 0 {
            self.value = 1.0;
            self.begin_decay();
        } else {
            self.attack_delta = 1.0 / params.attack as f32;
            self.remaining = params.attack;
            self.state = EnvState::Attack;
        }
    }

    fn begin_decay(&mut self) {
        if self.params.decay == 0 {
            self.value = self.params.sustain;
            self.state = Self::state_after_decay(self.params.sustain);
        } else {
            self.decay_delta = (self.params.sustain - 1.0) / self.params.decay as f32;
            self.remaining = self.params.decay;
            self.state = EnvState::Decay;
        }
    }

    /// What state decay hands off to. `sustain <= 0.0` is treated as "this
    /// envelope is a self-terminating one-shot" (the common case for
    /// percussive SFX recipes in `sfx.rs`, which decay straight to silence
    /// and are never explicitly `note_off()`'d): it goes straight to `Done`
    /// so the mixer frees the voice automatically, instead of holding
    /// forever at an inaudible `0.0` in `Sustain` (which would otherwise
    /// never release the pool slot without a `note_off` nobody ever calls).
    /// A genuinely positive `sustain` (pads, the spin hum) behaves as
    /// standard ADSR: it holds until `note_off`.
    fn state_after_decay(sustain: f32) -> EnvState {
        if sustain <= 0.0 {
            EnvState::Done
        } else {
            EnvState::Sustain
        }
    }

    /// Enter `Release` from wherever the envelope currently is (SPEC/module
    /// brief: "note_off() enters Release from anywhere"), releasing from the
    /// *current* value down to 0 over `params.release` samples. Works
    /// unconditionally — even from `Idle`/`Done` (value already 0, so the
    /// release is a fast no-op) or from mid-`Attack` (releases from whatever
    /// partial level attack had reached).
    pub fn note_off(&mut self) {
        if self.params.release == 0 {
            self.value = 0.0;
            self.state = EnvState::Done;
        } else {
            self.release_delta = -(self.value) / self.params.release as f32;
            self.remaining = self.params.release;
            self.state = EnvState::Release;
        }
    }

    /// Advance one sample and return the new envelope value.
    pub fn tick(&mut self) -> f32 {
        match self.state {
            EnvState::Idle | EnvState::Done => {
                self.value = 0.0;
            }
            EnvState::Attack => {
                self.value += self.attack_delta;
                self.remaining -= 1;
                if self.remaining == 0 {
                    self.value = 1.0; // snap exact, see module docs
                    self.begin_decay();
                }
            }
            EnvState::Decay => {
                self.value += self.decay_delta;
                self.remaining -= 1;
                if self.remaining == 0 {
                    self.value = self.params.sustain; // snap exact
                    self.state = Self::state_after_decay(self.params.sustain);
                }
            }
            EnvState::Sustain => {
                self.value = self.params.sustain;
            }
            EnvState::Release => {
                self.value += self.release_delta;
                self.remaining -= 1;
                if self.remaining == 0 {
                    self.value = 0.0; // snap exact
                    self.state = EnvState::Done;
                }
            }
        }
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test 3 (ADSR): attack reaches exactly 1.0 at its last sample.
    #[test]
    fn attack_reaches_exactly_one_at_end() {
        let mut env = Env::new();
        env.trigger(AdsrParams {
            attack: 10,
            decay: 20,
            sustain: 0.5,
            release: 30,
        });
        let mut last = 0.0;
        for _ in 0..10 {
            last = env.tick();
        }
        assert_eq!(last, 1.0);
        assert_eq!(env.state(), EnvState::Decay);
    }

    /// Test 3: decay reaches exactly `sustain` and holds there.
    #[test]
    fn decay_reaches_sustain_and_holds() {
        let mut env = Env::new();
        env.trigger(AdsrParams {
            attack: 4,
            decay: 8,
            sustain: 0.3,
            release: 5,
        });
        for _ in 0..4 {
            env.tick();
        }
        let mut last = 0.0;
        for _ in 0..8 {
            last = env.tick();
        }
        assert_eq!(last, 0.3);
        assert_eq!(env.state(), EnvState::Sustain);
        for _ in 0..100 {
            assert_eq!(env.tick(), 0.3);
        }
    }

    /// Test 3: release reaches exactly 0 after `release` samples.
    #[test]
    fn release_reaches_exactly_zero() {
        let mut env = Env::new();
        env.trigger(AdsrParams {
            attack: 2,
            decay: 2,
            sustain: 0.8,
            release: 16,
        });
        for _ in 0..4 {
            env.tick();
        }
        env.note_off();
        assert_eq!(env.state(), EnvState::Release);
        let mut last = 1.0;
        for _ in 0..16 {
            last = env.tick();
        }
        assert_eq!(last, 0.0);
        assert_eq!(env.state(), EnvState::Done);
        assert!(!env.is_active());
    }

    /// Test 3: `note_off` from mid-attack immediately begins a release from
    /// whatever partial level had been reached.
    #[test]
    fn note_off_from_attack_releases_from_partial_level() {
        let mut env = Env::new();
        env.trigger(AdsrParams {
            attack: 100,
            decay: 10,
            sustain: 0.5,
            release: 10,
        });
        for _ in 0..50 {
            env.tick();
        }
        let level_at_release = env.value();
        assert!(level_at_release > 0.0 && level_at_release < 1.0);
        env.note_off();
        assert_eq!(env.state(), EnvState::Release);
        let mut last = -1.0;
        for _ in 0..10 {
            last = env.tick();
        }
        assert_eq!(last, 0.0);
        assert_eq!(env.state(), EnvState::Done);
    }

    /// A percussive one-shot (sustain 0.0, no `note_off` ever called — the
    /// common shape for `sfx.rs` recipes) must reach `Done` on its own once
    /// decay finishes, so the mixer's pool can reclaim the voice without
    /// requiring an explicit `note_off`.
    #[test]
    fn zero_sustain_one_shot_self_terminates_without_note_off() {
        let mut env = Env::new();
        env.trigger(AdsrParams {
            attack: 2,
            decay: 8,
            sustain: 0.0,
            release: 0,
        });
        for _ in 0..2 {
            env.tick();
        }
        let mut last = 1.0;
        for _ in 0..8 {
            last = env.tick();
        }
        assert_eq!(last, 0.0);
        assert_eq!(env.state(), EnvState::Done);
        assert!(!env.is_active());
    }

    #[test]
    fn zero_length_segments_skip_immediately() {
        let mut env = Env::new();
        env.trigger(AdsrParams {
            attack: 0,
            decay: 0,
            sustain: 0.7,
            release: 0,
        });
        assert_eq!(env.state(), EnvState::Sustain);
        assert_eq!(env.value(), 0.7);
        env.note_off();
        assert_eq!(env.state(), EnvState::Done);
        assert_eq!(env.value(), 0.0);
    }
}
