//! Deterministic, production-path interaction replay support.
//!
//! Fixtures retain sanitized key phase/modifiers/repeat counts, relative time,
//! dimensions, lifecycle markers, and explicit tick/idle records. Paste and
//! mouse payloads are always redacted.

use std::collections::VecDeque;
use std::fmt;
use std::io;
use std::time::Duration;

use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MediaKeyCode, ModifierKeyCode,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use super::coordinator::{
    ProductionCoordinatorContext, ProductionStep, coordinate_production_action,
    coordinate_production_tick, production_frame,
};
use super::effect::{Clipboard, ClipboardError, EffectAcknowledgement};
use super::interaction::{
    AutomationKind, AutomationMode, ChordDrill, InputPhase, Intent, InteractionMode,
    InteractionModel, JumpStage, LeadDrill, LeadPlay, Navigation, NumericEntry, PaletteMode,
    PaletteStagedEdit, PerformanceInstrument, PerformanceKind, PerformanceMode, PhasePolicy,
    SemanticAction,
};
use super::runtime::{
    Clock, EventSource, FakeClock, InputMapping, MAX_FRAME_GAP, Modifiers, PhysicalKey,
    SanitizedTraceRecorder, Scheduler, SchedulerConfig, TerminalCapabilities, TransportEvent,
    TransportKey, decode_physical_key, normalize_key_event, parse_phase,
};
use super::view::{
    MIN_TERMINAL_HEIGHT, MIN_TERMINAL_WIDTH, TelemetryView, UiViewModel, ViewNotices,
    ViewPresentation, ViewProjection,
};
use super::*;

const EVENT_COST: Duration = Duration::from_millis(1);
const DEFAULT_WIDTH: u16 = 80;
const DEFAULT_HEIGHT: u16 = 24;

#[derive(Clone, Debug, PartialEq, Eq)]
enum TraceEvent {
    Key {
        after_ms: u64,
        code: PhysicalKey,
        phase: InputPhase,
        modifiers: u8,
        repeat_count: u64,
    },
    Resize {
        after_ms: u64,
        width: u16,
        height: u16,
    },
    Tick {
        after_ms: u64,
    },
    Idle {
        after_ms: u64,
    },
    Redacted {
        after_ms: u64,
        kind: &'static str,
    },
    Focus {
        after_ms: u64,
        gained: bool,
    },
    Shutdown {
        after_ms: u64,
    },
}

type FixtureKey = PhysicalKey;

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReplayTrace {
    events: Vec<TraceEvent>,
}

/// The record a fixture line stands for once played back: a transport event
/// the scheduler admits, or a turn boundary the scripted source marks.
enum TraceRecord {
    Transport(TransportEvent),
    Tick,
    Idle,
}

impl TraceEvent {
    fn after_ms(&self) -> u64 {
        match self {
            Self::Key { after_ms, .. }
            | Self::Resize { after_ms, .. }
            | Self::Tick { after_ms }
            | Self::Idle { after_ms }
            | Self::Redacted { after_ms, .. }
            | Self::Focus { after_ms, .. }
            | Self::Shutdown { after_ms } => *after_ms,
        }
    }

    /// Redacted records become empty paste/mouse payloads: the recorder
    /// prints those as `redacted`, which is what the fixture stores.
    fn record(&self) -> TraceRecord {
        match self {
            Self::Key {
                code,
                phase,
                modifiers,
                repeat_count,
                ..
            } => TraceRecord::Transport(TransportEvent::Key {
                key: TransportKey {
                    code: code.clone(),
                    modifiers: Modifiers::from_bits(*modifiers),
                },
                phase: *phase,
                repeat_count: *repeat_count,
            }),
            Self::Resize { width, height, .. } => TraceRecord::Transport(TransportEvent::Resize {
                width: *width,
                height: *height,
            }),
            Self::Focus { gained: true, .. } => TraceRecord::Transport(TransportEvent::FocusGained),
            Self::Focus { gained: false, .. } => TraceRecord::Transport(TransportEvent::FocusLost),
            Self::Shutdown { .. } => TraceRecord::Transport(TransportEvent::Shutdown),
            Self::Redacted { kind: "mouse", .. } => {
                TraceRecord::Transport(TransportEvent::Mouse(String::new()))
            }
            Self::Redacted { .. } => TraceRecord::Transport(TransportEvent::Paste(String::new())),
            Self::Tick { .. } => TraceRecord::Tick,
            Self::Idle { .. } => TraceRecord::Idle,
        }
    }
}

impl ReplayTrace {
    /// Writes the trace through the production recorder so the fixture
    /// grammar has exactly one writer.
    fn fixture(&self) -> String {
        let mut recorder = SanitizedTraceRecorder::new(Duration::ZERO);
        let mut now = Duration::ZERO;
        for event in &self.events {
            now = now.saturating_add(Duration::from_millis(event.after_ms()));
            match event.record() {
                TraceRecord::Transport(transport) => recorder.record(now, &transport),
                TraceRecord::Tick => recorder.record_tick(now),
                TraceRecord::Idle => recorder.record_idle(now),
            }
        }
        recorder.finish()
    }

    fn parse(fixture: &str) -> Result<Self, FixtureError> {
        let mut lines = fixture.lines();
        if lines.next() != Some("nooise-replay-v1") {
            return Err(FixtureError::VERSION);
        }
        let mut events = Vec::new();
        for (line_index, line) in lines.enumerate() {
            let mut parts = line.split_whitespace();
            let after_ms = parts
                .next()
                .and_then(|part| part.strip_prefix('+'))
                .ok_or(FixtureError::at(line_index + 1, "missing clock advance"))?
                .parse()
                .map_err(|_| FixtureError::at(line_index + 1, "invalid clock advance"))?;
            let kind = parts
                .next()
                .ok_or(FixtureError::at(line_index + 1, "missing event kind"))?;
            let event = match kind {
                "key" => {
                    let code = parts
                        .next()
                        .and_then(decode_physical_key)
                        .ok_or(FixtureError::at(line_index + 1, "invalid key"))?;
                    let phase = parts
                        .next()
                        .and_then(parse_phase)
                        .ok_or(FixtureError::at(line_index + 1, "invalid phase"))?;
                    let modifiers = parts
                        .next()
                        .and_then(|part| part.strip_prefix("mods:"))
                        .and_then(|bits| bits.parse().ok())
                        .filter(|bits| *bits <= 0b11_1111)
                        .ok_or(FixtureError::at(line_index + 1, "invalid modifiers"))?;
                    let repeat_count = parts
                        .next()
                        .and_then(|part| part.strip_prefix("repeats:"))
                        .and_then(|count| count.parse().ok())
                        .filter(|count| *count > 0)
                        .ok_or(FixtureError::at(line_index + 1, "invalid repeat count"))?;
                    TraceEvent::Key {
                        after_ms,
                        code,
                        phase,
                        modifiers,
                        repeat_count,
                    }
                }
                "resize" => {
                    let dimensions = parts
                        .next()
                        .ok_or(FixtureError::at(line_index + 1, "missing dimensions"))?;
                    let (width, height) = dimensions
                        .split_once('x')
                        .ok_or(FixtureError::at(line_index + 1, "invalid dimensions"))?;
                    TraceEvent::Resize {
                        after_ms,
                        width: width
                            .parse()
                            .map_err(|_| FixtureError::at(line_index + 1, "invalid width"))?,
                        height: height
                            .parse()
                            .map_err(|_| FixtureError::at(line_index + 1, "invalid height"))?,
                    }
                }
                "tick" => TraceEvent::Tick { after_ms },
                "idle" => TraceEvent::Idle { after_ms },
                "redacted" => {
                    let kind = parts
                        .next()
                        .ok_or(FixtureError::at(line_index + 1, "missing redacted kind"))?;
                    let kind = match kind {
                        "paste" => "paste",
                        "mouse" => "mouse",
                        _ => return Err(FixtureError::at(line_index + 1, "invalid redacted kind")),
                    };
                    TraceEvent::Redacted { after_ms, kind }
                }
                "focus-gained" => TraceEvent::Focus {
                    after_ms,
                    gained: true,
                },
                "focus-lost" => TraceEvent::Focus {
                    after_ms,
                    gained: false,
                },
                "shutdown" => TraceEvent::Shutdown { after_ms },
                _ => return Err(FixtureError::at(line_index + 1, "invalid event kind")),
            };
            if parts.next().is_some() {
                return Err(FixtureError::at(line_index + 1, "trailing fixture data"));
            }
            events.push(event);
        }
        Ok(Self { events })
    }
}

/// A fixture line the parser could not read.
///
/// Nothing branches on a parse failure — fixtures are authored, not received —
/// so the payload is a diagnostic: which line, and what the parser expected
/// there. `expected` is a compile-time constant rather than a formatted string
/// so the error's identity stays data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FixtureError {
    /// One-based fixture line, or `None` for the version header.
    line: Option<usize>,
    expected: &'static str,
}

impl FixtureError {
    const VERSION: Self = Self {
        line: None,
        expected: "unsupported or missing replay fixture version",
    };

    fn at(line: usize, expected: &'static str) -> Self {
        Self {
            line: Some(line),
            expected,
        }
    }
}

impl fmt::Display for FixtureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "line {line}: {}", self.expected),
            None => write!(f, "{}", self.expected),
        }
    }
}

impl std::error::Error for FixtureError {}

/// One step of a scripted session: a clock advance, a transport event, or an
/// explicit tick/idle boundary. Boundaries are part of the fixture because
/// where a turn ends is itself behavior under test.
enum PlaybackEvent {
    Advance(Duration),
    Transport(TransportEvent),
    TickBoundary,
    IdleBoundary,
}

/// Plays a `ReplayTrace` back as an `EventSource` against a `FakeClock`, so
/// the production scheduler sees the same shape of input it sees live.
struct ScriptedSource {
    clock: FakeClock,
    events: VecDeque<PlaybackEvent>,
    tick_boundaries: usize,
    idle_boundaries: usize,
}

impl ScriptedSource {
    fn new(trace: &ReplayTrace, _capabilities: TerminalCapabilities, clock: FakeClock) -> Self {
        let mut events = VecDeque::new();
        for event in &trace.events {
            events.push_back(PlaybackEvent::Advance(Duration::from_millis(
                event.after_ms(),
            )));
            match event.record() {
                // A redacted payload was never captured, so there is nothing
                // to play back; only its clock advance survives.
                TraceRecord::Transport(TransportEvent::Paste(_) | TransportEvent::Mouse(_)) => {}
                TraceRecord::Transport(transport) => {
                    events.push_back(PlaybackEvent::Transport(transport));
                }
                TraceRecord::Tick => events.push_back(PlaybackEvent::TickBoundary),
                TraceRecord::Idle => events.push_back(PlaybackEvent::IdleBoundary),
            }
        }
        Self {
            clock,
            events,
            tick_boundaries: 0,
            idle_boundaries: 0,
        }
    }

    fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    fn take_boundaries(&mut self) -> (usize, usize) {
        let boundaries = (self.tick_boundaries, self.idle_boundaries);
        self.tick_boundaries = 0;
        self.idle_boundaries = 0;
        boundaries
    }
}

impl EventSource for ScriptedSource {
    fn poll(&mut self, timeout: Duration) -> io::Result<bool> {
        if let Some(PlaybackEvent::Advance(duration)) = self.events.front_mut() {
            if *duration > timeout {
                self.clock.advance(timeout);
                *duration = duration.saturating_sub(timeout);
                return Ok(false);
            }
            self.clock.advance(*duration);
            self.events.pop_front();
        }
        match self.events.front() {
            Some(PlaybackEvent::TickBoundary) => {
                self.events.pop_front();
                self.tick_boundaries += 1;
                return Ok(false);
            }
            Some(PlaybackEvent::IdleBoundary) => {
                self.events.pop_front();
                self.idle_boundaries += 1;
                return Ok(false);
            }
            _ => {}
        }
        Ok(matches!(
            self.events.front(),
            Some(PlaybackEvent::Transport(_))
        ))
    }

    fn read(&mut self) -> io::Result<Option<TransportEvent>> {
        self.clock.advance(EVENT_COST);
        match self.events.pop_front() {
            Some(PlaybackEvent::Transport(event)) => Ok(Some(event)),
            _ => Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "script has no transport event",
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FrameRecord {
    completed_at: Duration,
    owner: String,
    help: String,
    activity: String,
    width: u16,
    height: u16,
    symbols: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReplayResult {
    model: InteractionModel,
    session_generation: u64,
    control_bits: Vec<(&'static str, u32)>,
    automation_kind: Option<String>,
    automation_address: Option<&'static str>,
    auto_running: bool,
    recent_ids: Vec<&'static str>,
    effects: Vec<String>,
    effect_notice: Option<String>,
    pending_edits: usize,
    pending_target_bits: Option<u64>,
    frames: Vec<FrameRecord>,
    max_queue: usize,
    deferred_inputs: Vec<String>,
    clipboard_writes: usize,
    state_history: Vec<ActionRecord>,
    explicit_ticks: usize,
    explicit_tick_turn_ids: Vec<u64>,
    idle_boundaries: usize,
    scheduler_turn_ids: Vec<u64>,
    idle_turn_ids: Vec<u64>,
    telemetry_beat_bits: u64,
}

impl ReplayResult {
    /// How many executed effects carry `prefix` (an `InteractionEffect`
    /// debug name, optionally with its acknowledgement).
    fn effect_count(&self, prefix: &str) -> usize {
        self.effects
            .iter()
            .filter(|effect| effect.starts_with(prefix))
            .count()
    }

    /// The control's final value, or `None` if no spec has that id.
    fn control(&self, id: &str) -> Option<f32> {
        self.control_bits
            .iter()
            .find_map(|(known, bits)| (*known == id).then(|| f32::from_bits(*bits)))
    }

    /// Keyboard owner label of the last rendered frame.
    fn final_owner(&self) -> Option<&str> {
        self.frames.last().map(|frame| frame.owner.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ActionRecord {
    action: SemanticAction,
    before: InteractionModel,
    after: InteractionModel,
    effects: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DivergenceSignature {
    field: String,
    left: String,
    right: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PropertyViolation {
    SourceError {
        kind: io::ErrorKind,
        message: String,
    },
    SchedulerDidNotConverge {
        turn_limit: usize,
    },
    KeyboardOwnerMismatch {
        expected: String,
        observed: String,
    },
    AcceptedFrameDeadlineExceeded {
        elapsed: Duration,
    },
    QueueCapacityExceeded {
        observed: usize,
        capacity: usize,
    },
    FrameGapExceeded {
        gap: Duration,
    },
    NondeterministicReplay {
        signature: DivergenceSignature,
    },
    EdgeChangedOnNonPress {
        record: Box<ActionRecord>,
    },
    RenderError {
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReplayOutcome {
    result: ReplayResult,
    violation: Option<PropertyViolation>,
}

#[derive(Default)]
struct FakeClipboard {
    writes: usize,
    failure: Option<ClipboardError>,
}

impl Clipboard for FakeClipboard {
    fn set_text(&mut self, _text: String) -> Result<(), ClipboardError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        self.writes += 1;
        Ok(())
    }
}

/// One replay run: real normalizer, real scheduler, real kernel, real effect
/// executor, real view projection, and a Ratatui `TestBackend`.
///
/// Nothing here may shortcut the production path — a harness that reached past
/// the mapper would let a binding regression pass. Only the clock, the input
/// source, and the clipboard are substituted, and each is a seam production
/// also goes through.
struct ReplayHarness {
    capabilities: TerminalCapabilities,
    model: InteractionModel,
    executor: EffectExecutor,
    scheduler: Scheduler,
    clock: FakeClock,
    fluid: RippleField,
    telemetry: FluidTelemetry,
    flipped: FlippedUnits,
    width: u16,
    height: u16,
    effects: Vec<String>,
    frames: Vec<FrameRecord>,
    max_queue: usize,
    deferred_inputs: Vec<String>,
    clipboard: FakeClipboard,
    state_history: Vec<ActionRecord>,
    requested_at: Option<Duration>,
    explicit_ticks: usize,
    explicit_tick_turn_ids: Vec<u64>,
    idle_boundaries: usize,
    scheduler_turn_ids: Vec<u64>,
    idle_turn_ids: Vec<u64>,
    violation: Option<PropertyViolation>,
}

/// Every replay rolls the same dice, so two runs of one trace agree.
const REPLAY_RNG_SEED: u64 = 0x6E6F_6F69_7365;

impl ReplayHarness {
    fn new(capabilities: TerminalCapabilities) -> Self {
        let session =
            LiveSession::new(LiveSessionSnapshot::from_controls(FluidControls::default()));
        // Replay has no audio callback, so acknowledge capture requests with
        // a deterministic empty bank newer than every request in the test.
        let executor = EffectExecutor::seeded(
            session,
            AutoControls::new(no_morph(), decode_auto_states(), DEFAULT_AUTO_BARS),
            REPLAY_RNG_SEED,
        );
        let clock = FakeClock::new();
        Self {
            capabilities,
            model: InteractionModel::default(),
            executor,
            scheduler: Scheduler::new(SchedulerConfig::default(), Duration::ZERO),
            clock,
            fluid: RippleField::new(),
            telemetry: FluidTelemetry::default(),
            flipped: FlippedUnits::new(),
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            effects: Vec::new(),
            frames: Vec::new(),
            max_queue: 0,
            deferred_inputs: Vec::new(),
            clipboard: FakeClipboard::default(),
            state_history: Vec::new(),
            requested_at: Some(Duration::ZERO),
            explicit_ticks: 0,
            explicit_tick_turn_ids: Vec::new(),
            idle_boundaries: 0,
            scheduler_turn_ids: Vec::new(),
            idle_turn_ids: Vec::new(),
            violation: None,
        }
    }

    fn with_model(mut self, model: InteractionModel) -> Self {
        self.model = model;
        self
    }

    fn with_session_edit(mut self, edit: impl FnMut(&mut LiveSessionSnapshot)) -> Self {
        self.executor.edit_session(None, edit);
        self
    }

    fn with_auto_running(mut self) -> Self {
        self.executor.toggle_auto(0.0);
        self
    }

    fn with_clipboard_failure(mut self, error: ClipboardError) -> Self {
        self.clipboard.failure = Some(error);
        self
    }

    fn replay(mut self, trace: &ReplayTrace) -> ReplayOutcome {
        let mut source = ScriptedSource::new(trace, self.capabilities, self.clock.clone());
        let mut turns = 0;
        let turn_limit = trace.events.len().saturating_mul(256).saturating_add(512);
        while !source.is_empty()
            || self.scheduler.queue_len() > 0
            || self.scheduler.render_due(self.clock.now())
            || self.scheduler.tick_due(self.clock.now())
        {
            turns += 1;
            self.scheduler_turn_ids.push(turns as u64);
            if turns > turn_limit {
                self.violate(PropertyViolation::SchedulerDidNotConverge { turn_limit });
                break;
            }
            let turn = match self.scheduler.collect_turn(&mut source, &self.clock) {
                Ok(turn) => turn,
                Err(error) => {
                    self.violate(PropertyViolation::SourceError {
                        kind: error.kind(),
                        message: error.to_string(),
                    });
                    break;
                }
            };
            // Production advances this clock from the audio callback. Replay
            // has no callback, so publish the deterministic fixture clock at
            // the same seam before effects sample gesture envelopes.
            self.executor
                .session()
                .publish_audio_seconds(self.clock.now().as_secs_f64());
            let shutdown_seen = turn.shutdown_seen;
            let (explicit_ticks, idle_boundaries) = source.take_boundaries();
            self.idle_boundaries += idle_boundaries;
            self.idle_turn_ids
                .extend(std::iter::repeat_n(turns as u64, idle_boundaries));
            for _ in 0..explicit_ticks {
                self.tick();
                coordinate_production_tick(&mut self.executor, self.clock.now().as_secs_f64())
                    .expect("pending commit has no fallible effects");
                self.explicit_ticks += 1;
                self.explicit_tick_turn_ids.push(turns as u64);
            }
            self.max_queue = self.max_queue.max(self.scheduler.queue_len());
            self.max_queue = self.max_queue.max(turn.events.len());
            for event in &turn.events {
                if let TransportEvent::Resize { width, height } = event {
                    self.width = (*width).max(MIN_TERMINAL_WIDTH);
                    self.height = (*height).max(MIN_TERMINAL_HEIGHT);
                    self.scheduler.request_frame();
                    self.requested_at.get_or_insert(self.clock.now());
                }
            }
            let production = super::coordinator::coordinate_production_turn(
                &mut self.model,
                &turn.events,
                turn.tick_due,
                &mut ProductionCoordinatorContext {
                    effects: &mut self.executor,
                    fluid: &self.fluid,
                    flipped: &mut self.flipped,
                    clipboard: &mut self.clipboard,
                    capabilities: self.capabilities,
                    beat: self.clock.now().as_secs_f64(),
                    active_chord: 0,
                },
            )
            .expect("pending commit has no fallible effects");
            for step in production.steps {
                self.consume_production_step(step);
                if self.violation.is_some() {
                    break;
                }
            }
            if turn.tick_due {
                self.tick();
                self.scheduler.complete_tick(self.clock.now());
            }
            if self.violation.is_some() {
                break;
            }
            if turn.render_due {
                self.render();
            }
            if shutdown_seen || self.violation.is_some() {
                break;
            }
        }
        if self.violation.is_none() && self.scheduler.render_due(self.clock.now()) {
            self.render();
        }
        let mut violation = self.violation.take();
        let session = self.executor.session().load();
        let auto_running = self
            .executor
            .auto_position(self.clock.now().as_secs_f64())
            .is_some();
        let recent_ids = self.executor.recent().ids().to_vec();
        let result = ReplayResult {
            model: self.model,
            session_generation: session.generation,
            control_bits: all_specs()
                .map(|spec| (spec.id, (spec.get)(&session.controls).to_bits()))
                .collect(),
            automation_kind: session
                .automation
                .active_kind()
                .map(|kind| format!("{kind:?}")),
            automation_address: session.automation.active_address().map(ControlAddress::id),
            auto_running,
            recent_ids,
            effects: self.effects,
            effect_notice: self.executor.message().map(str::to_string),
            pending_edits: self.executor.pending().map_or(0, |(_, edits)| edits.len()),
            pending_target_bits: self.executor.pending().map(|(beat, _)| beat.to_bits()),
            frames: self.frames,
            max_queue: self.max_queue,
            deferred_inputs: self.deferred_inputs,
            clipboard_writes: self.clipboard.writes,
            state_history: self.state_history,
            explicit_ticks: self.explicit_ticks,
            explicit_tick_turn_ids: self.explicit_tick_turn_ids,
            idle_boundaries: self.idle_boundaries,
            scheduler_turn_ids: self.scheduler_turn_ids,
            idle_turn_ids: self.idle_turn_ids,
            telemetry_beat_bits: self.telemetry.beat().to_bits(),
        };
        if violation.is_none() {
            violation = post_replay_violation(&result);
        }
        ReplayOutcome { result, violation }
    }

    /// The beat-and-ripple half of a tick, shared by explicit fixture ticks
    /// and scheduler-due ticks. Who commits pending edits differs: an explicit
    /// tick commits here, a due tick already committed inside its turn.
    fn tick(&mut self) {
        self.telemetry.publish_beat(self.clock.now().as_secs_f64());
        self.fluid.tick(
            SchedulerConfig::default().tick_interval.as_secs_f32(),
            &self.telemetry,
        );
    }

    fn consume_production_step(&mut self, step: ProductionStep) {
        if let InputMapping::Deferred(reason) = step.mapping {
            self.deferred_inputs.push(format!("{reason:?}"));
        }
        for action in step.actions {
            let changed = action.before != action.after || !action.effects.is_empty();
            let edge_changed = action.action.phase != InputPhase::Press
                && action.action.intent.phase_policy() == PhasePolicy::Edge
                && changed;
            let mut effect_labels = Vec::new();
            for record in action.effects {
                let result = match record.result {
                    Ok(acknowledgement) => acknowledgement_label(&acknowledgement),
                    Err(failure) => format!("ERR:{failure:?}"),
                };
                let label = format!("{:?}=>{result}", record.effect);
                self.effects.push(label.clone());
                effect_labels.push(label);
            }
            let record = ActionRecord {
                action: action.action,
                before: action.before,
                after: action.after,
                effects: effect_labels,
            };
            self.state_history.push(record.clone());
            if edge_changed {
                self.violate(PropertyViolation::EdgeChangedOnNonPress {
                    record: Box::new(record),
                });
            }
            self.check_legal();
            if changed {
                self.scheduler.request_frame();
                self.requested_at.get_or_insert(self.clock.now());
            }
        }
    }

    fn project<'a>(&'a self, session: &'a LiveSessionSnapshot) -> UiViewModel<'a> {
        UiViewModel::project(ViewProjection {
            interaction: &self.model,
            session,
            telemetry: TelemetryView {
                beat: self.clock.now().as_secs_f64(),
                active_chord: 0,
            },
            presentation: ViewPresentation {
                fluid: &self.fluid,
                flipped: &self.flipped,
                cursor_visible: true,
                notices: ViewNotices::default(),
                gesture_now_seconds: self.clock.now().as_secs_f64(),
                gesture_holds_available: self.capabilities.supports_holds(),
            },
        })
    }

    fn check_legal(&mut self) {
        let session = self.executor.session().load();
        let view = self.project(&session);
        let owner = view.owner.label().to_string();
        let expected = super::view::keyboard_owner(&self.model.mode)
            .label()
            .to_string();
        if owner != expected {
            self.violate(PropertyViolation::KeyboardOwnerMismatch {
                expected,
                observed: owner,
            });
        }
    }

    fn render(&mut self) {
        let now = self.clock.now();
        if let Some(requested_at) = self.requested_at {
            let elapsed = now.saturating_sub(requested_at);
            if elapsed > MAX_FRAME_GAP {
                self.violate(PropertyViolation::AcceptedFrameDeadlineExceeded { elapsed });
                return;
            }
        }
        if let Some(previous) = self.frames.last() {
            let gap = now.saturating_sub(previous.completed_at);
            if gap > MAX_FRAME_GAP {
                self.violate(PropertyViolation::FrameGapExceeded { gap });
                return;
            }
        }
        let session = self.executor.session().load();
        let view = self.project(&session);
        let owner = view.owner.label().to_string();
        let help = view.help.text().to_string();
        let activity = view.activity.clone();
        let mut terminal = match Terminal::new(TestBackend::new(self.width, self.height)) {
            Ok(terminal) => terminal,
            Err(error) => {
                self.violate(PropertyViolation::RenderError {
                    message: error.to_string(),
                });
                return;
            }
        };
        if let Err(error) = terminal.draw(|frame| render(frame, &view)) {
            self.violate(PropertyViolation::RenderError {
                message: error.to_string(),
            });
            return;
        }
        let buffer = terminal.backend().buffer();
        let symbols = (0..self.height)
            .map(|y| {
                (0..self.width)
                    .map(|x| {
                        let cell = &buffer[(x, y)];
                        format!("{}:{:?}", cell.symbol(), cell.style())
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        self.frames.push(FrameRecord {
            completed_at: now,
            owner,
            help,
            activity,
            width: self.width,
            height: self.height,
            symbols,
        });
        self.scheduler.complete_frame(now);
        self.requested_at = None;
    }

    fn violate(&mut self, violation: PropertyViolation) {
        if self.violation.is_none() {
            self.violation = Some(violation);
        }
    }
}

fn acknowledgement_label(acknowledgement: &EffectAcknowledgement) -> String {
    format!("OK:{acknowledgement:?}")
}

fn key(after_ms: u64, code: FixtureKey, phase: InputPhase) -> TraceEvent {
    TraceEvent::Key {
        after_ms,
        code,
        phase,
        modifiers: 0,
        repeat_count: 1,
    }
}

fn modified_key(after_ms: u64, code: FixtureKey, phase: InputPhase, modifiers: u8) -> TraceEvent {
    let mut event = key(after_ms, code, phase);
    if let TraceEvent::Key {
        modifiers: event_modifiers,
        ..
    } = &mut event
    {
        *event_modifiers = modifiers;
    }
    event
}

/// Replays `trace` on a harness shaped by `configure`, delta-reducing and
/// panicking on any property violation.
fn replay_with(
    trace: &[TraceEvent],
    capabilities: TerminalCapabilities,
    configure: impl Fn(ReplayHarness) -> ReplayHarness,
) -> ReplayResult {
    checked_replay(trace, |candidate| {
        let outcome = configure(ReplayHarness::new(capabilities)).replay(&ReplayTrace {
            events: candidate.to_vec(),
        });
        Observation {
            violation: outcome.violation,
            result: outcome.result,
            mirror: None,
        }
    })
}

fn replay(trace: &[TraceEvent], capabilities: TerminalCapabilities) -> ReplayResult {
    replay_with(trace, capabilities, |harness| harness)
}

fn replay_from_model(
    model: InteractionModel,
    trace: &[TraceEvent],
    capabilities: TerminalCapabilities,
) -> ReplayResult {
    replay_with(trace, capabilities, |harness| {
        harness.with_model(model.clone())
    })
}

/// What one check of a trace saw: the replay result, a second result when
/// the check compared two runs, and the violation it classified.
struct Observation {
    result: ReplayResult,
    mirror: Option<ReplayResult>,
    violation: Option<PropertyViolation>,
}

/// Observes `trace`; on a violation, delta-reduces the trace to the smallest
/// one reproducing that exact violation and panics with the diagnostic.
fn checked_replay(
    trace: &[TraceEvent],
    mut observe: impl FnMut(&[TraceEvent]) -> Observation,
) -> ReplayResult {
    let observed = observe(trace);
    let Some(violation) = observed.violation else {
        return observed.result;
    };
    let minimal = minimize_trace(trace.to_vec(), |candidate| {
        observe(candidate).violation.as_ref() == Some(&violation)
    });
    let minimized = observe(&minimal);
    let minimized_violation = minimized
        .violation
        .filter(|candidate| *candidate == violation)
        .unwrap_or(violation);
    panic!(
        "{}",
        format_property_diagnostic(
            &minimized_violation,
            &minimal,
            &minimized.result,
            minimized.mirror.as_ref().unwrap_or(&minimized.result),
        )
    );
}

#[test]
fn sanitized_trace_fixture_round_trips_without_user_payloads() {
    let mut recorder = SanitizedTraceRecorder::new(Duration::ZERO);
    let mut repeated = normalize_key_event(
        KeyEvent {
            code: KeyCode::Char('p'),
            modifiers: KeyModifiers::SHIFT | KeyModifiers::CONTROL,
            kind: KeyEventKind::Repeat,
            state: KeyEventState::NONE,
        },
        TerminalCapabilities::full(),
    )
    .expect("full capabilities report every key event");
    if let TransportEvent::Key { repeat_count, .. } = &mut repeated {
        *repeat_count = 4;
    }
    let events = [
        (
            1,
            normalize_key_event(
                KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
                TerminalCapabilities::full(),
            )
            .expect("full capabilities report every key event"),
        ),
        (2, repeated),
        (
            3,
            normalize_key_event(
                KeyEvent {
                    code: KeyCode::Char('s'),
                    modifiers: KeyModifiers::ALT,
                    kind: KeyEventKind::Release,
                    state: KeyEventState::NONE,
                },
                TerminalCapabilities::full(),
            )
            .expect("full capabilities report every key event"),
        ),
        (
            4,
            TransportEvent::Resize {
                width: 62,
                height: 14,
            },
        ),
        (5, TransportEvent::FocusGained),
        (6, TransportEvent::FocusLost),
        (7, TransportEvent::Paste("secret song code".into())),
        (8, TransportEvent::Mouse("secret mouse details".into())),
        (
            9,
            normalize_key_event(
                KeyEvent::new(KeyCode::F(7), KeyModifiers::NONE),
                TerminalCapabilities::full(),
            )
            .expect("full capabilities report every key event"),
        ),
        (10, TransportEvent::Shutdown),
    ];
    for (at_ms, event) in &events {
        recorder.record(Duration::from_millis(*at_ms), event);
    }
    recorder.record_tick(Duration::from_millis(11));
    recorder.record_idle(Duration::from_millis(12));
    let fixture = recorder.finish();
    let expected = ReplayTrace {
        events: vec![
            key(1, FixtureKey::Character('q'), InputPhase::Press),
            TraceEvent::Key {
                after_ms: 1,
                code: FixtureKey::Character('p'),
                phase: InputPhase::Repeat,
                modifiers: 3,
                repeat_count: 4,
            },
            TraceEvent::Key {
                after_ms: 1,
                code: FixtureKey::Character('s'),
                phase: InputPhase::Release,
                modifiers: 4,
                repeat_count: 1,
            },
            TraceEvent::Resize {
                after_ms: 1,
                width: 62,
                height: 14,
            },
            TraceEvent::Focus {
                after_ms: 1,
                gained: true,
            },
            TraceEvent::Focus {
                after_ms: 1,
                gained: false,
            },
            TraceEvent::Redacted {
                after_ms: 1,
                kind: "paste",
            },
            TraceEvent::Redacted {
                after_ms: 1,
                kind: "mouse",
            },
            TraceEvent::Key {
                after_ms: 1,
                code: PhysicalKey::Function(7),
                phase: InputPhase::Press,
                modifiers: 0,
                repeat_count: 1,
            },
            TraceEvent::Shutdown { after_ms: 1 },
            TraceEvent::Tick { after_ms: 1 },
            TraceEvent::Idle { after_ms: 1 },
        ],
    };
    assert!(!fixture.contains("secret"));
    assert_eq!(ReplayTrace::parse(&fixture), Ok(expected));
    assert!(ReplayTrace::parse("+0 tick\n").is_err());
    assert!(ReplayTrace::parse("nooise-replay-v2\n+0 tick\n").is_err());
    assert!(
        ReplayTrace::parse("nooise-replay-v1\n+0 semantic hold:1 press\n").is_err(),
        "semantic intents must not enter the persisted fixture grammar"
    );
    for invalid_bits in [64, 255] {
        assert!(
            ReplayTrace::parse(&format!(
                "nooise-replay-v1\n+0 key char:000078 press mods:{invalid_bits} repeats:1\n"
            ))
            .is_err(),
            "modifier bits outside the six-bit transport domain must be rejected"
        );
    }
}

#[test]
fn every_normalized_physical_key_identity_round_trips_through_replay() {
    let mut codes = vec![
        KeyCode::Backspace,
        KeyCode::Enter,
        KeyCode::Left,
        KeyCode::Right,
        KeyCode::Up,
        KeyCode::Down,
        KeyCode::Home,
        KeyCode::End,
        KeyCode::PageUp,
        KeyCode::PageDown,
        KeyCode::Tab,
        KeyCode::BackTab,
        KeyCode::Delete,
        KeyCode::Insert,
        KeyCode::F(1),
        KeyCode::F(255),
        KeyCode::Char(' '),
        KeyCode::Char('é'),
        KeyCode::Null,
        KeyCode::Esc,
        KeyCode::CapsLock,
        KeyCode::ScrollLock,
        KeyCode::NumLock,
        KeyCode::PrintScreen,
        KeyCode::Pause,
        KeyCode::Menu,
        KeyCode::KeypadBegin,
    ];
    codes.extend(
        [
            MediaKeyCode::Play,
            MediaKeyCode::Pause,
            MediaKeyCode::PlayPause,
            MediaKeyCode::Reverse,
            MediaKeyCode::Stop,
            MediaKeyCode::FastForward,
            MediaKeyCode::Rewind,
            MediaKeyCode::TrackNext,
            MediaKeyCode::TrackPrevious,
            MediaKeyCode::Record,
            MediaKeyCode::LowerVolume,
            MediaKeyCode::RaiseVolume,
            MediaKeyCode::MuteVolume,
        ]
        .into_iter()
        .map(KeyCode::Media),
    );
    codes.extend(
        [
            ModifierKeyCode::LeftShift,
            ModifierKeyCode::LeftControl,
            ModifierKeyCode::LeftAlt,
            ModifierKeyCode::LeftSuper,
            ModifierKeyCode::LeftHyper,
            ModifierKeyCode::LeftMeta,
            ModifierKeyCode::RightShift,
            ModifierKeyCode::RightControl,
            ModifierKeyCode::RightAlt,
            ModifierKeyCode::RightSuper,
            ModifierKeyCode::RightHyper,
            ModifierKeyCode::RightMeta,
            ModifierKeyCode::IsoLevel3Shift,
            ModifierKeyCode::IsoLevel5Shift,
        ]
        .into_iter()
        .map(KeyCode::Modifier),
    );

    for code in codes {
        for (kind, phase) in [
            (KeyEventKind::Press, InputPhase::Press),
            (KeyEventKind::Repeat, InputPhase::Repeat),
            (KeyEventKind::Release, InputPhase::Release),
        ] {
            for modifiers in [KeyModifiers::NONE, KeyModifiers::all()] {
                for repeat_count in [1, 3] {
                    let mut normalized = normalize_key_event(
                        KeyEvent {
                            code,
                            modifiers,
                            kind,
                            state: KeyEventState::NONE,
                        },
                        TerminalCapabilities::full(),
                    )
                    .expect("full capabilities report every key event");
                    let TransportEvent::Key {
                        repeat_count: normalized_count,
                        ..
                    } = &mut normalized
                    else {
                        unreachable!("raw key normalization must produce a key");
                    };
                    *normalized_count = repeat_count;
                    let normalized_code = match &normalized {
                        TransportEvent::Key { key, .. } => key.code.clone(),
                        _ => unreachable!("raw key normalization must produce a key"),
                    };
                    let mut recorder = SanitizedTraceRecorder::new(Duration::ZERO);
                    recorder.record(Duration::ZERO, &normalized);
                    let mut trace =
                        ReplayTrace::parse(&recorder.finish()).expect("recorded key must parse");
                    let TraceEvent::Key {
                        code: parsed_code,
                        phase: parsed_phase,
                        modifiers: parsed_modifiers,
                        repeat_count: parsed_count,
                        ..
                    } = &trace.events[0]
                    else {
                        unreachable!("recorded key must parse as key");
                    };
                    assert_eq!(*parsed_code, normalized_code);
                    assert_eq!(*parsed_phase, phase);
                    assert_eq!(*parsed_modifiers, if modifiers.is_empty() { 0 } else { 63 });
                    assert_eq!(*parsed_count, repeat_count);

                    // Return any mode entered by the identity to Browsing,
                    // then force a real ordered effect so every matrix row
                    // crosses the kernel and executor as well as render.
                    trace
                        .events
                        .push(key(0, PhysicalKey::Escape, InputPhase::Press));
                    trace.events.push(modified_key(
                        0,
                        PhysicalKey::Character('s'),
                        InputPhase::Press,
                        1 << 1,
                    ));
                    let outcome = replay_outcome(&trace.events, TerminalCapabilities::full());
                    assert!(
                        outcome.violation.is_none(),
                        "{code:?}/{phase:?}/{modifiers:?}/{repeat_count}: {:?}",
                        outcome.violation
                    );
                    assert!(!outcome.result.frames.is_empty());
                    assert_eq!(outcome.result.clipboard_writes, 1);
                }
            }
        }
    }
    assert!(decode_physical_key("media:fabricated").is_none());
    assert!(decode_physical_key("modifier:fabricated").is_none());
}

#[test]
fn replay_modifier_bits_cover_the_complete_six_bit_transport_domain() {
    for bits in 0_u8..=63 {
        let event = modified_key(0, PhysicalKey::Character('x'), InputPhase::Press, bits);
        let trace = ReplayTrace {
            events: vec![event],
        };
        let parsed = ReplayTrace::parse(&trace.fixture()).expect("fixture must parse");
        assert_eq!(parsed, trace);
        let clock = FakeClock::new();
        let mut source = ScriptedSource::new(&parsed, TerminalCapabilities::full(), clock);
        assert!(source.poll(Duration::ZERO).expect("poll"));
        let Some(TransportEvent::Key { key, .. }) = source.read().expect("read") else {
            panic!("expected key");
        };
        assert_eq!(key.modifiers, Modifiers::from_bits(bits));
    }
}

#[test]
fn scripted_poll_matches_real_timeout_boundaries() {
    for (delay, timeout, expected_ready, expected_now) in
        [(2, 3, true, 2), (3, 3, true, 3), (4, 3, false, 3)]
    {
        let clock = FakeClock::new();
        let trace = ReplayTrace {
            events: vec![key(delay, FixtureKey::Character('q'), InputPhase::Press)],
        };
        let mut source = ScriptedSource::new(&trace, TerminalCapabilities::full(), clock.clone());
        assert_eq!(
            source.poll(Duration::from_millis(timeout)).unwrap(),
            expected_ready
        );
        assert_eq!(clock.now(), Duration::from_millis(expected_now));
        if !expected_ready {
            assert!(source.poll(Duration::from_millis(1)).unwrap());
            assert_eq!(clock.now(), Duration::from_millis(delay));
        }
    }
}

#[test]
fn explicit_tick_processes_and_idle_delimits_scheduler_turns() {
    let result = replay(
        &[
            TraceEvent::Tick { after_ms: 0 },
            TraceEvent::Idle { after_ms: 0 },
            key(0, PhysicalKey::Character('p'), InputPhase::Press),
        ],
        TerminalCapabilities::full(),
    );
    assert_eq!(result.explicit_ticks, 1);
    assert_eq!(result.idle_boundaries, 1);
    assert_eq!(result.telemetry_beat_bits, 0.0_f64.to_bits());
    assert_eq!(result.explicit_tick_turn_ids.len(), 1);
    assert_eq!(result.idle_turn_ids.len(), 1);
    assert_ne!(result.explicit_tick_turn_ids, result.idle_turn_ids);
    assert!(
        result
            .explicit_tick_turn_ids
            .iter()
            .chain(&result.idle_turn_ids)
            .all(|turn| result.scheduler_turn_ids.contains(turn))
    );
    assert_eq!(result.model.mode, InteractionMode::Browsing);
    assert!(result.deferred_inputs.is_empty());
}

#[test]
fn recorded_backtab_round_trips_through_the_full_pipeline() {
    let mut recorder = SanitizedTraceRecorder::new(Duration::ZERO);
    recorder.record(
        Duration::from_millis(2),
        &normalize_key_event(
            KeyEvent {
                code: KeyCode::BackTab,
                modifiers: KeyModifiers::SHIFT,
                kind: KeyEventKind::Press,
                state: KeyEventState::NONE,
            },
            TerminalCapabilities::full(),
        )
        .expect("full capabilities report every key event"),
    );
    let fixture = recorder.finish();
    let trace = ReplayTrace::parse(&fixture).expect("recorder output must parse");
    assert_eq!(
        trace.events,
        vec![TraceEvent::Key {
            after_ms: 2,
            code: FixtureKey::BackTab,
            phase: InputPhase::Press,
            modifiers: 1,
            repeat_count: 1,
        }]
    );
    let result = replay(&trace.events, TerminalCapabilities::full());
    assert!(matches!(result.model.navigation, Navigation::Master { .. }));
    assert!(result.state_history.iter().any(|record| {
        record.action.intent == Intent::ChangePage(super::interaction::PageDirection::Previous)
    }));
}

#[test]
fn scheduler_batches_ready_events_but_wakes_at_frame_deadline() {
    let clock = FakeClock::new();
    let trace = ReplayTrace {
        events: vec![
            key(2, FixtureKey::Down, InputPhase::Press),
            key(0, FixtureKey::Up, InputPhase::Press),
        ],
    };
    let mut source = ScriptedSource::new(&trace, TerminalCapabilities::full(), clock.clone());
    let mut scheduler = Scheduler::new(SchedulerConfig::default(), Duration::ZERO);
    let initial = scheduler.collect_turn(&mut source, &clock).unwrap();
    assert!(initial.render_due);
    scheduler.complete_frame(clock.now());
    let batch = scheduler.collect_turn(&mut source, &clock).unwrap();
    assert_eq!(batch.events.len(), 2);

    let clock = FakeClock::new();
    let trace = ReplayTrace {
        events: vec![key(50, FixtureKey::Character('q'), InputPhase::Press)],
    };
    let mut source = ScriptedSource::new(&trace, TerminalCapabilities::full(), clock.clone());
    let mut scheduler = Scheduler::new(SchedulerConfig::default(), Duration::ZERO);
    assert!(
        scheduler
            .collect_turn(&mut source, &clock)
            .unwrap()
            .render_due
    );
    scheduler.complete_frame(clock.now());
    let deadline = scheduler.collect_turn(&mut source, &clock).unwrap();
    assert!(deadline.render_due);
    assert!(deadline.events.is_empty());
    assert_eq!(clock.now(), SchedulerConfig::default().frame_interval);
}

#[test]
fn regression_traces_cross_the_complete_ui_pipeline() {
    let cases = [
        (
            "original pp",
            vec![
                key(0, FixtureKey::Character('p'), InputPhase::Press),
                key(0, FixtureKey::Character('p'), InputPhase::Press),
            ],
            "BROWSE",
        ),
        (
            "double Space",
            vec![
                key(0, FixtureKey::Character(' '), InputPhase::Press),
                key(0, FixtureKey::Character(' '), InputPhase::Press),
            ],
            "JUMP",
        ),
        (
            "sustained entry key",
            vec![
                key(0, FixtureKey::Character('1'), InputPhase::Press),
                key(0, FixtureKey::Character('1'), InputPhase::Repeat),
                key(0, FixtureKey::Character('1'), InputPhase::Repeat),
                key(0, FixtureKey::Enter, InputPhase::Press),
            ],
            "BROWSE",
        ),
        (
            "rapid action",
            vec![
                key(0, FixtureKey::Right, InputPhase::Press),
                key(0, FixtureKey::Right, InputPhase::Repeat),
                key(0, FixtureKey::Down, InputPhase::Press),
                key(0, FixtureKey::Left, InputPhase::Press),
            ],
            "BROWSE",
        ),
        (
            "resize during input",
            vec![
                key(0, FixtureKey::Down, InputPhase::Press),
                TraceEvent::Resize {
                    after_ms: 0,
                    width: 46,
                    height: 10,
                },
                key(0, FixtureKey::Down, InputPhase::Repeat),
            ],
            "BROWSE",
        ),
        (
            "Escape recovery",
            vec![
                key(0, FixtureKey::Character(' '), InputPhase::Press),
                key(0, FixtureKey::Character('a'), InputPhase::Press),
                key(0, FixtureKey::Character('s'), InputPhase::Press),
                key(0, FixtureKey::Escape, InputPhase::Press),
                key(0, FixtureKey::Escape, InputPhase::Press),
            ],
            "BROWSE",
        ),
    ];

    for (name, trace, expected_owner) in cases {
        let first = replay(&trace, TerminalCapabilities::full());
        let second = replay(&trace, TerminalCapabilities::full());
        assert_eq!(first, second, "{name} was not deterministic");
        assert_eq!(first.final_owner(), Some(expected_owner), "{name}");
        if let Some(violation) = post_replay_violation(&first) {
            panic!(
                "{}",
                format_property_diagnostic(&violation, &trace, &first, &second)
            );
        }
    }
}

#[test]
fn gesture_press_repeat_navigation_and_modified_release_follow_the_production_path() {
    let result = replay(
        &[
            key(0, FixtureKey::Character('z'), InputPhase::Press),
            key(200, FixtureKey::Character('z'), InputPhase::Repeat),
            key(200, FixtureKey::Tab, InputPhase::Press),
            modified_key(200, FixtureKey::Character('z'), InputPhase::Release, 1 << 1),
        ],
        TerminalCapabilities::full(),
    );

    assert!(matches!(
        result.model.navigation,
        Navigation::Standard {
            page: super::interaction::StandardPage::Perc,
            ..
        }
    ));
    assert_eq!(result.effect_count("GesturePress"), 1);
    assert_eq!(result.effect_count("GestureRelease(Bloom)"), 1);
    assert!(
        result
            .effects
            .iter()
            .any(|effect| { effect.starts_with("GesturePress(Bloom)") })
    );
    assert_eq!(
        result
            .state_history
            .iter()
            .filter(|record| matches!(record.action.intent, Intent::StartGesture(_)))
            .count(),
        1,
        "Repeat must not restart the envelope"
    );
}

#[test]
fn gesture_mode_transition_and_focus_loss_release_without_rearming_quarantined_keys() {
    let result = replay(
        &[
            key(0, FixtureKey::Character('z'), InputPhase::Press),
            key(100, FixtureKey::Character('/'), InputPhase::Press),
            key(0, FixtureKey::Escape, InputPhase::Press),
            // The editor closed while the physical key is still down. A
            // repeated Press must remain quarantined until its real key-up.
            key(0, FixtureKey::Character('z'), InputPhase::Press),
            key(100, FixtureKey::Character('z'), InputPhase::Release),
            key(0, FixtureKey::Character('z'), InputPhase::Press),
            TraceEvent::Focus {
                after_ms: 100,
                gained: false,
            },
            key(0, FixtureKey::Character('z'), InputPhase::Repeat),
            key(0, FixtureKey::Character('z'), InputPhase::Release),
        ],
        TerminalCapabilities::full(),
    );

    assert_eq!(result.final_owner(), Some("BROWSE"));
    assert_eq!(result.effect_count("GesturePress"), 2);
    assert_eq!(result.effect_count("GestureReleaseAll"), 2);
    assert_eq!(result.effect_count("GestureRelease("), 0);
    let starts = result
        .state_history
        .iter()
        .filter(|record| matches!(record.action.intent, Intent::StartGesture(_)))
        .collect::<Vec<_>>();
    assert_eq!(starts.len(), 3);
    assert_eq!(
        starts
            .iter()
            .filter(|record| record.effects.is_empty())
            .count(),
        1,
        "the quarantined Press maps normally but cannot restart its gesture"
    );
}

#[test]
fn unsupported_gesture_press_is_inert_and_the_activity_row_stays_blank() {
    let result = replay(
        &[key(0, FixtureKey::Character('z'), InputPhase::Press)],
        TerminalCapabilities::default(),
    );

    assert!(result.effects.is_empty());
    assert!(result.state_history.is_empty());
    assert!(
        result.frames.iter().all(|frame| frame.activity.is_empty()),
        "a terminal without key-up support must never advertise a hold gesture"
    );
}

#[test]
fn production_binding_matrix_crosses_the_complete_pipeline() {
    let plain = |code| key(0, code, InputPhase::Press);
    let ctrl = |code| modified_key(0, code, InputPhase::Press, 1 << 1);
    let shift = |code| modified_key(0, code, InputPhase::Press, 1);
    let cases = [
        ("up", vec![plain(FixtureKey::Up)]),
        ("k", vec![plain(FixtureKey::Character('k'))]),
        ("down", vec![plain(FixtureKey::Down)]),
        ("j", vec![plain(FixtureKey::Character('j'))]),
        ("left", vec![plain(FixtureKey::Left)]),
        ("h", vec![plain(FixtureKey::Character('h'))]),
        ("right", vec![plain(FixtureKey::Right)]),
        ("l", vec![plain(FixtureKey::Character('l'))]),
        ("shift reset", vec![shift(FixtureKey::Left)]),
        ("H reset", vec![shift(FixtureKey::Character('H'))]),
        ("next page", vec![plain(FixtureKey::Tab)]),
        ("previous page", vec![shift(FixtureKey::BackTab)]),
        ("auto", vec![plain(FixtureKey::Character('a'))]),
        ("palette", vec![plain(FixtureKey::Character('/'))]),
        ("lfo", vec![plain(FixtureKey::Character('f'))]),
        ("envelope", vec![plain(FixtureKey::Character('e'))]),
        ("lift gesture", vec![plain(FixtureKey::Character('x'))]),
        ("unit flip", vec![plain(FixtureKey::Character('t'))]),
        ("track mute", vec![plain(FixtureKey::Character('m'))]),
        ("master mute", vec![shift(FixtureKey::Character('M'))]),
        ("clock stop", vec![shift(FixtureKey::Character('P'))]),
        ("randomize", vec![plain(FixtureKey::Character('r'))]),
        ("randomize set", vec![shift(FixtureKey::Character('R'))]),
        ("numeric", vec![plain(FixtureKey::Character('1'))]),
        ("touch", vec![plain(FixtureKey::Enter)]),
        ("save", vec![ctrl(FixtureKey::Character('s'))]),
        ("ctrl-q", vec![ctrl(FixtureKey::Character('q'))]),
        ("ctrl-c", vec![ctrl(FixtureKey::Character('c'))]),
        ("cancel", vec![plain(FixtureKey::Escape)]),
        (
            "automation navigation",
            vec![
                plain(FixtureKey::Character('f')),
                plain(FixtureKey::Down),
                plain(FixtureKey::Right),
                plain(FixtureKey::Character('t')),
                plain(FixtureKey::Character('r')),
                plain(FixtureKey::Escape),
            ],
        ),
        (
            "automation page close",
            vec![plain(FixtureKey::Character('f')), plain(FixtureKey::Tab)],
        ),
        (
            "automation palette",
            vec![
                plain(FixtureKey::Character('f')),
                plain(FixtureKey::Character('/')),
                plain(FixtureKey::Escape),
            ],
        ),
        (
            "numeric grammar and swallowing",
            vec![
                plain(FixtureKey::Character('1')),
                plain(FixtureKey::Character('.')),
                plain(FixtureKey::Character('.')),
                plain(FixtureKey::Character('-')),
                ctrl(FixtureKey::Character('s')),
                plain(FixtureKey::Backspace),
                plain(FixtureKey::Enter),
            ],
        ),
        (
            "palette typing and navigation",
            vec![
                plain(FixtureKey::Character('/')),
                plain(FixtureKey::Character('b')),
                plain(FixtureKey::Tab),
                plain(FixtureKey::Character('4')),
                plain(FixtureKey::Backspace),
                plain(FixtureKey::Up),
                plain(FixtureKey::Down),
                ctrl(FixtureKey::Character('p')),
                ctrl(FixtureKey::Character('n')),
                plain(FixtureKey::Escape),
            ],
        ),
        (
            "resize",
            vec![TraceEvent::Resize {
                after_ms: 0,
                width: 46,
                height: 10,
            }],
        ),
    ];

    struct ExpectedBinding {
        owner: &'static str,
        generation: u64,
        automation: Option<&'static str>,
        intents: Vec<Intent>,
        effects: Vec<&'static str>,
        notice: Option<&'static str>,
    }

    for (name, trace) in cases {
        let outcome = replay_outcome(&trace, TerminalCapabilities::full());
        let expected = match name {
            "up" | "k" => ExpectedBinding {
                owner: "BROWSE",
                generation: 0,
                automation: None,
                intents: vec![Intent::MoveSelection(-1)],
                effects: vec![],
                notice: None,
            },
            "down" | "j" => ExpectedBinding {
                owner: "BROWSE",
                generation: 0,
                automation: None,
                intents: vec![Intent::MoveSelection(1)],
                effects: vec![],
                notice: None,
            },
            "left" | "h" => ExpectedBinding {
                owner: "BROWSE",
                generation: 1,
                automation: None,
                intents: vec![Intent::AdjustSelected(-1)],
                effects: vec!["AdjustSelected(-1)=>OK:Published { generation: 1 }"],
                notice: None,
            },
            "right" | "l" => ExpectedBinding {
                owner: "BROWSE",
                generation: 1,
                automation: None,
                intents: vec![Intent::AdjustSelected(1)],
                effects: vec!["AdjustSelected(1)=>OK:Published { generation: 1 }"],
                notice: None,
            },
            "shift reset" | "H reset" => ExpectedBinding {
                owner: "BROWSE",
                generation: 1,
                automation: None,
                intents: vec![Intent::ResetSelected],
                effects: vec!["ResetSelected=>OK:Published { generation: 1 }"],
                notice: None,
            },
            "next page" => ExpectedBinding {
                owner: "BROWSE",
                generation: 0,
                automation: None,
                intents: vec![Intent::ChangePage(super::interaction::PageDirection::Next)],
                effects: vec![],
                notice: None,
            },
            "previous page" => ExpectedBinding {
                owner: "BROWSE",
                generation: 0,
                automation: None,
                intents: vec![Intent::ChangePage(
                    super::interaction::PageDirection::Previous,
                )],
                effects: vec![],
                notice: None,
            },
            "auto" => ExpectedBinding {
                owner: "BROWSE",
                generation: 1,
                automation: None,
                intents: vec![Intent::ToggleAuto],
                effects: vec!["ToggleAuto=>OK:Published { generation: 1 }"],
                notice: None,
            },
            "palette" => ExpectedBinding {
                owner: "PALETTE",
                generation: 0,
                automation: None,
                intents: vec![Intent::OpenPalette],
                effects: vec![],
                notice: None,
            },
            "lfo" => ExpectedBinding {
                owner: "LFO",
                generation: 1,
                automation: Some("Lfo"),
                intents: vec![Intent::OpenAutomation(AutomationKind::Lfo)],
                effects: vec!["AutomationConfirm(Lfo)=>OK:Published { generation: 1 }"],
                notice: None,
            },
            "envelope" => ExpectedBinding {
                owner: "ENV",
                generation: 1,
                automation: Some("Envelope"),
                intents: vec![Intent::OpenAutomation(AutomationKind::Envelope)],
                effects: vec!["AutomationConfirm(Envelope)=>OK:Published { generation: 1 }"],
                notice: None,
            },
            "lift gesture" => ExpectedBinding {
                owner: "BROWSE",
                generation: 1,
                automation: None,
                intents: vec![Intent::StartGesture(GestureKind::Lift)],
                effects: vec!["GesturePress(Lift)=>OK:Published { generation: 1 }"],
                notice: None,
            },
            "unit flip" => ExpectedBinding {
                owner: "BROWSE",
                generation: 0,
                automation: None,
                intents: vec![Intent::ToggleUnits],
                effects: vec!["ToggleUnits=>OK:Published { generation: 0 }"],
                notice: None,
            },
            "track mute" => ExpectedBinding {
                owner: "BROWSE",
                generation: 1,
                automation: None,
                intents: vec![Intent::ToggleMute { master: false }],
                effects: vec!["ToggleMute { master: false }=>OK:Published { generation: 1 }"],
                notice: None,
            },
            "master mute" => ExpectedBinding {
                owner: "BROWSE",
                generation: 1,
                automation: None,
                intents: vec![Intent::ToggleMute { master: true }],
                effects: vec!["ToggleMute { master: true }=>OK:Published { generation: 1 }"],
                notice: None,
            },
            "clock stop" => ExpectedBinding {
                owner: "BROWSE",
                generation: 1,
                automation: None,
                intents: vec![Intent::ToggleTransport],
                effects: vec!["ToggleTransport=>OK:Published { generation: 1 }"],
                notice: None,
            },
            "randomize" => ExpectedBinding {
                owner: "BROWSE",
                generation: 1,
                automation: None,
                intents: vec![Intent::RandomizeSelected],
                effects: vec!["RandomizeSelected=>OK:Published { generation: 1 }"],
                notice: None,
            },
            "randomize set" => ExpectedBinding {
                owner: "BROWSE",
                generation: 1,
                automation: None,
                intents: vec![Intent::RandomizeScope],
                effects: vec!["RandomizeScope=>OK:Published { generation: 1 }"],
                notice: None,
            },
            "numeric" => ExpectedBinding {
                owner: "NUMERIC",
                generation: 0,
                automation: None,
                intents: vec![Intent::BeginNumeric('1')],
                effects: vec![],
                notice: None,
            },
            "touch" => ExpectedBinding {
                owner: "BROWSE",
                generation: 0,
                automation: None,
                intents: vec![Intent::TouchSelected],
                effects: vec![
                    "TouchSelected=>OK:ControlSelected { tab: Chords, index: 0, id: \"pad.level\" }",
                ],
                notice: None,
            },
            "save" => ExpectedBinding {
                owner: "BROWSE",
                generation: 0,
                automation: None,
                intents: vec![Intent::Save],
                effects: vec!["Save=>OK:Message(\"song code copied to clipboard\")"],
                notice: Some("song code copied to clipboard"),
            },
            "ctrl-q" | "ctrl-c" => ExpectedBinding {
                owner: "BROWSE",
                generation: 0,
                automation: None,
                intents: vec![Intent::Quit],
                effects: vec!["Quit=>OK:QuitRequested"],
                notice: None,
            },
            "cancel" => ExpectedBinding {
                owner: "BROWSE",
                generation: 0,
                automation: None,
                intents: vec![Intent::Cancel],
                effects: vec![],
                notice: None,
            },
            "automation navigation" => ExpectedBinding {
                owner: "BROWSE",
                generation: 5,
                automation: None,
                intents: vec![
                    Intent::OpenAutomation(AutomationKind::Lfo),
                    Intent::MoveSelection(1),
                    Intent::AdjustSelected(1),
                    Intent::ToggleUnits,
                    Intent::RandomizeAutomationRow,
                    Intent::Cancel,
                ],
                effects: vec![
                    "AutomationConfirm(Lfo)=>OK:Published { generation: 1 }",
                    "AdjustSelected(1)=>OK:Published { generation: 2 }",
                    "ToggleUnits=>OK:Published { generation: 3 }",
                    "RandomizeAutomationRow=>OK:Published { generation: 4 }",
                    "CloseAutomationAll=>OK:NoChange",
                ],
                notice: None,
            },
            "automation page close" => ExpectedBinding {
                owner: "BROWSE",
                generation: 2,
                automation: None,
                intents: vec![
                    Intent::OpenAutomation(AutomationKind::Lfo),
                    Intent::ChangePage(super::interaction::PageDirection::Next),
                ],
                effects: vec![
                    "AutomationConfirm(Lfo)=>OK:Published { generation: 1 }",
                    "CloseAutomationAll=>OK:NoChange",
                ],
                notice: None,
            },
            "automation palette" => ExpectedBinding {
                owner: "LFO",
                generation: 1,
                automation: Some("Lfo"),
                intents: vec![
                    Intent::OpenAutomation(AutomationKind::Lfo),
                    Intent::OpenPalette,
                    Intent::Cancel,
                ],
                effects: vec!["AutomationConfirm(Lfo)=>OK:Published { generation: 1 }"],
                notice: None,
            },
            "numeric grammar and swallowing" => ExpectedBinding {
                owner: "BROWSE",
                generation: 1,
                automation: None,
                intents: vec![
                    Intent::BeginNumeric('1'),
                    Intent::TypeCharacter('.'),
                    Intent::TypeCharacter('.'),
                    Intent::TypeCharacter('-'),
                    Intent::Backspace,
                    Intent::Confirm,
                ],
                effects: vec!["CommitNumeric(1.0)=>OK:Published { generation: 1 }"],
                notice: None,
            },
            "palette typing and navigation" => ExpectedBinding {
                owner: "BROWSE",
                generation: 0,
                automation: None,
                intents: vec![
                    Intent::OpenPalette,
                    Intent::TypeCharacter('b'),
                    Intent::PaletteAutocomplete,
                    Intent::TypeCharacter('4'),
                    Intent::Backspace,
                    Intent::MoveSelection(-1),
                    Intent::MoveSelection(1),
                    Intent::MoveSelection(-1),
                    Intent::MoveSelection(1),
                    Intent::Cancel,
                ],
                effects: vec![],
                notice: None,
            },
            "resize" => ExpectedBinding {
                owner: "BROWSE",
                generation: 0,
                automation: None,
                intents: vec![],
                effects: vec![],
                notice: None,
            },
            _ => panic!("missing exact matrix expectation for {name}"),
        };
        assert_eq!(outcome.violation, None, "{name}: {:?}", outcome.violation);
        assert!(
            outcome.result.deferred_inputs.is_empty(),
            "{name}: {:?}",
            outcome.result.deferred_inputs
        );
        assert!(
            outcome
                .result
                .effects
                .iter()
                .all(|effect| !effect.contains("ERR:")),
            "{name}: {:?}",
            outcome.result.effects
        );
        assert!(!outcome.result.frames.is_empty(), "{name}");
        assert_eq!(outcome.result.final_owner(), Some(expected.owner), "{name}");
        assert_eq!(
            outcome.result.session_generation, expected.generation,
            "{name}"
        );
        assert_eq!(
            outcome.result.automation_kind.as_deref(),
            expected.automation,
            "{name}"
        );
        assert_eq!(
            outcome
                .result
                .state_history
                .iter()
                .map(|record| record.action.intent)
                .collect::<Vec<_>>(),
            expected.intents,
            "{name}"
        );
        assert_eq!(
            outcome
                .result
                .effects
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            expected.effects,
            "{name}"
        );
        assert_eq!(
            outcome.result.effect_notice.as_deref(),
            expected.notice,
            "{name}"
        );
    }
    let name = "jump leader entry";
    let code = FixtureKey::Character(' ');
    let kind = PerformanceKind::Jump;
    let result = replay(&[plain(code)], TerminalCapabilities::full());
    assert!(
        matches!(result.model.mode, InteractionMode::Performance(_)),
        "{name}"
    );
    assert_eq!(
        result
            .state_history
            .last()
            .map(|record| record.action.intent),
        Some(Intent::ActivatePerformance(kind)),
        "{name}"
    );
    assert_eq!(result.session_generation, 0, "{name}");
    assert_eq!(result.automation_kind, None, "{name}");
    assert_eq!(result.effect_notice, None, "{name}");
    assert!(result.deferred_inputs.is_empty(), "{name}");
    assert!(result.effects.is_empty(), "{name}");

    let staged = InteractionModel {
        mode: InteractionMode::Palette(PaletteMode {
            staged: vec![PaletteStagedEdit {
                id: "master.bpm",
                value_bits: 91.0f32.to_bits(),
            }],
            ..PaletteMode::default()
        }),
        ..InteractionModel::default()
    };
    let immediate = replay_from_model(
        staged.clone(),
        &[plain(FixtureKey::Enter)],
        TerminalCapabilities::full(),
    );
    assert!(
        immediate
            .effects
            .iter()
            .any(|effect| { effect.starts_with("PaletteCommit") && effect.contains("Published") })
    );
    let next_bar = replay_from_model(
        staged,
        &[ctrl(FixtureKey::Character('b'))],
        TerminalCapabilities::full(),
    );
    assert!(
        next_bar.effects.iter().any(|effect| {
            effect.starts_with("PaletteCommitAtBar") && effect.contains("Staged")
        })
    );
}

#[test]
fn escape_closes_a_neutral_lfo_without_trapping_the_keyboard_owner() {
    let plain = |code| key(0, code, InputPhase::Press);

    let closed = replay(
        &[plain(FixtureKey::Character('f')), plain(FixtureKey::Escape)],
        TerminalCapabilities::full(),
    );

    assert_eq!(closed.model.mode, InteractionMode::Browsing);
    assert_eq!(closed.automation_kind, None);
}

/// Enter on the Lead page opens play mode; arrows edit the selected control
/// while the letter row still sounds tones, autorepeat plays nothing, and Esc
/// returns to browsing.
#[test]
fn lead_play_mode_keeps_letters_for_tones_while_arrows_edit_controls() {
    let plain = |code| key(0, code, InputPhase::Press);
    let to_lead: Vec<_> = std::iter::repeat_n(plain(FixtureKey::Tab), 7).collect();

    let mut opened = to_lead.clone();
    opened.push(plain(FixtureKey::Enter));
    let opened = replay(&opened, TerminalCapabilities::full());
    assert_eq!(
        opened.model.mode,
        InteractionMode::Lead(LeadPlay::default())
    );
    assert_eq!(opened.final_owner(), Some("LEAD"));
    assert_eq!(
        opened.session_generation, 0,
        "opening play mode edits nothing"
    );

    let mut played = to_lead.clone();
    played.extend([
        plain(FixtureKey::Enter),
        plain(FixtureKey::Right),
        plain(FixtureKey::Character('a')),
        key(0, FixtureKey::Character('a'), InputPhase::Repeat),
        plain(FixtureKey::Down),
        plain(FixtureKey::Right),
        plain(FixtureKey::Character('l')),
        plain(FixtureKey::Escape),
    ]);
    let played = replay(&played, TerminalCapabilities::full());
    assert_eq!(
        played.effect_count("LeadTone"),
        2,
        "one tone per press, none per repeat"
    );
    assert_eq!(played.effect_count("AdjustSelected"), 2);
    assert_eq!(
        played.session_generation, 5,
        "two presses, two control edits, and Esc releasing the held key"
    );
    assert_eq!(played.model.mode, InteractionMode::Browsing);
    assert!(played.deferred_inputs.is_empty());
    assert_eq!(played.effect_notice, None);
}

/// `i` from any page opens play mode and lands on the Lead page; the top
/// row nudges Lead rows down (left key) and up (right key) without leaving
/// the keys, one dial step per press.
#[test]
fn i_enters_play_mode_from_any_page_and_the_top_row_nudges_rows() {
    let plain = |code| key(0, code, InputPhase::Press);
    let base = replay(&[], TerminalCapabilities::full());
    let level = base.control("lead.level").unwrap();
    let decay = base.control("lead.decay").unwrap();
    let glide = base.control("lead.glide").unwrap();

    let opened = replay(
        &[plain(FixtureKey::Character('i'))],
        TerminalCapabilities::full(),
    );
    assert_eq!(
        opened.model.mode,
        InteractionMode::Lead(LeadPlay::default())
    );
    assert!(matches!(opened.model.navigation, Navigation::Lead { .. }));
    assert_eq!(opened.session_generation, 0, "entering edits nothing");

    let nudged = replay(
        &[
            plain(FixtureKey::Character('i')),
            plain(FixtureKey::Character('w')),
            plain(FixtureKey::Character('r')),
            plain(FixtureKey::Character('y')),
        ],
        TerminalCapabilities::full(),
    );
    assert_eq!(nudged.effect_count("LeadNudge"), 3);
    assert!(
        nudged.control("lead.level").unwrap() > level,
        "w raises Level"
    );
    assert!(
        nudged.control("lead.decay").unwrap() > decay,
        "r lengthens Decay"
    );
    assert!(
        nudged.control("lead.glide").unwrap() > glide,
        "y raises Glide"
    );
    assert_eq!(nudged.final_owner(), Some("LEAD"));
    assert!(nudged.deferred_inputs.is_empty());
}

/// Space inside play mode drops the lane out and back without leaving the
/// keys; `c` keeps what was just played as the lane and sets it playing.
#[test]
fn space_toggles_the_lead_lane_and_c_keeps_the_phrase() {
    let plain = |code| key(0, code, InputPhase::Press);
    let mut trace: Vec<_> = std::iter::repeat_n(plain(FixtureKey::Tab), 7).collect();
    trace.push(plain(FixtureKey::Enter));
    trace.push(plain(FixtureKey::Character(' ')));
    let off = replay(&trace, TerminalCapabilities::full());
    assert_eq!(off.control("lead.pattern"), Some(0.0));
    assert_eq!(off.final_owner(), Some("LEAD"));
    trace.push(plain(FixtureKey::Character('c')));
    let empty = replay(&trace, TerminalCapabilities::full());
    assert_eq!(
        empty.control("lead.pattern"),
        Some(0.0),
        "nothing played: nothing kept, lane stays off"
    );
    trace.extend([
        plain(FixtureKey::Character('a')),
        plain(FixtureKey::Character('d')),
        plain(FixtureKey::Character('c')),
    ]);
    let kept = replay(&trace, TerminalCapabilities::full());
    assert_eq!(kept.effect_count("LeadCapture"), 2);
    assert_eq!(kept.control("lead.pattern"), Some(1.0));
    assert_eq!(kept.control("lead.steps"), Some(4.0));
    assert!(kept.deferred_inputs.is_empty());
}

/// Ctrl+Q and Ctrl+C quit from inside play mode and the Jump leader,
/// not only from browsing: no keyboard owner traps the user.
#[test]
fn control_quit_reaches_the_lead_and_performance_owners() {
    let plain = |code| key(0, code, InputPhase::Press);
    // Bit 1 is Control in the fixture encoding (`Modifiers::CONTROL`).
    let quit = modified_key(0, FixtureKey::Character('c'), InputPhase::Press, 1 << 1);
    let mut lead: Vec<_> = std::iter::repeat_n(plain(FixtureKey::Tab), 7).collect();
    lead.extend([plain(FixtureKey::Enter), quit.clone()]);
    let lead = replay(&lead, TerminalCapabilities::full());
    assert_eq!(lead.final_owner(), Some("LEAD"));
    assert_eq!(lead.effect_count("Quit"), 1);
    let jump = replay(
        &[plain(FixtureKey::Character(' ')), quit],
        TerminalCapabilities::full(),
    );
    assert_eq!(jump.effect_count("Quit"), 1);
}

/// Enter on the Lead's Steps row opens the lane instead of play mode; the
/// lane's own Enter plays, and Esc walks back to the Steps row.
#[test]
fn enter_on_the_steps_row_opens_the_lead_pattern_and_esc_returns_to_it() {
    let plain = |code| key(0, code, InputPhase::Press);
    let mut trace: Vec<_> = std::iter::repeat_n(plain(FixtureKey::Tab), 7).collect();
    trace.extend(std::iter::repeat_n(plain(FixtureKey::Down), 10));
    trace.push(plain(FixtureKey::Enter));
    let opened = replay(&trace, TerminalCapabilities::full());
    assert_eq!(
        opened.model.navigation,
        Navigation::Lead {
            selected: 0,
            drill: LeadDrill::Pattern { return_to: 10 },
        }
    );
    assert_eq!(opened.model.mode, InteractionMode::Browsing);

    trace.extend([plain(FixtureKey::Down), plain(FixtureKey::Character('r'))]);
    let rolled = replay(&trace, TerminalCapabilities::full());
    assert_eq!(rolled.effect_count("RandomizeSelected"), 1);
    assert_eq!(rolled.session_generation, 1, "the roll edits step 2");

    trace.extend([plain(FixtureKey::Enter), plain(FixtureKey::Escape)]);
    let played = replay(&trace, TerminalCapabilities::full());
    assert_eq!(played.final_owner(), Some("BROWSE"));
    assert_eq!(
        played.model.navigation,
        Navigation::Lead {
            selected: 1,
            drill: LeadDrill::Pattern { return_to: 10 },
        },
        "Esc leaves play mode but stays in the lane"
    );

    trace.push(plain(FixtureKey::Escape));
    let back = replay(&trace, TerminalCapabilities::full());
    assert_eq!(
        back.model.navigation,
        Navigation::Lead {
            selected: 10,
            drill: LeadDrill::None,
        }
    );
}

/// `r` while browsing rolls the selected control; the same key inside an
/// LFO editor rolls the editor row under the cursor instead.
#[test]
fn r_randomizes_the_selected_control_while_browsing_and_the_row_inside_an_editor() {
    let plain = |code| key(0, code, InputPhase::Press);
    let shift = |code| modified_key(0, code, InputPhase::Press, 1);

    let rolled = replay(
        &[plain(FixtureKey::Character('r'))],
        TerminalCapabilities::full(),
    );
    assert_eq!(rolled.effect_count("RandomizeSelected"), 1);
    assert_eq!(rolled.session_generation, 1);
    let level = rolled.control("pad.level").unwrap();
    assert!((0.0..=1.0).contains(&level));
    let again = replay(
        &[plain(FixtureKey::Character('r'))],
        TerminalCapabilities::full(),
    );
    assert_eq!(
        again.control("pad.level"),
        Some(level),
        "replay rolls the same dice"
    );

    let row = replay(
        &[
            plain(FixtureKey::Character('f')),
            plain(FixtureKey::Character('r')),
        ],
        TerminalCapabilities::full(),
    );
    assert_eq!(row.effect_count("RandomizeAutomationRow"), 1);
    assert_eq!(row.effect_count("RandomizeSelected"), 0);

    let set = replay(
        &[shift(FixtureKey::Character('R'))],
        TerminalCapabilities::full(),
    );
    assert_eq!(set.effect_count("RandomizeScope"), 1);
    assert_eq!(set.session_generation, 1);
    assert_ne!(
        set.control("pad.level"),
        Some(FluidControls::default().pad.level)
    );
    assert_eq!(
        set.control("bass.level"),
        Some(FluidControls::default().bass.level)
    );

    let lfo_set = replay(
        &[
            plain(FixtureKey::Character('f')),
            shift(FixtureKey::Character('R')),
        ],
        TerminalCapabilities::full(),
    );
    assert_eq!(lfo_set.effect_count("RandomizeScope"), 1);
    assert_eq!(lfo_set.session_generation, 2);
    assert_eq!(lfo_set.automation_kind.as_deref(), Some("Lfo"));
    assert_eq!(lfo_set.automation_address, Some("pad.level"));
}

#[test]
fn production_coordinator_preserves_modifier_palette_and_save_failure_parity() {
    let ctrl_shift_save = replay(
        &[modified_key(
            0,
            FixtureKey::Character('S'),
            InputPhase::Press,
            0b000011,
        )],
        TerminalCapabilities::full(),
    );
    assert_eq!(ctrl_shift_save.clipboard_writes, 1);
    assert!(
        ctrl_shift_save
            .effects
            .iter()
            .any(|effect| effect.starts_with("Save=>OK:Message"))
    );

    let ctrl_shift_quit = replay(
        &[modified_key(
            0,
            FixtureKey::Character('C'),
            InputPhase::Press,
            0b000011,
        )],
        TerminalCapabilities::full(),
    );
    assert!(
        ctrl_shift_quit
            .effects
            .iter()
            .any(|effect| effect.starts_with("Quit=>OK:QuitRequested"))
    );

    let shifted_palette = replay(
        &[
            key(0, FixtureKey::Character('/'), InputPhase::Press),
            modified_key(0, FixtureKey::Character('B'), InputPhase::Press, 0b000001),
        ],
        TerminalCapabilities::full(),
    );
    assert!(matches!(
        shifted_palette.model.mode,
        InteractionMode::Palette(PaletteMode { ref query, .. }) if query == "B"
    ));

    let failed = replay_with(
        &[modified_key(
            0,
            FixtureKey::Character('s'),
            InputPhase::Press,
            0b000010,
        )],
        TerminalCapabilities::full(),
        |harness| {
            harness.with_clipboard_failure(ClipboardError::Unavailable("no display server".into()))
        },
    );
    assert_eq!(failed.clipboard_writes, 0);
    assert_eq!(
        failed.effect_notice.as_deref(),
        Some("Save failed: clipboard unavailable: no display server")
    );
    assert_ne!(
        failed.effect_notice.as_deref(),
        Some("Action failed: clipboard unavailable: no display server")
    );
}

/// Adding an effect from the palette lands on its row on the page it was
/// added to. It never opens the effect's detail drill: Enter does that.
#[test]
fn palette_added_effect_stays_on_its_page_instead_of_drilling_in() {
    let mut events = vec![
        key(0, FixtureKey::Tab, InputPhase::Press),
        key(0, FixtureKey::Tab, InputPhase::Press),
        key(0, FixtureKey::Character('/'), InputPhase::Press),
    ];
    events.extend(
        "delay"
            .chars()
            .map(|c| key(0, FixtureKey::Character(c), InputPhase::Press)),
    );
    events.push(key(0, FixtureKey::Enter, InputPhase::Press));
    let result = replay(&events, TerminalCapabilities::full());

    assert_eq!(
        result.control("bass.slot3.kind"),
        Some(super::module_kind_value("delay"))
    );
    let mut controls = FluidControls::default();
    controls.modules.bass[2] = super::preset_slot("delay", 0.0);
    let row = super::tab_controls(Tab::Bass, &controls)
        .iter()
        .position(|item| item.id == "bass.slot3.amount")
        .expect("the added delay has a row");
    assert_eq!(
        result.model.navigation,
        Navigation::Standard {
            page: super::interaction::StandardPage::Bass,
            selected: row,
        }
    );
}

/// Switching a Filter from Low-pass to High-pass mirrors its cutoff, so Perc's
/// factory filter at 8 kHz becomes a high-pass at 50 Hz rather than one that
/// removes everything under 8 kHz.
#[test]
fn switching_a_filter_to_high_pass_mirrors_its_cutoff() {
    let mut events = vec![
        key(0, FixtureKey::Tab, InputPhase::Press),
        key(0, FixtureKey::Character('/'), InputPhase::Press),
    ];
    events.extend(
        "filter"
            .chars()
            .map(|c| key(0, FixtureKey::Character(c), InputPhase::Press)),
    );
    events.extend([
        key(0, FixtureKey::Enter, InputPhase::Press),
        key(0, FixtureKey::Enter, InputPhase::Press),
        key(0, FixtureKey::Down, InputPhase::Press),
        key(0, FixtureKey::Down, InputPhase::Press),
        key(0, FixtureKey::Right, InputPhase::Press),
    ]);
    let result = replay(&events, TerminalCapabilities::full());

    assert_eq!(result.control("perc.slot1.feedback"), Some(1.0));
    let cutoff = result
        .control("perc.slot1.time")
        .expect("perc filter cutoff");
    assert!((cutoff - 50.0).abs() < 0.01, "{cutoff}");
}

#[test]
fn production_tick_commits_pending_palette_edits_at_the_bar() {
    let staged = InteractionModel {
        mode: InteractionMode::Palette(PaletteMode {
            staged: vec![PaletteStagedEdit {
                id: "master.bpm",
                value_bits: 91.0f32.to_bits(),
            }],
            ..PaletteMode::default()
        }),
        ..InteractionModel::default()
    };
    let result = replay_from_model(
        staged,
        &[
            modified_key(0, FixtureKey::Character('b'), InputPhase::Press, 0b000010),
            TraceEvent::Tick { after_ms: 4_000 },
        ],
        TerminalCapabilities::full(),
    );
    assert_eq!(result.pending_edits, 0, "{result:#?}");
    assert_eq!(result.control("master.bpm"), Some(91.0));
    assert_eq!(result.model.mode, InteractionMode::Browsing);
}

#[test]
fn scheduler_due_tick_precedes_events_in_the_same_production_turn() {
    let staged = InteractionModel {
        mode: InteractionMode::Palette(PaletteMode {
            staged: vec![PaletteStagedEdit {
                id: "pad.level",
                value_bits: 50.0f32.to_bits(),
            }],
            ..PaletteMode::default()
        }),
        ..InteractionModel::default()
    };
    let mut harness = ReplayHarness::new(TerminalCapabilities::full()).with_model(staged);
    let stage_event = TransportEvent::key(
        PhysicalKey::Character('b'),
        Modifiers::CONTROL,
        InputPhase::Press,
    );
    let staged_turn = super::coordinator::coordinate_production_turn(
        &mut harness.model,
        &[stage_event],
        false,
        &mut ProductionCoordinatorContext {
            effects: &mut harness.executor,
            fluid: &harness.fluid,
            flipped: &mut harness.flipped,
            clipboard: &mut harness.clipboard,
            capabilities: harness.capabilities,
            beat: 0.0,
            active_chord: 0,
        },
    )
    .expect("staging turn");
    for step in staged_turn.steps {
        harness.consume_production_step(step);
    }
    assert_eq!(
        harness.executor.pending().map(|(_, edits)| edits.len()),
        Some(1)
    );
    assert_eq!(
        harness.executor.pending().map(|(target, _)| *target),
        Some(4.0)
    );

    let adjust_event =
        TransportEvent::key(PhysicalKey::Right, Modifiers::default(), InputPhase::Press);
    let due_turn = super::coordinator::coordinate_production_turn(
        &mut harness.model,
        &[adjust_event],
        true,
        &mut ProductionCoordinatorContext {
            effects: &mut harness.executor,
            fluid: &harness.fluid,
            flipped: &mut harness.flipped,
            clipboard: &mut harness.clipboard,
            capabilities: harness.capabilities,
            beat: 4.0,
            active_chord: 0,
        },
    )
    .expect("due turn");
    for step in due_turn.steps {
        harness.consume_production_step(step);
    }
    assert!(harness.executor.pending().is_none());
    let session = harness.executor.session().load();
    assert_eq!(session.controls.pad.level, 0.52);
    assert_eq!(session.generation, 2);
    assert_eq!(
        harness
            .state_history
            .iter()
            .map(|record| record.action.intent)
            .collect::<Vec<_>>(),
        vec![Intent::CommitPaletteAtBar, Intent::AdjustSelected(1)]
    );
    assert_eq!(
        harness.effects,
        vec![
            "PaletteCommitAtBar([PaletteStagedEdit { id: \"pad.level\", value_bits: 1112014848 }])=>OK:Staged { count: 1 }",
            "AdjustSelected(1)=>OK:Published { generation: 2 }"
        ]
    );
}

#[test]
fn raw_enter_drills_custom_progression_and_master_compression() {
    fn select_custom_progression(snapshot: &mut LiveSessionSnapshot) {
        snapshot.controls.pad.progression = super::voice::CUSTOM_PROGRESSION_INDEX as f32;
    }

    let custom = replay_with(
        &[
            key(0, FixtureKey::Enter, InputPhase::Press),
            key(0, FixtureKey::Enter, InputPhase::Press),
        ],
        TerminalCapabilities::full(),
        |harness| {
            harness
                .with_model(InteractionModel {
                    navigation: Navigation::Chords {
                        selected: 7,
                        drill: ChordDrill::None,
                    },
                    ..InteractionModel::default()
                })
                .with_session_edit(select_custom_progression)
        },
    );
    assert_eq!(
        custom.model,
        InteractionModel {
            navigation: Navigation::Chords {
                selected: 0,
                drill: ChordDrill::Slot {
                    slot: 0,
                    return_to: 7,
                },
            },
            ..InteractionModel::default()
        }
    );
    assert_eq!(
        custom
            .state_history
            .iter()
            .map(|record| record.action.intent)
            .collect::<Vec<_>>(),
        vec![Intent::EnterChordProgression, Intent::EnterChordSlot(0)]
    );
    assert!(custom.effects.is_empty());
    assert_eq!(custom.session_generation, 1);
    assert_eq!(custom.automation_kind, None);
    assert_eq!(custom.effect_notice, None);
    assert_eq!(custom.final_owner(), Some("BROWSE"));
    assert_eq!(
        custom.control("pad.progression"),
        Some(super::voice::CUSTOM_PROGRESSION_INDEX as f32)
    );
}

/// The leader depends on no terminal capability: it only ever moves a
/// cursor, so a press-only terminal and a full one reach the same state.
/// Shift+P stops and starts the clock from browsing and from an open editor on
/// every terminal: it is a Press edge, so autorepeat cannot flutter it, and
/// the stopped marker stays on the activity row whoever owns the keyboard.
#[test]
fn clock_stop_toggles_on_press_in_every_terminal_and_marks_the_activity_row() {
    for capabilities in [
        TerminalCapabilities::full(),
        TerminalCapabilities::default(),
    ] {
        let shift_p = |phase| modified_key(0, FixtureKey::Character('P'), phase, 1);
        let stop = vec![
            key(0, FixtureKey::Character('p'), InputPhase::Press),
            shift_p(InputPhase::Press),
            shift_p(InputPhase::Repeat),
            key(0, FixtureKey::Character('f'), InputPhase::Press),
        ];
        let result = replay(&stop, capabilities);
        assert_eq!(result.effect_count("ToggleTransport"), 1);
        assert_eq!(result.final_owner(), Some("LFO"));
        let activity = &result.frames.last().expect("a frame").activity;
        assert!(activity.contains("STOPPED"), "{activity}");

        let mut start = stop.clone();
        start.push(shift_p(InputPhase::Press));
        let result = replay(&start, capabilities);
        assert_eq!(result.effect_count("ToggleTransport"), 2);
        let activity = &result.frames.last().expect("a frame").activity;
        assert!(!activity.contains("STOPPED"), "{activity}");
    }
}

#[test]
fn performance_leader_is_capability_independent_and_idempotent() {
    for capabilities in [
        TerminalCapabilities::full(),
        TerminalCapabilities::default(),
    ] {
        let leaders = vec![
            key(0, FixtureKey::Escape, InputPhase::Press),
            key(0, FixtureKey::Character(' '), InputPhase::Press),
            key(0, FixtureKey::Character(' '), InputPhase::Repeat),
            key(0, FixtureKey::Character(' '), InputPhase::Press),
        ];
        let result = replay(&leaders, capabilities);
        assert!(matches!(
            result.model.mode,
            InteractionMode::Performance(PerformanceMode::Jump {
                stage: JumpStage::ChooseLayer,
            })
        ));
        assert!(result.deferred_inputs.is_empty());
        assert!(result.effects.is_empty());
        assert_eq!(result.session_generation, 0);
    }

    let quit = replay(
        &[
            modified_key(0, FixtureKey::Character('q'), InputPhase::Press, 1 << 1),
            modified_key(0, FixtureKey::Character('q'), InputPhase::Repeat, 1 << 1),
        ],
        TerminalCapabilities::full(),
    );
    assert_eq!(quit.effect_count("Quit"), 1);

    let save = replay(
        &[
            modified_key(0, FixtureKey::Character('s'), InputPhase::Press, 1 << 1),
            modified_key(0, FixtureKey::Character('s'), InputPhase::Repeat, 1 << 1),
        ],
        TerminalCapabilities::full(),
    );
    assert_eq!(save.effect_count("Save"), 1);
    assert_eq!(save.clipboard_writes, 1);
}

/// `Space d j` puts the cursor on Bass volume and hands the keyboard back,
/// having changed no value: arrival is the whole gesture.
#[test]
fn jump_to_volume_lands_the_cursor_without_editing() {
    let result = replay_with(
        &[
            key(0, FixtureKey::Character(' '), InputPhase::Press),
            key(0, FixtureKey::Character('d'), InputPhase::Press),
            key(0, FixtureKey::Character('j'), InputPhase::Press),
        ],
        TerminalCapabilities::full(),
        ReplayHarness::with_auto_running,
    );
    assert_eq!(result.model.mode, InteractionMode::Browsing);
    assert_eq!(result.recent_ids, ["bass.level"]);
    assert!(matches!(
        result.model.navigation,
        Navigation::Standard {
            page: super::interaction::StandardPage::Bass,
            selected: 0,
        }
    ));
    // Only an edit exits auto, and arriving is not an edit: the morph
    // keeps running and the level is untouched.
    assert!(result.auto_running);
    assert_eq!(
        result.control("bass.level"),
        replay_with(
            &[],
            TerminalCapabilities::full(),
            ReplayHarness::with_auto_running
        )
        .control("bass.level")
    );
    assert!(result.effects.iter().any(|effect| {
        effect.contains(r#"OK:ControlSelected { tab: Bass, index: 0, id: "bass.level""#)
    }));
}

/// Filter is a catalog module, not a per-layer control. Bass, Kick and Perc
/// ship with one in slot 1, so `k` jumps to it and leaves its amount alone;
/// Pads ships without one, so `k` adds an inert filter and lands on that.
#[test]
fn jump_to_filter_reaches_a_loaded_one_and_adds_an_inert_one_otherwise() {
    let jump = |layer| {
        vec![
            key(0, FixtureKey::Character(' '), InputPhase::Press),
            key(0, FixtureKey::Character(layer), InputPhase::Press),
            key(0, FixtureKey::Character('k'), InputPhase::Press),
        ]
    };
    let filter_value = super::module_kind_value(super::interaction::FILTER_MODULE_ID);

    // Bass already holds a filter: the leader must reach it, never stack a
    // second one or reset the amount the player is performing with.
    let loaded = replay(&jump('d'), TerminalCapabilities::full());
    assert_eq!(loaded.model.mode, InteractionMode::Browsing);
    assert_eq!(loaded.control("bass.slot1.kind"), Some(filter_value));
    // Its cutoff is the row that was performing; the leader must not reset it.
    assert_eq!(
        loaded.control("bass.slot1.time"),
        replay(&[], TerminalCapabilities::full()).control("bass.slot1.time")
    );
    assert_eq!(
        loaded.recent_ids,
        ["bass.slot1.time"],
        "the cursor landed on the filter's cutoff row"
    );

    // Pads holds `room` in slot 1, so the filter goes into the first free
    // slot, inert, and the existing chain is untouched.
    let added = replay(&jump('a'), TerminalCapabilities::full());
    assert_eq!(added.model.mode, InteractionMode::Browsing);
    assert_eq!(added.control("pad.slot2.kind"), Some(filter_value));
    // A filter is always fully wet; its cutoff is what starts transparent,
    // so adding one is audibly free and `h` is the first audible move.
    assert_eq!(added.control("pad.slot2.amount"), Some(1.0));
    assert_eq!(
        added.control("pad.slot2.time"),
        Some(super::FILTER_CUTOFF_MAX_HZ)
    );
    assert_eq!(added.recent_ids, ["pad.slot2.time"]);
    assert_eq!(
        added.control("pad.slot1.kind"),
        Some(super::module_kind_value("room"))
    );
}

/// Two keys reach a knob on the layer already open: `Space` then the
/// parameter, no layer key. It works on pages no layer key names.
#[test]
fn a_parameter_key_without_a_layer_aims_at_the_open_page() {
    // Tab twice from Pads reaches Bass.
    let on_bass = |mut trace: Vec<_>| {
        let mut keys = vec![
            key(0, FixtureKey::Tab, InputPhase::Press),
            key(0, FixtureKey::Tab, InputPhase::Press),
        ];
        keys.append(&mut trace);
        keys
    };

    let volume = replay(
        &on_bass(vec![
            key(0, FixtureKey::Character(' '), InputPhase::Press),
            key(0, FixtureKey::Character('j'), InputPhase::Press),
        ]),
        TerminalCapabilities::full(),
    );
    assert_eq!(volume.model.mode, InteractionMode::Browsing);
    assert_eq!(volume.recent_ids, ["bass.level"]);

    let filter = replay(
        &on_bass(vec![
            key(0, FixtureKey::Character(' '), InputPhase::Press),
            key(0, FixtureKey::Character('k'), InputPhase::Press),
        ]),
        TerminalCapabilities::full(),
    );
    assert_eq!(filter.model.mode, InteractionMode::Browsing);
    assert_eq!(
        filter.recent_ids,
        ["bass.slot1.time"],
        "the cursor landed on the filter's cutoff row"
    );

    // Master has no layer key at all, so the shorthand is the only way in.
    let master = replay(
        &[
            key(0, FixtureKey::BackTab, InputPhase::Press),
            key(0, FixtureKey::Character(' '), InputPhase::Press),
            key(0, FixtureKey::Character('j'), InputPhase::Press),
        ],
        TerminalCapabilities::full(),
    );
    assert_eq!(master.recent_ids, ["master.level"]);
}

/// A mistyped layer costs one key: a second layer key re-aims the pending
/// jump instead of being inert.
#[test]
fn a_second_layer_key_reaims_the_pending_jump() {
    let result = replay(
        &[
            key(0, FixtureKey::Character(' '), InputPhase::Press),
            key(0, FixtureKey::Character('d'), InputPhase::Press),
            key(0, FixtureKey::Character('f'), InputPhase::Press),
            key(0, FixtureKey::Character('j'), InputPhase::Press),
        ],
        TerminalCapabilities::full(),
    );
    assert_eq!(result.model.mode, InteractionMode::Browsing);
    assert_eq!(result.recent_ids, ["kick.level"]);
}

/// Autorepeat inside the leader cannot fire a second jump, and a stray
/// release never reaches it.
#[test]
fn leader_keys_ignore_repeat_and_release() {
    let result = replay(
        &[
            key(0, FixtureKey::Character(' '), InputPhase::Press),
            key(0, FixtureKey::Character('f'), InputPhase::Press),
            key(0, FixtureKey::Character('f'), InputPhase::Repeat),
            key(0, FixtureKey::Character('f'), InputPhase::Release),
            key(0, FixtureKey::Character('j'), InputPhase::Repeat),
        ],
        TerminalCapabilities::full(),
    );
    assert!(matches!(
        result.model.mode,
        InteractionMode::Performance(PerformanceMode::Jump {
            stage: JumpStage::ChooseParameter {
                instrument: PerformanceInstrument::Kick,
            },
        })
    ));
    assert!(
        result
            .effects
            .iter()
            .all(|effect| !effect.contains("ControlSelected"))
    );
}

#[test]
fn every_decided_edge_binding_ignores_repeat_exactly_once() {
    let repeated = |code: PhysicalKey| {
        vec![
            key(0, code.clone(), InputPhase::Press),
            key(0, code, InputPhase::Repeat),
        ]
    };
    let palette = replay(
        &repeated(FixtureKey::Character('/')),
        TerminalCapabilities::full(),
    );
    assert_eq!(palette.final_owner(), Some("PALETTE"));
    let leader = replay(
        &repeated(FixtureKey::Character(' ')),
        TerminalCapabilities::full(),
    );
    assert!(matches!(
        leader.model.mode,
        InteractionMode::Performance(PerformanceMode::Jump {
            stage: JumpStage::ChooseLayer,
        })
    ));

    let numeric = replay_from_model(
        InteractionModel {
            mode: InteractionMode::Numeric(NumericEntry {
                buffer: "88".into(),
                resume: None,
            }),
            ..InteractionModel::default()
        },
        &repeated(FixtureKey::Enter),
        TerminalCapabilities::full(),
    );
    assert_eq!(numeric.effect_count("CommitNumeric"), 1);

    let palette = replay_from_model(
        InteractionModel {
            mode: InteractionMode::Palette(PaletteMode::default()),
            ..InteractionModel::default()
        },
        &repeated(FixtureKey::Tab),
        TerminalCapabilities::full(),
    );
    assert!(matches!(
        palette.model.mode,
        InteractionMode::Palette(PaletteMode {
            locked: Some(_),
            ..
        })
    ));

    let performance = replay_from_model(
        InteractionModel {
            mode: InteractionMode::Performance(PerformanceMode::Jump {
                stage: JumpStage::ChooseLayer,
            }),
            ..InteractionModel::default()
        },
        &repeated(FixtureKey::Character('a')),
        TerminalCapabilities::full(),
    );
    assert_eq!(performance.effect_count("SelectPage"), 1);

    let drill = replay_from_model(
        InteractionModel {
            navigation: Navigation::Chords {
                selected: 2,
                drill: ChordDrill::Progression { return_to: 0 },
            },
            mode: InteractionMode::Browsing,
            ..InteractionModel::default()
        },
        &repeated(FixtureKey::Enter),
        TerminalCapabilities::full(),
    );
    assert!(matches!(
        drill.model.navigation,
        Navigation::Chords {
            drill: ChordDrill::Slot { slot: 2, .. },
            ..
        }
    ));
}

#[test]
fn every_edge_policy_intent_is_a_no_op_on_repeat_and_release() {
    let candidate_intents = [
        Intent::MoveSelection(1),
        Intent::ChangePage(super::interaction::PageDirection::Next),
        Intent::Cancel,
        Intent::EnterChordProgression,
        Intent::EnterChordSlot(0),
        Intent::BeginNumeric('1'),
        Intent::TypeCharacter('x'),
        Intent::Backspace,
        Intent::PaletteAutocomplete,
        Intent::Confirm,
        Intent::OpenPalette,
        Intent::OpenAutomation(AutomationKind::Lfo),
        Intent::OpenAutomationField,
        Intent::ActivatePerformance(PerformanceKind::Jump),
        Intent::SelectPerformanceInstrument {
            instrument: PerformanceInstrument::Pads,
        },
        Intent::JumpToParameter(super::interaction::PerformanceParameter::Volume),
        Intent::AdjustSelected(1),
        Intent::ResetSelected,
        Intent::ToggleAuto,
        Intent::ToggleUnits,
        Intent::ToggleMute { master: false },
        Intent::ToggleTransport,
        Intent::RemoveAutomation,
        Intent::RandomizeAutomationRow,
        Intent::TouchSelected,
        Intent::CommitPaletteAtBar,
        Intent::Save,
        Intent::Quit,
        Intent::ReleaseLeadTone(1),
    ];
    let edge_intents = candidate_intents
        .into_iter()
        .filter(|intent| intent.phase_policy() == PhasePolicy::Edge)
        .collect::<Vec<_>>();
    assert!(
        edge_intents.len() > 8,
        "edge coverage unexpectedly collapsed"
    );

    for intent in edge_intents {
        for phase in [InputPhase::Repeat, InputPhase::Release] {
            let mut harness = ReplayHarness::new(TerminalCapabilities::full());
            let mut context = ProductionCoordinatorContext {
                effects: &mut harness.executor,
                fluid: &harness.fluid,
                flipped: &mut harness.flipped,
                clipboard: &mut harness.clipboard,
                capabilities: harness.capabilities,
                beat: 0.0,
                active_chord: 0,
            };
            let action = SemanticAction { phase, intent };
            let frame = production_frame(&mut harness.model, &context);
            let record =
                coordinate_production_action(&mut harness.model, action, &frame, &mut context)
                    .expect("edge intents are never refused before the kernel");
            let quit = record.requested_quit();
            harness.consume_production_step(ProductionStep {
                mapping: InputMapping::Action(action),
                actions: vec![record],
                quit,
            });
            assert!(harness.violation.is_none(), "{intent:?}/{phase:?}");
            let record = harness
                .state_history
                .last()
                .expect("direct semantic action must be recorded");
            assert_eq!(record.before, record.after, "{intent:?}/{phase:?}");
            assert!(record.effects.is_empty(), "{intent:?}/{phase:?}");
        }
    }
}

#[test]
fn escape_converges_from_every_owner_and_nested_depth() {
    let escape = key(0, FixtureKey::Escape, InputPhase::Press);
    let models = [
        InteractionModel {
            mode: InteractionMode::Numeric(NumericEntry {
                buffer: "12".into(),
                resume: None,
            }),
            ..InteractionModel::default()
        },
        InteractionModel {
            mode: InteractionMode::Palette(PaletteMode::default()),
            ..InteractionModel::default()
        },
        InteractionModel {
            mode: InteractionMode::Automation(AutomationMode::Envelope { selected: 2 }),
            ..InteractionModel::default()
        },
        InteractionModel {
            mode: InteractionMode::Performance(PerformanceMode::Jump {
                stage: JumpStage::ChooseLayer,
            }),
            ..InteractionModel::default()
        },
        InteractionModel {
            mode: InteractionMode::Performance(PerformanceMode::Jump {
                stage: JumpStage::ChooseParameter {
                    instrument: PerformanceInstrument::Kick,
                },
            }),
            ..InteractionModel::default()
        },
        InteractionModel {
            navigation: Navigation::Chords {
                selected: 2,
                drill: ChordDrill::Slot {
                    slot: 1,
                    return_to: 0,
                },
            },
            mode: InteractionMode::Browsing,
            ..InteractionModel::default()
        },
    ];
    for model in models {
        let result = replay_from_model(
            model,
            &[escape.clone(), escape.clone(), escape.clone()],
            TerminalCapabilities::full(),
        );
        assert!(
            matches!(result.model.mode, InteractionMode::Browsing),
            "Escape did not restore browsing: {:?}",
            result.model
        );
        assert!(
            matches!(
                result.model.navigation,
                Navigation::Chords {
                    drill: ChordDrill::None,
                    ..
                } | Navigation::Master { .. }
                    | Navigation::Standard { .. }
            ),
            "Escape left a navigation drill open: {:?}",
            result.model
        );
    }
}

#[test]
fn rapid_ready_source_has_a_bounded_nontrivial_admission_high_water() {
    let trace = (0..32)
        .map(|index| {
            key(
                0,
                if index % 2 == 0 {
                    FixtureKey::Down
                } else {
                    FixtureKey::Up
                },
                InputPhase::Repeat,
            )
        })
        .collect::<Vec<_>>();
    let result = replay(&trace, TerminalCapabilities::full());
    assert!(result.max_queue > 1);
    if let Some(violation) = post_replay_violation(&result) {
        panic!(
            "{}",
            format_property_diagnostic(&violation, &trace, &result, &result)
        );
    }
}

#[test]
fn arbitrary_event_streams_preserve_runtime_and_model_invariants() {
    let mut observed_edge_actions = 0;
    for seed in 1..=64_u64 {
        let trace = generated_trace(seed, 96);
        for capabilities in [
            TerminalCapabilities::full(),
            TerminalCapabilities::default(),
        ] {
            let result = checked_replay(&trace, |candidate| {
                let first = replay_outcome(candidate, capabilities);
                let second = replay_outcome(candidate, capabilities);
                let violation =
                    nondeterministic_violation(&first.result, &second.result).or(first.violation);
                Observation {
                    result: first.result,
                    mirror: Some(second.result),
                    violation,
                }
            });
            observed_edge_actions += result
                .state_history
                .iter()
                .filter(|record| record.action.intent.phase_policy() == PhasePolicy::Edge)
                .count();
        }
    }
    assert!(
        observed_edge_actions > 0,
        "generator missed all edge actions"
    );
}

fn replay_outcome(trace: &[TraceEvent], capabilities: TerminalCapabilities) -> ReplayOutcome {
    ReplayHarness::new(capabilities).replay(&ReplayTrace {
        events: trace.to_vec(),
    })
}

fn nondeterministic_violation(
    left: &ReplayResult,
    right: &ReplayResult,
) -> Option<PropertyViolation> {
    divergence_signature(left, right)
        .map(|signature| PropertyViolation::NondeterministicReplay { signature })
}

fn divergence_signature(left: &ReplayResult, right: &ReplayResult) -> Option<DivergenceSignature> {
    if let Some(signature) =
        first_sequence_divergence("state_history", &left.state_history, &right.state_history)
    {
        return Some(signature);
    }
    if let Some(signature) = first_sequence_divergence("frames", &left.frames, &right.frames) {
        return Some(signature);
    }
    macro_rules! first_field {
        ($field:ident) => {
            if left.$field != right.$field {
                return Some(DivergenceSignature {
                    field: stringify!($field).into(),
                    left: format!("{:?}", left.$field),
                    right: format!("{:?}", right.$field),
                });
            }
        };
    }
    first_field!(model);
    first_field!(session_generation);
    first_field!(control_bits);
    first_field!(automation_kind);
    first_field!(automation_address);
    first_field!(auto_running);
    first_field!(recent_ids);
    first_field!(effects);
    first_field!(effect_notice);
    first_field!(pending_edits);
    first_field!(pending_target_bits);
    first_field!(max_queue);
    first_field!(deferred_inputs);
    first_field!(clipboard_writes);
    first_field!(explicit_ticks);
    first_field!(explicit_tick_turn_ids);
    first_field!(idle_boundaries);
    first_field!(scheduler_turn_ids);
    first_field!(idle_turn_ids);
    first_field!(telemetry_beat_bits);
    None
}

/// First position where the sequences disagree, or where the shorter one
/// ends; `None` when they are equal.
fn first_divergence_index<T: PartialEq>(left: &[T], right: &[T]) -> Option<usize> {
    left.iter()
        .zip(right)
        .position(|(left_item, right_item)| left_item != right_item)
        .or_else(|| (left.len() != right.len()).then(|| left.len().min(right.len())))
}

fn first_sequence_divergence<T: std::fmt::Debug + PartialEq>(
    field: &str,
    left: &[T],
    right: &[T],
) -> Option<DivergenceSignature> {
    let index = first_divergence_index(left, right)?;
    Some(match (left.get(index), right.get(index)) {
        (Some(left_item), Some(right_item)) => DivergenceSignature {
            field: format!("{field}[{index}]"),
            left: format!("{left_item:?}"),
            right: format!("{right_item:?}"),
        },
        _ => DivergenceSignature {
            field: format!("{field}.len"),
            left: left.len().to_string(),
            right: right.len().to_string(),
        },
    })
}

fn post_replay_violation(result: &ReplayResult) -> Option<PropertyViolation> {
    if result.max_queue > SchedulerConfig::default().queue_capacity {
        return Some(PropertyViolation::QueueCapacityExceeded {
            observed: result.max_queue,
            capacity: SchedulerConfig::default().queue_capacity,
        });
    }
    if let Some(gap) = result
        .frames
        .windows(2)
        .map(|frames| {
            frames[1]
                .completed_at
                .saturating_sub(frames[0].completed_at)
        })
        .find(|gap| *gap > MAX_FRAME_GAP)
    {
        return Some(PropertyViolation::FrameGapExceeded { gap });
    }
    None
}

fn format_property_diagnostic(
    violation: &PropertyViolation,
    minimal: &[TraceEvent],
    left: &ReplayResult,
    right: &ReplayResult,
) -> String {
    let divergent = match first_divergence_index(&left.state_history, &right.state_history) {
        Some(index) => (
            left.state_history.get(index),
            right.state_history.get(index),
        ),
        None => (left.state_history.last(), right.state_history.last()),
    };
    format!(
        "{violation:?}\nminimal fixture:\n{}\nleft replay result:\n{left:#?}\nright replay result:\n{right:#?}\nactual divergent ActionRecord:\nleft {}\nright {}",
        ReplayTrace {
            events: minimal.to_vec()
        }
        .fixture(),
        format_action_record(divergent.0),
        format_action_record(divergent.1),
    )
}

fn format_action_record(record: Option<&ActionRecord>) -> String {
    record.map_or_else(
        || "action=<none> before=<none> after=<none> effects=[]".into(),
        |record| {
            format!(
                "action={:?} before={:?} after={:?} effects={:?}",
                record.action, record.before, record.after, record.effects
            )
        },
    )
}

fn minimize_trace(
    mut events: Vec<TraceEvent>,
    mut still_fails: impl FnMut(&[TraceEvent]) -> bool,
) -> Vec<TraceEvent> {
    let mut chunk = events.len().next_power_of_two() / 2;
    while chunk > 0 {
        let mut start = 0;
        while start < events.len() {
            let end = (start + chunk).min(events.len());
            let mut candidate = events.clone();
            candidate.drain(start..end);
            if !candidate.is_empty() && still_fails(&candidate) {
                events = candidate;
            } else {
                start += chunk;
            }
        }
        chunk /= 2;
    }
    events
}

#[test]
fn failure_minimizer_returns_a_replayable_delta_reduced_trace() {
    let trace = vec![
        key(0, FixtureKey::Up, InputPhase::Press),
        key(0, FixtureKey::Character('p'), InputPhase::Press),
        key(0, FixtureKey::Escape, InputPhase::Press),
        key(0, FixtureKey::Down, InputPhase::Press),
    ];
    let minimal = minimize_trace(trace, |events| {
        events.iter().any(|event| {
            matches!(
                event,
                TraceEvent::Key {
                    code: FixtureKey::Character('p'),
                    ..
                }
            )
        })
    });
    assert_eq!(minimal.len(), 1);
    let fixture = ReplayTrace { events: minimal }.fixture();
    assert!(ReplayTrace::parse(&fixture).is_ok());
}

#[test]
fn nondeterministic_keys_and_minimizer_predicates_retain_the_exact_divergence() {
    let baseline = replay(&[], TerminalCapabilities::full());
    let mut queue_divergence = baseline.clone();
    queue_divergence.max_queue += 1;
    let mut clipboard_divergence = baseline.clone();
    clipboard_divergence.clipboard_writes += 1;

    let queue_violation = nondeterministic_violation(&baseline, &queue_divergence)
        .expect("queue difference must produce a signature");
    let clipboard_violation = nondeterministic_violation(&baseline, &clipboard_divergence)
        .expect("clipboard difference must produce a signature");
    assert_ne!(queue_violation, clipboard_violation);

    let matches_minimizer_target = |candidate: &PropertyViolation| *candidate == queue_violation;
    assert!(matches_minimizer_target(&queue_violation));
    assert!(!matches_minimizer_target(&clipboard_violation));
}

#[test]
fn ddmin_retains_the_exact_nondeterministic_divergence_signature() {
    let target_event = key(0, FixtureKey::Character('p'), InputPhase::Press);
    let competitor_event = key(0, FixtureKey::Character('q'), InputPhase::Press);
    let synthetic_violation = |events: &[TraceEvent]| {
        let contains = |character| {
            events.iter().any(|event| {
                matches!(
                    event,
                    TraceEvent::Key {
                        code: PhysicalKey::Character(found),
                        ..
                    } if *found == character
                )
            })
        };
        let right = if contains('p') {
            "queue=65"
        } else if contains('q') {
            "queue=66"
        } else {
            return None;
        };
        Some(PropertyViolation::NondeterministicReplay {
            signature: DivergenceSignature {
                field: "max_queue".into(),
                left: "queue=64".into(),
                right: right.into(),
            },
        })
    };
    let original = vec![
        key(0, FixtureKey::Up, InputPhase::Press),
        competitor_event,
        key(0, FixtureKey::Down, InputPhase::Press),
        target_event.clone(),
    ];
    let target = synthetic_violation(std::slice::from_ref(&target_event))
        .expect("target event must diverge");
    let competitor = synthetic_violation(&[key(0, FixtureKey::Character('q'), InputPhase::Press)])
        .expect("competitor event must diverge");
    assert_ne!(target, competitor);

    let minimal = minimize_trace(original, |candidate| {
        synthetic_violation(candidate).as_ref() == Some(&target)
    });
    let minimized_violation =
        synthetic_violation(&minimal).expect("minimized trace must still diverge");
    assert_eq!(minimized_violation, target);
    assert_ne!(minimized_violation, competitor);
    assert_eq!(minimal, vec![target_event]);
    assert!(ReplayTrace::parse(&ReplayTrace { events: minimal }.fixture()).is_ok());
}

#[test]
fn ddmin_retains_the_exact_edge_transition_record() {
    let target_event = key(0, FixtureKey::Character('p'), InputPhase::Repeat);
    let competitor_event = key(0, FixtureKey::Character('q'), InputPhase::Repeat);
    let edge_violation = |events: &[TraceEvent]| {
        let contains = |character| {
            events.iter().any(|event| {
                matches!(
                    event,
                    TraceEvent::Key {
                        code: PhysicalKey::Character(found),
                        ..
                    } if *found == character
                )
            })
        };
        let record = if contains('p') {
            ActionRecord {
                action: SemanticAction {
                    phase: InputPhase::Repeat,
                    intent: Intent::Save,
                },
                before: InteractionModel::default(),
                after: InteractionModel {
                    mode: InteractionMode::Numeric(NumericEntry {
                        buffer: "1".into(),
                        resume: None,
                    }),
                    ..InteractionModel::default()
                },
                effects: vec!["target-effect".into()],
            }
        } else if contains('q') {
            ActionRecord {
                action: SemanticAction {
                    phase: InputPhase::Repeat,
                    intent: Intent::Save,
                },
                before: InteractionModel {
                    mode: InteractionMode::Palette(PaletteMode::default()),
                    ..InteractionModel::default()
                },
                after: InteractionModel {
                    mode: InteractionMode::Numeric(NumericEntry {
                        buffer: "2".into(),
                        resume: None,
                    }),
                    ..InteractionModel::default()
                },
                effects: vec!["competitor-effect".into()],
            }
        } else {
            return None;
        };
        Some(PropertyViolation::EdgeChangedOnNonPress {
            record: Box::new(record),
        })
    };
    let original = vec![
        key(0, FixtureKey::Left, InputPhase::Press),
        competitor_event,
        key(0, FixtureKey::Right, InputPhase::Press),
        target_event.clone(),
    ];
    let target =
        edge_violation(std::slice::from_ref(&target_event)).expect("target event must violate");
    let competitor = edge_violation(&[key(0, FixtureKey::Character('q'), InputPhase::Repeat)])
        .expect("competitor event must violate");
    assert_ne!(target, competitor);

    let minimal = minimize_trace(original, |candidate| {
        edge_violation(candidate).as_ref() == Some(&target)
    });
    let minimized_violation =
        edge_violation(&minimal).expect("minimized trace must retain an edge violation");
    assert_eq!(minimized_violation, target);
    assert_ne!(minimized_violation, competitor);
    assert_eq!(minimal, vec![target_event]);
    assert!(ReplayTrace::parse(&ReplayTrace { events: minimal }.fixture()).is_ok());
}

#[test]
fn property_diagnostic_contains_minimal_trace_and_transition_details() {
    let minimal = vec![key(0, FixtureKey::Character('f'), InputPhase::Press)];
    let left = replay(&minimal, TerminalCapabilities::full());
    let right = replay(
        &[key(0, FixtureKey::Character('/'), InputPhase::Press)],
        TerminalCapabilities::full(),
    );
    let left_record = &left.state_history[0];
    assert_eq!(
        left_record.action,
        SemanticAction {
            phase: InputPhase::Press,
            intent: Intent::OpenAutomation(AutomationKind::Lfo),
        }
    );
    assert_eq!(left_record.before, InteractionModel::default());
    assert!(matches!(
        left_record.after.mode,
        InteractionMode::Automation(AutomationMode::Lfo { .. })
    ));
    assert!(
        left_record
            .effects
            .iter()
            .any(|effect| effect.starts_with("AutomationConfirm(Lfo)"))
    );
    assert_eq!(
        right.state_history[0].action,
        SemanticAction {
            phase: InputPhase::Press,
            intent: Intent::OpenPalette,
        }
    );
    let violation =
        nondeterministic_violation(&left, &right).expect("different results must have a signature");
    let diagnostic = format_property_diagnostic(&violation, &minimal, &left, &right);
    for required in [
        "minimal fixture:",
        "nooise-replay-v1",
        "left replay result:",
        "right replay result:",
        "actual divergent ActionRecord:",
        "action=",
        "before=",
        "after=",
        "effects=",
        "OpenAutomation(Lfo)",
        "OpenPalette",
    ] {
        assert!(
            diagnostic.contains(required),
            "missing {required:?} from diagnostic"
        );
    }
}

fn generated_trace(mut state: u64, length: usize) -> Vec<TraceEvent> {
    let mut events = Vec::with_capacity(length);
    let keys = [
        FixtureKey::Character('p'),
        FixtureKey::Character(' '),
        FixtureKey::Character('a'),
        FixtureKey::Character('s'),
        FixtureKey::Character('1'),
        FixtureKey::Character('/'),
        FixtureKey::Escape,
        FixtureKey::Enter,
        FixtureKey::Backspace,
        FixtureKey::Left,
        FixtureKey::Right,
        FixtureKey::Up,
        FixtureKey::Down,
        FixtureKey::Tab,
        FixtureKey::BackTab,
    ];
    for index in 0..length {
        let seeded = match index {
            0 => Some(key(0, PhysicalKey::Character(' '), InputPhase::Press)),
            1 => Some(key(0, PhysicalKey::Character('a'), InputPhase::Press)),
            2 => Some(key(0, PhysicalKey::Character('/'), InputPhase::Repeat)),
            3 => Some(key(0, PhysicalKey::Escape, InputPhase::Release)),
            4 => Some(modified_key(
                0,
                PhysicalKey::Character('s'),
                InputPhase::Repeat,
                1 << 1,
            )),
            _ => None,
        };
        if let Some(event) = seeded {
            events.push(event);
            continue;
        }
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let after_ms = state % 9;
        if index % 17 == 0 {
            events.push(TraceEvent::Resize {
                after_ms,
                width: 40 + (state % 61) as u16,
                height: 8 + ((state >> 8) % 25) as u16,
            });
        } else if index % 13 == 0 {
            events.push(TraceEvent::Tick {
                after_ms: after_ms + 33,
            });
        } else {
            let code = keys[(state as usize) % keys.len()].clone();
            let phase = match (state >> 16) % 3 {
                0 => InputPhase::Press,
                1 => InputPhase::Repeat,
                _ => InputPhase::Release,
            };
            let mut event = key(after_ms, code, phase);
            if let TraceEvent::Key {
                modifiers,
                repeat_count,
                ..
            } = &mut event
            {
                *modifiers = ((state >> 20) % 64) as u8;
                *repeat_count = if phase == InputPhase::Repeat {
                    1 + ((state >> 24) % 4)
                } else {
                    1
                };
            }
            events.push(event);
        }
    }
    events
}
