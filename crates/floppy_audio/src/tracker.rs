//! 4-channel music tracker: menu + battle themes (game_design.md §8 "Audio
//! identity", SPEC §8). Two songs, each a fixed loop of 16-row bars, driven
//! sample-accurately off a single global sample counter — no wall clock, no
//! per-frame drift (SPEC §5).
//!
//! ## Note encoding
//! Pitched channels (pulse1, pulse2, bass) use `i8` pattern arrays: each
//! entry is a semitone offset from A2 (110 Hz), or [`REST`] (`i8::MIN`,
//! `-128`) for silence. Every non-rest entry is a fresh note-on for that row
//! (no separate "hold" encoding — a repeated identical value is a deliberate
//! staccato restrike, and sustained-sounding notes are achieved by giving
//! that instrument's envelope a decay/release long enough to ring through
//! the gap until its next restrike, exactly like a real tracker relying on
//! instrument envelopes rather than explicit note-length columns).
//!
//! `note_freq` converts an offset to Hz via one hardcoded 12-tone-equal-
//! temperament ratio table (`2^(n/12)` for `n in 0..12`, computed once by
//! hand at authoring time — `powf` is both unavailable in `no_std`-style
//! spirit here and on the workspace's banned-call list, see crate docs) plus
//! repeated `*2.0`/`*0.5` octave shifts, exactly as the milestone brief
//! specifies ("octaves via *2.0 shifts").
//!
//! The drums "channel" is not pitched: its pattern values are the plain
//! `i8` event codes `REST`/`SNARE`/`HAT`. **Kick is a separate `bool` mask,
//! not one of those codes.** This is a deliberate architecture decision, not
//! an oversight: the brief calls for "four-on-floor kick" and
//! "snare rows 4/12" simultaneously, which literally coincide on the beat-2/
//! beat-4 rows — but the dedicated drums voice ([`CH_DRUMS`]) is a single
//! oscillator and can only sound one thing per row. Splitting kick out lets
//! kick and snare/hat truly overlap: kick's "occasional Square blip for kick
//! body" (module brief wording, `sfx.rs`-style Noise+Square drum recipe) is
//! fired as a one-shot borrowed from the generic SFX pool via
//! `Mixer::trigger` (still fully deterministic — same allocator, same
//! ordering rules as any other SFX), while [`CH_DRUMS`] itself only ever
//! carries the snare/hat texture. The same trick extends the drums beyond
//! a nominal "4 channels" only for percussive one-shots, which is exactly
//! what the generic pool exists for.
//!
//! ## Row scheduling
//! `samples_per_row = 60*SAMPLE_RATE/(BPM*4)` (4 rows per beat, i.e. 16th
//! notes at 16 rows/bar in 4/4 time) computed once via integer division,
//! plus a Bresenham/DDA-style remainder accumulator so the *cumulative*
//! sample count after any number of rows is always
//! `floor(60*SAMPLE_RATE*rows / (BPM*4))` **exactly** — not just within
//! ±1 — even though individual row lengths vary by at most one sample. See
//! [`Tracker::row_len_samples`].

use crate::adsr_ms;
use crate::mixer::{Mixer, VoiceParams, MUSIC_VOICE_BASE};
use crate::osc::Waveform;
use crate::SAMPLE_RATE;

/// Which song a [`Tracker`] plays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SongId {
    Menu,
    Battle,
}

const ROWS_PER_BAR: u32 = 16;

/// Rest marker for every pattern array in this module (pitched or drum).
const REST: i8 = i8::MIN;

/// Non-kick percussion event codes (see module docs on why kick is split
/// into its own `bool` mask instead of a third code here).
const SNARE: i8 = 0;
const HAT: i8 = 1;

/// Reserved tracker voice indices (see `mixer.rs`: `MUSIC_VOICE_BASE..+4`).
const CH_PULSE1: usize = MUSIC_VOICE_BASE;
const CH_PULSE2: usize = MUSIC_VOICE_BASE + 1;
const CH_BASS: usize = MUSIC_VOICE_BASE + 2;
const CH_DRUMS: usize = MUSIC_VOICE_BASE + 3;

/// 12-tone equal temperament ratios `2^(n/12)` for `n in 0..12`, hand-
/// computed constants (no `powf` — see module docs). `SEMITONE_RATIOS[0]`
/// is the unison (`1.0`); `SEMITONE_RATIOS[11]` is one semitone below the
/// octave (`~1.8877`, i.e. `2^(11/12)`). `SEMITONE_RATIOS[6]` (the tritone,
/// 6 semitones = exactly half an octave) is mathematically `2^0.5`, i.e.
/// `sqrt(2)` — spelled via the stdlib's `SQRT_2` constant rather than the
/// literal `1.414_214` because clippy's `approx_constant` lint (correctly)
/// flags a hand-typed float that close to a known constant as a likely typo.
const SEMITONE_RATIOS: [f32; 12] = [
    1.0,
    1.059_463,
    1.122_462,
    1.189_207,
    1.259_921,
    1.334_84,
    std::f32::consts::SQRT_2,
    1.498_307,
    1.587_401,
    1.681_793,
    1.781_797,
    1.887_749,
];

const A2_HZ: f32 = 110.0;

/// Convert a semitone offset from A2 to a frequency in Hz. `div_euclid` is
/// **not** on the workspace's banned-call list (only `rem_euclid` is, since
/// it's the one method-name spelling that list actually bans); it's used
/// here for a proper floor-division octave count, with the in-range
/// remainder then computed by plain subtraction (`n - octave*12`) instead of
/// calling the banned `rem_euclid` — so this never needs an out-of-range or
/// negative table index.
fn note_freq(semitone_offset: i8) -> f32 {
    let n = semitone_offset as i32;
    let octave = n.div_euclid(12);
    let idx = (n - octave * 12) as usize;
    let mut freq = A2_HZ * SEMITONE_RATIOS[idx];
    if octave >= 0 {
        for _ in 0..octave {
            freq *= 2.0;
        }
    } else {
        for _ in 0..(-octave) {
            freq *= 0.5;
        }
    }
    freq
}

fn trigger_channel(
    mixer: &mut Mixer,
    channel: usize,
    waveform: Waveform,
    freq_hz: f32,
    adsr: crate::env::AdsrParams,
    gain: f32,
) {
    mixer.trigger_at(
        channel,
        VoiceParams {
            waveform,
            freq_hz,
            sweep_hz_per_s: 0.0,
            adsr,
            gain,
            priority: 255,
        },
    );
}

/// Kick "body": a one-shot low Square blip borrowed from the generic SFX
/// pool (see module docs) so it can sound in the same row as the dedicated
/// drums voice's snare/hat.
fn trigger_kick(mixer: &mut Mixer) {
    mixer.trigger(VoiceParams {
        waveform: Waveform::Square { duty: 128 },
        freq_hz: 60.0,
        sweep_hz_per_s: -20.0,
        adsr: adsr_ms(2, 90, 0.0, 0),
        gain: 0.6,
        priority: 150,
    });
}

fn trigger_snare(mixer: &mut Mixer) {
    trigger_channel(
        mixer,
        CH_DRUMS,
        Waveform::Noise,
        900.0,
        adsr_ms(1, 90, 0.0, 0),
        0.5,
    );
}

fn trigger_hat(mixer: &mut Mixer) {
    trigger_channel(
        mixer,
        CH_DRUMS,
        Waveform::Noise,
        4_000.0,
        adsr_ms(1, 20, 0.0, 0),
        0.22,
    );
}

fn trigger_perc(mixer: &mut Mixer, code: i8) {
    match code {
        SNARE => trigger_snare(mixer),
        HAT => trigger_hat(mixer),
        _ => {}
    }
}

// ============================================================================
// Menu theme: 112 BPM, A natural minor, 8-bar loop (game_design.md §8).
// ============================================================================
mod menu {
    use super::*;

    pub const BPM: u32 = 112;
    pub const BARS: u32 = 8;
    pub const TOTAL_ROWS: u32 = BARS * ROWS_PER_BAR;

    // Pulse1: "rolling A-C-E arpeggio, 25% duty, warm" (A minor: A=0 C=3
    // E=7; the natural-minor G-natural color tone at 10 replaces the octave
    // A once per group for a wistful, non-harmonic-minor character).
    const LEAD_MAIN: [i8; 16] = [0, 3, 7, 12, 0, 3, 7, 12, 0, 3, 7, 10, 0, 3, 7, 12];
    // Turnaround (bar 4): steps down through the scale for a cadence.
    const LEAD_TURN: [i8; 16] = [0, 3, 7, 12, 2, 5, 8, 12, 0, 3, 7, 10, 7, 5, 3, 0];
    // Breath (bar 8): a short descending run then rests, giving the 8-bar
    // loop a moment of silence before it repeats.
    const LEAD_BREATH: [i8; 16] = [
        0, 3, 7, 10, 7, 5, 3, 2, 0, REST, REST, REST, REST, REST, REST, REST,
    ];

    // Pulse2: soft pad, chord tones an octave down from the arpeggio root,
    // one hit per beat (row 0/4/8/12).
    const PAD_MAIN: [i8; 16] = [
        -12, REST, REST, REST, -9, REST, REST, REST, -5, REST, REST, REST, -12, REST, REST, REST,
    ];
    const PAD_TURN: [i8; 16] = [
        -12, REST, REST, REST, -10, REST, REST, REST, -5, REST, REST, REST, -12, REST, REST, REST,
    ];

    // Bass: root-fifth on beats 1 & 3 ("root-fifth on 1 & 3"), two octaves
    // down from the arpeggio root.
    const BASS_MAIN: [i8; 16] = [
        -24, REST, REST, REST, REST, REST, REST, REST, -17, REST, REST, REST, REST, REST, REST,
        REST,
    ];

    // Drums: lazy kick(row0) / snare(row8) / offbeat hats(2,6,10,14).
    const KICK_MAIN: [bool; 16] = [
        true, false, false, false, false, false, false, false, false, false, false, false, false,
        false, false, false,
    ];
    const PERC_MAIN: [i8; 16] = [
        REST, REST, HAT, REST, REST, REST, HAT, REST, SNARE, REST, HAT, REST, REST, REST, HAT, REST,
    ];
    const PERC_BREATH: [i8; 16] = [REST; 16];

    fn lead_pattern_for(bar: usize) -> &'static [i8; 16] {
        match bar {
            3 => &LEAD_TURN,
            7 => &LEAD_BREATH,
            _ => &LEAD_MAIN,
        }
    }
    fn pad_pattern_for(bar: usize) -> &'static [i8; 16] {
        match bar {
            3 | 7 => &PAD_TURN,
            _ => &PAD_MAIN,
        }
    }
    fn perc_pattern_for(bar: usize) -> &'static [i8; 16] {
        match bar {
            7 => &PERC_BREATH,
            _ => &PERC_MAIN,
        }
    }

    pub fn kick_on_row(row: u32) -> bool {
        let r = (row % ROWS_PER_BAR) as usize;
        KICK_MAIN[r]
    }

    pub fn fire_row(row: u32, mixer: &mut Mixer) {
        let bar = ((row / ROWS_PER_BAR) % BARS) as usize;
        let r = (row % ROWS_PER_BAR) as usize;

        let lead = lead_pattern_for(bar)[r];
        if lead != REST {
            trigger_channel(
                mixer,
                CH_PULSE1,
                Waveform::Square { duty: 64 }, // 25% duty, warm
                note_freq(lead),
                adsr_ms(6, 110, 0.0, 0),
                0.4,
            );
        }

        let pad = pad_pattern_for(bar)[r];
        if pad != REST {
            trigger_channel(
                mixer,
                CH_PULSE2,
                Waveform::Square { duty: 64 },
                note_freq(pad),
                adsr_ms(20, 470, 0.0, 0),
                0.3,
            );
        }

        let bass = BASS_MAIN[r];
        if bass != REST {
            trigger_channel(
                mixer,
                CH_BASS,
                Waveform::Triangle,
                note_freq(bass),
                adsr_ms(10, 500, 0.0, 0),
                0.45,
            );
        }

        if KICK_MAIN[r] {
            trigger_kick(mixer);
        }
        trigger_perc(mixer, perc_pattern_for(bar)[r]);
    }
}

// ============================================================================
// Battle theme: 148 BPM, A Phrygian (natural minor + b2), 16-bar structure
// (4 riser + 8 groove + 4 break). Intensity layer: pulse2 counter-melody +
// doubled hats, silent until `set_intensity(true)` (game_design.md §8).
// ============================================================================
mod battle {
    use super::*;

    pub const BPM: u32 = 148;
    pub const BARS: u32 = 16;
    pub const TOTAL_ROWS: u32 = BARS * ROWS_PER_BAR;

    const GROOVE_START_BAR: u32 = 4;

    // A Phrygian degrees from A2: A=0, Bb(b2)=1, C=3, D=5, E=7, F=8, G=10.

    // --- Riser (bars 0..4): a slow single-note tension figure stepping
    // 0 -> 1 -> 3 across bars 1-3 (revisiting the b2 bite each time), then an
    // ascending run in bar 4 building straight into the groove drop.
    const RISER_LEAD1: [i8; 16] = [
        0, REST, REST, REST, REST, REST, REST, REST, REST, REST, REST, REST, REST, REST, REST, REST,
    ];
    const RISER_LEAD2: [i8; 16] = [
        1, REST, REST, REST, REST, REST, REST, REST, REST, REST, REST, REST, REST, REST, REST, REST,
    ];
    const RISER_LEAD3: [i8; 16] = [
        3, REST, REST, REST, REST, REST, REST, REST, REST, REST, REST, REST, REST, REST, REST, REST,
    ];
    const RISER_LEAD4: [i8; 16] = [
        0, 1, 3, 5, 7, 8, 10, 12, REST, REST, REST, REST, REST, REST, REST, REST,
    ];

    // --- Groove (bars 4..12): driving phrygian riff hammering the A-Bb
    // half-step, two alternating bar shapes for variety across 8 bars.
    const GROOVE_LEAD_A: [i8; 16] = [0, 1, 0, 1, 3, 1, 0, REST, 5, 3, 1, 0, 7, 5, 3, 1];
    const GROOVE_LEAD_B: [i8; 16] = [0, 1, 3, 1, 0, 1, 0, REST, 8, 7, 5, 3, 1, 0, 1, REST];

    // --- Break (bars 12..16): a sparse callback to the riff, then a rising
    // fill run in the last bar leading back into the riser loop restart.
    const BREAK_LEAD: [i8; 16] = [
        0, REST, 1, REST, 0, REST, REST, REST, 3, REST, 1, REST, 0, REST, REST, REST,
    ];
    const BREAK_LEAD_FILL: [i8; 16] = [
        0, 1, 3, 5, 7, 8, 10, 12, 13, 15, REST, REST, REST, REST, REST, REST,
    ];

    fn lead_pattern_for(bar: u32) -> &'static [i8; 16] {
        match bar {
            0 => &RISER_LEAD1,
            1 => &RISER_LEAD2,
            2 => &RISER_LEAD3,
            3 => &RISER_LEAD4,
            4..=11 => {
                if bar.is_multiple_of(2) {
                    &GROOVE_LEAD_A
                } else {
                    &GROOVE_LEAD_B
                }
            }
            15 => &BREAK_LEAD_FILL,
            _ => &BREAK_LEAD,
        }
    }

    // --- Intensity layer: pulse2 counter-melody, silent (never triggered)
    // until `set_intensity(true)` takes effect; only sounds in groove/break
    // (bars >= 4) — the riser stays clean either way, an arrangement choice.
    const INTENSITY_COUNTER: [i8; 16] = [
        REST, REST, 7, REST, 8, REST, 7, REST, REST, REST, 5, REST, 3, REST, 1, REST,
    ];

    // --- Bass: pedal tone in the riser, galloping 8ths in the groove
    // ("galloping eighth bass"), half-time in the break.
    const RISER_BASS: [i8; 16] = [
        -24, REST, REST, REST, -24, REST, REST, REST, -24, REST, REST, REST, -24, REST, REST, REST,
    ];
    const GROOVE_BASS: [i8; 16] = [
        -24, REST, -24, REST, -17, REST, -24, REST, -24, REST, -24, REST, -19, REST, -24, REST,
    ];
    const BREAK_BASS: [i8; 16] = [
        -24, REST, REST, REST, REST, REST, REST, REST, -24, REST, REST, REST, REST, REST, REST,
        REST,
    ];

    fn bass_pattern_for(bar: u32) -> &'static [i8; 16] {
        match bar {
            0..=3 => &RISER_BASS,
            4..=11 => &GROOVE_BASS,
            _ => &BREAK_BASS,
        }
    }

    // --- Kick: four-on-floor once fully built ("four-on-floor kick"),
    // building density bar-by-bar through the riser.
    const KICK_BUILD1: [bool; 16] = [
        true, false, false, false, false, false, false, false, false, false, false, false, false,
        false, false, false,
    ];
    const KICK_BUILD2: [bool; 16] = [
        true, false, false, false, false, false, false, false, true, false, false, false, false,
        false, false, false,
    ];
    const KICK_BUILD3: [bool; 16] = [
        true, false, false, false, true, false, false, false, true, false, false, false, false,
        false, false, false,
    ];
    const KICK_FULL: [bool; 16] = [
        true, false, false, false, true, false, false, false, true, false, false, false, true,
        false, false, false,
    ];

    fn kick_pattern_for(bar: u32) -> &'static [bool; 16] {
        match bar {
            0 => &KICK_BUILD1,
            1 => &KICK_BUILD2,
            2 => &KICK_BUILD3,
            _ => &KICK_FULL,
        }
    }

    // --- Perc (snare/hat, dedicated drums voice): "snare rows 4/12, 16th
    // hats" in the groove (16th = every row); "doubled hats" for the
    // intensity layer means the *base* groove is 8th-note hats and
    // intensity doubles it to 16ths — literally 2x density, hence the
    // separate `*_INTENSE` variants below instead of a shared base pattern.
    const PERC_RISER1: [i8; 16] = [REST; 16];
    const PERC_RISER2: [i8; 16] = [
        REST, REST, HAT, REST, REST, REST, HAT, REST, REST, REST, HAT, REST, REST, REST, HAT, REST,
    ];
    const PERC_RISER3: [i8; 16] = [
        REST, REST, HAT, REST, REST, REST, HAT, REST, REST, REST, HAT, REST, SNARE, REST, HAT, REST,
    ];
    const PERC_RISER4: [i8; 16] = [
        HAT, HAT, HAT, HAT, SNARE, HAT, HAT, HAT, HAT, HAT, HAT, HAT, SNARE, HAT, HAT, HAT,
    ];

    const PERC_GROOVE: [i8; 16] = [
        HAT, REST, HAT, REST, SNARE, REST, HAT, REST, HAT, REST, HAT, REST, SNARE, REST, HAT, REST,
    ];
    const PERC_GROOVE_INTENSE: [i8; 16] = [
        HAT, HAT, HAT, HAT, SNARE, HAT, HAT, HAT, HAT, HAT, HAT, HAT, SNARE, HAT, HAT, HAT,
    ];

    const PERC_BREAK: [i8; 16] = [
        REST, REST, HAT, REST, REST, REST, REST, REST, REST, REST, HAT, REST, REST, REST, REST,
        REST,
    ];
    const PERC_BREAK_INTENSE: [i8; 16] = [
        HAT, REST, HAT, REST, REST, REST, HAT, REST, HAT, REST, HAT, REST, REST, REST, HAT, REST,
    ];
    const PERC_FILL: [i8; 16] = [HAT; 16];

    fn perc_pattern_for(bar: u32, intensity_on: bool) -> &'static [i8; 16] {
        match bar {
            0 => &PERC_RISER1,
            1 => &PERC_RISER2,
            2 => &PERC_RISER3,
            3 => &PERC_RISER4,
            4..=11 => {
                if intensity_on {
                    &PERC_GROOVE_INTENSE
                } else {
                    &PERC_GROOVE
                }
            }
            15 => &PERC_FILL,
            _ => {
                if intensity_on {
                    &PERC_BREAK_INTENSE
                } else {
                    &PERC_BREAK
                }
            }
        }
    }

    pub fn kick_on_row(row: u32) -> bool {
        let bar = (row / ROWS_PER_BAR) % BARS;
        let r = (row % ROWS_PER_BAR) as usize;
        kick_pattern_for(bar)[r]
    }

    pub fn fire_row(row: u32, intensity_on: bool, mixer: &mut Mixer) {
        let bar = (row / ROWS_PER_BAR) % BARS;
        let r = (row % ROWS_PER_BAR) as usize;

        let lead = lead_pattern_for(bar)[r];
        if lead != REST {
            trigger_channel(
                mixer,
                CH_PULSE1,
                Waveform::Square { duty: 128 }, // 50% duty lead
                note_freq(lead),
                adsr_ms(4, 90, 0.0, 0),
                0.42,
            );
        }

        if intensity_on && bar >= GROOVE_START_BAR {
            let counter = INTENSITY_COUNTER[r];
            if counter != REST {
                trigger_channel(
                    mixer,
                    CH_PULSE2,
                    Waveform::Square { duty: 64 }, // 25% duty stabs
                    note_freq(counter),
                    adsr_ms(4, 90, 0.0, 0),
                    0.32,
                );
            }
        }

        let bass = bass_pattern_for(bar)[r];
        if bass != REST {
            trigger_channel(
                mixer,
                CH_BASS,
                Waveform::Triangle,
                note_freq(bass),
                adsr_ms(3, 140, 0.0, 0),
                0.5,
            );
        }

        if kick_pattern_for(bar)[r] {
            trigger_kick(mixer);
        }
        trigger_perc(mixer, perc_pattern_for(bar, intensity_on)[r]);
    }
}

/// The 4-channel music tracker (menu + battle themes).
pub struct Tracker {
    song: SongId,
    bpm: u32,
    total_rows: u32,
    row: u32,
    samples_until_next_row: u32,
    row_accum: u32,
    /// `false` only before the very first row has ever fired. Lets
    /// `advance` tell "fire row 0 for the first time" (don't increment)
    /// apart from every later boundary (increment, then fire) using the
    /// same code path — see `advance`'s doc comment for why this matters.
    started: bool,
    intensity_on: bool,
    intensity_pending: bool,
}

impl Tracker {
    pub fn new(song: SongId) -> Tracker {
        let (bpm, total_rows) = match song {
            SongId::Menu => (menu::BPM, menu::TOTAL_ROWS),
            SongId::Battle => (battle::BPM, battle::TOTAL_ROWS),
        };
        Tracker {
            song,
            bpm,
            total_rows,
            row: 0,
            // 0 => the very first `advance()` call fires row 0 immediately.
            samples_until_next_row: 0,
            row_accum: 0,
            started: false,
            intensity_on: false,
            intensity_pending: false,
        }
    }

    /// Request the intensity layer on/off. Takes effect at the next bar
    /// boundary (`row % 16 == 0`), never mid-bar (module brief).
    pub fn set_intensity(&mut self, on: bool) {
        self.intensity_pending = on;
    }

    pub fn row_index(&self) -> u32 {
        self.row
    }

    pub fn kick_on_current_row(&self) -> bool {
        match self.song {
            SongId::Menu => menu::kick_on_row(self.row),
            SongId::Battle => battle::kick_on_row(self.row),
        }
    }

    /// Bresenham/DDA row length: see module docs. `numerator` is
    /// `60 * SAMPLE_RATE` (seconds->samples for a whole-minute's worth of
    /// beats); `denom` is `bpm * 4` (4 rows per beat).
    fn row_len_samples(&mut self) -> u32 {
        let numerator: u64 = 60 * SAMPLE_RATE as u64;
        let denom: u64 = self.bpm as u64 * 4;
        let base = (numerator / denom) as u32;
        let rem = (numerator % denom) as u32;
        self.row_accum += rem;
        if self.row_accum >= denom as u32 {
            self.row_accum -= denom as u32;
            base + 1
        } else {
            base
        }
    }

    fn fire_row(&mut self, mixer: &mut Mixer) {
        if self.row.is_multiple_of(ROWS_PER_BAR) {
            self.intensity_on = self.intensity_pending;
        }
        match self.song {
            SongId::Menu => menu::fire_row(self.row, mixer),
            SongId::Battle => battle::fire_row(self.row, self.intensity_on, mixer),
        }
    }

    /// Advance the song by `samples` audio samples, firing every row
    /// boundary crossed — sample-accurate regardless of how the caller
    /// chunks its calls (one call for `samples` total gives byte-identical
    /// scheduling to `samples` calls of `1` each).
    ///
    /// The row increment and that row's `fire_row` happen in the same
    /// statement (guarded by `started`, so row 0's very first firing isn't
    /// preceded by a spurious increment) rather than the increment
    /// happening at the tail of the *previous* row's processing: splitting
    /// them across two separate actions would let a call that ends exactly
    /// on a row boundary return with `row_index()` already showing the new
    /// row number while that row's notes (and, for the battle theme, the
    /// bar-boundary intensity latch) hadn't actually fired yet — a real bug
    /// this crate's own tests caught by advancing one sample at a time.
    pub fn advance(&mut self, mixer: &mut Mixer, samples: u32) {
        let mut remaining = samples;
        while remaining > 0 {
            if self.samples_until_next_row == 0 {
                if self.started {
                    self.row = (self.row + 1) % self.total_rows;
                }
                self.started = true;
                self.fire_row(mixer);
                self.samples_until_next_row = self.row_len_samples();
            }
            let step = remaining.min(self.samples_until_next_row);
            self.samples_until_next_row -= step;
            remaining -= step;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test 6 (tracker timing): total samples over one full menu-theme loop
    /// equals `bars*16*samples_per_row` exactly (the Bresenham accumulator
    /// guarantees an exact cumulative sum, not just within ±1 — see module
    /// docs on `row_len_samples`).
    /// Exactness check shared by the menu/battle tests below: `advance` is a
    /// pull-based step function, so the wrap from the last row back to row 0
    /// is only *discovered* on the call that consumes the first sample of
    /// the next lap — asking `row_index()` to already show `0` after exactly
    /// `expected` samples would therefore always be one sample early by
    /// construction, regardless of any drift. The precise, drift-free claim
    /// is instead: after exactly `expected` samples the tracker is still
    /// sitting on the *last* row of the lap (which must therefore have been
    /// sounding for its full, exact duration), and the very next sample
    /// flips it to row 0 — neither early nor late.
    fn assert_loop_is_exact(song: SongId, bpm: u32, total_rows: u32) {
        let numerator: u64 = 60 * SAMPLE_RATE as u64;
        let denom: u64 = bpm as u64 * 4;
        let expected = numerator * total_rows as u64 / denom;

        let mut tracker = Tracker::new(song);
        let mut mixer = Mixer::new();
        tracker.advance(&mut mixer, expected as u32 - 1);
        assert_eq!(
            tracker.row_index(),
            total_rows - 1,
            "should still be on the last row with 1 sample to go"
        );
        tracker.advance(&mut mixer, 1); // exactly `expected` samples consumed now
        assert_eq!(
            tracker.row_index(),
            total_rows - 1,
            "last row's final sample"
        );
        tracker.advance(&mut mixer, 1); // the `expected + 1`-th sample reveals the wrap
        assert_eq!(
            tracker.row_index(),
            0,
            "must wrap at exactly `expected` samples, not early or late"
        );
    }

    /// Test 6 (tracker timing), menu theme.
    #[test]
    fn menu_loop_total_samples_matches_exact_formula() {
        assert_loop_is_exact(SongId::Menu, menu::BPM, menu::TOTAL_ROWS);
    }

    /// Test 6 (tracker timing), battle theme.
    #[test]
    fn battle_loop_total_samples_matches_exact_formula() {
        assert_loop_is_exact(SongId::Battle, battle::BPM, battle::TOTAL_ROWS);
    }

    /// Sample-accurate regardless of how the caller chunks calls: one big
    /// `advance` for the whole loop lands on the same row as many small
    /// ones.
    #[test]
    fn advance_is_chunking_independent() {
        let numerator: u64 = 60 * SAMPLE_RATE as u64;
        let denom: u64 = battle::BPM as u64 * 4;
        let expected = numerator * battle::TOTAL_ROWS as u64 / denom;

        let mut one_shot = Tracker::new(SongId::Battle);
        let mut mixer_a = Mixer::new();
        one_shot.advance(&mut mixer_a, expected as u32 * 2);

        let mut piecemeal = Tracker::new(SongId::Battle);
        let mut mixer_b = Mixer::new();
        let mut remaining = expected as u32 * 2;
        while remaining > 0 {
            let step = remaining.min(37); // an arbitrary, awkward chunk size
            piecemeal.advance(&mut mixer_b, step);
            remaining -= step;
        }

        assert_eq!(one_shot.row_index(), piecemeal.row_index());
    }

    /// `kick_on_current_row`'s underlying per-row logic fires on the
    /// documented rows: menu's lazy kick on row 0 of every bar only; battle's
    /// four-on-floor (rows 0/4/8/12) once fully built (bar 3 onward), built
    /// up gradually before that across the 4-bar riser.
    #[test]
    fn kick_flags_match_documented_rows() {
        // Menu: row 0 of every bar is a kick, everything else in the bar is
        // not (checked across all 8 bars).
        for bar in 0..menu::BARS {
            let base = bar * ROWS_PER_BAR;
            assert!(menu::kick_on_row(base), "menu bar {bar} row 0 should kick");
            for r in 1..ROWS_PER_BAR {
                assert!(
                    !menu::kick_on_row(base + r),
                    "menu bar {bar} row {r} should not kick"
                );
            }
        }

        // Battle: bar 0 (riser) kicks only on row 0; bar 3 onward (fully
        // built) is four-on-floor (rows 0/4/8/12), including every groove
        // and break bar.
        assert!(battle::kick_on_row(0));
        assert!(!battle::kick_on_row(4));
        for bar in 3..battle::BARS {
            let base = bar * ROWS_PER_BAR;
            for &beat_row in &[0u32, 4, 8, 12] {
                assert!(
                    battle::kick_on_row(base + beat_row),
                    "battle bar {bar} row {beat_row} should kick"
                );
            }
        }
    }

    /// `set_intensity` requested mid-bar only takes effect at the next bar
    /// boundary (`row % 16 == 0`), never immediately — confirmed by
    /// advancing exactly one row at a time (each `advance` call fires at
    /// most one row boundary here since row lengths are always far more
    /// than 1 sample) and checking the public `row_index`/`kick_on_current_row`
    /// stay consistent with a private intensity flag exposed only to this
    /// in-crate test module.
    #[test]
    fn set_intensity_takes_effect_only_at_next_bar_boundary() {
        let mut tracker = Tracker::new(SongId::Battle);
        let mut mixer = Mixer::new();

        // Advance row-by-row until row 2 of bar 4 (groove start), i.e. row
        // index `4*16 + 2`, well clear of any bar boundary.
        let target_row = 4 * ROWS_PER_BAR + 2;
        while tracker.row_index() != target_row {
            let before = tracker.row_index();
            while tracker.row_index() == before {
                tracker.advance(&mut mixer, 1);
            }
        }
        assert!(!tracker.intensity_on);

        tracker.set_intensity(true);
        // Still mid-bar: must not have taken effect yet.
        assert!(!tracker.intensity_on);

        // Advance to the next bar boundary (row % 16 == 0), one row at a
        // time, checking the flag flips exactly there and not before.
        loop {
            let before = tracker.row_index();
            while tracker.row_index() == before {
                tracker.advance(&mut mixer, 1);
            }
            if tracker.row_index().is_multiple_of(ROWS_PER_BAR) {
                break;
            }
            assert!(
                !tracker.intensity_on,
                "intensity flipped before the bar boundary"
            );
        }
        assert!(
            tracker.intensity_on,
            "intensity should be on right at the bar boundary"
        );
    }

    #[test]
    fn note_freq_a2_is_110_and_octave_up_is_220() {
        assert!((note_freq(0) - 110.0).abs() < 1e-3);
        assert!((note_freq(12) - 220.0).abs() < 1e-3);
        assert!((note_freq(-12) - 55.0).abs() < 1e-3);
    }
}
