//! Palette actions on a frozen, revision-checked LFO or envelope lane.

use super::*;
use crate::fluid::{CaptureAction, LiveSessionSnapshot, recipe::RecipeTarget};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LaneAction {
    Bypass,
    Resume,
    Delete,
}

impl LaneAction {
    pub(crate) fn from_capture(action: CaptureAction) -> Option<Self> {
        match action {
            CaptureAction::Capture => None,
            CaptureAction::Bypass => Some(Self::Bypass),
            CaptureAction::Resume => Some(Self::Resume),
            CaptureAction::Delete => Some(Self::Delete),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LaneTarget {
    pub(crate) control: RecipeTarget,
    pub(crate) kind: ModKind,
    pub(crate) index: usize,
    revision: u64,
}

impl LaneTarget {
    pub(crate) fn capture(snapshot: &LiveSessionSnapshot) -> Option<Self> {
        let open = snapshot.automation.open?;
        Some(Self {
            control: RecipeTarget::capture(open.address.id(), snapshot)?,
            kind: open.kind,
            index: open.index,
            revision: snapshot.automation_revision,
        })
    }

    pub(crate) fn is_current(self, snapshot: &LiveSessionSnapshot) -> bool {
        self.revision == snapshot.automation_revision
            && self.control.is_current(snapshot)
            && Self::capture(snapshot) == Some(self)
            && snapshot
                .automation
                .lane_enabled(ControlAddress::new(self.control.id), self.kind, self.index)
                .is_some()
    }
}

impl AutomationState {
    pub(crate) fn bypass_masks(&self) -> impl Iterator<Item = (ControlAddress, u8, u8)> + '_ {
        self.stacks.iter().filter_map(|(address, stack)| {
            let lfos = stack
                .lfos
                .iter()
                .enumerate()
                .fold(0, |bits, (index, route)| {
                    bits | (u8::from(!route.enabled) << index)
                });
            let envelopes = stack
                .envelopes
                .iter()
                .enumerate()
                .fold(0, |bits, (index, route)| {
                    bits | (u8::from(!route.enabled) << index)
                });
            (lfos != 0 || envelopes != 0).then_some((*address, lfos, envelopes))
        })
    }

    pub(crate) fn lane_enabled(
        &self,
        address: ControlAddress,
        kind: ModKind,
        index: usize,
    ) -> Option<bool> {
        let stack = self.stacks.get(&address)?;
        match kind {
            ModKind::Lfo => stack.lfos.get(index).map(|route| route.enabled),
            ModKind::Envelope => stack.envelopes.get(index).map(|route| route.enabled),
        }
    }

    pub(crate) fn set_lane_enabled(
        &mut self,
        address: ControlAddress,
        kind: ModKind,
        index: usize,
        enabled: bool,
    ) -> bool {
        let Some(stack) = self.stacks.get_mut(&address) else {
            return false;
        };
        let flag = match kind {
            ModKind::Lfo => stack.lfos.get_mut(index).map(|route| &mut route.enabled),
            ModKind::Envelope => stack
                .envelopes
                .get_mut(index)
                .map(|route| &mut route.enabled),
        };
        let Some(flag) = flag else {
            return false;
        };
        *flag = enabled;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fluid::{LiveSession, TimingContext};
    use rand::SeedableRng;

    #[test]
    fn lane_targets_reject_deleted_and_recreated_lanes_but_ignore_unrelated_edits() {
        let address = ControlAddress::new("master.level");
        let session =
            LiveSession::new(LiveSessionSnapshot::from_controls(FluidControls::default()));
        session.update(|snapshot| {
            snapshot.automation.open_or_create(address);
        });
        let target = LaneTarget::capture(&session.load()).unwrap();
        session.update(|snapshot| snapshot.controls.master.bpm = 91.0);
        assert!(target.is_current(&session.load()));
        session.update(|snapshot| {
            snapshot.automation.open_or_create(address);
        });
        assert!(
            target.is_current(&session.load()),
            "opening an unchanged lane is not a revision"
        );
        session.update(|snapshot| snapshot.automation.remove_open_route());
        session.update(|snapshot| {
            snapshot.automation.open_or_create(address);
        });
        assert!(!target.is_current(&session.load()));
    }

    #[test]
    fn bypass_keeps_lane_capacity_settings_and_structural_morph_choice() {
        let address = ControlAddress::new("master.level");
        for kind in [ModKind::Lfo, ModKind::Envelope] {
            let mut automation = AutomationState::default();
            for index in 0..MAX_AUTOMATION_LANES_PER_KIND {
                assert!(automation.add_and_open(address, kind));
                assert!(automation.set_lane_enabled(address, kind, index, false));
            }
            assert!(!automation.add_and_open(address, kind));
            let mut rng = rand::rngs::StdRng::seed_from_u64(9);
            match kind {
                ModKind::Lfo => {
                    let route = automation.route_mut(address).unwrap();
                    route.randomize(&mut rng, 0.0);
                    route.reset_field_at(LfoField::Amount, 0.0);
                    route.reset_step(StepTarget::Value(0));
                }
                ModKind::Envelope => {
                    let route = automation.envelope_mut(address).unwrap();
                    route.randomize(&mut rng);
                    route.reset_field(EnvField::Amount);
                }
            }
            assert_eq!(automation.lane_enabled(address, kind, 3), Some(false));
        }
        let from = LfoRoute {
            enabled: false,
            depth_ratio: 0.3,
            ..LfoRoute::default()
        };
        let to = LfoRoute {
            enabled: true,
            ..from
        };
        assert!(
            !LfoRoute::morph(Some(&from), Some(&to), 0.5, false)
                .unwrap()
                .enabled
        );
        assert!(
            LfoRoute::morph(Some(&from), Some(&to), 0.5, true)
                .unwrap()
                .enabled
        );
        let from = EnvelopeRoute {
            enabled: false,
            amount: -0.4,
            ..EnvelopeRoute::default()
        };
        let to = EnvelopeRoute {
            enabled: true,
            ..from
        };
        assert!(
            !EnvelopeRoute::morph(Some(&from), Some(&to), 0.5, false)
                .unwrap()
                .enabled
        );
        assert!(
            EnvelopeRoute::morph(Some(&from), Some(&to), 0.5, true)
                .unwrap()
                .enabled
        );
    }

    #[test]
    fn bypass_and_resume_declick_audio_plan_and_rejoin_the_current_phase() {
        let address = ControlAddress::new("master.level");
        for kind in [ModKind::Lfo, ModKind::Envelope] {
            let mut automation = AutomationState::default();
            automation.add_route(
                address,
                LfoRoute {
                    depth_ratio: 0.2,
                    cycle_beats: 4.0,
                    ..LfoRoute::default()
                },
            );
            automation.add_envelope(
                address,
                EnvelopeRoute {
                    amount: 0.15,
                    attack_beats: 0.0,
                    decay_beats: 4.0,
                    trigger: EnvTrigger::EveryBeats(4.0),
                    ..EnvelopeRoute::default()
                },
            );
            let mut plan = AutomationPlan::default();
            plan.rebuild(&automation);
            let render = |plan: &mut AutomationPlan, beat: f64| {
                let mut controls = FluidControls::default();
                controls.master.level = 0.4;
                plan.apply(&mut controls, TimingContext::new(48_000.0, 120.0, beat));
                controls.master.level
            };
            let initial = render(&mut plan, 1.0);
            automation.set_lane_enabled(address, kind, 0, false);
            plan.rebuild(&automation);
            let first = render(&mut plan, 1.0);
            assert!(
                (initial - first).abs() < 0.002,
                "bypass jumped: {initial} -> {first}"
            );
            for _ in 0..3000 {
                render(&mut plan, 1.0);
            }
            let bypassed = render(&mut plan, 1.0);
            assert!(
                (initial - bypassed).abs() > 0.08,
                "bypass must change the audible gain"
            );
            for _ in 0..3000 {
                render(&mut plan, 3.0);
            }
            let before_resume = render(&mut plan, 3.0);
            automation.set_lane_enabled(address, kind, 0, true);
            plan.rebuild(&automation);
            let first = render(&mut plan, 3.0);
            assert!((before_resume - first).abs() < 0.002, "resume jumped");
            for _ in 0..3000 {
                render(&mut plan, 3.0);
            }
            let resumed = render(&mut plan, 3.0);
            let expected = modulated_control_value_full(
                address.spec(),
                automation.lfo_lanes(address),
                automation.envelope_lanes(address),
                0.4,
                ModContext::lfo_only(3.0),
            );
            assert!(
                (resumed - expected).abs() < 0.0001,
                "audio {resumed} vs UI {expected}"
            );
            assert!(
                (resumed - initial).abs() > 0.1,
                "resume must use beat3, not retrigger beat1"
            );
        }
    }
}
