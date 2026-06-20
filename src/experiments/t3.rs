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

pub(crate) fn run() -> Result<(), Box<dyn Error>> {
    audio::run_engine("t3", T3Engine::new)
}

struct T3Engine {
    current_sample: u64,
    sample_rate: f32,
    layers: Vec<PadLayer>,
    next_change_sample: u64,
    chord_index: usize,
    reverb: Freeverb,
    depth_lfo: DriftingLfo,
    width_lfo: DriftingLfo,
    rng: StdRng,
    air: WhiteNoise,
}

impl T3Engine {
    fn new(sample_rate: f32) -> Self {
        Self {
            current_sample: 0,
            sample_rate,
            layers: vec![PadLayer::new(0, sample_rate)],
            next_change_sample: rare_change_samples(sample_rate, &mut StdRng::from_entropy()),
            chord_index: 0,
            reverb: Freeverb::new(sample_rate, 0.93, 0.46, 1.0),
            depth_lfo: DriftingLfo::new(1.0 / 42.0, sample_rate),
            width_lfo: DriftingLfo::new(1.0 / 54.0, sample_rate),
            rng: StdRng::from_entropy(),
            air: WhiteNoise::new(),
        }
    }

    fn dry_signal(&mut self, width: f32) -> (f32, f32) {
        let mut left = 0.0;
        let mut right = 0.0;

        for layer in &mut self.layers {
            let (layer_left, layer_right) = layer.next_stereo(width);
            left += layer_left;
            right += layer_right;
        }
        self.layers.retain(|layer| !layer.is_done());

        (left, right)
    }

    fn update_tonal_layer(&mut self) {
        if self.current_sample < self.next_change_sample {
            return;
        }

        for layer in &mut self.layers {
            layer.release();
        }

        self.chord_index = self.chord_index.wrapping_add(1);
        self.layers
            .push(PadLayer::new(self.chord_index, self.sample_rate));
        self.next_change_sample =
            self.current_sample + rare_change_samples(self.sample_rate, &mut self.rng);
    }
}

impl StereoEngine for T3Engine {
    fn next_stereo(&mut self) -> (f32, f32) {
        self.update_tonal_layer();

        let depth = normalized_lfo(self.depth_lfo.next(&mut self.rng, 1.0 / 68.0, 1.0 / 28.0));
        let width = 0.58
            + normalized_lfo(self.width_lfo.next(&mut self.rng, 1.0 / 86.0, 1.0 / 38.0)) * 0.16;
        let (dry_left, dry_right) = self.dry_signal(width);

        let reverb_send = 0.48 + depth * 0.22;
        let (wet_left, wet_right) = self
            .reverb
            .process(dry_left * reverb_send, dry_right * reverb_send);
        let wet_mix = 0.72 + depth * 0.34;
        let air = self.air.next_filtered(&mut self.rng, 0.0002) * 0.00025;
        let fade_in = (self.current_sample as f32 / (self.sample_rate * 8.0)).min(1.0);

        self.current_sample += 1;
        (
            ((dry_left * 0.58 + wet_left * wet_mix + air) * fade_in).clamp(-0.95, 0.95),
            ((dry_right * 0.58 + wet_right * wet_mix + air) * fade_in).clamp(-0.95, 0.95),
        )
    }
}

struct PadLayer {
    tones: Vec<PadTone>,
}

impl PadLayer {
    fn new(chord_index: usize, sample_rate: f32) -> Self {
        Self {
            tones: pad_tones(chord_index, sample_rate),
        }
    }

    fn next_stereo(&mut self, width: f32) -> (f32, f32) {
        let mut left = 0.0;
        let mut right = 0.0;

        for tone in &mut self.tones {
            let (tone_left, tone_right) = tone.next_stereo(width);
            left += tone_left;
            right += tone_right;
        }

        (left, right)
    }

    fn release(&mut self) {
        for tone in &mut self.tones {
            tone.release();
        }
    }

    fn is_done(&self) -> bool {
        self.tones.iter().all(PadTone::is_done)
    }
}

struct PadTone {
    primary: SineOscillator,
    detuned: SineOscillator,
    octave: SineOscillator,
    envelope: Adsr,
    pan: f32,
    gain: f32,
    detune_mix: f32,
    octave_mix: f32,
}

impl PadTone {
    fn new(frequency_hz: f32, pan: f32, gain: f32, sample_rate: f32) -> Self {
        Self {
            primary: SineOscillator::new(frequency_hz, sample_rate),
            detuned: SineOscillator::new(frequency_hz * 1.003, sample_rate),
            octave: SineOscillator::new(frequency_hz * 2.0, sample_rate),
            envelope: Adsr::new(6.0, 12.0, 0.86, 20.0, sample_rate),
            pan,
            gain,
            detune_mix: 0.42,
            octave_mix: 0.16,
        }
    }

    fn next_stereo(&mut self, width: f32) -> (f32, f32) {
        let sample = self.primary.next()
            + self.detuned.next() * self.detune_mix
            + self.octave.next() * self.octave_mix;
        let shaped = soft_clip(sample * 0.55) * self.envelope.next() * self.gain;

        StereoPanner::equal_power(shaped, self.pan * width)
    }

    fn release(&mut self) {
        self.envelope.note_off();
    }

    fn is_done(&self) -> bool {
        self.envelope.is_done()
    }
}

fn pad_tones(chord_index: usize, sample_rate: f32) -> Vec<PadTone> {
    let chord = pad_chord(chord_index);
    let pans = [-0.52, -0.18, 0.16, 0.46];
    let gains = [0.17, 0.132, 0.126, 0.098];

    chord
        .iter()
        .zip(pans)
        .zip(gains)
        .map(|((frequency_hz, pan), gain)| PadTone::new(*frequency_hz, pan, gain, sample_rate))
        .collect()
}

fn pad_chord(chord_index: usize) -> [f32; 4] {
    const CHORDS: [[f32; 4]; 5] = [
        [110.0, 146.83, 196.0, 261.63],
        [110.0, 164.81, 196.0, 293.66],
        [98.0, 146.83, 220.0, 261.63],
        [123.47, 164.81, 196.0, 293.66],
        [110.0, 146.83, 220.0, 329.63],
    ];

    CHORDS[chord_index % CHORDS.len()]
}

fn rare_change_samples<R: Rng>(sample_rate: f32, rng: &mut R) -> u64 {
    (rng.gen_range(130.0..260.0) * sample_rate).round() as u64
}

fn normalized_lfo(sample: f32) -> f32 {
    (sample * 0.5 + 0.5).clamp(0.0, 1.0)
}

fn soft_clip(sample: f32) -> f32 {
    sample / (1.0 + sample.abs())
}
