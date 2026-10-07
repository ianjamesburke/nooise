//! Opt-in OSC feed for external visualizers (`nooise --osc 127.0.0.1:9000`).
//!
//! A second reader of `FluidTelemetry`: one thread polls the same lock-free
//! counters the in-terminal visualizer animates from and mirrors every change
//! to a UDP socket as OSC. The audio thread is untouched, and a consumer that
//! is absent, slow, or dead can never back-pressure playback — UDP is
//! fire-and-forget by design, so send errors are dropped, not surfaced.
//!
//! Address vocabulary (the contract consumers like foorm build on):
//! - `/nooise/beat` `f32` — engine beat position, sent whenever it advances
//! - `/nooise/level` `f32` — master output RMS, sent whenever it changes;
//!   zero means silence no matter what the tempo is doing
//! - `/nooise/voice/<voice>/level` `f32` — that voice's RMS as it enters the
//!   mix (post effects, mute, and mix weight), for `<voice>` in `VOICES`
//! - `/nooise/chord` `i32 f32 f32` — the progression table slot (0..8) the
//!   pad is sounding plus the attack and release seconds of the layer it
//!   voiced, sent on each chord publication (including opening/restart)
//! - `/nooise/voice/kick` `f32` — legacy one-message-per-kick hit carrying
//!   `kick.level` (0 = inaudible)
//! - `/nooise/hit/<source>` `f32 f32` — one message per Beat, Pad, Perc, Kick,
//!   Tonal, Clap, or Arp onset. Arguments are that voice's level and pitch
//!   class (C = 0, B = 11/12); unpitched voices send -1 for pitch class
//! - `/nooise/chord/change` `i32 f32` — pad progression table slot (0..8)
//!   and its root pitch class, sent beside the existing `/nooise/chord` feed
//! - `/nooise/phrase/reset` — no arguments; starts a new Pad phrase epoch on
//!   initial play, transport restart, progression switch, and morph landing
//! - `/nooise/phrase` `i32 i32 i32` — zero-based phrase cycle since reset,
//!   zero-based chord index in the sounding Pad window, and chord count. Sent
//!   at each Pad chord boundary, after any reset in the same bundle
//! - `/nooise/chord/span` `f32 f32` — transport beat the sounding chord began
//!   on and its length in beats, sent whenever either changes (ahead of the
//!   chord messages it accompanies, so a consumer holds the span when the
//!   chord arrives). Chord progress is `/nooise/beat` minus the start beat,
//!   over the length; this also covers a one-chord loop that repeats a slot
//! - `/nooise/gesture/<gesture>` `f32` — that live gesture's amount over the
//!   Master bus (0..1), sent whenever it changes, for `<gesture>` in
//!   `GESTURES`; a visual can rise and return with it. `thin` is retired
//!   with its gesture and never sent

use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rosc::{OscBundle, OscMessage, OscPacket, OscTime, OscType, encoder};

use super::gesture::{GESTURE_COUNT, GestureKind};
use super::registry::{TAB_COUNT, Tab};
use super::{ChordSpan, FluidTelemetry, MUSICAL_HIT_COUNT, MusicalHit};

pub(crate) const ADDR_BEAT: &str = "/nooise/beat";
pub(crate) const ADDR_LEVEL: &str = "/nooise/level";
/// Voice names in `Tab` order, minus `Tab::Master` (which is `ADDR_LEVEL`).
pub(crate) const VOICES: [&str; TAB_COUNT - 1] = [
    "pad", "perc", "bass", "kick", "tonal", "clap", "arp", "lead",
];

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
pub(crate) const ADDR_CHORD: &str = "/nooise/chord";
pub(crate) const ADDR_CHORD_CHANGE: &str = "/nooise/chord/change";
pub(crate) const ADDR_PHRASE_RESET: &str = "/nooise/phrase/reset";
pub(crate) const ADDR_PHRASE: &str = "/nooise/phrase";
pub(crate) const ADDR_CHORD_SPAN: &str = "/nooise/chord/span";
pub(crate) const ADDR_KICK: &str = "/nooise/voice/kick";
/// Names in `MusicalHit::ALL` order. They are part of the OSC contract.
pub(crate) const HIT_NAMES: [&str; MUSICAL_HIT_COUNT] =
    ["beat", "pad", "perc", "kick", "tonal", "clap", "arp"];

fn hit_addr(hit: MusicalHit) -> String {
    format!("/nooise/hit/{}", HIT_NAMES[hit as usize])
}

/// How often the emitter samples telemetry. Kick hits are counted, never
/// missed, but their timestamps carry up to this much jitter.
const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// Owns the emitter thread; dropping it stops the feed.
pub(crate) struct OscEmitter {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl OscEmitter {
    pub(crate) fn spawn(target: SocketAddr, telemetry: Arc<FluidTelemetry>) -> io::Result<Self> {
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
            .spawn(move || emit_loop(&socket, target, &telemetry, mirrored, &stop_for_thread))?;
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
    mut mirrored: Mirrored,
    stop: &AtomicBool,
) {
    while !stop.load(Ordering::Relaxed) {
        let messages = mirrored.diff(telemetry);
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
    use super::*;
    use rosc::decoder;

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
        let mut pad = PadEngine::new(48_000.0, &controls, 0.0, Arc::clone(&telemetry));
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
            pad.next(controls, 0.0, timing);
            mirrored.diff(telemetry)
        }
        let tick_at = |pad: &mut PadEngine, mirrored: &mut Mirrored, beat, morph_start| {
            tick(pad, &controls, &telemetry, mirrored, beat, morph_start)
        };
        let start = tick_at(&mut pad, &mut mirrored, 0.0, None);
        assert_eq!(
            start.iter().map(|m| m.addr.as_str()).collect::<Vec<_>>(),
            [
                ADDR_CHORD,
                ADDR_CHORD_CHANGE,
                ADDR_PHRASE_RESET,
                ADDR_PHRASE
            ]
        );
        assert_eq!(
            start[3].args,
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
        pad.next(&controls, 0.0, stopped);
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
            if [ADDR_BEAT, ADDR_LEVEL, ADDR_KICK, ADDR_PHRASE, ADDR_CHORD_SPAN]
                .iter()
                .all(|a| seen.contains_key(*a))
            {
                break;
            }
        }
        drop(emitter);
        for addr in [ADDR_BEAT, ADDR_LEVEL, ADDR_KICK, ADDR_PHRASE, ADDR_CHORD_SPAN] {
            assert!(
                seen.get(addr).copied().unwrap_or(0) > 0,
                "no {addr}: {seen:?}"
            );
        }
        // Default chord length: 4 bars of 4 beats.
        assert_eq!(span_args[1], OscType::Float(16.0));
    }
}
