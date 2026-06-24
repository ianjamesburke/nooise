use std::error::Error;
use std::f32::consts::TAU;
use std::io;
use std::sync::Arc;

use arc_swap::ArcSwap;

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Gauge, Paragraph},
    Frame, Terminal,
};

use crate::audio::{self, StereoEngine};
use crate::fx::lfo::DriftingLfo;
use crate::fx::panner::StereoPanner;
use crate::fx::reverb::Freeverb;
use crate::synth::envelope::Adsr;
use crate::synth::noise::WhiteNoise;
use crate::synth::oscillator::SineOscillator;

// ============================================================
// Controls
// ============================================================

#[derive(Clone)]
pub(crate) struct MasterControls {
    pub bpm: f32,
    pub level: f32,
    pub drive: f32,
    pub comp_threshold: f32,  // dB, -40 to 0
    pub comp_ratio: f32,      // 1-8
    pub comp_release_ms: f32, // 10-500
    pub tone: f32,            // -1 (bass) to +1 (treble)
}

impl Default for MasterControls {
    fn default() -> Self {
        Self {
            bpm: 92.0,
            level: 0.8,
            drive: 0.0,
            comp_threshold: -12.0,
            comp_ratio: 2.0,
            comp_release_ms: 100.0,
            tone: 0.0,
        }
    }
}

#[derive(Clone)]
pub(crate) struct PercControls {
    pub level: f32,
    pub decay_ms: f32,
    pub filter: f32,
    pub lfo_rate: f32,
    pub lfo_depth: f32,
}

impl Default for PercControls {
    fn default() -> Self {
        Self { level: 0.0, decay_ms: 80.0, filter: 0.8, lfo_rate: 2.0, lfo_depth: 0.3 }
    }
}

#[derive(Clone)]
pub(crate) struct PadControls {
    pub level: f32,
    pub chord_bars: f32, // 1,2,4,8,16,32,64
    pub reverb_mix: f32,
    pub stereo_width: f32,
    pub detune: f32,
    pub octave_mix: f32,
    pub attack_time: f32,
}

impl Default for PadControls {
    fn default() -> Self {
        Self {
            level: 0.7,
            chord_bars: 4.0,
            reverb_mix: 0.8,
            stereo_width: 0.8,
            detune: 0.5,
            octave_mix: 0.5,
            attack_time: 6.0,
        }
    }
}

#[derive(Clone)]
pub(crate) struct KickControls {
    pub level: f32,
    pub start_freq: f32,
    pub pitch_decay_ms: f32,
    pub amp_decay_ms: f32,
    pub click: f32,        // 0–0.2 UI range
    pub drive: f32,
    pub interval_beats: f32,
    pub echo_time_beats: f32,
    pub echo_filter: f32,
    pub echo_amount: f32,
    pub echo_feedback: f32,
}

impl Default for KickControls {
    fn default() -> Self {
        Self {
            level: 0.0,
            start_freq: 80.0,
            pitch_decay_ms: 60.0,
            amp_decay_ms: 350.0,
            click: 0.0,
            drive: 0.2,
            interval_beats: 1.0,
            echo_time_beats: 1.0,
            echo_filter: 0.5,
            echo_amount: 0.0,
            echo_feedback: 0.0,
        }
    }
}

#[derive(Clone)]
pub(crate) struct TonalControls {
    pub level: f32,
    pub randomness: f32,
    pub note_length: f32,
    pub step_interval: f32,
    pub reverb_mix: f32,
}

impl Default for TonalControls {
    fn default() -> Self {
        Self { level: 0.0, randomness: 0.3, note_length: 0.8, step_interval: 1.0, reverb_mix: 0.6 }
    }
}

#[derive(Clone)]
pub(crate) struct ClapControls {
    pub level: f32,
    pub interval_beats: f32,
    pub slap_count: f32,     // 1-8
    pub slap_spread_ms: f32, // 0-100 ms
    pub decay_ms: f32,       // 10-200 ms
    pub filter: f32,         // 0=dark 1=bright
    pub room: f32,           // 0-1 reverb send
    pub body: f32,           // 0-1 low-freq flesh mix
}

impl Default for ClapControls {
    fn default() -> Self {
        Self {
            level: 0.0,
            interval_beats: 4.0,
            slap_count: 3.0,
            slap_spread_ms: 8.0,
            decay_ms: 40.0,
            filter: 0.85,
            room: 0.4,
            body: 0.2,
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct T5Controls {
    pub master: MasterControls,
    pub perc: PercControls,
    pub pad: PadControls,
    pub kick: KickControls,
    pub tonal: TonalControls,
    pub clap: ClapControls,
}

// ============================================================
// Entry point
// ============================================================

pub(crate) fn run() -> Result<(), Box<dyn Error>> {
    let controls = Arc::new(ArcSwap::from_pointee(T5Controls::default()));
    let controls_for_engine = Arc::clone(&controls);

    let _stream = audio::start_stream("t5", move |sr| T5Engine::new(sr, controls_for_engine))?;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = ui_loop(&mut terminal, controls);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    result
}

// ============================================================
// UI
// ============================================================

#[derive(Clone, Copy, PartialEq)]
enum Tab { Master = 0, Perc = 1, Chords = 2, Kick = 3, Tonal = 4, Clap = 5 }

impl Tab {
    fn all() -> [Tab; 6] { [Tab::Master, Tab::Perc, Tab::Chords, Tab::Kick, Tab::Tonal, Tab::Clap] }

    fn name(self) -> &'static str {
        match self {
            Tab::Master => "Master",
            Tab::Perc   => "Perc",
            Tab::Chords => "Chords",
            Tab::Kick   => "Kick",
            Tab::Tonal  => "Tonal",
            Tab::Clap   => "Clap",
        }
    }

    fn next(self) -> Self {
        match self {
            Tab::Master => Tab::Perc,
            Tab::Perc   => Tab::Chords,
            Tab::Chords => Tab::Kick,
            Tab::Kick   => Tab::Tonal,
            Tab::Tonal  => Tab::Clap,
            Tab::Clap   => Tab::Master,
        }
    }

    fn control_count(self) -> usize {
        match self {
            Tab::Master => 12,
            Tab::Perc   => 5,
            Tab::Chords => 7,
            Tab::Kick   => 11,
            Tab::Tonal  => 5,
            Tab::Clap   => 8,
        }
    }
}

struct ControlItem {
    label: &'static str,
    value: f32,
    min: f32,
    max: f32,
    display: String,
}

fn tab_controls(tab: Tab, c: &T5Controls) -> Vec<ControlItem> {
    match tab {
        Tab::Master => vec![
            ControlItem { label: "Chords Vol",     value: c.pad.level,              min: 0.0,   max: 1.0,   display: format!("{:.0}%", c.pad.level * 100.0) },
            ControlItem { label: "Perc Vol",       value: c.perc.level,             min: 0.0,   max: 1.0,   display: format!("{:.0}%", c.perc.level * 100.0) },
            ControlItem { label: "Kick Vol",       value: c.kick.level,             min: 0.0,   max: 1.0,   display: format!("{:.0}%", c.kick.level * 100.0) },
            ControlItem { label: "Tonal Vol",      value: c.tonal.level,            min: 0.0,   max: 1.0,   display: format!("{:.0}%", c.tonal.level * 100.0) },
            ControlItem { label: "Clap Vol",       value: c.clap.level,             min: 0.0,   max: 1.0,   display: format!("{:.0}%", c.clap.level * 100.0) },
            ControlItem { label: "BPM",            value: c.master.bpm,             min: 60.0,  max: 200.0, display: format!("{:.0} bpm", c.master.bpm) },
            ControlItem { label: "Master Level",   value: c.master.level,           min: 0.0,   max: 1.0,   display: format!("{:.0}%", c.master.level * 100.0) },
            ControlItem { label: "Drive",          value: c.master.drive,           min: 0.0,   max: 1.0,   display: format!("{:.0}%", c.master.drive * 100.0) },
            ControlItem { label: "Comp Threshold", value: c.master.comp_threshold,  min: -40.0, max: 0.0,   display: format!("{:.0} dB", c.master.comp_threshold) },
            ControlItem { label: "Comp Ratio",     value: c.master.comp_ratio,      min: 1.0,   max: 8.0,   display: format!("{:.1}:1", c.master.comp_ratio) },
            ControlItem { label: "Comp Release",   value: c.master.comp_release_ms, min: 10.0,  max: 500.0, display: format!("{:.0} ms", c.master.comp_release_ms) },
            ControlItem {
                label: "Tone",
                value: c.master.tone + 1.0,
                min: 0.0,
                max: 2.0,
                display: if c.master.tone < -0.05 {
                    format!("bass {:.0}%", -c.master.tone * 100.0)
                } else if c.master.tone > 0.05 {
                    format!("treble {:.0}%", c.master.tone * 100.0)
                } else {
                    "flat".to_string()
                },
            },
        ],
        Tab::Perc => vec![
            ControlItem { label: "Level",     value: c.perc.level,     min: 0.0,  max: 1.0,   display: format!("{:.0}%", c.perc.level * 100.0) },
            ControlItem { label: "Decay",     value: c.perc.decay_ms,  min: 20.0, max: 2000.0, display: if c.perc.decay_ms >= 1000.0 { format!("{:.1} s", c.perc.decay_ms / 1000.0) } else { format!("{:.0} ms", c.perc.decay_ms) } },
            ControlItem { label: "Filter",    value: c.perc.filter,    min: 0.5,  max: 1.0,   display: format!("{:.0}%", c.perc.filter * 100.0) },
            ControlItem { label: "LFO Rate",  value: c.perc.lfo_rate,  min: 0.25, max: 16.0,  display: {
                let b = c.perc.lfo_rate;
                if b < 1.0 { format!("1/{} bar", (1.0 / b).round() as u32) } else { format!("{} bar{}", b as u32, if b as u32 == 1 { "" } else { "s" }) }
            } },
            ControlItem { label: "LFO Depth", value: c.perc.lfo_depth, min: 0.0,  max: 1.0,   display: format!("{:.0}%", c.perc.lfo_depth * 100.0) },
        ],
        Tab::Chords => vec![
            ControlItem { label: "Level",        value: c.pad.level,             min: 0.0,  max: 1.0,  display: format!("{:.0}%", c.pad.level * 100.0) },
            ControlItem { label: "Chord Bars",   value: c.pad.chord_bars.log2(), min: 0.0,  max: 6.0,  display: format!("{:.0} bars", c.pad.chord_bars) },
            ControlItem { label: "Reverb Mix",   value: c.pad.reverb_mix,        min: 0.0,  max: 1.0,  display: format!("{:.0}%", c.pad.reverb_mix * 100.0) },
            ControlItem { label: "Stereo Width", value: c.pad.stereo_width,      min: 0.0,  max: 1.0,  display: format!("{:.0}%", c.pad.stereo_width * 100.0) },
            ControlItem { label: "Detune",       value: c.pad.detune,            min: 0.0,  max: 1.0,  display: format!("{:.0}%", c.pad.detune * 100.0) },
            ControlItem { label: "Octave Mix",   value: c.pad.octave_mix,        min: 0.0,  max: 1.0,  display: format!("{:.0}%", c.pad.octave_mix * 100.0) },
            ControlItem { label: "Attack",       value: c.pad.attack_time,       min: 1.0,  max: 30.0, display: format!("{:.1} s", c.pad.attack_time) },
        ],
        Tab::Kick => vec![
            ControlItem { label: "Level",       value: c.kick.level,           min: 0.0,   max: 1.0,    display: format!("{:.0}%", c.kick.level * 100.0) },
            ControlItem { label: "Start Freq",  value: c.kick.start_freq,      min: 40.0,  max: 200.0,  display: format!("{:.0} Hz", c.kick.start_freq) },
            ControlItem { label: "Pitch Decay", value: c.kick.pitch_decay_ms,  min: 10.0,  max: 300.0,  display: format!("{:.0} ms", c.kick.pitch_decay_ms) },
            ControlItem { label: "Amp Decay",   value: c.kick.amp_decay_ms,    min: 50.0,  max: 1000.0, display: format!("{:.0} ms", c.kick.amp_decay_ms) },
            ControlItem { label: "Click",        value: c.kick.click,            min: 0.0,   max: 0.2,    display: format!("{:.0}%", c.kick.click / 0.2 * 100.0) },
            ControlItem { label: "Drive",        value: c.kick.drive,            min: 0.0,   max: 1.0,    display: format!("{:.0}%", c.kick.drive * 100.0) },
            ControlItem { label: "Interval",     value: c.kick.interval_beats,   min: 0.5,   max: 4.0,    display: format!("{:.2} beats", c.kick.interval_beats) },
            ControlItem { label: "Echo Time",    value: c.kick.echo_time_beats,  min: 0.125, max: 2.0,    display: format!("{:.3} beats", c.kick.echo_time_beats) },
            ControlItem { label: "Echo Filter",  value: c.kick.echo_filter,      min: 0.0,   max: 1.0,    display: format!("{:.0}%", c.kick.echo_filter * 100.0) },
            ControlItem { label: "Echo Amount",  value: c.kick.echo_amount,      min: 0.0,   max: 0.9,    display: format!("{:.0}%", c.kick.echo_amount / 0.9 * 100.0) },
            ControlItem { label: "Echo Feedback",value: c.kick.echo_feedback,    min: 0.0,   max: 0.85,   display: format!("{:.0}%", c.kick.echo_feedback / 0.85 * 100.0) },
        ],
        Tab::Tonal => vec![
            ControlItem { label: "Level",         value: c.tonal.level,          min: 0.0, max: 1.0, display: format!("{:.0}%", c.tonal.level * 100.0) },
            ControlItem { label: "Randomness",    value: c.tonal.randomness,     min: 0.0, max: 1.0, display: format!("{:.0}%", c.tonal.randomness * 100.0) },
            ControlItem { label: "Note Length",   value: c.tonal.note_length,    min: 0.1, max: 2.0, display: format!("{:.2} beats", c.tonal.note_length) },
            ControlItem { label: "Step Interval", value: c.tonal.step_interval,  min: 0.5, max: 4.0, display: format!("{:.2} beats", c.tonal.step_interval) },
            ControlItem { label: "Reverb Mix",    value: c.tonal.reverb_mix,     min: 0.0, max: 1.0, display: format!("{:.0}%", c.tonal.reverb_mix * 100.0) },
        ],
        Tab::Clap => vec![
            ControlItem { label: "Level",        value: c.clap.level,           min: 0.0, max: 1.0,  display: format!("{:.0}%", c.clap.level * 100.0) },
            ControlItem { label: "Interval",     value: c.clap.interval_beats,  min: 0.5, max: 8.0,  display: format!("{:.2} beats", c.clap.interval_beats) },
            ControlItem { label: "Slap Count",   value: c.clap.slap_count,      min: 1.0, max: 8.0,   display: format!("{:.0}", c.clap.slap_count) },
            ControlItem { label: "Slap Spread",  value: c.clap.slap_spread_ms,  min: 0.0, max: 100.0, display: format!("{:.1} ms", c.clap.slap_spread_ms) },
            ControlItem { label: "Decay",        value: c.clap.decay_ms,        min: 10.0,max: 200.0, display: format!("{:.0} ms", c.clap.decay_ms) },
            ControlItem { label: "Filter",       value: c.clap.filter,          min: 0.5, max: 1.0,  display: format!("{:.0}%", c.clap.filter * 100.0) },
            ControlItem { label: "Room",         value: c.clap.room,            min: 0.0, max: 1.0,  display: format!("{:.0}%", c.clap.room * 100.0) },
            ControlItem { label: "Body",         value: c.clap.body,            min: 0.0, max: 1.0,  display: format!("{:.0}%", c.clap.body * 100.0) },
        ],
    }
}

fn apply_delta(tab: Tab, selected: usize, dir: f32, c: &mut T5Controls) {
    match tab {
        Tab::Master => match selected {
            0 => c.pad.level              = (c.pad.level              + dir *  0.02).clamp(0.0,   1.0),
            1 => c.perc.level             = (c.perc.level             + dir *  0.02).clamp(0.0,   1.0),
            2 => c.kick.level             = (c.kick.level             + dir *  0.02).clamp(0.0,   1.0),
            3 => c.tonal.level            = (c.tonal.level            + dir *  0.02).clamp(0.0,   1.0),
            4 => c.clap.level             = (c.clap.level             + dir *  0.02).clamp(0.0,   1.0),
            5 => c.master.bpm             = (c.master.bpm             + dir *  2.0).clamp(60.0,  200.0),
            6 => c.master.level           = (c.master.level           + dir *  0.02).clamp(0.0,   1.0),
            7 => c.master.drive           = (c.master.drive           + dir *  0.02).clamp(0.0,   1.0),
            8 => c.master.comp_threshold  = (c.master.comp_threshold  + dir *  1.0).clamp(-40.0,  0.0),
            9 => c.master.comp_ratio      = (c.master.comp_ratio      + dir *  0.25).clamp(1.0,   8.0),
           10 => c.master.comp_release_ms = (c.master.comp_release_ms + dir * 10.0).clamp(10.0,  500.0),
           11 => c.master.tone            = (c.master.tone            + dir *  0.05).clamp(-1.0,  1.0),
            _ => {}
        },
        Tab::Perc => match selected {
            0 => c.perc.level     = (c.perc.level     + dir *  0.02).clamp(0.0,  1.0),
            1 => c.perc.decay_ms  = (c.perc.decay_ms  + dir * 20.0).clamp(20.0, 2000.0),
            2 => c.perc.filter    = (c.perc.filter    + dir *  0.02).clamp(0.5,  1.0),
            3 => c.perc.lfo_rate  = (c.perc.lfo_rate  + dir * 0.25).clamp(0.25, 16.0),
            4 => c.perc.lfo_depth = (c.perc.lfo_depth + dir *  0.02).clamp(0.0,  1.0),
            _ => {}
        },
        Tab::Chords => match selected {
            0 => c.pad.level        = (c.pad.level        + dir * 0.02).clamp(0.0,  1.0),
            1 => if dir > 0.0 { c.pad.chord_bars = (c.pad.chord_bars * 2.0).min(64.0) } else { c.pad.chord_bars = (c.pad.chord_bars / 2.0).max(1.0) },
            2 => c.pad.reverb_mix   = (c.pad.reverb_mix   + dir * 0.02).clamp(0.0,  1.0),
            3 => c.pad.stereo_width = (c.pad.stereo_width + dir * 0.02).clamp(0.0,  1.0),
            4 => c.pad.detune       = (c.pad.detune       + dir * 0.02).clamp(0.0,  1.0),
            5 => c.pad.octave_mix   = (c.pad.octave_mix   + dir * 0.02).clamp(0.0,  1.0),
            6 => c.pad.attack_time  = (c.pad.attack_time  + dir * 1.0).clamp(1.0,   30.0),
            _ => {}
        },
        Tab::Kick => match selected {
            0 => c.kick.level           = (c.kick.level           + dir *  0.02).clamp(0.0,   1.0),
            1 => c.kick.start_freq      = (c.kick.start_freq      + dir *  5.0).clamp(40.0,   200.0),
            2 => c.kick.pitch_decay_ms  = (c.kick.pitch_decay_ms  + dir *  5.0).clamp(10.0,   300.0),
            3 => c.kick.amp_decay_ms    = (c.kick.amp_decay_ms    + dir * 20.0).clamp(50.0,   1000.0),
            4 => c.kick.click           = (c.kick.click           + dir *  0.01).clamp(0.0,   0.2),
            5 => c.kick.drive           = (c.kick.drive           + dir *  0.02).clamp(0.0,   1.0),
            6 => c.kick.interval_beats  = (c.kick.interval_beats  + dir *  0.25).clamp(0.5,   4.0),
            7 => c.kick.echo_time_beats = (c.kick.echo_time_beats + dir *  0.125).clamp(0.125, 2.0),
            8 => c.kick.echo_filter     = (c.kick.echo_filter     + dir *  0.02).clamp(0.0,   1.0),
            9 => c.kick.echo_amount     = (c.kick.echo_amount     + dir *  0.02).clamp(0.0,   0.9),
           10 => c.kick.echo_feedback   = (c.kick.echo_feedback   + dir *  0.02).clamp(0.0,   0.85),
            _ => {}
        },
        Tab::Tonal => match selected {
            0 => c.tonal.level         = (c.tonal.level         + dir *  0.02).clamp(0.0, 1.0),
            1 => c.tonal.randomness    = (c.tonal.randomness    + dir *  0.02).clamp(0.0, 1.0),
            2 => c.tonal.note_length   = (c.tonal.note_length   + dir *  0.05).clamp(0.1, 2.0),
            3 => c.tonal.step_interval = (c.tonal.step_interval + dir *  0.25).clamp(0.5, 4.0),
            4 => c.tonal.reverb_mix    = (c.tonal.reverb_mix    + dir *  0.02).clamp(0.0, 1.0),
            _ => {}
        },
        Tab::Clap => match selected {
            0 => c.clap.level          = (c.clap.level          + dir *  0.02).clamp(0.0,  1.0),
            1 => c.clap.interval_beats = (c.clap.interval_beats + dir *  0.25).clamp(0.5,  8.0),
            2 => c.clap.slap_count     = (c.clap.slap_count     + dir *  1.0).clamp(1.0,   8.0),
            3 => c.clap.slap_spread_ms = (c.clap.slap_spread_ms + dir *  2.0).clamp(0.0,  100.0),
            4 => c.clap.decay_ms       = (c.clap.decay_ms       + dir *  5.0).clamp(10.0,  200.0),
            5 => c.clap.filter         = (c.clap.filter         + dir *  0.02).clamp(0.5,  1.0),
            6 => c.clap.room           = (c.clap.room           + dir *  0.02).clamp(0.0,  1.0),
            7 => c.clap.body           = (c.clap.body           + dir *  0.02).clamp(0.0,  1.0),
            _ => {}
        },
    }
}

fn ui_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    controls: Arc<ArcSwap<T5Controls>>,
) -> Result<(), Box<dyn Error>> {
    let mut tab = Tab::Master;
    let mut selected = 0usize;

    loop {
        terminal.draw(|f| render(f, &controls, tab, selected))?;

        if event::poll(std::time::Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press { continue; }
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Tab => { tab = tab.next(); selected = 0; }
                    KeyCode::Up   | KeyCode::Char('k') => selected = selected.saturating_sub(1),
                    KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1).min(tab.control_count() - 1),
                    KeyCode::Left  | KeyCode::Char('h') => adjust(&controls, tab, selected, -1.0),
                    KeyCode::Right | KeyCode::Char('l') => adjust(&controls, tab, selected,  1.0),
                    _ => {}
                }
            }
        }
    }

    Ok(())
}

fn adjust(controls: &Arc<ArcSwap<T5Controls>>, tab: Tab, selected: usize, dir: f32) {
    let mut next = T5Controls::clone(&controls.load());
    apply_delta(tab, selected, dir, &mut next);
    controls.store(Arc::new(next));
}

fn render(f: &mut Frame, controls: &Arc<ArcSwap<T5Controls>>, active_tab: Tab, selected: usize) {
    let c = controls.load();
    let area = f.area();

    let outer = Block::default().title(" t5 ").borders(Borders::ALL);
    let inner = outer.inner(area);
    f.render_widget(outer, area);

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(inner);

    let tab_line: String = Tab::all()
        .iter()
        .map(|t| if *t == active_tab { format!("[{}]", t.name()) } else { t.name().to_string() })
        .collect::<Vec<_>>()
        .join("  |  ");
    let tab_style = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
    f.render_widget(Paragraph::new(tab_line).style(tab_style), layout[0]);

    let items = tab_controls(active_tab, &c);
    let gauge_height = 2u16;
    let constraints: Vec<Constraint> = items.iter().map(|_| Constraint::Length(gauge_height)).collect();
    let slots = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(layout[2]);

    for (i, item) in items.iter().enumerate() {
        if i >= slots.len() { break; }
        let active = i == selected;
        let style = if active {
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let ratio = ((item.value - item.min) / (item.max - item.min)).clamp(0.0, 1.0) as f64;
        let prefix = if active { "▶ " } else { "  " };
        let gauge = Gauge::default()
            .block(Block::default().title(format!("{}{:<14}  {}", prefix, item.label, item.display)))
            .gauge_style(style)
            .ratio(ratio);
        f.render_widget(gauge, slots[i]);
    }

    f.render_widget(
        Paragraph::new("↑↓/jk select   ←→/hl adjust   Tab switch   q quit"),
        layout[3],
    );
}

// ============================================================
// T5 Engine
// ============================================================

struct T5Engine {
    current_sample: u64,
    sample_rate: f32,
    pad: PadEngine,
    perc: PercEngine,
    kick: KickEngine,
    tonal: TonalEngine,
    clap: ClapEngine,
    master_bus: MasterBus,
    controls: Arc<ArcSwap<T5Controls>>,
    snapshot: T5Controls,
}

impl T5Engine {
    fn new(sample_rate: f32, controls: Arc<ArcSwap<T5Controls>>) -> Self {
        let snapshot = T5Controls::clone(&controls.load());
        Self {
            current_sample: 0,
            sample_rate,
            pad: PadEngine::new(sample_rate, &snapshot.pad),
            perc: PercEngine::new(sample_rate),
            kick: KickEngine::new(sample_rate),
            tonal: TonalEngine::new(sample_rate),
            clap: ClapEngine::new(sample_rate),
            master_bus: MasterBus::new(),
            controls,
            snapshot,
        }
    }
}

impl StereoEngine for T5Engine {
    fn next_stereo(&mut self) -> (f32, f32) {
        if self.current_sample % 512 == 0 {
            self.snapshot = T5Controls::clone(&self.controls.load());
        }

        let fade = (self.current_sample as f32 / (self.sample_rate * 8.0)).min(1.0);
        let bpm = self.snapshot.master.bpm;

        let (pad_l, pad_r)   = self.pad.next(&self.snapshot.pad, bpm);
        let perc             = self.perc.next(&self.snapshot.perc, bpm);
        let (kick_l, kick_r) = self.kick.next(&self.snapshot.kick, bpm);
        let (ton_l, ton_r)   = self.tonal.next(&self.snapshot.tonal, bpm);
        let (clap_l, clap_r) = self.clap.next(&self.snapshot.clap, bpm);

        self.current_sample += 1;

        let raw_l = (pad_l + perc * 0.6 + kick_l * 0.7 + ton_l + clap_l * 0.65) * fade;
        let raw_r = (pad_r + perc * 0.6 + kick_r * 0.7 + ton_r + clap_r * 0.65) * fade;
        self.master_bus.process(raw_l, raw_r, &self.snapshot.master, self.sample_rate)
    }
}

// ============================================================
// Master bus (drive, tilt EQ, compressor)
// ============================================================

struct MasterBus {
    comp_env: f32,
    tone_l: f32,
    tone_r: f32,
}

impl MasterBus {
    fn new() -> Self { Self { comp_env: 0.0, tone_l: 0.0, tone_r: 0.0 } }

    fn process(&mut self, mut l: f32, mut r: f32, c: &MasterControls, sample_rate: f32) -> (f32, f32) {
        if c.drive > 0.001 {
            let gain = 1.0 + c.drive * 6.0;
            l = soft_clip(l * gain);
            r = soft_clip(r * gain);
        }

        if c.tone.abs() > 0.01 {
            let coeff = (0.05 + c.tone.abs() * 0.7).min(0.99);
            self.tone_l += coeff * (l - self.tone_l);
            self.tone_r += coeff * (r - self.tone_r);
            if c.tone > 0.0 {
                l += (l - self.tone_l) * c.tone * 0.6;
                r += (r - self.tone_r) * c.tone * 0.6;
            } else {
                l += self.tone_l * (-c.tone) * 0.6;
                r += self.tone_r * (-c.tone) * 0.6;
            }
        }

        let thresh_lin = 10_f32.powf(c.comp_threshold / 20.0);
        let attack_coeff = (-1.0_f32 / (0.001 * sample_rate)).exp();
        let rel_coeff    = (-1.0_f32 / (c.comp_release_ms * 0.001 * sample_rate)).exp();
        let peak = l.abs().max(r.abs());
        self.comp_env = if peak > self.comp_env {
            peak + attack_coeff * (self.comp_env - peak)
        } else {
            peak + rel_coeff * (self.comp_env - peak)
        };
        let gain_reduction = if self.comp_env > thresh_lin && c.comp_ratio > 1.001 {
            (thresh_lin / self.comp_env) * (self.comp_env / thresh_lin).powf(1.0 / c.comp_ratio)
        } else {
            1.0
        };

        (
            (l * gain_reduction * c.level).clamp(-0.95, 0.95),
            (r * gain_reduction * c.level).clamp(-0.95, 0.95),
        )
    }
}

// ============================================================
// Pad engine (chord drones, from t3)
// ============================================================

struct PadEngine {
    current_sample: u64,
    sample_rate: f32,
    layers: Vec<PadLayer>,
    next_change_sample: u64,
    chord_index: usize,
    reverb: Freeverb,
    depth_lfo: DriftingLfo,
    width_lfo: DriftingLfo,
    air: WhiteNoise,
    rng: StdRng,
}

impl PadEngine {
    fn new(sample_rate: f32, c: &PadControls) -> Self {
        Self {
            current_sample: 0,
            sample_rate,
            layers: vec![PadLayer::new(0, sample_rate, c.attack_time)],
            next_change_sample: (c.chord_bars * 4.0 * 60.0 / 92.0 * sample_rate).round() as u64,
            chord_index: 0,
            reverb: Freeverb::new(sample_rate, 0.93, 0.46, 1.0),
            depth_lfo: DriftingLfo::new(1.0 / 42.0, sample_rate),
            width_lfo: DriftingLfo::new(1.0 / 54.0, sample_rate),
            air: WhiteNoise::new(),
            rng: StdRng::from_entropy(),
        }
    }

    fn next(&mut self, c: &PadControls, bpm: f32) -> (f32, f32) {
        if self.current_sample >= self.next_change_sample {
            for layer in &mut self.layers { layer.release(); }
            self.chord_index = self.chord_index.wrapping_add(1);
            self.layers.push(PadLayer::new(self.chord_index, self.sample_rate, c.attack_time));
            self.next_change_sample =
                self.current_sample + (c.chord_bars * 4.0 * 60.0 / bpm * self.sample_rate).round() as u64;
        }

        let depth = normalized_lfo(self.depth_lfo.next(&mut self.rng, 1.0 / 68.0, 1.0 / 28.0));
        let width = c.stereo_width * (0.58 + normalized_lfo(self.width_lfo.next(&mut self.rng, 1.0 / 86.0, 1.0 / 38.0)) * 0.16);
        let detune_mix = c.detune * 0.84;
        let octave_mix = c.octave_mix * 0.32;

        let mut dry_l = 0.0f32;
        let mut dry_r = 0.0f32;
        for layer in &mut self.layers {
            let (l, r) = layer.next_stereo(width, detune_mix, octave_mix);
            dry_l += l; dry_r += r;
        }
        self.layers.retain(|l| !l.is_done());

        let reverb_send = c.reverb_mix * (0.48 + depth * 0.22);
        let (wet_l, wet_r) = self.reverb.process(dry_l * reverb_send, dry_r * reverb_send);
        let wet_mix = 0.72 + depth * 0.34;
        let air = self.air.next_filtered(&mut self.rng, 0.0002) * 0.00025;

        self.current_sample += 1;
        ((dry_l * 0.58 + wet_l * wet_mix + air) * c.level, (dry_r * 0.58 + wet_r * wet_mix + air) * c.level)
    }
}

struct PadLayer { tones: Vec<PadTone> }

impl PadLayer {
    fn new(chord_index: usize, sample_rate: f32, attack_time: f32) -> Self {
        Self { tones: pad_tones(chord_index, sample_rate, attack_time) }
    }
    fn next_stereo(&mut self, width: f32, detune_mix: f32, octave_mix: f32) -> (f32, f32) {
        let (mut l, mut r) = (0.0f32, 0.0f32);
        for t in &mut self.tones { let (tl, tr) = t.next_stereo(width, detune_mix, octave_mix); l += tl; r += tr; }
        (l, r)
    }
    fn release(&mut self) { for t in &mut self.tones { t.release(); } }
    fn is_done(&self) -> bool { self.tones.iter().all(PadTone::is_done) }
}

struct PadTone {
    primary: SineOscillator,
    detuned: SineOscillator,
    octave: SineOscillator,
    envelope: Adsr,
    pan: f32,
    gain: f32,
}

impl PadTone {
    fn new(hz: f32, pan: f32, gain: f32, attack_time: f32, sample_rate: f32) -> Self {
        Self {
            primary:  SineOscillator::new(hz, sample_rate),
            detuned:  SineOscillator::new(hz * 1.003, sample_rate),
            octave:   SineOscillator::new(hz * 2.0, sample_rate),
            envelope: Adsr::new(attack_time, 12.0, 0.86, 20.0, sample_rate),
            pan, gain,
        }
    }
    fn next_stereo(&mut self, width: f32, detune_mix: f32, octave_mix: f32) -> (f32, f32) {
        let s = self.primary.next() + self.detuned.next() * detune_mix + self.octave.next() * octave_mix;
        let shaped = soft_clip(s * 0.55) * self.envelope.next() * self.gain;
        StereoPanner::equal_power(shaped, self.pan * width)
    }
    fn release(&mut self) { self.envelope.note_off(); }
    fn is_done(&self) -> bool { self.envelope.is_done() }
}

fn pad_tones(chord_index: usize, sample_rate: f32, attack_time: f32) -> Vec<PadTone> {
    let freqs = pad_chord(chord_index);
    let pans  = [-0.52_f32, -0.18, 0.16, 0.46];
    let gains = [0.17_f32, 0.132, 0.126, 0.098];
    freqs.iter().zip(pans).zip(gains)
        .map(|((hz, pan), gain)| PadTone::new(*hz, pan, gain, attack_time, sample_rate))
        .collect()
}

fn pad_chord(index: usize) -> [f32; 4] {
    const CHORDS: [[f32; 4]; 5] = [
        [110.0, 146.83, 196.0,  261.63],
        [110.0, 164.81, 196.0,  293.66],
        [98.0,  146.83, 220.0,  261.63],
        [123.47,164.81, 196.0,  293.66],
        [110.0, 146.83, 220.0,  329.63],
    ];
    CHORDS[index % CHORDS.len()]
}

// ============================================================
// Perc engine (16th-note white noise hits)
// ============================================================

struct PercEngine {
    current_sample: u64,
    sample_rate: f32,
    next_hit_sample: u64,
    hits: Vec<NoiseHit>,
    vol_lfo: DriftingLfo,
    rng: StdRng,
}

impl PercEngine {
    fn new(sample_rate: f32) -> Self {
        Self {
            current_sample: 0,
            sample_rate,
            next_hit_sample: 0,
            hits: Vec::with_capacity(8),
            vol_lfo: DriftingLfo::new(0.2, sample_rate),
            rng: StdRng::from_entropy(),
        }
    }

    fn next(&mut self, c: &PercControls, bpm: f32) -> f32 {
        // Advance LFO every sample so phase accumulates at the correct rate.
        // lfo_rate is in bars; convert to Hz using current bpm
        let rate_hz = bpm / (240.0 * c.lfo_rate);
        let lfo_raw = self.vol_lfo.next(&mut self.rng, rate_hz * 0.5, rate_hz * 2.0);

        while self.current_sample >= self.next_hit_sample {
            let lfo_norm = normalized_lfo(lfo_raw);
            let effective_level = c.level * ((1.0 - c.lfo_depth) + lfo_norm * c.lfo_depth);
            let smoothing = 10_f32.powf(c.filter * 4.0 - 4.0);
            self.hits.push(NoiseHit::new(effective_level, c.decay_ms, smoothing, self.sample_rate));
            let step = (self.sample_rate * 60.0 / bpm / 4.0).round() as u64;
            self.next_hit_sample += step.max(1);
        }

        let mut out = 0.0f32;
        for h in &mut self.hits { out += h.next(&mut self.rng); }
        self.hits.retain(|h| !h.is_done());
        self.current_sample += 1;
        out
    }
}

struct NoiseHit {
    noise: WhiteNoise,
    samples_remaining: u64,
    total_samples: u64,
    level: f32,
    filter: f32,
}

impl NoiseHit {
    fn new(level: f32, decay_ms: f32, filter: f32, sample_rate: f32) -> Self {
        let total = (decay_ms * 0.001 * sample_rate).round() as u64;
        Self { noise: WhiteNoise::new(), samples_remaining: total, total_samples: total, level, filter }
    }
    fn next<R: Rng>(&mut self, rng: &mut R) -> f32 {
        if self.samples_remaining == 0 { return 0.0; }
        let gain = self.samples_remaining as f32 / self.total_samples as f32;
        self.samples_remaining -= 1;
        self.noise.next_filtered(rng, self.filter) * gain * self.level * 0.4
    }
    fn is_done(&self) -> bool { self.samples_remaining == 0 }
}

// ============================================================
// Kick engine
// ============================================================

struct KickEngine {
    current_sample: u64,
    sample_rate: f32,
    next_hit_sample: u64,
    voices: Vec<KickVoice>,
    delay: KickDelay,
    rng: StdRng,
}

impl KickEngine {
    fn new(sample_rate: f32) -> Self {
        Self {
            current_sample: 0,
            sample_rate,
            next_hit_sample: 0,
            voices: Vec::with_capacity(4),
            delay: KickDelay::new((sample_rate * 3.0) as usize),
            rng: StdRng::from_entropy(),
        }
    }

    fn next(&mut self, c: &KickControls, bpm: f32) -> (f32, f32) {
        while self.current_sample >= self.next_hit_sample {
            self.voices.push(KickVoice::new(c, self.sample_rate, &mut self.rng));
            let step = (self.sample_rate * 60.0 / bpm * c.interval_beats).round() as u64;
            self.next_hit_sample += step.max(1);
        }

        let mut dry_l = 0.0f32;
        let mut dry_r = 0.0f32;
        for v in &mut self.voices {
            let (l, r) = v.next(&mut self.rng);
            dry_l += l; dry_r += r;
        }
        self.voices.retain(|v| !v.is_done());

        let delay_samples = ((c.echo_time_beats * 60.0 / bpm) * self.sample_rate) as usize;
        let (echo_l, echo_r) = self.delay.process(dry_l, dry_r, delay_samples, c.echo_filter, c.echo_amount, c.echo_feedback);
        self.current_sample += 1;
        (dry_l + echo_l, dry_r + echo_r)
    }
}

struct KickDelay {
    buf_l: Vec<f32>,
    buf_r: Vec<f32>,
    head: usize,
    lp_l: f32,
    lp_r: f32,
    hp_l: f32,
    hp_r: f32,
}

impl KickDelay {
    fn new(max_samples: usize) -> Self {
        let n = max_samples.max(2);
        Self { buf_l: vec![0.0; n], buf_r: vec![0.0; n], head: 0, lp_l: 0.0, lp_r: 0.0, hp_l: 0.0, hp_r: 0.0 }
    }

    fn process(&mut self, in_l: f32, in_r: f32, delay_samples: usize, echo_filter: f32, echo_amount: f32, feedback: f32) -> (f32, f32) {
        let len = self.buf_l.len();
        let delay = delay_samples.clamp(1, len - 1);
        let read_pos = (self.head + len - delay) % len;

        // Wide band-pass: LP at ~2kHz centre, HP at ~60Hz, both gentle (one-pole).
        // echo_filter sweeps the LP cutoff from ~200Hz (0.0) to ~8kHz (1.0).
        let lp_coeff = 10_f32.powf(echo_filter * 3.6 - 2.3); // ~0.005..2.0 → clamp keeps it stable
        let lp_coeff = lp_coeff.clamp(0.001, 0.99);
        let hp_coeff = 0.97_f32; // fixed ~60 Hz high-pass, wide open

        self.lp_l += lp_coeff * (self.buf_l[read_pos] - self.lp_l);
        self.lp_r += lp_coeff * (self.buf_r[read_pos] - self.lp_r);
        let bp_l = self.lp_l - self.hp_l;
        let bp_r = self.lp_r - self.hp_r;
        self.hp_l = self.lp_l - bp_l * (1.0 - hp_coeff);
        self.hp_r = self.lp_r - bp_r * (1.0 - hp_coeff);

        self.buf_l[self.head] = in_l + bp_l * feedback;
        self.buf_r[self.head] = in_r + bp_r * feedback;
        self.head = (self.head + 1) % len;
        (bp_l * echo_amount, bp_r * echo_amount)
    }
}

struct KickVoice {
    phase: f32,
    freq: f32,
    target_freq: f32,
    freq_glide: f32,
    amp: f32,
    amp_decay: f32,
    click_remaining: u64,
    click_level: f32,
    drive: f32,
    pan: f32,
    sample_rate: f32,
}

impl KickVoice {
    fn new(c: &KickControls, sample_rate: f32, rng: &mut StdRng) -> Self {
        let tau = (c.pitch_decay_ms * 0.001 * sample_rate / 3.0).max(1.0);
        let amp_tau = (c.amp_decay_ms * 0.001 * sample_rate / 3.0).max(1.0);
        Self {
            phase: 0.0,
            freq: c.start_freq,
            target_freq: c.start_freq * 0.28,
            freq_glide: 1.0 / tau,
            amp: c.level,
            amp_decay: (-1.0 / amp_tau).exp(),
            click_remaining: (c.amp_decay_ms * 0.001 * sample_rate * 0.04).round() as u64,
            click_level: c.click,
            drive: c.drive,
            pan: rng.gen_range(-0.15f32..0.15),
            sample_rate,
        }
    }

    fn next<R: Rng>(&mut self, rng: &mut R) -> (f32, f32) {
        if self.amp < 0.0001 { return (0.0, 0.0); }

        self.freq += (self.target_freq - self.freq) * self.freq_glide;
        self.phase += TAU * self.freq / self.sample_rate;
        if self.phase >= TAU { self.phase -= TAU; }

        let mut s = self.phase.sin() * self.amp;

        if self.click_remaining > 0 {
            s += rng.gen_range(-1.0f32..1.0) * self.click_level * self.amp;
            self.click_remaining -= 1;
        }

        if self.drive > 0.0 {
            let driven = s * (1.0 + self.drive * 8.0);
            s = driven / (1.0 + driven.abs()) * (1.0 + self.drive * 0.5);
        }

        self.amp *= self.amp_decay;
        StereoPanner::equal_power(s, self.pan)
    }

    fn is_done(&self) -> bool { self.amp < 0.0001 }
}

// ============================================================
// Tonal engine (melodic step sequencer with randomness)
// ============================================================

struct TonalEngine {
    current_sample: u64,
    sample_rate: f32,
    next_step_sample: u64,
    step_index: usize,
    voices: Vec<TonalVoice>,
    reverb: Freeverb,
    rng: StdRng,
}

const SCALE_HZ: [f32; 10] = [
    110.0, 130.81, 146.83, 164.81, 196.0,
    220.0, 261.63, 293.66, 329.63, 392.0,
];
const PATTERN: [usize; 8] = [0, 2, 4, 1, 3, 5, 2, 4];

impl TonalEngine {
    fn new(sample_rate: f32) -> Self {
        Self {
            current_sample: 0,
            sample_rate,
            next_step_sample: 0,
            step_index: 0,
            voices: Vec::with_capacity(8),
            reverb: Freeverb::new(sample_rate, 0.86, 0.38, 0.9),
            rng: StdRng::from_entropy(),
        }
    }

    fn next(&mut self, c: &TonalControls, bpm: f32) -> (f32, f32) {
        while self.current_sample >= self.next_step_sample {
            let degree = if self.rng.gen_range(0.0f32..1.0) < c.randomness {
                self.rng.gen_range(0..SCALE_HZ.len())
            } else {
                let d = PATTERN[self.step_index % PATTERN.len()];
                self.step_index += 1;
                d
            };
            let hz = SCALE_HZ[degree];
            let decay_samples = (c.note_length * 60.0 / bpm * self.sample_rate).round() as u64;
            let pan = self.rng.gen_range(-0.5f32..0.5);
            self.voices.push(TonalVoice::new(hz, pan, c.level, decay_samples, self.sample_rate));
            let step = (c.step_interval * 60.0 / bpm * self.sample_rate).round() as u64;
            self.next_step_sample += step.max(1);
        }

        let mut dry_l = 0.0f32;
        let mut dry_r = 0.0f32;
        for v in &mut self.voices {
            let (l, r) = v.next();
            dry_l += l; dry_r += r;
        }
        self.voices.retain(|v| !v.is_done());

        let (wet_l, wet_r) = self.reverb.process(dry_l * c.reverb_mix, dry_r * c.reverb_mix);
        self.current_sample += 1;
        (dry_l * (1.0 - c.reverb_mix * 0.5) + wet_l, dry_r * (1.0 - c.reverb_mix * 0.5) + wet_r)
    }
}

struct TonalVoice {
    primary: SineOscillator,
    detuned: SineOscillator,
    samples_remaining: u64,
    total_samples: u64,
    pan: f32,
    level: f32,
}

impl TonalVoice {
    fn new(hz: f32, pan: f32, level: f32, decay_samples: u64, sample_rate: f32) -> Self {
        let total = decay_samples.max(1);
        Self {
            primary: SineOscillator::new(hz, sample_rate),
            detuned: SineOscillator::new(hz * 1.004, sample_rate),
            samples_remaining: total,
            total_samples: total,
            pan,
            level,
        }
    }
    fn next(&mut self) -> (f32, f32) {
        if self.samples_remaining == 0 { return (0.0, 0.0); }
        let gain = (self.samples_remaining as f32 / self.total_samples as f32).sqrt();
        self.samples_remaining -= 1;
        let s = soft_clip((self.primary.next() + self.detuned.next() * 0.3) * 0.4) * gain * self.level;
        StereoPanner::equal_power(s, self.pan)
    }
    fn is_done(&self) -> bool { self.samples_remaining == 0 }
}

// ============================================================
// Clap engine (multi-slap noise burst with room reverb)
// ============================================================

struct ClapEngine {
    current_sample: u64,
    sample_rate: f32,
    next_hit_sample: u64,
    voices: Vec<ClapVoice>,
    reverb: Freeverb,
    rng: StdRng,
}

impl ClapEngine {
    fn new(sample_rate: f32) -> Self {
        Self {
            current_sample: 0,
            sample_rate,
            next_hit_sample: 0,
            voices: Vec::with_capacity(4),
            reverb: Freeverb::new(sample_rate, 0.28, 0.62, 0.85),
            rng: StdRng::from_entropy(),
        }
    }

    fn next(&mut self, c: &ClapControls, bpm: f32) -> (f32, f32) {
        while self.current_sample >= self.next_hit_sample {
            self.voices.push(ClapVoice::new(c, self.sample_rate, &mut self.rng));
            let step = (self.sample_rate * 60.0 / bpm * c.interval_beats).round() as u64;
            self.next_hit_sample += step.max(1);
        }

        let mut dry = 0.0f32;
        for v in &mut self.voices { dry += v.next(&mut self.rng); }
        self.voices.retain(|v| !v.is_done());

        let (wet_l, wet_r) = self.reverb.process(dry * c.room, dry * c.room);
        self.current_sample += 1;
        let dry_scale = 1.0 - c.room * 0.5;
        (dry * dry_scale + wet_l, dry * dry_scale + wet_r)
    }
}

struct ClapVoice {
    noise: WhiteNoise,
    scheduled: Vec<u64>,
    bursts: Vec<ClapBurst>,
    current: u64,
    decay_samples: u64,
    filter_smoothing: f32,
    body_coeff: f32,
    body_state: f32,
    level: f32,
}

impl ClapVoice {
    fn new(c: &ClapControls, sample_rate: f32, rng: &mut StdRng) -> Self {
        let count = c.slap_count.round().max(1.0) as usize;
        let spread = (c.slap_spread_ms * 0.001 * sample_rate) as u64;
        let mut scheduled: Vec<u64> = (0..count)
            .map(|i| if i == 0 { 0 } else { rng.gen_range(0..=spread.max(1)) })
            .collect();
        scheduled.sort_unstable();
        Self {
            noise: WhiteNoise::new(),
            scheduled,
            bursts: Vec::new(),
            current: 0,
            decay_samples: (c.decay_ms * 0.001 * sample_rate).round() as u64,
            filter_smoothing: 10_f32.powf(c.filter * 4.0 - 4.0),
            body_coeff: c.body * 0.08,
            body_state: 0.0,
            level: c.level,
        }
    }

    fn next<R: Rng>(&mut self, rng: &mut R) -> f32 {
        self.scheduled.retain(|&t| {
            if self.current >= t {
                self.bursts.push(ClapBurst { remaining: self.decay_samples, total: self.decay_samples });
                false
            } else { true }
        });

        if self.bursts.is_empty() && self.scheduled.is_empty() { return 0.0; }

        let mut out = 0.0f32;
        for burst in &mut self.bursts {
            if burst.remaining > 0 {
                let env = (burst.remaining as f32 / burst.total as f32).sqrt();
                burst.remaining -= 1;
                let raw = self.noise.next_filtered(rng, self.filter_smoothing);
                self.body_state += self.body_coeff * (raw - self.body_state);
                out += (raw + self.body_state) * env;
            }
        }
        self.bursts.retain(|b| b.remaining > 0);

        self.current += 1;
        out * self.level * 0.35
    }

    fn is_done(&self) -> bool {
        self.scheduled.is_empty() && self.bursts.is_empty()
    }
}

struct ClapBurst { remaining: u64, total: u64 }

// ============================================================
// Shared utilities
// ============================================================


fn normalized_lfo(sample: f32) -> f32 {
    (sample * 0.5 + 0.5).clamp(0.0, 1.0)
}

fn soft_clip(sample: f32) -> f32 {
    sample / (1.0 + sample.abs())
}
