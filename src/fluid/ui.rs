use std::collections::BTreeSet;

use super::*;

const SAVE_MESSAGE_TTL: std::time::Duration = std::time::Duration::from_secs(3);

/// TUI redraw/frame-pacing interval. 30 fps is smooth for a terminal
/// visualizer (most terminal emulators don't reliably render faster than
/// that anyway) and roughly halves the render-thread CPU spent rebuilding
/// widgets and diffing the terminal buffer versus the previous 16ms (60 fps)
/// pacing, with no perceptible loss of animation smoothness or key latency
/// (queued key events are still drained fully every frame, so held keys
/// never fall behind).
const FRAME_INTERVAL: std::time::Duration = std::time::Duration::from_millis(33);

/// Chords tab's local navigation depth: base params, drilled into the
/// active slots' Root list, or drilled into one slot's secondary fields.
#[derive(Clone, Copy, PartialEq, Default)]
pub(crate) enum ChordDrill {
    #[default]
    None,
    Progression,
    Slot(usize),
}

/// Translates a Chords-tab visible-row index to its real `CHORDS_CONTROLS`
/// index for the positional registry setters; a no-op on every other tab.
fn chords_selected_index(tab: Tab, chord_drill: ChordDrill, selected: usize) -> usize {
    if tab == Tab::Chords {
        chords_flat_index(chord_drill, selected)
    } else {
        selected
    }
}

pub(crate) struct UiSession {
    pub(crate) controls: Arc<ArcSwap<FluidControls>>,
    pub(crate) automation: Arc<ArcSwap<AutomationState>>,
    pub(crate) tonal_sequence: Arc<ArcSwap<TonalSequenceState>>,
}

pub(crate) fn ui_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    session: UiSession,
    telemetry: Arc<FluidTelemetry>,
    initial_automation: AutomationState,
    updates: UpdateNotice,
    auto: AutoControls,
) -> Result<(), Box<dyn Error>> {
    let UiSession {
        controls,
        automation: automation_shared,
        tonal_sequence,
    } = session;
    let mut tab = Tab::Chords;
    let mut selected = 0usize;
    let mut lfo_selected = 0usize;
    let mut chord_drill = ChordDrill::None;
    let mut flipped = FlippedUnits::new();
    let mut numeric_entry: Option<NumericEntry> = None;
    let mut mute: MuteState = [None; 9];
    let mut fluid = FluidState::new();
    let mut last = Instant::now();
    let started = Instant::now();
    let mut save_message: Option<(String, Instant)> = None;
    let mut automation = PublishedAutomation::new(initial_automation, automation_shared);

    'ui: loop {
        let c = FluidControls::clone(&controls.load());
        if save_message
            .as_ref()
            .is_some_and(|(_, shown_at)| shown_at.elapsed() >= SAVE_MESSAGE_TTL)
        {
            save_message = None;
        }
        let update_message = updates.message();
        let automation_message = automation_footer(automation.state());
        let chords_message = chords_footer(tab, chord_drill);
        let in_auto = auto.is_running();
        let auto_message =
            in_auto.then_some("\u{25cf} AUTO morphing   a or touch any param to exit");
        let footer_message = save_message
            .as_ref()
            .map(|(message, _)| message.as_str())
            .or(automation_message.as_deref())
            .or(chords_message.as_deref())
            .or(auto_message)
            .or(update_message.as_deref());
        let items = if tab == Tab::Chords {
            chords_tab_controls(&c, chord_drill)
        } else {
            tab_controls(tab, &c)
        };
        let items_len = items.len();
        selected = selected.min(items_len.saturating_sub(1));

        let now = Instant::now();
        let dt = (now - last).as_secs_f32().min(0.05);
        last = now;
        fluid.tick(dt, &telemetry);

        let cursor_visible = (started.elapsed().as_millis() / 400).is_multiple_of(2);
        let beat = telemetry.beat();
        terminal.draw(|f| {
            render(
                f,
                &items,
                tab,
                selected,
                lfo_selected,
                beat,
                NumericDisplay {
                    entry: numeric_entry.as_ref().map(|entry| entry.buffer.as_str()),
                    cursor_visible,
                },
                &fluid,
                automation.state(),
                &c,
                footer_message,
                &flipped,
                chord_drill,
                telemetry.chord_index.load(Ordering::Relaxed),
                &mute,
            )
        })?;

        // Drain every queued key event before the next draw so a held key
        // never falls behind the frame rate; the first poll doubles as the
        // frame pacing wait.
        let mut pending = event::poll(FRAME_INTERVAL)?;
        while pending {
            let event = event::read()?;
            pending = event::poll(std::time::Duration::ZERO)?;
            let Event::Key(key) = event else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            if let Some(entry) = numeric_entry.as_mut() {
                match key.code {
                    KeyCode::Esc => numeric_entry = None,
                    KeyCode::Enter => {
                        if entry.is_complete_number()
                            && let Ok(value) = entry.buffer.parse::<f32>()
                        {
                            set_modulator_or_control(
                                &mut automation,
                                lfo_selected,
                                &controls,
                                tab,
                                chords_selected_index(tab, chord_drill, selected),
                                value,
                                beat,
                                &flipped,
                            );
                            auto.exit(); // touching a param exits auto
                        }
                        numeric_entry = None;
                    }
                    KeyCode::Backspace => {
                        entry.buffer.pop();
                    }
                    KeyCode::Char(c) => entry.push(c),
                    _ => {}
                }
                continue;
            }
            match key.code {
                KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    save_message = Some(
                        match copy_launch_line(
                            &controls,
                            automation.state(),
                            &tonal_sequence.load(),
                        ) {
                            Ok(()) => ("nooise copied to clipboard".to_string(), Instant::now()),
                            Err(err) => (format!("Save failed: {err}"), Instant::now()),
                        },
                    );
                }
                // Esc only ever drills out one level (nested field-macro
                // editor, then the modulator editor, then nothing) — it
                // never quits, so escaping a deep edit can't risk exiting
                // the app. Only q and Ctrl+C do that.
                KeyCode::Esc if automation.state().is_editor_open() => {
                    close_one_level(&mut automation, &mut lfo_selected);
                }
                KeyCode::Esc if tab == Tab::Chords && chord_drill != ChordDrill::None => {
                    selected = match chord_drill {
                        ChordDrill::Slot(n) => n,
                        _ => 4,
                    };
                    chord_drill = match chord_drill {
                        ChordDrill::Slot(_) => ChordDrill::Progression,
                        _ => ChordDrill::None,
                    };
                }
                KeyCode::Esc => {}
                KeyCode::Char('q') => break 'ui,
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break 'ui,
                KeyCode::Tab | KeyCode::BackTab => {
                    if automation.state().is_editor_open() {
                        automation.edit(AutomationState::close_editor);
                    }
                    tab = if key.code == KeyCode::Tab {
                        tab.next()
                    } else {
                        tab.previous()
                    };
                    selected = 0;
                    lfo_selected = 0;
                    chord_drill = ChordDrill::None;
                }
                // Toggle auto-morph. On -> off swaps in `None` (the engine
                // stops rewriting controls, leaving the current morphed values
                // live). Off -> on builds a morph from the current state so
                // nothing jumps, heading to the nearest built-in state first.
                KeyCode::Char('a') => {
                    let current = FluidControls::clone(&controls.load());
                    auto.toggle(current, automation.state().clone(), beat);
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    if automation.state().is_editor_open() {
                        if automation.state().active_kind() == Some(ModKind::Lfo) {
                            lfo_selected = clamp_lfo_selection(
                                lfo_selected,
                                -1,
                                active_field_count(automation.state()),
                            );
                        } else if lfo_selected <= 1 {
                            automation.edit(AutomationState::close_editor);
                            lfo_selected = 0;
                        } else {
                            lfo_selected -= 1;
                        }
                    } else {
                        selected = selected.saturating_sub(1);
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if automation.state().is_editor_open() {
                        if automation.state().active_kind() == Some(ModKind::Lfo) {
                            lfo_selected = clamp_lfo_selection(
                                lfo_selected,
                                1,
                                active_field_count(automation.state()),
                            );
                        } else if lfo_selected >= active_field_count(automation.state()) {
                            automation.edit(AutomationState::close_editor);
                            selected = selected.saturating_add(1).min(items_len.saturating_sub(1));
                            lfo_selected = 0;
                        } else {
                            lfo_selected += 1;
                        }
                    } else {
                        selected = selected.saturating_add(1).min(items_len.saturating_sub(1));
                    }
                }
                KeyCode::Left | KeyCode::Char('h') | KeyCode::Char('H') => {
                    auto.exit(); // touching a param exits auto
                    let idx = chords_selected_index(tab, chord_drill, selected);
                    if key.code == KeyCode::Char('H') || key.modifiers.contains(KeyModifiers::SHIFT)
                    {
                        reset_lfo_or_control(
                            &mut automation,
                            lfo_selected,
                            &controls,
                            tab,
                            idx,
                            beat,
                        );
                    } else {
                        adjust_lfo_or_control(
                            &mut automation,
                            lfo_selected,
                            &controls,
                            tab,
                            idx,
                            -1.0,
                            beat,
                            &flipped,
                        );
                    }
                }
                KeyCode::Right | KeyCode::Char('l') => {
                    auto.exit(); // touching a param exits auto
                    adjust_lfo_or_control(
                        &mut automation,
                        lfo_selected,
                        &controls,
                        tab,
                        chords_selected_index(tab, chord_drill, selected),
                        1.0,
                        beat,
                        &flipped,
                    );
                }
                KeyCode::Char(c @ ('f' | 'e')) => {
                    auto.exit(); // touching a modulator exits auto
                    let kind = if c == 'f' {
                        ModKind::Lfo
                    } else {
                        ModKind::Envelope
                    };
                    open_modulator(&mut automation, &items, selected, kind, &mut lfo_selected);
                }
                KeyCode::Char('v') => {
                    auto.exit(); // touching a modulator exits auto
                    match active_field(automation.state(), lfo_selected) {
                        // On an LFO field row: stack (or un-stack) a macro
                        // onto that specific field, never on by default.
                        ActiveField::Lfo(address, field)
                            if !is_macro_id(address.id()) && field.macro_key().is_some() =>
                        {
                            let key = unit_key(address.id(), field.macro_key());
                            let was_open = automation.state().open_field() == Some(key.as_str());
                            automation.edit(|state| state.toggle_open_field(key));
                            let base = field_row_index(automation.state(), address, field);
                            lfo_selected = if was_open { base } else { base + 1 };
                        }
                        // Already inside a field's nested macro rows: v
                        // closes just that, same as Esc — never hijacks the
                        // parent LFO editor into swapping to a top-level
                        // Macro editor.
                        ActiveField::LfoMacro(..) => {
                            close_one_level(&mut automation, &mut lfo_selected);
                        }
                        // Discrete fields (Shape) carry no macro_key and
                        // can't take a stacked macro — a continuous -1..1
                        // contribution has no defined meaning against an
                        // enum index. A true no-op, same as v being refused
                        // on a macro slider's own rows, not a fallthrough
                        // into hijacking the parent LFO editor.
                        ActiveField::Lfo(_, field) if field.macro_key().is_none() => {}
                        // Step-editor rows carry no macro_key either; v is a
                        // true no-op, never a fallthrough into the Macro editor.
                        ActiveField::LfoStep(..) => {}
                        // Anywhere else (including already inside the
                        // top-level Macro editor, where this closes it): v
                        // opens the Macro editor for the selected control,
                        // for any slider.
                        _ => {
                            open_modulator(
                                &mut automation,
                                &items,
                                selected,
                                ModKind::Macro,
                                &mut lfo_selected,
                            );
                        }
                    }
                }
                KeyCode::Char('x') | KeyCode::Char('X') => {
                    auto.exit(); // touching a modulator exits auto
                    match active_field(automation.state(), lfo_selected) {
                        // On an open field-macro row: remove just that
                        // stacked macro, keep the parent LFO editor open.
                        ActiveField::LfoMacro(address, field, _) => {
                            let key = unit_key(address.id(), field.macro_key());
                            automation.edit(|state| state.remove_field_macro(&key));
                            lfo_selected = field_row_index(automation.state(), address, field);
                        }
                        _ if automation.state().is_editor_open() => {
                            automation.edit(AutomationState::remove_open_route);
                            lfo_selected = 0;
                        }
                        _ => {
                            if let Some(item) = items.get(selected) {
                                let address = ControlAddress::new(item.id);
                                automation.edit(|state| state.clear_control(address));
                            }
                        }
                    }
                }
                KeyCode::Enter => {
                    if !automation.state().is_editor_open() {
                        if tab == Tab::Chords {
                            match (chord_drill, items.get(selected).map(|i| i.id)) {
                                (ChordDrill::None, Some("pad.progression"))
                                    if is_custom_progression(progression_index(
                                        c.pad.progression,
                                    )) =>
                                {
                                    chord_drill = ChordDrill::Progression;
                                    selected = 0;
                                }
                                (ChordDrill::Progression, Some(_)) => {
                                    chord_drill = ChordDrill::Slot(selected);
                                    selected = 0;
                                }
                                _ => {}
                            }
                        } else if let Some(item) = items.get(selected)
                            && let Some(owner) = tab_owning_control(item.id)
                            && owner != tab
                        {
                            tab = owner;
                            selected = 0;
                            lfo_selected = 0;
                            chord_drill = ChordDrill::None;
                        }
                    }
                }
                KeyCode::Char('t') | KeyCode::Char('T') => {
                    if let Some(key) =
                        unit_toggle_key(automation.state(), lfo_selected, &items, selected)
                    {
                        let now_flipped = !flipped.remove(&key);
                        if now_flipped {
                            flipped.insert(key);
                        }
                        snap_after_unit_flip(
                            &mut automation,
                            lfo_selected,
                            &controls,
                            tab,
                            chords_selected_index(tab, chord_drill, selected),
                            now_flipped,
                            beat,
                        );
                    }
                }
                KeyCode::Char(c @ ('m' | 'M')) => {
                    let target = if c == 'm' { tab } else { Tab::Master };
                    toggle_mute(&controls, target, &mut mute);
                }
                KeyCode::Char('r') | KeyCode::Char('R') => {
                    if let Some(address) = automation.state().active_address()
                        && automation.state().active_kind() == Some(ModKind::Lfo)
                    {
                        auto.exit(); // touching a modulator exits auto
                        automation.edit(|state| {
                            if let Some(route) = state.route_mut(address)
                                && route.shape.is_random()
                            {
                                route.reseed();
                            }
                        });
                    }
                }
                KeyCode::Char(c) if c.is_ascii_digit() || c == '.' || c == '-' => {
                    let mut entry = NumericEntry::default();
                    entry.push(c);
                    numeric_entry = Some(entry);
                }
                _ => {}
            }
        }
    }

    Ok(())
}

/// Submenu row 0 is the parent slider; rows 1.. map onto the modulator fields.
/// Fields whose display and numeric entry have been flipped to the opposite
/// time base (beats <-> ms) by pressing T on that row. Keyed per field, so
/// each slider carries its own unit; stepping always stays on the native
/// grid and conversion happens at the current BPM.
pub(crate) type FlippedUnits = BTreeSet<String>;

/// Stable key for a flippable field: the control id, plus a submenu field
/// qualifier for modulator rows (e.g. "kick.level#lfo.interval").
pub(crate) fn unit_key(id: &str, field: Option<&str>) -> String {
    match field {
        Some(field) => format!("{id}#{field}"),
        None => id.to_string(),
    }
}

pub(crate) fn beats_to_ms(beats: f32, bpm: f32) -> f32 {
    beats * 60_000.0 / bpm.max(1.0)
}

pub(crate) fn ms_to_beats(ms: f32, bpm: f32) -> f32 {
    ms * bpm.max(1.0) / 60_000.0
}

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
    automation: &mut PublishedAutomation,
    lfo_selected: usize,
    controls: &Arc<ArcSwap<FluidControls>>,
    tab: Tab,
    selected: usize,
    now_flipped: bool,
    beat: f64,
) {
    let bpm = controls.load().master.bpm;
    match active_field(automation.state(), lfo_selected) {
        // LFO rate accepts exact typed beat values, so an exact ms-authored
        // value stays exact when returning to beats. Offset retains its grid.
        ActiveField::Lfo(address, field) if !now_flipped => automation.edit(|state| {
            if let Some(route) = state.route_mut(address) {
                match field {
                    LfoField::Interval => {}
                    LfoField::Offset => route.set_field_at(field, route.phase_offset_beats, beat),
                    _ => {}
                }
            }
        }),
        ActiveField::Envelope(address, field) if !now_flipped => automation.edit(|state| {
            if let Some(route) = state.envelope_mut(address) {
                match field {
                    EnvField::Attack => route.set_field(field, route.attack_beats),
                    EnvField::Decay => route.set_field(field, route.decay_beats),
                    EnvField::Amount | EnvField::Trigger => {}
                }
            }
        }),
        ActiveField::Control => {
            let Some(spec) = tab_specs(tab).get(selected) else {
                return;
            };
            let mut next = FluidControls::clone(&controls.load());
            let current = (spec.get)(&next);
            match (spec.time_base, now_flipped) {
                // Back to native beats: land on the control's own grid.
                (TimeBase::Beats, false) => spec.apply_quantized_value(current, &mut next),
                // An ms control now displayed in beats: round to the nearest
                // divided beat.
                (TimeBase::Ms, true) => {
                    let beats =
                        snap_step(ms_to_beats(current, bpm), FLIP_BEAT_STEP).max(FLIP_BEAT_STEP);
                    spec.apply_raw(beats_to_ms(beats, bpm), &mut next);
                }
                _ => return,
            }
            controls.store(Arc::new(next));
        }
        _ => {}
    }
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
fn unit_toggle_key(
    automation: &AutomationState,
    lfo_selected: usize,
    items: &[ControlItem],
    selected: usize,
) -> Option<String> {
    match active_field(automation, lfo_selected) {
        ActiveField::Lfo(address, field) => {
            lfo_time_key(field).map(|key| unit_key(address.id(), Some(key)))
        }
        ActiveField::Envelope(address, field) => {
            env_time_key(field).map(|key| unit_key(address.id(), Some(key)))
        }
        ActiveField::LfoMacro(..) | ActiveField::Macro(..) | ActiveField::LfoStep(..) => None,
        ActiveField::Control => {
            let item = items.get(selected)?;
            let spec = spec_by_id(item.id)?;
            (spec.time_base != TimeBase::None).then(|| unit_key(item.id, None))
        }
    }
}

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

/// Which LFO field currently has its nested macro editor expanded, if any.
fn field_macro_owner(automation: &AutomationState, address: ControlAddress) -> Option<LfoField> {
    let open_key = automation.open_field()?;
    LfoField::ALL.into_iter().find(|field| {
        field
            .macro_key()
            .is_some_and(|key_str| unit_key(address.id(), Some(key_str)) == open_key)
    })
}

/// Close exactly one level of nesting on the open editor: a field-macro's
/// own editor if one is expanded, else the whole modulator editor. This is
/// the single place that governs "close the innermost open thing" — Esc and
/// re-pressing `v` on a nested field-macro row both route through it, so
/// drilling out one step never destroys more than what's actually open.
pub(crate) fn close_one_level(automation: &mut PublishedAutomation, lfo_selected: &mut usize) {
    let Some(address) = automation.state().active_address() else {
        return;
    };
    if let Some(field) = field_macro_owner(automation.state(), address) {
        automation.edit(AutomationState::close_open_field);
        *lfo_selected = field_row_index(automation.state(), address, field);
    } else {
        automation.edit(AutomationState::close_editor);
        *lfo_selected = 0;
    }
}

pub(crate) fn env_field_at(index: usize) -> Option<EnvField> {
    EnvField::ALL.get(index.checked_sub(1)?).copied()
}

pub(crate) fn macro_field_at(index: usize) -> Option<MacroField> {
    MacroField::ALL.get(index.checked_sub(1)?).copied()
}

/// Submenu row count for the currently open editor (0 when none is open).
pub(crate) fn active_field_count(automation: &AutomationState) -> usize {
    match automation.active_kind() {
        Some(ModKind::Lfo) => automation
            .active_address()
            .map_or(LfoField::ALL.len(), |address| {
                lfo_submenu_rows(automation, address).len()
            }),
        Some(ModKind::Envelope) => EnvField::ALL.len(),
        Some(ModKind::Macro) => MacroField::ALL.len(),
        None => 0,
    }
}

/// LFO editors are explicitly collapsed with `f` or Escape. Arrow navigation
/// stays inside the submenu and clamps at its first and last selectable rows.
pub(crate) fn clamp_lfo_selection(current: usize, direction: isize, row_count: usize) -> usize {
    if row_count == 0 {
        return 0;
    }
    current.saturating_add_signed(direction).clamp(1, row_count)
}

/// Whether a modulator kind can open on a control. Envelopes live only on
/// macro sliders; macro routes live only on regular controls (no macro
/// feeding another macro).
fn kind_allowed_on(kind: ModKind, id: &str) -> bool {
    match kind {
        ModKind::Lfo => true,
        ModKind::Envelope => is_macro_id(id),
        ModKind::Macro => !is_macro_id(id),
    }
}

/// Toggle a modulator editor of `kind` on the selected control: same kind on
/// the same control closes it (settings kept — x is the remove gesture),
/// otherwise the open editor is swapped for the requested one (created
/// audible-neutral).
pub(crate) fn open_modulator(
    automation: &mut PublishedAutomation,
    items: &[ControlItem],
    selected: usize,
    kind: ModKind,
    sub_selected: &mut usize,
) {
    if let Some(item) = items.get(selected) {
        if !kind_allowed_on(kind, item.id) {
            return;
        }
        let address = ControlAddress::new(item.id);
        automation.edit(|state| {
            let already =
                state.active_address() == Some(address) && state.active_kind() == Some(kind);
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
}

pub(crate) struct PublishedAutomation {
    state: AutomationState,
    shared: Arc<ArcSwap<AutomationState>>,
}

impl PublishedAutomation {
    pub(crate) fn new(state: AutomationState, shared: Arc<ArcSwap<AutomationState>>) -> Self {
        shared.store(Arc::new(state.clone()));
        Self { state, shared }
    }

    pub(crate) fn state(&self) -> &AutomationState {
        &self.state
    }

    pub(crate) fn edit(&mut self, edit: impl FnOnce(&mut AutomationState)) {
        edit(&mut self.state);
        self.shared.store(Arc::new(self.state.clone()));
    }
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

#[allow(clippy::too_many_arguments)]
pub(crate) fn adjust_lfo_or_control(
    automation: &mut PublishedAutomation,
    lfo_selected: usize,
    controls: &Arc<ArcSwap<FluidControls>>,
    tab: Tab,
    selected: usize,
    dir: f32,
    beat: f64,
    flipped: &FlippedUnits,
) {
    let bpm = controls.load().master.bpm;
    match active_field(automation.state(), lfo_selected) {
        ActiveField::Lfo(address, field) => {
            let is_flipped = lfo_time_key(field)
                .is_some_and(|key| flipped.contains(&unit_key(address.id(), Some(key))));
            automation.edit(|state| {
                let Some(route) = state.route_mut(address) else {
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
            });
        }
        ActiveField::Envelope(address, field) => {
            let is_flipped = env_time_key(field)
                .is_some_and(|key| flipped.contains(&unit_key(address.id(), Some(key))));
            automation.edit(|state| {
                let Some(route) = state.envelope_mut(address) else {
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
            });
        }
        ActiveField::LfoMacro(address, field, macro_field) => automation.edit(|state| {
            let key = unit_key(address.id(), field.macro_key());
            if let Some(route) = state.field_macro_mut(&key) {
                route.adjust_field(macro_field, dir);
            }
        }),
        ActiveField::LfoStep(address, target) => automation.edit(|state| {
            if let Some(route) = state.route_mut(address) {
                route.adjust_step(target, dir);
            }
        }),
        ActiveField::Macro(address, field) => automation.edit(|state| {
            if let Some(route) = state.macro_route_mut(address) {
                route.adjust_field(field, dir);
            }
        }),
        ActiveField::Control => {
            let flipped_spec = tab_specs(tab).get(selected).filter(|spec| {
                spec.time_base != TimeBase::None && flipped.contains(&unit_key(spec.id, None))
            });
            match flipped_spec {
                Some(spec) => {
                    let mut next = FluidControls::clone(&controls.load());
                    let current = (spec.get)(&next);
                    spec.apply_raw(flipped_step(spec.time_base, current, dir, bpm), &mut next);
                    controls.store(Arc::new(next));
                }
                None => adjust(controls, tab, selected, dir),
            }
        }
    }
}

fn reset_lfo_or_control(
    automation: &mut PublishedAutomation,
    lfo_selected: usize,
    controls: &Arc<ArcSwap<FluidControls>>,
    tab: Tab,
    selected: usize,
    beat: f64,
) {
    match active_field(automation.state(), lfo_selected) {
        ActiveField::Lfo(address, field) => automation.edit(|state| {
            if let Some(route) = state.route_mut(address) {
                route.reset_field_at(field, beat);
            }
        }),
        ActiveField::Envelope(address, field) => automation.edit(|state| {
            if let Some(route) = state.envelope_mut(address) {
                route.reset_field(field);
            }
        }),
        ActiveField::LfoMacro(address, field, macro_field) => automation.edit(|state| {
            let key = unit_key(address.id(), field.macro_key());
            if let Some(route) = state.field_macro_mut(&key) {
                route.reset_field(macro_field);
            }
        }),
        ActiveField::LfoStep(address, target) => automation.edit(|state| {
            if let Some(route) = state.route_mut(address) {
                route.reset_step(target);
            }
        }),
        ActiveField::Macro(address, field) => automation.edit(|state| {
            if let Some(route) = state.macro_route_mut(address) {
                route.reset_field(field);
            }
        }),
        ActiveField::Control => reset_to_min(controls, tab, selected),
    }
}

#[allow(clippy::too_many_arguments)]
fn set_modulator_or_control(
    automation: &mut PublishedAutomation,
    lfo_selected: usize,
    controls: &Arc<ArcSwap<FluidControls>>,
    tab: Tab,
    selected: usize,
    value: f32,
    beat: f64,
    flipped: &FlippedUnits,
) {
    let bpm = controls.load().master.bpm;
    match active_field(automation.state(), lfo_selected) {
        ActiveField::Lfo(address, field) => automation.edit(|state| {
            let is_flipped = lfo_time_key(field)
                .is_some_and(|key| flipped.contains(&unit_key(address.id(), Some(key))));
            if let Some(route) = state.route_mut(address) {
                if is_flipped {
                    // Typed ms is exact: convert and clamp, but don't snap
                    // back onto the beat grid.
                    route.set_field_raw_at(field, flip_entry(TimeBase::Beats, value, bpm), beat);
                } else {
                    route.set_field_at(field, value, beat);
                }
            }
        }),
        ActiveField::Envelope(address, field) => automation.edit(|state| {
            let is_flipped = env_time_key(field)
                .is_some_and(|key| flipped.contains(&unit_key(address.id(), Some(key))));
            if let Some(route) = state.envelope_mut(address) {
                if is_flipped {
                    route.set_field_raw(field, flip_entry(TimeBase::Beats, value, bpm));
                } else {
                    route.set_field(field, value);
                }
            }
        }),
        ActiveField::LfoMacro(address, field, macro_field) => automation.edit(|state| {
            let key = unit_key(address.id(), field.macro_key());
            if let Some(route) = state.field_macro_mut(&key) {
                route.set_field(macro_field, value);
            }
        }),
        ActiveField::LfoStep(address, target) => automation.edit(|state| {
            if let Some(route) = state.route_mut(address) {
                route.set_step(target, value);
            }
        }),
        ActiveField::Macro(address, field) => automation.edit(|state| {
            if let Some(route) = state.macro_route_mut(address) {
                route.set_field(field, value);
            }
        }),
        ActiveField::Control => {
            match tab_specs(tab).get(selected) {
                Some(spec)
                    if spec.time_base != TimeBase::None
                        && flipped.contains(&unit_key(spec.id, None)) =>
                {
                    // Typed input in the flipped unit is exact: convert and
                    // clamp, but don't snap onto the native step grid.
                    let mut next = FluidControls::clone(&controls.load());
                    spec.apply_raw(flip_entry(spec.time_base, value, bpm), &mut next);
                    controls.store(Arc::new(next));
                }
                _ => set_value(controls, tab, selected, value),
            }
        }
    }
}

fn automation_footer(automation: &AutomationState) -> Option<String> {
    let address = automation.active_address()?;
    match automation.active_kind()? {
        ModKind::Lfo => {
            let route = automation.route(address)?;
            let reseed = if route.shape.is_random() {
                "   r reseed"
            } else {
                ""
            };
            Some(format!(
                "LFO {}   {}   {:.2} beats   depth {:.0}%{reseed}   x remove   Esc close",
                address.id(),
                route.shape.label(),
                route.cycle_beats,
                route.depth_ratio * 100.0
            ))
        }
        ModKind::Envelope => {
            let route = automation.envelope(address)?;
            Some(format!(
                "ENV {}   {}   amount {:+.0}%   x remove   Esc close",
                address.id(),
                route.field_display(EnvField::Trigger),
                route.amount * 100.0
            ))
        }
        ModKind::Macro => {
            let route = automation.macro_route(address)?;
            Some(format!(
                "MACRO {}   {}   x remove   Esc close",
                address.id(),
                route.summary(),
            ))
        }
    }
}

pub(crate) fn chords_footer(tab: Tab, chord_drill: ChordDrill) -> Option<String> {
    if tab != Tab::Chords {
        return None;
    }
    match chord_drill {
        ChordDrill::None => None,
        ChordDrill::Progression => Some("Progression   Enter: open chord   Esc: back".to_string()),
        ChordDrill::Slot(n) => Some(format!("Chord {}   Esc: back", n + 1)),
    }
}

fn copy_launch_line(
    controls: &Arc<ArcSwap<FluidControls>>,
    automation: &AutomationState,
    tonal_sequence: &TonalSequenceState,
) -> Result<(), Box<dyn Error>> {
    let c = FluidControls::clone(&controls.load());
    let line = launch_line(&SongState {
        controls: c,
        automation: automation.clone(),
        tonal_sequence: Some(tonal_sequence.clone()),
    })?;
    let mut clipboard = arboard::Clipboard::new()?;
    clipboard.set_text(line)?;
    Ok(())
}

/// Per-tab mute state, indexed by `Tab as usize` (`Tab::Master` included at
/// its own slot 0). `Some(level)` holds the pre-mute value so unmuting
/// restores it exactly instead of snapping to a hardcoded level; UI-local
/// only, never persisted to song code.
pub(crate) type MuteState = [Option<f32>; 9];

/// Toggle mute on `tab`'s level/gain control: mute stores the live value and
/// zeroes it, unmute restores the stored value. No-op on a tab with no level
/// control to mute (`Macros`).
pub(crate) fn toggle_mute(controls: &Arc<ArcSwap<FluidControls>>, tab: Tab, mute: &mut MuteState) {
    let Some(id) = tab.level_id() else { return };
    let spec = spec_by_id(id).expect("tab level_id must name a real control");
    let mut next = FluidControls::clone(&controls.load());
    let slot = &mut mute[tab as usize];
    match slot.take() {
        Some(previous) => (spec.set)(&mut next, previous),
        None => {
            *slot = Some((spec.get)(&next));
            (spec.set)(&mut next, 0.0);
        }
    }
    controls.store(Arc::new(next));
}

pub(crate) fn adjust(controls: &Arc<ArcSwap<FluidControls>>, tab: Tab, selected: usize, dir: f32) {
    let mut next = FluidControls::clone(&controls.load());
    apply_delta(tab, selected, dir, &mut next);
    controls.store(Arc::new(next));
}

pub(crate) fn reset_to_min(controls: &Arc<ArcSwap<FluidControls>>, tab: Tab, selected: usize) {
    let mut next = FluidControls::clone(&controls.load());
    apply_min(tab, selected, &mut next);
    controls.store(Arc::new(next));
}

pub(crate) fn set_value(
    controls: &Arc<ArcSwap<FluidControls>>,
    tab: Tab,
    selected: usize,
    value: f32,
) {
    let mut next = FluidControls::clone(&controls.load());
    apply_value(tab, selected, value, &mut next);
    controls.store(Arc::new(next));
}

pub(crate) struct NumericDisplay<'a> {
    entry: Option<&'a str>,
    cursor_visible: bool,
}

#[cfg(test)]
impl NumericDisplay<'_> {
    pub(crate) fn empty() -> Self {
        Self {
            entry: None,
            cursor_visible: false,
        }
    }
}

// ============================================================
// Fluid visualizer: chords drive the field colour, kicks
// spawn ripples. Driven entirely by live audio-thread telemetry.
// ============================================================

pub(crate) const FLUID_GRADIENT: &[char] = &[' ', '·', '∙', '•', '●', '◉', '⬤'];
pub(crate) const RIPPLE_LIFETIME: f32 = 3.0;
pub(crate) const RIPPLE_SPEED: f32 = 0.42; // normalized units / s

/// One chord = one hue. Cycles with the pad engine's 5-chord table.
pub(crate) fn hue_for_chord(index: u64) -> f32 {
    const HUES: [f32; 5] = [205.0, 270.0, 325.0, 158.0, 38.0];
    HUES[(index % HUES.len() as u64) as usize]
}

pub(crate) struct FluidState {
    t: f32,
    ripples: Vec<(f32, f32, f32)>, // (cx, cy, age) in 0..1 field coords
    last_kick: u64,
    hue: f32,
}

impl FluidState {
    pub(crate) fn new() -> Self {
        Self {
            t: 0.0,
            ripples: Vec::new(),
            last_kick: 0,
            hue: hue_for_chord(0),
        }
    }

    pub(crate) fn tick(&mut self, dt: f32, telemetry: &FluidTelemetry) {
        self.t += dt;

        // kick pulses -> ripples (golden-angle scatter so they don't stack)
        let kick = telemetry.kick_pulse.load(Ordering::Relaxed);
        if kick > self.last_kick {
            let new = (kick - self.last_kick).min(4);
            for k in 0..new {
                let n = (self.last_kick + k + 1) as f32;
                // Kick ripples originate along the bottom edge and radiate up,
                // keeping them clear of the centered control panel.
                let cx = (n * 0.618_034).fract();
                let cy = 0.92 + (n * 0.381_966).fract() * 0.06;
                self.ripples.push((cx.clamp(0.06, 0.94), cy, 0.0));
            }
            self.last_kick = kick;
        }

        for r in &mut self.ripples {
            r.2 += dt;
        }
        self.ripples.retain(|r| r.2 < RIPPLE_LIFETIME);
    }

    /// Liquid field value in 0..1 at normalized coords, with ripple distortion.
    pub(crate) fn field(&self, nx: f32, ny: f32) -> f32 {
        let z = self.t * 0.5;
        let mut v = 0.0;
        v += (nx * 6.0 + z).sin() * (ny * 5.0 - z * 0.7).cos();
        v += ((nx * 3.3 - ny * 4.1) + z * 1.3).sin() * 0.7;
        v += (nx * 11.0 + ny * 9.0 - z * 0.4).sin() * 0.35;
        v += ((nx + ny) * 7.5 + (z * 0.9).sin() * 2.0).cos() * 0.5;

        for &(cx, cy, age) in &self.ripples {
            let dx = nx - cx;
            let dy = ny - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            let front = age * RIPPLE_SPEED;
            let fade = (1.0 - age / RIPPLE_LIFETIME).max(0.0);
            // small, tight ripple rising from the bottom edge
            let ring = (-((dist - front) * 12.0).powi(2)).exp();
            v += (dist * 34.0 - age * 9.0).sin() * ring * fade * 1.6;
        }

        (v / 3.0).tanh() * 0.5 + 0.5
    }
}

pub(crate) fn fluid_hsv(h: f32, s: f32, v: f32) -> Color {
    let h = ((h % 360.0) + 360.0) % 360.0;
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    Color::Rgb(
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
    )
}

pub(crate) struct FluidWidget<'a> {
    fluid: &'a FluidState,
}

impl Widget for FluidWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let w = area.width.max(1) as f32;
        let h = area.height.max(1) as f32;
        let base = self.fluid.hue;

        for y in 0..area.height {
            for x in 0..area.width {
                let nx = x as f32 / w;
                let ny = y as f32 / h;
                let v = self.fluid.field(nx, ny);

                // edge vignette
                let edge_x = (nx.min(1.0 - nx) * 2.0).min(1.0);
                let edge_y = (ny.min(1.0 - ny) * 2.0).min(1.0);
                let vig = (edge_x.min(edge_y) * 1.4).clamp(0.2, 1.0);

                let hue = base + (v - 0.5) * 45.0;
                let sat = (0.5 + v * 0.3).clamp(0.0, 1.0);
                let val = ((0.12 + v * 0.8) * vig).clamp(0.0, 1.0);

                let gi = ((v * (FLUID_GRADIENT.len() - 1) as f32).round() as usize)
                    .min(FLUID_GRADIENT.len() - 1);
                buf[(area.x + x, area.y + y)]
                    .set_char(FLUID_GRADIENT[gi])
                    .set_style(Style::default().fg(fluid_hsv(hue, sat, val)));
            }
        }
    }
}

/// Multiply an RGB colour toward black; non-RGB passes through unchanged.
pub(crate) fn darken(c: Color, factor: f32) -> Color {
    if let Color::Rgb(r, g, b) = c {
        Color::Rgb(
            (r as f32 * factor) as u8,
            (g as f32 * factor) as u8,
            (b as f32 * factor) as u8,
        )
    } else {
        c
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn render(
    f: &mut Frame,
    items: &[ControlItem],
    active_tab: Tab,
    selected: usize,
    lfo_selected: usize,
    beat: f64,
    numeric: NumericDisplay<'_>,
    fluid: &FluidState,
    automation: &AutomationState,
    controls: &FluidControls,
    update_message: Option<&str>,
    flipped: &FlippedUnits,
    chord_drill: ChordDrill,
    active_chord: u64,
    mute: &MuteState,
) {
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
        .clamp(46, area.width.saturating_sub(2).max(46))
        .min(area.width);
    let ph = ((area.height as f32 * 0.92) as u16)
        .clamp(10, area.height.saturating_sub(2).max(10))
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
        .title(format!(" {APP_ID} v{} ", env!("CARGO_PKG_VERSION")))
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
            let name = if *t == Tab::Chords {
                match chord_drill {
                    ChordDrill::Progression => format!("{} › Progression", t.name()),
                    ChordDrill::Slot(n) => {
                        let live = if n == active_slot { " ♪" } else { "" };
                        format!("{} › Chord {}{live}", t.name(), n + 1)
                    }
                    ChordDrill::None => t.name().to_string(),
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
        let editor_here = automation.active_address() == Some(address);
        let lfo_open_here = editor_here && automation.active_kind() == Some(ModKind::Lfo);
        let env_open_here = editor_here && automation.active_kind() == Some(ModKind::Envelope);
        let macro_open_here = editor_here && automation.active_kind() == Some(ModKind::Macro);
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
        let chord_playing =
            active_tab == Tab::Chords && chord_drill == ChordDrill::Progression && i == active_slot;
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
                for (fi, sub_row) in lfo_submenu_rows(automation, address).iter().enumerate() {
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
                                route.field_ratio(field),
                                value_display,
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
                                if let Some(field_route) = automation.field_macro(&key)
                                    && !field_route.is_neutral()
                                {
                                    rows.push(macro_chip_line(field_route));
                                }
                            }
                        }
                        LfoSubRow::FieldMacro(field, macro_field) => {
                            let key = unit_key(item.id, field.macro_key());
                            let Some(field_route) = automation.field_macro(&key) else {
                                continue;
                            };
                            rows.push(field_line(
                                &format!("· {}", macro_field.label()),
                                field_route.field_ratio(macro_field),
                                field_route.field_display(macro_field),
                                lfo_selected == fi + 1,
                                &numeric,
                                bar_w,
                                MACRO_PALETTE,
                            ));
                        }
                        LfoSubRow::Step(target) => {
                            rows.push(field_line(
                                &route.step_label(target),
                                route.step_ratio(target),
                                route.step_display(target),
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
                        route.field_ratio(*field),
                        value_display,
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
                        route.field_ratio(*field),
                        route.field_display(*field),
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
    f.render_widget(Paragraph::new(rows), layout[4]);

    let footer = update_message
        .unwrap_or("jk select   h/l adjust   f LFO   v macro   a auto   T units   q quit");
    let footer_style = if update_message.is_some() {
        Style::default()
            .fg(Color::Rgb(255, 220, 120))
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Rgb(120, 128, 145))
    };
    f.render_widget(
        Paragraph::new(footer)
            .alignment(Alignment::Center)
            .style(footer_style),
        layout[5],
    );
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

/// Baseline submenu field row: label, clamped ratio bar, live display, shared
/// numeric-entry cursor. Every modulator field renders through this.
fn field_line(
    label: &str,
    ratio: f32,
    value_display: String,
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
    let display = numeric_cursor(numeric, active).unwrap_or(value_display);
    let bar = ratio_bar(ratio, bar_w, '█', '░');
    Line::from(Span::styled(
        format!("{prefix}  {label:<13} {bar} {display}"),
        style,
    ))
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
    let value = match item.kind {
        ControlKind::Discrete => item.value.round(),
        ControlKind::Gain | ControlKind::Continuous | ControlKind::Timing => item.value,
    };
    item.step.ratio(value, item.min, item.max, item.taper)
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
