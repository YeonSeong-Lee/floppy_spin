//! M4-B flow reachability test (SPEC §7 / §12.1): drives `FlowState::advance`
//! with a scripted, fully deterministic input sequence and asserts that
//! every `Screen` variant is visited, every visited screen is also EXITED
//! (no dead ends), MatchOver returns to MainMenu, and a full first-to-4
//! match loop terminates.

use floppy_core::flow::{FlowState, MatchPhase, Screen, MATCH_WIN_POINTS};
use floppy_core::input::InputState;

/// Stable index for each screen "shape" (Match phases counted separately so
/// the reachability assertion covers the whole SPEC §7 diagram).
const SCREEN_COUNT: usize = 12;

fn screen_idx(s: Screen) -> usize {
    match s {
        Screen::Boot => 0,
        Screen::Title => 1,
        Screen::MainMenu => 2,
        Screen::TopSelect => 3,
        Screen::Garage => 4,
        Screen::Settings => 5,
        Screen::Match(MatchPhase::Intro) => 6,
        Screen::Match(MatchPhase::Launch) => 7,
        Screen::Match(MatchPhase::Fight) => 8,
        Screen::Match(MatchPhase::Decided) => 9,
        Screen::Match(MatchPhase::RoundResult) => 10,
        Screen::MatchOver => 11,
    }
}

fn screen_name(i: usize) -> &'static str {
    [
        "Boot",
        "Title",
        "MainMenu",
        "TopSelect",
        "Garage",
        "Settings",
        "Match(Intro)",
        "Match(Launch)",
        "Match(Fight)",
        "Match(Decided)",
        "Match(RoundResult)",
        "MatchOver",
    ][i]
}

/// Test driver: wraps a `FlowState` and records which screens were visited
/// and which were exited (a transition away was observed).
struct Driver {
    flow: FlowState,
    visited: [bool; SCREEN_COUNT],
    exited: [bool; SCREEN_COUNT],
}

fn dash() -> InputState {
    InputState {
        dash: true,
        ..Default::default()
    }
}

fn dir(dx: i8, dy: i8) -> InputState {
    InputState {
        dir_x: dx,
        dir_y: dy,
        ..Default::default()
    }
}

fn guard() -> InputState {
    InputState {
        guard: true,
        ..Default::default()
    }
}

impl Driver {
    fn new(seed: u64) -> Self {
        let flow = FlowState::new(seed);
        let mut d = Driver {
            flow,
            visited: [false; SCREEN_COUNT],
            exited: [false; SCREEN_COUNT],
        };
        d.visited[screen_idx(d.flow.screen)] = true;
        d
    }

    fn step(&mut self, input: InputState) {
        let before = self.flow.screen;
        self.flow.advance(input, false);
        let after = self.flow.screen;
        self.visited[screen_idx(after)] = true;
        if before != after {
            self.exited[screen_idx(before)] = true;
        }
    }

    /// One rising-edge press followed by a release frame (all menu/minigame
    /// actions are edge-triggered).
    fn tap(&mut self, input: InputState) {
        self.step(input);
        self.step(InputState::default());
    }

    /// Advance with a constant input until `pred(screen)` holds, up to `cap`
    /// frames; panics (test failure) if the cap is hit first.
    fn run_until(
        &mut self,
        input: InputState,
        cap: u32,
        pred: impl Fn(Screen) -> bool,
        what: &str,
    ) {
        for _ in 0..cap {
            if pred(self.flow.screen) {
                return;
            }
            self.step(input);
        }
        panic!(
            "run_until({what}) exceeded {cap} frames; stuck on {:?}",
            self.flow.screen
        );
    }

    fn assert_screen(&self, want: Screen, ctx: &str) {
        assert_eq!(self.flow.screen, want, "{ctx}");
    }
}

/// Drive one complete round: Intro countdown → immediate power lock (power
/// ~0 → quality x1.00, so the AI's roll always out-spins P1) → Fight under
/// neutral P1 input against the chasing dummy until decided → RoundResult
/// confirm. Returns whether the match ended (MatchOver) or looped to Intro.
fn play_round(d: &mut Driver) -> bool {
    d.assert_screen(
        Screen::Match(MatchPhase::Intro),
        "round must start at Intro",
    );

    // Intro: input is ignored; hold a key down the whole time to prove it.
    d.run_until(
        dash(),
        200,
        |s| s == Screen::Match(MatchPhase::Launch),
        "intro countdown",
    );

    // Launch minigame: Aim -> SpinDir -> Power -> lock, one edge each.
    // (The held dash during Intro was released by run_until? No — it held
    // dash constantly, so first release it to arm the next rising edge.)
    d.step(InputState::default());
    d.tap(dash()); // Aim -> SpinDir
    d.assert_screen(
        Screen::Match(MatchPhase::Launch),
        "still in Launch after aim lock",
    );
    d.tap(dash()); // SpinDir -> Power
    d.tap(dash()); // Power lock (marker ~0 -> quality x1.00) -> Fight
    d.assert_screen(
        Screen::Match(MatchPhase::Fight),
        "power lock must start the fight",
    );

    // Fight: neutral P1 vs the chasing dummy AI. The AI always launches
    // with more spin (quality/power roll vs P1's 0-power x1.00 lock), so
    // P1 stamina-outs (or gets knocked out) first — every round scores.
    d.run_until(
        InputState::default(),
        200_000,
        |s| s == Screen::Match(MatchPhase::Decided),
        "fight to a decision",
    );

    // Decided: banner hold, input ignored.
    d.run_until(
        InputState::default(),
        200,
        |s| s == Screen::Match(MatchPhase::RoundResult),
        "decided banner hold",
    );

    // RoundResult: any key advances.
    d.tap(dash());
    match d.flow.screen {
        Screen::MatchOver => true,
        Screen::Match(MatchPhase::Intro) => false,
        other => panic!("RoundResult must go to Intro or MatchOver, got {other:?}"),
    }
}

#[test]
fn every_screen_is_reachable_none_is_a_dead_end_and_a_match_terminates() {
    let mut d = Driver::new(0xF10C_A5ED);

    // Boot auto-advances (no input at all).
    d.run_until(
        InputState::default(),
        100,
        |s| s == Screen::Title,
        "boot auto-advance",
    );

    // Title: any key.
    d.tap(dash());
    d.assert_screen(Screen::MainMenu, "any key on Title must open MainMenu");

    // Garage (cursor 0 -> 1, select, back out with guard/Z).
    d.tap(dir(0, 1)); // Down arrow intent = dir_y +1
    d.tap(dash());
    d.assert_screen(Screen::Garage, "menu item 1 must be Garage");
    d.tap(guard());
    d.assert_screen(Screen::MainMenu, "guard must back out of Garage");

    // Settings (cursor 1 -> 2, select, adjust one value, back out).
    d.tap(dir(0, 1));
    d.tap(dash());
    d.assert_screen(Screen::Settings, "menu item 2 must be Settings");
    let music_before = d.flow.settings.music_vol;
    d.tap(dir(-1, 0)); // Right arrow intent = dir_x -1 = increase
    assert_eq!(
        d.flow.settings.music_vol,
        (music_before + 1).min(10),
        "Right on the music row must raise the volume"
    );
    d.tap(dir(1, 0)); // Left = decrease, back to the original value
    assert_eq!(d.flow.settings.music_vol, music_before);
    d.tap(guard());
    d.assert_screen(Screen::MainMenu, "guard must back out of Settings");

    // Back up to QUICK BATTLE (cursor 2 -> 1 -> 0) and enter TopSelect.
    d.tap(dir(0, -1));
    d.tap(dir(0, -1));
    d.tap(dash());
    d.assert_screen(Screen::TopSelect, "QUICK BATTLE must open TopSelect");

    // TopSelect: back out once (exit edge), re-enter, then pick.
    d.tap(guard());
    d.assert_screen(Screen::MainMenu, "guard must back out of TopSelect");
    d.tap(dash());
    d.assert_screen(Screen::TopSelect, "re-entering TopSelect");
    d.tap(dir(0, 1)); // move the cursor once so a non-default pick is exercised
    let cursor = d.flow.select_cursor;
    d.tap(dash());
    d.assert_screen(
        Screen::Match(MatchPhase::Intro),
        "confirming a top must start the match",
    );
    assert_eq!(
        d.flow.p1_pick, cursor,
        "confirm must lock the hovered preset"
    );
    assert!(d.flow.ai_pick < 7, "AI pick must be a valid roster index");
    assert_eq!(d.flow.score, [0, 0], "match must start scoreless");

    // Play rounds until the match ends (first to MATCH_WIN_POINTS). The
    // AI's launch roll always out-spins P1's deliberate 0-power lock, so
    // every round awards at least 1 point to somebody; cap generously.
    let mut rounds = 0;
    loop {
        rounds += 1;
        assert!(
            rounds <= 12,
            "match failed to terminate within 12 rounds (score {:?})",
            d.flow.score
        );
        if play_round(&mut d) {
            break;
        }
    }
    d.assert_screen(Screen::MatchOver, "match loop must end at MatchOver");
    assert!(
        d.flow.score[0] >= MATCH_WIN_POINTS || d.flow.score[1] >= MATCH_WIN_POINTS,
        "MatchOver requires a first-to-{MATCH_WIN_POINTS} score, got {:?}",
        d.flow.score
    );

    // MatchOver returns to MainMenu (SPEC §7).
    d.tap(dash());
    d.assert_screen(Screen::MainMenu, "MatchOver must return to MainMenu");
    assert!(d.flow.world.is_none(), "match teardown must drop the world");

    // Quit exit from MainMenu (menu item 3).
    assert!(!d.flow.quit_requested);
    d.tap(dir(0, 1)); // 0 -> 1
    d.tap(dir(0, 1)); // 1 -> 2
    d.tap(dir(0, 1)); // 2 -> 3 (QUIT)
    d.tap(dash());
    assert!(
        d.flow.quit_requested,
        "selecting QUIT must set quit_requested"
    );

    // Reachability: every screen visited AND exited (no dead ends).
    for i in 0..SCREEN_COUNT {
        assert!(d.visited[i], "screen {} was never visited", screen_name(i));
        assert!(d.exited[i], "screen {} was never exited", screen_name(i));
    }
}

#[test]
fn esc_aborts_a_match_back_to_main_menu() {
    let mut d = Driver::new(1);
    d.run_until(InputState::default(), 100, |s| s == Screen::Title, "boot");
    d.tap(dash()); // -> MainMenu
    d.tap(dash()); // QUICK BATTLE -> TopSelect
    d.tap(dash()); // pick -> Match(Intro)
    d.assert_screen(Screen::Match(MatchPhase::Intro), "in a match");

    // Esc (rising edge) aborts to MainMenu and tears the match down.
    d.flow.advance(InputState::default(), true);
    assert_eq!(d.flow.screen, Screen::MainMenu);
    assert!(d.flow.world.is_none());
    assert!(!d.flow.quit_requested, "match abort must not quit the game");
}

// ---------------------------------------------------------------------------
// FIX 6 (M4 verifier, tests-only): Esc-abort coverage for the four Match
// phases `esc_aborts_a_match_back_to_main_menu` above doesn't reach (it only
// covers Intro). Each helper below drives one phase further than the last,
// mirroring that test's own boot->MainMenu->TopSelect->Intro script.
// ---------------------------------------------------------------------------

/// Boot through TopSelect confirm into a fresh match's Intro (the same
/// script `esc_aborts_a_match_back_to_main_menu` uses).
fn enter_match(d: &mut Driver) {
    d.run_until(InputState::default(), 100, |s| s == Screen::Title, "boot");
    d.tap(dash()); // -> MainMenu
    d.tap(dash()); // QUICK BATTLE -> TopSelect
    d.tap(dash()); // pick -> Match(Intro)
    d.assert_screen(
        Screen::Match(MatchPhase::Intro),
        "match must start at Intro",
    );
}

/// From Intro, run out the countdown into Launch (minigame not yet locked).
fn enter_launch(d: &mut Driver) {
    enter_match(d);
    d.run_until(
        dash(),
        200,
        |s| s == Screen::Match(MatchPhase::Launch),
        "intro countdown",
    );
    d.assert_screen(Screen::Match(MatchPhase::Launch), "must reach Launch");
}

/// From Launch, lock the minigame (Aim -> SpinDir -> Power) into Fight.
fn enter_fight(d: &mut Driver) {
    enter_launch(d);
    d.step(InputState::default()); // release the held dash from the countdown
    d.tap(dash()); // Aim -> SpinDir
    d.tap(dash()); // SpinDir -> Power
    d.tap(dash()); // Power lock -> Fight
    d.assert_screen(Screen::Match(MatchPhase::Fight), "must reach Fight");
}

/// From Fight, run the scripted chase-AI fight to a decision.
fn enter_decided(d: &mut Driver) {
    enter_fight(d);
    d.run_until(
        InputState::default(),
        200_000,
        |s| s == Screen::Match(MatchPhase::Decided),
        "fight to a decision",
    );
    d.assert_screen(Screen::Match(MatchPhase::Decided), "must reach Decided");
}

/// From Decided, wait out the banner hold into RoundResult.
fn enter_round_result(d: &mut Driver) {
    enter_decided(d);
    d.run_until(
        InputState::default(),
        200,
        |s| s == Screen::Match(MatchPhase::RoundResult),
        "decided banner hold",
    );
    d.assert_screen(
        Screen::Match(MatchPhase::RoundResult),
        "must reach RoundResult",
    );
}

#[test]
fn esc_aborts_launch_phase_back_to_main_menu() {
    let mut d = Driver::new(2);
    enter_launch(&mut d);
    d.flow.advance(InputState::default(), true);
    assert_eq!(d.flow.screen, Screen::MainMenu);
    assert!(d.flow.world.is_none());
    assert!(!d.flow.quit_requested, "match abort must not quit the game");
}

#[test]
fn esc_aborts_fight_phase_back_to_main_menu() {
    let mut d = Driver::new(3);
    enter_fight(&mut d);
    d.flow.advance(InputState::default(), true);
    assert_eq!(d.flow.screen, Screen::MainMenu);
    assert!(d.flow.world.is_none());
    assert!(!d.flow.quit_requested, "match abort must not quit the game");
}

#[test]
fn esc_aborts_decided_phase_back_to_main_menu() {
    let mut d = Driver::new(4);
    enter_decided(&mut d);
    d.flow.advance(InputState::default(), true);
    assert_eq!(d.flow.screen, Screen::MainMenu);
    assert!(d.flow.world.is_none());
    assert!(!d.flow.quit_requested, "match abort must not quit the game");
}

#[test]
fn esc_aborts_round_result_phase_back_to_main_menu() {
    let mut d = Driver::new(5);
    enter_round_result(&mut d);
    d.flow.advance(InputState::default(), true);
    assert_eq!(d.flow.screen, Screen::MainMenu);
    assert!(d.flow.world.is_none());
    assert!(!d.flow.quit_requested, "match abort must not quit the game");
}

#[test]
fn identical_scripts_replay_to_identical_flow_state() {
    // The whole UI is deterministic: two flows fed the same script must
    // agree on screen, seeds, picks, and score at every frame.
    fn script(seed: u64) -> (Screen, u64, usize, [u8; 2]) {
        let mut d = Driver::new(seed);
        d.run_until(InputState::default(), 100, |s| s == Screen::Title, "boot");
        d.tap(dash());
        d.tap(dash());
        d.tap(dir(0, 1));
        d.tap(dash()); // pick preset 1 -> Intro
        d.run_until(
            InputState::default(),
            200,
            |s| s == Screen::Match(MatchPhase::Launch),
            "countdown",
        );
        d.tap(dash());
        d.tap(dash());
        d.tap(dash()); // lock everything -> Fight
        for _ in 0..600 {
            d.step(InputState::default());
        }
        (
            d.flow.screen,
            d.flow.match_seed,
            d.flow.ai_pick,
            d.flow.score,
        )
    }
    assert_eq!(script(42), script(42));
}

/// `FlowState::frame_events` must accumulate events across BOTH of a flow
/// frame's sim sub-steps (M6-B finding: `World::step` clears `World::events`
/// per call, so reading `world.events` after `advance()` silently drops
/// everything the first sub-step emitted — audible as missing hit SFX).
/// This drives a real fight and requires at least one frame where the
/// accumulated buffer is strictly larger than the surviving `world.events`,
/// i.e. the fix demonstrably captured events the old path lost. It also
/// checks the buffer clears outside Fight (no stale events leak into menus).
#[test]
fn frame_events_accumulate_across_sub_steps_and_clear_outside_fight() {
    let mut d = Driver::new(7);
    enter_fight(&mut d);

    let mut saw_accumulation_beat_last_substep = false;
    let mut total_frame_events = 0usize;
    for _ in 0..200_000 {
        if d.flow.screen != Screen::Match(MatchPhase::Fight) {
            break;
        }
        d.step(InputState::default());
        total_frame_events += d.flow.frame_events.len();
        let surviving = d.flow.world.as_ref().map_or(0, |w| w.events.len());
        assert!(
            d.flow.frame_events.len() >= surviving,
            "frame_events must be a superset of the last sub-step's events"
        );
        if d.flow.frame_events.len() > surviving {
            saw_accumulation_beat_last_substep = true;
        }
    }
    assert!(
        total_frame_events > 0,
        "a full fight must produce BattleEvents"
    );
    assert!(
        saw_accumulation_beat_last_substep,
        "no frame ever carried a first-sub-step event; either this seed's \
         fight is degenerate (pick another) or accumulation is broken"
    );

    // Outside Fight the buffer clears on the next advance.
    d.run_until(
        InputState::default(),
        400,
        |s| s == Screen::Match(MatchPhase::RoundResult),
        "banner hold to RoundResult",
    );
    assert!(
        d.flow.frame_events.is_empty(),
        "frame_events must clear once no sim sub-steps run"
    );
}
