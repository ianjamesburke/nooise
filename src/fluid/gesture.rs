//! Temporary performance envelopes over the running song. Audio time, rather
//! than keyboard repeat or UI ticks, determines every gesture's amount.

use super::*;

pub(crate) const GESTURE_COUNT: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum GestureKind {
    Bloom,
    Submerge,
    Echo,
    Thin,
}

impl GestureKind {
    pub(crate) const ALL: [Self; GESTURE_COUNT] =
        [Self::Bloom, Self::Submerge, Self::Echo, Self::Thin];

    pub(crate) fn from_key(key: char) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.key() == key)
    }

    pub(crate) const fn key(self) -> char {
        match self {
            Self::Bloom => 'z',
            Self::Submerge => 'c',
            Self::Echo => 'v',
            Self::Thin => 'b',
        }
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Bloom => "Bloom",
            Self::Submerge => "Submerge",
            Self::Echo => "Echo",
            Self::Thin => "Thin",
        }
    }

    fn rise_seconds(self) -> f64 {
        match self {
            Self::Bloom => 1.5,
            Self::Submerge => 1.2,
            Self::Echo => 0.7,
            Self::Thin => 1.0,
        }
    }

    fn return_seconds(self) -> f64 {
        match self {
            Self::Bloom => 0.8,
            Self::Submerge => 0.45,
            Self::Echo => 0.15,
            Self::Thin => 0.4,
        }
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

/// Fixed storage per layer and gesture. A released envelope may coexist with
/// the same gesture newly held on another layer; no target change allocates.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct GestureState {
    pub(crate) lanes: [[GestureEnvelope; GESTURE_COUNT]; TAB_COUNT],
}

impl GestureState {
    pub(crate) fn envelope(&self, tab: Tab, kind: GestureKind) -> &GestureEnvelope {
        &self.lanes[tab as usize][kind as usize]
    }

    pub(crate) fn amounts(&self, tab: Tab, now_seconds: f64) -> [f32; GESTURE_COUNT] {
        GestureKind::ALL.map(|kind| self.envelope(tab, kind).amount_at(kind, now_seconds))
    }

    pub(crate) fn has_held(&self) -> bool {
        self.lanes.iter().flatten().any(|envelope| envelope.held)
    }

    pub(crate) fn held_target(&self, kind: GestureKind) -> Option<Tab> {
        Tab::all()
            .into_iter()
            .find(|tab| self.envelope(*tab, kind).held)
    }

    pub(crate) fn press(&mut self, kind: GestureKind, tab: Tab, now_seconds: f64) {
        // A restored hold is claimed where it was saved. Repeat/duplicate
        // presses also cannot retarget or restart a gesture already held.
        if let Some(held_tab) = self.held_target(kind) {
            self.lanes[held_tab as usize][kind as usize].restored = false;
            return;
        }
        let envelope = &mut self.lanes[tab as usize][kind as usize];
        *envelope = GestureEnvelope {
            amount: envelope.amount_at(kind, now_seconds),
            at_seconds: now_seconds,
            held: true,
            restored: false,
        };
    }

    pub(crate) fn release(&mut self, kind: GestureKind, now_seconds: f64) {
        for layer in &mut self.lanes {
            layer[kind as usize].release(kind, now_seconds);
        }
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
        for tab in Tab::all() {
            for kind in GestureKind::ALL {
                let envelope = self.envelope(tab, kind);
                let amount = envelope.amount_at(kind, now_seconds);
                if envelope.held || amount > 0.0 {
                    saved.lanes[tab as usize][kind as usize] = GestureEnvelope {
                        amount,
                        at_seconds: 0.0,
                        held: envelope.held,
                        restored: envelope.held,
                    };
                }
            }
        }
        saved
    }

    pub(crate) fn restored(&self) -> Self {
        let mut state = self.clone();
        for envelope in state.lanes.iter_mut().flatten() {
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
        state.press(GestureKind::Submerge, Tab::Chords, 10.0);
        let rising = state.amounts(Tab::Chords, 10.3)[1];
        assert!((rising - 0.25).abs() < 1e-6);
        state.release(GestureKind::Submerge, 10.3);
        assert_eq!(state.amounts(Tab::Chords, 10.3)[1], rising);
        let returning = state.amounts(Tab::Chords, 10.35)[1];
        assert!(returning < rising);
        state.press(GestureKind::Submerge, Tab::Chords, 10.35);
        assert_eq!(state.amounts(Tab::Chords, 10.35)[1], returning);
        assert!(state.amounts(Tab::Chords, 10.4)[1] > returning);
    }

    #[test]
    fn duplicate_press_keeps_original_target_and_trajectory() {
        let mut state = GestureState::default();
        state.press(GestureKind::Bloom, Tab::Chords, 2.0);
        state.press(GestureKind::Bloom, Tab::Perc, 2.5);
        assert_eq!(state.held_target(GestureKind::Bloom), Some(Tab::Chords));
        assert_eq!(state.amounts(Tab::Perc, 3.0), [0.0; GESTURE_COUNT]);
        assert!((state.amounts(Tab::Chords, 2.75)[0] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn old_target_returns_while_new_target_rises() {
        let mut state = GestureState::default();
        state.press(GestureKind::Bloom, Tab::Chords, 0.0);
        state.release(GestureKind::Bloom, 1.5);
        state.press(GestureKind::Bloom, Tab::Perc, 1.6);
        assert!(state.amounts(Tab::Chords, 1.7)[0] > 0.0);
        assert!(state.amounts(Tab::Perc, 1.7)[0] > 0.0);
        assert_eq!(state.held_target(GestureKind::Bloom), Some(Tab::Perc));
    }

    #[test]
    fn snapshot_resumes_held_and_returning_trajectories_at_zero() {
        let mut state = GestureState::default();
        state.press(GestureKind::Bloom, Tab::Chords, 8.0);
        state.press(GestureKind::Thin, Tab::Master, 7.0);
        state.release(GestureKind::Thin, 8.4);
        let saved = state.snapshot_at(8.5);
        for offset in [0.0, 0.1, 0.2] {
            for tab in [Tab::Chords, Tab::Master] {
                for kind in GestureKind::ALL {
                    assert!(
                        (state.envelope(tab, kind).amount_at(kind, 8.5 + offset)
                            - saved.envelope(tab, kind).amount_at(kind, offset))
                        .abs()
                            < 1e-6
                    );
                }
            }
        }
        assert!(saved.envelope(Tab::Chords, GestureKind::Bloom).restored);
    }

    #[test]
    fn restored_hold_is_claimed_on_its_saved_target_then_releases() {
        let mut state = GestureState::default();
        state.press(GestureKind::Echo, Tab::Tonal, 0.0);
        let mut saved = state.snapshot_at(0.2);
        saved.press(GestureKind::Echo, Tab::Master, 0.1);
        assert_eq!(saved.held_target(GestureKind::Echo), Some(Tab::Tonal));
        assert!(!saved.envelope(Tab::Tonal, GestureKind::Echo).restored);
        saved.release_all(0.2);
        assert!(!saved.has_held());
        assert_eq!(saved.snapshot_at(1.0), GestureState::default());
    }
}
