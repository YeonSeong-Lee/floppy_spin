// Game binary. Console subsystem in debug builds only (println! debugging).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod platform;

use floppy_audio::{on_event, play, Mixer, Sfx, SongId, Tracker};
use floppy_core::arena;
use floppy_core::clock::SimClock;
use floppy_core::combat;
use floppy_core::fixmath;
use floppy_core::flow::{self, FlowState, MatchPhase, Screen, MATCH_WIN_POINTS};
use floppy_core::input::InputState;
use floppy_core::physics::{self, BattleEvent, TUNE};
use floppy_core::rng::Rng;
use floppy_core::roster::Preset;
use floppy_core::save;
use floppy_core::vec::Vec2;
use floppy_render::battle::{accent_to_vec3, BattleScene};
use floppy_render::frame::Frame;
use floppy_render::particles::{self, ParticlePool};
use floppy_render::post::PostState;
use floppy_render::{hud, vfx};
use platform::win32::{self, Platform, WindowScaleMode, AUDIO_BUFFER_FRAMES, VK_ESCAPE};

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

// ---------------------------------------------------------------------------
// M7: render-side VFX state (Tasks 2/3/4/5). Everything here is plain data
// driven only by `BattleEvent`s / sim state / flow-screen transitions / a
// dedicated render-side `Rng` (SPEC §5 "HARD RULES" — never `World`'s own
// `rng`, never wall-clock). One `Vfx` instance lives for the process
// lifetime (like `Mixer`); its `rng` is reseeded per ROUND (module docs on
// `advance_vfx_for_flow_frame`), everything else just decays/eases forward.
// ---------------------------------------------------------------------------

/// Salt XORed into the round seed for the dedicated render-side VFX `Rng`
/// stream (distinct from `flow.rs`'s own `AI_ROLL_SEED_SALT`/
/// `AI_FIGHT_SEED_SALT` and from `World`'s own `rng` — SPEC §5's "HARD
/// RULES" bans reusing the sim's RNG for anything render-visible).
const VFX_RNG_SALT: u64 = 0x00F7_0000_0000_0001;

struct Vfx {
    post: PostState,
    particles: ParticlePool,
    shake: vfx::ShakeState,
    flash: vfx::FlashState,
    /// Arena ring-pulse intensity (Task 4), decayed/kicked in the audio
    /// submission loop below (that's where `Tracker::row_index()` actually
    /// advances).
    ring_pulse: f32,
    rng: Rng,
    menu_spring: vfx::Spring,
    select_spring: vfx::Spring,
    settings_spring: vfx::Spring,
    /// Garage slot cursor spring (M8), same "~120 ms spring settle" treatment
    /// as the other menu cursors.
    garage_spring: vfx::Spring,
    intro_scale: vfx::OvershootSpring,
    decided_scale: vfx::OvershootSpring,
    matchover_scale: vfx::OvershootSpring,
    prev_round_seed: u64,
    prev_intro_banner: &'static str,
    prev_tally: u8,
    prev_tracker_row: u32,
    prev_screen: Screen,
    wipe_frame: u32,
}

impl Vfx {
    fn new() -> Self {
        Self {
            post: PostState::new(W, H),
            particles: ParticlePool::new(),
            shake: vfx::ShakeState::default(),
            flash: vfx::FlashState::new(),
            ring_pulse: 0.0,
            rng: Rng::new(1),
            menu_spring: vfx::Spring::new(0.0),
            select_spring: vfx::Spring::new(0.0),
            settings_spring: vfx::Spring::new(0.0),
            garage_spring: vfx::Spring::new(0.0),
            intro_scale: vfx::OvershootSpring::default(),
            decided_scale: vfx::OvershootSpring::default(),
            matchover_scale: vfx::OvershootSpring::default(),
            prev_round_seed: 0,
            prev_intro_banner: "",
            prev_tally: 0,
            prev_tracker_row: 0,
            prev_screen: Screen::Boot,
            wipe_frame: vfx::WIPE_FRAMES, // settled: no wipe plays on boot
        }
    }
}

/// ms -> 60Hz render frames (juice-table durations are given in ms).
const fn ms_to_frames(ms: f32) -> f32 {
    ms * 0.06
}

/// game_design.md §5 juice table -> `Vfx` state (shake/flash/particles),
/// one `BattleEvent` at a time, IN ORDER (SPEC §5 "HARD RULES"). Events with
/// no juice-table row (`SpecialHit`, `AnchorBreak`, `AirborneLaunch`,
/// `Landed`) are deliberately silent here — see the M7 report for the full
/// per-row wiring list, including the two rows with no `BattleEvent` at all
/// (Wall bounce, Round/Match win) that are driven from sim-state deltas /
/// screen transitions elsewhere in this file instead.
fn handle_battle_event(
    v: &mut Vfx,
    world: &physics::World,
    ev: &BattleEvent,
    presets: [Preset; 2],
) {
    match *ev {
        BattleEvent::Hit { heavy, pos, .. } => {
            if heavy {
                v.shake.add(7.0, 0.86);
                v.flash.add(particles::WHITE * 0.35, ms_to_frames(60.0));
                particles::spawn_heavy_hit(&mut v.particles, &mut v.rng, pos);
            } else {
                v.shake.add(2.0, 0.80);
                particles::spawn_light_hit(&mut v.particles, &mut v.rng, pos);
            }
        }
        BattleEvent::Dash { who } => {
            v.shake.add(1.0, 0.80);
            let top = &world.tops[who as usize];
            let back = Vec2::new(-top.vel.x, -top.vel.z).normalize_or_zero();
            let accent = accent_to_vec3(presets[who as usize].accent);
            particles::spawn_dash(&mut v.particles, &mut v.rng, top.pos, back, accent);
        }
        BattleEvent::AerialSlam { who } => {
            // game_design.md §5's "Airborne clash" row: the closest sim
            // event to that moment is a landed aerial slam (Hop's airborne
            // attack) — see the M7 report for why this mapping was chosen
            // over a synthetic "both tops airborne" detector.
            let top = &world.tops[who as usize];
            v.shake.add(9.0, 0.88);
            v.flash.add(particles::CYAN * 0.30, ms_to_frames(80.0));
            particles::spawn_airborne_clash(&mut v.particles, &mut v.rng, top.pos);
        }
        BattleEvent::SpecialFire { who } => {
            let top = &world.tops[who as usize];
            let accent = accent_to_vec3(presets[who as usize].accent);
            v.shake.add(6.0, 0.84);
            v.flash.add(accent * 0.40, ms_to_frames(100.0));
            particles::spawn_special_fire(&mut v.particles, &mut v.rng, top.pos, accent);
        }
        BattleEvent::CrashOut { winner } => {
            let loser = 1 - winner;
            let pos = world.tops[loser as usize].pos;
            let accent = accent_to_vec3(presets[winner as usize].accent);
            v.shake.add(12.0, 0.90);
            v.flash.add(particles::WHITE * 0.60, ms_to_frames(120.0));
            particles::spawn_crash_out(&mut v.particles, &mut v.rng, pos, accent);
        }
        BattleEvent::RingOut { who } => {
            let pos = world.tops[who as usize].pos;
            v.shake.add(8.0, 0.87);
            v.flash
                .add(particles::RED_ORANGE * 0.30, ms_to_frames(90.0));
            particles::spawn_ring_out(&mut v.particles, &mut v.rng, pos);
        }
        BattleEvent::Topple { who } => {
            let pos = world.tops[who as usize].pos;
            v.shake.add(5.0, 0.80);
            v.flash.add(particles::AMBER * 0.25, ms_to_frames(100.0));
            particles::spawn_topple(&mut v.particles, &mut v.rng, pos);
        }
        BattleEvent::Parry { who } | BattleEvent::GuardBlock { who } => {
            let top = &world.tops[who as usize];
            v.shake.add(3.0, 0.85);
            v.flash.add(particles::WHITE * 0.20, ms_to_frames(50.0));
            let facing = combat::facing_xz(top);
            particles::spawn_guard_parry(&mut v.particles, &mut v.rng, top.pos, facing);
        }
        // No juice-table row: sim-visible bookkeeping events only.
        BattleEvent::AirborneLaunch { .. }
        | BattleEvent::Landed { .. }
        | BattleEvent::SpecialHit { .. }
        | BattleEvent::AnchorBreak { .. } => {}
    }
}

/// Wall bounce (game_design.md §5: "10 dust tangential") has no dedicated
/// `BattleEvent` — terrain contact (including the wall) is continuous
/// normal-force projection in `physics.rs`, not a discrete collision moment
/// (confirmed: `TuneParams::hitstop_wall_bounce` is defined but never read
/// anywhere in `physics.rs`). Approximated here from SIM STATE alone (SPEC
/// §5 explicitly allows this): a top on the wall band (`r > WALL_START`)
/// whose outward-radial velocity flips from clearly-positive (heading into
/// the wall) to clearly-negative (pushed back) between two consecutive
/// world snapshots reads as a bounce. Only sees the LAST of this flow
/// frame's `SIM_STEPS_PER_FLOW_FRAME` sub-steps (the same limitation
/// `flow.rs` documents for anything reading `world_prev`/`world` directly
/// instead of the accumulated `frame_events`) — an accepted approximation
/// for a cosmetic dust puff.
fn detect_wall_bounces(v: &mut Vfx, prev: &physics::World, curr: &physics::World) {
    for i in 0..2 {
        let p = &prev.tops[i];
        let c = &curr.tops[i];
        if !c.grounded {
            continue;
        }
        let r = fixmath::sqrt(c.pos.x * c.pos.x + c.pos.z * c.pos.z);
        if r <= arena::WALL_START {
            continue;
        }
        let radial_dir = Vec2::new(c.pos.x, c.pos.z).normalize_or_zero();
        let prev_out = p.vel.x * radial_dir.x + p.vel.z * radial_dir.y;
        let curr_out = c.vel.x * radial_dir.x + c.vel.z * radial_dir.y;
        if prev_out > 0.5 && curr_out < -0.2 {
            v.shake.add(4.0, 0.82);
            // "ring tint .15" (game_design.md §5): the arena ring bands'
            // own cyan-ish emissive color, at their given low alpha.
            v.flash.add(particles::CYAN * 0.15, ms_to_frames(80.0));
            let tangent = Vec2::new(-radial_dir.y, radial_dir.x);
            particles::spawn_wall_bounce(&mut v.particles, &mut v.rng, c.pos, tangent);
        }
    }
}

/// One flow frame's worth of `Vfx` bookkeeping (Tasks 2/3/4/5), called once
/// per `flow_state.advance()` — i.e. potentially more than once per
/// rendered frame if sim time is catching up (mirrors the existing SFX/
/// music wiring's own cadence in `main`). Returns whether the screen
/// discriminant changed this call (so `main` can react once, e.g. firing
/// round/match-win VFX below).
fn advance_vfx_for_flow_frame(v: &mut Vfx, flow_state: &FlowState) -> bool {
    let screen_changed = v.prev_screen != flow_state.screen;
    v.prev_screen = flow_state.screen;
    if screen_changed {
        v.wipe_frame = 0;
    } else if v.wipe_frame < vfx::WIPE_FRAMES {
        v.wipe_frame += 1;
    }

    // Reseed the dedicated render-side RNG once per ROUND (SPEC §5 "HARD
    // RULES"), never per-frame — a fresh, cheap `ParticlePool` too, so a
    // brand new round never shows a stray straggler from two rounds ago
    // (they'd have expired within their own lifetime anyway; this is just
    // tidy determinism, not a correctness requirement).
    let round_seed = flow_state.round_seed();
    if round_seed != v.prev_round_seed {
        v.rng = Rng::new(round_seed ^ VFX_RNG_SALT);
        v.prev_round_seed = round_seed;
        v.particles = ParticlePool::new();
    }

    // M8: `preset_view` resolves MY BEY (p1_pick == MY_BEY_INDEX) from the
    // live garage build instead of indexing `PRESETS` directly, so this
    // never panics once TopSelect's 8th entry is pickable.
    let presets = [
        flow_state.preset_view(flow_state.p1_pick),
        flow_state.preset_view(flow_state.ai_pick),
    ];

    if let Some(world) = &flow_state.world {
        for ev in &flow_state.frame_events {
            handle_battle_event(v, world, ev, presets);
        }
        if let Some(prev) = &flow_state.world_prev {
            detect_wall_bounces(v, prev, world);
        }
    }

    // Round win / Match win (game_design.md §5): no `BattleEvent` exists for
    // either — they're screen-transition moments (Decided entry / MatchOver
    // entry), driven here from the flow's own `last_winner`/`score`.
    if screen_changed {
        if matches!(flow_state.screen, Screen::Match(MatchPhase::Decided)) {
            if let (Some(winner), Some(world)) = (flow_state.last_winner, &flow_state.world) {
                v.shake.add(4.0, 0.78);
                v.flash.add(particles::GOLD * 0.30, ms_to_frames(150.0));
                particles::spawn_round_win(
                    &mut v.particles,
                    &mut v.rng,
                    world.tops[winner as usize].pos,
                );
            }
        }
        if matches!(flow_state.screen, Screen::MatchOver) {
            // "6px x2 pulses" (game_design.md §5): two stacked shake
            // additions (the 14px clamp still applies as one combined cap).
            v.shake.add(6.0, 0.80);
            v.shake.add(6.0, 0.80);
            v.flash.add(particles::GOLD * 0.45, ms_to_frames(250.0));
            if let Some(world) = &flow_state.world {
                let winner = if flow_state.score[0] >= MATCH_WIN_POINTS {
                    0
                } else {
                    1
                };
                particles::spawn_match_win(
                    &mut v.particles,
                    &mut v.rng,
                    world.tops[winner as usize].pos,
                );
            }
        }
        if matches!(flow_state.screen, Screen::Match(MatchPhase::Decided)) {
            v.decided_scale.snap(1.5);
        }
        if matches!(flow_state.screen, Screen::MatchOver) {
            v.matchover_scale.snap(1.5);
        }
    }
    if matches!(flow_state.screen, Screen::Match(MatchPhase::Decided)) {
        v.decided_scale.step(1.0);
    }
    if matches!(flow_state.screen, Screen::MatchOver) {
        v.matchover_scale.step(1.0);
    }

    // Intro countdown numeral pop (game_design.md §7): re-snap the overshoot
    // spring every time the displayed numeral/text actually changes.
    if matches!(flow_state.screen, Screen::Match(MatchPhase::Intro)) {
        let banner_text = flow::intro_banner(flow_state.frame);
        if banner_text != v.prev_intro_banner {
            v.intro_scale.snap(1.3);
            v.prev_intro_banner = banner_text;
        }
        v.intro_scale.step(1.0);
    }

    // RoundResult pip tally count (game_design.md §7 "main.rs edge"): just
    // the pure state update here; `main`'s loop captures `prev_tally`
    // before this call and compares after, so it can fire exactly one
    // `Sfx::ScoreTally` per newly-landed pip without this function needing
    // a `&mut Mixer` threaded through it.
    if matches!(flow_state.screen, Screen::Match(MatchPhase::RoundResult))
        && flow_state.last_winner.is_some()
    {
        v.prev_tally = hud::tally_pip_count(flow_state.frame, flow_state.last_points);
    } else {
        v.prev_tally = 0;
    }

    // Menu cursor springs (game_design.md §7: "~120 ms spring settle"),
    // eased every flow frame regardless of screen (cheap, and avoids a
    // stale spring position if the player left and returns to a screen).
    v.menu_spring.ease_toward(flow_state.menu_cursor as f32);
    v.select_spring.ease_toward(flow_state.select_cursor as f32);
    v.settings_spring
        .ease_toward(flow_state.settings_cursor as f32);
    v.garage_spring.ease_toward(flow_state.garage_slot as f32);

    v.particles.update();
    v.shake.step();
    v.flash.step();

    screen_changed
}

/// Draw the whole game for the current flow state. Pure function of
/// `(flow_state, alpha, vfxs)` — all blink/animation phases come from flow's
/// or `vfxs`'s own frame counters, never wall-clock (SPEC §5).
#[allow(clippy::too_many_arguments)]
fn render(
    frame: &mut Frame,
    flow_state: &FlowState,
    scene: &BattleScene,
    alpha: f32,
    vfxs: &mut Vfx,
) {
    vfxs.post.begin_frame();

    let shake_mult = vfx::shake_level_mult(flow_state.settings.shake);
    let shake_offset = vfxs.shake.offset(&mut vfxs.rng, shake_mult);
    let flash_color = vfxs.flash.current();
    let colorblind = flow_state.settings.colorblind;

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
            hud::draw_main_menu(
                frame,
                flow_state.menu_cursor,
                vfxs.menu_spring.value,
                flow_state.frame,
            );
        }
        Screen::Garage => {
            frame.clear(hud::COL_BG);
            hud::draw_garage(
                frame,
                flow_state.parts,
                flow_state.garage_slot,
                vfxs.garage_spring.value,
                colorblind,
                flow_state.frame,
            );
        }
        Screen::Settings => {
            frame.clear(hud::COL_BG);
            hud::draw_settings(
                frame,
                &flow_state.settings,
                flow_state.settings_cursor,
                vfxs.settings_spring.value,
            );
        }
        Screen::TopSelect => {
            frame.clear(hud::COL_BG);
            hud::draw_top_select(
                frame,
                flow_state.select_cursor,
                vfxs.select_spring.value,
                flow_state.frame,
                flow_state.parts,
            );
        }
        Screen::Match(phase) => {
            // M8: owned locals (not `&PRESETS[..]`) since `p1_pick` may be
            // `MY_BEY_INDEX` — `preset_view` resolves that from the live
            // garage build rather than indexing the roster array.
            let p1_visual = flow_state.preset_view(flow_state.p1_pick);
            let ai_visual = flow_state.preset_view(flow_state.ai_pick);
            let visuals = [&p1_visual, &ai_visual];
            let accents = [visuals[0].accent, visuals[1].accent];

            // 3D backdrop: the live sim during Fight/Decided, the cosmetic
            // launch preview during Intro/Launch (SPEC §5: interpolate the
            // two most recent sim states by alpha) — now through the M7
            // bloom/particle/shake/ring-pulse pipeline.
            match (&flow_state.world_prev, &flow_state.world) {
                (Some(prev), Some(curr)) => scene.draw_ex(
                    frame,
                    &mut vfxs.post,
                    prev,
                    curr,
                    alpha,
                    visuals,
                    vfxs.ring_pulse,
                    shake_offset,
                    &vfxs.particles,
                ),
                (None, Some(curr)) => scene.draw_ex(
                    frame,
                    &mut vfxs.post,
                    curr,
                    curr,
                    1.0,
                    visuals,
                    vfxs.ring_pulse,
                    shake_offset,
                    &vfxs.particles,
                ),
                _ => {
                    frame.clear(hud::COL_BG);
                    vfxs.post.begin_frame();
                }
            }

            match phase {
                MatchPhase::Intro => {
                    hud::draw_score_pips(frame, flow_state.score, accents, colorblind);
                    hud::draw_banner_scaled(
                        frame,
                        flow::intro_banner(flow_state.frame),
                        7.0 * vfxs.intro_scale.value,
                        hud::COL_ICE,
                    );
                }
                MatchPhase::Launch => {
                    hud::draw_score_pips(frame, flow_state.score, accents, colorblind);
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
                            colorblind,
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
                            colorblind,
                        );
                    }
                    if let Some(outcome) = flow_state.last_outcome {
                        hud::draw_banner_scaled(
                            frame,
                            flow::outcome_banner(outcome, flow_state.last_round_end),
                            6.0 * vfxs.decided_scale.value,
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
                        flow_state.last_winner,
                        flow_state.last_points,
                        colorblind,
                    );
                }
            }
        }
        Screen::MatchOver => {
            let p1_visual = flow_state.preset_view(flow_state.p1_pick);
            let ai_visual = flow_state.preset_view(flow_state.ai_pick);
            let visuals = [&p1_visual, &ai_visual];
            match (&flow_state.world_prev, &flow_state.world) {
                (Some(prev), Some(curr)) => scene.draw_ex(
                    frame,
                    &mut vfxs.post,
                    prev,
                    curr,
                    alpha,
                    visuals,
                    vfxs.ring_pulse,
                    shake_offset,
                    &vfxs.particles,
                ),
                (None, Some(curr)) => scene.draw_ex(
                    frame,
                    &mut vfxs.post,
                    curr,
                    curr,
                    1.0,
                    visuals,
                    vfxs.ring_pulse,
                    shake_offset,
                    &vfxs.particles,
                ),
                _ => {
                    frame.clear(hud::COL_BG);
                    vfxs.post.begin_frame();
                }
            }
            hud::draw_match_over(
                frame,
                flow_state.score[0] >= MATCH_WIN_POINTS,
                flow_state.score,
                flow_state.frame,
                6.0 * vfxs.matchover_scale.value,
            );
        }
    }

    vfxs.post.composite(frame, flash_color);
    if vfxs.wipe_frame < vfx::WIPE_FRAMES {
        vfx::draw_wipe(frame, vfxs.wipe_frame);
    }
}

// ---------------------------------------------------------------------------
// Audio wiring (Task M6-B; SPEC §8, game_design.md §8).
// ---------------------------------------------------------------------------

/// `flow::WindowScale` (SPEC §7 setting, persisted) -> the platform layer's
/// own `WindowScaleMode` (kept a distinct type so `win32` doesn't depend on
/// `floppy_core::flow` — module docs on `WindowScaleMode`).
fn window_scale_mode_for(scale: flow::WindowScale) -> WindowScaleMode {
    match scale {
        flow::WindowScale::X1 => WindowScaleMode::X1,
        flow::WindowScale::X1_5 => WindowScaleMode::X1_5,
        flow::WindowScale::X2 => WindowScaleMode::X2,
        flow::WindowScale::Fullscreen => WindowScaleMode::Fullscreen,
    }
}

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
    let mut vfxs = Vfx::new();

    // Fixed base seed: the flow folds menu timing (total frames until the
    // player confirms a top) into each match seed, so matches still vary
    // run-to-run while everything inside core stays wall-clock-free.
    let mut flow_state = FlowState::new(0xF10B_B75E);

    // ---- M8 Task 6: load-on-boot (SPEC §9). `platform::save_load` returns
    // an empty Vec on ANY failure (no %APPDATA%, no file, ...) and
    // `save::decode` turns anything that isn't a fully-valid blob
    // (including empty) into `SaveState::default()` — so this line alone
    // covers "no save file yet" and "corrupt save file" identically, with
    // no branching needed here.
    flow_state.apply_save(save::decode(&win32::save_load()));

    // ---- Window-scale (SPEC §7): apply the now-loaded setting once at
    // boot, so a persisted non-default scale takes effect immediately
    // instead of only after the player revisits Settings. `set_window_scale`
    // itself no-ops if the mode already matches `Platform::init`'s starting
    // X1, so this is cheap on the (default) common case.
    platform.set_window_scale(window_scale_mode_for(flow_state.settings.window_scale));

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
            let prev_tally = vfxs.prev_tally;

            flow_state.advance(input, esc);
            pending_steps -= flow::SIM_STEPS_PER_FLOW_FRAME;

            // ---- M8 Task 6: persist on leaving Garage or Settings (SPEC
            // §9). Screen-transition edge, not every frame: `prev_nav.screen`
            // (captured above, before this `advance`) was Garage/Settings and
            // the new screen isn't, i.e. the player just backed out — that's
            // the natural "commit this build/these settings" moment for
            // both screens (Garage: Z/Esc backs to MainMenu after part
            // swaps; Settings: same, after adjustments). Writing on every
            // frame would mean a write per keystroke while adjusting a
            // slider, which is unnecessary I/O for a file this small.
            if prev_nav.screen != flow_state.screen
                && matches!(prev_nav.screen, Screen::Garage | Screen::Settings)
            {
                let (parts, settings) = flow_state.save_snapshot();
                win32::save_store(&save::encode(parts, &settings));
            }

            // ---- Window-scale (SPEC §7): re-apply on the same "leaving
            // Settings" edge as the persist write just above — the only
            // screen where `window_scale` can change. `set_window_scale`
            // itself no-ops when the mode is unchanged, so this never
            // thrashes the window if the player left Settings without
            // touching that row.
            if prev_nav.screen != flow_state.screen && matches!(prev_nav.screen, Screen::Settings) {
                platform.set_window_scale(window_scale_mode_for(flow_state.settings.window_scale));
            }

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

            // ---- SFX from BattleEvents. `flow.frame_events` accumulates
            // across ALL of the frame's sim sub-steps (see its doc comment —
            // reading `world.events` here instead would drop every event the
            // first of the 2 sub-steps produced), and it's cleared at the
            // top of every `advance()`, so draining it unconditionally is
            // correct on every screen including the exact Fight→Decided
            // transition frame that carries the finishing hit.
            for ev in &flow_state.frame_events {
                on_event(&mut mixer, ev);
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

            // ---- M7 VFX: shake/flash/particles/springs/wipe/pip-tally.
            advance_vfx_for_flow_frame(&mut vfxs, &flow_state);
            if vfxs.prev_tally > prev_tally {
                play(&mut mixer, Sfx::ScoreTally);
            }
        }
        // Esc semantics (quit on Title/MainMenu, back/abort elsewhere) are
        // decided entirely inside flow::advance.
        if flow_state.quit_requested {
            // ---- M8 Task 6: persist on quit (SPEC §9), so settings/garage
            // changes made since the last Garage/Settings-exit (e.g. the
            // player tweaked Settings then immediately quit from MainMenu
            // without a further screen transition) are never silently lost.
            let (parts, settings) = flow_state.save_snapshot();
            win32::save_store(&save::encode(parts, &settings));
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
        //
        // ---- Task 4 ring pulse: driven off `Tracker::row_index()` (never
        // audio output — SPEC §5), an EDGE onto a kick row, checked once per
        // row actually fired (a row can span more than one audio buffer at
        // low BPM, so re-checking per buffer would re-trigger the same kick
        // repeatedly instead of decaying between rows).
        let free_buffers = platform.audio_free_buffers();
        let mut mono = [0i16; AUDIO_BUFFER_FRAMES];
        for _ in 0..free_buffers {
            tracker.advance(&mut mixer, AUDIO_BUFFER_FRAMES as u32);
            mixer.render(&mut mono);
            platform.audio_submit(&mono);

            let row = tracker.row_index();
            let kicked = row != vfxs.prev_tracker_row && tracker.kick_on_current_row();
            vfxs.prev_tracker_row = row;
            vfxs.ring_pulse = vfx::ring_pulse_step(vfxs.ring_pulse, kicked);
        }

        // Render interpolation factor: fraction of the way from the previous
        // sim state to the current one. A leftover banked step means we're
        // already a full step past `world` — clamp to 1 (never extrapolate).
        let alpha = (pending_steps as f32 + clock.alpha()).min(1.0);
        render(&mut frame, &flow_state, &scene, alpha, &mut vfxs);
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
