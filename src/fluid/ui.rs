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

pub(crate) struct NumericDisplay<'a> {
    entry: Option<&'a str>,
    cursor_visible: bool,
}

/// Everything the panel's sections derive from the view model once, so the
/// tab bar, control rows, and footer all draw from one reading of the frame.
struct PanelFrame<'a, 'v> {
    view: &'a UiViewModel<'v>,
    /// The modulator editor drawn this frame, if any — including the one a
    /// numeric entry was opened from, so the typed buffer lands on the field
    /// being edited rather than collapsing back to its parent row.
    automation: Option<&'a AutomationSurface<'v>>,
    lfo_selected: usize,
    numeric: NumericDisplay<'a>,
    mod_ctx: ModContext,
    /// Which custom-chord slot the pad engine is currently sounding, mapped
    /// from the shared telemetry step index. Only meaningful on Chords.
    active_slot: usize,
    bar_w: usize,
}

impl PanelFrame<'_, '_> {
    fn controls(&self) -> &FluidControls {
        &self.view.session.controls
    }

    fn automation_state(&self) -> &AutomationState {
        &self.view.session.automation
    }

    fn bpm(&self) -> f32 {
        self.view.session.controls.master.bpm
    }
}

pub(crate) fn render(f: &mut Frame, view: &UiViewModel<'_>) {
    let area = f.area();
    f.render_widget(FluidWidget { fluid: view.fluid }, area);

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

    draw_scrim(f, panel);

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

    let automation = match &view.mode {
        ModeSurface::Automation(surface) => Some(surface),
        ModeSurface::Numeric { resume, .. } => resume.as_ref(),
        _ => None,
    };
    let controls = &view.session.controls;
    let chord_count =
        (controls.pad.chord_count.round() as usize).clamp(1, controls.pad.chord_slots.len());
    let frame = PanelFrame {
        view,
        automation,
        lfo_selected: automation.map_or(0, |surface| surface.selected()),
        numeric: NumericDisplay {
            entry: match &view.mode {
                ModeSurface::Numeric { entry, .. } => Some(entry.as_str()),
                _ => None,
            },
            cursor_visible: view.cursor_visible,
        },
        mod_ctx: ModContext {
            beat: view.telemetry.beat,
            kick_interval_beats: controls.kick.interval_beats,
            kick_offset_beats: controls.kick.offset_beats,
        },
        active_slot: (view.telemetry.active_chord as usize) % chord_count,
        // One text row per control, blank line between for vertical breathing
        // room.
        bar_w: (inner.width as usize).saturating_sub(34).clamp(6, 80),
    };

    draw_tabs(f, layout[2], &frame);
    draw_control_rows(f, layout[4], &frame);
    draw_footer(f, layout[5], view);

    if let ModeSurface::Palette(palette) = &view.mode {
        draw_palette(
            f,
            panel,
            &palette.state,
            controls,
            frame.numeric.cursor_visible,
        );
    }
}

/// Frosted-glass scrim: darken the live fluid underneath instead of covering
/// it, so the visualizer still shows through the panel.
fn draw_scrim(f: &mut Frame, panel: Rect) {
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

/// The tab strip, with the active tab bracketed and carrying whatever it is
/// drilled into (a module, a chord slot, the progression).
fn draw_tabs(f: &mut Frame, area: Rect, frame: &PanelFrame<'_, '_>) {
    let view = frame.view;
    let active_tab = view.navigation.tab;
    let controls = frame.controls();
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
                match view.navigation.chord_drill {
                    interaction::ChordDrill::Progression { .. } => {
                        format!("{} › Progression", t.name())
                    }
                    interaction::ChordDrill::Slot { slot: n, .. } => {
                        let live = if n == frame.active_slot { " ♪" } else { "" };
                        format!("{} › Chord {}{live}", t.name(), n + 1)
                    }
                    interaction::ChordDrill::None => t.name().to_string(),
                }
            } else {
                t.name().to_string()
            };
            let name = if view.mute[*t as usize].is_some() {
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
        area,
    );
}

/// The control list: one row per control, each followed by whatever
/// modulation it carries — an open editor's fields, the live lanes, a closed
/// macro chip. Performance takes the same area over entirely.
fn draw_control_rows(f: &mut Frame, area: Rect, frame: &PanelFrame<'_, '_>) {
    let view = frame.view;
    if let ModeSurface::Performance(performance) = &view.mode {
        f.render_widget(Paragraph::new(performance_lines(performance)), area);
        return;
    }
    let items = &view.items;
    let selected = view.navigation.selected;
    let automation = frame.automation_state();
    let flipped = view.flipped;
    let bpm = frame.bpm();
    let beat = view.telemetry.beat;

    let mut rows: Vec<Line<'static>> = Vec::with_capacity(items.len() * 3);
    for (i, item) in items.iter().enumerate() {
        let active = i == selected;
        let address = ControlAddress::new(item.id);
        let route = automation.route(address);
        let envelope = automation.envelope(address);
        let macro_route = automation.macro_route(address);
        let editor_here = frame
            .automation
            .and_then(|surface| surface.active_address())
            == Some(address);
        let open_here = |kind: ModKind| match (frame.automation, kind) {
            (Some(AutomationSurface::Lfo { address: open, .. }), ModKind::Lfo)
            | (Some(AutomationSurface::Envelope { address: open, .. }), ModKind::Envelope)
            | (Some(AutomationSurface::Macro { address: open, .. }), ModKind::Macro) => {
                *open == address
            }
            _ => false,
        };
        let lfo_open_here = open_here(ModKind::Lfo);
        let parent_active = active && (!editor_here || frame.lfo_selected == 0);
        let prefix = if parent_active { "▶ " } else { "  " };
        let display =
            numeric_cursor(&frame.numeric, parent_active).unwrap_or_else(|| item.display.clone());
        let display = if (frame.numeric.entry.is_some() && parent_active)
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
        let markers = slider_markers(item, address, editor_here, frame);
        let mut spans = vec![Span::styled(format!("{prefix}{:<15} ", item.label), style)];
        spans.extend(slider_spans(item_ratio(item), markers, frame.bar_w, style));
        spans.push(Span::styled(format!(" {display}"), style));
        // Badge the chord slot the pad engine is currently sounding, so the
        // progression list shows which chord is live. Distinct from the cursor
        // ▶ so a row can be both selected and playing.
        let chord_playing = view.navigation.tab == Tab::Chords
            && matches!(
                view.navigation.chord_drill,
                interaction::ChordDrill::Progression { .. }
            )
            && i == frame.active_slot;
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
                } = frame
                    .automation
                    .expect("LFO editor flag requires LFO surface")
                else {
                    unreachable!("LFO editor flag requires LFO surface");
                };
                push_lfo_editor_rows(&mut rows, lfo_state, route, address, frame);
            }
            rows.push(lfo_lane_line(route, beat, frame.bar_w, lfo_open_here));
        }
        if let Some(route) = envelope {
            let env_open_here = open_here(ModKind::Envelope);
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
                        frame.lfo_selected == fi + 1,
                        &frame.numeric,
                        frame.bar_w,
                        ENV_PALETTE,
                    ));
                }
            }
            rows.push(env_lane_line(
                route,
                frame.mod_ctx,
                frame.bar_w,
                env_open_here,
            ));
        }
        if let Some(route) = macro_route {
            if open_here(ModKind::Macro) {
                for (fi, field) in MacroField::ALL.iter().enumerate() {
                    rows.push(field_line(
                        &field.label(),
                        &Dial::new(
                            route.field_value(*field),
                            MacroField::SCALE,
                            route.field_display(*field),
                        ),
                        frame.lfo_selected == fi + 1,
                        &frame.numeric,
                        frame.bar_w,
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
    f.render_widget(Paragraph::new(rows), area);
}

/// The rows of an open LFO editor: its own fields, any macro stacked on one
/// of them, and the inline step editor a Steps shape adds.
fn push_lfo_editor_rows(
    rows: &mut Vec<Line<'static>>,
    lfo_state: &AutomationState,
    route: &LfoRoute,
    address: ControlAddress,
    frame: &PanelFrame<'_, '_>,
) {
    let id = address.id();
    let flipped = frame.view.flipped;
    let bpm = frame.bpm();
    for (fi, sub_row) in lfo_submenu_rows(lfo_state, address).iter().enumerate() {
        let active = frame.lfo_selected == fi + 1;
        match *sub_row {
            LfoSubRow::Field(field) => {
                let value_display = match field {
                    LfoField::Interval if flipped.contains(&unit_key(id, Some("lfo.interval"))) => {
                        flip_display(TimeBase::Beats, route.cycle_beats, bpm)
                    }
                    LfoField::Offset if flipped.contains(&unit_key(id, Some("lfo.offset"))) => {
                        flip_display(TimeBase::Beats, route.phase_offset_beats, bpm)
                    }
                    _ => None,
                }
                .unwrap_or_else(|| route.field_display(field));
                rows.push(field_line(
                    field.label(),
                    &Dial::new(route.field_value(field), field.scale(), value_display),
                    active,
                    &frame.numeric,
                    frame.bar_w,
                    LFO_PALETTE,
                ));
                // A macro stacked on this field but not currently expanded
                // shows as a closed chip, same as a regular control's macro
                // assignment.
                if let Some(key_str) = field.macro_key() {
                    let key = unit_key(id, Some(key_str));
                    if let Some(field_route) = lfo_state.field_macro(&key)
                        && !field_route.is_neutral()
                    {
                        rows.push(macro_chip_line(field_route));
                    }
                }
            }
            LfoSubRow::FieldMacro(field, macro_field) => {
                let key = unit_key(id, field.macro_key());
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
                    active,
                    &frame.numeric,
                    frame.bar_w,
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
                    active,
                    &frame.numeric,
                    frame.bar_w,
                    LFO_PALETTE,
                ));
            }
        }
    }
}

/// Where every modulation source puts this row's handle: one bright marker at
/// the value the engine plays, a dim ghost per contributing source, and — while
/// an editor is open here — the faint band of their full reach.
fn slider_markers(
    item: &ControlItem,
    address: ControlAddress,
    editor_here: bool,
    frame: &PanelFrame<'_, '_>,
) -> SliderMarkers {
    let automation = frame.automation_state();
    let controls = frame.controls();
    let mod_ctx = frame.mod_ctx;
    let spec = address.spec();
    // Markers all sit on the same tapered bar as the value itself.
    let base = item.value;
    let ratio_of = |value: f32| spec.ratio(value);
    let macro_route = automation.macro_route(address);
    let macro_mod = live_macro_contribution(automation, controls, address, mod_ctx);
    // The LFO route folded with any macro stacked onto its own fields
    // (amount/interval/offset), so markers show what the engine hears.
    let effective_lfo = automation
        .route(address)
        .map(|r| live_effective_lfo_route(automation, controls, address, r, mod_ctx));
    // Ghosts only for sources that actually contribute.
    let lfo = effective_lfo
        .as_ref()
        .filter(|r| r.depth_ratio > f32::EPSILON);
    let envelope = automation
        .envelope(address)
        .filter(|r| r.amount.abs() > f32::EPSILON);
    let single = |l: Option<&LfoRoute>, e: Option<&EnvelopeRoute>, m: Option<f32>| {
        ratio_of(modulated_control_value_full(spec, l, e, m, base, mod_ctx))
    };
    // While an editor is open on this control, faintly shade the full reach of
    // every active source (its full throw, not just the live instant) so
    // turning a depth/amount knob previews how far it can push the effective
    // value.
    let mod_range = spec.max - spec.min;
    let shadow = editor_here.then(|| {
        let mut lo = base;
        let mut hi = base;
        if let Some(r) = effective_lfo.as_ref() {
            let swing = mod_range * r.depth_ratio.clamp(0.0, 1.0);
            lo = lo.min(base - swing);
            hi = hi.max(base + swing);
        }
        if let Some(r) = automation.envelope(address) {
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
        effective: (lfo.is_some() || envelope.is_some() || macro_mod.is_some())
            .then(|| single(lfo, envelope, macro_mod)),
        lfo: lfo.map(|r| single(Some(r), None, None)),
        envelope: envelope.map(|r| single(None, Some(r), None)),
        macro_: macro_mod.map(|combined| single(None, None, Some(combined))),
        shadow,
    }
}

/// The one help/notice line, emphasized when it is carrying something the
/// user needs to act on.
fn draw_footer(f: &mut Frame, area: Rect, view: &UiViewModel<'_>) {
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
        area,
    );
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

/// The shared modulator lane: label-width prefix, then one glyph per column.
/// `column` supplies that column's level (0..1 glyph height), hue, and focus
/// — 1 at the live head, falling toward 0 away from it — so each lane only
/// describes its own trajectory and never its own brightness ramp or layout.
fn lane_line(
    width: usize,
    active: bool,
    saturation: f32,
    column: impl Fn(usize) -> (f32, f32, f32),
) -> Line<'static> {
    let floor = if active { 0.35 } else { 0.25 };
    let mut spans = Vec::with_capacity(width + 1);
    spans.push(lane_prefix());
    for i in 0..width {
        let (level, hue, focus) = column(i);
        let brightness = (floor + focus.max(0.0) * 0.6).clamp(0.0, 1.0);
        spans.push(Span::styled(
            lane_glyph(level),
            Style::default().fg(fluid_hsv(hue, saturation, brightness)),
        ));
    }
    Line::from(spans)
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
    if route.shape.is_random() {
        let window = f64::from(route.cycle_beats.max(MIN_LFO_CYCLE_BEATS) * RANDOM_LANE_CYCLES);
        return lane_line(width, active, 0.6, |i| {
            let age = (width - 1 - i) as f64 / width as f64;
            let wave = route.wave_at(beat - age * window) * route.depth_ratio;
            (
                wave * 0.5 + 0.5,
                300.0 + wave * 25.0,
                i as f32 / (width - 1) as f32,
            )
        });
    }

    let head = (route.pattern_phase_at(beat) * width as f64) as usize % width;
    lane_line(width, active, 0.6, |i| {
        let phase = i as f32 / width as f32;
        let wave = route.shape_value_at_phase(phase) * route.depth_ratio;
        // One cycle wraps, so the head's falloff wraps with it.
        let raw = i.abs_diff(head);
        let wrapped = raw.min(width - raw);
        (
            wave * 0.5 + 0.5,
            300.0 + wave * 25.0,
            1.0 - (wrapped as f32 / width as f32) * 2.0,
        )
    })
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
    let window = f64::from(route.window_beats());
    let head = ((route.lane_head_phase(ctx) * width as f32) as usize).min(width - 1);
    let hue = if route.amount >= 0.0 { 150.0 } else { 15.0 };
    lane_line(width, active, 0.55, |i| {
        let col_since = (i as f64 / width as f64 * window) as f32;
        (
            route.level_for_lane(col_since) * route.amount.abs(),
            hue,
            // The ramp does not wrap: it runs once from trigger to release.
            1.0 - (i.abs_diff(head) as f32 / width as f32) * 2.0,
        )
    })
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
