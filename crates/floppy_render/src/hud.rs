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
use crate::vfx;
use floppy_core::fixmath;
use floppy_core::flow::{
    GameSettings, GARAGE_SLOTS, MAIN_MENU_ITEMS, MY_BEY_FLAVOR, MY_BEY_INDEX, MY_BEY_NAME,
};
use floppy_core::garage::{self, FRAMES, PART_SLOTS};
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
/// little above the frame middle so it doesn't cover the tops. `scale` is
/// the INTEGER base size (matches every existing call site); see
/// [`draw_banner_scaled`] for the M7 overshoot-settle animated version.
pub fn draw_banner(frame: &mut Frame, banner: &str, scale: usize, color: u32) {
    draw_banner_scaled(frame, banner, scale as f32, color);
}

/// M7 round choreography (game_design.md §7): same centered-banner layout as
/// [`draw_banner`], but `scale` is a continuous `f32` so a caller-driven
/// overshoot-settle spring (see `vfx::OvershootSpring`) can pop the banner in
/// at e.g. 1.5x and animate it down to 1.0x over several frames. The
/// centering math re-measures at the CURRENT scale every call (so the text
/// stays centered while it's still shrinking, not just once at its final
/// size).
pub fn draw_banner_scaled(frame: &mut Frame, banner: &str, scale: f32, color: u32) {
    let (w, h) = text::measure_scaled(banner, scale);
    let y = (frame.h as f32 - h) / 2.0 - 40.0;
    let x = (frame.w as f32 - w) / 2.0;
    let off = (scale * 0.5).max(1.0);
    text::draw_text_scaled(frame, x + off, y + off, scale, COL_SHADOW, banner);
    text::draw_text_scaled(frame, x, y, scale, color, banner);
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
/// the dark track color. `stripe` (game_design.md §7 colorblind mode: "stripe
/// pattern on the P2 stamina arc — hue never the sole signal") overlays dark
/// bands every few degrees across the FILLED portion so it reads distinctly
/// even if `color` is indistinguishable from P1's.
pub fn draw_stamina_arc(frame: &mut Frame, cx: i32, cy: i32, frac: f32, color: u32, stripe: bool) {
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
            let mut c = if t <= frac { color } else { COL_TRACK };
            if stripe && t <= frac {
                // ~10 stripe bands across the full 270-degree sweep: every
                // other band darkened toward the track color.
                let band = (t * 10.0) as i32;
                if band % 2 == 1 {
                    c = COL_TRACK;
                }
            }
            frame.set(cx + dx, cy + dy, c);
        }
    }
}

/// Fixed corner panel (game_design.md §7 "Battle HUD"): stamina arc in the
/// top's accent (flashing white at <= 20% spin, timed off `ui_frame`), RPM
/// numeral, and the 0..100 meter bar with an Armed glow (brighter fill +
/// pulsing border once the meter is full).
pub fn draw_player_panel(
    frame: &mut Frame,
    panel: &PlayerPanel,
    right_side: bool,
    ui_frame: u32,
    colorblind: bool,
) {
    let w = frame.w as i32;
    let frac = (panel.spin / panel.spin_max).clamp(0.0, 1.0);
    let accent = vfx::colorblind_remap(panel.accent, colorblind);

    // Low-spin flash: white on a fast blink (render-only timing from the
    // caller-supplied frame counter, game_design.md §7).
    let low = frac <= 0.2;
    let arc_color = if low && (ui_frame / 6).is_multiple_of(2) {
        COL_ICE
    } else {
        accent
    };
    // Colorblind mode (game_design.md §7): P2's (right-side) arc gets a
    // stripe pattern so hue is never the sole signal.
    let stripe = colorblind && right_side;

    let (arc_cx, name_x, text_x) = if right_side {
        let (name_w, _) = text::measure(panel.name, 2);
        (w - 40, w - 14 - name_w as i32, w - 76 - 100)
    } else {
        (40, 14, 76)
    };

    text::draw_text(frame, name_x, 10, 2, accent, panel.name);
    draw_stamina_arc(frame, arc_cx, 74, frac, arc_color, stripe);

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
    let fill_color = if armed { COL_ICE } else { accent };
    fill_rect(frame, bar_x, bar_y, fill_w, bar_h, fill_color);
    let border = if armed && (ui_frame / 8).is_multiple_of(2) {
        COL_ICE
    } else if armed {
        accent
    } else {
        COL_DIM
    };
    rect_outline(frame, bar_x - 1, bar_y - 1, bar_w + 2, bar_h + 2, border);
}

/// Small filled circle (colorblind P1 shape tag — game_design.md §7: "P1
/// (circle)"), stamped inside a filled pip in a contrasting dark color so it
/// reads regardless of hue.
fn draw_mini_circle(frame: &mut Frame, cx: i32, cy: i32, r: i32, color: u32) {
    for dy in -r..=r {
        for dx in -r..=r {
            if dx * dx + dy * dy <= r * r {
                frame.set(cx + dx, cy + dy, color);
            }
        }
    }
}

/// Small filled upward triangle (colorblind P2 shape tag — game_design.md
/// §7: "P2 (triangle)").
fn draw_mini_triangle(frame: &mut Frame, cx: i32, cy: i32, r: i32, color: u32) {
    for dy in -r..=r {
        let half_width = (dy + r) / 2;
        for dx in -half_width..=half_width {
            frame.set(cx + dx, cy + dy, color);
        }
    }
}

/// Score pips (game_design.md §7): 4 hollow neon diamonds per side of
/// center-top, filled left-to-right (P1) / right-to-left toward center (P2)
/// as points accrue; a 3-point Crash-Out simply fills 3 at once. Colorblind
/// mode (game_design.md §7) stamps a small shape tag inside every FILLED pip
/// — circle for P1, triangle for P2 — so score is never read by hue alone.
pub fn draw_score_pips(frame: &mut Frame, score: [u8; 2], accents: [u32; 2], colorblind: bool) {
    let cx = frame.w as i32 / 2;
    let y = 24;
    let r = 8;
    let gap = 26;
    let accents = [
        vfx::colorblind_remap(accents[0], colorblind),
        vfx::colorblind_remap(accents[1], colorblind),
    ];
    for k in 0..4i32 {
        // P1 pips grow leftward from center; nearest-to-center = first point.
        let filled_p1 = (k as u8) < score[0].min(4);
        let x1 = cx - 24 - k * gap;
        draw_diamond(frame, x1, y, r, accents[0], filled_p1);
        if colorblind && filled_p1 {
            draw_mini_circle(frame, x1, y, r / 3, COL_SHADOW);
        }
        let filled_p2 = (k as u8) < score[1].min(4);
        let x2 = cx + 24 + k * gap;
        draw_diamond(frame, x2, y, r, accents[1], filled_p2);
        if colorblind && filled_p2 {
            draw_mini_triangle(frame, x2, y, r / 3, COL_SHADOW);
        }
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
    colorblind: bool,
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
    draw_player_panel(frame, &p1, false, ui_frame, colorblind);
    draw_player_panel(frame, &p2, true, ui_frame, colorblind);
    draw_score_pips(
        frame,
        score,
        [visuals[0].accent, visuals[1].accent],
        colorblind,
    );
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

/// Ring outline (game_design.md §7 title screen: "neon-ring backdrop"): a
/// bounding-box distance-test annulus, same technique as `draw_diamond`'s
/// Manhattan-distance test but circular (`fixmath`-free — plain squared
/// distance, no `sqrt` needed since both bounds are compared squared).
fn draw_ring_outline(frame: &mut Frame, cx: i32, cy: i32, r: i32, thickness: i32, color: u32) {
    let r_in = (r - thickness).max(0);
    for dy in -r..=r {
        for dx in -r..=r {
            let d2 = dx * dx + dy * dy;
            if d2 <= r * r && d2 >= r_in * r_in {
                frame.set(cx + dx, cy + dy, color);
            }
        }
    }
}

fn dim_color(c: u32, k: f32) -> u32 {
    let k = k.clamp(0.0, 1.0);
    let r = (((c >> 16) & 0xFF) as f32 * k) as u32;
    let g = (((c >> 8) & 0xFF) as f32 * k) as u32;
    let b = ((c & 0xFF) as f32 * k) as u32;
    (r << 16) | (g << 8) | b
}

/// Idle spinning top with a fading trail (game_design.md §7 title screen).
/// The title screen has no 3D camera (it's a flat 2D HUD screen — the
/// lathed-mesh tops only ever appear via `BattleScene`'s battle camera), so
/// this is a deliberate 2D approximation: a small diamond orbiting `(cx,
/// cy)` on a squashed ellipse (reads as a top's rim seen from slightly
/// above), with several dimmer ghost copies at earlier angles for the
/// trail. Fully deterministic from `ui_frame` (`fixmath::sin`/`cos`, no
/// wall-clock, no RNG).
fn draw_idle_top(frame: &mut Frame, cx: i32, cy: i32, ui_frame: u32) {
    const ORBIT_R: f32 = 60.0;
    const SPEED: f32 = 0.06;
    const TRAIL: i32 = 6;
    for i in (0..TRAIL).rev() {
        let t = (ui_frame as i32 - i * 2).max(0) as f32;
        let angle = t * SPEED;
        let ox = fixmath::cos(angle) * ORBIT_R;
        let oy = fixmath::sin(angle) * ORBIT_R * 0.4;
        let fade = 1.0 - (i as f32 / TRAIL as f32) * 0.85;
        let sz = if i == 0 { 5 } else { 3 };
        let color = dim_color(COL_ICE, fade);
        draw_diamond(frame, cx + ox as i32, cy + oy as i32, sz, color, true);
    }
}

pub fn draw_title(frame: &mut Frame, ui_frame: u32) {
    // Neon ring backdrop (game_design.md §7) + idle spinning top with a
    // trail, both centered above the title text.
    let ring_cx = frame.w as i32 / 2;
    let ring_cy = 210;
    draw_ring_outline(frame, ring_cx, ring_cy, 150, 2, dim_color(COL_CURSOR, 0.5));
    draw_ring_outline(frame, ring_cx, ring_cy, 110, 2, dim_color(0x00FF_2D7D, 0.6));
    draw_ring_outline(frame, ring_cx, ring_cy, 75, 2, dim_color(COL_CURSOR, 0.8));
    draw_idle_top(frame, ring_cx, ring_cy, ui_frame);

    // Chunky title: cyan fill with a magenta drop-offset that jitters
    // +-0.5px over time (game_design.md §7; see `vfx::title_shadow_jitter_px`
    // for why the continuous +-0.5px signal is quantized to a whole extra
    // pixel here — the bitmap font has no sub-pixel rendering).
    let title = "FLOPPY SPIN";
    let scale = 7;
    let (tw, th) = text::measure(title, scale);
    let x = (frame.w as i32 - tw as i32) / 2;
    let y = 140;
    let jitter = vfx::title_shadow_jitter_px(ui_frame);
    text::draw_text(
        frame,
        x + 4 + jitter,
        y + 4 + jitter,
        scale,
        0x00B0_1878,
        title,
    );
    text::draw_text(frame, x, y, scale, COL_CURSOR, title);
    // Accent underline.
    fill_rect(frame, x, y + th as i32 + 10, tw as i32, 4, COL_CURSOR);

    // PRESS START blink (~1.2 s cycle at 60 flow-fps: 36 on / 36 off).
    if (ui_frame / 36).is_multiple_of(2) {
        draw_text_centered(frame, 380, 2, COL_ICE, "PRESS ANY KEY");
    }
    draw_text_centered(frame, 505, 1, COL_DIM, "M7 BUILD - ESC QUITS");
}

/// `cursor` picks which item is highlighted (text color snaps instantly);
/// `cursor_anim` is the caller-owned [`vfx::Spring`]'s CURRENT eased
/// fractional row index (game_design.md §7: "glowing chevron cursor with
/// ~120 ms spring settle") — the chevron glyph itself is drawn at this
/// continuously-eased position instead of snapping row-to-row.
pub fn draw_main_menu(frame: &mut Frame, cursor: usize, cursor_anim: f32, ui_frame: u32) {
    draw_text_centered(frame, 80, 4, COL_CURSOR, "FLOPPY SPIN");
    let x = 400;
    let y0 = 240;
    let row_h = 30;
    for (i, item) in MAIN_MENU_ITEMS.iter().enumerate() {
        let y = y0 + (i as i32) * row_h;
        let selected = i == cursor;
        let color = if selected { COL_ICE } else { COL_DIM };
        text::draw_text(frame, x, y, 2, color, item);
    }
    // Glowing chevron cursor with a subtle 2-frame bob, at the spring-eased
    // y position.
    let bob = ((ui_frame / 16) % 2) as i32;
    let cursor_y = y0 + (cursor_anim * row_h as f32) as i32;
    text::draw_text(frame, x - 24 + bob, cursor_y, 2, COL_CURSOR, ">");
    draw_text_centered(
        frame,
        500,
        1,
        COL_DIM,
        "ARROWS MOVE - SPACE SELECT - Z/ESC QUIT",
    );
}

/// Slot names in `parts` index order (task spec Task 5: "slot list (5)").
const GARAGE_SLOT_NAMES: [&str; GARAGE_SLOTS] = ["FRAME", "BLADE", "DISK", "RIDGE", "TIP"];

/// The name of the currently-selected part in garage slot `slot` (0 = Frame,
/// 1..=4 = the stat-delta slots), reading straight from `garage::FRAMES`/
/// `PART_SLOTS` so the display never drifts from what `resolve()` actually
/// used. Indices are taken mod each table's own length, mirroring
/// `garage::resolve`'s own out-of-range handling (never panics).
fn garage_part_name(slot: usize, idx: u8) -> &'static str {
    if slot == 0 {
        FRAMES[idx as usize % FRAMES.len()].name
    } else {
        let table = &PART_SLOTS[slot - 1];
        table[idx as usize % table.len()].name
    }
}

/// Bey Garage screen (M8 Task 5, SPEC §7 / game_design.md §3): the 5-slot
/// part list with the selected slot highlighted in accent, the LIVE
/// resolved 6-stat readout (recomputed from `parts` every call — never
/// cached), the build's total flagged if it strays outside the 300±6
/// budget band, and MY BEY's name/flavor.
///
/// ## Preview fidelity (documented decision)
///
/// The task brief allows either a full 3D rotating-silhouette preview or a
/// solid stat panel ("a full 3D preview needs a camera/scene; a rotating
/// silhouette is nice-to-have, a solid stat panel is the requirement").
/// This picks the **stat panel** or a colored disc standing in for the
/// silhouette: a real 3D lathe preview would need `floppy_render::battle`'s
/// `BattleScene`/camera plumbed into a screen that isn't a `Match` phase at
/// all (SPEC §7's screen list keeps Garage separate from `Match`), which is
/// a materially bigger change than this milestone's render-only garage
/// task. The accent-colored disc + spin-direction arrow chevron gives an
/// at-a-glance shape/color read of the current Frame without that cost.
pub fn draw_garage(
    frame: &mut Frame,
    parts: [u8; 5],
    garage_slot: usize,
    slot_anim: f32,
    colorblind: bool,
    ui_frame: u32,
) {
    let build = garage::resolve(parts);
    let accent = vfx::colorblind_remap(build.accent, colorblind);

    draw_text_centered(frame, 30, 3, COL_CURSOR, "BEY GARAGE");
    draw_text_centered(frame, 66, 2, accent, MY_BEY_NAME);

    // Left column: the 5 slots, each showing its currently-equipped part
    // name; the selected slot in accent, others dim (mirrors draw_top_select's
    // list style for a consistent Garage/TopSelect feel).
    let list_x = 60;
    let y0 = 130;
    let row_h = 34;
    for (i, slot_name) in GARAGE_SLOT_NAMES.iter().enumerate() {
        let y = y0 + (i as i32) * row_h;
        let selected = i == garage_slot;
        let color = if selected { accent } else { COL_DIM };
        let part_name = garage_part_name(i, parts[i]);
        text::draw_text(frame, list_x, y, 2, color, slot_name);
        text::draw_text(frame, list_x + 130, y, 2, color, part_name);
    }
    let bob = ((ui_frame / 16) % 2) as i32;
    let cursor_y = y0 + (slot_anim * row_h as f32) as i32;
    text::draw_text(frame, list_x - 24 + bob, cursor_y, 2, accent, ">");

    // A plain accent-colored disc stands in for the Frame silhouette
    // (module docs above): cheap, deterministic, and immediately reflects a
    // Frame swap's accent + spin-direction chevron.
    let preview_cx = 470;
    let preview_cy = 220;
    draw_ring_outline(frame, preview_cx, preview_cy, 46, 4, accent);
    draw_ring_outline(frame, preview_cx, preview_cy, 30, 8, dim_color(accent, 0.6));
    let dir_glyph = if build.spin_dir > 0 { "CW" } else { "CCW" };
    draw_text_centered(frame, preview_cy + 60, 1, COL_TEXT, dir_glyph);

    // Right panel: the resolved 6 stat bars, LIVE (recomputed from `parts`
    // above, never a stale snapshot).
    let stats: [(&str, u8); 6] = [
        ("ATK", build.stats.atk),
        ("DEF", build.stats.def),
        ("STA", build.stats.sta),
        ("WGT", build.stats.wgt),
        ("SPD", build.stats.spd),
        ("MTR", build.stats.mtr),
    ];
    let panel_x = 600;
    let bar_x = panel_x + 60;
    let bar_max = 220;
    for (i, (label, v)) in stats.iter().enumerate() {
        let y = 130 + (i as i32) * 30;
        text::draw_text(frame, panel_x, y, 2, COL_TEXT, label);
        let w = (*v as i32 * bar_max) / 100;
        fill_rect(frame, bar_x, y + 2, bar_max, 10, COL_TRACK);
        fill_rect(frame, bar_x, y + 2, w, 10, accent);
        let num = format!("{v:>3}");
        text::draw_text(frame, bar_x + bar_max + 10, y, 2, COL_TEXT, &num);
    }

    // Build total, flagged outside the 294..=306 budget band (task spec:
    // "flag if over 306 / under 294" — game_design.md §3's 300+-6).
    let total: i32 = build.stats.atk as i32
        + build.stats.def as i32
        + build.stats.sta as i32
        + build.stats.wgt as i32
        + build.stats.spd as i32
        + build.stats.mtr as i32;
    let total_y = 130 + 6 * 30 + 10;
    let in_band = (294..=306).contains(&total);
    let total_color = if in_band { COL_TEXT } else { COL_WARN };
    let total_line = if in_band {
        format!("TOTAL {total}")
    } else {
        format!("TOTAL {total} (OUT OF BUDGET)")
    };
    text::draw_text(frame, panel_x, total_y, 2, total_color, &total_line);

    draw_text_centered(frame, 470, 2, accent, MY_BEY_FLAVOR);
    draw_text_centered(
        frame,
        505,
        1,
        COL_DIM,
        "UP/DOWN SLOT - LEFT/RIGHT PART - Z/ESC BACK",
    );
}

pub fn draw_settings(frame: &mut Frame, settings: &GameSettings, cursor: usize, cursor_anim: f32) {
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
        text::draw_text(frame, label_x, y, 2, color, labels[i]);
        let (vw, _) = text::measure(&values[i], 2);
        text::draw_text(frame, value_right - vw as i32, y, 2, color, &values[i]);
    }
    let cursor_y = y0 + (cursor_anim * row_h as f32) as i32;
    text::draw_text(frame, label_x - 24, cursor_y, 2, COL_CURSOR, ">");
    draw_text_centered(frame, 500, 1, COL_DIM, "ARROWS ADJUST - Z/ESC BACK");
}

/// SELECT YOUR TOP (SPEC §7). `garage_parts` is the player's live garage
/// build (M8): the 8th entry, MY BEY at cursor [`MY_BEY_INDEX`], resolves
/// its stats/accent/spin-dir from these parts every call (`garage::resolve`)
/// rather than a stale snapshot, so a build changed in the Garage screen
/// immediately shows correctly here too.
pub fn draw_top_select(
    frame: &mut Frame,
    cursor: usize,
    cursor_anim: f32,
    ui_frame: u32,
    garage_parts: [u8; 5],
) {
    draw_text_centered(frame, 40, 3, COL_CURSOR, "SELECT YOUR TOP");

    let my_bey_build = garage::resolve(garage_parts);
    // A `Preset`-shaped view of the hovered entry: a real roster preset for
    // 0..PRESETS.len(), or MY BEY (synthesized from the live garage build)
    // at MY_BEY_INDEX — mirrors `flow::FlowState::preset_view` so render and
    // sim never disagree about what a given TopSelect index means.
    let entry_name = |i: usize| -> &'static str {
        if i == MY_BEY_INDEX {
            MY_BEY_NAME
        } else {
            PRESETS[i].name
        }
    };
    let entry_accent = |i: usize| -> u32 {
        if i == MY_BEY_INDEX {
            my_bey_build.accent
        } else {
            PRESETS[i].accent
        }
    };

    // Left column: the 7 preset names plus MY BEY, hovered one in its accent.
    let list_x = 90;
    let y0 = 110;
    let row_h = 36;
    for i in 0..=MY_BEY_INDEX {
        let y = y0 + (i as i32) * row_h;
        let selected = i == cursor;
        let color = if selected { entry_accent(i) } else { COL_DIM };
        text::draw_text(frame, list_x, y, 2, color, entry_name(i));
    }
    let bob = ((ui_frame / 16) % 2) as i32;
    let cursor_y = y0 + (cursor_anim * row_h as f32) as i32;
    let cursor_accent = entry_accent(cursor.min(MY_BEY_INDEX));
    text::draw_text(frame, list_x - 24 + bob, cursor_y, 2, cursor_accent, ">");

    // Right panel: hovered entry's stats as labeled bars (real preset or the
    // live garage build).
    let hovered = cursor.min(MY_BEY_INDEX);
    let (stats_src, accent, spin_dir, flavor) = if hovered == MY_BEY_INDEX {
        (
            my_bey_build.stats,
            my_bey_build.accent,
            my_bey_build.spin_dir,
            MY_BEY_FLAVOR,
        )
    } else {
        let p = &PRESETS[hovered];
        (p.stats, p.accent, p.spin_dir, p.flavor)
    };
    let stats: [(&str, u8); 6] = [
        ("ATK", stats_src.atk),
        ("DEF", stats_src.def),
        ("STA", stats_src.sta),
        ("WGT", stats_src.wgt),
        ("SPD", stats_src.spd),
        ("MTR", stats_src.mtr),
    ];
    let panel_x = 420;
    let bar_x = panel_x + 60;
    let bar_max = 300;
    for (i, (label, v)) in stats.iter().enumerate() {
        let y = 130 + (i as i32) * 30;
        text::draw_text(frame, panel_x, y, 2, COL_TEXT, label);
        let w = (*v as i32 * bar_max) / 100;
        fill_rect(frame, bar_x, y + 2, bar_max, 10, COL_TRACK);
        fill_rect(frame, bar_x, y + 2, w, 10, accent);
        let num = format!("{v:>3}");
        text::draw_text(frame, bar_x + bar_max + 10, y, 2, COL_TEXT, &num);
    }
    let dir_label = if spin_dir > 0 {
        "SPIN: CW"
    } else {
        "SPIN: CCW"
    };
    text::draw_text(frame, panel_x, 130 + 6 * 30, 2, COL_TEXT, dir_label);

    // Flavor line (roster.rs / MY_BEY_FLAVOR, shown on TopSelect per
    // game_design.md §3).
    draw_text_centered(frame, 460, 2, accent, flavor);
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

/// Frames per pip during the RoundResult tally (game_design.md §7: "tallies
/// pips with a ting each 120 ms" -> 120 ms @ 60 Hz = 7.2 frames, rounded to
/// 7).
pub const TALLY_FRAMES_PER_PIP: u32 = 7;

/// How many of the round winner's newly-earned points have "landed" by
/// `frame` (frames since RoundResult was entered — i.e. its own `ui_frame`,
/// which resets to 0 on every screen transition per `flow::FlowState`). Pure
/// function of `(frame, last_points)`; `main.rs` calls this itself each
/// frame to detect the edge and fire `Sfx::ScoreTally` once per newly-landed
/// pip (module docs: "main.rs edge").
pub fn tally_pip_count(frame: u32, last_points: u8) -> u8 {
    ((frame / TALLY_FRAMES_PER_PIP) as u8).min(last_points)
}

/// Inter-round tally screen content (drawn over the frozen fight frame or a
/// cleared background — caller's choice). `last_winner`/`last_points`
/// (mirroring `flow::FlowState`'s own fields) animate the WINNER's pips
/// filling in one at a time (game_design.md §7) rather than snapping
/// straight to the final `score`.
#[allow(clippy::too_many_arguments)]
pub fn draw_round_result(
    frame: &mut Frame,
    round: u32,
    score: [u8; 2],
    accents: [u32; 2],
    result_line: &str,
    ui_frame: u32,
    last_winner: Option<u8>,
    last_points: u8,
    colorblind: bool,
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
    let mut display_score = score;
    if let Some(winner) = last_winner {
        let tallied = tally_pip_count(ui_frame, last_points);
        display_score[winner as usize] = score[winner as usize]
            .saturating_sub(last_points)
            .saturating_add(tallied);
    }
    draw_score_pips(frame, display_score, accents, colorblind);
    if (ui_frame / 24).is_multiple_of(2) {
        draw_text_centered(frame, 420, 2, COL_ICE, "PRESS ANY KEY");
    }
}

/// Match-over screen: winner banner (slam-in overshoot — `banner_scale` is
/// the caller-owned `vfx::OvershootSpring`'s current value, game_design.md
/// §7: "slam in at 1.5x overshoot") + final score. The gold fountain
/// particle burst is drawn separately by the caller (M7 particle pool, not a
/// HUD-text concern).
pub fn draw_match_over(
    frame: &mut Frame,
    p1_won: bool,
    score: [u8; 2],
    ui_frame: u32,
    banner_scale: f32,
) {
    let banner = if p1_won { "P1 WINS!" } else { "CPU WINS!" };
    draw_banner_scaled(frame, banner, banner_scale, COL_GOLD);
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
        draw_stamina_arc(&mut f_full, 100, 100, 1.0, COL_ICE, false);
        let full = f_full.px.iter().filter(|&&p| p == COL_ICE).count();

        let mut f_half = frame();
        draw_stamina_arc(&mut f_half, 100, 100, 0.5, COL_ICE, false);
        let half = f_half.px.iter().filter(|&&p| p == COL_ICE).count();

        let mut f_zero = frame();
        draw_stamina_arc(&mut f_zero, 100, 100, 0.0, COL_ICE, false);
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
    fn stamina_arc_stripe_darkens_part_of_the_filled_arc() {
        let mut f_plain = frame();
        draw_stamina_arc(&mut f_plain, 100, 100, 1.0, COL_ICE, false);
        let plain = f_plain.px.iter().filter(|&&p| p == COL_ICE).count();

        let mut f_striped = frame();
        draw_stamina_arc(&mut f_striped, 100, 100, 1.0, COL_ICE, true);
        let striped = f_striped.px.iter().filter(|&&p| p == COL_ICE).count();

        assert!(
            striped < plain,
            "striped arc should show less solid accent color: striped={striped} plain={plain}"
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
            ("menu", Box::new(|f| draw_main_menu(f, 2, 2.0, 30))),
            (
                "garage",
                Box::new(|f| draw_garage(f, [1, 2, 3, 0, 1], 2, 2.0, false, 9)),
            ),
            (
                "settings",
                Box::new(move |f| draw_settings(f, &settings, 3, 3.0)),
            ),
            (
                "select",
                Box::new(|f| draw_top_select(f, 4, 4.0, 5, [0, 0, 0, 0, 0])),
            ),
            ("launch", Box::new(move |f| draw_launch_ui(f, &mg, -1, 12))),
            (
                "result",
                Box::new(|f| {
                    draw_round_result(
                        f,
                        2,
                        [3, 1],
                        [COL_WARN, COL_GOLD],
                        "TOPPLE!",
                        3,
                        Some(0),
                        1,
                        false,
                    )
                }),
            ),
            (
                "matchover",
                Box::new(|f| draw_match_over(f, true, [4, 2], 0, 1.2)),
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
        draw_score_pips(&mut f0, [0, 0], [COL_WARN, COL_GOLD], false);
        let empty_warn = f0.px.iter().filter(|&&p| p == COL_WARN).count();

        let mut f3 = frame();
        draw_score_pips(&mut f3, [3, 0], [COL_WARN, COL_GOLD], false);
        let filled_warn = f3.px.iter().filter(|&&p| p == COL_WARN).count();
        assert!(
            filled_warn > empty_warn * 2,
            "3 filled pips must paint far more accent pixels than 4 hollow ones"
        );
    }

    #[test]
    fn score_pips_colorblind_mode_stamps_shape_tags_without_changing_score() {
        let mut plain = frame();
        draw_score_pips(&mut plain, [2, 3], [COL_WARN, COL_GOLD], false);
        let mut cb = frame();
        draw_score_pips(&mut cb, [2, 3], [COL_WARN, COL_GOLD], true);
        assert_ne!(
            hash_u32s(&plain.px),
            hash_u32s(&cb.px),
            "colorblind mode should visibly stamp shape tags"
        );
    }

    #[test]
    fn tally_pip_count_advances_one_pip_per_120ms_and_caps_at_last_points() {
        assert_eq!(tally_pip_count(0, 3), 0);
        assert_eq!(tally_pip_count(TALLY_FRAMES_PER_PIP, 3), 1);
        assert_eq!(tally_pip_count(TALLY_FRAMES_PER_PIP * 2, 3), 2);
        assert_eq!(tally_pip_count(TALLY_FRAMES_PER_PIP * 10, 3), 3);
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
        draw_player_panel(&mut on, &panel, false, 0, false); // (0/6)%2 == 0 -> white
        let mut off = frame();
        draw_player_panel(&mut off, &panel, false, 6, false); // (6/6)%2 == 1 -> accent
        assert_ne!(
            hash_u32s(&on.px),
            hash_u32s(&off.px),
            "low-spin flash must alternate with ui_frame"
        );
    }

    #[test]
    fn player_panel_colorblind_remaps_lime_accent_to_ice_blue() {
        let panel = PlayerPanel {
            name: "EVERSPIN",
            accent: 0x0039_FF14, // lime (Everspin's roster accent)
            spin: 8000.0,
            spin_max: 10_000.0,
            meter: 10.0,
        };
        let mut plain = frame();
        draw_player_panel(&mut plain, &panel, false, 0, false);
        let mut cb = frame();
        draw_player_panel(&mut cb, &panel, false, 0, true);
        assert!(
            plain.px.contains(&0x0039_FF14),
            "plain mode should show the raw lime accent"
        );
        assert!(
            !cb.px.contains(&0x0039_FF14),
            "colorblind mode should never show raw lime"
        );
    }

    #[test]
    fn garage_screen_reflects_live_part_swaps() {
        let mut before = frame();
        draw_garage(&mut before, [0, 0, 0, 0, 0], 1, 1.0, false, 0);
        let mut after = frame();
        draw_garage(&mut after, [0, 3, 0, 0, 0], 1, 1.0, false, 0);
        assert_ne!(
            hash_u32s(&before.px),
            hash_u32s(&after.px),
            "swapping Blade to index 3 must visibly change the garage screen"
        );
    }

    #[test]
    fn garage_screen_flags_a_build_outside_the_budget_band() {
        // Default build (all-zero parts) sits exactly at 300 (in-band);
        // stacking every slot's most extreme option pushes it out.
        let mut in_band = frame();
        draw_garage(&mut in_band, [0, 0, 0, 0, 0], 0, 0.0, false, 0);
        let mut out_of_band = frame();
        draw_garage(&mut out_of_band, [0, 3, 3, 3, 3], 0, 0.0, false, 0);
        assert_ne!(
            hash_u32s(&in_band.px),
            hash_u32s(&out_of_band.px),
            "an out-of-budget build must render differently (flagged) from an in-band one"
        );
    }

    #[test]
    fn garage_screen_is_deterministic_and_colorblind_safe() {
        let parts = [1, 2, 3, 0, 2];
        let mut a = frame();
        draw_garage(&mut a, parts, 3, 3.0, false, 12);
        let mut b = frame();
        draw_garage(&mut b, parts, 3, 3.0, false, 12);
        assert_eq!(
            hash_u32s(&a.px),
            hash_u32s(&b.px),
            "garage draw must be pure"
        );
        assert!(painted(&a) > 50, "garage screen painted almost nothing");

        // Colorblind mode must actually change rendering for an
        // Everspin-framed build (lime accent -> remapped).
        let lime_parts = [2, 0, 0, 0, 0]; // Frame idx2 = Everspin
        let mut plain = frame();
        draw_garage(&mut plain, lime_parts, 0, 0.0, false, 0);
        let mut cb = frame();
        draw_garage(&mut cb, lime_parts, 0, 0.0, true, 0);
        assert_ne!(
            hash_u32s(&plain.px),
            hash_u32s(&cb.px),
            "colorblind mode must remap the garage screen's accent"
        );
    }

    #[test]
    fn top_select_my_bey_entry_shows_the_live_garage_build() {
        let mut default_parts = frame();
        draw_top_select(
            &mut default_parts,
            MY_BEY_INDEX,
            MY_BEY_INDEX as f32,
            0,
            [0; 5],
        );
        let mut swapped_parts = frame();
        draw_top_select(
            &mut swapped_parts,
            MY_BEY_INDEX,
            MY_BEY_INDEX as f32,
            0,
            [3, 3, 3, 3, 3],
        );
        assert_ne!(
            hash_u32s(&default_parts.px),
            hash_u32s(&swapped_parts.px),
            "hovering MY BEY must reflect the CURRENT garage parts, not a fixed preset"
        );
    }
}
