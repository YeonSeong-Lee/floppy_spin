//! Headless verification bin: sim → PNG/WAV without a display (SPEC C6).
use std::env;
use std::fs;
use std::path::PathBuf;

use floppy_core::arena;
use floppy_core::combat::CombatState;
use floppy_core::input::InputState;
use floppy_core::minigame::{MinigameState, Stage};
use floppy_core::physics::{BattleEvent, Stats, Top, World, TUNE};
use floppy_core::rng::Rng;
use floppy_core::roster::{Preset, Silhouette, PRESETS};
use floppy_core::vec::{Vec2, Vec3};
use floppy_render::battle::BattleScene;
use floppy_render::hud;
use floppy_render::particles::{self, ParticlePool};
use floppy_render::post::PostState;
use floppy_spin::{AppRuntime, PlaybackCursor, RuntimeConfig};

const WIDTH: usize = 960;
const HEIGHT: usize = 540;

enum SceneKind {
    Gradient,
    Test3d,
    Battle,
}

enum GoldenMode {
    None,
    Write,
    Check,
}

#[allow(clippy::too_many_arguments)]
fn parse_args() -> (u32, PathBuf, SceneKind, GoldenMode, Option<PathBuf>, bool) {
    let mut frames: u32 = 3;
    let mut out = PathBuf::from("out");
    let mut scene = SceneKind::Gradient;
    let mut golden = GoldenMode::None;
    let mut wav: Option<PathBuf> = None;
    let mut benchmark = false;

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--frames" => match args.next() {
                Some(v) => match v.parse::<u32>() {
                    Ok(n) => frames = n,
                    Err(_) => {
                        eprintln!("invalid --frames value: {v}");
                        std::process::exit(1);
                    }
                },
                None => {
                    eprintln!("--frames requires a value");
                    std::process::exit(1);
                }
            },
            "--out" => match args.next() {
                Some(v) => out = PathBuf::from(v),
                None => {
                    eprintln!("--out requires a value");
                    std::process::exit(1);
                }
            },
            "--scene" => match args.next().as_deref() {
                Some("gradient") => scene = SceneKind::Gradient,
                Some("test3d") => scene = SceneKind::Test3d,
                Some("battle") => scene = SceneKind::Battle,
                other => {
                    eprintln!("--scene must be gradient|test3d|battle, got {other:?}");
                    std::process::exit(1);
                }
            },
            "--golden" => match args.next().as_deref() {
                Some("write") => golden = GoldenMode::Write,
                Some("check") => golden = GoldenMode::Check,
                other => {
                    eprintln!("--golden must be write|check, got {other:?}");
                    std::process::exit(1);
                }
            },
            "--wav" => match args.next() {
                Some(v) => wav = Some(PathBuf::from(v)),
                None => {
                    eprintln!("--wav requires a value");
                    std::process::exit(1);
                }
            },
            "--benchmark" => benchmark = true,
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(1);
            }
        }
    }

    (frames, out, scene, golden, wav, benchmark)
}

/// Deterministic 960x540 test pattern. This formula must match byte-for-byte
/// the copy embedded in the game's own rendering path (SPEC C5, C6) — it is
/// intentionally duplicated rather than shared across the platform boundary
/// this crate is not allowed to touch.
fn test_pixel(x: usize, y: usize, t: u32) -> u32 {
    let r = ((x * 255) / 959) as u32;
    let g = ((y * 255) / 539) as u32;
    let b = ((x ^ (y + t as usize)) & 0xFF) as u32;
    (r << 16) | (g << 8) | b
}

// ---------------------------------------------------------------------------
// `--scene battle`: a scripted fight (M3-B task spec).
// ---------------------------------------------------------------------------

fn preset_by_silhouette(s: Silhouette) -> &'static Preset {
    PRESETS
        .iter()
        .find(|p| p.silhouette == s)
        .expect("every Silhouette has exactly one preset")
}

/// `World::launch` seeded 42, two Keystone-stats presets at headings 0/pi
/// (task spec), depth .7/.7, power .6/.55, quality 1.0/1.08.
/// Deterministic function of the rendered-frame index only (task spec:
/// "simple chase script", never RNG-derived): both tops steer toward each
/// other for a 30-frame block, then hold neutral, alternating.
fn battle_scripted_inputs(frame: u32) -> [InputState; 2] {
    let chase = (frame / 30).is_multiple_of(2);
    if chase {
        [
            InputState {
                dir_x: -1,
                ..Default::default()
            },
            InputState {
                dir_x: 1,
                ..Default::default()
            },
        ]
    } else {
        [InputState::default(), InputState::default()]
    }
}

fn tap_dash() -> floppy_core::input::FrameInput {
    floppy_core::input::FrameInput {
        pressed: InputState {
            dash: true,
            ..InputState::default()
        },
        released: InputState {
            dash: true,
            ..InputState::default()
        },
        focused: true,
        ..floppy_core::input::FrameInput::default()
    }
}

/// Enter a real fight exclusively through the shared production runtime.
fn build_battle_runtime() -> AppRuntime {
    let mut runtime = AppRuntime::new(
        RuntimeConfig { seed: 42 },
        floppy_core::save::SaveLoadOutcome::Missing,
    )
    .unwrap();
    let neutral = floppy_core::input::FrameInput {
        focused: true,
        ..floppy_core::input::FrameInput::default()
    };
    for _ in 0..=floppy_core::flow::BOOT_FRAMES {
        runtime.advance(neutral, PlaybackCursor(0));
    }
    runtime.advance(tap_dash(), PlaybackCursor(0)); // Title -> MainMenu
    runtime.advance(tap_dash(), PlaybackCursor(0)); // MainMenu -> TopSelect
    runtime.advance(tap_dash(), PlaybackCursor(0)); // TopSelect -> Intro
    for _ in 0..=floppy_core::flow::INTRO_TOTAL_FRAMES {
        runtime.advance(neutral, PlaybackCursor(0));
    }
    for _ in 0..3 {
        runtime.advance(tap_dash(), PlaybackCursor(0));
    }
    assert!(matches!(
        runtime.render(1.0).state.screen,
        floppy_core::flow::Screen::Match(floppy_core::flow::MatchPhase::Fight)
    ));
    runtime
}

// ---------------------------------------------------------------------------
// Staged golden frames (task spec): directly-constructed `World` states,
// deterministic, always rendered at alpha = 1.0. Both `--golden write`/
// `--golden check` here and the root-package `tests/goldens.rs` integration
// test build these same 3 states independently and compare through the
// shared `floppy_io::golden::compare` (the one thing that MUST stay
// identical between the two call sites).
// ---------------------------------------------------------------------------

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

/// `golden_midfight.png`: both tops mid-arena, both grounded and moving at
/// speed on a converging course, tilted (readable silhouettes, not just
/// upright cylinders), Cleaver vs Bulwark for maximum silhouette contrast.
/// Wider separation than the old `golden_rest` pose (task spec: "the money
/// shot — make it read well: distinct silhouettes, some tilt, rings
/// visible") so both tops sit clear of the center hole and each reads as its
/// own distinct shape against the neon rings.
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

/// `golden_airclash.png`: adapted from the former `golden_air.png` staged
/// pose — one top airborne mid-clash (shadow blob visible), the other
/// grounded on the wall slope — kept because it is exactly the "airborne
/// clash" SPEC §12 asks for.
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

/// `golden_ringout.png`: top 0 already well past `arena::RING_OUT_RADIUS`
/// (9.6 m) and clear of the rim height at that radius (out over open space,
/// the ring-out moment itself), top 1 still spinning safely inside the bowl.
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

const GOLDEN_NAMES: [&str; 6] = [
    "golden_title.png",
    "golden_launch.png",
    "golden_midfight.png",
    "golden_airclash.png",
    "golden_ringout.png",
    "golden_result.png",
];

/// `ui_frame` pinned for `golden_title.png` (module docs / task spec):
/// `(30/36)%2 == 0` puts "PRESS ANY KEY" in its "on" blink phase, and the
/// idle-top orbit angle `30 * 0.06 = 1.8` rad is well off either axis so the
/// orbiting-top-with-trail effect reads clearly (not caught at `angle == 0`
/// where the trail would visually stack on the leader).
const TITLE_UI_FRAME: u32 = 30;

/// Pinned `power_phase` for `golden_launch.png`'s `Stage::Power` marker
/// (module docs / task spec): `power_period` at `Difficulty::Normal` is used
/// (the default `MinigameState::new` value), and this phase is chosen so
/// `triangle_value(phase, period)` lands inside the PERFECT band
/// (`PERFECT_CENTER +- PERFECT_HALF_WIDTH` = 83..89) — see
/// `build_launch_minigame`'s assertion, which pins the exact percentage this
/// documents.
const LAUNCH_POWER_PHASE_FRAC: f32 = 0.86;

/// Deterministic staged `MinigameState` for `golden_launch.png`: forced into
/// `Stage::Power` (bypassing `Aim`/`SpinDir` input-driven transitions, which
/// would otherwise need a scripted step sequence for a single frame) with a
/// pinned heading/depth/spin_dir and `power_phase` set directly so the sweep
/// marker sits at a fixed, documented, PERFECT-band position.
fn build_launch_minigame(spin_dir: i8) -> MinigameState {
    let mut mg = MinigameState::new(spin_dir);
    mg.stage = Stage::Power;
    mg.heading = std::f32::consts::FRAC_PI_4;
    mg.depth = 0.7;
    mg.power_phase = mg.power_period * LAUNCH_POWER_PHASE_FRAC;
    mg
}

/// `golden_launch.png`'s 3D backdrop: both tops grounded, mid-approach
/// (matches the Launch phase's cosmetic-preview backdrop in `main.rs`, which
/// draws the live launch world under the minigame overlay).
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

/// `golden_result.png` is `hud::draw_round_result` over the void background
/// (no 3D backdrop — matches nothing in `main.rs`'s render match arm
/// directly since RoundResult draws over the live battle backdrop there, but
/// SPEC §12 lists `result` as its own named golden distinct from the
/// mid-fight shot, so this pins the HUD-only content deterministically: a
/// cleared background + the tally screen at a fixed frame past the last
/// pip's landing).
const RESULT_UI_FRAME: u32 = 40;
const RESULT_ROUND: u32 = 1;
const RESULT_SCORE: [u8; 2] = [3, 1];
const RESULT_LAST_WINNER: Option<u8> = Some(0);
const RESULT_LAST_POINTS: u8 = 3;

fn staged_visuals() -> [&'static Preset; 2] {
    [
        preset_by_silhouette(Silhouette::Cleaver),
        preset_by_silhouette(Silhouette::Bulwark),
    ]
}

/// Deterministic particle-rng seed shared by every golden that spawns a
/// burst (module docs: goldens must be a pure function of nothing but their
/// own constants, never wall-clock/never a shared mutable seed that could
/// drift with unrelated code changes elsewhere in this file).
const GOLDEN_PARTICLE_SEED: u64 = 0x0060_1DE0_0000_0001;

/// How many 60 Hz `ParticlePool::update()` ticks to run after spawning a
/// burst before rendering it (module docs): 0 would render particles at
/// their spawn instant (a single bright pinpoint); a small fixed count lets
/// the burst visibly spread/fall/fade to a representative mid-life pose
/// while staying fully deterministic (same seed + same tick count every
/// call).
const GOLDEN_PARTICLE_TICKS: u32 = 6;

/// Render the 6 SPEC §12-named ship goldens (alpha = 1.0) into raw
/// `0x00RRGGBB` pixel buffers, in [`GOLDEN_NAMES`] order. Each one is routed
/// through the SAME composite shape `main.rs` uses for its screen (clear +
/// scene/HUD + `PostState::composite`), so the checked-in goldens exercise
/// the real ship pipeline, not a simplified stand-in:
///
/// - `golden_title`: `clear(COL_BG)` -> `hud::draw_title` -> composite (no
///   flash), matching `Screen::Title`'s render arm exactly.
/// - `golden_launch`: `BattleScene::draw_ex` (mid-approach backdrop) ->
///   `hud::draw_launch_ui` -> composite, matching `MatchPhase::Launch`.
/// - `golden_midfight`: `draw_ex` -> `hud::draw_battle_hud` -> composite,
///   matching `MatchPhase::Fight`.
/// - `golden_airclash`: `draw_ex` (with a deterministic `spawn_airborne_clash`
///   burst ticked forward `GOLDEN_PARTICLE_TICKS` frames) -> battle HUD ->
///   composite.
/// - `golden_ringout`: `draw_ex` -> battle HUD + `hud::draw_banner` "RING
///   OUT!" -> composite with the same red-orange flash tint `main.rs` fires
///   on `BattleEvent::RingOut` (`RED_ORANGE * 0.30`).
/// - `golden_result`: `clear(COL_BG)` -> `hud::draw_round_result` ->
///   composite (no 3D backdrop — see that fn's doc comment).
///
/// Every EVENT-DRIVEN effect not called out above sits at its documented
/// OFF/zero state: `ring_pulse = 0.0` (headless has no `Tracker` in this
/// path), `shake = (0.0, 0.0)`, no flash unless documented above. This keeps
/// every golden a pure function of its own staged state (deterministic, no
/// wall-clock, RNG only from the fixed seeds above) while still catching
/// regressions in bloom/dither/scanline/vignette/particles themselves.
fn render_staged(scene: &BattleScene) -> [Vec<u32>; 6] {
    let visuals = staged_visuals();
    let cleaver = preset_by_silhouette(Silhouette::Cleaver);
    let bulwark = preset_by_silhouette(Silhouette::Bulwark);

    // -- golden_title --------------------------------------------------
    let title_px = {
        let mut frame = floppy_render::frame::Frame::new(WIDTH, HEIGHT);
        let mut post = PostState::new(WIDTH, HEIGHT);
        frame.clear(hud::COL_BG);
        hud::draw_title(&mut frame, TITLE_UI_FRAME);
        post.composite(&mut frame, Vec3::default());
        frame.px
    };

    // -- golden_launch ----------------------------------------------------
    let launch_px = {
        let world = golden_launch_world(cleaver, bulwark);
        let mg = build_launch_minigame(cleaver.spin_dir);
        let empty_particles = ParticlePool::new();
        let mut frame = floppy_render::frame::Frame::new(WIDTH, HEIGHT);
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

    // -- golden_midfight --------------------------------------------------
    let midfight_px = {
        let mut world = golden_midfight_world(cleaver, bulwark);
        world.tops[0].meter = 100.0; // Armed glow on P1's panel.
        let empty_particles = ParticlePool::new();
        let mut frame = floppy_render::frame::Frame::new(WIDTH, HEIGHT);
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

    // -- golden_airclash ----------------------------------------------------
    let airclash_px = {
        let world = golden_airclash_world(cleaver, bulwark);
        let mut particle_rng = Rng::new(GOLDEN_PARTICLE_SEED);
        let mut particles = ParticlePool::new();
        particles::spawn_airborne_clash(&mut particles, &mut particle_rng, world.tops[0].pos);
        for _ in 0..GOLDEN_PARTICLE_TICKS {
            particles.update();
        }
        let mut frame = floppy_render::frame::Frame::new(WIDTH, HEIGHT);
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

    // -- golden_ringout ----------------------------------------------------
    let ringout_px = {
        let world = golden_ringout_world(cleaver, bulwark);
        let mut particle_rng = Rng::new(GOLDEN_PARTICLE_SEED ^ 1);
        let mut particles = ParticlePool::new();
        particles::spawn_ring_out(&mut particles, &mut particle_rng, world.tops[0].pos);
        for _ in 0..GOLDEN_PARTICLE_TICKS {
            particles.update();
        }
        let mut frame = floppy_render::frame::Frame::new(WIDTH, HEIGHT);
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
        // Same red-orange flash tint `main.rs` fires on `BattleEvent::RingOut`
        // (`particles::RED_ORANGE * 0.30` — see that match arm's docs).
        post.composite(&mut frame, particles::RED_ORANGE * 0.30);
        frame.px
    };

    // -- golden_result ----------------------------------------------------
    let result_px = {
        let mut frame = floppy_render::frame::Frame::new(WIDTH, HEIGHT);
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

fn golden_dir() -> PathBuf {
    PathBuf::from("goldens")
}

fn run_golden_write() {
    let scene = BattleScene::new();
    let rendered = render_staged(&scene);
    let dir = golden_dir();
    if let Err(e) = fs::create_dir_all(&dir) {
        eprintln!("failed to create {}: {e}", dir.display());
        std::process::exit(1);
    }
    for (name, px) in GOLDEN_NAMES.iter().zip(rendered.iter()) {
        let png = floppy_io::png::encode_rgb(WIDTH as u32, HEIGHT as u32, px);
        let path = dir.join(name);
        if let Err(e) = fs::write(&path, &png) {
            eprintln!("failed to write {}: {e}", path.display());
            std::process::exit(1);
        }
        println!("wrote {}", path.display());
    }
}

fn run_golden_check() {
    let scene = BattleScene::new();
    let rendered = render_staged(&scene);
    let dir = golden_dir();
    let mut all_pass = true;

    for (name, px) in GOLDEN_NAMES.iter().zip(rendered.iter()) {
        let path = dir.join(name);
        let bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                println!("{name}: FAIL (cannot read {}: {e})", path.display());
                all_pass = false;
                continue;
            }
        };
        let (w, h, golden_px) = match floppy_io::png::decode_rgb(&bytes) {
            Ok(v) => v,
            Err(e) => {
                println!("{name}: FAIL (decode error: {e})");
                all_pass = false;
                continue;
            }
        };
        if w as usize != WIDTH || h as usize != HEIGHT {
            println!(
                "{name}: FAIL (dimension mismatch: golden is {w}x{h}, expected {WIDTH}x{HEIGHT})"
            );
            all_pass = false;
            continue;
        }
        let report = floppy_io::golden::compare(px, &golden_px);
        println!(
            "{name}: {} mean_abs_diff=({:.3},{:.3},{:.3}) over_threshold_fraction={:.4}",
            if report.pass { "PASS" } else { "FAIL" },
            report.mean_abs_diff[0],
            report.mean_abs_diff[1],
            report.mean_abs_diff[2],
            report.over_threshold_fraction
        );
        if !report.pass {
            all_pass = false;
        }
    }

    if !all_pass {
        println!("GOLDEN CHECK: FAIL");
        std::process::exit(1);
    }
    println!("GOLDEN CHECK: PASS");
}

// ---------------------------------------------------------------------------
// `--wav out.wav --frames N`: headless audio verification (Task M6-B / SPEC
// §8, §12). Same scripted battle (world + scripted inputs) as `--scene
// battle`, but stepped WITHOUT rendering any video, feeding
// `floppy_core::physics::BattleEvent`s and a steady spin-hum retrigger to
// the audio core each 60 Hz frame, exactly like `main.rs`'s real-time
// wiring. Both of the 2 per-frame 120 Hz sim steps get their events drained
// here directly; `main.rs` gets the same completeness via
// `flow::FlowState::frame_events`, which accumulates across sub-steps (see
// its doc comment in flow.rs).
// ---------------------------------------------------------------------------

/// Mono samples per 60 Hz frame at `floppy_audio::SAMPLE_RATE` (44_100/60 =
/// 735 exactly).
const WAV_SAMPLES_PER_FRAME: u32 = floppy_audio::SAMPLE_RATE / 60;

/// Render `frames` worth (60 Hz cadence) of the scripted battle's audio to a
/// flat mono `i16` buffer: a pure function of `frames` alone (fresh World +
/// Mixer + Tracker every call, scripted inputs are a pure function of the
/// frame index) — this is exactly what makes the golden-hash test's
/// twice-in-a-row determinism check meaningful.
fn render_wav_frames(frames: u32) -> Vec<i16> {
    let mut runtime = build_battle_runtime();

    let mut samples = Vec::with_capacity(WAV_SAMPLES_PER_FRAME as usize * frames as usize);
    let mut chunk = vec![0i16; WAV_SAMPLES_PER_FRAME as usize];

    for t in 0..frames {
        let input = battle_scripted_inputs(t)[0];
        runtime.advance(
            floppy_core::input::FrameInput::from_held(input, false),
            PlaybackCursor(t as u64 * WAV_SAMPLES_PER_FRAME as u64),
        );
        runtime.render_audio(&mut chunk);
        samples.extend_from_slice(&chunk);
    }

    samples
}

/// FNV-1a64 over the sample buffer's little-endian byte representation, via
/// `floppy_core::hash::hash_u32s` widening each `i16` through `u16` first (so
/// the bit pattern, not the signed value, is what's hashed) — the same
/// convention `floppy_audio`'s own integration tests use.
fn hash_samples(samples: &[i16]) -> u64 {
    let words: Vec<u32> = samples.iter().map(|&s| (s as u16) as u32).collect();
    floppy_core::hash::hash_u32s(&words)
}

fn run_wav(frames: u32, path: &PathBuf) {
    let samples = render_wav_frames(frames);
    let wav_bytes = floppy_io::wav::encode_mono16(floppy_audio::SAMPLE_RATE, &samples);

    if let Err(e) = fs::write(path, &wav_bytes) {
        eprintln!("failed to write {}: {e}", path.display());
        std::process::exit(1);
    }

    let hash = hash_samples(&samples);
    println!(
        "wrote {} ({} bytes, {} samples) hash=0x{hash:016x}",
        path.display(),
        wav_bytes.len(),
        samples.len()
    );
}

fn main() {
    let (frames, out_dir, scene, golden, wav, benchmark) = parse_args();

    if let Some(path) = wav {
        run_wav(frames, &path);
        return;
    }

    match golden {
        GoldenMode::Write => {
            run_golden_write();
            return;
        }
        GoldenMode::Check => {
            run_golden_check();
            return;
        }
        GoldenMode::None => {}
    }

    if let Err(e) = fs::create_dir_all(&out_dir) {
        eprintln!(
            "failed to create output directory {}: {e}",
            out_dir.display()
        );
        std::process::exit(1);
    }

    let mut frame3d = floppy_render::frame::Frame::new(WIDTH, HEIGHT);

    // Only built when actually needed (SPEC §10: don't pay for the battle
    // scene's mesh construction on the gradient/test3d paths).
    let mut battle_runtime: Option<AppRuntime> = None;
    let mut battle_scene: Option<BattleScene> = None;
    // M7: the real bloom/particle pipeline, exercised for perf realism (see
    // `print_perf_summary`'s docs) — NOT wired to the full main.rs juice
    // table (that lives in `src/main.rs`'s `Vfx`/`handle_battle_event`); a
    // headless perf/video path has no flow/screen-transition state to hang
    // Round-win/Match-win/etc. off of, and the SPEC §10 budget line this
    // exists to measure ("Particles/trails/bloom") is dominated by
    // per-particle draw cost, not which juice-table row spawned them — so
    // only `Hit` (the highest-frequency event in a real fight) is wired
    // here, giving a representative particle load without duplicating
    // main.rs's whole event-to-VFX table.
    let mut post_state: Option<PostState> = None;
    let mut particle_pool: Option<ParticlePool> = None;
    let mut particle_rng = Rng::new(0x00F7_0000_0000_002A);
    // `ring_pulse`/`shake` are the documented deterministic-zero/constant
    // fallback (module docs on `render_staged` above): headless has no
    // `Tracker` in this video path, so there is no row index to derive a
    // pulse from cheaply, and no persistent shake-decay state worth
    // threading through a throwaway perf/golden harness.
    const NO_SHAKE: (f32, f32) = (0.0, 0.0);
    if matches!(scene, SceneKind::Battle) {
        battle_runtime = Some(build_battle_runtime());
        battle_scene = Some(BattleScene::new());
        post_state = Some(PostState::new(WIDTH, HEIGHT));
        particle_pool = Some(ParticlePool::new());
    }

    // Coarse wall-clock perf sanity (task spec §5 "perf sanity"): timed here
    // in `main`, around the call sites only — never inside `World::step`,
    // `BattleScene::draw_ex`, or `PostState::composite` themselves, so no
    // wall-clock ever enters the deterministic sim/render path. Battle-scene
    // only; reported once at the end, not fed back into anything. `draw_ms`
    // is the scene+particle raster pass (SPEC §10's "Arena + tops raster" +
    // half of "Particles/trails/bloom"); `post_ms` is the OTHER half (the
    // bloom blur/composite pass) — split out so a budget overrun is
    // attributable to the right SPEC §10 line.
    let mut sim_ms: Vec<f64> = Vec::new();
    let mut draw_ms: Vec<f64> = Vec::new();
    let mut post_ms: Vec<f64> = Vec::new();
    // A 720-frame production benchmark is interpreted as 120 warm-up frames
    // followed by the requested 600-frame measurement window. Short visual
    // and golden runs still report every frame.
    let perf_warmup = if frames >= 720 { 120 } else { 0 };

    for t in 0..frames {
        let framebuffer: &[u32] = match scene {
            SceneKind::Gradient => {
                let px = frame3d.px.as_mut_slice();
                for y in 0..HEIGHT {
                    for x in 0..WIDTH {
                        px[y * WIDTH + x] = test_pixel(x, y, t);
                    }
                }
                &frame3d.px
            }
            SceneKind::Test3d => {
                // Fixed 20 ms per frame: headless time is a function of the
                // frame index, never the wall clock (SPEC §5).
                floppy_render::scene::draw_test_scene(&mut frame3d, t as f32 * 0.02);
                &frame3d.px
            }
            SceneKind::Battle => {
                let runtime = battle_runtime.as_mut().expect("battle runtime initialized");
                let scene_ref = battle_scene.as_ref().expect("battle_scene initialized");
                let post = post_state.as_mut().expect("post_state initialized");
                let particles = particle_pool.as_mut().expect("particle_pool initialized");
                // 2 sim steps per rendered frame (task spec), scripted
                // inputs as a pure function of the frame index. Events
                // drained after EACH step (not just the pair), matching
                // `render_wav_frames`'s own pattern below — `World::step`
                // clears `events` at the top of every call.
                let inputs = battle_scripted_inputs(t);
                let sim_start = std::time::Instant::now();
                let effects = runtime.advance(
                    floppy_core::input::FrameInput::from_held(inputs[0], false),
                    PlaybackCursor(t as u64 * WAV_SAMPLES_PER_FRAME as u64),
                );
                for event in effects.events.iter() {
                    if let floppy_spin::PresentationEvent::Battle(BattleEvent::Hit {
                        heavy,
                        pos,
                        ..
                    }) = event
                    {
                        if heavy {
                            particles::spawn_heavy_hit(particles, &mut particle_rng, pos);
                        } else {
                            particles::spawn_light_hit(particles, &mut particle_rng, pos);
                        }
                    }
                }
                let sim_elapsed = sim_start.elapsed().as_secs_f64() * 1000.0;
                particles.update();

                let view = runtime.render(1.0);
                let state = view.state;
                let p1 = state.preset_view(state.p1_pick);
                let ai = state.preset_view(state.ai_pick);
                let visuals = [&p1, &ai];
                let world = state.world.as_ref().expect("fight world initialized");
                let previous = state.world_prev.as_ref().unwrap_or(world);
                let draw_start = std::time::Instant::now();
                scene_ref.draw_ex(
                    &mut frame3d,
                    post,
                    previous,
                    world,
                    1.0,
                    visuals,
                    0.0,
                    NO_SHAKE,
                    particles,
                );
                let draw_elapsed = draw_start.elapsed().as_secs_f64() * 1000.0;

                let post_start = std::time::Instant::now();
                post.composite(&mut frame3d, Vec3::default());
                let post_elapsed = post_start.elapsed().as_secs_f64() * 1000.0;
                if t >= perf_warmup {
                    sim_ms.push(sim_elapsed);
                    draw_ms.push(draw_elapsed);
                    post_ms.push(post_elapsed);
                }
                &frame3d.px
            }
        };

        if !benchmark {
            let png_bytes = floppy_io::png::encode_rgb(WIDTH as u32, HEIGHT as u32, framebuffer);
            let path = out_dir.join(format!("frame_{t:03}.png"));
            if let Err(e) = fs::write(&path, &png_bytes) {
                eprintln!("failed to write {}: {e}", path.display());
                std::process::exit(1);
            }

            let hash = floppy_core::hash::hash_u32s(framebuffer);
            println!("frame {t:03} hash=0x{hash:016x}");
        }
    }

    if matches!(scene, SceneKind::Battle) && !sim_ms.is_empty() {
        print_perf_summary(&sim_ms, &draw_ms, &post_ms);
    }
}

fn mean(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len() as f64
}

fn max(v: &[f64]) -> f64 {
    v.iter().copied().fold(f64::MIN, f64::max)
}

fn percentile(mut values: Vec<f64>, percentile: f64) -> f64 {
    values.sort_by(f64::total_cmp);
    let index = ((values.len() - 1) as f64 * percentile).ceil() as usize;
    values[index]
}

fn print_perf_summary(sim_ms: &[f64], draw_ms: &[f64], post_ms: &[f64]) {
    let sim_mean = mean(sim_ms);
    let sim_max = max(sim_ms);
    let draw_mean = mean(draw_ms);
    let draw_max = max(draw_ms);
    let post_mean = mean(post_ms);
    let post_max = max(post_ms);
    let total_mean = sim_mean + draw_mean + post_mean;
    let total_max = sim_max + draw_max + post_max;
    let totals: Vec<f64> = sim_ms
        .iter()
        .zip(draw_ms)
        .zip(post_ms)
        .map(|((&sim, &draw), &post)| sim + draw + post)
        .collect();
    let total_p95 = percentile(totals.clone(), 0.95);
    let total_p99 = percentile(totals, 0.99);
    println!(
        "perf: sim mean={sim_mean:.3}ms max={sim_max:.3}ms | draw(scene+particles) mean={draw_mean:.3}ms max={draw_max:.3}ms | post(bloom composite) mean={post_mean:.3}ms max={post_max:.3}ms | total mean={total_mean:.3}ms p95={total_p95:.3}ms p99={total_p99:.3}ms max={total_max:.3}ms (budgets: p95 <= 10ms, p99 <= 16.67ms release)"
    );
    if total_p95 > 10.0 || total_p99 > 16.67 {
        println!(
            "PERF WARNING: percentile budget exceeded (p95={total_p95:.3}ms, p99={total_p99:.3}ms)"
        );
    }
}
