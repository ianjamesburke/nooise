use super::*;

pub(crate) fn ui_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    controls: Arc<ArcSwap<FluidControls>>,
    automation_shared: Arc<ArcSwap<AutomationState>>,
    telemetry: Arc<FluidTelemetry>,
    updates: UpdateNotice,
) -> Result<(), Box<dyn Error>> {
    let mut tab = Tab::Master;
    let mut selected = 0usize;
    let mut numeric_entry: Option<NumericEntry> = None;
    let mut fluid = FluidState::new();
    let mut last = Instant::now();
    let started = Instant::now();
    let mut save_message: Option<String> = None;
    let mut automation = AutomationState::default();

    loop {
        let c = FluidControls::clone(&controls.load());
        let update_message = updates.message();
        let automation_message = automation_footer(&automation);
        let footer_message = save_message
            .as_deref()
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
        terminal.draw(|f| {
            render(
                f,
                &items,
                tab,
                selected,
                NumericDisplay {
                    entry: numeric_entry.as_ref().map(|entry| entry.buffer.as_str()),
                    cursor_visible,
                },
                &fluid,
                &automation,
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
                            set_value(&controls, tab, selected, value);
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
                    save_message = Some(match copy_launch_line(&controls) {
                        Ok(line) => format!("Copied {line}"),
                        Err(err) => format!("Save failed: {err}"),
                    });
                }
                KeyCode::Esc if automation.is_editor_open() => {
                    automation.close_editor();
                    publish_automation(&automation_shared, &automation);
                }
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Tab => {
                    tab = tab.next();
                    selected = 0;
                }
                KeyCode::BackTab => {
                    tab = tab.previous();
                    selected = 0;
                }
                KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => {
                    selected = selected.saturating_add(1).min(items_len.saturating_sub(1))
                }
                KeyCode::Left if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    reset_to_min(&controls, tab, selected)
                }
                KeyCode::Char('H') => reset_to_min(&controls, tab, selected),
                KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    reset_to_min(&controls, tab, selected)
                }
                KeyCode::Left | KeyCode::Char('h') => adjust(&controls, tab, selected, -1.0),
                KeyCode::Right => adjust(&controls, tab, selected, 1.0),
                KeyCode::Char('l') => {
                    if let Some(item) = items.get(selected) {
                        automation.open_or_create(ControlAddress::new(item.id));
                        publish_automation(&automation_shared, &automation);
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

fn publish_automation(
    automation_shared: &Arc<ArcSwap<AutomationState>>,
    automation: &AutomationState,
) {
    automation_shared.store(Arc::new(automation.clone()));
}

fn automation_footer(automation: &AutomationState) -> Option<String> {
    let address = automation.active_address()?;
    let route = automation.route(address)?;
    Some(format!(
        "LFO {}   {:.2} beats   target +/-{:.0}%   effective {:.0}%   Esc close",
        address.id(),
        route.cycle_beats,
        route.target_depth_ratio * 100.0,
        route.effective_depth_ratio * 100.0
    ))
}

fn copy_launch_line(controls: &Arc<ArcSwap<FluidControls>>) -> Result<String, Box<dyn Error>> {
    let c = FluidControls::clone(&controls.load());
    let line = launch_line(&c)?;
    let mut clipboard = arboard::Clipboard::new()?;
    clipboard.set_text(line.clone())?;
    Ok(line)
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

pub(crate) fn render(
    f: &mut Frame,
    items: &[ControlItem],
    active_tab: Tab,
    selected: usize,
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

pub(crate) fn render_fluid(
    f: &mut Frame,
    items: &[ControlItem],
    active_tab: Tab,
    selected: usize,
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
        let bar = ratio_bar(item_ratio(item), bar_w, '█', '░');
        let prefix = if active { "▶ " } else { "  " };
        let display = if active {
            if let Some(entry) = numeric.entry {
                let cursor = if numeric.cursor_visible { "_" } else { " " };
                format!("> {entry}{cursor}")
            } else {
                item.display.clone()
            }
        } else {
            item.display.clone()
        };
        let fg = if active {
            Color::Rgb(120, 230, 255)
        } else {
            Color::Rgb(170, 178, 195)
        };
        let mut style = Style::default().fg(fg);
        if active {
            style = style.add_modifier(Modifier::BOLD);
        }
        rows.push(Line::from(Span::styled(
            format!("{prefix}{:<15} {bar} {display}", item.label),
            style,
        )));
        if let Some(route) = automation.route(address) {
            rows.push(automation_line(
                route,
                automation.active_address() == Some(address),
                bar_w,
            ));
        }
        if i + 1 < items.len() {
            rows.push(Line::from(""));
        }
    }
    f.render_widget(Paragraph::new(rows), layout[4]);

    let footer = update_message
        .unwrap_or("jk select   h/Left down   Right up   l LFO   type value   Enter set   q quit");
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

fn automation_line(route: &LfoRoute, active: bool, bar_w: usize) -> Line<'static> {
    let fg = if active {
        Color::Rgb(255, 130, 210)
    } else {
        Color::Rgb(190, 105, 210)
    };
    let style = Style::default().fg(fg);
    let label_style = Style::default().fg(Color::Rgb(130, 136, 160));
    Line::from(vec![
        Span::styled(format!("  {:<15} ", ""), label_style),
        Span::styled(oscillator_lane(bar_w.clamp(6, 24)), style),
        Span::styled(
            format!(
                " LFO {:.2}b +/-{:.0}% eff {:.0}%",
                route.cycle_beats,
                route.target_depth_ratio * 100.0,
                route.effective_depth_ratio * 100.0
            ),
            style,
        ),
    ])
}

fn oscillator_lane(width: usize) -> String {
    const WAVE: [char; 8] = ['▁', '▂', '▄', '▆', '█', '▆', '▄', '▂'];
    (0..width).map(|i| WAVE[i % WAVE.len()]).collect()
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
