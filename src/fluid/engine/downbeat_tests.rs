//! Downbeat reset (`d`, `/downbeat`) driven through the production terminal
//! input path into rendered audio.

use std::sync::atomic::Ordering;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::super::interaction::{InteractionModel, Navigation, Page};
use super::super::runtime::{TerminalCapabilities, normalize_key_event};
use super::*;

const RATE: f32 = 8000.0;
const BPM: f32 = 120.0;

#[derive(Default)]
struct TestClipboard;

impl Clipboard for TestClipboard {
    fn set_text(&mut self, _text: String) -> Result<(), ClipboardError> {
        Ok(())
    }
}

/// Four one-bar chords, so the phrase is 16 beats, and a Kick whose 8-step
/// pattern at 0.25 beats tells step 1 (beat 0) apart from every other step.
fn song() -> SongState {
    let mut song = SongState::default();
    song.controls.master.bpm = BPM;
    song.controls.pad.level = 0.5;
    song.controls.pad.chord_bars = 1.0;
    song.controls.pad.chord_count = 4.0;
    song.controls.pad.chord_offset = 0.0;
    song.controls.kick.level = 0.5;
    song.controls.kick.pattern = CUSTOM_KICK_PATTERN_INDEX as f32;
    song.controls.kick.interval_beats = 0.25;
    song.controls.kick.offset_beats = 0.0;
    song.controls.kick.step_count = 8.0;
    song.controls.kick.steps = [0.0; KICK_STEP_COUNT];
    song.controls.kick.steps[0] = 1.0;
    song.controls.kick.steps[3] = 1.0;
    song
}

struct Player {
    engine: FluidEngine,
    session: LiveSession,
    effects: EffectExecutor,
    model: InteractionModel,
    flipped: FlippedUnits,
    clipboard: TestClipboard,
    morph: Arc<ArcSwap<Option<MorphState>>>,
}

impl Player {
    fn new(song: &SongState, auto_states: Vec<SongState>) -> Self {
        let session = LiveSession::new(LiveSessionSnapshot::from_song(song));
        let morph = no_morph();
        let mut engine = FluidEngine::new(
            RATE,
            session.clone(),
            Arc::clone(&morph),
            Arc::new(FluidTelemetry::default()),
        );
        engine.reseed(7);
        Self {
            engine,
            effects: EffectExecutor::seeded(
                session.clone(),
                AutoControls::new(Arc::clone(&morph), auto_states, 8),
                7,
            ),
            session,
            model: InteractionModel {
                navigation: Navigation::for_page(Page::Master),
                ..InteractionModel::default()
            },
            flipped: FlippedUnits::default(),
            clipboard: TestClipboard,
            morph,
        }
    }

    fn key(&mut self, code: KeyCode, modifiers: KeyModifiers, phase: KeyEventKind) {
        let event = normalize_key_event(
            KeyEvent::new_with_kind(code, modifiers, phase),
            TerminalCapabilities::full(),
        )
        .expect("full capabilities report every key event");
        let step = coordinate_production_event(
            &mut self.model,
            &event,
            &mut ProductionCoordinatorContext {
                effects: &mut self.effects,
                flipped: &mut self.flipped,
                clipboard: &mut self.clipboard,
                capabilities: TerminalCapabilities::full(),
                beat: self.engine.tempo.beat,
            },
        );
        for action in step.actions {
            for effect in action.effects {
                assert!(effect.result.is_ok(), "{:?}", effect.result);
            }
        }
    }

    fn tap(&mut self, character: char) {
        let modifiers = if character.is_ascii_uppercase() {
            KeyModifiers::SHIFT
        } else {
            KeyModifiers::NONE
        };
        self.key(KeyCode::Char(character), modifiers, KeyEventKind::Press);
        self.key(KeyCode::Char(character), modifiers, KeyEventKind::Release);
    }

    fn downbeat(&mut self) {
        self.tap('d');
    }

    /// Render one sample; returns the beat that sample played at.
    fn step(&mut self) -> (f64, (f32, f32)) {
        let sample = self.engine.next_stereo();
        assert!(sample.0.is_finite() && sample.1.is_finite());
        // A re-anchor moves the clock inside this call, so the sample's own
        // beat is the advanced clock minus one sample's worth.
        let per_sample = self.engine.tempo.bpm / (60.0 * f64::from(RATE));
        (self.engine.tempo.beat - per_sample, sample)
    }

    fn play_until(&mut self, beat: f64) {
        while self.engine.tempo.beat < beat {
            self.step();
        }
    }

    fn kick_pulses(&self) -> u64 {
        self.engine.telemetry.kick_pulse.load(Ordering::Acquire)
    }

    fn chord_pulses(&self) -> u64 {
        self.engine.telemetry.chord_pulse.load(Ordering::Acquire)
    }

    fn per_sample(&self) -> f64 {
        self.engine.tempo.bpm / (60.0 * f64::from(RATE))
    }

    /// Play until the requested reset lands; returns the old-clock beat the
    /// landing sample started on.
    fn cross_line(&mut self) -> f64 {
        loop {
            let top = self.engine.tempo.beat;
            self.step();
            if self.engine.tempo.beat < top {
                return top;
            }
            assert!(top < 64.0, "the reset never landed");
        }
    }

    /// Beats the Kick fires on while playing `beats` more.
    fn kick_beats(&mut self, beats: f64) -> Vec<f64> {
        let end = self.engine.tempo.beat + beats;
        let mut hits = Vec::new();
        while self.engine.tempo.beat < end {
            let before = self.kick_pulses();
            let (beat, _) = self.step();
            if self.kick_pulses() != before {
                hits.push((beat * 1000.0).round() / 1000.0);
            }
        }
        hits
    }
}

#[test]
fn d_waits_for_the_next_beat_line_and_makes_it_beat_one() {
    let mut player = Player::new(&song(), Vec::new());
    player.play_until(5.6);
    let tempo = player.engine.tempo.bpm;
    let slot = player.engine.pad.cursor.slot();

    player.downbeat();
    // Until the line the old grid plays on: steps 1 and 4 of the 2-beat
    // cycle that began on beat 4 have both passed, so nothing fires.
    assert!(player.kick_beats(0.35).is_empty());
    let kicks = player.kick_pulses();
    let landed_from = player.cross_line();

    assert!(
        (0.0..player.per_sample()).contains(&(landed_from - 6.0)),
        "the reset lands on beat line 6, got {landed_from}"
    );
    assert_eq!(player.kick_pulses(), kicks + 1, "step 1 hits on the line");
    assert_eq!(player.engine.tempo.bpm, tempo, "tempo is unchanged");
    assert_eq!(player.engine.pad.cursor.slot(), slot, "no chord is skipped");
    // Steps 1 and 4 of an 8-step 0.25-beat lane: beats 0 and 0.75 of every
    // two-beat cycle on the new grid.
    assert_eq!(player.kick_beats(3.9), vec![0.75, 2.0, 2.75]);
    player.play_until(4.0);
    assert_eq!((player.engine.tempo.beat / 4.0).floor(), 1.0, "bar two");
}

#[test]
fn the_beat_phase_never_moves_only_the_bar_rotates() {
    let mut player = Player::new(&song(), Vec::new());
    let mut twin = Player::new(&song(), Vec::new());
    player.play_until(5.6);
    twin.play_until(5.6);
    player.downbeat();
    while player.engine.tempo.beat > 1.0 {
        player.step();
        twin.step();
    }
    let per_sample = player.per_sample();
    for _ in 0..(RATE as usize * 2) {
        player.step();
        twin.step();
        let rotation = twin.engine.tempo.beat - player.engine.tempo.beat;
        assert!(
            (rotation - 6.0).abs() < per_sample,
            "rotated by {rotation} beats, not a whole number"
        );
    }
}

#[test]
fn the_sounding_chord_restarts_its_count_without_being_struck_again() {
    let mut player = Player::new(&song(), Vec::new());
    player.play_until(5.6);
    let slot = player.engine.pad.cursor.slot();
    let chords = player.chord_pulses();

    player.downbeat();
    player.cross_line();

    let span = player.engine.pad.cursor.chord_span();
    assert_eq!((span.start_beat, span.length_beats), (0.0, 4.0));
    player.play_until(4.0 - 0.01);
    assert_eq!(player.engine.pad.cursor.slot(), slot);
    assert_eq!(player.chord_pulses(), chords, "the chord rang on, unstruck");
    player.play_until(4.01);
    assert_eq!(
        player.engine.pad.cursor.slot(),
        (slot + 1) % 4,
        "the next chord lands one full chord after the line"
    );
    for follower in [
        &player.engine.bass.progression,
        &player.engine.arp.progression,
        &player.engine.tonal.progression,
        &player.engine.lead.progression,
    ] {
        if let Some(cursor) = &follower.cursor {
            assert_eq!(cursor.slot(), player.engine.pad.cursor.slot());
        }
    }
}

#[test]
fn a_chord_due_on_the_line_arrives_on_beat_one() {
    let mut player = Player::new(&song(), Vec::new());
    player.play_until(7.6);
    let slot = player.engine.pad.cursor.slot();
    let chords = player.chord_pulses();

    player.downbeat();
    player.cross_line();
    player.step();

    assert_eq!(player.engine.pad.cursor.slot(), (slot + 1) % 4);
    assert_eq!(
        player.chord_pulses(),
        chords + 1,
        "struck once, on the line"
    );
    let span = player.engine.pad.cursor.chord_span();
    assert_eq!((span.start_beat, span.length_beats), (0.0, 4.0));
}

#[test]
fn a_queued_state_lands_at_the_phrase_end_measured_from_the_new_downbeat() {
    let mut player = Player::new(&song(), Vec::new());
    let destination = Arc::new(LiveSessionSnapshot::from_song(&song()));
    player.session.update(|snapshot| {
        snapshot.pending_state = Some(PendingState {
            index: 1,
            snapshot: Arc::clone(&destination),
        });
    });
    player.play_until(5.6);
    assert_eq!(player.session.audio_phrase_end(), Some(16.0));

    player.downbeat();
    player.play_until(5.9);
    assert_eq!(
        player.session.audio_phrase_end(),
        Some(16.0),
        "until the line"
    );
    player.cross_line();
    // The countdown is published at the top of each sample.
    player.step();

    // The chord that began on beat 4 now begins on the line, so the three
    // chords left in the phrase end 12 beats after it.
    assert_eq!(player.session.audio_phrase_end(), Some(12.0));
    player.play_until(11.9);
    assert!(player.session.load().pending_state.is_some());
    player.play_until(12.1);
    assert!(player.session.load().pending_state.is_none(), "landed");
    assert_eq!(player.session.load().current_state, Some(1));
}

#[test]
fn a_waiting_push_lands_on_the_first_new_bar_line_at_or_after_its_own() {
    let mut player = Player::new(&song(), Vec::new());
    player.play_until(5.6);
    player.session.update(|snapshot| {
        snapshot.push.set_quantize(PushQuantize::OneBar, 5.6);
        snapshot.push.queued = Some(QueuedPush {
            target: Triad::C_MAJOR,
            land_beat: 8.0,
        });
    });

    player.downbeat();
    player.cross_line();

    // Old beat 8 is two beats after the line; the new grid's bar line at or
    // after it is beat 4.
    let queued = player.session.load().push.queued.expect("still waiting");
    assert_eq!(queued.land_beat, 4.0);
    player.play_until(4.01);
    let push = player.session.load().push;
    assert!(push.queued.is_none(), "it landed on bar two");
    assert_eq!(push.held.map(|held| held.triad), Some(Triad::C_MAJOR));
}

#[test]
fn a_latched_vamp_keeps_its_chord_without_a_restrike() {
    let mut player = Player::new(&song(), Vec::new());
    player.play_until(5.6);
    player.tap('a');
    player.play_until(5.8);
    let vamp = player.session.load().vamp.expect("vamp latched");
    let chords = player.chord_pulses();

    player.downbeat();
    player.cross_line();

    let rebased = player.session.load().vamp.expect("vamp still latched");
    assert_eq!(rebased.slot, vamp.slot);
    assert_eq!(rebased.start_beat, vamp.start_beat - 4.0);
    // A vamp re-voices its chord at each chord boundary; the first one on
    // the new grid is a full chord after the line.
    player.play_until(3.9);
    assert_eq!(
        player.chord_pulses(),
        chords,
        "no restrike, no chord change"
    );
    assert_eq!(usize::from(vamp.slot), player.engine.pad.cursor.slot());
}

#[test]
fn a_held_beat_repeat_keeps_looping_and_releases_on_the_new_grid() {
    let mut player = Player::new(&song(), Vec::new());
    let mut dry = Player::new(&song(), Vec::new());
    player.play_until(4.3);
    dry.play_until(4.3);
    player.key(KeyCode::Char('b'), KeyModifiers::NONE, KeyEventKind::Press);
    player.play_until(6.5);
    dry.play_until(6.5);

    player.downbeat();
    dry.downbeat();
    player.cross_line();
    dry.cross_line();
    let mut difference = 0.0f32;
    while player.engine.tempo.beat < 0.5 {
        let wet = player.step().1;
        let clean = dry.step().1;
        difference = difference.max((wet.0 - clean.0).abs());
    }
    assert!(difference > 0.01, "the captured beat is still looping");
    assert!(
        player
            .session
            .load()
            .gestures
            .envelope(GestureKind::BeatRepeat)
            .held,
        "the hold survives the reset"
    );
    player.key(
        KeyCode::Char('b'),
        KeyModifiers::NONE,
        KeyEventKind::Release,
    );
    player.play_until(1.5);
    assert!(
        !player
            .session
            .load()
            .gestures
            .envelope(GestureKind::BeatRepeat)
            .held
    );
}

#[test]
fn the_reset_is_click_free() {
    // Pads only: the line must not add a step larger than the music's own.
    let mut quiet = song();
    quiet.controls.kick.level = 0.0;
    let mut player = Player::new(&quiet, Vec::new());
    player.play_until(5.0);
    let mut previous = player.step().1;
    let mut largest_before = 0.0f32;
    while player.engine.tempo.beat < 5.6 {
        let sample = player.step().1;
        largest_before = largest_before.max((sample.0 - previous.0).abs());
        previous = sample;
    }
    player.downbeat();
    let mut largest_after = 0.0f32;
    let mut crossed = false;
    for _ in 0..(RATE as usize) {
        let top = player.engine.tempo.beat;
        let sample = player.step().1;
        crossed |= player.engine.tempo.beat < top;
        largest_after = largest_after.max((sample.0 - previous.0).abs());
        previous = sample;
    }
    assert!(crossed, "the line fell inside the measured second");
    assert!(largest_before > 0.0, "fixture must sound");
    assert!(largest_after > 0.0, "the chord rings on through the reset");
    assert!(
        largest_after <= largest_before * 1.5,
        "reset stepped {largest_after}, playing stepped {largest_before}"
    );
}

#[test]
fn auto_keeps_its_progress_aligned_to_the_restarted_chord() {
    let auto_states = vec![song(), {
        let mut other = song();
        other.controls.pad.level = 0.2;
        other
    }];
    let mut player = Player::new(&song(), auto_states);
    player.play_until(0.5);
    player.tap('A');
    assert!(player.morph.load().is_some(), "auto running");
    player.play_until(5.6);
    let chord_start = 4.0;
    let before = player
        .morph
        .load()
        .as_ref()
        .as_ref()
        .unwrap()
        .position_at(chord_start);
    let chords = player.chord_pulses();

    player.downbeat();
    player.cross_line();

    let morph = player.morph.load();
    let morph = morph.as_ref().as_ref().expect("the reset keeps auto");
    assert_eq!(morph.position_at(0.0), before);
    assert_eq!(
        morph.phrase_start_at(player.engine.tempo.beat),
        player.engine.pad.cursor.morph_phrase_start,
        "the cursor keeps the morph's phrase, so nothing re-derives"
    );
    player.play_until(1.0);
    assert_eq!(player.chord_pulses(), chords);
}

#[test]
fn a_stopped_clock_ignores_the_reset() {
    let mut player = Player::new(&song(), Vec::new());
    player.play_until(5.6);
    player.tap('P');
    let resets = player.session.load().downbeat_reset;
    player.downbeat();
    assert_eq!(player.session.load().downbeat_reset, resets);
    assert!(
        player
            .effects
            .message()
            .is_some_and(|message| message.contains("Shift+P"))
    );
}

#[test]
fn slash_downbeat_resets_through_the_palette() {
    let mut player = Player::new(&song(), Vec::new());
    player.play_until(5.6);
    player.tap('/');
    for character in "downbeat".chars() {
        player.key(
            KeyCode::Char(character),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        );
    }
    player.key(KeyCode::Enter, KeyModifiers::NONE, KeyEventKind::Press);
    assert_eq!(player.session.load().downbeat_reset, 1);
    assert!(matches!(
        player.model.mode,
        super::super::interaction::InteractionMode::Browsing
    ));
    let landed_from = player.cross_line();
    assert!((landed_from - 6.0).abs() < player.per_sample());
    assert_eq!(player.session.load().downbeat_applied, 1);
}
