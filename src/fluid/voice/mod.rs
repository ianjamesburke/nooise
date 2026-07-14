use super::*;

mod arp;
mod bass;
mod clap;
mod kick;
mod pad;
mod perc;
mod tonal;

pub(crate) use arp::*;
pub(crate) use bass::*;
pub(crate) use clap::*;
pub(crate) use kick::*;
pub(crate) use pad::*;
pub(crate) use perc::*;
pub(crate) use tonal::*;

// ============================================================
// Shared voice utilities
// ============================================================

pub(crate) fn midi_to_hz(note: i32) -> f32 {
    440.0 * 2f32.powf((note as f32 - 69.0) / 12.0)
}

/// Frequency multiplier for a master tune offset in semitones.
pub(crate) fn tune_ratio(semitones: f32) -> f32 {
    2f32.powf(semitones / 12.0)
}

pub(crate) fn normalized_lfo(sample: f32) -> f32 {
    (sample * 0.5 + 0.5).clamp(0.0, 1.0)
}

pub(crate) fn soft_clip(sample: f32) -> f32 {
    sample / (1.0 + sample.abs())
}
