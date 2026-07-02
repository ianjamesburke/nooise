use super::*;

// ============================================================
// Fluid Engine
// ============================================================

pub(crate) struct FluidEngine {
    pub(crate) current_sample: u64,
    pub(crate) sample_rate: f32,
    pub(crate) tempo: TempoClock,
    pub(crate) gain_smoothers: GainSmoothers,
    pub(crate) pad: PadEngine,
    pub(crate) perc: PercEngine,
    pub(crate) kick: KickEngine,
    pub(crate) tonal: TonalEngine,
    pub(crate) clap: ClapEngine,
    pub(crate) bass: BassEngine,
    pub(crate) master_bus: MasterBus,
    pub(crate) controls: Arc<ArcSwap<FluidControls>>,
    pub(crate) automation: Arc<ArcSwap<AutomationState>>,
    pub(crate) telemetry: Arc<FluidTelemetry>,
    pub(crate) snapshot: FluidControls,
}

impl FluidEngine {
    pub(crate) fn new(
        sample_rate: f32,
        controls: Arc<ArcSwap<FluidControls>>,
        automation: Arc<ArcSwap<AutomationState>>,
        telemetry: Arc<FluidTelemetry>,
    ) -> Self {
        let snapshot = FluidControls::clone(&controls.load());
        Self {
            current_sample: 0,
            sample_rate,
            tempo: TempoClock::new(sample_rate, snapshot.master.bpm),
            gain_smoothers: GainSmoothers::new(&snapshot),
            pad: PadEngine::new(sample_rate, &snapshot.pad, Arc::clone(&telemetry)),
            perc: PercEngine::new(sample_rate),
            kick: KickEngine::new(sample_rate, Arc::clone(&telemetry)),
            tonal: TonalEngine::new(sample_rate),
            clap: ClapEngine::new(sample_rate),
            bass: BassEngine::new(sample_rate),
            master_bus: MasterBus::new(),
            controls,
            automation,
            telemetry,
            snapshot,
        }
    }
}

impl FluidEngine {
    /// Reseed every voice RNG for reproducible offline renders.
    pub(crate) fn reseed(&mut self, seed: u64) {
        self.pad.rng = StdRng::seed_from_u64(seed);
        self.perc.rng = StdRng::seed_from_u64(seed.wrapping_add(1));
        self.kick.rng = StdRng::seed_from_u64(seed.wrapping_add(2));
        self.tonal.rng = StdRng::seed_from_u64(seed.wrapping_add(3));
        self.clap.rng = StdRng::seed_from_u64(seed.wrapping_add(4));
    }
}

impl StereoEngine for FluidEngine {
    fn next_stereo(&mut self) -> (f32, f32) {
        if self.current_sample.is_multiple_of(512) {
            self.snapshot = FluidControls::clone(&self.controls.load());
            self.gain_smoothers
                .set_targets(&self.snapshot, self.sample_rate);
        }

        let fade = (self.current_sample as f32 / (self.sample_rate * 8.0)).min(1.0);
        let mut effective = self.gain_smoothers.next_controls(&self.snapshot);
        let timing = self.tempo.tick(effective.master.bpm);
        if self.current_sample.is_multiple_of(256) {
            self.telemetry.publish_beat(timing.beat);
        }
        let automation = self.automation.load();
        apply_automation(&mut effective, &automation, timing);

        let tune = effective.master.tune;
        let (pad_l, pad_r) = self.pad.next(&effective.pad, tune, timing);
        let perc = self.perc.next(&effective.perc, timing);
        let (kick_l, kick_r) = self.kick.next(&effective.kick, timing);
        let (ton_l, ton_r) = self.tonal.next(&effective.tonal, timing);
        let (clap_l, clap_r) = self.clap.next(&effective.clap, timing);
        let (bass_l, bass_r) = self
            .bass
            .next(&effective.bass, &effective.pad, tune, timing);

        self.current_sample += 1;

        let raw_l =
            (pad_l + perc * 0.6 + kick_l * 0.7 + ton_l + clap_l * 0.65 + bass_l * 0.75) * fade;
        let raw_r =
            (pad_r + perc * 0.6 + kick_r * 0.7 + ton_r + clap_r * 0.65 + bass_r * 0.75) * fade;
        self.master_bus
            .process(raw_l, raw_r, &effective.master, self.sample_rate)
    }
}

pub(crate) struct GainSmoother {
    pub(crate) current: f32,
    pub(crate) target: f32,
    pub(crate) step: f32,
    pub(crate) samples_remaining: u32,
}

impl GainSmoother {
    pub(crate) fn new(value: f32) -> Self {
        Self {
            current: value,
            target: value,
            step: 0.0,
            samples_remaining: 0,
        }
    }

    pub(crate) fn set_target(&mut self, target: f32, ramp_samples: u32) {
        if (target - self.target).abs() <= f32::EPSILON {
            return;
        }
        self.target = target;
        self.samples_remaining = ramp_samples.max(1);
        self.step = (self.target - self.current) / self.samples_remaining as f32;
    }

    pub(crate) fn next(&mut self) -> f32 {
        if self.samples_remaining == 0 {
            self.current = self.target;
            return self.current;
        }
        self.current += self.step;
        self.samples_remaining -= 1;
        if self.samples_remaining == 0 {
            self.current = self.target;
        }
        self.current
    }
}

pub(crate) fn set_smooth_target(
    smoother: &mut GainSmoother,
    kind: ControlKind,
    target: f32,
    ramp_samples: u32,
) {
    if kind.smooths_audio() {
        smoother.set_target(target, ramp_samples);
    }
}

pub(crate) struct GainSmoothers {
    pub(crate) pad: GainSmoother,
    pub(crate) pad_reverb_mix: GainSmoother,
    pub(crate) pad_stereo_width: GainSmoother,
    pub(crate) pad_detune: GainSmoother,
    pub(crate) pad_octave_mix: GainSmoother,
    pub(crate) perc: GainSmoother,
    pub(crate) perc_filter: GainSmoother,
    pub(crate) perc_lfo_depth: GainSmoother,
    pub(crate) kick: GainSmoother,
    pub(crate) kick_echo_filter: GainSmoother,
    pub(crate) kick_echo_amount: GainSmoother,
    pub(crate) kick_echo_feedback: GainSmoother,
    pub(crate) tonal: GainSmoother,
    pub(crate) tonal_reverb_mix: GainSmoother,
    pub(crate) clap: GainSmoother,
    pub(crate) clap_room: GainSmoother,
    pub(crate) bass: GainSmoother,
    pub(crate) master: GainSmoother,
    pub(crate) master_drive: GainSmoother,
}

impl GainSmoothers {
    pub(crate) fn new(c: &FluidControls) -> Self {
        Self {
            pad: GainSmoother::new(c.pad.level),
            pad_reverb_mix: GainSmoother::new(c.pad.reverb_mix),
            pad_stereo_width: GainSmoother::new(c.pad.stereo_width),
            pad_detune: GainSmoother::new(c.pad.detune),
            pad_octave_mix: GainSmoother::new(c.pad.octave_mix),
            perc: GainSmoother::new(c.perc.level),
            perc_filter: GainSmoother::new(c.perc.filter),
            perc_lfo_depth: GainSmoother::new(c.perc.lfo_depth),
            kick: GainSmoother::new(c.kick.level),
            kick_echo_filter: GainSmoother::new(c.kick.echo_filter),
            kick_echo_amount: GainSmoother::new(c.kick.echo_amount),
            kick_echo_feedback: GainSmoother::new(c.kick.echo_feedback),
            tonal: GainSmoother::new(c.tonal.level),
            tonal_reverb_mix: GainSmoother::new(c.tonal.reverb_mix),
            clap: GainSmoother::new(c.clap.level),
            clap_room: GainSmoother::new(c.clap.room),
            bass: GainSmoother::new(c.bass.level),
            master: GainSmoother::new(c.master.level),
            master_drive: GainSmoother::new(c.master.drive),
        }
    }

    pub(crate) fn set_targets(&mut self, c: &FluidControls, sample_rate: f32) {
        let ramp_samples = (LEVEL_RAMP_MS * 0.001 * sample_rate).round() as u32;
        set_smooth_target(&mut self.pad, ControlKind::Gain, c.pad.level, ramp_samples);
        set_smooth_target(
            &mut self.pad_reverb_mix,
            ControlKind::Gain,
            c.pad.reverb_mix,
            ramp_samples,
        );
        set_smooth_target(
            &mut self.pad_stereo_width,
            ControlKind::Gain,
            c.pad.stereo_width,
            ramp_samples,
        );
        set_smooth_target(
            &mut self.pad_detune,
            ControlKind::Gain,
            c.pad.detune,
            ramp_samples,
        );
        set_smooth_target(
            &mut self.pad_octave_mix,
            ControlKind::Gain,
            c.pad.octave_mix,
            ramp_samples,
        );
        set_smooth_target(
            &mut self.perc,
            ControlKind::Gain,
            c.perc.level,
            ramp_samples,
        );
        set_smooth_target(
            &mut self.perc_filter,
            ControlKind::Gain,
            c.perc.filter,
            ramp_samples,
        );
        set_smooth_target(
            &mut self.perc_lfo_depth,
            ControlKind::Gain,
            c.perc.lfo_depth,
            ramp_samples,
        );
        set_smooth_target(
            &mut self.kick,
            ControlKind::Gain,
            c.kick.level,
            ramp_samples,
        );
        set_smooth_target(
            &mut self.kick_echo_filter,
            ControlKind::Gain,
            c.kick.echo_filter,
            ramp_samples,
        );
        set_smooth_target(
            &mut self.kick_echo_amount,
            ControlKind::Gain,
            c.kick.echo_amount,
            ramp_samples,
        );
        set_smooth_target(
            &mut self.kick_echo_feedback,
            ControlKind::Gain,
            c.kick.echo_feedback,
            ramp_samples,
        );
        set_smooth_target(
            &mut self.tonal,
            ControlKind::Gain,
            c.tonal.level,
            ramp_samples,
        );
        set_smooth_target(
            &mut self.tonal_reverb_mix,
            ControlKind::Gain,
            c.tonal.reverb_mix,
            ramp_samples,
        );
        set_smooth_target(
            &mut self.clap,
            ControlKind::Gain,
            c.clap.level,
            ramp_samples,
        );
        set_smooth_target(
            &mut self.clap_room,
            ControlKind::Gain,
            c.clap.room,
            ramp_samples,
        );
        set_smooth_target(
            &mut self.bass,
            ControlKind::Gain,
            c.bass.level,
            ramp_samples,
        );
        set_smooth_target(
            &mut self.master,
            ControlKind::Gain,
            c.master.level,
            ramp_samples,
        );
        set_smooth_target(
            &mut self.master_drive,
            ControlKind::Gain,
            c.master.drive,
            ramp_samples,
        );
    }

    pub(crate) fn next_controls(&mut self, c: &FluidControls) -> FluidControls {
        let mut next = c.clone();
        next.pad.level = self.pad.next();
        next.pad.reverb_mix = self.pad_reverb_mix.next();
        next.pad.stereo_width = self.pad_stereo_width.next();
        next.pad.detune = self.pad_detune.next();
        next.pad.octave_mix = self.pad_octave_mix.next();
        next.perc.level = self.perc.next();
        next.perc.filter = self.perc_filter.next();
        next.perc.lfo_depth = self.perc_lfo_depth.next();
        next.kick.level = self.kick.next();
        next.kick.echo_filter = self.kick_echo_filter.next();
        next.kick.echo_amount = self.kick_echo_amount.next();
        next.kick.echo_feedback = self.kick_echo_feedback.next();
        next.tonal.level = self.tonal.next();
        next.tonal.reverb_mix = self.tonal_reverb_mix.next();
        next.clap.level = self.clap.next();
        next.clap.room = self.clap_room.next();
        next.bass.level = self.bass.next();
        next.master.level = self.master.next();
        next.master.drive = self.master_drive.next();
        next
    }
}

pub(crate) const TEMPO_SMOOTH_MS: f64 = 180.0;

pub(crate) struct TempoClock {
    pub(crate) beat: f64,
    pub(crate) bpm: f64,
    pub(crate) sample_rate: f64,
}

impl TempoClock {
    pub(crate) fn new(sample_rate: f32, bpm: f32) -> Self {
        Self {
            beat: 0.0,
            bpm: f64::from(bpm.clamp(MASTER_BPM_MIN, MASTER_BPM_MAX)),
            sample_rate: f64::from(sample_rate.max(1.0)),
        }
    }

    pub(crate) fn tick(&mut self, target_bpm: f32) -> TimingContext {
        let target_bpm = f64::from(target_bpm.clamp(MASTER_BPM_MIN, MASTER_BPM_MAX));
        let smoothing_samples = (TEMPO_SMOOTH_MS * 0.001 * self.sample_rate).max(1.0);
        let coeff = 1.0 - (-1.0 / smoothing_samples).exp();
        self.bpm += (target_bpm - self.bpm) * coeff;

        let timing = TimingContext::new(self.sample_rate, self.bpm, self.beat);
        self.beat += self.bpm / (60.0 * self.sample_rate);
        timing
    }
}

#[derive(Clone, Copy)]
pub(crate) struct TimingContext {
    pub(crate) sample_rate: f64,
    pub(crate) bpm: f64,
    pub(crate) beat: f64,
}

impl TimingContext {
    pub(crate) fn new(sample_rate: f64, bpm: f64, beat: f64) -> Self {
        Self {
            sample_rate: sample_rate.max(1.0),
            bpm: bpm.max(1.0),
            beat,
        }
    }

    pub(crate) fn samples_per_beat(self) -> f64 {
        self.sample_rate * 60.0 / self.bpm
    }

    pub(crate) fn beats_to_samples(self, beats: f32) -> u64 {
        (f64::from(beats.max(0.0)) * self.samples_per_beat())
            .round()
            .max(1.0) as u64
    }

    pub(crate) fn lfo_hz_for_bars(self, bars: f32) -> f32 {
        (self.bpm as f32) / (240.0 * bars.max(1.0 / 64.0))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GridSpec {
    pub(crate) interval_beats: f64,
    pub(crate) offset_beats: f64,
}

impl GridSpec {
    pub(crate) fn new(interval_beats: f32, offset_beats: f32) -> Self {
        let interval_beats = f64::from(interval_beats).max(1.0 / 64.0);
        Self {
            interval_beats,
            offset_beats: f64::from(offset_beats).rem_euclid(interval_beats),
        }
    }

    pub(crate) fn hit_at_or_after(self, beat: f64) -> GridHit {
        let interval = self.interval_beats;
        let offset = self.offset_beats;
        let slot = if beat <= offset {
            0
        } else {
            ((beat - offset) / interval).ceil().max(0.0) as u64
        };
        GridHit {
            beat: offset + slot as f64 * interval,
        }
    }

    pub(crate) fn hit_after(self, beat: f64) -> GridHit {
        self.hit_at_or_after(beat + GRID_BEAT_EPSILON)
    }
}

pub(crate) const GRID_BEAT_EPSILON: f64 = 1e-9;

#[derive(Clone, Copy, Debug)]
pub(crate) struct GridHit {
    pub(crate) beat: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FirstGridHit {
    AtOrAfterNow,
    AfterNow,
}

pub(crate) struct GridTrigger {
    pub(crate) spec: Option<GridSpec>,
    pub(crate) next_hit: Option<GridHit>,
    pub(crate) first_hit: FirstGridHit,
}

impl GridTrigger {
    pub(crate) fn new() -> Self {
        Self::with_first_hit(FirstGridHit::AtOrAfterNow)
    }

    pub(crate) fn after_start() -> Self {
        Self::with_first_hit(FirstGridHit::AfterNow)
    }

    pub(crate) fn with_first_hit(first_hit: FirstGridHit) -> Self {
        Self {
            spec: None,
            next_hit: None,
            first_hit,
        }
    }

    pub(crate) fn pop(
        &mut self,
        timing: TimingContext,
        interval_beats: f32,
        offset_beats: f32,
    ) -> bool {
        let spec = GridSpec::new(interval_beats, offset_beats);
        if self.spec != Some(spec) {
            let first_schedule =
                self.next_hit.is_none() && self.first_hit == FirstGridHit::AfterNow;
            self.spec = Some(spec);
            self.next_hit = Some(if first_schedule {
                spec.hit_after(timing.beat)
            } else {
                spec.hit_at_or_after(timing.beat)
            });
        }

        let Some(next_hit) = self.next_hit else {
            return false;
        };
        if timing.beat + GRID_BEAT_EPSILON >= next_hit.beat {
            self.next_hit = Some(spec.hit_after(timing.beat));
            true
        } else {
            false
        }
    }
}

// ============================================================
// Master bus (drive, tilt EQ, compressor)
// ============================================================

pub(crate) struct MasterBus {
    pub(crate) comp_env: f32,
    pub(crate) tone_l: f32,
    pub(crate) tone_r: f32,
}

impl MasterBus {
    pub(crate) fn new() -> Self {
        Self {
            comp_env: 0.0,
            tone_l: 0.0,
            tone_r: 0.0,
        }
    }

    pub(crate) fn process(
        &mut self,
        mut l: f32,
        mut r: f32,
        c: &MasterControls,
        sample_rate: f32,
    ) -> (f32, f32) {
        if c.drive > 0.001 {
            let gain = 1.0 + c.drive * 6.0;
            l = soft_clip(l * gain);
            r = soft_clip(r * gain);
        }

        if c.tone.abs() > 0.01 {
            let coeff = (0.05 + c.tone.abs() * 0.7).min(0.99);
            self.tone_l += coeff * (l - self.tone_l);
            self.tone_r += coeff * (r - self.tone_r);
            if c.tone > 0.0 {
                l += (l - self.tone_l) * c.tone * 0.6;
                r += (r - self.tone_r) * c.tone * 0.6;
            } else {
                l += self.tone_l * (-c.tone) * 0.6;
                r += self.tone_r * (-c.tone) * 0.6;
            }
        }

        let thresh_lin = 10_f32.powf(c.comp_threshold / 20.0);
        let attack_coeff = (-1.0_f32 / (0.001 * sample_rate)).exp();
        let rel_coeff = (-1.0_f32 / (c.comp_release_ms * 0.001 * sample_rate)).exp();
        let peak = l.abs().max(r.abs());
        self.comp_env = if peak > self.comp_env {
            peak + attack_coeff * (self.comp_env - peak)
        } else {
            peak + rel_coeff * (self.comp_env - peak)
        };
        let gain_reduction = if self.comp_env > thresh_lin && c.comp_ratio > 1.001 {
            (thresh_lin / self.comp_env) * (self.comp_env / thresh_lin).powf(1.0 / c.comp_ratio)
        } else {
            1.0
        };

        (
            (l * gain_reduction * c.level).clamp(-0.95, 0.95),
            (r * gain_reduction * c.level).clamp(-0.95, 0.95),
        )
    }
}
