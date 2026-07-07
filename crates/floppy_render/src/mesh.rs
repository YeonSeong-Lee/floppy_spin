//! Procedural mesh generation (SPEC §3/C3: zero bundled assets, everything
//! built at runtime): surfaces of revolution (`lathe`) and function-sampled
//! discs (`bowl_from`), both swept around the world Y axis.
//!
//! Winding convention (shared with `scene.rs`'s screen-space backface cull):
//! every generator here produces triangles whose right-hand-rule face normal
//! (`cross(v1-v0, v2-v0)`) points the same way as the per-vertex normals it
//! stores. For `lathe` that direction is "outward" (away from the axis /
//! away from the solid interior); for `bowl_from` it's "upward" (out of the
//! dish, toward the sky, matching an arena-style height-field normal). The
//! two shapes need OPPOSITE quad-splitting orders to achieve this: writing
//! `S(u, v)` for the swept surface (`u` = profile/ring index, `v` = angle),
//! `lathe`'s own normal formula is exactly `dS/du x dS/dv` (up to a positive
//! scalar), while a disc's outward/upward normal is the negative of its
//! `dS/du x dS/dv` (radius increases with `u`, so sweeping outward twists
//! the natural parametrization downward) — see the two triangle-order
//! comments below for the derivation each relies on.

use floppy_core::fixmath;
use floppy_core::vec::{Vec2, Vec3};

pub struct Mesh {
    pub verts: Vec<Vec3>,
    pub norms: Vec<Vec3>,
    pub tris: Vec<[u16; 3]>,
}

const TAU: f32 = std::f32::consts::TAU;

/// Surface of revolution around Y. `profile` is a list of `(radius, y)`
/// pairs from bottom to top (tip); `segments` rings are swept around Y using
/// `fixmath::sin`/`fixmath::cos`.
///
/// Per-vertex normals: the profile's 2D tangent at each point (central
/// difference for interior points, one-sided at the ends) gives an in-plane
/// normal `(dy, -dr)` (the tangent rotated -90 degrees), which is then swept
/// into 3D the same way position is (radial component scaled by cos/sin of
/// the ring angle, height component untouched) and normalized.
///
/// A profile point with `r == 0` (an apex/base) naturally becomes a fan:
/// every vertex in that ring lands on the same point, so the quad on either
/// side of it degenerates into one zero-area triangle (skipped by the
/// rasterizer's `signed_area <= 0` check) and one real triangle — no
/// special-casing needed.
pub fn lathe(profile: &[(f32, f32)], segments: usize) -> Mesh {
    lathe_impl(profile, segments, |_theta| 1.0)
}

/// [`lathe`] variant with radial "lobing"/scalloping (M3-B / game_design.md
/// §3: "6-lobe sawblade scallop", "12 shallow glancing lobes", ...): at
/// every ring vertex the swept radius is scaled by `1.0 - depth * (0.5 +
/// 0.5*cos(angle*lobes))`, i.e. `lobes` evenly-spaced dents around the full
/// circumference at every height level (`depth == 0.0` reproduces plain
/// [`lathe`] exactly). Per-vertex normals are carried over UNMODIFIED from
/// the base (unlobed) profile tangent rather than re-derived from the
/// lobed surface's true (angle-dependent) curvature — a known, deliberate
/// approximation: correcting it would need a normal per (profile-point,
/// angle) pair instead of the cheap per-profile-point tangent this renderer
/// otherwise shares with [`lathe`], and at this project's low-poly/flat-
/// Gouraud aesthetic (SPEC §10, game_design.md §6) the visible error is far
/// smaller than the faceting from `segments` itself.
pub fn lathe_lobed(profile: &[(f32, f32)], segments: usize, lobes: u32, depth: f32) -> Mesh {
    lathe_impl(profile, segments, move |theta| {
        1.0 - depth * (0.5 + 0.5 * fixmath::cos(theta * lobes as f32))
    })
}

/// Shared surface-of-revolution builder behind [`lathe`]/[`lathe_lobed`]:
/// `radial_factor(theta)` scales the swept radius per ring vertex (`lathe`
/// passes the constant `1.0`).
fn lathe_impl(
    profile: &[(f32, f32)],
    segments: usize,
    radial_factor: impl Fn(f32) -> f32 + Copy,
) -> Mesh {
    let mut verts = Vec::new();
    let mut norms = Vec::new();
    let mut tris = Vec::new();

    if profile.is_empty() || segments == 0 {
        return Mesh { verts, norms, tris };
    }

    let n_pts = profile.len();
    verts.reserve(n_pts * segments);
    norms.reserve(n_pts * segments);

    // Per-profile-point 2D tangent: central difference for interior points,
    // one-sided (forward/backward) at the ends.
    let mut tangents = Vec::with_capacity(n_pts);
    for i in 0..n_pts {
        let prev = if i == 0 { profile[0] } else { profile[i - 1] };
        let next = if i + 1 == n_pts {
            profile[n_pts - 1]
        } else {
            profile[i + 1]
        };
        tangents.push(Vec2::new(next.0 - prev.0, next.1 - prev.1));
    }

    for i in 0..n_pts {
        let (r, y) = profile[i];
        let tangent = tangents[i];
        let normal2d = Vec2::new(tangent.y, -tangent.x).normalize_or_zero();
        for k in 0..segments {
            let theta = TAU * (k as f32) / (segments as f32);
            let ct = fixmath::cos(theta);
            let st = fixmath::sin(theta);
            let r_mod = r * radial_factor(theta);
            verts.push(Vec3::new(r_mod * ct, y, r_mod * st));
            norms.push(Vec3::new(normal2d.x * ct, normal2d.y, normal2d.x * st).normalize_or_zero());
        }
    }

    // Quad (i,k)-(i,k+1)-(i+1,k)-(i+1,k+1) split so each triangle's face
    // normal, AS SEEN THROUGH `camera.rs`'s particular (left-handed —
    // `right = fwd x world_up` rather than `world_up x fwd`) basis and
    // `Camera::project`'s y-flip, projects to a positive `signed_area`
    // (this renderer's front-facing convention) when the outward normal
    // faces the camera. Empirically pinned via `scene::tests` (a
    // right-handed-camera derivation gives the opposite pairing — see the
    // module docs' `dS/du x dS/dv` discussion for the shape-normal side of
    // this, and `scene.rs`'s backface test for the camera-handedness side).
    for i in 0..(n_pts - 1) {
        for k in 0..segments {
            let k2 = (k + 1) % segments;
            let a = (i * segments + k) as u16;
            let b = (i * segments + k2) as u16;
            let c = ((i + 1) * segments + k) as u16;
            let d = ((i + 1) * segments + k2) as u16;
            tris.push([a, b, d]);
            tris.push([a, d, c]);
        }
    }

    Mesh { verts, norms, tris }
}

/// Function-sampled disc: samples `height_fn(x, z)` at the center plus
/// `rings` concentric rings (`r_j = radius_max * j / rings`, `j` in
/// `1..=rings`) of `segments` points each, with per-vertex normals from
/// `normal_fn`. Takes plain closures/fn-pointers (not a hard dependency on
/// `floppy_core::arena`), so `floppy_render` stays decoupled from that
/// module's existence/shape — the real arena (or a throwaway stand-in
/// closure, e.g. for a self-contained demo) is supplied by the caller.
pub fn bowl_from(
    height_fn: impl Fn(f32, f32) -> f32 + Copy,
    normal_fn: impl Fn(f32, f32) -> Vec3 + Copy,
    rings: usize,
    segments: usize,
    radius_max: f32,
) -> Mesh {
    let mut verts = Vec::new();
    let mut norms = Vec::new();
    let mut tris = Vec::new();

    // Center point always exists, even in the degenerate rings==0/segments==0
    // case (a single-vertex, zero-triangle mesh rather than a special case
    // the caller has to guard against).
    verts.push(Vec3::new(0.0, height_fn(0.0, 0.0), 0.0));
    norms.push(normal_fn(0.0, 0.0));
    if rings == 0 || segments == 0 {
        return Mesh { verts, norms, tris };
    }

    verts.reserve(1 + rings * segments);
    norms.reserve(1 + rings * segments);

    let ring_index = |j: usize, k: usize| -> u16 { (1 + (j - 1) * segments + k) as u16 };

    for j in 1..=rings {
        let r = radius_max * (j as f32) / (rings as f32);
        for k in 0..segments {
            let theta = TAU * (k as f32) / (segments as f32);
            let x = r * fixmath::cos(theta);
            let z = r * fixmath::sin(theta);
            verts.push(Vec3::new(x, height_fn(x, z), z));
            norms.push(normal_fn(x, z));
        }
    }

    // Fan from the center to ring 1: the j=0 "ring" degenerates to a single
    // repeated point, which is exactly the a==b degenerate case of the
    // general quad split below (its zero-area half triangle vanishes,
    // leaving only [center, ring1[k], ring1[k2]] per segment) — so this fan
    // uses the SAME winding as the general quads for consistency.
    for k in 0..segments {
        let k2 = (k + 1) % segments;
        tris.push([0u16, ring_index(1, k), ring_index(1, k2)]);
    }

    // Quads between consecutive rings. `bowl_from`'s natural (ring-index,
    // angle) parametrization twists the opposite way from `lathe`'s (radius
    // increases with the ring index, so its `dS/du x dS/dv` points DOWN
    // where `normal_fn`'s convention is "up out of the dish") — the mirror
    // image of `lathe`'s pairing, both by that 3D argument and, separately,
    // empirically pinned via `mesh::tests`/`scene::tests` against this
    // renderer's actual (left-handed) camera convention.
    for j in 1..rings {
        for k in 0..segments {
            let k2 = (k + 1) % segments;
            let a = ring_index(j, k);
            let b = ring_index(j, k2);
            let c = ring_index(j + 1, k);
            let d = ring_index(j + 1, k2);
            tris.push([a, d, b]);
            tris.push([a, c, d]);
        }
    }

    Mesh { verts, norms, tris }
}

/// Thin wrapper around [`bowl_from`] using the real arena heightfield
/// (`floppy_core::arena`), for callers that don't need to stay decoupled
/// from it (e.g. the M3-B integrator). `bowl_from` remains the primitive;
/// this is optional sugar.
pub fn bowl(radial_rings: usize, segments: usize) -> Mesh {
    bowl_from(
        floppy_core::arena::height,
        floppy_core::arena::normal,
        radial_rings,
        segments,
        floppy_core::arena::ARENA_RADIUS,
    )
}

/// A thin concentric ring band following `height_fn(x, z) + y_offset`, at
/// radius `radius` with half-width `half_width` (inner edge = `radius -
/// half_width`, clamped to `0.0`; outer edge = `radius + half_width`), swept
/// in `segments` steps around Y (M3-B / game_design.md §6: the arena's neon
/// ring bands). Normals point straight up (`(0, 1, 0)`) — a flat glow strip
/// drawn additively (see `scene::draw_instance_additive`) never needs real
/// shading contrast.
pub fn ring_band(
    height_fn: impl Fn(f32, f32) -> f32 + Copy,
    radius: f32,
    half_width: f32,
    y_offset: f32,
    segments: usize,
) -> Mesh {
    let mut verts = Vec::new();
    let mut norms = Vec::new();
    let mut tris = Vec::new();

    if segments == 0 {
        return Mesh { verts, norms, tris };
    }

    let inner = (radius - half_width).max(0.0);
    let outer = radius + half_width;
    verts.reserve(2 * segments);
    norms.reserve(2 * segments);

    // Inner ring: indices [0, segments).
    for k in 0..segments {
        let theta = TAU * (k as f32) / (segments as f32);
        let x = inner * fixmath::cos(theta);
        let z = inner * fixmath::sin(theta);
        verts.push(Vec3::new(x, height_fn(x, z) + y_offset, z));
        norms.push(Vec3::new(0.0, 1.0, 0.0));
    }
    // Outer ring: indices [segments, 2*segments).
    for k in 0..segments {
        let theta = TAU * (k as f32) / (segments as f32);
        let x = outer * fixmath::cos(theta);
        let z = outer * fixmath::sin(theta);
        verts.push(Vec3::new(x, height_fn(x, z) + y_offset, z));
        norms.push(Vec3::new(0.0, 1.0, 0.0));
    }

    // Same quad winding as `bowl_from`'s between-ring quads (radius
    // increasing from `a`/`b` to `c`/`d` — here inner -> outer, matching
    // that "upward normal" convention exactly).
    for k in 0..segments {
        let k2 = (k + 1) % segments;
        let a = k as u16;
        let b = k2 as u16;
        let c = (segments + k) as u16;
        let d = (segments + k2) as u16;
        tris.push([a, d, b]);
        tris.push([a, c, d]);
    }

    Mesh { verts, norms, tris }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lathe_vert_and_tri_counts_match_profile_and_segments() {
        let profile = [(0.0, 0.0), (0.3, 0.2), (0.4, 0.5), (0.1, 0.8)];
        let segments = 12;
        let mesh = lathe(&profile, segments);
        assert_eq!(mesh.verts.len(), profile.len() * segments);
        assert_eq!(mesh.norms.len(), profile.len() * segments);
        assert_eq!(mesh.tris.len(), 2 * (profile.len() - 1) * segments);
    }

    #[test]
    fn lathe_normals_are_unit_length() {
        let profile = [
            (0.0, 0.0),
            (0.12, 0.05),
            (0.34, 0.18),
            (0.45, 0.32),
            (0.0, 0.6),
        ];
        let mesh = lathe(&profile, 16);
        for n in &mesh.norms {
            let len = n.length();
            assert!((len - 1.0).abs() < 1e-2, "len={len}");
        }
    }

    #[test]
    fn lathe_indices_never_out_of_bounds() {
        let profile = [(0.0, 0.0), (0.5, 0.5), (0.0, 1.0)];
        let segments = 8;
        let mesh = lathe(&profile, segments);
        let n = mesh.verts.len() as u16;
        for tri in &mesh.tris {
            for &idx in tri {
                assert!(idx < n, "idx {idx} out of bounds (n={n})");
            }
        }
    }

    #[test]
    fn lathe_empty_profile_or_zero_segments_is_empty_mesh() {
        let mesh = lathe(&[], 8);
        assert!(mesh.verts.is_empty() && mesh.tris.is_empty());
        let mesh2 = lathe(&[(0.0, 0.0), (1.0, 1.0)], 0);
        assert!(mesh2.verts.is_empty() && mesh2.tris.is_empty());
    }

    #[test]
    fn bowl_from_paraboloid_counts_and_normals() {
        let height = |x: f32, z: f32| 0.02 * (x * x + z * z);
        let normal = |x: f32, z: f32| {
            // Gradient of 0.02*(x^2+z^2): (0.04x, 0.04z); up-normal =
            // normalize(-dhdx, 1, -dhdz).
            Vec3::new(-0.04 * x, 1.0, -0.04 * z).normalize_or_zero()
        };
        let rings = 5;
        let segments = 10;
        let mesh = bowl_from(height, normal, rings, segments, 9.5);
        assert_eq!(mesh.verts.len(), 1 + rings * segments);
        assert_eq!(mesh.tris.len(), segments + 2 * (rings - 1) * segments);
        for n in &mesh.norms {
            let len = n.length();
            assert!((len - 1.0).abs() < 1e-2, "len={len}");
        }
        let n = mesh.verts.len() as u16;
        for tri in &mesh.tris {
            for &idx in tri {
                assert!(idx < n);
            }
        }
    }

    #[test]
    fn bowl_from_degenerate_dims_are_single_point_no_triangles() {
        let mesh = bowl_from(|_, _| 0.0, |_, _| Vec3::new(0.0, 1.0, 0.0), 0, 8, 9.5);
        assert_eq!(mesh.verts.len(), 1);
        assert!(mesh.tris.is_empty());
    }

    #[test]
    fn lathe_lobed_with_zero_depth_matches_plain_lathe() {
        let profile = [(0.0, 0.0), (0.4, 0.3), (0.5, 0.6), (0.1, 1.0)];
        let plain = lathe(&profile, 20);
        let lobed = lathe_lobed(&profile, 20, 6, 0.0);
        assert_eq!(plain.verts.len(), lobed.verts.len());
        for (p, l) in plain.verts.iter().zip(lobed.verts.iter()) {
            assert!((p.x - l.x).abs() < 1e-4);
            assert!((p.y - l.y).abs() < 1e-4);
            assert!((p.z - l.z).abs() < 1e-4);
        }
        assert_eq!(plain.tris, lobed.tris);
    }

    #[test]
    fn lathe_lobed_produces_lobes_count_radius_peaks_around_a_ring() {
        // A single mid-profile ring at constant r=1.0, lobes=6, depth=0.5:
        // radius should oscillate between 1.0 (peak, factor=1) and 0.5
        // (trough, factor=0.5) exactly `lobes` times per revolution.
        let profile = [(0.0, 0.0), (1.0, 0.5), (0.0, 1.0)];
        let segments = 240usize; // fine enough to resolve 6 lobes cleanly
        let lobes = 6u32;
        let depth = 0.5;
        let mesh = lathe_lobed(&profile, segments, lobes, depth);
        // Ring index 1 (the r=1.0 profile point) occupies vertices
        // [segments..2*segments).
        let mut max_r: f32 = 0.0;
        let mut min_r: f32 = f32::MAX;
        for k in 0..segments {
            let v = mesh.verts[segments + k];
            let r = fixmath::sqrt(v.x * v.x + v.z * v.z);
            max_r = max_r.max(r);
            min_r = min_r.min(r);
        }
        assert!((max_r - 1.0).abs() < 0.05, "max_r={max_r}");
        assert!((min_r - 0.5).abs() < 0.05, "min_r={min_r}");
    }

    #[test]
    fn lathe_lobed_empty_profile_or_zero_segments_is_empty_mesh() {
        let mesh = lathe_lobed(&[], 8, 6, 0.3);
        assert!(mesh.verts.is_empty() && mesh.tris.is_empty());
        let mesh2 = lathe_lobed(&[(0.0, 0.0), (1.0, 1.0)], 0, 6, 0.3);
        assert!(mesh2.verts.is_empty() && mesh2.tris.is_empty());
    }

    #[test]
    fn ring_band_counts_and_radii() {
        let height = |_: f32, _: f32| 0.0;
        let segments = 24;
        let mesh = ring_band(height, 5.0, 0.1, 0.02, segments);
        assert_eq!(mesh.verts.len(), 2 * segments);
        assert_eq!(mesh.tris.len(), 2 * segments);
        for (i, v) in mesh.verts.iter().enumerate() {
            let r = fixmath::sqrt(v.x * v.x + v.z * v.z);
            if i < segments {
                assert!((r - 4.9).abs() < 1e-3, "inner r={r}");
            } else {
                assert!((r - 5.1).abs() < 1e-3, "outer r={r}");
            }
            assert!((v.y - 0.02).abs() < 1e-5, "y_offset not applied: y={}", v.y);
        }
        let n = mesh.verts.len() as u16;
        for tri in &mesh.tris {
            for &idx in tri {
                assert!(idx < n, "idx {idx} out of bounds (n={n})");
            }
        }
    }

    #[test]
    fn ring_band_inner_radius_clamps_at_zero() {
        let mesh = ring_band(|_, _| 0.0, 0.05, 0.5, 0.0, 8);
        for v in mesh.verts.iter().take(8) {
            let r = fixmath::sqrt(v.x * v.x + v.z * v.z);
            assert!(r < 1e-3, "expected inner radius clamped to 0, r={r}");
        }
    }

    #[test]
    fn ring_band_zero_segments_is_empty_mesh() {
        let mesh = ring_band(|_, _| 0.0, 5.0, 0.1, 0.0, 0);
        assert!(mesh.verts.is_empty() && mesh.tris.is_empty());
    }
}
