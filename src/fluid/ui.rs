use super::*;

const SAVE_MESSAGE_TTL: std::time::Duration = std::time::Duration::from_secs(3);

pub(crate) fn ui_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    controls: Arc<ArcSwap<FluidControls>>,
    automation_shared: Arc<ArcSwap<AutomationState>>,
    telemetry: Arc<FluidTelemetry>,
    initial_automation: AutomationState,
    updates: UpdateNotice,
) -> Result<(), Box<dyn Error>> {
    let mut tab = Tab::Master;
    let mut selected = 0usize;
    let mut lfo_selected = 0usize;
    let mut numeric_entry: Option<NumericEntry> = None;
    let mut fluid = FluidState::new();
    let mut last = Instant::now();
    let started = Instant::now();
    let mut save_message: Option<(String, Instant)> = None;
    let mut automation = PublishedAutomation::new(initial_automation, automation_shared);

    loop {
        let c = FluidControls::clone(&controls.load());
        if save_message
            .as_ref()
            .is_some_and(|(_, shown_at)| shown_at.elapsed() >= SAVE_MESSAGE_TTL)
        {
            save_message = None;
        }
        let update_message = updates.message();
        let automation_message = automation_footer(automation.state());
        let footer_message = save_message
            .as_ref()
            .map(|(message, _)| message.as_str())
            .or(automation_message.as_deref())
            .or(update_message.as_deref());
        let items = tab_controls(tab, &c);
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
                footer_message,
            )
        })?;

        if event::poll(std::time::Duration::from_millis(16))?
            && let Event::Key(key) = event::read()?
        {
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
                            if let Some(field) = lfo_field_at(lfo_selected)
                                && let Some(address) = automation.state().active_address()
                            {
                                automation.edit(|state| {
                                    if let Some(route) = state.route_mut(address) {
                                        route.set_field_at(field, value, beat);
                                    }
                                });
                            } else {
                                set_value(&controls, tab, selected, value);
                            }
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
                    save_message = Some(match copy_launch_line(&controls, automation.state()) {
                        Ok(()) => ("nooise copied to clipboard".to_string(), Instant::now()),
                        Err(err) => (format!("Save failed: {err}"), Instant::now()),
                    });
                }
                KeyCode::Esc if automation.state().is_editor_open() => {
                    automation.edit(AutomationState::close_editor);
                    lfo_selected = 0;
                }
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Tab => {
                    if automation.state().is_editor_open() {
                        automation.edit(AutomationState::close_editor);
                    }
                    tab = tab.next();
                    selected = 0;
                    lfo_selected = 0;
                }
                KeyCode::BackTab => {
                    if automation.state().is_editor_open() {
                        automation.edit(AutomationState::close_editor);
                    }
                    tab = tab.previous();
                    selected = 0;
                    lfo_selected = 0;
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    if automation.state().is_editor_open() {
                        if lfo_selected <= 1 {
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
                        if lfo_selected >= LfoField::ALL.len() {
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
                KeyCode::Left if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    reset_lfo_or_control(
                        &mut automation,
                        lfo_selected,
                        &controls,
                        tab,
                        selected,
                        beat,
                    );
                }
                KeyCode::Char('H') => {
                    reset_lfo_or_control(
                        &mut automation,
                        lfo_selected,
                        &controls,
                        tab,
                        selected,
                        beat,
                    );
                }
                KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    reset_lfo_or_control(
                        &mut automation,
                        lfo_selected,
                        &controls,
                        tab,
                        selected,
                        beat,
                    );
                }
                KeyCode::Left | KeyCode::Char('h') => {
                    adjust_lfo_or_control(
                        &mut automation,
                        lfo_selected,
                        &controls,
                        tab,
                        selected,
                        -1.0,
                        beat,
                    );
                }
                KeyCode::Right | KeyCode::Char('l') => {
                    adjust_lfo_or_control(
                        &mut automation,
                        lfo_selected,
                        &controls,
                        tab,
                        selected,
                        1.0,
                        beat,
                    );
                }
                KeyCode::Char('f') => {
                    if let Some(item) = items.get(selected) {
                        let address = ControlAddress::new(item.id);
                        automation.edit(|state| {
                            if state.active_address() == Some(address) {
                                state.close_editor();
                            } else {
                                state.close_editor();
                                state.open_or_create(address);
                            }
                        });
                        lfo_selected = 1;
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

/// Submenu row 0 is the parent slider; rows 1..=3 map onto the LFO fields.
pub(crate) fn lfo_field_at(index: usize) -> Option<LfoField> {
    LfoField::ALL.get(index.checked_sub(1)?).copied()
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

fn adjust_lfo_or_control(
    automation: &mut PublishedAutomation,
    lfo_selected: usize,
    controls: &Arc<ArcSwap<FluidControls>>,
    tab: Tab,
    selected: usize,
    dir: f32,
    beat: f64,
) {
    if let Some(field) = lfo_field_at(lfo_selected)
        && let Some(address) = automation.state().active_address()
    {
        automation.edit(|state| {
            if let Some(route) = state.route_mut(address) {
                route.adjust_field_at(field, dir, beat);
            }
        });
    } else {
        adjust(controls, tab, selected, dir);
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
    if let Some(field) = lfo_field_at(lfo_selected)
        && let Some(address) = automation.state().active_address()
    {
        automation.edit(|state| {
            if let Some(route) = state.route_mut(address) {
                route.reset_field_at(field, beat);
            }
        });
    } else {
        reset_to_min(controls, tab, selected);
    }
}

fn automation_footer(automation: &AutomationState) -> Option<String> {
    let address = automation.active_address()?;
    let route = automation.route(address)?;
    Some(format!(
        "LFO {}   {:.2} beats   depth {:.0}%   Esc close",
        address.id(),
        route.cycle_beats,
        route.depth_ratio * 100.0
    ))
}

fn copy_launch_line(
    controls: &Arc<ArcSwap<FluidControls>>,
    automation: &AutomationState,
) -> Result<(), Box<dyn Error>> {
    let c = FluidControls::clone(&controls.load());
    let line = launch_line(&SongState {
        controls: c,
        automation: automation.clone(),
    })?;
    let mut clipboard = arboard::Clipboard::new()?;
    clipboard.set_text(line)?;
    Ok(())
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
    update_message: Option<&str>,
) {
    render_fluid(
        f,
        items,
        active_tab,
        selected,
        lfo_selected,
        beat,
        numeric,
        fluid,
        automation,
        update_message,
    );
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
pub(crate) fn render_fluid(
    f: &mut Frame,
    items: &[ControlItem],
    active_tab: Tab,
    selected: usize,
    lfo_selected: usize,
    beat: f64,
    numeric: NumericDisplay<'_>,
    fluid: &FluidState,
    automation: &AutomationState,
    update_message: Option<&str>,
) {
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
        .title(format!(" {APP_ID} "))
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
            if *t == active_tab {
                format!("[{}]", t.name())
            } else {
                t.name().to_string()
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
        let editor_open_here = automation.active_address() == Some(address);
        let parent_active = active && (!editor_open_here || lfo_selected == 0);
        let prefix = if parent_active { "▶ " } else { "  " };
        let display = if parent_active {
            if let Some(entry) = numeric.entry {
                let cursor = if numeric.cursor_visible { "_" } else { " " };
                format!("> {entry}{cursor}")
            } else {
                item.display.clone()
            }
        } else {
            item.display.clone()
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
        let modulated = route.map(|route| {
            let spec = address.spec();
            let base = match spec.bar {
                Bar::Linear => item.value,
                Bar::Log2 => 2f32.powf(item.value),
            };
            let value = modulated_control_value(spec, route, base, beat);
            let value = match spec.bar {
                Bar::Linear => value,
                Bar::Log2 => value.log2(),
            };
            let range = item.max - item.min;
            if range.abs() <= f32::EPSILON {
                0.0
            } else {
                ((value - item.min) / range).clamp(0.0, 1.0)
            }
        });
        let mut spans = vec![Span::styled(format!("{prefix}{:<15} ", item.label), style)];
        spans.extend(slider_spans(item_ratio(item), modulated, bar_w, style));
        spans.push(Span::styled(format!(" {display}"), style));
        rows.push(Line::from(spans));

        if let Some(route) = route {
            if editor_open_here {
                for (fi, field) in LfoField::ALL.iter().enumerate() {
                    rows.push(lfo_field_line(
                        route,
                        *field,
                        lfo_selected == fi + 1,
                        &numeric,
                        bar_w,
                    ));
                }
            }
            rows.push(lfo_lane_line(route, beat, bar_w, editor_open_here));
        }
        if i + 1 < items.len() {
            rows.push(Line::from(""));
        }
    }
    f.render_widget(Paragraph::new(rows), layout[4]);

    let footer = update_message
        .unwrap_or("jk select   h/l adjust   f LFO   type value   Enter set   q quit");
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

fn lfo_field_line(
    route: &LfoRoute,
    field: LfoField,
    active: bool,
    numeric: &NumericDisplay<'_>,
    bar_w: usize,
) -> Line<'static> {
    let fg = if active {
        Color::Rgb(255, 130, 210)
    } else {
        Color::Rgb(190, 105, 210)
    };
    let mut style = Style::default().fg(fg);
    if active {
        style = style.add_modifier(Modifier::BOLD);
    }
    let prefix = if active { "▶ " } else { "  " };
    let display = if active && let Some(entry) = numeric.entry {
        let cursor = if numeric.cursor_visible { "_" } else { " " };
        format!("> {entry}{cursor}")
    } else {
        route.field_display(field)
    };
    let bar = ratio_bar(route.field_ratio(field), bar_w, '█', '░');
    Line::from(Span::styled(
        format!("{prefix}  {:<13} {bar} {display}", field.label()),
        style,
    ))
}

/// Live oscillator lane: one LFO cycle across the width, phase-locked to the
/// engine beat. Amplitude tracks the route depth; brightness peaks at the
/// current phase head so the sweep reads as motion.
pub(crate) fn lfo_lane_line(
    route: &LfoRoute,
    beat: f64,
    width: usize,
    active: bool,
) -> Line<'static> {
    const WAVE: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let width = width.clamp(6, 80);
    let head = (route.phase_at(beat) * width as f64) as usize % width;
    let floor = if active { 0.35 } else { 0.25 };
    let mut spans = Vec::with_capacity(width + 1);
    spans.push(Span::styled(
        format!("  {:<15} ", ""),
        Style::default().fg(Color::Rgb(130, 136, 160)),
    ));
    for i in 0..width {
        let phase = i as f32 / width as f32;
        let wave = (TAU * phase).sin() * route.depth_ratio;
        let level = (wave * 0.5 + 0.5).clamp(0.0, 1.0);
        let glyph = WAVE[((level * (WAVE.len() - 1) as f32).round() as usize).min(WAVE.len() - 1)];
        let raw = i.abs_diff(head);
        let wrapped = raw.min(width - raw);
        let falloff = 1.0 - (wrapped as f32 / width as f32) * 2.0;
        let brightness = (floor + falloff.max(0.0) * 0.6).clamp(0.0, 1.0);
        let hue = 300.0 + wave * 25.0;
        spans.push(Span::styled(
            glyph.to_string(),
            Style::default().fg(fluid_hsv(hue, 0.6, brightness)),
        ));
    }
    Line::from(spans)
}

/// Slider bar spans with an optional bright marker at the live modulated value.
fn slider_spans(
    ratio: f32,
    modulated: Option<f32>,
    width: usize,
    style: Style,
) -> Vec<Span<'static>> {
    let filled = (ratio.clamp(0.0, 1.0) * width as f32).round() as usize;
    let marker = modulated
        .map(|value| (value.clamp(0.0, 1.0) * width.saturating_sub(1) as f32).round() as usize);
    (0..width)
        .map(|i| {
            if Some(i) == marker {
                Span::styled(
                    "◆".to_string(),
                    Style::default()
                        .fg(Color::Rgb(255, 130, 210))
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled(if i < filled { "█" } else { "░" }.to_string(), style)
            }
        })
        .collect()
}

pub(crate) fn item_ratio(item: &ControlItem) -> f32 {
    let range = item.max - item.min;
    if range.abs() <= f32::EPSILON {
        0.0
    } else {
        let value = match item.kind {
            ControlKind::Discrete => item.value.round(),
            ControlKind::Gain | ControlKind::Continuous | ControlKind::Timing => item.value,
        };
        ((value - item.min) / range).clamp(0.0, 1.0)
    }
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
