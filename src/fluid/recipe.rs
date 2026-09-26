//! Static palette recipes that compile to ordinary editable automation lanes.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecipeId {
    Sway,
    Tremolo,
    Sidechain,
}

pub(crate) struct Recipe {
    pub(crate) id: RecipeId,
    pub(crate) name: &'static str,
    pub(crate) aliases: &'static [&'static str],
    pub(crate) description: &'static str,
    pub(crate) lane: RecipeLane,
}

pub(crate) enum RecipeLane {
    Sine { beats: f32, depth: f32 },
    Envelope(EnvelopeRoute),
}

pub(crate) const RECIPES: &[Recipe] = &[
    Recipe {
        id: RecipeId::Sway,
        name: "Sway",
        aliases: &[],
        description: "slow sine, 8 beats, 25%",
        lane: RecipeLane::Sine {
            beats: 8.0,
            depth: 0.25,
        },
    },
    Recipe {
        id: RecipeId::Tremolo,
        name: "Tremolo",
        aliases: &[],
        description: "fast sine, 1/2 beat, 25%",
        lane: RecipeLane::Sine {
            beats: 0.5,
            depth: 0.25,
        },
    },
    Recipe {
        id: RecipeId::Sidechain,
        name: "Sidechain",
        aliases: &["sc"],
        description: "kick duck, 1 beat, 50%",
        lane: RecipeLane::Envelope(EnvelopeRoute {
            amount: -0.5,
            attack_beats: 0.0,
            decay_beats: 1.0,
            trigger: EnvTrigger::OnKick,
        }),
    },
];

impl RecipeId {
    /// `recipe_table_has_unique_ids_and_valid_lanes` covers every enum value.
    pub(crate) fn recipe(self) -> &'static Recipe {
        RECIPES
            .iter()
            .find(|recipe| recipe.id == self)
            .expect("recipe table covers every id")
    }
}

/// The control at palette entry, including the module occupying a slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RecipeTarget {
    pub(crate) id: &'static str,
    module_topology_revision: Option<u64>,
}

impl RecipeTarget {
    pub(crate) fn capture(id: &'static str, snapshot: &LiveSessionSnapshot) -> Option<Self> {
        spec_by_id(id)?;
        let module_topology_revision = if parse_module_slot_id(id).is_some() {
            let (module, _) = module_slot_row(id, &snapshot.controls)?;
            module.kind()?;
            Some(snapshot.module_topology_revision)
        } else {
            None
        };
        Some(Self {
            id,
            module_topology_revision,
        })
    }

    pub(crate) fn is_current(self, snapshot: &LiveSessionSnapshot) -> bool {
        Self::capture(self.id, snapshot) == Some(self)
    }
}

impl Recipe {
    pub(crate) fn check(
        &self,
        snapshot: &LiveSessionSnapshot,
        target: RecipeTarget,
    ) -> Result<(), EffectFailure> {
        if !target.is_current(snapshot) {
            return Err(EffectFailure::StaleRecipeTarget);
        }
        let address = ControlAddress::new(target.id);
        let count = match self.lane {
            RecipeLane::Sine { .. } => snapshot.automation.routes_for(address).count(),
            RecipeLane::Envelope(_) => snapshot.automation.envelopes_for(address).count(),
        };
        if count >= MAX_AUTOMATION_LANES_PER_KIND {
            return Err(EffectFailure::AutomationLaneLimit);
        }
        Ok(())
    }

    pub(crate) fn apply(
        &self,
        snapshot: &mut LiveSessionSnapshot,
        target: RecipeTarget,
    ) -> Result<(), EffectFailure> {
        self.check(snapshot, target)?;
        let address = ControlAddress::new(target.id);
        match self.lane {
            RecipeLane::Sine { beats, depth } => {
                snapshot.automation.add_route(
                    address,
                    LfoRoute {
                        cycle_beats: beats,
                        depth_ratio: depth,
                        ..LfoRoute::default()
                    },
                );
            }
            RecipeLane::Envelope(route) => {
                snapshot.automation.add_envelope(address, route);
            }
        }
        snapshot.automation.close_editor();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recipe_table_has_unique_ids_and_valid_lanes() {
        for id in [RecipeId::Sway, RecipeId::Tremolo, RecipeId::Sidechain] {
            assert_eq!(RECIPES.iter().filter(|recipe| recipe.id == id).count(), 1);
            let recipe = id.recipe();
            assert!(!recipe.name.is_empty());
            match recipe.lane {
                RecipeLane::Sine { beats, depth } => {
                    assert!((MIN_LFO_CYCLE_BEATS..=MAX_LFO_CYCLE_BEATS).contains(&beats));
                    assert!(depth > 0.0 && depth <= 1.0);
                }
                RecipeLane::Envelope(route) => {
                    assert!(route.amount.abs() > 0.0 && route.amount.abs() <= 1.0);
                    assert!((0.0..=MAX_ENV_ATTACK_BEATS).contains(&route.attack_beats));
                    assert!(route.decay_beats > 0.0 && route.decay_beats <= MAX_ENV_DECAY_BEATS);
                }
            }
        }
    }

    #[test]
    fn recipe_alias_matches_clean_display_in_global_and_module_palettes() {
        for scope in [
            None,
            Some(ModuleScope {
                tab: Tab::Bass,
                slot: 0,
                catalog_index: module_catalog_index("filter"),
            }),
        ] {
            let mut palette = PaletteState::new(Tab::Bass, &[], scope);
            palette.push_char('s');
            palette.push_char('c');
            let found = &palette.matches[0];
            let entry = palette.entry(found.entry_index);
            assert!(matches!(entry, PaletteEntry::Recipe(RecipeId::Sidechain)));
            assert_eq!(entry.display_text(), "Sidechain · kick duck, 1 beat, 50%");
            assert!(found.hits.iter().all(|&index| index < "Sidechain".len()));
        }
    }

    #[test]
    fn sidechain_ducks_half_the_dial_and_recovers_on_the_kick_grid() {
        let mut snapshot = LiveSessionSnapshot::from_controls(FluidControls::default());
        snapshot.controls.pad.level = 0.75;
        let target = RecipeTarget::capture("pad.level", &snapshot).unwrap();
        RecipeId::Sidechain
            .recipe()
            .apply(&mut snapshot, target)
            .unwrap();
        let address = ControlAddress::new(target.id);
        let route = snapshot.automation.envelope(address).unwrap();
        let context = |beat| ModContext {
            beat,
            kick_interval_beats: 2.0,
            kick_offset_beats: 0.5,
        };
        assert_eq!(route.amount * route.level_at(context(0.25)), 0.0);
        assert_eq!(route.amount * route.level_at(context(0.5)), -0.5);
        assert_eq!(route.amount * route.level_at(context(1.0)), -0.25);
        assert_eq!(route.amount * route.level_at(context(1.5)), 0.0);
        assert_eq!(route.amount * route.level_at(context(2.5)), -0.5);
        assert_eq!(snapshot.controls.pad.level, 0.75);
    }
}
