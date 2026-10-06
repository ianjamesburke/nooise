//! Per-layer module slots.
//!
//! A layer carries a fixed number of anonymous slots. Each slot stores *which*
//! module is loaded as a value (`kind`, an index into [`MODULE_CATALOG`]) plus
//! family-shaped params. The module's name is deliberately never part of a
//! control id: ids are permanent entries in the append-only song-code table,
//! so putting the catalog in the id space would make every future module cost
//! another block of them forever.
//!
//! See `docs/proposals/2026-07-30-module-slot-addressing.md`.

use crate::fx::filter::FilterType;

/// Slots per layer. Appending more later is a pure append to the song-id
/// table; removing any is impossible, so this starts deliberately small.
pub(crate) const MODULE_SLOTS: usize = 8;
pub(crate) const MODULE_LAYERS: usize = 9;

/// Where a module runs. Taken from the loaded module, never from the slot
/// index, so all slots stay interchangeable and the UI shows one flat chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Domain {
    /// Intercepts the grid trigger before a voice fires.
    Pre,
    /// Runs on the rendered signal.
    Post,
}

/// Which parameter shape a module projects through the reusable detail drill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Family {
    /// `amount` only.
    SingleAmount,
    /// `amount` plus `time`.
    TwoKnob,
    /// Global timing drift with Amount and a wave length in trigger hits.
    Drunken,
    /// Stereo delay with a persisted clock mode, left/right time, and feedback.
    Delay,
    /// Reverb with size and damping controls behind one Amount row.
    Reverb,
    /// Compressor with threshold, ratio, release, and makeup controls.
    Compression,
    /// Stereo filter with cutoff, resonance, and response type controls.
    Filter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModuleSlotField {
    Kind,
    Amount,
    Time,
    RightTime,
    Clock,
    RightClock,
    Feedback,
    Vintage,
    /// Presence bit for Delay's one bounded wet-only Filter child.
    DelayFilterPresent,
    DelayFilterAmount,
    DelayFilterCutoff,
    DelayFilterResonance,
    DelayFilterType,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EffectParameter {
    pub(crate) field: ModuleSlotField,
    pub(crate) label: &'static str,
    pub(crate) search_name: &'static str,
}

const DELAY_PARAMETERS: &[EffectParameter] = &[
    EffectParameter {
        field: ModuleSlotField::Amount,
        label: "Amount",
        search_name: "Amount",
    },
    EffectParameter {
        field: ModuleSlotField::Time,
        label: "Left Time",
        search_name: "Left Time",
    },
    EffectParameter {
        field: ModuleSlotField::RightTime,
        label: "Right Time",
        search_name: "Right Time",
    },
    EffectParameter {
        field: ModuleSlotField::Feedback,
        label: "Feedback",
        search_name: "Feedback",
    },
    EffectParameter {
        field: ModuleSlotField::Vintage,
        label: "Vintage",
        search_name: "Vintage",
    },
];

const REVERB_PARAMETERS: &[EffectParameter] = &[
    EffectParameter {
        field: ModuleSlotField::Amount,
        label: "Amount",
        search_name: "Amount",
    },
    EffectParameter {
        field: ModuleSlotField::Time,
        label: "Size",
        search_name: "Size",
    },
    EffectParameter {
        field: ModuleSlotField::Feedback,
        label: "Damping",
        search_name: "Damping",
    },
];

const COMPRESSION_PARAMETERS: &[EffectParameter] = &[
    EffectParameter {
        field: ModuleSlotField::Amount,
        label: "Amount",
        search_name: "Amount",
    },
    EffectParameter {
        field: ModuleSlotField::Time,
        label: "Threshold",
        search_name: "Threshold",
    },
    EffectParameter {
        field: ModuleSlotField::RightTime,
        label: "Ratio",
        search_name: "Ratio",
    },
    EffectParameter {
        field: ModuleSlotField::Feedback,
        label: "Release",
        search_name: "Release",
    },
    EffectParameter {
        field: ModuleSlotField::Vintage,
        label: "Makeup",
        search_name: "Makeup",
    },
];

/// Cutoff leads because it is the Filter's ordinary browsing control. A
/// palette-added Filter opens this detail on Amount so its dry mix can rise.
const FILTER_PARAMETERS: &[EffectParameter] = &[
    EffectParameter {
        field: ModuleSlotField::Time,
        label: "Cutoff",
        search_name: "Cutoff",
    },
    EffectParameter {
        field: ModuleSlotField::Amount,
        label: "Amount",
        search_name: "Amount",
    },
    EffectParameter {
        field: ModuleSlotField::RightTime,
        label: "Resonance",
        search_name: "Resonance",
    },
    EffectParameter {
        field: ModuleSlotField::Feedback,
        label: "Type",
        search_name: "Type",
    },
];

/// The persisted child fields exposed to Delay's scoped palette. The Delay
/// page projects either Add Filter or the child's collapsed Cutoff row; its
/// supporting controls live in the explicit Filter drill.
const DELAY_FILTER_PARAMETERS: &[EffectParameter] = &[
    EffectParameter {
        field: ModuleSlotField::DelayFilterPresent,
        label: "Filter",
        search_name: "Add Filter",
    },
    EffectParameter {
        field: ModuleSlotField::DelayFilterCutoff,
        label: "Cutoff",
        search_name: "Filter Cutoff",
    },
    EffectParameter {
        field: ModuleSlotField::DelayFilterAmount,
        label: "Amount",
        search_name: "Filter Amount",
    },
    EffectParameter {
        field: ModuleSlotField::DelayFilterResonance,
        label: "Resonance",
        search_name: "Filter Resonance",
    },
    EffectParameter {
        field: ModuleSlotField::DelayFilterType,
        label: "Type",
        search_name: "Filter Type",
    },
];

pub(crate) fn delay_filter_parameters() -> &'static [EffectParameter] {
    DELAY_FILTER_PARAMETERS
}

pub(crate) fn delay_filter_detail_parameters() -> &'static [EffectParameter] {
    &DELAY_FILTER_PARAMETERS[1..]
}

const SINGLE_AMOUNT_PARAMETERS: &[EffectParameter] = &[EffectParameter {
    field: ModuleSlotField::Amount,
    label: "Amount",
    search_name: "Amount",
}];

const TWO_KNOB_PARAMETERS: &[EffectParameter] = &[
    EffectParameter {
        field: ModuleSlotField::Amount,
        label: "Amount",
        search_name: "Amount",
    },
    EffectParameter {
        field: ModuleSlotField::Time,
        label: "Time",
        search_name: "Time",
    },
];

const DRUNKEN_PARAMETERS: &[EffectParameter] = &[
    EffectParameter {
        field: ModuleSlotField::Amount,
        label: "Amount",
        search_name: "Amount",
    },
    EffectParameter {
        field: ModuleSlotField::Time,
        label: "Pace",
        search_name: "Pace",
    },
];

impl ModuleKind {
    pub(crate) fn parameters(self) -> &'static [EffectParameter] {
        match self.family {
            Family::SingleAmount => SINGLE_AMOUNT_PARAMETERS,
            Family::TwoKnob => TWO_KNOB_PARAMETERS,
            Family::Drunken => DRUNKEN_PARAMETERS,
            Family::Delay => DELAY_PARAMETERS,
            Family::Reverb => REVERB_PARAMETERS,
            Family::Compression => COMPRESSION_PARAMETERS,
            Family::Filter => FILTER_PARAMETERS,
        }
    }

    /// Whether Enter on the collapsed row opens a detail drill: every family
    /// with more than the one knob its collapsed row already shows.
    pub(crate) fn has_detail(self) -> bool {
        self.parameters().len() > 1
    }

    pub(crate) fn collapsed_field(self) -> ModuleSlotField {
        if self.family == Family::Filter {
            ModuleSlotField::Time
        } else {
            ModuleSlotField::Amount
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ModuleKind {
    /// Stable, lowercase, used for fuzzy-find matching.
    pub(crate) id: &'static str,
    pub(crate) display_name: &'static str,
    pub(crate) search_names: &'static [&'static str],
    pub(crate) domain: Domain,
    pub(crate) family: Family,
}

/// The v1 catalog.
///
/// APPEND-ONLY IN EFFECT: a slot stores its module as an index into this
/// array, and those indexes are written into song codes. Reordering or
/// removing an entry silently rewrites what every saved song is using.
/// Appending is always safe.
pub(crate) const MODULE_CATALOG: &[ModuleKind] = &[
    ModuleKind {
        id: "alcohol",
        search_names: &["alcohol"],
        display_name: "Alcohol",
        domain: Domain::Pre,
        family: Family::SingleAmount,
    },
    ModuleKind {
        id: "swing",
        search_names: &["swing"],
        display_name: "Swing",
        domain: Domain::Pre,
        family: Family::SingleAmount,
    },
    ModuleKind {
        id: "drive",
        search_names: &["drive"],
        display_name: "Drive",
        domain: Domain::Post,
        family: Family::SingleAmount,
    },
    ModuleKind {
        id: "room",
        search_names: &["room", "reverb"],
        display_name: "Reverb",
        domain: Domain::Post,
        family: Family::Reverb,
    },
    ModuleKind {
        id: "sidechain",
        search_names: &["sidechain"],
        display_name: "Sidechain",
        domain: Domain::Post,
        family: Family::TwoKnob,
    },
    ModuleKind {
        id: "delay",
        search_names: &["delay"],
        display_name: "Delay",
        domain: Domain::Post,
        family: Family::Delay,
    },
    ModuleKind {
        id: "compression",
        search_names: &["compression"],
        display_name: "Compression",
        domain: Domain::Post,
        family: Family::Compression,
    },
    ModuleKind {
        id: "filter",
        search_names: &["filter"],
        display_name: "Filter",
        domain: Domain::Post,
        family: Family::Filter,
    },
    ModuleKind {
        id: "drunken",
        search_names: &["drunken"],
        display_name: "Drunken",
        domain: Domain::Pre,
        family: Family::Drunken,
    },
];

/// The timing behavior an active delay uses. The stored time values carry
/// only the selected representation; changing modes converts them in place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DelayClock {
    Sync,
    Free,
}

pub(crate) const DELAY_SYNC_MIN_BEATS: f32 = 0.125;
pub(crate) const DELAY_SYNC_MAX_BEATS: f32 = 4.0;
pub(crate) const DELAY_FREE_MIN_MS: f32 = 10.0;
pub(crate) const DELAY_FREE_MAX_MS: f32 = 2_000.0;
const DELAY_SYNC_STEP_BEATS: f32 = 0.125;
const DELAY_FREE_STEP_MS: f32 = 10.0;

/// Change a delay slot's persisted clock mode while preserving the closest
/// audible duration at the current tempo. One active pair is stored; there is
/// deliberately no hidden Sync/Free second state to restore later.
pub(crate) fn switch_delay_clock(slot: &mut ModuleSlot, right: bool, bpm: f32) {
    let clock = if right {
        &mut slot.right_clock
    } else {
        &mut slot.clock
    };
    let time = if right {
        &mut slot.right_time
    } else {
        &mut slot.time
    };
    let from = DelayClock::from_value(*clock);
    let (base_from, base_to, min, max, step, next_clock) = match from {
        DelayClock::Sync => (
            super::TimeBase::Beats,
            super::TimeBase::Ms,
            DELAY_FREE_MIN_MS,
            DELAY_FREE_MAX_MS,
            DELAY_FREE_STEP_MS,
            DelayClock::Free,
        ),
        DelayClock::Free => (
            super::TimeBase::Ms,
            super::TimeBase::Beats,
            DELAY_SYNC_MIN_BEATS,
            DELAY_SYNC_MAX_BEATS,
            DELAY_SYNC_STEP_BEATS,
            DelayClock::Sync,
        ),
    };
    *time = super::snap_step(
        super::convert_time_base(*time, base_from, base_to, bpm),
        step,
    )
    .clamp(min, max);
    *clock = next_clock.value();
}

/// Current milliseconds for the DSP delay line, regardless of the saved
/// control representation.
pub(crate) fn delay_time_ms(value: f32, clock: DelayClock, bpm: f32) -> f32 {
    match clock {
        DelayClock::Sync => super::beats_to_ms(value, bpm),
        DelayClock::Free => value,
    }
}

impl DelayClock {
    pub(crate) const fn value(self) -> f32 {
        match self {
            Self::Sync => 0.0,
            Self::Free => 1.0,
        }
    }

    pub(crate) fn from_value(value: f32) -> Self {
        if value.round() == Self::Free.value() {
            Self::Free
        } else {
            Self::Sync
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Sync => "Sync",
            Self::Free => "Free",
        }
    }
}

/// A Filter's cutoff dial spans the audible band on a Log2 taper.
pub(crate) const FILTER_CUTOFF_MIN_HZ: f32 = 20.0;
pub(crate) const FILTER_CUTOFF_MAX_HZ: f32 = 20_000.0;

/// One optional Filter owned by a Delay slot. This is deliberately not a
/// second module chain: its only scope is the Delay's delayed wet signal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DelayWetFilter {
    pub(crate) amount: f32,
    pub(crate) cutoff: f32,
    pub(crate) resonance: f32,
    pub(crate) filter_type: f32,
}

impl Default for DelayWetFilter {
    fn default() -> Self {
        Self {
            amount: 1.0,
            cutoff: FILTER_CUTOFF_MAX_HZ,
            resonance: 0.0,
            filter_type: 0.0,
        }
    }
}

/// Where Perc, Bass, and Kick's factory Filters sit. Their default sound was
/// voiced there, and a saved song that never touched one carries no cutoff
/// at all, so moving this would re-voice every such song.
const FACTORY_FILTER_CUTOFF_HZ: f32 = 8_000.0;

/// Clap's factory Filter cutoff, fully wet like any Filter. The clap once ran
/// its noise through a one-pole lowpass of its own (`clap.filter`, default
/// 0.7); this is the cutoff of the least-squares fit of the shared Filter to
/// that voice's rendered default clap. The fit wanted a 90% mix to keep the
/// one-pole's gentler skirt; the factory filter starts fully wet instead, so
/// the default clap is darker than the retired one (spectral centroid about
/// 1.3 kHz against 1.7 kHz) and about half a decibel louder.
pub(crate) const CLAP_FACTORY_FILTER_CUTOFF_HZ: f32 = 3_170.0;

/// Change a Filter slot's response type. Swapping Low-pass and High-pass
/// mirrors the cutoff across the dial (`min * max / hz`, the same number of
/// octaves in from the opposite end), so a filter that was nearly
/// transparent stays nearly transparent instead of a high-pass at a high
/// cutoff taking out the whole signal. Band-pass has no transparent end, so
/// a switch to or from it keeps the cutoff.
pub(crate) fn switch_filter_type(slot: &mut ModuleSlot, next: f32) {
    let flips = matches!(
        (
            FilterType::from_value(slot.feedback),
            FilterType::from_value(next)
        ),
        (FilterType::Low, FilterType::High) | (FilterType::High, FilterType::Low)
    );
    if flips {
        slot.time = (FILTER_CUTOFF_MIN_HZ * FILTER_CUTOFF_MAX_HZ / slot.time)
            .clamp(FILTER_CUTOFF_MIN_HZ, FILTER_CUTOFF_MAX_HZ);
    }
    slot.feedback = next;
}

/// `kind` value meaning "no module here". Catalog entry `n` is stored as
/// `n + 1`, so the empty slot is the default and prunes out of song codes.
pub(crate) const MODULE_EMPTY: f32 = 0.0;

/// Highest valid `kind` value. Const so the registry macro can use it as a
/// spec bound instead of restating `MODULE_CATALOG.len()`.
pub(crate) const fn module_kind_max() -> f32 {
    MODULE_CATALOG.len() as f32
}

/// The module a stored `kind` value names, or `None` for an empty slot.
pub(crate) fn module_kind_at(value: f32) -> Option<&'static ModuleKind> {
    let index = value.round();
    if index < 1.0 {
        return None;
    }
    MODULE_CATALOG.get(index as usize - 1)
}

/// Display string for a slot's `kind` row.
pub(crate) fn module_kind_label(value: f32) -> String {
    module_kind_at(value).map_or_else(|| "empty".to_string(), |kind| kind.display_name.to_string())
}

/// Label for a loaded slot's collapsed row on its page. A module with a
/// detail drill carries the same `›` the breadcrumb shows once inside it, so
/// the page says which rows Enter opens.
pub(crate) fn module_row_label(value: f32) -> String {
    match module_kind_at(value) {
        Some(kind) if kind.has_detail() => format!("{} ›", kind.display_name),
        _ => module_kind_label(value),
    }
}

/// One slot's stored state. Defaults to empty and inert, which is what lets a
/// module be added automatically without confirmation: nothing is audible
/// until a knob moves.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct ModuleSlot {
    pub(crate) kind: f32,
    pub(crate) amount: f32,
    pub(crate) time: f32,
    pub(crate) right_time: f32,
    pub(crate) clock: f32,
    pub(crate) right_clock: f32,
    pub(crate) feedback: f32,
    pub(crate) vintage: f32,
    /// Present only for a Delay's bounded wet-only Filter child. `None` keeps
    /// old and plain Delay song-code snapshots byte-identical.
    pub(crate) delay_filter: Option<DelayWetFilter>,
}

impl ModuleSlot {
    pub(crate) fn is_empty(&self) -> bool {
        module_kind_at(self.kind).is_none()
    }

    pub(crate) fn kind(&self) -> Option<&'static ModuleKind> {
        module_kind_at(self.kind)
    }

    pub(crate) fn delay_filter_value(&self, field: ModuleSlotField) -> f32 {
        if !self.kind().is_some_and(|kind| kind.family == Family::Delay) {
            return 0.0;
        }
        let filter = self.delay_filter.unwrap_or_default();
        match field {
            ModuleSlotField::DelayFilterPresent => {
                if self.delay_filter.is_some() {
                    1.0
                } else {
                    0.0
                }
            }
            ModuleSlotField::DelayFilterAmount => filter.amount,
            ModuleSlotField::DelayFilterCutoff => filter.cutoff,
            ModuleSlotField::DelayFilterResonance => filter.resonance,
            ModuleSlotField::DelayFilterType => filter.filter_type,
            _ => 0.0,
        }
    }

    pub(crate) fn delay_filter_default_value(field: ModuleSlotField) -> f32 {
        let filter = DelayWetFilter::default();
        match field {
            ModuleSlotField::DelayFilterAmount => filter.amount,
            ModuleSlotField::DelayFilterCutoff => filter.cutoff,
            ModuleSlotField::DelayFilterResonance => filter.resonance,
            ModuleSlotField::DelayFilterType => filter.filter_type,
            _ => 0.0,
        }
    }

    pub(crate) fn set_delay_filter_value(&mut self, field: ModuleSlotField, value: f32) {
        if !self.kind().is_some_and(|kind| kind.family == Family::Delay) {
            return;
        }
        match field {
            ModuleSlotField::DelayFilterPresent => {
                if value >= 0.5 {
                    self.delay_filter.get_or_insert_default();
                } else {
                    self.delay_filter = None;
                }
            }
            ModuleSlotField::DelayFilterAmount => {
                self.delay_filter.get_or_insert_default().amount = value;
            }
            ModuleSlotField::DelayFilterCutoff => {
                self.delay_filter.get_or_insert_default().cutoff = value;
            }
            ModuleSlotField::DelayFilterResonance => {
                self.delay_filter.get_or_insert_default().resonance = value;
            }
            ModuleSlotField::DelayFilterType => {
                self.delay_filter.get_or_insert_default().filter_type = value;
            }
            _ => {}
        }
    }
}

/// Position of a catalog id in `MODULE_CATALOG`. Panics on an unknown id,
/// which can only be a typo in a compile-time constant.
pub(crate) fn module_catalog_index(id: &str) -> usize {
    MODULE_CATALOG
        .iter()
        .position(|kind| kind.id == id)
        .expect("catalog id must exist")
}

/// Index a catalog id occupies as a stored `kind` value (`MODULE_EMPTY` is
/// 0, so entry `n` is stored as `n + 1`).
pub(crate) fn module_kind_value(id: &str) -> f32 {
    module_catalog_index(id) as f32 + 1.0
}

/// One slot pre-loaded with a module at a factory amount. Used for the
/// effects that shipped as bespoke per-voice sliders before the module chain
/// existed, so the default sound is byte-identical.
pub(crate) fn preset_slot(id: &str, amount: f32) -> ModuleSlot {
    let mut slot = ModuleSlot {
        kind: module_kind_value(id),
        amount,
        clock: DelayClock::Sync.value(),
        right_clock: DelayClock::Sync.value(),
        ..ModuleSlot::default()
    };
    match id {
        "delay" => {
            slot.time = 0.5;
            slot.right_time = 0.75;
            slot.feedback = 0.35;
        }
        "room" => {
            slot.time = 0.72;
            slot.feedback = 0.45;
        }
        "compression" => {
            slot.time = -8.0;
            slot.right_time = 2.0;
            slot.feedback = 100.0;
            slot.vintage = 2.0;
        }
        "filter" => {
            // Keep factory callers' chosen amount. Palette insertion passes
            // zero for exact dry passthrough; preloaded filters remain wet.
            slot.time = FILTER_CUTOFF_MAX_HZ;
            slot.right_time = 0.0;
            slot.feedback = 0.0;
        }
        "drunken" => slot.time = 7.0,
        _ => {}
    }
    slot
}

/// Amount of the first slot holding `id`, or zero when the chain has none.
/// The single read path for an effect that used to be a bespoke field.
pub(crate) fn chain_amount(slots: &[ModuleSlot; MODULE_SLOTS], id: &str) -> f32 {
    chain_amount_slot(slots, id).map_or(0.0, |index| slots[index].amount)
}

/// Every layer's slots. Held on `FluidControls` rather than inside each voice
/// struct so the voices stay unaware of the module chain until execution.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LayerModules {
    pub(crate) pad: [ModuleSlot; MODULE_SLOTS],
    pub(crate) perc: [ModuleSlot; MODULE_SLOTS],
    pub(crate) bass: [ModuleSlot; MODULE_SLOTS],
    pub(crate) kick: [ModuleSlot; MODULE_SLOTS],
    pub(crate) tonal: [ModuleSlot; MODULE_SLOTS],
    pub(crate) clap: [ModuleSlot; MODULE_SLOTS],
    pub(crate) arp: [ModuleSlot; MODULE_SLOTS],
    pub(crate) lead: [ModuleSlot; MODULE_SLOTS],
    pub(crate) master: [ModuleSlot; MODULE_SLOTS],
}

impl Default for LayerModules {
    /// Factory module chains. Every unlisted slot is empty.
    fn default() -> Self {
        let empty = [ModuleSlot::default(); MODULE_SLOTS];
        let with_preset = |id: &str, amount: f32| {
            let mut slots = empty;
            slots[0] = preset_slot(id, amount);
            slots
        };
        let with_factory_filter = || {
            let mut slots = with_preset("filter", 1.0);
            slots[0].time = FACTORY_FILTER_CUTOFF_HZ;
            slots
        };
        Self {
            pad: with_preset("room", 0.4),
            perc: with_factory_filter(),
            bass: {
                let mut slots = with_factory_filter();
                slots[1] = preset_slot("drive", 0.15);
                slots
            },
            kick: {
                let mut slots = with_factory_filter();
                slots[1] = preset_slot("drive", 0.2);
                slots
            },
            tonal: with_preset("room", 0.1),
            clap: {
                let mut slots = with_preset("filter", 1.0);
                slots[0].time = CLAP_FACTORY_FILTER_CUTOFF_HZ;
                slots
            },
            arp: with_preset("room", 0.0),
            // The lead's "slightly distorted" character is the shared Drive
            // module, not a bespoke control, so a player can push it further
            // or take it off like any other effect.
            lead: with_preset("drive", 0.1),
            master: {
                let mut slots = empty;
                slots[0] = preset_slot("drive", 0.05);
                slots[1] = preset_slot("compression", 0.1);
                slots
            },
        }
    }
}

impl LayerModules {
    /// The slots a tab owns, or `None` for a tab with no chain of its own.
    pub(crate) fn for_tab(&self, tab: super::Tab) -> Option<&[ModuleSlot; MODULE_SLOTS]> {
        match tab {
            super::Tab::Chords => Some(&self.pad),
            super::Tab::Perc => Some(&self.perc),
            super::Tab::Bass => Some(&self.bass),
            super::Tab::Kick => Some(&self.kick),
            super::Tab::Tonal => Some(&self.tonal),
            super::Tab::Clap => Some(&self.clap),
            super::Tab::Arp => Some(&self.arp),
            super::Tab::Lead => Some(&self.lead),
            super::Tab::Master => Some(&self.master),
        }
    }

    pub(crate) fn for_tab_mut(
        &mut self,
        tab: super::Tab,
    ) -> Option<&mut [ModuleSlot; MODULE_SLOTS]> {
        match tab {
            super::Tab::Chords => Some(&mut self.pad),
            super::Tab::Perc => Some(&mut self.perc),
            super::Tab::Bass => Some(&mut self.bass),
            super::Tab::Kick => Some(&mut self.kick),
            super::Tab::Tonal => Some(&mut self.tonal),
            super::Tab::Clap => Some(&mut self.clap),
            super::Tab::Arp => Some(&mut self.arp),
            super::Tab::Lead => Some(&mut self.lead),
            super::Tab::Master => Some(&mut self.master),
        }
    }
}

/// Whether this tab owns a module chain at all. Pure: the palette's entry
/// list depends on it and must not read live controls.
pub(crate) fn tab_has_module_chain(_tab: super::Tab) -> bool {
    true
}

/// Whether a catalog entry has a real processor on this layer. Alcohol and
/// Sidechain retain their saved catalog indexes but stay out of the add
/// palette until they have DSP; an addable row must never be inert.
pub(crate) fn module_available_on(kind: ModuleKind, tab: super::Tab) -> bool {
    match kind.id {
        "swing" => tab_has_module_chain(tab),
        "drunken" => tab_has_module_chain(tab),
        "drive" | "room" | "delay" | "compression" | "filter" => tab_has_module_chain(tab),
        _ => false,
    }
}

pub(crate) fn module_layer_index(tab: super::Tab) -> Option<usize> {
    match tab {
        super::Tab::Chords => Some(0),
        super::Tab::Perc => Some(1),
        super::Tab::Bass => Some(2),
        super::Tab::Kick => Some(3),
        super::Tab::Tonal => Some(4),
        super::Tab::Clap => Some(5),
        super::Tab::Arp => Some(6),
        super::Tab::Lead => Some(7),
        super::Tab::Master => Some(8),
    }
}

/// Index of the first slot holding `id`.
pub(crate) fn chain_amount_slot(slots: &[ModuleSlot; MODULE_SLOTS], id: &str) -> Option<usize> {
    slots
        .iter()
        .position(|slot| slot.kind().is_some_and(|kind| kind.id == id))
}

/// The authored Drunken values loaded on one layer, if any. The engine owns
/// the choice between these local values and the Master values, then copies
/// that result into the layer's `TimingContext` before its pre-trigger grid
/// runs.
pub(crate) fn local_drunken(slots: &[ModuleSlot; MODULE_SLOTS]) -> Option<(f32, f32)> {
    chain_amount_slot(slots, "drunken").map(|index| (slots[index].amount, slots[index].time))
}

/// Master Drunken fills a layer only while the layer has no Drunken slot of
/// its own. A present local slot wins even when its Amount is zero.
pub(crate) fn drunken_for(
    slots: &[ModuleSlot; MODULE_SLOTS],
    master_amount: f32,
    master_pace: f32,
) -> (f32, f32) {
    local_drunken(slots).unwrap_or((master_amount, master_pace))
}

/// Resolve pre-synthesis module values into the grid fields the voices read.
/// Post-synthesis effects execute directly through `ModuleFxBank` instead of
/// being copied back into bespoke voice controls.
pub(crate) fn resolve_module_chain(c: &mut super::FluidControls) {
    c.master.swing = chain_amount(&c.modules.master, "swing");
    c.master.drunken_amount = chain_amount(&c.modules.master, "drunken");
    c.master.drunken_pace = chain_amount_slot(&c.modules.master, "drunken")
        .map_or(7.0, |index| c.modules.master[index].time);
    let swing_for = |slots: &[ModuleSlot; MODULE_SLOTS]| {
        chain_amount_slot(slots, "swing").map_or(c.master.swing, |index| slots[index].amount)
    };
    c.pad.swing = swing_for(&c.modules.pad);
    c.perc.swing = swing_for(&c.modules.perc);
    // Master Swing leaves the kick foundation straight. A local Kick Swing
    // remains an explicit choice, including when its amount is zero.
    c.kick.swing = chain_amount_slot(&c.modules.kick, "swing")
        .map_or(0.0, |index| c.modules.kick[index].amount);
    c.tonal.swing = swing_for(&c.modules.tonal);
    c.arp.swing = swing_for(&c.modules.arp);
    c.lead.swing = swing_for(&c.modules.lead);
    c.clap.swing = swing_for(&c.modules.clap);
    c.bass.swing = swing_for(&c.modules.bass);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_default_slot_is_empty_and_therefore_inert() {
        let slot = ModuleSlot::default();
        assert!(slot.is_empty());
        assert_eq!(slot.kind, MODULE_EMPTY);
        assert_eq!(module_kind_label(slot.kind), "empty");
    }

    #[test]
    fn delay_filter_values_materialize_only_on_a_delay_parent() {
        let mut slot = preset_slot("delay", 0.5);
        slot.set_delay_filter_value(ModuleSlotField::DelayFilterCutoff, 1_200.0);
        assert_eq!(slot.delay_filter.unwrap().cutoff, 1_200.0);

        let mut not_delay = ModuleSlot::default();
        not_delay.set_delay_filter_value(ModuleSlotField::DelayFilterCutoff, 1_200.0);
        assert!(not_delay.delay_filter.is_none());
    }

    #[test]
    fn catalog_indexes_are_one_based_so_zero_stays_empty() {
        assert!(module_kind_at(0.0).is_none());
        assert_eq!(module_kind_at(1.0).unwrap().id, "alcohol");
        assert_eq!(
            module_kind_at(module_kind_max()).unwrap().id,
            MODULE_CATALOG.last().unwrap().id
        );
        // Past the end is empty, not a panic: a song code from a later
        // version naming an unknown module must degrade, not crash.
        assert!(module_kind_at(module_kind_max() + 1.0).is_none());
    }

    #[test]
    fn switching_delay_clock_converts_only_the_selected_channel() {
        let mut slot = ModuleSlot {
            time: 0.5,
            right_time: 0.75,
            ..ModuleSlot::default()
        };
        switch_delay_clock(&mut slot, false, 120.0);
        assert_eq!(DelayClock::from_value(slot.clock), DelayClock::Free);
        assert_eq!((slot.time, slot.right_time), (250.0, 0.75));

        switch_delay_clock(&mut slot, true, 120.0);
        assert_eq!(DelayClock::from_value(slot.right_clock), DelayClock::Free);
        assert_eq!((slot.time, slot.right_time), (250.0, 380.0));

        switch_delay_clock(&mut slot, false, 120.0);
        assert_eq!(DelayClock::from_value(slot.clock), DelayClock::Sync);
        assert_eq!((slot.time, slot.right_time), (0.5, 380.0));
    }

    #[test]
    fn every_addable_module_has_a_complete_layer_execution_contract() {
        for tab in super::super::Tab::all() {
            for kind in MODULE_CATALOG {
                let expected = match kind.id {
                    "drive" | "room" | "delay" | "compression" | "filter" => {
                        tab_has_module_chain(tab)
                    }
                    "swing" => tab_has_module_chain(tab),
                    "drunken" => tab_has_module_chain(tab),
                    "alcohol" | "sidechain" => false,
                    other => panic!("catalog entry {other} needs an availability contract"),
                };
                assert_eq!(
                    module_available_on(*kind, tab),
                    expected,
                    "{} on {}",
                    kind.id,
                    tab.name()
                );
            }
        }
    }

    #[test]
    fn master_swing_fills_only_layers_without_their_own_swing() {
        let mut controls = super::super::FluidControls::default();
        controls.modules.master[2] = preset_slot("swing", 0.6);
        controls.modules.master[3] = preset_slot("drunken", 0.5);
        controls.modules.kick[3] = preset_slot("swing", 0.2);
        controls.modules.perc[1] = preset_slot("swing", 0.0);
        resolve_module_chain(&mut controls);
        assert_eq!(controls.master.swing, 0.6);
        assert_eq!(controls.master.drunken_amount, 0.5);
        assert_eq!(controls.master.drunken_pace, 7.0);
        assert_eq!(controls.pad.swing, 0.6);
        assert_eq!(controls.kick.swing, 0.2);
        assert_eq!(controls.perc.swing, 0.0);
        controls.modules.kick[3] = ModuleSlot::default();
        resolve_module_chain(&mut controls);
        assert_eq!(controls.kick.swing, 0.0);
    }

    #[test]
    fn local_drunken_exposes_its_own_amount_and_pace() {
        let mut slots = [ModuleSlot::default(); MODULE_SLOTS];
        assert_eq!(local_drunken(&slots), None);

        slots[4] = preset_slot("drunken", 0.6);
        slots[4].time = 11.0;
        assert_eq!(local_drunken(&slots), Some((0.6, 11.0)));
    }

    #[test]
    fn local_drunken_overrides_master_including_at_zero_amount() {
        let mut slots = [ModuleSlot::default(); MODULE_SLOTS];
        assert_eq!(drunken_for(&slots, 0.8, 10.0), (0.8, 10.0));

        slots[4] = preset_slot("drunken", 0.0);
        slots[4].time = 5.0;
        assert_eq!(drunken_for(&slots, 0.8, 10.0), (0.0, 5.0));
    }

    /// The resolved values must reach the same trigger scheduler the voices
    /// call, for both offset kicks and faster kick grids.
    #[test]
    fn master_swing_keeps_kick_hits_straight_but_swings_other_voices() {
        use super::super::{GridTrigger, TimingContext};

        fn hits(interval: f32, offset: f32, swing: f32, count: usize) -> Vec<f64> {
            let mut trigger = GridTrigger::new();
            let mut hits = Vec::new();
            for tick in 0..20_000 {
                let beat = tick as f64 / 1_000.0;
                if trigger.pop_swung(
                    TimingContext::new(2_000.0, 120.0, beat),
                    interval,
                    offset,
                    swing,
                ) {
                    hits.push(beat);
                    if hits.len() == count {
                        break;
                    }
                }
            }
            assert_eq!(hits.len(), count);
            hits
        }

        for global in [0.0, 1.0] {
            let mut controls = super::super::FluidControls::default();
            controls.modules.master[2] = preset_slot("swing", global);
            resolve_module_chain(&mut controls);
            assert_eq!(controls.kick.swing, 0.0);
            assert_eq!(controls.perc.swing, global);

            for (interval, offset) in [(0.125, 0.0), (0.5, 0.125), (1.0, 0.0), (2.0, 0.25)] {
                let actual = hits(interval, offset, controls.kick.swing, 6);
                let expected: Vec<_> = (0..6)
                    .map(|slot| f64::from(offset) + slot as f64 * f64::from(interval))
                    .collect();
                assert_eq!(
                    actual, expected,
                    "Kick interval={interval} offset={offset} global={global}"
                );
            }
            let expected_perc = if global == 0.0 {
                vec![0.0, 0.25, 0.5, 0.75]
            } else {
                vec![0.0, 0.375, 0.5, 0.875]
            };
            assert_eq!(hits(0.25, 0.0, controls.perc.swing, 4), expected_perc);
        }

        let mut controls = super::super::FluidControls::default();
        controls.modules.master[2] = preset_slot("swing", 1.0);
        controls.modules.kick[3] = preset_slot("swing", 0.0);
        resolve_module_chain(&mut controls);
        assert_eq!(
            hits(0.5, 0.0, controls.kick.swing, 4),
            vec![0.0, 0.5, 1.0, 1.5]
        );
        controls.modules.kick[3].amount = 1.0;
        resolve_module_chain(&mut controls);
        assert_eq!(
            hits(0.5, 0.0, controls.kick.swing, 4),
            vec![0.0, 0.75, 1.0, 1.75]
        );
        controls.modules.master[2].amount = 0.0;
        resolve_module_chain(&mut controls);
        assert_eq!(
            hits(0.5, 0.0, controls.kick.swing, 4),
            vec![0.0, 0.75, 1.0, 1.75]
        );
    }

    /// A detail drill opens on the knob its collapsed row showed, so the
    /// page and the drill agree on what the module's main control is.
    #[test]
    fn every_detail_drill_leads_with_its_collapsed_row() {
        for kind in MODULE_CATALOG {
            assert_eq!(
                kind.parameters()[0].field,
                kind.collapsed_field(),
                "{} detail must lead with its collapsed row",
                kind.id
            );
        }
    }

    #[test]
    fn clap_factory_filter_is_a_fully_wet_low_pass_at_its_fitted_cutoff() {
        let slot = LayerModules::default().clap[0];
        assert_eq!(slot.kind().map(|kind| kind.id), Some("filter"));
        assert_eq!(slot.amount, 1.0);
        assert_eq!(slot.time, CLAP_FACTORY_FILTER_CUTOFF_HZ);
        assert!(matches!(
            FilterType::from_value(slot.feedback),
            FilterType::Low
        ));
        assert_eq!(slot.right_time, 0.0);
    }

    #[test]
    fn an_added_filter_starts_at_the_top_of_its_dial() {
        assert_eq!(preset_slot("filter", 0.0).time, FILTER_CUTOFF_MAX_HZ);
        assert_eq!(preset_slot("filter", 0.0).amount, 0.0);
    }

    #[test]
    fn swapping_low_and_high_pass_mirrors_the_cutoff_across_the_dial() {
        let (low, high, band) = (0.0, 1.0, 2.0);
        let mut slot = preset_slot("filter", 0.0);
        switch_filter_type(&mut slot, high);
        assert_eq!(slot.time, FILTER_CUTOFF_MIN_HZ);
        switch_filter_type(&mut slot, low);
        assert_eq!(slot.time, FILTER_CUTOFF_MAX_HZ);

        // 1 kHz is as many octaves above the floor as 400 Hz is below the top.
        slot.time = 1_000.0;
        switch_filter_type(&mut slot, high);
        assert!((slot.time - 400.0).abs() < 0.01, "{}", slot.time);

        // Band-pass has no transparent end to mirror towards.
        switch_filter_type(&mut slot, band);
        assert!((slot.time - 400.0).abs() < 0.01, "{}", slot.time);
        switch_filter_type(&mut slot, low);
        assert!((slot.time - 400.0).abs() < 0.01, "{}", slot.time);
        assert_eq!(slot.feedback, low);
    }

    #[test]
    fn catalog_ids_are_unique_and_lowercase() {
        for (i, kind) in MODULE_CATALOG.iter().enumerate() {
            assert_eq!(
                kind.id,
                kind.id.to_lowercase(),
                "{} must be lowercase",
                kind.id
            );
            assert!(
                !MODULE_CATALOG[..i].iter().any(|other| other.id == kind.id),
                "duplicate catalog id {}",
                kind.id
            );
        }
    }

    #[test]
    fn only_two_knob_modules_use_the_time_param() {
        assert!(matches!(Family::SingleAmount, Family::SingleAmount));
        assert!(matches!(Family::TwoKnob, Family::TwoKnob));
    }
}
