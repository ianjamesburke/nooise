//! Opt-in Ableton Link (`--link`): shared tempo, bar phase, and start/stop
//! with every Link peer on the local network. The audio thread is the only
//! reader and writer of the Link timeline (`LinkClock`); peer tempo and
//! start/stop reach the UI thread through `LinkPeerChanges`, which publishes
//! them through the ordinary live-session boundary (`LinkFollower`).

use super::*;

use std::sync::atomic::{AtomicU8, AtomicU64};
use std::time::Duration;

use rusty_link::{AblLink, HostTimeFilter, SessionState};

/// Beats per Link phase cycle: one nooise bar, so a launch lands on a downbeat.
pub(crate) const LINK_QUANTUM: f64 = 4.0;

/// `tempo_bits` sentinel: a NaN pattern no real tempo produces.
const NO_TEMPO: u64 = u64::MAX;
const NO_TRANSPORT: u8 = 0;
const PEER_PLAY: u8 = 1;
const PEER_STOP: u8 = 2;

/// Membership in the Link session for one live run; dropping every clone of
/// the inner `AblLink` leaves the session.
pub(crate) struct LinkSession {
    pub(crate) link: Arc<AblLink>,
    pub(crate) changes: Arc<LinkPeerChanges>,
}

impl LinkSession {
    pub(crate) fn join(bpm: f32) -> Self {
        let link = Arc::new(AblLink::new(f64::from(bpm)));
        link.enable_start_stop_sync(true);
        link.enable(true);
        Self {
            link,
            changes: Arc::new(LinkPeerChanges::default()),
        }
    }

    /// The audio side. Built once per output stream, off the audio thread,
    /// because `SessionState::new` is not realtime-safe.
    pub(crate) fn clock(&self, sample_rate: f32) -> LinkClock {
        LinkClock::new(
            Arc::clone(&self.link),
            Arc::clone(&self.changes),
            sample_rate,
        )
    }

    pub(crate) fn follower(&self) -> LinkFollower {
        LinkFollower {
            changes: Arc::clone(&self.changes),
        }
    }
}

/// Peer-originated changes the audio thread saw on the timeline and the UI
/// thread has not yet applied. Each slot holds only the latest value.
#[derive(Debug)]
pub(crate) struct LinkPeerChanges {
    pub(crate) transport: AtomicU8,
    pub(crate) tempo_bits: AtomicU64,
}

impl Default for LinkPeerChanges {
    fn default() -> Self {
        Self {
            transport: AtomicU8::new(NO_TRANSPORT),
            tempo_bits: AtomicU64::new(NO_TEMPO),
        }
    }
}

impl LinkPeerChanges {
    pub(crate) fn report_transport(&self, playing: bool) {
        let value = if playing { PEER_PLAY } else { PEER_STOP };
        self.transport.store(value, Ordering::Release);
    }

    pub(crate) fn report_tempo(&self, bpm: f64) {
        self.tempo_bits.store(bpm.to_bits(), Ordering::Release);
    }

    pub(crate) fn take_transport(&self) -> Option<Transport> {
        match self.transport.swap(NO_TRANSPORT, Ordering::Acquire) {
            PEER_PLAY => Some(Transport::Playing),
            PEER_STOP => Some(Transport::Stopped),
            _ => None,
        }
    }

    pub(crate) fn take_tempo(&self) -> Option<f64> {
        match self.tempo_bits.swap(NO_TEMPO, Ordering::Acquire) {
            NO_TEMPO => None,
            bits => Some(f64::from_bits(bits)),
        }
    }
}

/// UI-thread half: applies peer changes each UI tick.
pub(crate) struct LinkFollower {
    pub(crate) changes: Arc<LinkPeerChanges>,
}

impl LinkFollower {
    pub(crate) fn follow_peers(&self, effects: &mut EffectExecutor) {
        if let Some(transport) = self.changes.take_transport() {
            effects.follow_peer_transport(transport);
        }
        if let Some(bpm) = self.changes.take_tempo() {
            effects.follow_peer_tempo(bpm);
        }
    }
}

/// The dial value a Link tempo lands on. Link allows a wider range than the
/// Master tempo dial; a peer outside it plays at its own tempo while the dial
/// rests at its nearest end.
pub(crate) fn dial_bpm(link_bpm: f64) -> f32 {
    (link_bpm as f32).clamp(MASTER_BPM_MIN, MASTER_BPM_MAX)
}

/// Audio-thread half: maps each output sample to a Link beat and commits
/// local tempo and start/stop edits.
pub(crate) struct LinkClock {
    pub(crate) link: Arc<AblLink>,
    pub(crate) changes: Arc<LinkPeerChanges>,
    pub(crate) state: SessionState,
    pub(crate) host_time: HostTimeFilter,
    pub(crate) micros_per_sample: f64,
    pub(crate) buffer_start_micros: i64,
    pub(crate) samples_into_buffer: u64,
    pub(crate) last_transport: Option<Transport>,
    pub(crate) last_restart: u64,
    pub(crate) last_target_bpm: Option<f32>,
    /// The timeline as this clock last left it, so a difference on the next
    /// capture is a peer's doing, never our own commit.
    pub(crate) link_playing: bool,
    pub(crate) link_bpm: f64,
    /// The last start/stop edge on the timeline was a peer's start, so the
    /// launch it prompts maps beat zero to their start time.
    pub(crate) peer_started: bool,
    /// A restart rewound the song but its launch is not mapped onto the
    /// timeline yet; the clock holds at beat zero until it is.
    pub(crate) awaiting_launch: bool,
}

impl LinkClock {
    pub(crate) fn new(link: Arc<AblLink>, changes: Arc<LinkPeerChanges>, sample_rate: f32) -> Self {
        Self {
            link,
            changes,
            state: SessionState::new(),
            host_time: HostTimeFilter::new(),
            micros_per_sample: 1_000_000.0 / f64::from(sample_rate.max(1.0)),
            buffer_start_micros: 0,
            samples_into_buffer: 0,
            last_transport: None,
            last_restart: 0,
            last_target_bpm: None,
            link_playing: false,
            link_bpm: f64::NAN,
            peer_started: false,
            awaiting_launch: true,
        }
    }

    /// Once per output buffer, before its first sample. `sample_clock` is the
    /// engine's running sample count; `output_latency` is how long the buffer
    /// waits before it is heard, so beats line up at the speaker.
    pub(crate) fn begin_buffer(
        &mut self,
        sample_clock: u64,
        output_latency: Duration,
        local: LocalTiming,
    ) {
        let latency = i64::try_from(output_latency.as_micros()).unwrap_or(i64::MAX);
        let now = self
            .host_time
            .sample_time_to_host_time(self.link.clock_micros(), sample_clock);
        self.buffer_start_micros = now.saturating_add(latency);
        self.samples_into_buffer = 0;
        self.link.capture_audio_session_state(&mut self.state);
        let at = self.buffer_start_micros;

        let playing = self.state.is_playing();
        if playing != self.link_playing {
            self.link_playing = playing;
            self.peer_started = playing;
            self.changes.report_transport(playing);
        }
        let link_bpm = self.state.tempo();
        if (link_bpm - self.link_bpm).abs() > 1e-6 || self.link_bpm.is_nan() {
            self.link_bpm = link_bpm;
            self.changes.report_tempo(link_bpm);
        }

        let mut commit = false;
        let first_buffer = self.last_target_bpm.is_none();
        if self.last_target_bpm.replace(local.target_bpm) != Some(local.target_bpm)
            && !first_buffer
            && local.target_bpm != dial_bpm(link_bpm)
        {
            self.state.set_tempo(f64::from(local.target_bpm), at);
            self.link_bpm = self.state.tempo();
            commit = true;
        }

        let restarted = local.restart != self.last_restart;
        self.last_restart = local.restart;
        let previous = self.last_transport.replace(local.transport);
        match local.transport {
            Transport::Playing if previous != Some(Transport::Playing) || restarted => {
                if first_buffer {
                    // Launching nooise joins the session in phase without
                    // starting anyone's transport; only a Play press does.
                    self.state.request_beat_at_time(0.0, at, LINK_QUANTUM);
                } else if !self.link_playing {
                    self.state
                        .set_is_playing_and_request_beat_at_time(true, at, 0.0, LINK_QUANTUM);
                    self.link_playing = true;
                } else if self.peer_started {
                    self.state
                        .request_beat_at_start_playing_time(0.0, LINK_QUANTUM);
                } else {
                    self.state.request_beat_at_time(0.0, at, LINK_QUANTUM);
                }
                self.peer_started = false;
                self.awaiting_launch = false;
                commit = true;
            }
            Transport::Stopped if previous == Some(Transport::Playing) => {
                if self.link_playing {
                    self.state.set_is_playing(false, at);
                    self.link_playing = false;
                    commit = true;
                }
                self.peer_started = false;
            }
            _ => {}
        }
        if commit {
            self.link.commit_audio_session_state(&self.state);
        }
    }

    /// Hold at beat zero until the next buffer maps this restart's launch.
    pub(crate) fn restart(&mut self) {
        self.awaiting_launch = true;
    }

    /// The next sample's place on the timeline: `None` while a launch waits
    /// for its downbeat, otherwise the Link beat and tempo.
    pub(crate) fn next_sample(&mut self) -> LinkSample {
        let offset = (self.samples_into_buffer as f64 * self.micros_per_sample).round() as i64;
        self.samples_into_buffer += 1;
        let beat = self
            .state
            .beat_at_time(self.buffer_start_micros + offset, LINK_QUANTUM);
        LinkSample {
            beat: (!self.awaiting_launch && beat >= 0.0).then_some(beat),
            bpm: self.state.tempo(),
        }
    }
}

/// What the engine last published about its own transport and tempo target.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LocalTiming {
    pub(crate) transport: Transport,
    pub(crate) restart: u64,
    pub(crate) target_bpm: f32,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct LinkSample {
    /// `None` while a launch waits for its downbeat.
    pub(crate) beat: Option<f64>,
    pub(crate) bpm: f64,
}

#[cfg(test)]
impl LinkSession {
    /// A session that never opens the network, for deterministic peers.
    pub(crate) fn offline(bpm: f32) -> Self {
        let link = Arc::new(AblLink::new(f64::from(bpm)));
        link.enable_start_stop_sync(true);
        Self {
            link,
            changes: Arc::new(LinkPeerChanges::default()),
        }
    }

    pub(crate) fn timeline(&self) -> SessionState {
        let mut state = SessionState::new();
        self.link.capture_app_session_state(&mut state);
        state
    }
}
