# src

## Purpose

All engine, terminal UI, and live-control code for the nooise binary.

## Ownership

- `main.rs` — binary entry point: CLI parsing (`run`/`version`/`update`/`render`/song code), wires up terminal + audio engine.
- `update_check.rs` — passive crates.io update notification helper; checks in the background and exposes a short TUI-safe message.
- `audio.rs` — cpal/audio-backend plumbing, sample callback wiring.
- `fluid/` — the core engine module:
  - `mod.rs` — crate-facing glue: `run()` (TUI + live audio), `render_wav()` (headless wav render), `FluidTelemetry`.
  - `controls.rs` — `FluidControls`, `MasterControls`, and per-voice control structs with defaults.
  - `registry.rs` — the control registry: one `ControlSpec` table per tab (stable ID, label, kind, range, step, entry semantics, reset, accessors, display). `tab_controls`/`apply_delta`/`apply_min`/`apply_value` all derive from it.
  - `automation.rs` — automation routes keyed by stable control ID. Each route holds one LFO (depth/interval/offset), edited via the `f` submenu in the TUI; LFO field specs own slider ranges, steps, reset targets, and numeric entry behavior. Routes drive runtime modulation and song-code persistence.
  - `song.rs` — versioned binary song-code export/import for controls plus automation records. `Ctrl+S` copies `nooise <code>` and shows a short confirmation; `nooise <code>` applies the decoded song state before audio/TUI startup.
  - `ui.rs` — TUI event loop, tab rendering, fluid visualizer.
  - `engine.rs` — `FluidEngine` (voice mixer), gain smoothers, tempo clock, grid triggers, ambient reverb send, master bus.
  - `voice/` — one module per voice (pad, bass, perc, kick, tonal, clap) plus shared helpers (`midi_to_hz`, `tune_ratio`, `soft_clip`, `normalized_lfo`) in `voice/mod.rs`.
- `fx/` — shared DSP building blocks (LFO, panner, reverb) consumed by voices. See `fx/AGENTS.md`.
- `synth/` — shared synthesis primitives (envelope, oscillator, noise) consumed by voices. See `synth/AGENTS.md`.

## Local Contracts

- The `ControlSpec` tables in `fluid/registry.rs` are the single source of truth for every control row and stable song snapshot ID. Adding a control = adding one table entry with a durable ID; never reintroduce per-function match arms for control rows. The `control_registry_specs_are_internally_consistent` test enforces table sanity.
- Control rows carry `ControlKind`; use it as the source of truth for gain/continuous/timing/discrete semantics.
- LFO submenu rows derive from `LfoFieldSpec`; keep LFO slider range, step, reset, display, and numeric-entry behavior there instead of adding per-key UI branches.
- Per-slider `f` automation is the user-facing LFO path; do not add voice-specific LFO rate/depth controls to core slider tabs.
- New LFO routes must start at 0% amount; opening the editor should be audible-neutral until the user raises amount.
- Grid-timing controls carry `LfoSnap` in their `ControlSpec` (intervals snap modulation to power-of-two subdivisions, offsets to their step grid); every modulated value — engine and UI marker alike — must come from `modulated_control_value` so what is shown matches what is heard. `GridTrigger` only pulls scheduled hits earlier between fires; grids that move later latch at the next fire so modulation can never starve a trigger.
- Live-read gain controls are ramped by registry-derived `GainSmoothers` in `FluidEngine`; every unique `ControlKind::Gain` spec must get a smoother automatically.
- TUI automation edits must go through `PublishedAutomation` so the shared audio-thread snapshot is stored on every mutation.
- Pitched voices (Pad, Bass, Tonal) route note numbers through `midi_to_hz` and respect master tune; unpitched voices (Perc, Kick, Clap) do not.
- Tonal separates trigger density from phrase shape: `tonal.rate_beats` controls note trigger spacing; `tonal.step_interval_beats` is the stable cycle-length ID for phrase wrapping and evolution boundaries.
- Tonal owns a slight fixed low cut before engine mixing so its low notes sit above sub/bass energy without requiring a user-facing control.
- Pad and Tonal emit dry voice output; `FluidEngine` owns their shared ambient reverb send/return so reverb mix changes do not add an uncontrolled per-voice wet gain boost.
- Voice RNGs must stay reseedable via `FluidEngine::reseed` so `nooise render --seed` stays byte-reproducible.
- Passive update checks must never block the TUI or audio callback; keep crates.io/network work off the main loop and show no message on failure.
- `nooise update` checks crates.io before invoking Cargo; do not force reinstall when the installed version is already current.

## Verification

- `just test` (cargo test) covers engine logic and the control registry; tests live in `fluid/tests.rs`.
- `just check` runs clippy across all targets.
- `nooise render --seconds N --seed N --out X.wav` (or `just render`) renders audio headlessly for DSP verification — same seed must produce byte-identical output.

## Child DOX Index

- `fx/AGENTS.md` — shared DSP effects (LFO, panner, reverb)
- `synth/AGENTS.md` — shared synthesis primitives (envelope, oscillator, noise)
