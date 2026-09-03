//! Private game aggregate with an exactly-once output interface.

use crate::flow::{FlowState, GameSettings, Screen};
use crate::hash::Hasher64;
use crate::input::FrameInput;
use crate::physics::BattleEvent;
use crate::save::SaveState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionDigest(pub u64);

const MAX_BATTLE_EVENTS_PER_TICK: usize = 64;

#[derive(Debug)]
pub struct BattleEvents {
    events: [Option<BattleEvent>; MAX_BATTLE_EVENTS_PER_TICK],
    len: usize,
}

impl BattleEvents {
    pub fn iter(&self) -> impl Iterator<Item = BattleEvent> + '_ {
        self.events[..self.len].iter().filter_map(|event| *event)
    }
}

#[derive(Debug)]
pub struct SessionOutput {
    pub transition: Option<(Screen, Screen)>,
    pub battle_events: BattleEvents,
    pub quit_requested: bool,
}

pub struct GameSnapshot<'a> {
    state: &'a FlowState,
}

impl<'a> GameSnapshot<'a> {
    /// Presentation-only compatibility view. Mutation remains inside the
    /// aggregate and callers cannot retain this across `advance`.
    pub fn presentation(&self) -> &'a FlowState {
        self.state
    }
}

pub struct GameSession {
    state: FlowState,
    quit_delivered: bool,
}

impl GameSession {
    pub fn new(seed: u64, save: SaveState) -> Self {
        let mut state = FlowState::new(seed);
        state.apply_save(save);
        Self {
            state,
            quit_delivered: false,
        }
    }

    pub fn advance(&mut self, input: FrameInput) -> SessionOutput {
        let from = self.state.screen;
        self.state.advance_frame(input);
        let to = self.state.screen;
        let mut battle_events = BattleEvents {
            events: [None; MAX_BATTLE_EVENTS_PER_TICK],
            len: 0,
        };
        for event in self.state.frame_events.drain(..) {
            assert!(
                battle_events.len < MAX_BATTLE_EVENTS_PER_TICK,
                "battle event capacity exceeded"
            );
            battle_events.events[battle_events.len] = Some(event);
            battle_events.len += 1;
        }
        let quit_requested = self.state.quit_requested && !self.quit_delivered;
        self.quit_delivered |= quit_requested;
        SessionOutput {
            transition: (from != to).then_some((from, to)),
            battle_events,
            quit_requested,
        }
    }

    pub fn snapshot(&self) -> GameSnapshot<'_> {
        GameSnapshot { state: &self.state }
    }

    pub fn digest(&self) -> SessionDigest {
        let mut hash = Hasher64::default();
        self.state.write_digest(&mut hash);
        hash.write_bool(self.quit_delivered);
        SessionDigest(hash.finish())
    }

    pub fn save_snapshot(&self) -> ([u8; 5], GameSettings) {
        self.state.save_snapshot()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_events_are_delivered_exactly_once() {
        let mut session = GameSession::new(7, SaveState::default());
        let first = session.advance(FrameInput::default());
        let second = session.advance(FrameInput::default());
        assert_eq!(first.battle_events.iter().count(), 0);
        assert_eq!(second.battle_events.iter().count(), 0);
        assert_ne!(session.digest().0, 0);
    }

    #[test]
    fn terminal_event_is_delivered_exactly_once() {
        let mut session = GameSession::new(7, SaveState::default());
        for _ in 0..=crate::flow::BOOT_FRAMES {
            session.advance(FrameInput::default());
        }
        let quit = FrameInput {
            escape_pressed: true,
            ..FrameInput::default()
        };
        assert!(session.advance(quit).quit_requested);
        assert!(!session.advance(quit).quit_requested);
        assert!(!session.advance(FrameInput::default()).quit_requested);
    }
}
