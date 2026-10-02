//! Retrospective manual-knob history and transport-aligned sixteen-beat loops.

use std::collections::VecDeque;

use super::widget::DialScale;
use super::*;

pub(crate) const CAPTURE_BEATS: f64 = 16.0;
pub(crate) const CAPTURE_SAMPLES: usize = 128;
pub(crate) const MAX_CAPTURES: usize = 4;
const HISTORY_TARGETS: usize = 16;
/// Two full phrases let the player keep the completed phrase through the next.
const HISTORY_BEATS: f64 = CAPTURE_BEATS * 2.0;
const HISTORY_EVENTS: usize = 4_096;

/// A playable Motion phrase. Its compact eight-samples-per-beat resolution
/// makes the duration part of the lane, rather than an accidental property of
/// the old sixteen-beat capture buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MotionDuration {
    Beats4,
    Beats8,
    Beats16,
}

/// A palette-visible Motion command. Capture remains an implementation name
/// only until its on-disk record is replaced; players see one automation lane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MotionAction {
    Grab(MotionDuration),
    Bypass,
    Resume,
    Delete,
}

impl MotionAction {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Grab(MotionDuration::Beats4) => "Motion Grab 4",
            Self::Grab(MotionDuration::Beats8) => "Motion Grab 8",
            Self::Grab(MotionDuration::Beats16) => "Motion Grab 16",
            Self::Bypass => "Motion Bypass",
            Self::Resume => "Motion Resume",
            Self::Delete => "Motion Delete",
        }
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::Grab(_) => "loop recent movement on this knob",
            Self::Bypass => "bypass Motion on this knob",
            Self::Resume => "resume Motion on this knob",
            Self::Delete => "delete Motion on this knob",
        }
    }
}

impl MotionDuration {
    pub(crate) const fn beats(self) -> u8 {
        match self {
            Self::Beats4 => 4,
            Self::Beats8 => 8,
            Self::Beats16 => 16,
        }
    }

    pub(crate) const fn samples(self) -> usize {
        self.beats() as usize * 8
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CaptureHistoryError {
    PhrasePending { beats_remaining: u32 },
    NoMovement,
    IncompleteHistory,
}

impl std::fmt::Display for CaptureHistoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PhrasePending { beats_remaining } => {
                write!(f, "phrase recording; capture in {beats_remaining} beats")
            }
            Self::NoMovement => write!(f, "no edits on this knob in the previous 16 beats"),
            Self::IncompleteHistory => {
                write!(f, "capture history full; record a new 16-beat phrase")
            }
        }
    }
}

impl std::error::Error for CaptureHistoryError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CaptureAction {
    Capture,
    Bypass,
    Resume,
    Delete,
}

impl CaptureAction {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Capture => "Capture",
            Self::Bypass => "Bypass",
            Self::Resume => "Resume",
            Self::Delete => "Delete",
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
    pub(crate) fn playback_spec(&self, mut spec: ControlSpec, beat: f64) -> ControlSpec {
        if self.enabled && beat >= self.launch && spec.kind == ControlKind::Timing {
            // Played intervals may include 0.75 or 1.25, outside the LFO's
            // power-of-two ladder. Preserve the control's own editing grid.
            spec.lfo_snap = LfoSnap::Step;
        }
        spec
    }

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
    matches!(
        spec.kind,
        ControlKind::Gain | ControlKind::Continuous | ControlKind::Timing
    )
}

pub(crate) fn capture_ratio(spec: &ControlSpec, value: f32, controls: &FluidControls) -> f32 {
    let spec = spec.contextual(controls);
    let scale = spec.scale();
    // Match modulation's value-space fallback for beat grids, whose display
    // scale has no inverse. Other controls retain their tapered positions.
    if scale.value_at(0.0).is_some() {
        scale.ratio(value)
    } else {
        DialScale::linear(spec.min, spec.max).ratio(value)
    }
}

/// The bar downbeat a Motion Grab belongs to. Ties choose the earlier bar so
/// a press exactly halfway through the bar joins the phrase already heard.
pub(crate) fn nearest_bar_beat(beat: f64) -> f64 {
    ((beat + 2.0 - f64::EPSILON) / 4.0).floor() * 4.0
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
                    initial: capture_ratio(spec, old, &before.controls),
                    events: VecDeque::new(),
                    lost_before: None,
                });
            while knob
                .events
                .front()
                .is_some_and(|event| event.0 < beat - HISTORY_BEATS)
                || knob.events.len() >= HISTORY_EVENTS
            {
                if let Some((at, value)) = knob.events.pop_front() {
                    knob.initial = value;
                    knob.lost_before = Some(at);
                }
            }
            knob.events
                .push_back((beat, capture_ratio(spec, new, &after.controls)));
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
    ) -> Result<CaptureClip, CaptureHistoryError> {
        let requested = end;
        // Capture always takes the completed phrase before the phrase in which
        // `/capture` was opened. This gives a full sixteen-beat grace phrase.
        let end = (end / CAPTURE_BEATS).floor() * CAPTURE_BEATS;
        let pending = || CaptureHistoryError::PhrasePending {
            beats_remaining: (end + CAPTURE_BEATS - requested).ceil() as u32,
        };
        if end < CAPTURE_BEATS {
            return Err(pending());
        }
        let knob = self
            .knobs
            .iter()
            .find(|knob| knob.target == target)
            .ok_or(CaptureHistoryError::NoMovement)?;
        if knob
            .lost_before
            .is_some_and(|beat| beat >= end - CAPTURE_BEATS)
        {
            return Err(CaptureHistoryError::IncompleteHistory);
        }
        if !knob
            .events
            .iter()
            .any(|event| event.0 >= end - CAPTURE_BEATS && event.0 < end)
        {
            return Err(
                if knob
                    .events
                    .iter()
                    .any(|event| event.0 >= end && event.0 <= requested)
                {
                    pending()
                } else {
                    CaptureHistoryError::NoMovement
                },
            );
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
        Ok(CaptureClip {
            samples,
            origin: launch,
            launch,
            enabled: true,
        })
    }

    /// Build a Motion loop from the finished interval ending at `end`.
    ///
    /// `CaptureClip` still uses its established 128-sample storage while the
    /// Motion format cut is in progress, so a shorter phrase repeats through
    /// that backing buffer. The named bar downbeat is both the history end
    /// and phase anchor: a future anchor queues, while a past anchor joins
    /// the already-running phrase at its current phase.
    pub(crate) fn motion_clip(
        &self,
        target: recipe::RecipeTarget,
        duration: MotionDuration,
        end: f64,
        _now: f64,
    ) -> Result<CaptureClip, CaptureHistoryError> {
        let anchor = nearest_bar_beat(end);
        let end = anchor;
        let beats = f64::from(duration.beats());
        let knob = self
            .knobs
            .iter()
            .find(|knob| knob.target == target)
            .ok_or(CaptureHistoryError::NoMovement)?;
        if knob.lost_before.is_some_and(|beat| beat >= end - beats) {
            return Err(CaptureHistoryError::IncompleteHistory);
        }
        if !knob
            .events
            .iter()
            .any(|event| event.0 >= end - beats && event.0 < end)
        {
            return Err(CaptureHistoryError::NoMovement);
        }
        let samples = std::array::from_fn(|index| {
            let phrase_index = index % duration.samples();
            let beat = end - beats + phrase_index as f64 / 8.0;
            let value = knob
                .events
                .iter()
                .rev()
                .find(|event| event.0 <= beat)
                .map_or(knob.initial, |event| event.1);
            (value.clamp(0.0, 1.0) * 255.0).round() as u8
        });
        Ok(CaptureClip {
            samples,
            origin: anchor,
            launch: anchor,
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
    fn capture_keeps_the_previous_sixteen_beat_phrase_through_the_following_phrase() {
        let mut snapshot = LiveSessionSnapshot::from_controls(FluidControls::default());
        snapshot.controls.pad.level = 0.0;
        let target = recipe::RecipeTarget::capture("pad.level", &snapshot).unwrap();
        let mut history = CaptureHistory::default();
        change(&mut history, &mut snapshot, 0.0, 1.0);
        change(&mut history, &mut snapshot, 8.0, 0.5);
        let clip = history.clip(target, 24.0, 24.0).unwrap();
        assert_eq!(clip.origin, 28.0);
        assert_eq!(clip.position(27.999), None);
        assert_eq!(clip.samples[0], 255);
        assert_eq!(clip.samples[64], 128);
        assert_eq!(clip.position(28.0), clip.position(44.0));

        // The capture window remains the first phrase until the grace phrase
        // ends, even when the palette opens near its final bar.
        let late = history.clip(target, 31.9, 31.9).unwrap();
        assert_eq!(late.samples, clip.samples);
        assert_eq!(
            history.clip(target, 32.0, 32.0),
            Err(CaptureHistoryError::NoMovement)
        );
    }

    #[test]
    fn motion_grab_uses_the_recent_requested_window_and_repeats_its_phrase() {
        let mut snapshot = LiveSessionSnapshot::from_controls(FluidControls::default());
        snapshot.controls.pad.level = 0.0;
        let target = recipe::RecipeTarget::capture("pad.level", &snapshot).unwrap();
        let mut history = CaptureHistory::default();
        change(&mut history, &mut snapshot, 8.0, 1.0);
        change(&mut history, &mut snapshot, 11.0, 0.5);

        let clip = history
            .motion_clip(target, MotionDuration::Beats4, 12.0, 12.0)
            .unwrap();

        assert_eq!(clip.origin, 12.0);
        assert_eq!(clip.samples[0], 255);
        assert_eq!(clip.samples[24], 128);
        assert_eq!(clip.samples[32], clip.samples[0]);
        assert_eq!(clip.samples[56], clip.samples[24]);
    }

    #[test]
    fn motion_grab_queues_before_its_nearest_downbeat_and_joins_after_it() {
        let mut snapshot = LiveSessionSnapshot::from_controls(FluidControls::default());
        snapshot.controls.pad.level = 0.0;
        let target = recipe::RecipeTarget::capture("pad.level", &snapshot).unwrap();
        let mut history = CaptureHistory::default();
        change(&mut history, &mut snapshot, 0.0, 1.0);

        let early = history
            .motion_clip(target, MotionDuration::Beats4, 3.0, 3.0)
            .unwrap();
        assert_eq!(early.launch, 4.0);
        assert_eq!(early.position(3.99), None);
        assert_eq!(early.position(4.0), Some(1.0));

        change(&mut history, &mut snapshot, 1.0, 0.5);
        let late = history
            .motion_clip(target, MotionDuration::Beats4, 5.0, 5.0)
            .unwrap();
        assert_eq!(late.launch, 4.0);
        assert_eq!(late.position(5.0), Some(128.0 / 255.0));
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
            for elapsed in [0.0, 0.5, 2.0, 16.0, 64.0, 71.0] {
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
    fn capture_refuses_history_overflow_until_a_complete_phrase_is_available() {
        let mut snapshot = LiveSessionSnapshot::from_controls(FluidControls::default());
        let target = recipe::RecipeTarget::capture("pad.level", &snapshot).unwrap();
        let mut history = CaptureHistory::default();
        for index in 0..(HISTORY_EVENTS + 1) {
            change(
                &mut history,
                &mut snapshot,
                index as f64 / 1_000.0,
                (index % 2) as f32,
            );
        }
        assert_eq!(
            history.clip(target, 16.0, 16.0),
            Err(CaptureHistoryError::IncompleteHistory)
        );
        change(&mut history, &mut snapshot, 16.0, 0.3);
        assert!(history.clip(target, 33.0, 33.0).is_ok());
        assert!(history.knobs[0].events.len() <= HISTORY_EVENTS);
    }

    #[test]
    fn timing_capture_replays_the_edited_grid_values_after_save_and_load() {
        for id in [
            "kick.interval_beats",
            "kick.offset_beats",
            "pad.chord_bars",
            "tonal.attack",
        ] {
            let spec = spec_by_id(id).unwrap();
            let mut snapshot = LiveSessionSnapshot::from_controls(FluidControls::default());
            let target = recipe::RecipeTarget::capture(id, &snapshot).unwrap();
            let mut history = CaptureHistory::default();
            let before = snapshot.clone();
            spec.apply_delta(1.0, &mut snapshot.controls);
            let expected = (spec.get)(&snapshot.controls);
            history.record_changes(&before, &snapshot, 1.0);
            let mut clip = history.clip(target, 16.0, 16.0).unwrap();
            clip.rebase(16.0);
            let mut song = SongState::default();
            song.automation
                .captures
                .insert(ControlAddress::new(id), clip);
            let code = encode_song_code(&song).unwrap();
            let restored = decode_song_code(&code).unwrap();
            let mut controls = restored.controls;
            apply_automation(
                &mut controls,
                &restored.automation,
                TimingContext::new(44100.0, 120.0, 8.0),
            );
            let actual = (spec.get)(&controls);
            if matches!(spec.step, Step::BeatGrid | Step::PowerOfTwo) {
                assert_eq!(actual, expected, "{id}");
            } else {
                assert!(
                    (capture_ratio(spec, actual, &controls)
                        - capture_ratio(spec, expected, &controls))
                    .abs()
                        < 1.0 / 255.0,
                    "{id}: {actual} != {expected}"
                );
            }
        }
    }

    #[test]
    fn capture_reports_pending_phrase_and_excludes_edits_on_its_end_boundary() {
        let mut snapshot = LiveSessionSnapshot::from_controls(FluidControls::default());
        let target = recipe::RecipeTarget::capture("pad.level", &snapshot).unwrap();
        let mut history = CaptureHistory::default();
        change(&mut history, &mut snapshot, 16.0, 0.3);
        assert_eq!(
            history.clip(target, 8.0, 8.0),
            Err(CaptureHistoryError::PhrasePending { beats_remaining: 8 })
        );
        assert_eq!(
            history.clip(target, 16.0, 16.0),
            Err(CaptureHistoryError::PhrasePending {
                beats_remaining: 16
            })
        );
        assert!(history.clip(target, 32.0, 32.0).is_ok());
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
