//! The vamp over a harmony loop, driven through the production terminal
//! input path (Space pushes, `G` `1`, `a`, `"` and `'`) into the audio
//! engine: the vamp holds the looped chord while the loop's clock runs on,
//! and letting go rejoins the loop where it has got to.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::super::interaction::InteractionModel;
use super::super::runtime::{TerminalCapabilities, normalize_key_event};
use super::*;

const RATE: f32 = 8000.0;

#[derive(Default)]
struct TestClipboard;

impl Clipboard for TestClipboard {
    fn set_text(&mut self, _text: String) -> Result<(), ClipboardError> {
        Ok(())
    }
}

/// The audio engine and the production key path over one session.
struct Rig {
    engine: FluidEngine,
    session: LiveSession,
    effects: EffectExecutor,
    model: InteractionModel,
    flipped: FlippedUnits,
}

impl Rig {
    fn new(song: &SongState) -> Self {
        let session = LiveSession::new(LiveSessionSnapshot::from_song(song));
        let morph = no_morph();
        let engine = FluidEngine::new(
            RATE,
            session.clone(),
            Arc::clone(&morph),
            Arc::new(FluidTelemetry::default()),
        );
        Self {
            engine,
            effects: EffectExecutor::seeded(
                session.clone(),
                AutoControls::new(morph, vec![], 8),
                7,
            ),
            session,
            model: InteractionModel::default(),
            flipped: FlippedUnits::default(),
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
                clipboard: &mut TestClipboard,
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
        let modifiers = if character.is_ascii_uppercase() || character == '"' {
            KeyModifiers::SHIFT
        } else {
            KeyModifiers::NONE
        };
        self.key(KeyCode::Char(character), modifiers, KeyEventKind::Press);
        self.key(KeyCode::Char(character), modifiers, KeyEventKind::Release);
    }

    /// Space held, `move` pressed, Space let go: one push.
    fn push(&mut self, harmony_move: char) {
        self.key(KeyCode::Char(' '), KeyModifiers::NONE, KeyEventKind::Press);
        self.tap(harmony_move);
        self.key(
            KeyCode::Char(' '),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        );
    }

    fn play_until(&mut self, beat: f64) {
        while self.engine.tempo.beat < beat {
            self.engine.next_stereo();
        }
    }

    /// Play until the pad's notes change; the beat of that sample.
    fn play_to_change(&mut self, limit: f64) -> f64 {
        let notes = self.engine.pad.last_chord_notes;
        while self.engine.pad.last_chord_notes == notes {
            assert!(self.engine.tempo.beat < limit, "no change before {limit}");
            self.engine.next_stereo();
        }
        self.engine.tempo.beat
    }

    fn notes(&self) -> [i32; 4] {
        self.engine.pad.last_chord_notes
    }
}

/// A `G` `1` loop of Am, C (pushed at 1.25) and G (at 2.75) from beat 4,
/// pushes unquantized and the vamp releasing on a 2-beat chord grid, with
/// the C and G notes the pad plays.
fn looping_rig() -> (Rig, [i32; 4], [i32; 4]) {
    let mut song = SongState::default();
    song.controls.master.bpm = 120.0;
    song.controls.pad.chord_bars = 0.5;
    let mut rig = Rig::new(&song);
    rig.session.update(|s| s.push.quantize = PushQuantize::Off);
    rig.play_until(1.25);
    rig.push('f');
    rig.play_until(2.0);
    let c = rig.notes();
    rig.play_until(2.75);
    rig.push('l');
    rig.play_until(3.0);
    let g = rig.notes();
    rig.play_until(3.5);
    rig.tap('G');
    rig.tap('1');
    assert!(rig.session.load().push.looping.is_some(), "G 1 loops");
    (rig, c, g)
}

/// The vamp overpowers a loop: `a` holds the loop chord sounding (C) while
/// the loop's clock keeps running, and a second `a` rejoins the loop where it
/// has got to (G) on the vamp's release line, through the ordinary chord
/// change. Before this change the loop simply played on under the vamp.
#[test]
fn a_vamp_holds_the_looped_chord_and_rejoins_the_running_loop() {
    let (mut rig, c, g) = looping_rig();
    let loop_start = rig.session.load().push.looping.unwrap().start_beat;
    rig.play_until(5.5);
    assert_eq!(rig.notes(), c, "the loop is on C");
    rig.tap('a');
    let vamp = rig.session.load().vamp.expect("a vamp");
    assert!((vamp.start_beat - 5.25).abs() < 0.01, "from C's onset");
    assert_eq!(vamp.slot, rig.session.load().push.looping.unwrap().slot);
    rig.play_until(7.0);
    assert_eq!(rig.notes(), c, "held past the loop's G at 6.75");
    rig.tap('a');
    let at = rig.play_to_change(8.0);
    assert!(
        (at - 7.25).abs() < 0.01,
        "released on the 2-beat line: {at}"
    );
    assert_eq!(rig.notes(), g, "rejoined mid-pass, on G");
    assert_eq!(rig.session.load().vamp, None);
    let looping = rig.session.load().push.looping.expect("the loop ran on");
    assert_eq!(looping.start_beat, loop_start);
}

/// A latched vamp over a loop survives a song code: the loaded song holds
/// the same loop chord, not the one the loop has moved on to.
#[test]
fn a_latched_vamp_over_a_loop_round_trips_through_a_song_code() {
    let (mut rig, c, g) = looping_rig();
    rig.play_until(5.5);
    rig.tap('a');
    rig.play_until(7.0);
    let live = rig.session.load();
    assert!(live.vamp_latched);
    let looping = live.push.looping.unwrap();
    assert_eq!(looping.at(7.0).map(|event| event.notes), Some(g));
    let song = SongState {
        controls: live.controls.clone(),
        push: live.push.rebased_at(7.0),
        vamp: live.vamp.map(|vamp| vamp.rebased_at(7.0)),
        ..SongState::default()
    };
    let decoded = decode_song_code(&encode_song_code(&song).unwrap()).unwrap();
    assert!(decoded.vamp.is_some() && decoded.push.looping.is_some());
    let mut reloaded = Rig::new(&decoded);
    reloaded.engine.next_stereo();
    assert_eq!(reloaded.notes(), c);
}

/// A state landing during a vamp over a loop takes the destination's own
/// harmony: state 1, stored before the grab, holds the pushed G and no loop,
/// and a latched (not physically held) vamp leaves with the outgoing state.
#[test]
fn a_state_landing_during_a_vamp_over_a_loop_takes_the_destination() {
    let mut song = SongState::default();
    song.controls.master.bpm = 120.0;
    song.controls.pad.chord_bars = 0.5;
    let mut rig = Rig::new(&song);
    rig.session.update(|s| s.push.quantize = PushQuantize::Off);
    rig.play_until(1.25);
    rig.push('f');
    rig.play_until(2.75);
    rig.push('l');
    rig.play_until(3.0);
    let g = rig.notes();
    rig.tap('"');
    rig.tap('1');
    rig.play_until(3.5);
    rig.tap('G');
    rig.tap('1');
    rig.play_until(5.5);
    rig.tap('a');
    rig.play_until(6.0);
    let c = rig.notes();
    assert_ne!(c, g, "the vamp holds the loop's C");
    assert!(rig.session.load().vamp.is_some() && rig.session.load().push.looping.is_some());
    rig.tap('\'');
    rig.tap('1');
    let revision = rig.session.load().state_revision;
    while rig.session.load().state_revision == revision {
        rig.engine.next_stereo();
        assert!(rig.engine.tempo.beat < 300.0, "the state never landed");
    }
    let landed = rig.session.load();
    assert_eq!(landed.vamp, None, "the latched vamp left with its state");
    assert_eq!(landed.push.looping, None, "state 1 had no loop");
    assert_eq!(landed.push.held.map(|held| held.notes), Some(g));
    for _ in 0..64 {
        rig.engine.next_stereo();
    }
    assert_eq!(rig.notes(), g, "the destination's pushed G plays");
}
