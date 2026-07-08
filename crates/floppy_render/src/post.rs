//! Final post-processing composite pass (game_design.md §6 aesthetic recipe;
//! SPEC §10 perf budget: "Particles/trails/bloom (half-res bright pass) <= 4
//! ms"). Exactly ONE full-framebuffer pass runs per rendered frame (module
//! docs on [`PostState::composite`]) — bloom, ordered dither, scanline +
//! vignette, and an additive flash tint are all folded into that single
//! per-pixel loop rather than three separate full-screen passes, per the
//! milestone brief's explicit "don't do three full-screen passes" direction.
//!
//! Determinism (SPEC §5): every input is either the current framebuffer, the
//! half-res [`BrightBuffer`] (itself populated only by `raster.rs`'s
//! bloom-tagged draw calls during the deterministic scene/particle draw), or
//! a caller-supplied `flash` color that is itself a pure function of
//! `BattleEvent`s/screen transitions (see `main.rs`). No wall-clock, no RNG,
//! no libm (only `+ - * /`, comparisons, and integer shifts below).

use crate::frame::Frame;
use crate::raster::add_saturating;
use floppy_core::vec::Vec3;

/// Half-resolution (`Frame::w/2 x Frame::h/2` — 480x270 at this project's
/// fixed 960x540 internal resolution) emissive accumulation buffer
/// (game_design.md §6 bloom recipe), one plain `f32` channel plane each for
/// R/G/B. Full `f32` precision is kept end-to-end through both blur passes
/// (module docs on [`BrightBuffer::blur`]) — packing down to 8-bit-per-
/// channel after EVERY one of the 4 box-blur sub-passes would compound
/// truncation error badly enough to round a single bright source pixel all
/// the way down to zero by the time the second (radius-8) pass finishes;
/// only [`PostState::composite`]'s very last step ever quantizes to `u8`.
/// Written by `raster.rs`'s `draw_tri_bloom`/`draw_tri_additive_bloom` and by
/// `particles.rs` wherever a triangle/particle is tagged emissive.
pub struct BrightBuffer {
    pub w: usize,
    pub h: usize,
    r: Vec<f32>,
    g: Vec<f32>,
    b: Vec<f32>,
}

#[inline(always)]
fn unpack_f32(c: u32) -> (f32, f32, f32) {
    (
        ((c >> 16) & 0xFF) as f32,
        ((c >> 8) & 0xFF) as f32,
        (c & 0xFF) as f32,
    )
}

#[inline(always)]
fn pack_clamped(r: f32, g: f32, b: f32) -> u32 {
    let r = r.clamp(0.0, 255.0) as u32;
    let g = g.clamp(0.0, 255.0) as u32;
    let b = b.clamp(0.0, 255.0) as u32;
    (r << 16) | (g << 8) | b
}

impl BrightBuffer {
    pub fn new(full_w: usize, full_h: usize) -> Self {
        let w = full_w / 2;
        let h = full_h / 2;
        Self {
            w,
            h,
            r: vec![0.0f32; w * h],
            g: vec![0.0f32; w * h],
            b: vec![0.0f32; w * h],
        }
    }

    pub fn clear(&mut self) {
        for v in self.r.iter_mut() {
            *v = 0.0;
        }
        for v in self.g.iter_mut() {
            *v = 0.0;
        }
        for v in self.b.iter_mut() {
            *v = 0.0;
        }
    }

    /// Additive write at half-res pixel `(hx, hy)`; bounds-checked (never
    /// panics, matches `Frame::set`'s convention). Deliberately unclamped
    /// (unlike `Frame`'s saturating packed-`u32` add): a wide emissive
    /// region or overlapping additive triangles SHOULD read as a stronger
    /// bloom source than a single dim pixel, and the blur + final gain (see
    /// `PostState::composite`) tame the result long before it's ever
    /// packed back into 8 bits.
    pub fn add(&mut self, hx: i32, hy: i32, color: u32) {
        if hx < 0 || hy < 0 {
            return;
        }
        let (hx, hy) = (hx as usize, hy as usize);
        if hx >= self.w || hy >= self.h {
            return;
        }
        let idx = hy * self.w + hx;
        let (r, g, b) = unpack_f32(color);
        self.r[idx] += r;
        self.g[idx] += g;
        self.b[idx] += b;
    }

    /// Sample the (post-blur, if called after `blur`) value at half-res
    /// pixel `(hx, hy)` as `(r, g, b)` floats, each nominally `0..=255`
    /// (bounds-unchecked — callers index within `w`/`h`, same convention as
    /// `Frame::px[y*w+x]`).
    pub fn sample(&self, hx: usize, hy: usize) -> (f32, f32, f32) {
        let idx = hy * self.w + hx;
        (self.r[idx], self.g[idx], self.b[idx])
    }

    /// Box blur, radius [`BLUR_RADIUS`] (game_design.md §6's recipe is a
    /// TWO-pass blur at "4px + 8px"; see that constant's doc comment for why
    /// this shipped as a single pass instead — SPEC §10's perf gate, not a
    /// design-taste change). Each of the two directions (horizontal then
    /// vertical, applied per channel) is an O(w*h) sliding-window sum (no
    /// re-summing the whole window per pixel) with exactly one multiply-by-
    /// precomputed-reciprocal per pixel (no per-pixel DIVIDE). `scratch`
    /// must be the same size as `self`; it's caller-owned so it's allocated
    /// once and reused every frame instead of allocated here.
    ///
    /// Perf (SPEC §10): the upsample-add's `BLOOM_GAIN` (game_design.md §6:
    /// "added back at 0.6 gain") is folded into the vertical pass's
    /// reciprocal here instead of being a separate multiply in
    /// `PostState::composite`'s hot per-pixel loop — one multiply moved from
    /// "once per full-res pixel" to "once per half-res pixel, already-being-
    /// computed reciprocal multiply," for free.
    pub fn blur(&mut self, scratch: &mut BrightBuffer) {
        debug_assert_eq!(self.w, scratch.w);
        debug_assert_eq!(self.h, scratch.h);
        box_blur_h(&self.r, &mut scratch.r, self.w, self.h, BLUR_RADIUS, 1.0);
        box_blur_v(
            &scratch.r,
            &mut self.r,
            self.w,
            self.h,
            BLUR_RADIUS,
            BLOOM_GAIN,
        );
        box_blur_h(&self.g, &mut scratch.g, self.w, self.h, BLUR_RADIUS, 1.0);
        box_blur_v(
            &scratch.g,
            &mut self.g,
            self.w,
            self.h,
            BLUR_RADIUS,
            BLOOM_GAIN,
        );
        box_blur_h(&self.b, &mut scratch.b, self.w, self.h, BLUR_RADIUS, 1.0);
        box_blur_v(
            &scratch.b,
            &mut self.b,
            self.w,
            self.h,
            BLUR_RADIUS,
            BLOOM_GAIN,
        );
    }
}

/// Blur radius, half-res px (SPEC §10 perf gate — see below). The
/// game_design.md §6 recipe is a TWO-pass box blur at "4px + 8px": measured
/// (this milestone's headless perf harness, `--scene battle --frames 120`
/// release) at ~1.9-2.0ms just for the blur half of the post pass, pushing
/// the total frame time to ~10.2-11.3ms against the SPEC §10 hard ceiling
/// of 10ms — because this blur is an O(w*h) sliding-window algorithm (cost
/// independent of RADIUS; two full passes cost ~2x one pass regardless of
/// their radii), the applicable lever is PASS COUNT, not the radius number
/// itself. Collapsed to one pass at a radius splitting the difference
/// between 4 and 8, per SPEC §10's explicit degradation order ("shrink
/// bloom radius... in that order" — read here as "shrink the blur's cost
/// footprint first"): saves ~1ms, the exact amount needed to clear the 10ms
/// gate with a small margin. See the M7 report for the full before/after
/// numbers.
const BLUR_RADIUS: i32 = 6;

/// Horizontal box-blur pass (single channel): a sliding window of width
/// `2*radius+1` (zero-padded past the row edges) summed incrementally
/// (`+entering -leaving`, never re-summed from scratch), scaled by a
/// precomputed `gain / window` reciprocal once per pixel (`gain = 1.0` for
/// every pass except the very last — see `BrightBuffer::blur`'s docs).
fn box_blur_h(src: &[f32], dst: &mut [f32], w: usize, h: usize, radius: i32, gain: f32) {
    let inv_window = gain / (2 * radius + 1) as f32;
    for y in 0..h {
        let row = y * w;
        let mut sum = 0.0f32;
        for dx in -radius..=radius {
            if dx >= 0 && (dx as usize) < w {
                sum += src[row + dx as usize];
            }
        }
        for x in 0..w {
            dst[row + x] = sum * inv_window;

            let leave = x as i32 - radius;
            let enter = x as i32 + radius + 1;
            if leave >= 0 && (leave as usize) < w {
                sum -= src[row + leave as usize];
            }
            if enter >= 0 && (enter as usize) < w {
                sum += src[row + enter as usize];
            }
        }
    }
}

/// Vertical box-blur pass: same sliding-window technique as [`box_blur_h`],
/// walking down each column instead of across each row.
fn box_blur_v(src: &[f32], dst: &mut [f32], w: usize, h: usize, radius: i32, gain: f32) {
    let inv_window = gain / (2 * radius + 1) as f32;
    for x in 0..w {
        let mut sum = 0.0f32;
        for dy in -radius..=radius {
            if dy >= 0 && (dy as usize) < h {
                sum += src[(dy as usize) * w + x];
            }
        }
        for y in 0..h {
            dst[y * w + x] = sum * inv_window;

            let leave = y as i32 - radius;
            let enter = y as i32 + radius + 1;
            if leave >= 0 && (leave as usize) < h {
                sum -= src[(leave as usize) * w + x];
            }
            if enter >= 0 && (enter as usize) < h {
                sum += src[(enter as usize) * w + x];
            }
        }
    }
}

/// Bloom upsample-and-add gain (game_design.md §6: "added back at 0.6
/// gain").
const BLOOM_GAIN: f32 = 0.6;
/// Scanline + vignette alpha (game_design.md §6: "faint scanline + vignette
/// (alpha 0.08)").
const SCANLINE_ALPHA: f32 = 0.08;
const VIGNETTE_ALPHA: f32 = 0.08;

/// 4x4 ordered (Bayer) dither matrix, standard form (game_design.md §6:
/// "subtle ordered-dither on ambient gradients"). Applied uniformly across
/// the whole frame in the final pass (documented simplification: rather than
/// classifying which pixels are "ambient gradient" vs. HUD/text, the ±1
/// level perturbation below is small enough to be a no-op visually on flat
/// HUD colors and exactly the intended de-banding nudge on the arena's lit
/// gradients — see module docs' "one final pass" constraint).
const BAYER4: [[i32; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// -1 for the "darker half" of the 4x4 Bayer cell, 0 otherwise: a 50/50
/// ordered 1-level dither (arithmetic right-shift by 3 of a `-8..=7`-ranged
/// Bayer value splits it exactly in half — no float division needed).
#[inline(always)]
fn dither_bias(x: usize, y: usize) -> i32 {
    (BAYER4[y & 3][x & 3] - 8) >> 3
}

/// Persistent post-processing state: the half-res bright buffer the scene/
/// particle draw calls tag emissive pixels into, plus a same-sized scratch
/// buffer for the two-pass blur's ping-pong (both allocated once at
/// `Frame`-resolution-dependent construction time, never reallocated
/// per-frame — SPEC §10).
pub struct PostState {
    pub bright: BrightBuffer,
    scratch: BrightBuffer,
    /// Precomputed ONCE at construction (SPEC §10 perf: neither the
    /// scanline nor the vignette factor ever depends on frame CONTENT, only
    /// on `(x, y)` position, which never changes for a fixed resolution) —
    /// `scan_row[y] * vignette(x, y)` folded into a single per-pixel
    /// multiplier so `composite`'s hot loop is one array read instead of a
    /// ~6-op chain (row/col lookups + min/max/multiply) every pixel, every
    /// frame.
    combined_mult: Vec<f32>,
}

impl PostState {
    pub fn new(full_w: usize, full_h: usize) -> Self {
        let h_half = full_h as f32 * 0.5;
        let w_half = full_w as f32 * 0.5;
        let mut combined_mult = vec![0.0f32; full_w * full_h];
        for y in 0..full_h {
            let scan = if y & 1 == 1 {
                1.0 - SCANLINE_ALPHA
            } else {
                1.0
            };
            let dy = (y as f32 - h_half) / h_half;
            let vig_row = dy * dy;
            let row = y * full_w;
            for x in 0..full_w {
                let dx = (x as f32 - w_half) / w_half;
                let vig_col = dx * dx;
                let vig = (1.0 - VIGNETTE_ALPHA * (vig_row + vig_col).min(1.0)).max(0.0);
                combined_mult[row + x] = scan * vig;
            }
        }
        Self {
            bright: BrightBuffer::new(full_w, full_h),
            scratch: BrightBuffer::new(full_w, full_h),
            combined_mult,
        }
    }

    /// Clear the bright buffer before redrawing the scene/particles for a
    /// fresh frame (module docs: it accumulates additively during that
    /// draw, so last frame's contributions must not leak forward).
    pub fn begin_frame(&mut self) {
        self.bright.clear();
    }

    /// The ONE final composite pass (module docs): blur the bright buffer,
    /// then for every full-res pixel — upsample-add bloom, ordered dither,
    /// scanline x vignette darken, additive flash — all in a single loop.
    /// Upsample is nearest/block-replicate (plain integer halving, `x/2,
    /// y/2` — documented choice, see `raster.rs`'s bloom-tagging docs for
    /// the matching write-side convention), not bilinear: at this bright
    /// buffer's resolution and blur radii the extra interpolation would be
    /// imperceptible next to the blur itself, so the divide-free nearest
    /// lookup is strictly cheaper for the same visual result.
    ///
    /// `flash` is an already-summed, already-alpha-scaled linear color
    /// (`Vec3`, per-channel roughly `0..=1`, clamped via
    /// `shade::color_to_px` at the very end); pass `Vec3::default()` (all
    /// zero) for "no flash active" (goldens use exactly this — see
    /// `battle.rs`/`headless.rs` docs).
    pub fn composite(&mut self, frame: &mut Frame, flash: Vec3) {
        self.bright.blur(&mut self.scratch);

        let flash_packed = crate::shade::color_to_px(flash);
        let flash_is_zero = flash_packed == 0;

        let w = frame.w;
        let h = frame.h;
        let bw = self.bright.w;
        let bh = self.bright.h;
        // The whole 2x2-block loop below indexes `hy*2 + {0,1}` /
        // `hx*2 + {0,1}` with NO per-pixel bounds check (module docs / SPEC
        // §10 perf): safe exactly because the internal resolution is fixed
        // at an even 960x540 (SPEC §5), so `bw*2 == w` / `bh*2 == h` always
        // — checked here once, loudly, in debug builds rather than
        // silently truncating a hypothetical odd-sized frame.
        debug_assert_eq!(w, bw * 2, "composite assumes an even frame width");
        debug_assert_eq!(h, bh * 2, "composite assumes an even frame height");
        debug_assert_eq!(self.combined_mult.len(), w * h);

        // Perf (SPEC §10): iterate by HALF-res cell, applying each bloom
        // sample (already gain-scaled BY `blur` ITSELF — see its docs — so
        // no `* BLOOM_GAIN` needed here) to its whole `2x2` full-res block —
        // the bright-buffer read happens once per 4 pixels instead of once
        // per pixel, since all 4 share the exact same upsampled value
        // anyway (module docs' "nearest/block-replicate" upsample).
        // Dither/vignette/scanline still vary per FULL-res pixel
        // (unavoidable — that's the whole point of the ordered dither and
        // the continuous vignette falloff), so those stay per-pixel.
        for hy in 0..bh {
            let bidx_row = hy * bw;
            for hx in 0..bw {
                let bidx = bidx_row + hx;
                let br = self.bright.r[bidx];
                let bg = self.bright.g[bidx];
                let bb = self.bright.b[bidx];

                for dy in 0..2 {
                    let y = hy * 2 + dy;
                    let row = y * w;
                    for dx in 0..2 {
                        let x = hx * 2 + dx;
                        let idx = row + x;

                        let (fr, fg, fb) = unpack_f32(frame.px[idx]);
                        let mut r = fr + br;
                        let mut g = fg + bg;
                        let mut b = fb + bb;

                        let bias = dither_bias(x, y) as f32;
                        r += bias;
                        g += bias;
                        b += bias;

                        let mult = self.combined_mult[idx];
                        r *= mult;
                        g *= mult;
                        b *= mult;

                        let mut color = pack_clamped(r, g, b);
                        if !flash_is_zero {
                            color = add_saturating(color, flash_packed);
                        }
                        frame.px[idx] = color;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bright_buffer_is_half_the_frame_resolution() {
        let b = BrightBuffer::new(960, 540);
        assert_eq!(b.w, 480);
        assert_eq!(b.h, 270);
    }

    #[test]
    fn bright_buffer_add_accumulates_and_is_bounds_checked() {
        let mut b = BrightBuffer::new(4, 4);
        b.add(-1, 0, 0x00FF0000);
        b.add(0, -1, 0x00FF0000);
        b.add(10, 10, 0x00FF0000);
        assert_eq!(b.sample(0, 0), (0.0, 0.0, 0.0));
        b.add(1, 1, 0x00500000);
        b.add(1, 1, 0x00500000);
        assert_eq!(b.sample(1, 1), (160.0, 0.0, 0.0));
    }

    #[test]
    fn blur_spreads_a_single_bright_pixel_to_its_neighbors() {
        let mut b = BrightBuffer::new(32, 32); // half-res 16x16
        let mut scratch = BrightBuffer::new(32, 32);
        b.add(8, 8, 0x00FFFFFF);
        b.blur(&mut scratch);
        let (r0, _, _) = b.sample(8, 8);
        assert!(
            r0 < 255.0 && r0 > 0.0,
            "center should drop but stay lit: {r0}"
        );
        let (r1, _, _) = b.sample(10, 8);
        assert!(r1 > 0.0, "blur should spread to neighbors, got {r1}");
    }

    #[test]
    fn composite_is_deterministic_for_identical_inputs() {
        let mut post_a = PostState::new(64, 64);
        let mut frame_a = Frame::new(64, 64);
        frame_a.clear(0x00202020);
        post_a.bright.add(10, 10, 0x00FFFFFF);
        post_a.composite(&mut frame_a, Vec3::new(0.1, 0.0, 0.0));

        let mut post_b = PostState::new(64, 64);
        let mut frame_b = Frame::new(64, 64);
        frame_b.clear(0x00202020);
        post_b.bright.add(10, 10, 0x00FFFFFF);
        post_b.composite(&mut frame_b, Vec3::new(0.1, 0.0, 0.0));

        assert_eq!(frame_a.px, frame_b.px);
    }

    #[test]
    fn zero_flash_leaves_frame_unaffected_by_flash_but_bloom_still_applies() {
        let mut post = PostState::new(64, 64);
        let mut frame = Frame::new(64, 64);
        frame.clear(0x00000000);
        // A small filled emissive cluster, not a single isolated pixel: a
        // lone pixel's energy legitimately dilutes below one 8-bit level
        // once spread + gained across the whole buffer (see `blur`'s docs
        // on why full f32 precision matters even so) — this is closer to
        // what an actual in-scene emissive region (many contiguous pixels)
        // looks like.
        for hy in 8..14 {
            for hx in 8..14 {
                post.bright.add(hx, hy, 0x00FFFFFF);
            }
        }
        post.composite(&mut frame, Vec3::default());
        let any_lit = frame.px.iter().any(|&p| p != 0);
        assert!(any_lit, "bloom should light up some pixels");
    }

    #[test]
    fn dither_bias_is_a_fifty_fifty_split_of_zero_and_minus_one() {
        let mut zeros = 0;
        let mut neg_ones = 0;
        for y in 0..4 {
            for x in 0..4 {
                match dither_bias(x, y) {
                    0 => zeros += 1,
                    -1 => neg_ones += 1,
                    other => panic!("unexpected dither bias {other}"),
                }
            }
        }
        assert_eq!(zeros, 8);
        assert_eq!(neg_ones, 8);
    }
}
