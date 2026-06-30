# Perc beat control with continuous-noise endpoint

## Problem

`PercEngine` is hard-wired to fire `NoiseHit`s every `0.25` beats (`src/fluid.rs:1721`). The perc `decay_ms` knob fights this fixed rhythm: two prior attempts to make full decay (`decay_ms` near 2000) sound like continuous white noise (crossfade blend, analytical RMS switch) both still audibly pulsed, because overlapping decaying hits produce an amplitude ripple at the trigger rate regardless of attenuation (see `GOTCHAS.md`).

The fix is to give perc its own beat/subdivision control and make one end of that control's range bypass discrete hits entirely, instead of disguising the trigger structure with decay.

## Scope

- Add `interval_beats` and `offset_beats` to `PercControls`, mirroring `KickControls`' existing fields and clamp ranges.
- Replace the hard-coded `0.25, 0.0` passed to `GridTrigger::pop` in `PercEngine::next` with the new controls.
- Define a continuous-mode sentinel at the top of `interval_beats`' range: one step past the last discrete value, fully bypassing `GridTrigger`/`NoiseHit`.
- Wire both new controls into the Perc tab terminal UI (`tab_controls`, `adjust`, `apply_min`).
- Lower `KickControls.interval_beats` minimum from `0.5` to `0.25` (UI min, adjust clamp), matching perc's new floor.
- Verify continuous mode against an audio-level signal (RMS/envelope shape over rendered samples), not just by checking `hits.is_empty()`.
- Trim the two failed-approach entries and the "Reimagined control direction" note from `GOTCHAS.md` once implemented, since this spec documents the working fix.

## Non-scope

- No retry of the crossfade or analytical RMS switch approaches.
- No redesign of kick (beyond the min-clamp change above), clap, pad, tonal, or master-bus controls.
- No app-wide switch to note-name beat divisions (sixteenth/eighth/quarter naming) — stays decimal, matching the rest of the app's existing convention (e.g. `lfo_rate_bars`, `tonal.step_interval_beats`).
- No separate continuous-mode loudness control — continuous noise reuses `level` and `filter` directly.
- No continuous endpoint for kick — kick stays purely discrete/percussive.

## Design

### `PercControls`

```rust
pub(crate) struct PercControls {
    pub level: f32,
    pub decay_ms: f32,
    pub filter: f32,
    pub lfo_rate_bars: f32,
    pub lfo_depth: f32,
    pub interval_beats: f32,  // new
    pub offset_beats: f32,    // new
}
```

Defaults: `interval_beats: 0.25` (preserves today's hard-wired behavior), `offset_beats: 0.0`.

Range/step, set in both the `ControlItem` (`min`/`max`) and the `adjust`/`apply_min` clamp logic:
- `interval_beats`: `0.25..=4.25` step `0.25`. The values `0.25` through `4.0` are discrete beat intervals (matching kick's range, extended down to `0.25`). `4.25` is one step past the last discrete value and is the continuous-mode sentinel.
- `offset_beats`: `0.0..=4.0` step `0.25`, matching kick exactly.

Display formatting: `interval_beats` shows `"Continuous"` when `>= 4.25` (compare with the same epsilon-free style used elsewhere for f32 control values — these are stepped by exact `0.25` increments so direct comparison is safe, consistent with `lfo_rate_bars`/`interval_beats` elsewhere in the file), otherwise `"{:.2} beats"` (or matches the existing display style used for kick's interval/offset).

### `KickControls`

`interval_beats` min lowered from `0.5` to `0.25` in:
- `tab_controls` `ControlItem.min` for the Kick tab's interval row.
- The `adjust` clamp at `c.kick.interval_beats = (... ).clamp(0.5, 4.0)` → `.clamp(0.25, 4.0)`.

No other kick behavior changes.

### `PercEngine::next`

```rust
fn next(&mut self, c: &PercControls, timing: TimingContext) -> f32 {
    let rate_hz = timing.lfo_hz_for_bars(c.lfo_rate_bars);
    let lfo_raw = self.vol_lfo.next(&mut self.rng, rate_hz * 0.5, rate_hz * 2.0);
    let lfo_norm = normalized_lfo(lfo_raw);
    let effective_level = c.level * ((1.0 - c.lfo_depth) + lfo_norm * c.lfo_depth);

    if c.interval_beats >= 4.25 {
        // Continuous mode: no triggers, no NoiseHits, steady filtered stream.
        return self.noise.next_filtered(&mut self.rng, c.filter) * effective_level * 0.4;
    }

    if self.trigger.pop(timing, c.interval_beats, c.offset_beats) {
        let smoothing = 10_f32.powf(c.filter * 4.0 - 4.0);
        self.hits.push(NoiseHit::new(effective_level, c.decay_ms, smoothing, self.sample_rate));
    }

    let mut out = 0.0f32;
    for h in &mut self.hits {
        out += h.next(&mut self.rng);
    }
    self.hits.retain(|h| !h.is_done());
    out
}
```

`PercEngine` gains a `noise: WhiteNoise` field for the continuous branch (separate from the per-hit `WhiteNoise` instances inside each `NoiseHit`), constructed in `PercEngine::new` alongside the existing fields.

Note: continuous mode applies `filter` directly via `next_filtered(filter)` (raw `0.5–1.0` filter value), not the `10^(filter*4-4)` smoothing transform used for discrete hits — that transform exists to convert the UI's filter knob into a per-hit envelope-following smoothing coefficient sized for short decays, which doesn't apply to a steady stream. This is a deliberate divergence from "same filter knob" in the literal sense; the perceptual intent (more filtering = darker noise at higher knob values) is preserved by passing the same `c.filter` value into `next_filtered`, just without the per-hit-decay-specific transform.

`decay_ms` is read by nothing in the continuous branch — the Decay control stays in its existing position/index in the Perc tab UI and remains adjustable, it simply has no audible effect while `interval_beats >= 4.25`.

### Terminal UI (`tab_controls`, `adjust`, `apply_min`)

Two new `ControlItem`s appended to the end of the `Tab::Perc` vec (after LFO Depth, index `5` and `6`):
- `"Interval"` → `c.perc.interval_beats`, min `0.25`, max `4.25`.
- `"Offset"` → `c.perc.offset_beats`, min `0.0`, max `4.0`.

Corresponding `adjust` match arms (indices `5`/`6`):
```rust
5 => c.perc.interval_beats = (c.perc.interval_beats + dir * 0.25).clamp(0.25, 4.25),
6 => c.perc.offset_beats = (c.perc.offset_beats + dir * 0.25).clamp(0.0, 4.0),
```

`apply_min` gets matching arms if the Perc tab's existing reset block covers all indices (check current `apply_min` Perc arms and extend the same way kick's are extended).

### Testing

- Unit test: `PercControls::default()` still yields `interval_beats == 0.25`, `offset_beats == 0.0` (extend `defaults_match_current_mix`).
- Unit test: at `interval_beats = 4.25`, render `PercEngine::next` for a few seconds of samples and assert no `NoiseHit`s are ever pushed (`hits` stays empty) — this is necessary but per the gotcha, not sufficient on its own.
- Audio-level test: render `PercEngine::next` output at `interval_beats = 4.25` for ~1s, compute RMS over short (~10ms) windows, and assert the windowed RMS stays within a tight band (no periodic dips matching any plausible former trigger rate) — directly verifying the "still beating" failure mode is gone, at the signal level rather than the internal-state level.
- Unit test: kick's `interval_beats` can be adjusted down to `0.25` and clamps there (extend existing kick adjust-clamp test pattern).

## Out of scope follow-ups

None identified — this closes the open task in `.stint/tasks/0001-add-perc-beat-control.md`.
