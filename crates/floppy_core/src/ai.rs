//! M5: utility AI (SPEC §11). A difficulty-scaled controller that observes
//! public [`World`] state and produces an [`InputState`] — nothing else. The
//! sim never knows or cares whether an `InputState` came from a keyboard or
//! from [`decide`] (SPEC §6.4); this module is a pure "hand on the keyboard"
//! sitting entirely OUTSIDE `World`.
//!
//! ## Design summary
//!
//! [`decide`] evaluates a small, FIXED-order candidate list each call (no
//! search trees, no lookahead simulation):
//!
//! 1. Capture the current step's observation and push it into a reaction-delay
//!    ring buffer ([`AiState::push_observation`]); read back the observation
//!    from `reaction_delay_steps` ago ([`AiState::delayed`]) — this is the
//!    only "world" the rest of `decide` ever looks at, modeling human-like
//!    reaction lag without cloning `World`.
//! 2. Movement: chase the opponent's lead-extrapolated position, blended with
//!    a center-ward "safety" direction whose weight grows near the rim, at
//!    low own spin, and when the opponent is armed/firing (tempered by
//!    `aggression`). `aim_error` then rotates the chosen direction by a
//!    bounded random angle before it's quantized to digital `dir_x`/`dir_y`.
//! 3. Escape read: if the opponent's special is ACTIVE (fired), this
//!    overrides everything else — see [`correct_escape_verb`]. The read
//!    (correct vs. a plausible-but-wrong alternative) is rolled ONCE per
//!    threat instance and held for its duration (a committed plan), not
//!    re-rolled every step, so low tiers don't flicker between verbs.
//! 4. Otherwise: a mutually-exclusive stance pick among Guard / Hop / Anchor /
//!    Carve, each scored independently and gated by `verb_skill`; ties break
//!    in the fixed order Guard, Hop, Anchor, Carve.
//! 5. Dash: an independent edge decision (closing attack), gated by
//!    `dash_skill` and cooldown-aware.
//! 6. Special fire: an independent edge decision when Armed, gated by
//!    `special_skill` and opponent range.
//!
//! A hard safety valve (`HARD_SAFE_R`) overrides everything with a pure
//! center-ward retreat if the AI's own top somehow gets dangerously close to
//! the rim, so no tier can accidentally walk itself out (needed for the
//! `ace_never_self_rings_out_alone` balance gate).

use crate::arena;
use crate::combat::SpecialId;
use crate::fixmath;
use crate::input::InputState;
use crate::minigame::Difficulty;
use crate::physics::{World, TUNE};
use crate::rng::Rng;
use crate::vec::{Vec2, Vec3};

// ---------------------------------------------------------------------------
// Tunable geometry/behavior constants (AI-only "feel" knobs — NOT physics
// tuning, so deliberately kept out of `physics::TuneParams`, per SPEC §11's
// "separate `AiParams` const table, NOT in `TuneParams`").
// ---------------------------------------------------------------------------

/// Ring-buffer length for the reaction-delay observation history. Must
/// exceed the largest `reaction_delay_steps` across [`AI_TIERS`] (Easy = 30);
/// kept as plain fixed-size scalars per observation, NOT cloned `World`s.
const OBS_BUFFER_LEN: usize = 40;

/// Anchor is never even attempted at or beyond this radius (task terrain
/// facts: the auto-break line probes at r ~= 7.49, past `WALL_START` = 7.0;
/// 7.4 stays a safe margin inside that so the AI never commits to an Anchor
/// that the sim is about to force-break anyway).
const ANCHOR_VIABLE_R: f32 = 7.4;
/// Radius past which "near the rim" escape-read logic prefers Anchor
/// (knockback/ring-out resistance) over whatever the opponent's special
/// would otherwise suggest — still capped by `ANCHOR_VIABLE_R` above.
const ANCHOR_OVERRIDE_R: f32 = 6.5;

/// Movement caution starts blending toward the arena center once the AI's
/// own radius passes this fraction of `arena::ARENA_RADIUS`.
const CAUTION_START_FRAC: f32 = 0.70;
/// Hard safety valve: beyond this radius, ignore every other candidate and
/// retreat straight toward the center (belt-and-suspenders against the
/// `ace_never_self_rings_out_alone` gate — `RING_OUT_RADIUS` is 9.6).
const HARD_SAFE_R: f32 = 8.6;

/// Digital-direction quantization deadzone (component magnitude below this,
/// out of a roughly-unit-length direction vector, reads as neutral) — mirrors
/// `flow::chase_input`'s `DEADZONE` idea for a normalized vector input.
const DIR_DEADZONE: f32 = 0.35;

/// Don't bother wall-riding/chasing height the AI has no realistic way to
/// reach (also protects the "opponent parked far above the rim" balance
/// fixture from luring a wall-climb toward the rim for nothing).
const CHASE_HEIGHT_LIMIT: f32 = 6.0;

const FIRE_RANGE: f32 = 5.0;
const DASH_MIN_RANGE: f32 = 2.2;
const DASH_MAX_RANGE: f32 = 8.0;
const DASH_ALIGN_COS: f32 = 0.80;
const GUARD_INCOMING_RANGE: f32 = 4.0;
const CARVE_CHASE_RANGE: f32 = 2.5;
const HOP_OFFENSE_RANGE: f32 = 2.5;
const ANCHOR_LOW_SPIN_FRAC: f32 = 0.45;
const ANCHOR_SAFE_DIST: f32 = 3.0;
/// Anchor commit cap, steps: `lerp(ANCHOR_MAX_HOLD_PASSIVE, ANCHOR_MAX_HOLD_AGGRO,
/// aggression)` — higher aggression tops give up the free regen sooner and
/// go re-engage (also guarantees `every_tier_terminates`: two Anchor-heavy
/// tiers can't turtle forever, since both are forced off Anchor periodically).
const ANCHOR_MAX_HOLD_PASSIVE: f32 = 500.0;
const ANCHOR_MAX_HOLD_AGGRO: f32 = 150.0;
/// Forced stand-down after hitting the Anchor cap, steps.
const ANCHOR_COOLDOWN_STEPS: u16 = 200;

/// Difficulty-scaled parameters (SPEC §11's exact knob list). Values are the
/// *inputs* to [`decide`]'s scoring, not physics tuning.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AiParams {
    /// Steps of reaction lag: the AI acts on an observation this many steps
    /// stale (see [`AiState::delayed`]).
    pub reaction_delay_steps: u16,
    /// Radians of random directional noise applied before quantizing to
    /// digital `dir_x`/`dir_y`.
    pub aim_error: f32,
    /// 0..1: how much this tier favors closing/attacking over caution.
    pub aggression: f32,
    /// 0..1: gates Dash usage (probability + fluency).
    pub dash_skill: f32,
    /// 0..1: gates Guard/Hop/Carve/Anchor usage entirely (`0.0` == never).
    pub verb_skill: f32,
    /// 0..1: gates special-fire timing AND escape-read correctness.
    pub special_skill: f32,
    /// Seconds of opponent-velocity extrapolation for the movement chase
    /// target.
    pub predictive_lead: f32,
    /// Own spin fraction (0..1) below which rim-avoidance panic kicks in.
    pub panic_spin_threshold: f32,
}

/// The four SPEC §11 tiers, in `Difficulty`'s declaration order (Easy,
/// Normal, Hard, Ace) so `AI_TIERS[difficulty as usize]` is the lookup — see
/// [`tier`].
///
/// Rationale per tier (task spec intent):
/// - **Easy**: slow (~30-step ~0.25s reaction), sloppy aim (~31 degrees of
///   noise), rarely dashes (8% roll when a dash would otherwise fire), and
///   `verb_skill`/`special_skill` are EXACTLY `0.0` — the gate, not just a
///   low probability — so Easy never presses Guard/Hop/Carve/Anchor/Special
///   at all (see `tests/ai.rs`'s gating test) and simply eats every armed
///   special.
/// - **Normal**: moderate across the board — a mid-pack reaction window,
///   real but imperfect aim, uses every verb some of the time, roughly a
///   coin-flip on escape reads.
/// - **Hard**: fast (~8 steps, ~67 ms), verb-fluent (0.8), and reads armed
///   specials correctly 75% of the time (`special_skill`) — "escapes armed
///   specials with the correct read MOST of the time" per the task spec.
/// - **Ace**: near-frame-perfect (~4 steps, ~33 ms), essentially always uses
///   every technique (0.97) and reads escapes correctly 95% of the time —
///   the `>= 80%` observed-correct-reads gate in `tests/ai.rs` is set well
///   below this 95% design value so the test has real margin against RNG
///   variance across seeds.
pub const AI_TIERS: [AiParams; 4] = [
    // Easy
    AiParams {
        reaction_delay_steps: 30,
        aim_error: 0.55,
        aggression: 0.25,
        dash_skill: 0.08,
        verb_skill: 0.0,
        special_skill: 0.0,
        predictive_lead: 0.0,
        panic_spin_threshold: 0.15,
    },
    // Normal
    AiParams {
        reaction_delay_steps: 16,
        aim_error: 0.28,
        aggression: 0.5,
        dash_skill: 0.4,
        verb_skill: 0.4,
        special_skill: 0.35,
        predictive_lead: 0.15,
        panic_spin_threshold: 0.25,
    },
    // Hard
    AiParams {
        reaction_delay_steps: 8,
        aim_error: 0.12,
        aggression: 0.7,
        dash_skill: 0.75,
        verb_skill: 0.8,
        special_skill: 0.75,
        predictive_lead: 0.25,
        panic_spin_threshold: 0.3,
    },
    // Ace
    AiParams {
        reaction_delay_steps: 4,
        aim_error: 0.04,
        aggression: 0.85,
        dash_skill: 0.95,
        verb_skill: 0.97,
        special_skill: 0.95,
        predictive_lead: 0.35,
        panic_spin_threshold: 0.35,
    },
];

/// `AI_TIERS[difficulty as usize]` as a named lookup (kept here rather than
/// as an inherent `impl Difficulty` method purely for locality with the table
/// it indexes).
pub fn tier(difficulty: Difficulty) -> AiParams {
    AI_TIERS[difficulty as usize]
}

/// The three verbs an "escape read" can commit to (game_design.md §2's
/// Crash-Out escape table).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EscapeVerb {
    Hop,
    Anchor,
    Carve,
}

/// One reaction-delay-buffer slot: the small set of scalar/vector facts
/// `decide` needs, snapshotted from `World` each call. Deliberately NOT a
/// clone of `Top`/`World` (task spec: "NOT by cloning Worlds") — just plain
/// `Copy` data.
#[derive(Clone, Copy, Debug)]
struct Observation {
    me_pos: Vec3,
    me_vel: Vec3,
    me_spin: f32,
    me_grounded: bool,
    me_dash_cd: u16,
    me_hop_land_lag: u8,
    me_special_armed: bool,
    opp_pos: Vec3,
    opp_vel: Vec3,
    opp_dash_active: u16,
    opp_dash_startup: u8,
    opp_special_active: u16,
    opp_special_armed: bool,
    opp_special_id: SpecialId,
}

impl Default for Observation {
    fn default() -> Self {
        Self {
            me_pos: Vec3::default(),
            me_vel: Vec3::default(),
            me_spin: 0.0,
            me_grounded: false,
            me_dash_cd: 0,
            me_hop_land_lag: 0,
            me_special_armed: false,
            opp_pos: Vec3::default(),
            opp_vel: Vec3::default(),
            opp_dash_active: 0,
            opp_dash_startup: 0,
            opp_special_active: 0,
            opp_special_armed: false,
            // Harmless placeholder (mirrors `CombatState::default`'s own
            // choice) — real observations always overwrite this from the
            // opponent's actual `combat.special_id`.
            opp_special_id: SpecialId::Overclock,
        }
    }
}

impl Observation {
    fn capture(world: &World, me: usize, opp: usize) -> Self {
        let m = &world.tops[me];
        let o = &world.tops[opp];
        Self {
            me_pos: m.pos,
            me_vel: m.vel,
            me_spin: m.spin,
            me_grounded: m.grounded,
            me_dash_cd: m.dash_cd,
            me_hop_land_lag: m.combat.hop_land_lag,
            me_special_armed: m.combat.special_armed,
            opp_pos: o.pos,
            opp_vel: o.vel,
            opp_dash_active: o.dash_active,
            opp_dash_startup: o.combat.dash_startup,
            opp_special_active: o.combat.special_active,
            opp_special_armed: o.combat.special_armed,
            opp_special_id: o.combat.special_id,
        }
    }
}

/// Per-controller AI state: its OWN `Rng` stream, the reaction-delay ring
/// buffer, and committed-plan timers (escape read, Anchor hold/cooldown).
/// Lives entirely outside `World` (task spec: "not sim state, not hashed").
#[derive(Clone, Debug)]
pub struct AiState {
    rng: Rng,
    buf: [Observation; OBS_BUFFER_LEN],
    write_idx: usize,
    filled: usize,

    // Escape-read commitment (rolled ONCE per threat instance, see `decide`).
    escape_verb: Option<EscapeVerb>,
    escape_prev_threat: bool,
    escape_hop_fired: bool,

    // Anchor commit cap / forced stand-down (see `ANCHOR_MAX_HOLD_*`).
    anchor_hold_steps: u16,
    anchor_cooldown: u16,
}

impl AiState {
    /// Fresh AI state seeded from `seed` (the caller mixes in a salt distinct
    /// from the round's own `World::rng` and the launch-roll RNG — see
    /// `flow.rs`'s `AI_FIGHT_SEED_SALT`).
    pub fn new(seed: u64) -> Self {
        Self {
            rng: Rng::new(seed),
            buf: [Observation::default(); OBS_BUFFER_LEN],
            write_idx: 0,
            filled: 0,
            escape_verb: None,
            escape_prev_threat: false,
            escape_hop_fired: false,
            anchor_hold_steps: 0,
            anchor_cooldown: 0,
        }
    }

    fn push_observation(&mut self, obs: Observation) {
        self.buf[self.write_idx] = obs;
        self.write_idx = (self.write_idx + 1) % OBS_BUFFER_LEN;
        if self.filled < OBS_BUFFER_LEN {
            self.filled += 1;
        }
    }

    /// The observation from `delay` steps ago, clamped to what's actually
    /// been recorded so far (early in a round the AI simply sees the
    /// freshest available frame rather than garbage/default data).
    fn delayed(&self, delay: u16) -> Observation {
        let max_delay = self.filled.saturating_sub(1);
        let delay = (delay as usize).min(max_delay).min(OBS_BUFFER_LEN - 1);
        let idx = (self.write_idx + OBS_BUFFER_LEN - 1 - delay) % OBS_BUFFER_LEN;
        self.buf[idx]
    }
}

/// Rotate a 2D vector by `angle` radians (fixmath-only trig, SPEC §5).
fn rotate(v: Vec2, angle: f32) -> Vec2 {
    let c = fixmath::cos(angle);
    let s = fixmath::sin(angle);
    Vec2::new(v.x * c - v.y * s, v.x * s + v.y * c)
}

/// Quantize a roughly-unit-length world-space `(x, z)` direction to digital
/// `dir_x`/`dir_y`. Mirrors `physics::input_dir_xz`'s convention exactly:
/// `dir_x` maps to `+x`, `dir_y` maps to `-z` — so recovering `dir_y` from a
/// desired world-z component negates it.
fn quantize_dir(v: Vec2) -> (i8, i8) {
    let dir_x = if v.x > DIR_DEADZONE {
        1
    } else if v.x < -DIR_DEADZONE {
        -1
    } else {
        0
    };
    let dir_y = if v.y > DIR_DEADZONE {
        -1
    } else if v.y < -DIR_DEADZONE {
        1
    } else {
        0
    };
    (dir_x, dir_y)
}

/// The CORRECT escape verb for an opponent's active special (game_design.md
/// §2's Crash-Out escape table + the task's terrain facts): Hop clears
/// Guillotine Rush's ground tracking and Sinkhole's pull (both read the
/// grounded/airborne split — Hop's i-frames cover steps 4-12, Sinkhole never
/// pulls airborne tops); Carve outranges Aegis Lock's outlast game and
/// Slipstream's telegraphed pass; near the rim (but still inside the
/// auto-break line) Anchor's knockback/ring-out resistance overrides
/// whatever the specific special would otherwise suggest, since a ring-out
/// push is the more urgent risk. Second Wind/Overclock/Riposte have no
/// dedicated escape verb in the design (a buff, a sub-window timer, and
/// "don't attack into it" respectively) — `None` means "play normal",
/// which the elevated movement caution already covers.
fn correct_escape_verb(special: SpecialId, r_me: f32) -> Option<EscapeVerb> {
    if (ANCHOR_OVERRIDE_R..ANCHOR_VIABLE_R).contains(&r_me) {
        return Some(EscapeVerb::Anchor);
    }
    match special {
        SpecialId::GuillotineRush | SpecialId::Sinkhole => Some(EscapeVerb::Hop),
        SpecialId::AegisLock | SpecialId::Slipstream => Some(EscapeVerb::Carve),
        SpecialId::SecondWind | SpecialId::Overclock | SpecialId::Riposte => None,
    }
}

/// A plausible-but-wrong escape pick for a failed read (task spec: "a WRONG
/// read picks a plausible-but-wrong verb, not a no-op") — one of the other
/// two escape verbs, chosen by the AI's own `Rng`.
fn wrong_escape_verb(rng: &mut Rng, correct: EscapeVerb) -> EscapeVerb {
    let candidates = match correct {
        EscapeVerb::Hop => [EscapeVerb::Anchor, EscapeVerb::Carve],
        EscapeVerb::Anchor => [EscapeVerb::Hop, EscapeVerb::Carve],
        EscapeVerb::Carve => [EscapeVerb::Hop, EscapeVerb::Anchor],
    };
    candidates[rng.range_i32(0, 2) as usize]
}

/// Decide one step's `InputState` for `world.tops[me]`, using `params` and
/// `state`'s reaction-delay buffer + own `Rng` (SPEC §11 / task spec). Pure
/// with respect to `World` (never mutates it); all internal state changes
/// land in `state`.
pub fn decide(state: &mut AiState, world: &World, me: usize, params: &AiParams) -> InputState {
    let opp = 1 - me;
    state.push_observation(Observation::capture(world, me, opp));
    let obs = state.delayed(params.reaction_delay_steps);

    let me_xz = Vec2::new(obs.me_pos.x, obs.me_pos.z);
    let opp_xz = Vec2::new(obs.opp_pos.x, obs.opp_pos.z);
    let r_me = me_xz.length();
    let me_spin_frac = (obs.me_spin / TUNE.spin_max).clamp(0.0, 1.0);

    // ---- 1. Movement: predictive chase blended with rim-avoidance. ----
    let lead_xz = opp_xz + Vec2::new(obs.opp_vel.x, obs.opp_vel.z).scaled(params.predictive_lead);
    let chase_dir = (lead_xz - me_xz).normalize_or_zero();
    let center_dir = (-me_xz).normalize_or_zero();

    let rim_frac = r_me / arena::ARENA_RADIUS;
    let mut caution = 0.0f32;
    if rim_frac > CAUTION_START_FRAC {
        caution += (rim_frac - CAUTION_START_FRAC) / (1.0 - CAUTION_START_FRAC);
    }
    if me_spin_frac < params.panic_spin_threshold {
        caution += 0.6;
    }
    if obs.opp_special_armed || obs.opp_special_active > 0 {
        caution += 0.3;
    }
    caution *= (1.4 - 0.6 * params.aggression).max(0.0);
    caution = caution.clamp(0.0, 1.0);

    let hard_safety = r_me > HARD_SAFE_R;
    let mut move_dir = if hard_safety {
        center_dir
    } else if chase_dir.length_sq() > 0.0 {
        (chase_dir.scaled(1.0 - caution) + center_dir.scaled(caution)).normalize_or_zero()
    } else {
        Vec2::default()
    };

    if move_dir.length_sq() > 0.0 && params.aim_error > 0.0 {
        let err = (state.rng.next_f32() * 2.0 - 1.0) * params.aim_error;
        move_dir = rotate(move_dir, err);
    }
    let (dir_x, dir_y) = quantize_dir(move_dir);

    // Hard safety valve: dangerously close to the rim — retreat only, skip
    // every other candidate so nothing can pull the AI further out.
    if hard_safety {
        return InputState {
            dir_x,
            dir_y,
            ..Default::default()
        };
    }

    // ---- 2. Escape read: opponent's special is ACTIVE (fired). ----
    let opp_threat = obs.opp_special_active > 0;
    if opp_threat && !state.escape_prev_threat {
        state.escape_verb = if params.verb_skill > 0.0 {
            correct_escape_verb(obs.opp_special_id, r_me).map(|correct| {
                if state.rng.next_f32() < params.special_skill {
                    correct
                } else {
                    wrong_escape_verb(&mut state.rng, correct)
                }
            })
        } else {
            None
        };
        state.escape_hop_fired = false;
    } else if !opp_threat {
        state.escape_verb = None;
    }
    state.escape_prev_threat = opp_threat;

    if let Some(verb) = state.escape_verb {
        let mut input = InputState {
            dir_x,
            dir_y,
            ..Default::default()
        };
        match verb {
            EscapeVerb::Hop => {
                // Edge-style: press exactly once per threat instance (Hop
                // itself is an edge trigger in the sim; holding it does
                // nothing extra and risks a landing re-trigger loop).
                if !state.escape_hop_fired && obs.me_grounded {
                    input.hop = true;
                    state.escape_hop_fired = true;
                }
            }
            EscapeVerb::Anchor => {
                input.anchor = r_me < ANCHOR_VIABLE_R;
            }
            EscapeVerb::Carve => {
                input.carve = true;
            }
        }
        return input;
    }

    let mut input = InputState {
        dir_x,
        dir_y,
        ..Default::default()
    };
    let dist = (opp_xz - me_xz).length();

    // ---- 3. Stance: Guard / Hop / Anchor / Carve, mutually exclusive. ----
    // Fixed candidate order for deterministic tie-breaks: Guard, Hop,
    // Anchor, Carve.
    let mut chosen_stance: Option<u8> = None;
    if params.verb_skill > 0.0 && obs.me_grounded {
        let opp_dashing = obs.opp_dash_active > 0 || obs.opp_dash_startup > 0;
        let guard_score = if opp_dashing && dist < GUARD_INCOMING_RANGE {
            params.verb_skill
        } else {
            0.0
        };

        let hop_score = if obs.me_hop_land_lag == 0
            && dist < HOP_OFFENSE_RANGE
            && obs.opp_pos.y < CHASE_HEIGHT_LIMIT
        {
            params.verb_skill * params.aggression * 0.5
        } else {
            0.0
        };

        let unpressured = dist > ANCHOR_SAFE_DIST && obs.opp_special_active == 0;
        let low_spin = me_spin_frac < ANCHOR_LOW_SPIN_FRAC;
        let outward_speed = Vec2::new(obs.me_vel.x, obs.me_vel.z).dot(center_dir.scaled(-1.0));
        let knocked_outward = outward_speed > 1.5;
        let anchor_wanted = r_me < ANCHOR_VIABLE_R
            && state.anchor_cooldown == 0
            && (knocked_outward || (low_spin && unpressured));
        let anchor_score = if anchor_wanted {
            params.verb_skill * 0.9
        } else {
            0.0
        };

        let chasing = dist > CARVE_CHASE_RANGE && obs.opp_pos.y < CHASE_HEIGHT_LIMIT;
        let carve_score = if chasing {
            params.verb_skill * 0.7
        } else {
            0.0
        };

        let candidates = [guard_score, hop_score, anchor_score, carve_score];
        let mut best_i = 0usize;
        let mut best_v = 0.0f32;
        for (i, &v) in candidates.iter().enumerate() {
            if v > best_v {
                best_v = v;
                best_i = i;
            }
        }
        if best_v > 0.0 {
            chosen_stance = Some(best_i as u8);
        }
    }

    match chosen_stance {
        Some(0) => input.guard = true,
        Some(1) => input.hop = true,
        Some(2) => input.anchor = true,
        Some(3) => input.carve = true,
        _ => {}
    }

    // Anchor commit cap: forced stand-down after `ANCHOR_MAX_HOLD_*` steps of
    // continuous hold, so no tier (and no mirror-matchup) can turtle forever
    // (`every_tier_terminates`; Anchor nets positive spin regen unpressured).
    if input.anchor {
        state.anchor_hold_steps = state.anchor_hold_steps.saturating_add(1);
        let max_hold = (ANCHOR_MAX_HOLD_PASSIVE
            - (ANCHOR_MAX_HOLD_PASSIVE - ANCHOR_MAX_HOLD_AGGRO) * params.aggression.clamp(0.0, 1.0))
            as u16;
        if state.anchor_hold_steps > max_hold {
            input.anchor = false;
            state.anchor_hold_steps = 0;
            state.anchor_cooldown = ANCHOR_COOLDOWN_STEPS;
        }
    } else {
        state.anchor_hold_steps = 0;
    }
    if state.anchor_cooldown > 0 {
        state.anchor_cooldown -= 1;
        input.anchor = false;
    }

    // ---- 4. Dash: independent edge decision (closing attack). ----
    let dash_stance_ok = !matches!(chosen_stance, Some(0) | Some(2)); // not Guard/Anchor
    if params.dash_skill > 0.0
        && dash_stance_ok
        && obs.me_dash_cd == 0
        && obs.me_grounded
        && dist > DASH_MIN_RANGE
        && dist < DASH_MAX_RANGE
    {
        let to_opp_dir = (opp_xz - me_xz).normalize_or_zero();
        let move_world = Vec2::new(dir_x as f32, -(dir_y as f32)).normalize_or_zero();
        let aligned = move_world.dot(to_opp_dir) > DASH_ALIGN_COS;
        if aligned && state.rng.next_f32() < params.dash_skill {
            input.dash = true;
        }
    }

    // ---- 5. Special: independent edge decision (fire when Armed). ----
    if params.special_skill > 0.0
        && obs.me_special_armed
        && dist < FIRE_RANGE
        && state.rng.next_f32() < params.special_skill
    {
        input.special = true;
    }

    input
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_are_in_declaration_order_and_scale_monotonically() {
        // Reaction delay should get faster Easy -> Ace; skills should get
        // more fluent Easy -> Ace (the whole point of the tier ladder).
        for i in 0..AI_TIERS.len() - 1 {
            assert!(
                AI_TIERS[i].reaction_delay_steps >= AI_TIERS[i + 1].reaction_delay_steps,
                "tier {i} should react no faster than tier {}",
                i + 1
            );
            assert!(AI_TIERS[i].aim_error >= AI_TIERS[i + 1].aim_error);
        }
        assert_eq!(AI_TIERS[0].verb_skill, 0.0, "Easy must gate verbs to zero");
        assert_eq!(
            AI_TIERS[0].special_skill, 0.0,
            "Easy must gate special usage to zero"
        );
    }

    #[test]
    fn tier_lookup_matches_difficulty_declaration_order() {
        assert_eq!(tier(Difficulty::Easy).reaction_delay_steps, 30);
        assert_eq!(tier(Difficulty::Normal).reaction_delay_steps, 16);
        assert_eq!(tier(Difficulty::Hard).reaction_delay_steps, 8);
        assert_eq!(tier(Difficulty::Ace).reaction_delay_steps, 4);
    }

    #[test]
    fn correct_escape_verb_matches_the_design_table() {
        // Mid-arena (not near the rim): per-special mapping applies.
        assert_eq!(
            correct_escape_verb(SpecialId::GuillotineRush, 3.0),
            Some(EscapeVerb::Hop)
        );
        assert_eq!(
            correct_escape_verb(SpecialId::Sinkhole, 3.0),
            Some(EscapeVerb::Hop)
        );
        assert_eq!(
            correct_escape_verb(SpecialId::AegisLock, 3.0),
            Some(EscapeVerb::Carve)
        );
        assert_eq!(
            correct_escape_verb(SpecialId::Slipstream, 3.0),
            Some(EscapeVerb::Carve)
        );
        assert_eq!(correct_escape_verb(SpecialId::SecondWind, 3.0), None);
        assert_eq!(correct_escape_verb(SpecialId::Overclock, 3.0), None);
        assert_eq!(correct_escape_verb(SpecialId::Riposte, 3.0), None);

        // Near the rim (but still inside the viable Anchor zone): Anchor
        // overrides regardless of special.
        assert_eq!(
            correct_escape_verb(SpecialId::GuillotineRush, 7.0),
            Some(EscapeVerb::Anchor)
        );
        // Past the auto-break line: back to the per-special mapping.
        assert_eq!(
            correct_escape_verb(SpecialId::GuillotineRush, 7.45),
            Some(EscapeVerb::Hop)
        );
    }

    #[test]
    fn wrong_escape_verb_never_returns_the_correct_one() {
        let mut rng = Rng::new(1234);
        for _ in 0..200 {
            let picked = wrong_escape_verb(&mut rng, EscapeVerb::Hop);
            assert_ne!(picked, EscapeVerb::Hop);
            let picked = wrong_escape_verb(&mut rng, EscapeVerb::Anchor);
            assert_ne!(picked, EscapeVerb::Anchor);
            let picked = wrong_escape_verb(&mut rng, EscapeVerb::Carve);
            assert_ne!(picked, EscapeVerb::Carve);
        }
    }

    #[test]
    fn quantize_dir_matches_input_dir_xz_convention() {
        // +x world direction -> dir_x = 1, dir_y = 0.
        assert_eq!(quantize_dir(Vec2::new(1.0, 0.0)), (1, 0));
        // +z world direction -> dir_y = -1 (physics::input_dir_xz convention).
        assert_eq!(quantize_dir(Vec2::new(0.0, 1.0)), (0, -1));
        // Small components stay neutral (deadzone).
        assert_eq!(quantize_dir(Vec2::new(0.1, -0.1)), (0, 0));
    }

    #[test]
    fn observation_ring_buffer_reads_back_the_delayed_slot() {
        let mut state = AiState::new(42);
        for i in 0..10u32 {
            let obs = Observation {
                me_spin: i as f32,
                ..Observation::default()
            };
            state.push_observation(obs);
        }
        // Most recent push was i=9 (delay 0); 3 steps back is i=6.
        assert_eq!(state.delayed(0).me_spin, 9.0);
        assert_eq!(state.delayed(3).me_spin, 6.0);
        // Delay beyond what's been recorded clamps to the oldest available.
        assert_eq!(state.delayed(999).me_spin, 0.0);
    }
}
