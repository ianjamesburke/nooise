//! The v3 chord model: harmony and voicing as independent data. Harmony is
//! what the chord *is* (a root, a quality, tensions); a voicing is *how* its
//! tones are stacked. `realize` is the single path from the two to MIDI notes,
//! so every voice and the editor share one resolver.
//!
//! Built-in content still arrives through `chord_from_slot`, which reads the
//! legacy eight-field controls and expresses them as these types; the editor
//! authors them directly in a later stage. This is not a song-code migration:
//! nothing here touches the wire format.

use super::*;

/// A semitone interval above the chord root, 0 through 11.
pub(crate) type Interval = i8;

/// The triad family. `Scale` keeps the editor's diatonic third in the
/// established A-minor root coordinates so legacy content is exact; new
/// authoring uses the explicit choices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Quality {
    Major,
    Minor,
    Sus2,
    Sus4,
    Power,
    Scale,
}

impl Quality {
    /// The third (or suspension) interval above the root; `Power` has none.
    /// `Scale` reads the third the editor's diatonic lattice gives `degree`.
    pub(crate) fn third(self, degree: i8) -> Option<Interval> {
        Some(match self {
            Self::Minor => 3,
            Self::Major => 4,
            Self::Sus2 => 2,
            Self::Sus4 => 5,
            Self::Power => return None,
            Self::Scale => {
                let root = shift_diatonic(45, i32::from(degree));
                (shift_diatonic(root, 2) - root) as Interval
            }
        })
    }
}

/// A chord. The root keeps the established editor coordinates; the quality
/// picks the third; tensions are resolved semitone intervals above the root;
/// `fifth` and `bass` are optional so a voicing may omit or slash an interval.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Harmony {
    pub(crate) root_degree: i8,
    pub(crate) accidental: i8,
    pub(crate) quality: Quality,
    pub(crate) tensions: [Option<Interval>; 2],
    pub(crate) fifth: Option<Interval>,
    /// A slash bass that is not a chord tone ("Dm/G").
    pub(crate) bass: Option<Interval>,
}

impl Harmony {
    pub(crate) fn root_note(self) -> i32 {
        shift_diatonic(45, i32::from(self.root_degree)) + i32::from(self.accidental)
    }

    /// The chord's available pitch classes above the root, root first, capped
    /// at the four source roles the engine reserves. The cap drops the fifth
    /// before a tension, matching the legacy source-order.
    pub(crate) fn intervals(self) -> ([Interval; 4], usize) {
        let mut roles = [0; 4];
        let mut count = 0;
        for role in [
            Some(0),
            self.quality.third(self.root_degree),
            self.tensions[0],
            self.tensions[1],
            self.fifth,
        ]
        .into_iter()
        .flatten()
        {
            if !roles[..count]
                .iter()
                .any(|n| (*n - role).rem_euclid(12) == 0)
            {
                roles[count] = role;
                count += 1;
                if count == 4 {
                    break;
                }
            }
        }
        (roles, count)
    }

    /// Whether `interval` is a chord tone, or the explicit slash bass.
    pub(crate) fn contains(self, interval: Interval) -> bool {
        let (roles, count) = self.intervals();
        roles[..count]
            .iter()
            .any(|role| role.rem_euclid(12) == interval.rem_euclid(12))
            || self
                .bass
                .is_some_and(|bass| bass.rem_euclid(12) == interval.rem_euclid(12))
    }
}

/// One note of a voicing: a chord interval and the octave it sits in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Voice {
    pub(crate) interval: Interval,
    pub(crate) octave: i8,
}

/// How a harmony's tones are stacked. Simple data: a voice names a chord
/// interval and the octave it sits in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Voicing {
    pub(crate) voices: [Voice; 4],
}

/// The one path from a harmony and voicing to MIDI notes, ascending.
pub(crate) fn realize(harmony: &Harmony, voicing: &Voicing) -> [i32; 4] {
    let root = harmony.root_note();
    let mut notes = [0; 4];
    for (note, voice) in notes.iter_mut().zip(voicing.voices) {
        *note = root + i32::from(voice.interval) + 12 * i32::from(voice.octave);
    }
    notes.sort_unstable();
    notes
}

/// Express a legacy chord slot as the v3 types. Voices are derived from the
/// slot's resolved notes, so the model carries every existing chord exactly;
/// an editor will author `Voicing` directly.
pub(crate) fn chord_from_slot(slot: &ChordSlotControls) -> (Harmony, Voicing) {
    let quality = match slot.quality.round() as i32 {
        -1 => Quality::Minor,
        2 => Quality::Sus2,
        3 => Quality::Sus4,
        4 => Quality::Power,
        0 => Quality::Scale,
        _ => Quality::Major,
    };
    let third = third_interval(slot);
    let fifth = chord_fifth_interval(slot);
    let (roles, count) = source_intervals(slot);
    let mut tensions = [None; 2];
    let mut next = 0;
    for role in roles[..count].iter().copied() {
        if role == 0 || Some(role) == third || Some(role) == fifth {
            continue;
        }
        if next < 2 {
            tensions[next] = Some(role as Interval);
            next += 1;
        }
    }
    let root = pad_chord_root_note(slot);
    let notes = legacy_chord_notes(slot);
    let mut voices = [Voice {
        interval: 0,
        octave: 0,
    }; 4];
    for (voice, note) in voices.iter_mut().zip(notes) {
        let interval = (note - root).rem_euclid(12) as Interval;
        voice.interval = interval;
        voice.octave = ((note - root - i32::from(interval)) / 12) as i8;
    }
    let mut harmony = Harmony {
        root_degree: slot.degree.round() as i8,
        accidental: slot.accidental.round() as i8,
        quality,
        tensions,
        fifth: fifth.map(|f| f as Interval),
        bass: None,
    };
    // A lowest voice outside the chord tones is an explicit slash bass.
    if !harmony.contains(voices[0].interval) {
        harmony.bass = Some(voices[0].interval);
    }
    (harmony, Voicing { voices })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_v3_model_carries_every_authored_chord_exactly() {
        for progression in &PROGRESSIONS {
            for chord in &progression.chords {
                let (harmony, voicing) = chord_from_slot(&chord.preset);
                assert_eq!(
                    realize(&harmony, &voicing),
                    legacy_chord_notes(&chord.preset),
                    "{}",
                    chord.name
                );
            }
        }
    }

    #[test]
    fn harmony_intervals_match_the_legacy_source_roles() {
        for progression in &PROGRESSIONS {
            for chord in &progression.chords {
                let (harmony, _) = chord_from_slot(&chord.preset);
                let (expected, expected_count) = source_intervals(&chord.preset);
                let (roles, count) = harmony.intervals();
                let expected: Vec<Interval> = expected[..expected_count]
                    .iter()
                    .map(|r| *r as Interval)
                    .collect();
                assert_eq!(&roles[..count], expected.as_slice(), "{}", chord.name);
            }
        }
    }
}
