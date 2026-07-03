//! Single-plane near clipping (Sutherland-Hodgman), one triangle at a time,
//! no allocation. `scene.rs` calls this against the camera's near plane
//! (view-space `z == Camera::NEAR`) before projecting, so triangles that
//! straddle the camera never divide-by-near-zero in `project`.

use floppy_core::vec::Vec3;

/// Clip one triangle — `[(position, color); 3]` in the SAME space (view
/// space in practice), `color` carried as a plain `Vec3` so it interpolates
/// with the same `t` as position — against the plane `z == near`, keeping
/// the side `z >= near`.
///
/// Returns up to 2 triangles in a fixed-size `[[_; 3]; 2]` plus a count `0`,
/// `1`, or `2` (never allocates): 3-in -> `(unchanged tri, 1)`, 2-in ->
/// `(quad split into 2 tris, 2)`, 1-in -> `(1 tri, 1)`, 0-in -> `(_, 0)`.
pub fn clip_near(tri: [(Vec3, Vec3); 3], near: f32) -> ([[(Vec3, Vec3); 3]; 2], u8) {
    let inside = |p: Vec3| p.z >= near;

    // Sutherland-Hodgman edge walk against a single plane: a triangle
    // clipped by one plane produces a polygon of at most 4 vertices.
    let mut poly: [(Vec3, Vec3); 4] = [tri[0]; 4];
    let mut n = 0usize;

    for i in 0..3 {
        let cur = tri[i];
        let next = tri[(i + 1) % 3];
        let cur_in = inside(cur.0);
        let next_in = inside(next.0);

        if cur_in {
            poly[n] = cur;
            n += 1;
        }
        if cur_in != next_in {
            let dz = next.0.z - cur.0.z;
            let t = if dz.abs() > 1e-8 {
                (near - cur.0.z) / dz
            } else {
                0.0
            };
            let pos = cur.0.lerp(next.0, t);
            let col = cur.1.lerp(next.1, t);
            poly[n] = (pos, col);
            n += 1;
        }
    }

    let mut out = [[tri[0]; 3]; 2];
    let count = match n {
        3 => {
            out[0] = [poly[0], poly[1], poly[2]];
            1
        }
        4 => {
            out[0] = [poly[0], poly[1], poly[2]];
            out[1] = [poly[0], poly[2], poly[3]];
            2
        }
        _ => 0,
    };
    (out, count)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f32, y: f32, z: f32) -> Vec3 {
        Vec3::new(x, y, z)
    }

    #[test]
    fn all_three_in_front_returns_unchanged_triangle() {
        let tri = [
            (v(0.0, 0.0, 1.0), v(1.0, 0.0, 0.0)),
            (v(1.0, 0.0, 1.0), v(0.0, 1.0, 0.0)),
            (v(0.0, 1.0, 1.0), v(0.0, 0.0, 1.0)),
        ];
        let (out, count) = clip_near(tri, 0.5);
        assert_eq!(count, 1);
        assert_eq!(out[0], tri);
    }

    #[test]
    fn all_three_behind_returns_zero_triangles() {
        let tri = [
            (v(0.0, 0.0, 0.1), v(1.0, 0.0, 0.0)),
            (v(1.0, 0.0, 0.2), v(0.0, 1.0, 0.0)),
            (v(0.0, 1.0, 0.3), v(0.0, 0.0, 1.0)),
        ];
        let (_out, count) = clip_near(tri, 0.5);
        assert_eq!(count, 0);
    }

    #[test]
    fn two_in_one_out_produces_two_triangles_with_correct_t() {
        // v0, v1 in front (z=1.0), v2 behind (z=0.0), near=0.5 -> t=0.5 on
        // both edges touching v2.
        let tri = [
            (v(0.0, 0.0, 1.0), v(2.0, 0.0, 0.0)),
            (v(1.0, 0.0, 1.0), v(0.0, 2.0, 0.0)),
            (v(0.0, 1.0, 0.0), v(0.0, 0.0, 2.0)),
        ];
        let (out, count) = clip_near(tri, 0.5);
        assert_eq!(count, 2);
        // First new vertex: intersection of edge v1->v2 at t=0.5.
        let p_a = out[0][2];
        assert!((p_a.0.z - 0.5).abs() < 1e-6);
        assert!((p_a.0.x - 0.5).abs() < 1e-6, "x={}", p_a.0.x);
        assert!((p_a.0.y - 0.5).abs() < 1e-6, "y={}", p_a.0.y);
        assert!((p_a.1.x - 0.0).abs() < 1e-6);
        assert!((p_a.1.y - 1.0).abs() < 1e-6);
    }

    #[test]
    fn one_in_two_out_produces_one_triangle_with_correct_t() {
        // v0 in front (z=1.0), v1,v2 behind (z=0,z=0), near=0.5.
        let tri = [
            (v(0.0, 0.0, 1.0), v(2.0, 0.0, 0.0)),
            (v(2.0, 0.0, 0.0), v(0.0, 2.0, 0.0)),
            (v(0.0, 2.0, 0.0), v(0.0, 0.0, 2.0)),
        ];
        let (out, count) = clip_near(tri, 0.5);
        assert_eq!(count, 1);
        // out[0] = [v0, isect(v0->v1), isect(v2->v0)]
        assert_eq!(out[0][0], tri[0]);
        let isect01 = out[0][1];
        // t on v0->v1: near=0.5, v0.z=1, v1.z=0 -> t = (0.5-1)/(0-1) = 0.5
        assert!((isect01.0.z - 0.5).abs() < 1e-6);
        assert!((isect01.0.x - 1.0).abs() < 1e-6, "x={}", isect01.0.x);
        let isect20 = out[0][2];
        // t on v2->v0: v2.z=0, v0.z=1 -> t = (0.5-0)/(1-0) = 0.5
        assert!((isect20.0.z - 0.5).abs() < 1e-6);
    }
}
