use super::*;

// ============================================================
// Pad engine (chord drones)
// ============================================================

pub(crate) const MAX_PAD_LAYERS: usize = 4;

pub(crate) struct PadEngine {
    pub(crate) sample_rate: f32,
    pub(crate) layers: Vec<PadLayer>,
    pub(crate) chord_trigger: GridTrigger,
    pub(crate) step_index: usize,
    pub(crate) last_progression: usize,
    pub(crate) last_chord_count: usize,
    pub(crate) last_chord_notes: [i32; 4],
    pub(crate) width_lfo: DriftingLfo,
    pub(crate) air: WhiteNoise,
    pub(crate) rng: StdRng,
    pub(crate) telemetry: Arc<FluidTelemetry>,
}

impl PadEngine {
    pub(crate) fn new(sample_rate: f32, c: &PadControls, telemetry: Arc<FluidTelemetry>) -> Self {
        let initial_notes = pad_chord_midi(0, 0);
        Self {
            sample_rate,
            layers: vec![PadLayer::new(
                pad_type_index(c.voice_type),
                initial_notes,
                0.0,
                sample_rate,
                c.attack_time,
                c.release_time,
            )],
            chord_trigger: GridTrigger::after_start(),
            step_index: 0,
            last_progression: 0,
            last_chord_count: 8,
            last_chord_notes: initial_notes,
            width_lfo: DriftingLfo::new(1.0 / 54.0, sample_rate),
            air: WhiteNoise::new(),
            rng: StdRng::from_entropy(),
            telemetry,
        }
    }

    pub(crate) fn next(&mut self, c: &PadControls, tune: f32, timing: TimingContext) -> (f32, f32) {
        let progression = progression_index(c.progression);
        let chord_count = pad_chord_count(c);
        let progression_changed = progression != self.last_progression;
        let chord_count_changed = chord_count != self.last_chord_count;
        self.last_progression = progression;
        self.last_chord_count = chord_count;
        if self.step_index >= chord_count {
            self.step_index = 0;
        }

        let advance = self.chord_trigger.pop(timing, c.chord_bars * 4.0, 0.0);
        if advance {
            self.step_index = (self.step_index + 1) % chord_count;
        }
        let chord_notes = pad_chord_tones(c, self.step_index);
        let chord_edited = chord_notes != self.last_chord_notes;
        self.last_chord_notes = chord_notes;

        if advance || progression_changed || chord_count_changed || chord_edited {
            for layer in &mut self.layers {
                layer.release();
            }
            self.telemetry
                .chord_index
                .store(self.step_index as u64, Ordering::Relaxed);
            if self.layers.len() >= MAX_PAD_LAYERS {
                let remove_count = self.layers.len() + 1 - MAX_PAD_LAYERS;
                self.layers.drain(0..remove_count);
            }
            self.layers.push(PadLayer::new(
                pad_type_index(c.voice_type),
                chord_notes,
                tune,
                self.sample_rate,
                c.attack_time,
                c.release_time,
            ));
        }

        let width = c.stereo_width
            * (0.58
                + normalized_lfo(self.width_lfo.next(&mut self.rng, 1.0 / 86.0, 1.0 / 38.0))
                    * 0.16);
        let detune_mix = c.detune * 0.84;
        let octave_mix = c.octave_mix * 0.32;

        let mut dry_l = 0.0f32;
        let mut dry_r = 0.0f32;
        for layer in &mut self.layers {
            let (l, r) = layer.next_stereo(width, detune_mix, octave_mix);
            dry_l += l;
            dry_r += r;
        }
        self.layers.retain(|l| !l.is_done());

        let air = self.air.next_filtered(&mut self.rng, 0.0002) * 0.00025;

        (
            (dry_l * 0.72 + air) * c.level,
            (dry_r * 0.72 + air) * c.level,
        )
    }
}

pub(crate) struct PadLayer {
    pub(crate) tones: Vec<PadTone>,
}

impl PadLayer {
    pub(crate) fn new(
        character: usize,
        notes: [i32; 4],
        tune: f32,
        sample_rate: f32,
        attack_time: f32,
        release_time: f32,
    ) -> Self {
        Self {
            tones: pad_tones(
                character,
                notes,
                tune,
                sample_rate,
                attack_time,
                release_time,
            ),
        }
    }
    pub(crate) fn next_stereo(
        &mut self,
        width: f32,
        detune_mix: f32,
        octave_mix: f32,
    ) -> (f32, f32) {
        let (mut l, mut r) = (0.0f32, 0.0f32);
        for t in &mut self.tones {
            let (tl, tr) = t.next_stereo(width, detune_mix, octave_mix);
            l += tl;
            r += tr;
        }
        (l, r)
    }
    pub(crate) fn release(&mut self) {
        for t in &mut self.tones {
            t.release();
        }
    }
    pub(crate) fn is_done(&self) -> bool {
        self.tones.iter().all(PadTone::is_done)
    }
}

/// `pad.type` selects the tone character used for every layer's tones.
/// Index 0 (`Warm`) is the legacy tone, byte-for-byte unchanged and the
/// default; switching type never touches the shared chord/progression
/// (`pad_chord`), trigger timing, attack/release, or pan authoring above.
pub(crate) enum PadTone {
    Warm(WarmPadTone),
    Dark(DarkPadTone),
    Glass(GlassPadTone),
}

impl PadTone {
    pub(crate) fn new(
        character: usize,
        hz: f32,
        pan: f32,
        gain: f32,
        attack_time: f32,
        release_time: f32,
        sample_rate: f32,
    ) -> Self {
        match character {
            0 => Self::Warm(WarmPadTone::new(
                hz,
                pan,
                gain,
                attack_time,
                release_time,
                sample_rate,
            )),
            1 => Self::Dark(DarkPadTone::new(
                hz,
                pan,
                gain,
                attack_time,
                release_time,
                sample_rate,
            )),
            _ => Self::Glass(GlassPadTone::new(
                hz,
                pan,
                gain,
                attack_time,
                release_time,
                sample_rate,
            )),
        }
    }

    pub(crate) fn next_stereo(
        &mut self,
        width: f32,
        detune_mix: f32,
        octave_mix: f32,
    ) -> (f32, f32) {
        match self {
            Self::Warm(tone) => tone.next_stereo(width, detune_mix, octave_mix),
            Self::Dark(tone) => tone.next_stereo(width, detune_mix, octave_mix),
            Self::Glass(tone) => tone.next_stereo(width, detune_mix, octave_mix),
        }
    }

    pub(crate) fn release(&mut self) {
        match self {
            Self::Warm(tone) => tone.release(),
            Self::Dark(tone) => tone.release(),
            Self::Glass(tone) => tone.release(),
        }
    }

    pub(crate) fn is_done(&self) -> bool {
        match self {
            Self::Warm(tone) => tone.is_done(),
            Self::Dark(tone) => tone.is_done(),
            Self::Glass(tone) => tone.is_done(),
        }
    }
}

/// Type 0 (default): the original pad tone, byte-for-byte unchanged. Three
/// sines (fundamental, slightly detuned, one octave up) summed, soft-clipped,
/// and shaped by the shared ADSR.
pub(crate) struct WarmPadTone {
    pub(crate) primary: SineOscillator,
    pub(crate) detuned: SineOscillator,
    pub(crate) octave: SineOscillator,
    pub(crate) envelope: Adsr,
    pub(crate) pan: f32,
    pub(crate) gain: f32,
}

impl WarmPadTone {
    pub(crate) fn new(
        hz: f32,
        pan: f32,
        gain: f32,
        attack_time: f32,
        release_time: f32,
        sample_rate: f32,
    ) -> Self {
        Self {
            primary: SineOscillator::new(hz, sample_rate),
            detuned: SineOscillator::new(hz * 1.003, sample_rate),
            octave: SineOscillator::new(hz * 2.0, sample_rate),
            envelope: Adsr::new(attack_time, 12.0, 0.86, release_time, sample_rate),
            pan,
            gain,
        }
    }
    pub(crate) fn next_stereo(
        &mut self,
        width: f32,
        detune_mix: f32,
        octave_mix: f32,
    ) -> (f32, f32) {
        let s = self.primary.next()
            + self.detuned.next() * detune_mix
            + self.octave.next() * octave_mix;
        let shaped = soft_clip(s * 0.55) * self.envelope.next() * self.gain;
        StereoPanner::equal_power(shaped, self.pan * width)
    }
    pub(crate) fn release(&mut self) {
        self.envelope.note_off();
    }
    pub(crate) fn is_done(&self) -> bool {
        self.envelope.is_done()
    }
}

/// One-pole lowpass coefficient shared by the Dark tone's per-sample
/// smoothing; low enough to noticeably round off the upper harmonic content
/// contributed by the detune/octave layers without muffling the fundamental.
const PAD_DARK_LOWPASS_COEFF: f32 = 0.18;
/// Output trim compensating for the lowpass stage's energy loss so Dark sits
/// at a comparable perceived level to Warm/Glass.
const PAD_DARK_OUTPUT_GAIN: f32 = 1.22;

/// Type 1: a darker, filtered character. Identical oscillator stack to Warm,
/// but the summed signal passes through a gentle fixed one-pole lowpass
/// before soft-clipping, rounding off the highs contributed by the detune and
/// octave layers.
pub(crate) struct DarkPadTone {
    pub(crate) primary: SineOscillator,
    pub(crate) detuned: SineOscillator,
    pub(crate) octave: SineOscillator,
    pub(crate) envelope: Adsr,
    pub(crate) pan: f32,
    pub(crate) gain: f32,
    pub(crate) lowpass_state: f32,
}

impl DarkPadTone {
    pub(crate) fn new(
        hz: f32,
        pan: f32,
        gain: f32,
        attack_time: f32,
        release_time: f32,
        sample_rate: f32,
    ) -> Self {
        Self {
            primary: SineOscillator::new(hz, sample_rate),
            detuned: SineOscillator::new(hz * 1.003, sample_rate),
            octave: SineOscillator::new(hz * 2.0, sample_rate),
            envelope: Adsr::new(attack_time, 12.0, 0.86, release_time, sample_rate),
            pan,
            gain,
            lowpass_state: 0.0,
        }
    }
    pub(crate) fn next_stereo(
        &mut self,
        width: f32,
        detune_mix: f32,
        octave_mix: f32,
    ) -> (f32, f32) {
        let s = self.primary.next()
            + self.detuned.next() * detune_mix
            + self.octave.next() * octave_mix;
        self.lowpass_state += PAD_DARK_LOWPASS_COEFF * (s - self.lowpass_state);
        let shaped = soft_clip(self.lowpass_state * 0.55)
            * self.envelope.next()
            * self.gain
            * PAD_DARK_OUTPUT_GAIN;
        StereoPanner::equal_power(shaped, self.pan * width)
    }
    pub(crate) fn release(&mut self) {
        self.envelope.note_off();
    }
    pub(crate) fn is_done(&self) -> bool {
        self.envelope.is_done()
    }
}

/// Fixed mix level of the Glass tone's shimmer layer (two octaves above the
/// fundamental), independent of the user-facing `pad.octave_mix` control.
const PAD_GLASS_SHIMMER_MIX: f32 = 0.09;
/// Output trim compensating for the shimmer layer's added energy so Glass
/// sits at a comparable perceived level to Warm/Dark.
const PAD_GLASS_OUTPUT_GAIN: f32 = 0.93;

/// Type 2: a brighter, glassier character. Identical oscillator stack to
/// Warm, plus a quiet fixed shimmer oscillator two octaves above the
/// fundamental for added upper harmonic content.
pub(crate) struct GlassPadTone {
    pub(crate) primary: SineOscillator,
    pub(crate) detuned: SineOscillator,
    pub(crate) octave: SineOscillator,
    pub(crate) shimmer: SineOscillator,
    pub(crate) envelope: Adsr,
    pub(crate) pan: f32,
    pub(crate) gain: f32,
}

impl GlassPadTone {
    pub(crate) fn new(
        hz: f32,
        pan: f32,
        gain: f32,
        attack_time: f32,
        release_time: f32,
        sample_rate: f32,
    ) -> Self {
        Self {
            primary: SineOscillator::new(hz, sample_rate),
            detuned: SineOscillator::new(hz * 1.003, sample_rate),
            octave: SineOscillator::new(hz * 2.0, sample_rate),
            shimmer: SineOscillator::new(hz * 4.0, sample_rate),
            envelope: Adsr::new(attack_time, 12.0, 0.86, release_time, sample_rate),
            pan,
            gain,
        }
    }
    pub(crate) fn next_stereo(
        &mut self,
        width: f32,
        detune_mix: f32,
        octave_mix: f32,
    ) -> (f32, f32) {
        let s = self.primary.next()
            + self.detuned.next() * detune_mix
            + self.octave.next() * octave_mix
            + self.shimmer.next() * PAD_GLASS_SHIMMER_MIX;
        let shaped = soft_clip(s * 0.55) * self.envelope.next() * self.gain * PAD_GLASS_OUTPUT_GAIN;
        StereoPanner::equal_power(shaped, self.pan * width)
    }
    pub(crate) fn release(&mut self) {
        self.envelope.note_off();
    }
    pub(crate) fn is_done(&self) -> bool {
        self.envelope.is_done()
    }
}

pub(crate) fn pad_tones(
    character: usize,
    notes: [i32; 4],
    tune: f32,
    sample_rate: f32,
    attack_time: f32,
    release_time: f32,
) -> Vec<PadTone> {
    let freqs = notes.map(|note| midi_to_hz(note) * tune_ratio(tune));
    let pans = [-0.52_f32, -0.18, 0.16, 0.46];
    let gains = [0.17_f32, 0.132, 0.126, 0.098];
    freqs
        .iter()
        .zip(pans)
        .zip(gains)
        .map(|((hz, pan), gain)| {
            PadTone::new(
                character,
                *hz,
                pan,
                gain,
                attack_time,
                release_time,
                sample_rate,
            )
        })
        .collect()
}

pub(crate) const PROGRESSIONS: [[[i32; 4]; 8]; 8] = [
    // Progression A: with an 8s release, each chord rings well into the next
    // (and beyond), so voicings are chosen to hold at least one common tone
    // across every step, including the loop back to step 0.
    [
        [45, 50, 55, 60], // Am
        [43, 50, 57, 60], // G   (holds D3/C4 from Am)
        [45, 52, 57, 60], // Am (alt voicing, holds A3/C4 from G)
        [47, 52, 55, 62], // B   (holds E3 from Am)
        [45, 52, 57, 64], // Am (alt voicing, holds E3 from B)
        [43, 50, 55, 62], // G   (parallel shift from Am, glides in stepwise)
        [48, 55, 60, 64], // C   (holds G3 from G)
        [55, 59, 64, 67], // Em (holds G3/C4 from C, and G3 back into Am)
    ],
    [
        [45, 50, 57, 60], // Am
        [50, 53, 57, 62], // Dm
        [48, 55, 60, 64], // C
        [43, 50, 55, 59], // G
        [41, 48, 53, 57], // F
        [52, 59, 64, 67], // Em
        [45, 52, 57, 60], // Am
        [43, 50, 55, 59], // G (non-tonic close, leads back to Am)
    ],
    [
        [45, 48, 52, 55], // Am7
        [41, 45, 48, 52], // Fmaj7
        [48, 52, 55, 59], // Cmaj7
        [43, 47, 50, 53], // G7
        [50, 53, 57, 60], // Dm7
        [52, 55, 59, 62], // Em7
        [47, 50, 53, 57], // Bm7b5 (half-diminished ii)
        [43, 50, 55, 59], // G (non-tonic close)
    ],
    [
        [45, 52, 57, 60], // Am, wide
        [41, 45, 48, 55], // Fmaj9-flavor
        [48, 55, 59, 62], // Cmaj9-flavor
        [43, 50, 53, 57], // G9-flavor
        [50, 57, 60, 64], // Dm9-flavor
        [52, 55, 59, 64], // Em, open
        [47, 53, 57, 64], // Bm7b5, wide (the "ache" chord)
        [43, 50, 55, 64], // G, wide (non-tonic close)
    ],
    // Progression E: dark, phrygian-leaning modal (A phrygian: A Bb C D E F G).
    // Suspended/added-tone voicings throughout; the bII-over-tonic close
    // (step 7) is deliberately dissonant, resolving into the Am at step 0.
    [
        [45, 48, 52, 57], // Am
        [46, 50, 53, 57], // Bbmaj7 (holds A3 from Am)
        [48, 53, 55, 60], // Csus4 (holds F3 from Bbmaj7)
        [50, 53, 60, 62], // Dm7, no 5th (holds C4 from Csus4)
        [52, 59, 62, 64], // Em7sus, no 3rd (holds D4 from Dm7)
        [55, 59, 62, 64], // G6 (holds B3/D4/E4 from Em7sus)
        [55, 58, 62, 65], // Gm7 (holds G3/D4 from G6)
        [45, 52, 58, 62], // Bbmaj7/A, ache (holds Bb3/D4 from Gm7, resolves to Am)
    ],
    // Progression F: suspended drone, phrygian-tinged E pedal (open fifths,
    // sus chords). Harmonic rhythm barely moves; the E pedal keeps ringing
    // through nearly every step for a moody, unresolved feel.
    [
        [52, 55, 59, 64], // Em (drone)
        [47, 52, 59, 64], // E5/B, open (holds E3/B3/E4 from Em)
        [50, 59, 62, 67], // G/D, sus (holds B3 from E5/B)
        [45, 57, 62, 65], // Dm/A (holds D4 from G/D)
        [45, 52, 57, 64], // Asus, open (holds A3/E4 from Dm/A)
        [52, 60, 64, 67], // Cmaj/E (holds E3/E4 from Asus)
        [55, 60, 62, 65], // Gsus4 (add C/D/F) (holds C4 from Cmaj/E)
        [52, 55, 59, 62], // Em7 (holds G3 from Gsus4, resolves to Em drone)
    ],
    // Progression G: bright C-major pop loop (I-V-vi-IV), common-tone rich.
    [
        [48, 52, 55, 60], // C
        [55, 60, 62, 67], // G (add C) (holds G3/C4 from C)
        [45, 57, 60, 64], // Am (holds C4 from G)
        [53, 57, 60, 65], // F (holds A3/C4 from Am)
        [48, 52, 60, 65], // C (add F) (holds C4/F4 from F)
        [55, 60, 62, 67], // G (add C) (holds C4 from C add F)
        [53, 57, 60, 65], // F (holds C4 from G add C)
        [48, 55, 60, 64], // C (open, add E) (holds C4 from F, loops to C)
    ],
    // Progression H: bright G-major "axis" loop (I-V-iii-vi, spelled here as
    // G-D-Em7-C, played twice with varied voicings), uplifting pop feel.
    [
        [55, 59, 62, 67], // G
        [50, 54, 57, 62], // D (holds D4 from G)
        [52, 55, 59, 62], // Em7 (holds D4 from D)
        [48, 52, 55, 60], // C (holds E3/G3 from Em7)
        [43, 55, 62, 67], // G, wide (holds G3 from C)
        [50, 57, 62, 66], // D (holds D4 from G)
        [52, 59, 62, 64], // Em7 (holds D4 from D)
        [48, 55, 60, 64], // C (holds E4 from Em7, loops to G)
    ],
];

/// The pad's current chord as raw MIDI note numbers (pre-`midi_to_hz`/tune),
/// for voices — like Arp — that need to build their own note list (e.g.
/// octave-extended cycles) rather than four fixed frequencies.
pub(crate) fn pad_chord_midi(progression: usize, step: usize) -> [i32; 4] {
    PROGRESSIONS[progression % PROGRESSIONS.len()][step % 8]
}

// ============================================================
// Custom chord-slot progression
//
// A ninth progression choice ("Custom") built from user-authored chord
// slots instead of a fixed table. `progression_index`/`pad_chord_tones`
// are the single chord-source path shared by Pad, Bass, and Arp: every
// voice resolves "what chord is playing at this step" through here so a
// custom progression drives all three identically. Built-in progressions
// (0..PROGRESSIONS.len()) are untouched — this only adds one more index.
// ============================================================

/// Selecting this progression index switches Pad/Bass/Arp onto the user's
/// chord slots (`PadControls::chord_slots`) instead of the `PROGRESSIONS`
/// table.
pub(crate) const CUSTOM_PROGRESSION_INDEX: usize = PROGRESSIONS.len();

/// Resolve `pad.progression`'s raw control value to a progression index,
/// wrapping across the built-ins plus the one Custom slot.
pub(crate) fn progression_index(value: f32) -> usize {
    (value.round() as i64).rem_euclid((PROGRESSIONS.len() + 1) as i64) as usize
}

pub(crate) fn is_custom_progression(progression: usize) -> bool {
    progression == CUSTOM_PROGRESSION_INDEX
}

/// Number of chord slots actually cycled through: `pad.chord_count` when
/// Custom is selected (so a shorter user progression loops correctly),
/// otherwise the built-ins' fixed 8-step length.
pub(crate) fn pad_chord_count(c: &PadControls) -> usize {
    if is_custom_progression(progression_index(c.progression)) {
        c.chord_count.round().clamp(1.0, CHORD_SLOT_COUNT as f32) as usize
    } else {
        8
    }
}

/// The shared chord-source entry point: the 4 chord tones (raw MIDI) at
/// `step` for whichever progression `c.progression` currently selects —
/// a built-in table lookup, or the custom chord slots. Pad, Bass, and Arp
/// all resolve their current chord through this one function.
pub(crate) fn pad_chord_tones(c: &PadControls, step: usize) -> [i32; 4] {
    let progression = progression_index(c.progression);
    if is_custom_progression(progression) {
        let count = pad_chord_count(c);
        pad_chord_notes_with_slot(&c.chord_slots[step % count])
    } else {
        pad_chord_midi(progression, step)
    }
}

/// A custom chord slot's four voiced tones: root (tonic-relative diatonic
/// degree + accidental), then diatonic third/fifth, then a top voice chosen
/// by `extension`, finally reshuffled by `inversion` and de-duplicated
/// upward so inversions/accidentals never collide two voices onto one note.
pub(crate) fn pad_chord_notes_with_slot(slot: &ChordSlotControls) -> [i32; 4] {
    const TONIC: i32 = 45; // A2, matching PROGRESSIONS' shared tonal center
    let root = shift_diatonic(TONIC, slot.degree.round().clamp(-7.0, 7.0) as i32);
    let accidental = slot.accidental.round().clamp(-1.0, 1.0) as i32;
    let extension = slot.extension.round().clamp(0.0, 3.0) as i32;
    let inversion = slot.inversion.round().clamp(0.0, 3.0) as i32;
    let top = match extension {
        1 => shift_diatonic(root, 6),
        2 => shift_diatonic(root, 8),
        3 => shift_diatonic(root, 10),
        _ => root + 12,
    };
    let mut notes = [root, shift_diatonic(root, 2), shift_diatonic(root, 4), top];
    apply_inversion(&mut notes, inversion);
    if accidental != 0 {
        notes = notes.map(|note| note + accidental);
    }
    dedupe_upwards(&mut notes);
    notes
}

/// A custom chord slot's root note alone (root + accidental, before
/// extension/inversion reshuffle) — what Bass follows instead of the pad's
/// full voicing.
pub(crate) fn pad_chord_root_note(slot: &ChordSlotControls) -> i32 {
    const TONIC: i32 = 45;
    let root = shift_diatonic(TONIC, slot.degree.round().clamp(-7.0, 7.0) as i32);
    root + slot.accidental.round().clamp(-1.0, 1.0) as i32
}

/// Move `note` by `steps` positions on the diatonic major scale (not raw
/// semitones), preserving octave-crossing correctly in either direction.
fn shift_diatonic(note: i32, steps: i32) -> i32 {
    const SCALE: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];
    let octave = note.div_euclid(12);
    let pitch = note.rem_euclid(12);
    let degree = SCALE
        .iter()
        .position(|&pc| pc == pitch)
        .map(|index| octave * 7 + index as i32)
        .unwrap_or_else(|| octave * 7);
    let shifted = degree + steps;
    let shifted_octave = shifted.div_euclid(7);
    let shifted_degree = shifted.rem_euclid(7) as usize;
    shifted_octave * 12 + SCALE[shifted_degree]
}

/// Move the lowest voice(s) up an octave `inversion` times, re-sorting after
/// each move so successive inversions keep stacking correctly.
fn apply_inversion(notes: &mut [i32; 4], inversion: i32) {
    notes.sort_unstable();
    for _ in 0..inversion {
        notes[0] += 12;
        notes.sort_unstable();
    }
}

/// Nudge any voice that lands on or below the one before it up by diatonic
/// steps until the chord is strictly ascending — accidentals or inversions
/// can otherwise stack two voices onto the same (or a crossed) pitch.
fn dedupe_upwards(notes: &mut [i32; 4]) {
    notes.sort_unstable();
    for i in 1..notes.len() {
        while notes[i] <= notes[i - 1] {
            notes[i] = shift_diatonic(notes[i], 1);
        }
    }
}
