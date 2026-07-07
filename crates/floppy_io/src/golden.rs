//! Golden-frame tolerance compare (SPEC §12.2): "per-channel mean abs diff
//! <= 2 AND <= 1% of pixels with any channel diff > 24" against a checked-in
//! reference PNG. Shared by `headless.rs`'s `--golden check` and the
//! root-package `tests/goldens.rs` integration test so both call exactly the
//! same rule.

/// Result of comparing two same-size `0x00RRGGBB` framebuffers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GoldenReport {
    /// Mean absolute per-pixel difference, one entry per channel (R, G, B).
    pub mean_abs_diff: [f32; 3],
    /// Fraction (`0.0..=1.0`) of pixels where ANY channel's absolute
    /// difference exceeds 24.
    pub over_threshold_fraction: f32,
    /// `true` iff every `mean_abs_diff` entry is `<= 2.0` AND
    /// `over_threshold_fraction <= 0.01` (SPEC §12.2).
    pub pass: bool,
}

const MEAN_DIFF_LIMIT: f32 = 2.0;
const PER_PIXEL_DIFF_LIMIT: i32 = 24;
const OVER_THRESHOLD_FRACTION_LIMIT: f32 = 0.01;

fn channels(p: u32) -> [u8; 3] {
    [
        ((p >> 16) & 0xFF) as u8,
        ((p >> 8) & 0xFF) as u8,
        (p & 0xFF) as u8,
    ]
}

/// Compare two framebuffers of `0x00RRGGBB` pixels against the SPEC §12.2
/// tolerance. Mismatched lengths (including either side being empty) are an
/// automatic failure — never a panic or an out-of-bounds read — with the
/// worst possible report (`mean_abs_diff` at the max representable
/// difference, `over_threshold_fraction = 1.0`) so a caller that only checks
/// `.pass` still fails loudly on that case.
pub fn compare(a: &[u32], b: &[u32]) -> GoldenReport {
    if a.len() != b.len() || a.is_empty() {
        return GoldenReport {
            mean_abs_diff: [255.0, 255.0, 255.0],
            over_threshold_fraction: 1.0,
            pass: false,
        };
    }

    let n = a.len() as f64;
    let mut sum_abs = [0f64; 3];
    let mut over_count: u64 = 0;

    for (&pa, &pb) in a.iter().zip(b.iter()) {
        let ca = channels(pa);
        let cb = channels(pb);
        let mut any_over = false;
        for c in 0..3 {
            let diff = (ca[c] as i32 - cb[c] as i32).abs();
            sum_abs[c] += diff as f64;
            if diff > PER_PIXEL_DIFF_LIMIT {
                any_over = true;
            }
        }
        if any_over {
            over_count += 1;
        }
    }

    let mean_abs_diff = [
        (sum_abs[0] / n) as f32,
        (sum_abs[1] / n) as f32,
        (sum_abs[2] / n) as f32,
    ];
    let over_threshold_fraction = (over_count as f64 / n) as f32;
    let pass = mean_abs_diff.iter().all(|&m| m <= MEAN_DIFF_LIMIT)
        && over_threshold_fraction <= OVER_THRESHOLD_FRACTION_LIMIT;

    GoldenReport {
        mean_abs_diff,
        over_threshold_fraction,
        pass,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(color: u32, n: usize) -> Vec<u32> {
        vec![color; n]
    }

    #[test]
    fn identical_frames_pass_with_zero_diff() {
        let a = solid(0x00112233, 100);
        let report = compare(&a, &a);
        assert_eq!(report.mean_abs_diff, [0.0, 0.0, 0.0]);
        assert_eq!(report.over_threshold_fraction, 0.0);
        assert!(report.pass);
    }

    #[test]
    fn mean_diff_exactly_at_the_two_limit_still_passes() {
        // Every pixel's R channel differs by exactly 2, G/B untouched.
        let a = solid(0x00000000, 50);
        let b = solid(0x00020000, 50);
        let report = compare(&a, &b);
        assert!((report.mean_abs_diff[0] - 2.0).abs() < 1e-5);
        assert!(report.pass, "report={report:?}");
    }

    #[test]
    fn mean_diff_just_over_the_limit_fails() {
        let a = solid(0x00000000, 50);
        let b = solid(0x00030000, 50);
        let report = compare(&a, &b);
        assert!((report.mean_abs_diff[0] - 3.0).abs() < 1e-5);
        assert!(!report.pass, "report={report:?}");
    }

    #[test]
    fn small_fraction_of_large_diffs_within_one_percent_still_passes() {
        // 200 pixels, 1 differs by 100 (0.5% <= 1%, mean = 100/200 = 0.5 <= 2).
        let mut a = solid(0x00000000, 200);
        let mut b = solid(0x00000000, 200);
        a[0] = 0x00000000;
        b[0] = 0x00640000; // R channel diff of 100
        let report = compare(&a, &b);
        assert!((report.over_threshold_fraction - 0.005).abs() < 1e-6);
        assert!(report.pass, "report={report:?}");
    }

    #[test]
    fn over_one_percent_large_diffs_fails_even_with_a_low_mean() {
        // 200 pixels, 3 differ by 100 (1.5% > 1%); mean = 300/200 = 1.5 <= 2
        // on its own, but the over-threshold-fraction rule must still fail it.
        let mut a = solid(0x00000000, 200);
        let mut b = solid(0x00000000, 200);
        for i in 0..3 {
            a[i] = 0x00000000;
            b[i] = 0x00640000;
        }
        let report = compare(&a, &b);
        assert!((report.mean_abs_diff[0] - 1.5).abs() < 1e-5);
        assert!(report.over_threshold_fraction > 0.01);
        assert!(!report.pass, "report={report:?}");
    }

    #[test]
    fn exactly_at_the_per_pixel_diff_boundary_does_not_count_as_over() {
        // A diff of exactly 24 must NOT count toward over_threshold_fraction
        // ("diff > 24", strictly greater).
        let a = solid(0x00000000, 10);
        let b = solid(0x00180000, 10); // 0x18 = 24
        let report = compare(&a, &b);
        assert_eq!(report.over_threshold_fraction, 0.0);
    }

    #[test]
    fn mismatched_lengths_fail_without_panicking() {
        let a = solid(0x0, 10);
        let b = solid(0x0, 11);
        let report = compare(&a, &b);
        assert!(!report.pass);
    }

    #[test]
    fn empty_slices_fail_without_panicking() {
        let report = compare(&[], &[]);
        assert!(!report.pass);
    }
}
