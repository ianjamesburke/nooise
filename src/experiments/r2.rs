use std::error::Error;

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use crate::audio::{self, StereoEngine};
use crate::fx::lfo::DriftingLfo;
use crate::fx::panner::StereoPanner;
use crate::fx::reverb::Freeverb;
use crate::synth::noise::WhiteNoise;

const TEMPO_BPM: f32 = 120.0;
const STEPS_PER_BEAT: f32 = 4.0;
const PATTERN: [f32; 16] = [
    0.74, 0.0, 0.24, 0.08, 0.42, 0.16, 0.0, 0.28, 0.62, 0.1, 0.34, 0.0, 0.46, 0.18, 0.0, 0.26,
];

pub(crate) fn run() -> Result<(), Box<dyn Error>> {
    audio::run_engine("r2", R2Engine::new)
}

struct R2Engine {
    current_sample: u64,
    step_samples: u64,
    next_step_sample: u64,
    step_index: usize,
    sample_rate: f32,
    hits: Vec<NoiseHit>,
    rng: StdRng,
    pan_lfo: DriftingLfo,
    reverb: Freeverb,
}

impl R2Engine {
    fn new(sample_rate: f32) -> Self {
        Self {
            current_sample: 0,
            step_samples: (sample_rate * 60.0 / TEMPO_BPM / STEPS_PER_BEAT).round() as u64,
            next_step_sample: 0,
            step_index: 0,
            sample_rate,
            hits: Vec::with_capacity(12),
            rng: StdRng::from_entropy(),
            pan_lfo: DriftingLfo::new(1.0 / 11.0, sample_rate),
            reverb: Freeverb::new(sample_rate, 0.54, 0.58, 0.28),
        }
    }

    fn trigger_due_steps(&mut self) {
        while self.current_sample >= self.next_step_sample {
            let accent = PATTERN[self.step_index % PATTERN.len()];
            if accent > 0.0 && self.rng.gen_bool(hit_probability(accent) as f64) {
                let velocity = (accent + self.rng.gen_range(-0.12..0.18)).clamp(0.08, 0.84);
                let decay_seconds = self.rng.gen_range(0.035..0.18) * (0.75 + accent);
                let pan = self.rng.gen_range(-0.66..0.66);
                self.hits.push(NoiseHit::new(
                    velocity,
                    decay_seconds,
                    pan,
                    self.sample_rate,
                ));
            }

            self.step_index = (self.step_index + 1) % PATTERN.len();
            self.next_step_sample += self.step_samples;
        }
    }

    fn next_noise_sample(&mut self) -> (f32, f32) {
        let mut left = 0.0;
        let mut right = 0.0;

        for hit in &mut self.hits {
            let (hit_left, hit_right) = hit.next_stereo(&mut self.rng);
            left += hit_left;
            right += hit_right;
        }
        self.hits.retain(|hit| !hit.is_done());

        (left, right)
    }
}

impl StereoEngine for R2Engine {
    fn next_stereo(&mut self) -> (f32, f32) {
        self.trigger_due_steps();

        let (mut left, mut right) = self.next_noise_sample();
        let drift = self.pan_lfo.next(&mut self.rng, 1.0 / 18.0, 1.0 / 7.0) * 0.08;
        let (wet_left, wet_right) = self.reverb.process(left, right);
        left = left * (1.0 - drift) + wet_left;
        right = right * (1.0 + drift) + wet_right;

        self.current_sample += 1;
        (left.clamp(-0.88, 0.88), right.clamp(-0.88, 0.88))
    }
}

struct NoiseHit {
    noise: WhiteNoise,
    age_samples: u64,
    length_samples: u64,
    attack_samples: u64,
    decay_samples: f32,
    amplitude: f32,
    pan: f32,
    smoothing: f32,
}

impl NoiseHit {
    fn new(amplitude: f32, decay_seconds: f32, pan: f32, sample_rate: f32) -> Self {
        Self {
            noise: WhiteNoise::new(),
            age_samples: 0,
            length_samples: (decay_seconds * sample_rate * 3.4).round() as u64,
            attack_samples: (0.004 * sample_rate).round() as u64,
            decay_samples: (decay_seconds * sample_rate).max(1.0),
            amplitude,
            pan,
            smoothing: 0.055 + amplitude * 0.09,
        }
    }

    fn next_stereo<R: Rng>(&mut self, rng: &mut R) -> (f32, f32) {
        let age = self.age_samples as f32;
        let attack = if self.attack_samples <= 1 {
            1.0
        } else {
            (age / self.attack_samples as f32).min(1.0)
        };
        let decay = (-age / self.decay_samples).exp();
        let end_fade = if self.age_samples + self.attack_samples >= self.length_samples {
            let remaining = self.length_samples.saturating_sub(self.age_samples) as f32;
            (remaining / self.attack_samples.max(1) as f32).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let sample = self.noise.next_filtered(rng, self.smoothing)
            * attack
            * decay
            * end_fade
            * self.amplitude
            * 0.22;

        self.age_samples += 1;
        StereoPanner::equal_power(sample, self.pan)
    }

    fn is_done(&self) -> bool {
        self.age_samples >= self.length_samples
    }
}

fn hit_probability(accent: f32) -> f32 {
    (0.28 + accent * 0.72).clamp(0.0, 1.0)
}
