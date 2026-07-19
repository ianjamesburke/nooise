use super::*;

// ============================================================
// UI
// ============================================================

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Tab {
    Chords = 0,
    Perc = 1,
    Bass = 2,
    Kick = 3,
    Tonal = 4,
    Clap = 5,
    Arp = 6,
    Macros = 7,
    Master = 8,
}

/// One row per tab: (variant, display name, mute-target level id, control
/// table) in discriminant order. `Tab::all`/`name`/`level_id`/`tab_specs`
/// all derive from indexing this single table by `self as usize`.
const TAB_META: [(Tab, &str, Option<&str>, &[ControlSpec]); 9] = [
    (Tab::Chords, "Chords", Some("pad.level"), CHORDS_CONTROLS),
    (Tab::Perc, "Perc", Some("perc.level"), PERC_CONTROLS),
    (Tab::Bass, "Bass", Some("bass.level"), BASS_CONTROLS),
    (Tab::Kick, "Kick", Some("kick.level"), KICK_CONTROLS),
    (Tab::Tonal, "Tonal", Some("tonal.level"), TONAL_CONTROLS),
    (Tab::Clap, "Clap", Some("clap.level"), CLAP_CONTROLS),
    (Tab::Arp, "Arp", Some("arp.gain"), ARP_CONTROLS),
    (Tab::Macros, "Macros", None, MACRO_CONTROLS),
    (Tab::Master, "Master", Some("master.level"), MASTER_CONTROLS),
];

impl Tab {
    pub(crate) fn all() -> [Tab; 9] {
        TAB_META.map(|(tab, _, _, _)| tab)
    }

    pub(crate) fn name(self) -> &'static str {
        TAB_META[self as usize].1
    }

    // Discriminants match `all()`'s order, so tab cycling is index arithmetic.
    pub(crate) fn next(self) -> Self {
        let all = Self::all();
        all[(self as usize + 1) % all.len()]
    }

    pub(crate) fn previous(self) -> Self {
        let all = Self::all();
        all[(self as usize + all.len() - 1) % all.len()]
    }

    /// Stable id of this tab's primary level/gain control, or `None` for a
    /// tab with no single level to mute (`Macros`). The one place that maps
    /// a tab to its mute target, so `m`/`M` never need a per-voice match arm.
    pub(crate) fn level_id(self) -> Option<&'static str> {
        TAB_META[self as usize].2
    }
}

pub(crate) struct ControlItem {
    pub(crate) id: &'static str,
    pub(crate) label: &'static str,
    pub(crate) kind: ControlKind,
    pub(crate) value: f32,
    pub(crate) min: f32,
    pub(crate) max: f32,
    /// How `value` maps onto the 0..1 bar — carried so `item_ratio` and the
    /// marker math stay one shared computation.
    pub(crate) taper: Taper,
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
    /// 0.125 as the floor value, sixteenths (0.25 grid) above it.
    BeatGrid,
}

/// How direct numeric entry is interpreted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Entry {
    /// Unit or percent input, scaled to [0, max] (e.g. 42 → 0.42 * max).
    Percent,
    /// Displayed/typed in beats while stored internally as bars.
    BeatsAsBars,
    /// Rounded to the nearest integer.
    Round,
    /// Snapped to the control's step grid.
    Snap,
    /// Used as-is (clamped only).
    Free,
}

/// Steps per full sweep of an exp-tapered control: one h/l press moves the dial
/// by this fraction of its throw, so a tapered time control gets this many fine
/// steps end to end no matter how wide its range.
pub(crate) const TAPER_STEPS_PER_SWEEP: f32 = 48.0;

/// Default exponent for time controls' exp taper — how hard resolution
/// concentrates at the low end (1.0 is linear; larger biases toward the floor).
/// The one place to retune the feel of every envelope-time dial. Tuned by ear.
pub(crate) const TIME_TAPER: f32 = 3.0;

/// How a control's value maps onto dial position — the shared taper driving
/// both the visual ratio bar and h/l stepping. `forward` sends a value into the
/// space where position is linear; `inverse` brings a position back to a value.
/// Position (0..1) of `v` in `[min, max]` is therefore
/// `(forward(v) - forward(min)) / (forward(max) - forward(min))`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Taper {
    Linear,
    /// Log2-scaled, for power-of-two (musical) ranges. Needs a positive min.
    Log2,
    /// Power-law with exponent `n > 1`, concentrating resolution at the low
    /// end: `forward(v) = v^(1/n)`, so `value ≈ max * ratio^n` — fine control
    /// near the floor, coarse near the ceiling. Handles a zero min, which a
    /// pure log cannot.
    Exp(f32),
}

impl Taper {
    pub(crate) fn forward(self, v: f32) -> f32 {
        match self {
            Self::Linear => v,
            Self::Log2 => v.log2(),
            Self::Exp(n) => v.max(0.0).powf(1.0 / n),
        }
    }

    pub(crate) fn inverse(self, t: f32) -> f32 {
        match self {
            Self::Linear => t,
            Self::Log2 => t.exp2(),
            Self::Exp(n) => t.max(0.0).powf(n),
        }
    }

    /// Position (0..1) of `value` within `[min, max]` under this taper.
    pub(crate) fn ratio(self, value: f32, min: f32, max: f32) -> f32 {
        let (lo, hi) = (self.forward(min), self.forward(max));
        let span = hi - lo;
        if span.abs() <= f32::EPSILON {
            0.0
        } else {
            ((self.forward(value) - lo) / span).clamp(0.0, 1.0)
        }
    }

    /// Value at position `ratio` (0..1) within `[min, max]` under this taper.
    pub(crate) fn value_at(self, ratio: f32, min: f32, max: f32) -> f32 {
        let (lo, hi) = (self.forward(min), self.forward(max));
        self.inverse(lo + ratio.clamp(0.0, 1.0) * (hi - lo))
    }
}

/// The native unit of a time-like control, letting the UI's unit toggle (T)
/// convert its display and numeric entry between beats and milliseconds at
/// the current BPM. Stepping always stays on the native grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TimeBase {
    None,
    Beats,
    Ms,
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

#[derive(Clone, Copy)]
pub(crate) struct ControlSpec {
    pub(crate) id: &'static str,
    pub(crate) label: &'static str,
    pub(crate) kind: ControlKind,
    pub(crate) min: f32,
    pub(crate) max: f32,
    pub(crate) step: Step,
    pub(crate) entry: Entry,
    pub(crate) reset: f32,
    pub(crate) taper: Taper,
    pub(crate) lfo_snap: LfoSnap,
    pub(crate) time_base: TimeBase,
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
            taper: Taper::Linear,
            lfo_snap: LfoSnap::None,
            time_base: TimeBase::None,
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

    pub(crate) const fn taper(mut self, taper: Taper) -> Self {
        self.taper = taper;
        self
    }

    pub(crate) const fn lfo_snap(mut self, snap: LfoSnap) -> Self {
        self.lfo_snap = snap;
        self
    }

    pub(crate) const fn in_beats(mut self) -> Self {
        self.time_base = TimeBase::Beats;
        self
    }

    pub(crate) const fn in_ms(mut self) -> Self {
        self.time_base = TimeBase::Ms;
        self
    }

    pub(crate) fn item(&self, c: &FluidControls) -> ControlItem {
        ControlItem {
            id: self.id,
            label: self.label,
            kind: self.kind,
            value: (self.get)(c),
            min: self.min,
            max: self.max,
            taper: self.taper,
            display: (self.display)(c),
        }
    }

    pub(crate) fn apply_delta(&self, dir: f32, c: &mut FluidControls) {
        let value = (self.get)(c);
        let next = if self.is_continuous_tapered() {
            // A tapered continuous dial steps in position space, so each press
            // moves an equal fraction of the throw — fine near the floor,
            // coarse near the ceiling (log-even octaves for Log2, low-biased
            // for Exp) — instead of a fixed value delta.
            let ratio = self.taper.ratio(value, self.min, self.max);
            let stepped = (ratio + dir / TAPER_STEPS_PER_SWEEP).clamp(0.0, 1.0);
            self.taper
                .value_at(stepped, self.min, self.max)
                .clamp(self.min, self.max)
        } else {
            match self.step {
                Step::Linear(step) => (value + dir * step).clamp(self.min, self.max),
                Step::PowerOfTwo => {
                    if dir > 0.0 {
                        (value * 2.0).min(self.max)
                    } else {
                        (value / 2.0).max(self.min)
                    }
                }
                Step::BeatGrid => beat_grid_adjust(value, dir, self.min, self.max),
            }
        };
        (self.set)(c, next);
    }

    /// A continuous dial with a non-linear taper and a plain `Linear` step:
    /// stepped in position space and stored at full precision. Discrete grids
    /// (`PowerOfTwo`/`BeatGrid`) keep their own musical stepping even under a
    /// `Log2` bar (e.g. chord bars doubling on octaves).
    fn is_continuous_tapered(&self) -> bool {
        !matches!(self.taper, Taper::Linear) && matches!(self.step, Step::Linear(_))
    }

    pub(crate) fn apply_min(&self, c: &mut FluidControls) {
        (self.set)(c, self.reset);
    }

    pub(crate) fn apply_value(&self, value: f32, c: &mut FluidControls) {
        let next = match self.entry {
            Entry::Percent => normalize_unit_input(value) * self.max,
            Entry::BeatsAsBars => nearest_power_of_two(value / 4.0, self.min, self.max),
            Entry::Round => value.round(),
            Entry::Snap => self.snap_on_grid(value),
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

    /// Set an exact value, clamped to range but not snapped to the step
    /// grid — used while a time control is being driven in its flipped unit.
    pub(crate) fn apply_raw(&self, value: f32, c: &mut FluidControls) {
        (self.set)(c, value.clamp(self.min, self.max));
    }

    pub(crate) fn quantize(&self, value: f32) -> f32 {
        let clamped = value.clamp(self.min, self.max);
        // Tapered continuous dials move in position space, so they carry no
        // value grid: store the exact value (full precision, exact song-code
        // round-trip) rather than snapping to a spurious step.
        if self.is_continuous_tapered() {
            return clamped;
        }
        self.snap_on_grid(clamped)
    }

    /// Snap `v` onto this control's step grid, clamped to range. Shared by
    /// `apply_value`'s `Entry::Snap` arm and `quantize`'s post-clamp match.
    fn snap_on_grid(&self, v: f32) -> f32 {
        let clamped = v.clamp(self.min, self.max);
        match self.step {
            Step::Linear(step) => snap_step(clamped, step).clamp(self.min, self.max),
            Step::PowerOfTwo => nearest_power_of_two(clamped, self.min, self.max),
            Step::BeatGrid => beat_grid_snap(clamped, self.min, self.max),
        }
    }
}

pub(crate) fn pct(v: f32) -> String {
    format!("{:.0}%", v * 100.0)
}

pub(crate) fn beats2(v: f32) -> String {
    format!("{v:.2} beats")
}

/// Canonical time readout, shared by every time control: whole milliseconds
/// below 1 s, seconds (2 dp) at or above. Takes seconds, so ms-stored controls
/// pass `ms / 1000.0` and get identical ms/s presentation.
pub(crate) fn secs(seconds: f32) -> String {
    if seconds < 1.0 {
        format!("{:.0} ms", seconds * 1000.0)
    } else {
        format!("{seconds:.2} s")
    }
}

/// Gain row on the plain 0..1 archetype: percent display of the field
/// itself. `$($f:tt)+` takes any field path, including an indexed one like
/// `macros.values[0]`. The first arm (tried first so its numeric literals
/// don't get swallowed by the generic field-path repetition) covers the
/// rare row with a non-default min/max, e.g. a 0.5..1.0 filter floor.
macro_rules! gain_pct {
    ($id:literal, $label:literal, $min:literal, $max:literal, $($f:tt)+) => {
        ControlSpec::gain(
            $id,
            $label,
            $min,
            $max,
            |c| c.$($f)+,
            |c, v| c.$($f)+ = v,
            |c| pct(c.$($f)+),
        )
    };
    ($id:literal, $label:literal, $($f:tt)+) => {
        ControlSpec::gain(
            $id,
            $label,
            0.0,
            1.0,
            |c| c.$($f)+,
            |c, v| c.$($f)+ = v,
            |c| pct(c.$($f)+),
        )
    };
}

/// Time row stored in seconds: `Timing` kind, exp taper, free numeric entry,
/// `secs` display of the field directly.
macro_rules! time_secs {
    ($id:literal, $label:literal, $min:expr, $max:expr, $step:expr, $($f:tt)+) => {
        ControlSpec::new(
            $id,
            $label,
            ControlKind::Timing,
            $min,
            $max,
            Step::Linear($step),
            Entry::Free,
            |c| c.$($f)+,
            |c, v| c.$($f)+ = v,
            |c| secs(c.$($f)+),
        )
        .taper(Taper::Exp(TIME_TAPER))
    };
}

/// Time row stored in milliseconds: same archetype as `time_secs!`, but the
/// field is ms so display converts to seconds and the control is flagged
/// `in_ms()` for the unit toggle.
macro_rules! time_ms {
    ($id:literal, $label:literal, $min:expr, $max:expr, $step:expr, $($f:tt)+) => {
        ControlSpec::new(
            $id,
            $label,
            ControlKind::Timing,
            $min,
            $max,
            Step::Linear($step),
            Entry::Free,
            |c| c.$($f)+,
            |c, v| c.$($f)+ = v,
            |c| secs(c.$($f)+ / 1000.0),
        )
        .taper(Taper::Exp(TIME_TAPER))
        .in_ms()
    };
}

/// Beat-grid interval row: `Timing` kind, `BeatGrid` step, snapped numeric
/// entry, beats display, LFO modulation snapped to power-of-two subdivisions.
macro_rules! beat_interval {
    ($id:literal, $label:literal, $min:expr, $max:expr, $($f:tt)+) => {
        ControlSpec::new(
            $id,
            $label,
            ControlKind::Timing,
            $min,
            $max,
            Step::BeatGrid,
            Entry::Snap,
            |c| c.$($f)+,
            |c, v| c.$($f)+ = v,
            |c| beats2(c.$($f)+),
        )
        .lfo_snap(LfoSnap::PowerOfTwo)
        .in_beats()
    };
}

/// Beat-grid offset row: same archetype as `beat_interval!` but min fixed at
/// 0.0 and LFO modulation snapped to the control's own step grid instead.
macro_rules! beat_offset {
    ($id:literal, $label:literal, $max:expr, $($f:tt)+) => {
        ControlSpec::new(
            $id,
            $label,
            ControlKind::Timing,
            0.0,
            $max,
            Step::BeatGrid,
            Entry::Snap,
            |c| c.$($f)+,
            |c, v| c.$($f)+ = v,
            |c| beats2(c.$($f)+),
        )
        .lfo_snap(LfoSnap::Step)
        .in_beats()
    };
}

pub(crate) const MASTER_CONTROLS: &[ControlSpec] = &[
    gain_pct!("pad.level", "Chords Vol", pad.level),
    gain_pct!("perc.level", "Perc Vol", perc.level),
    gain_pct!("kick.level", "Kick Vol", kick.level),
    gain_pct!("tonal.level", "Tonal Vol", tonal.level),
    gain_pct!("clap.level", "Clap Vol", clap.level),
    gain_pct!("bass.level", "Bass Vol", bass.level),
    gain_pct!("arp.gain", "Arp Vol", arp.gain),
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
    gain_pct!("master.level", "Master Level", master.level),
    gain_pct!("master.drive", "Drive", master.drive),
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
    time_ms!(
        "master.comp_release_ms",
        "Comp Release",
        10.0,
        500.0,
        1.0,
        master.comp_release_ms
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
    gain_pct!("perc.level", "Level", perc.level),
    gain_pct!("perc.filter", "Filter", 0.5, 1.0, perc.filter),
    time_ms!("perc.decay_ms", "Decay", 20.0, 2000.0, 1.0, perc.decay_ms),
    ControlSpec::new(
        "perc.interval_beats",
        "Interval",
        ControlKind::Timing,
        0.125,
        4.25,
        Step::BeatGrid,
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
    .lfo_snap(LfoSnap::PowerOfTwo)
    .in_beats(),
    beat_offset!("perc.offset_beats", "Offset", 4.0, perc.offset_beats),
    gain_pct!("perc.swing", "Swing", perc.swing),
];

/// One chord slot's four fields as `ControlSpec` rows (Root/Accidental/
/// Extension/Inversion), expanded inline into `CHORDS_CONTROLS`. Slot
/// numbers are 1-based in ids/labels, 0-based into `chord_slots`.
macro_rules! chord_slot_rows {
    ($slot:literal) => {
        [
            ControlSpec::new(
                concat!("pad.chord", $slot, "_degree"),
                concat!("Chord ", $slot, " Root"),
                ControlKind::Discrete,
                -7.0,
                7.0,
                Step::Linear(1.0),
                Entry::Round,
                |c| c.pad.chord_slots[$slot - 1].degree,
                |c, v| c.pad.chord_slots[$slot - 1].degree = v,
                |c| format!("{:+.0}", c.pad.chord_slots[$slot - 1].degree),
            )
            .reset_at(0.0),
            ControlSpec::new(
                concat!("pad.chord", $slot, "_accidental"),
                concat!("Chord ", $slot, " Accidental"),
                ControlKind::Discrete,
                -1.0,
                1.0,
                Step::Linear(1.0),
                Entry::Round,
                |c| c.pad.chord_slots[$slot - 1].accidental,
                |c, v| c.pad.chord_slots[$slot - 1].accidental = v,
                |c| match c.pad.chord_slots[$slot - 1].accidental.round() as i32 {
                    -1 => "b".to_string(),
                    1 => "#".to_string(),
                    _ => "natural".to_string(),
                },
            )
            .reset_at(0.0),
            ControlSpec::new(
                concat!("pad.chord", $slot, "_extension"),
                concat!("Chord ", $slot, " Extension"),
                ControlKind::Discrete,
                0.0,
                3.0,
                Step::Linear(1.0),
                Entry::Round,
                |c| c.pad.chord_slots[$slot - 1].extension,
                |c, v| c.pad.chord_slots[$slot - 1].extension = v,
                |c| format!("{:.0}", c.pad.chord_slots[$slot - 1].extension),
            ),
            ControlSpec::new(
                concat!("pad.chord", $slot, "_inversion"),
                concat!("Chord ", $slot, " Inversion"),
                ControlKind::Discrete,
                0.0,
                3.0,
                Step::Linear(1.0),
                Entry::Round,
                |c| c.pad.chord_slots[$slot - 1].inversion,
                |c, v| c.pad.chord_slots[$slot - 1].inversion = v,
                |c| format!("{:.0}", c.pad.chord_slots[$slot - 1].inversion),
            )
            .reset_at(0.0),
        ]
    };
}

pub(crate) const CHORDS_CONTROLS: &[ControlSpec] = &[
    gain_pct!("pad.level", "Level", pad.level),
    time_secs!(
        "pad.attack_time",
        "Attack",
        0.05,
        30.0,
        0.001,
        pad.attack_time
    ),
    time_secs!(
        "pad.release_time",
        "Release",
        0.05,
        20.0,
        0.001,
        pad.release_time
    ),
    ControlSpec::new(
        "pad.type",
        "Type",
        ControlKind::Discrete,
        0.0,
        2.0,
        Step::Linear(1.0),
        Entry::Round,
        |c| c.pad.voice_type,
        |c, v| c.pad.voice_type = v,
        |c| pad_type_label(c.pad.voice_type).to_string(),
    ),
    ControlSpec::new(
        "pad.chord_bars",
        "Chord Length",
        ControlKind::Timing,
        1.0,
        64.0,
        Step::PowerOfTwo,
        Entry::BeatsAsBars,
        |c| c.pad.chord_bars,
        |c, v| c.pad.chord_bars = v,
        |c| format!("{:.0} beats", c.pad.chord_bars * 4.0),
    )
    .taper(Taper::Log2)
    .lfo_snap(LfoSnap::Step),
    ControlSpec::new(
        "pad.chord_count",
        "Chord Count",
        ControlKind::Discrete,
        1.0,
        CHORD_SLOT_COUNT as f32,
        Step::Linear(1.0),
        Entry::Round,
        |c| c.pad.chord_count,
        |c, v| c.pad.chord_count = v,
        |c| format!("{:.0}", c.pad.chord_count),
    )
    .reset_at(CHORD_SLOT_COUNT as f32),
    ControlSpec::new(
        "pad.progression",
        "Progression",
        ControlKind::Discrete,
        0.0,
        CUSTOM_PROGRESSION_INDEX as f32,
        Step::Linear(1.0),
        Entry::Round,
        |c| c.pad.progression,
        |c, v| c.pad.progression = v,
        |c| {
            let index = progression_index(c.pad.progression);
            if is_custom_progression(index) {
                "Custom".to_string()
            } else {
                ["A", "B", "C", "D", "E", "F", "G", "H"][index].to_string()
            }
        },
    ),
    gain_pct!("pad.reverb_mix", "Reverb Mix", pad.reverb_mix),
    gain_pct!("pad.stereo_width", "Stereo Width", pad.stereo_width),
    gain_pct!("pad.detune", "Detune", pad.detune),
    gain_pct!("pad.octave_mix", "Octave Mix", pad.octave_mix),
    chord_slot_rows!(1)[0],
    chord_slot_rows!(1)[1],
    chord_slot_rows!(1)[2],
    chord_slot_rows!(1)[3],
    chord_slot_rows!(2)[0],
    chord_slot_rows!(2)[1],
    chord_slot_rows!(2)[2],
    chord_slot_rows!(2)[3],
    chord_slot_rows!(3)[0],
    chord_slot_rows!(3)[1],
    chord_slot_rows!(3)[2],
    chord_slot_rows!(3)[3],
    chord_slot_rows!(4)[0],
    chord_slot_rows!(4)[1],
    chord_slot_rows!(4)[2],
    chord_slot_rows!(4)[3],
    chord_slot_rows!(5)[0],
    chord_slot_rows!(5)[1],
    chord_slot_rows!(5)[2],
    chord_slot_rows!(5)[3],
    chord_slot_rows!(6)[0],
    chord_slot_rows!(6)[1],
    chord_slot_rows!(6)[2],
    chord_slot_rows!(6)[3],
    chord_slot_rows!(7)[0],
    chord_slot_rows!(7)[1],
    chord_slot_rows!(7)[2],
    chord_slot_rows!(7)[3],
    chord_slot_rows!(8)[0],
    chord_slot_rows!(8)[1],
    chord_slot_rows!(8)[2],
    chord_slot_rows!(8)[3],
];

pub(crate) const BASS_CONTROLS: &[ControlSpec] = &[
    gain_pct!("bass.level", "Level", bass.level),
    ControlSpec::new(
        "bass.cutoff",
        "Cutoff",
        ControlKind::Continuous,
        BASS_CUTOFF_MIN_HZ,
        BASS_CUTOFF_MAX_HZ,
        Step::Linear(100.0),
        Entry::Free,
        |c| c.bass.cutoff,
        |c, v| c.bass.cutoff = v,
        |c| format!("{:.0} Hz", c.bass.cutoff),
    )
    // Frequency is perceptually logarithmic: a Log2 taper spaces octaves
    // evenly across the dial so the sweep is smooth from 80 Hz to fully open.
    .taper(Taper::Log2)
    .reset_at(BASS_CUTOFF_MAX_HZ),
    time_secs!(
        "bass.attack_time",
        "Attack",
        0.005,
        1.0,
        0.001,
        bass.attack_time
    ),
    time_secs!(
        "bass.decay_time",
        "Decay",
        0.005,
        2.0,
        0.001,
        bass.decay_time
    ),
    ControlSpec::new(
        "bass.type",
        "Type",
        ControlKind::Discrete,
        0.0,
        2.0,
        Step::Linear(1.0),
        Entry::Round,
        |c| c.bass.voice_type,
        |c, v| c.bass.voice_type = v,
        |c| bass_type_label(c.bass.voice_type).to_string(),
    ),
    beat_interval!(
        "bass.interval_beats",
        "Interval",
        0.125,
        8.0,
        bass.interval_beats
    ),
    beat_offset!("bass.offset_beats", "Offset", 4.0, bass.offset_beats),
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
    gain_pct!("bass.drive", "Drive", bass.drive),
];

pub(crate) fn bass_type_label(value: f32) -> &'static str {
    match bass_type_index(value) {
        0 => "Sub",
        1 => "Saw",
        _ => "Pluck",
    }
}

pub(crate) fn bass_type_index(value: f32) -> usize {
    (value.round() as i64).rem_euclid(3) as usize
}

pub(crate) fn pad_type_label(value: f32) -> &'static str {
    match pad_type_index(value) {
        0 => "Warm",
        1 => "Dark",
        _ => "Glass",
    }
}

pub(crate) fn pad_type_index(value: f32) -> usize {
    (value.round() as i64).rem_euclid(3) as usize
}

pub(crate) fn kick_type_label(value: f32) -> &'static str {
    match kick_type_index(value) {
        0 => "Sub",
        1 => "Punch",
        2 => "Membrane",
        _ => "Driven",
    }
}

pub(crate) fn kick_type_index(value: f32) -> usize {
    (value.round() as i64).rem_euclid(4) as usize
}

pub(crate) const KICK_CONTROLS: &[ControlSpec] = &[
    gain_pct!("kick.level", "Level", kick.level),
    gain_pct!("kick.filter", "Filter", kick.filter),
    time_ms!(
        "kick.pitch_decay_ms",
        "Pitch Decay",
        10.0,
        300.0,
        1.0,
        kick.pitch_decay_ms
    ),
    time_ms!(
        "kick.amp_decay_ms",
        "Amp Decay",
        50.0,
        1000.0,
        1.0,
        kick.amp_decay_ms
    ),
    ControlSpec::new(
        "kick.type",
        "Type",
        ControlKind::Discrete,
        0.0,
        3.0,
        Step::Linear(1.0),
        Entry::Round,
        |c| c.kick.voice_type,
        |c, v| c.kick.voice_type = v,
        |c| kick_type_label(c.kick.voice_type).to_string(),
    ),
    beat_interval!(
        "kick.interval_beats",
        "Interval",
        0.125,
        4.0,
        kick.interval_beats
    ),
    beat_offset!("kick.offset_beats", "Offset", 4.0, kick.offset_beats),
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
    gain_pct!("kick.drive", "Drive", kick.drive),
];

pub(crate) const TONAL_CONTROLS: &[ControlSpec] = &[
    gain_pct!("tonal.level", "Level", tonal.level),
    time_secs!("tonal.attack", "Attack", 0.0, 1.0, 0.001, tonal.attack),
    time_secs!(
        "tonal.decay",
        "Decay",
        TONAL_DECAY_MIN,
        6.0,
        0.001,
        tonal.decay
    ),
    ControlSpec::new(
        "tonal.synth_type",
        "Type",
        ControlKind::Discrete,
        0.0,
        9.0,
        Step::Linear(1.0),
        Entry::Round,
        |c| c.tonal.synth_type,
        |c, v| c.tonal.synth_type = v,
        |c| tonal_synth_type_label(c.tonal.synth_type).to_string(),
    ),
    ControlSpec::new(
        "tonal.octave",
        "Octave",
        ControlKind::Discrete,
        -2.0,
        2.0,
        Step::Linear(1.0),
        Entry::Round,
        |c| c.tonal.octave,
        |c, v| c.tonal.octave = v,
        |c| format!("{:.0}", c.tonal.octave),
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
    beat_interval!(
        "tonal.rate_beats",
        "Rate",
        TONAL_RATE_BEATS_MIN,
        TONAL_RATE_BEATS_MAX,
        tonal.rate_beats
    ),
    beat_interval!(
        "tonal.step_interval_beats",
        "Cycle",
        TONAL_CYCLE_BEATS_MIN,
        TONAL_CYCLE_BEATS_MAX,
        tonal.step_interval_beats
    ),
    beat_offset!("tonal.offset_beats", "Offset", 4.0, tonal.offset_beats),
    gain_pct!("tonal.swing", "Swing", tonal.swing),
    gain_pct!("tonal.randomness", "Randomness", tonal.randomness),
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
    gain_pct!("tonal.reverb_mix", "Reverb Mix", tonal.reverb_mix),
];

pub(crate) fn tonal_synth_type_label(value: f32) -> &'static str {
    match tonal_synth_type_index(value) {
        0 => "Sine",
        1 => "Rhodes",
        2 => "Wurli",
        3 => "Felt",
        4 => "Marimba",
        5 => "Kalimba",
        6 => "Pluck",
        7 => "Dulcet",
        8 => "Cloud Keys",
        _ => "Haze",
    }
}

pub(crate) fn tonal_synth_type_index(value: f32) -> usize {
    (value.round() as i64).rem_euclid(10) as usize
}

pub(crate) const CLAP_CONTROLS: &[ControlSpec] = &[
    gain_pct!("clap.level", "Level", clap.level),
    gain_pct!("clap.filter", "Filter", 0.5, 1.0, clap.filter),
    time_ms!("clap.decay_ms", "Decay", 10.0, 200.0, 1.0, clap.decay_ms),
    beat_interval!(
        "clap.interval_beats",
        "Interval",
        0.5,
        8.0,
        clap.interval_beats
    ),
    beat_offset!("clap.offset_beats", "Offset", 8.0, clap.offset_beats),
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
    time_ms!(
        "clap.slap_spread_ms",
        "Slap Spread",
        0.0,
        100.0,
        1.0,
        clap.slap_spread_ms
    ),
    gain_pct!("clap.room", "Room", clap.room),
    gain_pct!("clap.body", "Body", clap.body),
];

pub(crate) const ARP_CONTROLS: &[ControlSpec] = &[
    gain_pct!("arp.gain", "Level", arp.gain),
    time_secs!("arp.attack", "Attack", 0.0, 1.0, 0.001, arp.attack),
    time_secs!("arp.decay", "Decay", TONAL_DECAY_MIN, 6.0, 0.001, arp.decay),
    ControlSpec::new(
        "arp.type",
        "Type",
        ControlKind::Discrete,
        0.0,
        9.0,
        Step::Linear(1.0),
        Entry::Round,
        |c| c.arp.voice_type,
        |c, v| c.arp.voice_type = v,
        |c| tonal_synth_type_label(c.arp.voice_type).to_string(),
    ),
    beat_interval!(
        "arp.rate_beats",
        "Rate",
        ARP_RATE_BEATS_MIN,
        ARP_RATE_BEATS_MAX,
        arp.rate_beats
    ),
    beat_offset!("arp.offset_beats", "Offset", 4.0, arp.offset_beats),
    gain_pct!("arp.swing", "Swing", arp.swing),
    ControlSpec::new(
        "arp.pattern",
        "Pattern",
        ControlKind::Discrete,
        0.0,
        3.0,
        Step::Linear(1.0),
        Entry::Round,
        |c| c.arp.pattern,
        |c, v| c.arp.pattern = v,
        |c| arp_pattern_label(c.arp.pattern).to_string(),
    ),
    ControlSpec::new(
        "arp.octaves",
        "Octaves",
        ControlKind::Discrete,
        ARP_OCTAVES_MIN,
        ARP_OCTAVES_MAX,
        Step::Linear(1.0),
        Entry::Round,
        |c| c.arp.octaves,
        |c, v| c.arp.octaves = v,
        |c| format!("{:.0}", c.arp.octaves),
    ),
    gain_pct!("arp.reverb_mix", "Reverb Mix", arp.reverb_mix),
];

pub(crate) const MACRO_CONTROLS: &[ControlSpec] = &[
    gain_pct!("macro.1", "Macro 1", macros.values[0]),
    gain_pct!("macro.2", "Macro 2", macros.values[1]),
    gain_pct!("macro.3", "Macro 3", macros.values[2]),
    gain_pct!("macro.4", "Macro 4", macros.values[3]),
];

/// Whether a control id names one of the macro sliders. Macro sliders take
/// LFOs and envelopes but cannot themselves be macro targets.
pub(crate) fn is_macro_id(id: &str) -> bool {
    MACRO_CONTROLS.iter().any(|spec| spec.id == id)
}

/// The tab a control lives on natively (its deepest editing surface), so
/// Enter on a cross-tab row like the Master voice levels expands into that
/// voice's own tab. Master picks up its own rows via the fallback scan.
pub(crate) fn tab_owning_control(id: &str) -> Option<Tab> {
    let owner = Tab::all()
        .into_iter()
        .filter(|tab| *tab != Tab::Master)
        .find(|tab| tab_specs(*tab).iter().any(|spec| spec.id == id));
    owner.or_else(|| {
        MASTER_CONTROLS
            .iter()
            .any(|spec| spec.id == id)
            .then_some(Tab::Master)
    })
}

pub(crate) fn tab_specs(tab: Tab) -> &'static [ControlSpec] {
    TAB_META[tab as usize].3
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

/// Chords-tab visible rows for the given drill level: the 11 base params,
/// the active slots' Root list, or one slot's Accidental/Extension/Inversion.
/// Read-only view over `CHORDS_CONTROLS`'s fixed layout (11 base rows, then
/// 8 slots x 4 rows in degree/accidental/extension/inversion order) — never
/// reorders the underlying array.
pub(crate) fn chords_tab_controls(c: &FluidControls, drill: ChordDrill) -> Vec<ControlItem> {
    match drill {
        ChordDrill::None => CHORDS_CONTROLS[..11]
            .iter()
            .map(|spec| spec.item(c))
            .collect(),
        ChordDrill::Progression => {
            let count = (c.pad.chord_count.round() as usize).clamp(1, CHORD_SLOT_COUNT);
            (0..count)
                .map(|slot| CHORDS_CONTROLS[11 + 4 * slot].item(c))
                .collect()
        }
        ChordDrill::Slot(n) => {
            let base = 11 + 4 * n;
            [base + 1, base + 2, base + 3]
                .iter()
                .map(|&i| CHORDS_CONTROLS[i].item(c))
                .collect()
        }
    }
}

/// Maps a visible-row index under `chords_tab_controls` back to its real
/// index into `CHORDS_CONTROLS`, for the positional registry setters below.
pub(crate) fn chords_flat_index(drill: ChordDrill, visible_row: usize) -> usize {
    match drill {
        ChordDrill::None => visible_row,
        ChordDrill::Progression => 11 + 4 * visible_row,
        ChordDrill::Slot(n) => 11 + 4 * n + 1 + visible_row,
    }
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

/// Typed percent entry is always a plain integer meaning percent (`50` =>
/// 50%, `1` => 1%) — never a pre-divided ratio, so there is no ambiguous
/// small-value branch.
pub(crate) fn normalize_unit_input(value: f32) -> f32 {
    (value / 100.0).clamp(0.0, 1.0)
}

pub(crate) fn snap_step(value: f32, step: f32) -> f32 {
    (value / step).round() * step
}

/// Musical grid shared by every interval- and offset-like field: the 32nd
/// (0.125) survives only as a floor rung; everything above it locks to
/// sixteenths (0.25 multiples). A control whose own minimum sits below the
/// floor (offsets: 0 beats, meaning "no shift") keeps that true minimum as an
/// extra rung below 0.125, so "no offset" stays reachable.
pub(crate) const BEAT_GRID_FLOOR: f32 = 0.125;
pub(crate) const BEAT_GRID_STEP: f32 = 0.25;

pub(crate) fn beat_grid_snap(value: f32, min: f32, max: f32) -> f32 {
    let clamped = value.clamp(min, max);
    let low = if min < BEAT_GRID_FLOOR {
        min
    } else {
        BEAT_GRID_FLOOR
    };
    if low < BEAT_GRID_FLOOR && clamped <= (low + BEAT_GRID_FLOOR) / 2.0 {
        return low.clamp(min, max);
    }
    if clamped < (BEAT_GRID_FLOOR + BEAT_GRID_STEP) / 2.0 {
        return BEAT_GRID_FLOOR.clamp(min, max);
    }
    snap_step(clamped, BEAT_GRID_STEP).clamp(min, max)
}

pub(crate) fn beat_grid_adjust(value: f32, dir: f32, min: f32, max: f32) -> f32 {
    let current = beat_grid_snap(value, min, max);
    let low = if min < BEAT_GRID_FLOOR {
        min
    } else {
        BEAT_GRID_FLOOR
    };
    let next = if dir > 0.0 {
        if current < BEAT_GRID_FLOOR {
            BEAT_GRID_FLOOR
        } else if current <= BEAT_GRID_FLOOR {
            BEAT_GRID_STEP
        } else {
            current + BEAT_GRID_STEP
        }
    } else if current > BEAT_GRID_STEP {
        current - BEAT_GRID_STEP
    } else if current > BEAT_GRID_FLOOR {
        BEAT_GRID_FLOOR
    } else {
        low
    };
    beat_grid_snap(next, min, max)
}

pub(crate) fn nearest_power_of_two(value: f32, min: f32, max: f32) -> f32 {
    let clamped = value.clamp(min, max);
    let exponent = clamped.log2().round();
    2.0f32.powf(exponent).clamp(min, max)
}
