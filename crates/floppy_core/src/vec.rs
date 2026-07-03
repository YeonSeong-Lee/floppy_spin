//! 2D/3D vectors for the deterministic sim (SPEC §5). Every operation is a
//! plain, explicit expression evaluated in a fixed order — no fused
//! multiply-add, no iterator/SIMD tricks that could reorder floating point
//! operations and change rounding on some target. `length`/`normalize_or_zero`
//! route through `fixmath` (the only allowed source of `sqrt`/`rsqrt`).

use crate::fixmath;
use std::ops::{Add, Mul, Neg, Sub};

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec2 {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn dot(self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y
    }

    pub fn length_sq(self) -> f32 {
        self.dot(self)
    }

    pub fn length(self) -> f32 {
        fixmath::sqrt(self.length_sq())
    }

    /// Unit-length vector in the same direction, or the zero vector if `self`
    /// is (at or near) zero — never divides by zero, never produces `NaN`.
    pub fn normalize_or_zero(self) -> Self {
        let len_sq = self.length_sq();
        if len_sq <= 0.0 {
            return Self::default();
        }
        let inv_len = fixmath::rsqrt(len_sq);
        Self {
            x: self.x * inv_len,
            y: self.y * inv_len,
        }
    }

    pub fn lerp(self, other: Self, t: f32) -> Self {
        Self {
            x: self.x + (other.x - self.x) * t,
            y: self.y + (other.y - self.y) * t,
        }
    }

    pub fn scaled(self, s: f32) -> Self {
        Self {
            x: self.x * s,
            y: self.y * s,
        }
    }

    pub fn min(self, other: Self) -> Self {
        Self {
            x: self.x.min(other.x),
            y: self.y.min(other.y),
        }
    }

    pub fn max(self, other: Self) -> Self {
        Self {
            x: self.x.max(other.x),
            y: self.y.max(other.y),
        }
    }

    pub fn clamp(self, lo: Self, hi: Self) -> Self {
        Self {
            x: self.x.clamp(lo.x, hi.x),
            y: self.y.clamp(lo.y, hi.y),
        }
    }
}

impl Add for Vec2 {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self {
            x: self.x + rhs.x,
            y: self.y + rhs.y,
        }
    }
}

impl Sub for Vec2 {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self {
            x: self.x - rhs.x,
            y: self.y - rhs.y,
        }
    }
}

impl Neg for Vec2 {
    type Output = Self;
    fn neg(self) -> Self {
        Self {
            x: -self.x,
            y: -self.y,
        }
    }
}

impl Mul<f32> for Vec2 {
    type Output = Self;
    fn mul(self, rhs: f32) -> Self {
        Self {
            x: self.x * rhs,
            y: self.y * rhs,
        }
    }
}

impl Vec3 {
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    pub fn dot(self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    pub fn cross(self, other: Self) -> Self {
        Self {
            x: self.y * other.z - self.z * other.y,
            y: self.z * other.x - self.x * other.z,
            z: self.x * other.y - self.y * other.x,
        }
    }

    pub fn length_sq(self) -> f32 {
        self.dot(self)
    }

    pub fn length(self) -> f32 {
        fixmath::sqrt(self.length_sq())
    }

    /// Unit-length vector in the same direction, or the zero vector if `self`
    /// is (at or near) zero — never divides by zero, never produces `NaN`.
    pub fn normalize_or_zero(self) -> Self {
        let len_sq = self.length_sq();
        if len_sq <= 0.0 {
            return Self::default();
        }
        let inv_len = fixmath::rsqrt(len_sq);
        Self {
            x: self.x * inv_len,
            y: self.y * inv_len,
            z: self.z * inv_len,
        }
    }

    pub fn lerp(self, other: Self, t: f32) -> Self {
        Self {
            x: self.x + (other.x - self.x) * t,
            y: self.y + (other.y - self.y) * t,
            z: self.z + (other.z - self.z) * t,
        }
    }

    pub fn scaled(self, s: f32) -> Self {
        Self {
            x: self.x * s,
            y: self.y * s,
            z: self.z * s,
        }
    }

    pub fn min(self, other: Self) -> Self {
        Self {
            x: self.x.min(other.x),
            y: self.y.min(other.y),
            z: self.z.min(other.z),
        }
    }

    pub fn max(self, other: Self) -> Self {
        Self {
            x: self.x.max(other.x),
            y: self.y.max(other.y),
            z: self.z.max(other.z),
        }
    }

    pub fn clamp(self, lo: Self, hi: Self) -> Self {
        Self {
            x: self.x.clamp(lo.x, hi.x),
            y: self.y.clamp(lo.y, hi.y),
            z: self.z.clamp(lo.z, hi.z),
        }
    }
}

impl Add for Vec3 {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self {
            x: self.x + rhs.x,
            y: self.y + rhs.y,
            z: self.z + rhs.z,
        }
    }
}

impl Sub for Vec3 {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self {
            x: self.x - rhs.x,
            y: self.y - rhs.y,
            z: self.z - rhs.z,
        }
    }
}

impl Neg for Vec3 {
    type Output = Self;
    fn neg(self) -> Self {
        Self {
            x: -self.x,
            y: -self.y,
            z: -self.z,
        }
    }
}

impl Mul<f32> for Vec3 {
    type Output = Self;
    fn mul(self, rhs: f32) -> Self {
        Self {
            x: self.x * rhs,
            y: self.y * rhs,
            z: self.z * rhs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_or_zero_unit_length() {
        let v = Vec3::new(3.0, 4.0, 0.0).normalize_or_zero();
        let len = v.length();
        assert!((len - 1.0).abs() < 1e-3, "len={len}");
    }

    #[test]
    fn normalize_or_zero_zero_input_is_exact_zero() {
        let v = Vec3::default().normalize_or_zero();
        assert_eq!(v, Vec3::new(0.0, 0.0, 0.0));

        let v2 = Vec2::default().normalize_or_zero();
        assert_eq!(v2, Vec2::new(0.0, 0.0));
    }

    #[test]
    fn cross_is_perpendicular_to_both_inputs() {
        let a = Vec3::new(1.0, 2.0, 3.0);
        let b = Vec3::new(-3.0, 0.5, 2.0);
        let c = a.cross(b);
        assert!(c.dot(a).abs() < 1e-3, "c.dot(a)={}", c.dot(a));
        assert!(c.dot(b).abs() < 1e-3, "c.dot(b)={}", c.dot(b));
    }

    #[test]
    fn lerp_endpoints_and_midpoint() {
        let a = Vec3::new(0.0, 0.0, 0.0);
        let b = Vec3::new(2.0, 4.0, 6.0);
        assert_eq!(a.lerp(b, 0.0), a);
        assert_eq!(a.lerp(b, 1.0), b);
        assert_eq!(a.lerp(b, 0.5), Vec3::new(1.0, 2.0, 3.0));
    }

    #[test]
    fn componentwise_min_max_clamp() {
        let a = Vec2::new(1.0, 5.0);
        let b = Vec2::new(3.0, 2.0);
        assert_eq!(a.min(b), Vec2::new(1.0, 2.0));
        assert_eq!(a.max(b), Vec2::new(3.0, 5.0));
        let clamped = Vec2::new(10.0, -10.0).clamp(Vec2::new(0.0, 0.0), Vec2::new(5.0, 5.0));
        assert_eq!(clamped, Vec2::new(5.0, 0.0));
    }
}
