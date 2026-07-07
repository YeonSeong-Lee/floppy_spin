//! Deterministic 3D simulation: fixmath, physics, combat, AI, screen flow.
//! No unsafe, no libm transcendentals (SPEC §5), no wall-clock, no HashMap state.
#![forbid(unsafe_code)]

pub mod arena;
pub mod clock;
pub mod combat;
pub mod fixmath;
pub mod flow;
pub mod hash;
pub mod input;
pub mod minigame;
pub mod physics;
pub mod rng;
pub mod roster;
pub mod vec;
