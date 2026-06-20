use std::error::Error;

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use crate::audio::{self, StereoEngine};
use crate::fx::lfo::DriftingLfo;
use crate::fx::panner::StereoPanner;
use crate::fx::reverb::Freeverb;
use crate::synth::fm::BellVoice;
use crate::synth::noise::WhiteNoise;

const TEMPO_BPM: f32 = 108.0;
const CYCLE_BEATS: f32 = 6.0;

pub(crate) fn run() -> Result<(), Box<dyn Error>> {
    audio::run_engine("t2", T2Engine::new)
}

struct T2Engine {
    sample_rate: f32,
    current_sample: u64,
    cycle_samples: u64,
    lanes: [PolyrhythmLane; 2],
    voices: Vec<PlacedVoice>,
    reverb: Freeverb,
    pan_lfo: DriftingLfo,
    rng: StdRng,
    air: WhiteNoise,
}

impl T2Engine {
    fn new(sample_rate: f32) -> Self {
        let beat_samples = sample_rate * 60.0 / TEMPO_BPM;

        Self {
            sample_rate,
            current_sample: 0,
            cycle_samples: (beat_samples * CYCLE_BEATS).round() as u64,
            lanes: [
                PolyrhythmLane::new(2.0, [-3, 2, 4], -0.42, 0.56, beat_samples),
                PolyrhythmLane::new(3.0, [0, 5, 7], 0.38, 0.48, beat_samples),
            ],
            voices: Vec::with_capacity(32),
            reverb: Freeverb::new(sample_rate, 0.82, 0.32, 0.76),
            pan_lfo: DriftingLfo::new(1.0 / 20.0, sample_rate),
            rng: StdRng::from_entropy(),
            air: WhiteNoise::new(),
        }
    }

    fn trigger_due_notes(&mut self) {
        for lane in &mut self.lanes {
            while self.current_sample >= lane.next_sample {
                let frequency = frequency_for_degree(lane.next_degree(), lane.drift_cents);
                let hold_seconds = lane.step_seconds() * 1.42;
                self.voices.push(PlacedVoice {
                    voice: BellVoice::new(frequency, hold_seconds, lane.velocity, self.sample_rate),
                    pan: lane.pan,
                });

                lane.advance(&mut self.rng);
            }
        }
    }

    fn next_voice_sample(&mut self, pan_offset: f32) -> (f32, f32) {
        let mut left = 0.0;
        let mut right = 0.0;

        for placed in &mut self.voices {
            let (voice_left, voice_right) =
                StereoPanner::equal_power(placed.voice.next(), placed.pan + pan_offset);
            left += voice_left;
            right += voice_right;
        }

        self.voices.retain(|placed| !placed.voice.is_done());
        (left, right)
    }
}

impl StereoEngine for T2Engine {
    fn next_stereo(&mut self) -> (f32, f32) {
        self.trigger_due_notes();

        let cycle_phase = self.current_sample as f32 / self.cycle_samples as f32;
        let phase_sway = (cycle_phase * std::f32::consts::TAU).sin() * 0.06;
        let drift_sway = self.pan_lfo.next(&mut self.rng, 1.0 / 32.0, 1.0 / 14.0) * 0.08;
        let (dry_left, dry_right) = self.next_voice_sample(phase_sway + drift_sway);
        let (wet_left, wet_right) = self.reverb.process(dry_left, dry_right);
        let air = self.air.next_filtered(&mut self.rng, 0.0005) * 0.00045;

        self.current_sample += 1;
        (
            (dry_left * 0.64 + wet_left + air).clamp(-0.95, 0.95),
            (dry_right * 0.64 + wet_right + air).clamp(-0.95, 0.95),
        )
    }
}

struct PolyrhythmLane {
    step_samples: u64,
    step_beats: f32,
    degrees: [i32; 3],
    note_index: usize,
    next_sample: u64,
    drift_cents: f32,
    pan: f32,
    velocity: f32,
}

impl PolyrhythmLane {
    fn new(step_beats: f32, degrees: [i32; 3], pan: f32, velocity: f32, beat_samples: f32) -> Self {
        Self {
            step_samples: (step_beats * beat_samples).round() as u64,
            step_beats,
            degrees,
            note_index: 0,
            next_sample: 0,
            drift_cents: 0.0,
            pan,
            velocity,
        }
    }

    fn next_degree(&self) -> i32 {
        self.degrees[self.note_index]
    }

    fn step_seconds(&self) -> f32 {
        self.step_beats * 60.0 / TEMPO_BPM
    }

    fn advance<R: Rng>(&mut self, rng: &mut R) {
        self.next_sample += self.step_samples;
        self.note_index = (self.note_index + 1) % self.degrees.len();
        self.drift_cents = (self.drift_cents + rng.gen_range(-1.8..1.8)).clamp(-9.0, 9.0);
    }
}

struct PlacedVoice {
    voice: BellVoice,
    pan: f32,
}

fn frequency_for_degree(degree: i32, drift_cents: f32) -> f32 {
    let scale = [0.0_f32, 3.0, 5.0, 7.0, 10.0];
    let octave = degree.div_euclid(scale.len() as i32);
    let scale_degree = degree.rem_euclid(scale.len() as i32) as usize;
    let semitones = octave as f32 * 12.0 + scale[scale_degree] + drift_cents / 100.0;

    220.0 * 2.0_f32.powf(semitones / 12.0)
}
