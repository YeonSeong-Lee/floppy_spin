//! Root-package golden-frame integration test (SPEC §12.2 / M3-B task spec):
//! independently builds the same 3 staged `World` states as
//! `src/bin/headless.rs`'s `--golden write`/`--golden check` (a binary can't
//! be `use`d from an integration test, so this construction is intentionally
//! duplicated — see that file's matching functions), renders them, and
//! compares against the checked-in `goldens/*.png` through the SAME
//! `floppy_io::golden::compare` / tolerance rule `--golden check` uses. Run
//! `cargo run --bin headless -- --golden write` first to (re)generate the
//! checked-in PNGs after an intentional visual change.

use floppy_core::arena;
use floppy_core::combat::CombatState;
use floppy_core::physics::{Stats, Top, World, TUNE};
use floppy_core::rng::Rng;
use floppy_core::roster::{Preset, Silhouette, PRESETS};
use floppy_core::vec::{Vec2, Vec3};
use floppy_render::battle::BattleScene;
use floppy_render::frame::Frame;
use floppy_render::particles::ParticlePool;
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

fn golden_rest_world(cleaver: &Preset, bulwark: &Preset) -> World {
    let x = 2.5f32;
    let top0 = top_at(
        Vec3::new(-x, arena::height(-x, 0.0), 0.0),
        Vec3::default(),
        TUNE.spin_max,
        cleaver.spin_dir,
        Vec2::default(),
        true,
        0,
        cleaver.stats,
    );
    let top1 = top_at(
        Vec3::new(x, arena::height(x, 0.0), 0.0),
        Vec3::default(),
        TUNE.spin_max,
        bulwark.spin_dir,
        Vec2::default(),
        true,
        0,
        bulwark.stats,
    );
    world_of([top0, top1], 1)
}

fn golden_clash_world(cleaver: &Preset, bulwark: &Preset) -> World {
    let top0 = top_at(
        Vec3::new(-0.9, arena::height(-0.9, 0.0), 0.0),
        Vec3::new(3.0, 0.0, 0.0),
        TUNE.spin_max * 0.8,
        cleaver.spin_dir,
        Vec2::new(0.15, 0.0),
        true,
        6,
        cleaver.stats,
    );
    let top1 = top_at(
        Vec3::new(0.9, arena::height(0.9, 0.0), 0.0),
        Vec3::new(-3.0, 0.0, 0.0),
        TUNE.spin_max * 0.8,
        bulwark.spin_dir,
        Vec2::new(0.2, 0.0),
        true,
        0,
        bulwark.stats,
    );
    world_of([top0, top1], 2)
}

fn golden_air_world(cleaver: &Preset, bulwark: &Preset) -> World {
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

const GOLDEN_NAMES: [&str; 3] = ["golden_rest.png", "golden_clash.png", "golden_air.png"];

fn staged_goldens() -> [World; 3] {
    let cleaver = preset_by_silhouette(Silhouette::Cleaver);
    let bulwark = preset_by_silhouette(Silhouette::Bulwark);
    [
        golden_rest_world(cleaver, bulwark),
        golden_clash_world(cleaver, bulwark),
        golden_air_world(cleaver, bulwark),
    ]
}

fn staged_visuals() -> [&'static Preset; 2] {
    [
        preset_by_silhouette(Silhouette::Cleaver),
        preset_by_silhouette(Silhouette::Bulwark),
    ]
}

#[test]
fn staged_golden_renders_match_checked_in_pngs() {
    let scene = BattleScene::new();
    let visuals = staged_visuals();
    let dir = PathBuf::from("goldens");
    let empty_particles = ParticlePool::new();

    let mut failures = Vec::new();

    for (name, world) in GOLDEN_NAMES.iter().zip(staged_goldens()) {
        let mut frame = Frame::new(WIDTH, HEIGHT);
        // M7: routed through the bloom/dither/scanline/vignette post
        // pipeline (see `src/bin/headless.rs`'s `render_staged` docs for why
        // — this MUST stay byte-for-byte the same call shape, since both
        // sites are compared against the exact same checked-in PNGs).
        // `ring_pulse = 0.0` / `shake = (0.0, 0.0)` / no particles / no
        // flash: the documented OFF/zero state for goldens.
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
        post.composite(&mut frame, Vec3::default());

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

        let report = floppy_io::golden::compare(&frame.px, &golden_px);
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
