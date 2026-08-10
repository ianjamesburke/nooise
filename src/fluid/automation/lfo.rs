//! LFO modulation routes: the shape vocabulary, the per-field spec table, the
//! route itself, and the inline `Steps` staircase.

use std::f32::consts::TAU;

use crate::fluid::widget::DialScale;
use crate::fluid::{Entry, beat_grid_adjust, beat_grid_snap, normalize_unit_input, snap_step};

use super::{clamped_index, morph_scalar_route, stepped_index};

pub(crate) const DEFAULT_LFO_CYCLE_BEATS: f32 = 2.0;
pub(crate) const DEFAULT_LFO_DEPTH_RATIO: f32 = 0.0;
pub(crate) const MIN_LFO_CYCLE_BEATS: f32 = 0.125;
pub(crate) const MAX_LFO_CYCLE_BEATS: f32 = 64.0;
pub(crate) const MAX_LFO_OFFSET_BEATS: f32 = 4.0;

/// Upper bound on a `Steps` shape's custom automation sequence. Fixed so
/// `LfoRoute` stays `Copy` (a `[f32; MAX_LFO_STEPS]` array, no allocation on
/// the audio thread).
pub(crate) const MAX_LFO_STEPS: usize = 16;
pub(crate) const DEFAULT_LFO_STEP_COUNT: u8 = 4;
/// Default edge-glide: a slight slide into each step so the staircase doesn't
/// click on a live-read control (see `step_value_at`). 0 = hard steps.
pub(crate) const DEFAULT_LFO_STEP_GLIDE: f32 = 0.15;
/// Default `Steps` pattern: three neutral steps then a full up-step, so a
/// fresh Steps shape reads as a rhythmic accent on the last beat.
const DEFAULT_LFO_STEPS: [f32; MAX_LFO_STEPS] = {
    let mut steps = [0.0f32; MAX_LFO_STEPS];
    steps[3] = 1.0;
    steps
};

const AMOUNT_STEP: f32 = 0.01;
const INTERVAL_STEP: f32 = 0.125;
const OFFSET_STEP: f32 = 0.125;
const STEP_VALUE_STEP: f32 = 0.05;
const STEP_GLIDE_STEP: f32 = 0.05;

/// Softness of the smoothed square edge; higher = closer to a hard square.
const SQUARE_SMOOTH: f32 = 6.0;

/// Fraction of a ramp's cycle, right before it wraps, eased toward the next
/// cycle's start value instead of jumping there in a single sample. Every
/// other shape is continuous at the wrap already (sine and triangle by
/// construction, square via SQUARE_SMOOTH); a bare ramp is a sawtooth with a
/// full-swing discontinuity every cycle, which clicks when applied straight
/// to a live-read control like level or cutoff.
const RAMP_WRAP_EASE: f32 = 0.02;
const PICKUP_SCAN_STEP_BEATS: f64 = 1.0 / 256.0;
const PICKUP_CROSSING_EPSILON: f32 = 1e-4;
// ============================================================
// LFO shapes
// ============================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LfoShape {
    Sine,
    Triangle,
    RampUp,
    RampDown,
    Square,
    RandomDrift,
    SampleHold,
    /// User-drawn staircase: a per-cycle sequence of `step_count` bipolar
    /// values on `LfoRoute`, edited in the Shape row's inline step submenu.
    Steps,
}

impl LfoShape {
    pub(crate) const ALL: [LfoShape; 8] = [
        Self::Sine,
        Self::Triangle,
        Self::RampUp,
        Self::RampDown,
        Self::Square,
        Self::RandomDrift,
        Self::SampleHold,
        Self::Steps,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Sine => "sine",
            Self::Triangle => "triangle",
            Self::RampUp => "ramp up",
            Self::RampDown => "ramp down",
            Self::Square => "square",
            Self::RandomDrift => "random drift",
            Self::SampleHold => "sample & hold",
            Self::Steps => "steps",
        }
    }

    /// Random shapes generate their trajectory from the route seed instead of a
    /// fixed periodic curve, so the animated lane must scope them differently.
    pub(crate) fn is_random(self) -> bool {
        matches!(self, Self::RandomDrift | Self::SampleHold)
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|&s| s == self).unwrap_or(0)
    }

    fn cycled(self, dir: f32) -> Self {
        Self::ALL[stepped_index(self.index(), dir, Self::ALL.len())]
    }

    fn from_index(index: f32) -> Self {
        Self::ALL[clamped_index(index, Self::ALL.len())]
    }
}

/// Deterministic per-index value in -1..1, keyed by the route seed. Pure hash,
/// no RNG state, so the UI and engine agree and offline renders stay identical.
fn seeded_unit(seed: u32, index: i64) -> f32 {
    let mut z = (index as u64)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(u64::from(seed))
        .wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    let unit = (z >> 40) as f32 / f32::from(1u16 << 8) / f32::from(1u16 << 8) / 256.0;
    unit * 2.0 - 1.0
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Blends a ramp's raw value toward `next_cycle_start` over the last
/// `RAMP_WRAP_EASE` fraction of the cycle, so the value at phase 1 (== the
/// next cycle's phase 0) is reached smoothly instead of jumping there.
fn ease_ramp_wrap(phase: f32, raw: f32, next_cycle_start: f32) -> f32 {
    let window_start = 1.0 - RAMP_WRAP_EASE;
    if phase < window_start {
        return raw;
    }
    let t = smoothstep((phase - window_start) / RAMP_WRAP_EASE);
    raw + (next_cycle_start - raw) * t
}

/// Periodic shape value in -1..1 for a phase in 0..1. Random shapes return 0
/// here; they are evaluated from absolute beat position in `wave_at`.
fn periodic_shape_value(shape: LfoShape, phase: f32) -> f32 {
    match shape {
        LfoShape::Sine => (TAU * phase).sin(),
        LfoShape::Triangle => {
            if phase < 0.25 {
                4.0 * phase
            } else if phase < 0.75 {
                1.0 - 4.0 * (phase - 0.25)
            } else {
                -1.0 + 4.0 * (phase - 0.75)
            }
        }
        LfoShape::RampUp => ease_ramp_wrap(phase, 2.0 * phase - 1.0, -1.0),
        LfoShape::RampDown => ease_ramp_wrap(phase, 1.0 - 2.0 * phase, 1.0),
        LfoShape::Square => (SQUARE_SMOOTH * (TAU * phase).sin()).tanh(),
        // Random and Steps shapes are route-dependent: evaluated from seed or
        // the custom step array in `wave_at`/`step_value_at`, not from phase alone.
        LfoShape::RandomDrift | LfoShape::SampleHold | LfoShape::Steps => 0.0,
    }
}

/// Deterministic FNV-1a hash so each control's random modulator starts from an
/// independent seed without persisting per-route state.
pub(super) fn seed_for_id(id: &str) -> u32 {
    let mut hash = 0x811C_9DC5u32;
    for byte in id.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LfoField {
    Amount,
    Interval,
    Offset,
    Shape,
}

impl LfoField {
    pub(crate) const ALL: [LfoField; 4] = [Self::Amount, Self::Interval, Self::Offset, Self::Shape];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Shape => "shape",
            _ => self.spec().label,
        }
    }

    /// How this field maps onto bar position. Shape is a discrete enum and
    /// carries no numeric spec, so it spans the bar by variant index.
    pub(crate) fn scale(self) -> DialScale {
        match self {
            Self::Shape => DialScale::enumerated(LfoShape::ALL.len()),
            _ => self.spec().scale(),
        }
    }

    /// Only continuous slider fields carry a numeric spec; Shape is discrete.
    fn spec(self) -> &'static LfoFieldSpec {
        LFO_FIELD_SPECS
            .iter()
            .find(|spec| spec.field == self)
            .expect("every continuous LFO field has a spec")
    }

    /// Stable key qualifier for a field a macro can stack onto (see
    /// `AutomationState::field_macros`); None for Shape, which is discrete.
    /// Only meaningful on regular controls — a macro slider's own LFO never
    /// takes a stacked macro (no macro chasing itself).
    pub(crate) fn macro_key(self) -> Option<&'static str> {
        match self {
            Self::Amount => Some("lfo.amount"),
            Self::Interval => Some("lfo.interval"),
            Self::Offset => Some("lfo.offset"),
            Self::Shape => None,
        }
    }
}

/// One editable target inside a `Steps` shape's inline submenu: the sequence
/// length, the shared edge-glide, or one bipolar step value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StepTarget {
    Count,
    Glide,
    Value(usize),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LfoFieldSpec {
    pub(crate) field: LfoField,
    pub(crate) label: &'static str,
    pub(crate) min: f32,
    pub(crate) max: f32,
    pub(crate) step: f32,
    pub(crate) entry: Entry,
    pub(crate) reset: f32,
    /// Interval-like fields lock to the musical beat grid (0.125 floor,
    /// sixteenths above) instead of a fixed linear step.
    pub(crate) beat_grid: bool,
}

impl LfoFieldSpec {
    pub(crate) fn adjust(self, value: f32, dir: f32) -> f32 {
        if self.field == LfoField::Interval {
            lfo_rate_adjust(value, dir)
        } else if self.beat_grid {
            beat_grid_adjust(value, dir, self.min, self.max)
        } else {
            self.quantize(value + dir * self.step)
        }
    }

    pub(crate) fn parse_value(self, value: f32) -> f32 {
        match self.entry {
            Entry::Percent => normalize_unit_input(value).clamp(self.min, self.max),
            Entry::Snap => self.quantize(value),
            Entry::Round => value.round().clamp(self.min, self.max),
            // No automation field is stored in bars, so BeatsAsBars carries
            // no extra meaning here and reads as a plain exact value.
            Entry::BeatsAsBars | Entry::Free => value.clamp(self.min, self.max),
        }
    }

    pub(crate) fn quantize(self, value: f32) -> f32 {
        if self.beat_grid {
            beat_grid_snap(value, self.min, self.max)
        } else {
            snap_step(value.clamp(self.min, self.max), self.step).clamp(self.min, self.max)
        }
    }

    /// Rate rides its own arrow ladder, other beat-grid fields the musical
    /// grid, everything else a plain linear span.
    pub(crate) fn scale(self) -> DialScale {
        if self.field == LfoField::Interval {
            DialScale::Rungs(LFO_RATE_ARROW_STEPS)
        } else if self.beat_grid {
            DialScale::BeatGrid {
                min: self.min,
                max: self.max,
            }
        } else {
            DialScale::linear(self.min, self.max)
        }
    }
}

pub(crate) const LFO_FIELD_SPECS: &[LfoFieldSpec] = &[
    LfoFieldSpec {
        field: LfoField::Amount,
        label: "amount",
        min: 0.0,
        max: 1.0,
        step: AMOUNT_STEP,
        entry: Entry::Percent,
        reset: 0.0,
        beat_grid: false,
    },
    LfoFieldSpec {
        field: LfoField::Interval,
        label: "rate",
        min: MIN_LFO_CYCLE_BEATS,
        max: MAX_LFO_CYCLE_BEATS,
        step: INTERVAL_STEP,
        entry: Entry::Free,
        reset: MIN_LFO_CYCLE_BEATS,
        beat_grid: true,
    },
    LfoFieldSpec {
        field: LfoField::Offset,
        label: "offset",
        min: 0.0,
        max: MAX_LFO_OFFSET_BEATS,
        step: OFFSET_STEP,
        entry: Entry::Snap,
        reset: 0.0,
        beat_grid: true,
    },
];

pub(crate) const LFO_RATE_ARROW_STEPS: &[f32] = &[
    0.125, 0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 2.75, 3.0, 3.25, 3.5, 3.75, 4.0,
    8.0, 12.0, 16.0, 32.0, 64.0,
];

fn lfo_rate_adjust(value: f32, dir: f32) -> f32 {
    if dir > 0.0 {
        LFO_RATE_ARROW_STEPS
            .iter()
            .copied()
            .find(|step| *step > value + f32::EPSILON)
            .unwrap_or(MAX_LFO_CYCLE_BEATS)
    } else {
        LFO_RATE_ARROW_STEPS
            .iter()
            .rev()
            .copied()
            .find(|step| *step < value - f32::EPSILON)
            .unwrap_or(MIN_LFO_CYCLE_BEATS)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LfoPickup {
    pub(crate) from_cycle_beats: f32,
    pub(crate) at_beat: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LfoRoute {
    pub(crate) depth_ratio: f32,
    pub(crate) cycle_beats: f32,
    pub(crate) phase_offset_beats: f32,
    pub(crate) shape: LfoShape,
    /// Seed for random shapes; hashed with the cycle index to produce values.
    pub(crate) seed: u32,
    /// Custom staircase for `LfoShape::Steps`; only the first `step_count`
    /// entries are live. Bipolar (-1..1), inert unless the shape is `Steps`.
    /// Each step spans one LFO interval (`cycle_beats`), so the full pattern
    /// lasts `step_count` intervals — raising the count extends the pattern.
    pub(crate) steps: [f32; MAX_LFO_STEPS],
    pub(crate) step_count: u8,
    /// Edge-glide fraction (0..1): how much of each step window eases in from
    /// the previous step's value instead of holding flat. See `step_value_at`.
    pub(crate) step_glide: f32,
    /// Transient handoff after a live rate edit. The old globally anchored
    /// clock keeps playing until it next crosses the new clock, then the new
    /// rate takes over without rewriting the user's offset.
    pub(crate) pickup: Option<LfoPickup>,
}

impl Default for LfoRoute {
    fn default() -> Self {
        Self {
            depth_ratio: DEFAULT_LFO_DEPTH_RATIO,
            cycle_beats: DEFAULT_LFO_CYCLE_BEATS,
            phase_offset_beats: 0.0,
            shape: LfoShape::Sine,
            seed: 0,
            steps: DEFAULT_LFO_STEPS,
            step_count: DEFAULT_LFO_STEP_COUNT,
            step_glide: DEFAULT_LFO_STEP_GLIDE,
            pickup: None,
        }
    }
}

impl LfoRoute {
    pub(crate) fn with_seed(seed: u32) -> Self {
        Self {
            seed,
            ..Self::default()
        }
    }

    pub(crate) fn phase_at(&self, beat: f64) -> f64 {
        self.phase_at_cycle(beat, self.active_cycle_at(beat))
    }

    fn phase_at_cycle(&self, beat: f64, cycle_beats: f32) -> f64 {
        global_lfo_position(beat, cycle_beats, self.phase_offset_beats).1
    }

    fn active_cycle_at(&self, beat: f64) -> f32 {
        self.pickup
            .filter(|pickup| beat < pickup.at_beat)
            .map_or(self.cycle_beats, |pickup| pickup.from_cycle_beats)
    }

    /// Absolute cycle index and phase-in-cycle for the given beat. Random shapes
    /// hash the cycle index; the fractional part doubles as the periodic phase.
    fn cycle_index_and_phase_for(&self, beat: f64, cycle_beats: f32) -> (i64, f32) {
        let (index, phase) = global_lfo_position(beat, cycle_beats, self.phase_offset_beats);
        (index, phase as f32)
    }

    /// Oscillator output in -1..1 at the given beat; depth scaling is the
    /// caller's job. Single source of truth for both the engine and the lane.
    pub(crate) fn wave_at(&self, beat: f64) -> f32 {
        self.wave_at_cycle(beat, self.active_cycle_at(beat))
    }

    fn wave_at_cycle(&self, beat: f64, cycle_beats: f32) -> f32 {
        let (index, phase) = self.cycle_index_and_phase_for(beat, cycle_beats);
        match self.shape {
            LfoShape::SampleHold => seeded_unit(self.seed, index),
            LfoShape::RandomDrift => {
                let a = seeded_unit(self.seed, index);
                let b = seeded_unit(self.seed, index + 1);
                a + (b - a) * smoothstep(phase)
            }
            LfoShape::Steps => {
                let count = self.active_step_count();
                self.step_value_at(index.rem_euclid(count as i64) as usize, phase)
            }
            shape => periodic_shape_value(shape, phase),
        }
    }

    /// Live step count, clamped into the valid `1..=MAX_LFO_STEPS` range.
    pub(crate) fn active_step_count(&self) -> usize {
        (self.step_count as usize).clamp(1, MAX_LFO_STEPS)
    }

    /// Staircase value in -1..1 for step `idx` at fraction `frac` (0..1)
    /// through that step. Each step spans one LFO interval and holds its
    /// value, easing in from the previous step's value over the first
    /// `step_glide` fraction of the step. Because step 0 eases from the last
    /// step, the curve is value-continuous across the pattern wrap too (at
    /// `step_glide` 0 it hard-steps and clicks, same as sample & hold).
    fn step_value_at(&self, idx: usize, frac: f32) -> f32 {
        let count = self.active_step_count();
        let idx = idx.min(count - 1);
        let cur = self.steps[idx];
        let glide = self.step_glide.clamp(0.0, 1.0);
        if glide <= f32::EPSILON || frac >= glide {
            return cur;
        }
        let prev = self.steps[(idx + count - 1) % count];
        prev + (cur - prev) * smoothstep(frac / glide)
    }

    /// Periodic shape value in -1..1 at a phase in 0..1, for lane drawing.
    /// For `Steps` the phase spans the whole pattern (`step_count` intervals),
    /// matching `pattern_phase_at`. Random shapes return 0 here; draw them
    /// from `wave_at` over time instead.
    pub(crate) fn shape_value_at_phase(&self, phase: f32) -> f32 {
        match self.shape {
            LfoShape::Steps => {
                let count = self.active_step_count();
                let scaled = phase.rem_euclid(1.0) * count as f32;
                let idx = (scaled.floor() as usize).min(count - 1);
                self.step_value_at(idx, scaled - idx as f32)
            }
            _ => periodic_shape_value(self.shape, phase),
        }
    }

    /// Phase of the full drawn pattern at `beat`: one interval for periodic
    /// shapes, `step_count` intervals for `Steps` (each step spans one
    /// interval). Drives the lane's bright head so it tracks what plays.
    pub(crate) fn pattern_phase_at(&self, beat: f64) -> f64 {
        match self.shape {
            LfoShape::Steps => {
                let cycle = f64::from(self.active_cycle_at(beat).max(MIN_LFO_CYCLE_BEATS));
                let t = (beat + f64::from(self.phase_offset_beats)) / cycle;
                (t / self.active_step_count() as f64).rem_euclid(1.0)
            }
            _ => self.phase_at(beat),
        }
    }

    /// Adjust a step submenu target by one h/l press.
    pub(crate) fn adjust_step(&mut self, target: StepTarget, dir: f32) {
        self.pickup = None;
        match target {
            StepTarget::Count => {
                self.set_step_count(self.step_count as i32 + dir.signum() as i32);
            }
            StepTarget::Glide => {
                self.step_glide = (self.step_glide + dir * STEP_GLIDE_STEP).clamp(0.0, 1.0);
            }
            StepTarget::Value(i) => {
                if let Some(value) = self.steps.get_mut(i) {
                    *value = (*value + dir * STEP_VALUE_STEP).clamp(-1.0, 1.0);
                }
            }
        }
    }

    /// Numeric entry for a step target: count is a whole number, glide a
    /// unipolar percent, a step value a bipolar percent (`-100`..`100`).
    pub(crate) fn set_step(&mut self, target: StepTarget, value: f32) {
        self.pickup = None;
        match target {
            StepTarget::Count => self.set_step_count(value.round() as i32),
            StepTarget::Glide => self.step_glide = (value / 100.0).clamp(0.0, 1.0),
            StepTarget::Value(i) => {
                if let Some(step) = self.steps.get_mut(i) {
                    *step = (value / 100.0).clamp(-1.0, 1.0);
                }
            }
        }
    }

    pub(crate) fn reset_step(&mut self, target: StepTarget) {
        self.pickup = None;
        match target {
            StepTarget::Count => self.step_count = DEFAULT_LFO_STEP_COUNT,
            StepTarget::Glide => self.step_glide = DEFAULT_LFO_STEP_GLIDE,
            StepTarget::Value(i) => {
                if let Some(step) = self.steps.get_mut(i) {
                    *step = 0.0;
                }
            }
        }
    }

    fn set_step_count(&mut self, count: i32) {
        self.step_count = count.clamp(1, MAX_LFO_STEPS as i32) as u8;
    }

    /// How each inline staircase target maps onto bar position: the length
    /// spans the reachable step count, glide a plain unit span, and each step
    /// value is a bipolar depth like every other modulation amount.
    pub(crate) fn step_scale(target: StepTarget) -> DialScale {
        match target {
            StepTarget::Count => DialScale::linear(1.0, MAX_LFO_STEPS as f32),
            StepTarget::Glide => DialScale::linear(0.0, 1.0),
            StepTarget::Value(_) => DialScale::bipolar(),
        }
    }

    pub(crate) fn step_value(&self, target: StepTarget) -> f32 {
        match target {
            StepTarget::Count => self.active_step_count() as f32,
            StepTarget::Glide => self.step_glide,
            StepTarget::Value(i) => self.steps.get(i).copied().unwrap_or(0.0),
        }
    }

    pub(crate) fn step_display(&self, target: StepTarget) -> String {
        match target {
            StepTarget::Count => format!("{}", self.active_step_count()),
            StepTarget::Glide => format!("{:.0}%", self.step_glide * 100.0),
            StepTarget::Value(i) => {
                format!("{:+.0}%", self.steps.get(i).copied().unwrap_or(0.0) * 100.0)
            }
        }
    }

    pub(crate) fn step_label(&self, target: StepTarget) -> String {
        match target {
            StepTarget::Count => "steps".to_string(),
            StepTarget::Glide => "glide".to_string(),
            StepTarget::Value(i) => format!("· step {}", i + 1),
        }
    }

    /// Re-roll the random seed to a new but repeatable pattern.
    pub(crate) fn reseed(&mut self) {
        self.seed = self
            .seed
            .wrapping_mul(1_664_525)
            .wrapping_add(1_013_904_223)
            ^ 0x5DEE_CE66;
    }

    pub(crate) fn adjust_field_at(&mut self, field: LfoField, dir: f32, beat: f64) {
        match field {
            LfoField::Shape => {
                self.shape = self.shape.cycled(dir);
                self.pickup = None;
            }
            LfoField::Amount => {
                self.depth_ratio = field.spec().adjust(self.depth_ratio, dir);
            }
            LfoField::Interval => {
                self.set_cycle_with_pickup(field.spec().adjust(self.cycle_beats, dir), beat);
            }
            LfoField::Offset => {
                self.phase_offset_beats = field.spec().adjust(self.phase_offset_beats, dir);
                self.pickup = None;
            }
        }
    }

    pub(crate) fn set_field_at(&mut self, field: LfoField, value: f32, beat: f64) {
        match field {
            LfoField::Shape => {
                self.shape = LfoShape::from_index(value);
                self.pickup = None;
            }
            LfoField::Amount => self.depth_ratio = field.spec().parse_value(value),
            LfoField::Interval => {
                self.set_cycle_with_pickup(field.spec().parse_value(value), beat);
            }
            LfoField::Offset => {
                self.phase_offset_beats = field.spec().parse_value(value);
                self.pickup = None;
            }
        }
    }

    /// Set a time field to an exact value, clamped to range but not snapped
    /// to the beat grid — used while the field is being driven in ms.
    pub(crate) fn set_field_raw_at(&mut self, field: LfoField, value: f32, beat: f64) {
        match field {
            LfoField::Interval => self
                .set_cycle_with_pickup(value.clamp(MIN_LFO_CYCLE_BEATS, MAX_LFO_CYCLE_BEATS), beat),
            LfoField::Offset => {
                self.phase_offset_beats = value.clamp(0.0, MAX_LFO_OFFSET_BEATS);
                self.pickup = None;
            }
            _ => self.set_field_at(field, value, beat),
        }
    }

    pub(crate) fn reset_field_at(&mut self, field: LfoField, beat: f64) {
        match field {
            LfoField::Shape => {
                self.shape = LfoShape::Sine;
                self.pickup = None;
            }
            LfoField::Amount => self.depth_ratio = field.spec().reset,
            LfoField::Interval => self.set_cycle_with_pickup(field.spec().reset, beat),
            LfoField::Offset => {
                self.phase_offset_beats = field.spec().reset;
                self.pickup = None;
            }
        }
    }

    /// The field's value in the units its own scale is expressed in.
    pub(crate) fn field_value(&self, field: LfoField) -> f32 {
        match field {
            LfoField::Shape => self.shape.index() as f32,
            LfoField::Amount => self.depth_ratio,
            LfoField::Interval => self.cycle_beats,
            LfoField::Offset => self.phase_offset_beats,
        }
    }

    pub(crate) fn field_display(&self, field: LfoField) -> String {
        match field {
            LfoField::Shape => self.shape.label().to_string(),
            LfoField::Amount => format!("{:.0}%", self.depth_ratio * 100.0),
            LfoField::Interval => format!("{:.2} beats", self.cycle_beats),
            LfoField::Offset => format!("{:.2} beats", self.phase_offset_beats),
        }
    }

    fn set_cycle_with_pickup(&mut self, cycle_beats: f32, beat: f64) {
        let old_cycle = self.active_cycle_at(beat);
        if (old_cycle - cycle_beats).abs() <= f32::EPSILON {
            self.cycle_beats = cycle_beats;
            self.pickup = None;
            return;
        }
        self.cycle_beats = cycle_beats;
        self.pickup = next_wave_crossing(self, old_cycle, cycle_beats, beat).and_then(|at_beat| {
            (at_beat > beat + f64::EPSILON).then_some(LfoPickup {
                from_cycle_beats: old_cycle,
                at_beat,
            })
        });
    }

    /// Morph an optional route on each side of a leg transition: `depth_ratio`
    /// glides by `tt` (0..1, matching `ControlKind::Gain`'s treatment of
    /// every other slider), every other field snaps together to `to`'s value
    /// once `use_to` flips true, matching `ControlKind::Discrete`'s
    /// structural-snap treatment. A route missing on one side glides its
    /// depth to/from 0 while holding the present side's other fields — it
    /// fades in or out rather than popping, and naturally disappears once the
    /// leg's `to` state becomes the next leg's `from`. See `morph_scalar_route`
    /// for the shared 4-arm glide/snap logic.
    pub(super) fn morph(
        from: Option<&LfoRoute>,
        to: Option<&LfoRoute>,
        tt: f32,
        use_to: bool,
    ) -> Option<LfoRoute> {
        morph_scalar_route(
            from,
            to,
            tt,
            use_to,
            |r| r.depth_ratio,
            |r, v| r.depth_ratio = v,
        )
    }
}

/// Every LFO derives position from the shared transport beat. A zero offset
/// therefore puts every route with the same rate on the same song-wide grid,
/// regardless of which control owns it or when its editor was opened.
fn global_lfo_position(beat: f64, cycle_beats: f32, offset_beats: f32) -> (i64, f64) {
    let cycle = f64::from(cycle_beats.max(MIN_LFO_CYCLE_BEATS));
    let position = (beat + f64::from(offset_beats)) / cycle;
    let index = position.floor();
    (index as i64, position - index)
}
fn next_wave_crossing(
    route: &LfoRoute,
    old_cycle_beats: f32,
    new_cycle_beats: f32,
    beat: f64,
) -> Option<f64> {
    let delta_at =
        |at| route.wave_at_cycle(at, old_cycle_beats) - route.wave_at_cycle(at, new_cycle_beats);
    let mut previous_beat = beat;
    let mut previous_delta = delta_at(beat);
    if previous_delta.abs() <= PICKUP_CROSSING_EPSILON {
        return Some(beat);
    }

    let horizon = f64::from(old_cycle_beats.max(new_cycle_beats)) * 2.0;
    let steps = (horizon / PICKUP_SCAN_STEP_BEATS).ceil() as usize;
    let mut closest = (previous_delta.abs(), beat);
    for step in 1..=steps {
        let next_beat = beat + step as f64 * PICKUP_SCAN_STEP_BEATS;
        let next_delta = delta_at(next_beat);
        if next_delta.abs() < closest.0 {
            closest = (next_delta.abs(), next_beat);
        }
        if next_delta.abs() <= PICKUP_CROSSING_EPSILON {
            return Some(next_beat);
        }
        if previous_delta.signum() != next_delta.signum() {
            let mut lo = previous_beat;
            let mut hi = next_beat;
            let lo_sign = previous_delta.signum();
            for _ in 0..20 {
                let mid = (lo + hi) * 0.5;
                if delta_at(mid).signum() == lo_sign {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            return Some((lo + hi) * 0.5);
        }
        previous_beat = next_beat;
        previous_delta = next_delta;
    }

    // Discrete/random shapes may not have a literal continuous crossing.
    // Hand off at their closest encounter within two cycles instead.
    Some(closest.1)
}
