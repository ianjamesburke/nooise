//! The live control values themselves: `FluidControls` and the per-voice
//! structs it aggregates, each a plain field bag with a default. Meaning,
//! range, and presentation all live in the registry, never here.

use std::sync::Arc;

use super::module::LayerModules;
use super::voice::{
    CUSTOM_PROGRESSION_INDEX, LeadPattern, PROGRESSIONS, arp_generate_steps,
    arp_pattern_from_control, progression_index,
};

pub(crate) const MASTER_BPM_MIN: f32 = 30.0;
pub(crate) const MASTER_BPM_MAX: f32 = 200.0;
// Short enough to feel instant under a moving slider, long enough to stay
// click-free on gain changes.
pub(crate) const LEVEL_RAMP_MS: f32 = 30.0;

#[derive(Clone)]
pub(crate) struct MasterControls {
    pub(crate) bpm: f32,
    pub(crate) level: f32,
    pub(crate) tone: f32,           // -1 (bass) to +1 (treble)
    pub(crate) key: f32,            // 0 = each progression's own key, 1..=12 = C..B
    pub(crate) swing: f32,          // fallback for voices without a Swing module
    pub(crate) drunken_amount: f32, // 0..1, derived from the Master module
    pub(crate) drunken_pace: f32,   // nominal rise-and-fall length in trigger hits
}

impl Default for MasterControls {
    fn default() -> Self {
        Self {
            bpm: 82.0,
            level: 0.8,
            tone: 0.0,
            key: 0.0,
            swing: 0.0,
            drunken_amount: 0.0,
            drunken_pace: 7.0,
        }
    }
}

pub(crate) const PERC_STEP_COUNT: usize = 16;
pub(crate) const PERC_PATTERN_PRESET_COUNT: usize = 4;
pub(crate) const CONTINUOUS_PERC_PATTERN_INDEX: usize = 4;
pub(crate) const CUSTOM_PERC_PATTERN_INDEX: usize = 5;

pub(crate) const PERC_PATTERNS: &[&str] = &[
    "Steady",
    "Offbeat",
    "Broken",
    "Groove",
    "Continuous",
    "Custom",
];

pub(crate) const DEFAULT_PERC_STEPS: [f32; PERC_STEP_COUNT] = [1.0; PERC_STEP_COUNT];

/// Built-in Perc pattern presets (`Steady`, `Offbeat`, `Broken`, `Groove`).
pub(crate) const PERC_PATTERN_PRESETS: [(u8, [f32; PERC_STEP_COUNT]); PERC_PATTERN_PRESET_COUNT] = [
    (8, DEFAULT_PERC_STEPS),
    (
        8,
        [
            0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0,
        ],
    ),
    (
        8,
        [
            1.0, 0.0, 1.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0, 1.0, 0.0,
        ],
    ),
    (
        16,
        [
            1.0, 0.0, 1.0, 1.0, 1.0, 0.0, 1.0, -1.0, 1.0, 0.0, 1.0, 1.0, 1.0, 0.0, -1.0, 0.0,
        ],
    ),
];

#[derive(Clone)]
pub(crate) struct PercControls {
    pub(crate) level: f32,
    pub(crate) attack_ms: f32,
    pub(crate) decay_ms: f32,
    pub(crate) pattern: f32,
    pub(crate) interval_beats: f32,
    pub(crate) offset_beats: f32,
    pub(crate) step_count: f32,
    pub(crate) evolve_rate: f32,
    pub(crate) steps: [f32; PERC_STEP_COUNT],
    pub(crate) swing: f32, // 0 (straight) to 1 (max shuffle) on this voice's grid
}

impl Default for PercControls {
    fn default() -> Self {
        Self {
            level: 0.0,
            attack_ms: 0.0,
            decay_ms: 200.0,
            pattern: 0.0,
            interval_beats: 0.25,
            offset_beats: 0.0,
            step_count: 8.0,
            evolve_rate: 0.0,
            steps: DEFAULT_PERC_STEPS,
            swing: 0.0,
        }
    }
}

impl PercControls {
    pub(crate) fn apply_pattern_preset(&mut self, pattern_value: f32) {
        let idx =
            (pattern_value.round() as isize).clamp(0, CUSTOM_PERC_PATTERN_INDEX as isize) as usize;
        self.pattern = idx as f32;
        if let Some(&(len, steps)) = PERC_PATTERN_PRESETS.get(idx) {
            self.step_count = len as f32;
            self.steps = steps;
        }
    }
}

/// One editable chord definition. Root keeps its original A-minor editor
/// coordinates; quality chooses the third/suspension, fifth is independent,
/// and two extensions share the source-tone priority. Defaults preserve the
/// existing Custom sound. Registry specs own every field's allowed values.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct ChordSlotControls {
    pub(crate) degree: f32,
    pub(crate) accidental: f32,
    pub(crate) quality: f32,
    pub(crate) extension: f32,
    pub(crate) inversion: f32,
    pub(crate) extension2: f32,
    pub(crate) voicing: f32,
    pub(crate) fifth: f32,
}

impl ChordSlotControls {
    pub(crate) const REST_DEGREE: f32 = -9.0;
    pub(crate) const TIE_DEGREE: f32 = -8.0;

    pub(crate) fn is_rest(self) -> bool {
        (self.degree.round() as i32) <= Self::REST_DEGREE as i32
    }

    pub(crate) fn is_tie(self) -> bool {
        (self.degree.round() as i32) == Self::TIE_DEGREE as i32
    }

    /// Compact authored/wire order: root, accidental, third, extensions,
    /// bass, spread, fifth. All values are exact discrete choices.
    pub(crate) const fn preset(values: [i8; 8]) -> Self {
        Self {
            degree: values[0] as f32,
            accidental: values[1] as f32,
            quality: values[2] as f32,
            extension: values[3] as f32,
            extension2: values[4] as f32,
            inversion: values[5] as f32,
            voicing: values[6] as f32,
            fifth: values[7] as f32,
        }
    }

    pub(crate) fn values(&self) -> [f32; 8] {
        [
            self.degree,
            self.accidental,
            self.quality,
            self.extension,
            self.extension2,
            self.inversion,
            self.voicing,
            self.fifth,
        ]
    }

    pub(crate) fn is_valid(&self) -> bool {
        const IDS: [&str; 8] = [
            "pad.chord1_degree",
            "pad.chord1_accidental",
            "pad.chord1_quality",
            "pad.chord1_extension",
            "pad.chord1_extension2",
            "pad.chord1_inversion",
            "pad.chord1_voicing",
            "pad.chord1_fifth",
        ];
        self.values().into_iter().zip(IDS).all(|(value, id)| {
            value.is_finite()
                && super::registry::spec_by_id(id).is_some_and(|spec| spec.quantize(value) == value)
        })
    }
}

/// Number of chords in each built-in progression table (`PROGRESSIONS`), and the
/// default active length of `pad.chord_count`.
pub(crate) const BUILTIN_PROGRESSION_CHORDS: usize = 8;
pub(crate) const DEFAULT_CHORD_COUNT: usize = 8;

/// Number of chord sequence steps (`PadControls::chord_slots`), and the max of
/// `pad.chord_count`.
pub(crate) const CHORD_SLOT_COUNT: usize = 16;

/// Default per-slot root degrees for a fresh custom progression: a stepwise
/// shape around the tonic so switching into Custom mode is immediately
/// musical rather than identical tonic chords.
pub(crate) const DEFAULT_CHORD_SLOT_DEGREES: [f32; CHORD_SLOT_COUNT] = [
    0.0, -1.0, 0.0, 1.0, 0.0, -1.0, 2.0, 4.0, 0.0, -1.0, 0.0, 1.0, 0.0, -1.0, 2.0, 4.0,
];

/// Audio-only scratch for the currently evaluated progression. Registry
/// automation writes here so its shared authored bank never needs a copy.
#[derive(Clone, Copy)]
pub(crate) struct EvaluatedChords {
    progression: usize,
    slots: [Option<ChordSlotControls>; CHORD_SLOT_COUNT],
}

pub(crate) type ChordOverrideSlots =
    [[Option<ChordSlotControls>; CHORD_SLOT_COUNT]; PROGRESSIONS.len()];

#[derive(Clone)]
pub(crate) struct PadControls {
    pub(crate) level: f32,
    pub(crate) voice_type: f32, // Character selector; labels and range live in PAD_TYPES.
    pub(crate) chord_bars: f32, // 0.0625..=64.0 bars (0.25..=256 beats)
    pub(crate) chord_count: f32,
    /// The table slot the loop starts from, 0..16; `ChordWindow` wraps.
    pub(crate) chord_offset: f32,
    pub(crate) progression: f32,
    pub(crate) chord_notes: f32,
    pub(crate) swing: f32,
    pub(crate) evolve_rate: f32,
    pub(crate) chord_slots: [ChordSlotControls; CHORD_SLOT_COUNT],
    pub(crate) chord_overrides: Arc<ChordOverrideSlots>,
    /// Rebuilt each audio evaluation; excluded from authored song state.
    pub(crate) evaluated_chords: Option<EvaluatedChords>,
    pub(crate) stereo_width: f32,
    pub(crate) detune: f32,
    pub(crate) octave_mix: f32,
    pub(crate) attack_time: f32,
    pub(crate) release_time: f32,
}

impl Default for PadControls {
    fn default() -> Self {
        Self {
            level: 0.7,
            voice_type: 0.0,
            chord_bars: 4.0,
            chord_count: DEFAULT_CHORD_COUNT as f32,
            chord_offset: 0.0,
            progression: 0.0,
            chord_notes: 4.0,
            swing: 0.0,
            evolve_rate: 0.0,
            chord_slots: std::array::from_fn(|slot| ChordSlotControls {
                degree: DEFAULT_CHORD_SLOT_DEGREES[slot],
                ..ChordSlotControls::default()
            }),
            chord_overrides: Arc::new([[None; CHORD_SLOT_COUNT]; PROGRESSIONS.len()]),
            evaluated_chords: None,
            stereo_width: 0.8,
            detune: 0.5,
            octave_mix: 0.5,
            attack_time: 6.0,
            release_time: 8.0,
        }
    }
}

impl PadControls {
    /// Resolve without materializing an override. Editor pitch coordinates
    /// remain distinct from a progression's future explicit home key.
    pub(crate) fn chord_slot(&self, progression: usize, slot: usize) -> &ChordSlotControls {
        let slot = slot % CHORD_SLOT_COUNT;
        if let Some(evaluated) = &self.evaluated_chords
            && evaluated.progression == progression
            && let Some(chord) = &evaluated.slots[slot]
        {
            return chord;
        }
        match PROGRESSIONS.get(progression) {
            Some(authored) => self.chord_overrides[progression][slot]
                .as_ref()
                .unwrap_or(&authored.chords[slot % BUILTIN_PROGRESSION_CHORDS].preset),
            None => &self.chord_slots[slot],
        }
    }

    /// Called before any audio automation setter. Resetting sixteen optional
    /// slots is bounded; subsequent effective edits never mutate the Arc.
    pub(crate) fn begin_chord_evaluation(&mut self) {
        self.evaluated_chords = Some(EvaluatedChords {
            progression: progression_index(self.progression),
            slots: [None; CHORD_SLOT_COUNT],
        });
    }

    pub(crate) fn edit_chord_slot(&mut self, slot: usize) -> &mut ChordSlotControls {
        let progression = progression_index(self.progression);
        let slot = slot % CHORD_SLOT_COUNT;
        let Some(authored) = PROGRESSIONS.get(progression) else {
            return &mut self.chord_slots[slot];
        };
        let preset = self.chord_overrides[progression][slot]
            .unwrap_or(authored.chords[slot % BUILTIN_PROGRESSION_CHORDS].preset);
        if let Some(evaluated) = &mut self.evaluated_chords {
            if evaluated.progression != progression {
                evaluated.progression = progression;
                evaluated.slots = [None; CHORD_SLOT_COUNT];
            }
            evaluated.slots[slot].get_or_insert(preset)
        } else {
            // Only authored UI/codec controls take this path; audio plans
            // enter evaluation before applying effective registry setters.
            Arc::make_mut(&mut self.chord_overrides)[progression][slot].get_or_insert(preset)
        }
    }

    pub(crate) fn chord_is_edited(&self, progression: usize, slot: usize) -> bool {
        self.chord_overrides
            .get(progression)
            .is_some_and(|bank| bank[slot % CHORD_SLOT_COUNT].is_some())
    }

    pub(crate) fn restore_chord_slot(&mut self, progression: usize, slot: usize) -> bool {
        if slot >= CHORD_SLOT_COUNT {
            return false;
        }
        if progression < PROGRESSIONS.len() {
            Arc::make_mut(&mut self.chord_overrides)[progression][slot]
                .take()
                .is_some()
        } else if progression == CUSTOM_PROGRESSION_INDEX {
            let restored = ChordSlotControls {
                degree: DEFAULT_CHORD_SLOT_DEGREES[slot],
                ..ChordSlotControls::default()
            };
            let changed = self.chord_slots[slot] != restored;
            self.chord_slots[slot] = restored;
            changed
        } else {
            false
        }
    }
}

pub(crate) const KICK_STEP_COUNT: usize = 16;
pub(crate) const KICK_PATTERN_PRESET_COUNT: usize = 4;
pub(crate) const CUSTOM_KICK_PATTERN_INDEX: usize = 4;

pub(crate) const KICK_PATTERNS: &[&str] = &["Steady", "Offbeat", "Broken", "Groove", "Custom"];

pub(crate) const DEFAULT_KICK_STEPS: [f32; KICK_STEP_COUNT] = [1.0; KICK_STEP_COUNT];

/// Built-in Kick pattern presets (`Steady`, `Offbeat`, `Broken`, `Groove`).
pub(crate) const KICK_PATTERN_PRESETS: [(u8, [f32; KICK_STEP_COUNT]); KICK_PATTERN_PRESET_COUNT] = [
    (8, DEFAULT_KICK_STEPS),
    (
        8,
        [
            0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0,
        ],
    ),
    (
        8,
        [
            1.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0,
        ],
    ),
    (
        16,
        [
            1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0,
        ],
    ),
];

#[derive(Clone)]
pub(crate) struct KickControls {
    pub(crate) level: f32,
    pub(crate) voice_type: f32, // index into KICK_TYPES; 0=legacy Sub
    pub(crate) start_freq: f32,
    pub(crate) pitch_decay_ms: f32,
    pub(crate) amp_decay_ms: f32,
    pub(crate) click: f32, // 0–0.2 UI range
    pub(crate) pattern: f32,
    pub(crate) interval_beats: f32,
    pub(crate) offset_beats: f32,
    pub(crate) step_count: f32,
    pub(crate) evolve_rate: f32,
    pub(crate) steps: [f32; KICK_STEP_COUNT],
    pub(crate) swing: f32, // 0 (straight) to 1 (max shuffle) on this voice's grid
}

impl Default for KickControls {
    fn default() -> Self {
        Self {
            level: 0.0,
            voice_type: 0.0,
            start_freq: 160.0,
            pitch_decay_ms: 55.0,
            amp_decay_ms: 150.0,
            click: 0.02,
            pattern: 0.0,
            interval_beats: 1.0,
            offset_beats: 0.0,
            step_count: 8.0,
            evolve_rate: 0.0,
            steps: DEFAULT_KICK_STEPS,
            swing: 0.0,
        }
    }
}

impl KickControls {
    pub(crate) fn apply_pattern_preset(&mut self, pattern_value: f32) {
        let idx =
            (pattern_value.round() as isize).clamp(0, CUSTOM_KICK_PATTERN_INDEX as isize) as usize;
        self.pattern = idx as f32;
        if let Some(&(len, steps)) = KICK_PATTERN_PRESETS.get(idx) {
            self.step_count = len as f32;
            self.steps = steps;
        }
    }
}

pub(crate) const TONAL_STEP_COUNT: usize = 16;
pub(crate) const TONAL_PHRASE_PRESET_COUNT: usize = 8;
pub(crate) const CUSTOM_TONAL_PHRASE_INDEX: usize = TONAL_PHRASE_PRESET_COUNT;

/// Built-in Tonal phrase presets A..H (`(step_count, steps)`).
pub(crate) const TONAL_PHRASE_PRESETS: [(u8, [f32; TONAL_STEP_COUNT]); TONAL_PHRASE_PRESET_COUNT] = [
    (
        8,
        [
            1.0, 4.0, 7.0, 3.0, 5.0, 8.0, 4.0, 7.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
        ],
    ),
    (
        12,
        [
            1.0, 5.0, 8.0, 9.0, 8.0, 5.0, 4.0, 3.0, 4.0, 7.0, 5.0, 1.0, 0.0, 0.0, 0.0, 0.0,
        ],
    ),
    (
        8,
        [
            8.0, 9.0, 5.0, 4.0, 9.0, 8.0, 5.0, 7.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
        ],
    ),
    (
        12,
        [
            1.0, 3.0, 5.0, 7.0, 9.0, 8.0, 7.0, 5.0, 4.0, 5.0, 7.0, 3.0, 0.0, 0.0, 0.0, 0.0,
        ],
    ),
    (
        8,
        [
            5.0, 7.0, 9.0, 5.0, 7.0, 5.0, 9.0, 7.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
        ],
    ),
    (
        16,
        [
            1.0, 4.0, 5.0, 7.0, 8.0, 7.0, 5.0, 4.0, 3.0, 4.0, 5.0, 1.0, 7.0, 1.0, 3.0, 4.0,
        ],
    ),
    (
        8,
        [
            9.0, 8.0, 7.0, 5.0, 4.0, 5.0, 7.0, 8.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
        ],
    ),
    (
        16,
        [
            1.0, 3.0, 4.0, 7.0, 5.0, 8.0, 7.0, 9.0, 8.0, 5.0, 9.0, 7.0, 5.0, 9.0, 7.0, 5.0,
        ],
    ),
];

#[derive(Clone)]
pub(crate) struct TonalControls {
    pub(crate) level: f32,
    pub(crate) synth_type: f32,
    pub(crate) octave: f32,
    pub(crate) phrase: f32,
    pub(crate) follow: f32,
    pub(crate) evolve_rate: f32,
    pub(crate) rate_beats: f32,
    pub(crate) offset_beats: f32,
    pub(crate) step_count: f32,
    pub(crate) steps: [f32; TONAL_STEP_COUNT],
    pub(crate) attack: f32,
    pub(crate) decay: f32,
    pub(crate) swing: f32, // 0 (straight) to 1 (max shuffle) on this voice's grid
}

impl Default for TonalControls {
    fn default() -> Self {
        Self {
            level: 0.0,
            synth_type: 0.0,
            octave: 0.0,
            phrase: 0.0,
            follow: 1.0,
            evolve_rate: 0.0,
            rate_beats: 0.5,
            offset_beats: 0.0,
            step_count: TONAL_PHRASE_PRESETS[0].0 as f32,
            steps: TONAL_PHRASE_PRESETS[0].1,
            // Attack + decay are the note's whole envelope: ramp in over
            // `attack`, then fall from the peak to silence over `decay`. The
            // note's sounding length is exactly `attack + decay`, decoupled
            // from the step grid, so `decay` alone sets how long each note
            // rings.
            attack: 0.03,
            decay: 1.2,
            swing: 0.0,
        }
    }
}

impl TonalControls {
    #[cfg(test)]
    pub(crate) fn active_step_count(&self) -> usize {
        (self.step_count.round() as usize).clamp(1, TONAL_STEP_COUNT)
    }

    pub(crate) fn apply_phrase_preset(&mut self, phrase_value: f32) {
        let idx =
            (phrase_value.round() as isize).clamp(0, CUSTOM_TONAL_PHRASE_INDEX as isize) as usize;
        self.phrase = idx as f32;
        if let Some(&(len, steps)) = TONAL_PHRASE_PRESETS.get(idx) {
            self.step_count = len as f32;
            self.steps = steps;
        }
    }
}

#[derive(Clone)]
pub(crate) struct ClapControls {
    pub(crate) level: f32,
    pub(crate) interval_beats: f32,
    pub(crate) offset_beats: f32,
    pub(crate) swing: f32, // 0 (straight) to 1 (max shuffle) on this voice's grid
    pub(crate) slap_count: f32, // 1-8
    pub(crate) slap_spread_ms: f32, // 0-100 ms
    pub(crate) decay_ms: f32, // 10-200 ms
    pub(crate) body: f32,  // 0-1 low-freq flesh mix
}

impl Default for ClapControls {
    fn default() -> Self {
        Self {
            level: 0.0,
            interval_beats: 2.0,
            offset_beats: 1.0,
            swing: 0.0,
            slap_count: 3.0,
            slap_spread_ms: 8.0,
            decay_ms: 40.0,
            body: 0.2,
        }
    }
}

pub(crate) const BASS_STEP_COUNT: usize = 16;
pub(crate) const BASS_RHYTHM_PRESET_COUNT: usize = 4;
pub(crate) const CUSTOM_BASS_RHYTHM_INDEX: usize = BASS_RHYTHM_PRESET_COUNT;

pub(crate) const DEFAULT_BASS_STEPS: [f32; BASS_STEP_COUNT] = [
    1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0,
];

pub(crate) fn bass_rhythm_steps(rhythm: usize) -> [f32; BASS_STEP_COUNT] {
    let flags = super::voice::BASS_RHYTHMS[rhythm % super::voice::BASS_RHYTHMS.len()];
    std::array::from_fn(|idx| if flags[idx] { 1.0 } else { 0.0 })
}

#[derive(Clone)]
pub(crate) struct BassControls {
    pub(crate) level: f32,
    pub(crate) voice_type: f32, // index into BASS_TYPES; 0=Sub is the legacy voice
    pub(crate) punch: f32, // 0..1 onset pitch drop and brightness; 0 leaves every type untouched
    pub(crate) interval_beats: f32, // crops the 16-step rhythm phrase to this many beats (step length is fixed)
    pub(crate) offset_beats: f32,
    pub(crate) rhythm: f32, // 0..=3 A/B/C/D pattern selector, 4=Custom
    pub(crate) follow: f32, // 0=Chord, 1=Scale
    pub(crate) step_count: f32,
    pub(crate) evolve_rate: f32,
    pub(crate) steps: [f32; BASS_STEP_COUNT],
    pub(crate) octave: f32, // octaves relative to the chord root, e.g. -1.0 = one octave down
    pub(crate) attack_time: f32,
    pub(crate) decay_time: f32, // also used as the cutoff curve when a hit retriggers mid-decay
    pub(crate) swing: f32,      // derived from the optional Swing module
}

impl Default for BassControls {
    fn default() -> Self {
        Self {
            level: 0.0,
            voice_type: 0.0,
            punch: 0.0,
            interval_beats: 4.0,
            offset_beats: 0.0,
            rhythm: 0.0,
            follow: 0.0,
            step_count: BASS_STEP_COUNT as f32,
            evolve_rate: 0.0,
            steps: DEFAULT_BASS_STEPS,
            octave: -1.0,
            decay_time: 0.3,
            attack_time: 0.01,
            swing: 0.0,
        }
    }
}

impl BassControls {
    pub(crate) fn apply_rhythm_preset(&mut self, rhythm_value: f32) {
        let idx =
            (rhythm_value.round() as isize).clamp(0, CUSTOM_BASS_RHYTHM_INDEX as isize) as usize;
        self.rhythm = idx as f32;
        if idx < CUSTOM_BASS_RHYTHM_INDEX {
            self.steps = bass_rhythm_steps(idx);
        }
    }
}

pub(crate) const ARP_STEP_COUNT: usize = 16;
pub(crate) const ARP_TONE_COUNT: usize = 12;
pub(crate) const CUSTOM_ARP_PATTERN_INDEX: usize = 4;

pub(crate) const DEFAULT_ARP_STEPS: [f32; ARP_STEP_COUNT] = [
    1.0, 2.0, 3.0, 4.0, 1.0, 2.0, 3.0, 4.0, 1.0, 2.0, 3.0, 4.0, 1.0, 2.0, 3.0, 4.0,
];

#[derive(Clone)]
pub(crate) struct ArpControls {
    pub(crate) gain: f32,
    pub(crate) voice_type: f32, // same Sine/piano-profile set as tonal.synth_type
    pub(crate) rate_beats: f32,
    pub(crate) offset_beats: f32,
    pub(crate) pattern: f32, // 0=Up, 1=Down, 2=Up-Down, 3=Random, 4=Custom
    pub(crate) follow: f32,  // 0=Chord, 1=Scale
    pub(crate) octaves: f32, // 1-3, octave span of the cycled chord tones
    pub(crate) step_count: f32,
    pub(crate) evolve_rate: f32,
    pub(crate) steps: [f32; ARP_STEP_COUNT],
    pub(crate) attack: f32,
    pub(crate) decay: f32,
    pub(crate) swing: f32, // 0 (straight) to 1 (max shuffle) on this voice's grid
}

impl Default for ArpControls {
    fn default() -> Self {
        Self {
            // Silent by default: a new voice must never change the sound of
            // existing songs or a fresh startup.
            gain: 0.0,
            // 6 => the "Pluck" piano profile, matching the arp's former
            // fixed synth character byte-for-byte.
            voice_type: 6.0,
            rate_beats: 0.5,
            offset_beats: 0.0,
            pattern: 0.0,
            follow: 0.0,
            octaves: 1.0,
            step_count: 8.0,
            evolve_rate: 0.0,
            steps: DEFAULT_ARP_STEPS,
            attack: 0.005,
            decay: 0.4,
            swing: 0.0,
        }
    }
}

impl ArpControls {
    pub(crate) fn apply_pattern_preset(&mut self, pattern_value: f32) {
        let idx =
            (pattern_value.round() as isize).clamp(0, CUSTOM_ARP_PATTERN_INDEX as isize) as usize;
        self.pattern = idx as f32;
        if idx < CUSTOM_ARP_PATTERN_INDEX {
            let (len, steps) =
                arp_generate_steps(arp_pattern_from_control(self.pattern), self.octaves);
            self.step_count = len as f32;
            self.steps = steps;
        }
    }

    pub(crate) fn apply_octaves(&mut self, octaves_value: f32) {
        self.octaves = octaves_value.round().clamp(1.0, 3.0);
        let idx =
            (self.pattern.round() as isize).clamp(0, CUSTOM_ARP_PATTERN_INDEX as isize) as usize;
        if idx < CUSTOM_ARP_PATTERN_INDEX {
            let (len, steps) =
                arp_generate_steps(arp_pattern_from_control(self.pattern), self.octaves);
            self.step_count = len as f32;
            self.steps = steps;
        }
    }
}

/// Number of steps in the Lead's pattern lane, and the max of `lead.steps`.
pub(crate) const LEAD_STEP_COUNT: usize = 16;

/// Default Lead pattern: a stepwise line over the chord tones so a fresh
/// Lead is immediately musical once its level comes up. Step values index
/// the Lead's reach (0 = rest, 1..=4 chord tones, 5..=8 an octave up, 9 the
/// root two octaves up; in Scale follow the same nine walk the progression's
/// scale). Only the default live length is authored; every step past it
/// rests, so lengthening the lane adds silence to fill in, not notes to
/// remove.
pub(crate) const DEFAULT_LEAD_STEPS: [f32; LEAD_STEP_COUNT] = [
    1.0, 2.0, 3.0, 5.0, 4.0, 3.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
];

#[derive(Clone)]
pub(crate) struct LeadControls {
    pub(crate) level: f32,
    pub(crate) attack: f32,
    pub(crate) decay: f32,
    pub(crate) glide: f32,      // seconds for a pitch change to settle
    pub(crate) voice_type: f32, // character selector; labels and range live in LEAD_TYPES
    pub(crate) octave: f32,     // whole octaves shifting every played tone
    pub(crate) follow: f32,     // what the tones index: labels and range live in LEAD_FOLLOWS
    pub(crate) rate_beats: f32,
    pub(crate) offset_beats: f32,
    pub(crate) step_count: f32, // 1..=LEAD_STEP_COUNT live steps in the lane
    pub(crate) evolve_rate: f32,
    pub(crate) pattern: f32, // lane transport: labels and range live in LEAD_PATTERNS
    pub(crate) steps: [f32; LEAD_STEP_COUNT],
    pub(crate) swing: f32, // derived from the optional Swing module
}

impl Default for LeadControls {
    fn default() -> Self {
        Self {
            // Silent by default: a new voice must never change the sound of
            // existing songs or a fresh startup.
            level: 0.0,
            attack: 0.02,
            decay: 0.5,
            // Settled within a 32nd at 82 BPM: audible as a slide between
            // held notes, invisible as lag under a played line.
            glide: 0.03,
            voice_type: 2.0,
            octave: 0.0,
            follow: 0.0,
            rate_beats: 0.5,
            offset_beats: 0.0,
            step_count: 8.0,
            evolve_rate: 0.0,
            // Play, so a code from before the transport existed still runs
            // its lane.
            pattern: LeadPattern::Play.value(),
            steps: DEFAULT_LEAD_STEPS,
            swing: 0.0,
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct FluidControls {
    pub(crate) master: MasterControls,
    pub(crate) perc: PercControls,
    pub(crate) pad: PadControls,
    pub(crate) kick: KickControls,
    pub(crate) tonal: TonalControls,
    pub(crate) clap: ClapControls,
    pub(crate) bass: BassControls,
    pub(crate) arp: ArpControls,
    pub(crate) lead: LeadControls,
    /// Per-layer module chains, including the factory presets defined by
    /// `LayerModules::default`.
    pub(crate) modules: LayerModules,
}
