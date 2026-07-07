//! M4-A: combat layer — verb state machines, meter economy, specials,
//! Crash-Out window, round scoring (game_design.md §1-§3, SPEC §6.3/§6.5).
//!
//! This module holds the *data* ([`CombatState`], [`SpecialId`], [`RoundEnd`])
//! and the *pure* helper functions the step machinery in `physics.rs` calls
//! into. The actual per-step mutation (verb state machines, collision-time
//! parry/guard/special resolution, meter gains) lives as `World` methods in
//! `physics.rs` alongside the M2/M3 step phases it extends — this mirrors
//! the existing `step_dash`/`step_forces`/`step_collision` structure rather
//! than introducing a second, competing owner of `World`'s step order.
//!
//! ## Why one new field on `Top` instead of a dozen
//!
//! Every verb timer + the special/meter state below is collected into a
//! SINGLE new field, `Top::combat: CombatState`, rather than being added to
//! `Top` directly one-by-one. Two workspace files this milestone must not
//! edit (`crates/floppy_render/src/battle.rs`, `crates/floppy_core/src/
//! flow.rs`) never construct `Top { .. }` or `World { .. }` literals
//! themselves (they only go through `World::launch`/`LaunchParams`), so they
//! are unaffected either way — but three files this milestone DOES touch
//! (`physics.rs`'s own tests, `tests/physics_invariants.rs`, and the two
//! root-level golden-frame fixtures `src/bin/headless.rs` / `tests/
//! goldens.rs`) construct `Top { .. }` literals directly. Collecting all new
//! state into one field means each of those call sites needs exactly one
//! new line (`combat: CombatState::default()`) instead of ~15.

use crate::physics::{Stats, Top, World, TUNE};
use crate::roster::Silhouette;
use crate::vec::Vec2;

/// Which special a top has, keyed off its roster preset (game_design.md §3
/// specials table, in roster-table order).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpecialId {
    /// Cleaver.
    GuillotineRush,
    /// Bulwark.
    AegisLock,
    /// Everspin.
    SecondWind,
    /// Keystone.
    Overclock,
    /// Riptide.
    Slipstream,
    /// Gravewell.
    Sinkhole,
    /// Mirrorfang.
    Riposte,
}

impl SpecialId {
    /// Roster-table mapping (game_design.md §3), verbatim silhouette order.
    pub fn from_silhouette(s: Silhouette) -> Self {
        match s {
            Silhouette::Cleaver => SpecialId::GuillotineRush,
            Silhouette::Bulwark => SpecialId::AegisLock,
            Silhouette::Everspin => SpecialId::SecondWind,
            Silhouette::Keystone => SpecialId::Overclock,
            Silhouette::Riptide => SpecialId::Slipstream,
            Silhouette::Gravewell => SpecialId::Sinkhole,
            Silhouette::Mirrorfang => SpecialId::Riposte,
        }
    }

    /// Duration of this special's own active effect, in sim steps
    /// (game_design.md §3; "instant" effects like Second Wind still carry a
    /// steps-long buff tail, encoded here as that tail's length).
    pub fn duration_steps(self) -> u16 {
        match self {
            SpecialId::GuillotineRush => TUNE.special_guillotine_steps,
            SpecialId::AegisLock => TUNE.special_aegis_steps,
            SpecialId::SecondWind => TUNE.special_secondwind_steps,
            SpecialId::Overclock => TUNE.special_overclock_steps,
            SpecialId::Slipstream => TUNE.special_slipstream_steps,
            SpecialId::Sinkhole => TUNE.special_sinkhole_steps,
            SpecialId::Riposte => TUNE.special_riposte_steps,
        }
    }
}

/// Per-top combat/verb state (game_design.md §2/§3, SPEC §6.1's verb timers
/// and `SpecialState`). See the module doc for why this is one field on
/// `Top` rather than many. Every timer here lives in `World::step`'s body
/// and freezes during hit-stop exactly like the rest of `Top` (SPEC "HARD
/// RULES"); nothing here is ever derived from `World.step` deltas (which
/// freeze across hit-stop skips — see `physics::World::step`'s doc comment).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CombatState {
    // ---- Dash (extends the M2 dash_cd/dash_active/airdash_used on Top) ----
    /// Steps remaining in dash startup (committed, zero effect yet). `0` =
    /// not starting up.
    pub dash_startup: u8,
    /// Steps remaining in dash recovery (other verb PRESSES are locked out;
    /// movement input still applies). `0` = not recovering.
    pub dash_recovery: u8,
    /// Camera-relative direction captured at the moment of the press,
    /// applied when startup ends (so a direction change during the 2-step
    /// startup doesn't retroactively steer an already-committed dash).
    pub dash_dir: Vec2,

    // ---- Guard (Z, held) ----
    /// Steps guard has been continuously held: `1` on the press step, `2` the
    /// step after, etc. `0` whenever guard is not held (fully inert per SPEC
    /// "HARD RULES").
    pub guard_hold: u16,
    /// Steps remaining of drop-recovery after releasing guard. `0` = none.
    pub guard_drop_recovery: u8,

    // ---- Hop (X, edge) ----
    /// Steps since the hop button was PRESSED: `1` on the press step,
    /// incrementing every step through startup and the airborne flight that
    /// follows, reset to `0` on landing (or never hopped). Folds hop's
    /// 3-step startup into the same counter the 4..=12 i-frame window and
    /// the impulse-fire moment (`== startup_steps + 1`) are measured
    /// against, rather than a separate startup field — unlike dash, nothing
    /// else in the sim needs to distinguish "hop startup" from "hop
    /// airborne" as separate external states.
    pub hop_air_steps: u16,
    /// Steps remaining of post-hop land-lag. `0` = none.
    pub hop_land_lag: u8,
    /// Set once a movement direction is held during a hop-airborne state;
    /// consumed by the landing-on-opponent collision that fires the aerial
    /// slam (or by landing on the ground without contact).
    pub hop_slam_armed: bool,
    /// Highest `pos.y` observed since the current hop-airborne state began
    /// (the slam's fall-height reference).
    pub hop_apex_y: f32,

    // ---- Carve (C, held) ----
    /// Steps carve has been continuously held. `0` when not held.
    pub carve_hold: u16,
    /// Steps remaining of the post-release tilt-bonus decay window. `0` =
    /// none (also means `carve_tilt_bonus` is already at rest).
    pub carve_release_decay: u8,
    /// Accumulated extra tilt magnitude from holding Carve (game_design.md
    /// §2: "+0.05 rad/s while held, decaying over ~30 steps after release").
    /// Persists across the hold (grows unboundedly while held — this IS the
    /// "over-carving topples you" risk) and decays linearly to exactly `0`
    /// over `carve_release_decay_steps` after release.
    pub carve_tilt_bonus: f32,

    // ---- Anchor (Ctrl, held) ----
    /// Steps anchor has been continuously held. `0` when not held.
    pub anchor_hold: u16,
    /// Steps remaining of post-release lag. `0` = none.
    pub anchor_release_lag: u8,

    // ---- Facing (Guard's frontal-hemisphere reference, game_design.md §2)
    // ----
    /// Last nonzero camera-relative movement direction (documented decision:
    /// facing = velocity direction when moving at a meaningful speed, else
    /// this fallback — see `facing_xz`).
    pub last_move_dir: Vec2,

    // ---- Meter / special (game_design.md §1/§2/§3, SPEC §6.1 SpecialState)
    // ----
    /// Which special this top has (derived once at spawn, constant
    /// thereafter — see `LaunchParams::special_id`).
    pub special_id: SpecialId,
    /// `true` once meter reaches 100 and until the special fires.
    pub special_armed: bool,
    /// Remaining steps of the special's own active effect. `0` = inactive.
    pub special_active: u16,
    /// One-shot flag whose meaning depends on `special_id` (documented at
    /// each use site in `physics.rs`): Riposte "already reflected a hit this
    /// window" (gates the fizzle refund), Guillotine "bonus already
    /// consumed", Slipstream "already passed through". Unused by the other
    /// four specials. Reset to `false` every time a NEW activation begins.
    pub special_flag: bool,
    /// Previous step's `InputState::special` value, so special-fire can use
    /// a TRUE rising edge (game_design.md §2: "Shift fires it manually —
    /// arming and firing are separate moments on purpose"). Unlike Dash/Hop
    /// (which reuse this crate's existing "pressed && available" pattern,
    /// where the long cooldown/sequence timers already prevent an
    /// unintended re-trigger), a held Shift spanning the moment meter
    /// re-arms to 100 must NOT auto-fire — the player must press again.
    pub special_was_pressed: bool,
    /// Remaining steps of the Crash-Out window opened by firing. `0` =
    /// closed.
    pub crash_window: u16,
}

impl Default for CombatState {
    /// The "never touched" state every M2/M3 fixture (which never presses a
    /// verb button) needs: every timer at `0`/`false`, `special_id`
    /// defaulted to `Overclock` (harmless placeholder — real spawns always
    /// override it via `LaunchParams::special_id`).
    fn default() -> Self {
        Self {
            dash_startup: 0,
            dash_recovery: 0,
            dash_dir: Vec2::default(),
            guard_hold: 0,
            guard_drop_recovery: 0,
            hop_air_steps: 0,
            hop_land_lag: 0,
            hop_slam_armed: false,
            hop_apex_y: 0.0,
            carve_hold: 0,
            carve_release_decay: 0,
            carve_tilt_bonus: 0.0,
            anchor_hold: 0,
            anchor_release_lag: 0,
            last_move_dir: Vec2::new(1.0, 0.0),
            special_id: SpecialId::Overclock,
            special_armed: false,
            special_active: 0,
            special_flag: false,
            special_was_pressed: false,
            crash_window: 0,
        }
    }
}

impl CombatState {
    /// Fresh combat state for a top spawning with a known special.
    pub fn new(special_id: SpecialId) -> Self {
        Self {
            special_id,
            ..Self::default()
        }
    }
}

/// Stats as modified by an active buff/debuff special: Overclock's flat
/// bonus to all six stats, Aegis Lock's DEF-100-equivalent (game_design.md
/// §3). Read wherever `physics.rs` consults `top.stats` for a COMBAT
/// calculation; NOT used for `Stats::mass` (Overclock/Aegis Lock don't claim
/// to change WGT-derived momentum exchange — a deliberate scope trim,
/// documented in the milestone report).
pub fn effective_stats(top: &Top) -> Stats {
    let mut s = top.stats;
    if top.combat.special_active > 0 {
        match top.combat.special_id {
            SpecialId::Overclock => {
                let bonus = TUNE.special_overclock_stat_bonus;
                s.atk = s.atk.saturating_add(bonus);
                s.def = s.def.saturating_add(bonus);
                s.sta = s.sta.saturating_add(bonus);
                s.wgt = s.wgt.saturating_add(bonus);
                s.spd = s.spd.saturating_add(bonus);
                s.mtr = s.mtr.saturating_add(bonus);
            }
            SpecialId::AegisLock => {
                s.def = s.def.max(TUNE.special_aegis_def_effective);
            }
            _ => {}
        }
    }
    s
}

/// Extra control-acceleration multiplier from an active special
/// (game_design.md §3: Guillotine Rush "accel ×2.2", Slipstream "accel
/// ×1.8"). Combines multiplicatively with verb-driven multipliers (Guard/
/// Anchor/hop-land-lag) at the `step_forces` call site.
pub fn special_accel_mult(top: &Top) -> f32 {
    if top.combat.special_active == 0 {
        return 1.0;
    }
    match top.combat.special_id {
        SpecialId::GuillotineRush => TUNE.special_guillotine_accel_mult,
        SpecialId::Slipstream => TUNE.special_slipstream_accel_mult,
        _ => 1.0,
    }
}

/// Whether `top` is currently rooted (zero move control) by a held verb or
/// special: Anchor (game_design.md §2's "move ×0.1" is handled as a
/// multiplier, not here — this is specifically for Aegis Lock's "rooted",
/// which the design calls out as a harder lock than Anchor's).
pub fn is_rooted_by_special(top: &Top) -> bool {
    top.combat.special_active > 0 && top.combat.special_id == SpecialId::AegisLock
}

/// Facing direction (game_design.md §2 Guard doc: "facing = velocity dir, or
/// last nonzero move dir when slow"). Documented decision: "slow" means
/// horizontal speed below `FACING_SPEED_DEADZONE` m/s.
const FACING_SPEED_DEADZONE: f32 = 0.5;

pub fn facing_xz(top: &Top) -> Vec2 {
    let horiz = Vec2::new(top.vel.x, top.vel.z);
    if horiz.length_sq() > FACING_SPEED_DEADZONE * FACING_SPEED_DEADZONE {
        horiz.normalize_or_zero()
    } else {
        top.combat.last_move_dir
    }
}

/// Whether an incoming hit from horizontal direction `attacker_dir` (unit
/// vector pointing FROM the defender TOWARD the attacker) lands in the
/// defender's frontal 180° hemisphere (game_design.md §2 Guard: "frontal
/// 180° only"). Dot product against facing, no trig needed: `>= 0` means the
/// attacker is within 90° of dead ahead on either side.
pub fn is_frontal_hit(facing: Vec2, attacker_dir: Vec2) -> bool {
    facing.dot(attacker_dir) >= 0.0
}

/// Meter-gain scaling (game_design.md §2: "all × `0.7 + 0.6*MTR/100`").
/// Thin wrapper over `Stats::meter_gain_mult` so every meter-gain call site
/// in `physics.rs` reads the same formula name the design doc uses.
pub fn scaled_meter_gain(base: f32, stats: &Stats) -> f32 {
    base * stats.meter_gain_mult()
}

/// Round-ending condition (SPEC §6.5 point table). `Draw`'s `points()` value
/// (`0`) is never actually returned by [`round_points`] below — a
/// simultaneous double-out returns `None` (matching the pre-M4 convention in
/// `flow.rs`'s local mapping: "no single winner" and "replay, zero points to
/// either side" are the same event) — but the variant and its `points()`
/// still exist so callers have a total mapping if they ever need to render
/// "this would have been a draw" explicitly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoundEnd {
    CrashOut,
    Over,
    Survivor,
    Draw,
}

impl RoundEnd {
    /// Points awarded (SPEC §6.5): Crash-Out 3 / Over 2 / Survivor 1 / Draw 0.
    pub fn points(self) -> u8 {
        match self {
            RoundEnd::CrashOut => 3,
            RoundEnd::Over => 2,
            RoundEnd::Survivor => 1,
            RoundEnd::Draw => 0,
        }
    }
}

/// Round points from the current `World` state (SPEC §6.5, extending
/// `Outcome` with the Crash-Out read): a kill (ring-out OR stamina-out,
/// "opponent out by any means") while the WINNER's own Crash-Out window is
/// still open (`crash_window > 0`) scores `CrashOut` (3); otherwise ring-out
/// scores `Over` (2) and stamina-out scores `Survivor` (1). A simultaneous
/// double-out, or no decided outcome yet, returns `None` (see the `RoundEnd`
/// doc comment above for why `Draw` is never actually constructed here).
///
/// This is the API `flow.rs`'s Fight->Decided transition consumes directly
/// (M4 integration).
pub fn round_points(world: &World) -> Option<(u8, RoundEnd)> {
    use crate::physics::Outcome;
    match world.outcome {
        None | Some(Outcome::Simultaneous) => None,
        Some(Outcome::RingOut { loser }) => {
            let winner = 1 - loser;
            let end = if world.tops[winner as usize].combat.crash_window > 0 {
                RoundEnd::CrashOut
            } else {
                RoundEnd::Over
            };
            Some((winner, end))
        }
        Some(Outcome::StaminaOut { loser }) => {
            let winner = 1 - loser;
            let end = if world.tops[winner as usize].combat.crash_window > 0 {
                RoundEnd::CrashOut
            } else {
                RoundEnd::Survivor
            };
            Some((winner, end))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn special_id_from_silhouette_matches_roster_table_order() {
        assert_eq!(
            SpecialId::from_silhouette(Silhouette::Cleaver),
            SpecialId::GuillotineRush
        );
        assert_eq!(
            SpecialId::from_silhouette(Silhouette::Bulwark),
            SpecialId::AegisLock
        );
        assert_eq!(
            SpecialId::from_silhouette(Silhouette::Everspin),
            SpecialId::SecondWind
        );
        assert_eq!(
            SpecialId::from_silhouette(Silhouette::Keystone),
            SpecialId::Overclock
        );
        assert_eq!(
            SpecialId::from_silhouette(Silhouette::Riptide),
            SpecialId::Slipstream
        );
        assert_eq!(
            SpecialId::from_silhouette(Silhouette::Gravewell),
            SpecialId::Sinkhole
        );
        assert_eq!(
            SpecialId::from_silhouette(Silhouette::Mirrorfang),
            SpecialId::Riposte
        );
    }

    #[test]
    fn round_end_points_match_spec_6_5() {
        assert_eq!(RoundEnd::CrashOut.points(), 3);
        assert_eq!(RoundEnd::Over.points(), 2);
        assert_eq!(RoundEnd::Survivor.points(), 1);
        assert_eq!(RoundEnd::Draw.points(), 0);
    }

    #[test]
    fn is_frontal_hit_covers_the_180_degree_hemisphere() {
        let facing = Vec2::new(1.0, 0.0);
        assert!(is_frontal_hit(facing, Vec2::new(1.0, 0.0)));
        assert!(is_frontal_hit(facing, Vec2::new(0.5, 0.9)));
        assert!(is_frontal_hit(facing, Vec2::new(0.0, 1.0)));
        assert!(!is_frontal_hit(facing, Vec2::new(-1.0, 0.0)));
        assert!(!is_frontal_hit(facing, Vec2::new(-0.5, 0.9)));
    }
}
