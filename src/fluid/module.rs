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

/// Slots per layer. Appending more later is a pure append to the song-id
/// table; removing any is impossible, so this starts deliberately small.
pub(crate) const MODULE_SLOTS: usize = 8;
pub(crate) const MODULE_LAYERS: usize = 8;

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
    /// Stereo delay with a persisted clock mode, left/right time, and feedback.
    Delay,
    /// Reverb with size and damping controls behind one Amount row.
    Reverb,
    /// Compressor with threshold, ratio, release, and makeup controls.
    Compression,
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EffectParameter {
    pub(crate) field: ModuleSlotField,
    pub(crate) label: &'static str,
}

const DELAY_PARAMETERS: &[EffectParameter] = &[
    EffectParameter {
        field: ModuleSlotField::Amount,
        label: "Amount",
    },
    EffectParameter {
        field: ModuleSlotField::Time,
        label: "Left Time",
    },
    EffectParameter {
        field: ModuleSlotField::RightTime,
        label: "Right Time",
    },
    EffectParameter {
        field: ModuleSlotField::Feedback,
        label: "Feedback",
    },
    EffectParameter {
        field: ModuleSlotField::Vintage,
        label: "Vintage",
    },
];

const REVERB_PARAMETERS: &[EffectParameter] = &[
    EffectParameter {
        field: ModuleSlotField::Amount,
        label: "Amount",
    },
    EffectParameter {
        field: ModuleSlotField::Time,
        label: "Size",
    },
    EffectParameter {
        field: ModuleSlotField::Feedback,
        label: "Damping",
    },
];

const COMPRESSION_PARAMETERS: &[EffectParameter] = &[
    EffectParameter {
        field: ModuleSlotField::Amount,
        label: "Amount",
    },
    EffectParameter {
        field: ModuleSlotField::Time,
        label: "Threshold",
    },
    EffectParameter {
        field: ModuleSlotField::RightTime,
        label: "Ratio",
    },
    EffectParameter {
        field: ModuleSlotField::Feedback,
        label: "Release",
    },
    EffectParameter {
        field: ModuleSlotField::Vintage,
        label: "Makeup",
    },
];

const SINGLE_AMOUNT_PARAMETERS: &[EffectParameter] = &[EffectParameter {
    field: ModuleSlotField::Amount,
    label: "Amount",
}];

const TWO_KNOB_PARAMETERS: &[EffectParameter] = &[
    EffectParameter {
        field: ModuleSlotField::Amount,
        label: "Amount",
    },
    EffectParameter {
        field: ModuleSlotField::Time,
        label: "Time",
    },
];

impl ModuleKind {
    pub(crate) fn parameters(self) -> &'static [EffectParameter] {
        match self.family {
            Family::SingleAmount => SINGLE_AMOUNT_PARAMETERS,
            Family::TwoKnob => TWO_KNOB_PARAMETERS,
            Family::Delay => DELAY_PARAMETERS,
            Family::Reverb => REVERB_PARAMETERS,
            Family::Compression => COMPRESSION_PARAMETERS,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ModuleKind {
    /// Stable, lowercase, used for fuzzy-find matching.
    pub(crate) id: &'static str,
    pub(crate) display_name: &'static str,
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
        display_name: "Alcohol",
        domain: Domain::Pre,
        family: Family::SingleAmount,
    },
    ModuleKind {
        id: "swing",
        display_name: "Swing",
        domain: Domain::Pre,
        family: Family::SingleAmount,
    },
    ModuleKind {
        id: "drive",
        display_name: "Drive",
        domain: Domain::Post,
        family: Family::SingleAmount,
    },
    ModuleKind {
        id: "room",
        display_name: "Reverb",
        domain: Domain::Post,
        family: Family::Reverb,
    },
    ModuleKind {
        id: "sidechain",
        display_name: "Sidechain",
        domain: Domain::Post,
        family: Family::TwoKnob,
    },
    ModuleKind {
        id: "delay",
        display_name: "Delay",
        domain: Domain::Post,
        family: Family::Delay,
    },
    ModuleKind {
        id: "compression",
        display_name: "Compression",
        domain: Domain::Post,
        family: Family::Compression,
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

/// One slot's stored state. Defaults to empty and inert, which is what lets a
/// module be added automatically without confirmation: nothing is audible
/// until a knob moves.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct ModuleSlot {
    pub kind: f32,
    pub amount: f32,
    pub time: f32,
    pub right_time: f32,
    pub clock: f32,
    pub right_clock: f32,
    pub feedback: f32,
    pub vintage: f32,
}

impl ModuleSlot {
    pub(crate) fn is_empty(&self) -> bool {
        module_kind_at(self.kind).is_none()
    }

    pub(crate) fn kind(&self) -> Option<&'static ModuleKind> {
        module_kind_at(self.kind)
    }
}

/// Index a catalog id occupies as a stored `kind` value. Panics on an unknown
/// id, which can only be a typo in a compile-time constant.
pub(crate) fn module_kind_value(id: &str) -> f32 {
    MODULE_CATALOG
        .iter()
        .position(|kind| kind.id == id)
        .map(|index| index as f32 + 1.0)
        .expect("catalog id must exist")
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
    pub pad: [ModuleSlot; MODULE_SLOTS],
    pub perc: [ModuleSlot; MODULE_SLOTS],
    pub bass: [ModuleSlot; MODULE_SLOTS],
    pub kick: [ModuleSlot; MODULE_SLOTS],
    pub tonal: [ModuleSlot; MODULE_SLOTS],
    pub clap: [ModuleSlot; MODULE_SLOTS],
    pub arp: [ModuleSlot; MODULE_SLOTS],
    pub master: [ModuleSlot; MODULE_SLOTS],
}

impl Default for LayerModules {
    /// Slot 1 of each layer carries whatever that voice shipped as a bespoke
    /// effect slider, at its old default, so folding those controls into the
    /// chain changed no sound. Every other slot is empty.
    fn default() -> Self {
        let empty = [ModuleSlot::default(); MODULE_SLOTS];
        let with_preset = |id: &str, amount: f32| {
            let mut slots = empty;
            slots[0] = preset_slot(id, amount);
            slots
        };
        Self {
            pad: with_preset("room", 0.8),
            perc: empty,
            bass: with_preset("drive", 0.15),
            kick: with_preset("drive", 0.2),
            tonal: with_preset("room", 0.6),
            clap: with_preset("room", 0.0),
            arp: with_preset("room", 0.0),
            master: {
                let mut slots = empty;
                slots[0] = preset_slot("drive", 0.1);
                slots[1] = preset_slot("compression", 0.2);
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
            super::Tab::Master => Some(&self.master),
            super::Tab::Macros => None,
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
            super::Tab::Master => Some(&mut self.master),
            super::Tab::Macros => None,
        }
    }
}

/// Whether this tab owns a module chain at all. Pure: the palette's entry
/// list depends on it and must not read live controls.
pub(crate) fn tab_has_module_chain(tab: super::Tab) -> bool {
    tab != super::Tab::Macros
}

/// Whether a catalog entry has a real processor on this layer. Alcohol and
/// Sidechain retain their saved catalog indexes but stay out of the add
/// palette until they have DSP; an addable row must never be inert.
pub(crate) fn module_available_on(kind: ModuleKind, tab: super::Tab) -> bool {
    match kind.id {
        "swing" => !matches!(
            tab,
            super::Tab::Chords | super::Tab::Macros | super::Tab::Master
        ),
        "drive" | "room" | "delay" | "compression" => tab_has_module_chain(tab),
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
        super::Tab::Master => Some(7),
        super::Tab::Macros => None,
    }
}

/// Index of the first slot holding `id`.
pub(crate) fn chain_amount_slot(slots: &[ModuleSlot; MODULE_SLOTS], id: &str) -> Option<usize> {
    slots
        .iter()
        .position(|slot| slot.kind().is_some_and(|kind| kind.id == id))
}

/// Resolve pre-synthesis module values into the grid fields the voices read.
/// Post-synthesis effects execute directly through `ModuleFxBank` instead of
/// being copied back into bespoke voice controls.
pub(crate) fn resolve_module_chain(c: &mut super::FluidControls) {
    c.perc.swing = chain_amount(&c.modules.perc, "swing");
    c.kick.swing = chain_amount(&c.modules.kick, "swing");
    c.tonal.swing = chain_amount(&c.modules.tonal, "swing");
    c.arp.swing = chain_amount(&c.modules.arp, "swing");
    c.clap.swing = chain_amount(&c.modules.clap, "swing");
    c.bass.swing = chain_amount(&c.modules.bass, "swing");
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
                    "drive" | "room" | "delay" | "compression" => tab_has_module_chain(tab),
                    "swing" => matches!(
                        tab,
                        super::super::Tab::Perc
                            | super::super::Tab::Bass
                            | super::super::Tab::Kick
                            | super::super::Tab::Tonal
                            | super::super::Tab::Clap
                            | super::super::Tab::Arp
                    ),
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
