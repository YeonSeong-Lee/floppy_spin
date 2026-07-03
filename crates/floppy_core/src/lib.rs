//! Deterministic 3D simulation: fixmath, physics, combat, AI, screen flow.
//! No unsafe, no libm transcendentals (SPEC §5), no wall-clock, no HashMap state.
#![forbid(unsafe_code)]

pub mod arena;
pub mod clock;
pub mod fixmath;
pub mod hash;
pub mod input;
pub mod physics;
pub mod rng;
pub mod vec;
