//! Pinhole camera: view-space transform + perspective projection. Basis
//! vectors are precomputed once in [`Camera::look_at`] (per-triangle/vertex
//! calls only do dot products), and the `fov -> focal length` conversion
//! goes through `fixmath::sin`/`fixmath::cos` (never `f32::tan`, which
//! doesn't exist as a direct libm call point but is deliberately avoided per
//! the M3-A brief: everything trigonometric routes through `fixmath`).

use floppy_core::fixmath;
use floppy_core::vec::Vec3;

pub struct Camera {
    pub pos: Vec3,
    right: Vec3,
    up: Vec3,
    fwd: Vec3,
    focal: f32,
    half_w: f32,
    half_h: f32,
}

impl Camera {
    /// Near clip plane distance (view-space `z`), shared with `clip.rs` /
    /// `scene.rs`.
    pub const NEAR: f32 = 0.5;

    /// Build a camera at `pos` looking at `target`, with vertical field of
    /// view `fov_y_deg` degrees, targeting a `w x h` framebuffer.
    ///
    /// Basis: `fwd = normalize(target - pos)`, `right = normalize(fwd x
    /// world_up)` (`world_up = (0,1,0)`), `up = right x fwd` (already unit
    /// since `right` and `fwd` are unit and perpendicular by construction).
    /// `focal = half_h / tan(fov_y/2)`, with `tan` computed as
    /// `sin/cos` via `fixmath` (guarding `cos` near zero for a
    /// pathological >= 180 degree FOV rather than dividing by ~0).
    pub fn look_at(pos: Vec3, target: Vec3, fov_y_deg: f32, w: usize, h: usize) -> Camera {
        let fwd = (target - pos).normalize_or_zero();
        let world_up = Vec3::new(0.0, 1.0, 0.0);
        let right = fwd.cross(world_up).normalize_or_zero();
        let up = right.cross(fwd);

        let half_h = h as f32 * 0.5;
        let half_w = w as f32 * 0.5;

        const DEG_TO_RAD: f32 = std::f32::consts::PI / 180.0;
        let half_fov_rad = fov_y_deg * DEG_TO_RAD * 0.5;
        let s = fixmath::sin(half_fov_rad);
        let c = fixmath::cos(half_fov_rad);
        const MIN_COS: f32 = 1e-4;
        let c_safe = if c.abs() < MIN_COS {
            if c < 0.0 {
                -MIN_COS
            } else {
                MIN_COS
            }
        } else {
            c
        };
        let t_half = s / c_safe;
        let focal = if t_half.abs() > 1e-8 {
            half_h / t_half
        } else {
            half_h / 1e-8
        };

        Camera {
            pos,
            right,
            up,
            fwd,
            focal,
            half_w,
            half_h,
        }
    }

    /// World-space point `p` -> view space: `(dot(d,right), dot(d,up),
    /// dot(d,fwd))` where `d = p - pos`. `+z` is "forward, into the screen".
    pub fn to_view(&self, p: Vec3) -> Vec3 {
        let d = p - self.pos;
        Vec3::new(d.dot(self.right), d.dot(self.up), d.dot(self.fwd))
    }

    /// Project a view-space point (`v.z` must be `> 0`, i.e. in front of the
    /// camera — callers clip against `Camera::NEAR` first) to screen space:
    /// `(sx, sy, inv_z)`. `sy` subtracts so that `+y` (up in view space) maps
    /// to a smaller (higher on screen) `sy`, matching the top-left-origin,
    /// y-down pixel convention `frame.rs`/`raster.rs` use. `inv_z = 1/v.z`
    /// doubles as the depth-buffer value (see `frame.rs` docs).
    pub fn project(&self, v: Vec3) -> (f32, f32, f32) {
        let sx = self.half_w + v.x * self.focal / v.z;
        let sy = self.half_h - v.y * self.focal / v.z;
        (sx, sy, 1.0 / v.z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looking_down_positive_z_axis_has_expected_basis() {
        let cam = Camera::look_at(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
            60.0,
            960,
            540,
        );
        // fwd should be +z.
        let v = cam.to_view(Vec3::new(0.0, 0.0, 1.0));
        assert!((v.z - 1.0).abs() < 1e-4, "v={v:?}");
        assert!(v.x.abs() < 1e-4);
        assert!(v.y.abs() < 1e-4);
    }

    #[test]
    fn center_point_projects_to_screen_center() {
        let cam = Camera::look_at(
            Vec3::new(0.0, 0.0, -5.0),
            Vec3::new(0.0, 0.0, 0.0),
            60.0,
            960,
            540,
        );
        let v = cam.to_view(Vec3::new(0.0, 0.0, 0.0));
        let (sx, sy, inv_z) = cam.project(v);
        assert!((sx - 480.0).abs() < 1e-3, "sx={sx}");
        assert!((sy - 270.0).abs() < 1e-3, "sy={sy}");
        assert!(inv_z > 0.0);
    }

    #[test]
    fn moving_up_in_world_moves_up_on_screen() {
        let cam = Camera::look_at(
            Vec3::new(0.0, 0.0, -5.0),
            Vec3::new(0.0, 0.0, 0.0),
            60.0,
            960,
            540,
        );
        let v_center = cam.to_view(Vec3::new(0.0, 0.0, 0.0));
        let v_up = cam.to_view(Vec3::new(0.0, 1.0, 0.0));
        let (_, sy_center, _) = cam.project(v_center);
        let (_, sy_up, _) = cam.project(v_up);
        assert!(sy_up < sy_center, "sy_up={sy_up} sy_center={sy_center}");
    }

    #[test]
    fn wider_fov_gives_smaller_focal_length() {
        let narrow = Camera::look_at(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
            30.0,
            960,
            540,
        );
        let wide = Camera::look_at(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
            120.0,
            960,
            540,
        );
        assert!(narrow.focal > wide.focal);
    }
}
