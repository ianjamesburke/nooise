use super::*;

/// Bass line for each progression, authored independently of the Pad's
/// chord voicings (one MIDI note per step, same 8-step indexing as
/// PROGRESSIONS). B/C/D currently mirror their chord's lowest tone; A
/// diverges deliberately (step 3 walks to G2 instead of following the
/// B-chord's root) to give the bass its own melodic movement. E/F/G/H
/// mostly follow their chord's root (transposed down an octave where the
/// pad voicing sits too high for the bass register).
pub(crate) const BASS_LINES: [[i32; 8]; 8] = [
    [45, 47, 45, 43, 52, 53, 45, 45], // A
    [45, 50, 48, 43, 41, 52, 45, 43], // B
    [45, 41, 48, 43, 50, 52, 47, 43], // C
    [45, 41, 48, 43, 50, 52, 47, 43], // D
    [45, 46, 48, 50, 52, 43, 43, 45], // E: dark phrygian, walks up then falls back
    [52, 47, 50, 45, 45, 52, 43, 52], // F: suspended drone, mostly pedal E
    [48, 55, 45, 53, 48, 55, 53, 48], // G: bright C-G-Am-F pop bass
    [43, 50, 52, 48, 43, 50, 52, 48], // H: bright G-D-Em-C axis-loop bass
];

pub(crate) fn bass_root_note(progression: usize, step: usize) -> i32 {
    BASS_LINES[progression % BASS_LINES.len()][step % 8]
}

// ============================================================
// Bass engine (follows the Pad's chord root on a rhythm pattern)
// ============================================================

/// Four 16-step rhythm patterns (one bar at 16th-note resolution: counted
/// "1 e & a 2 e & a 3 e & a 4 e & a"). A/B/C/D selects between them; `true`
/// re-articulates the bass note at that step.
pub(crate) const BASS_RHYTHMS: [[bool; 16]; 4] = [
    // A: quarter notes on the beat
    [
        true, false, false, false, true, false, false, false, true, false, false, false, true,
        false, false, false,
    ],
    // B: syncopated — pickup into 1, push before 3, quick pickups into 4
    [
        true, false, false, true, false, false, true, false, true, true, false, false, true, true,
        false, false,
    ],
    // C: straight eighths — steady walking-bass feel
    [
        true, false, true, false, true, false, true, false, true, false, true, false, true, false,
        true, false,
    ],
    // D: busy 16th groove
    [
        true, false, false, true, false, false, true, false, true, false, false, true, false,
        false, true, false,
    ],
];

pub(crate) const MAX_BASS_VOICES: usize = 3;

/// Fixed duration of one rhythm-pattern step (a 16th note). Step timing never
/// changes; `interval_beats` instead crops how many steps of the 16-step
/// phrase play before looping back to step 0 (or extends the loop with
/// trailing silence, for a "gap" feel).
pub(crate) const BASS_STEP_BEATS: f32 = 0.25;

pub(crate) struct BassEngine {
    pub(crate) sample_rate: f32,
    pub(crate) chord_trigger: GridTrigger,
    pub(crate) step_index: usize,
    pub(crate) step_trigger: GridTrigger,
    pub(crate) rhythm_step: usize,
    pub(crate) voices: Vec<BassVoice>,
}

impl BassEngine {
    pub(crate) fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            chord_trigger: GridTrigger::after_start(),
            step_index: 0,
            step_trigger: GridTrigger::new(),
            rhythm_step: BASS_RHYTHMS[0].len() - 1,
            voices: Vec::with_capacity(MAX_BASS_VOICES),
        }
    }

    pub(crate) fn next(
        &mut self,
        c: &BassControls,
        pad: &PadControls,
        tune: f32,
        timing: TimingContext,
    ) -> (f32, f32) {
        let progression = (pad.progression.round() as i64).rem_euclid(8) as usize;
        if self.chord_trigger.pop(timing, pad.chord_bars * 4.0, 0.0) {
            self.step_index = (self.step_index + 1) % 8;
        }

        let loop_len = (c.interval_beats / BASS_STEP_BEATS)
            .round()
            .clamp(1.0, 32.0) as usize;
        if self
            .step_trigger
            .pop(timing, BASS_STEP_BEATS, c.offset_beats)
        {
            self.rhythm_step = (self.rhythm_step + 1) % loop_len;
            let rhythm = (c.rhythm.round() as usize) % BASS_RHYTHMS.len();
            let hit = self.rhythm_step < BASS_RHYTHMS[rhythm].len()
                && BASS_RHYTHMS[rhythm][self.rhythm_step];
            if hit {
                let note =
                    bass_root_note(progression, self.step_index) + (c.octave.round() as i32) * 12;
                let hz = midi_to_hz(note) * tune_ratio(tune);
                for voice in &mut self.voices {
                    voice.release();
                }
                if self.voices.len() >= MAX_BASS_VOICES {
                    let remove_count = self.voices.len() + 1 - MAX_BASS_VOICES;
                    self.voices.drain(0..remove_count);
                }
                self.voices.push(BassVoice::new(
                    bass_type_index(c.voice_type),
                    hz,
                    c.attack_time,
                    c.decay_time,
                    c.drive,
                    self.sample_rate,
                ));
            }
        }

        let mut l = 0.0f32;
        let mut r = 0.0f32;
        for voice in &mut self.voices {
            let (vl, vr) = voice.next();
            l += vl;
            r += vr;
        }
        self.voices.retain(|v| !v.is_done());

        (l * c.level, r * c.level)
    }
}

/// `bass.type` selects the voice character used for every new bass note.
/// Index 0 (`Sub`) is the legacy voice, unchanged and the default; switching
/// type never touches the shared trigger/rhythm, pitch, or reseed paths in
/// `BassEngine::next` above.
pub(crate) enum BassVoice {
    Sub(SubBassVoice),
    Saw(SawBassVoice),
    Pluck(PluckBassVoice),
}

impl BassVoice {
    pub(crate) fn new(
        voice_type: usize,
        hz: f32,
        attack_time: f32,
        decay_time: f32,
        drive: f32,
        sample_rate: f32,
    ) -> Self {
        match voice_type {
            0 => Self::Sub(SubBassVoice::new(
                hz,
                attack_time,
                decay_time,
                drive,
                sample_rate,
            )),
            1 => Self::Saw(SawBassVoice::new(
                hz,
                attack_time,
                decay_time,
                drive,
                sample_rate,
            )),
            _ => Self::Pluck(PluckBassVoice::new(
                hz,
                attack_time,
                decay_time,
                drive,
                sample_rate,
            )),
        }
    }

    pub(crate) fn next(&mut self) -> (f32, f32) {
        match self {
            Self::Sub(voice) => voice.next(),
            Self::Saw(voice) => voice.next(),
            Self::Pluck(voice) => voice.next(),
        }
    }

    pub(crate) fn release(&mut self) {
        match self {
            Self::Sub(voice) => voice.release(),
            Self::Saw(voice) => voice.release(),
            Self::Pluck(voice) => voice.release(),
        }
    }

    pub(crate) fn is_done(&self) -> bool {
        match self {
            Self::Sub(voice) => voice.is_done(),
            Self::Saw(voice) => voice.is_done(),
            Self::Pluck(voice) => voice.is_done(),
        }
    }
}

/// Type 0 (default): the original bass voice, byte-for-byte unchanged. A
/// single sine oscillator through the shared attack/decay envelope, with an
/// optional soft-clip drive stage.
pub(crate) struct SubBassVoice {
    pub(crate) osc: SineOscillator,
    pub(crate) envelope: Adsr,
    pub(crate) drive: f32,
}

impl SubBassVoice {
    pub(crate) fn new(
        hz: f32,
        attack_time: f32,
        decay_time: f32,
        drive: f32,
        sample_rate: f32,
    ) -> Self {
        Self {
            osc: SineOscillator::new(hz, sample_rate),
            // No sustain — Decay carries the note fully to silence, like the
            // Perc voice's percussive envelope. Decay also doubles as the
            // release curve, smoothing the cutoff if a hit retriggers before
            // the previous note has fully decayed.
            envelope: Adsr::new(attack_time, decay_time, 0.0, decay_time, sample_rate),
            drive,
        }
    }

    pub(crate) fn next(&mut self) -> (f32, f32) {
        let mut s = self.osc.next() * self.envelope.next();
        if self.drive > 0.0 {
            let driven = s * (1.0 + self.drive * 8.0);
            s = driven / (1.0 + driven.abs()) * (1.0 + self.drive * 0.5);
        }
        StereoPanner::equal_power(s, 0.0)
    }

    pub(crate) fn release(&mut self) {
        self.envelope.note_off();
    }

    pub(crate) fn is_done(&self) -> bool {
        self.envelope.is_done()
    }
}

/// Number of stacked harmonics in the Saw voice's additive stack (fundamental
/// plus three overtones).
const BASS_SAW_HARMONIC_COUNT: usize = 4;
/// Per-harmonic amplitude weights, steeper than a true sawtooth's `1/n`
/// falloff so the extra brightness stays bass-register appropriate (tamed
/// highs) instead of turning harsh. Chosen (together with
/// `BASS_SAW_OUTPUT_GAIN`) so the voice sits at the same perceived level as
/// the Sub and Pluck voices.
const BASS_SAW_HARMONIC_GAINS: [f32; BASS_SAW_HARMONIC_COUNT] = [1.0, 0.46, 0.24, 0.14];
/// Output trim balancing the additive stack's summed energy against the
/// single-oscillator Sub/Pluck voices.
const BASS_SAW_OUTPUT_GAIN: f32 = 0.62;

/// Type 1: a brighter, saw-leaning character. An additive stack of sine
/// harmonics (not a literal bandlimited sawtooth oscillator — none exists in
/// `synth::oscillator` — but a steeper-than-`1/n` weighted approximation)
/// gives it more harmonic content than the Sub voice while a fast tilt keeps
/// the top end tame. Shares the Sub voice's envelope shape and drive stage.
pub(crate) struct SawBassVoice {
    pub(crate) oscillators: [SineOscillator; BASS_SAW_HARMONIC_COUNT],
    pub(crate) envelope: Adsr,
    pub(crate) drive: f32,
}

impl SawBassVoice {
    pub(crate) fn new(
        hz: f32,
        attack_time: f32,
        decay_time: f32,
        drive: f32,
        sample_rate: f32,
    ) -> Self {
        Self {
            oscillators: std::array::from_fn(|index| {
                SineOscillator::new(hz * (index + 1) as f32, sample_rate)
            }),
            envelope: Adsr::new(attack_time, decay_time, 0.0, decay_time, sample_rate),
            drive,
        }
    }

    pub(crate) fn next(&mut self) -> (f32, f32) {
        let mut raw = 0.0f32;
        for (osc, gain) in self.oscillators.iter_mut().zip(BASS_SAW_HARMONIC_GAINS) {
            raw += osc.next() * gain;
        }
        let mut s = raw * BASS_SAW_OUTPUT_GAIN * self.envelope.next();
        if self.drive > 0.0 {
            let driven = s * (1.0 + self.drive * 8.0);
            s = driven / (1.0 + driven.abs()) * (1.0 + self.drive * 0.5);
        }
        StereoPanner::equal_power(s, 0.0)
    }

    pub(crate) fn release(&mut self) {
        self.envelope.note_off();
    }

    pub(crate) fn is_done(&self) -> bool {
        self.envelope.is_done()
    }
}

/// Fraction of the control-supplied attack/decay used by the Pluck voice's
/// body envelope — shorter than Sub/Saw so it decays faster while still
/// tracking the shared `bass.attack_time`/`bass.decay_time` controls.
const BASS_PLUCK_ENVELOPE_SCALE: f32 = 0.4;
/// Fixed decay time (seconds) of the Pluck voice's attack transient (an
/// octave-up click that fires once per note, independent of the shared decay
/// control).
const BASS_PLUCK_TRANSIENT_DECAY_SECONDS: f32 = 0.02;
/// Mix level of the transient click relative to the sustained body.
const BASS_PLUCK_TRANSIENT_MIX: f32 = 0.5;
/// Output trim balancing the Pluck voice's body + transient against the Sub
/// voice's single-oscillator level.
const BASS_PLUCK_OUTPUT_GAIN: f32 = 0.82;

/// Type 2: a plucked/shorter character. Faster attack/decay than Sub (scaled
/// via `BASS_PLUCK_ENVELOPE_SCALE`) plus a short octave-up transient at the
/// onset of the note for a percussive pluck attack.
pub(crate) struct PluckBassVoice {
    pub(crate) osc: SineOscillator,
    pub(crate) transient_osc: SineOscillator,
    pub(crate) envelope: Adsr,
    pub(crate) transient_samples_remaining: u64,
    pub(crate) transient_total_samples: f32,
    pub(crate) drive: f32,
}

impl PluckBassVoice {
    pub(crate) fn new(
        hz: f32,
        attack_time: f32,
        decay_time: f32,
        drive: f32,
        sample_rate: f32,
    ) -> Self {
        let attack_time = (attack_time * BASS_PLUCK_ENVELOPE_SCALE).max(0.001);
        let decay_time = (decay_time * BASS_PLUCK_ENVELOPE_SCALE).max(0.001);
        let transient_total_samples = (BASS_PLUCK_TRANSIENT_DECAY_SECONDS * sample_rate).max(1.0);
        Self {
            osc: SineOscillator::new(hz, sample_rate),
            transient_osc: SineOscillator::new(hz * 2.0, sample_rate),
            envelope: Adsr::new(attack_time, decay_time, 0.0, decay_time, sample_rate),
            transient_samples_remaining: transient_total_samples as u64,
            transient_total_samples,
            drive,
        }
    }

    pub(crate) fn next(&mut self) -> (f32, f32) {
        let body = self.osc.next() * self.envelope.next();

        let transient = if self.transient_samples_remaining > 0 {
            let elapsed = self.transient_total_samples - self.transient_samples_remaining as f32;
            let gain = (-elapsed / self.transient_total_samples * 5.0).exp();
            self.transient_samples_remaining -= 1;
            self.transient_osc.next() * gain
        } else {
            0.0
        };

        let mut s = (body + transient * BASS_PLUCK_TRANSIENT_MIX) * BASS_PLUCK_OUTPUT_GAIN;
        if self.drive > 0.0 {
            let driven = s * (1.0 + self.drive * 8.0);
            s = driven / (1.0 + driven.abs()) * (1.0 + self.drive * 0.5);
        }
        StereoPanner::equal_power(s, 0.0)
    }

    pub(crate) fn release(&mut self) {
        self.envelope.note_off();
    }

    pub(crate) fn is_done(&self) -> bool {
        self.envelope.is_done()
    }
}
