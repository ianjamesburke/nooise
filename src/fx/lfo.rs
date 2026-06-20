use rand::Rng;

pub(crate) struct DriftingLfo {
    phase: f32,
    rate_hz: f32,
    target_rate_hz: f32,
    sample_rate: f32,
    samples_until_target: u64,
}

impl DriftingLfo {
    pub(crate) fn new(rate_hz: f32, sample_rate: f32) -> Self {
        Self {
            phase: 0.0,
            rate_hz,
            target_rate_hz: rate_hz,
            sample_rate,
            samples_until_target: sample_rate as u64 * 8,
        }
    }

    pub(crate) fn next<R: Rng>(&mut self, rng: &mut R, min_rate_hz: f32, max_rate_hz: f32) -> f32 {
        if self.samples_until_target == 0 {
            self.target_rate_hz = rng.gen_range(min_rate_hz..max_rate_hz);
            self.samples_until_target = rng.gen_range(4..14) * self.sample_rate as u64;
        }

        self.rate_hz += (self.target_rate_hz - self.rate_hz) * 0.000_01;
        self.phase += self.rate_hz / self.sample_rate;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        self.samples_until_target -= 1;

        (self.phase * std::f32::consts::TAU).sin()
    }
}
