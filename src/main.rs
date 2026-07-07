// Game binary. Console subsystem in debug builds only (println! debugging).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod platform;

use floppy_core::clock::SimClock;
use floppy_core::flow::{self, FlowState, MatchPhase, Screen, MATCH_WIN_POINTS};
use floppy_core::input::InputState;
use floppy_core::roster::PRESETS;
use floppy_render::battle::BattleScene;
use floppy_render::frame::Frame;
use floppy_render::hud;
use platform::win32::{Platform, VK_ESCAPE};

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

fn main() {
    let mut platform = Platform::init("FLOPPY SPIN", W as i32, H as i32);
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
            flow_state.advance(input, esc);
            pending_steps -= flow::SIM_STEPS_PER_FLOW_FRAME;
        }
        // Esc semantics (quit on Title/MainMenu, back/abort elsewhere) are
        // decided entirely inside flow::advance.
        if flow_state.quit_requested {
            break;
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
