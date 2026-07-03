//! Ties `camera`/`clip`/`mesh`/`raster`/`shade` together: transform a mesh
//! instance into world space, shade per-vertex, clip against the near plane,
//! project, cull backfaces, rasterize.
//!
//! Transform composition (documented, fixed order — SPEC §5 determinism):
//! `world = offset + Rz(tilt.y) * Rx(tilt.x) * Ry(yaw) * local`, i.e. yaw
//! around Y first, then tilt around X, then tilt around Z, then translate.
//! Normals use the same three rotations (no translation).

use crate::camera::Camera;
use crate::clip;
use crate::frame::Frame;
use crate::mesh::{self, Mesh};
use crate::raster::{self, draw_tri, SVert};
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

/// Draw every triangle of `inst.mesh`, transformed by `inst`, through
/// `cam`, into `frame`. Per triangle: transform + shade (world space) each
/// vertex, near-clip in view space (colors already computed, so clipping
/// just interpolates them alongside position), backface-cull the projected
/// triangle (`signed_area <= 0` — same convention/threshold `raster.rs`
/// itself re-checks), then rasterize.
pub fn draw_instance(frame: &mut Frame, cam: &Camera, inst: &Instance) {
    for tri in &inst.mesh.tris {
        let mut world = [Vec3::default(); 3];
        let mut world_n = [Vec3::default(); 3];
        let mut col = [Vec3::default(); 3];

        for i in 0..3 {
            let idx = tri[i] as usize;
            let local = inst.mesh.verts[idx];
            let local_n = inst.mesh.norms[idx];
            let w = rotate(local, inst.yaw, inst.tilt) + inst.offset;
            let wn = rotate(local_n, inst.yaw, inst.tilt);
            let view_dir = (cam.pos - w).normalize_or_zero();
            world[i] = w;
            world_n[i] = wn;
            col[i] = shade::shade(wn, view_dir, &inst.material);
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
            draw_tri(
                frame,
                sv(x0, y0, z0, t[0].1),
                sv(x1, y1, z1, t[1].1),
                sv(x2, y2, z2, t[2].1),
            );
        }
    }
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
        };
        let mut f = Frame::new(64, 64);
        draw_instance(&mut f, &cam, &inst);
        assert_eq!(painted_count(&f), 0);
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
