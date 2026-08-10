//! One module per voice, plus the helpers they share: pitch conversion,
//! soft clipping, normalized LFO shaping, and voice-pool mixing.

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

/// Shared `filter` control -> noise lowpass smoothing coefficient mapping
/// used by Perc and Clap's noise-based hits (Kick's filter curve is
/// different and stays local to `kick.rs`).
#[inline]
pub(crate) fn noise_filter_smoothing(filter: f32) -> f32 {
    10_f32.powf(filter * 4.0 - 4.0)
}

/// Shared sum-then-cull idiom: advance every voice (in order), accumulating
/// its stereo output, then drop whichever voices are finished. Voice order
/// and the summation order are unchanged from the equivalent hand-written
/// loop, so this is a pure extraction.
#[inline]
pub(crate) fn mix_and_retain<V>(
    voices: &mut Vec<V>,
    mut next: impl FnMut(&mut V) -> (f32, f32),
    done: impl Fn(&V) -> bool,
) -> (f32, f32) {
    let mut dry_l = 0.0f32;
    let mut dry_r = 0.0f32;
    for v in voices.iter_mut() {
        let (l, r) = next(v);
        dry_l += l;
        dry_r += r;
    }
    voices.retain(|v| !done(v));
    (dry_l, dry_r)
}
