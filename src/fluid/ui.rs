//! Ratatui rendering, and nothing else.
//!
//! Draws one frame from an immutable `UiViewModel` and the frame area. It must
//! not mutate session state, execute effects, or poll input. Owns the control
//! and modulator row widgets, the shared lane renderer, and display-unit
//! formatting.

use std::collections::BTreeSet;

use super::interaction::LEAD_PLAY_KEYS;
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
        .border_style(Style::default().fg(BORDER));
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
            Constraint::Length(1), // 5 gesture activity row (blank when idle)
            Constraint::Length(1), // 6 footer: exits/mode help/notices
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
        lfo_selected: automation.map_or(0, AutomationSurface::selected),
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
    draw_activity(f, layout[5], view);
    draw_footer(f, layout[6], view);

    if let ModeSurface::Palette(palette) = &view.mode {
        draw_palette(
            f,
            panel,
            &palette.state,
            controls,
            frame.numeric.cursor_visible,
        );
    }

    if matches!(view.mode, ModeSurface::Help) {
        draw_help(f, inner);
    }
}

/// Frosted-glass scrim: darken the live fluid underneath instead of covering
/// it, so the visualizer still shows through the panel.
fn draw_scrim(f: &mut Frame, panel: Rect) {
    fill_scrim(f.buffer_mut(), panel, |cell| {
        let tint = darken(cell.fg, 0.30);
        cell.set_bg(tint);
        cell.set_fg(Color::Rgb(30, 34, 44));
    });
}

/// Blank every cell in `area` and let `paint` set its colours; the panel
/// scrim and the palette's opaque backdrop both fill this way.
fn fill_scrim(buf: &mut Buffer, area: Rect, paint: impl Fn(&mut ratatui::buffer::Cell)) {
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buf[(x, y)];
            cell.set_char(' ');
            paint(cell);
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
            } else if *t == Tab::Lead
                && matches!(
                    view.navigation.lead_drill,
                    interaction::LeadDrill::Pattern { .. }
                )
            {
                format!("{} › Pattern ♪", t.name())
            } else {
                t.name().to_string()
            };
            let name = if view.mute[*t as usize] {
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
/// modulation chip. A pending Jump renders here too, as the page it is
/// aiming at: the leader lives in the footer so the player watches the rows
/// they are about to land on.
fn draw_control_rows(f: &mut Frame, area: Rect, frame: &PanelFrame<'_, '_>) {
    let view = frame.view;
    let items = &view.items;
    let selected = view.navigation.selected;
    let automation = frame.automation_state();
    let flipped = view.flipped;
    let bpm = frame.bpm();
    let beat = view.telemetry.beat;

    let mut rows: Vec<Line<'static>> = Vec::with_capacity(items.len() * 3 + 2);
    if let ModeSurface::Lead(lead) = &view.mode {
        rows.push(lead_keyboard_line(*lead));
        rows.push(Line::from(""));
    }
    let mut selected_line = 0;
    for (i, item) in items.iter().enumerate() {
        let active = i == selected;
        if active {
            selected_line = rows.len();
        }
        let address = ControlAddress::new(item.id);
        let editor_here =
            frame.automation.and_then(AutomationSurface::active_address) == Some(address);
        let open_here = |kind: ModKind| match (frame.automation, kind) {
            (Some(AutomationSurface::Lfo { address: open, .. }), ModKind::Lfo)
            | (Some(AutomationSurface::Envelope { address: open, .. }), ModKind::Envelope) => {
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
        let style = BROWSE_PALETTE.style(parent_active);
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
                Style::default().fg(LIVE_AMBER).add_modifier(Modifier::BOLD),
            ));
        }
        // The Lead lane has the same transport-derived playhead treatment as
        // Chords: the badge marks what is playing, independently of the ▶
        // selection cursor.
        let lead_step_playing = view.navigation.tab == Tab::Lead
            && matches!(
                view.navigation.lead_drill,
                interaction::LeadDrill::Pattern { .. }
            )
            && i == lead_step_at(
                beat,
                frame.controls().lead.rate_beats,
                frame.controls().lead.offset_beats,
                lead_live_step_count(frame.controls().lead.step_count),
            );
        if lead_step_playing {
            spans.push(Span::styled(
                " ♪",
                Style::default().fg(LIVE_AMBER).add_modifier(Modifier::BOLD),
            ));
        }
        rows.push(Line::from(spans));

        let lfo_count = automation.routes_for(address).count();
        for (lane_index, route) in automation.routes_for(address).enumerate() {
            let lane_open = lfo_open_here && automation.active_lane_index() == Some(lane_index);
            if lane_open {
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
            let label = format!("LFO {}/{}", lane_index + 1, lfo_count);
            rows.push(lfo_lane_line_with_label(
                route,
                beat,
                frame.bar_w,
                lane_open,
                &label,
            ));
        }
        let env_open_here = open_here(ModKind::Envelope);
        let envelope_count = automation.envelopes_for(address).count();
        for (lane_index, route) in automation.envelopes_for(address).enumerate() {
            let lane_open = env_open_here && automation.active_lane_index() == Some(lane_index);
            if lane_open {
                for (fi, field) in EnvField::ALL.iter().enumerate() {
                    // A zero decay keeps its native display rather than a
                    // flipped 0 ms.
                    let value_display = field
                        .time_key()
                        .filter(|key| flipped.contains(&unit_key(item.id, Some(key))))
                        .filter(|_| *field != EnvField::Decay || route.decay_beats > 0.0)
                        .and_then(|_| flip_display(TimeBase::Beats, route.field_value(*field), bpm))
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
            let label = format!("ENV {}/{}", lane_index + 1, envelope_count);
            rows.push(env_lane_line_with_label(
                route,
                frame.mod_ctx,
                frame.bar_w,
                lane_open,
                &label,
            ));
        }
        if i + 1 < items.len() {
            rows.push(Line::from(""));
        }
    }
    let scroll = row_scroll(selected_line, rows.len(), area.height);
    f.render_widget(Paragraph::new(rows).scroll((scroll, 0)), area);
}

/// Lines to drop from the top so the selected row stays on screen. A row on
/// the first screen never scrolls (the top of the page, including the Lead
/// keyboard, stays put); past that, the selected row is shown with two lines
/// under it so its own lanes stay in view. The list never scrolls past its
/// own end.
fn row_scroll(selected_line: usize, line_count: usize, height: u16) -> u16 {
    let height = height.max(1) as usize;
    if selected_line < height {
        return 0;
    }
    let keep_below = 2;
    let wanted = selected_line + 1 + keep_below - height;
    let max_scroll = line_count.saturating_sub(height);
    wanted.min(max_scroll) as u16
}

/// The rows of an open LFO editor and its inline step editor.
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
                let value_display = field
                    .time_key()
                    .filter(|key| flipped.contains(&unit_key(id, Some(key))))
                    .and_then(|_| flip_display(TimeBase::Beats, route.field_value(field), bpm))
                    .unwrap_or_else(|| route.field_display(field));
                rows.push(field_line(
                    field.label(),
                    &Dial::new(route.field_value(field), field.scale(), value_display),
                    active,
                    &frame.numeric,
                    frame.bar_w,
                    LFO_PALETTE,
                ));
            }
            LfoSubRow::Step(target) => {
                let mut line = field_line(
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
                );
                if matches!(target, StepTarget::Value(step) if route.active_step_at(frame.view.telemetry.beat) == Some(step))
                {
                    line.spans.push(Span::styled(
                        " ♪",
                        Style::default().fg(LIVE_AMBER).add_modifier(Modifier::BOLD),
                    ));
                }
                rows.push(line);
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
    // Markers all sit on the same tapered bar as the value itself, so the
    // spec must be the contextual one — a loaded slot's family bounds, not
    // the registry's raw row.
    let spec = address.spec().contextual(controls);
    let base = item.value;
    let ratio_of = |value: f32| spec.ratio(value, controls);
    let lfos = automation.lfo_lanes(address);
    let envelopes = automation.envelope_lanes(address);
    let has_lfo = lfos.iter().any(|route| route.depth_ratio > f32::EPSILON);
    let has_envelope = envelopes
        .iter()
        .any(|route| route.amount.abs() > f32::EPSILON);
    let marker = |l: &[LfoRoute], e: &[EnvelopeRoute]| {
        ratio_of(modulated_control_value_full(&spec, l, e, base, mod_ctx))
    };
    // While an editor is open on this control, faintly shade the full reach of
    // every active source (its full throw, not just the live instant) so
    // turning a depth/amount knob previews how far it can push the effective
    // value.
    let mod_range = spec.max - spec.min;
    let shadow = editor_here.then(|| {
        let mut lo = base;
        let mut hi = base;
        let lfo_depth: f32 = lfos
            .iter()
            .map(|route| route.depth_ratio.clamp(0.0, 1.0))
            .sum();
        if lfo_depth > f32::EPSILON {
            let swing = mod_range * lfo_depth;
            lo = lo.min(base - swing);
            hi = hi.max(base + swing);
        }
        let envelope_min: f32 = envelopes
            .iter()
            .map(|route| route.amount.clamp(-1.0, 0.0))
            .sum();
        let envelope_max: f32 = envelopes
            .iter()
            .map(|route| route.amount.clamp(0.0, 1.0))
            .sum();
        lo = lo.min(base + mod_range * envelope_min);
        hi = hi.max(base + mod_range * envelope_max);
        (
            ratio_of(lo.clamp(spec.min, spec.max)),
            ratio_of(hi.clamp(spec.min, spec.max)),
        )
    });
    SliderMarkers {
        effective: (has_lfo || has_envelope).then(|| marker(lfos, envelopes)),
        lfo: has_lfo.then(|| marker(lfos, &[])),
        envelope: has_envelope.then(|| marker(&[], envelopes)),
        shadow,
    }
}

/// The gesture-activity row, above the footer proper. Shows the idle key
/// hints when nothing is held, and switches to a bold live readout while a
/// gesture is held or returning, so its presence never shifts the layout
/// above it either way.
fn draw_activity(f: &mut Frame, area: Rect, view: &UiViewModel<'_>) {
    let style = if view.activity_live {
        Style::default()
            .fg(EMPHASIS_YELLOW)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(DIM_TEXT)
    };
    f.render_widget(
        Paragraph::new(view.activity.as_str())
            .alignment(Alignment::Center)
            .style(style),
        area,
    );
}

/// The one help/notice line, emphasized when it is carrying something the
/// user needs to act on.
fn draw_footer(f: &mut Frame, area: Rect, view: &UiViewModel<'_>) {
    let footer_style = if view.help.emphasized() {
        Style::default()
            .fg(EMPHASIS_YELLOW)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(DIM_TEXT)
    };
    f.render_widget(
        Paragraph::new(view.help.text())
            .alignment(Alignment::Center)
            .style(footer_style),
        area,
    );
}

/// The play-mode keyboard: each letter over the tone it plays, the last one
/// pressed lit. Sits above the Lead's rows so the lane stays in view.
fn lead_keyboard_line(lead: LeadSurface) -> Line<'static> {
    let mut spans = vec![Span::styled("  ", BROWSE_PALETTE.style(false))];
    for (index, key) in LEAD_PLAY_KEYS.iter().enumerate() {
        let tone = index + 1;
        let label = lead_tone_label(tone, lead.reach_len);
        let lit = lead.last_tone == Some(tone);
        spans.push(Span::styled(
            format!("{key}{label} "),
            BROWSE_PALETTE.style(lit),
        ));
    }
    Line::from(spans)
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
    fill_scrim(f.buffer_mut(), area, |cell| {
        cell.set_bg(Color::Rgb(18, 22, 32));
    });
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(BORDER));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let cursor = if cursor_visible { "\u{258c}" } else { " " };
    let prompt = match pal.locked {
        Some(entry) => Line::from(vec![
            Span::styled("/", Style::default().fg(DIM_TEXT)),
            Span::styled(
                pal.entry(entry).id().unwrap_or("module"),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" = ", Style::default().fg(DIM_TEXT)),
            Span::styled(
                format!("{}{cursor}", pal.value_buf),
                Style::default().fg(Color::White),
            ),
        ]),
        None => Line::from(vec![
            Span::styled("/", Style::default().fg(DIM_TEXT)),
            Span::styled(
                format!("{}{cursor}", pal.query),
                Style::default().fg(Color::White),
            ),
        ]),
    };

    let mut lines = vec![prompt];
    for (row, m) in pal.matches.iter().skip(first_row).take(shown).enumerate() {
        let entry = pal.entry(m.entry_index);
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
            Style::default().fg(EMPHASIS_YELLOW),
        )));
    }
    lines.push(Line::from(Span::styled(
        "\u{21e5} complete   type value   \u{21b5} stage/jump   \u{21b5}\u{21b5} commit   ^B on bar   Esc cancel",
        Style::default().fg(DIM_TEXT),
    )));
    f.render_widget(Paragraph::new(lines), inner);
}

/// The full keyboard-shortcut map, opened with `?` from Browsing. Covers the
/// tab/control area but leaves the activity and footer rows showing beneath
/// it, same as the palette leaving its own exits visible.
/// One key-combo and what it does, rendered as a colour-matched pair so the
/// keys scan as a column even though rows hold a variable number of pairs.
type KeyRow<'a> = &'a [(&'a str, &'a str)];

fn draw_help(f: &mut Frame, inner: Rect) {
    let above_footer = inner.height.saturating_sub(2);
    let area = Rect::new(
        inner.x + 1,
        inner.y,
        inner.width.saturating_sub(2),
        above_footer,
    );
    fill_scrim(f.buffer_mut(), area, |cell| {
        cell.set_bg(Color::Rgb(16, 19, 28));
    });
    let block = Block::default()
        .title(Line::from(Span::styled(
            " Shortcuts ",
            Style::default()
                .fg(EMPHASIS_YELLOW)
                .add_modifier(Modifier::BOLD),
        )))
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(BORDER))
        .padding(Padding::new(2, 2, 1, 1));
    let inner_block = block.inner(area);
    f.render_widget(block, area);

    let key_style = Style::default()
        .fg(BROWSE_PALETTE.active)
        .add_modifier(Modifier::BOLD);
    let desc_style = Style::default().fg(Color::Rgb(205, 210, 222));
    let heading_style = Style::default()
        .fg(EMPHASIS_YELLOW)
        .add_modifier(Modifier::BOLD);
    let rule_style = Style::default().fg(Color::Rgb(60, 66, 84));
    let rule: String = "\u{2500}".repeat(inner_block.width as usize);

    // The leader's layer keys are spelled out here rather than restated,
    // so the map and `INSTRUMENTS` cannot drift apart.
    let jump_layer_names: Vec<(String, String)> = crate::fluid::interaction::INSTRUMENTS
        .iter()
        .map(|row| (row.key.to_string(), row.instrument.name().to_lowercase()))
        .collect();
    let jump_layers: Vec<(&str, &str)> = jump_layer_names
        .iter()
        .map(|(key, name)| (key.as_str(), name.as_str()))
        .collect();

    let key_row = |row: KeyRow| -> Line<'static> {
        let mut spans = Vec::new();
        for (i, (key, desc)) in row.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw("   "));
            }
            spans.push(Span::styled(key.to_string(), key_style));
            spans.push(Span::raw("  "));
            spans.push(Span::styled(desc.to_string(), desc_style));
        }
        Line::from(spans)
    };
    let section = |title: &'static str, rows: &[KeyRow]| -> Vec<Line<'static>> {
        let mut lines = vec![
            Line::from(Span::styled(title, heading_style)),
            Line::from(Span::styled(rule.clone(), rule_style)),
        ];
        lines.extend(rows.iter().map(|row| key_row(row)));
        lines.push(Line::from(""));
        lines
    };

    let mut lines = section(
        "General",
        &[
            &[
                ("jk / \u{2191}\u{2193}", "select"),
                ("hl / \u{2190}\u{2192}", "adjust"),
                ("Tab / \u{21e7}Tab", "page"),
            ],
            &[
                ("r", "random"),
                ("\u{21e7}R", "randomize set"),
                ("\u{21b5}", "open/confirm"),
                ("x", "remove"),
            ],
            &[("m", "mute"), ("\u{21e7}M", "master mute"), ("T", "units")],
        ],
    );
    lines.extend(section(
        "Gestures (hold)",
        &[&[
            ("z", "bloom"),
            ("c", "submerge"),
            ("v", "echo"),
            ("b", "thin"),
            ("x", "lift"),
        ]],
    ));
    lines.extend(section(
        "Editors",
        &[
            &[("/", "palette"), ("f", "LFO"), ("\u{21e7}F", "add LFO")],
            &[("a", "toggle auto"), ("e", "ENV"), ("\u{21e7}E", "add ENV")],
        ],
    ));
    lines.extend(section(
        "Lead (i to enter)",
        &[
            &[
                ("a s d f g h j k l", "tones"),
                ("c", "capture"),
                ("Space", "pattern on/off"),
            ],
            &[
                ("z/x", "oct"),
                ("q/w", "level"),
                ("e/r", "decay"),
                ("t/y", "glide"),
            ],
        ],
    ));
    lines.extend(section(
        "Jump (Space, then layer, then parameter)",
        &[
            &jump_layers[..4],
            &jump_layers[4..],
            &[("j", "volume"), ("k", "filter"), ("Space j/k", "this page")],
        ],
    ));
    lines.push(Line::from(Span::styled("System", heading_style)));
    lines.push(Line::from(Span::styled(rule, rule_style)));
    lines.push(key_row(&[
        ("^S", "save"),
        ("^Q", "quit"),
        ("Esc", "back/cancel"),
        ("?", "this screen"),
    ]));
    f.render_widget(Paragraph::new(lines), inner_block);
}

/// Colour pair for a row family: (active row, idle row).
#[derive(Clone, Copy)]
pub(crate) struct FieldPalette {
    active: Color,
    idle: Color,
}

impl FieldPalette {
    /// The row style: active colour in bold when the cursor is here, idle
    /// colour otherwise.
    fn style(self, active: bool) -> Style {
        let style = Style::default().fg(if active { self.active } else { self.idle });
        if active {
            style.add_modifier(Modifier::BOLD)
        } else {
            style
        }
    }
}

/// Browse rows and Sequence share one colour language: idle grey, focused
/// cyan.
const BROWSE_PALETTE: FieldPalette = FieldPalette {
    active: Color::Rgb(120, 230, 255),
    idle: Color::Rgb(170, 178, 195),
};

/// Amber for something sounding or physically held right now: the playing
/// chord badge and a held Sequence instrument.
const LIVE_AMBER: Color = Color::Rgb(255, 200, 90);
/// Help/notice text the user must act on, and staged palette edits.
const EMPHASIS_YELLOW: Color = Color::Rgb(255, 220, 120);
/// Quiet help text and prompt punctuation.
const DIM_TEXT: Color = Color::Rgb(120, 128, 145);
const BORDER: Color = Color::Rgb(150, 160, 185);

pub(crate) const LFO_PALETTE: FieldPalette = FieldPalette {
    active: Color::Rgb(255, 130, 210),
    idle: Color::Rgb(190, 105, 210),
};

pub(crate) const ENV_PALETTE: FieldPalette = FieldPalette {
    active: Color::Rgb(140, 235, 175),
    idle: Color::Rgb(95, 195, 140),
};

const ENV_LANE_HUE: f32 = 150.0;

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
    let style = palette.style(active);
    let prefix = if active { "▶ " } else { "  " };
    let display = numeric_cursor(numeric, active).unwrap_or_else(|| dial.display.clone());
    let mut spans = vec![Span::styled(format!("{prefix}  {label:<13} "), style)];
    spans.extend(slider_spans(
        dial.ratio(),
        SliderMarkers::default(),
        bar_w,
        style,
    ));
    spans.push(Span::styled(format!(" {display}"), style));
    Line::from(spans)
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
    LANE_WAVE[ladder_index(level, LANE_WAVE.len())]
}

/// Label-width prefix shared by every modulator lane line, so stacked lanes
/// identify themselves without moving their glyphs off the slider grid.
fn lane_prefix(label: &str) -> Span<'static> {
    Span::styled(
        format!("  {label:<15} "),
        Style::default().fg(Color::Rgb(130, 136, 160)),
    )
}

/// The shared modulator lane: label-width prefix, then one glyph per column.
/// `column` supplies that column's level (0..1 glyph height), hue, and focus
/// — 1 at the live head, falling toward 0 away from it — so each lane only
/// describes its own trajectory and never its own brightness ramp or layout.
fn lane_line(
    label: &str,
    width: usize,
    active: bool,
    saturation: f32,
    column: impl Fn(usize) -> (f32, f32, f32),
) -> Line<'static> {
    let floor = if active { 0.35 } else { 0.25 };
    let mut spans = Vec::with_capacity(width + 1);
    spans.push(lane_prefix(label));
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
#[cfg(test)]
pub(crate) fn lfo_lane_line(
    route: &LfoRoute,
    beat: f64,
    width: usize,
    active: bool,
) -> Line<'static> {
    lfo_lane_line_with_label(route, beat, width, active, "LFO")
}

fn lfo_lane_line_with_label(
    route: &LfoRoute,
    beat: f64,
    width: usize,
    active: bool,
    label: &str,
) -> Line<'static> {
    let width = width.clamp(6, 80);
    if route.shape.is_random() {
        let window = f64::from(route.cycle_beats.max(MIN_LFO_CYCLE_BEATS) * RANDOM_LANE_CYCLES);
        return lane_line(label, width, active, 0.6, |i| {
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
    lane_line(label, width, active, 0.6, |i| {
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
#[cfg(test)]
pub(crate) fn env_lane_line(
    route: &EnvelopeRoute,
    ctx: ModContext,
    width: usize,
    active: bool,
) -> Line<'static> {
    env_lane_line_with_label(route, ctx, width, active, "ENV")
}

fn env_lane_line_with_label(
    route: &EnvelopeRoute,
    ctx: ModContext,
    width: usize,
    active: bool,
    label: &str,
) -> Line<'static> {
    let width = width.clamp(6, 80);
    let window = f64::from(route.window_beats());
    let head = ((route.lane_head_phase(ctx) * width as f32) as usize).min(width - 1);
    lane_line(label, width, active, 0.55, |i| {
        let col_since = (i as f64 / width as f64 * window) as f32;
        (
            route.level_for_lane(col_since) * route.amount.abs(),
            ENV_LANE_HUE,
            // The ramp does not wrap: it runs once from trigger to release.
            1.0 - (i.abs_diff(head) as f32 / width as f32) * 2.0,
        )
    })
}

/// Live marker positions on a slider, all as 0..1 bar ratios. `effective` is
/// the summed value the engine plays; the per-source entries are base plus
/// that source alone, drawn as dim ghost diamonds so a diverging cursor is
/// explained at a glance (pink = LFO, green = envelope).
#[derive(Default, Clone, Copy)]
pub(crate) struct SliderMarkers {
    pub(crate) effective: Option<f32>,
    pub(crate) lfo: Option<f32>,
    pub(crate) envelope: Option<f32>,
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

#[cfg(test)]
mod row_scroll_tests {
    use super::row_scroll;

    #[test]
    fn selected_row_stays_on_screen_and_a_short_page_never_scrolls() {
        // Everything fits, or the row is on the first screen: no scroll.
        assert_eq!(row_scroll(0, 8, 10), 0);
        assert_eq!(row_scroll(7, 8, 10), 0);
        assert_eq!(row_scroll(9, 40, 10), 0);
        // Selecting past the fold scrolls just enough to show the row and
        // two lines under it.
        assert_eq!(row_scroll(10, 40, 10), 3);
        assert_eq!(row_scroll(20, 40, 10), 13);
        // Never past the end of the list.
        assert_eq!(row_scroll(39, 40, 10), 30);
        assert_eq!(row_scroll(39, 40, 0), 39);
    }
}
