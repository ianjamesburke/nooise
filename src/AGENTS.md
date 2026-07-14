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
  - `automation.rs` — modulation routes keyed by stable control ID. A control can carry an independent LFO route (`f` submenu: shape/amount/interval/offset) and/or a one-shot envelope route (`e` submenu: amount/attack/decay/trigger); `modulated_control_value_full` sums both, clamps, then snaps. LFO field specs own slider ranges, steps, reset targets, and numeric entry; envelope field behavior lives on `EnvelopeRoute`. LFO shapes cover sine/triangle/ramp/square plus seeded random drift and sample & hold (pure `(seed, cycle index)` hash — no RNG state — reseedable via `LfoRoute::reseed`). Routes drive runtime modulation; song-code persists LFO routes (incl. shape), envelope routes are not persisted on the experiment branch.
  - `song.rs` — versioned binary song-code export/import for controls plus automation records. `Ctrl+S` copies `nooise <code>` and shows a short confirmation; `nooise <code>` applies the decoded song state before audio/TUI startup.
  - `ui.rs` — TUI event loop, tab rendering, fluid visualizer.
  - `engine.rs` — `FluidEngine` (voice mixer), gain smoothers, tempo clock, grid triggers, ambient reverb send, master bus.
  - `voice/` — one module per voice (pad, bass, perc, kick, tonal, clap, arp) plus shared helpers (`midi_to_hz`, `tune_ratio`, `soft_clip`, `normalized_lfo`) in `voice/mod.rs`.
- `fx/` — shared DSP building blocks (LFO, panner, reverb) consumed by voices. See `fx/AGENTS.md`.
- `synth/` — shared synthesis primitives (envelope, oscillator, noise) consumed by voices. See `synth/AGENTS.md`.

## Local Contracts

- The `ControlSpec` tables in `fluid/registry.rs` are the single source of truth for every control row and stable song snapshot ID. Adding a control = adding one table entry with a durable ID; never reintroduce per-function match arms for control rows. The `control_registry_specs_are_internally_consistent` test enforces table sanity.
- `song.rs`'s snapshot codec is fully generic over `all_specs()`: any state expressible as `ControlSpec` rows persists and round-trips automatically, and old codes missing a newer id simply decode that field to its default — no `song.rs` change needed when adding controls this way. Only reach for a new versioned payload/section in `song.rs` for state that cannot be expressed as a flat control value (e.g. automation routes).
- Control rows carry `ControlKind`; use it as the source of truth for gain/continuous/timing/discrete semantics.
- Continuous LFO submenu rows derive from `LfoFieldSpec` (range/step/reset/display/entry); the discrete LFO shape field and all envelope fields are handled on `LfoRoute`/`EnvelopeRoute` instead of per-key UI branches.
- Per-slider `f` (LFO) and `e` (envelope) automation are the user-facing modulation paths; do not add voice-specific LFO/envelope rate/depth controls to core slider tabs.
- New LFO routes start at 0% amount and new envelope routes at 0 amount; opening either editor is audible-neutral until the user raises amount, and `close_editor` drops a still-neutral route.
- Every modulated value shown or heard must come from `modulated_control_value_full` (LFO + envelope summed, clamped, snapped); never add divergent UI-only modulation math.
- Grid-timing controls carry `LfoSnap` in their `ControlSpec` (intervals snap modulation to power-of-two subdivisions, offsets to their step grid); every modulated value — engine and UI marker alike — must come from `modulated_control_value` so what is shown matches what is heard. `GridTrigger` only pulls scheduled hits earlier between fires; grids that move later latch at the next fire so modulation can never starve a trigger.
- Chord length (`pad.chord_bars`) stores bars for the pad/bass engines but displays and accepts numeric entry in beats; typed values convert to bars and snap to the existing power-of-two grid.
- Live-read gain controls are ramped by registry-derived `GainSmoothers` in `FluidEngine`; every unique `ControlKind::Gain` spec must get a smoother automatically.
- TUI automation edits must go through `PublishedAutomation` so the shared audio-thread snapshot is stored on every mutation.
- Pitched voices (Pad, Bass, Tonal) route note numbers through `midi_to_hz` and respect master tune; unpitched voices (Perc, Kick, Clap) do not.
- Tonal separates trigger density from phrase shape: `tonal.rate_beats` controls note trigger spacing; `tonal.step_interval_beats` is the stable cycle-length ID for phrase wrapping and evolution boundaries.
- Tonal synth selection lives in `tonal.synth_type`; it changes the voice created for new tonal notes while preserving the shared phrase/randomness/timing/master-tune/reverb path. Exploration variants may differ in harmonic tables, spectral tilt, and pitch-scaled harmonic decay; keep piano-family profiles warm and non-metallic unless the user explicitly asks for FM-like brightness.
- Attack and release timing for every tonal synth type (Sine and all piano-family profiles) come from `tonal.attack`/`tonal.release` (seconds) via the shared `tonal_envelope_gain` helper, not per-profile fields; only the release curve's shape exponent (`PianoProfile::body_power`, or the sine voice's fixed sqrt taper) stays profile-owned character. A control value longer than a note's own duration clamps to the note length instead of overshooting.
- Tonal owns a slight fixed low cut before engine mixing so its low notes sit above sub/bass energy without requiring a user-facing control.
- Pad, Tonal, and Arp emit dry voice output; `FluidEngine` owns their shared ambient reverb send/return so reverb mix changes do not add an uncontrolled per-voice wet gain boost. Arp has no user-facing `arp.reverb_mix` control; it rides the send at a fixed effective mix (`AMBIENT_REVERB_ARP_MIX_FIXED`/`_SEND` in `engine.rs`) instead.
- Arp follows the Pad's current chord without reaching into `PadEngine` directly: it keeps its own `chord_trigger`/`step_index` synced to the same `pad.chord_bars` grid (same pattern as Bass), reads chord tones via `pad_chord_midi`, and cycles them (Up/Down/Up-Down/Random, 1–3 octave span) on its own `arp.rate_beats` grid. It always uses one fixed warm piano profile (`ARP_PROFILE_INDEX`, reusing Tonal's `PianoTonalVoice`/`tonal_envelope_gain` path) — no per-voice synth-type control. `arp.gain` defaults to 0 (silent) so adding the voice never changes an existing song or default startup. When the chord or octave span changes mid-cycle, the cycle position is clamped into the new tone list rather than reset, avoiding a click.
- A ninth `pad.progression` value (`voice::CUSTOM_PROGRESSION_INDEX`) selects a user-built progression instead of the 8 built-in tables: 8 chord slots (`PadControls::chord_slots`, `ControlSpec` rows `pad.chordN_degree/_accidental/_extension/_inversion`, visible directly on the Chords tab) each define a tonic-relative root degree, semitone accidental, extension (triad/6th/7th/9th-flavor top voice), and inversion; `pad.chord_count` (1–8) sets how many slots loop. `voice::pad_chord_tones`/`voice::pad_chord_count` are the single chord-source path Pad, Bass, and Arp all resolve "what chord is at this step" through — a custom progression drives all three identically without any voice reaching into another's state. Built-in progressions are untouched; the custom path is fully inert unless progression selects it.
- Bass character lives in `bass.type` (`BassControls::voice_type`): index 0 (`Sub`) is the legacy voice and the default — its DSP path must stay byte-identical to a pre-`bass.type` render. Index 1 (`Saw`) is a brighter additive-harmonic character, index 2 (`Pluck`) a shorter character with an attack transient; all three share the same trigger/rhythm, pitch (`midi_to_hz` + master tune), and drive/panner tail, and are gain-authored to a comparable perceived level.
- Bass is monophonic (`BassEngine::voice: Option<BassVoice>` in `bass.rs`): each rhythm-grid hit hard-cuts whatever is currently sounding and starts the new note immediately, regardless of `decay_time`. The replaced voice isn't dropped instantly (that clicks) — it rings down over a fixed short (`BASS_MONO_FADE_SECONDS`, ~3ms) `fading_voice` slot, independent of the voice's own envelope. This applies identically to all three `bass.type` characters. No user-facing control; this is not a voice pool.
- `bass.cutoff` (`BassLowPass` in `bass.rs`) is a one-pole lowpass applied above the `bass.type` dispatch, to `BassEngine`'s summed stereo output (including the mono fade tail), so it affects all three bass characters identically. Its coefficient is recomputed every sample from the live modulated value (it can carry an LFO/envelope route), unlike `TonalLowCut`'s fixed cached coefficient. `BASS_CUTOFF_MAX_HZ` (the default) is a true bypass — `BassEngine::next` skips the filter call rather than relying on a wide-open coefficient, since a one-pole pass at any finite max cutoff still audibly attenuates.
- Pad character lives in `pad.type` (`PadControls::voice_type`, `PadTone` enum over `WarmPadTone`/`DarkPadTone`/`GlassPadTone`): index 0 (`Warm`) is the legacy tone and the default — its DSP path must stay byte-identical to a pre-`pad.type` render. Index 1 (`Dark`) adds a fixed one-pole lowpass before soft-clipping; index 2 (`Glass`) adds a quiet fixed shimmer oscillator two octaves up; all three share the unchanged chord/progression logic (`pad_chord`), trigger timing, attack/release, and pans, are gain-authored to a comparable perceived level, and keep the pad's dry-output/engine-owned-reverb-send contract unchanged.
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
