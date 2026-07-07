# FLOPPY SPIN — Game Design ("what makes it fun")

Synthesized from three parallel design drafts (combat / roster / feel) on 2026-07-03.
All numbers are **starting tunings**; `TuneParams` in code is canonical once implemented.
Deviations from the drafts and their reasons are recorded in §9.

## 0. Design pillars

1. **Every action has a price.** Spin is health, stamina, and currency at once; no held
   button wins on its own. Risk/reward everywhere, no infinite turtle.
2. **The bowl is a weapon.** Altitude, slope, and rim proximity change what every verb
   does. Skilled play *looks* different: wall-rides, aerial slams, downhill dashes.
3. **Specials are a mind-game, not a cutscene.** Arming is public information; the
   1.2 s Crash-Out window is a bidirectional read (attacker converts, defender escapes).
4. **Determinism is a feature.** Same seed, same fight — replays, tests, and fairness.

## 1. Core resources

- **Spin** (= stamina = axial angular speed): `SPIN_MAX = 10_000` units. Launch grants
  ~7,000–10,000 depending on the minigame. Passive decay ≈ `34 − 20·STA/100` per
  second; a clean neutral hit drains ~400 (scaled by ATK/DEF). Spin → 0, or wobble past
  the tilt threshold while slow, = **topple** = stamina-out.
- **Meter**: 0–100. Fills from combat (below); at 100 the special becomes **Armed**
  (public glow + SFX). **Shift fires it manually** — arming and firing are separate
  moments on purpose. Firing zeroes the meter and opens the 144-step (1.2 s) Crash-Out
  window. Meter does not persist across rounds.
- **Tilt** (precession): grows as spin drops and from Carve abuse; drives wobble and
  topple. It is sim state — the wobble you see is the wobble that kills you.

## 2. Combat verbs

Controls: arrows = camera-relative movement · **Space** = Dash · **Shift** = Special ·
**Z** = Guard · **X** = Hop · **C** = Carve · **Ctrl** = Anchor. Every verb is one
`InputState` bool, fully inert unless pressed. Times in sim steps @120 Hz.

| Verb | Type | Effect | Cost / risk |
|---|---|---|---|
| **Dash** (Space) | edge | +6 m/s impulse along input (additive, momentum & slope carry through), clamp 11 m/s. Startup 2, active 12, recovery 8, cooldown `72 − 0.4·SPD` steps (SPD 50 → 52). Hits during active: knockback ×1.4, drain ×1.25, shove-armor vs knockback < 2 m/s. One air-dash per airborne state. | cooldown-gated; committed forward hitbox, deflectable |
| **Guard** (Z) | held | frontal-hemisphere barrier: incoming knockback ×0.25, drain ×0.35 — front 180° only. Startup 4, drop-recovery 6. **Parry**: hits in the first 8 steps of a press → knockback ×0, drain ×0.1, attacker staggered (instant tilt +0.12 rad), +12 meter. | drains 90 spin/s, move ×0.4; open from behind/above; slides downhill ×0.6 on slopes > 6° (turtling on the wall drifts toward ring-out) |
| **Hop** (X) | edge | vertical impulse +4.5 m/s. Startup 3, de-penetration i-frames steps 4–12, land-lag 10 (vulnerable). Holding a direction in air = **aerial slam**: drain `250 + 8×fall_height_m` (cap 900), knockback likewise. Airborne: cannot Guard/Anchor; immune to ground-tracking effects. | 120 spin per hop; punishable landing |
| **Carve** (C) | held | lean the spin axis into motion: top speed ×1.5, slope climb ×1.8 (the wall-ride verb), contact knockback ×1.35. Ramp-in 20 steps. | tilt +0.05 rad/s while held (decays over ~30 steps after release); over-carving topples **you** |
| **Anchor** (Ctrl) | held | grip terrain: incoming knockback ×0.2, ring-out/downhill slide ×0.1, tilt recovers 0.08 rad/s, **spin regen +150/s**. Startup 6, release 8. | move ×0.1 (rooted), deals nothing, builds **no** meter, auto-breaks on slopes > 12°; opponent charges freely while you sit |

**Counterplay loop:** Carve → beats Anchor → beats Hop(-slam) → beats Guard → beats
Carve, with Dash and Parry as the neutral-skill layer punishing over-commitment.

**No dominant strategy (math sketch):**
- *Guard turtle*: barrier 90/s + passive ~25/s + leaked hits ≈ −200/s vs an attacker's
  −25/s — turtle topples first, while sliding rimward. Loses.
- *Anchor turtle*: +150/s regen − pressure drain ≈ net negative under attack, and zero
  meter income means the opponent's special arrives uncontested. Only pays when
  unpressured — and an unpressured opponent is charging on you.
- *Carve aggression*: continuous hold reaches topple tilt in ~18 s; must be pulsed.

**Escaping an armed special (the Crash-Out read):** Guard never survives a special.
The defender reads the special type and picks: **Hop** clears ground
shockwaves/tracking (risk: land-lag), **Anchor** survives knockback/ring-out pushes
near the rim (risk: rooted for the follow-up), **Carve** outranges slow lunges/zones
(risk: wobble). Reading wrong is the Crash-Out. Higher AI tiers make these reads.

**Advanced techniques (what Hard/Ace demonstrate):**
1. *Wall-ride slam*: Carve up the wall (+3–4 m), Hop at apex, directional slam into the
   basin — hardest single hit in the game (~850 drain); a miss lands you rim-side.
2. *Downhill dash-cancel*: crest a cross-hill with Carve, release into a downhill Dash —
   gravity + impulse stack to `dash_max` for maximum knockback at basin center.
3. *Parry-to-Crash-Out*: with special armed, bait a lunge, parry (stagger, +12 meter),
   fire into the stagger — converts defense into the 3-point finish.

**Meter economy** (all × `0.7 + 0.6·MTR/100`): drain dealt ×0.01 (a 400-hit = +4),
drain taken ×0.0067 (comeback bias), Parry +12, i-frame dodge of a real hit +8, landed
aerial slam +15, dash-hit +5, passive trickle +1.5/s (a pure stalemate still arms
eventually). Anchor and walking grant nothing.

## 3. Stats & roster

Six stats 0–100, preset budget 300±6 (no free lunch). BASE = 50 across the board.

| Stat | Sim mapping |
|---|---|
| ATK | knockback multiplier `0.5 + ATK/100`; raises drain dealt |
| DEF | drain taken ×`(1 − 0.6·DEF/100)`; knockback taken ×`(1 − 0.4·DEF/100)` |
| STA | passive spin decay `34 − 20·STA/100` /s |
| WGT | mass `20 + 0.6·WGT`; momentum exchange, ring-out resistance |
| SPD | control accel `6 + 0.10·SPD` m/s²; dash cooldown `72 − 0.4·SPD` steps |
| MTR | meter gain ×`(0.7 + 0.6·MTR/100)` |

Derived: tilt-recovery torque ∝ WGT·STA (low both → topples early). Spin direction
matters: opposite-spin contact maximizes spin transfer (grind duels), same-spin
maximizes knockback exchange (clash duels).

### Presets (7)

| Name | Type | ATK/DEF/STA/WGT/SPD/MTR | Spin | Accent | Silhouette |
|---|---|---|---|---|---|
| **Cleaver** | Attack | 88/30/34/58/62/30 | +1 | `#FF2D55` | wide flat flange at 0.8h, razor lip, 6-lobe sawblade scallop |
| **Bulwark** | Defense | 28/90/52/78/22/30 | −1 | `#2D7DFF` | squat dome, rounded shoulders, 12 shallow glancing lobes |
| **Everspin** | Stamina | 24/44/92/40/48/52 | +1 | `#39FF14` | narrow tall cylinder, smooth, sharp low tip |
| **Keystone** | Balance | 52/54/52/50/50/44 | −1 | `#FFD400` | balanced trapezoid, mild 8-facet |
| **Riptide** | Assault-Drift | 70/26/40/34/84/46 | −1 | `#00E5D0` | skewed teardrop, asymmetric flare at 0.65h, 3-lobe hooks |
| **Gravewell** | Siege-Anchor | 46/72/30/90/20/42 | +1 | `#B026FF` | massive inverted bell, widest at 0.2h, monolithic 2-lobe |
| **Mirrorfang** | Reversal | 40/66/58/44/46/48 | −1 | `#FF7A00` | hourglass pinched at 0.5h, mirrored twin fang flares |

Flavor lines per top live with the roster table in code (shown on TopSelect).

### Specials (fired with Shift when Armed; durations @120 Hz)

| Top | Special | Effect | Counterplay |
|---|---|---|---|
| Cleaver | **Guillotine Rush** | 48 steps: accel ×2.2, dash CD 0, steering-homing 0.3/step; next contact ×1.8 knockback + one-time +22 impulse | sidestep the committed line; a whiff leaves 0.8 s of the window exposed, self-ring-out risk |
| Bulwark | **Aegis Lock** | 150 steps: DEF→100-equivalent, knockback taken ×0.35, reflects 50% of absorbed drain; rooted | outlasts the 1.2 s window by design — disengage and stamina-race; can't chase you |
| Everspin | **Second Wind** | instant +18% max spin; 240 steps of near-zero decay, tilt recovery ×1.5 | no knockback protection — punish with raw aggression/ring-out |
| Keystone | **Overclock** | 120 steps: all effective stats +12 | out-specializes nobody; exactly sub-window so a timed counter-kill lands as it fades |
| Riptide | **Slipstream** | 60 steps intangible glide; passes through once, exit hit ×1.6 (backstab bonus only if exit angle > 90° off defender facing); accel ×1.8 | telegraph flash 8 steps before the pass — turn to face; low WGT means a rim clip is death |
| Gravewell | **Sinkhole** | 180 steps: local gravity well +3.5 m/s² toward self within 2.4 m, local floor depression; self immune | high-SPD tops out-climb the pull (SPD ≳ 70); the well sets up but doesn't kill — the follow-up must land inside 1.2 s |
| Mirrorfang | **Riposte** | 90-step parry window: negates the hit, returns ×1.4 knockback along attacker velocity + 60% drain transfer; fizzle refunds 30 meter | don't attack into it — wait it out; firing your special into an armed Riposte turns your kill into your death |

Diversity check: movement (Rush, Slipstream) / zone-terrain (Sinkhole) / buff (Second
Wind, Overclock) / defensive-reversal (Aegis, Riposte). Every special's counterplay is
one of the §2 escape verbs or spacing, so the Crash-Out read stays learnable.

### Balance triangle

**Attack ▶ Stamina ▶ Defense ▶ Attack.** Attack ring-outs endurance before the spin
race matters; Stamina outlasts low-ATK walls; Defense reflects and shrugs off burst
while its WGT resists ring-out. Balance/exotics sit off the triangle: Keystone is the
safe pick, Riptide a fragile positional Attack, Gravewell bends the triangle by
punishing slow Stamina tops, Mirrorfang preys on special-over-commitment. The 300±6
budget and paired-delta parts keep the triangle honest.

## 4. Launch minigame (start of every round)

Sequence (~2 s ritual, all pre-contact): aim → spin direction → power.
- **Aim** (◄/► rotate heading around the bowl lip, ▲/▼ pick entry depth 0.4–1.0 of
  `V_MAX`): glowing reticle orbits the lip.
- **Spin direction** (Shift toggles): chevron ribbon reverses with a whoosh; tooltip
  "GRIND" (opposite) / "CLASH" (same) vs the opponent's locked direction.
- **Power** (Space locks): marker sweeps 0→100→0, period 0.66 s Normal (0.80 Easy,
  0.52 Hard/Ace — the AI rolls its own quality from skill params). Sweet spot center
  86%, width 6% → **PERFECT**: RPM ×1.20 + bonus start meter (+10). Good band 72–94%:
  ×1.08. Below 72: ×1.00. **Overcharge** (lock on the descending pass > 94%): ×1.12 but
  +0.06 rad starting tilt — the risk lane.
- Presentation: procedural rip-cord winds tighter, charge glow dim-cyan→hot-white,
  countdown beeps 660/770/880 Hz, "GO!" stinger; dust puff + afterimage burst on drop.
- Launch spin = `(7000 + 2000·power_frac) × quality`, clamped to `SPIN_MAX`.

## 5. Juice table (render/audio only; hit-stop is the one sim-visible knob)

1 step = 8.33 ms. Shake in px @960×540 (summed, clamped 14 px, scaled by the settings
multiplier ×0/0.5/1/1.5). Flash = full-screen additive. All flashes/neon feed bloom.

| Moment | Hit-stop | Shake amp/decay | Flash | Particles | SFX |
|---|---|---|---|---|---|
| Light hit | 1 | 2 px /0.80 | — | 6 white-cyan sparks, 180 ms | tri click 300→220 Hz, 60 ms |
| Heavy hit | 4 | 7 px /0.86 | white .35, 60 ms | 20 hot-white→amber, 350 ms | saw+noise crunch 180→90 Hz, 140 ms |
| Airborne clash | 6 | 9 px /0.88 | cyan .30, 80 ms | 28 cyan cone-up, 500 ms | detuned saw swell 120→300 Hz |
| Wall bounce | 2 | 4 px /0.82 | ring tint .15 | 10 dust tangential | noise thud + tri ping, 90 ms |
| Dash | 0 | 1 px | — | 8 accent streaks behind | noise whoosh, HP-swept, 160 ms |
| Guard block / Parry | 2 | 3 px /0.85 | white .20, 50 ms | 12 silver arc | metallic clink, sines 1200+1800 Hz |
| Special fire | 3 | 6 px /0.84 | accent .40, 100 ms | 40 accent spiral, 600 ms | saw chord swell + noise, 400 ms |
| **Crash-Out** | **10** | 12 px /0.90 | white .60, 120 ms | 60 white→accent shards, 800 ms | sub-boom + descending saw, 600 ms |
| Ring-out | 5 | 8 px /0.87 | red .30, 90 ms | 24 red-orange falling | doppler whistle 800→200 → thud |
| Topple | 4 | 5 px /0.80 | amber .25, 100 ms | 16 amber collapse | detuning sine wobble 400→80 Hz |
| Round win | 3 | 4 px /0.78 | gold .30, 150 ms | 30 gold sparkle | major-triad arpeggio |
| Match win | 8 | 6 px ×2 pulses | gold .45, 250 ms | 80 gold fountain, 1 s | 4-note fanfare, 1.2 s |

## 6. Aesthetic — "PS1-meets-TRON"

Palette: void `#0A0A14`, bowl metal `#141826`, grid lines `#1E2A44`; neon accents are
the tops' colors plus ice-white `#F0FFFF` highlights. Keep base geometry dark and let
emissive bloom do the lifting. Bloom: emissive-tagged pixels always bloom; two-pass box
blur (4 px + 8 px at half-res), added back at 0.6 gain. Commit to the constraints:
flat/Gouraud + one hard specular hotspot reads as chrome; 1 px emissive rim on neon
edges gives the vector-graphics feel; subtle ordered-dither on ambient gradients makes
banding look intentional; faint scanline + vignette (α 0.08) unifies the CPU frame.
Arena rings pulse on music downbeats **driven by tracker row index** (kick rows set
`ring_pulse = 1.0`, decay ×0.88/frame) — deterministic, identical on every machine.

## 7. UI/UX

- **Title**: chunky vector "FLOPPY SPIN", cyan fill, magenta drop-offset with ±0.5 px
  sine jitter; neon-ring backdrop; an idle top spinning with trail; "PRESS START"
  blinking at 1.2 s. One-bar battle-theme hook as stinger.
- **Menus**: vertical list, glowing chevron cursor with ~120 ms spring settle; move =
  tri blip 440 Hz; select = 440→660; back = falling pair; diagonal neon wipe, 9 frames.
- **Battle HUD** (decision: **fixed corner panels**, not orbiting arcs — arcs force the
  eye to track moving targets and clutter the collision read at bird's-eye scale).
  P1 top-left / P2 top-right: 270° stamina arc in the top's accent (flashes white
  ≤ 20%), RPM numeral (vector-AA), special charge bar (glow + tick when Armed). Score
  pips center-top: 4 hollow neon diamonds per side (Crash-Out fills 3 with a pop).
  Edge vignette in a player's color when their stamina is critical.
- **Round choreography**: READY? → 3/2/1 at 1.0 s each (beeps, numeral scale 1.3→1.0 +
  bloom) → GO! 1.4× white flash. Outcome banners (CRASH-OUT!! / RING OUT! / TOPPLE!)
  slam in at 1.5× overshoot, settle 10 frames. RoundResult tallies pips with a ting
  each 120 ms; MatchOver = banner + gold fountain + REMATCH / MENU.
- **Colorblind mode**: remaps risky accent pairs (lime→ice-blue, orange→amber-white),
  adds shape tags to pips (P1 ◯ / P2 △) and stripes to stamina arcs — hue is never the
  sole signal.

## 8. Audio identity

- **Menu theme**: 112 BPM, A natural minor, 8-bar synthwave loop; pulse1 rolling A-C-E
  arpeggio (25% duty, warm), pulse2 soft pad, bass root-fifth on 1 & 3, lazy
  kick/snare/offbeat hats.
- **Battle theme**: 148 BPM, A minor with Phrygian ♭2 bite; 4-bar riser → 8-bar groove
  → 4-bar break; galloping eighth bass, four-on-floor kick, snare 2 & 4, 16th hats,
  50% duty lead. **Intensity layer**: while any special is Armed, unmute a pre-authored
  counter-melody + denser hats starting at the next bar boundary (row % 16 == 0);
  one-bar tom fill on special fire / Crash-Out. All keyed off row index.
- **SFX palette** (~14, waveform recipes in the feel draft, ADSR in ms): menu move/
  select/back, countdown beeps, launch rip (noise+saw 200→600 + sweet-spot ting), spin
  hum loop (saw+sine, pitch tracks RPM — the top audibly *tires*), light/heavy hit,
  dash whoosh, guard clink, parry ting, special charge/fire, Crash-Out sting, ring-out
  doppler+thud, topple wobble, score tally ting. Voice priority: hits/stings preempt
  the hum, never the music channels.

## 9. Synthesis decisions (deviations from the drafts)

1. **Meter scale unified** on 0–100 with the combat draft's gain table + the roster
   draft's MTR multiplier and a slower trickle (1.5/s, not 6/s — specials should
   usually be earned in combat).
2. **Manual special fire** (Armed at 100, Shift to fire) over the roster draft's
   auto-fire: the armed state is the mind-game, and firing is a skill decision
   (see Parry-to-Crash-Out).
3. **Player colors = top accents** everywhere (HUD, particles, banners); the feel
   draft's fixed cyan/magenta P1/P2 assignment is dropped, colorblind mode handles
   ambiguous pairs.
4. **Launch controls are keyboard-only** (no mouse/analog): ◄/► heading, ▲/▼ depth,
   Shift spin-direction, Space power lock. Overcharge lane kept, but its cost is
   +starting tilt (interacts with the precession system) instead of a stamina pip.
5. **Internal resolution fixed at 960×540**; the settings "resolution" entry scales the
   window (1×/1.5×/2×/fullscreen), not the internal buffer (spec C7 fixes perf there).
6. Guard's old "Defense", Hop's "Jump", Anchor's regen "Recover" from the previous
   generation are folded into the four-verb set; sharper costs, terrain interactions.
