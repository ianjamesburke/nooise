//! Opt-in MIDI input and output with bounded, nonblocking audio-thread queues.
//! Note input is never echoed directly; generated Pad, Arp, and Lead notes
//! and the engine's transport clock go to the chosen output channel.

use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

use midir::{MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};

use crate::fluid::{TimingContext, Transport, pad_voicing};

const QUEUE_CAPACITY: usize = 4096;
const CLOCKS_PER_BEAT: f64 = 24.0;
const PAD_VELOCITY: u8 = 100;
const ARP_VELOCITY: u8 = 100;
const LEAD_VELOCITY: u8 = 100;
const INPUT_DRAIN_LIMIT: usize = 128;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct MidiConfig<'a> {
    pub(crate) input: Option<MidiEndpoint<'a>>,
    pub(crate) output: Option<MidiEndpoint<'a>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MidiEndpoint<'a> {
    pub(crate) name: &'a str,
    /// Human-facing channel, 1 through 16.
    pub(crate) channel: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MidiInputEvent {
    NoteOn(u8),
    NoteOff(u8),
    AllNotesOff,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MidiMessage {
    Start,
    Continue,
    Stop,
    Clock,
    PadChord(PadMidiNotes),
    PadOff,
    ArpNote(u8),
    ArpOff,
    LeadNote(u8),
    LeadOff,
    Shutdown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PadMidiNotes {
    notes: [u8; 5],
    count: usize,
}

impl PadMidiNotes {
    fn active(&self) -> &[u8] {
        &self.notes[..self.count]
    }
}

pub(crate) enum MidiPortError {
    Missing {
        direction: &'static str,
        name: String,
    },
    Ambiguous {
        direction: &'static str,
        name: String,
    },
}

impl fmt::Debug for MidiPortError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for MidiPortError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing { direction, name } => write!(
                f,
                "MIDI {direction} port {name:?} was not found; run nooise midi-ports to list names"
            ),
            Self::Ambiguous { direction, name } => {
                write!(f, "multiple MIDI {direction} ports are named {name:?}")
            }
        }
    }
}

impl Error for MidiPortError {}

#[derive(Clone)]
pub(crate) struct MidiSink {
    sender: mpsc::SyncSender<MidiMessage>,
    overflowed: Arc<AtomicBool>,
}

impl MidiSink {
    /// The audio callback never waits for the MIDI backend or sender thread.
    pub(crate) fn send(&self, message: MidiMessage) {
        if self.sender.try_send(message).is_err() {
            self.overflowed.store(true, Ordering::Release);
        }
    }

    #[cfg(test)]
    pub(crate) fn test_channel() -> (Self, mpsc::Receiver<MidiMessage>) {
        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        (
            Self {
                sender,
                overflowed: Arc::new(AtomicBool::new(false)),
            },
            receiver,
        )
    }
}

pub(crate) struct MidiOutputManager {
    sink: MidiSink,
    writer: Option<thread::JoinHandle<()>>,
}

impl MidiOutputManager {
    pub(crate) fn open(endpoint: MidiEndpoint<'_>) -> Result<Self, Box<dyn Error>> {
        let output = MidiOutput::new("nooise MIDI output")?;
        let mut matching = output.ports().into_iter().filter(|port| {
            output
                .port_name(port)
                .is_ok_and(|found| found == endpoint.name)
        });
        let port = matching.next().ok_or_else(|| MidiPortError::Missing {
            direction: "output",
            name: endpoint.name.to_owned(),
        })?;
        if matching.next().is_some() {
            return Err(Box::new(MidiPortError::Ambiguous {
                direction: "output",
                name: endpoint.name.to_owned(),
            }));
        }
        let connection = output.connect(&port, "nooise MIDI out")?;
        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        let overflowed = Arc::new(AtomicBool::new(false));
        let writer_overflowed = Arc::clone(&overflowed);
        let writer = thread::Builder::new()
            .name("nooise-midi-out".into())
            .spawn(move || {
                write_events(
                    connection,
                    receiver,
                    writer_overflowed,
                    endpoint.channel - 1,
                )
            })?;
        Ok(Self {
            sink: MidiSink { sender, overflowed },
            writer: Some(writer),
        })
    }

    pub(crate) fn sink(&self) -> MidiSink {
        self.sink.clone()
    }
}

pub(crate) fn list_ports() -> Result<(), Box<dyn Error>> {
    let input = MidiInput::new("nooise MIDI input discovery")?;
    println!("Input:");
    for port in input.ports() {
        println!("  {}", input.port_name(&port)?);
    }
    let output = MidiOutput::new("nooise MIDI discovery")?;
    println!("Output:");
    for port in output.ports() {
        println!("  {}", output.port_name(&port)?);
    }
    Ok(())
}

#[derive(Clone)]
pub(crate) struct MidiInputSource {
    receiver: Arc<Mutex<mpsc::Receiver<MidiInputEvent>>>,
    overflowed: Arc<AtomicBool>,
}

impl MidiInputSource {
    #[cfg(test)]
    pub(crate) fn test_channel() -> (Self, mpsc::SyncSender<MidiInputEvent>) {
        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        (
            Self {
                receiver: Arc::new(Mutex::new(receiver)),
                overflowed: Arc::new(AtomicBool::new(false)),
            },
            sender,
        )
    }

    /// A busy input callback never makes the audio thread wait. When the
    /// bounded queue overflows, the caller releases its held input notes.
    pub(crate) fn drain(&self, mut consume: impl FnMut(MidiInputEvent)) -> bool {
        let overflowed = self.overflowed.swap(false, Ordering::AcqRel);
        if let Ok(receiver) = self.receiver.try_lock() {
            for _ in 0..INPUT_DRAIN_LIMIT {
                match receiver.try_recv() {
                    Ok(event) => consume(event),
                    Err(_) => break,
                }
            }
        }
        overflowed
    }
}

pub(crate) struct MidiInputManager {
    _connection: MidiInputConnection<()>,
    source: MidiInputSource,
}

impl MidiInputManager {
    pub(crate) fn open(endpoint: MidiEndpoint<'_>) -> Result<Self, Box<dyn Error>> {
        let input = MidiInput::new("nooise MIDI input")?;
        let mut matching = input.ports().into_iter().filter(|port| {
            input
                .port_name(port)
                .is_ok_and(|found| found == endpoint.name)
        });
        let port = matching.next().ok_or_else(|| MidiPortError::Missing {
            direction: "input",
            name: endpoint.name.to_owned(),
        })?;
        if matching.next().is_some() {
            return Err(Box::new(MidiPortError::Ambiguous {
                direction: "input",
                name: endpoint.name.to_owned(),
            }));
        }
        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        let overflowed = Arc::new(AtomicBool::new(false));
        let callback_overflowed = Arc::clone(&overflowed);
        let channel = endpoint.channel - 1;
        let connection = input.connect(
            &port,
            "nooise MIDI in",
            move |_, bytes, _| {
                if let Some(event) = decode_input(bytes, channel)
                    && sender.try_send(event).is_err()
                {
                    callback_overflowed.store(true, Ordering::Release);
                }
            },
            (),
        )?;
        Ok(Self {
            _connection: connection,
            source: MidiInputSource {
                receiver: Arc::new(Mutex::new(receiver)),
                overflowed,
            },
        })
    }

    pub(crate) fn source(&self) -> MidiInputSource {
        self.source.clone()
    }
}

fn decode_input(bytes: &[u8], channel: u8) -> Option<MidiInputEvent> {
    if bytes.len() < 3 || bytes[0] & 0x0f != channel {
        return None;
    }
    match bytes[0] & 0xf0 {
        0x90 if bytes[2] != 0 => Some(MidiInputEvent::NoteOn(bytes[1])),
        0x80 | 0x90 => Some(MidiInputEvent::NoteOff(bytes[1])),
        0xb0 if bytes[1] == 120 || bytes[1] == 123 => Some(MidiInputEvent::AllNotesOff),
        _ => None,
    }
}

impl Drop for MidiOutputManager {
    fn drop(&mut self) {
        // The audio stream is dropped before this manager by run_interactive.
        // Shutdown is sent after its last possible event.
        let _ = self.sink.sender.send(MidiMessage::Shutdown);
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

fn write_events(
    mut connection: MidiOutputConnection,
    receiver: mpsc::Receiver<MidiMessage>,
    overflowed: Arc<AtomicBool>,
    channel: u8,
) {
    let mut active = ActiveNotes::default();
    while let Ok(message) = receiver.recv() {
        if overflowed.swap(false, Ordering::AcqRel) {
            // A dropped Note Off must never leave a stuck note on the synth.
            let _ = connection.send(&[0xb0 | channel, 123, 0]);
            active = ActiveNotes::default();
        }
        match dispatch_on_channel(message, channel, &mut active, &mut |bytes| {
            connection.send(bytes)
        }) {
            Ok(true) => {}
            Ok(false) => return,
            Err(_) => {
                // A disconnected output cannot be repaired in the audio callback.
                let _ = connection.send(&[0xb0 | channel, 123, 0]);
                return;
            }
        }
    }
    let _ = release_all(&mut active, channel, &mut |bytes| connection.send(bytes));
    let _ = connection.send(&[0xfc]);
}

#[derive(Default)]
struct ActiveNotes {
    pad: Option<PadMidiNotes>,
    arp: Option<u8>,
    lead: Option<u8>,
}

impl ActiveNotes {
    fn contains(&self, note: u8) -> bool {
        self.pad.is_some_and(|notes| notes.active().contains(&note))
            || self.arp == Some(note)
            || self.lead == Some(note)
    }
}

fn dispatch_on_channel<E>(
    message: MidiMessage,
    channel: u8,
    active: &mut ActiveNotes,
    send: &mut impl FnMut(&[u8]) -> Result<(), E>,
) -> Result<bool, E> {
    match message {
        MidiMessage::Start => send(&[0xfa])?,
        MidiMessage::Continue => send(&[0xfb])?,
        MidiMessage::Stop => send(&[0xfc])?,
        MidiMessage::Clock => send(&[0xf8])?,
        MidiMessage::PadChord(notes) => {
            release_pad(active, channel, send)?;
            for &note in notes.active() {
                if !active.contains(note) {
                    send(&[0x90 | channel, note, PAD_VELOCITY])?;
                }
            }
            active.pad = Some(notes);
        }
        MidiMessage::PadOff => release_pad(active, channel, send)?,
        MidiMessage::ArpNote(note) => {
            release_arp(active, channel, send)?;
            if !active.contains(note) {
                send(&[0x90 | channel, note, ARP_VELOCITY])?;
            }
            active.arp = Some(note);
        }
        MidiMessage::ArpOff => release_arp(active, channel, send)?,
        MidiMessage::LeadNote(note) => {
            release_lead(active, channel, send)?;
            if !active.contains(note) {
                send(&[0x90 | channel, note, LEAD_VELOCITY])?;
            }
            active.lead = Some(note);
        }
        MidiMessage::LeadOff => release_lead(active, channel, send)?,
        MidiMessage::Shutdown => {
            release_all(active, channel, send)?;
            send(&[0xb0 | channel, 123, 0])?;
            send(&[0xfc])?;
            return Ok(false);
        }
    }
    Ok(true)
}

fn release_pad<E>(
    active: &mut ActiveNotes,
    channel: u8,
    send: &mut impl FnMut(&[u8]) -> Result<(), E>,
) -> Result<(), E> {
    if let Some(notes) = active.pad.take() {
        for &note in notes.active() {
            if !active.contains(note) {
                send(&[0x80 | channel, note, 0])?;
            }
        }
    }
    Ok(())
}

fn release_arp<E>(
    active: &mut ActiveNotes,
    channel: u8,
    send: &mut impl FnMut(&[u8]) -> Result<(), E>,
) -> Result<(), E> {
    if let Some(note) = active.arp.take()
        && !active.contains(note)
    {
        send(&[0x80 | channel, note, 0])?;
    }
    Ok(())
}

fn release_lead<E>(
    active: &mut ActiveNotes,
    channel: u8,
    send: &mut impl FnMut(&[u8]) -> Result<(), E>,
) -> Result<(), E> {
    if let Some(note) = active.lead.take()
        && !active.contains(note)
    {
        send(&[0x80 | channel, note, 0])?;
    }
    Ok(())
}

fn release_all<E>(
    active: &mut ActiveNotes,
    channel: u8,
    send: &mut impl FnMut(&[u8]) -> Result<(), E>,
) -> Result<(), E> {
    release_pad(active, channel, send)?;
    release_arp(active, channel, send)?;
    release_lead(active, channel, send)
}

#[cfg(test)]
fn dispatch<E>(
    message: MidiMessage,
    active: &mut ActiveNotes,
    send: &mut impl FnMut(&[u8]) -> Result<(), E>,
) -> Result<bool, E> {
    dispatch_on_channel(message, 0, active, send)
}

pub(crate) struct MidiClockFollower {
    sink: MidiSink,
    last_transport: Option<Transport>,
    next_clock: u64,
}

impl MidiClockFollower {
    pub(crate) fn new(sink: MidiSink) -> Self {
        Self {
            sink,
            last_transport: None,
            next_clock: 0,
        }
    }

    pub(crate) fn tick(&mut self, timing: TimingContext) {
        // A clock that has not started has nothing to stop, so a launch that
        // waits for its downbeat still opens with Start rather than Continue.
        if self.last_transport.is_none() && timing.transport == Transport::Stopped {
            return;
        }
        if self.last_transport != Some(timing.transport) {
            match timing.transport {
                Transport::Playing => self.sink.send(if self.last_transport.is_none() {
                    MidiMessage::Start
                } else {
                    MidiMessage::Continue
                }),
                Transport::Stopped => {
                    self.sink.send(MidiMessage::PadOff);
                    self.sink.send(MidiMessage::ArpOff);
                    self.sink.send(MidiMessage::LeadOff);
                    self.sink.send(MidiMessage::Stop);
                }
            }
            self.last_transport = Some(timing.transport);
        }
        if timing.transport == Transport::Playing {
            let current_clock = (timing.beat * CLOCKS_PER_BEAT).floor() as u64;
            while self.next_clock <= current_clock {
                self.sink.send(MidiMessage::Clock);
                self.next_clock += 1;
            }
        }
    }

    pub(crate) fn restart(&mut self) {
        self.sink.send(MidiMessage::PadOff);
        self.sink.send(MidiMessage::ArpOff);
        self.sink.send(MidiMessage::LeadOff);
        self.last_transport = None;
        self.next_clock = 0;
    }
}

impl Drop for MidiClockFollower {
    fn drop(&mut self) {
        self.sink.send(MidiMessage::PadOff);
        self.sink.send(MidiMessage::ArpOff);
        self.sink.send(MidiMessage::LeadOff);
        self.sink.send(MidiMessage::Stop);
    }
}

#[cfg(test)]
pub(crate) fn pad_notes(notes: [i32; 4], tune: f32) -> PadMidiNotes {
    pad_notes_with_count(notes, 4, tune)
}

pub(crate) fn pad_notes_with_count(notes: [i32; 4], count: usize, tune: f32) -> PadMidiNotes {
    let (voicing, count) = pad_voicing(notes, count);
    PadMidiNotes {
        notes: voicing.map(|note| tuned_note(note, tune)),
        count,
    }
}

pub(crate) fn tuned_note(note: i32, tune: f32) -> u8 {
    (note + tune.round() as i32).clamp(0, 127) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clock_waiting_for_its_first_downbeat_opens_with_start() {
        let (sender, receiver) = mpsc::sync_channel(64);
        let sink = MidiSink {
            sender,
            overflowed: Arc::new(AtomicBool::new(false)),
        };
        let mut clock = MidiClockFollower::new(sink);
        let mut waiting = TimingContext::new(44_100.0, 120.0, 0.0);
        waiting.transport = Transport::Stopped;
        clock.tick(waiting);
        clock.tick(TimingContext::new(44_100.0, 120.0, 0.0));
        assert_eq!(
            receiver.try_iter().collect::<Vec<_>>(),
            [MidiMessage::Start, MidiMessage::Clock]
        );
    }

    #[test]
    fn clock_follows_audio_beats_and_transport_without_restarting_the_song() {
        let (sender, receiver) = mpsc::sync_channel(64);
        let sink = MidiSink {
            sender,
            overflowed: Arc::new(AtomicBool::new(false)),
        };
        let mut clock = MidiClockFollower::new(sink);
        for beat in [0.0, 1.0 / 48.0, 1.0 / 24.0, 2.0 / 24.0] {
            clock.tick(TimingContext::new(44_100.0, 120.0, beat));
        }
        let mut stopped = TimingContext::new(44_100.0, 120.0, 2.0 / 24.0);
        stopped.transport = Transport::Stopped;
        clock.tick(stopped);
        clock.tick(stopped);
        clock.tick(TimingContext::new(44_100.0, 120.0, 2.0 / 24.0));
        assert_eq!(
            receiver.try_iter().collect::<Vec<_>>(),
            [
                MidiMessage::Start,
                MidiMessage::Clock,
                MidiMessage::Clock,
                MidiMessage::Clock,
                MidiMessage::PadOff,
                MidiMessage::ArpOff,
                MidiMessage::LeadOff,
                MidiMessage::Stop,
                MidiMessage::Continue,
            ]
        );
    }

    #[test]
    fn pad_notes_follow_master_tune_without_level_gating() {
        assert_eq!(pad_notes([48, 52, 55, 60], 12.0).active(), [60, 64, 67, 72]);
    }

    #[test]
    fn pad_note_count_uses_fifth_for_two_and_top_root_for_five() {
        assert_eq!(
            pad_notes_with_count([48, 52, 55, 60], 2, 0.0).active(),
            [48, 55]
        );
        assert_eq!(
            pad_notes_with_count([48, 52, 55, 60], 5, 0.0).active(),
            [48, 52, 55, 60, 72]
        );
    }

    #[test]
    fn five_note_pad_chord_releases_all_five_notes() {
        let mut packets = Vec::<Vec<u8>>::new();
        let mut active = ActiveNotes::default();
        let mut send = |bytes: &[u8]| {
            packets.push(bytes.to_vec());
            Ok::<(), ()>(())
        };
        dispatch(
            MidiMessage::PadChord(pad_notes_with_count([48, 52, 55, 60], 5, 0.0)),
            &mut active,
            &mut send,
        )
        .unwrap();
        dispatch(MidiMessage::PadOff, &mut active, &mut send).unwrap();
        assert_eq!(packets.len(), 10);
        assert_eq!(packets[4], [0x90, 72, PAD_VELOCITY]);
        assert_eq!(packets[9], [0x80, 72, 0]);
    }

    #[test]
    fn pad_chord_change_releases_old_notes_and_shutdown_clears_channel_one() {
        let mut packets = Vec::<Vec<u8>>::new();
        let mut active = ActiveNotes::default();
        let mut send = |bytes: &[u8]| {
            packets.push(bytes.to_vec());
            Ok::<(), ()>(())
        };
        dispatch(
            MidiMessage::PadChord(pad_notes([48, 52, 55, 60], 0.0)),
            &mut active,
            &mut send,
        )
        .unwrap();
        dispatch(
            MidiMessage::PadChord(pad_notes([50, 53, 57, 62], 0.0)),
            &mut active,
            &mut send,
        )
        .unwrap();
        assert!(!dispatch(MidiMessage::Shutdown, &mut active, &mut send).unwrap());
        assert_eq!(packets[0], [0x90, 48, PAD_VELOCITY]);
        assert_eq!(packets[4], [0x80, 48, 0]);
        assert_eq!(packets[8], [0x90, 50, PAD_VELOCITY]);
        assert_eq!(
            &packets[12..16],
            &[
                vec![0x80, 50, 0],
                vec![0x80, 53, 0],
                vec![0x80, 57, 0],
                vec![0x80, 62, 0]
            ]
        );
        assert_eq!(&packets[16..], &[vec![0xb0, 123, 0], vec![0xfc]]);
    }

    #[test]
    fn arp_note_and_pad_chord_release_independently() {
        let mut packets = Vec::<Vec<u8>>::new();
        let mut active = ActiveNotes::default();
        let mut send = |bytes: &[u8]| {
            packets.push(bytes.to_vec());
            Ok::<(), ()>(())
        };
        dispatch(
            MidiMessage::PadChord(pad_notes([48, 52, 55, 60], 0.0)),
            &mut active,
            &mut send,
        )
        .unwrap();
        dispatch(MidiMessage::ArpNote(64), &mut active, &mut send).unwrap();
        dispatch(MidiMessage::ArpOff, &mut active, &mut send).unwrap();
        dispatch(MidiMessage::PadOff, &mut active, &mut send).unwrap();
        assert_eq!(
            &packets[4..],
            &[
                vec![0x90, 64, ARP_VELOCITY],
                vec![0x80, 64, 0],
                vec![0x80, 48, 0],
                vec![0x80, 52, 0],
                vec![0x80, 55, 0],
                vec![0x80, 60, 0],
            ]
        );
    }

    #[test]
    fn all_three_sources_release_shared_notes_without_channel_wide_reset() {
        for order in [
            [
                MidiMessage::PadOff,
                MidiMessage::ArpOff,
                MidiMessage::LeadOff,
            ],
            [
                MidiMessage::LeadOff,
                MidiMessage::PadOff,
                MidiMessage::ArpOff,
            ],
        ] {
            let mut packets = Vec::<Vec<u8>>::new();
            let mut active = ActiveNotes::default();
            let mut send = |bytes: &[u8]| {
                packets.push(bytes.to_vec());
                Ok::<(), ()>(())
            };
            dispatch(
                MidiMessage::PadChord(pad_notes([48, 52, 55, 60], 0.0)),
                &mut active,
                &mut send,
            )
            .unwrap();
            dispatch(MidiMessage::ArpNote(60), &mut active, &mut send).unwrap();
            dispatch(MidiMessage::LeadNote(60), &mut active, &mut send).unwrap();
            for off in order {
                dispatch(off, &mut active, &mut send).unwrap();
            }
            assert_eq!(packets.last().unwrap(), &[0x80, 60, 0]);
            assert_eq!(
                packets
                    .iter()
                    .filter(|packet| packet.as_slice() == [0x80, 60, 0])
                    .count(),
                1
            );
            assert_eq!(packets.len(), 8);
            assert!(packets.iter().all(|packet| packet[0] != 0xb0));
        }
    }

    #[test]
    fn shared_pitch_stays_on_until_both_sources_release_it() {
        let mut packets = Vec::<Vec<u8>>::new();
        let mut active = ActiveNotes::default();
        let mut send = |bytes: &[u8]| {
            packets.push(bytes.to_vec());
            Ok::<(), ()>(())
        };
        dispatch(MidiMessage::ArpNote(60), &mut active, &mut send).unwrap();
        dispatch(MidiMessage::LeadNote(60), &mut active, &mut send).unwrap();
        dispatch(MidiMessage::ArpOff, &mut active, &mut send).unwrap();
        dispatch(MidiMessage::LeadOff, &mut active, &mut send).unwrap();
        assert_eq!(packets, [vec![0x90, 60, ARP_VELOCITY], vec![0x80, 60, 0]]);
    }

    #[test]
    fn shutdown_releases_an_active_arp_note() {
        let mut packets = Vec::<Vec<u8>>::new();
        let mut active = ActiveNotes::default();
        let mut send = |bytes: &[u8]| {
            packets.push(bytes.to_vec());
            Ok::<(), ()>(())
        };
        dispatch(MidiMessage::ArpNote(64), &mut active, &mut send).unwrap();
        assert!(!dispatch(MidiMessage::Shutdown, &mut active, &mut send).unwrap());
        assert_eq!(
            packets,
            [
                vec![0x90, 64, ARP_VELOCITY],
                vec![0x80, 64, 0],
                vec![0xb0, 123, 0],
                vec![0xfc],
            ]
        );
    }

    #[test]
    fn input_decodes_only_note_messages_on_the_selected_channel() {
        assert_eq!(
            decode_input(&[0x91, 60, 100], 1),
            Some(MidiInputEvent::NoteOn(60))
        );
        assert_eq!(
            decode_input(&[0x91, 60, 0], 1),
            Some(MidiInputEvent::NoteOff(60))
        );
        assert_eq!(
            decode_input(&[0x81, 60, 64], 1),
            Some(MidiInputEvent::NoteOff(60))
        );
        assert_eq!(decode_input(&[0x90, 60, 100], 1), None);
        assert_eq!(decode_input(&[0xb1, 64, 127], 1), None);
        assert_eq!(
            decode_input(&[0xb1, 123, 0], 1),
            Some(MidiInputEvent::AllNotesOff)
        );
        assert_eq!(decode_input(&[0xf8], 1), None);
    }

    #[test]
    fn output_notes_and_safety_off_use_selected_channel_but_clock_is_global() {
        let mut packets = Vec::<Vec<u8>>::new();
        let mut active = ActiveNotes::default();
        let mut send = |bytes: &[u8]| {
            packets.push(bytes.to_vec());
            Ok::<(), ()>(())
        };
        dispatch_on_channel(MidiMessage::Clock, 4, &mut active, &mut send).unwrap();
        dispatch_on_channel(MidiMessage::LeadNote(60), 4, &mut active, &mut send).unwrap();
        dispatch_on_channel(MidiMessage::Shutdown, 4, &mut active, &mut send).unwrap();
        assert_eq!(
            packets,
            [
                vec![0xf8],
                vec![0x94, 60, LEAD_VELOCITY],
                vec![0x84, 60, 0],
                vec![0xb4, 123, 0],
                vec![0xfc],
            ]
        );
    }
}
