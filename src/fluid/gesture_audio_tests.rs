//! Audio acceptance tests driven through the production terminal input path.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::interaction::{InteractionModel, Navigation, Page};
use super::runtime::{TerminalCapabilities, normalize_key_event};
use super::*;

#[derive(Default)]
struct TestClipboard(String);

impl Clipboard for TestClipboard {
    fn set_text(&mut self, text: String) -> Result<(), ClipboardError> {
        self.0 = text;
        Ok(())
    }
}

struct GesturePlayer {
    engine: FluidEngine,
    effects: EffectExecutor,
    model: InteractionModel,
    fluid: RippleField,
    flipped: FlippedUnits,
    clipboard: TestClipboard,
    capabilities: TerminalCapabilities,
}

impl GesturePlayer {
    fn new(sample_rate: f32) -> Self {
        Self::from_snapshot(
            sample_rate,
            LiveSessionSnapshot::from_controls(FluidControls::default()),
        )
    }

    fn from_snapshot(sample_rate: f32, snapshot: LiveSessionSnapshot) -> Self {
        let session = LiveSession::new(snapshot);
        let morph = no_morph();
        let mut engine = FluidEngine::new(
            sample_rate,
            session.clone(),
            Arc::clone(&morph),
            Arc::new(FluidTelemetry::default()),
        );
        engine.reseed(42);
        Self {
            engine,
            effects: EffectExecutor::seeded(session, AutoControls::new(morph, Vec::new(), 64), 42),
            model: InteractionModel {
                navigation: Navigation::for_page(Page::Master),
                ..InteractionModel::default()
            },
            fluid: RippleField::new(),
            flipped: FlippedUnits::default(),
            clipboard: TestClipboard::default(),
            capabilities: TerminalCapabilities::full(),
        }
    }

    fn key(&mut self, code: KeyCode, phase: KeyEventKind, modifiers: KeyModifiers) {
        let event = normalize_key_event(
            KeyEvent::new_with_kind(code, modifiers, phase),
            self.capabilities,
        )
        .expect("full capabilities report every key event");
        let step = coordinate_production_event(
            &mut self.model,
            &event,
            &mut ProductionCoordinatorContext {
                effects: &mut self.effects,
                fluid: &self.fluid,
                flipped: &mut self.flipped,
                clipboard: &mut self.clipboard,
                capabilities: self.capabilities,
                beat: self.engine.tempo.beat,
                active_chord: 0,
            },
        );
        assert!(!step.quit);
        for action in step.actions {
            for effect in action.effects {
                assert!(effect.result.is_ok(), "{:?}", effect.result);
            }
        }
    }

    fn gesture(&mut self, kind: GestureKind, phase: KeyEventKind) {
        self.key(KeyCode::Char(kind.key()), phase, KeyModifiers::NONE);
    }

    fn render(&mut self, seconds: f32) {
        for _ in 0..(seconds * self.engine.sample_rate) as usize {
            let sample = self.engine.next_stereo();
            assert!(sample.0.is_finite() && sample.1.is_finite());
        }
    }
}

#[test]
fn every_normal_mode_gesture_changes_the_rendered_audio() {
    const SAMPLE_RATE: f32 = 12_000.0;
    for kind in GestureKind::ALL {
        let mut dry = GesturePlayer::new(SAMPLE_RATE);
        let mut played = GesturePlayer::new(SAMPLE_RATE);
        dry.render(2.0);
        played.render(2.0);
        played.gesture(kind, KeyEventKind::Press);
        let mut difference = 0.0f64;
        let mut reference = 0.0f64;
        for _ in 0..(SAMPLE_RATE * 2.0) as usize {
            let clean = dry.engine.next_stereo();
            let wet = played.engine.next_stereo();
            difference += (wet.0 as f64 - clean.0 as f64).powi(2);
            reference += (clean.0 as f64).powi(2);
            assert!(wet.0.is_finite() && wet.1.is_finite());
            assert!(wet.0.abs() <= 1.0 && wet.1.abs() <= 1.0);
        }
        assert!(reference > 0.001, "fixture must contain audible music");
        let relative_difference = (difference / reference).sqrt();
        eprintln!(
            "{} relative audio difference: {relative_difference:.3}",
            kind.name()
        );
        assert!(
            relative_difference > 0.02,
            "{} did not change audio",
            kind.name()
        );
        played.gesture(kind, KeyEventKind::Release);
        played.render(1.0);
        assert!(!played.effects.session().load().gestures.has_held());
        assert_eq!(
            played
                .effects
                .session()
                .load()
                .gestures
                .amounts(played.effects.session().audio_seconds(),),
            [0.0; GESTURE_COUNT]
        );
    }
}

#[test]
fn saving_mid_swell_captures_audio_time_and_preserves_base_controls() {
    let mut played = GesturePlayer::new(12_000.0);
    played.render(2.0);
    played.gesture(GestureKind::Submerge, KeyEventKind::Press);
    played.render(0.4);
    played.key(
        KeyCode::Char('s'),
        KeyEventKind::Press,
        KeyModifiers::CONTROL,
    );
    let saved = decode_song_code(&played.clipboard.0).unwrap();
    let amount = saved.gestures.amounts(0.0)[GestureKind::Submerge as usize];
    assert!((amount - 1.0 / 3.0).abs() < 0.02, "saved amount {amount}");
    assert!(saved.gestures.envelope(GestureKind::Submerge).held);
    assert_eq!(
        saved.controls.master.level,
        FluidControls::default().master.level
    );
    let restored = LiveSessionSnapshot::from_song(&saved);
    assert!(restored.gestures.envelope(GestureKind::Submerge).restored);
}

#[test]
fn restored_hold_changes_audio_and_can_be_claimed_after_navigation() {
    let mut original = GesturePlayer::new(12_000.0);
    original.render(2.0);
    original.gesture(GestureKind::Submerge, KeyEventKind::Press);
    original.render(0.4);
    original.key(
        KeyCode::Char('s'),
        KeyEventKind::Press,
        KeyModifiers::CONTROL,
    );
    let song = decode_song_code(&original.clipboard.0).unwrap();
    let restored = LiveSessionSnapshot::from_song(&song);
    let mut dry_snapshot = restored.clone();
    dry_snapshot.gestures = GestureState::default();
    let mut wet = GesturePlayer::from_snapshot(12_000.0, restored);
    let mut dry = GesturePlayer::from_snapshot(12_000.0, dry_snapshot);
    let mut difference = 0.0f64;
    let mut reference = 0.0f64;
    for _ in 0..24_000 {
        let clean = dry.engine.next_stereo();
        let played = wet.engine.next_stereo();
        difference += (played.0 as f64 - clean.0 as f64).powi(2);
        reference += (clean.0 as f64).powi(2);
    }
    assert!(reference > 0.001);
    assert!((difference / reference).sqrt() > 0.02);
    wet.key(KeyCode::Tab, KeyEventKind::Press, KeyModifiers::NONE);
    wet.gesture(GestureKind::Submerge, KeyEventKind::Press);
    let snapshot = wet.effects.session().load();
    assert!(snapshot.gestures.envelope(GestureKind::Submerge).held);
    assert!(!snapshot.gestures.envelope(GestureKind::Submerge).restored);
    wet.gesture(GestureKind::Submerge, KeyEventKind::Release);
    wet.render(0.5);
    assert_eq!(
        wet.effects
            .session()
            .load()
            .gestures
            .amounts(wet.effects.session().audio_seconds()),
        [0.0; GESTURE_COUNT]
    );

    for cancel in [KeyCode::Esc, KeyCode::Char('/')] {
        let mut loaded =
            GesturePlayer::from_snapshot(12_000.0, LiveSessionSnapshot::from_song(&song));
        loaded.key(cancel, KeyEventKind::Press, KeyModifiers::NONE);
        assert!(
            !loaded.effects.session().load().gestures.has_held(),
            "{cancel:?} must release restored holds without a physical latch"
        );
    }
}

#[test]
fn live_gesture_keeps_auto_running_and_release_preserves_an_intervening_edit() {
    let mut player = GesturePlayer::new(12_000.0);
    player.engine.morph.store(Arc::new(Some(MorphState::new(
        vec![SongState::default()],
        64,
    ))));
    player.render(2.0);
    player.gesture(GestureKind::Thin, KeyEventKind::Press);
    player.render(0.2);
    assert!(
        player.engine.morph.load().is_some(),
        "a gesture must preserve auto"
    );
    for _ in 0..spec_index(Tab::Master, "master.bpm").unwrap() {
        player.key(KeyCode::Down, KeyEventKind::Press, KeyModifiers::NONE);
    }
    let old_tempo = player.effects.session().load().controls.master.bpm;
    player.key(KeyCode::Right, KeyEventKind::Press, KeyModifiers::NONE);
    let edited_tempo = player.effects.session().load().controls.master.bpm;
    assert!(
        edited_tempo > old_tempo,
        "the normal-mode arrow must still edit tempo"
    );
    assert!(
        player.engine.morph.load().is_none(),
        "ordinary edits still exit auto"
    );
    player.gesture(GestureKind::Thin, KeyEventKind::Release);
    player.render(0.5);
    assert_eq!(
        player.effects.session().load().controls.master.bpm,
        edited_tempo
    );
    assert_eq!(
        player
            .effects
            .session()
            .load()
            .gestures
            .amounts(player.effects.session().audio_seconds(),),
        [0.0; GESTURE_COUNT]
    );
}

/// Optional listening artifact: ordinary music, then each gesture's rise,
/// hold, and return through the same input/audio path as the live program.
#[test]
#[ignore = "writes an audition WAV to NOOISE_GESTURE_DEMO_WAV"]
fn render_live_gesture_audition() {
    let path = std::env::var("NOOISE_GESTURE_DEMO_WAV")
        .expect("set NOOISE_GESTURE_DEMO_WAV to the audition WAV path");
    const SAMPLE_RATE: f32 = 44_100.0;
    let mut player = GesturePlayer::new(SAMPLE_RATE);
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: SAMPLE_RATE as u32,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut wav = hound::WavWriter::create(&path, spec).unwrap();
    let mut write = |player: &mut GesturePlayer, seconds: f32| {
        for _ in 0..(SAMPLE_RATE * seconds) as usize {
            let (left, right) = player.engine.next_stereo();
            assert!(left.is_finite() && right.is_finite());
            wav.write_sample((left.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
                .unwrap();
            wav.write_sample((right.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
                .unwrap();
        }
    };
    write(&mut player, 3.0);
    for kind in GestureKind::ALL {
        eprintln!(
            "{} at {:.1}s",
            kind.name(),
            player.engine.current_sample as f64 / SAMPLE_RATE as f64
        );
        player.gesture(kind, KeyEventKind::Press);
        write(&mut player, 3.0);
        player.gesture(kind, KeyEventKind::Release);
        write(&mut player, 4.0);
    }
    wav.finalize().unwrap();
    eprintln!("Audition WAV: {path}");
}
