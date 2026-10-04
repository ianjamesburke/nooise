//! Static palette recipes that compile to ordinary editable automation lanes.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecipeId {
    Sway,
    Tremolo,
    Sidechain,
    Pulse,
    Drift,
    Rise,
}

pub(crate) struct Recipe {
    pub(crate) id: RecipeId,
    pub(crate) name: &'static str,
    pub(crate) aliases: &'static [&'static str],
    pub(crate) description: &'static str,
    pub(crate) lane: RecipeLane,
}

pub(crate) enum RecipeLane {
    Lfo {
        shape: LfoShape,
        beats: f32,
        depth: f32,
        seed: u32,
    },
}

pub(crate) const RECIPES: &[Recipe] = &[
    Recipe {
        id: RecipeId::Sway,
        name: "Sway",
        aliases: &[],
        description: "slow sine, 8 beats, 25%",
        lane: RecipeLane::Lfo {
            shape: LfoShape::Sine,
            seed: 0,
            beats: 8.0,
            depth: 0.25,
        },
    },
    Recipe {
        id: RecipeId::Tremolo,
        name: "Tremolo",
        aliases: &[],
        description: "fast sine, 1/2 beat, 25%",
        lane: RecipeLane::Lfo {
            shape: LfoShape::Sine,
            seed: 0,
            beats: 0.5,
            depth: 0.25,
        },
    },
    Recipe {
        id: RecipeId::Sidechain,
        name: "Sidechain",
        aliases: &["sc"],
        description: "beat ramp duck, 1 beat, 50%",
        lane: RecipeLane::Lfo {
            shape: LfoShape::RampUp,
            beats: 1.0,
            depth: 0.25,
            seed: 0,
        },
    },
    Recipe {
        id: RecipeId::Pulse,
        name: "Pulse",
        aliases: &[],
        description: "square, 1 beat, 25%",
        lane: RecipeLane::Lfo {
            shape: LfoShape::Square,
            beats: 1.0,
            depth: 0.25,
            seed: 0,
        },
    },
    Recipe {
        id: RecipeId::Drift,
        name: "Drift",
        aliases: &[],
        description: "slow random drift, 16 beats, 25%",
        lane: RecipeLane::Lfo {
            shape: LfoShape::RandomDrift,
            beats: 16.0,
            depth: 0.25,
            seed: 0x4452_4946,
        },
    },
    Recipe {
        id: RecipeId::Rise,
        name: "Rise",
        aliases: &[],
        description: "ramp up, 8 beats, 25%",
        lane: RecipeLane::Lfo {
            shape: LfoShape::RampUp,
            beats: 8.0,
            depth: 0.25,
            seed: 0,
        },
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
    chord_progression: Option<i8>,
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
            chord_progression: parse_chord_slot_id(id).map(|_| {
                PROGRESSION_SONG_VALUES[progression_index(snapshot.controls.pad.progression)]
            }),
        })
    }

    pub(crate) fn is_current(self, snapshot: &LiveSessionSnapshot) -> bool {
        Self::capture(self.id, snapshot) == Some(self)
    }
}

impl Recipe {
    fn lfo_route(&self) -> LfoRoute {
        match self.lane {
            RecipeLane::Lfo {
                shape,
                beats,
                depth,
                seed,
            } => LfoRoute {
                cycle_beats: beats,
                depth_ratio: depth,
                shape,
                seed,
                ..LfoRoute::default()
            },
        }
    }

    fn keeps_one_authored_lane(&self) -> bool {
        self.id == RecipeId::Tremolo
    }

    pub(crate) fn check(
        &self,
        snapshot: &LiveSessionSnapshot,
        target: RecipeTarget,
    ) -> Result<(), EffectFailure> {
        if !target.is_current(snapshot) {
            return Err(EffectFailure::StaleRecipeTarget);
        }
        let address = ControlAddress::new(target.id);
        let route = self.lfo_route();
        if self.keeps_one_authored_lane()
            && snapshot
                .automation
                .routes_for(address)
                .any(|lane| *lane == route)
        {
            return Ok(());
        }
        let count = snapshot.automation.routes_for(address).count();
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
        if self.id == RecipeId::Sidechain {
            let spec = spec_by_id(target.id).ok_or(EffectFailure::StaleRecipeTarget)?;
            let ratio = spec.ratio((spec.get)(&snapshot.controls), &snapshot.controls);
            spec.apply_ratio((ratio - 0.25).max(0.0), &mut snapshot.controls);
        }
        match self.lane {
            RecipeLane::Lfo { .. } => {
                let route = self.lfo_route();
                let duplicate = self.keeps_one_authored_lane()
                    && snapshot
                        .automation
                        .routes_for(address)
                        .any(|lane| *lane == route);
                if !duplicate {
                    snapshot.automation.add_route(address, route);
                }
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
        for id in [
            RecipeId::Sway,
            RecipeId::Tremolo,
            RecipeId::Sidechain,
            RecipeId::Pulse,
            RecipeId::Drift,
            RecipeId::Rise,
        ] {
            assert_eq!(RECIPES.iter().filter(|recipe| recipe.id == id).count(), 1);
            let recipe = id.recipe();
            assert!(!recipe.name.is_empty());
            match recipe.lane {
                RecipeLane::Lfo { beats, depth, .. } => {
                    assert!((MIN_LFO_CYCLE_BEATS..=MAX_LFO_CYCLE_BEATS).contains(&beats));
                    assert!(depth > 0.0 && depth <= 1.0);
                }
            }
        }
    }

    #[test]
    fn every_recipe_name_matches_in_global_and_module_palettes() {
        for scope in [
            None,
            Some(ModuleScope {
                tab: Tab::Bass,
                slot: 0,
                catalog_index: module_catalog_index("filter"),
            }),
        ] {
            for recipe in RECIPES {
                let mut palette = PaletteState::new(Tab::Bass, &[], scope);
                for c in recipe.name.to_lowercase().chars() {
                    palette.push_char(c);
                }
                let entry = palette.entry(palette.matches[0].entry_index);
                assert!(
                    matches!(entry, PaletteEntry::Operation(Operation::Recipe(id)) if *id == recipe.id),
                    "{}",
                    recipe.name
                );
            }
        }
    }

    #[test]
    fn every_recipe_respects_existing_silent_lane_capacity() {
        for recipe in RECIPES {
            let mut snapshot = LiveSessionSnapshot::from_controls(FluidControls::default());
            let target = RecipeTarget::capture("pad.level", &snapshot).unwrap();
            let address = ControlAddress::new(target.id);
            for _ in 0..MAX_AUTOMATION_LANES_PER_KIND {
                match recipe.lane {
                    RecipeLane::Lfo { .. } => {
                        snapshot.automation.add_route(address, LfoRoute::default());
                    }
                }
            }
            let before = snapshot.automation.clone();
            assert_eq!(
                recipe.apply(&mut snapshot, target),
                Err(EffectFailure::AutomationLaneLimit)
            );
            assert!(
                snapshot.automation == before,
                "capacity failure leaves lanes untouched"
            );
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
            assert!(matches!(
                entry,
                PaletteEntry::Operation(Operation::Recipe(RecipeId::Sidechain))
            ));
            assert_eq!(
                entry.display_text(),
                "Sidechain · beat ramp duck, 1 beat, 50%"
            );
            assert!(found.hits.iter().all(|&index| index < "Sidechain".len()));
        }
    }

    #[test]
    fn sidechain_rises_each_beat_independently_of_the_kick_grid() {
        let mut snapshot = LiveSessionSnapshot::from_controls(FluidControls::default());
        snapshot.controls.pad.level = 0.75;
        let target = RecipeTarget::capture("pad.level", &snapshot).unwrap();
        RecipeId::Sidechain
            .recipe()
            .apply(&mut snapshot, target)
            .unwrap();
        let address = ControlAddress::new(target.id);
        let route = snapshot.automation.route(address).unwrap();
        let context = |beat| ModContext {
            beat,
            kick_interval_beats: 2.0,
            kick_offset_beats: 0.5,
        };
        let spec = spec_by_id(target.id).unwrap();
        let value = |beat| {
            modulated_control_value_full(
                spec,
                &[*route],
                &[],
                snapshot.controls.pad.level,
                context(beat),
            )
        };
        assert_eq!(route.shape, LfoShape::RampUp);
        assert_eq!(value(0.0), 0.25);
        assert_eq!(value(0.25), 0.375);
        assert_eq!(value(0.5), 0.5);
        assert_eq!(value(0.75), 0.625);
        assert_eq!(value(1.0), 0.25);
        assert_eq!(snapshot.controls.pad.level, 0.5);
        assert!(snapshot.automation.envelope(address).is_none());
    }
}
