use std::error::Error;

use crate::audio::{self, StereoEngine};
use crate::fx::panner::StereoPanner;
use crate::synth::envelope::Adsr;
use crate::synth::oscillator::SineOscillator;

const TEMPO_BPM: f32 = 120.0;
const PULSE_FREQUENCY_HZ: f32 = 196.0;

pub(crate) fn run() -> Result<(), Box<dyn Error>> {
    audio::run_engine("r1", R1Engine::new)
}

struct R1Engine {
    current_sample: u64,
    beat_samples: u64,
    next_pulse_sample: u64,
    next_side_left: bool,
    pulse: Option<BilateralPulse>,
    sample_rate: f32,
}

impl R1Engine {
    fn new(sample_rate: f32) -> Self {
        Self {
            current_sample: 0,
            beat_samples: (sample_rate * 60.0 / TEMPO_BPM).round() as u64,
            next_pulse_sample: 0,
            next_side_left: true,
            pulse: None,
            sample_rate,
        }
    }

    fn trigger_due_pulses(&mut self) {
        while self.current_sample >= self.next_pulse_sample {
            self.pulse = Some(BilateralPulse::new(self.next_side_left, self.sample_rate));
            self.next_side_left = !self.next_side_left;
            self.next_pulse_sample += self.beat_samples;
        }
    }
}

impl StereoEngine for R1Engine {
    fn next_stereo(&mut self) -> (f32, f32) {
        self.trigger_due_pulses();

        let (left, right) = match self.pulse.as_mut() {
            Some(pulse) => pulse.next_stereo(),
            None => (0.0, 0.0),
        };
        if self.pulse.as_ref().is_some_and(BilateralPulse::is_done) {
            self.pulse = None;
        }

        self.current_sample += 1;
        (left.clamp(-0.9, 0.9), right.clamp(-0.9, 0.9))
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
        let envelope = Adsr::new(0.012, 0.045, 0.72, 0.14, sample_rate);
        Self {
            oscillator: SineOscillator::new(PULSE_FREQUENCY_HZ, sample_rate),
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

        let sample = self.oscillator.next() * self.envelope.next() * 0.12;
        self.age_samples += 1;
        StereoPanner::equal_power(sample, self.pan)
    }

    fn is_done(&self) -> bool {
        self.envelope.is_done()
    }
}
