# src/fx

## Purpose

Shared DSP effects consumed by voice engines in `fluid.rs`.

## Ownership

- `lfo.rs` — drifting/periodic LFO generators.
- `panner.rs` — stereo pan helper.
- `compression.rs` — shared stateful stereo compressor used by every layer and Master slot chain.
- `delay.rs` — stereo feedback delay whose read taps never glide: a delay-time change crossfades to a new fixed tap position over a fixed window instead of sliding the old one (so tempo-synced retargeting never pitch-bends), plus wet-only level-compensated Vintage colour and end-weighted pitch motion, and true dry bypass.
- `drive.rs` — shared stateless stereo saturation with exact zero-amount bypass.
- `reverb.rs` — shared Freeverb-style slot reverb with live Size/Damping updates.
- `mod.rs` — module re-exports only.

## Local Contracts

- These are stateless-per-call or self-contained stateful DSP units with no dependency on `FluidControls`; the shared module bank owns control interpretation and passes primitives in/out.
- A stateful unit takes at construction only what genuinely fixes its state (sample rate, buffer capacity) and everything else per call, through one `…Params` struct (`DelayParams`, `CompressorParams`, `ReverbParams`). Never a setter paired with a duplicate constructor argument: the caller then has two ways to say the same thing and the constructor's copy is dead before the first sample. The struct also keeps `process` under clippy's argument-count lint without an `#[allow]`.
- Range clamps here express DSP stability invariants only (delay feedback below 1.0, compressor ratio at or above 1.0), never a control's editable bounds. Those live in `fluid/registry.rs`'s `ControlSpec`, which every write path already clamps against; restating one downstream can only mask a spec change, so pin it with a test instead.

## Verification

- `cargo test fx::` covers effect-local behavior; `fluid::engine::module_fx_tests` covers shared slot-chain execution.
