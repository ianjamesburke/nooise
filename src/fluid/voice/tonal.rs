use super::*;

// ============================================================
// Tonal engine (melodic steps with randomness)
// ============================================================

pub(crate) struct TonalEngine {
    pub(crate) sample_rate: f32,
    pub(crate) trigger: GridTrigger,
    pub(crate) step_index: usize,
    pub(crate) voices: Vec<TonalVoice>,
    pub(crate) reverb: Freeverb,
    pub(crate) rng: StdRng,
}

pub(crate) const SCALE_HZ: [f32; 10] = [
    110.0, 130.81, 146.83, 164.81, 196.0, 220.0, 261.63, 293.66, 329.63, 392.0,
];
pub(crate) const PATTERN: [usize; 8] = [0, 2, 4, 1, 3, 5, 2, 4];

impl TonalEngine {
    pub(crate) fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            trigger: GridTrigger::new(),
            step_index: 0,
            voices: Vec::with_capacity(8),
            reverb: Freeverb::new(sample_rate, 0.86, 0.38, 0.9),
            rng: StdRng::from_entropy(),
        }
    }

    pub(crate) fn next(&mut self, c: &TonalControls, timing: TimingContext) -> (f32, f32) {
        if self
            .trigger
            .pop(timing, c.step_interval_beats, c.offset_beats)
        {
            let degree = if self.rng.gen_range(0.0f32..1.0) < c.randomness {
                self.rng.gen_range(0..SCALE_HZ.len())
            } else {
                let d = PATTERN[self.step_index % PATTERN.len()];
                self.step_index += 1;
                d
            };
            let hz = SCALE_HZ[degree];
            let decay_samples = timing.beats_to_samples(c.note_length_beats);
            let pan = self.rng.gen_range(-0.5f32..0.5);
            self.voices.push(TonalVoice::new(
                hz,
                pan,
                c.level,
                decay_samples,
                self.sample_rate,
            ));
        }

        let mut dry_l = 0.0f32;
        let mut dry_r = 0.0f32;
        for v in &mut self.voices {
            let (l, r) = v.next();
            dry_l += l;
            dry_r += r;
        }
        self.voices.retain(|v| !v.is_done());

        let (wet_l, wet_r) = self
            .reverb
            .process(dry_l * c.reverb_mix, dry_r * c.reverb_mix);
        (
            dry_l * (1.0 - c.reverb_mix * 0.5) + wet_l,
            dry_r * (1.0 - c.reverb_mix * 0.5) + wet_r,
        )
    }
}

pub(crate) struct TonalVoice {
    pub(crate) primary: SineOscillator,
    pub(crate) detuned: SineOscillator,
    pub(crate) samples_remaining: u64,
    pub(crate) total_samples: u64,
    pub(crate) pan: f32,
    pub(crate) level: f32,
}

impl TonalVoice {
    pub(crate) fn new(hz: f32, pan: f32, level: f32, decay_samples: u64, sample_rate: f32) -> Self {
        let total = decay_samples.max(1);
        Self {
            primary: SineOscillator::new(hz, sample_rate),
            detuned: SineOscillator::new(hz * 1.004, sample_rate),
            samples_remaining: total,
            total_samples: total,
            pan,
            level,
        }
    }
    pub(crate) fn next(&mut self) -> (f32, f32) {
        if self.samples_remaining == 0 {
            return (0.0, 0.0);
        }
        let gain = (self.samples_remaining as f32 / self.total_samples as f32).sqrt();
        self.samples_remaining -= 1;
        let s =
            soft_clip((self.primary.next() + self.detuned.next() * 0.3) * 0.4) * gain * self.level;
        StereoPanner::equal_power(s, self.pan)
    }
    pub(crate) fn is_done(&self) -> bool {
        self.samples_remaining == 0
    }
}
