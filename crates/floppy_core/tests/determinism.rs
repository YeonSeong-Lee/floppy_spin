//! M2 determinism harness (SPEC §5, §12) over the real `physics::World`,
//! replacing the M1 placeholder particle sim. Scripted inputs are a pure
//! function of the step index (never RNG-derived) so a "replay" is just this
//! function; `state_hash` is checked at fixed intervals across a long run.

use floppy_core::combat::SpecialId;
use floppy_core::input::InputState;
use floppy_core::physics::{LaunchParams, Stats, World};

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

fn make_world(seed: u64) -> World {
    let params = [
        LaunchParams {
            heading: 0.0,
            depth: 0.7,
            power: 0.6,
            quality: 1.08,
            spin_dir: 1,
            stats: keystone_stats(),
            special_id: SpecialId::Overclock,
        },
        LaunchParams {
            heading: std::f32::consts::PI,
            depth: 0.7,
            power: 0.6,
            quality: 1.08,
            spin_dir: -1,
            stats: keystone_stats(),
            special_id: SpecialId::Overclock,
        },
    ];
    World::launch(seed, params)
}

/// Deterministic function of the step index only (SPEC §6.4 / task spec):
/// `dir` cycles `-1, 0, 1` on a 40-step cadence, `dash` fires every 97th
/// step for whichever top's index matches (alternating).
fn scripted_inputs(step: u32) -> [InputState; 2] {
    let phase = (step / 40) % 3;
    let dir = match phase {
        0 => -1,
        1 => 0,
        _ => 1,
    };
    let dash_now = step.is_multiple_of(97);
    [
        InputState {
            dir_x: dir,
            dir_y: -dir,
            dash: dash_now,
            ..Default::default()
        },
        InputState {
            dir_x: -dir,
            dir_y: dir,
            dash: dash_now,
            ..Default::default()
        },
    ]
}

const STEPS: u32 = 1200;
const HASH_INTERVAL: u32 = 120;

fn run_hash_sequence(seed: u64) -> Vec<u64> {
    let mut world = make_world(seed);
    let mut hashes = Vec::new();
    for s in 0..STEPS {
        world.step(scripted_inputs(s));
        if (s + 1) % HASH_INTERVAL == 0 {
            hashes.push(world.state_hash());
        }
    }
    hashes
}

#[test]
fn same_seed_and_script_yields_identical_hash_sequence() {
    let a = run_hash_sequence(42);
    let b = run_hash_sequence(42);
    assert_eq!(a, b);
    // Sanity: a 1200-step scripted fight should actually produce more than
    // one distinct hash (i.e. the world isn't frozen/degenerate).
    assert!(
        a.windows(2).any(|w| w[0] != w[1]),
        "hashes never changed: {a:?}"
    );
}

#[test]
fn different_seed_diverges_or_matches_only_by_the_tiniest_chance() {
    let a = run_hash_sequence(42);
    let b = run_hash_sequence(1_000_003);
    assert_ne!(a, b);
}

#[test]
fn cloned_world_stays_hash_identical_when_stepped_in_lockstep() {
    let mut original = make_world(7);
    // Advance partway into the fight before cloning, so the clone starts
    // from genuinely "mid-fight" state (non-launch positions/velocities).
    for s in 0..300u32 {
        original.step(scripted_inputs(s));
    }
    let mut clone = original.clone();
    assert_eq!(original.state_hash(), clone.state_hash());

    for s in 300..600u32 {
        let inputs = scripted_inputs(s);
        original.step(inputs);
        clone.step(inputs);
        assert_eq!(
            original.state_hash(),
            clone.state_hash(),
            "diverged at step {s}"
        );
    }
}
