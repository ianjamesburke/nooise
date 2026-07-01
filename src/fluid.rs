use std::error::Error;
use std::f32::consts::TAU;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use arc_swap::ArcSwap;

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
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
    widgets::{Block, Borders, Paragraph, Widget},
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
pub(crate) struct FluidTelemetry {
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
            bpm: 82.0,
            level: 0.8,
            drive: 0.1,
            comp_threshold: -8.0,
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
    pub interval_beats: f32,
    pub offset_beats: f32,
}

impl Default for PercControls {
    fn default() -> Self {
        Self {
            level: 0.0,
            decay_ms: 200.0,
            filter: 0.7,
            lfo_rate_bars: 1.0,
            lfo_depth: 0.1,
            interval_beats: 0.25,
            offset_beats: 0.0,
        }
    }
}

#[derive(Clone)]
pub(crate) struct PadControls {
    pub level: f32,
    pub chord_bars: f32, // 1,2,4,8,16,32,64
    pub progression: f32,
    pub reverb_mix: f32,
    pub stereo_width: f32,
    pub detune: f32,
    pub octave_mix: f32,
    pub attack_time: f32,
    pub release_time: f32,
}

impl Default for PadControls {
    fn default() -> Self {
        Self {
            level: 0.7,
            chord_bars: 8.0,
            progression: 0.0,
            reverb_mix: 0.8,
            stereo_width: 0.8,
            detune: 0.5,
            octave_mix: 0.5,
            attack_time: 6.0,
            release_time: 8.0,
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
            start_freq: 160.0,
            pitch_decay_ms: 55.0,
            amp_decay_ms: 250.0,
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
            randomness: 0.5,
            note_length_beats: 1.5,
            step_interval_beats: 2.5,
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
            room: 0.0,
            body: 0.2,
        }
    }
}

#[derive(Clone)]
pub(crate) struct BassControls {
    pub level: f32,
    pub interval_beats: f32, // crops the 16-step rhythm phrase to this many beats (step length is fixed)
    pub offset_beats: f32,
    pub rhythm: f32, // 0..=3, A/B/C/D pattern selector
    pub octave: f32, // octaves relative to the chord root, e.g. -1.0 = one octave down
    pub attack_time: f32,
    pub decay_time: f32, // also used as the cutoff curve when a hit retriggers mid-decay
    pub drive: f32,
}

impl Default for BassControls {
    fn default() -> Self {
        Self {
            level: 0.0,
            interval_beats: 4.0,
            offset_beats: 0.0,
            rhythm: 0.0,
            octave: -1.0,
            decay_time: 0.05,
            attack_time: 0.01,
            drive: 0.15,
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct FluidControls {
    pub master: MasterControls,
    pub perc: PercControls,
    pub pad: PadControls,
    pub kick: KickControls,
    pub tonal: TonalControls,
    pub clap: ClapControls,
    pub bass: BassControls,
}

// ============================================================
// Entry point
// ============================================================

const APP_ID: &str = "nooise";

pub(crate) fn run() -> Result<(), Box<dyn Error>> {
    let controls = Arc::new(ArcSwap::from_pointee(FluidControls::default()));
    let controls_for_engine = Arc::clone(&controls);
    let telemetry = Arc::new(FluidTelemetry::default());
    let telemetry_for_engine = Arc::clone(&telemetry);

    let _stream = audio::start_stream(APP_ID, move |sr| {
        FluidEngine::new(sr, controls_for_engine, telemetry_for_engine)
    })?;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = ui_loop(&mut terminal, controls, telemetry);

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
    Bass = 3,
    Kick = 4,
    Tonal = 5,
    Clap = 6,
}

impl Tab {
    fn all() -> [Tab; 7] {
        [
            Tab::Master,
            Tab::Perc,
            Tab::Chords,
            Tab::Bass,
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
            Tab::Bass => "Bass",
            Tab::Kick => "Kick",
            Tab::Tonal => "Tonal",
            Tab::Clap => "Clap",
        }
    }

    fn next(self) -> Self {
        match self {
            Tab::Master => Tab::Perc,
            Tab::Perc => Tab::Chords,
            Tab::Chords => Tab::Bass,
            Tab::Bass => Tab::Kick,
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
            Tab::Bass => Tab::Chords,
            Tab::Kick => Tab::Bass,
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

fn tab_controls(tab: Tab, c: &FluidControls) -> Vec<ControlItem> {
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
                label: "Bass Vol",
                value: c.bass.level,
                min: 0.0,
                max: 1.0,
                display: format!("{:.0}%", c.bass.level * 100.0),
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
                label: "Interval",
                value: c.perc.interval_beats,
                min: 0.25,
                max: 4.25,
                display: if c.perc.interval_beats >= 4.25 {
                    "Continuous".to_string()
                } else {
                    format!("{:.2} beats", c.perc.interval_beats)
                },
            },
            ControlItem {
                label: "Offset",
                value: c.perc.offset_beats,
                min: 0.0,
                max: 4.0,
                display: format!("{:.2} beats", c.perc.offset_beats),
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
                label: "Progression",
                value: c.pad.progression,
                min: 0.0,
                max: 3.0,
                display: ["A", "B", "C", "D"][c.pad.progression.round() as usize % 4]
                    .to_string(),
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
                min: 0.05,
                max: 30.0,
                display: format!("{:.2} s", c.pad.attack_time),
            },
            ControlItem {
                label: "Release",
                value: c.pad.release_time,
                min: 0.05,
                max: 20.0,
                display: format!("{:.2} s", c.pad.release_time),
            },
        ],
        Tab::Bass => vec![
            ControlItem {
                label: "Level",
                value: c.bass.level,
                min: 0.0,
                max: 1.0,
                display: format!("{:.0}%", c.bass.level * 100.0),
            },
            ControlItem {
                label: "Interval",
                value: c.bass.interval_beats,
                min: 0.25,
                max: 8.0,
                display: format!("{:.2} beats", c.bass.interval_beats),
            },
            ControlItem {
                label: "Offset",
                value: c.bass.offset_beats,
                min: 0.0,
                max: 4.0,
                display: format!("{:.2} beats", c.bass.offset_beats),
            },
            ControlItem {
                label: "Rhythm",
                value: c.bass.rhythm,
                min: 0.0,
                max: 3.0,
                display: ["A", "B", "C", "D"][c.bass.rhythm.round() as usize % 4].to_string(),
            },
            ControlItem {
                label: "Octave",
                value: c.bass.octave,
                min: -3.0,
                max: 0.0,
                display: format!("{:.0}", c.bass.octave),
            },
            ControlItem {
                label: "Attack",
                value: c.bass.attack_time,
                min: 0.005,
                max: 1.0,
                display: format!("{:.3} s", c.bass.attack_time),
            },
            ControlItem {
                label: "Decay",
                value: c.bass.decay_time,
                min: 0.005,
                max: 2.0,
                display: format!("{:.3} s", c.bass.decay_time),
            },
            ControlItem {
                label: "Drive",
                value: c.bass.drive,
                min: 0.0,
                max: 1.0,
                display: format!("{:.0}%", c.bass.drive * 100.0),
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
                label: "Interval",
                value: c.kick.interval_beats,
                min: 0.25,
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

fn apply_delta(tab: Tab, selected: usize, dir: f32, c: &mut FluidControls) {
    match tab {
        Tab::Master => match selected {
            0 => c.pad.level = (c.pad.level + dir * 0.02).clamp(0.0, 1.0),
            1 => c.perc.level = (c.perc.level + dir * 0.02).clamp(0.0, 1.0),
            2 => c.kick.level = (c.kick.level + dir * 0.02).clamp(0.0, 1.0),
            3 => c.tonal.level = (c.tonal.level + dir * 0.02).clamp(0.0, 1.0),
            4 => c.clap.level = (c.clap.level + dir * 0.02).clamp(0.0, 1.0),
            5 => c.bass.level = (c.bass.level + dir * 0.02).clamp(0.0, 1.0),
            6 => c.master.bpm = (c.master.bpm + dir * 2.0).clamp(MASTER_BPM_MIN, MASTER_BPM_MAX),
            7 => c.master.level = (c.master.level + dir * 0.02).clamp(0.0, 1.0),
            8 => c.master.drive = (c.master.drive + dir * 0.02).clamp(0.0, 1.0),
            9 => c.master.comp_threshold = (c.master.comp_threshold + dir * 1.0).clamp(-40.0, 0.0),
            10 => c.master.comp_ratio = (c.master.comp_ratio + dir * 0.25).clamp(1.0, 8.0),
            11 => {
                c.master.comp_release_ms =
                    (c.master.comp_release_ms + dir * 10.0).clamp(10.0, 500.0)
            }
            12 => c.master.tone = (c.master.tone + dir * 0.05).clamp(-1.0, 1.0),
            _ => {}
        },
        Tab::Perc => match selected {
            0 => c.perc.level = (c.perc.level + dir * 0.02).clamp(0.0, 1.0),
            1 => c.perc.interval_beats = (c.perc.interval_beats + dir * 0.25).clamp(0.25, 4.25),
            2 => c.perc.offset_beats = (c.perc.offset_beats + dir * 0.25).clamp(0.0, 4.0),
            3 => c.perc.decay_ms = (c.perc.decay_ms + dir * 20.0).clamp(20.0, 2000.0),
            4 => c.perc.filter = (c.perc.filter + dir * 0.02).clamp(0.5, 1.0),
            5 => c.perc.lfo_rate_bars = (c.perc.lfo_rate_bars + dir * 0.25).clamp(0.25, 16.0),
            6 => c.perc.lfo_depth = (c.perc.lfo_depth + dir * 0.02).clamp(0.0, 1.0),
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
            2 => c.pad.progression = (c.pad.progression + dir).clamp(0.0, 3.0),
            3 => c.pad.reverb_mix = (c.pad.reverb_mix + dir * 0.02).clamp(0.0, 1.0),
            4 => c.pad.stereo_width = (c.pad.stereo_width + dir * 0.02).clamp(0.0, 1.0),
            5 => c.pad.detune = (c.pad.detune + dir * 0.02).clamp(0.0, 1.0),
            6 => c.pad.octave_mix = (c.pad.octave_mix + dir * 0.02).clamp(0.0, 1.0),
            7 => c.pad.attack_time = (c.pad.attack_time + dir * 0.5).clamp(0.05, 30.0),
            8 => c.pad.release_time = (c.pad.release_time + dir * 0.5).clamp(0.05, 20.0),
            _ => {}
        },
        Tab::Bass => match selected {
            0 => c.bass.level = (c.bass.level + dir * 0.02).clamp(0.0, 1.0),
            1 => c.bass.interval_beats = (c.bass.interval_beats + dir * 0.25).clamp(0.25, 8.0),
            2 => c.bass.offset_beats = (c.bass.offset_beats + dir * 0.25).clamp(0.0, 4.0),
            3 => c.bass.rhythm = (c.bass.rhythm + dir).clamp(0.0, 3.0),
            4 => c.bass.octave = (c.bass.octave + dir).clamp(-3.0, 0.0),
            5 => c.bass.attack_time = (c.bass.attack_time + dir * 0.02).clamp(0.005, 1.0),
            6 => c.bass.decay_time = (c.bass.decay_time + dir * 0.05).clamp(0.005, 2.0),
            7 => c.bass.drive = (c.bass.drive + dir * 0.02).clamp(0.0, 1.0),
            _ => {}
        },
        Tab::Kick => match selected {
            0 => c.kick.level = (c.kick.level + dir * 0.02).clamp(0.0, 1.0),
            1 => c.kick.interval_beats = (c.kick.interval_beats + dir * 0.25).clamp(0.25, 4.0),
            2 => c.kick.offset_beats = (c.kick.offset_beats + dir * 0.25).clamp(0.0, 4.0),
            3 => c.kick.start_freq = (c.kick.start_freq + dir * 5.0).clamp(40.0, 200.0),
            4 => c.kick.pitch_decay_ms = (c.kick.pitch_decay_ms + dir * 5.0).clamp(10.0, 300.0),
            5 => c.kick.amp_decay_ms = (c.kick.amp_decay_ms + dir * 20.0).clamp(50.0, 1000.0),
            6 => c.kick.click = (c.kick.click + dir * 0.01).clamp(0.0, 0.2),
            7 => c.kick.drive = (c.kick.drive + dir * 0.02).clamp(0.0, 1.0),
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
            1 => {
                c.tonal.step_interval_beats =
                    (c.tonal.step_interval_beats + dir * 0.25).clamp(0.5, 4.0)
            }
            2 => c.tonal.offset_beats = (c.tonal.offset_beats + dir * 0.25).clamp(0.0, 4.0),
            3 => c.tonal.randomness = (c.tonal.randomness + dir * 0.02).clamp(0.0, 1.0),
            4 => {
                c.tonal.note_length_beats = (c.tonal.note_length_beats + dir * 0.05).clamp(0.1, 2.0)
            }
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

fn apply_min(tab: Tab, selected: usize, c: &mut FluidControls) {
    match tab {
        Tab::Master => match selected {
            0 => c.pad.level = 0.0,
            1 => c.perc.level = 0.0,
            2 => c.kick.level = 0.0,
            3 => c.tonal.level = 0.0,
            4 => c.clap.level = 0.0,
            5 => c.bass.level = 0.0,
            6 => c.master.bpm = MASTER_BPM_MIN,
            7 => c.master.level = 0.0,
            8 => c.master.drive = 0.0,
            9 => c.master.comp_threshold = -40.0,
            10 => c.master.comp_ratio = 1.0,
            11 => c.master.comp_release_ms = 10.0,
            12 => c.master.tone = -1.0,
            _ => {}
        },
        Tab::Perc => match selected {
            0 => c.perc.level = 0.0,
            1 => c.perc.interval_beats = 0.25,
            2 => c.perc.offset_beats = 0.0,
            3 => c.perc.decay_ms = 20.0,
            4 => c.perc.filter = 0.5,
            5 => c.perc.lfo_rate_bars = 0.25,
            6 => c.perc.lfo_depth = 0.0,
            _ => {}
        },
        Tab::Chords => match selected {
            0 => c.pad.level = 0.0,
            1 => c.pad.chord_bars = 1.0,
            2 => c.pad.progression = 0.0,
            3 => c.pad.reverb_mix = 0.0,
            4 => c.pad.stereo_width = 0.0,
            5 => c.pad.detune = 0.0,
            6 => c.pad.octave_mix = 0.0,
            7 => c.pad.attack_time = 0.05,
            8 => c.pad.release_time = 0.05,
            _ => {}
        },
        Tab::Bass => match selected {
            0 => c.bass.level = 0.0,
            1 => c.bass.interval_beats = 0.25,
            2 => c.bass.offset_beats = 0.0,
            3 => c.bass.rhythm = 0.0,
            4 => c.bass.octave = -3.0,
            5 => c.bass.attack_time = 0.005,
            6 => c.bass.decay_time = 0.005,
            7 => c.bass.drive = 0.0,
            _ => {}
        },
        Tab::Kick => match selected {
            0 => c.kick.level = 0.0,
            1 => c.kick.interval_beats = 0.25,
            2 => c.kick.offset_beats = 0.0,
            3 => c.kick.start_freq = 40.0,
            4 => c.kick.pitch_decay_ms = 10.0,
            5 => c.kick.amp_decay_ms = 50.0,
            6 => c.kick.click = 0.0,
            7 => c.kick.drive = 0.0,
            8 => c.kick.echo_time_beats = KICK_ECHO_TIME_BEATS_MIN,
            9 => c.kick.echo_filter = 0.0,
            10 => c.kick.echo_amount = 0.0,
            11 => c.kick.echo_feedback = 0.0,
            _ => {}
        },
        Tab::Tonal => match selected {
            0 => c.tonal.level = 0.0,
            1 => c.tonal.step_interval_beats = 0.5,
            2 => c.tonal.offset_beats = 0.0,
            3 => c.tonal.randomness = 0.0,
            4 => c.tonal.note_length_beats = 0.1,
            5 => c.tonal.reverb_mix = 0.0,
            _ => {}
        },
        Tab::Clap => match selected {
            0 => c.clap.level = 0.0,
            1 => c.clap.interval_beats = 0.5,
            2 => c.clap.offset_beats = 0.0,
            3 => c.clap.slap_count = 1.0,
            4 => c.clap.slap_spread_ms = 0.0,
            5 => c.clap.decay_ms = 10.0,
            6 => c.clap.filter = 0.5,
            7 => c.clap.room = 0.0,
            8 => c.clap.body = 0.0,
            _ => {}
        },
    }
}

fn ui_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    controls: Arc<ArcSwap<FluidControls>>,
    telemetry: Arc<FluidTelemetry>,
) -> Result<(), Box<dyn Error>> {
    let mut tab = Tab::Master;
    let mut selected = 0usize;
    let mut fluid = FluidState::new();
    let mut last = Instant::now();

    loop {
        let c = FluidControls::clone(&controls.load());
        let items = tab_controls(tab, &c);
        let items_len = items.len();
        selected = selected.min(items_len.saturating_sub(1));

        let now = Instant::now();
        let dt = (now - last).as_secs_f32().min(0.05);
        last = now;
        fluid.tick(dt, &telemetry);

        terminal.draw(|f| render(f, &items, tab, selected, &fluid))?;

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
                KeyCode::Left if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    reset_to_min(&controls, tab, selected)
                }
                KeyCode::Char('H') => reset_to_min(&controls, tab, selected),
                KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    reset_to_min(&controls, tab, selected)
                }
                KeyCode::Left | KeyCode::Char('h') => adjust(&controls, tab, selected, -1.0),
                KeyCode::Right | KeyCode::Char('l') => adjust(&controls, tab, selected, 1.0),
                _ => {}
            }
        }
    }

    Ok(())
}

fn adjust(controls: &Arc<ArcSwap<FluidControls>>, tab: Tab, selected: usize, dir: f32) {
    let mut next = FluidControls::clone(&controls.load());
    apply_delta(tab, selected, dir, &mut next);
    controls.store(Arc::new(next));
}

fn reset_to_min(controls: &Arc<ArcSwap<FluidControls>>, tab: Tab, selected: usize) {
    let mut next = FluidControls::clone(&controls.load());
    apply_min(tab, selected, &mut next);
    controls.store(Arc::new(next));
}

fn render(
    f: &mut Frame,
    items: &[ControlItem],
    active_tab: Tab,
    selected: usize,
    fluid: &FluidState,
) {
    render_fluid(f, items, active_tab, selected, fluid);
}

// ============================================================
// Fluid visualizer: chords drive the field colour, kicks
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

    fn tick(&mut self, dt: f32, telemetry: &FluidTelemetry) {
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

fn render_fluid(
    f: &mut Frame,
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
        Paragraph::new("jk select   hl adjust   H min   Tab layer   q quit")
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::Rgb(120, 128, 145))),
        layout[5],
    );
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

// ============================================================
// Fluid Engine
// ============================================================

struct FluidEngine {
    current_sample: u64,
    sample_rate: f32,
    tempo: TempoClock,
    pad: PadEngine,
    perc: PercEngine,
    kick: KickEngine,
    tonal: TonalEngine,
    clap: ClapEngine,
    bass: BassEngine,
    master_bus: MasterBus,
    controls: Arc<ArcSwap<FluidControls>>,
    snapshot: FluidControls,
}

impl FluidEngine {
    fn new(
        sample_rate: f32,
        controls: Arc<ArcSwap<FluidControls>>,
        telemetry: Arc<FluidTelemetry>,
    ) -> Self {
        let snapshot = FluidControls::clone(&controls.load());
        Self {
            current_sample: 0,
            sample_rate,
            tempo: TempoClock::new(sample_rate, snapshot.master.bpm),
            pad: PadEngine::new(sample_rate, &snapshot.pad, Arc::clone(&telemetry)),
            perc: PercEngine::new(sample_rate),
            kick: KickEngine::new(sample_rate, telemetry),
            tonal: TonalEngine::new(sample_rate),
            clap: ClapEngine::new(sample_rate),
            bass: BassEngine::new(sample_rate),
            master_bus: MasterBus::new(),
            controls,
            snapshot,
        }
    }
}

impl StereoEngine for FluidEngine {
    fn next_stereo(&mut self) -> (f32, f32) {
        if self.current_sample.is_multiple_of(512) {
            self.snapshot = FluidControls::clone(&self.controls.load());
        }

        let fade = (self.current_sample as f32 / (self.sample_rate * 8.0)).min(1.0);
        let timing = self.tempo.tick(self.snapshot.master.bpm);

        let (pad_l, pad_r) = self.pad.next(&self.snapshot.pad, timing);
        let perc = self.perc.next(&self.snapshot.perc, timing);
        let (kick_l, kick_r) = self.kick.next(&self.snapshot.kick, timing);
        let (ton_l, ton_r) = self.tonal.next(&self.snapshot.tonal, timing);
        let (clap_l, clap_r) = self.clap.next(&self.snapshot.clap, timing);
        let (bass_l, bass_r) = self.bass.next(&self.snapshot.bass, &self.snapshot.pad, timing);

        self.current_sample += 1;

        let raw_l =
            (pad_l + perc * 0.6 + kick_l * 0.7 + ton_l + clap_l * 0.65 + bass_l * 0.75) * fade;
        let raw_r =
            (pad_r + perc * 0.6 + kick_r * 0.7 + ton_r + clap_r * 0.65 + bass_r * 0.75) * fade;
        self.master_bus
            .process(raw_l, raw_r, &self.snapshot.master, self.sample_rate)
    }
}

const TEMPO_SMOOTH_MS: f64 = 180.0;

struct TempoClock {
    beat: f64,
    bpm: f64,
    sample_rate: f64,
}

impl TempoClock {
    fn new(sample_rate: f32, bpm: f32) -> Self {
        Self {
            beat: 0.0,
            bpm: f64::from(bpm.clamp(MASTER_BPM_MIN, MASTER_BPM_MAX)),
            sample_rate: f64::from(sample_rate.max(1.0)),
        }
    }

    fn tick(&mut self, target_bpm: f32) -> TimingContext {
        let target_bpm = f64::from(target_bpm.clamp(MASTER_BPM_MIN, MASTER_BPM_MAX));
        let smoothing_samples = (TEMPO_SMOOTH_MS * 0.001 * self.sample_rate).max(1.0);
        let coeff = 1.0 - (-1.0 / smoothing_samples).exp();
        self.bpm += (target_bpm - self.bpm) * coeff;

        let timing = TimingContext::new(self.sample_rate, self.bpm, self.beat);
        self.beat += self.bpm / (60.0 * self.sample_rate);
        timing
    }
}

#[derive(Clone, Copy)]
struct TimingContext {
    sample_rate: f64,
    bpm: f64,
    beat: f64,
}

impl TimingContext {
    fn new(sample_rate: f64, bpm: f64, beat: f64) -> Self {
        Self {
            sample_rate: sample_rate.max(1.0),
            bpm: bpm.max(1.0),
            beat,
        }
    }

    fn samples_per_beat(self) -> f64 {
        self.sample_rate * 60.0 / self.bpm
    }

    fn beats_to_samples(self, beats: f32) -> u64 {
        (f64::from(beats.max(0.0)) * self.samples_per_beat())
            .round()
            .max(1.0) as u64
    }

    fn lfo_hz_for_bars(self, bars: f32) -> f32 {
        (self.bpm as f32) / (240.0 * bars.max(1.0 / 64.0))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct GridSpec {
    interval_beats: f64,
    offset_beats: f64,
}

impl GridSpec {
    fn new(interval_beats: f32, offset_beats: f32) -> Self {
        let interval_beats = f64::from(interval_beats).max(1.0 / 64.0);
        Self {
            interval_beats,
            offset_beats: f64::from(offset_beats).rem_euclid(interval_beats),
        }
    }

    fn hit_at_or_after(self, beat: f64) -> GridHit {
        let interval = self.interval_beats;
        let offset = self.offset_beats;
        let slot = if beat <= offset {
            0
        } else {
            ((beat - offset) / interval).ceil().max(0.0) as u64
        };
        GridHit {
            beat: offset + slot as f64 * interval,
        }
    }

    fn hit_after(self, beat: f64) -> GridHit {
        self.hit_at_or_after(beat + GRID_BEAT_EPSILON)
    }
}

const GRID_BEAT_EPSILON: f64 = 1e-9;

#[derive(Clone, Copy, Debug)]
struct GridHit {
    beat: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FirstGridHit {
    AtOrAfterNow,
    AfterNow,
}

struct GridTrigger {
    spec: Option<GridSpec>,
    next_hit: Option<GridHit>,
    first_hit: FirstGridHit,
}

impl GridTrigger {
    fn new() -> Self {
        Self::with_first_hit(FirstGridHit::AtOrAfterNow)
    }

    fn after_start() -> Self {
        Self::with_first_hit(FirstGridHit::AfterNow)
    }

    fn with_first_hit(first_hit: FirstGridHit) -> Self {
        Self {
            spec: None,
            next_hit: None,
            first_hit,
        }
    }

    fn pop(&mut self, timing: TimingContext, interval_beats: f32, offset_beats: f32) -> bool {
        let spec = GridSpec::new(interval_beats, offset_beats);
        if self.spec != Some(spec) {
            let first_schedule =
                self.next_hit.is_none() && self.first_hit == FirstGridHit::AfterNow;
            self.spec = Some(spec);
            self.next_hit = Some(if first_schedule {
                spec.hit_after(timing.beat)
            } else {
                spec.hit_at_or_after(timing.beat)
            });
        }

        let Some(next_hit) = self.next_hit else {
            return false;
        };
        if timing.beat + GRID_BEAT_EPSILON >= next_hit.beat {
            self.next_hit = Some(spec.hit_after(timing.beat));
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
// Pad engine (chord drones)
// ============================================================

const MAX_PAD_LAYERS: usize = 4;

struct PadEngine {
    sample_rate: f32,
    layers: Vec<PadLayer>,
    chord_trigger: GridTrigger,
    step_index: usize,
    last_progression: usize,
    reverb: Freeverb,
    depth_lfo: DriftingLfo,
    width_lfo: DriftingLfo,
    air: WhiteNoise,
    rng: StdRng,
    telemetry: Arc<FluidTelemetry>,
}

impl PadEngine {
    fn new(sample_rate: f32, c: &PadControls, telemetry: Arc<FluidTelemetry>) -> Self {
        Self {
            sample_rate,
            layers: vec![PadLayer::new(
                0,
                0,
                sample_rate,
                c.attack_time,
                c.release_time,
            )],
            chord_trigger: GridTrigger::after_start(),
            step_index: 0,
            last_progression: 0,
            reverb: Freeverb::new(sample_rate, 0.93, 0.46, 1.0),
            depth_lfo: DriftingLfo::new(1.0 / 42.0, sample_rate),
            width_lfo: DriftingLfo::new(1.0 / 54.0, sample_rate),
            air: WhiteNoise::new(),
            rng: StdRng::from_entropy(),
            telemetry,
        }
    }

    fn next(&mut self, c: &PadControls, timing: TimingContext) -> (f32, f32) {
        let progression = (c.progression.round() as i64).rem_euclid(4) as usize;
        let progression_changed = progression != self.last_progression;
        self.last_progression = progression;

        let advance = self.chord_trigger.pop(timing, c.chord_bars * 4.0, 0.0);

        if advance || progression_changed {
            for layer in &mut self.layers {
                layer.release();
            }
            if advance {
                self.step_index = (self.step_index + 1) % 8;
            }
            self.telemetry
                .chord_index
                .store(self.step_index as u64, Ordering::Relaxed);
            if self.layers.len() >= MAX_PAD_LAYERS {
                let remove_count = self.layers.len() + 1 - MAX_PAD_LAYERS;
                self.layers.drain(0..remove_count);
            }
            self.layers.push(PadLayer::new(
                progression,
                self.step_index,
                self.sample_rate,
                c.attack_time,
                c.release_time,
            ));
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
    fn new(
        progression: usize,
        step: usize,
        sample_rate: f32,
        attack_time: f32,
        release_time: f32,
    ) -> Self {
        Self {
            tones: pad_tones(progression, step, sample_rate, attack_time, release_time),
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
    fn new(
        hz: f32,
        pan: f32,
        gain: f32,
        attack_time: f32,
        release_time: f32,
        sample_rate: f32,
    ) -> Self {
        Self {
            primary: SineOscillator::new(hz, sample_rate),
            detuned: SineOscillator::new(hz * 1.003, sample_rate),
            octave: SineOscillator::new(hz * 2.0, sample_rate),
            envelope: Adsr::new(attack_time, 12.0, 0.86, release_time, sample_rate),
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

fn pad_tones(
    progression: usize,
    step: usize,
    sample_rate: f32,
    attack_time: f32,
    release_time: f32,
) -> Vec<PadTone> {
    let freqs = pad_chord(progression, step);
    let pans = [-0.52_f32, -0.18, 0.16, 0.46];
    let gains = [0.17_f32, 0.132, 0.126, 0.098];
    freqs
        .iter()
        .zip(pans)
        .zip(gains)
        .map(|((hz, pan), gain)| PadTone::new(*hz, pan, gain, attack_time, release_time, sample_rate))
        .collect()
}

fn midi_to_hz(note: i32) -> f32 {
    440.0 * 2f32.powf((note as f32 - 69.0) / 12.0)
}

const PROGRESSIONS: [[[i32; 4]; 8]; 4] = [
    // Progression A: with an 8s release, each chord rings well into the next
    // (and beyond), so voicings are chosen to hold at least one common tone
    // across every step, including the loop back to step 0.
    [
        [45, 50, 55, 60], // Am
        [43, 50, 57, 60], // G   (holds D3/C4 from Am)
        [45, 52, 57, 60], // Am (alt voicing, holds A3/C4 from G)
        [47, 52, 55, 62], // B   (holds E3 from Am)
        [45, 52, 57, 64], // Am (alt voicing, holds E3 from B)
        [43, 50, 55, 62], // G   (parallel shift from Am, glides in stepwise)
        [48, 55, 60, 64], // C   (holds G3 from G)
        [55, 59, 64, 67], // Em (holds G3/C4 from C, and G3 back into Am)
    ],
    [
        [45, 50, 57, 60], // Am
        [50, 53, 57, 62], // Dm
        [48, 55, 60, 64], // C
        [43, 50, 55, 59], // G
        [41, 48, 53, 57], // F
        [52, 59, 64, 67], // Em
        [45, 52, 57, 60], // Am
        [43, 50, 55, 59], // G (non-tonic close, leads back to Am)
    ],
    [
        [45, 48, 52, 55], // Am7
        [41, 45, 48, 52], // Fmaj7
        [48, 52, 55, 59], // Cmaj7
        [43, 47, 50, 53], // G7
        [50, 53, 57, 60], // Dm7
        [52, 55, 59, 62], // Em7
        [47, 50, 53, 57], // Bm7b5 (half-diminished ii)
        [43, 50, 55, 59], // G (non-tonic close)
    ],
    [
        [45, 52, 57, 60], // Am, wide
        [41, 45, 48, 55], // Fmaj9-flavor
        [48, 55, 59, 62], // Cmaj9-flavor
        [43, 50, 53, 57], // G9-flavor
        [50, 57, 60, 64], // Dm9-flavor
        [52, 55, 59, 64], // Em, open
        [47, 53, 57, 64], // Bm7b5, wide (the "ache" chord)
        [43, 50, 55, 64], // G, wide (non-tonic close)
    ],
];

fn pad_chord(progression: usize, step: usize) -> [f32; 4] {
    PROGRESSIONS[progression % PROGRESSIONS.len()][step % 8].map(midi_to_hz)
}

/// Lowest MIDI note of the chord currently playing on the Pad voice — the
/// note the Bass voice tracks.
fn bass_root_note(progression: usize, step: usize) -> i32 {
    PROGRESSIONS[progression % PROGRESSIONS.len()][step % 8]
        .into_iter()
        .min()
        .expect("chords always have 4 notes")
}

// ============================================================
// Bass engine (follows the Pad's chord root on a rhythm pattern)
// ============================================================

/// Four 16-step rhythm patterns (one bar at 16th-note resolution: counted
/// "1 e & a 2 e & a 3 e & a 4 e & a"). A/B/C/D selects between them; `true`
/// re-articulates the bass note at that step.
const BASS_RHYTHMS: [[bool; 16]; 4] = [
    // A: quarter notes on the beat
    [
        true, false, false, false, true, false, false, false, true, false, false, false, true,
        false, false, false,
    ],
    // B: syncopated — pickup into 1, push before 3, quick pickups into 4
    [
        true, false, false, true, false, false, true, false, true, true, false, false, true,
        true, false, false,
    ],
    // C: straight eighths — steady walking-bass feel
    [
        true, false, true, false, true, false, true, false, true, false, true, false, true,
        false, true, false,
    ],
    // D: busy 16th groove
    [
        true, false, false, true, false, false, true, false, true, false, false, true, false,
        false, true, false,
    ],
];

const MAX_BASS_VOICES: usize = 3;

/// Fixed duration of one rhythm-pattern step (a 16th note). Step timing never
/// changes; `interval_beats` instead crops how many steps of the 16-step
/// phrase play before looping back to step 0 (or extends the loop with
/// trailing silence, for a "gap" feel).
const BASS_STEP_BEATS: f32 = 0.25;

struct BassEngine {
    sample_rate: f32,
    chord_trigger: GridTrigger,
    step_index: usize,
    step_trigger: GridTrigger,
    rhythm_step: usize,
    voices: Vec<BassVoice>,
}

impl BassEngine {
    fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            chord_trigger: GridTrigger::after_start(),
            step_index: 0,
            step_trigger: GridTrigger::new(),
            rhythm_step: BASS_RHYTHMS[0].len() - 1,
            voices: Vec::with_capacity(MAX_BASS_VOICES),
        }
    }

    fn next(&mut self, c: &BassControls, pad: &PadControls, timing: TimingContext) -> (f32, f32) {
        let progression = (pad.progression.round() as i64).rem_euclid(4) as usize;
        if self.chord_trigger.pop(timing, pad.chord_bars * 4.0, 0.0) {
            self.step_index = (self.step_index + 1) % 8;
        }

        let loop_len = (c.interval_beats / BASS_STEP_BEATS).round().clamp(1.0, 32.0) as usize;
        if self.step_trigger.pop(timing, BASS_STEP_BEATS, c.offset_beats) {
            self.rhythm_step = (self.rhythm_step + 1) % loop_len;
            let rhythm = (c.rhythm.round() as usize) % BASS_RHYTHMS.len();
            let hit = self.rhythm_step < BASS_RHYTHMS[rhythm].len()
                && BASS_RHYTHMS[rhythm][self.rhythm_step];
            if hit {
                let note = bass_root_note(progression, self.step_index)
                    + (c.octave.round() as i32) * 12;
                let hz = midi_to_hz(note);
                for voice in &mut self.voices {
                    voice.release();
                }
                if self.voices.len() >= MAX_BASS_VOICES {
                    let remove_count = self.voices.len() + 1 - MAX_BASS_VOICES;
                    self.voices.drain(0..remove_count);
                }
                self.voices.push(BassVoice::new(
                    hz,
                    c.attack_time,
                    c.decay_time,
                    c.drive,
                    self.sample_rate,
                ));
            }
        }

        let mut l = 0.0f32;
        let mut r = 0.0f32;
        for voice in &mut self.voices {
            let (vl, vr) = voice.next();
            l += vl;
            r += vr;
        }
        self.voices.retain(|v| !v.is_done());

        (l * c.level, r * c.level)
    }
}

struct BassVoice {
    osc: SineOscillator,
    envelope: Adsr,
    drive: f32,
}

impl BassVoice {
    fn new(hz: f32, attack_time: f32, decay_time: f32, drive: f32, sample_rate: f32) -> Self {
        Self {
            osc: SineOscillator::new(hz, sample_rate),
            // No sustain — Decay carries the note fully to silence, like the
            // Perc voice's percussive envelope. Decay also doubles as the
            // release curve, smoothing the cutoff if a hit retriggers before
            // the previous note has fully decayed.
            envelope: Adsr::new(attack_time, decay_time, 0.0, decay_time, sample_rate),
            drive,
        }
    }

    fn next(&mut self) -> (f32, f32) {
        let mut s = self.osc.next() * self.envelope.next();
        if self.drive > 0.0 {
            let driven = s * (1.0 + self.drive * 8.0);
            s = driven / (1.0 + driven.abs()) * (1.0 + self.drive * 0.5);
        }
        StereoPanner::equal_power(s, 0.0)
    }

    fn release(&mut self) {
        self.envelope.note_off();
    }

    fn is_done(&self) -> bool {
        self.envelope.is_done()
    }
}

// ============================================================
// Perc engine (16th-note white noise hits)
// ============================================================

struct PercEngine {
    sample_rate: f32,
    trigger: GridTrigger,
    hits: Vec<NoiseHit>,
    noise: WhiteNoise,
    vol_lfo: DriftingLfo,
    rng: StdRng,
}

impl PercEngine {
    fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            trigger: GridTrigger::new(),
            hits: Vec::with_capacity(8),
            noise: WhiteNoise::new(),
            vol_lfo: DriftingLfo::new(0.2, sample_rate),
            rng: StdRng::from_entropy(),
        }
    }

    fn next(&mut self, c: &PercControls, timing: TimingContext) -> f32 {
        // Advance LFO every sample so phase accumulates at the correct rate.
        let rate_hz = timing.lfo_hz_for_bars(c.lfo_rate_bars);
        let lfo_raw = self
            .vol_lfo
            .next(&mut self.rng, rate_hz * 0.5, rate_hz * 2.0);
        let lfo_norm = normalized_lfo(lfo_raw);
        let effective_level = c.level * ((1.0 - c.lfo_depth) + lfo_norm * c.lfo_depth);

        if c.interval_beats >= 4.25 {
            // Continuous mode: bypass GridTrigger/NoiseHit entirely so there is
            // no trigger-rate amplitude ripple to disguise (see GOTCHAS.md).
            // Reuse the same exponential smoothing transform as discrete hits so
            // Filter has a comparably audible range in both modes.
            let smoothing = 10_f32.powf(c.filter * 4.0 - 4.0);
            return self.noise.next_filtered(&mut self.rng, smoothing) * effective_level * 0.4;
        }

        if self.trigger.pop(timing, c.interval_beats, c.offset_beats) {
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
    telemetry: Arc<FluidTelemetry>,
}

impl KickEngine {
    fn new(sample_rate: f32, telemetry: Arc<FluidTelemetry>) -> Self {
        Self {
            sample_rate,
            trigger: GridTrigger::new(),
            voices: Vec::with_capacity(4),
            delay: KickDelay::new(max_kick_echo_delay_samples(sample_rate)),
            rng: StdRng::from_entropy(),
            telemetry,
        }
    }

    fn next(&mut self, c: &KickControls, timing: TimingContext) -> (f32, f32) {
        if self.trigger.pop(timing, c.interval_beats, c.offset_beats) {
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

        let delay_samples = timing.beats_to_samples(c.echo_time_beats) as usize;
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
// Tonal engine (melodic steps with randomness)
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

    fn next(&mut self, c: &TonalControls, timing: TimingContext) -> (f32, f32) {
        if self
            .trigger
            .pop(timing, c.step_interval_beats, c.offset_beats)
        {
            let degree = if self.rng.gen_range(0.0f32..1.0) < c.randomness {
                self.rng.gen_range(0..SCALE_HZ.len())
            } else {
                let d = PATTERN[self.step_index % PATTERN.len()];
                self.step_index += 1;
                d
            };
            let hz = SCALE_HZ[degree];
            let decay_samples = timing.beats_to_samples(c.note_length_beats);
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

    fn next(&mut self, c: &ClapControls, timing: TimingContext) -> (f32, f32) {
        if self.trigger.pop(timing, c.interval_beats, c.offset_beats) {
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

    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < f32::EPSILON,
            "expected {expected}, got {actual}"
        );
    }

    fn timing(sample: u64, bpm: f32) -> TimingContext {
        let sample_rate = f64::from(SAMPLE_RATE);
        let bpm = f64::from(bpm);
        let beat = sample as f64 * bpm / (60.0 * sample_rate);
        TimingContext::new(sample_rate, bpm, beat)
    }

    #[test]
    fn midi_to_hz_matches_known_notes() {
        assert_close(midi_to_hz(69), 440.0); // A4
        assert_close(midi_to_hz(45), 110.0); // A2
        assert_close(midi_to_hz(60), 440.0 * 2f32.powf((60.0 - 69.0) / 12.0)); // C4
    }

    #[test]
    fn pad_chord_converts_progression_a_first_chord() {
        let chord = pad_chord(0, 0);
        assert_close(chord[0], 110.0); // A2
        assert_close(chord[1], 440.0 * 2f32.powf((50.0 - 69.0) / 12.0)); // D3
        assert_close(chord[2], 440.0 * 2f32.powf((55.0 - 69.0) / 12.0)); // G3
        assert_close(chord[3], 440.0 * 2f32.powf((60.0 - 69.0) / 12.0)); // C4
    }

    #[test]
    fn pad_chord_converts_progression_d_last_chord() {
        let chord = pad_chord(3, 7);
        assert_close(chord[0], 440.0 * 2f32.powf((43.0 - 69.0) / 12.0)); // G2
        assert_close(chord[1], 440.0 * 2f32.powf((50.0 - 69.0) / 12.0)); // D3
        assert_close(chord[2], 440.0 * 2f32.powf((55.0 - 69.0) / 12.0)); // G3
        assert_close(chord[3], 440.0 * 2f32.powf((64.0 - 69.0) / 12.0)); // E4
    }

    #[test]
    fn pad_chord_wraps_progression_and_step_index() {
        let wrapped_progression = pad_chord(4, 0);
        let base_progression = pad_chord(0, 0);
        assert_eq!(wrapped_progression, base_progression);

        let wrapped_step = pad_chord(0, 8);
        let base_step = pad_chord(0, 0);
        assert_eq!(wrapped_step, base_step);
    }

    #[test]
    fn pad_defaults_use_progression_a_and_eight_bar_chords() {
        let controls = PadControls::default();
        assert_close(controls.chord_bars, 8.0);
        assert_close(controls.progression, 0.0);
    }

    #[test]
    fn tab_previous_wraps_back_one_tab() {
        assert_eq!(Tab::Master.previous(), Tab::Clap);
        assert_eq!(Tab::Kick.previous(), Tab::Bass);
        assert_eq!(Tab::Bass.previous(), Tab::Chords);
    }

    #[test]
    fn render_fluid_draws_without_terminal_backend() {
        let controls = FluidControls::default();
        let fluid = FluidState::new();
        let backend = TestBackend::new(100, 32);
        let mut terminal = Terminal::new(backend).unwrap();
        let items = tab_controls(Tab::Master, &controls);

        terminal
            .draw(|f| render(f, &items, Tab::Master, 0, &fluid))
            .unwrap();
    }

    #[test]
    fn defaults_match_current_mix() {
        let controls = FluidControls::default();

        assert_close(controls.master.bpm, 82.0);
        assert_close(controls.master.drive, 0.1);
        assert_close(controls.master.comp_threshold, -8.0);

        assert_close(controls.perc.decay_ms, 200.0);
        assert_close(controls.perc.filter, 0.7);
        assert_close(controls.perc.lfo_rate_bars, 1.0);
        assert_close(controls.perc.lfo_depth, 0.1);
        assert_close(controls.perc.interval_beats, 0.25);
        assert_close(controls.perc.offset_beats, 0.0);

        assert_close(controls.kick.start_freq, 160.0);
        assert_close(controls.kick.pitch_decay_ms, 55.0);
        assert_close(controls.kick.amp_decay_ms, 250.0);

        assert_close(controls.tonal.step_interval_beats, 2.5);
        assert_close(controls.tonal.note_length_beats, 1.5);
        assert_close(controls.tonal.randomness, 0.5);

        assert_close(controls.clap.room, 0.0);
    }

    #[test]
    fn apply_min_moves_selected_control_to_floor() {
        let mut controls = FluidControls::default();

        controls.master.drive = 0.8;
        apply_min(Tab::Master, 8, &mut controls);
        assert_close(controls.master.drive, 0.0);

        controls.master.bpm = 120.0;
        apply_min(Tab::Master, 6, &mut controls);
        assert_close(controls.master.bpm, MASTER_BPM_MIN);

        controls.master.tone = 0.5;
        apply_min(Tab::Master, 12, &mut controls);
        assert_close(controls.master.tone, -1.0);

        controls.pad.chord_bars = 16.0;
        apply_min(Tab::Chords, 1, &mut controls);
        assert_close(controls.pad.chord_bars, 1.0);
    }

    #[test]
    fn chords_tab_shows_progression_row_with_letter_display() {
        let mut controls = FluidControls::default();
        let rows = tab_controls(Tab::Chords, &controls);
        assert_eq!(rows[2].label, "Progression");
        assert_eq!(rows[2].display, "A");

        controls.pad.progression = 2.0;
        let rows = tab_controls(Tab::Chords, &controls);
        assert_eq!(rows[2].display, "C");
    }

    #[test]
    fn chords_progression_adjusts_and_clamps() {
        let mut controls = FluidControls::default();

        apply_delta(Tab::Chords, 2, 1.0, &mut controls);
        assert_close(controls.pad.progression, 1.0);

        controls.pad.progression = 3.0;
        apply_delta(Tab::Chords, 2, 1.0, &mut controls);
        assert_close(controls.pad.progression, 3.0);

        controls.pad.progression = 0.0;
        apply_delta(Tab::Chords, 2, -1.0, &mut controls);
        assert_close(controls.pad.progression, 0.0);

        controls.pad.progression = 2.0;
        apply_min(Tab::Chords, 2, &mut controls);
        assert_close(controls.pad.progression, 0.0);
    }

    #[test]
    fn bass_rhythms_have_expected_hit_counts() {
        assert_eq!(BASS_RHYTHMS[0].iter().filter(|&&b| b).count(), 4);
        assert!(BASS_RHYTHMS[0][0]);
        assert!(BASS_RHYTHMS[1].iter().filter(|&&b| b).count() > 4);
        assert_eq!(BASS_RHYTHMS[2].iter().filter(|&&b| b).count(), 8);
    }

    #[test]
    fn bass_root_note_matches_lowest_chord_tone() {
        assert_eq!(bass_root_note(0, 0), 45);
        assert_eq!(bass_root_note(2, 3), 43);
    }

    #[test]
    fn bass_defaults_are_silent_quarter_note_a() {
        let controls = BassControls::default();
        assert_close(controls.level, 0.0);
        assert_close(controls.rhythm, 0.0);
        assert_close(controls.octave, -1.0);
        assert_close(controls.interval_beats, 4.0);
    }

    #[test]
    fn bass_tab_shows_rhythm_row_with_letter_display() {
        let mut controls = FluidControls::default();
        let rows = tab_controls(Tab::Bass, &controls);
        assert_eq!(rows[3].label, "Rhythm");
        assert_eq!(rows[3].display, "A");

        controls.bass.rhythm = 3.0;
        let rows = tab_controls(Tab::Bass, &controls);
        assert_eq!(rows[3].display, "D");
    }

    #[test]
    fn bass_controls_adjust_and_clamp() {
        let mut controls = FluidControls::default();

        apply_delta(Tab::Bass, 3, 1.0, &mut controls);
        assert_close(controls.bass.rhythm, 1.0);

        controls.bass.rhythm = 3.0;
        apply_delta(Tab::Bass, 3, 1.0, &mut controls);
        assert_close(controls.bass.rhythm, 3.0);

        controls.bass.octave = -1.0;
        apply_delta(Tab::Bass, 4, -1.0, &mut controls);
        apply_delta(Tab::Bass, 4, -1.0, &mut controls);
        assert_close(controls.bass.octave, -3.0);

        apply_min(Tab::Bass, 0, &mut controls);
        assert_close(controls.bass.level, 0.0);

        controls.bass.decay_time = 0.4;
        apply_delta(Tab::Bass, 6, 1.0, &mut controls);
        assert!(controls.bass.decay_time > 0.4);

        apply_min(Tab::Bass, 6, &mut controls);
        assert_close(controls.bass.decay_time, 0.005);
    }

    #[test]
    fn bass_engine_follows_pad_chord_root_across_advances() {
        let sample_rate = 48_000.0;
        let mut bass = BassEngine::new(sample_rate);
        let pad = PadControls {
            chord_bars: 1.0 / 4.0, // advance every beat, fast enough to observe within the test
            ..PadControls::default()
        };
        let bass_controls = BassControls {
            interval_beats: 1.0,
            rhythm: 0.0,
            ..BassControls::default()
        };
        let mut clock = TempoClock::new(sample_rate, 120.0);

        // Step far enough to guarantee at least one chord advance and one
        // rhythm hit have occurred.
        for _ in 0..(sample_rate as usize) {
            let timing = clock.tick(120.0);
            bass.next(&bass_controls, &pad, timing);
        }

        assert_ne!(bass.step_index, 0);
        assert!(bass.rhythm_step < BASS_RHYTHMS[0].len());
    }

    #[test]
    fn bass_voice_decays_to_silence_without_sustaining() {
        let sample_rate = 48_000.0;
        let mut voice = BassVoice::new(110.0, 0.005, 0.05, 0.0, sample_rate);

        // Well past attack+decay (0.055s); a sustaining envelope would still
        // be holding at ~0.85 gain here, an AD envelope has decayed to ~0.
        for _ in 0..(sample_rate * 0.5) as usize {
            voice.next();
        }

        let (l, r) = voice.next();
        assert!(l.abs() < 0.001 && r.abs() < 0.001);
    }

    #[test]
    fn bass_interval_crops_phrase_instead_of_stretching_it() {
        // Step duration is always a fixed 16th note; `interval_beats` only
        // decides how many steps of the 16-step phrase play before looping
        // back to step 0.
        let hits_within = |rhythm: usize, loop_len: usize| -> Vec<usize> {
            (0..loop_len)
                .filter(|&s| s < BASS_RHYTHMS[rhythm].len() && BASS_RHYTHMS[rhythm][s])
                .collect()
        };

        // Progression A (quarter notes) hits every 4 steps; cropping to a
        // 4-step (1 beat) loop still only exposes step 0, which recurs at
        // the same cadence as the full 16-step phrase - no audible change.
        assert_eq!(hits_within(0, 16), vec![0, 4, 8, 12]);
        assert_eq!(hits_within(0, 4), vec![0]);
        assert_eq!(hits_within(0, 8), vec![0, 4]);

        // A busier pattern's crop is audibly different: only its first half
        // survives an 8-step (2 beat) loop.
        let full = hits_within(1, 16);
        let cropped = hits_within(1, 8);
        assert!(cropped.len() < full.len());
        assert!(cropped.iter().all(|s| full.contains(s)));
    }

    #[test]
    fn chords_reverb_mix_row_shifted_to_index_three() {
        let controls = FluidControls::default();
        let rows = tab_controls(Tab::Chords, &controls);
        assert_eq!(rows[3].label, "Reverb Mix");
    }

    #[test]
    fn chords_release_row_present_with_lowered_attack_floor() {
        let controls = FluidControls::default();
        let rows = tab_controls(Tab::Chords, &controls);
        assert_eq!(rows[7].label, "Attack");
        assert_close(rows[7].min, 0.05);
        assert_eq!(rows[8].label, "Release");
        assert_close(rows[8].value, 8.0);
        assert_close(rows[8].min, 0.05);
        assert_close(rows[8].max, 20.0);
    }

    #[test]
    fn chords_attack_and_release_adjust_and_clamp_low() {
        let mut controls = FluidControls::default();

        controls.pad.attack_time = 0.1;
        apply_delta(Tab::Chords, 7, -1.0, &mut controls);
        assert_close(controls.pad.attack_time, 0.05);
        apply_min(Tab::Chords, 7, &mut controls);
        assert_close(controls.pad.attack_time, 0.05);

        controls.pad.release_time = 0.1;
        apply_delta(Tab::Chords, 8, -1.0, &mut controls);
        assert_close(controls.pad.release_time, 0.05);
        apply_min(Tab::Chords, 8, &mut controls);
        assert_close(controls.pad.release_time, 0.05);
    }

    #[test]
    fn kick_interval_floor_is_quarter_beat() {
        let mut controls = FluidControls::default();
        controls.kick.interval_beats = 1.0;
        apply_min(Tab::Kick, 1, &mut controls);
        assert_close(controls.kick.interval_beats, 0.25);

        controls.kick.interval_beats = 0.25;
        apply_delta(Tab::Kick, 1, -1.0, &mut controls);
        assert_close(controls.kick.interval_beats, 0.25);
    }

    #[test]
    fn perc_continuous_mode_pushes_no_hits() {
        let mut controls = PercControls::default();
        controls.level = 1.0;
        controls.interval_beats = 4.25;

        let mut engine = PercEngine::new(SAMPLE_RATE);
        let bpm = 82.0;
        for sample in 0..(SAMPLE_RATE as u64 * 2) {
            let t = timing(sample, bpm);
            engine.next(&controls, t);
        }
        assert!(engine.hits.is_empty());
    }

    #[test]
    fn perc_continuous_mode_has_no_periodic_rms_dips() {
        let mut controls = PercControls::default();
        controls.level = 1.0;
        controls.lfo_depth = 0.0;
        controls.interval_beats = 4.25;

        let mut engine = PercEngine::new(SAMPLE_RATE);
        let bpm = 82.0;
        let window_samples = (SAMPLE_RATE * 0.01) as usize;
        let total_samples = SAMPLE_RATE as usize * 2;
        let mut window_rms = Vec::new();
        let mut window = Vec::with_capacity(window_samples);

        for sample in 0..total_samples as u64 {
            let t = timing(sample, bpm);
            let out = engine.next(&controls, t);
            window.push(out);
            if window.len() == window_samples {
                let sum_sq: f32 = window.iter().map(|x| x * x).sum();
                window_rms.push((sum_sq / window.len() as f32).sqrt());
                window.clear();
            }
        }

        let settle_windows = (SAMPLE_RATE * 0.25) as usize / window_samples;
        let rms_tail = &window_rms[settle_windows..];

        let min_rms = rms_tail.iter().cloned().fold(f32::INFINITY, f32::min);
        let max_rms = rms_tail.iter().cloned().fold(f32::NEG_INFINITY, f32::max);

        assert!(min_rms > 0.0, "continuous mode produced silence in a window");
        assert!(
            max_rms / min_rms < 2.0,
            "windowed RMS varies too much ({min_rms}..{max_rms}), suggests periodic triggering survived"
        );
    }

    #[test]
    fn perc_tab_controls_include_interval_and_offset() {
        let controls = FluidControls::default();
        let rows = tab_controls(Tab::Perc, &controls);
        assert_eq!(rows.len(), 7);
        assert_eq!(rows[1].label, "Interval");
        assert_close(rows[1].min, 0.25);
        assert_close(rows[1].max, 4.25);
        assert_eq!(rows[2].label, "Offset");
        assert_close(rows[2].min, 0.0);
        assert_close(rows[2].max, 4.0);
    }

    #[test]
    fn perc_interval_displays_continuous_at_top() {
        let mut controls = FluidControls::default();
        controls.perc.interval_beats = 4.25;
        let rows = tab_controls(Tab::Perc, &controls);
        assert_eq!(rows[1].display, "Continuous");
    }

    #[test]
    fn perc_interval_and_offset_adjust_and_clamp() {
        let mut controls = FluidControls::default();

        apply_delta(Tab::Perc, 1, 1.0, &mut controls);
        assert_close(controls.perc.interval_beats, 0.5);

        controls.perc.interval_beats = 4.25;
        apply_delta(Tab::Perc, 1, 1.0, &mut controls);
        assert_close(controls.perc.interval_beats, 4.25);

        apply_delta(Tab::Perc, 2, 1.0, &mut controls);
        assert_close(controls.perc.offset_beats, 0.25);

        controls.perc.offset_beats = 4.0;
        apply_delta(Tab::Perc, 2, 1.0, &mut controls);
        assert_close(controls.perc.offset_beats, 4.0);

        apply_min(Tab::Perc, 1, &mut controls);
        assert_close(controls.perc.interval_beats, 0.25);

        apply_min(Tab::Perc, 2, &mut controls);
        assert_close(controls.perc.offset_beats, 0.0);
    }

    #[test]
    fn pad_engine_caps_released_layers() {
        let controls = PadControls {
            chord_bars: 1.0,
            attack_time: 1.0,
            ..PadControls::default()
        };
        let mut pad = PadEngine::new(SAMPLE_RATE, &controls, Arc::new(FluidTelemetry::default()));

        for chord in 1..12 {
            let sample = chord * SAMPLE_RATE as u64 * 2;
            let _ = pad.next(&controls, timing(sample, 120.0));
            assert!(pad.layers.len() <= MAX_PAD_LAYERS);
        }
    }

    #[test]
    fn pad_engine_step_index_wraps_at_eight() {
        let controls = PadControls {
            chord_bars: 1.0,
            attack_time: 1.0,
            ..PadControls::default()
        };
        let mut pad = PadEngine::new(SAMPLE_RATE, &controls, Arc::new(FluidTelemetry::default()));

        // chord_bars=1.0 means chord_trigger fires every 4.0 beats; at 120 BPM
        // that's 2 seconds of samples per chord. Render 9 chord-advances worth
        // of samples (18 seconds) and confirm the telemetry index wrapped past 8.
        for chord in 1..=9 {
            let sample = chord * SAMPLE_RATE as u64 * 2;
            let _ = pad.next(&controls, timing(sample, 120.0));
        }
        let final_index = pad.telemetry.chord_index.load(Ordering::Relaxed);
        assert!(final_index < 8, "step_index must wrap into 0..8, got {final_index}");
    }

    #[test]
    fn pad_engine_progression_switch_retriggers_immediately() {
        let mut controls = PadControls {
            chord_bars: 64.0, // long chord length so no chord-advance trigger fires
            // Short attack so the original layer's envelope is already audible
            // (not still ~0 from the very first sample) by the time it's released;
            // otherwise the release phase completes in the same tick it starts and
            // `retain` prunes it before this test can observe the pushed layer.
            attack_time: 0.001,
            ..PadControls::default()
        };
        let mut pad = PadEngine::new(SAMPLE_RATE, &controls, Arc::new(FluidTelemetry::default()));

        // Warm up the original layer's envelope (still progression 0, so no push
        // happens here) so its level is non-negligible before it gets released;
        // otherwise the Adsr release completes in the same tick it starts and
        // `retain` prunes the layer before this test can observe the pushed one.
        for sample in 0..10 {
            let _ = pad.next(&controls, timing(sample, 120.0));
        }
        let layers_before = pad.layers.len();

        // Flip progression with no further elapsed time / no chord-advance trigger.
        controls.progression = 1.0;
        let _ = pad.next(&controls, timing(10, 120.0));

        assert!(
            pad.layers.len() > layers_before,
            "switching progression must push a new layer immediately, without waiting for chord_trigger"
        );
    }

    #[test]
    fn kick_delay_buffer_covers_max_echo_at_min_bpm() {
        let max_delay =
            ((KICK_ECHO_TIME_BEATS_MAX * 60.0 / MASTER_BPM_MIN) * SAMPLE_RATE).ceil() as usize;
        let delay = KickDelay::new(max_kick_echo_delay_samples(SAMPLE_RATE));

        assert_eq!(delay.buf_l.len(), max_delay + 1);
    }

    #[test]
    fn tempo_clock_preserves_beat_phase_when_bpm_changes() {
        let mut clock = TempoClock::new(SAMPLE_RATE, 120.0);
        let mut before = clock.tick(120.0);

        for _ in 1..20_000 {
            before = clock.tick(120.0);
        }

        let after = clock.tick(60.0);

        assert!(after.beat > before.beat);
        assert!(after.beat - before.beat < 0.001);
        assert!(after.bpm < before.bpm);
        assert!(after.bpm > 60.0);
    }

    #[test]
    fn grid_trigger_keeps_next_hit_when_only_bpm_changes() {
        let mut clock = TempoClock::new(SAMPLE_RATE, 120.0);
        let mut trigger = GridTrigger::new();

        for _ in 0..25_000 {
            let timing = clock.tick(120.0);
            let _ = trigger.pop(timing, 1.0, 0.0);
        }

        let before = trigger.next_hit.map(|hit| hit.beat);
        let timing = clock.tick(60.0);
        let fired = trigger.pop(timing, 1.0, 0.0);
        let after = trigger.next_hit.map(|hit| hit.beat);

        assert!(!fired);
        assert_eq!(before, after);
    }

    #[test]
    fn grid_trigger_fires_identically_for_same_params() {
        let mut a = GridTrigger::new();
        let mut b = GridTrigger::new();
        let mut a_hits = Vec::new();
        let mut b_hits = Vec::new();

        for sample in 0..(SAMPLE_RATE as u64 * 6) {
            let timing = timing(sample, 120.0);
            if a.pop(timing, 2.0, 1.0) {
                a_hits.push(sample);
            }
            if b.pop(timing, 2.0, 1.0) {
                b_hits.push(sample);
            }
        }

        assert!(a_hits.len() >= 3);
        assert_eq!(a_hits, b_hits);
    }

    #[test]
    fn grid_trigger_no_silence_after_bpm_decrease() {
        let change_at = 50_000u64;
        let mut clock = TempoClock::new(SAMPLE_RATE, 120.0);
        let mut kick = GridTrigger::new();
        let mut clap = GridTrigger::new();
        let mut kick_hits: Vec<u64> = Vec::new();
        let mut clap_hits: Vec<u64> = Vec::new();

        for sample in 0..change_at {
            let timing = clock.tick(120.0);
            if kick.pop(timing, 1.0, 0.0) {
                kick_hits.push(sample);
            }
            if clap.pop(timing, 2.0, 1.0) {
                clap_hits.push(sample);
            }
        }

        for sample in change_at..(SAMPLE_RATE as u64 * 8) {
            let timing = clock.tick(60.0);
            if kick.pop(timing, 1.0, 0.0) {
                kick_hits.push(sample);
            }
            if clap.pop(timing, 2.0, 1.0) {
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
    fn grid_trigger_no_silence_after_interval_increase() {
        let change_at = 50_000u64;
        let mut trigger = GridTrigger::new();
        let mut hits: Vec<u64> = Vec::new();

        for sample in 0..change_at {
            if trigger.pop(timing(sample, 120.0), 0.5, 0.0) {
                hits.push(sample);
            }
        }

        for sample in change_at..(SAMPLE_RATE as u64 * 8) {
            if trigger.pop(timing(sample, 120.0), 4.0, 0.0) {
                hits.push(sample);
            }
        }

        let new_interval_samples = (4.0 * 60.0 / 120.0 * SAMPLE_RATE) as u64;
        let first_post = hits.iter().copied().find(|&s| s >= change_at);
        assert!(
            first_post.is_some_and(|s| s - change_at <= new_interval_samples),
            "trigger stalled after interval increase"
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
