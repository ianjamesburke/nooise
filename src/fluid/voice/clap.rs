use super::*;

// ============================================================
// Clap engine (multi-slap noise burst with room reverb)
// ============================================================

pub(crate) struct ClapEngine {
    pub(crate) sample_rate: f32,
    pub(crate) trigger: GridTrigger,
    pub(crate) voices: Vec<ClapVoice>,
    pub(crate) reverb: Freeverb,
    pub(crate) rng: StdRng,
}

impl ClapEngine {
    pub(crate) fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            trigger: GridTrigger::new(),
            voices: Vec::with_capacity(4),
            reverb: Freeverb::new(sample_rate, 0.28, 0.62, 0.85),
            rng: StdRng::from_entropy(),
        }
    }

    pub(crate) fn next(&mut self, c: &ClapControls, timing: TimingContext) -> (f32, f32) {
        if self.trigger.pop(timing, c.interval_beats, c.offset_beats) {
            self.voices
                .push(ClapVoice::new(c, self.sample_rate, &mut self.rng));
        }

        let mut dry = 0.0f32;
        for v in &mut self.voices {
            dry += v.next(&mut self.rng);
        }
        self.voices.retain(|v| !v.is_done());

        let (wet_l, wet_r) = self.reverb.process(dry * c.room, dry * c.room);
        let dry_scale = 1.0 - c.room * 0.5;
        (dry * dry_scale + wet_l, dry * dry_scale + wet_r)
    }
}

pub(crate) struct ClapVoice {
    pub(crate) noise: WhiteNoise,
    pub(crate) scheduled: Vec<u64>,
    pub(crate) bursts: Vec<ClapBurst>,
    pub(crate) current: u64,
    pub(crate) decay_samples: u64,
    pub(crate) filter_smoothing: f32,
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
            filter_smoothing: 10_f32.powf(c.filter * 4.0 - 4.0),
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
                let raw = self.noise.next_filtered(rng, self.filter_smoothing);
                self.body_state += self.body_coeff * (raw - self.body_state);
                out += (raw + self.body_state) * env;
            }
        }
        self.bursts.retain(|b| b.remaining > 0);

        self.current += 1;
        out * self.level * 0.35
    }

    pub(crate) fn is_done(&self) -> bool {
        self.scheduled.is_empty() && self.bursts.is_empty()
    }
}

pub(crate) struct ClapBurst {
    pub(crate) remaining: u64,
    pub(crate) total: u64,
}
