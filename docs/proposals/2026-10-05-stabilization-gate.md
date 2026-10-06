# Stabilization integration gate — October 5, 2026

Status: the worktree includes the later slices through `56a67b9`, and its combined automated gate passes. Ian's musical and live terminal audition is **pending**. Root `main` remains the October 3 reference at `f28f95d`. This is a listening handoff, not release approval.

## Later integrated slices

- `2b25494` adds Perc Attack: a saved 0–1000 ms Attack row before Decay. It starts at 0 ms, preserves the prior onset at that setting, and rises into the existing decay when raised.
- `0e4627d` and formatting follow-up `c3ee76e` make a Filter's collapsed browsing row control Cutoff. A newly added Filter still opens on dry Amount so its first right arrow makes it audible.
- `5d0b0bf` adds layer-level Drunken. A layer's Drunken slot overrides Master Drunken, including when its local Amount is 0%; the slot and shared wave phase save in the song code.
- `56a67b9` adds one optional Filter child inside a Delay detail. It processes only Delay's wet signal, begins fully wet, stays on a selected Filter/Cutoff row after insertion, and opens Amount, Resonance, and Type with Enter. It persists when added and fades its colour out when removed while the Delay tail continues.

## Combined gate progress

| Check | Result |
| --- | --- |
| `RUSTC_WRAPPER= cargo test --locked` | Pass on `56a67b9`: 876 passed, 6 ignored, 0 failed. |
| `cargo fmt --check`; `git diff --check` | Pass on `56a67b9`. |
| `RUSTC_WRAPPER= cargo build --locked`; `RUSTC_WRAPPER= cargo clippy --all-targets --locked -- -D warnings` | Pass on `56a67b9`. |
| Seeded audio | Two combined seed-42 2-second WAVs are byte-identical and match the October 3 baseline: SHA-256 `a8995df35255429d324c623f6cc4f69eb98fdd913a7961ccbfeb5b83c9919830`; 44.1 kHz, 88,200 frames. |
| Production keyboard replay, song-code, and OSC | Covered by the passing full suite. No new standalone replay, song-code, OSC, manual keyboard, terminal, or OSC-consumer run was made for these later slices. |

The full suite is automated evidence. It does not replace the pending musical and live terminal audition.

## Earlier check evidence, before the later slices

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
4. **Groove and layer Drunken:** raise Master Swing while Kick plays quarter and half notes; Kick anchors should stay straight while eligible offbeats move. Try local Kick Swing. Add Master Drunken through `/drunken` and raise its Amount. On Perc, add `/drunken`, leave its local Amount at 0%, and confirm Perc returns to the straight grid while the other layers keep Master Drunken. Raise the local Amount, save and reopen, then remove the local Drunken slot and confirm Perc follows Master Drunken again. Report the song code and the layer if the override, restore, or handoff feels wrong.
5. **Morph:** use an LFO on a sounding control, then run `RUSTC_WRAPPER= cargo run -- 9,13 --bars 4` for a short two-song loop (the exact automated trace used the default 64 bars). Hear both directions, watch the Amount marker, chord header and BPM, and listen for a stale first chord, late hit or jump. This also supplies the A→B→A listening check for stint 0024.
6. **Perc Attack:** Tab to Perc and select Attack, the row between Level and Decay. Start at 0 ms, press Right once, then compare a short, mid, and long setting on repeated Perc hits. Set an interval short enough to overlap a long Attack. Reset Attack to 0 ms. The onset should remain the authored sound at 0 ms; increasing Attack should make the hit rise into its existing decay without killing an earlier rising hit. Report any click, missing hit, or taper that does not feel playable.
7. **Collapsed Filter Cutoff:** use `Space k` on a layer with a factory Filter and adjust Cutoff directly from the collapsed row. On Pads, use `Space k` to add a Filter: it should open at Amount 0%, the first Right should make it audible, and Escape should return to the collapsed Cutoff row. Sweep Cutoff after raising Amount. Confirm the added Filter stays silent until Amount moves and that Cutoff remains the browsing control.
8. **Delay wet-only Filter:** add `/delay` to a sounding layer, open its Delay detail with Enter, select `Add Filter`, and press Enter. It should add the child fully wet, leave Delay detail open, and select its `Filter ›` Cutoff row. Sweep Cutoff there, then press Enter to reach Amount, Resonance, and Type. Listen to repeated echoes: the delayed signal should darken while the dry source stays unchanged. Save and reopen, then remove the child while echoes remain. Its colour should fade away without a click or an abruptly cut Delay tail.
9. **Older feel checks:** sweep envelope Attack/Decay through the low end, then Bass/Kick Drive and the folded Swing/Room slots. Record whether the taper and folded effects feel right for stint 0044.
10. **OSC, if a consumer is available:** launch with `--osc` (UDP `127.0.0.1:9000`) or `--osc=HOST:PORT`. Check that `/nooise/phrase/reset` precedes `/nooise/phrase` at start, restart, progression switch and morph landing, and that chord visuals follow the sounding Pad. Report the consumer and any observed missed or stale event.

Gate 0075 stays open until Ian records the combined automated result and musical/UI judgment. No physical keyboard audition, speaker or headphone listening, live terminal inspection, save/reopen interaction, external hardware check, or OSC-consumer display check is claimed here. Stint 0024 still needs that live A→B→A observation; 0044 still needs taper and folded-effect listening. Stint 0050 retains its broader morph design beyond this bounded 9→13 regression. Stint 0073 remains the separate versioned release and clean install gate after approval.

## Worktree boundary

The cleanup audit removed only `other-kids-edition`, `temp-hosking-dbmaj7`, and `temp-knob-declick` through `wtp remove`: each was clean, had zero commits unique from main, and had no ignored artifacts outside `target/`. Their branch references remain. Thirty worktrees, including root, remained afterward. Other branches and audition builds are preserved, including the APC40 family, `feat/ask-mode`, active `temp-voice-input`, and `feat/progression-mode` with open PR #35. The latter is design evidence for a later harmony project, outside this sprint.
