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
//!   voiced, sent on change
//! - `/nooise/voice/kick` `f32` — one message per kick hit carrying
//!   `kick.level` (0 = inaudible)
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

use super::FluidTelemetry;
use super::gesture::{GESTURE_COUNT, GestureKind};
use super::registry::{TAB_COUNT, Tab};

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
pub(crate) const ADDR_KICK: &str = "/nooise/voice/kick";

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
    chord: u64,
    chord_attack: f32,
    chord_release: f32,
    beat_bits: u64,
    level_bits: [u32; TAB_COUNT],
    gesture_bits: [u32; GESTURE_COUNT],
}

impl Mirrored {
    fn from(telemetry: &FluidTelemetry) -> Self {
        // Acquire pairs with the Release in `publish_kick`/`publish_chord`:
        // once the counter is seen, the values stored before it are too.
        let kick = telemetry.kick_pulse.load(Ordering::Acquire);
        let chord = telemetry.chord_slot.load(Ordering::Acquire);
        Self {
            kick,
            kick_level: f32::from_bits(telemetry.kick_level_bits.load(Ordering::Relaxed)),
            chord,
            chord_attack: f32::from_bits(telemetry.chord_attack_bits.load(Ordering::Relaxed)),
            chord_release: f32::from_bits(telemetry.chord_release_bits.load(Ordering::Relaxed)),
            beat_bits: telemetry.beat_bits.load(Ordering::Relaxed),
            level_bits: std::array::from_fn(|i| telemetry.level_bits[i].load(Ordering::Relaxed)),
            gesture_bits: std::array::from_fn(|i| {
                telemetry.gesture_bits[i].load(Ordering::Relaxed)
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
        if now.chord != self.chord {
            out.push(message(
                ADDR_CHORD,
                vec![
                    OscType::Int(now.chord as i32),
                    OscType::Float(now.chord_attack),
                    OscType::Float(now.chord_release),
                ],
            ));
        }
        for _ in self.kick..now.kick {
            out.push(message(ADDR_KICK, vec![OscType::Float(now.kick_level)]));
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
        telemetry.publish_chord(2, 6.0, 8.0);
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
                ADDR_CHORD,
                ADDR_KICK,
                ADDR_KICK,
                ADDR_KICK
            ]
        );
        assert_eq!(out[0].args, vec![OscType::Float(4.5)]);
        assert_eq!(out[1].args, vec![OscType::Float(0.05)]);
        assert_eq!(out[2].args, vec![OscType::Float(0.1)]);
        assert_eq!(out[3].args, vec![OscType::Float(0.6)]);
        assert_eq!(
            out[4].args,
            vec![OscType::Int(2), OscType::Float(6.0), OscType::Float(8.0)]
        );
        // Hits inside one poll share the latest level.
        assert_eq!(out[5].args, vec![OscType::Float(0.75)]);
        assert!(mirrored.diff(&telemetry).is_empty());
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
        assert_eq!(addrs, [ADDR_KICK]);
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
                    *seen.entry(m.addr).or_default() += 1;
                }
                receiver
                    .set_read_timeout(Some(Duration::from_millis(1)))
                    .unwrap();
            }
            if [ADDR_BEAT, ADDR_LEVEL, ADDR_KICK]
                .iter()
                .all(|a| seen.contains_key(*a))
            {
                break;
            }
        }
        drop(emitter);
        for addr in [ADDR_BEAT, ADDR_LEVEL, ADDR_KICK] {
            assert!(
                seen.get(addr).copied().unwrap_or(0) > 0,
                "no {addr}: {seen:?}"
            );
        }
    }
}
