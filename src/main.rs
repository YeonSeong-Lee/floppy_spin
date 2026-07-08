// Game binary. Console subsystem in debug builds only (println! debugging).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod platform;

use floppy_audio::{on_event, play, Mixer, Sfx, SongId, Tracker};
use floppy_core::clock::SimClock;
use floppy_core::flow::{self, FlowState, MatchPhase, Screen, MATCH_WIN_POINTS};
use floppy_core::input::InputState;
use floppy_core::physics::TUNE;
use floppy_core::roster::PRESETS;
use floppy_render::battle::BattleScene;
use floppy_render::frame::Frame;
use floppy_render::hud;
use platform::win32::{Platform, AUDIO_BUFFER_FRAMES, VK_ESCAPE};

const W: usize = 960;
const H: usize = 540;
const FRAME_DT: f64 = 1.0 / 60.0;
/// Spin-wait the tail of the frame instead of sleeping through it, so pacing
/// lands on the 16.666ms boundary precisely; Sleep()'s ~1ms granularity can't.
const SPIN_MARGIN_S: f64 = 0.0015;

// Virtual-key codes used by the game (plain u8 values, no FFI — the actual
// key polling lives behind platform::win32).
const VK_LEFT: u8 = 0x25;
const VK_UP: u8 = 0x26;
const VK_RIGHT: u8 = 0x27;
const VK_DOWN: u8 = 0x28;
const VK_SPACE: u8 = 0x20;
const VK_SHIFT: u8 = 0x10;
const VK_CONTROL: u8 = 0x11;
const VK_Z: u8 = 0x5A;
const VK_X: u8 = 0x58;
const VK_C: u8 = 0x43;

/// Build the camera-relative `InputState` from raw key state.
///
/// Direction signs compensate for the fixed M3 battle camera's mirrored
/// basis (see `floppy_core::flow`'s "Arrow-key intent convention" docs):
/// pressing Right must move the top toward screen-right, which is world
/// `-x`, i.e. `dir_x = -1`; pressing Up must move up-screen (away from the
/// camera, world `+z`), i.e. `dir_y = -1`. Menu logic in `flow` is written
/// against the same constants, so arrows behave naturally everywhere.
fn read_input(p: &Platform) -> InputState {
    let mut dir_x: i8 = 0;
    let mut dir_y: i8 = 0;
    if p.key(VK_LEFT) {
        dir_x += flow::DIR_LEFT;
    }
    if p.key(VK_RIGHT) {
        dir_x += flow::DIR_RIGHT;
    }
    if p.key(VK_UP) {
        dir_y += flow::DIR_UP;
    }
    if p.key(VK_DOWN) {
        dir_y += flow::DIR_DOWN;
    }
    InputState {
        dir_x,
        dir_y,
        dash: p.key(VK_SPACE),
        special: p.key(VK_SHIFT),
        guard: p.key(VK_Z),
        hop: p.key(VK_X),
        carve: p.key(VK_C),
        anchor: p.key(VK_CONTROL),
    }
}

/// Draw the whole game for the current flow state. Pure function of
/// `(flow_state, alpha)` — all blink phases come from flow's frame counters.
fn render(frame: &mut Frame, flow_state: &FlowState, scene: &BattleScene, alpha: f32) {
    match flow_state.screen {
        Screen::Boot => {
            frame.clear(hud::COL_BG);
            hud::draw_boot(frame, flow_state.frame);
        }
        Screen::Title => {
            frame.clear(hud::COL_BG);
            hud::draw_title(frame, flow_state.frame);
        }
        Screen::MainMenu => {
            frame.clear(hud::COL_BG);
            hud::draw_main_menu(frame, flow_state.menu_cursor, flow_state.frame);
        }
        Screen::Garage => {
            frame.clear(hud::COL_BG);
            hud::draw_garage_stub(frame);
        }
        Screen::Settings => {
            frame.clear(hud::COL_BG);
            hud::draw_settings(frame, &flow_state.settings, flow_state.settings_cursor);
        }
        Screen::TopSelect => {
            frame.clear(hud::COL_BG);
            hud::draw_top_select(frame, flow_state.select_cursor, flow_state.frame);
        }
        Screen::Match(phase) => {
            let visuals = [&PRESETS[flow_state.p1_pick], &PRESETS[flow_state.ai_pick]];
            let accents = [visuals[0].accent, visuals[1].accent];

            // 3D backdrop: the live sim during Fight/Decided, the cosmetic
            // launch preview during Intro/Launch (SPEC §5: interpolate the
            // two most recent sim states by alpha).
            match (&flow_state.world_prev, &flow_state.world) {
                (Some(prev), Some(curr)) => scene.draw(frame, prev, curr, alpha, visuals),
                (None, Some(curr)) => scene.draw(frame, curr, curr, 1.0, visuals),
                _ => frame.clear(hud::COL_BG),
            }

            match phase {
                MatchPhase::Intro => {
                    hud::draw_score_pips(frame, flow_state.score, accents);
                    hud::draw_banner(frame, flow::intro_banner(flow_state.frame), 7, hud::COL_ICE);
                }
                MatchPhase::Launch => {
                    hud::draw_score_pips(frame, flow_state.score, accents);
                    let opp_dir = flow_state
                        .ai_choice
                        .map(|c| c.spin_dir)
                        .unwrap_or(visuals[1].spin_dir);
                    hud::draw_launch_ui(frame, &flow_state.minigame, opp_dir, flow_state.frame);
                }
                MatchPhase::Fight => {
                    if let Some(world) = &flow_state.world {
                        hud::draw_battle_hud(
                            frame,
                            world,
                            visuals,
                            flow_state.score,
                            flow_state.frame,
                        );
                    }
                }
                MatchPhase::Decided => {
                    if let Some(world) = &flow_state.world {
                        hud::draw_battle_hud(
                            frame,
                            world,
                            visuals,
                            flow_state.score,
                            flow_state.frame,
                        );
                    }
                    if let Some(outcome) = flow_state.last_outcome {
                        hud::draw_banner(
                            frame,
                            flow::outcome_banner(outcome, flow_state.last_round_end),
                            6,
                            hud::COL_ICE,
                        );
                    }
                }
                MatchPhase::RoundResult => {
                    let line = flow_state
                        .last_outcome
                        .map(|o| flow::outcome_banner(o, flow_state.last_round_end))
                        .unwrap_or("ROUND OVER");
                    hud::draw_round_result(
                        frame,
                        flow_state.round,
                        flow_state.score,
                        accents,
                        line,
                        flow_state.frame,
                    );
                }
            }
        }
        Screen::MatchOver => {
            frame.clear(hud::COL_BG);
            hud::draw_match_over(
                frame,
                flow_state.score[0] >= MATCH_WIN_POINTS,
                flow_state.score,
                flow_state.frame,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Audio wiring (Task M6-B; SPEC §8, game_design.md §8).
// ---------------------------------------------------------------------------

/// Which song plays for a given screen: every match phase (Intro/Launch/
/// Fight/Decided/RoundResult) uses the battle theme — including the
/// cosmetic-preview Intro/Launch phases, so the riser builds tension into
/// the fight rather than only starting once Fight begins — and every other
/// screen uses the menu theme.
fn song_for_screen(screen: Screen) -> SongId {
    match screen {
        Screen::Match(_) => SongId::Battle,
        _ => SongId::Menu,
    }
}

/// The small `Copy` slice of `FlowState` menu-navigation actually needed to
/// detect cursor/screen deltas for menu SFX — deliberately NOT a clone of
/// the whole `FlowState` (which would drag along the live `World`, RNG
/// state, etc. for no reason every flow frame).
#[derive(Clone, Copy)]
struct MenuNavSnapshot {
    screen: Screen,
    menu_cursor: usize,
    select_cursor: usize,
    settings_cursor: usize,
}

impl MenuNavSnapshot {
    fn of(flow_state: &FlowState) -> Self {
        Self {
            screen: flow_state.screen,
            menu_cursor: flow_state.menu_cursor,
            select_cursor: flow_state.select_cursor,
            settings_cursor: flow_state.settings_cursor,
        }
    }
}

/// Menu navigation SFX, detected from screen/cursor deltas across one flow
/// frame. `flow::FlowState` doesn't expose cursor-edit events directly, so
/// this is deliberately render-side (main.rs), per the milestone brief's
/// explicit "or skip" escape hatch for menu blips — a curated, judgment-call
/// mapping rather than an exhaustive one: cursor moves within a screen read
/// as `MenuMove`; a hand-picked set of screen-transition edges reads as
/// "select" (entering a sub-screen, or confirming a TopSelect pick) or
/// "back" (leaving one, aborting a match, or leaving MatchOver). Transitions
/// not listed here (e.g. RoundResult's "any key" advance, which already has
/// its own tally-ting cue via `Sfx::ScoreTally` elsewhere) are silent by
/// design, not by omission.
fn menu_sfx_for(prev: MenuNavSnapshot, cur: MenuNavSnapshot) -> Option<Sfx> {
    use Screen::*;
    if prev.screen == cur.screen {
        return match cur.screen {
            MainMenu if prev.menu_cursor != cur.menu_cursor => Some(Sfx::MenuMove),
            TopSelect if prev.select_cursor != cur.select_cursor => Some(Sfx::MenuMove),
            Settings if prev.settings_cursor != cur.settings_cursor => Some(Sfx::MenuMove),
            _ => None,
        };
    }
    match (prev.screen, cur.screen) {
        (Title, MainMenu) => Some(Sfx::MenuSelect),
        (MainMenu, TopSelect) | (MainMenu, Garage) | (MainMenu, Settings) => Some(Sfx::MenuSelect),
        (TopSelect, Match(MatchPhase::Intro)) => Some(Sfx::MenuSelect),
        (TopSelect, MainMenu) | (Garage, MainMenu) | (Settings, MainMenu) => Some(Sfx::MenuBack),
        (Match(_), MainMenu) => Some(Sfx::MenuBack),
        (MatchOver, MainMenu) => Some(Sfx::MenuBack),
        _ => None,
    }
}

fn main() {
    let mut platform = Platform::init("FLOPPY SPIN", W as i32, H as i32);
    platform.audio_init();
    let scene = BattleScene::new();
    let mut frame = Frame::new(W, H);

    // Fixed base seed: the flow folds menu timing (total frames until the
    // player confirms a top) into each match seed, so matches still vary
    // run-to-run while everything inside core stays wall-clock-free.
    let mut flow_state = FlowState::new(0xF10B_B75E);

    // Fixed-timestep pump (SPEC §5): the SimClock banks wall time into whole
    // 120 Hz steps; every SIM_STEPS_PER_FLOW_FRAME (2) banked steps run one
    // flow frame (which itself steps the World/minigame twice during
    // Fight/Launch — see flow.rs's tick-model docs). `alpha` interpolates
    // the last two sim states for rendering.
    let mut clock = SimClock::new();
    let mut pending_steps: u32 = 0;
    let mut last_t = platform.now_s();

    // Audio (Task M6-B / SPEC §8): one `Mixer` for the process lifetime; the
    // `Tracker` is replaced wholesale on a song switch (no fade — see the
    // milestone report). `hum_active` remembers whether the spin-hum voice
    // was sounding so leaving Fight sends exactly one release trigger
    // (`spin_hum`'s own zero-rpm path handles the actual fade, see
    // `floppy_audio::sfx`).
    let mut mixer = Mixer::new();
    let mut current_song = song_for_screen(flow_state.screen);
    let mut tracker = Tracker::new(current_song);
    let mut hum_active = false;

    loop {
        let frame_start = platform.now_s();

        if !platform.poll() {
            break;
        }
        let input = read_input(&platform);
        let esc = platform.key(VK_ESCAPE);

        let dt = frame_start - last_t;
        last_t = frame_start;
        pending_steps += clock.advance(dt, 8);
        while pending_steps >= flow::SIM_STEPS_PER_FLOW_FRAME {
            let prev_nav = MenuNavSnapshot::of(&flow_state);

            flow_state.advance(input, esc);
            pending_steps -= flow::SIM_STEPS_PER_FLOW_FRAME;

            // ---- Music: replace the Tracker wholesale on a song switch.
            let desired_song = song_for_screen(flow_state.screen);
            if desired_song != current_song {
                tracker = Tracker::new(desired_song);
                current_song = desired_song;
            }

            // ---- Intensity layer (game_design.md §8): on iff either top
            // has a special Armed, only during Fight.
            let armed_in_fight = matches!(flow_state.screen, Screen::Match(MatchPhase::Fight))
                && flow_state
                    .world
                    .as_ref()
                    .map(|w| w.tops[0].combat.special_armed || w.tops[1].combat.special_armed)
                    .unwrap_or(false);
            tracker.set_intensity(armed_in_fight);

            // ---- SFX from BattleEvents, Fight only. `fight_active` checks
            // BOTH the screen before and after this `advance()` call so the
            // exact frame Fight ends (screen already reads Decided) still
            // drains the finishing hit's events. See the milestone report
            // for `World::events`' clear-per-step lifecycle and the
            // resulting caveat: `flow::advance` runs up to 2 sim steps per
            // flow frame internally and only the LAST step's events survive
            // (`World::step` clears `events` at the top of every call that
            // doesn't early-return with an outcome already set), so a
            // BattleEvent produced only on the first of those 2 sub-steps
            // during an ordinary (non-round-ending) frame is not visible
            // here — a real, documented gap `main.rs` cannot close without
            // touching `flow.rs` (out of scope for this milestone).
            let fight_active = matches!(prev_nav.screen, Screen::Match(MatchPhase::Fight))
                || matches!(flow_state.screen, Screen::Match(MatchPhase::Fight));
            if fight_active {
                if let Some(world) = &flow_state.world {
                    for ev in &world.events {
                        on_event(&mut mixer, ev);
                    }
                }
            }

            // ---- Spin hum: steady per-flow-frame retrigger/retune while
            // fighting (P1's top only — SPEC §8 reserves a single dedicated
            // hum voice, not one per top); one explicit release the frame
            // Fight ends.
            if matches!(flow_state.screen, Screen::Match(MatchPhase::Fight)) {
                if let Some(world) = &flow_state.world {
                    let rpm_frac = world.tops[0].spin / TUNE.spin_max;
                    play(&mut mixer, Sfx::SpinHum { rpm_frac });
                    hum_active = true;
                }
            } else if hum_active {
                play(&mut mixer, Sfx::SpinHum { rpm_frac: 0.0 });
                hum_active = false;
            }

            // ---- Menu navigation blips (cursor/screen deltas).
            if let Some(sfx) = menu_sfx_for(prev_nav, MenuNavSnapshot::of(&flow_state)) {
                play(&mut mixer, sfx);
            }
        }
        // Esc semantics (quit on Title/MainMenu, back/abort elsewhere) are
        // decided entirely inside flow::advance.
        if flow_state.quit_requested {
            break;
        }

        // ---- Volume settings -> group gains (SPEC §7: music/SFX volume
        // 0..=10). Pure f32 multiply before soft-clip inside the mixer, so
        // this stays deterministic (SPEC §5).
        mixer.set_group_gains(
            flow_state.settings.sfx_vol as f32 / 10.0,
            flow_state.settings.music_vol as f32 / 10.0,
        );

        // ---- waveOut ring pump (Task M6-B): fill every currently-free
        // ring buffer with exactly `AUDIO_BUFFER_FRAMES` fresh mono samples.
        // Advancing the tracker in these fixed-size chunks (rather than once
        // for the whole render frame) keeps its row scheduling sample-
        // accurate regardless of render-frame jitter or how many ring slots
        // happened to free up; `Platform::audio_submit` duplicates mono to
        // interleaved stereo (SPEC §8) and is a no-op with no audio device.
        let free_buffers = platform.audio_free_buffers();
        let mut mono = [0i16; AUDIO_BUFFER_FRAMES];
        for _ in 0..free_buffers {
            tracker.advance(&mut mixer, AUDIO_BUFFER_FRAMES as u32);
            mixer.render(&mut mono);
            platform.audio_submit(&mono);
        }

        // Render interpolation factor: fraction of the way from the previous
        // sim state to the current one. A leftover banked step means we're
        // already a full step past `world` — clamp to 1 (never extrapolate).
        let alpha = (pending_steps as f32 + clock.alpha()).min(1.0);
        render(&mut frame, &flow_state, &scene, alpha);
        platform.blit(&frame.px, W as i32, H as i32);

        // Pace to 60 fps: sleep off the coarse remainder, leaving a small
        // margin, then spin-wait onto the exact frame boundary. Never
        // busy-spins the whole frame.
        let target = frame_start + FRAME_DT;
        let remaining = target - platform.now_s();
        if remaining > SPIN_MARGIN_S {
            Platform::sleep_ms(((remaining - SPIN_MARGIN_S) * 1000.0) as u32);
        }
        while platform.now_s() < target {}
    }
}
