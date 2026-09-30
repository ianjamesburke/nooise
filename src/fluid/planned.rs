//! Musical actions waiting for a transport boundary.

use super::Tab;

/// A user-visible action that will land at an absolute song beat.
///
/// The action stays in the aggregate session rather than the UI executor so a
/// save carries the audible future and the production tick owns its commit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum PlannedAction {
    Mute { tab: Tab, target_beat: f64 },
}

impl PlannedAction {
    pub(crate) const fn tab(self) -> Tab {
        match self {
            Self::Mute { tab, .. } => tab,
        }
    }

    pub(crate) fn is_due(self, beat: f64) -> bool {
        match self {
            Self::Mute { target_beat, .. } => beat >= target_beat,
        }
    }

    /// Rebase an absolute live target onto beat zero for a song code.
    pub(crate) fn rebased_at(self, beat: f64) -> Self {
        match self {
            Self::Mute { tab, target_beat } => Self::Mute {
                tab,
                target_beat: (target_beat - beat).max(0.0),
            },
        }
    }
}
