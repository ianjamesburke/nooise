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

    pub(crate) fn process(&mut self, sample: (f32, f32), params: CompressorParams) -> (f32, f32) {
        let peak = sample.0.abs().max(sample.1.abs());
        let coeff = if peak > self.envelope {
            (-1.0 / (0.001 * params.sample_rate)).exp()
        } else {
            (-1.0 / (params.release_ms.max(1.0) * 0.001 * params.sample_rate)).exp()
        };
        self.envelope = peak + coeff * (self.envelope - peak);

        let threshold = 10_f32.powf(params.threshold_db / 20.0);
        let ratio = params.ratio.max(1.0);
        let gain_reduction = if self.envelope > threshold {
            (threshold / self.envelope) * (self.envelope / threshold).powf(1.0 / ratio)
        } else {
            1.0
        };
        let makeup = 10_f32.powf(params.makeup_db / 20.0);
        let amount = params.amount.clamp(0.0, 1.0);
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

/// Grouped to keep `process` under clippy's argument-count lint.
pub(crate) struct CompressorParams {
    pub(crate) sample_rate: f32,
    pub(crate) threshold_db: f32,
    pub(crate) ratio: f32,
    pub(crate) release_ms: f32,
    pub(crate) makeup_db: f32,
    pub(crate) amount: f32,
}
