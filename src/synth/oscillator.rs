use std::f32::consts::TAU;

pub(crate) struct SineOscillator {
    phase: f32,
    phase_increment: f32,
}

impl SineOscillator {
    pub(crate) fn new(frequency_hz: f32, sample_rate: f32) -> Self {
        Self {
            phase: 0.0,
            phase_increment: TAU * frequency_hz / sample_rate,
        }
    }

    pub(crate) fn next(&mut self) -> f32 {
        let sample = self.phase.sin();
        self.advance();
        sample
    }

    fn advance(&mut self) {
        self.phase += self.phase_increment;
        if self.phase >= TAU {
            self.phase -= TAU;
        }
    }
}
