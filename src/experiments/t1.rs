use std::error::Error;

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use crate::audio::{self, StereoEngine};
use crate::fx::lfo::DriftingLfo;
use crate::fx::panner::StereoPanner;
use crate::fx::reverb::Freeverb;
use crate::sequencer;
use crate::synth::envelope::Adsr;
use crate::synth::noise::WhiteNoise;
use crate::synth::oscillator::SineOscillator;

const TEMPO_BPM: f32 = 92.0;
const BEATS_PER_BAR: f32 = 4.0;
const PHRASE_BARS: f32 = 3.0;
const GRID_BARS: u32 = 4;

#[derive(Clone)]
struct PhraseNote {
    degree: i32,
    beats: f32,
    velocity: f32,
}

pub(crate) fn run() -> Result<(), Box<dyn Error>> {
    audio::run_engine("t1", T1Engine::new)
}

struct T1Engine {
    sample_rate: f32,
    current_sample: u64,
    beat_samples: f32,
    phrase_samples: u64,
    alignment_samples: u64,
    phrase_start_sample: u64,
    next_event_sample: u64,
    next_note_index: usize,
    phrase: Vec<PhraseNote>,
    voices: Vec<SoftToneVoice>,
    reverb: Freeverb,
    pan_lfo: DriftingLfo,
    rng: StdRng,
    air: WhiteNoise,
}

impl T1Engine {
    fn new(sample_rate: f32) -> Self {
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
            voices: Vec::with_capacity(24),
            reverb: Freeverb::new(sample_rate, 0.78, 0.46, 0.74),
            pan_lfo: DriftingLfo::new(1.0 / 24.0, sample_rate),
            rng: StdRng::from_entropy(),
            air: WhiteNoise::new(),
        }
    }
}

impl StereoEngine for T1Engine {
    fn next_stereo(&mut self) -> (f32, f32) {
        self.advance_phrase_if_needed();
        self.trigger_due_notes();

        let dry = self.next_voice_sample();
        let drift = self.pan_lfo.next(&mut self.rng, 1.0 / 42.0, 1.0 / 18.0) * 0.14;
        let grid_phase = self.current_sample as f32 / self.alignment_samples as f32;
        let slow_grid_sway = (grid_phase * std::f32::consts::TAU).sin() * 0.045;
        let (dry_left, dry_right) = StereoPanner::equal_power(dry, drift + slow_grid_sway);
        let (wet_left, wet_right) = self.reverb.process(dry_left, dry_right);
        let air = self.air.next_filtered(&mut self.rng, 0.0008) * 0.00035;

        self.current_sample += 1;
        (
            (dry_left * 0.64 + wet_left * 0.86 + air).clamp(-0.95, 0.95),
            (dry_right * 0.64 + wet_right * 0.86 + air).clamp(-0.95, 0.95),
        )
    }
}

impl T1Engine {
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
            let hold_seconds = note.beats * 60.0 / TEMPO_BPM * 1.18;
            self.voices.push(SoftToneVoice::new(
                frequency_for_degree(note.degree),
                hold_seconds,
                note.velocity,
                self.sample_rate,
            ));
            self.next_event_sample += (note.beats * self.beat_samples).round() as u64;
            self.next_note_index += 1;
        }
    }

    fn next_voice_sample(&mut self) -> f32 {
        let mut sample = 0.0;
        for voice in &mut self.voices {
            sample += voice.next();
        }
        self.voices.retain(|voice| !voice.is_done());
        sample
    }
}

struct SoftToneVoice {
    fundamental: SineOscillator,
    overtone: SineOscillator,
    envelope: Adsr,
    age_samples: u64,
    hold_samples: u64,
    velocity: f32,
    released: bool,
}

impl SoftToneVoice {
    fn new(frequency_hz: f32, hold_seconds: f32, velocity: f32, sample_rate: f32) -> Self {
        let envelope = Adsr::new(0.08, 1.2, 0.58, 4.8, sample_rate);
        let hold_samples = envelope.samples_from_seconds(hold_seconds);

        Self {
            fundamental: SineOscillator::new(frequency_hz, sample_rate),
            overtone: SineOscillator::new(frequency_hz * 2.0, sample_rate),
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
        (self.fundamental.next() * 0.94 + self.overtone.next() * 0.06) * amp * self.velocity * 0.18
    }

    fn is_done(&self) -> bool {
        self.envelope.is_done()
    }
}

fn base_phrase() -> Vec<PhraseNote> {
    vec![
        PhraseNote {
            degree: 2,
            beats: 2.0,
            velocity: 0.58,
        },
        PhraseNote {
            degree: 4,
            beats: 2.0,
            velocity: 0.52,
        },
        PhraseNote {
            degree: 6,
            beats: 4.0,
            velocity: 0.54,
        },
        PhraseNote {
            degree: 5,
            beats: 1.0,
            velocity: 0.44,
        },
        PhraseNote {
            degree: 3,
            beats: 3.0,
            velocity: 0.5,
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

fn frequency_for_degree(degree: i32) -> f32 {
    let scale = [0.0_f32, 3.0, 5.0, 7.0, 10.0];
    let octave = degree.div_euclid(scale.len() as i32);
    let scale_degree = degree.rem_euclid(scale.len() as i32) as usize;
    let semitones = octave as f32 * 12.0 + scale[scale_degree];
    220.0 * 2.0_f32.powf(semitones / 12.0)
}
