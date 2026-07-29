//! Deterministic, production-path interaction replay support.
//!
//! Fixtures retain sanitized key phase/modifiers/repeat counts, relative time,
//! dimensions, lifecycle markers, and explicit tick/idle records. Paste and
//! mouse payloads are always redacted.

use std::cell::Cell;
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::io;
use std::rc::Rc;
use std::time::Duration;

use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MediaKeyCode, ModifierKeyCode,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use super::effect::{Clipboard, EffectAcknowledgement, EffectFailure, InteractionExecutionContext};
use super::interaction::{
    AutomationKind, AutomationMode, ChordDrill, InputPhase, Intent, InteractionEffect,
    InteractionMode, InteractionModel, LfoDepth, MasterDrill, Navigation, NumericEntry,
    PaletteMode, PerformanceKind, PerformanceMode, PhasePolicy, SemanticAction, SequenceStage,
};
use super::runtime::{
    Clock, EventSource, InputMapping, MAX_FRAME_GAP, Modifiers, PhysicalKey,
    SanitizedTraceRecorder, Scheduler, SchedulerConfig, TerminalCapabilities, TransportEvent,
    TransportKey, decode_physical_key, encode_physical_key, map_input, normalize_key_event,
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

impl ReplayTrace {
    fn fixture(&self) -> String {
        let mut fixture = "nooise-replay-v1\n".to_string();
        for event in &self.events {
            match event {
                TraceEvent::Key {
                    after_ms,
                    code,
                    phase,
                    modifiers,
                    repeat_count,
                } => {
                    writeln!(
                        fixture,
                        "+{after_ms} key {} {} mods:{modifiers} repeats:{repeat_count}",
                        encode_physical_key(code),
                        phase_token(*phase)
                    )
                    .expect("writing to String cannot fail");
                }
                TraceEvent::Resize {
                    after_ms,
                    width,
                    height,
                } => {
                    writeln!(fixture, "+{after_ms} resize {width}x{height}")
                        .expect("writing to String cannot fail");
                }
                TraceEvent::Tick { after_ms } => {
                    writeln!(fixture, "+{after_ms} tick").expect("writing to String cannot fail");
                }
                TraceEvent::Idle { after_ms } => {
                    writeln!(fixture, "+{after_ms} idle").expect("writing to String cannot fail");
                }
                TraceEvent::Redacted { after_ms, kind } => {
                    writeln!(fixture, "+{after_ms} redacted {kind}")
                        .expect("writing to String cannot fail");
                }
                TraceEvent::Focus { after_ms, gained } => writeln!(
                    fixture,
                    "+{after_ms} focus-{}",
                    if *gained { "gained" } else { "lost" }
                )
                .expect("writing to String cannot fail"),
                TraceEvent::Shutdown { after_ms } => {
                    writeln!(fixture, "+{after_ms} shutdown")
                        .expect("writing to String cannot fail");
                }
            }
        }
        fixture
    }

    fn parse(fixture: &str) -> Result<Self, String> {
        let mut lines = fixture.lines();
        if lines.next() != Some("nooise-replay-v1") {
            return Err("unsupported or missing replay fixture version".into());
        }
        let mut events = Vec::new();
        for (line_index, line) in lines.enumerate() {
            let mut parts = line.split_whitespace();
            let after_ms = parts
                .next()
                .and_then(|part| part.strip_prefix('+'))
                .ok_or_else(|| format!("line {}: missing clock advance", line_index + 1))?
                .parse()
                .map_err(|_| format!("line {}: invalid clock advance", line_index + 1))?;
            let kind = parts
                .next()
                .ok_or_else(|| format!("line {}: missing event kind", line_index + 1))?;
            let event = match kind {
                "key" => {
                    let code = parts
                        .next()
                        .and_then(decode_physical_key)
                        .ok_or_else(|| format!("line {}: invalid key", line_index + 1))?;
                    let phase = parts
                        .next()
                        .and_then(parse_phase)
                        .ok_or_else(|| format!("line {}: invalid phase", line_index + 1))?;
                    let modifiers = parts
                        .next()
                        .and_then(|part| part.strip_prefix("mods:"))
                        .and_then(|bits| bits.parse().ok())
                        .filter(|bits| *bits <= 0b11_1111)
                        .ok_or_else(|| format!("line {}: invalid modifiers", line_index + 1))?;
                    let repeat_count = parts
                        .next()
                        .and_then(|part| part.strip_prefix("repeats:"))
                        .and_then(|count| count.parse().ok())
                        .filter(|count| *count > 0)
                        .ok_or_else(|| format!("line {}: invalid repeat count", line_index + 1))?;
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
                        .ok_or_else(|| format!("line {}: missing dimensions", line_index + 1))?;
                    let (width, height) = dimensions
                        .split_once('x')
                        .ok_or_else(|| format!("line {}: invalid dimensions", line_index + 1))?;
                    TraceEvent::Resize {
                        after_ms,
                        width: width
                            .parse()
                            .map_err(|_| format!("line {}: invalid width", line_index + 1))?,
                        height: height
                            .parse()
                            .map_err(|_| format!("line {}: invalid height", line_index + 1))?,
                    }
                }
                "tick" => TraceEvent::Tick { after_ms },
                "idle" => TraceEvent::Idle { after_ms },
                "redacted" => {
                    let kind = parts
                        .next()
                        .ok_or_else(|| format!("line {}: missing redacted kind", line_index + 1))?;
                    let kind = match kind {
                        "paste" => "paste",
                        "mouse" => "mouse",
                        _ => return Err(format!("line {}: invalid redacted kind", line_index + 1)),
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
                _ => return Err(format!("line {}: invalid event kind", line_index + 1)),
            };
            if parts.next().is_some() {
                return Err(format!("line {}: trailing fixture data", line_index + 1));
            }
            events.push(event);
        }
        Ok(Self { events })
    }
}

fn phase_token(phase: InputPhase) -> &'static str {
    match phase {
        InputPhase::Press => "press",
        InputPhase::Repeat => "repeat",
        InputPhase::Release => "release",
    }
}

fn parse_phase(token: &str) -> Option<InputPhase> {
    match token {
        "press" => Some(InputPhase::Press),
        "repeat" => Some(InputPhase::Repeat),
        "release" => Some(InputPhase::Release),
        _ => None,
    }
}

#[derive(Clone)]
struct FakeClock(Rc<Cell<Duration>>);

impl FakeClock {
    fn new() -> Self {
        Self(Rc::new(Cell::new(Duration::ZERO)))
    }

    fn advance(&self, duration: Duration) {
        self.0.set(self.0.get().saturating_add(duration));
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Duration {
        self.0.get()
    }
}

enum PlaybackEvent {
    Advance(Duration),
    Transport(TransportEvent),
    TickBoundary,
    IdleBoundary,
}

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
            let after_ms = match event {
                TraceEvent::Key { after_ms, .. }
                | TraceEvent::Resize { after_ms, .. }
                | TraceEvent::Tick { after_ms }
                | TraceEvent::Idle { after_ms }
                | TraceEvent::Redacted { after_ms, .. }
                | TraceEvent::Focus { after_ms, .. }
                | TraceEvent::Shutdown { after_ms } => *after_ms,
            };
            events.push_back(PlaybackEvent::Advance(Duration::from_millis(after_ms)));
            match event {
                TraceEvent::Key {
                    code,
                    phase,
                    modifiers,
                    repeat_count,
                    ..
                } => {
                    events.push_back(PlaybackEvent::Transport(TransportEvent::Key {
                        key: TransportKey {
                            code: code.clone(),
                            modifiers: Modifiers::from_bits(*modifiers),
                        },
                        phase: *phase,
                        repeat_count: *repeat_count,
                    }));
                }
                TraceEvent::Resize { width, height, .. } => {
                    events.push_back(PlaybackEvent::Transport(TransportEvent::Resize {
                        width: *width,
                        height: *height,
                    }));
                }
                TraceEvent::Focus { gained, .. } => {
                    events.push_back(PlaybackEvent::Transport(if *gained {
                        TransportEvent::FocusGained
                    } else {
                        TransportEvent::FocusLost
                    }));
                }
                TraceEvent::Shutdown { .. } => {
                    events.push_back(PlaybackEvent::Transport(TransportEvent::Shutdown));
                }
                TraceEvent::Tick { .. } => events.push_back(PlaybackEvent::TickBoundary),
                TraceEvent::Idle { .. } => events.push_back(PlaybackEvent::IdleBoundary),
                TraceEvent::Redacted { .. } => {}
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

    fn read(&mut self) -> io::Result<TransportEvent> {
        self.clock.advance(EVENT_COST);
        match self.events.pop_front() {
            Some(PlaybackEvent::Transport(event)) => Ok(event),
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
    width: u16,
    height: u16,
    symbols: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReplayResult {
    model: InteractionModel,
    session_generation: u64,
    effects: Vec<String>,
    frames: Vec<FrameRecord>,
    max_queue: usize,
    unsupported_holds: usize,
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

#[derive(Clone, Debug, PartialEq, Eq)]
struct ActionRecord {
    action: SemanticAction,
    before: InteractionModel,
    after: InteractionModel,
    effects: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
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
    InvalidPerformanceState {
        detail: String,
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
    UnsupportedHoldCount {
        expected: usize,
        observed: usize,
    },
    DeferredHoldMissing {
        expected: usize,
        observed: usize,
    },
    RenderError {
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum ViolationKey {
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
    InvalidPerformanceState(String),
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
        action: String,
        before: String,
        after: String,
        effects: String,
    },
    UnsupportedHoldCount {
        expected: usize,
        observed: usize,
    },
    DeferredHoldMissing {
        expected: usize,
        observed: usize,
    },
    RenderError {
        message: String,
    },
}

impl PropertyViolation {
    fn key(&self) -> ViolationKey {
        match self {
            Self::SourceError { kind, message } => ViolationKey::SourceError {
                kind: *kind,
                message: message.clone(),
            },
            Self::SchedulerDidNotConverge { turn_limit } => ViolationKey::SchedulerDidNotConverge {
                turn_limit: *turn_limit,
            },
            Self::KeyboardOwnerMismatch { expected, observed } => {
                ViolationKey::KeyboardOwnerMismatch {
                    expected: expected.clone(),
                    observed: observed.clone(),
                }
            }
            Self::InvalidPerformanceState { detail } => {
                ViolationKey::InvalidPerformanceState(detail.clone())
            }
            Self::AcceptedFrameDeadlineExceeded { elapsed } => {
                ViolationKey::AcceptedFrameDeadlineExceeded { elapsed: *elapsed }
            }
            Self::QueueCapacityExceeded { observed, capacity } => {
                ViolationKey::QueueCapacityExceeded {
                    observed: *observed,
                    capacity: *capacity,
                }
            }
            Self::FrameGapExceeded { gap } => ViolationKey::FrameGapExceeded { gap: *gap },
            Self::NondeterministicReplay { signature } => ViolationKey::NondeterministicReplay {
                signature: signature.clone(),
            },
            Self::EdgeChangedOnNonPress { record } => ViolationKey::EdgeChangedOnNonPress {
                action: format!("{:?}", record.action),
                before: format!("{:?}", record.before),
                after: format!("{:?}", record.after),
                effects: format!("{:?}", record.effects),
            },
            Self::UnsupportedHoldCount { expected, observed } => {
                ViolationKey::UnsupportedHoldCount {
                    expected: *expected,
                    observed: *observed,
                }
            }
            Self::DeferredHoldMissing { expected, observed } => ViolationKey::DeferredHoldMissing {
                expected: *expected,
                observed: *observed,
            },
            Self::RenderError { message } => ViolationKey::RenderError {
                message: message.clone(),
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReplayOutcome {
    result: ReplayResult,
    violation: Option<PropertyViolation>,
}

#[derive(Default)]
struct FakeClipboard {
    writes: usize,
}

impl Clipboard for FakeClipboard {
    fn set_text(&mut self, _text: String) -> Result<(), String> {
        self.writes += 1;
        Ok(())
    }
}

struct ReplayHarness {
    capabilities: TerminalCapabilities,
    model: InteractionModel,
    executor: EffectExecutor,
    scheduler: Scheduler,
    clock: FakeClock,
    fluid: FluidState,
    telemetry: FluidTelemetry,
    flipped: FlippedUnits,
    mute: MuteState,
    width: u16,
    height: u16,
    effects: Vec<String>,
    frames: Vec<FrameRecord>,
    max_queue: usize,
    unsupported_holds: usize,
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

impl ReplayHarness {
    fn new(capabilities: TerminalCapabilities) -> Self {
        let session =
            LiveSession::new(LiveSessionSnapshot::from_controls(FluidControls::default()));
        let executor = EffectExecutor::new(
            session,
            AutoControls::new(no_morph(), Vec::new(), DEFAULT_AUTO_BARS),
        );
        let clock = FakeClock::new();
        Self {
            capabilities,
            model: InteractionModel::default(),
            executor,
            scheduler: Scheduler::new(SchedulerConfig::default(), Duration::ZERO),
            clock,
            fluid: FluidState::new(),
            telemetry: FluidTelemetry::default(),
            flipped: FlippedUnits::new(),
            mute: [None; 9],
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            effects: Vec::new(),
            frames: Vec::new(),
            max_queue: 0,
            unsupported_holds: 0,
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

    fn replay(mut self, trace: &ReplayTrace, staged_holds: &[SemanticAction]) -> ReplayOutcome {
        let mut source = ScriptedSource::new(trace, self.capabilities, self.clock.clone());
        let mut turns = 0;
        let turn_limit = trace.events.len().saturating_mul(32).saturating_add(128);
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
            let shutdown_seen = turn.shutdown_seen;
            let (explicit_ticks, idle_boundaries) = source.take_boundaries();
            self.idle_boundaries += idle_boundaries;
            self.idle_turn_ids
                .extend(std::iter::repeat_n(turns as u64, idle_boundaries));
            for _ in 0..explicit_ticks {
                self.telemetry.publish_beat(self.clock.now().as_secs_f64());
                self.fluid.tick(
                    SchedulerConfig::default().tick_interval.as_secs_f32(),
                    &self.telemetry,
                );
                self.explicit_ticks += 1;
                self.explicit_tick_turn_ids.push(turns as u64);
            }
            self.max_queue = self.max_queue.max(self.scheduler.queue_len());
            self.max_queue = self.max_queue.max(turn.events.len());
            for event in turn.events {
                self.handle_transport(event);
                if self.violation.is_some() {
                    break;
                }
            }
            if self.violation.is_some() {
                break;
            }
            if turn.tick_due {
                self.telemetry.publish_beat(self.clock.now().as_secs_f64());
                self.fluid.tick(
                    SchedulerConfig::default().tick_interval.as_secs_f32(),
                    &self.telemetry,
                );
                self.scheduler.complete_tick(self.clock.now());
            }
            if turn.render_due {
                self.render();
            }
            if shutdown_seen || self.violation.is_some() {
                break;
            }
        }
        let deferred_before_staged = self.deferred_inputs.len();
        for action in staged_holds {
            self.apply_staged_hold(*action);
            if self.violation.is_some() {
                break;
            }
        }
        let accepted_staged_holds = staged_holds
            .iter()
            .filter(|action| action.intent.phase_policy().accepts(action.phase))
            .count();
        let observed_staged_deferred = self
            .deferred_inputs
            .len()
            .saturating_sub(deferred_before_staged);
        if self.violation.is_none()
            && !staged_holds.is_empty()
            && self.capabilities.supports_holds()
            && self.unsupported_holds != accepted_staged_holds
        {
            self.violate(PropertyViolation::UnsupportedHoldCount {
                expected: accepted_staged_holds,
                observed: self.unsupported_holds,
            });
        } else if self.violation.is_none()
            && !staged_holds.is_empty()
            && !self.capabilities.supports_holds()
            && observed_staged_deferred != accepted_staged_holds
        {
            self.violate(PropertyViolation::DeferredHoldMissing {
                expected: accepted_staged_holds,
                observed: observed_staged_deferred,
            });
        }
        if self.violation.is_none() && self.scheduler.render_due(self.clock.now()) {
            self.render();
        }
        let mut violation = self.violation.take();
        let result = ReplayResult {
            model: self.model,
            session_generation: self.executor.session().load().generation,
            effects: self.effects,
            frames: self.frames,
            max_queue: self.max_queue,
            unsupported_holds: self.unsupported_holds,
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

    fn handle_transport(&mut self, event: TransportEvent) {
        match &event {
            TransportEvent::Key {
                key: _,
                phase: _,
                repeat_count,
            } => {
                for _ in 0..*repeat_count {
                    match map_input(
                        &self.model.mode,
                        self.model.navigation,
                        &event,
                        self.capabilities,
                    ) {
                        InputMapping::Action(action) => self.apply(action),
                        InputMapping::Deferred(reason) => {
                            self.deferred_inputs.push(format!("{reason:?}"));
                        }
                        InputMapping::Ignored => {}
                    }
                }
            }
            TransportEvent::Resize { width, height } => {
                self.width = (*width).max(MIN_TERMINAL_WIDTH);
                self.height = (*height).max(MIN_TERMINAL_HEIGHT);
                self.scheduler.request_frame();
                self.requested_at.get_or_insert(self.clock.now());
            }
            TransportEvent::FocusGained
            | TransportEvent::FocusLost
            | TransportEvent::Paste(_)
            | TransportEvent::Mouse(_)
            | TransportEvent::Shutdown => {}
        }
    }

    /// Test-only staged adapter for hold semantics. Stint 0043 replaces this
    /// seam with canonical raw bindings; it is deliberately absent from the
    /// sanitized recorder and persisted fixture grammar.
    fn apply_staged_hold(&mut self, action: SemanticAction) {
        if !matches!(
            action.intent,
            Intent::HoldPerformanceSelector(_) | Intent::ReleaseHeldSelector
        ) {
            self.violate(PropertyViolation::InvalidPerformanceState {
                detail: format!("non-hold action passed to staged hold adapter: {action:?}"),
            });
            return;
        }
        if !action.intent.phase_policy().accepts(action.phase) {
            return;
        }
        if self.capabilities.supports_holds() {
            self.apply(action);
        } else {
            self.deferred_inputs
                .push("PerformanceGrammar0043(press-only hold fallback is unsupported)".into());
        }
    }

    fn apply(&mut self, action: SemanticAction) {
        let before = self.model.clone();
        let effect_start = self.effects.len();
        let selected_control = self.selected_control();
        let transition = self.model.clone().update(action);
        let changed = transition.model != self.model || !transition.effects.is_empty();
        self.model = transition.model;
        let emitted = transition.effects;
        let edge_changed = action.phase != InputPhase::Press
            && action.intent.phase_policy() == PhasePolicy::Edge
            && (self.model != before || !emitted.is_empty());
        let results = self.executor.execute_interactions_ordered_with_clipboard(
            emitted.clone(),
            &InteractionExecutionContext {
                selected_control,
                beat: self.clock.now().as_secs_f64(),
            },
            &mut self.clipboard,
        );
        for (effect, result) in emitted.into_iter().zip(results) {
            let effect_label = format!("{effect:?}");
            if matches!(
                result,
                Err(EffectFailure::UnsupportedInteraction(
                    InteractionEffect::HoldPerformanceSelector(_)
                        | InteractionEffect::ReleaseHeldSelector(_)
                ))
            ) {
                self.unsupported_holds += 1;
            }
            let result_label = match result {
                Ok(acknowledgement) => acknowledgement_label(&acknowledgement),
                Err(failure) => format!("ERR:{failure:?}"),
            };
            self.effects.push(format!("{effect_label}=>{result_label}"));
        }
        let record = ActionRecord {
            action,
            before: before.clone(),
            after: self.model.clone(),
            effects: self.effects[effect_start..].to_vec(),
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

    fn selected_control(&self) -> Option<&'static str> {
        let session = self.executor.session().load();
        let view = self.project(&session);
        view.items.get(view.navigation.selected).map(|item| item.id)
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
                mute: &self.mute,
                cursor_visible: true,
                notices: ViewNotices::default(),
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
            return;
        }
        let invalid_performance = match &self.model.mode {
            InteractionMode::Performance(PerformanceMode::Deck {
                selected,
                held_selector,
            }) => {
                if !selected.is_none_or(|value| value < 4)
                    || !held_selector.is_none_or(|value| value < 4)
                {
                    Some(format!(
                        "deck selected={selected:?} held_selector={held_selector:?}"
                    ))
                } else {
                    None
                }
            }
            InteractionMode::Performance(PerformanceMode::Sequence {
                stage,
                held_selector,
            }) => {
                if !held_selector.is_none_or(|value| value < 4) {
                    Some(format!("sequence held_selector={held_selector:?}"))
                } else if let SequenceStage::Perform { instrument } = stage
                    && *instrument >= 4
                {
                    Some(format!("sequence instrument={instrument}"))
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(detail) = invalid_performance {
            self.violate(PropertyViolation::InvalidPerformanceState { detail });
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

fn replay(trace: &[TraceEvent], capabilities: TerminalCapabilities) -> ReplayResult {
    let run = |candidate: &[TraceEvent]| {
        ReplayHarness::new(capabilities).replay(
            &ReplayTrace {
                events: candidate.to_vec(),
            },
            &[],
        )
    };
    checked_replay(run(trace), trace, run)
}

fn replay_from_model(
    model: InteractionModel,
    trace: &[TraceEvent],
    capabilities: TerminalCapabilities,
) -> ReplayResult {
    let run = |candidate: &[TraceEvent]| {
        ReplayHarness::new(capabilities)
            .with_model(model.clone())
            .replay(
                &ReplayTrace {
                    events: candidate.to_vec(),
                },
                &[],
            )
    };
    checked_replay(run(trace), trace, run)
}

fn replay_with_staged_holds(
    trace: &[TraceEvent],
    staged_holds: &[SemanticAction],
    capabilities: TerminalCapabilities,
) -> ReplayResult {
    let run = |candidate: &[TraceEvent]| {
        ReplayHarness::new(capabilities).replay(
            &ReplayTrace {
                events: candidate.to_vec(),
            },
            staged_holds,
        )
    };
    checked_replay(run(trace), trace, run)
}

fn checked_replay(
    outcome: ReplayOutcome,
    trace: &[TraceEvent],
    mut rerun: impl FnMut(&[TraceEvent]) -> ReplayOutcome,
) -> ReplayResult {
    if let Some(violation) = &outcome.violation {
        let key = violation.key();
        let minimal = minimize_trace(trace.to_vec(), |candidate| {
            rerun(candidate)
                .violation
                .as_ref()
                .is_some_and(|candidate| candidate.key() == key)
        });
        let minimized = rerun(&minimal);
        let minimized_violation = minimized
            .violation
            .as_ref()
            .filter(|candidate| candidate.key() == key)
            .unwrap_or(violation);
        panic!(
            "{}",
            format_property_diagnostic(
                minimized_violation,
                &minimal,
                &minimized.result,
                &minimized.result
            )
        );
    }
    outcome.result
}

fn full_capabilities() -> TerminalCapabilities {
    TerminalCapabilities {
        key_event_types: true,
        plain_key_releases: true,
    }
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
        full_capabilities(),
    );
    if let TransportEvent::Key { repeat_count, .. } = &mut repeated {
        *repeat_count = 4;
    }
    let events = [
        (
            1,
            normalize_key_event(
                KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
                full_capabilities(),
            ),
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
                full_capabilities(),
            ),
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
                full_capabilities(),
            ),
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
                        full_capabilities(),
                    );
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
                    let outcome = replay_outcome(&trace.events, full_capabilities());
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
        let mut source = ScriptedSource::new(&parsed, full_capabilities(), clock);
        assert!(source.poll(Duration::ZERO).expect("poll"));
        let TransportEvent::Key { key, .. } = source.read().expect("read") else {
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
        let mut source = ScriptedSource::new(&trace, full_capabilities(), clock.clone());
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
        full_capabilities(),
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
    assert!(matches!(
        result.model.mode,
        InteractionMode::Performance(PerformanceMode::Deck { .. })
    ));
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
            full_capabilities(),
        ),
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
    let result = replay(&trace.events, full_capabilities());
    assert!(matches!(
        result.model.navigation,
        Navigation::Master {
            drill: MasterDrill::None,
            ..
        }
    ));
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
    let mut source = ScriptedSource::new(&trace, full_capabilities(), clock.clone());
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
    let mut source = ScriptedSource::new(&trace, full_capabilities(), clock.clone());
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
            "DECK",
        ),
        (
            "double Space",
            vec![
                key(0, FixtureKey::Character(' '), InputPhase::Press),
                key(0, FixtureKey::Character(' '), InputPhase::Press),
            ],
            "SEQUENCE",
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
        let first = replay(&trace, full_capabilities());
        let second = replay(&trace, full_capabilities());
        assert_eq!(first, second, "{name} was not deterministic");
        assert_eq!(
            first.frames.last().map(|frame| frame.owner.as_str()),
            Some(expected_owner),
            "{name}"
        );
        if let Some(violation) = post_replay_violation(&first) {
            panic!(
                "{}",
                format_property_diagnostic(&violation, &trace, &first, &second)
            );
        }
    }

    let held = replay_with_staged_holds(
        &[
            key(0, FixtureKey::Character(' '), InputPhase::Press),
            key(0, FixtureKey::Character('a'), InputPhase::Press),
        ],
        &[
            SemanticAction {
                phase: InputPhase::Press,
                intent: Intent::HoldPerformanceSelector(1),
            },
            SemanticAction {
                phase: InputPhase::Release,
                intent: Intent::ReleaseHeldSelector,
            },
        ],
        full_capabilities(),
    );
    assert_eq!(
        held.frames.last().map(|frame| frame.owner.as_str()),
        Some("SEQUENCE")
    );
}

#[test]
fn edge_actions_are_exactly_once_and_hold_fallback_is_explicit() {
    let leaders = vec![
        key(0, FixtureKey::Character('p'), InputPhase::Press),
        key(0, FixtureKey::Character('p'), InputPhase::Repeat),
        key(0, FixtureKey::Escape, InputPhase::Press),
        key(0, FixtureKey::Character(' '), InputPhase::Press),
        key(0, FixtureKey::Character(' '), InputPhase::Repeat),
    ];
    let result = replay(&leaders, full_capabilities());
    assert!(matches!(
        result.model.mode,
        InteractionMode::Performance(PerformanceMode::Sequence {
            stage: SequenceStage::ChooseInstrument,
            ..
        })
    ));
    assert!(result.effects.is_empty());

    let quit = replay(
        &[
            key(0, FixtureKey::Character('q'), InputPhase::Press),
            key(0, FixtureKey::Character('q'), InputPhase::Repeat),
        ],
        full_capabilities(),
    );
    assert_eq!(
        quit.effects
            .iter()
            .filter(|effect| effect.starts_with("Quit"))
            .count(),
        1
    );

    let save = replay(
        &[
            modified_key(0, FixtureKey::Character('s'), InputPhase::Press, 1 << 1),
            modified_key(0, FixtureKey::Character('s'), InputPhase::Repeat, 1 << 1),
        ],
        full_capabilities(),
    );
    assert_eq!(
        save.effects
            .iter()
            .filter(|effect| effect.starts_with("Save"))
            .count(),
        1
    );
    assert_eq!(save.clipboard_writes, 1);

    let held_trace = vec![
        key(0, FixtureKey::Character(' '), InputPhase::Press),
        key(0, FixtureKey::Character('a'), InputPhase::Press),
    ];
    let staged_holds = [
        SemanticAction {
            phase: InputPhase::Press,
            intent: Intent::HoldPerformanceSelector(1),
        },
        SemanticAction {
            phase: InputPhase::Repeat,
            intent: Intent::HoldPerformanceSelector(1),
        },
        SemanticAction {
            phase: InputPhase::Release,
            intent: Intent::ReleaseHeldSelector,
        },
    ];
    let full = replay_with_staged_holds(&held_trace, &staged_holds, full_capabilities());
    assert_eq!(
        full.effects
            .iter()
            .filter(|effect| effect.starts_with("HoldPerformanceSelector"))
            .count(),
        1
    );
    assert_eq!(
        full.effects
            .iter()
            .filter(|effect| effect.starts_with("ReleaseHeldSelector"))
            .count(),
        1
    );
    assert_eq!(full.unsupported_holds, 2);

    let fallback =
        replay_with_staged_holds(&held_trace, &staged_holds, TerminalCapabilities::default());
    assert_eq!(fallback.unsupported_holds, 0);
    assert_eq!(
        fallback.deferred_inputs.len(),
        2,
        "Press hold and Release-held defer; phase-rejected Repeat is ignored"
    );
    assert!(
        fallback
            .effects
            .iter()
            .all(|effect| !effect.starts_with("HoldPerformanceSelector")
                && !effect.starts_with("ReleaseHeldSelector"))
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
    for (code, owner) in [
        (FixtureKey::Character('/'), "PALETTE"),
        (FixtureKey::Character('p'), "DECK"),
        (FixtureKey::Character(' '), "SEQUENCE"),
    ] {
        let result = replay(&repeated(code), full_capabilities());
        assert_eq!(
            result.frames.last().map(|frame| frame.owner.as_str()),
            Some(owner)
        );
    }

    let numeric = replay_from_model(
        InteractionModel {
            mode: InteractionMode::Numeric(NumericEntry {
                buffer: "88".into(),
            }),
            ..InteractionModel::default()
        },
        &repeated(FixtureKey::Enter),
        full_capabilities(),
    );
    assert_eq!(
        numeric
            .effects
            .iter()
            .filter(|effect| effect.starts_with("CommitNumeric"))
            .count(),
        1
    );

    let palette = replay_from_model(
        InteractionModel {
            mode: InteractionMode::Palette(PaletteMode::default()),
            ..InteractionModel::default()
        },
        &repeated(FixtureKey::Tab),
        full_capabilities(),
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
            mode: InteractionMode::Performance(PerformanceMode::Deck {
                selected: None,
                held_selector: None,
            }),
            ..InteractionModel::default()
        },
        &repeated(FixtureKey::Character('a')),
        full_capabilities(),
    );
    assert_eq!(
        performance
            .effects
            .iter()
            .filter(|effect| effect.starts_with("PerformanceInstrument"))
            .count(),
        1
    );

    let drill = replay_from_model(
        InteractionModel {
            navigation: Navigation::Chords {
                selected: 2,
                drill: ChordDrill::Progression { return_to: 0 },
            },
            mode: InteractionMode::Browsing,
        },
        &repeated(FixtureKey::Enter),
        full_capabilities(),
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
        Intent::EnterMasterCompression,
        Intent::BeginNumeric('1'),
        Intent::TypeCharacter('x'),
        Intent::Backspace,
        Intent::PaletteAutocomplete,
        Intent::Confirm,
        Intent::OpenPalette,
        Intent::OpenAutomation(AutomationKind::Lfo),
        Intent::OpenAutomationField,
        Intent::ActivatePerformance(PerformanceKind::Deck),
        Intent::SelectPerformanceInstrument {
            instrument: 0,
            page: super::interaction::Page::Chords,
        },
        Intent::HoldPerformanceSelector(0),
        Intent::AdjustSelected(1),
        Intent::Save,
        Intent::Quit,
        Intent::ReleaseHeldSelector,
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
            let mut harness = ReplayHarness::new(full_capabilities());
            harness.apply(SemanticAction { phase, intent });
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
            }),
            ..InteractionModel::default()
        },
        InteractionModel {
            mode: InteractionMode::Palette(PaletteMode::default()),
            ..InteractionModel::default()
        },
        InteractionModel {
            mode: InteractionMode::Automation(AutomationMode::Lfo {
                depth: LfoDepth::Editor,
                selected: 0,
            }),
            ..InteractionModel::default()
        },
        InteractionModel {
            mode: InteractionMode::Automation(AutomationMode::Lfo {
                depth: LfoDepth::NestedField,
                selected: 3,
            }),
            ..InteractionModel::default()
        },
        InteractionModel {
            mode: InteractionMode::Automation(AutomationMode::Envelope { selected: 2 }),
            ..InteractionModel::default()
        },
        InteractionModel {
            mode: InteractionMode::Automation(AutomationMode::Macro { selected: 1 }),
            ..InteractionModel::default()
        },
        InteractionModel {
            mode: InteractionMode::Performance(PerformanceMode::Deck {
                selected: Some(1),
                held_selector: None,
            }),
            ..InteractionModel::default()
        },
        InteractionModel {
            mode: InteractionMode::Performance(PerformanceMode::Sequence {
                stage: SequenceStage::ChooseInstrument,
                held_selector: None,
            }),
            ..InteractionModel::default()
        },
        InteractionModel {
            mode: InteractionMode::Performance(PerformanceMode::Sequence {
                stage: SequenceStage::Perform { instrument: 2 },
                held_selector: Some(1),
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
        },
        InteractionModel {
            navigation: Navigation::Master {
                selected: 1,
                drill: MasterDrill::Compression { return_to: 0 },
            },
            mode: InteractionMode::Browsing,
        },
    ];
    for model in models {
        let result = replay_from_model(
            model,
            &[escape.clone(), escape.clone(), escape.clone()],
            full_capabilities(),
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
                } | Navigation::Master {
                    drill: MasterDrill::None,
                    ..
                } | Navigation::Standard { .. }
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
    let result = replay(&trace, full_capabilities());
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
        for capabilities in [full_capabilities(), TerminalCapabilities::default()] {
            let first = replay_outcome(&trace, capabilities);
            let second = replay_outcome(&trace, capabilities);
            observed_edge_actions += first
                .result
                .state_history
                .iter()
                .filter(|record| record.action.intent.phase_policy() == PhasePolicy::Edge)
                .count();
            if let Some(class) = nondeterministic_violation(&first.result, &second.result) {
                let key = class.key();
                let minimal = minimize_trace(trace.clone(), |candidate| {
                    let left = replay_outcome(candidate, capabilities);
                    let right = replay_outcome(candidate, capabilities);
                    nondeterministic_violation(&left.result, &right.result)
                        .is_some_and(|candidate| candidate.key() == key)
                });
                let left = replay_outcome(&minimal, capabilities);
                let right = replay_outcome(&minimal, capabilities);
                panic!(
                    "{}",
                    format_property_diagnostic(&class, &minimal, &left.result, &right.result)
                );
            }
            if let Some(violation) = first.violation.clone() {
                let violation_key = violation.key();
                let minimal = minimize_trace(trace.clone(), |candidate| {
                    replay_outcome(candidate, capabilities)
                        .violation
                        .as_ref()
                        .is_some_and(|candidate| candidate.key() == violation_key)
                });
                let left = replay_outcome(&minimal, capabilities);
                let right = replay_outcome(&minimal, capabilities);
                panic!(
                    "{}",
                    format_property_diagnostic(&violation, &minimal, &left.result, &right.result)
                );
            }
        }
    }
    assert!(
        observed_edge_actions > 0,
        "generator missed all edge actions"
    );
}

fn replay_outcome(trace: &[TraceEvent], capabilities: TerminalCapabilities) -> ReplayOutcome {
    ReplayHarness::new(capabilities).replay(
        &ReplayTrace {
            events: trace.to_vec(),
        },
        &[],
    )
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
    first_field!(effects);
    first_field!(max_queue);
    first_field!(unsupported_holds);
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

fn first_sequence_divergence<T: std::fmt::Debug + PartialEq>(
    field: &str,
    left: &[T],
    right: &[T],
) -> Option<DivergenceSignature> {
    for (index, (left_item, right_item)) in left.iter().zip(right).enumerate() {
        if left_item != right_item {
            return Some(DivergenceSignature {
                field: format!("{field}[{index}]"),
                left: format!("{left_item:?}"),
                right: format!("{right_item:?}"),
            });
        }
    }
    (left.len() != right.len()).then(|| DivergenceSignature {
        field: format!("{field}.len"),
        left: left.len().to_string(),
        right: right.len().to_string(),
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
    let divergent = left
        .state_history
        .iter()
        .zip(&right.state_history)
        .find(|(left, right)| left != right)
        .map(|(left, right)| (Some(left), Some(right)))
        .unwrap_or_else(|| (left.state_history.last(), right.state_history.last()));
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
fn violation_keys_retain_causal_action_identity() {
    let violation = |intent| PropertyViolation::EdgeChangedOnNonPress {
        record: Box::new(ActionRecord {
            action: SemanticAction {
                phase: InputPhase::Repeat,
                intent,
            },
            before: InteractionModel::default(),
            after: InteractionModel::default(),
            effects: vec!["unexpected".into()],
        }),
    };
    assert_ne!(violation(Intent::Save).key(), violation(Intent::Quit).key());

    let record_violation =
        |before: InteractionModel, after: InteractionModel, effects: Vec<String>| {
            PropertyViolation::EdgeChangedOnNonPress {
                record: Box::new(ActionRecord {
                    action: SemanticAction {
                        phase: InputPhase::Repeat,
                        intent: Intent::Save,
                    },
                    before,
                    after,
                    effects,
                }),
            }
        };
    let base = record_violation(
        InteractionModel::default(),
        InteractionModel::default(),
        vec![],
    );
    let different_before = record_violation(
        InteractionModel {
            mode: InteractionMode::Numeric(NumericEntry { buffer: "1".into() }),
            ..InteractionModel::default()
        },
        InteractionModel::default(),
        vec![],
    );
    let different_after = record_violation(
        InteractionModel::default(),
        InteractionModel {
            mode: InteractionMode::Palette(PaletteMode::default()),
            ..InteractionModel::default()
        },
        vec![],
    );
    let different_effects = record_violation(
        InteractionModel::default(),
        InteractionModel::default(),
        vec!["Save=>unexpected".into()],
    );
    for different in [different_before, different_after, different_effects] {
        assert_ne!(base.key(), different.key());
    }

    let pairs = [
        (
            PropertyViolation::SourceError {
                kind: io::ErrorKind::Other,
                message: "left".into(),
            },
            PropertyViolation::SourceError {
                kind: io::ErrorKind::Other,
                message: "right".into(),
            },
        ),
        (
            PropertyViolation::SourceError {
                kind: io::ErrorKind::Other,
                message: "same".into(),
            },
            PropertyViolation::SourceError {
                kind: io::ErrorKind::WouldBlock,
                message: "same".into(),
            },
        ),
        (
            PropertyViolation::SchedulerDidNotConverge { turn_limit: 10 },
            PropertyViolation::SchedulerDidNotConverge { turn_limit: 11 },
        ),
        (
            PropertyViolation::KeyboardOwnerMismatch {
                expected: "BROWSE".into(),
                observed: "DECK".into(),
            },
            PropertyViolation::KeyboardOwnerMismatch {
                expected: "BROWSE".into(),
                observed: "SEQUENCE".into(),
            },
        ),
        (
            PropertyViolation::KeyboardOwnerMismatch {
                expected: "BROWSE".into(),
                observed: "DECK".into(),
            },
            PropertyViolation::KeyboardOwnerMismatch {
                expected: "PALETTE".into(),
                observed: "DECK".into(),
            },
        ),
        (
            PropertyViolation::InvalidPerformanceState {
                detail: "selector=4".into(),
            },
            PropertyViolation::InvalidPerformanceState {
                detail: "selector=5".into(),
            },
        ),
        (
            PropertyViolation::AcceptedFrameDeadlineExceeded {
                elapsed: Duration::from_millis(51),
            },
            PropertyViolation::AcceptedFrameDeadlineExceeded {
                elapsed: Duration::from_millis(52),
            },
        ),
        (
            PropertyViolation::QueueCapacityExceeded {
                observed: 65,
                capacity: 64,
            },
            PropertyViolation::QueueCapacityExceeded {
                observed: 66,
                capacity: 64,
            },
        ),
        (
            PropertyViolation::QueueCapacityExceeded {
                observed: 65,
                capacity: 64,
            },
            PropertyViolation::QueueCapacityExceeded {
                observed: 65,
                capacity: 63,
            },
        ),
        (
            PropertyViolation::FrameGapExceeded {
                gap: Duration::from_millis(51),
            },
            PropertyViolation::FrameGapExceeded {
                gap: Duration::from_millis(52),
            },
        ),
        (
            PropertyViolation::UnsupportedHoldCount {
                expected: 1,
                observed: 0,
            },
            PropertyViolation::UnsupportedHoldCount {
                expected: 2,
                observed: 0,
            },
        ),
        (
            PropertyViolation::UnsupportedHoldCount {
                expected: 1,
                observed: 0,
            },
            PropertyViolation::UnsupportedHoldCount {
                expected: 1,
                observed: 2,
            },
        ),
        (
            PropertyViolation::DeferredHoldMissing {
                expected: 1,
                observed: 0,
            },
            PropertyViolation::DeferredHoldMissing {
                expected: 1,
                observed: 2,
            },
        ),
        (
            PropertyViolation::DeferredHoldMissing {
                expected: 1,
                observed: 0,
            },
            PropertyViolation::DeferredHoldMissing {
                expected: 2,
                observed: 0,
            },
        ),
        (
            PropertyViolation::RenderError {
                message: "left".into(),
            },
            PropertyViolation::RenderError {
                message: "right".into(),
            },
        ),
    ];
    for (left, right) in pairs {
        assert_ne!(left.key(), right.key(), "{left:?} and {right:?}");
    }
}

#[test]
fn nondeterministic_keys_and_minimizer_predicates_retain_the_exact_divergence() {
    let baseline = replay(&[], full_capabilities());
    let mut queue_divergence = baseline.clone();
    queue_divergence.max_queue += 1;
    let mut clipboard_divergence = baseline.clone();
    clipboard_divergence.clipboard_writes += 1;

    let queue_violation = nondeterministic_violation(&baseline, &queue_divergence)
        .expect("queue difference must produce a signature");
    let clipboard_violation = nondeterministic_violation(&baseline, &clipboard_divergence)
        .expect("clipboard difference must produce a signature");
    assert_ne!(queue_violation.key(), clipboard_violation.key());

    let target = queue_violation.key();
    let matches_minimizer_target = |candidate: &PropertyViolation| candidate.key() == target;
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
    let target_key = synthetic_violation(std::slice::from_ref(&target_event))
        .expect("target event must diverge")
        .key();
    let competitor_key =
        synthetic_violation(&[key(0, FixtureKey::Character('q'), InputPhase::Press)])
            .expect("competitor event must diverge")
            .key();
    assert_ne!(target_key, competitor_key);

    let minimal = minimize_trace(original, |candidate| {
        synthetic_violation(candidate).is_some_and(|violation| violation.key() == target_key)
    });
    let minimized_violation =
        synthetic_violation(&minimal).expect("minimized trace must still diverge");
    assert_eq!(minimized_violation.key(), target_key);
    assert_ne!(minimized_violation.key(), competitor_key);
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
                    mode: InteractionMode::Numeric(NumericEntry { buffer: "1".into() }),
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
                    mode: InteractionMode::Numeric(NumericEntry { buffer: "2".into() }),
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
    let target_key = edge_violation(std::slice::from_ref(&target_event))
        .expect("target event must violate")
        .key();
    let competitor_key = edge_violation(&[key(0, FixtureKey::Character('q'), InputPhase::Repeat)])
        .expect("competitor event must violate")
        .key();
    assert_ne!(target_key, competitor_key);

    let minimal = minimize_trace(original, |candidate| {
        edge_violation(candidate).is_some_and(|violation| violation.key() == target_key)
    });
    let minimized_violation =
        edge_violation(&minimal).expect("minimized trace must retain an edge violation");
    assert_eq!(minimized_violation.key(), target_key);
    assert_ne!(minimized_violation.key(), competitor_key);
    assert_eq!(minimal, vec![target_event]);
    assert!(ReplayTrace::parse(&ReplayTrace { events: minimal }.fixture()).is_ok());
}

#[test]
fn property_diagnostic_contains_minimal_trace_and_transition_details() {
    let minimal = vec![key(0, FixtureKey::Character('p'), InputPhase::Press)];
    let left = replay(&minimal, full_capabilities());
    let right = replay(
        &[key(0, FixtureKey::Character('/'), InputPhase::Press)],
        full_capabilities(),
    );
    let left_record = &left.state_history[0];
    assert_eq!(
        left_record.action,
        SemanticAction {
            phase: InputPhase::Press,
            intent: Intent::ActivatePerformance(PerformanceKind::Deck),
        }
    );
    assert_eq!(left_record.before, InteractionModel::default());
    assert!(matches!(
        left_record.after.mode,
        InteractionMode::Performance(PerformanceMode::Deck {
            selected: None,
            held_selector: None,
        })
    ));
    assert_eq!(left_record.effects, Vec::<String>::new());
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
        "ActivatePerformance(Deck)",
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
