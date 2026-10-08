//! Basic waveform oscillators shared by the pitched voices.

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

    /// One sample with the frequency scaled by `ratio` for this step only,
    /// for a caller-owned pitch envelope. `next` stays the unscaled path so
    /// voices without one render exactly as before.
    pub(crate) fn next_scaled(&mut self, ratio: f32) -> f32 {
        let sample = self.phase.sin();
        self.phase += self.phase_increment * ratio;
        if self.phase >= TAU {
            self.phase -= TAU;
        }
        sample
    }

    fn advance(&mut self) {
        self.phase += self.phase_increment;
        if self.phase >= TAU {
            self.phase -= TAU;
        }
    }
}

/// Band-limited sawtooth: a naive ramp with a two-sample polynomial (polyBLEP)
/// correction at each wrap, which removes most of the aliasing a hard reset
/// folds below Nyquist (Välimäki, Pekonen, Nam, "Perceptually informed
/// synthesis of bandlimited classical waveforms using integrated polynomial
/// interpolation", JASA 131(1), 2012).
pub(crate) struct PolyBlepSaw {
    /// 0..1 position in the cycle.
    phase: f32,
    sample_rate: f32,
}

impl PolyBlepSaw {
    pub(crate) fn new(sample_rate: f32) -> Self {
        Self {
            phase: 0.0,
            sample_rate,
        }
    }

    /// One sample in -1..1 at `frequency_hz`, read fresh every call so a
    /// caller can sweep it.
    pub(crate) fn next(&mut self, frequency_hz: f32) -> f32 {
        let increment = (frequency_hz / self.sample_rate).clamp(0.0, 0.5);
        let mut sample = 2.0 * self.phase - 1.0;
        sample -= poly_blep(self.phase, increment);
        self.phase += increment;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        sample
    }
}

/// The polyBLEP residual for a unit downward step at phase 0, spread over the
/// sample either side of it.
fn poly_blep(phase: f32, increment: f32) -> f32 {
    if increment <= 0.0 {
        0.0
    } else if phase < increment {
        let t = phase / increment;
        t + t - t * t - 1.0
    } else if phase > 1.0 - increment {
        let t = (phase - 1.0) / increment;
        t * t + t + t + 1.0
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_ratio_scaling_matches_the_plain_sine() {
        let mut plain = SineOscillator::new(110.0, 48_000.0);
        let mut scaled = SineOscillator::new(110.0, 48_000.0);
        for _ in 0..1_000 {
            assert_eq!(plain.next(), scaled.next_scaled(1.0));
        }
    }

    #[test]
    fn the_band_limited_saw_ramps_and_stays_in_range() {
        let mut saw = PolyBlepSaw::new(48_000.0);
        let samples: Vec<f32> = (0..4_800).map(|_| saw.next(100.0)).collect();
        assert!(samples.iter().all(|s| s.abs() <= 1.0 + 1e-6));
        let mean = samples.iter().sum::<f32>() / samples.len() as f32;
        assert!(mean.abs() < 0.01, "{mean}");
        // Rising between resets: most consecutive steps go up.
        let rising = samples.windows(2).filter(|pair| pair[1] > pair[0]).count();
        assert!(rising > samples.len() * 9 / 10);
    }
}
