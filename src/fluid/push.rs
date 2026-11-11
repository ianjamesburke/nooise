//! Push the Harmony's live state: the chord the harmony has been pushed to,
//! a move waiting to land, and a bounded undo history.
//!
//! The harmony model itself (`Triad`, `HarmonyMove`, `voice_lead`) is pure and
//! lives in `voice/triad.rs`. This module only decides *when* a chord sounds:
//! a move queues, composes with any move already queued, and lands at once or
//! on the quantize line, where the audio clock voices it from the notes
//! sounding at that moment. There is no way back to the authored progression:
//! once pushed, the home row is the instrument.

use super::*;

/// How many moves Backspace can walk back. Live-only, never in a song code.
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct QueuedPush {
    /// The chord the move lands on.
    pub(crate) target: Triad,
    /// The beat it lands on: now when unquantized, else a bar line.
    pub(crate) land_beat: f64,
}

/// Where moves land: at once (`Off`, the default, for playing chords live)
/// or on the next 1, 2 or 4 bar line. A play-style preference of the player,
/// not part of the song: live-only, never in a song code.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PushQuantize {
    #[default]
    Off,
    OneBar,
    TwoBars,
    FourBars,
}

impl PushQuantize {
    pub(crate) const fn bars(self) -> Option<u32> {
        match self {
            Self::Off => None,
            Self::OneBar => Some(1),
            Self::TwoBars => Some(2),
            Self::FourBars => Some(4),
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::OneBar => "1 bar",
            Self::TwoBars => "2 bars",
            Self::FourBars => "4 bars",
        }
    }

    /// The beat a move made at `beat` lands on: `beat` itself when off, else
    /// the next line of this many bars strictly after it.
    pub(crate) fn landing(self, beat: f64) -> f64 {
        match self.bars() {
            None => beat,
            Some(bars) => {
                let grid = f64::from(bars) * 4.0;
                (beat / grid).floor() * grid + grid
            }
        }
    }
}

/// What the harmony was headed to before one move: a pushed triad, or
/// `None` while it still followed the progression (undoing that withdraws a
/// waiting first move; it never hands back a landed one).
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
    /// Live-only, like the undo history.
    pub(crate) quantize: PushQuantize,
    undo: UndoHistory,
}

/// One Push the Harmony key, as the interaction kernel names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HarmonyAction {
    Move(HarmonyMove),
    Undo,
}

/// What one push key did, for the confirmation message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PushOutcome {
    Queued(Triad),
    /// The queued move was withdrawn; the harmony stays where it is.
    Cancelled,
    Nothing,
}

impl PushedHarmony {
    /// A saved song's state: what was held, and a queued target, which lands
    /// on the loaded song's first bar. Undo history is never saved.
    pub(crate) fn restored(held: Option<HeldChord>, queued: Option<Triad>) -> Self {
        Self {
            held,
            queued: queued.map(|target| QueuedPush {
                target,
                land_beat: 0.0,
            }),
            quantize: PushQuantize::default(),
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
        self.heading().unwrap_or(authored)
    }

    /// Where the harmony is headed, as the undo history records it.
    fn heading(&self) -> UndoEntry {
        self.queued
            .map(|queued| queued.target)
            .or(self.held.map(|held| held.triad))
    }

    fn queue(&mut self, target: Triad, beat: f64) {
        let land_beat = self
            .queued
            .map_or_else(|| self.quantize.landing(beat), |queued| queued.land_beat);
        self.queued = Some(QueuedPush { target, land_beat });
    }

    /// Queue `harmony_move` from the current base, landing where `quantize`
    /// says. A second move before it lands composes from the queued chord and
    /// keeps its landing.
    pub(crate) fn push(
        &mut self,
        harmony_move: HarmonyMove,
        key: HomeKey,
        authored: Triad,
        beat: f64,
    ) -> PushOutcome {
        let target = harmony_move.apply(self.base(authored), key);
        self.undo.push(self.heading());
        self.queue(target, beat);
        PushOutcome::Queued(target)
    }

    /// Walk back one move. Returning to where the harmony already sounds
    /// withdraws the queue; another pushed chord is queued again. Nothing
    /// walks back past the first landed push to the authored progression.
    pub(crate) fn undo(&mut self, beat: f64) -> PushOutcome {
        let Some(previous) = self.undo.pop() else {
            return PushOutcome::Nothing;
        };
        if previous == self.held.map(|held| held.triad) {
            self.queued = None;
            return PushOutcome::Cancelled;
        }
        let Some(target) = previous else {
            // The first move already landed: the progression is behind us.
            self.undo = UndoHistory::default();
            return PushOutcome::Nothing;
        };
        self.queue(target, beat);
        PushOutcome::Queued(target)
    }

    pub(crate) fn is_due(&self, beat: f64) -> bool {
        self.queued.is_some_and(|queued| beat >= queued.land_beat)
    }

    /// Land the queued move: voice its chord from `sounding` on `slot` (a
    /// held chord keeps its slot).
    pub(crate) fn land(&mut self, sounding: [i32; 4], slot: usize) {
        let Some(queued) = self.queued.take() else {
            return;
        };
        self.held = Some(HeldChord {
            slot: self.held.map_or(slot as u8, |held| held.slot),
            triad: queued.target,
            notes: voice_lead(queued.target, sounding),
        });
    }

    /// Change where moves land. A waiting move is re-timed to the new
    /// setting from `beat`: off lands it at once, a grid moves it to that
    /// grid's next line.
    pub(crate) fn set_quantize(&mut self, quantize: PushQuantize, beat: f64) {
        self.quantize = quantize;
        if let Some(queued) = &mut self.queued {
            queued.land_beat = quantize.landing(beat);
        }
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

    /// The bar-quantized state the design table's examples use.
    fn bar_push() -> PushedHarmony {
        PushedHarmony {
            quantize: PushQuantize::OneBar,
            ..PushedHarmony::default()
        }
    }

    fn target(push: &PushedHarmony) -> Option<Triad> {
        push.queued.map(|queued| queued.target)
    }

    #[test]
    fn a_move_queues_for_the_next_bar_and_a_second_composes_from_it() {
        let mut push = bar_push();
        push.push(HarmonyMove::FifthUp, KEY, am(), 5.5);
        assert_eq!(push.queued.unwrap().land_beat, 8.0);
        push.push(HarmonyMove::FifthUp, KEY, am(), 7.0);
        // Two fifths up from A minor: E minor, then B (diminished in A minor).
        assert_eq!(
            target(&push),
            Some(Triad::new(11, TriadQuality::Diminished))
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
        let mut push = bar_push();
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
    fn undo_walks_back_through_pushes_but_never_to_the_progression() {
        let mut push = bar_push();
        push.push(HarmonyMove::Relative, KEY, am(), 0.5);
        assert_eq!(push.undo(1.0), PushOutcome::Cancelled);
        assert_eq!(push.queued, None);
        assert_eq!(push.undo(1.0), PushOutcome::Nothing);

        push.push(HarmonyMove::Relative, KEY, am(), 0.5);
        push.land([45, 48, 52, 57], 0);
        push.push(HarmonyMove::FifthUp, KEY, am(), 4.5);
        push.land(push.held.unwrap().notes, 0);
        // Back to C; past that the progression is behind us.
        assert_eq!(
            push.undo(9.0),
            PushOutcome::Queued(Triad::new(0, TriadQuality::Major))
        );
        assert_eq!(push.queued.unwrap().land_beat, 12.0);
        push.land(push.held.unwrap().notes, 0);
        assert_eq!(push.undo(13.0), PushOutcome::Nothing);
        assert_eq!(push.held.unwrap().triad.name(0), "C");
        assert_eq!(push.queued, None);
    }

    #[test]
    fn undo_history_is_bounded() {
        let mut push = bar_push();
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
    fn off_lands_at_once_and_each_grid_lands_on_its_next_line() {
        assert_eq!(PushQuantize::default(), PushQuantize::Off);
        for (quantize, beat, expected) in [
            (PushQuantize::Off, 5.25, 5.25),
            (PushQuantize::OneBar, 5.25, 8.0),
            (PushQuantize::OneBar, 8.0, 12.0),
            (PushQuantize::TwoBars, 5.25, 8.0),
            (PushQuantize::TwoBars, 8.5, 16.0),
            (PushQuantize::FourBars, 5.25, 16.0),
            (PushQuantize::FourBars, 16.0, 32.0),
        ] {
            assert_eq!(quantize.landing(beat), expected, "{quantize:?} at {beat}");
        }
        let mut push = PushedHarmony::default();
        push.push(HarmonyMove::Relative, KEY, am(), 5.25);
        assert!(push.is_due(5.25), "off lands on the next sample");
    }

    #[test]
    fn changing_the_setting_re_times_a_waiting_move() {
        let mut push = bar_push();
        push.push(HarmonyMove::Relative, KEY, am(), 5.0);
        push.set_quantize(PushQuantize::FourBars, 6.0);
        assert_eq!(push.queued.unwrap().land_beat, 16.0);
        push.set_quantize(PushQuantize::Off, 7.0);
        assert!(push.is_due(7.0));
        assert_eq!(target(&push), Some(Triad::new(0, TriadQuality::Major)));
    }

    #[test]
    fn a_restart_lands_a_waiting_move_on_bar_one() {
        let mut push = bar_push();
        push.push(HarmonyMove::Home, KEY, am(), 30.0);
        assert_eq!(push.restarted().queued.unwrap().land_beat, 0.0);
    }
}
