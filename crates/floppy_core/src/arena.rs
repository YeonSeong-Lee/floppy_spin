//! Analytic bowl heightfield (SPEC §6.2): no grid, just a closed-form
//! `h(x, z)` plus its exact analytic gradient. Physics samples both directly;
//! the renderer (M3) tessellates the same function so the visual bowl and the
//! collision bowl can never drift apart.
//!
//! Shape: a parabolic basin, a cubic wall that kicks in past `WALL_START`,
//! and two low-amplitude decorative terms (concentric ridges + cross-hills)
//! that fade to zero by `r = 7.5` via an envelope — so the rim itself is a
//! clean, feature-free cubic bowl (SPEC §6.2 "the outer wall is clean").

use crate::fixmath;
use crate::vec::Vec3;

/// Bowl radius / nominal rim location, meters (SPEC §6.2: "~9.5 m").
pub const ARENA_RADIUS: f32 = 9.5;
/// Radius at which the cubic wall term begins (SPEC §6.2: "steep wall from
/// r ≈ 7 m").
pub const WALL_START: f32 = 7.0;
/// Ring-out trigger radius — a hair past the rim so a top has to visibly
/// clear the wall before it counts (SPEC §6.2).
pub const RING_OUT_RADIUS: f32 = 9.6;
/// Documented approximate rim height for design purposes (SPEC §6.2:
/// "rim height ~3.2 m"). NOTE: with the exact basin/wall coefficients below,
/// `height(ARENA_RADIUS, 0.0)` actually evaluates to ≈4.0 m, not 3.2 m — see
/// the `height_at_rim_is_in_documented_ballpark` test for the measured value
/// and rationale for keeping the formula as specified rather than re-tuning
/// it (the wall-climb gameplay feel it produces is validated by the physics
/// invariant tests instead).
pub const RIM_HEIGHT_DOC: f32 = 3.2;

const BASIN_COEFF: f32 = 0.02;
const WALL_COEFF: f32 = 2.2;
const WALL_SPAN: f32 = ARENA_RADIUS - WALL_START;
const RIDGE_AMPLITUDE: f32 = 0.06;
const RIDGE_FREQ: f32 = 4.0;
const CROSS_AMPLITUDE: f32 = 0.10;
const CROSS_FREQ: f32 = 0.9;
/// Envelope radius: ridges/cross-hills fade to zero by this radius.
const ENV_RADIUS: f32 = 7.5;
const ENV_RADIUS_SQ: f32 = ENV_RADIUS * ENV_RADIUS;

/// Radial envelope `(1 - (r/7.5)^2)` clamped to `[0, 1]` — decorative
/// features vanish approaching the rim (SPEC §6.2 intent).
fn envelope(r: f32) -> f32 {
    (1.0 - (r * r) / ENV_RADIUS_SQ).clamp(0.0, 1.0)
}

/// Derivative of [`envelope`] w.r.t. `r`. The raw expression `-2r/7.5^2` is
/// exact on the open interval where the clamp is inactive; at/beyond
/// `r = 7.5` (envelope pinned to 0) and vanishing at `r = 0` in the same
/// formula, the derivative is 0. This introduces one clamp-boundary
/// discontinuity at `r = 7.5` (documented, intentional per the task spec:
/// normals feed forces, not energy integration, so a single-point
/// discontinuity in the derivative is harmless).
fn envelope_deriv(r: f32) -> f32 {
    let e = envelope(r);
    if e > 0.0 && e < 1.0 {
        -2.0 * r / ENV_RADIUS_SQ
    } else {
        0.0
    }
}

/// Wall term `w = max(0, (r - WALL_START) / (ARENA_RADIUS - WALL_START))`.
/// Deliberately NOT clamped above 1 — past the rim it keeps rising, which is
/// fine because ring-out (SPEC §6.2) triggers well before it matters.
fn wall_w(r: f32) -> f32 {
    ((r - WALL_START) / WALL_SPAN).max(0.0)
}

/// Bowl height at `(x, z)` (SPEC §6.2): parabolic basin + cubic wall +
/// enveloped ridges + enveloped cross-hills.
pub fn height(x: f32, z: f32) -> f32 {
    let r = fixmath::sqrt(x * x + z * z);
    let basin = BASIN_COEFF * r * r;
    let w = wall_w(r);
    let wall = WALL_COEFF * w * w * w;
    let env = envelope(r);
    let ridges = RIDGE_AMPLITUDE * fixmath::sin(r * RIDGE_FREQ) * env;
    let cross = CROSS_AMPLITUDE * fixmath::sin(x * CROSS_FREQ) * fixmath::sin(z * CROSS_FREQ) * env;
    basin + wall + ridges + cross
}

/// Analytic gradient of the basin term alone: `d/dx[0.02*(x^2+z^2)] =
/// 0.04*x` (and symmetric in `z`), differentiated directly rather than
/// through the chain-rule `r` form so it needs no division and stays exact
/// (and perfectly well-defined) at the origin. Shared by [`gradient`] and
/// [`structural_gradient`] so the two can never drift apart on this term.
fn basin_gradient(x: f32, z: f32) -> (f32, f32) {
    (2.0 * BASIN_COEFF * x, 2.0 * BASIN_COEFF * z)
}

/// Analytic gradient of the wall term alone, given the caller's
/// already-computed radial derivative direction `(dr_dx, dr_dz)` (zeroed in
/// the guarded neighborhood of the origin — see [`gradient`]'s doc
/// comment). `d/dr[2.2*w^3] = 6.6*w^2 * dw/dr`, `dw/dr = 1/WALL_SPAN` for `r
/// > WALL_START` (0 below, matching the `max(0, ..)` clamp's derivative).
/// Shared by [`gradient`] and [`structural_gradient`].
fn wall_gradient(r: f32, dr_dx: f32, dr_dz: f32) -> (f32, f32) {
    let w = wall_w(r);
    let dwall_dr = if r > WALL_START {
        3.0 * WALL_COEFF * w * w / WALL_SPAN
    } else {
        0.0
    };
    (dwall_dr * dr_dx, dwall_dr * dr_dz)
}

/// Analytic `(dh/dx, dh/dz)` — the exact chain-rule derivative of
/// [`height`], used to build the terrain normal for contact response.
///
/// Guard: for `r < 1e-4` the radial direction `(x/r, z/r)` is undefined, so
/// every term whose derivative is carried through that direction (wall,
/// ridges, and the `env'(r)` half of the cross-hills) is zeroed there, per
/// the task spec's explicit "guard r < 1e-4 -> radial terms zero" — the
/// terms are numerically negligible in that tiny neighborhood anyway (the
/// basin gradient `0.04*x`/`0.04*z`, computed directly rather than through
/// `r`, still contributes exactly and continuously through the origin).
pub fn gradient(x: f32, z: f32) -> (f32, f32) {
    let r = fixmath::sqrt(x * x + z * z);

    // dr/dx, dr/dz — zero in the guarded neighborhood of the origin.
    let (dr_dx, dr_dz) = if r < 1e-4 { (0.0, 0.0) } else { (x / r, z / r) };

    let (basin_dx, basin_dz) = basin_gradient(x, z);
    let (wall_dx, wall_dz) = wall_gradient(r, dr_dx, dr_dz);

    // Ridges: 0.06*sin(4r)*env(r), product rule over r then chain to x/z.
    let env = envelope(r);
    let env_deriv = envelope_deriv(r);
    let dridges_dr = RIDGE_AMPLITUDE
        * (RIDGE_FREQ * fixmath::cos(r * RIDGE_FREQ) * env
            + fixmath::sin(r * RIDGE_FREQ) * env_deriv);
    let ridges_dx = dridges_dr * dr_dx;
    let ridges_dz = dridges_dr * dr_dz;

    // Cross-hills: 0.10*sin(0.9x)*sin(0.9z)*env(r). Partial derivatives:
    // the "direct" trig term needs no r-chain, but the shared envelope term
    // does (via dr/dx, dr/dz), so it's zeroed by the same origin guard.
    let sx = fixmath::sin(x * CROSS_FREQ);
    let sz = fixmath::sin(z * CROSS_FREQ);
    let cx = fixmath::cos(x * CROSS_FREQ);
    let cz = fixmath::cos(z * CROSS_FREQ);
    let cross_dx = CROSS_AMPLITUDE * (CROSS_FREQ * cx * sz * env + sx * sz * env_deriv * dr_dx);
    let cross_dz = CROSS_AMPLITUDE * (sx * CROSS_FREQ * cz * env + sx * sz * env_deriv * dr_dz);

    (
        basin_dx + wall_dx + ridges_dx + cross_dx,
        basin_dz + wall_dz + ridges_dz + cross_dz,
    )
}

/// Gradient of ONLY the basin + wall terms — the "structural" slope that is
/// gameplay-legible steepness (basin bowl + outer wall). Deliberately
/// EXCLUDES the decorative ridge/cross-hill terms that [`gradient`] (and
/// [`height`]) include: their derivative amplitude (`RIDGE_AMPLITUDE *
/// RIDGE_FREQ` alone is ~0.24) swamps the handful-of-degrees thresholds a
/// gameplay verb check cares about almost everywhere the decoration is
/// active, turning what should be smooth basin/wall geography into a
/// patchwork of spurious steep readings.
///
/// Terrain contact, slope gravity, and Carve's climb bonus all still read
/// the FULL [`gradient`] — ridges are real physical bumps a top rolls over
/// and feels, and should stay that way. This function exists specifically
/// for verb-viability geography that must stay gameplay-legible instead:
/// Anchor's auto-break check and Guard's downhill-slide-extra check
/// (`physics.rs`) sample this rather than `gradient`, per the M4 verifier's
/// FIX 1 — the ridge/hill decoration stays purely physical flavor and must
/// not drive which verbs work where.
pub fn structural_gradient(x: f32, z: f32) -> (f32, f32) {
    let r = fixmath::sqrt(x * x + z * z);
    let (dr_dx, dr_dz) = if r < 1e-4 { (0.0, 0.0) } else { (x / r, z / r) };

    let (basin_dx, basin_dz) = basin_gradient(x, z);
    let (wall_dx, wall_dz) = wall_gradient(r, dr_dx, dr_dz);

    (basin_dx + wall_dx, basin_dz + wall_dz)
}

/// Terrain normal at `(x, z)`: `normalize(-dh/dx, 1, -dh/dz)`, i.e. the
/// upward-facing normal of the height field's tangent plane. Falls back to
/// the zero vector only in the degenerate case both partials AND the "1"
/// somehow vanish, which cannot happen here (the y component is always
/// exactly `1.0`), so this is effectively always unit length.
pub fn normal(x: f32, z: f32) -> Vec3 {
    let (dhdx, dhdz) = gradient(x, z);
    Vec3::new(-dhdx, 1.0, -dhdz).normalize_or_zero()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Central finite-difference gradient, used only to validate the
    /// analytic [`gradient`] above — independent of it, deliberately crude
    /// (plain central difference, no LUT-awareness needed since `height`
    /// itself already routes through `fixmath`).
    fn finite_diff_gradient(x: f32, z: f32) -> (f32, f32) {
        const EPS: f32 = 1e-3;
        let dhdx = (height(x + EPS, z) - height(x - EPS, z)) / (2.0 * EPS);
        let dhdz = (height(x, z + EPS) - height(x, z - EPS)) / (2.0 * EPS);
        (dhdx, dhdz)
    }

    #[test]
    fn gradient_matches_central_finite_difference_over_grid() {
        // ~200 points covering basin / ridge / wall / rim regions: a 14x15
        // grid of (x, z) pairs spanning r in [0, ~10].
        let mut max_err: f32 = 0.0;
        let mut count = 0;
        for ix in -7..=7i32 {
            for iz in -7..=7i32 {
                let x = ix as f32 * 1.0; // -7.0..=7.0
                let z = iz as f32 * 1.0;
                // Also probe a few finer points to make sure the wall/rim
                // band (r in [7, 9.6]) is well covered even though the
                // integer grid above tops out around r ~ 9.9 only on the
                // diagonals.
                let (ax, az) = gradient(x, z);
                let (fx, fz) = finite_diff_gradient(x, z);
                max_err = max_err.max((ax - fx).abs()).max((az - fz).abs());
                count += 1;
            }
        }
        // Extra targeted samples in the wall band.
        for i in 0..40 {
            let r = 6.5 + 3.0 * (i as f32) / 39.0; // 6.5..=9.5
            let theta = i as f32 * 0.7;
            let x = fixmath::cos(theta) * r;
            let z = fixmath::sin(theta) * r;
            let (ax, az) = gradient(x, z);
            let (fx, fz) = finite_diff_gradient(x, z);
            max_err = max_err.max((ax - fx).abs()).max((az - fz).abs());
            count += 1;
        }
        assert!(count >= 200, "count={count}");
        assert!(max_err < 2e-2, "max_err={max_err}");
    }

    #[test]
    fn height_at_center_is_below_ridge_amplitude() {
        let h0 = height(0.0, 0.0);
        assert!(
            h0 < RIDGE_AMPLITUDE,
            "h0={h0} ridge_amplitude={RIDGE_AMPLITUDE}"
        );
    }

    #[test]
    fn height_at_rim_is_in_documented_ballpark() {
        // See the RIM_HEIGHT_DOC doc comment: the exact formula measures
        // ~4.0 m at r = ARENA_RADIUS, not the SPEC's ~3.2 m estimate (basin
        // 1.805 m + wall 2.2 m = 4.005 m, since w = 1.0 exactly at the rim).
        // We assert against the real computed value with a tight tolerance
        // (regression pin) and separately assert it's within a generous
        // "few meters, clearly taller than the basin" envelope of the
        // documented estimate so a future formula change that drifts wildly
        // out of the intended ballpark still fails loudly.
        let h = height(ARENA_RADIUS, 0.0);
        assert!((h - 4.005).abs() < 0.05, "h={h}");
        assert!(
            (h - RIM_HEIGHT_DOC).abs() < 1.0,
            "h={h} doc_estimate={RIM_HEIGHT_DOC}"
        );
    }

    #[test]
    fn normal_at_center_points_up() {
        let n = normal(0.0, 0.0);
        assert!((n.x).abs() < 1e-3, "n.x={}", n.x);
        assert!((n.y - 1.0).abs() < 1e-3, "n.y={}", n.y);
        assert!((n.z).abs() < 1e-3, "n.z={}", n.z);
    }

    #[test]
    fn normal_is_unit_length_away_from_origin() {
        let n = normal(5.0, 3.0);
        let len = n.length();
        assert!((len - 1.0).abs() < 1e-3, "len={len}");
    }

    /// FIX 1 (M4 verifier): `structural_gradient` must equal the analytic
    /// basin-only derivative at a point deep in the basin (r=5, on the +x
    /// axis so the cross-hill/ridge terms — which the full `gradient`
    /// includes but this one must not — would otherwise be large and
    /// obvious if they leaked in).
    #[test]
    fn structural_gradient_matches_basin_only_deep_in_the_basin() {
        let (gx, gz) = structural_gradient(5.0, 0.0);
        assert!((gx - 2.0 * BASIN_COEFF * 5.0).abs() < 1e-4, "gx={gx}");
        assert!(gz.abs() < 1e-4, "gz={gz}");
    }

    /// Past `WALL_START`, the structural gradient must equal basin + wall
    /// with no ridge/cross-hill contribution at all.
    #[test]
    fn structural_gradient_adds_the_wall_term_past_wall_start() {
        let r = 8.0f32;
        let (gx, gz) = structural_gradient(r, 0.0);
        let w = (r - WALL_START) / WALL_SPAN;
        let expected_wall_dr = 3.0 * WALL_COEFF * w * w / WALL_SPAN;
        let expected = 2.0 * BASIN_COEFF * r + expected_wall_dr;
        assert!((gx - expected).abs() < 1e-3, "gx={gx} expected={expected}");
        assert!(gz.abs() < 1e-4, "gz={gz}");
    }

    /// The whole point of FIX 1: at a point where the decorative ridge/
    /// cross-hill terms are NOT negligible, the full `gradient` and the
    /// `structural_gradient` must measurably diverge (proving the latter
    /// really does exclude them, not just coincide with the former
    /// everywhere by construction).
    #[test]
    fn structural_gradient_diverges_from_full_gradient_where_decoration_is_active() {
        let (fgx, fgz) = gradient(3.0, 2.0);
        let (sgx, sgz) = structural_gradient(3.0, 2.0);
        let dx = fgx - sgx;
        let dz = fgz - sgz;
        let diff = fixmath::sqrt(dx * dx + dz * dz);
        assert!(
            diff > 0.01,
            "expected full and structural gradients to diverge here, diff={diff}"
        );
    }
}
