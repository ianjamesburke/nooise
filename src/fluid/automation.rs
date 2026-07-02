use std::collections::BTreeMap;

use super::{FluidControls, TimingContext, normalize_unit_input, spec_by_id};

pub(crate) const DEFAULT_LFO_CYCLE_BEATS: f32 = 2.0;
pub(crate) const DEFAULT_LFO_DEPTH_RATIO: f32 = 0.25;
pub(crate) const MIN_LFO_CYCLE_BEATS: f32 = 0.25;
pub(crate) const INTERVAL_LADDER: [f32; 7] = [0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0];

const AMOUNT_STEP: f32 = 0.05;
const OFFSET_STEP: f32 = 0.125;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct ControlAddress {
    id: &'static str,
}

impl ControlAddress {
    pub(crate) const fn new(id: &'static str) -> Self {
        Self { id }
    }

    pub(crate) fn id(self) -> &'static str {
        self.id
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
        match self {
            Self::Amount => "amount",
            Self::Interval => "interval",
            Self::Offset => "offset",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LfoRoute {
    pub(crate) depth_ratio: f32,
    pub(crate) cycle_beats: f32,
    pub(crate) phase_offset_cycles: f32,
    pub(crate) shape: LfoShape,
}

impl Default for LfoRoute {
    fn default() -> Self {
        Self {
            depth_ratio: DEFAULT_LFO_DEPTH_RATIO,
            cycle_beats: DEFAULT_LFO_CYCLE_BEATS,
            phase_offset_cycles: 0.0,
            shape: LfoShape::Sine,
        }
    }
}

impl LfoRoute {
    pub(crate) fn phase_at(&self, beat: f64) -> f64 {
        (beat / f64::from(self.cycle_beats.max(MIN_LFO_CYCLE_BEATS))
            + f64::from(self.phase_offset_cycles))
        .rem_euclid(1.0)
    }

    /// Oscillator output in -1..1 at the given beat; depth scaling is the caller's job.
    pub(crate) fn wave_at(&self, beat: f64) -> f32 {
        match self.shape {
            LfoShape::Sine => (std::f64::consts::TAU * self.phase_at(beat)).sin() as f32,
        }
    }

    fn ladder_index(&self) -> usize {
        nearest_ladder_index(self.cycle_beats)
    }

    pub(crate) fn adjust_field(&mut self, field: LfoField, dir: f32) {
        match field {
            LfoField::Amount => {
                self.depth_ratio = (self.depth_ratio + dir * AMOUNT_STEP).clamp(0.0, 1.0);
            }
            LfoField::Interval => {
                let index = self.ladder_index();
                let next = if dir > 0.0 {
                    (index + 1).min(INTERVAL_LADDER.len() - 1)
                } else {
                    index.saturating_sub(1)
                };
                self.cycle_beats = INTERVAL_LADDER[next];
            }
            LfoField::Offset => {
                self.phase_offset_cycles =
                    (self.phase_offset_cycles + dir * OFFSET_STEP).rem_euclid(1.0);
            }
        }
    }

    pub(crate) fn set_field(&mut self, field: LfoField, value: f32) {
        match field {
            LfoField::Amount => self.depth_ratio = normalize_unit_input(value),
            LfoField::Interval => {
                self.cycle_beats = INTERVAL_LADDER[nearest_ladder_index(value)];
            }
            LfoField::Offset => self.phase_offset_cycles = value.rem_euclid(1.0),
        }
    }

    pub(crate) fn reset_field(&mut self, field: LfoField) {
        match field {
            LfoField::Amount => self.depth_ratio = DEFAULT_LFO_DEPTH_RATIO,
            LfoField::Interval => self.cycle_beats = DEFAULT_LFO_CYCLE_BEATS,
            LfoField::Offset => self.phase_offset_cycles = 0.0,
        }
    }

    pub(crate) fn field_ratio(&self, field: LfoField) -> f32 {
        match field {
            LfoField::Amount => self.depth_ratio,
            LfoField::Interval => self.ladder_index() as f32 / (INTERVAL_LADDER.len() - 1) as f32,
            LfoField::Offset => self.phase_offset_cycles,
        }
    }

    pub(crate) fn field_display(&self, field: LfoField) -> String {
        match field {
            LfoField::Amount => format!("{:.0}%", self.depth_ratio * 100.0),
            LfoField::Interval => format!("{:.2} beats", self.cycle_beats),
            LfoField::Offset => format!("{:.2} cyc", self.phase_offset_cycles),
        }
    }
}

fn nearest_ladder_index(value: f32) -> usize {
    INTERVAL_LADDER
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            (**a - value)
                .abs()
                .partial_cmp(&(**b - value).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, _)| i)
        .unwrap_or(0)
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

pub(crate) fn apply_automation(
    controls: &mut FluidControls,
    automation: &AutomationState,
    timing: TimingContext,
) {
    for (address, route) in automation.routes() {
        let Some(spec) = spec_by_id(address.id()) else {
            continue;
        };
        let depth = (spec.max - spec.min) * route.depth_ratio.clamp(0.0, 1.0);
        if depth <= f32::EPSILON {
            continue;
        }
        let base = (spec.get)(controls);
        let offset = route.wave_at(timing.beat) * depth;
        (spec.set)(controls, (base + offset).clamp(spec.min, spec.max));
    }
}
