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
//! - `/nooise/chord` `i32` — pad chord index, sent on change
//! - `/nooise/voice/kick` — one message per kick hit, no arguments

use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rosc::{OscBundle, OscMessage, OscPacket, OscTime, OscType, encoder};

use super::FluidTelemetry;

pub(crate) const ADDR_BEAT: &str = "/nooise/beat";
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
    chord: u64,
    beat_bits: u64,
}

impl Mirrored {
    fn from(telemetry: &FluidTelemetry) -> Self {
        Self {
            kick: telemetry.kick_pulse.load(Ordering::Relaxed),
            chord: telemetry.chord_slot.load(Ordering::Relaxed),
            beat_bits: telemetry.beat_bits.load(Ordering::Relaxed),
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
        if now.chord != self.chord {
            out.push(message(ADDR_CHORD, vec![OscType::Int(now.chord as i32)]));
        }
        for _ in self.kick..now.kick {
            out.push(message(ADDR_KICK, Vec::new()));
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

        telemetry.kick_pulse.fetch_add(3, Ordering::Relaxed);
        telemetry.chord_slot.store(2, Ordering::Relaxed);
        telemetry.publish_beat(4.5);
        let out = mirrored.diff(&telemetry);
        let addrs: Vec<&str> = out.iter().map(|m| m.addr.as_str()).collect();
        assert_eq!(
            addrs,
            [ADDR_BEAT, ADDR_CHORD, ADDR_KICK, ADDR_KICK, ADDR_KICK]
        );
        assert_eq!(out[0].args, vec![OscType::Float(4.5)]);
        assert_eq!(out[1].args, vec![OscType::Int(2)]);
        assert!(mirrored.diff(&telemetry).is_empty());
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

        telemetry.kick_pulse.fetch_add(1, Ordering::Relaxed);
        let mut buf = [0u8; 1024];
        let (len, _) = receiver.recv_from(&mut buf).unwrap();
        let (_, packet) = decoder::decode_udp(&buf[..len]).unwrap();
        let addrs: Vec<String> = messages_in(packet).into_iter().map(|m| m.addr).collect();
        assert_eq!(addrs, [ADDR_KICK]);
        drop(emitter);
    }

    /// The whole path a visualizer depends on: engine renders, telemetry
    /// moves, the emitter mirrors it, and beat plus kick messages land on UDP.
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
            if seen.contains_key(ADDR_BEAT) && seen.contains_key(ADDR_KICK) {
                break;
            }
        }
        drop(emitter);
        assert!(
            seen.get(ADDR_BEAT).copied().unwrap_or(0) > 0,
            "no beat: {seen:?}"
        );
        assert!(
            seen.get(ADDR_KICK).copied().unwrap_or(0) > 0,
            "no kick: {seen:?}"
        );
    }
}
