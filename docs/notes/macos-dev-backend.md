# Lesson: a second backend is a determinism test you can't fake

The macOS dev backend (SPEC §2) existed in the docs long before it existed on
disk — `README.md` and `SPEC.md` both drew `src/platform/macos.rs` into the
architecture tree while `git log` showed the file had never been committed.
Aspirational documentation reads exactly like descriptive documentation. Worth
checking the tree, not the diagram.

Implementing it paid back more than convenience:

- **It re-verified determinism across an architecture boundary.** All six
  golden PNGs, generated on Windows/x86_64, compare on macOS/aarch64 at
  `mean_abs_diff = 0.000` — byte-identical, not merely inside the ±2 tolerance.
  The `fixmath` discipline (LUT sin/cos, Newton sqrt, no libm) is what makes
  that true, and until there was a second architecture to run on, "deterministic"
  only ever meant "reproducible on one machine".
- **A shared `main.rs` turns one compile into two.** Both backends export the
  same API and are re-exported as `platform::backend`, so `main.rs` is not
  cfg-split. Compiling on macOS therefore type-checks the exact `main.rs` source
  Windows compiles — a Windows-only regression in that file would have to be
  invisible to the type system to survive.

## Gotchas worth remembering

- `rust-toolchain.toml` pins a Windows *host* toolchain, so bare `cargo`
  anywhere else dies with `target tuple in channel name`. `RUSTUP_TOOLCHAIN=stable`
  is the override that wins (a `rustup override` does **not** beat the file).
- winit only creates windows inside `ApplicationHandler::resumed`, but
  `Platform::init` has to hand back a live window. The fix is to
  `pump_app_events` in a bounded loop inside `init` until the handler has stored
  one.
- Ask cpal for 44.1 kHz explicitly. CoreAudio's default is 48 kHz, and since
  every note is synthesized against `floppy_audio::SAMPLE_RATE`, taking the
  default silently transposes the entire soundtrack.
- Size the window in *logical* points, not physical pixels. `win32` uses
  physical, which on a 2x Retina display would produce a window a quarter of the
  intended area.

## Don't over-read a failed synthetic input test

Driving the game with `osascript ... key code` looked like proof the input path
was broken: the window was frontmost, Escape was sent, the game didn't quit.
Tracing `WindowEvent` showed the opposite — the key arrived correctly decoded,
and `keys[0x1b]` went `true` then `false`. A synthetic keystroke's press and
release land in the *same* `poll()`, so a polling loop can never observe the
press. `win32` drains `PeekMessageW` the same way and would miss it identically.

The lesson is the method, not the finding: when a black-box test says "broken",
instrument the boundary before believing it. Two screenshots and a guess would
have recorded a bug that does not exist.

**Still unverified by any of this:** how the backend *feels* — input latency,
audio glitching, frame pacing. That's the same gap `docs/real-hardware-checklist.md`
exists to close on Windows, and the macOS side has no equivalent checklist run.
