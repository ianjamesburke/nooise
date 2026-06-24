# Dev Log

## 2026-06-24

Final fix applied: replaced the rhythmic layers' independent sample counters with one master-grid trigger driven by `T5Engine.current_sample`. Pad, perc, kick, tonal, and clap now receive the master `now`; kick, tonal, and clap have `offset_beats` controls; clap defaults to a backbeat grid (`interval_beats = 2`, `offset_beats = 1`). The trigger computes absolute grid samples from BPM/interval/offset and re-anchors from the current master sample when those parameters change. It does not compare raw old slot indices and does not accumulate `round(step)` from prior hits.

Diagnosis: bounded tests now record the exact trigger sample numbers for matching kick/clap settings. With BPM 120, interval 2, offset 1, both triggers fire at `[24000, 72000, 120000, ...]`. Another test grows interval/BPM and proves the trigger re-anchors within the new interval instead of waiting for an old slot index to catch up. Voice-onset inspection showed clap already schedules a first burst at local sample 0; the test asserts the first burst is active after the first clap sample, so random slap spread is not delaying the first clap onset.

Verification: `cargo test`, `cargo build`, and `cargo clippy --all-targets --all-features --locked -- -D warnings` pass clean.

Second attempted fix (reverted): replaced the raw slot-index approach with a `BeatScheduler` that stored `last_hit_sample` and `next_sample`, received `T5Engine.current_sample` as the only transport, and re-anchored timing on BPM/interval/offset changes instead of comparing old absolute slot indices. Added `offset_beats` controls for kick, tonal, and clap; set clap's default toward a backbeat. Built clean.

Why it was reverted: listening test still showed kick and clap offset from each other when both were set to the same interval/beat position. That means the bug is not solved by the scheduler shape alone, or the audible onset of the two voices is not equivalent to their trigger sample. Do not re-apply this exact `BeatScheduler` patch as-is.

Next direction: before changing timing again, add a cheap debug/instrumentation path that records or prints the actual trigger sample for kick and clap from the audio thread, and distinguish "scheduler fired at different samples" from "scheduler fired together but voice attack/perceived transient is offset." Also clarify the UI semantics for beat/offset: if interval is 1 beat, an offset of 1 beat is equivalent to 0 on the grid, which may be confusing as a control label.

Tried to fix `t5` beat misalignment. Symptom: each voice (perc, kick, tonal, clap) keeps its own free-running counter and accumulates `next_hit_sample += round(step)`. Every voice anchors at sample 0, so they all fire on beat 0, and there is no way to place clap on the backbeat. Worse, changing a voice's interval mid-play, or changing global BPM, lets each counter continue from its own phase with its own rounding remainder, so alignment between kick/clap/perc/tonal is luck and a tempo nudge slides them apart.

Attempted fix (reverted): one master clock. `T5Engine.current_sample` became the only timebase, passed to each voice as `now`. Each voice triggered off `grid_slot(now, samples_per_beat, interval_beats, offset_beats)` computed in f64 from the absolute sample position, tracking a single `last_slot` and firing when the computed slot exceeded it. Added a per-voice `Offset` (beats) control to kick/tonal/clap so clap could sit on 2 and 4. Built clean.

Why it was reverted: testing showed moving an interval slider **left** (shorter interval) updated timing correctly, but moving **right** (longer interval) silenced the layer for ~20s before it came back. Root cause is the `last_slot` comparison across a grid change. `slot = floor((now - offset) / step)`. Widening `step` shrinks the slot index computed for the same `now`, so the new slot lands *below* the `last_slot` recorded under the previous narrow grid. The voice then stays silent until the absolute clock advances far enough that the coarse-grid slot finally exceeds the old `last_slot` — which at the new larger step can be many seconds out. Shortening the interval has the opposite, harmless effect (slot jumps up, fires immediately). The same stall showed up when moving **BPM**: lowering BPM grows `samples_per_beat`, which grows `step` and shrinks the slot for the current `now`, dropping it below `last_slot` and silencing the layer for a few bars; raising BPM fires immediately. Same mechanism, same parameter-rescale bug.

Lesson / direction for the real fix: an absolute-slot-index comparison is not stable across interval or BPM changes because the slot numbering itself rescales. The master-transport idea is still correct, but the trigger must re-anchor on parameter change instead of comparing raw slot indices from different grids. Options to try next: store the absolute sample of the last hit and schedule `next = last_hit + step` (re-evaluated each block so BPM changes take effect from the next hit, not retroactively), or keep a normalized phase in [0,1) that is reinterpreted under the new step, or re-map `last_slot` through the new grid whenever interval/offset/BPM changes. Any of these keeps voices grid-locked while letting interval changes take effect at the *next* beat rather than stalling.

## 2026-06-21

We tried a first Textual UI pass around `r4`: Rust emitted live JSON telemetry, the UI rendered a breathing ASCII field, and the engine accepted simple layer controls over stdin.

That pass was reverted. It moved too much at once and did not improve the sound enough to keep.

What worked:

- The organic motion idea is worth keeping. A slow field that breathes with the LFO feels closer to the product than a standard meter.
- Live engine telemetry is probably the right boundary for a UI. The UI should observe real phase, pan, beat, and layer state instead of inventing its own animation.
- `r4` still feels like the right combined experiment to build around: tonal bed, bilateral pulse, and noise texture.

What did not work:

- The kick was not audible enough, even after adding gain, transient, and tonal ducking.
- The bilateral pulse is too quiet in the combined stack.
- The ASCII line/field reads as a prototype, not as a finished visual identity.
- Controls arrived before the system had a clear sound target. That made the UI feel busy without making the engine better.
- Splitting the UI package was technically clean, but premature while the sound model is still moving.

Current direction:

- Stay focused on the sound first.
- Get the bilateral pulse clearly audible inside `r4` without making it annoying.
- Build a better kick in isolation before mixing it into the full stack.
- Keep the UI idea, but treat it as a later instrument surface, not the next source of product truth.
- When the UI returns, use richer terminal visuals than plain ASCII lines: layered motion, density, stereo position, and slow LFO shape should be visible without looking like a debug graph.

Next useful work:

1. Make a focused kick experiment.
2. Make a focused bilateral mix experiment.
3. Re-listen to `r4` after those two pieces work alone.
4. Only then bring back Textual controls.
