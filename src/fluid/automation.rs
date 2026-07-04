use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;

use super::{
    ControlSpec, FluidControls, LfoSnap, TimingContext, nearest_power_of_two, normalize_unit_input,
    snap_step, spec_by_id,
};

pub(crate) const DEFAULT_LFO_CYCLE_BEATS: f32 = 2.0;
pub(crate) const DEFAULT_LFO_DEPTH_RATIO: f32 = 0.0;
pub(crate) const MIN_LFO_CYCLE_BEATS: f32 = 0.25;
pub(crate) const MAX_LFO_CYCLE_BEATS: f32 = 16.0;
pub(crate) const MAX_LFO_OFFSET_BEATS: f32 = 4.0;

const AMOUNT_STEP: f32 = 0.01;
const INTERVAL_STEP: f32 = 0.25;
const OFFSET_STEP: f32 = 0.25;

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

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum LfoShape {
    Sine,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LfoField {
    Amount,
    Interval,
    Offset,
}

impl LfoField {
    pub(crate) const ALL: [LfoField; 3] = [Self::Amount, Self::Interval, Self::Offset];

    pub(crate) fn label(self) -> &'static str {
        self.spec().label
    }

    pub(crate) fn spec(self) -> &'static LfoFieldSpec {
        LFO_FIELD_SPECS
            .iter()
            .find(|spec| spec.field == self)
            .expect("every LFO field has a spec")
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
}

impl Default for LfoRoute {
    fn default() -> Self {
        Self {
            depth_ratio: DEFAULT_LFO_DEPTH_RATIO,
            cycle_beats: DEFAULT_LFO_CYCLE_BEATS,
            phase_offset_beats: 0.0,
            shape: LfoShape::Sine,
        }
    }
}

impl LfoRoute {
    pub(crate) fn phase_at(&self, beat: f64) -> f64 {
        ((beat + f64::from(self.phase_offset_beats))
            / f64::from(self.cycle_beats.max(MIN_LFO_CYCLE_BEATS)))
        .rem_euclid(1.0)
    }

    /// Oscillator output in -1..1 at the given beat; depth scaling is the caller's job.
    pub(crate) fn wave_at(&self, beat: f64) -> f32 {
        match self.shape {
            LfoShape::Sine => (std::f64::consts::TAU * self.phase_at(beat)).sin() as f32,
        }
    }

    pub(crate) fn adjust_field_at(&mut self, field: LfoField, dir: f32, beat: f64) {
        let spec = field.spec();
        match field {
            LfoField::Amount => {
                self.depth_ratio = spec.adjust(self.depth_ratio, dir);
            }
            LfoField::Interval => {
                self.set_cycle_preserving_phase(spec.adjust(self.cycle_beats, dir), beat);
            }
            LfoField::Offset => {
                self.phase_offset_beats = spec.adjust(self.phase_offset_beats, dir);
            }
        }
    }

    pub(crate) fn set_field_at(&mut self, field: LfoField, value: f32, beat: f64) {
        let spec = field.spec();
        match field {
            LfoField::Amount => self.depth_ratio = spec.parse_value(value),
            LfoField::Interval => {
                self.set_cycle_preserving_phase(spec.parse_value(value), beat);
            }
            LfoField::Offset => {
                self.phase_offset_beats = spec.parse_value(value);
            }
        }
    }

    pub(crate) fn reset_field_at(&mut self, field: LfoField, beat: f64) {
        let reset = field.spec().reset;
        match field {
            LfoField::Amount => self.depth_ratio = reset,
            LfoField::Interval => self.set_cycle_preserving_phase(reset, beat),
            LfoField::Offset => self.phase_offset_beats = reset,
        }
    }

    pub(crate) fn field_ratio(&self, field: LfoField) -> f32 {
        let spec = field.spec();
        match field {
            LfoField::Amount => spec.ratio(self.depth_ratio),
            LfoField::Interval => spec.ratio(self.cycle_beats),
            LfoField::Offset => spec.ratio(self.phase_offset_beats),
        }
    }

    pub(crate) fn field_display(&self, field: LfoField) -> String {
        match field {
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

#[derive(Clone, Default)]
pub(crate) struct AutomationState {
    routes: BTreeMap<ControlAddress, LfoRoute>,
    open: Option<ControlAddress>,
}

impl AutomationState {
    pub(crate) fn open_or_create(&mut self, address: ControlAddress) -> &mut LfoRoute {
        let route = self.routes.entry(address).or_default();
        self.open = Some(address);
        route
    }

    /// Close the editor; a route left at zero depth is dead weight and is removed.
    pub(crate) fn close_editor(&mut self) {
        if let Some(address) = self.open.take()
            && self
                .routes
                .get(&address)
                .is_some_and(|route| route.depth_ratio <= f32::EPSILON)
        {
            self.routes.remove(&address);
        }
    }

    pub(crate) fn is_editor_open(&self) -> bool {
        self.open.is_some()
    }

    pub(crate) fn active_address(&self) -> Option<ControlAddress> {
        self.open
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
}

/// The effective value the engine plays for a modulated control: base plus
/// LFO, clamped to range, then snapped per the control's `LfoSnap`. The UI's
/// modulation marker must go through this too so it shows what is heard.
pub(crate) fn modulated_control_value(
    spec: &ControlSpec,
    route: &LfoRoute,
    base: f32,
    beat: f64,
) -> f32 {
    let depth = (spec.max - spec.min) * route.depth_ratio.clamp(0.0, 1.0);
    let modulated = (base + route.wave_at(beat) * depth).clamp(spec.min, spec.max);
    match spec.lfo_snap {
        LfoSnap::None => modulated,
        LfoSnap::PowerOfTwo => nearest_power_of_two(modulated, spec.min, spec.max),
        LfoSnap::Step => spec.quantize(modulated),
    }
}

pub(crate) fn apply_automation(
    controls: &mut FluidControls,
    automation: &AutomationState,
    timing: TimingContext,
) {
    for (address, route) in automation.routes() {
        let spec = address.spec();
        if route.depth_ratio <= f32::EPSILON {
            continue;
        }
        let base = (spec.get)(controls);
        let value = modulated_control_value(spec, route, base, timing.beat);
        (spec.set)(controls, value);
    }
}
