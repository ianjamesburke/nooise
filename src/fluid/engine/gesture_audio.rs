//! Fixed per-layer processors for temporary live gesture audio.

use super::*;

const SUBMERGE_MIN_CUTOFF_HZ: f32 = 420.0;
const SUBMERGE_OPEN_CUTOFF_HZ: f32 = 8_000.0;
const LIFT_MIN_CUTOFF_HZ: f32 = 110.0;
const LIFT_MAX_CUTOFF_HZ: f32 = 1_800.0;
const BLOOM_SEND_GAIN: f32 = 0.72;
const BLOOM_TAIL_SECONDS: f32 = 8.0;
const ECHO_SEND_GAIN: f32 = 0.68;
const ECHO_TAIL_SECONDS: f32 = 6.0;
const THIN_PEAK_DB: f32 = -12.0;
const PROCESSOR_CLEAR_SAMPLES_PER_FRAME: usize = 512;
const ECHO_FEEDBACK_SECONDS: f32 = 0.03;

struct GestureLayerFx {
    submerge: SlotFx,
    lift: SlotFx,
    bloom: SlotFx,
    echo: SlotFx,
    submerge_active: bool,
    lift_active: bool,
    bloom_has_history: bool,
    echo_has_history: bool,
    bloom_tail_samples: u32,
    echo_tail_samples: u32,
    bloom_clear_cursor: usize,
    echo_clear_cursor: usize,
    echo_feedback: f32,
    echo_feedback_target: f32,
    echo_feedback_alpha: f32,
    previous_echo_amount: f32,
}

impl GestureLayerFx {
    fn new(sample_rate: f32, max_delay_samples: usize) -> Self {
        Self {
            submerge: SlotFx::Filter(StereoFilter::default()),
            lift: SlotFx::Filter(StereoFilter::default()),
            bloom: SlotFx::Reverb(Freeverb::new(sample_rate)),
            echo: SlotFx::Delay(StereoDelay::new(max_delay_samples)),
            submerge_active: false,
            lift_active: false,
            bloom_has_history: false,
            echo_has_history: false,
            bloom_tail_samples: 0,
            echo_tail_samples: 0,
            bloom_clear_cursor: 0,
            echo_clear_cursor: 0,
            echo_feedback: 0.25,
            echo_feedback_target: 0.25,
            echo_feedback_alpha: 1.0 - (-1.0 / (ECHO_FEEDBACK_SECONDS * sample_rate)).exp(),
            previous_echo_amount: 0.0,
        }
    }

    fn process(
        &mut self,
        sample: (f32, f32),
        amounts: [f32; GESTURE_COUNT],
        timing: TimingContext,
        max_delay_samples: usize,
        sample_rate: f32,
    ) -> (f32, f32) {
        if amounts == [0.0; GESTURE_COUNT]
            && !self.submerge_active
            && !self.lift_active
            && !self.bloom_has_history
            && !self.echo_has_history
        {
            return sample;
        }

        let thin_amount = amounts[GestureKind::Thin as usize];
        let thin_gain = if thin_amount > f32::EPSILON {
            10.0_f32.powf(THIN_PEAK_DB * thin_amount / 20.0)
        } else {
            1.0
        };
        let mut source = (sample.0 * thin_gain, sample.1 * thin_gain);

        let submerge_amount = amounts[GestureKind::Submerge as usize];
        if submerge_amount > f32::EPSILON {
            self.submerge_active = true;
            let cutoff_hz = SUBMERGE_OPEN_CUTOFF_HZ
                * (SUBMERGE_MIN_CUTOFF_HZ / SUBMERGE_OPEN_CUTOFF_HZ).powf(submerge_amount);
            let slot = ModuleSlot {
                amount: 1.0,
                time: cutoff_hz,
                right_time: 0.08,
                feedback: 0.0,
                ..ModuleSlot::default()
            };
            source = ModuleFxBank::process_slot_fx(
                &mut self.submerge,
                &slot,
                source,
                timing,
                max_delay_samples,
                sample_rate,
            );
            source = mix_stereo(
                (sample.0 * thin_gain, sample.1 * thin_gain),
                source,
                submerge_amount,
            );
        } else if self.submerge_active {
            self.submerge = SlotFx::Filter(StereoFilter::default());
            self.submerge_active = false;
        }

        let lift_amount = amounts[GestureKind::Lift as usize];
        if lift_amount > f32::EPSILON {
            self.lift_active = true;
            let cutoff_hz =
                LIFT_MIN_CUTOFF_HZ + (LIFT_MAX_CUTOFF_HZ - LIFT_MIN_CUTOFF_HZ) * lift_amount;
            let slot = ModuleSlot {
                amount: 1.0,
                time: cutoff_hz,
                right_time: 0.05,
                feedback: 1.0,
                ..ModuleSlot::default()
            };
            let filtered = ModuleFxBank::process_slot_fx(
                &mut self.lift,
                &slot,
                source,
                timing,
                max_delay_samples,
                sample_rate,
            );
            source = mix_stereo(source, filtered, lift_amount);
        } else if self.lift_active {
            self.lift = SlotFx::Filter(StereoFilter::default());
            self.lift_active = false;
        }

        let bloom_amount = amounts[GestureKind::Bloom as usize];
        let bloom_input = scale(source, bloom_amount * BLOOM_SEND_GAIN);
        let bloom_return = self.process_bloom(
            bloom_input,
            bloom_amount,
            timing,
            max_delay_samples,
            sample_rate,
        );

        let echo_amount = amounts[GestureKind::Echo as usize];
        let echo_input = scale(source, echo_amount * ECHO_SEND_GAIN);
        let echo_return = self.process_echo(
            echo_input,
            echo_amount,
            timing,
            max_delay_samples,
            sample_rate,
        );

        (
            source.0 + bloom_return.0 + echo_return.0,
            source.1 + bloom_return.1 + echo_return.1,
        )
    }

    fn process_bloom(
        &mut self,
        input: (f32, f32),
        amount: f32,
        timing: TimingContext,
        max_delay_samples: usize,
        sample_rate: f32,
    ) -> (f32, f32) {
        if amount > f32::EPSILON {
            self.bloom_has_history = true;
            self.bloom_clear_cursor = 0;
            self.bloom_tail_samples = tail_samples(sample_rate, BLOOM_TAIL_SECONDS);
        }
        if !self.bloom_has_history {
            return (0.0, 0.0);
        }
        if amount <= f32::EPSILON && self.bloom_tail_samples == 0 {
            if let SlotFx::Reverb(reverb) = &mut self.bloom
                && reverb.clear_chunk(
                    &mut self.bloom_clear_cursor,
                    PROCESSOR_CLEAR_SAMPLES_PER_FRAME,
                )
            {
                self.bloom_has_history = false;
                self.bloom_clear_cursor = 0;
            }
            return (0.0, 0.0);
        }

        let slot = ModuleSlot {
            amount: 1.0,
            time: 0.88,
            feedback: 0.58,
            ..ModuleSlot::default()
        };
        let processed = ModuleFxBank::process_slot_fx(
            &mut self.bloom,
            &slot,
            input,
            timing,
            max_delay_samples,
            sample_rate,
        );
        let weight = if amount > f32::EPSILON {
            1.0
        } else {
            let weight = tail_weight(self.bloom_tail_samples, sample_rate);
            self.bloom_tail_samples -= 1;
            weight
        };
        scale(subtract(processed, input), weight)
    }

    fn clear_echo(&mut self) {
        if let SlotFx::Delay(delay) = &mut self.echo
            && delay.clear_chunk(
                &mut self.echo_clear_cursor,
                PROCESSOR_CLEAR_SAMPLES_PER_FRAME,
            )
        {
            self.echo_has_history = false;
            self.echo_clear_cursor = 0;
            self.echo_feedback = 0.25;
            self.echo_feedback_target = 0.25;
            self.previous_echo_amount = 0.0;
        }
    }

    fn process_echo(
        &mut self,
        input: (f32, f32),
        amount: f32,
        timing: TimingContext,
        max_delay_samples: usize,
        sample_rate: f32,
    ) -> (f32, f32) {
        if amount > f32::EPSILON {
            self.echo_has_history = true;
            self.echo_clear_cursor = 0;
            self.echo_tail_samples = tail_samples(sample_rate, ECHO_TAIL_SECONDS);
            if amount > self.previous_echo_amount {
                self.echo_feedback_target = 0.25 + amount * 0.37;
            }
        }
        if !self.echo_has_history {
            return (0.0, 0.0);
        }
        self.previous_echo_amount = amount;
        if amount <= f32::EPSILON && self.echo_tail_samples == 0 {
            self.clear_echo();
            return (0.0, 0.0);
        }
        self.echo_feedback +=
            (self.echo_feedback_target - self.echo_feedback) * self.echo_feedback_alpha;

        let slot = ModuleSlot {
            amount: 1.0,
            time: 250.0,
            right_time: 375.0,
            clock: DelayClock::Free.value(),
            right_clock: DelayClock::Free.value(),
            feedback: self.echo_feedback,
            vintage: 0.12,
            ..ModuleSlot::default()
        };
        let processed = ModuleFxBank::process_slot_fx(
            &mut self.echo,
            &slot,
            input,
            timing,
            max_delay_samples,
            sample_rate,
        );
        let weight = if amount > f32::EPSILON {
            1.0
        } else {
            let weight = tail_weight(self.echo_tail_samples, sample_rate);
            self.echo_tail_samples -= 1;
            weight
        };
        scale(subtract(processed, input), weight)
    }
}

fn tail_samples(sample_rate: f32, seconds: f32) -> u32 {
    ((seconds + LEVEL_RAMP_MS * 0.001) * sample_rate).round() as u32
}

fn tail_weight(samples_remaining: u32, sample_rate: f32) -> f32 {
    let fade_samples = (LEVEL_RAMP_MS * 0.001 * sample_rate).round().max(1.0) as u32;
    if samples_remaining >= fade_samples {
        1.0
    } else {
        smoothstep(samples_remaining as f32 / fade_samples as f32)
    }
}

fn scale(sample: (f32, f32), gain: f32) -> (f32, f32) {
    (sample.0 * gain, sample.1 * gain)
}

fn subtract(sample: (f32, f32), dry: (f32, f32)) -> (f32, f32) {
    (sample.0 - dry.0, sample.1 - dry.1)
}

pub(super) struct GestureAudioBank {
    layers: [GestureLayerFx; TAB_COUNT],
    max_delay_samples: usize,
    sample_rate: f32,
}

impl GestureAudioBank {
    pub(super) fn new(sample_rate: f32) -> Self {
        let max_delay_samples = (sample_rate * (DELAY_FREE_MAX_MS / 1_000.0)).ceil() as usize;
        Self {
            layers: std::array::from_fn(|_| GestureLayerFx::new(sample_rate, max_delay_samples)),
            max_delay_samples,
            sample_rate,
        }
    }

    pub(super) fn process(
        &mut self,
        tab: Tab,
        sample: (f32, f32),
        amounts: [f32; GESTURE_COUNT],
        timing: TimingContext,
    ) -> (f32, f32) {
        self.layers[tab as usize].process(
            sample,
            amounts,
            timing,
            self.max_delay_samples,
            self.sample_rate,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_RATE: f32 = 48_000.0;

    fn timing() -> TimingContext {
        TimingContext::new(SAMPLE_RATE as f64, 120.0, 0.0)
    }

    fn active(kind: GestureKind) -> [f32; GESTURE_COUNT] {
        let mut amounts = [0.0; GESTURE_COUNT];
        amounts[kind as usize] = 1.0;
        amounts
    }

    #[test]
    fn zero_submerge_is_an_exact_dry_bypass() {
        let mut bank = GestureAudioBank::new(SAMPLE_RATE);
        let input = (0.4, -0.2);

        assert_eq!(
            bank.process(Tab::Chords, input, [0.0; GESTURE_COUNT], timing()),
            input
        );
    }

    #[test]
    fn full_submerge_audibly_removes_high_frequency_alternation() {
        let mut bank = GestureAudioBank::new(SAMPLE_RATE);
        let mut energy = 0.0;
        for index in 0..512 {
            let input = if index % 2 == 0 {
                (1.0, 1.0)
            } else {
                (-1.0, -1.0)
            };
            let output = bank.process(Tab::Chords, input, active(GestureKind::Submerge), timing());
            energy += output.0.abs() + output.1.abs();
        }

        assert!(
            energy < 32.0,
            "submerge left high-frequency energy at {energy}"
        );
    }

    #[test]
    fn full_lift_audibly_removes_low_frequency_content() {
        let mut bank = GestureAudioBank::new(SAMPLE_RATE);
        let mut low_energy = 0.0;
        for index in 0..SAMPLE_RATE as usize {
            let phase = index as f32 * std::f32::consts::TAU * 80.0 / SAMPLE_RATE;
            let output = bank.process(
                Tab::Chords,
                (phase.sin(), phase.sin()),
                active(GestureKind::Lift),
                timing(),
            );
            low_energy += output.0.abs() + output.1.abs();
        }

        assert!(
            low_energy < 400.0,
            "lift left low-frequency energy at {low_energy}"
        );
    }

    #[test]
    fn submerge_onset_is_continuous_from_exact_dry() {
        let mut bank = GestureAudioBank::new(SAMPLE_RATE);
        let input = (0.8, -0.6);
        let mut amounts = [0.0; GESTURE_COUNT];
        amounts[GestureKind::Submerge as usize] = 1e-6;

        let output = bank.process(Tab::Perc, input, amounts, timing());

        assert!(
            (output.0 - input.0).abs() < 2e-6 && (output.1 - input.1).abs() < 2e-6,
            "Submerge onset jumped from {input:?} to {output:?}"
        );
    }

    #[test]
    fn submerge_release_resets_filter_history_before_repress() {
        let mut used = GestureAudioBank::new(SAMPLE_RATE);
        for frame in 0..512 {
            let sample = if frame % 2 == 0 {
                (1.0, 1.0)
            } else {
                (-1.0, -1.0)
            };
            used.process(Tab::Kick, sample, active(GestureKind::Submerge), timing());
        }
        let input = (0.3, -0.2);
        assert_eq!(
            used.process(Tab::Kick, input, [0.0; GESTURE_COUNT], timing()),
            input
        );
        let mut shallow = [0.0; GESTURE_COUNT];
        shallow[GestureKind::Submerge as usize] = 0.001;
        let repressed = used.process(Tab::Kick, input, shallow, timing());
        let mut fresh = GestureAudioBank::new(SAMPLE_RATE);

        let clean_start = fresh.process(Tab::Kick, input, shallow, timing());

        assert_eq!(repressed, clean_start);
    }

    #[test]
    fn full_thin_reduces_the_target_by_twelve_decibels() {
        let mut bank = GestureAudioBank::new(SAMPLE_RATE);

        let output = bank.process(Tab::Kick, (1.0, -1.0), active(GestureKind::Thin), timing());

        let expected = 10.0_f32.powf(THIN_PEAK_DB / 20.0);
        assert!((output.0 - expected).abs() < 1e-6);
    }

    #[test]
    fn bloom_adds_a_parallel_reverb_return_without_removing_dry() {
        let mut bank = GestureAudioBank::new(SAMPLE_RATE);
        let mut changed = false;
        for frame in 0..8_000 {
            let input = if frame == 0 { (1.0, 1.0) } else { (0.0, 0.0) };
            let output = bank.process(Tab::Chords, input, active(GestureKind::Bloom), timing());
            changed |= output != input;
        }

        assert!(changed, "Bloom produced no reverb return");
    }

    #[test]
    fn echo_adds_a_delayed_parallel_return() {
        let mut bank = GestureAudioBank::new(SAMPLE_RATE);
        let mut changed = false;
        for frame in 0..24_000 {
            let input = if frame == 0 { (1.0, -1.0) } else { (0.0, 0.0) };
            let output = bank.process(Tab::Lead, input, active(GestureKind::Echo), timing());
            changed |= output != input;
        }

        assert!(changed, "Echo produced no delayed return");
    }

    #[test]
    fn bloom_tail_survives_after_the_send_returns_to_zero() {
        let mut bank = GestureAudioBank::new(SAMPLE_RATE);
        for _ in 0..8_000 {
            bank.process(Tab::Tonal, (0.5, 0.5), active(GestureKind::Bloom), timing());
        }

        let tail_is_audible = (0..2_000).any(|_| {
            bank.process(Tab::Tonal, (0.0, 0.0), [0.0; GESTURE_COUNT], timing()) != (0.0, 0.0)
        });

        assert!(tail_is_audible, "Bloom release cut its reverb tail");
    }

    #[test]
    fn echo_tail_survives_after_the_send_returns_to_zero() {
        let mut bank = GestureAudioBank::new(SAMPLE_RATE);
        for _ in 0..20_000 {
            bank.process(Tab::Lead, (0.4, -0.4), active(GestureKind::Echo), timing());
        }

        let tail_is_audible = (0..20_000).any(|_| {
            bank.process(Tab::Lead, (0.0, 0.0), [0.0; GESTURE_COUNT], timing()) != (0.0, 0.0)
        });

        assert!(tail_is_audible, "Echo release cut its delay tail");
    }

    #[test]
    fn overlapping_gestures_across_every_target_keep_the_stage_bounded() {
        let mut bank = GestureAudioBank::new(SAMPLE_RATE);
        let mut peak = 0.0_f32;
        for frame in 0..12_000 {
            let input = (((frame as f32 * 0.07).sin() * 0.25), 0.0);
            for tab in Tab::all() {
                let output = bank.process(tab, input, [1.0; GESTURE_COUNT], timing());
                peak = peak.max(output.0.abs()).max(output.1.abs());
            }
        }

        assert!(
            peak.is_finite() && peak < 4.0,
            "gesture stage peak was {peak}"
        );
    }

    #[test]
    fn tail_retirement_fades_smoothly_to_silence() {
        let fade_samples = (LEVEL_RAMP_MS * 0.001 * SAMPLE_RATE).round() as u32;
        let halfway = tail_weight(fade_samples / 2, SAMPLE_RATE);
        let last = tail_weight(1, SAMPLE_RATE);

        assert!(1.0 > halfway && halfway > last && last > 0.0);
    }

    #[test]
    fn drained_tails_clear_incrementally_then_restore_exact_dry_bypass() {
        let mut bank = GestureAudioBank::new(SAMPLE_RATE);
        bank.process(
            Tab::Master,
            (0.5, -0.5),
            [1.0, 0.0, 1.0, 0.0, 0.0],
            timing(),
        );
        bank.layers[Tab::Master as usize].bloom_tail_samples = 0;
        bank.layers[Tab::Master as usize].echo_tail_samples = 0;
        for _ in 0..1_000 {
            bank.process(Tab::Master, (0.0, 0.0), [0.0; GESTURE_COUNT], timing());
            let layer = &bank.layers[Tab::Master as usize];
            if !layer.bloom_has_history && !layer.echo_has_history {
                break;
            }
        }
        let layer = &bank.layers[Tab::Master as usize];
        assert!(!layer.bloom_has_history && !layer.echo_has_history);

        let input = (0.31, -0.27);
        assert_eq!(
            bank.process(Tab::Master, input, [0.0; GESTURE_COUNT], timing()),
            input
        );
    }

    #[test]
    fn repress_during_incremental_clear_accepts_the_new_send() {
        let mut bank = GestureAudioBank::new(SAMPLE_RATE);
        bank.process(Tab::Lead, (1.0, -1.0), active(GestureKind::Echo), timing());
        bank.layers[Tab::Lead as usize].echo_tail_samples = 0;
        for _ in 0..8 {
            bank.process(Tab::Lead, (0.0, 0.0), [0.0; GESTURE_COUNT], timing());
        }
        assert!(bank.layers[Tab::Lead as usize].echo_clear_cursor > 0);
        let mut shallow = [0.0; GESTURE_COUNT];
        shallow[GestureKind::Echo as usize] = 0.001;
        bank.process(Tab::Lead, (1.0, -1.0), shallow, timing());

        let heard_new_send = (0..20_000).any(|_| {
            bank.process(Tab::Lead, (0.0, 0.0), [0.0; GESTURE_COUNT], timing()) != (0.0, 0.0)
        });

        assert!(
            heard_new_send,
            "repress lost the new Echo send while clearing"
        );
        assert_eq!(bank.layers[Tab::Lead as usize].echo_clear_cursor, 0);
    }

    #[test]
    fn every_voice_and_master_have_an_independent_gesture_stage() {
        let mut bank = GestureAudioBank::new(SAMPLE_RATE);
        for tab in Tab::all() {
            let output = bank.process(tab, (1.0, -1.0), active(GestureKind::Thin), timing());
            assert_ne!(output, (1.0, -1.0), "Thin was inert on {}", tab.name());
        }
    }

    #[test]
    fn full_user_slot_bank_does_not_block_the_temporary_stage() {
        let mut user_bank = ModuleFxBank::new(SAMPLE_RATE);
        let user_slots = [preset_slot("drive", 0.2); MODULE_SLOTS];
        let after_user_slots = user_bank.process(Tab::Perc, &user_slots, (0.4, -0.2), timing());
        let mut gesture_bank = GestureAudioBank::new(SAMPLE_RATE);

        let output = gesture_bank.process(
            Tab::Perc,
            after_user_slots,
            active(GestureKind::Thin),
            timing(),
        );

        assert_ne!(output, after_user_slots);
    }

    #[test]
    fn repeated_target_changes_reuse_the_fixed_processor_storage() {
        let mut bank = GestureAudioBank::new(SAMPLE_RATE);
        let identities = bank.layers.each_ref().map(|layer| {
            [
                std::ptr::from_ref(&layer.submerge) as usize,
                std::ptr::from_ref(&layer.bloom) as usize,
                std::ptr::from_ref(&layer.echo) as usize,
            ]
        });

        for pass in 0..128 {
            for tab in Tab::all() {
                let kind = GestureKind::ALL[(pass + tab as usize) % GESTURE_COUNT];
                bank.process(tab, (0.1, -0.1), active(kind), timing());
                bank.process(tab, (0.0, 0.0), [0.0; GESTURE_COUNT], timing());
            }
        }

        let after = bank.layers.each_ref().map(|layer| {
            [
                std::ptr::from_ref(&layer.submerge) as usize,
                std::ptr::from_ref(&layer.bloom) as usize,
                std::ptr::from_ref(&layer.echo) as usize,
            ]
        });
        assert_eq!(after, identities);
    }
}
