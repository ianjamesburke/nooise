//! The control registry: one `ControlSpec` table per tab, and the single
//! source of truth for every control row, its range and stepping, its stable
//! song-snapshot id, and how a value reads on screen.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tab {
    Chords = 0,
    Perc = 1,
    Bass = 2,
    Kick = 3,
    Tonal = 4,
    Clap = 5,
    Arp = 6,
    Master = 7,
}

pub(crate) const TAB_COUNT: usize = 8;

/// One row per tab: (variant, display name, mute-target level id, control
/// table) in discriminant order. `Tab::all`/`name`/`level_id`/`tab_specs`
/// all derive from indexing this single table by `self as usize`.
const TAB_META: [(Tab, &str, Option<&str>, &[ControlSpec]); TAB_COUNT] = [
    (Tab::Chords, "Pads", Some("pad.level"), CHORDS_CONTROLS),
    (Tab::Perc, "Perc", Some("perc.level"), PERC_CONTROLS),
    (Tab::Bass, "Bass", Some("bass.level"), BASS_CONTROLS),
    (Tab::Kick, "Kick", Some("kick.level"), KICK_CONTROLS),
    (Tab::Tonal, "Tonal", Some("tonal.level"), TONAL_CONTROLS),
    (Tab::Clap, "Clap", Some("clap.level"), CLAP_CONTROLS),
    (Tab::Arp, "Arp", Some("arp.gain"), ARP_CONTROLS),
    (Tab::Master, "Master", Some("master.level"), MASTER_CONTROLS),
];

impl Tab {
    pub(crate) fn all() -> [Tab; TAB_COUNT] {
        TAB_META.map(|(tab, _, _, _)| tab)
    }

    pub(crate) fn name(self) -> &'static str {
        TAB_META[self as usize].1
    }

    #[cfg(test)]
    pub(crate) fn previous(self) -> Self {
        let all = Self::all();
        all[(self as usize + all.len() - 1) % all.len()]
    }

    /// Stable id of this tab's primary level/gain control, or `None` for a
    /// The one place that maps a tab to its mute target, so `m`/`M` never
    /// need a per-voice match arm.
    pub(crate) fn level_id(self) -> Option<&'static str> {
        TAB_META[self as usize].2
    }
}

pub(crate) struct ControlItem {
    pub(crate) id: &'static str,
    /// Resolved at projection time: module-slot rows label themselves with
    /// whichever module is loaded, so a slot reads "Swing", not "Slot 1".
    pub(crate) label: String,
    pub(crate) kind: ControlKind,
    pub(crate) value: f32,
    pub(crate) min: f32,
    pub(crate) max: f32,
    /// Step ladder and continuous taper used by the shared 0..1 bar mapping.
    pub(crate) step: Step,
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

// ============================================================
// Control registry
//
// Single source of truth for every UI control row. Each row is one
// ControlSpec: range, step, numeric-entry semantics, reset target,
// accessors, and display formatting. tab_controls / apply_delta /
// apply_reset / apply_value all derive from these tables — adding a
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

impl Step {
    pub(crate) fn ratio(self, value: f32, min: f32, max: f32, taper: Taper) -> f32 {
        match self {
            Self::Linear(_) => taper.ratio(value, min, max),
            Self::PowerOfTwo => Taper::Log2.ratio(value, min, max),
            Self::BeatGrid => beat_grid_ratio(value, min, max),
        }
    }
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

/// Convert a tempo-relative duration to its current free-time equivalent.
pub(crate) fn beats_to_ms(beats: f32, bpm: f32) -> f32 {
    beats * 60_000.0 / bpm.max(1.0)
}

/// Convert a free-time duration to its current tempo-relative equivalent.
pub(crate) fn ms_to_beats(ms: f32, bpm: f32) -> f32 {
    ms * bpm.max(1.0) / 60_000.0
}

/// Re-express a duration at the current BPM without changing its audible
/// length. Callers own any target-grid snapping after this conversion.
pub(crate) fn convert_time_base(value: f32, from: TimeBase, to: TimeBase, bpm: f32) -> f32 {
    match (from, to) {
        (TimeBase::Beats, TimeBase::Ms) => beats_to_ms(value, bpm),
        (TimeBase::Ms, TimeBase::Beats) => ms_to_beats(value, bpm),
        _ => value,
    }
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
    pub(crate) exact_in_song: bool,
    pub(crate) get: GetFn,
    pub(crate) set: SetFn,
    pub(crate) display: DisplayFn,
    /// Overrides `label` per render. Only module-slot rows use it, so a slot
    /// names the module it holds instead of its index.
    pub(crate) label_of: Option<DisplayFn>,
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
            exact_in_song: false,
            get,
            set,
            display,
            label_of: None,
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

    /// Resolve this row's label per render instead of using the static one.
    pub(crate) const fn labeled_by(mut self, label_of: DisplayFn) -> Self {
        self.label_of = Some(label_of);
        self
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

    pub(crate) const fn exact_in_song(mut self) -> Self {
        self.exact_in_song = true;
        self
    }

    /// Resolve a stable slot field into the units owned by the module that is
    /// currently loaded there. Every editor, reset, automation route, and song
    /// decode crosses this seam, so detail rendering cannot disagree with the
    /// value semantics used by the engine.
    pub(crate) fn contextual(&self, c: &FluidControls) -> Self {
        let Some((slot, field)) = module_slot_row(self.id, c) else {
            return *self;
        };
        let Some(kind) = slot.kind() else {
            return *self;
        };
        let mut spec = *self;
        match (kind.family, field) {
            (Family::Delay, ModuleSlotField::Time | ModuleSlotField::RightTime) => {
                let clock = if field == ModuleSlotField::RightTime {
                    DelayClock::from_value(slot.right_clock)
                } else {
                    DelayClock::from_value(slot.clock)
                };
                spec.kind = ControlKind::Timing;
                spec.entry = Entry::Free;
                spec.taper = Taper::Linear;
                match clock {
                    DelayClock::Sync => {
                        spec.min = DELAY_SYNC_MIN_BEATS;
                        spec.max = DELAY_SYNC_MAX_BEATS;
                        spec.step = Step::BeatGrid;
                    }
                    DelayClock::Free => {
                        spec.min = DELAY_FREE_MIN_MS;
                        spec.max = DELAY_FREE_MAX_MS;
                        spec.step = Step::Linear(10.0);
                    }
                }
                spec.reset = spec.min;
            }
            (Family::Delay, ModuleSlotField::Feedback) => {
                spec.max = 0.95;
            }
            (Family::Reverb, ModuleSlotField::Time | ModuleSlotField::Feedback) => {
                spec.kind = ControlKind::Continuous;
                spec.min = 0.0;
                spec.max = 1.0;
                spec.step = Step::Linear(0.01);
                spec.entry = Entry::Percent;
                spec.reset = if field == ModuleSlotField::Time {
                    0.72
                } else {
                    0.45
                };
            }
            (Family::Compression, ModuleSlotField::Time) => {
                spec.kind = ControlKind::Continuous;
                spec.min = -40.0;
                spec.max = 0.0;
                spec.step = Step::Linear(1.0);
                spec.entry = Entry::Round;
                spec.reset = -8.0;
            }
            (Family::Compression, ModuleSlotField::RightTime) => {
                spec.kind = ControlKind::Continuous;
                spec.min = 1.0;
                spec.max = 8.0;
                spec.step = Step::Linear(0.25);
                spec.entry = Entry::Snap;
                spec.reset = 2.0;
            }
            (Family::Compression, ModuleSlotField::Feedback) => {
                spec.kind = ControlKind::Timing;
                spec.min = 10.0;
                spec.max = 500.0;
                spec.step = Step::Linear(1.0);
                spec.entry = Entry::Round;
                spec.reset = 100.0;
            }
            (Family::Compression, ModuleSlotField::Vintage) => {
                spec.min = 0.0;
                spec.max = 12.0;
                spec.step = Step::Linear(0.5);
                spec.entry = Entry::Free;
                spec.reset = 2.0;
            }
            _ => {}
        }
        spec
    }

    pub(crate) fn item(&self, c: &FluidControls) -> ControlItem {
        let spec = self.contextual(c);
        ControlItem {
            id: spec.id,
            label: spec
                .label_of
                .map_or_else(|| spec.label.to_string(), |resolve| resolve(c)),
            kind: spec.kind,
            value: (spec.get)(c),
            min: spec.min,
            max: spec.max,
            step: spec.step,
            taper: spec.taper,
            display: (spec.display)(c),
        }
    }

    pub(crate) fn apply_delta(&self, dir: f32, c: &mut FluidControls) {
        let spec = self.contextual(c);
        let value = (spec.get)(c);
        let next = if spec.is_continuous_tapered() {
            // A tapered continuous dial steps in position space, so each press
            // moves an equal fraction of the throw — fine near the floor,
            // coarse near the ceiling (log-even octaves for Log2, low-biased
            // for Exp) — instead of a fixed value delta.
            let ratio = spec.taper.ratio(value, spec.min, spec.max);
            let stepped = (ratio + dir / TAPER_STEPS_PER_SWEEP).clamp(0.0, 1.0);
            spec.taper
                .value_at(stepped, spec.min, spec.max)
                .clamp(spec.min, spec.max)
        } else {
            match spec.step {
                Step::Linear(step) => (value + dir * step).clamp(spec.min, spec.max),
                Step::PowerOfTwo => {
                    if dir > 0.0 {
                        (value * 2.0).min(spec.max)
                    } else {
                        (value / 2.0).max(spec.min)
                    }
                }
                Step::BeatGrid => beat_grid_adjust(value, dir, spec.min, spec.max),
            }
        };
        (spec.set)(c, next);
    }

    pub(crate) fn ratio(&self, value: f32, c: &FluidControls) -> f32 {
        let spec = self.contextual(c);
        spec.step.ratio(value, spec.min, spec.max, spec.taper)
    }

    /// A continuous dial with a non-linear taper and a plain `Linear` step:
    /// stepped in position space and stored at full precision. Discrete grids
    /// (`PowerOfTwo`/`BeatGrid`) keep their own musical stepping even under a
    /// `Log2` bar (e.g. chord bars doubling on octaves).
    fn is_continuous_tapered(&self) -> bool {
        !matches!(self.taper, Taper::Linear) && matches!(self.step, Step::Linear(_))
    }

    pub(crate) fn apply_reset(&self, c: &mut FluidControls) {
        let spec = self.contextual(c);
        (spec.set)(c, spec.reset);
    }

    pub(crate) fn apply_value(&self, value: f32, c: &mut FluidControls) {
        let spec = self.contextual(c);
        let next = match spec.entry {
            Entry::Percent if spec.id.contains(".slot") => normalize_unit_input(value),
            Entry::Percent => normalize_unit_input(value) * spec.max,
            Entry::BeatsAsBars => nearest_power_of_two(value / 4.0, spec.min, spec.max),
            Entry::Round => value.round(),
            Entry::Snap => spec.snap_on_grid(value),
            Entry::Free => value,
        };
        (spec.set)(c, next.clamp(spec.min, spec.max));
    }

    pub(crate) fn quantized_value(&self, c: &FluidControls) -> f32 {
        let spec = self.contextual(c);
        spec.quantize((spec.get)(c))
    }

    pub(crate) fn apply_quantized_value(&self, value: f32, c: &mut FluidControls) {
        let spec = self.contextual(c);
        (spec.set)(c, spec.quantize(value));
    }

    /// Set an exact value, clamped to range but not snapped to the step
    /// grid — used while a time control is being driven in its flipped unit.
    pub(crate) fn apply_raw(&self, value: f32, c: &mut FluidControls) {
        let spec = self.contextual(c);
        (spec.set)(c, value.clamp(spec.min, spec.max));
    }

    pub(crate) fn quantize(&self, value: f32) -> f32 {
        let clamped = value.clamp(self.min, self.max);
        // Tapered continuous dials move in position space, so they carry no
        // value grid: keep the exact value rather than snapping to a spurious
        // step. These are the only rows a song code cannot round-trip exactly
        // — with no grid to re-snap onto, they decode from a u16 taper
        // position and land within one position step of the original.
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

/// One module slot's rows: which module is loaded (a value, never part of
/// the id) plus the family-shaped params. Generalises `chord_slot_rows!`.
/// Slot numbers are 1-based in ids and labels, 0-based into the array.
/// Reached only through `layer_controls!`, which appends every slot.
macro_rules! module_slot_rows {
    ($layer:ident, $prefix:literal, $slot:literal) => {
        [
            ControlSpec::new(
                concat!($prefix, ".slot", $slot, ".kind"),
                concat!("Slot ", $slot),
                ControlKind::Discrete,
                MODULE_EMPTY,
                module_kind_max(),
                Step::Linear(1.0),
                Entry::Round,
                |c| c.modules.$layer[$slot - 1].kind,
                |c, v| c.modules.$layer[$slot - 1].kind = v,
                |c| module_kind_label(c.modules.$layer[$slot - 1].kind),
            )
            .reset_at(MODULE_EMPTY),
            ControlSpec::new(
                concat!($prefix, ".slot", $slot, ".amount"),
                concat!("Slot ", $slot, " Amount"),
                ControlKind::Gain,
                0.0,
                1.0,
                Step::Linear(0.01),
                Entry::Percent,
                |c| c.modules.$layer[$slot - 1].amount,
                |c, v| c.modules.$layer[$slot - 1].amount = v,
                |c| format!("{:.0}%", c.modules.$layer[$slot - 1].amount * 100.0),
            )
            .labeled_by(|c| module_kind_label(c.modules.$layer[$slot - 1].kind))
            .reset_at(0.0),
            ControlSpec::new(
                concat!($prefix, ".slot", $slot, ".time"),
                concat!("Slot ", $slot, " Time"),
                ControlKind::Continuous,
                0.0,
                2_000.0,
                Step::Linear(0.01),
                Entry::Free,
                |c| c.modules.$layer[$slot - 1].time,
                |c, v| c.modules.$layer[$slot - 1].time = v,
                |c| format!("{:.0}", c.modules.$layer[$slot - 1].time),
            )
            .reset_at(0.0)
            .exact_in_song(),
            ControlSpec::new(
                concat!($prefix, ".slot", $slot, ".right_time"),
                concat!("Slot ", $slot, " Right Time"),
                ControlKind::Continuous,
                0.0,
                2_000.0,
                Step::Linear(0.01),
                Entry::Free,
                |c| c.modules.$layer[$slot - 1].right_time,
                |c, v| c.modules.$layer[$slot - 1].right_time = v,
                |c| format!("{:.0}", c.modules.$layer[$slot - 1].right_time),
            )
            .reset_at(0.0)
            .exact_in_song(),
            ControlSpec::new(
                concat!($prefix, ".slot", $slot, ".clock"),
                concat!("Slot ", $slot, " Clock"),
                ControlKind::Discrete,
                0.0,
                1.0,
                Step::Linear(1.0),
                Entry::Round,
                |c| c.modules.$layer[$slot - 1].clock,
                |c, v| c.modules.$layer[$slot - 1].clock = v,
                |c| match DelayClock::from_value(c.modules.$layer[$slot - 1].clock) {
                    DelayClock::Sync => "Sync".to_string(),
                    DelayClock::Free => "Free".to_string(),
                },
            )
            .reset_at(DelayClock::Sync.value()),
            ControlSpec::new(
                concat!($prefix, ".slot", $slot, ".feedback"),
                concat!("Slot ", $slot, " Feedback"),
                ControlKind::Gain,
                0.0,
                1.0,
                Step::Linear(0.01),
                Entry::Percent,
                |c| c.modules.$layer[$slot - 1].feedback,
                // Not redundant with `contextual`'s 0.95 Delay max: auto morph
                // writes through the base spec, and a leg whose slot changes
                // family glides this field from the old module's range (Reverb
                // damping to 1.0, Compression release to 500 ms) while the
                // discrete kind row has already snapped to Delay. Without this
                // the delay line would run above unity for that window.
                |c, v| {
                    let slot = &mut c.modules.$layer[$slot - 1];
                    slot.feedback = if slot.kind().is_some_and(|kind| kind.family == Family::Delay)
                    {
                        v.min(0.95)
                    } else {
                        v
                    };
                },
                |c| format!("{:.0}%", c.modules.$layer[$slot - 1].feedback * 100.0),
            )
            .reset_at(0.0)
            .exact_in_song(),
            ControlSpec::new(
                concat!($prefix, ".slot", $slot, ".vintage"),
                concat!("Slot ", $slot, " Vintage"),
                ControlKind::Gain,
                0.0,
                1.0,
                Step::Linear(0.01),
                Entry::Percent,
                |c| c.modules.$layer[$slot - 1].vintage,
                |c, v| c.modules.$layer[$slot - 1].vintage = v,
                |c| format!("{:.0}%", c.modules.$layer[$slot - 1].vintage * 100.0),
            )
            .reset_at(0.0)
            .exact_in_song(),
            ControlSpec::new(
                concat!($prefix, ".slot", $slot, ".right_clock"),
                concat!("Slot ", $slot, " Right Clock"),
                ControlKind::Discrete,
                0.0,
                1.0,
                Step::Linear(1.0),
                Entry::Round,
                |c| c.modules.$layer[$slot - 1].right_clock,
                |c, v| c.modules.$layer[$slot - 1].right_clock = v,
                |c| match DelayClock::from_value(c.modules.$layer[$slot - 1].right_clock) {
                    DelayClock::Sync => "Sync".to_string(),
                    DelayClock::Free => "Free".to_string(),
                },
            )
            .reset_at(DelayClock::Sync.value()),
        ]
    };
}

/// One chord slot's four fields as `ControlSpec` rows (Root/Accidental/
/// Extension/Inversion), appended to `CHORDS_CONTROLS` by `layer_controls!`.
/// Slot numbers are 1-based in ids/labels, 0-based into `chord_slots`.
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
                concat!("pad.chord", $slot, "_quality"),
                concat!("Chord ", $slot, " Quality"),
                ControlKind::Discrete,
                -1.0,
                1.0,
                Step::Linear(1.0),
                Entry::Round,
                |c| c.pad.chord_slots[$slot - 1].quality,
                |c, v| c.pad.chord_slots[$slot - 1].quality = v,
                |c| {
                    let slot = &c.pad.chord_slots[$slot - 1];
                    let sound = if pad_chord_slot_is_minor(slot) {
                        "min"
                    } else {
                        "maj"
                    };
                    match slot.quality.round() as i32 {
                        0 => format!("scale ({sound})"),
                        _ => sound.to_string(),
                    }
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

/// One layer's whole `ControlSpec` table: its hand-written base rows, then
/// the pad's eight chord slots (Pads only), then the eight module slots.
/// The tail rows are identical for every layer, so each table names only what
/// is specific to it.
macro_rules! layer_controls {
    ($layer:ident, $prefix:literal, [$($base:expr),* $(,)?]) => {
        layer_controls!(@rows $layer, $prefix, [$($base),*], [], [1, 2, 3, 4, 5, 6, 7, 8])
    };
    (chords $layer:ident, $prefix:literal, [$($base:expr),* $(,)?]) => {
        layer_controls!(
            @rows $layer,
            $prefix,
            [$($base),*],
            [1, 2, 3, 4, 5, 6, 7, 8],
            [1, 2, 3, 4, 5, 6, 7, 8]
        )
    };
    (@rows $layer:ident, $prefix:literal, [$($base:expr),*], [$($chord:literal),*], [$($slot:literal),*]) => {
        [
            $($base,)*
            $(
                chord_slot_rows!($chord)[0],
                chord_slot_rows!($chord)[1],
                chord_slot_rows!($chord)[2],
                chord_slot_rows!($chord)[3],
                chord_slot_rows!($chord)[4],
            )*
            $(
                module_slot_rows!($layer, $prefix, $slot)[0],
                module_slot_rows!($layer, $prefix, $slot)[1],
                module_slot_rows!($layer, $prefix, $slot)[2],
                module_slot_rows!($layer, $prefix, $slot)[3],
                module_slot_rows!($layer, $prefix, $slot)[4],
                module_slot_rows!($layer, $prefix, $slot)[5],
                module_slot_rows!($layer, $prefix, $slot)[6],
                module_slot_rows!($layer, $prefix, $slot)[7],
            )*
        ]
    };
}

/// `layer_controls!` enumerates its slots literally.
const _: () = assert!(MODULE_SLOTS == 8);
const _: () = assert!(CHORD_SLOT_COUNT == 8);

pub(crate) const MASTER_CONTROLS: &[ControlSpec] = &layer_controls!(
    master,
    "master",
    [
        gain_pct!("pad.level", "Pads Vol", pad.level),
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
    ]
);

pub(crate) const PERC_CONTROLS: &[ControlSpec] = &layer_controls!(
    perc,
    "perc",
    [
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
    ]
);

const CHORD_BASE_CONTROL_COUNT: usize = 10;

pub(crate) const CHORDS_CONTROLS: &[ControlSpec] = &layer_controls!(chords pad, "pad", [
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
        last_index_of(PAD_TYPES),
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
    gain_pct!("pad.stereo_width", "Stereo Width", pad.stereo_width),
    gain_pct!("pad.detune", "Detune", pad.detune),
    gain_pct!("pad.octave_mix", "Octave Mix", pad.octave_mix),
]);

pub(crate) const BASS_CONTROLS: &[ControlSpec] = &layer_controls!(
    bass,
    "bass",
    [
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
            last_index_of(BASS_TYPES),
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
    ]
);

/// The character choices behind each voice-type control, in stored-value
/// order. A control's `max` is `last_index_of` its own table, so a table and
/// its slider range can never disagree.
pub(crate) const BASS_TYPES: &[&str] = &["Sub", "Saw", "Pluck"];
pub(crate) const PAD_TYPES: &[&str] = &["Warm", "Dark", "Glass", "Choir", "Hollow", "Tape"];
pub(crate) const KICK_TYPES: &[&str] = &["Sub", "Warm", "Wood", "Felt"];
pub(crate) const TONAL_SYNTH_TYPES: &[&str] = &[
    "Sine",
    "Rhodes",
    "Wurli",
    "Felt",
    "Marimba",
    "Kalimba",
    "Pluck",
    "Dulcet",
    "Cloud Keys",
    "Haze",
];

/// A voice-type value wraps into its table, so the dial cycles rather than
/// dying at either end.
fn type_index(value: f32, types: &[&str]) -> usize {
    (value.round() as i64).rem_euclid(types.len() as i64) as usize
}

fn type_label(value: f32, types: &'static [&'static str]) -> &'static str {
    types[type_index(value, types)]
}

/// Highest value a voice-type control accepts, for its `ControlSpec` max.
const fn last_index_of(types: &[&str]) -> f32 {
    (types.len() - 1) as f32
}

pub(crate) fn bass_type_label(value: f32) -> &'static str {
    type_label(value, BASS_TYPES)
}

pub(crate) fn bass_type_index(value: f32) -> usize {
    type_index(value, BASS_TYPES)
}

pub(crate) fn pad_type_label(value: f32) -> &'static str {
    type_label(value, PAD_TYPES)
}

pub(crate) fn pad_type_index(value: f32) -> usize {
    type_index(value, PAD_TYPES)
}

pub(crate) fn kick_type_label(value: f32) -> &'static str {
    type_label(value, KICK_TYPES)
}

pub(crate) fn kick_type_index(value: f32) -> usize {
    type_index(value, KICK_TYPES)
}

pub(crate) const KICK_CONTROLS: &[ControlSpec] = &layer_controls!(
    kick,
    "kick",
    [
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
            last_index_of(KICK_TYPES),
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
    ]
);

pub(crate) const TONAL_CONTROLS: &[ControlSpec] = &layer_controls!(
    tonal,
    "tonal",
    [
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
            last_index_of(TONAL_SYNTH_TYPES),
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
    ]
);

pub(crate) fn tonal_synth_type_label(value: f32) -> &'static str {
    type_label(value, TONAL_SYNTH_TYPES)
}

pub(crate) fn tonal_synth_type_index(value: f32) -> usize {
    type_index(value, TONAL_SYNTH_TYPES)
}

pub(crate) const CLAP_CONTROLS: &[ControlSpec] = &layer_controls!(
    clap,
    "clap",
    [
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
        gain_pct!("clap.body", "Body", clap.body),
    ]
);

pub(crate) const ARP_CONTROLS: &[ControlSpec] = &layer_controls!(
    arp,
    "arp",
    [
        gain_pct!("arp.gain", "Level", arp.gain),
        time_secs!("arp.attack", "Attack", 0.0, 1.0, 0.001, arp.attack),
        time_secs!("arp.decay", "Decay", TONAL_DECAY_MIN, 6.0, 0.001, arp.decay),
        ControlSpec::new(
            "arp.type",
            "Type",
            ControlKind::Discrete,
            0.0,
            last_index_of(TONAL_SYNTH_TYPES),
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
    ]
);

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

/// Registry-backed target for the shared Deck/Sequence performance grammar.
/// Musical words stay stable while every actual edit still uses the owning
/// control's range, taper, and step semantics.
pub(crate) fn performance_target(
    instrument: interaction::PerformanceInstrument,
    action: interaction::PerformanceAction,
) -> Option<(Tab, usize, &'static ControlSpec, f32)> {
    let (tab, level, shape, density) = match instrument {
        interaction::PerformanceInstrument::Pads => (
            Tab::Chords,
            "pad.level",
            "pad.release_time",
            "pad.chord_bars",
        ),
        interaction::PerformanceInstrument::Bass => (
            Tab::Bass,
            "bass.level",
            "bass.decay_time",
            "bass.interval_beats",
        ),
        interaction::PerformanceInstrument::Kick => (
            Tab::Kick,
            "kick.level",
            "kick.amp_decay_ms",
            "kick.interval_beats",
        ),
        interaction::PerformanceInstrument::Perc => (
            Tab::Perc,
            "perc.level",
            "perc.decay_ms",
            "perc.interval_beats",
        ),
    };
    let (id, direction) = match action {
        interaction::PerformanceAction::Shorter => (shape, -1.0),
        interaction::PerformanceAction::Longer => (shape, 1.0),
        interaction::PerformanceAction::Quieter => (level, -1.0),
        interaction::PerformanceAction::Louder => (level, 1.0),
        interaction::PerformanceAction::Sparser => (density, 1.0),
        interaction::PerformanceAction::Denser => (density, -1.0),
    };
    let index = tab_specs(tab).iter().position(|spec| spec.id == id)?;
    Some((tab, index, &tab_specs(tab)[index], direction))
}

pub(crate) fn tab_controls(tab: Tab, c: &FluidControls) -> Vec<ControlItem> {
    tab_specs(tab)
        .iter()
        .filter(|spec| module_slot_row_visible(spec.id, c))
        .map(|spec| spec.item(c))
        .collect()
}

/// The `amount` control id for a tab's slot, which is the row a loaded slot
/// actually renders. Compile-time strings, so this is a lookup rather than a
/// format, and returns `None` for a tab with no chain.
pub(crate) fn module_slot_amount_id(tab: Tab, slot: usize) -> Option<&'static str> {
    let suffix = format!(".slot{}.amount", slot + 1);
    tab_specs(tab)
        .iter()
        .map(|spec| spec.id)
        .find(|id| id.ends_with(&suffix))
}

/// Loaded module slot addressed by its collapsed amount row.
pub(crate) fn module_slot_at_amount_id<'a>(
    tab: Tab,
    id: &str,
    controls: &'a FluidControls,
) -> Option<(usize, &'a ModuleSlot)> {
    let slots = controls.modules.for_tab(tab)?;
    slots.iter().enumerate().find(|(slot, _)| {
        module_slot_amount_id(tab, *slot).is_some_and(|amount_id| amount_id == id)
    })
}

/// Loaded module slot addressed by any one of its static parameter ids.
pub(crate) fn module_slot_at_id<'a>(
    tab: Tab,
    id: &str,
    controls: &'a FluidControls,
) -> Option<(usize, &'a ModuleSlot)> {
    let slots = controls.modules.for_tab(tab)?;
    slots.iter().enumerate().find(|(slot, _)| {
        let prefix = format!(".slot{}.", slot + 1);
        id.contains(&prefix)
    })
}

/// The rows projected inside a loaded module's detail scope. The backing
/// registry ids stay slot-addressed and therefore persist independently of
/// whichever catalog module currently occupies the slot.
pub(crate) fn module_detail_controls(
    tab: Tab,
    slot: usize,
    controls: &FluidControls,
) -> Vec<ControlItem> {
    let prefix = format!(".slot{}.", slot + 1);
    let Some(kind) = controls
        .modules
        .for_tab(tab)
        .and_then(|slots| slots.get(slot))
        .and_then(ModuleSlot::kind)
    else {
        return Vec::new();
    };
    let mut items = kind
        .parameters()
        .iter()
        .filter_map(|parameter| {
            let field = parameter.field.id();
            tab_specs(tab)
                .iter()
                .find(|spec| spec.id.ends_with(&format!("{prefix}{field}")))
                .map(|spec| {
                    let mut item = spec.item(controls);
                    item.label = parameter.label.to_string();
                    item
                })
        })
        .collect::<Vec<_>>();
    if let Some((_, slot)) = module_slot_at_amount_id(
        tab,
        module_slot_amount_id(tab, slot).unwrap_or_default(),
        controls,
    ) && slot.kind().is_some_and(|kind| kind.family == Family::Delay)
    {
        for item in &mut items {
            if item.id.ends_with(".feedback") {
                item.max = 0.95;
            }
            if matches!(item.id.rsplit('.').next(), Some("time" | "right_time")) {
                let clock = if item.id.ends_with(".right_time") {
                    DelayClock::from_value(slot.right_clock)
                } else {
                    DelayClock::from_value(slot.clock)
                };
                match clock {
                    DelayClock::Sync => {
                        item.kind = ControlKind::Timing;
                        item.min = DELAY_SYNC_MIN_BEATS;
                        item.max = DELAY_SYNC_MAX_BEATS;
                        item.step = Step::BeatGrid;
                        item.display = beats2(item.value);
                    }
                    DelayClock::Free => {
                        item.kind = ControlKind::Timing;
                        item.min = DELAY_FREE_MIN_MS;
                        item.max = DELAY_FREE_MAX_MS;
                        item.step = Step::Linear(10.0);
                        item.display = secs(item.value / 1000.0);
                    }
                }
            }
        }
    }
    if kind.family == Family::Reverb {
        for item in &mut items {
            if matches!(item.id.rsplit('.').next(), Some("time" | "feedback")) {
                item.display = pct(item.value);
            }
        }
    }
    if kind.family == Family::Compression {
        for item in &mut items {
            item.display = match item.id.rsplit('.').next() {
                Some("amount") => pct(item.value),
                Some("time") => format!("{:.0} dB", item.value),
                Some("right_time") => format!("{:.1}:1", item.value),
                Some("feedback") => format!("{:.0} ms", item.value),
                Some("vintage") => format!("{:.1} dB", item.value),
                _ => item.display.clone(),
            };
        }
    }
    items
}

/// Whether a module-slot row belongs on screen. Every layer carries
/// `MODULE_SLOTS` slots whether or not anything is loaded, so an empty slot's
/// three rows must not render: eight empty slots per layer would bury every
/// page under blank rows and break the 15-second floor. Non-slot ids always
/// show. An occupied slot shows its kind row plus whichever params its
/// family actually uses.
pub(crate) fn module_slot_row_visible(id: &str, c: &FluidControls) -> bool {
    let Some((slot, field)) = module_slot_row(id, c) else {
        return true;
    };
    if slot.is_empty() {
        return false;
    }
    match field {
        // A loaded slot collapses to one row: its amount, labelled with the
        // module's name. Which module is loaded is chosen through the palette,
        // so `kind` needs no row of its own.
        ModuleSlotField::Kind => false,
        ModuleSlotField::Amount => true,
        // The established two-knob family remains inline. Delay is the first
        // family that enters the reusable detail scope.
        ModuleSlotField::Time => slot
            .kind()
            .is_some_and(|kind| kind.family == Family::TwoKnob),
        ModuleSlotField::RightTime
        | ModuleSlotField::Clock
        | ModuleSlotField::RightClock
        | ModuleSlotField::Feedback
        | ModuleSlotField::Vintage => false,
    }
}

impl ModuleSlotField {
    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::Kind => "kind",
            Self::Amount => "amount",
            Self::Time => "time",
            Self::RightTime => "right_time",
            Self::Clock => "clock",
            Self::RightClock => "right_clock",
            Self::Feedback => "feedback",
            Self::Vintage => "vintage",
        }
    }
}

/// Parse `<layer>.slot<N>.<field>` into its layer prefix, 0-based slot index,
/// and field, independent of any `FluidControls`. `None` for anything else.
/// The single place that recognizes a module-slot id from its shape alone —
/// every other module-slot-aware lookup (including the Chords-tab drill
/// addressing below) goes through this rather than re-deriving the pattern.
fn parse_module_slot_id(id: &str) -> Option<(&str, usize, ModuleSlotField)> {
    let (layer, rest) = id.split_once(".slot")?;
    let (index, field) = rest.split_once('.')?;
    let field = match field {
        "kind" => ModuleSlotField::Kind,
        "amount" => ModuleSlotField::Amount,
        "time" => ModuleSlotField::Time,
        "right_time" => ModuleSlotField::RightTime,
        "clock" => ModuleSlotField::Clock,
        "right_clock" => ModuleSlotField::RightClock,
        "feedback" => ModuleSlotField::Feedback,
        "vintage" => ModuleSlotField::Vintage,
        _ => return None,
    };
    let index = index.parse::<usize>().ok()?.checked_sub(1)?;
    Some((layer, index, field))
}

/// Parse `pad.chord<N>_<field>` into its 0-based chord slot and field index
/// (root, accidental, quality, extension, inversion — `chord_slot_rows!`'s
/// emission order). `None` for anything else, including module-slot ids
/// (`.slotN.`, not `chordN_`), so it never misclassifies one as the other.
fn parse_chord_slot_id(id: &str) -> Option<(usize, usize)> {
    let rest = id.strip_prefix("pad.chord")?;
    let (num, field) = rest.split_once('_')?;
    let field = match field {
        "degree" => 0,
        "accidental" => 1,
        "quality" => 2,
        "extension" => 3,
        "inversion" => 4,
        _ => return None,
    };
    let slot = num.parse::<usize>().ok()?.checked_sub(1)?;
    Some((slot, field))
}

/// Parse `<layer>.slot<N>.<field>` back to the slot it addresses. `None` for
/// any id that is not a module-slot row.
fn module_slot_row<'a>(
    id: &str,
    c: &'a FluidControls,
) -> Option<(&'a ModuleSlot, ModuleSlotField)> {
    let (layer, index, field) = parse_module_slot_id(id)?;
    let slots: &[ModuleSlot; MODULE_SLOTS] = match layer {
        "pad" => &c.modules.pad,
        "perc" => &c.modules.perc,
        "bass" => &c.modules.bass,
        "kick" => &c.modules.kick,
        "tonal" => &c.modules.tonal,
        "clap" => &c.modules.clap,
        "arp" => &c.modules.arp,
        "master" => &c.modules.master,
        _ => return None,
    };
    slots.get(index).map(|slot| (slot, field))
}

/// Chords-tab visible rows for the given drill level: the 10 base params
/// plus any occupied module slots, the active chord slots' Root list, or one
/// chord slot's Accidental/Quality/Extension/Inversion. Read-only view over
/// `CHORDS_CONTROLS`'s fixed layout (10 base rows, then 8 chord slots x 5
/// rows, then 8 module slots x 8 rows) — never reorders the underlying
/// array. `chords_drill_for_index` below is this projection's inverse and
/// must stay consistent with it for every region.
pub(crate) fn chords_tab_controls(
    c: &FluidControls,
    drill: interaction::ChordDrill,
) -> Vec<ControlItem> {
    match drill {
        interaction::ChordDrill::None => CHORDS_CONTROLS[..CHORD_BASE_CONTROL_COUNT]
            .iter()
            .chain(CHORDS_CONTROLS[CHORD_BASE_CONTROL_COUNT + CHORD_SLOT_COUNT * 5..].iter())
            .filter(|spec| module_slot_row_visible(spec.id, c))
            .map(|spec| spec.item(c))
            .collect(),
        interaction::ChordDrill::Progression { .. } => {
            let count = (c.pad.chord_count.round() as usize).clamp(1, CHORD_SLOT_COUNT);
            (0..count)
                .map(|slot| CHORDS_CONTROLS[CHORD_BASE_CONTROL_COUNT + 5 * slot].item(c))
                .collect()
        }
        interaction::ChordDrill::Slot { slot, .. } => {
            let base = CHORD_BASE_CONTROL_COUNT + 5 * slot;
            [base + 1, base + 2, base + 3, base + 4]
                .iter()
                .map(|&i| CHORDS_CONTROLS[i].item(c))
                .collect()
        }
    }
}

/// Maps a visible-row index under `chords_tab_controls` back to its real
/// index into `CHORDS_CONTROLS`, for the chord-slot drill tests below. Covers
/// only the base and chord-slot regions, where the mapping is fixed; it is
/// not a complete inverse for module-slot rows, whose visible position
/// depends on which slots are occupied (see `chords_drill_for_index`, which
/// handles all three regions and is the real inverse of `chords_tab_controls`).
#[cfg(test)]
pub(crate) fn chords_flat_index(drill: interaction::ChordDrill, visible_row: usize) -> usize {
    match drill {
        interaction::ChordDrill::None => visible_row,
        interaction::ChordDrill::Progression { .. } => CHORD_BASE_CONTROL_COUNT + 5 * visible_row,
        interaction::ChordDrill::Slot { slot, .. } => {
            CHORD_BASE_CONTROL_COUNT + 5 * slot + 1 + visible_row
        }
    }
}

/// Inverse of `chords_tab_controls`: the drill level + visible row that
/// shows a real `CHORDS_CONTROLS` index, so a palette jump or a freshly
/// placed module can land on the right row. A control's region is read from
/// its own id via `parse_chord_slot_id`, never re-derived from its position
/// — position-based arithmetic on every index past the base rows is exactly
/// what silently broke this function when the module-slot region was
/// appended after the chord-slot region (module-slot rows have no chord-slot
/// meaning, but old code divided their index by the chord-slot stride
/// anyway). Base rows and module-slot rows share `ChordDrill::None`'s
/// projection, so their visible row is resolved by asking that same
/// projection, which keeps this function correct automatically if its
/// filtering rules ever change instead of requiring a second hand-edit here.
pub(crate) fn chords_drill_for_index(
    flat: usize,
    c: &FluidControls,
) -> (interaction::ChordDrill, usize) {
    let Some(spec) = CHORDS_CONTROLS.get(flat) else {
        return (interaction::ChordDrill::None, 0);
    };
    if let Some((slot, field)) = parse_chord_slot_id(spec.id) {
        return if field == 0 {
            (interaction::ChordDrill::Progression { return_to: 4 }, slot)
        } else {
            (
                interaction::ChordDrill::Slot { slot, return_to: 4 },
                field - 1,
            )
        };
    }
    let selected = chords_tab_controls(c, interaction::ChordDrill::None)
        .iter()
        .position(|item| item.id == spec.id)
        .unwrap_or(0);
    (interaction::ChordDrill::None, selected)
}

pub(crate) fn apply_delta(tab: Tab, selected: usize, dir: f32, c: &mut FluidControls) {
    if let Some(spec) = tab_specs(tab).get(selected) {
        spec.apply_delta(dir, c);
    }
}

pub(crate) fn apply_reset(tab: Tab, selected: usize, c: &mut FluidControls) {
    if let Some(spec) = tab_specs(tab).get(selected) {
        spec.apply_reset(c);
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

/// Position a value across an irregular ordered step ladder. Every adjacent
/// pair gets the same share of the visual throw; exact values between rungs
/// interpolate within that share.
pub(crate) fn ordered_step_ratio(value: f32, steps: &[f32]) -> f32 {
    let Some((&first, rest)) = steps.split_first() else {
        return 0.0;
    };
    if rest.is_empty() || value <= first {
        return 0.0;
    }
    let last = *rest.last().unwrap_or(&first);
    if value >= last {
        return 1.0;
    }

    let upper = steps.partition_point(|step| *step <= value);
    let lower = upper - 1;
    let local = (value - steps[lower]) / (steps[upper] - steps[lower]);
    (lower as f32 + local) / (steps.len() - 1) as f32
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

/// Position on the beat grid by reachable arrow rungs, not raw beat value.
/// This gives 0 -> 0.125 the same visual distance as 0.125 -> 0.25 and every
/// later sixteenth step.
pub(crate) fn beat_grid_ratio(value: f32, min: f32, max: f32) -> f32 {
    if max <= min {
        return 0.0;
    }
    let value = value.clamp(min, max);
    let mut rung = min;
    let mut rung_index = 0usize;
    let mut value_position = 0.0;
    let mut found = value <= min;

    while rung < max {
        let next = beat_grid_adjust(rung, 1.0, min, max);
        if next <= rung {
            break;
        }
        if !found && value <= next {
            let local = (value - rung) / (next - rung);
            value_position = rung_index as f32 + local;
            found = true;
        }
        rung = next;
        rung_index += 1;
    }

    if rung_index == 0 {
        0.0
    } else if found {
        value_position / rung_index as f32
    } else {
        1.0
    }
}

pub(crate) fn nearest_power_of_two(value: f32, min: f32, max: f32) -> f32 {
    let clamped = value.clamp(min, max);
    let exponent = clamped.log2().round();
    2.0f32.powf(exponent).clamp(min, max)
}

#[cfg(test)]
mod performance_tests {
    use super::*;
    use crate::fluid::interaction::{PerformanceAction, PerformanceInstrument};

    #[test]
    fn performance_vocabulary_resolves_every_instrument_action_through_registry() {
        let actions = [
            PerformanceAction::Shorter,
            PerformanceAction::Longer,
            PerformanceAction::Quieter,
            PerformanceAction::Louder,
            PerformanceAction::Sparser,
            PerformanceAction::Denser,
        ];
        let expected = [
            (
                PerformanceInstrument::Pads,
                [
                    ("pad.release_time", -1.0),
                    ("pad.release_time", 1.0),
                    ("pad.level", -1.0),
                    ("pad.level", 1.0),
                    ("pad.chord_bars", 1.0),
                    ("pad.chord_bars", -1.0),
                ],
            ),
            (
                PerformanceInstrument::Bass,
                [
                    ("bass.decay_time", -1.0),
                    ("bass.decay_time", 1.0),
                    ("bass.level", -1.0),
                    ("bass.level", 1.0),
                    ("bass.interval_beats", 1.0),
                    ("bass.interval_beats", -1.0),
                ],
            ),
            (
                PerformanceInstrument::Kick,
                [
                    ("kick.amp_decay_ms", -1.0),
                    ("kick.amp_decay_ms", 1.0),
                    ("kick.level", -1.0),
                    ("kick.level", 1.0),
                    ("kick.interval_beats", 1.0),
                    ("kick.interval_beats", -1.0),
                ],
            ),
            (
                PerformanceInstrument::Perc,
                [
                    ("perc.decay_ms", -1.0),
                    ("perc.decay_ms", 1.0),
                    ("perc.level", -1.0),
                    ("perc.level", 1.0),
                    ("perc.interval_beats", 1.0),
                    ("perc.interval_beats", -1.0),
                ],
            ),
        ];
        for (instrument, targets) in expected {
            for (action, (expected_id, expected_direction)) in actions.into_iter().zip(targets) {
                let (tab, index, spec, direction) =
                    performance_target(instrument, action).expect("closed grammar has a target");
                assert_eq!(spec.id, expected_id);
                assert_eq!(direction, expected_direction);
                assert_eq!(tab, tab_owning_control(spec.id).expect("target has owner"));
                assert_eq!(tab_specs(tab)[index].id, spec.id);

                let mut controls = FluidControls::default();
                (spec.set)(&mut controls, (spec.min + spec.max) / 2.0);
                let before = (spec.get)(&controls);
                spec.apply_delta(direction, &mut controls);
                let after = (spec.get)(&controls);
                let expected_ordering = if direction > 0.0 {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Less
                };
                assert_eq!(
                    after.partial_cmp(&before),
                    Some(expected_ordering),
                    "{instrument:?}/{action:?} moved {spec_id} with {direction}",
                    spec_id = spec.id,
                );
            }
        }
    }
}
