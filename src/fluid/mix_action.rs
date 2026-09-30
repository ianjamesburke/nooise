//! Static global mix actions offered by the palette, independent of knob recipes.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MixAction {
    KickOnly,
    MuteKick,
}

impl MixAction {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::KickOnly => "Kick Only",
            Self::MuteKick => "Mute Kick",
        }
    }

    pub(crate) fn aliases(self) -> &'static [&'static str] {
        match self {
            Self::KickOnly => &["solo kick", "kick only"],
            Self::MuteKick => &["mute kick", "kick mute"],
        }
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::KickOnly => "mute others",
            Self::MuteKick => "mute kick",
        }
    }

    pub(crate) fn apply(self, muted: &mut MuteState) {
        match self {
            Self::KickOnly => {
                for tab in Tab::all() {
                    muted[tab as usize] = !matches!(tab, Tab::Kick | Tab::Master);
                }
            }
            Self::MuteKick => muted[Tab::Kick as usize] = true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mute_kick_aliases_match_in_global_and_scoped_palettes() {
        let scope = ModuleScope {
            tab: Tab::Kick,
            slot: 0,
            catalog_index: MODULE_CATALOG
                .iter()
                .position(|kind| kind.id == "filter")
                .unwrap(),
        };
        for scope in [None, Some(scope)] {
            for query in MixAction::MuteKick.aliases() {
                let mut palette = PaletteState::new(Tab::Kick, &[], scope);
                for character in query.chars() {
                    palette.push_char(character);
                }
                assert!(matches!(
                    palette.entry(palette.matches[0].entry_index),
                    PaletteEntry::Operation(Operation::Mix(MixAction::MuteKick))
                ));
            }
        }
    }

    #[test]
    fn mute_kick_preserves_every_other_mute_from_every_initial_mask() {
        for mask in 0..(1usize << TAB_COUNT) {
            let before: MuteState = std::array::from_fn(|index| mask & (1 << index) != 0);
            let mut muted = before;
            for _ in 0..2 {
                MixAction::MuteKick.apply(&mut muted);
                for tab in Tab::all() {
                    assert_eq!(
                        muted[tab as usize],
                        tab == Tab::Kick || before[tab as usize],
                        "mask {mask}, tab {tab:?}"
                    );
                }
            }
        }
    }
}
