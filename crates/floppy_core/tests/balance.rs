//! M5 exit gate (ROADMAP.md M5 row / SPEC §12 point 4): the utility AI's
//! balance properties. A headless full-match harness mirrors `flow.rs`'s
//! round/match structure (spawn two launched tops directly, run `ai::decide`
//! for both sides every 120 Hz step, score via `combat::round_points`,
//! first-to-4 wins) without any screen machinery.
//!
//! Both tops here use identical (Keystone) stats/special so a match's
//! outcome is driven purely by the two `AiParams` tiers being compared, not
//! by roster asymmetry.

use floppy_core::ai::{self, AiParams, AiState, AI_TIERS};
use floppy_core::combat::{self, RoundEnd, SpecialId};
use floppy_core::input::InputState;
use floppy_core::physics::{LaunchParams, Outcome, Stats, World, TUNE};
use floppy_core::rng::mix_seed;
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

/// Step cap per round (task budget: ~120*180 = 21_600 steps, ~180s of
/// in-fight time). A round that doesn't decide within the cap counts as a
/// draw (task spec: "count a cap-out as a draw round") — the match simply
/// proceeds to the next round (a fresh seed, per `flow.rs`'s own documented
/// "a replayed draw round gets a fresh seed" convention: `round` increments
/// after every round including draws, so the next `mix_seed` call is
/// automatically a fresh attempt).
const ROUND_STEP_CAP: u32 = 120 * 180;

/// First-to-N (SPEC §6.5 / `flow::MATCH_WIN_POINTS`) — duplicated here
/// rather than depending on `flow` since this harness deliberately mirrors
/// flow's structure without its screen machinery.
const MATCH_WIN_POINTS: u8 = 4;

/// AI `Rng` seed salts distinct from the round seed and from each other
/// (mirrors `flow::AI_FIGHT_SEED_SALT`'s "distinct salt" reasoning).
const AI_SEED_SALT_0: u64 = 0xA00A_0000_0000_0001;
const AI_SEED_SALT_1: u64 = 0xA00B_0000_0000_0001;

fn spawn_round(seed: u64) -> World {
    let stats = keystone_stats();
    let params = [
        LaunchParams {
            heading: 0.0,
            depth: 0.7,
            power: 0.5,
            quality: 1.0,
            spin_dir: 1,
            stats,
            special_id: SpecialId::Overclock,
        },
        LaunchParams {
            heading: std::f32::consts::PI,
            depth: 0.7,
            power: 0.5,
            quality: 1.0,
            spin_dir: -1,
            stats,
            special_id: SpecialId::Overclock,
        },
    ];
    World::launch(seed, params)
}

/// Run one round to completion (or the step cap). `Some((winner, end))` on a
/// decided round (SPEC §6.5); `None` on a simultaneous double-out OR a
/// step-cap timeout (both scored as a draw).
fn run_round(seed: u64, params: [AiParams; 2], cap: u32) -> Option<(u8, RoundEnd)> {
    let mut world = spawn_round(seed);
    let mut ai0 = AiState::new(seed ^ AI_SEED_SALT_0);
    let mut ai1 = AiState::new(seed ^ AI_SEED_SALT_1);
    for _ in 0..cap {
        if world.outcome.is_some() {
            break;
        }
        let in0 = ai::decide(&mut ai0, &world, 0, &params[0]);
        let in1 = ai::decide(&mut ai1, &world, 1, &params[1]);
        world.step([in0, in1]);
    }
    combat::round_points(&world)
}

/// Run a full match (first to [`MATCH_WIN_POINTS`]), mirroring `flow.rs`'s
/// round-seed derivation (`mix_seed(match_seed, round)`, `round` incrementing
/// after every round including draws/cap-outs — see the module doc). Returns
/// `(winner_side, cap_out_count)`.
fn run_match(match_seed: u64, params: [AiParams; 2], cap: u32) -> (u8, u32) {
    let mut score = [0u8; 2];
    let mut round: u32 = 0;
    let mut cap_outs = 0u32;
    loop {
        let seed = mix_seed(match_seed, round);
        match run_round(seed, params, cap) {
            Some((winner, end)) => {
                score[winner as usize] = score[winner as usize].saturating_add(end.points());
            }
            None => cap_outs += 1,
        }
        if score[0] >= MATCH_WIN_POINTS {
            return (0, cap_outs);
        }
        if score[1] >= MATCH_WIN_POINTS {
            return (1, cap_outs);
        }
        round += 1;
        // Not part of the design — purely a guard against the TEST spinning
        // forever on a pathological configuration.
        assert!(round < 200, "match did not converge within 200 rounds");
    }
}

/// Balance gate 1 (ROADMAP M5): Hard must beat Easy in > 70% of matches over
/// a seed spread. 20 seeds, alternating which side is Hard, to cancel any
/// positional bias in the harness (top 0 spawns aimed 65% inward on the
/// launch circle same as top 1, so there shouldn't be one, but alternating
/// costs nothing and removes the question).
#[test]
fn hard_beats_easy_over_seed_spread() {
    const SEEDS: u32 = 20;
    let hard = AI_TIERS[2];
    let easy = AI_TIERS[0];

    let mut hard_wins = 0u32;
    for i in 0..SEEDS {
        let match_seed = 0x5EED_0000_0000_0000 ^ (i as u64);
        let hard_is_side0 = i % 2 == 0;
        let params = if hard_is_side0 {
            [hard, easy]
        } else {
            [easy, hard]
        };
        let (winner, cap_outs) = run_match(match_seed, params, ROUND_STEP_CAP);
        assert!(
            cap_outs <= 4,
            "seed {i}: {cap_outs} capped-out rounds (Hard vs Easy should decide cleanly)"
        );
        let hard_won = (hard_is_side0 && winner == 0) || (!hard_is_side0 && winner == 1);
        if hard_won {
            hard_wins += 1;
        }
    }

    let rate = hard_wins as f32 / SEEDS as f32;
    eprintln!(
        "hard_beats_easy_over_seed_spread: {hard_wins}/{SEEDS} ({:.0}%)",
        rate * 100.0
    );
    assert!(
        rate > 0.70,
        "Hard should beat Easy in > 70% of matches, got {hard_wins}/{SEEDS} ({:.0}%)",
        rate * 100.0
    );
}

/// Balance gate 2 (ROADMAP M5): Ace must never ring itself out. Fixture
/// (documented choice): the opponent top is pinned every step at `(0, 50, 0)`
/// — directly above the arena CENTER, far above any height Hop/Carve could
/// reach — with zero velocity, max spin, and zero tilt, so it never decides
/// the round itself (no stamina-out/ring-out/collision ever touches it) and
/// never moves. This isolates Ace with literally nothing to interact with:
/// its chase target sits exactly at the arena center (the safest possible
/// direction), so the only way this test could fail is Ace's own
/// movement/dash/carve logic driving it toward the rim by itself (dash/
/// aim-error overshoot, wall-ride chasing pointless height, etc.) — the
/// concern the `HARD_SAFE_R` valve and the `CHASE_HEIGHT_LIMIT` gate on
/// Carve/offense-Hop in `ai.rs` specifically exist to rule out.
#[test]
fn ace_never_self_rings_out_alone() {
    const SEEDS: u32 = 10;
    const STEPS: u32 = 120 * 120;
    let ace = AI_TIERS[3];

    for i in 0..SEEDS {
        let seed = 0xACE0_0000_0000_0000 ^ (i as u64);
        let mut world = spawn_round(seed);
        pin_parked_top(&mut world);
        let mut ai0 = AiState::new(seed ^ AI_SEED_SALT_0);

        for step in 0..STEPS {
            if world.outcome.is_some() {
                // Any OTHER outcome for top 0 (stamina-out) ends the fixture
                // harmlessly; only a self-ring-out is the failure this test
                // rules out, and it's checked every step below regardless.
                break;
            }
            let in0 = ai::decide(&mut ai0, &world, 0, &ace);
            world.step([in0, InputState::default()]);
            pin_parked_top(&mut world);
            assert_ne!(
                world.outcome,
                Some(Outcome::RingOut { loser: 0 }),
                "seed {i} step {step}: Ace rang itself out with no opponent interaction"
            );
        }
    }
}

/// Re-pin the fixture opponent (top 1) to its parked-out-of-reach state
/// (see `ace_never_self_rings_out_alone`'s doc comment) after every step.
fn pin_parked_top(world: &mut World) {
    let t = &mut world.tops[1];
    t.pos = Vec3::new(0.0, 50.0, 0.0);
    t.vel = Vec3::default();
    t.spin = TUNE.spin_max;
    t.tilt = Vec2::default();
    t.grounded = false;
}

/// Balance gate 3 (ROADMAP M5, implicit playability requirement): every tier
/// vs Normal must actually finish a match, hitting the per-round step cap at
/// most twice across the whole match.
#[test]
fn every_tier_terminates() {
    let normal = AI_TIERS[1];
    let tiers: [(&str, AiParams); 4] = [
        ("Easy", AI_TIERS[0]),
        ("Normal", AI_TIERS[1]),
        ("Hard", AI_TIERS[2]),
        ("Ace", AI_TIERS[3]),
    ];
    for (i, (name, params)) in tiers.iter().enumerate() {
        let match_seed = 0x7E12_1000_0000_0000 ^ (i as u64);
        let (_, cap_outs) = run_match(match_seed, [*params, normal], ROUND_STEP_CAP);
        assert!(
            cap_outs <= 2,
            "{name} vs Normal hit the step cap {cap_outs} times (> 2)"
        );
    }
}

/// Balance gate 4 (task spec "AI determinism"): same seed + same tier on
/// both sides -> two independent runs produce IDENTICAL `InputState`
/// sequences (compared via `pack()`) over 1000 steps.
#[test]
fn same_seed_and_tier_yields_identical_input_sequences() {
    fn run(seed: u64, params: [AiParams; 2], steps: u32) -> Vec<(u16, u16)> {
        let mut world = spawn_round(seed);
        let mut ai0 = AiState::new(seed ^ AI_SEED_SALT_0);
        let mut ai1 = AiState::new(seed ^ AI_SEED_SALT_1);
        let mut packed = Vec::with_capacity(steps as usize);
        for _ in 0..steps {
            if world.outcome.is_some() {
                break;
            }
            let in0 = ai::decide(&mut ai0, &world, 0, &params[0]);
            let in1 = ai::decide(&mut ai1, &world, 1, &params[1]);
            packed.push((in0.pack(), in1.pack()));
            world.step([in0, in1]);
        }
        packed
    }

    for &tier_params in AI_TIERS.iter() {
        let params = [tier_params, AI_TIERS[1]];
        let a = run(999, params, 1000);
        let b = run(999, params, 1000);
        assert_eq!(
            a, b,
            "tier {tier_params:?} produced divergent input sequences"
        );
    }
}
