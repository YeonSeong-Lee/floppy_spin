//! Headless verification bin: sim → PNG/WAV without a display (SPEC C6).
use std::env;
use std::fs;
use std::path::PathBuf;

use floppy_core::arena;
use floppy_core::combat::{CombatState, SpecialId};
use floppy_core::input::InputState;
use floppy_core::physics::{BattleEvent, LaunchParams, Stats, Top, World, TUNE};
use floppy_core::rng::Rng;
use floppy_core::roster::{Preset, Silhouette, PRESETS};
use floppy_core::vec::{Vec2, Vec3};
use floppy_render::battle::BattleScene;
use floppy_render::particles::{self, ParticlePool};
use floppy_render::post::PostState;

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
fn parse_args() -> (u32, PathBuf, SceneKind, GoldenMode, Option<PathBuf>) {
    let mut frames: u32 = 3;
    let mut out = PathBuf::from("out");
    let mut scene = SceneKind::Gradient;
    let mut golden = GoldenMode::None;
    let mut wav: Option<PathBuf> = None;

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
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(1);
            }
        }
    }

    (frames, out, scene, golden, wav)
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
fn build_battle_world() -> World {
    let preset = preset_by_silhouette(Silhouette::Keystone);
    let params = [
        LaunchParams {
            heading: 0.0,
            depth: 0.7,
            power: 0.6,
            quality: 1.0,
            spin_dir: preset.spin_dir,
            stats: preset.stats,
            special_id: SpecialId::from_silhouette(preset.silhouette),
        },
        LaunchParams {
            heading: std::f32::consts::PI,
            depth: 0.7,
            power: 0.55,
            quality: 1.08,
            spin_dir: preset.spin_dir,
            stats: preset.stats,
            special_id: SpecialId::from_silhouette(preset.silhouette),
        },
    ];
    World::launch(42, params)
}

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

/// `golden_rest.png`: two tops at `(+-2.5, ground, 0)`, full spin, distinct
/// silhouettes Cleaver vs Bulwark.
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

/// `golden_clash.png`: tops adjacent at center in a mid-collision pose
/// (overlapping the collision sphere sum-radius), tilt 0.15/0.2, one
/// `dash_active`.
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

/// `golden_air.png`: one top airborne at `y = 3` with tilt 0.3 (shadow blob
/// visible), the other grounded on the wall slope at `r = 6`.
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

/// Render the 3 staged goldens (alpha = 1.0) into raw `0x00RRGGBB` pixel
/// buffers, in [`GOLDEN_NAMES`] order. M7: routed through the FULL
/// bloom/dither/scanline/vignette post pipeline (`BattleScene::draw_ex` +
/// `PostState::composite`) so the checked-in goldens actually exercise it —
/// but with every EVENT-DRIVEN effect at its documented OFF/zero state:
/// `ring_pulse = 0.0` (headless has no `Tracker` in this path — see the
/// `--scene battle` docs below for the same call), `shake = (0.0, 0.0)`, an
/// empty `ParticlePool`, and no flash (`Vec3::default()`). This keeps the
/// goldens a pure function of the staged `World`s (deterministic, no RNG
/// consumed) while still catching regressions in bloom/dither/scanline/
/// vignette themselves.
fn render_staged(scene: &BattleScene) -> [Vec<u32>; 3] {
    let visuals = staged_visuals();
    let empty_particles = ParticlePool::new();
    staged_goldens().map(|world| {
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
        post.composite(&mut frame, Vec3::default());
        frame.px
    })
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
    let mut world = build_battle_world();
    let mut mixer = floppy_audio::Mixer::new();
    let mut tracker = floppy_audio::Tracker::new(floppy_audio::SongId::Battle);

    let mut samples = Vec::with_capacity(WAV_SAMPLES_PER_FRAME as usize * frames as usize);
    let mut chunk = vec![0i16; WAV_SAMPLES_PER_FRAME as usize];

    for t in 0..frames {
        let inputs = battle_scripted_inputs(t);
        // 2 sim steps @120 Hz per 60 Hz audio frame (task spec), draining
        // events after EACH step (not just the pair) so nothing is lost to
        // the next step's `World::events` clear.
        for _ in 0..2 {
            world.step(inputs);
            for ev in &world.events {
                floppy_audio::on_event(&mut mixer, ev);
            }
        }

        // Spin hum: steady per-frame retrigger/retune, P1's top (mirrors
        // main.rs's wiring).
        let rpm_frac = world.tops[0].spin / floppy_core::physics::TUNE.spin_max;
        floppy_audio::play(&mut mixer, floppy_audio::Sfx::SpinHum { rpm_frac });

        tracker.advance(&mut mixer, WAV_SAMPLES_PER_FRAME);
        mixer.render(&mut chunk);
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
    let (frames, out_dir, scene, golden, wav) = parse_args();

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
    let mut battle_world: Option<World> = None;
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
        battle_world = Some(build_battle_world());
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
                let world = battle_world.as_mut().expect("battle_world initialized");
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
                for _ in 0..2 {
                    world.step(inputs);
                    for ev in &world.events {
                        if let BattleEvent::Hit { heavy, pos, .. } = *ev {
                            if heavy {
                                particles::spawn_heavy_hit(particles, &mut particle_rng, pos);
                            } else {
                                particles::spawn_light_hit(particles, &mut particle_rng, pos);
                            }
                        }
                    }
                }
                sim_ms.push(sim_start.elapsed().as_secs_f64() * 1000.0);
                particles.update();

                let visuals = [
                    preset_by_silhouette(Silhouette::Keystone),
                    preset_by_silhouette(Silhouette::Keystone),
                ];
                let draw_start = std::time::Instant::now();
                scene_ref.draw_ex(
                    &mut frame3d,
                    post,
                    world,
                    world,
                    1.0,
                    visuals,
                    0.0,
                    NO_SHAKE,
                    particles,
                );
                draw_ms.push(draw_start.elapsed().as_secs_f64() * 1000.0);

                let post_start = std::time::Instant::now();
                post.composite(&mut frame3d, Vec3::default());
                post_ms.push(post_start.elapsed().as_secs_f64() * 1000.0);
                &frame3d.px
            }
        };

        let png_bytes = floppy_io::png::encode_rgb(WIDTH as u32, HEIGHT as u32, framebuffer);
        let path = out_dir.join(format!("frame_{t:03}.png"));
        if let Err(e) = fs::write(&path, &png_bytes) {
            eprintln!("failed to write {}: {e}", path.display());
            std::process::exit(1);
        }

        let hash = floppy_core::hash::hash_u32s(framebuffer);
        println!("frame {t:03} hash=0x{hash:016x}");
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

fn print_perf_summary(sim_ms: &[f64], draw_ms: &[f64], post_ms: &[f64]) {
    let sim_mean = mean(sim_ms);
    let sim_max = max(sim_ms);
    let draw_mean = mean(draw_ms);
    let draw_max = max(draw_ms);
    let post_mean = mean(post_ms);
    let post_max = max(post_ms);
    let total_mean = sim_mean + draw_mean + post_mean;
    let total_max = sim_max + draw_max + post_max;
    println!(
        "perf: sim mean={sim_mean:.3}ms max={sim_max:.3}ms | draw(scene+particles) mean={draw_mean:.3}ms max={draw_max:.3}ms | post(bloom composite) mean={post_mean:.3}ms max={post_max:.3}ms | total mean={total_mean:.3}ms max={total_max:.3}ms (budget: 10ms/frame release, SPEC §10)"
    );
    if total_mean > 10.0 {
        println!("PERF WARNING: mean total frame time {total_mean:.3}ms exceeds the 10ms budget!");
    }
}
