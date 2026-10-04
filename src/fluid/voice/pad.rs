//! The Pad voice: sustained chords or sixteenth-note stabs from the chord
//! source Bass and Arp both follow.

use crate::fx::crossfade::{Outgoing, mix};
use crate::midi::{MidiMessage, MidiSink, pad_notes_with_count};

use super::*;

pub(crate) const MAX_PAD_LAYERS: usize = 4;
const MAX_MIDI_PAD_VOICES: usize = 16;
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
    input_voices: Vec<PadInputVoice>,
    pub(crate) cursor: ProgressionCursor,
    pub(crate) active_character: usize,
    pub(crate) last_chord_notes: [i32; 4],
    last_chord_definition: ChordSlotControls,
    active_note_count: usize,
    /// The transport seen on the previous sample, so a stop releases the
    /// sounding chord once and a restart voices it once.
    transport: Transport,
    pub(crate) width_lfo: DriftingLfo,
    pub(crate) air: WhiteNoise,
    pub(crate) rng: StdRng,
    pub(crate) telemetry: Arc<FluidTelemetry>,
    midi: Option<MidiSink>,
    midi_input_attached: bool,
    internal_audio: EasedRamp,
    midi_suppressed: bool,
    midi_initial_pending: bool,
    stab_trigger: GridTrigger,
    active_stab_until_beat: Option<f64>,
    active_stab_until_sample: Option<u64>,
    sample_clock: u64,
    last_midi_trigger_sample: Option<u64>,
    stabbing: bool,
    midi_triggering: bool,
}

struct PadInputVoice {
    note: u8,
    held: bool,
    tone: PadTone,
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
        let last_chord_definition = *c.chord_slot(cursor.window.progression, cursor.slot());
        let initial_notes = pad_chord_notes_with_slot(&last_chord_definition);
        let note_count = pad_note_count(c.chord_notes);
        telemetry
            .chord_slot
            .store(cursor.slot() as u64, Ordering::Relaxed);
        Self {
            sample_rate,
            layers: vec![PadLayer::new(
                active_character,
                initial_notes,
                note_count,
                tune,
                sample_rate,
                c.attack_time,
                c.release_time,
            )],
            input_voices: Vec::with_capacity(MAX_MIDI_PAD_VOICES),
            cursor,
            active_character,
            last_chord_notes: initial_notes,
            last_chord_definition,
            active_note_count: note_count,
            transport: Transport::Playing,
            width_lfo: DriftingLfo::new(1.0 / 54.0, sample_rate),
            air: WhiteNoise::new(),
            rng: StdRng::from_entropy(),
            telemetry,
            midi: None,
            midi_input_attached: false,
            internal_audio: EasedRamp::settled(1.0),
            midi_suppressed: false,
            midi_initial_pending: false,
            stab_trigger: GridTrigger::new(),
            active_stab_until_beat: None,
            active_stab_until_sample: None,
            sample_clock: 0,
            last_midi_trigger_sample: None,
            stabbing: false,
            midi_triggering: false,
        }
    }

    pub(crate) fn set_midi(&mut self, sink: MidiSink) {
        self.midi = Some(sink);
        self.midi_initial_pending = true;
    }

    pub(crate) fn set_midi_input_attached(&mut self) {
        self.midi_input_attached = true;
    }

    pub(crate) fn set_midi_suppressed(&mut self, suppressed: bool) {
        if self.midi_suppressed != suppressed {
            if suppressed {
                if let Some(sink) = &self.midi {
                    sink.send(MidiMessage::PadOff);
                }
                self.midi_initial_pending = false;
            } else {
                self.midi_initial_pending = true;
            }
            self.midi_suppressed = suppressed;
        }
    }

    fn send_midi(&self, message: MidiMessage) {
        if !self.midi_suppressed
            && let Some(sink) = &self.midi
        {
            sink.send(message);
        }
    }

    fn release_stab(&mut self) {
        let beat_active = self.active_stab_until_beat.take().is_some();
        let sample_active = self.active_stab_until_sample.take().is_some();
        if beat_active || sample_active {
            for layer in &mut self.layers {
                layer.release();
            }
            self.send_midi(MidiMessage::PadOff);
        }
    }

    pub(crate) fn release_input(&mut self) {
        for voice in &mut self.input_voices {
            if voice.held {
                voice.held = false;
                voice.tone.release();
            }
        }
        if self.midi_triggering {
            self.release_stab();
        }
    }

    pub(crate) fn midi_note_off(&mut self, note: u8) {
        for voice in &mut self.input_voices {
            if voice.note == note && voice.held {
                voice.held = false;
                voice.tone.release();
            }
        }
    }

    pub(crate) fn midi_note_on(
        &mut self,
        note: u8,
        c: &PadControls,
        tune: f32,
        timing: TimingContext,
    ) {
        if self.stabbing && self.midi_triggering {
            let coalesce_samples = (self.sample_rate * 0.01).round() as u64;
            if self.last_midi_trigger_sample.is_some_and(|previous| {
                self.sample_clock.saturating_sub(previous) < coalesce_samples
            }) {
                return;
            }
            self.last_midi_trigger_sample = Some(self.sample_clock);
            self.trigger_stab(c, tune, timing);
            return;
        }
        self.midi_note_off(note);
        if self.input_voices.len() >= MAX_MIDI_PAD_VOICES {
            self.input_voices.remove(0);
        }
        self.input_voices.push(PadInputVoice {
            note,
            held: true,
            tone: PadTone::new(
                self.active_character,
                note_hz(i32::from(note), tune),
                0.0,
                0.17,
                c.attack_time,
                c.release_time,
                self.sample_rate,
            ),
        });
    }

    fn trigger_stab(&mut self, c: &PadControls, tune: f32, timing: TimingContext) {
        self.release_stab();
        let chord_notes = self.last_chord_notes;
        self.telemetry
            .publish_hit(MusicalHit::Pad, c.level, pitch_class(chord_notes[0]));
        self.send_midi(MidiMessage::PadChord(pad_notes_with_count(
            chord_notes,
            self.active_note_count,
            tune,
        )));
        if self.layers.len() >= MAX_PAD_LAYERS {
            let remove_count = self.layers.len() + 1 - MAX_PAD_LAYERS;
            self.layers.drain(0..remove_count);
        }
        self.layers.push(PadLayer::new(
            self.active_character,
            chord_notes,
            self.active_note_count,
            tune,
            self.sample_rate,
            c.attack_time,
            c.release_time,
        ));
        if timing.transport == Transport::Stopped {
            let samples = (f64::from(c.gate_beats) * 60.0 / timing.bpm
                * f64::from(self.sample_rate))
            .round() as u64;
            self.active_stab_until_sample = Some(self.sample_clock + samples.max(1));
        } else {
            self.active_stab_until_beat = Some(timing.beat + f64::from(c.gate_beats));
        }
    }

    pub(crate) fn restart_sequence(&mut self, c: &PadControls) {
        self.release_stab();
        for layer in &mut self.layers {
            layer.release();
        }
        self.cursor = ProgressionCursor::new(c);
        self.stab_trigger = GridTrigger::new();
        self.transport = Transport::Stopped;
    }

    pub(crate) fn next(&mut self, c: &PadControls, tune: f32, timing: TimingContext) -> (f32, f32) {
        let advance = self.cursor.tick(c, timing);
        let definition = c.chord_slot(self.cursor.window.progression, self.cursor.slot());
        let chord_notes = if *definition == self.last_chord_definition {
            self.last_chord_notes
        } else {
            self.last_chord_definition = *definition;
            pad_chord_notes_with_slot(definition)
        };
        let note_count = pad_note_count(c.chord_notes);
        let chord_edited =
            chord_notes != self.last_chord_notes || note_count != self.active_note_count;
        let character = wrapped_index(c.voice_type, PAD_TYPES.len());
        let character_changed = character != self.active_character;
        self.last_chord_notes = chord_notes;
        self.active_note_count = note_count;
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
            for voice in &mut self.input_voices {
                voice.tone.set_character(character, self.sample_rate);
            }
        }

        // A chord sustains until the next one replaces it, so a stopped
        // clock would otherwise hold it forever: stopping releases it into
        // its tail, and restarting voices the current chord straight away
        // rather than leaving the pads silent until the next chord boundary.
        let transport_changed = timing.transport != self.transport;
        self.transport = timing.transport;
        let playing = timing.transport == Transport::Playing;
        let stabbing = c.trigger >= 0.5;
        let midi_triggering = stabbing && c.midi_trigger >= 0.5;
        let mode_changed = stabbing != self.stabbing;
        let source_changed = midi_triggering != self.midi_triggering;
        self.stabbing = stabbing;
        self.midi_triggering = midi_triggering;
        if mode_changed {
            if stabbing {
                self.layers.clear();
                self.stab_trigger = GridTrigger::new();
            } else {
                for layer in &mut self.layers {
                    layer.release();
                }
            }
            self.active_stab_until_beat = None;
            self.active_stab_until_sample = None;
            self.send_midi(MidiMessage::PadOff);
        }
        if source_changed {
            self.release_input();
            self.release_stab();
        }
        if mode_changed || source_changed {
            self.last_midi_trigger_sample = None;
        }
        if transport_changed && !playing {
            self.release_stab();
            for layer in &mut self.layers {
                layer.release();
            }
        }

        if stabbing {
            self.midi_initial_pending = false;
            if playing && (advance || chord_edited) {
                self.telemetry.publish_chord(
                    self.cursor.slot() as u64,
                    pitch_class(chord_notes[0]),
                    0.01,
                    0.08,
                );
            }
            if chord_edited
                && (self.active_stab_until_beat.is_some()
                    || self.active_stab_until_sample.is_some())
            {
                self.release_stab();
            }
            if self
                .active_stab_until_beat
                .is_some_and(|end| timing.beat >= end)
                || self
                    .active_stab_until_sample
                    .is_some_and(|end| self.sample_clock >= end)
            {
                self.release_stab();
            }
            if self
                .stab_trigger
                .pop_swung(timing, 0.25, c.offset_beats, c.swing)
                && !midi_triggering
            {
                // The lane travels with the grid: slot k of the shifted grid
                // plays step k, so Offset rotates the pattern, not just the clock.
                let slot = ((timing.beat - f64::from(c.offset_beats)) / 0.25 + 1e-6).floor() as i64;
                let step = slot.rem_euclid(16) as usize;
                if c.steps[step] >= 0.5 {
                    self.trigger_stab(c, tune, timing);
                }
            }
        }

        if !stabbing && playing && (advance || chord_edited || transport_changed || mode_changed) {
            for layer in &mut self.layers {
                layer.release();
            }
            self.telemetry.publish_chord(
                self.cursor.slot() as u64,
                pitch_class(chord_notes[0]),
                c.attack_time,
                c.release_time,
            );
            self.telemetry
                .publish_hit(MusicalHit::Pad, c.level, pitch_class(chord_notes[0]));
            self.send_midi(MidiMessage::PadChord(pad_notes_with_count(
                chord_notes,
                note_count,
                tune,
            )));
            self.midi_initial_pending = false;
            if self.layers.len() >= MAX_PAD_LAYERS {
                let remove_count = self.layers.len() + 1 - MAX_PAD_LAYERS;
                self.layers.drain(0..remove_count);
            }
            self.layers.push(PadLayer::new(
                character,
                chord_notes,
                note_count,
                tune,
                self.sample_rate,
                c.attack_time,
                c.release_time,
            ));
        }
        if !stabbing && playing && self.midi_initial_pending {
            self.send_midi(MidiMessage::PadChord(pad_notes_with_count(
                chord_notes,
                note_count,
                tune,
            )));
            self.midi_initial_pending = false;
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
        let (input_l, input_r) = mix_and_retain(
            &mut self.input_voices,
            |voice| voice.tone.next_stereo(width, detune_mix, octave_mix),
            |voice| voice.tone.is_done(),
        );

        let air = self.air.next_filtered(&mut self.rng, 0.0002) * 0.00025;

        // Headroom trim on the summed layer output, not a character control —
        // `pad.level` at 100% should reach close to full scale on its own,
        // leaving final safety margin to the master bus's soft-clip/compressor.
        const OUTPUT_TRIM: f32 = 0.95;
        // A connected keyboard owns Hold-mode Pad audio while MIDI In is on.
        // The progression still advances and can drive MIDI Out; Stabs remain
        // an explicit rhythm/chord-trigger surface.
        let internal_target = if self.midi_input_attached && c.midi_in >= 0.5 && !stabbing {
            0.0
        } else {
            1.0
        };
        if self.internal_audio.target != internal_target {
            let samples = (LEVEL_RAMP_MS * 0.001 * self.sample_rate).round() as u32;
            self.internal_audio.retarget(internal_target, samples);
        }
        let internal_gain = self.internal_audio.next();
        self.sample_clock += 1;
        (
            ((dry_l * internal_gain + input_l) * OUTPUT_TRIM + air) * c.level,
            ((dry_r * internal_gain + input_r) * OUTPUT_TRIM + air) * c.level,
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
        note_count: usize,
        tune: f32,
        sample_rate: f32,
        attack_time: f32,
        release_time: f32,
    ) -> Self {
        Self {
            tones: pad_tones(
                character,
                notes,
                note_count,
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
    note_count: usize,
    tune: f32,
    sample_rate: f32,
    attack_time: f32,
    release_time: f32,
) -> Vec<PadTone> {
    let (voicing, count) = pad_voicing(notes, note_count);
    let (pans, gains) = match count {
        2 => ([-0.3, 0.3, 0.0, 0.0, 0.0], [0.17, 0.126, 0.0, 0.0, 0.0]),
        3 => ([-0.45, 0.0, 0.45, 0.0, 0.0], [0.17, 0.132, 0.126, 0.0, 0.0]),
        4 => (
            [-0.52, -0.18, 0.16, 0.46, 0.0],
            [0.17, 0.132, 0.126, 0.098, 0.0],
        ),
        _ => (
            [-0.52, -0.25, 0.0, 0.25, 0.52],
            [0.17, 0.132, 0.126, 0.098, 0.07],
        ),
    };
    voicing[..count]
        .iter()
        .zip(pans)
        .zip(gains)
        .map(|((hz, pan), gain)| {
            PadTone::new(
                character,
                note_hz(*hz, tune),
                pan,
                gain,
                attack_time,
                release_time,
                sample_rate,
            )
        })
        .collect()
}

pub(crate) fn pad_note_count(value: f32) -> usize {
    (value.round() as i64).clamp(2, 5) as usize
}

/// Preserve the saved positional output choices: 2 uses source 1+3, 3 the
/// first three, 4 all four, and 5 adds source 1 two octaves up. This may omit
/// extensions or thirds; it never recovers a fifth omitted by the builder.
/// Bass, Arp and Lead keep following the underlying four source tones.
pub(crate) fn pad_voicing(notes: [i32; 4], count: usize) -> ([i32; 5], usize) {
    let count = count.clamp(2, 5);
    let mut voiced = [notes[0], notes[1], notes[2], notes[3], notes[0] + 24];
    if count == 2 {
        voiced[1] = notes[2];
    }
    (voiced, count)
}

/// An authored shared-builder preset. The symbol anchors the progression's
/// key label; the slot UI derives its actual name from the effective notes.
pub(crate) struct Chord {
    pub(crate) name: &'static str,
    pub(crate) preset: ChordSlotControls,
}

const fn chord(name: &'static str, values: [i8; 8]) -> Chord {
    Chord {
        name,
        preset: ChordSlotControls::preset(values),
    }
}

impl Chord {
    #[cfg(test)]
    pub(crate) fn notes(&self) -> [i32; 4] {
        pad_chord_notes_with_slot(&self.preset)
    }
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
pub(crate) const PROGRESSIONS: [Progression; 15] = [
    Progression {
        song_value: 0,
        mood: "Drift",
        chords: [
            chord("Am11", [-7, 0, 0, 1, 3, 3, 1, 0]),
            chord("Gsus", [-1, 0, 2, 3, 0, 0, 1, 0]),
            chord("Am", [0, 0, 0, 0, 0, 0, 1, 0]),
            chord("Em7/B", [-3, 0, 0, 1, 0, 2, 1, 0]),
            chord("A5", [0, 0, 4, 0, 0, 0, 0, 0]),
            chord("G5", [-1, 0, 4, 0, 0, 0, 0, 0]),
            chord("C", [2, 0, 0, 0, 0, 0, 1, 0]),
            chord("Em/G", [4, 0, 0, 0, 0, 1, 4, 0]),
        ],
        // Walks to G2 under the Em7/B instead of following its lowest tone,
        // giving the bass its own melodic movement.
        bass: [45, 47, 45, 43, 52, 53, 45, 45],
    },
    Progression {
        song_value: 1,
        mood: "Tide",
        chords: [
            chord("Am11", [-7, 0, 0, 3, 0, 2, 1, 2]),
            chord("Dm", [3, 0, 0, 0, 0, 0, 0, 0]),
            chord("C", [2, 0, 0, 0, 0, 0, 1, 0]),
            chord("G", [-1, 0, 0, 0, 0, 0, 1, 0]),
            chord("F", [-2, 0, 0, 0, 0, 0, 1, 0]),
            chord("Em", [4, 0, 0, 0, 0, 0, 1, 0]),
            chord("Am", [0, 0, 0, 0, 0, 0, 1, 0]),
            chord("G", [-1, 0, 0, 0, 0, 0, 1, 0]),
        ],
        bass: [45, 50, 48, 43, 41, 52, 45, 43],
    },
    Progression {
        song_value: 2,
        mood: "Velvet",
        chords: [
            chord("Am7", [0, 0, 0, 1, 0, 0, 0, 0]),
            chord("Fmaj7", [-2, 0, 0, 1, 0, 0, 0, 0]),
            chord("Cmaj7", [2, 0, 0, 1, 0, 0, 0, 0]),
            chord("G7", [-1, 0, 0, 1, 0, 0, 0, 0]),
            chord("Dm7", [3, 0, 0, 1, 0, 0, 0, 0]),
            chord("Em7", [4, 0, 0, 1, 0, 0, 0, 0]),
            chord("Bm7b5", [1, 0, 0, 1, 0, 0, 0, 0]),
            chord("G", [-1, 0, 0, 0, 0, 0, 1, 0]),
        ],
        bass: [45, 41, 48, 43, 50, 52, 47, 43],
    },
    Progression {
        song_value: 3,
        mood: "Ache",
        chords: [
            chord("Am", [0, 0, 0, 0, 0, 0, 1, 0]),
            chord("Fadd9", [-2, 0, 0, 2, 0, 0, 0, 0]),
            chord("G/C", [-1, 0, 0, 0, 0, 5, 0, 0]),
            chord("Dm/G", [-4, 0, 0, 0, 0, 5, 0, 0]),
            chord("Am/D", [0, 0, 0, 0, 0, 5, 0, 0]),
            chord("Em", [4, 0, 0, 0, 0, 0, 0, 0]),
            chord("Fmaj7/B", [-2, 0, 0, 1, 0, 6, 0, 0]),
            chord("Em7/G", [-3, 0, 0, 1, 0, 1, 3, 0]),
        ],
        bass: [45, 41, 48, 43, 50, 52, 47, 43],
    },
    // A phrygian (A Bb C D E F G), suspended and added-tone voicings; the
    // Bb over A that closes the loop is deliberately dissonant.
    Progression {
        song_value: 4,
        mood: "Shadow",
        chords: [
            chord("Am", [0, 0, 0, 0, 0, 0, 0, 0]),
            chord("Bbmaj7", [0, 1, 1, 5, 0, 0, 0, 0]),
            chord("Csus4", [2, 0, 3, 0, 0, 0, 0, 0]),
            chord("Dm7", [3, 0, 0, 1, 0, 0, 0, 2]),
            chord("E7sus", [4, 0, 4, 1, 0, 0, 0, 0]),
            chord("G6", [-1, 0, 0, 9, 0, 3, 0, 0]),
            chord("Gm7", [6, 0, -1, 1, 0, 0, 0, 0]),
            chord("Bbmaj7#11/A", [-7, 1, 1, 5, 8, 2, 3, 0]),
        ],
        bass: [45, 46, 48, 50, 52, 43, 43, 45],
    },
    // An E pedal rings through nearly every step; the harmony barely moves.
    Progression {
        song_value: 5,
        mood: "Drone",
        chords: [
            chord("Em", [4, 0, 0, 0, 0, 0, 0, 0]),
            chord("E5/B", [-3, 0, 4, 0, 0, 1, 4, 0]),
            chord("G/D", [-1, 0, 0, 0, 0, 2, 1, 0]),
            chord("Dm/A", [-4, 0, 0, 0, 0, 2, 2, 0]),
            chord("A5", [0, 0, 4, 0, 0, 0, 0, 0]),
            chord("C/E", [2, 0, 0, 0, 0, 1, 1, 0]),
            chord("G7sus", [6, 0, 3, 1, 0, 0, 0, 0]),
            chord("Em7", [4, 0, 0, 1, 0, 0, 0, 0]),
        ],
        bass: [52, 47, 50, 45, 45, 52, 43, 52],
    },
    // I-V-vi-IV, common-tone rich.
    Progression {
        song_value: 6,
        mood: "Sunny",
        chords: [
            chord("C", [2, 0, 0, 0, 0, 0, 0, 0]),
            chord("Gsus4", [6, 0, 3, 0, 0, 0, 0, 0]),
            chord("Am", [0, 0, 0, 0, 0, 0, 2, 0]),
            chord("F", [5, 0, 0, 0, 0, 0, 0, 0]),
            chord("Cadd4", [2, 0, 0, 3, 0, 0, 0, 2]),
            chord("Gsus4", [6, 0, 3, 0, 0, 0, 0, 0]),
            chord("F", [5, 0, 0, 0, 0, 0, 0, 0]),
            chord("C", [2, 0, 0, 0, 0, 0, 1, 0]),
        ],
        bass: [48, 55, 45, 53, 48, 55, 53, 48],
    },
    // The I-V-vi-IV "axis" loop played twice with varied voicings.
    Progression {
        song_value: 7,
        mood: "Lift",
        chords: [
            chord("G", [6, 0, 0, 0, 0, 0, 0, 0]),
            chord("D", [3, 0, 1, 0, 0, 0, 0, 0]),
            chord("Em7", [4, 0, 0, 1, 0, 0, 0, 0]),
            chord("C", [2, 0, 0, 0, 0, 0, 0, 0]),
            chord("G5", [-1, 0, 4, 0, 0, 0, 2, 0]),
            chord("D", [3, 0, 1, 0, 0, 0, 1, 0]),
            chord("E7sus", [4, 0, 4, 1, 0, 0, 0, 0]),
            chord("C", [2, 0, 0, 0, 0, 0, 1, 0]),
        ],
        bass: [43, 50, 52, 48, 43, 50, 52, 48],
    },
    // D lydian: the E over the D pedal is the raised fourth, bright and
    // unresolved. A3 holds through all eight chords.
    Progression {
        song_value: 9,
        mood: "Dawn",
        chords: [
            chord("Dmaj7", [3, 0, 1, 5, 0, 0, 1, 0]),
            chord("E/D", [-3, 0, 1, 0, 0, 7, 0, 0]),
            chord("Bm7", [1, 0, 0, 1, 0, 0, 1, 2]),
            chord("A/C#", [0, 0, 1, 0, 0, 1, 1, 0]),
            chord("Gmaj9", [-1, 0, 0, 2, 5, 0, 2, 0]),
            chord("A6", [0, 0, 1, 9, 0, 0, 1, 2]),
            chord("F#m7", [-1, -1, -1, 1, 0, 0, 2, 0]),
            chord("Bm7", [1, 0, 0, 1, 0, 0, 3, 1]),
        ],
        bass: [50, 50, 47, 49, 43, 45, 42, 47],
    },
    // D dorian: the major IV (G6) is the one bright colour in a minor key.
    Progression {
        song_value: 10,
        mood: "Rain",
        chords: [
            chord("Dm9", [3, 0, 0, 1, 2, 0, 0, 0]),
            chord("G6", [-1, 0, 0, 9, 0, 0, 1, 2]),
            chord("Am7", [0, 0, 0, 1, 0, 0, 3, 0]),
            chord("Cmaj7", [2, 0, 0, 1, 0, 0, 1, 0]),
            chord("Fmaj7", [-2, 0, 0, 1, 0, 0, 2, 0]),
            chord("Em", [4, 0, 0, 0, 0, 0, 0, 0]),
            chord("G", [-1, 0, 0, 0, 0, 0, 2, 0]),
            chord("Asus", [0, 0, 3, 0, 0, 0, 2, 0]),
        ],
        bass: [50, 43, 45, 48, 41, 52, 43, 45],
    },
    // F# aeolian, the cinematic i-VI-III-VII, each half closing on a
    // suspended or minor fifth that leans back home.
    Progression {
        song_value: 11,
        mood: "Night",
        chords: [
            chord("F#m7", [5, 1, -1, 4, 0, 0, 0, 0]),
            chord("Dmaj7", [3, 0, 1, 5, 0, 0, 1, 0]),
            chord("A", [0, 0, 1, 0, 0, 0, 2, 0]),
            chord("Esus4", [4, 0, 3, 0, 0, 0, 0, 0]),
            chord("Bm7", [1, 0, 0, 1, 0, 0, 1, 2]),
            chord("Dadd9", [3, 0, 1, 2, 0, 0, 0, 0]),
            chord("A/C#", [0, 0, 1, 0, 0, 1, 1, 0]),
            chord("C#m7", [2, 1, -1, 4, 0, 0, 1, 0]),
        ],
        bass: [42, 50, 45, 52, 47, 50, 49, 49],
    },
    // Eb major warmed by the borrowed minor iv (Abm) in the second half.
    Progression {
        song_value: 12,
        mood: "Glow",
        chords: [
            chord("Ebmaj7", [3, 1, 1, 5, 0, 0, 0, 0]),
            chord("Abmaj7", [0, -1, 1, 5, 0, 0, 3, 0]),
            chord("Cm7", [2, 0, -1, 4, 0, 0, 1, 0]),
            chord("Bbsus4", [0, 1, 3, 0, 0, 0, 1, 0]),
            chord("Fm7", [-2, 0, -1, 4, 0, 0, 2, 0]),
            chord("Abm", [0, -1, 0, 0, 0, 0, 2, 0]),
            chord("Eb/G", [-3, -1, 1, 0, 0, 1, 2, 0]),
            chord("Bb7sus", [0, 1, 3, 1, 0, 0, 1, 2]),
        ],
        bass: [51, 44, 48, 46, 41, 44, 43, 46],
    },
    // C mixolydian: the flat seventh (Bb) keeps it floating instead of
    // resolving.
    Progression {
        song_value: 13,
        mood: "Float",
        chords: [
            chord("Cadd9", [2, 0, 0, 2, 0, 0, 1, 0]),
            chord("Bb", [0, 1, 1, 0, 0, 0, 2, 0]),
            chord("F/A", [-2, 0, 0, 0, 0, 1, 2, 0]),
            chord("Gm7", [-1, 0, -1, 1, 0, 0, 2, 0]),
            chord("Dm7", [3, 0, 0, 1, 0, 0, 1, 0]),
            chord("Bbmaj7", [0, 1, 1, 5, 0, 0, 3, 0]),
            chord("F", [5, 0, 0, 0, 0, 0, 0, 0]),
            chord("Csus4", [2, 0, 3, 0, 0, 0, 1, 0]),
        ],
        bass: [48, 46, 45, 43, 50, 46, 41, 48],
    },
    // C minor: the first half moves over a held C pedal, the second walks
    // down from the minor v and hangs on a suspended fifth.
    Progression {
        song_value: 14,
        mood: "Deep",
        chords: [
            chord("Cm", [2, 0, -1, 0, 0, 0, 1, 0]),
            chord("Fm/C", [-2, 0, -1, 0, 0, 2, 1, 0]),
            chord("Ab/C", [0, -1, 1, 0, 0, 1, 1, 0]),
            chord("Bb/C", [0, 1, 1, 0, 0, 4, 0, 0]),
            chord("Gm7", [-1, 0, -1, 1, 0, 0, 2, 0]),
            chord("Ebmaj7", [3, 1, 1, 5, 0, 0, 0, 0]),
            chord("Abmaj7", [0, -1, 1, 5, 0, 0, 3, 0]),
            chord("Gsus4", [-1, 0, 3, 0, 0, 0, 2, 0]),
        ],
        bass: [48, 48, 48, 48, 43, 51, 44, 43],
    },
    // F minor, inferred from Jay Hosking's "Crown": a consonant first half
    // opens into a warm major lift in the second.
    Progression {
        song_value: 15,
        mood: "Hosking",
        chords: [
            chord("Fm7", [5, 0, -1, 4, 0, 0, 0, 0]),
            chord("Abmaj7/C", [0, -1, 1, 5, 0, 1, 1, 2]),
            chord("Bbm7", [0, 1, 0, 1, 0, 0, 1, 0]),
            chord("Db/F", [2, 1, 0, 0, 0, 1, 4, 0]),
            chord("Fm9/Ab", [-2, 0, -1, 2, 4, 1, 0, 0]),
            chord("Ebadd9/G", [3, 1, 1, 2, 0, 1, 0, 0]),
            chord("Bbadd9/D", [0, 1, 1, 2, 0, 1, 0, 0]),
            chord("Dbmaj7", [2, 1, 0, 1, 0, 0, 0, 0]),
        ],
        bass: [41, 48, 46, 53, 44, 55, 50, 49],
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

/// Resolve an authored preset for catalog regression tests.
#[cfg(test)]
pub(crate) fn pad_chord_midi(progression: usize, slot: usize) -> [i32; 4] {
    PROGRESSIONS[progression % PROGRESSIONS.len()].chords[slot % CHORD_SLOT_COUNT].notes()
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
// "Custom" is one more progression choice with its own authored chord bank.
// Built-ins resolve the same builder through authored presets and overrides.
// `progression_index`/`pad_chord_tones` are the single chord-source path shared by Pad, Bass, Arp, and Lead: every
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

    /// One full pass through this window, shared by playback and morph timing.
    pub(crate) fn phrase_beats(self, c: &PadControls) -> f64 {
        GridSpec::new(c.chord_bars * 4.0, 0.0, 0.0).interval_beats * self.count as f64
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
/// Window and length edits take effect at the next chord boundary without
/// cutting short the sounding chord. A window that still contains the current
/// slot continues with its new successor; if the current slot was removed but
/// its old successor remains, that successor plays next. Otherwise the new
/// window begins at its offset. Switching progressions always begins there
/// because the slot numbers now name different chords. The old chord length
/// times the pending boundary and the new length times later boundaries.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ProgressionCursor {
    pub(crate) window: ChordWindow,
    /// `pad.chord_bars` in beats, as the phrase started with it.
    pub(crate) chord_beats: f32,
    pub(crate) step: usize,
    /// Transport beat the next chord starts on; set on the first tick, the
    /// first chord boundary after the engine starts.
    next_chord_beat: Option<f64>,
    morph_phrase_start: Option<f64>,
}

impl ProgressionCursor {
    pub(crate) fn new(c: &PadControls) -> Self {
        Self {
            window: ChordWindow::requested(c),
            chord_beats: c.chord_bars * 4.0,
            step: 0,
            next_chord_beat: None,
            morph_phrase_start: None,
        }
    }

    /// The table slot sounding now.
    pub(crate) fn slot(&self) -> usize {
        self.window.slot(self.step)
    }

    /// Start of the sounding phrase, for a live auto toggle after chord edits.
    pub(crate) fn phrase_start_beat(&self) -> f64 {
        self.next_chord_beat.map_or(0.0, |next| {
            next - f64::from(self.chord_beats) * (self.step + 1) as f64
        })
    }

    /// Moves the cursor up to `timing`'s beat, reading `c` only at a chord
    /// boundary. Returns whether a new chord starts on this tick.
    pub(crate) fn tick(&mut self, c: &PadControls, timing: TimingContext) -> bool {
        if timing.transport == Transport::Stopped {
            return false;
        }
        if let Some(start) = timing.morph_phrase_start
            && self.morph_phrase_start != Some(start)
        {
            self.morph_phrase_start = Some(start);
            self.window = ChordWindow::requested(c);
            self.chord_beats = c.chord_bars * 4.0;
            let interval = GridSpec::new(self.chord_beats, 0.0, 0.0).interval_beats;
            let chord = ((timing.beat - start).max(0.0) / interval).floor();
            self.step = chord as usize % self.window.count;
            self.next_chord_beat = Some(start + (chord + 1.0) * interval);
            return timing.beat > 0.0;
        }
        self.morph_phrase_start = timing.morph_phrase_start;
        let next = *self.next_chord_beat.get_or_insert_with(|| {
            GridSpec::new(self.chord_beats, 0.0, 0.0)
                .hit_after(timing.beat)
                .beat
        });
        // A stopped transport holds its beat, and like every grid it fires
        // nothing until the clock runs again.
        if timing.beat + GRID_BEAT_EPSILON < next {
            return false;
        }
        let window = ChordWindow::requested(c);
        let chord_beats = c.chord_bars * 4.0;
        let old_next_slot = self.window.slot((self.step + 1) % self.window.count);
        self.step = if window.progression != self.window.progression {
            0
        } else if let Some(current_step) = window.slots().position(|slot| slot == self.slot()) {
            (current_step + 1) % window.count
        } else {
            window
                .slots()
                .position(|slot| slot == old_next_slot)
                .unwrap_or(0)
        };
        self.window = window;
        self.chord_beats = chord_beats;
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
    pad_chord_notes_with_slot(c.chord_slot(progression, slot))
}

/// The displayed identity follows the same resolved chord as every voice.
pub(crate) fn pad_chord_name(c: &PadControls, progression: usize, slot: usize) -> String {
    custom_chord_name(c.chord_slot(progression, slot))
}
