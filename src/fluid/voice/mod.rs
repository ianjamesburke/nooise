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

/// Soft-clip drive stage shared by the bass voices and the kick: boost by
/// `1 + drive * 8`, saturate, then restore presence with `1 + drive * 0.5`.
/// A drive of 0 passes the sample through untouched.
pub(crate) fn drive_stage(sample: f32, drive: f32) -> f32 {
    if drive > 0.0 {
        let driven = sample * (1.0 + drive * 8.0);
        driven / (1.0 + driven.abs()) * (1.0 + drive * 0.5)
    } else {
        sample
    }
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
