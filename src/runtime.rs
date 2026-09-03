use std::collections::VecDeque;

use floppy_audio::{AudioBatch, AudioCue, AudioEngine, Sfx, SongId};
use floppy_core::flow::{FlowState, MatchPhase, Screen, WindowScale};
use floppy_core::input::FrameInput;
use floppy_core::physics::{BattleEvent, TUNE};
use floppy_core::save::{self, SaveLoadOutcome};
use floppy_core::session::{GameSession, SessionDigest};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeConfig {
    pub seed: u64,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self { seed: 0xF10B_B75E }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct PlaybackCursor(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitReason {
    Requested,
    WindowClosed,
    FatalError,
}

#[derive(Clone, Copy, Debug)]
pub enum PresentationEvent {
    ScreenChanged { from: Screen, to: Screen },
    Battle(BattleEvent),
    ScoreTally,
    MusicRow(AudioCue),
    ExitRequested,
}

const MAX_PRESENTATION_EVENTS_PER_TICK: usize = 72;

#[derive(Clone, Debug)]
pub struct PresentationEvents {
    events: [PresentationEvent; MAX_PRESENTATION_EVENTS_PER_TICK],
    len: usize,
}

impl Default for PresentationEvents {
    fn default() -> Self {
        Self {
            events: [PresentationEvent::ExitRequested; MAX_PRESENTATION_EVENTS_PER_TICK],
            len: 0,
        }
    }
}

impl PresentationEvents {
    fn push(&mut self, event: PresentationEvent) {
        assert!(
            self.len < self.events.len(),
            "presentation event capacity exceeded"
        );
        self.events[self.len] = event;
        self.len += 1;
    }

    pub fn iter(&self) -> impl Iterator<Item = PresentationEvent> + '_ {
        self.events[..self.len].iter().copied()
    }

    pub fn as_slice(&self) -> &[PresentationEvent] {
        &self.events[..self.len]
    }
}

#[derive(Clone, Debug, Default)]
pub struct RuntimeEffects {
    pub events: PresentationEvents,
    pub save: Option<Vec<u8>>,
    pub exit_requested: bool,
    pub window_scale: Option<WindowScale>,
}

pub struct FrameView<'a> {
    pub alpha: f32,
    pub state: &'a FlowState,
    pub playback_cursor: PlaybackCursor,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShutdownEffects {
    pub save: Option<Vec<u8>>,
    pub reason: ExitReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeError {
    AlreadyFinished,
}

/// Shared production runtime used by window and headless adapters.
pub struct AppRuntime {
    session: GameSession,
    audio: AudioEngine,
    playback_cursor: PlaybackCursor,
    hum_active: bool,
    dirty: bool,
    finished: bool,
    pending_audio_cues: VecDeque<AudioCue>,
}

impl AppRuntime {
    pub fn new(config: RuntimeConfig, save: SaveLoadOutcome) -> Result<Self, RuntimeError> {
        let session = GameSession::new(config.seed, save.state_or_default());
        let song = song_for_screen(session.snapshot().presentation().screen);
        Ok(Self {
            session,
            audio: AudioEngine::new(song),
            playback_cursor: PlaybackCursor::default(),
            hum_active: false,
            dirty: false,
            finished: false,
            pending_audio_cues: VecDeque::with_capacity(16),
        })
    }

    pub fn advance(
        &mut self,
        input: FrameInput,
        playback_cursor: PlaybackCursor,
    ) -> RuntimeEffects {
        if self.finished {
            return RuntimeEffects {
                exit_requested: true,
                ..RuntimeEffects::default()
            };
        }
        self.playback_cursor = playback_cursor;
        let old = self.session.snapshot();
        let old = old.presentation();
        let old_screen = old.screen;
        let old_score = old.score;
        let old_settings = old.settings;
        let old_parts = old.parts;
        let old_nav = (old.menu_cursor, old.select_cursor, old.settings_cursor);
        let output = self.session.advance(input);
        let state = self.session.snapshot();
        let state = state.presentation();

        let mut effects = RuntimeEffects::default();
        while self
            .pending_audio_cues
            .front()
            .is_some_and(|cue| cue.sample <= playback_cursor.0)
        {
            if let Some(cue) = self.pending_audio_cues.pop_front() {
                effects.events.push(PresentationEvent::MusicRow(cue));
            }
        }
        if let Some((from, to)) = output.transition {
            effects
                .events
                .push(PresentationEvent::ScreenChanged { from, to });
            if matches!(old_screen, Screen::Garage | Screen::Settings) {
                let (parts, settings) = self.session.save_snapshot();
                effects.save = Some(save::encode(parts, &settings));
            }
            if matches!(old_screen, Screen::Settings) {
                effects.window_scale = Some(state.settings.window_scale);
            }
        }
        let new_nav = (
            state.menu_cursor,
            state.select_cursor,
            state.settings_cursor,
        );
        if old_screen == state.screen && old_nav != new_nav {
            self.audio.play(Sfx::MenuMove);
        } else if old_screen != state.screen {
            let sfx = if matches!(state.screen, Screen::MainMenu) {
                Sfx::MenuBack
            } else {
                Sfx::MenuSelect
            };
            self.audio.play(sfx);
        }
        for event in output.battle_events.iter() {
            self.audio.on_battle_event(&event);
            effects.events.push(PresentationEvent::Battle(event));
        }
        if state.score != old_score {
            effects.events.push(PresentationEvent::ScoreTally);
        }
        self.dirty |= old_settings != state.settings || old_parts != state.parts;

        self.audio.set_song(song_for_screen(state.screen));
        let fighting = matches!(state.screen, Screen::Match(MatchPhase::Fight));
        let armed = fighting
            && state
                .world
                .as_ref()
                .map(|world| {
                    world.tops[0].combat.special_armed || world.tops[1].combat.special_armed
                })
                .unwrap_or(false);
        self.audio.set_intensity(armed);
        if fighting {
            if let Some(world) = &state.world {
                self.audio.play(Sfx::SpinHum {
                    rpm_frac: world.tops[0].spin / TUNE.spin_max,
                });
                self.hum_active = true;
            }
        } else if self.hum_active {
            self.audio.play(Sfx::SpinHum { rpm_frac: 0.0 });
            self.hum_active = false;
        }
        self.audio.set_group_gains(
            state.settings.sfx_vol as f32 / 10.0,
            state.settings.music_vol as f32 / 10.0,
        );

        effects.exit_requested = output.quit_requested;
        if effects.exit_requested {
            effects.events.push(PresentationEvent::ExitRequested);
        }
        effects
    }

    pub fn render(&self, alpha: f32) -> FrameView<'_> {
        FrameView {
            alpha: if alpha.is_finite() {
                alpha.clamp(0.0, 1.0)
            } else {
                0.0
            },
            state: self.session.snapshot().presentation(),
            playback_cursor: self.playback_cursor,
        }
    }

    pub fn render_audio(&mut self, out: &mut [i16]) -> AudioBatch {
        let batch = self.audio.render(out);
        self.pending_audio_cues.extend(batch.cues());
        batch
    }

    pub fn play_ui_sfx(&mut self, cue: Sfx) {
        self.audio.play(cue);
    }

    pub fn finish(&mut self, reason: ExitReason) -> ShutdownEffects {
        let save = if self.finished {
            None
        } else {
            self.finished = true;
            let (parts, settings) = self.session.save_snapshot();
            self.dirty = false;
            Some(save::encode(parts, &settings))
        };
        ShutdownEffects { save, reason }
    }

    pub fn mark_saved(&mut self) {
        self.dirty = false;
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn digest(&self) -> SessionDigest {
        self.session.digest()
    }
}

fn song_for_screen(screen: Screen) -> SongId {
    match screen {
        Screen::Match(_) => SongId::Battle,
        _ => SongId::Menu,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use floppy_core::input::InputState;

    #[test]
    fn tap_between_polls_is_delivered_once() {
        let mut runtime =
            AppRuntime::new(RuntimeConfig::default(), SaveLoadOutcome::Missing).unwrap();
        for _ in 0..=floppy_core::flow::BOOT_FRAMES {
            runtime.advance(FrameInput::default(), PlaybackCursor(0));
        }
        let tap = FrameInput {
            pressed: InputState {
                dash: true,
                ..InputState::default()
            },
            released: InputState {
                dash: true,
                ..InputState::default()
            },
            focused: true,
            ..FrameInput::default()
        };
        let effects = runtime.advance(tap, PlaybackCursor(0));
        assert!(effects.events.iter().any(|event| matches!(
            event,
            PresentationEvent::ScreenChanged {
                from: Screen::Title,
                to: Screen::MainMenu
            }
        )));
        let effects = runtime.advance(FrameInput::default(), PlaybackCursor(0));
        assert!(!effects
            .events
            .iter()
            .any(|event| matches!(event, PresentationEvent::ScreenChanged { .. })));
    }

    #[test]
    fn finish_emits_the_save_exactly_once() {
        let mut runtime =
            AppRuntime::new(RuntimeConfig::default(), SaveLoadOutcome::Missing).unwrap();
        assert!(runtime.finish(ExitReason::WindowClosed).save.is_some());
        assert!(runtime.finish(ExitReason::WindowClosed).save.is_none());
    }

    #[test]
    fn exit_presentation_event_is_emitted_exactly_once() {
        let mut runtime =
            AppRuntime::new(RuntimeConfig::default(), SaveLoadOutcome::Missing).unwrap();
        for _ in 0..=floppy_core::flow::BOOT_FRAMES {
            runtime.advance(FrameInput::default(), PlaybackCursor(0));
        }
        let quit = FrameInput {
            escape_pressed: true,
            ..FrameInput::default()
        };
        let first = runtime.advance(quit, PlaybackCursor(0));
        let second = runtime.advance(quit, PlaybackCursor(0));
        assert!(first.exit_requested);
        assert_eq!(
            first
                .events
                .iter()
                .filter(|event| matches!(event, PresentationEvent::ExitRequested))
                .count(),
            1
        );
        assert!(!second.exit_requested);
        assert!(!second
            .events
            .iter()
            .any(|event| matches!(event, PresentationEvent::ExitRequested)));
    }

    #[test]
    fn music_cue_order_is_independent_of_playback_backpressure() {
        fn collect(cursors: &[u64]) -> Vec<(u64, u32, bool)> {
            let mut runtime =
                AppRuntime::new(RuntimeConfig::default(), SaveLoadOutcome::Missing).unwrap();
            let mut pcm = [0i16; 44_100];
            runtime.render_audio(&mut pcm);
            let mut cues = Vec::new();
            for &cursor in cursors {
                let effects = runtime.advance(FrameInput::default(), PlaybackCursor(cursor));
                cues.extend(effects.events.iter().filter_map(|event| match event {
                    PresentationEvent::MusicRow(cue) => Some((cue.sample, cue.row, cue.kick)),
                    _ => None,
                }));
            }
            cues
        }

        let smooth: Vec<u64> = (0..=60).map(|tick| tick * 735).collect();
        let bursty = [0, 0, 0, 4_410, 4_410, 14_700, 14_700, 29_400, 44_100];
        assert_eq!(collect(&smooth), collect(&bursty));
    }

    #[test]
    fn render_schedule_does_not_change_session_state() {
        let mut sparse =
            AppRuntime::new(RuntimeConfig::default(), SaveLoadOutcome::Missing).unwrap();
        let mut duplicate =
            AppRuntime::new(RuntimeConfig::default(), SaveLoadOutcome::Missing).unwrap();
        for tick in 0..240u64 {
            let input = FrameInput::default();
            sparse.advance(input, PlaybackCursor(tick * 735));
            duplicate.advance(input, PlaybackCursor(tick * 735));
            if tick.is_multiple_of(3) {
                let _ = sparse.render(0.25);
            }
            let _ = duplicate.render(0.25);
            let _ = duplicate.render(0.75);
        }
        let a = sparse.render(0.0).state;
        let b = duplicate.render(1.0).state;
        assert_eq!(a.screen, b.screen);
        assert_eq!(a.frame, b.frame);
        assert_eq!(a.total_frames, b.total_frames);
        assert_eq!(
            a.world.as_ref().map(|world| world.state_hash()),
            b.world.as_ref().map(|world| world.state_hash())
        );
    }
}
