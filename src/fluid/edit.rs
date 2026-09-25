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
    let recent_id = recent_id(automation, tab, selected);
    effects.edit_session(recent_id, |snapshot| match active {
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
                    let beats =
                        snap_step(ms_to_beats(current, bpm), FLIP_BEAT_STEP).max(FLIP_BEAT_STEP);
                    spec.apply_raw(beats_to_ms(beats, bpm), &mut snapshot.controls);
                }
                _ => {}
            }
        }
        _ => {}
    });
}

/// The flip key qualifier for a modulator time field, None for unit-less ones.
/// The control an edit counts as touching for the palette's MRU: the open
/// modulator's parent control when an editor is open, else the cursor row.
fn recent_id(automation: &AutomationState, tab: Tab, selected: usize) -> Option<&'static str> {
    automation
        .active_address()
        .map(ControlAddress::id)
        .or_else(|| tab_specs(tab).get(selected).map(|spec| spec.id))
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

/// Whether the LFO editor cursor (1-based; 0 is the parent slider row) sits on
/// a row of the Steps staircase, where `Shift+R` rolls only the step values.
pub(crate) fn lfo_steps_selected(
    automation: &AutomationState,
    address: ControlAddress,
    selected: usize,
) -> bool {
    selected
        .checked_sub(1)
        .and_then(|row| lfo_submenu_rows(automation, address).get(row).copied())
        .is_some_and(|row| matches!(row, LfoSubRow::Step(_)))
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
    effects.edit_session(Some(id), |snapshot| {
        let state = &mut snapshot.automation;
        let already = state.active_address() == Some(address) && state.active_kind() == Some(kind);
        if already {
            state.cycle_open(address, kind);
        } else {
            state.close_editor();
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

pub(crate) fn add_modulator_effect_for_id(
    effects: &mut EffectExecutor,
    id: &'static str,
    kind: ModKind,
) -> bool {
    let address = ControlAddress::new(id);
    let mut added = false;
    effects.edit_session(Some(id), |snapshot| {
        let state = &mut snapshot.automation;
        if state.active_address() != Some(address) || state.active_kind() != Some(kind) {
            state.close_editor();
        }
        added = state.add_and_open(address, kind);
    });
    added
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
    /// The mirror of `Reset`: jump to the top of the field's own range.
    Max,
    /// A typed value, exact in the field's displayed unit.
    Set {
        value: f32,
        flipped: &'a FlippedUnits,
    },
    /// A uniform roll (`ratio` in 0..1) across the field's own dial.
    Randomize {
        ratio: f32,
    },
}

impl<'a> FieldOp<'a> {
    /// The flipped-unit set for the ops that take user-entered values; None
    /// for a reset, which always lands on the native grid.
    fn flipped(self) -> Option<&'a FlippedUnits> {
        match self {
            FieldOp::Adjust { flipped, .. } | FieldOp::Set { flipped, .. } => Some(flipped),
            FieldOp::Reset | FieldOp::Max | FieldOp::Randomize { .. } => None,
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
    let recent_id = recent_id(automation, tab, selected);
    effects.edit_session(recent_id, |snapshot| {
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
                field
                    .time_key()
                    .is_some_and(|key| flipped.contains(&unit_key(address.id(), Some(key))))
            });
            let Some(route) = snapshot.automation.route_mut(address) else {
                return;
            };
            match op {
                FieldOp::Adjust { dir, .. } if is_flipped => {
                    let next = flipped_step(TimeBase::Beats, route.field_value(field), dir, bpm);
                    route.set_field_raw_at(field, next, beat);
                }
                FieldOp::Adjust { dir, .. } => route.adjust_field_at(field, dir, beat),
                FieldOp::Reset => route.reset_field_at(field, beat),
                FieldOp::Max => route.max_field_at(field, beat),
                // Typed ms is exact: convert and clamp, but don't snap back
                // onto the beat grid.
                FieldOp::Set { value, .. } if is_flipped => {
                    route.set_field_raw_at(field, flip_entry(TimeBase::Beats, value, bpm), beat);
                }
                FieldOp::Set { value, .. } => route.set_field_at(field, value, beat),
                // A random shape's value is its pattern: roll the seed, keep the shape.
                FieldOp::Randomize { .. }
                    if field == LfoField::Shape && route.shape.is_random() =>
                {
                    route.reseed();
                }
                FieldOp::Randomize { ratio } => route.randomize_field_at(field, ratio, beat),
            }
        }
        ActiveField::Envelope(address, field) => {
            let is_flipped = op.flipped().is_some_and(|flipped| {
                field
                    .time_key()
                    .is_some_and(|key| flipped.contains(&unit_key(address.id(), Some(key))))
            });
            let Some(route) = snapshot.automation.envelope_mut(address) else {
                return;
            };
            match op {
                FieldOp::Adjust { dir, .. } if is_flipped => {
                    let next = flipped_step(TimeBase::Beats, route.field_value(field), dir, bpm);
                    route.set_field_raw(field, next);
                }
                FieldOp::Adjust { dir, .. } => route.adjust_field(field, dir),
                FieldOp::Reset => route.reset_field(field),
                FieldOp::Max => route.max_field(field),
                FieldOp::Set { value, .. } if is_flipped => {
                    route.set_field_raw(field, flip_entry(TimeBase::Beats, value, bpm));
                }
                FieldOp::Set { value, .. } => route.set_field(field, value),
                FieldOp::Randomize { ratio } => route.randomize_field(field, ratio),
            }
        }
        ActiveField::LfoStep(address, target) => {
            if let Some(route) = snapshot.automation.route_mut(address) {
                match op {
                    FieldOp::Adjust { dir, .. } => route.adjust_step(target, dir),
                    FieldOp::Reset => route.reset_step(target),
                    FieldOp::Max => route.max_step(target),
                    FieldOp::Set { value, .. } => route.set_step(target, value),
                    FieldOp::Randomize { ratio } => route.randomize_step(target, ratio),
                }
            }
        }
        ActiveField::Control => {
            let filter_type =
                tab_specs(tab).get(selected).and_then(|spec| {
                    match module_slot_field(tab, spec, &snapshot.controls, Family::Filter)? {
                        (slot, ModuleSlotField::Feedback) => Some(slot),
                        _ => None,
                    }
                });
            let before =
                filter_type.and_then(|slot| filter_slot(snapshot, tab, slot).map(|m| m.feedback));
            match op {
                FieldOp::Reset => apply_reset(tab, selected, &mut snapshot.controls),
                FieldOp::Max => apply_max(tab, selected, &mut snapshot.controls),
                FieldOp::Randomize { ratio } => {
                    if let Some(spec) = tab_specs(tab).get(selected) {
                        spec.apply_ratio(ratio, &mut snapshot.controls);
                    }
                }
                FieldOp::Adjust { .. } | FieldOp::Set { .. } => {
                    apply_control_value_op(snapshot, tab, selected, op, bpm)
                }
            }
            // The Type row stepped like any discrete row; replay the change
            // through `switch_filter_type` so the cutoff follows a flip. A
            // roll lands anywhere on the dial, so it keeps its random cutoff.
            if let (Some(slot), Some(before)) = (filter_type, before)
                && !matches!(op, FieldOp::Randomize { .. })
                && let Some(module) = filter_slot(snapshot, tab, slot)
            {
                let next = module.feedback;
                module.feedback = before;
                switch_filter_type(module, next);
            }
        }
    }
}

fn filter_slot(
    snapshot: &mut LiveSessionSnapshot,
    tab: Tab,
    slot: usize,
) -> Option<&mut ModuleSlot> {
    snapshot.controls.modules.for_tab_mut(tab)?.get_mut(slot)
}

/// The selected control taking a user-entered value: a Delay clock row
/// first, which flips rather than steps, then a field displayed in a flipped
/// unit, then the ordinary registry path.
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
    let flipped = op
        .flipped()
        .expect("only Reset and Randomize carry no flipped set");
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
        (_, FieldOp::Reset | FieldOp::Max | FieldOp::Randomize { .. }) => {
            unreachable!("a reset, max, or roll never reaches the value path")
        }
    }
}

/// Which field of a loaded `family` slot the row is, or `None` for any other
/// row. The one decision behind every module-specific gesture: a Delay's
/// clock flip and time-unit toggle, a Filter's type switch.
fn module_slot_field(
    tab: Tab,
    spec: &ControlSpec,
    controls: &FluidControls,
    family: Family,
) -> Option<(usize, ModuleSlotField)> {
    let (_, slot, field) = parse_module_slot_id(spec.id)?;
    let module = controls.modules.for_tab(tab)?.get(slot)?;
    module
        .kind()
        .is_some_and(|kind| kind.family == family)
        .then_some((slot, field))
}

/// A Delay slot's clock row flips Sync/Free on an arrow press instead of
/// stepping a value; the time rows are registry-stepped, since `contextual`
/// gives them the loaded clock's range and grid. Returns true when the edit
/// was a clock flip and has been applied.
fn apply_delay_row(
    snapshot: &mut LiveSessionSnapshot,
    tab: Tab,
    spec: &ControlSpec,
    op: FieldOp<'_>,
    bpm: f32,
) -> bool {
    let Some((slot, ModuleSlotField::Clock)) =
        module_slot_field(tab, spec, &snapshot.controls, Family::Delay)
    else {
        return false;
    };
    // A typed value goes through the ordinary discrete-control path instead.
    if !matches!(op, FieldOp::Adjust { .. }) {
        return false;
    }
    if let Some(slots) = snapshot.controls.modules.for_tab_mut(tab)
        && let Some(module) = slots.get_mut(slot)
    {
        switch_delay_clock(module, false, bpm);
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

/// The ceiling half of the extremes gesture: the mirror of
/// `reset_lfo_or_control` above, landing on each field's own range top
/// instead of its reset target.
pub(crate) fn max_lfo_or_control(
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
        FieldOp::Max,
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
        && let Some((slot, field @ (ModuleSlotField::Time | ModuleSlotField::RightTime))) =
            module_slot_field(tab, spec, &effects.session().load().controls, Family::Delay)
    {
        effects.edit_session(Some(spec.id), |snapshot| {
            let bpm = snapshot.controls.master.bpm;
            if let Some(slots) = snapshot.controls.modules.for_tab_mut(tab)
                && let Some(module) = slots.get_mut(slot)
            {
                switch_delay_clock(module, field == ModuleSlotField::RightTime, bpm);
            }
        });
        return;
    }
    let key = match active_field(automation, lfo_selected) {
        ActiveField::Lfo(address, field) => field
            .time_key()
            .map(|key| unit_key(address.id(), Some(key))),
        ActiveField::Envelope(address, field) => field
            .time_key()
            .map(|key| unit_key(address.id(), Some(key))),
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
            effects.edit_session(Some(id), |snapshot| {
                snapshot.automation.remove_open_route();
            });
        }
        _ => {
            if let Some(id) = selected_control {
                let address = ControlAddress::new(id);
                effects.edit_session(Some(id), |snapshot| {
                    snapshot.automation.clear_control(address);
                });
            }
        }
    }
}

pub(crate) fn randomize_lfo_or_control(
    effects: &mut EffectExecutor,
    automation: &AutomationState,
    lfo_selected: usize,
    tab: Tab,
    selected: usize,
    ratio: f32,
    beat: f64,
) {
    with_active_field(
        effects,
        automation,
        lfo_selected,
        tab,
        selected,
        beat,
        FieldOp::Randomize { ratio },
    );
}
