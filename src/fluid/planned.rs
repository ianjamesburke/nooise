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

/// Duration of the left-hand breakdown/vamp (`a` in Browsing): 16 four-beat
/// bars (64 beats) on Chord 1 with Kick and Bass muted, after which Kick and
/// Bass drop back in together and the progression resumes on bar 17.
pub(crate) const VAMP_BARS: usize = 16;
pub(crate) const VAMP_BEATS: f64 = (VAMP_BARS * 4) as f64;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct VampState {
    pub(crate) start_beat: f64,
    pub(crate) end_beat: f64,
}

impl VampState {
    pub(crate) fn start(beat: f64) -> Self {
        let start_beat = super::next_bar_beat(beat.max(0.0));
        Self {
            start_beat,
            end_beat: start_beat + VAMP_BEATS,
        }
    }

    pub(crate) fn is_active(self, beat: f64) -> bool {
        beat >= self.start_beat && beat < self.end_beat
    }

    pub(crate) fn is_releasing_early(self) -> bool {
        self.end_beat < self.start_beat + VAMP_BEATS - 1e-6
    }

    pub(crate) fn is_due(self, beat: f64) -> bool {
        beat >= self.end_beat
    }

    pub(crate) fn rebased_at(self, beat: f64) -> Self {
        let beat = beat.max(0.0);
        Self {
            start_beat: self.start_beat - beat,
            end_beat: (self.end_beat - beat).max(0.0),
        }
    }
}
