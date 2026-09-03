//! Deterministic 3D simulation: fixmath, physics, combat, AI, screen flow.
//! No unsafe, no libm transcendentals (SPEC §5), no wall-clock, no HashMap state.
#![forbid(unsafe_code)]

pub mod ai;
pub mod arena;
pub mod clock;
pub mod combat;
pub mod domain;
pub mod fixmath;
pub mod flow;
pub mod garage;
pub mod hash;
pub mod input;
pub mod minigame;
pub mod physics;
pub mod rng;
pub mod roster;
pub mod save;
pub mod session;
pub mod vec;
