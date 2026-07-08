//! Small render-side VFX state helpers that don't belong to the particle
//! pool or the post-process composite pass: screen shake accumulation
//! (Task 3), flash pulses (Task 3), the arena ring-pulse decay (Task 4), and
//! choreography easing (menu cursor spring, countdown/banner overshoot,
//! title jitter, colorblind remap, screen-wipe transitions — Task 5).
//!
//! Every struct here is plain data updated by an explicit `step`/`update`
//! call the caller (`main.rs`) drives once per flow frame (60 Hz) — nothing
//! reads wall-clock time, and the only randomness ([`ShakeState::offset`])
//! takes an explicit `&mut Rng` the caller owns (SPEC §5 "HARD RULES": a
//! dedicated render-side stream, never `World`'s own `rng`).

use crate::frame::Frame;
use crate::hud::COL_CURSOR;
use floppy_core::fixmath;
use floppy_core::flow::ShakeLevel;
use floppy_core::rng::Rng;
use floppy_core::vec::Vec3;

// ---------------------------------------------------------------------------
// Task 3: screen shake.
// ---------------------------------------------------------------------------

/// Settings `ShakeLevel` -> multiplier (game_design.md §5: "x0/0.5/1/1.5").
pub fn shake_level_mult(level: ShakeLevel) -> f32 {
    match level {
        ShakeLevel::Off => 0.0,
        ShakeLevel::Low => 0.5,
        ShakeLevel::Normal => 1.0,
        ShakeLevel::High => 1.5,
    }
}

/// Hard clamp on the SUMMED shake amplitude before the settings multiplier
/// is applied (game_design.md §5: "summed, clamped 14 px, scaled by the
/// settings multiplier" — so High's x1.5 can still push the final offset
/// past 14px, deliberately).
const SHAKE_CLAMP_PX: f32 = 14.0;

/// Accumulated screen-shake amplitude + its current decay rate (milestone
/// brief: `ShakeState { amp, decay }`). Multiple events landing the same
/// frame SUM their amplitude (clamped) but the most recently added event's
/// decay rate wins — a documented simplification for the rare case of two
/// differently-decaying shakes overlapping.
#[derive(Clone, Copy, Debug, Default)]
pub struct ShakeState {
    pub amp: f32,
    pub decay: f32,
}

impl ShakeState {
    /// Add one juice-table `(amp, decay)` entry (call once per triggering
    /// `BattleEvent` this frame, in event order).
    pub fn add(&mut self, amp: f32, decay: f32) {
        self.amp = (self.amp + amp).min(SHAKE_CLAMP_PX);
        self.decay = decay;
    }

    /// Decay for the next frame (call exactly once per flow frame, after
    /// this frame's `offset` has been consumed).
    pub fn step(&mut self) {
        self.amp *= self.decay;
        if self.amp < 0.05 {
            self.amp = 0.0;
        }
    }

    /// This frame's whole-pixel screen offset: a deterministic pseudo-random
    /// direction drawn from the caller's render-side `rng`, magnitude
    /// `self.amp * level_mult`, truncated to whole pixels (module docs).
    pub fn offset(&self, rng: &mut Rng, level_mult: f32) -> (f32, f32) {
        let amp = self.amp * level_mult;
        if amp <= 0.0 {
            return (0.0, 0.0);
        }
        let dx = (rng.next_f32() * 2.0 - 1.0) * amp;
        let dy = (rng.next_f32() * 2.0 - 1.0) * amp;
        (dx as i32 as f32, dy as i32 as f32)
    }
}

// ---------------------------------------------------------------------------
// Task 3: flash pulses (full-screen additive tint).
// ---------------------------------------------------------------------------

/// Fixed small pool of concurrently-fading flash pulses (same round-robin,
/// no-Vec-growth pattern as `particles::ParticlePool`; four slots is far
/// more than the juice table ever fires in one frame).
const FLASH_SLOTS: usize = 4;

#[derive(Clone, Copy, Debug)]
struct FlashPulse {
    color: Vec3,
    t: f32,
    duration: f32,
}

const EMPTY_PULSE: FlashPulse = FlashPulse {
    color: Vec3::new(0.0, 0.0, 0.0),
    t: 0.0,
    duration: 0.0,
};

pub struct FlashState {
    pulses: [FlashPulse; FLASH_SLOTS],
    next: usize,
}

impl Default for FlashState {
    fn default() -> Self {
        Self::new()
    }
}

impl FlashState {
    pub fn new() -> Self {
        Self {
            pulses: [EMPTY_PULSE; FLASH_SLOTS],
            next: 0,
        }
    }

    /// Trigger one juice-table flash entry: `color` at full intensity,
    /// linearly fading to zero over `duration_frames`.
    pub fn add(&mut self, color: Vec3, duration_frames: f32) {
        self.pulses[self.next] = FlashPulse {
            color,
            t: 0.0,
            duration: duration_frames.max(1.0),
        };
        self.next = (self.next + 1) % FLASH_SLOTS;
    }

    /// This frame's combined flash color (sum of every active pulse's
    /// `color * (1 - t/duration)`), ready to hand straight to
    /// `post::PostState::composite`.
    pub fn current(&self) -> Vec3 {
        let mut sum = Vec3::default();
        for p in &self.pulses {
            if p.t < p.duration {
                let k = (1.0 - p.t / p.duration).max(0.0);
                sum = sum + p.color * k;
            }
        }
        sum
    }

    /// Age every active pulse by one flow frame (call once per frame, after
    /// `current()` has been consumed).
    pub fn step(&mut self) {
        for p in self.pulses.iter_mut() {
            if p.t < p.duration {
                p.t += 1.0;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Task 4: arena ring pulse.
// ---------------------------------------------------------------------------

/// One frame's ring-pulse update (game_design.md §6: "kick rows set
/// `ring_pulse = 1.0`, decay x0.88/frame"). `kicked` must be an EDGE (the
/// tracker's row index just changed onto a kick row this frame), not a
/// level — see `main.rs`'s wiring, which compares `Tracker::row_index()`
/// across frames.
pub fn ring_pulse_step(current: f32, kicked: bool) -> f32 {
    if kicked {
        1.0
    } else {
        current * 0.88
    }
}

// ---------------------------------------------------------------------------
// Task 5: menu cursor spring settle (~120 ms).
// ---------------------------------------------------------------------------

/// Exponential-approach cursor position, in fractional ROW-INDEX units (not
/// pixels — the caller multiplies by its own row height), so one `Spring`
/// works for any menu's layout. `RATE` is chosen so the gap to target closes
/// to ~2% within 7 frames (~117 ms @ 60 Hz, matching the "~120 ms" brief):
/// `(1-RATE)^7 ~= 0.02` gives `RATE ~= 0.42`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Spring {
    pub value: f32,
}

impl Spring {
    pub fn new(initial: f32) -> Self {
        Self { value: initial }
    }

    const RATE: f32 = 0.42;

    pub fn ease_toward(&mut self, target: f32) {
        self.value += (target - self.value) * Self::RATE;
    }
}

// ---------------------------------------------------------------------------
// Task 5: overshoot-settle spring (countdown numerals, outcome banners).
// ---------------------------------------------------------------------------

/// Discrete critically-damped-ish spring-damper (semi-implicit Euler: only
/// `+ - *`, no `exp`/`powf` — those aren't in `fixmath`'s exported set, so a
/// closed-form damped-sinusoid isn't available outside it). `snap` starts a
/// fresh overshoot (e.g. banner/numeral pop-in at 1.5x/1.3x); `step` walks
/// one frame closer to `target`, tuned so a 1.5x start settles within ~10
/// frames (game_design.md §7: "settle 10 frames").
#[derive(Clone, Copy, Debug, Default)]
pub struct OvershootSpring {
    pub value: f32,
    vel: f32,
}

impl OvershootSpring {
    const STIFFNESS: f32 = 0.15;
    const DAMPING: f32 = 0.6;

    pub fn snap(&mut self, value: f32) {
        self.value = value;
        self.vel = 0.0;
    }

    pub fn step(&mut self, target: f32) {
        let accel = (target - self.value) * Self::STIFFNESS;
        self.vel = (self.vel + accel) * Self::DAMPING;
        self.value += self.vel;
    }
}

// ---------------------------------------------------------------------------
// Task 5: title drop-shadow jitter.
// ---------------------------------------------------------------------------

/// Deterministic +-0.5px sine jitter for the title's magenta drop-shadow
/// (game_design.md §7), a pure function of the render-side frame counter
/// (never wall-clock). The bitmap font has no sub-pixel rendering, so the
/// continuous +-0.5px signal is quantized to an extra whole pixel of offset
/// whenever it's positive (documented approximation — see `hud::draw_title`).
pub fn title_shadow_jitter_px(ui_frame: u32) -> i32 {
    const SPEED: f32 = 0.07;
    let s = fixmath::sin(ui_frame as f32 * SPEED);
    if s > 0.0 {
        1
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// Task 5: colorblind remap (game_design.md §7).
// ---------------------------------------------------------------------------

const LIME: u32 = 0x0039_FF14; // Everspin's accent (game_design.md §3).
const ICE_BLUE: u32 = 0x0000_BFFF;
const ORANGE: u32 = 0x00FF_7A00; // Mirrorfang's accent (game_design.md §3).
const AMBER_WHITE: u32 = 0x00FF_D9A0;

/// Remap the two risky accent pairs (game_design.md §7: "lime -> ice-blue,
/// orange -> amber-white"); every other color passes through unchanged.
/// `enabled == false` is a no-op (so call sites can pass `settings.colorblind`
/// directly without an `if`).
pub fn colorblind_remap(color: u32, enabled: bool) -> u32 {
    if !enabled {
        return color;
    }
    match color {
        LIME => ICE_BLUE,
        ORANGE => AMBER_WHITE,
        other => other,
    }
}

// ---------------------------------------------------------------------------
// Task 5: screen-transition diagonal neon wipe.
// ---------------------------------------------------------------------------

/// Wipe duration (game_design.md §7: "diagonal neon wipe, 9 frames").
pub const WIPE_FRAMES: u32 = 9;

/// Draw one frame of the diagonal wipe-reveal transition: pixels the
/// sweeping diagonal band hasn't reached yet are covered to black; pixels
/// right at the band's leading edge get a bright neon tint; pixels already
/// past the band are left showing whatever the new screen already drew
/// there. `k` is frames-since-transition (`0..WIPE_FRAMES`); callers outside
/// that range should simply not call this (module docs: main.rs tracks its
/// own `wipe_frame` counter and only calls this while it's live).
pub fn draw_wipe(frame: &mut Frame, k: u32) {
    if k >= WIPE_FRAMES {
        return;
    }
    let w = frame.w as i32;
    let h = frame.h as i32;
    let total = (w + h) as f32;
    let threshold = (k as f32 + 1.0) / WIPE_FRAMES as f32 * total;
    const BAND: f32 = 36.0;

    for y in 0..h {
        for x in 0..w {
            let d = (x + y) as f32;
            if d > threshold + BAND {
                frame.set(x, y, 0x0000_0000);
            } else if d > threshold - BAND {
                frame.set(x, y, COL_CURSOR);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shake_state_sums_and_clamps_then_decays() {
        let mut s = ShakeState::default();
        s.add(7.0, 0.8);
        s.add(9.0, 0.9);
        assert_eq!(s.amp, SHAKE_CLAMP_PX, "7+9=16 should clamp to 14");
        assert_eq!(s.decay, 0.9);
        s.step();
        assert!((s.amp - 14.0 * 0.9).abs() < 1e-4);
    }

    #[test]
    fn shake_offset_is_deterministic_for_the_same_rng_state() {
        let mut s = ShakeState::default();
        s.add(10.0, 0.8);
        let mut rng_a = Rng::new(123);
        let mut rng_b = Rng::new(123);
        assert_eq!(s.offset(&mut rng_a, 1.0), s.offset(&mut rng_b, 1.0));
    }

    #[test]
    fn shake_offset_is_zero_at_zero_amplitude_or_off_level() {
        let s = ShakeState::default();
        let mut rng = Rng::new(1);
        assert_eq!(s.offset(&mut rng, 1.0), (0.0, 0.0));

        let mut s2 = ShakeState::default();
        s2.add(10.0, 0.8);
        let mut rng2 = Rng::new(1);
        assert_eq!(s2.offset(&mut rng2, 0.0), (0.0, 0.0));
    }

    #[test]
    fn flash_state_fades_linearly_to_zero() {
        let mut f = FlashState::new();
        f.add(Vec3::new(1.0, 0.0, 0.0), 4.0);
        let c0 = f.current();
        assert!((c0.x - 1.0).abs() < 1e-5);
        f.step();
        f.step();
        let c_mid = f.current();
        assert!((c_mid.x - 0.5).abs() < 1e-4, "c_mid={c_mid:?}");
        f.step();
        f.step();
        f.step();
        let c_done = f.current();
        assert_eq!(c_done, Vec3::default());
    }

    #[test]
    fn ring_pulse_kicks_to_one_and_decays_otherwise() {
        assert_eq!(ring_pulse_step(0.3, true), 1.0);
        assert!((ring_pulse_step(1.0, false) - 0.88).abs() < 1e-6);
    }

    #[test]
    fn spring_settles_toward_target_monotonically_here() {
        let mut sp = Spring::new(0.0);
        for _ in 0..30 {
            sp.ease_toward(3.0);
        }
        assert!((sp.value - 3.0).abs() < 0.01, "value={}", sp.value);
    }

    #[test]
    fn overshoot_spring_settles_within_ten_frames() {
        let mut sp = OvershootSpring::default();
        sp.snap(1.5);
        for _ in 0..10 {
            sp.step(1.0);
        }
        assert!(
            (sp.value - 1.0).abs() < 0.05,
            "expected settle near 1.0, got {}",
            sp.value
        );
    }

    #[test]
    fn colorblind_remap_only_touches_the_two_documented_accents() {
        assert_eq!(colorblind_remap(LIME, true), ICE_BLUE);
        assert_eq!(colorblind_remap(ORANGE, true), AMBER_WHITE);
        assert_eq!(colorblind_remap(0x00_ABCDEF, true), 0x00_ABCDEF);
        assert_eq!(colorblind_remap(LIME, false), LIME);
    }

    #[test]
    fn draw_wipe_is_deterministic_and_a_noop_past_wipe_frames() {
        let mut f1 = Frame::new(32, 32);
        draw_wipe(&mut f1, 0);
        let mut f2 = Frame::new(32, 32);
        draw_wipe(&mut f2, 0);
        assert_eq!(f1.px, f2.px);

        let mut f3 = Frame::new(32, 32);
        f3.clear(0x00123456);
        draw_wipe(&mut f3, WIPE_FRAMES);
        assert!(f3.px.iter().all(|&p| p == 0x00123456));
    }
}
