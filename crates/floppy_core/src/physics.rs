//! Deterministic 3D top sim (SPEC §6.3): the fixed-order `World::step` and
//! everything it touches. Combat verbs (guard/hop/carve/anchor) and the
//! special/meter system are M4 scope — this milestone reads `InputState`
//! only for `dir_x`/`dir_y`/`dash`.
//!
//! Every quantity here is plain `f32`/integer state, stepped in a fixed
//! order with tops always visited index 0 then 1 (SPEC §5). All
//! trig/sqrt goes through [`crate::fixmath`].

use crate::arena;
use crate::fixmath;
use crate::input::InputState;
use crate::rng::Rng;
use crate::vec::{Vec2, Vec3};

/// Fixed simulation timestep (SPEC §5: 120 Hz).
pub const SIM_DT: f32 = crate::clock::SIM_DT;

/// Vertical offset from the tracked "tip/contact" point ([`Top::pos`]) up to
/// the sphere used for top-vs-top collision. Not exposed as a `TuneParams`
/// field because it's a body-shape constant (SPEC §6.1: `radius`/`height`),
/// not a tunable feel knob.
const BODY_CENTER_OFFSET: f32 = 0.35;

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

    pub launch_radius: f32,
    pub launch_drop_height: f32,
    pub launch_speed_base: f32,
    pub launch_speed_depth: f32,
    pub launch_spin_base: f32,
    pub launch_spin_power: f32,
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

    launch_radius: 6.5,
    launch_drop_height: 1.2,
    launch_speed_base: 3.0,
    launch_speed_depth: 5.0,
    launch_spin_base: 7_000.0,
    launch_spin_power: 2_000.0,
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
    pub radius: f32,
    pub height: f32,
    pub stats: Stats,
    pub grounded: bool,
    pub dash_cd: u16,
    pub dash_active: u16,
    pub meter: f32,
    pub airdash_used: bool,
}

impl Top {
    fn spin_frac(&self) -> f32 {
        (self.spin / TUNE.spin_max).clamp(0.0, 1.0)
    }
}

/// Discrete, headlessly-consumable gameplay moments emitted by [`World::step`]
/// (SPEC §6.3 step 6/7, render/audio hook points per game_design.md §5).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BattleEvent {
    Hit { heavy: bool, pos: Vec3, speed: f32 },
    Dash { who: u8 },
    AirborneLaunch { who: u8 },
    Landed { who: u8, impact: f32 },
    RingOut { who: u8 },
    Topple { who: u8 },
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
}

/// Full match/round state (SPEC §6.1/6.5, trimmed to M2 scope).
#[derive(Clone, Debug)]
pub struct World {
    pub tops: [Top; 2],
    pub rng: Rng,
    pub step: u64,
    pub hitstop: u8,
    pub outcome: Option<Outcome>,
    pub events: Vec<BattleEvent>,
}

/// Build one top's initial state for the Launch phase (game_design.md §4):
/// spawns on the launch circle, aimed 65% toward center / 35% tangential.
fn spawn_launch_top(p: &LaunchParams) -> Top {
    let x = fixmath::cos(p.heading) * TUNE.launch_radius;
    let z = fixmath::sin(p.heading) * TUNE.launch_radius;
    let y = arena::height(x, z) + TUNE.launch_drop_height;

    let to_center = Vec2::new(-x, -z).normalize_or_zero();
    let tangent = Vec2::new(-to_center.y, to_center.x);
    let aim = (to_center.scaled(0.65) + tangent.scaled(0.35)).normalize_or_zero();
    let speed = TUNE.launch_speed_base + TUNE.launch_speed_depth * p.depth;
    let vel = Vec3::new(aim.x * speed, 0.0, aim.y * speed);

    let spin = ((TUNE.launch_spin_base + TUNE.launch_spin_power * p.power) * p.quality)
        .clamp(0.0, TUNE.spin_max);

    Top {
        pos: Vec3::new(x, y, z),
        vel,
        spin,
        spin_dir: p.spin_dir,
        tilt: Vec2::default(),
        tilt_phase: 0.0,
        radius: 0.45,
        height: 0.5,
        stats: p.stats,
        grounded: false,
        dash_cd: 0,
        dash_active: 0,
        meter: 0.0,
        airdash_used: false,
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

        // Phase 1: dash state machine, top 0 then top 1.
        for (i, input) in inputs.into_iter().enumerate() {
            self.step_dash(i, input);
        }

        // Phase 2: control accel / gravity / slope / friction.
        for (i, input) in inputs.into_iter().enumerate() {
            self.step_forces(i, input);
        }

        // Phase 3+4: integrate, clamp, terrain contact.
        for i in 0..2 {
            self.step_integrate_and_terrain(i);
        }

        // Phase 5: spin decay + precession + topple/stamina-out check.
        let mut stamina_out = [false, false];
        for (i, out) in stamina_out.iter_mut().enumerate() {
            *out = self.step_spin_and_precession(i);
        }
        if self.outcome.is_none() {
            self.outcome = match stamina_out {
                [true, true] => Some(Outcome::Simultaneous),
                [true, false] => Some(Outcome::StaminaOut { loser: 0 }),
                [false, true] => Some(Outcome::StaminaOut { loser: 1 }),
                [false, false] => None,
            };
        }

        // Phase 6: single-pair collision.
        self.step_collision();

        // Phase 7: ring-out check.
        self.step_ring_out();

        // Final clamp pass (belt-and-suspenders — HARD RULES: clamp
        // aggressively every step) + phase 8 step counter.
        for top in &mut self.tops {
            finalize_clamps(top);
        }
        self.step += 1;
    }

    fn step_dash(&mut self, i: usize, input: InputState) {
        let top = &mut self.tops[i];
        if top.dash_cd > 0 {
            top.dash_cd -= 1;
        }
        if top.dash_active > 0 {
            top.dash_active -= 1;
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

        top.vel.x += dir.x * TUNE.dash_impulse;
        top.vel.z += dir.y * TUNE.dash_impulse;
        let horiz_speed = Vec2::new(top.vel.x, top.vel.z).length();
        if horiz_speed > TUNE.dash_speed_clamp {
            let scale = TUNE.dash_speed_clamp / horiz_speed;
            top.vel.x *= scale;
            top.vel.z *= scale;
        }

        top.dash_active = TUNE.dash_active_steps as u16;
        top.dash_cd = top.stats.dash_cooldown_steps();
        self.events.push(BattleEvent::Dash { who: i as u8 });
    }

    fn step_forces(&mut self, i: usize, input: InputState) {
        let top = &mut self.tops[i];
        let dir = input_dir_xz(input.dir_x, input.dir_y);
        let air_factor = if top.grounded {
            1.0
        } else {
            TUNE.air_ctrl_factor
        };
        let accel = top.stats.ctrl_accel() * air_factor;
        top.vel.x += dir.x * accel * SIM_DT;
        top.vel.z += dir.y * accel * SIM_DT;

        top.vel.y -= TUNE.gravity * SIM_DT;

        if top.grounded {
            let (dhdx, dhdz) = arena::gradient(top.pos.x, top.pos.z);
            top.vel.x += -dhdx * TUNE.slope_accel * SIM_DT;
            top.vel.z += -dhdz * TUNE.slope_accel * SIM_DT;

            let friction = (1.0 - TUNE.ground_friction * SIM_DT).max(0.0);
            top.vel.x *= friction;
            top.vel.z *= friction;
        }
    }

    fn step_integrate_and_terrain(&mut self, i: usize) {
        let top = &mut self.tops[i];

        // Velocity clamps (ground/air/fall speed).
        let max_h = if top.grounded {
            TUNE.max_ground_speed
        } else {
            TUNE.max_air_speed
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
                    // `v -= n*(v.n)`), then add the small bounce.
                    top.vel = top.vel - n.scaled(v_n);
                    top.vel.y = -impact * 0.25;
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
                    let speed = top.vel.length();
                    let tangential = (top.vel - n.scaled(v_n)).normalize_or_zero();
                    top.vel = tangential.scaled(speed);
                }
            }
            top.grounded = true;
            top.airdash_used = false;
        } else {
            top.grounded = false;
        }
    }

    /// Returns `true` if this top hit a stamina-out condition this step.
    fn step_spin_and_precession(&mut self, i: usize) -> bool {
        let top = &mut self.tops[i];

        top.spin -= top.stats.decay_per_s() * SIM_DT;
        if top.dash_active > 0 {
            top.spin -= 2.0;
        }
        top.spin = top.spin.clamp(0.0, TUNE.spin_max);

        let spin_frac = top.spin_frac();
        let one_minus = 1.0 - spin_frac;
        let wobble_freq = TUNE.wobble_freq_base + TUNE.wobble_freq_growth * one_minus;
        top.tilt_phase += 2.0 * std::f32::consts::PI * wobble_freq * SIM_DT;

        let stability = (top.stats.wgt as f32 / 100.0) * (top.stats.sta as f32 / 100.0);
        let target_mag = (TUNE.tilt_base + TUNE.tilt_growth * one_minus * one_minus)
            * (1.2 - TUNE.tilt_stability_mul * stability);
        let target_mag = target_mag.max(0.0);

        let current_mag = top.tilt.length();
        let max_step = TUNE.tilt_rate * SIM_DT;
        let diff = target_mag - current_mag;
        let new_mag = if diff > max_step {
            current_mag + max_step
        } else if diff < -max_step {
            current_mag - max_step
        } else {
            target_mag
        }
        .max(0.0);

        top.tilt =
            Vec2::new(fixmath::cos(top.tilt_phase), fixmath::sin(top.tilt_phase)).scaled(new_mag);

        let toppled =
            top.spin <= 0.0 || (new_mag > TUNE.topple_tilt && top.spin < TUNE.topple_spin);
        if toppled {
            self.events.push(BattleEvent::Topple { who: i as u8 });
        }
        toppled
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

        // De-penetration by inverse-mass split (pre-collision masses).
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

        if v_rel <= 0.0 {
            return;
        }

        let stats_a = self.tops[0].stats;
        let stats_b = self.tops[1].stats;
        let spin_frac_a = self.tops[0].spin_frac();
        let spin_frac_b = self.tops[1].spin_frac();
        let same_dir = self.tops[0].spin_dir == self.tops[1].spin_dir;
        let dash_active_a = self.tops[0].dash_active > 0;
        let dash_active_b = self.tops[1].dash_active > 0;
        let grounded_a = self.tops[0].grounded;
        let grounded_b = self.tops[1].grounded;

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
        let clash_mul = if same_dir { TUNE.clash_knock_mul } else { 1.0 };
        let grind_mul = if !same_dir { TUNE.grind_drain_mul } else { 1.0 };

        let j = (1.0 + TUNE.restitution) * v_rel / total_inv;

        let dv_a_mag = (j / mass_a)
            * (1.0 + TUNE.knock_scale * stats_b.knock_mult() * spin_frac_b * dash_knock_mul_b)
            * stats_a.knock_taken_mult()
            * clash_mul;
        let dv_b_mag = (j / mass_b)
            * (1.0 + TUNE.knock_scale * stats_a.knock_mult() * spin_frac_a * dash_knock_mul_a)
            * stats_b.knock_taken_mult()
            * clash_mul;

        self.tops[0].vel = self.tops[0].vel + n.scaled(dv_a_mag);
        self.tops[1].vel = self.tops[1].vel - n.scaled(dv_b_mag);

        let drain_a_base = (TUNE.drain_scale * (v_rel / 6.0) * stats_b.knock_mult() * spin_frac_b)
            .max(TUNE.drain_min);
        let drain_b_base = (TUNE.drain_scale * (v_rel / 6.0) * stats_a.knock_mult() * spin_frac_a)
            .max(TUNE.drain_min);
        let drain_a = drain_a_base * stats_a.drain_taken_mult() * grind_mul * dash_drain_mul_b;
        let drain_b = drain_b_base * stats_b.drain_taken_mult() * grind_mul * dash_drain_mul_a;
        self.tops[0].spin = (self.tops[0].spin - drain_a).clamp(0.0, TUNE.spin_max);
        self.tops[1].spin = (self.tops[1].spin - drain_b).clamp(0.0, TUNE.spin_max);

        let heavy = v_rel > TUNE.heavy_hit_speed;
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
        self.hitstop = if heavy {
            TUNE.hitstop_heavy as u8
        } else {
            TUNE.hitstop_light as u8
        };
    }

    fn step_ring_out(&mut self) {
        let mut out = [false, false];
        for (i, out_i) in out.iter_mut().enumerate() {
            let p = self.tops[i].pos;
            let r = fixmath::sqrt(p.x * p.x + p.z * p.z);
            *out_i = r > arena::RING_OUT_RADIUS;
        }
        if !out[0] && !out[1] {
            return;
        }
        for (i, &out_i) in out.iter().enumerate() {
            if out_i {
                self.events.push(BattleEvent::RingOut { who: i as u8 });
            }
        }
        if self.outcome.is_none() {
            self.outcome = match out {
                [true, true] => Some(Outcome::Simultaneous),
                [true, false] => Some(Outcome::RingOut { loser: 0 }),
                [false, true] => Some(Outcome::RingOut { loser: 1 }),
                [false, false] => unreachable!(),
            };
        }
    }

    /// Fixed-order serialization of ALL sim state (SPEC §5 determinism
    /// fingerprint): every `f32` via `to_bits()`, ints/bools as `u32`, the
    /// RNG's stream position, and the outcome discriminant.
    pub fn state_hash(&self) -> u64 {
        let mut words: Vec<u32> = Vec::with_capacity(64);
        for top in &self.tops {
            words.push(top.pos.x.to_bits());
            words.push(top.pos.y.to_bits());
            words.push(top.pos.z.to_bits());
            words.push(top.vel.x.to_bits());
            words.push(top.vel.y.to_bits());
            words.push(top.vel.z.to_bits());
            words.push(top.spin.to_bits());
            words.push(top.spin_dir as i32 as u32);
            words.push(top.tilt.x.to_bits());
            words.push(top.tilt.y.to_bits());
            words.push(top.tilt_phase.to_bits());
            words.push(top.radius.to_bits());
            words.push(top.height.to_bits());
            words.push(top.stats.atk as u32);
            words.push(top.stats.def as u32);
            words.push(top.stats.sta as u32);
            words.push(top.stats.wgt as u32);
            words.push(top.stats.spd as u32);
            words.push(top.stats.mtr as u32);
            words.push(top.grounded as u32);
            words.push(top.dash_cd as u32);
            words.push(top.dash_active as u32);
            words.push(top.meter.to_bits());
            words.push(top.airdash_used as u32);
        }
        words.push(self.step as u32);
        words.push((self.step >> 32) as u32);
        words.push(self.hitstop as u32);
        let (disc, loser): (u32, u32) = match self.outcome {
            None => (0, 0),
            Some(Outcome::RingOut { loser }) => (1, loser as u32),
            Some(Outcome::StaminaOut { loser }) => (2, loser as u32),
            Some(Outcome::Simultaneous) => (3, 0),
        };
        words.push(disc);
        words.push(loser);
        let rng_state = self.rng.state();
        words.push(rng_state as u32);
        words.push((rng_state >> 32) as u32);
        crate::hash::hash_u32s(&words)
    }
}

/// Final per-top clamp pass (HARD RULES: velocity magnitude, position,
/// tilt magnitude, spin range, denormal snap — every step).
fn finalize_clamps(top: &mut Top) {
    let horiz = Vec2::new(top.vel.x, top.vel.z);
    let max_h = if top.grounded {
        TUNE.max_ground_speed
    } else {
        TUNE.max_air_speed
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
                    radius: 0.45,
                    height: 0.5,
                    stats,
                    grounded: false,
                    dash_cd: 0,
                    dash_active: 0,
                    meter: 0.0,
                    airdash_used: false,
                },
                Top {
                    pos: Vec3::new(0.2, 0.0, 0.0),
                    vel: Vec3::new(-3.0, 0.0, 0.0),
                    spin: TUNE.spin_max,
                    spin_dir: 1,
                    tilt: Vec2::default(),
                    tilt_phase: 0.0,
                    radius: 0.45,
                    height: 0.5,
                    stats,
                    grounded: false,
                    dash_cd: 0,
                    dash_active: 0,
                    meter: 0.0,
                    airdash_used: false,
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
}
