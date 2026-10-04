//! Retrospective manual-knob history and transport-aligned Motion loops.

use std::collections::VecDeque;

use super::widget::DialScale;
use super::*;

pub(crate) const CAPTURE_BEATS: f64 = 16.0;
pub(crate) const CAPTURE_SAMPLES: usize = 128;
/// Operational ceiling for simultaneously saved Motion phrases. Playback is
/// map-backed; this bounds only deliberately admitted state and song payload.
pub(crate) const MAX_CAPTURES: usize = 16;
const HISTORY_TARGETS: usize = 16;
/// Two full phrases let the player keep the completed phrase through the next.
const HISTORY_BEATS: f64 = 64.0;
const HISTORY_EVENTS: usize = 4_096;
const EVENT_TICKS_PER_BEAT: f64 = 256.0;
pub(crate) const MAX_MOTION_EVENTS: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MotionEvent {
    pub(crate) tick: u16,
    pub(crate) position: u16,
}

/// Named loop length. Sampled curves use eight positions per beat through
/// four bars and four per beat for eight bars, bounded at 128 positions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MotionDuration {
    Beats4,
    Beats8,
    Beats16,
    Beats32,
}

/// A palette-visible Motion command. Players see one automation lane.
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
            Self::Grab(MotionDuration::Beats32) => "Motion Grab 32",
            Self::Bypass => "Motion Bypass",
            Self::Resume => "Motion Resume",
            Self::Delete => "Motion Delete",
        }
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::Grab(MotionDuration::Beats4) => "loop the last 1 bar",
            Self::Grab(MotionDuration::Beats8) => "loop the last 2 bars",
            Self::Grab(MotionDuration::Beats16) => "loop the last 4 bars",
            Self::Grab(MotionDuration::Beats32) => "loop the last 8 bars",
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
            Self::Beats32 => 32,
        }
    }

    pub(crate) const fn samples(self) -> usize {
        match self {
            Self::Beats32 => CAPTURE_SAMPLES,
            _ => self.beats() as usize * 8,
        }
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
            Self::NoMovement => write!(f, "no recent edits on this control"),
            Self::IncompleteHistory => {
                write!(f, "Motion history full; record a new phrase")
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
pub(crate) enum MotionData {
    /// Recorded positions minus this fixed reference shift the authored base.
    Relative {
        reference: u16,
        samples: [u8; CAPTURE_SAMPLES],
    },
    /// Choices retain their exact identities and yield to a manual edit.
    Absolute { events: Vec<MotionEvent> },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CaptureClip {
    /// The actual phrase length; unused sampled storage is never encoded.
    pub(crate) duration: MotionDuration,
    pub(crate) data: MotionData,
    /// Loop phase anchor and admission beat are distinct: resume keeps phase.
    pub(crate) origin: f64,
    pub(crate) launch: f64,
    pub(crate) enabled: bool,
}

impl CaptureClip {
    #[cfg(test)]
    fn samples(&self) -> &[u8; CAPTURE_SAMPLES] {
        match &self.data {
            MotionData::Relative { samples, .. } => samples,
            MotionData::Absolute { .. } => panic!("expected sampled Motion"),
        }
    }

    #[cfg(test)]
    fn events(&self) -> &[MotionEvent] {
        match &self.data {
            MotionData::Relative { .. } => &[],
            MotionData::Absolute { events } => events,
        }
    }

    pub(crate) fn is_relative(&self) -> bool {
        matches!(self.data, MotionData::Relative { .. })
    }

    pub(crate) fn set_reference(&mut self, position: f32) {
        if let MotionData::Relative { reference, .. } = &mut self.data {
            *reference = (position.clamp(0.0, 1.0) * f32::from(u16::MAX)).round() as u16;
        }
    }

    pub(crate) fn delta(&self, base_position: f32, beat: f64) -> f32 {
        self.position(beat).map_or(0.0, |position| {
            position - self.reference_position(base_position)
        })
    }

    fn reference_position(&self, base_position: f32) -> f32 {
        match self.data {
            MotionData::Relative { reference, .. } => f32::from(reference) / f32::from(u16::MAX),
            MotionData::Absolute { .. } => base_position,
        }
    }

    pub(crate) fn position_from_base(&self, base_position: f32, beat: f64) -> Option<f32> {
        self.position(beat).map(|position| {
            (base_position + position - self.reference_position(base_position)).clamp(0.0, 1.0)
        })
    }

    pub(crate) fn playback_spec(&self, mut spec: ControlSpec, beat: f64) -> ControlSpec {
        if self.enabled
            && beat >= self.launch
            && matches!(spec.kind, ControlKind::Timing | ControlKind::Discrete)
        {
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
        let sample_count = self.duration.samples();
        let phase = (beat - self.origin).rem_euclid(f64::from(self.duration.beats()));
        if let MotionData::Absolute { events } = &self.data {
            let tick = (phase * EVENT_TICKS_PER_BEAT).floor() as u16;
            let value = events
                .iter()
                .rev()
                .find(|event| event.tick <= tick)
                .or_else(|| events.first())?;
            return Some(f32::from(value.position) / f32::from(u16::MAX));
        }
        let MotionData::Relative { samples, .. } = &self.data else {
            return None;
        };
        let position = phase * sample_count as f64 / f64::from(self.duration.beats());
        let index = position.floor() as usize % sample_count;
        let a = f32::from(samples[index]);
        // A captured phrase holds its terminal value through its final sample.
        // The de-clicker, not sample interpolation, handles the reset at the
        // next phrase downbeat.
        let b = f32::from(if index + 1 < sample_count {
            samples[index + 1]
        } else {
            samples[index]
        });
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
        self.origin = -(beat - self.origin).rem_euclid(f64::from(self.duration.beats()));
        self.launch = (self.launch - beat).max(0.0);
    }
}

pub(crate) fn capture_eligible(spec: &ControlSpec) -> bool {
    matches!(
        spec.kind,
        ControlKind::Gain | ControlKind::Continuous | ControlKind::Timing | ControlKind::Discrete
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
    kind: ControlKind,
    initial: f32,
    events: VecDeque<(f64, f32)>,
    lost_before: Option<f64>,
}

struct EditorHistory {
    target: EditorMotionAddress,
    initial: f32,
    events: VecDeque<(f64, f32)>,
    lost_before: Option<f64>,
}

fn same_editor_history_owner(
    control: ControlAddress,
    kind: ModKind,
    before: &LiveSessionSnapshot,
    after: &LiveSessionSnapshot,
) -> bool {
    let module_kind = |snapshot: &LiveSessionSnapshot| {
        module_slot_row(control.id(), &snapshot.controls).map(|(slot, _)| slot.kind.to_bits())
    };
    let lane_count = |snapshot: &LiveSessionSnapshot| match kind {
        ModKind::Lfo => snapshot.automation.lfo_lanes(control).len(),
        ModKind::Envelope => snapshot.automation.envelope_lanes(control).len(),
    };
    module_kind(before) == module_kind(after) && lane_count(before) == lane_count(after)
}

#[derive(Default)]
pub(crate) struct CaptureHistory {
    knobs: VecDeque<KnobHistory>,
    fields: VecDeque<EditorHistory>,
}

impl CaptureHistory {
    pub(crate) fn record_changes(
        &mut self,
        before: &LiveSessionSnapshot,
        after: &LiveSessionSnapshot,
        beat: f64,
    ) {
        // An ordinal can name another lane after structural edits. Clear the
        // affected family's history rather than replaying its former owner.
        self.fields.retain(|field| {
            same_editor_history_owner(
                field.target.control,
                field.target.target.kind(),
                before,
                after,
            )
        });
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
            // Slot replacement and progression selection change the meaning
            // behind an address; neither is a played edit of its fields.
            if !target.is_current(before) {
                continue;
            }
            let index = self.knobs.iter().position(|knob| knob.target == target);
            let mut knob = index
                .and_then(|index| self.knobs.remove(index))
                .unwrap_or_else(|| KnobHistory {
                    target,
                    kind: spec.contextual(&after.controls).kind,
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
        for (control, lane_index, route) in after.automation.indexed_routes() {
            if !same_editor_history_owner(control, ModKind::Lfo, before, after) {
                continue;
            }
            let Some(before_route) = before.automation.route_at(control, lane_index) else {
                continue;
            };
            for field in LfoField::ALL {
                let target = EditorMotionAddress {
                    control,
                    lane_index,
                    target: EditorMotionField::Lfo(field),
                };
                self.record_editor_change(
                    target,
                    field.motion_position(before_route),
                    field.motion_position(route),
                    beat,
                );
            }
            if route.shape == LfoShape::Steps && before_route.shape == LfoShape::Steps {
                for step in [StepTarget::Count, StepTarget::Glide]
                    .into_iter()
                    .chain((0..MAX_LFO_STEPS).map(StepTarget::Value))
                {
                    let target = EditorMotionAddress {
                        control,
                        lane_index,
                        target: EditorMotionField::Step(step),
                    };
                    self.record_editor_change(
                        target,
                        step.motion_position(before_route),
                        step.motion_position(route),
                        beat,
                    );
                }
            }
        }
        for (control, lane_index, route) in after.automation.indexed_envelopes() {
            if !same_editor_history_owner(control, ModKind::Envelope, before, after) {
                continue;
            }
            let Some(before_route) = before.automation.envelope_at(control, lane_index) else {
                continue;
            };
            for field in EnvField::ALL {
                let target = EditorMotionAddress {
                    control,
                    lane_index,
                    target: EditorMotionField::Envelope(field),
                };
                self.record_editor_change(
                    target,
                    field.motion_position(before_route),
                    field.motion_position(route),
                    beat,
                );
            }
        }
    }

    fn record_editor_change(&mut self, target: EditorMotionAddress, old: f32, new: f32, beat: f64) {
        if old.to_bits() == new.to_bits() {
            return;
        }
        let index = self.fields.iter().position(|field| field.target == target);
        let mut field = index
            .and_then(|index| self.fields.remove(index))
            .unwrap_or_else(|| EditorHistory {
                target,
                initial: old,
                events: VecDeque::new(),
                lost_before: None,
            });
        while field
            .events
            .front()
            .is_some_and(|event| event.0 < beat - HISTORY_BEATS)
            || field.events.len() >= HISTORY_EVENTS
        {
            if let Some((at, value)) = field.events.pop_front() {
                field.initial = value;
                field.lost_before = Some(at);
            }
        }
        field.events.push_back((beat, new));
        self.fields.push_back(field);
        while self.fields.len() > HISTORY_TARGETS {
            self.fields.pop_front();
        }
    }

    pub(crate) fn editor_motion_clip(
        &self,
        target: EditorMotionAddress,
        duration: MotionDuration,
        end: f64,
    ) -> Result<CaptureClip, CaptureHistoryError> {
        let step = self
            .fields
            .iter()
            .find(|step| step.target == target)
            .ok_or(CaptureHistoryError::NoMovement)?;
        build_motion_clip(
            duration,
            end,
            target.target.is_discrete(),
            step.initial,
            &step.events,
            step.lost_before,
        )
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
        if knob.kind == ControlKind::Discrete {
            let mut clip = self.motion_clip(target, MotionDuration::Beats16, end, now)?;
            clip.origin = next_bar_beat(now);
            clip.launch = clip.origin;
            return Ok(clip);
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
            duration: MotionDuration::Beats16,
            data: MotionData::Relative {
                reference: (knob.events.back().map_or(knob.initial, |event| event.1)
                    * f32::from(u16::MAX))
                .round() as u16,
                samples,
            },
            origin: launch,
            launch,
            enabled: true,
        })
    }

    /// Build a Motion loop from the finished interval ending at `end`.
    ///
    /// The named bar downbeat is both the history end
    /// and phase anchor: a future anchor queues, while a past anchor joins
    /// the already-running phrase at its current phase.
    pub(crate) fn motion_clip(
        &self,
        target: recipe::RecipeTarget,
        duration: MotionDuration,
        end: f64,
        _now: f64,
    ) -> Result<CaptureClip, CaptureHistoryError> {
        let knob = self
            .knobs
            .iter()
            .find(|knob| knob.target == target)
            .ok_or(CaptureHistoryError::NoMovement)?;
        build_motion_clip(
            duration,
            end,
            knob.kind == ControlKind::Discrete,
            knob.initial,
            &knob.events,
            knob.lost_before,
        )
    }
}

/// Build either representation from the same musical window. Both registry
/// controls and inline step rows use this path so phase and tail rules agree.
fn build_motion_clip(
    duration: MotionDuration,
    requested_end: f64,
    discrete: bool,
    initial: f32,
    changes: &VecDeque<(f64, f32)>,
    lost_before: Option<f64>,
) -> Result<CaptureClip, CaptureHistoryError> {
    let end = nearest_bar_beat(requested_end);
    let beats = f64::from(duration.beats());
    let start = end - beats;
    if lost_before.is_some_and(|beat| beat >= start) {
        return Err(CaptureHistoryError::IncompleteHistory);
    }
    if !changes
        .iter()
        .any(|event| event.0 >= start && event.0 < end)
    {
        return Err(CaptureHistoryError::NoMovement);
    }
    let value_at = |beat: f64| {
        changes
            .iter()
            .rev()
            .find(|event| event.0 <= beat)
            .map_or(initial, |event| event.1)
    };
    let events = if discrete {
        let mut events = vec![MotionEvent {
            tick: 0,
            position: (value_at(start).clamp(0.0, 1.0) * f32::from(u16::MAX)).round() as u16,
        }];
        for &(at, value) in changes {
            if at > start && at < end {
                let tick = ((at - start) * EVENT_TICKS_PER_BEAT)
                    .round()
                    .min(beats * EVENT_TICKS_PER_BEAT - 1.0) as u16;
                let position = (value.clamp(0.0, 1.0) * f32::from(u16::MAX)).round() as u16;
                if let Some(last) = events.last_mut()
                    && last.tick == tick
                {
                    last.position = position;
                } else {
                    events.push(MotionEvent { tick, position });
                }
            }
        }
        if events.len() > MAX_MOTION_EVENTS {
            return Err(CaptureHistoryError::IncompleteHistory);
        }
        events
    } else {
        Vec::new()
    };
    let samples = if discrete {
        [0; CAPTURE_SAMPLES]
    } else {
        std::array::from_fn(|index| {
            if index >= duration.samples() {
                return 0;
            }
            let beat = start + index as f64 * beats / duration.samples() as f64;
            (value_at(beat).clamp(0.0, 1.0) * 255.0).round() as u8
        })
    };
    Ok(CaptureClip {
        duration,
        data: if discrete {
            MotionData::Absolute { events }
        } else {
            MotionData::Relative {
                reference: (changes.back().map_or(initial, |event| event.1) * f32::from(u16::MAX))
                    .round() as u16,
                samples,
            }
        },
        origin: end,
        launch: end,
        enabled: true,
    })
}

pub(crate) fn suspend_changed_captures(
    before: &LiveSessionSnapshot,
    after: &mut LiveSessionSnapshot,
) {
    for (address, clip) in &mut after.automation.captures {
        let spec = address.spec();
        if parse_chord_slot_id(address.id()).is_some()
            && progression_index(before.controls.pad.progression)
                != progression_index(after.controls.pad.progression)
        {
            continue;
        }
        if !clip.is_relative()
            && (spec.get)(&before.controls).to_bits() != (spec.get)(&after.controls).to_bits()
        {
            clip.enabled = false;
        }
    }
    if !after.automation.editor_captures.is_empty() {
        let after_routes = after.automation.clone();
        for (target, clip) in &mut after.automation.editor_captures {
            let position = |automation: &AutomationState| {
                target
                    .target
                    .position(
                        automation.route_at(target.control, target.lane_index),
                        automation.envelope_at(target.control, target.lane_index),
                    )
                    .map(f32::to_bits)
            };
            let changed = position(&before.automation) != position(&after_routes);
            if changed && !clip.is_relative() {
                clip.enabled = false;
            }
        }
    }
}

pub(crate) fn suspend_capture(snapshot: &mut LiveSessionSnapshot, id: &'static str) {
    if let Some(clip) = snapshot
        .automation
        .captures
        .get_mut(&ControlAddress::new(id))
        && !clip.is_relative()
    {
        clip.enabled = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fluid::song::SongCodeError;

    #[test]
    fn discrete_motion_replays_lead_pitch_and_pad_trigger_as_exact_events() {
        let mut snapshot = LiveSessionSnapshot::from_controls(FluidControls::default());
        let lead = recipe::RecipeTarget::capture("lead.step1", &snapshot).unwrap();
        let trigger = recipe::RecipeTarget::capture("pad.trigger", &snapshot).unwrap();
        let lead_initial = snapshot.controls.lead.steps[0];
        let trigger_initial = snapshot.controls.pad.trigger;
        let mut history = CaptureHistory::default();
        let before = snapshot.clone();
        snapshot.controls.lead.steps[0] = 5.0;
        snapshot.controls.pad.trigger = 1.0 - trigger_initial;
        history.record_changes(&before, &snapshot, 0.5);
        let before = snapshot.clone();
        snapshot.controls.lead.steps[0] = 8.0;
        history.record_changes(&before, &snapshot, 1.25);

        let lead_clip = history
            .motion_clip(lead, MotionDuration::Beats4, 4.0, 4.0)
            .unwrap();
        let trigger_clip = history
            .motion_clip(trigger, MotionDuration::Beats4, 4.0, 4.0)
            .unwrap();
        assert_eq!(
            lead_clip
                .events()
                .iter()
                .map(|event| event.tick)
                .collect::<Vec<_>>(),
            vec![0, 128, 320]
        );
        assert_eq!(
            trigger_clip
                .events()
                .iter()
                .map(|event| event.tick)
                .collect::<Vec<_>>(),
            vec![0, 128]
        );
        assert!(
            (lead_clip.position(4.49).unwrap()
                - capture_ratio(
                    spec_by_id("lead.step1").unwrap(),
                    lead_initial,
                    &snapshot.controls
                ))
            .abs()
                < 1.0 / f32::from(u16::MAX)
        );
        let mut automation = AutomationState::default();
        automation
            .captures
            .insert(ControlAddress::new("lead.step1"), lead_clip);
        automation
            .captures
            .insert(ControlAddress::new("pad.trigger"), trigger_clip);
        let mut plan = AutomationPlan::default();
        plan.rebuild(&automation);
        for (beat, pitch, mode) in [
            (4.25, lead_initial, trigger_initial),
            (4.5, 5.0, 1.0 - trigger_initial),
            (5.25, 8.0, 1.0 - trigger_initial),
            (8.0, lead_initial, trigger_initial),
        ] {
            let mut controls = snapshot.controls.clone();
            plan.apply(&mut controls, TimingContext::new(44_100.0, 120.0, beat));
            assert_eq!(controls.lead.steps[0], pitch, "beat {beat}");
            assert_eq!(controls.pad.trigger, mode, "beat {beat}");
        }
    }

    #[test]
    fn discrete_motion_song_code_restores_phase_and_rejects_sampled_switch_data() {
        let mut song = SongState::default();
        let address = ControlAddress::new("lead.step1");
        let clip = CaptureClip {
            duration: MotionDuration::Beats4,
            data: MotionData::Absolute {
                events: vec![
                    MotionEvent {
                        tick: 0,
                        position: 0,
                    },
                    MotionEvent {
                        tick: 128,
                        position: u16::MAX,
                    },
                ],
            },
            origin: -1.0,
            launch: 0.0,
            enabled: true,
        };
        song.automation.captures.insert(address, clip.clone());
        let code = encode_song_code(&song).unwrap();
        let restored = decode_song_code(&code).unwrap();
        assert_eq!(restored.automation.captures[&address], clip);
        let mut controls = restored.controls.clone();
        apply_automation(
            &mut controls,
            &restored.automation,
            TimingContext::new(44_100.0, 120.0, 1.0),
        );
        assert_eq!(controls.lead.steps[0], LEAD_TONE_COUNT as f32);

        song.automation.captures.get_mut(&address).unwrap().data = MotionData::Relative {
            reference: 0,
            samples: [0; CAPTURE_SAMPLES],
        };
        assert_eq!(
            encode_song_code(&song).err(),
            Some(SongCodeError::InvalidCapture)
        );
    }

    #[test]
    fn motion_on_a_step_lfo_value_changes_the_played_rung_and_restores_from_song_code() {
        let mut snapshot = LiveSessionSnapshot::from_controls(FluidControls::default());
        let address = ControlAddress::new("pad.level");
        let route = snapshot.automation.open_or_create(address);
        route.shape = LfoShape::Steps;
        route.step_count = 1;
        route.step_glide = 0.0;
        route.depth_ratio = 1.0;
        route.steps[0] = 0.0;
        let target = EditorMotionAddress {
            control: address,
            lane_index: 0,
            target: EditorMotionField::Step(StepTarget::Value(0)),
        };
        let mut history = CaptureHistory::default();
        let before = snapshot.clone();
        snapshot
            .automation
            .route_mut(address)
            .unwrap()
            .set_step(StepTarget::Value(0), 50.0);
        history.record_changes(&before, &snapshot, 0.5);
        let before = snapshot.clone();
        snapshot
            .automation
            .route_mut(address)
            .unwrap()
            .set_step(StepTarget::Value(0), 100.0);
        history.record_changes(&before, &snapshot, 1.25);
        let mut clip = history
            .editor_motion_clip(target, MotionDuration::Beats4, 4.0)
            .unwrap();
        assert!(clip.events().is_empty());
        clip.rebase(4.0);
        let mut song = SongState {
            controls: snapshot.controls,
            automation: snapshot.automation,
            ..SongState::default()
        };
        song.automation.editor_captures.insert(target, clip.clone());
        let restored = decode_song_code(&encode_song_code(&song).unwrap()).unwrap();
        assert_eq!(restored.automation.editor_captures[&target], clip);
        let mut plan = AutomationPlan::default();
        plan.rebuild(&restored.automation);
        let mut before_edit = restored.controls.clone();
        plan.apply(&mut before_edit, TimingContext::new(44_100.0, 120.0, 0.25));
        let mut after_edit = restored.controls.clone();
        plan.apply(&mut after_edit, TimingContext::new(44_100.0, 120.0, 0.75));
        assert!(
            after_edit.pad.level > before_edit.pad.level,
            "{} vs {}",
            after_edit.pad.level,
            before_edit.pad.level
        );
    }

    #[test]
    fn motion_on_step_lfo_count_replays_only_valid_whole_counts() {
        let mut snapshot = LiveSessionSnapshot::from_controls(FluidControls::default());
        let address = ControlAddress::new("pad.level");
        let route = snapshot.automation.open_or_create(address);
        route.shape = LfoShape::Steps;
        route.step_count = 2;
        let target = EditorMotionAddress {
            control: address,
            lane_index: 0,
            target: EditorMotionField::Step(StepTarget::Count),
        };
        let mut history = CaptureHistory::default();
        let before = snapshot.clone();
        snapshot
            .automation
            .route_mut(address)
            .unwrap()
            .set_step(StepTarget::Count, 4.0);
        history.record_changes(&before, &snapshot, 0.5);
        let before = snapshot.clone();
        snapshot
            .automation
            .route_mut(address)
            .unwrap()
            .set_step(StepTarget::Count, 8.0);
        history.record_changes(&before, &snapshot, 1.25);
        let mut clip = history
            .editor_motion_clip(target, MotionDuration::Beats4, 4.0)
            .unwrap();
        assert_eq!(
            clip.events()
                .iter()
                .map(|event| event.tick)
                .collect::<Vec<_>>(),
            vec![0, 128, 320]
        );
        clip.rebase(4.0);
        for (beat, count) in [(0.25, 2), (0.75, 4), (1.5, 8), (4.0, 2)] {
            let mut route = *snapshot.automation.route(address).unwrap();
            let EditorMotionField::Step(step) = target.target else {
                unreachable!()
            };
            step.apply_motion_position(&mut route, clip.position(beat).unwrap());
            assert_eq!(route.step_count, count, "beat {beat}");
        }
        let mut song = SongState {
            automation: snapshot.automation,
            ..SongState::default()
        };
        song.automation.editor_captures.insert(target, clip.clone());
        let restored = decode_song_code(&encode_song_code(&song).unwrap()).unwrap();
        assert_eq!(restored.automation.editor_captures[&target], clip);
    }

    #[test]
    fn motion_on_lfo_shape_and_envelope_trigger_replays_enum_events_after_restore() {
        let mut snapshot = LiveSessionSnapshot::from_controls(FluidControls::default());
        let address = ControlAddress::new("pad.level");
        snapshot.automation.open_or_create(address);
        snapshot.automation.open_or_create_envelope(address);
        let mut history = CaptureHistory::default();
        let before = snapshot.clone();
        snapshot
            .automation
            .route_mut(address)
            .unwrap()
            .set_field_at(LfoField::Shape, 4.0, 0.5);
        snapshot
            .automation
            .envelope_mut(address)
            .unwrap()
            .set_field(EnvField::Trigger, 7.0);
        history.record_changes(&before, &snapshot, 0.5);
        let shape = EditorMotionAddress {
            control: address,
            lane_index: 0,
            target: EditorMotionField::Lfo(LfoField::Shape),
        };
        let trigger = EditorMotionAddress {
            control: address,
            lane_index: 0,
            target: EditorMotionField::Envelope(EnvField::Trigger),
        };
        let mut shape_clip = history
            .editor_motion_clip(shape, MotionDuration::Beats4, 4.0)
            .unwrap();
        let mut trigger_clip = history
            .editor_motion_clip(trigger, MotionDuration::Beats4, 4.0)
            .unwrap();
        assert_eq!(shape_clip.events().len(), 2);
        assert_eq!(trigger_clip.events().len(), 2);
        shape_clip.rebase(4.0);
        trigger_clip.rebase(4.0);
        let mut song = SongState {
            automation: snapshot.automation,
            ..SongState::default()
        };
        song.automation.editor_captures.insert(shape, shape_clip);
        song.automation
            .editor_captures
            .insert(trigger, trigger_clip);
        let restored = decode_song_code(&encode_song_code(&song).unwrap()).unwrap();
        let mut lfo = *restored.automation.route_at(address, 0).unwrap();
        LfoField::Shape.apply_motion_position(
            &mut lfo,
            restored.automation.editor_captures[&shape]
                .position(0.25)
                .unwrap(),
        );
        assert_eq!(lfo.shape, LfoShape::Sine);
        LfoField::Shape.apply_motion_position(
            &mut lfo,
            restored.automation.editor_captures[&shape]
                .position(0.75)
                .unwrap(),
        );
        assert_eq!(lfo.shape, LfoShape::Square);
        let mut env = *restored.automation.envelope_at(address, 0).unwrap();
        EnvField::Trigger.apply_motion_position(
            &mut env,
            restored.automation.editor_captures[&trigger]
                .position(0.25)
                .unwrap(),
        );
        assert_eq!(env.trigger, EnvTrigger::EveryBeats(4.0));
        EnvField::Trigger.apply_motion_position(
            &mut env,
            restored.automation.editor_captures[&trigger]
                .position(0.75)
                .unwrap(),
        );
        assert_eq!(env.trigger, EnvTrigger::Once);
    }

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
        assert_eq!(clip.samples()[0], 255);
        assert_eq!(clip.samples()[64], 128);
        assert_eq!(clip.position(28.0), clip.position(44.0));

        // The capture window remains the first phrase until the grace phrase
        // ends, even when the palette opens near its final bar.
        let late = history.clip(target, 31.9, 31.9).unwrap();
        assert_eq!(late.samples(), clip.samples());
        assert_eq!(
            history.clip(target, 32.0, 32.0),
            Err(CaptureHistoryError::NoMovement)
        );
    }

    #[test]
    fn motion_grab_uses_only_its_requested_duration() {
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
        assert_eq!(clip.duration, MotionDuration::Beats4);
        assert_eq!(clip.samples()[0], 255);
        assert_eq!(clip.samples()[24], 128);
        assert_eq!(clip.samples()[32], 0);
        assert_eq!(clip.samples()[56], 0);
    }

    #[test]
    fn motion_grab_holds_its_terminal_value_until_the_loop_boundary() {
        let mut snapshot = LiveSessionSnapshot::from_controls(FluidControls::default());
        snapshot.controls.pad.level = 0.0;
        let target = recipe::RecipeTarget::capture("pad.level", &snapshot).unwrap();
        let mut history = CaptureHistory::default();
        change(&mut history, &mut snapshot, 7.0, 1.0);

        let clip = history
            .motion_clip(target, MotionDuration::Beats8, 7.0, 7.0)
            .unwrap();

        assert_eq!(clip.origin, 8.0);
        assert_eq!(clip.position(15.0), Some(1.0));
        assert_eq!(clip.position(15.99), Some(1.0));
        assert_eq!(clip.position(16.0), Some(0.0));
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
            duration: MotionDuration::Beats16,
            data: MotionData::Relative {
                reference: 0,
                samples: [0; CAPTURE_SAMPLES],
            },
            origin: 20.0,
            launch: 20.0,
            enabled: true,
        };
        if let MotionData::Relative { samples, .. } = &mut clip.data {
            samples[1] = 255;
        }
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
            let mut song = SongState::from_controls(snapshot.controls.clone());
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
                duration: MotionDuration::Beats16,
                data: MotionData::Relative {
                    reference: 45875,
                    samples: [51; CAPTURE_SAMPLES],
                },
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
                    duration: MotionDuration::Beats16,
                    data: MotionData::Relative {
                        reference: u16::MAX,
                        samples: std::array::from_fn(|index| if index < 64 { 0 } else { 255 }),
                    },
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
        for (index, (id, duration)) in [
            ("pad.level", MotionDuration::Beats4),
            ("tonal.level", MotionDuration::Beats8),
            ("lead.level", MotionDuration::Beats16),
            ("bass.level", MotionDuration::Beats4),
        ]
        .into_iter()
        .enumerate()
        {
            song.automation.captures.insert(
                ControlAddress::new(id),
                CaptureClip {
                    duration,
                    data: MotionData::Relative {
                        reference: 32768,
                        samples: std::array::from_fn(|sample| {
                            if sample < duration.samples() {
                                (sample * 37) as u8
                            } else {
                                0
                            }
                        }),
                    },
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

    fn relative_clip(duration: MotionDuration, reference: f32, value: u8) -> CaptureClip {
        let mut clip = CaptureClip {
            duration,
            data: MotionData::Relative {
                reference: 0,
                samples: [value; CAPTURE_SAMPLES],
            },
            origin: 0.0,
            launch: 0.0,
            enabled: true,
        };
        clip.set_reference(reference);
        clip
    }

    #[test]
    fn relative_motion_sums_opposing_lanes_before_clamping_and_tracks_cutoff_positions() {
        let address = ControlAddress::new("pad.level");
        let mut automation = AutomationState::default();
        automation
            .captures
            .insert(address, relative_clip(MotionDuration::Beats32, 0.4, 153));
        let lane = LfoRoute {
            shape: LfoShape::Sine,
            cycle_beats: 1.0,
            phase_offset_beats: 0.75,
            depth_ratio: 0.2,
            ..LfoRoute::default()
        };
        assert_eq!(lane.wave_at(0.0), -1.0);
        automation.add_route(address, lane);
        let mut controls = FluidControls::default();
        controls.pad.level = 0.9;
        apply_automation(
            &mut controls,
            &automation,
            TimingContext::new(8_000.0, 120.0, 0.0),
        );
        assert!(
            (controls.pad.level - 0.9).abs() < 0.0001,
            "{}",
            controls.pad.level
        );

        let address = ControlAddress::new("bass.slot1.time");
        let spec = address.spec().contextual(&controls);
        let reference = capture_ratio(&spec, 1000.0, &controls);
        let mut automation = AutomationState::default();
        let clip = relative_clip(MotionDuration::Beats16, reference, 102);
        let delta = clip.delta(reference, 0.0);
        automation.captures.insert(address, clip);
        for base in [100.0, 1_000.0, 10_000.0] {
            (spec.set)(&mut controls, base);
            let expected = modulated_control_value_from_delta(&spec, base, delta);
            apply_automation(
                &mut controls,
                &automation,
                TimingContext::new(8_000.0, 120.0, 0.0),
            );
            assert!(((spec.get)(&controls) - expected).abs() < 0.01);
        }
    }

    #[test]
    fn eight_bar_history_uses_bounded_samples_and_keeps_the_finished_phrase_during_grace() {
        let mut snapshot = LiveSessionSnapshot::from_controls(FluidControls::default());
        let target = recipe::RecipeTarget::capture("pad.level", &snapshot).unwrap();
        let mut history = CaptureHistory::default();
        for (beat, value) in [(32.0, 0.4), (48.0, 0.6), (72.0, 0.8)] {
            change(&mut history, &mut snapshot, beat, value);
        }
        let clip = history
            .motion_clip(target, MotionDuration::Beats32, 64.0, 72.0)
            .unwrap();
        assert_eq!(clip.duration.samples(), 128);
        assert_eq!(clip.samples()[0], 102);
        assert_eq!(clip.samples()[64], 153);
        assert_eq!(clip.position(95.999), Some(0.6));
        assert_eq!(clip.position(96.0), Some(0.4));
        assert!((clip.position_from_base(0.8, 64.0).unwrap() - 0.4).abs() < 0.0001);
        let mut upper = clip.clone();
        upper.set_reference(0.0);
        assert_eq!(upper.position_from_base(0.9, 64.0), Some(1.0));
        upper.set_reference(1.0);
        assert_eq!(upper.position_from_base(0.1, 64.0), Some(0.0));
    }

    #[test]
    fn editor_relative_motion_uses_authored_baselines_on_every_sample_and_rebuild() {
        let address = ControlAddress::new("pad.level");
        let mut automation = AutomationState::default();
        let route = automation.open_or_create(address);
        route.shape = LfoShape::Steps;
        route.step_count = 1;
        route.steps[0] = 0.4;
        route.depth_ratio = 0.4;
        let amount = EditorMotionAddress {
            control: address,
            lane_index: 0,
            target: EditorMotionField::Lfo(LfoField::Amount),
        };
        let step = EditorMotionAddress {
            target: EditorMotionField::Step(StepTarget::Value(0)),
            ..amount
        };
        for target in [amount, step] {
            automation
                .editor_captures
                .insert(target, relative_clip(MotionDuration::Beats32, 0.4, 153));
        }
        let mut plan = AutomationPlan::default();
        plan.rebuild(&automation);
        let timing = TimingContext::new(8_000.0, 120.0, 8.0);
        for _ in 0..10_000 {
            let mut controls = FluidControls::default();
            controls.pad.level = 0.0;
            plan.apply(&mut controls, timing);
            assert!((controls.pad.level - 0.36).abs() < 0.0001);
        }
        automation.route_mut(address).unwrap().depth_ratio = 0.6;
        plan.rebuild(&automation);
        for _ in 0..2_000 {
            let mut controls = FluidControls::default();
            controls.pad.level = 0.0;
            plan.apply(&mut controls, timing);
        }
        let lanes = automation.effective_lanes(address, timing.beat);
        assert!((lanes.lfos()[0].depth_ratio - 0.8).abs() < 0.0001);
        assert!((lanes.lfos()[0].steps[0] - 0.6).abs() < 0.0001);
        let mut controls = FluidControls::default();
        controls.pad.level = 0.0;
        plan.apply(&mut controls, timing);
        assert!((controls.pad.level - 0.48).abs() < 0.0001);
        for queued in [false, true] {
            for clip in automation.editor_captures.values_mut() {
                clip.enabled = queued;
                clip.launch = 12.0;
            }
            let lanes = automation.effective_lanes(address, 8.0);
            assert_eq!(lanes.lfos()[0].depth_ratio, 0.6);
            assert_eq!(lanes.lfos()[0].steps[0], 0.4);
            plan.rebuild(&automation);
            for _ in 0..2_000 {
                controls.pad.level = 0.0;
                plan.apply(&mut controls, timing);
            }
            assert!((controls.pad.level - 0.24).abs() < 0.0001);
        }
    }

    #[test]
    fn progression_selection_does_not_record_chord_edits_or_mix_history_banks() {
        let id = "pad.chord1_degree";
        let spec = spec_by_id(id).unwrap();
        let mut snapshot = LiveSessionSnapshot::from_controls(FluidControls::default());
        snapshot.controls.pad.progression = 0.0;
        let a = recipe::RecipeTarget::capture(id, &snapshot).unwrap();
        let mut history = CaptureHistory::default();
        let before = snapshot.clone();
        spec.apply_delta(1.0, &mut snapshot.controls);
        history.record_changes(&before, &snapshot, 1.0);
        let before = snapshot.clone();
        snapshot.automation.captures.insert(
            ControlAddress::new(id),
            CaptureClip {
                duration: MotionDuration::Beats4,
                data: MotionData::Absolute {
                    events: vec![MotionEvent {
                        tick: 0,
                        position: 0,
                    }],
                },
                origin: 0.0,
                launch: 0.0,
                enabled: true,
            },
        );
        snapshot.controls.pad.progression = 1.0;
        suspend_changed_captures(&before, &mut snapshot);
        assert!(snapshot.automation.captures[&ControlAddress::new(id)].enabled);
        history.record_changes(&before, &snapshot, 2.0);
        let b = recipe::RecipeTarget::capture(id, &snapshot).unwrap();
        assert!(!a.is_current(&snapshot));
        assert_eq!(
            history.motion_clip(b, MotionDuration::Beats4, 4.0, 4.0),
            Err(CaptureHistoryError::NoMovement)
        );
        let before = snapshot.clone();
        spec.apply_value(2.0, &mut snapshot.controls);
        history.record_changes(&before, &snapshot, 3.0);
        let clip = history
            .motion_clip(b, MotionDuration::Beats4, 4.0, 4.0)
            .unwrap();
        assert!(!clip.is_relative());
        assert_eq!(clip.events().len(), 2);
        assert_eq!(clip.events()[1].tick, 3 * 256);
        assert!(
            history
                .motion_clip(a, MotionDuration::Beats4, 4.0, 4.0)
                .is_ok()
        );
    }

    #[test]
    fn motion_codes_measure_realistic_and_maximum_bounded_payloads() {
        let builtins = decode_auto_states();
        assert!(
            builtins
                .iter()
                .all(|song| song.automation.captures.is_empty()
                    && song.automation.editor_captures.is_empty())
        );
        let baseline = builtins
            .into_iter()
            .max_by_key(|song| encode_song_code(song).unwrap().len())
            .unwrap();
        let make = |count: usize, discrete: bool| {
            let mut song = baseline.clone();
            let mut used = std::collections::BTreeSet::new();
            let snapshot = LiveSessionSnapshot::from_song(&song);
            let addresses: Vec<_> = all_specs()
                .filter(|spec| {
                    (spec.contextual(&song.controls).kind == ControlKind::Discrete) == discrete
                })
                .filter(|spec| recipe::RecipeTarget::capture(spec.id, &snapshot).is_some())
                .filter(|spec| used.insert(spec.id))
                .take(count)
                .map(|spec| ControlAddress::new(spec.id))
                .collect();
            assert_eq!(addresses.len(), count);
            for (index, address) in addresses.into_iter().enumerate() {
                let mut clip = relative_clip(MotionDuration::Beats32, 0.5, 128);
                clip.origin = -(index as f64) - 0.25;
                clip.launch = index as f64 % 4.0;
                clip.enabled = index % 3 != 0;
                clip.data = if discrete {
                    MotionData::Absolute {
                        events: (0..MAX_MOTION_EVENTS)
                            .map(|event| MotionEvent {
                                tick: event as u16 * 64,
                                position: (event * 521) as u16,
                            })
                            .collect(),
                    }
                } else {
                    MotionData::Relative {
                        reference: 32768,
                        samples: std::array::from_fn(|sample| (sample * 37) as u8),
                    }
                };
                song.automation.captures.insert(address, clip);
            }
            let code = encode_song_code(&song).unwrap();
            let decoded = decode_song_code(&code).unwrap();
            assert_eq!(decoded.automation.captures, song.automation.captures);
            code.len()
        };
        let realistic = make(4, false);
        let maximum_sampled = make(MAX_CAPTURES, false);
        let maximum_events = make(MAX_CAPTURES, true);
        eprintln!(
            "Motion code characters: four eight-bar curves={realistic}; sixteen curves={maximum_sampled}; sixteen full event clips={maximum_events}"
        );
        assert!(realistic < 2_000);
        assert!(maximum_sampled < 4_500);
        assert!(maximum_events < 12_700);
    }

    #[test]
    fn editing_the_base_raises_a_running_relative_loop_in_deterministic_audio() {
        let render = |shift_base| {
            let mut song = SongState::default();
            song.controls.master.bpm = 120.0;
            song.controls.pad.level = 0.4;
            song.controls.modules.pad = Default::default();
            song.controls.modules.master = Default::default();
            song.muted.fill(true);
            song.muted[Tab::Chords as usize] = false;
            song.muted[Tab::Master as usize] = false;
            let address = ControlAddress::new("pad.level");
            let mut clip = relative_clip(MotionDuration::Beats32, 0.4, 102);
            if let MotionData::Relative { samples, .. } = &mut clip.data {
                *samples = std::array::from_fn(|index| {
                    (102.0
                        + 26.0
                            * (std::f32::consts::TAU * index as f32 / CAPTURE_SAMPLES as f32).sin())
                    .round() as u8
                });
            }
            song.automation.captures.insert(address, clip.clone());
            let session = LiveSession::new(LiveSessionSnapshot::from_song(&song));
            let mut engine = FluidEngine::new(
                8_000.0,
                session.clone(),
                no_morph(),
                Arc::new(FluidTelemetry::default()),
            );
            engine.reseed(551003);
            let mut energy = [0.0f64; 2];
            for sample in 0..72_000 {
                if sample == 40_000 && shift_base {
                    session.update(|snapshot| snapshot.controls.pad.level = 0.7);
                }
                let (left, right) = engine.next_stereo();
                let window = if (16_000..32_000).contains(&sample) {
                    Some(0)
                } else if (52_000..68_000).contains(&sample) {
                    Some(1)
                } else {
                    None
                };
                if let Some(index) = window {
                    energy[index] += f64::from(left * left + right * right);
                }
            }
            assert_eq!(session.load().automation.captures[&address], clip);
            energy
        };
        let steady = render(false);
        let shifted = render(true);
        assert_eq!(steady[0], shifted[0]);
        assert_eq!(shifted, render(true));
        assert!(
            shifted[1] > steady[1] * 1.8,
            "{shifted:?} versus {steady:?}"
        );
    }

    #[test]
    fn editor_history_discards_replaced_modules_and_deleted_lane_owners() {
        for replace_module in [false, true] {
            let control = ControlAddress::new("bass.slot1.time");
            let target = EditorMotionAddress {
                control,
                lane_index: 0,
                target: EditorMotionField::Lfo(LfoField::Amount),
            };
            let mut snapshot = LiveSessionSnapshot::from_controls(FluidControls::default());
            snapshot.automation.open_or_create(control);
            let mut history = CaptureHistory::default();
            let before = snapshot.clone();
            snapshot.automation.route_mut(control).unwrap().depth_ratio = 0.4;
            history.record_changes(&before, &snapshot, 1.0);
            assert!(
                history
                    .editor_motion_clip(target, MotionDuration::Beats4, 4.0)
                    .is_ok()
            );
            let before = snapshot.clone();
            if replace_module {
                snapshot.controls.modules.bass[0] = preset_slot("delay", 0.0);
            } else {
                snapshot.automation.clear_control(control);
            }
            history.record_changes(&before, &snapshot, 2.0);
            if !replace_module {
                let before = snapshot.clone();
                snapshot.automation.open_or_create(control);
                history.record_changes(&before, &snapshot, 2.5);
            }
            assert_eq!(
                history.editor_motion_clip(target, MotionDuration::Beats4, 4.0),
                Err(CaptureHistoryError::NoMovement)
            );
            let before = snapshot.clone();
            snapshot.automation.route_mut(control).unwrap().depth_ratio = 0.6;
            snapshot.automation.route_mut(control).unwrap().shape = LfoShape::Triangle;
            history.record_changes(&before, &snapshot, 3.0);
            assert!(
                history
                    .editor_motion_clip(target, MotionDuration::Beats4, 4.0)
                    .is_ok()
            );
            let shape = EditorMotionAddress {
                target: EditorMotionField::Lfo(LfoField::Shape),
                ..target
            };
            assert!(
                history
                    .editor_motion_clip(shape, MotionDuration::Beats4, 4.0)
                    .is_ok()
            );
        }
    }
}
