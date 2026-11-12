//! Ignored cost probe for the downbeat reset: per-buffer audio cost, session
//! publications, and UI tick/frame cost around a press, through the real key
//! path, in the show's launch shape (`nooise 1 --start-stopped`: one-song
//! auto, transport stopped until Shift+P) and with each live feature held.
//! `cargo test --release downbeat_probe -- --ignored --nocapture --test-threads=1`

use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use super::super::auto::decode_auto_states;
use super::super::interaction::InteractionModel;
use super::super::runtime::{TerminalCapabilities, normalize_key_event};
use super::super::ui::render;
use super::super::view::{
    TelemetryView, UiViewModel, ViewNotices, ViewPresentation, ViewProjection,
};
use super::super::visualizer::RippleField;
use super::*;

const RATE: f32 = 44_100.0;
const BUF: usize = 256;
/// The UI turn loop's tick: about every 33 ms, six buffers.
const UI_EVERY: usize = 6;

#[derive(Default)]
struct TestClipboard;

impl Clipboard for TestClipboard {
    fn set_text(&mut self, _text: String) -> Result<(), ClipboardError> {
        Ok(())
    }
}

struct Rig {
    engine: FluidEngine,
    session: LiveSession,
    effects: EffectExecutor,
    model: InteractionModel,
    flipped: FlippedUnits,
    clipboard: TestClipboard,
    telemetry: Arc<FluidTelemetry>,
    fluid: RippleField,
    capabilities: TerminalCapabilities,
}

#[derive(Default)]
struct Window {
    audio_ns: Vec<u64>,
    ui_ns: Vec<u64>,
    publications: u64,
    reanchors: u64,
}

impl Window {
    fn report(&self, label: &str) {
        let budget = BUF as f64 / f64::from(RATE) * 1e9;
        let stat = |v: &[u64]| {
            let mut s = v.to_vec();
            s.sort_unstable();
            let mean = s.iter().sum::<u64>() as f64 / s.len().max(1) as f64;
            (mean / 1e3, s.last().copied().unwrap_or(0) as f64 / 1e3)
        };
        let (am, ax) = stat(&self.audio_ns);
        let (um, ux) = stat(&self.ui_ns);
        println!(
            "{label:<44} audio mean {am:6.1} max {ax:7.1} us ({:4.1}% budget) | ui frame mean {um:7.1} max {ux:8.1} us | publications {} | reanchors {}",
            am * 1e3 / budget * 100.0,
            self.publications,
            self.reanchors,
        );
    }
}

impl Rig {
    fn show(stopped: bool, capabilities: TerminalCapabilities) -> Self {
        let songs = decode_auto_states();
        let song = songs[0].clone();
        let mut snapshot = LiveSessionSnapshot::from_song(&song);
        if stopped {
            snapshot.transport = Transport::Stopped;
        }
        let session = LiveSession::new(snapshot);
        let morph = Arc::new(ArcSwap::from_pointee(Some(MorphState::labelled(
            vec![song],
            vec![1],
            8,
        ))));
        let telemetry = Arc::new(FluidTelemetry::default());
        let engine = FluidEngine::new(
            RATE,
            session.clone(),
            Arc::clone(&morph),
            Arc::clone(&telemetry),
        );
        Self {
            engine,
            effects: EffectExecutor::seeded(session.clone(), AutoControls::new(morph, songs, 8), 7),
            session,
            model: InteractionModel::default(),
            flipped: FlippedUnits::default(),
            clipboard: TestClipboard,
            telemetry,
            fluid: RippleField::new(),
            capabilities,
        }
    }

    fn key(&mut self, code: KeyCode, modifiers: KeyModifiers, phase: KeyEventKind) {
        let Some(event) = normalize_key_event(
            KeyEvent::new_with_kind(code, modifiers, phase),
            self.capabilities,
        ) else {
            return;
        };
        coordinate_production_event(
            &mut self.model,
            &event,
            &mut ProductionCoordinatorContext {
                effects: &mut self.effects,
                flipped: &mut self.flipped,
                clipboard: &mut self.clipboard,
                capabilities: self.capabilities,
                beat: self.telemetry.beat(),
            },
        );
    }

    fn press(&mut self, character: char) {
        let modifiers = if character.is_ascii_uppercase() {
            KeyModifiers::SHIFT
        } else {
            KeyModifiers::NONE
        };
        self.key(KeyCode::Char(character), modifiers, KeyEventKind::Press);
    }

    fn release(&mut self, character: char) {
        let modifiers = if character.is_ascii_uppercase() {
            KeyModifiers::SHIFT
        } else {
            KeyModifiers::NONE
        };
        self.key(KeyCode::Char(character), modifiers, KeyEventKind::Release);
    }

    fn tap(&mut self, character: char) {
        self.press(character);
        self.release(character);
    }

    fn frame(&mut self) -> u64 {
        let start = Instant::now();
        coordinate_production_tick(&mut self.effects, self.telemetry.beat())
            .expect("pending commit has no fallible effects");
        self.session.reclaim();
        self.fluid.tick(0.033, &self.telemetry);
        let session = self.effects.session().load();
        let view = UiViewModel::project(ViewProjection {
            interaction: &self.model,
            session: &session,
            telemetry: TelemetryView {
                beat: self.telemetry.beat(),
                active_chord: self
                    .telemetry
                    .chord_slot
                    .load(std::sync::atomic::Ordering::Relaxed),
                chord_span: self.telemetry.chord_span(),
                levels: self.telemetry.levels(),
                gain_reduction_db: self.telemetry.gain_reduction(),
                phrase_end_beat: self.effects.session().audio_phrase_end(),
            },
            presentation: ViewPresentation {
                fluid: &self.fluid,
                flipped: &self.flipped,
                cursor_visible: true,
                notices: ViewNotices {
                    effect: self.effects.message().map(str::to_string),
                    effect_failed: self.effects.message_failed(),
                    ..ViewNotices::default()
                },
                gesture_now_seconds: self.effects.session().audio_seconds(),
                gesture_holds_available: self.capabilities.supports_holds(),
                filled_states: self.effects.filled_states(),
            },
        });
        let mut terminal = Terminal::new(TestBackend::new(160, 50)).expect("test backend");
        terminal.draw(|frame| render(frame, &view)).expect("render");
        start.elapsed().as_nanos() as u64
    }

    /// Play `seconds`, calling `between` before each buffer.
    fn run(&mut self, seconds: f64, mut between: impl FnMut(usize, &mut Self)) -> Window {
        let buffers = (seconds * f64::from(RATE) / BUF as f64) as usize;
        let mut window = Window::default();
        for b in 0..buffers {
            between(b, self);
            let generation = self.session.load().generation;
            let applied = self.session.load().downbeat_applied;
            let start = Instant::now();
            let mut acc = 0.0f32;
            for _ in 0..BUF {
                let (l, r) = self.engine.next_stereo();
                acc += l + r;
            }
            std::hint::black_box(acc);
            window.audio_ns.push(start.elapsed().as_nanos() as u64);
            let after = self.session.load();
            window.publications += after.generation.wrapping_sub(generation);
            window.reanchors += after.downbeat_applied.wrapping_sub(applied);
            if b % UI_EVERY == 0 {
                window.ui_ns.push(self.frame());
            }
        }
        window
    }
}

fn scenario(label: &str, stopped: bool, setup: impl Fn(&mut Rig)) {
    for capabilities in [
        TerminalCapabilities::full(),
        TerminalCapabilities::default(),
    ] {
        let caps = if capabilities.supports_holds() {
            "full"
        } else {
            "reduced"
        };
        let mut rig = Rig::show(stopped, capabilities);
        rig.run(1.0, |_, _| {});
        if stopped {
            rig.tap('D');
            rig.run(2.0, |_, _| {})
                .report(&format!("{label} [{caps}] stopped, after d"));
            rig.tap('P');
        }
        rig.run(3.0, |_, _| {});
        setup(&mut rig);
        rig.run(2.0, |_, _| {})
            .report(&format!("{label} [{caps}] before d"));
        rig.tap('D');
        rig.run(4.0, |_, _| {})
            .report(&format!("{label} [{caps}] after one d"));
        rig.run(4.0, |b, rig| {
            if b % 5 == 0 {
                rig.tap('D');
            }
        })
        .report(&format!("{label} [{caps}] d at autorepeat rate"));
        rig.run(2.0, |_, _| {})
            .report(&format!("{label} [{caps}] settled"));
    }
}

#[test]
#[ignore = "timing probe; run in release with --nocapture"]
fn downbeat_probe() {
    scenario("plain", false, |_| {});
    scenario("launched stopped", true, |_| {});
    scenario("queued state", true, |rig| {
        rig.tap('"');
        rig.tap('1');
        rig.tap('\'');
        rig.tap('1');
    });
    scenario("G loop", true, |rig| {
        rig.tap('G');
        rig.tap('1');
    });
    scenario("vamp", true, |rig| rig.tap('a'));
    scenario("held beat repeat", true, |rig| {
        rig.press('b');
        rig.run(1.0, |_, _| {});
    });
    scenario("motion loop", true, |rig| {
        rig.key(KeyCode::Down, KeyModifiers::NONE, KeyEventKind::Press);
        for _ in 0..8 {
            rig.key(KeyCode::Right, KeyModifiers::NONE, KeyEventKind::Press);
            rig.run(0.2, |_, _| {});
            rig.key(KeyCode::Left, KeyModifiers::NONE, KeyEventKind::Press);
            rig.run(0.2, |_, _| {});
        }
        rig.tap('g');
        rig.tap('1');
    });
}
