# src/synth

## Purpose

Shared synthesis primitives consumed by voice engines in `fluid.rs`.

## Ownership

- `envelope.rs` — ADSR/attack-release envelope generators.
- `oscillator.rs` — waveform generators (sine/saw/etc.) used by pitched voices.
- `noise.rs` — white-noise generator used by unpitched/textural voices.
- `mod.rs` — module re-exports only.

## Local Contracts

- Primitives take explicit parameters (sample rate, frequency, etc.) rather than reading control structs directly — voices in `fluid.rs` own the control state and pass values in.

## Verification

- Covered indirectly via `fluid.rs` engine tests; no dedicated per-file test suite here today.
