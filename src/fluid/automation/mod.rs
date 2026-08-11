//! Bounded automation stacks keyed by stable control ID. Each route family
//! owns its submodule; this root owns shared addresses, lane storage,
//! position-space summing, and the audio-side de-clicker.

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
    index: usize,
}

pub(crate) const MAX_AUTOMATION_LANES_PER_KIND: usize = 4;

#[derive(Clone, Default, PartialEq)]
struct AutomationStack {
    lfos: Vec<LfoRoute>,
    envelopes: Vec<EnvelopeRoute>,
}

#[derive(Clone, Default, PartialEq)]
pub(crate) struct AutomationState {
    stacks: BTreeMap<ControlAddress, AutomationStack>,
    open: Option<OpenEditor>,
}

impl AutomationState {
    fn remove_stack_if_empty(&mut self, address: ControlAddress) {
        if self
            .stacks
            .get(&address)
            .is_some_and(|stack| stack.lfos.is_empty() && stack.envelopes.is_empty())
        {
            self.stacks.remove(&address);
        }
    }

    pub(crate) fn open_or_create(&mut self, address: ControlAddress) -> &mut LfoRoute {
        self.open = Some(OpenEditor {
            address,
            kind: ModKind::Lfo,
            index: 0,
        });
        let stack = self.stacks.entry(address).or_default();
        if stack.lfos.is_empty() {
            stack
                .lfos
                .push(LfoRoute::with_seed(seed_for_id(address.id())));
        }
        &mut stack.lfos[0]
    }

    pub(crate) fn open_or_create_envelope(
        &mut self,
        address: ControlAddress,
    ) -> &mut EnvelopeRoute {
        self.open = Some(OpenEditor {
            address,
            kind: ModKind::Envelope,
            index: 0,
        });
        let stack = self.stacks.entry(address).or_default();
        if stack.envelopes.is_empty() {
            stack.envelopes.push(EnvelopeRoute::default());
        }
        &mut stack.envelopes[0]
    }

    pub(crate) fn add_route(&mut self, address: ControlAddress, route: LfoRoute) -> bool {
        let stack = self.stacks.entry(address).or_default();
        if stack.lfos.len() >= MAX_AUTOMATION_LANES_PER_KIND {
            return false;
        }
        stack.lfos.push(route);
        true
    }

    pub(crate) fn add_envelope(&mut self, address: ControlAddress, route: EnvelopeRoute) -> bool {
        let stack = self.stacks.entry(address).or_default();
        if stack.envelopes.len() >= MAX_AUTOMATION_LANES_PER_KIND {
            return false;
        }
        stack.envelopes.push(route);
        true
    }

    pub(crate) fn add_and_open(&mut self, address: ControlAddress, kind: ModKind) -> bool {
        let index = match kind {
            ModKind::Lfo => self.routes_for(address).count(),
            ModKind::Envelope => self.envelopes_for(address).count(),
        };
        let added = match kind {
            ModKind::Lfo => self.add_route(
                address,
                LfoRoute::with_seed(seed_for_id(address.id()).wrapping_add(index as u32)),
            ),
            ModKind::Envelope => self.add_envelope(address, EnvelopeRoute::default()),
        };
        if added {
            self.open = Some(OpenEditor {
                address,
                kind,
                index,
            });
        }
        added
    }

    pub(crate) fn cycle_open(&mut self, address: ControlAddress, kind: ModKind) {
        let len = match kind {
            ModKind::Lfo => self.routes_for(address).count(),
            ModKind::Envelope => self.envelopes_for(address).count(),
        };
        if len == 0 {
            match kind {
                ModKind::Lfo => {
                    self.open_or_create(address);
                }
                ModKind::Envelope => {
                    self.open_or_create_envelope(address);
                }
            }
            return;
        }
        let next = self
            .open
            .filter(|open| open.address == address && open.kind == kind)
            .map_or(0, |open| (open.index + 1) % len);
        self.open = Some(OpenEditor {
            address,
            kind,
            index: next,
        });
    }

    /// Remove the route backing the open editor and close it. The x gesture:
    /// explicit, worked on the first try, unlike double-tap.
    pub(crate) fn remove_open_route(&mut self) {
        let Some(open) = self.open.take() else {
            return;
        };
        if let Some(stack) = self.stacks.get_mut(&open.address) {
            match open.kind {
                ModKind::Lfo if open.index < stack.lfos.len() => {
                    stack.lfos.remove(open.index);
                }
                ModKind::Envelope if open.index < stack.envelopes.len() => {
                    stack.envelopes.remove(open.index);
                }
                ModKind::Lfo | ModKind::Envelope => {}
            }
        }
        self.remove_stack_if_empty(open.address);
    }

    /// Strip every modulator from a control, closing the editor if it was open.
    pub(crate) fn clear_control(&mut self, address: ControlAddress) {
        self.stacks.remove(&address);
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
        if let Some(stack) = self.stacks.get_mut(&open.address) {
            match open.kind {
                ModKind::Lfo
                    if stack
                        .lfos
                        .get(open.index)
                        .is_some_and(|route| route.depth_ratio <= f32::EPSILON) =>
                {
                    stack.lfos.remove(open.index);
                }
                ModKind::Envelope
                    if stack
                        .envelopes
                        .get(open.index)
                        .is_some_and(|route| route.amount.abs() <= f32::EPSILON) =>
                {
                    stack.envelopes.remove(open.index);
                }
                ModKind::Lfo | ModKind::Envelope => {}
            }
        }
        self.remove_stack_if_empty(open.address);
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

    pub(crate) fn active_lane_index(&self) -> Option<usize> {
        self.open.map(|open| open.index)
    }

    pub(crate) fn active_lane_count(&self) -> Option<usize> {
        let open = self.open?;
        Some(match open.kind {
            ModKind::Lfo => self.routes_for(open.address).count(),
            ModKind::Envelope => self.envelopes_for(open.address).count(),
        })
    }

    pub(crate) fn route(&self, address: ControlAddress) -> Option<&LfoRoute> {
        let index = self
            .open
            .filter(|open| open.address == address && open.kind == ModKind::Lfo)
            .map_or(0, |open| open.index);
        self.stacks.get(&address)?.lfos.get(index)
    }

    pub(crate) fn route_mut(&mut self, address: ControlAddress) -> Option<&mut LfoRoute> {
        let index = self
            .open
            .filter(|open| open.address == address && open.kind == ModKind::Lfo)
            .map_or(0, |open| open.index);
        self.stacks.get_mut(&address)?.lfos.get_mut(index)
    }

    #[cfg(test)]
    pub(crate) fn set_route(&mut self, address: ControlAddress, route: LfoRoute) {
        let routes = &mut self.stacks.entry(address).or_default().lfos;
        if let Some(first) = routes.first_mut() {
            *first = route;
        } else {
            routes.push(route);
        }
    }

    pub(crate) fn routes(&self) -> impl Iterator<Item = (ControlAddress, &LfoRoute)> {
        self.stacks
            .iter()
            .flat_map(|(address, stack)| stack.lfos.iter().map(move |route| (*address, route)))
    }

    pub(crate) fn routes_for(&self, address: ControlAddress) -> impl Iterator<Item = &LfoRoute> {
        self.stacks
            .get(&address)
            .into_iter()
            .flat_map(|stack| stack.lfos.iter())
    }

    pub(crate) fn lfo_lanes(&self, address: ControlAddress) -> &[LfoRoute] {
        self.stacks
            .get(&address)
            .map_or(&[], |stack| stack.lfos.as_slice())
    }

    pub(crate) fn envelope(&self, address: ControlAddress) -> Option<&EnvelopeRoute> {
        let index = self
            .open
            .filter(|open| open.address == address && open.kind == ModKind::Envelope)
            .map_or(0, |open| open.index);
        self.stacks.get(&address)?.envelopes.get(index)
    }

    pub(crate) fn envelope_mut(&mut self, address: ControlAddress) -> Option<&mut EnvelopeRoute> {
        let index = self
            .open
            .filter(|open| open.address == address && open.kind == ModKind::Envelope)
            .map_or(0, |open| open.index);
        self.stacks.get_mut(&address)?.envelopes.get_mut(index)
    }

    #[cfg(test)]
    pub(crate) fn set_envelope(&mut self, address: ControlAddress, route: EnvelopeRoute) {
        let routes = &mut self.stacks.entry(address).or_default().envelopes;
        if let Some(first) = routes.first_mut() {
            *first = route;
        } else {
            routes.push(route);
        }
    }

    pub(crate) fn envelopes(&self) -> impl Iterator<Item = (ControlAddress, &EnvelopeRoute)> {
        self.stacks
            .iter()
            .flat_map(|(address, stack)| stack.envelopes.iter().map(move |route| (*address, route)))
    }

    pub(crate) fn envelopes_for(
        &self,
        address: ControlAddress,
    ) -> impl Iterator<Item = &EnvelopeRoute> {
        self.stacks
            .get(&address)
            .into_iter()
            .flat_map(|stack| stack.envelopes.iter())
    }

    pub(crate) fn envelope_lanes(&self, address: ControlAddress) -> &[EnvelopeRoute] {
        self.stacks
            .get(&address)
            .map_or(&[], |stack| stack.envelopes.as_slice())
    }

    fn modulated_addresses(&self) -> BTreeSet<ControlAddress> {
        self.stacks.keys().copied().collect()
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
        let addresses: BTreeSet<_> = from
            .stacks
            .keys()
            .chain(to.stacks.keys())
            .copied()
            .collect();
        for address in addresses {
            let from_stack = from.stacks.get(&address);
            let to_stack = to.stacks.get(&address);
            let mut stack = AutomationStack::default();
            morph_lane_family(
                from_stack.map_or(&[], |stack| stack.lfos.as_slice()),
                to_stack.map_or(&[], |stack| stack.lfos.as_slice()),
                &mut stack.lfos,
                |f, t| LfoRoute::morph(f, t, tt, use_to),
            );
            morph_lane_family(
                from_stack.map_or(&[], |stack| stack.envelopes.as_slice()),
                to_stack.map_or(&[], |stack| stack.envelopes.as_slice()),
                &mut stack.envelopes,
                |f, t| EnvelopeRoute::morph(f, t, tt, use_to),
            );
            if !stack.lfos.is_empty() || !stack.envelopes.is_empty() {
                result.stacks.insert(address, stack);
            }
        }
        result
    }
}

fn morph_lane_family<T>(
    from: &[T],
    to: &[T],
    out: &mut Vec<T>,
    morph: impl Fn(Option<&T>, Option<&T>) -> Option<T>,
) where
    T: Copy,
{
    for index in 0..from.len().max(to.len()) {
        if let Some(route) = morph(from.get(index), to.get(index)) {
            out.push(route);
        }
    }
}

/// The effective value the engine plays for a modulated control: base plus
/// LFO plus envelope, summed, clamped to range, then snapped per the control's
/// `LfoSnap`. The UI's modulation marker must go through this too so it shows
/// what is heard.
pub(crate) fn modulated_control_value_full(
    spec: &ControlSpec,
    lfos: &[LfoRoute],
    envelopes: &[EnvelopeRoute],
    base: f32,
    ctx: ModContext,
) -> f32 {
    // Every source contributes a signed fraction of the dial's throw, summed
    // once. Applying that sum in position space is what makes a depth mean a
    // fixed musical amount: a flat offset in raw value space made "50%" swing
    // four octaves up from a low cutoff and nothing at all from a high one,
    // because the control's own taper was ignored.
    let delta = automation_delta(lfos, envelopes, ctx);
    modulated_control_value_from_delta(spec, base, delta)
}

fn automation_delta(lfos: &[LfoRoute], envelopes: &[EnvelopeRoute], ctx: ModContext) -> f32 {
    let lfo_delta: f32 = lfos
        .iter()
        .filter(|route| route.depth_ratio > f32::EPSILON)
        .map(|route| route.wave_at(ctx.beat) * route.depth_ratio.clamp(0.0, 1.0))
        .sum();
    let envelope_delta: f32 = envelopes
        .iter()
        .filter(|route| route.amount.abs() > f32::EPSILON)
        .map(|route| route.level_at(ctx) * route.amount.clamp(-1.0, 1.0))
        .sum();
    lfo_delta + envelope_delta
}

fn modulated_control_value_from_delta(spec: &ControlSpec, base: f32, delta: f32) -> f32 {
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
    modulated_control_value_full(
        spec,
        std::slice::from_ref(route),
        &[],
        base,
        ModContext::lfo_only(beat),
    )
}

/// One modulated control's routes, resolved to plain copies so applying
/// them per sample needs no map lookups, string keys, or heap.
struct PlannedRoute {
    address: ControlAddress,
    spec: &'static ControlSpec,
    lfos: Vec<LfoRoute>,
    envelopes: Vec<EnvelopeRoute>,
    smoothed_delta: Option<f32>,
}

pub(crate) const AUTOMATION_DECLICK_MS: f64 = 3.0;

impl PlannedRoute {
    fn next_delta(&mut self, target: f32, sample_rate: f64) -> f32 {
        let Some(current) = self.smoothed_delta else {
            self.smoothed_delta = Some(target);
            return target;
        };
        let smoothing_samples = (AUTOMATION_DECLICK_MS * 0.001 * sample_rate).max(1.0);
        let coefficient = 1.0 - (-1.0 / smoothing_samples).exp();
        let next = current + (target - current) * coefficient as f32;
        self.smoothed_delta = Some(next);
        next
    }

    fn is_finished_fading(&self) -> bool {
        self.lfos.is_empty()
            && self.envelopes.is_empty()
            && self
                .smoothed_delta
                .is_none_or(|delta| delta.abs() <= f32::EPSILON)
    }
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
        let mut previous = std::mem::take(&mut self.routes);
        let addresses = automation.modulated_addresses();
        for address in addresses {
            let smoothed_delta = previous
                .iter()
                .position(|route| route.address == address)
                .and_then(|index| previous.swap_remove(index).smoothed_delta);
            self.routes.push(PlannedRoute {
                address,
                spec: address.spec(),
                lfos: automation.routes_for(address).copied().collect(),
                envelopes: automation.envelopes_for(address).copied().collect(),
                smoothed_delta,
            });
        }
        for mut removed in previous {
            removed.lfos.clear();
            removed.envelopes.clear();
            self.routes.push(removed);
        }
    }

    pub(crate) fn apply(&mut self, controls: &mut FluidControls, timing: TimingContext) {
        let ctx = ModContext {
            beat: timing.beat,
            kick_interval_beats: controls.kick.interval_beats,
            kick_offset_beats: controls.kick.offset_beats,
        };
        for planned in &mut self.routes {
            let target_delta = automation_delta(&planned.lfos, &planned.envelopes, ctx);
            let delta = planned.next_delta(target_delta, timing.sample_rate);
            let spec = planned.spec.contextual(controls);
            let base = (spec.get)(controls);
            let value = modulated_control_value_from_delta(&spec, base, delta);
            (spec.set)(controls, value);
        }
        self.routes.retain(|route| !route.is_finished_fading());
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
