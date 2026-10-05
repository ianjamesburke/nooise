# Stabilization integration gate — October 5, 2026

Status: automated integration **pass** through code commit `dad9f26` in `worktrees/temp-stabilization-sprint`; Ian's musical and live terminal audition is **pending**. Root `main` remains the October 3 reference at `f28f95d`. This is a listening handoff, not release approval.

## Recorded checks

| Check | Result |
| --- | --- |
| `RUSTC_WRAPPER= cargo build --locked`; `RUSTC_WRAPPER= cargo test --locked --quiet` | Pass; 856 passed, 6 ignored, 0 failed. |
| `cargo fmt --check`; `RUSTC_WRAPPER= cargo clippy --all-targets --locked -- -D warnings`; `git diff --check f28f95d..HEAD` | Pass; standard all-target Clippy has zero warnings. |
| Production keyboard replay | Pass: binding matrix, minimum 46x11 recipe Amount focus/arrows, Shift+H neutral reset, and `r` randomization. This is a deterministic headless terminal path, not physical typing or a live screen inspection. |
| Tonal, recipe, Kick, LFO | Pass: home-scale note pool; sample-identical zero-Amount recipe insertion; global Swing leaves Kick straight and moves other voices; writer glides matched, entering, leaving, and bypassed LFO Amount routes. |
| OSC | Pass: captured UDP ordering for chord, phrase reset, and phrase position; headless engine reaches a UDP listener. No external visualizer was available for display acceptance. |
| Built-in 9→13→9 | Pass: at beats 256.000017 and 512.000235, each landing adopted chord bank, first chord, BPM, and one expected Pad MIDI hit on the same sample. First 0.1-beat audio peaks were 0.181717 and 0.290349, with no duplicate hit. These are trace assertions, not a live transition listen. |
| Seeded audio | Pass: two 2-second, seed-42 default renders are byte-identical and match the fixed October 3 binary (`SHA-256 a8995df3…b83c9919830`; 44.1 kHz, 88,200 frames). Four 12-second Tonal renders completed and were checked with `ffprobe`. |

The default render is a stability comparison, not proof that every authored song stayed identical. A timing-only Kick Drunken probe at 120 BPM measured full contribution at 24.69/50.00 ms mean/maximum lag and candidate half contribution at 12.34/25.00 ms. The half coefficient was **not adopted**; no authored A/B groove WAV was produced, so its feel remains an explicit listening decision. The optional extra `-D clippy::redundant_clone` probe still fails at the same ten source sites on root `main` and this branch; two new sprint-test clones were removed. Its log is `target/stabilization-gate/extra-clippy.log`. The required zero-warning all-target Clippy gate passes.

Audio files, untracked under `target/stabilization-gate/`: `default-a.wav`, `default-b.wav`, `oct3-reference.wav`, and `tonal-{ache,float,shadow,sunny}.wav`. The fixed October 3 build and its earlier chord before/after files remain at `worktrees/feature/ergonomics-redesign/target/oct3-sprint/` and `target/oct3-chords/` in that worktree.

## Ian's ordered audition

Start the stabilized instrument at the exact worktree path:

```sh
cd /Users/ianburke/Documents/GitHub/nooise/worktrees/temp-stabilization-sprint
RUSTC_WRAPPER= cargo run --
```

1. **Keyboard and October 3 baseline:** browse with arrows and Tab; try Space menu/mute, scoped `/` search, Ache/Float chords, and four/eight-bar Motion including save/reopen. Use the fixed October 3 binary or its Ache/Float before/after WAVs for comparison. Report any control or chord that feels worse.
2. **Tonal:** listen to `target/stabilization-gate/tonal-{ache,float,shadow,sunny}.wav`, then play Tonal against changing Pad progressions. Check whether phrases remain melodic and any note clashes with the sounding chord; a concrete song code and beat/pitch would make a failure reproducible.
3. **Recipes and focus:** on a continuous control, add `/sway`, `/sc`, `/tremolo`, and `/drift` one at a time. Each should open at `Amount 0%` without changing the sound; arrows should bring it in immediately. Save/reopen a silent lane. Drift should cycle every four beats.
4. **Groove:** raise Master Swing while Kick plays quarter and half notes; Kick anchors should stay straight while eligible offbeats move. Try local Kick Swing. Raise Master Drunken and judge whether its current full Kick influence feels too loose; the 50% suggestion remains a proposal.
5. **Morph:** use an LFO on a sounding control, then run `RUSTC_WRAPPER= cargo run -- 9,13 --bars 4` for a short two-song loop (the exact automated trace used the default 64 bars). Hear both directions, watch the Amount marker, chord header and BPM, and listen for a stale first chord, late hit or jump. This also supplies the A→B→A listening check for stint 0024.
6. **Older feel checks:** sweep envelope Attack/Decay through the low end, then Bass/Kick Drive and the folded Swing/Room slots. Record whether the taper and folded effects feel right for stint 0044.
7. **OSC, if a consumer is available:** launch with `--osc` (UDP `127.0.0.1:9000`) or `--osc=HOST:PORT`. Check that `/nooise/phrase/reset` precedes `/nooise/phrase` at start, restart, progression switch and morph landing, and that chord visuals follow the sounding Pad. Report the consumer and any observed missed or stale event.

Gate 0075 stays open until Ian records his combined musical/UI judgment. Stint 0024 still needs that live A→B→A observation; 0044 still needs taper and folded-effect listening. Stint 0050 retains its broader morph design beyond this bounded 9→13 regression. Stint 0073 remains the separate versioned release and clean install gate after approval.

## Worktree boundary

The cleanup audit removed only `other-kids-edition`, `temp-hosking-dbmaj7`, and `temp-knob-declick` through `wtp remove`: each was clean, had zero commits unique from main, and had no ignored artifacts outside `target/`. Their branch references remain. Thirty worktrees, including root, remained afterward. Other branches and audition builds are preserved, including the APC40 family, `feat/ask-mode`, active `temp-voice-input`, and `feat/progression-mode` with open PR #35. The latter is design evidence for a later harmony project, outside this sprint.
