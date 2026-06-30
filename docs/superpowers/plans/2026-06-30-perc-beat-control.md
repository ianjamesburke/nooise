# Perc Beat Control with Continuous-Noise Endpoint Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `PercEngine`'s hard-coded `0.25`-beat trigger with user-controllable `interval_beats`/`offset_beats`, and add a continuous-noise mode at the top of `interval_beats`' range that bypasses discrete hits entirely.

**Architecture:** All changes live in `src/fluid.rs`. `PercControls` gains two new `f32` fields. `PercEngine` gains a `noise: WhiteNoise` field for the continuous branch and a conditional at the top of `next()` that either streams filtered white noise directly (continuous mode) or falls through to the existing `GridTrigger`/`NoiseHit` path (discrete mode), now parameterized by the new controls instead of literals. Terminal UI wiring (`tab_controls`, `apply_delta`, `apply_min`) gets two new `Tab::Perc` rows appended after `LFO Depth` (indices 5/6), and `KickControls.interval_beats`'s floor drops from `0.5` to `0.25` in three places.

**Tech Stack:** Rust, no new dependencies. `cargo test` for verification.

## Global Constraints

- `interval_beats` (perc): range `0.25..=4.25`, step `0.25`. Default `0.25`. `>= 4.25` is the continuous-mode sentinel.
- `offset_beats` (perc): range `0.0..=4.0`, step `0.25`. Default `0.0`.
- `KickControls.interval_beats` floor lowered from `0.5` to `0.25` (max stays `4.0`).
- No retry of crossfade or analytical RMS switch approaches (see `GOTCHAS.md`).
- No app-wide note-name beat divisions — stay decimal, consistent with `lfo_rate_bars`/`tonal.step_interval_beats`.
- Continuous mode reuses `level`/`filter` directly; `decay_ms` has no effect in continuous mode but the Decay slider stays at its current UI index/position.
- Verify continuous mode at the audio/signal level (windowed RMS), not only via internal state (`hits.is_empty()`).
- Trim the two failed-approach entries and "Reimagined control direction" note from `GOTCHAS.md` once implemented.

---

### Task 1: Add `interval_beats`/`offset_beats` to `PercControls`, lower kick's floor

**Files:**
- Modify: `src/fluid.rs:84-102` (`PercControls` struct + `Default` impl)
- Modify: `src/fluid.rs:584-590` (Kick tab `ControlItem` "Interval" `min`)
- Modify: `src/fluid.rs:833` (Kick `apply_delta` clamp, currently `840` area — see exact line below)
- Modify: `src/fluid.rs:843` (Kick `apply_min`, currently sets `0.5`)
- Test: `src/fluid.rs` (inline `mod tests`, extend `defaults_match_current_mix`)

**Interfaces:**
- Produces: `PercControls.interval_beats: f32` (default `0.25`), `PercControls.offset_beats: f32` (default `0.0`) — consumed by Task 2.

- [ ] **Step 1: Write the failing test**

In `mod tests`, extend `defaults_match_current_mix` (around `src/fluid.rs:2239-2260`) by adding these two lines inside the existing perc block (after the `controls.perc.lfo_depth` assertion):

```rust
        assert_close(controls.perc.interval_beats, 0.25);
        assert_close(controls.perc.offset_beats, 0.0);
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test defaults_match_current_mix --lib`
Expected: FAIL with "no field `interval_beats` on type `PercControls`" (compile error, which counts as a failing test here).

- [ ] **Step 3: Add the fields**

In `src/fluid.rs`, change the `PercControls` struct (lines 84-90):

```rust
#[derive(Clone)]
pub(crate) struct PercControls {
    pub level: f32,
    pub decay_ms: f32,
    pub filter: f32,
    pub lfo_rate_bars: f32,
    pub lfo_depth: f32,
    pub interval_beats: f32,
    pub offset_beats: f32,
}
```

And its `Default` impl (lines 92-101):

```rust
impl Default for PercControls {
    fn default() -> Self {
        Self {
            level: 0.0,
            decay_ms: 200.0,
            filter: 0.7,
            lfo_rate_bars: 1.0,
            lfo_depth: 0.1,
            interval_beats: 0.25,
            offset_beats: 0.0,
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test defaults_match_current_mix --lib`
Expected: PASS

- [ ] **Step 5: Lower kick's interval floor in the three places it's clamped/displayed**

In `tab_controls`, `Tab::Kick`'s "Interval" `ControlItem` (around `src/fluid.rs:584-590`):

```rust
            ControlItem {
                label: "Interval",
                value: c.kick.interval_beats,
                min: 0.25,
                max: 4.0,
                display: format!("{:.2} beats", c.kick.interval_beats),
            },
```

In `apply_delta`, `Tab::Kick` match arm 6 (around `src/fluid.rs:758`):

```rust
            6 => c.kick.interval_beats = (c.kick.interval_beats + dir * 0.25).clamp(0.25, 4.0),
```

In `apply_min`, `Tab::Kick` match arm 6 (around `src/fluid.rs:843`):

```rust
            6 => c.kick.interval_beats = 0.25,
```

- [ ] **Step 6: Write the failing test for kick's new floor**

Add a new test near the existing `apply_min_moves_selected_control_to_floor` test:

```rust
    #[test]
    fn kick_interval_floor_is_quarter_beat() {
        let mut controls = FluidControls::default();
        controls.kick.interval_beats = 1.0;
        apply_min(Tab::Kick, 6, &mut controls);
        assert_close(controls.kick.interval_beats, 0.25);

        controls.kick.interval_beats = 0.25;
        apply_delta(Tab::Kick, 6, -1.0, &mut controls);
        assert_close(controls.kick.interval_beats, 0.25);
    }
```

- [ ] **Step 7: Run test to verify it fails, then passes**

Run: `cargo test kick_interval_floor_is_quarter_beat --lib`
Expected before Step 5 edits: FAIL (asserts `0.5` floor behavior under old code). Since Step 5 already landed above, instead run now and expect PASS. If it fails, re-check Step 5's three edits landed correctly.
Expected: PASS

- [ ] **Step 8: Run full test suite**

Run: `cargo test --lib`
Expected: PASS, no regressions

- [ ] **Step 9: Commit**

```bash
git add src/fluid.rs
git commit -m "feat: add perc interval/offset controls, lower kick interval floor to 0.25"
```

---

### Task 2: Continuous-noise mode in `PercEngine::next`

**Files:**
- Modify: `src/fluid.rs:1695-1701` (`PercEngine` struct)
- Modify: `src/fluid.rs:1703-1709` (`PercEngine::new`)
- Modify: `src/fluid.rs:1714-1739` (`PercEngine::next`)
- Test: `src/fluid.rs` (inline `mod tests`, new tests)

**Interfaces:**
- Consumes: `PercControls.interval_beats`, `PercControls.offset_beats` (from Task 1); `GridTrigger::pop(timing: TimingContext, interval_beats: f32, offset_beats: f32) -> bool` (existing, `src/fluid.rs:1426`); `WhiteNoise::next_filtered<R: Rng>(&mut self, rng: &mut R, smoothing: f32) -> f32` (existing, `src/synth/noise.rs:12`).
- Produces: `PercEngine::next(&mut self, c: &PercControls, timing: TimingContext) -> f32` keeps its existing signature; behavior now branches on `c.interval_beats >= 4.25`.

- [ ] **Step 1: Write the failing test for "no hits in continuous mode"**

Add to `mod tests`. This needs a way to construct `PercEngine` and drive it — `PercEngine` and `PercControls` are both in-crate (`pub(crate)`/private), so the test can use `PercEngine::new` and `PercEngine::next` directly since the test module is `mod tests { use super::*; ... }` within `fluid.rs`.

```rust
    #[test]
    fn perc_continuous_mode_pushes_no_hits() {
        let mut controls = PercControls::default();
        controls.level = 1.0;
        controls.interval_beats = 4.25;

        let mut engine = PercEngine::new(SAMPLE_RATE);
        let bpm = 82.0;
        for sample in 0..(SAMPLE_RATE as u64 * 2) {
            let t = timing(sample, bpm);
            engine.next(&controls, t);
        }
        assert!(engine.hits.is_empty());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test perc_continuous_mode_pushes_no_hits --lib`
Expected: FAIL — at `interval_beats = 4.25`, `GridTrigger::pop` still fires on whatever grid `4.25` maps to today (current code ignores `c.interval_beats` entirely and hard-codes `0.25`, so hits accumulate).

- [ ] **Step 3: Write the failing audio-level test (windowed RMS stays flat)**

Add to `mod tests`:

```rust
    #[test]
    fn perc_continuous_mode_has_no_periodic_rms_dips() {
        let mut controls = PercControls::default();
        controls.level = 1.0;
        controls.lfo_depth = 0.0; // isolate trigger-rate ripple from the volume LFO
        controls.interval_beats = 4.25;

        let mut engine = PercEngine::new(SAMPLE_RATE);
        let bpm = 82.0;
        let window_samples = (SAMPLE_RATE * 0.01) as usize; // ~10ms windows
        let total_samples = SAMPLE_RATE as usize * 2; // 2s, well past startup transient
        let mut window_rms = Vec::new();
        let mut window = Vec::with_capacity(window_samples);

        for sample in 0..total_samples as u64 {
            let t = timing(sample, bpm);
            let out = engine.next(&controls, t);
            window.push(out);
            if window.len() == window_samples {
                let sum_sq: f32 = window.iter().map(|x| x * x).sum();
                window_rms.push((sum_sq / window.len() as f32).sqrt());
                window.clear();
            }
        }

        // Skip the first 1/4s of windows to let the one-pole filter settle.
        let settle_windows = (SAMPLE_RATE * 0.25) as usize / window_samples;
        let rms_tail = &window_rms[settle_windows..];

        let min_rms = rms_tail.iter().cloned().fold(f32::INFINITY, f32::min);
        let max_rms = rms_tail.iter().cloned().fold(f32::NEG_INFINITY, f32::max);

        assert!(min_rms > 0.0, "continuous mode produced silence in a window");
        // Tight band: max no more than 2x min. A 0.25-beat-interval trigger
        // ripple would swing RMS toward zero between hits, blowing this past 2x.
        assert!(
            max_rms / min_rms < 2.0,
            "windowed RMS varies too much ({min_rms}..{max_rms}), suggests periodic triggering survived"
        );
    }
```

- [ ] **Step 4: Run test to verify it fails**

Run: `cargo test perc_continuous_mode_has_no_periodic_rms_dips --lib`
Expected: FAIL — current code triggers `NoiseHit`s every `0.25` beats regardless of `interval_beats`, producing periodic RMS dips between decaying hits.

- [ ] **Step 5: Implement continuous mode**

In `src/fluid.rs`, change the `PercEngine` struct (lines 1695-1701):

```rust
struct PercEngine {
    sample_rate: f32,
    trigger: GridTrigger,
    hits: Vec<NoiseHit>,
    noise: WhiteNoise,
    vol_lfo: DriftingLfo,
    rng: StdRng,
}
```

`PercEngine::new` (lines 1703-1712):

```rust
impl PercEngine {
    fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            trigger: GridTrigger::new(),
            hits: Vec::with_capacity(8),
            noise: WhiteNoise::new(),
            vol_lfo: DriftingLfo::new(0.2, sample_rate),
            rng: StdRng::from_entropy(),
        }
    }
```

`PercEngine::next` (lines 1714-1739):

```rust
    fn next(&mut self, c: &PercControls, timing: TimingContext) -> f32 {
        // Advance LFO every sample so phase accumulates at the correct rate.
        let rate_hz = timing.lfo_hz_for_bars(c.lfo_rate_bars);
        let lfo_raw = self
            .vol_lfo
            .next(&mut self.rng, rate_hz * 0.5, rate_hz * 2.0);
        let lfo_norm = normalized_lfo(lfo_raw);
        let effective_level = c.level * ((1.0 - c.lfo_depth) + lfo_norm * c.lfo_depth);

        if c.interval_beats >= 4.25 {
            // Continuous mode: bypass GridTrigger/NoiseHit entirely so there is
            // no trigger-rate amplitude ripple to disguise (see GOTCHAS.md).
            return self.noise.next_filtered(&mut self.rng, c.filter) * effective_level * 0.4;
        }

        if self.trigger.pop(timing, c.interval_beats, c.offset_beats) {
            let smoothing = 10_f32.powf(c.filter * 4.0 - 4.0);
            self.hits.push(NoiseHit::new(
                effective_level,
                c.decay_ms,
                smoothing,
                self.sample_rate,
            ));
        }

        let mut out = 0.0f32;
        for h in &mut self.hits {
            out += h.next(&mut self.rng);
        }
        self.hits.retain(|h| !h.is_done());
        out
    }
```

- [ ] **Step 6: Run both new tests to verify they pass**

Run: `cargo test perc_continuous_mode --lib`
Expected: PASS (both `perc_continuous_mode_pushes_no_hits` and `perc_continuous_mode_has_no_periodic_rms_dips`)

- [ ] **Step 7: Run full test suite**

Run: `cargo test --lib`
Expected: PASS, no regressions

- [ ] **Step 8: Commit**

```bash
git add src/fluid.rs
git commit -m "feat: bypass GridTrigger for continuous perc noise at interval >= 4.25"
```

---

### Task 3: Wire `interval_beats`/`offset_beats` into the Perc tab UI

**Files:**
- Modify: `src/fluid.rs:721-745` (`tab_controls`, `Tab::Perc` arm)
- Modify: `src/fluid.rs:814-820` (`apply_delta`, `Tab::Perc` arm)
- Modify: `src/fluid.rs:815-820` (`apply_min`, `Tab::Perc` arm — see exact block below)
- Test: `src/fluid.rs` (inline `mod tests`, new tests)

**Interfaces:**
- Consumes: `PercControls.interval_beats`/`offset_beats` (Task 1); `c.interval_beats >= 4.25` sentinel semantics (Task 2).
- Produces: nothing new consumed elsewhere — this is the UI leaf.

- [ ] **Step 1: Write the failing test for `tab_controls` row count and labels**

Add to `mod tests`:

```rust
    #[test]
    fn perc_tab_controls_include_interval_and_offset() {
        let controls = FluidControls::default();
        let rows = tab_controls(Tab::Perc, &controls);
        assert_eq!(rows.len(), 7);
        assert_eq!(rows[5].label, "Interval");
        assert_close(rows[5].min, 0.25);
        assert_close(rows[5].max, 4.25);
        assert_eq!(rows[6].label, "Offset");
        assert_close(rows[6].min, 0.0);
        assert_close(rows[6].max, 4.0);
    }

    #[test]
    fn perc_interval_displays_continuous_at_top() {
        let mut controls = FluidControls::default();
        controls.perc.interval_beats = 4.25;
        let rows = tab_controls(Tab::Perc, &controls);
        assert_eq!(rows[5].display, "Continuous");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test perc_tab_controls_include_interval_and_offset perc_interval_displays_continuous_at_top --lib`
Expected: FAIL — `tab_controls(Tab::Perc, ...)` currently returns 5 rows, no `Interval`/`Offset` labels exist.

- [ ] **Step 3: Add the two `ControlItem`s to `tab_controls`' `Tab::Perc` arm**

In `src/fluid.rs`, the `Tab::Perc => vec![...]` arm in `tab_controls` (around lines 721-745) currently ends after the "LFO Depth" `ControlItem`. Append two more items so the arm reads:

```rust
        Tab::Perc => vec![
            ControlItem {
                label: "Level",
                value: c.perc.level,
                min: 0.0,
                max: 1.0,
                display: format!("{:.0}%", c.perc.level * 100.0),
            },
            ControlItem {
                label: "Decay",
                value: c.perc.decay_ms,
                min: 20.0,
                max: 2000.0,
                display: if c.perc.decay_ms >= 1000.0 {
                    format!("{:.1} s", c.perc.decay_ms / 1000.0)
                } else {
                    format!("{:.0} ms", c.perc.decay_ms)
                },
            },
            ControlItem {
                label: "Filter",
                value: c.perc.filter,
                min: 0.5,
                max: 1.0,
                display: format!("{:.0}%", c.perc.filter * 100.0),
            },
            ControlItem {
                label: "LFO Rate",
                value: c.perc.lfo_rate_bars,
                min: 0.25,
                max: 16.0,
                display: format!("{:.0} beats", c.perc.lfo_rate_bars * 4.0),
            },
            ControlItem {
                label: "LFO Depth",
                value: c.perc.lfo_depth,
                min: 0.0,
                max: 1.0,
                display: format!("{:.0}%", c.perc.lfo_depth * 100.0),
            },
            ControlItem {
                label: "Interval",
                value: c.perc.interval_beats,
                min: 0.25,
                max: 4.25,
                display: if c.perc.interval_beats >= 4.25 {
                    "Continuous".to_string()
                } else {
                    format!("{:.2} beats", c.perc.interval_beats)
                },
            },
            ControlItem {
                label: "Offset",
                value: c.perc.offset_beats,
                min: 0.0,
                max: 4.0,
                display: format!("{:.2} beats", c.perc.offset_beats),
            },
        ],
```

(Only the `Decay`, `Filter`, `LFO Rate`, `LFO Depth` items are unchanged from current code — they're shown here for placement context. The Decay item's position and behavior are untouched, per the spec's requirement that it not move.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test perc_tab_controls_include_interval_and_offset perc_interval_displays_continuous_at_top --lib`
Expected: PASS

- [ ] **Step 5: Write the failing test for `apply_delta`/`apply_min` on the new indices**

Add to `mod tests`:

```rust
    #[test]
    fn perc_interval_and_offset_adjust_and_clamp() {
        let mut controls = FluidControls::default();

        apply_delta(Tab::Perc, 5, 1.0, &mut controls);
        assert_close(controls.perc.interval_beats, 0.5);

        controls.perc.interval_beats = 4.25;
        apply_delta(Tab::Perc, 5, 1.0, &mut controls);
        assert_close(controls.perc.interval_beats, 4.25); // clamps at continuous sentinel

        apply_delta(Tab::Perc, 6, 1.0, &mut controls);
        assert_close(controls.perc.offset_beats, 0.25);

        controls.perc.offset_beats = 4.0;
        apply_delta(Tab::Perc, 6, 1.0, &mut controls);
        assert_close(controls.perc.offset_beats, 4.0); // clamps at top

        apply_min(Tab::Perc, 5, &mut controls);
        assert_close(controls.perc.interval_beats, 0.25);

        apply_min(Tab::Perc, 6, &mut controls);
        assert_close(controls.perc.offset_beats, 0.0);
    }
```

- [ ] **Step 6: Run test to verify it fails**

Run: `cargo test perc_interval_and_offset_adjust_and_clamp --lib`
Expected: FAIL — indices 5/6 currently hit the `_ => {}` no-op arm in both `apply_delta` and `apply_min`.

- [ ] **Step 7: Add the match arms**

In `apply_delta`'s `Tab::Perc` arm (around line 814):

```rust
        Tab::Perc => match selected {
            0 => c.perc.level = (c.perc.level + dir * 0.02).clamp(0.0, 1.0),
            1 => c.perc.decay_ms = (c.perc.decay_ms + dir * 20.0).clamp(20.0, 2000.0),
            2 => c.perc.filter = (c.perc.filter + dir * 0.02).clamp(0.5, 1.0),
            3 => c.perc.lfo_rate_bars = (c.perc.lfo_rate_bars + dir * 0.25).clamp(0.25, 16.0),
            4 => c.perc.lfo_depth = (c.perc.lfo_depth + dir * 0.02).clamp(0.0, 1.0),
            5 => c.perc.interval_beats = (c.perc.interval_beats + dir * 0.25).clamp(0.25, 4.25),
            6 => c.perc.offset_beats = (c.perc.offset_beats + dir * 0.25).clamp(0.0, 4.0),
            _ => {}
        },
```

In `apply_min`'s `Tab::Perc` arm (around line 815-820):

```rust
        Tab::Perc => match selected {
            0 => c.perc.level = 0.0,
            1 => c.perc.decay_ms = 20.0,
            2 => c.perc.filter = 0.5,
            3 => c.perc.lfo_rate_bars = 0.25,
            4 => c.perc.lfo_depth = 0.0,
            5 => c.perc.interval_beats = 0.25,
            6 => c.perc.offset_beats = 0.0,
            _ => {}
        },
```

- [ ] **Step 8: Run test to verify it passes**

Run: `cargo test perc_interval_and_offset_adjust_and_clamp --lib`
Expected: PASS

- [ ] **Step 9: Run full test suite and build**

Run: `cargo test --lib && cargo build`
Expected: PASS, clean build

- [ ] **Step 10: Commit**

```bash
git add src/fluid.rs
git commit -m "feat: wire perc interval/offset controls into terminal UI"
```

---

### Task 4: Trim resolved `GOTCHAS.md` entries

**Files:**
- Modify: `GOTCHAS.md`

**Interfaces:**
- Consumes: nothing code-level — this is documentation cleanup confirming Tasks 1-3 landed.
- Produces: nothing consumed elsewhere.

- [ ] **Step 1: Read the current entries**

Run: `grep -n "crossfade\|analytical RMS\|Reimagined control direction" GOTCHAS.md`

Read the matched sections with the Read tool to get exact text and line ranges before editing.

- [ ] **Step 2: Remove the two failed-approach entries and the "Reimagined control direction" note**

Delete the crossfade-blend entry, the analytical-RMS-switch entry, and the "Reimagined control direction" note in full (headings, body text, and any surrounding blank lines they own) — this spec's implementation is the working fix those notes were pointing toward, so they're now stale rather than informative.

- [ ] **Step 3: Verify no other code/docs reference the removed sections**

Run: `grep -rn "crossfade\|analytical RMS\|Reimagined control direction" --include="*.md" --include="*.rs" .`
Expected: no remaining references outside of this plan/spec doc (which are historical records of the design process, not living docs).

- [ ] **Step 4: Commit**

```bash
git add GOTCHAS.md
git commit -m "docs: remove resolved perc continuous-noise gotchas"
```

---

### Task 5: Close the backlog task

**Files:**
- Modify: `.stint/tasks/0001-add-perc-beat-control.md`

**Interfaces:**
- Consumes: nothing code-level.
- Produces: nothing consumed elsewhere.

- [ ] **Step 1: Update the task status**

In `.stint/tasks/0001-add-perc-beat-control.md`, change the frontmatter `status: backlog` to `status: done`.

- [ ] **Step 2: Commit**

```bash
git add .stint/tasks/0001-add-perc-beat-control.md
git commit -m "chore: close perc beat control task"
```
