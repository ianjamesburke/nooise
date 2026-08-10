//! Modulation routes keyed by stable control ID. Each route family owns its
//! own submodule; this root owns the address vocabulary they share, the
//! `AutomationState` that stores all three, and the summing that turns them
//! into the value the engine plays.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use super::widget::DialScale;
use super::{
    ControlSpec, Entry, FluidControls, LfoSnap, MACRO_CONTROLS, MACRO_COUNT, TAPER_STEPS_PER_SWEEP,
    TimingContext, beat_grid_adjust, beat_grid_snap, is_macro_id, nearest_power_of_two, snap_step,
    spec_by_id,
};

mod envelope;
mod lfo;
mod macro_route;

// The submodules split one flat module along its route families; their
// `pub(crate)` surface is this module's surface, unchanged by the split.
pub(crate) use envelope::*;
pub(crate) use lfo::*;
pub(crate) use macro_route::*;

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

/// One family of modulation route stored on `AutomationState`. The three
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

impl Route for MacroRoute {
    const KIND: ModKind = ModKind::Macro;

    fn map(state: &AutomationState) -> &BTreeMap<ControlAddress, Self> {
        &state.macros
    }

    fn map_mut(state: &mut AutomationState) -> &mut BTreeMap<ControlAddress, Self> {
        &mut state.macros
    }

    fn fresh(_address: ControlAddress) -> Self {
        Self::default()
    }

    fn is_neutral(&self) -> bool {
        // The inherent method of the same name; field macros reach for it
        // through a plain `MacroRoute`, outside this trait.
        MacroRoute::is_neutral(*self)
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

    pub(crate) fn open_or_create_macro(&mut self, address: ControlAddress) -> &mut MacroRoute {
        self.open_or_create_route(address)
    }

    /// Remove the route backing the open editor and close it. The x gesture:
    /// explicit, worked on the first try, unlike double-tap.
    pub(crate) fn remove_open_route(&mut self) {
        let Some(open) = self.open.take() else {
            return;
        };
        match open.kind {
            ModKind::Lfo => {
                self.remove_route_of::<LfoRoute>(open.address);
                self.remove_field_macros_for(open.address, "lfo.");
            }
            ModKind::Envelope => self.remove_route_of::<EnvelopeRoute>(open.address),
            ModKind::Macro => self.remove_route_of::<MacroRoute>(open.address),
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
            // depth_ratio alone isn't the whole story for an LFO: a field
            // macro stacked on lfo.amount (or interval/offset) can still be
            // driving the route externally even while its own base amount
            // sits at neutral, so the route stays live and must not be pruned
            // out from under it.
            ModKind::Lfo if self.has_live_field_macro(open.address) => {}
            ModKind::Lfo => self.prune_neutral_route::<LfoRoute>(open.address),
            ModKind::Envelope => self.prune_neutral_route::<EnvelopeRoute>(open.address),
            ModKind::Macro => self.prune_neutral_route::<MacroRoute>(open.address),
        }
    }

    /// Whether any macro stacked onto this control's LFO fields is still
    /// contributing, which keeps the parent LFO route alive on close.
    fn has_live_field_macro(&self, address: ControlAddress) -> bool {
        let prefix = format!("{}#lfo.", address.id());
        self.field_macros
            .iter()
            .any(|(key, route)| key.starts_with(&prefix) && !route.is_neutral())
    }

    /// The field-macro key currently expanded for editing, if any.
    pub(crate) fn open_field(&self) -> Option<&str> {
        self.open_field.as_deref()
    }

    /// Resolve the exact eligible LFO field whose nested macro editor is open
    /// on `address`. Foreign controls, discrete fields, macro-slider targets,
    /// and keys without a backing route are not valid nested editors.
    pub(crate) fn field_macro_owner(&self, address: ControlAddress) -> Option<LfoField> {
        if is_macro_id(address.id()) {
            return None;
        }
        let open_key = self.open_field()?;
        self.field_macro(open_key)?;
        LfoField::ALL.into_iter().find(|field| {
            field
                .macro_key()
                .is_some_and(|key| unit_key(address.id(), Some(key)) == open_key)
        })
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

    pub(crate) fn macro_route(&self, address: ControlAddress) -> Option<&MacroRoute> {
        self.route_of(address)
    }

    pub(crate) fn macro_route_mut(&mut self, address: ControlAddress) -> Option<&mut MacroRoute> {
        self.route_of_mut(address)
    }

    pub(crate) fn set_macro_route(&mut self, address: ControlAddress, route: MacroRoute) {
        self.set_route_of(address, route);
    }

    pub(crate) fn macro_routes(&self) -> impl Iterator<Item = (ControlAddress, &MacroRoute)> {
        self.routes_of()
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
    if let Some(combined) = macro_mod {
        delta += combined;
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
            let spec = planned.spec.contextual(controls);
            let base = (spec.get)(controls);
            let value = modulated_control_value_full(&spec, lfo, envelope, macro_mod, base, ctx);
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
