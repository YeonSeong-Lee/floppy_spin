//! Root-package golden-frame integration test (SPEC §12.2 / M9-A task spec):
//! independently builds the same 6 SPEC §12-named staged states as
//! `src/bin/headless.rs`'s `--golden write`/`--golden check` (a binary can't
//! be `use`d from an integration test, so this construction is intentionally
//! duplicated — see that file's matching functions), renders them through the
//! same clear/scene→HUD→composite shape `main.rs` uses per screen, and
//! compares against the checked-in `goldens/*.png` through the SAME
//! `floppy_io::golden::compare` / tolerance rule `--golden check` uses. Run
//! `cargo run --bin headless -- --golden write` first to (re)generate the
//! checked-in PNGs after an intentional visual change.

use floppy_core::arena;
use floppy_core::combat::CombatState;
use floppy_core::minigame::{MinigameState, Stage};
use floppy_core::physics::{Stats, Top, World, TUNE};
use floppy_core::rng::Rng;
use floppy_core::roster::{Preset, Silhouette, PRESETS};
use floppy_core::vec::{Vec2, Vec3};
use floppy_render::battle::BattleScene;
use floppy_render::frame::Frame;
use floppy_render::hud;
use floppy_render::particles::{self, ParticlePool};
use floppy_render::post::PostState;
use std::path::PathBuf;

const WIDTH: usize = 960;
const HEIGHT: usize = 540;

fn preset_by_silhouette(s: Silhouette) -> &'static Preset {
    PRESETS
        .iter()
        .find(|p| p.silhouette == s)
        .expect("every Silhouette has exactly one preset")
}

#[allow(clippy::too_many_arguments)]
fn top_at(
    pos: Vec3,
    vel: Vec3,
    spin: f32,
    spin_dir: i8,
    tilt: Vec2,
    grounded: bool,
    dash_active: u16,
    stats: Stats,
) -> Top {
    Top {
        pos,
        vel,
        spin,
        spin_dir,
        tilt,
        tilt_phase: 0.0,
        spin_angle: 0.0,
        radius: 0.95,
        height: 1.0,
        stats,
        grounded,
        dash_cd: 0,
        dash_active,
        meter: 0.0,
        airdash_used: false,
        combat: CombatState::default(),
    }
}

fn world_of(tops: [Top; 2], seed: u64) -> World {
    World {
        tops,
        rng: Rng::new(seed),
        step: 0,
        hitstop: 0,
        outcome: None,
        events: Vec::new(),
    }
}

/// `golden_midfight.png`'s backdrop: both tops mid-arena, moving, tilted.
/// Mirrors `src/bin/headless.rs::golden_midfight_world` exactly.
fn golden_midfight_world(cleaver: &Preset, bulwark: &Preset) -> World {
    let x = 4.2f32;
    let top0 = top_at(
        Vec3::new(-x, arena::height(-x, 0.0), 1.0),
        Vec3::new(3.0, 0.0, -0.6),
        TUNE.spin_max * 0.85,
        cleaver.spin_dir,
        Vec2::new(0.16, 0.05),
        true,
        0,
        cleaver.stats,
    );
    let top1 = top_at(
        Vec3::new(x, arena::height(x, 0.0), -1.0),
        Vec3::new(-3.0, 0.0, 0.6),
        TUNE.spin_max * 0.35,
        bulwark.spin_dir,
        Vec2::new(0.10, -0.06),
        true,
        0,
        bulwark.stats,
    );
    world_of([top0, top1], 10)
}

/// `golden_airclash.png`'s backdrop (former `golden_air.png` staged pose):
/// one top airborne mid-clash (shadow blob visible), the other grounded on
/// the wall slope.
fn golden_airclash_world(cleaver: &Preset, bulwark: &Preset) -> World {
    let top0 = top_at(
        Vec3::new(0.0, 3.0, 0.0),
        Vec3::new(0.0, -2.0, 0.0),
        TUNE.spin_max * 0.7,
        cleaver.spin_dir,
        Vec2::new(0.3, 0.0),
        false,
        0,
        cleaver.stats,
    );
    let r = 6.0f32;
    let top1 = top_at(
        Vec3::new(r, arena::height(r, 0.0), 0.0),
        Vec3::default(),
        TUNE.spin_max,
        bulwark.spin_dir,
        Vec2::default(),
        true,
        0,
        bulwark.stats,
    );
    world_of([top0, top1], 3)
}

/// `golden_ringout.png`'s backdrop: top 0 past `arena::RING_OUT_RADIUS`,
/// clear of the rim height there; top 1 still spinning inside the bowl.
fn golden_ringout_world(cleaver: &Preset, bulwark: &Preset) -> World {
    let r_out = 11.0f32;
    let clear_y = arena::height(r_out, 0.0) + 1.5;
    let top0 = top_at(
        Vec3::new(r_out, clear_y, 0.0),
        Vec3::new(4.0, -1.0, 0.0),
        TUNE.spin_max * 0.5,
        cleaver.spin_dir,
        Vec2::new(0.25, 0.1),
        false,
        0,
        cleaver.stats,
    );
    let top1 = top_at(
        Vec3::new(-1.0, arena::height(-1.0, 0.0), 0.0),
        Vec3::default(),
        TUNE.spin_max * 0.9,
        bulwark.spin_dir,
        Vec2::default(),
        true,
        0,
        bulwark.stats,
    );
    world_of([top0, top1], 4)
}

/// `golden_launch.png`'s backdrop: both tops grounded, mid-approach.
fn golden_launch_world(cleaver: &Preset, bulwark: &Preset) -> World {
    let top0 = top_at(
        Vec3::new(-4.0, arena::height(-4.0, 0.0), 0.0),
        Vec3::new(1.5, 0.0, 0.0),
        TUNE.spin_max,
        cleaver.spin_dir,
        Vec2::default(),
        true,
        0,
        cleaver.stats,
    );
    let top1 = top_at(
        Vec3::new(4.0, arena::height(4.0, 0.0), 0.0),
        Vec3::new(-1.5, 0.0, 0.0),
        TUNE.spin_max,
        bulwark.spin_dir,
        Vec2::default(),
        true,
        0,
        bulwark.stats,
    );
    world_of([top0, top1], 5)
}

/// Same pinned values as `src/bin/headless.rs` — see that file's constants'
/// doc comments for why each was chosen.
const TITLE_UI_FRAME: u32 = 30;
const LAUNCH_POWER_PHASE_FRAC: f32 = 0.86;
const RESULT_UI_FRAME: u32 = 40;
const RESULT_ROUND: u32 = 1;
const RESULT_SCORE: [u8; 2] = [3, 1];
const RESULT_LAST_WINNER: Option<u8> = Some(0);
const RESULT_LAST_POINTS: u8 = 3;
const GOLDEN_PARTICLE_SEED: u64 = 0x0060_1DE0_0000_0001;
const GOLDEN_PARTICLE_TICKS: u32 = 6;

fn build_launch_minigame(spin_dir: i8) -> MinigameState {
    let mut mg = MinigameState::new(spin_dir);
    mg.stage = Stage::Power;
    mg.heading = std::f32::consts::FRAC_PI_4;
    mg.depth = 0.7;
    mg.power_phase = mg.power_period * LAUNCH_POWER_PHASE_FRAC;
    mg
}

const GOLDEN_NAMES: [&str; 6] = [
    "golden_title.png",
    "golden_launch.png",
    "golden_midfight.png",
    "golden_airclash.png",
    "golden_ringout.png",
    "golden_result.png",
];

fn staged_visuals() -> [&'static Preset; 2] {
    [
        preset_by_silhouette(Silhouette::Cleaver),
        preset_by_silhouette(Silhouette::Bulwark),
    ]
}

/// Renders the 6 named goldens through the exact same per-screen composite
/// shape as `src/bin/headless.rs::render_staged` (see that fn's doc comment
/// for the full per-golden pipeline description) — this MUST stay in lock
/// step with that function since both sites are compared against the exact
/// same checked-in PNGs.
fn render_staged(scene: &BattleScene) -> [Vec<u32>; 6] {
    let visuals = staged_visuals();
    let cleaver = preset_by_silhouette(Silhouette::Cleaver);
    let bulwark = preset_by_silhouette(Silhouette::Bulwark);

    let title_px = {
        let mut frame = Frame::new(WIDTH, HEIGHT);
        let mut post = PostState::new(WIDTH, HEIGHT);
        frame.clear(hud::COL_BG);
        hud::draw_title(&mut frame, TITLE_UI_FRAME);
        post.composite(&mut frame, Vec3::default());
        frame.px
    };

    let launch_px = {
        let world = golden_launch_world(cleaver, bulwark);
        let mg = build_launch_minigame(cleaver.spin_dir);
        let empty_particles = ParticlePool::new();
        let mut frame = Frame::new(WIDTH, HEIGHT);
        let mut post = PostState::new(WIDTH, HEIGHT);
        scene.draw_ex(
            &mut frame,
            &mut post,
            &world,
            &world,
            1.0,
            visuals,
            0.0,
            (0.0, 0.0),
            &empty_particles,
        );
        hud::draw_launch_ui(&mut frame, &mg, bulwark.spin_dir, TITLE_UI_FRAME);
        post.composite(&mut frame, Vec3::default());
        frame.px
    };

    let midfight_px = {
        let mut world = golden_midfight_world(cleaver, bulwark);
        world.tops[0].meter = 100.0;
        let empty_particles = ParticlePool::new();
        let mut frame = Frame::new(WIDTH, HEIGHT);
        let mut post = PostState::new(WIDTH, HEIGHT);
        scene.draw_ex(
            &mut frame,
            &mut post,
            &world,
            &world,
            1.0,
            visuals,
            0.0,
            (0.0, 0.0),
            &empty_particles,
        );
        hud::draw_battle_hud(&mut frame, &world, visuals, [2, 1], TITLE_UI_FRAME, false);
        post.composite(&mut frame, Vec3::default());
        frame.px
    };

    let airclash_px = {
        let world = golden_airclash_world(cleaver, bulwark);
        let mut particle_rng = Rng::new(GOLDEN_PARTICLE_SEED);
        let mut particles = ParticlePool::new();
        particles::spawn_airborne_clash(&mut particles, &mut particle_rng, world.tops[0].pos);
        for _ in 0..GOLDEN_PARTICLE_TICKS {
            particles.update();
        }
        let mut frame = Frame::new(WIDTH, HEIGHT);
        let mut post = PostState::new(WIDTH, HEIGHT);
        scene.draw_ex(
            &mut frame,
            &mut post,
            &world,
            &world,
            1.0,
            visuals,
            0.0,
            (0.0, 0.0),
            &particles,
        );
        hud::draw_battle_hud(&mut frame, &world, visuals, [1, 1], TITLE_UI_FRAME, false);
        post.composite(&mut frame, Vec3::default());
        frame.px
    };

    let ringout_px = {
        let world = golden_ringout_world(cleaver, bulwark);
        let mut particle_rng = Rng::new(GOLDEN_PARTICLE_SEED ^ 1);
        let mut particles = ParticlePool::new();
        particles::spawn_ring_out(&mut particles, &mut particle_rng, world.tops[0].pos);
        for _ in 0..GOLDEN_PARTICLE_TICKS {
            particles.update();
        }
        let mut frame = Frame::new(WIDTH, HEIGHT);
        let mut post = PostState::new(WIDTH, HEIGHT);
        scene.draw_ex(
            &mut frame,
            &mut post,
            &world,
            &world,
            1.0,
            visuals,
            0.0,
            (0.0, 0.0),
            &particles,
        );
        hud::draw_battle_hud(&mut frame, &world, visuals, [1, 2], TITLE_UI_FRAME, false);
        hud::draw_banner(&mut frame, "RING OUT!", 6, hud::COL_ICE);
        post.composite(&mut frame, particles::RED_ORANGE * 0.30);
        frame.px
    };

    let result_px = {
        let mut frame = Frame::new(WIDTH, HEIGHT);
        let mut post = PostState::new(WIDTH, HEIGHT);
        frame.clear(hud::COL_BG);
        hud::draw_round_result(
            &mut frame,
            RESULT_ROUND,
            RESULT_SCORE,
            [visuals[0].accent, visuals[1].accent],
            "RING OUT!",
            RESULT_UI_FRAME,
            RESULT_LAST_WINNER,
            RESULT_LAST_POINTS,
            false,
        );
        post.composite(&mut frame, Vec3::default());
        frame.px
    };

    [
        title_px,
        launch_px,
        midfight_px,
        airclash_px,
        ringout_px,
        result_px,
    ]
}

#[test]
fn staged_golden_renders_match_checked_in_pngs() {
    let scene = BattleScene::new();
    let dir = PathBuf::from("goldens");
    let rendered = render_staged(&scene);

    let mut failures = Vec::new();

    for (name, px) in GOLDEN_NAMES.iter().zip(rendered.iter()) {
        let path = dir.join(name);
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                failures.push(format!(
                    "{name}: cannot read {} ({e}) — run `cargo run --bin headless -- --golden write` first",
                    path.display()
                ));
                continue;
            }
        };
        let (w, h, golden_px) = match floppy_io::png::decode_rgb(&bytes) {
            Ok(v) => v,
            Err(e) => {
                failures.push(format!("{name}: PNG decode error: {e}"));
                continue;
            }
        };
        if w as usize != WIDTH || h as usize != HEIGHT {
            failures.push(format!(
                "{name}: dimension mismatch, golden is {w}x{h}, expected {WIDTH}x{HEIGHT}"
            ));
            continue;
        }

        let report = floppy_io::golden::compare(px, &golden_px);
        if !report.pass {
            failures.push(format!(
                "{name}: tolerance FAIL mean_abs_diff={:?} over_threshold_fraction={}",
                report.mean_abs_diff, report.over_threshold_fraction
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "golden mismatch(es):\n{}",
        failures.join("\n")
    );
}
