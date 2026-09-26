//! Retrospective manual-knob history and compact sixteen-beat playback curves.

use std::collections::VecDeque;

use super::*;

pub(crate) const CAPTURE_BEATS: f64 = 16.0;
pub(crate) const CAPTURE_SAMPLES: usize = 128;
pub(crate) const MAX_CAPTURES: usize = 4;
const HISTORY_TARGETS: usize = 16;
const HISTORY_EVENTS: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CaptureAction {
    Capture,
    Bypass,
    Resume,
    Delete,
}

impl CaptureAction {
    pub(crate) const ALL: [Self; 4] = [Self::Capture, Self::Bypass, Self::Resume, Self::Delete];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Capture => "Capture",
            Self::Bypass => "Bypass",
            Self::Resume => "Resume",
            Self::Delete => "Delete",
        }
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::Capture => "keep last 16 beats of this knob",
            Self::Bypass => "bypass this knob's captured loop",
            Self::Resume => "resume captured loop next bar",
            Self::Delete => "delete this knob's captured loop",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CaptureClip {
    pub(crate) samples: [u8; CAPTURE_SAMPLES],
    /// Loop phase anchor and admission beat are distinct: resume keeps phase.
    pub(crate) origin: f64,
    pub(crate) launch: f64,
    pub(crate) enabled: bool,
}

impl CaptureClip {
    pub(crate) fn position(&self, beat: f64) -> Option<f32> {
        if !self.enabled || beat < self.launch {
            return None;
        }
        let position =
            (beat - self.origin).rem_euclid(CAPTURE_BEATS) * CAPTURE_SAMPLES as f64 / CAPTURE_BEATS;
        let index = position.floor() as usize % CAPTURE_SAMPLES;
        let a = f32::from(self.samples[index]);
        let b = f32::from(self.samples[(index + 1) % CAPTURE_SAMPLES]);
        Some((a + (b - a) * position.fract() as f32) / 255.0)
    }

    pub(crate) fn status(&self, beat: f64) -> &'static str {
        if !self.enabled {
            "bypassed"
        } else if beat < self.launch {
            "queued"
        } else {
            "loop"
        }
    }

    pub(crate) fn rebase(&mut self, beat: f64) {
        self.origin = -(beat - self.origin).rem_euclid(CAPTURE_BEATS);
        self.launch = (self.launch - beat).max(0.0);
    }
}

pub(crate) fn capture_eligible(spec: &ControlSpec) -> bool {
    matches!(spec.kind, ControlKind::Gain | ControlKind::Continuous)
}

struct KnobHistory {
    target: recipe::RecipeTarget,
    initial: f32,
    events: VecDeque<(f64, f32)>,
    lost_before: Option<f64>,
}

#[derive(Default)]
pub(crate) struct CaptureHistory {
    knobs: VecDeque<KnobHistory>,
}

impl CaptureHistory {
    pub(crate) fn record_changes(
        &mut self,
        before: &LiveSessionSnapshot,
        after: &LiveSessionSnapshot,
        beat: f64,
    ) {
        let mut seen = std::collections::BTreeSet::new();
        for spec in all_specs().filter(|spec| capture_eligible(&spec.contextual(&after.controls))) {
            let old = (spec.get)(&before.controls);
            let new = (spec.get)(&after.controls);
            if old.to_bits() == new.to_bits() {
                continue;
            }
            if !seen.insert(spec.id) {
                continue;
            }
            let Some(target) = recipe::RecipeTarget::capture(spec.id, after) else {
                continue;
            };
            // Slot replacements are structural, not a played gesture.
            if !target.is_current(before) {
                continue;
            }
            let index = self.knobs.iter().position(|knob| knob.target == target);
            let mut knob = index
                .and_then(|index| self.knobs.remove(index))
                .unwrap_or_else(|| KnobHistory {
                    target,
                    initial: spec.ratio(old, &before.controls),
                    events: VecDeque::new(),
                    lost_before: None,
                });
            while knob
                .events
                .front()
                .is_some_and(|event| event.0 < beat - CAPTURE_BEATS)
                || knob.events.len() >= HISTORY_EVENTS
            {
                if let Some((at, value)) = knob.events.pop_front() {
                    knob.initial = value;
                    knob.lost_before = Some(at);
                }
            }
            knob.events
                .push_back((beat, spec.ratio(new, &after.controls)));
            self.knobs.push_back(knob);
            while self.knobs.len() > HISTORY_TARGETS {
                self.knobs.pop_front();
            }
        }
    }

    pub(crate) fn clip(
        &self,
        target: recipe::RecipeTarget,
        end: f64,
        now: f64,
    ) -> Option<CaptureClip> {
        let knob = self.knobs.iter().find(|knob| knob.target == target)?;
        if knob
            .lost_before
            .is_some_and(|beat| beat > end - CAPTURE_BEATS)
        {
            return None;
        }
        if !knob
            .events
            .iter()
            .any(|event| event.0 >= end - CAPTURE_BEATS && event.0 <= end)
        {
            return None;
        }
        let samples = std::array::from_fn(|index| {
            let beat = end - CAPTURE_BEATS + index as f64 * CAPTURE_BEATS / CAPTURE_SAMPLES as f64;
            let value = knob
                .events
                .iter()
                .rev()
                .find(|event| event.0 <= beat)
                .map_or(knob.initial, |event| event.1);
            (value.clamp(0.0, 1.0) * 255.0).round() as u8
        });
        let launch = next_bar_beat(now);
        Some(CaptureClip {
            samples,
            origin: launch,
            launch,
            enabled: true,
        })
    }
}

pub(crate) fn suspend_changed_captures(
    before: &LiveSessionSnapshot,
    after: &mut LiveSessionSnapshot,
) {
    for (address, clip) in &mut after.automation.captures {
        let spec = address.spec();
        if (spec.get)(&before.controls).to_bits() != (spec.get)(&after.controls).to_bits() {
            clip.enabled = false;
        }
    }
}

pub(crate) fn suspend_capture(snapshot: &mut LiveSessionSnapshot, id: &'static str) {
    if let Some(clip) = snapshot
        .automation
        .captures
        .get_mut(&ControlAddress::new(id))
    {
        clip.enabled = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(
        history: &mut CaptureHistory,
        snapshot: &mut LiveSessionSnapshot,
        beat: f64,
        value: f32,
    ) {
        let before = snapshot.clone();
        snapshot.controls.pad.level = value;
        history.record_changes(&before, snapshot, beat);
    }

    #[test]
    fn capture_keeps_sixteen_beats_prefills_early_history_and_expires_idle_takes() {
        let mut snapshot = LiveSessionSnapshot::from_controls(FluidControls::default());
        snapshot.controls.pad.level = 0.0;
        let target = recipe::RecipeTarget::capture("pad.level", &snapshot).unwrap();
        let mut history = CaptureHistory::default();
        change(&mut history, &mut snapshot, 1.0, 1.0);
        change(&mut history, &mut snapshot, 2.0, 0.5);
        let clip = history.clip(target, 3.0, 9.0).unwrap();
        assert_eq!(clip.origin, 12.0);
        assert_eq!(clip.position(11.999), None);
        assert_eq!(clip.samples[0], 0);
        assert_eq!(clip.samples[14 * 8], 255);
        assert_eq!(clip.samples[15 * 8], 128);
        assert_eq!(clip.position(12.0), clip.position(28.0));
        assert!(history.clip(target, 19.0, 19.0).is_none());
    }

    #[test]
    fn capture_interpolates_within_the_beat_and_rebases_live_pending_and_bypassed_phase() {
        let mut clip = CaptureClip {
            samples: [0; CAPTURE_SAMPLES],
            origin: 20.0,
            launch: 20.0,
            enabled: true,
        };
        clip.samples[1] = 255;
        assert_eq!(clip.position(20.0625), Some(0.5));
        for beat in [18.0, 20.0625, 71.0] {
            let mut restored = clip.clone();
            restored.rebase(beat);
            for elapsed in [0.0, 0.125, 2.0, 16.0, 21.0] {
                assert_eq!(restored.position(elapsed), clip.position(beat + elapsed));
            }
        }
        clip.enabled = false;
        clip.rebase(25.0);
        assert_eq!(clip.position(0.0), None);
        clip.enabled = true;
        clip.launch = 4.0;
        assert_eq!(clip.position(3.99), None);
        assert_eq!(clip.position(4.0), Some(0.0));
    }

    #[test]
    fn capture_refuses_history_overflow_until_a_complete_window_is_available() {
        let mut snapshot = LiveSessionSnapshot::from_controls(FluidControls::default());
        let target = recipe::RecipeTarget::capture("pad.level", &snapshot).unwrap();
        let mut history = CaptureHistory::default();
        for index in 0..600 {
            change(
                &mut history,
                &mut snapshot,
                index as f64 / 100.0,
                (index % 2) as f32,
            );
        }
        assert!(history.clip(target, 6.0, 6.0).is_none());
        change(&mut history, &mut snapshot, 30.0, 0.3);
        assert!(history.clip(target, 31.0, 31.0).is_some());
        assert!(history.knobs[0].events.len() <= HISTORY_EVENTS);
    }

    #[test]
    fn capture_audio_plan_composes_with_lfo_and_declicks_bypass() {
        let address = ControlAddress::new("pad.level");
        let mut automation = AutomationState::default();
        automation.captures.insert(
            address,
            CaptureClip {
                samples: [51; CAPTURE_SAMPLES],
                origin: 0.0,
                launch: 0.0,
                enabled: true,
            },
        );
        automation.add_route(
            address,
            LfoRoute {
                depth_ratio: 0.1,
                shape: LfoShape::Square,
                ..LfoRoute::default()
            },
        );
        let mut controls = FluidControls::default();
        controls.pad.level = 0.7;
        let mut plan = AutomationPlan::default();
        plan.rebuild(&automation);
        plan.apply(&mut controls, TimingContext::new(44100.0, 120.0, 0.0));
        let expected = modulated_control_value_full(
            address.spec(),
            automation.lfo_lanes(address),
            &[],
            0.2,
            ModContext::lfo_only(0.0),
        );
        assert!((controls.pad.level - expected).abs() < 0.0001);
        automation.captures.get_mut(&address).unwrap().enabled = false;
        plan.rebuild(&automation);
        controls.pad.level = 0.7;
        plan.apply(&mut controls, TimingContext::new(44100.0, 120.0, 0.0));
        assert!((controls.pad.level - expected).abs() < 0.01);
        for _ in 0..4000 {
            controls.pad.level = 0.7;
            plan.apply(&mut controls, TimingContext::new(44100.0, 120.0, 0.0));
        }
        let expected = modulated_control_value_full(
            address.spec(),
            automation.lfo_lanes(address),
            &[],
            0.7,
            ModContext::lfo_only(0.0),
        );
        assert!((controls.pad.level - expected).abs() < 0.0001);
    }

    #[test]
    fn captured_level_loop_is_audible_in_deterministic_engine_render() {
        let render = |enabled| {
            let mut song = SongState::default();
            song.controls.master.bpm = 120.0;
            song.controls.pad.level = 1.0;
            song.controls.modules.pad = Default::default();
            song.muted.fill(true);
            song.muted[Tab::Chords as usize] = false;
            song.muted[Tab::Master as usize] = false;
            song.automation.captures.insert(
                ControlAddress::new("pad.level"),
                CaptureClip {
                    samples: std::array::from_fn(|index| if index < 64 { 0 } else { 255 }),
                    origin: 4.0,
                    launch: 4.0,
                    enabled,
                },
            );
            let session = LiveSession::new(LiveSessionSnapshot::from_song(&song));
            let mut engine = FluidEngine::new(
                8_000.0,
                session,
                no_morph(),
                Arc::new(FluidTelemetry::default()),
            );
            engine.reseed(42);
            let mut energy = [0.0f64; 3];
            for sample in 0..104_000 {
                let (left, right) = engine.next_stereo();
                let second = sample as f64 / 8_000.0;
                let window = if (3.0..5.0).contains(&second) {
                    Some(0)
                } else if (7.0..9.0).contains(&second) {
                    Some(1)
                } else if (11.0..13.0).contains(&second) {
                    Some(2)
                } else {
                    None
                };
                if let Some(window) = window {
                    energy[window] += f64::from(left * left + right * right);
                }
            }
            energy
        };
        let looped = render(true);
        let bypassed = render(false);
        assert_eq!(looped, render(true));
        assert!(
            looped[0] < bypassed[0] * 0.001,
            "{looped:?} versus {bypassed:?}"
        );
        assert!(looped[1] > bypassed[1] * 0.5);
        assert!(looped[2] < bypassed[2] * 0.001);
    }

    #[test]
    fn capture_codec_roundtrips_four_curves_with_phase_in_a_shareable_code() {
        let mut song = SongState::default();
        song.controls.modules.pad[0] = preset_slot("delay", 0.8);
        song.controls.modules.pad[1] = preset_slot("room", 0.8);
        song.controls.modules.bass[0] = preset_slot("compression", 0.8);
        for (index, id) in ["pad.level", "tonal.level", "lead.level", "bass.level"]
            .into_iter()
            .enumerate()
        {
            song.automation.captures.insert(
                ControlAddress::new(id),
                CaptureClip {
                    samples: std::array::from_fn(|sample| (sample * 37) as u8),
                    origin: -(index as f64) - 0.25,
                    launch: index as f64,
                    enabled: index != 2,
                },
            );
        }
        let code = encode_song_code(&song).unwrap();
        assert!(code.len() < 2000, "{} characters", code.len());
        let decoded = decode_song_code(&code).unwrap();
        assert_eq!(song.automation.captures, decoded.automation.captures);
        let session = LiveSessionSnapshot::from_song(&decoded);
        assert_eq!(song.automation.captures, session.automation.captures);
    }
}
