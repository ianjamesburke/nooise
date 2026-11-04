//! An ordered bank of named, independently encoded song snapshots.
use super::*;
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};

pub(crate) const SECTION_NAMES: [&str; 3] = ["A", "B", "C"];

#[derive(Clone, Debug)]
pub(crate) enum SectionError {
    Io(Arc<std::io::Error>),
    InvalidBank(&'static str),
    Song(song::SongCodeError),
    Empty,
    Occupied,
    Stopped,
}
impl PartialEq for SectionError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Io(a), Self::Io(b)) => a.kind() == b.kind() && a.to_string() == b.to_string(),
            (Self::InvalidBank(a), Self::InvalidBank(b)) => a == b,
            (Self::Song(a), Self::Song(b)) => a == b,
            (Self::Empty, Self::Empty)
            | (Self::Occupied, Self::Occupied)
            | (Self::Stopped, Self::Stopped) => true,
            _ => false,
        }
    }
}
impl std::error::Error for SectionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Song(error) => Some(error),
            Self::Io(error) => Some(error.as_ref()),
            _ => None,
        }
    }
}
impl From<std::io::Error> for SectionError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(Arc::new(error))
    }
}
impl fmt::Display for SectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::InvalidBank(e) => write!(f, "{e}"),
            Self::Song(e) => write!(f, "{e}"),
            Self::Empty => write!(f, "empty section"),
            Self::Occupied => write!(f, "section already stored; use Replace section"),
            Self::Stopped => write!(f, "start playback before queuing a section"),
        }
    }
}

pub(crate) struct SectionBank {
    path: Option<PathBuf>,
    codes: [Option<String>; 3],
}
impl SectionBank {
    pub(crate) fn open(path: Option<PathBuf>) -> Result<Self, SectionError> {
        let mut codes: [Option<String>; 3] = Default::default();
        if let Some(path) = &path {
            match std::fs::read_to_string(path) {
                Ok(contents) => {
                    for line in contents.lines() {
                        let (name, code) = line
                            .split_once('\t')
                            .ok_or(SectionError::InvalidBank("invalid section bank"))?;
                        let index = SECTION_NAMES
                            .iter()
                            .position(|item| *item == name)
                            .ok_or(SectionError::InvalidBank("unknown section name"))?;
                        if codes[index].is_some() {
                            return Err(SectionError::InvalidBank("duplicate section name"));
                        }
                        decode_song_code(code).map_err(SectionError::Song)?;
                        codes[index] = Some(code.to_owned());
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(SectionError::from(error)),
            }
        }
        Ok(Self { path, codes })
    }
    pub(crate) fn song(&self, index: usize) -> Result<SongState, SectionError> {
        let code = self
            .codes
            .get(index)
            .and_then(Option::as_deref)
            .ok_or(SectionError::Empty)?;
        decode_song_code(code).map_err(SectionError::Song)
    }
    pub(crate) fn next(&self, current: Option<usize>) -> Option<usize> {
        let start = current.map_or(0, |index| (index + 1) % SECTION_NAMES.len());
        (0..SECTION_NAMES.len())
            .map(|offset| (start + offset) % SECTION_NAMES.len())
            .find(|&index| self.codes[index].is_some())
    }
    pub(crate) fn store(
        &mut self,
        index: usize,
        song: &SongState,
        replace: bool,
    ) -> Result<(), SectionError> {
        if self
            .codes
            .get(index)
            .ok_or(SectionError::InvalidBank("unknown section"))?
            .is_some()
            && !replace
        {
            return Err(SectionError::Occupied);
        }
        let code = encode_song_code(song).map_err(SectionError::Song)?;
        let mut next = self.codes.clone();
        next[index] = Some(code);
        if let Some(path) = &self.path {
            write_bank(path, &next)?;
        }
        self.codes = next;
        Ok(())
    }
}
fn write_bank(path: &Path, codes: &[Option<String>; 3]) -> Result<(), SectionError> {
    let mut text = String::new();
    for (name, code) in SECTION_NAMES.iter().zip(codes) {
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
                return result.map_err(SectionError::from);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(SectionError::from(error)),
        }
    }
    Err(SectionError::InvalidBank(
        "cannot allocate temporary set file",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bank_round_trip_refuses_overwrite_and_invalid_code() {
        let path =
            std::env::temp_dir().join(format!("nooise-sections-test-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut bank = SectionBank::open(Some(path.clone())).unwrap();
        let mut song = SongState::default();
        song.controls.master.bpm = 130.0;
        bank.store(0, &song, false).unwrap();
        assert!(matches!(
            bank.store(0, &song, false),
            Err(SectionError::Occupied)
        ));
        song.controls.master.bpm = 90.0;
        bank.store(0, &song, true).unwrap();
        let reopened = SectionBank::open(Some(path.clone())).unwrap();
        assert_eq!(reopened.song(0).unwrap().controls.master.bpm, 90.0);
        std::fs::write(&path, "A\tinvalid\n").unwrap();
        assert!(SectionBank::open(Some(path.clone())).is_err());
        assert_eq!(reopened.song(0).unwrap().controls.master.bpm, 90.0);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn reopened_song_lands_with_tonal_evolution_and_module_motion() {
        let path =
            std::env::temp_dir().join(format!("nooise-section-evolution-{}", std::process::id()));
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
        let mut bank = SectionBank::open(Some(path.clone())).unwrap();
        bank.store(0, &song, false).unwrap();
        let restored = SectionBank::open(Some(path.clone()))
            .unwrap()
            .song(0)
            .unwrap();
        let session = LiveSession::new(LiveSessionSnapshot::from_song(&SongState::default()));
        let pending = PendingSection {
            index: 0,
            target_beat: 4.0,
            snapshot: Arc::new(LiveSessionSnapshot::from_song(&restored)),
        };
        session.update(|snapshot| snapshot.pending_section = Some(pending.clone()));
        assert!(session.land_section(&pending, 0.0));
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
    use crate::fluid::interaction::InteractionModel;
    use crate::fluid::runtime::{TerminalCapabilities, normalize_key_event};
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

    #[derive(Default)]
    struct ClipboardSink;
    impl Clipboard for ClipboardSink {
        fn set_text(&mut self, _: String) -> Result<(), ClipboardError> {
            Ok(())
        }
    }
    #[test]
    fn slash_store_queue_replace_and_cancel_reach_live_session() {
        let session =
            LiveSession::new(LiveSessionSnapshot::from_controls(FluidControls::default()));
        let mut effects = EffectExecutor::seeded(
            session.clone(),
            AutoControls::new(no_morph(), vec![], 64),
            1,
        );
        let mut model = InteractionModel::default();
        let mut flipped = FlippedUnits::default();
        let mut clipboard = ClipboardSink;
        let capabilities = TerminalCapabilities::full();
        let mut input = |key: KeyCode| {
            let event = normalize_key_event(
                KeyEvent::new_with_kind(key, KeyModifiers::NONE, KeyEventKind::Press),
                capabilities,
            )
            .unwrap();
            let step = coordinate_production_event(
                &mut model,
                &event,
                &mut ProductionCoordinatorContext {
                    effects: &mut effects,
                    flipped: &mut flipped,
                    clipboard: &mut clipboard,
                    capabilities,
                    beat: 0.0,
                },
            );
            for action in step.actions {
                for result in action.effects {
                    assert!(result.result.is_ok(), "{:?}", result.result);
                }
            }
        };
        for query in ["store section a", "queue section a", "cancel section queue"] {
            input(KeyCode::Char('/'));
            for character in query.chars() {
                input(KeyCode::Char(character));
            }
            input(KeyCode::Enter);
        }
        assert!(session.load().pending_section.is_none());
    }
    #[test]
    fn production_palette_plays_a_b_c_a_at_next_bar() {
        let session =
            LiveSession::new(LiveSessionSnapshot::from_controls(FluidControls::default()));
        let morph = no_morph();
        let mut effects = EffectExecutor::seeded(
            session.clone(),
            AutoControls::new(morph.clone(), vec![], 64),
            1,
        );
        let mut engine = FluidEngine::new(
            8000.0,
            session.clone(),
            morph,
            Arc::new(FluidTelemetry::default()),
        );
        let mut model = InteractionModel::default();
        let mut flipped = FlippedUnits::default();
        let mut clipboard = ClipboardSink;
        let caps = TerminalCapabilities::full();
        fn command(
            query: &str,
            beat: f64,
            model: &mut InteractionModel,
            effects: &mut EffectExecutor,
            flipped: &mut FlippedUnits,
            clipboard: &mut ClipboardSink,
            caps: TerminalCapabilities,
        ) {
            for key in std::iter::once(KeyCode::Char('/'))
                .chain(query.chars().map(KeyCode::Char))
                .chain(std::iter::once(KeyCode::Enter))
            {
                let event = normalize_key_event(
                    KeyEvent::new_with_kind(key, KeyModifiers::NONE, KeyEventKind::Press),
                    caps,
                )
                .unwrap();
                let step = coordinate_production_event(
                    model,
                    &event,
                    &mut ProductionCoordinatorContext {
                        effects,
                        flipped,
                        clipboard,
                        capabilities: caps,
                        beat,
                    },
                );
                for action in step.actions {
                    for result in action.effects {
                        assert!(result.result.is_ok(), "{:?}", result.result);
                    }
                }
            }
        }
        for (index, bpm) in [90.0, 130.0, 110.0].into_iter().enumerate() {
            session.update(|snapshot| snapshot.controls.master.bpm = bpm);
            for _ in 0..128 {
                engine.next_stereo();
            }
            command(
                &format!("store section {}", SECTION_NAMES[index].to_lowercase()),
                engine.tempo.beat,
                &mut model,
                &mut effects,
                &mut flipped,
                &mut clipboard,
                caps,
            );
        }
        command(
            "queue section b",
            engine.tempo.beat,
            &mut model,
            &mut effects,
            &mut flipped,
            &mut clipboard,
            caps,
        );
        command(
            "cancel section queue",
            engine.tempo.beat,
            &mut model,
            &mut effects,
            &mut flipped,
            &mut clipboard,
            caps,
        );
        assert!(session.load().pending_section.is_none());
        command(
            "queue section c",
            engine.tempo.beat,
            &mut model,
            &mut effects,
            &mut flipped,
            &mut clipboard,
            caps,
        );
        for (index, bpm) in [(0, 90.0), (1, 130.0), (2, 110.0), (0, 90.0)] {
            command(
                &format!("queue section {}", SECTION_NAMES[index].to_lowercase()),
                engine.tempo.beat,
                &mut model,
                &mut effects,
                &mut flipped,
                &mut clipboard,
                caps,
            );
            let target = session.load().pending_section.as_ref().unwrap().target_beat;
            assert!((target / 4.0).fract().abs() < 1e-9);
            let mut landed = false;
            for _ in 0..50000 {
                let sample = engine.next_stereo();
                assert!(sample.0.is_finite() && sample.1.is_finite());
                if session.load().pending_section.is_none() {
                    landed = true;
                    break;
                }
            }
            assert!(landed);
            assert_eq!(session.load().current_section, Some(index));
            assert!((engine.tempo.bpm - bpm).abs() < 0.001);
        }
    }
}
