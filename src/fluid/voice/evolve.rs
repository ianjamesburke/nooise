//! Smart chord evolution for the v3 model. Evolve changes at most one
//! sounding cell per phrase, choosing it from diatonic families ranked by how
//! much it keeps in place, never leaving the progression's home mode.
//!
//! The math is pure: it selects a `Harmony` and writes it back through the
//! legacy slot bridge. When the v3 lane lands, only `slot_with_harmony`'s call
//! site changes; the selection below is model-independent.

use super::*;
use rand::Rng;

/// The key a progression is in: its tonic note and the scale that stays put
/// when a chord borrows a note outside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Home {
    pub(crate) tonic: i32,
    pub(crate) mode: HomeMode,
}

impl Home {
    pub(crate) fn of(progression: usize) -> Self {
        let (tonic, mode, _) = progression_home(progression);
        Self { tonic, mode }
    }

    /// The 0-based mode degree `note` sits on, or `None` when it borrows.
    pub(crate) fn degree_of(self, note: i32) -> Option<usize> {
        self.mode
            .intervals()
            .iter()
            .position(|&step| step == (note - self.tonic).rem_euclid(12))
    }

    /// Whether every realized note is in the home mode.
    pub(crate) fn holds(self, notes: [i32; 4]) -> bool {
        let intervals = self.mode.intervals();
        notes
            .iter()
            .all(|note| intervals.contains(&(note - self.tonic).rem_euclid(12)))
    }
}

/// How far a candidate moves from the chord it replaces. Drawn by weight, so
/// a recolor is the common case and a function change the rare one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Family {
    Recolor,
    Relative,
    Fifth,
}

const FAMILY_WEIGHTS: [(Family, u32); 3] = [
    (Family::Recolor, 50),
    (Family::Relative, 35),
    (Family::Fifth, 15),
];

const MAX_CANDIDATES: usize = 4;

/// The diatonic triad on `degree`, rooted near `near`. `None` for the
/// diminished degree or an augmented one, which a plain triad cannot spell.
fn diatonic_triad(degree: usize, home: Home, near: i32) -> Option<Harmony> {
    let steps = home.mode.intervals();
    let root_pc = (home.tonic + steps[degree]).rem_euclid(12);
    let (root_degree, accidental) = root_fields_near(root_pc, near)?;
    let third = (steps[(degree + 2) % 7] - steps[degree]).rem_euclid(12);
    let fifth = (steps[(degree + 4) % 7] - steps[degree]).rem_euclid(12);
    if fifth == 6 {
        return None;
    }
    let quality = match third {
        3 => Quality::Minor,
        4 => Quality::Major,
        _ => return None,
    };
    Some(Harmony {
        root_degree,
        accidental,
        quality,
        tensions: [None; 2],
        fifth: Some(7),
        bass: None,
    })
}

/// The lattice `(root_degree, accidental)` that sounds `pitch_class` nearest
/// `near`, preferring a natural spelling on a tie.
fn root_fields_near(pitch_class: i32, near: i32) -> Option<(i8, i8)> {
    let mut best: Option<(i8, i8)> = None;
    let mut best_distance = i32::MAX;
    for degree in -7..=7i8 {
        for accidental in -1..=1i8 {
            let note = shift_diatonic(45, i32::from(degree)) + i32::from(accidental);
            if note.rem_euclid(12) != pitch_class.rem_euclid(12) {
                continue;
            }
            let distance = (note - near).abs();
            if distance < best_distance || (distance == best_distance && accidental == 0) {
                best_distance = distance;
                best = Some((degree, accidental));
            }
        }
    }
    best
}

/// The diatonic tension `steps` scale steps above `degree`, only when the
/// legacy slot can encode its pitch class exactly (7th, 9th, or 13th).
fn tension_interval(degree: usize, steps: usize, home: Home) -> Option<Interval> {
    let intervals = home.mode.intervals();
    let interval = (intervals[(degree + steps) % 7] - intervals[degree]).rem_euclid(12);
    // Match `chord_from_slot`'s representation: 7ths stay in the octave, a
    // 9th is 14 and a 13th is 21, so a round-trip is exact.
    match interval {
        10 => Some(10),
        11 => Some(11),
        2 => Some(14),
        9 => Some(21),
        _ => None,
    }
}

/// The candidate harmonies for one family, written into `out`.
fn candidates(
    current: &Harmony,
    family: Family,
    home: Home,
    out: &mut [Harmony; MAX_CANDIDATES],
) -> usize {
    let Some(degree) = home.degree_of(current.root_note()) else {
        // A borrowed root is authored colour, not raw material.
        return 0;
    };
    let near = current.root_note();
    let mut count = 0;
    let push = |harmony: Harmony, out: &mut [Harmony; MAX_CANDIDATES], count: &mut usize| {
        if *count < MAX_CANDIDATES {
            out[*count] = harmony;
            *count += 1;
        }
    };
    match family {
        Family::Recolor => {
            let mut bare = *current;
            bare.tensions = [None; 2];
            push(bare, out, &mut count);
            for steps in [6, 1, 5] {
                if let Some(tension) = tension_interval(degree, steps, home) {
                    let mut harmony = *current;
                    harmony.tensions = [Some(tension), None];
                    push(harmony, out, &mut count);
                }
            }
        }
        Family::Relative => {
            for offset in [2, 5] {
                if let Some(harmony) = diatonic_triad((degree + offset) % 7, home, near) {
                    push(harmony, out, &mut count);
                }
            }
        }
        Family::Fifth => {
            for offset in [3, 4] {
                if let Some(harmony) = diatonic_triad((degree + offset) % 7, home, near) {
                    push(harmony, out, &mut count);
                }
            }
        }
    }
    count
}

fn shared_notes(a: [i32; 4], b: [i32; 4]) -> usize {
    a.iter().filter(|note| b.contains(note)).count()
}

fn voice_leading_cost(a: [i32; 4], b: [i32; 4]) -> i32 {
    a.iter().zip(b).map(|(x, y)| (x - y).abs()).sum()
}

/// What the chosen cell has to fit between: its current notes and every
/// neighbour the overlap law links it to. Stage 1 links every neighbour.
pub(crate) struct CellContext {
    pub(crate) current: [i32; 4],
    pub(crate) linked: [Option<([i32; 4], bool)>; 6],
}

/// The one chord choice. `voice` realizes a candidate the way it will sound,
/// so the filters never learn which stage of the model is live. `None` when
/// no candidate fits.
pub(crate) fn evolve_harmony(
    current: &Harmony,
    ctx: &CellContext,
    home: Home,
    rng: &mut StdRng,
    voice: impl Fn(&Harmony) -> [i32; 4],
) -> Option<Harmony> {
    let linked: Vec<([i32; 4], bool)> = ctx.linked.iter().flatten().copied().collect();
    // Draw the family, then walk the table if it offered nothing that fits.
    let roll = rng.gen_range(0..100u32);
    let mut start = 0;
    let mut cumulative = 0;
    for (index, entry) in FAMILY_WEIGHTS.iter().enumerate() {
        cumulative += entry.1;
        if roll < cumulative {
            start = index;
            break;
        }
    }
    let mut buffer = [*current; MAX_CANDIDATES];
    for step in 0..FAMILY_WEIGHTS.len() {
        let family = FAMILY_WEIGHTS[(start + step) % FAMILY_WEIGHTS.len()].0;
        let count = candidates(current, family, home, &mut buffer);
        let mut best: Option<(&Harmony, usize, i32, i32)> = None;
        for candidate in &buffer[..count] {
            let notes = voice(candidate);
            if notes == ctx.current || !home.holds(notes) {
                continue;
            }
            if linked
                .iter()
                .any(|(neighbour, sustained)| *sustained && shared_notes(notes, *neighbour) == 0)
            {
                continue;
            }
            let shared: usize = linked
                .iter()
                .filter(|(_, sustained)| *sustained)
                .map(|(neighbour, _)| shared_notes(notes, *neighbour))
                .sum();
            // Higher shared, then lower voice-leading cost, then lower root.
            let key = (
                candidate,
                shared,
                -voice_leading_cost(notes, ctx.current),
                -candidate.root_note(),
            );
            let better = match best {
                None => true,
                Some((_, best_shared, best_cost, best_root)) => {
                    (key.1, key.2, key.3) > (best_shared, best_cost, best_root)
                }
            };
            if better {
                best = Some((key.0, key.1, key.2, key.3));
            }
        }
        if let Some((harmony, ..)) = best {
            return Some(*harmony);
        }
    }
    None
}

/// The inverse of the harmony half of `chord_from_slot`: root, quality,
/// tensions, and fifth over a copy of `base`. `None` when the harmony needs a
/// tension the legacy slot cannot spell exactly.
pub(crate) fn slot_with_harmony(
    base: &ChordSlotControls,
    harmony: &Harmony,
) -> Option<ChordSlotControls> {
    let quality = match harmony.quality {
        Quality::Major => 1.0,
        Quality::Minor => -1.0,
        Quality::Sus2 => 2.0,
        Quality::Sus4 => 3.0,
        Quality::Power => 4.0,
        Quality::Scale => 0.0,
    };
    let fifth = match harmony.fifth {
        None => 2.0,
        Some(7) => 1.0,
        Some(_) => 0.0,
    };
    let extension = |tension: Option<Interval>| -> Option<f32> {
        Some(match tension.map(|interval| interval.rem_euclid(12)) {
            None => 0.0,
            Some(10) => 4.0,
            Some(11) => 5.0,
            Some(2) => 6.0,
            Some(5) => 7.0,
            Some(9) => 9.0,
            _ => return None,
        })
    };
    let mut slot = *base;
    slot.degree = f32::from(harmony.root_degree);
    slot.accidental = f32::from(harmony.accidental);
    slot.quality = quality;
    slot.fifth = fifth;
    slot.extension = extension(harmony.tensions[0])?;
    slot.extension2 = extension(harmony.tensions[1]).unwrap_or(0.0);
    Some(slot)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODES: [HomeMode; 5] = [
        HomeMode::Major,
        HomeMode::Minor,
        HomeMode::Dorian,
        HomeMode::Phrygian,
        HomeMode::Mixolydian,
    ];

    fn in_mode(home: Home, harmony: &Harmony) -> bool {
        let (roles, count) = harmony.intervals();
        let root = harmony.root_note();
        roles[..count].iter().all(|role| {
            home.mode
                .intervals()
                .contains(&(root + i32::from(*role) - home.tonic).rem_euclid(12))
        })
    }

    fn all_candidates(home: Home, current: &Harmony) -> Vec<Harmony> {
        let mut out = Vec::new();
        let mut buffer = [*current; MAX_CANDIDATES];
        for family in [Family::Recolor, Family::Relative, Family::Fifth] {
            let count = candidates(current, family, home, &mut buffer);
            out.extend_from_slice(&buffer[..count]);
        }
        out
    }

    #[test]
    fn every_offered_candidate_stays_in_the_home_mode() {
        for mode in MODES {
            let home = Home { tonic: 45, mode };
            for degree in 0..7 {
                let Some(current) = diatonic_triad(degree, home, home.tonic + 12) else {
                    continue;
                };
                for candidate in all_candidates(home, &current) {
                    assert!(in_mode(home, &candidate), "{mode:?} degree {degree}");
                }
            }
        }
    }

    #[test]
    fn tensions_are_always_legacy_encodable() {
        for mode in MODES {
            let home = Home { tonic: 45, mode };
            for degree in 0..7 {
                for steps in 0..7 {
                    if let Some(interval) = tension_interval(degree, steps, home) {
                        assert!(matches!(interval, 10 | 11 | 14 | 21));
                    }
                }
            }
        }
    }

    #[test]
    fn borrowed_roots_are_left_alone() {
        let home = Home {
            tonic: 45,
            mode: HomeMode::Major,
        };
        // A root a semitone above the tonic is outside every major degree.
        let (root_degree, accidental) = root_fields_near(46, 45).unwrap();
        let borrowed = Harmony {
            root_degree,
            accidental,
            quality: Quality::Major,
            tensions: [None; 2],
            fifth: Some(7),
            bass: None,
        };
        assert_eq!(all_candidates(home, &borrowed).len(), 0);
    }

    #[test]
    fn slot_with_harmony_recovers_the_same_chord() {
        let base = ChordSlotControls::default();
        let home = Home {
            tonic: 45,
            mode: HomeMode::Minor,
        };
        for degree in 0..7 {
            let Some(current) = diatonic_triad(degree, home, home.tonic + 12) else {
                continue;
            };
            for candidate in all_candidates(home, &current) {
                let slot = slot_with_harmony(&base, &candidate).unwrap();
                let (recovered, _) = chord_from_slot(&slot);
                assert_eq!(recovered.intervals(), candidate.intervals());
            }
        }
    }

    #[test]
    fn evolve_never_returns_the_chord_it_replaces() {
        let base = ChordSlotControls::default();
        let home = Home {
            tonic: 45,
            mode: HomeMode::Minor,
        };
        let Some(current) = diatonic_triad(0, home, home.tonic + 12) else {
            panic!("tonic triad");
        };
        let current_slot = slot_with_harmony(&base, &current).unwrap();
        let ctx = CellContext {
            current: pad_chord_notes_with_slot(&current_slot),
            linked: [None; 6],
        };
        let voice = |candidate: &Harmony| {
            slot_with_harmony(&base, candidate)
                .map_or(ctx.current, |slot| pad_chord_notes_with_slot(&slot))
        };
        let mut rng = StdRng::seed_from_u64(7);
        for _ in 0..50 {
            let next = evolve_harmony(&current, &ctx, home, &mut rng, voice).unwrap();
            assert_ne!(voice(&next), ctx.current);
            assert!(in_mode(home, &next));
        }
    }
}
