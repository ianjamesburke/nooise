//! The Perc voice: white-noise hits on a grid.

use super::*;

/// Keep the noise layer's useful Level range above the first dial step.
/// Built-in songs compensate their Level and its LFO depths by three.
const OUTPUT_TRIM: f32 = 0.5 / 3.0;

pub(crate) fn perc_live_step_count(step_count: f32) -> usize {
    (step_count.round() as i64).clamp(1, PERC_STEP_COUNT as i64) as usize
}

pub(crate) fn perc_pattern_label(value: f32) -> &'static str {
    PERC_PATTERNS[wrapped_index(value, PERC_PATTERNS.len())]
}

pub(crate) fn perc_step_label(value: f32) -> &'static str {
    match (value.round() as i32).clamp(-1, 1) {
        v if v < 0 => "Tie",
        0 => "Rest",
        _ => "Hit",
    }
}

pub(crate) fn perc_authored_steps(c: &PercControls) -> [f32; PERC_STEP_COUNT] {
    let index = (c.pattern.round() as isize).clamp(0, CUSTOM_PERC_PATTERN_INDEX as isize) as usize;
    if index < PERC_PATTERN_PRESET_COUNT {
        PERC_PATTERN_PRESETS[index].1
    } else {
        c.steps
    }
}

pub(crate) struct PercEngine {
    pub(crate) sample_rate: f32,
    pub(crate) trigger: GridTrigger,
    pub(crate) hits: Vec<NoiseHit>,
    pub(crate) noise: WhiteNoise,
    pub(crate) rng: StdRng,
    pub(crate) telemetry: Arc<FluidTelemetry>,
    pub(crate) last_cycle: Option<u64>,
    pub(crate) active_steps_source: [f32; PERC_STEP_COUNT],
    pub(crate) evolved_steps: [f32; PERC_STEP_COUNT],
    pub(crate) evolution_seed: u64,
    pub(crate) evolution_count: u64,
    pub(crate) session_state: Option<LiveSession>,
}

impl PercEngine {
    #[cfg(test)]
    pub(crate) fn new(sample_rate: f32) -> Self {
        Self::with_telemetry(sample_rate, Arc::new(FluidTelemetry::default()))
    }

    #[cfg(test)]
    pub(crate) fn with_telemetry(sample_rate: f32, telemetry: Arc<FluidTelemetry>) -> Self {
        Self::new_with_live_state(sample_rate, None, telemetry)
    }

    pub(crate) fn new_with_live_state(
        sample_rate: f32,
        session_state: Option<LiveSession>,
        telemetry: Arc<FluidTelemetry>,
    ) -> Self {
        let mut rng = StdRng::from_entropy();
        let evolution_seed = rng.r#gen();
        Self {
            sample_rate,
            trigger: GridTrigger::new(),
            hits: Vec::with_capacity(8),
            noise: WhiteNoise::new(),
            rng,
            telemetry,
            last_cycle: None,
            active_steps_source: DEFAULT_PERC_STEPS,
            evolved_steps: DEFAULT_PERC_STEPS,
            evolution_seed,
            evolution_count: 0,
            session_state,
        }
    }

    fn evolve_steps(&mut self, live_len: usize, evolve_rate: f32, cycle: u64) {
        let live_len = live_len.clamp(1, PERC_STEP_COUNT);
        let seed = self
            .evolution_seed
            .wrapping_add(
                self.evolution_count
                    .wrapping_add(1)
                    .wrapping_mul(0x9E37_79B9_7F4A_7C15),
            )
            .wrapping_add(cycle.rotate_left(17));
        let mutations = evolve_mutation_count(evolve_rate, 3, seed);
        if mutations == 0 {
            return;
        }
        let mut evo_rng = StdRng::seed_from_u64(seed);
        for _ in 0..mutations {
            let idx = evo_rng.gen_range(0..live_len);
            let current = (self.evolved_steps[idx].round() as i32).clamp(-1, 1);
            let next = if current <= 0 {
                1
            } else if idx > 0 && evo_rng.gen_bool(0.20) {
                -1
            } else {
                0
            };
            self.evolved_steps[idx] = next as f32;
        }
        if self.evolved_steps[0] < 0.0 {
            self.evolved_steps[0] = 1.0;
        }
        self.evolution_count = self.evolution_count.saturating_add(1);
        if let Some(session_state) = &self.session_state {
            let steps = self.evolved_steps;
            session_state.update(|snapshot| {
                snapshot.controls.perc.steps = steps;
                snapshot.controls.perc.pattern = CUSTOM_PERC_PATTERN_INDEX as f32;
            });
        }
    }

    pub(crate) fn next(&mut self, c: &PercControls, timing: TimingContext) -> f32 {
        let pattern_index =
            (c.pattern.round() as isize).clamp(0, CUSTOM_PERC_PATTERN_INDEX as isize) as usize;
        if pattern_index == CONTINUOUS_PERC_PATTERN_INDEX {
            // Continuous mode: bypass GridTrigger/NoiseHit entirely so there is
            // no trigger-rate amplitude ripple to disguise (see GOTCHAS.md).
            return self.noise.next(&mut self.rng) * c.level * OUTPUT_TRIM;
        }

        if self
            .trigger
            .pop_swung(timing, c.interval_beats, c.offset_beats, c.swing)
        {
            let authored = perc_authored_steps(c);
            let is_custom = pattern_index >= CUSTOM_PERC_PATTERN_INDEX;
            if is_custom && c.steps == self.evolved_steps {
                self.active_steps_source = c.steps;
            } else if authored != self.active_steps_source {
                self.active_steps_source = authored;
                self.evolved_steps = authored;
                self.last_cycle = None;
            }
            let count = perc_live_step_count(c.step_count);
            let interval = c.interval_beats.max(0.125);
            let cycle_beats = (interval * count as f32).max(0.125);
            let cycle = (timing.beat.max(0.0) / f64::from(cycle_beats)).floor() as u64;
            if self.last_cycle.is_some_and(|last| cycle > last) && c.evolve_rate > 0.0 {
                self.evolve_steps(count, c.evolve_rate, cycle);
            }
            self.last_cycle = Some(cycle);

            let step = lane_step_at(timing.beat, interval, c.offset_beats, count);
            let step_val = (self.evolved_steps[step].round() as i32).clamp(-1, 1);
            if step_val > 0 {
                let ties = consecutive_ties_after(&self.evolved_steps, step, count);
                let hold_samples = if ties > 0 {
                    ((f64::from(ties as f32 * interval) * 60.0 / timing.bpm.max(1.0))
                        * f64::from(self.sample_rate))
                    .round() as u64
                } else {
                    0
                };
                self.hits.push(NoiseHit::new_with_hold(
                    c.level,
                    c.attack_ms,
                    c.decay_ms,
                    hold_samples,
                    self.sample_rate,
                ));
                self.telemetry
                    .publish_hit(MusicalHit::Perc, c.level, NO_PITCH_CLASS);
            }
        }

        let rng = &mut self.rng;
        mix_and_retain_mono(&mut self.hits, |h| h.next(rng), NoiseHit::is_done)
    }
}

pub(crate) struct NoiseHit {
    pub(crate) noise: WhiteNoise,
    pub(crate) attack_samples_remaining: u64,
    pub(crate) attack_total_samples: u64,
    pub(crate) hold_samples_remaining: u64,
    pub(crate) samples_remaining: u64,
    pub(crate) total_samples: u64,
    pub(crate) level: f32,
}

impl NoiseHit {
    #[cfg(test)]
    pub(crate) fn new(level: f32, attack_ms: f32, decay_ms: f32, sample_rate: f32) -> Self {
        Self::new_with_hold(level, attack_ms, decay_ms, 0, sample_rate)
    }

    pub(crate) fn new_with_hold(
        level: f32,
        attack_ms: f32,
        decay_ms: f32,
        hold_samples: u64,
        sample_rate: f32,
    ) -> Self {
        let attack_total = (attack_ms * 0.001 * sample_rate).round() as u64;
        let total = (decay_ms * 0.001 * sample_rate).round() as u64;
        Self {
            noise: WhiteNoise::new(),
            attack_samples_remaining: attack_total,
            attack_total_samples: attack_total,
            hold_samples_remaining: hold_samples,
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
        if self.hold_samples_remaining > 0 {
            self.hold_samples_remaining -= 1;
            return self.noise.next(rng) * self.level * OUTPUT_TRIM;
        }
        if self.samples_remaining == 0 {
            return 0.0;
        }
        let gain = self.samples_remaining as f32 / self.total_samples as f32;
        self.samples_remaining -= 1;
        self.noise.next(rng) * gain * self.level * OUTPUT_TRIM
    }
    pub(crate) fn is_done(&self) -> bool {
        self.attack_samples_remaining == 0
            && self.hold_samples_remaining == 0
            && self.samples_remaining == 0
    }
}
