//! M4-B: battle HUD + menu/screen drawing (game_design.md §7). Pure
//! rendering: every function here is a deterministic function of its
//! arguments (`ui_frame` drives blink/pulse phases — a frame counter passed
//! in by the caller, never wall-clock, never RNG). UI *logic* (cursors,
//! selections, transitions) lives in `floppy_core::flow`; this module only
//! draws what it is told (SPEC §7).
//!
//! Everything is built from three primitives: the 5x7 bitmap font
//! (`text.rs`), solid rects (`Frame::set` loops), and a bounding-box-scanned
//! arc that classifies pixels with `fixmath::atan2` (the sanctioned
//! deterministic atan2 — no libm).

use crate::frame::Frame;
use crate::text;
use floppy_core::fixmath;
use floppy_core::flow::{GameSettings, MAIN_MENU_ITEMS};
use floppy_core::minigame::{
    self, MinigameState, Stage, GOOD_HI, GOOD_LO, PERFECT_CENTER, PERFECT_HALF_WIDTH,
};
use floppy_core::physics::{World, TUNE};
use floppy_core::roster::{Preset, PRESETS};

// ---------------------------------------------------------------------------
// Palette (game_design.md §6): dark base, neon accents, ice-white highlights.
// ---------------------------------------------------------------------------

/// "Void" background.
pub const COL_BG: u32 = 0x000A_0A14;
/// Ice-white highlight.
pub const COL_ICE: u32 = 0x00F0_FFFF;
/// Default body text.
pub const COL_TEXT: u32 = 0x00C0_D0E0;
/// De-emphasized text / empty gauge track.
pub const COL_DIM: u32 = 0x004A_5A75;
/// Cursor / accent cyan (menu chevron, underline).
pub const COL_CURSOR: u32 = 0x0000_E5D0;
/// Warning red (overcharge zone).
pub const COL_WARN: u32 = 0x00FF_2D55;
/// Gold (match-win celebration, game_design.md §5 "round/match win").
pub const COL_GOLD: u32 = 0x00FF_D400;
/// Dark panel/track fill.
const COL_TRACK: u32 = 0x001E_2A44;
/// Banner drop shadow.
const COL_SHADOW: u32 = 0x0014_1826;

// ---------------------------------------------------------------------------
// Primitive helpers.
// ---------------------------------------------------------------------------

/// Solid axis-aligned rect; clipped by `Frame::set` (never panics).
pub fn fill_rect(frame: &mut Frame, x: i32, y: i32, w: i32, h: i32, color: u32) {
    for yy in y..y + h {
        for xx in x..x + w {
            frame.set(xx, yy, color);
        }
    }
}

/// 1px rect outline.
pub fn rect_outline(frame: &mut Frame, x: i32, y: i32, w: i32, h: i32, color: u32) {
    fill_rect(frame, x, y, w, 1, color);
    fill_rect(frame, x, y + h - 1, w, 1, color);
    fill_rect(frame, x, y, 1, h, color);
    fill_rect(frame, x + w - 1, y, 1, h, color);
}

/// Neon diamond pip (game_design.md §7 score pips): `filled` paints the
/// solid rhombus, otherwise a 2px hollow outline.
pub fn draw_diamond(frame: &mut Frame, cx: i32, cy: i32, r: i32, color: u32, filled: bool) {
    for dy in -r..=r {
        for dx in -r..=r {
            let m = dx.abs() + dy.abs();
            let hit = if filled { m <= r } else { m == r || m == r - 1 };
            if hit {
                frame.set(cx + dx, cy + dy, color);
            }
        }
    }
}

/// Text centered horizontally at `y`.
fn draw_text_centered(frame: &mut Frame, y: i32, scale: usize, color: u32, s: &str) {
    let (w, _) = text::measure(s, scale);
    let x = (frame.w as i32 - w as i32) / 2;
    text::draw_text(frame, x, y, scale, color, s);
}

// ---------------------------------------------------------------------------
// Banners (game_design.md §7: READY? / 3 / 2 / 1 / GO! / RING OUT! / ...).
// ---------------------------------------------------------------------------

/// Centered large banner text with a drop shadow, vertically centered a
/// little above the frame middle so it doesn't cover the tops.
pub fn draw_banner(frame: &mut Frame, banner: &str, scale: usize, color: u32) {
    let (_, h) = text::measure(banner, scale);
    let y = (frame.h as i32 - h as i32) / 2 - 40;
    let (w, _) = text::measure(banner, scale);
    let x = (frame.w as i32 - w as i32) / 2;
    let off = (scale as i32 / 2).max(1);
    text::draw_text(frame, x + off, y + off, scale, COL_SHADOW, banner);
    text::draw_text(frame, x, y, scale, color, banner);
}

// ---------------------------------------------------------------------------
// Battle HUD: fixed corner panels + score pips (game_design.md §7).
// ---------------------------------------------------------------------------

/// Stamina arc geometry: 270-degree sweep with the 90-degree gap opening
/// downward, drawn by scanning the bounding box and classifying each pixel
/// by radius + angle (`fixmath::atan2`, fixed cost, no libm).
const ARC_R_OUTER: i32 = 26;
const ARC_R_INNER: i32 = 20;
/// The arc spans `[-3pi/4, +3pi/4]` measured clockwise from straight-up.
const ARC_HALF_SPAN: f32 = 2.356_194_5; // 3*pi/4

/// One player's HUD panel content, bundled so `draw_player_panel` stays
/// within a sane argument count. `spin`/`meter` come straight off the
/// `Top`'s public fields.
pub struct PlayerPanel<'a> {
    pub name: &'a str,
    pub accent: u32,
    pub spin: f32,
    pub spin_max: f32,
    pub meter: f32,
}

/// 270-degree stamina arc: `frac` full (`0..=1`) in `color`, remainder in
/// the dark track color.
pub fn draw_stamina_arc(frame: &mut Frame, cx: i32, cy: i32, frac: f32, color: u32) {
    let frac = frac.clamp(0.0, 1.0);
    let span = 2.0 * ARC_HALF_SPAN;
    for dy in -ARC_R_OUTER..=ARC_R_OUTER {
        for dx in -ARC_R_OUTER..=ARC_R_OUTER {
            let d2 = dx * dx + dy * dy;
            if !(ARC_R_INNER * ARC_R_INNER..=ARC_R_OUTER * ARC_R_OUTER).contains(&d2) {
                continue;
            }
            // Angle measured clockwise from straight-up (screen y grows
            // downward): up = 0, right = +pi/2, left = -pi/2.
            let ang = fixmath::atan2(dx as f32, -(dy as f32));
            if !(-ARC_HALF_SPAN..=ARC_HALF_SPAN).contains(&ang) {
                continue; // the bottom gap
            }
            let t = (ang + ARC_HALF_SPAN) / span;
            let c = if t <= frac { color } else { COL_TRACK };
            frame.set(cx + dx, cy + dy, c);
        }
    }
}

/// Fixed corner panel (game_design.md §7 "Battle HUD"): stamina arc in the
/// top's accent (flashing white at <= 20% spin, timed off `ui_frame`), RPM
/// numeral, and the 0..100 meter bar with an Armed glow (brighter fill +
/// pulsing border once the meter is full).
pub fn draw_player_panel(frame: &mut Frame, panel: &PlayerPanel, right_side: bool, ui_frame: u32) {
    let w = frame.w as i32;
    let frac = (panel.spin / panel.spin_max).clamp(0.0, 1.0);

    // Low-spin flash: white on a fast blink (render-only timing from the
    // caller-supplied frame counter, game_design.md §7).
    let low = frac <= 0.2;
    let arc_color = if low && (ui_frame / 6).is_multiple_of(2) {
        COL_ICE
    } else {
        panel.accent
    };

    let (arc_cx, name_x, text_x) = if right_side {
        let (name_w, _) = text::measure(panel.name, 2);
        (w - 40, w - 14 - name_w as i32, w - 76 - 100)
    } else {
        (40, 14, 76)
    };

    text::draw_text(frame, name_x, 10, 2, panel.accent, panel.name);
    draw_stamina_arc(frame, arc_cx, 74, frac, arc_color);

    // RPM numeral (plain scaled font — vector-AA numerals are M7 polish).
    let rpm = format!("{:>5}", panel.spin.max(0.0) as i32);
    let rpm_color = if low { COL_WARN } else { COL_ICE };
    text::draw_text(frame, text_x, 48, 1, COL_DIM, "RPM");
    text::draw_text(frame, text_x, 58, 2, rpm_color, &rpm);

    // Meter bar 0..100 with Armed glow.
    let armed = panel.meter >= 100.0;
    let bar_x = text_x;
    let bar_y = 84;
    let bar_w = 100;
    let bar_h = 8;
    let fill_w = ((panel.meter.clamp(0.0, 100.0) / 100.0) * bar_w as f32) as i32;
    fill_rect(frame, bar_x, bar_y, bar_w, bar_h, COL_TRACK);
    let fill_color = if armed { COL_ICE } else { panel.accent };
    fill_rect(frame, bar_x, bar_y, fill_w, bar_h, fill_color);
    let border = if armed && (ui_frame / 8).is_multiple_of(2) {
        COL_ICE
    } else if armed {
        panel.accent
    } else {
        COL_DIM
    };
    rect_outline(frame, bar_x - 1, bar_y - 1, bar_w + 2, bar_h + 2, border);
}

/// Score pips (game_design.md §7): 4 hollow neon diamonds per side of
/// center-top, filled left-to-right (P1) / right-to-left toward center (P2)
/// as points accrue; a 3-point Crash-Out simply fills 3 at once.
pub fn draw_score_pips(frame: &mut Frame, score: [u8; 2], accents: [u32; 2]) {
    let cx = frame.w as i32 / 2;
    let y = 24;
    let r = 8;
    let gap = 26;
    for k in 0..4i32 {
        // P1 pips grow leftward from center; nearest-to-center = first point.
        let filled_p1 = (k as u8) < score[0].min(4);
        draw_diamond(frame, cx - 24 - k * gap, y, r, accents[0], filled_p1);
        let filled_p2 = (k as u8) < score[1].min(4);
        draw_diamond(frame, cx + 24 + k * gap, y, r, accents[1], filled_p2);
    }
}

/// Full battle HUD composition: both panels + pips. `visuals` follows
/// `World::tops` index order (0 = P1 left panel, 1 = AI right panel).
pub fn draw_battle_hud(
    frame: &mut Frame,
    world: &World,
    visuals: [&Preset; 2],
    score: [u8; 2],
    ui_frame: u32,
) {
    let p1 = PlayerPanel {
        name: visuals[0].name,
        accent: visuals[0].accent,
        spin: world.tops[0].spin,
        spin_max: TUNE.spin_max,
        meter: world.tops[0].meter,
    };
    let p2 = PlayerPanel {
        name: visuals[1].name,
        accent: visuals[1].accent,
        spin: world.tops[1].spin,
        spin_max: TUNE.spin_max,
        meter: world.tops[1].meter,
    };
    draw_player_panel(frame, &p1, false, ui_frame);
    draw_player_panel(frame, &p2, true, ui_frame);
    draw_score_pips(frame, score, [visuals[0].accent, visuals[1].accent]);
}

// ---------------------------------------------------------------------------
// Boot / Title / menus (fancy vector title is M7; these are the readable
// first-pass text screens the task spec asks for).
// ---------------------------------------------------------------------------

pub fn draw_boot(frame: &mut Frame, ui_frame: u32) {
    draw_text_centered(frame, 240, 3, COL_DIM, "FLOPPY SPIN");
    // Simple deterministic "activity" dots.
    let dots = ((ui_frame / 8) % 4) as usize;
    let s = ["", ".", "..", "..."][dots];
    draw_text_centered(frame, 290, 2, COL_DIM, s);
}

pub fn draw_title(frame: &mut Frame, ui_frame: u32) {
    // Chunky title: cyan fill with a magenta drop-offset (game_design.md §7;
    // the vector version with sine jitter is M7).
    let title = "FLOPPY SPIN";
    let scale = 7;
    let (tw, th) = text::measure(title, scale);
    let x = (frame.w as i32 - tw as i32) / 2;
    let y = 140;
    text::draw_text(frame, x + 4, y + 4, scale, 0x00B0_1878, title);
    text::draw_text(frame, x, y, scale, COL_CURSOR, title);
    // Accent underline.
    fill_rect(frame, x, y + th as i32 + 10, tw as i32, 4, COL_CURSOR);

    // PRESS START blink (~1.2 s cycle at 60 flow-fps: 36 on / 36 off).
    if (ui_frame / 36).is_multiple_of(2) {
        draw_text_centered(frame, 380, 2, COL_ICE, "PRESS ANY KEY");
    }
    draw_text_centered(frame, 505, 1, COL_DIM, "M4 BUILD - ESC QUITS");
}

pub fn draw_main_menu(frame: &mut Frame, cursor: usize, ui_frame: u32) {
    draw_text_centered(frame, 80, 4, COL_CURSOR, "FLOPPY SPIN");
    let x = 400;
    let y0 = 240;
    let row_h = 30;
    for (i, item) in MAIN_MENU_ITEMS.iter().enumerate() {
        let y = y0 + (i as i32) * row_h;
        let selected = i == cursor;
        let color = if selected { COL_ICE } else { COL_DIM };
        if selected {
            // Glowing chevron cursor with a subtle 2-frame bob.
            let bob = ((ui_frame / 16) % 2) as i32;
            text::draw_text(frame, x - 24 + bob, y, 2, COL_CURSOR, ">");
        }
        text::draw_text(frame, x, y, 2, color, item);
    }
    draw_text_centered(
        frame,
        500,
        1,
        COL_DIM,
        "ARROWS MOVE - SPACE SELECT - Z/ESC QUIT",
    );
}

pub fn draw_garage_stub(frame: &mut Frame) {
    draw_text_centered(frame, 160, 3, COL_CURSOR, "GARAGE");
    draw_text_centered(
        frame,
        240,
        2,
        COL_TEXT,
        "PART SWAPPING ARRIVES IN A LATER BUILD",
    );
    draw_text_centered(frame, 500, 1, COL_DIM, "Z/ESC BACK");
}

pub fn draw_settings(frame: &mut Frame, settings: &GameSettings, cursor: usize) {
    draw_text_centered(frame, 80, 3, COL_CURSOR, "SETTINGS");

    let labels = [
        "MUSIC VOLUME",
        "SFX VOLUME",
        "SCREEN SHAKE",
        "DIFFICULTY",
        "WINDOW SCALE",
        "COLORBLIND",
    ];
    let values: [String; 6] = [
        format!("{:>2}/10", settings.music_vol),
        format!("{:>2}/10", settings.sfx_vol),
        settings.shake.label().to_string(),
        floppy_core::flow::difficulty_label(settings.difficulty).to_string(),
        settings.window_scale.label().to_string(),
        if settings.colorblind { "ON" } else { "OFF" }.to_string(),
    ];

    let label_x = 300;
    let value_right = 660;
    let y0 = 180;
    let row_h = 34;
    for i in 0..labels.len() {
        let y = y0 + (i as i32) * row_h;
        let selected = i == cursor;
        let color = if selected { COL_ICE } else { COL_DIM };
        if selected {
            text::draw_text(frame, label_x - 24, y, 2, COL_CURSOR, ">");
        }
        text::draw_text(frame, label_x, y, 2, color, labels[i]);
        let (vw, _) = text::measure(&values[i], 2);
        text::draw_text(frame, value_right - vw as i32, y, 2, color, &values[i]);
    }
    draw_text_centered(frame, 500, 1, COL_DIM, "ARROWS ADJUST - Z/ESC BACK");
}

pub fn draw_top_select(frame: &mut Frame, cursor: usize, ui_frame: u32) {
    draw_text_centered(frame, 40, 3, COL_CURSOR, "SELECT YOUR TOP");

    // Left column: the 7 preset names, hovered one in its accent.
    let list_x = 90;
    let y0 = 120;
    let row_h = 40;
    for (i, p) in PRESETS.iter().enumerate() {
        let y = y0 + (i as i32) * row_h;
        let selected = i == cursor;
        let color = if selected { p.accent } else { COL_DIM };
        if selected {
            let bob = ((ui_frame / 16) % 2) as i32;
            text::draw_text(frame, list_x - 24 + bob, y, 2, p.accent, ">");
        }
        text::draw_text(frame, list_x, y, 2, color, p.name);
    }

    // Right panel: hovered preset's stats as labeled bars.
    let p = &PRESETS[cursor.min(PRESETS.len() - 1)];
    let stats: [(&str, u8); 6] = [
        ("ATK", p.stats.atk),
        ("DEF", p.stats.def),
        ("STA", p.stats.sta),
        ("WGT", p.stats.wgt),
        ("SPD", p.stats.spd),
        ("MTR", p.stats.mtr),
    ];
    let panel_x = 420;
    let bar_x = panel_x + 60;
    let bar_max = 300;
    for (i, (label, v)) in stats.iter().enumerate() {
        let y = 130 + (i as i32) * 30;
        text::draw_text(frame, panel_x, y, 2, COL_TEXT, label);
        let w = (*v as i32 * bar_max) / 100;
        fill_rect(frame, bar_x, y + 2, bar_max, 10, COL_TRACK);
        fill_rect(frame, bar_x, y + 2, w, 10, p.accent);
        let num = format!("{v:>3}");
        text::draw_text(frame, bar_x + bar_max + 10, y, 2, COL_TEXT, &num);
    }
    let dir_label = if p.spin_dir > 0 {
        "SPIN: CW"
    } else {
        "SPIN: CCW"
    };
    text::draw_text(frame, panel_x, 130 + 6 * 30, 2, COL_TEXT, dir_label);

    // Flavor line (roster.rs, shown on TopSelect per game_design.md §3).
    draw_text_centered(frame, 460, 2, p.accent, p.flavor);
    draw_text_centered(
        frame,
        505,
        1,
        COL_DIM,
        "ARROWS MOVE - SPACE PICK - Z/ESC BACK",
    );
}

// ---------------------------------------------------------------------------
// Launch minigame overlay (game_design.md §4).
// ---------------------------------------------------------------------------

/// Bottom-strip overlay for the three launch stages. `opponent_spin_dir` is
/// the AI's already-locked direction for the GRIND/CLASH tooltip.
pub fn draw_launch_ui(frame: &mut Frame, mg: &MinigameState, opponent_spin_dir: i8, ui_frame: u32) {
    let w = frame.w as i32;
    let panel_y = 430;
    fill_rect(frame, 180, panel_y, w - 360, 100, COL_SHADOW);
    rect_outline(frame, 180, panel_y, w - 360, 100, COL_TRACK);

    match mg.stage {
        Stage::Aim => {
            text::draw_text(frame, 200, panel_y + 10, 2, COL_CURSOR, "LAUNCH: AIM");
            let deg = (mg.heading * (180.0 / std::f32::consts::PI)) as i32;
            let line = format!("HEADING {deg:>3}   DEPTH");
            text::draw_text(frame, 200, panel_y + 40, 2, COL_TEXT, &line);
            // Depth gauge: 0.4..1.0 mapped onto a 200px bar.
            let (lw, _) = text::measure(&line, 2);
            let gx = 200 + lw as i32 + 12;
            let gw = 200;
            let t = ((mg.depth - 0.4) / 0.6).clamp(0.0, 1.0);
            fill_rect(frame, gx, panel_y + 42, gw, 10, COL_TRACK);
            fill_rect(
                frame,
                gx,
                panel_y + 42,
                (t * gw as f32) as i32,
                10,
                COL_CURSOR,
            );
            text::draw_text(
                frame,
                200,
                panel_y + 74,
                1,
                COL_DIM,
                "ARROWS: ROTATE + DEPTH   SPACE: LOCK",
            );
        }
        Stage::SpinDir => {
            text::draw_text(
                frame,
                200,
                panel_y + 10,
                2,
                COL_CURSOR,
                "LAUNCH: SPIN DIRECTION",
            );
            let dir_label = if mg.spin_dir > 0 {
                "CW  >>>"
            } else {
                "CCW <<<"
            };
            let read = if mg.spin_dir == opponent_spin_dir {
                "CLASH (SAME SPIN: BIG KNOCKBACK)"
            } else {
                "GRIND (OPPOSITE SPIN: SPIN STEAL)"
            };
            text::draw_text(frame, 200, panel_y + 40, 2, COL_ICE, dir_label);
            text::draw_text(frame, 380, panel_y + 40, 2, COL_TEXT, read);
            text::draw_text(
                frame,
                200,
                panel_y + 74,
                1,
                COL_DIM,
                "SHIFT: FLIP   SPACE: LOCK",
            );
        }
        Stage::Power | Stage::Locked => {
            text::draw_text(frame, 200, panel_y + 10, 2, COL_CURSOR, "LAUNCH: POWER");
            // Power bar 0..100 across 400 px with the game_design.md §4
            // bands marked: good 72-94, PERFECT 83-89, overcharge > 94.
            let bx = 280;
            let bw = 400;
            let by = panel_y + 40;
            let bh = 18;
            let px_of = |pct: f32| bx + ((pct / 100.0) * bw as f32) as i32;
            fill_rect(frame, bx, by, bw, bh, COL_TRACK);
            fill_rect(
                frame,
                px_of(GOOD_LO),
                by,
                px_of(GOOD_HI) - px_of(GOOD_LO),
                bh,
                0x0020_6048,
            );
            let p_lo = PERFECT_CENTER - PERFECT_HALF_WIDTH;
            let p_hi = PERFECT_CENTER + PERFECT_HALF_WIDTH;
            fill_rect(
                frame,
                px_of(p_lo),
                by,
                px_of(p_hi) - px_of(p_lo),
                bh,
                0x0060_D0A0,
            );
            fill_rect(
                frame,
                px_of(minigame::OVERCHARGE_THRESHOLD),
                by,
                px_of(100.0) - px_of(minigame::OVERCHARGE_THRESHOLD),
                bh,
                0x0070_2030,
            );
            rect_outline(frame, bx - 1, by - 1, bw + 2, bh + 2, COL_DIM);
            // Sweep marker (blinks white/cyan so it reads while moving fast).
            if let Some(pct) = mg.power_marker_pct() {
                let mx = px_of(pct);
                let mc = if (ui_frame / 4).is_multiple_of(2) {
                    COL_ICE
                } else {
                    COL_CURSOR
                };
                fill_rect(frame, mx - 1, by - 4, 3, bh + 8, mc);
            }
            text::draw_text(frame, 200, panel_y + 74, 1, COL_DIM, "SPACE: LOCK POWER");
        }
    }
}

// ---------------------------------------------------------------------------
// Round result / match over.
// ---------------------------------------------------------------------------

/// Inter-round tally screen content (drawn over the frozen fight frame or a
/// cleared background — caller's choice).
pub fn draw_round_result(
    frame: &mut Frame,
    round: u32,
    score: [u8; 2],
    accents: [u32; 2],
    result_line: &str,
    ui_frame: u32,
) {
    draw_text_centered(frame, 120, 3, COL_CURSOR, &format!("ROUND {}", round + 1));
    draw_text_centered(frame, 200, 3, COL_ICE, result_line);
    draw_text_centered(
        frame,
        260,
        3,
        COL_TEXT,
        &format!("P1 {}  -  {} CPU", score[0], score[1]),
    );
    draw_score_pips(frame, score, accents);
    if (ui_frame / 24).is_multiple_of(2) {
        draw_text_centered(frame, 420, 2, COL_ICE, "PRESS ANY KEY");
    }
}

/// Match-over screen: winner banner + final score (gold fountain is M6 VFX).
pub fn draw_match_over(frame: &mut Frame, p1_won: bool, score: [u8; 2], ui_frame: u32) {
    let banner = if p1_won { "P1 WINS!" } else { "CPU WINS!" };
    draw_banner(frame, banner, 6, COL_GOLD);
    draw_text_centered(
        frame,
        330,
        3,
        COL_TEXT,
        &format!("FINAL {} - {}", score[0], score[1]),
    );
    if (ui_frame / 24).is_multiple_of(2) {
        draw_text_centered(frame, 420, 2, COL_ICE, "PRESS ANY KEY FOR MENU");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use floppy_core::hash::hash_u32s;

    const W: usize = 960;
    const H: usize = 540;

    fn frame() -> Frame {
        let mut f = Frame::new(W, H);
        f.clear(COL_BG);
        f
    }

    fn painted(f: &Frame) -> usize {
        f.px.iter().filter(|&&p| p != COL_BG).count()
    }

    #[test]
    fn banner_paints_centered_pixels() {
        let mut f = frame();
        draw_banner(&mut f, "GO!", 6, COL_ICE);
        assert!(painted(&f) > 100);
        // Nothing in the far left/right eighth of the frame for a short
        // centered banner.
        for y in 0..H {
            for x in 0..W / 8 {
                assert_eq!(f.px[y * W + x], COL_BG, "left margin painted at {x},{y}");
            }
        }
    }

    #[test]
    fn stamina_arc_fill_fraction_scales_with_frac() {
        let mut f_full = frame();
        draw_stamina_arc(&mut f_full, 100, 100, 1.0, COL_ICE);
        let full = f_full.px.iter().filter(|&&p| p == COL_ICE).count();

        let mut f_half = frame();
        draw_stamina_arc(&mut f_half, 100, 100, 0.5, COL_ICE);
        let half = f_half.px.iter().filter(|&&p| p == COL_ICE).count();

        let mut f_zero = frame();
        draw_stamina_arc(&mut f_zero, 100, 100, 0.0, COL_ICE);
        let zero = f_zero.px.iter().filter(|&&p| p == COL_ICE).count();

        assert!(full > 0);
        assert_eq!(zero, 0, "empty arc must paint only the track color");
        let ratio = half as f32 / full as f32;
        assert!(
            (0.4..=0.6).contains(&ratio),
            "half arc should be ~half the full arc: {half}/{full}"
        );
    }

    #[test]
    fn every_screen_drawer_is_deterministic_and_paints_something() {
        // Same args -> bit-identical frame (pure f(state)); and each screen
        // actually draws visible content.
        let settings = GameSettings::default();
        let mg = MinigameState::new(1);

        type DrawFn = Box<dyn Fn(&mut Frame)>;
        let draws: Vec<(&str, DrawFn)> = vec![
            ("boot", Box::new(|f| draw_boot(f, 7))),
            ("title", Box::new(|f| draw_title(f, 10))),
            ("menu", Box::new(|f| draw_main_menu(f, 2, 30))),
            ("garage", Box::new(draw_garage_stub)),
            (
                "settings",
                Box::new(move |f| draw_settings(f, &settings, 3)),
            ),
            ("select", Box::new(|f| draw_top_select(f, 4, 5))),
            ("launch", Box::new(move |f| draw_launch_ui(f, &mg, -1, 12))),
            (
                "result",
                Box::new(|f| draw_round_result(f, 2, [3, 1], [COL_WARN, COL_GOLD], "TOPPLE!", 3)),
            ),
            (
                "matchover",
                Box::new(|f| draw_match_over(f, true, [4, 2], 0)),
            ),
        ];

        for (name, draw) in &draws {
            let mut a = frame();
            draw(&mut a);
            let mut b = frame();
            draw(&mut b);
            assert!(painted(&a) > 50, "{name} painted almost nothing");
            assert_eq!(
                hash_u32s(&a.px),
                hash_u32s(&b.px),
                "{name} draw is not deterministic"
            );
        }
    }

    #[test]
    fn score_pips_fill_matches_score_and_crash_out_style_jumps() {
        let mut f0 = frame();
        draw_score_pips(&mut f0, [0, 0], [COL_WARN, COL_GOLD]);
        let empty_warn = f0.px.iter().filter(|&&p| p == COL_WARN).count();

        let mut f3 = frame();
        draw_score_pips(&mut f3, [3, 0], [COL_WARN, COL_GOLD]);
        let filled_warn = f3.px.iter().filter(|&&p| p == COL_WARN).count();
        assert!(
            filled_warn > empty_warn * 2,
            "3 filled pips must paint far more accent pixels than 4 hollow ones"
        );
    }

    #[test]
    fn player_panel_low_spin_flashes_white_on_the_documented_cadence() {
        let panel = PlayerPanel {
            name: "CLEAVER",
            accent: COL_WARN,
            spin: 500.0, // 5% of max -> low
            spin_max: 10_000.0,
            meter: 40.0,
        };
        let mut on = frame();
        draw_player_panel(&mut on, &panel, false, 0); // (0/6)%2 == 0 -> white
        let mut off = frame();
        draw_player_panel(&mut off, &panel, false, 6); // (6/6)%2 == 1 -> accent
        assert_ne!(
            hash_u32s(&on.px),
            hash_u32s(&off.px),
            "low-spin flash must alternate with ui_frame"
        );
    }
}
