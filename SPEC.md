# FLOPPY SPIN — SPEC (authoritative)

A commercial-quality 3D spinning-top battle game that ships as a **single Windows
`floppy_spin.exe` ≤ 1,474,560 bytes**, with no external assets or dependencies and its
own CPU renderer. Size, determinism, and self-containment are the identity of the
project. Where this document and code comments disagree, this document wins; where
tuning numbers disagree, `TuneParams` in `floppy_core::physics` wins (this doc records
starting values).

Companion docs: `game_design.md` (what makes it fun — combat, roster, feel),
`ROADMAP.md` (milestones and exit gates), `docs/notes/` (recorded lessons).

---

## 1. Hard constraints (non-negotiable)

| # | Constraint |
|---|---|
| C1 | Single self-contained Windows x64 exe, **≤ 1,474,560 bytes**. Imports Windows system DLLs only. No packers. |
| C2 | Rust 2021. Crate `floppy_spin` → `floppy_spin.exe`. Target `x86_64-pc-windows-gnu`. |
| C3 | Zero bundled asset files. All meshes, textures-equivalents, audio generated procedurally at runtime. |
| C4 | Fully 3D: simulation AND rendering in 3D space. Tops have 3D position/velocity, roll around a bowl-shaped arena, climb walls, go airborne, land. Not a 2D sim with 3D visuals. |
| C5 | Deterministic: same seed + same inputs → identical frame hash. Fixed 120 Hz physics; no libm transcendentals, no fast-math. |
| C6 | Frames renderable headlessly (no display) to PNG for verification. |
| C7 | Single-threaded; 60 fps at internal 960×540 on a ~2015 dual-core x64. |
| C8 | `core`, `render`, `audio`, `io` crates are `#![forbid(unsafe_code)]`. All unsafe/FFI lives in `platform::win32` only. |
| C9 | Single-player vs AI only. Commercial quality bar: physics-feel, VFX, UI, AI weighted equally. |
| C10 | Release profile: `opt-level="z"`, `lto=true`, `codegen-units=1`, `panic="abort"`, `strip=true`. Zero runtime crate dependencies for the Windows binary. |

## 2. Toolchain & build (decision record)

**Decision:** build natively on Windows with the rustup **`stable-x86_64-pc-windows-gnu`
host toolchain**, pinned by `rust-toolchain.toml`. The original spec assumed
cross-compilation from Linux via MinGW-w64; this machine is Windows 11 and has no MinGW.
The gnu host toolchain is self-sufficient (bundles `rust-mingw`: binutils `ld`, CRT
startup objects, and system-DLL import libraries), so the same target triple (C2) is
produced with zero extra installs. Verified 2026-07-03: hello-world with the C10 release
profile builds, runs, and is 241,664 bytes.

- Build: `cargo build --release` (default target of the pinned toolchain is
  `x86_64-pc-windows-gnu`).
- The msvc toolchain must not be used for shipping builds (different CRT/ABI, drags in
  `vcruntime` unless static).
- The macOS dev backend (winit+softbuffer+cpal, safe Rust) is cfg-gated behind
  `target_os = "macos"` in `[target.'cfg(...)'.dependencies]`, so it never enters the
  Windows build graph or size budget. It is best-effort (cannot be compiled or tested on
  this machine) and is not a ship gate.

## 3. Rendering approach (decision record)

**Decision: CPU software rasterization** into a `Vec<u32>` 0x00RRGGBB framebuffer
(BGRA byte order in memory), internal resolution **960×540**, blitted with GDI
`StretchDIBits` and encoded directly to PNG for headless verification.

Rationale against the constraints: (1) zero dependencies beyond `user32/gdi32` — no
GPU driver, no D3D version risk; (2) code-only renderer costs tens of KB, far inside the
size budget; (3) headless rendering is the *same* code path minus the blit, making
golden-frame verification exact; (4) everything procedural by construction. A GPU path
would tie determinism to driver behavior and add API surface for zero size benefit.
The dominant performance cost is fill-rate (C7); the renderer budget is §10.

## 4. Architecture

Cargo workspace:

```
floppy_spin/
├── Cargo.toml            # workspace + root package `floppy_spin`
├── rust-toolchain.toml   # pins stable-x86_64-pc-windows-gnu
├── crates/
│   ├── floppy_core/      # deterministic 3D sim: math, physics, combat, AI, flow  [forbid(unsafe)]
│   ├── floppy_render/    # 3D software renderer + HUD/menus drawing               [forbid(unsafe)]
│   ├── floppy_audio/     # procedural synth: mixer, SFX, tracker                  [forbid(unsafe)]
│   └── floppy_io/        # PNG/WAV encoders, PE-import parsing (host verification) [forbid(unsafe)]
├── src/
│   ├── main.rs           # bin `floppy_spin`: game loop, wires core+render+audio to platform
│   ├── bin/headless.rs   # bin `headless`: sim → PNG/WAV, no display, CI verification
│   ├── bin/gate.rs       # bin `gate`: size gate + PE import allowlist check (host tool)
│   └── platform/
│       ├── win32.rs      # ALL unsafe/FFI: window, GDI blit, input, waveOut, timing, APPDATA
│       └── macos.rs      # cfg-gated safe backend (best-effort, not shipped)
└── tools/size_check.sh   # convenience wrapper around `gate`
```

Dependency direction: `core` depends on nothing. `render`/`audio` depend only on `core`.
`io` depends on nothing. Binaries wire them together. `core` knows nothing about
pixels or samples; it emits state + `BattleEvent`s.

### Data flow per frame
```
platform input → InputState ─┐
                             ├→ core::sim (0..n fixed steps @120Hz) → GameState + BattleEvents
seed/menu state ─────────────┘         │
                                       ├→ render::draw(state_prev, state_curr, alpha) → framebuffer → blit/PNG
                                       └→ audio::on_events + mixer → samples → waveOut/WAV
```

## 5. Determinism rules (C5)

- Physics at fixed **120 Hz**; rendering interpolates between the two most recent sim
  states at ~60 fps. Headless golden frames render at `alpha = 1.0`.
- `f32` in fixed evaluation order. Iteration over entities is by fixed index order.
- **No libm.** `sin/cos` via a shared 4096-entry quarter-symmetric LUT; `sqrt`/`rsqrt`
  via fixed-iteration Newton refinement of a bit-trick seed; `atan2`-style needs via
  LUT/polynomial with fixed iterations. All in `floppy_core::fixmath`; clippy lint or
  test greps guard against `f32::sin/cos/sqrt/powf/...` creeping in outside `fixmath`.
- RNG: integer **xorshift** (`u64` state); round seed = pure function
  `mix(match_seed, round_index)`. Gameplay consumes RNG only at deterministic points;
  no verb/action consumes RNG when unused.
- Deterministic clamps: velocity, tilt, position, and spin are clamped every step so
  the extra 3D degrees of freedom cannot diverge; denormals avoided by clamping small
  magnitudes to zero at fixed thresholds.
- Hit-stop = skip N whole sim steps (integer), identical for both tops.
- Frame hash: FNV-1a 64 over the framebuffer after each rendered frame (headless).
  Determinism test: two runs, same seed + scripted inputs → identical hash sequence.
- The sim never reads wall-clock time, thread IDs, pointers, or hash-map iteration
  order (no `HashMap` in `core` state; fixed arrays / `Vec` with fixed order only).

## 6. Simulation spec

### 6.1 Top state (core struct, plain data)
```rust
struct Top {
    pos: Vec3, vel: Vec3,          // meters, m/s
    spin: f32,                     // stamina == axial angular speed, 0..=SPIN_MAX (10_000)
    spin_dir: i8,                  // +1 CW / -1 CCW (top-down)
    tilt: Vec2,                    // spin-axis tilt vector (precession state, radians)
    tilt_phase: f32,               // precession angle
    mass: f32, radius: f32, height: f32,
    stats: Stats,                  // ATK DEF STA WGT SPD MTR, 0..=100 (see game_design.md)
    kind: TopKind,                 // preset id or Custom (garage)
    grounded: bool,
    dash_cd: u16, verb state timers (guard/hop/carve/anchor), meter: f32, // 0..=100
    special: SpecialState,         // Idle | Armed | Active{kind, steps_left} | CrashWindow{steps_left}
}
```

### 6.2 Arena
Analytic heightfield `h(x, z)` (no grid): parabolic basin + concentric ridge rings
(LUT-sin of radius) + gentle cross-hills (LUT-sin of x/z), with all features enveloped
to zero approaching the rim so the outer wall is clean. Physics samples `h` and its
analytic gradient for terrain normal; render tessellates the same function. Bowl radius
~9.5 m, rim height ~3.2 m, steep wall from r ≈ 7 m. **Ring-out:** clearing rim height
outside the bowl radius. **Stamina-out:** spin → 0, or topple (tilt magnitude past
threshold while slow).

### 6.3 Dynamics per step (fixed order)
1. Inputs → verb state machines (guard/hop/carve/anchor/dash/special; see game_design.md §Combat).
2. Control acceleration (camera-relative, digital dir × `a_max(SPD)`), slope gravity
   component, friction, verb modifiers.
3. Integrate velocity → position (semi-implicit Euler).
4. Terrain contact: if below `h`, project out along normal, apply normal force response,
   grounded=true; else airborne, full gravity.
5. Spin decay (passive + verb costs), precession update (tilt grows as spin drops,
   wobble frequency from spin), topple check.
6. Pairwise collision (one pair): sphere-sphere de-penetration by inverse mass,
   restitution, knockback = f(attacker spin, ATK, approach), drain = f(impulse, ATK,
   DEF), spin transfer by relative spin-dir, airborne launch on hard vertical impulse.
   Emit `BattleEvent`s.
7. Meter gains, special/Crash-Out timers, out-condition checks.

All tuning constants in one `TuneParams` const block in `floppy_core::physics`.

### 6.4 InputState (the only way anything drives the sim)
```rust
struct InputState {
    dir_x: i8, dir_y: i8,        // -1/0/+1, camera-relative
    dash: bool, special: bool,    // Space, Shift
    guard: bool, hop: bool, carve: bool, anchor: bool, // Z, X, C, Ctrl
}
```
Human and AI both produce `InputState`; the sim never branches on who produced it.
Every verb is fully inert (zero state change, zero RNG) unless pressed. During the
Launch phase the same fields are reinterpreted (dir = aim, `special` toggles spin
direction, `dash` locks power) — documented in game_design.md §Launch.

### 6.5 Match structure (fixed)
Round points: **Crash-Out 3** (kill within the 144-step window after firing a special) /
**Over 2** (ring-out or KO) / **Survivor 1** (opponent stamina-out) / simultaneous-out
**0** and replay. **First to 4 points** wins the match. Round seed =
`mix(match_seed, round)`. Score pips shown live (HUD) and on the inter-round screen.

## 7. Screens & flow

```
Boot ▶ Title ▶ MainMenu ┬ QuickBattle ▶ TopSelect ▶ Match ▶ MatchOver ▶ MainMenu
                        ├ Garage      ├ Settings   └ Quit
Match: Intro(countdown) ▶ Launch ▶ Fight ▶ Decided ▶ RoundResult ─(loop)─ ▶ MatchOver
```
Transitions are explicit enum edges; a flow test asserts every screen is reachable and
none is a dead end. UI logic (cursors, selection, transitions) lives in `core` and is
headlessly testable; drawing lives in `render`. Settings: music/SFX volume, screen-shake
(Off/Low/Normal/High), difficulty, window scale (1×/1.5×/2×/borderless-fullscreen;
internal resolution is always 960×540), colorblind mode.

## 8. Audio

44.1 kHz mono→stereo-duplicated i16; **16-voice mixer**; oscillators square (variable
duty) / saw / triangle / sine-LUT / noise-LFSR; per-voice ADSR; global soft-clip. SFX
are pure functions of `BattleEvent`s (headlessly reproducible). Music: lightweight
tracker, 4 channels, `i8` patterns embedded in code, menu + battle themes; battle theme
adds an intensity layer at bar boundaries while a special is armed (keyed off tracker
row index, never off audio output). Windows output: `waveOut` multi-buffer ring in
`platform::win32`; buffers as small as sustain glitch-free. Headless renders the same
mixer output to WAV.

## 9. Persistence

Save file `%APPDATA%\floppy_spin\save.bin` (path via `GetEnvironmentVariableW`):
magic `FSPN`, version byte, garage part indices (5×u8), settings block, XOR checksum.
Any missing/truncated/corrupt/wrong-version file → silently fall back to defaults;
never crash, never partially apply.

## 10. Performance budget (C7, per 16.6 ms frame @ 960×540)

| Item | Budget |
|---|---|
| 2 sim steps @120 Hz | ≤ 1 ms |
| Clear + z-clear | ≤ 1.5 ms |
| Arena + tops raster (~hundreds of tris, ~1.5× overdraw) | ≤ 7 ms |
| Particles/trails/bloom (half-res bright pass) | ≤ 4 ms |
| HUD/text + blit | ≤ 1.5 ms |
| Headroom | ~1.5 ms |

Single-threaded. If over budget: shrink bloom radius, cap particles, simplify arena
tessellation — in that order.

## 11. AI

Utility controller in `floppy_core::ai`, producing `InputState` only. Difficulty-scaled
parameters (separate `AiParams` const table, NOT in `TuneParams`): reaction delay
(steps), 3D aim error, aggression, verb/dash/special skill gates, predictive lead,
panic threshold. Tiers Easy/Normal/Hard/Ace. Higher tiers use combat verbs as
survival/offense tools (e.g. the correct escape vs an armed special per the counterplay
table); lower tiers don't.

## 12. Verification gates (every milestone must keep these green)

1. **`cargo test`**: math properties, physics invariants (momentum direction in
   collisions, grounded stability, symmetry), ring-out, topple, flow reachability,
   combat unit tests, frame-hash determinism (two identical runs), AI balance
   (Hard > Easy in >70% over a seed spread; Ace never self-rings-out in an empty arena),
   save corruption fallback, no-libm grep test.
2. **`cargo run --bin headless`**: renders golden frames (title / launch / mid-fight /
   airborne-clash / ring-out / result) to PNG + SFX/music WAV; tolerance-based compare
   (per-channel mean abs diff ≤ 2, ≤1% pixels above diff 24) against checked-in goldens.
3. **`cargo run --bin gate`** (also via `tools/size_check.sh`): fails if
   `floppy_spin.exe` > 1,474,560 bytes; parses the PE import table and fails on any
   import outside the allowlist {kernel32.dll, user32.dll, gdi32.dll, winmm.dll,
   msvcrt.dll, ntdll.dll} plus the `api-ms-win-*` API-set prefix (case-insensitive).
   msvcrt/ntdll/api-sets are what the windows-gnu ABI links by construction (measured
   2026-07-04 on the skeleton exe); all ship with Windows. This is the
   objdump-equivalent, implemented dependency-free.
4. **`cargo clippy` / `cargo fmt --check`** clean.
5. Real-hardware Windows checks at ship: key/XInput feel, glitch-free waveOut, GDI
   blit pacing. (XInput, if added, is loaded via `LoadLibraryW` at runtime — it must not
   appear in the static import table.)
