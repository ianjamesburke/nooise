//! One module per voice, plus the helpers they share: pitch conversion,
//! soft clipping, normalized LFO shaping, and voice-pool mixing.

use super::*;

mod arp;
mod bass;
mod chord;
mod clap;
mod kick;
mod lead;
mod pad;
mod perc;
mod tonal;

pub(crate) use arp::*;
pub(crate) use bass::*;
pub(crate) use chord::*;
pub(crate) use clap::*;
pub(crate) use kick::*;
pub(crate) use lead::*;
pub(crate) use pad::*;
pub(crate) use perc::*;
pub(crate) use tonal::*;

// ============================================================
// Shared voice utilities
// ============================================================

/// Pattern position comes from the transport, so editing a loop length never
/// leaves a voice in a private phase. The slack covers swung grid hits.
pub(crate) fn lane_step_at(beat: f64, rate_beats: f32, offset_beats: f32, count: usize) -> usize {
    let position = (beat - offset_beats as f64) / rate_beats as f64 + 0.25;
    (position.floor() as i64).rem_euclid(count.max(1) as i64) as usize
}

pub(crate) fn midi_to_hz(note: i32) -> f32 {
    440.0 * 2f32.powf((note as f32 - 69.0) / 12.0)
}

/// The Master Key control: each progression keeps its authored key (`Home`),
/// or every progression is moved into one chosen pitch class. The shift is
/// resolved per progression, at the moment a voice builds a note, so a morph
/// or a progression change lands in the key without a click or a stale shift.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum MasterKey {
    #[default]
    Home,
    /// A pitch class, C = 0 through B = 11.
    Pitch(u8),
}

impl MasterKey {
    /// `master.key`'s dial: 0 is Home, 1 through 12 are C through B.
    pub(crate) const DIAL_MAX: f32 = 12.0;

    pub(crate) fn from_control(value: f32) -> Self {
        match value.round().clamp(0.0, Self::DIAL_MAX) as u8 {
            0 => Self::Home,
            step => Self::Pitch(step - 1),
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Home => "Home",
            Self::Pitch(pitch) => chord_pitch_name(i32::from(pitch)),
        }
    }

    /// Semitones that carry `progression`'s authored key into this key by the
    /// shortest way round: -6 through +5, and 0 for Home.
    pub(crate) fn shift(self, progression: usize) -> i32 {
        match self {
            Self::Home => 0,
            Self::Pitch(pitch) => {
                (i32::from(pitch) - progression_tonic_pitch_class(progression) + 6).rem_euclid(12)
                    - 6
            }
        }
    }
}

/// Frequency multiplier for a transposition in semitones.
pub(crate) fn tune_ratio(semitones: f32) -> f32 {
    2f32.powf(semitones / 12.0)
}

/// A MIDI note's frequency under a transposition (`tune`, in semitones, from
/// `MasterKey::shift`): the one pitch path every pitched voice (Pad, Bass,
/// Tonal, Arp, Lead) builds a note through.
pub(crate) fn note_hz(note: i32, tune: f32) -> f32 {
    midi_to_hz(note) * tune_ratio(tune)
}

/// MIDI pitch folded into one octave for visual/event consumers: C is 0,
/// B is 11/12, and octave transposition does not change the value.
pub(crate) fn pitch_class(note: i32) -> f32 {
    note.rem_euclid(12) as f32 / 12.0
}

pub(crate) fn normalized_lfo(sample: f32) -> f32 {
    (sample * 0.5 + 0.5).clamp(0.0, 1.0)
}

pub(crate) fn soft_clip(sample: f32) -> f32 {
    sample / (1.0 + sample.abs())
}

/// Expected mutations per cycle follow `rate^1.5 * max_mutations`, with the
/// fractional part resolved deterministically from `seed` so low evolve rates
/// only mutate occasionally instead of forcing a step change every cycle.
pub(crate) fn evolve_mutation_count(rate: f32, max_mutations: usize, seed: u64) -> usize {
    let rate = rate.clamp(0.0, 1.0);
    if rate <= 0.0 || max_mutations == 0 {
        return 0;
    }
    let expected = rate.powf(1.5) * max_mutations as f32;
    let guaranteed = expected.floor() as usize;
    let frac = expected - guaranteed as f32;
    let extra = if frac > 0.0 {
        let unit = (splitmix64_mix(seed) >> 40) as f32 * (1.0 / (1u64 << 24) as f32);
        usize::from(unit < frac)
    } else {
        0
    };
    (guaranteed + extra).min(max_mutations)
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

/// `mix_and_retain` for a mono voice pool.
#[inline]
pub(crate) fn mix_and_retain_mono<V>(
    voices: &mut Vec<V>,
    mut next: impl FnMut(&mut V) -> f32,
    done: impl Fn(&V) -> bool,
) -> f32 {
    let mut dry = 0.0f32;
    for v in voices.iter_mut() {
        dry += next(v);
    }
    voices.retain(|v| !done(v));
    dry
}
