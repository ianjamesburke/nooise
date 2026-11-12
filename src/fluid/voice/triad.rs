//! Push the Harmony's harmony model: a triad (root pitch class and quality),
//! the eight moves that push one triad to the next, and minimal voice leading
//! from the chord sounding now. Pure and deterministic: no clock, no session,
//! no randomness.
//!
//! Pitch classes live in the progression's own coordinates (before the Master
//! Key shift), the same coordinates `progression_home` and the chord builder
//! use, so a pushed chord and the authored progression are always comparable.

use super::*;

/// The triad family Push the Harmony moves between. Every triad of the five
/// home modes is one of these, so diatonic moves never leave the set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TriadQuality {
    Major,
    Minor,
    Diminished,
}

impl TriadQuality {
    /// Wire tag for song codes. Append-only.
    pub(crate) const fn tag(self) -> u8 {
        match self {
            Self::Major => 0,
            Self::Minor => 1,
            Self::Diminished => 2,
        }
    }

    pub(crate) const fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            0 => Some(Self::Major),
            1 => Some(Self::Minor),
            2 => Some(Self::Diminished),
            _ => None,
        }
    }

    /// Semitones above the root: the third, then the fifth.
    const fn intervals(self) -> (i32, i32) {
        match self {
            Self::Major => (4, 7),
            Self::Minor => (3, 7),
            Self::Diminished => (3, 6),
        }
    }

    const fn suffix(self) -> &'static str {
        match self {
            Self::Major => "",
            Self::Minor => "m",
            Self::Diminished => "dim",
        }
    }
}

/// A triad: a root pitch class (0 = C) and a quality.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Triad {
    root: u8,
    pub(crate) quality: TriadQuality,
}

impl Triad {
    /// A placeholder for unused fixed-capacity storage.
    pub(crate) const C_MAJOR: Self = Self {
        root: 0,
        quality: TriadQuality::Major,
    };

    pub(crate) fn new(root: i32, quality: TriadQuality) -> Self {
        Self {
            root: root.rem_euclid(12) as u8,
            quality,
        }
    }

    pub(crate) fn root(self) -> i32 {
        i32::from(self.root)
    }

    /// Root, third, fifth as pitch classes.
    pub(crate) fn pitch_classes(self) -> [i32; 3] {
        let (third, fifth) = self.quality.intervals();
        let root = self.root();
        [root, (root + third) % 12, (root + fifth) % 12]
    }

    /// Letter name with every pitch moved by `shift` semitones (the Master
    /// Key's shift for the progression): "Am", "C", "Bdim".
    pub(crate) fn name(self, shift: i32) -> String {
        format!(
            "{}{}",
            chord_pitch_name(self.root() + shift),
            self.quality.suffix()
        )
    }

    /// As the screen writes it: a Roman numeral in `key` (the home pushes
    /// move in), or a letter name under the Master Key for `progression`.
    pub(crate) fn label(self, labels: ChordLabels, key: HomeKey, progression: usize) -> String {
        match labels.names {
            ChordNames::Roman => key.roman(self),
            ChordNames::Letters => self.name(labels.key.shift(progression)),
        }
    }

    /// The triad a resolved chord is heard as: its root, and the quality its
    /// third and fifth spell. A chord with no third (sus, power) takes the
    /// home mode's own triad on its root, or major off the scale.
    pub(crate) fn from_harmony(harmony: &Harmony, key: HomeKey) -> Self {
        let root = harmony.root_note();
        let quality = match (harmony.quality.third(harmony.root_degree), harmony.fifth) {
            (Some(3), Some(6)) => TriadQuality::Diminished,
            (Some(3), _) => TriadQuality::Minor,
            (Some(4), _) => TriadQuality::Major,
            _ => key
                .degree_of(root)
                .map_or(TriadQuality::Major, |degree| key.triad_on(degree).quality),
        };
        Self::new(root, quality)
    }
}

/// The Master Mode dial: the mode every progression is re-read in
/// (`ModeMap`) and pushes move in.
pub(crate) const MASTER_MODES: [(&str, HomeMode); 5] = [
    ("Major", HomeMode::Major),
    ("Minor", HomeMode::Minor),
    ("Dorian", HomeMode::Dorian),
    ("Phrygian", HomeMode::Phrygian),
    ("Mixolydian", HomeMode::Mixolydian),
];

/// The `MASTER_MODES` row a `master.key_mode` value names.
pub(crate) fn master_mode(value: f32) -> (&'static str, HomeMode) {
    MASTER_MODES[wrapped_index(value, MASTER_MODES.len())]
}

/// A home key in a progression's own coordinates: a tonic and a mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct HomeKey {
    tonic: u8,
    mode: HomeMode,
}

impl HomeKey {
    pub(crate) const fn new(tonic: i32, mode: HomeMode) -> Self {
        Self {
            tonic: tonic.rem_euclid(12) as u8,
            mode,
        }
    }

    /// The home pushes move in: the Master Key and Mode, in `progression`'s
    /// coordinates (its tonic, which the Master Key shift carries to the
    /// Master Key).
    pub(crate) fn of(progression: usize, key: MasterKey) -> Self {
        Self::new(progression_tonic_pitch_class(progression), key.mode)
    }

    pub(crate) fn of_song(controls: &FluidControls) -> Self {
        Self::of(
            progression_index(controls.pad.progression),
            MasterKey::from_controls(&controls.master),
        )
    }

    /// `triad` as a Roman numeral in this key: its degree, flat (or sharp
    /// for a raised seventh) off the scale, upper case major, lower case
    /// minor, lower case with `°` diminished ("i", "bVI", "#vii°").
    pub(crate) fn roman(self, triad: Triad) -> String {
        let (accidental, degree) = scale_degree(triad.root() - i32::from(self.tonic), self.mode);
        let numeral = ROMAN_NUMERALS[degree];
        match triad.quality {
            TriadQuality::Major => format!("{accidental}{numeral}"),
            TriadQuality::Minor => format!("{accidental}{}", numeral.to_lowercase()),
            TriadQuality::Diminished => format!("{accidental}{}°", numeral.to_lowercase()),
        }
    }

    fn scale(self) -> [i32; 7] {
        self.mode
            .intervals()
            .map(|step| (i32::from(self.tonic) + step) % 12)
    }

    /// Which 0-based scale degree `pitch` sits on, if it is in the scale.
    pub(crate) fn degree_of(self, pitch: i32) -> Option<usize> {
        let pitch = pitch.rem_euclid(12);
        self.scale().iter().position(|&pc| pc == pitch)
    }

    /// The mode's own triad on `degree`, stacked in scale thirds.
    pub(crate) fn triad_on(self, degree: usize) -> Triad {
        let steps = self.mode.intervals();
        let at = |offset: usize| steps[(degree + offset) % 7];
        let third = (at(2) - at(0)).rem_euclid(12);
        let fifth = (at(4) - at(0)).rem_euclid(12);
        let quality = match (third, fifth) {
            (3, 6) => TriadQuality::Diminished,
            (3, _) => TriadQuality::Minor,
            _ => TriadQuality::Major,
        };
        Triad::new(i32::from(self.tonic) + at(0), quality)
    }

    pub(crate) fn tonic_triad(self) -> Triad {
        self.triad_on(0)
    }

    /// Diatonic quality on `root` when it is in the scale, else `fallback`.
    fn quality_on(self, root: i32, fallback: TriadQuality) -> TriadQuality {
        self.degree_of(root)
            .map_or(fallback, |degree| self.triad_on(degree).quality)
    }

    /// `count` scale degrees below (`-1`) or above (`+1`) `root`: the first
    /// step lands on the nearest degree that way, each further step on the
    /// next degree.
    fn steps(self, root: i32, direction: i32, count: usize) -> Triad {
        let mut triad = self.step(root, direction);
        for _ in 1..count {
            triad = self.step(triad.root(), direction);
        }
        triad
    }

    /// The nearest scale degree strictly below (`-1`) or above (`+1`) `root`.
    fn step(self, root: i32, direction: i32) -> Triad {
        let degree = self
            .scale()
            .iter()
            .enumerate()
            .filter(|(_, pc)| (**pc - root).rem_euclid(12) != 0)
            .min_by_key(|(_, pc)| (direction * (**pc - root)).rem_euclid(12))
            .map(|(degree, _)| degree)
            .expect("a seven-note scale always has another degree");
        self.triad_on(degree)
    }
}

/// One push. Every move reads the chord the harmony is on now (or the one
/// already queued) and names exactly one destination, so no key is ever wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HarmonyMove {
    StepDown,
    StepUp,
    FifthDown,
    FifthUp,
    Relative,
    Parallel,
    LeadingTone,
    Home,
    StepDownTwo,
    StepDownThree,
    StepUpTwo,
    StepUpThree,
    /// Lean: the root a half step up, to a diminished triad. From a major
    /// chord only the root moves (G B D to G# B D), the vii° of the chord a
    /// step up, so the next step up resolves it (G, G#dim, Am: a rising bass).
    Lean,
}

/// One move, the key that plays it, and the overlay's word for it. The step
/// moves sit in two columns of a columnar keyboard: `u`/`j`/`m` go down 2, 1
/// and 3 scale steps, and `i`/`k`/`,` go up 2, 1 and 3, so the row above and
/// below the home key is a bigger step the same way. `o`, above `l`, raises
/// the root a half step.
pub(crate) struct HarmonyMoveRow {
    pub(crate) harmony_move: HarmonyMove,
    pub(crate) key: char,
    pub(crate) label: &'static str,
}

/// The single key table for the moves, in `HarmonyMove` discriminant order
/// (test-enforced). The runtime key map, the overlay and the help read it.
pub(crate) const HARMONY_MOVES: [HarmonyMoveRow; 13] = [
    HarmonyMoveRow {
        harmony_move: HarmonyMove::StepDown,
        key: 'j',
        label: "↓1",
    },
    HarmonyMoveRow {
        harmony_move: HarmonyMove::StepUp,
        key: 'k',
        label: "↑1",
    },
    HarmonyMoveRow {
        harmony_move: HarmonyMove::FifthDown,
        key: 'h',
        // "Fall", not "resolve": a fifth down only resolves from a
        // dominant; from the tonic it lands on iv, a lift.
        label: "fall",
    },
    HarmonyMoveRow {
        harmony_move: HarmonyMove::FifthUp,
        key: 'l',
        label: "lift",
    },
    HarmonyMoveRow {
        harmony_move: HarmonyMove::Relative,
        key: 'f',
        label: "relative",
    },
    HarmonyMoveRow {
        harmony_move: HarmonyMove::Parallel,
        key: 'd',
        label: "parallel",
    },
    HarmonyMoveRow {
        harmony_move: HarmonyMove::LeadingTone,
        key: 's',
        label: "leading",
    },
    HarmonyMoveRow {
        harmony_move: HarmonyMove::Home,
        key: 'a',
        label: "home",
    },
    HarmonyMoveRow {
        harmony_move: HarmonyMove::StepDownTwo,
        key: 'u',
        label: "↓2",
    },
    HarmonyMoveRow {
        harmony_move: HarmonyMove::StepDownThree,
        key: 'm',
        label: "↓3",
    },
    HarmonyMoveRow {
        harmony_move: HarmonyMove::StepUpTwo,
        key: 'i',
        label: "↑2",
    },
    HarmonyMoveRow {
        harmony_move: HarmonyMove::StepUpThree,
        key: ',',
        label: "↑3",
    },
    HarmonyMoveRow {
        harmony_move: HarmonyMove::Lean,
        key: 'o',
        label: "lean",
    },
];

impl HarmonyMove {
    pub(crate) fn from_key(key: char) -> Option<Self> {
        HARMONY_MOVES
            .iter()
            .find(|row| row.key == key)
            .map(|row| row.harmony_move)
    }

    /// The triad this move pushes `from` to in `key`.
    ///
    /// Steps walk the home scale and take its own triad. Fifths are perfect
    /// fifths that take the diatonic quality when they land in the scale and
    /// keep the current quality outside it. P, L and R are the
    /// neo-Riemannian swaps, each moving one voice; a diminished triad has no
    /// textbook P/L/R, so each moves one voice to a consonant neighbour: P
    /// raises the fifth (Bdim to Bm), R lowers the root a tone (Bdim to Dm),
    /// L raises the fifth a tone (Bdim to G).
    pub(crate) fn apply(self, from: Triad, key: HomeKey) -> Triad {
        let root = from.root();
        match self {
            Self::StepDown => key.steps(root, -1, 1),
            Self::StepUp => key.steps(root, 1, 1),
            Self::StepDownTwo => key.steps(root, -1, 2),
            Self::StepDownThree => key.steps(root, -1, 3),
            Self::StepUpTwo => key.steps(root, 1, 2),
            Self::StepUpThree => key.steps(root, 1, 3),
            Self::FifthDown => Triad::new(root - 7, key.quality_on(root - 7, from.quality)),
            Self::FifthUp => Triad::new(root + 7, key.quality_on(root + 7, from.quality)),
            Self::Parallel => match from.quality {
                TriadQuality::Major => Triad::new(root, TriadQuality::Minor),
                TriadQuality::Minor => Triad::new(root, TriadQuality::Major),
                TriadQuality::Diminished => Triad::new(root, TriadQuality::Minor),
            },
            Self::Relative => match from.quality {
                TriadQuality::Major => Triad::new(root - 3, TriadQuality::Minor),
                TriadQuality::Minor => Triad::new(root + 3, TriadQuality::Major),
                TriadQuality::Diminished => Triad::new(root + 3, TriadQuality::Minor),
            },
            Self::LeadingTone => match from.quality {
                TriadQuality::Major => Triad::new(root + 4, TriadQuality::Minor),
                TriadQuality::Minor => Triad::new(root - 4, TriadQuality::Major),
                TriadQuality::Diminished => Triad::new(root - 4, TriadQuality::Major),
            },
            Self::Home => key.tonic_triad(),
            Self::Lean => Triad::new(root + 1, TriadQuality::Diminished),
        }
    }
}

/// The lowest and highest MIDI notes a pushed voicing keeps to: the pad's
/// authored register, so a long run of moves never drifts out of it.
const PUSH_REGISTER: (i32, i32) = (40, 79);
/// What one voice outside `PUSH_REGISTER` costs, in semitones of movement.
const REGISTER_PENALTY: i32 = 24;

/// Voice `target` from the four notes sounding now with the least total
/// movement: every voice moves to a nearby tone of the new triad, all three
/// tones sound, no two voices share a note, and the voicing stays in the
/// pad's register. Ties resolve to the first candidate in a fixed order, so
/// the answer is deterministic. Returns ascending MIDI notes.
pub(crate) fn voice_lead(target: Triad, from: [i32; 4]) -> [i32; 4] {
    let pcs = target.pitch_classes();
    let mut best: Option<(i32, [i32; 4])> = None;
    // Each voice picks a tone (3 ways) and a direction (2 ways): 6^4 tries.
    for choice in 0..6_usize.pow(4) {
        let mut notes = [0; 4];
        let mut used = [false; 3];
        let mut cost = 0;
        let mut code = choice;
        for (voice, note) in notes.iter_mut().enumerate() {
            let tone = code % 3;
            let up = (code / 3) % 2 == 1;
            code /= 6;
            used[tone] = true;
            let rise = (pcs[tone] - from[voice]).rem_euclid(12);
            *note = if up || rise == 0 {
                from[voice] + rise
            } else {
                from[voice] + rise - 12
            };
            cost += (*note - from[voice]).abs();
            if *note < PUSH_REGISTER.0 || *note > PUSH_REGISTER.1 {
                cost += REGISTER_PENALTY;
            }
        }
        let distinct = (0..4).all(|a| (a + 1..4).all(|b| notes[a] != notes[b]));
        if !used.iter().all(|tone| *tone) || !distinct {
            continue;
        }
        if best.is_none_or(|(best_cost, _)| cost < best_cost) {
            best = Some((cost, notes));
        }
    }
    let (_, mut notes) = best.expect("four voices always cover three tones without a unison");
    notes.sort_unstable();
    notes
}

/// The root note nearest `anchor` (within a tritone, ties below), so a
/// pushed root keeps the Bass in its authored register.
pub(crate) fn root_near(root: i32, anchor: i32) -> i32 {
    anchor + (root - anchor + 6).rem_euclid(12) - 6
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: i32 = 9;
    const A_MINOR: HomeKey = HomeKey::new(A, HomeMode::Minor);

    fn am() -> Triad {
        Triad::new(A, TriadQuality::Minor)
    }

    fn named(triad: Triad) -> String {
        triad.name(0)
    }

    #[test]
    fn the_key_table_is_in_move_order_with_distinct_home_row_keys() {
        for (index, row) in HARMONY_MOVES.iter().enumerate() {
            assert_eq!(row.harmony_move as usize, index);
            // The home row, the rows above and below the step columns, and
            // `o` above `l`.
            assert!(
                "asdfhjkluim,o".contains(row.key),
                "{} is unplanned",
                row.key
            );
            assert_eq!(HarmonyMove::from_key(row.key), Some(row.harmony_move));
        }
        let mut keys: Vec<char> = HARMONY_MOVES.iter().map(|row| row.key).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), HARMONY_MOVES.len());
    }

    #[test]
    fn every_move_from_a_minor_matches_the_design_table() {
        let expected = [
            (HarmonyMove::StepDown, "G"),
            (HarmonyMove::StepUp, "Bdim"),
            (HarmonyMove::FifthDown, "Dm"),
            (HarmonyMove::FifthUp, "Em"),
            (HarmonyMove::Relative, "C"),
            (HarmonyMove::Parallel, "A"),
            (HarmonyMove::LeadingTone, "F"),
            (HarmonyMove::Home, "Am"),
            (HarmonyMove::StepDownTwo, "F"),
            (HarmonyMove::StepDownThree, "Em"),
            (HarmonyMove::StepUpTwo, "C"),
            (HarmonyMove::StepUpThree, "Dm"),
        ];
        for (harmony_move, name) in expected {
            assert_eq!(
                named(harmony_move.apply(am(), A_MINOR)),
                name,
                "{harmony_move:?}"
            );
        }
    }

    #[test]
    fn p_l_and_r_are_involutions_on_every_major_and_minor_triad() {
        for root in 0..12 {
            for quality in [TriadQuality::Major, TriadQuality::Minor] {
                let triad = Triad::new(root, quality);
                for harmony_move in [
                    HarmonyMove::Parallel,
                    HarmonyMove::LeadingTone,
                    HarmonyMove::Relative,
                ] {
                    let once = harmony_move.apply(triad, A_MINOR);
                    assert_ne!(once, triad);
                    assert_eq!(harmony_move.apply(once, A_MINOR), triad, "{harmony_move:?}");
                }
            }
        }
    }

    #[test]
    fn p_l_and_r_each_move_exactly_one_voice() {
        for root in 0..12 {
            for quality in [
                TriadQuality::Major,
                TriadQuality::Minor,
                TriadQuality::Diminished,
            ] {
                let from = Triad::new(root, quality);
                for harmony_move in [
                    HarmonyMove::Parallel,
                    HarmonyMove::LeadingTone,
                    HarmonyMove::Relative,
                ] {
                    let to = harmony_move.apply(from, A_MINOR);
                    let shared = from
                        .pitch_classes()
                        .iter()
                        .filter(|pc| to.pitch_classes().contains(pc))
                        .count();
                    assert_eq!(shared, 2, "{from:?} {harmony_move:?} {to:?}");
                }
            }
        }
    }

    #[test]
    fn steps_walk_the_home_scale_in_every_mode() {
        for mode in [
            HomeMode::Major,
            HomeMode::Minor,
            HomeMode::Dorian,
            HomeMode::Phrygian,
            HomeMode::Mixolydian,
        ] {
            let key = HomeKey::new(2, mode);
            let mut triad = key.tonic_triad();
            for degree in 1..=7 {
                triad = HarmonyMove::StepUp.apply(triad, key);
                assert_eq!(triad, key.triad_on(degree % 7), "{mode:?} up {degree}");
            }
            for degree in (0..7).rev() {
                triad = HarmonyMove::StepDown.apply(triad, key);
                assert_eq!(triad, key.triad_on(degree), "{mode:?} down to {degree}");
            }
        }
    }

    /// A bigger step is the same number of single steps, in every mode.
    #[test]
    fn interval_steps_are_repeated_single_steps() {
        for mode in [
            HomeMode::Major,
            HomeMode::Minor,
            HomeMode::Dorian,
            HomeMode::Phrygian,
            HomeMode::Mixolydian,
        ] {
            let key = HomeKey::new(5, mode);
            for root in 0..12 {
                let from = Triad::new(root, TriadQuality::Minor);
                let twice = |single: HarmonyMove| single.apply(single.apply(from, key), key);
                let thrice = |single: HarmonyMove| single.apply(twice(single), key);
                assert_eq!(
                    HarmonyMove::StepDownTwo.apply(from, key),
                    twice(HarmonyMove::StepDown)
                );
                assert_eq!(
                    HarmonyMove::StepUpTwo.apply(from, key),
                    twice(HarmonyMove::StepUp)
                );
                assert_eq!(
                    HarmonyMove::StepDownThree.apply(from, key),
                    thrice(HarmonyMove::StepDown)
                );
                assert_eq!(
                    HarmonyMove::StepUpThree.apply(from, key),
                    thrice(HarmonyMove::StepUp)
                );
            }
        }
    }

    #[test]
    fn a_step_from_off_the_scale_lands_on_the_nearest_degree_that_way() {
        // C# minor is not in A minor: up reaches D, down reaches C.
        let c_sharp = Triad::new(1, TriadQuality::Minor);
        assert_eq!(named(HarmonyMove::StepUp.apply(c_sharp, A_MINOR)), "Dm");
        assert_eq!(named(HarmonyMove::StepDown.apply(c_sharp, A_MINOR)), "C");
    }

    #[test]
    fn fifths_keep_their_quality_off_the_scale_and_undo_each_other() {
        let f = Triad::new(5, TriadQuality::Major);
        // Bb is outside A minor: the major quality carries over.
        assert_eq!(named(HarmonyMove::FifthDown.apply(f, A_MINOR)), "Bb");
        for root in 0..12 {
            let triad = Triad::new(root, TriadQuality::Major);
            let key = HomeKey::new(0, HomeMode::Major);
            if key.degree_of(root).is_none() && key.degree_of(root + 7).is_none() {
                let lifted = HarmonyMove::FifthUp.apply(triad, key);
                assert_eq!(HarmonyMove::FifthDown.apply(lifted, key), triad);
            }
        }
    }

    #[test]
    fn home_is_the_tonic_triad_of_every_mode() {
        assert_eq!(named(HomeKey::new(0, HomeMode::Major).tonic_triad()), "C");
        assert_eq!(named(HomeKey::new(4, HomeMode::Dorian).tonic_triad()), "Em");
        assert_eq!(
            named(HomeKey::new(4, HomeMode::Phrygian).tonic_triad()),
            "Em"
        );
        assert_eq!(
            named(HomeKey::new(7, HomeMode::Mixolydian).tonic_triad()),
            "G"
        );
        let far = Triad::new(6, TriadQuality::Diminished);
        assert_eq!(HarmonyMove::Home.apply(far, A_MINOR), am());
    }

    /// A derived song pushes in the progression's own home; a chosen mode
    /// keeps the progression's tonic and changes what steps and home produce.
    #[test]
    fn the_master_mode_sets_the_home_pushes_move_in() {
        let mut controls = FluidControls::default();
        for progression in 0..=CUSTOM_PROGRESSION_INDEX {
            controls.pad.progression = progression as f32;
            controls.derive_key();
            let (tonic, mode, _) = progression_home(progression);
            assert_eq!(HomeKey::of_song(&controls), HomeKey::new(tonic, mode));
        }
        controls.pad.progression = 0.0; // A minor
        controls.derive_key();
        let am = Triad::new(A, TriadQuality::Minor);
        let named = |controls: &FluidControls, harmony_move: HarmonyMove| {
            harmony_move.apply(am, HomeKey::of_song(controls)).name(0)
        };
        assert_eq!(named(&controls, HarmonyMove::StepUp), "Bdim");
        controls.master.mode = 2.0; // Dorian
        assert_eq!(named(&controls, HarmonyMove::StepUp), "Bm");
        controls.master.mode = 0.0; // Major
        assert_eq!(named(&controls, HarmonyMove::Home), "A");
        assert_eq!(named(&controls, HarmonyMove::StepDown), "Abdim");
        assert_eq!(master_mode(controls.master.mode).0, "Major");
        // The Master Key moves the home with it: a C key homes on C's
        // degree in the progression's coordinates, still A there.
        controls.master.key = 0.0;
        assert_eq!(named(&controls, HarmonyMove::Home), "A");
    }

    #[test]
    fn names_follow_the_master_key_shift() {
        assert_eq!(am().name(3), "Cm");
        assert_eq!(Triad::new(11, TriadQuality::Diminished).name(0), "Bdim");
    }

    #[test]
    fn raising_the_root_is_a_rising_bass_that_a_step_up_resolves() {
        use TriadQuality::{Diminished, Major};
        let g = Triad::new(7, Major);
        let lifted = HarmonyMove::Lean.apply(g, A_MINOR);
        assert_eq!(lifted, Triad::new(8, Diminished), "G to G#dim");
        assert_eq!(A_MINOR.roman(lifted), "#vii°");
        assert_eq!(
            HarmonyMove::StepUp.apply(lifted, A_MINOR),
            am(),
            "resolves to Am"
        );
        // From a major chord only the root voice moves: G B D to G# B D.
        let sounding = [43, 55, 59, 62];
        assert_eq!(voice_lead(lifted, sounding), [44, 56, 59, 62]);
        let moved: i32 = voice_lead(lifted, [47, 50, 55, 59])
            .iter()
            .zip([47, 50, 55, 59])
            .map(|(to, from)| (to - from).abs())
            .sum();
        assert_eq!(moved, 1, "one voice, one semitone");
        // From a minor chord the root and the minor third both rise: Am
        // (A C E) to A#dim (A# C# E), the vii° of B, and a step up lands on
        // the scale's own chord there (Bdim in A minor).
        let from_minor = HarmonyMove::Lean.apply(am(), A_MINOR);
        assert_eq!(from_minor, Triad::new(10, Diminished));
        assert_eq!(from_minor.pitch_classes(), [10, 1, 4]);
        assert_eq!(
            HarmonyMove::StepUp.apply(from_minor, A_MINOR),
            Triad::new(11, Diminished)
        );
        // Lifting a diminished chord keeps climbing in diminished chords.
        assert_eq!(
            HarmonyMove::Lean.apply(lifted, A_MINOR),
            Triad::new(9, Diminished)
        );
    }

    #[test]
    fn roman_numerals_read_the_home_key_and_mode() {
        use TriadQuality::{Diminished, Major, Minor};
        let a_minor = HomeKey::new(9, HomeMode::Minor);
        for (root, quality, expected) in [
            (9, Minor, "i"),
            (11, Diminished, "ii°"),
            (0, Major, "III"),
            (2, Minor, "iv"),
            (4, Major, "V"),
            (5, Major, "VI"),
            (7, Major, "VII"),
            (8, Diminished, "#vii°"),
            (3, Major, "bV"),
        ] {
            assert_eq!(a_minor.roman(Triad::new(root, quality)), expected);
        }
        let a_major = HomeKey::new(9, HomeMode::Major);
        assert_eq!(a_major.roman(Triad::new(5, Major)), "bVI");
        assert_eq!(a_major.roman(Triad::new(7, Major)), "bVII");
        assert_eq!(a_major.roman(Triad::new(9, Major)), "I");
        // The same triad in another key or mode reads as its function there.
        let a_dorian = HomeKey::new(9, HomeMode::Dorian);
        assert_eq!(a_dorian.roman(Triad::new(2, Major)), "IV");
    }

    #[test]
    fn the_key_reads_as_a_chord_symbol_for_major_and_minor_and_by_name_otherwise() {
        for (mode, expected) in [
            (HomeMode::Minor, "Am"),
            (HomeMode::Major, "A"),
            (HomeMode::Dorian, "A Dorian"),
            (HomeMode::Phrygian, "A Phrygian"),
            (HomeMode::Mixolydian, "A Mixolydian"),
        ] {
            assert_eq!(MasterKey::new(A, mode).label(), expected);
        }
    }

    #[test]
    fn every_built_in_chord_reads_as_a_triad_on_its_own_root() {
        for (index, progression) in PROGRESSIONS.iter().enumerate() {
            let (tonic, mode, _) = progression_home(index);
            let key = HomeKey::new(tonic, mode);
            for chord in &progression.chords {
                let (harmony, _) = chord_from_slot(&chord.preset);
                let triad = Triad::from_harmony(&harmony, key);
                assert_eq!(
                    triad.root(),
                    harmony.root_note().rem_euclid(12),
                    "{}",
                    chord.name
                );
                let minor = chord.name[1..].starts_with('m') && !chord.name[1..].starts_with("maj");
                if minor && !chord.name.contains("sus") {
                    assert_ne!(triad.quality, TriadQuality::Major, "{}", chord.name);
                }
            }
        }
    }

    #[test]
    fn voice_leading_moves_each_voice_to_the_nearest_tone() {
        // A minor close: A2 C3 E3 A3 -> C major moves only A to G... or keeps
        // C and E: the cheapest is one voice moving a tone.
        let from = [45, 48, 52, 57];
        let to = voice_lead(Triad::new(0, TriadQuality::Major), from);
        let moved: i32 = from.iter().zip(to).map(|(a, b)| (a - b).abs()).sum();
        assert_eq!(to, [43, 48, 52, 55]);
        assert_eq!(moved, 4);
        // P: A minor to A major moves the third up one semitone, nothing else.
        let to = voice_lead(Triad::new(A, TriadQuality::Major), from);
        assert_eq!(to, [45, 49, 52, 57]);
    }

    #[test]
    fn voice_leading_sounds_every_tone_without_unisons_and_stays_in_register() {
        let key = A_MINOR;
        let mut notes = [45, 48, 52, 57];
        let mut triad = am();
        // A long deterministic walk through every move.
        for step in 0..400 {
            let harmony_move =
                HARMONY_MOVES[(step * 5 + step / 7) % HARMONY_MOVES.len()].harmony_move;
            triad = harmony_move.apply(triad, key);
            let next = voice_lead(triad, notes);
            for pc in triad.pitch_classes() {
                assert!(
                    next.iter().any(|note| note.rem_euclid(12) == pc),
                    "{triad:?}"
                );
            }
            assert!(next.windows(2).all(|pair| pair[0] < pair[1]), "{next:?}");
            assert!(
                next.iter()
                    .all(|note| (PUSH_REGISTER.0..=PUSH_REGISTER.1).contains(note)),
                "{next:?} drifted"
            );
            assert_eq!(voice_lead(triad, notes), next, "deterministic");
            notes = next;
        }
    }

    #[test]
    fn a_pushed_root_stays_within_a_tritone_of_its_anchor() {
        assert_eq!(root_near(0, 45), 48);
        assert_eq!(root_near(7, 45), 43);
        assert_eq!(root_near(3, 45), 39);
        assert_eq!(root_near(9, 45), 45);
        for root in 0..12 {
            let note = root_near(root, 50);
            assert!((44..=55).contains(&note) && note.rem_euclid(12) == root);
        }
    }

    #[test]
    fn wire_tags_round_trip() {
        for quality in [
            TriadQuality::Major,
            TriadQuality::Minor,
            TriadQuality::Diminished,
        ] {
            assert_eq!(TriadQuality::from_tag(quality.tag()), Some(quality));
        }
        assert_eq!(TriadQuality::from_tag(3), None);
    }
}
