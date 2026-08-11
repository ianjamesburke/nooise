//! Modulation routes keyed by stable control ID. Each route family owns its
//! own submodule; this root owns the address vocabulary they share, the
//! `AutomationState` that stores both, and the summing that turns them
//! into the value the engine plays.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use super::widget::DialScale;
use super::{
    ControlSpec, Entry, FluidControls, LfoSnap, TAPER_STEPS_PER_SWEEP, TimingContext,
    beat_grid_adjust, beat_grid_snap, nearest_power_of_two, snap_step, spec_by_id,
};

mod envelope;
mod lfo;

// The submodules split one flat module along its route families; their
// `pub(crate)` surface is this module's surface, unchanged by the split.
pub(crate) use envelope::*;
pub(crate) use lfo::*;

/// Stable key for a control or one of its automation fields.
pub(crate) fn unit_key(id: &str, field: Option<&str>) -> String {
    match field {
        Some(field) => format!("{id}#{field}"),
        None => id.to_string(),
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ControlAddress {
    spec: &'static ControlSpec,
}

impl ControlAddress {
    /// Resolve a registry control id to the one spec every route is keyed by.
    ///
    /// Infallible by construction rather than by luck: `id` is always a
    /// `ControlSpec::id` — read back off a spec, a `ControlItem`, or
    /// `Tab::level_id` — never a hand-typed literal that could drift. An
    /// unregistered id is a programming error with no sensible runtime
    /// recovery: silently dropping the route would lose the user's automation
    /// without telling anyone.
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

/// How one h/l press moves a continuous automation field.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Stepping {
    /// Add `step`, then clamp to the field's range.
    Linear,
    /// Add `step`, then snap back onto the field's own grid — for fields
    /// whose stored value is meant to stay on that grid.
    Snapped,
    /// Move to the next value of an explicit ordered ladder, so a field with
    /// musically uneven rungs steps rung to rung instead of by a raw amount.
    Ladder(&'static [f32]),
    /// Move to the next musical beat-grid value (0.125 floor, sixteenths up).
    BeatGrid,
    /// Move an equal fraction of a tapered throw, so one press covers the
    /// same share of the bar wherever the value currently sits.
    Position,
}

/// One continuous automation field's range, stepping, numeric entry, and
/// reset target — the same table treatment the registry gives control rows,
/// shared by LFO fields, envelope fields, and the inline step targets.
/// Discrete fields (LFO shape, envelope trigger) are enums cycled by index
/// and carry no spec.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct FieldSpec<F: 'static> {
    pub(super) field: F,
    pub(super) label: &'static str,
    pub(super) min: f32,
    pub(super) max: f32,
    pub(super) step: f32,
    /// How the field maps onto bar position; display and stepping read the
    /// same scale so they cannot disagree.
    pub(super) scale: DialScale,
    pub(super) stepping: Stepping,
    pub(super) entry: Entry,
    pub(super) reset: f32,
}

impl<F: Copy + PartialEq> FieldSpec<F> {
    /// One h/l press.
    pub(super) fn adjust(&self, value: f32, dir: f32) -> f32 {
        match self.stepping {
            Stepping::Linear => (value + dir * self.step).clamp(self.min, self.max),
            Stepping::Snapped => self.quantize(value + dir * self.step),
            Stepping::Ladder(rungs) => self.next_rung(rungs, value, dir),
            Stepping::BeatGrid => beat_grid_adjust(value, dir, self.min, self.max),
            Stepping::Position => self
                .scale
                .step_in_position(value, dir, TAPER_STEPS_PER_SWEEP)
                .expect("a position-stepped field is tapered, so it has an inverse")
                .clamp(self.min, self.max),
        }
    }

    /// Numeric entry, in the field's own unit.
    pub(super) fn parse_value(&self, value: f32) -> f32 {
        match self.entry {
            Entry::Percent => (value / 100.0).clamp(self.min, self.max),
            Entry::Snap => self.quantize(value),
            Entry::Round => value.round().clamp(self.min, self.max),
            // No automation field is stored in bars, so BeatsAsBars carries
            // no extra meaning here and reads as a plain exact value.
            Entry::BeatsAsBars | Entry::Free => value.clamp(self.min, self.max),
        }
    }

    pub(super) fn quantize(&self, value: f32) -> f32 {
        if matches!(self.scale, DialScale::BeatGrid { .. }) {
            beat_grid_snap(value, self.min, self.max)
        } else {
            snap_step(value.clamp(self.min, self.max), self.step).clamp(self.min, self.max)
        }
    }

    fn next_rung(&self, rungs: &[f32], value: f32, dir: f32) -> f32 {
        if dir > 0.0 {
            rungs
                .iter()
                .copied()
                .find(|rung| *rung > value + f32::EPSILON)
                .unwrap_or(self.max)
        } else {
            rungs
                .iter()
                .rev()
                .copied()
                .find(|rung| *rung < value - f32::EPSILON)
                .unwrap_or(self.min)
        }
    }
}

/// Which modulator editor is currently open on a control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModKind {
    Lfo,
    Envelope,
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
pub(crate) fn stepped_index(index: usize, dir: f32, len: usize) -> usize {
    let next = index as i64 + i64::from(dir.signum() as i32);
    next.clamp(0, len.saturating_sub(1) as i64) as usize
}

/// Numeric entry for a discrete field: round and clamp to the valid range.
pub(crate) fn clamped_index(index: f32, len: usize) -> usize {
    (index.round() as i64).clamp(0, len.saturating_sub(1) as i64) as usize
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
// ============================================================
// Automation state
// ============================================================

#[derive(Clone, Copy, Debug, PartialEq)]
struct OpenEditor {
    address: ControlAddress,
    kind: ModKind,
}

#[derive(Clone, Default, PartialEq)]
pub(crate) struct AutomationState {
    routes: BTreeMap<ControlAddress, LfoRoute>,
    envelopes: BTreeMap<ControlAddress, EnvelopeRoute>,
    open: Option<OpenEditor>,
}

/// One family of modulation route stored on `AutomationState`. The two
/// families differ only in which map they live in, how a fresh route is
/// seeded, and what counts as neutral, so every accessor and the close-time
/// prune below is written once against this instead of three times.
pub(super) trait Route: Copy + Sized + 'static {
    const KIND: ModKind;

    fn map(state: &AutomationState) -> &BTreeMap<ControlAddress, Self>;
    fn map_mut(state: &mut AutomationState) -> &mut BTreeMap<ControlAddress, Self>;
    /// The route the editor creates when opened on a control with none.
    fn fresh(address: ControlAddress) -> Self;
    /// A neutral route contributes nothing; closing its editor prunes it.
    fn is_neutral(&self) -> bool;
}

impl Route for LfoRoute {
    const KIND: ModKind = ModKind::Lfo;

    fn map(state: &AutomationState) -> &BTreeMap<ControlAddress, Self> {
        &state.routes
    }

    fn map_mut(state: &mut AutomationState) -> &mut BTreeMap<ControlAddress, Self> {
        &mut state.routes
    }

    fn fresh(address: ControlAddress) -> Self {
        Self::with_seed(seed_for_id(address.id()))
    }

    fn is_neutral(&self) -> bool {
        self.depth_ratio <= f32::EPSILON
    }
}

impl Route for EnvelopeRoute {
    const KIND: ModKind = ModKind::Envelope;

    fn map(state: &AutomationState) -> &BTreeMap<ControlAddress, Self> {
        &state.envelopes
    }

    fn map_mut(state: &mut AutomationState) -> &mut BTreeMap<ControlAddress, Self> {
        &mut state.envelopes
    }

    fn fresh(_address: ControlAddress) -> Self {
        Self::default()
    }

    fn is_neutral(&self) -> bool {
        self.amount.abs() <= f32::EPSILON
    }
}

impl AutomationState {
    fn open_or_create_route<R: Route>(&mut self, address: ControlAddress) -> &mut R {
        self.open = Some(OpenEditor {
            address,
            kind: R::KIND,
        });
        R::map_mut(self)
            .entry(address)
            .or_insert_with(|| R::fresh(address))
    }

    fn route_of<R: Route>(&self, address: ControlAddress) -> Option<&R> {
        R::map(self).get(&address)
    }

    fn route_of_mut<R: Route>(&mut self, address: ControlAddress) -> Option<&mut R> {
        R::map_mut(self).get_mut(&address)
    }

    fn set_route_of<R: Route>(&mut self, address: ControlAddress, route: R) {
        R::map_mut(self).insert(address, route);
    }

    fn routes_of<R: Route>(&self) -> impl Iterator<Item = (ControlAddress, &R)> {
        R::map(self)
            .iter()
            .map(|(address, route)| (*address, route))
    }

    fn remove_route_of<R: Route>(&mut self, address: ControlAddress) {
        R::map_mut(self).remove(&address);
    }

    /// Drop the route if it is neutral; a route left contributing nothing is
    /// dead weight that would still colour the UI and the song code.
    fn prune_neutral_route<R: Route>(&mut self, address: ControlAddress) {
        if R::map(self).get(&address).is_some_and(R::is_neutral) {
            R::map_mut(self).remove(&address);
        }
    }

    pub(crate) fn open_or_create(&mut self, address: ControlAddress) -> &mut LfoRoute {
        self.open_or_create_route(address)
    }

    pub(crate) fn open_or_create_envelope(
        &mut self,
        address: ControlAddress,
    ) -> &mut EnvelopeRoute {
        self.open_or_create_route(address)
    }

    /// Remove the route backing the open editor and close it. The x gesture:
    /// explicit, worked on the first try, unlike double-tap.
    pub(crate) fn remove_open_route(&mut self) {
        let Some(open) = self.open.take() else {
            return;
        };
        match open.kind {
            ModKind::Lfo => self.remove_route_of::<LfoRoute>(open.address),
            ModKind::Envelope => self.remove_route_of::<EnvelopeRoute>(open.address),
        }
    }

    /// Strip every modulator from a control, closing the editor if it was open.
    pub(crate) fn clear_control(&mut self, address: ControlAddress) {
        self.routes.remove(&address);
        self.envelopes.remove(&address);
        if self.open.is_some_and(|open| open.address == address) {
            self.open = None;
        }
    }

    /// Close the editor; a route left at neutral amount is dead weight and is
    /// removed so it never colours the UI or the song code.
    pub(crate) fn close_editor(&mut self) {
        let Some(open) = self.open.take() else {
            return;
        };
        match open.kind {
            ModKind::Lfo => self.prune_neutral_route::<LfoRoute>(open.address),
            ModKind::Envelope => self.prune_neutral_route::<EnvelopeRoute>(open.address),
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
        self.route_of(address)
    }

    pub(crate) fn route_mut(&mut self, address: ControlAddress) -> Option<&mut LfoRoute> {
        self.route_of_mut(address)
    }

    pub(crate) fn set_route(&mut self, address: ControlAddress, route: LfoRoute) {
        self.set_route_of(address, route);
    }

    pub(crate) fn routes(&self) -> impl Iterator<Item = (ControlAddress, &LfoRoute)> {
        self.routes_of()
    }

    pub(crate) fn envelope(&self, address: ControlAddress) -> Option<&EnvelopeRoute> {
        self.route_of(address)
    }

    pub(crate) fn envelope_mut(&mut self, address: ControlAddress) -> Option<&mut EnvelopeRoute> {
        self.route_of_mut(address)
    }

    pub(crate) fn set_envelope(&mut self, address: ControlAddress, route: EnvelopeRoute) {
        self.set_route_of(address, route);
    }

    pub(crate) fn envelopes(&self) -> impl Iterator<Item = (ControlAddress, &EnvelopeRoute)> {
        self.routes_of()
    }

    fn modulated_addresses(&self) -> BTreeSet<ControlAddress> {
        self.routes
            .keys()
            .chain(self.envelopes.keys())
            .copied()
            .collect()
    }

    /// Morphed automation state for a leg transition between `from` and `to`,
    /// the `AutomationState` counterpart to `MorphState::controls_at`'s
    /// per-`FluidControls`-field glide/snap split: `tt` (0..1) is the glide
    /// fraction for each route's level field (`LfoRoute::depth_ratio` or
    /// `EnvelopeRoute::amount`), and `use_to`
    /// selects which side's other fields (shape, cycle, attack/decay,
    /// trigger, …) are live — false holds `from`'s, true snaps to `to`'s, all
    /// together at the transition downbeat, mirroring
    /// `STRUCTURAL_SNAP_IDS`. A route present on only one side fades in or
    /// out via the level field rather than popping, and never needs explicit
    /// removal: once this leg's `to` becomes the next leg's `from`, an
    /// absent route is simply absent from the map again. Editor-open state
    /// (`open`) is UI navigation, not audible, and is never morphed.
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
/// LFO plus envelope, summed, clamped to range, then snapped per the control's
/// `LfoSnap`. The UI's modulation marker must go through this too so it shows
/// what is heard.
pub(crate) fn modulated_control_value_full(
    spec: &ControlSpec,
    lfo: Option<&LfoRoute>,
    envelope: Option<&EnvelopeRoute>,
    base: f32,
    ctx: ModContext,
) -> f32 {
    // Every source contributes a signed fraction of the dial's throw, summed
    // once. Applying that sum in position space is what makes a depth mean a
    // fixed musical amount: a flat offset in raw value space made "50%" swing
    // four octaves up from a low cutoff and nothing at all from a high one,
    // because the control's own taper was ignored.
    let mut delta = 0.0;
    if let Some(route) = lfo {
        delta += route.wave_at(ctx.beat) * route.depth_ratio.clamp(0.0, 1.0);
    }
    if let Some(route) = envelope {
        delta += route.level_at(ctx) * route.amount.clamp(-1.0, 1.0);
    }
    let scale = DialScale::from_step(spec.min, spec.max, spec.step, spec.taper);
    // Grid and rung scales have no inverse, so they keep the value-space
    // offset. Their taper is Linear anyway, so nothing else changes.
    let value = scale
        .offset_in_position(base, delta)
        .unwrap_or(base + delta * (spec.max - spec.min));
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
    modulated_control_value_full(spec, Some(route), None, base, ModContext::lfo_only(beat))
}

/// One modulated control's routes, resolved to plain copies so applying
/// them per sample needs no map lookups, string keys, or heap.
struct PlannedRoute {
    spec: &'static ControlSpec,
    lfo: Option<LfoRoute>,
    envelope: Option<EnvelopeRoute>,
}

/// Allocation-free application plan for an `AutomationState`. The engine
/// rebuilds it only when the published automation Arc changes (a UI edit),
/// so the per-sample audio hot path never touches the allocator.
#[derive(Default)]
pub(crate) struct AutomationPlan {
    routes: Vec<PlannedRoute>,
}

impl AutomationPlan {
    pub(crate) fn rebuild(&mut self, automation: &AutomationState) {
        self.routes.clear();
        let addresses = automation.modulated_addresses();
        for address in addresses {
            let lfo = automation.route(address).copied();
            self.routes.push(PlannedRoute {
                spec: address.spec(),
                lfo,
                envelope: automation.envelope(address).copied(),
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
            let lfo = planned
                .lfo
                .as_ref()
                .filter(|route| route.depth_ratio > f32::EPSILON);
            let envelope = planned
                .envelope
                .as_ref()
                .filter(|route| route.amount.abs() > f32::EPSILON);
            if lfo.is_none() && envelope.is_none() {
                continue;
            }
            let spec = planned.spec.contextual(controls);
            let base = (spec.get)(controls);
            let value = modulated_control_value_full(&spec, lfo, envelope, base, ctx);
            (spec.set)(controls, value);
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
