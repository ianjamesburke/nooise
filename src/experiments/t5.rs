use std::error::Error;
use std::f32::consts::TAU;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use arc_swap::ArcSwap;

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, Paragraph, Widget},
};

use crate::audio::{self, StereoEngine};
use crate::fx::lfo::DriftingLfo;
use crate::fx::panner::StereoPanner;
use crate::fx::reverb::Freeverb;
use crate::synth::envelope::Adsr;
use crate::synth::noise::WhiteNoise;
use crate::synth::oscillator::SineOscillator;

// ============================================================
// Telemetry — audio thread publishes, UI thread reads
// ============================================================

/// Lock-free channel from the engine to the visualizer. The audio thread only
/// ever stores; the UI thread only ever loads. `kick_pulse` is a monotonic
/// counter (UI tracks the delta to fire one ripple per hit); `chord_index`
/// mirrors the pad engine's current chord.
#[derive(Default)]
pub(crate) struct T5Telemetry {
    pub chord_index: AtomicU64,
    pub kick_pulse: AtomicU64,
}

// ============================================================
// Controls
// ============================================================

const MASTER_BPM_MIN: f32 = 60.0;
const MASTER_BPM_MAX: f32 = 200.0;
const KICK_ECHO_TIME_BEATS_MIN: f32 = 0.125;
const KICK_ECHO_TIME_BEATS_MAX: f32 = 2.0;

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
    pub lfo_rate_bars: f32,
    pub lfo_depth: f32,
}

impl Default for PercControls {
    fn default() -> Self {
        Self {
            level: 0.0,
            decay_ms: 80.0,
            filter: 0.8,
            lfo_rate_bars: 2.0,
            lfo_depth: 0.3,
        }
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
    pub click: f32, // 0–0.2 UI range
    pub drive: f32,
    pub interval_beats: f32,
    pub offset_beats: f32,
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
            offset_beats: 0.0,
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
    pub note_length_beats: f32,
    pub step_interval_beats: f32,
    pub offset_beats: f32,
    pub reverb_mix: f32,
}

impl Default for TonalControls {
    fn default() -> Self {
        Self {
            level: 0.0,
            randomness: 0.3,
            note_length_beats: 0.8,
            step_interval_beats: 1.0,
            offset_beats: 0.0,
            reverb_mix: 0.6,
        }
    }
}

#[derive(Clone)]
pub(crate) struct ClapControls {
    pub level: f32,
    pub interval_beats: f32,
    pub offset_beats: f32,
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
            interval_beats: 2.0,
            offset_beats: 1.0,
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

#[derive(Clone, Copy)]
pub(crate) enum UiVariant {
    T5a,
    T5b,
    T5c,
    T5d,
    T5e,
}

impl UiVariant {
    fn id(self) -> &'static str {
        match self {
            UiVariant::T5a => "t5a",
            UiVariant::T5b => "t5b",
            UiVariant::T5c => "t5c",
            UiVariant::T5d => "t5d",
            UiVariant::T5e => "t5e",
        }
    }

    fn name(self) -> &'static str {
        match self {
            UiVariant::T5a => "gauges",
            UiVariant::T5b => "orbit",
            UiVariant::T5c => "matrix",
            UiVariant::T5d => "score",
            UiVariant::T5e => "fluid",
        }
    }
}

pub(crate) fn run(variant: UiVariant) -> Result<(), Box<dyn Error>> {
    let controls = Arc::new(ArcSwap::from_pointee(T5Controls::default()));
    let controls_for_engine = Arc::clone(&controls);
    let telemetry = Arc::new(T5Telemetry::default());
    let telemetry_for_engine = Arc::clone(&telemetry);

    let _stream = audio::start_stream(variant.id(), move |sr| {
        T5Engine::new(sr, controls_for_engine, telemetry_for_engine)
    })?;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = ui_loop(&mut terminal, controls, telemetry, variant);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    result
}

// ============================================================
// UI
// ============================================================

#[derive(Clone, Copy, Debug, PartialEq)]
enum Tab {
    Master = 0,
    Perc = 1,
    Chords = 2,
    Kick = 3,
    Tonal = 4,
    Clap = 5,
}

impl Tab {
    fn all() -> [Tab; 6] {
        [
            Tab::Master,
            Tab::Perc,
            Tab::Chords,
            Tab::Kick,
            Tab::Tonal,
            Tab::Clap,
        ]
    }

    fn name(self) -> &'static str {
        match self {
            Tab::Master => "Master",
            Tab::Perc => "Perc",
            Tab::Chords => "Chords",
            Tab::Kick => "Kick",
            Tab::Tonal => "Tonal",
            Tab::Clap => "Clap",
        }
    }

    fn short_name(self) -> &'static str {
        match self {
            Tab::Master => "MST",
            Tab::Perc => "PRC",
            Tab::Chords => "CHD",
            Tab::Kick => "KIK",
            Tab::Tonal => "TON",
            Tab::Clap => "CLP",
        }
    }

    fn next(self) -> Self {
        match self {
            Tab::Master => Tab::Perc,
            Tab::Perc => Tab::Chords,
            Tab::Chords => Tab::Kick,
            Tab::Kick => Tab::Tonal,
            Tab::Tonal => Tab::Clap,
            Tab::Clap => Tab::Master,
        }
    }

    fn previous(self) -> Self {
        match self {
            Tab::Master => Tab::Clap,
            Tab::Perc => Tab::Master,
            Tab::Chords => Tab::Perc,
            Tab::Kick => Tab::Chords,
            Tab::Tonal => Tab::Kick,
            Tab::Clap => Tab::Tonal,
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
            ControlItem {
                label: "Chords Vol",
                value: c.pad.level,
                min: 0.0,
                max: 1.0,
                display: format!("{:.0}%", c.pad.level * 100.0),
            },
            ControlItem {
                label: "Perc Vol",
                value: c.perc.level,
                min: 0.0,
                max: 1.0,
                display: format!("{:.0}%", c.perc.level * 100.0),
            },
            ControlItem {
                label: "Kick Vol",
                value: c.kick.level,
                min: 0.0,
                max: 1.0,
                display: format!("{:.0}%", c.kick.level * 100.0),
            },
            ControlItem {
                label: "Tonal Vol",
                value: c.tonal.level,
                min: 0.0,
                max: 1.0,
                display: format!("{:.0}%", c.tonal.level * 100.0),
            },
            ControlItem {
                label: "Clap Vol",
                value: c.clap.level,
                min: 0.0,
                max: 1.0,
                display: format!("{:.0}%", c.clap.level * 100.0),
            },
            ControlItem {
                label: "BPM",
                value: c.master.bpm,
                min: MASTER_BPM_MIN,
                max: MASTER_BPM_MAX,
                display: format!("{:.0} bpm", c.master.bpm),
            },
            ControlItem {
                label: "Master Level",
                value: c.master.level,
                min: 0.0,
                max: 1.0,
                display: format!("{:.0}%", c.master.level * 100.0),
            },
            ControlItem {
                label: "Drive",
                value: c.master.drive,
                min: 0.0,
                max: 1.0,
                display: format!("{:.0}%", c.master.drive * 100.0),
            },
            ControlItem {
                label: "Comp Threshold",
                value: c.master.comp_threshold,
                min: -40.0,
                max: 0.0,
                display: format!("{:.0} dB", c.master.comp_threshold),
            },
            ControlItem {
                label: "Comp Ratio",
                value: c.master.comp_ratio,
                min: 1.0,
                max: 8.0,
                display: format!("{:.1}:1", c.master.comp_ratio),
            },
            ControlItem {
                label: "Comp Release",
                value: c.master.comp_release_ms,
                min: 10.0,
                max: 500.0,
                display: format!("{:.0} ms", c.master.comp_release_ms),
            },
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
            ControlItem {
                label: "Level",
                value: c.perc.level,
                min: 0.0,
                max: 1.0,
                display: format!("{:.0}%", c.perc.level * 100.0),
            },
            ControlItem {
                label: "Decay",
                value: c.perc.decay_ms,
                min: 20.0,
                max: 2000.0,
                display: if c.perc.decay_ms >= 1000.0 {
                    format!("{:.1} s", c.perc.decay_ms / 1000.0)
                } else {
                    format!("{:.0} ms", c.perc.decay_ms)
                },
            },
            ControlItem {
                label: "Filter",
                value: c.perc.filter,
                min: 0.5,
                max: 1.0,
                display: format!("{:.0}%", c.perc.filter * 100.0),
            },
            ControlItem {
                label: "LFO Rate",
                value: c.perc.lfo_rate_bars,
                min: 0.25,
                max: 16.0,
                display: format!("{:.0} beats", c.perc.lfo_rate_bars * 4.0),
            },
            ControlItem {
                label: "LFO Depth",
                value: c.perc.lfo_depth,
                min: 0.0,
                max: 1.0,
                display: format!("{:.0}%", c.perc.lfo_depth * 100.0),
            },
        ],
        Tab::Chords => vec![
            ControlItem {
                label: "Level",
                value: c.pad.level,
                min: 0.0,
                max: 1.0,
                display: format!("{:.0}%", c.pad.level * 100.0),
            },
            ControlItem {
                label: "Chord Length",
                value: c.pad.chord_bars.log2(),
                min: 0.0,
                max: 6.0,
                display: format!("{:.0} beats", c.pad.chord_bars * 4.0),
            },
            ControlItem {
                label: "Reverb Mix",
                value: c.pad.reverb_mix,
                min: 0.0,
                max: 1.0,
                display: format!("{:.0}%", c.pad.reverb_mix * 100.0),
            },
            ControlItem {
                label: "Stereo Width",
                value: c.pad.stereo_width,
                min: 0.0,
                max: 1.0,
                display: format!("{:.0}%", c.pad.stereo_width * 100.0),
            },
            ControlItem {
                label: "Detune",
                value: c.pad.detune,
                min: 0.0,
                max: 1.0,
                display: format!("{:.0}%", c.pad.detune * 100.0),
            },
            ControlItem {
                label: "Octave Mix",
                value: c.pad.octave_mix,
                min: 0.0,
                max: 1.0,
                display: format!("{:.0}%", c.pad.octave_mix * 100.0),
            },
            ControlItem {
                label: "Attack",
                value: c.pad.attack_time,
                min: 1.0,
                max: 30.0,
                display: format!("{:.1} s", c.pad.attack_time),
            },
        ],
        Tab::Kick => vec![
            ControlItem {
                label: "Level",
                value: c.kick.level,
                min: 0.0,
                max: 1.0,
                display: format!("{:.0}%", c.kick.level * 100.0),
            },
            ControlItem {
                label: "Start Freq",
                value: c.kick.start_freq,
                min: 40.0,
                max: 200.0,
                display: format!("{:.0} Hz", c.kick.start_freq),
            },
            ControlItem {
                label: "Pitch Decay",
                value: c.kick.pitch_decay_ms,
                min: 10.0,
                max: 300.0,
                display: format!("{:.0} ms", c.kick.pitch_decay_ms),
            },
            ControlItem {
                label: "Amp Decay",
                value: c.kick.amp_decay_ms,
                min: 50.0,
                max: 1000.0,
                display: format!("{:.0} ms", c.kick.amp_decay_ms),
            },
            ControlItem {
                label: "Click",
                value: c.kick.click,
                min: 0.0,
                max: 0.2,
                display: format!("{:.0}%", c.kick.click / 0.2 * 100.0),
            },
            ControlItem {
                label: "Drive",
                value: c.kick.drive,
                min: 0.0,
                max: 1.0,
                display: format!("{:.0}%", c.kick.drive * 100.0),
            },
            ControlItem {
                label: "Interval",
                value: c.kick.interval_beats,
                min: 0.5,
                max: 4.0,
                display: format!("{:.2} beats", c.kick.interval_beats),
            },
            ControlItem {
                label: "Offset",
                value: c.kick.offset_beats,
                min: 0.0,
                max: 4.0,
                display: format!("{:.2} beats", c.kick.offset_beats),
            },
            ControlItem {
                label: "Echo Time",
                value: c.kick.echo_time_beats,
                min: KICK_ECHO_TIME_BEATS_MIN,
                max: KICK_ECHO_TIME_BEATS_MAX,
                display: format!("{:.3} beats", c.kick.echo_time_beats),
            },
            ControlItem {
                label: "Echo Filter",
                value: c.kick.echo_filter,
                min: 0.0,
                max: 1.0,
                display: format!("{:.0}%", c.kick.echo_filter * 100.0),
            },
            ControlItem {
                label: "Echo Amount",
                value: c.kick.echo_amount,
                min: 0.0,
                max: 0.9,
                display: format!("{:.0}%", c.kick.echo_amount / 0.9 * 100.0),
            },
            ControlItem {
                label: "Echo Feedback",
                value: c.kick.echo_feedback,
                min: 0.0,
                max: 0.85,
                display: format!("{:.0}%", c.kick.echo_feedback / 0.85 * 100.0),
            },
        ],
        Tab::Tonal => vec![
            ControlItem {
                label: "Level",
                value: c.tonal.level,
                min: 0.0,
                max: 1.0,
                display: format!("{:.0}%", c.tonal.level * 100.0),
            },
            ControlItem {
                label: "Randomness",
                value: c.tonal.randomness,
                min: 0.0,
                max: 1.0,
                display: format!("{:.0}%", c.tonal.randomness * 100.0),
            },
            ControlItem {
                label: "Note Length",
                value: c.tonal.note_length_beats,
                min: 0.1,
                max: 2.0,
                display: format!("{:.2} beats", c.tonal.note_length_beats),
            },
            ControlItem {
                label: "Interval",
                value: c.tonal.step_interval_beats,
                min: 0.5,
                max: 4.0,
                display: format!("{:.2} beats", c.tonal.step_interval_beats),
            },
            ControlItem {
                label: "Offset",
                value: c.tonal.offset_beats,
                min: 0.0,
                max: 4.0,
                display: format!("{:.2} beats", c.tonal.offset_beats),
            },
            ControlItem {
                label: "Reverb Mix",
                value: c.tonal.reverb_mix,
                min: 0.0,
                max: 1.0,
                display: format!("{:.0}%", c.tonal.reverb_mix * 100.0),
            },
        ],
        Tab::Clap => vec![
            ControlItem {
                label: "Level",
                value: c.clap.level,
                min: 0.0,
                max: 1.0,
                display: format!("{:.0}%", c.clap.level * 100.0),
            },
            ControlItem {
                label: "Interval",
                value: c.clap.interval_beats,
                min: 0.5,
                max: 8.0,
                display: format!("{:.2} beats", c.clap.interval_beats),
            },
            ControlItem {
                label: "Offset",
                value: c.clap.offset_beats,
                min: 0.0,
                max: 8.0,
                display: format!("{:.2} beats", c.clap.offset_beats),
            },
            ControlItem {
                label: "Slap Count",
                value: c.clap.slap_count,
                min: 1.0,
                max: 8.0,
                display: format!("{:.0}", c.clap.slap_count),
            },
            ControlItem {
                label: "Slap Spread",
                value: c.clap.slap_spread_ms,
                min: 0.0,
                max: 100.0,
                display: format!("{:.1} ms", c.clap.slap_spread_ms),
            },
            ControlItem {
                label: "Decay",
                value: c.clap.decay_ms,
                min: 10.0,
                max: 200.0,
                display: format!("{:.0} ms", c.clap.decay_ms),
            },
            ControlItem {
                label: "Filter",
                value: c.clap.filter,
                min: 0.5,
                max: 1.0,
                display: format!("{:.0}%", c.clap.filter * 100.0),
            },
            ControlItem {
                label: "Room",
                value: c.clap.room,
                min: 0.0,
                max: 1.0,
                display: format!("{:.0}%", c.clap.room * 100.0),
            },
            ControlItem {
                label: "Body",
                value: c.clap.body,
                min: 0.0,
                max: 1.0,
                display: format!("{:.0}%", c.clap.body * 100.0),
            },
        ],
    }
}

fn apply_delta(tab: Tab, selected: usize, dir: f32, c: &mut T5Controls) {
    match tab {
        Tab::Master => match selected {
            0 => c.pad.level = (c.pad.level + dir * 0.02).clamp(0.0, 1.0),
            1 => c.perc.level = (c.perc.level + dir * 0.02).clamp(0.0, 1.0),
            2 => c.kick.level = (c.kick.level + dir * 0.02).clamp(0.0, 1.0),
            3 => c.tonal.level = (c.tonal.level + dir * 0.02).clamp(0.0, 1.0),
            4 => c.clap.level = (c.clap.level + dir * 0.02).clamp(0.0, 1.0),
            5 => c.master.bpm = (c.master.bpm + dir * 2.0).clamp(MASTER_BPM_MIN, MASTER_BPM_MAX),
            6 => c.master.level = (c.master.level + dir * 0.02).clamp(0.0, 1.0),
            7 => c.master.drive = (c.master.drive + dir * 0.02).clamp(0.0, 1.0),
            8 => c.master.comp_threshold = (c.master.comp_threshold + dir * 1.0).clamp(-40.0, 0.0),
            9 => c.master.comp_ratio = (c.master.comp_ratio + dir * 0.25).clamp(1.0, 8.0),
            10 => {
                c.master.comp_release_ms =
                    (c.master.comp_release_ms + dir * 10.0).clamp(10.0, 500.0)
            }
            11 => c.master.tone = (c.master.tone + dir * 0.05).clamp(-1.0, 1.0),
            _ => {}
        },
        Tab::Perc => match selected {
            0 => c.perc.level = (c.perc.level + dir * 0.02).clamp(0.0, 1.0),
            1 => c.perc.decay_ms = (c.perc.decay_ms + dir * 20.0).clamp(20.0, 2000.0),
            2 => c.perc.filter = (c.perc.filter + dir * 0.02).clamp(0.5, 1.0),
            3 => c.perc.lfo_rate_bars = (c.perc.lfo_rate_bars + dir * 0.25).clamp(0.25, 16.0),
            4 => c.perc.lfo_depth = (c.perc.lfo_depth + dir * 0.02).clamp(0.0, 1.0),
            _ => {}
        },
        Tab::Chords => match selected {
            0 => c.pad.level = (c.pad.level + dir * 0.02).clamp(0.0, 1.0),
            1 => {
                if dir > 0.0 {
                    c.pad.chord_bars = (c.pad.chord_bars * 2.0).min(64.0)
                } else {
                    c.pad.chord_bars = (c.pad.chord_bars / 2.0).max(1.0)
                }
            }
            2 => c.pad.reverb_mix = (c.pad.reverb_mix + dir * 0.02).clamp(0.0, 1.0),
            3 => c.pad.stereo_width = (c.pad.stereo_width + dir * 0.02).clamp(0.0, 1.0),
            4 => c.pad.detune = (c.pad.detune + dir * 0.02).clamp(0.0, 1.0),
            5 => c.pad.octave_mix = (c.pad.octave_mix + dir * 0.02).clamp(0.0, 1.0),
            6 => c.pad.attack_time = (c.pad.attack_time + dir * 1.0).clamp(1.0, 30.0),
            _ => {}
        },
        Tab::Kick => match selected {
            0 => c.kick.level = (c.kick.level + dir * 0.02).clamp(0.0, 1.0),
            1 => c.kick.start_freq = (c.kick.start_freq + dir * 5.0).clamp(40.0, 200.0),
            2 => c.kick.pitch_decay_ms = (c.kick.pitch_decay_ms + dir * 5.0).clamp(10.0, 300.0),
            3 => c.kick.amp_decay_ms = (c.kick.amp_decay_ms + dir * 20.0).clamp(50.0, 1000.0),
            4 => c.kick.click = (c.kick.click + dir * 0.01).clamp(0.0, 0.2),
            5 => c.kick.drive = (c.kick.drive + dir * 0.02).clamp(0.0, 1.0),
            6 => c.kick.interval_beats = (c.kick.interval_beats + dir * 0.25).clamp(0.5, 4.0),
            7 => c.kick.offset_beats = (c.kick.offset_beats + dir * 0.25).clamp(0.0, 4.0),
            8 => {
                c.kick.echo_time_beats = (c.kick.echo_time_beats + dir * 0.125)
                    .clamp(KICK_ECHO_TIME_BEATS_MIN, KICK_ECHO_TIME_BEATS_MAX)
            }
            9 => c.kick.echo_filter = (c.kick.echo_filter + dir * 0.02).clamp(0.0, 1.0),
            10 => c.kick.echo_amount = (c.kick.echo_amount + dir * 0.02).clamp(0.0, 0.9),
            11 => c.kick.echo_feedback = (c.kick.echo_feedback + dir * 0.02).clamp(0.0, 0.85),
            _ => {}
        },
        Tab::Tonal => match selected {
            0 => c.tonal.level = (c.tonal.level + dir * 0.02).clamp(0.0, 1.0),
            1 => c.tonal.randomness = (c.tonal.randomness + dir * 0.02).clamp(0.0, 1.0),
            2 => {
                c.tonal.note_length_beats = (c.tonal.note_length_beats + dir * 0.05).clamp(0.1, 2.0)
            }
            3 => {
                c.tonal.step_interval_beats =
                    (c.tonal.step_interval_beats + dir * 0.25).clamp(0.5, 4.0)
            }
            4 => c.tonal.offset_beats = (c.tonal.offset_beats + dir * 0.25).clamp(0.0, 4.0),
            5 => c.tonal.reverb_mix = (c.tonal.reverb_mix + dir * 0.02).clamp(0.0, 1.0),
            _ => {}
        },
        Tab::Clap => match selected {
            0 => c.clap.level = (c.clap.level + dir * 0.02).clamp(0.0, 1.0),
            1 => c.clap.interval_beats = (c.clap.interval_beats + dir * 0.25).clamp(0.5, 8.0),
            2 => c.clap.offset_beats = (c.clap.offset_beats + dir * 0.25).clamp(0.0, 8.0),
            3 => c.clap.slap_count = (c.clap.slap_count + dir * 1.0).clamp(1.0, 8.0),
            4 => c.clap.slap_spread_ms = (c.clap.slap_spread_ms + dir * 2.0).clamp(0.0, 100.0),
            5 => c.clap.decay_ms = (c.clap.decay_ms + dir * 5.0).clamp(10.0, 200.0),
            6 => c.clap.filter = (c.clap.filter + dir * 0.02).clamp(0.5, 1.0),
            7 => c.clap.room = (c.clap.room + dir * 0.02).clamp(0.0, 1.0),
            8 => c.clap.body = (c.clap.body + dir * 0.02).clamp(0.0, 1.0),
            _ => {}
        },
    }
}

fn ui_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    controls: Arc<ArcSwap<T5Controls>>,
    telemetry: Arc<T5Telemetry>,
    variant: UiVariant,
) -> Result<(), Box<dyn Error>> {
    let mut tab = Tab::Master;
    let mut selected = 0usize;
    let mut fluid = FluidState::new();
    let mut last = Instant::now();

    loop {
        let c = T5Controls::clone(&controls.load());
        let items = tab_controls(tab, &c);
        let items_len = items.len();
        selected = selected.min(items_len.saturating_sub(1));

        let now = Instant::now();
        let dt = (now - last).as_secs_f32().min(0.05);
        last = now;
        fluid.tick(dt, &telemetry);

        terminal.draw(|f| render(f, variant, &c, &items, tab, selected, &fluid))?;

        if event::poll(std::time::Duration::from_millis(16))?
            && let Event::Key(key) = event::read()?
        {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match key.code {
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
                KeyCode::Left | KeyCode::Char('h') => adjust(&controls, tab, selected, -1.0),
                KeyCode::Right | KeyCode::Char('l') => adjust(&controls, tab, selected, 1.0),
                _ => {}
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

fn render(
    f: &mut Frame,
    variant: UiVariant,
    controls: &T5Controls,
    items: &[ControlItem],
    active_tab: Tab,
    selected: usize,
    fluid: &FluidState,
) {
    match variant {
        UiVariant::T5a => render_t5a(f, variant, items, active_tab, selected),
        UiVariant::T5b => render_t5b(f, variant, controls, items, active_tab, selected),
        UiVariant::T5c => render_t5c(f, variant, controls, items, active_tab, selected),
        UiVariant::T5d => render_t5d(f, variant, controls, items, active_tab, selected),
        UiVariant::T5e => render_t5e(f, variant, items, active_tab, selected, fluid),
    }
}

// ============================================================
// Fluid visualizer (t5e) — chords drive the field colour, kicks
// spawn ripples. Driven entirely by live audio-thread telemetry.
// ============================================================

const FLUID_GRADIENT: &[char] = &[' ', '·', '∙', '•', '●', '◉', '⬤'];
const RIPPLE_LIFETIME: f32 = 3.0;
const RIPPLE_SPEED: f32 = 0.42; // normalized units / s

/// One chord = one hue. Cycles with the pad engine's 5-chord table.
fn hue_for_chord(index: u64) -> f32 {
    const HUES: [f32; 5] = [205.0, 270.0, 325.0, 158.0, 38.0];
    HUES[(index % HUES.len() as u64) as usize]
}

struct FluidState {
    t: f32,
    ripples: Vec<(f32, f32, f32)>, // (cx, cy, age) in 0..1 field coords
    last_kick: u64,
    hue: f32,
}

impl FluidState {
    fn new() -> Self {
        Self {
            t: 0.0,
            ripples: Vec::new(),
            last_kick: 0,
            hue: hue_for_chord(0),
        }
    }

    fn tick(&mut self, dt: f32, telemetry: &T5Telemetry) {
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
    fn field(&self, nx: f32, ny: f32) -> f32 {
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

fn fluid_hsv(h: f32, s: f32, v: f32) -> Color {
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

struct FluidWidget<'a> {
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
fn darken(c: Color, factor: f32) -> Color {
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

fn render_t5e(
    f: &mut Frame,
    variant: UiVariant,
    items: &[ControlItem],
    active_tab: Tab,
    selected: usize,
    fluid: &FluidState,
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
        .title(format!(" {} {} ", variant.id(), variant.name()))
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
        Paragraph::new(tab_line)
            .alignment(Alignment::Center)
            .style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
        layout[2],
    );

    // One text row per control, blank line between for vertical breathing room.
    let bar_w = (inner.width as usize).saturating_sub(34).clamp(6, 80);
    let mut rows: Vec<Line> = Vec::with_capacity(items.len() * 2);
    for (i, item) in items.iter().enumerate() {
        let active = i == selected;
        let bar = ratio_bar(item_ratio(item), bar_w, '█', '░');
        let prefix = if active { "▶ " } else { "  " };
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
            format!("{prefix}{:<15} {bar} {}", item.label, item.display),
            style,
        )));
        if i + 1 < items.len() {
            rows.push(Line::from(""));
        }
    }
    f.render_widget(Paragraph::new(rows), layout[4]);

    f.render_widget(
        Paragraph::new("jk select   hl adjust   Tab layer   q quit")
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::Rgb(120, 128, 145))),
        layout[5],
    );
}

fn render_t5a(
    f: &mut Frame,
    variant: UiVariant,
    items: &[ControlItem],
    active_tab: Tab,
    selected: usize,
) {
    let area = f.area();

    let outer = Block::default()
        .title(format!(" {} {} ", variant.id(), variant.name()))
        .borders(Borders::ALL);
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
        .map(|t| {
            if *t == active_tab {
                format!("[{}]", t.name())
            } else {
                t.name().to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("  |  ");
    let tab_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    f.render_widget(Paragraph::new(tab_line).style(tab_style), layout[0]);

    let gauge_height = 2u16;
    let constraints: Vec<Constraint> = items
        .iter()
        .map(|_| Constraint::Length(gauge_height))
        .collect();
    let slots = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(layout[2]);

    for (i, item) in items.iter().enumerate() {
        if i >= slots.len() {
            break;
        }
        let active = i == selected;
        let style = if active {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let ratio = ((item.value - item.min) / (item.max - item.min)).clamp(0.0, 1.0) as f64;
        let prefix = if active { "▶ " } else { "  " };
        let gauge = Gauge::default()
            .block(
                Block::default().title(format!("{}{:<14}  {}", prefix, item.label, item.display)),
            )
            .gauge_style(style)
            .ratio(ratio);
        f.render_widget(gauge, slots[i]);
    }

    f.render_widget(
        Paragraph::new("↑↓/jk select   ←→/hl adjust   Tab/Shift+Tab layer   q quit"),
        layout[3],
    );
}

fn render_t5b(
    f: &mut Frame,
    variant: UiVariant,
    controls: &T5Controls,
    items: &[ControlItem],
    active_tab: Tab,
    selected: usize,
) {
    let area = f.area();
    let outer = Block::default()
        .title(format!(" {} {} ", variant.id(), variant.name()))
        .borders(Borders::ALL);
    let inner = outer.inner(area);
    f.render_widget(outer, area);

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(5),
            Constraint::Length(1),
        ])
        .split(inner);

    render_variant_header(f, layout[0], variant, controls, active_tab);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
        .split(layout[1]);

    f.render_widget(
        Paragraph::new(orbit_lines(controls, active_tab)).block(Block::default().title(" layers ")),
        body[0],
    );
    f.render_widget(
        Paragraph::new(spoke_lines(items, selected)).block(Block::default().title(" spokes ")),
        body[1],
    );
    render_focus_panel(f, layout[2], active_tab, items, selected);
    render_footer(
        f,
        layout[3],
        "Tab/Shift+Tab orbit   jk spoke   hl bend   q quit",
    );
}

fn render_t5c(
    f: &mut Frame,
    variant: UiVariant,
    controls: &T5Controls,
    items: &[ControlItem],
    active_tab: Tab,
    selected: usize,
) {
    let area = f.area();
    let outer = Block::default()
        .title(format!(" {} {} ", variant.id(), variant.name()))
        .borders(Borders::ALL);
    let inner = outer.inner(area);
    f.render_widget(outer, area);

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(5),
            Constraint::Length(1),
        ])
        .split(inner);

    render_variant_header(f, layout[0], variant, controls, active_tab);
    f.render_widget(
        Paragraph::new(matrix_lines(controls, active_tab, selected))
            .block(Block::default().title(" all controls ")),
        layout[1],
    );
    render_focus_panel(f, layout[2], active_tab, items, selected);
    render_footer(
        f,
        layout[3],
        "Tab/Shift+Tab column   jk row   hl value   q quit",
    );
}

fn render_t5d(
    f: &mut Frame,
    variant: UiVariant,
    controls: &T5Controls,
    items: &[ControlItem],
    active_tab: Tab,
    selected: usize,
) {
    let area = f.area();
    let outer = Block::default()
        .title(format!(" {} {} ", variant.id(), variant.name()))
        .borders(Borders::ALL);
    let inner = outer.inner(area);
    f.render_widget(outer, area);

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(5),
            Constraint::Length(1),
        ])
        .split(inner);

    render_variant_header(f, layout[0], variant, controls, active_tab);
    f.render_widget(
        Paragraph::new(score_lines(controls, active_tab))
            .block(Block::default().title(" eight beat score ")),
        layout[1],
    );
    render_focus_panel(f, layout[2], active_tab, items, selected);
    render_footer(
        f,
        layout[3],
        "Tab/Shift+Tab staff   jk parameter   hl conduct   q quit",
    );
}

fn render_variant_header(
    f: &mut Frame,
    area: Rect,
    variant: UiVariant,
    controls: &T5Controls,
    active_tab: Tab,
) {
    let status = Line::from(vec![
        Span::styled(
            format!("{} {}", variant.id(), variant.name()),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("   "),
        Span::styled(
            format!("{:.0} bpm", controls.master.bpm),
            Style::default().fg(Color::Cyan),
        ),
        Span::raw("   "),
        Span::styled(
            format!(
                "master {:.0}%  drive {:.0}%  tone {}",
                controls.master.level * 100.0,
                controls.master.drive * 100.0,
                tone_label(controls.master.tone)
            ),
            Style::default().fg(Color::Gray),
        ),
    ]);
    f.render_widget(
        Paragraph::new(vec![status, tab_selector_line(active_tab)]).alignment(Alignment::Center),
        area,
    );
}

fn render_footer(f: &mut Frame, area: Rect, text: &'static str) {
    f.render_widget(Paragraph::new(text), area);
}

fn render_focus_panel(
    f: &mut Frame,
    area: Rect,
    active_tab: Tab,
    items: &[ControlItem],
    selected: usize,
) {
    let Some(item) = items.get(selected) else {
        return;
    };
    let ratio = item_ratio(item);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(1),
        ])
        .split(area);
    let headline = Paragraph::new(vec![Line::from(vec![
        Span::styled(
            format!("{} / ", active_tab.name()),
            Style::default().fg(layer_color(active_tab)),
        ),
        Span::styled(
            item.label,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(item.display.clone(), Style::default().fg(Color::Cyan)),
    ])])
    .block(Block::default().title(" focus "));
    f.render_widget(headline, chunks[0]);
    f.render_widget(
        Gauge::default()
            .gauge_style(
                Style::default()
                    .fg(layer_color(active_tab))
                    .add_modifier(Modifier::BOLD),
            )
            .ratio(ratio as f64),
        chunks[1],
    );
}

fn tab_selector_line(active_tab: Tab) -> Line<'static> {
    let mut spans = Vec::new();
    for tab in Tab::all() {
        if !spans.is_empty() {
            spans.push(Span::raw("  "));
        }
        let label = if tab == active_tab {
            format!("[{}]", tab.short_name())
        } else {
            format!(" {} ", tab.short_name())
        };
        let style = if tab == active_tab {
            Style::default()
                .fg(layer_color(tab))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        spans.push(Span::styled(label, style));
    }
    Line::from(spans)
}

fn orbit_lines(controls: &T5Controls, active_tab: Tab) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled(
        "        volume orbit        motion orbit",
        Style::default().fg(Color::DarkGray),
    )));
    lines.push(Line::from(""));

    for tab in Tab::all() {
        let active = tab == active_tab;
        let marker = if active { ">>" } else { "  " };
        let name_style = if active {
            Style::default()
                .fg(layer_color(tab))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(layer_color(tab))
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{marker} {:<7}", tab.name()), name_style),
            Span::raw(ratio_bar(tab_level(tab, controls), 18, '#', '.')),
            Span::raw("   "),
            Span::raw(ratio_bar(tab_motion(tab, controls), 12, '=', '.')),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("core ", Style::default().fg(Color::Yellow)),
        Span::raw(format!(
            "{:.0} bpm / comp {:.1}:1 / release {:.0}ms",
            controls.master.bpm, controls.master.comp_ratio, controls.master.comp_release_ms
        )),
    ]));
    lines
}

fn spoke_lines(items: &[ControlItem], selected: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let active = i == selected;
        let marker = if active { "@" } else { "." };
        let style = if active {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{marker} {:02} ", i + 1), style),
            Span::styled(format!("{:<18}", item.label), style),
            Span::raw(ratio_bar(item_ratio(item), 16, '#', '.')),
            Span::raw(" "),
            Span::styled(item.display.clone(), Style::default().fg(Color::White)),
        ]));
    }
    lines
}

fn matrix_lines(controls: &T5Controls, active_tab: Tab, selected: usize) -> Vec<Line<'static>> {
    let all_controls: Vec<(Tab, Vec<ControlItem>)> = Tab::all()
        .into_iter()
        .map(|tab| (tab, tab_controls(tab, controls)))
        .collect();
    let row_count = all_controls
        .iter()
        .map(|(_, controls)| controls.len())
        .max()
        .unwrap_or(0);

    let mut lines = Vec::new();
    let mut header = vec![Span::raw("    ")];
    for (tab, _) in &all_controls {
        let style = if *tab == active_tab {
            Style::default()
                .fg(layer_color(*tab))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(layer_color(*tab))
        };
        header.push(Span::styled(format!("{:^10}", tab.short_name()), style));
    }
    lines.push(Line::from(header));

    for row in 0..row_count {
        let mut spans = vec![Span::styled(
            format!("{:02}  ", row + 1),
            Style::default().fg(Color::DarkGray),
        )];
        for (tab, tab_items) in &all_controls {
            if let Some(item) = tab_items.get(row) {
                let active = *tab == active_tab && row == selected;
                let bar = ratio_bar(item_ratio(item), 6, '#', '.');
                let cell = if active {
                    format!("[{bar}] ")
                } else {
                    format!(" {bar}  ")
                };
                let style = if active {
                    Style::default()
                        .fg(Color::White)
                        .bg(layer_color(*tab))
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(layer_color(*tab))
                };
                spans.push(Span::styled(format!("{cell:<10}"), style));
            } else {
                spans.push(Span::raw("          "));
            }
        }
        lines.push(Line::from(spans));
    }
    lines
}

fn score_lines(controls: &T5Controls, active_tab: Tab) -> Vec<Line<'static>> {
    const STEPS: usize = 32;
    const STEP_BEATS: f32 = 0.25;

    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("beats ", Style::default().fg(Color::DarkGray)),
        Span::raw("1...2...3...4...5...6...7...8..."),
    ]));
    lines.push(Line::from(""));

    for tab in Tab::all() {
        let active = tab == active_tab;
        let pattern = match tab {
            Tab::Master => "||||::::||||::::||||::::||||::::".to_string(),
            Tab::Perc => pulse_pattern(0.25, 0.0, STEPS, STEP_BEATS, '#', '.'),
            Tab::Chords => pulse_pattern(
                controls.pad.chord_bars * 4.0,
                0.0,
                STEPS,
                STEP_BEATS,
                'O',
                '~',
            ),
            Tab::Kick => pulse_pattern(
                controls.kick.interval_beats,
                controls.kick.offset_beats,
                STEPS,
                STEP_BEATS,
                'K',
                '.',
            ),
            Tab::Tonal => pulse_pattern(
                controls.tonal.step_interval_beats,
                controls.tonal.offset_beats,
                STEPS,
                STEP_BEATS,
                'T',
                '.',
            ),
            Tab::Clap => pulse_pattern(
                controls.clap.interval_beats,
                controls.clap.offset_beats,
                STEPS,
                STEP_BEATS,
                'C',
                '.',
            ),
        };
        let style = if active {
            Style::default()
                .fg(layer_color(tab))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let marker = if active { ">" } else { " " };
        lines.push(Line::from(vec![
            Span::styled(format!("{marker} {:<7}", tab.name()), style),
            Span::styled(pattern, style),
            Span::raw("  "),
            Span::styled(
                ratio_bar(tab_level(tab, controls), 10, '#', '.'),
                Style::default().fg(layer_color(tab)),
            ),
        ]));
    }
    lines
}

fn pulse_pattern(
    interval_beats: f32,
    offset_beats: f32,
    steps: usize,
    step_beats: f32,
    hit: char,
    rest: char,
) -> String {
    (0..steps)
        .map(|i| {
            let beat = i as f32 * step_beats;
            if grid_hit_at_step(beat, interval_beats, offset_beats, step_beats * 0.45) {
                hit
            } else {
                rest
            }
        })
        .collect()
}

fn grid_hit_at_step(beat: f32, interval_beats: f32, offset_beats: f32, tolerance: f32) -> bool {
    let interval = interval_beats.max(1.0 / 64.0);
    let offset = offset_beats.rem_euclid(interval);
    if beat + tolerance < offset {
        return false;
    }
    let rel = (beat - offset).rem_euclid(interval);
    rel <= tolerance || interval - rel <= tolerance
}

fn tab_level(tab: Tab, controls: &T5Controls) -> f32 {
    match tab {
        Tab::Master => controls.master.level,
        Tab::Perc => controls.perc.level,
        Tab::Chords => controls.pad.level,
        Tab::Kick => controls.kick.level,
        Tab::Tonal => controls.tonal.level,
        Tab::Clap => controls.clap.level,
    }
    .clamp(0.0, 1.0)
}

fn tab_motion(tab: Tab, controls: &T5Controls) -> f32 {
    match tab {
        Tab::Master => controls.master.drive.max(controls.master.tone.abs()),
        Tab::Perc => controls.perc.lfo_depth,
        Tab::Chords => (controls.pad.stereo_width + controls.pad.detune) * 0.5,
        Tab::Kick => controls.kick.echo_amount.max(controls.kick.drive),
        Tab::Tonal => controls.tonal.randomness.max(controls.tonal.reverb_mix),
        Tab::Clap => controls.clap.room.max(controls.clap.body),
    }
    .clamp(0.0, 1.0)
}

fn item_ratio(item: &ControlItem) -> f32 {
    let range = item.max - item.min;
    if range.abs() <= f32::EPSILON {
        0.0
    } else {
        ((item.value - item.min) / range).clamp(0.0, 1.0)
    }
}

fn ratio_bar(ratio: f32, width: usize, filled: char, empty: char) -> String {
    let filled_count = (ratio.clamp(0.0, 1.0) * width as f32).round() as usize;
    let filled_count = filled_count.min(width);
    let empty_count = width.saturating_sub(filled_count);
    format!(
        "{}{}",
        filled.to_string().repeat(filled_count),
        empty.to_string().repeat(empty_count)
    )
}

fn tone_label(tone: f32) -> String {
    if tone < -0.05 {
        format!("bass {:.0}%", -tone * 100.0)
    } else if tone > 0.05 {
        format!("treble {:.0}%", tone * 100.0)
    } else {
        "flat".to_string()
    }
}

fn layer_color(tab: Tab) -> Color {
    match tab {
        Tab::Master => Color::Yellow,
        Tab::Perc => Color::Green,
        Tab::Chords => Color::Magenta,
        Tab::Kick => Color::Red,
        Tab::Tonal => Color::Cyan,
        Tab::Clap => Color::LightBlue,
    }
}

// ============================================================
// T5 Engine
// ============================================================

struct T5Engine {
    current_sample: u64,
    beat_clock: f64,
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
    fn new(
        sample_rate: f32,
        controls: Arc<ArcSwap<T5Controls>>,
        telemetry: Arc<T5Telemetry>,
    ) -> Self {
        let snapshot = T5Controls::clone(&controls.load());
        Self {
            current_sample: 0,
            beat_clock: 0.0,
            sample_rate,
            pad: PadEngine::new(sample_rate, &snapshot.pad, Arc::clone(&telemetry)),
            perc: PercEngine::new(sample_rate),
            kick: KickEngine::new(sample_rate, telemetry),
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
        if self.current_sample.is_multiple_of(512) {
            self.snapshot = T5Controls::clone(&self.controls.load());
        }

        let fade = (self.current_sample as f32 / (self.sample_rate * 8.0)).min(1.0);
        let bpm = self.snapshot.master.bpm;
        self.beat_clock += bpm as f64 / (60.0 * self.sample_rate as f64);

        let (pad_l, pad_r) = self.pad.next(&self.snapshot.pad, bpm, self.beat_clock);
        let perc = self.perc.next(&self.snapshot.perc, bpm, self.beat_clock);
        let (kick_l, kick_r) = self.kick.next(&self.snapshot.kick, bpm, self.beat_clock);
        let (ton_l, ton_r) = self.tonal.next(&self.snapshot.tonal, bpm, self.beat_clock);
        let (clap_l, clap_r) = self.clap.next(&self.snapshot.clap, self.beat_clock);

        self.current_sample += 1;

        let raw_l = (pad_l + perc * 0.6 + kick_l * 0.7 + ton_l + clap_l * 0.65) * fade;
        let raw_r = (pad_r + perc * 0.6 + kick_r * 0.7 + ton_r + clap_r * 0.65) * fade;
        self.master_bus
            .process(raw_l, raw_r, &self.snapshot.master, self.sample_rate)
    }
}

struct GridTrigger {
    last_slot: Option<f64>,
}

impl GridTrigger {
    fn new() -> Self {
        Self { last_slot: None }
    }

    fn pop(&mut self, beat_phase: f64, interval_beats: f32, offset_beats: f32) -> bool {
        let interval = (interval_beats as f64).max(1.0 / 64.0);
        let offset = (offset_beats as f64).rem_euclid(interval);

        if beat_phase < offset {
            return false;
        }
        let slot = ((beat_phase - offset) / interval).floor();

        let is_new = self.last_slot.is_none_or(|last| slot > last);
        if is_new {
            self.last_slot = Some(slot);
            true
        } else {
            false
        }
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
    fn new() -> Self {
        Self {
            comp_env: 0.0,
            tone_l: 0.0,
            tone_r: 0.0,
        }
    }

    fn process(
        &mut self,
        mut l: f32,
        mut r: f32,
        c: &MasterControls,
        sample_rate: f32,
    ) -> (f32, f32) {
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
        let rel_coeff = (-1.0_f32 / (c.comp_release_ms * 0.001 * sample_rate)).exp();
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

const MAX_PAD_LAYERS: usize = 4;

struct PadEngine {
    sample_rate: f32,
    layers: Vec<PadLayer>,
    next_change_beat: f64,
    chord_index: usize,
    reverb: Freeverb,
    depth_lfo: DriftingLfo,
    width_lfo: DriftingLfo,
    air: WhiteNoise,
    rng: StdRng,
    telemetry: Arc<T5Telemetry>,
}

impl PadEngine {
    fn new(sample_rate: f32, c: &PadControls, telemetry: Arc<T5Telemetry>) -> Self {
        Self {
            sample_rate,
            layers: vec![PadLayer::new(0, sample_rate, c.attack_time)],
            next_change_beat: c.chord_bars as f64 * 4.0,
            chord_index: 0,
            reverb: Freeverb::new(sample_rate, 0.93, 0.46, 1.0),
            depth_lfo: DriftingLfo::new(1.0 / 42.0, sample_rate),
            width_lfo: DriftingLfo::new(1.0 / 54.0, sample_rate),
            air: WhiteNoise::new(),
            rng: StdRng::from_entropy(),
            telemetry,
        }
    }

    fn next(&mut self, c: &PadControls, _bpm: f32, beat_phase: f64) -> (f32, f32) {
        if beat_phase >= self.next_change_beat {
            for layer in &mut self.layers {
                layer.release();
            }
            self.chord_index = self.chord_index.wrapping_add(1);
            self.telemetry
                .chord_index
                .store(self.chord_index as u64, Ordering::Relaxed);
            if self.layers.len() >= MAX_PAD_LAYERS {
                let remove_count = self.layers.len() + 1 - MAX_PAD_LAYERS;
                self.layers.drain(0..remove_count);
            }
            self.layers.push(PadLayer::new(
                self.chord_index,
                self.sample_rate,
                c.attack_time,
            ));
            self.next_change_beat += c.chord_bars as f64 * 4.0;
        }

        let depth = normalized_lfo(self.depth_lfo.next(&mut self.rng, 1.0 / 68.0, 1.0 / 28.0));
        let width = c.stereo_width
            * (0.58
                + normalized_lfo(self.width_lfo.next(&mut self.rng, 1.0 / 86.0, 1.0 / 38.0))
                    * 0.16);
        let detune_mix = c.detune * 0.84;
        let octave_mix = c.octave_mix * 0.32;

        let mut dry_l = 0.0f32;
        let mut dry_r = 0.0f32;
        for layer in &mut self.layers {
            let (l, r) = layer.next_stereo(width, detune_mix, octave_mix);
            dry_l += l;
            dry_r += r;
        }
        self.layers.retain(|l| !l.is_done());

        let reverb_send = c.reverb_mix * (0.48 + depth * 0.22);
        let (wet_l, wet_r) = self
            .reverb
            .process(dry_l * reverb_send, dry_r * reverb_send);
        let wet_mix = 0.72 + depth * 0.34;
        let air = self.air.next_filtered(&mut self.rng, 0.0002) * 0.00025;

        (
            (dry_l * 0.58 + wet_l * wet_mix + air) * c.level,
            (dry_r * 0.58 + wet_r * wet_mix + air) * c.level,
        )
    }
}

struct PadLayer {
    tones: Vec<PadTone>,
}

impl PadLayer {
    fn new(chord_index: usize, sample_rate: f32, attack_time: f32) -> Self {
        Self {
            tones: pad_tones(chord_index, sample_rate, attack_time),
        }
    }
    fn next_stereo(&mut self, width: f32, detune_mix: f32, octave_mix: f32) -> (f32, f32) {
        let (mut l, mut r) = (0.0f32, 0.0f32);
        for t in &mut self.tones {
            let (tl, tr) = t.next_stereo(width, detune_mix, octave_mix);
            l += tl;
            r += tr;
        }
        (l, r)
    }
    fn release(&mut self) {
        for t in &mut self.tones {
            t.release();
        }
    }
    fn is_done(&self) -> bool {
        self.tones.iter().all(PadTone::is_done)
    }
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
            primary: SineOscillator::new(hz, sample_rate),
            detuned: SineOscillator::new(hz * 1.003, sample_rate),
            octave: SineOscillator::new(hz * 2.0, sample_rate),
            envelope: Adsr::new(attack_time, 12.0, 0.86, 20.0, sample_rate),
            pan,
            gain,
        }
    }
    fn next_stereo(&mut self, width: f32, detune_mix: f32, octave_mix: f32) -> (f32, f32) {
        let s = self.primary.next()
            + self.detuned.next() * detune_mix
            + self.octave.next() * octave_mix;
        let shaped = soft_clip(s * 0.55) * self.envelope.next() * self.gain;
        StereoPanner::equal_power(shaped, self.pan * width)
    }
    fn release(&mut self) {
        self.envelope.note_off();
    }
    fn is_done(&self) -> bool {
        self.envelope.is_done()
    }
}

fn pad_tones(chord_index: usize, sample_rate: f32, attack_time: f32) -> Vec<PadTone> {
    let freqs = pad_chord(chord_index);
    let pans = [-0.52_f32, -0.18, 0.16, 0.46];
    let gains = [0.17_f32, 0.132, 0.126, 0.098];
    freqs
        .iter()
        .zip(pans)
        .zip(gains)
        .map(|((hz, pan), gain)| PadTone::new(*hz, pan, gain, attack_time, sample_rate))
        .collect()
}

fn pad_chord(index: usize) -> [f32; 4] {
    const CHORDS: [[f32; 4]; 5] = [
        [110.0, 146.83, 196.0, 261.63],
        [110.0, 164.81, 196.0, 293.66],
        [98.0, 146.83, 220.0, 261.63],
        [123.47, 164.81, 196.0, 293.66],
        [110.0, 146.83, 220.0, 329.63],
    ];
    CHORDS[index % CHORDS.len()]
}

// ============================================================
// Perc engine (16th-note white noise hits)
// ============================================================

struct PercEngine {
    sample_rate: f32,
    trigger: GridTrigger,
    hits: Vec<NoiseHit>,
    vol_lfo: DriftingLfo,
    rng: StdRng,
}

impl PercEngine {
    fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            trigger: GridTrigger::new(),
            hits: Vec::with_capacity(8),
            vol_lfo: DriftingLfo::new(0.2, sample_rate),
            rng: StdRng::from_entropy(),
        }
    }

    fn next(&mut self, c: &PercControls, bpm: f32, beat_phase: f64) -> f32 {
        // Advance LFO every sample so phase accumulates at the correct rate.
        let rate_hz = bpm / (240.0 * c.lfo_rate_bars);
        let lfo_raw = self
            .vol_lfo
            .next(&mut self.rng, rate_hz * 0.5, rate_hz * 2.0);

        if self.trigger.pop(beat_phase, 0.25, 0.0) {
            let lfo_norm = normalized_lfo(lfo_raw);
            let effective_level = c.level * ((1.0 - c.lfo_depth) + lfo_norm * c.lfo_depth);
            let smoothing = 10_f32.powf(c.filter * 4.0 - 4.0);
            self.hits.push(NoiseHit::new(
                effective_level,
                c.decay_ms,
                smoothing,
                self.sample_rate,
            ));
        }

        let mut out = 0.0f32;
        for h in &mut self.hits {
            out += h.next(&mut self.rng);
        }
        self.hits.retain(|h| !h.is_done());
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
        Self {
            noise: WhiteNoise::new(),
            samples_remaining: total,
            total_samples: total,
            level,
            filter,
        }
    }
    fn next<R: Rng>(&mut self, rng: &mut R) -> f32 {
        if self.samples_remaining == 0 {
            return 0.0;
        }
        let gain = self.samples_remaining as f32 / self.total_samples as f32;
        self.samples_remaining -= 1;
        self.noise.next_filtered(rng, self.filter) * gain * self.level * 0.4
    }
    fn is_done(&self) -> bool {
        self.samples_remaining == 0
    }
}

// ============================================================
// Kick engine
// ============================================================

fn max_kick_echo_delay_samples(sample_rate: f32) -> usize {
    ((KICK_ECHO_TIME_BEATS_MAX * 60.0 / MASTER_BPM_MIN) * sample_rate).ceil() as usize + 1
}

struct KickEngine {
    sample_rate: f32,
    trigger: GridTrigger,
    voices: Vec<KickVoice>,
    delay: KickDelay,
    rng: StdRng,
    telemetry: Arc<T5Telemetry>,
}

impl KickEngine {
    fn new(sample_rate: f32, telemetry: Arc<T5Telemetry>) -> Self {
        Self {
            sample_rate,
            trigger: GridTrigger::new(),
            voices: Vec::with_capacity(4),
            delay: KickDelay::new(max_kick_echo_delay_samples(sample_rate)),
            rng: StdRng::from_entropy(),
            telemetry,
        }
    }

    fn next(&mut self, c: &KickControls, bpm: f32, beat_phase: f64) -> (f32, f32) {
        if self
            .trigger
            .pop(beat_phase, c.interval_beats, c.offset_beats)
        {
            self.voices
                .push(KickVoice::new(c, self.sample_rate, &mut self.rng));
            self.telemetry.kick_pulse.fetch_add(1, Ordering::Relaxed);
        }

        let mut dry_l = 0.0f32;
        let mut dry_r = 0.0f32;
        for v in &mut self.voices {
            let (l, r) = v.next(&mut self.rng);
            dry_l += l;
            dry_r += r;
        }
        self.voices.retain(|v| !v.is_done());

        let delay_samples = ((c.echo_time_beats * 60.0 / bpm) * self.sample_rate) as usize;
        let (echo_l, echo_r) = self.delay.process(
            dry_l,
            dry_r,
            delay_samples,
            c.echo_filter,
            c.echo_amount,
            c.echo_feedback,
        );
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
        Self {
            buf_l: vec![0.0; n],
            buf_r: vec![0.0; n],
            head: 0,
            lp_l: 0.0,
            lp_r: 0.0,
            hp_l: 0.0,
            hp_r: 0.0,
        }
    }

    fn process(
        &mut self,
        in_l: f32,
        in_r: f32,
        delay_samples: usize,
        echo_filter: f32,
        echo_amount: f32,
        feedback: f32,
    ) -> (f32, f32) {
        let len = self.buf_l.len();
        let delay = delay_samples.clamp(1, len - 1);
        let read_pos = (self.head + len - delay) % len;

        // Wide band-pass: LP at ~2kHz centre, HP at ~60Hz, both gentle (one-pole).
        // echo_filter sweeps the LP cutoff from ~200Hz (0.0) to ~8kHz (1.0).
        let lp_coeff = 10_f32.powf(echo_filter * 3.6 - 2.3); // ~0.005..2.0 → clamp keeps it stable
        let lp_coeff = lp_coeff.clamp(0.001, 0.99);
        let hp_coeff = 0.9994_f32; // ~30 Hz high-pass, removes DC only

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
        if self.amp < 0.0001 {
            return (0.0, 0.0);
        }

        self.freq += (self.target_freq - self.freq) * self.freq_glide;
        self.phase += TAU * self.freq / self.sample_rate;
        if self.phase >= TAU {
            self.phase -= TAU;
        }

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

    fn is_done(&self) -> bool {
        self.amp < 0.0001
    }
}

// ============================================================
// Tonal engine (melodic step sequencer with randomness)
// ============================================================

struct TonalEngine {
    sample_rate: f32,
    trigger: GridTrigger,
    step_index: usize,
    voices: Vec<TonalVoice>,
    reverb: Freeverb,
    rng: StdRng,
}

const SCALE_HZ: [f32; 10] = [
    110.0, 130.81, 146.83, 164.81, 196.0, 220.0, 261.63, 293.66, 329.63, 392.0,
];
const PATTERN: [usize; 8] = [0, 2, 4, 1, 3, 5, 2, 4];

impl TonalEngine {
    fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            trigger: GridTrigger::new(),
            step_index: 0,
            voices: Vec::with_capacity(8),
            reverb: Freeverb::new(sample_rate, 0.86, 0.38, 0.9),
            rng: StdRng::from_entropy(),
        }
    }

    fn next(&mut self, c: &TonalControls, bpm: f32, beat_phase: f64) -> (f32, f32) {
        if self
            .trigger
            .pop(beat_phase, c.step_interval_beats, c.offset_beats)
        {
            let degree = if self.rng.gen_range(0.0f32..1.0) < c.randomness {
                self.rng.gen_range(0..SCALE_HZ.len())
            } else {
                let d = PATTERN[self.step_index % PATTERN.len()];
                self.step_index += 1;
                d
            };
            let hz = SCALE_HZ[degree];
            let decay_samples =
                (c.note_length_beats * 60.0 / bpm * self.sample_rate).round() as u64;
            let pan = self.rng.gen_range(-0.5f32..0.5);
            self.voices.push(TonalVoice::new(
                hz,
                pan,
                c.level,
                decay_samples,
                self.sample_rate,
            ));
        }

        let mut dry_l = 0.0f32;
        let mut dry_r = 0.0f32;
        for v in &mut self.voices {
            let (l, r) = v.next();
            dry_l += l;
            dry_r += r;
        }
        self.voices.retain(|v| !v.is_done());

        let (wet_l, wet_r) = self
            .reverb
            .process(dry_l * c.reverb_mix, dry_r * c.reverb_mix);
        (
            dry_l * (1.0 - c.reverb_mix * 0.5) + wet_l,
            dry_r * (1.0 - c.reverb_mix * 0.5) + wet_r,
        )
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
        if self.samples_remaining == 0 {
            return (0.0, 0.0);
        }
        let gain = (self.samples_remaining as f32 / self.total_samples as f32).sqrt();
        self.samples_remaining -= 1;
        let s =
            soft_clip((self.primary.next() + self.detuned.next() * 0.3) * 0.4) * gain * self.level;
        StereoPanner::equal_power(s, self.pan)
    }
    fn is_done(&self) -> bool {
        self.samples_remaining == 0
    }
}

// ============================================================
// Clap engine (multi-slap noise burst with room reverb)
// ============================================================

struct ClapEngine {
    sample_rate: f32,
    trigger: GridTrigger,
    voices: Vec<ClapVoice>,
    reverb: Freeverb,
    rng: StdRng,
}

impl ClapEngine {
    fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            trigger: GridTrigger::new(),
            voices: Vec::with_capacity(4),
            reverb: Freeverb::new(sample_rate, 0.28, 0.62, 0.85),
            rng: StdRng::from_entropy(),
        }
    }

    fn next(&mut self, c: &ClapControls, beat_phase: f64) -> (f32, f32) {
        if self
            .trigger
            .pop(beat_phase, c.interval_beats, c.offset_beats)
        {
            self.voices
                .push(ClapVoice::new(c, self.sample_rate, &mut self.rng));
        }

        let mut dry = 0.0f32;
        for v in &mut self.voices {
            dry += v.next(&mut self.rng);
        }
        self.voices.retain(|v| !v.is_done());

        let (wet_l, wet_r) = self.reverb.process(dry * c.room, dry * c.room);
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
            .map(|i| {
                if i == 0 {
                    0
                } else {
                    rng.gen_range(0..=spread.max(1))
                }
            })
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
                self.bursts.push(ClapBurst {
                    remaining: self.decay_samples,
                    total: self.decay_samples,
                });
                false
            } else {
                true
            }
        });

        if self.bursts.is_empty() && self.scheduled.is_empty() {
            return 0.0;
        }

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

struct ClapBurst {
    remaining: u64,
    total: u64,
}

// ============================================================
// Shared utilities
// ============================================================

fn normalized_lfo(sample: f32) -> f32 {
    (sample * 0.5 + 0.5).clamp(0.0, 1.0)
}

fn soft_clip(sample: f32) -> f32 {
    sample / (1.0 + sample.abs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    const SAMPLE_RATE: f32 = 48_000.0;

    #[test]
    fn tab_previous_wraps_back_one_tab() {
        assert_eq!(Tab::Master.previous(), Tab::Clap);
        assert_eq!(Tab::Kick.previous(), Tab::Chords);
    }

    #[test]
    fn render_variants_draw_without_terminal_backend() {
        let controls = T5Controls::default();

        let fluid = FluidState::new();
        for variant in [
            UiVariant::T5a,
            UiVariant::T5b,
            UiVariant::T5c,
            UiVariant::T5d,
            UiVariant::T5e,
        ] {
            let backend = TestBackend::new(100, 32);
            let mut terminal = Terminal::new(backend).unwrap();
            let items = tab_controls(Tab::Master, &controls);

            terminal
                .draw(|f| render(f, variant, &controls, &items, Tab::Master, 0, &fluid))
                .unwrap();
        }
    }

    #[test]
    fn pad_engine_caps_released_layers() {
        let controls = PadControls {
            chord_bars: 1.0,
            attack_time: 1.0,
            ..PadControls::default()
        };
        let mut pad = PadEngine::new(SAMPLE_RATE, &controls, Arc::new(T5Telemetry::default()));

        for chord in 1..12 {
            let _ = pad.next(&controls, 120.0, chord as f64 * 4.0);
            assert!(pad.layers.len() <= MAX_PAD_LAYERS);
        }
    }

    #[test]
    fn kick_delay_buffer_covers_max_echo_at_min_bpm() {
        let max_delay =
            ((KICK_ECHO_TIME_BEATS_MAX * 60.0 / MASTER_BPM_MIN) * SAMPLE_RATE).ceil() as usize;
        let delay = KickDelay::new(max_kick_echo_delay_samples(SAMPLE_RATE));

        assert_eq!(delay.buf_l.len(), max_delay + 1);
    }

    #[test]
    fn grid_trigger_fires_identically_for_same_params() {
        let mut a = GridTrigger::new();
        let mut b = GridTrigger::new();
        let mut a_hits = Vec::new();
        let mut b_hits = Vec::new();
        let mut clock = 0.0f64;
        let inc = 120.0_f64 / (60.0 * SAMPLE_RATE as f64);

        for sample in 0..(SAMPLE_RATE as u64 * 6) {
            clock += inc;
            if a.pop(clock, 2.0, 1.0) {
                a_hits.push(sample);
            }
            if b.pop(clock, 2.0, 1.0) {
                b_hits.push(sample);
            }
        }

        assert!(a_hits.len() >= 3);
        assert_eq!(a_hits, b_hits);
    }

    #[test]
    fn grid_trigger_no_silence_after_bpm_decrease() {
        let change_at = 50_000u64;
        let mut kick = GridTrigger::new();
        let mut clap = GridTrigger::new();
        let mut kick_hits: Vec<u64> = Vec::new();
        let mut clap_hits: Vec<u64> = Vec::new();
        let mut clock = 0.0f64;

        let inc1 = 120.0_f64 / (60.0 * SAMPLE_RATE as f64);
        for sample in 0..change_at {
            clock += inc1;
            if kick.pop(clock, 1.0, 0.0) {
                kick_hits.push(sample);
            }
            if clap.pop(clock, 2.0, 1.0) {
                clap_hits.push(sample);
            }
        }

        // BPM drops to 60 -- clock continues upward, never jumps back
        let inc2 = 60.0_f64 / (60.0 * SAMPLE_RATE as f64);
        for sample in change_at..(SAMPLE_RATE as u64 * 8) {
            clock += inc2;
            if kick.pop(clock, 1.0, 0.0) {
                kick_hits.push(sample);
            }
            if clap.pop(clock, 2.0, 1.0) {
                clap_hits.push(sample);
            }
        }

        // Kick should fire within one new beat period after the change
        let one_beat_samples = (60.0 / 60.0 * SAMPLE_RATE as f64) as u64;
        let first_post = kick_hits.iter().copied().find(|&s| s >= change_at);
        assert!(
            first_post.is_some_and(|s| s - change_at <= one_beat_samples),
            "kick stalled after BPM decrease"
        );
    }

    #[test]
    fn clap_voice_starts_first_burst_at_local_sample_zero() {
        let controls = ClapControls {
            level: 1.0,
            slap_count: 4.0,
            slap_spread_ms: 40.0,
            ..ClapControls::default()
        };
        let mut rng = StdRng::seed_from_u64(99);
        let mut voice = ClapVoice::new(&controls, SAMPLE_RATE, &mut rng);

        assert_eq!(voice.scheduled.first().copied(), Some(0));
        let _ = voice.next(&mut rng);
        assert_eq!(voice.current, 1);
        assert!(!voice.bursts.is_empty());
        assert!(voice.scheduled.iter().all(|&sample| sample > 0));
    }
}
