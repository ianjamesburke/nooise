//! Per-voice mono filters. Module slots use the stereo biquad in
//! `fx::filter`; a voice that sweeps its own cutoff every sample needs a
//! structure that stays stable under that modulation.

use std::f32::consts::PI;

/// Topology-preserving-transform state-variable lowpass (Zavalishin, "The Art
/// of VA Filter Design", ch. 4; Simper, "Linear Trapezoidal Integrated SVF",
/// Cytomic technical paper, 2013). Its trapezoidal integrators keep the
/// response stable and free of zipper noise while cutoff moves per sample.
#[derive(Default)]
pub(crate) struct SvfLowpass {
    ic1: f32,
    ic2: f32,
}

impl SvfLowpass {
    /// One sample. `resonance` is 1/Q: 2 is no peak, small values ring.
    pub(crate) fn next(
        &mut self,
        input: f32,
        cutoff_hz: f32,
        resonance: f32,
        sample_rate: f32,
    ) -> f32 {
        let g = (PI * cutoff_hz.clamp(10.0, sample_rate * 0.45) / sample_rate).tan();
        let a1 = 1.0 / (1.0 + g * (g + resonance));
        let a2 = g * a1;
        let a3 = g * a2;
        let v3 = input - self.ic2;
        let v1 = a1 * self.ic1 + a2 * v3;
        let v2 = self.ic2 + a2 * self.ic1 + a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        v2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gain_at(hz: f32, cutoff: f32) -> f32 {
        let sample_rate = 48_000.0;
        let mut filter = SvfLowpass::default();
        let mut peak = 0.0f32;
        for n in 0..48_000 {
            let x = (std::f32::consts::TAU * hz * n as f32 / sample_rate).sin();
            let y = filter.next(x, cutoff, 2.0_f32.sqrt(), sample_rate);
            if n > 24_000 {
                peak = peak.max(y.abs());
            }
        }
        peak
    }

    #[test]
    fn passes_below_cutoff_and_cuts_above() {
        assert!((gain_at(100.0, 2_000.0) - 1.0).abs() < 0.02);
        assert!((gain_at(2_000.0, 2_000.0) - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.03);
        assert!(gain_at(16_000.0, 2_000.0) < 0.03);
    }
}
