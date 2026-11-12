# docs/adr

## Purpose

Architecture decisions and their implementation contracts. An ADR marked
Proposed is a design under review and does not bind implementation; only an
Accepted ADR is a contract.

## Ownership

- `0001-unidirectional-interaction-architecture.md` owns the interaction kernel,
  terminal runtime, live-session transaction, rendering, and migration
  contracts.
- `0002-v3-chord-model.md` (Accepted; Stage 1 implemented) owns the separation
  of chord harmony, voicing, and rhythm, the gate-aware common-tone rule, and
  the chord payload and migration contracts.
- `0003-master-output-stage.md` (Accepted) owns the fixed output makeup, the
  -1 dBFS lookahead limiter that replaced the 0.95 clamp, and their loudness
  targets and verification.

## Local Contracts

- Number ADRs sequentially and record status, date, context, decision,
  alternatives, consequences, and verification scenarios.
- Amend or supersede an ADR when its accepted contract changes; do not leave
  contradictory decisions active.
- Mark an unaccepted design Proposed; do not describe Proposed decisions as
  shipped contracts in parent docs or source.

## Work Guidance

## Verification

- Check referenced source paths, related task contracts, and parent DOX indexes.

## Child DOX Index
