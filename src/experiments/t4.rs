use std::error::Error;

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use crate::audio::{self, StereoEngine};
use crate::fx::lfo::DriftingLfo;
use crate::fx::panner::StereoPanner;
use crate::fx::reverb::Freeverb;
use crate::sequencer;
use crate::synth::envelope::Adsr;
use crate::synth::fm::BellVoice;
use crate::synth::noise::WhiteNoise;
use crate::synth::oscillator::SineOscillator;

const TEMPO_BPM: f32 = 112.0;
const BEATS_PER_BAR: f32 = 4.0;
const PHRASE_BARS: f32 = 3.0;
const GRID_BARS: u32 = 4;
const PAD_BARS: f32 = 8.0;

#[derive(Clone)]
struct PhraseNote {
    degree: i32,
    beats: f32,
    velocity: f32,
}

pub(crate) fn run() -> Result<(), Box<dyn Error>> {
    audio::run_engine("t4", T4Engine::new)
}

pub(crate) struct T4Engine {
    sample_rate: f32,
    current_sample: u64,
    beat_samples: f32,
    phrase_samples: u64,
    alignment_samples: u64,
    phrase_start_sample: u64,
    next_event_sample: u64,
    next_note_index: usize,
    phrase: Vec<PhraseNote>,
    melody_voices: Vec<BellVoice>,
    pad_voices: Vec<PadVoice>,
    pad_cycle_samples: u64,
    next_pad_sample: u64,
    pad_chord_index: usize,
    reverb: Freeverb,
    pan_lfo: DriftingLfo,
    eq_lfo: DriftingLfo,
    eq_left: DriftingEq,
    eq_right: DriftingEq,
    rng: StdRng,
    air: WhiteNoise,
}

impl T4Engine {
    pub(crate) fn new(sample_rate: f32) -> Self {
        let beat_samples = sample_rate * 60.0 / TEMPO_BPM;
        let bar_samples = beat_samples * BEATS_PER_BAR;
        let phrase_samples = (bar_samples * PHRASE_BARS).round() as u64;
        let alignment_bars = sequencer::lcm(PHRASE_BARS as u32, GRID_BARS);

        Self {
            sample_rate,
            current_sample: 0,
            beat_samples,
            phrase_samples,
            alignment_samples: (bar_samples * alignment_bars as f32).round() as u64,
            phrase_start_sample: 0,
            next_event_sample: 0,
            next_note_index: 0,
            phrase: base_phrase(),
            melody_voices: Vec::with_capacity(32),
            pad_voices: Vec::with_capacity(24),
            pad_cycle_samples: (bar_samples * PAD_BARS).round() as u64,
            next_pad_sample: 0,
            pad_chord_index: 0,
            reverb: Freeverb::new(sample_rate, 0.84, 0.38, 0.9),
            pan_lfo: DriftingLfo::new(1.0 / 18.0, sample_rate),
            eq_lfo: DriftingLfo::new(1.0 / 32.0, sample_rate),
            eq_left: DriftingEq::new(),
            eq_right: DriftingEq::new(),
            rng: StdRng::from_entropy(),
            air: WhiteNoise::new(),
        }
    }

    pub(crate) fn next_tonal(&mut self) -> (f32, f32) {
        self.advance_phrase_if_needed();
        self.trigger_due_notes();
        self.trigger_pad_if_due();

        let melody = self.next_melody_sample();
        let pad = self.next_pad_sample();
        let drift = self.pan_lfo.next(&mut self.rng, 1.0 / 28.0, 1.0 / 11.0);
        let grid_phase = self.current_sample as f32 / self.alignment_samples as f32;
        let grid_sway = (grid_phase * std::f32::consts::TAU).sin() * 0.14;
        let (melody_left, melody_right) =
            StereoPanner::equal_power(melody, drift * 0.36 + grid_sway);
        let (pad_left, pad_right) = StereoPanner::equal_power(pad, -drift * 0.18);
        let dry_left = melody_left * 0.68 + pad_left * 0.72;
        let dry_right = melody_right * 0.68 + pad_right * 0.72;
        let (wet_left, wet_right) = self.reverb.process(dry_left, dry_right);
        let eq_motion = self.eq_lfo.next(&mut self.rng, 1.0 / 58.0, 1.0 / 24.0);
        let air = self.air.next_filtered(&mut self.rng, 0.0006) * 0.0005;
        let left = self.eq_left.process(dry_left * 0.56 + wet_left, eq_motion) + air;
        let right = self
            .eq_right
            .process(dry_right * 0.56 + wet_right, eq_motion)
            + air;

        self.current_sample += 1;
        (left.clamp(-0.95, 0.95), right.clamp(-0.95, 0.95))
    }

    fn advance_phrase_if_needed(&mut self) {
        if self.current_sample < self.phrase_start_sample + self.phrase_samples {
            return;
        }

        while self.current_sample >= self.phrase_start_sample + self.phrase_samples {
            self.phrase_start_sample += self.phrase_samples;
        }
        self.phrase = mutate_phrase(&self.phrase, &mut self.rng);
        self.next_note_index = 0;
        self.next_event_sample = self.phrase_start_sample;
    }

    fn trigger_due_notes(&mut self) {
        while self.next_note_index < self.phrase.len()
            && self.current_sample >= self.next_event_sample
            && self.current_sample < self.phrase_start_sample + self.phrase_samples
        {
            let note = &self.phrase[self.next_note_index];
            let hold_seconds = note.beats * 60.0 / TEMPO_BPM * 1.08;
            self.melody_voices.push(BellVoice::new(
                frequency_for_degree(note.degree),
                hold_seconds,
                note.velocity,
                self.sample_rate,
            ));
            self.next_event_sample += (note.beats * self.beat_samples).round() as u64;
            self.next_note_index += 1;
        }
    }

    fn trigger_pad_if_due(&mut self) {
        while self.current_sample >= self.next_pad_sample {
            let chord = pad_chord(self.pad_chord_index);
            let hold_seconds = self.pad_cycle_samples as f32 / self.sample_rate * 0.92;
            for (index, degree) in chord.iter().enumerate() {
                let velocity = 0.07 + index as f32 * 0.006;
                self.pad_voices.push(PadVoice::new(
                    frequency_for_degree(*degree),
                    hold_seconds,
                    velocity,
                    self.sample_rate,
                ));
            }
            self.pad_chord_index = self.pad_chord_index.wrapping_add(1);
            self.next_pad_sample += self.pad_cycle_samples;
        }
    }

    fn next_melody_sample(&mut self) -> f32 {
        let mut sample = 0.0;
        for voice in &mut self.melody_voices {
            sample += voice.next();
        }
        self.melody_voices.retain(|voice| !voice.is_done());
        sample
    }

    fn next_pad_sample(&mut self) -> f32 {
        let mut sample = 0.0;
        for voice in &mut self.pad_voices {
            sample += voice.next();
        }
        self.pad_voices.retain(|voice| !voice.is_done());
        sample
    }
}

impl StereoEngine for T4Engine {
    fn next_stereo(&mut self) -> (f32, f32) {
        self.next_tonal()
    }
}

struct PadVoice {
    fundamental: SineOscillator,
    overtone: SineOscillator,
    envelope: Adsr,
    age_samples: u64,
    hold_samples: u64,
    velocity: f32,
    released: bool,
}

impl PadVoice {
    fn new(frequency_hz: f32, hold_seconds: f32, velocity: f32, sample_rate: f32) -> Self {
        let envelope = Adsr::new(3.4, 5.2, 0.68, 7.0, sample_rate);
        let hold_samples = envelope.samples_from_seconds(hold_seconds);

        Self {
            fundamental: SineOscillator::new(frequency_hz, sample_rate),
            overtone: SineOscillator::new(frequency_hz * 2.002, sample_rate),
            envelope,
            age_samples: 0,
            hold_samples,
            velocity,
            released: false,
        }
    }

    fn next(&mut self) -> f32 {
        if !self.released && self.age_samples >= self.hold_samples {
            self.envelope.note_off();
            self.released = true;
        }

        let amp = self.envelope.next();
        self.age_samples += 1;
        (self.fundamental.next() * 0.86 + self.overtone.next() * 0.14) * amp * self.velocity
    }

    fn is_done(&self) -> bool {
        self.envelope.is_done()
    }
}

struct DriftingEq {
    low_state: f32,
}

impl DriftingEq {
    fn new() -> Self {
        Self { low_state: 0.0 }
    }

    fn process(&mut self, input: f32, motion: f32) -> f32 {
        let normalized = (motion + 1.0) * 0.5;
        let smoothing = 0.004 + normalized * 0.036;
        self.low_state += (input - self.low_state) * smoothing;
        let high = input - self.low_state;
        let warmth = 1.06 - normalized * 0.18;
        let brightness = 0.68 + normalized * 0.36;

        self.low_state * warmth + high * brightness
    }
}

fn base_phrase() -> Vec<PhraseNote> {
    vec![
        PhraseNote {
            degree: 2,
            beats: 2.0,
            velocity: 0.54,
        },
        PhraseNote {
            degree: 4,
            beats: 2.0,
            velocity: 0.48,
        },
        PhraseNote {
            degree: 6,
            beats: 4.0,
            velocity: 0.5,
        },
        PhraseNote {
            degree: 5,
            beats: 1.0,
            velocity: 0.42,
        },
        PhraseNote {
            degree: 3,
            beats: 3.0,
            velocity: 0.46,
        },
    ]
}

fn mutate_phrase<R: Rng>(phrase: &[PhraseNote], rng: &mut R) -> Vec<PhraseNote> {
    let mut next = phrase.to_vec();
    let note_index = rng.gen_range(0..next.len());
    let movement = if rng.gen_bool(0.5) { 1 } else { -1 };
    next[note_index].degree = (next[note_index].degree + movement).clamp(0, 9);

    let stretch_index = rng.gen_range(0..next.len());
    let shrink_index = (stretch_index + rng.gen_range(1..next.len())) % next.len();
    let delta = rng.gen_range(-0.5..0.75);

    if next[stretch_index].beats + delta >= 1.0 && next[shrink_index].beats - delta >= 1.0 {
        next[stretch_index].beats += delta;
        next[shrink_index].beats -= delta;
    }

    next
}

fn pad_chord(index: usize) -> [i32; 4] {
    const CHORDS: [[i32; 4]; 4] = [
        [-5, -3, 0, 2],
        [-4, -2, 1, 4],
        [-3, -1, 2, 5],
        [-5, -1, 1, 3],
    ];

    CHORDS[index % CHORDS.len()]
}

fn frequency_for_degree(degree: i32) -> f32 {
    let scale = [0.0_f32, 3.0, 5.0, 7.0, 10.0];
    let octave = degree.div_euclid(scale.len() as i32);
    let scale_degree = degree.rem_euclid(scale.len() as i32) as usize;
    let semitones = octave as f32 * 12.0 + scale[scale_degree];
    220.0 * 2.0_f32.powf(semitones / 12.0)
}
