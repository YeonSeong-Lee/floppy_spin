//! Fixed-capacity particle system (game_design.md §5 juice table). Every
//! spawn/motion input is either a `BattleEvent`'s own fields, `World`/flow
//! sim state, or the dedicated render-side `Rng` the caller threads through
//! (SPEC §5 "HARD RULES": never `World`'s own `rng`, never wall-clock) — see
//! `spawn_*` below, one per juice-table row.
//!
//! Motion tuning (initial speed ranges, gravity/drag multipliers) is a
//! documented creative approximation: game_design.md §5 specifies spawn
//! COUNT, COLOR, LIFETIME, and a qualitative motion description ("sparks",
//! "cone-up", "tangential", ...) per row, but not exact velocities — those
//! are chosen here to read as the described shape and are NOT pinned by the
//! design doc.
//!
//! Rendering: particles are small constant-screen-size additive quads,
//! written directly into `Frame::px`/`BrightBuffer` with a depth TEST
//! (never a depth WRITE) against the already-drawn scene — the same
//! cheaper-than-a-full-triangle-raster route the milestone brief calls out
//! explicitly, since a particle covers only a handful of pixels. Every
//! particle is emissive-tagged (game_design.md §5: "all flashes/neon feed
//! bloom") so it always stamps into the half-res `BrightBuffer` too.

use crate::camera::Camera;
use crate::frame::Frame;
use crate::post::BrightBuffer;
use crate::raster::add_saturating;
use floppy_core::fixmath;
use floppy_core::rng::Rng;
use floppy_core::vec::{Vec2, Vec3};
use std::f32::consts::TAU;

/// Fixed pool capacity (milestone brief: "e.g. `[Particle; 256]`").
pub const CAPACITY: usize = 256;

/// One live (or expired-but-not-yet-overwritten) particle. `age >= life`
/// means dead/free — checked by both `update`/`draw`, never by a separate
/// `alive: bool` (one less thing that could desync from `age`/`life`).
#[derive(Clone, Copy, Debug)]
pub struct Particle {
    pub pos: Vec3,
    pub vel: Vec3,
    pub color_start: Vec3,
    pub color_end: Vec3,
    /// Elapsed frames (60 Hz render-frame ticks, NOT sim steps — see
    /// `ParticlePool::update` docs).
    pub age: f32,
    pub life: f32,
    /// Constant screen-space size in pixels (module docs: no perspective
    /// depth-scaling — the cheap route).
    pub size_px: f32,
    /// Fraction of full gravity (`GRAVITY` below) applied to `vel.y` each
    /// update.
    pub gravity_mult: f32,
    /// Per-frame multiplicative velocity damping.
    pub drag: f32,
    /// Angular velocity (rad/s) rotating `vel`'s XZ component each update —
    /// nonzero only for Special Fire's spiral (game_design.md §5).
    pub ang_vel: f32,
}

const EMPTY: Particle = Particle {
    pos: Vec3::new(0.0, 0.0, 0.0),
    vel: Vec3::new(0.0, 0.0, 0.0),
    color_start: Vec3::new(0.0, 0.0, 0.0),
    color_end: Vec3::new(0.0, 0.0, 0.0),
    age: 0.0,
    life: 0.0,
    size_px: 0.0,
    gravity_mult: 0.0,
    drag: 1.0,
    ang_vel: 0.0,
};

/// World-space downward acceleration applied to particles (m/s^2, scaled per
/// particle by `gravity_mult`); a loose approximation of the arena's own
/// gravity, not read from `floppy_core::physics::TUNE` (VFX-only, no sim
/// coupling per the milestone's "hit-stop is the one sim-visible knob" rule).
const GRAVITY: f32 = 9.0;
/// Fixed per-update timestep: particles advance once per rendered (60 Hz)
/// flow frame, never per wall-clock delta (SPEC §5 determinism).
const DT: f32 = 1.0 / 60.0;

/// Fixed-capacity, integer-indexed round-robin particle pool (milestone
/// brief: "no Vec growth ... deterministic"). `spawn_raw` always overwrites
/// the next slot regardless of whether it's currently alive — a fixed,
/// input-order-only eviction rule, never a "find a free slot" search.
pub struct ParticlePool {
    particles: [Particle; CAPACITY],
    next: usize,
}

impl Default for ParticlePool {
    fn default() -> Self {
        Self::new()
    }
}

impl ParticlePool {
    pub fn new() -> Self {
        Self {
            particles: [EMPTY; CAPACITY],
            next: 0,
        }
    }

    fn spawn_raw(&mut self, p: Particle) {
        self.particles[self.next] = p;
        self.next = (self.next + 1) % CAPACITY;
    }

    /// Advance every live particle by one 60 Hz frame: spiral rotation (if
    /// `ang_vel != 0`), gravity, drag, integrate position. Dead particles
    /// (`age >= life`) are skipped entirely (cheap: no work, no removal).
    pub fn update(&mut self) {
        for p in self.particles.iter_mut() {
            if p.age >= p.life {
                continue;
            }
            p.age += 1.0;
            if p.ang_vel != 0.0 {
                let c = fixmath::cos(p.ang_vel * DT);
                let s = fixmath::sin(p.ang_vel * DT);
                let vx = p.vel.x * c - p.vel.z * s;
                let vz = p.vel.x * s + p.vel.z * c;
                p.vel.x = vx;
                p.vel.z = vz;
            }
            p.vel.y -= GRAVITY * p.gravity_mult * DT;
            p.vel = p.vel * p.drag;
            p.pos = p.pos + p.vel * DT;
        }
    }

    /// Draw every live particle as a small constant-screen-size additive
    /// quad (module docs): camera-project, depth-TEST (never write) against
    /// `frame.depth`, additive-saturate into `frame.px`, and stamp the same
    /// color into the half-res `bright` buffer (every particle is
    /// emissive/bloom-tagged). `shake` is a whole-pixel screen-space offset
    /// (same one applied to the 3D scene — see `battle.rs`), so particles
    /// shake together with the geometry they're attached to.
    pub fn draw(
        &self,
        frame: &mut Frame,
        bright: &mut BrightBuffer,
        cam: &Camera,
        shake: (f32, f32),
    ) {
        for p in self.particles.iter() {
            if p.age >= p.life || p.life <= 0.0 {
                continue;
            }
            let v = cam.to_view(p.pos);
            if v.z <= Camera::NEAR {
                continue;
            }
            let (sx, sy, inv_z) = cam.project(v);
            let cx = (sx + shake.0) as i32;
            let cy = (sy + shake.1) as i32;

            let t = (p.age / p.life).clamp(0.0, 1.0);
            let color = p.color_start.lerp(p.color_end, t);
            let packed = crate::shade::color_to_px(color);

            let half = ((p.size_px * 0.5) as i32).max(1);
            for dy in -half..=half {
                let y = cy + dy;
                if y < 0 || y as usize >= frame.h {
                    continue;
                }
                for dx in -half..=half {
                    let x = cx + dx;
                    if x < 0 || x as usize >= frame.w {
                        continue;
                    }
                    let idx = y as usize * frame.w + x as usize;
                    if inv_z > frame.depth[idx] {
                        frame.px[idx] = add_saturating(frame.px[idx], packed);
                        bright.add(x / 2, y / 2, packed);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Direction generators (module docs: motion shape is the design-doc-given
// part, exact numbers are this file's own tuning).
// ---------------------------------------------------------------------------

fn dir_sphere(rng: &mut Rng) -> Vec3 {
    let theta = rng.next_f32() * TAU;
    let y = rng.next_f32() * 2.0 - 1.0;
    let horiz = (1.0 - y * y).max(0.0);
    // horiz is sin^2-like magnitude; a cheap sqrt-free "spread" scale (not a
    // true uniform sphere sample, but isotropic-looking and deterministic).
    Vec3::new(fixmath::cos(theta) * horiz, y, fixmath::sin(theta) * horiz).normalize_or_zero()
}

fn dir_upward_cone(rng: &mut Rng, spread: f32) -> Vec3 {
    let theta = rng.next_f32() * TAU;
    let r = rng.next_f32() * spread;
    Vec3::new(fixmath::cos(theta) * r, 1.0, fixmath::sin(theta) * r).normalize_or_zero()
}

fn dir_planar(rng: &mut Rng, base: Vec2, spread: f32, up: f32) -> Vec3 {
    let jitter = dir_sphere(rng);
    let bx = base.x + jitter.x * spread;
    let bz = base.y + jitter.z * spread;
    Vec3::new(bx, up + jitter.y * spread * 0.3, bz).normalize_or_zero()
}

fn dir_down_scatter(rng: &mut Rng, spread: f32) -> Vec3 {
    let theta = rng.next_f32() * TAU;
    let r = rng.next_f32() * spread;
    Vec3::new(fixmath::cos(theta) * r, -0.3, fixmath::sin(theta) * r).normalize_or_zero()
}

// ---------------------------------------------------------------------------
// Colors (game_design.md §5 juice-table color column, where given verbatim;
// otherwise a documented reading of the row's name).
// ---------------------------------------------------------------------------

// `pub` (not just module-private): `main.rs` reuses this exact palette for
// the matching juice-table FLASH colors, so a hit's spark color and its
// screen flash are always visually consistent rather than two independently
// hand-picked constants drifting apart.
pub const WHITE: Vec3 = Vec3::new(1.0, 1.0, 1.0);
const WHITE_CYAN: Vec3 = Vec3::new(0.55, 0.95, 1.0);
const HOT_WHITE: Vec3 = Vec3::new(1.0, 0.98, 0.92);
pub const AMBER: Vec3 = Vec3::new(1.0, 0.62, 0.12);
pub const CYAN: Vec3 = Vec3::new(0.15, 0.85, 1.0);
const DUST: Vec3 = Vec3::new(0.55, 0.5, 0.4);
const SILVER: Vec3 = Vec3::new(0.82, 0.88, 0.95);
pub const RED_ORANGE: Vec3 = Vec3::new(1.0, 0.3, 0.12);
pub const GOLD: Vec3 = Vec3::new(1.0, 0.83, 0.22);

fn dim(c: Vec3) -> Vec3 {
    c * 0.22
}

/// Internal per-call spawn parameters, shared by every `spawn_*` row
/// function below (module docs: one shared core, one function per juice-
/// table row for the count/color/lifetime/shape it actually needs).
#[allow(clippy::too_many_arguments)]
fn spawn_n(
    pool: &mut ParticlePool,
    rng: &mut Rng,
    pos: Vec3,
    count: u32,
    life_frames: f32,
    color_start: Vec3,
    color_end: Vec3,
    speed_lo: f32,
    speed_hi: f32,
    size_px: f32,
    gravity_mult: f32,
    drag: f32,
    ang_vel: f32,
    mut dir_fn: impl FnMut(&mut Rng) -> Vec3,
) {
    for _ in 0..count {
        let dir = dir_fn(rng);
        let speed = speed_lo + rng.next_f32() * (speed_hi - speed_lo);
        let life_jitter = 0.85 + rng.next_f32() * 0.3;
        pool.spawn_raw(Particle {
            pos,
            vel: dir * speed,
            color_start,
            color_end,
            age: 0.0,
            life: (life_frames * life_jitter).max(1.0),
            size_px,
            gravity_mult,
            drag,
            ang_vel,
        });
    }
}

/// ms -> 60Hz render frames (juice-table durations are given in ms).
const fn frames(ms: f32) -> f32 {
    ms * 0.06
}

/// Light hit: 6 white-cyan sparks, 180 ms.
pub fn spawn_light_hit(pool: &mut ParticlePool, rng: &mut Rng, pos: Vec3) {
    spawn_n(
        pool,
        rng,
        pos,
        6,
        frames(180.0),
        WHITE_CYAN,
        dim(WHITE_CYAN),
        1.0,
        2.5,
        2.0,
        0.3,
        0.90,
        0.0,
        dir_sphere,
    );
}

/// Heavy hit: 20 hot-white -> amber, 350 ms.
pub fn spawn_heavy_hit(pool: &mut ParticlePool, rng: &mut Rng, pos: Vec3) {
    spawn_n(
        pool,
        rng,
        pos,
        20,
        frames(350.0),
        HOT_WHITE,
        AMBER,
        1.5,
        4.0,
        3.0,
        0.5,
        0.92,
        0.0,
        dir_sphere,
    );
}

/// Airborne clash: 28 cyan cone-up, 500 ms.
pub fn spawn_airborne_clash(pool: &mut ParticlePool, rng: &mut Rng, pos: Vec3) {
    spawn_n(
        pool,
        rng,
        pos,
        28,
        frames(500.0),
        CYAN,
        dim(CYAN),
        1.5,
        3.5,
        3.0,
        0.2,
        0.94,
        0.0,
        |r| dir_upward_cone(r, 0.6),
    );
}

/// Wall bounce: 10 dust tangential (no juice-table lifetime given; 300 ms
/// chosen — dust settling quickly).
pub fn spawn_wall_bounce(pool: &mut ParticlePool, rng: &mut Rng, pos: Vec3, tangent: Vec2) {
    spawn_n(
        pool,
        rng,
        pos,
        10,
        frames(300.0),
        DUST,
        dim(DUST),
        0.5,
        1.5,
        2.0,
        0.6,
        0.88,
        0.0,
        move |r| dir_planar(r, tangent, 0.35, 0.1),
    );
}

/// Dash: 8 accent streaks behind (no lifetime given; 150 ms — a quick
/// afterimage). `accent` is the dashing top's roster color, `back` points
/// from the top toward where it came from (opposite its dash direction).
pub fn spawn_dash(pool: &mut ParticlePool, rng: &mut Rng, pos: Vec3, back: Vec2, accent: Vec3) {
    spawn_n(
        pool,
        rng,
        pos,
        8,
        frames(150.0),
        accent,
        dim(accent),
        2.0,
        4.0,
        2.0,
        0.0,
        0.85,
        0.0,
        move |r| dir_planar(r, back, 0.25, 0.0),
    );
}

/// Guard block / Parry: 12 silver arc (no lifetime given; 200 ms).
/// `facing` is the defender's facing direction (the arc reads across their
/// front hemisphere).
pub fn spawn_guard_parry(pool: &mut ParticlePool, rng: &mut Rng, pos: Vec3, facing: Vec2) {
    spawn_n(
        pool,
        rng,
        pos,
        12,
        frames(200.0),
        SILVER,
        dim(SILVER),
        1.0,
        2.5,
        2.0,
        0.1,
        0.90,
        0.0,
        move |r| dir_planar(r, facing, 0.7, 0.2),
    );
}

/// Special fire: 40 accent spiral, 600 ms. `accent` is the firing top's
/// roster color; the spiral itself comes from a nonzero `ang_vel`.
pub fn spawn_special_fire(pool: &mut ParticlePool, rng: &mut Rng, pos: Vec3, accent: Vec3) {
    spawn_n(
        pool,
        rng,
        pos,
        40,
        frames(600.0),
        accent,
        dim(accent),
        1.0,
        2.5,
        3.0,
        0.1,
        0.95,
        8.0,
        |r| dir_upward_cone(r, 0.8),
    );
}

/// Crash-Out: 60 white -> accent shards, 800 ms.
pub fn spawn_crash_out(pool: &mut ParticlePool, rng: &mut Rng, pos: Vec3, accent: Vec3) {
    spawn_n(
        pool,
        rng,
        pos,
        60,
        frames(800.0),
        WHITE,
        accent,
        2.0,
        6.0,
        4.0,
        0.7,
        0.90,
        0.0,
        dir_sphere,
    );
}

/// Ring-out: 24 red-orange falling (no lifetime given; 500 ms).
pub fn spawn_ring_out(pool: &mut ParticlePool, rng: &mut Rng, pos: Vec3) {
    spawn_n(
        pool,
        rng,
        pos,
        24,
        frames(500.0),
        RED_ORANGE,
        dim(RED_ORANGE),
        0.5,
        2.0,
        3.0,
        1.2,
        0.97,
        0.0,
        |r| dir_down_scatter(r, 0.6),
    );
}

/// Topple: 16 amber collapse (no lifetime given; 400 ms).
pub fn spawn_topple(pool: &mut ParticlePool, rng: &mut Rng, pos: Vec3) {
    spawn_n(
        pool,
        rng,
        pos,
        16,
        frames(400.0),
        AMBER,
        dim(AMBER),
        0.5,
        1.5,
        3.0,
        0.8,
        0.9,
        0.0,
        |r| dir_down_scatter(r, 0.4),
    );
}

/// Round win: 30 gold sparkle (no lifetime given; 500 ms).
pub fn spawn_round_win(pool: &mut ParticlePool, rng: &mut Rng, pos: Vec3) {
    spawn_n(
        pool,
        rng,
        pos,
        30,
        frames(500.0),
        GOLD,
        dim(GOLD),
        0.5,
        1.5,
        2.5,
        0.15,
        0.95,
        0.0,
        |r| dir_upward_cone(r, 0.9),
    );
}

/// Match win: 80 gold fountain, 1 s.
pub fn spawn_match_win(pool: &mut ParticlePool, rng: &mut Rng, pos: Vec3) {
    spawn_n(
        pool,
        rng,
        pos,
        80,
        frames(1000.0),
        GOLD,
        WHITE,
        2.0,
        5.0,
        3.0,
        0.9,
        0.97,
        0.0,
        |r| dir_upward_cone(r, 0.9),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_writes_the_requested_count_of_live_particles() {
        let mut pool = ParticlePool::new();
        let mut rng = Rng::new(7);
        spawn_light_hit(&mut pool, &mut rng, Vec3::new(0.0, 1.0, 0.0));
        let alive = pool.particles.iter().filter(|p| p.age < p.life).count();
        assert_eq!(alive, 6);
    }

    #[test]
    fn round_robin_allocation_overwrites_the_oldest_slot_first() {
        let mut pool = ParticlePool::new();
        let mut rng = Rng::new(1);
        // Fill the entire pool exactly once.
        for _ in 0..CAPACITY {
            spawn_n(
                &mut pool,
                &mut rng,
                Vec3::default(),
                1,
                100.0,
                WHITE,
                WHITE,
                1.0,
                1.0,
                1.0,
                0.0,
                1.0,
                0.0,
                dir_sphere,
            );
        }
        assert_eq!(pool.next, 0);
        // One more spawn must overwrite slot 0 (round-robin wraps).
        let before = pool.particles[0];
        spawn_n(
            &mut pool,
            &mut rng,
            Vec3::new(9.0, 9.0, 9.0),
            1,
            50.0,
            WHITE,
            WHITE,
            1.0,
            1.0,
            1.0,
            0.0,
            1.0,
            0.0,
            dir_sphere,
        );
        assert_ne!(pool.particles[0].pos, before.pos);
        assert_eq!(pool.next, 1);
    }

    #[test]
    fn update_ages_particles_and_they_die_on_schedule() {
        // Constructed directly (not via `spawn_n`, whose lifetime jitter
        // would make the exact death frame non-obvious) so `life == 2.0`
        // exactly.
        let mut pool = ParticlePool::new();
        pool.spawn_raw(Particle {
            pos: Vec3::default(),
            vel: Vec3::default(),
            color_start: WHITE,
            color_end: WHITE,
            age: 0.0,
            life: 2.0,
            size_px: 1.0,
            gravity_mult: 0.0,
            drag: 1.0,
            ang_vel: 0.0,
        });
        let alive_now =
            |pool: &ParticlePool| pool.particles.iter().filter(|p| p.age < p.life).count();
        assert_eq!(alive_now(&pool), 1);
        pool.update();
        assert_eq!(alive_now(&pool), 1);
        pool.update();
        assert_eq!(
            alive_now(&pool),
            0,
            "particle should be dead after 2 updates with life=2"
        );
    }

    #[test]
    fn gravity_pulls_velocity_downward_over_time() {
        let mut pool = ParticlePool::new();
        let mut rng = Rng::new(5);
        spawn_n(
            &mut pool,
            &mut rng,
            Vec3::default(),
            1,
            1000.0,
            WHITE,
            WHITE,
            0.0,
            0.0,
            1.0,
            1.0,
            1.0,
            0.0,
            |_| Vec3::new(0.0, 1.0, 0.0),
        );
        let vy0 = pool.particles[0].vel.y;
        pool.update();
        assert!(pool.particles[0].vel.y < vy0, "gravity should reduce vel.y");
    }

    #[test]
    fn determinism_same_seed_same_spawn_positions() {
        let mut pool_a = ParticlePool::new();
        let mut rng_a = Rng::new(99);
        spawn_crash_out(&mut pool_a, &mut rng_a, Vec3::new(1.0, 2.0, 3.0), AMBER);

        let mut pool_b = ParticlePool::new();
        let mut rng_b = Rng::new(99);
        spawn_crash_out(&mut pool_b, &mut rng_b, Vec3::new(1.0, 2.0, 3.0), AMBER);

        for i in 0..CAPACITY {
            assert_eq!(pool_a.particles[i].vel, pool_b.particles[i].vel);
            assert_eq!(pool_a.particles[i].life, pool_b.particles[i].life);
        }
    }
}
