//! The nooise engine.
//!
//! This root wires the pieces together for each entry point: `run` and
//! `run_auto` for live audio plus TUI, `render_wav` for a headless render. It
//! also owns `FluidTelemetry`, the lock-free audio-to-UI counters the
//! visualizer animates from.

use std::error::Error;
use std::f32::consts::TAU;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
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
    widgets::{Block, Borders, Paragraph, Widget},
};

use crate::audio::{self, StereoEngine};
use crate::fx::lfo::DriftingLfo;
use crate::fx::panner::StereoPanner;
use crate::synth::envelope::Adsr;
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
mod interaction;
mod module;
mod palette;
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

pub(crate) use auto::{
    AutoControls, DEFAULT_AUTO_BARS, MorphState, MorphWriter, decode_auto_states, no_morph,
};
use automation::*;
use controls::*;
use coordinator::*;
use edit::*;
use effect::*;
use engine::*;
use module::*;
use palette::*;
use registry::*;
use session::*;
pub(crate) use song::{SongState, decode_song_code, encode_song_code};
use ui::*;
use view::*;
use visualizer::*;
use voice::*;

// ============================================================
// Telemetry — audio thread publishes, UI thread reads
// ============================================================

/// Lock-free channel from the engine to the visualizer. The audio thread only
/// ever stores; the UI thread only ever loads. `kick_pulse` is a monotonic
/// counter (UI tracks the delta to fire one ripple per hit); `chord_index`
/// mirrors the pad engine's current chord.
#[derive(Default)]
pub(crate) struct FluidTelemetry {
    pub(crate) chord_index: AtomicU64,
    pub(crate) kick_pulse: AtomicU64,
    /// Engine beat position as `f64::to_bits`, for beat-synced UI animation.
    pub(crate) beat_bits: AtomicU64,
}

impl FluidTelemetry {
    pub(crate) fn publish_beat(&self, beat: f64) {
        self.beat_bits.store(beat.to_bits(), Ordering::Relaxed);
    }

    pub(crate) fn beat(&self) -> f64 {
        f64::from_bits(self.beat_bits.load(Ordering::Relaxed))
    }
}

// ============================================================
// Entry point
// ============================================================

const APP_ID: &str = "nooise";

pub(crate) fn run() -> Result<(), Box<dyn Error>> {
    run_with_song_state(SongState::from_controls(FluidControls::default()))
}

pub(crate) fn run_with_song_state(initial_song: SongState) -> Result<(), Box<dyn Error>> {
    // Interactive start: no morph running. `A` can begin one live, heading
    // toward the built-in states from wherever the user currently is.
    let auto_states = decode_auto_states();
    run_interactive(initial_song, no_morph(), auto_states, DEFAULT_AUTO_BARS)
}

/// Run the live interactive TUI already morphing forever between the built-in
/// `AUTO_STATES` over `bars`-bar legs (`nooise auto [BARS]`). `A` toggles it off
/// — as does touching any parameter — and back on from the current state.
pub(crate) fn run_auto(bars: u32) -> Result<(), Box<dyn Error>> {
    let states = decode_auto_states();
    let initial_song = states[0].clone();
    let morph = Arc::new(ArcSwap::from_pointee(Some(MorphState::new(
        states.clone(),
        bars,
    ))));
    run_interactive(initial_song, morph, states, bars)
}

/// Shared interactive setup: wire the audio engine, terminal, and UI loop
/// around the aggregate live session, telemetry, and morph state. `morph` starts
/// `Some` for `nooise auto` and `None` otherwise; `auto_states`/`auto_bars` let
/// the UI build a fresh morph when the user toggles auto mode on live.
fn run_interactive(
    initial_song: SongState,
    morph: Arc<ArcSwap<Option<MorphState>>>,
    auto_states: Vec<SongState>,
    auto_bars: u32,
) -> Result<(), Box<dyn Error>> {
    let session = LiveSession::new(LiveSessionSnapshot::from_song(&initial_song));
    let session_for_engine = session.clone();
    let morph_for_engine = Arc::clone(&morph);
    let telemetry = Arc::new(FluidTelemetry::default());
    let telemetry_for_engine = Arc::clone(&telemetry);
    let updates = UpdateNotice::default();
    spawn_update_check(updates.clone());

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
