use std::collections::BTreeMap;

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

#[derive(Default)]
pub(crate) struct AutomationState {
    routes: BTreeMap<ControlAddress, LfoRoute>,
    open: Option<ControlAddress>,
}

impl AutomationState {
    pub(crate) fn open_or_create(&mut self, address: ControlAddress) -> &LfoRoute {
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
}
