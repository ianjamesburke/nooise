use super::*;

// ============================================================
// Perc engine (16th-note white noise hits)
// ============================================================

pub(crate) struct PercEngine {
    pub(crate) sample_rate: f32,
    pub(crate) trigger: GridTrigger,
    pub(crate) hits: Vec<NoiseHit>,
    pub(crate) noise: WhiteNoise,
    pub(crate) rng: StdRng,
    pub(crate) telemetry: Arc<FluidTelemetry>,
}

impl PercEngine {
    pub(crate) fn new(sample_rate: f32, telemetry: Arc<FluidTelemetry>) -> Self {
        Self {
            sample_rate,
            trigger: GridTrigger::new(),
            hits: Vec::with_capacity(8),
            noise: WhiteNoise::new(),
            rng: StdRng::from_entropy(),
            telemetry,
        }
    }

    pub(crate) fn next(&mut self, c: &PercControls, timing: TimingContext) -> f32 {
        if c.interval_beats >= 4.25 {
            // Continuous mode: bypass GridTrigger/NoiseHit entirely so there is
            // no trigger-rate amplitude ripple to disguise (see GOTCHAS.md).
            // Reuse the same exponential smoothing transform as discrete hits so
            // Filter has a comparably audible range in both modes.
            let smoothing = 10_f32.powf(c.filter * 4.0 - 4.0);
            return self.noise.next_filtered(&mut self.rng, smoothing) * c.level * 0.4;
        }

        if self.trigger.pop(timing, c.interval_beats, c.offset_beats) {
            self.telemetry.perc_pulse.fetch_add(1, Ordering::Relaxed);
            let smoothing = 10_f32.powf(c.filter * 4.0 - 4.0);
            self.hits.push(NoiseHit::new(
                c.level,
                c.decay_ms,
                smoothing,
                self.sample_rate,
            ));
        }

        let mut out = 0.0f32;
        for h in &mut self.hits {
            out += h.next(&mut self.rng);
        }
        self.hits.retain(|h| !h.is_done());
        out
    }
}

pub(crate) struct NoiseHit {
    pub(crate) noise: WhiteNoise,
    pub(crate) samples_remaining: u64,
    pub(crate) total_samples: u64,
    pub(crate) level: f32,
    pub(crate) filter: f32,
}

impl NoiseHit {
    pub(crate) fn new(level: f32, decay_ms: f32, filter: f32, sample_rate: f32) -> Self {
        let total = (decay_ms * 0.001 * sample_rate).round() as u64;
        Self {
            noise: WhiteNoise::new(),
            samples_remaining: total,
            total_samples: total,
            level,
            filter,
        }
    }
    pub(crate) fn next<R: Rng>(&mut self, rng: &mut R) -> f32 {
        if self.samples_remaining == 0 {
            return 0.0;
        }
        let gain = self.samples_remaining as f32 / self.total_samples as f32;
        self.samples_remaining -= 1;
        self.noise.next_filtered(rng, self.filter) * gain * self.level * 0.4
    }
    pub(crate) fn is_done(&self) -> bool {
        self.samples_remaining == 0
    }
}
