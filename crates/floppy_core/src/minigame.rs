//! M4-B: launch minigame — aim → spin direction → power sweep with
//! PERFECT/good/overcharge bands (game_design.md §4). Pure, deterministic,
//! step-counted (one [`MinigameState::step`] call == one 120 Hz tick — no
//! wall-clock, no `World.step` deltas, matching the flow-level contract in
//! `crate::flow`).
//!
//! Sequence: `Aim` (heading around the lip + entry depth) → `SpinDir` (toggle
//! ±1) → `Power` (0→100→0 triangle-wave marker, locked by `dash`) → `Locked`.
//! `dash` (Space) advances Aim→SpinDir→Power→(produces [`LaunchChoice`]);
//! `special` (Shift) toggles spin direction while in `SpinDir`. All edges
//! (dash/special) are detected against the *previous* tick's input stored in
//! the state, so a held key only fires once — exactly the "edge" semantics
//! `flow.rs` uses for its own menu cursors.

use crate::input::InputState;
use crate::rng::Rng;
use std::f32::consts::TAU;

/// Heading rotation rate while `Aim` sees a nonzero `dir_x`, radians/tick.
/// A full 360-degree loop takes `TAU / HEADING_RATE` ~= 126 ticks (~1.05 s
/// @120 Hz) — brisk enough to feel responsive, slow enough to aim precisely.
const HEADING_RATE: f32 = 0.05;
/// Entry-depth adjustment rate while `Aim` sees a nonzero `dir_y`, per tick.
const DEPTH_RATE: f32 = 0.01;
/// Entry depth bounds (game_design.md §4: "0.4-1.0 of V_MAX").
const DEPTH_MIN: f32 = 0.4;
const DEPTH_MAX: f32 = 1.0;
/// Depth the minigame starts at — the midpoint of the documented range, so
/// an untouched Aim stage still produces a sane launch if the player dashes
/// through it immediately.
const DEPTH_START: f32 = 0.7;

/// Power-sweep period, in fractional 120 Hz steps (game_design.md §4:
/// "period 0.66 s Normal (0.80 Easy, 0.52 Hard/Ace)"). These are the ONLY
/// difficulty-scaled numbers here; per the design doc's own parenthetical
/// ("the AI rolls its own quality from skill params") the tier spread is
/// consumed by [`ai_roll`], not by the human player's own sweep — the human
/// minigame always runs at the `Normal` period regardless of the opponent's
/// difficulty setting (documented decision, M4-B scope).
const POWER_PERIOD_EASY: f32 = 96.0; // 0.80s * 120Hz
const POWER_PERIOD_NORMAL: f32 = 79.2; // 0.66s * 120Hz
const POWER_PERIOD_HARD_ACE: f32 = 62.4; // 0.52s * 120Hz

/// AI difficulty tier (SPEC §11 names; the full utility controller is M5
/// scope — here the tier only parameterizes [`ai_roll`]'s power-timing
/// spread and `power_period_steps` for documentation/future use).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Difficulty {
    Easy,
    Normal,
    Hard,
    Ace,
}

impl Difficulty {
    /// Power-sweep period in fractional steps for this tier (game_design.md
    /// §4). Hard and Ace share the same (fastest) period per the doc.
    pub fn power_period_steps(self) -> f32 {
        match self {
            Difficulty::Easy => POWER_PERIOD_EASY,
            Difficulty::Normal => POWER_PERIOD_NORMAL,
            Difficulty::Hard | Difficulty::Ace => POWER_PERIOD_HARD_ACE,
        }
    }
}

/// Sweet-spot / band constants (game_design.md §4, verbatim). Public so the
/// HUD can draw the band markers on the power bar without duplicating the
/// numbers (drawing in render, numbers in core — SPEC §7).
pub const PERFECT_CENTER: f32 = 86.0;
pub const PERFECT_HALF_WIDTH: f32 = 3.0; // "width 6%" -> +-3
pub const GOOD_LO: f32 = 72.0;
pub const GOOD_HI: f32 = 94.0;
pub const OVERCHARGE_THRESHOLD: f32 = 94.0;

const QUALITY_PERFECT: f32 = 1.20;
const QUALITY_GOOD: f32 = 1.08;
const QUALITY_BASE: f32 = 1.00;
const QUALITY_OVERCHARGE: f32 = 1.12;
const PERFECT_BONUS_METER: f32 = 10.0;
const OVERCHARGE_START_TILT: f32 = 0.06;

/// Classify a locked power value into `(quality, bonus_meter, start_tilt)`
/// (game_design.md §4). `descending` is whether the lock happened on the
/// falling half of the 0->100->0 sweep — Overcharge only exists on that pass.
/// Order matters: PERFECT (narrowest, highest-priority band) is checked
/// first, then Overcharge, then the wider Good band, else base quality.
fn classify_power(power_pct: f32, descending: bool) -> (f32, f32, f32) {
    if (power_pct - PERFECT_CENTER).abs() <= PERFECT_HALF_WIDTH {
        (QUALITY_PERFECT, PERFECT_BONUS_METER, 0.0)
    } else if descending && power_pct > OVERCHARGE_THRESHOLD {
        (QUALITY_OVERCHARGE, 0.0, OVERCHARGE_START_TILT)
    } else if (GOOD_LO..=GOOD_HI).contains(&power_pct) {
        (QUALITY_GOOD, 0.0, 0.0)
    } else {
        (QUALITY_BASE, 0.0, 0.0)
    }
}

/// Triangle wave `0 -> 100 -> 0` over `period` ticks, given a phase already
/// wrapped into `[0, period)`. No `%`/`fract`/`rem_euclid` (all banned
/// outside `fixmath.rs` by `tests/no_libm.rs` as platform-dependent-libm
/// risks) — callers wrap `phase` themselves with the same
/// increment-then-branch-subtract idiom `physics::step_spin_and_precession`
/// uses for `spin_angle`.
fn triangle_value(phase: f32, period: f32) -> f32 {
    if period <= 0.0 {
        return 0.0;
    }
    let half = period * 0.5;
    if phase < half {
        (phase / half) * 100.0
    } else {
        (2.0 - phase / half) * 100.0
    }
}

/// The three stages plus the terminal "already resolved" state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Aim,
    SpinDir,
    Power,
    Locked,
}

/// Everything the Launch phase needs to hand to `physics::spawn_launch_top`
/// (via `physics::LaunchParams`) plus the two extra starting conditions
/// (`bonus_meter`, `start_tilt`) that aren't part of `LaunchParams` — the
/// caller (`flow.rs`) applies those directly to the spawned `Top`'s public
/// `meter`/`tilt` fields after `World::launch`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LaunchChoice {
    pub heading: f32,
    pub depth: f32,
    pub spin_dir: i8,
    pub power_frac: f32,
    pub quality: f32,
    pub bonus_meter: f32,
    pub start_tilt: f32,
}

/// One player's live launch-minigame state. `Aim`/`SpinDir` fields keep
/// updating during their own stage then freeze; `power_phase` only moves
/// during `Stage::Power`. Deterministic and headlessly steppable: construct
/// with [`MinigameState::new`], call [`MinigameState::step`] once per 120 Hz
/// tick with that tick's `InputState`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MinigameState {
    pub stage: Stage,
    pub heading: f32,
    pub depth: f32,
    pub spin_dir: i8,
    pub power_phase: f32,
    pub power_period: f32,
    pub result: Option<LaunchChoice>,
    prev: InputState,
}

impl MinigameState {
    /// Fresh minigame starting at `Stage::Aim`, heading `0.0`, depth at the
    /// documented range's midpoint, spin direction defaulted to the chosen
    /// top's preset spin (game_design.md §3) — the player can still flip it
    /// in the `SpinDir` stage.
    pub fn new(initial_spin_dir: i8) -> Self {
        Self {
            stage: Stage::Aim,
            heading: 0.0,
            depth: DEPTH_START,
            spin_dir: initial_spin_dir,
            power_phase: 0.0,
            power_period: Difficulty::Normal.power_period_steps(),
            result: None,
            prev: InputState::default(),
        }
    }

    /// Live power marker (0..=100), only meaningful during `Stage::Power` —
    /// `None` otherwise so HUD code can't accidentally sample a stale value
    /// from a different stage.
    pub fn power_marker_pct(&self) -> Option<f32> {
        if self.stage == Stage::Power {
            Some(triangle_value(self.power_phase, self.power_period))
        } else {
            None
        }
    }

    /// Advance exactly one 120 Hz tick. Returns `Some(LaunchChoice)` on the
    /// single tick Power locks (the `Aim`->`SpinDir`->`Power` transitions
    /// themselves return `None`); calling `step` again after locking is a
    /// harmless no-op (`Stage::Locked` never re-fires).
    pub fn step(&mut self, input: InputState) -> Option<LaunchChoice> {
        // Terminal state: fully frozen (not even `prev` mutates), so a
        // Locked state is bit-stable no matter how much input arrives.
        if self.stage == Stage::Locked {
            return None;
        }
        let dash_edge = input.dash && !self.prev.dash;
        let special_edge = input.special && !self.prev.special;
        let mut out = None;

        match self.stage {
            Stage::Aim => {
                // Sign convention (contract with `flow`/`main.rs` key mapping,
                // see `flow::DIR_UP` docs): the Right arrow arrives as
                // `dir_x = -1` and the Up arrow as `dir_y = -1`, so heading
                // increases on Right and depth increases (deeper entry) on Up.
                if input.dir_x != 0 {
                    self.heading -= input.dir_x as f32 * HEADING_RATE;
                    if self.heading >= TAU {
                        self.heading -= TAU;
                    } else if self.heading < 0.0 {
                        self.heading += TAU;
                    }
                }
                if input.dir_y != 0 {
                    self.depth =
                        (self.depth - input.dir_y as f32 * DEPTH_RATE).clamp(DEPTH_MIN, DEPTH_MAX);
                }
                if dash_edge {
                    self.stage = Stage::SpinDir;
                }
            }
            Stage::SpinDir => {
                if special_edge {
                    self.spin_dir = -self.spin_dir;
                }
                if dash_edge {
                    self.stage = Stage::Power;
                    self.power_phase = 0.0;
                }
            }
            Stage::Power => {
                let power_pct = triangle_value(self.power_phase, self.power_period);
                let descending = self.power_phase >= self.power_period * 0.5;
                if dash_edge {
                    let (quality, bonus_meter, start_tilt) = classify_power(power_pct, descending);
                    let choice = LaunchChoice {
                        heading: self.heading,
                        depth: self.depth,
                        spin_dir: self.spin_dir,
                        power_frac: power_pct / 100.0,
                        quality,
                        bonus_meter,
                        start_tilt,
                    };
                    self.result = Some(choice);
                    self.stage = Stage::Locked;
                    out = Some(choice);
                } else {
                    self.power_phase += 1.0;
                    if self.power_phase >= self.power_period {
                        self.power_phase -= self.power_period;
                    }
                }
            }
            // Unreachable (early return above) but kept for match totality.
            Stage::Locked => {}
        }

        self.prev = input;
        out
    }
}

/// Deterministic placeholder for the AI's launch roll (M4-B scope; the full
/// utility controller is M5 — SPEC §11). Models "the AI rolls its own
/// quality from skill params" (game_design.md §4) as a single RNG-driven
/// power sample whose spread narrows with difficulty (Ace lands near-PERFECT
/// almost every time, Easy is close to a coin flip across the whole range),
/// then reuses the exact same [`classify_power`] bands a human lock would
/// hit. `heading`/`depth`/`spin_dir` are supplied by the caller (`flow.rs`
/// picks a fixed opposite-side heading and the preset's default spin) since
/// this milestone's AI has no positioning logic of its own yet.
pub fn ai_roll(
    rng: &mut Rng,
    difficulty: Difficulty,
    spin_dir: i8,
    heading: f32,
    depth: f32,
) -> LaunchChoice {
    let spread = match difficulty {
        Difficulty::Easy => 30.0,
        Difficulty::Normal => 20.0,
        Difficulty::Hard => 10.0,
        Difficulty::Ace => 4.0,
    };
    let jitter = (rng.next_f32() - 0.5) * 2.0 * spread;
    let power_pct = (PERFECT_CENTER + jitter).clamp(0.0, 100.0);
    let descending = rng.next_f32() < 0.5;
    let (quality, bonus_meter, start_tilt) = classify_power(power_pct, descending);
    LaunchChoice {
        heading,
        depth,
        spin_dir,
        power_frac: power_pct / 100.0,
        quality,
        bonus_meter,
        start_tilt,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(dir_x: i8, dir_y: i8, dash: bool, special: bool) -> InputState {
        InputState {
            dir_x,
            dir_y,
            dash,
            special,
            ..Default::default()
        }
    }

    #[test]
    fn classify_power_band_edges_match_game_design_doc() {
        // PERFECT band: |power - 86| <= 3, i.e. [83, 89].
        assert_eq!(classify_power(85.9, false).0, QUALITY_PERFECT);
        assert_eq!(classify_power(86.1, false).0, QUALITY_PERFECT);
        // Just past the PERFECT band's upper edge (89.1 > 89): falls into Good.
        assert_eq!(classify_power(89.1, false).0, QUALITY_GOOD);

        // Good band lower edge: 72.0 inclusive, 71.9 just below (-> base).
        assert_eq!(classify_power(72.0, false).0, QUALITY_GOOD);
        assert_eq!(classify_power(71.9, false).0, QUALITY_BASE);

        // Overcharge requires BOTH > 94 AND the descending pass; ascending
        // at the same value gets base quality instead.
        let (q_desc, bonus_desc, tilt_desc) = classify_power(94.1, true);
        assert_eq!(q_desc, QUALITY_OVERCHARGE);
        assert_eq!(bonus_desc, 0.0);
        assert!((tilt_desc - OVERCHARGE_START_TILT).abs() < 1e-6);

        let (q_asc, _, tilt_asc) = classify_power(94.1, false);
        assert_eq!(q_asc, QUALITY_BASE);
        assert_eq!(tilt_asc, 0.0);
    }

    #[test]
    fn perfect_band_grants_bonus_meter_and_zero_tilt() {
        let (q, bonus, tilt) = classify_power(86.0, false);
        assert_eq!(q, QUALITY_PERFECT);
        assert_eq!(bonus, PERFECT_BONUS_METER);
        assert_eq!(tilt, 0.0);
    }

    #[test]
    fn triangle_wave_rises_to_100_at_half_period_and_returns_near_0() {
        let period = 80.0;
        assert_eq!(triangle_value(0.0, period), 0.0);
        assert!((triangle_value(40.0, period) - 100.0).abs() < 1e-4);
        // Just under a full period: back down near (but not at) 0.
        assert!(triangle_value(79.0, period) < 5.0);
    }

    /// Drives a `MinigameState` through all three stages with a fixed
    /// scripted input sequence, asserting the exact same sequence of
    /// `power_marker_pct()` readings and the exact same final `LaunchChoice`
    /// come out both times — the whole point of "pure, step-counted, no
    /// wall-clock" is that this is trivially true, but it's the direct
    /// regression guard for that property.
    fn run_script(spin_dir: i8, power_lock_tick: u32) -> (Vec<Option<f32>>, LaunchChoice) {
        let mut state = MinigameState::new(spin_dir);
        let mut markers = Vec::new();

        // Aim: rotate (Right arrow = dir_x -1) a few ticks, then lock.
        for _ in 0..5 {
            state.step(input(-1, 0, false, false));
        }
        assert!(state.step(input(0, 0, true, false)).is_none());
        assert_eq!(state.stage, Stage::SpinDir);

        // SpinDir: toggle once, then lock.
        assert!(state.step(input(0, 0, false, true)).is_none());
        assert!(state.step(input(0, 0, true, false)).is_none());
        assert_eq!(state.stage, Stage::Power);

        // Power: run ticks until the scripted lock tick, recording the
        // marker read on each tick (mirrors what HUD code would sample).
        let mut choice = None;
        for i in 0..power_lock_tick {
            let lock_now = i + 1 == power_lock_tick;
            let inp = input(0, 0, lock_now, false);
            markers.push(state.power_marker_pct());
            if let Some(c) = state.step(inp) {
                choice = Some(c);
            }
        }
        (markers, choice.expect("expected a lock within the script"))
    }

    #[test]
    fn same_script_yields_identical_marker_sequence_and_choice() {
        let (markers_a, choice_a) = run_script(1, 30);
        let (markers_b, choice_b) = run_script(1, 30);
        assert_eq!(markers_a, markers_b);
        assert_eq!(choice_a, choice_b);
    }

    #[test]
    fn full_state_machine_locks_in_order_and_respects_spin_toggle() {
        let (_, choice) = run_script(1, 10);
        // Started at spin_dir=1, toggled once in SpinDir -> -1.
        assert_eq!(choice.spin_dir, -1);
        // 5 ticks of dir_x=-1 (Right) at HEADING_RATE=0.05 -> heading ~= 0.25.
        assert!((choice.heading - 0.25).abs() < 1e-4);
        assert_eq!(choice.depth, DEPTH_START);
    }

    #[test]
    fn held_dash_only_fires_one_edge() {
        let mut state = MinigameState::new(1);
        state.step(input(0, 0, true, false)); // edge -> SpinDir
        state.step(input(0, 0, true, false)); // still held: no edge
        assert_eq!(state.stage, Stage::SpinDir, "held dash must not re-fire");
    }

    #[test]
    fn locked_stage_step_is_a_harmless_no_op() {
        let mut state = MinigameState::new(1);
        state.step(input(0, 0, true, false)); // edge -> SpinDir
        state.step(input(0, 0, false, false)); // release
        state.step(input(0, 0, true, false)); // edge -> Power
        state.step(input(0, 0, false, false)); // release
        state.step(input(0, 0, true, false)); // edge: locks -> Locked
        assert_eq!(state.stage, Stage::Locked);
        let before = state;
        assert!(state.step(input(1, 1, true, true)).is_none());
        assert_eq!(state, before, "Locked stage must ignore all further input");
    }

    #[test]
    fn ai_roll_is_deterministic_given_the_same_rng_seed() {
        let mut rng_a = Rng::new(99);
        let mut rng_b = Rng::new(99);
        let a = ai_roll(&mut rng_a, Difficulty::Hard, 1, 0.0, 0.7);
        let b = ai_roll(&mut rng_b, Difficulty::Hard, 1, 0.0, 0.7);
        assert_eq!(a, b);
    }

    #[test]
    fn ai_roll_never_produces_a_negative_or_over_100_power_frac() {
        let mut rng = Rng::new(12345);
        for _ in 0..500 {
            let choice = ai_roll(&mut rng, Difficulty::Easy, 1, 0.0, 0.7);
            assert!((0.0..=1.0).contains(&choice.power_frac), "{choice:?}");
        }
    }
}
