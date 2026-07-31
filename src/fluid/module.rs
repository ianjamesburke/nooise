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

/// Where a module runs. Taken from the loaded module, never from the slot
/// index, so all slots stay interchangeable and the UI shows one flat chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Domain {
    /// Intercepts the grid trigger before a voice fires.
    Pre,
    /// Runs on the rendered signal.
    Post,
}

/// Which params a module actually uses. Keeping this closed at two is what
/// keeps the per-slot id count fixed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Family {
    /// `amount` only.
    SingleAmount,
    /// `amount` plus `time`.
    TwoKnob,
}

impl Family {
    pub(crate) fn uses_time(self) -> bool {
        matches!(self, Self::TwoKnob)
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
        id: "sidechain",
        display_name: "Sidechain",
        domain: Domain::Post,
        family: Family::TwoKnob,
    },
];

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
}

impl ModuleSlot {
    pub(crate) fn is_empty(&self) -> bool {
        module_kind_at(self.kind).is_none()
    }

    pub(crate) fn kind(&self) -> Option<&'static ModuleKind> {
        module_kind_at(self.kind)
    }
}

/// Every layer's slots. Held on `FluidControls` rather than inside each voice
/// struct so the voices stay unaware of the module chain until execution.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct LayerModules {
    pub pad: [ModuleSlot; MODULE_SLOTS],
    pub perc: [ModuleSlot; MODULE_SLOTS],
    pub bass: [ModuleSlot; MODULE_SLOTS],
    pub kick: [ModuleSlot; MODULE_SLOTS],
    pub tonal: [ModuleSlot; MODULE_SLOTS],
    pub clap: [ModuleSlot; MODULE_SLOTS],
    pub arp: [ModuleSlot; MODULE_SLOTS],
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
        assert!(!Family::SingleAmount.uses_time());
        assert!(Family::TwoKnob.uses_time());
    }
}
