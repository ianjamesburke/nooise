use std::error::Error;

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use crate::audio::{self, StereoEngine};
use crate::fx::lfo::DriftingLfo;
use crate::fx::panner::StereoPanner;
use crate::fx::reverb::Freeverb;
use crate::synth::envelope::Adsr;
use crate::synth::noise::WhiteNoise;
use crate::synth::oscillator::SineOscillator;

const TEMPO_BPM: f32 = 120.0;
const STEPS_PER_BEAT: f32 = 4.0;
const BILATERAL_FREQUENCY_HZ: f32 = 196.0;
const PATTERN: [f32; 16] = [
    0.62, 0.0, 0.2, 0.08, 0.34, 0.14, 0.0, 0.24, 0.54, 0.08, 0.28, 0.0, 0.38, 0.16, 0.0, 0.22,
];

pub(crate) fn run() -> Result<(), Box<dyn Error>> {
    audio::run_engine("r3", R3Engine::new)
}

struct R3Engine {
    current_sample: u64,
    beat_samples: u64,
    step_samples: u64,
    next_pulse_sample: u64,
    next_step_sample: u64,
    next_side_left: bool,
    step_index: usize,
    sample_rate: f32,
    pulse: Option<BilateralPulse>,
    hits: Vec<NoiseHit>,
    rng: StdRng,
    pan_lfo: DriftingLfo,
    reverb: Freeverb,
}

impl R3Engine {
    fn new(sample_rate: f32) -> Self {
        Self {
            current_sample: 0,
            beat_samples: (sample_rate * 60.0 / TEMPO_BPM).round() as u64,
            step_samples: (sample_rate * 60.0 / TEMPO_BPM / STEPS_PER_BEAT).round() as u64,
            next_pulse_sample: 0,
            next_step_sample: 0,
            next_side_left: true,
            step_index: 0,
            sample_rate,
            pulse: None,
            hits: Vec::with_capacity(12),
            rng: StdRng::from_entropy(),
            pan_lfo: DriftingLfo::new(1.0 / 13.0, sample_rate),
            reverb: Freeverb::new(sample_rate, 0.5, 0.62, 0.22),
        }
    }

    fn trigger_due_pulses(&mut self) {
        while self.current_sample >= self.next_pulse_sample {
            self.pulse = Some(BilateralPulse::new(self.next_side_left, self.sample_rate));
            self.next_side_left = !self.next_side_left;
            self.next_pulse_sample += self.beat_samples;
        }
    }

    fn trigger_due_steps(&mut self) {
        while self.current_sample >= self.next_step_sample {
            let accent = PATTERN[self.step_index % PATTERN.len()];
            if accent > 0.0 && self.rng.gen_bool(hit_probability(accent) as f64) {
                let velocity = (accent + self.rng.gen_range(-0.1..0.14)).clamp(0.06, 0.68);
                let decay_seconds = self.rng.gen_range(0.03..0.14) * (0.78 + accent);
                let pan = self.rng.gen_range(-0.48..0.48);
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

impl StereoEngine for R3Engine {
    fn next_stereo(&mut self) -> (f32, f32) {
        self.trigger_due_pulses();
        self.trigger_due_steps();

        let (pulse_left, pulse_right) = match self.pulse.as_mut() {
            Some(pulse) => pulse.next_stereo(),
            None => (0.0, 0.0),
        };
        if self.pulse.as_ref().is_some_and(BilateralPulse::is_done) {
            self.pulse = None;
        }

        let (noise_left, noise_right) = self.next_noise_sample();
        let drift = self.pan_lfo.next(&mut self.rng, 1.0 / 20.0, 1.0 / 8.0) * 0.06;
        let dry_left = pulse_left + noise_left * (1.0 - drift);
        let dry_right = pulse_right + noise_right * (1.0 + drift);
        let (wet_left, wet_right) = self.reverb.process(noise_left, noise_right);

        self.current_sample += 1;
        (
            (dry_left + wet_left).clamp(-0.88, 0.88),
            (dry_right + wet_right).clamp(-0.88, 0.88),
        )
    }
}

struct BilateralPulse {
    oscillator: SineOscillator,
    envelope: Adsr,
    age_samples: u64,
    hold_samples: u64,
    pan: f32,
    released: bool,
}

impl BilateralPulse {
    fn new(left: bool, sample_rate: f32) -> Self {
        let envelope = Adsr::new(0.012, 0.045, 0.68, 0.14, sample_rate);
        Self {
            oscillator: SineOscillator::new(BILATERAL_FREQUENCY_HZ, sample_rate),
            hold_samples: envelope.samples_from_seconds(0.32),
            envelope,
            age_samples: 0,
            pan: if left { -0.96 } else { 0.96 },
            released: false,
        }
    }

    fn next_stereo(&mut self) -> (f32, f32) {
        if !self.released && self.age_samples >= self.hold_samples {
            self.envelope.note_off();
            self.released = true;
        }

        let sample = self.oscillator.next() * self.envelope.next() * 0.085;
        self.age_samples += 1;
        StereoPanner::equal_power(sample, self.pan)
    }

    fn is_done(&self) -> bool {
        self.envelope.is_done()
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
            length_samples: (decay_seconds * sample_rate * 3.2).round() as u64,
            attack_samples: (0.004 * sample_rate).round() as u64,
            decay_samples: (decay_seconds * sample_rate).max(1.0),
            amplitude,
            pan,
            smoothing: 0.05 + amplitude * 0.08,
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
            * 0.16;

        self.age_samples += 1;
        StereoPanner::equal_power(sample, self.pan)
    }

    fn is_done(&self) -> bool {
        self.age_samples >= self.length_samples
    }
}

fn hit_probability(accent: f32) -> f32 {
    (0.22 + accent * 0.66).clamp(0.0, 1.0)
}
