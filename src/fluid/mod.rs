//! The nooise engine.
//!
//! This root wires the pieces together for each entry point: `run` and
//! `run_auto` for live audio plus TUI, `render_wav` for a headless render. It
//! also owns `FluidTelemetry`, the lock-free audio-to-UI counters the
//! visualizer animates from.

use std::error::Error;
use std::f32::consts::TAU;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

use arc_swap::ArcSwap;

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
#[cfg(test)]
use ratatui::Terminal;
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Padding, Paragraph, Widget},
};

use crate::audio::{self, StereoEngine};
use crate::fx::lfo::DriftingLfo;
use crate::fx::panner::StereoPanner;
use crate::synth::envelope::Adsr;
use crate::synth::fm::{FmPair, FmStack, FmWave};
use crate::synth::noise::WhiteNoise;
use crate::synth::oscillator::SineOscillator;
use crate::update_check::{UpdateNotice, spawn_update_check};

mod auto;
mod automation;
mod controls;
mod coordinator;
mod edit;
mod effect;
mod engine;
mod gesture;
mod interaction;
mod module;
mod osc;
mod palette;
mod range_epoch;
mod registry;
#[cfg(test)]
mod replay;
mod runtime;
mod session;
mod song;
mod song_ids;
mod ui;
mod view;
mod visualizer;
mod voice;
mod widget;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod gesture_audio_tests;

#[cfg(test)]
mod gesture_level_probe;

pub(crate) use auto::{
    AutoControls, DEFAULT_AUTO_BARS, MorphPosition, MorphState, MorphWriter, decode_auto_states,
    no_morph,
};
use automation::*;
use controls::*;
use coordinator::*;
use edit::*;
use effect::*;
use engine::*;
use gesture::*;
use module::*;
use palette::*;
use registry::*;
use session::*;
pub(crate) use song::{CODE_PREFIX, SongState, decode_song_code, encode_song_code};
use ui::*;
use view::*;
use visualizer::*;
use voice::*;

// ============================================================
// Shared numeric helpers
// ============================================================

/// Hermite ease: 0 at 0, 1 at 1, flat at both ends. The one curve every
/// click-free ramp in the engine and every LFO glide is shaped by.
pub(crate) fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The splitmix64 finaliser: a stateless bit mixer every seeded, RNG-free
/// value in the engine derives from, so the UI and the audio thread agree and
/// offline renders stay identical. Callers fold their own inputs into `z`.
pub(crate) fn splitmix64_mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

// ============================================================
// Telemetry — audio thread publishes, UI thread reads
// ============================================================

/// A musical onset that OSC mirrors as a discrete hit. The order is stable:
/// it indexes the telemetry arrays, not a UI or registry table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MusicalHit {
    Beat,
    Pad,
    Perc,
    Kick,
    Tonal,
    Clap,
    Arp,
}

impl MusicalHit {
    pub(crate) const ALL: [Self; 7] = [
        Self::Beat,
        Self::Pad,
        Self::Perc,
        Self::Kick,
        Self::Tonal,
        Self::Clap,
        Self::Arp,
    ];
}

pub(crate) const MUSICAL_HIT_COUNT: usize = MusicalHit::ALL.len();
/// Pitch-class sentinel for an unpitched onset.
pub(crate) const NO_PITCH_CLASS: f32 = -1.0;

/// The latest payload of one monotonically counted musical-onset stream.
/// Payload stores happen before `pulse` advances, so a reader that observes a
/// new pulse also observes the event's level and pitch class.
pub(crate) struct HitTelemetry {
    pub(crate) pulse: AtomicU64,
    pub(crate) level_bits: AtomicU32,
    pub(crate) pitch_class_bits: AtomicU32,
}

impl Default for HitTelemetry {
    fn default() -> Self {
        Self {
            pulse: AtomicU64::new(0),
            level_bits: AtomicU32::new(0.0f32.to_bits()),
            pitch_class_bits: AtomicU32::new(NO_PITCH_CLASS.to_bits()),
        }
    }
}

/// Lock-free channel from the engine to visual consumers. The audio thread
/// only ever stores; readers only ever load. `kick_pulse` remains the
/// visualizer's dedicated ripple counter; `hits` mirrors every musical onset
/// for OSC.
pub(crate) struct FluidTelemetry {
    pub(crate) chord_slot: AtomicU64,
    pub(crate) kick_pulse: AtomicU64,
    /// Engine beat position as `f64::to_bits`, for beat-synced UI animation.
    pub(crate) beat_bits: AtomicU64,
    /// `kick.level` at the latest hit (`f32::to_bits`), stored before
    /// `kick_pulse` advances so a reader that sees the hit sees its level.
    pub(crate) kick_level_bits: AtomicU32,
    pub(crate) hits: [HitTelemetry; MUSICAL_HIT_COUNT],
    /// Pad attack and release seconds (`f32::to_bits`) of the layer voiced by
    /// the latest chord change, stored before `chord_slot` is written.
    pub(crate) chord_attack_bits: AtomicU32,
    pub(crate) chord_release_bits: AtomicU32,
    /// Root pitch class of the latest chord onset (`f32::to_bits`).
    pub(crate) chord_root_pitch_class_bits: AtomicU32,
    /// Per-`Tab` output RMS over the latest `LEVEL_BLOCK` frames
    /// (`f32::to_bits`), each voice as it enters the mix (post effects, mute,
    /// and mix weight) and `Tab::Master` as the final output: what is actually
    /// audible, so consumers can tell silence from tempo and see every layer.
    pub(crate) level_bits: [AtomicU32; TAB_COUNT],
    /// Per-`GestureKind` Master-bus amount (`f32::to_bits`), published each
    /// `LEVEL_BLOCK` frames so a consumer can mirror a held gesture.
    pub(crate) gesture_bits: [AtomicU32; GESTURE_COUNT],
}

impl Default for FluidTelemetry {
    fn default() -> Self {
        Self {
            chord_slot: AtomicU64::new(0),
            kick_pulse: AtomicU64::new(0),
            beat_bits: AtomicU64::new(0.0f64.to_bits()),
            kick_level_bits: AtomicU32::new(0.0f32.to_bits()),
            hits: std::array::from_fn(|_| HitTelemetry::default()),
            chord_attack_bits: AtomicU32::new(0.0f32.to_bits()),
            chord_release_bits: AtomicU32::new(0.0f32.to_bits()),
            chord_root_pitch_class_bits: AtomicU32::new(0.0f32.to_bits()),
            level_bits: std::array::from_fn(|_| AtomicU32::new(0.0f32.to_bits())),
            gesture_bits: std::array::from_fn(|_| AtomicU32::new(0.0f32.to_bits())),
        }
    }
}

/// Frames per level measurement (~5.8 ms at 44.1 kHz).
pub(crate) const LEVEL_BLOCK: u64 = 256;

impl FluidTelemetry {
    pub(crate) fn publish_beat(&self, beat: f64) {
        self.beat_bits.store(beat.to_bits(), Ordering::Relaxed);
    }

    pub(crate) fn beat(&self) -> f64 {
        f64::from_bits(self.beat_bits.load(Ordering::Relaxed))
    }

    pub(crate) fn publish_kick(&self, level: f32) {
        self.kick_level_bits
            .store(level.to_bits(), Ordering::Relaxed);
        self.kick_pulse.fetch_add(1, Ordering::Release);
        self.publish_hit(MusicalHit::Kick, level, NO_PITCH_CLASS);
    }

    pub(crate) fn publish_hit(&self, hit: MusicalHit, level: f32, pitch_class: f32) {
        let telemetry = &self.hits[hit as usize];
        telemetry
            .level_bits
            .store(level.to_bits(), Ordering::Relaxed);
        telemetry
            .pitch_class_bits
            .store(pitch_class.to_bits(), Ordering::Relaxed);
        telemetry.pulse.fetch_add(1, Ordering::Release);
    }

    pub(crate) fn publish_chord(
        &self,
        slot: u64,
        root_pitch_class: f32,
        attack_secs: f32,
        release_secs: f32,
    ) {
        self.chord_attack_bits
            .store(attack_secs.to_bits(), Ordering::Relaxed);
        self.chord_release_bits
            .store(release_secs.to_bits(), Ordering::Relaxed);
        self.chord_root_pitch_class_bits
            .store(root_pitch_class.to_bits(), Ordering::Relaxed);
        self.chord_slot.store(slot, Ordering::Release);
    }

    pub(crate) fn publish_level(&self, tab: Tab, rms: f32) {
        self.level_bits[tab as usize].store(rms.to_bits(), Ordering::Relaxed);
    }

    pub(crate) fn publish_gesture(&self, kind: GestureKind, amount: f32) {
        self.gesture_bits[kind as usize].store(amount.to_bits(), Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(crate) fn gesture(&self, kind: GestureKind) -> f32 {
        f32::from_bits(self.gesture_bits[kind as usize].load(Ordering::Relaxed))
    }

    #[cfg(test)]
    pub(crate) fn level(&self, tab: Tab) -> f32 {
        f32::from_bits(self.level_bits[tab as usize].load(Ordering::Relaxed))
    }
}

// ============================================================
// Entry point
// ============================================================

const APP_ID: &str = "nooise";

/// Where a bare `--osc` sends: foorm's default listen address.
pub(crate) const DEFAULT_OSC_TARGET: &str = "127.0.0.1:9000";

pub(crate) fn run(osc: Option<SocketAddr>) -> Result<(), Box<dyn Error>> {
    let mut rng = rand::thread_rng();
    run_with_song_state(randomized_start_song(&mut rng), osc)
}

fn randomized_start_song(rng: &mut impl Rng) -> SongState {
    let mut controls = FluidControls::default();
    controls.pad.progression = rng.gen_range(0..PROGRESSIONS.len()) as f32;
    SongState::from_controls(controls)
}

pub(crate) fn run_with_song_state(
    initial_song: SongState,
    osc: Option<SocketAddr>,
) -> Result<(), Box<dyn Error>> {
    // Interactive start: no morph running. `A` can begin one live, heading
    // toward the built-in states from wherever the user currently is.
    let auto_states = decode_auto_states();
    run_interactive(
        initial_song,
        no_morph(),
        auto_states,
        DEFAULT_AUTO_BARS,
        osc,
    )
}

/// Run the live interactive TUI already morphing forever between the built-in
/// `AUTO_STATES` over `bars`-bar legs (`nooise auto [BARS]`). `A` toggles it off
/// — as does touching any parameter — and back on from the current state.
pub(crate) fn run_auto(bars: u32, osc: Option<SocketAddr>) -> Result<(), Box<dyn Error>> {
    let states = decode_auto_states();
    let initial_song = states[0].clone();
    let morph = Arc::new(ArcSwap::from_pointee(Some(MorphState::new(
        states.clone(),
        bars,
    ))));
    run_interactive(initial_song, morph, states, bars, osc)
}

/// Play built-in songs by number (`nooise 9`, `nooise 9,10,11`). One song
/// holds; several morph through in the order given and loop. Numbers are
/// one-based to match the AUTO footer, and an unknown one is an error rather
/// than a silent skip — a mistyped song should say so, not quietly play
/// something else.
pub(crate) fn run_songs(
    numbers: &[usize],
    bars: u32,
    osc: Option<SocketAddr>,
) -> Result<(), Box<dyn Error>> {
    let all = decode_auto_states();
    if numbers.is_empty() {
        return Err("expected at least one song".into());
    }
    let mut chosen = Vec::with_capacity(numbers.len());
    for &number in numbers {
        match all.get(number.wrapping_sub(1)) {
            Some(song) if number >= 1 => chosen.push(song.clone()),
            _ => {
                return Err(format!(
                    "there is no song {number}; the built-ins are 1..={}",
                    all.len()
                )
                .into());
            }
        }
    }
    let initial_song = chosen[0].clone();
    let morph = Arc::new(ArcSwap::from_pointee(Some(MorphState::labelled(
        chosen.clone(),
        numbers.to_vec(),
        bars,
    ))));
    run_interactive(initial_song, morph, chosen, bars, osc)
}

/// Shared interactive setup: wire the audio engine, terminal, and UI loop
/// around the aggregate live session, telemetry, and morph state. `morph` starts
/// `Some` for `nooise auto` and `None` otherwise; `auto_states`/`auto_bars` let
/// the UI build a fresh morph when the user toggles auto mode on live. `osc`
/// names a UDP target to mirror telemetry to for external visualizers.
fn run_interactive(
    initial_song: SongState,
    morph: Arc<ArcSwap<Option<MorphState>>>,
    auto_states: Vec<SongState>,
    auto_bars: u32,
    osc: Option<SocketAddr>,
) -> Result<(), Box<dyn Error>> {
    let session = LiveSession::new(LiveSessionSnapshot::from_song(&initial_song));
    let session_for_engine = session.clone();
    let morph_for_engine = Arc::clone(&morph);
    let telemetry = Arc::new(FluidTelemetry::default());
    let telemetry_for_engine = Arc::clone(&telemetry);
    let updates = UpdateNotice::default();
    spawn_update_check(updates.clone());
    let _osc_emitter = osc
        .map(|target| osc::OscEmitter::spawn(target, Arc::clone(&telemetry)))
        .transpose()?;

    let _audio_output = audio::start_stream(APP_ID, move |sr| {
        FluidEngine::new_with_tonal_session_state(
            sr,
            session_for_engine.clone(),
            Arc::clone(&morph_for_engine),
            Arc::clone(&telemetry_for_engine),
            true,
        )
    })?;

    let mut terminal = runtime::TerminalSession::enter()?;
    let result = production_ui_loop(
        &mut terminal,
        UiSession {
            live: session.clone(),
        },
        telemetry,
        updates,
        AutoControls::new(morph, auto_states, auto_bars),
    );

    let restore = terminal.restore();
    result?;
    restore?;
    Ok(())
}

/// Render the default mix to a wav file without a terminal or audio device.
pub(crate) fn render_wav(
    seconds: f32,
    out: &std::path::Path,
    seed: Option<u64>,
) -> Result<(), Box<dyn Error>> {
    const RENDER_SAMPLE_RATE: u32 = 44_100;

    let song = SongState::from_controls(FluidControls::default());
    let session = LiveSession::new(LiveSessionSnapshot::from_song(&song));
    let telemetry = Arc::new(FluidTelemetry::default());
    let mut engine = FluidEngine::new(RENDER_SAMPLE_RATE as f32, session, no_morph(), telemetry);
    if let Some(seed) = seed {
        engine.reseed(seed);
    }

    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: RENDER_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(out, spec)
        .map_err(|e| format!("failed to create {}: {e}", out.display()))?;

    let total_frames = (seconds * RENDER_SAMPLE_RATE as f32) as u64;
    for _ in 0..total_frames {
        let (left, right) = engine.next_stereo();
        writer.write_sample((left.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
        writer.write_sample((right.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
    }
    writer
        .finalize()
        .map_err(|e| format!("failed to finalize {}: {e}", out.display()))?;
    println!(
        "rendered {seconds} s ({total_frames} frames) at {RENDER_SAMPLE_RATE} Hz to {}",
        out.display()
    );
    Ok(())
}
