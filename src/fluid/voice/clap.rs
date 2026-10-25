//! The Clap voice: a burst of several noise slaps per hit.

use super::*;

/// Headroom trim on the summed burst output, not a character control —
/// `clap.level` at 100% should reach close to full scale on its own. Kept
/// below `perc`'s trim since up to `slap_count` (max 8) bursts can overlap
/// and sum before this scale is applied, unlike perc's single hit stream.
const OUTPUT_TRIM: f32 = 0.42;

pub(crate) fn clap_live_step_count(step_count: f32) -> usize {
    (step_count.round() as i64).clamp(1, CLAP_STEP_COUNT as i64) as usize
}

pub(crate) fn clap_pattern_label(value: f32) -> &'static str {
    CLAP_PATTERNS[wrapped_index(value, CLAP_PATTERNS.len())]
}

pub(crate) fn clap_step_label(value: f32) -> &'static str {
    match (value.round() as i32).clamp(0, 1) {
        0 => "Rest",
        _ => "Hit",
    }
}

pub(crate) fn clap_authored_steps(c: &ClapControls) -> [f32; CLAP_STEP_COUNT] {
    let index = (c.pattern.round() as isize).clamp(0, CUSTOM_CLAP_PATTERN_INDEX as isize) as usize;
    if index < CLAP_PATTERN_PRESET_COUNT {
        CLAP_PATTERN_PRESETS[index].1
    } else {
        c.steps
    }
}

pub(crate) struct ClapEngine {
    pub(crate) sample_rate: f32,
    pub(crate) trigger: GridTrigger,
    pub(crate) voices: Vec<ClapVoice>,
    pub(crate) rng: StdRng,
    pub(crate) telemetry: Arc<FluidTelemetry>,
    pub(crate) last_cycle: Option<u64>,
    pub(crate) active_steps_source: [f32; CLAP_STEP_COUNT],
    pub(crate) evolved_steps: [f32; CLAP_STEP_COUNT],
    pub(crate) evolution_seed: u64,
    pub(crate) evolution_count: u64,
    pub(crate) session_state: Option<LiveSession>,
}

impl ClapEngine {
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
        let evolution_seed = StdRng::from_entropy().r#gen();
        Self {
            sample_rate,
            trigger: GridTrigger::new(),
            voices: Vec::with_capacity(4),
            rng: StdRng::from_entropy(),
            telemetry,
            last_cycle: None,
            active_steps_source: DEFAULT_CLAP_STEPS,
            evolved_steps: DEFAULT_CLAP_STEPS,
            evolution_seed,
            evolution_count: 0,
            session_state,
        }
    }

    fn evolve_steps(&mut self, live_len: usize, evolve_rate: f32, cycle: u64) {
        let live_len = live_len.clamp(1, CLAP_STEP_COUNT);
        let seed = self
            .evolution_seed
            .wrapping_add(
                self.evolution_count
                    .wrapping_add(1)
                    .wrapping_mul(0x9E37_79B9_7F4A_7C15),
            )
            .wrapping_add(cycle.rotate_left(23));
        let mutations = evolve_mutation_count(evolve_rate, 3, seed);
        if mutations == 0 {
            return;
        }
        let mut evo_rng = StdRng::seed_from_u64(seed);
        for _ in 0..mutations {
            let index = evo_rng.gen_range(0..live_len);
            let current = (self.evolved_steps[index].round() as i32).clamp(0, 1);
            let next = if current <= 0 { 1 } else { 0 };
            self.evolved_steps[index] = next as f32;
        }
        if self.evolved_steps[..live_len]
            .iter()
            .all(|&v| v.round() as i32 <= 0)
        {
            self.evolved_steps[0] = 1.0;
        }
        self.evolution_count = self.evolution_count.saturating_add(1);
        if let Some(session_state) = &self.session_state {
            let steps = self.evolved_steps;
            session_state.update(|snapshot| {
                snapshot.controls.clap.steps = steps;
                snapshot.controls.clap.pattern = CUSTOM_CLAP_PATTERN_INDEX as f32;
            });
        }
    }

    pub(crate) fn next(&mut self, c: &ClapControls, timing: TimingContext) -> (f32, f32) {
        if self
            .trigger
            .pop_swung(timing, c.interval_beats, c.offset_beats, c.swing)
        {
            let pattern_index =
                (c.pattern.round() as isize).clamp(0, CUSTOM_CLAP_PATTERN_INDEX as isize) as usize;
            let authored = clap_authored_steps(c);
            let is_custom = pattern_index >= CUSTOM_CLAP_PATTERN_INDEX;
            if is_custom && c.steps == self.evolved_steps {
                self.active_steps_source = c.steps;
            } else if authored != self.active_steps_source {
                self.active_steps_source = authored;
                self.evolved_steps = authored;
                self.last_cycle = None;
            }
            let count = clap_live_step_count(c.step_count);
            let interval = c.interval_beats.max(0.125);
            let cycle_beats = (interval * count as f32).max(0.125);
            let cycle = (timing.beat.max(0.0) / f64::from(cycle_beats)).floor() as u64;
            if self.last_cycle.is_some_and(|last| cycle > last) && c.evolve_rate > 0.0 {
                self.evolve_steps(count, c.evolve_rate, cycle);
            }
            self.last_cycle = Some(cycle);

            let step = lane_step_at(timing.beat, interval, c.offset_beats, count);
            let step_val = (self.evolved_steps[step].round() as i32).clamp(0, 1);
            if step_val > 0 {
                self.voices
                    .push(ClapVoice::new(c, self.sample_rate, &mut self.rng));
                self.telemetry
                    .publish_hit(MusicalHit::Clap, c.level, NO_PITCH_CLASS);
            }
        }

        let rng = &mut self.rng;
        let dry = mix_and_retain_mono(&mut self.voices, |v| v.next(rng), ClapVoice::is_done);
        (dry, dry)
    }
}

pub(crate) struct ClapVoice {
    pub(crate) noise: WhiteNoise,
    pub(crate) scheduled: Vec<u64>,
    pub(crate) bursts: Vec<ClapBurst>,
    pub(crate) current: u64,
    pub(crate) decay_samples: u64,
    pub(crate) body_coeff: f32,
    pub(crate) body_state: f32,
    pub(crate) level: f32,
}

impl ClapVoice {
    pub(crate) fn new(c: &ClapControls, sample_rate: f32, rng: &mut StdRng) -> Self {
        let count = c.slap_count.round().max(1.0) as usize;
        let spread = (c.slap_spread_ms * 0.001 * sample_rate) as u64;
        let mut scheduled: Vec<u64> = (0..count)
            .map(|i| {
                if i == 0 {
                    0
                } else {
                    rng.gen_range(0..=spread.max(1))
                }
            })
            .collect();
        scheduled.sort_unstable();
        Self {
            noise: WhiteNoise::new(),
            scheduled,
            bursts: Vec::new(),
            current: 0,
            decay_samples: (c.decay_ms * 0.001 * sample_rate).round() as u64,
            body_coeff: c.body * 0.08,
            body_state: 0.0,
            level: c.level,
        }
    }

    pub(crate) fn next<R: Rng>(&mut self, rng: &mut R) -> f32 {
        self.scheduled.retain(|&t| {
            if self.current >= t {
                self.bursts.push(ClapBurst {
                    remaining: self.decay_samples,
                    total: self.decay_samples,
                });
                false
            } else {
                true
            }
        });

        if self.bursts.is_empty() && self.scheduled.is_empty() {
            return 0.0;
        }

        let mut out = 0.0f32;
        for burst in &mut self.bursts {
            if burst.remaining > 0 {
                let env = (burst.remaining as f32 / burst.total as f32).sqrt();
                burst.remaining -= 1;
                let raw = self.noise.next(rng);
                self.body_state += self.body_coeff * (raw - self.body_state);
                out += (raw + self.body_state) * env;
            }
        }
        self.bursts.retain(|b| b.remaining > 0);

        self.current += 1;
        out * self.level * OUTPUT_TRIM
    }

    pub(crate) fn is_done(&self) -> bool {
        self.scheduled.is_empty() && self.bursts.is_empty()
    }
}

pub(crate) struct ClapBurst {
    pub(crate) remaining: u64,
    pub(crate) total: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swing_delays_the_claps_odd_subdivision() {
        let mut clap = ClapEngine::new(48_000.0);
        let controls = ClapControls {
            level: 1.0,
            interval_beats: 0.5,
            swing: 1.0,
            ..ClapControls::default()
        };

        clap.next(&controls, TimingContext::new(48_000.0, 120.0, 0.0));
        clap.next(&controls, TimingContext::new(48_000.0, 120.0, 0.5));
        assert_eq!(clap.voices.len(), 1);

        clap.next(&controls, TimingContext::new(48_000.0, 120.0, 0.75));
        assert_eq!(clap.voices.len(), 2);
    }
}
