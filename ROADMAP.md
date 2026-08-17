# FLOPPY SPIN — Roadmap

Milestones are sequential; each ends with `cargo test` green, `cargo run --bin gate`
passing (size + import allowlist, once the exe exists at M0), and fresh-context
verifier review at the boundary. Cross-cutting invariants re-checked at EVERY milestone:
**determinism** (frame-hash test, no-libm guard), **size gate**, and from M5 on,
**balance** (Hard > Easy 70%, Ace never self-rings-out).

| M | Scope | Exit gate |
|---|---|---|
| **M0** | Workspace scaffold (4 lib crates + root, forbid(unsafe) boundaries), rust-toolchain.toml (gnu host), release profile, `gate` bin (size + PE import allowlist parser), win32 window + GDI StretchDIBits blit of animated gradient, `headless` bin writing PNG via own stored-deflate encoder, .gitignore, fmt/clippy config | exe builds & runs, window shows animation, headless PNG opens, gate passes, `cargo test` green |
| **M1** | `fixmath` (sin/cos LUT, fixed-iteration sqrt/rsqrt, atan2, xorshift64, Vec2/Vec3 — camera transforms are built from basis vectors in M3, no matrix type), fixed 120 Hz timestep + render interpolation skeleton, `InputState`, FNV-1a frame hash, scripted-input harness | determinism test: two runs same seed+script → identical hash sequence; math property tests; no-libm grep test |
| **M2** | Arena analytic heightfield + gradient, top dynamics (gravity/normal/slope/friction/decay), precession & topple, sphere collisions (de-penetration, restitution, knockback, drain, spin transfer, airborne), ring-out & stamina-out, launch trajectory, `BattleEvent`s, `TuneParams` block | invariant tests (collision symmetry & momentum, grounded stability, ring-out, topple), determinism green |
| **M3** | Software renderer: fixed camera, perspective, near clip, backface cull, z-buffer, flat/Gouraud + specular + emissive, surface-of-revolution top meshes (silhouette params), bowl tessellation + neon rings, 5×7 font + vector-AA numerals, headless golden-frame infra + tolerance compare | golden frames render & compare; first visual build (tops rolling in bowl); perf sanity at 960×540 |
| **M4** | Screen state machine + flow test, launch minigame, combat verbs (Guard/Hop/Carve/Anchor/Dash), 7 specials + meter + Crash-Out window, round scoring → first-to-4 match, hit-stop, HUD (panels, pips, banners) | flow reachability test, combat unit tests (verb inertness, counterplay smoke), playable match vs dummy |
| **M5** | Utility AI (`AiParams` per tier), Easy/Normal/Hard/Ace, verb usage gated by tier, special-escape reads | balance gates: Hard > Easy in > 70% over seed spread; Ace never self-rings-out in empty arena |
| **M6** | Audio: 16-voice mixer, oscillators + ADSR + soft-clip, BattleEvent SFX, tracker (menu + battle + intensity layer), waveOut ring in platform, headless WAV | WAV goldens reproducible; on-hardware glitch-free playback; sim untouched by audio (hash test unchanged) |
| **M7** | VFX & presentation: particles, emissive bloom, trails, shake, flashes, dust/sparks, title/menu polish, round choreography, settings screen (incl. colorblind, window scale) | golden frames updated; perf budget §10 holds; settings persist-safe |
| **M8** | Garage: 20 parts with signed deltas → plain Top resolve, garage screen with live preview, MY BEY in top-select, %APPDATA% save (version + validation fallback) | save corruption tests (missing/truncated/corrupt/wrong version → defaults); parts resolve test (no sim special-casing) |
| **M9** | Ship gate: size squeeze if needed, full golden set (title/launch/mid-fight/airborne-clash/ring-out/result), clippy/fmt clean, all invariants re-run, README, real-hardware checklist (input feel, waveOut, blit pacing) | every §12 SPEC gate green on a clean checkout |

Deferred / stretch (explicitly out of scope until M9 passes): ~~macOS backend~~
(**done 2026-08-17** — `src/platform/macos.rs`, winit/softbuffer/cpal, safe Rust;
it turned out to be testable after all, just not from Windows), XInput pad support
via runtime `LoadLibraryW`, replay export.

## Post-M9 status

M0–M9 are code-complete and every automated gate was green on a clean checkout at
`122e4bb`. Two things are still open:

1. **SPEC §12.5 real-hardware Windows checks** — `docs/real-hardware-checklist.md`
   is entirely unticked. This is the last M9 exit-gate item and it cannot be run
   anywhere but Windows.
2. **The clean-checkout ship gate is re-armed.** The macOS backend touched
   `main.rs`, `Cargo.toml`, and `src/platform/`, and per
   `docs/notes/ship-gate-clean-checkout.md` that resets the clock: the gate that
   counts is the one run on Windows on the latest commit. The macOS run
   (319 tests, goldens at diff 0.000, clippy/fmt clean) is evidence, not a
   substitute — it cannot measure the exe's size or its import table.

## Working method

Per milestone: (1) architect (this session) writes/updates module interface contracts;
(2) independent modules implemented by parallel Sonnet sub-agents (worktree isolation
when they touch shared files); (3) architect integrates; (4) fresh-context verifier
agents check the milestone against SPEC §12 + the three invariants; (5) findings fixed
before advancing; (6) lessons recorded in docs/notes/.
