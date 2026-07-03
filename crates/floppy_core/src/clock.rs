//! Fixed-timestep accumulator (SPEC §5): physics always runs at a constant
//! 120 Hz regardless of the display's frame rate. Wall-clock time never
//! enters `core` — callers measure `frame_dt_s` themselves (from
//! `platform::win32` or the headless driver) and hand it in as a plain
//! number.

/// Fixed simulation rate.
pub const SIM_HZ: u32 = 120;
/// Fixed simulation timestep in seconds.
pub const SIM_DT: f32 = 1.0 / 120.0;

/// Accumulates wall/frame time and reports how many whole 120 Hz steps to
/// run. Uses `f64` internally so the accumulator doesn't lose precision over
/// a long play session.
#[derive(Clone, Copy, Debug, Default)]
pub struct SimClock {
    acc: f64,
}

impl SimClock {
    pub const fn new() -> Self {
        Self { acc: 0.0 }
    }

    /// Add `frame_dt_s` seconds to the accumulator and return how many whole
    /// simulation steps should run, capped at `max_steps`.
    ///
    /// The cap is an anti-"spiral of death" guard: if a frame stalls (e.g.
    /// a debugger pause, or the window being dragged), the accumulator can
    /// build up an arbitrarily large backlog of steps. Blindly running all of
    /// them would make the game hitch even harder trying to "catch up". So
    /// `advance` always **drains the full accumulated time** it saw this
    /// call — including the part beyond `max_steps` — even though it only
    /// reports `max_steps` steps to actually run. The excess time is dropped
    /// on the floor rather than remembered, so a single slow frame costs one
    /// visible hitch and nothing more; it can never accrue unboundedly.
    ///
    /// Boundary clamps (M1 verifier findings): a negative `frame_dt_s`
    /// (timer anomaly / caller bug) is ignored rather than driving the
    /// accumulator negative and silently suppressing future steps, and the
    /// backlog is bounded at 1 second, which also keeps the `f64 → u32` cast
    /// below far from saturation so the drain invariant holds exactly.
    pub fn advance(&mut self, frame_dt_s: f64, max_steps: u32) -> u32 {
        self.acc += frame_dt_s.max(0.0);
        if self.acc > 1.0 {
            self.acc = 1.0;
        }
        let step_dt = 1.0 / (SIM_HZ as f64);
        let available = (self.acc / step_dt) as u32;
        self.acc -= (available as f64) * step_dt;
        available.min(max_steps)
    }

    /// Fraction (in `[0, 1)`) of a step remaining in the accumulator — the
    /// interpolation factor `render::draw` uses to blend the two most recent
    /// sim states (SPEC §5). Headless golden frames instead force `alpha =
    /// 1.0` at the call site.
    pub fn alpha(&self) -> f32 {
        let step_dt = 1.0 / (SIM_HZ as f64);
        if step_dt <= 0.0 {
            return 0.0;
        }
        let a = (self.acc / step_dt) as f32;
        a.clamp(0.0, 0.999_999_9)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sixty_frames_per_second_yields_two_steps_each() {
        let mut clock = SimClock::new();
        let mut total = 0u32;
        for _ in 0..100 {
            total += clock.advance(1.0 / 60.0, 8);
        }
        assert_eq!(total, 200);
    }

    #[test]
    fn alpha_stays_in_zero_one_range() {
        let mut clock = SimClock::new();
        for _ in 0..50 {
            let _ = clock.advance(1.0 / 90.0, 8);
            let a = clock.alpha();
            assert!((0.0..1.0).contains(&a), "alpha={a}");
        }
    }

    #[test]
    fn negative_dt_is_ignored_not_banked() {
        let mut clock = SimClock::new();
        assert_eq!(clock.advance(-5.0, 8), 0);
        // A negative delta must not create a deficit that swallows the next
        // frames: the very next 1/60 s frame still yields its 2 steps.
        assert_eq!(clock.advance(1.0 / 60.0, 8), 2);
    }

    #[test]
    fn huge_stall_cannot_saturate_the_cast() {
        let mut clock = SimClock::new();
        // Backlog is clamped to 1 s (120 steps) internally; a preposterous
        // dt neither panics nor leaves residue beyond one step.
        let steps = clock.advance(1.0e12, 8);
        assert_eq!(steps, 8);
        assert!(clock.advance(0.0, 1_000) <= 1);
    }

    #[test]
    fn one_second_stall_capped_and_drained() {
        let mut clock = SimClock::new();
        let steps = clock.advance(1.0, 8);
        assert_eq!(steps, 8);
        // The ~112 steps beyond the cap must be drained, not remembered: a
        // zero-time probe call must not suddenly reveal a huge backlog. (A
        // remainder of at most one step from floor-division rounding is
        // acceptable; anything close to the original 112-step excess is not.)
        let probe_steps = clock.advance(0.0, 1_000);
        assert!(probe_steps <= 1, "probe_steps={probe_steps}");
    }
}
