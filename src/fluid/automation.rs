use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::f32::consts::TAU;
use std::fmt;

use super::{
    ControlSpec, FluidControls, LfoSnap, MACRO_CONTROLS, MACRO_COUNT, TimingContext,
    beat_grid_adjust, beat_grid_snap, is_macro_id, nearest_power_of_two, normalize_unit_input,
    snap_step, spec_by_id, unit_key,
};

pub(crate) const DEFAULT_LFO_CYCLE_BEATS: f32 = 2.0;
pub(crate) const DEFAULT_LFO_DEPTH_RATIO: f32 = 0.0;
pub(crate) const MIN_LFO_CYCLE_BEATS: f32 = 0.125;
pub(crate) const MAX_LFO_CYCLE_BEATS: f32 = 16.0;
pub(crate) const MAX_LFO_OFFSET_BEATS: f32 = 4.0;

const AMOUNT_STEP: f32 = 0.01;
const INTERVAL_STEP: f32 = 0.125;
const OFFSET_STEP: f32 = 0.125;

/// Softness of the smoothed square edge; higher = closer to a hard square.
const SQUARE_SMOOTH: f32 = 6.0;

/// Fraction of a ramp's cycle, right before it wraps, eased toward the next
/// cycle's start value instead of jumping there in a single sample. Every
/// other shape is continuous at the wrap already (sine and triangle by
/// construction, square via SQUARE_SMOOTH); a bare ramp is a sawtooth with a
/// full-swing discontinuity every cycle, which clicks when applied straight
/// to a live-read control like level or cutoff.
const RAMP_WRAP_EASE: f32 = 0.02;

// Envelope route field ranges. Attack/decay reach into the minutes at slow
// tempos (512 beats is ~6 min at 82 BPM, ~12 min at 40 BPM) so the same
// one-shot serves both fast swells and set-and-forget macro blooms.
pub(crate) const MAX_ENV_ATTACK_BEATS: f32 = 512.0;
pub(crate) const MAX_ENV_DECAY_BEATS: f32 = 512.0;
const ENV_BEATS_STEP: f32 = 0.5;
const ENV_AMOUNT_STEP: f32 = 0.01;
const DEFAULT_ENV_ATTACK_BEATS: f32 = 1.0;
const DEFAULT_ENV_DECAY_BEATS: f32 = 4.0;

#[derive(Clone, Copy)]
pub(crate) struct ControlAddress {
    spec: &'static ControlSpec,
}

impl ControlAddress {
    pub(crate) fn new(id: &'static str) -> Self {
        let spec = spec_by_id(id).expect("control address must reference a registered control");
        Self { spec }
    }

    pub(crate) fn id(self) -> &'static str {
        self.spec.id
    }

    pub(crate) fn spec(self) -> &'static ControlSpec {
        self.spec
    }
}

impl fmt::Debug for ControlAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ControlAddress").field(&self.id()).finish()
    }
}

impl PartialEq for ControlAddress {
    fn eq(&self, other: &Self) -> bool {
        self.id() == other.id()
    }
}

impl Eq for ControlAddress {}

impl Ord for ControlAddress {
    fn cmp(&self, other: &Self) -> Ordering {
        self.id().cmp(other.id())
    }
}

impl PartialOrd for ControlAddress {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Which modulator editor is currently open on a control. LFO, envelope, and
/// macro routes are independent siblings that can all live on one control
/// (envelopes only on macro sliders, macro routes only on regular controls).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModKind {
    Lfo,
    Envelope,
    Macro,
}

/// Sampling context shared by every modulator so the UI marker and the engine
/// value come from the same math. `kick_*` describe the live kick grid, which
/// the on-kick envelope trigger reconstructs deterministically.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ModContext {
    pub(crate) beat: f64,
    pub(crate) kick_interval_beats: f32,
    pub(crate) kick_offset_beats: f32,
}

impl ModContext {
    /// Context for an LFO-only evaluation; the kick fields are unused because
    /// no LFO shape depends on the kick grid.
    #[cfg(test)]
    pub(crate) fn lfo_only(beat: f64) -> Self {
        Self {
            beat,
            kick_interval_beats: 1.0,
            kick_offset_beats: 0.0,
        }
    }
}

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
}

impl LfoShape {
    pub(crate) const ALL: [LfoShape; 7] = [
        Self::Sine,
        Self::Triangle,
        Self::RampUp,
        Self::RampDown,
        Self::Square,
        Self::RandomDrift,
        Self::SampleHold,
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

/// Step a discrete field's index without wrapping: h/l stop at the ends, the
/// baseline behaviour for every slider-like field.
pub(crate) fn stepped_index(index: usize, dir: f32, len: usize) -> usize {
    let next = index as i64 + i64::from(dir.signum() as i32);
    next.clamp(0, len.saturating_sub(1) as i64) as usize
}

/// Numeric entry for a discrete field: round and clamp to the valid range.
pub(crate) fn clamped_index(index: f32, len: usize) -> usize {
    (index.round() as i64).clamp(0, len.saturating_sub(1) as i64) as usize
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
        LfoShape::RandomDrift | LfoShape::SampleHold => 0.0,
    }
}

/// Deterministic FNV-1a hash so each control's random modulator starts from an
/// independent seed without persisting per-route state.
fn seed_for_id(id: &str) -> u32 {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LfoEntry {
    Percent,
    Snap,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LfoFieldSpec {
    pub(crate) field: LfoField,
    pub(crate) label: &'static str,
    pub(crate) min: f32,
    pub(crate) max: f32,
    pub(crate) step: f32,
    pub(crate) entry: LfoEntry,
    pub(crate) reset: f32,
    /// Interval-like fields lock to the musical beat grid (0.125 floor,
    /// sixteenths above) instead of a fixed linear step.
    pub(crate) beat_grid: bool,
}

impl LfoFieldSpec {
    pub(crate) fn adjust(self, value: f32, dir: f32) -> f32 {
        if self.beat_grid {
            beat_grid_adjust(value, dir, self.min, self.max)
        } else {
            self.quantize(value + dir * self.step)
        }
    }

    pub(crate) fn parse_value(self, value: f32) -> f32 {
        match self.entry {
            LfoEntry::Percent => normalize_unit_input(value).clamp(self.min, self.max),
            LfoEntry::Snap => self.quantize(value),
        }
    }

    pub(crate) fn quantize(self, value: f32) -> f32 {
        if self.beat_grid {
            beat_grid_snap(value, self.min, self.max)
        } else {
            snap_step(value.clamp(self.min, self.max), self.step).clamp(self.min, self.max)
        }
    }

    pub(crate) fn ratio(self, value: f32) -> f32 {
        let range = self.max - self.min;
        if range.abs() <= f32::EPSILON {
            0.0
        } else {
            ((value - self.min) / range).clamp(0.0, 1.0)
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
        entry: LfoEntry::Percent,
        reset: 0.0,
        beat_grid: false,
    },
    LfoFieldSpec {
        field: LfoField::Interval,
        label: "interval",
        min: MIN_LFO_CYCLE_BEATS,
        max: MAX_LFO_CYCLE_BEATS,
        step: INTERVAL_STEP,
        entry: LfoEntry::Snap,
        reset: MIN_LFO_CYCLE_BEATS,
        beat_grid: true,
    },
    LfoFieldSpec {
        field: LfoField::Offset,
        label: "offset",
        min: 0.0,
        max: MAX_LFO_OFFSET_BEATS,
        step: OFFSET_STEP,
        entry: LfoEntry::Snap,
        reset: 0.0,
        beat_grid: true,
    },
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LfoRoute {
    pub(crate) depth_ratio: f32,
    pub(crate) cycle_beats: f32,
    pub(crate) phase_offset_beats: f32,
    pub(crate) shape: LfoShape,
    /// Seed for random shapes; hashed with the cycle index to produce values.
    pub(crate) seed: u32,
}

impl Default for LfoRoute {
    fn default() -> Self {
        Self {
            depth_ratio: DEFAULT_LFO_DEPTH_RATIO,
            cycle_beats: DEFAULT_LFO_CYCLE_BEATS,
            phase_offset_beats: 0.0,
            shape: LfoShape::Sine,
            seed: 0,
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
        ((beat + f64::from(self.phase_offset_beats))
            / f64::from(self.cycle_beats.max(MIN_LFO_CYCLE_BEATS)))
        .rem_euclid(1.0)
    }

    /// Absolute cycle index and phase-in-cycle for the given beat. Random shapes
    /// hash the cycle index; the fractional part doubles as the periodic phase.
    fn cycle_index_and_phase(&self, beat: f64) -> (i64, f32) {
        let cycle = f64::from(self.cycle_beats.max(MIN_LFO_CYCLE_BEATS));
        let t = (beat + f64::from(self.phase_offset_beats)) / cycle;
        let index = t.floor();
        ((index as i64), (t - index) as f32)
    }

    /// Oscillator output in -1..1 at the given beat; depth scaling is the
    /// caller's job. Single source of truth for both the engine and the lane.
    pub(crate) fn wave_at(&self, beat: f64) -> f32 {
        let (index, phase) = self.cycle_index_and_phase(beat);
        match self.shape {
            LfoShape::SampleHold => seeded_unit(self.seed, index),
            LfoShape::RandomDrift => {
                let a = seeded_unit(self.seed, index);
                let b = seeded_unit(self.seed, index + 1);
                a + (b - a) * smoothstep(phase)
            }
            shape => periodic_shape_value(shape, phase),
        }
    }

    /// Periodic shape value in -1..1 at a phase in 0..1, for lane drawing.
    /// Random shapes return 0 here; draw them from `wave_at` over time instead.
    pub(crate) fn shape_value_at_phase(&self, phase: f32) -> f32 {
        periodic_shape_value(self.shape, phase)
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
            LfoField::Shape => self.shape = self.shape.cycled(dir),
            LfoField::Amount => {
                self.depth_ratio = field.spec().adjust(self.depth_ratio, dir);
            }
            LfoField::Interval => {
                self.set_cycle_preserving_phase(field.spec().adjust(self.cycle_beats, dir), beat);
            }
            LfoField::Offset => {
                self.phase_offset_beats = field.spec().adjust(self.phase_offset_beats, dir);
            }
        }
    }

    pub(crate) fn set_field_at(&mut self, field: LfoField, value: f32, beat: f64) {
        match field {
            LfoField::Shape => self.shape = LfoShape::from_index(value),
            LfoField::Amount => self.depth_ratio = field.spec().parse_value(value),
            LfoField::Interval => {
                self.set_cycle_preserving_phase(field.spec().parse_value(value), beat);
            }
            LfoField::Offset => {
                self.phase_offset_beats = field.spec().parse_value(value);
            }
        }
    }

    /// Set a time field to an exact value, clamped to range but not snapped
    /// to the beat grid — used while the field is being driven in ms.
    pub(crate) fn set_field_raw_at(&mut self, field: LfoField, value: f32, beat: f64) {
        match field {
            LfoField::Interval => self.set_cycle_preserving_phase(
                value.clamp(MIN_LFO_CYCLE_BEATS, MAX_LFO_CYCLE_BEATS),
                beat,
            ),
            LfoField::Offset => {
                self.phase_offset_beats = value.clamp(0.0, MAX_LFO_OFFSET_BEATS);
            }
            _ => self.set_field_at(field, value, beat),
        }
    }

    pub(crate) fn reset_field_at(&mut self, field: LfoField, beat: f64) {
        match field {
            LfoField::Shape => self.shape = LfoShape::Sine,
            LfoField::Amount => self.depth_ratio = field.spec().reset,
            LfoField::Interval => self.set_cycle_preserving_phase(field.spec().reset, beat),
            LfoField::Offset => self.phase_offset_beats = field.spec().reset,
        }
    }

    pub(crate) fn field_ratio(&self, field: LfoField) -> f32 {
        match field {
            LfoField::Shape => self.shape.index() as f32 / (LfoShape::ALL.len() - 1).max(1) as f32,
            LfoField::Amount => field.spec().ratio(self.depth_ratio),
            LfoField::Interval => field.spec().ratio(self.cycle_beats),
            LfoField::Offset => field.spec().ratio(self.phase_offset_beats),
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

    fn set_cycle_preserving_phase(&mut self, cycle_beats: f32, beat: f64) {
        let old_phase = self.phase_at(beat);
        self.cycle_beats = cycle_beats;
        self.phase_offset_beats = nearest_offset_for_phase(old_phase, beat, cycle_beats);
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
    fn morph(
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

/// Shared glide/snap morph for a route type whose only "level" field crosses
/// a leg transition on a glide while every other field snaps: on both sides
/// present, all-but-`get`/`set` fields snap to `to` once `use_to` flips true
/// while the level field glides `tt` between the two; on only one side
/// present, the level field glides to/from 0 while the present side's other
/// fields hold, so the route fades in or out instead of popping.
fn morph_scalar_route<T: Copy>(
    from: Option<&T>,
    to: Option<&T>,
    tt: f32,
    use_to: bool,
    get: fn(&T) -> f32,
    set: fn(&mut T, f32),
) -> Option<T> {
    match (from, to) {
        (Some(f), Some(t)) => {
            let mut route = if use_to { *t } else { *f };
            set(&mut route, get(f) + (get(t) - get(f)) * tt);
            Some(route)
        }
        (Some(f), None) => {
            let mut route = *f;
            set(&mut route, get(f) * (1.0 - tt));
            Some(route)
        }
        (None, Some(t)) => {
            let mut route = *t;
            set(&mut route, get(t) * tt);
            Some(route)
        }
        (None, None) => None,
    }
}

fn nearest_offset_for_phase(phase: f64, beat: f64, cycle_beats: f32) -> f32 {
    let cycle = f64::from(cycle_beats.max(MIN_LFO_CYCLE_BEATS));
    let desired = (phase * cycle - beat).rem_euclid(cycle) as f32;
    let offset_spec = LfoField::Offset.spec();
    let snapped = offset_spec.quantize(desired);
    if phase_distance(phase, phase_at_with_offset(beat, cycle, snapped)) < 0.001 {
        return snapped;
    }

    let mut best = snapped;
    let mut best_distance = phase_distance(phase, phase_at_with_offset(beat, cycle, best));
    // Scan at the finest grid resolution and re-quantize each sample onto the
    // control's actual grid, so `best` always lands on a real rung even
    // though the grid itself is coarser above the floor.
    let steps = ((offset_spec.max - offset_spec.min) / OFFSET_STEP).round() as usize;
    for i in 0..=steps {
        let raw = offset_spec.min + i as f32 * OFFSET_STEP;
        let candidate = offset_spec.quantize(raw);
        let distance = phase_distance(phase, phase_at_with_offset(beat, cycle, candidate));
        if distance < best_distance {
            best = candidate;
            best_distance = distance;
        }
    }
    best
}

fn phase_at_with_offset(beat: f64, cycle: f64, offset: f32) -> f64 {
    ((beat + f64::from(offset)) / cycle).rem_euclid(1.0)
}

fn phase_distance(a: f64, b: f64) -> f64 {
    let diff = (a - b).abs();
    diff.min(1.0 - diff)
}

// ============================================================
// Envelope routes
// ============================================================

/// What re-triggers a one-shot envelope. `EveryBeats` cycles on a musical grid,
/// `OnKick` fires with the kick, and `Once` is the set-and-forget macro that
/// sweeps a single time from song start.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum EnvTrigger {
    EveryBeats(f32),
    OnKick,
    Once,
}

impl EnvTrigger {
    /// Ordered presets the Trigger field cycles through, folding the every-N
    /// interval choices and the macro one-shot into one discrete field.
    const CYCLE: [EnvTrigger; 8] = [
        Self::EveryBeats(1.0),
        Self::EveryBeats(2.0),
        Self::EveryBeats(4.0),
        Self::EveryBeats(8.0),
        Self::EveryBeats(16.0),
        Self::EveryBeats(32.0),
        Self::OnKick,
        Self::Once,
    ];

    fn index(self) -> usize {
        Self::CYCLE.iter().position(|&t| t == self).unwrap_or(2) // default: every 4 beats
    }

    fn cycled(self, dir: f32) -> Self {
        Self::CYCLE[stepped_index(self.index(), dir, Self::CYCLE.len())]
    }

    fn from_index(index: f32) -> Self {
        Self::CYCLE[clamped_index(index, Self::CYCLE.len())]
    }

    fn label(self) -> String {
        match self {
            Self::EveryBeats(n) => format!("every {n:.0} beats"),
            Self::OnKick => "on kick".to_string(),
            Self::Once => "once (macro)".to_string(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EnvField {
    Amount,
    Attack,
    Decay,
    Trigger,
}

impl EnvField {
    pub(crate) const ALL: [EnvField; 4] = [Self::Amount, Self::Attack, Self::Decay, Self::Trigger];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Amount => "amount",
            Self::Attack => "attack",
            Self::Decay => "decay",
            Self::Trigger => "trigger",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct EnvelopeRoute {
    /// Bipolar sweep depth in -1..1; positive blooms up, negative dips down.
    pub(crate) amount: f32,
    pub(crate) attack_beats: f32,
    /// Fall time back to base; 0 holds at the peak indefinitely (macro hold).
    pub(crate) decay_beats: f32,
    pub(crate) trigger: EnvTrigger,
}

impl Default for EnvelopeRoute {
    fn default() -> Self {
        Self {
            amount: 0.0,
            attack_beats: DEFAULT_ENV_ATTACK_BEATS,
            decay_beats: DEFAULT_ENV_DECAY_BEATS,
            trigger: EnvTrigger::EveryBeats(4.0),
        }
    }
}

impl EnvelopeRoute {
    /// Beats elapsed since the most recent trigger, or None before the first
    /// trigger has fired. Pure function of the context so UI and engine agree.
    fn beats_since_trigger(&self, ctx: ModContext) -> Option<f32> {
        match self.trigger {
            EnvTrigger::EveryBeats(n) => {
                let n = f64::from(n.max(ENV_BEATS_STEP));
                if ctx.beat < 0.0 {
                    return None;
                }
                Some(ctx.beat.rem_euclid(n) as f32)
            }
            EnvTrigger::Once => {
                if ctx.beat < 0.0 {
                    None
                } else {
                    Some(ctx.beat as f32)
                }
            }
            EnvTrigger::OnKick => {
                let interval = f64::from(ctx.kick_interval_beats.max(1.0 / 64.0));
                let offset = f64::from(ctx.kick_offset_beats).rem_euclid(interval);
                let slot = ((ctx.beat - offset) / interval).floor();
                let last = offset + slot * interval;
                if last < -1e-9 {
                    None
                } else {
                    Some((ctx.beat - last) as f32)
                }
            }
        }
    }

    /// One-shot AD level in 0..1 at the given beat. Zero attack fires instantly;
    /// zero decay holds at the peak (set-and-forget macro).
    pub(crate) fn level_at(&self, ctx: ModContext) -> f32 {
        let Some(since) = self.beats_since_trigger(ctx) else {
            return 0.0;
        };
        self.level_for_elapsed(since)
    }

    fn level_for_elapsed(&self, since: f32) -> f32 {
        if since < 0.0 {
            0.0
        } else if self.attack_beats > 0.0 && since < self.attack_beats {
            since / self.attack_beats
        } else if self.decay_beats <= 0.0 {
            1.0
        } else if since < self.attack_beats + self.decay_beats {
            1.0 - (since - self.attack_beats) / self.decay_beats
        } else {
            0.0
        }
    }

    /// Beats spanned by one trigger period, used to scope the animated lane.
    pub(crate) fn window_beats(&self) -> f32 {
        match self.trigger {
            EnvTrigger::EveryBeats(n) => n.max(ENV_BEATS_STEP),
            EnvTrigger::OnKick => self.attack_beats + self.decay_beats.max(ENV_BEATS_STEP),
            EnvTrigger::Once => (self.attack_beats + self.decay_beats).max(ENV_BEATS_STEP),
        }
    }

    /// Envelope level at a given elapsed beat, for drawing the lane curve.
    pub(crate) fn level_for_lane(&self, since: f32) -> f32 {
        self.level_for_elapsed(since)
    }

    /// Where the live phase head sits along the lane window, 0..1.
    pub(crate) fn lane_head_phase(&self, ctx: ModContext) -> f32 {
        match self.beats_since_trigger(ctx) {
            Some(since) => (since / self.window_beats().max(ENV_BEATS_STEP)).clamp(0.0, 1.0),
            None => 0.0,
        }
    }

    pub(crate) fn adjust_field(&mut self, field: EnvField, dir: f32) {
        match field {
            EnvField::Amount => {
                self.amount = (self.amount + dir * ENV_AMOUNT_STEP).clamp(-1.0, 1.0);
            }
            EnvField::Attack => {
                self.attack_beats =
                    snap_step(self.attack_beats + dir * ENV_BEATS_STEP, ENV_BEATS_STEP)
                        .clamp(0.0, MAX_ENV_ATTACK_BEATS);
            }
            EnvField::Decay => {
                self.decay_beats =
                    snap_step(self.decay_beats + dir * ENV_BEATS_STEP, ENV_BEATS_STEP)
                        .clamp(0.0, MAX_ENV_DECAY_BEATS);
            }
            EnvField::Trigger => self.trigger = self.trigger.cycled(dir),
        }
    }

    pub(crate) fn set_field(&mut self, field: EnvField, value: f32) {
        match field {
            EnvField::Amount => {
                self.amount = (value / 100.0).clamp(-1.0, 1.0);
            }
            EnvField::Attack => {
                self.attack_beats =
                    snap_step(value, ENV_BEATS_STEP).clamp(0.0, MAX_ENV_ATTACK_BEATS);
            }
            EnvField::Decay => {
                self.decay_beats = snap_step(value, ENV_BEATS_STEP).clamp(0.0, MAX_ENV_DECAY_BEATS);
            }
            EnvField::Trigger => self.trigger = EnvTrigger::from_index(value),
        }
    }

    /// Set a time field to an exact value, clamped to range but not snapped
    /// to the beat grid — used while the field is being driven in ms.
    pub(crate) fn set_field_raw(&mut self, field: EnvField, value: f32) {
        match field {
            EnvField::Attack => self.attack_beats = value.clamp(0.0, MAX_ENV_ATTACK_BEATS),
            EnvField::Decay => self.decay_beats = value.clamp(0.0, MAX_ENV_DECAY_BEATS),
            EnvField::Amount | EnvField::Trigger => self.set_field(field, value),
        }
    }

    pub(crate) fn reset_field(&mut self, field: EnvField) {
        let defaults = EnvelopeRoute::default();
        match field {
            EnvField::Amount => self.amount = defaults.amount,
            EnvField::Attack => self.attack_beats = defaults.attack_beats,
            EnvField::Decay => self.decay_beats = defaults.decay_beats,
            EnvField::Trigger => self.trigger = defaults.trigger,
        }
    }

    pub(crate) fn field_ratio(&self, field: EnvField) -> f32 {
        match field {
            EnvField::Amount => (self.amount * 0.5 + 0.5).clamp(0.0, 1.0),
            EnvField::Attack => (self.attack_beats / MAX_ENV_ATTACK_BEATS).clamp(0.0, 1.0),
            EnvField::Decay => (self.decay_beats / MAX_ENV_DECAY_BEATS).clamp(0.0, 1.0),
            EnvField::Trigger => {
                self.trigger.index() as f32 / (EnvTrigger::CYCLE.len() - 1).max(1) as f32
            }
        }
    }

    pub(crate) fn field_display(&self, field: EnvField) -> String {
        match field {
            EnvField::Amount => format!("{:+.0}%", self.amount * 100.0),
            EnvField::Attack => format!("{:.2} beats", self.attack_beats),
            EnvField::Decay => {
                if self.decay_beats <= 0.0 {
                    "hold".to_string()
                } else {
                    format!("{:.2} beats", self.decay_beats)
                }
            }
            EnvField::Trigger => self.trigger.label(),
        }
    }

    /// Morph an optional envelope route across a leg transition; same
    /// glide/snap split as `LfoRoute::morph` with `amount` as the level
    /// field. See `morph_scalar_route` for the full rationale.
    fn morph(
        from: Option<&EnvelopeRoute>,
        to: Option<&EnvelopeRoute>,
        tt: f32,
        use_to: bool,
    ) -> Option<EnvelopeRoute> {
        morph_scalar_route(from, to, tt, use_to, |r| r.amount, |r, v| r.amount = v)
    }
}

// ============================================================
// Macro routes
// ============================================================

const MACRO_AMOUNT_STEP: f32 = 0.01;

/// One of the four macro sliders' independent amount fields on a route.
/// There is no "target" selection any more: every macro assignment (a
/// regular control's `v` route, or a field macro stacked on an LFO field)
/// holds a bipolar amount for all four macro sliders at once, so a single
/// control can ride several macros simultaneously.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MacroField(usize);

impl MacroField {
    pub(crate) const ALL: [MacroField; MACRO_COUNT] = {
        let mut all = [MacroField(0); MACRO_COUNT];
        let mut i = 0;
        while i < MACRO_COUNT {
            all[i] = MacroField(i);
            i += 1;
        }
        all
    };

    pub(crate) fn label(self) -> String {
        format!("macro {}", self.0 + 1)
    }

    fn index(self) -> usize {
        self.0
    }
}

/// Assignment of a control (or a single stacked LFO field) to the macro
/// sliders. Each of the four macro sliders has its own independent bipolar
/// amount in -1..1, applied to the control's full range and summed — a
/// control can ride several macros at once, each set directly, none of them
/// requiring the others to be neutral.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MacroRoute {
    pub(crate) amounts: [f32; MACRO_COUNT],
}

impl Default for MacroRoute {
    fn default() -> Self {
        Self {
            amounts: [0.0; MACRO_COUNT],
        }
    }
}

impl MacroRoute {
    pub(crate) fn is_neutral(self) -> bool {
        self.amounts.iter().all(|a| a.abs() <= f32::EPSILON)
    }

    pub(crate) fn adjust_field(&mut self, field: MacroField, dir: f32) {
        let a = &mut self.amounts[field.index()];
        *a = (*a + dir * MACRO_AMOUNT_STEP).clamp(-1.0, 1.0);
    }

    pub(crate) fn set_field(&mut self, field: MacroField, value: f32) {
        self.amounts[field.index()] = (value / 100.0).clamp(-1.0, 1.0);
    }

    pub(crate) fn reset_field(&mut self, field: MacroField) {
        self.amounts[field.index()] = 0.0;
    }

    pub(crate) fn field_ratio(self, field: MacroField) -> f32 {
        (self.amounts[field.index()] * 0.5 + 0.5).clamp(0.0, 1.0)
    }

    pub(crate) fn field_display(self, field: MacroField) -> String {
        format!("{:+.0}%", self.amounts[field.index()] * 100.0)
    }

    /// Compact summary of every non-neutral slot, e.g. "m1 +30%  m3 -50%",
    /// for the closed chip line. "none" when every slot is at zero.
    pub(crate) fn summary(self) -> String {
        let parts: Vec<String> = self
            .amounts
            .iter()
            .enumerate()
            .filter(|(_, a)| a.abs() > f32::EPSILON)
            .map(|(i, a)| format!("m{} {:+.0}%", i + 1, a * 100.0))
            .collect();
        if parts.is_empty() {
            "none".to_string()
        } else {
            parts.join("  ")
        }
    }

    /// Combined bipolar contribution ratio: sum over every macro slider of
    /// this route's amount times that slider's live value, each individually
    /// clamped before summing. Multiplied by the control's range by the
    /// caller. 0.0 when neutral, matching the "no effect" case.
    fn combined(self, macro_values: &[f32; MACRO_COUNT]) -> f32 {
        self.amounts
            .iter()
            .zip(macro_values)
            .map(|(a, v)| a.clamp(-1.0, 1.0) * v.clamp(0.0, 1.0))
            .sum()
    }

    /// Morph an optional macro route across a leg transition. Every slot is
    /// a plain bipolar amount, so there's no snap-field split — the whole
    /// route just glides by `tt`, fading in/out toward 0 on the side it's
    /// missing from.
    fn morph(from: Option<&MacroRoute>, to: Option<&MacroRoute>, tt: f32) -> Option<MacroRoute> {
        match (from, to) {
            (Some(f), Some(t)) => Some(MacroRoute {
                amounts: std::array::from_fn(|i| f.amounts[i] + (t.amounts[i] - f.amounts[i]) * tt),
            }),
            (Some(f), None) => Some(MacroRoute {
                amounts: f.amounts.map(|a| a * (1.0 - tt)),
            }),
            (None, Some(t)) => Some(MacroRoute {
                amounts: t.amounts.map(|a| a * tt),
            }),
            (None, None) => None,
        }
    }

    /// Best-case full reach: how far the combined contribution could swing
    /// the control below (negative) and above (positive) base if every macro
    /// slider it rides independently reached its own extreme (1.0). Used by
    /// the reach-shadow marker, not the live value.
    pub(crate) fn swing(self, range: f32) -> (f32, f32) {
        let mut lo = 0.0;
        let mut hi = 0.0;
        for a in self.amounts {
            let a = a.clamp(-1.0, 1.0);
            if a < 0.0 {
                lo += a * range;
            } else {
                hi += a * range;
            }
        }
        (lo, hi)
    }
}

// ============================================================
// Automation state
// ============================================================

#[derive(Clone, Copy, Debug, PartialEq)]
struct OpenEditor {
    address: ControlAddress,
    kind: ModKind,
}

#[derive(Clone, Default)]
pub(crate) struct AutomationState {
    routes: BTreeMap<ControlAddress, LfoRoute>,
    envelopes: BTreeMap<ControlAddress, EnvelopeRoute>,
    macros: BTreeMap<ControlAddress, MacroRoute>,
    /// A macro stacked onto a single numeric field of an open LFO editor
    /// (amount, interval, or offset), keyed the same way as `FlippedUnits`:
    /// `unit_key(control id, Some(field key))`. Only ever created when the
    /// user explicitly presses `v` on that field — never on by default —
    /// and pruned back out on close if left at neutral, same as every other
    /// route kind.
    field_macros: BTreeMap<String, MacroRoute>,
    open: Option<OpenEditor>,
    /// The field-macro key currently expanded for editing, if any. Only
    /// meaningful while `open` points at the same control's LFO editor.
    open_field: Option<String>,
}

impl AutomationState {
    pub(crate) fn open_or_create(&mut self, address: ControlAddress) -> &mut LfoRoute {
        let route = self
            .routes
            .entry(address)
            .or_insert_with(|| LfoRoute::with_seed(seed_for_id(address.id())));
        self.open = Some(OpenEditor {
            address,
            kind: ModKind::Lfo,
        });
        route
    }

    pub(crate) fn open_or_create_envelope(
        &mut self,
        address: ControlAddress,
    ) -> &mut EnvelopeRoute {
        let route = self.envelopes.entry(address).or_default();
        self.open = Some(OpenEditor {
            address,
            kind: ModKind::Envelope,
        });
        route
    }

    pub(crate) fn open_or_create_macro(&mut self, address: ControlAddress) -> &mut MacroRoute {
        let route = self.macros.entry(address).or_default();
        self.open = Some(OpenEditor {
            address,
            kind: ModKind::Macro,
        });
        route
    }

    /// Remove the route backing the open editor and close it. The x gesture:
    /// explicit, worked on the first try, unlike double-tap.
    pub(crate) fn remove_open_route(&mut self) {
        let Some(open) = self.open.take() else {
            return;
        };
        match open.kind {
            ModKind::Lfo => {
                self.routes.remove(&open.address);
                self.remove_field_macros_for(open.address, "lfo.");
            }
            ModKind::Envelope => {
                self.envelopes.remove(&open.address);
            }
            ModKind::Macro => {
                self.macros.remove(&open.address);
            }
        }
        self.open_field = None;
    }

    /// Strip every modulator from a control (LFO, envelope, macro route,
    /// field macros), closing the editor if it was open on that control.
    pub(crate) fn clear_control(&mut self, address: ControlAddress) {
        self.routes.remove(&address);
        self.envelopes.remove(&address);
        self.macros.remove(&address);
        self.remove_field_macros_for(address, "");
        if self.open.is_some_and(|open| open.address == address) {
            self.open = None;
        }
    }

    fn remove_field_macros_for(&mut self, address: ControlAddress, field_prefix: &str) {
        let prefix = format!("{}#{field_prefix}", address.id());
        self.field_macros.retain(|key, _| !key.starts_with(&prefix));
        if self
            .open_field
            .as_ref()
            .is_some_and(|key| key.starts_with(&prefix))
        {
            self.open_field = None;
        }
    }

    /// Close the editor; a route left at neutral amount is dead weight and is
    /// removed so it never colours the UI or the song code.
    pub(crate) fn close_editor(&mut self) {
        self.close_open_field();
        let Some(open) = self.open.take() else {
            return;
        };
        match open.kind {
            ModKind::Lfo => {
                // depth_ratio alone isn't the whole story: a field macro
                // stacked on lfo.amount (or interval/offset) can still be
                // driving the route externally even while its own base
                // amount sits at neutral, so the route stays live and must
                // not be pruned out from under it.
                let base_neutral = self
                    .routes
                    .get(&open.address)
                    .is_some_and(|route| route.depth_ratio <= f32::EPSILON);
                let field_macro_prefix = format!("{}#lfo.", open.address.id());
                let has_live_field_macro = self.field_macros.iter().any(|(key, route)| {
                    key.starts_with(&field_macro_prefix) && !route.is_neutral()
                });
                if base_neutral && !has_live_field_macro {
                    self.routes.remove(&open.address);
                }
            }
            ModKind::Envelope => {
                if self
                    .envelopes
                    .get(&open.address)
                    .is_some_and(|route| route.amount.abs() <= f32::EPSILON)
                {
                    self.envelopes.remove(&open.address);
                }
            }
            ModKind::Macro => {
                if self
                    .macros
                    .get(&open.address)
                    .is_some_and(|route| route.is_neutral())
                {
                    self.macros.remove(&open.address);
                }
            }
        }
    }

    /// The field-macro key currently expanded for editing, if any.
    pub(crate) fn open_field(&self) -> Option<&str> {
        self.open_field.as_deref()
    }

    /// Toggle the nested macro editor for a field: same key closes (pruning
    /// it if left neutral), any other key swaps to it (creating it
    /// audible-neutral). This is the only way a field macro is created —
    /// never on by default.
    pub(crate) fn toggle_open_field(&mut self, key: String) {
        if self.open_field.as_deref() == Some(key.as_str()) {
            self.close_open_field();
            return;
        }
        self.close_open_field();
        self.field_macros.entry(key.clone()).or_default();
        self.open_field = Some(key);
    }

    /// Close just the nested field-macro editor, keeping the parent LFO
    /// editor open. The inner half of Esc/`v`'s one-level-at-a-time close.
    pub(crate) fn close_open_field(&mut self) {
        let Some(key) = self.open_field.take() else {
            return;
        };
        if self
            .field_macros
            .get(&key)
            .is_some_and(|route| route.is_neutral())
        {
            self.field_macros.remove(&key);
        }
    }

    pub(crate) fn field_macro(&self, key: &str) -> Option<&MacroRoute> {
        self.field_macros.get(key)
    }

    pub(crate) fn field_macro_mut(&mut self, key: &str) -> Option<&mut MacroRoute> {
        self.field_macros.get_mut(key)
    }

    pub(crate) fn set_field_macro(&mut self, key: String, route: MacroRoute) {
        self.field_macros.insert(key, route);
    }

    /// Remove a stacked field macro outright (the x gesture on its nested
    /// row), closing it if it was the one expanded for editing.
    pub(crate) fn remove_field_macro(&mut self, key: &str) {
        self.field_macros.remove(key);
        if self.open_field.as_deref() == Some(key) {
            self.open_field = None;
        }
    }

    pub(crate) fn field_macros(&self) -> impl Iterator<Item = (&str, &MacroRoute)> {
        self.field_macros.iter().map(|(k, v)| (k.as_str(), v))
    }

    pub(crate) fn is_editor_open(&self) -> bool {
        self.open.is_some()
    }

    pub(crate) fn active_address(&self) -> Option<ControlAddress> {
        self.open.map(|open| open.address)
    }

    pub(crate) fn active_kind(&self) -> Option<ModKind> {
        self.open.map(|open| open.kind)
    }

    pub(crate) fn route(&self, address: ControlAddress) -> Option<&LfoRoute> {
        self.routes.get(&address)
    }

    pub(crate) fn route_mut(&mut self, address: ControlAddress) -> Option<&mut LfoRoute> {
        self.routes.get_mut(&address)
    }

    pub(crate) fn set_route(&mut self, address: ControlAddress, route: LfoRoute) {
        self.routes.insert(address, route);
    }

    pub(crate) fn routes(&self) -> impl Iterator<Item = (ControlAddress, &LfoRoute)> {
        self.routes.iter().map(|(address, route)| (*address, route))
    }

    pub(crate) fn envelope(&self, address: ControlAddress) -> Option<&EnvelopeRoute> {
        self.envelopes.get(&address)
    }

    pub(crate) fn envelope_mut(&mut self, address: ControlAddress) -> Option<&mut EnvelopeRoute> {
        self.envelopes.get_mut(&address)
    }

    pub(crate) fn set_envelope(&mut self, address: ControlAddress, route: EnvelopeRoute) {
        self.envelopes.insert(address, route);
    }

    pub(crate) fn macro_route(&self, address: ControlAddress) -> Option<&MacroRoute> {
        self.macros.get(&address)
    }

    pub(crate) fn macro_route_mut(&mut self, address: ControlAddress) -> Option<&mut MacroRoute> {
        self.macros.get_mut(&address)
    }

    pub(crate) fn set_macro_route(&mut self, address: ControlAddress, route: MacroRoute) {
        self.macros.insert(address, route);
    }

    pub(crate) fn macro_routes(&self) -> impl Iterator<Item = (ControlAddress, &MacroRoute)> {
        self.macros.iter().map(|(address, route)| (*address, route))
    }

    pub(crate) fn envelopes(&self) -> impl Iterator<Item = (ControlAddress, &EnvelopeRoute)> {
        self.envelopes
            .iter()
            .map(|(address, route)| (*address, route))
    }

    fn modulated_addresses(&self) -> BTreeSet<ControlAddress> {
        self.routes
            .keys()
            .chain(self.envelopes.keys())
            .chain(self.macros.keys())
            .copied()
            .collect()
    }

    /// Morphed automation state for a leg transition between `from` and `to`,
    /// the `AutomationState` counterpart to `MorphState::controls_at`'s
    /// per-`FluidControls`-field glide/snap split: `tt` (0..1) is the glide
    /// fraction for each route's level field (`LfoRoute::depth_ratio`,
    /// `EnvelopeRoute::amount`, every `MacroRoute` amount), and `use_to`
    /// selects which side's other fields (shape, cycle, attack/decay,
    /// trigger, …) are live — false holds `from`'s, true snaps to `to`'s, all
    /// together at the transition downbeat, mirroring
    /// `STRUCTURAL_SNAP_IDS`. A route present on only one side fades in or
    /// out via the level field rather than popping, and never needs explicit
    /// removal: once this leg's `to` becomes the next leg's `from`, an
    /// absent route is simply absent from the map again. Editor-open state
    /// (`open`, `open_field`) is UI navigation, not audible, and is never
    /// morphed — the result always has neither open.
    pub(crate) fn morph(
        from: &AutomationState,
        to: &AutomationState,
        tt: f32,
        use_to: bool,
    ) -> AutomationState {
        let mut result = AutomationState::default();
        morph_map(&from.routes, &to.routes, &mut result.routes, |f, t| {
            LfoRoute::morph(f, t, tt, use_to)
        });
        morph_map(
            &from.envelopes,
            &to.envelopes,
            &mut result.envelopes,
            |f, t| EnvelopeRoute::morph(f, t, tt, use_to),
        );
        morph_map(&from.macros, &to.macros, &mut result.macros, |f, t| {
            MacroRoute::morph(f, t, tt)
        });
        morph_map(
            &from.field_macros,
            &to.field_macros,
            &mut result.field_macros,
            |f, t| MacroRoute::morph(f, t, tt),
        );
        result
    }
}

/// Merge two route maps across a leg transition: build the union of both
/// sides' keys (kept in sorted order via `BTreeSet`, matching the previous
/// per-map key-collection loops), then insert `morph(from, to)` for each key
/// that yields a route. A key absent from the result (both morph inputs
/// `None`, or `morph` returning `None`) is simply left out — this is how a
/// route naturally disappears once both legs' endpoints lack it.
fn morph_map<K: Ord + Clone, V>(
    from: &BTreeMap<K, V>,
    to: &BTreeMap<K, V>,
    out: &mut BTreeMap<K, V>,
    morph: impl Fn(Option<&V>, Option<&V>) -> Option<V>,
) {
    let keys: BTreeSet<&K> = from.keys().chain(to.keys()).collect();
    for key in keys {
        if let Some(route) = morph(from.get(key), to.get(key)) {
            out.insert(key.clone(), route);
        }
    }
}

/// The effective value the engine plays for a modulated control: base plus
/// LFO plus envelope plus macro, summed, clamped to range, then snapped per
/// the control's `LfoSnap`. `macro_mod` carries `(route amount, live macro
/// value)`. The UI's modulation marker must go through this too so it shows
/// what is heard.
pub(crate) fn modulated_control_value_full(
    spec: &ControlSpec,
    lfo: Option<&LfoRoute>,
    envelope: Option<&EnvelopeRoute>,
    macro_mod: Option<f32>,
    base: f32,
    ctx: ModContext,
) -> f32 {
    let range = spec.max - spec.min;
    let mut value = base;
    if let Some(route) = lfo {
        value += route.wave_at(ctx.beat) * range * route.depth_ratio.clamp(0.0, 1.0);
    }
    if let Some(route) = envelope {
        value += route.level_at(ctx) * range * route.amount.clamp(-1.0, 1.0);
    }
    if let Some(combined) = macro_mod {
        value += combined * range;
    }
    let value = value.clamp(spec.min, spec.max);
    match spec.lfo_snap {
        LfoSnap::None => value,
        LfoSnap::PowerOfTwo => nearest_power_of_two(value, spec.min, spec.max),
        LfoSnap::Step => spec.quantize(value),
    }
}

/// LFO-only convenience wrapper over `modulated_control_value_full`.
#[cfg(test)]
pub(crate) fn modulated_control_value(
    spec: &ControlSpec,
    route: &LfoRoute,
    base: f32,
    beat: f64,
) -> f32 {
    modulated_control_value_full(
        spec,
        Some(route),
        None,
        None,
        base,
        ModContext::lfo_only(beat),
    )
}

/// Combined contribution of every macro slider a route rides, or None when
/// the route is neutral (every slot at zero). Reads the macro sliders from
/// `controls`, so callers that want their own modulation reflected must
/// apply it to `controls` first (`apply_automation` pass one does).
fn macro_pair(route: &MacroRoute, controls: &FluidControls) -> Option<f32> {
    if route.is_neutral() {
        return None;
    }
    Some(route.combined(&controls.macros.values))
}

/// UI-side variant: recomputes each ridden macro slider's own modulated
/// value from raw controls, mirroring what `apply_automation` pass one
/// produces, so markers show what the engine hears.
fn live_macro_pair(
    route: &MacroRoute,
    automation: &AutomationState,
    controls: &FluidControls,
    ctx: ModContext,
) -> Option<f32> {
    if route.is_neutral() {
        return None;
    }
    let mut values = [0.0; MACRO_COUNT];
    for (i, value) in values.iter_mut().enumerate() {
        if route.amounts[i].abs() <= f32::EPSILON {
            continue;
        }
        let spec = spec_by_id(MACRO_CONTROLS[i].id).expect("macro sliders are registered controls");
        let macro_address = ControlAddress::new(spec.id);
        *value = modulated_control_value_full(
            spec,
            automation
                .route(macro_address)
                .filter(|route| route.depth_ratio > f32::EPSILON),
            automation
                .envelope(macro_address)
                .filter(|route| route.amount.abs() > f32::EPSILON),
            None,
            (spec.get)(controls),
            ctx,
        );
    }
    Some(route.combined(&values))
}

pub(crate) fn live_macro_contribution(
    automation: &AutomationState,
    controls: &FluidControls,
    address: ControlAddress,
    ctx: ModContext,
) -> Option<f32> {
    let route = automation.macro_route(address)?;
    live_macro_pair(route, automation, controls, ctx)
}

/// Slot order for stacked LFO field macros, shared by every fold over them
/// (`PlannedRoute::field_macros` uses the same indices).
const LFO_FIELD_MACRO_SLOTS: [LfoField; 3] =
    [LfoField::Amount, LfoField::Interval, LfoField::Offset];

/// Fold per-slot combined macro ratios into a modulated copy of the route.
/// `contribution(slot)` resolves the stacked macro on `LFO_FIELD_MACRO_SLOTS[slot]`,
/// or None when there is none / it is neutral.
fn fold_field_macro_contributions(
    route: &LfoRoute,
    mut contribution: impl FnMut(usize) -> Option<f32>,
) -> LfoRoute {
    let mut effective = *route;
    if let Some(combined) = contribution(0) {
        effective.depth_ratio = (route.depth_ratio + combined).clamp(0.0, 1.0);
    }
    if let Some(combined) = contribution(1) {
        effective.cycle_beats = (route.cycle_beats
            + combined * (MAX_LFO_CYCLE_BEATS - MIN_LFO_CYCLE_BEATS))
            .clamp(MIN_LFO_CYCLE_BEATS, MAX_LFO_CYCLE_BEATS);
    }
    if let Some(combined) = contribution(2) {
        effective.phase_offset_beats = (route.phase_offset_beats + combined * MAX_LFO_OFFSET_BEATS)
            .clamp(0.0, MAX_LFO_OFFSET_BEATS);
    }
    effective
}

/// Fold any macros stacked onto an LFO route's amount/interval/offset (via
/// the field editor's `v` gesture) into a modulated copy, using whatever
/// `contribution` resolves each stacked field-macro's combined ratio to.
/// A macro slider's own LFO never takes a stacked macro (no macro chasing
/// itself), so this is a no-op there.
fn apply_field_macros(
    automation: &AutomationState,
    address: ControlAddress,
    route: &LfoRoute,
    mut contribution: impl FnMut(&MacroRoute) -> Option<f32>,
) -> LfoRoute {
    if is_macro_id(address.id()) {
        return *route;
    }
    fold_field_macro_contributions(route, |slot| {
        let key = unit_key(address.id(), LFO_FIELD_MACRO_SLOTS[slot].macro_key());
        automation.field_macro(&key).and_then(&mut contribution)
    })
}

/// Engine-side semantics (`AutomationPlan::apply` is the production copy):
/// `controls` already reflects pass-one's modulated macro slider values,
/// so a plain lookup is correct.
#[cfg(test)]
pub(crate) fn effective_lfo_route(
    automation: &AutomationState,
    controls: &FluidControls,
    address: ControlAddress,
    route: &LfoRoute,
) -> LfoRoute {
    apply_field_macros(automation, address, route, |field_route| {
        macro_pair(field_route, controls)
    })
}

/// UI-side twin: recomputes each stacked macro's own live modulation so the
/// parent slider's markers show what the engine hears.
pub(crate) fn live_effective_lfo_route(
    automation: &AutomationState,
    controls: &FluidControls,
    address: ControlAddress,
    route: &LfoRoute,
    ctx: ModContext,
) -> LfoRoute {
    apply_field_macros(automation, address, route, |field_route| {
        live_macro_pair(field_route, automation, controls, ctx)
    })
}

/// One modulated control's routes, resolved to plain copies so applying
/// them per sample needs no map lookups, string keys, or heap.
struct PlannedRoute {
    spec: &'static ControlSpec,
    lfo: Option<LfoRoute>,
    /// Stacked field macros indexed by `LFO_FIELD_MACRO_SLOTS`.
    field_macros: [Option<MacroRoute>; 3],
    envelope: Option<EnvelopeRoute>,
    macro_route: Option<MacroRoute>,
}

/// Allocation-free application plan for an `AutomationState`. The engine
/// rebuilds it only when the published automation Arc changes (a UI edit),
/// so the per-sample audio hot path never touches the allocator.
#[derive(Default)]
pub(crate) struct AutomationPlan {
    /// Macro sliders first, so targets read already-modulated macro values.
    routes: Vec<PlannedRoute>,
}

impl AutomationPlan {
    pub(crate) fn rebuild(&mut self, automation: &AutomationState) {
        self.routes.clear();
        let addresses = automation.modulated_addresses();
        let (macro_sliders, targets): (Vec<_>, Vec<_>) = addresses
            .into_iter()
            .partition(|address| is_macro_id(address.id()));
        for address in macro_sliders.into_iter().chain(targets) {
            let lfo = automation.route(address).copied();
            // Macro sliders' own LFOs never take a stacked macro.
            let field_macros = if lfo.is_none() || is_macro_id(address.id()) {
                [None; 3]
            } else {
                LFO_FIELD_MACRO_SLOTS.map(|field| {
                    let key = unit_key(address.id(), field.macro_key());
                    automation.field_macro(&key).copied()
                })
            };
            self.routes.push(PlannedRoute {
                spec: address.spec(),
                lfo,
                field_macros,
                envelope: automation.envelope(address).copied(),
                macro_route: automation.macro_route(address).copied(),
            });
        }
    }

    pub(crate) fn apply(&self, controls: &mut FluidControls, timing: TimingContext) {
        let ctx = ModContext {
            beat: timing.beat,
            kick_interval_beats: controls.kick.interval_beats,
            kick_offset_beats: controls.kick.offset_beats,
        };
        for planned in &self.routes {
            let lfo = planned.lfo.map(|route| {
                fold_field_macro_contributions(&route, |slot| {
                    planned.field_macros[slot]
                        .as_ref()
                        .and_then(|field_route| macro_pair(field_route, controls))
                })
            });
            let lfo = lfo
                .as_ref()
                .filter(|route| route.depth_ratio > f32::EPSILON);
            let envelope = planned
                .envelope
                .as_ref()
                .filter(|route| route.amount.abs() > f32::EPSILON);
            let macro_mod = planned
                .macro_route
                .as_ref()
                .and_then(|route| macro_pair(route, controls));
            if lfo.is_none() && envelope.is_none() && macro_mod.is_none() {
                continue;
            }
            let base = (planned.spec.get)(controls);
            let value =
                modulated_control_value_full(planned.spec, lfo, envelope, macro_mod, base, ctx);
            (planned.spec.set)(controls, value);
        }
    }
}

/// One-shot convenience over `AutomationPlan` for tests: rebuild + apply.
#[cfg(test)]
pub(crate) fn apply_automation(
    controls: &mut FluidControls,
    automation: &AutomationState,
    timing: TimingContext,
) {
    let mut plan = AutomationPlan::default();
    plan.rebuild(automation);
    plan.apply(controls, timing);
}
