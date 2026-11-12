//! `FluidEngine`: the voice mixer that the audio callback pulls frames from.
//!
//! Owns the tempo clock and its `TimingContext`, the grid triggers voices fire
//! on, registry-derived control smoothing, the per-layer and master module effect
//! banks, and the master bus.

use std::collections::BTreeSet;

use crate::fluid::osc::{OscVoiceCommand, OscVoiceSink, OscVoiceSource};
use crate::fx::compression::{CompressorParams, StereoCompressor};
use crate::fx::crossfade::{Outgoing, mix_stereo};
use crate::fx::delay::{DelayParams, StereoDelay};
use crate::fx::drive::{DriveParams, DriveType, StereoDrive};
use crate::fx::filter::{FilterParams, FilterType, StereoFilter};
use crate::fx::reverb::{FdnReverb, ReverbParams};

use super::*;

#[cfg(test)]
mod downbeat_tests;
mod gesture_audio;
#[cfg(test)]
mod perf_probe;

use gesture_audio::GestureAudioBank;

const STARTUP_FADE_SECONDS: f32 = 2.0;

/// Time for a compressor's displayed gain reduction to fall to 1/e once the
/// compressor lets go: slow enough to read at UI frame rate, fast enough that
/// the meter follows each kick.
const GAIN_REDUCTION_RELEASE_SECONDS: f32 = 0.3;
/// Time for a voice or Master RMS meter to fall to 1/e after a peak.
const LEVEL_METER_RELEASE_SECONDS: f32 = 0.25;

/// Stateful post-synthesis Delay instances, addressed by the stable
/// `(layer, slot)` storage identity rather than a catalog name.
enum SlotFx {
    Delay(StereoDelay),
    Reverb(FdnReverb),
    Compression(StereoCompressor),
    Filter(StereoFilter),
    Drive(StereoDrive),
}

impl SlotFx {
    fn family(&self) -> Family {
        match self {
            Self::Delay(_) => Family::Delay,
            Self::Reverb(_) => Family::Reverb,
            Self::Compression(_) => Family::Compression,
            Self::Filter(_) => Family::Filter,
            Self::Drive(_) => Family::Drive,
        }
    }
}

/// A processor whose slot no longer wants it — the module was removed, or
/// replaced by one of another family.
///
/// Dropping it outright cuts whatever it was producing to zero in a single
/// sample, which on a Reverb or Delay holding a live tail is a loud click.
/// Instead it keeps running on the live input while its contribution
/// crossfades back to the dry signal, and only then is it dropped. `slot` is
/// the slot's field values captured at the moment it was retired, since the
/// live slot has already moved on to whatever replaced it.
struct RetiredFx {
    fx: SlotFx,
    slot: ModuleSlot,
}

struct ModuleFxBank {
    slots: [Option<SlotFx>; MODULE_SLOT_TOTAL],
    retiring: [Option<Outgoing<RetiredFx>>; MODULE_SLOT_TOTAL],
    /// Each slot's field values from the last frame it was loaded. A retiring
    /// processor keeps running on these, not on the live slot: by the time a
    /// removal is noticed the live slot has already been cleared, and its
    /// zeroed Amount would silence the very tail being faded out.
    last_loaded: [ModuleSlot; MODULE_SLOT_TOTAL],
    /// Lowest linear gain each slot's compressor applied since the last
    /// meter publish; 1.0 for a slot that reduced nothing or holds no
    /// compressor.
    gain_floor: [f32; MODULE_SLOT_TOTAL],
    /// Each slot's displayed gain reduction in dB: instant attack, then an
    /// exponential release, so a meter read once per UI frame neither
    /// flickers nor misses a transient.
    gain_reduction_db: [f32; MODULE_SLOT_TOTAL],
    max_delay_samples: usize,
    sample_rate: f32,
    retire_fade_samples: f32,
}

impl ModuleFxBank {
    fn new(sample_rate: f32) -> Self {
        Self {
            slots: std::array::from_fn(|_| None),
            retiring: std::array::from_fn(|_| None),
            last_loaded: [ModuleSlot::default(); MODULE_SLOT_TOTAL],
            gain_floor: [1.0; MODULE_SLOT_TOTAL],
            gain_reduction_db: [0.0; MODULE_SLOT_TOTAL],
            max_delay_samples: (sample_rate * (DELAY_FREE_MAX_MS / 1_000.0)).ceil() as usize,
            sample_rate,
            // The same window every other click-free level change in the
            // engine uses, so a module leaving sounds like any other gain
            // change rather than its own event.
            retire_fade_samples: LEVEL_RAMP_MS * 0.001 * sample_rate,
        }
    }

    /// One sample through one loaded processor, dispatched on the processor
    /// itself rather than on the slot's requested kind — a retiring processor
    /// outlives the slot's claim on it.
    fn process_slot_fx(
        fx: &mut SlotFx,
        slot: &ModuleSlot,
        sample: (f32, f32),
        timing: TimingContext,
        max_delay_samples: usize,
        sample_rate: f32,
    ) -> (f32, f32) {
        match fx {
            SlotFx::Delay(line) => {
                let left = delay_time_ms(
                    slot.time,
                    DelayClock::from_value(slot.clock),
                    timing.bpm as f32,
                );
                let right = delay_time_ms(
                    slot.right_time,
                    DelayClock::from_value(slot.right_clock),
                    timing.bpm as f32,
                );
                let samples = |ms: f32| {
                    ((ms * timing.sample_rate as f32 / 1_000.0).round() as usize)
                        .clamp(1, max_delay_samples)
                };
                line.process(
                    sample,
                    DelayParams {
                        left_delay_samples: samples(left),
                        right_delay_samples: samples(right),
                        feedback: slot.feedback,
                        amount: slot.amount,
                        vintage: slot.vintage,
                        sample_rate: timing.sample_rate as f32,
                    },
                )
            }
            SlotFx::Reverb(reverb) => {
                // A silenced reverb is fed silence rather than skipped, so
                // its tail rings out instead of stopping dead.
                let input = if slot.amount > 0.0 {
                    sample
                } else {
                    (0.0, 0.0)
                };
                let wet = reverb.process(
                    input.0,
                    input.1,
                    ReverbParams {
                        room_size: slot.time,
                        damp: slot.feedback,
                    },
                );
                (
                    sample.0 + wet.0 * slot.amount,
                    sample.1 + wet.1 * slot.amount,
                )
            }
            SlotFx::Compression(compressor) => compressor.process(
                sample,
                CompressorParams {
                    sample_rate,
                    threshold_db: slot.time,
                    ratio: slot.right_time,
                    release_ms: slot.feedback,
                    makeup_db: slot.vintage,
                    amount: slot.amount,
                },
            ),
            SlotFx::Filter(filter) => filter.process(
                sample,
                FilterParams {
                    sample_rate,
                    cutoff_hz: slot.time,
                    resonance: slot.right_time,
                    filter_type: FilterType::from_value(slot.feedback),
                    amount: slot.amount,
                },
            ),
            SlotFx::Drive(drive) => drive.process(
                sample,
                DriveParams {
                    amount: slot.amount,
                    drive_type: DriveType::from_value(slot.feedback),
                    bias: slot.time,
                    dry: slot.right_time,
                },
            ),
        }
    }

    /// Moves a loaded processor into retirement when its slot no longer asks
    /// for that family — emptied, or swapped for a different module.
    fn retire_if_replaced(&mut self, index: usize, slot: &ModuleSlot) {
        let wanted = slot.kind().map(|kind| kind.family);
        let loaded = self.slots[index].as_ref().map(SlotFx::family);
        if loaded.is_none() || loaded == wanted {
            return;
        }
        let Some(fx) = self.slots[index].take() else {
            return;
        };
        self.retiring[index] = Some(Outgoing::start(
            RetiredFx {
                fx,
                slot: self.last_loaded[index],
            },
            self.retire_fade_samples,
        ));
    }

    /// Crossfades a retiring processor's contribution back to dry, then drops
    /// it once it contributes nothing.
    fn run_retiring(
        &mut self,
        index: usize,
        sample: (f32, f32),
        timing: TimingContext,
    ) -> (f32, f32) {
        let (max_delay_samples, sample_rate) = (self.max_delay_samples, self.sample_rate);
        let Some(retiring) = &mut self.retiring[index] else {
            return sample;
        };
        let wet = Self::process_slot_fx(
            &mut retiring.inner.fx,
            &retiring.inner.slot,
            sample,
            timing,
            max_delay_samples,
            sample_rate,
        );
        let weight = retiring.advance();
        if retiring.is_done() {
            self.retiring[index] = None;
        }
        mix_stereo(sample, wet, weight)
    }

    /// Fold the block's deepest compressor gain into each slot's meter and
    /// publish it. A retiring compressor is not metered: its slot already
    /// shows whatever replaced it.
    fn publish_gain_reduction(&mut self, telemetry: &FluidTelemetry, block_seconds: f32) {
        let release = (-block_seconds / GAIN_REDUCTION_RELEASE_SECONDS).exp();
        for (index, (floor, shown)) in self
            .gain_floor
            .iter_mut()
            .zip(&mut self.gain_reduction_db)
            .enumerate()
        {
            let reduction = if *floor < 1.0 {
                -20.0 * floor.log10()
            } else {
                0.0
            };
            *shown = reduction.max(*shown * release);
            *floor = 1.0;
            telemetry.publish_gain_reduction(index, *shown);
        }
    }

    fn process(
        &mut self,
        tab: Tab,
        slots: &[ModuleSlot; MODULE_SLOTS],
        mut sample: (f32, f32),
        timing: TimingContext,
    ) -> (f32, f32) {
        let Some(layer) = module_layer_index(tab) else {
            return sample;
        };
        for (slot_index, slot) in slots.iter().enumerate() {
            let index = layer * MODULE_SLOTS + slot_index;
            // A module the slot has stopped asking for hands its tail over to
            // the retiring path first, so it fades instead of being cut.
            self.retire_if_replaced(index, slot);
            sample = self.run_retiring(index, sample, timing);

            let Some(kind) = slot.kind() else {
                continue;
            };
            // Recorded before the amount bypass below, so a slot idling at
            // zero still retires with the settings it was last loaded with.
            self.last_loaded[index] = *slot;
            let (max_delay_samples, sample_rate) = (self.max_delay_samples, self.sample_rate);
            let processor = &mut self.slots[index];
            if slot.amount <= 0.0 && processor.is_none() {
                continue;
            }
            match kind.family {
                Family::Delay
                | Family::Reverb
                | Family::Compression
                | Family::Filter
                | Family::Drive => {
                    if processor.is_none() {
                        *processor = Some(match kind.family {
                            Family::Delay => SlotFx::Delay(StereoDelay::new(max_delay_samples)),
                            Family::Reverb => SlotFx::Reverb(FdnReverb::new(sample_rate)),
                            Family::Filter => SlotFx::Filter(StereoFilter::default()),
                            Family::Drive => SlotFx::Drive(StereoDrive::new(sample_rate)),
                            _ => SlotFx::Compression(StereoCompressor::new(0.0)),
                        });
                    }
                    let Some(fx) = processor else {
                        continue;
                    };
                    sample = Self::process_slot_fx(
                        fx,
                        slot,
                        sample,
                        timing,
                        max_delay_samples,
                        sample_rate,
                    );
                    if let SlotFx::Compression(compressor) = fx {
                        self.gain_floor[index] = self.gain_floor[index].min(compressor.gain());
                    }
                }
                // Pre-trigger and DSP-less families shape nothing here.
                Family::SingleAmount | Family::TwoKnob | Family::Drunken => {}
            }
        }
        sample
    }
}

#[cfg(test)]
mod module_fx_tests {
    use super::*;

    const TEST_SAMPLE_RATE: f32 = 48_000.0;

    fn timing() -> TimingContext {
        TimingContext::new(TEST_SAMPLE_RATE as f64, 120.0, 0.0)
    }

    #[test]
    fn manual_compressor_edits_do_not_jump_the_audio_output() {
        for tab in [Tab::Chords, Tab::Master] {
            for (field, from, to) in [
                (ModuleSlotField::Time, -8.0, -40.0),
                (ModuleSlotField::RightTime, 1.0, 8.0),
                (ModuleSlotField::Vintage, 0.0, 12.0),
            ] {
                let mut controls = FluidControls::default();
                let slots = controls.modules.for_tab_mut(tab).unwrap();
                slots.fill(ModuleSlot::default());
                slots[0] = preset_slot("compression", 1.0);
                slots[0].time = -20.0;
                slots[0].right_time = 8.0;
                slots[0].vintage = 0.0;
                let spec = module_slot_spec(tab, 0, field).unwrap();
                (spec.set)(&mut controls, from);
                let mut smoothers = ControlSmoothers::new(&controls);
                let mut bank = ModuleFxBank::new(TEST_SAMPLE_RATE);
                let input = (0.8, -0.4);
                let mut previous = (0.0_f32, 0.0_f32);
                for _ in 0..4_800 {
                    previous =
                        bank.process(tab, controls.modules.for_tab(tab).unwrap(), input, timing());
                }
                let before = previous;
                (spec.set)(&mut controls, to);
                // Reference the immediate DSP result to prove the edit is
                // audible and that the ramp actually removes its jump.
                let mut reference = ModuleFxBank::new(TEST_SAMPLE_RATE);
                let mut after = before;
                for _ in 0..4_800 {
                    after = reference.process(
                        tab,
                        controls.modules.for_tab(tab).unwrap(),
                        input,
                        timing(),
                    );
                }
                let raw_jump = (after.0 - before.0).abs();
                assert!(
                    raw_jump > 0.1,
                    "edit must exercise a substantial gain change"
                );
                smoothers.set_targets(&controls, TEST_SAMPLE_RATE);
                let mut largest_jump = 0.0_f32;
                let frames = (LEVEL_RAMP_MS * 0.001 * TEST_SAMPLE_RATE).round() as usize;
                for _ in 0..frames {
                    let effective = smoothers.next_controls(&controls);
                    let output = bank.process(
                        tab,
                        effective.modules.for_tab(tab).unwrap(),
                        input,
                        timing(),
                    );
                    largest_jump = largest_jump.max((output.0 - previous.0).abs());
                    assert!(
                        (output.1 + output.0 * 0.5).abs() < 1e-6,
                        "stereo image changed"
                    );
                    previous = output;
                }
                assert!(
                    largest_jump < raw_jump * 0.01,
                    "{}: largest sample jump {largest_jump}, unsmoothed {raw_jump}",
                    spec.id
                );
                assert!(
                    (previous.0 - after.0).abs() < 1e-5,
                    "{} never reached its target",
                    spec.id
                );
            }
        }
    }

    /// Feeds silence for up to `frames` frames and returns the first non-zero
    /// output, or `None` when the chain stays silent for the whole span.
    fn first_nonzero_after_silence(
        bank: &mut ModuleFxBank,
        slots: &[ModuleSlot; MODULE_SLOTS],
        tab: Tab,
        timing: TimingContext,
        frames: usize,
    ) -> Option<(f32, f32)> {
        (0..frames)
            .map(|_| bank.process(tab, slots, (0.0, 0.0), timing))
            .find(|output| *output != (0.0, 0.0))
    }

    /// Runs one slot chain for `samples` frames and returns the magnitude of
    /// every output frame. `input` is fed every frame.
    fn run(
        bank: &mut ModuleFxBank,
        slots: &[ModuleSlot; MODULE_SLOTS],
        input: (f32, f32),
        samples: usize,
    ) -> Vec<f32> {
        (0..samples)
            .map(|_| {
                let (l, r) = bank.process(Tab::Chords, slots, input, timing());
                (l * l + r * r).sqrt()
            })
            .collect()
    }

    /// Builds a reverb tail in a slot, then hands back the bank and the tail
    /// level it is currently producing on silence.
    fn bank_with_a_live_tail() -> (ModuleFxBank, [ModuleSlot; MODULE_SLOTS], f32) {
        let mut bank = ModuleFxBank::new(TEST_SAMPLE_RATE);
        let mut slots: [ModuleSlot; MODULE_SLOTS] = std::array::from_fn(|_| ModuleSlot::default());
        slots[0] = preset_slot("room", 1.0);

        run(&mut bank, &slots, (0.6, 0.6), 12_000);
        let tail = run(&mut bank, &slots, (0.0, 0.0), 64);
        let level = tail.iter().sum::<f32>() / tail.len() as f32;
        assert!(level > 0.001, "no reverb tail to test against: {level}");
        (bank, slots, level)
    }

    /// Removing a module used to skip its processor outright, dropping a live
    /// reverb or delay tail to zero in one sample — a loud click on a single
    /// keystroke. The tail has to fade instead.
    #[test]
    fn removing_a_module_fades_its_tail_instead_of_cutting_it() {
        let (mut bank, mut slots, level) = bank_with_a_live_tail();

        slots[0] = ModuleSlot::default();
        assert!(slots[0].is_empty(), "the slot under test must be empty");

        // The sample right after removal must still carry essentially the
        // whole tail: that is the difference between a fade and a cliff.
        let first = run(&mut bank, &slots, (0.0, 0.0), 1)[0];
        assert!(
            first > level * 0.5,
            "tail was cut to {first} from a {level} tail"
        );

        // ...and it must be gone by the end of the ramp, not lingering.
        let ramp = (LEVEL_RAMP_MS * 0.001 * TEST_SAMPLE_RATE) as usize;
        run(&mut bank, &slots, (0.0, 0.0), ramp);
        let settled = run(&mut bank, &slots, (0.0, 0.0), 64);
        assert!(
            settled.iter().all(|s| *s <= level * 0.02),
            "removed module still audible after its fade"
        );
    }

    /// A removed processor used to be skipped rather than dropped, so its
    /// buffers froze mid-tail. Re-adding the same module to that slot then
    /// replayed the previous take's reverb out of nowhere.
    #[test]
    fn re_adding_a_module_does_not_resurrect_the_previous_tail() {
        let (mut bank, mut slots, level) = bank_with_a_live_tail();

        slots[0] = ModuleSlot::default();
        let ramp = (LEVEL_RAMP_MS * 0.001 * TEST_SAMPLE_RATE) as usize;
        run(&mut bank, &slots, (0.0, 0.0), ramp + 64);

        slots[0] = preset_slot("room", 1.0);
        let revived = run(&mut bank, &slots, (0.0, 0.0), 256);
        let loudest = revived.iter().fold(0.0f32, |acc, s| acc.max(*s));
        assert!(
            loudest <= level * 0.02,
            "re-adding the module replayed a stale tail at {loudest}"
        );
    }

    /// Swapping one module for another is the same cliff as removing it, so
    /// the outgoing processor retires the same way.
    #[test]
    fn replacing_a_module_fades_the_outgoing_one() {
        let (mut bank, mut slots, level) = bank_with_a_live_tail();

        slots[0] = preset_slot("drive", 0.0);
        let first = run(&mut bank, &slots, (0.0, 0.0), 1)[0];
        assert!(
            first > level * 0.5,
            "outgoing module was cut to {first} from a {level} tail"
        );
    }

    /// The engine hands Compression slot fields straight to the DSP, which
    /// was written against these ranges. `ControlSpec::apply_value` and the
    /// song decoder both clamp to the spec, so the spec is the only bound
    /// there is — re-clamping at the engine would just let a spec change
    /// drift past what the DSP expects without anything noticing.
    #[test]
    fn compression_slot_specs_bound_every_field_the_dsp_reads() {
        let mut controls = FluidControls::default();
        controls.modules.master[1] = preset_slot("compression", 1.0);

        for (field, min, max) in [
            ("time", -40.0, 0.0),
            ("right_time", 1.0, 8.0),
            ("feedback", 10.0, 500.0),
            ("vintage", 0.0, 12.0),
        ] {
            let id = format!("master.slot2.{field}");
            let spec = spec_by_id(&id)
                .unwrap_or_else(|| panic!("{id} is a registry control"))
                .contextual(&controls);
            assert_eq!(
                (spec.min, spec.max),
                (min, max),
                "{id} no longer carries the range the compressor DSP assumes"
            );
        }
    }

    #[test]
    fn zero_delay_amount_preserves_the_dry_track() {
        let timing = TimingContext::new(44_100.0, 120.0, 0.0);
        let mut bank = ModuleFxBank::new(44_100.0);
        let mut slots = [ModuleSlot::default(); MODULE_SLOTS];
        slots[0] = preset_slot("delay", 0.0);

        let output = bank.process(Tab::Clap, &slots, (0.4, -0.2), timing);

        assert_eq!(output, (0.4, -0.2));
    }

    #[test]
    fn every_post_effect_executes_on_every_layer_chain() {
        let timing = TimingContext::new(44_100.0, 120.0, 0.0);
        for tab in Tab::all() {
            let mut slots = [ModuleSlot::default(); MODULE_SLOTS];
            slots[0] = preset_slot("drive", 0.7);
            let mut bank = ModuleFxBank::new(44_100.0);
            assert_ne!(
                bank.process(tab, &slots, (0.4, -0.2), timing),
                (0.4, -0.2),
                "Drive is inert on {}",
                tab.name()
            );

            slots[0] = preset_slot("compression", 1.0);
            slots[0].time = -20.0;
            slots[0].right_time = 4.0;
            slots[0].vintage = 0.0;
            let mut bank = ModuleFxBank::new(44_100.0);
            let mut compressed = (1.0, 1.0);
            for _ in 0..256 {
                compressed = bank.process(tab, &slots, (1.0, 1.0), timing);
            }
            assert!(compressed.0 < 1.0, "Compression is inert on {}", tab.name());

            slots[0] = preset_slot("room", 1.0);
            let mut bank = ModuleFxBank::new(44_100.0);
            bank.process(tab, &slots, (1.0, 1.0), timing);
            assert!(
                first_nonzero_after_silence(&mut bank, &slots, tab, timing, 2_000).is_some(),
                "Reverb is inert on {}",
                tab.name()
            );

            slots[0] = preset_slot("delay", 1.0);
            slots[0].clock = DelayClock::Free.value();
            slots[0].right_clock = DelayClock::Free.value();
            slots[0].time = 10.0;
            slots[0].right_time = 10.0;
            slots[0].feedback = 0.0;
            let mut bank = ModuleFxBank::new(44_100.0);
            bank.process(tab, &slots, (1.0, -1.0), timing);
            assert!(
                first_nonzero_after_silence(&mut bank, &slots, tab, timing, 500).is_some(),
                "Delay is inert on {}",
                tab.name()
            );
        }
    }

    #[test]
    fn a_compressing_slot_publishes_its_gain_reduction_and_releases_it() {
        let telemetry = FluidTelemetry::default();
        let mut bank = ModuleFxBank::new(TEST_SAMPLE_RATE);
        let mut slots = [ModuleSlot::default(); MODULE_SLOTS];
        slots[2] = preset_slot("compression", 1.0);
        slots[2].time = -20.0;
        slots[2].right_time = 4.0;
        let index = module_layer_index(Tab::Bass).unwrap() * MODULE_SLOTS + 2;
        let block = LEVEL_BLOCK as f32 / TEST_SAMPLE_RATE;
        for _ in 0..4 {
            for _ in 0..LEVEL_BLOCK {
                bank.process(Tab::Bass, &slots, (0.9, -0.9), timing());
            }
            bank.publish_gain_reduction(&telemetry, block);
        }
        // A 0.9 peak sits 19 dB over a -20 dB threshold; 4:1 keeps a quarter.
        let loud = telemetry.gain_reduction()[index];
        assert!((loud - 19.08 * 0.75).abs() < 0.5, "{loud}");
        assert!(
            telemetry
                .gain_reduction()
                .iter()
                .enumerate()
                .all(|(other, db)| other == index || *db == 0.0)
        );

        for _ in 0..LEVEL_BLOCK {
            bank.process(Tab::Bass, &slots, (0.0, 0.0), timing());
        }
        bank.publish_gain_reduction(&telemetry, block);
        let releasing = telemetry.gain_reduction()[index];
        assert!(releasing > 0.0 && releasing < loud, "{releasing}");
        // Five seconds of silence is over sixteen release time constants.
        for _ in 0..(5.0 / block) as usize {
            bank.publish_gain_reduction(&telemetry, block);
        }
        assert!(telemetry.gain_reduction()[index] < 0.01);
    }

    #[test]
    fn zero_amount_is_an_exact_dry_bypass_for_every_post_family() {
        let timing = TimingContext::new(44_100.0, 120.0, 0.0);
        for kind in ["drive", "room", "delay", "compression"] {
            let mut bank = ModuleFxBank::new(44_100.0);
            let mut slots = [ModuleSlot::default(); MODULE_SLOTS];
            slots[0] = preset_slot(kind, 0.0);
            assert_eq!(
                bank.process(Tab::Clap, &slots, (0.4, -0.2), timing),
                (0.4, -0.2),
                "{kind} amount zero changed the dry signal"
            );
        }
    }
}

// ============================================================
// Fluid Engine
// ============================================================

pub(crate) struct FluidEngine {
    pub(crate) current_sample: u64,
    pub(crate) sample_rate: f32,
    pub(crate) tempo: TempoClock,
    beat_trigger: GridTrigger,
    osc_input: Option<OscVoiceSource>,
    pub(crate) control_smoothers: ControlSmoothers,
    mute_gates: OutputGates,
    muted: MuteState,
    hold_muted: [bool; HOLD_MUTE_COUNT],
    state_landed_sample: Option<u64>,
    /// The beat a queued state lands on, while one is queued.
    state_end_beat: Option<f64>,
    state_gain: f32,
    pub(crate) vamp: Option<VampState>,
    vamp_active: bool,
    pub(crate) vamp_landed_beat: Option<f64>,
    vamp_landed_at: Option<f64>,
    /// Push the Harmony as this engine last read it from the session. The
    /// audio clock lands a queued push on its bar, voicing it from the notes
    /// the pad is sounding at that sample.
    push: PushedHarmony,
    pub(crate) pad: PadEngine,
    pub(crate) perc: PercEngine,
    pub(crate) kick: KickEngine,
    pub(crate) tonal: TonalEngine,
    pub(crate) clap: ClapEngine,
    pub(crate) bass: BassEngine,
    pub(crate) arp: ArpEngine,
    pub(crate) lead: LeadEngine,
    module_fx: ModuleFxBank,
    gesture_audio: GestureAudioBank,
    pub(crate) master_bus: MasterBus,
    pub(crate) session: LiveSession,
    /// `Some` only while running `nooise auto`; rewrites `controls` on a
    /// throttled tick so the morph is audible and visible.
    pub(crate) morph: Arc<ArcSwap<Option<MorphState>>>,
    morph_writer: MorphWriter,
    pub(crate) telemetry: Arc<FluidTelemetry>,
    /// Per-`Tab` sum of squared output samples since the last `LEVEL_BLOCK`.
    level_acc: [f32; TAB_COUNT],
    /// Per-`Tab` displayed RMS level with instant attack and exponential release.
    level_meter: [f32; TAB_COUNT],
    pub(crate) snapshot: FluidControls,
    gesture_snapshot: GestureState,
    drunken_phase_beat: f64,
    transport: Transport,
    transport_restart: u64,
    downbeat_reset: u64,
    /// The beat line a requested downbeat reset waits for.
    downbeat_line: Option<f64>,
    /// A stopped launch must not sound continuous voices or age the saved state.
    awaiting_initial_play: bool,
    /// Allocation-free per-sample plan, rebuilt only when aggregate
    /// session identity changes, without comparing or cloning automation maps.
    plan: AutomationPlan,
    /// The published generation this engine has observed: its only retained
    /// session reference. Superseded generations go to `LiveSession::retire`,
    /// so the audio thread never frees one.
    observed: Arc<LiveSessionSnapshot>,
    /// `observed` changed since its state was last applied to the engine.
    observed_unapplied: bool,
    effective: FluidControls,
    authored_muted: MuteState,
    module_timing: ModuleTimingPlan,
}

impl FluidEngine {
    pub(crate) fn new(
        sample_rate: f32,
        session: LiveSession,
        morph: Arc<ArcSwap<Option<MorphState>>>,
        telemetry: Arc<FluidTelemetry>,
    ) -> Self {
        Self::new_with_tonal_session_state(sample_rate, session, morph, telemetry, false)
    }

    pub(crate) fn new_with_tonal_session_state(
        sample_rate: f32,
        session: LiveSession,
        morph: Arc<ArcSwap<Option<MorphState>>>,
        telemetry: Arc<FluidTelemetry>,
        publish_tonal_session_state: bool,
    ) -> Self {
        // Build the progression-tonic table here, off the audio callback.
        progression_tonic_pitch_class(0);
        let live = session.load();
        let snapshot = live.controls.clone();
        let vamp_active = live.vamp.is_some_and(|vamp| vamp.is_active(0.0));
        let mut initial_pad = snapshot.pad.clone();
        if let Some(vamp) = live.vamp.filter(|vamp| vamp.is_active(0.0)) {
            initial_pad.chord_count = 1.0;
            initial_pad.chord_offset = f32::from(vamp.slot);
        }
        if let Some(held) = live.push.sounding(0.0) {
            initial_pad.chord_count = 1.0;
            initial_pad.chord_offset = f32::from(held.slot);
            initial_pad.held_chord = Some(held);
        }
        let mut plan = AutomationPlan::default();
        // Construction runs off the audio thread; an empty plan has no storage.
        let _ = plan.rebuild(&live.automation);
        Self {
            current_sample: 0,
            sample_rate,
            tempo: TempoClock::new(sample_rate, snapshot.master.bpm),
            beat_trigger: GridTrigger::new(),
            osc_input: None,
            control_smoothers: ControlSmoothers::new(&snapshot),
            mute_gates: OutputGates::new(&live.muted),
            muted: live.muted,
            hold_muted: {
                let mut holds = live.hold_muted;
                holds[HoldMute::KickAndBass as usize] |= live.kick_bass_latched;
                holds
            },
            state_landed_sample: None,
            state_end_beat: None,
            state_gain: 1.0,
            vamp: live.vamp,
            vamp_active,
            vamp_landed_beat: None,
            vamp_landed_at: None,
            push: live.push,
            pad: PadEngine::new_with_live_state(
                sample_rate,
                &initial_pad,
                MasterKey::from_controls(&snapshot.master),
                publish_tonal_session_state.then(|| session.clone()),
                Arc::clone(&telemetry),
            ),
            perc: PercEngine::new_with_live_state(
                sample_rate,
                publish_tonal_session_state.then(|| session.clone()),
                Arc::clone(&telemetry),
            ),
            kick: KickEngine::new_with_live_state(
                sample_rate,
                publish_tonal_session_state.then(|| session.clone()),
                Arc::clone(&telemetry),
            ),
            tonal: TonalEngine::new_with_live_state(
                sample_rate,
                live.tonal_sequence.clone(),
                publish_tonal_session_state.then(|| session.clone()),
                Arc::clone(&telemetry),
            ),
            clap: ClapEngine::new_with_live_state(
                sample_rate,
                publish_tonal_session_state.then(|| session.clone()),
                Arc::clone(&telemetry),
            ),
            bass: BassEngine::new_with_live_state(
                sample_rate,
                publish_tonal_session_state.then(|| session.clone()),
            ),
            arp: ArpEngine::new_with_live_state(
                sample_rate,
                publish_tonal_session_state.then(|| session.clone()),
                Arc::clone(&telemetry),
            ),
            lead: LeadEngine::new_with_live_state(
                sample_rate,
                live.lead_play,
                publish_tonal_session_state.then(|| session.clone()),
            ),
            module_fx: ModuleFxBank::new(sample_rate),
            gesture_audio: GestureAudioBank::new(sample_rate),
            master_bus: MasterBus::new(&snapshot.master, sample_rate),
            session,
            morph,
            morph_writer: MorphWriter::default(),
            telemetry,
            level_acc: [0.0; TAB_COUNT],
            level_meter: [0.0; TAB_COUNT],
            authored_muted: live.muted,
            effective: snapshot.clone(),
            module_timing: ModuleTimingPlan::default(),
            snapshot,
            gesture_snapshot: live.gestures.clone(),
            drunken_phase_beat: live.drunken_phase_beat,
            transport: live.transport,
            transport_restart: live.transport_restart,
            downbeat_reset: live.downbeat_reset,
            downbeat_line: None,
            awaiting_initial_play: live.transport == Transport::Stopped,
            plan,
            observed: live,
            observed_unapplied: false,
        }
    }

    /// Rewind musical schedulers while release envelopes and effect tails keep running.
    fn restart_sequence(&mut self) {
        self.tempo.restart();
        self.downbeat_line = None;
        self.vamp_active = false;
        self.vamp_landed_beat = None;
        self.vamp_landed_at = None;
        self.morph_writer = MorphWriter::default();
        self.morph
            .rcu(|current| current.as_ref().as_ref().map(MorphState::restarted));
        self.bass.progression = ProgressionFollower::new();
        self.tonal.progression = ProgressionFollower::new();
        self.arp.progression = ProgressionFollower::new();
        self.lead.progression = ProgressionFollower::new();
        self.rewind_grids();
    }

    /// Downbeat reset on the beat line `line`, reached on this sample: it
    /// becomes beat zero of bar one. Tempo and beat phase are unchanged, so
    /// the bars rotate by whole beats. The chord sounding on the line keeps
    /// ringing and restarts its count (a chord due on the line arrives on
    /// beat zero), so the harmonic timeline (chord cursors, the auto morph, a
    /// vamp) moves back by that chord's start. Every grid and step sequencer
    /// starts again at step 1. Session state stamped on the old clock moves
    /// in the same transaction: Motion loops restart, pushed harmony moves
    /// to the new clock (`PushedHarmony::reanchored`), and `downbeat_applied`/`downbeat_shift` tell the UI to move its own.
    fn reanchor_downbeat(&mut self, line: f64) {
        let shift = self.pad.cursor.restart_at_line(line);
        for follower in [
            &mut self.bass.progression,
            &mut self.tonal.progression,
            &mut self.arp.progression,
            &mut self.lead.progression,
        ] {
            if let Some(cursor) = &mut follower.cursor {
                cursor.restart_at_line(line);
            }
        }
        // A line reached on this sample is beat zero exactly; a line already
        // past keeps the beats played since it, so the phase never moves.
        let since = self.tempo.beat - line;
        let per_sample = self.tempo.bpm / (60.0 * self.tempo.sample_rate);
        self.tempo.beat = if since < per_sample { 0.0 } else { since };
        self.gesture_audio.renumber_beats(line as i64);
        self.vamp_landed_beat = self.vamp_landed_beat.map(|beat| beat - shift);
        self.vamp_landed_at = self.vamp_landed_at.map(|beat| beat - shift);
        self.morph_writer = MorphWriter::default();
        self.morph
            .rcu(|current| current.as_ref().as_ref().map(|morph| morph.rebased(shift)));
        self.session.update(|snapshot| {
            snapshot.vamp = snapshot.vamp.map(|vamp| vamp.rebased_at(shift));
            snapshot.automation.restart();
            snapshot.push = snapshot.push.reanchored(line);
            snapshot.downbeat_applied = snapshot.downbeat_applied.wrapping_add(1);
            snapshot.downbeat_shift = line;
        });
        self.observe_published();
        self.push = self.session.load().push;
        self.rewind_grids();
    }

    /// Every voice grid, step position and evolution cycle starts over.
    fn rewind_grids(&mut self) {
        self.beat_trigger = GridTrigger::new();
        self.bass.step_trigger = GridTrigger::new();
        self.bass.last_cycle = None;
        self.perc.trigger = GridTrigger::new();
        self.perc.last_cycle = None;
        self.kick.trigger = GridTrigger::new();
        self.kick.last_cycle = None;
        self.clap.trigger = GridTrigger::new();
        self.clap.last_cycle = None;
        self.tonal.step_trigger = GridTrigger::new();
        self.tonal.step_index = 0;
        self.tonal.last_cycle = None;
        self.arp.note_trigger = GridTrigger::new();
        self.arp.cycle_pos = 0;
        self.arp.ping_pong_dir = 1;
        self.arp.step_index = 0;
        self.arp.last_cycle = None;
        self.lead.step_trigger = GridTrigger::new();
        self.lead.last_cycle = None;
    }

    pub(crate) fn with_osc_voice(mut self, sink: OscVoiceSink) -> Self {
        self.pad.set_osc(sink.clone());
        self.arp.set_osc(sink.clone());
        self.lead.set_osc(sink.clone());
        self.bass.set_osc(sink);
        self
    }

    pub(crate) fn with_osc_input(mut self, source: OscVoiceSource) -> Self {
        self.osc_input = Some(source);
        self
    }

    fn route_osc_input(&mut self, cmd: OscVoiceCommand) {
        let controls = &self.effective;
        // Played notes follow the key of the progression sounding now.
        let tune =
            MasterKey::from_controls(&controls.master).tune(self.pad.cursor.window.progression);
        match cmd {
            OscVoiceCommand::PadNoteOn(note) => {
                self.pad.osc_note_on(note, &controls.pad, tune);
            }
            OscVoiceCommand::PadNoteOff(note) => {
                self.pad.osc_note_off(note);
            }
            OscVoiceCommand::PadNotes(notes) => {
                self.pad.set_osc_notes(&notes, &controls.pad, tune);
            }
            OscVoiceCommand::ArpNoteOn(note) => {
                self.arp.osc_note_on(note);
            }
            OscVoiceCommand::ArpNoteOff(note) => {
                self.arp.osc_note_off(note);
            }
            OscVoiceCommand::ArpNotes(notes) => {
                self.arp.set_osc_notes(&notes);
            }
            OscVoiceCommand::LeadNoteOn(note) => {
                self.lead.osc_note_on(note, &controls.lead, tune);
            }
            OscVoiceCommand::LeadNoteOff(note) => {
                self.lead.osc_note_off(note);
            }
            OscVoiceCommand::BassNoteOn(note) => {
                self.bass.osc_note_on(note, &controls.bass, tune);
            }
            OscVoiceCommand::KickTrigger(velocity) => {
                self.kick.trigger_hit(&controls.kick, velocity);
            }
            OscVoiceCommand::AllNotesOff => {
                self.pad.release_input();
                self.arp.release_input();
                self.lead.release_input();
                self.bass.release_input();
            }
        }
    }
}

impl FluidEngine {
    /// Reseed every voice RNG for reproducible offline renders.
    pub(crate) fn reseed(&mut self, seed: u64) {
        self.pad.rng = StdRng::seed_from_u64(seed);
        self.pad.evolution_seed = seed.wrapping_add(10);
        self.perc.rng = StdRng::seed_from_u64(seed.wrapping_add(1));
        self.perc.evolution_seed = seed.wrapping_add(11);
        self.kick.rng = StdRng::seed_from_u64(seed.wrapping_add(2));
        self.kick.evolution_seed = seed.wrapping_add(12);
        self.tonal.rng = StdRng::seed_from_u64(seed.wrapping_add(3));
        self.clap.rng = StdRng::seed_from_u64(seed.wrapping_add(4));
        self.clap.evolution_seed = seed.wrapping_add(14);
        self.arp.rng = StdRng::seed_from_u64(seed.wrapping_add(5));
        self.arp.evolution_seed = seed.wrapping_add(15);
        self.bass.evolution_seed = seed.wrapping_add(16);
        self.lead.evolution_seed = seed.wrapping_add(17);
    }
}

impl StereoEngine for FluidEngine {
    fn next_stereo(&mut self) -> (f32, f32) {
        // Every publication reaches the engine on the next sample, so a
        // command arriving at any moment still lands on its boundary.
        self.observe_published();
        if self.awaiting_initial_play {
            if self.observed.transport == Transport::Stopped {
                return (0.0, 0.0);
            }
            self.awaiting_initial_play = false;
        }
        // A queued state lands on the pad cursor's phrase end, the same
        // `ProgressionCursor::phrase_end_beat` that every phrase consumer
        // reads, recomputed each sample so chord edits and a held Vamp move
        // it exactly as they move the phrase. `tempo.beat` is already the beat
        // this sample's pad tick will use, so landing here is before the
        // crossing, never a sample after it.
        self.state_end_beat = self
            .observed
            .pending_state
            .as_ref()
            .and_then(|_| self.phrase_end_now());
        self.session.publish_audio_phrase_end(self.state_end_beat);
        let state_due = self.transport == Transport::Playing
            && self
                .state_end_beat
                .is_some_and(|end| self.tempo.beat + GRID_BEAT_EPSILON >= end);
        let landed = state_due
            && self.observed.pending_state.as_ref().is_some_and(|pending| {
                self.session.land_state(
                    pending,
                    self.current_sample as f64 / f64::from(self.sample_rate),
                )
            });
        if landed {
            self.morph.store(Arc::new(None));
            self.state_landed_sample = Some(self.current_sample);
            self.observe_published();
        }
        let restarting =
            self.observed_unapplied && self.transport_restart != self.observed.transport_restart;
        if restarting {
            self.transport_restart = self.observed.transport_restart;
            self.restart_sequence();
        }
        // A requested reset takes the nearest beat line, within half a beat
        // either side: one ahead is waited for, one just past applies now as
        // though it had been beat zero. Either way the beat phase never
        // moves. A restart already starts at bar one, so it absorbs a reset
        // requested in the same generation.
        if self.observed_unapplied && self.downbeat_reset != self.observed.downbeat_reset {
            self.downbeat_reset = self.observed.downbeat_reset;
            if !restarting {
                self.downbeat_line = Some(nearest_beat_line(self.tempo.beat));
            }
        }
        let reanchoring = self.transport == Transport::Playing
            && self
                .downbeat_line
                .is_some_and(|line| self.tempo.beat + GRID_BEAT_EPSILON >= line);
        if let Some(line) = self.downbeat_line.filter(|_| reanchoring) {
            self.downbeat_line = None;
            self.reanchor_downbeat(line);
        }
        // ~2.9 ms at 44.1 kHz: the clock and the auto morph advance on this
        // cadence; session state is applied on change, never polled.
        let morph_boundary = self
            .morph
            .load()
            .as_ref()
            .as_ref()
            .is_some_and(|morph| self.morph_writer.boundary_due(morph, self.tempo.beat));
        if self.current_sample.is_multiple_of(128)
            || morph_boundary
            || landed
            || restarting
            || reanchoring
        {
            self.session.publish_audio_beat(self.tempo.beat);
            self.session
                .publish_audio_phrase_start(self.pad.cursor.phrase_start_beat());
            self.session
                .publish_audio_seconds(self.current_sample as f64 / self.sample_rate as f64);
            let morph_source = self.morph.load_full();
            if let Some(morph) = morph_source.as_ref()
                && let Some((next_controls, next_automation)) =
                    self.morph_writer.tick(morph, self.tempo.beat)
            {
                let _ = self.session.transact(|snapshot| {
                    if !Arc::ptr_eq(&self.morph.load_full(), &morph_source) {
                        return Err(());
                    }
                    snapshot.controls = next_controls.clone();
                    snapshot.automation = next_automation.clone();
                    Ok(())
                });
                self.observe_published();
            }
            if morph_boundary
                && morph_source
                    .as_ref()
                    .as_ref()
                    .is_some_and(|morph| morph.position_at(self.tempo.beat).blend.is_none())
            {
                self.apply_observed(landed, restarting);
                // Restore both dropped layers for the first landing note.
                // Kick also captures Level when the hit is constructed.
                let smoothers = &mut self.control_smoothers;
                smoothers.settle(
                    smoothers.kick_level,
                    self.snapshot.kick.level,
                    &mut self.effective,
                );
                smoothers.settle(
                    smoothers.bass_level,
                    self.snapshot.bass.level,
                    &mut self.effective,
                );
            }
        }
        self.apply_observed(landed, restarting);

        let fade = startup_fade(self.current_sample, self.sample_rate);
        self.control_smoothers.apply(&mut self.effective);
        let base_bpm = self.effective.master.bpm;
        let base_kick = self.effective.kick.level;
        let base_bass = self.effective.bass.level;
        // Perc Roll and the vamp override these per sample; restore them below
        // so the persistent working controls never compound the override.
        let base_perc_interval = self.effective.perc.interval_beats;
        let base_chord_count = self.effective.pad.chord_count;
        let base_chord_offset = self.effective.pad.chord_offset;
        let morph = self.morph.load();
        let active_morph = morph.as_ref().as_ref();
        if let Some(morph) = active_morph {
            self.effective.master.bpm = morph.tempo_at(self.tempo.beat);
            // The morph already supplies a smooth curve at sample rate.
            // A second lag would drag half/double-time jumps past the landing.
            self.tempo.bpm = f64::from(self.effective.master.bpm);
        }
        let mut timing = self.tempo.tick(self.effective.master.bpm, self.transport);
        self.advance_vamp(timing);
        self.advance_push(timing);
        if self.beat_trigger.pop_swung(timing, 1.0, 0.0, 0.0) {
            self.telemetry
                .publish_hit(MusicalHit::Beat, 1.0, NO_PITCH_CLASS);
        }
        if self.current_sample.is_multiple_of(256) {
            self.telemetry.publish_beat(timing.beat);
        }
        self.plan.apply(&mut self.effective, timing);
        if self.hold_muted[HoldMute::PercRoll as usize] {
            self.effective.perc.interval_beats =
                faster_perc_interval(self.effective.perc.interval_beats);
        }
        // Both dropped layers stay silent through the percentage phase,
        // including Level automation and their post-module effect tails.
        let morph_phrase = active_morph.and_then(|morph| morph.phrase_start_at(timing.beat));
        timing.morph_phrase_start = match (morph_phrase, self.vamp_landed_beat) {
            (Some(a), Some(b)) => {
                if self.vamp_landed_at.is_some_and(|at| at >= a) {
                    Some(b)
                } else {
                    Some(a)
                }
            }
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        if let Some(vamp) = self.vamp.filter(|vamp| vamp.is_active(timing.beat)) {
            self.effective.pad.chord_count = 1.0;
            self.effective.pad.chord_offset = f32::from(vamp.slot);
            timing.morph_phrase_start = Some(vamp.start_beat);
        }
        // A pushed chord overrides the progression the way a held vamp does:
        // every chord follower stays on its slot and reads the pushed notes.
        let sounding = self.push.sounding(timing.beat);
        self.effective.pad.held_chord = sounding;
        if let Some(held) = sounding {
            self.effective.pad.chord_count = 1.0;
            self.effective.pad.chord_offset = f32::from(held.slot);
        }
        let drop_gain =
            active_morph.map_or(1.0, |morph| morph.drop_gain_at(timing.beat, timing.bpm));
        if drop_gain == 0.0 {
            self.effective.kick.level = 0.0;
            self.effective.bass.level = 0.0;
        }
        self.module_timing.resolve(&mut self.effective);
        timing.drunken_amount = self.effective.master.drunken_amount;
        timing.drunken_pace = self.effective.master.drunken_pace;
        timing.drunken_phase_beat = self.drunken_phase_beat;
        // A local slot owns its layer's grid timing, including at Amount 0.
        // Without one, Master remains the global fallback. The session's
        // saved wave phase belongs to every per-layer timing copy.
        if self.current_sample.is_multiple_of(128)
            && let Some(source) = self.osc_input.clone()
        {
            while let Some(cmd) = source.try_recv() {
                self.route_osc_input(cmd);
            }
        }
        let layer_timing = |tab: Tab, modules: &LayerModules| {
            let (amount, pace) = self.module_timing.drunken(
                tab,
                modules,
                timing.drunken_amount,
                timing.drunken_pace,
            );
            timing.with_drunken(amount, pace)
        };
        // Suppress only effective OSC voice output when a layer or Master is muted.
        let master_muted = self.muted[Tab::Master as usize];
        self.pad
            .set_osc_suppressed(master_muted || self.muted[Tab::Chords as usize]);
        self.arp
            .set_osc_suppressed(master_muted || self.muted[Tab::Arp as usize]);
        self.lead
            .set_osc_suppressed(master_muted || self.muted[Tab::Lead as usize]);
        self.bass
            .set_osc_suppressed(master_muted || self.muted[Tab::Bass as usize]);
        let mute_gains = self.mute_gates.next();
        let now_seconds = self.current_sample as f64 / self.sample_rate as f64;

        let key = MasterKey::from_controls(&self.effective.master);
        let pad = self.module_fx.process(
            Tab::Chords,
            &self.effective.modules.pad,
            self.pad.next(
                &self.effective.pad,
                key,
                layer_timing(Tab::Chords, &self.effective.modules).with_groove_seed(1),
            ),
            timing,
        );
        let (pad_l, pad_r) = gate_stereo(pad, mute_gains[Tab::Chords as usize]);
        let perc = self.module_fx.process(
            Tab::Perc,
            &self.effective.modules.perc,
            {
                let perc = self.perc.next(
                    &self.effective.perc,
                    layer_timing(Tab::Perc, &self.effective.modules).with_groove_seed(2),
                );
                (perc, perc)
            },
            timing,
        );
        let (perc_l, perc_r) = gate_stereo(perc, mute_gains[Tab::Perc as usize]);
        let kick = self.module_fx.process(
            Tab::Kick,
            &self.effective.modules.kick,
            self.kick.next(
                &self.effective.kick,
                layer_timing(Tab::Kick, &self.effective.modules).with_groove_seed(3),
            ),
            timing,
        );
        let (kick_l, kick_r) = gate_stereo(kick, mute_gains[Tab::Kick as usize] * drop_gain);
        let tonal = self.module_fx.process(
            Tab::Tonal,
            &self.effective.modules.tonal,
            self.tonal.next(
                &self.effective.tonal,
                &self.effective.pad,
                key,
                layer_timing(Tab::Tonal, &self.effective.modules).with_groove_seed(4),
            ),
            timing,
        );
        let (ton_l, ton_r) = gate_stereo(tonal, mute_gains[Tab::Tonal as usize]);
        let clap = self.module_fx.process(
            Tab::Clap,
            &self.effective.modules.clap,
            self.clap.next(
                &self.effective.clap,
                layer_timing(Tab::Clap, &self.effective.modules).with_groove_seed(5),
            ),
            timing,
        );
        let (clap_l, clap_r) = gate_stereo(clap, mute_gains[Tab::Clap as usize]);
        let bass = self.module_fx.process(
            Tab::Bass,
            &self.effective.modules.bass,
            self.bass.next(
                &self.effective.bass,
                &self.effective.pad,
                key,
                layer_timing(Tab::Bass, &self.effective.modules).with_groove_seed(6),
            ),
            timing,
        );
        let (bass_l, bass_r) = gate_stereo(bass, mute_gains[Tab::Bass as usize] * drop_gain);
        let arp = self.module_fx.process(
            Tab::Arp,
            &self.effective.modules.arp,
            self.arp.next(
                &self.effective.arp,
                &self.effective.pad,
                key,
                layer_timing(Tab::Arp, &self.effective.modules).with_groove_seed(7),
            ),
            timing,
        );
        let (arp_l, arp_r) = gate_stereo(arp, mute_gains[Tab::Arp as usize]);
        let lead = self.module_fx.process(
            Tab::Lead,
            &self.effective.modules.lead,
            self.lead.next(
                &self.effective.lead,
                &self.effective.pad,
                key,
                layer_timing(Tab::Lead, &self.effective.modules).with_groove_seed(8),
            ),
            timing,
        );
        let (lead_l, lead_r) = gate_stereo(lead, mute_gains[Tab::Lead as usize]);
        self.current_sample += 1;

        let voices_l = VoiceMix {
            pad: pad_l,
            perc: perc_l,
            kick: kick_l,
            tonal: ton_l,
            clap: clap_l,
            bass: bass_l,
            arp: arp_l,
            lead: lead_l,
        };
        let voices_r = VoiceMix {
            pad: pad_r,
            perc: perc_r,
            kick: kick_r,
            tonal: ton_r,
            clap: clap_r,
            bass: bass_r,
            arp: arp_r,
            lead: lead_r,
        };
        let heard_l = voices_l.weighted(fade);
        let heard_r = voices_r.weighted(fade);
        for (acc, (l, r)) in self.level_acc.iter_mut().zip(heard_l.iter().zip(&heard_r)) {
            *acc += l * l + r * r;
        }
        let raw_l = voices_l.sum(fade);
        let raw_r = voices_r.sum(fade);
        let master = self.module_fx.process(
            Tab::Master,
            &self.effective.modules.master,
            (raw_l, raw_r),
            timing,
        );
        let mut gesture_amounts = self.gesture_snapshot.amounts(now_seconds);
        let rise_amount = self.gesture_snapshot.rise_amount(now_seconds);
        gesture_amounts[GestureKind::Lift as usize] =
            gesture_amounts[GestureKind::Lift as usize].max(rise_amount * 0.52);
        gesture_amounts[GestureKind::Echo as usize] =
            gesture_amounts[GestureKind::Echo as usize].max(rise_amount * 0.25);
        let master = self.gesture_audio.process(master, gesture_amounts, timing);
        let mut out = gate_stereo(
            self.master_bus
                .process(master.0, master.1, &self.effective.master),
            mute_gains[Tab::Master as usize],
        );
        let fade_samples = (self.sample_rate * 0.005).max(1.0) as u64;
        let fade_in = self.state_landed_sample.map_or(1.0, |start| {
            (self.current_sample.saturating_sub(start) as f32 / fade_samples as f32).min(1.0)
        });
        let fade_out = self.state_end_beat.map_or(1.0, |end| {
            let remaining = ((end - self.tempo.beat) * 60.0 * f64::from(self.sample_rate)
                / self.tempo.bpm) as f32;
            (remaining / fade_samples as f32).clamp(0.0, 1.0)
        });
        let target_gain = fade_in.min(fade_out);
        let step = 1.0 / fade_samples as f32;
        self.state_gain += (target_gain - self.state_gain).clamp(-step, step);
        out.0 *= self.state_gain;
        out.1 *= self.state_gain;
        self.level_acc[Tab::Master as usize] += out.0 * out.0 + out.1 * out.1;
        if self.current_sample.is_multiple_of(LEVEL_BLOCK) {
            let block_seconds = LEVEL_BLOCK as f32 / self.sample_rate;
            let level_release = (-block_seconds / LEVEL_METER_RELEASE_SECONDS).exp();
            for ((tab, acc), shown) in Tab::all()
                .into_iter()
                .zip(&mut self.level_acc)
                .zip(&mut self.level_meter)
            {
                let rms = (*acc / (2 * LEVEL_BLOCK) as f32).sqrt();
                *shown = rms.max(*shown * level_release);
                self.telemetry.publish_level(tab, *shown);
                *acc = 0.0;
            }
            for (kind, amount) in GestureKind::ALL
                .into_iter()
                .zip(self.gesture_snapshot.amounts(now_seconds))
            {
                self.telemetry.publish_gesture(kind, amount);
            }
            self.module_fx
                .publish_gain_reduction(&self.telemetry, block_seconds);
        }
        self.log_heard_onset(timing.beat);
        self.plan.restore(&mut self.effective);
        self.effective.master.bpm = base_bpm;
        self.effective.kick.level = base_kick;
        self.effective.bass.level = base_bass;
        self.effective.perc.interval_beats = base_perc_interval;
        self.effective.pad.chord_count = base_chord_count;
        self.effective.pad.chord_offset = base_chord_offset;
        out
    }
}

impl FluidEngine {
    /// Adopt a newer published generation, if there is one. The superseded
    /// Arc is retired rather than dropped: the publisher has usually released
    /// it already, so this engine may hold its last reference.
    fn observe_published(&mut self) -> bool {
        let published = self.session.published();
        if Arc::ptr_eq(&published, &self.observed) {
            return false;
        }
        let previous =
            std::mem::replace(&mut self.observed, arc_swap::Guard::into_inner(published));
        self.session.retire(Retired::Snapshot(previous));
        self.observed_unapplied = true;
        true
    }

    /// Derive the engine's working state from `observed` once per change.
    fn apply_observed(&mut self, landed: bool, restarting: bool) {
        if !self.observed_unapplied {
            return;
        }
        self.observed_unapplied = false;
        // A second handle so fields of `self` can be written while reading it;
        // `observed` keeps the generation alive, so this drop never frees.
        let session = Arc::clone(&self.observed);
        if landed {
            self.tonal.restore_sequence(&session.tonal_sequence);
        }
        self.snapshot.clone_from(&session.controls);
        self.effective.clone_from(&self.snapshot);
        if restarting {
            self.pad.restart_sequence(&self.snapshot.pad);
        }
        self.gesture_snapshot.clone_from(&session.gestures);
        self.drunken_phase_beat = session.drunken_phase_beat;
        self.transport = session.transport;
        self.control_smoothers
            .set_targets(&self.snapshot, self.sample_rate);
        if landed {
            // Tempo is a Timing row with no smoother: the clock takes it at once.
            self.tempo.bpm = f64::from(self.snapshot.master.bpm);
        }
        self.authored_muted = session.muted;
        self.hold_muted = session.hold_muted;
        self.hold_muted[HoldMute::KickAndBass as usize] |= session.kick_bass_latched;
        if self.vamp != session.vamp {
            self.vamp = session.vamp;
        }
        let muted = self.gated_mute_state(session.muted);
        self.mute_gates.set_targets(&muted, self.sample_rate);
        self.muted = muted;
        self.lead.observe(session.lead_play);
        self.master_bus
            .set_controls(&self.snapshot.master, self.sample_rate);
        if let Some(retired) = self.plan.rebuild(&session.automation) {
            self.session.retire(Retired::AutomationRoutes(retired));
        }
    }

    /// The mutes the output gates follow: the authored ones plus the Kick+Bass
    /// group gate while it is held or latched.
    fn gated_mute_state(&self, muted: MuteState) -> MuteState {
        let mut gated = muted;
        if self.hold_muted[HoldMute::KickAndBass as usize] {
            gated[Tab::Kick as usize] = true;
            gated[Tab::Bass as usize] = true;
        }
        gated
    }

    /// Where the sounding phrase ends, read from the pad cursor under the
    /// controls its next tick will use: the requested window and chord length,
    /// narrowed to the held chord while a Vamp is active.
    fn phrase_end_now(&self) -> Option<f64> {
        let pad = &self.effective.pad;
        let mut window = ChordWindow::requested(pad);
        if let Some(vamp) = self.vamp.filter(|vamp| vamp.is_active(self.tempo.beat)) {
            window.count = 1;
            window.offset = usize::from(vamp.slot);
        }
        if let Some(held) = self.push.sounding(self.tempo.beat) {
            window.count = 1;
            window.offset = usize::from(held.slot);
        }
        self.pad
            .cursor
            .phrase_end_beat(window, pad.chord_bars * 4.0, self.tempo.beat)
    }

    fn reset_progression_phrase_anchors(&mut self) {
        self.pad.cursor.morph_phrase_start = None;
        for follower in [
            &mut self.bass.progression,
            &mut self.arp.progression,
            &mut self.lead.progression,
        ] {
            if let Some(cursor) = &mut follower.cursor {
                cursor.morph_phrase_start = None;
            }
        }
    }

    fn pad_landing_slot_at(&self, beat: f64) -> usize {
        if self
            .pad
            .cursor
            .next_chord_beat
            .is_some_and(|next| beat + GRID_BEAT_EPSILON >= next)
        {
            let window = ChordWindow::requested(&self.snapshot.pad);
            let old_next_slot = self
                .pad
                .cursor
                .window
                .slot((self.pad.cursor.step + 1) % self.pad.cursor.window.count);
            let step = if window.progression != self.pad.cursor.window.progression {
                0
            } else if let Some(current_step) = window
                .slots()
                .position(|slot| slot == self.pad.cursor.slot())
            {
                (current_step + 1) % window.count
            } else {
                window
                    .slots()
                    .position(|slot| slot == old_next_slot)
                    .unwrap_or(0)
            };
            window.slot(step)
        } else {
            self.pad.cursor.slot()
        }
    }

    fn advance_vamp(&mut self, timing: TimingContext) {
        if self.observed.vamp != self.vamp {
            self.vamp = self.observed.vamp;
        }

        let mut released_vamp = None;
        if timing.transport == Transport::Playing
            && let Some(vamp) = self.vamp
            && vamp.is_due(timing.beat)
        {
            match self.session.transact(|snapshot| {
                if snapshot.vamp != Some(vamp) {
                    return Err(());
                }
                snapshot.vamp = None;
                Ok(())
            }) {
                Ok(_) => {
                    self.vamp = None;
                    released_vamp = Some(vamp);
                    self.observe_published();
                }
                Err(()) => {
                    self.observe_published();
                    self.vamp = self.observed.vamp;
                }
            }
        }

        let now_active = self.vamp.is_some_and(|vamp| vamp.is_active(timing.beat));
        if now_active != self.vamp_active {
            if now_active {
                if let Some(mut vamp) = self.vamp {
                    let landing_slot = self.pad_landing_slot_at(timing.beat) as u8;
                    if landing_slot != vamp.slot {
                        vamp.slot = landing_slot;
                        self.vamp = Some(vamp);
                        let _ = self.session.transact(|snapshot| {
                            if let Some(active) = &mut snapshot.vamp {
                                active.slot = landing_slot;
                            }
                            Ok::<(), ()>(())
                        });
                        self.observe_published();
                    }
                }
                self.vamp_landed_beat = None;
                self.vamp_landed_at = None;
            } else {
                let interval =
                    GridSpec::new(self.snapshot.pad.chord_bars * 4.0, 0.0, 0.0).interval_beats;
                let landed_at = released_vamp.map_or_else(
                    || (timing.beat / interval).floor() * interval,
                    |vamp| vamp.end_beat,
                );
                let window = ChordWindow::requested(&self.snapshot.pad);
                let locked_slot = released_vamp
                    .map_or_else(|| self.pad.cursor.slot(), |vamp| usize::from(vamp.slot));
                let locked_step = window
                    .slots()
                    .position(|slot| slot == locked_slot)
                    .unwrap_or(0);
                let next_step = (locked_step + 1) % window.count;
                self.vamp_landed_beat = Some(landed_at - (next_step as f64) * interval);
                self.vamp_landed_at = Some(landed_at);
                self.reset_progression_phrase_anchors();
            }
            self.vamp_active = now_active;
        }
    }
}

impl FluidEngine {
    /// Append a chord the pad just began to the heard log `G` grabs from:
    /// a pushed or looped chord by its own triad, an authored one heard as a
    /// triad on its slot, each with the notes actually sounded.
    fn log_heard_onset(&mut self, beat: f64) {
        let Some(notes) = self.pad.onset.take() else {
            return;
        };
        let pad = &self.effective.pad;
        let key = MasterKey::from_controls(&self.effective.master);
        let slot = self.pad.cursor.slot();
        let triad = pad.held_at(slot).map_or_else(
            || authored_triad(pad, key, progression_index(pad.progression), slot),
            |held| held.triad,
        );
        let event = HarmonyEvent {
            at: beat,
            triad,
            notes,
        };
        let _ = self.session.transact(|snapshot| {
            snapshot.push.heard.record(event);
            Ok::<(), ()>(())
        });
        self.push.heard.record(event);
    }

    /// Land a queued push on its bar line. The chord is voiced from the notes
    /// the pad sounds at this sample, on the slot sounding now (a held chord
    /// keeps its own), and published before this sample's chord tick reads
    /// it. A UI writer racing the landing wins; the landing retries next
    /// sample from the newer state.
    fn advance_push(&mut self, timing: TimingContext) {
        let push = self.session.load().push;
        self.push = push;
        if timing.transport != Transport::Playing || !push.is_due(timing.beat + GRID_BEAT_EPSILON) {
            return;
        }
        let mut landed = push;
        landed.land(self.pad.last_chord_notes, self.pad.cursor.slot());
        if self
            .session
            .transact(|snapshot| {
                if snapshot.push != push {
                    return Err(());
                }
                snapshot.push = landed;
                Ok(())
            })
            .is_ok()
        {
            self.push = landed;
        }
    }
}

/// The beat line nearest `beat`: a press within half a beat after a line
/// means that line, so a slightly late tap is not a beat late.
fn nearest_beat_line(beat: f64) -> f64 {
    (beat + GRID_BEAT_EPSILON).round()
}

fn faster_perc_interval(interval_beats: f32) -> f32 {
    if interval_beats >= 4.25 {
        interval_beats
    } else if interval_beats <= BEAT_GRID_FLOOR {
        BEAT_GRID_FLOOR * 0.5
    } else {
        beat_grid_adjust(interval_beats, -1.0, BEAT_GRID_FLOOR, 4.25)
    }
}

#[cfg(test)]
mod retire_tests {
    use super::*;
    use crate::alloc_count::{HeapTraffic, count};
    use crate::fluid::auto::decode_auto_states;

    /// A UI edit is observed on the next sample. On an automated song that
    /// sample rebuilds the automation plan, and the superseded snapshot and
    /// route storage must reach the UI thread instead of being freed here.
    #[test]
    fn ui_edits_on_an_automated_song_free_nothing_on_the_audio_thread() {
        let song = &decode_auto_states()[2];
        let session = LiveSession::new(LiveSessionSnapshot::from_song(song));
        let mut engine = FluidEngine::new(
            44_100.0,
            session.clone(),
            no_morph(),
            Arc::new(FluidTelemetry::default()),
        );
        engine.reseed(3);
        for _ in 0..44_100 {
            engine.next_stereo();
        }
        assert!(engine.plan.live_routes() > 0, "song must carry automation");
        let lfo = ControlAddress::new("pad.level");
        let mut observed = HeapTraffic::default();
        for edit in 0..48 {
            // Alternate a plain knob edit with adding and clearing a lane, so
            // routes are rebuilt, carried, faded and removed.
            session.update(|snapshot| match edit % 3 {
                0 => snapshot.controls.master.level = 0.5 + 0.004 * edit as f32,
                1 => {
                    snapshot.automation.add_route(lfo, LfoRoute::default());
                }
                _ => snapshot.automation.clear_control(lfo),
            });
            let before = Arc::as_ptr(&engine.observed);
            let ((), traffic) = count(|| {
                engine.next_stereo();
            });
            assert_ne!(
                Arc::as_ptr(&engine.observed),
                before,
                "edit {edit} observed"
            );
            assert_eq!(traffic.frees, 0, "edit {edit} freed on the audio thread");
            observed += traffic;
            // The UI turn loop frees what was retired.
            assert!(session.reclaim() >= 1);
            for _ in 0..2_048 {
                engine.next_stereo();
            }
            session.reclaim();
        }
        assert_eq!(session.retire_overflows(), 0);
        // The plan rebuild still allocates its new routes (out of scope);
        // print it so the remaining half stays visible.
        println!("48 observed edits: {observed:?}");
    }
}

#[inline]
fn gate_stereo(sample: (f32, f32), gain: f32) -> (f32, f32) {
    (sample.0 * gain, sample.1 * gain)
}

pub(crate) fn startup_fade(current_sample: u64, sample_rate: f32) -> f32 {
    (current_sample as f32 / (sample_rate * STARTUP_FADE_SECONDS)).min(1.0)
}

/// One channel of every voice's output, summed at the fixed mix weights.
struct VoiceMix {
    pad: f32,
    perc: f32,
    kick: f32,
    tonal: f32,
    clap: f32,
    bass: f32,
    arp: f32,
    lead: f32,
}

impl VoiceMix {
    /// The mix. Kept as one expression: its rounding order is what the
    /// golden render pins, so it is not rebuilt from `weighted`.
    #[inline]
    fn sum(&self, fade: f32) -> f32 {
        (self.pad
            + self.perc * 0.6
            + self.kick * 0.7
            + self.tonal
            + self.clap * 0.65
            + self.bass * 0.75
            + self.arp
            + self.lead)
            * fade
    }

    /// Each voice at its mix weight, indexed by `Tab` (`Tab::Master` stays
    /// zero here), for level telemetry only.
    #[inline]
    fn weighted(&self, fade: f32) -> [f32; TAB_COUNT] {
        let mut out = [0.0; TAB_COUNT];
        out[Tab::Chords as usize] = self.pad * fade;
        out[Tab::Perc as usize] = self.perc * 0.6 * fade;
        out[Tab::Bass as usize] = self.bass * 0.75 * fade;
        out[Tab::Kick as usize] = self.kick * 0.7 * fade;
        out[Tab::Tonal as usize] = self.tonal * fade;
        out[Tab::Clap as usize] = self.clap * 0.65 * fade;
        out[Tab::Arp as usize] = self.arp * fade;
        out[Tab::Lead as usize] = self.lead * fade;
        out
    }
}

/// A smoothstep-eased ramp from one value to the next over a fixed sample
/// count: the shape behind every click-free level change in the engine. A
/// caller decides *whether* a new target is worth a ramp; `retarget` always
/// starts one from wherever the ramp currently sits.
#[derive(Clone, Copy)]
pub(crate) struct EasedRamp {
    pub(crate) start: f32,
    pub(crate) current: f32,
    pub(crate) target: f32,
    pub(crate) samples_total: u32,
    pub(crate) samples_remaining: u32,
}

impl EasedRamp {
    pub(crate) fn settled(value: f32) -> Self {
        Self {
            start: value,
            current: value,
            target: value,
            samples_total: 0,
            samples_remaining: 0,
        }
    }

    pub(crate) fn retarget(&mut self, target: f32, ramp_samples: u32) {
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
        let eased = smoothstep(elapsed as f32 / self.samples_total as f32);
        self.current = self.start + (self.target - self.start) * eased;
        self.samples_remaining -= 1;
        if self.samples_remaining == 0 {
            self.current = self.target;
        }
        self.current
    }
}

/// One click-free mute gate per layer, ramping between silence and unity.
struct OutputGates {
    gates: [EasedRamp; TAB_COUNT],
}

impl OutputGates {
    fn gain(muted: bool) -> f32 {
        if muted { 0.0 } else { 1.0 }
    }

    fn new(muted: &MuteState) -> Self {
        Self {
            gates: std::array::from_fn(|index| EasedRamp::settled(Self::gain(muted[index]))),
        }
    }

    fn set_targets(&mut self, muted: &MuteState, sample_rate: f32) {
        let ramp_samples = (LEVEL_RAMP_MS * 0.001 * sample_rate).round() as u32;
        for (gate, muted) in self.gates.iter_mut().zip(muted) {
            let target = Self::gain(*muted);
            if target != gate.target {
                gate.retarget(target, ramp_samples);
            }
        }
    }

    fn next(&mut self) -> [f32; TAB_COUNT] {
        std::array::from_fn(|index| self.gates[index].next())
    }
}

pub(crate) struct ControlSmoother {
    pub(crate) spec: &'static ControlSpec,
    pub(crate) ramp: EasedRamp,
    /// The module slot this row addresses, parsed once from its id so the
    /// audio thread never parses id strings.
    slot_address: Option<ModuleSlotAddress>,
    slot_kind: Option<f32>,
    slot_filter_type: Option<f32>,
    /// True while the smoother is settled AND its target equals the snapshot
    /// value bit-for-bit, so `next_controls` can skip the per-sample write
    /// (which would be a no-op). Recomputed every `set_targets` call; stays
    /// false when `set_target`'s epsilon guard leaves a sub-epsilon gap
    /// between target and snapshot, where the write is load-bearing.
    pub(crate) idle: bool,
}

impl ControlSmoother {
    /// A smoother on the first registry gain control, for tests that exercise
    /// the ramp itself rather than which control it drives.
    #[cfg(test)]
    pub(crate) fn new(value: f32) -> Self {
        let spec = all_specs()
            .find(|spec| spec.kind.smooths_audio())
            .expect("the registry declares at least one gain control");
        Self::for_spec(spec, value)
    }

    pub(crate) fn for_spec(spec: &'static ControlSpec, value: f32) -> Self {
        Self {
            spec,
            ramp: EasedRamp::settled(value),
            slot_address: ModuleSlotAddress::parse(spec.id),
            slot_kind: None,
            slot_filter_type: None,
            idle: false,
        }
    }

    pub(crate) fn set_target(&mut self, target: f32, ramp_samples: u32) {
        if (target - self.ramp.target).abs() <= f32::EPSILON {
            return;
        }
        self.ramp.retarget(target, ramp_samples);
    }

    pub(crate) fn next(&mut self) -> f32 {
        self.ramp.next()
    }
}

// A Filter Type edit mirrors Cutoff. Keep that pair atomic instead of
// sweeping through the old cutoff under the new response.
fn filter_type_for_slot(slot: &ModuleSlot) -> Option<f32> {
    slot.kind()
        .filter(|kind| kind.family == Family::Filter)
        .map(|_| slot.feedback)
}

pub(crate) struct ControlSmoothers {
    pub(crate) smoothers: Vec<ControlSmoother>,
    active: Vec<usize>,
    /// Rows the engine settles at a morph landing, resolved at construction.
    kick_level: usize,
    bass_level: usize,
}

impl ControlSmoothers {
    pub(crate) fn new(c: &FluidControls) -> Self {
        let mut seen = BTreeSet::new();
        let smoothers: Vec<_> = all_specs()
            .filter(|spec| spec.kind.smooths_audio())
            .filter(|spec| seen.insert(spec.id))
            .map(|spec| {
                let mut smoother = ControlSmoother::for_spec(spec, (spec.get)(c));
                let slot = smoother
                    .slot_address
                    .and_then(|address| address.resolve(c))
                    .map(|(slot, _)| slot);
                smoother.slot_kind = slot.map(|slot| slot.kind);
                smoother.slot_filter_type = slot.and_then(filter_type_for_slot);
                smoother
            })
            .collect();
        let index_of = |id: &str| {
            smoothers
                .iter()
                .position(|smoother: &ControlSmoother| smoother.spec.id == id)
                .unwrap_or_else(|| panic!("{id} is a registered smoothed control"))
        };
        Self {
            active: Vec::with_capacity(smoothers.len()),
            kick_level: index_of("kick.level"),
            bass_level: index_of("bass.level"),
            smoothers,
        }
    }

    pub(crate) fn set_targets(&mut self, c: &FluidControls, sample_rate: f32) {
        let ramp_samples = (LEVEL_RAMP_MS * 0.001 * sample_rate).round() as u32;
        self.active.clear();
        for (index, smoother) in self.smoothers.iter_mut().enumerate() {
            let snapshot_value = (smoother.spec.get)(c);
            let row = smoother.slot_address.and_then(|address| address.resolve(c));
            let slot = row.map(|(slot, _)| slot);
            let slot_kind = slot.map(|slot| slot.kind);
            let slot_filter_type = slot.and_then(filter_type_for_slot);
            if slot_kind != smoother.slot_kind
                || slot_filter_type != smoother.slot_filter_type
                || (snapshot_value != smoother.ramp.target
                    && (!smoother.spec.contextual_row(row).kind.smooths_audio()
                        || is_swing_amount(row)))
            {
                // Slot fields change meaning with the module. Timing and
                // discrete values (including Swing) must arrive immediately;
                // Delay owns its tap crossfade instead of gliding its time.
                smoother.ramp = EasedRamp::settled(snapshot_value);
                smoother.slot_kind = slot_kind;
                smoother.slot_filter_type = slot_filter_type;
            }
            smoother.set_target(snapshot_value, ramp_samples);
            smoother.idle =
                smoother.ramp.samples_remaining == 0 && smoother.ramp.target == snapshot_value;
            if !smoother.idle {
                self.active.push(index);
            }
        }
    }

    /// Jump one row to `value` and write it to the working controls, so a
    /// settle between session changes is heard on this very sample.
    fn settle(&mut self, index: usize, value: f32, working: &mut FluidControls) {
        let smoother = &mut self.smoothers[index];
        smoother.ramp = EasedRamp::settled(value);
        smoother.idle = true;
        (smoother.spec.set)(working, value);
    }

    pub(crate) fn apply(&mut self, next: &mut FluidControls) {
        // Most knobs are stationary. The control-rate pass prepares this
        // bounded list, so audio visits only ramps that need a sample write.
        for &index in &self.active {
            let smoother = &mut self.smoothers[index];
            if smoother.idle {
                continue;
            }
            (smoother.spec.set)(next, smoother.next());
        }
    }

    #[cfg(test)]
    pub(crate) fn next_controls(&mut self, c: &FluidControls) -> FluidControls {
        let mut next = c.clone();
        self.apply(&mut next);
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

    pub(crate) fn restart(&mut self) {
        self.beat = 0.0;
    }

    /// A stopped clock holds its beat, so every beat-derived value (LFO
    /// positions, the auto morph, grid phases) waits where it was and
    /// resumes from there. Tempo still glides toward its target.
    pub(crate) fn tick(&mut self, target_bpm: f32, transport: Transport) -> TimingContext {
        let target_bpm = f64::from(target_bpm.clamp(MASTER_BPM_MIN, MASTER_BPM_MAX));
        self.bpm += (target_bpm - self.bpm) * self.smoothing_coeff;

        let timing = TimingContext {
            transport,
            ..TimingContext::new(self.sample_rate, self.bpm, self.beat)
        };
        if transport == Transport::Playing {
            self.beat += self.bpm / (60.0 * self.sample_rate);
        }
        timing
    }
}

/// Whether the beat clock runs. Stopped fires no new grid hit anywhere and
/// releases sustaining chords, while the audio path keeps running so
/// releases, reverb, and delay tails ring out. A live-session value only:
/// loading a song always starts it playing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Transport {
    #[default]
    Playing,
    Stopped,
}

/// One sample's worth of transport: where the beat clock stands, whether it
/// is running, and the rates needed to turn musical time into samples. Every
/// voice's `next` reads it.
#[derive(Clone, Copy)]
pub(crate) struct TimingContext {
    pub(crate) sample_rate: f64,
    pub(crate) bpm: f64,
    pub(crate) beat: f64,
    pub(crate) transport: Transport,
    pub(crate) drunken_amount: f32,
    pub(crate) drunken_pace: f32,
    pub(crate) drunken_phase_beat: f64,
    pub(crate) groove_seed: u64,
    /// Auto song-landing anchor shared by chord followers; normal playback uses its cursor.
    pub(crate) morph_phrase_start: Option<f64>,
}

impl TimingContext {
    /// A playing transport at `beat`. Only `TempoClock::tick` stops one.
    pub(crate) fn new(sample_rate: f64, bpm: f64, beat: f64) -> Self {
        Self {
            sample_rate: sample_rate.max(1.0),
            bpm: bpm.max(1.0),
            beat,
            transport: Transport::Playing,
            drunken_amount: 0.0,
            drunken_pace: 7.0,
            drunken_phase_beat: 0.0,
            groove_seed: 0,
            morph_phrase_start: None,
        }
    }

    fn with_groove_seed(mut self, seed: u64) -> Self {
        self.groove_seed = seed;
        self
    }

    /// Replaces the pre-trigger Drunken values for one layer while preserving
    /// the shared transport and saved wave phase.
    pub(crate) fn with_drunken(mut self, amount: f32, pace: f32) -> Self {
        self.drunken_amount = amount;
        self.drunken_pace = pace;
        self
    }

    /// Tests that predict a voice's step spacing compute it from the same
    /// transport the engine plays; production voices advance sample by sample
    /// and never need the conversion.
    #[cfg(test)]
    pub(crate) fn samples_per_beat(self) -> f64 {
        self.sample_rate * 60.0 / self.bpm
    }

    #[cfg(test)]
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

#[derive(Clone, Copy, Debug)]
pub(crate) struct GridSpec {
    pub(crate) interval_beats: f64,
    pub(crate) offset_beats: f64,
    /// Beats each odd slot is pushed late; 0 on straight or chord-rate grids.
    swing_delay_beats: f64,
    drunken_amount: f32,
    drunken_pace: u8,
    drunken_phase_slots: u64,
    drunken_max_beats: f64,
    groove_seed: u64,
}

impl PartialEq for GridSpec {
    fn eq(&self, other: &Self) -> bool {
        self.interval_beats == other.interval_beats
            && self.offset_beats == other.offset_beats
            && self.swing_delay_beats == other.swing_delay_beats
            && self.drunken_amount == other.drunken_amount
            && self.drunken_pace == other.drunken_pace
            && self.drunken_phase_slots == other.drunken_phase_slots
            && self.groove_seed == other.groove_seed
    }
}

impl GridSpec {
    pub(crate) fn new(interval_beats: f32, offset_beats: f32, swing: f32) -> Self {
        Self::new_grooved(
            interval_beats,
            offset_beats,
            swing,
            TimingContext::new(1.0, 120.0, 0.0),
        )
    }

    fn new_grooved(
        interval_beats: f32,
        offset_beats: f32,
        swing: f32,
        timing: TimingContext,
    ) -> Self {
        let interval_beats = f64::from(interval_beats).max(1.0 / 64.0);
        let offset_beats = f64::from(offset_beats).rem_euclid(interval_beats);
        let swing_fraction = if interval_beats <= SWING_MAX_INTERVAL_BEATS {
            f64::from(swing.clamp(0.0, 1.0)) * SWING_MAX_FRACTION
        } else {
            0.0
        };
        Self {
            interval_beats,
            offset_beats,
            swing_delay_beats: swing_fraction * interval_beats,
            drunken_amount: timing.drunken_amount,
            drunken_pace: timing.drunken_pace.round().clamp(4.0, 12.0) as u8,
            drunken_phase_slots: ((timing.drunken_phase_beat - offset_beats) / interval_beats)
                .ceil()
                .max(0.0) as u64,
            // The full-scale lag is 10% of a beat, capped at 35% of this
            // voice's trigger interval so even full swing remains ordered.
            drunken_max_beats: f64::from(timing.drunken_amount.clamp(0.0, 1.0))
                * (0.10_f64).min(interval_beats * 0.35),
            groove_seed: timing.groove_seed,
        }
    }

    /// Beat of grid slot `slot`, with odd slots pushed late by the swing delay.
    /// Strictly increasing in `slot` since the delay is always < one interval.
    fn swung_beat(self, slot: u64) -> f64 {
        let base = self.offset_beats + slot as f64 * self.interval_beats;
        let swing = if slot % 2 == 1 {
            self.swing_delay_beats
        } else {
            0.0
        };
        let drunken = if self.drunken_max_beats > 0.0 {
            self.drunken_wave(slot) * self.drunken_max_beats
        } else {
            0.0
        };
        base + swing + drunken
    }

    /// A sequence of triangular waves, each at least four triggers long.
    /// The 31-trigger cells keep their boundaries off the four-beat meter;
    /// a seeded choice of wave lengths in each cell changes the pace without
    /// accumulating random state or depending on the order of grid queries.
    fn drunken_wave(self, slot: u64) -> f64 {
        const CELL: u64 = 31;
        let phase = splitmix64_mix(self.groove_seed.wrapping_mul(0x9e3779b97f4a7c15)) % CELL;
        let shifted = slot
            .wrapping_add(self.drunken_phase_slots)
            .wrapping_add(phase);
        let cell = shifted / CELL;
        let within = shifted % CELL;
        let mut start = 0;
        let mut wave = 0_u64;
        loop {
            let remaining = CELL - start;
            let random = splitmix64_mix(
                cell ^ self.groove_seed.wrapping_mul(0xbf58476d1ce4e5b9)
                    ^ wave.wrapping_mul(0x94d049bb133111eb),
            );
            let preferred =
                (i64::from(self.drunken_pace) + (random % 5) as i64 - 2).clamp(4, 14) as u64;
            let length = if remaining <= preferred + 3 {
                remaining
            } else {
                preferred
            };
            if within < start + length {
                let progress = (within - start) as f64 / length as f64;
                return 1.0 - (2.0 * progress - 1.0).abs();
            }
            start += length;
            wave += 1;
        }
    }

    pub(crate) fn hit_at_or_after(self, beat: f64) -> GridHit {
        if beat <= self.offset_beats {
            return self.hit_for_slot(0);
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
                return self.hit_for_slot(slot);
            }
            slot += 1;
        }
    }

    fn hit_for_slot(self, slot: u64) -> GridHit {
        GridHit {
            beat: self.swung_beat(slot),
            slot,
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
    fn a_zero_layer_drunken_matches_the_default_grid() {
        let default = TimingContext::new(48_000.0, 120.0, 0.0);
        let zeroed = default.with_drunken(0.0, 7.0);
        assert_eq!(
            GridSpec::new_grooved(0.25, 0.0, 0.0, default),
            GridSpec::new_grooved(0.25, 0.0, 0.0, zeroed)
        );
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

    #[test]
    fn drunken_waves_vary_by_layer_and_stay_in_order() {
        let mut timing = TimingContext::new(48_000.0, 120.0, 0.0);
        timing.drunken_amount = 1.0;
        timing.groove_seed = 3;
        let grid = GridSpec::new_grooved(0.25, 0.0, 1.0, timing);
        let again = GridSpec::new_grooved(0.25, 0.0, 1.0, timing);
        timing.groove_seed = 4;
        let other_voice = GridSpec::new_grooved(0.25, 0.0, 1.0, timing);
        let mut differs = false;
        for slot in 0..128 {
            let base = slot as f64 * 0.25 + if slot % 2 == 1 { 0.125 } else { 0.0 };
            let hit = grid.swung_beat(slot);
            assert!((base..=base + grid.drunken_max_beats).contains(&hit));
            assert_eq!(hit, again.swung_beat(slot));
            differs |= hit != other_voice.swung_beat(slot);
            if slot > 0 {
                assert!(hit > grid.swung_beat(slot - 1));
            }
        }
        assert!(differs, "each voice needs its own timing variation");
        assert_eq!(grid.hit_at_or_after(0.0).beat, grid.swung_beat(0));
        // A cycle has a real rise and fall, rather than independent jitter.
        let peaks = (1..127)
            .filter(|&slot| {
                grid.drunken_wave(slot) > grid.drunken_wave(slot - 1)
                    && grid.drunken_wave(slot) > grid.drunken_wave(slot + 1)
            })
            .count();
        assert!((8..=32).contains(&peaks));
    }

    #[test]
    fn fast_grid_caps_drunken_delay_and_still_fires_every_slot() {
        let mut timing = TimingContext::new(48_000.0, 200.0, 0.0);
        timing.drunken_amount = 1.0;
        timing.groove_seed = 2;
        let spec = GridSpec::new_grooved(0.125, 0.0, 1.0, timing);
        assert_eq!(spec.drunken_max_beats, 0.125 * 0.35);
        let mut trigger = GridTrigger::new();
        let mut hits = 0;
        for sample in 0..48_000 {
            let mut timing = TimingContext::new(48_000.0, 200.0, sample as f64 / 14_400.0);
            timing.drunken_amount = 1.0;
            timing.groove_seed = 2;
            hits += usize::from(trigger.pop_swung(timing, 0.125, 0.0, 1.0));
        }
        assert_eq!(hits, 27);
    }

    #[test]
    fn drunken_waves_have_a_four_hit_minimum_and_pace_changes_their_rate() {
        let mut timing = TimingContext::new(48_000.0, 120.0, 0.0);
        timing.drunken_amount = 1.0;
        timing.groove_seed = 1;
        let mut zero_counts = Vec::new();
        for pace in [4.0, 7.0, 12.0] {
            timing.drunken_pace = pace;
            let grid = GridSpec::new_grooved(0.25, 0.0, 0.0, timing);
            let zeros: Vec<_> = (0..620)
                .filter(|&slot| grid.drunken_wave(slot) == 0.0)
                .collect();
            assert!(zeros.windows(2).all(|pair| pair[1] - pair[0] >= 4));
            zero_counts.push(zeros.len());
        }
        assert!(zero_counts[0] > zero_counts[1]);
        assert!(zero_counts[1] > zero_counts[2]);
    }

    #[test]
    fn drunken_depth_tracks_tempo_and_saved_phase() {
        let mut timing = TimingContext::new(48_000.0, 80.0, 0.0);
        timing.drunken_amount = 0.5;
        timing.groove_seed = 3;
        let slow = GridSpec::new_grooved(0.5, 0.0, 0.0, timing);
        timing.bpm = 160.0;
        let fast = GridSpec::new_grooved(0.5, 0.0, 0.0, timing);
        assert_eq!(slow.drunken_max_beats, 0.05);
        assert_eq!(slow.drunken_max_beats, fast.drunken_max_beats);
        assert!((slow.drunken_max_beats * 60_000.0 / 80.0 - 37.5).abs() < 1e-9);
        assert!((fast.drunken_max_beats * 60_000.0 / 160.0 - 18.75).abs() < 1e-9);

        timing.drunken_phase_beat = 8.0;
        let restored = GridSpec::new_grooved(0.5, 0.0, 0.0, timing);
        for slot in 0..64 {
            assert_eq!(restored.drunken_wave(slot), fast.drunken_wave(slot + 16));
        }
        timing.drunken_phase_beat = 8.2;
        let offset_restored = GridSpec::new_grooved(0.5, 0.1, 0.0, timing);
        timing.drunken_phase_beat = 0.0;
        let offset_original = GridSpec::new_grooved(0.5, 0.1, 0.0, timing);
        for slot in 0..64 {
            assert_eq!(
                offset_restored.drunken_wave(slot),
                offset_original.drunken_wave(slot + 17)
            );
        }
    }

    #[test]
    fn kick_drunken_half_weight_probe() {
        let mut timing = TimingContext::new(48_000.0, 120.0, 0.0);
        timing.groove_seed = 3;
        timing.drunken_amount = 1.0;
        let full = GridSpec::new_grooved(0.5, 0.0, 0.0, timing);
        timing.drunken_amount = 0.5;
        let half = GridSpec::new_grooved(0.5, 0.0, 0.0, timing);
        let mut sum_ms = [0.0; 2];
        let mut max_ms = [0.0_f64; 2];
        for slot in 0..32 {
            let straight = slot as f64 * 0.5;
            for (index, grid) in [full, half].into_iter().enumerate() {
                let lag_ms = (grid.swung_beat(slot) - straight) * 500.0;
                sum_ms[index] += lag_ms;
                max_ms[index] = max_ms[index].max(lag_ms);
            }
        }
        assert!((sum_ms[0] - 2.0 * sum_ms[1]).abs() < 1e-9);
        assert!((max_ms[0] - 2.0 * max_ms[1]).abs() < 1e-9);
        println!(
            "Kick Drunken at 120 BPM, 0.5-beat grid, 32 hits: full mean/max {:.2}/{:.2} ms; half mean/max {:.2}/{:.2} ms",
            sum_ms[0] / 32.0,
            max_ms[0],
            sum_ms[1] / 32.0,
            max_ms[1]
        );
    }

    #[test]
    fn tempo_glide_keeps_the_same_drunken_grid_schedule() {
        let mut timing = TimingContext::new(48_000.0, 100.0, 0.0);
        timing.drunken_amount = 1.0;
        timing.groove_seed = 2;
        let slow = GridSpec::new_grooved(0.25, 0.0, 0.0, timing);
        timing.bpm = 200.0;
        let fast = GridSpec::new_grooved(0.25, 0.0, 0.0, timing);
        assert_eq!(slow, fast);
        assert_eq!(slow.drunken_max_beats, fast.drunken_max_beats);
        let mut trigger = GridTrigger::new();
        let mut timing = TimingContext::new(48_000.0, 100.0, slow.swung_beat(0));
        timing.drunken_amount = 1.0;
        timing.groove_seed = 2;
        assert!(trigger.pop_swung(timing, 0.25, 0.0, 0.0));
        assert_eq!(trigger.next_hit.unwrap().slot, 1);
        timing.bpm = 200.0;
        timing.beat = trigger.next_hit.unwrap().beat;
        assert!(trigger.pop_swung(timing, 0.25, 0.0, 0.0));
        assert_eq!(trigger.next_hit.unwrap().slot, 2);
    }
}

pub(crate) const GRID_BEAT_EPSILON: f64 = 1e-9;

#[derive(Clone, Copy, Debug)]
pub(crate) struct GridHit {
    pub(crate) beat: f64,
    slot: u64,
}

pub(crate) struct GridTrigger {
    pub(crate) spec: Option<GridSpec>,
    pub(crate) next_hit: Option<GridHit>,
    scheduled_spec: Option<GridSpec>,
    /// Beat of the most recently emitted hit. A live grid reshape may never
    /// reschedule inside half an interval of this beat.
    last_hit_beat: Option<f64>,
}

impl GridTrigger {
    pub(crate) fn new() -> Self {
        Self {
            spec: None,
            next_hit: None,
            scheduled_spec: None,
            last_hit_beat: None,
        }
    }

    /// An unswung `pop_swung`, for the grid tests.
    #[cfg(test)]
    pub(crate) fn pop(
        &mut self,
        timing: TimingContext,
        interval_beats: f32,
        offset_beats: f32,
    ) -> bool {
        self.pop_swung(timing, interval_beats, offset_beats, 0.0)
    }

    /// Earliest beat a live reshape may pull a hit to: at or after the
    /// playhead, and never within half an interval of the last hit. Regular
    /// drunken progression advances by slot so tight swung hits are preserved.
    fn earliest_hit(&self, spec: GridSpec, beat: f64) -> f64 {
        let floor = self
            .last_hit_beat
            .map_or(f64::NEG_INFINITY, |b| b + spec.interval_beats * 0.5);
        (beat + GRID_BEAT_EPSILON).max(floor)
    }

    /// Swing the voice's odd subdivisions and apply its deterministic drunken
    /// delay from `timing`, if present.
    /// A stopped transport fires nothing and leaves the schedule untouched;
    /// a reshape made while stopped lands on the first playing sample.
    pub(crate) fn pop_swung(
        &mut self,
        timing: TimingContext,
        interval_beats: f32,
        offset_beats: f32,
        swing: f32,
    ) -> bool {
        if timing.transport == Transport::Stopped {
            return false;
        }
        let spec = GridSpec::new_grooved(interval_beats, offset_beats, swing, timing);
        if self.spec != Some(spec) {
            self.spec = Some(spec);
            match self.next_hit {
                None => {
                    self.next_hit = Some(spec.hit_at_or_after(timing.beat));
                    self.scheduled_spec = Some(spec);
                }
                // Pull the scheduled hit earlier when the reshaped grid lands
                // sooner, so a denser grid isn't starved — but never earlier than
                // `earliest_hit`, which rejects a re-fire of the slot that just
                // sounded while still admitting the genuinely-next denser slot.
                Some(hit) => {
                    let candidate = spec.hit_at_or_after(self.earliest_hit(spec, timing.beat));
                    if candidate.beat < hit.beat {
                        self.next_hit = Some(candidate);
                        self.scheduled_spec = Some(spec);
                    }
                }
            }
        }

        let Some(next_hit) = self.next_hit else {
            return false;
        };
        if timing.beat + GRID_BEAT_EPSILON >= next_hit.beat {
            self.last_hit_beat = Some(next_hit.beat);
            self.next_hit = Some(
                if spec.drunken_max_beats > 0.0 && self.scheduled_spec == Some(spec) {
                    spec.hit_for_slot(next_hit.slot + 1)
                } else {
                    spec.hit_at_or_after(self.earliest_hit(spec, timing.beat))
                },
            );
            self.scheduled_spec = Some(spec);
            true
        } else {
            false
        }
    }
}

// ============================================================
// Master bus (tilt EQ and final level)
// ============================================================

pub(crate) struct MasterBus {
    pub(crate) tone_l: f32,
    pub(crate) tone_r: f32,
}

impl MasterBus {
    pub(crate) fn new(_c: &MasterControls, _sample_rate: f32) -> Self {
        Self {
            tone_l: 0.0,
            tone_r: 0.0,
        }
    }

    pub(crate) fn set_controls(&mut self, _c: &MasterControls, _sample_rate: f32) {}

    pub(crate) fn process(&mut self, mut l: f32, mut r: f32, c: &MasterControls) -> (f32, f32) {
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

        (
            (l * c.level).clamp(-0.95, 0.95),
            (r * c.level).clamp(-0.95, 0.95),
        )
    }
}

#[cfg(test)]
mod state_tests {
    use super::*;

    const RATE: f32 = 8000.0;

    /// Four 2-beat chords: an 8-beat phrase, short enough to play through.
    fn short_song(bpm: f32) -> SongState {
        let mut song = SongState::default();
        song.controls.master.bpm = bpm;
        song.controls.pad.chord_bars = 0.5;
        song.controls.pad.chord_count = 4.0;
        song.controls.pad.chord_offset = 0.0;
        song
    }

    fn engine_for(song: &SongState) -> (LiveSession, FluidEngine) {
        let session = LiveSession::new(LiveSessionSnapshot::from_song(song));
        let engine = FluidEngine::new(
            RATE,
            session.clone(),
            no_morph(),
            Arc::new(FluidTelemetry::default()),
        );
        (session, engine)
    }

    fn queue(session: &LiveSession, index: usize, destination: &SongState) {
        let snapshot = Arc::new(LiveSessionSnapshot::from_song(destination));
        session.update(|s| {
            s.pending_state = Some(PendingState {
                index,
                snapshot: snapshot.clone(),
            })
        });
    }

    fn beats_per_sample(engine: &FluidEngine) -> f64 {
        engine.tempo.bpm / (60.0 * f64::from(RATE))
    }

    fn play_until_beat(engine: &mut FluidEngine, beat: f64) {
        while engine.tempo.beat < beat {
            engine.next_stereo();
        }
    }

    /// Plays until a state lands and returns the beat the landing sample
    /// started on, i.e. the beat the old song had reached.
    fn play_to_landing(engine: &mut FluidEngine, session: &LiveSession) -> f64 {
        for _ in 0..400_000 {
            let before = engine.tempo.beat;
            let generation = session.load().state_revision;
            engine.next_stereo();
            if session.load().state_revision != generation {
                return before;
            }
        }
        panic!("the queued state never landed");
    }

    fn assert_on_wrap(landed_at: f64, wrap: f64, engine: &FluidEngine) {
        let tolerance = beats_per_sample(engine) + GRID_BEAT_EPSILON;
        assert!(
            (landed_at - wrap).abs() <= tolerance,
            "landed at beat {landed_at}, the phrase wraps at {wrap}"
        );
    }

    #[test]
    fn a_queued_state_lands_exactly_on_the_phrase_wrap_and_starts_at_beat_zero() {
        let (session, mut engine) = engine_for(&short_song(120.0));
        let destination = short_song(90.0);
        play_until_beat(&mut engine, 3.0);
        queue(&session, 1, &destination);
        // Not at the next bar (beat 4), not at the next chord (beat 4): the
        // phrase wraps at 8.
        let landed_at = play_to_landing(&mut engine, &session);
        assert_on_wrap(landed_at, 8.0, &engine);
        assert_eq!(session.load().current_state, Some(1));
        assert!(engine.tempo.beat < 0.01, "destination starts at beat zero");
        assert_eq!(session.load().controls.master.bpm, 90.0);
        assert!(session.load().pending_state.is_none());
        // The destination's own phrase is what the next queue waits for.
        play_until_beat(&mut engine, 1.0);
        queue(&session, 0, &short_song(120.0));
        let again = play_to_landing(&mut engine, &session);
        assert_on_wrap(again, 8.0, &engine);
        assert_eq!(session.load().previous_state, Some(1));
    }

    /// A hold across a landing must not loop the song swap: the cut is hidden
    /// by the engine's fade only once, so a loop that carried it would click
    /// on every repeat. Worst jump is compared with the same landing dry.
    #[test]
    fn beat_repeat_held_across_a_state_landing_adds_no_click() {
        let worst_jump = |hold: bool, hold_at: f64| {
            let (session, mut engine) = engine_for(&short_song(120.0));
            engine.reseed(5);
            play_until_beat(&mut engine, hold_at);
            if hold {
                let now = session.audio_seconds();
                session.update(|s| s.gestures.press(GestureKind::BeatRepeat, now));
            }
            queue(&session, 1, &short_song(90.0));
            let landed_at = play_to_landing(&mut engine, &session);
            assert!(landed_at > 7.9, "landed at {landed_at}");
            let mut previous = engine.next_stereo();
            let mut worst = 0.0_f32;
            // Past the landing fade, across several repeats of the new song.
            for frame in 0..RATE as usize * 8 {
                let next = engine.next_stereo();
                if frame > (RATE * 0.05) as usize {
                    worst = worst.max((next.0 - previous.0).abs());
                }
                previous = next;
            }
            worst
        };
        for hold_at in [2.5, 7.2] {
            let dry = worst_jump(false, hold_at);
            let held = worst_jump(true, hold_at);
            assert!(
                held < dry * 2.0 + 0.01,
                "hold from beat {hold_at}: dry worst {dry}, held worst {held}"
            );
        }
    }

    #[test]
    fn landing_follows_the_destinations_phrase_length_not_the_sources() {
        let (session, mut engine) = engine_for(&short_song(120.0));
        let mut long = short_song(120.0);
        long.controls.pad.chord_count = 2.0;
        long.controls.pad.chord_bars = 1.0;
        play_until_beat(&mut engine, 1.0);
        queue(&session, 0, &long);
        assert_on_wrap(play_to_landing(&mut engine, &session), 8.0, &engine);
        // Now playing the 8-beat phrase of 2 four-beat chords.
        play_until_beat(&mut engine, 1.0);
        queue(&session, 1, &short_song(120.0));
        assert_on_wrap(play_to_landing(&mut engine, &session), 8.0, &engine);
    }

    #[test]
    fn cancelling_before_the_wrap_keeps_the_song_and_replacing_retargets_it() {
        let (session, mut engine) = engine_for(&short_song(120.0));
        play_until_beat(&mut engine, 1.0);
        queue(&session, 1, &short_song(100.0));
        play_until_beat(&mut engine, 6.0);
        session.update(|s| s.pending_state = None);
        play_until_beat(&mut engine, 9.0);
        assert_eq!(session.load().current_state, None);
        assert!(engine.state_gain > 0.99, "cancel recovers the fade");
        // A replacement queued before the wrap lands once, at that wrap.
        queue(&session, 2, &short_song(100.0));
        play_until_beat(&mut engine, 12.0);
        queue(&session, 3, &short_song(80.0));
        let landed_at = play_to_landing(&mut engine, &session);
        assert_on_wrap(landed_at, 16.0, &engine);
        assert_eq!(session.load().current_state, Some(3));
        assert_eq!(session.load().controls.master.bpm, 80.0);
    }

    #[test]
    fn queued_on_the_last_beat_it_lands_at_the_imminent_wrap_not_a_phrase_later() {
        let (session, mut engine) = engine_for(&short_song(120.0));
        play_until_beat(&mut engine, 7.0);
        queue(&session, 1, &short_song(90.0));
        assert_on_wrap(play_to_landing(&mut engine, &session), 8.0, &engine);
        // And a few samples before the wrap, inside the fade window.
        let (session, mut engine) = engine_for(&short_song(120.0));
        let samples_before = 3.0;
        let beat = 8.0 - samples_before * beats_per_sample(&engine);
        play_until_beat(&mut engine, beat);
        queue(&session, 1, &short_song(90.0));
        assert_on_wrap(play_to_landing(&mut engine, &session), 8.0, &engine);
        // The fade stays continuous even with no room to ramp.
        assert!(engine.state_gain.is_finite());
    }

    /// A queue the engine sees by the sample that crosses the wrap belongs to
    /// the phrase that ends there; one made after that sample belongs to the
    /// next phrase.
    #[test]
    fn a_queue_on_the_boundary_sample_belongs_to_the_ending_phrase_and_one_after_to_the_next() {
        let (session, mut engine) = engine_for(&short_song(120.0));
        while engine.tempo.beat + GRID_BEAT_EPSILON < 8.0 {
            engine.next_stereo();
        }
        // The next call crosses the wrap.
        queue(&session, 1, &short_song(90.0));
        assert_on_wrap(play_to_landing(&mut engine, &session), 8.0, &engine);

        let (session, mut engine) = engine_for(&short_song(120.0));
        while engine.tempo.beat + GRID_BEAT_EPSILON < 8.0 {
            engine.next_stereo();
        }
        engine.next_stereo(); // the crossing sample, nothing queued
        assert_eq!(session.load().current_state, None);
        queue(&session, 1, &short_song(90.0));
        assert_on_wrap(play_to_landing(&mut engine, &session), 16.0, &engine);
    }

    #[test]
    fn editing_chord_count_while_queued_moves_the_landing_with_the_phrase() {
        let (session, mut engine) = engine_for(&short_song(120.0));
        play_until_beat(&mut engine, 1.0);
        queue(&session, 1, &short_song(90.0));
        play_until_beat(&mut engine, 2.5);
        // Halving the window while on chord 2 of 4: the phrase now ends at 4.
        session.update(|s| s.controls.pad.chord_count = 2.0);
        assert_on_wrap(play_to_landing(&mut engine, &session), 4.0, &engine);
    }

    #[test]
    fn editing_chord_length_while_queued_follows_the_boundaries_as_played() {
        let (session, mut engine) = engine_for(&short_song(120.0));
        play_until_beat(&mut engine, 0.5);
        queue(&session, 1, &short_song(90.0));
        // The sounding 2-beat chord finishes at its old length; the three
        // after it are 4 beats each: 2 + 3 * 4 = 14.
        session.update(|s| s.controls.pad.chord_bars = 1.0);
        assert_on_wrap(play_to_landing(&mut engine, &session), 14.0, &engine);
    }

    #[test]
    fn editing_chord_offset_while_queued_follows_the_phrase_as_played() {
        let (session, mut engine) = engine_for(&short_song(120.0));
        play_until_beat(&mut engine, 0.5);
        queue(&session, 1, &short_song(90.0));
        // Offset 2 removes the sounding slot 0 from the window; its old
        // successor (slot 1) is not in it either, so the new window starts at
        // its first chord at the next boundary: that boundary is the wrap.
        session.update(|s| s.controls.pad.chord_offset = 2.0);
        assert_on_wrap(play_to_landing(&mut engine, &session), 2.0, &engine);
    }

    #[test]
    fn a_tempo_change_during_the_wait_does_not_move_the_landing_off_the_wrap() {
        let (session, mut engine) = engine_for(&short_song(120.0));
        play_until_beat(&mut engine, 1.0);
        queue(&session, 1, &short_song(90.0));
        play_until_beat(&mut engine, 3.0);
        session.update(|s| s.controls.master.bpm = 60.0);
        play_until_beat(&mut engine, 5.0);
        session.update(|s| s.controls.master.bpm = 180.0);
        assert_on_wrap(play_to_landing(&mut engine, &session), 8.0, &engine);
    }

    #[test]
    fn a_queue_survives_a_stop_and_lands_at_the_first_phrase_end_after_restart() {
        let (session, mut engine) = engine_for(&short_song(120.0));
        let mut effects = EffectExecutor::seeded(
            session.clone(),
            AutoControls::new(no_morph(), vec![], 64),
            1,
        );
        play_until_beat(&mut engine, 1.0);
        queue(&session, 1, &short_song(90.0));
        session.update(|s| s.transport = Transport::Stopped);
        for _ in 0..20_000 {
            engine.next_stereo();
        }
        assert!(session.load().pending_state.is_some());
        assert_eq!(session.load().current_state, None);
        effects.start_transport();
        // Restart is beat zero of a fresh phrase: the first phrase end after
        // it is a whole phrase away, where the bar rule's first new bar was
        // beat 4.
        assert_on_wrap(play_to_landing(&mut engine, &session), 8.0, &engine);
    }

    /// A held or latched Vamp narrows the pad to one chord, so the shared
    /// boundary is that chord's end: the queue does not wait forever.
    #[test]
    fn under_a_vamp_the_phrase_end_is_the_vamped_chord_end() {
        let (session, mut engine) = engine_for(&short_song(120.0));
        play_until_beat(&mut engine, 2.5);
        session.update(|s| {
            s.vamp = Some(VampState::start(2.0, 1));
            s.vamp_latched = true;
        });
        play_until_beat(&mut engine, 3.0);
        queue(&session, 1, &short_song(90.0));
        let landed_at = play_to_landing(&mut engine, &session);
        assert_on_wrap(landed_at, 4.0, &engine);
    }

    #[test]
    fn the_published_phrase_end_is_the_same_beat_the_state_lands_on() {
        let (session, mut engine) = engine_for(&short_song(120.0));
        assert_eq!(session.audio_phrase_end(), None);
        play_until_beat(&mut engine, 1.0);
        queue(&session, 1, &short_song(90.0));
        engine.next_stereo();
        assert_eq!(session.audio_phrase_end(), Some(8.0));
        play_until_beat(&mut engine, 5.0);
        assert_eq!(session.audio_phrase_end(), Some(8.0));
        let landed_at = play_to_landing(&mut engine, &session);
        assert_on_wrap(landed_at, 8.0, &engine);
        engine.next_stereo();
        assert_eq!(session.audio_phrase_end(), None);
    }

    #[test]
    fn longest_wait_and_song_one_wait() {
        // 8 chords of 64 bars (256 beats) at the slowest tempo.
        let longest = PadControls {
            chord_bars: 64.0,
            chord_count: 8.0,
            ..PadControls::default()
        };
        let window = ChordWindow::requested(&longest);
        let beats = window.phrase_beats(&longest);
        assert_eq!(beats, 2048.0);
        assert_eq!(beats / 4.0, 512.0);
        assert_eq!(beats * 60.0 / f64::from(MASTER_BPM_MIN), 4096.0);
        // Song 1's own defaults, read from the built-in.
        let song = decode_auto_states().remove(0);
        let pad = &song.controls.pad;
        let phrase = ChordWindow::requested(pad).phrase_beats(pad);
        let bpm = f64::from(song.controls.master.bpm);
        assert_eq!((pad.chord_bars, pad.chord_count, bpm), (4.0, 8.0, 82.0));
        assert_eq!(phrase, 128.0);
        assert_eq!(phrase / 4.0, 32.0);
        assert!((phrase * 60.0 / bpm - 93.66).abs() < 0.01);
    }

    #[test]
    fn cancellation_during_fade_recovers_without_gain_jump() {
        let (session, mut engine) = engine_for(&short_song(120.0));
        let mut peak = 0.0_f32;
        for _ in 0..3000 {
            let sample = engine.next_stereo();
            peak = peak.max(sample.0.abs()).max(sample.1.abs());
        }
        assert!(peak > 0.000001);
        // Queue just before the wrap so the fade-out is under way.
        let beat = 8.0 - 20.0 * beats_per_sample(&engine);
        play_until_beat(&mut engine, beat);
        queue(&session, 0, &short_song(120.0));
        for _ in 0..1000 {
            engine.next_stereo();
            if engine.state_gain < 0.7 {
                break;
            }
        }
        assert!(engine.state_gain < 0.7);
        session.update(|s| s.pending_state = None);
        let mut previous = engine.state_gain;
        for _ in 0..400 {
            engine.next_stereo();
            assert!((engine.state_gain - previous).abs() <= 0.026);
            previous = engine.state_gain;
        }
        assert!(engine.state_gain > 0.99);
        assert_eq!(session.load().current_state, None);
    }
}

#[cfg(test)]
mod push_tests {
    use super::*;

    const RATE: f32 = 8000.0;

    /// Song 1's shape at a quick tempo: A minor Drift, one chord per 4 bars,
    /// with pushes quantized to the bar so landings are easy to place.
    fn engine() -> (LiveSession, FluidEngine, EffectExecutor) {
        engine_quantized(PushQuantize::OneBar)
    }

    fn engine_quantized(quantize: PushQuantize) -> (LiveSession, FluidEngine, EffectExecutor) {
        let mut song = SongState::default();
        song.controls.master.bpm = 120.0;
        let (session, engine, effects) = engine_for(&song);
        session.update(|s| s.push.quantize = quantize);
        (session, engine, effects)
    }

    fn engine_for(song: &SongState) -> (LiveSession, FluidEngine, EffectExecutor) {
        let session = LiveSession::new(LiveSessionSnapshot::from_song(song));
        let engine = FluidEngine::new(
            RATE,
            session.clone(),
            no_morph(),
            Arc::new(FluidTelemetry::default()),
        );
        let effects = EffectExecutor::seeded(
            session.clone(),
            AutoControls::new(no_morph(), vec![], 64),
            1,
        );
        (session, engine, effects)
    }

    fn play_until_beat(engine: &mut FluidEngine, beat: f64) {
        while engine.tempo.beat < beat {
            engine.next_stereo();
        }
    }

    fn push(effects: &mut EffectExecutor, action: HarmonyAction) {
        effects
            .push_harmony(action, 0.0)
            .expect("a push always executes");
    }

    /// What a chord change does to the pad, sampled from the first sample
    /// of the change: the newest layer's envelope level at 0, 0.5, 2 and 5
    /// seconds, how many layers start during the window, and the oldest
    /// sounding layer's level at 2 s (the outgoing chord ringing out).
    #[derive(Debug)]
    struct ChangeProfile {
        incoming: [f32; 4],
        layers_started: usize,
        outgoing_at_2s: f32,
    }

    fn newest_level(engine: &FluidEngine) -> f32 {
        engine.pad.layers.last().map_or(0.0, |layer| {
            layer
                .tones
                .iter()
                .map(|tone| tone.stack.envelope.level())
                .fold(0.0, f32::max)
        })
    }

    /// Play to `beat` (one sample before the change), then profile 5 s.
    fn profile_change(engine: &mut FluidEngine, beat: f64) -> ChangeProfile {
        let before = engine.tempo.bpm / (60.0 * f64::from(RATE));
        play_until_beat(engine, beat - 2.0 * before);
        let notes = engine.pad.last_chord_notes;
        let limit = engine.current_sample + 4 * RATE as u64;
        while engine.pad.last_chord_notes == notes {
            assert!(
                engine.current_sample < limit,
                "no chord change near beat {beat}"
            );
            engine.next_stereo();
        }
        let mut incoming = [0.0; 4];
        let mut layers_started = 0;
        let mut last_newest = newest_level(engine);
        let mut outgoing_at_2s = 0.0;
        let marks = [0, RATE as usize / 2, 2 * RATE as usize, 5 * RATE as usize];
        for sample in 0..=marks[3] {
            if let Some(index) = marks.iter().position(|mark| *mark == sample) {
                incoming[index] = newest_level(engine);
            }
            if sample == marks[2] && engine.pad.layers.len() > 1 {
                let outgoing = &engine.pad.layers[engine.pad.layers.len() - 2];
                outgoing_at_2s = outgoing.tones[0].stack.envelope.level();
            }
            engine.next_stereo();
            // A fresh layer replaces the newest one: its level steps down.
            let newest = newest_level(engine);
            if newest + 0.01 < last_newest {
                layers_started += 1;
            }
            last_newest = newest;
        }
        ChangeProfile {
            incoming,
            layers_started,
            outgoing_at_2s,
        }
    }

    #[test]
    #[ignore = "diagnostic: prints authored vs pushed chord-change profiles"]
    fn print_chord_change_profiles() {
        let (_, mut authored, _) = engine();
        println!("authored {:?}", profile_change(&mut authored, 16.0));
        let (_, mut pushed, mut effects) = engine();
        play_until_beat(&mut pushed, 20.5);
        push(&mut effects, HarmonyAction::Move(HarmonyMove::Relative));
        println!("pushed {:?}", profile_change(&mut pushed, 24.0));
    }

    /// A pushed change, its release and an undo must each sound like the
    /// progression's own chord change: the new chord fades in from silence
    /// over the pad Attack, the old one rings out over Release, and nothing
    /// re-strikes afterwards.
    #[test]
    fn a_pushed_change_sounds_like_an_authored_one() {
        let (_, mut authored, _) = engine();
        let reference = profile_change(&mut authored, 16.0);
        let close = |profile: &ChangeProfile, what: &str| {
            for (got, want) in profile.incoming.iter().zip(reference.incoming) {
                assert!(
                    (got - want).abs() < 0.02,
                    "{what} jolts: {profile:?} vs {reference:?}"
                );
            }
            assert_eq!(profile.layers_started, 0, "{what} re-strikes: {profile:?}");
            assert!(
                profile.outgoing_at_2s > 0.1,
                "{what} cuts the old chord: {profile:?}"
            );
        };
        let (_, mut engine, mut effects) = engine();
        play_until_beat(&mut engine, 20.5);
        push(&mut effects, HarmonyAction::Move(HarmonyMove::Relative));
        close(&profile_change(&mut engine, 24.0), "a push");
        // A second push, then undo back to the first, each on its own bar
        // with no authored boundary (every 16 beats) inside the 5 s window.
        play_until_beat(&mut engine, 32.5);
        push(&mut effects, HarmonyAction::Move(HarmonyMove::Parallel));
        close(&profile_change(&mut engine, 36.0), "a second push");
        play_until_beat(&mut engine, 48.5);
        push(&mut effects, HarmonyAction::Undo);
        close(&profile_change(&mut engine, 52.0), "an undo");
    }

    /// The sample a move lands on under each setting: off on the very next
    /// sample, a grid on the first sample at or past its next line.
    #[test]
    fn each_quantize_setting_lands_on_its_own_sample() {
        for (quantize, line) in [
            (PushQuantize::Off, None),
            (PushQuantize::Beat, Some(6.0)),
            (PushQuantize::OneBar, Some(8.0)),
            (PushQuantize::TwoBars, Some(8.0)),
            (PushQuantize::FourBars, Some(16.0)),
        ] {
            let (session, mut engine, mut effects) = engine_quantized(quantize);
            play_until_beat(&mut engine, 5.5);
            push(&mut effects, HarmonyAction::Move(HarmonyMove::Relative));
            let pressed = engine.current_sample;
            while session.load().push.held.is_none() {
                engine.next_stereo();
                assert!(engine.current_sample < pressed + 20 * RATE as u64);
            }
            let step = engine.tempo.bpm / (60.0 * f64::from(RATE));
            match line {
                None => assert_eq!(engine.current_sample, pressed + 1, "off lands at once"),
                Some(line) => {
                    // The landing sample's beat is the first at or past the line.
                    let landed = engine.tempo.beat;
                    assert!(
                        landed >= line - GRID_BEAT_EPSILON - step,
                        "{quantize:?} early"
                    );
                    assert!(landed < line + 2.0 * step, "{quantize:?} late at {landed}");
                }
            }
        }
    }

    /// Changing the setting re-times a waiting move from now.
    #[test]
    fn changing_the_setting_re_times_a_waiting_move() {
        let (session, mut engine, mut effects) = engine_quantized(PushQuantize::FourBars);
        play_until_beat(&mut engine, 1.0);
        push(&mut effects, HarmonyAction::Move(HarmonyMove::Relative));
        effects
            .set_harmony_quantize(PushQuantize::OneBar, 0.0)
            .unwrap();
        play_until_beat(&mut engine, 4.01);
        assert!(
            session.load().push.held.is_some(),
            "re-timed to the next bar"
        );
        push(&mut effects, HarmonyAction::Move(HarmonyMove::Parallel));
        effects
            .set_harmony_quantize(PushQuantize::Off, 0.0)
            .unwrap();
        engine.next_stereo();
        assert_eq!(session.load().push.held.unwrap().triad.name(0), "Cm");
    }

    /// Off still changes through the smooth authored path.
    #[test]
    fn an_unquantized_push_still_fades_in_over_the_attack() {
        let (_, mut authored, _) = engine();
        let reference = profile_change(&mut authored, 16.0);
        let (_, mut engine, mut effects) = engine_quantized(PushQuantize::Off);
        play_until_beat(&mut engine, 22.3);
        push(&mut effects, HarmonyAction::Move(HarmonyMove::Relative));
        let now = engine.tempo.beat;
        let profile = profile_change(&mut engine, now);
        for (got, want) in profile.incoming.iter().zip(reference.incoming) {
            assert!((got - want).abs() < 0.02, "{profile:?} vs {reference:?}");
        }
    }

    #[test]
    fn a_push_lands_on_the_next_bar_voiced_from_the_sounding_notes() {
        let (session, mut engine, mut effects) = engine();
        play_until_beat(&mut engine, 1.5);
        let sounding = engine.pad.last_chord_notes;
        push(&mut effects, HarmonyAction::Move(HarmonyMove::Relative));
        play_until_beat(&mut engine, 3.99);
        assert_eq!(session.load().push.held, None, "nothing lands mid-bar");
        assert_eq!(engine.pad.last_chord_notes, sounding);
        play_until_beat(&mut engine, 4.01);
        let held = session.load().push.held.expect("lands on beat 4");
        assert_eq!(held.triad.name(0), "C");
        assert_eq!(held.slot, 0);
        assert_eq!(held.notes, voice_lead(held.triad, sounding));
        assert_eq!(engine.pad.last_chord_notes, held.notes, "the pad voices it");
    }

    #[test]
    fn the_band_follows_the_pushed_chord_and_it_holds_past_chord_boundaries() {
        let (session, mut engine, mut effects) = engine();
        play_until_beat(&mut engine, 0.5);
        push(&mut effects, HarmonyAction::Move(HarmonyMove::Relative));
        play_until_beat(&mut engine, 4.01);
        let held = session.load().push.held.unwrap();
        let pad = &engine.effective.pad;
        assert_eq!(
            pad_chord_tones(pad, MasterKey::of_progression(0), 0, 0),
            held.notes,
            "Arp and Lead read it"
        );
        // Slot 0 is Am11 rooted on A1; C sits a minor third above, and the
        // authored bass (A2) moves with it.
        assert_eq!(bass_root_note(0, 0, pad, MasterKey::of_progression(0)), 48);
        // The progression would move to slot 1 at beat 16; the push holds.
        play_until_beat(&mut engine, 33.0);
        assert_eq!(engine.pad.cursor.slot(), 0);
        assert_eq!(engine.pad.last_chord_notes, held.notes);
    }

    #[test]
    fn moves_compose_before_the_bar_and_the_push_never_hands_back() {
        let (session, mut engine, mut effects) = engine();
        play_until_beat(&mut engine, 0.5);
        push(&mut effects, HarmonyAction::Move(HarmonyMove::FifthUp));
        push(&mut effects, HarmonyAction::Move(HarmonyMove::FifthUp));
        play_until_beat(&mut engine, 4.01);
        assert_eq!(session.load().push.held.unwrap().triad.name(0), "Bdim");
        // Undo walks back through the composed moves, never to the
        // progression: once landed, the home row is the instrument.
        push(&mut effects, HarmonyAction::Undo);
        push(&mut effects, HarmonyAction::Undo);
        play_until_beat(&mut engine, 64.01);
        assert_eq!(session.load().push.held.unwrap().triad.name(0), "Em");
        assert_eq!(engine.pad.cursor.slot(), 0, "the progression stays held");
    }

    #[test]
    fn undo_before_the_bar_withdraws_and_after_it_walks_back() {
        let (session, mut engine, mut effects) = engine();
        play_until_beat(&mut engine, 0.5);
        push(&mut effects, HarmonyAction::Move(HarmonyMove::Parallel));
        push(&mut effects, HarmonyAction::Undo);
        assert_eq!(session.load().push.queued, None);
        push(&mut effects, HarmonyAction::Move(HarmonyMove::Parallel));
        play_until_beat(&mut engine, 4.01);
        push(&mut effects, HarmonyAction::Move(HarmonyMove::Relative));
        play_until_beat(&mut engine, 8.01);
        assert_eq!(session.load().push.held.unwrap().triad.name(0), "F#m");
        push(&mut effects, HarmonyAction::Undo);
        play_until_beat(&mut engine, 12.01);
        assert_eq!(session.load().push.held.unwrap().triad.name(0), "A");
    }

    #[test]
    fn a_saved_push_restores_as_though_playback_never_stopped() {
        let (session, mut engine, mut effects) = engine();
        play_until_beat(&mut engine, 0.5);
        push(&mut effects, HarmonyAction::Move(HarmonyMove::LeadingTone));
        play_until_beat(&mut engine, 4.01);
        push(&mut effects, HarmonyAction::Move(HarmonyMove::FifthDown));
        let live = session.load();
        let song = SongState {
            controls: live.controls.clone(),
            push: live.push,
            ..SongState::default()
        };
        let code = encode_song_code(&song).unwrap();
        let loaded = decode_song_code(&code).unwrap();
        assert_eq!(loaded.push.held, live.push.held);
        assert_eq!(
            loaded
                .push
                .queued
                .map(|queued| (queued.target, queued.land_beat)),
            Some((live.push.queued.unwrap().target, 0.0)),
            "the queued move lands on the loaded song's first bar"
        );
        let (session, mut engine, _) = engine_for(&loaded);
        // The pushed chord sounds from the first sample, then the queued
        // move lands on bar one.
        assert_eq!(engine.pad.last_chord_notes, live.push.held.unwrap().notes);
        engine.next_stereo();
        let held = session.load().push.held.unwrap();
        assert_eq!(held.triad.name(0), "Bb");
        assert_eq!(engine.pad.last_chord_notes, held.notes);
    }

    fn grab(effects: &mut EffectExecutor, duration: MotionDuration) {
        effects
            .grab_harmony(duration, 0.0)
            .expect("a grab always executes");
    }

    /// Play until the pad's notes change and return the beat of that sample.
    fn play_to_change(engine: &mut FluidEngine, limit_beat: f64) -> f64 {
        let notes = engine.pad.last_chord_notes;
        while engine.pad.last_chord_notes == notes {
            assert!(
                engine.tempo.beat < limit_beat,
                "no chord change before {limit_beat}"
            );
            engine.next_stereo();
        }
        engine.tempo.beat
    }

    fn assert_near(beat: f64, expected: f64, engine: &FluidEngine) {
        let step = engine.tempo.bpm / (60.0 * f64::from(RATE));
        assert!(
            (beat - expected).abs() <= 2.0 * step,
            "changed at {beat}, expected {expected}"
        );
    }

    /// Unquantized pushes land off the beat; `G1` loops the bar that held
    /// them, opening chord included, and replays every change at its own
    /// offset into each pass.
    #[test]
    fn grab_loops_off_beat_pushes_with_their_exact_timing() {
        let (session, mut engine, mut effects) = engine_quantized(PushQuantize::Off);
        let opening = engine.pad.last_chord_notes;
        play_until_beat(&mut engine, 1.25);
        push(&mut effects, HarmonyAction::Move(HarmonyMove::Relative));
        play_until_beat(&mut engine, 2.0);
        let c = session.load().push.held.unwrap();
        play_until_beat(&mut engine, 2.75);
        push(&mut effects, HarmonyAction::Move(HarmonyMove::FifthUp));
        play_until_beat(&mut engine, 3.5);
        grab(&mut effects, MotionDuration::Beats4);
        let looping = session.load().push.looping.expect("a loop");
        assert_eq!(looping.start_beat, 4.0);
        let offsets: Vec<f64> = looping.events().iter().map(|event| event.at).collect();
        assert_eq!(offsets.len(), 3);
        assert_eq!(offsets[0], 0.0);
        assert!((offsets[1] - 1.25).abs() < 0.01, "{offsets:?}");
        assert!((offsets[2] - 2.75).abs() < 0.01, "{offsets:?}");
        assert_eq!(
            looping.events()[0].notes,
            opening,
            "the authored chord is kept"
        );
        for pass in [4.0, 8.0] {
            let at = play_to_change(&mut engine, pass + 1.0);
            assert_near(at, pass, &engine);
            assert_eq!(engine.pad.last_chord_notes, opening);
            let at = play_to_change(&mut engine, pass + 2.0);
            assert_near(at, pass + offsets[1], &engine);
            assert_eq!(engine.pad.last_chord_notes, c.notes);
            let at = play_to_change(&mut engine, pass + 3.5);
            assert_near(at, pass + offsets[2], &engine);
        }
    }

    /// Beat-quantized pushes land on beat lines and loop on them.
    #[test]
    fn grab_loops_beat_quantized_pushes_on_their_beats() {
        let (session, mut engine, mut effects) = engine_quantized(PushQuantize::Beat);
        play_until_beat(&mut engine, 4.3);
        push(&mut effects, HarmonyAction::Move(HarmonyMove::Relative));
        play_until_beat(&mut engine, 5.6);
        push(&mut effects, HarmonyAction::Move(HarmonyMove::StepUp));
        play_until_beat(&mut engine, 7.5);
        grab(&mut effects, MotionDuration::Beats4);
        let looping = session.load().push.looping.unwrap();
        let offsets: Vec<f64> = looping.events().iter().map(|event| event.at).collect();
        assert_eq!(looping.start_beat, 8.0);
        assert_eq!(offsets.len(), 3);
        assert!(
            (offsets[1] - 1.0).abs() < 0.01 && (offsets[2] - 2.0).abs() < 0.01,
            "{offsets:?}"
        );
        let at = play_to_change(&mut engine, 9.5);
        assert_near(at, 8.0, &engine);
        let at = play_to_change(&mut engine, 10.5);
        assert_near(at, 9.0, &engine);
    }

    /// Each size loops that many bars; a new grab replaces the loop, and a
    /// landed push ends it and holds the pushed chord.
    #[test]
    fn grabs_replace_each_other_and_a_push_ends_the_loop() {
        let (session, mut engine, mut effects) = engine_quantized(PushQuantize::Off);
        play_until_beat(&mut engine, 1.0);
        push(&mut effects, HarmonyAction::Move(HarmonyMove::Relative));
        play_until_beat(&mut engine, 32.5);
        for (duration, bars) in [
            (MotionDuration::Beats4, 1),
            (MotionDuration::Beats8, 2),
            (MotionDuration::Beats16, 4),
            (MotionDuration::Beats32, 8),
        ] {
            grab(&mut effects, duration);
            let looping = session.load().push.looping.unwrap();
            assert_eq!(looping.bars, bars);
            assert_eq!(looping.period(), f64::from(bars) * 4.0);
        }
        // The 8-bar grab reached back past the push to the authored chord.
        assert_eq!(session.load().push.looping.unwrap().events().len(), 2);
        play_until_beat(&mut engine, 33.5);
        push(&mut effects, HarmonyAction::Move(HarmonyMove::Parallel));
        engine.next_stereo();
        let after = session.load();
        assert_eq!(after.push.looping, None, "the push took over");
        assert_eq!(engine.pad.last_chord_notes, after.push.held.unwrap().notes);
    }

    /// A loop survives a song code and a stored state, keeping its phase.
    #[test]
    fn a_loop_round_trips_through_song_codes_and_states() {
        let (session, mut engine, mut effects) = engine_quantized(PushQuantize::Off);
        play_until_beat(&mut engine, 1.25);
        push(&mut effects, HarmonyAction::Move(HarmonyMove::Relative));
        play_until_beat(&mut engine, 3.5);
        grab(&mut effects, MotionDuration::Beats4);
        play_until_beat(&mut engine, 6.0);
        let live = session.load();
        let looping = live.push.looping.unwrap();
        let song = SongState {
            controls: live.controls.clone(),
            push: live.push.rebased_at(6.0),
            ..SongState::default()
        };
        let decoded = decode_song_code(&encode_song_code(&song).unwrap()).unwrap();
        let restored = decoded.push.looping.unwrap();
        let same = |a: &HarmonyLoop, b: &HarmonyLoop| {
            a.events().len() == b.events().len()
                && a.events().iter().zip(b.events()).all(|(x, y)| {
                    x.triad == y.triad && x.notes == y.notes && (x.at - y.at).abs() < 1.0 / 1024.0
                })
        };
        assert!(same(&restored, &looping), "{restored:?} vs {looping:?}");
        assert_eq!(restored.start_beat, looping.start_beat - 6.0);
        // At the loaded song's beat zero it plays what was playing at 6.0.
        assert_eq!(
            restored.at(0.0).map(|event| event.notes),
            looping.at(6.0).map(|event| event.notes)
        );
        let (reloaded, mut replay, _) = engine_for(&decoded);
        replay.next_stereo();
        assert_eq!(
            Some(replay.pad.last_chord_notes),
            looping.at(6.0).map(|event| event.notes)
        );
        drop(reloaded);
        // A stored state carries it the same way.
        let context = InteractionExecutionContext {
            selected_control: None,
            beat: 6.0,
        };
        effects
            .execute_interaction(
                interaction::InteractionEffect::State(StateAction::Store(0)),
                &context,
            )
            .unwrap();
        session.update(|s| s.push.looping = None);
        effects
            .execute_interaction(
                interaction::InteractionEffect::ToggleStateQueue(0),
                &context,
            )
            .unwrap();
        let revision = session.load().state_revision;
        while session.load().state_revision == revision {
            engine.next_stereo();
            assert!(engine.tempo.beat < 300.0, "the state never landed");
        }
        let landed = session.load().push.looping.expect("the state's loop");
        assert!(same(&landed, &looping), "{landed:?}");
    }

    /// Mid-push, a Mode change leaves the sounding pushed chord and the
    /// authored progression data exactly as they were; only the next move
    /// reads the new mode (the progression is re-read, never rewritten).
    #[test]
    fn mode_moves_later_pushes_but_never_the_sounding_chord_or_progression() {
        let (session, mut engine, mut effects) = engine();
        play_until_beat(&mut engine, 0.5);
        push(&mut effects, HarmonyAction::Move(HarmonyMove::Relative));
        play_until_beat(&mut engine, 4.5);
        let before = session.load();
        let held = before.push.held.unwrap();
        session.update(|s| s.controls.master.mode = 2.0); // Dorian
        play_until_beat(&mut engine, 7.5);
        let after = session.load();
        assert_eq!(after.push.held, Some(held));
        assert_eq!(engine.pad.last_chord_notes, held.notes);
        assert_eq!(
            after.controls.pad.progression,
            before.controls.pad.progression
        );
        for slot in 0..CHORD_SLOT_COUNT {
            assert_eq!(
                after.controls.pad.chord_slot(0, slot).values(),
                before.controls.pad.chord_slot(0, slot).values()
            );
        }
        // A step up from C: D minor in A minor, D major in A Dorian.
        push(&mut effects, HarmonyAction::Move(HarmonyMove::StepUp));
        play_until_beat(&mut engine, 8.5);
        assert_eq!(session.load().push.held.unwrap().triad.name(0), "D");
    }
}
