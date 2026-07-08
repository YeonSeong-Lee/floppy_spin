//! M4-B: screens & flow — explicit enum-edge screen state machine, menu
//! cursors, match/round structure (SPEC §7, §6.5). Headlessly testable;
//! drawing lives in `floppy_render::hud`.
//!
//! ## Tick model (documented decision)
//!
//! One [`FlowState::advance`] call == one **flow frame** at a nominal 60 Hz.
//! Screens that contain 120 Hz simulation ([`MatchPhase::Fight`] steps the
//! `World`, [`MatchPhase::Launch`] steps the minigame sweep) run exactly
//! [`SIM_STEPS_PER_FLOW_FRAME`] (= 2) sim ticks per flow frame, which is how
//! `main.rs`'s `SimClock` (120 Hz accumulator) maps onto flow frames: every
//! 2 banked sim steps trigger one `advance`. All duration counters below
//! (`BOOT_FRAMES`, the Intro countdown, `DECIDED_HOLD_FRAMES`) are in flow
//! frames — the flow's OWN counter (`FlowState::frame`), never wall-clock
//! and never `World::step` deltas (which freeze during hit-stop; see the
//! `World::step` doc).
//!
//! ## Intro countdown breakdown (task spec: "120 steps total, your choice")
//!
//! READY? 24 frames (0.4 s) → "3"/"2"/"1" 30 frames each (0.5 s each) →
//! GO! 6 frames (0.1 s) = 120 flow frames = 2.0 s. game_design.md §7's
//! "1.0 s per numeral" choreography is compressed to fit the task-mandated
//! 120-step total; input is ignored throughout (except Esc = abort).
//!
//! ## Arrow-key intent convention (contract with `main.rs`)
//!
//! The M3 battle camera's basis mirrors world `+x` to screen-left, so
//! `main.rs` maps keys camera-relatively: **Up arrow → `dir_y = -1`, Down →
//! `+1`, Right → `dir_x = -1`, Left → `+1`** (constants [`DIR_UP`] /
//! [`DIR_DOWN`] / [`DIR_RIGHT`] / [`DIR_LEFT`]). Menu cursor and settings
//! logic below (and the minigame's aim stage) are written against these
//! constants, so "press Up" always means "cursor up" for the player.
//!
//! ## Esc semantics per screen (documented decision)
//!
//! Title → quit; MainMenu → **direct quit, no confirm dialog** (task spec
//! allows either; confirm UI is deferred); TopSelect/Garage/Settings → back
//! to MainMenu; any Match phase → abort the match back to MainMenu;
//! MatchOver → back to MainMenu. `Esc` arrives as a separate `bool` because
//! `InputState` (SPEC §6.4) has no escape field.

use crate::ai;
use crate::combat::SpecialId;
use crate::garage::{self, DEFAULT_PARTS};
use crate::input::InputState;
use crate::minigame::{ai_roll, Difficulty, LaunchChoice, MinigameState};
use crate::physics::{BattleEvent, LaunchParams, Outcome, World};
use crate::rng::{mix_seed, Rng};
use crate::roster::{Preset, PRESETS};
use crate::save::SaveState;
use crate::vec::Vec2;
use std::f32::consts::{PI, TAU};

/// 120 Hz sim ticks executed per flow frame (module docs above).
pub const SIM_STEPS_PER_FLOW_FRAME: u32 = 2;
/// Boot screen auto-advances after this many flow frames.
pub const BOOT_FRAMES: u32 = 30;
/// Intro countdown segment lengths, flow frames (module docs above).
pub const INTRO_READY_FRAMES: u32 = 24;
pub const INTRO_NUMERAL_FRAMES: u32 = 30;
pub const INTRO_GO_FRAMES: u32 = 6;
pub const INTRO_TOTAL_FRAMES: u32 = INTRO_READY_FRAMES + 3 * INTRO_NUMERAL_FRAMES + INTRO_GO_FRAMES; // 120
/// Decided-phase banner hold before RoundResult, flow frames (~1.5 s).
pub const DECIDED_HOLD_FRAMES: u32 = 90;
/// First to this many points wins the match (SPEC §6.5).
pub const MATCH_WIN_POINTS: u8 = 4;

/// Arrow-key intent constants (module docs above).
pub const DIR_UP: i8 = -1;
pub const DIR_DOWN: i8 = 1;
pub const DIR_RIGHT: i8 = -1;
pub const DIR_LEFT: i8 = 1;

/// MainMenu rows, in cursor order (SPEC §7 diagram).
pub const MAIN_MENU_ITEMS: [&str; 4] = ["QUICK BATTLE", "GARAGE", "SETTINGS", "QUIT"];
const MENU_QUICK_BATTLE: usize = 0;
const MENU_GARAGE: usize = 1;
const MENU_SETTINGS: usize = 2;
const MENU_QUIT: usize = 3;

/// Settings rows, in cursor order (SPEC §7 settings list).
pub const SETTINGS_ROWS: usize = 6;

/// TopSelect grows one entry (M8): the 7 `PRESETS` plus MY BEY, the
/// player's garage-built custom top, at cursor index [`MY_BEY_INDEX`].
pub const TOP_SELECT_ENTRIES: usize = PRESETS.len() + 1;
/// TopSelect cursor value that means "MY BEY" (garage build) rather than a
/// `PRESETS` index (task spec: "MY BEY as an 8th pick (index 7 = custom)").
pub const MY_BEY_INDEX: usize = PRESETS.len();

/// Garage slot count (task spec: "5 slots x 4 parts"; SPEC §9 `[u8; 5]`).
pub const GARAGE_SLOTS: usize = 5;
/// Parts-per-slot count (task spec: "4 parts").
pub const GARAGE_PARTS_PER_SLOT: usize = 4;

/// MY BEY's fixed display name/flavor line (TopSelect/Garage screens) — the
/// garage build has no `roster::Preset` entry of its own (it's synthesized
/// from `garage::resolve`), so these live here instead.
pub const MY_BEY_NAME: &str = "MY BEY";
pub const MY_BEY_FLAVOR: &str = "Built in the garage. Whatever it is, it's yours.";

/// Salt XORed into the round seed for the AI's launch roll so the roll's RNG
/// stream is independent of the `World`'s own stream (both derive from the
/// same round seed).
const AI_ROLL_SEED_SALT: u64 = 0x00A1_0000_0000_0001;

/// Salt XORed into the round seed for the Fight-phase utility AI's own
/// [`ai::AiState`] (M5, SPEC §11) — distinct from both `AI_ROLL_SEED_SALT`
/// and the `World`'s own `rng` so the three RNG streams derived from the same
/// round seed never correlate.
const AI_FIGHT_SEED_SALT: u64 = 0x00A2_0000_0000_0001;

/// Match sub-phase (SPEC §7: `Intro > Launch > Fight > Decided >
/// RoundResult -(loop)-> MatchOver`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchPhase {
    Intro,
    Launch,
    Fight,
    Decided,
    RoundResult,
}

/// Top-level screen (SPEC §7 diagram, verbatim set).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Boot,
    Title,
    MainMenu,
    TopSelect,
    Garage,
    Settings,
    Match(MatchPhase),
    MatchOver,
}

/// Screen-shake intensity setting (SPEC §7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShakeLevel {
    Off,
    Low,
    Normal,
    High,
}

impl ShakeLevel {
    pub fn label(self) -> &'static str {
        match self {
            ShakeLevel::Off => "OFF",
            ShakeLevel::Low => "LOW",
            ShakeLevel::Normal => "NORMAL",
            ShakeLevel::High => "HIGH",
        }
    }
}

/// Window scale setting (SPEC §7 / game_design.md §9: scales the window,
/// never the internal 960x540 buffer). Applied by the platform layer: `main.rs`
/// maps this to `platform::win32::WindowScaleMode` and calls
/// `Platform::set_window_scale` on boot and whenever Settings is left with a
/// changed value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowScale {
    X1,
    X1_5,
    X2,
    Fullscreen,
}

impl WindowScale {
    pub fn label(self) -> &'static str {
        match self {
            WindowScale::X1 => "1X",
            WindowScale::X1_5 => "1.5X",
            WindowScale::X2 => "2X",
            WindowScale::Fullscreen => "FULL",
        }
    }
}

/// The settings block (SPEC §7). Persistence is M8; for now this lives only
/// in `FlowState` and is applied where relevant (difficulty feeds the AI's
/// launch roll; the rest are stored for their consumers to read).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GameSettings {
    /// Music volume, `0..=10`.
    pub music_vol: u8,
    /// SFX volume, `0..=10`.
    pub sfx_vol: u8,
    pub shake: ShakeLevel,
    pub difficulty: Difficulty,
    pub window_scale: WindowScale,
    pub colorblind: bool,
}

impl Default for GameSettings {
    fn default() -> Self {
        Self {
            music_vol: 8,
            sfx_vol: 8,
            shake: ShakeLevel::Normal,
            difficulty: Difficulty::Normal,
            window_scale: WindowScale::X1,
            colorblind: false,
        }
    }
}

/// Human-readable tier name for the settings screen (defined here rather
/// than in `minigame` because it's pure UI concern).
pub fn difficulty_label(d: Difficulty) -> &'static str {
    match d {
        Difficulty::Easy => "EASY",
        Difficulty::Normal => "NORMAL",
        Difficulty::Hard => "HARD",
        Difficulty::Ace => "ACE",
    }
}

/// Per-frame rising-edge summary of the two input channels (`InputState` +
/// the out-of-band Esc bool). Pure function of (prev, current).
struct Edges {
    up: bool,
    down: bool,
    left: bool,
    right: bool,
    dash: bool,
    back: bool,
    esc: bool,
    /// Any `InputState` rising edge at all (buttons or directions) — the
    /// Title / RoundResult / MatchOver "press any key" trigger. Esc is NOT
    /// included (it always means back/quit, never "continue").
    any_key: bool,
}

fn detect_edges(prev: InputState, cur: InputState, prev_esc: bool, esc_now: bool) -> Edges {
    let up = cur.dir_y == DIR_UP && prev.dir_y != DIR_UP;
    let down = cur.dir_y == DIR_DOWN && prev.dir_y != DIR_DOWN;
    let left = cur.dir_x == DIR_LEFT && prev.dir_x != DIR_LEFT;
    let right = cur.dir_x == DIR_RIGHT && prev.dir_x != DIR_RIGHT;
    let dash = cur.dash && !prev.dash;
    let special = cur.special && !prev.special;
    let back = cur.guard && !prev.guard;
    let hop = cur.hop && !prev.hop;
    let carve = cur.carve && !prev.carve;
    let anchor = cur.anchor && !prev.anchor;
    let esc = esc_now && !prev_esc;
    let any_key = dash || special || back || hop || carve || anchor || up || down || left || right;
    Edges {
        up,
        down,
        left,
        right,
        dash,
        back,
        esc,
        any_key,
    }
}

/// Whole game-flow state: screen machine, menu cursors, selected tops, match
/// score, and the live round `World`. Everything here is plain data driven
/// only by [`FlowState::advance`]'s `(InputState, esc)` arguments — no
/// wall-clock, no OS state — so a scripted input sequence replays the entire
/// UI deterministically (SPEC §7's flow test relies on this).
#[derive(Clone, Debug)]
pub struct FlowState {
    pub screen: Screen,
    /// Flow frames elapsed on the CURRENT screen (reset to 0 on every
    /// transition). Drives Boot/Intro/Decided timers and render-side blink
    /// phases.
    pub frame: u32,
    /// Flow frames elapsed since construction (never reset). Folded into the
    /// match seed at TopSelect confirm so different menu timings give
    /// different matches while an identical input script replays identically.
    pub total_frames: u64,
    pub menu_cursor: usize,
    pub settings_cursor: usize,
    /// TopSelect cursor: `0..PRESETS.len()` picks a roster preset,
    /// [`MY_BEY_INDEX`] picks the garage-built custom top (M8).
    pub select_cursor: usize,
    /// TopSelect pick locked in at match start: a `PRESETS` index, or
    /// [`MY_BEY_INDEX`] for MY BEY (M8). AI's own pick (`ai_pick`) is always
    /// a `PRESETS` index — see `begin_match`'s doc comment for why.
    pub p1_pick: usize,
    pub ai_pick: usize,
    /// The saved/live garage build: 5 part indices (SPEC §9 `[u8; 5]`,
    /// `garage::resolve` input). Persisted via `save_snapshot`/`apply_save`.
    pub parts: [u8; 5],
    /// Garage screen cursor: which of the 5 slots is selected (`0..5`).
    pub garage_slot: usize,
    /// Match score `[P1, AI]` (SPEC §6.5: first to 4).
    pub score: [u8; 2],
    /// 0-based round index; the round seed is `mix_seed(match_seed, round)`.
    /// Increments after EVERY decided round including draws (a replayed draw
    /// round gets a fresh seed — documented decision).
    pub round: u32,
    pub match_seed: u64,
    pub settings: GameSettings,
    /// Set by the Quit menu item / Esc on MainMenu or Title; `main.rs` polls
    /// it and exits its loop. Never cleared by `advance`.
    pub quit_requested: bool,
    /// P1's live launch minigame (reset at each round start).
    pub minigame: MinigameState,
    /// The AI's pre-rolled launch (computed once at Intro→Launch).
    pub ai_choice: Option<LaunchChoice>,
    /// The Fight-phase utility AI's own state (M5, SPEC §11): reaction-delay
    /// buffer, committed-plan timers, its own `Rng` stream. Lives outside
    /// `World` (not sim state); (re)created fresh each round in
    /// `spawn_fight_world` from the round seed + `AI_FIGHT_SEED_SALT`.
    pub ai_state: Option<ai::AiState>,
    /// Current round world. During Intro/Launch this holds a cosmetic
    /// PREVIEW world (never stepped) so the renderer has an arena + tops to
    /// draw; from Fight on it is the real stepped sim.
    pub world: Option<World>,
    /// Snapshot taken immediately before the most recent `World::step`, for
    /// `pose_lerp` render interpolation (SPEC §5).
    pub world_prev: Option<World>,
    /// Every `BattleEvent` emitted during THIS flow frame, across all of the
    /// frame's sim sub-steps. `World::step` clears `World::events` at the top
    /// of each call, so with [`SIM_STEPS_PER_FLOW_FRAME`] = 2 a consumer
    /// reading `world.events` after `advance()` only ever sees the LAST
    /// sub-step's events — the first sub-step's hits would be silently lost
    /// (M6-B finding: dropped SFX in the live loop). Render/audio consumers
    /// must read this buffer instead; it is presentation plumbing, not sim
    /// state (never hashed, sim never reads it).
    pub frame_events: Vec<BattleEvent>,
    /// The outcome that decided the current/most recent round.
    pub last_outcome: Option<Outcome>,
    /// Points awarded for the decided round and to whom (None = draw).
    pub last_winner: Option<u8>,
    pub last_points: u8,
    /// The Crash-Out/Over/Survivor/Draw classification of the decided
    /// round, mirroring `last_winner`'s lifecycle exactly (SPEC §6.5).
    pub last_round_end: Option<crate::combat::RoundEnd>,
    base_seed: u64,
    prev_input: InputState,
    prev_esc: bool,
}

/// Intro banner text as a pure function of the Intro frame counter (UI
/// logic in core, drawing in render — SPEC §7).
pub fn intro_banner(frame: u32) -> &'static str {
    if frame < INTRO_READY_FRAMES {
        "READY?"
    } else if frame < INTRO_READY_FRAMES + INTRO_NUMERAL_FRAMES {
        "3"
    } else if frame < INTRO_READY_FRAMES + 2 * INTRO_NUMERAL_FRAMES {
        "2"
    } else if frame < INTRO_READY_FRAMES + 3 * INTRO_NUMERAL_FRAMES {
        "1"
    } else {
        "GO!"
    }
}

/// Outcome banner text (game_design.md §7 banner list): a Crash-Out kill
/// (SPEC §6.5) always shows "CRASH-OUT!!" regardless of the underlying
/// `Outcome`; otherwise the per-outcome mapping below applies.
pub fn outcome_banner(outcome: Outcome, end: Option<crate::combat::RoundEnd>) -> &'static str {
    if end == Some(crate::combat::RoundEnd::CrashOut) {
        return "CRASH-OUT!!";
    }
    match outcome {
        Outcome::RingOut { .. } => "RING OUT!",
        Outcome::StaminaOut { .. } => "TOPPLE!",
        Outcome::Simultaneous => "DRAW!",
    }
}

/// **Superseded by [`ai::decide`] (M5, SPEC §11)** — the Fight phase now
/// drives its AI top through the real utility controller; this scripted
/// dummy is kept only because existing tests exercise it directly. Plain
/// chase toward P1 (top index 0), dashing when far and off cooldown. Pure
/// function of the world's public state — no RNG, no trig (sign tests only).
pub fn chase_input(world: &World) -> InputState {
    const DEADZONE: f32 = 0.25;
    const DASH_DIST_SQ: f32 = 9.0; // dash when > 3 m away

    let me = &world.tops[1];
    let target = &world.tops[0];
    let dx = target.pos.x - me.pos.x;
    let dz = target.pos.z - me.pos.z;

    // `input_dir_xz` (physics) maps dir_x -> +x and dir_y -> -z.
    let dir_x = if dx > DEADZONE {
        1
    } else if dx < -DEADZONE {
        -1
    } else {
        0
    };
    let dir_y = if dz > DEADZONE {
        -1
    } else if dz < -DEADZONE {
        1
    } else {
        0
    };

    let dist_sq = dx * dx + dz * dz;
    InputState {
        dir_x,
        dir_y,
        dash: dist_sq > DASH_DIST_SQ && me.dash_cd == 0,
        ..Default::default()
    }
}

fn wrap_tau(a: f32) -> f32 {
    if a >= TAU {
        a - TAU
    } else if a < 0.0 {
        a + TAU
    } else {
        a
    }
}

/// Cycle an index within `n` variants by `delta` in {-1, +1}, wrapping.
fn cycle_index(i: usize, n: usize, delta: i32) -> usize {
    debug_assert!(n > 0);
    let n = n as i32;
    (((i as i32 + delta) + n) % n) as usize
}

fn cycle_shake(s: ShakeLevel, delta: i32) -> ShakeLevel {
    let order = [
        ShakeLevel::Off,
        ShakeLevel::Low,
        ShakeLevel::Normal,
        ShakeLevel::High,
    ];
    let i = order.iter().position(|&x| x == s).unwrap_or(0);
    order[cycle_index(i, order.len(), delta)]
}

fn cycle_difficulty(d: Difficulty, delta: i32) -> Difficulty {
    let order = [
        Difficulty::Easy,
        Difficulty::Normal,
        Difficulty::Hard,
        Difficulty::Ace,
    ];
    let i = order.iter().position(|&x| x == d).unwrap_or(0);
    order[cycle_index(i, order.len(), delta)]
}

fn cycle_window_scale(w: WindowScale, delta: i32) -> WindowScale {
    let order = [
        WindowScale::X1,
        WindowScale::X1_5,
        WindowScale::X2,
        WindowScale::Fullscreen,
    ];
    let i = order.iter().position(|&x| x == w).unwrap_or(0);
    order[cycle_index(i, order.len(), delta)]
}

fn adjust_volume(v: u8, delta: i32) -> u8 {
    (v as i32 + delta).clamp(0, 10) as u8
}

/// Number of selectable parts for garage slot `slot` (0 = Frame, 1..=4 =
/// the stat-delta slots) — reads `garage::FRAMES`/`PART_SLOTS`'s actual
/// lengths rather than hardcoding 4 everywhere `advance`'s Garage arm needs
/// a wrap width.
fn garage_slot_width(slot: usize) -> usize {
    if slot == 0 {
        garage::FRAMES.len()
    } else {
        garage::PART_SLOTS[slot - 1].len()
    }
}

fn adjust_setting(s: &mut GameSettings, row: usize, delta: i32) {
    match row {
        0 => s.music_vol = adjust_volume(s.music_vol, delta),
        1 => s.sfx_vol = adjust_volume(s.sfx_vol, delta),
        2 => s.shake = cycle_shake(s.shake, delta),
        3 => s.difficulty = cycle_difficulty(s.difficulty, delta),
        4 => s.window_scale = cycle_window_scale(s.window_scale, delta),
        _ => s.colorblind = !s.colorblind,
    }
}

impl FlowState {
    /// Fresh flow at the Boot screen. `base_seed` is the only entropy the
    /// flow ever receives; everything downstream (match seed, AI pick, AI
    /// launch roll, round worlds) derives from it plus input timing.
    pub fn new(base_seed: u64) -> Self {
        Self {
            screen: Screen::Boot,
            frame: 0,
            total_frames: 0,
            menu_cursor: 0,
            settings_cursor: 0,
            select_cursor: 0,
            p1_pick: 0,
            ai_pick: 0,
            parts: DEFAULT_PARTS,
            garage_slot: 0,
            score: [0, 0],
            round: 0,
            match_seed: 0,
            settings: GameSettings::default(),
            quit_requested: false,
            minigame: MinigameState::new(PRESETS[0].spin_dir),
            ai_choice: None,
            ai_state: None,
            world: None,
            world_prev: None,
            frame_events: Vec::new(),
            last_outcome: None,
            last_winner: None,
            last_points: 0,
            last_round_end: None,
            base_seed,
            prev_input: InputState::default(),
            prev_esc: false,
        }
    }

    /// Seed for the current round (SPEC §6.5).
    pub fn round_seed(&self) -> u64 {
        mix_seed(self.match_seed, self.round)
    }

    /// Resolve the CURRENT garage build (module docs: recomputed live so the
    /// Garage screen's preview always reflects `parts` immediately).
    pub fn garage_build(&self) -> garage::CustomBuild {
        garage::resolve(self.parts)
    }

    /// A `Preset`-shaped view of whatever TopSelect index `pick` refers to:
    /// a real roster entry for `0..PRESETS.len()`, or MY BEY (synthesized
    /// from the live garage build) for [`MY_BEY_INDEX`]. This is the ONLY
    /// place the custom top's identity is materialized; every downstream
    /// consumer (`preview_world`, `spawn_fight_world`, rendering) reads a
    /// plain `Preset` either way, so the sim/render code never special-cases
    /// "is this a preset or a custom top" — it just sees a `Preset`
    /// (task spec / SPEC §12 gate: no special-casing).
    ///
    /// Out-of-range `pick` values (defensive only — `select_cursor`/
    /// `p1_pick`/`ai_pick` are always kept in `0..=MY_BEY_INDEX` by
    /// `advance`/`begin_match`) fall back to preset 0.
    pub fn preset_view(&self, pick: usize) -> Preset {
        if pick == MY_BEY_INDEX {
            let build = self.garage_build();
            Preset {
                name: MY_BEY_NAME,
                flavor: MY_BEY_FLAVOR,
                stats: build.stats,
                spin_dir: build.spin_dir,
                accent: build.accent,
                silhouette: build.silhouette,
            }
        } else {
            PRESETS[pick.min(PRESETS.len() - 1)]
        }
    }

    /// Cosmetic never-stepped preview world for Intro/Launch rendering: P1's
    /// top on the launch circle at the minigame's live heading/depth, the AI
    /// opposite. Power/quality are fixed placeholders — this world never
    /// becomes the fight sim (the real one is spawned fresh on power lock).
    fn preview_world(&self) -> World {
        let p1_preset = self.preset_view(self.p1_pick);
        let ai_preset = self.preset_view(self.ai_pick);
        let params = [
            LaunchParams {
                heading: self.minigame.heading,
                depth: self.minigame.depth,
                power: 0.5,
                quality: 1.0,
                spin_dir: self.minigame.spin_dir,
                stats: p1_preset.stats,
                special_id: SpecialId::from_silhouette(p1_preset.silhouette),
            },
            LaunchParams {
                heading: wrap_tau(self.minigame.heading + PI),
                depth: 0.7,
                power: 0.5,
                quality: 1.0,
                spin_dir: ai_preset.spin_dir,
                stats: ai_preset.stats,
                special_id: SpecialId::from_silhouette(ai_preset.silhouette),
            },
        ];
        World::launch(self.round_seed(), params)
    }

    fn refresh_preview(&mut self) {
        let w = self.preview_world();
        self.world_prev = Some(w.clone());
        self.world = Some(w);
    }

    /// TopSelect confirm: lock picks, derive the match seed, roll the AI's
    /// roster pick, zero the score. (Screen transitions happen only at the
    /// call sites inside `advance`'s match — helpers never change `screen`.)
    ///
    /// The AI always picks a real `PRESETS` entry, never MY BEY (documented
    /// decision: the garage build is the player's own creation, and letting
    /// the AI roll it too would need a second, independently-tuned "AI
    /// plays a custom top" balance pass that's out of scope here — simplest
    /// or the AI just plays the roster, which the balance tests already
    /// cover).
    fn begin_match(&mut self) {
        self.p1_pick = self.select_cursor;
        self.match_seed = mix_seed(self.base_seed, self.total_frames as u32);
        let mut rng = Rng::new(self.match_seed);
        self.ai_pick = (rng.next_u64() % PRESETS.len() as u64) as usize;
        self.score = [0, 0];
        self.round = 0;
        self.last_outcome = None;
        self.last_winner = None;
        self.last_points = 0;
        self.last_round_end = None;
        self.begin_round();
    }

    /// Per-round reset: fresh minigame (defaulted to the pick's preset spin
    /// direction) + preview world for the Intro/Launch backdrop.
    fn begin_round(&mut self) {
        self.minigame = MinigameState::new(self.preset_view(self.p1_pick).spin_dir);
        self.ai_choice = None;
        self.refresh_preview();
    }

    /// Intro→Launch: pre-roll the AI's launch quality (game_design.md §4:
    /// "the AI rolls its own quality from skill params"), difficulty-scaled.
    fn roll_ai_choice(&mut self) {
        let mut rng = Rng::new(self.round_seed() ^ AI_ROLL_SEED_SALT);
        self.ai_choice = Some(ai_roll(
            &mut rng,
            self.settings.difficulty,
            PRESETS[self.ai_pick].spin_dir,
            0.0, // heading placeholder; the AI spawns opposite P1's locked heading
            0.7,
        ));
    }

    /// Power lock: spawn the real fight world from both launch choices
    /// (P1's minigame result + the AI's pre-rolled quality), applying the
    /// PERFECT bonus meter and Overcharge starting tilt to the spawned tops.
    /// P1's `LaunchParams` come from `preset_view(p1_pick)` — when
    /// `p1_pick == MY_BEY_INDEX` that's the garage build's resolved
    /// `Stats`/`spin_dir`/`special_id`, fed through the EXACT same
    /// `LaunchParams` -> `World::launch` -> `spawn_launch_top` path a preset
    /// uses (SPEC §12 gate: the sim never special-cases a custom top).
    fn spawn_fight_world(&mut self, choice: LaunchChoice) {
        let p1_preset = self.preset_view(self.p1_pick);
        let ai_preset = self.preset_view(self.ai_pick);
        let ai_choice = self.ai_choice.unwrap_or(LaunchChoice {
            heading: 0.0,
            depth: 0.7,
            spin_dir: ai_preset.spin_dir,
            power_frac: 0.5,
            quality: 1.0,
            bonus_meter: 0.0,
            start_tilt: 0.0,
        });
        let params = [
            LaunchParams {
                heading: choice.heading,
                depth: choice.depth,
                power: choice.power_frac,
                quality: choice.quality,
                spin_dir: choice.spin_dir,
                stats: p1_preset.stats,
                special_id: SpecialId::from_silhouette(p1_preset.silhouette),
            },
            LaunchParams {
                heading: wrap_tau(choice.heading + PI),
                depth: ai_choice.depth,
                power: ai_choice.power_frac,
                quality: ai_choice.quality,
                spin_dir: ai_choice.spin_dir,
                stats: ai_preset.stats,
                special_id: SpecialId::from_silhouette(ai_preset.silhouette),
            },
        ];
        let mut world = World::launch(self.round_seed(), params);
        world.tops[0].meter = choice.bonus_meter.clamp(0.0, 100.0);
        world.tops[0].tilt = Vec2::new(choice.start_tilt, 0.0);
        world.tops[1].meter = ai_choice.bonus_meter.clamp(0.0, 100.0);
        world.tops[1].tilt = Vec2::new(ai_choice.start_tilt, 0.0);
        self.world_prev = Some(world.clone());
        self.world = Some(world);
        // Fresh AI controller state for the Fight phase (M5): seeded from
        // this round's seed, mixed with a salt distinct from `World::rng`
        // and the launch-roll RNG so the three streams never correlate.
        self.ai_state = Some(ai::AiState::new(self.round_seed() ^ AI_FIGHT_SEED_SALT));
    }

    /// Match abort / teardown (Esc during a match, or MatchOver exit).
    fn clear_match(&mut self) {
        self.world = None;
        self.world_prev = None;
        self.ai_choice = None;
        self.ai_state = None;
    }

    /// Advance one flow frame (module docs). ALL screen transitions are
    /// explicit enum edges inside the single `match` below (SPEC §7);
    /// helper methods mutate data but never `screen`.
    pub fn advance(&mut self, input: InputState, esc: bool) {
        let e = detect_edges(self.prev_input, input, self.prev_esc, esc);
        let mut next: Option<Screen> = None;
        // Fresh event window every flow frame; only the Fight arm refills it.
        self.frame_events.clear();

        match self.screen {
            // Boot ▶ Title: auto after BOOT_FRAMES.
            Screen::Boot => {
                if self.frame >= BOOT_FRAMES {
                    next = Some(Screen::Title);
                }
            }

            // Title ▶ MainMenu on any key; Esc quits.
            Screen::Title => {
                if e.esc {
                    self.quit_requested = true;
                } else if e.any_key {
                    next = Some(Screen::MainMenu);
                }
            }

            // MainMenu ▶ TopSelect | Garage | Settings | Quit.
            Screen::MainMenu => {
                let n = MAIN_MENU_ITEMS.len();
                if e.up {
                    self.menu_cursor = (self.menu_cursor + n - 1) % n;
                }
                if e.down {
                    self.menu_cursor = (self.menu_cursor + 1) % n;
                }
                if e.esc || e.back {
                    // Direct quit, no confirm (module docs).
                    self.quit_requested = true;
                } else if e.dash {
                    match self.menu_cursor {
                        MENU_QUICK_BATTLE => next = Some(Screen::TopSelect),
                        MENU_GARAGE => next = Some(Screen::Garage),
                        MENU_SETTINGS => next = Some(Screen::Settings),
                        MENU_QUIT => self.quit_requested = true,
                        _ => {}
                    }
                }
            }

            // TopSelect ▶ Match(Intro) on pick | back to MainMenu. `n`
            // includes MY BEY as an 8th entry (M8: TOP_SELECT_ENTRIES =
            // PRESETS.len() + 1, cursor index MY_BEY_INDEX).
            Screen::TopSelect => {
                let n = TOP_SELECT_ENTRIES;
                if e.up || e.left {
                    self.select_cursor = (self.select_cursor + n - 1) % n;
                }
                if e.down || e.right {
                    self.select_cursor = (self.select_cursor + 1) % n;
                }
                if e.esc || e.back {
                    next = Some(Screen::MainMenu);
                } else if e.dash {
                    self.begin_match();
                    next = Some(Screen::Match(MatchPhase::Intro));
                }
            }

            // Garage ▶ MainMenu (M8: real part-swapping, SPEC §7). `garage_slot`
            // (up/down) selects which of the 5 slots is active; left/right
            // cycles that slot's part index within its own option count
            // (4 for every slot — `garage::FRAMES`/`PART_SLOTS` are all
            // 4-wide, but this reads each slot's own length rather than
            // hardcoding 4, so a future slot with a different width Just
            // Works). `garage::resolve` is recomputed live by
            // `FlowState::garage_build` — nothing here caches a stale build.
            Screen::Garage => {
                if e.up {
                    self.garage_slot = (self.garage_slot + GARAGE_SLOTS - 1) % GARAGE_SLOTS;
                }
                if e.down {
                    self.garage_slot = (self.garage_slot + 1) % GARAGE_SLOTS;
                }
                if e.left || e.right {
                    let slot = self.garage_slot;
                    let width = garage_slot_width(slot);
                    let delta: i32 = if e.right { 1 } else { -1 };
                    let cur = self.parts[slot] as i32;
                    self.parts[slot] =
                        (((cur + delta) % width as i32 + width as i32) % width as i32) as u8;
                }
                if e.esc || e.back {
                    next = Some(Screen::MainMenu);
                }
            }

            // Settings ▶ MainMenu; rows adjusted in place.
            Screen::Settings => {
                if e.up {
                    self.settings_cursor =
                        (self.settings_cursor + SETTINGS_ROWS - 1) % SETTINGS_ROWS;
                }
                if e.down {
                    self.settings_cursor = (self.settings_cursor + 1) % SETTINGS_ROWS;
                }
                if e.right {
                    adjust_setting(&mut self.settings, self.settings_cursor, 1);
                }
                if e.left {
                    adjust_setting(&mut self.settings, self.settings_cursor, -1);
                }
                if e.esc || e.back {
                    next = Some(Screen::MainMenu);
                }
            }

            // Match: Intro ▶ Launch (countdown; input ignored except Esc).
            Screen::Match(MatchPhase::Intro) => {
                if e.esc {
                    self.clear_match();
                    next = Some(Screen::MainMenu);
                } else if self.frame >= INTRO_TOTAL_FRAMES {
                    self.roll_ai_choice();
                    next = Some(Screen::Match(MatchPhase::Launch));
                }
            }

            // Match: Launch ▶ Fight on power lock.
            Screen::Match(MatchPhase::Launch) => {
                if e.esc {
                    self.clear_match();
                    next = Some(Screen::MainMenu);
                } else {
                    let mut locked = None;
                    for _ in 0..SIM_STEPS_PER_FLOW_FRAME {
                        if let Some(c) = self.minigame.step(input) {
                            locked = Some(c);
                            break;
                        }
                    }
                    if let Some(choice) = locked {
                        self.spawn_fight_world(choice);
                        next = Some(Screen::Match(MatchPhase::Fight));
                    } else {
                        // Keep the cosmetic preview tracking the live aim.
                        self.refresh_preview();
                    }
                }
            }

            // Match: Fight ▶ Decided on World outcome.
            Screen::Match(MatchPhase::Fight) => {
                if e.esc {
                    self.clear_match();
                    next = Some(Screen::MainMenu);
                } else if let Some(mut world) = self.world.take() {
                    // Difficulty comes straight from settings (M5, SPEC
                    // §11); resolved once outside the loop so the per-step
                    // AI call below never needs to borrow `self.settings`
                    // alongside the mutable `self.ai_state` borrow.
                    let params = ai::tier(self.settings.difficulty);
                    for _ in 0..SIM_STEPS_PER_FLOW_FRAME {
                        if world.outcome.is_some() {
                            break;
                        }
                        let ai_input = match self.ai_state.as_mut() {
                            Some(state) => ai::decide(state, &world, 1, &params),
                            // Defensive fallback only: `ai_state` is always
                            // populated by `spawn_fight_world` before Fight
                            // is reachable, but keep the old dummy so a
                            // missing-state world still has a live opponent
                            // rather than an idle one.
                            None => chase_input(&world),
                        };
                        self.world_prev = Some(world.clone());
                        world.step([input, ai_input]);
                        self.frame_events.extend_from_slice(&world.events);
                    }
                    if let Some(outcome) = world.outcome {
                        self.last_outcome = Some(outcome);
                        match crate::combat::round_points(&world) {
                            Some((winner, end)) => {
                                self.score[winner as usize] =
                                    self.score[winner as usize].saturating_add(end.points());
                                self.last_winner = Some(winner);
                                self.last_points = end.points();
                                self.last_round_end = Some(end);
                            }
                            None => {
                                self.last_winner = None;
                                self.last_points = 0;
                                self.last_round_end = None;
                            }
                        }
                        next = Some(Screen::Match(MatchPhase::Decided));
                    }
                    self.world = Some(world);
                } else {
                    // Defensive: a Fight without a world is unreachable via
                    // the public API; recover to the menu rather than hang.
                    next = Some(Screen::MainMenu);
                }
            }

            // Match: Decided ▶ RoundResult after the banner hold.
            Screen::Match(MatchPhase::Decided) => {
                if e.esc {
                    self.clear_match();
                    next = Some(Screen::MainMenu);
                } else if self.frame >= DECIDED_HOLD_FRAMES {
                    next = Some(Screen::Match(MatchPhase::RoundResult));
                }
            }

            // Match: RoundResult ▶ Match(Intro) (next round) | MatchOver.
            Screen::Match(MatchPhase::RoundResult) => {
                if e.esc {
                    self.clear_match();
                    next = Some(Screen::MainMenu);
                } else if e.any_key {
                    if self.score[0] >= MATCH_WIN_POINTS || self.score[1] >= MATCH_WIN_POINTS {
                        next = Some(Screen::MatchOver);
                    } else {
                        self.round += 1;
                        self.begin_round();
                        next = Some(Screen::Match(MatchPhase::Intro));
                    }
                }
            }

            // MatchOver ▶ MainMenu.
            Screen::MatchOver => {
                if e.any_key || e.esc {
                    self.clear_match();
                    next = Some(Screen::MainMenu);
                }
            }
        }

        if let Some(s) = next {
            self.screen = s;
            self.frame = 0;
        } else {
            self.frame = self.frame.wrapping_add(1);
        }
        self.total_frames = self.total_frames.wrapping_add(1);
        self.prev_input = input;
        self.prev_esc = esc;
    }

    /// Apply a decoded save (Task M8-6, called once on boot before the main
    /// loop starts): overwrites the live garage `parts` and `settings` with
    /// the save's values. `save::decode` already guarantees `s` is either a
    /// fully-valid save or `SaveState::default()` — this method never needs
    /// to validate anything itself, it just applies both fields together
    /// (SPEC §9 "never partially apply" is `decode`'s job, not this one's).
    pub fn apply_save(&mut self, s: SaveState) {
        self.parts = s.parts;
        self.settings = s.settings;
    }

    /// The persistable slice of flow state: current garage `parts` plus
    /// `settings`, ready for `save::encode`. See `main.rs` for the exact
    /// persist-trigger points (Task M8-6 docs there: Garage-exit,
    /// Settings-exit, and quit).
    pub fn save_snapshot(&self) -> ([u8; 5], GameSettings) {
        (self.parts, self.settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intro_banner_segments_cover_the_whole_countdown() {
        assert_eq!(intro_banner(0), "READY?");
        assert_eq!(intro_banner(INTRO_READY_FRAMES - 1), "READY?");
        assert_eq!(intro_banner(INTRO_READY_FRAMES), "3");
        assert_eq!(intro_banner(INTRO_READY_FRAMES + INTRO_NUMERAL_FRAMES), "2");
        assert_eq!(
            intro_banner(INTRO_READY_FRAMES + 2 * INTRO_NUMERAL_FRAMES),
            "1"
        );
        assert_eq!(
            intro_banner(INTRO_READY_FRAMES + 3 * INTRO_NUMERAL_FRAMES),
            "GO!"
        );
        assert_eq!(intro_banner(INTRO_TOTAL_FRAMES - 1), "GO!");
        assert_eq!(INTRO_TOTAL_FRAMES, 120);
    }

    #[test]
    fn settings_cycles_wrap_in_both_directions() {
        assert_eq!(cycle_shake(ShakeLevel::Off, -1), ShakeLevel::High);
        assert_eq!(cycle_shake(ShakeLevel::High, 1), ShakeLevel::Off);
        assert_eq!(cycle_difficulty(Difficulty::Ace, 1), Difficulty::Easy);
        assert_eq!(
            cycle_window_scale(WindowScale::X1, -1),
            WindowScale::Fullscreen
        );
        assert_eq!(adjust_volume(10, 1), 10);
        assert_eq!(adjust_volume(0, -1), 0);
    }

    #[test]
    fn wrap_tau_keeps_angles_in_range() {
        assert!((wrap_tau(TAU + 0.5) - 0.5).abs() < 1e-6);
        assert!((wrap_tau(-0.5) - (TAU - 0.5)).abs() < 1e-6);
        assert_eq!(wrap_tau(1.0), 1.0);
    }

    #[test]
    fn chase_input_steers_the_ai_toward_p1() {
        // Build a tiny world via the public launch API, then place the tops
        // at known offsets: P1 at +x/+z relative to the AI.
        let stats = PRESETS[3].stats; // Keystone
        let params = LaunchParams {
            heading: 0.0,
            depth: 0.7,
            power: 0.5,
            quality: 1.0,
            spin_dir: 1,
            stats,
            special_id: SpecialId::from_silhouette(PRESETS[3].silhouette),
        };
        let mut world = World::launch(1, [params, params]);
        world.tops[0].pos = crate::vec::Vec3::new(2.0, 0.0, 2.0);
        world.tops[1].pos = crate::vec::Vec3::new(0.0, 0.0, 0.0);
        let inp = chase_input(&world);
        // +x target -> dir_x = 1; +z target -> dir_y = -1 (dir_y maps to -z).
        assert_eq!(inp.dir_x, 1);
        assert_eq!(inp.dir_y, -1);
        // Distance ~2.83 m < 3 m: no dash.
        assert!(!inp.dash);

        world.tops[0].pos = crate::vec::Vec3::new(-5.0, 0.0, 0.0);
        let inp2 = chase_input(&world);
        assert_eq!(inp2.dir_x, -1);
        assert_eq!(inp2.dir_y, 0);
        assert!(inp2.dash, "far target with dash off cooldown should dash");
    }
}
