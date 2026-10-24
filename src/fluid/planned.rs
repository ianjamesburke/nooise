//! Musical actions waiting for a transport boundary.

use super::Tab;

/// A user-visible action that will land at an absolute song beat.
///
/// The action stays in the aggregate session rather than the UI executor so a
/// save carries the audible future and the audio clock owns its exact commit.
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

/// Left-hand chord vamp (`a` in Browsing): arms for the next bar downbeat,
/// holds whatever chord slot the progression lands on at `start_beat` across
/// all chord-following voices until the player taps `a` again to release on
/// the next bar downbeat into the next chord in the progression.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct VampState {
    pub(crate) start_beat: f64,
    pub(crate) end_beat: f64,
    pub(crate) slot: u8,
}

impl VampState {
    pub(crate) fn start(beat: f64, slot: u8) -> Self {
        let start_beat = super::next_bar_beat(beat.max(0.0));
        Self {
            start_beat,
            end_beat: f64::INFINITY,
            slot,
        }
    }

    pub(crate) fn is_active(self, beat: f64) -> bool {
        beat >= self.start_beat && beat < self.end_beat
    }

    pub(crate) fn is_releasing(self) -> bool {
        self.end_beat.is_finite()
    }

    pub(crate) fn is_due(self, beat: f64) -> bool {
        self.end_beat.is_finite() && beat >= self.end_beat
    }

    pub(crate) fn rebased_at(self, beat: f64) -> Self {
        let beat = beat.max(0.0);
        Self {
            start_beat: self.start_beat - beat,
            end_beat: if self.end_beat.is_finite() {
                (self.end_beat - beat).max(0.0)
            } else {
                f64::INFINITY
            },
            slot: self.slot,
        }
    }
}
