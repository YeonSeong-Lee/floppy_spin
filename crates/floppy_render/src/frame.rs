//! The framebuffer + depth buffer the whole renderer draws into.
//!
//! `px` is `0x00RRGGBB` (top byte unused/zero), one `u32` per pixel, row-major
//! (`y * w + x`) — this is what gets blitted via GDI `StretchDIBits` and what
//! headless mode encodes straight to PNG (SPEC §3).
//!
//! `depth` stores **`1/z`** (the camera-space inverse depth, see
//! `camera::Camera::project`), NOT raw `z` and NOT a normalized `[0,1]`
//! device depth:
//! - `0.0` means "far / cleared" (a `1/z` of `0` corresponds to `z ->
//!   infinity`, i.e. nothing has been drawn there yet, or the drawn surface
//!   is infinitely far away — practically: no surface).
//! - Larger values mean *nearer* to the camera (`1/z` grows as `z` shrinks
//!   toward the near plane), so the depth test is `new_inv_z > depth[i]`
//!   (strictly nearer wins ties keep the first-drawn surface).
//!
//! This is exactly the quantity that's already affine in screen space for a
//! perspective projection, so it interpolates correctly with plain
//! (non-perspective-corrected) screen-space barycentric weights — see
//! `raster.rs`.
pub struct Frame {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u32>,
    pub depth: Vec<f32>,
}

impl Frame {
    pub fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            px: vec![0u32; w * h],
            depth: vec![0.0f32; w * h],
        }
    }

    /// Fill every pixel with `color` and reset every depth entry to `0.0`
    /// (far/cleared, see module docs).
    pub fn clear(&mut self, color: u32) {
        for p in self.px.iter_mut() {
            *p = color;
        }
        for d in self.depth.iter_mut() {
            *d = 0.0;
        }
    }

    /// Bounds-checked pixel write: does nothing if `(x, y)` falls outside the
    /// frame (never panics, never wraps).
    pub fn set(&mut self, x: i32, y: i32, color: u32) {
        if x < 0 || y < 0 {
            return;
        }
        let (x, y) = (x as usize, y as usize);
        if x >= self.w || y >= self.h {
            return;
        }
        self.px[y * self.w + x] = color;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_frame_is_cleared_to_zero() {
        let f = Frame::new(4, 3);
        assert_eq!(f.px.len(), 12);
        assert_eq!(f.depth.len(), 12);
        assert!(f.px.iter().all(|&p| p == 0));
        assert!(f.depth.iter().all(|&d| d == 0.0));
    }

    #[test]
    fn clear_sets_color_and_resets_depth() {
        let mut f = Frame::new(2, 2);
        f.depth[0] = 5.0;
        f.clear(0x00112233);
        assert!(f.px.iter().all(|&p| p == 0x00112233));
        assert!(f.depth.iter().all(|&d| d == 0.0));
    }

    #[test]
    fn set_is_bounds_checked() {
        let mut f = Frame::new(2, 2);
        f.set(-1, 0, 0xFF);
        f.set(0, -1, 0xFF);
        f.set(2, 0, 0xFF);
        f.set(0, 2, 0xFF);
        assert!(f.px.iter().all(|&p| p == 0));
        f.set(1, 1, 0xABCDEF);
        assert_eq!(f.px[3], 0xABCDEF); // (x=1, y=1) in a 2-wide frame -> index 1*2+1
    }
}
