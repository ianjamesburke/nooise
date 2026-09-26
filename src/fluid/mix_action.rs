//! Static global mix actions offered by the palette, independent of knob recipes.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MixAction {
    KickOnly,
}

pub(crate) const MIX_ACTIONS: &[MixAction] = &[MixAction::KickOnly];

impl MixAction {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::KickOnly => "Kick Only",
        }
    }

    pub(crate) fn aliases(self) -> &'static [&'static str] {
        match self {
            Self::KickOnly => &["solo kick", "kick only"],
        }
    }

    pub(crate) fn apply(self, muted: &mut MuteState) {
        match self {
            Self::KickOnly => {
                for tab in Tab::all() {
                    muted[tab as usize] = !matches!(tab, Tab::Kick | Tab::Master);
                }
            }
        }
    }
}
