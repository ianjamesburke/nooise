//! Every control and automation mutation a UI gesture performs.
//!
//! `active_field` resolves the cursor to exactly one target and
//! `with_active_field` applies one `FieldOp` to it, so adjust, reset, and set
//! cannot drift apart. `effect.rs` calls into this module; never the reverse.

use super::*;

/// Numeric entry typed in a flipped field's display unit, converted back to
/// the native base before snapping.
fn flip_entry(base: TimeBase, value: f32, bpm: f32) -> f32 {
    match base {
        TimeBase::Beats => ms_to_beats(value, bpm),
        TimeBase::Ms => beats_to_ms(value, bpm),
        TimeBase::None => value,
    }
}

/// Step grids for a flipped time field: native-beats fields move on a 10 ms
/// grid, native-ms fields on the 0.125-beat grid.
const FLIP_MS_STEP: f32 = 10.0;
const FLIP_BEAT_STEP: f32 = 0.125;

/// One h/l step for a flipped time field, taken in its display unit and
/// returned in the native unit (unclamped; the setter clamps).
fn flipped_step(native: TimeBase, value: f32, dir: f32, bpm: f32) -> f32 {
    match native {
        TimeBase::Beats => ms_to_beats(
            snap_step(beats_to_ms(value, bpm) + dir * FLIP_MS_STEP, FLIP_MS_STEP),
            bpm,
        ),
        TimeBase::Ms => beats_to_ms(
            snap_step(
                ms_to_beats(value, bpm) + dir * FLIP_BEAT_STEP,
                FLIP_BEAT_STEP,
            ),
            bpm,
        ),
        TimeBase::None => value,
    }
}

/// Landing rule for the unit toggle: flipping a field so it displays beats
/// rounds its value onto the beat grid; flipping to ms keeps the exact
/// equivalent so the value can then move freely in time.
pub(crate) fn snap_after_unit_flip(
    effects: &mut EffectExecutor,
    automation: &AutomationState,
    lfo_selected: usize,
    tab: Tab,
    selected: usize,
    now_flipped: bool,
    beat: f64,
) {
    let active = active_field(automation, lfo_selected);
    let recent_id = automation
        .active_address()
        .map(ControlAddress::id)
        .or_else(|| tab_specs(tab).get(selected).map(|spec| spec.id));
    effects.edit_session(
        AutoOwnership::TakeOver,
        recent_id,
        |snapshot| match active {
            // LFO rate accepts exact typed beat values, so an exact ms-authored
            // value stays exact when returning to beats. Offset retains its grid.
            ActiveField::Lfo(address, field) if !now_flipped => {
                // Interval is deliberately untouched: an exact ms-authored
                // rate must stay exact when the unit flips back to beats.
                if let Some(route) = snapshot.automation.route_mut(address)
                    && field == LfoField::Offset
                {
                    route.set_field_at(field, route.phase_offset_beats, beat);
                }
            }
            ActiveField::Envelope(address, field) if !now_flipped => {
                if let Some(route) = snapshot.automation.envelope_mut(address) {
                    match field {
                        EnvField::Attack => route.set_field(field, route.attack_beats),
                        EnvField::Decay => route.set_field(field, route.decay_beats),
                        EnvField::Amount | EnvField::Trigger => {}
                    }
                }
            }
            ActiveField::Control => {
                let Some(spec) = tab_specs(tab).get(selected) else {
                    return;
                };
                let bpm = snapshot.controls.master.bpm;
                let current = (spec.get)(&snapshot.controls);
                match (spec.time_base, now_flipped) {
                    // Back to native beats: land on the control's own grid.
                    (TimeBase::Beats, false) => {
                        spec.apply_quantized_value(current, &mut snapshot.controls)
                    }
                    // An ms control now displayed in beats: round to the nearest
                    // divided beat.
                    (TimeBase::Ms, true) => {
                        let beats = snap_step(ms_to_beats(current, bpm), FLIP_BEAT_STEP)
                            .max(FLIP_BEAT_STEP);
                        spec.apply_raw(beats_to_ms(beats, bpm), &mut snapshot.controls);
                    }
                    _ => {}
                }
            }
            _ => {}
        },
    );
}

/// The flip key qualifier for a modulator time field, None for unit-less ones.
fn lfo_time_key(field: LfoField) -> Option<&'static str> {
    match field {
        LfoField::Interval => Some("lfo.interval"),
        LfoField::Offset => Some("lfo.offset"),
        _ => None,
    }
}

fn env_time_key(field: EnvField) -> Option<&'static str> {
    match field {
        EnvField::Attack => Some("env.attack"),
        EnvField::Decay => Some("env.decay"),
        EnvField::Amount | EnvField::Trigger => None,
    }
}

/// One selectable row inside an open LFO editor.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum LfoSubRow {
    Field(LfoField),
    /// A row of the Steps shape's inline step editor: sequence length, edge
    /// glide, or one step value. Present only while the shape is `Steps`,
    /// listed right after the Shape field (which is last in `LfoField::ALL`).
    Step(StepTarget),
}

pub(crate) fn lfo_submenu_rows(
    automation: &AutomationState,
    address: ControlAddress,
) -> Vec<LfoSubRow> {
    let mut rows = Vec::with_capacity(LfoField::ALL.len());
    for field in LfoField::ALL {
        rows.push(LfoSubRow::Field(field));
    }
    if let Some(route) = automation.route(address)
        && route.shape == LfoShape::Steps
    {
        rows.push(LfoSubRow::Step(StepTarget::Count));
        rows.push(LfoSubRow::Step(StepTarget::Glide));
        for i in 0..route.active_step_count() {
            rows.push(LfoSubRow::Step(StepTarget::Value(i)));
        }
    }
    rows
}

/// Close the open modulator editor.
pub(crate) fn close_one_level_effect(
    effects: &mut EffectExecutor,
    automation: &AutomationState,
) -> Option<usize> {
    automation.active_address()?;
    effects.edit_navigation_automation(AutomationState::close_editor);
    None
}

pub(crate) fn env_field_at(index: usize) -> Option<EnvField> {
    EnvField::ALL.get(index.checked_sub(1)?).copied()
}

/// LFO editors are explicitly collapsed with `f` or Escape. Arrow navigation
/// stays inside the submenu and clamps at its first and last selectable rows.
#[cfg(test)]
pub(crate) fn clamp_lfo_selection(current: usize, direction: isize, row_count: usize) -> usize {
    if row_count == 0 {
        return 0;
    }
    current.saturating_add_signed(direction).clamp(1, row_count)
}

pub(crate) fn open_modulator_effect_for_id(
    effects: &mut EffectExecutor,
    id: &'static str,
    kind: ModKind,
    sub_selected: &mut usize,
) {
    let address = ControlAddress::new(id);
    effects.edit_session(AutoOwnership::TakeOver, Some(id), |snapshot| {
        let state = &mut snapshot.automation;
        let already = state.active_address() == Some(address) && state.active_kind() == Some(kind);
        state.close_editor();
        if !already {
            match kind {
                ModKind::Lfo => {
                    state.open_or_create(address);
                }
                ModKind::Envelope => {
                    state.open_or_create_envelope(address);
                }
            }
        }
    });
    *sub_selected = 1;
}

/// Which modulator field (if any) the submenu cursor sits on for the open
/// editor. Returns None when the parent slider row (index 0) is selected or no
/// editor is open, so the caller edits the underlying control instead.
#[derive(Clone, Copy)]
enum ActiveField {
    Lfo(ControlAddress, LfoField),
    /// A step-editor row of a Steps-shaped LFO (count, glide, or one value).
    LfoStep(ControlAddress, StepTarget),
    Envelope(ControlAddress, EnvField),
    Control,
}

fn active_field(automation: &AutomationState, lfo_selected: usize) -> ActiveField {
    let Some(address) = automation.active_address() else {
        return ActiveField::Control;
    };
    if lfo_selected == 0 {
        return ActiveField::Control;
    }
    match automation.active_kind() {
        Some(ModKind::Lfo) => match lfo_submenu_rows(automation, address).get(lfo_selected - 1) {
            Some(LfoSubRow::Field(field)) => ActiveField::Lfo(address, *field),
            Some(LfoSubRow::Step(target)) => ActiveField::LfoStep(address, *target),
            None => ActiveField::Control,
        },
        Some(ModKind::Envelope) => match env_field_at(lfo_selected) {
            Some(field) => ActiveField::Envelope(address, field),
            None => ActiveField::Control,
        },
        None => ActiveField::Control,
    }
}

pub(crate) fn automation_kind_is_supported(
    _selected_control: Option<&str>,
    kind: interaction::AutomationKind,
) -> bool {
    matches!(
        kind,
        interaction::AutomationKind::Lfo | interaction::AutomationKind::Envelope
    )
}

/// The one verb an active-field edit applies. Every field kind — LFO,
/// envelope, step, plain control — accepts all three, so the routing
/// from cursor position to target lives in one place and only the verb
/// differs between an arrow press, a reset, and a typed value.
#[derive(Clone, Copy)]
enum FieldOp<'a> {
    /// One h/l step in `dir`, in the field's displayed unit.
    Adjust {
        dir: f32,
        flipped: &'a FlippedUnits,
    },
    Reset,
    /// A typed value, exact in the field's displayed unit.
    Set {
        value: f32,
        flipped: &'a FlippedUnits,
    },
}

impl<'a> FieldOp<'a> {
    /// The flipped-unit set for the ops that take user-entered values; None
    /// for a reset, which always lands on the native grid.
    fn flipped(self) -> Option<&'a FlippedUnits> {
        match self {
            FieldOp::Adjust { flipped, .. } | FieldOp::Set { flipped, .. } => Some(flipped),
            FieldOp::Reset => None,
        }
    }
}

/// Apply `op` to whatever the cursor currently addresses: a modulator field
/// inside an open editor, or the selected control itself. Resolves the target
/// once, records the touched control as recent, and publishes one aggregate
/// session edit.
fn with_active_field(
    effects: &mut EffectExecutor,
    automation: &AutomationState,
    lfo_selected: usize,
    tab: Tab,
    selected: usize,
    beat: f64,
    op: FieldOp<'_>,
) {
    let active = active_field(automation, lfo_selected);
    let recent_id = automation
        .active_address()
        .map(ControlAddress::id)
        .or_else(|| tab_specs(tab).get(selected).map(|spec| spec.id));
    effects.edit_session(AutoOwnership::TakeOver, recent_id, |snapshot| {
        apply_field_op(snapshot, active, tab, selected, beat, op);
    });
}

fn apply_field_op(
    snapshot: &mut LiveSessionSnapshot,
    active: ActiveField,
    tab: Tab,
    selected: usize,
    beat: f64,
    op: FieldOp<'_>,
) {
    let bpm = snapshot.controls.master.bpm;
    match active {
        ActiveField::Lfo(address, field) => {
            // Only interval and offset carry a time base, so a flipped LFO
            // field is always one of those two.
            let is_flipped = op.flipped().is_some_and(|flipped| {
                lfo_time_key(field)
                    .is_some_and(|key| flipped.contains(&unit_key(address.id(), Some(key))))
            });
            let Some(route) = snapshot.automation.route_mut(address) else {
                return;
            };
            match op {
                FieldOp::Adjust { dir, .. } if is_flipped => {
                    let current = match field {
                        LfoField::Offset => route.phase_offset_beats,
                        _ => route.cycle_beats,
                    };
                    let next = flipped_step(TimeBase::Beats, current, dir, bpm);
                    route.set_field_raw_at(field, next, beat);
                }
                FieldOp::Adjust { dir, .. } => route.adjust_field_at(field, dir, beat),
                FieldOp::Reset => route.reset_field_at(field, beat),
                // Typed ms is exact: convert and clamp, but don't snap back
                // onto the beat grid.
                FieldOp::Set { value, .. } if is_flipped => {
                    route.set_field_raw_at(field, flip_entry(TimeBase::Beats, value, bpm), beat);
                }
                FieldOp::Set { value, .. } => route.set_field_at(field, value, beat),
            }
        }
        ActiveField::Envelope(address, field) => {
            let is_flipped = op.flipped().is_some_and(|flipped| {
                env_time_key(field)
                    .is_some_and(|key| flipped.contains(&unit_key(address.id(), Some(key))))
            });
            let Some(route) = snapshot.automation.envelope_mut(address) else {
                return;
            };
            match op {
                FieldOp::Adjust { dir, .. } if is_flipped => {
                    let current = match field {
                        EnvField::Decay => route.decay_beats,
                        _ => route.attack_beats,
                    };
                    let next = flipped_step(TimeBase::Beats, current, dir, bpm);
                    route.set_field_raw(field, next);
                }
                FieldOp::Adjust { dir, .. } => route.adjust_field(field, dir),
                FieldOp::Reset => route.reset_field(field),
                FieldOp::Set { value, .. } if is_flipped => {
                    route.set_field_raw(field, flip_entry(TimeBase::Beats, value, bpm));
                }
                FieldOp::Set { value, .. } => route.set_field(field, value),
            }
        }
        ActiveField::LfoStep(address, target) => {
            if let Some(route) = snapshot.automation.route_mut(address) {
                match op {
                    FieldOp::Adjust { dir, .. } => route.adjust_step(target, dir),
                    FieldOp::Reset => route.reset_step(target),
                    FieldOp::Set { value, .. } => route.set_step(target, value),
                }
            }
        }
        ActiveField::Control => match op {
            FieldOp::Reset => apply_reset(tab, selected, &mut snapshot.controls),
            FieldOp::Adjust { .. } | FieldOp::Set { .. } => {
                apply_control_value_op(snapshot, tab, selected, op, bpm)
            }
        },
    }
}

/// The selected control taking a user-entered value: a Delay time row first,
/// which owns its own clock-derived range, then a field displayed in a
/// flipped unit, then the ordinary registry path.
fn apply_control_value_op(
    snapshot: &mut LiveSessionSnapshot,
    tab: Tab,
    selected: usize,
    op: FieldOp<'_>,
    bpm: f32,
) {
    let spec = tab_specs(tab).get(selected);
    if let Some(spec) = spec
        && apply_delay_row(snapshot, tab, spec, op, bpm)
    {
        return;
    }
    let flipped = op.flipped().expect("only Reset carries no flipped set");
    let flipped_spec = spec.filter(|spec| {
        spec.time_base != TimeBase::None && flipped.contains(&unit_key(spec.id, None))
    });
    match (flipped_spec, op) {
        (Some(spec), FieldOp::Adjust { dir, .. }) => {
            let current = (spec.get)(&snapshot.controls);
            spec.apply_raw(
                flipped_step(spec.time_base, current, dir, bpm),
                &mut snapshot.controls,
            );
        }
        // Typed input in the flipped unit is exact: convert and clamp, but
        // don't snap onto the native step grid.
        (Some(spec), FieldOp::Set { value, .. }) => {
            spec.apply_raw(
                flip_entry(spec.time_base, value, bpm),
                &mut snapshot.controls,
            );
        }
        (None, FieldOp::Adjust { dir, .. }) => {
            apply_delta(tab, selected, dir, &mut snapshot.controls)
        }
        (None, FieldOp::Set { value, .. }) => {
            apply_value(tab, selected, value, &mut snapshot.controls)
        }
        (_, FieldOp::Reset) => unreachable!("a reset never reaches the value path"),
    }
}

/// A Delay slot's time rows are not registry-stepped: each side carries its
/// own Sync/Free clock, so the row's range and step come from that clock
/// rather than the spec. Returns true when the row belongs to a Delay slot
/// and the edit has been applied there.
fn apply_delay_row(
    snapshot: &mut LiveSessionSnapshot,
    tab: Tab,
    spec: &ControlSpec,
    op: FieldOp<'_>,
    bpm: f32,
) -> bool {
    let Some((slot, module)) = module_slot_at_id(tab, spec.id, &snapshot.controls) else {
        return false;
    };
    if !module
        .kind()
        .is_some_and(|kind| kind.family == Family::Delay)
    {
        return false;
    }
    let field = spec.id.rsplit('.').next();
    // The clock row itself flips Sync/Free on an arrow press; a typed value
    // goes through the ordinary discrete-control path instead.
    if field == Some("clock") {
        if !matches!(op, FieldOp::Adjust { .. }) {
            return false;
        }
        if let Some(slots) = snapshot.controls.modules.for_tab_mut(tab)
            && let Some(module) = slots.get_mut(slot)
        {
            switch_delay_clock(module, false, bpm);
        }
        return true;
    }
    let right = match field {
        Some("time") => false,
        Some("right_time") => true,
        _ => return false,
    };
    if let Some(slots) = snapshot.controls.modules.for_tab_mut(tab)
        && let Some(module) = slots.get_mut(slot)
    {
        let clock = DelayClock::from_value(if right {
            module.right_clock
        } else {
            module.clock
        });
        let current = if right {
            &mut module.right_time
        } else {
            &mut module.time
        };
        *current = match (clock, op) {
            (DelayClock::Sync, FieldOp::Adjust { dir, .. }) => {
                beat_grid_adjust(*current, dir, DELAY_SYNC_MIN_BEATS, DELAY_SYNC_MAX_BEATS)
            }
            (DelayClock::Free, FieldOp::Adjust { dir, .. }) => {
                (*current + dir * 10.0).clamp(DELAY_FREE_MIN_MS, DELAY_FREE_MAX_MS)
            }
            (DelayClock::Sync, FieldOp::Set { value, .. }) => {
                value.clamp(DELAY_SYNC_MIN_BEATS, DELAY_SYNC_MAX_BEATS)
            }
            (DelayClock::Free, FieldOp::Set { value, .. }) => {
                value.clamp(DELAY_FREE_MIN_MS, DELAY_FREE_MAX_MS)
            }
            (_, FieldOp::Reset) => unreachable!("a reset never reaches the delay rows"),
        };
    }
    true
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn adjust_lfo_or_control(
    effects: &mut EffectExecutor,
    automation: &AutomationState,
    lfo_selected: usize,
    tab: Tab,
    selected: usize,
    dir: f32,
    beat: f64,
    flipped: &FlippedUnits,
) {
    with_active_field(
        effects,
        automation,
        lfo_selected,
        tab,
        selected,
        beat,
        FieldOp::Adjust { dir, flipped },
    );
}

pub(crate) fn reset_lfo_or_control(
    effects: &mut EffectExecutor,
    automation: &AutomationState,
    lfo_selected: usize,
    tab: Tab,
    selected: usize,
    beat: f64,
) {
    with_active_field(
        effects,
        automation,
        lfo_selected,
        tab,
        selected,
        beat,
        FieldOp::Reset,
    );
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn set_modulator_or_control(
    effects: &mut EffectExecutor,
    automation: &AutomationState,
    lfo_selected: usize,
    tab: Tab,
    selected: usize,
    value: f32,
    beat: f64,
    flipped: &FlippedUnits,
) {
    with_active_field(
        effects,
        automation,
        lfo_selected,
        tab,
        selected,
        beat,
        FieldOp::Set { value, flipped },
    );
}

pub(crate) fn toggle_units_effect(
    effects: &mut EffectExecutor,
    automation: &AutomationState,
    flipped: &mut FlippedUnits,
    lfo_selected: usize,
    tab: Tab,
    selected: usize,
    beat: f64,
) {
    if matches!(active_field(automation, lfo_selected), ActiveField::Control)
        && let Some(spec) = tab_specs(tab).get(selected)
        && matches!(spec.id.rsplit('.').next(), Some("time" | "right_time"))
        && let snapshot = effects.session().load()
        && let Some((slot, module)) = module_slot_at_id(tab, spec.id, &snapshot.controls)
        && module
            .kind()
            .is_some_and(|kind| kind.family == Family::Delay)
    {
        effects.edit_session(AutoOwnership::TakeOver, Some(spec.id), |snapshot| {
            let bpm = snapshot.controls.master.bpm;
            if let Some(slots) = snapshot.controls.modules.for_tab_mut(tab)
                && let Some(module) = slots.get_mut(slot)
            {
                switch_delay_clock(module, spec.id.ends_with(".right_time"), bpm);
            }
        });
        return;
    }
    let key = match active_field(automation, lfo_selected) {
        ActiveField::Lfo(address, field) => {
            lfo_time_key(field).map(|key| unit_key(address.id(), Some(key)))
        }
        ActiveField::Envelope(address, field) => {
            env_time_key(field).map(|key| unit_key(address.id(), Some(key)))
        }
        ActiveField::LfoStep(..) => None,
        ActiveField::Control => tab_specs(tab)
            .get(selected)
            .filter(|spec| spec.time_base != TimeBase::None)
            .map(|spec| unit_key(spec.id, None)),
    };
    let Some(key) = key else { return };
    let now_flipped = !flipped.remove(&key);
    if now_flipped {
        flipped.insert(key);
    }
    snap_after_unit_flip(
        effects,
        automation,
        lfo_selected,
        tab,
        selected,
        now_flipped,
        beat,
    );
}

pub(crate) fn remove_automation_effect(
    effects: &mut EffectExecutor,
    automation: &AutomationState,
    selected_control: Option<&'static str>,
    lfo_selected: usize,
) {
    match active_field(automation, lfo_selected) {
        _ if automation.is_editor_open() => {
            let id = automation
                .active_address()
                .expect("open editor has an address")
                .id();
            effects.edit_session(AutoOwnership::TakeOver, Some(id), |snapshot| {
                snapshot.automation.remove_open_route();
            });
        }
        _ => {
            if let Some(id) = selected_control {
                let address = ControlAddress::new(id);
                effects.edit_session(AutoOwnership::TakeOver, Some(id), |snapshot| {
                    snapshot.automation.clear_control(address);
                });
            }
        }
    }
}

pub(crate) fn reseed_automation_effect(effects: &mut EffectExecutor, automation: &AutomationState) {
    if let Some(address) = automation.active_address()
        && automation.active_kind() == Some(ModKind::Lfo)
    {
        effects.edit_session(AutoOwnership::TakeOver, Some(address.id()), |snapshot| {
            let state = &mut snapshot.automation;
            if let Some(route) = state.route_mut(address)
                && route.shape.is_random()
            {
                route.reseed();
            }
        });
    }
}
