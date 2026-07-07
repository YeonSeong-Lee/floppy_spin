//! The 7 launch-ready presets (game_design.md §3 roster table, verbatim
//! stats/spin-direction/accent). `floppy_render::battle` maps [`Silhouette`]
//! to a per-top lathe profile; this module only names/describes the roster —
//! `floppy_core` has no notion of meshes or pixels.
//!
//! Flavor lines are original one-liners written for this module:
//! game_design.md §3 explicitly defers them ("Flavor lines per top live with
//! the roster table in code, shown on TopSelect") rather than spelling them
//! out in the doc.

use crate::physics::Stats;

/// Body-shape identifier for the battle-scene renderer (game_design.md §3
/// "Silhouette" column). Order matches the roster table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Silhouette {
    Cleaver,
    Bulwark,
    Everspin,
    Keystone,
    Riptide,
    Gravewell,
    Mirrorfang,
}

/// One roster entry: name, TopSelect flavor line, base stats, spin
/// direction (SPEC §6.1: `+1` CW / `-1` CCW top-down), accent color
/// (`0x00RRGGBB`), and body silhouette.
#[derive(Clone, Copy, Debug)]
pub struct Preset {
    pub name: &'static str,
    pub flavor: &'static str,
    pub stats: Stats,
    pub spin_dir: i8,
    pub accent: u32,
    pub silhouette: Silhouette,
}

/// The 7 presets, in game_design.md §3 table order.
pub const PRESETS: [Preset; 7] = [
    Preset {
        name: "Cleaver",
        flavor: "Six teeth, one purpose: open you up.",
        stats: Stats {
            atk: 88,
            def: 30,
            sta: 34,
            wgt: 58,
            spd: 62,
            mtr: 30,
        },
        spin_dir: 1,
        accent: 0x00FF_2D55,
        silhouette: Silhouette::Cleaver,
    },
    Preset {
        name: "Bulwark",
        flavor: "Immovable isn't a boast. It's a description.",
        stats: Stats {
            atk: 28,
            def: 90,
            sta: 52,
            wgt: 78,
            spd: 22,
            mtr: 30,
        },
        spin_dir: -1,
        accent: 0x002D_7DFF,
        silhouette: Silhouette::Bulwark,
    },
    Preset {
        name: "Everspin",
        flavor: "It doesn't spin — it circles. Forever, if you let it.",
        stats: Stats {
            atk: 24,
            def: 44,
            sta: 92,
            wgt: 40,
            spd: 48,
            mtr: 52,
        },
        spin_dir: 1,
        accent: 0x0039_FF14,
        silhouette: Silhouette::Everspin,
    },
    Preset {
        name: "Keystone",
        flavor: "No weakness. No miracle, either.",
        stats: Stats {
            atk: 52,
            def: 54,
            sta: 52,
            wgt: 50,
            spd: 50,
            mtr: 44,
        },
        spin_dir: -1,
        accent: 0x00FF_D400,
        silhouette: Silhouette::Keystone,
    },
    Preset {
        name: "Riptide",
        flavor: "Never where you swung. Always where you're not.",
        stats: Stats {
            atk: 70,
            def: 26,
            sta: 40,
            wgt: 34,
            spd: 84,
            mtr: 46,
        },
        spin_dir: -1,
        accent: 0x0000_E5D0,
        silhouette: Silhouette::Riptide,
    },
    Preset {
        name: "Gravewell",
        flavor: "The bowl bends toward it. Everything else follows.",
        stats: Stats {
            atk: 46,
            def: 72,
            sta: 30,
            wgt: 90,
            spd: 20,
            mtr: 42,
        },
        spin_dir: 1,
        accent: 0x00B0_26FF,
        silhouette: Silhouette::Gravewell,
    },
    Preset {
        name: "Mirrorfang",
        flavor: "Throw your best hit. Watch it come home.",
        stats: Stats {
            atk: 40,
            def: 66,
            sta: 58,
            wgt: 44,
            spd: 46,
            mtr: 48,
        },
        spin_dir: -1,
        accent: 0x00FF_7A00,
        silhouette: Silhouette::Mirrorfang,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    fn find(name: &str) -> &'static Preset {
        PRESETS
            .iter()
            .find(|p| p.name == name)
            .unwrap_or_else(|| panic!("no preset named {name}"))
    }

    #[test]
    fn every_preset_stat_total_is_within_300_plus_minus_6() {
        for p in &PRESETS {
            let s = p.stats;
            let sum = s.atk as i32
                + s.def as i32
                + s.sta as i32
                + s.wgt as i32
                + s.spd as i32
                + s.mtr as i32;
            assert!(
                (sum - 300).abs() <= 6,
                "{}: stat sum {sum} outside 300+-6",
                p.name
            );
        }
    }

    #[test]
    fn accents_match_game_design_doc_hex_values() {
        let expected: [(&str, u32); 7] = [
            ("Cleaver", 0x00FF_2D55),
            ("Bulwark", 0x002D_7DFF),
            ("Everspin", 0x0039_FF14),
            ("Keystone", 0x00FF_D400),
            ("Riptide", 0x0000_E5D0),
            ("Gravewell", 0x00B0_26FF),
            ("Mirrorfang", 0x00FF_7A00),
        ];
        for (name, accent) in expected {
            assert_eq!(find(name).accent, accent, "{name} accent mismatch");
        }
    }

    #[test]
    fn spin_dirs_match_game_design_doc() {
        let expected: [(&str, i8); 7] = [
            ("Cleaver", 1),
            ("Bulwark", -1),
            ("Everspin", 1),
            ("Keystone", -1),
            ("Riptide", -1),
            ("Gravewell", 1),
            ("Mirrorfang", -1),
        ];
        for (name, dir) in expected {
            assert_eq!(find(name).spin_dir, dir, "{name} spin_dir mismatch");
        }
    }

    #[test]
    fn every_preset_has_a_nonempty_flavor_line() {
        for p in &PRESETS {
            assert!(!p.flavor.is_empty(), "{} missing a flavor line", p.name);
        }
    }

    #[test]
    fn every_preset_has_a_distinct_silhouette() {
        let mut seen: Vec<Silhouette> = Vec::new();
        for p in &PRESETS {
            assert!(
                !seen.contains(&p.silhouette),
                "duplicate silhouette for {}",
                p.name
            );
            seen.push(p.silhouette);
        }
    }

    #[test]
    fn every_preset_name_is_distinct() {
        let mut seen: Vec<&str> = Vec::new();
        for p in &PRESETS {
            assert!(!seen.contains(&p.name), "duplicate name {}", p.name);
            seen.push(p.name);
        }
    }
}
