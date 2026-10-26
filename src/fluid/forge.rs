//! Song authoring: chord names in, an `n1_` song code out.
//!
//! The engine's chord coordinates are A-minor editor degrees, not note names,
//! so authoring a song used to mean a throwaway Rust harness around
//! `encode_song_code`. This is the missing half of that path: a progression
//! written the way a musician writes it, plus the rhythmic backbone, becomes a
//! code that loads like any other.
//!
//! The output is the chord backbone only -- harmony, tempo, harmonic rhythm,
//! bass rhythm, and at most one arp figure. Drums, lead, and the tonal lane are
//! left at their defaults for the player to add in the TUI.

use super::*;

/// What `make-song` can author, kept as the CLI spells it so there is one
/// vocabulary rather than a flag name and a field name that drift apart.
pub(crate) struct SongSpec {
    pub(crate) bpm: f32,
    pub(crate) chords: String,
    pub(crate) chord_bars: f32,
    pub(crate) bass: String,
    pub(crate) arp: String,
    pub(crate) arp_rate_beats: f32,
    pub(crate) arp_octaves: f32,
    pub(crate) attack: f32,
    pub(crate) release: f32,
}

/// A chord written the way a musician writes it. `dim` and `aug` are absent on
/// purpose: the engine's `fifth` has only perfect-or-diatonic settings, so
/// neither can be spelled. `make_song_code` refuses them rather than voicing
/// something else and calling it diminished.
struct Chord {
    /// Index into the A-minor letter ladder: A=0, B=1, C=2, ... G=6.
    letter: i32,
    accidental: f32,
    quality: f32,
    fifth: f32,
    extension: f32,
    extension2: f32,
}

impl Chord {
    fn root_note(&self, degree: i32) -> i32 {
        pad_chord_root_note(&self.slot(degree))
    }

    fn slot(&self, degree: i32) -> ChordSlotControls {
        ChordSlotControls {
            degree: degree as f32,
            accidental: self.accidental,
            quality: self.quality,
            fifth: self.fifth,
            extension: self.extension,
            extension2: self.extension2,
            // Stack: the close voicing climbs a colliding voice up the diatonic
            // scale and invents non-chord tones, so authored songs avoid it.
            voicing: 4.0,
            ..ChordSlotControls::default()
        }
    }
}

/// Suffix table, matched longest-first so `maj7` never reads as `m` plus
/// `aj7`. Fields are (quality, fifth, first extension, second extension).
const SUFFIXES: &[(&str, f32, f32, f32, f32)] = &[
    ("maj9", 1.0, 1.0, 5.0, 6.0),
    ("sus2", 2.0, 1.0, 0.0, 0.0),
    ("sus4", 3.0, 1.0, 0.0, 0.0),
    ("add9", 1.0, 1.0, 0.0, 6.0),
    ("maj7", 1.0, 1.0, 5.0, 0.0),
    ("min7", -1.0, 1.0, 4.0, 0.0),
    ("min", -1.0, 1.0, 0.0, 0.0),
    ("maj", 1.0, 1.0, 0.0, 0.0),
    ("sus", 3.0, 1.0, 0.0, 0.0),
    ("m9", -1.0, 1.0, 4.0, 6.0),
    ("m7", -1.0, 1.0, 4.0, 0.0),
    ("9", 1.0, 1.0, 4.0, 6.0),
    ("5", 4.0, 1.0, 0.0, 0.0),
    ("7", 1.0, 1.0, 4.0, 0.0),
    ("m", -1.0, 1.0, 0.0, 0.0),
    ("", 1.0, 1.0, 0.0, 0.0),
];

fn parse_chord(text: &str) -> Result<Chord, String> {
    let mut chars = text.chars();
    let letter = match chars.next().map(|c| c.to_ascii_uppercase()) {
        Some('A') => 0,
        Some('B') => 1,
        Some('C') => 2,
        Some('D') => 3,
        Some('E') => 4,
        Some('F') => 5,
        Some('G') => 6,
        _ => return Err(format!("`{text}` does not start with a note letter A-G")),
    };
    let mut rest = chars.as_str();
    let accidental = if let Some(r) = rest.strip_prefix('#') {
        rest = r;
        1.0
    } else if let Some(r) = rest.strip_prefix('b') {
        rest = r;
        -1.0
    } else {
        0.0
    };
    let suffix = rest.to_ascii_lowercase();
    let matched = SUFFIXES
        .iter()
        .filter(|(name, ..)| suffix.starts_with(name))
        .max_by_key(|(name, ..)| name.len());
    let Some((name, quality, fifth, extension, extension2)) = matched else {
        return Err(format!(
            "`{text}` is not a chord this engine can spell; try a plain triad, m, 7, maj7, m7, sus2, sus4, add9, or 5"
        ));
    };
    if name.len() != suffix.len() {
        return Err(format!(
            "`{text}` ends in `{}`, which is not a supported chord suffix",
            &suffix[name.len()..]
        ));
    }
    Ok(Chord {
        letter,
        accidental,
        quality: *quality,
        fifth: *fifth,
        extension: *extension,
        extension2: *extension2,
    })
}

/// Where each chord's root sits. Degrees are A-minor editor coordinates clamped
/// to -7..=7, so every letter has only two reachable octaves (A has three).
/// Pick the one that keeps the root inside the pad's comfortable band, breaking
/// ties toward the previous root so the bass does not leap.
fn place_roots(chords: &[Chord]) -> Vec<i32> {
    // Wide enough that the nearest root usually wins on motion alone, which
    // keeps a descending bass line (C-A-F-G) descending instead of bouncing a
    // sixth to stay inside a narrow window. Restricting it forces the pad into
    // a tighter chord register at the cost of the bass line.
    const LOW: i32 = 43;
    const HIGH: i32 = 59;
    let mut previous: Option<i32> = None;
    chords
        .iter()
        .map(|chord| {
            let mut best: Option<(i32, i32)> = None;
            for octave in -2..=2 {
                let degree = chord.letter + octave * 7;
                if !(-7..=7).contains(&degree) {
                    continue;
                }
                let root = chord.root_note(degree);
                let outside = if root < LOW {
                    LOW - root
                } else if root > HIGH {
                    root - HIGH
                } else {
                    0
                };
                let motion = previous.map_or(0, |p| (root - p).abs());
                // Band membership dominates; motion only breaks near-ties.
                let score = outside * 100 + motion;
                if best.is_none_or(|(_, s)| score < s) {
                    best = Some((degree, score));
                }
            }
            let degree = best.expect("every letter has a reachable octave").0;
            previous = Some(chord.root_note(degree));
            degree
        })
        .collect()
}

fn preset_index(name: &str, table: &[&str], what: &str) -> Result<usize, String> {
    let wanted = name.trim().to_ascii_lowercase();
    table
        .iter()
        .position(|candidate| *candidate == wanted)
        .ok_or_else(|| {
            format!(
                "`{name}` is not a {what}; choose one of {}",
                table
                    .iter()
                    .map(|c| format!("`{c}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
}

fn note_name(midi: i32) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!(
        "{}{}",
        NAMES[midi.rem_euclid(12) as usize],
        midi.div_euclid(12) - 1
    )
}

/// Build the code and a human summary of what it will play. `--show` prints the
/// summary so the realized register can be checked without auditioning.
pub(crate) fn make_song_code(spec: &SongSpec) -> Result<(String, String), String> {
    if !spec.bpm.is_finite() || spec.bpm <= 0.0 {
        return Err("--bpm must be a positive number".into());
    }
    if !spec.chord_bars.is_finite() || !(0.0625..=64.0).contains(&spec.chord_bars) {
        return Err("--chord-bars must be between 0.0625 and 64".into());
    }
    let names: Vec<&str> = spec
        .chords
        .split([',', ' ', '|', '-'])
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect();
    if names.is_empty() {
        return Err("--chords needs at least one chord".into());
    }
    if names.len() > CHORD_SLOT_COUNT {
        return Err(format!(
            "--chords holds at most {CHORD_SLOT_COUNT} chords, got {}",
            names.len()
        ));
    }
    let chords: Vec<Chord> = names
        .iter()
        .map(|name| parse_chord(name))
        .collect::<Result<_, _>>()?;
    let degrees = place_roots(&chords);

    let mut controls = FluidControls::default();
    controls.master.bpm = spec.bpm;
    controls.pad.progression = CUSTOM_PROGRESSION_INDEX as f32;
    controls.pad.chord_count = chords.len() as f32;
    controls.pad.chord_bars = spec.chord_bars;
    controls.pad.attack_time = spec.attack;
    controls.pad.release_time = spec.release;
    for slot in controls.pad.chord_slots.iter_mut().skip(chords.len()) {
        // Silent, so raising Chord Count in the TUI cannot reveal filler.
        *slot = ChordSlotControls {
            gate: ChordSlotControls::GATE_REST,
            ..*slot
        };
    }

    let mut summary = format!(
        "{:.2} bpm  ·  {} chords  ·  {:.4} bars each ({:.2} beats)\n",
        spec.bpm,
        chords.len(),
        spec.chord_bars,
        spec.chord_bars * 4.0
    );
    for (index, (chord, degree)) in chords.iter().zip(&degrees).enumerate() {
        let slot = chord.slot(*degree);
        controls.pad.chord_slots[index] = slot;
        let notes = pad_chord_notes_with_slot(&slot);
        let spelled = notes
            .iter()
            .map(|note| note_name(*note))
            .collect::<Vec<_>>()
            .join(" ");
        summary.push_str(&format!(
            "  {:>2}  {:<6} root {:<4} {}\n",
            index + 1,
            names[index],
            note_name(pad_chord_root_note(&slot)),
            spelled
        ));
    }

    let off = "off".to_string();
    let bass_name = if spec.bass.trim().is_empty() {
        off.clone()
    } else {
        spec.bass.trim().to_ascii_lowercase()
    };
    if bass_name == "off" {
        controls.bass.level = 0.0;
        summary.push_str("bass: off\n");
    } else {
        let rhythm = preset_index(
            &bass_name,
            &["quarter", "syncopated", "eighths", "sixteenths"],
            "bass rhythm",
        )?;
        controls.bass.apply_rhythm_preset(rhythm as f32);
        // The rhythm presets assume 16 steps over 4 beats; the ladder roots sit
        // low enough that the default -1 octave drops them below the speakers.
        controls.bass.interval_beats = 0.25;
        controls.bass.octave = 0.0;
        controls.bass.follow = 0.0;
        controls.bass.level = 0.5;
        summary.push_str(&format!("bass: {bass_name} on the chord root\n"));
    }

    let arp_name = if spec.arp.trim().is_empty() {
        off.clone()
    } else {
        spec.arp.trim().to_ascii_lowercase()
    };
    if arp_name == "off" {
        controls.arp.gain = 0.0;
        summary.push_str("arp: off\n");
    } else {
        let pattern = preset_index(
            &arp_name,
            &["up", "down", "up-down", "random"],
            "arp figure",
        )?;
        if !spec.arp_rate_beats.is_finite() || spec.arp_rate_beats <= 0.0 {
            return Err("--arp-rate must be a positive number of beats".into());
        }
        controls.arp.octaves = spec.arp_octaves;
        controls.arp.apply_octaves(spec.arp_octaves);
        controls.arp.apply_pattern_preset(pattern as f32);
        controls.arp.rate_beats = spec.arp_rate_beats;
        controls.arp.follow = 0.0;
        controls.arp.gain = 0.45;
        summary.push_str(&format!(
            "arp: {arp_name} · {:.2} beats per step · {:.0} octave{}\n",
            spec.arp_rate_beats,
            spec.arp_octaves,
            if spec.arp_octaves == 1.0 { "" } else { "s" }
        ));
    }

    let code = encode_song_code(&SongState {
        controls,
        ..Default::default()
    })
    .map_err(|error| error.to_string())?;
    Ok((code, summary))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(chords: &str) -> SongSpec {
        SongSpec {
            bpm: 100.0,
            chords: chords.into(),
            chord_bars: 1.0,
            bass: "quarter".into(),
            arp: "off".into(),
            arp_rate_beats: 0.5,
            arp_octaves: 1.0,
            attack: 0.4,
            release: 1.2,
        }
    }

    #[test]
    fn chord_names_decode_to_the_intended_pitch_classes() {
        for (name, want) in [
            ("C", vec![0, 4, 7]),
            ("Cm", vec![0, 3, 7]),
            ("C7", vec![0, 4, 7, 10]),
            ("Cmaj7", vec![0, 4, 7, 11]),
            ("Cm7", vec![0, 3, 7, 10]),
            ("Csus4", vec![0, 5, 7]),
            ("Csus2", vec![0, 2, 7]),
            ("F#m", vec![1, 6, 9]),
            ("Bb", vec![2, 5, 10]),
        ] {
            let chord = parse_chord(name).unwrap();
            // The letter only picks the octave; the degree carries the note, so
            // the placement has to run before the chord can be voiced.
            let degree = place_roots(std::slice::from_ref(&chord))[0];
            let slot = chord.slot(degree);
            let mut got: Vec<i32> = pad_chord_notes_with_slot(&slot)
                .iter()
                .map(|note| note.rem_euclid(12))
                .collect();
            got.sort_unstable();
            got.dedup();
            assert_eq!(got, want, "{name}");
        }
    }

    #[test]
    fn unsupported_suffixes_are_refused_rather_than_respelled() {
        for name in ["Cdim", "Caug", "C11", "Hmaj", "Czz"] {
            assert!(parse_chord(name).is_err(), "{name} should be refused");
        }
    }

    #[test]
    fn roots_stay_in_the_pad_band_and_do_not_leap() {
        let chords: Vec<Chord> = ["Em", "Cmaj7", "G", "D"]
            .iter()
            .map(|name| parse_chord(name).unwrap())
            .collect();
        let roots: Vec<i32> = place_roots(&chords)
            .iter()
            .map(|degree| chords[0].root_note(*degree))
            .collect();
        assert!(
            roots.iter().all(|root| (43..=59).contains(root)),
            "roots escaped the band: {roots:?}"
        );
    }

    #[test]
    fn a_song_code_round_trips_through_the_real_decoder() {
        let (code, summary) = make_song_code(&spec("Em Cmaj7 G D")).unwrap();
        let back = decode_song_code(&code).expect("the code we emit must decode");
        assert!((back.controls.master.bpm - 100.0).abs() < 0.01);
        assert_eq!(back.controls.pad.chord_count, 4.0);
        for slot in 4..CHORD_SLOT_COUNT {
            assert!(
                back.controls.pad.chord_slots[slot].is_rest(),
                "unused slot {slot} would sound if Chord Count were raised"
            );
        }
        assert!(summary.contains("Cmaj7"), "{summary}");
    }

    #[test]
    fn bad_preset_names_and_limits_are_refused() {
        assert!(make_song_code(&spec("C")).is_ok());
        let mut s = spec("C");
        s.bass = "walking".into();
        assert!(s.bass == "walking" && make_song_code(&s).is_err());
        let mut s = spec("C");
        s.chord_bars = 0.0;
        assert!(make_song_code(&s).is_err());
        let mut s = spec("C");
        s.arp = "sideways".into();
        assert!(s.arp == "sideways" && make_song_code(&s).is_err());
    }
}
