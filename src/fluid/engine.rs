use std::collections::BTreeSet;

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
    pub(crate) ambient_reverb: AmbientReverbSend,
    pub(crate) master_bus: MasterBus,
    pub(crate) controls: Arc<ArcSwap<FluidControls>>,
    pub(crate) automation: Arc<ArcSwap<AutomationState>>,
    pub(crate) telemetry: Arc<FluidTelemetry>,
    pub(crate) snapshot: FluidControls,
    /// Allocation-free per-sample plan, rebuilt only when `plan_source`
    /// (the last-seen published automation Arc) changes.
    plan: AutomationPlan,
    plan_source: Arc<AutomationState>,
}

impl FluidEngine {
    pub(crate) fn new(
        sample_rate: f32,
        controls: Arc<ArcSwap<FluidControls>>,
        automation: Arc<ArcSwap<AutomationState>>,
        telemetry: Arc<FluidTelemetry>,
    ) -> Self {
        let snapshot = FluidControls::clone(&controls.load());
        let plan_source = automation.load_full();
        let mut plan = AutomationPlan::default();
        plan.rebuild(&plan_source);
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
            ambient_reverb: AmbientReverbSend::new(sample_rate),
            master_bus: MasterBus::new(&snapshot.master, sample_rate),
            controls,
            automation,
            telemetry,
            snapshot,
            plan,
            plan_source,
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
        // ~2.9 ms at 44.1 kHz: control edits reach the engine within a frame.
        if self.current_sample.is_multiple_of(128) {
            self.snapshot = FluidControls::clone(&self.controls.load());
            self.gain_smoothers
                .set_targets(&self.snapshot, self.sample_rate);
            self.master_bus
                .set_controls(&self.snapshot.master, self.sample_rate);
            let automation = self.automation.load_full();
            if !Arc::ptr_eq(&automation, &self.plan_source) {
                self.plan.rebuild(&automation);
                self.plan_source = automation;
            }
        }

        let fade = (self.current_sample as f32 / (self.sample_rate * 8.0)).min(1.0);
        let mut effective = self.gain_smoothers.next_controls(&self.snapshot);
        let timing = self.tempo.tick(effective.master.bpm);
        if self.current_sample.is_multiple_of(256) {
            self.telemetry.publish_beat(timing.beat);
        }
        self.plan.apply(&mut effective, timing);

        let tune = effective.master.tune;
        let (pad_l, pad_r) = self.pad.next(&effective.pad, tune, timing);
        let perc = self.perc.next(&effective.perc, timing);
        let (kick_l, kick_r) = self.kick.next(&effective.kick, timing);
        let (ton_l, ton_r) = self.tonal.next(&effective.tonal, tune, timing);
        let (clap_l, clap_r) = self.clap.next(&effective.clap, timing);
        let (bass_l, bass_r) = self
            .bass
            .next(&effective.bass, &effective.pad, tune, timing);
        let AmbientReverbFrame {
            pad_l,
            pad_r,
            tonal_l: ton_l,
            tonal_r: ton_r,
            wet_l,
            wet_r,
        } = self.ambient_reverb.process(
            (pad_l, pad_r),
            (ton_l, ton_r),
            effective.pad.reverb_mix,
            effective.tonal.reverb_mix,
        );

        self.current_sample += 1;

        let raw_l =
            (pad_l + perc * 0.6 + kick_l * 0.7 + ton_l + clap_l * 0.65 + bass_l * 0.75 + wet_l)
                * fade;
        let raw_r =
            (pad_r + perc * 0.6 + kick_r * 0.7 + ton_r + clap_r * 0.65 + bass_r * 0.75 + wet_r)
                * fade;
        self.master_bus.process(raw_l, raw_r, &effective.master)
    }
}

pub(crate) struct GainSmoother {
    pub(crate) spec: Option<&'static ControlSpec>,
    pub(crate) current: f32,
    pub(crate) target: f32,
    pub(crate) step: f32,
    pub(crate) samples_remaining: u32,
}

impl GainSmoother {
    #[cfg(test)]
    pub(crate) fn new(value: f32) -> Self {
        Self::for_spec(None, value)
    }

    pub(crate) fn for_spec(spec: Option<&'static ControlSpec>, value: f32) -> Self {
        Self {
            spec,
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

pub(crate) struct GainSmoothers {
    pub(crate) smoothers: Vec<GainSmoother>,
}

impl GainSmoothers {
    pub(crate) fn new(c: &FluidControls) -> Self {
        let mut seen = BTreeSet::new();
        let smoothers = all_specs()
            .filter(|spec| spec.kind.smooths_audio())
            .filter(|spec| seen.insert(spec.id))
            .map(|spec| GainSmoother::for_spec(Some(spec), (spec.get)(c)))
            .collect();
        Self { smoothers }
    }

    pub(crate) fn set_targets(&mut self, c: &FluidControls, sample_rate: f32) {
        let ramp_samples = (LEVEL_RAMP_MS * 0.001 * sample_rate).round() as u32;
        for smoother in &mut self.smoothers {
            let spec = smoother
                .spec
                .expect("registry-derived gain smoothers carry a control spec");
            smoother.set_target((spec.get)(c), ramp_samples);
        }
    }

    pub(crate) fn next_controls(&mut self, c: &FluidControls) -> FluidControls {
        let mut next = c.clone();
        for smoother in &mut self.smoothers {
            let spec = smoother
                .spec
                .expect("registry-derived gain smoothers carry a control spec");
            (spec.set)(&mut next, smoother.next());
        }
        next
    }
}

pub(crate) const TEMPO_SMOOTH_MS: f64 = 180.0;

pub(crate) struct TempoClock {
    pub(crate) beat: f64,
    pub(crate) bpm: f64,
    pub(crate) sample_rate: f64,
    pub(crate) smoothing_coeff: f64,
}

impl TempoClock {
    pub(crate) fn new(sample_rate: f32, bpm: f32) -> Self {
        let sample_rate = f64::from(sample_rate.max(1.0));
        let smoothing_samples = (TEMPO_SMOOTH_MS * 0.001 * sample_rate).max(1.0);
        Self {
            beat: 0.0,
            bpm: f64::from(bpm.clamp(MASTER_BPM_MIN, MASTER_BPM_MAX)),
            sample_rate,
            smoothing_coeff: 1.0 - (-1.0 / smoothing_samples).exp(),
        }
    }

    pub(crate) fn tick(&mut self, target_bpm: f32) -> TimingContext {
        let target_bpm = f64::from(target_bpm.clamp(MASTER_BPM_MIN, MASTER_BPM_MAX));
        self.bpm += (target_bpm - self.bpm) * self.smoothing_coeff;

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
            self.spec = Some(spec);
            match self.next_hit {
                None => {
                    self.next_hit = Some(match self.first_hit {
                        FirstGridHit::AtOrAfterNow => spec.hit_at_or_after(timing.beat),
                        FirstGridHit::AfterNow => spec.hit_after(timing.beat),
                    });
                }
                // Pull the scheduled hit earlier when the new grid lands sooner;
                // a grid that lands later waits until the scheduled hit fires.
                // A modulated grid can therefore never push the target ahead of
                // the playhead indefinitely and starve the trigger.
                Some(hit) => {
                    let candidate = spec.hit_at_or_after(timing.beat);
                    if candidate.beat < hit.beat {
                        self.next_hit = Some(candidate);
                    }
                }
            }
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
// Ambient reverb send
// ============================================================

pub(crate) const AMBIENT_REVERB_DRY_DUCK: f32 = 0.3;
pub(crate) const AMBIENT_REVERB_PAD_SEND: f32 = 0.4;
pub(crate) const AMBIENT_REVERB_TONAL_SEND: f32 = 0.32;
pub(crate) const AMBIENT_REVERB_RETURN: f32 = 0.22;

pub(crate) struct AmbientReverbSend {
    pub(crate) reverb: Freeverb,
}

pub(crate) struct AmbientReverbFrame {
    pub(crate) pad_l: f32,
    pub(crate) pad_r: f32,
    pub(crate) tonal_l: f32,
    pub(crate) tonal_r: f32,
    pub(crate) wet_l: f32,
    pub(crate) wet_r: f32,
}

impl AmbientReverbSend {
    pub(crate) fn new(sample_rate: f32) -> Self {
        Self {
            reverb: Freeverb::new(sample_rate, 0.9, 0.44, 1.0),
        }
    }

    pub(crate) fn process(
        &mut self,
        pad: (f32, f32),
        tonal: (f32, f32),
        pad_mix: f32,
        tonal_mix: f32,
    ) -> AmbientReverbFrame {
        let pad_mix = pad_mix.clamp(0.0, 1.0);
        let tonal_mix = tonal_mix.clamp(0.0, 1.0);
        let pad_dry = Self::dry_gain(pad_mix);
        let tonal_dry = Self::dry_gain(tonal_mix);
        let pad_send = pad_mix * AMBIENT_REVERB_PAD_SEND;
        let tonal_send = tonal_mix * AMBIENT_REVERB_TONAL_SEND;
        let send_l = pad.0 * pad_send + tonal.0 * tonal_send;
        let send_r = pad.1 * pad_send + tonal.1 * tonal_send;
        let (wet_l, wet_r) = self.reverb.process(send_l, send_r);

        AmbientReverbFrame {
            pad_l: pad.0 * pad_dry,
            pad_r: pad.1 * pad_dry,
            tonal_l: tonal.0 * tonal_dry,
            tonal_r: tonal.1 * tonal_dry,
            wet_l: wet_l * AMBIENT_REVERB_RETURN,
            wet_r: wet_r * AMBIENT_REVERB_RETURN,
        }
    }

    pub(crate) fn dry_gain(mix: f32) -> f32 {
        1.0 - mix.clamp(0.0, 1.0) * AMBIENT_REVERB_DRY_DUCK
    }
}

// ============================================================
// Master bus (drive, tilt EQ, compressor)
// ============================================================

pub(crate) struct MasterBus {
    pub(crate) comp_env: f32,
    pub(crate) tone_l: f32,
    pub(crate) tone_r: f32,
    pub(crate) thresh_lin: f32,
    pub(crate) attack_coeff: f32,
    pub(crate) rel_coeff: f32,
}

impl MasterBus {
    pub(crate) fn new(c: &MasterControls, sample_rate: f32) -> Self {
        let mut bus = Self {
            comp_env: 0.0,
            tone_l: 0.0,
            tone_r: 0.0,
            thresh_lin: 1.0,
            attack_coeff: 0.0,
            rel_coeff: 0.0,
        };
        bus.set_controls(c, sample_rate);
        bus
    }

    pub(crate) fn set_controls(&mut self, c: &MasterControls, sample_rate: f32) {
        self.thresh_lin = 10_f32.powf(c.comp_threshold / 20.0);
        self.attack_coeff = (-1.0_f32 / (0.001 * sample_rate)).exp();
        self.rel_coeff = (-1.0_f32 / (c.comp_release_ms * 0.001 * sample_rate)).exp();
    }

    pub(crate) fn process(&mut self, mut l: f32, mut r: f32, c: &MasterControls) -> (f32, f32) {
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

        let peak = l.abs().max(r.abs());
        self.comp_env = if peak > self.comp_env {
            peak + self.attack_coeff * (self.comp_env - peak)
        } else {
            peak + self.rel_coeff * (self.comp_env - peak)
        };
        let gain_reduction = if self.comp_env > self.thresh_lin && c.comp_ratio > 1.001 {
            (self.thresh_lin / self.comp_env)
                * (self.comp_env / self.thresh_lin).powf(1.0 / c.comp_ratio)
        } else {
            1.0
        };

        (
            (l * gain_reduction * c.level).clamp(-0.95, 0.95),
            (r * gain_reduction * c.level).clamp(-0.95, 0.95),
        )
    }
}
