//! Deterministic 3D top sim (SPEC §6.3): the fixed-order `World::step` and
//! everything it touches. Combat verbs (guard/hop/carve/anchor/dash/special)
//! and the meter/Crash-Out system are implemented here (M4-A); the verb
//! state machines and per-top special/meter state live in
//! [`crate::combat`]'s [`CombatState`] (one field, `Top::combat`).
//!
//! Every quantity here is plain `f32`/integer state, stepped in a fixed
//! order with tops always visited index 0 then 1 (SPEC §5). All
//! trig/sqrt goes through [`crate::fixmath`].

use crate::arena;
use crate::combat::{self, CombatState, SpecialId};
use crate::fixmath;
use crate::input::InputState;
use crate::rng::Rng;
use crate::vec::{Vec2, Vec3};

/// Fixed simulation timestep (SPEC §5: 120 Hz).
pub const SIM_DT: f32 = crate::clock::SIM_DT;

/// Vertical offset from the tracked "tip/contact" point ([`Top::pos`]) up to
/// the sphere used for top-vs-top collision. Not exposed as a `TuneParams`
/// field because it's a body-shape constant (SPEC §6.1: `radius`/`height`),
/// not a tunable feel knob. M3-B (game_design.md §3/§6): scaled from 0.35 to
/// 0.7 alongside the `radius`/`height` bump below so silhouettes read at
/// 60-100 px under the fixed whole-arena camera.
const BODY_CENTER_OFFSET: f32 = 0.7;

/// Full turn, radians (M3-B `spin_angle` wrap).
const TAU: f32 = std::f32::consts::TAU;

/// Magnitudes below this snap to exactly `0.0` every step (SPEC "HARD
/// RULES": denormal guard).
const DENORMAL_EPS: f32 = 1e-6;

/// Position clamp (SPEC "HARD RULES"): `|x|, |z| <= 12, y <= 12`.
const POS_XZ_LIMIT: f32 = 12.0;
const POS_Y_LIMIT: f32 = 12.0;

/// Safety ceiling on tilt magnitude (SPEC "HARD RULES": clamp tilt magnitude
/// every step). The precession model's rate-limited approach toward a
/// bounded target (see [`TuneParams`] tilt fields) never gets close to this
/// in practice; it exists purely as a belt-and-suspenders guard against
/// runaway state.
const TILT_MAGNITUDE_LIMIT: f32 = 3.0;

/// Snap tiny magnitudes to exactly zero (denormal guard, SPEC "HARD RULES").
fn snap_denormal(v: f32) -> f32 {
    if v.abs() < DENORMAL_EPS {
        0.0
    } else {
        v
    }
}

fn snap_denormal_v3(v: Vec3) -> Vec3 {
    Vec3::new(snap_denormal(v.x), snap_denormal(v.y), snap_denormal(v.z))
}

fn snap_denormal_v2(v: Vec2) -> Vec2 {
    Vec2::new(snap_denormal(v.x), snap_denormal(v.y))
}

/// Camera-relative digital direction -> horizontal unit vector. `dir_x` maps
/// to `+x`, `dir_y` maps to `-z` (SPEC §6.4 / task spec step 1). Returns the
/// zero vector for neutral input (never normalizes zero).
fn input_dir_xz(dir_x: i8, dir_y: i8) -> Vec2 {
    Vec2::new(dir_x as f32, -(dir_y as f32)).normalize_or_zero()
}

/// Carve's ramp-in fraction (game_design.md §2: "ramp-in 20 steps to full
/// effect"), `0` the instant it's pressed rising smoothly to `1` after
/// `carve_ramp_steps`. Shared by `step_forces` (slope-climb) and
/// `step_collision` (contact knockback); the max speed clamp lives inline in
/// `step_integrate_and_terrain`/`finalize_clamps` since it's a clamp bound,
/// not an accelerating force.
fn carve_ramp_frac(top: &Top) -> f32 {
    if TUNE.carve_ramp_steps == 0 {
        return 1.0;
    }
    (top.combat.carve_hold as f32 / TUNE.carve_ramp_steps as f32).min(1.0)
}

/// All tuning constants in one place (SPEC §E / task spec): nothing outside
/// this block should hardcode a gameplay-feel number.
#[derive(Clone, Copy, Debug)]
pub struct TuneParams {
    pub gravity: f32,
    pub spin_max: f32,

    pub decay_base: f32,
    pub decay_sta_bonus: f32,

    pub ground_friction: f32,
    pub slope_accel: f32,
    pub ctrl_accel_base: f32,
    pub ctrl_accel_spd: f32,
    pub max_ground_speed: f32,
    pub max_air_speed: f32,
    pub max_fall_speed: f32,
    pub air_ctrl_factor: f32,

    pub restitution: f32,
    pub knock_scale: f32,
    pub drain_scale: f32,
    pub drain_min: f32,
    pub heavy_hit_speed: f32,
    pub airborne_pop: f32,
    pub grind_drain_mul: f32,
    pub clash_knock_mul: f32,

    pub dash_impulse: f32,
    pub dash_speed_clamp: f32,
    pub dash_active_steps: u32,
    pub dash_cooldown_base: f32,
    pub dash_cooldown_spd: f32,
    pub dash_knock_mul: f32,
    pub dash_drain_mul: f32,

    pub wobble_freq_base: f32,
    pub wobble_freq_growth: f32,
    pub tilt_base: f32,
    pub tilt_growth: f32,
    pub tilt_stability_mul: f32,
    pub tilt_rate: f32,
    pub topple_tilt: f32,
    pub topple_spin: f32,

    pub hitstop_light: u32,
    pub hitstop_heavy: u32,
    /// Drain cutoff (task spec / game_design.md §5: "~350") separating a
    /// light hit's hit-stop from a heavy hit's: a collision counts as
    /// "heavy" for hit-stop/event purposes if EITHER the pre-existing
    /// speed-based rule (`v_rel > heavy_hit_speed`) OR the actual drain dealt
    /// exceeds this. Combined with OR (not replacing the speed rule) so a
    /// slow-but-high-ATK grind hit that drains hard still reads as heavy,
    /// while every pre-M4 speed-based invariant test keeps passing
    /// unchanged.
    pub heavy_hit_drain_threshold: f32,
    pub hitstop_airborne_clash: u32,
    pub hitstop_wall_bounce: u32,
    pub hitstop_guard: u32,
    pub hitstop_special_fire: u32,
    pub hitstop_crash_out: u32,
    pub hitstop_ring_out: u32,
    pub hitstop_topple: u32,

    // ---- Dash extension (game_design.md §2: startup/recovery/shove-armor)
    // ----
    pub dash_startup_steps: u32,
    pub dash_recovery_steps: u32,
    pub dash_shove_armor_threshold: f32,

    // ---- Guard (Z, held; game_design.md §2) ----
    pub guard_startup_steps: u32,
    pub guard_drop_recovery_steps: u32,
    pub guard_parry_window_steps: u32,
    pub guard_knock_mult: f32,
    pub guard_drain_mult: f32,
    pub guard_parry_knock_mult: f32,
    pub guard_parry_drain_mult: f32,
    pub guard_parry_attacker_tilt: f32,
    pub guard_spin_cost_per_s: f32,
    pub guard_move_mult: f32,
    pub guard_slope_extra_mult: f32,
    /// `tan(6 degrees)`, precomputed offline (not a runtime libm call — a
    /// plain literal) so slope-angle comparisons never need `atan`.
    pub guard_slope_tan_threshold: f32,

    // ---- Hop (X, edge; game_design.md §2) ----
    pub hop_startup_steps: u32,
    pub hop_impulse: f32,
    pub hop_spin_cost: f32,
    pub hop_iframe_start: u32,
    pub hop_iframe_end: u32,
    pub hop_land_lag_steps: u32,
    pub hop_land_lag_ctrl_mult: f32,
    pub hop_slam_drain_base: f32,
    pub hop_slam_drain_per_m: f32,
    pub hop_slam_drain_cap: f32,
    /// Knockback scaling for the aerial slam: `1.0 + (drain/cap) *
    /// this_bonus` (game_design.md §2 says knockback is "likewise scaled"
    /// without exact numbers — documented interpretation, see the milestone
    /// report).
    pub hop_slam_knock_bonus_at_cap: f32,

    // ---- Carve (C, held; game_design.md §2) ----
    pub carve_ramp_steps: u32,
    pub carve_top_speed_mult: f32,
    pub carve_slope_climb_mult: f32,
    pub carve_knock_mult: f32,
    pub carve_tilt_rate_per_s: f32,
    pub carve_release_decay_steps: u32,

    // ---- Anchor (Ctrl, held; game_design.md §2) ----
    pub anchor_startup_steps: u32,
    pub anchor_release_steps: u32,
    pub anchor_knock_mult: f32,
    pub anchor_slide_mult: f32,
    pub anchor_tilt_recovery_per_s: f32,
    pub anchor_spin_regen_per_s: f32,
    pub anchor_move_mult: f32,
    /// `tan(12 degrees)`, precomputed offline (see `guard_slope_tan_threshold`).
    pub anchor_break_slope_tan_threshold: f32,

    // ---- Meter economy (game_design.md §1/§2) ----
    pub meter_gain_scale_base: f32,
    pub meter_gain_scale_mtr: f32,
    pub meter_drain_dealt_mult: f32,
    pub meter_drain_taken_mult: f32,
    pub meter_gain_parry: f32,
    pub meter_gain_iframe_dodge: f32,
    pub meter_gain_aerial_slam: f32,
    pub meter_gain_dash_hit: f32,
    pub meter_gain_passive_per_s: f32,
    pub meter_armed_threshold: f32,
    pub crash_window_steps: u16,

    // ---- Specials (game_design.md §3, durations @120 Hz) ----
    pub special_guillotine_steps: u16,
    pub special_guillotine_accel_mult: f32,
    pub special_guillotine_homing: f32,
    pub special_guillotine_knock_mult: f32,
    pub special_guillotine_bonus_impulse: f32,

    pub special_aegis_steps: u16,
    pub special_aegis_knock_mult: f32,
    pub special_aegis_reflect_frac: f32,
    pub special_aegis_def_effective: u8,

    pub special_secondwind_steps: u16,
    pub special_secondwind_spin_bonus_frac: f32,
    pub special_secondwind_decay_mult: f32,
    pub special_secondwind_tilt_recovery_mult: f32,

    pub special_overclock_steps: u16,
    pub special_overclock_stat_bonus: u8,

    pub special_slipstream_steps: u16,
    pub special_slipstream_accel_mult: f32,
    pub special_slipstream_exit_knock_mult: f32,
    pub special_slipstream_backstab_bonus: f32,

    pub special_sinkhole_steps: u16,
    pub special_sinkhole_pull_accel: f32,
    pub special_sinkhole_radius: f32,

    pub special_riposte_steps: u16,
    pub special_riposte_knock_mult: f32,
    pub special_riposte_drain_transfer: f32,
    pub special_riposte_fizzle_refund: f32,

    pub launch_radius: f32,
    pub launch_drop_height: f32,
    pub launch_speed_base: f32,
    pub launch_speed_depth: f32,
    pub launch_spin_base: f32,
    pub launch_spin_power: f32,

    /// Visual-only accumulated spin angle rate, radians per spin-unit-second
    /// (M3-B / game_design.md §6): `spin_angle += spin * spin_angle_rate *
    /// spin_dir * SIM_DT` each step. Not a physics quantity — nothing reads
    /// it back into forces/collisions — purely the render's rotation state
    /// (see [`pose_lerp`]). Tuned so the apparent (aliased at 60 fps) motion
    /// reads as authentic spinning-top shimmer rather than a strobing blur.
    pub spin_angle_rate: f32,
}

/// Starting tuning values (task spec table). See the final report for any
/// values adjusted after invariant-test tuning, with rationale.
pub const TUNE: TuneParams = TuneParams {
    gravity: 22.0,
    spin_max: 10_000.0,

    decay_base: 34.0,
    decay_sta_bonus: 20.0,

    ground_friction: 0.25,
    slope_accel: 14.0,
    ctrl_accel_base: 6.0,
    ctrl_accel_spd: 0.10,
    max_ground_speed: 9.0,
    max_air_speed: 14.0,
    max_fall_speed: 18.0,
    air_ctrl_factor: 0.2,

    restitution: 0.55,
    knock_scale: 0.9,
    drain_scale: 260.0,
    drain_min: 40.0,
    heavy_hit_speed: 4.5,
    airborne_pop: 3.2,
    grind_drain_mul: 1.25,
    clash_knock_mul: 1.15,

    dash_impulse: 6.0,
    dash_speed_clamp: 11.0,
    dash_active_steps: 12,
    dash_cooldown_base: 72.0,
    dash_cooldown_spd: 0.4,
    dash_knock_mul: 1.4,
    dash_drain_mul: 1.25,

    wobble_freq_base: 2.0,
    wobble_freq_growth: 6.0,
    tilt_base: 0.04,
    tilt_growth: 0.9,
    tilt_stability_mul: 0.7,
    tilt_rate: 0.8,
    topple_tilt: 1.0,
    topple_spin: 2_500.0,

    hitstop_light: 1,
    hitstop_heavy: 4,
    heavy_hit_drain_threshold: 350.0,
    hitstop_airborne_clash: 6,
    hitstop_wall_bounce: 2,
    hitstop_guard: 2,
    hitstop_special_fire: 3,
    hitstop_crash_out: 10,
    hitstop_ring_out: 5,
    hitstop_topple: 4,

    dash_startup_steps: 2,
    dash_recovery_steps: 8,
    dash_shove_armor_threshold: 2.0,

    guard_startup_steps: 4,
    guard_drop_recovery_steps: 6,
    guard_parry_window_steps: 8,
    guard_knock_mult: 0.25,
    guard_drain_mult: 0.35,
    guard_parry_knock_mult: 0.0,
    guard_parry_drain_mult: 0.1,
    guard_parry_attacker_tilt: 0.12,
    guard_spin_cost_per_s: 90.0,
    guard_move_mult: 0.4,
    guard_slope_extra_mult: 0.6,
    guard_slope_tan_threshold: 0.105_104_24, // tan(6 deg)

    hop_startup_steps: 3,
    hop_impulse: 4.5,
    hop_spin_cost: 120.0,
    hop_iframe_start: 4,
    hop_iframe_end: 12,
    hop_land_lag_steps: 10,
    hop_land_lag_ctrl_mult: 0.3,
    hop_slam_drain_base: 250.0,
    hop_slam_drain_per_m: 8.0,
    hop_slam_drain_cap: 900.0,
    hop_slam_knock_bonus_at_cap: 1.5,

    carve_ramp_steps: 20,
    carve_top_speed_mult: 1.5,
    carve_slope_climb_mult: 1.8,
    carve_knock_mult: 1.35,
    carve_tilt_rate_per_s: 0.05,
    carve_release_decay_steps: 30,

    anchor_startup_steps: 6,
    anchor_release_steps: 8,
    anchor_knock_mult: 0.2,
    anchor_slide_mult: 0.1,
    anchor_tilt_recovery_per_s: 0.08,
    anchor_spin_regen_per_s: 150.0,
    anchor_move_mult: 0.1,
    anchor_break_slope_tan_threshold: 0.212_556_56, // tan(12 deg)

    meter_gain_scale_base: 0.7,
    meter_gain_scale_mtr: 0.6,
    meter_drain_dealt_mult: 0.01,
    meter_drain_taken_mult: 0.0067,
    meter_gain_parry: 12.0,
    meter_gain_iframe_dodge: 8.0,
    meter_gain_aerial_slam: 15.0,
    meter_gain_dash_hit: 5.0,
    meter_gain_passive_per_s: 1.5,
    meter_armed_threshold: 100.0,
    crash_window_steps: 144,

    special_guillotine_steps: 48,
    special_guillotine_accel_mult: 2.2,
    special_guillotine_homing: 0.3,
    special_guillotine_knock_mult: 1.8,
    special_guillotine_bonus_impulse: 22.0,

    special_aegis_steps: 150,
    special_aegis_knock_mult: 0.35,
    special_aegis_reflect_frac: 0.5,
    special_aegis_def_effective: 100,

    special_secondwind_steps: 240,
    special_secondwind_spin_bonus_frac: 0.18,
    special_secondwind_decay_mult: 0.05,
    special_secondwind_tilt_recovery_mult: 1.5,

    special_overclock_steps: 120,
    special_overclock_stat_bonus: 12,

    special_slipstream_steps: 60,
    special_slipstream_accel_mult: 1.8,
    special_slipstream_exit_knock_mult: 1.6,
    special_slipstream_backstab_bonus: 1.3,

    special_sinkhole_steps: 180,
    special_sinkhole_pull_accel: 3.5,
    special_sinkhole_radius: 2.4,

    special_riposte_steps: 90,
    special_riposte_knock_mult: 1.4,
    special_riposte_drain_transfer: 0.6,
    special_riposte_fizzle_refund: 30.0,

    launch_radius: 6.5,
    launch_drop_height: 1.2,
    launch_speed_base: 3.0,
    launch_speed_depth: 5.0,
    launch_spin_base: 7_000.0,
    launch_spin_power: 2_000.0,

    spin_angle_rate: 0.045,
};

/// Six 0..=100 stats (game_design.md §3). Methods return the derived sim
/// quantities the design doc maps them to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Stats {
    pub atk: u8,
    pub def: u8,
    pub sta: u8,
    pub wgt: u8,
    pub spd: u8,
    pub mtr: u8,
}

impl Stats {
    /// `mass = 20 + 0.6*WGT`.
    pub fn mass(&self) -> f32 {
        20.0 + 0.6 * self.wgt as f32
    }

    /// Knockback dealt multiplier `0.5 + ATK/100`.
    pub fn knock_mult(&self) -> f32 {
        0.5 + self.atk as f32 / 100.0
    }

    /// Drain taken multiplier `1 - 0.6*DEF/100`.
    pub fn drain_taken_mult(&self) -> f32 {
        1.0 - 0.6 * self.def as f32 / 100.0
    }

    /// Knockback taken multiplier `1 - 0.4*DEF/100`.
    pub fn knock_taken_mult(&self) -> f32 {
        1.0 - 0.4 * self.def as f32 / 100.0
    }

    /// Passive spin decay per second: `decay_base - decay_sta_bonus*STA/100`.
    pub fn decay_per_s(&self) -> f32 {
        TUNE.decay_base - TUNE.decay_sta_bonus * self.sta as f32 / 100.0
    }

    /// Control acceleration `ctrl_accel_base + ctrl_accel_spd*SPD` m/s².
    pub fn ctrl_accel(&self) -> f32 {
        TUNE.ctrl_accel_base + TUNE.ctrl_accel_spd * self.spd as f32
    }

    /// Dash cooldown in whole sim steps: `dash_cooldown_base -
    /// dash_cooldown_spd*SPD`, floored at 1 step so a dash can never be
    /// immediately re-available in the same step it was thrown.
    pub fn dash_cooldown_steps(&self) -> u16 {
        let steps = TUNE.dash_cooldown_base - TUNE.dash_cooldown_spd * self.spd as f32;
        steps.max(1.0) as u16
    }

    /// Meter gain multiplier `0.7 + 0.6*MTR/100` (unused until M4; defined
    /// now so the stat mapping table has one authoritative home).
    pub fn meter_gain_mult(&self) -> f32 {
        0.7 + 0.6 * self.mtr as f32 / 100.0
    }
}

/// One top's full sim state (SPEC §6.1, trimmed to M2 scope — no
/// `TopKind`/verb-timer/`SpecialState` fields yet).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Top {
    /// The tip/contact point of the top — NOT the sphere center used for
    /// collision. `pos.y` is the height of the point that touches the arena
    /// surface; the collision sphere's center is `pos + (0,
    /// BODY_CENTER_OFFSET, 0)`. Documented here since the task spec calls
    /// this choice out explicitly.
    pub pos: Vec3,
    pub vel: Vec3,
    pub spin: f32,
    pub spin_dir: i8,
    pub tilt: Vec2,
    pub tilt_phase: f32,
    /// Real accumulated visual rotation angle around the spin axis, radians,
    /// wrapped into `[0, 2*pi)` (M3-B / game_design.md §6). Sim-frozen during
    /// hitstop like everything else in the step body; render-only, never fed
    /// back into physics.
    pub spin_angle: f32,
    pub radius: f32,
    pub height: f32,
    pub stats: Stats,
    pub grounded: bool,
    pub dash_cd: u16,
    pub dash_active: u16,
    pub meter: f32,
    pub airdash_used: bool,
    /// Verb-machine + special/Crash-Out state (M4-A, `combat::CombatState`).
    /// One field rather than many — see `combat.rs`'s module doc for why.
    pub combat: CombatState,
}

impl Top {
    fn spin_frac(&self) -> f32 {
        (self.spin / TUNE.spin_max).clamp(0.0, 1.0)
    }
}

/// Render-ready interpolated snapshot of one top (SPEC §5: rendering blends
/// the two most recent 120 Hz sim states by `alpha`). `radius`/`height` are
/// body-shape constants, not "previous vs current" state, so they're just
/// carried from `curr`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TopPose {
    pub pos: Vec3,
    pub tilt: Vec2,
    pub spin_angle: f32,
    pub radius: f32,
    pub height: f32,
}

/// Interpolate `prev` -> `curr` by `alpha` (clamped to `[0, 1]`) for one top.
/// `pos`/`tilt` lerp linearly. `spin_angle` lerps along the SHORTEST arc
/// around the `[0, 2*pi)` wrap: naively lerping the raw wrapped values would
/// animate a near-`0`/near-`2*pi` pair as one all-the-way-around revolution
/// backwards instead of the one small step it actually is.
pub fn pose_lerp(prev: &Top, curr: &Top, alpha: f32) -> TopPose {
    let alpha = alpha.clamp(0.0, 1.0);

    let mut diff = curr.spin_angle - prev.spin_angle;
    if diff > TAU * 0.5 {
        diff -= TAU;
    } else if diff < -TAU * 0.5 {
        diff += TAU;
    }
    let mut spin_angle = prev.spin_angle + diff * alpha;
    if spin_angle >= TAU {
        spin_angle -= TAU;
    } else if spin_angle < 0.0 {
        spin_angle += TAU;
    }

    TopPose {
        pos: prev.pos.lerp(curr.pos, alpha),
        tilt: prev.tilt.lerp(curr.tilt, alpha),
        spin_angle,
        radius: curr.radius,
        height: curr.height,
    }
}

/// Discrete, headlessly-consumable gameplay moments emitted by [`World::step`]
/// (SPEC §6.3 step 6/7, render/audio hook points per game_design.md §5).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BattleEvent {
    Hit {
        heavy: bool,
        pos: Vec3,
        speed: f32,
    },
    Dash {
        who: u8,
    },
    AirborneLaunch {
        who: u8,
    },
    Landed {
        who: u8,
        impact: f32,
    },
    RingOut {
        who: u8,
    },
    Topple {
        who: u8,
    },
    /// `who` fired their special (game_design.md §2/§3).
    SpecialFire {
        who: u8,
    },
    /// `who` landed a special-driven bonus hit (Guillotine's next-contact
    /// bonus, Aegis Lock's reflect, Slipstream's exit hit, Riposte's
    /// reversal — see the relevant `step_collision` branch).
    SpecialHit {
        who: u8,
    },
    /// `winner` scored a Crash-Out finish (SPEC §6.5).
    CrashOut {
        winner: u8,
    },
    /// `who` parried (game_design.md §2 Guard).
    Parry {
        who: u8,
    },
    /// `who` blocked a hit with (non-parry) Guard.
    GuardBlock {
        who: u8,
    },
    /// `who` landed an aerial slam (game_design.md §2 Hop).
    AerialSlam {
        who: u8,
    },
    /// `who`'s Anchor was forcibly released by a steep slope.
    AnchorBreak {
        who: u8,
    },
}

/// Round-ending condition (SPEC §6.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    RingOut { loser: u8 },
    StaminaOut { loser: u8 },
    Simultaneous,
}

/// Parameters for one top's Launch-phase entry (game_design.md §4).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LaunchParams {
    /// Heading around the bowl lip, radians.
    pub heading: f32,
    /// Entry depth fraction, `0.4..=1.0` of max approach speed.
    pub depth: f32,
    /// Power-sweep fraction, `0..=1`.
    pub power: f32,
    /// Timing-quality multiplier: `1.0` / `1.08` / `1.12` / `1.2`.
    pub quality: f32,
    pub spin_dir: i8,
    pub stats: Stats,
    /// Explicit special identity for the spawned top (SPEC §6.1's
    /// `SpecialState`). LaunchParams now carries the identity explicitly
    /// (garage-built tops in M8 set it from their base preset).
    pub special_id: SpecialId,
}

/// Full match/round state (SPEC §6.1/6.5, trimmed to M2 scope).
#[derive(Clone, Debug)]
pub struct World {
    pub tops: [Top; 2],
    pub rng: Rng,
    /// EXECUTED-step counter. Deliberately frozen during hit-stop and after
    /// an outcome (those calls are whole-step skips, SPEC §5). Duration
    /// timers built in M4+ (Crash-Out window, special durations) must use
    /// their own counters decremented inside the step body — which freeze
    /// identically — and must NOT be measured as differences of this counter
    /// across hit-stop (M2 verifier finding #5).
    pub step: u64,
    pub hitstop: u8,
    pub outcome: Option<Outcome>,
    pub events: Vec<BattleEvent>,
}

/// Build one top's initial state for the Launch phase (game_design.md §4):
/// spawns on the launch circle, aimed 65% toward center / 35% tangential.
fn spawn_launch_top(p: &LaunchParams) -> Top {
    // System-boundary validation (launch params arrive from UI/AI code):
    // clamp to the documented ranges rather than trusting the caller — a
    // garbage depth would otherwise produce a garbage (even inward-negative)
    // launch speed (M2 verifier finding #7).
    let depth = p.depth.clamp(0.4, 1.0);
    let power = p.power.clamp(0.0, 1.0);
    let quality = p.quality.clamp(1.0, 1.25);

    let x = fixmath::cos(p.heading) * TUNE.launch_radius;
    let z = fixmath::sin(p.heading) * TUNE.launch_radius;
    let y = arena::height(x, z) + TUNE.launch_drop_height;

    let to_center = Vec2::new(-x, -z).normalize_or_zero();
    let tangent = Vec2::new(-to_center.y, to_center.x);
    let aim = (to_center.scaled(0.65) + tangent.scaled(0.35)).normalize_or_zero();
    let speed = TUNE.launch_speed_base + TUNE.launch_speed_depth * depth;
    let vel = Vec3::new(aim.x * speed, 0.0, aim.y * speed);

    let spin = ((TUNE.launch_spin_base + TUNE.launch_spin_power * power) * quality)
        .clamp(0.0, TUNE.spin_max);

    Top {
        pos: Vec3::new(x, y, z),
        vel,
        spin,
        spin_dir: p.spin_dir,
        tilt: Vec2::default(),
        tilt_phase: 0.0,
        spin_angle: 0.0,
        radius: 0.95,
        height: 1.0,
        stats: p.stats,
        grounded: false,
        dash_cd: 0,
        dash_active: 0,
        meter: 0.0,
        airdash_used: false,
        // LaunchParams now carries the identity explicitly (garage-built
        // tops in M8 set it from their base preset).
        combat: CombatState::new(p.special_id),
    }
}

impl World {
    /// Construct a fresh round (SPEC §6.5: round seed = `mix(match_seed,
    /// round)`, produced by the caller and passed in as `seed`).
    pub fn launch(seed: u64, params: [LaunchParams; 2]) -> World {
        World {
            tops: [spawn_launch_top(&params[0]), spawn_launch_top(&params[1])],
            rng: Rng::new(seed),
            step: 0,
            hitstop: 0,
            outcome: None,
            events: Vec::new(),
        }
    }

    /// Advance the sim by exactly one fixed 120 Hz step (SPEC §6.3, fixed
    /// order; task spec's numbered phases below map 1:1 onto the SPEC's).
    pub fn step(&mut self, inputs: [InputState; 2]) {
        // Phase 0: outcome / hitstop gate.
        if self.outcome.is_some() {
            return;
        }
        self.events.clear();
        if self.hitstop > 0 {
            self.hitstop -= 1;
            return;
        }

        // Phase 1: verb + special state machines, top 0 then top 1 (SPEC
        // §6.3 step 1, game_design.md §2/§3). Dash/Hop/special are
        // edge-style triggers; Guard/Carve/Anchor are held modifiers whose
        // hold-counters this phase updates for phases 2/5/6 to read.
        for (i, input) in inputs.into_iter().enumerate() {
            self.step_dash(i, input);
            self.step_guard(i, input);
            self.step_hop(i, input);
            self.step_carve(i, input);
            self.step_anchor(i, input);
            self.step_special_fire(i, input);
        }

        // Phase 2: control accel / gravity / slope / friction / verb+special
        // modifiers.
        for (i, input) in inputs.into_iter().enumerate() {
            self.step_forces(i, input);
        }

        // Phase 3+4: integrate, clamp, terrain contact.
        for i in 0..2 {
            self.step_integrate_and_terrain(i);
        }

        // Phase 5: spin decay + precession + verb/special spin & tilt
        // modifiers.
        for i in 0..2 {
            self.step_spin_and_precession(i);
        }

        // Phase 6: single-pair collision (parry/guard/i-frames/dash/carve/
        // aerial-slam/special resolution + meter gains from combat).
        self.step_collision();

        // Phase 7: meter passive trickle + armed-check (game_design.md §2).
        for i in 0..2 {
            self.step_meter_passive(i);
        }

        // Phase 8: final clamp pass (belt-and-suspenders — HARD RULES: clamp
        // aggressively every step).
        for top in &mut self.tops {
            finalize_clamps(top);
        }

        // Phase 9: out conditions, evaluated ATOMICALLY at the end of the
        // step from final state (M2 verifier findings #3/#4): everything
        // within a step happens first, then the round is decided once. This
        // makes a same-step cross-type double-out (one top toppling while
        // the other rings out) a genuine Simultaneous, and it means a
        // collision drain that zeroes spin is caught in the same step it
        // happened rather than one step later.
        self.resolve_outs();

        self.step += 1;
    }

    /// Dash (Space, edge; game_design.md §2): startup 2 -> active
    /// `dash_active_steps` (12) -> recovery `dash_recovery_steps` (8), with
    /// `dash_cd` (the cooldown, `72 - 0.4*SPD` steps) ticking down from the
    /// PRESS rather than from the end of recovery. This is safe because
    /// `dash_cooldown_steps()` floors at 32 (SPD=100: `72 - 40`), always
    /// well past `dash_startup_steps + dash_active_steps +
    /// dash_recovery_steps` (2+12+8 = 22), so the phase sequence always
    /// finishes before the top is eligible to dash again.
    fn step_dash(&mut self, i: usize, input: InputState) {
        // Guillotine Rush (game_design.md §3): "dash CD 0" while active —
        // forced every step so the cooldown gate never blocks a re-dash.
        if self.tops[i].combat.special_active > 0
            && self.tops[i].combat.special_id == SpecialId::GuillotineRush
        {
            self.tops[i].dash_cd = 0;
        }

        let top = &mut self.tops[i];
        if top.dash_cd > 0 {
            top.dash_cd -= 1;
        }

        // Startup -> Active transition: the impulse fires the step startup
        // reaches 0 (direction was captured at the press, see `dash_dir`'s
        // doc comment).
        if top.combat.dash_startup > 0 {
            top.combat.dash_startup -= 1;
            if top.combat.dash_startup == 0 {
                let dir = top.combat.dash_dir;
                top.vel.x += dir.x * TUNE.dash_impulse;
                top.vel.z += dir.y * TUNE.dash_impulse;
                let horiz_speed = Vec2::new(top.vel.x, top.vel.z).length();
                if horiz_speed > TUNE.dash_speed_clamp {
                    let scale = TUNE.dash_speed_clamp / horiz_speed;
                    top.vel.x *= scale;
                    top.vel.z *= scale;
                }
                top.dash_active = TUNE.dash_active_steps as u16;
                self.events.push(BattleEvent::Dash { who: i as u8 });
            }
            return;
        }

        // Active -> Recovery transition.
        if top.dash_active > 0 {
            top.dash_active -= 1;
            if top.dash_active == 0 {
                top.combat.dash_recovery = TUNE.dash_recovery_steps as u8;
            }
        }

        if top.combat.dash_recovery > 0 {
            top.combat.dash_recovery -= 1;
            return;
        }

        if !input.dash || top.dash_cd > 0 {
            return;
        }
        let can_dash = top.grounded || !top.airdash_used;
        if !can_dash {
            return;
        }
        if !top.grounded {
            top.airdash_used = true;
        }

        let mut dir = input_dir_xz(input.dir_x, input.dir_y);
        if dir.length_sq() <= 0.0 {
            let horiz = Vec2::new(top.vel.x, top.vel.z);
            dir = horiz.normalize_or_zero();
            if dir.length_sq() <= 0.0 {
                dir = Vec2::new(1.0, 0.0);
            }
        }
        top.combat.dash_dir = dir;
        top.combat.dash_startup = TUNE.dash_startup_steps as u8;
        top.dash_cd = combat::effective_stats(top).dash_cooldown_steps();
    }

    /// Guard (Z, held; game_design.md §2). `guard_hold` counts steps held
    /// (`1` on the press step, incrementing while held). The parry window
    /// (first `guard_parry_window_steps` = 8 steps of a press) SUBSUMES
    /// `guard_startup_steps` (4): parry protection covers the whole startup
    /// and a few steps beyond it, so there is no genuinely vulnerable
    /// "pressed but not yet parrying-or-blocking" gap — a documented reading
    /// of the design doc's two windows as overlapping rather than
    /// sequential (see the milestone report). Airborne: cannot Guard
    /// (game_design.md §2 Hop doc).
    fn step_guard(&mut self, i: usize, input: InputState) {
        let top = &mut self.tops[i];

        if top.combat.guard_drop_recovery > 0 {
            top.combat.guard_drop_recovery -= 1;
        }

        if !top.grounded {
            if top.combat.guard_hold > 0 {
                top.combat.guard_drop_recovery = TUNE.guard_drop_recovery_steps as u8;
            }
            top.combat.guard_hold = 0;
            return;
        }

        // Drop-recovery gates RE-engaging Guard (a fresh press) but never
        // stops the top-of-function decrement above from eventually
        // clearing it; an already-continuously-held Guard is unaffected
        // since `guard_hold` never returns to 0 while genuinely held.
        if input.guard && top.combat.guard_drop_recovery == 0 {
            top.combat.guard_hold = top.combat.guard_hold.saturating_add(1);
        } else if top.combat.guard_hold > 0 {
            top.combat.guard_hold = 0;
            top.combat.guard_drop_recovery = TUNE.guard_drop_recovery_steps as u8;
        }
    }

    /// Hop (X, edge; game_design.md §2). `hop_air_steps` counts steps since
    /// the PRESS (see `CombatState::hop_air_steps`'s doc comment): the
    /// vertical impulse fires once, the step the counter reaches
    /// `hop_startup_steps + 1`.
    fn step_hop(&mut self, i: usize, input: InputState) {
        let top = &mut self.tops[i];

        if top.combat.hop_land_lag > 0 {
            top.combat.hop_land_lag -= 1;
        }

        if top.combat.hop_air_steps > 0 {
            let fire_at = TUNE.hop_startup_steps as u16 + 1;
            if top.combat.hop_air_steps == fire_at {
                top.vel.y += TUNE.hop_impulse;
                top.combat.hop_apex_y = top.pos.y;
            }
            let dir = input_dir_xz(input.dir_x, input.dir_y);
            if dir.length_sq() > 0.0 {
                top.combat.hop_slam_armed = true;
            }
            top.combat.hop_air_steps = top.combat.hop_air_steps.saturating_add(1);
            return;
        }

        if !input.hop || top.combat.hop_land_lag > 0 || !top.grounded {
            return;
        }

        top.spin = (top.spin - TUNE.hop_spin_cost).max(0.0);
        top.combat.hop_slam_armed = false;
        top.combat.hop_air_steps = 1;
    }

    /// Carve (C, held; game_design.md §2). `carve_hold` drives the 20-step
    /// ramp for speed/slope-climb/knockback bonuses (read in `step_forces`/
    /// `step_collision`); `carve_tilt_bonus` grows at a FLAT (unramped) rate
    /// while held and decays linearly to exactly `0` over
    /// `carve_release_decay_steps` after release (see the field's doc
    /// comment on `CombatState`).
    fn step_carve(&mut self, i: usize, input: InputState) {
        let top = &mut self.tops[i];

        if input.carve {
            top.combat.carve_hold = top.combat.carve_hold.saturating_add(1);
            top.combat.carve_release_decay = 0;
            top.combat.carve_tilt_bonus += TUNE.carve_tilt_rate_per_s * SIM_DT;
        } else {
            top.combat.carve_hold = 0;
            if top.combat.carve_release_decay == 0 && top.combat.carve_tilt_bonus > 0.0 {
                top.combat.carve_release_decay = TUNE.carve_release_decay_steps as u8;
            }
            if top.combat.carve_release_decay > 0 {
                let frac = 1.0 / top.combat.carve_release_decay as f32;
                top.combat.carve_tilt_bonus -= top.combat.carve_tilt_bonus * frac;
                top.combat.carve_release_decay -= 1;
            }
        }
    }

    /// Anchor (Ctrl, held; game_design.md §2): auto-breaks (forced release)
    /// on a slope steeper than 12 degrees. Airborne: cannot Anchor
    /// (game_design.md §2 Hop doc).
    fn step_anchor(&mut self, i: usize, input: InputState) {
        let top = &mut self.tops[i];

        if top.combat.anchor_release_lag > 0 {
            top.combat.anchor_release_lag -= 1;
        }

        if !top.grounded {
            if top.combat.anchor_hold > 0 {
                top.combat.anchor_release_lag = TUNE.anchor_release_steps as u8;
            }
            top.combat.anchor_hold = 0;
            return;
        }

        // Release-lag gates RE-engaging Anchor (a fresh press), mirroring
        // Guard's drop-recovery gate above; an already-continuously-held
        // Anchor is unaffected.
        if !input.anchor || (top.combat.anchor_hold == 0 && top.combat.anchor_release_lag > 0) {
            if top.combat.anchor_hold > 0 {
                top.combat.anchor_hold = 0;
                top.combat.anchor_release_lag = TUNE.anchor_release_steps as u8;
            }
            return;
        }

        top.combat.anchor_hold = top.combat.anchor_hold.saturating_add(1);

        let (dhdx, dhdz) = arena::gradient(top.pos.x, top.pos.z);
        let slope = Vec2::new(dhdx, dhdz).length();
        if slope > TUNE.anchor_break_slope_tan_threshold {
            top.combat.anchor_hold = 0;
            top.combat.anchor_release_lag = TUNE.anchor_release_steps as u8;
            self.events.push(BattleEvent::AnchorBreak { who: i as u8 });
        }
    }

    /// Special fire (Shift, TRUE rising edge while Armed; game_design.md
    /// §2/§3 — see `CombatState::special_was_pressed`'s doc comment for why
    /// this needs a genuine edge unlike Dash/Hop). Sets up the special's own
    /// active-duration effect AND opens the 144-step Crash-Out window
    /// (independent timers, both decremented at the top of this same method
    /// next step — mirroring the existing `dash_cd`/`dash_active`
    /// decrement-then-assign-fresh pattern — so "kill inside 144 steps"
    /// means exactly steps `fire_step ..= fire_step + 143`, see the
    /// milestone report for the worked-out step-counting argument).
    fn step_special_fire(&mut self, i: usize, input: InputState) {
        {
            let top = &mut self.tops[i];
            if top.combat.special_active > 0 {
                // Riposte fizzle: the window is about to close having never
                // triggered (game_design.md §3: "fizzle refunds 30 meter").
                if top.combat.special_active == 1
                    && top.combat.special_id == SpecialId::Riposte
                    && !top.combat.special_flag
                {
                    top.meter = (top.meter + TUNE.special_riposte_fizzle_refund).min(100.0);
                }
                top.combat.special_active -= 1;
            }
            if top.combat.crash_window > 0 {
                top.combat.crash_window -= 1;
            }
        }

        let edge = input.special && !self.tops[i].combat.special_was_pressed;
        self.tops[i].combat.special_was_pressed = input.special;

        if !edge || !self.tops[i].combat.special_armed {
            return;
        }

        let top = &mut self.tops[i];
        top.combat.special_armed = false;
        top.meter = 0.0;
        top.combat.special_flag = false;
        top.combat.special_active = top.combat.special_id.duration_steps();
        top.combat.crash_window = TUNE.crash_window_steps;

        if top.combat.special_id == SpecialId::SecondWind {
            top.spin = (top.spin + TUNE.spin_max * TUNE.special_secondwind_spin_bonus_frac)
                .clamp(0.0, TUNE.spin_max);
        }

        self.events.push(BattleEvent::SpecialFire { who: i as u8 });
        self.hitstop = self.hitstop.max(TUNE.hitstop_special_fire as u8);
    }

    fn step_forces(&mut self, i: usize, input: InputState) {
        let opponent = self.tops[1 - i]; // Top is Copy: cheap, avoids a borrow conflict.
        let top = &mut self.tops[i];
        let dir = input_dir_xz(input.dir_x, input.dir_y);
        if dir.length_sq() > 0.0 {
            top.combat.last_move_dir = dir;
        }

        let air_factor = if top.grounded {
            1.0
        } else {
            TUNE.air_ctrl_factor
        };

        // Verb/special move-control multipliers (game_design.md §2/§3).
        // Dash startup/recovery and Aegis Lock's "rooted" zero it entirely;
        // hop land-lag/Guard/Anchor slow it. Multiplicative: these verb
        // states are mutually exclusive for one top in one step in
        // practice, but multiplying keeps the formula total regardless.
        let mut move_mult = 1.0;
        if top.combat.dash_startup > 0 || top.combat.dash_recovery > 0 {
            move_mult = 0.0;
        }
        if top.combat.hop_land_lag > 0 {
            move_mult *= TUNE.hop_land_lag_ctrl_mult;
        }
        if top.combat.guard_hold > 0 {
            move_mult *= TUNE.guard_move_mult;
        }
        if top.combat.anchor_hold > 0 {
            move_mult *= TUNE.anchor_move_mult;
        }
        if combat::is_rooted_by_special(top) {
            move_mult = 0.0;
        }

        let eff_stats = combat::effective_stats(top);
        let accel =
            eff_stats.ctrl_accel() * air_factor * move_mult * combat::special_accel_mult(top);
        top.vel.x += dir.x * accel * SIM_DT;
        top.vel.z += dir.y * accel * SIM_DT;

        top.vel.y -= TUNE.gravity * SIM_DT;

        if top.grounded {
            let (dhdx, dhdz) = arena::gradient(top.pos.x, top.pos.z);

            // Carve: ramped slope-climb bonus (game_design.md §2).
            let carve_frac = carve_ramp_frac(top);
            let climb_mult = if top.combat.carve_hold > 0 {
                1.0 + (TUNE.carve_slope_climb_mult - 1.0) * carve_frac
            } else {
                1.0
            };
            // Anchor: downhill slide reduced to `anchor_slide_mult`.
            let slide_mult = if top.combat.anchor_hold > 0 {
                TUNE.anchor_slide_mult
            } else {
                1.0
            };
            top.vel.x += -dhdx * TUNE.slope_accel * climb_mult * slide_mult * SIM_DT;
            top.vel.z += -dhdz * TUNE.slope_accel * climb_mult * slide_mult * SIM_DT;

            // Guard: extra downhill slide on slopes > 6 degrees
            // (game_design.md §2: "slides downhill x0.6 EXTRA" — documented
            // interpretation: an ADDITIONAL slope-accel term on top of the
            // normal one above, while guard is held on a steep slope; see
            // the milestone report).
            let slope_mag = Vec2::new(dhdx, dhdz).length();
            if top.combat.guard_hold > 0 && slope_mag > TUNE.guard_slope_tan_threshold {
                top.vel.x += -dhdx * TUNE.slope_accel * TUNE.guard_slope_extra_mult * SIM_DT;
                top.vel.z += -dhdz * TUNE.slope_accel * TUNE.guard_slope_extra_mult * SIM_DT;
            }

            let friction = (1.0 - TUNE.ground_friction * SIM_DT).max(0.0);
            top.vel.x *= friction;
            top.vel.z *= friction;
        }

        // Guillotine Rush: steering-homing toward the opponent
        // (game_design.md §3: "0.3/step"), preserving speed.
        if top.combat.special_active > 0 && top.combat.special_id == SpecialId::GuillotineRush {
            let to_opp = Vec2::new(opponent.pos.x - top.pos.x, opponent.pos.z - top.pos.z)
                .normalize_or_zero();
            let horiz = Vec2::new(top.vel.x, top.vel.z);
            let speed = horiz.length();
            if speed > 0.01 && to_opp.length_sq() > 0.0 {
                let cur_dir = horiz.normalize_or_zero();
                let homing = TUNE.special_guillotine_homing;
                let new_dir =
                    (cur_dir.scaled(1.0 - homing) + to_opp.scaled(homing)).normalize_or_zero();
                top.vel.x = new_dir.x * speed;
                top.vel.z = new_dir.y * speed;
            }
        }

        // Sinkhole: pull toward the OPPONENT if THEY have it active
        // (game_design.md §3: "self immune" — a top only ever reads its
        // OPPONENT'S Sinkhole state here, never its own, so the owner is
        // never pulled toward itself by construction).
        if opponent.combat.special_active > 0 && opponent.combat.special_id == SpecialId::Sinkhole {
            let dx = opponent.pos.x - top.pos.x;
            let dz = opponent.pos.z - top.pos.z;
            let dist_sq = dx * dx + dz * dz;
            let radius = TUNE.special_sinkhole_radius;
            if dist_sq < radius * radius && dist_sq > 1e-6 {
                let dist = fixmath::sqrt(dist_sq);
                let pull = TUNE.special_sinkhole_pull_accel;
                top.vel.x += (dx / dist) * pull * SIM_DT;
                top.vel.z += (dz / dist) * pull * SIM_DT;
            }
        }
    }

    fn step_integrate_and_terrain(&mut self, i: usize) {
        let top = &mut self.tops[i];

        // Velocity clamps (ground/air/fall speed). Carve: ramped top-speed
        // bonus (game_design.md §2: "top speed x1.5").
        let base_max_h = if top.grounded {
            TUNE.max_ground_speed
        } else {
            TUNE.max_air_speed
        };
        let max_h = if top.combat.carve_hold > 0 {
            let ramp = carve_ramp_frac(top);
            base_max_h * (1.0 + (TUNE.carve_top_speed_mult - 1.0) * ramp)
        } else {
            base_max_h
        };
        let horiz = Vec2::new(top.vel.x, top.vel.z);
        let horiz_len = horiz.length();
        if horiz_len > max_h {
            let scale = max_h / horiz_len;
            top.vel.x *= scale;
            top.vel.z *= scale;
        }
        top.vel.y = top.vel.y.clamp(-TUNE.max_fall_speed, TUNE.max_fall_speed);

        // Integrate (semi-implicit Euler).
        top.pos = top.pos + top.vel.scaled(SIM_DT);

        // Position clamps.
        top.pos.x = top.pos.x.clamp(-POS_XZ_LIMIT, POS_XZ_LIMIT);
        top.pos.z = top.pos.z.clamp(-POS_XZ_LIMIT, POS_XZ_LIMIT);
        top.pos.y = top.pos.y.min(POS_Y_LIMIT);

        // Hop: track the highest point reached since the impulse fired, for
        // the aerial slam's fall-height calculation (game_design.md §2).
        if top.combat.hop_air_steps > 0 {
            top.combat.hop_apex_y = top.combat.hop_apex_y.max(top.pos.y);
        }

        // Terrain contact. `pos.y` is the tip/contact height (documented on
        // `Top::pos`), so it's compared directly against the heightfield —
        // no radius subtraction.
        let h = arena::height(top.pos.x, top.pos.z);
        if top.pos.y < h {
            top.pos.y = h;
            let n = arena::normal(top.pos.x, top.pos.z);
            let v_n = top.vel.dot(n);
            if v_n < 0.0 {
                let impact = -v_n;
                if impact > 2.0 {
                    // Hard vertical landing: a genuine impact, so let the
                    // into-surface component actually dissipate (SPEC-literal
                    // `v -= n*(v.n)`), then add the small upward rebound
                    // (positive: away from the surface — M2 verifier BLOCKER
                    // #1 had this sign inverted, burying tops into the floor).
                    top.vel = top.vel - n.scaled(v_n);
                    top.vel.y = impact * 0.25;
                    self.events.push(BattleEvent::Landed {
                        who: i as u8,
                        impact,
                    });
                } else {
                    // Gentle, continuous ground contact (e.g. climbing a
                    // rising slope): re-orient velocity to be tangent to the
                    // surface WITHOUT bleeding speed. A literal `v -=
                    // n*(v.n)` here would remove real kinetic energy every
                    // single 120 Hz step purely because the surface curves
                    // (gravity keeps nudging vel.y "wrong" relative to the
                    // slope, and a naive projection blames that error on the
                    // large horizontal component too) — compounding into a
                    // top that can barely climb a 9-degree incline. A ball
                    // rolling smoothly along a curving surface doesn't lose
                    // speed from the curvature alone, so preserve `|vel|`
                    // and only correct its direction. Hard impacts (above)
                    // still dissipate energy as specified.
                    //
                    // Speed is preserved ONLY when real tangential motion
                    // exists. For a settling top the velocity is essentially
                    // pure into-surface; the tangential remainder is numeric
                    // noise (the approximate-rsqrt normal leaves ~1e-5
                    // residue), and renormalizing that noise to full |vel|
                    // resurrected the killed component in a junk direction —
                    // the top never came to rest (M2 verifier BLOCKER #2).
                    let speed = top.vel.length();
                    let tangential = top.vel - n.scaled(v_n);
                    let min_tangent = 0.15 * speed;
                    if tangential.length_sq() > min_tangent * min_tangent {
                        top.vel = tangential.normalize_or_zero().scaled(speed);
                    } else {
                        top.vel = tangential;
                    }
                }
            }
            top.grounded = true;
            top.airdash_used = false;
            // Landing ends a hop's airborne phase (game_design.md §2:
            // "land-lag 10"). The aerial slam itself is resolved against an
            // opponent CONTACT in `step_collision`, not a terrain landing —
            // this just closes out the hop cycle so a later terrain landing
            // doesn't leave a stale slam armed for the NEXT hop.
            if top.combat.hop_air_steps > 0 {
                top.combat.hop_air_steps = 0;
                top.combat.hop_land_lag = TUNE.hop_land_lag_steps as u8;
                top.combat.hop_slam_armed = false;
            }
        } else {
            top.grounded = false;
        }
    }

    /// Spin decay + precession/wobble update. Out conditions are NOT decided
    /// here — [`World::resolve_outs`] evaluates them atomically at the end of
    /// the step (M2 verifier findings #3/#4).
    fn step_spin_and_precession(&mut self, i: usize) {
        let top = &mut self.tops[i];
        let eff_stats = combat::effective_stats(top);

        // Second Wind: near-zero decay for the buff's duration
        // (game_design.md §3).
        let mut decay = eff_stats.decay_per_s();
        if top.combat.special_active > 0 && top.combat.special_id == SpecialId::SecondWind {
            decay *= TUNE.special_secondwind_decay_mult;
        }
        top.spin -= decay * SIM_DT;
        if top.dash_active > 0 {
            top.spin -= 2.0;
        }
        // Guard: 90 spin/s while held. Anchor: +150 spin/s regen while held
        // (game_design.md §2).
        if top.combat.guard_hold > 0 {
            top.spin -= TUNE.guard_spin_cost_per_s * SIM_DT;
        }
        if top.combat.anchor_hold > 0 {
            top.spin += TUNE.anchor_spin_regen_per_s * SIM_DT;
        }
        top.spin = top.spin.clamp(0.0, TUNE.spin_max);

        // Visual-only accumulated spin angle (M3-B): NOT fed back into
        // physics, purely render state (see `pose_lerp`). Wrapped by branch
        // (never `.fract()`, which the no-libm scan bans outside fixmath.rs)
        // into `[0, 2*pi)`. A single subtract/add suffices because the
        // per-step increment magnitude (`spin_max * spin_angle_rate * SIM_DT`
        // ~= 3.75 rad) never reaches `TAU` (~6.28), so the previous
        // already-wrapped value plus one step's delta can cross the `[0,
        // TAU)` boundary at most once in either direction.
        top.spin_angle += top.spin * TUNE.spin_angle_rate * top.spin_dir as f32 * SIM_DT;
        if top.spin_angle >= TAU {
            top.spin_angle -= TAU;
        } else if top.spin_angle < 0.0 {
            top.spin_angle += TAU;
        }

        let spin_frac = top.spin_frac();
        let one_minus = 1.0 - spin_frac;
        let wobble_freq = TUNE.wobble_freq_base + TUNE.wobble_freq_growth * one_minus;
        top.tilt_phase += 2.0 * std::f32::consts::PI * wobble_freq * SIM_DT;

        let stability = (eff_stats.wgt as f32 / 100.0) * (eff_stats.sta as f32 / 100.0);
        let target_mag = (TUNE.tilt_base + TUNE.tilt_growth * one_minus * one_minus)
            * (1.2 - TUNE.tilt_stability_mul * stability);
        let target_mag = target_mag.max(0.0);

        let current_mag = top.tilt.length();
        // Second Wind: tilt recovery x1.5 (game_design.md §3) — modeled as a
        // faster rate-limited approach toward `target_mag` in either
        // direction (a documented simplification: the design doc frames
        // this as "recovery", but the sim's tilt model has no separate
        // growing-vs-shrinking rate to selectively speed up).
        let mut max_step = TUNE.tilt_rate * SIM_DT;
        if top.combat.special_active > 0 && top.combat.special_id == SpecialId::SecondWind {
            max_step *= TUNE.special_secondwind_tilt_recovery_mult;
        }
        let diff = target_mag - current_mag;
        let mut new_mag = if diff > max_step {
            current_mag + max_step
        } else if diff < -max_step {
            current_mag - max_step
        } else {
            target_mag
        }
        .max(0.0);

        // Anchor: extra flat tilt recovery, 0.08 rad/s (game_design.md §2).
        if top.combat.anchor_hold > 0 {
            new_mag = (new_mag - TUNE.anchor_tilt_recovery_per_s * SIM_DT).max(0.0);
        }

        // Carve: accumulated tilt bonus (game_design.md §2), added post-
        // rate-limit since it is already smooth/gradual by construction
        // (see `step_carve`).
        new_mag += top.combat.carve_tilt_bonus;

        top.tilt =
            Vec2::new(fixmath::cos(top.tilt_phase), fixmath::sin(top.tilt_phase)).scaled(new_mag);
    }

    /// Whether `top` is currently in its hop's de-penetration i-frame window
    /// (game_design.md §2: "de-penetration i-frames steps 4-12 of the hop").
    fn hop_iframe_active(top: &Top) -> bool {
        let s = top.combat.hop_air_steps;
        s >= TUNE.hop_iframe_start as u16 && s <= TUNE.hop_iframe_end as u16
    }

    /// Guard-block/Anchor/Aegis-Lock multiplicative defense reduction on
    /// what `top` RECEIVES (game_design.md §2/§3). Does NOT cover Parry or
    /// Riposte, which fully override/negate the hit instead — see
    /// `step_collision`'s caller.
    fn defense_reduction(top: &Top, frontal: bool) -> (f32, f32) {
        let mut knock = 1.0;
        let mut drain = 1.0;
        if frontal && top.combat.guard_hold > TUNE.guard_parry_window_steps as u16 {
            knock *= TUNE.guard_knock_mult;
            drain *= TUNE.guard_drain_mult;
        }
        if top.combat.anchor_hold > 0 {
            knock *= TUNE.anchor_knock_mult;
        }
        if top.combat.special_active > 0 && top.combat.special_id == SpecialId::AegisLock {
            knock *= TUNE.special_aegis_knock_mult;
        }
        (knock, drain)
    }

    /// Slipstream's one-time pass-through exit hit (game_design.md §3):
    /// `user` is intangible (no de-penetration, no incoming knockback/
    /// drain) and deals a bonus hit to `opp` instead. Consumes
    /// `special_flag` so the pass only happens once per activation.
    fn resolve_slipstream_exit(&mut self, user: usize, opp: usize, n: Vec3) {
        let user_top = self.tops[user];
        let opp_top = self.tops[opp];
        let rel_speed = (user_top.vel - opp_top.vel).length();
        let eff_user = combat::effective_stats(&user_top);
        let eff_opp = combat::effective_stats(&opp_top);

        // `n` = normalize(center_0 - center_1) (points from top 1 toward top
        // 0). The exit push on `opp` must point AWAY from `user`: that's
        // `-n` when `user == 0` (push points from 0 toward 1) and `+n` when
        // `user == 1` (push points from 1 toward 0) — the same convention
        // `step_collision`'s `dv_a_final`/`dv_b_final` application uses
        // (`+n` for top 0, `-n` for top 1).
        let facing_opp = combat::facing_xz(&opp_top);
        let n_sign = if user == 0 { -1.0 } else { 1.0 };
        let exit_dir = Vec2::new(n.x * n_sign, n.z * n_sign).normalize_or_zero();
        // Backstab (game_design.md §3: "exit angle > 90 degrees off defender
        // facing"): `exit_dir` points away from the user, i.e. roughly the
        // direction the user was traveling relative to the defender: if the
        // defender's own facing agrees with that direction (positive dot),
        // the defender had their back to the user — a backstab.
        let backstab = facing_opp.dot(exit_dir) > 0.0;
        let mut mult = TUNE.special_slipstream_exit_knock_mult;
        if backstab {
            mult *= TUNE.special_slipstream_backstab_bonus;
        }

        let frontal_opp = combat::is_frontal_hit(facing_opp, exit_dir.scaled(-1.0));
        let (def_knock, def_drain) = World::defense_reduction(&opp_top, frontal_opp);

        let knock = rel_speed * TUNE.knock_scale * eff_user.knock_mult() * mult
            / eff_opp.mass().max(1.0)
            * eff_opp.knock_taken_mult()
            * def_knock;
        let drain = (TUNE.drain_scale * (rel_speed / 6.0) * eff_user.knock_mult() * mult)
            .max(TUNE.drain_min)
            * eff_opp.drain_taken_mult()
            * def_drain;

        let push = Vec3::new(exit_dir.x, 0.0, exit_dir.y).scaled(knock);
        self.tops[opp].vel = self.tops[opp].vel + push;
        self.tops[opp].spin = (self.tops[opp].spin - drain).clamp(0.0, TUNE.spin_max);
        self.tops[user].combat.special_flag = true;

        let gain = combat::scaled_meter_gain(drain * TUNE.meter_drain_dealt_mult, &user_top.stats);
        self.tops[user].meter = (self.tops[user].meter + gain).min(100.0);

        self.events
            .push(BattleEvent::SpecialHit { who: user as u8 });
        self.hitstop = self.hitstop.max(TUNE.hitstop_heavy as u8);
    }

    fn step_collision(&mut self) {
        let center_a = self.tops[0].pos + Vec3::new(0.0, BODY_CENTER_OFFSET, 0.0);
        let center_b = self.tops[1].pos + Vec3::new(0.0, BODY_CENTER_OFFSET, 0.0);
        let delta = center_a - center_b;
        let dist = delta.length();
        let sum_r = self.tops[0].radius + self.tops[1].radius;
        if dist >= sum_r {
            return;
        }

        let n = if dist > 1e-6 {
            delta.scaled(1.0 / dist)
        } else {
            Vec3::new(1.0, 0.0, 0.0)
        };

        // Slipstream: intangible pass-through (game_design.md §3), checked
        // BEFORE de-penetration since its user does not get pushed apart.
        for (user, opp) in [(0usize, 1usize), (1, 0)] {
            let c = self.tops[user].combat;
            if c.special_active > 0 && c.special_id == SpecialId::Slipstream && !c.special_flag {
                self.resolve_slipstream_exit(user, opp, n);
                return;
            }
        }

        // De-penetration by inverse-mass split (pre-collision masses; NOT
        // Overclock-adjusted — see `combat::effective_stats`'s doc comment).
        let mass_a = self.tops[0].stats.mass();
        let mass_b = self.tops[1].stats.mass();
        let inv_a = 1.0 / mass_a;
        let inv_b = 1.0 / mass_b;
        let total_inv = inv_a + inv_b;
        let penetration = sum_r - dist;
        if total_inv > 0.0 {
            self.tops[0].pos = self.tops[0].pos + n.scaled(penetration * inv_a / total_inv);
            self.tops[1].pos = self.tops[1].pos - n.scaled(penetration * inv_b / total_inv);
        }

        // Snapshot pre-collision state so both tops' deltas are computed
        // from the SAME starting point (fixed evaluation order / symmetry).
        let vel_a = self.tops[0].vel;
        let vel_b = self.tops[1].vel;
        let v_rel = (vel_b - vel_a).dot(n); // positive == approaching

        // Hop i-frames (game_design.md §2): "no collision impulses in or
        // out, pass-through de-penetration only" — the position correction
        // above still applies; nothing else does. A dodged "real hit"
        // (v_rel > 0, i.e. a hit that would otherwise have landed) grants
        // the i-frame dodge meter bonus.
        let iframe_a = World::hop_iframe_active(&self.tops[0]);
        let iframe_b = World::hop_iframe_active(&self.tops[1]);
        if iframe_a || iframe_b {
            if v_rel > 0.0 {
                if iframe_a {
                    let gain = combat::scaled_meter_gain(
                        TUNE.meter_gain_iframe_dodge,
                        &self.tops[0].stats,
                    );
                    self.tops[0].meter = (self.tops[0].meter + gain).min(100.0);
                }
                if iframe_b {
                    let gain = combat::scaled_meter_gain(
                        TUNE.meter_gain_iframe_dodge,
                        &self.tops[1].stats,
                    );
                    self.tops[1].meter = (self.tops[1].meter + gain).min(100.0);
                }
            }
            return;
        }

        if v_rel <= 0.0 {
            return;
        }

        let stats_a = combat::effective_stats(&self.tops[0]);
        let stats_b = combat::effective_stats(&self.tops[1]);
        let spin_frac_a = self.tops[0].spin_frac();
        let spin_frac_b = self.tops[1].spin_frac();
        let same_dir = self.tops[0].spin_dir == self.tops[1].spin_dir;
        let dash_active_a = self.tops[0].dash_active > 0;
        let dash_active_b = self.tops[1].dash_active > 0;
        let grounded_a = self.tops[0].grounded;
        let grounded_b = self.tops[1].grounded;
        let carve_a = self.tops[0].combat.carve_hold > 0;
        let carve_b = self.tops[1].combat.carve_hold > 0;
        let carve_ramp_a = carve_ramp_frac(&self.tops[0]);
        let carve_ramp_b = carve_ramp_frac(&self.tops[1]);

        let dash_knock_mul_a = if dash_active_a {
            TUNE.dash_knock_mul
        } else {
            1.0
        };
        let dash_knock_mul_b = if dash_active_b {
            TUNE.dash_knock_mul
        } else {
            1.0
        };
        let dash_drain_mul_a = if dash_active_a {
            TUNE.dash_drain_mul
        } else {
            1.0
        };
        let dash_drain_mul_b = if dash_active_b {
            TUNE.dash_drain_mul
        } else {
            1.0
        };
        // Carve: contact knockback bonus, ramped (game_design.md §2).
        let carve_knock_mul_a = if carve_a {
            1.0 + (TUNE.carve_knock_mult - 1.0) * carve_ramp_a
        } else {
            1.0
        };
        let carve_knock_mul_b = if carve_b {
            1.0 + (TUNE.carve_knock_mult - 1.0) * carve_ramp_b
        } else {
            1.0
        };
        let clash_mul = if same_dir { TUNE.clash_knock_mul } else { 1.0 };
        let grind_mul = if !same_dir { TUNE.grind_drain_mul } else { 1.0 };

        // Guillotine Rush: one-time next-contact bonus (game_design.md §3),
        // consumed by whichever side has it active and untriggered.
        let guillotine_bonus_a = self.tops[0].combat.special_active > 0
            && self.tops[0].combat.special_id == SpecialId::GuillotineRush
            && !self.tops[0].combat.special_flag;
        let guillotine_bonus_b = self.tops[1].combat.special_active > 0
            && self.tops[1].combat.special_id == SpecialId::GuillotineRush
            && !self.tops[1].combat.special_flag;
        let guillotine_knock_mul_a = if guillotine_bonus_a {
            TUNE.special_guillotine_knock_mult
        } else {
            1.0
        };
        let guillotine_knock_mul_b = if guillotine_bonus_b {
            TUNE.special_guillotine_knock_mult
        } else {
            1.0
        };

        let j = (1.0 + TUNE.restitution) * v_rel / total_inv;

        // Dealt-side multiplier (attacker bonuses) for what the OTHER top
        // receives; defender-side reduction is applied afterward.
        let dealt_mul_from_b = dash_knock_mul_b * carve_knock_mul_b * guillotine_knock_mul_b;
        let dealt_mul_from_a = dash_knock_mul_a * carve_knock_mul_a * guillotine_knock_mul_a;

        let dv_a_mag = (j / mass_a)
            * (1.0 + TUNE.knock_scale * stats_b.knock_mult() * spin_frac_b * dealt_mul_from_b)
            * stats_a.knock_taken_mult()
            * clash_mul;
        let dv_b_mag = (j / mass_b)
            * (1.0 + TUNE.knock_scale * stats_a.knock_mult() * spin_frac_a * dealt_mul_from_a)
            * stats_b.knock_taken_mult()
            * clash_mul;

        let drain_a_base = (TUNE.drain_scale * (v_rel / 6.0) * stats_b.knock_mult() * spin_frac_b)
            .max(TUNE.drain_min);
        let drain_b_base = (TUNE.drain_scale * (v_rel / 6.0) * stats_a.knock_mult() * spin_frac_a)
            .max(TUNE.drain_min);
        let drain_a = drain_a_base * stats_a.drain_taken_mult() * grind_mul * dash_drain_mul_b;
        let drain_b = drain_b_base * stats_b.drain_taken_mult() * grind_mul * dash_drain_mul_a;

        // Defender-side reduction: Guard-block/Anchor/Aegis Lock multiply,
        // Parry/Riposte fully override (highest priority).
        let facing_a = combat::facing_xz(&self.tops[0]);
        let facing_b = combat::facing_xz(&self.tops[1]);
        let attacker_dir_a = Vec2::new(-n.x, -n.z);
        let attacker_dir_b = Vec2::new(n.x, n.z);
        let frontal_a = combat::is_frontal_hit(facing_a, attacker_dir_a);
        let frontal_b = combat::is_frontal_hit(facing_b, attacker_dir_b);
        let guard_hold_a = self.tops[0].combat.guard_hold;
        let guard_hold_b = self.tops[1].combat.guard_hold;
        let parry_a =
            frontal_a && guard_hold_a >= 1 && guard_hold_a <= TUNE.guard_parry_window_steps as u16;
        let parry_b =
            frontal_b && guard_hold_b >= 1 && guard_hold_b <= TUNE.guard_parry_window_steps as u16;
        let guard_block_a = !parry_a
            && frontal_a
            && guard_hold_a > 0
            && guard_hold_a > TUNE.guard_parry_window_steps as u16;
        let guard_block_b = !parry_b
            && frontal_b
            && guard_hold_b > 0
            && guard_hold_b > TUNE.guard_parry_window_steps as u16;
        let riposte_a = self.tops[0].combat.special_active > 0
            && self.tops[0].combat.special_id == SpecialId::Riposte
            && !self.tops[0].combat.special_flag;
        let riposte_b = self.tops[1].combat.special_active > 0
            && self.tops[1].combat.special_id == SpecialId::Riposte
            && !self.tops[1].combat.special_flag;

        let (def_knock_a, def_drain_a) = World::defense_reduction(&self.tops[0], frontal_a);
        let (def_knock_b, def_drain_b) = World::defense_reduction(&self.tops[1], frontal_b);

        let mut dv_a_final = dv_a_mag * def_knock_a;
        let mut drain_a_final = drain_a * def_drain_a;
        let mut dv_b_final = dv_b_mag * def_knock_b;
        let mut drain_b_final = drain_b * def_drain_b;

        if parry_a {
            dv_a_final = dv_a_mag * TUNE.guard_parry_knock_mult;
            drain_a_final = drain_a * TUNE.guard_parry_drain_mult;
        }
        if parry_b {
            dv_b_final = dv_b_mag * TUNE.guard_parry_knock_mult;
            drain_b_final = drain_b * TUNE.guard_parry_drain_mult;
        }

        // Aegis Lock: reflect 50% of the absorbed drain back to the
        // attacker (game_design.md §3).
        let aegis_a = self.tops[0].combat.special_active > 0
            && self.tops[0].combat.special_id == SpecialId::AegisLock;
        let aegis_b = self.tops[1].combat.special_active > 0
            && self.tops[1].combat.special_id == SpecialId::AegisLock;
        let mut reflect_to_b = 0.0;
        let mut reflect_to_a = 0.0;
        if aegis_a && !riposte_a {
            reflect_to_b = drain_a_final * TUNE.special_aegis_reflect_frac;
        }
        if aegis_b && !riposte_b {
            reflect_to_a = drain_b_final * TUNE.special_aegis_reflect_frac;
        }

        // Riposte: negate the hit and reverse it along the ATTACKER's own
        // velocity, transferring 60% of the negated drain (game_design.md
        // §3). Takes precedence over Guard/Anchor/Aegis Lock for the same
        // defender (checked last so it overwrites any of the above).
        let mut riposte_reverse_knock_a = 0.0; // extra knockback applied TO top 1 (from top 0's riposte)
        let mut riposte_reverse_knock_b = 0.0; // extra knockback applied TO top 0 (from top 1's riposte)
        if riposte_a {
            let negated_drain = drain_a_final.max(drain_a);
            dv_a_final = 0.0;
            drain_a_final = 0.0;
            reflect_to_b = 0.0;
            riposte_reverse_knock_a = dv_b_mag.max(dv_a_mag) * TUNE.special_riposte_knock_mult;
            drain_b_final += negated_drain * TUNE.special_riposte_drain_transfer;
            self.tops[0].combat.special_flag = true;
            self.events.push(BattleEvent::SpecialHit { who: 0 });
        }
        if riposte_b {
            let negated_drain = drain_b_final.max(drain_b);
            dv_b_final = 0.0;
            drain_b_final = 0.0;
            reflect_to_a = 0.0;
            riposte_reverse_knock_b = dv_a_mag.max(dv_b_mag) * TUNE.special_riposte_knock_mult;
            drain_a_final += negated_drain * TUNE.special_riposte_drain_transfer;
            self.tops[1].combat.special_flag = true;
            self.events.push(BattleEvent::SpecialHit { who: 1 });
        }

        drain_a_final += reflect_to_a;
        drain_b_final += reflect_to_b;

        // Aerial slam (game_design.md §2): additive bonus on top of the
        // normal hit for whichever side is descending from an armed hop.
        let slam_a = self.tops[0].combat.hop_slam_armed
            && !self.tops[0].grounded
            && self.tops[0].vel.y < 0.0;
        let slam_b = self.tops[1].combat.hop_slam_armed
            && !self.tops[1].grounded
            && self.tops[1].vel.y < 0.0;
        let mut slam_bonus_drain_b = 0.0; // dealt BY top 0's slam, received by top 1
        let mut slam_bonus_drain_a = 0.0; // dealt BY top 1's slam, received by top 0
        let mut slam_bonus_knock_b = 0.0;
        let mut slam_bonus_knock_a = 0.0;
        if slam_a {
            let fall_height = (self.tops[0].combat.hop_apex_y - self.tops[0].pos.y).max(0.0);
            let slam_drain = (TUNE.hop_slam_drain_base + TUNE.hop_slam_drain_per_m * fall_height)
                .min(TUNE.hop_slam_drain_cap);
            slam_bonus_drain_b = slam_drain * def_drain_b;
            slam_bonus_knock_b = (dv_b_mag.max(1.0))
                * (slam_drain / TUNE.hop_slam_drain_cap)
                * TUNE.hop_slam_knock_bonus_at_cap;
            self.tops[0].combat.hop_slam_armed = false;
            let gain = combat::scaled_meter_gain(TUNE.meter_gain_aerial_slam, &self.tops[0].stats);
            self.tops[0].meter = (self.tops[0].meter + gain).min(100.0);
            self.events.push(BattleEvent::AerialSlam { who: 0 });
        }
        if slam_b {
            let fall_height = (self.tops[1].combat.hop_apex_y - self.tops[1].pos.y).max(0.0);
            let slam_drain = (TUNE.hop_slam_drain_base + TUNE.hop_slam_drain_per_m * fall_height)
                .min(TUNE.hop_slam_drain_cap);
            slam_bonus_drain_a = slam_drain * def_drain_a;
            slam_bonus_knock_a = (dv_a_mag.max(1.0))
                * (slam_drain / TUNE.hop_slam_drain_cap)
                * TUNE.hop_slam_knock_bonus_at_cap;
            self.tops[1].combat.hop_slam_armed = false;
            let gain = combat::scaled_meter_gain(TUNE.meter_gain_aerial_slam, &self.tops[1].stats);
            self.tops[1].meter = (self.tops[1].meter + gain).min(100.0);
            self.events.push(BattleEvent::AerialSlam { who: 1 });
        }
        drain_a_final += slam_bonus_drain_a;
        drain_b_final += slam_bonus_drain_b;

        // Dash shove-armor (game_design.md §2): a dasher in its active
        // window ignores incoming knockback below the threshold.
        if dash_active_a && dv_a_final < TUNE.dash_shove_armor_threshold {
            dv_a_final = 0.0;
        }
        if dash_active_b && dv_b_final < TUNE.dash_shove_armor_threshold {
            dv_b_final = 0.0;
        }

        // `riposte_reverse_knock_a` (top 0's counter) pushes top 1, along
        // the SAME `-n` direction top 1's normal incoming knockback uses;
        // `riposte_reverse_knock_b` (top 1's counter) pushes top 0 along
        // `+n` likewise. `slam_bonus_knock_a`/`_b` follow the same +n/-n
        // convention as the base `dv_a_final`/`dv_b_final` they extend.
        // (Documented simplification: game_design.md §3 says the reversal
        // goes "along attacker velocity" — this reuses the collision
        // normal instead, consistent with how every other knockback in
        // this function is expressed.)
        self.tops[0].vel =
            self.tops[0].vel + n.scaled(dv_a_final + riposte_reverse_knock_b + slam_bonus_knock_a);
        self.tops[1].vel =
            self.tops[1].vel - n.scaled(dv_b_final + riposte_reverse_knock_a + slam_bonus_knock_b);
        self.tops[0].spin = (self.tops[0].spin - drain_a_final).clamp(0.0, TUNE.spin_max);
        self.tops[1].spin = (self.tops[1].spin - drain_b_final).clamp(0.0, TUNE.spin_max);

        // Guillotine Rush's one-time flat bonus impulse (game_design.md
        // §3), consumed alongside the knockback multiplier above.
        if guillotine_bonus_a {
            self.tops[1].vel = self.tops[1].vel - n.scaled(TUNE.special_guillotine_bonus_impulse);
            self.tops[0].combat.special_flag = true;
            self.events.push(BattleEvent::SpecialHit { who: 0 });
        }
        if guillotine_bonus_b {
            self.tops[0].vel = self.tops[0].vel + n.scaled(TUNE.special_guillotine_bonus_impulse);
            self.tops[1].combat.special_flag = true;
            self.events.push(BattleEvent::SpecialHit { who: 1 });
        }

        // Parry: attacker staggered (instant tilt), defender gains meter
        // (game_design.md §2).
        if parry_a {
            self.tilt_bump(1, TUNE.guard_parry_attacker_tilt);
            let gain = combat::scaled_meter_gain(TUNE.meter_gain_parry, &self.tops[0].stats);
            self.tops[0].meter = (self.tops[0].meter + gain).min(100.0);
            self.events.push(BattleEvent::Parry { who: 0 });
        }
        if parry_b {
            self.tilt_bump(0, TUNE.guard_parry_attacker_tilt);
            let gain = combat::scaled_meter_gain(TUNE.meter_gain_parry, &self.tops[1].stats);
            self.tops[1].meter = (self.tops[1].meter + gain).min(100.0);
            self.events.push(BattleEvent::Parry { who: 1 });
        }
        if guard_block_a {
            self.events.push(BattleEvent::GuardBlock { who: 0 });
        }
        if guard_block_b {
            self.events.push(BattleEvent::GuardBlock { who: 1 });
        }

        // Dash-hit meter bonus (game_design.md §2): granted to a dasher
        // whose active window landed a hit.
        if dash_active_a {
            let gain = combat::scaled_meter_gain(TUNE.meter_gain_dash_hit, &self.tops[0].stats);
            self.tops[0].meter = (self.tops[0].meter + gain).min(100.0);
        }
        if dash_active_b {
            let gain = combat::scaled_meter_gain(TUNE.meter_gain_dash_hit, &self.tops[1].stats);
            self.tops[1].meter = (self.tops[1].meter + gain).min(100.0);
        }

        // Meter from drain dealt/taken (game_design.md §2): "drain dealt
        // x0.01 (a 400-hit = +4), drain taken x0.0067".
        let gain_dealt_a = combat::scaled_meter_gain(
            drain_b_final * TUNE.meter_drain_dealt_mult,
            &self.tops[0].stats,
        );
        let gain_dealt_b = combat::scaled_meter_gain(
            drain_a_final * TUNE.meter_drain_dealt_mult,
            &self.tops[1].stats,
        );
        let gain_taken_a = combat::scaled_meter_gain(
            drain_a_final * TUNE.meter_drain_taken_mult,
            &self.tops[0].stats,
        );
        let gain_taken_b = combat::scaled_meter_gain(
            drain_b_final * TUNE.meter_drain_taken_mult,
            &self.tops[1].stats,
        );
        self.tops[0].meter = (self.tops[0].meter + gain_dealt_a + gain_taken_a).min(100.0);
        self.tops[1].meter = (self.tops[1].meter + gain_dealt_b + gain_taken_b).min(100.0);

        let heavy = v_rel > TUNE.heavy_hit_speed
            || drain_a_final.max(drain_b_final) > TUNE.heavy_hit_drain_threshold;
        let airborne_clash = heavy && !grounded_a && !grounded_b;
        if heavy {
            if grounded_a {
                self.tops[0].vel.y += TUNE.airborne_pop;
                self.tops[0].grounded = false;
                self.events.push(BattleEvent::AirborneLaunch { who: 0 });
            }
            if grounded_b {
                self.tops[1].vel.y += TUNE.airborne_pop;
                self.tops[1].grounded = false;
                self.events.push(BattleEvent::AirborneLaunch { who: 1 });
            }
        }

        let contact_pos = (center_a + center_b).scaled(0.5);
        self.events.push(BattleEvent::Hit {
            heavy,
            pos: contact_pos,
            speed: v_rel,
        });

        let mut hitstop_candidate = if airborne_clash {
            TUNE.hitstop_airborne_clash
        } else if heavy {
            TUNE.hitstop_heavy
        } else {
            TUNE.hitstop_light
        };
        if parry_a || parry_b || guard_block_a || guard_block_b {
            hitstop_candidate = hitstop_candidate.max(TUNE.hitstop_guard);
        }
        self.hitstop = self.hitstop.max(hitstop_candidate as u8);
    }

    /// Bump `tops[who]`'s tilt magnitude by `amount` along its current
    /// direction (or a fixed fallback direction if it's currently at rest) —
    /// Guard's parry-stagger effect on the attacker (game_design.md §2:
    /// "attacker gets instant tilt +0.12 rad").
    fn tilt_bump(&mut self, who: usize, amount: f32) {
        let top = &mut self.tops[who];
        let dir = if top.tilt.length_sq() > 1e-6 {
            top.tilt.normalize_or_zero()
        } else {
            Vec2::new(1.0, 0.0)
        };
        top.tilt = top.tilt + dir.scaled(amount);
    }

    /// Passive meter trickle + Armed check (game_design.md §1/§2, Phase 7).
    /// Anchor grants ZERO meter, including this passive trickle
    /// (game_design.md §2: Anchor "builds ZERO meter" — read as an absolute
    /// suppression while held, not just "no bonus on top of the trickle").
    fn step_meter_passive(&mut self, i: usize) {
        let top = &mut self.tops[i];
        if top.combat.anchor_hold == 0 {
            let gain =
                combat::scaled_meter_gain(TUNE.meter_gain_passive_per_s * SIM_DT, &top.stats);
            top.meter = (top.meter + gain).min(100.0);
        }
        if top.meter >= TUNE.meter_armed_threshold {
            top.meter = top.meter.min(100.0);
            top.combat.special_armed = true;
        }
    }

    /// Evaluate BOTH out conditions for BOTH tops from final end-of-step
    /// state and commit the outcome once (atomic — M2 verifier findings
    /// #3/#4). A top hitting both conditions in the same step counts as
    /// stamina-out (its spin reached zero; documented precedence). Events
    /// match the recorded outcome exactly.
    fn resolve_outs(&mut self) {
        let mut stamina = [false, false];
        let mut ring = [false, false];
        for i in 0..2 {
            let top = &self.tops[i];
            stamina[i] = top.spin <= 0.0
                || (top.tilt.length() > TUNE.topple_tilt && top.spin < TUNE.topple_spin);
            let r = fixmath::sqrt(top.pos.x * top.pos.x + top.pos.z * top.pos.z);
            ring[i] = r > arena::RING_OUT_RADIUS;
        }
        for i in 0..2 {
            if stamina[i] {
                self.events.push(BattleEvent::Topple { who: i as u8 });
                self.hitstop = self.hitstop.max(TUNE.hitstop_topple as u8);
            } else if ring[i] {
                self.events.push(BattleEvent::RingOut { who: i as u8 });
                self.hitstop = self.hitstop.max(TUNE.hitstop_ring_out as u8);
            }
        }
        let out = [stamina[0] || ring[0], stamina[1] || ring[1]];
        self.outcome = match out {
            [true, true] => Some(Outcome::Simultaneous),
            [true, false] => Some(if stamina[0] {
                Outcome::StaminaOut { loser: 0 }
            } else {
                Outcome::RingOut { loser: 0 }
            }),
            [false, true] => Some(if stamina[1] {
                Outcome::StaminaOut { loser: 1 }
            } else {
                Outcome::RingOut { loser: 1 }
            }),
            [false, false] => None,
        };

        // Crash-Out (SPEC §6.5): a kill while the WINNER's own Crash-Out
        // window is still open. `combat::round_points` is the authoritative
        // scoring API this feeds into; this just emits the matching event +
        // its dedicated hit-stop (game_design.md §5: 10 steps).
        if let Some(winner) = match self.outcome {
            Some(Outcome::RingOut { loser }) | Some(Outcome::StaminaOut { loser }) => {
                Some(1 - loser)
            }
            _ => None,
        } {
            if self.tops[winner as usize].combat.crash_window > 0 {
                self.events.push(BattleEvent::CrashOut { winner });
                self.hitstop = self.hitstop.max(TUNE.hitstop_crash_out as u8);
            }
        }
    }

    /// Fixed-order serialization of ALL sim state (SPEC §5 determinism
    /// fingerprint): every `f32` via `to_bits()`, ints/bools as `u32`, the
    /// RNG's stream position, and the outcome discriminant.
    pub fn state_hash(&self) -> u64 {
        // Exhaustive destructures (no `..`) so that adding a field to World,
        // Top, or Stats without deciding whether to hash it is a COMPILE
        // error, not a silent determinism blind spot (M2 verifier finding
        // #6). `events` is deliberately unhashed: it is a per-step output
        // channel for render/audio, not sim state.
        let World {
            tops,
            rng,
            step,
            hitstop,
            outcome,
            events: _,
        } = self;
        let mut words: Vec<u32> = Vec::with_capacity(64);
        for top in tops {
            let Top {
                pos,
                vel,
                spin,
                spin_dir,
                tilt,
                tilt_phase,
                spin_angle,
                radius,
                height,
                stats,
                grounded,
                dash_cd,
                dash_active,
                meter,
                airdash_used,
                combat,
            } = *top;
            let Stats {
                atk,
                def,
                sta,
                wgt,
                spd,
                mtr,
            } = stats;
            words.push(pos.x.to_bits());
            words.push(pos.y.to_bits());
            words.push(pos.z.to_bits());
            words.push(vel.x.to_bits());
            words.push(vel.y.to_bits());
            words.push(vel.z.to_bits());
            words.push(spin.to_bits());
            words.push(spin_dir as i32 as u32);
            words.push(tilt.x.to_bits());
            words.push(tilt.y.to_bits());
            words.push(tilt_phase.to_bits());
            words.push(spin_angle.to_bits());
            words.push(radius.to_bits());
            words.push(height.to_bits());
            words.push(atk as u32);
            words.push(def as u32);
            words.push(sta as u32);
            words.push(wgt as u32);
            words.push(spd as u32);
            words.push(mtr as u32);
            words.push(grounded as u32);
            words.push(dash_cd as u32);
            words.push(dash_active as u32);
            words.push(meter.to_bits());
            words.push(airdash_used as u32);

            // `combat: CombatState` (M4-A) — exhaustive destructure for the
            // same reason as `Top`/`Stats` above.
            let CombatState {
                dash_startup,
                dash_recovery,
                dash_dir,
                guard_hold,
                guard_drop_recovery,
                hop_air_steps,
                hop_land_lag,
                hop_slam_armed,
                hop_apex_y,
                carve_hold,
                carve_release_decay,
                carve_tilt_bonus,
                anchor_hold,
                anchor_release_lag,
                last_move_dir,
                special_id,
                special_armed,
                special_active,
                special_flag,
                special_was_pressed,
                crash_window,
            } = combat;
            words.push(dash_startup as u32);
            words.push(dash_recovery as u32);
            words.push(dash_dir.x.to_bits());
            words.push(dash_dir.y.to_bits());
            words.push(guard_hold as u32);
            words.push(guard_drop_recovery as u32);
            words.push(hop_air_steps as u32);
            words.push(hop_land_lag as u32);
            words.push(hop_slam_armed as u32);
            words.push(hop_apex_y.to_bits());
            words.push(carve_hold as u32);
            words.push(carve_release_decay as u32);
            words.push(carve_tilt_bonus.to_bits());
            words.push(anchor_hold as u32);
            words.push(anchor_release_lag as u32);
            words.push(last_move_dir.x.to_bits());
            words.push(last_move_dir.y.to_bits());
            words.push(special_id as u32);
            words.push(special_armed as u32);
            words.push(special_active as u32);
            words.push(special_flag as u32);
            words.push(special_was_pressed as u32);
            words.push(crash_window as u32);
        }
        words.push(*step as u32);
        words.push((*step >> 32) as u32);
        words.push(*hitstop as u32);
        let (disc, loser): (u32, u32) = match outcome {
            None => (0, 0),
            Some(Outcome::RingOut { loser }) => (1, *loser as u32),
            Some(Outcome::StaminaOut { loser }) => (2, *loser as u32),
            Some(Outcome::Simultaneous) => (3, 0),
        };
        words.push(disc);
        words.push(loser);
        let rng_state = rng.state();
        words.push(rng_state as u32);
        words.push((rng_state >> 32) as u32);
        crate::hash::hash_u32s(&words)
    }
}

/// Final per-top clamp pass (HARD RULES: velocity magnitude, position,
/// tilt magnitude, spin range, denormal snap — every step).
fn finalize_clamps(top: &mut Top) {
    let horiz = Vec2::new(top.vel.x, top.vel.z);
    // Carve: ramped top-speed bonus (game_design.md §2), mirroring the same
    // clamp bonus `step_integrate_and_terrain` applies — a collision impulse
    // later in the same step must not un-clamp a Carve-boosted top back down
    // to the un-boosted speed cap.
    let base_max_h = if top.grounded {
        TUNE.max_ground_speed
    } else {
        TUNE.max_air_speed
    };
    let max_h = if top.combat.carve_hold > 0 {
        let ramp = carve_ramp_frac(top);
        base_max_h * (1.0 + (TUNE.carve_top_speed_mult - 1.0) * ramp)
    } else {
        base_max_h
    };
    let horiz_len = horiz.length();
    if horiz_len > max_h && horiz_len > 0.0 {
        let scale = max_h / horiz_len;
        top.vel.x *= scale;
        top.vel.z *= scale;
    }
    top.vel.y = top.vel.y.clamp(-TUNE.max_fall_speed, TUNE.max_fall_speed);
    top.vel = snap_denormal_v3(top.vel);

    top.pos.x = top.pos.x.clamp(-POS_XZ_LIMIT, POS_XZ_LIMIT);
    top.pos.z = top.pos.z.clamp(-POS_XZ_LIMIT, POS_XZ_LIMIT);
    top.pos.y = top.pos.y.min(POS_Y_LIMIT);

    top.spin = snap_denormal(top.spin.clamp(0.0, TUNE.spin_max));

    let tilt_len = top.tilt.length();
    if tilt_len > TILT_MAGNITUDE_LIMIT {
        top.tilt = top.tilt.scaled(TILT_MAGNITUDE_LIMIT / tilt_len);
    }
    top.tilt = snap_denormal_v2(top.tilt);

    // Hygiene clamp on the Carve tilt-bonus accumulator itself (belt-and-
    // suspenders — HARD RULES: nothing should be able to grow without
    // bound, even though `top.tilt`'s own magnitude is already clamped
    // above regardless of this accumulator's raw size).
    const CARVE_TILT_BONUS_LIMIT: f32 = 8.0;
    top.combat.carve_tilt_bonus = snap_denormal(
        top.combat
            .carve_tilt_bonus
            .clamp(0.0, CARVE_TILT_BONUS_LIMIT),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keystone_stats() -> Stats {
        Stats {
            atk: 52,
            def: 54,
            sta: 52,
            wgt: 50,
            spd: 50,
            mtr: 44,
        }
    }

    #[test]
    fn stat_mapping_matches_game_design_formulas() {
        let s = Stats {
            atk: 50,
            def: 50,
            sta: 50,
            wgt: 50,
            spd: 50,
            mtr: 50,
        };
        assert!((s.mass() - 50.0).abs() < 1e-5);
        assert!((s.knock_mult() - 1.0).abs() < 1e-5);
        assert!((s.drain_taken_mult() - 0.7).abs() < 1e-5);
        assert!((s.knock_taken_mult() - 0.8).abs() < 1e-5);
        assert!((s.decay_per_s() - 24.0).abs() < 1e-5);
        assert!((s.ctrl_accel() - 11.0).abs() < 1e-5);
        assert_eq!(s.dash_cooldown_steps(), 52);
        assert!((s.meter_gain_mult() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn launch_spawns_on_launch_circle_with_zero_lateral_drift() {
        let params = LaunchParams {
            heading: 0.4,
            depth: 0.6,
            power: 0.5,
            quality: 1.0,
            spin_dir: 1,
            stats: keystone_stats(),
            special_id: SpecialId::Overclock,
        };
        let top = spawn_launch_top(&params);
        let r = fixmath::sqrt(top.pos.x * top.pos.x + top.pos.z * top.pos.z);
        assert!((r - TUNE.launch_radius).abs() < 1e-2, "r={r}");
    }

    #[test]
    fn hitstop_skip_leaves_state_bit_identical() {
        let stats = keystone_stats();
        let mut world = World {
            tops: [
                Top {
                    pos: Vec3::new(-0.2, 0.0, 0.0),
                    vel: Vec3::new(3.0, 0.0, 0.0),
                    spin: TUNE.spin_max,
                    spin_dir: 1,
                    tilt: Vec2::default(),
                    tilt_phase: 0.0,
                    spin_angle: 0.0,
                    radius: 0.95,
                    height: 1.0,
                    stats,
                    grounded: false,
                    dash_cd: 0,
                    dash_active: 0,
                    meter: 0.0,
                    airdash_used: false,
                    combat: CombatState::default(),
                },
                Top {
                    pos: Vec3::new(0.2, 0.0, 0.0),
                    vel: Vec3::new(-3.0, 0.0, 0.0),
                    spin: TUNE.spin_max,
                    spin_dir: 1,
                    tilt: Vec2::default(),
                    tilt_phase: 0.0,
                    spin_angle: 0.0,
                    radius: 0.95,
                    height: 1.0,
                    stats,
                    grounded: false,
                    dash_cd: 0,
                    dash_active: 0,
                    meter: 0.0,
                    airdash_used: false,
                    combat: CombatState::default(),
                },
            ],
            rng: Rng::new(1),
            step: 0,
            hitstop: 0,
            outcome: None,
            events: Vec::new(),
        };
        world.step([InputState::default(), InputState::default()]);
        assert!(world.hitstop > 0, "expected a collision to trigger hitstop");
        let stopped = world.hitstop;
        for _ in 0..stopped {
            let before = world.tops;
            world.step([InputState::default(), InputState::default()]);
            assert_eq!(before, world.tops, "state changed during hitstop skip");
        }
        assert_eq!(world.hitstop, 0);
    }

    #[test]
    fn spin_angle_accumulates_and_wraps_into_zero_tau_range() {
        let params = LaunchParams {
            heading: 0.0,
            depth: 0.7,
            power: 0.5,
            quality: 1.0,
            spin_dir: 1,
            stats: keystone_stats(),
            special_id: SpecialId::Overclock,
        };
        let mut world = World::launch(1, [params, params]);
        assert_eq!(world.tops[0].spin_angle, 0.0, "launch spawns spin_angle=0");

        let mut prev_angle = world.tops[0].spin_angle;
        let mut saw_wrap = false;
        for _ in 0..2000 {
            world.step([InputState::default(), InputState::default()]);
            for top in &world.tops {
                assert!(
                    (0.0..TAU).contains(&top.spin_angle),
                    "spin_angle escaped [0, TAU): {}",
                    top.spin_angle
                );
            }
            let a = world.tops[0].spin_angle;
            if a < prev_angle {
                saw_wrap = true;
            }
            prev_angle = a;
        }
        assert!(
            saw_wrap,
            "expected spin_angle to wrap at least once over 2000 steps"
        );
    }

    #[test]
    fn spin_angle_decreases_for_negative_spin_dir() {
        let params_pos = LaunchParams {
            heading: 0.0,
            depth: 0.7,
            power: 0.5,
            quality: 1.0,
            spin_dir: -1,
            stats: keystone_stats(),
            special_id: SpecialId::Overclock,
        };
        let mut world = World::launch(1, [params_pos, params_pos]);
        world.step([InputState::default(), InputState::default()]);
        // spin_dir=-1: the raw angle moves negative, then wraps up toward
        // TAU rather than staying near 0 (verifies the underflow branch).
        assert!(
            world.tops[0].spin_angle > TAU * 0.5,
            "spin_angle={}",
            world.tops[0].spin_angle
        );
    }

    #[test]
    fn pose_lerp_endpoints_match_prev_and_curr() {
        let mut prev = spawn_launch_top(&LaunchParams {
            heading: 0.0,
            depth: 0.7,
            power: 0.5,
            quality: 1.0,
            spin_dir: 1,
            stats: keystone_stats(),
            special_id: SpecialId::Overclock,
        });
        prev.pos = Vec3::new(0.0, 0.0, 0.0);
        prev.spin_angle = 1.0;
        let mut curr = prev;
        curr.pos = Vec3::new(1.0, 2.0, 3.0);
        curr.tilt = Vec2::new(0.2, -0.1);
        curr.spin_angle = 2.0;

        let at0 = pose_lerp(&prev, &curr, 0.0);
        assert_eq!(at0.pos, prev.pos);
        assert_eq!(at0.tilt, prev.tilt);
        assert!((at0.spin_angle - 1.0).abs() < 1e-6);

        let at1 = pose_lerp(&prev, &curr, 1.0);
        assert_eq!(at1.pos, curr.pos);
        assert_eq!(at1.tilt, curr.tilt);
        assert!((at1.spin_angle - 2.0).abs() < 1e-6);

        let mid = pose_lerp(&prev, &curr, 0.5);
        assert!((mid.pos.x - 0.5).abs() < 1e-6);
        assert!((mid.spin_angle - 1.5).abs() < 1e-6);
        assert_eq!(mid.radius, curr.radius);
        assert_eq!(mid.height, curr.height);
    }

    #[test]
    fn pose_lerp_spin_angle_takes_the_shortest_arc_across_the_wrap() {
        let mut prev = spawn_launch_top(&LaunchParams {
            heading: 0.0,
            depth: 0.7,
            power: 0.5,
            quality: 1.0,
            spin_dir: 1,
            stats: keystone_stats(),
            special_id: SpecialId::Overclock,
        });
        // prev near TAU, curr just past the wrap (near 0): the true motion is
        // a tiny forward step, NOT a near-full revolution backward.
        prev.spin_angle = TAU - 0.1;
        let mut curr = prev;
        curr.spin_angle = 0.05;

        let mid = pose_lerp(&prev, &curr, 0.5);
        // Naive lerp (no wrap-awareness) would give ~ (TAU-0.1 + 0.05)/2 ~=
        // TAU/2, near pi — the wrong direction entirely. The correct
        // shortest-arc midpoint is near TAU (equivalently just under 0).
        let dist_to_zero = mid.spin_angle.min(TAU - mid.spin_angle);
        assert!(
            dist_to_zero < 0.1,
            "expected midpoint near the 0/TAU wrap, got {}",
            mid.spin_angle
        );
    }

    #[test]
    fn pose_lerp_clamps_alpha_outside_zero_one() {
        let prev = spawn_launch_top(&LaunchParams {
            heading: 0.0,
            depth: 0.7,
            power: 0.5,
            quality: 1.0,
            spin_dir: 1,
            stats: keystone_stats(),
            special_id: SpecialId::Overclock,
        });
        let mut curr = prev;
        curr.pos = Vec3::new(5.0, 5.0, 5.0);
        let below = pose_lerp(&prev, &curr, -1.0);
        let above = pose_lerp(&prev, &curr, 2.0);
        assert_eq!(below.pos, prev.pos);
        assert_eq!(above.pos, curr.pos);
    }
}
