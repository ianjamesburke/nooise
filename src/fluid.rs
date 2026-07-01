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
const LEVEL_RAMP_MS: f32 = 100.0;

#[derive(Clone)]
pub(crate) struct MasterControls {
    pub bpm: f32,
    pub level: f32,
    pub drive: f32,
    pub comp_threshold: f32,  // dB, -40 to 0
    pub comp_ratio: f32,      // 1-8
    pub comp_release_ms: f32, // 10-500
    pub tone: f32,            // -1 (bass) to +1 (treble)
    pub tune: f32,            // semitones, -12 (1 octave down) to +12 (1 octave up)
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
            tune: 0.0,
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
    pub filter: f32,
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
            filter: 0.7,
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
    kind: ControlKind,
    value: f32,
    min: f32,
    max: f32,
    display: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ControlKind {
    Gain,
    Continuous,
    Timing,
    Discrete,
}

impl ControlKind {
    fn smooths_audio(self) -> bool {
        matches!(self, Self::Gain)
    }
}

#[derive(Default)]
struct NumericEntry {
    buffer: String,
}

impl NumericEntry {
    fn push(&mut self, c: char) {
        match c {
            '0'..='9' => self.buffer.push(c),
            '.' if !self.buffer.contains('.') => self.buffer.push(c),
            '-' if self.buffer.is_empty() => self.buffer.push(c),
            _ => {}
        }
    }

    fn is_complete_number(&self) -> bool {
        !self.buffer.is_empty() && self.buffer != "." && self.buffer != "-"
    }
}

// ============================================================
// Control registry
//
// Single source of truth for every UI control row. Each row is one
// ControlSpec: range, step, numeric-entry semantics, reset target,
// accessors, and display formatting. tab_controls / apply_delta /
// apply_min / apply_value all derive from these tables — adding a
// control means adding one entry here.
// ============================================================

type GetFn = fn(&FluidControls) -> f32;
type SetFn = fn(&mut FluidControls, f32);
type DisplayFn = fn(&FluidControls) -> String;

/// How left/right adjustment moves the value.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Step {
    /// value += dir * step, clamped to [min, max].
    Linear(f32),
    /// value doubles/halves, clamped to [min, max].
    PowerOfTwo,
}

/// How direct numeric entry is interpreted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Entry {
    /// Unit or percent input, scaled to [0, max] (e.g. 42 → 0.42 * max).
    Percent,
    /// Rounded to the nearest integer.
    Round,
    /// Snapped to the control's step grid.
    Snap,
    /// Used as-is (clamped only).
    Free,
}

/// How the value maps onto the ratio bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Bar {
    Linear,
    /// Bar position is log2-scaled (for power-of-two ranges).
    Log2,
}

struct ControlSpec {
    label: &'static str,
    kind: ControlKind,
    min: f32,
    max: f32,
    step: Step,
    entry: Entry,
    reset: f32,
    bar: Bar,
    get: GetFn,
    set: SetFn,
    display: DisplayFn,
}

impl ControlSpec {
    #[allow(clippy::too_many_arguments)]
    const fn new(
        label: &'static str,
        kind: ControlKind,
        min: f32,
        max: f32,
        step: Step,
        entry: Entry,
        get: GetFn,
        set: SetFn,
        display: DisplayFn,
    ) -> Self {
        Self {
            label,
            kind,
            min,
            max,
            step,
            entry,
            reset: min,
            bar: Bar::Linear,
            get,
            set,
            display,
        }
    }

    /// Gain-kind control: 2% steps, percent-style numeric entry, resets to min.
    const fn gain(
        label: &'static str,
        min: f32,
        max: f32,
        get: GetFn,
        set: SetFn,
        display: DisplayFn,
    ) -> Self {
        Self::new(
            label,
            ControlKind::Gain,
            min,
            max,
            Step::Linear(0.02),
            Entry::Percent,
            get,
            set,
            display,
        )
    }

    const fn with_step(mut self, step: f32) -> Self {
        self.step = Step::Linear(step);
        self
    }

    const fn reset_at(mut self, reset: f32) -> Self {
        self.reset = reset;
        self
    }

    const fn log_bar(mut self) -> Self {
        self.bar = Bar::Log2;
        self
    }

    fn item(&self, c: &FluidControls) -> ControlItem {
        let (value, min, max) = match self.bar {
            Bar::Linear => ((self.get)(c), self.min, self.max),
            Bar::Log2 => ((self.get)(c).log2(), self.min.log2(), self.max.log2()),
        };
        ControlItem {
            label: self.label,
            kind: self.kind,
            value,
            min,
            max,
            display: (self.display)(c),
        }
    }

    fn apply_delta(&self, dir: f32, c: &mut FluidControls) {
        let value = (self.get)(c);
        let next = match self.step {
            Step::Linear(step) => (value + dir * step).clamp(self.min, self.max),
            Step::PowerOfTwo => {
                if dir > 0.0 {
                    (value * 2.0).min(self.max)
                } else {
                    (value / 2.0).max(self.min)
                }
            }
        };
        (self.set)(c, next);
    }

    fn apply_min(&self, c: &mut FluidControls) {
        (self.set)(c, self.reset);
    }

    fn apply_value(&self, value: f32, c: &mut FluidControls) {
        let next = match self.entry {
            Entry::Percent => normalize_unit_input(value) * self.max,
            Entry::Round => value.round(),
            Entry::Snap => match self.step {
                Step::Linear(step) => snap_step(value, step),
                Step::PowerOfTwo => nearest_power_of_two(value, self.min, self.max),
            },
            Entry::Free => value,
        };
        (self.set)(c, next.clamp(self.min, self.max));
    }
}

fn pct(v: f32) -> String {
    format!("{:.0}%", v * 100.0)
}

fn beats2(v: f32) -> String {
    format!("{v:.2} beats")
}

fn ms0(v: f32) -> String {
    format!("{v:.0} ms")
}

const MASTER_CONTROLS: &[ControlSpec] = &[
    ControlSpec::gain(
        "Chords Vol",
        0.0,
        1.0,
        |c| c.pad.level,
        |c, v| c.pad.level = v,
        |c| pct(c.pad.level),
    ),
    ControlSpec::gain(
        "Perc Vol",
        0.0,
        1.0,
        |c| c.perc.level,
        |c, v| c.perc.level = v,
        |c| pct(c.perc.level),
    ),
    ControlSpec::gain(
        "Kick Vol",
        0.0,
        1.0,
        |c| c.kick.level,
        |c, v| c.kick.level = v,
        |c| pct(c.kick.level),
    ),
    ControlSpec::gain(
        "Tonal Vol",
        0.0,
        1.0,
        |c| c.tonal.level,
        |c, v| c.tonal.level = v,
        |c| pct(c.tonal.level),
    ),
    ControlSpec::gain(
        "Clap Vol",
        0.0,
        1.0,
        |c| c.clap.level,
        |c, v| c.clap.level = v,
        |c| pct(c.clap.level),
    ),
    ControlSpec::gain(
        "Bass Vol",
        0.0,
        1.0,
        |c| c.bass.level,
        |c, v| c.bass.level = v,
        |c| pct(c.bass.level),
    ),
    ControlSpec::new(
        "BPM",
        ControlKind::Timing,
        MASTER_BPM_MIN,
        MASTER_BPM_MAX,
        Step::Linear(1.0),
        Entry::Round,
        |c| c.master.bpm,
        |c, v| c.master.bpm = v,
        |c| format!("{:.0} bpm", c.master.bpm),
    ),
    ControlSpec::gain(
        "Master Level",
        0.0,
        1.0,
        |c| c.master.level,
        |c, v| c.master.level = v,
        |c| pct(c.master.level),
    ),
    ControlSpec::gain(
        "Drive",
        0.0,
        1.0,
        |c| c.master.drive,
        |c, v| c.master.drive = v,
        |c| pct(c.master.drive),
    ),
    ControlSpec::new(
        "Comp Threshold",
        ControlKind::Continuous,
        -40.0,
        0.0,
        Step::Linear(1.0),
        Entry::Round,
        |c| c.master.comp_threshold,
        |c, v| c.master.comp_threshold = v,
        |c| format!("{:.0} dB", c.master.comp_threshold),
    ),
    ControlSpec::new(
        "Comp Ratio",
        ControlKind::Continuous,
        1.0,
        8.0,
        Step::Linear(0.25),
        Entry::Snap,
        |c| c.master.comp_ratio,
        |c, v| c.master.comp_ratio = v,
        |c| format!("{:.1}:1", c.master.comp_ratio),
    ),
    ControlSpec::new(
        "Comp Release",
        ControlKind::Timing,
        10.0,
        500.0,
        Step::Linear(10.0),
        Entry::Snap,
        |c| c.master.comp_release_ms,
        |c, v| c.master.comp_release_ms = v,
        |c| ms0(c.master.comp_release_ms),
    ),
    ControlSpec::new(
        "Tone",
        ControlKind::Continuous,
        -1.0,
        1.0,
        Step::Linear(0.05),
        Entry::Free,
        |c| c.master.tone,
        |c, v| c.master.tone = v,
        |c| {
            if c.master.tone < -0.05 {
                format!("bass {:.0}%", -c.master.tone * 100.0)
            } else if c.master.tone > 0.05 {
                format!("treble {:.0}%", c.master.tone * 100.0)
            } else {
                "flat".to_string()
            }
        },
    ),
    ControlSpec::new(
        "Tune",
        ControlKind::Discrete,
        -12.0,
        12.0,
        Step::Linear(1.0),
        Entry::Round,
        |c| c.master.tune,
        |c, v| c.master.tune = v,
        |c| {
            if c.master.tune.abs() < 0.05 {
                "0 st".to_string()
            } else {
                format!("{:+.0} st", c.master.tune)
            }
        },
    )
    .reset_at(0.0),
];

const PERC_CONTROLS: &[ControlSpec] = &[
    ControlSpec::gain(
        "Level",
        0.0,
        1.0,
        |c| c.perc.level,
        |c, v| c.perc.level = v,
        |c| pct(c.perc.level),
    ),
    ControlSpec::new(
        "Interval",
        ControlKind::Timing,
        0.25,
        4.25,
        Step::Linear(0.25),
        Entry::Snap,
        |c| c.perc.interval_beats,
        |c, v| c.perc.interval_beats = v,
        |c| {
            if c.perc.interval_beats >= 4.25 {
                "Continuous".to_string()
            } else {
                beats2(c.perc.interval_beats)
            }
        },
    ),
    ControlSpec::new(
        "Offset",
        ControlKind::Timing,
        0.0,
        4.0,
        Step::Linear(0.25),
        Entry::Snap,
        |c| c.perc.offset_beats,
        |c, v| c.perc.offset_beats = v,
        |c| beats2(c.perc.offset_beats),
    ),
    ControlSpec::new(
        "Decay",
        ControlKind::Timing,
        20.0,
        2000.0,
        Step::Linear(20.0),
        Entry::Free,
        |c| c.perc.decay_ms,
        |c, v| c.perc.decay_ms = v,
        |c| {
            if c.perc.decay_ms >= 1000.0 {
                format!("{:.1} s", c.perc.decay_ms / 1000.0)
            } else {
                ms0(c.perc.decay_ms)
            }
        },
    ),
    ControlSpec::gain(
        "Filter",
        0.5,
        1.0,
        |c| c.perc.filter,
        |c, v| c.perc.filter = v,
        |c| pct(c.perc.filter),
    ),
    ControlSpec::new(
        "LFO Rate",
        ControlKind::Timing,
        0.25,
        16.0,
        Step::Linear(0.25),
        Entry::Snap,
        |c| c.perc.lfo_rate_bars,
        |c, v| c.perc.lfo_rate_bars = v,
        |c| format!("{:.0} beats", c.perc.lfo_rate_bars * 4.0),
    ),
    ControlSpec::gain(
        "LFO Depth",
        0.0,
        1.0,
        |c| c.perc.lfo_depth,
        |c, v| c.perc.lfo_depth = v,
        |c| pct(c.perc.lfo_depth),
    ),
];

const CHORDS_CONTROLS: &[ControlSpec] = &[
    ControlSpec::gain(
        "Level",
        0.0,
        1.0,
        |c| c.pad.level,
        |c, v| c.pad.level = v,
        |c| pct(c.pad.level),
    ),
    ControlSpec::new(
        "Chord Length",
        ControlKind::Timing,
        1.0,
        64.0,
        Step::PowerOfTwo,
        Entry::Snap,
        |c| c.pad.chord_bars,
        |c, v| c.pad.chord_bars = v,
        |c| format!("{:.0} beats", c.pad.chord_bars * 4.0),
    )
    .log_bar(),
    ControlSpec::new(
        "Progression",
        ControlKind::Discrete,
        0.0,
        3.0,
        Step::Linear(1.0),
        Entry::Round,
        |c| c.pad.progression,
        |c, v| c.pad.progression = v,
        |c| ["A", "B", "C", "D"][c.pad.progression.round() as usize % 4].to_string(),
    ),
    ControlSpec::gain(
        "Reverb Mix",
        0.0,
        1.0,
        |c| c.pad.reverb_mix,
        |c, v| c.pad.reverb_mix = v,
        |c| pct(c.pad.reverb_mix),
    ),
    ControlSpec::gain(
        "Stereo Width",
        0.0,
        1.0,
        |c| c.pad.stereo_width,
        |c, v| c.pad.stereo_width = v,
        |c| pct(c.pad.stereo_width),
    ),
    ControlSpec::gain(
        "Detune",
        0.0,
        1.0,
        |c| c.pad.detune,
        |c, v| c.pad.detune = v,
        |c| pct(c.pad.detune),
    ),
    ControlSpec::gain(
        "Octave Mix",
        0.0,
        1.0,
        |c| c.pad.octave_mix,
        |c, v| c.pad.octave_mix = v,
        |c| pct(c.pad.octave_mix),
    ),
    ControlSpec::new(
        "Attack",
        ControlKind::Timing,
        0.05,
        30.0,
        Step::Linear(0.5),
        Entry::Free,
        |c| c.pad.attack_time,
        |c, v| c.pad.attack_time = v,
        |c| format!("{:.2} s", c.pad.attack_time),
    ),
    ControlSpec::new(
        "Release",
        ControlKind::Timing,
        0.05,
        20.0,
        Step::Linear(0.5),
        Entry::Free,
        |c| c.pad.release_time,
        |c, v| c.pad.release_time = v,
        |c| format!("{:.2} s", c.pad.release_time),
    ),
];

const BASS_CONTROLS: &[ControlSpec] = &[
    ControlSpec::gain(
        "Level",
        0.0,
        1.0,
        |c| c.bass.level,
        |c, v| c.bass.level = v,
        |c| pct(c.bass.level),
    ),
    ControlSpec::new(
        "Interval",
        ControlKind::Timing,
        0.25,
        8.0,
        Step::Linear(0.25),
        Entry::Snap,
        |c| c.bass.interval_beats,
        |c, v| c.bass.interval_beats = v,
        |c| beats2(c.bass.interval_beats),
    ),
    ControlSpec::new(
        "Offset",
        ControlKind::Timing,
        0.0,
        4.0,
        Step::Linear(0.25),
        Entry::Snap,
        |c| c.bass.offset_beats,
        |c, v| c.bass.offset_beats = v,
        |c| beats2(c.bass.offset_beats),
    ),
    ControlSpec::new(
        "Rhythm",
        ControlKind::Discrete,
        0.0,
        3.0,
        Step::Linear(1.0),
        Entry::Round,
        |c| c.bass.rhythm,
        |c, v| c.bass.rhythm = v,
        |c| ["A", "B", "C", "D"][c.bass.rhythm.round() as usize % 4].to_string(),
    ),
    ControlSpec::new(
        "Octave",
        ControlKind::Discrete,
        -3.0,
        0.0,
        Step::Linear(1.0),
        Entry::Round,
        |c| c.bass.octave,
        |c, v| c.bass.octave = v,
        |c| format!("{:.0}", c.bass.octave),
    ),
    ControlSpec::new(
        "Attack",
        ControlKind::Timing,
        0.005,
        1.0,
        Step::Linear(0.02),
        Entry::Free,
        |c| c.bass.attack_time,
        |c, v| c.bass.attack_time = v,
        |c| format!("{:.3} s", c.bass.attack_time),
    ),
    ControlSpec::new(
        "Decay",
        ControlKind::Timing,
        0.005,
        2.0,
        Step::Linear(0.05),
        Entry::Free,
        |c| c.bass.decay_time,
        |c, v| c.bass.decay_time = v,
        |c| format!("{:.3} s", c.bass.decay_time),
    ),
    ControlSpec::gain(
        "Drive",
        0.0,
        1.0,
        |c| c.bass.drive,
        |c, v| c.bass.drive = v,
        |c| pct(c.bass.drive),
    ),
];

const KICK_CONTROLS: &[ControlSpec] = &[
    ControlSpec::gain(
        "Level",
        0.0,
        1.0,
        |c| c.kick.level,
        |c, v| c.kick.level = v,
        |c| pct(c.kick.level),
    ),
    ControlSpec::new(
        "Interval",
        ControlKind::Timing,
        0.25,
        4.0,
        Step::Linear(0.25),
        Entry::Snap,
        |c| c.kick.interval_beats,
        |c, v| c.kick.interval_beats = v,
        |c| beats2(c.kick.interval_beats),
    ),
    ControlSpec::new(
        "Offset",
        ControlKind::Timing,
        0.0,
        4.0,
        Step::Linear(0.25),
        Entry::Snap,
        |c| c.kick.offset_beats,
        |c, v| c.kick.offset_beats = v,
        |c| beats2(c.kick.offset_beats),
    ),
    ControlSpec::new(
        "Start Freq",
        ControlKind::Continuous,
        40.0,
        200.0,
        Step::Linear(5.0),
        Entry::Snap,
        |c| c.kick.start_freq,
        |c, v| c.kick.start_freq = v,
        |c| format!("{:.0} Hz", c.kick.start_freq),
    ),
    ControlSpec::new(
        "Pitch Decay",
        ControlKind::Timing,
        10.0,
        300.0,
        Step::Linear(5.0),
        Entry::Snap,
        |c| c.kick.pitch_decay_ms,
        |c, v| c.kick.pitch_decay_ms = v,
        |c| ms0(c.kick.pitch_decay_ms),
    ),
    ControlSpec::new(
        "Amp Decay",
        ControlKind::Timing,
        50.0,
        1000.0,
        Step::Linear(20.0),
        Entry::Snap,
        |c| c.kick.amp_decay_ms,
        |c, v| c.kick.amp_decay_ms = v,
        |c| ms0(c.kick.amp_decay_ms),
    ),
    ControlSpec::gain(
        "Click",
        0.0,
        0.2,
        |c| c.kick.click,
        |c, v| c.kick.click = v,
        |c| pct(c.kick.click / 0.2),
    )
    .with_step(0.01),
    ControlSpec::gain(
        "Drive",
        0.0,
        1.0,
        |c| c.kick.drive,
        |c, v| c.kick.drive = v,
        |c| pct(c.kick.drive),
    ),
    ControlSpec::gain(
        "Filter",
        0.0,
        1.0,
        |c| c.kick.filter,
        |c, v| c.kick.filter = v,
        |c| pct(c.kick.filter),
    ),
    ControlSpec::new(
        "Echo Time",
        ControlKind::Timing,
        KICK_ECHO_TIME_BEATS_MIN,
        KICK_ECHO_TIME_BEATS_MAX,
        Step::Linear(0.125),
        Entry::Snap,
        |c| c.kick.echo_time_beats,
        |c, v| c.kick.echo_time_beats = v,
        |c| format!("{:.3} beats", c.kick.echo_time_beats),
    ),
    ControlSpec::gain(
        "Echo Filter",
        0.0,
        1.0,
        |c| c.kick.echo_filter,
        |c, v| c.kick.echo_filter = v,
        |c| pct(c.kick.echo_filter),
    ),
    ControlSpec::gain(
        "Echo Amount",
        0.0,
        0.9,
        |c| c.kick.echo_amount,
        |c, v| c.kick.echo_amount = v,
        |c| pct(c.kick.echo_amount / 0.9),
    ),
    ControlSpec::gain(
        "Echo Feedback",
        0.0,
        0.85,
        |c| c.kick.echo_feedback,
        |c, v| c.kick.echo_feedback = v,
        |c| pct(c.kick.echo_feedback / 0.85),
    ),
];

const TONAL_CONTROLS: &[ControlSpec] = &[
    ControlSpec::gain(
        "Level",
        0.0,
        1.0,
        |c| c.tonal.level,
        |c, v| c.tonal.level = v,
        |c| pct(c.tonal.level),
    ),
    ControlSpec::new(
        "Interval",
        ControlKind::Timing,
        0.5,
        4.0,
        Step::Linear(0.25),
        Entry::Snap,
        |c| c.tonal.step_interval_beats,
        |c, v| c.tonal.step_interval_beats = v,
        |c| beats2(c.tonal.step_interval_beats),
    ),
    ControlSpec::new(
        "Offset",
        ControlKind::Timing,
        0.0,
        4.0,
        Step::Linear(0.25),
        Entry::Snap,
        |c| c.tonal.offset_beats,
        |c, v| c.tonal.offset_beats = v,
        |c| beats2(c.tonal.offset_beats),
    ),
    ControlSpec::gain(
        "Randomness",
        0.0,
        1.0,
        |c| c.tonal.randomness,
        |c, v| c.tonal.randomness = v,
        |c| pct(c.tonal.randomness),
    ),
    ControlSpec::new(
        "Note Length",
        ControlKind::Timing,
        0.1,
        2.0,
        Step::Linear(0.05),
        Entry::Free,
        |c| c.tonal.note_length_beats,
        |c, v| c.tonal.note_length_beats = v,
        |c| beats2(c.tonal.note_length_beats),
    ),
    ControlSpec::gain(
        "Reverb Mix",
        0.0,
        1.0,
        |c| c.tonal.reverb_mix,
        |c, v| c.tonal.reverb_mix = v,
        |c| pct(c.tonal.reverb_mix),
    ),
];

const CLAP_CONTROLS: &[ControlSpec] = &[
    ControlSpec::gain(
        "Level",
        0.0,
        1.0,
        |c| c.clap.level,
        |c, v| c.clap.level = v,
        |c| pct(c.clap.level),
    ),
    ControlSpec::new(
        "Interval",
        ControlKind::Timing,
        0.5,
        8.0,
        Step::Linear(0.25),
        Entry::Snap,
        |c| c.clap.interval_beats,
        |c, v| c.clap.interval_beats = v,
        |c| beats2(c.clap.interval_beats),
    ),
    ControlSpec::new(
        "Offset",
        ControlKind::Timing,
        0.0,
        8.0,
        Step::Linear(0.25),
        Entry::Snap,
        |c| c.clap.offset_beats,
        |c, v| c.clap.offset_beats = v,
        |c| beats2(c.clap.offset_beats),
    ),
    ControlSpec::new(
        "Slap Count",
        ControlKind::Discrete,
        1.0,
        8.0,
        Step::Linear(1.0),
        Entry::Round,
        |c| c.clap.slap_count,
        |c, v| c.clap.slap_count = v,
        |c| format!("{:.0}", c.clap.slap_count),
    ),
    ControlSpec::new(
        "Slap Spread",
        ControlKind::Timing,
        0.0,
        100.0,
        Step::Linear(2.0),
        Entry::Snap,
        |c| c.clap.slap_spread_ms,
        |c, v| c.clap.slap_spread_ms = v,
        |c| format!("{:.1} ms", c.clap.slap_spread_ms),
    ),
    ControlSpec::new(
        "Decay",
        ControlKind::Timing,
        10.0,
        200.0,
        Step::Linear(5.0),
        Entry::Snap,
        |c| c.clap.decay_ms,
        |c, v| c.clap.decay_ms = v,
        |c| ms0(c.clap.decay_ms),
    ),
    ControlSpec::gain(
        "Filter",
        0.5,
        1.0,
        |c| c.clap.filter,
        |c, v| c.clap.filter = v,
        |c| pct(c.clap.filter),
    ),
    ControlSpec::gain(
        "Room",
        0.0,
        1.0,
        |c| c.clap.room,
        |c, v| c.clap.room = v,
        |c| pct(c.clap.room),
    ),
    ControlSpec::gain(
        "Body",
        0.0,
        1.0,
        |c| c.clap.body,
        |c, v| c.clap.body = v,
        |c| pct(c.clap.body),
    ),
];

fn tab_specs(tab: Tab) -> &'static [ControlSpec] {
    match tab {
        Tab::Master => MASTER_CONTROLS,
        Tab::Perc => PERC_CONTROLS,
        Tab::Chords => CHORDS_CONTROLS,
        Tab::Bass => BASS_CONTROLS,
        Tab::Kick => KICK_CONTROLS,
        Tab::Tonal => TONAL_CONTROLS,
        Tab::Clap => CLAP_CONTROLS,
    }
}

fn tab_controls(tab: Tab, c: &FluidControls) -> Vec<ControlItem> {
    tab_specs(tab).iter().map(|spec| spec.item(c)).collect()
}

fn apply_delta(tab: Tab, selected: usize, dir: f32, c: &mut FluidControls) {
    if let Some(spec) = tab_specs(tab).get(selected) {
        spec.apply_delta(dir, c);
    }
}

fn apply_min(tab: Tab, selected: usize, c: &mut FluidControls) {
    if let Some(spec) = tab_specs(tab).get(selected) {
        spec.apply_min(c);
    }
}

fn apply_value(tab: Tab, selected: usize, value: f32, c: &mut FluidControls) {
    if let Some(spec) = tab_specs(tab).get(selected) {
        spec.apply_value(value, c);
    }
}

fn normalize_unit_input(value: f32) -> f32 {
    if value > 1.0 {
        (value / 100.0).clamp(0.0, 1.0)
    } else {
        value.clamp(0.0, 1.0)
    }
}

fn snap_step(value: f32, step: f32) -> f32 {
    (value / step).round() * step
}

fn nearest_power_of_two(value: f32, min: f32, max: f32) -> f32 {
    let clamped = value.clamp(min, max);
    let exponent = clamped.log2().round();
    2.0f32.powf(exponent).clamp(min, max)
}

fn ui_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    controls: Arc<ArcSwap<FluidControls>>,
    telemetry: Arc<FluidTelemetry>,
) -> Result<(), Box<dyn Error>> {
    let mut tab = Tab::Master;
    let mut selected = 0usize;
    let mut numeric_entry: Option<NumericEntry> = None;
    let mut fluid = FluidState::new();
    let mut last = Instant::now();
    let started = Instant::now();

    loop {
        let c = FluidControls::clone(&controls.load());
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
                numeric_entry.as_ref().map(|entry| entry.buffer.as_str()),
                cursor_visible,
                &fluid,
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

fn set_value(controls: &Arc<ArcSwap<FluidControls>>, tab: Tab, selected: usize, value: f32) {
    let mut next = FluidControls::clone(&controls.load());
    apply_value(tab, selected, value, &mut next);
    controls.store(Arc::new(next));
}

fn render(
    f: &mut Frame,
    items: &[ControlItem],
    active_tab: Tab,
    selected: usize,
    numeric_entry: Option<&str>,
    cursor_visible: bool,
    fluid: &FluidState,
) {
    render_fluid(
        f,
        items,
        active_tab,
        selected,
        numeric_entry,
        cursor_visible,
        fluid,
    );
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
    numeric_entry: Option<&str>,
    cursor_visible: bool,
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
        let display = if active {
            if let Some(entry) = numeric_entry {
                let cursor = if cursor_visible { "_" } else { " " };
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
        if i + 1 < items.len() {
            rows.push(Line::from(""));
        }
    }
    f.render_widget(Paragraph::new(rows), layout[4]);

    f.render_widget(
        Paragraph::new("jk select   hl adjust   type value   Enter set   Esc cancel   q quit")
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
        let value = match item.kind {
            ControlKind::Discrete => item.value.round(),
            ControlKind::Gain | ControlKind::Continuous | ControlKind::Timing => item.value,
        };
        ((value - item.min) / range).clamp(0.0, 1.0)
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
    gain_smoothers: GainSmoothers,
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
            gain_smoothers: GainSmoothers::new(&snapshot),
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
            self.gain_smoothers
                .set_targets(&self.snapshot, self.sample_rate);
        }

        let fade = (self.current_sample as f32 / (self.sample_rate * 8.0)).min(1.0);
        let smoothed = self.gain_smoothers.next_controls(&self.snapshot);
        let timing = self.tempo.tick(smoothed.master.bpm);

        let tune = smoothed.master.tune;
        let (pad_l, pad_r) = self.pad.next(&smoothed.pad, tune, timing);
        let perc = self.perc.next(&smoothed.perc, timing);
        let (kick_l, kick_r) = self.kick.next(&smoothed.kick, timing);
        let (ton_l, ton_r) = self.tonal.next(&smoothed.tonal, timing);
        let (clap_l, clap_r) = self.clap.next(&smoothed.clap, timing);
        let (bass_l, bass_r) = self.bass.next(&smoothed.bass, &smoothed.pad, tune, timing);

        self.current_sample += 1;

        let raw_l =
            (pad_l + perc * 0.6 + kick_l * 0.7 + ton_l + clap_l * 0.65 + bass_l * 0.75) * fade;
        let raw_r =
            (pad_r + perc * 0.6 + kick_r * 0.7 + ton_r + clap_r * 0.65 + bass_r * 0.75) * fade;
        self.master_bus
            .process(raw_l, raw_r, &smoothed.master, self.sample_rate)
    }
}

struct GainSmoother {
    current: f32,
    target: f32,
    step: f32,
    samples_remaining: u32,
}

impl GainSmoother {
    fn new(value: f32) -> Self {
        Self {
            current: value,
            target: value,
            step: 0.0,
            samples_remaining: 0,
        }
    }

    fn set_target(&mut self, target: f32, ramp_samples: u32) {
        if (target - self.target).abs() <= f32::EPSILON {
            return;
        }
        self.target = target;
        self.samples_remaining = ramp_samples.max(1);
        self.step = (self.target - self.current) / self.samples_remaining as f32;
    }

    fn next(&mut self) -> f32 {
        if self.samples_remaining == 0 {
            self.current = self.target;
            return self.current;
        }
        self.current += self.step;
        self.samples_remaining -= 1;
        if self.samples_remaining == 0 {
            self.current = self.target;
        }
        self.current
    }
}

fn set_smooth_target(
    smoother: &mut GainSmoother,
    kind: ControlKind,
    target: f32,
    ramp_samples: u32,
) {
    if kind.smooths_audio() {
        smoother.set_target(target, ramp_samples);
    }
}

struct GainSmoothers {
    pad: GainSmoother,
    pad_reverb_mix: GainSmoother,
    pad_stereo_width: GainSmoother,
    pad_detune: GainSmoother,
    pad_octave_mix: GainSmoother,
    perc: GainSmoother,
    perc_filter: GainSmoother,
    perc_lfo_depth: GainSmoother,
    kick: GainSmoother,
    kick_echo_filter: GainSmoother,
    kick_echo_amount: GainSmoother,
    kick_echo_feedback: GainSmoother,
    tonal: GainSmoother,
    tonal_reverb_mix: GainSmoother,
    clap: GainSmoother,
    clap_room: GainSmoother,
    bass: GainSmoother,
    master: GainSmoother,
    master_drive: GainSmoother,
}

impl GainSmoothers {
    fn new(c: &FluidControls) -> Self {
        Self {
            pad: GainSmoother::new(c.pad.level),
            pad_reverb_mix: GainSmoother::new(c.pad.reverb_mix),
            pad_stereo_width: GainSmoother::new(c.pad.stereo_width),
            pad_detune: GainSmoother::new(c.pad.detune),
            pad_octave_mix: GainSmoother::new(c.pad.octave_mix),
            perc: GainSmoother::new(c.perc.level),
            perc_filter: GainSmoother::new(c.perc.filter),
            perc_lfo_depth: GainSmoother::new(c.perc.lfo_depth),
            kick: GainSmoother::new(c.kick.level),
            kick_echo_filter: GainSmoother::new(c.kick.echo_filter),
            kick_echo_amount: GainSmoother::new(c.kick.echo_amount),
            kick_echo_feedback: GainSmoother::new(c.kick.echo_feedback),
            tonal: GainSmoother::new(c.tonal.level),
            tonal_reverb_mix: GainSmoother::new(c.tonal.reverb_mix),
            clap: GainSmoother::new(c.clap.level),
            clap_room: GainSmoother::new(c.clap.room),
            bass: GainSmoother::new(c.bass.level),
            master: GainSmoother::new(c.master.level),
            master_drive: GainSmoother::new(c.master.drive),
        }
    }

    fn set_targets(&mut self, c: &FluidControls, sample_rate: f32) {
        let ramp_samples = (LEVEL_RAMP_MS * 0.001 * sample_rate).round() as u32;
        set_smooth_target(&mut self.pad, ControlKind::Gain, c.pad.level, ramp_samples);
        set_smooth_target(
            &mut self.pad_reverb_mix,
            ControlKind::Gain,
            c.pad.reverb_mix,
            ramp_samples,
        );
        set_smooth_target(
            &mut self.pad_stereo_width,
            ControlKind::Gain,
            c.pad.stereo_width,
            ramp_samples,
        );
        set_smooth_target(
            &mut self.pad_detune,
            ControlKind::Gain,
            c.pad.detune,
            ramp_samples,
        );
        set_smooth_target(
            &mut self.pad_octave_mix,
            ControlKind::Gain,
            c.pad.octave_mix,
            ramp_samples,
        );
        set_smooth_target(
            &mut self.perc,
            ControlKind::Gain,
            c.perc.level,
            ramp_samples,
        );
        set_smooth_target(
            &mut self.perc_filter,
            ControlKind::Gain,
            c.perc.filter,
            ramp_samples,
        );
        set_smooth_target(
            &mut self.perc_lfo_depth,
            ControlKind::Gain,
            c.perc.lfo_depth,
            ramp_samples,
        );
        set_smooth_target(
            &mut self.kick,
            ControlKind::Gain,
            c.kick.level,
            ramp_samples,
        );
        set_smooth_target(
            &mut self.kick_echo_filter,
            ControlKind::Gain,
            c.kick.echo_filter,
            ramp_samples,
        );
        set_smooth_target(
            &mut self.kick_echo_amount,
            ControlKind::Gain,
            c.kick.echo_amount,
            ramp_samples,
        );
        set_smooth_target(
            &mut self.kick_echo_feedback,
            ControlKind::Gain,
            c.kick.echo_feedback,
            ramp_samples,
        );
        set_smooth_target(
            &mut self.tonal,
            ControlKind::Gain,
            c.tonal.level,
            ramp_samples,
        );
        set_smooth_target(
            &mut self.tonal_reverb_mix,
            ControlKind::Gain,
            c.tonal.reverb_mix,
            ramp_samples,
        );
        set_smooth_target(
            &mut self.clap,
            ControlKind::Gain,
            c.clap.level,
            ramp_samples,
        );
        set_smooth_target(
            &mut self.clap_room,
            ControlKind::Gain,
            c.clap.room,
            ramp_samples,
        );
        set_smooth_target(
            &mut self.bass,
            ControlKind::Gain,
            c.bass.level,
            ramp_samples,
        );
        set_smooth_target(
            &mut self.master,
            ControlKind::Gain,
            c.master.level,
            ramp_samples,
        );
        set_smooth_target(
            &mut self.master_drive,
            ControlKind::Gain,
            c.master.drive,
            ramp_samples,
        );
    }

    fn next_controls(&mut self, c: &FluidControls) -> FluidControls {
        let mut next = c.clone();
        next.pad.level = self.pad.next();
        next.pad.reverb_mix = self.pad_reverb_mix.next();
        next.pad.stereo_width = self.pad_stereo_width.next();
        next.pad.detune = self.pad_detune.next();
        next.pad.octave_mix = self.pad_octave_mix.next();
        next.perc.level = self.perc.next();
        next.perc.filter = self.perc_filter.next();
        next.perc.lfo_depth = self.perc_lfo_depth.next();
        next.kick.level = self.kick.next();
        next.kick.echo_filter = self.kick_echo_filter.next();
        next.kick.echo_amount = self.kick_echo_amount.next();
        next.kick.echo_feedback = self.kick_echo_feedback.next();
        next.tonal.level = self.tonal.next();
        next.tonal.reverb_mix = self.tonal_reverb_mix.next();
        next.clap.level = self.clap.next();
        next.clap.room = self.clap_room.next();
        next.bass.level = self.bass.next();
        next.master.level = self.master.next();
        next.master.drive = self.master_drive.next();
        next
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
                0.0,
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

    fn next(&mut self, c: &PadControls, tune: f32, timing: TimingContext) -> (f32, f32) {
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
                tune,
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
        tune: f32,
        sample_rate: f32,
        attack_time: f32,
        release_time: f32,
    ) -> Self {
        Self {
            tones: pad_tones(
                progression,
                step,
                tune,
                sample_rate,
                attack_time,
                release_time,
            ),
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
    tune: f32,
    sample_rate: f32,
    attack_time: f32,
    release_time: f32,
) -> Vec<PadTone> {
    let freqs = pad_chord(progression, step, tune);
    let pans = [-0.52_f32, -0.18, 0.16, 0.46];
    let gains = [0.17_f32, 0.132, 0.126, 0.098];
    freqs
        .iter()
        .zip(pans)
        .zip(gains)
        .map(|((hz, pan), gain)| {
            PadTone::new(*hz, pan, gain, attack_time, release_time, sample_rate)
        })
        .collect()
}

fn midi_to_hz(note: i32) -> f32 {
    440.0 * 2f32.powf((note as f32 - 69.0) / 12.0)
}

/// Frequency multiplier for a master tune offset in semitones.
fn tune_ratio(semitones: f32) -> f32 {
    2f32.powf(semitones / 12.0)
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

fn pad_chord(progression: usize, step: usize, tune: f32) -> [f32; 4] {
    PROGRESSIONS[progression % PROGRESSIONS.len()][step % 8]
        .map(|note| midi_to_hz(note) * tune_ratio(tune))
}

/// Bass line for each progression, authored independently of the Pad's
/// chord voicings (one MIDI note per step, same 8-step indexing as
/// PROGRESSIONS). B/C/D currently mirror their chord's lowest tone; A
/// diverges deliberately (step 3 walks to G2 instead of following the
/// B-chord's root) to give the bass its own melodic movement.
const BASS_LINES: [[i32; 8]; 4] = [
    [45, 47, 45, 43, 52, 53, 45, 45], // A
    [45, 50, 48, 43, 41, 52, 45, 43], // B
    [45, 41, 48, 43, 50, 52, 47, 43], // C
    [45, 41, 48, 43, 50, 52, 47, 43], // D
];

fn bass_root_note(progression: usize, step: usize) -> i32 {
    BASS_LINES[progression % BASS_LINES.len()][step % 8]
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
        true, false, false, true, false, false, true, false, true, true, false, false, true, true,
        false, false,
    ],
    // C: straight eighths — steady walking-bass feel
    [
        true, false, true, false, true, false, true, false, true, false, true, false, true, false,
        true, false,
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

    fn next(
        &mut self,
        c: &BassControls,
        pad: &PadControls,
        tune: f32,
        timing: TimingContext,
    ) -> (f32, f32) {
        let progression = (pad.progression.round() as i64).rem_euclid(4) as usize;
        if self.chord_trigger.pop(timing, pad.chord_bars * 4.0, 0.0) {
            self.step_index = (self.step_index + 1) % 8;
        }

        let loop_len = (c.interval_beats / BASS_STEP_BEATS)
            .round()
            .clamp(1.0, 32.0) as usize;
        if self
            .step_trigger
            .pop(timing, BASS_STEP_BEATS, c.offset_beats)
        {
            self.rhythm_step = (self.rhythm_step + 1) % loop_len;
            let rhythm = (c.rhythm.round() as usize) % BASS_RHYTHMS.len();
            let hit = self.rhythm_step < BASS_RHYTHMS[rhythm].len()
                && BASS_RHYTHMS[rhythm][self.rhythm_step];
            if hit {
                let note =
                    bass_root_note(progression, self.step_index) + (c.octave.round() as i32) * 12;
                let hz = midi_to_hz(note) * tune_ratio(tune);
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
    mod_phase: f32,
    freq: f32,
    target_freq: f32,
    freq_glide: f32,
    amp: f32,
    amp_decay: f32,
    fm_depth: f32,
    fm_depth_decay: f32,
    lp_state: f32,
    lp_coeff: f32,
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
        // FM depth decays ~3x faster than pitch for a tight transient thud
        let fm_tau = (c.pitch_decay_ms * 0.001 * sample_rate / 9.0).max(1.0);
        Self {
            phase: 0.0,
            mod_phase: 0.0,
            freq: c.start_freq,
            target_freq: c.start_freq * 0.28,
            freq_glide: 1.0 / tau,
            amp: c.level,
            amp_decay: (-1.0 / amp_tau).exp(),
            fm_depth: 3.5,
            fm_depth_decay: (-1.0 / fm_tau).exp(),
            lp_state: 0.0,
            lp_coeff: 10_f32.powf(c.filter * 3.0 - 2.5).clamp(0.01, 0.99),
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

        // FM: modulator at 2x carrier freq, decaying depth
        let mod_freq = self.freq * 2.0;
        self.mod_phase += TAU * mod_freq / self.sample_rate;
        if self.mod_phase >= TAU {
            self.mod_phase -= TAU;
        }
        let fm = self.mod_phase.sin() * self.fm_depth * self.freq;
        self.fm_depth *= self.fm_depth_decay;

        self.phase += TAU * (self.freq + fm) / self.sample_rate;
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

        self.lp_state += self.lp_coeff * (s - self.lp_state);
        s = self.lp_state;

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
        let chord = pad_chord(0, 0, 0.0);
        assert_close(chord[0], 110.0); // A2
        assert_close(chord[1], 440.0 * 2f32.powf((50.0 - 69.0) / 12.0)); // D3
        assert_close(chord[2], 440.0 * 2f32.powf((55.0 - 69.0) / 12.0)); // G3
        assert_close(chord[3], 440.0 * 2f32.powf((60.0 - 69.0) / 12.0)); // C4
    }

    #[test]
    fn pad_chord_applies_master_tune_offset() {
        let flat = pad_chord(0, 0, 0.0);
        let up_octave = pad_chord(0, 0, 12.0);
        let down_octave = pad_chord(0, 0, -12.0);
        for i in 0..4 {
            assert_close(up_octave[i], flat[i] * 2.0);
            assert_close(down_octave[i], flat[i] * 0.5);
        }
    }

    #[test]
    fn pad_chord_converts_progression_d_last_chord() {
        let chord = pad_chord(3, 7, 0.0);
        assert_close(chord[0], 440.0 * 2f32.powf((43.0 - 69.0) / 12.0)); // G2
        assert_close(chord[1], 440.0 * 2f32.powf((50.0 - 69.0) / 12.0)); // D3
        assert_close(chord[2], 440.0 * 2f32.powf((55.0 - 69.0) / 12.0)); // G3
        assert_close(chord[3], 440.0 * 2f32.powf((64.0 - 69.0) / 12.0)); // E4
    }

    #[test]
    fn pad_chord_wraps_progression_and_step_index() {
        let wrapped_progression = pad_chord(4, 0, 0.0);
        let base_progression = pad_chord(0, 0, 0.0);
        assert_eq!(wrapped_progression, base_progression);

        let wrapped_step = pad_chord(0, 8, 0.0);
        let base_step = pad_chord(0, 0, 0.0);
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
            .draw(|f| render(f, &items, Tab::Master, 0, None, false, &fluid))
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
    fn apply_value_accepts_percent_style_unit_controls() {
        let mut controls = FluidControls::default();

        apply_value(Tab::Master, 7, 42.0, &mut controls);
        assert_close(controls.master.level, 0.42);

        apply_value(Tab::Master, 7, 0.7, &mut controls);
        assert_close(controls.master.level, 0.7);
    }

    #[test]
    fn apply_value_snaps_direct_numeric_entry_to_control_grid() {
        let mut controls = FluidControls::default();

        apply_value(Tab::Kick, 1, 1.13, &mut controls);
        assert_close(controls.kick.interval_beats, 1.25);

        apply_value(Tab::Chords, 1, 12.0, &mut controls);
        assert_close(controls.pad.chord_bars, 16.0);

        apply_value(Tab::Clap, 3, 3.6, &mut controls);
        assert_close(controls.clap.slap_count, 4.0);
    }

    #[test]
    fn tab_controls_classify_each_slider_kind() {
        use ControlKind::{Continuous, Discrete, Gain, Timing};

        let controls = FluidControls::default();
        let cases = [
            (
                Tab::Master,
                vec![
                    Gain, Gain, Gain, Gain, Gain, Gain, Timing, Gain, Gain, Continuous, Continuous,
                    Timing, Continuous, Discrete,
                ],
            ),
            (
                Tab::Perc,
                vec![Gain, Timing, Timing, Timing, Gain, Timing, Gain],
            ),
            (
                Tab::Chords,
                vec![
                    Gain, Timing, Discrete, Gain, Gain, Gain, Gain, Timing, Timing,
                ],
            ),
            (
                Tab::Bass,
                vec![
                    Gain, Timing, Timing, Discrete, Discrete, Timing, Timing, Gain,
                ],
            ),
            (
                Tab::Kick,
                vec![
                    Gain, Timing, Timing, Continuous, Timing, Timing, Gain, Gain, Gain, Timing,
                    Gain, Gain, Gain,
                ],
            ),
            (Tab::Tonal, vec![Gain, Timing, Timing, Gain, Timing, Gain]),
            (
                Tab::Clap,
                vec![
                    Gain, Timing, Timing, Discrete, Timing, Timing, Gain, Gain, Gain,
                ],
            ),
        ];

        for (tab, expected) in cases {
            let actual: Vec<_> = tab_controls(tab, &controls)
                .into_iter()
                .map(|item| item.kind)
                .collect();
            assert_eq!(actual, expected, "unexpected kind map for {}", tab.name());
        }
    }

    #[test]
    fn control_registry_specs_are_internally_consistent() {
        let tabs = [
            Tab::Master,
            Tab::Perc,
            Tab::Chords,
            Tab::Bass,
            Tab::Kick,
            Tab::Tonal,
            Tab::Clap,
        ];
        for tab in tabs {
            for spec in tab_specs(tab) {
                let ctx = format!("{} / {}", tab.name(), spec.label);
                assert!(!spec.label.is_empty(), "{ctx}: empty label");
                assert!(spec.min < spec.max, "{ctx}: min must be below max");
                assert!(
                    spec.reset >= spec.min && spec.reset <= spec.max,
                    "{ctx}: reset outside [min, max]"
                );
                if spec.bar == Bar::Log2 {
                    assert!(spec.min > 0.0, "{ctx}: log bar needs positive min");
                }
                if let Step::Linear(step) = spec.step {
                    assert!(step > 0.0, "{ctx}: step must be positive");
                }

                // get/set must address the same field.
                let mut c = FluidControls::default();
                (spec.set)(&mut c, spec.max);
                assert!(
                    ((spec.get)(&c) - spec.max).abs() < 1e-6,
                    "{ctx}: get/set roundtrip failed at max"
                );
                (spec.set)(&mut c, spec.reset);
                assert!(
                    ((spec.get)(&c) - spec.reset).abs() < 1e-6,
                    "{ctx}: get/set roundtrip failed at reset"
                );
            }
        }
    }

    #[test]
    fn control_kind_smoothing_policy_is_explicit() {
        assert!(ControlKind::Gain.smooths_audio());
        assert!(!ControlKind::Continuous.smooths_audio());
        assert!(!ControlKind::Timing.smooths_audio());
        assert!(!ControlKind::Discrete.smooths_audio());
    }

    #[test]
    fn gain_smoother_reaches_target_over_ramp() {
        let mut smoother = GainSmoother::new(0.0);
        smoother.set_target(1.0, 10);

        assert_close(smoother.next(), 0.1);
        for _ in 0..8 {
            smoother.next();
        }
        assert_close(smoother.next(), 1.0);
        assert_close(smoother.next(), 1.0);
    }

    #[test]
    fn gain_smoothers_ramp_live_gain_controls_without_timing_changes() {
        let mut controls = FluidControls::default();
        controls.pad.level = 0.0;
        controls.pad.reverb_mix = 0.0;
        controls.perc.filter = 0.5;
        controls.kick.echo_amount = 0.0;
        controls.master.level = 0.0;
        controls.master.drive = 0.0;

        let mut smoothers = GainSmoothers::new(&controls);
        controls.pad.level = 1.0;
        controls.pad.reverb_mix = 1.0;
        controls.perc.filter = 1.0;
        controls.kick.echo_amount = 0.9;
        controls.master.level = 0.5;
        controls.master.drive = 1.0;
        controls.master.bpm = 123.0;
        controls.bass.drive = 1.0;
        smoothers.set_targets(&controls, 100.0);

        let next = smoothers.next_controls(&controls);
        assert_close(next.master.bpm, 123.0);
        assert!(next.pad.level > 0.0 && next.pad.level < 1.0);
        assert!(next.pad.reverb_mix > 0.0 && next.pad.reverb_mix < 1.0);
        assert!(next.perc.filter > 0.5 && next.perc.filter < 1.0);
        assert!(next.kick.echo_amount > 0.0 && next.kick.echo_amount < 0.9);
        assert!(next.master.level > 0.0 && next.master.level < 0.5);
        assert!(next.master.drive > 0.0 && next.master.drive < 1.0);
        assert_close(next.bass.drive, 1.0);
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
    fn bass_root_note_follows_authored_bass_line() {
        assert_eq!(bass_root_note(0, 0), 45);
        // Progression A's authored line diverges from the chord's lowest
        // tone at step 3 (B chord's min is 47) — proves the bass line is
        // independent data, not derived from PROGRESSIONS.
        assert_eq!(bass_root_note(0, 3), 43);
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
            bass.next(&bass_controls, &pad, 0.0, timing);
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
        engine.rng = StdRng::seed_from_u64(7);
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
        engine.rng = StdRng::seed_from_u64(7);
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

        assert!(
            min_rms > 0.0,
            "continuous mode produced silence in a window"
        );
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
            let _ = pad.next(&controls, 0.0, timing(sample, 120.0));
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
            let _ = pad.next(&controls, 0.0, timing(sample, 120.0));
        }
        let final_index = pad.telemetry.chord_index.load(Ordering::Relaxed);
        assert!(
            final_index < 8,
            "step_index must wrap into 0..8, got {final_index}"
        );
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
            let _ = pad.next(&controls, 0.0, timing(sample, 120.0));
        }
        let layers_before = pad.layers.len();

        // Flip progression with no further elapsed time / no chord-advance trigger.
        controls.progression = 1.0;
        let _ = pad.next(&controls, 0.0, timing(10, 120.0));

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
