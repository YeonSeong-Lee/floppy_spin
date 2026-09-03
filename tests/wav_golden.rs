//! Golden verification through the real headless runtime adapter.

use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

const FRAMES: u32 = 120;
const SAMPLES_PER_FRAME: usize = floppy_audio::SAMPLE_RATE as usize / 60;
const PINNED_HASH: u64 = 0xb6ac_3f70_db4f_3851;
static NEXT_FILE: AtomicUsize = AtomicUsize::new(0);

fn render_wav() -> Vec<u8> {
    let path = std::env::temp_dir().join(format!(
        "floppy-spin-wav-golden-{}-{}.wav",
        std::process::id(),
        NEXT_FILE.fetch_add(1, Ordering::Relaxed)
    ));
    let frames = FRAMES.to_string();
    let status = Command::new(env!("CARGO_BIN_EXE_headless"))
        .args([
            "--frames",
            &frames,
            "--wav",
            path.to_str().expect("temporary path is utf-8"),
        ])
        .status()
        .expect("headless binary starts");
    assert!(status.success());
    let bytes = fs::read(&path).expect("headless wrote wav");
    let _ = fs::remove_file(path);
    bytes
}

fn pcm_hash(wav: &[u8]) -> u64 {
    assert_eq!(&wav[..4], b"RIFF");
    assert_eq!(&wav[8..12], b"WAVE");
    assert_eq!(wav.len(), 44 + SAMPLES_PER_FRAME * FRAMES as usize * 2);
    let samples: Vec<u32> = wav[44..]
        .chunks_exact(2)
        .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]) as u32)
        .collect();
    floppy_core::hash::hash_u32s(&samples)
}

#[test]
fn scripted_battle_audio_is_byte_identical_across_runs() {
    assert_eq!(render_wav(), render_wav());
}

#[test]
fn scripted_battle_audio_hash_matches_pinned_constant() {
    assert_eq!(pcm_hash(&render_wav()), PINNED_HASH);
}
