//! Push the Harmony's live state: the chord the harmony has been pushed to,
//! a move waiting for its bar, and a bounded undo history.
//!
//! The harmony model itself (`Triad`, `HarmonyMove`, `voice_lead`) is pure and
//! lives in `voice/triad.rs`. This module only decides *when* a chord sounds:
//! a move queues, composes with any move already queued, and lands on the next
//! bar, where the audio clock voices it from the notes sounding at that moment.

use super::*;

/// How many moves `u` can walk back. Live-only, never in a song code.
pub(crate) const PUSH_UNDO_DEPTH: usize = 16;

/// The chord the harmony was pushed to and how the pad voices it. While one
/// is held, the progression is overridden: Pads, Bass, Arp and Lead stay on
/// table slot `slot` and read this chord instead of the authored one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct HeldChord {
    pub(crate) slot: u8,
    pub(crate) triad: Triad,
    /// Ascending MIDI notes in progression coordinates (before Master Key).
    pub(crate) notes: [i32; 4],
}

/// Where a queued move takes the harmony when its bar arrives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PushTarget {
    Chord(Triad),
    /// Hand the harmony back to the authored progression.
    Release,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct QueuedPush {
    pub(crate) target: PushTarget,
    /// The bar line it lands on, in transport beats.
    pub(crate) land_beat: f64,
}

/// What the harmony was before one move: a pushed triad, or the progression.
type UndoEntry = Option<Triad>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct UndoHistory {
    entries: [UndoEntry; PUSH_UNDO_DEPTH],
    len: usize,
}

impl UndoHistory {
    fn push(&mut self, entry: UndoEntry) {
        if self.len == PUSH_UNDO_DEPTH {
            self.entries.copy_within(1.., 0);
            self.len -= 1;
        }
        self.entries[self.len] = entry;
        self.len += 1;
    }

    fn pop(&mut self) -> Option<UndoEntry> {
        self.len = self.len.checked_sub(1)?;
        Some(self.entries[self.len])
    }
}

/// The whole of Push the Harmony's state, carried in the live session.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct PushedHarmony {
    pub(crate) held: Option<HeldChord>,
    pub(crate) queued: Option<QueuedPush>,
    undo: UndoHistory,
}

/// One Push the Harmony key, as the interaction kernel names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HarmonyAction {
    Move(HarmonyMove),
    Undo,
    /// Hand the harmony back to the authored progression at the next bar.
    Release,
}

/// What one push key did, for the confirmation message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PushOutcome {
    Queued(PushTarget),
    /// The queued move was withdrawn; the harmony stays where it is.
    Cancelled,
    Nothing,
}

impl PushedHarmony {
    /// A saved song's state: what was held, and a queued target, which lands
    /// on the loaded song's first bar. Undo history is never saved.
    pub(crate) fn restored(held: Option<HeldChord>, queued: Option<PushTarget>) -> Self {
        Self {
            held,
            queued: queued.map(|target| QueuedPush {
                target,
                land_beat: 0.0,
            }),
            undo: UndoHistory::default(),
        }
    }

    /// Whether the harmony is pushed or about to be.
    pub(crate) fn is_active(&self) -> bool {
        self.held.is_some() || self.queued.is_some()
    }

    /// The harmony a new move reads: the queued chord, else the held one,
    /// else `authored`, the progression chord sounding now.
    pub(crate) fn base(&self, authored: Triad) -> Triad {
        match self.queued.map(|queued| queued.target) {
            Some(PushTarget::Chord(triad)) => triad,
            Some(PushTarget::Release) => authored,
            None => self.held.map_or(authored, |held| held.triad),
        }
    }

    /// Where the harmony is headed, as the undo history records it.
    fn heading(&self) -> UndoEntry {
        match self.queued.map(|queued| queued.target) {
            Some(PushTarget::Chord(triad)) => Some(triad),
            Some(PushTarget::Release) => None,
            None => self.held.map(|held| held.triad),
        }
    }

    fn queue(&mut self, target: PushTarget, beat: f64) {
        let land_beat = self
            .queued
            .map_or_else(|| next_bar_beat(beat), |queued| queued.land_beat);
        self.queued = Some(QueuedPush { target, land_beat });
    }

    /// Queue `harmony_move` from the current base for the next bar. A second
    /// move before that bar composes from the queued chord and keeps its bar.
    pub(crate) fn push(
        &mut self,
        harmony_move: HarmonyMove,
        key: HomeKey,
        authored: Triad,
        beat: f64,
    ) -> PushOutcome {
        let target = harmony_move.apply(self.base(authored), key);
        self.undo.push(self.heading());
        self.queue(PushTarget::Chord(target), beat);
        PushOutcome::Queued(PushTarget::Chord(target))
    }

    /// Queue the hand-back to the authored progression for the next bar.
    pub(crate) fn release(&mut self, beat: f64) -> PushOutcome {
        if self.heading().is_none() {
            return PushOutcome::Nothing;
        }
        self.undo.push(self.heading());
        self.queue(PushTarget::Release, beat);
        PushOutcome::Queued(PushTarget::Release)
    }

    /// Walk back one move. Returning to where the harmony already sounds
    /// withdraws the queue; anywhere else queues that harmony for the bar.
    pub(crate) fn undo(&mut self, beat: f64) -> PushOutcome {
        let Some(previous) = self.undo.pop() else {
            return PushOutcome::Nothing;
        };
        if previous == self.held.map(|held| held.triad) {
            self.queued = None;
            return PushOutcome::Cancelled;
        }
        let target = previous.map_or(PushTarget::Release, PushTarget::Chord);
        self.queue(target, beat);
        PushOutcome::Queued(target)
    }

    pub(crate) fn is_due(&self, beat: f64) -> bool {
        self.queued.is_some_and(|queued| beat >= queued.land_beat)
    }

    /// Land the queued move: voice its chord from `sounding` on `slot` (a
    /// held chord keeps its slot), or hand back to the progression.
    pub(crate) fn land(&mut self, sounding: [i32; 4], slot: usize) {
        let Some(queued) = self.queued.take() else {
            return;
        };
        self.held = match queued.target {
            PushTarget::Chord(triad) => Some(HeldChord {
                slot: self.held.map_or(slot as u8, |held| held.slot),
                triad,
                notes: voice_lead(triad, sounding),
            }),
            PushTarget::Release => None,
        };
    }

    /// A restart begins at bar one, so a waiting move lands there.
    pub(crate) fn restarted(mut self) -> Self {
        if let Some(queued) = &mut self.queued {
            queued.land_beat = 0.0;
        }
        self
    }
}

impl PushedHarmony {
    /// Apply one key. `authored` is the progression chord sounding now.
    pub(crate) fn act(
        &mut self,
        action: HarmonyAction,
        key: HomeKey,
        authored: Triad,
        beat: f64,
    ) -> PushOutcome {
        match action {
            HarmonyAction::Move(harmony_move) => self.push(harmony_move, key, authored, beat),
            HarmonyAction::Undo => self.undo(beat),
            HarmonyAction::Release => self.release(beat),
        }
    }
}

/// The progression chord sounding on `slot`, heard as a triad in its home key.
pub(crate) fn authored_triad(pad: &PadControls, progression: usize, slot: usize) -> Triad {
    let cell = Cell::from_slot(pad.chord_slot(progression, slot));
    Triad::from_harmony(&cell.harmony, HomeKey::of_progression(progression))
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: HomeKey = HomeKey::new(9, HomeMode::Minor);

    fn am() -> Triad {
        Triad::new(9, TriadQuality::Minor)
    }

    fn target(push: &PushedHarmony) -> Option<PushTarget> {
        push.queued.map(|queued| queued.target)
    }

    #[test]
    fn a_move_queues_for_the_next_bar_and_a_second_composes_from_it() {
        let mut push = PushedHarmony::default();
        push.push(HarmonyMove::FifthUp, KEY, am(), 5.5);
        assert_eq!(push.queued.unwrap().land_beat, 8.0);
        push.push(HarmonyMove::FifthUp, KEY, am(), 7.0);
        // Two fifths up from A minor: E minor, then B (diminished in A minor).
        assert_eq!(
            target(&push),
            Some(PushTarget::Chord(Triad::new(11, TriadQuality::Diminished)))
        );
        assert_eq!(
            push.queued.unwrap().land_beat,
            8.0,
            "composition keeps the bar"
        );
        assert!(!push.is_due(7.99));
        assert!(push.is_due(8.0));
    }

    #[test]
    fn landing_voices_from_the_sounding_notes_and_keeps_the_held_slot() {
        let mut push = PushedHarmony::default();
        push.push(HarmonyMove::Relative, KEY, am(), 0.5);
        push.land([45, 48, 52, 57], 3);
        let held = push.held.unwrap();
        assert_eq!((held.slot, held.triad.name(0)), (3, "C".to_string()));
        assert_eq!(held.notes, [43, 48, 52, 55]);
        assert_eq!(push.queued, None);
        push.push(HarmonyMove::Parallel, KEY, am(), 4.5);
        push.land(held.notes, 6);
        assert_eq!(push.held.unwrap().slot, 3, "a held chord keeps its slot");
        assert_eq!(push.held.unwrap().triad.name(0), "Cm");
    }

    #[test]
    fn undo_walks_back_through_queued_and_landed_moves() {
        let mut push = PushedHarmony::default();
        push.push(HarmonyMove::Relative, KEY, am(), 0.5);
        assert_eq!(push.undo(1.0), PushOutcome::Cancelled);
        assert_eq!(push.queued, None);
        assert_eq!(push.undo(1.0), PushOutcome::Nothing);

        push.push(HarmonyMove::Relative, KEY, am(), 0.5);
        push.land([45, 48, 52, 57], 0);
        push.push(HarmonyMove::FifthUp, KEY, am(), 4.5);
        push.land(push.held.unwrap().notes, 0);
        // Back to C, then back to the progression.
        assert_eq!(
            push.undo(9.0),
            PushOutcome::Queued(PushTarget::Chord(Triad::new(0, TriadQuality::Major)))
        );
        assert_eq!(push.queued.unwrap().land_beat, 12.0);
        push.land(push.held.unwrap().notes, 0);
        assert_eq!(push.undo(13.0), PushOutcome::Queued(PushTarget::Release));
        push.land([0; 4], 0);
        assert_eq!(push.held, None);
        assert!(!push.is_active());
    }

    #[test]
    fn undo_history_is_bounded() {
        let mut push = PushedHarmony::default();
        for _ in 0..PUSH_UNDO_DEPTH + 5 {
            push.push(HarmonyMove::FifthUp, KEY, am(), 0.0);
        }
        let mut undone = 0;
        while push.undo(0.0) != PushOutcome::Nothing {
            undone += 1;
        }
        assert_eq!(undone, PUSH_UNDO_DEPTH);
    }

    #[test]
    fn release_hands_back_only_when_pushed() {
        let mut push = PushedHarmony::default();
        assert_eq!(push.release(1.0), PushOutcome::Nothing);
        push.push(HarmonyMove::Home, KEY, am(), 1.0);
        assert_eq!(push.release(1.0), PushOutcome::Queued(PushTarget::Release));
        assert_eq!(
            push.base(am()),
            am(),
            "a queued release reads the progression"
        );
    }

    #[test]
    fn a_restart_lands_a_waiting_move_on_bar_one() {
        let mut push = PushedHarmony::default();
        push.push(HarmonyMove::Home, KEY, am(), 30.0);
        assert_eq!(push.restarted().queued.unwrap().land_beat, 0.0);
    }
}
