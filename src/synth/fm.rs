use super::envelope::Adsr;
use super::oscillator::SineOscillator;

pub(crate) struct BellVoice {
    carrier: SineOscillator,
    modulator: SineOscillator,
    envelope: Adsr,
    age_samples: u64,
    hold_samples: u64,
    sample_rate: f32,
    velocity: f32,
    base_mod_index: f32,
    released: bool,
}

impl BellVoice {
    pub(crate) fn new(
        frequency_hz: f32,
        hold_seconds: f32,
        velocity: f32,
        sample_rate: f32,
    ) -> Self {
        let envelope = Adsr::new(0.075, 1.25, 0.36, 5.2, sample_rate);
        let hold_samples = envelope.samples_from_seconds(hold_seconds);

        Self {
            carrier: SineOscillator::new(frequency_hz, sample_rate),
            modulator: SineOscillator::new(frequency_hz * 2.0, sample_rate),
            envelope,
            age_samples: 0,
            hold_samples,
            sample_rate,
            velocity,
            base_mod_index: 1.25,
            released: false,
        }
    }

    pub(crate) fn next(&mut self) -> f32 {
        if !self.released && self.age_samples >= self.hold_samples {
            self.envelope.note_off();
            self.released = true;
        }

        let age_seconds = self.age_samples as f32 / self.sample_rate;
        let shimmer_decay = (-age_seconds / 2.8).exp();
        let mod_index = 0.035 + self.base_mod_index * shimmer_decay;
        let mod_sample = self.modulator.next() * mod_index;
        let amp = self.envelope.next();
        self.age_samples += 1;

        self.carrier.next_with_phase_modulation(mod_sample) * amp * self.velocity * 0.2
    }

    pub(crate) fn is_done(&self) -> bool {
        self.envelope.is_done()
    }
}
