//! M1 placeholder determinism harness (SPEC §5, §12). `floppy_core` has no
//! real sim yet — this file exercises the exact pattern the real one will
//! use from M2 onward: seed -> Rng -> fixmath-driven stepping ->
//! `InputState`-scripted impulses -> `hash_u32s` over the raw f32 state bits.
//! Delete this file once the real M2 sim ships with its own determinism test
//! covering the same property.

use floppy_core::clock::SIM_DT;
use floppy_core::fixmath;
use floppy_core::hash::hash_u32s;
use floppy_core::input::InputState;
use floppy_core::rng::Rng;
use floppy_core::vec::Vec3;

const NUM_PARTICLES: usize = 6;
const BOX_HALF: f32 = 5.0;
const STEPS: u32 = 600;

struct Particle {
    pos: Vec3,
    vel: Vec3,
}

struct PlaceholderSim {
    particles: [Particle; NUM_PARTICLES],
}

impl PlaceholderSim {
    fn new(seed: u64) -> Self {
        let mut rng = Rng::new(seed);
        let particles = std::array::from_fn(|_| {
            let pos = Vec3::new(
                (rng.next_f32() - 0.5) * BOX_HALF,
                (rng.next_f32() - 0.5) * BOX_HALF,
                (rng.next_f32() - 0.5) * BOX_HALF,
            );
            let vel = Vec3::new(
                (rng.next_f32() - 0.5) * 2.0,
                (rng.next_f32() - 0.5) * 2.0,
                (rng.next_f32() - 0.5) * 2.0,
            );
            Particle { pos, vel }
        });
        Self { particles }
    }

    /// One fixed 120 Hz step: apply an input-driven impulse to particle 0,
    /// a fixmath-driven "wind" term to every particle, integrate, and bounce
    /// off an axis-aligned box. Entities are iterated by fixed index order
    /// only (SPEC §5) — no HashMap, no wall-clock.
    fn step(&mut self, input: InputState) {
        let impulse = Vec3::new(
            input.dir_x as f32,
            if input.hop { 1.0 } else { 0.0 },
            input.dir_y as f32,
        )
        .scaled(0.3);

        for (i, p) in self.particles.iter_mut().enumerate() {
            if i == 0 {
                p.vel = p.vel + impulse;
            }
            let wind = fixmath::sin(p.pos.x + p.pos.z) * 0.05;
            p.vel.y -= (0.2 + wind) * SIM_DT;
            p.pos = p.pos + p.vel.scaled(SIM_DT);

            bounce(&mut p.pos.x, &mut p.vel.x);
            bounce(&mut p.pos.y, &mut p.vel.y);
            bounce(&mut p.pos.z, &mut p.vel.z);
        }
    }

    fn state_hash(&self) -> u64 {
        let mut words = Vec::with_capacity(NUM_PARTICLES * 6);
        for p in &self.particles {
            words.push(p.pos.x.to_bits());
            words.push(p.pos.y.to_bits());
            words.push(p.pos.z.to_bits());
            words.push(p.vel.x.to_bits());
            words.push(p.vel.y.to_bits());
            words.push(p.vel.z.to_bits());
        }
        hash_u32s(&words)
    }
}

fn bounce(pos: &mut f32, vel: &mut f32) {
    if *pos > BOX_HALF {
        *pos = BOX_HALF;
        *vel = -*vel;
    }
    if *pos < -BOX_HALF {
        *pos = -BOX_HALF;
        *vel = -*vel;
    }
}

/// A fixed, non-random scripted input sequence (a replay would be exactly
/// this, decoded from `InputState::pack`/`unpack` — SPEC §6.4).
fn scripted_input(step: u32) -> InputState {
    let phase = step % 8;
    InputState {
        dir_x: match phase {
            0..=2 => 1,
            3..=5 => -1,
            _ => 0,
        },
        dir_y: if phase.is_multiple_of(2) { 1 } else { -1 },
        hop: phase == 4,
        ..Default::default()
    }
}

fn run(seed: u64) -> u64 {
    let mut sim = PlaceholderSim::new(seed);
    for s in 0..STEPS {
        sim.step(scripted_input(s));
    }
    sim.state_hash()
}

#[test]
fn same_seed_and_script_yields_identical_hash() {
    let a = run(12345);
    let b = run(12345);
    assert_eq!(a, b);
}

#[test]
fn different_seed_diverges() {
    let a = run(12345);
    let b = run(54321);
    assert_ne!(a, b);
}
