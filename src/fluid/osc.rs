//! Opt-in bidirectional OSC feed for external visualizers (`foorm`) and
//! hardware/surface bridges (`baton`):
//! `nooise --osc[=ADDR]` and `nooise --osc-in[=ADDR]`.
//!
//! Outbound: one thread polls `FluidTelemetry`, the shared `OscUiSnapshot`, and
//! the audio engine's `OscVoiceEvent` channel, mirroring changes to a UDP
//! socket as OSC bundles. The audio thread never blocks on network I/O, and an
//! absent, slow, or dead consumer can never back-pressure playback.
//!
//! Inbound: an optional UDP listener parses `/nooise/cmd/*` packets into
//! semantic UI commands (`OscUiCommand`, drained on the coordinator turn loop)
//! and low-latency voice note commands (`OscVoiceCommand`, drained on the
//! audio engine's control tick). Zero hardware-specific or APC40-specific
//! concepts live here: `nooise` speaks only its own instrument-centric domain
//! vocabulary.

use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use rosc::{OscBundle, OscMessage, OscPacket, OscTime, OscType, decoder, encoder};

use super::controls::{FluidControls, LEAD_STEP_COUNT, PAD_STEP_COUNT};
use super::gesture::{GESTURE_COUNT, GestureKind};
use super::interaction::{InteractionMode, InteractionModel};
use super::planned::PlannedAction;
use super::registry::{ControlKind, TAB_COUNT, Tab, module_slot_collapsed_id, spec_by_id};
use super::session::LiveSessionSnapshot;
use super::view::UiViewModel;
use super::voice::{
    ChordWindow, lead_live_step_count, lead_step_tone, pad_voicing, progression_index,
};
use super::{ChordSpan, FluidTelemetry, MUSICAL_HIT_COUNT, MusicalHit, Transport};

pub(crate) const ADDR_BEAT: &str = "/nooise/beat";
pub(crate) const ADDR_LEVEL: &str = "/nooise/level";
/// Voice names in `Tab` order, minus `Tab::Master` (which is `ADDR_LEVEL`).
pub(crate) const VOICES: [&str; TAB_COUNT - 1] = [
    "pad", "perc", "bass", "kick", "tonal", "clap", "arp", "lead",
];
/// Layer names in `Tab::all()` order, including `"master"` at index 8.
pub(crate) const LAYERS: [&str; TAB_COUNT] = [
    "pad", "perc", "bass", "kick", "tonal", "clap", "arp", "lead", "master",
];

pub(crate) fn layer_name(tab: Tab) -> &'static str {
    LAYERS[tab as usize]
}

pub(crate) fn parse_layer_name(name: &str) -> Option<Tab> {
    let trimmed = name.trim();
    if trimmed.eq_ignore_ascii_case("master") {
        Some(Tab::Master)
    } else if trimmed.eq_ignore_ascii_case("pad")
        || trimmed.eq_ignore_ascii_case("pads")
        || trimmed.eq_ignore_ascii_case("chords")
    {
        Some(Tab::Chords)
    } else if trimmed.eq_ignore_ascii_case("perc") {
        Some(Tab::Perc)
    } else if trimmed.eq_ignore_ascii_case("bass") {
        Some(Tab::Bass)
    } else if trimmed.eq_ignore_ascii_case("kick") {
        Some(Tab::Kick)
    } else if trimmed.eq_ignore_ascii_case("tonal") {
        Some(Tab::Tonal)
    } else if trimmed.eq_ignore_ascii_case("clap") {
        Some(Tab::Clap)
    } else if trimmed.eq_ignore_ascii_case("arp") {
        Some(Tab::Arp)
    } else if trimmed.eq_ignore_ascii_case("lead") {
        Some(Tab::Lead)
    } else {
        None
    }
}

fn level_addr(tab: Tab) -> String {
    match tab {
        Tab::Master => ADDR_LEVEL.to_string(),
        voice => format!("/nooise/voice/{}/level", VOICES[voice as usize]),
    }
}

/// Gesture names in `GestureKind::ALL` order.
pub(crate) const GESTURES: [&str; GESTURE_COUNT] = ["bloom", "submerge", "echo", "lift"];

fn gesture_addr(kind: GestureKind) -> String {
    format!("/nooise/gesture/{}", GESTURES[kind as usize])
}

pub(crate) fn parse_gesture_name(name: &str) -> Option<GestureKind> {
    let trimmed = name.trim();
    if trimmed.eq_ignore_ascii_case("bloom") {
        Some(GestureKind::Bloom)
    } else if trimmed.eq_ignore_ascii_case("submerge") {
        Some(GestureKind::Submerge)
    } else if trimmed.eq_ignore_ascii_case("echo") {
        Some(GestureKind::Echo)
    } else if trimmed.eq_ignore_ascii_case("lift") {
        Some(GestureKind::Lift)
    } else {
        None
    }
}

pub(crate) const ADDR_CHORD: &str = "/nooise/chord";
pub(crate) const ADDR_CHORD_CHANGE: &str = "/nooise/chord/change";
pub(crate) const ADDR_PHRASE_RESET: &str = "/nooise/phrase/reset";
pub(crate) const ADDR_PHRASE: &str = "/nooise/phrase";
pub(crate) const ADDR_CHORD_SPAN: &str = "/nooise/chord/span";
pub(crate) const ADDR_KICK: &str = "/nooise/voice/kick";

pub(crate) const ADDR_VOICE_PAD_NOTES: &str = "/nooise/voice/pad/notes";
pub(crate) const ADDR_VOICE_ARP_NOTE: &str = "/nooise/voice/arp/note";
pub(crate) const ADDR_VOICE_ARP_NOTE_OFF: &str = "/nooise/voice/arp/note_off";
pub(crate) const ADDR_VOICE_LEAD_NOTE: &str = "/nooise/voice/lead/note";
pub(crate) const ADDR_VOICE_LEAD_NOTE_OFF: &str = "/nooise/voice/lead/note_off";
pub(crate) const ADDR_VOICE_BASS_NOTE: &str = "/nooise/voice/bass/note";
pub(crate) const ADDR_VOICE_BASS_NOTE_OFF: &str = "/nooise/voice/bass/note_off";

pub(crate) const ADDR_STATE_CURSOR: &str = "/nooise/state/cursor";
pub(crate) const ADDR_STATE_PAGE_ROWS: &str = "/nooise/state/page_rows";
pub(crate) const ADDR_STATE_PAD_WINDOW: &str = "/nooise/state/pad_window";
pub(crate) const ADDR_STATE_STEPS: &str = "/nooise/state/steps";

fn state_layer_addr(tab: Tab) -> String {
    format!("/nooise/state/layer/{}", layer_name(tab))
}

/// Names in `MusicalHit::ALL` order. They are part of the OSC contract.
pub(crate) const HIT_NAMES: [&str; MUSICAL_HIT_COUNT] =
    ["beat", "pad", "perc", "kick", "tonal", "clap", "arp"];

fn hit_addr(hit: MusicalHit) -> String {
    format!("/nooise/hit/{}", HIT_NAMES[hit as usize])
}

/// How often the emitter samples telemetry and state.
const POLL_INTERVAL: Duration = Duration::from_millis(5);
const VOICE_CHANNEL_CAPACITY: usize = 256;
const UI_CMD_CHANNEL_CAPACITY: usize = 256;

// ============================================================
// Voiced note helpers and OSC voice event stream
// ============================================================

/// Up to five MIDI-numbered notes voiced by the Pad at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PadNotes {
    pub(crate) notes: [u8; 5],
    pub(crate) len: usize,
}

impl PadNotes {
    pub(crate) fn as_slice(&self) -> &[u8] {
        &self.notes[..self.len]
    }
}

pub(crate) fn tuned_note(note: i32, tune_semitones: f32) -> u8 {
    (note as f32 + tune_semitones).round().clamp(0.0, 127.0) as u8
}

pub(crate) fn pad_notes_with_count(notes: [i32; 4], count: usize, tune_semitones: f32) -> PadNotes {
    let (voiced, len) = pad_voicing(notes, count);
    let mut out = [0u8; 5];
    for (dst, &src) in out.iter_mut().zip(&voiced[..len]) {
        *dst = tuned_note(src, tune_semitones);
    }
    PadNotes { notes: out, len }
}

#[cfg(test)]
pub(crate) fn pad_notes(notes: [i32; 4], tune_semitones: f32) -> PadNotes {
    pad_notes_with_count(notes, 4, tune_semitones)
}

/// Musical note/gate events emitted by the pitched voices for external bridges.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum OscVoiceEvent {
    PadChord(PadNotes),
    PadOff,
    ArpNote { note: u8, gate_beats: f32 },
    ArpOff,
    LeadNote { note: u8, gate_beats: f32 },
    LeadOff(u8),
    BassNote { note: u8, decay_beats: f32 },
    BassOff,
}

impl OscVoiceEvent {
    pub(crate) fn into_message(self) -> OscMessage {
        match self {
            Self::PadChord(notes) => message(
                ADDR_VOICE_PAD_NOTES,
                notes
                    .as_slice()
                    .iter()
                    .map(|&n| OscType::Int(i32::from(n)))
                    .collect(),
            ),
            Self::PadOff => message(ADDR_VOICE_PAD_NOTES, vec![]),
            Self::ArpNote { note, gate_beats } => message(
                ADDR_VOICE_ARP_NOTE,
                vec![OscType::Int(i32::from(note)), OscType::Float(gate_beats)],
            ),
            Self::ArpOff => message(ADDR_VOICE_ARP_NOTE_OFF, vec![]),
            Self::LeadNote { note, gate_beats } => message(
                ADDR_VOICE_LEAD_NOTE,
                vec![OscType::Int(i32::from(note)), OscType::Float(gate_beats)],
            ),
            Self::LeadOff(note) => message(
                ADDR_VOICE_LEAD_NOTE_OFF,
                vec![OscType::Int(i32::from(note))],
            ),
            Self::BassNote { note, decay_beats } => message(
                ADDR_VOICE_BASS_NOTE,
                vec![OscType::Int(i32::from(note)), OscType::Float(decay_beats)],
            ),
            Self::BassOff => message(ADDR_VOICE_BASS_NOTE_OFF, vec![]),
        }
    }
}

/// Non-blocking sender held by the audio engine's pitched voices.
#[derive(Clone, Default)]
pub(crate) struct OscVoiceSink {
    sender: Option<SyncSender<OscVoiceEvent>>,
}

impl OscVoiceSink {
    pub(crate) fn channel() -> (Self, Receiver<OscVoiceEvent>) {
        let (sender, receiver) = mpsc::sync_channel(VOICE_CHANNEL_CAPACITY);
        (
            Self {
                sender: Some(sender),
            },
            receiver,
        )
    }

    #[cfg(test)]
    pub(crate) fn test_channel() -> (Self, Receiver<OscVoiceEvent>) {
        Self::channel()
    }

    pub(crate) fn is_attached(&self) -> bool {
        self.sender.is_some()
    }

    pub(crate) fn send(&self, event: OscVoiceEvent) {
        if let Some(sender) = &self.sender {
            let _ = sender.try_send(event);
        }
    }
}

// ============================================================
// Outbound UI/session state snapshot
// ============================================================

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct OscCursorState {
    pub(crate) tab: Tab,
    pub(crate) row_index: usize,
    pub(crate) browsing: bool,
    pub(crate) playing: bool,
    pub(crate) auto: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct OscLayerState {
    pub(crate) level_ratio: f32,
    pub(crate) muted: bool,
    pub(crate) planned_mute_beats: f32,
    pub(crate) first_module_label: String,
    pub(crate) first_module_ratio: f32,
    pub(crate) first_module_discrete: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct OscPageRow {
    pub(crate) index: usize,
    pub(crate) id: &'static str,
    pub(crate) label: String,
    pub(crate) ratio: f32,
    pub(crate) discrete: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OscPadWindowState {
    pub(crate) progression_index: usize,
    pub(crate) chord_count: usize,
    pub(crate) chord_offset: usize,
    pub(crate) active_slot: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OscStepsState {
    pub(crate) pad_steps: [u8; PAD_STEP_COUNT],
    pub(crate) lead_step_count: usize,
    pub(crate) lead_steps: [u8; LEAD_STEP_COUNT],
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct OscUiSnapshot {
    pub(crate) cursor: OscCursorState,
    pub(crate) layers: [OscLayerState; TAB_COUNT],
    pub(crate) page_rows: Vec<OscPageRow>,
    pub(crate) pad_window: OscPadWindowState,
    pub(crate) steps: OscStepsState,
}

pub(crate) type SharedOscUiState = Arc<ArcSwap<Option<OscUiSnapshot>>>;

pub(crate) fn new_shared_osc_ui_state() -> SharedOscUiState {
    Arc::new(ArcSwap::from_pointee(None))
}

fn layer_level_ratio(tab: Tab, controls: &FluidControls) -> f32 {
    let Some(spec) = tab.level_id().and_then(spec_by_id) else {
        return 0.0;
    };
    spec.ratio((spec.get)(controls), controls)
}

fn first_module_summary(tab: Tab, controls: &FluidControls) -> (String, f32, bool) {
    let Some(slots) = controls.modules.for_tab(tab) else {
        return (String::new(), 0.0, false);
    };
    for (slot_idx, slot) in slots.iter().enumerate() {
        if slot.kind().is_some()
            && let Some(id) = module_slot_collapsed_id(tab, slot_idx, controls)
            && let Some(spec) = spec_by_id(id)
        {
            let contextual = spec.contextual(controls);
            let value = (spec.get)(controls);
            let is_discrete = matches!(contextual.kind, ControlKind::Discrete);
            return (id.to_string(), contextual.scale().ratio(value), is_discrete);
        }
    }
    (String::new(), 0.0, false)
}

impl OscUiSnapshot {
    pub(crate) fn capture(
        model: &InteractionModel,
        session: &LiveSessionSnapshot,
        view: &UiViewModel,
        beat: f64,
        active_chord: u64,
        auto_active: bool,
    ) -> Self {
        let cursor = OscCursorState {
            tab: view.navigation.tab,
            row_index: view.navigation.selected,
            browsing: matches!(model.mode, InteractionMode::Browsing),
            playing: session.transport == Transport::Playing,
            auto: auto_active,
        };
        let layers = std::array::from_fn(|index| {
            let tab = Tab::all()[index];
            let planned_mute_beats = match session.planned {
                Some(PlannedAction::Mute {
                    tab: planned_tab,
                    target_beat,
                }) if planned_tab == tab => (target_beat - beat).max(0.0) as f32,
                _ => 0.0,
            };
            let (first_module_label, first_module_ratio, first_module_discrete) =
                first_module_summary(tab, &session.controls);
            OscLayerState {
                level_ratio: layer_level_ratio(tab, &session.controls),
                muted: session.muted[index],
                planned_mute_beats,
                first_module_label,
                first_module_ratio,
                first_module_discrete,
            }
        });
        let page_rows = view
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let (ratio, discrete) = spec_by_id(item.id).map_or((0.0, false), |spec| {
                    let contextual = spec.contextual(&session.controls);
                    let value = (spec.get)(&session.controls);
                    let is_discrete = matches!(contextual.kind, ControlKind::Discrete);
                    (contextual.scale().ratio(value), is_discrete)
                });
                OscPageRow {
                    index,
                    id: item.id,
                    label: item.label.clone(),
                    ratio,
                    discrete,
                }
            })
            .collect();
        let window = ChordWindow::requested(&session.controls.pad);
        let pad_window = OscPadWindowState {
            progression_index: progression_index(session.controls.pad.progression),
            chord_count: window.count,
            chord_offset: window.offset,
            active_slot: (active_chord as usize) % 8,
        };
        let pad_steps = std::array::from_fn(|i| u8::from(session.controls.pad.steps[i] >= 0.5));
        let lead_step_count = lead_live_step_count(session.controls.lead.step_count);
        let lead_steps = std::array::from_fn(|i| {
            lead_step_tone(session.controls.lead.steps[i]).unwrap_or(0) as u8
        });
        Self {
            cursor,
            layers,
            page_rows,
            pad_window,
            steps: OscStepsState {
                pad_steps,
                lead_step_count,
                lead_steps,
            },
        }
    }

    pub(crate) fn diff_messages(&self, previous: Option<&Self>) -> Vec<OscMessage> {
        let mut out = Vec::new();
        if previous.map(|p| &p.cursor) != Some(&self.cursor) {
            out.push(message(
                ADDR_STATE_CURSOR,
                vec![
                    OscType::String(layer_name(self.cursor.tab).to_string()),
                    OscType::Int(self.cursor.row_index as i32),
                    OscType::Int(i32::from(self.cursor.browsing)),
                    OscType::Int(i32::from(self.cursor.playing)),
                    OscType::Int(i32::from(self.cursor.auto)),
                ],
            ));
        }
        for (index, tab) in Tab::all().into_iter().enumerate() {
            let layer = &self.layers[index];
            if previous.map(|p| &p.layers[index]) != Some(layer) {
                out.push(message(
                    &state_layer_addr(tab),
                    vec![
                        OscType::Float(layer.level_ratio),
                        OscType::Int(i32::from(layer.muted)),
                        OscType::Float(layer.planned_mute_beats),
                        OscType::String(layer.first_module_label.clone()),
                        OscType::Float(layer.first_module_ratio),
                        OscType::Int(i32::from(layer.first_module_discrete)),
                    ],
                ));
            }
        }
        if previous.map(|p| &p.page_rows) != Some(&self.page_rows) {
            let mut args = Vec::with_capacity(self.page_rows.len() * 5);
            for row in &self.page_rows {
                args.push(OscType::Int(row.index as i32));
                args.push(OscType::String(row.id.to_string()));
                args.push(OscType::String(row.label.clone()));
                args.push(OscType::Float(row.ratio));
                args.push(OscType::Int(i32::from(row.discrete)));
            }
            out.push(message(ADDR_STATE_PAGE_ROWS, args));
        }
        if previous.map(|p| &p.pad_window) != Some(&self.pad_window) {
            out.push(message(
                ADDR_STATE_PAD_WINDOW,
                vec![
                    OscType::Int(self.pad_window.progression_index as i32),
                    OscType::Int(self.pad_window.chord_count as i32),
                    OscType::Int(self.pad_window.chord_offset as i32),
                    OscType::Int(self.pad_window.active_slot as i32),
                ],
            ));
        }
        if previous.map(|p| &p.steps) != Some(&self.steps) {
            let mut args = Vec::with_capacity(PAD_STEP_COUNT + 1 + LEAD_STEP_COUNT);
            for &step in &self.steps.pad_steps {
                args.push(OscType::Int(i32::from(step)));
            }
            args.push(OscType::Int(self.steps.lead_step_count as i32));
            for &step in &self.steps.lead_steps {
                args.push(OscType::Int(i32::from(step)));
            }
            out.push(message(ADDR_STATE_STEPS, args));
        }
        out
    }
}

// ============================================================
// Inbound OSC commands and receiver
// ============================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OscTransportCmd {
    Play,
    Stop,
    Toggle,
}

/// Semantic UI/control command received over OSC.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum OscUiCommand {
    ControlRatio { id: String, ratio: f32 },
    ControlValue { id: String, value: f32 },
    ControlDelta { id: String, steps: f32 },
    NavLayer(Tab),
    NavRow(usize),
    NavMove(i32),
    NavPage(i32),
    NavTouch,
    NavCancel,
    NavAdjust { steps: i32 },
    MuteToggle(Tab),
    MuteSet { tab: Tab, muted: bool },
    MutePlan(Tab),
    Gesture { kind: GestureKind, active: bool },
    GestureReleaseAll,
    Transport(OscTransportCmd),
    AutoToggle,
    MotionGrab(u8),
}

/// Low-latency voice command received over OSC and routed to the audio engine.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum OscVoiceCommand {
    PadNoteOn(u8),
    PadNoteOff(u8),
    PadNotes(Vec<u8>),
    ArpNoteOn(u8),
    ArpNoteOff(u8),
    ArpNotes(Vec<u8>),
    LeadNoteOn(u8),
    LeadNoteOff(u8),
    BassNoteOn(u8),
    KickTrigger(f32),
    AllNotesOff,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum OscInboundCommand {
    Ui(OscUiCommand),
    Voice(OscVoiceCommand),
}

/// Non-blocking receiver held by `FluidEngine` for inbound voice commands.
#[derive(Clone, Default)]
pub(crate) struct OscVoiceSource {
    receiver: Option<Arc<Mutex<Receiver<OscVoiceCommand>>>>,
}

impl OscVoiceSource {
    pub(crate) fn channel() -> (SyncSender<OscVoiceCommand>, Self) {
        let (sender, receiver) = mpsc::sync_channel(VOICE_CHANNEL_CAPACITY);
        (
            sender,
            Self {
                receiver: Some(Arc::new(Mutex::new(receiver))),
            },
        )
    }

    #[cfg(test)]
    pub(crate) fn test_channel() -> (Self, SyncSender<OscVoiceCommand>) {
        let (sender, source) = Self::channel();
        (source, sender)
    }

    pub(crate) fn try_recv(&self) -> Option<OscVoiceCommand> {
        self.receiver
            .as_ref()
            .and_then(|rx| rx.try_lock().ok()?.try_recv().ok())
    }
}

fn osc_f32(arg: &OscType) -> Option<f32> {
    match arg {
        OscType::Float(v) => Some(*v),
        OscType::Double(v) => Some(*v as f32),
        OscType::Int(v) => Some(*v as f32),
        OscType::Long(v) => Some(*v as f32),
        _ => None,
    }
}

fn osc_i32(arg: &OscType) -> Option<i32> {
    match arg {
        OscType::Int(v) => Some(*v),
        OscType::Long(v) => Some(*v as i32),
        OscType::Float(v) => Some(v.round() as i32),
        OscType::Double(v) => Some(v.round() as i32),
        _ => None,
    }
}

fn osc_str(arg: &OscType) -> Option<&str> {
    match arg {
        OscType::String(s) => Some(s.as_str()),
        _ => None,
    }
}

fn osc_u8_note(arg: &OscType) -> Option<u8> {
    osc_i32(arg)
        .and_then(|n| u8::try_from(n).ok())
        .filter(|&n| n < 128)
}

pub(crate) fn parse_inbound_message(msg: &OscMessage) -> Option<OscInboundCommand> {
    match msg.addr.as_str() {
        "/nooise/cmd/control/ratio" => {
            let id = osc_str(msg.args.first()?)?.to_string();
            let ratio = osc_f32(msg.args.get(1)?)?;
            Some(OscInboundCommand::Ui(OscUiCommand::ControlRatio {
                id,
                ratio,
            }))
        }
        "/nooise/cmd/control/value" => {
            let id = osc_str(msg.args.first()?)?.to_string();
            let value = osc_f32(msg.args.get(1)?)?;
            Some(OscInboundCommand::Ui(OscUiCommand::ControlValue {
                id,
                value,
            }))
        }
        "/nooise/cmd/control/delta" => {
            let id = osc_str(msg.args.first()?)?.to_string();
            let steps = osc_f32(msg.args.get(1)?)?;
            Some(OscInboundCommand::Ui(OscUiCommand::ControlDelta {
                id,
                steps,
            }))
        }
        "/nooise/cmd/nav/layer" => {
            let tab = parse_layer_name(osc_str(msg.args.first()?)?)?;
            Some(OscInboundCommand::Ui(OscUiCommand::NavLayer(tab)))
        }
        "/nooise/cmd/nav/row" => {
            let row = osc_i32(msg.args.first()?)?.max(0) as usize;
            Some(OscInboundCommand::Ui(OscUiCommand::NavRow(row)))
        }
        "/nooise/cmd/nav/move" => {
            let delta = osc_i32(msg.args.first()?)?;
            Some(OscInboundCommand::Ui(OscUiCommand::NavMove(delta)))
        }
        "/nooise/cmd/nav/page" => {
            let delta = osc_i32(msg.args.first()?)?;
            Some(OscInboundCommand::Ui(OscUiCommand::NavPage(delta)))
        }
        "/nooise/cmd/nav/touch" => Some(OscInboundCommand::Ui(OscUiCommand::NavTouch)),
        "/nooise/cmd/nav/cancel" => Some(OscInboundCommand::Ui(OscUiCommand::NavCancel)),
        "/nooise/cmd/nav/adjust" => {
            let steps = osc_i32(msg.args.first()?)?;
            Some(OscInboundCommand::Ui(OscUiCommand::NavAdjust { steps }))
        }
        "/nooise/cmd/mute/toggle" => {
            let tab = parse_layer_name(osc_str(msg.args.first()?)?)?;
            Some(OscInboundCommand::Ui(OscUiCommand::MuteToggle(tab)))
        }
        "/nooise/cmd/mute/set" => {
            let tab = parse_layer_name(osc_str(msg.args.first()?)?)?;
            let muted = osc_i32(msg.args.get(1)?)? != 0;
            Some(OscInboundCommand::Ui(OscUiCommand::MuteSet { tab, muted }))
        }
        "/nooise/cmd/mute/plan" => {
            let tab = parse_layer_name(osc_str(msg.args.first()?)?)?;
            Some(OscInboundCommand::Ui(OscUiCommand::MutePlan(tab)))
        }
        "/nooise/cmd/gesture" => {
            let kind = parse_gesture_name(osc_str(msg.args.first()?)?)?;
            let active = osc_i32(msg.args.get(1)?)? != 0;
            Some(OscInboundCommand::Ui(OscUiCommand::Gesture {
                kind,
                active,
            }))
        }
        "/nooise/cmd/gesture/release_all" => {
            Some(OscInboundCommand::Ui(OscUiCommand::GestureReleaseAll))
        }
        "/nooise/cmd/transport" => {
            let cmd = match osc_str(msg.args.first()?)?.trim() {
                s if s.eq_ignore_ascii_case("play") || s.eq_ignore_ascii_case("start") => {
                    OscTransportCmd::Play
                }
                s if s.eq_ignore_ascii_case("stop") => OscTransportCmd::Stop,
                s if s.eq_ignore_ascii_case("toggle") => OscTransportCmd::Toggle,
                _ => return None,
            };
            Some(OscInboundCommand::Ui(OscUiCommand::Transport(cmd)))
        }
        "/nooise/cmd/auto/toggle" => Some(OscInboundCommand::Ui(OscUiCommand::AutoToggle)),
        "/nooise/cmd/motion/grab" => {
            let raw = osc_i32(msg.args.first()?)?;
            let length_beats = match raw {
                1 | 4 => 4,
                2 | 8 => 8,
                16 => 16,
                32 => 32,
                _ => return None,
            };
            Some(OscInboundCommand::Ui(OscUiCommand::MotionGrab(
                length_beats,
            )))
        }
        "/nooise/cmd/voice/pad/note_on" => {
            let note = osc_u8_note(msg.args.first()?)?;
            Some(OscInboundCommand::Voice(OscVoiceCommand::PadNoteOn(note)))
        }
        "/nooise/cmd/voice/pad/note_off" => {
            let note = osc_u8_note(msg.args.first()?)?;
            Some(OscInboundCommand::Voice(OscVoiceCommand::PadNoteOff(note)))
        }
        "/nooise/cmd/voice/pad/notes" => {
            let notes: Vec<u8> = msg.args.iter().filter_map(osc_u8_note).collect();
            Some(OscInboundCommand::Voice(OscVoiceCommand::PadNotes(notes)))
        }
        "/nooise/cmd/voice/arp/note_on" => {
            let note = osc_u8_note(msg.args.first()?)?;
            Some(OscInboundCommand::Voice(OscVoiceCommand::ArpNoteOn(note)))
        }
        "/nooise/cmd/voice/arp/note_off" => {
            let note = osc_u8_note(msg.args.first()?)?;
            Some(OscInboundCommand::Voice(OscVoiceCommand::ArpNoteOff(note)))
        }
        "/nooise/cmd/voice/arp/notes" => {
            let notes: Vec<u8> = msg.args.iter().filter_map(osc_u8_note).collect();
            Some(OscInboundCommand::Voice(OscVoiceCommand::ArpNotes(notes)))
        }
        "/nooise/cmd/voice/lead/note_on" => {
            let note = osc_u8_note(msg.args.first()?)?;
            Some(OscInboundCommand::Voice(OscVoiceCommand::LeadNoteOn(note)))
        }
        "/nooise/cmd/voice/lead/note_off" => {
            let note = osc_u8_note(msg.args.first()?)?;
            Some(OscInboundCommand::Voice(OscVoiceCommand::LeadNoteOff(note)))
        }
        "/nooise/cmd/voice/bass/note_on" => {
            let note = osc_u8_note(msg.args.first()?)?;
            Some(OscInboundCommand::Voice(OscVoiceCommand::BassNoteOn(note)))
        }
        "/nooise/cmd/voice/kick/trigger" => {
            let velocity = msg
                .args
                .first()
                .and_then(osc_f32)
                .unwrap_or(1.0)
                .clamp(0.0, 2.0);
            Some(OscInboundCommand::Voice(OscVoiceCommand::KickTrigger(
                velocity,
            )))
        }
        "/nooise/cmd/voice/all_notes_off" => {
            Some(OscInboundCommand::Voice(OscVoiceCommand::AllNotesOff))
        }
        _ => None,
    }
}

pub(crate) fn parse_inbound_packet(packet: OscPacket, out: &mut Vec<OscInboundCommand>) {
    match packet {
        OscPacket::Message(msg) => {
            if let Some(cmd) = parse_inbound_message(&msg) {
                out.push(cmd);
            }
        }
        OscPacket::Bundle(bundle) => {
            for item in bundle.content {
                parse_inbound_packet(item, out);
            }
        }
    }
}

/// Owns the inbound UDP listener thread and the UI command receiver.
pub(crate) struct OscReceiver {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    ui_rx: Receiver<OscUiCommand>,
    voice_source: OscVoiceSource,
}

impl OscReceiver {
    pub(crate) fn spawn(listen: SocketAddr) -> io::Result<Self> {
        let socket = UdpSocket::bind(listen)
            .map_err(|e| io::Error::new(e.kind(), format!("osc-in: bind {listen} failed: {e}")))?;
        socket.set_read_timeout(Some(Duration::from_millis(20)))?;
        println!("listening for OSC commands on {listen}");
        let (ui_tx, ui_rx) = mpsc::sync_channel(UI_CMD_CHANNEL_CAPACITY);
        let (voice_tx, voice_source) = OscVoiceSource::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = Arc::clone(&stop);
        let handle = thread::Builder::new()
            .name("nooise-osc-in".into())
            .spawn(move || recv_loop(socket, ui_tx, voice_tx, &stop_for_thread))?;
        Ok(Self {
            stop,
            handle: Some(handle),
            ui_rx,
            voice_source,
        })
    }

    pub(crate) fn voice_source(&self) -> OscVoiceSource {
        self.voice_source.clone()
    }

    pub(crate) fn drain_ui(&self) -> Vec<OscUiCommand> {
        self.ui_rx.try_iter().collect()
    }
}

impl Drop for OscReceiver {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn recv_loop(
    socket: UdpSocket,
    ui_tx: SyncSender<OscUiCommand>,
    voice_tx: SyncSender<OscVoiceCommand>,
    stop: &AtomicBool,
) {
    let mut buf = [0u8; 4096];
    let mut parsed = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        let Ok((len, _)) = socket.recv_from(&mut buf) else {
            continue;
        };
        let Ok((_, packet)) = decoder::decode_udp(&buf[..len]) else {
            continue;
        };
        parsed.clear();
        parse_inbound_packet(packet, &mut parsed);
        for cmd in parsed.drain(..) {
            match cmd {
                OscInboundCommand::Ui(ui) => {
                    let _ = ui_tx.try_send(ui);
                }
                OscInboundCommand::Voice(voice) => {
                    let _ = voice_tx.try_send(voice);
                }
            }
        }
    }
}

// ============================================================
// Outbound OSC emitter
// ============================================================

/// Owns the emitter thread; dropping it stops the feed.
pub(crate) struct OscEmitter {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl OscEmitter {
    #[cfg(test)]
    pub(crate) fn spawn(target: SocketAddr, telemetry: Arc<FluidTelemetry>) -> io::Result<Self> {
        Self::spawn_full(target, telemetry, None, None)
    }

    pub(crate) fn spawn_full(
        target: SocketAddr,
        telemetry: Arc<FluidTelemetry>,
        ui_state: Option<SharedOscUiState>,
        voice_rx: Option<Receiver<OscVoiceEvent>>,
    ) -> io::Result<Self> {
        let bind: SocketAddr = if target.is_ipv4() {
            "0.0.0.0:0".parse().expect("static v4 bind address")
        } else {
            "[::]:0".parse().expect("static v6 bind address")
        };
        let socket = UdpSocket::bind(bind)
            .map_err(|e| io::Error::new(e.kind(), format!("osc: bind {bind} failed: {e}")))?;
        println!("mirroring telemetry as OSC to {target}");
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = Arc::clone(&stop);
        // Baseline on the caller's thread so every change after `spawn`
        // returns is mirrored, even ones that land before the thread starts.
        let mirrored = Mirrored::from(&telemetry);
        let handle = thread::Builder::new()
            .name("nooise-osc".into())
            .spawn(move || {
                emit_loop(
                    &socket,
                    target,
                    &telemetry,
                    ui_state.as_ref(),
                    voice_rx.as_ref(),
                    mirrored,
                    &stop_for_thread,
                )
            })?;
        Ok(Self {
            stop,
            handle: Some(handle),
        })
    }
}

impl Drop for OscEmitter {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Telemetry as last mirrored, so each poll sends only what changed.
struct Mirrored {
    kick: u64,
    kick_level: f32,
    chord_pulse: u64,
    chord_slot: u64,
    chord_attack: f32,
    chord_release: f32,
    chord_root_pitch_class: f32,
    phrase_pulse: u64,
    phrase_index: u64,
    phrase_chord_index: u64,
    phrase_chord_count: u64,
    phrase_reset: bool,
    chord_span: ChordSpan,
    beat_bits: u64,
    level_bits: [u32; TAB_COUNT],
    gesture_bits: [u32; GESTURE_COUNT],
    hit_pulses: [u64; MUSICAL_HIT_COUNT],
    hit_level_bits: [u32; MUSICAL_HIT_COUNT],
    hit_pitch_class_bits: [u32; MUSICAL_HIT_COUNT],
}

impl Mirrored {
    fn from(telemetry: &FluidTelemetry) -> Self {
        // Acquire pairs with the Release in `publish_kick`/`publish_chord`:
        // once the counter is seen, the values stored before it are too.
        let kick = telemetry.kick_pulse.load(Ordering::Acquire);
        // Pad publishes its chord before its phrase. Read the phrase counter
        // first so a new phrase cannot be mirrored ahead of that chord.
        let phrase_pulse = telemetry.phrase_pulse.load(Ordering::Acquire);
        let chord_pulse = telemetry.chord_pulse.load(Ordering::Acquire);
        Self {
            kick,
            kick_level: f32::from_bits(telemetry.kick_level_bits.load(Ordering::Relaxed)),
            chord_pulse,
            chord_slot: telemetry.chord_slot.load(Ordering::Relaxed),
            chord_attack: f32::from_bits(telemetry.chord_attack_bits.load(Ordering::Relaxed)),
            chord_release: f32::from_bits(telemetry.chord_release_bits.load(Ordering::Relaxed)),
            chord_root_pitch_class: f32::from_bits(
                telemetry
                    .chord_root_pitch_class_bits
                    .load(Ordering::Relaxed),
            ),
            phrase_pulse,
            phrase_index: telemetry.phrase_index.load(Ordering::Relaxed),
            phrase_chord_index: telemetry.phrase_chord_index.load(Ordering::Relaxed),
            phrase_chord_count: telemetry.phrase_chord_count.load(Ordering::Relaxed),
            phrase_reset: telemetry.phrase_reset.load(Ordering::Relaxed),
            chord_span: telemetry.chord_span(),
            beat_bits: telemetry.beat_bits.load(Ordering::Relaxed),
            level_bits: std::array::from_fn(|i| telemetry.level_bits[i].load(Ordering::Relaxed)),
            gesture_bits: std::array::from_fn(|i| {
                telemetry.gesture_bits[i].load(Ordering::Relaxed)
            }),
            hit_pulses: std::array::from_fn(|i| telemetry.hits[i].pulse.load(Ordering::Acquire)),
            hit_level_bits: std::array::from_fn(|i| {
                telemetry.hits[i].level_bits.load(Ordering::Relaxed)
            }),
            hit_pitch_class_bits: std::array::from_fn(|i| {
                telemetry.hits[i].pitch_class_bits.load(Ordering::Relaxed)
            }),
        }
    }

    /// Messages describing the change from `self` to the current telemetry,
    /// then advance `self` to it.
    fn diff(&mut self, telemetry: &FluidTelemetry) -> Vec<OscMessage> {
        let now = Mirrored::from(telemetry);
        let mut out = Vec::new();
        if now.beat_bits != self.beat_bits {
            out.push(message(
                ADDR_BEAT,
                vec![OscType::Float(f64::from_bits(now.beat_bits) as f32)],
            ));
        }
        for (tab, (new, old)) in Tab::all()
            .into_iter()
            .zip(now.level_bits.iter().zip(&self.level_bits))
        {
            if new != old {
                out.push(message(
                    &level_addr(tab),
                    vec![OscType::Float(f32::from_bits(*new))],
                ));
            }
        }
        for (kind, (new, old)) in GestureKind::ALL
            .into_iter()
            .zip(now.gesture_bits.iter().zip(&self.gesture_bits))
        {
            if new != old {
                out.push(message(
                    &gesture_addr(kind),
                    vec![OscType::Float(f32::from_bits(*new))],
                ));
            }
        }
        if now.chord_span != self.chord_span {
            out.push(message(
                ADDR_CHORD_SPAN,
                vec![
                    OscType::Float(now.chord_span.start_beat as f32),
                    OscType::Float(now.chord_span.length_beats),
                ],
            ));
        }
        if now.chord_pulse != self.chord_pulse {
            out.push(message(
                ADDR_CHORD,
                vec![
                    OscType::Int(now.chord_slot as i32),
                    OscType::Float(now.chord_attack),
                    OscType::Float(now.chord_release),
                ],
            ));
            out.push(message(
                ADDR_CHORD_CHANGE,
                vec![
                    OscType::Int(now.chord_slot as i32),
                    OscType::Float(now.chord_root_pitch_class),
                ],
            ));
        }
        if now.phrase_pulse != self.phrase_pulse {
            if now.phrase_reset {
                out.push(message(ADDR_PHRASE_RESET, vec![]));
            }
            out.push(message(
                ADDR_PHRASE,
                vec![
                    OscType::Int(now.phrase_index.min(i32::MAX as u64) as i32),
                    OscType::Int(now.phrase_chord_index as i32),
                    OscType::Int(now.phrase_chord_count as i32),
                ],
            ));
        }
        for _ in self.kick..now.kick {
            out.push(message(ADDR_KICK, vec![OscType::Float(now.kick_level)]));
        }
        for hit in MusicalHit::ALL {
            let index = hit as usize;
            for _ in self.hit_pulses[index]..now.hit_pulses[index] {
                out.push(message(
                    &hit_addr(hit),
                    vec![
                        OscType::Float(f32::from_bits(now.hit_level_bits[index])),
                        OscType::Float(f32::from_bits(now.hit_pitch_class_bits[index])),
                    ],
                ));
            }
        }
        *self = now;
        out
    }
}

fn message(addr: &str, args: Vec<OscType>) -> OscMessage {
    OscMessage {
        addr: addr.to_string(),
        args,
    }
}

fn emit_loop(
    socket: &UdpSocket,
    target: SocketAddr,
    telemetry: &FluidTelemetry,
    ui_state: Option<&SharedOscUiState>,
    voice_rx: Option<&Receiver<OscVoiceEvent>>,
    mut mirrored: Mirrored,
    stop: &AtomicBool,
) {
    let mut last_ui: Option<OscUiSnapshot> = None;
    let mut last_full_ui = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        if last_full_ui.elapsed() >= Duration::from_millis(500) {
            last_ui = None;
            last_full_ui = Instant::now();
        }
        let mut messages = mirrored.diff(telemetry);
        if let Some(rx) = voice_rx {
            for event in rx.try_iter() {
                messages.push(event.into_message());
            }
        }
        if let Some(shared) = ui_state
            && let Some(current) = shared.load().as_ref()
        {
            let state_msgs = current.diff_messages(last_ui.as_ref());
            if !state_msgs.is_empty() {
                messages.extend(state_msgs);
                last_ui = Some(current.clone());
            }
        }
        if !messages.is_empty() {
            let bundle = OscPacket::Bundle(OscBundle {
                timetag: OscTime::from((0, 1)),
                content: messages.into_iter().map(OscPacket::Message).collect(),
            });
            if let Ok(bytes) = encoder::encode(&bundle) {
                // Fire-and-forget: no consumer is a normal state, not an error.
                let _ = socket.send_to(&bytes, target);
            }
        }
        thread::sleep(POLL_INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_osc_layer_reports_its_registered_level() {
        let mut controls = FluidControls::default();
        for tab in Tab::all() {
            let spec = spec_by_id(tab.level_id().unwrap()).unwrap();
            (spec.set)(&mut controls, spec.max);
            assert_eq!(layer_level_ratio(tab, &controls), 1.0, "{tab:?}");
            (spec.set)(&mut controls, spec.min);
            assert_eq!(layer_level_ratio(tab, &controls), 0.0, "{tab:?}");
        }
    }

    use super::*;
    use crate::fluid::MasterKey;

    fn messages_in(packet: OscPacket) -> Vec<OscMessage> {
        match packet {
            OscPacket::Message(m) => vec![m],
            OscPacket::Bundle(b) => b.content.into_iter().flat_map(messages_in).collect(),
        }
    }

    #[test]
    fn diff_sends_only_changes_and_one_message_per_kick() {
        let telemetry = FluidTelemetry::default();
        let mut mirrored = Mirrored::from(&telemetry);
        assert!(mirrored.diff(&telemetry).is_empty());

        telemetry.publish_kick(0.25);
        telemetry.publish_kick(0.5);
        telemetry.publish_kick(0.75);
        telemetry.publish_chord_span(ChordSpan {
            start_beat: 8.0,
            length_beats: 16.0,
        });
        telemetry.publish_chord(2, 0.25, 6.0, 8.0);
        telemetry.publish_beat(4.5);
        telemetry.publish_level(Tab::Bass, 0.05);
        telemetry.publish_level(Tab::Master, 0.1);
        telemetry.publish_gesture(GestureKind::Echo, 0.6);
        let out = mirrored.diff(&telemetry);
        let addrs: Vec<&str> = out.iter().map(|m| m.addr.as_str()).collect();
        assert_eq!(
            addrs,
            [
                ADDR_BEAT,
                "/nooise/voice/bass/level",
                ADDR_LEVEL,
                "/nooise/gesture/echo",
                ADDR_CHORD_SPAN,
                ADDR_CHORD,
                ADDR_CHORD_CHANGE,
                ADDR_KICK,
                ADDR_KICK,
                ADDR_KICK,
                "/nooise/hit/kick",
                "/nooise/hit/kick",
                "/nooise/hit/kick"
            ]
        );
        assert_eq!(out[0].args, vec![OscType::Float(4.5)]);
        assert_eq!(out[1].args, vec![OscType::Float(0.05)]);
        assert_eq!(out[2].args, vec![OscType::Float(0.1)]);
        assert_eq!(out[3].args, vec![OscType::Float(0.6)]);
        assert_eq!(out[4].args, vec![OscType::Float(8.0), OscType::Float(16.0)]);
        assert_eq!(
            out[5].args,
            vec![OscType::Int(2), OscType::Float(6.0), OscType::Float(8.0)]
        );
        assert_eq!(out[6].args, vec![OscType::Int(2), OscType::Float(0.25)]);
        // Hits inside one poll share the latest level.
        assert_eq!(out[7].args, vec![OscType::Float(0.75)]);
        assert_eq!(
            out[10].args,
            vec![OscType::Float(0.75), OscType::Float(-1.0)]
        );
        assert!(mirrored.diff(&telemetry).is_empty());
    }

    #[test]
    fn pad_cursor_phrase_events_follow_window_restart_and_morph_landing() {
        use crate::fluid::PadEngine;
        use crate::fluid::controls::PadControls;
        use crate::fluid::engine::TimingContext;

        let controls = PadControls {
            chord_bars: 0.25,
            chord_count: 2.0,
            chord_offset: 3.0,
            ..PadControls::default()
        };
        let telemetry = Arc::new(FluidTelemetry::default());
        let mut pad = PadEngine::new(48_000.0, &controls, MasterKey::Home, Arc::clone(&telemetry));
        let mut mirrored = Mirrored::from(&telemetry);
        fn tick(
            pad: &mut PadEngine,
            controls: &PadControls,
            telemetry: &FluidTelemetry,
            mirrored: &mut Mirrored,
            beat: f64,
            morph_start: Option<f64>,
        ) -> Vec<OscMessage> {
            let mut timing = TimingContext::new(48_000.0, 120.0, beat);
            timing.morph_phrase_start = morph_start;
            pad.next(controls, MasterKey::Home, timing);
            mirrored.diff(telemetry)
        }
        let tick_at = |pad: &mut PadEngine, mirrored: &mut Mirrored, beat, morph_start| {
            tick(pad, &controls, &telemetry, mirrored, beat, morph_start)
        };
        let start = tick_at(&mut pad, &mut mirrored, 0.0, None);
        assert_eq!(
            start.iter().map(|m| m.addr.as_str()).collect::<Vec<_>>(),
            [
                ADDR_CHORD_SPAN,
                ADDR_CHORD,
                ADDR_CHORD_CHANGE,
                ADDR_PHRASE_RESET,
                ADDR_PHRASE
            ]
        );
        assert_eq!(
            start[4].args,
            vec![OscType::Int(0), OscType::Int(0), OscType::Int(2)]
        );
        assert!(tick_at(&mut pad, &mut mirrored, 0.5, None).is_empty());
        let second = tick_at(&mut pad, &mut mirrored, 1.0, None);
        assert_eq!(
            second.iter().find(|m| m.addr == ADDR_PHRASE).unwrap().args,
            vec![OscType::Int(0), OscType::Int(1), OscType::Int(2)]
        );
        let wrapped = tick_at(&mut pad, &mut mirrored, 2.0, None);
        assert_eq!(
            wrapped.iter().find(|m| m.addr == ADDR_PHRASE).unwrap().args,
            vec![OscType::Int(1), OscType::Int(0), OscType::Int(2)]
        );
        assert!(!wrapped.iter().any(|m| m.addr == ADDR_PHRASE_RESET));

        let mut stopped = TimingContext::new(48_000.0, 120.0, 2.0);
        stopped.transport = crate::fluid::engine::Transport::Stopped;
        pad.next(&controls, MasterKey::Home, stopped);
        assert!(mirrored.diff(&telemetry).is_empty());
        pad.restart_sequence(&controls);
        let restart = tick_at(&mut pad, &mut mirrored, 0.0, None);
        assert!(restart.iter().any(|m| m.addr == ADDR_CHORD));
        assert_eq!(
            restart
                .iter()
                .filter(|m| m.addr == ADDR_PHRASE_RESET)
                .count(),
            1
        );
        assert_eq!(
            restart.iter().find(|m| m.addr == ADDR_PHRASE).unwrap().args,
            vec![OscType::Int(0), OscType::Int(0), OscType::Int(2)]
        );

        let landed = tick_at(&mut pad, &mut mirrored, 4.0, Some(4.0));
        assert_eq!(
            landed
                .iter()
                .filter(|m| m.addr == ADDR_PHRASE_RESET)
                .count(),
            1
        );
        assert_eq!(
            landed.iter().find(|m| m.addr == ADDR_PHRASE).unwrap().args,
            vec![OscType::Int(0), OscType::Int(0), OscType::Int(2)]
        );

        let mut new_progression = controls.clone();
        new_progression.progression += 1.0;
        let switched = tick(
            &mut pad,
            &new_progression,
            &telemetry,
            &mut mirrored,
            5.0,
            None,
        );
        assert_eq!(
            switched
                .iter()
                .filter(|m| m.addr == ADDR_PHRASE_RESET)
                .count(),
            1
        );
        assert_eq!(
            switched
                .iter()
                .find(|m| m.addr == ADDR_PHRASE)
                .unwrap()
                .args,
            vec![OscType::Int(0), OscType::Int(0), OscType::Int(2)]
        );
    }

    #[test]
    fn phrase_reset_and_position_arrive_in_order_over_udp() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let telemetry = Arc::new(FluidTelemetry::default());
        let emitter =
            OscEmitter::spawn(receiver.local_addr().unwrap(), Arc::clone(&telemetry)).unwrap();
        telemetry.publish_chord(3, 0.0, 0.1, 1.0);
        telemetry.publish_phrase(0, 0, 2, true);
        let mut buf = [0u8; 4096];
        let mut messages = Vec::new();
        while !messages.iter().any(|m: &OscMessage| m.addr == ADDR_PHRASE) {
            let (len, _) = receiver.recv_from(&mut buf).unwrap();
            let (_, packet) = decoder::decode_udp(&buf[..len]).unwrap();
            messages.extend(messages_in(packet));
        }
        assert_eq!(
            messages.iter().map(|m| m.addr.as_str()).collect::<Vec<_>>(),
            [
                ADDR_CHORD,
                ADDR_CHORD_CHANGE,
                ADDR_PHRASE_RESET,
                ADDR_PHRASE
            ]
        );
        assert_eq!(
            messages[3].args,
            vec![OscType::Int(0), OscType::Int(0), OscType::Int(2)]
        );
        drop(emitter);
    }

    #[test]
    fn hit_addresses_and_pitch_classes_cover_every_musical_voice() {
        let telemetry = FluidTelemetry::default();
        let mut mirrored = Mirrored::from(&telemetry);
        for (hit, level, pitch_class) in [
            (MusicalHit::Beat, 1.0, -1.0),
            (MusicalHit::Pad, 0.1, 0.0),
            (MusicalHit::Perc, 0.2, -1.0),
            (MusicalHit::Kick, 0.3, -1.0),
            (MusicalHit::Tonal, 0.4, 0.25),
            (MusicalHit::Clap, 0.5, -1.0),
            (MusicalHit::Arp, 0.6, 11.0 / 12.0),
        ] {
            telemetry.publish_hit(hit, level, pitch_class);
        }

        let messages = mirrored.diff(&telemetry);
        let addresses: Vec<&str> = messages
            .iter()
            .map(|message| message.addr.as_str())
            .collect();
        assert_eq!(
            addresses,
            [
                "/nooise/hit/beat",
                "/nooise/hit/pad",
                "/nooise/hit/perc",
                "/nooise/hit/kick",
                "/nooise/hit/tonal",
                "/nooise/hit/clap",
                "/nooise/hit/arp",
            ]
        );
        assert_eq!(
            messages[4].args,
            vec![OscType::Float(0.4), OscType::Float(0.25)]
        );
    }

    /// External visualizers match these strings; a new gesture extends the
    /// list, and a retired one leaves it without its name being reused.
    #[test]
    fn gesture_addresses_are_the_published_contract() {
        let addrs: Vec<String> = GestureKind::ALL.into_iter().map(gesture_addr).collect();
        assert_eq!(
            addrs,
            [
                "/nooise/gesture/bloom",
                "/nooise/gesture/submerge",
                "/nooise/gesture/echo",
                "/nooise/gesture/lift",
            ]
        );
    }

    #[test]
    fn emitter_delivers_kick_over_udp() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let telemetry = Arc::new(FluidTelemetry::default());
        let emitter =
            OscEmitter::spawn(receiver.local_addr().unwrap(), Arc::clone(&telemetry)).unwrap();

        telemetry.publish_kick(0.4);
        let mut buf = [0u8; 1024];
        let (len, _) = receiver.recv_from(&mut buf).unwrap();
        let (_, packet) = decoder::decode_udp(&buf[..len]).unwrap();
        let addrs: Vec<String> = messages_in(packet).into_iter().map(|m| m.addr).collect();
        assert_eq!(addrs, [ADDR_KICK, "/nooise/hit/kick"]);
        drop(emitter);
    }

    #[test]
    fn inbound_osc_messages_parse_semantic_ui_and_voice_commands() {
        let cases = [
            (
                message(
                    "/nooise/cmd/control/ratio",
                    vec![OscType::String("master.bpm".into()), OscType::Float(0.5)],
                ),
                Some(OscInboundCommand::Ui(OscUiCommand::ControlRatio {
                    id: "master.bpm".into(),
                    ratio: 0.5,
                })),
            ),
            (
                message("/nooise/cmd/nav/layer", vec![OscType::String("pad".into())]),
                Some(OscInboundCommand::Ui(OscUiCommand::NavLayer(Tab::Chords))),
            ),
            (
                message(
                    "/nooise/cmd/mute/set",
                    vec![OscType::String("kick".into()), OscType::Int(1)],
                ),
                Some(OscInboundCommand::Ui(OscUiCommand::MuteSet {
                    tab: Tab::Kick,
                    muted: true,
                })),
            ),
            (
                message(
                    "/nooise/cmd/gesture",
                    vec![OscType::String("bloom".into()), OscType::Int(1)],
                ),
                Some(OscInboundCommand::Ui(OscUiCommand::Gesture {
                    kind: GestureKind::Bloom,
                    active: true,
                })),
            ),
            (
                message("/nooise/cmd/motion/grab", vec![OscType::Int(4)]),
                Some(OscInboundCommand::Ui(OscUiCommand::MotionGrab(4))),
            ),
            (
                message(
                    "/nooise/cmd/voice/pad/notes",
                    vec![OscType::Int(48), OscType::Int(52), OscType::Int(55)],
                ),
                Some(OscInboundCommand::Voice(OscVoiceCommand::PadNotes(vec![
                    48, 52, 55,
                ]))),
            ),
            (
                message("/nooise/cmd/voice/lead/note_on", vec![OscType::Int(60)]),
                Some(OscInboundCommand::Voice(OscVoiceCommand::LeadNoteOn(60))),
            ),
            (
                message("/nooise/cmd/voice/kick/trigger", vec![OscType::Float(0.8)]),
                Some(OscInboundCommand::Voice(OscVoiceCommand::KickTrigger(0.8))),
            ),
        ];
        for (msg, expected) in cases {
            assert_eq!(parse_inbound_message(&msg), expected);
        }
    }

    #[test]
    fn voice_events_encode_to_published_osc_addresses() {
        let chord = pad_notes([48, 52, 55, 60], 0.0);
        assert_eq!(
            OscVoiceEvent::PadChord(chord).into_message(),
            message(
                ADDR_VOICE_PAD_NOTES,
                vec![
                    OscType::Int(48),
                    OscType::Int(52),
                    OscType::Int(55),
                    OscType::Int(60)
                ]
            )
        );
        assert_eq!(
            OscVoiceEvent::PadOff.into_message(),
            message(ADDR_VOICE_PAD_NOTES, vec![])
        );
        assert_eq!(
            OscVoiceEvent::ArpNote {
                note: 64,
                gate_beats: 0.5
            }
            .into_message(),
            message(
                ADDR_VOICE_ARP_NOTE,
                vec![OscType::Int(64), OscType::Float(0.5)]
            )
        );
        assert_eq!(
            OscVoiceEvent::LeadOff(67).into_message(),
            message(ADDR_VOICE_LEAD_NOTE_OFF, vec![OscType::Int(67)])
        );
    }

    /// Level profile of a built-in song, for calibrating consumer
    /// sensitivity from real material rather than guesses:
    /// `NOOISE_SONG=12 cargo test --release song_level_profile -- --ignored --nocapture`
    #[test]
    #[ignore = "prints a calibration table; run on demand"]
    fn song_level_profile() {
        use crate::audio::StereoEngine;
        use crate::fluid::auto::decode_auto_states;
        use crate::fluid::{FluidEngine, LEVEL_BLOCK, LiveSession, LiveSessionSnapshot, no_morph};

        let env_or = |key: &str, default: u64| {
            std::env::var(key)
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default)
        };
        let number = env_or("NOOISE_SONG", 12) as usize;
        let bars = env_or("NOOISE_BARS", 32);
        let song = decode_auto_states()
            .into_iter()
            .nth(number - 1)
            .unwrap_or_else(|| panic!("no built-in song {number}"));
        let sample_rate = 48_000.0;
        let bpm = song.controls.master.bpm as f64;
        let frames = (bars as f64 * 4.0 * 60.0 / bpm * sample_rate as f64) as u64;
        let telemetry = Arc::new(FluidTelemetry::default());
        let session = LiveSession::new(LiveSessionSnapshot::from_song(&song));
        let mut engine = FluidEngine::new(sample_rate, session, no_morph(), Arc::clone(&telemetry));
        let mut blocks: Vec<Vec<f32>> = vec![Vec::new(); TAB_COUNT];
        for frame in 1..=frames {
            engine.next_stereo();
            if frame % LEVEL_BLOCK == 0 {
                for (tab, series) in Tab::all().into_iter().zip(&mut blocks) {
                    series.push(telemetry.level(tab));
                }
            }
        }
        println!(
            "song {number}, {bars} bars at {bpm} bpm, {} blocks per voice",
            blocks[0].len()
        );
        println!(
            "{:<8} {:>8} {:>8} {:>8} {:>8} {:>8}",
            "voice", "p50", "p90", "p99", "max", "active%"
        );
        for (tab, series) in Tab::all().into_iter().zip(&mut blocks) {
            series.sort_by(f32::total_cmp);
            let at = |q: f64| series[((series.len() - 1) as f64 * q) as usize];
            let active = series.iter().filter(|v| **v > 1e-4).count() as f64 / series.len() as f64;
            let name = match tab {
                Tab::Master => "master",
                voice => VOICES[voice as usize],
            };
            println!(
                "{name:<8} {:>8.4} {:>8.4} {:>8.4} {:>8.4} {:>7.0}%",
                at(0.5),
                at(0.9),
                at(0.99),
                series[series.len() - 1],
                active * 100.0
            );
        }
    }

    /// The whole path a visualizer depends on: engine renders, telemetry
    /// moves, the emitter mirrors it, and beat, level, and kick messages land on UDP.
    #[test]
    fn headless_engine_reaches_a_udp_listener() {
        use crate::audio::StereoEngine;
        use crate::fluid::{
            FluidControls, FluidEngine, LiveSession, LiveSessionSnapshot, SongState, no_morph,
        };
        use std::collections::BTreeMap;

        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver
            .set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        let telemetry = Arc::new(FluidTelemetry::default());
        let emitter =
            OscEmitter::spawn(receiver.local_addr().unwrap(), Arc::clone(&telemetry)).unwrap();

        let song = SongState::from_controls(FluidControls::default());
        let session = LiveSession::new(LiveSessionSnapshot::from_song(&song));
        let mut engine = FluidEngine::new(48_000.0, session, no_morph(), telemetry);
        let mut seen: BTreeMap<String, usize> = BTreeMap::new();
        let mut span_args = Vec::new();
        let mut buf = [0u8; 4096];
        // Render in slices so the emitter's poll cadence interleaves with
        // playback the way it does live; stop as soon as both arrive.
        for _ in 0..200 {
            for _ in 0..4_800 {
                engine.next_stereo();
            }
            while let Ok((len, _)) = receiver.recv_from(&mut buf) {
                let (_, packet) = decoder::decode_udp(&buf[..len]).unwrap();
                for m in messages_in(packet) {
                    if m.addr == ADDR_CHORD_SPAN {
                        span_args = m.args.clone();
                    }
                    *seen.entry(m.addr).or_default() += 1;
                }
                receiver
                    .set_read_timeout(Some(Duration::from_millis(1)))
                    .unwrap();
            }
            if [
                ADDR_BEAT,
                ADDR_LEVEL,
                ADDR_KICK,
                ADDR_PHRASE,
                ADDR_CHORD_SPAN,
            ]
            .iter()
            .all(|a| seen.contains_key(*a))
            {
                break;
            }
        }
        drop(emitter);
        for addr in [
            ADDR_BEAT,
            ADDR_LEVEL,
            ADDR_KICK,
            ADDR_PHRASE,
            ADDR_CHORD_SPAN,
        ] {
            assert!(
                seen.get(addr).copied().unwrap_or(0) > 0,
                "no {addr}: {seen:?}"
            );
        }
        // Default chord length: 4 bars of 4 beats.
        assert_eq!(span_args[1], OscType::Float(16.0));
    }

    #[test]
    fn parse_inbound_message_covers_ui_and_voice_commands() {
        let pos_msg = OscMessage {
            addr: "/nooise/cmd/control/ratio".into(),
            args: vec![OscType::String("pad.level".into()), OscType::Float(0.75)],
        };
        assert_eq!(
            parse_inbound_message(&pos_msg),
            Some(OscInboundCommand::Ui(OscUiCommand::ControlRatio {
                id: "pad.level".into(),
                ratio: 0.75,
            }))
        );

        let nav_msg = OscMessage {
            addr: "/nooise/cmd/nav/layer".into(),
            args: vec![OscType::String("pads".into())],
        };
        assert_eq!(
            parse_inbound_message(&nav_msg),
            Some(OscInboundCommand::Ui(OscUiCommand::NavLayer(Tab::Chords)))
        );

        let mute_msg = OscMessage {
            addr: "/nooise/cmd/mute/set".into(),
            args: vec![OscType::String("kick".into()), OscType::Int(1)],
        };
        assert_eq!(
            parse_inbound_message(&mute_msg),
            Some(OscInboundCommand::Ui(OscUiCommand::MuteSet {
                tab: Tab::Kick,
                muted: true,
            }))
        );

        let note_msg = OscMessage {
            addr: "/nooise/cmd/voice/lead/note_on".into(),
            args: vec![OscType::Int(60)],
        };
        assert_eq!(
            parse_inbound_message(&note_msg),
            Some(OscInboundCommand::Voice(OscVoiceCommand::LeadNoteOn(60)))
        );

        let chord_msg = OscMessage {
            addr: "/nooise/cmd/voice/pad/notes".into(),
            args: vec![OscType::Int(60), OscType::Int(64), OscType::Int(67)],
        };
        assert_eq!(
            parse_inbound_message(&chord_msg),
            Some(OscInboundCommand::Voice(OscVoiceCommand::PadNotes(vec![
                60, 64, 67
            ])))
        );
    }
}
