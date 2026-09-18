//! The Pad voice: sustained chord drones, the chord source Bass and Arp
//! both follow.

use crate::fx::crossfade::{Outgoing, mix};

use super::*;

pub(crate) const MAX_PAD_LAYERS: usize = 4;
/// How long a `pad.type` change takes to crossfade from the outgoing
/// character stage to the incoming one, inside each already-sounding tone.
///
/// This is short because it does not have to hide an onset: the oscillators
/// and the amplitude envelope keep running untouched across a type change, so
/// the only discontinuity to smooth is the step between two stages' outputs
/// (a filter starting from zero state, a different output trim). Both sides
/// are the same oscillators through different post-stages, so they are
/// strongly correlated and a linear crossfade holds the level steady.
const PAD_TYPE_CROSSFADE_SECONDS: f32 = 0.03;

pub(crate) struct PadEngine {
    pub(crate) sample_rate: f32,
    pub(crate) layers: Vec<PadLayer>,
    pub(crate) cursor: ProgressionCursor,
    pub(crate) active_character: usize,
    pub(crate) last_chord_notes: [i32; 4],
    /// The transport seen on the previous sample, so a stop releases the
    /// sounding chord once and a restart voices it once.
    transport: Transport,
    pub(crate) width_lfo: DriftingLfo,
    pub(crate) air: WhiteNoise,
    pub(crate) rng: StdRng,
    pub(crate) telemetry: Arc<FluidTelemetry>,
}

impl PadEngine {
    /// `tune` is `master.tune` at construction. It must be passed in rather
    /// than assumed neutral: the opening chord is voiced here and holds for a
    /// whole `chord_bars` (~12 s at defaults, its release bleeding into the
    /// next chord), so a session started from a song code with a non-zero
    /// tune would play its first chord at concert pitch while Bass, Tonal and
    /// Arp — which read tune per note — are all transposed.
    pub(crate) fn new(
        sample_rate: f32,
        c: &PadControls,
        tune: f32,
        telemetry: Arc<FluidTelemetry>,
    ) -> Self {
        let cursor = ProgressionCursor::new(c);
        let active_character = wrapped_index(c.voice_type, PAD_TYPES.len());
        let initial_notes = pad_chord_tones(c, cursor.window.progression, cursor.slot());
        telemetry
            .chord_slot
            .store(cursor.slot() as u64, Ordering::Relaxed);
        Self {
            sample_rate,
            layers: vec![PadLayer::new(
                active_character,
                initial_notes,
                tune,
                sample_rate,
                c.attack_time,
                c.release_time,
            )],
            cursor,
            active_character,
            last_chord_notes: initial_notes,
            transport: Transport::Playing,
            width_lfo: DriftingLfo::new(1.0 / 54.0, sample_rate),
            air: WhiteNoise::new(),
            rng: StdRng::from_entropy(),
            telemetry,
        }
    }

    pub(crate) fn next(&mut self, c: &PadControls, tune: f32, timing: TimingContext) -> (f32, f32) {
        let advance = self.cursor.tick(c, timing);
        let chord_notes = pad_chord_tones(c, self.cursor.window.progression, self.cursor.slot());
        let chord_edited = chord_notes != self.last_chord_notes;
        let character = wrapped_index(c.voice_type, PAD_TYPES.len());
        let character_changed = character != self.active_character;
        self.last_chord_notes = chord_notes;
        self.active_character = character;

        // A type change is a change of character, not a new note. Every
        // character runs the same oscillator stack and differs only in the
        // stage after it, so the sounding tones swap that stage in place —
        // no new layer, no restarted envelope, no oscillator phase reset.
        // Voicing a fresh layer instead meant a full chord re-attacking from
        // silence with all its oscillators phase-aligned, which is an onset
        // transient, and it cut off every sustaining tail to do it.
        if character_changed {
            for layer in &mut self.layers {
                layer.set_character(character, self.sample_rate);
            }
        }

        // A chord sustains until the next one replaces it, so a stopped
        // clock would otherwise hold it forever: stopping releases it into
        // its tail, and restarting voices the current chord straight away
        // rather than leaving the pads silent until the next chord boundary.
        let transport_changed = timing.transport != self.transport;
        self.transport = timing.transport;
        let playing = timing.transport == Transport::Playing;
        if transport_changed && !playing {
            for layer in &mut self.layers {
                layer.release();
            }
        }

        if playing && (advance || chord_edited || transport_changed) {
            for layer in &mut self.layers {
                layer.release();
            }
            self.telemetry
                .publish_chord(self.cursor.slot() as u64, c.attack_time, c.release_time);
            if self.layers.len() >= MAX_PAD_LAYERS {
                let remove_count = self.layers.len() + 1 - MAX_PAD_LAYERS;
                self.layers.drain(0..remove_count);
            }
            self.layers.push(PadLayer::new(
                character,
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

        let (dry_l, dry_r) = mix_and_retain(
            &mut self.layers,
            |layer| layer.next_stereo(width, detune_mix, octave_mix),
            PadLayer::is_done,
        );

        let air = self.air.next_filtered(&mut self.rng, 0.0002) * 0.00025;

        // Headroom trim on the summed layer output, not a character control —
        // `pad.level` at 100% should reach close to full scale on its own,
        // leaving final safety margin to the master bus's soft-clip/compressor.
        const OUTPUT_TRIM: f32 = 0.95;
        (
            (dry_l * OUTPUT_TRIM + air) * c.level,
            (dry_r * OUTPUT_TRIM + air) * c.level,
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
    /// Swaps every tone onto a new `pad.type` character in place, leaving
    /// their oscillators and envelopes running.
    pub(crate) fn set_character(&mut self, character: usize, sample_rate: f32) {
        for t in &mut self.tones {
            t.set_character(character, sample_rate);
        }
    }
    pub(crate) fn is_done(&self) -> bool {
        self.tones.iter().all(PadTone::is_done)
    }
}

/// Shared oscillator/envelope/pan/gain stack behind every `pad.type`
/// character: fundamental, slightly detuned, one octave up, an ADSR, and the
/// pan/gain the layer was authored with. `PadTone` wraps this with the one
/// extra stage its character adds to `stack_sum`'s output.
pub(crate) struct PadOscStack {
    pub(crate) primary: SineOscillator,
    pub(crate) detuned: SineOscillator,
    pub(crate) octave: SineOscillator,
    pub(crate) envelope: Adsr,
    pub(crate) pan: f32,
    pub(crate) gain: f32,
}

impl PadOscStack {
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
    #[inline]
    pub(crate) fn stack_sum(&mut self, detune_mix: f32, octave_mix: f32) -> f32 {
        self.primary.next() + self.detuned.next() * detune_mix + self.octave.next() * octave_mix
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
/// Fixed mix level of the Glass tone's shimmer layer (two octaves above the
/// fundamental), independent of the user-facing `pad.octave_mix` control.
const PAD_GLASS_SHIMMER_MIX: f32 = 0.09;
/// Output trim compensating for the shimmer layer's added energy so Glass
/// sits at a comparable perceived level to Warm/Dark.
const PAD_GLASS_OUTPUT_GAIN: f32 = 0.93;
/// Fixed upper-partial range for Choir's gently moving breath layer.
const PAD_CHOIR_PARTIAL_MIX_MIN: f32 = 0.04;
const PAD_CHOIR_PARTIAL_MIX_RANGE: f32 = 0.07;
const PAD_CHOIR_OUTPUT_GAIN: f32 = 0.94;
/// Hollow pulls the shared stack back and replaces some energy with a
/// sub-octave sine, leaving a quieter center beneath the chord.
const PAD_HOLLOW_STACK_MIX: f32 = 0.82;
const PAD_HOLLOW_SUB_MIX: f32 = 0.18;
const PAD_HOLLOW_OUTPUT_GAIN: f32 = 1.1;
/// Tape rounds the stack and adds a slow, shallow level drift.
const PAD_TAPE_LOWPASS_COEFF: f32 = 0.32;
const PAD_TAPE_OUTPUT_GAIN: f32 = 1.16;

/// The one thing a `pad.type` character adds between the shared oscillator
/// stack and the soft-clipper. Warm — the legacy tone — adds nothing, so its
/// signal path through `PadTone::next_stereo` is the original one unchanged.
pub(crate) enum PadStage {
    /// Warm: the summed stack goes straight to the soft-clipper.
    None,
    /// Dark: a gentle fixed one-pole lowpass rounding off the highs the
    /// detune and octave layers contribute.
    Lowpass { state: f32 },
    /// Glass: a quiet fixed oscillator two octaves above the fundamental,
    /// added for upper harmonic content.
    Shimmer { oscillator: SineOscillator },
    /// Choir: a quiet third harmonic swells independently behind each tone.
    Choir {
        partial: SineOscillator,
        movement: SineOscillator,
    },
    /// Hollow: a sub-octave sine replaces part of the shared stack.
    Hollow { sub: SineOscillator },
    /// Tape: a fixed lowpass and slow level drift soften the shared stack.
    Tape {
        state: f32,
        movement: SineOscillator,
    },
}

impl PadStage {
    #[inline]
    fn apply(&mut self, s: f32) -> f32 {
        match self {
            Self::None => s,
            Self::Lowpass { state } => {
                *state += PAD_DARK_LOWPASS_COEFF * (s - *state);
                *state
            }
            Self::Shimmer { oscillator } => s + oscillator.next() * PAD_GLASS_SHIMMER_MIX,
            Self::Choir { partial, movement } => {
                let partial_mix = PAD_CHOIR_PARTIAL_MIX_MIN
                    + normalized_lfo(movement.next()) * PAD_CHOIR_PARTIAL_MIX_RANGE;
                s + partial.next() * partial_mix
            }
            Self::Hollow { sub } => s * PAD_HOLLOW_STACK_MIX + sub.next() * PAD_HOLLOW_SUB_MIX,
            Self::Tape { state, movement } => {
                *state += PAD_TAPE_LOWPASS_COEFF * (s - *state);
                *state * (0.94 + normalized_lfo(movement.next()) * 0.06)
            }
        }
    }
}

/// `pad.type` selects the tone character used for every layer's tones: index 0
/// (`Warm`, the default) is three sines summed, soft-clipped, and shaped by the
/// shared ADSR. Every other index selects one character stage before the
/// soft-clipper. The characters differ only in that stage and in the output
/// trim that keeps them at a comparable perceived level, so switching type
/// never touches chord selection, trigger timing, attack/release, or pan.
/// The character stage a tone is fading out of after a `pad.type` change,
/// held only for the length of the crossfade.
struct OutgoingStage {
    stage: PadStage,
    output_gain: f32,
}

pub(crate) struct PadTone {
    pub(crate) stack: PadOscStack,
    pub(crate) stage: PadStage,
    pub(crate) output_gain: f32,
    /// Kept so a later character swap can rebuild stages whose oscillators
    /// are pitched relative to this tone's own note.
    hz: f32,
    outgoing: Option<Outgoing<OutgoingStage>>,
}

/// Builds the one stage a `pad.type` character adds after the shared stack,
/// plus the output trim that keeps it level with the others.
fn pad_stage(character: usize, hz: f32, sample_rate: f32) -> (PadStage, f32) {
    match character {
        0 => (PadStage::None, 1.0),
        1 => (PadStage::Lowpass { state: 0.0 }, PAD_DARK_OUTPUT_GAIN),
        2 => (
            PadStage::Shimmer {
                oscillator: SineOscillator::new(hz * 4.0, sample_rate),
            },
            PAD_GLASS_OUTPUT_GAIN,
        ),
        3 => (
            PadStage::Choir {
                partial: SineOscillator::new(hz * 3.0, sample_rate),
                movement: SineOscillator::new(0.17, sample_rate),
            },
            PAD_CHOIR_OUTPUT_GAIN,
        ),
        4 => (
            PadStage::Hollow {
                sub: SineOscillator::new(hz * 0.5, sample_rate),
            },
            PAD_HOLLOW_OUTPUT_GAIN,
        ),
        5 => (
            PadStage::Tape {
                state: 0.0,
                movement: SineOscillator::new(0.11, sample_rate),
            },
            PAD_TAPE_OUTPUT_GAIN,
        ),
        _ => (PadStage::None, 1.0),
    }
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
        let (stage, output_gain) = pad_stage(character, hz, sample_rate);
        Self {
            stack: PadOscStack::new(hz, pan, gain, attack_time, release_time, sample_rate),
            stage,
            output_gain,
            hz,
            outgoing: None,
        }
    }

    /// Swaps in a new character stage, crossfading from the old one. The
    /// oscillator stack and the amplitude envelope are untouched, so the note
    /// keeps sounding exactly where it was in its own life.
    pub(crate) fn set_character(&mut self, character: usize, sample_rate: f32) {
        let (stage, output_gain) = pad_stage(character, self.hz, sample_rate);
        self.outgoing = Some(Outgoing::start(
            OutgoingStage {
                stage: std::mem::replace(&mut self.stage, stage),
                output_gain: std::mem::replace(&mut self.output_gain, output_gain),
            },
            PAD_TYPE_CROSSFADE_SECONDS * sample_rate,
        ));
    }

    pub(crate) fn next_stereo(
        &mut self,
        width: f32,
        detune_mix: f32,
        octave_mix: f32,
    ) -> (f32, f32) {
        let raw = self.stack.stack_sum(detune_mix, octave_mix);
        // Read once and shared by both stages: the envelope is the note's
        // own life and must not advance twice, or run differently, just
        // because a character change happens to be in flight.
        let envelope = self.stack.envelope.next();
        let mut shaped =
            soft_clip(self.stage.apply(raw) * 0.55) * envelope * self.stack.gain * self.output_gain;

        if let Some(outgoing) = &mut self.outgoing {
            let previous = soft_clip(outgoing.inner.stage.apply(raw) * 0.55)
                * envelope
                * self.stack.gain
                * outgoing.inner.output_gain;
            shaped = mix(shaped, previous, outgoing.advance());
            if outgoing.is_done() {
                self.outgoing = None;
            }
        }

        StereoPanner::equal_power(shaped, self.stack.pan * width)
    }

    pub(crate) fn release(&mut self) {
        self.stack.release();
    }

    pub(crate) fn is_done(&self) -> bool {
        self.stack.is_done()
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
    let freqs = notes.map(|note| note_hz(note, tune));
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

/// One chord of a built-in progression: the symbol a listener reads and the
/// four notes the Pad voices. The symbol is the voicing's only name, so the
/// page can never show a chord other than the one sounding;
/// `built_in_chord_names_match_their_voicings` holds every symbol to its
/// notes.
pub(crate) struct Chord {
    pub(crate) name: &'static str,
    pub(crate) notes: [i32; 4],
}

const fn chord(name: &'static str, notes: [i32; 4]) -> Chord {
    Chord { name, notes }
}

/// A built-in progression: eight chords, the Bass line under them, and the
/// mood word its label pairs with its key.
pub(crate) struct Progression {
    /// What a song code stores for this progression. Permanent: dial order
    /// is free to change, a saved value is not. Never reuse or renumber one,
    /// and never take `CUSTOM_SONG_VALUE`.
    pub(crate) song_value: i8,
    pub(crate) mood: &'static str,
    pub(crate) chords: [Chord; CHORD_SLOT_COUNT],
    /// One Bass note per chord, authored independently of the voicing so
    /// the line can walk where the chord's lowest tone would not.
    pub(crate) bass: [i32; CHORD_SLOT_COUNT],
}

/// Built-in progressions in dial order. With an 8 s release each chord rings
/// well into the next, so voicings hold at least one common tone from step
/// to step. Every progression from song value 9 on also holds one across the
/// loop back and within each half's own loop (chords 4 → 1 and 8 → 5), so a
/// Chord Count of 4 at Offset 0 or 4 is a complete progression in its own
/// right (`voice_led_progressions_hold_a_tone_through_every_window`).
pub(crate) const PROGRESSIONS: [Progression; 14] = [
    Progression {
        song_value: 0,
        mood: "Drift",
        chords: [
            chord("Am11", [45, 50, 55, 60]),
            chord("Gsus", [43, 50, 57, 60]),
            chord("Am", [45, 52, 57, 60]),
            chord("Em7/B", [47, 52, 55, 62]),
            chord("A5", [45, 52, 57, 64]),
            chord("G5", [43, 50, 55, 62]),
            chord("C", [48, 55, 60, 64]),
            chord("Em/G", [55, 59, 64, 67]),
        ],
        // Walks to G2 under the Em7/B instead of following its lowest tone,
        // giving the bass its own melodic movement.
        bass: [45, 47, 45, 43, 52, 53, 45, 45],
    },
    Progression {
        song_value: 1,
        mood: "Tide",
        chords: [
            chord("Am11", [45, 50, 57, 60]),
            chord("Dm", [50, 53, 57, 62]),
            chord("C", [48, 55, 60, 64]),
            chord("G", [43, 50, 55, 59]),
            chord("F", [41, 48, 53, 57]),
            chord("Em", [52, 59, 64, 67]),
            chord("Am", [45, 52, 57, 60]),
            chord("G", [43, 50, 55, 59]),
        ],
        bass: [45, 50, 48, 43, 41, 52, 45, 43],
    },
    Progression {
        song_value: 2,
        mood: "Velvet",
        chords: [
            chord("Am7", [45, 48, 52, 55]),
            chord("Fmaj7", [41, 45, 48, 52]),
            chord("Cmaj7", [48, 52, 55, 59]),
            chord("G7", [43, 47, 50, 53]),
            chord("Dm7", [50, 53, 57, 60]),
            chord("Em7", [52, 55, 59, 62]),
            chord("Bm7b5", [47, 50, 53, 57]),
            chord("G", [43, 50, 55, 59]),
        ],
        bass: [45, 41, 48, 43, 50, 52, 47, 43],
    },
    Progression {
        song_value: 3,
        mood: "Ache",
        chords: [
            chord("Am", [45, 52, 57, 60]),
            chord("Fadd9", [41, 45, 48, 55]),
            chord("G/C", [48, 55, 59, 62]),
            chord("Dm/G", [43, 50, 53, 57]),
            chord("Am/D", [50, 57, 60, 64]),
            chord("Em", [52, 55, 59, 64]),
            chord("Fmaj7/B", [47, 53, 57, 64]),
            chord("Em7/G", [43, 50, 55, 64]),
        ],
        bass: [45, 41, 48, 43, 50, 52, 47, 43],
    },
    // A phrygian (A Bb C D E F G), suspended and added-tone voicings; the
    // Bb over A that closes the loop is deliberately dissonant.
    Progression {
        song_value: 4,
        mood: "Shadow",
        chords: [
            chord("Am", [45, 48, 52, 57]),
            chord("Bbmaj7", [46, 50, 53, 57]),
            chord("Csus4", [48, 53, 55, 60]),
            chord("Dm7", [50, 53, 60, 62]),
            chord("E7sus", [52, 59, 62, 64]),
            chord("G6", [55, 59, 62, 64]),
            chord("Gm7", [55, 58, 62, 65]),
            chord("Bbmaj7#11/A", [45, 52, 58, 62]),
        ],
        bass: [45, 46, 48, 50, 52, 43, 43, 45],
    },
    // An E pedal rings through nearly every step; the harmony barely moves.
    Progression {
        song_value: 5,
        mood: "Drone",
        chords: [
            chord("Em", [52, 55, 59, 64]),
            chord("E5/B", [47, 52, 59, 64]),
            chord("G/D", [50, 59, 62, 67]),
            chord("Dm/A", [45, 57, 62, 65]),
            chord("A5", [45, 52, 57, 64]),
            chord("C/E", [52, 60, 64, 67]),
            chord("G7sus", [55, 60, 62, 65]),
            chord("Em7", [52, 55, 59, 62]),
        ],
        bass: [52, 47, 50, 45, 45, 52, 43, 52],
    },
    // I-V-vi-IV, common-tone rich.
    Progression {
        song_value: 6,
        mood: "Sunny",
        chords: [
            chord("C", [48, 52, 55, 60]),
            chord("Gsus4", [55, 60, 62, 67]),
            chord("Am", [45, 57, 60, 64]),
            chord("F", [53, 57, 60, 65]),
            chord("Cadd4", [48, 52, 60, 65]),
            chord("Gsus4", [55, 60, 62, 67]),
            chord("F", [53, 57, 60, 65]),
            chord("C", [48, 55, 60, 64]),
        ],
        bass: [48, 55, 45, 53, 48, 55, 53, 48],
    },
    // The I-V-vi-IV "axis" loop played twice with varied voicings.
    Progression {
        song_value: 7,
        mood: "Lift",
        chords: [
            chord("G", [55, 59, 62, 67]),
            chord("D", [50, 54, 57, 62]),
            chord("Em7", [52, 55, 59, 62]),
            chord("C", [48, 52, 55, 60]),
            chord("G5", [43, 55, 62, 67]),
            chord("D", [50, 57, 62, 66]),
            chord("E7sus", [52, 59, 62, 64]),
            chord("C", [48, 55, 60, 64]),
        ],
        bass: [43, 50, 52, 48, 43, 50, 52, 48],
    },
    // D lydian: the E over the D pedal is the raised fourth, bright and
    // unresolved. A3 holds through all eight chords.
    Progression {
        song_value: 9,
        mood: "Dawn",
        chords: [
            chord("Dmaj7", [50, 57, 61, 66]),
            chord("E/D", [50, 56, 59, 64]),
            chord("Bm7", [47, 57, 59, 62]),
            chord("A/C#", [49, 57, 61, 64]),
            chord("Gmaj9", [43, 57, 59, 66]),
            chord("A6", [45, 57, 61, 66]),
            chord("F#m7", [42, 57, 61, 64]),
            chord("Bm7", [47, 57, 62, 66]),
        ],
        bass: [50, 50, 47, 49, 43, 45, 42, 47],
    },
    // D dorian: the major IV (G6) is the one bright colour in a minor key.
    Progression {
        song_value: 10,
        mood: "Rain",
        chords: [
            chord("Dm9", [50, 53, 60, 64]),
            chord("G6", [43, 55, 59, 64]),
            chord("Am7", [45, 55, 60, 64]),
            chord("Cmaj7", [48, 55, 59, 64]),
            chord("Fmaj7", [41, 57, 60, 64]),
            chord("Em", [52, 55, 59, 64]),
            chord("G", [43, 55, 59, 62]),
            chord("Asus", [45, 57, 62, 64]),
        ],
        bass: [50, 43, 45, 48, 41, 52, 43, 45],
    },
    // F# aeolian, the cinematic i-VI-III-VII, each half closing on a
    // suspended or minor fifth that leans back home.
    Progression {
        song_value: 11,
        mood: "Night",
        chords: [
            chord("F#m7", [54, 57, 61, 64]),
            chord("Dmaj7", [50, 57, 61, 66]),
            chord("A", [45, 57, 61, 64]),
            chord("Esus4", [52, 57, 59, 64]),
            chord("Bm7", [47, 57, 59, 62]),
            chord("Dadd9", [50, 54, 57, 64]),
            chord("A/C#", [49, 57, 61, 64]),
            chord("C#m7", [49, 56, 59, 64]),
        ],
        bass: [42, 50, 45, 52, 47, 50, 49, 49],
    },
    // Eb major warmed by the borrowed minor iv (Abm) in the second half.
    Progression {
        song_value: 12,
        mood: "Glow",
        chords: [
            chord("Ebmaj7", [51, 55, 58, 62]),
            chord("Abmaj7", [44, 55, 60, 63]),
            chord("Cm7", [48, 55, 58, 63]),
            chord("Bbsus4", [46, 53, 58, 63]),
            chord("Fm7", [41, 56, 60, 63]),
            chord("Abm", [44, 56, 59, 63]),
            chord("Eb/G", [43, 55, 58, 63]),
            chord("Bb7sus", [46, 56, 58, 63]),
        ],
        bass: [51, 44, 48, 46, 41, 44, 43, 46],
    },
    // C mixolydian: the flat seventh (Bb) keeps it floating instead of
    // resolving.
    Progression {
        song_value: 13,
        mood: "Float",
        chords: [
            chord("Cadd9", [48, 55, 62, 64]),
            chord("Bb", [46, 58, 62, 65]),
            chord("F/A", [45, 57, 60, 65]),
            chord("Gm7", [43, 58, 62, 65]),
            chord("Dm7", [50, 57, 60, 65]),
            chord("Bbmaj7", [46, 57, 62, 65]),
            chord("F", [41, 57, 60, 65]),
            chord("Csus4", [48, 55, 60, 65]),
        ],
        bass: [48, 46, 45, 43, 50, 46, 41, 48],
    },
    // C minor: the first half moves over a held C pedal, the second walks
    // down from the minor v and hangs on a suspended fifth.
    Progression {
        song_value: 14,
        mood: "Deep",
        chords: [
            chord("Cm", [48, 55, 60, 63]),
            chord("Fm/C", [48, 56, 60, 65]),
            chord("Ab/C", [48, 56, 60, 63]),
            chord("Bb/C", [48, 58, 62, 65]),
            chord("Gm7", [43, 58, 62, 65]),
            chord("Ebmaj7", [51, 55, 58, 62]),
            chord("Abmaj7", [44, 55, 60, 63]),
            chord("Gsus4", [43, 55, 60, 62]),
        ],
        bass: [48, 48, 48, 48, 43, 51, 44, 43],
    },
];

/// What a song code stores when `pad.progression` is on Custom. Custom
/// was the ninth value before any progression was added after it, and keeps
/// that value for good.
const CUSTOM_SONG_VALUE: i8 = 8;

/// `pad.progression`'s song-code value at each dial position: every built-in
/// in dial order, then Custom. `ControlSpec::song_values` reads it, so a
/// progression added to the dial never changes what an existing code means.
pub(crate) const PROGRESSION_SONG_VALUES: [i8; CUSTOM_PROGRESSION_INDEX + 1] = {
    let mut values = [CUSTOM_SONG_VALUE; CUSTOM_PROGRESSION_INDEX + 1];
    let mut index = 0;
    while index < PROGRESSIONS.len() {
        values[index] = PROGRESSIONS[index].song_value;
        index += 1;
    }
    values
};

/// A built-in chord's raw MIDI notes (pre-`midi_to_hz`/tune), for voices —
/// like Arp — that build their own note list rather than four fixed
/// frequencies.
pub(crate) fn pad_chord_midi(progression: usize, slot: usize) -> [i32; 4] {
    PROGRESSIONS[progression % PROGRESSIONS.len()].chords[slot % CHORD_SLOT_COUNT].notes
}

/// The key a built-in progression is heard in: its first chord's root, with
/// `m` when that chord is minor ("Am11" is in Am, "Ebmaj7" in Eb).
pub(crate) fn progression_key(progression: &Progression) -> &'static str {
    let name = progression.chords[0].name;
    let root_len = match name.as_bytes().get(1) {
        Some(b'b' | b'#') => 2,
        _ => 1,
    };
    let rest = &name[root_len..];
    if rest.starts_with('m') && !rest.starts_with("maj") {
        &name[..root_len + 1]
    } else {
        &name[..root_len]
    }
}

/// `pad.progression`'s label: key and mood for a built-in ("Am · Drift"),
/// or "Custom".
pub(crate) fn progression_label(progression: usize) -> String {
    match PROGRESSIONS.get(progression) {
        Some(built_in) => format!("{} · {}", progression_key(built_in), built_in.mood),
        None => "Custom".to_string(),
    }
}

// ============================================================
// Chord window and the shared progression cursor
//
// "Custom" is one more progression choice, built from user-authored chord
// slots instead of a fixed table. `progression_index`/`pad_chord_tones` are
// the single chord-source path shared by Pad, Bass, Arp, and Lead: every
// voice resolves "what chord is playing at this slot" through here so a
// custom progression drives all of them identically.
// ============================================================

/// Selecting this progression index switches Pad/Bass/Arp onto the user's
/// chord slots (`PadControls::chord_slots`) instead of the `PROGRESSIONS`
/// table. It is the dial position one past the last built-in, which is not
/// what a song code stores for it (`PROGRESSION_SONG_VALUES`).
pub(crate) const CUSTOM_PROGRESSION_INDEX: usize = PROGRESSIONS.len();

/// Resolve `pad.progression`'s raw control value to a progression index,
/// wrapping across the built-ins plus the one Custom slot.
pub(crate) fn progression_index(value: f32) -> usize {
    wrapped_index(value, CUSTOM_PROGRESSION_INDEX + 1)
}

pub(crate) fn is_custom_progression(progression: usize) -> bool {
    progression == CUSTOM_PROGRESSION_INDEX
}

/// Which chords a progression loop plays: `count` chords starting at table
/// slot `offset`, wrapping past the eighth back to the first. Count 4 at
/// Offset 4 plays chords 5–8. The same for built-in and Custom
/// progressions: both hold `CHORD_SLOT_COUNT` chords.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ChordWindow {
    pub(crate) progression: usize,
    pub(crate) count: usize,
    pub(crate) offset: usize,
}

impl ChordWindow {
    /// The window the controls ask for right now.
    pub(crate) fn requested(c: &PadControls) -> Self {
        Self {
            progression: progression_index(c.progression),
            count: c.chord_count.round().clamp(1.0, CHORD_SLOT_COUNT as f32) as usize,
            offset: wrapped_index(c.chord_offset, CHORD_SLOT_COUNT),
        }
    }

    /// The table slot the window's `step`th chord comes from.
    pub(crate) fn slot(self, step: usize) -> usize {
        (self.offset + step) % CHORD_SLOT_COUNT
    }

    /// Every slot the window plays, in play order.
    pub(crate) fn slots(self) -> impl Iterator<Item = usize> {
        (0..self.count).map(move |step| self.slot(step))
    }
}

/// Where one engine is in its phrase: the chord window and chord length the
/// phrase was started with, how far through it, and when the next chord
/// begins. Pad, Bass, Arp, and Lead each hold one and advance it from the
/// same transport beat, so they always agree on the chord without reaching
/// into each other.
///
/// A phrase is fixed once it starts. A change to Chord Length, Chord Count,
/// Progression, or Chord Offset waits for the chord sounding now to end,
/// then restarts the phrase: the next chord is the first of the new window
/// (the one at slot `offset`), and the new length times it and every chord
/// after. The sounding chord is never cut short or stretched. A boundary
/// only restarts when the values the engine sees there (after automation
/// has snapped them) differ from the ones the phrase started with, so
/// automation that settles back on the phrase's own values never pins the
/// loop to its first chord, and an auto-morph that snaps all four together
/// lands as one restart.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ProgressionCursor {
    pub(crate) window: ChordWindow,
    /// `pad.chord_bars` in beats, as the phrase started with it.
    pub(crate) chord_beats: f32,
    pub(crate) step: usize,
    /// Transport beat the next chord starts on; set on the first tick, the
    /// first chord boundary after the engine starts.
    next_chord_beat: Option<f64>,
}

impl ProgressionCursor {
    pub(crate) fn new(c: &PadControls) -> Self {
        Self {
            window: ChordWindow::requested(c),
            chord_beats: c.chord_bars * 4.0,
            step: 0,
            next_chord_beat: None,
        }
    }

    /// The table slot sounding now.
    pub(crate) fn slot(&self) -> usize {
        self.window.slot(self.step)
    }

    /// Moves the cursor up to `timing`'s beat, reading `c` only at a chord
    /// boundary. Returns whether a new chord starts on this tick.
    pub(crate) fn tick(&mut self, c: &PadControls, timing: TimingContext) -> bool {
        let next = *self.next_chord_beat.get_or_insert_with(|| {
            GridSpec::new(self.chord_beats, 0.0, 0.0)
                .hit_after(timing.beat)
                .beat
        });
        // A stopped transport holds its beat, and like every grid it fires
        // nothing until the clock runs again.
        if timing.transport == Transport::Stopped || timing.beat + GRID_BEAT_EPSILON < next {
            return false;
        }
        let window = ChordWindow::requested(c);
        let chord_beats = c.chord_bars * 4.0;
        if window != self.window || chord_beats != self.chord_beats {
            self.window = window;
            self.chord_beats = chord_beats;
            self.step = 0;
        } else {
            self.step = (self.step + 1) % self.window.count;
        }
        // Counted from the boundary itself rather than the sample that
        // noticed it, so chords stay on the transport grid.
        self.next_chord_beat =
            Some(next + GridSpec::new(self.chord_beats, 0.0, 0.0).interval_beats);
        true
    }
}

/// How Bass, Arp, and Lead follow the Pad's chord loop without reaching into
/// `PadEngine`: the same cursor on the same transport beat, so the
/// follower's chord always matches the pad's. The phrase is adopted from the
/// first frame's controls, exactly as the pad adopts it at construction.
pub(crate) struct ProgressionFollower {
    pub(crate) cursor: Option<ProgressionCursor>,
}

impl ProgressionFollower {
    pub(crate) fn new() -> Self {
        Self { cursor: None }
    }

    /// Advances the phrase and returns the `(progression, slot)` to voice
    /// this frame.
    pub(crate) fn follow(&mut self, pad: &PadControls, timing: TimingContext) -> (usize, usize) {
        let cursor = self
            .cursor
            .get_or_insert_with(|| ProgressionCursor::new(pad));
        cursor.tick(pad, timing);
        (cursor.window.progression, cursor.slot())
    }
}

/// The shared chord-source entry point: the four notes at one table slot of
/// a progression, built-in or Custom.
pub(crate) fn pad_chord_tones(c: &PadControls, progression: usize, slot: usize) -> [i32; 4] {
    if is_custom_progression(progression) {
        pad_chord_notes_with_slot(&c.chord_slots[slot])
    } else {
        pad_chord_midi(progression, slot)
    }
}

/// The name of the chord at one table slot, built-in or Custom.
pub(crate) fn pad_chord_name(c: &PadControls, progression: usize, slot: usize) -> String {
    match PROGRESSIONS.get(progression) {
        Some(built_in) => built_in.chords[slot % CHORD_SLOT_COUNT].name.to_string(),
        None => custom_chord_name(&c.chord_slots[slot]),
    }
}

/// A custom chord slot's four voiced tones: root (tonic-relative diatonic
/// degree + accidental), then third/fifth (diatonic, with the third
/// overridable by `quality` for modal interchange), then a top voice chosen
/// by `extension`, finally reshuffled by `inversion` and de-duplicated
/// upward so inversions/accidentals never collide two voices onto one note.
pub(crate) fn pad_chord_notes_with_slot(slot: &ChordSlotControls) -> [i32; 4] {
    let root = slot_root(slot);
    let accidental = slot.accidental.round().clamp(-1.0, 1.0) as i32;
    let extension = slot.extension.round().clamp(0.0, 3.0) as i32;
    let inversion = slot.inversion.round().clamp(0.0, 3.0) as i32;
    let third = slot_third(slot, root);
    let top = match extension {
        1 => shift_diatonic(root, 6),
        2 => shift_diatonic(root, 8),
        3 => shift_diatonic(root, 10),
        _ => root + 12,
    };
    let mut notes = [root, third, shift_diatonic(root, 4), top];
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
    slot_root(slot) + slot.accidental.round().clamp(-1.0, 1.0) as i32
}

/// Whether a slot's resolved third is minor — honors a forced `quality`,
/// otherwise reports what the diatonic scale gives at this degree. Drives the
/// Quality row's "scale (min)"-style display so the inherit position still
/// tells the user what they're hearing.
pub(crate) fn pad_chord_slot_is_minor(slot: &ChordSlotControls) -> bool {
    let root = slot_root(slot);
    slot_third(slot, root) - root == 3
}

/// A2, matching `PROGRESSIONS`' shared tonal center: the note a custom
/// slot's `degree` counts from.
const CUSTOM_TONIC: i32 = 45;

/// A custom slot's root before its accidental: the tonic shifted by the
/// slot's diatonic degree.
fn slot_root(slot: &ChordSlotControls) -> i32 {
    shift_diatonic(CUSTOM_TONIC, slot.degree.round().clamp(-7.0, 7.0) as i32)
}

/// A custom slot's third: forced minor/major by `quality`, otherwise the
/// diatonic third above `root`.
fn slot_third(slot: &ChordSlotControls, root: i32) -> i32 {
    match slot.quality.round().clamp(-1.0, 1.0) as i32 {
        -1 => root + 3,
        1 => root + 4,
        _ => shift_diatonic(root, 2),
    }
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

/// A Custom chord slot's name, read off the same fields that voice it:
/// root (degree plus accidental), the third `slot_third` resolves, the
/// extension's top voice, and the inversion's bass as a slash.
pub(crate) fn custom_chord_name(slot: &ChordSlotControls) -> String {
    const NATURALS: [&str; 12] = ["C", "", "D", "", "E", "F", "", "G", "", "A", "", "B"];
    let natural = slot_root(slot);
    let accidental = slot.accidental.round().clamp(-1.0, 1.0) as i32;
    let minor = slot_third(slot, natural) - natural == 3;
    let flat_five = shift_diatonic(natural, 4) - natural == 6;
    let minor_seventh = shift_diatonic(natural, 6) - natural == 10;
    let body = match (slot.extension.round().clamp(0.0, 3.0) as i32, minor) {
        (1, true) => "m6",
        (1, false) => "6",
        (2, true) if minor_seventh && flat_five => "m7b5",
        (2, true) if minor_seventh => "m7",
        (2, true) => "mmaj7",
        (2, false) if minor_seventh => "7",
        (2, false) => "maj7",
        (3, true) => "madd9",
        (3, false) => "add9",
        (_, true) if flat_five => "dim",
        (_, true) => "m",
        (_, false) => "",
    };
    // `dim` and `m7b5` already say the fifth is flat; nothing else does.
    let flat_five_suffix = if flat_five && !matches!(body, "dim" | "m7b5") {
        "b5"
    } else {
        ""
    };
    let root_name = format!(
        "{}{}",
        NATURALS[natural.rem_euclid(12) as usize],
        match accidental {
            -1 => "b",
            1 => "#",
            _ => "",
        }
    );
    let bass = pad_chord_notes_with_slot(slot)[0];
    let slash = if (bass - natural - accidental).rem_euclid(12) == 0 {
        String::new()
    } else {
        format!("/{}", pitch_class_name(bass, accidental > 0))
    };
    format!("{root_name}{body}{flat_five_suffix}{slash}")
}

/// A note's pitch-class name, spelled with sharps or flats.
fn pitch_class_name(note: i32, sharps: bool) -> &'static str {
    const FLATS: [&str; 12] = [
        "C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B",
    ];
    const SHARPS: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let pitch = note.rem_euclid(12) as usize;
    if sharps { SHARPS[pitch] } else { FLATS[pitch] }
}
