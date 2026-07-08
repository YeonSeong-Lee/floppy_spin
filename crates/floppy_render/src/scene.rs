//! Ties `camera`/`clip`/`mesh`/`raster`/`shade` together: transform a mesh
//! instance into world space, shade per-vertex, clip against the near plane,
//! project, cull backfaces, rasterize.
//!
//! Transform composition (documented, fixed order — SPEC §5 determinism):
//! `world = offset + Rz(tilt.y) * Rx(tilt.x) * Ry(yaw) * Scale(radial,
//! height, radial) * local`, i.e. a (possibly non-uniform, radius-vs-height)
//! scale first, then yaw around Y, then tilt around X, then tilt around Z,
//! then translate (M3-B: lets a body-of-revolution mesh be authored once at
//! `radius = height = 1.0` and reused at any `Top::radius`/`Top::height`).
//! Normals get the inverse-transpose of the scale (a diagonal matrix, so
//! just the elementwise reciprocal) followed by the same three rotations (no
//! translation), then are renormalized.

use crate::camera::Camera;
use crate::clip;
use crate::frame::Frame;
use crate::mesh::{self, Mesh};
use crate::post::BrightBuffer;
use crate::raster::{
    self, draw_tri, draw_tri_additive, draw_tri_additive_bloom, draw_tri_bloom, SVert,
};
use crate::shade::{self, Material};
use floppy_core::fixmath;
use floppy_core::vec::{Vec2, Vec3};

pub struct Instance<'a> {
    pub mesh: &'a Mesh,
    pub offset: Vec3,
    pub yaw: f32,
    /// Axis tilt: rotation around X (`.x`) then around Z (`.y`), radians.
    pub tilt: Vec2,
    pub material: Material,
    /// Local-space scale applied to the X/Z (radial) axes before rotation
    /// (M3-B: a lathed mesh authored at `radius = 1.0` scaled to a top's
    /// actual `radius`). `1.0` reproduces the mesh's authored geometry.
    pub radial_scale: f32,
    /// Local-space scale applied to the Y (height) axis before rotation.
    /// `1.0` reproduces the mesh's authored geometry.
    pub height_scale: f32,
}

fn rot_y(v: Vec3, angle: f32) -> Vec3 {
    let c = fixmath::cos(angle);
    let s = fixmath::sin(angle);
    Vec3::new(v.x * c + v.z * s, v.y, -v.x * s + v.z * c)
}

fn rot_x(v: Vec3, angle: f32) -> Vec3 {
    let c = fixmath::cos(angle);
    let s = fixmath::sin(angle);
    Vec3::new(v.x, v.y * c - v.z * s, v.y * s + v.z * c)
}

fn rot_z(v: Vec3, angle: f32) -> Vec3 {
    let c = fixmath::cos(angle);
    let s = fixmath::sin(angle);
    Vec3::new(v.x * c - v.y * s, v.x * s + v.y * c, v.z)
}

/// Yaw -> tilt.x (around X) -> tilt.y (around Z), matching module docs.
fn rotate(v: Vec3, yaw: f32, tilt: Vec2) -> Vec3 {
    let v = rot_y(v, yaw);
    let v = rot_x(v, tilt.x);
    rot_z(v, tilt.y)
}

/// Local-space (radial, height, radial) scale applied to a position before
/// rotation (module docs).
fn scale_pos(v: Vec3, radial: f32, height: f32) -> Vec3 {
    Vec3::new(v.x * radial, v.y * height, v.z * radial)
}

/// Inverse-transpose of [`scale_pos`]'s diagonal scale matrix applied to a
/// normal (elementwise reciprocal, since a diagonal matrix is its own
/// transpose), renormalized. `radial`/`height` are floored to a small
/// epsilon first so a degenerate `0.0` scale can never divide-by-zero into
/// `NaN`/`inf` (module docs).
fn scale_normal(n: Vec3, radial: f32, height: f32) -> Vec3 {
    const MIN_SCALE: f32 = 1e-4;
    let r = radial.max(MIN_SCALE);
    let h = height.max(MIN_SCALE);
    Vec3::new(n.x / r, n.y / h, n.z / r).normalize_or_zero()
}

/// Draw every triangle of `inst.mesh`, transformed by `inst`, through
/// `cam`, into `frame`. Per triangle: transform + shade (world space) each
/// vertex, near-clip in view space (colors already computed, so clipping
/// just interpolates them alongside position), backface-cull the projected
/// triangle (`signed_area <= 0` — same convention/threshold `raster.rs`
/// itself re-checks), then rasterize.
pub fn draw_instance(frame: &mut Frame, cam: &Camera, inst: &Instance) {
    draw_instance_impl(frame, cam, inst, |_local_idx| &inst.material, false);
}

/// Like [`draw_instance`], but draws with [`raster::draw_tri_additive`]
/// (depth-tested, not depth-written, colors ADD onto the framebuffer) —
/// M3-B / game_design.md §6's neon arena ring bands.
pub fn draw_instance_additive(frame: &mut Frame, cam: &Camera, inst: &Instance) {
    draw_instance_impl(frame, cam, inst, |_local_idx| &inst.material, true);
}

/// Like [`draw_instance`], but selects between `inst.material` and
/// `alt_material` per vertex via `is_alt(local_vertex_index)` — the index
/// into `inst.mesh.verts`/`norms` (M3-B / game_design.md §3/§6: a top's
/// dark-metal body vs. its emissive accent flange, without needing a second
/// overlapping mesh that would z-fight the first).
pub fn draw_instance_split(
    frame: &mut Frame,
    cam: &Camera,
    inst: &Instance,
    alt_material: &Material,
    is_alt: impl Fn(usize) -> bool,
) {
    draw_instance_impl(
        frame,
        cam,
        inst,
        |local_idx| {
            if is_alt(local_idx) {
                alt_material
            } else {
                &inst.material
            }
        },
        false,
    );
}

/// Shared per-triangle pipeline behind [`draw_instance`]/
/// [`draw_instance_additive`]/[`draw_instance_split`]:
/// `material_for(local_vertex_index)` resolves which `Material` shades each
/// vertex; `additive` picks [`raster::draw_tri`] vs
/// [`raster::draw_tri_additive`] for the final rasterize call.
fn draw_instance_impl<'a>(
    frame: &mut Frame,
    cam: &Camera,
    inst: &Instance,
    material_for: impl Fn(usize) -> &'a Material,
    additive: bool,
) {
    for tri in &inst.mesh.tris {
        let mut world = [Vec3::default(); 3];
        let mut col = [Vec3::default(); 3];

        for i in 0..3 {
            let idx = tri[i] as usize;
            let local = scale_pos(inst.mesh.verts[idx], inst.radial_scale, inst.height_scale);
            let local_n = scale_normal(inst.mesh.norms[idx], inst.radial_scale, inst.height_scale);
            let w = rotate(local, inst.yaw, inst.tilt) + inst.offset;
            let wn = rotate(local_n, inst.yaw, inst.tilt);
            let view_dir = (cam.pos - w).normalize_or_zero();
            world[i] = w;
            col[i] = shade::shade(wn, view_dir, material_for(idx));
        }

        let view_tri = [
            (cam.to_view(world[0]), col[0]),
            (cam.to_view(world[1]), col[1]),
            (cam.to_view(world[2]), col[2]),
        ];

        let (clipped, count) = clip::clip_near(view_tri, Camera::NEAR);
        for t in clipped.iter().take(count as usize) {
            let t = *t;
            let (x0, y0, z0) = cam.project(t[0].0);
            let (x1, y1, z1) = cam.project(t[1].0);
            let (x2, y2, z2) = cam.project(t[2].0);

            if raster::signed_area(x0, y0, x1, y1, x2, y2) <= 0.0 {
                continue;
            }

            let sv = |x: f32, y: f32, inv_z: f32, c: Vec3| SVert {
                x,
                y,
                inv_z,
                r: c.x,
                g: c.y,
                b: c.z,
            };
            let va = sv(x0, y0, z0, t[0].1);
            let vb = sv(x1, y1, z1, t[1].1);
            let vc = sv(x2, y2, z2, t[2].1);
            if additive {
                draw_tri_additive(frame, va, vb, vc);
            } else {
                draw_tri(frame, va, vb, vc);
            }
        }
    }
}

/// Does this material contribute to bloom (game_design.md §6: "emissive-
/// tagged pixels always bloom")? A material bloomas iff it has ANY nonzero
/// emissive component — the dark-metal top body / arena materials are
/// exactly `emissive = (0,0,0)` and never bloom; the accent flange / neon
/// ring materials are always nonzero.
fn is_emissive(m: &Material) -> bool {
    m.emissive.length_sq() > 0.0
}

/// Bloom-aware, shake-aware sibling of [`draw_instance`] (M7 Task 1/3):
/// tags emissive triangles into `bright` (see `raster.rs`'s
/// `draw_tri_bloom` docs) and offsets every projected screen coordinate by
/// `shake` (whole pixels — game_design.md §5 shake, applied here so the
/// WHOLE 3D scene shakes together; HUD is drawn separately, unshifted, by
/// `main.rs`/`hud.rs`). A separate code path from `draw_instance_impl`
/// (rather than threading an `Option<&mut BrightBuffer>` through the shared
/// one) — `battle.rs`'s bloom pipeline always has a live `BrightBuffer`, so
/// there's no "sometimes None" case to thread, and keeping the two loops
/// textually separate sidesteps a double-mutable-reference reborrow dance
/// for a few lines' worth of near-duplication.
#[allow(clippy::too_many_arguments)]
fn draw_instance_impl_bloom<'a>(
    frame: &mut Frame,
    bright: &mut BrightBuffer,
    cam: &Camera,
    inst: &Instance,
    material_for: impl Fn(usize) -> &'a Material,
    additive: bool,
    shake: (f32, f32),
) {
    for tri in &inst.mesh.tris {
        let mut world = [Vec3::default(); 3];
        let mut col = [Vec3::default(); 3];
        let mut emissive = false;

        for i in 0..3 {
            let idx = tri[i] as usize;
            let m = material_for(idx);
            emissive = emissive || is_emissive(m);
            let local = scale_pos(inst.mesh.verts[idx], inst.radial_scale, inst.height_scale);
            let local_n = scale_normal(inst.mesh.norms[idx], inst.radial_scale, inst.height_scale);
            let w = rotate(local, inst.yaw, inst.tilt) + inst.offset;
            let wn = rotate(local_n, inst.yaw, inst.tilt);
            let view_dir = (cam.pos - w).normalize_or_zero();
            world[i] = w;
            col[i] = shade::shade(wn, view_dir, m);
        }

        let view_tri = [
            (cam.to_view(world[0]), col[0]),
            (cam.to_view(world[1]), col[1]),
            (cam.to_view(world[2]), col[2]),
        ];

        let (clipped, count) = clip::clip_near(view_tri, Camera::NEAR);
        for t in clipped.iter().take(count as usize) {
            let t = *t;
            let (x0, y0, z0) = cam.project(t[0].0);
            let (x1, y1, z1) = cam.project(t[1].0);
            let (x2, y2, z2) = cam.project(t[2].0);

            if raster::signed_area(x0, y0, x1, y1, x2, y2) <= 0.0 {
                continue;
            }

            let sv = |x: f32, y: f32, inv_z: f32, c: Vec3| SVert {
                x: x + shake.0,
                y: y + shake.1,
                inv_z,
                r: c.x,
                g: c.y,
                b: c.z,
            };
            let va = sv(x0, y0, z0, t[0].1);
            let vb = sv(x1, y1, z1, t[1].1);
            let vc = sv(x2, y2, z2, t[2].1);
            if additive {
                draw_tri_additive_bloom(frame, bright, va, vb, vc, emissive);
            } else {
                draw_tri_bloom(frame, bright, va, vb, vc, emissive);
            }
        }
    }
}

/// Bloom/shake-aware sibling of [`draw_instance`] (see
/// `draw_instance_impl_bloom`'s docs).
pub fn draw_instance_bloom(
    frame: &mut Frame,
    bright: &mut BrightBuffer,
    cam: &Camera,
    inst: &Instance,
    shake: (f32, f32),
) {
    draw_instance_impl_bloom(
        frame,
        bright,
        cam,
        inst,
        |_local_idx| &inst.material,
        false,
        shake,
    );
}

/// Bloom/shake-aware sibling of [`draw_instance_additive`].
pub fn draw_instance_additive_bloom(
    frame: &mut Frame,
    bright: &mut BrightBuffer,
    cam: &Camera,
    inst: &Instance,
    shake: (f32, f32),
) {
    draw_instance_impl_bloom(
        frame,
        bright,
        cam,
        inst,
        |_local_idx| &inst.material,
        true,
        shake,
    );
}

/// Bloom/shake-aware sibling of [`draw_instance_split`].
#[allow(clippy::too_many_arguments)]
pub fn draw_instance_split_bloom(
    frame: &mut Frame,
    bright: &mut BrightBuffer,
    cam: &Camera,
    inst: &Instance,
    alt_material: &Material,
    is_alt: impl Fn(usize) -> bool,
    shake: (f32, f32),
) {
    draw_instance_impl_bloom(
        frame,
        bright,
        cam,
        inst,
        |local_idx| {
            if is_alt(local_idx) {
                alt_material
            } else {
                &inst.material
            }
        },
        false,
        shake,
    );
}

/// The M3-A demo scene / determinism-test target (SPEC §10 perf smoke also
/// exercises this): a fixed camera, a spinning lathed top, and a bowl. The
/// bowl uses a self-contained paraboloid closure (NOT
/// `floppy_core::arena`), deliberately — `floppy_core::arena` is being
/// authored concurrently by another agent this session, and this demo's
/// determinism-test output must not silently change out from under it. See
/// `mesh::bowl` for the real-arena wrapper used once an integrator wires the
/// two crates together (M3-B).
pub fn draw_test_scene(frame: &mut Frame, t: f32) {
    frame.clear(0x000A0A14);

    let cam = Camera::look_at(
        Vec3::new(0.0, 14.0, -13.0),
        Vec3::new(0.0, 0.5, 0.0),
        55.0,
        frame.w,
        frame.h,
    );

    let profile = [
        (0.0, 0.0),
        (0.12, 0.05),
        (0.34, 0.18),
        (0.45, 0.32),
        (0.42, 0.40),
        (0.18, 0.48),
        (0.10, 0.60),
    ];
    let top_mesh = mesh::lathe(&profile, 24);
    let top_material = Material {
        base: Vec3::new(0.75, 0.78, 0.82),
        emissive: Vec3::new(0.0, 0.9, 1.0) * 0.15,
        spec_power_i: 32,
        spec_strength: 0.6,
    };
    let top_instance = Instance {
        mesh: &top_mesh,
        offset: Vec3::new(0.0, 0.0, 0.0),
        yaw: t * 8.0,
        tilt: Vec2::new(0.12 * fixmath::sin(t * 3.0), 0.0),
        material: top_material,
        radial_scale: 1.0,
        height_scale: 1.0,
    };
    draw_instance(frame, &cam, &top_instance);

    // Self-contained paraboloid bowl + implicit rim (the paraboloid itself
    // curls upward toward the edge, giving a wall-like silhouette without
    // needing a separate term) — see the doc comment above for why this
    // demo doesn't call `floppy_core::arena` directly.
    let height = |x: f32, z: f32| -> f32 { 0.02 * (x * x + z * z) };
    let normal =
        |x: f32, z: f32| -> Vec3 { Vec3::new(-0.04 * x, 1.0, -0.04 * z).normalize_or_zero() };
    let bowl_mesh = mesh::bowl_from(height, normal, 10, 32, 9.5);
    let bowl_material = Material {
        base: Vec3::new(0.08, 0.09, 0.15),
        emissive: Vec3::new(0.0, 0.0, 0.0),
        spec_power_i: 8,
        spec_strength: 0.15,
    };
    let bowl_instance = Instance {
        mesh: &bowl_mesh,
        offset: Vec3::new(0.0, 0.0, 0.0),
        yaw: 0.0,
        tilt: Vec2::new(0.0, 0.0),
        material: bowl_material,
        radial_scale: 1.0,
        height_scale: 1.0,
    };
    draw_instance(frame, &cam, &bowl_instance);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_tri_mesh(order: [u16; 3]) -> Mesh {
        Mesh {
            verts: vec![
                Vec3::new(-1.0, -1.0, 0.0),
                Vec3::new(1.0, -1.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
            ],
            norms: vec![
                Vec3::new(0.0, 0.0, -1.0),
                Vec3::new(0.0, 0.0, -1.0),
                Vec3::new(0.0, 0.0, -1.0),
            ],
            tris: vec![order],
        }
    }

    fn flat_material() -> Material {
        Material {
            base: Vec3::new(0.8, 0.8, 0.8),
            emissive: Vec3::new(0.0, 0.0, 0.0),
            spec_power_i: 4,
            spec_strength: 0.0,
        }
    }

    fn painted_count(frame: &Frame) -> usize {
        frame.px.iter().filter(|&&p| p != 0).count()
    }

    #[test]
    fn backface_culling_via_draw_instance_paints_nothing() {
        let cam = Camera::look_at(
            Vec3::new(0.0, 0.0, -5.0),
            Vec3::new(0.0, 0.0, 0.0),
            60.0,
            64,
            64,
        );

        // Empirically pinned (see mesh.rs module docs on camera
        // handedness): winding [0,1,2] projects to positive `signed_area`
        // through THIS camera's basis and must be visible.
        let front_mesh = flat_tri_mesh([0, 1, 2]);
        let front_inst = Instance {
            mesh: &front_mesh,
            offset: Vec3::default(),
            yaw: 0.0,
            tilt: Vec2::default(),
            material: flat_material(),
            radial_scale: 1.0,
            height_scale: 1.0,
        };
        let mut f = Frame::new(64, 64);
        draw_instance(&mut f, &cam, &front_inst);
        assert!(
            painted_count(&f) > 0,
            "front-facing triangle must be visible"
        );

        // The reversed winding [0,2,1] projects to negative `signed_area`
        // and must be culled entirely.
        let back_mesh = flat_tri_mesh([0, 2, 1]);
        let back_inst = Instance {
            mesh: &back_mesh,
            offset: Vec3::default(),
            yaw: 0.0,
            tilt: Vec2::default(),
            material: flat_material(),
            radial_scale: 1.0,
            height_scale: 1.0,
        };
        let mut f2 = Frame::new(64, 64);
        draw_instance(&mut f2, &cam, &back_inst);
        assert_eq!(painted_count(&f2), 0, "back-facing triangle must be culled");
    }

    #[test]
    fn straddling_near_plane_renders_without_panic_and_paints_something() {
        let cam = Camera::look_at(
            Vec3::new(0.0, 0.0, -5.0),
            Vec3::new(0.0, 0.0, 0.0),
            60.0,
            64,
            64,
        );
        // view.z = world.z + 5 for this camera (see scene tests above /
        // camera derivation): NEAR = 0.5 -> straddle at world.z ~ -4.5.
        let straddle_mesh = Mesh {
            verts: vec![
                Vec3::new(-8.0, -8.0, -4.8), // view.z = 0.2 (behind near)
                Vec3::new(8.0, -8.0, 0.0),   // view.z = 5.0 (in front)
                Vec3::new(0.0, 8.0, 0.0),    // view.z = 5.0 (in front)
            ],
            norms: vec![
                Vec3::new(0.0, 0.0, -1.0),
                Vec3::new(0.0, 0.0, -1.0),
                Vec3::new(0.0, 0.0, -1.0),
            ],
            tris: vec![[0, 1, 2]],
        };
        let inst = Instance {
            mesh: &straddle_mesh,
            offset: Vec3::default(),
            yaw: 0.0,
            tilt: Vec2::default(),
            material: flat_material(),
            radial_scale: 1.0,
            height_scale: 1.0,
        };
        let mut f = Frame::new(64, 64);
        draw_instance(&mut f, &cam, &inst); // must not panic
        assert!(
            painted_count(&f) > 0,
            "straddling triangle should paint something"
        );
    }

    #[test]
    fn fully_behind_near_plane_paints_nothing_and_does_not_panic() {
        let cam = Camera::look_at(
            Vec3::new(0.0, 0.0, -5.0),
            Vec3::new(0.0, 0.0, 0.0),
            60.0,
            64,
            64,
        );
        // All three vertices have view.z = 0.1 < NEAR (0.5): fully behind.
        let behind_mesh = Mesh {
            verts: vec![
                Vec3::new(-8.0, -8.0, -4.9),
                Vec3::new(8.0, -8.0, -4.9),
                Vec3::new(0.0, 8.0, -4.9),
            ],
            norms: vec![
                Vec3::new(0.0, 0.0, -1.0),
                Vec3::new(0.0, 0.0, -1.0),
                Vec3::new(0.0, 0.0, -1.0),
            ],
            tris: vec![[0, 2, 1]],
        };
        let inst = Instance {
            mesh: &behind_mesh,
            offset: Vec3::default(),
            yaw: 0.0,
            tilt: Vec2::default(),
            material: flat_material(),
            radial_scale: 1.0,
            height_scale: 1.0,
        };
        let mut f = Frame::new(64, 64);
        draw_instance(&mut f, &cam, &inst);
        assert_eq!(painted_count(&f), 0);
    }

    #[test]
    fn scale_pos_scales_radial_and_height_independently() {
        let v = Vec3::new(2.0, 3.0, -1.0);
        let s = scale_pos(v, 2.0, 5.0);
        assert_eq!(s, Vec3::new(4.0, 15.0, -2.0));
    }

    #[test]
    fn scale_normal_keeps_direction_under_uniform_scale() {
        // A uniform (radial == height) scale doesn't skew the surface, so
        // the reciprocal-then-renormalize transform should reproduce the
        // same direction it started with.
        let n = Vec3::new(0.6, 0.8, 0.0); // already unit length
        let s = scale_normal(n, 2.0, 2.0);
        assert!((s.x - n.x).abs() < 1e-5, "s={s:?}");
        assert!((s.y - n.y).abs() < 1e-5, "s={s:?}");
        assert!((s.length() - 1.0).abs() < 1e-5, "len={}", s.length());
    }

    #[test]
    fn scale_normal_never_produces_non_finite_output_on_degenerate_scale() {
        let n = Vec3::new(0.0, 1.0, 0.0);
        let s = scale_normal(n, 0.0, 0.0);
        assert!(
            s.x.is_finite() && s.y.is_finite() && s.z.is_finite(),
            "s={s:?}"
        );
    }

    #[test]
    fn draw_instance_radial_scale_grows_the_painted_footprint() {
        let cam = Camera::look_at(
            Vec3::new(0.0, 0.0, -5.0),
            Vec3::new(0.0, 0.0, 0.0),
            60.0,
            64,
            64,
        );
        let front_mesh = flat_tri_mesh([0, 1, 2]);
        let base_inst = Instance {
            mesh: &front_mesh,
            offset: Vec3::default(),
            yaw: 0.0,
            tilt: Vec2::default(),
            material: flat_material(),
            radial_scale: 1.0,
            height_scale: 1.0,
        };
        let mut f_base = Frame::new(64, 64);
        draw_instance(&mut f_base, &cam, &base_inst);
        let base_count = painted_count(&f_base);

        let scaled_inst = Instance {
            radial_scale: 1.6,
            height_scale: 1.6,
            ..base_inst
        };
        let mut f_scaled = Frame::new(64, 64);
        draw_instance(&mut f_scaled, &cam, &scaled_inst);
        let scaled_count = painted_count(&f_scaled);

        assert!(
            scaled_count > base_count,
            "base_count={base_count} scaled_count={scaled_count}"
        );
    }

    /// Two independent (non-adjacent) triangles, left and right, sharing no
    /// vertices — so `draw_instance_split`'s per-vertex-index material
    /// selection can be pinned to one triangle without touching the other.
    fn split_test_mesh() -> Mesh {
        Mesh {
            verts: vec![
                // Left triangle: indices 0,1,2 (base material).
                Vec3::new(-1.0, -1.0, 0.0),
                Vec3::new(0.0, -1.0, 0.0),
                Vec3::new(-0.5, 1.0, 0.0),
                // Right triangle: indices 3,4,5 (alt material).
                Vec3::new(0.0, -1.0, 0.0),
                Vec3::new(1.0, -1.0, 0.0),
                Vec3::new(0.5, 1.0, 0.0),
            ],
            norms: vec![Vec3::new(0.0, 0.0, -1.0); 6],
            tris: vec![[0, 1, 2], [3, 4, 5]],
        }
    }

    #[test]
    fn draw_instance_split_uses_alt_material_only_for_selected_vertices() {
        let cam = Camera::look_at(
            Vec3::new(0.0, 0.0, -5.0),
            Vec3::new(0.0, 0.0, 0.0),
            60.0,
            64,
            64,
        );
        let mesh = split_test_mesh();
        // Base material: pure black under this lighting (base=0 -> the
        // ambient/diffuse term multiplies to 0, no emissive, no spec) so it
        // is indistinguishable from the cleared background — exactly the
        // point: only the alt (right) triangle should show any color.
        let base_material = Material {
            base: Vec3::new(0.0, 0.0, 0.0),
            emissive: Vec3::new(0.0, 0.0, 0.0),
            spec_power_i: 1,
            spec_strength: 0.0,
        };
        let alt_material = Material {
            base: Vec3::new(0.0, 0.0, 0.0),
            emissive: Vec3::new(1.0, 0.0, 0.0),
            spec_power_i: 1,
            spec_strength: 0.0,
        };
        let inst = Instance {
            mesh: &mesh,
            offset: Vec3::default(),
            yaw: 0.0,
            tilt: Vec2::default(),
            material: base_material,
            radial_scale: 1.0,
            height_scale: 1.0,
        };
        let mut f = Frame::new(64, 64);
        // Vertex indices 3,4,5 (the right triangle) get the alt material.
        draw_instance_split(&mut f, &cam, &inst, &alt_material, |idx| idx >= 3);

        let mut red_xs = Vec::new();
        for y in 0..64 {
            for x in 0..64 {
                let p = f.px[y * 64 + x];
                if (p >> 16) & 0xFF == 255 {
                    red_xs.push(x);
                }
            }
        }
        assert!(!red_xs.is_empty(), "expected some red (alt) pixels");
        // camera.rs's LEFT-handed basis (`right = fwd x world_up`) means
        // world +x (the alt/right triangle, indices 3,4,5) projects to the
        // SMALLER screen-x half, not the larger one — empirically confirmed
        // here rather than assumed, matching the same handedness quirk
        // `mesh.rs`'s module docs call out for triangle winding.
        assert!(
            red_xs.iter().all(|&x| x < 32),
            "alt-material pixels leaked into the base-material half: {red_xs:?}"
        );
    }

    #[test]
    fn draw_instance_additive_adds_onto_existing_pixels_and_writes_no_depth() {
        let cam = Camera::look_at(
            Vec3::new(0.0, 0.0, -5.0),
            Vec3::new(0.0, 0.0, 0.0),
            60.0,
            64,
            64,
        );
        let front_mesh = flat_tri_mesh([0, 1, 2]);
        let material = Material {
            base: Vec3::new(0.0, 0.0, 0.0),
            emissive: Vec3::new(50.0 / 255.0, 0.0, 0.0),
            spec_power_i: 1,
            spec_strength: 0.0,
        };
        let inst = Instance {
            mesh: &front_mesh,
            offset: Vec3::default(),
            yaw: 0.0,
            tilt: Vec2::default(),
            material,
            radial_scale: 1.0,
            height_scale: 1.0,
        };
        let mut f = Frame::new(64, 64);
        draw_instance_additive(&mut f, &cam, &inst);
        draw_instance_additive(&mut f, &cam, &inst);
        // Two additive passes at the same low value must sum (not
        // overwrite): distinguishable from a single pass, matching
        // `raster::tests::fill_rule_covers_every_pixel_exactly_once`'s
        // "doubling-detectable value" approach.
        let any_doubled =
            f.px.iter()
                .any(|&p| (98..=102).contains(&((p >> 16) & 0xFF)));
        assert!(any_doubled, "expected additive passes to sum");
        // No depth written: every depth entry stays at the cleared 0.0.
        assert!(
            f.depth.iter().all(|&d| d == 0.0),
            "additive must not write depth"
        );
    }

    #[test]
    fn draw_test_scene_paints_a_reasonable_fraction_of_the_frame() {
        let mut f = Frame::new(960, 540);
        draw_test_scene(&mut f, 0.0);
        let painted = painted_count(&f);
        // Background is 0x000A0A14 (nonzero as a u32), so "painted" here
        // really means "differs from 0" is the wrong metric for the clear
        // color; recompute against the actual clear color instead.
        let bg = 0x000A0A14u32;
        let non_bg = f.px.iter().filter(|&&p| p != bg).count();
        assert!(
            non_bg > 1000,
            "expected a visible top+bowl, got {non_bg} non-background pixels (painted={painted})"
        );
    }

    #[test]
    fn determinism_same_t_same_hash_different_t_differs() {
        let mut f1 = Frame::new(960, 540);
        let mut f2 = Frame::new(960, 540);
        draw_test_scene(&mut f1, 0.0);
        draw_test_scene(&mut f2, 0.0);
        let h1 = floppy_core::hash::hash_u32s(&f1.px);
        let h2 = floppy_core::hash::hash_u32s(&f2.px);
        assert_eq!(h1, h2, "identical inputs must hash identically");

        let mut f3 = Frame::new(960, 540);
        draw_test_scene(&mut f3, 1.0);
        let h3 = floppy_core::hash::hash_u32s(&f3.px);
        assert_ne!(h1, h3, "different t must produce a different frame");
    }

    #[test]
    #[ignore]
    fn perf_smoke_sixty_frames_under_two_seconds() {
        let mut f = Frame::new(960, 540);
        let start = std::time::Instant::now();
        for i in 0..60 {
            draw_test_scene(&mut f, i as f32 * (1.0 / 60.0));
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_secs_f64() < 2.0,
            "60 frames took {:?}, expected < 2s (run --release)",
            elapsed
        );
    }
}
