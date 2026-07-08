//! M5 tier-behavior unit tests (task spec Task 4): skill gating, escape
//! reads, reaction delay, rim avoidance. Fixture style mirrors
//! `tests/combat.rs` (hand-built `Top`/`World` literals via the public
//! `physics` structs) for the scenarios that need exact, reproducible setup;
//! `tests/balance.rs` covers full-match outcomes separately.

use floppy_core::ai::{self, AiParams, AiState, AI_TIERS};
use floppy_core::combat::{CombatState, SpecialId};
use floppy_core::input::InputState;
use floppy_core::physics::{LaunchParams, Stats, Top, World};
use floppy_core::rng::Rng;
use floppy_core::vec::{Vec2, Vec3};

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

#[allow(clippy::too_many_arguments)]
fn make_top(
    pos: Vec3,
    vel: Vec3,
    spin: f32,
    spin_dir: i8,
    grounded: bool,
    stats: Stats,
    combat: CombatState,
) -> Top {
    Top {
        pos,
        vel,
        spin,
        spin_dir,
        tilt: Vec2::default(),
        tilt_phase: 0.0,
        spin_angle: 0.0,
        radius: 0.95,
        height: 1.0,
        stats,
        grounded,
        dash_cd: 0,
        dash_active: 0,
        meter: 0.0,
        airdash_used: false,
        combat,
    }
}

fn world_with(top0: Top, top1: Top) -> World {
    World {
        tops: [top0, top1],
        rng: Rng::new(1),
        step: 0,
        hitstop: 0,
        outcome: None,
        events: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// 1. Easy's skill gates actually gate.
// ---------------------------------------------------------------------------

/// Easy's `verb_skill`/`special_skill` are exactly `0.0` (see `ai::AI_TIERS`'s
/// doc comment) — the design intent is that this is a hard gate, not merely
/// a low probability, so Easy must NEVER press special/guard/hop/carve/
/// anchor no matter how much "opportunity" an adversarial opponent creates
/// (constant dashing, repeated special fires, Easy's own meter kept
/// Armed). Ace is used as the adversarial opponent specifically because it
/// uses every technique fluently and aggressively.
#[test]
fn easy_never_presses_gated_verbs_over_a_long_adversarial_run() {
    let easy = AI_TIERS[0];
    let ace = AI_TIERS[3];
    assert_eq!(
        easy.verb_skill, 0.0,
        "test assumes Easy's verb_skill gate is 0"
    );
    assert_eq!(
        easy.special_skill, 0.0,
        "test assumes Easy's special_skill gate is 0"
    );

    let stats = keystone_stats();
    let params = [
        LaunchParams {
            heading: 0.0,
            depth: 0.7,
            power: 0.5,
            quality: 1.0,
            spin_dir: 1,
            stats,
            special_id: SpecialId::GuillotineRush,
        },
        LaunchParams {
            heading: std::f32::consts::PI,
            depth: 0.7,
            power: 0.5,
            quality: 1.0,
            spin_dir: -1,
            stats,
            special_id: SpecialId::GuillotineRush,
        },
    ];
    let mut world = World::launch(777, params);
    // Keep Easy's own special Armed from the start so the (gated) offensive
    // special-fire opportunity is live throughout, not just whenever the
    // passive meter trickle happens to reach 100.
    world.tops[1].meter = 100.0;
    world.tops[1].combat.special_armed = true;

    let mut ai_easy = AiState::new(1);
    let mut ai_ace = AiState::new(2);

    let mut steps_run = 0u32;
    for step in 0..6000u32 {
        if world.outcome.is_some() {
            break;
        }
        let ace_input = ai::decide(&mut ai_ace, &world, 0, &ace);
        let easy_input = ai::decide(&mut ai_easy, &world, 1, &easy);
        assert!(!easy_input.special, "Easy pressed special at step {step}");
        assert!(!easy_input.guard, "Easy pressed guard at step {step}");
        assert!(!easy_input.hop, "Easy pressed hop at step {step}");
        assert!(!easy_input.carve, "Easy pressed carve at step {step}");
        assert!(!easy_input.anchor, "Easy pressed anchor at step {step}");
        world.step([ace_input, easy_input]);
        // Re-arm every step so the opportunity persists even though Easy's
        // gating means it's never consumed by an actual fire.
        world.tops[1].meter = 100.0;
        world.tops[1].combat.special_armed = true;
        steps_run += 1;
    }
    assert!(
        steps_run > 200,
        "adversarial run ended too quickly to be a meaningful check ({steps_run} steps)"
    );
}

// ---------------------------------------------------------------------------
// 2. Hard/Ace escape read.
// ---------------------------------------------------------------------------

/// Ace's `special_skill` is 0.95 (see `ai::AI_TIERS`'s doc comment): with the
/// opponent firing Guillotine Rush at mid-arena (the task's own terrain fact:
/// Hop's i-frames are the correct escape here), Ace should output `hop`
/// within a short read window in at least 80% of seeds — well below the 95%
/// design value, leaving real margin against RNG variance across seeds.
#[test]
fn ace_hop_escapes_guillotine_rush_within_the_read_window() {
    const SEEDS: u32 = 20;
    const WINDOW: u32 = 20;
    let ace = AI_TIERS[3];
    let stats = keystone_stats();

    let mut correct = 0u32;
    for seed in 0..SEEDS {
        let mut combat0 = CombatState::new(SpecialId::GuillotineRush);
        combat0.special_active = 48; // just fired (game_design.md §3 duration)
        let top0 = make_top(
            Vec3::new(-2.0, 0.0, 0.0),
            Vec3::new(2.0, 0.0, 0.0),
            6000.0,
            1,
            true,
            stats,
            combat0,
        );
        let top1 = make_top(
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::default(),
            6000.0,
            -1,
            true,
            stats,
            CombatState::new(SpecialId::Overclock),
        );
        let mut world = world_with(top0, top1);

        let mut ai_state = AiState::new(1000 + seed as u64);
        let mut saw_hop = false;
        for _ in 0..WINDOW {
            if world.outcome.is_some() {
                break;
            }
            let input1 = ai::decide(&mut ai_state, &world, 1, &ace);
            if input1.hop {
                saw_hop = true;
                break;
            }
            world.step([InputState::default(), input1]);
            // Keep the threat "live" for the whole window rather than
            // letting Guillotine Rush's own 48-step timer be the limiting
            // factor here — the read window is about REACTION latency, not
            // the special's duration.
            world.tops[0].combat.special_active = 48;
        }
        if saw_hop {
            correct += 1;
        }
    }

    let rate = correct as f32 / SEEDS as f32;
    assert!(
        rate >= 0.80,
        "Ace should read Guillotine Rush correctly (Hop) >= 80% of the time, got {correct}/{SEEDS} ({:.0}%)",
        rate * 100.0
    );
}

// ---------------------------------------------------------------------------
// 3. Reaction delay.
// ---------------------------------------------------------------------------

fn build_world_with_target_at(target_x: f32, stats: Stats) -> World {
    let top0 = make_top(
        Vec3::new(target_x, 0.0, 0.0),
        Vec3::default(),
        8000.0,
        1,
        true,
        stats,
        CombatState::new(SpecialId::Overclock),
    );
    let top1 = make_top(
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::default(),
        8000.0,
        -1,
        true,
        stats,
        CombatState::new(SpecialId::Overclock),
    );
    world_with(top0, top1)
}

/// Number of calls (after an abrupt world change) until `decide`'s movement
/// output flips to reflect it, for a fixed `tier`/seed. The world is never
/// stepped in between — this isolates the reaction-delay RING BUFFER from
/// physics entirely (task spec: "change the world abruptly").
fn steps_until_dir_x_flips(tier: AiParams, seed: u64) -> u32 {
    let stats = keystone_stats();
    let mut world = build_world_with_target_at(-5.0, stats);
    let mut state = AiState::new(seed);

    // Warm the ring buffer past its capacity with the ORIGINAL (target-left)
    // world so every delay depth genuinely reflects "target is to the left".
    for _ in 0..50 {
        let _ = ai::decide(&mut state, &world, 1, &tier);
    }
    let warmed = ai::decide(&mut state, &world, 1, &tier);
    assert_eq!(
        warmed.dir_x, -1,
        "warmed-up read should see the original (left) target"
    );

    // Abruptly move the target to the far right; count calls until dir_x
    // flips to +1.
    world.tops[0].pos.x = 5.0;
    for i in 1..200u32 {
        let input = ai::decide(&mut state, &world, 1, &tier);
        if input.dir_x == 1 {
            return i;
        }
    }
    panic!("dir_x never flipped within 200 post-change calls");
}

/// Easy's much longer `reaction_delay_steps` (30 vs Ace's 4, see
/// `ai::AI_TIERS`) should make it visibly slower to react to an abrupt world
/// change than Ace — both in absolute terms (near its own configured delay)
/// and relative to Ace.
#[test]
fn easy_lags_far_behind_ace_after_an_abrupt_world_change() {
    let easy = AI_TIERS[0];
    let ace = AI_TIERS[3];

    let easy_flip = steps_until_dir_x_flips(easy, 11);
    let ace_flip = steps_until_dir_x_flips(ace, 12);

    // Exact expectation (ring-buffer indexing is deterministic, not
    // RNG-dependent): the flip lands `reaction_delay_steps + 1` calls after
    // the change. A tolerance of a couple of steps guards against off-by-one
    // interpretation differences without weakening the property being
    // tested.
    assert!(
        (easy_flip as i32 - (easy.reaction_delay_steps as i32 + 1)).abs() <= 2,
        "easy_flip={easy_flip}, expected near {}",
        easy.reaction_delay_steps + 1
    );
    assert!(
        (ace_flip as i32 - (ace.reaction_delay_steps as i32 + 1)).abs() <= 2,
        "ace_flip={ace_flip}, expected near {}",
        ace.reaction_delay_steps + 1
    );
    assert!(
        easy_flip > ace_flip + 10,
        "Easy ({easy_flip} steps) should lag far behind Ace ({ace_flip} steps)"
    );
}

// ---------------------------------------------------------------------------
// 4. Rim avoidance.
// ---------------------------------------------------------------------------

/// A low-spin AI near the rim must move INWARD (panic threshold, weighted by
/// aggression) even when its chase target sits further outward still — the
/// rim-avoidance caution should dominate the raw chase direction.
#[test]
fn low_spin_ai_near_rim_moves_inward_despite_an_outward_chase_target() {
    let stats = keystone_stats();
    let hard = AI_TIERS[2]; // panic_spin_threshold = 0.3

    // top0 (chase target) sits further out along +x than top1, so a naive
    // chase would push top1 OUTWARD; top1's spin is far below the panic
    // threshold and it's already well past the caution-start radius.
    let top0 = make_top(
        Vec3::new(9.0, 0.0, 0.0),
        Vec3::default(),
        8000.0,
        1,
        true,
        stats,
        CombatState::new(SpecialId::Overclock),
    );
    let top1 = make_top(
        Vec3::new(8.0, 0.0, 0.0),
        Vec3::default(),
        1000.0, // spin_frac = 0.1, well under Hard's 0.3 panic threshold
        -1,
        true,
        stats,
        CombatState::new(SpecialId::Overclock),
    );
    let world = world_with(top0, top1);

    let mut state = AiState::new(5);
    let input = ai::decide(&mut state, &world, 1, &hard);
    assert_eq!(
        input.dir_x, -1,
        "expected the panicked AI to retreat toward the center (-x), got dir_x={}",
        input.dir_x
    );
}
