use super::*;

// ============================================================
// Kick engine
// ============================================================

pub(crate) struct KickEngine {
    pub(crate) sample_rate: f32,
    pub(crate) trigger: GridTrigger,
    pub(crate) voices: Vec<KickVoice>,
    pub(crate) rng: StdRng,
    pub(crate) telemetry: Arc<FluidTelemetry>,
}

impl KickEngine {
    pub(crate) fn new(sample_rate: f32, telemetry: Arc<FluidTelemetry>) -> Self {
        Self {
            sample_rate,
            trigger: GridTrigger::new(),
            voices: Vec::with_capacity(4),
            rng: StdRng::from_entropy(),
            telemetry,
        }
    }

    pub(crate) fn next(&mut self, c: &KickControls, timing: TimingContext) -> (f32, f32) {
        if self.trigger.pop(timing, c.interval_beats, c.offset_beats) {
            self.voices
                .push(KickVoice::new(c, self.sample_rate, &mut self.rng));
            self.telemetry.kick_pulse.fetch_add(1, Ordering::Relaxed);
        }

        let rng = &mut self.rng;
        mix_and_retain(&mut self.voices, |v| v.next(rng), KickVoice::is_done)
    }
}

pub(crate) struct KickVoice {
    pub(crate) phase: f32,
    pub(crate) mod_phase: f32,
    pub(crate) freq: f32,
    pub(crate) target_freq: f32,
    pub(crate) freq_glide: f32,
    pub(crate) amp: f32,
    pub(crate) amp_decay: f32,
    pub(crate) fm_depth: f32,
    pub(crate) fm_depth_decay: f32,
    pub(crate) lp_state: f32,
    pub(crate) lp_coeff: f32,
    pub(crate) click_remaining: u64,
    pub(crate) click_level: f32,
    pub(crate) drive: f32,
    pub(crate) pan_gains: (f32, f32),
    pub(crate) sample_rate: f32,
}

impl KickVoice {
    pub(crate) fn new(c: &KickControls, sample_rate: f32, rng: &mut StdRng) -> Self {
        let tau = (c.pitch_decay_ms * 0.001 * sample_rate / 3.0).max(1.0);
        let amp_tau = (c.amp_decay_ms * 0.001 * sample_rate / 3.0).max(1.0);
        // FM depth decays ~3x faster than pitch for a tight transient thud
        let fm_tau = (c.pitch_decay_ms * 0.001 * sample_rate / 9.0).max(1.0);
        Self {
            phase: 0.0,
            mod_phase: 0.0,
            freq: c.start_freq,
            target_freq: c.start_freq * 0.28,
            freq_glide: 1.0 / tau,
            amp: c.level,
            amp_decay: (-1.0 / amp_tau).exp(),
            fm_depth: 3.5,
            fm_depth_decay: (-1.0 / fm_tau).exp(),
            lp_state: 0.0,
            lp_coeff: 10_f32.powf(c.filter * 3.0 - 2.5).clamp(0.01, 0.99),
            click_remaining: (c.amp_decay_ms * 0.001 * sample_rate * 0.04).round() as u64,
            click_level: c.click,
            drive: c.drive,
            pan_gains: StereoPanner::gains(rng.gen_range(-0.15f32..0.15)),
            sample_rate,
        }
    }

    pub(crate) fn next<R: Rng>(&mut self, rng: &mut R) -> (f32, f32) {
        if self.amp < 0.0001 {
            return (0.0, 0.0);
        }

        self.freq += (self.target_freq - self.freq) * self.freq_glide;

        // FM: modulator at 2x carrier freq, decaying depth
        let mod_freq = self.freq * 2.0;
        self.mod_phase += TAU * mod_freq / self.sample_rate;
        if self.mod_phase >= TAU {
            self.mod_phase -= TAU;
        }
        let fm = self.mod_phase.sin() * self.fm_depth * self.freq;
        self.fm_depth *= self.fm_depth_decay;

        self.phase += TAU * (self.freq + fm) / self.sample_rate;
        if self.phase >= TAU {
            self.phase -= TAU;
        }

        let mut s = self.phase.sin() * self.amp;

        if self.click_remaining > 0 {
            s += rng.gen_range(-1.0f32..1.0) * self.click_level * self.amp;
            self.click_remaining -= 1;
        }

        s = drive_stage(s, self.drive);

        self.lp_state += self.lp_coeff * (s - self.lp_state);
        s = self.lp_state;

        self.amp *= self.amp_decay;
        (s * self.pan_gains.0, s * self.pan_gains.1)
    }

    pub(crate) fn is_done(&self) -> bool {
        self.amp < 0.0001
    }
}
