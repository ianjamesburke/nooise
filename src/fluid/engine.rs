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
    pub(crate) arp: ArpEngine,
    pub(crate) ambient_reverb: AmbientReverbSend,
    pub(crate) master_bus: MasterBus,
    pub(crate) controls: Arc<ArcSwap<FluidControls>>,
    pub(crate) automation: Arc<ArcSwap<AutomationState>>,
    /// `Some` only while running `nooise auto`; rewrites `controls` on a
    /// throttled tick so the morph is audible and visible.
    pub(crate) morph: Arc<ArcSwap<Option<MorphState>>>,
    morph_writer: MorphWriter,
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
        morph: Arc<ArcSwap<Option<MorphState>>>,
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
            arp: ArpEngine::new(sample_rate),
            ambient_reverb: AmbientReverbSend::new(sample_rate),
            master_bus: MasterBus::new(&snapshot.master, sample_rate),
            controls,
            automation,
            morph,
            morph_writer: MorphWriter::default(),
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
        self.arp.rng = StdRng::seed_from_u64(seed.wrapping_add(5));
    }
}

impl StereoEngine for FluidEngine {
    fn next_stereo(&mut self) -> (f32, f32) {
        // ~2.9 ms at 44.1 kHz: control edits reach the engine within a frame.
        if self.current_sample.is_multiple_of(128) {
            if let Some(morph) = self.morph.load_full().as_ref()
                && let Some((next_controls, next_automation)) =
                    self.morph_writer.tick(morph, self.tempo.beat)
            {
                self.controls.store(Arc::new(next_controls));
                self.automation.store(Arc::new(next_automation));
            }
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

        let fade = (self.current_sample as f32 / (self.sample_rate * 4.0)).min(1.0);
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
        let (arp_l, arp_r) = self.arp.next(&effective.arp, &effective.pad, tune, timing);
        let AmbientReverbFrame {
            pad_l,
            pad_r,
            tonal_l: ton_l,
            tonal_r: ton_r,
            arp_l,
            arp_r,
            wet_l,
            wet_r,
        } = self.ambient_reverb.process(
            (pad_l, pad_r),
            (ton_l, ton_r),
            (arp_l, arp_r),
            effective.pad.reverb_mix,
            effective.tonal.reverb_mix,
            effective.arp.reverb_mix,
        );

        self.current_sample += 1;

        let raw_l = mix_voices(
            pad_l, perc, kick_l, ton_l, clap_l, bass_l, arp_l, wet_l, fade,
        );
        let raw_r = mix_voices(
            pad_r, perc, kick_r, ton_r, clap_r, bass_r, arp_r, wet_r, fade,
        );
        self.master_bus.process(raw_l, raw_r, &effective.master)
    }
}

#[inline]
// One arg per voice channel plus fade; splitting further would obscure the mix expression.
#[allow(clippy::too_many_arguments)]
fn mix_voices(
    pad: f32,
    perc: f32,
    kick: f32,
    ton: f32,
    clap: f32,
    bass: f32,
    arp: f32,
    wet: f32,
    fade: f32,
) -> f32 {
    (pad + perc * 0.6 + kick * 0.7 + ton + clap * 0.65 + bass * 0.75 + arp + wet) * fade
}

pub(crate) struct GainSmoother {
    pub(crate) spec: Option<&'static ControlSpec>,
    pub(crate) start: f32,
    pub(crate) current: f32,
    pub(crate) target: f32,
    pub(crate) samples_total: u32,
    pub(crate) samples_remaining: u32,
    /// True while the smoother is settled AND its target equals the snapshot
    /// value bit-for-bit, so `next_controls` can skip the per-sample write
    /// (which would be a no-op). Recomputed every `set_targets` call; stays
    /// false when `set_target`'s epsilon guard leaves a sub-epsilon gap
    /// between target and snapshot, where the write is load-bearing.
    pub(crate) idle: bool,
}

impl GainSmoother {
    #[cfg(test)]
    pub(crate) fn new(value: f32) -> Self {
        Self::for_spec(None, value)
    }

    pub(crate) fn for_spec(spec: Option<&'static ControlSpec>, value: f32) -> Self {
        Self {
            spec,
            start: value,
            current: value,
            target: value,
            samples_total: 0,
            samples_remaining: 0,
            idle: false,
        }
    }

    pub(crate) fn set_target(&mut self, target: f32, ramp_samples: u32) {
        if (target - self.target).abs() <= f32::EPSILON {
            return;
        }
        self.start = self.current;
        self.target = target;
        self.samples_total = ramp_samples.max(1);
        self.samples_remaining = self.samples_total;
    }

    pub(crate) fn next(&mut self) -> f32 {
        if self.samples_remaining == 0 {
            self.current = self.target;
            return self.current;
        }
        let elapsed = self.samples_total - self.samples_remaining + 1;
        let t = elapsed as f32 / self.samples_total as f32;
        let eased = t * t * (3.0 - 2.0 * t);
        self.current = self.start + (self.target - self.start) * eased;
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
            let snapshot_value = (spec.get)(c);
            smoother.set_target(snapshot_value, ramp_samples);
            smoother.idle = smoother.samples_remaining == 0 && smoother.target == snapshot_value;
        }
    }

    pub(crate) fn next_controls(&mut self, c: &FluidControls) -> FluidControls {
        let mut next = c.clone();
        for smoother in &mut self.smoothers {
            if smoother.idle {
                continue;
            }
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

// `sample_rate`/`bpm` and the beats<->samples conversions below are only
// exercised by tests that need production timing math to compute expected
// values (e.g. arp step spacing) — genuinely unread outside `cfg(test)`.
#[derive(Clone, Copy)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct TimingContext {
    pub(crate) sample_rate: f64,
    pub(crate) bpm: f64,
    pub(crate) beat: f64,
}

#[cfg_attr(not(test), allow(dead_code))]
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

/// Only grids at or below this interval (one beat) swing; slower chord-rate
/// grids stay straight, so a progression never lands off the downbeat.
const SWING_MAX_INTERVAL_BEATS: f64 = 1.0;
/// A full (100%) swing delays each off-slot by half its interval — the hardest
/// shuffle that still keeps slots strictly ordered.
const SWING_MAX_FRACTION: f64 = 0.5;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GridSpec {
    pub(crate) interval_beats: f64,
    pub(crate) offset_beats: f64,
    /// Beats each odd slot is pushed late; 0 on straight or chord-rate grids.
    swing_delay_beats: f64,
}

impl GridSpec {
    pub(crate) fn new(interval_beats: f32, offset_beats: f32, swing: f32) -> Self {
        let interval_beats = f64::from(interval_beats).max(1.0 / 64.0);
        let swing_fraction = if interval_beats <= SWING_MAX_INTERVAL_BEATS {
            f64::from(swing.clamp(0.0, 1.0)) * SWING_MAX_FRACTION
        } else {
            0.0
        };
        Self {
            interval_beats,
            offset_beats: f64::from(offset_beats).rem_euclid(interval_beats),
            swing_delay_beats: swing_fraction * interval_beats,
        }
    }

    /// Beat of grid slot `slot`, with odd slots pushed late by the swing delay.
    /// Strictly increasing in `slot` since the delay is always < one interval.
    fn swung_beat(self, slot: u64) -> f64 {
        let base = self.offset_beats + slot as f64 * self.interval_beats;
        if slot % 2 == 1 {
            base + self.swing_delay_beats
        } else {
            base
        }
    }

    pub(crate) fn hit_at_or_after(self, beat: f64) -> GridHit {
        if beat <= self.offset_beats {
            return GridHit {
                beat: self.offset_beats,
            };
        }
        // Straight-grid estimate, then walk forward to the first swung slot at
        // or after `beat`. Swing moves a slot by less than one interval, so the
        // true slot is at most one past the estimate — a handful of iterations.
        let est = ((beat - self.offset_beats) / self.interval_beats)
            .floor()
            .max(0.0) as u64;
        let mut slot = est.saturating_sub(1);
        loop {
            let hit = self.swung_beat(slot);
            if hit >= beat {
                return GridHit { beat: hit };
            }
            slot += 1;
        }
    }

    pub(crate) fn hit_after(self, beat: f64) -> GridHit {
        self.hit_at_or_after(beat + GRID_BEAT_EPSILON)
    }
}

#[cfg(test)]
mod grid_swing_tests {
    use super::*;

    #[test]
    fn straight_grid_hits_land_on_even_subdivisions() {
        let grid = GridSpec::new(0.5, 0.0, 0.0);
        assert_eq!(grid.hit_at_or_after(0.0).beat, 0.0);
        assert_eq!(grid.hit_at_or_after(0.1).beat, 0.5);
        assert_eq!(grid.hit_at_or_after(0.5).beat, 0.5);
        assert_eq!(grid.hit_at_or_after(0.6).beat, 1.0);
    }

    #[test]
    fn swing_delays_odd_slots_only_and_stays_ordered() {
        // 0.5-beat grid, full swing: odd slots pushed by (1.0 * 0.5) * 0.5 = 0.25.
        let grid = GridSpec::new(0.5, 0.0, 1.0);
        assert_eq!(grid.hit_at_or_after(0.0).beat, 0.0); // slot 0 (even) straight
        assert!((grid.hit_at_or_after(0.1).beat - 0.75).abs() < 1e-9); // slot 1 pushed late
        assert_eq!(grid.hit_at_or_after(0.8).beat, 1.0); // slot 2 (even) straight
        // Never reorders: consecutive hits are strictly increasing.
        assert!(grid.hit_at_or_after(0.0).beat < grid.hit_at_or_after(0.1).beat);
        assert!(grid.hit_at_or_after(0.1).beat < grid.hit_at_or_after(0.8).beat);
    }

    #[test]
    fn chord_rate_grids_never_swing() {
        // Interval above the subdivision threshold: swing is ignored entirely.
        let straight = GridSpec::new(4.0, 0.0, 0.0);
        let asked_to_swing = GridSpec::new(4.0, 0.0, 1.0);
        assert_eq!(straight, asked_to_swing);
        assert_eq!(asked_to_swing.hit_at_or_after(4.1).beat, 8.0);
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
    /// Beat of the most recently emitted hit. A live grid reshape (rate/offset/
    /// swing change) may never reschedule the next hit within half an interval
    /// of this — the guard that stops a timing tweak from re-firing the slot
    /// that just sounded (an audible double-trigger / flam).
    last_hit_beat: Option<f64>,
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
            last_hit_beat: None,
        }
    }

    pub(crate) fn pop(
        &mut self,
        timing: TimingContext,
        interval_beats: f32,
        offset_beats: f32,
    ) -> bool {
        self.pop_swung(timing, interval_beats, offset_beats, 0.0)
    }

    /// Earliest beat the next hit may occupy: at or after the playhead, and
    /// never within half an interval of the hit already emitted. A live reshape
    /// (swing/offset/rate) moves any slot by at most half an interval, so this
    /// floor is what stops the just-played slot from being scheduled again.
    fn earliest_hit(&self, spec: GridSpec, beat: f64) -> f64 {
        let floor = self
            .last_hit_beat
            .map_or(f64::NEG_INFINITY, |b| b + spec.interval_beats * 0.5);
        (beat + GRID_BEAT_EPSILON).max(floor)
    }

    /// Like `pop`, but this voice's grid swings its odd subdivisions by
    /// `swing` (0 straight .. 1 max shuffle). Only voices that opt in call this.
    pub(crate) fn pop_swung(
        &mut self,
        timing: TimingContext,
        interval_beats: f32,
        offset_beats: f32,
        swing: f32,
    ) -> bool {
        let spec = GridSpec::new(interval_beats, offset_beats, swing);
        if self.spec != Some(spec) {
            self.spec = Some(spec);
            match self.next_hit {
                None => {
                    self.next_hit = Some(match self.first_hit {
                        FirstGridHit::AtOrAfterNow => spec.hit_at_or_after(timing.beat),
                        FirstGridHit::AfterNow => spec.hit_after(timing.beat),
                    });
                }
                // Pull the scheduled hit earlier when the reshaped grid lands
                // sooner, so a denser grid isn't starved — but never earlier than
                // `earliest_hit`, which rejects a re-fire of the slot that just
                // sounded while still admitting the genuinely-next denser slot.
                Some(hit) => {
                    let candidate = spec.hit_at_or_after(self.earliest_hit(spec, timing.beat));
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
            self.last_hit_beat = Some(next_hit.beat);
            self.next_hit = Some(spec.hit_at_or_after(self.earliest_hit(spec, timing.beat)));
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
/// Arp's send level stays fixed (unlike its mix, `arp.reverb_mix`, which is a
/// user-facing control) — tuned in the same range as Tonal's default send.
pub(crate) const AMBIENT_REVERB_ARP_SEND: f32 = 0.3;

pub(crate) struct AmbientReverbSend {
    pub(crate) reverb: Freeverb,
}

pub(crate) struct AmbientReverbFrame {
    pub(crate) pad_l: f32,
    pub(crate) pad_r: f32,
    pub(crate) tonal_l: f32,
    pub(crate) tonal_r: f32,
    pub(crate) arp_l: f32,
    pub(crate) arp_r: f32,
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
        arp: (f32, f32),
        pad_mix: f32,
        tonal_mix: f32,
        arp_mix: f32,
    ) -> AmbientReverbFrame {
        let pad_mix = pad_mix.clamp(0.0, 1.0);
        let tonal_mix = tonal_mix.clamp(0.0, 1.0);
        let arp_mix = arp_mix.clamp(0.0, 1.0);
        let pad_dry = Self::dry_gain(pad_mix);
        let tonal_dry = Self::dry_gain(tonal_mix);
        let arp_dry = Self::dry_gain(arp_mix);
        let pad_send = pad_mix * AMBIENT_REVERB_PAD_SEND;
        let tonal_send = tonal_mix * AMBIENT_REVERB_TONAL_SEND;
        let arp_send = arp_mix * AMBIENT_REVERB_ARP_SEND;
        let send_l = pad.0 * pad_send + tonal.0 * tonal_send + arp.0 * arp_send;
        let send_r = pad.1 * pad_send + tonal.1 * tonal_send + arp.1 * arp_send;
        let (wet_l, wet_r) = self.reverb.process(send_l, send_r);

        AmbientReverbFrame {
            pad_l: pad.0 * pad_dry,
            pad_r: pad.1 * pad_dry,
            tonal_l: tonal.0 * tonal_dry,
            tonal_r: tonal.1 * tonal_dry,
            arp_l: arp.0 * arp_dry,
            arp_r: arp.1 * arp_dry,
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
