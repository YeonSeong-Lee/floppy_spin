//! Deterministic PRNG (SPEC §5): integer `xorshift64*`, `u64` state only —
//! no wall-clock, no OS entropy source ever enters `core`. Gameplay must only
//! consume the RNG at fixed, scripted points so replays stay bit-identical.

/// `xorshift64*` generator. A single `u64` of state; the public API never
/// exposes the raw state so its encoding can change without breaking callers.
pub struct Rng(u64);

/// Fixed nonzero replacement for a `0` seed — `xorshift` is stuck at all-zero
/// state forever, so `0` cannot be a valid seed. Chosen as a "random-looking"
/// odd constant (golden-ratio-derived, same family as splitmix64's gamma) to
/// avoid any accidental structure.
const ZERO_SEED_REPLACEMENT: u64 = 0x9E37_79B9_7F4A_7C15;

/// `xorshift64*` multiplier from Marsaglia's original paper / SPEC §5.
const XORSHIFT_STAR_MULTIPLIER: u64 = 0x2545_F491_4F6C_DD1D;

impl Rng {
    /// Seed the generator. Seed `0` is remapped to a fixed nonzero constant
    /// (documented above) so every seed produces a valid, non-degenerate
    /// stream.
    pub fn new(seed: u64) -> Self {
        let s = if seed == 0 {
            ZERO_SEED_REPLACEMENT
        } else {
            seed
        };
        Self(s)
    }

    /// Next 64-bit output. Advances the internal state.
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(XORSHIFT_STAR_MULTIPLIER)
    }

    /// Next 32-bit output: the high 32 bits of a `next_u64` draw (the
    /// higher-quality half of an xorshift* output).
    pub fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    /// Next f32 in `[0, 1)`. Uses the top 24 bits after discarding the low 40
    /// (an f32 mantissa only holds 24 significant bits, so this is the
    /// cheapest bit slice that can't produce a biased/rounded distribution).
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 * (1.0 / 16_777_216.0)
    }

    /// Uniform integer in `[lo, hi)`. Returns `lo` if the range is empty or
    /// inverted (`hi <= lo`) rather than panicking — this function is total.
    pub fn range_i32(&mut self, lo: i32, hi: i32) -> i32 {
        if hi <= lo {
            return lo;
        }
        let span = hi.wrapping_sub(lo) as u32 as u64;
        lo.wrapping_add((self.next_u64() % span) as i32)
    }
}

/// Round-seed function (SPEC §6.5): `mix(match_seed, round)`. A pure,
/// splitmix64-style finalizer (three xorshift/multiply rounds) over
/// `match_seed` folded with the round index, so every round of a match gets
/// an independent-looking but perfectly reproducible seed.
pub fn mix_seed(match_seed: u64, round: u32) -> u64 {
    /// Large odd constant (splitmix64's gamma, `2^64 / phi`) used to fold the
    /// round index into the seed before the finalizer avalanche.
    const ROUND_FOLD_CONSTANT: u64 = 0x9E37_79B9_7F4A_7C15;

    let mut z = match_seed ^ (round as u64).wrapping_mul(ROUND_FOLD_CONSTANT);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Determinism canary: these 4 values are the actual first 4 `next_u64()`
    // outputs of `Rng::new(1)`, computed once and frozen forever. If this
    // test ever fails, the xorshift64* implementation changed — which must
    // never happen silently, since it would desync every existing replay.
    #[test]
    fn rng_seed_1_first_four_values_are_frozen() {
        let mut rng = Rng::new(1);
        let v: [u64; 4] = [
            rng.next_u64(),
            rng.next_u64(),
            rng.next_u64(),
            rng.next_u64(),
        ];
        assert_eq!(
            v,
            [
                0x47e4ce4b896cdd1d,
                0xabcfa6a8e079651d,
                0xb9d10d8feb731f57,
                0x4db418a0bb1b019d,
            ]
        );
    }

    #[test]
    fn next_f32_is_in_zero_one_and_roughly_uniform() {
        let mut rng = Rng::new(42);
        let n = 10_000;
        let mut sum = 0.0_f64;
        for _ in 0..n {
            let f = rng.next_f32();
            assert!((0.0..1.0).contains(&f), "f={f}");
            sum += f as f64;
        }
        let mean = sum / n as f64;
        assert!((0.47..0.53).contains(&mean), "mean={mean}");
    }

    #[test]
    fn zero_seed_is_remapped_to_nonzero() {
        let mut rng = Rng::new(0);
        // If the state were truly 0, xorshift would stay 0 forever and
        // next_u64() would be 0 too (0 * anything == 0).
        assert_ne!(rng.next_u64(), 0);
    }

    #[test]
    fn mix_seed_differs_across_rounds() {
        let a = mix_seed(123, 0);
        let b = mix_seed(123, 1);
        assert_ne!(a, b);
    }

    #[test]
    fn range_i32_stays_in_bounds() {
        let mut rng = Rng::new(7);
        for _ in 0..1_000 {
            let v = rng.range_i32(-5, 5);
            assert!((-5..5).contains(&v), "v={v}");
        }
        // Degenerate ranges never panic.
        assert_eq!(rng.range_i32(5, 5), 5);
        assert_eq!(rng.range_i32(5, 2), 5);
    }
}
