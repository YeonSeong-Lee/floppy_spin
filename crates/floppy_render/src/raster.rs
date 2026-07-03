//! Edge-function triangle rasterization (SPEC §3/§10): incremental integer-
//! style stepping over a clamped bounding box, top-left fill rule so shared
//! edges between adjacent triangles are painted exactly once, one reciprocal
//! per triangle (no per-pixel division). Backface culling and near-plane
//! clipping happen upstream in `scene.rs`/`clip.rs` — this module only skips
//! triangles that are degenerate (zero or negative area) by the same
//! `signed_area` convention `scene.rs` uses to cull.
//!
//! Colors are interpolated linearly in screen space using plain (affine)
//! barycentric weights — NOT perspective-corrected. At this project's
//! triangle density/size (SPEC §10: hundreds of small triangles) the
//! artifact is invisible and the affine interpolation is exactly what keeps
//! the inner loop divide-free. `inv_z`, by contrast, IS exactly affine in
//! screen space for a perspective projection (see `frame.rs`), so
//! interpolating it with the same weights is exact, not an approximation.

use crate::frame::Frame;

/// Gouraud-shaded, already-projected vertex: `(x, y)` in screen pixels,
/// `inv_z` = `1/view_z` (see `frame.rs`/`camera.rs`), `r`/`g`/`b` in `0..1`.
#[derive(Clone, Copy, Debug)]
pub struct SVert {
    pub x: f32,
    pub y: f32,
    pub inv_z: f32,
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

/// Twice the signed area of triangle `(a, b, c)` under the convention this
/// whole renderer shares: **positive area = front-facing / keep**.
/// `scene.rs` culls backfaces with this exact function before a triangle
/// ever reaches `draw_tri`/`draw_tri_additive`; those functions re-check it
/// (skip if `<= 0`) as a degenerate-triangle safety net for any caller
/// (including tests) that builds `SVert`s directly.
pub fn signed_area(ax: f32, ay: f32, bx: f32, by: f32, cx: f32, cy: f32) -> f32 {
    (bx - ax) * (cy - ay) - (by - ay) * (cx - ax)
}

/// An edge direction is "top-left" if pixels sitting exactly ON that edge
/// belong to THIS triangle (inclusive `>= 0` test) rather than a
/// triangle-with-the-reversed-edge on the other side (exclusive `> 0`
/// test). `(dy < 0) || (dy == 0 && dx > 0)` partitions every nonzero
/// direction and its exact negation into opposite classes (proof: for `dy !=
/// 0` exactly one of `dy`/`-dy` is negative; for `dy == 0` exactly one of
/// `dx`/`-dx` is positive), so a shared edge walked in opposite directions
/// by its two owning triangles is always inclusive on exactly one side —
/// zero gaps, zero double-paints, independent of overall winding.
fn is_top_left(dx: f32, dy: f32) -> bool {
    (dy < 0.0) || (dy == 0.0 && dx > 0.0)
}

struct EdgeSetup {
    // Edge function value at the bbox's first pixel center, and its
    // per-x / per-y increments, for each of the 3 edges (w0 opposite a i.e.
    // edge b->c, w1 opposite b i.e. edge c->a, w2 opposite c i.e. edge a->b).
    w0_row: f32,
    w1_row: f32,
    w2_row: f32,
    dx0: f32,
    dx1: f32,
    dx2: f32,
    dy0: f32,
    dy1: f32,
    dy2: f32,
    incl0: bool,
    incl1: bool,
    incl2: bool,
    inv_area: f32,
}

/// Same formula/convention as [`signed_area`] (`edge_val(a, b, c) ==
/// signed_area(a, b, c)` exactly — `p` just plays the role `c` plays there),
/// so a positive `signed_area` triangle has all three of `w0`/`w1`/`w2`
/// positive at every interior point (the standard edge-function/barycentric
/// identity). Getting this consistent with `signed_area` matters: they used
/// to disagree by an overall sign, which meant every triangle that passed
/// the `area > 0` cull then failed all three inside tests and silently
/// painted nothing.
fn edge_val(ax: f32, ay: f32, bx: f32, by: f32, px: f32, py: f32) -> f32 {
    (bx - ax) * (py - ay) - (by - ay) * (px - ax)
}

/// Shared setup for both `draw_tri` and `draw_tri_additive`: bounding box
/// (clamped to the frame) plus incremental edge-function state. Returns
/// `None` for degenerate/backfacing triangles (`area <= 0`) or an
/// empty-after-clamp bbox.
#[allow(clippy::too_many_arguments)]
fn setup(
    frame: &Frame,
    a: SVert,
    b: SVert,
    c: SVert,
) -> Option<(usize, usize, usize, usize, EdgeSetup)> {
    let area = signed_area(a.x, a.y, b.x, b.y, c.x, c.y);
    if area <= 0.0 {
        return None;
    }

    let min_x = a.x.min(b.x).min(c.x);
    let max_x = a.x.max(b.x).max(c.x);
    let min_y = a.y.min(b.y).min(c.y);
    let max_y = a.y.max(b.y).max(c.y);

    let x0 = (min_x.floor().max(0.0)) as usize;
    let y0 = (min_y.floor().max(0.0)) as usize;
    let x1 = (max_x.ceil().max(0.0) as usize).min(frame.w);
    let y1 = (max_y.ceil().max(0.0) as usize).min(frame.h);
    if x0 >= x1 || y0 >= y1 {
        return None;
    }

    let px0 = x0 as f32 + 0.5;
    let py0 = y0 as f32 + 0.5;

    // Partial derivatives of `edge_val(X, Y, p)` w.r.t. `p.x`/`p.y`:
    // `d/dpx = -(Y.y - X.y)`, `d/dpy = (Y.x - X.x)` (see `edge_val` docs for
    // why the sign differs from the more commonly seen convention).
    let dx0 = b.y - c.y;
    let dy0 = c.x - b.x;
    let dx1 = c.y - a.y;
    let dy1 = a.x - c.x;
    let dx2 = a.y - b.y;
    let dy2 = b.x - a.x;

    let setup = EdgeSetup {
        w0_row: edge_val(b.x, b.y, c.x, c.y, px0, py0),
        w1_row: edge_val(c.x, c.y, a.x, a.y, px0, py0),
        w2_row: edge_val(a.x, a.y, b.x, b.y, px0, py0),
        dx0,
        dx1,
        dx2,
        dy0,
        dy1,
        dy2,
        incl0: is_top_left(c.x - b.x, c.y - b.y),
        incl1: is_top_left(a.x - c.x, a.y - c.y),
        incl2: is_top_left(b.x - a.x, b.y - a.y),
        inv_area: 1.0 / area,
    };

    Some((x0, y0, x1, y1, setup))
}

/// Rasterize a Gouraud triangle with a full depth test/write (`inv_z >
/// depth[i]` — strictly nearer wins). Skips (does nothing) if the triangle
/// is degenerate or back-facing per `signed_area` (`<= 0`).
pub fn draw_tri(frame: &mut Frame, a: SVert, b: SVert, c: SVert) {
    let Some((x0, y0, x1, y1, s)) = setup(frame, a, b, c) else {
        return;
    };

    let mut w0_row = s.w0_row;
    let mut w1_row = s.w1_row;
    let mut w2_row = s.w2_row;

    for y in y0..y1 {
        let mut w0 = w0_row;
        let mut w1 = w1_row;
        let mut w2 = w2_row;
        for x in x0..x1 {
            let in0 = if s.incl0 { w0 >= 0.0 } else { w0 > 0.0 };
            let in1 = if s.incl1 { w1 >= 0.0 } else { w1 > 0.0 };
            let in2 = if s.incl2 { w2 >= 0.0 } else { w2 > 0.0 };
            if in0 && in1 && in2 {
                let l0 = w0 * s.inv_area;
                let l1 = w1 * s.inv_area;
                let l2 = w2 * s.inv_area;
                let idx = y * frame.w + x;
                let inv_z = l0 * a.inv_z + l1 * b.inv_z + l2 * c.inv_z;
                if inv_z > frame.depth[idx] {
                    let r = l0 * a.r + l1 * b.r + l2 * c.r;
                    let g = l0 * a.g + l1 * b.g + l2 * c.g;
                    let bl = l0 * a.b + l1 * b.b + l2 * c.b;
                    frame.px[idx] = pack(r, g, bl);
                    frame.depth[idx] = inv_z;
                }
            }
            w0 += s.dx0;
            w1 += s.dx1;
            w2 += s.dx2;
        }
        w0_row += s.dy0;
        w1_row += s.dy1;
        w2_row += s.dy2;
    }
}

/// Same coverage/fill-rule/depth-TEST as `draw_tri`, but for additive
/// glow/VFX passes: colors ADD (saturating) onto the existing pixel instead
/// of replacing it, and depth is never written (so overlapping glow
/// triangles all contribute, and later opaque geometry can still draw over
/// or behind them correctly). Depth-tested against the CURRENT buffer
/// (`inv_z > depth[i]`, same comparison as `draw_tri`) — deliberately not
/// "no test at all", so a glow triangle behind solid geometry doesn't bleed
/// through it; deliberately not "reject on equal" either, so multiple
/// additive passes at the exact same depth (e.g. two glow triangles sharing
/// an edge) all still contribute rather than only the first.
pub fn draw_tri_additive(frame: &mut Frame, a: SVert, b: SVert, c: SVert) {
    let Some((x0, y0, x1, y1, s)) = setup(frame, a, b, c) else {
        return;
    };

    let mut w0_row = s.w0_row;
    let mut w1_row = s.w1_row;
    let mut w2_row = s.w2_row;

    for y in y0..y1 {
        let mut w0 = w0_row;
        let mut w1 = w1_row;
        let mut w2 = w2_row;
        for x in x0..x1 {
            let in0 = if s.incl0 { w0 >= 0.0 } else { w0 > 0.0 };
            let in1 = if s.incl1 { w1 >= 0.0 } else { w1 > 0.0 };
            let in2 = if s.incl2 { w2 >= 0.0 } else { w2 > 0.0 };
            if in0 && in1 && in2 {
                let l0 = w0 * s.inv_area;
                let l1 = w1 * s.inv_area;
                let l2 = w2 * s.inv_area;
                let idx = y * frame.w + x;
                let inv_z = l0 * a.inv_z + l1 * b.inv_z + l2 * c.inv_z;
                if inv_z > frame.depth[idx] {
                    let r = l0 * a.r + l1 * b.r + l2 * c.r;
                    let g = l0 * a.g + l1 * b.g + l2 * c.g;
                    let bl = l0 * a.b + l1 * b.b + l2 * c.b;
                    frame.px[idx] = add_saturating(frame.px[idx], pack(r, g, bl));
                }
            }
            w0 += s.dx0;
            w1 += s.dx1;
            w2 += s.dx2;
        }
        w0_row += s.dy0;
        w1_row += s.dy1;
        w2_row += s.dy2;
    }
}

fn pack(r: f32, g: f32, b: f32) -> u32 {
    let r = (r.clamp(0.0, 1.0) * 255.0) as u32;
    let g = (g.clamp(0.0, 1.0) * 255.0) as u32;
    let b = (b.clamp(0.0, 1.0) * 255.0) as u32;
    (r << 16) | (g << 8) | b
}

fn add_saturating(dst: u32, src: u32) -> u32 {
    let dr = (dst >> 16) & 0xFF;
    let dg = (dst >> 8) & 0xFF;
    let db = dst & 0xFF;
    let sr = (src >> 16) & 0xFF;
    let sg = (src >> 8) & 0xFF;
    let sb = src & 0xFF;
    let r = (dr + sr).min(255);
    let g = (dg + sg).min(255);
    let b = (db + sb).min(255);
    (r << 16) | (g << 8) | b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vert(x: f32, y: f32, inv_z: f32, r: f32, g: f32, b: f32) -> SVert {
        SVert {
            x,
            y,
            inv_z,
            r,
            g,
            b,
        }
    }

    /// A 64x64 quad (from (0,0) to (64,64)) split along the diagonal into
    /// two triangles, both wound so `signed_area > 0` (this renderer's
    /// front-facing convention). Painting both additively at full-white with
    /// depth 1.0 must leave every pixel in the quad at EXACTLY white (0xFF
    /// per channel from ONE paint) and every pixel outside untouched —
    /// double-painting would saturate-clip identically to single-painting on
    /// a solid color, so this test instead paints at a low, exactly
    /// doubling-detectable value and checks the additive sum precisely.
    #[test]
    fn fill_rule_covers_every_pixel_exactly_once() {
        let mut f = Frame::new(64, 64);
        // Value chosen so 1x paint != 2x paint after u8 packing (2*50=100).
        let v = 50.0 / 255.0;

        // signed_area(a,b,c) = (b-a) x (c-a); pick orientation so both are
        // positive.
        let t1 = [
            vert(0.0, 0.0, 1.0, v, v, v),
            vert(64.0, 0.0, 1.0, v, v, v),
            vert(64.0, 64.0, 1.0, v, v, v),
        ];
        let t2 = [
            vert(0.0, 0.0, 1.0, v, v, v),
            vert(64.0, 64.0, 1.0, v, v, v),
            vert(0.0, 64.0, 1.0, v, v, v),
        ];
        assert!(signed_area(t1[0].x, t1[0].y, t1[1].x, t1[1].y, t1[2].x, t1[2].y) > 0.0);
        assert!(signed_area(t2[0].x, t2[0].y, t2[1].x, t2[1].y, t2[2].x, t2[2].y) > 0.0);

        draw_tri_additive(&mut f, t1[0], t1[1], t1[2]);
        draw_tri_additive(&mut f, t2[0], t2[1], t2[2]);

        // Exact equality would be fragile against last-bit float rounding in
        // the barycentric weights (which can nudge a truncating f32->u8
        // pack by +-1 even for a mathematically-exact single paint); a gap
        // (0) or a double-paint (~100) are each unmistakably far outside
        // this tolerance, so it still fully validates the fill rule.
        for y in 0..64 {
            for x in 0..64 {
                let p = f.px[y * 64 + x];
                let r = (p >> 16) & 0xFF;
                assert!(
                    (48..=52).contains(&r),
                    "gap or double-paint at ({x},{y}): r={r}"
                );
            }
        }
        // Nothing painted outside the quad.
        let mut f2 = Frame::new(66, 66);
        draw_tri_additive(&mut f2, t1[0], t1[1], t1[2]);
        draw_tri_additive(&mut f2, t2[0], t2[1], t2[2]);
        for y in 64..66 {
            for x in 0..66 {
                assert_eq!(f2.px[y * 66 + x], 0);
            }
        }
        for y in 0..66 {
            for x in 64..66 {
                assert_eq!(f2.px[y * 66 + x], 0);
            }
        }
    }

    #[test]
    fn depth_test_near_wins_regardless_of_draw_order() {
        let far = [
            vert(10.0, 10.0, 0.1, 1.0, 0.0, 0.0),
            vert(50.0, 10.0, 0.1, 1.0, 0.0, 0.0),
            vert(30.0, 50.0, 0.1, 1.0, 0.0, 0.0),
        ];
        let near = [
            vert(10.0, 10.0, 1.0, 0.0, 1.0, 0.0),
            vert(50.0, 10.0, 1.0, 0.0, 1.0, 0.0),
            vert(30.0, 50.0, 1.0, 0.0, 1.0, 0.0),
        ];
        assert!(signed_area(far[0].x, far[0].y, far[1].x, far[1].y, far[2].x, far[2].y) > 0.0);

        // near drawn after far: near wins.
        let mut f1 = Frame::new(64, 64);
        draw_tri(&mut f1, far[0], far[1], far[2]);
        draw_tri(&mut f1, near[0], near[1], near[2]);
        let p1 = f1.px[30 * 64 + 30];
        assert_eq!((p1 >> 8) & 0xFF, 255, "green channel should win at p1");

        // near drawn before far: near still wins.
        let mut f2 = Frame::new(64, 64);
        draw_tri(&mut f2, near[0], near[1], near[2]);
        draw_tri(&mut f2, far[0], far[1], far[2]);
        let p2 = f2.px[30 * 64 + 30];
        assert_eq!((p2 >> 8) & 0xFF, 255, "green channel should win at p2");
    }

    #[test]
    fn degenerate_or_backfacing_triangle_paints_nothing() {
        let mut f = Frame::new(32, 32);
        // Zero area.
        let a = vert(5.0, 5.0, 1.0, 1.0, 1.0, 1.0);
        let b = vert(5.0, 5.0, 1.0, 1.0, 1.0, 1.0);
        let c = vert(5.0, 5.0, 1.0, 1.0, 1.0, 1.0);
        draw_tri(&mut f, a, b, c);
        assert!(f.px.iter().all(|&p| p == 0));

        // Reversed winding (negative area).
        let a = vert(0.0, 0.0, 1.0, 1.0, 1.0, 1.0);
        let b = vert(0.0, 20.0, 1.0, 1.0, 1.0, 1.0);
        let c = vert(20.0, 0.0, 1.0, 1.0, 1.0, 1.0);
        assert!(signed_area(a.x, a.y, b.x, b.y, c.x, c.y) < 0.0);
        draw_tri(&mut f, a, b, c);
        assert!(f.px.iter().all(|&p| p == 0));
    }

    #[test]
    fn additive_saturates_and_respects_existing_depth() {
        let mut f = Frame::new(16, 16);
        f.px[5 * 16 + 5] = 0x00FF0000;
        f.depth[5 * 16 + 5] = 1.0;
        let tri = [
            vert(0.0, 0.0, 1.0, 1.0, 0.0, 0.0),
            vert(16.0, 0.0, 1.0, 1.0, 0.0, 0.0),
            vert(0.0, 16.0, 1.0, 1.0, 0.0, 0.0),
        ];
        assert!(signed_area(tri[0].x, tri[0].y, tri[1].x, tri[1].y, tri[2].x, tri[2].y) > 0.0);
        draw_tri_additive(&mut f, tri[0], tri[1], tri[2]);
        let p = f.px[5 * 16 + 5];
        assert_eq!((p >> 16) & 0xFF, 255, "saturating add must clamp at 255");

        // A pixel that's strictly farther than existing depth must not be
        // touched by the additive pass.
        let mut f2 = Frame::new(16, 16);
        f2.depth[5 * 16 + 5] = 2.0; // something nearer already there
        f2.px[5 * 16 + 5] = 0x00112233;
        draw_tri_additive(&mut f2, tri[0], tri[1], tri[2]);
        assert_eq!(f2.px[5 * 16 + 5], 0x00112233);
    }
}
