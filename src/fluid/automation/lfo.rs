//! LFO modulation routes: the shape vocabulary, the per-field spec table, the
//! route itself, and the inline `Steps` staircase.

use std::f32::consts::TAU;

use crate::fluid::Entry;
use crate::fluid::widget::DialScale;

use super::{FieldSpec, Stepping, clamped_index, morph_scalar_route, stepped_index};

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
            _ => self.spec().scale,
        }
    }

    /// Only continuous slider fields carry a numeric spec; Shape is discrete.
    fn spec(self) -> &'static FieldSpec<LfoField> {
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

const LFO_FIELD_SPECS: &[FieldSpec<LfoField>] = &[
    FieldSpec {
        field: LfoField::Amount,
        label: "amount",
        min: 0.0,
        max: 1.0,
        step: AMOUNT_STEP,
        scale: DialScale::linear(0.0, 1.0),
        stepping: Stepping::Snapped,
        entry: Entry::Percent,
        reset: 0.0,
    },
    FieldSpec {
        field: LfoField::Interval,
        label: "rate",
        min: MIN_LFO_CYCLE_BEATS,
        max: MAX_LFO_CYCLE_BEATS,
        step: INTERVAL_STEP,
        scale: DialScale::Rungs(LFO_RATE_ARROW_STEPS),
        stepping: Stepping::Ladder(LFO_RATE_ARROW_STEPS),
        entry: Entry::Free,
        reset: MIN_LFO_CYCLE_BEATS,
    },
    FieldSpec {
        field: LfoField::Offset,
        label: "offset",
        min: 0.0,
        max: MAX_LFO_OFFSET_BEATS,
        step: OFFSET_STEP,
        scale: DialScale::BeatGrid {
            min: 0.0,
            max: MAX_LFO_OFFSET_BEATS,
        },
        stepping: Stepping::BeatGrid,
        entry: Entry::Snap,
        reset: 0.0,
    },
];

pub(crate) const LFO_RATE_ARROW_STEPS: &[f32] = &[
    0.125, 0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 2.75, 3.0, 3.25, 3.5, 3.75, 4.0,
    8.0, 12.0, 16.0, 32.0, 64.0,
];

/// The inline `Steps` submenu's fields. Every `Value(i)` shares one spec —
/// each step is the same bipolar depth, only its index differs.
const STEP_FIELD_SPECS: &[FieldSpec<StepTarget>] = &[
    FieldSpec {
        field: StepTarget::Count,
        label: "steps",
        min: 1.0,
        max: MAX_LFO_STEPS as f32,
        step: 1.0,
        scale: DialScale::linear(1.0, MAX_LFO_STEPS as f32),
        stepping: Stepping::Linear,
        entry: Entry::Round,
        reset: DEFAULT_LFO_STEP_COUNT as f32,
    },
    FieldSpec {
        field: StepTarget::Glide,
        label: "glide",
        min: 0.0,
        max: 1.0,
        step: STEP_GLIDE_STEP,
        scale: DialScale::linear(0.0, 1.0),
        stepping: Stepping::Linear,
        entry: Entry::Percent,
        reset: DEFAULT_LFO_STEP_GLIDE,
    },
    FieldSpec {
        field: StepTarget::Value(0),
        label: "step",
        min: -1.0,
        max: 1.0,
        step: STEP_VALUE_STEP,
        scale: DialScale::bipolar(),
        stepping: Stepping::Linear,
        entry: Entry::Percent,
        reset: 0.0,
    },
];

impl StepTarget {
    fn spec(self) -> &'static FieldSpec<StepTarget> {
        let key = match self {
            Self::Value(_) => Self::Value(0),
            other => other,
        };
        STEP_FIELD_SPECS
            .iter()
            .find(|spec| spec.field == key)
            .expect("every step target has a spec")
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
        let next = target.spec().adjust(self.step_value(target), dir);
        self.write_step(target, next);
    }

    /// Numeric entry for a step target: count is a whole number, glide a
    /// unipolar percent, a step value a bipolar percent (`-100`..`100`).
    pub(crate) fn set_step(&mut self, target: StepTarget, value: f32) {
        self.write_step(target, target.spec().parse_value(value));
    }

    pub(crate) fn reset_step(&mut self, target: StepTarget) {
        self.write_step(target, target.spec().reset);
    }

    /// Store an already ranged value on the target its spec came from.
    fn write_step(&mut self, target: StepTarget, value: f32) {
        self.pickup = None;
        match target {
            StepTarget::Count => self.step_count = value.round() as u8,
            StepTarget::Glide => self.step_glide = value,
            StepTarget::Value(i) => {
                if let Some(step) = self.steps.get_mut(i) {
                    *step = value;
                }
            }
        }
    }

    /// How each inline staircase target maps onto bar position: the length
    /// spans the reachable step count, glide a plain unit span, and each step
    /// value is a bipolar depth like every other modulation amount.
    pub(crate) fn step_scale(target: StepTarget) -> DialScale {
        target.spec().scale
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
            LfoField::Shape => self.write_shape(self.shape.cycled(dir)),
            _ => {
                let next = field.spec().adjust(self.field_value(field), dir);
                self.write_field_at(field, next, beat);
            }
        }
    }

    pub(crate) fn set_field_at(&mut self, field: LfoField, value: f32, beat: f64) {
        match field {
            LfoField::Shape => self.write_shape(LfoShape::from_index(value)),
            _ => self.write_field_at(field, field.spec().parse_value(value), beat),
        }
    }

    /// Set a time field to an exact value, clamped to range but not snapped
    /// to the beat grid — used while the field is being driven in ms.
    pub(crate) fn set_field_raw_at(&mut self, field: LfoField, value: f32, beat: f64) {
        match field {
            LfoField::Interval | LfoField::Offset => {
                let spec = field.spec();
                self.write_field_at(field, value.clamp(spec.min, spec.max), beat);
            }
            _ => self.set_field_at(field, value, beat),
        }
    }

    pub(crate) fn reset_field_at(&mut self, field: LfoField, beat: f64) {
        match field {
            LfoField::Shape => self.write_shape(LfoShape::Sine),
            _ => self.write_field_at(field, field.spec().reset, beat),
        }
    }

    /// Store an already ranged value on the field its spec came from. A rate
    /// change hands the old waveform over at its next crossing instead of
    /// rewriting the offset; every other edit drops a pending handoff. Shape
    /// is discrete and goes through `write_shape` instead.
    fn write_field_at(&mut self, field: LfoField, value: f32, beat: f64) {
        match field {
            LfoField::Amount => self.depth_ratio = value,
            LfoField::Interval => self.set_cycle_with_pickup(value, beat),
            LfoField::Offset => {
                self.phase_offset_beats = value;
                self.pickup = None;
            }
            LfoField::Shape => {}
        }
    }

    fn write_shape(&mut self, shape: LfoShape) {
        self.shape = shape;
        self.pickup = None;
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
