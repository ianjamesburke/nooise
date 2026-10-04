//! Typed, static musical operations projected into control surfaces.

use super::*;

/// A named musical action, distinct from an editable control.
///
/// Operations are a closed vocabulary. Their palette metadata and typed
/// execution payload stay together here; no surface dispatches a string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Operation {
    Motion(MotionAction),
    Mix(mix_action::MixAction),
    Recipe(recipe::RecipeId),
    PlannedMute,
    Chord(ChordAction),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChordAction {
    AddExtension,
    Restore,
}

/// A palette action keeps its original preset and slot, and refuses a slot
/// that changed while the palette was open (including an auto transition).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ChordTarget {
    pub(crate) progression: i8,
    pub(crate) slot: usize,
    fields: [u32; 8],
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
    pub(crate) const ALL: [Self; 18] = [
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
        Self::PlannedMute,
        Self::Chord(ChordAction::AddExtension),
        Self::Chord(ChordAction::Restore),
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
                },
                aliases: &[],
                description: match action {
                    ChordAction::AddExtension => "open the chord's second extension",
                    ChordAction::Restore => "restore this chord's original voicing",
                },
            },
            Self::PlannedMute => OperationSpec {
                operation: self,
                label: "Mute next bar",
                aliases: &["planned mute", "mute next"],
                description: "mute the visible layer at the next bar",
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
            Self::Mix(_) | Self::Recipe(_) | Self::PlannedMute | Self::Chord(_) => None,
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
