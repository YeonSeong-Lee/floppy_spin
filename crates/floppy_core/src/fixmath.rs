//! Deterministic replacements for libm transcendentals (SPEC §5, C5).
//!
//! **This module is the ONLY place in the whole workspace allowed to compute
//! trig or square roots.** Rationale: IEEE-754 basic operations (`+ - * /` and
//! comparisons) are required by the standard to be *exactly rounded*, so they
//! produce bit-identical results on every conforming platform/CPU. Libm's
//! `sin`/`cos`/`sqrt`/... are NOT specified to be exactly rounded and, worse,
//! different C runtimes (glibc vs musl vs MSVC's `ucrt` vs `libm` shipped with
//! a `windows-gnu` toolchain) ship different polynomial approximations that
//! disagree in the last few bits. A game that hashes simulation state for
//! determinism (SPEC C5) cannot tolerate that: two bit-identical inputs must
//! produce a bit-identical hash forever, independent of OS/CPU/compiler
//! version. So every transcendental the sim needs is reimplemented here from
//! primitive `+ - * /` operations with a *fixed* number of steps, and nothing
//! outside this file is allowed to call the real thing (`tests/no_libm.rs`
//! greps for it).
//!
//! All functions in this module are total: they never panic and never let a
//! `NaN` escape into the sim. Policy: `NaN` input maps to `0.0`; `sqrt`/`rsqrt`
//! additionally clamp non-positive input to `0.0` (documented per-function
//! below). This matches SPEC §5's "deterministic clamps" philosophy — bad
//! floating point state gets squashed to a safe, fixed value rather than
//! propagating.
//!
//! f64 arithmetic appears in exactly two places — LUT construction (Taylor
//! series, one-time) and the per-call sin/cos range reduction in
//! [`lut_index_and_frac`] — and both are restricted to IEEE-exact operations
//! (`+ - * /`, `fract`, `round`-to-integral), so they are just as
//! deterministic as the f32 paths.

use std::sync::OnceLock;

/// Number of entries in the full-period sine LUT (SPEC §5: "4096-entry").
const LUT_SIZE: usize = 4096;

/// `sin(2*pi*i/LUT_SIZE)` for `i` in `0..LUT_SIZE`, built once lazily.
static SIN_LUT: OnceLock<[f32; LUT_SIZE]> = OnceLock::new();

/// Exact octant reference values (multiples of pi/4) used to reduce an
/// arbitrary angle to a small residual before applying the Taylor series.
/// `std::f64::consts::FRAC_1_SQRT_2` is a compile-time constant (not a libm
/// call), so these tables cost nothing at runtime and involve no
/// approximation of their own.
const SIN8: [f64; 8] = [
    0.0,
    std::f64::consts::FRAC_1_SQRT_2,
    1.0,
    std::f64::consts::FRAC_1_SQRT_2,
    0.0,
    -std::f64::consts::FRAC_1_SQRT_2,
    -1.0,
    -std::f64::consts::FRAC_1_SQRT_2,
];
const COS8: [f64; 8] = [
    1.0,
    std::f64::consts::FRAC_1_SQRT_2,
    0.0,
    -std::f64::consts::FRAC_1_SQRT_2,
    -1.0,
    -std::f64::consts::FRAC_1_SQRT_2,
    0.0,
    std::f64::consts::FRAC_1_SQRT_2,
];

/// 8-term Taylor series for `sin(r)`, valid for small `|r| <= pi/8` (used only
/// during one-time LUT construction, in f64, so the eventual f32 LUT entry is
/// exact after rounding). This file is excluded from the no-libm source scan
/// (`tests/no_libm.rs`) precisely so this constructor can use plain f64
/// arithmetic and standard-library float helpers freely — it IS the
/// authoritative implementation the rest of the workspace is forbidden from
/// re-deriving.
fn taylor_sin_f64(r: f64) -> f64 {
    // sum = r - r^3/3! + r^5/5! - r^7/7! + r^9/9! - r^11/11! + r^13/13! - r^15/15!
    const FACT: [f64; 7] = [
        6.0,
        120.0,
        5_040.0,
        362_880.0,
        39_916_800.0,
        6_227_020_800.0,
        1_307_674_368_000.0,
    ];
    let r2 = r * r;
    let mut term = r;
    let mut sum = r;
    let mut sign = -1.0_f64;
    for f in FACT.iter() {
        term *= r2;
        sum += sign * term / f;
        sign = -sign;
    }
    sum
}

/// 8-term Taylor series for `cos(r)`, valid for small `|r| <= pi/8`. See
/// [`taylor_sin_f64`] for the LUT-construction rationale.
fn taylor_cos_f64(r: f64) -> f64 {
    // sum = 1 - r^2/2! + r^4/4! - r^6/6! + r^8/8! - r^10/10! + r^12/12! - r^14/14!
    const FACT: [f64; 7] = [
        2.0,
        24.0,
        720.0,
        40_320.0,
        3_628_800.0,
        479_001_600.0,
        87_178_291_200.0,
    ];
    let r2 = r * r;
    let mut term = 1.0_f64;
    let mut sum = 1.0_f64;
    let mut sign = -1.0_f64;
    for f in FACT.iter() {
        term *= r2;
        sum += sign * term / f;
        sign = -sign;
    }
    sum
}

/// `sin(theta)` for `theta` already reduced to `[0, 2*pi)`, computed by
/// reducing to the nearest multiple of pi/4 (an "octant") — for which the
/// exact value is a known constant (0, +-1, +-sqrt(2)/2) — and applying the
/// angle-sum identity with an 8-term Taylor series over the small residual.
fn sin_reduced_f64(theta: f64) -> f64 {
    let frac_pi_4 = std::f64::consts::FRAC_PI_4;
    let k = (theta / frac_pi_4).round();
    let r = theta - k * frac_pi_4;
    let k8 = (k as i64 as usize) % 8;
    SIN8[k8] * taylor_cos_f64(r) + COS8[k8] * taylor_sin_f64(r)
}

fn build_sin_lut() -> [f32; LUT_SIZE] {
    let mut lut = [0.0_f32; LUT_SIZE];
    let tau = 2.0 * std::f64::consts::PI;
    for (i, slot) in lut.iter_mut().enumerate() {
        let theta = (i as f64) * tau / (LUT_SIZE as f64);
        *slot = sin_reduced_f64(theta) as f32;
    }
    lut
}

fn sin_lut() -> &'static [f32; LUT_SIZE] {
    SIN_LUT.get_or_init(build_sin_lut)
}

/// Map an arbitrary finite `rad` to a fractional-turn LUT index (in
/// `0..LUT_SIZE`) plus the interpolation fraction `t` in `[0, 1)` toward the
/// next entry. Range reduction (`rad * 1/(2*pi)`, then `fract`) is done in
/// f64 so the result stays meaningful up to fairly large `|rad|`; precision
/// degrades gracefully beyond about `1e4` radians (the sim clamps angles well
/// below that, SPEC §5), since an f64 turn count there still carries ~11
/// significant fractional digits.
fn lut_index_and_frac(rad: f32) -> (usize, f32) {
    const INV_TAU: f64 = 1.0 / (2.0 * std::f64::consts::PI);
    let turns = (rad as f64) * INV_TAU;
    let mut f = turns.fract();
    if f < 0.0 {
        f += 1.0;
    }
    let pos = f * (LUT_SIZE as f64);
    let idx0 = (pos as usize) % LUT_SIZE;
    let t = (pos - (idx0 as f64)) as f32;
    (idx0, t)
}

/// Sine via the shared 4096-entry LUT with linear interpolation. `NaN` or
/// infinite input maps to `0.0` (never panics, never propagates `NaN`).
pub fn sin(rad: f32) -> f32 {
    if !rad.is_finite() {
        return 0.0;
    }
    let (idx0, t) = lut_index_and_frac(rad);
    let lut = sin_lut();
    let idx1 = (idx0 + 1) % LUT_SIZE;
    let a = lut[idx0];
    let b = lut[idx1];
    let result = a + (b - a) * t;
    if result.is_nan() {
        0.0
    } else {
        result
    }
}

/// Cosine via the same LUT as [`sin`], phase-shifted by a quarter period
/// (`LUT_SIZE / 4` entries = pi/2 radians) — a full-period LUT already
/// contains every cosine value, so there is no need for a second table.
pub fn cos(rad: f32) -> f32 {
    if !rad.is_finite() {
        return 0.0;
    }
    let (idx0, t) = lut_index_and_frac(rad);
    let lut = sin_lut();
    let offset = LUT_SIZE / 4;
    let idx0c = (idx0 + offset) % LUT_SIZE;
    let idx1c = (idx0c + 1) % LUT_SIZE;
    let a = lut[idx0c];
    let b = lut[idx1c];
    let result = a + (b - a) * t;
    if result.is_nan() {
        0.0
    } else {
        result
    }
}

/// `sqrt(x)`: a bit-trick initial guess refined by exactly 3 Newton-Raphson
/// iterations, f32 arithmetic only. Policy: `x <= 0.0` (including `-0.0`,
/// `NaN`) clamps to `0.0`; non-finite positive input (`+inf`) returns `+inf`
/// directly rather than iterating (Newton on an infinite seed is well-defined
/// but pointless work, and this keeps the function total).
pub fn sqrt(x: f32) -> f32 {
    if x.is_nan() || x <= 0.0 {
        return 0.0;
    }
    if x.is_infinite() {
        return f32::INFINITY;
    }
    // Fast "magic number" initial guess for sqrt(x): halve the (biased)
    // exponent by shifting the bit pattern right one place, then correct
    // with a constant tuned so the result approximates sqrt(x) rather than
    // 1/sqrt(x) (the more commonly seen Quake variant).
    let i = x.to_bits();
    let guess_bits = (i >> 1) + 0x1FBD_1DF5;
    let mut y = f32::from_bits(guess_bits);

    // Newton-Raphson for f(y) = y^2 - x: y_{n+1} = 0.5 * (y_n + x / y_n).
    // Exactly 3 iterations, fixed, so evaluation cost and rounding behavior
    // are identical on every run.
    y = 0.5 * (y + x / y);
    y = 0.5 * (y + x / y);
    y = 0.5 * (y + x / y);

    if y.is_nan() {
        0.0
    } else {
        y
    }
}

/// `1/sqrt(x)`: the classic Quake III bit-trick seed refined by exactly 2
/// Newton-Raphson iterations, f32 arithmetic only. Policy: non-finite input,
/// `NaN`, or `x` at or below a tiny epsilon clamps to `0.0` (a guard against
/// division blowing up near zero, since the true reciprocal-sqrt diverges
/// there).
pub fn rsqrt(x: f32) -> f32 {
    const TINY_EPS: f32 = 1e-12;
    if !x.is_finite() || x <= TINY_EPS {
        return 0.0;
    }

    let i = x.to_bits();
    let guess_bits = 0x5F37_59DF - (i >> 1);
    let mut y = f32::from_bits(guess_bits);

    // Newton-Raphson for f(y) = 1/y^2 - x: y_{n+1} = y_n * (1.5 - 0.5*x*y_n^2).
    // Exactly 2 iterations, fixed.
    let half_x = 0.5 * x;
    y = y * (1.5 - half_x * y * y);
    y = y * (1.5 - half_x * y * y);

    if y.is_nan() {
        0.0
    } else {
        y
    }
}

/// Minimax polynomial approximation of `atan(z)` for `|z| <= 1`, degree 11
/// (odd terms only, Horner form), documented max absolute error on that
/// domain is about `1.3e-8` rad — far inside the `~1e-3` rad tolerance this
/// project needs (SPEC §5), leaving headroom for the f32 rounding in
/// `atan2`'s quadrant reduction below.
fn atan_poly(z: f32) -> f32 {
    const C1: f32 = 0.999_977_26;
    const C3: f32 = -0.332_623_47;
    const C5: f32 = 0.193_543_46;
    const C7: f32 = -0.116_432_87;
    const C9: f32 = 0.052_653_32;
    const C11: f32 = -0.011_721_2;
    let z2 = z * z;
    z * (C1 + z2 * (C3 + z2 * (C5 + z2 * (C7 + z2 * (C9 + z2 * C11)))))
}

/// `atan2(y, x)`, fixed operation count: reduce to `atan(z)` for some `|z| <=
/// 1` via the standard octant split, then correct by a quadrant offset.
/// `(0, 0)` maps to `0.0` (documented policy); `NaN` in either argument maps
/// to `0.0`.
pub fn atan2(y: f32, x: f32) -> f32 {
    if x.is_nan() || y.is_nan() {
        return 0.0;
    }
    if x == 0.0 && y == 0.0 {
        return 0.0;
    }

    const FRAC_PI_2: f32 = std::f32::consts::FRAC_PI_2;
    const PI: f32 = std::f32::consts::PI;

    let abs_x = x.abs();
    let abs_y = y.abs();

    let result = if abs_x >= abs_y {
        // |y/x| <= 1: base angle is atan(y/x), then shift by 0 or +-pi
        // depending on which half-plane x puts us in.
        let z = y / x;
        let base = atan_poly(z);
        if x > 0.0 {
            base
        } else if y >= 0.0 {
            base + PI
        } else {
            base - PI
        }
    } else {
        // |x/y| < 1: base angle is measured from the y-axis instead, i.e.
        // +-pi/2 minus atan(x/y).
        let z = x / y;
        let base = atan_poly(z);
        if y > 0.0 {
            FRAC_PI_2 - base
        } else {
            -FRAC_PI_2 - base
        }
    };

    if result.is_nan() {
        0.0
    } else {
        result
    }
}

#[cfg(test)]
mod accuracy_tests {
    use super::*;

    /// Independent f64 reference for `sin`/`cos`, written fresh here (not
    /// reusing any of this module's private helpers) so the accuracy tests
    /// below are a genuine cross-check rather than testing the
    /// implementation against itself. Uses only `+ - * /` and a plain
    /// `round`-based range reduction; the term count (16) is chosen for
    /// convergence over `|r| <= pi`, not for the tight [-pi/4, pi/4]
    /// reduction the real LUT builder uses.
    fn reference_sin_cos_f64(x: f64) -> (f64, f64) {
        let tau = 2.0 * std::f64::consts::PI;
        let r = x - tau * (x / tau).round();
        let r2 = r * r;

        let mut sin_sum = r;
        let mut sin_term = r;
        for k in 0..16u32 {
            sin_term *= -r2 / (((2 * k + 2) * (2 * k + 3)) as f64);
            sin_sum += sin_term;
        }

        let mut cos_sum = 1.0;
        let mut cos_term = 1.0;
        for k in 0..16u32 {
            cos_term *= -r2 / (((2 * k + 1) * (2 * k + 2)) as f64);
            cos_sum += cos_term;
        }

        (sin_sum, cos_sum)
    }

    #[test]
    fn sin_cos_match_reference_over_wide_range() {
        const N: usize = 1000;
        let mut max_sin_err: f32 = 0.0;
        let mut max_cos_err: f32 = 0.0;
        let mut max_pyth_err: f32 = 0.0;
        for i in 0..N {
            let x = -10.0 + 20.0 * (i as f32) / ((N - 1) as f32);
            let got_sin = sin(x);
            let got_cos = cos(x);
            let (ref_sin, ref_cos) = reference_sin_cos_f64(x as f64);
            max_sin_err = max_sin_err.max((got_sin - ref_sin as f32).abs());
            max_cos_err = max_cos_err.max((got_cos - ref_cos as f32).abs());
            let pyth = got_sin * got_sin + got_cos * got_cos;
            max_pyth_err = max_pyth_err.max((pyth - 1.0).abs());
        }
        // Tight tolerances on purpose (M1 verifier finding): the theoretical
        // lerp error of a 4096-entry table is ~3e-7, while a one-slot LUT
        // shift produces ~1.5e-3 error — a 2e-3 tolerance would have let that
        // bug class ship. 5e-6 keeps ~10x headroom over legitimate error
        // while catching any indexing/phase defect outright.
        assert!(max_sin_err < 5e-6, "max_sin_err={max_sin_err}");
        assert!(max_cos_err < 5e-6, "max_cos_err={max_cos_err}");
        assert!(max_pyth_err < 1e-5, "max_pyth_err={max_pyth_err}");
    }

    #[test]
    fn lut_phase_alignment_at_exact_points() {
        // Pins against uniform LUT phase shifts, which the Pythagorean check
        // is structurally blind to (sin²+cos²=1 holds under ANY shared phase
        // shift) and which loose range tolerances also missed (M1 verifier
        // finding). sin(0) must hit LUT slot 0 exactly (t = 0, entry 0.0).
        assert_eq!(sin(0.0), 0.0);
        assert!((sin(std::f32::consts::FRAC_PI_2) - 1.0).abs() < 1e-6);
        assert!(sin(std::f32::consts::PI).abs() < 1e-6);
        assert!((sin(3.0 * std::f32::consts::FRAC_PI_2) + 1.0).abs() < 1e-6);
        assert!((cos(0.0) - 1.0).abs() < 1e-6);
        assert!(cos(std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert!((cos(std::f32::consts::PI) + 1.0).abs() < 1e-6);
    }

    #[test]
    fn sqrt_relative_error_over_log_spaced_range() {
        const N: usize = 1000;
        let mut max_rel_err: f32 = 0.0;
        for i in 0..N {
            // log-spaced from 1e-6 to 1e6
            let log_x = -6.0 + 12.0 * (i as f32) / ((N - 1) as f32);
            let x = 10f32.powf(log_x);
            let s = sqrt(x);
            let rel_err = ((s * s - x) / x).abs();
            max_rel_err = max_rel_err.max(rel_err);
        }
        assert!(max_rel_err < 1e-3, "max_rel_err={max_rel_err}");
    }

    #[test]
    fn rsqrt_consistent_with_one_over_sqrt() {
        const N: usize = 1000;
        let mut max_rel_err: f32 = 0.0;
        for i in 0..N {
            let log_x = -6.0 + 12.0 * (i as f32) / ((N - 1) as f32);
            let x = 10f32.powf(log_x);
            let r = rsqrt(x);
            let expected = 1.0 / sqrt(x);
            let rel_err = ((r - expected) / expected).abs();
            max_rel_err = max_rel_err.max(rel_err);
        }
        assert!(max_rel_err < 1e-3, "max_rel_err={max_rel_err}");
    }

    #[test]
    fn atan2_matches_reference_on_grid_including_axes() {
        // Independent atan2 reference via the reference sin/cos above is
        // circular (sin/cos don't directly give atan2); instead cross-check
        // against a direct high-term Taylor arctangent reference computed
        // fresh here.
        // Plain alternating-series (Leibniz-style) Taylor sum for atan(w),
        // only accurate for small |w| — it converges too slowly near |w| = 1
        // to be useful directly (needs thousands of terms for 1e-3 accuracy
        // at w = 1), so `reference_atan_f64` below reduces to this small
        // range first via the standard half-argument identity.
        fn atan_series_f64(w: f64) -> f64 {
            let w2 = w * w;
            let mut sum = w;
            let mut term = w;
            for k in 0..40u32 {
                term *= -w2;
                sum += term / ((2 * k + 3) as f64);
            }
            sum
        }
        // Valid for any `z` (that's all this reference is used for below,
        // after the same octant reduction atan2 itself needs, so `|z| <=
        // 1`). Reduces to `|w| <= 0.5` via `atan(z) = pi/4 + atan((z-1)/(z+1))`
        // (and the mirror image for negative z) before summing the series,
        // so the series above converges fast everywhere it's used —
        // including exactly `z = 1`, which the test grid hits on its
        // diagonal (`y == x`).
        fn reference_atan_f64(z: f64) -> f64 {
            let frac_pi_4 = std::f64::consts::FRAC_PI_4;
            if z.abs() <= 0.5 {
                atan_series_f64(z)
            } else if z > 0.5 {
                frac_pi_4 + atan_series_f64((z - 1.0) / (z + 1.0))
            } else {
                -frac_pi_4 + atan_series_f64((1.0 + z) / (1.0 - z))
            }
        }
        fn reference_atan2_f64(y: f64, x: f64) -> f64 {
            if x == 0.0 && y == 0.0 {
                return 0.0;
            }
            let pi = std::f64::consts::PI;
            let frac_pi_2 = std::f64::consts::FRAC_PI_2;
            if x.abs() >= y.abs() {
                let base = reference_atan_f64(y / x);
                if x > 0.0 {
                    base
                } else if y >= 0.0 {
                    base + pi
                } else {
                    base - pi
                }
            } else {
                let base = reference_atan_f64(x / y);
                if y > 0.0 {
                    frac_pi_2 - base
                } else {
                    -frac_pi_2 - base
                }
            }
        }

        let mut max_err: f32 = 0.0;
        let coords: [f32; 9] = [-2.0, -1.0, -0.5, -0.001, 0.0, 0.001, 0.5, 1.0, 2.0];
        for &y in &coords {
            for &x in &coords {
                let got = atan2(y, x);
                let want = reference_atan2_f64(y as f64, x as f64) as f32;
                let err = (got - want).abs();
                max_err = max_err.max(err);
            }
        }
        assert!(max_err < 2e-3, "max_err={max_err}");
        // (0,0) policy
        assert_eq!(atan2(0.0, 0.0), 0.0);
    }

    #[test]
    fn nan_and_infinite_inputs_never_propagate() {
        assert_eq!(sin(f32::NAN), 0.0);
        assert_eq!(cos(f32::NAN), 0.0);
        assert_eq!(sin(f32::INFINITY), 0.0);
        assert_eq!(cos(f32::NEG_INFINITY), 0.0);
        assert_eq!(sqrt(f32::NAN), 0.0);
        assert_eq!(sqrt(-1.0), 0.0);
        assert_eq!(sqrt(0.0), 0.0);
        assert_eq!(rsqrt(f32::NAN), 0.0);
        assert_eq!(rsqrt(0.0), 0.0);
        assert_eq!(rsqrt(-4.0), 0.0);
        assert_eq!(atan2(f32::NAN, 1.0), 0.0);
        assert_eq!(atan2(1.0, f32::NAN), 0.0);
    }
}
