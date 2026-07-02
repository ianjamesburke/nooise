# src

## Purpose

All engine, terminal UI, and live-control code for the nooise binary.

## Ownership

- `main.rs` — binary entry point: CLI parsing (`run`/`version`/`update`/`render`), wires up terminal + audio engine.
- `audio.rs` — cpal/audio-backend plumbing, sample callback wiring.
- `fluid/` — the core engine module:
  - `mod.rs` — crate-facing glue: `run()` (TUI + live audio), `render_wav()` (headless wav render), `FluidTelemetry`.
  - `controls.rs` — `FluidControls`, `MasterControls`, and per-voice control structs with defaults.
  - `registry.rs` — the control registry: one `ControlSpec` table per tab (label, kind, range, step, entry semantics, reset, accessors, display). `tab_controls`/`apply_delta`/`apply_min`/`apply_value` all derive from it.
  - `ui.rs` — TUI event loop, tab rendering, fluid visualizer.
  - `engine.rs` — `FluidEngine` (voice mixer), gain smoothers, tempo clock, grid triggers, master bus.
  - `voice/` — one module per voice (pad, bass, perc, kick, tonal, clap) plus shared helpers (`midi_to_hz`, `tune_ratio`, `soft_clip`, `normalized_lfo`) in `voice/mod.rs`.
- `fx/` — shared DSP building blocks (LFO, panner, reverb) consumed by voices. See `fx/AGENTS.md`.
- `synth/` — shared synthesis primitives (envelope, oscillator, noise) consumed by voices. See `synth/AGENTS.md`.

## Local Contracts

- The `ControlSpec` tables in `fluid/registry.rs` are the single source of truth for every control row. Adding a control = adding one table entry; never reintroduce per-function match arms for control rows. The `control_registry_specs_are_internally_consistent` test enforces table sanity.
- Control rows carry `ControlKind`; use it as the source of truth for gain/continuous/timing/discrete semantics.
- Live-read gain controls are ramped by `GainSmoothers` in `FluidEngine`; do not apply UI volume jumps directly in the audio callback path.
- Pitched voices (Pad, Bass) route note numbers through `midi_to_hz`; unpitched voices (Perc, Kick, Tonal, Clap) do not and are unaffected by global pitch controls like master tune.
- Voice RNGs must stay reseedable via `FluidEngine::reseed` so `nooise render --seed` stays byte-reproducible.

## Verification

- `just test` (cargo test) covers engine logic and the control registry; tests live in `fluid/tests.rs`.
- `just check` runs clippy across all targets.
- `nooise render --seconds N --seed N --out X.wav` (or `just render`) renders audio headlessly for DSP verification — same seed must produce byte-identical output.

## Child DOX Index

- `fx/AGENTS.md` — shared DSP effects (LFO, panner, reverb)
- `synth/AGENTS.md` — shared synthesis primitives (envelope, oscillator, noise)
