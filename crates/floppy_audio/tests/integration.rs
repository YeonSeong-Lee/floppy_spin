//! Cross-module integration tests for floppy_audio (M6-A), using only the
//! crate's public API — these exercise `Mixer` + `Tracker` + `sfx` together
//! the way a real game loop would, unlike the in-module unit tests.

use floppy_audio::{on_event, play, AudioEngine, Mixer, Sfx, SongId, Tracker, SAMPLE_RATE};
use floppy_core::hash::hash_u32s;
use floppy_core::physics::BattleEvent;
use floppy_core::vec::Vec3;

/// Render `seconds` of `song`, chunked in small blocks (so the tracker's
/// row scheduling and the mixer's per-block frequency sweeps both exercise
/// their normal multi-call code paths), with a small fixed script of
/// SFX/`BattleEvent` triggers layered in at fixed sample offsets.
fn render_scripted(song: SongId, seconds: u32) -> Vec<i16> {
    let mut mixer = Mixer::new();
    let mut tracker = Tracker::new(song);
    let total_samples = SAMPLE_RATE as usize * seconds as usize;
    let mut buf = vec![0i16; total_samples];

    const CHUNK: usize = 512;
    let mut pos = 0usize;
    while pos < total_samples {
        let len = CHUNK.min(total_samples - pos);
        tracker.advance(&mut mixer, len as u32);

        // A fixed, deterministic script of SFX/BattleEvent triggers at
        // specific sample offsets, exercising both `play` and `on_event`.
        if pos == 0 {
            play(&mut mixer, Sfx::HitLight);
        }
        if (22_050..22_050 + CHUNK).contains(&pos) {
            on_event(&mut mixer, &BattleEvent::Dash { who: 0 });
        }
        if (44_100..44_100 + CHUNK).contains(&pos) {
            play(&mut mixer, Sfx::SpecialFire);
        }
        if (66_150..66_150 + CHUNK).contains(&pos) {
            on_event(&mut mixer, &BattleEvent::RingOut { who: 1 });
        }
        if (88_200..88_200 + CHUNK).contains(&pos) {
            on_event(
                &mut mixer,
                &BattleEvent::Hit {
                    heavy: true,
                    pos: Vec3::default(),
                    speed: 4.0,
                },
            );
        }

        mixer.render(&mut buf[pos..pos + len]);
        pos += len;
    }
    buf
}

/// FNV-1a over the sample buffer, folding each i16 in as its 2 little-endian
/// bytes via `hash_u32s` (widened through `u16` first so the bit pattern —
/// not the signed value — is what's hashed, matching `hash_u32s`'s
/// documented little-endian-byte semantics).
fn hash_samples(buf: &[i16]) -> u64 {
    let words: Vec<u32> = buf.iter().map(|&s| (s as u16) as u32).collect();
    hash_u32s(&words)
}

/// Test 7 (determinism): rendering 2s of the battle theme plus the scripted
/// SFX/event sequence twice from fresh state produces byte-identical
/// buffers (via their FNV-1a hash) both times.
#[test]
fn battle_theme_render_is_deterministic() {
    let a = render_scripted(SongId::Battle, 2);
    let b = render_scripted(SongId::Battle, 2);
    assert_eq!(hash_samples(&a), hash_samples(&b));
}

#[test]
fn menu_theme_render_is_deterministic() {
    let a = render_scripted(SongId::Menu, 2);
    let b = render_scripted(SongId::Menu, 2);
    assert_eq!(hash_samples(&a), hash_samples(&b));
}

/// Test 7: menu vs battle hashes differ (sanity check that the hash isn't
/// trivially constant / the two songs are actually different).
#[test]
fn menu_and_battle_hashes_differ() {
    let menu = render_scripted(SongId::Menu, 2);
    let battle = render_scripted(SongId::Battle, 2);
    assert_ne!(hash_samples(&menu), hash_samples(&battle));
}

fn render_engine_chunks(chunk: usize) -> (Vec<i16>, Vec<(u64, u32, bool)>) {
    let total = SAMPLE_RATE as usize * 2;
    let mut pcm = vec![0i16; total];
    let mut cues = Vec::new();
    let mut engine = AudioEngine::new(SongId::Battle);
    let mut offset = 0;
    while offset < total {
        let end = (offset + chunk).min(total);
        let batch = engine.render(&mut pcm[offset..end]);
        cues.extend(batch.cues().map(|cue| (cue.sample, cue.row, cue.kick)));
        offset = end;
    }
    (pcm, cues)
}

#[test]
fn audio_engine_is_byte_identical_for_every_chunk_size() {
    let expected = render_engine_chunks(1);
    for chunk in [37usize, 512, 1024] {
        assert_eq!(render_engine_chunks(chunk), expected, "chunk size {chunk}");
    }
}

/// Test 8 (perf smoke): 5s of the battle theme with 8 SFX overlaid renders
/// much faster than realtime in `--release` on 2015-class hardware. Bound
/// is deliberately generous (0.5s wall-clock for 5s of audio, a 10x
/// realtime-or-better margin) — this is a smoke test, not a tight
/// benchmark; the measured time is printed so it's visible with
/// `cargo test --release -p floppy_audio -- --nocapture`.
#[test]
fn perf_smoke_battle_theme_5s_with_8_sfx() {
    let mut mixer = Mixer::new();
    let mut tracker = Tracker::new(SongId::Battle);
    let total_samples = SAMPLE_RATE as usize * 5;
    let mut buf = vec![0i16; total_samples];

    let sfx_positions_ms: [usize; 8] = [50, 700, 1_350, 2_000, 2_650, 3_300, 3_950, 4_600];
    let sfx_positions: Vec<usize> = sfx_positions_ms
        .iter()
        .map(|ms| ms * (SAMPLE_RATE as usize) / 1000)
        .collect();

    const CHUNK: usize = 1024;
    let start = std::time::Instant::now();
    let mut pos = 0usize;
    while pos < total_samples {
        let len = CHUNK.min(total_samples - pos);
        tracker.advance(&mut mixer, len as u32);
        for &sfx_pos in &sfx_positions {
            if (pos..pos + len).contains(&sfx_pos) {
                play(&mut mixer, Sfx::HitHeavy);
            }
        }
        mixer.render(&mut buf[pos..pos + len]);
        pos += len;
    }
    let elapsed = start.elapsed();
    println!("perf smoke: rendered 5s of battle theme + 8 SFX in {elapsed:?}");
    assert!(
        elapsed.as_secs_f64() < 0.5,
        "rendering 5s of audio took {elapsed:?}, expected well under 0.5s"
    );
}

/// Test 9 (on_event): the `BattleEvent` match in `on_event` has no wildcard
/// arm (see `sfx.rs`) — this is a compile-time property, so its mere
/// existence in this crate proves it; this test instead re-confirms the
/// runtime half (every currently-mapped event triggers >= 1 voice) through
/// the public API end-to-end, complementing the in-module unit test in
/// `sfx.rs` that checks the same thing more granularly.
#[test]
fn on_event_end_to_end_triggers_audible_voices() {
    let events = [
        BattleEvent::Hit {
            heavy: false,
            pos: Vec3::default(),
            speed: 1.0,
        },
        BattleEvent::Dash { who: 0 },
        BattleEvent::AirborneLaunch { who: 1 },
        BattleEvent::Landed {
            who: 0,
            impact: 3.0,
        },
        BattleEvent::RingOut { who: 0 },
        BattleEvent::Topple { who: 1 },
    ];
    for ev in &events {
        let mut mixer = Mixer::new();
        on_event(&mut mixer, ev);
        let mut buf = [0i16; 256];
        mixer.render(&mut buf);
        assert!(
            buf.iter().any(|&s| s != 0),
            "{ev:?} produced a silent buffer"
        );
    }
}
