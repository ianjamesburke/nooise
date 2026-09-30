//! Typed, static musical operations projected into control surfaces.

use super::*;

/// A named musical action, distinct from an editable control.
///
/// Operations are a closed vocabulary. Their palette metadata and typed
/// execution payload stay together here; no surface dispatches a string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Operation {
    Capture(CaptureAction),
    Mix(mix_action::MixAction),
    Recipe(recipe::RecipeId),
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
    pub(crate) const ALL: [Self; 12] = [
        Self::Capture(CaptureAction::Capture),
        Self::Capture(CaptureAction::Bypass),
        Self::Capture(CaptureAction::Resume),
        Self::Capture(CaptureAction::Delete),
        Self::Mix(mix_action::MixAction::KickOnly),
        Self::Mix(mix_action::MixAction::MuteKick),
        Self::Recipe(recipe::RecipeId::Sway),
        Self::Recipe(recipe::RecipeId::Tremolo),
        Self::Recipe(recipe::RecipeId::Sidechain),
        Self::Recipe(recipe::RecipeId::Pulse),
        Self::Recipe(recipe::RecipeId::Drift),
        Self::Recipe(recipe::RecipeId::Rise),
    ];

    pub(crate) fn spec(self) -> OperationSpec {
        match self {
            Self::Capture(action) => OperationSpec {
                operation: self,
                label: action.name(),
                aliases: &[],
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
        }
    }

    pub(crate) const fn capture_action(self) -> Option<CaptureAction> {
        match self {
            Self::Capture(action) => Some(action),
            Self::Mix(_) | Self::Recipe(_) => None,
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
