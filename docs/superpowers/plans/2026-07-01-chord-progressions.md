# Chord Progressions (A/B/C/D) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the Pad voice's single fixed 5-chord loop with four selectable 8-chord progressions (A/B/C/D), authored as MIDI note numbers, with a UI selector that instantly retriggers the current chord slot in the new progression when switched.

**Architecture:** All changes live in `src/fluid.rs`. A new `midi_to_hz` helper and a `PROGRESSIONS: [[[i32; 4]; 8]; 4]` table replace `pad_chord`'s existing 5-chord Hz table. `PadControls` gains a `progression: f32` field; `PadEngine` renames its unbounded `chord_index` to a wrapping `step_index: usize` (0..8) and adds `last_progression: usize` to detect selector changes and force an immediate layer retrigger. The terminal UI gets one new "Progression" row on the Chords tab.

**Tech Stack:** Rust, `cargo test`.

## Global Constraints

- All 32 chords (4 progressions x 8 chords) use only MIDI notes whose pitch class is A/B/C/D/E/F/G (no sharps/flats) — no chromatic notes anywhere, per the spec.
- `chord_bars` default changes from `4.0` to `1.0`; min/max (`1.0`/`64.0`) and the existing doubling/halving control behavior are unchanged.
- No changes to voice count (4 notes/chord), panning, gains, attack, reverb, stereo width, detune, or octave-mix logic.
- `progression` UI value range is `0.0..=3.0`, stepped by `1.0`, displayed as `"A"`/`"B"`/`"C"`/`"D"`.

---

### Task 1: MIDI progression table and `pad_chord`/`pad_tones`/`PadLayer` plumbing

**Files:**
- Modify: `src/fluid.rs` (`pad_chord` at line 1706, `pad_tones` at line 1694, `PadLayer::new` at line 1635, `PadControls` struct at line 109, its `Default` impl at line ~123)
- Test: `src/fluid.rs` (inline `#[cfg(test)]` module, same file, existing tests start around line 2242)

**Interfaces:**
- Produces: `fn midi_to_hz(note: i32) -> f32`, `fn pad_chord(progression: usize, step: usize) -> [f32; 4]`, `fn pad_tones(progression: usize, step: usize, sample_rate: f32, attack_time: f32) -> Vec<PadTone>`, `PadLayer::new(progression: usize, step: usize, sample_rate: f32, attack_time: f32) -> Self`, `PadControls.progression: f32` (default `0.0`), `PadControls.chord_bars` default now `1.0`.
- Consumes: nothing from other tasks (this is the base layer).

- [ ] **Step 1: Write the failing tests for `midi_to_hz` and `pad_chord`**

Add to the `#[cfg(test)]` module in `src/fluid.rs`, near the other `pad`-related tests:

```rust
#[test]
fn midi_to_hz_matches_known_notes() {
    assert_close(midi_to_hz(69), 440.0); // A4
    assert_close(midi_to_hz(45), 110.0); // A2
    assert_close(midi_to_hz(60), 261.63); // C4 (allow the existing assert_close tolerance)
}

#[test]
fn pad_chord_converts_progression_a_first_chord() {
    let chord = pad_chord(0, 0);
    assert_close(chord[0], 110.0); // A2
    assert_close(chord[1], 146.83); // D3
    assert_close(chord[2], 196.0); // G3
    assert_close(chord[3], 261.63); // C4
}

#[test]
fn pad_chord_converts_progression_d_last_chord() {
    let chord = pad_chord(3, 7);
    assert_close(chord[0], 110.0); // A2
    assert_close(chord[1], 164.81); // E3
    assert_close(chord[2], 220.0); // A3
    assert_close(chord[3], 261.63); // C4
}

#[test]
fn pad_chord_wraps_progression_and_step_index() {
    let wrapped_progression = pad_chord(4, 0);
    let base_progression = pad_chord(0, 0);
    assert_eq!(wrapped_progression, base_progression);

    let wrapped_step = pad_chord(0, 8);
    let base_step = pad_chord(0, 0);
    assert_eq!(wrapped_step, base_step);
}

#[test]
fn pad_defaults_use_progression_a_and_one_bar_chords() {
    let controls = PadControls::default();
    assert_close(controls.chord_bars, 1.0);
    assert_close(controls.progression, 0.0);
}
```

Note: `assert_close`'s existing tolerance (check its definition at line 2242) must be loose enough for the `261.63`-style rounded reference values above — if it isn't, use the exact computed value `440.0 * 2f32.powf((60.0 - 69.0) / 12.0)` in the assertion instead of the rounded literal.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test pad_chord`
Expected: FAIL — `midi_to_hz`/`pad_chord(usize, usize)`/`PadControls.progression` don't exist yet (compile error).

- [ ] **Step 3: Implement `midi_to_hz` and the 4x8 `PROGRESSIONS` table, replacing `pad_chord`**

Replace lines 1706-1715 (the current `fn pad_chord`) with:

```rust
fn midi_to_hz(note: i32) -> f32 {
    440.0 * 2f32.powf((note as f32 - 69.0) / 12.0)
}

const PROGRESSIONS: [[[i32; 4]; 8]; 4] = [
    [
        [45, 50, 55, 60],
        [45, 52, 55, 62],
        [43, 50, 57, 60],
        [47, 52, 55, 62],
        [45, 50, 57, 64],
        [48, 55, 60, 64],
        [43, 50, 55, 59],
        [45, 52, 57, 60],
    ],
    [
        [45, 50, 57, 60],
        [50, 53, 57, 62],
        [48, 55, 60, 64],
        [43, 50, 55, 59],
        [41, 48, 53, 57],
        [45, 52, 57, 60],
        [52, 59, 64, 67],
        [45, 50, 57, 60],
    ],
    [
        [45, 48, 52, 55],
        [41, 45, 48, 52],
        [48, 52, 55, 59],
        [43, 47, 50, 53],
        [50, 53, 57, 60],
        [45, 48, 52, 55],
        [52, 55, 59, 62],
        [41, 45, 48, 52],
    ],
    [
        [45, 52, 57, 60],
        [41, 45, 48, 55],
        [48, 55, 59, 62],
        [43, 50, 53, 57],
        [50, 57, 60, 64],
        [45, 52, 55, 60],
        [52, 55, 59, 64],
        [45, 52, 57, 60],
    ],
];

fn pad_chord(progression: usize, step: usize) -> [f32; 4] {
    PROGRESSIONS[progression % PROGRESSIONS.len()][step % 8].map(midi_to_hz)
}
```

- [ ] **Step 4: Update `pad_tones` and `PadLayer::new` to take `progression` and `step`**

Replace lines 1694-1704 (`fn pad_tones`):

```rust
fn pad_tones(progression: usize, step: usize, sample_rate: f32, attack_time: f32) -> Vec<PadTone> {
    let freqs = pad_chord(progression, step);
    let pans = [-0.52_f32, -0.18, 0.16, 0.46];
    let gains = [0.17_f32, 0.132, 0.126, 0.098];
    freqs
        .iter()
        .zip(pans)
        .zip(gains)
        .map(|((hz, pan), gain)| PadTone::new(*hz, pan, gain, attack_time, sample_rate))
        .collect()
}
```

Replace lines 1634-1639 (`impl PadLayer { fn new ... }`):

```rust
impl PadLayer {
    fn new(progression: usize, step: usize, sample_rate: f32, attack_time: f32) -> Self {
        Self {
            tones: pad_tones(progression, step, sample_rate, attack_time),
        }
    }
```

This will break the two existing call sites of `PadLayer::new` (in `PadEngine::new` and `PadEngine::next`) and `PadControls`'s field list — Task 2 fixes both. For now, add the `progression` field to `PadControls` (near `chord_bars`, line 109) and its `Default` impl (line ~123) so this task compiles standalone:

```rust
pub(crate) struct PadControls {
    pub level: f32,
    pub chord_bars: f32, // 1,2,4,8,16,32,64
    pub progression: f32,
    pub reverb_mix: f32,
    pub stereo_width: f32,
    pub detune: f32,
    pub octave_mix: f32,
    pub attack_time: f32,
}
```

```rust
impl Default for PadControls {
    fn default() -> Self {
        Self {
            level: /* unchanged existing value */,
            chord_bars: 1.0,
            progression: 0.0,
            reverb_mix: /* unchanged existing value */,
            stereo_width: /* unchanged existing value */,
            detune: /* unchanged existing value */,
            octave_mix: /* unchanged existing value */,
            attack_time: /* unchanged existing value */,
        }
    }
}
```

Copy the unchanged field values verbatim from the current `Default for PadControls` impl at line ~123 — only `chord_bars` (4.0 -> 1.0) changes value, and `progression` is newly inserted.

Because `PadLayer::new` and `PadEngine::new`/`PadEngine::next` are not yet updated to pass `progression`, the crate will not compile after this step alone. Proceed directly to Task 2 in the same session before running the full test suite — Steps 5-6 below run only the two new-table tests via targeted `cargo test` invocations that still fail to compile; this is expected and resolved by Task 2's Step 3.

- [ ] **Step 5: Commit the data-layer change together with Task 2 (do not commit yet)**

Do not run `cargo test` or commit at the end of this task — the crate does not compile until Task 2's engine changes land. Proceed immediately to Task 2.

---

### Task 2: `PadEngine` step/progression tracking and instant-retrigger switching

**Files:**
- Modify: `src/fluid.rs` (`PadEngine` struct at line 1550, `PadEngine::new` at line 1563, `PadEngine::next` at line 1579)
- Test: `src/fluid.rs` inline test module

**Interfaces:**
- Consumes: `PadLayer::new(progression: usize, step: usize, sample_rate: f32, attack_time: f32)`, `pad_chord(progression: usize, step: usize) -> [f32; 4]`, `PadControls.progression: f32` (all from Task 1).
- Produces: `PadEngine.step_index: usize` (renamed from `chord_index`, now wraps `0..8`), `PadEngine.last_progression: usize` (new field) — Task 3 does not depend on these directly (UI only touches `PadControls`), but keep the names exact since they appear in this task's own tests.

- [ ] **Step 1: Update `PadEngine` struct and `PadEngine::new`**

Replace line 1554 (`chord_index: usize,`) with:

```rust
    step_index: usize,
    last_progression: usize,
```

Replace lines 1567-1569 inside `PadEngine::new`'s `Self { ... }` (currently `layers: vec![PadLayer::new(0, sample_rate, c.attack_time)],` and `chord_index: 0,`):

```rust
            layers: vec![PadLayer::new(0, 0, sample_rate, c.attack_time)],
            chord_trigger: GridTrigger::after_start(),
            step_index: 0,
            last_progression: 0,
```

- [ ] **Step 2: Write the failing tests for step wrapping and instant progression retrigger**

Add to the test module:

```rust
#[test]
fn pad_engine_step_index_wraps_at_eight() {
    let controls = PadControls {
        chord_bars: 1.0,
        attack_time: 1.0,
        ..PadControls::default()
    };
    let mut pad = PadEngine::new(SAMPLE_RATE, &controls, Arc::new(FluidTelemetry::default()));

    // chord_bars=1.0 means chord_trigger fires every 4.0 beats; at 120 BPM
    // that's 2 seconds of samples per chord. Render 9 chord-advances worth
    // of samples (18 seconds) and confirm the telemetry index wrapped past 8.
    for chord in 1..=9 {
        let sample = chord * SAMPLE_RATE as u64 * 2;
        let _ = pad.next(&controls, timing(sample, 120.0));
    }
    let final_index = pad.telemetry.chord_index.load(Ordering::Relaxed);
    assert!(final_index < 8, "step_index must wrap into 0..8, got {final_index}");
}

#[test]
fn pad_engine_progression_switch_retriggers_immediately() {
    let mut controls = PadControls {
        chord_bars: 64.0, // long chord length so no chord-advance trigger fires
        attack_time: 1.0,
        ..PadControls::default()
    };
    let mut pad = PadEngine::new(SAMPLE_RATE, &controls, Arc::new(FluidTelemetry::default()));

    let layers_before = pad.layers.len();
    let _ = pad.next(&controls, timing(0, 120.0));

    // Flip progression with no elapsed time / no chord-advance trigger.
    controls.progression = 1.0;
    let _ = pad.next(&controls, timing(1, 120.0));

    assert!(
        pad.layers.len() > layers_before,
        "switching progression must push a new layer immediately, without waiting for chord_trigger"
    );
}
```

- [ ] **Step 3: Run the tests to verify they fail (or fail to compile)**

Run: `cargo test pad_engine_step_index_wraps_at_eight pad_engine_progression_switch_retriggers_immediately`
Expected: FAIL to compile — `PadEngine::next` still calls `PadLayer::new(self.chord_index, ...)` with the old 2-arg signature, and never reads `c.progression`.

- [ ] **Step 4: Implement the new `PadEngine::next` trigger logic**

Replace lines 1579-1597 (the top of `fn next`, from `if self.chord_trigger.pop(...)` through the closing `}` of that `if` block):

```rust
    fn next(&mut self, c: &PadControls, timing: TimingContext) -> (f32, f32) {
        let progression = (c.progression.round() as i64).rem_euclid(4) as usize;
        let progression_changed = progression != self.last_progression;
        self.last_progression = progression;

        let advance = self.chord_trigger.pop(timing, c.chord_bars * 4.0, 0.0);

        if advance || progression_changed {
            for layer in &mut self.layers {
                layer.release();
            }
            if advance {
                self.step_index = (self.step_index + 1) % 8;
            }
            self.telemetry
                .chord_index
                .store(self.step_index as u64, Ordering::Relaxed);
            if self.layers.len() >= MAX_PAD_LAYERS {
                let remove_count = self.layers.len() + 1 - MAX_PAD_LAYERS;
                self.layers.drain(0..remove_count);
            }
            self.layers.push(PadLayer::new(
                progression,
                self.step_index,
                self.sample_rate,
                c.attack_time,
            ));
        }
```

Leave the rest of `fn next` (the LFO/reverb/mixing code below the old `if` block) untouched — it doesn't reference `chord_index` or `progression`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test pad_chord pad_defaults pad_engine`
Expected: PASS (all Task 1 and Task 2 tests, plus the pre-existing `pad_engine_caps_released_layers` test).

- [ ] **Step 6: Run the full test suite**

Run: `cargo test`
Expected: PASS. If `pad_engine_caps_released_layers` (existing test, uses `chord_bars: 1.0`) fails, check it doesn't reference the removed `chord_index` field name — it doesn't, per the current source, so it should be unaffected.

- [ ] **Step 7: Commit**

```bash
git add src/fluid.rs
git commit -m "Add MIDI-authored A/B/C/D chord progressions to the Pad voice"
```

---

### Task 3: Terminal UI — Progression selector on the Chords tab

**Files:**
- Modify: `src/fluid.rs` (`tab_controls`'s `Tab::Chords` arm at line 480, `apply_delta`'s `Tab::Chords` arm at line 759, `apply_min`'s `Tab::Chords` arm at line 849)
- Test: `src/fluid.rs` inline test module

**Interfaces:**
- Consumes: `PadControls.progression: f32` (Task 1).
- Produces: nothing consumed by later tasks (this is the last task).

- [ ] **Step 1: Write the failing tests for the new UI row and its indices**

Add to the test module:

```rust
#[test]
fn chords_tab_shows_progression_row_with_letter_display() {
    let mut controls = FluidControls::default();
    let rows = tab_controls(Tab::Chords, &controls);
    assert_eq!(rows[2].label, "Progression");
    assert_eq!(rows[2].display, "A");

    controls.pad.progression = 2.0;
    let rows = tab_controls(Tab::Chords, &controls);
    assert_eq!(rows[2].display, "C");
}

#[test]
fn chords_progression_adjusts_and_clamps() {
    let mut controls = FluidControls::default();

    apply_delta(Tab::Chords, 2, 1.0, &mut controls);
    assert_close(controls.pad.progression, 1.0);

    controls.pad.progression = 3.0;
    apply_delta(Tab::Chords, 2, 1.0, &mut controls);
    assert_close(controls.pad.progression, 3.0);

    controls.pad.progression = 0.0;
    apply_delta(Tab::Chords, 2, -1.0, &mut controls);
    assert_close(controls.pad.progression, 0.0);

    controls.pad.progression = 2.0;
    apply_min(Tab::Chords, 2, &mut controls);
    assert_close(controls.pad.progression, 0.0);
}

#[test]
fn chords_reverb_mix_row_shifted_to_index_three() {
    let controls = FluidControls::default();
    let rows = tab_controls(Tab::Chords, &controls);
    assert_eq!(rows[3].label, "Reverb Mix");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test chords_tab_shows_progression_row chords_progression_adjusts chords_reverb_mix_row`
Expected: FAIL — row 2 is currently `"Reverb Mix"`, `apply_delta`/`apply_min` index `2` currently falls through to `_ => {}` (no-op) for `Tab::Chords`.

- [ ] **Step 3: Insert the Progression row into `tab_controls`'s `Tab::Chords` arm**

In the `Tab::Chords => vec![ ... ]` arm (starting line 480), after the existing `"Chord Length"` `ControlItem` (ends around line 494) and before the existing `"Reverb Mix"` `ControlItem`, insert:

```rust
            ControlItem {
                label: "Progression".to_string(),
                value: c.pad.progression,
                min: 0.0,
                max: 3.0,
                display: ["A", "B", "C", "D"][c.pad.progression.round() as usize % 4]
                    .to_string(),
            },
```

- [ ] **Step 4: Add the index-2 case to `apply_delta`'s `Tab::Chords` arm and shift existing indices**

In the `Tab::Chords => match selected { ... }` arm (starting line 759): keep index `0` (`c.pad.level = ...`) and index `1` (`c.pad.chord_bars` doubling logic) unchanged. Insert a new index `2`, then renumber every existing case from `2` onward (Reverb Mix, Stereo Width, Detune, Octave Mix, Attack) up by one:

```rust
        Tab::Chords => match selected {
            0 => c.pad.level = (c.pad.level + dir * 0.02).clamp(0.0, 1.0),
            1 => {
                if dir > 0.0 {
                    c.pad.chord_bars = (c.pad.chord_bars * 2.0).min(64.0)
                } else {
                    c.pad.chord_bars = (c.pad.chord_bars / 2.0).max(1.0)
                }
            }
            2 => c.pad.progression = (c.pad.progression + dir).clamp(0.0, 3.0),
            3 => c.pad.reverb_mix = (c.pad.reverb_mix + dir * 0.02).clamp(0.0, 1.0),
            4 => c.pad.stereo_width = (c.pad.stereo_width + dir * 0.02).clamp(0.0, 1.0),
            5 => c.pad.detune = (c.pad.detune + dir * 0.02).clamp(0.0, 1.0),
            6 => c.pad.octave_mix = (c.pad.octave_mix + dir * 0.02).clamp(0.0, 1.0),
            7 => c.pad.attack_time = (c.pad.attack_time + dir * 0.5).clamp(1.0, 30.0),
            _ => {}
        },
```

Read the current bodies of the Reverb Mix / Stereo Width / Detune / Octave Mix / Attack cases at their existing indices `2`-`6` (lines ~759-780) before renumbering — copy each body verbatim into its new index slot above; do not change any clamp ranges or step sizes, only the index number and (for Attack) confirm the field name matches `PadControls.attack_time`.

- [ ] **Step 5: Add the index-2 case to `apply_min`'s `Tab::Chords` arm and shift existing indices**

In the `Tab::Chords => match selected { ... }` arm inside `apply_min` (starting line 849): keep index `0` and `1` unchanged, insert index `2`, renumber the rest up by one, following the same read-current-bodies-first approach as Step 4:

```rust
        Tab::Chords => match selected {
            0 => c.pad.level = 0.0,
            1 => c.pad.chord_bars = 1.0,
            2 => c.pad.progression = 0.0,
            3 => c.pad.reverb_mix = 0.0,
            4 => c.pad.stereo_width = 0.0,
            5 => c.pad.detune = 0.0,
            6 => c.pad.octave_mix = 0.0,
            7 => c.pad.attack_time = 1.0,
            _ => {}
        },
```

Verify against the current source before committing to these literals — copy the exact reset values from the existing indices `2`-`6`, don't assume they're all `0.0`.

- [ ] **Step 6: Run the new tests to verify they pass**

Run: `cargo test chords_tab_shows_progression_row chords_progression_adjusts chords_reverb_mix_row`
Expected: PASS

- [ ] **Step 7: Search for any other tests hardcoding `Tab::Chords` indices >= 2 and update them**

Run: `grep -n "Tab::Chords" src/fluid.rs`
Expected: review every match; any `apply_delta(Tab::Chords, N, ...)` or `apply_min(Tab::Chords, N, ...)` or `rows[N]` (for `Tab::Chords` rows) where `N >= 2` in pre-existing tests must be incremented by 1 to account for the new Progression row. Update each one found.

- [ ] **Step 8: Run the full test suite**

Run: `cargo test`
Expected: PASS, all tests green.

- [ ] **Step 9: Commit**

```bash
git add src/fluid.rs
git commit -m "Add Progression (A/B/C/D) selector to the Chords tab UI"
```

## Out of scope follow-ups

None identified — this closes the spec at `docs/superpowers/specs/2026-07-01-chord-progressions-design.md`.
