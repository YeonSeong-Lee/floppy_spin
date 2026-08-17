# FLOPPY SPIN

FLOPPY SPIN is a commercial-quality 3D spinning-top battle game that ships as a
single self-contained Windows executable, `floppy_spin.exe`, **≤ 1,474,560
bytes (1.44 MB)**. Two tops roll, climb, and collide in a bowl-shaped arena in
full 3D — not a 2D sim with 3D dressing — with real physics-driven knockback,
spin drain, airborne launches, and ring-outs. Every pixel is drawn by the
game's own CPU software rasterizer, every sound is synthesized at runtime by
its own procedural audio engine, and every mesh, texture, and note is
generated in code — there are no bundled asset files at all. The simulation
is fully deterministic: the same seed and the same input script reproduce an
identical sequence of frames, byte for byte, which is what makes the
headless golden-frame and golden-WAV verification in this repo possible.

## Hard constraints

These are non-negotiable and enforced by the `gate` tool and the test suite
(see SPEC.md §1 for the authoritative list):

- **Size.** `floppy_spin.exe` must stay at or under 1,474,560 bytes. It
  imports Windows system DLLs only (`kernel32`, `user32`, `gdi32`, `winmm`,
  plus what the toolchain's C runtime/ABI links by construction) — no
  packers, no extra runtime DLLs.
- **Zero bundled assets.** No image, audio, or mesh files ship alongside the
  exe. Meshes are lathed procedurally from silhouette profiles, audio is
  synthesized by a 16-voice mixer + tracker, and text is a hand-authored 5x7
  bitmap font baked into code.
- **Own rendering.** No GPU API, no external rendering library — a
  single-threaded CPU rasterizer draws into a `Vec<u32>` framebuffer at a
  fixed internal 960x540, blitted to the window with GDI `StretchDIBits`.
- **Deterministic simulation.** Physics runs at a fixed 120 Hz, uses `f32`
  math in a fixed evaluation order, and never calls a libm transcendental
  outside `floppy_core::fixmath` (sin/cos come from a shared LUT, sqrt from
  fixed-iteration Newton refinement). The sim never reads the wall clock,
  thread IDs, pointers, or hash-map iteration order.
- **Safety boundary.** The four library crates (`floppy_core`,
  `floppy_render`, `floppy_audio`, `floppy_io`) are all
  `#![forbid(unsafe_code)]`. Every `unsafe` block and every FFI call in the
  entire project lives in one place: `src/platform/win32.rs`.

## Building

The toolchain is pinned in `rust-toolchain.toml` to the rustup **host**
toolchain `stable-x86_64-pc-windows-gnu`. On a Windows machine with rustup,
no extra installs are required — the gnu host toolchain bundles the MinGW
linker, CRT startup objects, and system-DLL import libraries needed to
produce the `x86_64-pc-windows-gnu` target.

```
cargo build --release
```

The shipping binary lands at `target/release/floppy_spin.exe` (also
buildable explicitly at `target/x86_64-pc-windows-gnu/release/floppy_spin.exe`
depending on how the default target resolves). The release profile
(`opt-level="z"`, `lto=true`, `codegen-units=1`, `panic="abort"`,
`strip=true`) is what keeps it small. Last measured by CI on a clean
checkout: **391,680 bytes**, leaving 1,082,880 bytes of margin — about 73%
under the 1,474,560-byte budget. (The exact figure moves a little with the
toolchain version; the `gate` bin prints it on every run.)

Do not build this target with the `msvc` toolchain — it uses a different
CRT/ABI and will drag in `vcruntime` unless statically linked, which this
project does not do.

### Developing on macOS

The game also builds and runs on macOS through a cfg-gated dev backend
(`src/platform/macos.rs`, safe Rust over `winit`/`softbuffer`/`cpal`). This
is a development convenience, **not** a shipping target and **not** a ship
gate — `floppy_spin.exe` remains the only artifact any §12 gate judges.

Because `rust-toolchain.toml` pins a Windows *host* toolchain, bare `cargo`
fails here with `target tuple in channel name`. Select the host toolchain
explicitly instead:

```
RUSTUP_TOOLCHAIN=stable cargo run --release
RUSTUP_TOOLCHAIN=stable cargo test --workspace --release
```

The macOS-only dependencies live under
`[target.'cfg(target_os = "macos")'.dependencies]`, so they never enter the
Windows build graph, the import allowlist, or the size budget — a Windows
build still resolves to the four path crates and needs no extra installs.
The two backends expose an identical API and `main.rs` is not cfg-split, so
compiling here exercises the same `main.rs` lines Windows compiles.

## Running and controls

Launch `floppy_spin.exe` directly (double-click, or run it from a shell).
It opens a window at the game's internal 960x540 resolution (scaled per the
Settings screen's window-scale option, up to borderless fullscreen).

Menu flow: `Boot -> Title -> Main Menu -> {Quick Battle -> Top Select ->
Match -> Match Over -> Main Menu} | Garage | Settings | Quit`. A match runs
`Intro (countdown) -> Launch (minigame) -> Fight -> Decided -> Round Result`,
looping rounds until one side reaches the winning score, then `Match Over`.

In-match controls (all camera-relative — "left" always means toward
screen-left regardless of which way your top is facing):

| Key | Action |
|---|---|
| Arrow keys | Move / aim (camera-relative direction) |
| Space | Dash |
| Shift | Special |
| Z | Guard |
| X | Hop |
| C | Carve |
| Ctrl | Anchor |
| Esc | Back / quit |

During the Launch minigame the same keys are reinterpreted: arrows aim
heading/depth, Shift flips spin direction, Space locks in the current stage
(aim, then spin direction, then power).

## Verification

Three binaries plus the test suite gate every change:

1. **`cargo run --release --bin gate`** — measures `floppy_spin.exe` against
   the 1,474,560-byte budget and parses its PE import table against the
   allowlist (`kernel32.dll`, `user32.dll`, `gdi32.dll`, `winmm.dll`,
   `msvcrt.dll`, `ntdll.dll`, and any `api-ms-win-*` API-set). Fails loudly
   on either violation; this is the dependency-free equivalent of running
   `objdump` on the shipped binary.
2. **`cargo run --release --bin headless -- --golden check`** — renders the
   full SPEC §12-named golden-frame set (title / launch / mid-fight /
   airborne-clash / ring-out / result) headlessly and compares each against
   the checked-in PNGs in `goldens/` with a tolerance rule (mean abs
   per-channel diff <= 2, <= 1% of pixels with any channel diff > 24). Run
   `-- --golden write` to regenerate the checked-in PNGs after an
   intentional visual change. The same binary also writes golden WAV audio
   (`-- --wav out.wav --frames N`) for headless audio verification.
3. **`cargo test --workspace --release`** — 312 tests on Windows, 319 on
   macOS (each platform compiles only its own backend, and the macOS one
   carries 7 unit tests of its own), covering math properties, physics
   invariants (collision symmetry,
   grounded stability, ring-out, topple), flow reachability, combat verb
   unit tests, frame-hash determinism, AI balance (Hard beats Easy in >70%
   of a seed spread; Ace never self-rings-out), save-corruption fallback,
   and a no-libm grep guard, plus the golden-frame and golden-WAV
   integration tests above.

`cargo clippy --workspace --release --all-targets` and `cargo fmt --check`
must both be clean; both are part of the ship gate.

### What runs where

Gates 2, 3, clippy, and fmt are platform-independent and run on macOS too
(prefix them with `RUSTUP_TOOLCHAIN=stable`). Gate 1 compiles anywhere but
only means anything against a Windows PE, and SPEC §12.5's real-hardware
checks are Windows-only by definition. Measured on `aarch64-apple-darwin`
2026-08-17: 319 tests pass and all six goldens compare at mean-abs-diff
**0.000** — not merely inside the tolerance rule but byte-identical to PNGs
generated on Windows/x86_64, which is the determinism claim above holding
across two architectures rather than just across two runs.

Never run `--golden write` anywhere but Windows: the checked-in PNGs are the
ship reference, and regenerating them elsewhere would silently redefine what
"correct" means.

Because gates 1 and 5 are Windows-only, **any change touching `main.rs`,
`Cargo.toml`, or `src/platform/` re-arms the clean-checkout ship gate**
(`docs/notes/ship-gate-clean-checkout.md`) — a green macOS run does not
retire it.

### CI

`.github/workflows/ci.yml` runs gates 1–4 on a `windows-latest` runner and,
in a second job, the platform-independent ones on `macos-latest`. Because
Actions always starts from a fresh clone, the clean-checkout ship gate is
satisfied by construction on every push rather than by remembering to run
it. The Windows job is deliberately uncached for the same reason, and it
uploads the built `floppy_spin.exe` as a run artifact, so a playable binary
is available without a Windows machine being used to produce it.

The workflow does **not** cover SPEC §12.5 — input feel, waveOut glitching,
and blit pacing still need a human at real Windows hardware.

Do not add a Linux job: neither `cfg(windows)` nor `cfg(target_os =
"macos")` matches there, so `platform::backend` does not exist and the
build fails by design.

## Architecture

```
floppy_spin/
├── crates/
│   ├── floppy_core/    deterministic 3D sim: fixmath, physics, combat, AI, screen flow  [forbid(unsafe)]
│   ├── floppy_render/  CPU software rasterizer + HUD/menu drawing                       [forbid(unsafe)]
│   ├── floppy_audio/   procedural synth: 16-voice mixer, SFX, tracker                    [forbid(unsafe)]
│   └── floppy_io/      PNG/WAV encoders + PE-import parser (host verification tools)     [forbid(unsafe)]
└── src/
    ├── main.rs         bin `floppy_spin`: the shipped game, wires core+render+audio to platform
    ├── bin/headless.rs bin `headless`: sim -> PNG/WAV with no display, for CI/golden verification
    ├── bin/gate.rs     bin `gate`: size budget + PE import allowlist check (host tool, not shipped)
    └── platform/
        ├── mod.rs      picks one backend per target and re-exports it as `platform::backend`
        ├── win32.rs    the ONLY unsafe/FFI in the project: window, GDI blit, input, waveOut, timing
        └── macos.rs    cfg-gated best-effort dev backend (winit/softbuffer/cpal, safe Rust),
                        not shipped, not a ship gate
```

The two backends expose an identical API, so `main.rs` names neither and is
not cfg-split — it imports `platform::backend`. The macOS backend translates
winit key codes into the same Windows virtual-key codes `main.rs` polls, and
carries `#![forbid(unsafe_code)]` of its own: `win32.rs` remains the only
file in the project containing `unsafe`.

`floppy_core` depends on nothing; `floppy_render` and `floppy_audio` depend
only on `floppy_core`; `floppy_io` depends on nothing. The three binaries
wire the crates together. `floppy_core` never touches pixels or samples — it
emits plain state and `BattleEvent`s that `floppy_render`/`floppy_audio`
turn into pictures and sound.

## Determinism

Given the same match seed and the same recorded input script, FLOPPY SPIN
reproduces an identical sequence of frame hashes (FNV-1a 64 over the
framebuffer) every single run — on any machine, any time. This is what makes
headless golden-frame/golden-WAV comparison meaningful rather than
approximate. It holds because the simulation never calls a libm
transcendental: `sin`/`cos` come from a shared 4096-entry quarter-symmetric
lookup table, `sqrt`/`rsqrt` from fixed-iteration Newton refinement of a
bit-trick seed, and all of it lives behind `floppy_core::fixmath` — enforced
by a dedicated grep test that fails the build if a stray `f32::sin` (or
similar) creeps in anywhere else in the codebase.
