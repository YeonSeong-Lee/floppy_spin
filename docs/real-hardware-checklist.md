# Real-hardware checklist (SPEC §12.5 / ROADMAP M9)

Everything in this repo's automated gates (`cargo test`, `cargo run --bin
headless`, `cargo run --bin gate`, clippy/fmt) runs headlessly and proves the
simulation, rendering, and audio are byte-for-byte deterministic. None of it
can observe how the game actually *feels* on a real Windows machine: input
latency, waveOut buffering behavior, GDI blit pacing, and window/save
behavior under real OS conditions are not visible to a headless run. This
list is the manual spot-check to run on real Windows hardware before
shipping, and after any change that touches `src/main.rs` or
`src/platform/win32.rs`.

Run `floppy_spin.exe` directly (not through a debugger) for every item below.

## Input feel

- [ ] Arrow keys respond immediately with no perceptible input lag in the
      Fight phase.
- [ ] All seven verb keys (Space dash, Shift special, Z guard, X hop, C
      carve, Ctrl anchor, arrows aim/move) register reliably under rapid
      repeated presses — no dropped presses, no keys that intermittently
      fail to register.
- [ ] No "stuck key" behavior: releasing a key stops its effect immediately;
      holding Ctrl (anchor) or Shift (special) does not interfere with the
      OS's own sticky-keys/toggle behavior in a way that breaks control.
- [ ] Camera-relative directions feel right: pressing Right visibly moves
      your top toward screen-right and pressing Up moves it away from the
      camera, consistently, from both players' starting sides of the arena.
- [ ] Menu navigation (arrows + Space/Z/Esc) feels responsive with no
      missed presses when moving quickly through Main Menu / Garage /
      Settings / Top Select.
- [ ] The Launch minigame's fast-moving power sweep is controllable — the
      Space-to-lock timing window feels fair and consistent with what's
      drawn on screen (no felt input-to-render lag on the marker).

## Audio (waveOut)

- [ ] Menu music plays on Title/Main Menu/Garage/Settings/Top Select and
      loops cleanly with no audible seam or gap.
- [ ] Music switches to the battle theme when entering Match (Intro/Launch/
      Fight/Decided/Round Result) and back to menu music on return to the
      Main Menu.
- [ ] SFX fire audibly and promptly on hits, dashes, guards/parries, hops,
      carves, wall bounces, specials firing, Crash-Outs, ring-outs, and
      topples — no missing or delayed one-shots during a busy exchange.
- [ ] The spin-hum tone audibly tracks each top's RPM (pitch/character
      changes as spin decays) rather than sounding static.
- [ ] The battle theme's intensity layer audibly kicks in while a special is
      armed (meter full) and drops back out once it's consumed/expires.
- [ ] No glitches, underruns, crackle, or pops during sustained play,
      including during dense particle/SFX moments (multiple simultaneous
      hits, a Crash-Out finish).
- [ ] Music-volume and SFX-volume settings in the Settings screen audibly
      change the correct channel in real time, including muting at 0.

## Blit pacing

- [ ] The game holds a steady 60 fps during normal Fight-phase play on
      target-class hardware (~2015 dual-core x64) with no visible stutter.
- [ ] No visible tearing during the GDI blit under normal window movement/
      focus changes.
- [ ] No frame-pacing hitches when particle-heavy events fire (Crash-Out,
      simultaneous hits, ring-out) or when the bloom/post pipeline is at its
      busiest.
- [ ] Frame time has visible headroom — task-switching to another window and
      back, or briefly moving the game window, does not cause a lasting
      slowdown once focus returns.

## Window

- [ ] The window opens at the correct size for the default window-scale
      setting (960x540 at 1x, scaled correctly at 1.5x/2x).
- [ ] Changing the window-scale setting (1x / 1.5x / 2x / borderless
      fullscreen) in Settings visibly resizes/restyles the window correctly,
      and the internal render resolution stays a crisp 960x540 upscaled (no
      corruption, no stretched-wrong-aspect artifacts).
- [ ] Borderless fullscreen covers the full screen with no visible border
      and correctly restores to windowed mode when toggled back.
- [ ] Closing the window (title-bar close button and Alt+F4) exits cleanly
      with no hang, no crash dialog, and no leftover process in Task
      Manager.

## Save

- [ ] Settings changes (volumes, screen-shake level, difficulty, window
      scale, colorblind mode) persist across a full app restart.
- [ ] A Garage build (part selections) persists across a full app restart
      and MY BEY still reflects the same build after relaunch.
- [ ] Deleting `%APPDATA%\floppy_spin\save.bin` and relaunching falls back
      to defaults cleanly (no crash, no error dialog, sensible default
      settings/garage).
- [ ] A corrupted save file (e.g. truncate it or overwrite a few bytes in
      the middle) is handled the same way — falls back to defaults silently
      rather than crashing or partially applying garbage state.
- [ ] A save file from a different/older version byte is rejected cleanly
      and falls back to defaults rather than misinterpreting the layout.

## Full match, start to finish

- [ ] From Main Menu, Quick Battle -> Top Select -> pick a top -> Match
      Intro countdown -> Launch minigame -> Fight -> a round resolves
      (Crash-Out, ring-out, stamina-out/topple, or timeout) -> Round Result
      -> next round (or Match Over once a side reaches the winning score)
      -> back to Main Menu, all playable without any headless/debug
      assistance, using only keyboard input.
- [ ] Losing and winning both read clearly on screen at every transition
      (round banners, score pips, match-over banner).

## Garage

- [ ] Build a custom top in the Garage screen (change at least the Frame
      and one stat-delta slot) and confirm the live stat readout and
      preview update immediately as parts change.
- [ ] MY BEY appears as its own entry in Top Select and reflects the
      current Garage build's stats/accent/spin direction (not a stale
      snapshot from a previous session).
- [ ] Fight a full match using MY BEY and confirm it behaves consistently
      with its displayed stats (e.g., a heavy-ATK build visibly hits harder,
      a heavy-DEF build visibly survives longer) — no crashes or
      obviously-wrong behavior specific to a custom build.
