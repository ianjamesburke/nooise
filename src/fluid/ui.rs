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
    // "Just watch it" mode: hide the control panel so the full field shows.
    // Any key returns to the controls.
    let mut visuals_focus = false;

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
                visuals_focus,
            )
        })?;

        if event::poll(std::time::Duration::from_millis(16))?
            && let Event::Key(key) = event::read()?
        {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            // In focus mode any key returns to the controls (and is consumed).
            if visuals_focus {
                visuals_focus = false;
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
                KeyCode::Char('V') => visuals_focus = true,
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
    visuals_focus: bool,
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
        visuals_focus,
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
// Fluid visualizer, two layers. LAYER 1 (fluid field): pad is the
// ambient medium, bass radiates rings, kick injects wavefronts from
// the bottom edge — all superpose, so the low end visibly interacts.
// LAYER 2 (surface): tonal notes and perc/clap glints are discrete
// sparks composited over the field so they stay crisp. Every element
// is gated by its voice's live telemetry level: silence is a still,
// black screen.
// ============================================================

pub(crate) const FLUID_GRADIENT: &[char] = &[' ', '·', '∙', '•', '●', '◉', '⬤'];

/// One chord = one hue. Cycles with the pad engine's chord table; drives the
/// pad node's hue and the ambient wash behind the whole field.
pub(crate) fn hue_for_chord(index: u64) -> f32 {
    const HUES: [f32; 5] = [205.0, 270.0, 325.0, 158.0, 38.0];
    HUES[(index % HUES.len() as u64) as usize]
}

/// Bass note pitch -> spatial frequency (rings per unit distance). A low note
/// gives a long, slow wavelength (few rings); a high note packs more rings in.
/// Monotonically increasing in `hz`.
pub(crate) fn bass_spatial_freq(hz: f32) -> f32 {
    let hz = hz.clamp(20.0, 500.0);
    let octaves = (hz / 55.0).log2(); // 0 at A1 (55 Hz)
    (5.0 + octaves * 2.2).clamp(3.0, 16.0)
}

/// Tonal note pitch -> vertical field position. High notes sit higher on the
/// screen (smaller y), low notes lower. Monotonically decreasing in `hz`.
pub(crate) fn tonal_node_y(hz: f32) -> f32 {
    let hz = hz.clamp(55.0, 2000.0);
    let lo = 110.0_f32.log2();
    let hi = 784.0_f32.log2();
    let t = ((hz.log2() - lo) / (hi - lo)).clamp(0.0, 1.0);
    0.62 - t * 0.44
}

/// Map a raw voice level to a 0..1 visual amplitude. Silence -> 0 (node goes
/// dark); a mild power curve makes quiet voices still legible.
fn visual_amp(level: f32, gain: f32) -> f32 {
    (level * gain).clamp(0.0, 1.0).powf(0.7)
}

/// One-pole smoothing factor for a time constant `tau` seconds over step `dt`.
fn smooth(dt: f32, tau: f32) -> f32 {
    1.0 - (-dt / tau).exp()
}

/// Move a hue toward `target` along the shortest arc on the colour wheel.
fn lerp_hue(cur: f32, target: f32, a: f32) -> f32 {
    let delta = (target - cur + 540.0).rem_euclid(360.0) - 180.0;
    (cur + delta * a).rem_euclid(360.0)
}

/// A persistent voice body: a home position that radiates concentric waves
/// whose amplitude tracks the voice's live level.
struct Node {
    x: f32,
    y: f32,
    reach: f32,
    amp: f32,
    spatial_freq: f32,
    phase: f32,
    hue: f32,
}

impl Node {
    fn new(x: f32, y: f32, reach: f32, spatial_freq: f32, hue: f32) -> Self {
        Self {
            x,
            y,
            reach,
            amp: 0.0,
            spatial_freq,
            phase: 0.0,
            hue,
        }
    }
}

/// A coherent kick wavefront: a ring expanding radially from a point near
/// the bottom edge, pushed hardest straight up. Lives in the shared field so
/// it visibly ripples through the bass rings and pad wash.
struct KickWave {
    x: f32,
    age: f32,
    life: f32,
    amp: f32,
}

/// Which voice a surface spark belongs to, so its brightness can keep
/// tracking that voice's live envelope after it spawns.
#[derive(Clone, Copy)]
enum SparkVoice {
    Tonal,
    Perc,
    Clap,
}

/// A discrete surface element (tonal note, perc/clap glint) composited over
/// the field so it stays crisp when the field is busy. `amp` is clamped to
/// the owning voice's live level every frame, so the spark decays exactly
/// with the sound: a long tonal decay glows on, a choked perc winks out.
struct Spark {
    voice: SparkVoice,
    x: f32,
    y: f32,
    age: f32,
    life: f32,
    size: f32,
    amp: f32,
    hue: f32,
}

/// Sampled field value plus the dominant hue at a point.
pub(crate) struct FieldSample {
    pub value: f32,
    pub hue: f32,
}

pub(crate) struct FluidState {
    t: f32,
    bass: Node,
    pad: Node,
    kicks: Vec<KickWave>,
    sparks: Vec<Spark>,
    ambient_hue: f32,
    last_kick: u64,
    last_bass: u64,
    last_tonal: u64,
    last_perc: u64,
    last_clap: u64,
    scatter: f32,
}

impl FluidState {
    pub(crate) fn new() -> Self {
        let hue0 = hue_for_chord(0);
        Self {
            t: 0.0,
            // bass low-center, pad wide upper arc.
            bass: Node::new(0.5, 0.80, 0.55, 7.0, 30.0),
            pad: Node::new(0.5, 0.24, 0.72, 3.2, hue0),
            kicks: Vec::with_capacity(MAX_KICK_WAVES),
            sparks: Vec::with_capacity(MAX_SPARKS),
            ambient_hue: hue0,
            last_kick: 0,
            last_bass: 0,
            last_tonal: 0,
            last_perc: 0,
            last_clap: 0,
            scatter: 0.0,
        }
    }

    pub(crate) fn tick(&mut self, dt: f32, telemetry: &FluidTelemetry) {
        self.t += dt;
        let levels = telemetry.levels();
        let chord_hue = hue_for_chord(telemetry.chord_index.load(Ordering::Relaxed));
        self.ambient_hue = lerp_hue(self.ambient_hue, chord_hue, smooth(dt, 1.2));

        // Bass node: amplitude from level, wavelength retunes to the note.
        let bass_sf = bass_spatial_freq(telemetry.bass_note_hz());
        self.bass.spatial_freq += (bass_sf - self.bass.spatial_freq) * smooth(dt, 0.25);
        let bass_amp = visual_amp(levels.bass, BASS_LEVEL_GAIN);
        self.bass.amp += (bass_amp - self.bass.amp) * smooth(dt, 0.07);
        self.bass.phase += dt * BASS_RING_SPEED;

        // Pad node: the field's ambient medium. Hue follows the chord; its
        // level also gates the base simmer in `field`, so a silent pad
        // leaves the medium black and still.
        let pad_amp = visual_amp(levels.pad, PAD_LEVEL_GAIN);
        self.pad.amp += (pad_amp - self.pad.amp) * smooth(dt, 0.3);
        self.pad.hue = lerp_hue(self.pad.hue, chord_hue, smooth(dt, 0.8));
        self.pad.phase += dt * PAD_RING_SPEED;

        // Bass trigger restarts the ring so a fresh wavefront emanates.
        let bass_pulse = telemetry.bass_pulse.load(Ordering::Relaxed);
        if bass_pulse > self.last_bass {
            self.bass.phase = 0.0;
            self.last_bass = bass_pulse;
        }

        // Kick -> one radial wavefront from a point near the bottom, pushed
        // strongest straight up. Amplitude is strictly the live kick level:
        // a muted kick draws nothing. Multiple pulses within one frame
        // collapse into one front.
        let kick = telemetry.kick_pulse.load(Ordering::Relaxed);
        if kick > self.last_kick {
            let amp = visual_amp(levels.kick, KICK_LEVEL_GAIN);
            if amp > SPAWN_MIN {
                if self.kicks.len() >= MAX_KICK_WAVES {
                    self.kicks.remove(0);
                }
                self.scatter = (self.scatter + 0.618_034).fract();
                self.kicks.push(KickWave {
                    x: 0.35 + self.scatter * 0.3,
                    age: 0.0,
                    life: KICK_WAVE_LIFE,
                    amp,
                });
            }
            self.last_kick = kick;
        }

        // Tonal -> a surface spark at the pitch's height; size and
        // brightness from the live level, hue cyan->green by pitch.
        let tonal = telemetry.tonal_pulse.load(Ordering::Relaxed);
        if tonal > self.last_tonal {
            let amp = visual_amp(levels.tonal, TONAL_LEVEL_GAIN);
            if amp > SPAWN_MIN {
                self.scatter = (self.scatter + 0.618_034).fract();
                let hz = telemetry.tonal_note_hz();
                let pitch_t =
                    ((hz.max(1.0).log2() - 110.0_f32.log2()) / 3.0).clamp(0.0, 1.0);
                self.push_spark(Spark {
                    voice: SparkVoice::Tonal,
                    x: 0.16 + self.scatter * 0.68,
                    y: tonal_node_y(hz),
                    age: 0.0,
                    life: TONAL_SPARK_LIFE,
                    size: 0.035 + amp * 0.05,
                    amp,
                    hue: 190.0 - pitch_t * 60.0,
                });
            }
            self.last_tonal = tonal;
        }

        // Perc -> sharp glint on the left flank; clap mirrors on the right.
        let perc = telemetry.perc_pulse.load(Ordering::Relaxed);
        if perc > self.last_perc {
            let amp = visual_amp(levels.perc, PERC_LEVEL_GAIN);
            if amp > SPAWN_MIN {
                self.scatter = (self.scatter + 0.381_966).fract();
                self.push_spark(glint(
                    SparkVoice::Perc,
                    0.13,
                    0.35 + self.scatter * 0.3,
                    amp,
                    210.0,
                ));
            }
            self.last_perc = perc;
        }
        let clap = telemetry.clap_pulse.load(Ordering::Relaxed);
        if clap > self.last_clap {
            let amp = visual_amp(levels.clap, CLAP_LEVEL_GAIN);
            if amp > SPAWN_MIN {
                self.scatter = (self.scatter + 0.381_966).fract();
                self.push_spark(glint(
                    SparkVoice::Clap,
                    0.87,
                    0.35 + self.scatter * 0.3,
                    amp,
                    330.0,
                ));
            }
            self.last_clap = clap;
        }

        for k in &mut self.kicks {
            k.age += dt;
        }
        self.kicks.retain(|k| k.age < k.life);
        // Live decay: a spark can never outshine its voice's current
        // envelope, so brightness follows the sound's real decay tail.
        let live = |voice: SparkVoice| match voice {
            SparkVoice::Tonal => visual_amp(levels.tonal, TONAL_LEVEL_GAIN),
            SparkVoice::Perc => visual_amp(levels.perc, PERC_LEVEL_GAIN),
            SparkVoice::Clap => visual_amp(levels.clap, CLAP_LEVEL_GAIN),
        };
        for s in &mut self.sparks {
            s.age += dt;
            s.amp = s.amp.min(live(s.voice));
        }
        self.sparks.retain(|s| s.age < s.life && s.amp > 0.005);
    }

    fn push_spark(&mut self, s: Spark) {
        if self.sparks.len() >= MAX_SPARKS {
            self.sparks.remove(0);
        }
        self.sparks.push(s);
    }

    /// Sample the fluid layer at normalized coords: pad wash + bass rings +
    /// kick wavefronts superposed. Brightness is 0 where nothing sounds.
    /// Hue accumulates as a weighted vector on the colour wheel so
    /// overlapping sources mix instead of flickering winner-take-all.
    pub(crate) fn field(&self, nx: f32, ny: f32) -> FieldSample {
        let z = self.t * 0.4;
        let mut v = ((nx * 5.0 + z).sin() * (ny * 4.0 - z * 0.6).cos()
            + ((nx * 3.0 - ny * 3.5) + z * 1.1).sin() * 0.5)
            * AMBIENT_DRIFT
            * self.pad.amp;
        let mut energy = AMBIENT_FLOOR * self.pad.amp;
        let mut hx = 0.0f32;
        let mut hy = 0.0f32;
        let add_hue = |hx: &mut f32, hy: &mut f32, hue: f32, w: f32| {
            let r = hue.to_radians();
            *hx += r.cos() * w;
            *hy += r.sin() * w;
        };
        add_hue(&mut hx, &mut hy, self.ambient_hue, energy.max(1e-4));

        for node in [&self.bass, &self.pad] {
            if node.amp < 1e-3 {
                continue;
            }
            let dx = nx - node.x;
            let dy = ny - node.y;
            let d2 = dx * dx + dy * dy;
            let falloff = (-d2 / (node.reach * node.reach)).exp();
            let w = node.amp * falloff;
            if w > 1e-4 {
                let dist = d2.sqrt();
                v += (dist * node.spatial_freq - node.phase).sin() * w;
                energy += w;
                add_hue(&mut hx, &mut hy, node.hue, w);
            }
        }

        for kw in &self.kicks {
            let dx = nx - kw.x;
            let dy = ny - KICK_ORIGIN_Y;
            let dist = (dx * dx + dy * dy).sqrt();
            let front = kw.age * KICK_WAVE_SPEED;
            let fade = (1.0 - kw.age / kw.life).max(0.0);
            let band = (-((dist - front) * KICK_WAVE_TIGHT).powi(2)).exp();
            // Radial ring, weighted to push hardest straight up.
            let up = if dist > 1e-4 { (-dy / dist).max(0.0) } else { 1.0 };
            let w = band * fade * kw.amp * (0.45 + 0.55 * up);
            if w > 1e-4 {
                // A bright ring with ripple texture trailing the front.
                v += ((front - dist) * KICK_WAVE_FREQ - kw.age * 6.0)
                    .sin()
                    .mul_add(0.5, 0.8)
                    * w;
                energy += w;
                add_hue(&mut hx, &mut hy, KICK_HUE, w);
            }
        }

        let hue = if hx == 0.0 && hy == 0.0 {
            self.ambient_hue
        } else {
            hy.atan2(hx).to_degrees().rem_euclid(360.0)
        };
        let wave = (v * FIELD_GAIN).tanh() * 0.5 + 0.5;
        let value =
            ((0.35 + 0.65 * wave) * (energy * ENERGY_GAIN).min(1.0)).clamp(0.0, 1.0);
        FieldSample { value, hue }
    }

    /// Sample the discrete surface layer (tonal notes, perc/clap glints).
    /// Returns the brightest covering element, if any; the widget composites
    /// it over the field so sparks stay crisp when the field is busy.
    pub(crate) fn surface(&self, nx: f32, ny: f32) -> Option<FieldSample> {
        let mut best_v = 0.0f32;
        let mut hue = 0.0f32;
        for s in &self.sparks {
            let dx = (nx - s.x) / s.size;
            let dy = (ny - s.y) / s.size;
            let fade = (1.0 - s.age / s.life).max(0.0);
            let v = (-(dx * dx + dy * dy)).exp() * fade * s.amp;
            if v > best_v {
                best_v = v;
                hue = s.hue;
            }
        }
        (best_v > SURFACE_MIN).then_some(FieldSample {
            value: best_v.min(1.0),
            hue,
        })
    }
}

/// A small flank glint (perc/clap). Its lifetime is a generous ceiling; the
/// voice's live envelope is the real clock that fades it out.
fn glint(voice: SparkVoice, x: f32, y: f32, amp: f32, hue: f32) -> Spark {
    Spark {
        voice,
        x,
        y,
        age: 0.0,
        life: GLINT_LIFE,
        size: 0.028,
        amp,
        hue,
    }
}

// Per-voice level -> visual amplitude gains, and wave motion/mix constants.
// Voice output magnitudes are small; these lift them into a legible 0..1 range.
const BASS_LEVEL_GAIN: f32 = 5.0;
const PAD_LEVEL_GAIN: f32 = 6.0;
const KICK_LEVEL_GAIN: f32 = 5.0;
const TONAL_LEVEL_GAIN: f32 = 6.0;
const PERC_LEVEL_GAIN: f32 = 8.0;
const CLAP_LEVEL_GAIN: f32 = 6.0;
const BASS_RING_SPEED: f32 = 3.2;
const PAD_RING_SPEED: f32 = 1.1;
/// Caps on live transients so per-cell field cost stays bounded fullscreen.
const MAX_KICK_WAVES: usize = 8;
const MAX_SPARKS: usize = 24;
/// Spawn gate: a trigger whose voice level maps below this draws nothing.
const SPAWN_MIN: f32 = 0.02;
/// Minimum surface-layer intensity that claims a cell from the field.
const SURFACE_MIN: f32 = 0.04;
/// Spark lifetime ceilings. Generous on purpose: the owning voice's live
/// envelope is the real clock; these only bound the spatial cleanup.
const TONAL_SPARK_LIFE: f32 = 2.5;
const GLINT_LIFE: f32 = 1.5;
/// Kick wavefront: origin height, radial speed (units/s), ring sharpness,
/// ripple frequency, lifetime, and hue (warm white-amber).
const KICK_ORIGIN_Y: f32 = 0.96;
const KICK_WAVE_SPEED: f32 = 0.7;
const KICK_WAVE_TIGHT: f32 = 7.0;
const KICK_WAVE_FREQ: f32 = 26.0;
const KICK_WAVE_LIFE: f32 = 1.6;
const KICK_HUE: f32 = 40.0;
/// Weight of the always-on ambient plasma (kept low so voices dominate).
const AMBIENT_DRIFT: f32 = 0.5;
/// Baseline energy so a silent field is near-black but not pure void.
const AMBIENT_FLOOR: f32 = 0.06;
/// Scales summed local wave energy into cell brightness.
const ENERGY_GAIN: f32 = 1.6;
/// Soft-clamp gain on the summed wave before mapping to 0..1.
const FIELD_GAIN: f32 = 0.85;

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

        for y in 0..area.height {
            for x in 0..area.width {
                let nx = x as f32 / w;
                let ny = y as f32 / h;
                let sample = self.fluid.field(nx, ny);
                // Surface elements win the cell: the field dims beneath them
                // so tonal notes and perc/clap glints stay crisp.
                let (v, hue, sat_boost) = match self.fluid.surface(nx, ny) {
                    Some(s) => (
                        (sample.value * 0.35 + s.value).clamp(0.0, 1.0),
                        s.hue,
                        0.25,
                    ),
                    None => (sample.value, sample.hue, 0.0),
                };

                // edge vignette
                let edge_x = (nx.min(1.0 - nx) * 2.0).min(1.0);
                let edge_y = (ny.min(1.0 - ny) * 2.0).min(1.0);
                let vig = (edge_x.min(edge_y) * 1.4).clamp(0.2, 1.0);

                let sat = (0.55 + v * 0.25 + sat_boost).clamp(0.0, 1.0);
                let val = ((0.05 + v * 0.9) * vig).clamp(0.0, 1.0);

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
    visuals_focus: bool,
) {
    let area = f.area();
    f.render_widget(FluidWidget { fluid }, area);

    // Focus mode: show the full field, no control panel. A one-line hint sits
    // at the bottom so the user knows any key returns.
    if visuals_focus {
        f.render_widget(
            Paragraph::new("visuals focus — press any key to return")
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::Rgb(150, 160, 185))),
            Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1),
        );
        return;
    }

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
