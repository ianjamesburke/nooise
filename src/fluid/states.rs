//! An ordered bank of numbered, independently encoded song snapshots.
use super::*;
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};

pub(crate) const STATE_COUNT: usize = 9;
/// The slot names, in bank order. Also the names in a bank file.
pub(crate) const STATE_NAMES: [&str; STATE_COUNT] = ["1", "2", "3", "4", "5", "6", "7", "8", "9"];

#[derive(Clone, Debug)]
pub(crate) enum StateError {
    Io(Arc<std::io::Error>),
    InvalidBank(&'static str),
    Song(song::SongCodeError),
    Empty,
    NoPrevious,
    Stopped,
}
impl PartialEq for StateError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Io(a), Self::Io(b)) => a.kind() == b.kind() && a.to_string() == b.to_string(),
            (Self::InvalidBank(a), Self::InvalidBank(b)) => a == b,
            (Self::Song(a), Self::Song(b)) => a == b,
            (Self::Empty, Self::Empty)
            | (Self::NoPrevious, Self::NoPrevious)
            | (Self::Stopped, Self::Stopped) => true,
            _ => false,
        }
    }
}
impl std::error::Error for StateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Song(error) => Some(error),
            Self::Io(error) => Some(error.as_ref()),
            _ => None,
        }
    }
}
impl From<std::io::Error> for StateError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(Arc::new(error))
    }
}
impl fmt::Display for StateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::InvalidBank(e) => write!(f, "{e}"),
            Self::Song(e) => write!(f, "{e}"),
            Self::Empty => write!(f, "empty state"),
            Self::NoPrevious => write!(f, "no previous state"),
            Self::Stopped => write!(f, "start playback before queuing a state"),
        }
    }
}

pub(crate) struct StateBank {
    path: Option<PathBuf>,
    codes: [Option<String>; STATE_COUNT],
}
impl StateBank {
    pub(crate) fn open(path: Option<PathBuf>) -> Result<Self, StateError> {
        let mut codes: [Option<String>; STATE_COUNT] = Default::default();
        if let Some(path) = &path {
            match std::fs::read_to_string(path) {
                Ok(contents) => {
                    for line in contents.lines() {
                        let (name, code) = line
                            .split_once('\t')
                            .ok_or(StateError::InvalidBank("invalid state bank"))?;
                        let index = STATE_NAMES.iter().position(|item| *item == name).ok_or(
                            StateError::InvalidBank(
                                "unknown state name in bank; states are now numbered 1 to 9",
                            ),
                        )?;
                        if codes[index].is_some() {
                            return Err(StateError::InvalidBank("duplicate state name"));
                        }
                        decode_song_code(code).map_err(StateError::Song)?;
                        codes[index] = Some(code.to_owned());
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(StateError::from(error)),
            }
        }
        Ok(Self { path, codes })
    }
    pub(crate) fn song(&self, index: usize) -> Result<SongState, StateError> {
        let code = self
            .codes
            .get(index)
            .and_then(Option::as_deref)
            .ok_or(StateError::Empty)?;
        decode_song_code(code).map_err(StateError::Song)
    }
    pub(crate) fn next(&self, current: Option<usize>) -> Option<usize> {
        let start = current.map_or(0, |index| (index + 1) % STATE_NAMES.len());
        (0..STATE_NAMES.len())
            .map(|offset| (start + offset) % STATE_NAMES.len())
            .find(|&index| self.codes[index].is_some())
    }
    /// Which slots hold a song, in bank order.
    pub(crate) fn filled(&self) -> [bool; STATE_COUNT] {
        std::array::from_fn(|index| self.codes[index].is_some())
    }
    /// Keep `song` in slot `index`, overwriting what was there. Returns
    /// whether a stored song was replaced.
    pub(crate) fn store(&mut self, index: usize, song: &SongState) -> Result<bool, StateError> {
        let replaced = self
            .codes
            .get(index)
            .ok_or(StateError::InvalidBank("unknown state"))?
            .is_some();
        let code = encode_song_code(song).map_err(StateError::Song)?;
        let mut next = self.codes.clone();
        next[index] = Some(code);
        if let Some(path) = &self.path {
            write_bank(path, &next)?;
        }
        self.codes = next;
        Ok(replaced)
    }
}
fn write_bank(path: &Path, codes: &[Option<String>; STATE_COUNT]) -> Result<(), StateError> {
    let mut text = String::new();
    for (name, code) in STATE_NAMES.iter().zip(codes) {
        if let Some(code) = code {
            text.push_str(name);
            text.push('\t');
            text.push_str(code);
            text.push('\n');
        }
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    for attempt in 0..1000 {
        let temporary = parent.join(format!(".nooise-set-{}-{attempt}.tmp", std::process::id()));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(mut file) => {
                let result = file
                    .write_all(text.as_bytes())
                    .and_then(|_| file.sync_all())
                    .and_then(|_| std::fs::rename(&temporary, path));
                if result.is_err() {
                    let _ = std::fs::remove_file(&temporary);
                }
                return result.map_err(StateError::from);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(StateError::from(error)),
        }
    }
    Err(StateError::InvalidBank(
        "cannot allocate temporary set file",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn temp_bank_path(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("nooise-{name}-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }
    fn song_at(bpm: f32) -> SongState {
        let mut song = SongState::default();
        song.controls.master.bpm = bpm;
        song
    }
    #[test]
    fn bank_has_nine_numbered_slots_that_round_trip_through_a_file() {
        assert_eq!(STATE_NAMES, ["1", "2", "3", "4", "5", "6", "7", "8", "9"]);
        let path = temp_bank_path("states-nine");
        let mut bank = StateBank::open(Some(path.clone())).unwrap();
        assert_eq!(bank.filled(), [false; STATE_COUNT]);
        for index in 0..STATE_COUNT {
            assert!(!bank.store(index, &song_at(60.0 + index as f32)).unwrap());
        }
        assert_eq!(bank.filled(), [true; STATE_COUNT]);
        let reopened = StateBank::open(Some(path.clone())).unwrap();
        for index in 0..STATE_COUNT {
            assert_eq!(
                reopened.song(index).unwrap().controls.master.bpm,
                60.0 + index as f32
            );
        }
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("1\t") && text.contains("\n9\t"), "{text}");
        assert!(matches!(
            bank.store(9, &song_at(80.0)),
            Err(StateError::InvalidBank(_))
        ));
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn store_overwrites_in_place_and_reports_the_replacement() {
        let path = temp_bank_path("states-overwrite");
        let mut bank = StateBank::open(Some(path.clone())).unwrap();
        assert_eq!(bank.store(2, &song_at(130.0)), Ok(false));
        assert_eq!(bank.store(2, &song_at(90.0)), Ok(true));
        assert_eq!(bank.song(2).unwrap().controls.master.bpm, 90.0);
        let reopened = StateBank::open(Some(path.clone())).unwrap();
        assert_eq!(reopened.song(2).unwrap().controls.master.bpm, 90.0);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn next_skips_empty_slots_and_wraps_in_number_order() {
        let mut bank = StateBank::open(None).unwrap();
        assert_eq!(bank.next(None), None);
        for index in [1, 4, 8] {
            bank.store(index, &song_at(100.0)).unwrap();
        }
        assert_eq!(bank.next(None), Some(1));
        assert_eq!(bank.next(Some(1)), Some(4));
        assert_eq!(bank.next(Some(4)), Some(8));
        assert_eq!(bank.next(Some(8)), Some(1));
        assert_eq!(bank.next(Some(6)), Some(8));
    }
    #[test]
    fn lettered_bank_is_refused_and_numbered_wording_is_clear() {
        let path = temp_bank_path("states-letters");
        let mut bank = StateBank::open(Some(path.clone())).unwrap();
        bank.store(0, &song_at(90.0)).unwrap();
        let code = std::fs::read_to_string(&path)
            .unwrap()
            .split_once('\t')
            .unwrap()
            .1
            .trim_end()
            .to_owned();
        std::fs::write(&path, format!("A\t{code}\n")).unwrap();
        let Err(StateError::InvalidBank(message)) = StateBank::open(Some(path.clone())) else {
            panic!("a lettered bank must be refused");
        };
        assert!(message.contains("numbered 1 to 9"), "{message}");
        // The bank already in memory keeps working after the refusal.
        assert_eq!(bank.song(0).unwrap().controls.master.bpm, 90.0);
        std::fs::write(&path, "1\tinvalid\n").unwrap();
        assert!(StateBank::open(Some(path.clone())).is_err());
        std::fs::write(&path, format!("1\t{code}\n1\t{code}\n")).unwrap();
        assert!(matches!(
            StateBank::open(Some(path.clone())),
            Err(StateError::InvalidBank("duplicate state name"))
        ));
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn reopened_song_lands_with_tonal_evolution_and_module_motion() {
        let path =
            std::env::temp_dir().join(format!("nooise-state-evolution-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut song = SongState::default();
        song.controls.modules.pad[0] = preset_slot("delay", 0.8);
        let address = ControlAddress::new("pad.slot1.time");
        let clip = CaptureClip {
            duration: MotionDuration::Beats4,
            data: MotionData::Relative {
                reference: 32768,
                samples: std::array::from_fn(|index| if index < 32 { 42 } else { 0 }),
            },
            origin: -1.0,
            launch: 0.0,
            enabled: true,
        };
        song.automation.captures.insert(address, clip.clone());
        let mut tonal = TonalSequenceState::from_controls(&song.controls.tonal);
        tonal.evolution_seed = 12345;
        tonal.evolution_count = 3;
        song.tonal_sequence = Some(tonal.clone());
        let mut bank = StateBank::open(Some(path.clone())).unwrap();
        bank.store(0, &song).unwrap();
        let restored = StateBank::open(Some(path.clone()))
            .unwrap()
            .song(0)
            .unwrap();
        let session = LiveSession::new(LiveSessionSnapshot::from_song(&SongState::default()));
        let pending = PendingState {
            index: 0,
            snapshot: Arc::new(LiveSessionSnapshot::from_song(&restored)),
        };
        session.update(|snapshot| snapshot.pending_state = Some(pending.clone()));
        assert!(session.land_state(&pending, 0.0));
        assert_eq!(
            session.load().automation.captures.get(&address),
            Some(&clip)
        );
        assert_eq!(session.load().tonal_sequence, tonal);
        std::fs::remove_file(path).unwrap();
    }
}

#[cfg(test)]
mod input_tests {
    use super::*;
    use crate::fluid::interaction::{InteractionMode, InteractionModel, PerformanceMode};
    use crate::fluid::runtime::{TerminalCapabilities, normalize_key_event};
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

    #[derive(Default)]
    struct ClipboardSink;
    impl Clipboard for ClipboardSink {
        fn set_text(&mut self, _: String) -> Result<(), ClipboardError> {
            Ok(())
        }
    }

    type Outcome = Vec<Result<EffectAcknowledgement, EffectFailure>>;

    /// The production key path end to end: key event, mapper, kernel, effect
    /// executor, live session and a real engine to land states on.
    struct Rig {
        session: LiveSession,
        effects: EffectExecutor,
        engine: FluidEngine,
        model: InteractionModel,
        flipped: FlippedUnits,
        clipboard: ClipboardSink,
        caps: TerminalCapabilities,
    }
    impl Rig {
        fn new(caps: TerminalCapabilities) -> Self {
            // A short phrase (four 2-beat chords) keeps landings quick to play.
            let mut controls = FluidControls::default();
            controls.pad.chord_bars = 0.5;
            controls.pad.chord_count = 4.0;
            let session = LiveSession::new(LiveSessionSnapshot::from_controls(controls));
            let morph = no_morph();
            Self {
                effects: EffectExecutor::seeded(
                    session.clone(),
                    AutoControls::new(morph.clone(), vec![], 64),
                    1,
                ),
                engine: FluidEngine::new(
                    8000.0,
                    session.clone(),
                    morph,
                    Arc::new(FluidTelemetry::default()),
                ),
                session,
                model: InteractionModel::default(),
                flipped: FlippedUnits::default(),
                clipboard: ClipboardSink,
                caps,
            }
        }
        fn send(&mut self, code: KeyCode, modifiers: KeyModifiers, kind: KeyEventKind) -> Outcome {
            let Some(event) =
                normalize_key_event(KeyEvent::new_with_kind(code, modifiers, kind), self.caps)
            else {
                return Vec::new();
            };
            let step = coordinate_production_event(
                &mut self.model,
                &event,
                &mut ProductionCoordinatorContext {
                    effects: &mut self.effects,
                    flipped: &mut self.flipped,
                    clipboard: &mut self.clipboard,
                    capabilities: self.caps,
                    beat: self.engine.tempo.beat,
                },
            );
            step.actions
                .into_iter()
                .flat_map(|action| action.effects)
                .map(|record| record.result)
                .collect()
        }
        fn tap(&mut self, key: char) -> Outcome {
            self.send(KeyCode::Char(key), KeyModifiers::NONE, KeyEventKind::Press)
        }
        fn release(&mut self, key: char) -> Outcome {
            self.send(
                KeyCode::Char(key),
                KeyModifiers::NONE,
                KeyEventKind::Release,
            )
        }
        fn esc(&mut self) -> Outcome {
            self.send(KeyCode::Esc, KeyModifiers::NONE, KeyEventKind::Press)
        }
        fn ok(outcome: Outcome) {
            for result in outcome {
                assert!(result.is_ok(), "{result:?}");
            }
        }
        fn command(&mut self, query: &str) {
            Self::ok(self.tap('/'));
            for character in query.chars() {
                Self::ok(self.tap(character));
            }
            Self::ok(self.send(KeyCode::Enter, KeyModifiers::NONE, KeyEventKind::Press));
        }
        fn browsing(&self) -> bool {
            matches!(self.model.mode, InteractionMode::Browsing)
        }
        fn message(&self) -> Option<&str> {
            self.effects.message()
        }
        fn pending(&self) -> Option<usize> {
            self.session.load().pending_state.as_ref().map(|p| p.index)
        }
        /// Run audio until the queued state lands.
        fn land(&mut self) {
            for _ in 0..600_000 {
                let sample = self.engine.next_stereo();
                assert!(sample.0.is_finite() && sample.1.is_finite());
                if self.session.load().pending_state.is_none() {
                    return;
                }
            }
            panic!("the queued state never landed");
        }
        fn settle(&mut self) {
            for _ in 0..128 {
                self.engine.next_stereo();
            }
        }
        /// Store a song whose tempo is `bpm` into slot `index` (1-based)
        /// through the palette.
        fn store_with_bpm(&mut self, index: usize, bpm: f32) {
            self.session
                .update(|snapshot| snapshot.controls.master.bpm = bpm);
            self.settle();
            self.command(&format!("store state {index}"));
        }
        fn fill_three(&mut self) {
            for (index, bpm) in [(1, 90.0), (2, 130.0), (3, 110.0)] {
                self.store_with_bpm(index, bpm);
            }
        }
        fn submerge_held(&self) -> bool {
            self.session
                .load()
                .gestures
                .envelope(GestureKind::Submerge)
                .held
        }
    }

    #[test]
    fn slash_store_queue_and_cancel_reach_live_session() {
        let mut rig = Rig::new(TerminalCapabilities::full());
        for query in ["store state 1", "queue state 1", "cancel state queue"] {
            rig.command(query);
        }
        assert!(rig.session.load().pending_state.is_none());
    }

    #[test]
    fn production_palette_plays_1_2_3_1_at_next_bar() {
        let mut rig = Rig::new(TerminalCapabilities::full());
        rig.fill_three();
        rig.command("queue state 2");
        rig.command("cancel state queue");
        assert!(rig.pending().is_none());
        rig.command("queue state 3");
        for (index, bpm) in [(1, 90.0), (2, 130.0), (3, 110.0), (1, 90.0)] {
            rig.command(&format!("queue state {index}"));
            rig.land();
            assert_eq!(rig.session.load().current_state, Some(index - 1));
            assert!((rig.engine.tempo.bpm - bpm).abs() < 0.001);
        }
    }

    #[test]
    fn palette_stores_and_queues_every_numbered_slot_and_has_no_replace() {
        let mut rig = Rig::new(TerminalCapabilities::full());
        for index in 1..=9 {
            rig.command(&format!("store state {index}"));
            assert_eq!(
                rig.message(),
                Some(format!("Stored state {index}").as_str())
            );
        }
        assert_eq!(rig.effects.filled_states(), [true; STATE_COUNT]);
        rig.command("store state 3");
        assert_eq!(rig.message(), Some("Replaced state 3"));
        for index in 1..=9 {
            rig.command(&format!("queue state {index}"));
            assert_eq!(rig.pending(), Some(index - 1));
        }
        for operation in Operation::ALL {
            let label = operation.spec().label;
            assert!(!label.contains("Replace"), "{label}");
            assert!(!label.contains(" A") && !label.ends_with(" B"), "{label}");
        }
    }

    #[test]
    fn palette_queue_next_follows_number_order_and_previous_goes_back() {
        let mut rig = Rig::new(TerminalCapabilities::full());
        rig.store_with_bpm(2, 90.0);
        rig.store_with_bpm(5, 130.0);
        rig.command("queue next state");
        assert_eq!(rig.pending(), Some(1));
        rig.land();
        rig.command("queue next state");
        assert_eq!(rig.pending(), Some(4));
        rig.land();
        rig.command("queue previous state");
        assert_eq!(rig.pending(), Some(1));
        rig.land();
        assert_eq!(rig.session.load().previous_state, Some(4));
    }

    #[test]
    fn queue_leader_digit_queues_and_the_same_digit_cancels() {
        let mut rig = Rig::new(TerminalCapabilities::full());
        rig.fill_three();
        Rig::ok(rig.tap('\''));
        assert!(matches!(
            rig.model.mode,
            InteractionMode::Performance(PerformanceMode::States { .. })
        ));
        Rig::ok(rig.tap('2'));
        assert!(rig.browsing());
        assert_eq!(rig.pending(), Some(1));
        assert_eq!(rig.message(), Some("Queued state 2 for phrase end"));
        Rig::ok(rig.tap('\''));
        Rig::ok(rig.tap('3'));
        assert_eq!(rig.pending(), Some(2), "another digit replaces the queue");
        Rig::ok(rig.tap('\''));
        Rig::ok(rig.tap('3'));
        assert!(rig.pending().is_none());
        assert_eq!(rig.message(), Some("State queue cancelled"));
    }

    #[test]
    fn queue_leader_reports_empty_slot_and_stopped_clock_then_browses() {
        let mut rig = Rig::new(TerminalCapabilities::full());
        rig.fill_three();
        Rig::ok(rig.tap('\''));
        let outcome = rig.tap('7');
        assert!(
            matches!(&outcome[..], [Err(EffectFailure::State(StateError::Empty))]),
            "{outcome:?}"
        );
        assert!(rig.browsing());
        rig.session
            .update(|snapshot| snapshot.transport = Transport::Stopped);
        Rig::ok(rig.tap('\''));
        let outcome = rig.tap('1');
        assert!(
            matches!(
                &outcome[..],
                [Err(EffectFailure::State(StateError::Stopped))]
            ),
            "{outcome:?}"
        );
        assert!(rig.browsing());
        assert!(rig.pending().is_none());
    }

    #[test]
    fn double_apostrophe_queues_the_state_before_the_last_landing() {
        let mut rig = Rig::new(TerminalCapabilities::full());
        rig.fill_three();
        Rig::ok(rig.tap('\''));
        let outcome = rig.tap('\'');
        assert!(
            matches!(
                &outcome[..],
                [Err(EffectFailure::State(StateError::NoPrevious))]
            ),
            "{outcome:?}"
        );
        assert!(rig.browsing());
        rig.command("queue state 1");
        rig.land();
        Rig::ok(rig.tap('\''));
        assert!(matches!(
            &rig.tap('\'')[..],
            [Err(EffectFailure::State(StateError::NoPrevious))]
        ));
        rig.command("queue state 3");
        rig.land();
        for expected in [0, 2, 0] {
            Rig::ok(rig.tap('\''));
            Rig::ok(rig.tap('\''));
            assert!(rig.browsing());
            assert_eq!(rig.pending(), Some(expected));
            rig.land();
        }
    }

    #[test]
    fn store_leader_overwrites_at_once_and_says_which() {
        let mut rig = Rig::new(TerminalCapabilities::full());
        Rig::ok(rig.tap('"'));
        Rig::ok(rig.tap('3'));
        assert!(rig.browsing());
        assert_eq!(rig.message(), Some("Stored state 3"));
        assert!(rig.effects.filled_states()[2]);
        Rig::ok(rig.tap('"'));
        Rig::ok(rig.tap('3'));
        assert_eq!(rig.message(), Some("Replaced state 3"));
        assert_eq!(
            rig.effects.filled_states().iter().filter(|f| **f).count(),
            1
        );
    }

    #[test]
    fn escape_cancels_either_leader_with_no_effect() {
        let mut rig = Rig::new(TerminalCapabilities::full());
        rig.store_with_bpm(1, 90.0);
        for opener in ['\'', '"'] {
            let before = rig.effects.filled_states();
            Rig::ok(rig.tap(opener));
            assert!(!rig.browsing());
            assert!(rig.esc().is_empty());
            assert!(rig.browsing());
            assert_eq!(rig.effects.filled_states(), before);
            assert!(rig.pending().is_none());
        }
    }

    #[test]
    fn leaders_are_ordinary_characters_in_text_entry_and_inert_in_editors() {
        let mut rig = Rig::new(TerminalCapabilities::full());
        Rig::ok(rig.tap('/'));
        Rig::ok(rig.tap('\''));
        Rig::ok(rig.tap('"'));
        let InteractionMode::Palette(palette) = &rig.model.mode else {
            panic!("the palette must keep the keyboard");
        };
        assert_eq!(palette.query, "'\"");
        Rig::ok(rig.esc());
        Rig::ok(rig.tap('5'));
        assert!(matches!(rig.model.mode, InteractionMode::Numeric(_)));
        Rig::ok(rig.tap('\''));
        Rig::ok(rig.tap('"'));
        assert!(matches!(rig.model.mode, InteractionMode::Numeric(_)));
        Rig::ok(rig.esc());
        Rig::ok(rig.tap('f'));
        assert!(matches!(rig.model.mode, InteractionMode::Automation(_)));
        Rig::ok(rig.tap('\''));
        Rig::ok(rig.tap('"'));
        assert!(matches!(rig.model.mode, InteractionMode::Automation(_)));
        Rig::ok(rig.esc());
        Rig::ok(rig.tap('i'));
        assert!(matches!(rig.model.mode, InteractionMode::Lead(_)));
        Rig::ok(rig.tap('\''));
        assert!(matches!(rig.model.mode, InteractionMode::Lead(_)));
    }

    #[test]
    fn every_reported_form_of_the_double_quote_opens_the_store_leader() {
        for (code, modifiers) in [
            ('"', KeyModifiers::NONE),
            ('"', KeyModifiers::SHIFT),
            ('\'', KeyModifiers::SHIFT),
        ] {
            for caps in [
                TerminalCapabilities::full(),
                TerminalCapabilities::default(),
            ] {
                let mut rig = Rig::new(caps);
                Rig::ok(rig.send(KeyCode::Char(code), modifiers, KeyEventKind::Press));
                Rig::ok(rig.tap('4'));
                assert!(rig.browsing(), "{code:?} {modifiers:?}");
                assert_eq!(
                    rig.message(),
                    Some("Stored state 4"),
                    "{code:?} {modifiers:?}"
                );
            }
        }
    }

    /// A hold must come through opening a state leader, completing it with
    /// a digit, and cancelling it, and a key-up inside the leader releases it.
    #[test]
    fn a_physical_gesture_hold_survives_both_state_leaders() {
        for opener in ['\'', '"'] {
            for finish in ["digit", "escape", "keyup"] {
                let mut rig = Rig::new(TerminalCapabilities::full());
                rig.store_with_bpm(1, 90.0);
                Rig::ok(rig.tap('w'));
                assert!(rig.submerge_held());
                Rig::ok(rig.tap(opener));
                assert!(!rig.browsing());
                assert!(rig.submerge_held(), "{opener} opening dropped the hold");
                match finish {
                    "digit" => {
                        Rig::ok(rig.tap('1'));
                        assert!(rig.browsing());
                        assert!(rig.submerge_held(), "{opener} digit dropped the hold");
                        Rig::ok(rig.release('w'));
                        assert!(!rig.submerge_held());
                    }
                    "escape" => {
                        rig.esc();
                        assert!(rig.browsing());
                        assert!(rig.submerge_held(), "{opener} Esc dropped the hold");
                        Rig::ok(rig.release('w'));
                        assert!(!rig.submerge_held());
                    }
                    _ => {
                        Rig::ok(rig.release('w'));
                        assert!(!rig.submerge_held(), "key-up inside {opener} leader");
                        assert!(!rig.browsing(), "key-up must not close the leader");
                        rig.esc();
                    }
                }
            }
        }
    }

    #[test]
    fn a_held_hold_mute_survives_the_leader() {
        let mut rig = Rig::new(TerminalCapabilities::full());
        rig.store_with_bpm(1, 90.0);
        Rig::ok(rig.tap('q'));
        assert!(rig.session.load().hold_muted[HoldMute::KickAndBass as usize]);
        Rig::ok(rig.tap('\''));
        Rig::ok(rig.tap('1'));
        assert!(rig.session.load().hold_muted[HoldMute::KickAndBass as usize]);
        Rig::ok(rig.release('q'));
        assert!(!rig.session.load().hold_muted[HoldMute::KickAndBass as usize]);
    }

    #[test]
    fn a_held_vamp_locks_the_destination_first_chord_through_the_leader() {
        let mut rig = Rig::new(TerminalCapabilities::full());
        rig.session
            .update(|snapshot| snapshot.controls.pad.chord_offset = 3.0);
        rig.store_with_bpm(2, 100.0);
        rig.session
            .update(|snapshot| snapshot.controls.pad.chord_offset = 0.0);
        Rig::ok(rig.tap('a'));
        assert!(rig.session.load().vamp_press_seconds.is_some());
        Rig::ok(rig.tap('\''));
        Rig::ok(rig.tap('2'));
        let queued = rig.session.load().pending_state.clone().unwrap();
        // The palette queues the identical destination at the identical bar.
        let mut palette = Rig::new(TerminalCapabilities::full());
        palette.store_with_bpm(2, 100.0);
        palette.command("queue state 2");
        let via_palette = palette.session.load().pending_state.clone().unwrap();
        assert_eq!(queued.index, via_palette.index);
        rig.land();
        let landed = rig.session.load();
        assert_eq!(landed.vamp, Some(VampState::start(0.0, 3)));
        assert!(landed.vamp_press_seconds.is_some());
        assert!(!landed.vamp_latched);
        drop(landed);
        Rig::ok(rig.release('a'));
    }

    /// A hold restored from a song code has no physical latch; the
    /// coordinator's own modal-entry release must spare it too. Jump, which
    /// keeps today's behaviour, still returns it.
    #[test]
    fn restored_holds_survive_state_leaders_but_not_the_palette() {
        let restore = |rig: &Rig| {
            rig.session.update(|snapshot| {
                let lane = &mut snapshot.gestures.lanes[GestureKind::Submerge as usize];
                lane.held = true;
                lane.restored = true;
            });
        };
        for opener in ['\'', '"'] {
            let mut rig = Rig::new(TerminalCapabilities::full());
            restore(&rig);
            Rig::ok(rig.tap(opener));
            assert!(rig.submerge_held(), "{opener}");
        }
        let mut rig = Rig::new(TerminalCapabilities::full());
        restore(&rig);
        Rig::ok(rig.tap('/'));
        assert!(
            !rig.submerge_held(),
            "the palette must keep releasing holds"
        );
        let mut rig = Rig::new(TerminalCapabilities::full());
        Rig::ok(rig.tap('w'));
        Rig::ok(rig.tap('/'));
        assert!(
            !rig.submerge_held(),
            "the palette must keep releasing holds"
        );
    }

    #[test]
    fn latched_toggles_survive_the_leader_on_terminals_without_key_release() {
        let mut rig = Rig::new(TerminalCapabilities::default());
        rig.store_with_bpm(1, 90.0);
        Rig::ok(rig.tap('q'));
        Rig::ok(rig.tap('a'));
        let before = rig.session.load();
        assert!(before.kick_bass_latched && before.vamp.is_some());
        drop(before);
        for opener in ['\'', '"'] {
            Rig::ok(rig.tap(opener));
            Rig::ok(rig.tap('1'));
            let after = rig.session.load();
            assert!(after.kick_bass_latched, "{opener}");
            assert!(after.vamp.is_some(), "{opener}");
        }
    }
}
