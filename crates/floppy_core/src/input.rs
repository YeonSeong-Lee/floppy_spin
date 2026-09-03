//! `InputState` (SPEC §6.4): the *only* way anything — human or AI — drives
//! the sim. `pack`/`unpack` give a stable 16-bit replay/scripting format.

/// One frame's worth of player intent. `dir_x`/`dir_y` are camera-relative
/// digital directions in `{-1, 0, 1}`. During the Launch phase these fields
/// are reinterpreted (see game_design.md §Launch) but the wire shape never
/// changes.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct InputState {
    pub dir_x: i8,
    pub dir_y: i8,
    pub dash: bool,
    pub special: bool,
    pub guard: bool,
    pub hop: bool,
    pub carve: bool,
    pub anchor: bool,
}

/// Host input sampled for one 60 Hz application tick.
///
/// `held` is the state at the end of the host poll. `pressed` and
/// `released` are latches: both may be true when a key was tapped entirely
/// between two ticks. This is intentionally independent of platform key
/// codes so replay, headless, and window adapters share one contract.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FrameInput {
    pub held: InputState,
    pub pressed: InputState,
    pub released: InputState,
    pub escape_held: bool,
    pub escape_pressed: bool,
    pub escape_released: bool,
    pub focused: bool,
}

impl Default for FrameInput {
    fn default() -> Self {
        Self {
            held: InputState::default(),
            pressed: InputState::default(),
            released: InputState::default(),
            escape_held: false,
            escape_pressed: false,
            escape_released: false,
            // A synthetic/headless host is focused unless it explicitly
            // reports otherwise. This also keeps `FrameInput::default()` a
            // useful neutral input for deterministic tests.
            focused: true,
        }
    }
}

impl FrameInput {
    /// Compatibility constructor for callers that sample only held state.
    /// New code should preserve host edge latches in `pressed`/`released`.
    pub fn from_held(held: InputState, escape_held: bool) -> Self {
        Self {
            held,
            escape_held,
            focused: true,
            ..Self::default()
        }
    }

    /// Input delivered to a simulation substep. Directional and continuous
    /// actions remain held, while one-shot actions exist only on substep 0.
    pub fn for_substep(self, first: bool) -> InputState {
        let mut input = self.held;
        if first {
            input.dash |= self.pressed.dash;
            input.special |= self.pressed.special;
            input.hop |= self.pressed.hop;
        } else {
            input.dash = false;
            input.special = false;
            input.hop = false;
        }
        input
    }
}

/// Bit layout of the packed replay/scripting format (16 bits total):
/// ```text
/// bit:  0 1 | 2 3 | 4    | 5       | 6     | 7   | 8     | 9      | 10..15
/// field: dx |  dy | dash | special | guard | hop | carve | anchor | unused (0)
/// ```
/// Each direction field is 2 bits: `0 = -1`, `1 = 0`, `2 = +1`, `3` unused
/// (decodes as `0`, i.e. neutral). Unused/reserved bits are always packed as
/// `0` and ignored on unpack.
fn dir_to_bits(d: i8) -> u16 {
    match d {
        -1 => 0,
        1 => 2,
        // 0, and any out-of-range value, packs as neutral. Digital directions
        // are only ever -1/0/1 in practice; this keeps pack() total.
        _ => 1,
    }
}

fn bits_to_dir(bits: u16) -> i8 {
    match bits & 0b11 {
        0 => -1,
        2 => 1,
        // 1 (neutral) and 3 (unused code) both decode to neutral.
        _ => 0,
    }
}

impl InputState {
    pub fn pack(self) -> u16 {
        let mut v = dir_to_bits(self.dir_x) | (dir_to_bits(self.dir_y) << 2);
        if self.dash {
            v |= 1 << 4;
        }
        if self.special {
            v |= 1 << 5;
        }
        if self.guard {
            v |= 1 << 6;
        }
        if self.hop {
            v |= 1 << 7;
        }
        if self.carve {
            v |= 1 << 8;
        }
        if self.anchor {
            v |= 1 << 9;
        }
        v
    }

    pub fn unpack(bits: u16) -> Self {
        Self {
            dir_x: bits_to_dir(bits),
            dir_y: bits_to_dir(bits >> 2),
            dash: bits & (1 << 4) != 0,
            special: bits & (1 << 5) != 0,
            guard: bits & (1 << 6) != 0,
            hop: bits & (1 << 7) != 0,
            carve: bits & (1 << 8) != 0,
            anchor: bits & (1 << 9) != 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_unpack_roundtrip_over_dirs_and_button_masks() {
        let dirs: [i8; 3] = [-1, 0, 1];
        let button_masks: [(bool, bool, bool, bool, bool, bool); 5] = [
            (false, false, false, false, false, false),
            (true, false, false, false, false, false),
            (false, true, true, false, false, false),
            (true, true, true, true, true, true),
            (false, false, false, true, false, true),
        ];

        for &dx in &dirs {
            for &dy in &dirs {
                for &(dash, special, guard, hop, carve, anchor) in &button_masks {
                    let original = InputState {
                        dir_x: dx,
                        dir_y: dy,
                        dash,
                        special,
                        guard,
                        hop,
                        carve,
                        anchor,
                    };
                    let roundtripped = InputState::unpack(original.pack());
                    assert_eq!(roundtripped, original);
                }
            }
        }
    }

    #[test]
    fn unused_high_bits_are_ignored_on_unpack() {
        let base = InputState {
            dir_x: 1,
            dir_y: -1,
            dash: true,
            ..Default::default()
        };
        let packed = base.pack();
        let with_garbage = packed | 0xFC00; // set all reserved bits 10..15
        assert_eq!(InputState::unpack(with_garbage), base);
    }

    #[test]
    fn one_shot_actions_only_reach_the_first_substep() {
        let frame = FrameInput {
            held: InputState {
                guard: true,
                carve: true,
                ..InputState::default()
            },
            pressed: InputState {
                dash: true,
                special: true,
                hop: true,
                ..InputState::default()
            },
            focused: true,
            ..FrameInput::default()
        };
        let first = frame.for_substep(true);
        let second = frame.for_substep(false);
        assert!(first.dash && first.special && first.hop);
        assert!(!second.dash && !second.special && !second.hop);
        assert!(second.guard && second.carve);
    }
}
