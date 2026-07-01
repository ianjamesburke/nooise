# src/fx

## Purpose

Shared DSP effects consumed by voice engines in `fluid.rs`.

## Ownership

- `lfo.rs` — drifting/periodic LFO generators.
- `panner.rs` — stereo pan helper.
- `reverb.rs` — Freeverb-style reverb used by Pad (and any other voice wanting space).
- `mod.rs` — module re-exports only.

## Local Contracts

- These are stateless-per-call or self-contained stateful DSP units with no dependency on `FluidControls`; voices own the control values and pass primitives in/out.

## Verification

- Covered indirectly via `fluid.rs` engine tests; no dedicated per-file test suite here today.
