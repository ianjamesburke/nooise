# LFO Ergonomics Rework Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the `l`-key LFO stub with an `f`-toggled editable submenu (amount/interval/offset) and a beat-synced animated wave lane.

**Architecture:** `LfoRoute` collapses to one `depth_ratio`; a `LfoField` enum gives the three submenu rows the same adjust/set/reset semantics as registry controls. The audio engine publishes its beat through `FluidTelemetry` (lock-free `AtomicU64` of `f64::to_bits`), and the UI renders a phase-locked sine lane plus a live modulated-value marker on the parent slider bar.

**Tech Stack:** Rust, ratatui, crossterm, arc-swap. Tests via `cargo test`.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-02-lfo-ergonomics-design.md`
- No compatibility with the unshipped stint-0005 automation record layout; `AUTOMATION_PAYLOAD_VERSION` stays 1 with the new field set.
- Default route: depth 25%, interval 2 beats, offset 0. Interval ladder: 1/4, 1/2, 1, 2, 4, 8, 16 beats.
- Verify with `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` before each commit.

---

### Task 1: Simplify LfoRoute and add field editing model

**Files:**
- Modify: `src/fluid/automation.rs`
- Test: `src/fluid/tests.rs`

**Interfaces:**
- Produces: `LfoRoute { depth_ratio, cycle_beats, phase_offset_cycles, shape }`, `LfoRoute::phase_at(beat: f64) -> f64`, `LfoRoute::wave_at(beat: f64) -> f32` (sine in −1..1, NOT depth-scaled), `LfoField::{Amount, Interval, Offset}` with `ALL`, `label()`, and `LfoRoute::{adjust_field, set_field, reset_field, field_ratio, field_display}`, `AutomationState::close_editor` deleting depth≈0 routes, `INTERVAL_LADDER: [f32; 7]`.

- [ ] **Step 1: Write failing tests** in `src/fluid/tests.rs`: replace the old depth-split assertions in `automation_open_or_create_uses_safe_lfo_defaults` (default depth 0.25, interval 2, offset 0); add `lfo_field_adjust_steps_and_clamps` (amount ±0.05 clamped 0..1, interval walks the ladder and clamps at ends, offset ±0.125 wraps via rem_euclid), `lfo_field_set_snaps_interval_to_ladder` (`set_field(Interval, 3.1)` → 4.0; `set_field(Amount, 130.0)` → 1.0 via percent normalization), `close_editor_deletes_zero_depth_route`, `lfo_phase_at_uses_cycle_and_offset` (cycle 2, offset 0.25, beat 1.0 → phase 0.75).
- [ ] **Step 2: Run** `cargo test lfo_ close_editor automation_open` — expect FAIL (fields don't exist).
- [ ] **Step 3: Implement** in `src/fluid/automation.rs`: delete `DEFAULT_LFO_TARGET_DEPTH_RATIO`/`DEFAULT_LFO_EFFECTIVE_DEPTH_RATIO`, add `DEFAULT_LFO_DEPTH_RATIO = 0.25`, `MIN_LFO_CYCLE_BEATS = 0.25`, `INTERVAL_LADDER`. New route shape and field model:

```rust
pub(crate) const INTERVAL_LADDER: [f32; 7] = [0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LfoField { Amount, Interval, Offset }

impl LfoField {
    pub(crate) const ALL: [LfoField; 3] = [Self::Amount, Self::Interval, Self::Offset];
    pub(crate) fn label(self) -> &'static str {
        match self { Self::Amount => "amount", Self::Interval => "interval", Self::Offset => "offset" }
    }
}

impl LfoRoute {
    pub(crate) fn phase_at(&self, beat: f64) -> f64 {
        (beat / f64::from(self.cycle_beats.max(MIN_LFO_CYCLE_BEATS))
            + f64::from(self.phase_offset_cycles)).rem_euclid(1.0)
    }
    pub(crate) fn wave_at(&self, beat: f64) -> f32 {
        match self.shape { LfoShape::Sine => (std::f64::consts::TAU * self.phase_at(beat)).sin() as f32 }
    }
    pub(crate) fn adjust_field(&mut self, field: LfoField, dir: f32) { /* amount ±0.05 clamp; interval ladder index ± 1; offset ±0.125 rem_euclid(1.0) */ }
    pub(crate) fn set_field(&mut self, field: LfoField, value: f32) { /* amount: normalize_unit_input; interval: nearest ladder; offset: rem_euclid(1.0) */ }
    pub(crate) fn reset_field(&mut self, field: LfoField) { /* back to per-field default */ }
    pub(crate) fn field_ratio(&self, field: LfoField) -> f32 { /* amount, ladder index/(len-1), offset */ }
    pub(crate) fn field_display(&self, field: LfoField) -> String { /* "25%", "2.00 beats" (fractions as 0.25), "1/8 cyc" style: use {:.2} */ }
}
```

`close_editor` becomes: take `open`; if that route's `depth_ratio <= f32::EPSILON`, remove it. Add `AutomationState::route_mut(&mut self, address) -> Option<&mut LfoRoute>`.
- [ ] **Step 4: Run** the same tests — expect PASS (Task 2/3 call sites will still break the build; fix them in the same commit if `cargo test` won't compile: update `apply_automation` and song.rs minimally). If compile blocks, fold Tasks 1–3 into one commit.
- [ ] **Step 5: Commit** `feat: single-depth lfo route with editable fields`

### Task 2: Automation engine uses depth_ratio

**Files:**
- Modify: `src/fluid/automation.rs` (`apply_automation`)
- Test: `src/fluid/tests.rs` (adapt `automation_applies_bounded_lfo_offset_and_clamps_to_spec_range`, `automation_uses_beat_cycle_phase_for_opposite_lfo_offsets`, `automation_preserves_base_controls_and_modulates_only_effective_clone`)

**Interfaces:**
- Consumes: `LfoRoute::wave_at`.
- Produces: `apply_automation` modulating by `(spec.max - spec.min) * depth_ratio * wave_at(beat)`.

- [ ] **Step 1:** Update the three engine tests to build routes with `depth_ratio` and assert the same modulation math.
- [ ] **Step 2:** `cargo test automation_` — FAIL.
- [ ] **Step 3:** Rewrite the `apply_automation` body: `depth = (spec.max - spec.min) * route.depth_ratio.clamp(0.0, 1.0)`, skip if ≤ EPSILON, `offset = route.wave_at(timing.beat) * depth`.
- [ ] **Step 4:** `cargo test automation_` — PASS.
- [ ] **Step 5:** Commit `feat: automation engine reads single lfo depth`

### Task 3: Song-code record layout for the new route

**Files:**
- Modify: `src/fluid/song.rs` (`write_automation`, `read_automation`)
- Test: `src/fluid/tests.rs` (`song_code_round_trips_lfo_automation_record`, `automation_payload` helper)

**Interfaces:**
- Produces: record fields per route: `id, cycle_beats: f32, depth_ratio: f32, shape: u8, phase_offset_cycles: f32`.

- [ ] **Step 1:** Update `automation_payload` helper + round-trip test for the new layout (write depth 0.4, cycle 4, offset 0.25; decode equals input; non-finite depth falls back to 0.25).
- [ ] **Step 2:** `cargo test song_code` — FAIL.
- [ ] **Step 3:** Update writer/reader; decode fallbacks: `cycle finite_or(2.0).max(0.25)`, `depth finite_or(0.25).clamp(0,1)`, `offset finite_or(0.0).rem_euclid(1.0)`.
- [ ] **Step 4:** `cargo test song_code` — PASS.
- [ ] **Step 5:** Commit `feat: song codes carry simplified lfo routes`

### Task 4: Engine publishes beat through telemetry

**Files:**
- Modify: `src/fluid/mod.rs` (`FluidTelemetry`), `src/fluid/engine.rs` (`next_stereo`)
- Test: `src/fluid/tests.rs`

**Interfaces:**
- Produces: `FluidTelemetry::beat(&self) -> f64`, `FluidTelemetry::publish_beat(&self, beat: f64)` backed by `beat_bits: AtomicU64`.

- [ ] **Step 1:** Test `engine_publishes_beat_telemetry`: build `FluidEngine` at 44_100 Hz, call `next_stereo()` 512 times, assert `telemetry.beat()` > 0 and ≈ `512.0 * bpm / (60.0 * 44_100.0)` within 1%.
- [ ] **Step 2:** `cargo test engine_publishes` — FAIL.
- [ ] **Step 3:** Add field + methods (`store(bits, Ordering::Relaxed)` / `f64::from_bits(load)`); engine keeps an `Arc<FluidTelemetry>` field and calls `publish_beat(timing.beat)` when `current_sample.is_multiple_of(256)`.
- [ ] **Step 4:** `cargo test engine_publishes` — PASS.
- [ ] **Step 5:** Commit `feat: publish beat position to ui telemetry`

### Task 5: Keybindings and submenu interaction

**Files:**
- Modify: `src/fluid/ui.rs` (`ui_loop`)
- Test: `src/fluid/tests.rs`

**Interfaces:**
- Consumes: `LfoField`, `AutomationState::{open_or_create, close_editor, route_mut, active_address}`.
- Produces: extracted pure handler `handle_control_key(key: KeyCode, modifiers: KeyModifiers, ctx: &mut KeyContext) -> KeyOutcome` is NOT required; instead extract `LfoEditor { selected: usize }` cursor logic as free functions testable without a terminal: `lfo_rows_len() -> usize` (4: parent + 3 fields), and drive tests through `AutomationState` + route mutations directly.

Behavior in `ui_loop`:
- `f`: if editor open on the selected control's address → `close_editor` + publish; else `open_or_create` + publish; `lfo_selected = 0`.
- `l` / `Right`: if editor open and `lfo_selected > 0` → `adjust_field(field, 1.0)` + publish; else `adjust(...)` (slider up). Same for `h`/`Left` with −1.0. `H`/`Shift+Left`: `reset_field` or `reset_to_min`.
- `j`/`k`/arrows while editor open move `lfo_selected` within 0..=3 instead of `selected`.
- Numeric Enter while editor open and `lfo_selected > 0` → `set_field` + publish.
- `Esc` with editor open closes it (existing arm stays; add publish already present).
- `Tab`/`BackTab` close the editor first.
- Footer help becomes: `"jk select   h/l adjust   f LFO   type value   Enter set   q quit"`.

- [ ] **Step 1:** Tests: `lfo_toggle_open_close_removes_dead_route` (open, leave depth 0, close via toggle → no route), `lfo_field_edit_publishes_route` (adjust amount on field row → route depth 0.30). These exercise `AutomationState` + `LfoRoute` seams added in Task 1 plus any new free functions; UI wiring itself is covered by the render test in Task 6.
- [ ] **Step 2:** Run — FAIL where new seams are missing.
- [ ] **Step 3:** Implement `ui_loop` changes above.
- [ ] **Step 4:** `cargo test` — PASS; manual smoke: `cargo run` (skip if headless).
- [ ] **Step 5:** Commit `feat: f-key lfo submenu with editable fields`

### Task 6: Animated lane and modulated slider marker

**Files:**
- Modify: `src/fluid/ui.rs` (`render_fluid`, `automation_line` → `lfo_lane_line`, new `slider_spans`), `src/fluid/mod.rs` (pass `beat: f64` into `render`)
- Test: `src/fluid/tests.rs` (adapt `render_fluid_draws_oscillator_lane_for_automated_slider`, add `lfo_lane_is_phase_locked`)

**Interfaces:**
- Consumes: `FluidTelemetry::beat()`, `LfoRoute::{phase_at, wave_at, depth_ratio}`.
- Produces: `render(f, items, tab, selected, numeric, fluid, automation, lfo_selected, beat, footer)`; `lfo_lane_line(route, beat, width, active) -> Line<'static>`; `slider_spans(ratio, modulated: Option<f32>, width) -> Vec<Span<'static>>`.

Rendering rules:
- Lane: one cycle across `width` columns; per column `v = wave shape sampled at column phase`, height char from `['▁','▂','▃','▄','▅','▆','▇','█']` with amplitude scaled by `depth_ratio`; head column = `phase_at(beat) * width`; per-column color `fluid_hsv(300.0 ± 20*v, 0.6, brightness)` where brightness falls off with wrapped distance from the head (bright pink head ≥ 0.9, tail floor 0.35).
- Submenu rows (editor open): three indented rows `label bar display` styled like control rows, `▶` prefix on `lfo_selected` row (parent row keeps its existing `▶` when `lfo_selected == 0`).
- Parent slider bar: when a route exists, `modulated = (ratio + depth_ratio * wave_at(beat)).clamp(0,1)`; marker cell drawn as `'◆'` in bright cyan at `modulated * (width-1)`, rest of the bar unchanged.
- `ui_loop` reads `telemetry.beat()` each frame and threads it to `render`.

- [ ] **Step 1:** Tests: update the lane render test to open an editor, render at `beat = 0.0` and `beat = 1.0` (cycle 2 → phases 0.0/0.5) and assert the buffers differ and contain block chars; `lfo_lane_is_phase_locked` asserts `phase_at` determinism drives the head column (call `lfo_lane_line` directly at two beats, compare span styles).
- [ ] **Step 2:** Run — FAIL.
- [ ] **Step 3:** Implement rendering; delete `oscillator_lane` and the old static `automation_line`.
- [ ] **Step 4:** `cargo test` — PASS.
- [ ] **Step 5:** Commit `feat: beat-synced animated lfo lane and slider marker`

### Task 7: Docs and final verification

**Files:**
- Modify: `src/AGENTS.md:16`

- [ ] **Step 1:** Update the `automation.rs` bullet: routes hold `depth/interval/offset`, edited via the `f` submenu.
- [ ] **Step 2:** `cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check` — all clean.
- [ ] **Step 3:** Commit `docs: describe lfo submenu in module map`
