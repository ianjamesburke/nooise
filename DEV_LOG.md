# Dev Log

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
