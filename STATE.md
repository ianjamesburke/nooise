# nooise Stabilization State

Updated: 2026-10-05

## Current candidate

- Worktree: `worktrees/temp-stabilization-sprint`
- Integrated code: `2956aca` (`fix: keep delay filter on cutoff`)
- Latest report commit: `ba9848b`
- Root `main`: `f28f95d`, clean and unchanged
- Release status: not approved; no release PR, tag, or publication

## Progress

The stabilization implementation is integrated in this worktree. It includes:

- Tonal phrases follow the selected progression home scale.
- Kick stays straight under Master Swing; OSC exposes Pad phrase timing.
- Centered controls reset to neutral; recipes start silently and focus Amount; Sidechain stays below its authored base.
- Randomize avoids returning the current value; morph structural landing and LFO Amount continuity have focused coverage.
- Perc has a saved Attack control.
- Collapsed Filter rows browse Cutoff.
- A layer's Drunken module overrides Master Drunken, including at local Amount 0%.

## Automated evidence

- `RUSTC_WRAPPER= cargo test --locked --quiet`: 876 passed, 0 failed, 6 ignored.
- `cargo fmt --check` and `git diff --check`: pass.
- Locked build and strict all-target Clippy pass on the same code in the implementation worktree; the integrated candidate has no additional code beyond that verified commit.
- Two seed-42, 2-second renders are byte-identical and match the October 3 baseline SHA-256 `a8995df35255429d324c623f6cc4f69eb98fdd913a7961ccbfeb5b83c9919830`.
- Production keyboard replay, song-code, and OSC behavior are covered by the full suite. There has been no new physical keyboard, live terminal, speaker/headphone, or external OSC-consumer audition.

Detailed procedures and measured evidence: [stabilization gate](docs/proposals/2026-10-05-stabilization-gate.md).

## Stint coverage audit

The local tracker lives in the root checkout's ignored `.stint/`; it is not present in this worktree. Current coverage is incomplete and some statuses are stale.

| Stint | Current tracker state | Relationship to this candidate |
| --- | --- | --- |
| 0075 | Ready/open | Combined hands-on audition gate. It is the direct acceptance checkpoint for this candidate. |
| 0073 | Blocked by 0075 | Versioned release follow-on; still requires the audition and release decision. |
| 0024 | Active | Auto-morph still needs full-loop musical audition. |
| 0044 | Backlog | Envelope taper and folded-effect listening remain open. |
| 0052 | Done | Existing recipe work; this candidate adjusts recipe insertion behavior. |
| 0029 | Backlog | Neutral reset fix is present in the candidate, but tracker status was not reconciled. |
| 0057 | Backlog | No-op randomize fix is present in the candidate, but tracker status was not reconciled. |
| 0050 | Backlog | Broad morph redesign remains open; this candidate only covers bounded structural-landing and Amount-continuity checks. |
| 0064 | Done | Harmony/track model decision; it does not fully track the new Tonal scale implementation. |

No dedicated stint was found for the stabilization-specific Tonal scale fix, Kick anchoring under Master Swing, OSC phrase feed, Perc Attack, collapsed Filter Cutoff browsing, layer Drunken override, or Delay child Filter. These changes are in the integration worktree but are not individually represented in `.stint/`. The older `s1` sprint is the October 3 keyboard/harmony sprint, not this October 5 stabilization batch.

## Remaining

1. Ian auditions the candidate using the ordered checks in the linked gate report, including the corrected Delay Filter interaction.
2. Record the musical and live-terminal results against 0075; make any requested fixes in this worktree.
3. Reconcile the local stint ledger before closing this stabilization: update 0029/0057 evidence, distinguish the bounded 0050 checks from the broader open design, and decide whether to create dedicated records for untracked slices.
4. Only after acceptance, proceed through 0073's versioned release gate.
