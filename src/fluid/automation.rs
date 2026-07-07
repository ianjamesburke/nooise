use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::f32::consts::TAU;
use std::fmt;

use super::{
    ControlSpec, FluidControls, LfoSnap, MACRO_CONTROLS, MACRO_COUNT, TimingContext, is_macro_id,
    nearest_power_of_two, normalize_unit_input, snap_step, spec_by_id,
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
        LfoShape::RampUp => 2.0 * phase - 1.0,
        LfoShape::RampDown => 1.0 - 2.0 * phase,
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
}

impl LfoFieldSpec {
    pub(crate) fn adjust(self, value: f32, dir: f32) -> f32 {
        self.quantize(value + dir * self.step)
    }

    pub(crate) fn parse_value(self, value: f32) -> f32 {
        match self.entry {
            LfoEntry::Percent => normalize_unit_input(value).clamp(self.min, self.max),
            LfoEntry::Snap => self.quantize(value),
        }
    }

    pub(crate) fn quantize(self, value: f32) -> f32 {
        snap_step(value.clamp(self.min, self.max), self.step).clamp(self.min, self.max)
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
    },
    LfoFieldSpec {
        field: LfoField::Interval,
        label: "interval",
        min: MIN_LFO_CYCLE_BEATS,
        max: MAX_LFO_CYCLE_BEATS,
        step: INTERVAL_STEP,
        entry: LfoEntry::Snap,
        reset: MIN_LFO_CYCLE_BEATS,
    },
    LfoFieldSpec {
        field: LfoField::Offset,
        label: "offset",
        min: 0.0,
        max: MAX_LFO_OFFSET_BEATS,
        step: OFFSET_STEP,
        entry: LfoEntry::Snap,
        reset: 0.0,
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
            LfoField::Shape => {
                self.shape.index() as f32 / (LfoShape::ALL.len() - 1).max(1) as f32
            }
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
    let steps = ((offset_spec.max - offset_spec.min) / offset_spec.step).round() as usize;
    for i in 0..=steps {
        let candidate = offset_spec.min + i as f32 * offset_spec.step;
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
        Self::CYCLE
            .iter()
            .position(|&t| t == self)
            .unwrap_or(2) // default: every 4 beats
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
                let unit = if value.abs() > 1.0 { value / 100.0 } else { value };
                self.amount = unit.clamp(-1.0, 1.0);
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
}

// ============================================================
// Macro routes
// ============================================================

const MACRO_AMOUNT_STEP: f32 = 0.01;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MacroField {
    Target,
    Amount,
}

impl MacroField {
    pub(crate) const ALL: [MacroField; 2] = [Self::Target, Self::Amount];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Target => "macro",
            Self::Amount => "amount",
        }
    }
}

/// Assignment of a regular control to one of the macro sliders. The macro's
/// live value scales `amount` into the control's range as an independent
/// modulation source alongside the LFO.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MacroRoute {
    /// Index into the macro sliders, or None while unassigned.
    pub(crate) target: Option<usize>,
    /// Bipolar depth in -1..1 applied to the control's full range.
    pub(crate) amount: f32,
}

impl Default for MacroRoute {
    fn default() -> Self {
        Self {
            target: None,
            amount: 0.0,
        }
    }
}

impl MacroRoute {
    /// Target cycles through none + each macro slider; none sits at index 0.
    fn target_index(self) -> usize {
        self.target.map_or(0, |t| t + 1)
    }

    fn target_from_index(index: usize) -> Option<usize> {
        index.checked_sub(1)
    }

    pub(crate) fn is_neutral(self) -> bool {
        self.target.is_none() || self.amount.abs() <= f32::EPSILON
    }

    pub(crate) fn adjust_field(&mut self, field: MacroField, dir: f32) {
        match field {
            MacroField::Target => {
                let states = MACRO_COUNT + 1;
                self.target =
                    Self::target_from_index(stepped_index(self.target_index(), dir, states));
            }
            MacroField::Amount => {
                self.amount = (self.amount + dir * MACRO_AMOUNT_STEP).clamp(-1.0, 1.0);
            }
        }
    }

    pub(crate) fn set_field(&mut self, field: MacroField, value: f32) {
        match field {
            MacroField::Target => {
                self.target = Self::target_from_index(clamped_index(value, MACRO_COUNT + 1));
            }
            MacroField::Amount => {
                let unit = if value.abs() > 1.0 { value / 100.0 } else { value };
                self.amount = unit.clamp(-1.0, 1.0);
            }
        }
    }

    pub(crate) fn reset_field(&mut self, field: MacroField) {
        let defaults = MacroRoute::default();
        match field {
            MacroField::Target => self.target = defaults.target,
            MacroField::Amount => self.amount = defaults.amount,
        }
    }

    pub(crate) fn field_ratio(self, field: MacroField) -> f32 {
        match field {
            MacroField::Target => self.target_index() as f32 / MACRO_COUNT as f32,
            MacroField::Amount => (self.amount * 0.5 + 0.5).clamp(0.0, 1.0),
        }
    }

    pub(crate) fn field_display(self, field: MacroField) -> String {
        match field {
            MacroField::Target => match self.target {
                Some(t) => format!("macro {}", t + 1),
                None => "none".to_string(),
            },
            MacroField::Amount => format!("{:+.0}%", self.amount * 100.0),
        }
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
    open: Option<OpenEditor>,
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

    /// Close the editor; a route left at neutral amount is dead weight and is
    /// removed so it never colours the UI or the song code.
    pub(crate) fn close_editor(&mut self) {
        let Some(open) = self.open.take() else {
            return;
        };
        match open.kind {
            ModKind::Lfo => {
                if self
                    .routes
                    .get(&open.address)
                    .is_some_and(|route| route.depth_ratio <= f32::EPSILON)
                {
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

    #[cfg(test)]
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
    macro_mod: Option<(f32, f32)>,
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
    if let Some((amount, macro_value)) = macro_mod {
        value += macro_value.clamp(0.0, 1.0) * range * amount.clamp(-1.0, 1.0);
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
    modulated_control_value_full(spec, Some(route), None, None, base, ModContext::lfo_only(beat))
}

/// The `(amount, live macro value)` pair a control's macro route contributes,
/// or None when the route is missing/neutral. Reads the macro slider from
/// `controls`, so callers that want the macro's own modulation reflected must
/// apply it to `controls` first (`apply_automation` pass one does).
pub(crate) fn macro_contribution(
    automation: &AutomationState,
    controls: &FluidControls,
    address: ControlAddress,
) -> Option<(f32, f32)> {
    let route = automation.macro_route(address)?;
    let target = route.target.filter(|_| route.amount.abs() > f32::EPSILON)?;
    Some((route.amount, controls.macros.values[target]))
}

/// UI-side macro contribution: recomputes the macro slider's own modulated
/// value from raw controls, mirroring what `apply_automation` pass one
/// produces, so the marker shows what the engine hears.
pub(crate) fn live_macro_contribution(
    automation: &AutomationState,
    controls: &FluidControls,
    address: ControlAddress,
    ctx: ModContext,
) -> Option<(f32, f32)> {
    let route = automation.macro_route(address)?;
    let target = route.target.filter(|_| route.amount.abs() > f32::EPSILON)?;
    let spec = spec_by_id(MACRO_CONTROLS[target].id)
        .expect("macro sliders are registered controls");
    let macro_address = ControlAddress::new(spec.id);
    let value = modulated_control_value_full(
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
    Some((route.amount, value))
}

pub(crate) fn apply_automation(
    controls: &mut FluidControls,
    automation: &AutomationState,
    timing: TimingContext,
) {
    let ctx = ModContext {
        beat: timing.beat,
        kick_interval_beats: controls.kick.interval_beats,
        kick_offset_beats: controls.kick.offset_beats,
    };
    // Two passes: macro sliders modulate first so their targets read the
    // already-modulated macro values in the second pass.
    let addresses = automation.modulated_addresses();
    let (macro_sliders, targets): (Vec<_>, Vec<_>) = addresses
        .into_iter()
        .partition(|address| is_macro_id(address.id()));
    for address in macro_sliders.into_iter().chain(targets) {
        let spec = address.spec();
        let lfo = automation
            .route(address)
            .filter(|route| route.depth_ratio > f32::EPSILON);
        let envelope = automation
            .envelope(address)
            .filter(|route| route.amount.abs() > f32::EPSILON);
        let macro_mod = macro_contribution(automation, controls, address);
        if lfo.is_none() && envelope.is_none() && macro_mod.is_none() {
            continue;
        }
        let base = (spec.get)(controls);
        let value = modulated_control_value_full(spec, lfo, envelope, macro_mod, base, ctx);
        (spec.set)(controls, value);
    }
}
