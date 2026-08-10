//! Shared stereo compressor primitive for track and master effect chains.

pub(crate) struct StereoCompressor {
    envelope: f32,
}

impl StereoCompressor {
    pub(crate) fn new(envelope: f32) -> Self {
        Self {
            envelope: envelope.max(0.0),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn process(
        &mut self,
        sample: (f32, f32),
        sample_rate: f32,
        threshold_db: f32,
        ratio: f32,
        release_ms: f32,
        makeup_db: f32,
        amount: f32,
    ) -> (f32, f32) {
        let peak = sample.0.abs().max(sample.1.abs());
        let coeff = if peak > self.envelope {
            (-1.0 / (0.001 * sample_rate)).exp()
        } else {
            (-1.0 / (release_ms.max(1.0) * 0.001 * sample_rate)).exp()
        };
        self.envelope = peak + coeff * (self.envelope - peak);

        let threshold = 10_f32.powf(threshold_db / 20.0);
        let ratio = ratio.max(1.0);
        let gain_reduction = if self.envelope > threshold {
            (threshold / self.envelope) * (self.envelope / threshold).powf(1.0 / ratio)
        } else {
            1.0
        };
        let makeup = 10_f32.powf(makeup_db / 20.0);
        let amount = amount.clamp(0.0, 1.0);
        let wet = (
            sample.0 * gain_reduction * makeup,
            sample.1 * gain_reduction * makeup,
        );
        (
            sample.0 + (wet.0 - sample.0) * amount,
            sample.1 + (wet.1 - sample.1) * amount,
        )
    }
}
