# nooise — PRD

## Vision

A generative focus music engine for ADHD. Infinite, never repeating, never boring. Inspired by Max Richter's Sleep -- long tonal phrases, polyrhythmic phasing, subtle mutation over time. Not a song player. Not a beat generator. A sound environment you work inside.

Tempo: 100–120 BPM. Pentatonic. Wind-chime tonal character. Things drift, nothing snaps.

---

## Phase 1 — Sound Exploration (Rust, `--experiment` flag)

Single binary. `cargo run -- --experiment t1`. Plays until Ctrl+C. No TUI.

Goal: find the 2-3 sonic ideas worth productizing before building UI.

### Project structure

```
nooise/
  nooise-engine/
    src/
      main.rs
      synth/
        oscillator.rs   -- sine oscillator, FM synthesis (bell/wind-chime timbre)
        envelope.rs     -- ADSR
        noise.rs        -- white noise, filtered variants
      sequencer.rs      -- polyrhythm scheduler, LCM-based independent loop clocks
      fx/
        reverb.rs       -- Freeverb algorithm
        lfo.rs          -- drifting LFO (rate itself slowly random-walks)
        panner.rs       -- tempo-synced L/R autopanner
      experiments/
        mod.rs
        t1.rs  t2.rs  t3.rs  t4.rs
        r1.rs  r2.rs  r3.rs  r4.rs
  PRD.md
```

### Core crates

- `cpal` -- cross-platform audio I/O
- `rand` -- stochastic elements
- `dasp` -- signal utilities (optional)

### Tonal / Max Richter experiments

| # | Experiment | What it explores |
|---|---|---|
| T1 | Single pentatonic melodic line, 3-bar phrase over 4-bar grid, slow mutation each cycle | Core phasing/variation feel |
| T2 | Two voices, 3-over-2 polyrhythm, long note decay, slight pitch drift on repeat | Reich-style phase lock/unlock |
| T3 | Layered sustained tones (pad-like), slow LFO on reverb depth, no melody | Texture without movement |
| T4 | Full tonal stack: phasing melody + pad + drifting LFO on EQ | Kitchen sink tonal |

### Rhythmic / bilateral experiments

| # | Experiment | What it explores |
|---|---|---|
| R1 | Bilateral only -- 1/4 note L/R ping-pong sine at 120 BPM | Is bilateral alone tolerable / useful? |
| R2 | Abstract noise pattern -- 16th grid, variable accent + decay, no melody | Rhythmic texture feel |
| R3 | Noise pattern + bilateral | Minimal ADHD stack |
| R4 | Best tonal experiment + bilateral + noise | Full combined system |

**Exit criteria:** listen to all 8, pick 2-3 that feel right. Those become Phase 2 scope.

---

## Phase 2 — Harden the Engine

Productize the winning experiments. Still no TUI -- runs headless.

- Lock the sound model from Phase 1 findings
- Bilateral autopanner tempo-synced (1/4 note L/R at session BPM)
- Drifting LFO on reverb + EQ (rate random-walks within a range over minutes)
- Control protocol: newline-delimited JSON over stdin/stdout (prep for Phase 3)
- Clean infinite playback -- no audible loops, no clicks or pops at phrase boundaries

---

## Phase 3 — Minimal Textual TUI

Build only the controls the sound actually needs. Derived from Phase 1/2 learnings.

Likely controls:
- Layer toggles (melody / bilateral / noise)
- Tempo
- Mutation rate
- Noise accent density
- Master volume
- One useful visual (not decorative)

Dark theme. Ships as `uv tool install nooise` with pre-compiled Rust binary bundled.

### Architecture

- **`nooise-engine`** (Rust) -- all audio, no UI, JSON control protocol
- **`nooise`** (Python/Textual) -- UI shell, launches engine subprocess
- **Bridge** -- bidirectional newline-delimited JSON over pipes

---

## Not in scope (any phase)

- Saving presets
- MIDI in/out
- Recording output
- Session timers
- Mobile
