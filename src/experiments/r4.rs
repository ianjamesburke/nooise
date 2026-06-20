use std::error::Error;

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use crate::audio::{self, StereoEngine};
use crate::experiments::t4::T4Engine;
use crate::fx::panner::StereoPanner;
use crate::synth::noise::WhiteNoise;
use crate::synth::oscillator::SineOscillator;

const TEMPO_BPM: f32 = 120.0;
const STEPS_PER_BEAT: f32 = 4.0;

pub(crate) fn run() -> Result<(), Box<dyn Error>> {
    audio::run_engine("r4", R4Engine::new)
}

struct R4Engine {
    tonal: T4Engine,
    bilateral: SineOscillator,
    noise: WhiteNoise,
    rng: StdRng,
    current_sample: u64,
    beat_samples: u64,
    step_samples: u64,
    next_noise_step_sample: u64,
    noise_envelope: f32,
    noise_decay: f32,
    noise_pan: f32,
}

impl R4Engine {
    fn new(sample_rate: f32) -> Self {
        let beat_samples = (sample_rate * 60.0 / TEMPO_BPM).round() as u64;
        let step_samples = (beat_samples as f32 / STEPS_PER_BEAT).round() as u64;
        let noise_decay = (-1.0 / (sample_rate * 0.09)).exp();

        Self {
            tonal: T4Engine::new(sample_rate),
            bilateral: SineOscillator::new(196.0, sample_rate),
            noise: WhiteNoise::new(),
            rng: StdRng::from_entropy(),
            current_sample: 0,
            beat_samples,
            step_samples,
            next_noise_step_sample: 0,
            noise_envelope: 0.0,
            noise_decay,
            noise_pan: 0.0,
        }
    }

    fn next_bilateral(&mut self) -> (f32, f32) {
        let beat_index = self.current_sample / self.beat_samples;
        let sample_in_beat = self.current_sample % self.beat_samples;
        let beat_position = sample_in_beat as f32 / self.beat_samples as f32;
        let attack = (beat_position / 0.08).min(1.0);
        let release = (1.0 - beat_position).max(0.0).powf(1.55);
        let envelope = attack * release;
        let pan = if beat_index.is_multiple_of(2) {
            -0.82
        } else {
            0.82
        };
        let pulse = self.bilateral.next() * envelope * 0.052;

        StereoPanner::equal_power(pulse, pan)
    }

    fn next_noise(&mut self) -> (f32, f32) {
        while self.current_sample >= self.next_noise_step_sample {
            if self.rng.gen_bool(0.58) {
                self.noise_envelope = self.rng.gen_range(0.018..0.07);
                self.noise_pan = self.rng.gen_range(-0.48..0.48);
            }
            self.next_noise_step_sample += self.step_samples;
        }

        let sample = self.noise.next_filtered(&mut self.rng, 0.32) * self.noise_envelope;
        self.noise_envelope *= self.noise_decay;

        StereoPanner::equal_power(sample, self.noise_pan)
    }
}

impl StereoEngine for R4Engine {
    fn next_stereo(&mut self) -> (f32, f32) {
        let (tonal_left, tonal_right) = self.tonal.next_tonal();
        let (bilateral_left, bilateral_right) = self.next_bilateral();
        let (noise_left, noise_right) = self.next_noise();

        self.current_sample += 1;
        (
            (tonal_left * 0.66 + bilateral_left + noise_left).clamp(-0.95, 0.95),
            (tonal_right * 0.66 + bilateral_right + noise_right).clamp(-0.95, 0.95),
        )
    }
}
