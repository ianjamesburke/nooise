//! Temporary performance envelopes over the running song's Master bus. Audio time, rather
//! than keyboard repeat or UI ticks, determines every gesture's amount.

pub(crate) const GESTURE_COUNT: usize = 4;

/// The discriminant is the storage index. The song-code tag is `wire_tag`,
/// which never moves when a gesture retires.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum GestureKind {
    Bloom,
    Submerge,
    Echo,
    Lift,
}

impl GestureKind {
    pub(crate) const ALL: [Self; GESTURE_COUNT] =
        [Self::Bloom, Self::Submerge, Self::Echo, Self::Lift];

    /// Wire tag 3 belonged to the retired Thin gesture and is never reused.
    pub(crate) const RETIRED_THIN_WIRE_TAG: u8 = 3;

    pub(crate) const fn wire_tag(self) -> u8 {
        match self {
            Self::Bloom => 0,
            Self::Submerge => 1,
            Self::Echo => 2,
            Self::Lift => 4,
        }
    }

    pub(crate) fn from_wire_tag(tag: u8) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.wire_tag() == tag)
    }

    pub(crate) fn from_key(key: char) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.key() == key)
    }

    pub(crate) const fn key(self) -> char {
        match self {
            Self::Bloom => 'z',
            Self::Submerge => 'c',
            Self::Echo => 'v',
            Self::Lift => 'x',
        }
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Bloom => "Bloom",
            Self::Submerge => "Submerge",
            Self::Echo => "Echo",
            Self::Lift => "Lift",
        }
    }

    fn rise_seconds(self) -> f64 {
        match self {
            Self::Bloom => 1.5,
            Self::Submerge => 1.2,
            Self::Echo => 0.7,
            Self::Lift => 0.55,
        }
    }

    fn return_seconds(self) -> f64 {
        0.05
    }
}

/// An amount anchored to the engine's monotonic seconds. Keeping the anchor
/// immutable between presses lets audio and UI evaluate the same trajectory
/// without per-sample session writes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct GestureEnvelope {
    pub(crate) amount: f32,
    pub(crate) at_seconds: f64,
    pub(crate) held: bool,
    /// A loaded hold is audible without pretending a physical key is down.
    pub(crate) restored: bool,
}

impl GestureEnvelope {
    pub(crate) fn amount_at(self, kind: GestureKind, now_seconds: f64) -> f32 {
        let elapsed = (now_seconds - self.at_seconds).max(0.0);
        if self.held {
            (self.amount as f64 + elapsed / kind.rise_seconds()).min(1.0) as f32
        } else {
            (self.amount as f64 - elapsed / kind.return_seconds()).max(0.0) as f32
        }
    }

    fn release(&mut self, kind: GestureKind, now_seconds: f64) {
        if self.held {
            self.amount = self.amount_at(kind, now_seconds);
            self.at_seconds = now_seconds;
            self.held = false;
            self.restored = false;
        }
    }
}

/// One envelope per gesture. Every gesture plays over the Master bus, so the
/// state carries no target; the key alone names the lane.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct GestureState {
    pub(crate) lanes: [GestureEnvelope; GESTURE_COUNT],
}

impl GestureState {
    pub(crate) fn envelope(&self, kind: GestureKind) -> &GestureEnvelope {
        &self.lanes[kind as usize]
    }

    pub(crate) fn amounts(&self, now_seconds: f64) -> [f32; GESTURE_COUNT] {
        GestureKind::ALL.map(|kind| self.envelope(kind).amount_at(kind, now_seconds))
    }

    pub(crate) fn has_held(&self) -> bool {
        self.lanes.iter().any(|envelope| envelope.held)
    }

    pub(crate) fn press(&mut self, kind: GestureKind, now_seconds: f64) {
        let envelope = &mut self.lanes[kind as usize];
        // A restored hold is claimed rather than restarted, and a repeat or
        // duplicate press cannot restart a gesture already held.
        if envelope.held {
            envelope.restored = false;
            return;
        }
        *envelope = GestureEnvelope {
            amount: envelope.amount_at(kind, now_seconds),
            at_seconds: now_seconds,
            held: true,
            restored: false,
        };
    }

    pub(crate) fn release(&mut self, kind: GestureKind, now_seconds: f64) {
        self.lanes[kind as usize].release(kind, now_seconds);
    }

    pub(crate) fn release_all(&mut self, now_seconds: f64) {
        for kind in GestureKind::ALL {
            self.release(kind, now_seconds);
        }
    }

    /// Capture compact envelope state relative to the next playback's zero.
    /// Completed returns disappear; processor tails are deliberately absent.
    pub(crate) fn snapshot_at(&self, now_seconds: f64) -> Self {
        let mut saved = Self::default();
        for kind in GestureKind::ALL {
            let envelope = self.envelope(kind);
            let amount = envelope.amount_at(kind, now_seconds);
            if envelope.held || amount > 0.0 {
                saved.lanes[kind as usize] = GestureEnvelope {
                    amount,
                    at_seconds: 0.0,
                    held: envelope.held,
                    restored: envelope.held,
                };
            }
        }
        saved
    }

    pub(crate) fn restored(&self) -> Self {
        let mut state = self.clone();
        for envelope in &mut state.lanes {
            envelope.restored = envelope.held;
        }
        state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_touch_release_and_repress_are_continuous() {
        let mut state = GestureState::default();
        state.press(GestureKind::Submerge, 10.0);
        let rising = state.amounts(10.3)[1];
        assert!((rising - 0.25).abs() < 1e-6);
        state.release(GestureKind::Submerge, 10.3);
        assert_eq!(state.amounts(10.3)[1], rising);
        let returning = state.amounts(10.35)[1];
        assert!(returning < rising);
        state.press(GestureKind::Submerge, 10.35);
        assert_eq!(state.amounts(10.35)[1], returning);
        assert!(state.amounts(10.4)[1] > returning);
    }

    #[test]
    fn every_gesture_returns_to_rest_within_fifty_milliseconds() {
        for kind in GestureKind::ALL {
            let mut state = GestureState::default();
            state.press(kind, 0.0);
            state.release(kind, kind.rise_seconds());
            assert!(
                state.amounts(kind.rise_seconds() + 0.05)[kind as usize] <= f32::EPSILON,
                "{} did not return within 50 ms",
                kind.name()
            );
        }
    }

    #[test]
    fn duplicate_press_keeps_the_original_trajectory() {
        let mut state = GestureState::default();
        state.press(GestureKind::Bloom, 2.0);
        state.press(GestureKind::Bloom, 2.5);
        assert!((state.amounts(2.75)[0] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn snapshot_resumes_held_and_returning_trajectories_at_zero() {
        let mut state = GestureState::default();
        state.press(GestureKind::Bloom, 8.0);
        state.press(GestureKind::Lift, 7.0);
        state.release(GestureKind::Lift, 8.4);
        let saved = state.snapshot_at(8.5);
        for offset in [0.0, 0.1, 0.2] {
            for kind in GestureKind::ALL {
                assert!(
                    (state.envelope(kind).amount_at(kind, 8.5 + offset)
                        - saved.envelope(kind).amount_at(kind, offset))
                    .abs()
                        < 1e-6
                );
            }
        }
        assert!(saved.envelope(GestureKind::Bloom).restored);
    }

    #[test]
    fn restored_hold_is_claimed_then_releases() {
        let mut state = GestureState::default();
        state.press(GestureKind::Echo, 0.0);
        let mut saved = state.snapshot_at(0.2);
        saved.press(GestureKind::Echo, 0.1);
        assert!(saved.envelope(GestureKind::Echo).held);
        assert!(!saved.envelope(GestureKind::Echo).restored);
        saved.release_all(0.2);
        assert!(!saved.has_held());
        assert_eq!(saved.snapshot_at(1.0), GestureState::default());
    }
}
