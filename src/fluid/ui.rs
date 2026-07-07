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
    let mut unit = UnitMode::Native;
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
                &c,
                footer_message,
                unit,
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
                            set_modulator_or_control(
                                &mut automation,
                                lfo_selected,
                                &controls,
                                tab,
                                selected,
                                value,
                                beat,
                                unit,
                            );
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
                        if lfo_selected >= active_field_count(automation.state()) {
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
                    open_modulator(&mut automation, &items, selected, ModKind::Lfo, &mut lfo_selected);
                }
                KeyCode::Char('e') => {
                    open_modulator(
                        &mut automation,
                        &items,
                        selected,
                        ModKind::Envelope,
                        &mut lfo_selected,
                    );
                }
                KeyCode::Char('v') => {
                    open_modulator(
                        &mut automation,
                        &items,
                        selected,
                        ModKind::Macro,
                        &mut lfo_selected,
                    );
                }
                KeyCode::Char('t') | KeyCode::Char('T') => {
                    unit = unit.cycled();
                }
                KeyCode::Char('r') | KeyCode::Char('R') => {
                    if let Some(address) = automation.state().active_address()
                        && automation.state().active_kind() == Some(ModKind::Lfo)
                    {
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
/// Global unit mode cycled by T. Native shows each time field in its own
/// base; Ms/Beats convert every cross-base field's display and numeric entry
/// at the current BPM. Stepping always stays on the native grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UnitMode {
    Native,
    Ms,
    Beats,
}

impl UnitMode {
    pub(crate) fn cycled(self) -> Self {
        match self {
            Self::Native => Self::Ms,
            Self::Ms => Self::Beats,
            Self::Beats => Self::Native,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Ms => "ms",
            Self::Beats => "beats",
        }
    }
}

pub(crate) fn beats_to_ms(beats: f32, bpm: f32) -> f32 {
    beats * 60_000.0 / bpm.max(1.0)
}

pub(crate) fn ms_to_beats(ms: f32, bpm: f32) -> f32 {
    ms * bpm.max(1.0) / 60_000.0
}

fn fmt_ms(ms: f32) -> String {
    if ms >= 1000.0 {
        format!("{:.2} s", ms / 1000.0)
    } else {
        format!("{ms:.0} ms")
    }
}

fn fmt_beats(beats: f32) -> String {
    format!("{beats:.3} beats")
}

/// Converted display for a time field when the unit mode crosses its native
/// base; None keeps the native display.
fn unit_display(base: TimeBase, value: f32, bpm: f32, unit: UnitMode) -> Option<String> {
    match (base, unit) {
        (TimeBase::Beats, UnitMode::Ms) => Some(fmt_ms(beats_to_ms(value, bpm))),
        (TimeBase::Ms, UnitMode::Beats) => Some(fmt_beats(ms_to_beats(value, bpm))),
        _ => None,
    }
}

/// Numeric entry typed in the current unit mode, converted back to the
/// field's native base before snapping.
fn entry_to_native(base: TimeBase, value: f32, bpm: f32, unit: UnitMode) -> f32 {
    match (base, unit) {
        (TimeBase::Beats, UnitMode::Ms) => ms_to_beats(value, bpm),
        (TimeBase::Ms, UnitMode::Beats) => beats_to_ms(value, bpm),
        _ => value,
    }
}

pub(crate) fn lfo_field_at(index: usize) -> Option<LfoField> {
    LfoField::ALL.get(index.checked_sub(1)?).copied()
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
        Some(ModKind::Lfo) => LfoField::ALL.len(),
        Some(ModKind::Envelope) => EnvField::ALL.len(),
        Some(ModKind::Macro) => MacroField::ALL.len(),
        None => 0,
    }
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

/// Toggle a modulator editor of `kind` on the selected control. Pressing the
/// key again while its editor is open (double-tap) disables the modulator:
/// amount drops to zero and the neutral-route cleanup removes it entirely.
/// Otherwise any open editor is swapped for the requested one (created
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
            if already {
                match kind {
                    ModKind::Lfo => {
                        if let Some(route) = state.route_mut(address) {
                            route.depth_ratio = 0.0;
                        }
                    }
                    ModKind::Envelope => {
                        if let Some(route) = state.envelope_mut(address) {
                            route.amount = 0.0;
                        }
                    }
                    ModKind::Macro => {
                        if let Some(route) = state.macro_route_mut(address) {
                            route.amount = 0.0;
                        }
                    }
                }
            }
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
    Envelope(ControlAddress, EnvField),
    Macro(ControlAddress, MacroField),
    Control,
}

fn active_field(automation: &AutomationState, lfo_selected: usize) -> ActiveField {
    let Some(address) = automation.active_address() else {
        return ActiveField::Control;
    };
    match automation.active_kind() {
        Some(ModKind::Lfo) => match lfo_field_at(lfo_selected) {
            Some(field) => ActiveField::Lfo(address, field),
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

fn adjust_lfo_or_control(
    automation: &mut PublishedAutomation,
    lfo_selected: usize,
    controls: &Arc<ArcSwap<FluidControls>>,
    tab: Tab,
    selected: usize,
    dir: f32,
    beat: f64,
) {
    match active_field(automation.state(), lfo_selected) {
        ActiveField::Lfo(address, field) => automation.edit(|state| {
            if let Some(route) = state.route_mut(address) {
                route.adjust_field_at(field, dir, beat);
            }
        }),
        ActiveField::Envelope(address, field) => automation.edit(|state| {
            if let Some(route) = state.envelope_mut(address) {
                route.adjust_field(field, dir);
            }
        }),
        ActiveField::Macro(address, field) => automation.edit(|state| {
            if let Some(route) = state.macro_route_mut(address) {
                route.adjust_field(field, dir);
            }
        }),
        ActiveField::Control => adjust(controls, tab, selected, dir),
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
    unit: UnitMode,
) {
    let bpm = controls.load().master.bpm;
    match active_field(automation.state(), lfo_selected) {
        ActiveField::Lfo(address, field) => automation.edit(|state| {
            let value = match field {
                LfoField::Interval | LfoField::Offset => {
                    entry_to_native(TimeBase::Beats, value, bpm, unit)
                }
                LfoField::Amount | LfoField::Shape => value,
            };
            if let Some(route) = state.route_mut(address) {
                route.set_field_at(field, value, beat);
            }
        }),
        ActiveField::Envelope(address, field) => automation.edit(|state| {
            let value = match field {
                EnvField::Attack | EnvField::Decay => {
                    entry_to_native(TimeBase::Beats, value, bpm, unit)
                }
                EnvField::Amount | EnvField::Trigger => value,
            };
            if let Some(route) = state.envelope_mut(address) {
                route.set_field(field, value);
            }
        }),
        ActiveField::Macro(address, field) => automation.edit(|state| {
            if let Some(route) = state.macro_route_mut(address) {
                route.set_field(field, value);
            }
        }),
        ActiveField::Control => {
            let value = match tab_specs(tab).get(selected) {
                Some(spec) => entry_to_native(spec.time_base, value, bpm, unit),
                None => value,
            };
            set_value(controls, tab, selected, value);
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
                "LFO {}   {}   {:.2} beats   depth {:.0}%{reseed}   Esc close",
                address.id(),
                route.shape.label(),
                route.cycle_beats,
                route.depth_ratio * 100.0
            ))
        }
        ModKind::Envelope => {
            let route = automation.envelope(address)?;
            Some(format!(
                "ENV {}   {}   amount {:+.0}%   Esc close",
                address.id(),
                route.field_display(EnvField::Trigger),
                route.amount * 100.0
            ))
        }
        ModKind::Macro => {
            let route = automation.macro_route(address)?;
            Some(format!(
                "MACRO {}   {}   amount {:+.0}%   Esc close",
                address.id(),
                route.field_display(MacroField::Target),
                route.amount * 100.0
            ))
        }
    }
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
    controls: &FluidControls,
    update_message: Option<&str>,
    unit: UnitMode,
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
        controls,
        update_message,
        unit,
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
    controls: &FluidControls,
    update_message: Option<&str>,
    unit: UnitMode,
) {
    let bpm = controls.master.bpm;
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
        let display = if numeric.entry.is_some() && parent_active {
            display
        } else {
            unit_display(address.spec().time_base, item.value, bpm, unit).unwrap_or(display)
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
        let modulated = (route.is_some() || envelope.is_some() || macro_mod.is_some()).then(|| {
            let spec = address.spec();
            let base = match spec.bar {
                Bar::Linear => item.value,
                Bar::Log2 => 2f32.powf(item.value),
            };
            let value = modulated_control_value_full(spec, route, envelope, macro_mod, base, mod_ctx);
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
            if lfo_open_here {
                for (fi, field) in LfoField::ALL.iter().enumerate() {
                    let value_display = match field {
                        LfoField::Interval => {
                            unit_display(TimeBase::Beats, route.cycle_beats, bpm, unit)
                        }
                        LfoField::Offset => {
                            unit_display(TimeBase::Beats, route.phase_offset_beats, bpm, unit)
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
                        LFO_PALETTE,
                    ));
                }
            }
            rows.push(lfo_lane_line(route, beat, bar_w, lfo_open_here));
        }
        if let Some(route) = envelope {
            if env_open_here {
                for (fi, field) in EnvField::ALL.iter().enumerate() {
                    let value_display = match field {
                        EnvField::Attack => {
                            unit_display(TimeBase::Beats, route.attack_beats, bpm, unit)
                        }
                        EnvField::Decay if route.decay_beats > 0.0 => {
                            unit_display(TimeBase::Beats, route.decay_beats, bpm, unit)
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
                        field.label(),
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

    let footer_line;
    let footer = match update_message {
        Some(message) => message,
        None => {
            footer_line = format!(
                "jk select   h/l adjust   f LFO   v macro   T units({})   Enter set   q quit",
                unit.label()
            );
            footer_line.as_str()
        }
    };
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
        format!(
            "    {:<15} ⇒ {}   {}",
            "",
            route.field_display(MacroField::Target),
            route.field_display(MacroField::Amount)
        ),
        Style::default().fg(MACRO_PALETTE.idle),
    ))
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
    let display = if active && let Some(entry) = numeric.entry {
        let cursor = if numeric.cursor_visible { "_" } else { " " };
        format!("> {entry}{cursor}")
    } else {
        value_display
    };
    let bar = ratio_bar(ratio, bar_w, '█', '░');
    Line::from(Span::styled(
        format!("{prefix}  {label:<13} {bar} {display}"),
        style,
    ))
}

const LANE_WAVE: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// How many random cycles the lane scopes so sample & hold / random drift read
/// as an actual scrolling trajectory rather than a single flat step.
const RANDOM_LANE_CYCLES: f32 = 4.0;

fn lane_glyph(level: f32) -> char {
    let level = level.clamp(0.0, 1.0);
    LANE_WAVE[((level * (LANE_WAVE.len() - 1) as f32).round() as usize).min(LANE_WAVE.len() - 1)]
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
    spans.push(Span::styled(
        format!("  {:<15} ", ""),
        Style::default().fg(Color::Rgb(130, 136, 160)),
    ));

    if route.shape.is_random() {
        let window = f64::from(route.cycle_beats.max(MIN_LFO_CYCLE_BEATS) * RANDOM_LANE_CYCLES);
        for i in 0..width {
            let age = (width - 1 - i) as f64 / width as f64;
            let wave = route.wave_at(beat - age * window) * route.depth_ratio;
            let level = wave * 0.5 + 0.5;
            let brightness = (floor + (i as f32 / (width - 1) as f32) * 0.6).clamp(0.0, 1.0);
            let hue = 300.0 + wave * 25.0;
            spans.push(Span::styled(
                lane_glyph(level).to_string(),
                Style::default().fg(fluid_hsv(hue, 0.6, brightness)),
            ));
        }
        return Line::from(spans);
    }

    let head = (route.phase_at(beat) * width as f64) as usize % width;
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
            lane_glyph(level).to_string(),
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
    spans.push(Span::styled(
        format!("  {:<15} ", ""),
        Style::default().fg(Color::Rgb(130, 136, 160)),
    ));
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
