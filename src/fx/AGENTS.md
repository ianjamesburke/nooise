# src/fx

## Purpose

Shared DSP effects consumed by voice engines in `fluid.rs`.

## Ownership

- `lfo.rs` — drifting/periodic LFO generators.
- `panner.rs` — stereo pan helper.
- `compression.rs` — shared stateful stereo compressor used by every layer and Master slot chain.
- `delay.rs` — stereo feedback delay whose read taps never glide: a delay-time change crossfades to a new fixed tap position over a fixed window instead of sliding the old one (so tempo-synced retargeting never pitch-bends), plus wet-only level-compensated Vintage colour and end-weighted pitch motion, true dry bypass, and exact runtime capture/restore (including any in-flight crossfade).
- `drive.rs` — shared stateless stereo saturation with exact zero-amount bypass.
- `reverb.rs` — shared Freeverb-style slot reverb with live Size/Damping updates and exact tail capture/restore.
- `mod.rs` — module re-exports only.

## Local Contracts

- These are stateless-per-call or self-contained stateful DSP units with no dependency on `FluidControls`; the shared module bank owns control interpretation and passes primitives in/out.

## Verification

- `cargo test fx::` covers effect-local behavior; `fluid::engine::module_fx_tests` covers shared slot-chain execution.
