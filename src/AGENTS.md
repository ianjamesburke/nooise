# src

## Purpose

All engine, terminal UI, and live-control code for the nooise binary.

## Ownership

- `main.rs` — binary entry point, wires up terminal + audio engine.
- `audio.rs` — cpal/audio-backend plumbing, sample callback wiring.
- `fluid.rs` — the core: all voice engines (Pad, Bass, Perc, Kick, Tonal, Clap), `FluidControls`/`MasterControls`/per-voice controls, tab UI (`tab_controls`/`apply_delta`/`apply_min`), chord progressions, master tune, and their tests. This is the largest and most actively changed file in the crate.
- `fx/` — shared DSP building blocks (LFO, panner, reverb) consumed by voices in `fluid.rs`. See `fx/AGENTS.md`.
- `synth/` — shared synthesis primitives (envelope, oscillator, noise) consumed by voices in `fluid.rs`. See `synth/AGENTS.md`.

## Local Contracts

- Per-voice control structs (e.g. `PadControls`, `BassControls`, `MasterControls`) live in `fluid.rs` next to the engine that consumes them — not split into separate files.
- Tab-indexed UI rows (`tab_controls`/`apply_delta`/`apply_min`) use a shared match-arm-by-row-index pattern per tab; adding a control row means adding matching arms in all three functions, appended at the end of that tab's existing rows to avoid renumbering.
- Pitched voices (Pad, Bass) route note numbers through `midi_to_hz`; unpitched voices (Perc, Kick, Tonal, Clap) do not and are unaffected by global pitch controls like master tune.

## Verification

- `cargo test` covers `fluid.rs` engine logic and UI control tables extensively (unit tests live inline in the same file via `#[cfg(test)] mod tests`).
- Run full `cargo build` after any function-signature change in `fluid.rs` — call sites across engines/tests are otherwise easy to miss.

## Child DOX Index

- `fx/AGENTS.md` — shared DSP effects (LFO, panner, reverb)
- `synth/AGENTS.md` — shared synthesis primitives (envelope, oscillator, noise)
