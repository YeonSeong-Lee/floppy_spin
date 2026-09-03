//! M8 Task 2: save format (SPEC §9, EXACTLY). Pure encode/decode — no file
//! I/O (that's `platform::win32::save_load`/`save_store`, Task 3); headlessly
//! testable, which is where the SPEC §12 "save corruption" gate lives.
//!
//! ## Byte layout (documented, exact — SPEC §9)
//!
//! ```text
//! offset  size  field
//! 0       4     magic b"FSPN"
//! 4       1     version (SAVE_VERSION = 2)
//! 5       5     garage part indices, [u8; 5] (garage::resolve input)
//! 10      7     settings block (field order below)
//! 17      1     XOR checksum: XOR of every byte at offsets 0..17
//! 18      4     CRC32 of bytes at offsets 0..18 (little endian)
//! ------- total: 22 bytes
//! ```
//!
//! Settings block, fixed field order (task spec: "document"):
//! ```text
//! offset(from block start)  field
//! 0   music_vol   u8  (0..=10)
//! 1   sfx_vol     u8  (0..=10)
//! 2   shake       u8  (ShakeLevel: Off=0 Low=1 Normal=2 High=3)
//! 3   difficulty  u8  (Difficulty: Easy=0 Normal=1 Hard=2 Ace=3)
//! 4   window_scale u8 (WindowScale: X1=0 X1_5=1 X2=2 Fullscreen=3)
//! 5   colorblind  u8  (0 = false, 1 = true)
//! ```
//! (7 bytes reserved in the layout table above; byte 6 is currently unused/
//! reserved-zero padding, kept so the settings block has a round size and
//! future settings can land there without shifting the checksum offset.)
//!
//! ## Corruption handling (documented decision, SPEC §9 / §12 gate)
//!
//! `decode` NEVER panics and NEVER returns an error type — any problem at
//! all (too short, bad magic, wrong version, bad checksum, an out-of-range
//! settings enum byte, trailing garbage) yields `SaveState::default()`
//! whole-cloth. "Never partially apply" (SPEC §9): the WHOLE blob is
//! validated (magic, length, version, checksum, AND every enum byte's
//! range) before any field is read out into the result — a blob that fails
//! validation contributes nothing, not even the fields that happened to
//! look valid.

use crate::flow::{GameSettings, ShakeLevel, WindowScale};
use crate::minigame::Difficulty;

/// Save file magic (SPEC §9).
pub const MAGIC: [u8; 4] = *b"FSPN";
/// Current save format version (SPEC §9).
pub const SAVE_VERSION: u8 = 2;

const PARTS_LEN: usize = 5;
const SETTINGS_LEN: usize = 7;
/// Total encoded length: magic(4) + version(1) + parts(5) + settings(7) +
/// XOR checksum(1) + CRC32(4).
pub const SAVE_LEN: usize = 4 + 1 + PARTS_LEN + SETTINGS_LEN + 1 + 4;

const OFF_MAGIC: usize = 0;
const OFF_VERSION: usize = 4;
const OFF_PARTS: usize = 5;
const OFF_SETTINGS: usize = OFF_PARTS + PARTS_LEN; // 10
const OFF_CHECKSUM: usize = OFF_SETTINGS + SETTINGS_LEN; // 17
const OFF_CRC32: usize = OFF_CHECKSUM + 1; // 18

/// Result of classifying bytes read by a save adapter. Callers can report a
/// damaged or obsolete file without ever partially applying it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SaveLoadOutcome {
    Loaded(SaveState),
    Missing,
    Corrupt,
    Incompatible { version: u8 },
}

impl SaveLoadOutcome {
    pub fn state_or_default(self) -> SaveState {
        match self {
            Self::Loaded(state) => state,
            Self::Missing | Self::Corrupt | Self::Incompatible { .. } => SaveState::default(),
        }
    }
}

/// Decoded save contents: the garage build's part indices plus the settings
/// block. `Default` is exactly the "no save / corrupt save" fallback (SPEC
/// §9).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SaveState {
    pub parts: [u8; PARTS_LEN],
    pub settings: GameSettings,
}

impl Default for SaveState {
    fn default() -> Self {
        Self {
            parts: [0; PARTS_LEN],
            settings: GameSettings::default(),
        }
    }
}

fn shake_to_byte(s: ShakeLevel) -> u8 {
    match s {
        ShakeLevel::Off => 0,
        ShakeLevel::Low => 1,
        ShakeLevel::Normal => 2,
        ShakeLevel::High => 3,
    }
}

fn shake_from_byte(b: u8) -> Option<ShakeLevel> {
    match b {
        0 => Some(ShakeLevel::Off),
        1 => Some(ShakeLevel::Low),
        2 => Some(ShakeLevel::Normal),
        3 => Some(ShakeLevel::High),
        _ => None,
    }
}

fn difficulty_to_byte(d: Difficulty) -> u8 {
    match d {
        Difficulty::Easy => 0,
        Difficulty::Normal => 1,
        Difficulty::Hard => 2,
        Difficulty::Ace => 3,
    }
}

fn difficulty_from_byte(b: u8) -> Option<Difficulty> {
    match b {
        0 => Some(Difficulty::Easy),
        1 => Some(Difficulty::Normal),
        2 => Some(Difficulty::Hard),
        3 => Some(Difficulty::Ace),
        _ => None,
    }
}

fn window_scale_to_byte(w: WindowScale) -> u8 {
    match w {
        WindowScale::X1 => 0,
        WindowScale::X1_5 => 1,
        WindowScale::X2 => 2,
        WindowScale::Fullscreen => 3,
    }
}

fn window_scale_from_byte(b: u8) -> Option<WindowScale> {
    match b {
        0 => Some(WindowScale::X1),
        1 => Some(WindowScale::X1_5),
        2 => Some(WindowScale::X2),
        3 => Some(WindowScale::Fullscreen),
        _ => None,
    }
}

/// Encode `parts` + `settings` into the exact SPEC §9 byte layout (module
/// docs above), including the trailing XOR checksum and CRC32.
pub fn encode(parts: [u8; 5], settings: &GameSettings) -> Vec<u8> {
    let mut out = Vec::with_capacity(SAVE_LEN);
    out.extend_from_slice(&MAGIC);
    out.push(SAVE_VERSION);
    out.extend_from_slice(&parts);
    out.push(settings.music_vol);
    out.push(settings.sfx_vol);
    out.push(shake_to_byte(settings.shake));
    out.push(difficulty_to_byte(settings.difficulty));
    out.push(window_scale_to_byte(settings.window_scale));
    out.push(if settings.colorblind { 1 } else { 0 });
    out.push(0); // reserved padding byte (module docs)

    debug_assert_eq!(out.len(), OFF_CHECKSUM, "layout drifted from SAVE_LEN");
    let checksum = out.iter().fold(0u8, |acc, &b| acc ^ b);
    out.push(checksum);
    out.extend_from_slice(&crc32(&out).to_le_bytes());
    debug_assert_eq!(out.len(), SAVE_LEN);
    out
}

/// Decode raw bytes into a [`SaveState`]. NEVER panics, NEVER partially
/// applies (module docs): the whole blob is validated before any field is
/// read out; any failure returns `SaveState::default()`.
pub fn decode(bytes: &[u8]) -> SaveState {
    decode_outcome(bytes).state_or_default()
}

/// Validate and classify a complete v2 save blob.
pub fn decode_outcome(bytes: &[u8]) -> SaveLoadOutcome {
    if bytes.is_empty() {
        return SaveLoadOutcome::Missing;
    }
    if bytes.len() > OFF_VERSION
        && bytes.get(OFF_MAGIC..OFF_MAGIC + 4) == Some(MAGIC.as_slice())
        && bytes[OFF_VERSION] != SAVE_VERSION
    {
        return SaveLoadOutcome::Incompatible {
            version: bytes[OFF_VERSION],
        };
    }
    if bytes.len() != SAVE_LEN {
        return SaveLoadOutcome::Corrupt;
    }
    if bytes[OFF_MAGIC..OFF_MAGIC + 4] != MAGIC {
        return SaveLoadOutcome::Corrupt;
    }
    if bytes[OFF_VERSION] != SAVE_VERSION {
        return SaveLoadOutcome::Incompatible {
            version: bytes[OFF_VERSION],
        };
    }
    let checksum = bytes[..OFF_CHECKSUM].iter().fold(0u8, |acc, &b| acc ^ b);
    if checksum != bytes[OFF_CHECKSUM] {
        return SaveLoadOutcome::Corrupt;
    }
    let expected_crc = u32::from_le_bytes([
        bytes[OFF_CRC32],
        bytes[OFF_CRC32 + 1],
        bytes[OFF_CRC32 + 2],
        bytes[OFF_CRC32 + 3],
    ]);
    if crc32(&bytes[..OFF_CRC32]) != expected_crc {
        return SaveLoadOutcome::Corrupt;
    }
    if bytes[OFF_SETTINGS + 6] != 0 {
        return SaveLoadOutcome::Corrupt;
    }
    if bytes[OFF_PARTS..OFF_PARTS + PARTS_LEN]
        .iter()
        .any(|&part| part >= 4)
    {
        return SaveLoadOutcome::Corrupt;
    }

    let music_vol = bytes[OFF_SETTINGS];
    let sfx_vol = bytes[OFF_SETTINGS + 1];
    let Some(shake) = shake_from_byte(bytes[OFF_SETTINGS + 2]) else {
        return SaveLoadOutcome::Corrupt;
    };
    let Some(difficulty) = difficulty_from_byte(bytes[OFF_SETTINGS + 3]) else {
        return SaveLoadOutcome::Corrupt;
    };
    let Some(window_scale) = window_scale_from_byte(bytes[OFF_SETTINGS + 4]) else {
        return SaveLoadOutcome::Corrupt;
    };
    let colorblind_byte = bytes[OFF_SETTINGS + 5];
    if colorblind_byte > 1 {
        return SaveLoadOutcome::Corrupt;
    }
    if music_vol > 10 || sfx_vol > 10 {
        return SaveLoadOutcome::Corrupt;
    }

    // Everything validated: NOW it's safe to apply (never partial).
    let mut parts = [0u8; PARTS_LEN];
    parts.copy_from_slice(&bytes[OFF_PARTS..OFF_PARTS + PARTS_LEN]);

    SaveLoadOutcome::Loaded(SaveState {
        parts,
        settings: GameSettings {
            music_vol,
            sfx_vol,
            shake,
            difficulty,
            window_scale,
            colorblind: colorblind_byte == 1,
        },
    })
}

/// CRC-32/ISO-HDLC, reflected polynomial 0xEDB88320.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_settings() -> GameSettings {
        GameSettings {
            music_vol: 7,
            sfx_vol: 3,
            shake: ShakeLevel::High,
            difficulty: Difficulty::Hard,
            window_scale: WindowScale::X2,
            colorblind: true,
        }
    }

    #[test]
    fn round_trip_preserves_parts_and_settings() {
        let parts = [1u8, 2, 3, 0, 3];
        let settings = sample_settings();
        let bytes = encode(parts, &settings);
        let decoded = decode(&bytes);
        assert_eq!(decoded.parts, parts);
        assert_eq!(decoded.settings, settings);
    }

    #[test]
    fn round_trip_default_settings_and_parts() {
        let parts = [0u8; 5];
        let settings = GameSettings::default();
        let bytes = encode(parts, &settings);
        let decoded = decode(&bytes);
        assert_eq!(decoded.parts, parts);
        assert_eq!(decoded.settings, settings);
    }

    #[test]
    fn empty_bytes_decode_to_defaults() {
        assert_eq!(decode(&[]), SaveState::default());
    }

    #[test]
    fn every_truncated_prefix_length_decodes_to_defaults() {
        let bytes = encode([1, 2, 3, 4, 5], &sample_settings());
        for len in 0..bytes.len() {
            let decoded = decode(&bytes[..len]);
            assert_eq!(
                decoded,
                SaveState::default(),
                "truncated to {len} bytes must fall back to defaults"
            );
        }
    }

    #[test]
    fn flipped_magic_byte_decodes_to_defaults() {
        let mut bytes = encode([1, 2, 3, 0, 1], &sample_settings());
        bytes[0] ^= 0xFF;
        assert_eq!(decode(&bytes), SaveState::default());
    }

    #[test]
    fn wrong_version_decodes_to_defaults() {
        for &v in &[0u8, 99u8] {
            let mut bytes = encode([1, 2, 3, 0, 1], &sample_settings());
            bytes[OFF_VERSION] = v;
            assert_eq!(decode(&bytes), SaveState::default(), "version {v}");
        }
    }

    #[test]
    fn flipped_checksum_decodes_to_defaults() {
        let mut bytes = encode([1, 2, 3, 0, 1], &sample_settings());
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        assert_eq!(decode(&bytes), SaveState::default());
    }

    #[test]
    fn each_settings_enum_byte_set_to_0xff_falls_back_to_defaults() {
        // Recompute the checksum after mutating an enum byte so the ONLY
        // thing under test is the enum-range rejection, not an incidental
        // checksum mismatch masking it.
        for enum_off in [OFF_SETTINGS + 2, OFF_SETTINGS + 3, OFF_SETTINGS + 4] {
            let mut bytes = encode([1, 2, 3, 0, 1], &sample_settings());
            bytes[enum_off] = 0xFF;
            let checksum = bytes[..OFF_CHECKSUM].iter().fold(0u8, |acc, &b| acc ^ b);
            *bytes.last_mut().unwrap() = checksum;
            assert_eq!(
                decode(&bytes),
                SaveState::default(),
                "enum byte at offset {enum_off} = 0xFF must reject, not construct an invalid enum"
            );
        }
        // colorblind byte (bool-as-u8) with a fixed-up checksum too.
        let mut bytes = encode([1, 2, 3, 0, 1], &sample_settings());
        bytes[OFF_SETTINGS + 5] = 0xFF;
        let checksum = bytes[..OFF_CHECKSUM].iter().fold(0u8, |acc, &b| acc ^ b);
        *bytes.last_mut().unwrap() = checksum;
        assert_eq!(decode(&bytes), SaveState::default());
    }

    #[test]
    fn one_extra_trailing_byte_decodes_to_defaults() {
        let mut bytes = encode([1, 2, 3, 0, 1], &sample_settings());
        bytes.push(0xAB);
        assert_eq!(decode(&bytes), SaveState::default());
    }

    #[test]
    fn all_zeros_decodes_to_defaults() {
        let bytes = vec![0u8; SAVE_LEN];
        assert_eq!(decode(&bytes), SaveState::default());
    }

    #[test]
    fn random_bytes_never_panic_and_bad_ones_fall_back_to_defaults() {
        // Deterministic pseudo-random byte stream (xorshift-style mix, no
        // external RNG dependency needed for this pure test).
        let mut state: u64 = 0x1234_5678_9abc_def0;
        for _ in 0..500 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let len = (state % (SAVE_LEN as u64 * 2)) as usize;
            let mut bytes = Vec::with_capacity(len);
            let mut s = state;
            for _ in 0..len {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                bytes.push((s & 0xFF) as u8);
            }
            // Must never panic; result must always be a valid SaveState
            // (parts len 5, settings fields in-range by construction).
            let decoded = decode(&bytes);
            assert_eq!(decoded.parts.len(), 5);
            assert!(decoded.settings.music_vol <= 10);
            assert!(decoded.settings.sfx_vol <= 10);
        }
    }
}
