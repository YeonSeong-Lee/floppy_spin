//! Deep audio module: cue scheduling and synthesis behind one render call.

use floppy_core::physics::BattleEvent;

use crate::{mixer::Mixer, sfx, tracker::Tracker, Sfx, SongId};

const MAX_CUES_PER_BATCH: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioCue {
    pub sample: u64,
    pub row: u32,
    pub kick: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioBatch {
    pub start_sample: u64,
    pub end_sample: u64,
    cues: [Option<AudioCue>; MAX_CUES_PER_BATCH],
    cue_count: usize,
}

impl AudioBatch {
    pub fn cues(&self) -> impl Iterator<Item = AudioCue> + '_ {
        self.cues[..self.cue_count].iter().filter_map(|cue| *cue)
    }
}

pub struct AudioEngine {
    mixer: Mixer,
    tracker: Tracker,
    song: SongId,
    sample_cursor: u64,
}

impl AudioEngine {
    pub fn new(song: SongId) -> Self {
        Self {
            mixer: Mixer::new(),
            tracker: Tracker::new(song),
            song,
            sample_cursor: 0,
        }
    }

    pub fn set_song(&mut self, song: SongId) {
        if self.song != song {
            self.tracker = Tracker::new(song);
            self.song = song;
        }
    }

    pub fn set_intensity(&mut self, enabled: bool) {
        self.tracker.set_intensity(enabled);
    }

    pub fn set_group_gains(&mut self, sfx_gain: f32, music_gain: f32) {
        self.mixer.set_group_gains(sfx_gain, music_gain);
    }

    pub fn play(&mut self, cue: Sfx) {
        sfx::play(&mut self.mixer, cue);
    }

    pub fn on_battle_event(&mut self, event: &BattleEvent) {
        sfx::on_event(&mut self.mixer, event);
    }

    pub fn sample_cursor(&self) -> u64 {
        self.sample_cursor
    }

    pub fn render(&mut self, out: &mut [i16]) -> AudioBatch {
        let start_sample = self.sample_cursor;
        let mut cues = [None; MAX_CUES_PER_BATCH];
        let mut cue_count = 0;
        self.tracker
            .render(&mut self.mixer, out, |row, kick, offset| {
                if cue_count < cues.len() {
                    cues[cue_count] = Some(AudioCue {
                        sample: start_sample + offset as u64,
                        row,
                        kick,
                    });
                    cue_count += 1;
                }
            });
        self.sample_cursor += out.len() as u64;
        AudioBatch {
            start_sample,
            end_sample: self.sample_cursor,
            cues,
            cue_count,
        }
    }
}
