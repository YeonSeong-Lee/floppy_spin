//! M3-B integrator: wires `floppy_core::physics`/`arena`/`roster` into a
//! real battle scene — fixed whole-arena camera, the arena bowl + neon ring
//! bands, per-silhouette lathed tops (dark metal body / emissive accent
//! flange), and airborne shadow blobs.

use crate::camera::Camera;
use crate::frame::Frame;
use crate::mesh::{self, Mesh};
use crate::scene::{self, Instance};
use crate::shade::Material;
use floppy_core::arena;
use floppy_core::physics::{self, World};
use floppy_core::roster::{Preset, Silhouette};
use floppy_core::vec::{Vec2, Vec3};

/// Fixed whole-arena camera (task spec's suggested `(0, 12.5, -11.5)` looking
/// at `(0, 0.8, 0)` at 55 degrees did NOT clear the near rim within the
/// frame — see `tests::camera_frames_the_whole_arena_...` below and the
/// module's verification history: the near-side rim point at `r =
/// ARENA_RADIUS, z < 0` projected to `sy ~= 585` against a 540px-tall frame.
/// Re-derived by grid search over `(pos.y, pos.z, fov)` for the configuration
/// with the largest worst-case margin across every hard constraint while
/// keeping `fov` at the spec's original 55 degrees (a bird's-eye framing,
/// not a flattened telephoto shot) — `(0, 9.0, -13.0)` clears every
/// constraint with >= 7px of slack (see the frozen test below for exact
/// numbers).
pub const CAM_POS: Vec3 = Vec3::new(0.0, 9.0, -13.0);
pub const CAM_TARGET: Vec3 = Vec3::new(0.0, 0.8, 0.0);
pub const CAM_FOV_DEG: f32 = 55.0;

fn make_camera(w: usize, h: usize) -> Camera {
    Camera::look_at(CAM_POS, CAM_TARGET, CAM_FOV_DEG, w, h)
}

/// Clear color (game_design.md §6 "void" palette entry).
const CLEAR_COLOR: u32 = 0x000A_0A14;

/// Segments swept around Y for each lathed top mesh (task spec: "20-28").
const TOP_SEGMENTS: usize = 22;
/// Fraction of a top's (normalized, 0..1) profile height above which a
/// vertex gets the emissive accent material instead of the dark metal base
/// (task spec: "verts above 60% height").
const EMISSIVE_HEIGHT_FRAC: f32 = 0.6;

/// Dark neutral metal tint (game_design.md §6 "bowl metal" family) blended
/// toward each preset's accent for the top's base material.
const DARK_METAL: Vec3 = Vec3::new(0.10, 0.11, 0.14);
/// How strongly the base material leans toward the preset accent.
const ACCENT_TINT: f32 = 0.15;
/// Emissive intensity multiplier applied to the accent color for the upper
/// flange material.
const ACCENT_EMISSIVE_GAIN: f32 = 0.9;
/// Shared "chrome read" specular params (game_design.md §6), matching the
/// M3-A demo top material.
const TOP_SPEC_POWER: u32 = 32;
const TOP_SPEC_STRENGTH: f32 = 0.6;

const ARENA_MATERIAL: Material = Material {
    base: Vec3::new(0.08, 0.09, 0.15),
    emissive: Vec3::new(0.0, 0.0, 0.0),
    spec_power_i: 8,
    spec_strength: 0.15,
};

/// Neon ring band radii (task spec: "r = 2, 3.5, 5, 6.5").
const RING_RADII: [f32; 4] = [2.0, 3.5, 5.0, 6.5];
const RING_HALF_WIDTH: f32 = 0.08;
/// Ring bands follow terrain height plus this offset (task spec: "+ 0.02").
const RING_Y_OFFSET: f32 = 0.02;
const RING_SEGMENTS: usize = 28;
const RING_MATERIAL: Material = Material {
    base: Vec3::new(0.0, 0.0, 0.0),
    emissive: Vec3::new(0.0, 0.35, 0.45),
    spec_power_i: 1,
    spec_strength: 0.0,
};

/// Shadow blob (task spec: "flattened dark disc (12-tri fan)").
const SHADOW_SEGMENTS: usize = 12;
const SHADOW_RADIUS: f32 = 0.6;
const SHADOW_Y_OFFSET: f32 = 0.02;
const SHADOW_MATERIAL: Material = Material {
    base: Vec3::new(0.02, 0.02, 0.03),
    emissive: Vec3::new(0.0, 0.0, 0.0),
    spec_power_i: 1,
    spec_strength: 0.0,
};

/// Arena bowl tessellation (task spec: "36 segments x 14 rings").
const ARENA_RINGS: usize = 14;
const ARENA_SEGMENTS: usize = 36;

/// Per-silhouette body-of-revolution profile: `(radius, y)` pairs, bottom
/// (contact tip) to top, normalized to `radius <= 1.0` / `y` in `[0, 1]`.
/// Scaled to a top's real `radius`/`height` at draw time via
/// `Instance::radial_scale`/`height_scale` (see `scene.rs`).
///
/// Riptide's "skewed teardrop, asymmetric flare" is approximated within a
/// body of revolution — a lathed mesh cannot be genuinely asymmetric about
/// its own axis; the flare position/size is captured, the "skew" is not
/// (documented limitation, task spec's own caveat).
fn profile_for(s: Silhouette) -> &'static [(f32, f32)] {
    match s {
        // Cleaver: narrow shaft, wide flat flange at 0.8h with a thin razor
        // lip, closing back to a point.
        Silhouette::Cleaver => &[
            (0.00, 0.00),
            (0.10, 0.04),
            (0.16, 0.20),
            (0.20, 0.45),
            (0.24, 0.70),
            (0.95, 0.78),
            (1.00, 0.80),
            (0.55, 0.83),
            (0.30, 0.92),
            (0.00, 1.00),
        ],
        // Bulwark: squat dome, rounded shoulders widest at mid-height.
        Silhouette::Bulwark => &[
            (0.00, 0.00),
            (0.35, 0.05),
            (0.70, 0.15),
            (0.92, 0.30),
            (1.00, 0.50),
            (0.90, 0.72),
            (0.60, 0.90),
            (0.00, 1.00),
        ],
        // Everspin: sharp low tip, then a smooth, nearly-constant narrow
        // cylinder for most of the height.
        Silhouette::Everspin => &[
            (0.00, 0.00),
            (0.06, 0.03),
            (0.28, 0.08),
            (0.35, 0.15),
            (0.38, 0.50),
            (0.38, 0.85),
            (0.30, 0.95),
            (0.00, 1.00),
        ],
        // Keystone: balanced trapezoid.
        Silhouette::Keystone => &[
            (0.00, 0.00),
            (0.15, 0.05),
            (0.55, 0.25),
            (0.70, 0.45),
            (0.70, 0.65),
            (0.55, 0.82),
            (0.25, 0.95),
            (0.00, 1.00),
        ],
        // Riptide: teardrop with a flare at 0.65h (asymmetry not
        // representable, see function doc).
        Silhouette::Riptide => &[
            (0.00, 0.00),
            (0.08, 0.04),
            (0.20, 0.20),
            (0.30, 0.45),
            (0.85, 0.62),
            (0.95, 0.65),
            (0.45, 0.72),
            (0.20, 0.85),
            (0.00, 1.00),
        ],
        // Gravewell: inverted bell, widest near the base (0.2h).
        Silhouette::Gravewell => &[
            (0.00, 0.00),
            (0.55, 0.05),
            (0.92, 0.12),
            (1.00, 0.20),
            (0.85, 0.35),
            (0.60, 0.55),
            (0.42, 0.75),
            (0.25, 0.92),
            (0.00, 1.00),
        ],
        // Mirrorfang: hourglass, pinched at 0.5h, mirrored twin flares.
        Silhouette::Mirrorfang => &[
            (0.00, 0.00),
            (0.55, 0.06),
            (0.85, 0.25),
            (0.55, 0.35),
            (0.30, 0.50),
            (0.55, 0.65),
            (0.85, 0.75),
            (0.55, 0.94),
            (0.00, 1.00),
        ],
    }
}

/// Radial lobe count/depth per silhouette (game_design.md §3's per-top
/// scallop/facet/hook descriptions); `(0, 0.0)` means "no lobing" (plain
/// `lathe`).
fn lobes_for(s: Silhouette) -> (u32, f32) {
    match s {
        Silhouette::Cleaver => (6, 0.18),   // "6-lobe sawblade scallop"
        Silhouette::Bulwark => (12, 0.06),  // "12 shallow glancing lobes"
        Silhouette::Everspin => (0, 0.0),   // "smooth"
        Silhouette::Keystone => (8, 0.05),  // "mild 8-facet"
        Silhouette::Riptide => (3, 0.15),   // "3-lobe hooks"
        Silhouette::Gravewell => (2, 0.05), // "monolithic 2-lobe"
        Silhouette::Mirrorfang => (0, 0.0), // twin flares come from the profile itself
    }
}

fn build_top_mesh(s: Silhouette) -> Mesh {
    let profile = profile_for(s);
    let (lobes, depth) = lobes_for(s);
    if lobes == 0 {
        mesh::lathe(profile, TOP_SEGMENTS)
    } else {
        mesh::lathe_lobed(profile, TOP_SEGMENTS, lobes, depth)
    }
}

fn accent_to_vec3(accent: u32) -> Vec3 {
    let r = ((accent >> 16) & 0xFF) as f32 / 255.0;
    let g = ((accent >> 8) & 0xFF) as f32 / 255.0;
    let b = (accent & 0xFF) as f32 / 255.0;
    Vec3::new(r, g, b)
}

/// Dark metal base tinted slightly toward the preset's accent (task spec).
fn base_material_for(accent: u32) -> Material {
    let tint = accent_to_vec3(accent);
    Material {
        base: DARK_METAL.lerp(tint, ACCENT_TINT),
        emissive: Vec3::new(0.0, 0.0, 0.0),
        spec_power_i: TOP_SPEC_POWER,
        spec_strength: TOP_SPEC_STRENGTH,
    }
}

/// Emissive accent material for the upper flange region (task spec).
fn emissive_material_for(accent: u32) -> Material {
    let tint = accent_to_vec3(accent);
    Material {
        base: DARK_METAL.lerp(tint, ACCENT_TINT),
        emissive: tint * ACCENT_EMISSIVE_GAIN,
        spec_power_i: TOP_SPEC_POWER,
        spec_strength: TOP_SPEC_STRENGTH,
    }
}

/// Cached, once-built meshes for a whole battle (task spec: "cached meshes:
/// arena, rings, per-silhouette tops"). Materials are cheap plain-data
/// values recomputed per `draw` call from each preset's accent, so they are
/// not cached here.
pub struct BattleScene {
    arena_mesh: Mesh,
    ring_meshes: [Mesh; 4],
    top_meshes: [Mesh; 7],
    shadow_mesh: Mesh,
}

impl Default for BattleScene {
    fn default() -> Self {
        Self::new()
    }
}

impl BattleScene {
    pub fn new() -> Self {
        let arena_mesh = mesh::bowl(ARENA_RINGS, ARENA_SEGMENTS);
        let ring_meshes = RING_RADII.map(|r| {
            mesh::ring_band(
                arena::height,
                r,
                RING_HALF_WIDTH,
                RING_Y_OFFSET,
                RING_SEGMENTS,
            )
        });
        let top_meshes = [
            build_top_mesh(Silhouette::Cleaver),
            build_top_mesh(Silhouette::Bulwark),
            build_top_mesh(Silhouette::Everspin),
            build_top_mesh(Silhouette::Keystone),
            build_top_mesh(Silhouette::Riptide),
            build_top_mesh(Silhouette::Gravewell),
            build_top_mesh(Silhouette::Mirrorfang),
        ];
        let shadow_mesh = mesh::bowl_from(
            |_, _| 0.0,
            |_, _| Vec3::new(0.0, 1.0, 0.0),
            1,
            SHADOW_SEGMENTS,
            SHADOW_RADIUS,
        );
        Self {
            arena_mesh,
            ring_meshes,
            top_meshes,
            shadow_mesh,
        }
    }

    fn top_mesh(&self, s: Silhouette) -> &Mesh {
        &self.top_meshes[s as usize]
    }

    /// Total triangle count across every cached mesh (arena + 4 rings + 7
    /// top silhouettes + the shadow blob) — reported once at startup /
    /// in tests to track SPEC §10's "hundreds of tris" budget.
    pub fn total_tri_count(&self) -> usize {
        let arena = self.arena_mesh.tris.len();
        let rings: usize = self.ring_meshes.iter().map(|m| m.tris.len()).sum();
        let tops: usize = self.top_meshes.iter().map(|m| m.tris.len()).sum();
        let shadow = self.shadow_mesh.tris.len();
        arena + rings + tops + shadow
    }

    /// Draw one interpolated frame (SPEC §5): `world_prev`/`world_curr` are
    /// the two most recent fixed-step sim states, `alpha` blends them
    /// (headless goldens always pass `1.0`), `visuals` is each top's roster
    /// preset (silhouette + accent) in the same index order as
    /// `World::tops`.
    pub fn draw(
        &self,
        frame: &mut Frame,
        world_prev: &World,
        world_curr: &World,
        alpha: f32,
        visuals: [&Preset; 2],
    ) {
        frame.clear(CLEAR_COLOR);
        let cam = make_camera(frame.w, frame.h);

        let arena_inst = Instance {
            mesh: &self.arena_mesh,
            offset: Vec3::default(),
            yaw: 0.0,
            tilt: Vec2::default(),
            material: ARENA_MATERIAL,
            radial_scale: 1.0,
            height_scale: 1.0,
        };
        scene::draw_instance(frame, &cam, &arena_inst);

        for ring_mesh in &self.ring_meshes {
            let ring_inst = Instance {
                mesh: ring_mesh,
                offset: Vec3::default(),
                yaw: 0.0,
                tilt: Vec2::default(),
                material: RING_MATERIAL,
                radial_scale: 1.0,
                height_scale: 1.0,
            };
            scene::draw_instance_additive(frame, &cam, &ring_inst);
        }

        for (i, &preset) in visuals.iter().enumerate() {
            let prev = &world_prev.tops[i];
            let curr = &world_curr.tops[i];
            let pose = physics::pose_lerp(prev, curr, alpha);
            let top_mesh = self.top_mesh(preset.silhouette);
            let base_material = base_material_for(preset.accent);
            let emissive_material = emissive_material_for(preset.accent);

            let top_inst = Instance {
                mesh: top_mesh,
                offset: pose.pos,
                yaw: pose.spin_angle,
                tilt: pose.tilt,
                material: base_material,
                radial_scale: pose.radius,
                height_scale: pose.height,
            };
            scene::draw_instance_split(frame, &cam, &top_inst, &emissive_material, |idx| {
                top_mesh.verts[idx].y > EMISSIVE_HEIGHT_FRAC
            });

            // Shadow blob under airborne tops only (task spec: "skip when
            // grounded"). `curr.grounded` is the authoritative discrete
            // state for this render (a bool has no meaningful lerp).
            if !curr.grounded {
                let ground_y = arena::height(pose.pos.x, pose.pos.z) + SHADOW_Y_OFFSET;
                let shadow_inst = Instance {
                    mesh: &self.shadow_mesh,
                    offset: Vec3::new(pose.pos.x, ground_y, pose.pos.z),
                    yaw: 0.0,
                    tilt: Vec2::default(),
                    material: SHADOW_MATERIAL,
                    radial_scale: 1.0,
                    height_scale: 1.0,
                };
                scene::draw_instance(frame, &cam, &shadow_inst);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use floppy_core::combat::SpecialId;
    use floppy_core::fixmath;
    use floppy_core::physics::{LaunchParams, Stats};
    use floppy_core::roster::PRESETS;

    const W: usize = 960;
    const H: usize = 540;
    const MARGIN: f32 = 20.0;

    fn keystone_stats() -> Stats {
        Stats {
            atk: 52,
            def: 54,
            sta: 52,
            wgt: 50,
            spd: 50,
            mtr: 44,
        }
    }

    /// FROZEN camera verification (task spec): the whole arena rim, a point
    /// 4m above the bowl center, and a centered top's on-screen width must
    /// all clear their required margins. See `CAM_POS`/`CAM_TARGET`/
    /// `CAM_FOV_DEG`'s doc comment for how these were derived.
    #[test]
    fn camera_frames_the_whole_arena_and_a_centered_top_with_required_margins() {
        let cam = make_camera(W, H);

        for i in 0..32 {
            let theta = std::f32::consts::TAU * i as f32 / 32.0;
            let x = fixmath::cos(theta) * arena::ARENA_RADIUS;
            let z = fixmath::sin(theta) * arena::ARENA_RADIUS;
            let y = arena::height(x, z);
            let p = Vec3::new(x, y, z);
            let v = cam.to_view(p);
            assert!(
                v.z > Camera::NEAR,
                "rim point at theta={theta} is behind the near plane"
            );
            let (sx, sy, _) = cam.project(v);
            assert!(
                (MARGIN..=W as f32 - MARGIN).contains(&sx),
                "rim sx={sx} out of bounds at theta={theta}"
            );
            assert!(
                (MARGIN..=H as f32 - MARGIN).contains(&sy),
                "rim sy={sy} out of bounds at theta={theta}"
            );
        }

        let apex = Vec3::new(0.0, 4.0, 0.0);
        let v = cam.to_view(apex);
        let (sx, sy, _) = cam.project(v);
        assert!(
            (MARGIN..=W as f32 - MARGIN).contains(&sx),
            "apex sx={sx} out of bounds"
        );
        assert!(
            (MARGIN..=H as f32 - MARGIN).contains(&sy),
            "apex sy={sy} out of bounds"
        );

        let ground_y = arena::height(0.0, 0.0);
        let e0 = cam.to_view(Vec3::new(-0.95, ground_y, 0.0));
        let e1 = cam.to_view(Vec3::new(0.95, ground_y, 0.0));
        let (sx0, _, _) = cam.project(e0);
        let (sx1, _, _) = cam.project(e1);
        let width = (sx1 - sx0).abs();
        assert!(width >= 55.0, "centered top width {width} < 55px");
    }

    #[test]
    fn every_silhouette_builds_a_nonempty_mesh_with_no_lobe_zero_and_in_bounds_indices() {
        for s in [
            Silhouette::Cleaver,
            Silhouette::Bulwark,
            Silhouette::Everspin,
            Silhouette::Keystone,
            Silhouette::Riptide,
            Silhouette::Gravewell,
            Silhouette::Mirrorfang,
        ] {
            let m = build_top_mesh(s);
            assert!(!m.verts.is_empty(), "{s:?} produced an empty mesh");
            assert!(!m.tris.is_empty(), "{s:?} produced zero triangles");
            let n = m.verts.len() as u16;
            for tri in &m.tris {
                for &idx in tri {
                    assert!(idx < n, "{s:?}: idx {idx} out of bounds (n={n})");
                }
            }
            // Every profile is authored with radius <= 1.0 and y in [0, 1];
            // lobing only ever SHRINKS the radius (factor <= 1.0), so no
            // vertex should ever exceed radius 1.0 + a tiny slack.
            for v in &m.verts {
                let r = fixmath::sqrt(v.x * v.x + v.z * v.z);
                assert!(r <= 1.01, "{s:?}: radius {r} exceeds normalized bound");
                assert!(
                    (-0.01..=1.01).contains(&v.y),
                    "{s:?}: y {} outside normalized [0,1]",
                    v.y
                );
            }
        }
    }

    #[test]
    fn total_tri_count_is_reported_and_nonzero() {
        let scene = BattleScene::new();
        let total = scene.total_tri_count();
        assert!(total > 0);
        // Not a hard SPEC gate (measured/reported in the milestone summary
        // instead), but a loud regression guard against an accidental
        // explosion in tessellation.
        assert!(total < 5000, "total_tri_count={total} looks runaway");
    }

    fn preset_for(silhouette: Silhouette) -> &'static Preset {
        PRESETS
            .iter()
            .find(|p| p.silhouette == silhouette)
            .expect("every Silhouette has exactly one preset")
    }

    fn launch_world() -> World {
        let params = [
            LaunchParams {
                heading: 0.0,
                depth: 0.7,
                power: 0.6,
                quality: 1.0,
                spin_dir: 1,
                stats: keystone_stats(),
                special_id: SpecialId::Overclock,
            },
            LaunchParams {
                heading: std::f32::consts::PI,
                depth: 0.7,
                power: 0.55,
                quality: 1.08,
                spin_dir: -1,
                stats: keystone_stats(),
                special_id: SpecialId::Overclock,
            },
        ];
        World::launch(42, params)
    }

    #[test]
    fn draw_paints_a_reasonable_fraction_of_the_frame_without_panicking() {
        let scene = BattleScene::new();
        let world = launch_world();
        let mut frame = Frame::new(W, H);
        let visuals = [
            preset_for(Silhouette::Cleaver),
            preset_for(Silhouette::Bulwark),
        ];
        scene.draw(&mut frame, &world, &world, 1.0, visuals);
        let non_bg = frame.px.iter().filter(|&&p| p != CLEAR_COLOR).count();
        assert!(
            non_bg > 2000,
            "expected a visible arena+tops, got {non_bg} non-background pixels"
        );
    }

    #[test]
    fn draw_does_not_panic_across_a_short_scripted_sim() {
        let scene = BattleScene::new();
        let mut world = launch_world();
        let mut frame = Frame::new(W, H);
        let visuals = [
            preset_for(Silhouette::Riptide),
            preset_for(Silhouette::Gravewell),
        ];
        for _ in 0..30 {
            let prev = world.clone();
            world.step([Default::default(), Default::default()]);
            scene.draw(&mut frame, &prev, &world, 0.5, visuals);
        }
    }

    #[test]
    fn shadow_blob_is_skipped_when_grounded_and_present_when_airborne() {
        let scene = BattleScene::new();
        let mut world = launch_world();
        // Force top 0 airborne, top 1 grounded, both directly under the
        // camera-facing center so any painted shadow pixels are easy to spot.
        world.tops[0].pos = Vec3::new(-2.0, arena::height(-2.0, 0.0) + 3.0, 0.0);
        world.tops[0].grounded = false;
        world.tops[1].pos = Vec3::new(2.0, arena::height(2.0, 0.0), 0.0);
        world.tops[1].grounded = true;

        let mut frame_air = Frame::new(W, H);
        let visuals = [
            preset_for(Silhouette::Everspin),
            preset_for(Silhouette::Keystone),
        ];
        scene.draw(&mut frame_air, &world, &world, 1.0, visuals);

        // Ground both and redraw with everything else held constant except
        // `grounded`. The shadow blob sits over the (already non-background)
        // arena floor, so a raw "painted pixel count" comparison is blind to
        // it — hash the frames instead to catch the actual color change.
        let mut world_grounded = world.clone();
        world_grounded.tops[0].grounded = true;
        let mut frame_grounded = Frame::new(W, H);
        scene.draw(
            &mut frame_grounded,
            &world_grounded,
            &world_grounded,
            1.0,
            visuals,
        );

        let hash_air = floppy_core::hash::hash_u32s(&frame_air.px);
        let hash_grounded = floppy_core::hash::hash_u32s(&frame_grounded.px);
        assert_ne!(
            hash_air, hash_grounded,
            "expected the shadow blob to change the rendered frame"
        );
    }
}
