use std::collections::BTreeSet;

use super::widget::{Dial, DialScale};
use super::*;

/// Submenu row 0 is the parent slider; rows 1.. map onto the modulator fields.
/// Fields whose display and numeric entry have been flipped to the opposite
/// time base (beats <-> ms) by pressing T on that row. Keyed per field, so
/// each slider carries its own unit; stepping always stays on the native
/// grid and conversion happens at the current BPM.
pub(crate) type FlippedUnits = BTreeSet<String>;

fn fmt_ms(ms: f32) -> String {
    secs(ms / 1000.0)
}

fn fmt_beats(beats: f32) -> String {
    format!("{beats:.3} beats")
}

/// Cross-base display for a flipped time field; None when the field has no
/// time base to flip.
fn flip_display(base: TimeBase, value: f32, bpm: f32) -> Option<String> {
    match base {
        TimeBase::Beats => Some(fmt_ms(beats_to_ms(value, bpm))),
        TimeBase::Ms => Some(fmt_beats(ms_to_beats(value, bpm))),
        TimeBase::None => None,
    }
}

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
                if let Some(route) = snapshot.automation.route_mut(address) {
                    match field {
                        LfoField::Interval => {}
                        LfoField::Offset => {
                            route.set_field_at(field, route.phase_offset_beats, beat)
                        }
                        _ => {}
                    }
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

/// The flip key for whatever time field the cursor sits on, or None when the
/// selection has no time base (T is then a no-op).
/// One selectable row inside an open LFO editor: either one of the LFO's own
/// fields, or one of the two rows (amount, target) of a macro currently
/// stacked onto that field. The macro rows only exist while that field's `v`
/// gesture has expanded them — never by default.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum LfoSubRow {
    Field(LfoField),
    FieldMacro(LfoField, MacroField),
    /// A row of the Steps shape's inline step editor: sequence length, edge
    /// glide, or one step value. Present only while the shape is `Steps`,
    /// listed right after the Shape field (which is last in `LfoField::ALL`).
    Step(StepTarget),
}

pub(crate) fn lfo_submenu_rows(
    automation: &AutomationState,
    address: ControlAddress,
) -> Vec<LfoSubRow> {
    let mut rows = Vec::with_capacity(LfoField::ALL.len() * (1 + MacroField::ALL.len()));
    for field in LfoField::ALL {
        rows.push(LfoSubRow::Field(field));
        if is_macro_id(address.id()) {
            continue;
        }
        if let Some(key_str) = field.macro_key() {
            let key = unit_key(address.id(), Some(key_str));
            if automation.open_field() == Some(key.as_str()) {
                for slot in MacroField::ALL {
                    rows.push(LfoSubRow::FieldMacro(field, slot));
                }
            }
        }
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

/// The submenu row index (1-based, matching `lfo_selected`) of an LFO
/// field's own row, or 0 if it isn't present. Used to land the cursor back
/// on a field's row after its nested rows appear or disappear, since the
/// field's own position never shifts (nested rows only ever insert or
/// remove immediately after it).
fn field_row_index(
    automation: &AutomationState,
    address: ControlAddress,
    field: LfoField,
) -> usize {
    lfo_submenu_rows(automation, address)
        .iter()
        .position(|row| *row == LfoSubRow::Field(field))
        .map_or(0, |pos| pos + 1)
}

/// Close exactly one level of nesting on the open editor: a field-macro's
/// own editor if one is expanded, else the whole modulator editor. This is
/// the single place that governs "close the innermost open thing" — Esc and
/// re-pressing `v` on a nested field-macro row both route through it, so
/// drilling out one step never destroys more than what's actually open.
pub(crate) fn close_one_level_effect(
    effects: &mut EffectExecutor,
    automation: &AutomationState,
) -> Option<usize> {
    let address = automation.active_address()?;
    if let Some(field) = automation.field_macro_owner(address) {
        effects.edit_navigation_automation(AutomationState::close_open_field);
        let current = effects.session().load();
        Some(field_row_index(&current.automation, address, field))
    } else {
        effects.edit_navigation_automation(AutomationState::close_editor);
        None
    }
}

pub(crate) fn env_field_at(index: usize) -> Option<EnvField> {
    EnvField::ALL.get(index.checked_sub(1)?).copied()
}

pub(crate) fn macro_field_at(index: usize) -> Option<MacroField> {
    MacroField::ALL.get(index.checked_sub(1)?).copied()
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
    if id.starts_with("macro.") && kind != ModKind::Lfo {
        return;
    }
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
                ModKind::Macro => {
                    state.open_or_create_macro(address);
                }
            }
        }
    });
    *sub_selected = 1;
}

/// Which modulator field (if any) the submenu cursor sits on for the open
/// editor. Returns None when the parent slider row (index 0) is selected or no
/// editor is open, so the caller edits the underlying control instead.
enum ActiveField {
    Lfo(ControlAddress, LfoField),
    /// A macro's amount/target row nested under an LFO field, only present
    /// while that field's stacked macro is expanded for editing.
    LfoMacro(ControlAddress, LfoField, MacroField),
    /// A step-editor row of a Steps-shaped LFO (count, glide, or one value).
    LfoStep(ControlAddress, StepTarget),
    Envelope(ControlAddress, EnvField),
    Macro(ControlAddress, MacroField),
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
            Some(LfoSubRow::FieldMacro(field, mf)) => ActiveField::LfoMacro(address, *field, *mf),
            Some(LfoSubRow::Step(target)) => ActiveField::LfoStep(address, *target),
            None => ActiveField::Control,
        },
        Some(ModKind::Envelope) => match env_field_at(lfo_selected) {
            Some(field) => ActiveField::Envelope(address, field),
            None => ActiveField::Control,
        },
        Some(ModKind::Macro) => match macro_field_at(lfo_selected) {
            Some(field) => ActiveField::Macro(address, field),
            None => ActiveField::Control,
        },
        None => ActiveField::Control,
    }
}

pub(crate) fn macro_toggle_is_supported(
    automation: &AutomationState,
    lfo_selected: usize,
    selected_control: Option<&str>,
) -> bool {
    match active_field(automation, lfo_selected) {
        ActiveField::Lfo(_, field) => field.macro_key().is_some(),
        ActiveField::LfoStep(..) => false,
        ActiveField::LfoMacro(..) | ActiveField::Envelope(..) | ActiveField::Macro(..) => true,
        ActiveField::Control => selected_control.is_none_or(|id| !is_macro_id(id)),
    }
}

pub(crate) fn automation_kind_is_supported(
    selected_control: Option<&str>,
    kind: interaction::AutomationKind,
) -> bool {
    kind == interaction::AutomationKind::Lfo || selected_control.is_none_or(|id| !is_macro_id(id))
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
    let active = active_field(automation, lfo_selected);
    let recent_id = automation
        .active_address()
        .map(ControlAddress::id)
        .or_else(|| tab_specs(tab).get(selected).map(|spec| spec.id));
    effects.edit_session(AutoOwnership::TakeOver, recent_id, |snapshot| {
        let bpm = snapshot.controls.master.bpm;
        match active {
            ActiveField::Lfo(address, field) => {
                let is_flipped = lfo_time_key(field)
                    .is_some_and(|key| flipped.contains(&unit_key(address.id(), Some(key))));
                let Some(route) = snapshot.automation.route_mut(address) else {
                    return;
                };
                match (is_flipped, field) {
                    (true, LfoField::Interval) => {
                        let next = flipped_step(TimeBase::Beats, route.cycle_beats, dir, bpm);
                        route.set_field_raw_at(field, next, beat);
                    }
                    (true, LfoField::Offset) => {
                        let next =
                            flipped_step(TimeBase::Beats, route.phase_offset_beats, dir, bpm);
                        route.set_field_raw_at(field, next, beat);
                    }
                    _ => route.adjust_field_at(field, dir, beat),
                }
            }
            ActiveField::Envelope(address, field) => {
                let is_flipped = env_time_key(field)
                    .is_some_and(|key| flipped.contains(&unit_key(address.id(), Some(key))));
                let Some(route) = snapshot.automation.envelope_mut(address) else {
                    return;
                };
                match (is_flipped, field) {
                    (true, EnvField::Attack) => {
                        let next = flipped_step(TimeBase::Beats, route.attack_beats, dir, bpm);
                        route.set_field_raw(field, next);
                    }
                    (true, EnvField::Decay) => {
                        let next = flipped_step(TimeBase::Beats, route.decay_beats, dir, bpm);
                        route.set_field_raw(field, next);
                    }
                    _ => route.adjust_field(field, dir),
                }
            }
            ActiveField::LfoMacro(address, field, macro_field) => {
                let key = unit_key(address.id(), field.macro_key());
                if let Some(route) = snapshot.automation.field_macro_mut(&key) {
                    route.adjust_field(macro_field, dir);
                }
            }
            ActiveField::LfoStep(address, target) => {
                if let Some(route) = snapshot.automation.route_mut(address) {
                    route.adjust_step(target, dir);
                }
            }
            ActiveField::Macro(address, field) => {
                if let Some(route) = snapshot.automation.macro_route_mut(address) {
                    route.adjust_field(field, dir);
                }
            }
            ActiveField::Control => {
                if let Some(spec) = tab_specs(tab).get(selected)
                    && let Some((slot, module)) =
                        module_slot_at_id(tab, spec.id, &snapshot.controls)
                    && let Some(kind) = module.kind()
                {
                    let field = spec.id.rsplit('.').next();
                    if kind.family == Family::Delay && field == Some("clock") {
                        if let Some(slots) = snapshot.controls.modules.for_tab_mut(tab)
                            && let Some(module) = slots.get_mut(slot)
                        {
                            switch_delay_clock(module, false, bpm);
                        }
                        return;
                    }
                    if kind.family == Family::Delay && matches!(field, Some("time" | "right_time"))
                    {
                        if let Some(slots) = snapshot.controls.modules.for_tab_mut(tab)
                            && let Some(module) = slots.get_mut(slot)
                        {
                            let value = if field == Some("time") {
                                &mut module.time
                            } else {
                                &mut module.right_time
                            };
                            let clock = if field == Some("right_time") {
                                module.right_clock
                            } else {
                                module.clock
                            };
                            *value = match DelayClock::from_value(clock) {
                                DelayClock::Sync => beat_grid_adjust(
                                    *value,
                                    dir,
                                    DELAY_SYNC_MIN_BEATS,
                                    DELAY_SYNC_MAX_BEATS,
                                ),
                                DelayClock::Free => (*value + dir * 10.0)
                                    .clamp(DELAY_FREE_MIN_MS, DELAY_FREE_MAX_MS),
                            };
                        }
                        return;
                    }
                }
                let flipped_spec = tab_specs(tab).get(selected).filter(|spec| {
                    spec.time_base != TimeBase::None && flipped.contains(&unit_key(spec.id, None))
                });
                match flipped_spec {
                    Some(spec) => {
                        let current = (spec.get)(&snapshot.controls);
                        spec.apply_raw(
                            flipped_step(spec.time_base, current, dir, bpm),
                            &mut snapshot.controls,
                        );
                    }
                    None => apply_delta(tab, selected, dir, &mut snapshot.controls),
                }
            }
        }
    });
}

pub(crate) fn reset_lfo_or_control(
    effects: &mut EffectExecutor,
    automation: &AutomationState,
    lfo_selected: usize,
    tab: Tab,
    selected: usize,
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
            ActiveField::Lfo(address, field) => {
                if let Some(route) = snapshot.automation.route_mut(address) {
                    route.reset_field_at(field, beat);
                }
            }
            ActiveField::Envelope(address, field) => {
                if let Some(route) = snapshot.automation.envelope_mut(address) {
                    route.reset_field(field);
                }
            }
            ActiveField::LfoMacro(address, field, macro_field) => {
                let key = unit_key(address.id(), field.macro_key());
                if let Some(route) = snapshot.automation.field_macro_mut(&key) {
                    route.reset_field(macro_field);
                }
            }
            ActiveField::LfoStep(address, target) => {
                if let Some(route) = snapshot.automation.route_mut(address) {
                    route.reset_step(target);
                }
            }
            ActiveField::Macro(address, field) => {
                if let Some(route) = snapshot.automation.macro_route_mut(address) {
                    route.reset_field(field);
                }
            }
            ActiveField::Control => apply_reset(tab, selected, &mut snapshot.controls),
        },
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
    let active = active_field(automation, lfo_selected);
    let recent_id = automation
        .active_address()
        .map(ControlAddress::id)
        .or_else(|| tab_specs(tab).get(selected).map(|spec| spec.id));
    effects.edit_session(AutoOwnership::TakeOver, recent_id, |snapshot| {
        let bpm = snapshot.controls.master.bpm;
        match active {
            ActiveField::Lfo(address, field) => {
                let is_flipped = lfo_time_key(field)
                    .is_some_and(|key| flipped.contains(&unit_key(address.id(), Some(key))));
                if let Some(route) = snapshot.automation.route_mut(address) {
                    if is_flipped {
                        // Typed ms is exact: convert and clamp, but don't snap
                        // back onto the beat grid.
                        route.set_field_raw_at(
                            field,
                            flip_entry(TimeBase::Beats, value, bpm),
                            beat,
                        );
                    } else {
                        route.set_field_at(field, value, beat);
                    }
                }
            }
            ActiveField::Envelope(address, field) => {
                let is_flipped = env_time_key(field)
                    .is_some_and(|key| flipped.contains(&unit_key(address.id(), Some(key))));
                if let Some(route) = snapshot.automation.envelope_mut(address) {
                    if is_flipped {
                        route.set_field_raw(field, flip_entry(TimeBase::Beats, value, bpm));
                    } else {
                        route.set_field(field, value);
                    }
                }
            }
            ActiveField::LfoMacro(address, field, macro_field) => {
                let key = unit_key(address.id(), field.macro_key());
                if let Some(route) = snapshot.automation.field_macro_mut(&key) {
                    route.set_field(macro_field, value);
                }
            }
            ActiveField::LfoStep(address, target) => {
                if let Some(route) = snapshot.automation.route_mut(address) {
                    route.set_step(target, value);
                }
            }
            ActiveField::Macro(address, field) => {
                if let Some(route) = snapshot.automation.macro_route_mut(address) {
                    route.set_field(field, value);
                }
            }
            ActiveField::Control => {
                if let Some(spec) = tab_specs(tab).get(selected)
                    && let Some((slot, module)) =
                        module_slot_at_id(tab, spec.id, &snapshot.controls)
                    && module
                        .kind()
                        .is_some_and(|kind| kind.family == Family::Delay)
                    && matches!(spec.id.rsplit('.').next(), Some("time" | "right_time"))
                {
                    if let Some(slots) = snapshot.controls.modules.for_tab_mut(tab)
                        && let Some(module) = slots.get_mut(slot)
                    {
                        let clock = if spec.id.ends_with(".right_time") {
                            module.right_clock
                        } else {
                            module.clock
                        };
                        let value = match DelayClock::from_value(clock) {
                            DelayClock::Sync => {
                                value.clamp(DELAY_SYNC_MIN_BEATS, DELAY_SYNC_MAX_BEATS)
                            }
                            DelayClock::Free => value.clamp(DELAY_FREE_MIN_MS, DELAY_FREE_MAX_MS),
                        };
                        if spec.id.ends_with(".time") {
                            module.time = value;
                        } else {
                            module.right_time = value;
                        }
                    }
                    return;
                }
                match tab_specs(tab).get(selected) {
                    Some(spec)
                        if spec.time_base != TimeBase::None
                            && flipped.contains(&unit_key(spec.id, None)) =>
                    {
                        // Typed input in the flipped unit is exact: convert and
                        // clamp, but don't snap onto the native step grid.
                        spec.apply_raw(
                            flip_entry(spec.time_base, value, bpm),
                            &mut snapshot.controls,
                        );
                    }
                    _ => apply_value(tab, selected, value, &mut snapshot.controls),
                }
            }
        }
    });
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
        ActiveField::LfoMacro(..) | ActiveField::Macro(..) | ActiveField::LfoStep(..) => None,
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

pub(crate) fn toggle_macro_effect(
    effects: &mut EffectExecutor,
    automation: &AutomationState,
    selected_control: Option<&'static str>,
    lfo_selected: usize,
) -> Option<(interaction::LfoDepth, usize)> {
    match active_field(automation, lfo_selected) {
        ActiveField::Lfo(address, field)
            if !is_macro_id(address.id()) && field.macro_key().is_some() =>
        {
            let key = unit_key(address.id(), field.macro_key());
            effects.edit_session(AutoOwnership::TakeOver, Some(address.id()), |snapshot| {
                let state = &mut snapshot.automation;
                state.toggle_open_field(key.clone());
            });
            let current = effects.session().load();
            let depth = if current.automation.field_macro_owner(address).is_some() {
                interaction::LfoDepth::NestedField
            } else {
                interaction::LfoDepth::Editor
            };
            Some((depth, field_row_index(&current.automation, address, field)))
        }
        ActiveField::LfoMacro(address, field, _) => {
            effects.edit_session(AutoOwnership::TakeOver, Some(address.id()), |snapshot| {
                snapshot.automation.close_open_field();
            });
            let current = effects.session().load();
            Some((
                interaction::LfoDepth::Editor,
                field_row_index(&current.automation, address, field),
            ))
        }
        ActiveField::Lfo(_, _)
        | ActiveField::LfoStep(..)
        | ActiveField::Envelope(..)
        | ActiveField::Macro(..)
        | ActiveField::Control => {
            if let Some(id) = selected_control {
                let mut selected = 1;
                open_modulator_effect_for_id(effects, id, ModKind::Macro, &mut selected);
            }
            None
        }
    }
}

pub(crate) fn remove_automation_effect(
    effects: &mut EffectExecutor,
    automation: &AutomationState,
    selected_control: Option<&'static str>,
    lfo_selected: usize,
) {
    match active_field(automation, lfo_selected) {
        ActiveField::LfoMacro(address, field, _) => {
            let key = unit_key(address.id(), field.macro_key());
            effects.edit_session(AutoOwnership::TakeOver, Some(address.id()), |snapshot| {
                let state = &mut snapshot.automation;
                state.remove_field_macro(&key);
            });
        }
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

pub(crate) struct NumericDisplay<'a> {
    entry: Option<&'a str>,
    cursor_visible: bool,
}

pub(crate) fn render(f: &mut Frame, view: &UiViewModel<'_>) {
    let items = &view.items;
    let active_tab = view.navigation.tab;
    let selected = view.navigation.selected;
    let active_automation = match &view.mode {
        ModeSurface::Automation(surface) => Some(surface),
        // Numeric entry opened from inside an editor keeps that editor drawn,
        // so the typed buffer lands on the field being edited.
        ModeSurface::Numeric { resume, .. } => resume.as_ref(),
        _ => None,
    };
    let lfo_selected = active_automation.map_or(0, |surface| surface.selected());
    let beat = view.telemetry.beat;
    let numeric = NumericDisplay {
        entry: match &view.mode {
            ModeSurface::Numeric { entry, .. } => Some(entry.as_str()),
            _ => None,
        },
        cursor_visible: view.cursor_visible,
    };
    let fluid = view.fluid;
    let automation = &view.session.automation;
    let controls = &view.session.controls;
    let flipped = view.flipped;
    let chord_drill = view.navigation.chord_drill;
    let active_chord = view.telemetry.active_chord;
    let mute = view.mute;
    let bpm = controls.master.bpm;
    // Which custom-chord slot the pad engine is currently sounding, mapped
    // from the shared telemetry step index. Only meaningful on the Chords tab.
    let chord_count =
        (controls.pad.chord_count.round() as usize).clamp(1, controls.pad.chord_slots.len());
    let active_slot = (active_chord as usize) % chord_count;
    let mod_ctx = ModContext {
        beat,
        kick_interval_beats: controls.kick.interval_beats,
        kick_offset_beats: controls.kick.offset_beats,
    };
    let area = f.area();
    f.render_widget(FluidWidget { fluid }, area);

    // centered control overlay
    let pw = ((area.width as f32 * 0.62) as u16)
        .clamp(
            MIN_TERMINAL_WIDTH,
            area.width.saturating_sub(2).max(MIN_TERMINAL_WIDTH),
        )
        .min(area.width);
    let ph = ((area.height as f32 * 0.92) as u16)
        .clamp(
            MIN_TERMINAL_HEIGHT,
            area.height.saturating_sub(2).max(MIN_TERMINAL_HEIGHT),
        )
        .min(area.height);
    let px = area.x + (area.width.saturating_sub(pw)) / 2;
    let py = area.y + (area.height.saturating_sub(ph)) / 2;
    let panel = Rect::new(px, py, pw, ph);

    // Frosted-glass scrim: darken the live fluid underneath instead of covering
    // it, so the visualizer still shows through the panel.
    {
        let buf = f.buffer_mut();
        for y in panel.top()..panel.bottom() {
            for x in panel.left()..panel.right() {
                let cell = &mut buf[(x, y)];
                let tint = darken(cell.fg, 0.30);
                cell.set_char(' ');
                cell.set_bg(tint);
                cell.set_fg(Color::Rgb(30, 34, 44));
            }
        }
    }

    // Borders only (transparent fill) so the scrim shows through.
    let block = Block::default()
        .title(format!(
            " {APP_ID} v{} · {} ",
            env!("CARGO_PKG_VERSION"),
            view.owner.label()
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(150, 160, 185)));
    let inner = block.inner(panel);
    f.render_widget(block, panel);

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // 0 top pad
            Constraint::Length(1), // 1 pad
            Constraint::Length(1), // 2 tab line
            Constraint::Length(1), // 3 pad
            Constraint::Min(0),    // 4 control rows
            Constraint::Length(1), // 5 footer
        ])
        .split(inner);

    let tab_line: String = Tab::all()
        .iter()
        .map(|t| {
            let name = if *t == active_tab
                && let Some(slot) = view.navigation.module_slot
            {
                let module = controls
                    .modules
                    .for_tab(*t)
                    .and_then(|slots| slots[slot].kind());
                format!(
                    "{} › {}",
                    t.name(),
                    module.map_or("Module", |kind| kind.display_name)
                )
            } else if *t == Tab::Chords {
                match chord_drill {
                    interaction::ChordDrill::Progression { .. } => {
                        format!("{} › Progression", t.name())
                    }
                    interaction::ChordDrill::Slot { slot: n, .. } => {
                        let live = if n == active_slot { " ♪" } else { "" };
                        format!("{} › Chord {}{live}", t.name(), n + 1)
                    }
                    interaction::ChordDrill::None => t.name().to_string(),
                }
            } else {
                t.name().to_string()
            };
            let name = if mute[*t as usize].is_some() {
                format!("{name} (M)")
            } else {
                name
            };
            if *t == active_tab {
                format!("[{name}]")
            } else {
                name
            }
        })
        .collect::<Vec<_>>()
        .join("  ");
    f.render_widget(
        Paragraph::new(tab_line).alignment(Alignment::Center).style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        layout[2],
    );

    // One text row per control, blank line between for vertical breathing room.
    let bar_w = (inner.width as usize).saturating_sub(34).clamp(6, 80);
    let mut rows: Vec<Line> = Vec::with_capacity(items.len() * 3);
    for (i, item) in items.iter().enumerate() {
        let active = i == selected;
        let address = ControlAddress::new(item.id);
        let route = automation.route(address);
        let envelope = automation.envelope(address);
        let macro_route = automation.macro_route(address);
        let macro_mod = live_macro_contribution(automation, controls, address, mod_ctx);
        let editor_here =
            active_automation.and_then(|surface| surface.active_address()) == Some(address);
        let lfo_open_here = matches!(
            active_automation,
            Some(AutomationSurface::Lfo {
                address: active,
                ..
            }) if *active == address
        );
        let env_open_here = matches!(
            active_automation,
            Some(AutomationSurface::Envelope {
                address: active,
                ..
            }) if *active == address
        );
        let macro_open_here = matches!(
            active_automation,
            Some(AutomationSurface::Macro {
                address: active,
                ..
            }) if *active == address
        );
        let editor_open_here = editor_here;
        let parent_active = active && (!editor_open_here || lfo_selected == 0);
        let prefix = if parent_active { "▶ " } else { "  " };
        let display =
            numeric_cursor(&numeric, parent_active).unwrap_or_else(|| item.display.clone());
        let display = if (numeric.entry.is_some() && parent_active)
            || !flipped.contains(&unit_key(item.id, None))
        {
            display
        } else {
            flip_display(address.spec().time_base, item.value, bpm).unwrap_or(display)
        };
        let fg = if parent_active {
            Color::Rgb(120, 230, 255)
        } else {
            Color::Rgb(170, 178, 195)
        };
        let mut style = Style::default().fg(fg);
        if parent_active {
            style = style.add_modifier(Modifier::BOLD);
        }
        // The LFO route folded with any macro stacked onto its own fields
        // (amount/interval/offset), so markers show what the engine hears.
        let effective_lfo =
            route.map(|r| live_effective_lfo_route(automation, controls, address, r, mod_ctx));
        let markers = {
            let spec = address.spec();
            // Markers all sit on the same tapered bar as the value itself.
            let base = item.value;
            let ratio_of = |value: f32| spec.ratio(value);
            // Ghosts only for sources that actually contribute.
            let lfo = effective_lfo
                .as_ref()
                .filter(|r| r.depth_ratio > f32::EPSILON);
            let env = envelope.filter(|r| r.amount.abs() > f32::EPSILON);
            let single = |l: Option<&LfoRoute>, e: Option<&EnvelopeRoute>, m: Option<f32>| {
                ratio_of(modulated_control_value_full(spec, l, e, m, base, mod_ctx))
            };
            // While an editor is open on this control, faintly shade the full
            // reach of every active source (its full throw, not just the
            // live instant) so turning a depth/amount knob previews how far
            // it can push the effective value.
            let mod_range = spec.max - spec.min;
            let shadow = editor_here.then(|| {
                let mut lo = base;
                let mut hi = base;
                if let Some(r) = effective_lfo.as_ref() {
                    let swing = mod_range * r.depth_ratio.clamp(0.0, 1.0);
                    lo = lo.min(base - swing);
                    hi = hi.max(base + swing);
                }
                if let Some(r) = envelope {
                    let swing = mod_range * r.amount.clamp(-1.0, 1.0);
                    lo = lo.min(base + swing.min(0.0));
                    hi = hi.max(base + swing.max(0.0));
                }
                if let Some(r) = macro_route {
                    let (swing_lo, swing_hi) = r.swing(mod_range);
                    lo = lo.min(base + swing_lo);
                    hi = hi.max(base + swing_hi);
                }
                (
                    ratio_of(lo.clamp(spec.min, spec.max)),
                    ratio_of(hi.clamp(spec.min, spec.max)),
                )
            });
            SliderMarkers {
                effective: (lfo.is_some() || env.is_some() || macro_mod.is_some())
                    .then(|| single(lfo, env, macro_mod)),
                lfo: lfo.map(|r| single(Some(r), None, None)),
                envelope: env.map(|r| single(None, Some(r), None)),
                macro_: macro_mod.map(|combined| single(None, None, Some(combined))),
                shadow,
            }
        };
        let mut spans = vec![Span::styled(format!("{prefix}{:<15} ", item.label), style)];
        spans.extend(slider_spans(item_ratio(item), markers, bar_w, style));
        spans.push(Span::styled(format!(" {display}"), style));
        // Badge the chord slot the pad engine is currently sounding, so the
        // progression list shows which chord is live. Distinct from the cursor
        // ▶ so a row can be both selected and playing.
        let chord_playing = active_tab == Tab::Chords
            && matches!(chord_drill, interaction::ChordDrill::Progression { .. })
            && i == active_slot;
        if chord_playing {
            spans.push(Span::styled(
                " ♪",
                Style::default()
                    .fg(Color::Rgb(255, 200, 90))
                    .add_modifier(Modifier::BOLD),
            ));
        }
        rows.push(Line::from(spans));

        if let Some(route) = route {
            if lfo_open_here {
                let AutomationSurface::Lfo {
                    state: lfo_state, ..
                } = active_automation.expect("LFO editor flag requires LFO surface")
                else {
                    unreachable!("LFO editor flag requires LFO surface");
                };
                for (fi, sub_row) in lfo_submenu_rows(lfo_state, address).iter().enumerate() {
                    match *sub_row {
                        LfoSubRow::Field(field) => {
                            let value_display = match field {
                                LfoField::Interval
                                    if flipped
                                        .contains(&unit_key(item.id, Some("lfo.interval"))) =>
                                {
                                    flip_display(TimeBase::Beats, route.cycle_beats, bpm)
                                }
                                LfoField::Offset
                                    if flipped.contains(&unit_key(item.id, Some("lfo.offset"))) =>
                                {
                                    flip_display(TimeBase::Beats, route.phase_offset_beats, bpm)
                                }
                                _ => None,
                            }
                            .unwrap_or_else(|| route.field_display(field));
                            rows.push(field_line(
                                field.label(),
                                &Dial::new(route.field_value(field), field.scale(), value_display),
                                lfo_selected == fi + 1,
                                &numeric,
                                bar_w,
                                LFO_PALETTE,
                            ));
                            // A macro stacked on this field but not currently
                            // expanded shows as a closed chip, same as a
                            // regular control's macro assignment.
                            if let Some(key_str) = field.macro_key() {
                                let key = unit_key(item.id, Some(key_str));
                                if let Some(field_route) = lfo_state.field_macro(&key)
                                    && !field_route.is_neutral()
                                {
                                    rows.push(macro_chip_line(field_route));
                                }
                            }
                        }
                        LfoSubRow::FieldMacro(field, macro_field) => {
                            let key = unit_key(item.id, field.macro_key());
                            let Some(field_route) = lfo_state.field_macro(&key) else {
                                continue;
                            };
                            rows.push(field_line(
                                &format!("· {}", macro_field.label()),
                                &Dial::new(
                                    field_route.field_value(macro_field),
                                    MacroField::SCALE,
                                    field_route.field_display(macro_field),
                                ),
                                lfo_selected == fi + 1,
                                &numeric,
                                bar_w,
                                MACRO_PALETTE,
                            ));
                        }
                        LfoSubRow::Step(target) => {
                            rows.push(field_line(
                                &route.step_label(target),
                                &Dial::new(
                                    route.step_value(target),
                                    LfoRoute::step_scale(target),
                                    route.step_display(target),
                                ),
                                lfo_selected == fi + 1,
                                &numeric,
                                bar_w,
                                LFO_PALETTE,
                            ));
                        }
                    }
                }
            }
            rows.push(lfo_lane_line(route, beat, bar_w, lfo_open_here));
        }
        if let Some(route) = envelope {
            if env_open_here {
                for (fi, field) in EnvField::ALL.iter().enumerate() {
                    let value_display = match field {
                        EnvField::Attack
                            if flipped.contains(&unit_key(item.id, Some("env.attack"))) =>
                        {
                            flip_display(TimeBase::Beats, route.attack_beats, bpm)
                        }
                        EnvField::Decay
                            if route.decay_beats > 0.0
                                && flipped.contains(&unit_key(item.id, Some("env.decay"))) =>
                        {
                            flip_display(TimeBase::Beats, route.decay_beats, bpm)
                        }
                        _ => None,
                    }
                    .unwrap_or_else(|| route.field_display(*field));
                    rows.push(field_line(
                        field.label(),
                        &Dial::new(route.field_value(*field), field.scale(), value_display),
                        lfo_selected == fi + 1,
                        &numeric,
                        bar_w,
                        ENV_PALETTE,
                    ));
                }
            }
            rows.push(env_lane_line(route, mod_ctx, bar_w, env_open_here));
        }
        if let Some(route) = macro_route {
            if macro_open_here {
                for (fi, field) in MacroField::ALL.iter().enumerate() {
                    rows.push(field_line(
                        &field.label(),
                        &Dial::new(
                            route.field_value(*field),
                            MacroField::SCALE,
                            route.field_display(*field),
                        ),
                        lfo_selected == fi + 1,
                        &numeric,
                        bar_w,
                        MACRO_PALETTE,
                    ));
                }
            } else {
                rows.push(macro_chip_line(route));
            }
        }
        if i + 1 < items.len() {
            rows.push(Line::from(""));
        }
    }
    if let ModeSurface::Performance(performance) = &view.mode {
        f.render_widget(Paragraph::new(performance_lines(performance)), layout[4]);
    } else {
        f.render_widget(Paragraph::new(rows), layout[4]);
    }

    let footer_style = if view.help.emphasized() {
        Style::default()
            .fg(Color::Rgb(255, 220, 120))
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Rgb(120, 128, 145))
    };
    f.render_widget(
        Paragraph::new(view.help.text())
            .alignment(Alignment::Center)
            .style(footer_style),
        layout[5],
    );

    if let ModeSurface::Palette(palette) = &view.mode {
        draw_palette(f, panel, &palette.state, controls, numeric.cursor_visible);
    }
}

fn performance_lines(surface: &PerformanceSurface) -> Vec<Line<'static>> {
    let selector = |value: Option<usize>| {
        value
            .and_then(|index| index.checked_add(1))
            .map_or_else(|| "none".to_string(), |index| index.to_string())
    };
    match surface {
        PerformanceSurface::Deck {
            selected,
            held_selectors,
            instruments,
        } => {
            let held = if held_selectors.is_empty() {
                "none".to_string()
            } else {
                held_selectors
                    .iter()
                    .map(performance_key)
                    .collect::<Vec<_>>()
                    .join("+")
            };
            let mut lines = vec![Line::from(format!(
                "DECK · selected {} · held {held}",
                selector(*selected)
            ))];
            if instruments.is_empty() {
                lines.push(Line::from("hold a/s/d/f, then tap h/l j/k u/i"));
            } else {
                lines.extend(instruments.iter().map(performance_instrument_line));
            }
            lines
        }
        PerformanceSurface::SequenceChoose { held_selector } => vec![
            Line::from("SEQUENCE · CHOOSE INSTRUMENT"),
            Line::from("instrument · waiting"),
            Line::from(format!("held · {}", selector(*held_selector))),
        ],
        PerformanceSurface::SequencePerform {
            instrument,
            held_selector,
            values,
        } => {
            let mut lines = vec![Line::from(format!(
                "SEQUENCE · PERFORM · held {}",
                selector(*held_selector)
            ))];
            if let Some(values) = values {
                lines.push(performance_instrument_line(values));
            } else {
                lines.push(Line::from(format!(
                    "instrument · {}",
                    selector(*instrument)
                )));
            }
            lines
        }
        PerformanceSurface::SequenceComplete {
            instrument,
            release_pending,
            values,
        } => {
            let mut lines = vec![Line::from("SEQUENCE · APPLIED")];
            if let Some(values) = values {
                lines.push(performance_instrument_line(values));
            } else {
                lines.push(Line::from(format!(
                    "instrument · {}",
                    selector(*instrument)
                )));
            }
            lines.push(Line::from(if *release_pending {
                "release action to return"
            } else {
                "Space rearm · Esc back"
            }));
            lines
        }
    }
}

/// Deck rows carry the same colour language as a browse row: idle grey,
/// focused cyan, and amber for an instrument the player is physically
/// holding. Without a style they rendered in the terminal default and read
/// as a different application.
const PERFORMANCE_PALETTE: FieldPalette = FieldPalette {
    active: Color::Rgb(120, 230, 255),
    idle: Color::Rgb(170, 178, 195),
};

const PERFORMANCE_HELD: Color = Color::Rgb(255, 200, 90);

fn performance_instrument_line(values: &PerformanceInstrumentSurface) -> Line<'static> {
    let marker = if values.held {
        "●"
    } else if values.focused {
        "▶"
    } else {
        " "
    };
    let mut style = Style::default().fg(if values.held {
        PERFORMANCE_HELD
    } else if values.focused {
        PERFORMANCE_PALETTE.active
    } else {
        PERFORMANCE_PALETTE.idle
    });
    if values.held || values.focused {
        style = style.add_modifier(Modifier::BOLD);
    }
    let mut spans = vec![Span::styled(
        format!(
            "{marker} {} {:<4}",
            performance_key(values.instrument),
            performance_name(values.instrument),
        ),
        style,
    )];
    // Three compact dials per instrument. The deck is deliberately denser
    // than a browse row, but each bar is the same primitive, so modulation
    // markers land here the moment the deck learns about routes.
    for (tag, item) in [
        ("L", &values.level),
        ("T", &values.length),
        ("D", &values.density),
    ] {
        let dial = control_dial(item);
        spans.push(Span::styled(format!(" {tag}"), style));
        spans.extend(slider_spans(
            dial.ratio(),
            SliderMarkers::default(),
            3,
            style,
        ));
        spans.push(Span::styled(
            compact_performance_value(&dial.display),
            style,
        ));
    }
    Line::from(spans)
}

fn compact_performance_value(value: &str) -> String {
    value
        .replace(" beats", "b")
        .replace(" beat", "b")
        .replace(' ', "")
}

fn performance_key(instrument: interaction::PerformanceInstrument) -> &'static str {
    match instrument {
        interaction::PerformanceInstrument::Pads => "a",
        interaction::PerformanceInstrument::Bass => "s",
        interaction::PerformanceInstrument::Kick => "d",
        interaction::PerformanceInstrument::Perc => "f",
    }
}

fn performance_name(instrument: interaction::PerformanceInstrument) -> &'static str {
    match instrument {
        interaction::PerformanceInstrument::Pads => "Pads",
        interaction::PerformanceInstrument::Bass => "Bass",
        interaction::PerformanceInstrument::Kick => "Kick",
        interaction::PerformanceInstrument::Perc => "Perc",
    }
}

/// Bottom-anchored palette overlay inside the main panel: prompt line,
/// best-first matches (fuzzy hits highlighted), staged edits, key help.
fn draw_palette(
    f: &mut Frame,
    panel: Rect,
    pal: &PaletteState,
    controls: &FluidControls,
    cursor_visible: bool,
) {
    const MAX_MATCH_ROWS: usize = 16;
    let max_rows_that_fit = panel.height.saturating_sub(6) as usize;
    let shown = pal.matches.len().min(MAX_MATCH_ROWS).min(max_rows_that_fit);
    let first_row = pal
        .selected
        .saturating_sub(shown / 2)
        .min(pal.matches.len().saturating_sub(shown));
    let staged_rows = usize::from(!pal.staged.is_empty()) as u16;
    // prompt + matches + optional staged line + help line, inside a border.
    let height = (shown as u16 + staged_rows + 4).min(panel.height.saturating_sub(2));
    let width = panel.width.saturating_sub(6).max(30).min(panel.width);
    let x = panel.x + (panel.width.saturating_sub(width)) / 2;
    let y = panel.bottom().saturating_sub(height + 1);
    let area = Rect::new(x, y, width, height);

    // Opaque scrim so the palette reads over the control rows behind it.
    {
        let buf = f.buffer_mut();
        for row in area.top()..area.bottom() {
            for col in area.left()..area.right() {
                let cell = &mut buf[(col, row)];
                cell.set_char(' ');
                cell.set_bg(Color::Rgb(18, 22, 32));
            }
        }
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(150, 160, 185)));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let cursor = if cursor_visible { "\u{258c}" } else { " " };
    let prompt = match pal.locked {
        Some(entry) => Line::from(vec![
            Span::styled("/", Style::default().fg(Color::Rgb(120, 128, 145))),
            Span::styled(
                pal.entry(entry).id().unwrap_or("module"),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" = ", Style::default().fg(Color::Rgb(120, 128, 145))),
            Span::styled(
                format!("{}{cursor}", pal.value_buf),
                Style::default().fg(Color::White),
            ),
        ]),
        None => Line::from(vec![
            Span::styled("/", Style::default().fg(Color::Rgb(120, 128, 145))),
            Span::styled(
                format!("{}{cursor}", pal.query),
                Style::default().fg(Color::White),
            ),
        ]),
    };

    let mut lines = vec![prompt];
    for (row, m) in pal.matches.iter().skip(first_row).take(shown).enumerate() {
        let entry = pal.entry(m.entry);
        let is_selected = first_row + row == pal.selected;
        let marker = if is_selected { "\u{25b8} " } else { "  " };
        let base = if is_selected {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Rgb(150, 158, 175))
        };
        let hit = Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD);
        let haystack = entry.haystack();
        let mut spans = vec![Span::styled(marker, base)];
        for (i, ch) in haystack.chars().enumerate() {
            let style = if m.hits.contains(&i) { hit } else { base };
            spans.push(Span::styled(ch.to_string(), style));
        }
        spans.push(Span::styled(
            format!("  {}", entry.value(controls)),
            Style::default().fg(Color::Rgb(120, 200, 170)),
        ));
        lines.push(Line::from(spans));
    }
    if !pal.staged.is_empty() {
        let staged = pal
            .staged
            .iter()
            .map(|edit| format!("{}\u{2192}{}", edit.id, edit.value))
            .collect::<Vec<_>>()
            .join("  ");
        lines.push(Line::from(Span::styled(
            format!("staged: {staged}"),
            Style::default().fg(Color::Rgb(255, 220, 120)),
        )));
    }
    lines.push(Line::from(Span::styled(
        "\u{21e5} complete   type value   \u{21b5} stage/jump   \u{21b5}\u{21b5} commit   ^B on bar   Esc cancel",
        Style::default().fg(Color::Rgb(120, 128, 145)),
    )));
    f.render_widget(Paragraph::new(lines), inner);
}

/// Colour pair for a modulator submenu: (active row, idle row).
#[derive(Clone, Copy)]
pub(crate) struct FieldPalette {
    active: Color,
    idle: Color,
}

pub(crate) const LFO_PALETTE: FieldPalette = FieldPalette {
    active: Color::Rgb(255, 130, 210),
    idle: Color::Rgb(190, 105, 210),
};

pub(crate) const ENV_PALETTE: FieldPalette = FieldPalette {
    active: Color::Rgb(140, 235, 175),
    idle: Color::Rgb(95, 195, 140),
};

pub(crate) const MACRO_PALETTE: FieldPalette = FieldPalette {
    active: Color::Rgb(255, 200, 120),
    idle: Color::Rgb(210, 160, 90),
};

/// Compact one-line reminder of a closed macro assignment under its control.
fn macro_chip_line(route: &MacroRoute) -> Line<'static> {
    Line::from(Span::styled(
        format!("    {:<15} ⇒ {}", "", route.summary()),
        Style::default().fg(MACRO_PALETTE.idle),
    ))
}

/// Shared numeric-entry cursor: renders the in-progress typed value with a
/// blinking cursor when this row is the active numeric-entry target.
fn numeric_cursor(numeric: &NumericDisplay<'_>, active: bool) -> Option<String> {
    let entry = active.then_some(numeric.entry).flatten()?;
    let cursor = if numeric.cursor_visible { "_" } else { " " };
    Some(format!("> {entry}{cursor}"))
}

/// Baseline submenu field row: label, dial bar, live display, shared
/// numeric-entry cursor. Every modulator field renders through this, so the
/// dial's own scale is the single thing deciding where the handle sits.
fn field_line(
    label: &str,
    dial: &Dial,
    active: bool,
    numeric: &NumericDisplay<'_>,
    bar_w: usize,
    palette: FieldPalette,
) -> Line<'static> {
    let mut style = Style::default().fg(if active { palette.active } else { palette.idle });
    if active {
        style = style.add_modifier(Modifier::BOLD);
    }
    let prefix = if active { "▶ " } else { "  " };
    let display = numeric_cursor(numeric, active).unwrap_or_else(|| dial.display.clone());
    let bar = ratio_bar(dial.ratio(), bar_w, '█', '░');
    Line::from(Span::styled(
        format!("{prefix}  {label:<13} {bar} {display}"),
        style,
    ))
}

/// A registry control's dial: its declared step and taper decide the mapping,
/// so a row's bar can never disagree with how its value actually moves.
pub(crate) fn control_dial(item: &ControlItem) -> Dial {
    let value = match item.kind {
        ControlKind::Discrete => item.value.round(),
        ControlKind::Gain | ControlKind::Continuous | ControlKind::Timing => item.value,
    };
    Dial::new(
        value,
        DialScale::from_step(item.min, item.max, item.step, item.taper),
        item.display.clone(),
    )
}

const LANE_WAVE: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];

/// How many random cycles the lane scopes so sample & hold / random drift read
/// as an actual scrolling trajectory rather than a single flat step.
const RANDOM_LANE_CYCLES: f32 = 4.0;

fn lane_glyph(level: f32) -> &'static str {
    let level = level.clamp(0.0, 1.0);
    LANE_WAVE[((level * (LANE_WAVE.len() - 1) as f32).round() as usize).min(LANE_WAVE.len() - 1)]
}

/// Blank label-width prefix shared by every modulator lane line, so lane
/// glyphs line up under the field label column.
fn lane_prefix() -> Span<'static> {
    Span::styled(
        format!("  {:<15} ", ""),
        Style::default().fg(Color::Rgb(130, 136, 160)),
    )
}

/// Live modulator lane. Periodic shapes draw one phase-locked cycle across the
/// width with a bright head at the current phase. Random shapes scroll the real
/// generated trajectory right-to-left, head at "now" on the right edge, so what
/// the lane shows is exactly what the engine plays.
pub(crate) fn lfo_lane_line(
    route: &LfoRoute,
    beat: f64,
    width: usize,
    active: bool,
) -> Line<'static> {
    let width = width.clamp(6, 80);
    let floor = if active { 0.35 } else { 0.25 };
    let mut spans = Vec::with_capacity(width + 1);
    spans.push(lane_prefix());

    if route.shape.is_random() {
        let window = f64::from(route.cycle_beats.max(MIN_LFO_CYCLE_BEATS) * RANDOM_LANE_CYCLES);
        for i in 0..width {
            let age = (width - 1 - i) as f64 / width as f64;
            let wave = route.wave_at(beat - age * window) * route.depth_ratio;
            let level = wave * 0.5 + 0.5;
            let brightness = (floor + (i as f32 / (width - 1) as f32) * 0.6).clamp(0.0, 1.0);
            let hue = 300.0 + wave * 25.0;
            spans.push(Span::styled(
                lane_glyph(level),
                Style::default().fg(fluid_hsv(hue, 0.6, brightness)),
            ));
        }
        return Line::from(spans);
    }

    let head = (route.pattern_phase_at(beat) * width as f64) as usize % width;
    for i in 0..width {
        let phase = i as f32 / width as f32;
        let wave = route.shape_value_at_phase(phase) * route.depth_ratio;
        let level = wave * 0.5 + 0.5;
        let raw = i.abs_diff(head);
        let wrapped = raw.min(width - raw);
        let falloff = 1.0 - (wrapped as f32 / width as f32) * 2.0;
        let brightness = (floor + falloff.max(0.0) * 0.6).clamp(0.0, 1.0);
        let hue = 300.0 + wave * 25.0;
        spans.push(Span::styled(
            lane_glyph(level),
            Style::default().fg(fluid_hsv(hue, 0.6, brightness)),
        ));
    }
    Line::from(spans)
}

/// Envelope lane: the one-shot AD ramp across one trigger period, with a bright
/// head at the live phase. Uses the same `level_at` math as the engine.
pub(crate) fn env_lane_line(
    route: &EnvelopeRoute,
    ctx: ModContext,
    width: usize,
    active: bool,
) -> Line<'static> {
    let width = width.clamp(6, 80);
    let floor = if active { 0.35 } else { 0.25 };
    let window = f64::from(route.window_beats());
    let head_phase = route.lane_head_phase(ctx);
    let head = ((head_phase * width as f32) as usize).min(width - 1);

    let mut spans = Vec::with_capacity(width + 1);
    spans.push(lane_prefix());
    for i in 0..width {
        let col_since = (i as f64 / width as f64 * window) as f32;
        let level = route.level_for_lane(col_since) * route.amount.abs();
        let raw = i.abs_diff(head);
        let falloff = 1.0 - (raw as f32 / width as f32) * 2.0;
        let brightness = (floor + falloff.max(0.0) * 0.6).clamp(0.0, 1.0);
        let hue = if route.amount >= 0.0 { 150.0 } else { 15.0 };
        spans.push(Span::styled(
            lane_glyph(level).to_string(),
            Style::default().fg(fluid_hsv(hue, 0.55, brightness)),
        ));
    }
    Line::from(spans)
}

/// Live marker positions on a slider, all as 0..1 bar ratios. `effective` is
/// the summed value the engine plays; the per-source entries are base plus
/// that source alone, drawn as dim ghost diamonds so a diverging cursor is
/// explained at a glance (pink = LFO, green = envelope, amber = macro).
#[derive(Default, Clone, Copy)]
pub(crate) struct SliderMarkers {
    pub(crate) effective: Option<f32>,
    pub(crate) lfo: Option<f32>,
    pub(crate) envelope: Option<f32>,
    pub(crate) macro_: Option<f32>,
    /// Faint reach band (lo, hi ratios) showing the full throw of every
    /// active source while its editor is open — a preview of how far the
    /// effective value could swing, not just where it sits this instant.
    pub(crate) shadow: Option<(f32, f32)>,
}

const EFFECTIVE_MARKER_COLOR: Color = Color::Rgb(235, 245, 255);
const SHADOW_COLOR: Color = Color::Rgb(95, 100, 115);

/// Slider bar spans with ghost diamonds per modulation source, a faint reach
/// band, and one bright diamond at the effective value. Precedence: the
/// effective marker wins overlaps, then ghosts, then the actual filled bar,
/// then the shadow band, then empty track.
fn slider_spans(
    ratio: f32,
    markers: SliderMarkers,
    width: usize,
    style: Style,
) -> Vec<Span<'static>> {
    let filled = (ratio.clamp(0.0, 1.0) * width as f32).round() as usize;
    let cell = |value: Option<f32>| {
        value.map(|v| (v.clamp(0.0, 1.0) * width.saturating_sub(1) as f32).round() as usize)
    };
    let effective = cell(markers.effective);
    let ghosts = [
        (cell(markers.lfo), LFO_PALETTE.idle),
        (cell(markers.envelope), ENV_PALETTE.idle),
        (cell(markers.macro_), MACRO_PALETTE.idle),
    ];
    let shadow_range = markers.shadow.map(|(lo, hi)| {
        let lo = cell(Some(lo)).unwrap_or(0);
        let hi = cell(Some(hi)).unwrap_or(0);
        lo.min(hi)..=lo.max(hi)
    });
    (0..width)
        .map(|i| {
            if Some(i) == effective {
                Span::styled(
                    "◆",
                    Style::default()
                        .fg(EFFECTIVE_MARKER_COLOR)
                        .add_modifier(Modifier::BOLD),
                )
            } else if let Some((_, color)) = ghosts.iter().find(|(pos, _)| *pos == Some(i)) {
                Span::styled("◇", Style::default().fg(*color))
            } else if i < filled {
                Span::styled("█", style)
            } else if shadow_range.as_ref().is_some_and(|r| r.contains(&i)) {
                Span::styled("▒", Style::default().fg(SHADOW_COLOR))
            } else {
                Span::styled("░", style)
            }
        })
        .collect()
}

pub(crate) fn item_ratio(item: &ControlItem) -> f32 {
    control_dial(item).ratio()
}

pub(crate) fn ratio_bar(ratio: f32, width: usize, filled: char, empty: char) -> String {
    let filled_count = (ratio.clamp(0.0, 1.0) * width as f32).round() as usize;
    let filled_count = filled_count.min(width);
    let empty_count = width.saturating_sub(filled_count);
    format!(
        "{}{}",
        filled.to_string().repeat(filled_count),
        empty.to_string().repeat(empty_count)
    )
}
