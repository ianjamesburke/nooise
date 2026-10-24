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
use crate::fx::reverb::{Freeverb, ReverbParams};

use super::*;

mod gesture_audio;

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
    Reverb(Freeverb),
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
                            Family::Reverb => SlotFx::Reverb(Freeverb::new(sample_rate)),
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
    /// The UI publishes planned actions ahead of time; audio owns their exact
    /// boundary so an onset never sees the previous mute state.
    planned: Option<PlannedAction>,
    pub(crate) vamp: Option<VampState>,
    vamp_active: bool,
    pub(crate) vamp_landed_beat: Option<f64>,
    vamp_landed_at: Option<f64>,
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
    /// Allocation-free per-sample plan, rebuilt only when aggregate
    /// session identity changes, without comparing or cloning automation maps.
    plan: AutomationPlan,
    plan_source: Arc<LiveSessionSnapshot>,
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
        let plan_source = Arc::clone(&live);
        let mut plan = AutomationPlan::default();
        plan.rebuild(&plan_source.automation);
        Self {
            current_sample: 0,
            sample_rate,
            tempo: TempoClock::new(sample_rate, snapshot.master.bpm),
            beat_trigger: GridTrigger::new(),
            osc_input: None,
            control_smoothers: ControlSmoothers::new(&snapshot),
            mute_gates: OutputGates::new(&live.muted),
            muted: live.muted,
            hold_muted: live.hold_muted,
            planned: live.planned,
            vamp: live.vamp,
            vamp_active,
            vamp_landed_beat: None,
            vamp_landed_at: None,
            pad: PadEngine::new_with_live_state(
                sample_rate,
                &initial_pad,
                MasterKey::from_control(snapshot.master.key),
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
            plan,
            plan_source,
        }
    }

    /// Rewind musical schedulers while release envelopes and effect tails keep running.
    fn restart_sequence(&mut self) {
        self.tempo.restart();
        self.vamp_active = false;
        self.vamp_landed_beat = None;
        self.vamp_landed_at = None;
        self.morph_writer = MorphWriter::default();
        self.morph
            .rcu(|current| current.as_ref().as_ref().map(MorphState::restarted));
        self.beat_trigger = GridTrigger::new();
        self.bass.progression = ProgressionFollower::new();
        self.bass.step_trigger = GridTrigger::new();
        self.bass.last_cycle = None;
        self.perc.trigger = GridTrigger::new();
        self.perc.last_cycle = None;
        self.kick.trigger = GridTrigger::new();
        self.kick.last_cycle = None;
        self.clap.trigger = GridTrigger::new();
        self.clap.last_cycle = None;
        self.tonal.progression = ProgressionFollower::new();
        self.tonal.step_trigger = GridTrigger::new();
        self.tonal.step_index = 0;
        self.tonal.last_cycle = None;
        self.arp.progression = ProgressionFollower::new();
        self.arp.note_trigger = GridTrigger::new();
        self.arp.cycle_pos = 0;
        self.arp.ping_pong_dir = 1;
        self.arp.step_index = 0;
        self.arp.last_cycle = None;
        self.lead.progression = ProgressionFollower::new();
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

    #[cfg(feature = "link")]
    pub(crate) fn with_link(mut self, clock: LinkClock) -> Self {
        self.tempo.link = Some(clock);
        self
    }

    pub(crate) fn with_osc_input(mut self, source: OscVoiceSource) -> Self {
        self.osc_input = Some(source);
        self
    }

    fn route_osc_input(&mut self, cmd: OscVoiceCommand) {
        let controls = &self.effective;
        // Played notes follow the key of the progression sounding now.
        let tune = MasterKey::from_control(controls.master.key)
            .shift(self.pad.cursor.window.progression) as f32;
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
    #[cfg(feature = "link")]
    fn begin_buffer(&mut self, output_latency: std::time::Duration) {
        if let Some(link) = &mut self.tempo.link {
            link.begin_buffer(
                self.current_sample,
                output_latency,
                LocalTiming {
                    transport: self.transport,
                    restart: self.transport_restart,
                    target_bpm: self.snapshot.master.bpm,
                },
            );
        }
    }

    fn next_stereo(&mut self) -> (f32, f32) {
        // ~2.9 ms at 44.1 kHz: control edits reach the engine within a frame.
        let morph_boundary = self
            .morph
            .load()
            .as_ref()
            .as_ref()
            .is_some_and(|morph| self.morph_writer.boundary_due(morph, self.tempo.beat));
        if self.current_sample.is_multiple_of(128) || morph_boundary {
            let live = self.session.load();
            let restarting = self.transport_restart != live.transport_restart;
            if restarting {
                self.transport_restart = live.transport_restart;
                self.restart_sequence();
            }
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
            }
            let session = self.session.load();
            self.snapshot = session.controls.clone();
            self.effective.clone_from(&self.snapshot);
            if restarting {
                self.pad.restart_sequence(&self.snapshot.pad);
            }
            self.gesture_snapshot = session.gestures.clone();
            self.drunken_phase_beat = session.drunken_phase_beat;
            self.transport = session.transport;
            self.control_smoothers
                .set_targets(&self.snapshot, self.sample_rate);
            if morph_boundary
                && morph_source
                    .as_ref()
                    .as_ref()
                    .is_some_and(|morph| morph.position_at(self.tempo.beat).blend.is_none())
            {
                // Restore both dropped layers for the first landing note.
                // Kick also captures Level when the hit is constructed.
                self.control_smoothers
                    .settle("kick.level", self.snapshot.kick.level);
                self.control_smoothers
                    .settle("bass.level", self.snapshot.bass.level);
            }
            self.authored_muted = session.muted;
            self.planned = session.planned;
            self.hold_muted = session.hold_muted;
            if self.vamp != session.vamp {
                self.vamp = session.vamp;
            }
            let muted = self.planned_mute_state(self.tempo.beat, self.tempo.bpm, session.muted);
            self.mute_gates.set_targets(&muted, self.sample_rate);
            self.muted = muted;
            self.lead.observe(session.lead_play);
            self.master_bus
                .set_controls(&self.snapshot.master, self.sample_rate);
            if !Arc::ptr_eq(&session, &self.plan_source) {
                self.plan.rebuild(&session.automation);
                self.plan_source = Arc::clone(&session);
            }
        }

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
        self.advance_planned_mute(timing);
        self.advance_vamp(timing);
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

        let key = MasterKey::from_control(self.effective.master.key);
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
        let out = gate_stereo(
            self.master_bus
                .process(master.0, master.1, &self.effective.master),
            mute_gains[Tab::Master as usize],
        );
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
    /// Start the existing click-free gate early enough that it reaches its
    /// target on the downbeat, then publish the audible state on that sample.
    fn advance_planned_mute(&mut self, timing: TimingContext) {
        if timing.transport != Transport::Playing {
            return;
        }
        let Some(PlannedAction::Mute { tab, target_beat }) = self.planned else {
            return;
        };
        let boundary_muted = self.planned_mute_state(timing.beat, timing.bpm, self.authored_muted);
        if timing.beat < target_beat && boundary_muted == self.muted {
            return;
        }
        let action = PlannedAction::Mute { tab, target_beat };
        let session = self.session.load();
        if session.planned != Some(action) {
            self.planned = session.planned;
            let boundary_muted = self.planned_mute_state(timing.beat, timing.bpm, session.muted);
            self.mute_gates
                .set_targets(&boundary_muted, self.sample_rate);
            self.muted = boundary_muted;
            return;
        }

        let boundary_muted = self.planned_mute_state(timing.beat, timing.bpm, session.muted);
        if self.muted != boundary_muted {
            self.mute_gates
                .set_targets(&boundary_muted, self.sample_rate);
            self.muted = boundary_muted;
        }

        if timing.beat < target_beat {
            return;
        }
        match self.session.transact(|snapshot| {
            if snapshot.planned != Some(action) {
                return Err(());
            }
            snapshot.muted[tab as usize] = !snapshot.muted[tab as usize];
            snapshot.planned = None;
            Ok(())
        }) {
            Ok(snapshot) => {
                self.planned = None;
                self.muted = self.planned_mute_state(timing.beat, timing.bpm, snapshot.muted);
            }
            Err(()) => {
                let session = self.session.load();
                self.planned = session.planned;
                let boundary_muted =
                    self.planned_mute_state(timing.beat, timing.bpm, session.muted);
                self.mute_gates
                    .set_targets(&boundary_muted, self.sample_rate);
                self.muted = boundary_muted;
            }
        }
    }

    /// State the gate must be approaching before a planned mute's boundary.
    /// The extra sample guarantees the eased ramp has settled before the
    /// first onset at that downbeat is rendered.
    fn planned_mute_state(&self, beat: f64, bpm: f64, muted: MuteState) -> MuteState {
        let ramp_samples = (LEVEL_RAMP_MS * 0.001 * self.sample_rate).round() as f64 + 1.0;
        let ramp_beats = ramp_samples * bpm / (60.0 * f64::from(self.sample_rate));
        let mut boundary_muted = muted;
        if let Some(PlannedAction::Mute { tab, target_beat }) = self.planned
            && beat >= target_beat - ramp_beats
        {
            boundary_muted[tab as usize] = !boundary_muted[tab as usize];
        }
        if self.hold_muted[HoldMute::KickAndBass as usize] {
            boundary_muted[Tab::Kick as usize] = true;
            boundary_muted[Tab::Bass as usize] = true;
        }
        boundary_muted
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
        let session = self.session.load();
        if session.vamp != self.vamp {
            self.vamp = session.vamp;
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
                }
                Err(()) => {
                    self.vamp = self.session.load().vamp;
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
mod planned_mute_tests {
    use super::*;

    const SAMPLE_RATE: f32 = 48_000.0;
    const TARGET_BEAT: f64 = 0.1;

    fn engine_with_planned_mute(initially_muted: bool) -> (FluidEngine, LiveSession) {
        let mut snapshot = LiveSessionSnapshot::from_controls(FluidControls::default());
        snapshot.muted[Tab::Kick as usize] = initially_muted;
        snapshot.planned = Some(PlannedAction::Mute {
            tab: Tab::Kick,
            target_beat: TARGET_BEAT,
        });
        let session = LiveSession::new(snapshot);
        let engine = FluidEngine::new(
            SAMPLE_RATE,
            session.clone(),
            no_morph(),
            Arc::new(FluidTelemetry::default()),
        );
        (engine, session)
    }

    fn render_through_boundary(engine: &mut FluidEngine) {
        while engine.tempo.beat < TARGET_BEAT {
            engine.next_stereo();
        }
        // TempoClock reports the sample's beat before it advances, so render
        // the first sample at or beyond the boundary as well.
        engine.next_stereo();
    }

    #[test]
    fn plan_source_retains_the_published_session_without_map_copies() {
        let (mut engine, session) = engine_with_planned_mute(false);
        let original = session.load();
        engine.next_stereo();
        assert!(Arc::ptr_eq(&engine.plan_source, &original));
        session
            .transact(|snapshot| {
                snapshot.controls.master.level = 0.4;
                Ok::<_, ()>(())
            })
            .unwrap();
        for _ in 0..128 {
            engine.next_stereo();
        }
        assert!(Arc::ptr_eq(&engine.plan_source, &session.load()));
        assert!(!Arc::ptr_eq(&engine.plan_source, &original));
    }

    #[test]
    fn perc_roll_override_does_not_compound_between_control_ticks() {
        let mut snapshot = LiveSessionSnapshot::from_controls(FluidControls::default());
        snapshot.hold_muted[HoldMute::PercRoll as usize] = true;
        let authored = snapshot.controls.perc.interval_beats;
        let mut engine = FluidEngine::new(
            SAMPLE_RATE,
            LiveSession::new(snapshot),
            no_morph(),
            Arc::new(FluidTelemetry::default()),
        );
        for _ in 0..127 {
            engine.next_stereo();
            assert_eq!(engine.effective.perc.interval_beats, authored);
        }
    }

    #[test]
    fn cancelled_planned_mute_does_not_commit_at_the_boundary() {
        let (mut engine, session) = engine_with_planned_mute(false);
        while engine.muted == engine.authored_muted {
            engine.next_stereo();
        }
        session
            .transact(|snapshot| {
                snapshot.planned = None;
                Ok::<_, ()>(())
            })
            .unwrap();
        render_through_boundary(&mut engine);
        assert!(!session.load().muted[Tab::Kick as usize]);
        assert_eq!(engine.planned, None);
    }

    #[test]
    fn planned_mute_gate_is_closed_at_its_downbeat() {
        let (mut engine, session) = engine_with_planned_mute(false);
        render_through_boundary(&mut engine);

        assert!(session.load().muted[Tab::Kick as usize]);
        assert_eq!(session.load().planned, None);
        assert_eq!(engine.mute_gates.gates[Tab::Kick as usize].current, 0.0);
    }

    #[test]
    fn planned_unmute_gate_is_open_at_its_downbeat() {
        let (mut engine, session) = engine_with_planned_mute(true);
        render_through_boundary(&mut engine);

        assert!(!session.load().muted[Tab::Kick as usize]);
        assert_eq!(session.load().planned, None);
        assert_eq!(engine.mute_gates.gates[Tab::Kick as usize].current, 1.0);
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
}

impl ControlSmoothers {
    pub(crate) fn new(c: &FluidControls) -> Self {
        let mut seen = BTreeSet::new();
        let smoothers: Vec<_> = all_specs()
            .filter(|spec| spec.kind.smooths_audio())
            .filter(|spec| seen.insert(spec.id))
            .map(|spec| {
                let mut smoother = ControlSmoother::for_spec(spec, (spec.get)(c));
                let slot = module_slot_row(spec.id, c).map(|(slot, _)| slot);
                smoother.slot_kind = slot.map(|slot| slot.kind);
                smoother.slot_filter_type = slot.and_then(filter_type_for_slot);
                smoother
            })
            .collect();
        Self {
            active: Vec::with_capacity(smoothers.len()),
            smoothers,
        }
    }

    pub(crate) fn set_targets(&mut self, c: &FluidControls, sample_rate: f32) {
        let ramp_samples = (LEVEL_RAMP_MS * 0.001 * sample_rate).round() as u32;
        self.active.clear();
        for (index, smoother) in self.smoothers.iter_mut().enumerate() {
            let snapshot_value = (smoother.spec.get)(c);
            let slot = module_slot_row(smoother.spec.id, c).map(|(slot, _)| slot);
            let slot_kind = slot.map(|slot| slot.kind);
            let slot_filter_type = slot.and_then(filter_type_for_slot);
            if slot_kind != smoother.slot_kind
                || slot_filter_type != smoother.slot_filter_type
                || (snapshot_value != smoother.ramp.target
                    && (!smoother.spec.contextual(c).kind.smooths_audio()
                        || is_swing_amount_row(smoother.spec.id, c)))
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

    fn settle(&mut self, id: &str, value: f32) {
        if let Some(smoother) = self.smoothers.iter_mut().find(|s| s.spec.id == id) {
            smoother.ramp = EasedRamp::settled(value);
            smoother.idle = true;
        }
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
    /// `Some` under `--link`: beat and tempo come from the shared timeline
    /// instead of accumulating locally.
    #[cfg(feature = "link")]
    pub(crate) link: Option<LinkClock>,
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
            #[cfg(feature = "link")]
            link: None,
        }
    }

    pub(crate) fn restart(&mut self) {
        self.beat = 0.0;
        #[cfg(feature = "link")]
        if let Some(link) = &mut self.link {
            link.restart();
        }
    }

    /// A stopped clock holds its beat, so every beat-derived value (LFO
    /// positions, the auto morph, grid phases) waits where it was and
    /// resumes from there. Tempo still glides toward its target.
    pub(crate) fn tick(&mut self, target_bpm: f32, transport: Transport) -> TimingContext {
        #[cfg(feature = "link")]
        if let Some(link) = &mut self.link {
            let sample = link.next_sample();
            return self.follow_link(sample, transport);
        }
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

    /// A stopped transport holds its beat exactly as the free clock does; a
    /// launch waiting for its downbeat reads as stopped at beat zero.
    #[cfg(feature = "link")]
    fn follow_link(&mut self, sample: LinkSample, transport: Transport) -> TimingContext {
        self.bpm = sample.bpm;
        let transport = match (transport, sample.beat) {
            (Transport::Playing, Some(beat)) => {
                self.beat = beat;
                Transport::Playing
            }
            _ => Transport::Stopped,
        };
        TimingContext {
            transport,
            ..TimingContext::new(self.sample_rate, self.bpm, self.beat)
        }
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
