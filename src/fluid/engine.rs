//! `FluidEngine`: the voice mixer that the audio callback pulls frames from.
//!
//! Owns the tempo clock and its `TimingContext`, the grid triggers voices fire
//! on, registry-derived gain smoothing, the per-layer and master module effect
//! banks, and the master bus.

use std::collections::BTreeSet;

use crate::fx::compression::{CompressorParams, StereoCompressor};
use crate::fx::delay::{DelayParams, StereoDelay};
use crate::fx::drive;
use crate::fx::reverb::{Freeverb, ReverbParams};

use super::*;

const STARTUP_FADE_SECONDS: f32 = 2.0;

/// Stateful post-synthesis Delay instances, addressed by the stable
/// `(layer, slot)` storage identity rather than a catalog name.
enum SlotFx {
    Delay(StereoDelay),
    Reverb(Freeverb),
    Compression(StereoCompressor),
}

impl SlotFx {
    fn family(&self) -> Family {
        match self {
            Self::Delay(_) => Family::Delay,
            Self::Reverb(_) => Family::Reverb,
            Self::Compression(_) => Family::Compression,
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
struct RetiringFx {
    fx: SlotFx,
    slot: ModuleSlot,
    /// Weight of the retiring processor's output, walking 1.0 down to 0.0.
    weight: f32,
    step: f32,
}

struct ModuleFxBank {
    slots: [Option<SlotFx>; MODULE_LAYERS * MODULE_SLOTS],
    retiring: [Option<RetiringFx>; MODULE_LAYERS * MODULE_SLOTS],
    /// Each slot's field values from the last frame it was loaded. A retiring
    /// processor keeps running on these, not on the live slot: by the time a
    /// removal is noticed the live slot has already been cleared, and its
    /// zeroed Amount would silence the very tail being faded out.
    last_loaded: [ModuleSlot; MODULE_LAYERS * MODULE_SLOTS],
    max_delay_samples: usize,
    sample_rate: f32,
    retire_step: f32,
}

impl ModuleFxBank {
    fn new(sample_rate: f32) -> Self {
        Self {
            slots: std::array::from_fn(|_| None),
            retiring: std::array::from_fn(|_| None),
            last_loaded: [ModuleSlot::default(); MODULE_LAYERS * MODULE_SLOTS],
            max_delay_samples: (sample_rate * (DELAY_FREE_MAX_MS / 1_000.0)).ceil() as usize,
            sample_rate,
            // The same window every other click-free level change in the
            // engine uses, so a module leaving sounds like any other gain
            // change rather than its own event.
            retire_step: 1.0 / (LEVEL_RAMP_MS * 0.001 * sample_rate).max(1.0),
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
        self.retiring[index] = Some(RetiringFx {
            fx,
            slot: self.last_loaded[index],
            weight: 1.0,
            step: self.retire_step,
        });
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
            &mut retiring.fx,
            &retiring.slot,
            sample,
            timing,
            max_delay_samples,
            sample_rate,
        );
        let weight = retiring.weight;
        retiring.weight -= retiring.step;
        if retiring.weight <= 0.0 {
            self.retiring[index] = None;
        }
        (
            sample.0 + (wet.0 - sample.0) * weight,
            sample.1 + (wet.1 - sample.1) * weight,
        )
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
                Family::Delay | Family::Reverb | Family::Compression => {
                    if processor.is_none() {
                        *processor = Some(match kind.family {
                            Family::Delay => SlotFx::Delay(StereoDelay::new(max_delay_samples)),
                            Family::Reverb => SlotFx::Reverb(Freeverb::new(sample_rate)),
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
                }
                // Stateless shaping families hold no tail, so they have
                // nothing to retire and are simply absent when unloaded.
                Family::SingleAmount => {
                    if kind.id == "drive" {
                        sample = drive::process(sample, slot.amount);
                    }
                }
                Family::TwoKnob => {}
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
    fn reverb_and_compression_execute_through_the_same_slot_chain() {
        let timing = TimingContext::new(44_100.0, 120.0, 0.0);
        let mut reverb_bank = ModuleFxBank::new(44_100.0);
        let mut reverb_slots = [ModuleSlot::default(); MODULE_SLOTS];
        reverb_slots[0] = preset_slot("room", 1.0);
        reverb_slots[0].time = 0.72;
        reverb_slots[0].feedback = 0.45;
        reverb_bank.process(Tab::Kick, &reverb_slots, (1.0, 1.0), timing);
        let mut tail = (0.0, 0.0);
        for _ in 0..2_000 {
            tail = reverb_bank.process(Tab::Kick, &reverb_slots, (0.0, 0.0), timing);
            if tail != (0.0, 0.0) {
                break;
            }
        }
        assert_ne!(tail, (0.0, 0.0));

        let mut compression_bank = ModuleFxBank::new(44_100.0);
        let mut compression_slots = [ModuleSlot::default(); MODULE_SLOTS];
        compression_slots[0] = preset_slot("compression", 1.0);
        compression_slots[0].time = -20.0;
        compression_slots[0].right_time = 4.0;
        compression_slots[0].vintage = 0.0;
        let mut compressed = (1.0, 1.0);
        for _ in 0..256 {
            compressed =
                compression_bank.process(Tab::Kick, &compression_slots, (1.0, 1.0), timing);
        }
        assert!(compressed.0 < 1.0);
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
        let tabs = [
            Tab::Chords,
            Tab::Perc,
            Tab::Bass,
            Tab::Kick,
            Tab::Tonal,
            Tab::Clap,
            Tab::Arp,
            Tab::Master,
        ];

        for tab in tabs {
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
            let mut reverb_tail = (0.0, 0.0);
            for _ in 0..2_000 {
                reverb_tail = bank.process(tab, &slots, (0.0, 0.0), timing);
                if reverb_tail != (0.0, 0.0) {
                    break;
                }
            }
            assert_ne!(reverb_tail, (0.0, 0.0), "Reverb is inert on {}", tab.name());

            slots[0] = preset_slot("delay", 1.0);
            slots[0].clock = DelayClock::Free.value();
            slots[0].right_clock = DelayClock::Free.value();
            slots[0].time = 10.0;
            slots[0].right_time = 10.0;
            slots[0].feedback = 0.0;
            let mut bank = ModuleFxBank::new(44_100.0);
            bank.process(tab, &slots, (1.0, -1.0), timing);
            let mut echo = (0.0, 0.0);
            for _ in 0..500 {
                echo = bank.process(tab, &slots, (0.0, 0.0), timing);
                if echo != (0.0, 0.0) {
                    break;
                }
            }
            assert_ne!(echo, (0.0, 0.0), "Delay is inert on {}", tab.name());
        }
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
    pub(crate) gain_smoothers: GainSmoothers,
    mute_gates: OutputGates,
    pub(crate) pad: PadEngine,
    pub(crate) perc: PercEngine,
    pub(crate) kick: KickEngine,
    pub(crate) tonal: TonalEngine,
    pub(crate) clap: ClapEngine,
    pub(crate) bass: BassEngine,
    pub(crate) arp: ArpEngine,
    module_fx: ModuleFxBank,
    pub(crate) master_bus: MasterBus,
    pub(crate) session: LiveSession,
    /// `Some` only while running `nooise auto`; rewrites `controls` on a
    /// throttled tick so the morph is audible and visible.
    pub(crate) morph: Arc<ArcSwap<Option<MorphState>>>,
    morph_writer: MorphWriter,
    pub(crate) telemetry: Arc<FluidTelemetry>,
    pub(crate) snapshot: FluidControls,
    /// Allocation-free per-sample plan, rebuilt only when aggregate
    /// automation differs from the last planned state.
    plan: AutomationPlan,
    plan_source: Arc<AutomationState>,
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
        let live = session.load();
        let snapshot = live.controls.clone();
        let plan_source = Arc::new(live.automation.clone());
        let mut plan = AutomationPlan::default();
        plan.rebuild(&plan_source);
        Self {
            current_sample: 0,
            sample_rate,
            tempo: TempoClock::new(sample_rate, snapshot.master.bpm),
            gain_smoothers: GainSmoothers::new(&snapshot),
            mute_gates: OutputGates::new(&live.muted),
            pad: PadEngine::new(sample_rate, &snapshot.pad, Arc::clone(&telemetry)),
            perc: PercEngine::new(sample_rate),
            kick: KickEngine::new(sample_rate, Arc::clone(&telemetry)),
            tonal: TonalEngine::new_with_session_state(
                sample_rate,
                publish_tonal_session_state.then(|| session.clone()),
            ),
            clap: ClapEngine::new(sample_rate),
            bass: BassEngine::new(sample_rate),
            arp: ArpEngine::new(sample_rate),
            module_fx: ModuleFxBank::new(sample_rate),
            master_bus: MasterBus::new(&snapshot.master, sample_rate),
            session,
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
            self.gain_smoothers
                .set_targets(&self.snapshot, self.sample_rate);
            self.mute_gates
                .set_targets(&session.muted, self.sample_rate);
            self.master_bus
                .set_controls(&self.snapshot.master, self.sample_rate);
            if session.automation != *self.plan_source {
                self.plan.rebuild(&session.automation);
                self.plan_source = Arc::new(session.automation.clone());
            }
        }

        let fade = startup_fade(self.current_sample, self.sample_rate);
        let mut effective = self.gain_smoothers.next_controls(&self.snapshot);
        let timing = self.tempo.tick(effective.master.bpm);
        if self.current_sample.is_multiple_of(256) {
            self.telemetry.publish_beat(timing.beat);
        }
        self.plan.apply(&mut effective, timing);
        resolve_module_chain(&mut effective);
        let mute_gains = self.mute_gates.next();

        let tune = effective.master.tune;
        let (pad_l, pad_r) = gate_stereo(
            self.module_fx.process(
                Tab::Chords,
                &effective.modules.pad,
                self.pad.next(&effective.pad, tune, timing),
                timing,
            ),
            mute_gains[Tab::Chords as usize],
        );
        let (perc_l, perc_r) = gate_stereo(
            self.module_fx.process(
                Tab::Perc,
                &effective.modules.perc,
                {
                    let perc = self.perc.next(&effective.perc, timing);
                    (perc, perc)
                },
                timing,
            ),
            mute_gains[Tab::Perc as usize],
        );
        let (kick_l, kick_r) = gate_stereo(
            self.module_fx.process(
                Tab::Kick,
                &effective.modules.kick,
                self.kick.next(&effective.kick, timing),
                timing,
            ),
            mute_gains[Tab::Kick as usize],
        );
        let (ton_l, ton_r) = gate_stereo(
            self.module_fx.process(
                Tab::Tonal,
                &effective.modules.tonal,
                self.tonal.next(&effective.tonal, tune, timing),
                timing,
            ),
            mute_gains[Tab::Tonal as usize],
        );
        let (clap_l, clap_r) = gate_stereo(
            self.module_fx.process(
                Tab::Clap,
                &effective.modules.clap,
                self.clap.next(&effective.clap, timing),
                timing,
            ),
            mute_gains[Tab::Clap as usize],
        );
        let (bass_l, bass_r) = gate_stereo(
            self.module_fx.process(
                Tab::Bass,
                &effective.modules.bass,
                self.bass
                    .next(&effective.bass, &effective.pad, tune, timing),
                timing,
            ),
            mute_gains[Tab::Bass as usize],
        );
        let (arp_l, arp_r) = gate_stereo(
            self.module_fx.process(
                Tab::Arp,
                &effective.modules.arp,
                self.arp.next(&effective.arp, &effective.pad, tune, timing),
                timing,
            ),
            mute_gains[Tab::Arp as usize],
        );
        self.current_sample += 1;

        let raw_l = mix_voices(pad_l, perc_l, kick_l, ton_l, clap_l, bass_l, arp_l, fade);
        let raw_r = mix_voices(pad_r, perc_r, kick_r, ton_r, clap_r, bass_r, arp_r, fade);
        let master = self.module_fx.process(
            Tab::Master,
            &effective.modules.master,
            (raw_l, raw_r),
            timing,
        );
        gate_stereo(
            self.master_bus
                .process(master.0, master.1, &effective.master),
            mute_gains[Tab::Master as usize],
        )
    }
}

#[inline]
fn gate_stereo(sample: (f32, f32), gain: f32) -> (f32, f32) {
    (sample.0 * gain, sample.1 * gain)
}

pub(crate) fn startup_fade(current_sample: u64, sample_rate: f32) -> f32 {
    (current_sample as f32 / (sample_rate * STARTUP_FADE_SECONDS)).min(1.0)
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
    fade: f32,
) -> f32 {
    (pad + perc * 0.6 + kick * 0.7 + ton + clap * 0.65 + bass * 0.75 + arp) * fade
}

#[derive(Clone, Copy)]
struct OutputGate {
    start: f32,
    current: f32,
    target: f32,
    samples_total: u32,
    samples_remaining: u32,
}

impl OutputGate {
    fn new(muted: bool) -> Self {
        let gain = if muted { 0.0 } else { 1.0 };
        Self {
            start: gain,
            current: gain,
            target: gain,
            samples_total: 0,
            samples_remaining: 0,
        }
    }

    fn set_muted(&mut self, muted: bool, ramp_samples: u32) {
        let target = if muted { 0.0 } else { 1.0 };
        if target == self.target {
            return;
        }
        self.start = self.current;
        self.target = target;
        self.samples_total = ramp_samples.max(1);
        self.samples_remaining = self.samples_total;
    }

    fn next(&mut self) -> f32 {
        if self.samples_remaining == 0 {
            return self.target;
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

struct OutputGates {
    gates: [OutputGate; TAB_COUNT],
}

impl OutputGates {
    fn new(muted: &MuteState) -> Self {
        Self {
            gates: std::array::from_fn(|index| OutputGate::new(muted[index])),
        }
    }

    fn set_targets(&mut self, muted: &MuteState, sample_rate: f32) {
        let ramp_samples = (LEVEL_RAMP_MS * 0.001 * sample_rate).round() as u32;
        for (gate, muted) in self.gates.iter_mut().zip(muted) {
            gate.set_muted(*muted, ramp_samples);
        }
    }

    fn next(&mut self) -> [f32; TAB_COUNT] {
        std::array::from_fn(|index| self.gates[index].next())
    }
}

pub(crate) struct GainSmoother {
    pub(crate) spec: &'static ControlSpec,
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
            .map(|spec| GainSmoother::for_spec(spec, (spec.get)(c)))
            .collect();
        Self { smoothers }
    }

    pub(crate) fn set_targets(&mut self, c: &FluidControls, sample_rate: f32) {
        let ramp_samples = (LEVEL_RAMP_MS * 0.001 * sample_rate).round() as u32;
        for smoother in &mut self.smoothers {
            let snapshot_value = (smoother.spec.get)(c);
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
            (smoother.spec.set)(&mut next, smoother.next());
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

/// One sample's worth of transport: where the beat clock stands and the rates
/// needed to turn musical time into samples. Every voice's `next` reads it.
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
