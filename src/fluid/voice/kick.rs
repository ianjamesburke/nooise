use super::*;

// ============================================================
// Kick engine
// ============================================================

pub(crate) fn max_kick_echo_delay_samples(sample_rate: f32) -> usize {
    ((KICK_ECHO_TIME_BEATS_MAX * 60.0 / MASTER_BPM_MIN) * sample_rate).ceil() as usize + 1
}

pub(crate) struct KickEngine {
    pub(crate) sample_rate: f32,
    pub(crate) trigger: GridTrigger,
    pub(crate) voices: Vec<KickVoice>,
    pub(crate) delay: KickDelay,
    pub(crate) rng: StdRng,
    pub(crate) telemetry: Arc<FluidTelemetry>,
}

impl KickEngine {
    pub(crate) fn new(sample_rate: f32, telemetry: Arc<FluidTelemetry>) -> Self {
        Self {
            sample_rate,
            trigger: GridTrigger::new(),
            voices: Vec::with_capacity(4),
            delay: KickDelay::new(max_kick_echo_delay_samples(sample_rate)),
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

        let mut dry_l = 0.0f32;
        let mut dry_r = 0.0f32;
        for v in &mut self.voices {
            let (l, r) = v.next(&mut self.rng);
            dry_l += l;
            dry_r += r;
        }
        self.voices.retain(|v| !v.is_done());

        let delay_samples = timing.beats_to_samples(c.echo_time_beats) as usize;
        let (echo_l, echo_r) = self.delay.process(
            dry_l,
            dry_r,
            delay_samples,
            c.echo_filter,
            c.echo_amount,
            c.echo_feedback,
        );
        (dry_l + echo_l, dry_r + echo_r)
    }
}

pub(crate) struct KickDelay {
    pub(crate) buf_l: Vec<f32>,
    pub(crate) buf_r: Vec<f32>,
    pub(crate) head: usize,
    pub(crate) lp_l: f32,
    pub(crate) lp_r: f32,
    pub(crate) hp_l: f32,
    pub(crate) hp_r: f32,
}

impl KickDelay {
    pub(crate) fn new(max_samples: usize) -> Self {
        let n = max_samples.max(2);
        Self {
            buf_l: vec![0.0; n],
            buf_r: vec![0.0; n],
            head: 0,
            lp_l: 0.0,
            lp_r: 0.0,
            hp_l: 0.0,
            hp_r: 0.0,
        }
    }

    pub(crate) fn process(
        &mut self,
        in_l: f32,
        in_r: f32,
        delay_samples: usize,
        echo_filter: f32,
        echo_amount: f32,
        feedback: f32,
    ) -> (f32, f32) {
        let len = self.buf_l.len();
        let delay = delay_samples.clamp(1, len - 1);
        let read_pos = (self.head + len - delay) % len;

        // Wide band-pass: LP at ~2kHz centre, HP at ~60Hz, both gentle (one-pole).
        // echo_filter sweeps the LP cutoff from ~200Hz (0.0) to ~8kHz (1.0).
        let lp_coeff = 10_f32.powf(echo_filter * 3.6 - 2.3); // ~0.005..2.0 → clamp keeps it stable
        let lp_coeff = lp_coeff.clamp(0.001, 0.99);
        let hp_coeff = 0.9994_f32; // ~30 Hz high-pass, removes DC only

        self.lp_l += lp_coeff * (self.buf_l[read_pos] - self.lp_l);
        self.lp_r += lp_coeff * (self.buf_r[read_pos] - self.lp_r);
        let bp_l = self.lp_l - self.hp_l;
        let bp_r = self.lp_r - self.hp_r;
        self.hp_l = self.lp_l - bp_l * (1.0 - hp_coeff);
        self.hp_r = self.lp_r - bp_r * (1.0 - hp_coeff);

        self.buf_l[self.head] = in_l + bp_l * feedback;
        self.buf_r[self.head] = in_r + bp_r * feedback;
        self.head = (self.head + 1) % len;
        (bp_l * echo_amount, bp_r * echo_amount)
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
    pub(crate) pan: f32,
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
            pan: rng.gen_range(-0.15f32..0.15),
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

        if self.drive > 0.0 {
            let driven = s * (1.0 + self.drive * 8.0);
            s = driven / (1.0 + driven.abs()) * (1.0 + self.drive * 0.5);
        }

        self.lp_state += self.lp_coeff * (s - self.lp_state);
        s = self.lp_state;

        self.amp *= self.amp_decay;
        StereoPanner::equal_power(s, self.pan)
    }

    pub(crate) fn is_done(&self) -> bool {
        self.amp < 0.0001
    }
}
