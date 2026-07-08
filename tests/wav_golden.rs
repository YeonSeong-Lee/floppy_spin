//! Root-package headless-WAV golden test (Task M6-B / SPEC §8, §12):
//! independently re-implements `src/bin/headless.rs`'s `--wav` scripted-
//! battle audio render (a binary can't be `use`d from an integration test,
//! so this construction is intentionally duplicated — see `tests/goldens.rs`'s
//! matching header comment for the same convention applied to golden PNGs).
//!
//! Two checks: (1) rendering the first 120 frames' audio twice in-process
//! from fresh state is byte-identical (SPEC §5 determinism, extended to
//! audio); (2) the FNV-1a64 hash of that render matches a PINNED constant
//! computed once below. Regenerate the pinned constant with:
//! `cargo run --release --bin headless -- --wav <path> --frames 120`
//! (prints the same hash) if an intentional audio change moves it.

use floppy_audio::{on_event, play, Mixer, Sfx, SongId, Tracker, SAMPLE_RATE};
use floppy_core::combat::SpecialId;
use floppy_core::input::InputState;
use floppy_core::physics::{LaunchParams, World, TUNE};
use floppy_core::roster::{Preset, Silhouette, PRESETS};

const FRAMES: u32 = 120;
/// Mono samples per 60 Hz frame at `SAMPLE_RATE` (44_100/60 = 735 exactly).
const SAMPLES_PER_FRAME: u32 = SAMPLE_RATE / 60;

/// Pinned FNV-1a64 hash of [`render_wav_frames(FRAMES)`]'s sample buffer,
/// computed once via `cargo run --release --bin headless -- --wav out.wav
/// --frames 120` (see module docs) — regenerate only on an intentional
/// mix/tuning change.
const PINNED_HASH: u64 = 0xc1ef_467d_01d9_d6c4;

fn preset_by_silhouette(s: Silhouette) -> &'static Preset {
    PRESETS
        .iter()
        .find(|p| p.silhouette == s)
        .expect("every Silhouette has exactly one preset")
}

/// `World::launch` seeded 42, two Keystone-stats presets at headings 0/pi,
/// depth .7/.7, power .6/.55, quality 1.0/1.08 — must match
/// `src/bin/headless.rs`'s `build_battle_world` exactly (same seed/script as
/// `--scene battle`, per the milestone brief).
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

/// Deterministic function of the rendered-frame index only, matching
/// `src/bin/headless.rs`'s `battle_scripted_inputs` exactly.
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

/// Render `frames` worth (60 Hz cadence) of the scripted battle's audio to a
/// flat mono `i16` buffer — mirrors `src/bin/headless.rs`'s
/// `render_wav_frames` exactly: 2 sim steps @120 Hz per 60 Hz frame with
/// events drained after EACH step, a steady per-frame spin-hum retrigger off
/// P1's top, then the Battle tracker advanced by exactly
/// `SAMPLES_PER_FRAME` samples.
fn render_wav_frames(frames: u32) -> Vec<i16> {
    let mut world = build_battle_world();
    let mut mixer = Mixer::new();
    let mut tracker = Tracker::new(SongId::Battle);

    let mut samples = Vec::with_capacity(SAMPLES_PER_FRAME as usize * frames as usize);
    let mut chunk = vec![0i16; SAMPLES_PER_FRAME as usize];

    for t in 0..frames {
        let inputs = battle_scripted_inputs(t);
        for _ in 0..2 {
            world.step(inputs);
            for ev in &world.events {
                on_event(&mut mixer, ev);
            }
        }

        let rpm_frac = world.tops[0].spin / TUNE.spin_max;
        play(&mut mixer, Sfx::SpinHum { rpm_frac });

        tracker.advance(&mut mixer, SAMPLES_PER_FRAME);
        mixer.render(&mut chunk);
        samples.extend_from_slice(&chunk);
    }

    samples
}

/// FNV-1a64 over the sample buffer's little-endian byte representation
/// (each `i16` widened through `u16` first, matching `floppy_audio`'s own
/// integration-test convention).
fn hash_samples(samples: &[i16]) -> u64 {
    let words: Vec<u32> = samples.iter().map(|&s| (s as u16) as u32).collect();
    floppy_core::hash::hash_u32s(&words)
}

#[test]
fn scripted_battle_audio_is_byte_identical_across_runs() {
    let a = render_wav_frames(FRAMES);
    let b = render_wav_frames(FRAMES);
    assert_eq!(
        a, b,
        "rendering the same scripted battle's audio twice must be byte-identical (SPEC §5)"
    );
}

#[test]
fn scripted_battle_audio_hash_matches_pinned_constant() {
    let samples = render_wav_frames(FRAMES);
    // Sanity: the exact sample count `--wav` would write (SPEC-literal size
    // check `44 + SAMPLES_PER_FRAME*FRAMES*2` bytes for the WAV file).
    assert_eq!(samples.len(), SAMPLES_PER_FRAME as usize * FRAMES as usize);

    let hash = hash_samples(&samples);
    assert_eq!(
        hash, PINNED_HASH,
        "scripted-battle audio hash changed from the pinned constant \
         (0x{hash:016x} != 0x{PINNED_HASH:016x}) — if this is an intentional \
         mix/tuning change, regenerate via `cargo run --release --bin \
         headless -- --wav <path> --frames 120` and update PINNED_HASH"
    );
}
