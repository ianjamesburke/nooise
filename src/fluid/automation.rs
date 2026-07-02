use std::collections::BTreeMap;

use super::{FluidControls, TimingContext, spec_by_id};

pub(crate) const DEFAULT_LFO_CYCLE_BEATS: f32 = 2.0;
pub(crate) const DEFAULT_LFO_TARGET_DEPTH_RATIO: f32 = 0.10;
pub(crate) const DEFAULT_LFO_EFFECTIVE_DEPTH_RATIO: f32 = 0.0;

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
pub(crate) struct LfoRoute {
    pub(crate) cycle_beats: f32,
    pub(crate) target_depth_ratio: f32,
    pub(crate) effective_depth_ratio: f32,
}

impl Default for LfoRoute {
    fn default() -> Self {
        Self {
            cycle_beats: DEFAULT_LFO_CYCLE_BEATS,
            target_depth_ratio: DEFAULT_LFO_TARGET_DEPTH_RATIO,
            effective_depth_ratio: DEFAULT_LFO_EFFECTIVE_DEPTH_RATIO,
        }
    }
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

    pub(crate) fn close_editor(&mut self) {
        self.open = None;
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
        let base = (spec.get)(controls);
        let depth = (spec.max - spec.min) * route.effective_depth_ratio.clamp(0.0, 1.0);
        if depth <= f32::EPSILON {
            continue;
        }
        let cycle_beats = f64::from(route.cycle_beats.max(1.0 / 64.0));
        let phase = (timing.beat / cycle_beats).rem_euclid(1.0);
        let offset = (std::f64::consts::TAU * phase).sin() as f32 * depth;
        (spec.set)(controls, (base + offset).clamp(spec.min, spec.max));
    }
}
