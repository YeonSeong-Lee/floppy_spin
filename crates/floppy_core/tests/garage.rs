//! M8 Task 1 integration test (SPEC §12 "parts-resolve-no-special-casing"
//! gate): black-box coverage of `garage::resolve` through the PUBLIC API
//! only, plus the specific gate the milestone brief calls out — that a
//! garage-built top feeds `flow::spawn_fight_world`'s `LaunchParams` through
//! the exact same path a roster preset does, so the sim never special-cases
//! "is this a custom top". Per-part-table unit tests already live inline in
//! `garage.rs`; this file is the cross-module wiring check.

use floppy_core::combat::SpecialId;
use floppy_core::flow::{FlowState, MatchPhase, Screen, MY_BEY_INDEX};
use floppy_core::garage::{self, DEFAULT_PARTS};
use floppy_core::input::InputState;
use floppy_core::physics::{LaunchParams, World};

fn dash() -> InputState {
    InputState {
        dash: true,
        ..Default::default()
    }
}

fn dir_down() -> InputState {
    InputState {
        dir_y: 1,
        ..Default::default()
    }
}

fn tap(flow: &mut FlowState, input: InputState) {
    flow.advance(input, false);
    flow.advance(InputState::default(), false);
}

/// Drives a `FlowState` from Boot all the way to a spawned Fight world with
/// MY BEY as P1's pick, given a garage build. Returns the spawned `World`.
fn spawn_my_bey_fight(seed: u64, parts: [u8; 5]) -> World {
    let mut flow = FlowState::new(seed);
    for _ in 0..100 {
        if flow.screen == Screen::Title {
            break;
        }
        flow.advance(InputState::default(), false);
    }
    tap(&mut flow, dash()); // -> MainMenu
    tap(&mut flow, dash()); // QUICK BATTLE -> TopSelect

    flow.parts = parts;
    for _ in 0..MY_BEY_INDEX {
        tap(&mut flow, dir_down());
    }
    assert_eq!(flow.select_cursor, MY_BEY_INDEX);
    tap(&mut flow, dash()); // confirm -> Match(Intro)
    assert_eq!(flow.screen, Screen::Match(MatchPhase::Intro));

    for _ in 0..200 {
        if flow.screen == Screen::Match(MatchPhase::Launch) {
            break;
        }
        flow.advance(dash(), false);
    }
    flow.advance(InputState::default(), false);
    tap(&mut flow, dash()); // Aim -> SpinDir
    tap(&mut flow, dash()); // SpinDir -> Power
    tap(&mut flow, dash()); // Power lock -> Fight
    assert_eq!(flow.screen, Screen::Match(MatchPhase::Fight));

    flow.world.expect("Fight must have a spawned world")
}

#[test]
fn garage_build_reaches_the_fight_world_through_the_ordinary_launch_params_path() {
    let parts = [1, 3, 2, 0, 1];
    let expected = garage::resolve(parts);
    let world = spawn_my_bey_fight(0x6A5B_0001, parts);

    // P1 (index 0) is the garage build; the AI (index 1) is always a real
    // preset with a valid special id — neither top needed any custom-top
    // branch to spawn correctly.
    assert_eq!(world.tops[0].stats, expected.stats);
    assert_eq!(world.tops[0].spin_dir.signum(), expected.spin_dir.signum());
    assert!(
        world.tops[0].spin > 0.0,
        "a spawned top must have live spin"
    );
    assert!(world.tops[1].spin > 0.0);
}

#[test]
fn every_frame_choice_produces_a_fight_world_with_a_valid_special() {
    // Sweep every Frame (garage slot 0) with the rest at defaults, confirm
    // each one reaches Fight and carries a coherent special identity (i.e.
    // `SpecialId::from_silhouette` was exercised, not skipped).
    for frame_idx in 0..4u8 {
        let parts = [frame_idx, 0, 0, 0, 0];
        let expected = garage::resolve(parts);
        let world = spawn_my_bey_fight(0x1000 + frame_idx as u64, parts);
        let expected_special = SpecialId::from_silhouette(expected.silhouette);
        assert_eq!(world.tops[0].combat.special_id, expected_special);
    }
}

#[test]
fn out_of_range_saved_parts_still_spawn_a_valid_fight_world() {
    // A corrupted/foreign save could hand back any u8 per slot (garage::resolve
    // clamps out-of-range indices to 0 rather than panicking) — confirm that
    // property survives all the way through a real match spawn, not just
    // `resolve()` in isolation.
    let wild = [255u8, 200, 7, 42, 99];
    let world = spawn_my_bey_fight(0x0BAD_5EED, wild);
    let expected = garage::resolve(wild);
    assert_eq!(world.tops[0].stats, expected.stats);
}

#[test]
fn default_parts_build_matches_a_hand_rolled_launch_params_top() {
    // The garage build resolved from DEFAULT_PARTS, fed through
    // `LaunchParams`/`World::launch` directly (bypassing `flow` entirely),
    // must produce identical stats/spin_dir to the one flow spawns — proof
    // that flow.rs adds no hidden adjustment of its own.
    let build = garage::resolve(DEFAULT_PARTS);
    let params = LaunchParams {
        heading: 0.0,
        depth: 0.7,
        power: 0.5,
        quality: 1.0,
        spin_dir: build.spin_dir,
        stats: build.stats,
        special_id: SpecialId::from_silhouette(build.silhouette),
    };
    let world = World::launch(1, [params, params]);
    assert_eq!(world.tops[0].stats, build.stats);
    assert_eq!(world.tops[0].spin_dir, build.spin_dir);

    let flow_world = spawn_my_bey_fight(0x2222, DEFAULT_PARTS);
    assert_eq!(flow_world.tops[0].stats, build.stats);
}
