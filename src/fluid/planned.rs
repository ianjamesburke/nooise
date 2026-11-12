//! The chord vamp: a held or latched chord waiting on chord boundaries.

use super::push::HeldChord;

/// Left-hand chord vamp (`a` in Browsing): quantizes to nearest chord start,
/// holds whatever chord slot the progression lands on across all chord-following
/// voices until the player taps `a` again to release on the next chord start
/// into the next chord in the progression.
///
/// `chord` is the chord the vamp holds, the notes themselves rather than a
/// progression slot, so editing the progression, Key or Mode underneath
/// never moves a vamp. The engine fills it when the vamp lands; until then
/// the slot's chord is read live.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct VampState {
    pub(crate) start_beat: f64,
    pub(crate) end_beat: f64,
    pub(crate) slot: u8,
    pub(crate) chord: Option<HeldChord>,
}

impl VampState {
    pub(crate) fn start(start_beat: f64, slot: u8) -> Self {
        Self {
            start_beat,
            end_beat: f64::INFINITY,
            slot,
            chord: None,
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
            chord: self.chord,
        }
    }
}
