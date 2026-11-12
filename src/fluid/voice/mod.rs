//! One module per voice, plus the helpers they share: pitch conversion,
//! soft clipping, normalized LFO shaping, and voice-pool mixing.

use super::*;

mod arp;
mod bass;
mod cell;
mod chord;
mod clap;
mod evolve;
mod harmony;
mod kick;
mod lead;
mod mode_map;
mod pad;
mod perc;
mod sequence;
mod tonal;
mod triad;

pub(crate) use arp::*;
pub(crate) use bass::*;
pub(crate) use cell::*;
pub(crate) use chord::*;
pub(crate) use clap::*;
pub(crate) use evolve::*;
pub(crate) use harmony::*;
pub(crate) use kick::*;
pub(crate) use lead::*;
pub(crate) use mode_map::*;
pub(crate) use pad::*;
pub(crate) use perc::*;
pub(crate) use sequence::*;
pub(crate) use tonal::*;
pub(crate) use triad::*;

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

/// The Master Key and Master Mode: the one concrete key every progression
/// plays in. A progression is relative to it: its chords are re-read in this
/// mode (`ModeMap`), then carried to this tonic (`shift`). Both are resolved
/// per progression at the moment a voice builds a note, so a morph or a
/// progression change lands in the key without a click or a stale shift.
/// Launch and song loading derive it from the progression
/// (`of_progression`), so nothing sounds different until it is changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MasterKey {
    /// A pitch class, C = 0 through B = 11.
    pitch: u8,
    pub(crate) mode: HomeMode,
}

/// `master.key`'s song-code value at each dial position, C through B. A code
/// stores 1 for C because 0 was the retired Home key; a new code never
/// writes 0, so an old code's chosen key still means the same pitch.
pub(crate) const MASTER_KEY_SONG_VALUES: [i8; 12] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];

impl MasterKey {
    /// `master.key`'s dial: 0 through 11 are C through B.
    pub(crate) const DIAL_MAX: f32 = 11.0;

    pub(crate) const fn new(pitch: i32, mode: HomeMode) -> Self {
        Self {
            pitch: pitch.rem_euclid(12) as u8,
            mode,
        }
    }

    pub(crate) fn from_controls(master: &MasterControls) -> Self {
        Self::new(
            master.key.round().clamp(0.0, Self::DIAL_MAX) as i32,
            master_mode(master.mode).1,
        )
    }

    /// The key `progression` is authored in: its tonic and its own mode.
    pub(crate) fn of_progression(progression: usize) -> Self {
        Self::new(
            progression_tonic_pitch_class(progression),
            progression_home_mode(progression),
        )
    }

    pub(crate) fn pitch(self) -> i32 {
        i32::from(self.pitch)
    }

    /// The `master.key` and `master.mode` dial values that name this key.
    pub(crate) fn dial_values(self) -> (f32, f32) {
        let mode = MASTER_MODES
            .iter()
            .position(|(_, mode)| *mode == self.mode)
            .expect("every home mode is a Master Mode");
        (f32::from(self.pitch), mode as f32)
    }

    /// The tonic spelled as its key signature spells it: sharps when the
    /// mode's parent major scale is a sharp key, else flats (Db major, C#
    /// minor, Bb minor, G# minor).
    pub(crate) fn pitch_name(self) -> &'static str {
        const SHARPS: [&str; 12] = [
            "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
        ];
        const FLATS: [&str; 12] = [
            "C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B",
        ];
        // The mode's tonic is this degree of its parent major scale.
        let parent = (self.pitch()
            - HomeMode::Major.intervals()[match self.mode {
                HomeMode::Major => 0,
                HomeMode::Dorian => 1,
                HomeMode::Phrygian => 2,
                HomeMode::Mixolydian => 4,
                HomeMode::Minor => 5,
            }])
        .rem_euclid(12);
        let names = if [7, 2, 9, 4, 11].contains(&parent) {
            SHARPS
        } else {
            FLATS
        };
        names[self.pitch() as usize]
    }

    /// Written as a chord symbol for major and minor ("A", "Am") and by name
    /// for the other modes ("A Dorian").
    pub(crate) fn label(self) -> String {
        let tonic = self.pitch_name();
        match self.mode {
            HomeMode::Major => tonic.to_string(),
            HomeMode::Minor => format!("{tonic}m"),
            mode => format!("{tonic} {}", mode.label()),
        }
    }

    /// Semitones that carry `progression`'s authored tonic to this one by
    /// the shortest way round: -6 through +5.
    pub(crate) fn shift(self, progression: usize) -> i32 {
        (self.pitch() - progression_tonic_pitch_class(progression) + 6).rem_euclid(12) - 6
    }

    /// How `progression`'s chords are re-read in this mode.
    pub(crate) fn mode_map(self, progression: usize) -> ModeMap {
        ModeMap::new(progression, self.mode)
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
