//! Stateful stereo filter used by Filter module slots.

use super::crossfade::mix_stereo;

#[derive(Clone, Copy)]
pub(crate) enum FilterType {
    Low,
    High,
    Band,
}

impl FilterType {
    pub(crate) fn from_value(value: f32) -> Self {
        match value.round() as i32 {
            1 => Self::High,
            2 => Self::Band,
            _ => Self::Low,
        }
    }
}

#[derive(Default)]
struct StateVariableFilter {
    z1: f32,
    z2: f32,
}

impl StateVariableFilter {
    fn process(&mut self, input: f32, params: FilterParams) -> f32 {
        let cutoff = params.cutoff_hz.clamp(20.0, params.sample_rate * 0.45);
        let omega = 2.0 * std::f32::consts::PI * cutoff / params.sample_rate;
        let (sin, cos) = omega.sin_cos();
        let q = 0.5 + params.resonance.clamp(0.0, 1.0) * 19.5;
        let alpha = sin / (2.0 * q);
        let (b0, b1, b2) = match params.filter_type {
            FilterType::Low => ((1.0 - cos) * 0.5, 1.0 - cos, (1.0 - cos) * 0.5),
            FilterType::High => ((1.0 + cos) * 0.5, -(1.0 + cos), (1.0 + cos) * 0.5),
            FilterType::Band => (alpha, 0.0, -alpha),
        };
        let a0 = 1.0 + alpha;
        let output = b0 / a0 * input + self.z1;
        self.z1 = b1 / a0 * input - (-2.0 * cos / a0) * output + self.z2;
        self.z2 = b2 / a0 * input - ((1.0 - alpha) / a0) * output;
        output
    }
}

#[derive(Default)]
pub(crate) struct StereoFilter {
    left: StateVariableFilter,
    right: StateVariableFilter,
}

impl StereoFilter {
    pub(crate) fn process(&mut self, sample: (f32, f32), params: FilterParams) -> (f32, f32) {
        let amount = params.amount.clamp(0.0, 1.0);
        if amount <= f32::EPSILON {
            return sample;
        }
        let wet = (
            self.left.process(sample.0, params),
            self.right.process(sample.1, params),
        );
        mix_stereo(sample, wet, amount)
    }
}

/// Per-call filter controls. State stays in [`StereoFilter`] so slot identity
/// owns filter history while song codes carry only audible settings.
#[derive(Clone, Copy)]
pub(crate) struct FilterParams {
    pub(crate) sample_rate: f32,
    pub(crate) cutoff_hz: f32,
    pub(crate) resonance: f32,
    pub(crate) filter_type: FilterType,
    pub(crate) amount: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_amount_is_an_exact_stereo_bypass() {
        let mut filter = StereoFilter::default();
        assert_eq!(
            filter.process(
                (0.4, -0.2),
                FilterParams {
                    sample_rate: 48_000.0,
                    cutoff_hz: 800.0,
                    resonance: 0.0,
                    filter_type: FilterType::Low,
                    amount: 0.0,
                },
            ),
            (0.4, -0.2)
        );
    }
}
