use std::error::Error;
use std::f32::consts::TAU;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

use arc_swap::ArcSwap;

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};

use crate::audio::{self, StereoEngine};
use crate::fx::lfo::DriftingLfo;
use crate::fx::panner::StereoPanner;
use crate::fx::reverb::Freeverb;
use crate::synth::envelope::Adsr;
use crate::synth::noise::WhiteNoise;
use crate::synth::oscillator::SineOscillator;
use crate::update_check::{UpdateNotice, spawn_update_check};

mod automation;
mod controls;
mod engine;
mod registry;
mod song;
mod ui;
mod voice;

#[cfg(test)]
mod tests;

use automation::*;
use controls::*;
use engine::*;
use registry::*;
pub(crate) use song::{SongState, decode_song_code, launch_line};
use ui::*;
use voice::*;

// ============================================================
// Telemetry — audio thread publishes, UI thread reads
// ============================================================

/// Lock-free channel from the engine to the visualizer. The audio thread only
/// ever stores (relaxed); the UI thread only ever loads. It carries three kinds
/// of signal, one visual body per sounding voice:
///
/// * Trigger pulses — monotonic counters bumped once per note/hit
///   (`kick_pulse`, `bass_pulse`, `tonal_pulse`, `perc_pulse`, `clap_pulse`).
///   The UI tracks the delta to spawn one wavelet per event.
/// * Pitch — the last bass and tonal note, packed as `f32::to_bits` Hz, so the
///   visualizer can place and tune those voices' waves.
/// * Smoothed output level — one `f32` per voice, an envelope follower the
///   engine updates per sample and publishes at control rate (every
///   `LEVEL_PUBLISH_INTERVAL` samples), never per sample. A silent voice's
///   level falls to zero so its node goes still and dark.
///
/// `chord_index` mirrors the pad engine's current chord (drives pad hue).
#[derive(Default)]
pub(crate) struct FluidTelemetry {
    pub chord_index: AtomicU64,
    pub kick_pulse: AtomicU64,
    pub bass_pulse: AtomicU64,
    pub tonal_pulse: AtomicU64,
    pub perc_pulse: AtomicU64,
    pub clap_pulse: AtomicU64,
    /// Engine beat position as `f64::to_bits`, for beat-synced UI animation.
    pub beat_bits: AtomicU64,
    /// Last bass / tonal note as `f32::to_bits` Hz.
    pub bass_note_bits: AtomicU32,
    pub tonal_note_bits: AtomicU32,
    /// Per-voice smoothed output level as `f32::to_bits`.
    pub bass_level_bits: AtomicU32,
    pub pad_level_bits: AtomicU32,
    pub kick_level_bits: AtomicU32,
    pub tonal_level_bits: AtomicU32,
    pub perc_level_bits: AtomicU32,
    pub clap_level_bits: AtomicU32,
}

/// Smoothed per-voice output levels, published together at control rate.
#[derive(Default, Clone, Copy)]
pub(crate) struct VoiceLevels {
    pub bass: f32,
    pub pad: f32,
    pub kick: f32,
    pub tonal: f32,
    pub perc: f32,
    pub clap: f32,
}

impl FluidTelemetry {
    pub(crate) fn publish_beat(&self, beat: f64) {
        self.beat_bits.store(beat.to_bits(), Ordering::Relaxed);
    }

    pub(crate) fn beat(&self) -> f64 {
        f64::from_bits(self.beat_bits.load(Ordering::Relaxed))
    }

    pub(crate) fn publish_bass_note(&self, hz: f32) {
        self.bass_note_bits.store(hz.to_bits(), Ordering::Relaxed);
    }

    pub(crate) fn bass_note_hz(&self) -> f32 {
        f32::from_bits(self.bass_note_bits.load(Ordering::Relaxed))
    }

    pub(crate) fn publish_tonal_note(&self, hz: f32) {
        self.tonal_note_bits.store(hz.to_bits(), Ordering::Relaxed);
    }

    pub(crate) fn tonal_note_hz(&self) -> f32 {
        f32::from_bits(self.tonal_note_bits.load(Ordering::Relaxed))
    }

    pub(crate) fn publish_levels(&self, levels: VoiceLevels) {
        self.bass_level_bits
            .store(levels.bass.to_bits(), Ordering::Relaxed);
        self.pad_level_bits
            .store(levels.pad.to_bits(), Ordering::Relaxed);
        self.kick_level_bits
            .store(levels.kick.to_bits(), Ordering::Relaxed);
        self.tonal_level_bits
            .store(levels.tonal.to_bits(), Ordering::Relaxed);
        self.perc_level_bits
            .store(levels.perc.to_bits(), Ordering::Relaxed);
        self.clap_level_bits
            .store(levels.clap.to_bits(), Ordering::Relaxed);
    }

    pub(crate) fn levels(&self) -> VoiceLevels {
        VoiceLevels {
            bass: f32::from_bits(self.bass_level_bits.load(Ordering::Relaxed)),
            pad: f32::from_bits(self.pad_level_bits.load(Ordering::Relaxed)),
            kick: f32::from_bits(self.kick_level_bits.load(Ordering::Relaxed)),
            tonal: f32::from_bits(self.tonal_level_bits.load(Ordering::Relaxed)),
            perc: f32::from_bits(self.perc_level_bits.load(Ordering::Relaxed)),
            clap: f32::from_bits(self.clap_level_bits.load(Ordering::Relaxed)),
        }
    }
}

// ============================================================
// Entry point
// ============================================================

const APP_ID: &str = "nooise";

pub(crate) fn run() -> Result<(), Box<dyn Error>> {
    run_with_controls(FluidControls::default())
}

pub(crate) fn run_with_controls(initial_controls: FluidControls) -> Result<(), Box<dyn Error>> {
    run_with_song_state(SongState::from_controls(initial_controls))
}

pub(crate) fn run_with_song_state(initial_song: SongState) -> Result<(), Box<dyn Error>> {
    let controls = Arc::new(ArcSwap::from_pointee(initial_song.controls));
    let controls_for_engine = Arc::clone(&controls);
    let automation = Arc::new(ArcSwap::from_pointee(initial_song.automation.clone()));
    let automation_for_engine = Arc::clone(&automation);
    let telemetry = Arc::new(FluidTelemetry::default());
    let telemetry_for_engine = Arc::clone(&telemetry);
    let updates = UpdateNotice::default();
    spawn_update_check(updates.clone());

    let _stream = audio::start_stream(APP_ID, move |sr| {
        FluidEngine::new(
            sr,
            controls_for_engine,
            automation_for_engine,
            telemetry_for_engine,
        )
    })?;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = ui_loop(
        &mut terminal,
        controls,
        automation,
        telemetry,
        initial_song.automation,
        updates,
    );

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    result
}

/// Render the default mix to a wav file without a terminal or audio device.
pub(crate) fn render_wav(
    seconds: f32,
    out: &std::path::Path,
    seed: Option<u64>,
) -> Result<(), Box<dyn Error>> {
    const RENDER_SAMPLE_RATE: u32 = 44_100;

    let controls = Arc::new(ArcSwap::from_pointee(FluidControls::default()));
    let automation = Arc::new(ArcSwap::from_pointee(AutomationState::default()));
    let telemetry = Arc::new(FluidTelemetry::default());
    let mut engine = FluidEngine::new(RENDER_SAMPLE_RATE as f32, controls, automation, telemetry);
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
