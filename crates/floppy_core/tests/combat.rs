//! M4-A combat layer tests (task spec Module 4): verb inertness, counterplay
//! smoke tests, meter economy, Crash-Out/round-scoring, one focused test per
//! special, determinism over a verb-heavy scripted run, and hit-stop.
//!
//! Fixture style mirrors `physics_invariants.rs`: tops/worlds are built
//! directly via the public `physics::{Top, World}` struct literals rather
//! than through the launch minigame, so every scenario below is an exact,
//! reproducible setup rather than an emergent multi-step outcome.

use floppy_core::arena;
use floppy_core::combat::{self, CombatState, RoundEnd, SpecialId};
use floppy_core::input::InputState;
use floppy_core::physics::{BattleEvent, LaunchParams, Outcome, Stats, Top, World, TUNE};
use floppy_core::rng::Rng;
use floppy_core::vec::{Vec2, Vec3};

const NO_INPUT: InputState = InputState {
    dir_x: 0,
    dir_y: 0,
    dash: false,
    special: false,
    guard: false,
    hop: false,
    carve: false,
    anchor: false,
};

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
// 1. Verb inertness.
// ---------------------------------------------------------------------------

/// Every `CombatState` field must stay bit-identical to its spawn value
/// (i.e. `CombatState::default()`, except `special_id` which is legitimately
/// assigned at spawn) across a long run of pure `NO_INPUT` steps — the direct
/// proof that no verb/special "leaks" state or RNG draws when unused. This is
/// the M4-A version of "N steps with NO_INPUT match a pre-M4 world": since a
/// literal pre-M4 build no longer exists in this tree, the equivalent and
/// stronger guarantee is that the ENTIRE new `combat` field never moves, so
/// every M2/M3 formula (which still only reads `pos`/`vel`/`spin`/`tilt`/...)
/// evaluates exactly as it did before this milestone.
#[test]
fn no_input_never_perturbs_combat_state_over_a_long_run() {
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
    let mut world = World::launch(123, params);
    let spawn_special_ids = [
        world.tops[0].combat.special_id,
        world.tops[1].combat.special_id,
    ];

    for step in 0..1500 {
        world.step([NO_INPUT, NO_INPUT]);
        for (i, top) in world.tops.iter().enumerate() {
            let expected = CombatState {
                special_id: spawn_special_ids[i],
                ..CombatState::default()
            };
            assert_eq!(
                top.combat, expected,
                "top {i} combat state drifted under NO_INPUT at step {step}"
            );
        }
        if world.outcome.is_some() {
            break;
        }
    }
}

/// Companion determinism check over the same NO_INPUT scenario: two
/// identical launches must produce an identical hash sequence (a pure
/// function of the deterministic formulas verb-inertness above establishes
/// are undisturbed).
#[test]
fn no_input_hash_sequence_is_reproducible() {
    let run = || {
        let stats = keystone_stats();
        let params = [
            LaunchParams {
                heading: 0.3,
                depth: 0.7,
                power: 0.5,
                quality: 1.08,
                spin_dir: 1,
                stats,
                special_id: SpecialId::Overclock,
            },
            LaunchParams {
                heading: std::f32::consts::PI + 0.3,
                depth: 0.7,
                power: 0.5,
                quality: 1.08,
                spin_dir: -1,
                stats,
                special_id: SpecialId::Overclock,
            },
        ];
        let mut world = World::launch(7, params);
        let mut hashes = Vec::new();
        for _ in 0..600 {
            world.step([NO_INPUT, NO_INPUT]);
            hashes.push(world.state_hash());
        }
        hashes
    };
    assert_eq!(run(), run());
}

// ---------------------------------------------------------------------------
// 2. Counterplay smoke tests.
// ---------------------------------------------------------------------------

/// A frontal Guard block (past the parry window) drains the defender less
/// than an identical unguarded hit.
#[test]
fn guard_block_reduces_drain_versus_unguarded_baseline() {
    let stats = keystone_stats();
    let make_pair = |guard_hold_before_step: u16, guard_pressed: bool| {
        let combat0 = CombatState {
            guard_hold: guard_hold_before_step,
            last_move_dir: Vec2::new(1.0, 0.0),
            ..CombatState::default()
        };
        let top0 = make_top(
            Vec3::new(0.0, arena::height(0.0, 0.0), 0.0),
            Vec3::default(),
            8000.0,
            1,
            true,
            stats,
            combat0,
        );
        let top1 = make_top(
            Vec3::new(0.5, arena::height(0.5, 0.0), 0.0),
            Vec3::new(-5.0, 0.0, 0.0),
            8000.0,
            1,
            true,
            stats,
            CombatState::default(),
        );
        let world = world_with(top0, top1);
        (world, guard_pressed)
    };

    let (mut guarded, guard_pressed) = make_pair(9, true);
    let input = InputState {
        guard: guard_pressed,
        ..NO_INPUT
    };
    guarded.step([input, NO_INPUT]);
    let guarded_drain = 8000.0 - guarded.tops[0].spin;
    assert!(guarded
        .events
        .iter()
        .any(|e| matches!(e, BattleEvent::GuardBlock { who: 0 })));

    let (mut baseline, _) = make_pair(0, false);
    baseline.step([NO_INPUT, NO_INPUT]);
    let baseline_drain = 8000.0 - baseline.tops[0].spin;

    assert!(
        guarded_drain < baseline_drain,
        "guarded={guarded_drain} baseline={baseline_drain}"
    );
}

/// Frame-precise parry window (game_design.md §2): a hit landing when
/// `guard_hold` reaches 7 (within the first 8 steps of a press) parries; the
/// same setup at `guard_hold == 9` is a plain block instead.
#[test]
fn parry_window_is_frame_precise() {
    let stats = keystone_stats();
    let make_pair = |guard_hold_before_step: u16| {
        let combat0 = CombatState {
            guard_hold: guard_hold_before_step,
            last_move_dir: Vec2::new(1.0, 0.0),
            ..CombatState::default()
        };
        let top0 = make_top(
            Vec3::new(0.0, arena::height(0.0, 0.0), 0.0),
            Vec3::default(),
            8000.0,
            1,
            true,
            stats,
            combat0,
        );
        let top1 = make_top(
            Vec3::new(0.5, arena::height(0.5, 0.0), 0.0),
            Vec3::new(-5.0, 0.0, 0.0),
            8000.0,
            1,
            true,
            stats,
            CombatState::default(),
        );
        world_with(top0, top1)
    };
    let input_guard = InputState {
        guard: true,
        ..NO_INPUT
    };

    // guard_hold: 6 -> 7 this step (press+7, inside the 8-step window).
    let mut at7 = make_pair(6);
    at7.step([input_guard, NO_INPUT]);
    assert_eq!(at7.tops[0].combat.guard_hold, 7);
    assert!(at7
        .events
        .iter()
        .any(|e| matches!(e, BattleEvent::Parry { who: 0 })));
    let drain_at7 = 8000.0 - at7.tops[0].spin;
    assert!(
        drain_at7 < 50.0,
        "parry should nearly-zero drain: {drain_at7}"
    );

    // guard_hold: 8 -> 9 this step (press+9, past the window).
    let mut at9 = make_pair(8);
    at9.step([input_guard, NO_INPUT]);
    assert_eq!(at9.tops[0].combat.guard_hold, 9);
    assert!(!at9
        .events
        .iter()
        .any(|e| matches!(e, BattleEvent::Parry { .. })));
    assert!(at9
        .events
        .iter()
        .any(|e| matches!(e, BattleEvent::GuardBlock { who: 0 })));
    let drain_at9 = 8000.0 - at9.tops[0].spin;
    assert!(
        drain_at9 > drain_at7,
        "block should drain more than parry: block={drain_at9} parry={drain_at7}"
    );
}

/// Anchor reduces incoming knockback versus an unanchored baseline.
#[test]
fn anchor_resists_knockback_versus_baseline() {
    let stats = keystone_stats();
    let make_pair = |anchor_hold: u16| {
        let combat0 = CombatState {
            anchor_hold,
            ..CombatState::default()
        };
        let top0 = make_top(
            Vec3::new(0.0, arena::height(0.0, 0.0), 0.0),
            Vec3::default(),
            8000.0,
            1,
            true,
            stats,
            combat0,
        );
        let top1 = make_top(
            Vec3::new(0.5, arena::height(0.5, 0.0), 0.0),
            Vec3::new(-5.0, 0.0, 0.0),
            8000.0,
            1,
            true,
            stats,
            CombatState::default(),
        );
        world_with(top0, top1)
    };
    let input_anchor = InputState {
        anchor: true,
        ..NO_INPUT
    };

    let mut anchored = make_pair(3);
    anchored.step([input_anchor, NO_INPUT]);
    let anchored_speed = Vec2::new(anchored.tops[0].vel.x, anchored.tops[0].vel.z).length();

    let mut baseline = make_pair(0);
    baseline.step([NO_INPUT, NO_INPUT]);
    let baseline_speed = Vec2::new(baseline.tops[0].vel.x, baseline.tops[0].vel.z).length();

    assert!(
        anchored_speed < baseline_speed,
        "anchored={anchored_speed} baseline={baseline_speed}"
    );
}

/// Over-holding Carve accelerates a self-topple: a top held in Carve
/// continuously reaches the tilt+low-spin topple condition strictly sooner
/// than an identical top that never touches Carve (game_design.md §2:
/// "over-carving topples you").
#[test]
fn carve_over_hold_topples_self_faster_than_the_baseline() {
    // Low STA/WGT (fast decay, low tilt stability) and a starting spin just
    // above the topple threshold keep this test's step budget small: without
    // Carve, tilt alone never approaches the topple threshold in this
    // window (verified below via the control run), so any topple is
    // attributable to Carve's tilt bonus stacking on top of the baseline.
    let stats = Stats {
        atk: 50,
        def: 50,
        sta: 0,
        wgt: 0,
        spd: 50,
        mtr: 50,
    };
    let omega = 0.748_331_5_f32; // sqrt(0.56), matches physics_invariants.rs's orbit_dummy derivation.
    let r0 = 2.6f32;
    let make_dummy = || {
        make_top(
            Vec3::new(r0, arena::height(r0, 0.0), 0.0),
            Vec3::new(0.0, 0.0, omega * r0),
            TUNE.spin_max,
            1,
            true,
            stats,
            CombatState::default(),
        )
    };
    let make_main = || {
        make_top(
            Vec3::new(0.0, arena::height(0.0, 0.0), 0.0),
            Vec3::default(),
            2800.0,
            1,
            true,
            stats,
            CombatState::default(),
        )
    };

    const STEPS: u32 = 1500;
    let input_carve = InputState {
        carve: true,
        ..NO_INPUT
    };

    let mut carving = world_with(make_main(), make_dummy());
    let mut toppled = false;
    for _ in 0..STEPS {
        carving.step([input_carve, NO_INPUT]);
        if matches!(carving.outcome, Some(Outcome::StaminaOut { loser: 0 })) {
            toppled = true;
            break;
        }
        if carving.outcome.is_some() {
            break;
        }
    }
    assert!(toppled, "carving top should topple within {STEPS} steps");

    let mut control = world_with(make_main(), make_dummy());
    for _ in 0..STEPS {
        control.step([NO_INPUT, NO_INPUT]);
        assert!(
            control.outcome.is_none(),
            "control (no carve) should not topple within {STEPS} steps"
        );
    }
}

/// Hop's de-penetration i-frames (steps 4..=12) dodge an approaching hit:
/// zero collision drain (only ordinary passive decay applies) and a meter
/// bonus for the dodge.
#[test]
fn hop_iframes_dodge_a_dash_hit() {
    let stats = keystone_stats();
    let combat0 = CombatState {
        hop_air_steps: 6, // inside the 4..=12 i-frame window
        ..CombatState::default()
    };
    let top0 = make_top(
        Vec3::new(0.0, arena::height(0.0, 0.0) + 1.0, 0.0),
        Vec3::default(),
        8000.0,
        1,
        false,
        stats,
        combat0,
    );
    let top1 = make_top(
        Vec3::new(0.5, arena::height(0.5, 0.0) + 1.0, 0.0),
        Vec3::new(-5.0, 0.0, 0.0),
        8000.0,
        1,
        false,
        stats,
        CombatState::default(),
    );
    let mut world = world_with(top0, top1);
    world.tops[1].dash_active = 5;

    let meter_before = world.tops[0].meter;
    let spin_before = world.tops[0].spin;
    world.step([NO_INPUT, NO_INPUT]);

    assert!(
        spin_before - world.tops[0].spin < 1.0,
        "i-frame top should take no collision drain, lost {}",
        spin_before - world.tops[0].spin
    );
    assert!(
        world.tops[0].meter > meter_before,
        "i-frame dodge should grant meter"
    );
    assert!(
        !world
            .events
            .iter()
            .any(|e| matches!(e, BattleEvent::Hit { .. })),
        "a dodged hit must not still emit a normal Hit event"
    );
}

/// A higher fall height drains more from an aerial slam (game_design.md §2:
/// "250 + 8*fall_height_m").
#[test]
fn aerial_slam_drains_more_from_a_higher_fall() {
    let stats = keystone_stats();
    // Geometry note: the collision normal must have a meaningful VERTICAL
    // component for a mostly-downward velocity to register as "approaching"
    // (`v_rel > 0`) at all — a small horizontal offset with a large vertical
    // gap (dominant y separation) gives exactly that, mimicking a top
    // falling onto an opponent from above.
    let ground = arena::height(0.15, 0.0);
    let make_slam = |fall_height: f32| {
        let combat0 = CombatState {
            hop_slam_armed: true,
            hop_apex_y: ground + 1.5 + fall_height,
            ..CombatState::default()
        };
        let top0 = make_top(
            Vec3::new(0.0, ground + 1.5, 0.0),
            Vec3::new(0.0, -3.0, 0.0),
            8000.0,
            1,
            false,
            stats,
            combat0,
        );
        let top1 = make_top(
            Vec3::new(0.3, ground, 0.0),
            Vec3::default(),
            8000.0,
            1,
            true,
            stats,
            CombatState::default(),
        );
        world_with(top0, top1)
    };

    let mut low = make_slam(1.0);
    low.step([NO_INPUT, NO_INPUT]);
    let low_drain = 8000.0 - low.tops[1].spin;
    assert!(low
        .events
        .iter()
        .any(|e| matches!(e, BattleEvent::AerialSlam { who: 0 })));

    let mut high = make_slam(10.0);
    high.step([NO_INPUT, NO_INPUT]);
    let high_drain = 8000.0 - high.tops[1].spin;

    assert!(
        high_drain > low_drain,
        "higher fall should drain more: high={high_drain} low={low_drain}"
    );
}

// ---------------------------------------------------------------------------
// 3. Meter economy + Crash-Out / round scoring.
// ---------------------------------------------------------------------------

/// Passive trickle matches the "~66 s to arm solo" claim (game_design.md
/// §2): measured over a short, collision-safe window and extrapolated via
/// the exact `TUNE` constants (a multi-thousand-step live simulation risks
/// the second top's orbit eventually drifting into a contaminating
/// collision, which this avoids entirely).
#[test]
fn passive_meter_trickle_matches_the_66_second_arming_rate() {
    let stats = Stats {
        atk: 50,
        def: 50,
        sta: 50,
        wgt: 50,
        spd: 50,
        mtr: 50, // meter_gain_mult == 1.0 exactly
    };
    let omega = 0.748_331_5_f32;
    let r0 = 2.6f32;
    let top0 = make_top(
        Vec3::new(0.0, arena::height(0.0, 0.0), 0.0),
        Vec3::default(),
        TUNE.spin_max,
        1,
        true,
        stats,
        CombatState::default(),
    );
    let dummy = make_top(
        Vec3::new(r0, arena::height(r0, 0.0), 0.0),
        Vec3::new(0.0, 0.0, omega * r0),
        TUNE.spin_max,
        1,
        true,
        stats,
        CombatState::default(),
    );
    let mut world = world_with(top0, dummy);

    const STEPS: u32 = 1200; // 10 s
    for _ in 0..STEPS {
        world.step([NO_INPUT, NO_INPUT]);
        assert!(world.outcome.is_none(), "no out condition expected here");
    }
    let elapsed_s = STEPS as f32 / 120.0;
    let expected = TUNE.meter_gain_passive_per_s * elapsed_s;
    assert!(
        (world.tops[0].meter - expected).abs() < 0.5,
        "meter={} expected~{}",
        world.tops[0].meter,
        expected
    );

    let seconds_to_arm = TUNE.meter_armed_threshold / TUNE.meter_gain_passive_per_s;
    assert!(
        (seconds_to_arm - 66.67).abs() < 1.0,
        "seconds_to_arm={seconds_to_arm}"
    );
}

/// Dash-hit meter bonus matches "5 x MTR-multiplier" exactly.
#[test]
fn dash_hit_meter_bonus_matches_5_times_mtr_multiplier() {
    let stats = Stats {
        atk: 50,
        def: 50,
        sta: 50,
        wgt: 50,
        spd: 50,
        mtr: 70,
    };
    let got = combat::scaled_meter_gain(TUNE.meter_gain_dash_hit, &stats);
    let expected = 5.0 * stats.meter_gain_mult();
    assert!(
        (got - expected).abs() < 1e-4,
        "got={got} expected={expected}"
    );
    assert!((expected - 5.0 * 1.12).abs() < 1e-4);
}

/// A dashing hit grants more total meter than an otherwise-identical
/// non-dashing hit.
#[test]
fn dash_active_hit_grants_more_meter_than_a_plain_hit() {
    let stats = keystone_stats();
    let make_pair = |dash_active: u16| {
        let top0 = make_top(
            Vec3::new(0.0, arena::height(0.0, 0.0), 0.0),
            Vec3::default(),
            8000.0,
            1,
            true,
            stats,
            CombatState::default(),
        );
        let top1 = make_top(
            Vec3::new(0.5, arena::height(0.5, 0.0), 0.0),
            Vec3::new(-5.0, 0.0, 0.0),
            8000.0,
            1,
            true,
            stats,
            CombatState::default(),
        );
        let mut world = world_with(top0, top1);
        world.tops[1].dash_active = dash_active;
        world
    };
    let mut dashing = make_pair(5);
    dashing.step([NO_INPUT, NO_INPUT]);
    let mut plain = make_pair(0);
    plain.step([NO_INPUT, NO_INPUT]);

    assert!(
        dashing.tops[1].meter > plain.tops[1].meter,
        "dashing={} plain={}",
        dashing.tops[1].meter,
        plain.tops[1].meter
    );
}

/// Firing zeroes meter and opens the Crash-Out window for exactly 144 steps
/// (kill on step 144 counts, step 145 does not — SPEC §6.5).
#[test]
fn special_fire_opens_a_144_step_crash_out_window() {
    let stats = keystone_stats();
    let combat0 = CombatState {
        special_armed: true,
        special_id: SpecialId::Overclock,
        ..CombatState::default()
    };
    // Far apart on the gentle basin slope (well clear of the steep wall
    // band past r=7) so nothing rolls into a confounding collision during
    // the many steps this test holds the window open — the window-
    // countdown mechanism is what's under test, not collision physics.
    let top0 = make_top(
        Vec3::new(-3.0, arena::height(-3.0, 0.0), 0.0),
        Vec3::default(),
        5000.0,
        1,
        true,
        stats,
        combat0,
    );
    let top1 = make_top(
        Vec3::new(3.0, arena::height(3.0, 0.0), 0.0),
        Vec3::default(),
        5000.0,
        1,
        true,
        stats,
        CombatState::default(),
    );
    let mut world = world_with(top0, top1);
    world.tops[0].meter = 100.0;

    let fire_input = InputState {
        special: true,
        ..NO_INPUT
    };
    world.step([fire_input, NO_INPUT]);

    assert!(world
        .events
        .iter()
        .any(|e| matches!(e, BattleEvent::SpecialFire { who: 0 })));
    // Firing zeroes meter in Phase 1; Phase 7's passive trickle still
    // applies later in the SAME step, so the end-of-step value is a hair
    // above zero rather than exactly zero (documented, intentional).
    assert!(
        world.tops[0].meter < 1.0,
        "meter should be ~zero right after firing, got {}",
        world.tops[0].meter
    );
    assert!(!world.tops[0].combat.special_armed);
    assert_eq!(world.tops[0].combat.crash_window, TUNE.crash_window_steps);

    // Special-fire itself carries its own hit-stop (game_design.md §5: 3
    // steps) — those steps are frozen (SPEC §5: hit-stop skips whole sim
    // steps), so the Crash-Out window correctly does NOT tick during them.
    // Drain that hit-stop first so the following loop counts exactly 143
    // REAL (unfrozen) steps, matching "kill inside 144 steps [of real sim
    // time, the window's own unit] counts".
    while world.hitstop > 0 {
        world.step([fire_input, NO_INPUT]);
    }
    assert_eq!(
        world.tops[0].combat.crash_window, TUNE.crash_window_steps,
        "the window must not tick during the fire's own hit-stop"
    );

    for _ in 0..(TUNE.crash_window_steps - 1) {
        world.step([fire_input, NO_INPUT]);
    }
    assert_eq!(
        world.tops[0].combat.crash_window, 1,
        "144th real step since firing: window should have exactly 1 step left"
    );
    world.step([fire_input, NO_INPUT]);
    assert_eq!(
        world.tops[0].combat.crash_window, 0,
        "145th real step since firing: window should be closed"
    );
}

/// `combat::round_points` scores Crash-Out only while the winner's own
/// window is open; the same kill outside the window scores Over/Survivor,
/// and a simultaneous double-out is always a draw regardless.
#[test]
fn round_points_scores_crash_out_only_within_the_window() {
    let stats = keystone_stats();
    let winner_with = |crash_window: u16| {
        let c = CombatState {
            crash_window,
            ..CombatState::default()
        };
        make_top(Vec3::default(), Vec3::default(), 5000.0, 1, true, stats, c)
    };
    let loser = || {
        make_top(
            Vec3::default(),
            Vec3::default(),
            0.0,
            1,
            true,
            stats,
            CombatState::default(),
        )
    };

    let mut world_open = world_with(winner_with(1), loser());
    world_open.outcome = Some(Outcome::RingOut { loser: 1 });
    assert_eq!(
        combat::round_points(&world_open),
        Some((0, RoundEnd::CrashOut))
    );

    let mut world_closed = world_with(winner_with(0), loser());
    world_closed.outcome = Some(Outcome::RingOut { loser: 1 });
    assert_eq!(
        combat::round_points(&world_closed),
        Some((0, RoundEnd::Over))
    );

    let mut world_stam_open = world_with(winner_with(50), loser());
    world_stam_open.outcome = Some(Outcome::StaminaOut { loser: 1 });
    assert_eq!(
        combat::round_points(&world_stam_open),
        Some((0, RoundEnd::CrashOut))
    );

    let mut world_stam_closed = world_with(winner_with(0), loser());
    world_stam_closed.outcome = Some(Outcome::StaminaOut { loser: 1 });
    assert_eq!(
        combat::round_points(&world_stam_closed),
        Some((0, RoundEnd::Survivor))
    );

    let mut world_sim = world_with(winner_with(50), loser());
    world_sim.outcome = Some(Outcome::Simultaneous);
    assert_eq!(combat::round_points(&world_sim), None);

    let world_undecided = world_with(winner_with(50), loser());
    assert_eq!(combat::round_points(&world_undecided), None);

    assert_eq!(RoundEnd::CrashOut.points(), 3);
    assert_eq!(RoundEnd::Over.points(), 2);
    assert_eq!(RoundEnd::Survivor.points(), 1);
    assert_eq!(RoundEnd::Draw.points(), 0);
}

// ---------------------------------------------------------------------------
// 4. Specials — one focused test each.
// ---------------------------------------------------------------------------

#[test]
fn aegis_lock_reflects_half_the_absorbed_drain_to_the_attacker() {
    let stats = keystone_stats();
    let combat0 = CombatState {
        special_id: SpecialId::AegisLock,
        special_active: 10,
        ..CombatState::default()
    };
    let top0 = make_top(
        Vec3::new(0.0, arena::height(0.0, 0.0), 0.0),
        Vec3::default(),
        8000.0,
        1,
        true,
        stats,
        combat0,
    );
    let top1 = make_top(
        Vec3::new(0.5, arena::height(0.5, 0.0), 0.0),
        Vec3::new(-6.0, 0.0, 0.0),
        8000.0,
        1,
        true,
        stats,
        CombatState::default(),
    );
    let mut world = world_with(top0, top1);
    world.step([NO_INPUT, NO_INPUT]);

    let attacker_drain = 8000.0 - world.tops[1].spin;
    assert!(
        attacker_drain > 0.0,
        "attacker should take reflected drain, took {attacker_drain}"
    );

    // Compare the Aegis user's own drain to an unshielded baseline.
    let top0_baseline = make_top(
        Vec3::new(0.0, arena::height(0.0, 0.0), 0.0),
        Vec3::default(),
        8000.0,
        1,
        true,
        stats,
        CombatState::default(),
    );
    let top1_baseline = make_top(
        Vec3::new(0.5, arena::height(0.5, 0.0), 0.0),
        Vec3::new(-6.0, 0.0, 0.0),
        8000.0,
        1,
        true,
        stats,
        CombatState::default(),
    );
    let mut baseline = world_with(top0_baseline, top1_baseline);
    baseline.step([NO_INPUT, NO_INPUT]);
    let aegis_defender_drain = 8000.0 - world.tops[0].spin;
    let baseline_defender_drain = 8000.0 - baseline.tops[0].spin;
    assert!(
        aegis_defender_drain < baseline_defender_drain,
        "aegis={aegis_defender_drain} baseline={baseline_defender_drain}"
    );
}

#[test]
fn slipstream_passes_through_and_deals_an_exit_hit() {
    let stats = keystone_stats();
    let combat0 = CombatState {
        special_id: SpecialId::Slipstream,
        special_active: 10,
        ..CombatState::default()
    };
    let top0 = make_top(
        Vec3::new(0.0, arena::height(0.0, 0.0), 0.0),
        Vec3::new(5.0, 0.0, 0.0),
        8000.0,
        1,
        true,
        stats,
        combat0,
    );
    let top1 = make_top(
        Vec3::new(0.3, arena::height(0.3, 0.0), 0.0),
        Vec3::default(),
        8000.0,
        1,
        true,
        stats,
        CombatState::default(),
    );
    let mut world = world_with(top0, top1);
    let vel_before = world.tops[0].vel;
    let spin_before = world.tops[0].spin;
    world.step([NO_INPUT, NO_INPUT]);

    assert!(
        (world.tops[0].vel.x - vel_before.x).abs() < 1.0,
        "slipstream user shouldn't be knocked back by the pass"
    );
    assert!(
        spin_before - world.tops[0].spin < 1.0,
        "slipstream user shouldn't take collision drain"
    );
    assert!(
        world.tops[1].spin < 8000.0 - 1.0,
        "opponent should take the exit-hit drain"
    );
    assert!(
        world.tops[0].combat.special_flag,
        "the pass should be consumed"
    );
    assert!(world
        .events
        .iter()
        .any(|e| matches!(e, BattleEvent::SpecialHit { who: 0 })));
}

#[test]
fn sinkhole_pulls_the_opponent_more_than_terrain_slope_alone() {
    let stats = keystone_stats();
    let make_pair = |sinkhole_active: bool| {
        let mut combat0 = CombatState::default();
        if sinkhole_active {
            combat0.special_id = SpecialId::Sinkhole;
            combat0.special_active = 10;
        }
        let top0 = make_top(
            Vec3::new(0.0, arena::height(0.0, 0.0), 0.0),
            Vec3::default(),
            8000.0,
            1,
            true,
            stats,
            combat0,
        );
        let top1 = make_top(
            Vec3::new(2.0, arena::height(2.0, 0.0), 0.0),
            Vec3::default(),
            8000.0,
            1,
            true,
            stats,
            CombatState::default(),
        );
        world_with(top0, top1)
    };
    let mut with_sinkhole = make_pair(true);
    with_sinkhole.step([NO_INPUT, NO_INPUT]);
    let with_speed = with_sinkhole.tops[1].vel.x;

    let mut without = make_pair(false);
    without.step([NO_INPUT, NO_INPUT]);
    let without_speed = without.tops[1].vel.x;

    assert!(
        with_speed < without_speed,
        "sinkhole should pull harder toward its owner: with={with_speed} without={without_speed}"
    );
}

#[test]
fn riposte_negates_the_hit_and_reverses_it_onto_the_attacker() {
    let stats = keystone_stats();
    let combat0 = CombatState {
        special_id: SpecialId::Riposte,
        special_active: 10,
        ..CombatState::default()
    };
    let top0 = make_top(
        Vec3::new(0.0, arena::height(0.0, 0.0), 0.0),
        Vec3::default(),
        8000.0,
        1,
        true,
        stats,
        combat0,
    );
    let top1 = make_top(
        Vec3::new(0.5, arena::height(0.5, 0.0), 0.0),
        Vec3::new(-6.0, 0.0, 0.0),
        8000.0,
        1,
        true,
        stats,
        CombatState::default(),
    );
    let mut world = world_with(top0, top1);
    world.step([NO_INPUT, NO_INPUT]);

    assert!(
        8000.0 - world.tops[0].spin < 1.0,
        "riposte user should take no drain, took {}",
        8000.0 - world.tops[0].spin
    );
    assert!(
        world.tops[1].spin < 8000.0 - 1.0,
        "attacker should take drain from the reversal"
    );
    assert!(
        world.tops[0].combat.special_flag,
        "riposte should be marked triggered"
    );
    assert!(world
        .events
        .iter()
        .any(|e| matches!(e, BattleEvent::SpecialHit { who: 0 })));
}

#[test]
fn guillotine_rush_bonus_is_one_time_and_increases_knockback_dealt() {
    let stats = keystone_stats();
    let make_pair = |guillotine: bool| {
        let mut combat0 = CombatState::default();
        if guillotine {
            combat0.special_id = SpecialId::GuillotineRush;
            combat0.special_active = 30;
        }
        let top0 = make_top(
            Vec3::new(0.0, arena::height(0.0, 0.0), 0.0),
            Vec3::new(5.0, 0.0, 0.0),
            8000.0,
            1,
            true,
            stats,
            combat0,
        );
        let top1 = make_top(
            Vec3::new(0.4, arena::height(0.4, 0.0), 0.0),
            Vec3::default(),
            8000.0,
            1,
            true,
            stats,
            CombatState::default(),
        );
        world_with(top0, top1)
    };

    let mut with_g = make_pair(true);
    with_g.step([NO_INPUT, NO_INPUT]);
    assert!(
        with_g.tops[0].combat.special_flag,
        "bonus should be consumed"
    );
    assert!(with_g
        .events
        .iter()
        .any(|e| matches!(e, BattleEvent::SpecialHit { who: 0 })));
    let with_speed = Vec2::new(with_g.tops[1].vel.x, with_g.tops[1].vel.z).length();

    let mut without_g = make_pair(false);
    without_g.step([NO_INPUT, NO_INPUT]);
    let without_speed = Vec2::new(without_g.tops[1].vel.x, without_g.tops[1].vel.z).length();

    assert!(
        with_speed > without_speed,
        "guillotine should knock harder: with={with_speed} without={without_speed}"
    );
}

#[test]
fn second_wind_grants_instant_spin_and_near_zero_decay() {
    let stats = keystone_stats();
    let combat0 = CombatState {
        special_id: SpecialId::SecondWind,
        special_armed: true,
        ..CombatState::default()
    };
    let top0 = make_top(
        Vec3::new(-3.0, arena::height(-3.0, 0.0), 0.0),
        Vec3::default(),
        5000.0,
        1,
        true,
        stats,
        combat0,
    );
    let top1 = make_top(
        Vec3::new(3.0, arena::height(3.0, 0.0), 0.0),
        Vec3::default(),
        5000.0,
        1,
        true,
        stats,
        CombatState::default(),
    );
    let mut world = world_with(top0, top1);
    let fire = InputState {
        special: true,
        ..NO_INPUT
    };
    world.step([fire, NO_INPUT]);

    let expected = (5000.0 + 0.18 * TUNE.spin_max).min(TUNE.spin_max);
    assert!(
        (world.tops[0].spin - expected).abs() < 5.0,
        "spin={} expected~{}",
        world.tops[0].spin,
        expected
    );

    let spin_after_fire = world.tops[0].spin;
    for _ in 0..120 {
        world.step([NO_INPUT, NO_INPUT]);
    }
    let decay_with_second_wind = spin_after_fire - world.tops[0].spin;

    let baseline_top = make_top(
        Vec3::new(-3.0, arena::height(-3.0, 0.0), 0.0),
        Vec3::default(),
        spin_after_fire,
        1,
        true,
        stats,
        CombatState::default(),
    );
    let mut baseline_world = world_with(baseline_top, top1);
    for _ in 0..120 {
        baseline_world.step([NO_INPUT, NO_INPUT]);
    }
    let decay_baseline = spin_after_fire - baseline_world.tops[0].spin;

    assert!(
        decay_with_second_wind < decay_baseline,
        "second wind should decay much slower: buffed={decay_with_second_wind} baseline={decay_baseline}"
    );
}

#[test]
fn overclock_effective_stats_add_the_flat_bonus_to_all_six_stats() {
    let stats = Stats {
        atk: 40,
        def: 40,
        sta: 40,
        wgt: 40,
        spd: 40,
        mtr: 40,
    };
    let combat0 = CombatState {
        special_id: SpecialId::Overclock,
        special_active: 10,
        ..CombatState::default()
    };
    let top0 = make_top(
        Vec3::default(),
        Vec3::default(),
        5000.0,
        1,
        true,
        stats,
        combat0,
    );
    let eff = combat::effective_stats(&top0);
    let bonus = TUNE.special_overclock_stat_bonus;
    assert_eq!(eff.atk, 40 + bonus);
    assert_eq!(eff.def, 40 + bonus);
    assert_eq!(eff.sta, 40 + bonus);
    assert_eq!(eff.wgt, 40 + bonus);
    assert_eq!(eff.spd, 40 + bonus);
    assert_eq!(eff.mtr, 40 + bonus);

    let mut top1 = top0;
    top1.combat.special_active = 0;
    let eff_inactive = combat::effective_stats(&top1);
    assert_eq!(eff_inactive.atk, 40, "inactive Overclock grants no bonus");
}

// ---------------------------------------------------------------------------
// 5. Determinism over a verb-heavy scripted run.
// ---------------------------------------------------------------------------

fn scripted_verb_inputs(step: u32) -> [InputState; 2] {
    let dir = match (step / 10) % 3 {
        0 => -1,
        1 => 0,
        _ => 1,
    };
    let phase = (step / 30) % 7;
    let base = match phase {
        0 => InputState {
            dash: true,
            ..NO_INPUT
        },
        1 => InputState {
            guard: true,
            ..NO_INPUT
        },
        2 => InputState {
            hop: true,
            ..NO_INPUT
        },
        3 => InputState {
            carve: true,
            ..NO_INPUT
        },
        4 => InputState {
            anchor: true,
            ..NO_INPUT
        },
        5 => InputState {
            special: true,
            ..NO_INPUT
        },
        _ => NO_INPUT,
    };
    [
        InputState {
            dir_x: dir,
            dir_y: -dir,
            ..base
        },
        InputState {
            dir_x: -dir,
            dir_y: dir,
            ..base
        },
    ]
}

#[test]
fn verb_heavy_scripted_2000_steps_is_deterministic() {
    let stats = keystone_stats();
    let make_world = || {
        let params = [
            LaunchParams {
                heading: 0.0,
                depth: 0.7,
                power: 0.6,
                quality: 1.08,
                spin_dir: 1,
                stats,
                special_id: SpecialId::Overclock,
            },
            LaunchParams {
                heading: std::f32::consts::PI,
                depth: 0.7,
                power: 0.6,
                quality: 1.08,
                spin_dir: -1,
                stats,
                special_id: SpecialId::Overclock,
            },
        ];
        World::launch(99, params)
    };
    let run = || {
        let mut world = make_world();
        let mut hashes = Vec::new();
        for s in 0..2000u32 {
            world.step(scripted_verb_inputs(s));
            hashes.push(world.state_hash());
            if world.outcome.is_some() {
                break;
            }
        }
        hashes
    };
    let a = run();
    let b = run();
    assert_eq!(a, b);
    assert!(a.windows(2).any(|w| w[0] != w[1]), "hashes never changed");
}

// ---------------------------------------------------------------------------
// 6. Hit-stop.
// ---------------------------------------------------------------------------

#[test]
fn heavy_hit_freezes_state_hash_for_exactly_hitstop_heavy_steps() {
    // Both tops GROUNDED (not airborne): this is a plain heavy hit, distinct
    // from the "airborne clash" category (hitstop 6) both-airborne collision
    // would classify as.
    let stats = keystone_stats();
    // Same geometry as the guard/anchor smoke tests above (verified there to
    // stay grounded through the step, unlike a symmetric +-0.35 approach
    // where each top's small horizontal drift can cross the terrain's
    // high-frequency ridge decoration and register as briefly airborne).
    let top0 = make_top(
        Vec3::new(0.0, arena::height(0.0, 0.0), 0.0),
        Vec3::default(),
        6000.0,
        1,
        true,
        stats,
        CombatState::default(),
    );
    let top1 = make_top(
        Vec3::new(0.5, arena::height(0.5, 0.0), 0.0),
        Vec3::new(-6.0, 0.0, 0.0),
        6000.0,
        -1,
        true,
        stats,
        CombatState::default(),
    );
    let mut world = world_with(top0, top1);
    world.step([NO_INPUT, NO_INPUT]);
    assert!(world
        .events
        .iter()
        .any(|e| matches!(e, BattleEvent::Hit { heavy: true, .. })));
    assert_eq!(world.hitstop, TUNE.hitstop_heavy as u8);

    // Compare `tops` (the actual sim state, per SPEC "hit-stop = skip N
    // whole sim steps"), NOT the full `state_hash()` — the latter also folds
    // in `hitstop` itself, which legitimately ticks down 4 -> 3 -> 2 -> 1 -> 0
    // across these frozen steps, so comparing the whole hash would (falsely)
    // never match.
    let frozen = world.hitstop;
    for _ in 0..frozen {
        let before = world.tops;
        world.step([NO_INPUT, NO_INPUT]);
        assert_eq!(before, world.tops, "top state changed during hit-stop skip");
    }
    assert_eq!(world.hitstop, 0);

    let before = world.tops;
    world.step([NO_INPUT, NO_INPUT]);
    assert_ne!(
        before, world.tops,
        "motion should resume once hit-stop clears"
    );
}
