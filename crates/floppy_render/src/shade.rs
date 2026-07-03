//! Gouraud lighting (game_design.md §6): one fixed directional key light +
//! flat ambient + an integer-exponent Blinn-Phong specular hotspot, shaded
//! per-vertex (not per-pixel) — cheap enough to stay well inside SPEC §10's
//! budget at hundreds-of-triangles scale, and "flat/Gouraud + one hard
//! specular hotspot reads as chrome" is the explicit aesthetic target.

use floppy_core::vec::Vec3;
use std::sync::OnceLock;

pub struct Material {
    pub base: Vec3,
    pub emissive: Vec3,
    /// Specular exponent as an integer (computed via repeated squaring, no
    /// `powf` — SPEC §5 bans libm outside `fixmath`).
    pub spec_power_i: u32,
    pub spec_strength: f32,
}

/// Directional key light color (game_design.md §6 lighting fixture).
const KEY_COLOR: Vec3 = Vec3::new(1.0, 0.97, 0.9);
/// Flat ambient term (game_design.md §6 lighting fixture).
const AMBIENT: Vec3 = Vec3::new(0.10, 0.11, 0.16);

static KEY_LIGHT_DIR: OnceLock<Vec3> = OnceLock::new();

/// Normalized key light direction, computed once (the raw spec vector
/// `(-0.45, -0.8, 0.35)` isn't unit length) and cached — every call after the
/// first is a plain read, no repeated `rsqrt`.
fn key_light_dir() -> Vec3 {
    *KEY_LIGHT_DIR.get_or_init(|| Vec3::new(-0.45, -0.8, 0.35).normalize_or_zero())
}

/// Componentwise (Hadamard) product — `floppy_core::vec::Vec3` only exposes
/// scalar `Mul`, so this small helper lives here instead.
fn mul3(a: Vec3, b: Vec3) -> Vec3 {
    Vec3::new(a.x * b.x, a.y * b.y, a.z * b.z)
}

/// `x^n` for integer `n` via exponentiation by repeated squaring — plain
/// multiplication only, so it's exempt from the `no_libm` scan (no
/// `powf`/`powi` call).
fn ipow(x: f32, n: u32) -> f32 {
    if n == 0 {
        return 1.0;
    }
    let mut base = x;
    let mut exp = n;
    let mut result = 1.0f32;
    while exp > 0 {
        if exp & 1 == 1 {
            result *= base;
        }
        base *= base;
        exp >>= 1;
    }
    result
}

/// Shade one vertex/point: `n` the surface normal, `view_dir` pointing FROM
/// the surface TOWARD the camera (both normalized internally, so callers
/// don't have to pre-normalize). Diffuse uses `max(0, -n.dot(l))` (light
/// direction `l` points from the light toward the surface, so illumination
/// needs the negated dot). Specular is Blinn-Phong: `halfway =
/// normalize(view_dir - l)`, `spec = max(0, n.dot(halfway))^power *
/// strength`. Output is `base*(ambient + diffuse*key_color) +
/// spec*key_color + emissive`, clamped to `[0, 1]` per channel.
pub fn shade(n: Vec3, view_dir: Vec3, m: &Material) -> Vec3 {
    let l = key_light_dir();
    let nn = n.normalize_or_zero();
    let v = view_dir.normalize_or_zero();

    let diffuse = (-nn.dot(l)).max(0.0);

    let half = (v - l).normalize_or_zero();
    let ndoth = nn.dot(half).max(0.0);
    let spec = ipow(ndoth, m.spec_power_i) * m.spec_strength;

    let lit = AMBIENT + KEY_COLOR * diffuse;
    let out = mul3(m.base, lit) + KEY_COLOR * spec + m.emissive;
    out.clamp(Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 1.0, 1.0))
}

/// Pack a `[0, 1]`-range linear color into `0x00RRGGBB` (clamps out-of-range
/// input rather than wrapping/panicking).
pub fn color_to_px(c: Vec3) -> u32 {
    let r = (c.x.clamp(0.0, 1.0) * 255.0) as u32;
    let g = (c.y.clamp(0.0, 1.0) * 255.0) as u32;
    let b = (c.z.clamp(0.0, 1.0) * 255.0) as u32;
    (r << 16) | (g << 8) | b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shade_output_stays_in_zero_one_range() {
        let m = Material {
            base: Vec3::new(2.0, 2.0, 2.0), // deliberately out-of-range input
            emissive: Vec3::new(0.5, 0.5, 0.5),
            spec_power_i: 32,
            spec_strength: 1.0,
        };
        let n = Vec3::new(0.0, 1.0, 0.0);
        let v = Vec3::new(0.0, 1.0, -1.0);
        let c = shade(n, v, &m);
        assert!((0.0..=1.0).contains(&c.x));
        assert!((0.0..=1.0).contains(&c.y));
        assert!((0.0..=1.0).contains(&c.z));
    }

    #[test]
    fn facing_the_key_light_is_brighter_than_facing_away() {
        let m = Material {
            base: Vec3::new(0.5, 0.5, 0.5),
            emissive: Vec3::new(0.0, 0.0, 0.0),
            spec_power_i: 8,
            spec_strength: 0.0,
        };
        let l = key_light_dir();
        let n_toward = -l; // surface normal opposing the light direction = lit
        let n_away = l;
        let v = Vec3::new(0.0, 1.0, 0.0);
        let lit = shade(n_toward, v, &m);
        let dark = shade(n_away, v, &m);
        assert!(lit.x > dark.x, "lit={} dark={}", lit.x, dark.x);
    }

    #[test]
    fn ipow_matches_repeated_multiplication() {
        assert_eq!(ipow(2.0, 0), 1.0);
        assert_eq!(ipow(2.0, 1), 2.0);
        assert_eq!(ipow(2.0, 10), 1024.0);
        assert!((ipow(0.5, 5) - 0.03125).abs() < 1e-6);
    }

    #[test]
    fn color_to_px_packs_and_clamps() {
        assert_eq!(color_to_px(Vec3::new(0.0, 0.0, 0.0)), 0x00000000);
        assert_eq!(color_to_px(Vec3::new(1.0, 1.0, 1.0)), 0x00FFFFFF);
        assert_eq!(color_to_px(Vec3::new(2.0, -1.0, 0.5)), 0x00FF007F);
    }
}
