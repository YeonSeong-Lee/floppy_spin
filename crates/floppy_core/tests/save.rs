//! M8 Task 2 integration test (SPEC §9 / §12 "save corruption" gate):
//! black-box coverage of `save::encode`/`decode` through the PUBLIC API,
//! plus the exact byte-layout contract documented in `save.rs`'s module
//! docs (magic/version/parts/settings/checksum offsets) so a future layout
//! change is forced to update this test deliberately rather than silently
//! drifting. Per-corruption-mode unit tests already live inline in
//! `save.rs`; this file checks the wire format and end-to-end
//! `flow::FlowState` round-trip (`save_snapshot` -> `encode` -> `decode` ->
//! `apply_save`).

use floppy_core::flow::{FlowState, GameSettings, ShakeLevel, WindowScale};
use floppy_core::minigame::Difficulty;
use floppy_core::save::{self, SaveLoadOutcome, SaveState, SAVE_LEN};

#[test]
fn save_len_matches_the_documented_v2_layout() {
    // magic(4) + version(1) + parts(5) + settings(7) + xor(1) + crc32(4).
    assert_eq!(SAVE_LEN, 22);
    let bytes = save::encode([0; 5], &GameSettings::default());
    assert_eq!(bytes.len(), SAVE_LEN);
}

#[test]
fn encoded_bytes_match_the_documented_offsets_exactly() {
    let parts = [1u8, 2, 3, 0, 0];
    let settings = GameSettings {
        music_vol: 5,
        sfx_vol: 6,
        shake: ShakeLevel::Low,
        difficulty: Difficulty::Ace,
        window_scale: WindowScale::X1_5,
        colorblind: true,
    };
    let bytes = save::encode(parts, &settings);

    assert_eq!(&bytes[0..4], b"FSPN", "magic must be at offset 0");
    assert_eq!(bytes[4], save::SAVE_VERSION, "version must be at offset 4");
    assert_eq!(&bytes[5..10], &parts, "parts must be at offset 5, 5 bytes");
    assert_eq!(bytes[10], 5, "music_vol at settings offset 0");
    assert_eq!(bytes[11], 6, "sfx_vol at settings offset 1");
    assert_eq!(bytes[12], 1, "shake byte (Low=1) at settings offset 2");
    assert_eq!(bytes[13], 3, "difficulty byte (Ace=3) at settings offset 3");
    assert_eq!(
        bytes[14], 1,
        "window_scale byte (X1_5=1) at settings offset 4"
    );
    assert_eq!(
        bytes[15], 1,
        "colorblind byte (true=1) at settings offset 5"
    );
    assert_eq!(bytes[16], 0, "reserved padding byte must be 0");
    let checksum = bytes[..17].iter().fold(0u8, |acc, &b| acc ^ b);
    assert_eq!(bytes[17], checksum, "checksum must be XOR of bytes 0..17");
    assert_ne!(&bytes[18..22], &[0; 4], "CRC32 follows the XOR byte");
}

#[test]
fn v1_is_classified_as_incompatible() {
    let mut old = vec![0u8; 18];
    old[..4].copy_from_slice(b"FSPN");
    old[4] = 1;
    assert_eq!(
        save::decode_outcome(&old),
        SaveLoadOutcome::Incompatible { version: 1 }
    );
}

#[test]
fn invalid_part_and_reserved_byte_are_corrupt() {
    for offset in [5usize, 16] {
        let mut bytes = save::encode([0; 5], &GameSettings::default());
        bytes[offset] = 4;
        assert_eq!(save::decode_outcome(&bytes), SaveLoadOutcome::Corrupt);
    }
}

#[test]
fn flow_state_save_snapshot_round_trips_through_encode_decode_apply() {
    let mut flow = FlowState::new(42);
    flow.parts = [3, 1, 0, 2, 3];
    flow.settings.music_vol = 2;
    flow.settings.sfx_vol = 9;
    flow.settings.shake = ShakeLevel::High;
    flow.settings.difficulty = Difficulty::Hard;
    flow.settings.window_scale = WindowScale::Fullscreen;
    flow.settings.colorblind = true;

    let (parts, settings) = flow.save_snapshot();
    let bytes = save::encode(parts, &settings);
    let decoded = save::decode(&bytes);

    let mut restored = FlowState::new(999); // different seed: proves apply_save overwrites
    restored.apply_save(decoded);
    assert_eq!(restored.parts, flow.parts);
    assert_eq!(restored.settings, flow.settings);
}

#[test]
fn a_save_from_an_unrelated_random_blob_never_panics_and_is_a_valid_default_or_real_state() {
    // Simulates "some other program's 18 bytes happened to land in our save
    // slot" — decode must treat it as opaque bytes, not assume any
    // structure, and either reject it (defaults) or, in the astronomically
    // unlikely case the magic/version/checksum/enum-range all coincidentally
    // line up, produce a perfectly valid SaveState (never a crash either way).
    let candidates: [&[u8]; 4] = [
        b"NOTASAVEFILE1234\x00\x00",
        b"\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00",
        b"FSPN\x01\x00\x00\x00\x00\x00\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF",
        b"FSPNFSPNFSPNFSPNFS",
    ];
    for c in candidates {
        let decoded = save::decode(c);
        assert!(decoded.parts.len() == 5);
        assert!(decoded.settings.music_vol <= 10);
        assert!(decoded.settings.sfx_vol <= 10);
    }
}

#[test]
fn decode_default_equals_save_state_default() {
    assert_eq!(save::decode(&[]), SaveState::default());
    assert_eq!(SaveState::default().parts, [0u8; 5]);
    assert_eq!(SaveState::default().settings, GameSettings::default());
}
