//! Shared allocation-free chord builder, pitch spelling, and chord identity.
//! Root degrees retain their established A-minor editor coordinates. They
//! do not declare or infer the song's future home tonic and mode.

use super::*;

fn shift_diatonic(note: i32, steps: i32) -> i32 {
    const SCALE: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];
    let degree = note.div_euclid(12) * 7
        + SCALE
            .iter()
            .position(|pc| *pc == note.rem_euclid(12))
            .unwrap_or(0) as i32;
    let shifted = degree + steps;
    shifted.div_euclid(7) * 12 + SCALE[shifted.rem_euclid(7) as usize]
}

fn natural_root(slot: &ChordSlotControls) -> i32 {
    shift_diatonic(45, slot.degree.round().clamp(-7.0, 7.0) as i32)
}

pub(crate) fn pad_chord_root_note(slot: &ChordSlotControls) -> i32 {
    natural_root(slot) + slot.accidental.round().clamp(-1.0, 1.0) as i32
}

fn third_interval(slot: &ChordSlotControls) -> Option<i32> {
    Some(match slot.quality.round() as i32 {
        -1 => 3,
        1 => 4,
        2 => 2,
        3 => 5,
        4 => return None,
        _ => shift_diatonic(natural_root(slot), 2) - natural_root(slot),
    })
}

pub(crate) fn chord_fifth_interval(slot: &ChordSlotControls) -> Option<i32> {
    match slot.fifth.round() as i32 {
        2 => None,
        1 => Some(7),
        _ if slot.quality >= 1.5 => Some(7),
        _ => Some(shift_diatonic(natural_root(slot), 4) - natural_root(slot)),
    }
}

pub(crate) fn pad_chord_slot_is_minor(slot: &ChordSlotControls) -> bool {
    third_interval(slot) == Some(3)
}

fn extension_interval(slot: &ChordSlotControls, value: f32) -> Option<i32> {
    Some(match value.round() as i32 {
        1 => shift_diatonic(natural_root(slot), 6) - natural_root(slot),
        2 => shift_diatonic(natural_root(slot), 8) - natural_root(slot),
        3 => shift_diatonic(natural_root(slot), 10) - natural_root(slot),
        4 => 10,
        5 => 11,
        6 => 14,
        7 => 17,
        8 => 18,
        9 => 21,
        _ => return None,
    })
}

/// Four distinct source roles at most: root, third/suspension, extensions,
/// fifth. Duplicate pitch classes and a conflicting second seventh are
/// omitted; spare voices double the lowest selected tones by octaves.
fn source_intervals(slot: &ChordSlotControls) -> ([i32; 4], usize) {
    let first = extension_interval(slot, slot.extension);
    let second = extension_interval(slot, slot.extension2).filter(|value| {
        !(matches!(first, Some(10 | 11)) && matches!(value, 10 | 11) && first != Some(*value))
    });
    let mut roles = [0; 4];
    let mut count = 0;
    for role in [
        Some(0),
        third_interval(slot),
        first,
        second,
        chord_fifth_interval(slot),
    ]
    .into_iter()
    .flatten()
    {
        if !roles[..count]
            .iter()
            .any(|n| (n - role).rem_euclid(12) == 0)
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

pub(crate) fn pad_chord_notes_with_slot(slot: &ChordSlotControls) -> [i32; 4] {
    let root = pad_chord_root_note(slot);
    let (mut roles, count) = source_intervals(slot);
    let inversion = slot.inversion.round().clamp(0.0, 7.0) as usize;
    let voicing = slot.voicing.round().clamp(0.0, 4.0) as usize;
    let mut notes = if inversion < 4 {
        roles[..count].sort_unstable();
        let mut notes = roles;
        let mut len = count;
        if voicing == 0 {
            fill_octaves(&mut notes, &mut len);
        }
        for _ in 0..inversion {
            notes[0] += 12;
            notes[..len].sort_unstable();
        }
        fill_octaves(&mut notes, &mut len);
        notes.map(|note| note + root)
    } else {
        let bass = root + [2, 5, 6, 10][inversion - 4];
        let count = count.min(3);
        let mut notes = [bass; 4];
        let mut next = bass + 6;
        for note in &mut notes[1..] {
            while !roles[..count]
                .iter()
                .any(|role| (next - root - role).rem_euclid(12) == 0)
            {
                next += 1;
            }
            *note = next;
            next += 1;
        }
        notes
    };
    match voicing {
        0 => {
            notes.sort_unstable();
            // This established Close behavior is audible in existing Custom
            // songs: a unison inversion climbs the editor's diatonic scale.
            for index in 1..4 {
                while notes[index] <= notes[index - 1] {
                    notes[index] = shift_diatonic(notes[index], 1);
                }
            }
            return notes;
        }
        1 => notes[1] += 12,
        2 => {
            let bass = notes[0];
            let mut upper = [0; 4];
            let mut len = 0;
            for note in &notes[1..] {
                let pc = note.rem_euclid(12);
                if pc != bass.rem_euclid(12) && !upper[..len].contains(&pc) {
                    upper[len] = pc;
                    len += 1;
                }
            }
            if len < 3 {
                upper[len] = bass.rem_euclid(12);
                len += 1;
            }
            for note in &mut upper[..len] {
                *note = bass + 12 + (*note - bass).rem_euclid(12);
            }
            upper[..len].sort_unstable();
            let mut index = 0;
            while len < 3 {
                upper[len] = upper[index] + 12;
                len += 1;
                index += 1;
            }
            notes = [bass, upper[0], upper[1], upper[2]];
        }
        3 => {
            notes[1] += 12;
            notes[2] += 12;
        }
        _ => {}
    }
    notes.sort_unstable();
    for index in 1..4 {
        while notes[index] <= notes[index - 1] {
            notes[index] += 12;
        }
    }
    notes
}

fn fill_octaves(notes: &mut [i32; 4], len: &mut usize) {
    let mut source = 0;
    while *len < 4 {
        notes[*len] = notes[source] + 12;
        *len += 1;
        source += 1;
    }
    notes.sort_unstable();
}

pub(crate) fn chord_pitch_name(note: i32) -> &'static str {
    [
        "C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B",
    ][note.rem_euclid(12) as usize]
}

/// Name the four source notes, including tones introduced by Close's
/// inversion/doubling rule. Output-count thinning does not rename harmony.
pub(crate) fn custom_chord_name(slot: &ChordSlotControls) -> String {
    let root = pad_chord_root_note(slot);
    let notes = pad_chord_notes_with_slot(slot);
    let has = |interval| {
        notes
            .iter()
            .any(|note| (note - root).rem_euclid(12) == interval)
    };
    let third = if has(3) {
        "m"
    } else if has(4) {
        ""
    } else if slot.quality.round() == 3.0 && has(5) {
        "sus4"
    } else if has(2) {
        "sus2"
    } else if has(5) {
        "sus4"
    } else {
        "5"
    };
    let (roles, role_count) = source_intervals(slot);
    let diminished = chord_fifth_interval(slot) == Some(6)
        && roles[..role_count].contains(&6)
        && has(6)
        && !has(7);
    let seventh = if has(11) {
        "maj7"
    } else if has(10) {
        "7"
    } else {
        ""
    };
    let mut body = if seventh.is_empty() {
        if diminished && third == "m" {
            "dim".to_string()
        } else {
            third.to_string()
        }
    } else {
        match third {
            "" => seventh.to_string(),
            "m" if diminished && seventh == "7" => "m7b5".to_string(),
            "m" if seventh == "maj7" => "m(maj7)".to_string(),
            "m" => format!("m{seventh}"),
            "5" => format!("{seventh}(no3)"),
            _ => format!("{seventh}{third}"),
        }
    };
    for (interval, label) in [
        (1, "b9"),
        (2, "9"),
        (5, "11"),
        (6, "#11"),
        (8, "b13"),
        (9, "13"),
    ] {
        if !has(interval)
            || (interval == 2 && third == "sus2")
            || (interval == 5 && third == "sus4")
            || (interval == 6 && diminished)
        {
            continue;
        }
        if seventh.is_empty() && interval == 9 && matches!(third, "" | "m") {
            body.push('6');
        } else {
            body.push_str("add");
            body.push_str(label);
        }
    }
    if diminished && !matches!(third, "m") {
        body.push_str("b5");
    }
    let mut name = format!("{}{body}", chord_pitch_name(root));
    if (notes[0] - root).rem_euclid(12) != 0 {
        name.push('/');
        name.push_str(chord_pitch_name(notes[0]));
    }
    name
}
