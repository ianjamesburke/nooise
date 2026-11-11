//! Typed, static musical and application operations projected into control surfaces.

use super::*;

/// A named action, distinct from an editable musical control.
///
/// Operations are a closed vocabulary. Their palette metadata and typed
/// execution payload stay together here; no surface dispatches a string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Operation {
    Motion(MotionAction),
    Mix(mix_action::MixAction),
    Recipe(recipe::RecipeId),
    PlannedMute,
    OutputDevice,
    Chord(ChordAction),
    ToggleAuto,
    ToggleVamp,
    ToggleKickBass,
    ToggleBeatRepeat,
    ChordNames(ChordNames),
    ClearSequence,
    Section(SectionAction),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SectionAction {
    /// Keep the current song in slot `0..SECTION_COUNT`, overwriting it.
    Store(usize),
    Recall(usize),
    Next,
    Previous,
    Cancel,
}

const STORE_LABELS: [&str; sections::SECTION_COUNT] = [
    "Store section 1",
    "Store section 2",
    "Store section 3",
    "Store section 4",
    "Store section 5",
    "Store section 6",
    "Store section 7",
    "Store section 8",
    "Store section 9",
];
const QUEUE_LABELS: [&str; sections::SECTION_COUNT] = [
    "Queue section 1",
    "Queue section 2",
    "Queue section 3",
    "Queue section 4",
    "Queue section 5",
    "Queue section 6",
    "Queue section 7",
    "Queue section 8",
    "Queue section 9",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChordAction {
    AddExtension,
    Restore,
    Copy,
    Paste,
    PasteAll,
}

/// A palette action keeps its original preset and slot, and refuses a slot
/// that changed while the palette was open (including an auto transition).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ChordTarget {
    pub(crate) progression: i8,
    pub(crate) slot: usize,
    fields: [u32; 9],
}

impl ChordTarget {
    pub(crate) fn capture(id: &str, controls: &FluidControls) -> Option<Self> {
        let (slot, _) = parse_chord_slot_id(id)?;
        let progression = progression_index(controls.pad.progression);
        Some(Self {
            progression: PROGRESSION_SONG_VALUES[progression],
            slot,
            fields: controls
                .pad
                .chord_slot(progression, slot)
                .values()
                .map(f32::to_bits),
        })
    }

    pub(crate) fn resolve(self, controls: &FluidControls) -> Option<usize> {
        let progression = progression_index(controls.pad.progression);
        (PROGRESSION_SONG_VALUES[progression] == self.progression
            && self.slot < CHORD_SLOT_COUNT
            && controls
                .pad
                .chord_slot(progression, self.slot)
                .values()
                .map(f32::to_bits)
                == self.fields)
            .then_some(progression)
    }
}

/// The display metadata belonging to one [`Operation`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OperationSpec {
    pub(crate) operation: Operation,
    pub(crate) label: &'static str,
    pub(crate) aliases: &'static [&'static str],
    pub(crate) description: &'static str,
}

impl Operation {
    pub(crate) const ALL: [Self; 51] = [
        Self::Motion(MotionAction::Grab(MotionDuration::Beats4)),
        Self::Motion(MotionAction::Grab(MotionDuration::Beats8)),
        Self::Motion(MotionAction::Grab(MotionDuration::Beats16)),
        Self::Motion(MotionAction::Grab(MotionDuration::Beats32)),
        Self::Motion(MotionAction::Bypass),
        Self::Motion(MotionAction::Resume),
        Self::Motion(MotionAction::Delete),
        Self::Mix(mix_action::MixAction::KickOnly),
        Self::Mix(mix_action::MixAction::MuteKick),
        Self::Recipe(recipe::RecipeId::Sway),
        Self::Recipe(recipe::RecipeId::Tremolo),
        Self::Recipe(recipe::RecipeId::Sidechain),
        Self::Recipe(recipe::RecipeId::Pulse),
        Self::Recipe(recipe::RecipeId::Drift),
        Self::Recipe(recipe::RecipeId::Rise),
        Self::Recipe(recipe::RecipeId::Rhythm),
        Self::PlannedMute,
        Self::OutputDevice,
        Self::Chord(ChordAction::AddExtension),
        Self::Chord(ChordAction::Restore),
        Self::Chord(ChordAction::Copy),
        Self::Chord(ChordAction::Paste),
        Self::Chord(ChordAction::PasteAll),
        Self::ToggleAuto,
        Self::ToggleVamp,
        Self::ToggleKickBass,
        Self::ToggleBeatRepeat,
        Self::ChordNames(ChordNames::Roman),
        Self::ChordNames(ChordNames::Letters),
        Self::ClearSequence,
        Self::Section(SectionAction::Store(0)),
        Self::Section(SectionAction::Store(1)),
        Self::Section(SectionAction::Store(2)),
        Self::Section(SectionAction::Store(3)),
        Self::Section(SectionAction::Store(4)),
        Self::Section(SectionAction::Store(5)),
        Self::Section(SectionAction::Store(6)),
        Self::Section(SectionAction::Store(7)),
        Self::Section(SectionAction::Store(8)),
        Self::Section(SectionAction::Recall(0)),
        Self::Section(SectionAction::Recall(1)),
        Self::Section(SectionAction::Recall(2)),
        Self::Section(SectionAction::Recall(3)),
        Self::Section(SectionAction::Recall(4)),
        Self::Section(SectionAction::Recall(5)),
        Self::Section(SectionAction::Recall(6)),
        Self::Section(SectionAction::Recall(7)),
        Self::Section(SectionAction::Recall(8)),
        Self::Section(SectionAction::Next),
        Self::Section(SectionAction::Previous),
        Self::Section(SectionAction::Cancel),
    ];

    pub(crate) fn spec(self) -> OperationSpec {
        match self {
            Self::Motion(action) => OperationSpec {
                operation: self,
                label: action.name(),
                aliases: match action {
                    MotionAction::Grab(MotionDuration::Beats4) => {
                        &["grab 1 bar", "motion grab 1 bar"]
                    }
                    MotionAction::Grab(MotionDuration::Beats8) => {
                        &["grab 2 bars", "motion grab 2 bars"]
                    }
                    MotionAction::Grab(MotionDuration::Beats16) => {
                        &["grab 4 bars", "motion grab 4 bars"]
                    }
                    MotionAction::Grab(MotionDuration::Beats32) => {
                        &["grab 8 bars", "motion grab 8 bars"]
                    }
                    _ => &[],
                },
                description: action.description(),
            },
            Self::Mix(action) => OperationSpec {
                operation: self,
                label: action.name(),
                aliases: action.aliases(),
                description: action.description(),
            },
            Self::Recipe(id) => {
                let recipe = id.recipe();
                OperationSpec {
                    operation: self,
                    label: recipe.name,
                    aliases: recipe.aliases,
                    description: recipe.description,
                }
            }
            Self::Chord(action) => OperationSpec {
                operation: self,
                label: match action {
                    ChordAction::AddExtension => "Add extension",
                    ChordAction::Restore => "Restore chord",
                    ChordAction::Copy => "Copy chord",
                    ChordAction::Paste => "Paste chord",
                    ChordAction::PasteAll => "Paste chord to all steps",
                },
                aliases: match action {
                    ChordAction::Copy => &["copy step", "copy this chord"],
                    ChordAction::Paste => &["paste step", "paste this chord"],
                    ChordAction::PasteAll => &["paste all", "fill with chord"],
                    ChordAction::AddExtension | ChordAction::Restore => &[],
                },
                description: match action {
                    ChordAction::AddExtension => "open the chord's second extension",
                    ChordAction::Restore => "restore this chord's original voicing",
                    ChordAction::Copy => "keep this chord for pasting onto other steps",
                    ChordAction::Paste => "write the copied chord onto this step",
                    ChordAction::PasteAll => "write the copied chord onto every step of the phrase",
                },
            },
            Self::ChordNames(ChordNames::Roman) => OperationSpec {
                operation: self,
                label: "Roman chords",
                aliases: &["roman", "numerals", "roman numerals"],
                description: "write chords as Roman numerals in the home key",
            },
            Self::ChordNames(ChordNames::Letters) => OperationSpec {
                operation: self,
                label: "Letter chords",
                aliases: &["letters", "chord letters", "chord names"],
                description: "write chords as letter names",
            },
            Self::OutputDevice => OperationSpec {
                operation: self,
                label: "Output device",
                aliases: &["output", "device", "audio output", "routing"],
                description: "choose where live audio plays",
            },
            Self::PlannedMute => OperationSpec {
                operation: self,
                label: "Mute next bar",
                aliases: &["planned mute", "mute next"],
                description: "mute the visible layer at the next bar",
            },
            Self::ToggleAuto => OperationSpec {
                operation: self,
                label: "Toggle auto",
                aliases: &["auto", "auto morph", "morph"],
                description: "start or stop auto-morphing between songs",
            },
            Self::ToggleVamp => OperationSpec {
                operation: self,
                label: "Toggle vamp",
                aliases: &["vamp", "chord vamp"],
                description: "vamp the current chord or cue release",
            },
            Self::ToggleKickBass => OperationSpec {
                operation: self,
                label: "Toggle group mute",
                aliases: &["group mute", "q mute"],
                description: "toggle the Kick and Bass group gate",
            },
            Self::ToggleBeatRepeat => OperationSpec {
                operation: self,
                label: "Beat repeat",
                aliases: &["repeat", "beat repeat", "stutter"],
                description: "loop the current beat of live audio; Escape releases",
            },
            Self::Section(action) => OperationSpec {
                operation: self,
                label: match action {
                    SectionAction::Store(index) => STORE_LABELS[index],
                    SectionAction::Recall(index) => QUEUE_LABELS[index],
                    SectionAction::Next => "Queue next section",
                    SectionAction::Previous => "Queue previous section",
                    SectionAction::Cancel => "Cancel section queue",
                },
                aliases: &[],
                description: match action {
                    SectionAction::Store(_) => "save current song, replacing any stored one",
                    SectionAction::Recall(_) | SectionAction::Next | SectionAction::Previous => {
                        "change when the phrase ends"
                    }
                    SectionAction::Cancel => "keep playing current section",
                },
            },
            Self::ClearSequence => OperationSpec {
                operation: self,
                label: "Clear sequence",
                aliases: &[
                    "clear steps",
                    "clear pattern",
                    "clear phrase",
                    "clear progression",
                    "reset sequence",
                ],
                description: "reset the active sequence steps (Pads to root, notes to rest)",
            },
        }
    }

    pub(crate) const fn capture_action(self) -> Option<CaptureAction> {
        match self {
            Self::Motion(action) => match action {
                MotionAction::Grab(_) => None,
                MotionAction::Bypass => Some(CaptureAction::Bypass),
                MotionAction::Resume => Some(CaptureAction::Resume),
                MotionAction::Delete => Some(CaptureAction::Delete),
            },
            Self::Mix(_)
            | Self::Recipe(_)
            | Self::PlannedMute
            | Self::OutputDevice
            | Self::Chord(_)
            | Self::ChordNames(_)
            | Self::ClearSequence
            | Self::Section(_)
            | Self::ToggleVamp
            | Self::ToggleKickBass
            | Self::ToggleBeatRepeat
            | Self::ToggleAuto => None,
        }
    }

    pub(crate) const fn is_mix(self) -> bool {
        matches!(self, Self::Mix(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_operation_has_a_nonempty_label_and_description() {
        for operation in Operation::ALL {
            let spec = operation.spec();
            assert_eq!(spec.operation, operation);
            assert!(!spec.label.is_empty());
            assert!(!spec.description.is_empty());
        }
    }

    #[test]
    fn every_operation_projects_into_global_and_module_palettes() {
        let scope = ModuleScope {
            tab: Tab::Bass,
            slot: 0,
            catalog_index: module_catalog_index("filter"),
        };
        for module_scope in [None, Some(scope)] {
            for operation in Operation::ALL {
                let mut palette = PaletteState::new(Tab::Bass, &[], module_scope);
                for character in operation.spec().label.chars() {
                    palette.push_char(character);
                }
                assert!(palette.matches.iter().any(|found| {
                    matches!(palette.entry(found.entry_index), PaletteEntry::Operation(candidate) if *candidate == operation)
                }));
            }
        }
    }
}
