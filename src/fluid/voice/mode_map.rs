//! A progression re-read in the Master Mode. The authored progression data
//! never changes: every chord is resolved through `ModeMap::cell` at the
//! moment a voice or the screen reads it.
//!
//! A diatonic chord (a major or minor third, every sounding note inside the
//! progression's own scale) moves by scale degree: its root, third, fifth and
//! seventh take the new mode's notes on the same degrees, so its quality
//! follows the mode (I-vi-IV-V in major becomes i-VI-iv-v in minor). An upper
//! extension (2nd/9th, 4th/11th, 6th/13th) keeps its interval above the root
//! when the new scale has that note, and is dropped otherwise; its voice takes
//! the nearest chord tone instead. Everything else (sus and power chords, and
//! chords borrowing a note from outside the scale) is kept as authored, so
//! the Master Key only transposes it. The Bass note under a re-read chord
//! moves by degree with it.
//!
//! Coordinates are the progression's own, before the Master Key shift.

use super::*;

/// One progression's authored home and the mode it is re-read in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ModeMap {
    tonic: i32,
    from: HomeMode,
    to: HomeMode,
}

impl ModeMap {
    pub(crate) fn new(progression: usize, to: HomeMode) -> Self {
        Self {
            tonic: progression_tonic_pitch_class(progression),
            from: progression_home_mode(progression),
            to,
        }
    }

    /// The chord `slot` sounds as.
    pub(crate) fn cell(self, slot: &ChordSlotControls) -> Cell {
        let cell = Cell::from_slot(slot);
        self.remap(&cell).unwrap_or(cell)
    }

    /// The four notes `slot` sounds.
    pub(crate) fn notes(self, slot: &ChordSlotControls) -> [i32; 4] {
        self.cell(slot).notes()
    }

    /// The Bass `note` authored under `slot`: moved by degree with a re-read
    /// chord when it sits in the authored scale, else as authored.
    pub(crate) fn bass(self, slot: &ChordSlotControls, note: i32) -> i32 {
        if self.diatonic_root(&Cell::from_slot(slot)).is_none() {
            return note;
        }
        self.degree_in(self.from, note)
            .map_or(note, |degree| note + self.degree_shift(degree))
    }

    /// The 0-based scale degree `note` sits on in `mode` from this tonic.
    fn degree_in(self, mode: HomeMode, note: i32) -> Option<usize> {
        let interval = (note - self.tonic).rem_euclid(12);
        mode.intervals().iter().position(|&step| step == interval)
    }

    /// Semitones scale degree `degree` moves from the authored mode to the
    /// Master Mode (-1, 0 or +1 between any two of the five home modes).
    fn degree_shift(self, degree: usize) -> i32 {
        self.to.intervals()[degree] - self.from.intervals()[degree]
    }

    /// The root's scale degree when `cell` is re-read: a major or minor
    /// third and every sounding note in the authored scale, in another mode.
    fn diatonic_root(self, cell: &Cell) -> Option<usize> {
        if self.from == self.to {
            return None;
        }
        let harmony = cell.harmony;
        if !matches!(harmony.quality.third(harmony.root_degree), Some(3 | 4)) {
            return None;
        }
        let root = self.degree_in(self.from, harmony.root_note())?;
        let (roles, count) = harmony.intervals();
        let tones = roles[..count]
            .iter()
            .map(|role| harmony.root_note() + i32::from(*role))
            .chain(cell.notes());
        for note in tones {
            self.degree_in(self.from, note)?;
        }
        Some(root)
    }

    fn remap(self, cell: &Cell) -> Option<Cell> {
        let root_degree = self.diatonic_root(cell)?;
        let harmony = cell.harmony;
        let root = harmony.root_note();
        let root_shift = self.degree_shift(root_degree);
        let new_root = root + root_shift;
        // An interval above the old root, re-read: a chord tone (3rd, 5th or
        // 7th) moves with its degree; an upper extension keeps its interval
        // above the new root while the new scale has it.
        let moved = |interval: i32| -> Option<i32> {
            let degree = self.degree_in(self.from, root + interval)?;
            if ((degree + 7 - root_degree) % 7) % 2 == 0 {
                Some(interval + self.degree_shift(degree) - root_shift)
            } else {
                self.degree_in(self.to, new_root + interval)
                    .map(|_| interval)
            }
        };
        let third = harmony
            .quality
            .third(harmony.root_degree)
            .map_or(0, i32::from);
        let quality = if moved(third)? == 4 {
            Quality::Major
        } else {
            Quality::Minor
        };
        let fifth = harmony
            .fifth
            .and_then(|fifth| moved(i32::from(fifth)))
            .map(|fifth| fifth as Interval);
        let tensions = harmony
            .tensions
            .map(|tension| tension.and_then(|t| moved(i32::from(t)).map(|t| t as Interval)));
        let mut remapped = Harmony {
            root_degree: harmony.root_degree,
            accidental: harmony.accidental + root_shift as i8,
            quality,
            tensions,
            fifth,
            bass: None,
        };
        // Fixed-size: the audio thread resolves chords and never allocates.
        let chord_tones = [
            Some(0),
            moved(third),
            fifth.map(i32::from),
            tensions[0].map(i32::from),
            tensions[1].map(i32::from),
        ];
        let old_notes = cell.notes();
        let mut notes = [None; 4];
        for (slot, note) in notes.iter_mut().zip(old_notes) {
            *slot = moved(note - root).map(|interval| new_root + interval);
        }
        // A dropped extension's voice takes the nearest chord tone that is
        // not already sounding (ties go down).
        for index in 0..4 {
            if notes[index].is_some() {
                continue;
            }
            let from = old_notes[index];
            let mut best: Option<i32> = None;
            for tone in chord_tones.iter().flatten() {
                for octave in -2..=2 {
                    let candidate =
                        new_root + tone + 12 * (octave + (from - new_root).div_euclid(12));
                    if notes.contains(&Some(candidate)) {
                        continue;
                    }
                    let better = best.is_none_or(|best| {
                        let (distance, best_distance) =
                            ((candidate - from).abs(), (best - from).abs());
                        distance < best_distance || (distance == best_distance && candidate < best)
                    });
                    if better {
                        best = Some(candidate);
                    }
                }
            }
            notes[index] = best;
        }
        let mut notes = notes.map(|note| note.expect("a chord always has a free chord tone"));
        notes.sort_unstable();
        let voicing = Voicing {
            voices: notes.map(|note| {
                let interval = (note - new_root).rem_euclid(12);
                Voice {
                    interval: interval as Interval,
                    octave: ((note - new_root - interval) / 12) as i8,
                }
            }),
        };
        if !remapped.contains(voicing.voices[0].interval) {
            remapped.bass = Some(voicing.voices[0].interval);
        }
        Some(Cell {
            harmony: remapped,
            voicing,
            gate: cell.gate,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(progression: usize, mode: HomeMode) -> Vec<String> {
        let map = ModeMap::new(progression, mode);
        PROGRESSIONS[progression]
            .chords
            .iter()
            .map(|chord| cell_chord_name(&map.cell(&chord.preset), 0))
            .collect()
    }

    fn harvest() -> usize {
        PROGRESSIONS
            .iter()
            .position(|progression| progression.mood == "Harvest")
            .expect("Harvest is built in")
    }

    /// The authored mode leaves every built-in chord, and its Bass, exactly
    /// as written: launch sounds the same.
    #[test]
    fn the_authored_mode_changes_nothing() {
        for (index, progression) in PROGRESSIONS.iter().enumerate() {
            let map = ModeMap::new(index, progression.home_mode);
            for (slot, chord) in progression.chords.iter().enumerate() {
                assert_eq!(map.cell(&chord.preset), Cell::from_slot(&chord.preset));
                let bass = progression.bass[slot];
                assert_eq!(map.bass(&chord.preset, bass), bass);
            }
        }
    }

    #[test]
    fn a_major_progression_in_minor_takes_the_minor_qualities() {
        // Sunny (C major, I-V-vi-IV with sus colours) in C minor.
        let sunny = PROGRESSIONS
            .iter()
            .position(|progression| progression.mood == "Sunny")
            .unwrap();
        assert_eq!(
            named(sunny, HomeMode::Minor),
            ["Cm", "Gsus4", "Ab", "Fm", "Cmadd11", "Gsus4", "Fm", "Cm"]
        );
    }

    #[test]
    fn harvest_in_minor_keeps_its_sus_and_drops_what_the_scale_lacks() {
        assert_eq!(
            named(harvest(), HomeMode::Minor),
            [
                "Fmadd9",
                "Bbmadd9",
                "Fm7",
                "Abmaj7",
                "C#maj7add9",
                "Bbm7",
                "Gm7b5",
                "C7sus4",
            ]
        );
    }

    /// A re-read chord is four distinct notes inside the Master Mode. A chord
    /// tone moves a semitone at most; a dropped extension's voice takes the
    /// nearest free chord tone, within a fourth (G6/B from A Dorian to A
    /// major is G#dim/B: its E becomes the upper G#). A chord left as
    /// authored is unchanged.
    #[test]
    fn re_read_chords_stay_close_distinct_and_in_the_new_scale() {
        for mode in [
            HomeMode::Major,
            HomeMode::Minor,
            HomeMode::Dorian,
            HomeMode::Phrygian,
            HomeMode::Mixolydian,
        ] {
            for (index, progression) in PROGRESSIONS.iter().enumerate() {
                let map = ModeMap::new(index, mode);
                for chord in &progression.chords {
                    let authored = Cell::from_slot(&chord.preset).notes();
                    let notes = map.notes(&chord.preset);
                    if notes == authored {
                        continue;
                    }
                    let label = format!("{} in {mode:?}: {notes:?}", chord.name);
                    assert!(notes.windows(2).all(|pair| pair[0] < pair[1]), "{label}");
                    for note in notes {
                        assert!(map.degree_in(mode, note).is_some(), "{label} left the mode");
                        assert!(
                            authored.iter().any(|old| (old - note).abs() <= 5),
                            "{label} moved too far"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_re_read_bass_moves_with_its_chord() {
        // Tide (A minor) chord 3 is C over C: in A major it is C# over C#.
        let tide = PROGRESSIONS
            .iter()
            .position(|progression| progression.mood == "Tide")
            .unwrap();
        let map = ModeMap::new(tide, HomeMode::Major);
        let chord = &PROGRESSIONS[tide].chords[2].preset;
        assert_eq!(cell_chord_name(&map.cell(chord), 0), "C#m");
        assert_eq!(map.bass(chord, 48), 49);
    }
}
