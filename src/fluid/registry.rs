use super::*;

// ============================================================
// UI
// ============================================================

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Tab {
    Master = 0,
    Perc = 1,
    Chords = 2,
    Bass = 3,
    Kick = 4,
    Tonal = 5,
    Clap = 6,
}

impl Tab {
    pub(crate) fn all() -> [Tab; 7] {
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

    pub(crate) fn name(self) -> &'static str {
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

    pub(crate) fn next(self) -> Self {
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

    pub(crate) fn previous(self) -> Self {
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

pub(crate) struct ControlItem {
    pub(crate) id: &'static str,
    pub(crate) label: &'static str,
    pub(crate) kind: ControlKind,
    pub(crate) value: f32,
    pub(crate) min: f32,
    pub(crate) max: f32,
    pub(crate) display: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ControlKind {
    Gain,
    Continuous,
    Timing,
    Discrete,
}

impl ControlKind {
    pub(crate) fn smooths_audio(self) -> bool {
        matches!(self, Self::Gain)
    }
}

#[derive(Default)]
pub(crate) struct NumericEntry {
    pub(crate) buffer: String,
}

impl NumericEntry {
    pub(crate) fn push(&mut self, c: char) {
        match c {
            '0'..='9' => self.buffer.push(c),
            '.' if !self.buffer.contains('.') => self.buffer.push(c),
            '-' if self.buffer.is_empty() => self.buffer.push(c),
            _ => {}
        }
    }

    pub(crate) fn is_complete_number(&self) -> bool {
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

pub(crate) type GetFn = fn(&FluidControls) -> f32;
pub(crate) type SetFn = fn(&mut FluidControls, f32);
pub(crate) type DisplayFn = fn(&FluidControls) -> String;

/// How left/right adjustment moves the value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Step {
    /// value += dir * step, clamped to [min, max].
    Linear(f32),
    /// value doubles/halves, clamped to [min, max].
    PowerOfTwo,
}

/// How direct numeric entry is interpreted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Entry {
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
pub(crate) enum Bar {
    Linear,
    /// Bar position is log2-scaled (for power-of-two ranges).
    Log2,
}

/// How LFO modulation lands on the control. Grid-timing controls snap the
/// modulated value so triggers step through musical grids instead of
/// smearing continuously; everything else takes the raw value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LfoSnap {
    None,
    /// Snap to power-of-two beat subdivisions (interval-like controls).
    PowerOfTwo,
    /// Snap to the control's own step grid (offset-like controls).
    Step,
}

pub(crate) struct ControlSpec {
    pub(crate) id: &'static str,
    pub(crate) label: &'static str,
    pub(crate) kind: ControlKind,
    pub(crate) min: f32,
    pub(crate) max: f32,
    pub(crate) step: Step,
    pub(crate) entry: Entry,
    pub(crate) reset: f32,
    pub(crate) bar: Bar,
    pub(crate) lfo_snap: LfoSnap,
    pub(crate) get: GetFn,
    pub(crate) set: SetFn,
    pub(crate) display: DisplayFn,
}

impl ControlSpec {
    #[allow(clippy::too_many_arguments)]
    pub(crate) const fn new(
        id: &'static str,
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
            id,
            label,
            kind,
            min,
            max,
            step,
            entry,
            reset: min,
            bar: Bar::Linear,
            lfo_snap: LfoSnap::None,
            get,
            set,
            display,
        }
    }

    /// Gain-kind control: 2% steps, percent-style numeric entry, resets to min.
    pub(crate) const fn gain(
        id: &'static str,
        label: &'static str,
        min: f32,
        max: f32,
        get: GetFn,
        set: SetFn,
        display: DisplayFn,
    ) -> Self {
        Self::new(
            id,
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

    pub(crate) const fn with_step(mut self, step: f32) -> Self {
        self.step = Step::Linear(step);
        self
    }

    pub(crate) const fn reset_at(mut self, reset: f32) -> Self {
        self.reset = reset;
        self
    }

    pub(crate) const fn log_bar(mut self) -> Self {
        self.bar = Bar::Log2;
        self
    }

    pub(crate) const fn lfo_snap(mut self, snap: LfoSnap) -> Self {
        self.lfo_snap = snap;
        self
    }

    pub(crate) fn item(&self, c: &FluidControls) -> ControlItem {
        let (value, min, max) = match self.bar {
            Bar::Linear => ((self.get)(c), self.min, self.max),
            Bar::Log2 => ((self.get)(c).log2(), self.min.log2(), self.max.log2()),
        };
        ControlItem {
            id: self.id,
            label: self.label,
            kind: self.kind,
            value,
            min,
            max,
            display: (self.display)(c),
        }
    }

    pub(crate) fn apply_delta(&self, dir: f32, c: &mut FluidControls) {
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

    pub(crate) fn apply_min(&self, c: &mut FluidControls) {
        (self.set)(c, self.reset);
    }

    pub(crate) fn apply_value(&self, value: f32, c: &mut FluidControls) {
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

    pub(crate) fn quantized_value(&self, c: &FluidControls) -> f32 {
        self.quantize((self.get)(c))
    }

    pub(crate) fn apply_quantized_value(&self, value: f32, c: &mut FluidControls) {
        (self.set)(c, self.quantize(value));
    }

    pub(crate) fn quantize(&self, value: f32) -> f32 {
        let clamped = value.clamp(self.min, self.max);
        match self.step {
            Step::Linear(step) => snap_step(clamped, step).clamp(self.min, self.max),
            Step::PowerOfTwo => nearest_power_of_two(clamped, self.min, self.max),
        }
    }
}

pub(crate) fn pct(v: f32) -> String {
    format!("{:.0}%", v * 100.0)
}

pub(crate) fn beats2(v: f32) -> String {
    format!("{v:.2} beats")
}

pub(crate) fn ms0(v: f32) -> String {
    format!("{v:.0} ms")
}

pub(crate) const MASTER_CONTROLS: &[ControlSpec] = &[
    ControlSpec::gain(
        "pad.level",
        "Chords Vol",
        0.0,
        1.0,
        |c| c.pad.level,
        |c, v| c.pad.level = v,
        |c| pct(c.pad.level),
    ),
    ControlSpec::gain(
        "perc.level",
        "Perc Vol",
        0.0,
        1.0,
        |c| c.perc.level,
        |c, v| c.perc.level = v,
        |c| pct(c.perc.level),
    ),
    ControlSpec::gain(
        "kick.level",
        "Kick Vol",
        0.0,
        1.0,
        |c| c.kick.level,
        |c, v| c.kick.level = v,
        |c| pct(c.kick.level),
    ),
    ControlSpec::gain(
        "tonal.level",
        "Tonal Vol",
        0.0,
        1.0,
        |c| c.tonal.level,
        |c, v| c.tonal.level = v,
        |c| pct(c.tonal.level),
    ),
    ControlSpec::gain(
        "clap.level",
        "Clap Vol",
        0.0,
        1.0,
        |c| c.clap.level,
        |c, v| c.clap.level = v,
        |c| pct(c.clap.level),
    ),
    ControlSpec::gain(
        "bass.level",
        "Bass Vol",
        0.0,
        1.0,
        |c| c.bass.level,
        |c, v| c.bass.level = v,
        |c| pct(c.bass.level),
    ),
    ControlSpec::new(
        "master.bpm",
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
        "master.level",
        "Master Level",
        0.0,
        1.0,
        |c| c.master.level,
        |c, v| c.master.level = v,
        |c| pct(c.master.level),
    ),
    ControlSpec::gain(
        "master.drive",
        "Drive",
        0.0,
        1.0,
        |c| c.master.drive,
        |c, v| c.master.drive = v,
        |c| pct(c.master.drive),
    ),
    ControlSpec::new(
        "master.comp_threshold",
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
        "master.comp_ratio",
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
        "master.comp_release_ms",
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
        "master.tone",
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
        "master.tune",
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

pub(crate) const PERC_CONTROLS: &[ControlSpec] = &[
    ControlSpec::gain(
        "perc.level",
        "Level",
        0.0,
        1.0,
        |c| c.perc.level,
        |c, v| c.perc.level = v,
        |c| pct(c.perc.level),
    ),
    ControlSpec::new(
        "perc.interval_beats",
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
    )
    .lfo_snap(LfoSnap::PowerOfTwo),
    ControlSpec::new(
        "perc.offset_beats",
        "Offset",
        ControlKind::Timing,
        0.0,
        4.0,
        Step::Linear(0.25),
        Entry::Snap,
        |c| c.perc.offset_beats,
        |c, v| c.perc.offset_beats = v,
        |c| beats2(c.perc.offset_beats),
    )
    .lfo_snap(LfoSnap::Step),
    ControlSpec::new(
        "perc.decay_ms",
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
        "perc.filter",
        "Filter",
        0.5,
        1.0,
        |c| c.perc.filter,
        |c, v| c.perc.filter = v,
        |c| pct(c.perc.filter),
    ),
];

pub(crate) const CHORDS_CONTROLS: &[ControlSpec] = &[
    ControlSpec::gain(
        "pad.level",
        "Level",
        0.0,
        1.0,
        |c| c.pad.level,
        |c, v| c.pad.level = v,
        |c| pct(c.pad.level),
    ),
    ControlSpec::new(
        "pad.chord_bars",
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
    .log_bar()
    .lfo_snap(LfoSnap::Step),
    ControlSpec::new(
        "pad.progression",
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
        "pad.reverb_mix",
        "Reverb Mix",
        0.0,
        1.0,
        |c| c.pad.reverb_mix,
        |c, v| c.pad.reverb_mix = v,
        |c| pct(c.pad.reverb_mix),
    ),
    ControlSpec::gain(
        "pad.stereo_width",
        "Stereo Width",
        0.0,
        1.0,
        |c| c.pad.stereo_width,
        |c, v| c.pad.stereo_width = v,
        |c| pct(c.pad.stereo_width),
    ),
    ControlSpec::gain(
        "pad.detune",
        "Detune",
        0.0,
        1.0,
        |c| c.pad.detune,
        |c, v| c.pad.detune = v,
        |c| pct(c.pad.detune),
    ),
    ControlSpec::gain(
        "pad.octave_mix",
        "Octave Mix",
        0.0,
        1.0,
        |c| c.pad.octave_mix,
        |c, v| c.pad.octave_mix = v,
        |c| pct(c.pad.octave_mix),
    ),
    ControlSpec::new(
        "pad.attack_time",
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
        "pad.release_time",
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

pub(crate) const BASS_CONTROLS: &[ControlSpec] = &[
    ControlSpec::gain(
        "bass.level",
        "Level",
        0.0,
        1.0,
        |c| c.bass.level,
        |c, v| c.bass.level = v,
        |c| pct(c.bass.level),
    ),
    ControlSpec::new(
        "bass.interval_beats",
        "Interval",
        ControlKind::Timing,
        0.25,
        8.0,
        Step::Linear(0.25),
        Entry::Snap,
        |c| c.bass.interval_beats,
        |c, v| c.bass.interval_beats = v,
        |c| beats2(c.bass.interval_beats),
    )
    .lfo_snap(LfoSnap::PowerOfTwo),
    ControlSpec::new(
        "bass.offset_beats",
        "Offset",
        ControlKind::Timing,
        0.0,
        4.0,
        Step::Linear(0.25),
        Entry::Snap,
        |c| c.bass.offset_beats,
        |c, v| c.bass.offset_beats = v,
        |c| beats2(c.bass.offset_beats),
    )
    .lfo_snap(LfoSnap::Step),
    ControlSpec::new(
        "bass.rhythm",
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
        "bass.octave",
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
        "bass.attack_time",
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
        "bass.decay_time",
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
        "bass.drive",
        "Drive",
        0.0,
        1.0,
        |c| c.bass.drive,
        |c, v| c.bass.drive = v,
        |c| pct(c.bass.drive),
    ),
];

pub(crate) const KICK_CONTROLS: &[ControlSpec] = &[
    ControlSpec::gain(
        "kick.level",
        "Level",
        0.0,
        1.0,
        |c| c.kick.level,
        |c, v| c.kick.level = v,
        |c| pct(c.kick.level),
    ),
    ControlSpec::new(
        "kick.interval_beats",
        "Interval",
        ControlKind::Timing,
        0.25,
        4.0,
        Step::Linear(0.25),
        Entry::Snap,
        |c| c.kick.interval_beats,
        |c, v| c.kick.interval_beats = v,
        |c| beats2(c.kick.interval_beats),
    )
    .lfo_snap(LfoSnap::PowerOfTwo),
    ControlSpec::new(
        "kick.offset_beats",
        "Offset",
        ControlKind::Timing,
        0.0,
        4.0,
        Step::Linear(0.25),
        Entry::Snap,
        |c| c.kick.offset_beats,
        |c, v| c.kick.offset_beats = v,
        |c| beats2(c.kick.offset_beats),
    )
    .lfo_snap(LfoSnap::Step),
    ControlSpec::new(
        "kick.start_freq",
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
        "kick.pitch_decay_ms",
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
        "kick.amp_decay_ms",
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
        "kick.click",
        "Click",
        0.0,
        0.2,
        |c| c.kick.click,
        |c, v| c.kick.click = v,
        |c| pct(c.kick.click / 0.2),
    )
    .with_step(0.01),
    ControlSpec::gain(
        "kick.drive",
        "Drive",
        0.0,
        1.0,
        |c| c.kick.drive,
        |c, v| c.kick.drive = v,
        |c| pct(c.kick.drive),
    ),
    ControlSpec::gain(
        "kick.filter",
        "Filter",
        0.0,
        1.0,
        |c| c.kick.filter,
        |c, v| c.kick.filter = v,
        |c| pct(c.kick.filter),
    ),
    ControlSpec::new(
        "kick.echo_time_beats",
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
        "kick.echo_filter",
        "Echo Filter",
        0.0,
        1.0,
        |c| c.kick.echo_filter,
        |c, v| c.kick.echo_filter = v,
        |c| pct(c.kick.echo_filter),
    ),
    ControlSpec::gain(
        "kick.echo_amount",
        "Echo Amount",
        0.0,
        0.9,
        |c| c.kick.echo_amount,
        |c, v| c.kick.echo_amount = v,
        |c| pct(c.kick.echo_amount / 0.9),
    ),
    ControlSpec::gain(
        "kick.echo_feedback",
        "Echo Feedback",
        0.0,
        0.85,
        |c| c.kick.echo_feedback,
        |c, v| c.kick.echo_feedback = v,
        |c| pct(c.kick.echo_feedback / 0.85),
    ),
];

pub(crate) const TONAL_CONTROLS: &[ControlSpec] = &[
    ControlSpec::gain(
        "tonal.level",
        "Level",
        0.0,
        1.0,
        |c| c.tonal.level,
        |c, v| c.tonal.level = v,
        |c| pct(c.tonal.level),
    ),
    ControlSpec::new(
        "tonal.synth_type",
        "Type",
        ControlKind::Discrete,
        0.0,
        4.0,
        Step::Linear(1.0),
        Entry::Round,
        |c| c.tonal.synth_type,
        |c, v| c.tonal.synth_type = v,
        |c| tonal_synth_type_label(c.tonal.synth_type).to_string(),
    ),
    ControlSpec::new(
        "tonal.phrase",
        "Phrase",
        ControlKind::Discrete,
        0.0,
        7.0,
        Step::Linear(1.0),
        Entry::Round,
        |c| c.tonal.phrase,
        |c, v| c.tonal.phrase = v,
        |c| {
            ["A", "B", "C", "D", "E", "F", "G", "H"][c.tonal.phrase.round() as usize % 8]
                .to_string()
        },
    ),
    ControlSpec::new(
        "tonal.rate_beats",
        "Rate",
        ControlKind::Timing,
        TONAL_RATE_BEATS_MIN,
        TONAL_RATE_BEATS_MAX,
        Step::Linear(0.25),
        Entry::Snap,
        |c| c.tonal.rate_beats,
        |c, v| c.tonal.rate_beats = v,
        |c| beats2(c.tonal.rate_beats),
    )
    .lfo_snap(LfoSnap::PowerOfTwo),
    ControlSpec::new(
        "tonal.step_interval_beats",
        "Cycle",
        ControlKind::Timing,
        TONAL_CYCLE_BEATS_MIN,
        TONAL_CYCLE_BEATS_MAX,
        Step::Linear(0.25),
        Entry::Snap,
        |c| c.tonal.step_interval_beats,
        |c, v| c.tonal.step_interval_beats = v,
        |c| beats2(c.tonal.step_interval_beats),
    )
    .lfo_snap(LfoSnap::PowerOfTwo),
    ControlSpec::new(
        "tonal.offset_beats",
        "Offset",
        ControlKind::Timing,
        0.0,
        4.0,
        Step::Linear(0.25),
        Entry::Snap,
        |c| c.tonal.offset_beats,
        |c, v| c.tonal.offset_beats = v,
        |c| beats2(c.tonal.offset_beats),
    )
    .lfo_snap(LfoSnap::Step),
    ControlSpec::gain(
        "tonal.randomness",
        "Randomness",
        0.0,
        1.0,
        |c| c.tonal.randomness,
        |c, v| c.tonal.randomness = v,
        |c| pct(c.tonal.randomness),
    ),
    ControlSpec::new(
        "tonal.evolve_rate",
        "Evolve",
        ControlKind::Continuous,
        0.0,
        1.0,
        Step::Linear(0.05),
        Entry::Percent,
        |c| c.tonal.evolve_rate,
        |c, v| c.tonal.evolve_rate = v,
        |c| pct(c.tonal.evolve_rate),
    ),
    ControlSpec::new(
        "tonal.note_length_beats",
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
        "tonal.reverb_mix",
        "Reverb Mix",
        0.0,
        1.0,
        |c| c.tonal.reverb_mix,
        |c, v| c.tonal.reverb_mix = v,
        |c| pct(c.tonal.reverb_mix),
    ),
];

pub(crate) fn tonal_synth_type_label(value: f32) -> &'static str {
    match tonal_synth_type_index(value) {
        0 => "Sine",
        1 => "Piano A",
        2 => "Piano B",
        3 => "Piano C",
        _ => "Marimba",
    }
}

pub(crate) fn tonal_synth_type_index(value: f32) -> usize {
    (value.round() as i64).rem_euclid(5) as usize
}

pub(crate) const CLAP_CONTROLS: &[ControlSpec] = &[
    ControlSpec::gain(
        "clap.level",
        "Level",
        0.0,
        1.0,
        |c| c.clap.level,
        |c, v| c.clap.level = v,
        |c| pct(c.clap.level),
    ),
    ControlSpec::new(
        "clap.interval_beats",
        "Interval",
        ControlKind::Timing,
        0.5,
        8.0,
        Step::Linear(0.25),
        Entry::Snap,
        |c| c.clap.interval_beats,
        |c, v| c.clap.interval_beats = v,
        |c| beats2(c.clap.interval_beats),
    )
    .lfo_snap(LfoSnap::PowerOfTwo),
    ControlSpec::new(
        "clap.offset_beats",
        "Offset",
        ControlKind::Timing,
        0.0,
        8.0,
        Step::Linear(0.25),
        Entry::Snap,
        |c| c.clap.offset_beats,
        |c, v| c.clap.offset_beats = v,
        |c| beats2(c.clap.offset_beats),
    )
    .lfo_snap(LfoSnap::Step),
    ControlSpec::new(
        "clap.slap_count",
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
        "clap.slap_spread_ms",
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
        "clap.decay_ms",
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
        "clap.filter",
        "Filter",
        0.5,
        1.0,
        |c| c.clap.filter,
        |c, v| c.clap.filter = v,
        |c| pct(c.clap.filter),
    ),
    ControlSpec::gain(
        "clap.room",
        "Room",
        0.0,
        1.0,
        |c| c.clap.room,
        |c, v| c.clap.room = v,
        |c| pct(c.clap.room),
    ),
    ControlSpec::gain(
        "clap.body",
        "Body",
        0.0,
        1.0,
        |c| c.clap.body,
        |c, v| c.clap.body = v,
        |c| pct(c.clap.body),
    ),
];

pub(crate) fn tab_specs(tab: Tab) -> &'static [ControlSpec] {
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

pub(crate) fn all_specs() -> impl Iterator<Item = &'static ControlSpec> {
    Tab::all().into_iter().flat_map(tab_specs)
}

pub(crate) fn spec_by_id(id: &str) -> Option<&'static ControlSpec> {
    all_specs().find(|spec| spec.id == id)
}

pub(crate) fn tab_controls(tab: Tab, c: &FluidControls) -> Vec<ControlItem> {
    tab_specs(tab).iter().map(|spec| spec.item(c)).collect()
}

pub(crate) fn apply_delta(tab: Tab, selected: usize, dir: f32, c: &mut FluidControls) {
    if let Some(spec) = tab_specs(tab).get(selected) {
        spec.apply_delta(dir, c);
    }
}

pub(crate) fn apply_min(tab: Tab, selected: usize, c: &mut FluidControls) {
    if let Some(spec) = tab_specs(tab).get(selected) {
        spec.apply_min(c);
    }
}

pub(crate) fn apply_value(tab: Tab, selected: usize, value: f32, c: &mut FluidControls) {
    if let Some(spec) = tab_specs(tab).get(selected) {
        spec.apply_value(value, c);
    }
}

pub(crate) fn normalize_unit_input(value: f32) -> f32 {
    if value > 1.0 {
        (value / 100.0).clamp(0.0, 1.0)
    } else {
        value.clamp(0.0, 1.0)
    }
}

pub(crate) fn snap_step(value: f32, step: f32) -> f32 {
    (value / step).round() * step
}

pub(crate) fn nearest_power_of_two(value: f32, min: f32, max: f32) -> f32 {
    let clamped = value.clamp(min, max);
    let exponent = clamped.log2().round();
    2.0f32.powf(exponent).clamp(min, max)
}
