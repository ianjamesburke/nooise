# src/synth

## Purpose

Shared synthesis primitives consumed by voice engines in `fluid.rs`.

## Ownership

- `envelope.rs` — ADSR/attack-release envelope generators.
- `oscillator.rs` — waveform generators (sine/saw/etc.) used by pitched voices.
- `noise.rs` — white-noise generator used by unpitched/textural voices.
- `fm.rs` — multi-operator FM: `FmWave`, `FmPair`, `FmStack`, and the test-only `rms` level measure.
- `mod.rs` — module re-exports only.

## Local Contracts

- Primitives take explicit parameters (sample rate, frequency, etc.) rather than reading control structs directly — voices in `fluid.rs` own the control state and pass values in.
- `fm.rs` is the single home for FM timbre construction; no voice hand-rolls its own operator math. An `FmPair` is one modulator→carrier voice (both ratios relative to the base frequency, index in units of the carrier frequency, optional per-sample index decay, carrier waveform, detune, level), and `FmStack` sums up to `MAX_FM_PAIRS` of them in parallel. One pair is a percussive body; several pairs with carrier ratios of roughly 4–8 against a modulator ratio of 1 place formant peaks, which is how vowel/choir timbres are approximated. Deliberately not a general operator graph — FM patching is not exposed in the app.
- `FmStack::next` takes a base frequency every sample and owns nothing else: pitch envelopes, amplitude envelopes, filters, panning, and output trims stay with the calling voice.
- `FmPair::next_carrier_phase`'s operation order is load-bearing. `kick_type_zero_matches_legacy_sub_voice_exactly` and `GOLDEN_RENDER_CHECKSUM` pin kick type 0 byte-for-byte, and reordering these steps changes float rounding even when behaviour-neutral.
- The stack carries no output trim. Loudness matching belongs to the consumer, which is the only layer that knows where in its own chain the trim must sit — but a trim chosen by ear is not acceptable: pin it with a test that measures rendered level (`kick_types_render_at_a_matched_level` is the reference example).

## Verification

- `cargo test synth::fm` covers the FM primitive directly; the remaining files are covered indirectly via `fluid.rs` engine tests.
