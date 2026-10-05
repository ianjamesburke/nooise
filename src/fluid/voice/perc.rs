//! The Perc voice: white-noise hits on a grid.

use super::*;

/// Keep the noise layer's useful Level range above the first dial step.
/// Built-in songs compensate their Level and its LFO depths by three.
const OUTPUT_TRIM: f32 = 0.5 / 3.0;

pub(crate) struct PercEngine {
    pub(crate) sample_rate: f32,
    pub(crate) trigger: GridTrigger,
    pub(crate) hits: Vec<NoiseHit>,
    pub(crate) noise: WhiteNoise,
    pub(crate) rng: StdRng,
    pub(crate) telemetry: Arc<FluidTelemetry>,
}

impl PercEngine {
    #[cfg(test)]
    pub(crate) fn new(sample_rate: f32) -> Self {
        Self::with_telemetry(sample_rate, Arc::new(FluidTelemetry::default()))
    }

    pub(crate) fn with_telemetry(sample_rate: f32, telemetry: Arc<FluidTelemetry>) -> Self {
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
            return self.noise.next(&mut self.rng) * c.level * OUTPUT_TRIM;
        }

        if self
            .trigger
            .pop_swung(timing, c.interval_beats, c.offset_beats, c.swing)
        {
            self.hits.push(NoiseHit::new(
                c.level,
                c.attack_ms,
                c.decay_ms,
                self.sample_rate,
            ));
            self.telemetry
                .publish_hit(MusicalHit::Perc, c.level, NO_PITCH_CLASS);
        }

        let rng = &mut self.rng;
        mix_and_retain_mono(&mut self.hits, |h| h.next(rng), NoiseHit::is_done)
    }
}

pub(crate) struct NoiseHit {
    pub(crate) noise: WhiteNoise,
    pub(crate) attack_samples_remaining: u64,
    pub(crate) attack_total_samples: u64,
    pub(crate) samples_remaining: u64,
    pub(crate) total_samples: u64,
    pub(crate) level: f32,
}

impl NoiseHit {
    pub(crate) fn new(level: f32, attack_ms: f32, decay_ms: f32, sample_rate: f32) -> Self {
        let attack_total = (attack_ms * 0.001 * sample_rate).round() as u64;
        let total = (decay_ms * 0.001 * sample_rate).round() as u64;
        Self {
            noise: WhiteNoise::new(),
            attack_samples_remaining: attack_total,
            attack_total_samples: attack_total,
            samples_remaining: total,
            total_samples: total,
            level,
        }
    }
    pub(crate) fn next<R: Rng>(&mut self, rng: &mut R) -> f32 {
        if self.attack_samples_remaining > 0 {
            // The first hit sample is already audible, and the last reaches
            // peak before the existing decay begins.
            let gain = (self.attack_total_samples - self.attack_samples_remaining + 1) as f32
                / self.attack_total_samples as f32;
            self.attack_samples_remaining -= 1;
            return self.noise.next(rng) * gain * self.level * OUTPUT_TRIM;
        }
        if self.samples_remaining == 0 {
            return 0.0;
        }
        let gain = self.samples_remaining as f32 / self.total_samples as f32;
        self.samples_remaining -= 1;
        self.noise.next(rng) * gain * self.level * OUTPUT_TRIM
    }
    pub(crate) fn is_done(&self) -> bool {
        self.attack_samples_remaining == 0 && self.samples_remaining == 0
    }
}
