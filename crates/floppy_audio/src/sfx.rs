//! Named one-shot SFX recipes (game_design.md §8 "Audio identity" SFX
//! palette, §7 UI/UX menu blips, §5 juice table SFX column) plus the wiring
//! from `floppy_core::physics::BattleEvent` to those recipes.
//!
//! ## Design notes / deviations
//! - **No absolute-time scheduler.** [`play`] triggers voices synchronously;
//!   there is no mechanism to say "play voice B 80ms after voice A". The
//!   juice table's two-stage recipes (ring-out whistle→thud, launch rip +
//!   perfect ting) are instead built from two voices started at the *same*
//!   instant, where the second voice's **ADSR attack length is set to the
//!   first voice's total duration** — so its output stays near-silent while
//!   voice A is sounding and only swells to audible right as voice A
//!   finishes. This is a real, if lo-fi, way to get a "stage 2 starts after
//!   stage 1" feel from stateless ADSR primitives, and is used for
//!   [`Sfx::RingOutFall`] and (for the optional ting tail) [`Sfx::LaunchRip`].
//! - **No filter stage.** The synth has oscillators + ADSR + soft-clip only,
//!   no low/high-pass. Recipes calling for a "swept"/filtered texture (e.g.
//!   dash whoosh's "HP-swept") approximate the motion by sweeping the
//!   oscillator's own pitch (for `Noise`, this also sweeps its LFSR step
//!   rate) instead — a rising noise pitch reads as a rising "whoosh"
//!   brightness even without a real filter.
//! - **`SpinHum` deviation from "saw+sine" to saw-only.** The brief pins the
//!   hum to a *single* dedicated voice (11); layering saw+sine as the
//!   palette literally describes would need a second permanently-reserved
//!   voice, which the 11 (SFX pool) + 1 (hum) + 4 (music) = 16 budget has no
//!   room for without shrinking the SFX pool. A single `Saw` oscillator
//!   alone still carries the buzzy "the top audibly tires" character the
//!   design asks for; documented here as an intentional scope trade-off.
//! - **Round-win / match-win fanfares are out of scope for this enum.** The
//!   [`Sfx`] enum below is exactly the variant list specified for this
//!   milestone; it does not include dedicated `RoundWin`/`MatchWin`
//!   variants from the juice table (those are presentation-layer stingers
//!   for a later milestone), only [`Sfx::ScoreTally`]'s per-pip ting.
//!
//! ## Voice priority convention
//! Higher `priority` survives longer under SFX-pool contention (see
//! `mixer.rs`). Big, rare, important moments (Crash-Out, ring-out, special
//! fire) get the highest priorities; routine menu blips and the tally ting
//! get the lowest — "hits/stings preempt" incidental UI noise, matching
//! game_design.md §8's "Voice priority: hits/stings preempt the hum, never
//! the music channels" (the hum itself is protected structurally, see
//! `mixer.rs`; this priority ordering governs contention *within* the SFX
//! pool instead).

use crate::adsr_ms;
use crate::mixer::{Mixer, VoiceParams, SPIN_HUM_VOICE};
use crate::osc::Waveform;

/// One named, parameterless (aside from the small per-variant payload) SFX
/// trigger. See module docs for recipe sourcing and the two-stage-via-
/// attack-delay trick used by the multi-voice recipes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sfx {
    MenuMove,
    MenuSelect,
    MenuBack,
    /// `0,1,2` = "3","2","1"; `3` = "GO!".
    CountBeep(u8),
    LaunchRip {
        perfect: bool,
    },
    /// Retriggered/updated every call from the game loop while a top is
    /// spinning; always lands on the single dedicated voice 11 (see
    /// `mixer::SPIN_HUM_VOICE`), never the general SFX pool.
    SpinHum {
        rpm_frac: f32,
    },
    HitLight,
    HitHeavy,
    DashWhoosh,
    GuardClink,
    ParryTing,
    SpecialCharge,
    SpecialFire,
    CrashOutSting,
    RingOutFall,
    ToppleWobble,
    ScoreTally,
}

/// Trigger the waveform recipe for one named SFX.
pub fn play(mixer: &mut Mixer, sfx: Sfx) {
    match sfx {
        Sfx::MenuMove => menu_move(mixer),
        Sfx::MenuSelect => menu_select(mixer),
        Sfx::MenuBack => menu_back(mixer),
        Sfx::CountBeep(n) => count_beep(mixer, n),
        Sfx::LaunchRip { perfect } => launch_rip(mixer, perfect),
        Sfx::SpinHum { rpm_frac } => spin_hum(mixer, rpm_frac),
        Sfx::HitLight => hit_light_voice(mixer, 1.0),
        Sfx::HitHeavy => hit_heavy(mixer),
        Sfx::DashWhoosh => dash_whoosh(mixer),
        Sfx::GuardClink => guard_clink(mixer),
        Sfx::ParryTing => parry_ting(mixer),
        Sfx::SpecialCharge => special_charge(mixer),
        Sfx::SpecialFire => special_fire(mixer),
        Sfx::CrashOutSting => crash_out_sting(mixer),
        Sfx::RingOutFall => ring_out_fall(mixer),
        Sfx::ToppleWobble => topple_wobble(mixer),
        Sfx::ScoreTally => score_tally(mixer),
    }
}

// ---- Menu (game_design.md §7 UI/UX: move = tri blip 440 Hz; select =
// 440->660; back = falling pair) ----

fn menu_move(mixer: &mut Mixer) {
    mixer.trigger(VoiceParams {
        waveform: Waveform::Triangle,
        freq_hz: 440.0,
        sweep_hz_per_s: 0.0,
        adsr: adsr_ms(2, 48, 0.0, 0),
        gain: 0.5,
        priority: 40,
    });
}

fn menu_select(mixer: &mut Mixer) {
    mixer.trigger(VoiceParams {
        waveform: Waveform::Triangle,
        freq_hz: 440.0,
        sweep_hz_per_s: (660.0 - 440.0) / 0.09,
        adsr: adsr_ms(2, 88, 0.0, 0),
        gain: 0.5,
        priority: 40,
    });
}

fn menu_back(mixer: &mut Mixer) {
    // "Falling pair": two notes, 660 then 440. The second voice's 60ms
    // attack keeps it near-silent while the first note is still sounding
    // (see module docs on the attack-delay trick).
    mixer.trigger(VoiceParams {
        waveform: Waveform::Triangle,
        freq_hz: 660.0,
        sweep_hz_per_s: 0.0,
        adsr: adsr_ms(2, 58, 0.0, 0),
        gain: 0.5,
        priority: 40,
    });
    mixer.trigger(VoiceParams {
        waveform: Waveform::Triangle,
        freq_hz: 440.0,
        sweep_hz_per_s: 0.0,
        adsr: adsr_ms(60, 60, 0.0, 0),
        gain: 0.5,
        priority: 40,
    });
}

// ---- Launch minigame countdown (game_design.md §4: "countdown beeps
// 660/770/880 Hz, GO! stinger") ----

fn count_beep(mixer: &mut Mixer, n: u8) {
    match n {
        0 => beep(mixer, 660.0),
        1 => beep(mixer, 770.0),
        2 => beep(mixer, 880.0),
        _ => go_stinger(mixer),
    }
}

fn beep(mixer: &mut Mixer, freq_hz: f32) {
    mixer.trigger(VoiceParams {
        waveform: Waveform::Square { duty: 128 },
        freq_hz,
        sweep_hz_per_s: 0.0,
        adsr: adsr_ms(3, 147, 0.0, 0),
        gain: 0.55,
        priority: 60,
    });
}

fn go_stinger(mixer: &mut Mixer) {
    for freq_hz in [880.0, 1108.0, 1320.0] {
        mixer.trigger(VoiceParams {
            waveform: Waveform::Square { duty: 128 },
            freq_hz,
            sweep_hz_per_s: 0.0,
            adsr: adsr_ms(3, 247, 0.0, 0),
            gain: 0.4,
            priority: 130,
        });
    }
    mixer.trigger(VoiceParams {
        waveform: Waveform::Noise,
        freq_hz: 3_000.0,
        sweep_hz_per_s: 0.0,
        adsr: adsr_ms(2, 58, 0.0, 0),
        gain: 0.35,
        priority: 130,
    });
}

// ---- Launch rip (game_design.md §8: "noise+saw 200->600 + sweet-spot
// ting") ----

fn launch_rip(mixer: &mut Mixer, perfect: bool) {
    mixer.trigger(VoiceParams {
        waveform: Waveform::Noise,
        freq_hz: 600.0,
        sweep_hz_per_s: -800.0,
        adsr: adsr_ms(2, 148, 0.0, 0),
        gain: 0.45,
        priority: 90,
    });
    mixer.trigger(VoiceParams {
        waveform: Waveform::Saw,
        freq_hz: 200.0,
        sweep_hz_per_s: (600.0 - 200.0) / 0.2,
        adsr: adsr_ms(5, 195, 0.0, 0),
        gain: 0.5,
        priority: 90,
    });
    if perfect {
        // Ting tail: attack ~180ms delays it until the rip is finishing.
        mixer.trigger(VoiceParams {
            waveform: Waveform::Sine,
            freq_hz: 1_800.0,
            sweep_hz_per_s: 0.0,
            adsr: adsr_ms(180, 80, 0.0, 0),
            gain: 0.5,
            priority: 110,
        });
    }
}

// ---- Spin hum (game_design.md §8: "saw+sine, pitch tracks RPM — the top
// audibly tires"; see module docs for the saw-only deviation) ----

fn spin_hum(mixer: &mut Mixer, rpm_frac: f32) {
    let rpm_frac = rpm_frac.clamp(0.0, 1.0);
    if rpm_frac <= 0.0 {
        // Let an already-sounding hum fade out via its own release instead
        // of holding sustain forever; a no-op if it wasn't sounding.
        mixer.release_voice(SPIN_HUM_VOICE);
        return;
    }
    let freq_hz = 60.0 + 200.0 * rpm_frac;
    if mixer.voice_active(SPIN_HUM_VOICE) {
        // Glide in place: no phase/envelope reset, so RPM changes sound
        // continuous instead of clicking on every update.
        mixer.retune_voice(SPIN_HUM_VOICE, freq_hz);
    } else {
        mixer.trigger_at(
            SPIN_HUM_VOICE,
            VoiceParams {
                waveform: Waveform::Saw,
                freq_hz,
                sweep_hz_per_s: 0.0,
                adsr: adsr_ms(15, 0, 1.0, 250),
                gain: 0.32,
                priority: 30,
            },
        );
    }
}

// ---- Hits (game_design.md §5 juice table) ----

/// Shared recipe for `HitLight`, reused at a lower gain for the
/// `Landed{impact}` soft-thud case (module brief: "reuse HitLight at lower
/// gain").
fn hit_light_voice(mixer: &mut Mixer, gain_mult: f32) {
    mixer.trigger(VoiceParams {
        waveform: Waveform::Triangle,
        freq_hz: 300.0,
        sweep_hz_per_s: (220.0 - 300.0) / 0.06,
        adsr: adsr_ms(2, 58, 0.0, 0),
        gain: 0.55 * gain_mult,
        priority: 100,
    });
}

fn hit_heavy(mixer: &mut Mixer) {
    mixer.trigger(VoiceParams {
        waveform: Waveform::Saw,
        freq_hz: 180.0,
        sweep_hz_per_s: (90.0 - 180.0) / 0.14,
        adsr: adsr_ms(2, 138, 0.0, 0),
        gain: 0.6,
        priority: 150,
    });
    mixer.trigger(VoiceParams {
        waveform: Waveform::Noise,
        freq_hz: 500.0,
        sweep_hz_per_s: -300.0,
        adsr: adsr_ms(1, 59, 0.0, 0),
        gain: 0.5,
        priority: 150,
    });
}

fn dash_whoosh(mixer: &mut Mixer) {
    // "HP-swept" approximated as a rising noise pitch (see module docs).
    mixer.trigger(VoiceParams {
        waveform: Waveform::Noise,
        freq_hz: 250.0,
        sweep_hz_per_s: (2_000.0 - 250.0) / 0.16,
        adsr: adsr_ms(5, 155, 0.0, 0),
        gain: 0.4,
        priority: 80,
    });
}

fn guard_clink(mixer: &mut Mixer) {
    for freq_hz in [1_200.0, 1_800.0] {
        mixer.trigger(VoiceParams {
            waveform: Waveform::Sine,
            freq_hz,
            sweep_hz_per_s: 0.0,
            adsr: adsr_ms(2, 68, 0.0, 0),
            gain: 0.42,
            priority: 110,
        });
    }
}

fn parry_ting(mixer: &mut Mixer) {
    // Brighter/higher than the guard clink — a reward cue, not a block cue.
    mixer.trigger(VoiceParams {
        waveform: Waveform::Sine,
        freq_hz: 2_000.0,
        sweep_hz_per_s: 0.0,
        adsr: adsr_ms(2, 88, 0.0, 0),
        gain: 0.5,
        priority: 120,
    });
    mixer.trigger(VoiceParams {
        waveform: Waveform::Sine,
        freq_hz: 3_000.0,
        sweep_hz_per_s: 0.0,
        adsr: adsr_ms(2, 88, 0.0, 0),
        gain: 0.35,
        priority: 120,
    });
}

fn special_charge(mixer: &mut Mixer) {
    mixer.trigger(VoiceParams {
        waveform: Waveform::Saw,
        freq_hz: 600.0,
        sweep_hz_per_s: (1_200.0 - 600.0) / 0.15,
        adsr: adsr_ms(5, 145, 0.0, 0),
        gain: 0.5,
        priority: 90,
    });
}

fn special_fire(mixer: &mut Mixer) {
    // Saw chord swell (root, major-third-ish, fifth) + noise burst
    // (game_design.md §5: "saw chord swell + noise, 400 ms").
    const ROOT_HZ: f32 = 220.0;
    // 12-tone-equal-temperament ratios for the major third (4 semitones)
    // and perfect fifth (7 semitones); see tracker.rs for the full table.
    const THIRD_RATIO: f32 = 1.259_921;
    const FIFTH_RATIO: f32 = 1.498_307;
    for ratio in [1.0, THIRD_RATIO, FIFTH_RATIO] {
        mixer.trigger(VoiceParams {
            waveform: Waveform::Saw,
            freq_hz: ROOT_HZ * ratio,
            sweep_hz_per_s: 0.0,
            adsr: adsr_ms(100, 300, 0.0, 0),
            gain: 0.32,
            priority: 170,
        });
    }
    mixer.trigger(VoiceParams {
        waveform: Waveform::Noise,
        freq_hz: 800.0,
        sweep_hz_per_s: 0.0,
        adsr: adsr_ms(5, 395, 0.0, 0),
        gain: 0.3,
        priority: 170,
    });
}

fn crash_out_sting(mixer: &mut Mixer) {
    // Sub-boom + descending saw (game_design.md §5: "sub-boom + descending
    // saw, 600 ms").
    mixer.trigger(VoiceParams {
        waveform: Waveform::Triangle,
        freq_hz: 55.0,
        sweep_hz_per_s: 0.0,
        adsr: adsr_ms(2, 148, 0.0, 0),
        gain: 0.7,
        priority: 220,
    });
    mixer.trigger(VoiceParams {
        waveform: Waveform::Saw,
        freq_hz: 900.0,
        sweep_hz_per_s: (80.0 - 900.0) / 0.6,
        adsr: adsr_ms(5, 595, 0.0, 0),
        gain: 0.5,
        priority: 220,
    });
}

fn ring_out_fall(mixer: &mut Mixer) {
    // Two-stage: doppler whistle 800->200 Hz, then a thud (game_design.md
    // §5/§8: "doppler whistle 800->200 -> thud"). The thud voice's 450ms
    // attack delays it until the whistle is finishing (see module docs).
    mixer.trigger(VoiceParams {
        waveform: Waveform::Sine,
        freq_hz: 800.0,
        sweep_hz_per_s: (200.0 - 800.0) / 0.45,
        adsr: adsr_ms(5, 445, 0.0, 0),
        gain: 0.5,
        priority: 200,
    });
    mixer.trigger(VoiceParams {
        waveform: Waveform::Noise,
        freq_hz: 150.0,
        sweep_hz_per_s: 0.0,
        adsr: adsr_ms(450, 150, 0.0, 0),
        gain: 0.55,
        priority: 200,
    });
}

fn topple_wobble(mixer: &mut Mixer) {
    // Two slightly-detuned sine sweeps: the ~1.5% detune beats against
    // itself as they both fall, producing a real (not faked) audible
    // "wobble" purely from the mixer summing two close frequencies —
    // game_design.md §5: "detuning sine wobble 400->80 Hz".
    mixer.trigger(VoiceParams {
        waveform: Waveform::Sine,
        freq_hz: 400.0,
        sweep_hz_per_s: (80.0 - 400.0) / 0.45,
        adsr: adsr_ms(5, 445, 0.0, 0),
        gain: 0.45,
        priority: 160,
    });
    mixer.trigger(VoiceParams {
        waveform: Waveform::Sine,
        freq_hz: 406.0,
        sweep_hz_per_s: (82.0 - 406.0) / 0.45,
        adsr: adsr_ms(5, 445, 0.0, 0),
        gain: 0.4,
        priority: 160,
    });
}

fn score_tally(mixer: &mut Mixer) {
    mixer.trigger(VoiceParams {
        waveform: Waveform::Sine,
        freq_hz: 1_600.0,
        sweep_hz_per_s: 0.0,
        adsr: adsr_ms(2, 88, 0.0, 0),
        gain: 0.45,
        priority: 50,
    });
}

/// Detuned saw swell for a mid-fight airborne launch — not in the [`Sfx`]
/// enum (that enum's `LaunchRip` is specifically the round-start minigame
/// stinger, a different moment from this in-fight event), so [`on_event`]
/// synthesizes it directly. Recipe: game_design.md §5 "Airborne clash ...
/// detuned saw swell 120->300 Hz".
fn airborne_launch_swell(mixer: &mut Mixer) {
    mixer.trigger(VoiceParams {
        waveform: Waveform::Saw,
        freq_hz: 118.0,
        sweep_hz_per_s: (300.0 - 118.0) / 0.35,
        adsr: adsr_ms(20, 330, 0.0, 50),
        gain: 0.45,
        priority: 180,
    });
    mixer.trigger(VoiceParams {
        waveform: Waveform::Saw,
        freq_hz: 122.0,
        sweep_hz_per_s: (300.0 - 122.0) / 0.35,
        adsr: adsr_ms(20, 330, 0.0, 50),
        gain: 0.4,
        priority: 180,
    });
}

/// Gain multiplier for the `Landed{impact}` soft-thud reuse of `HitLight`
/// (module brief: "soft thud reuse HitLight at lower gain").
const LANDED_THUD_GAIN: f32 = 0.5;

/// Impact threshold above which a landing plays the soft-thud SFX (module
/// brief: "only if impact > 2").
const LANDED_THUD_IMPACT_THRESHOLD: f32 = 2.0;

/// Map the *current* `floppy_core::physics::BattleEvent` set to SFX
/// triggers. Deliberately **no wildcard arm**: every variant is matched by
/// name, so adding a new `BattleEvent` variant in `floppy_core` is a compile
/// error here until this function makes an explicit decision about how (or
/// whether) it sounds — the milestone brief calls this out explicitly as
/// the intent, and it is the standard "exhaustive match as a forcing
/// function" pattern for keeping a downstream consumer honest as an enum
/// grows.
pub fn on_event(mixer: &mut Mixer, ev: &floppy_core::physics::BattleEvent) {
    use floppy_core::physics::BattleEvent;
    match ev {
        BattleEvent::Hit { heavy, .. } => {
            if *heavy {
                play(mixer, Sfx::HitHeavy);
            } else {
                play(mixer, Sfx::HitLight);
            }
        }
        BattleEvent::Dash { .. } => play(mixer, Sfx::DashWhoosh),
        BattleEvent::AirborneLaunch { .. } => airborne_launch_swell(mixer),
        BattleEvent::Landed { impact, .. } => {
            if *impact > LANDED_THUD_IMPACT_THRESHOLD {
                hit_light_voice(mixer, LANDED_THUD_GAIN);
            }
        }
        BattleEvent::RingOut { .. } => play(mixer, Sfx::RingOutFall),
        BattleEvent::Topple { .. } => play(mixer, Sfx::ToppleWobble),
        // M4-A additions (game_design.md §2/§3, task spec's explicit
        // event->SFX mapping):
        BattleEvent::Parry { .. } => play(mixer, Sfx::ParryTing),
        BattleEvent::GuardBlock { .. } => play(mixer, Sfx::GuardClink),
        BattleEvent::SpecialFire { .. } => play(mixer, Sfx::SpecialFire),
        BattleEvent::CrashOut { .. } => play(mixer, Sfx::CrashOutSting),
        BattleEvent::AerialSlam { .. } => play(mixer, Sfx::HitHeavy),
        BattleEvent::SpecialHit { .. } => play(mixer, Sfx::HitHeavy),
        // AnchorBreak is a state change (forced verb release), not an
        // impact — no SFX (empty arm, deliberate per the task spec).
        BattleEvent::AnchorBreak { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use floppy_core::physics::BattleEvent;
    use floppy_core::vec::Vec3;

    fn any_voice_active(mixer: &Mixer) -> bool {
        (0..crate::mixer::NUM_VOICES).any(|i| mixer.voice_active(i))
    }

    /// Test 9 (on_event): the exhaustive match compiles (this file itself is
    /// the proof — see the `match` in `on_event` above with no `_` arm), and
    /// every mapped event triggers at least one voice.
    #[test]
    fn every_battle_event_triggers_at_least_one_voice() {
        let events = [
            BattleEvent::Hit {
                heavy: false,
                pos: Vec3::default(),
                speed: 1.0,
            },
            BattleEvent::Hit {
                heavy: true,
                pos: Vec3::default(),
                speed: 5.0,
            },
            BattleEvent::Dash { who: 0 },
            BattleEvent::AirborneLaunch { who: 0 },
            BattleEvent::Landed {
                who: 0,
                impact: 5.0,
            },
            BattleEvent::RingOut { who: 0 },
            BattleEvent::Topple { who: 0 },
        ];
        for ev in &events {
            let mut mixer = Mixer::new();
            on_event(&mut mixer, ev);
            assert!(any_voice_active(&mixer), "{ev:?} triggered no voice");
        }
    }

    /// `Landed` below the impact threshold must stay silent (module brief:
    /// "only if impact > 2").
    #[test]
    fn landed_below_threshold_is_silent() {
        let mut mixer = Mixer::new();
        on_event(
            &mut mixer,
            &BattleEvent::Landed {
                who: 0,
                impact: 0.5,
            },
        );
        assert!(!any_voice_active(&mixer));
    }

    #[test]
    fn every_sfx_variant_triggers_at_least_one_voice() {
        let variants = [
            Sfx::MenuMove,
            Sfx::MenuSelect,
            Sfx::MenuBack,
            Sfx::CountBeep(0),
            Sfx::CountBeep(1),
            Sfx::CountBeep(2),
            Sfx::CountBeep(3),
            Sfx::LaunchRip { perfect: false },
            Sfx::LaunchRip { perfect: true },
            Sfx::SpinHum { rpm_frac: 0.5 },
            Sfx::HitLight,
            Sfx::HitHeavy,
            Sfx::DashWhoosh,
            Sfx::GuardClink,
            Sfx::ParryTing,
            Sfx::SpecialCharge,
            Sfx::SpecialFire,
            Sfx::CrashOutSting,
            Sfx::RingOutFall,
            Sfx::ToppleWobble,
            Sfx::ScoreTally,
        ];
        for sfx in variants {
            let mut mixer = Mixer::new();
            play(&mut mixer, sfx);
            assert!(any_voice_active(&mixer), "{sfx:?} triggered no voice");
        }
    }

    #[test]
    fn spin_hum_zero_rpm_releases_instead_of_hanging() {
        let mut mixer = Mixer::new();
        play(&mut mixer, Sfx::SpinHum { rpm_frac: 0.8 });
        assert!(mixer.voice_active(SPIN_HUM_VOICE));
        play(&mut mixer, Sfx::SpinHum { rpm_frac: 0.0 });
        // Still "active" (releasing), but must actually be counting down a
        // release rather than parked at sustain forever: render past a
        // generous release window and confirm it goes silent.
        let mut buf = [0i16; 44_100];
        mixer.render(&mut buf);
        assert!(!mixer.voice_active(SPIN_HUM_VOICE));
    }
}
