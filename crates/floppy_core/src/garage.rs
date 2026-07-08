//! M8 Task 1: Bey Garage parts + resolve (SPEC §6.1 `TopKind::Custom` /
//! §9 "garage part indices"; game_design.md §3 "paired-delta parts keep the
//! triangle honest"). Pure data + pure functions — no rendering, no I/O.
//!
//! ## Slot design (documented decision)
//!
//! `[u8; 5]` part indices, one slot per array entry, SPEC §9-exact shape.
//! Slot 0 is the **Frame**: it doesn't add stat deltas at all — it picks a
//! roster [`Silhouette`] wholesale (so MY BEY renders as a real shape and has
//! a real special, reusing `roster`/`combat` rather than inventing a second
//! special system). Slots 1..=4 are **stat-delta parts**: each part carries a
//! genuine plus AND minus on two different stats (paired delta), so no part
//! is a free lunch and stacking the same lane repeatedly still costs
//! something elsewhere.
//!
//! Slot 1 **Blade**: `ATK +` / `DEF -`. Slot 2 **Disk**: `WGT +` / `SPD -`.
//! Slot 3 **Ridge**: `STA +` / `MTR -`. Slot 4 **Tip**: `SPD +` / `WGT -`
//! (Disk and Tip deliberately oppose each other on the same stat pair —
//! WGT/SPD — so a player leaning all-Disk or all-Tip feels the tradeoff
//! twice as hard, while mixing them cancels back toward neutral).
//!
//! [`BASE_STATS`] is 40 across every stat (sum 240) — deliberately below the
//! preset 300±6 budget (game_design.md §3) so the 4 stat-delta parts build
//! *up* to it. Every slot's index-0 part is a mild, safe "default" choice
//! netting `+15` (e.g. Blade 0 = `ATK+18/DEF-3`), so the all-zero build
//! (`[0, 0, 0, 0, 0]`) lands at exactly `240 + 4*15 = 300` — dead center of
//! the budget band. Indices 1..=3 per slot trade further out along that
//! slot's pair, up to a deliberately extreme index-3 (the "player's
//! tradeoff" the task brief calls out); combining several extreme picks can
//! push the total outside 300±6 on purpose.
//!
//! ## Frame choices (documented decision)
//!
//! 4 of the roster's 7 silhouettes, chosen to keep one representative from
//! each corner of the balance triangle (game_design.md §3 "Attack ▶ Stamina
//! ▶ Defense ▶ Attack") plus one off-triangle exotic, so MY BEY's frame pick
//! meaningfully changes both its special and its playstyle identity:
//! `Cleaver` (Attack corner), `Bulwark` (Defense corner), `Everspin`
//! (Stamina corner), `Riptide` (off-triangle Assault-Drift exotic).

use crate::physics::Stats;
use crate::roster::{Silhouette, PRESETS};

/// One stat-delta part: a name plus signed deltas on all six stats (module
/// docs: exactly two of the six are ever non-zero per part, one positive one
/// negative — the rest are `0`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Part {
    pub name: &'static str,
    pub d_atk: i8,
    pub d_def: i8,
    pub d_sta: i8,
    pub d_wgt: i8,
    pub d_spd: i8,
    pub d_mtr: i8,
}

const fn part(
    name: &'static str,
    d_atk: i8,
    d_def: i8,
    d_sta: i8,
    d_wgt: i8,
    d_spd: i8,
    d_mtr: i8,
) -> Part {
    Part {
        name,
        d_atk,
        d_def,
        d_sta,
        d_wgt,
        d_spd,
        d_mtr,
    }
}

/// One Frame choice: a name plus the roster [`Silhouette`] it borrows shape/
/// special/spin-direction/accent from (module docs above).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub name: &'static str,
    pub silhouette: Silhouette,
}

/// The 4 Frame options (module docs: one per triangle corner + one exotic).
pub const FRAMES: [Frame; 4] = [
    Frame {
        name: "Cleaver Frame",
        silhouette: Silhouette::Cleaver,
    },
    Frame {
        name: "Bulwark Frame",
        silhouette: Silhouette::Bulwark,
    },
    Frame {
        name: "Everspin Frame",
        silhouette: Silhouette::Everspin,
    },
    Frame {
        name: "Riptide Frame",
        silhouette: Silhouette::Riptide,
    },
];

/// Slot 1: **Blade** — `ATK +` / `DEF -` (module docs for the full table).
/// Index 0 = default (net `+15`), 1 = aggressive (net `+14` but bigger
/// swings both ways), 2 = "Guard" — a small, safe, low-swing pick (net `+5`),
/// 3 = "Razor" — the extreme tradeoff (net `+14` via much bigger swings).
pub const BLADE: [Part; 4] = [
    part("Blade Mk0", 18, -3, 0, 0, 0, 0),
    part("Blade Mk1", 28, -14, 0, 0, 0, 0),
    part("Blade Guard", 6, -1, 0, 0, 0, 0),
    part("Blade Razor", 34, -20, 0, 0, 0, 0),
];

/// Slot 2: **Disk** — `WGT +` / `SPD -`.
pub const DISK: [Part; 4] = [
    part("Disk Mk0", 0, 0, 0, 18, -3, 0),
    part("Disk Mk1", 0, 0, 0, 26, -13, 0),
    part("Disk Guard", 0, 0, 0, 6, -1, 0),
    part("Disk Heavy", 0, 0, 0, 30, -18, 0),
];

/// Slot 3: **Ridge** — `STA +` / `MTR -`.
pub const RIDGE: [Part; 4] = [
    part("Ridge Mk0", 0, 0, 18, 0, 0, -3),
    part("Ridge Mk1", 0, 0, 24, 0, 0, -11),
    part("Ridge Guard", 0, 0, 6, 0, 0, -1),
    part("Ridge Endless", 0, 0, 32, 0, 0, -19),
];

/// Slot 4: **Tip** — `SPD +` / `WGT -`.
pub const TIP: [Part; 4] = [
    part("Tip Mk0", 0, 0, 0, -3, 18, 0),
    part("Tip Mk1", 0, 0, 0, -11, 25, 0),
    part("Tip Guard", 0, 0, 0, -1, 6, 0),
    part("Tip Needle", 0, 0, 0, -17, 30, 0),
];

/// The 4 stat-delta slots, in `indices[1..=4]` order (module docs).
pub const PART_SLOTS: [[Part; 4]; 4] = [BLADE, DISK, RIDGE, TIP];

/// Garage base stats before any part deltas (module docs): 40 across the
/// board, sum 240.
pub const BASE_STATS: Stats = Stats {
    atk: 40,
    def: 40,
    sta: 40,
    wgt: 40,
    spd: 40,
    mtr: 40,
};

/// The fully-resolved custom top a garage build produces: plain [`Stats`]
/// plus the Frame's shape/special/spin-direction/accent identity — this is
/// exactly what `flow::spawn_fight_world` feeds into `LaunchParams` for MY
/// BEY, with NO special-casing anywhere in the sim (SPEC §12 gate).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CustomBuild {
    pub stats: Stats,
    pub silhouette: Silhouette,
    pub special_id: crate::combat::SpecialId,
    pub accent: u32,
    pub spin_dir: i8,
}

/// Clamp an `i32` accumulator into a `u8` stat, saturating at `0..=100`
/// (task spec: "each final stat to 0..=100").
fn clamp_stat(v: i32) -> u8 {
    v.clamp(0, 100) as u8
}

/// Resolve a 5-slot garage build into a [`CustomBuild`]. Total function:
/// every `[u8; 5]` combination (including indices `>= 4`, which the save
/// loader might hand back from a corrupted/foreign file) returns valid
/// clamped stats and never panics — out-of-range indices clamp to slot
/// index 0 (task spec: "out-of-range indices clamp to 0").
pub fn resolve(indices: [u8; 5]) -> CustomBuild {
    let frame_idx = (indices[0] as usize) % FRAMES.len();
    let frame = &FRAMES[frame_idx];

    let mut atk = BASE_STATS.atk as i32;
    let mut def = BASE_STATS.def as i32;
    let mut sta = BASE_STATS.sta as i32;
    let mut wgt = BASE_STATS.wgt as i32;
    let mut spd = BASE_STATS.spd as i32;
    let mut mtr = BASE_STATS.mtr as i32;

    for (slot, parts) in PART_SLOTS.iter().enumerate() {
        let idx = indices[slot + 1] as usize;
        // Out-of-range clamps to 0 rather than panicking (task spec) — a
        // wild index from a corrupted save must never index out of bounds.
        let idx = if idx < parts.len() { idx } else { 0 };
        let p = &parts[idx];
        atk += p.d_atk as i32;
        def += p.d_def as i32;
        sta += p.d_sta as i32;
        wgt += p.d_wgt as i32;
        spd += p.d_spd as i32;
        mtr += p.d_mtr as i32;
    }

    let stats = Stats {
        atk: clamp_stat(atk),
        def: clamp_stat(def),
        sta: clamp_stat(sta),
        wgt: clamp_stat(wgt),
        spd: clamp_stat(spd),
        mtr: clamp_stat(mtr),
    };

    // Frame's roster preset supplies accent/spin_dir (module docs); special
    // id derives from the same silhouette via the existing roster mapping
    // (combat::SpecialId::from_silhouette) so garage tops go through the
    // IDENTICAL special-identity path a preset uses — no new special table.
    let preset = PRESETS
        .iter()
        .find(|p| p.silhouette == frame.silhouette)
        .expect("every FRAMES silhouette must have a matching roster preset");

    CustomBuild {
        stats,
        silhouette: frame.silhouette,
        special_id: crate::combat::SpecialId::from_silhouette(frame.silhouette),
        accent: preset.accent,
        spin_dir: preset.spin_dir,
    }
}

/// Default garage build: every slot at index 0 (task spec: "the default
/// all-index-0 build is a sane ~balanced top").
pub const DEFAULT_PARTS: [u8; 5] = [0, 0, 0, 0, 0];

#[cfg(test)]
mod tests {
    use super::*;

    fn stat_sum(s: Stats) -> i32 {
        s.atk as i32 + s.def as i32 + s.sta as i32 + s.wgt as i32 + s.spd as i32 + s.mtr as i32
    }

    #[test]
    fn resolve_is_total_over_every_index_combo_and_never_panics() {
        for i0 in 0..4u8 {
            for i1 in 0..4u8 {
                for i2 in 0..4u8 {
                    for i3 in 0..4u8 {
                        for i4 in 0..4u8 {
                            let build = resolve([i0, i1, i2, i3, i4]);
                            let s = build.stats;
                            assert!(s.atk <= 100 && s.def <= 100 && s.sta <= 100);
                            assert!(s.wgt <= 100 && s.spd <= 100 && s.mtr <= 100);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn resolve_never_panics_on_out_of_range_indices() {
        // Wild bytes a corrupted save could hand back, including 255.
        for &v in &[4u8, 5, 6, 100, 200, 255] {
            let build = resolve([v, v, v, v, v]);
            let s = build.stats;
            assert!(s.atk <= 100 && s.def <= 100 && s.sta <= 100);
            assert!(s.wgt <= 100 && s.spd <= 100 && s.mtr <= 100);
        }
        // Every 4^5 = 1024 in-range combo already covered above; spot-check
        // a full sweep of out-of-range in slot 0 (Frame) too.
        for v in 0..=255u8 {
            let build = resolve([v, 0, 0, 0, 0]);
            assert!(build.stats.atk <= 100);
        }
    }

    #[test]
    fn default_build_total_is_within_the_documented_band() {
        let build = resolve(DEFAULT_PARTS);
        let sum = stat_sum(build.stats);
        // Module docs: 240 base + 4*15 = 300 exactly.
        assert_eq!(sum, 300, "default build should total exactly 300");
        assert!(
            (sum - 300).abs() <= 6,
            "default build must be within 300+-6"
        );
    }

    #[test]
    fn known_part_combo_yields_hand_computed_stats() {
        // Frame 1 (Bulwark), Blade idx1 "Blade Mk1" (ATK+28/DEF-14),
        // Disk idx2 "Disk Guard" (WGT+6/SPD-1), Ridge idx3 "Ridge Endless"
        // (STA+32/MTR-19), Tip idx0 "Tip Mk0" (WGT-3/SPD+18).
        let build = resolve([1, 1, 2, 3, 0]);
        let expected = Stats {
            atk: clamp_stat(40 + 28),
            def: clamp_stat(40 - 14),
            sta: clamp_stat(40 + 32),
            wgt: clamp_stat(40 + 6 - 3),
            spd: clamp_stat(40 - 1 + 18),
            mtr: clamp_stat(40 - 19),
        };
        assert_eq!(build.stats, expected);
        assert_eq!(build.silhouette, Silhouette::Bulwark);
    }

    #[test]
    fn clamping_actually_caps_at_0_and_100() {
        // Direct check of the clamping primitive at and past both boundaries
        // (every `resolve()` stat funnels through this).
        assert_eq!(clamp_stat(-1), 0);
        assert_eq!(clamp_stat(0), 0);
        assert_eq!(clamp_stat(100), 100);
        assert_eq!(clamp_stat(101), 100);
        assert_eq!(clamp_stat(-500), 0);
        assert_eq!(clamp_stat(500), 100);

        // And confirm `resolve()` itself never exceeds `0..=100` for the
        // most WGT-stacked real build the tables allow: Disk Heavy (+30wgt/
        // -18spd) plus the default Tip Mk0 (-3wgt/+18spd) — Disk and Tip
        // share the WGT/SPD pair (module docs), so both slots contribute.
        // indices = [frame, blade, disk, ridge, tip]; Disk is index 2.
        let high_wgt = resolve([0, 0, 3, 0, 0]);
        assert!(high_wgt.stats.wgt <= 100);
        assert_eq!(high_wgt.stats.wgt, clamp_stat(40 + 30 - 3));
        assert_eq!(high_wgt.stats.spd, clamp_stat(40 - 18 + 18));
    }

    #[test]
    fn frames_map_to_distinct_silhouettes_with_matching_presets() {
        let mut seen = Vec::new();
        for f in &FRAMES {
            assert!(!seen.contains(&f.silhouette), "duplicate frame silhouette");
            seen.push(f.silhouette);
            assert!(
                PRESETS.iter().any(|p| p.silhouette == f.silhouette),
                "frame silhouette must exist in PRESETS"
            );
        }
    }

    #[test]
    fn every_part_has_exactly_one_positive_and_one_negative_delta() {
        for slot in &PART_SLOTS {
            for p in slot {
                let deltas = [p.d_atk, p.d_def, p.d_sta, p.d_wgt, p.d_spd, p.d_mtr];
                let pos = deltas.iter().filter(|&&d| d > 0).count();
                let neg = deltas.iter().filter(|&&d| d < 0).count();
                let zero = deltas.iter().filter(|&&d| d == 0).count();
                assert_eq!(pos, 1, "{}: expected exactly one positive delta", p.name);
                assert_eq!(neg, 1, "{}: expected exactly one negative delta", p.name);
                assert_eq!(zero, 4, "{}: expected exactly four zero deltas", p.name);
            }
        }
    }
}
