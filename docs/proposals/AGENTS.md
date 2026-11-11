# Proposals

## Purpose

Feature designs that can be discussed before changing runtime behavior.

## Ownership

- Dated proposal files own each feature's scope, interaction, tradeoffs, and acceptance criteria.
- `2026-11-02-show-readiness.md` owns the November 22 V3 show checkpoints, rehearsal/post-mortem loop, current performance batch, deferred melodic spike, November 6 Pad rhythm and melodic consolidation recommendations, and proposed named-song library/transition previews. `../../STATE.md` records the current candidate's verification and remaining acceptance.
- `2026-10-04-stabilization-intake.md` owns the current stabilization intake, reported musical bugs, near-term hardware priorities, and song-cue, harmony, groove, and recipe decisions.
- `2026-11-11-v4-push-the-harmony.md` owns v4 Push the Harmony and the v4 key map (Space held, `'` go, `"` keep, `g` grab, retired planned mute): keys, harmony model, voicing seam, persistence, what the build leaves out (including the stubbed `G` harmony grab), and what only listening can confirm.
- `2026-10-04-sprint-audit.md` records baseline worktree and stint evidence. `2026-10-05-stabilization-sprint.md` owns the nine-step order and focused reproduction/acceptance cases.
- `2026-10-05-stabilization-gate.md` records the integrated automated results, listening artifacts, and remaining hands-on acceptance for that sprint.
- `2026-09-16-live-gestures.md` records the implemented worktree design and hands-on acceptance criteria for temporary effects in normal browsing and replacing Deck.
- `2026-09-30-ergonomics-redesign.md` records the accepted operation catalog, Space map, planned mute (the Space map and planned mute are superseded by the v4 key map), and Motion Grab worktree behavior (relative numeric loops and absolute choices through eight bars), with Motion Record and shared lane editing deferred.
- `2026-10-03-shared-chord-builder.md` records the shared preset/editor contract, voice-count policy, persistence boundaries and two revoice audition cases.
- `2026-10-04-ai-lead.md` — agreed direction for a personal AI Lead, its MIDI
  corpus, model path, runtime limits, and listening evaluation
- `2026-10-04-live-audio-textures.md` owns the agreed direction and proposed milestones for audio-input tracks, resonant objects, granular effects, and bowed textures; runtime design remains open.
- Product rules remain in `../NORTH_STAR.md`; accepted architecture belongs in `../adr/`; shipped performance behavior belongs in `../PERFORMANCE.md`.

## Local Contracts

- Label new proposals with their decision status. Distinguish agreed direction, proposed defaults, and unresolved decisions.
- A draft does not change shipped behavior or supersede an accepted ADR.
- Reuse the shared module path for equivalent effects. Specify audible verification alongside input and state checks.

## Work Guidance

- Ground bindings in the current input map and evaluate features against the North Star.
- Keep design choices and acceptance criteria concrete enough to test by playing.

## Verification

- Check relative links and the parent Child DOX Index after editing.

## Child DOX Index
