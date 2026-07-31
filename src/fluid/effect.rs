#[cfg(test)]
use super::interaction::PaletteStagedEdit;
use super::interaction::{InteractionEffect, LfoDepth, Page};
use super::*;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum EffectFailure {
    UnknownControl(&'static str),
    Clipboard(String),
    MissingContext(&'static str),
    UnsupportedInteraction(InteractionEffect),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum EffectAcknowledgement {
    Published {
        generation: u64,
    },
    Staged {
        count: usize,
    },
    Message(String),
    ControlSelected {
        tab: Tab,
        index: usize,
        id: &'static str,
    },
    PerformanceEdited {
        tab: Tab,
        index: usize,
        id: &'static str,
        generation: u64,
    },
    AutomationPosition {
        depth: LfoDepth,
        selected: usize,
    },
    PageSelected(Page),
    QuitRequested,
    NoChange,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ControlEdit {
    Delta(f32),
    Value(f32),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum LiveEffect {
    EditControl {
        id: &'static str,
        edit: ControlEdit,
    },
    SelectControl {
        tab: Tab,
        index: usize,
        id: &'static str,
    },
    StageForBar {
        target_beat: f64,
        edits: Vec<StagedEdit>,
    },
    CommitPending {
        beat: f64,
    },
    CopySong,
    ShowMessage(String),
}

#[derive(Default)]
pub(crate) struct InteractionExecutionContext {
    pub(crate) selected_control: Option<&'static str>,
    pub(crate) beat: f64,
}

pub(crate) struct ProductionInteractionContext<'a> {
    pub(crate) selected_control: Option<&'static str>,
    pub(crate) tab: Tab,
    pub(crate) selected: usize,
    pub(crate) automation_selected: usize,
    pub(crate) beat: f64,
    pub(crate) flipped: &'a mut FlippedUnits,
    pub(crate) mute: &'a mut MuteState,
}

pub(crate) trait Clipboard {
    fn set_text(&mut self, text: String) -> Result<(), String>;
}

pub(crate) struct SystemClipboard;

impl Clipboard for SystemClipboard {
    fn set_text(&mut self, text: String) -> Result<(), String> {
        let mut clipboard = arboard::Clipboard::new().map_err(|error| error.to_string())?;
        clipboard.set_text(text).map_err(|error| error.to_string())
    }
}

/// Whether a session edit claims the controls away from auto mode.
///
/// Every `edit_session` caller states this explicitly. Auto mode is driving
/// the controls, so a deliberate edit has to stand it down or the morph would
/// immediately overwrite the user; but a gesture that writes a control without
/// claiming authorship must not, or muting would silently end the morph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AutoOwnership {
    /// The user is steering now. Auto mode stands down and whatever the morph
    /// has already written stays as the new starting point.
    TakeOver,
    /// A reversible gesture layered over whatever is playing (mute/unmute).
    /// Auto mode keeps driving.
    Preserve,
}

/// Stateful boundary for executing ordered interaction effects. All
/// user-audible edits delegate to one `LiveSession` transaction; UI-only
/// consequences are acknowledged only after publication succeeds.
pub(crate) struct EffectExecutor {
    session: LiveSession,
    auto: AutoControls,
    recent: RecentControls,
    pending: Option<(f64, Vec<StagedEdit>)>,
    message: Option<(String, Instant)>,
}

impl EffectExecutor {
    pub(crate) fn new(session: LiveSession, auto: AutoControls) -> Self {
        Self {
            session,
            auto,
            recent: RecentControls::default(),
            pending: None,
            message: None,
        }
    }

    pub(crate) fn session(&self) -> &LiveSession {
        &self.session
    }

    pub(crate) fn auto_morph_ids(&self, beat: f64) -> Option<(Option<usize>, Option<usize>)> {
        self.auto.morph_ids_at(beat)
    }

    pub(crate) fn toggle_auto(&mut self, beat: f64) {
        if self.auto.is_running() {
            self.auto.exit();
            self.session.update(|_| {});
        } else {
            let current = self.session.load();
            self.auto
                .toggle(current.controls.clone(), current.automation.clone(), beat);
            self.session.update(|_| {});
        }
    }

    pub(crate) fn recent(&self) -> &RecentControls {
        &self.recent
    }

    pub(crate) fn pending(&self) -> Option<&(f64, Vec<StagedEdit>)> {
        self.pending.as_ref()
    }

    pub(crate) fn message(&self) -> Option<&str> {
        self.message.as_ref().map(|(message, _)| message.as_str())
    }

    pub(crate) fn expire_message(&mut self, ttl: std::time::Duration) {
        if self
            .message
            .as_ref()
            .is_some_and(|(_, shown_at)| shown_at.elapsed() >= ttl)
        {
            self.message = None;
        }
    }

    pub(crate) fn execute(
        &mut self,
        effect: LiveEffect,
    ) -> Result<EffectAcknowledgement, EffectFailure> {
        let mut clipboard = SystemClipboard;
        self.execute_with_clipboard(effect, &mut clipboard)
    }

    pub(crate) fn execute_with_clipboard(
        &mut self,
        effect: LiveEffect,
        clipboard: &mut dyn Clipboard,
    ) -> Result<EffectAcknowledgement, EffectFailure> {
        match effect {
            LiveEffect::EditControl { id, edit } => {
                let spec = spec_by_id(id).ok_or(EffectFailure::UnknownControl(id))?;
                let snapshot =
                    self.edit_session(AutoOwnership::TakeOver, Some(id), |snapshot| match edit {
                        ControlEdit::Delta(delta) => {
                            spec.apply_delta(delta, &mut snapshot.controls)
                        }
                        ControlEdit::Value(value) => {
                            spec.apply_value(value, &mut snapshot.controls)
                        }
                    });
                Ok(EffectAcknowledgement::Published {
                    generation: snapshot.generation,
                })
            }
            LiveEffect::SelectControl { tab, index, id } => {
                if spec_by_id(id).is_none() {
                    return Err(EffectFailure::UnknownControl(id));
                }
                self.recent.touch(id);
                Ok(EffectAcknowledgement::ControlSelected { tab, index, id })
            }
            LiveEffect::StageForBar { target_beat, edits } => {
                if let Some(unknown) = edits.iter().find(|edit| spec_by_id(edit.id).is_none()) {
                    return Err(EffectFailure::UnknownControl(unknown.id));
                }
                let count = edits.len();
                self.pending = Some((target_beat, edits));
                Ok(EffectAcknowledgement::Staged { count })
            }
            LiveEffect::CommitPending { beat } => {
                let Some((target, edits)) = self.pending.as_ref() else {
                    return Ok(EffectAcknowledgement::NoChange);
                };
                if beat < *target {
                    return Ok(EffectAcknowledgement::NoChange);
                }
                let edits = edits.clone();
                if let Some(unknown) = edits.iter().find(|edit| spec_by_id(edit.id).is_none()) {
                    return Err(EffectFailure::UnknownControl(unknown.id));
                }
                self.auto.exit();
                let snapshot = self.session.update(|snapshot| {
                    for edit in &edits {
                        spec_by_id(edit.id)
                            .expect("validated staged control")
                            .apply_value(edit.value, &mut snapshot.controls);
                    }
                });
                self.pending = None;
                self.recent.touch_edits(&edits);
                let plural = if edits.len() == 1 { "" } else { "s" };
                let message = format!("{} edit{plural} applied", edits.len());
                self.message = Some((message, Instant::now()));
                Ok(EffectAcknowledgement::Published {
                    generation: snapshot.generation,
                })
            }
            LiveEffect::CopySong => {
                let snapshot = self.session.load();
                let code = encode_song_code(&SongState {
                    controls: snapshot.controls.clone(),
                    automation: snapshot.automation.clone(),
                    tonal_sequence: Some(snapshot.tonal_sequence.clone()),
                })
                .map_err(|error| EffectFailure::Clipboard(error.to_string()))?;
                clipboard.set_text(code).map_err(EffectFailure::Clipboard)?;
                let message = "song code copied to clipboard".to_string();
                self.message = Some((message.clone(), Instant::now()));
                Ok(EffectAcknowledgement::Message(message))
            }
            LiveEffect::ShowMessage(message) => {
                self.message = Some((message.clone(), Instant::now()));
                Ok(EffectAcknowledgement::Message(message))
            }
        }
    }

    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "ordered generic bridge is exercised by adapter tests"
        )
    )]
    pub(crate) fn execute_ordered(
        &mut self,
        effects: impl IntoIterator<Item = LiveEffect>,
    ) -> Vec<Result<EffectAcknowledgement, EffectFailure>> {
        let mut results = Vec::new();
        for effect in effects {
            let result = self.execute(effect);
            let failed = result.is_err();
            results.push(result);
            if failed {
                break;
            }
        }
        results
    }

    /// Apply an edit to the live session.
    ///
    /// `ownership` is required and has no default on purpose. Standing auto
    /// mode down is a user-visible consequence, and it used to be an implicit
    /// side effect of calling this helper, so gestures that merely write a
    /// control — mute — silently killed the morph and stranded whatever
    /// automation it had just installed. Making every caller name its intent
    /// is what stops that from being reintroduced by accident: a new effect
    /// cannot compile without deciding.
    pub(crate) fn edit_session(
        &mut self,
        ownership: AutoOwnership,
        recent_id: Option<&'static str>,
        mut edit: impl FnMut(&mut LiveSessionSnapshot),
    ) -> Arc<LiveSessionSnapshot> {
        if ownership == AutoOwnership::TakeOver {
            self.auto.exit();
        }
        let snapshot = self.session.update(|snapshot| edit(snapshot));
        if let Some(id) = recent_id {
            self.recent.touch(id);
        }
        snapshot
    }

    pub(crate) fn edit_navigation_automation(
        &mut self,
        mut edit: impl FnMut(&mut AutomationState),
    ) {
        self.session
            .update(|snapshot| edit(&mut snapshot.automation));
    }

    /// Mute/unmute a tab's level. This writes a control, but it is a
    /// performance gesture rather than authorship: the stored level comes
    /// straight back on the second press, so auto mode keeps driving.
    pub(crate) fn toggle_mute(&mut self, tab: Tab, mute: &mut MuteState) {
        let Some(id) = tab.level_id() else { return };
        let spec = spec_by_id(id).expect("tab level_id must name a real control");
        let slot = &mut mute[tab as usize];
        match *slot {
            Some(previous) => {
                self.edit_session(AutoOwnership::Preserve, Some(id), |snapshot| {
                    (spec.set)(&mut snapshot.controls, previous);
                });
                *slot = None;
            }
            None => {
                let previous = std::cell::Cell::new(0.0);
                self.edit_session(AutoOwnership::Preserve, Some(id), |snapshot| {
                    previous.set((spec.get)(&snapshot.controls));
                    (spec.set)(&mut snapshot.controls, 0.0);
                });
                *slot = Some(previous.get());
            }
        }
    }

    /// Typed bridge from the pure interaction kernel to effect execution.
    /// Effects needing adapter-owned data must receive it explicitly through
    /// `context`; unsupported staged performance effects fail visibly.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "generic interaction bridge is exercised by adapter tests"
        )
    )]
    pub(crate) fn execute_interaction(
        &mut self,
        effect: InteractionEffect,
        context: &InteractionExecutionContext,
    ) -> Result<EffectAcknowledgement, EffectFailure> {
        let mut clipboard = SystemClipboard;
        self.execute_interaction_with_clipboard(effect, context, &mut clipboard)
    }

    pub(crate) fn execute_interaction_with_clipboard(
        &mut self,
        effect: InteractionEffect,
        context: &InteractionExecutionContext,
        clipboard: &mut dyn Clipboard,
    ) -> Result<EffectAcknowledgement, EffectFailure> {
        match effect {
            InteractionEffect::AdjustSelected(delta) => {
                let id = context
                    .selected_control
                    .ok_or(EffectFailure::MissingContext("selected control"))?;
                self.execute(LiveEffect::EditControl {
                    id,
                    edit: ControlEdit::Delta(f32::from(delta)),
                })
            }
            InteractionEffect::CommitNumeric(value) => {
                let id = context
                    .selected_control
                    .ok_or(EffectFailure::MissingContext("selected control"))?;
                self.execute(LiveEffect::EditControl {
                    id,
                    edit: ControlEdit::Value(value),
                })
            }
            InteractionEffect::PaletteJump { tab, index, id } => {
                self.execute(LiveEffect::SelectControl { tab, index, id })
            }
            InteractionEffect::PaletteCommit(edits) => {
                let edits = edits
                    .into_iter()
                    .map(|edit| StagedEdit {
                        id: edit.id,
                        value: f32::from_bits(edit.value_bits),
                    })
                    .collect::<Vec<_>>();
                if edits.is_empty() {
                    return Ok(EffectAcknowledgement::NoChange);
                }
                self.execute(LiveEffect::StageForBar {
                    target_beat: context.beat,
                    edits,
                })?;
                self.execute(LiveEffect::CommitPending { beat: context.beat })
            }
            InteractionEffect::SelectPage(page) => Ok(EffectAcknowledgement::PageSelected(page)),
            InteractionEffect::Save => self.execute_with_clipboard(LiveEffect::CopySong, clipboard),
            InteractionEffect::Quit => Ok(EffectAcknowledgement::QuitRequested),
            unsupported @ (InteractionEffect::AutomationConfirm(_)
            | InteractionEffect::ResetSelected
            | InteractionEffect::MaxSelected
            | InteractionEffect::ToggleAuto
            | InteractionEffect::ToggleUnits
            | InteractionEffect::ToggleMute { .. }
            | InteractionEffect::ToggleMacro
            | InteractionEffect::RemoveAutomation
            | InteractionEffect::ReseedAutomation
            | InteractionEffect::CloseAutomationDepth
            | InteractionEffect::CloseAutomationAll
            | InteractionEffect::TouchSelected
            | InteractionEffect::PaletteCommitAtBar(_)
            | InteractionEffect::PerformanceInstrument(_)
            | InteractionEffect::HoldPerformanceSelector(_)
            | InteractionEffect::ReleaseHeldSelector(_)
            | InteractionEffect::PerformanceEdit { .. }) => {
                Err(EffectFailure::UnsupportedInteraction(unsupported))
            }
        }
    }

    /// Execute one kernel transition's ordered effects and stop at the first
    /// failure. Later effects are never attempted.
    #[expect(
        dead_code,
        reason = "ordered generic interaction bridge is exercised by adapter tests"
    )]
    pub(crate) fn execute_interactions_ordered(
        &mut self,
        effects: impl IntoIterator<Item = InteractionEffect>,
        context: &InteractionExecutionContext,
    ) -> Vec<Result<EffectAcknowledgement, EffectFailure>> {
        let mut clipboard = SystemClipboard;
        self.execute_interactions_ordered_with_clipboard(effects, context, &mut clipboard)
    }

    pub(crate) fn execute_interactions_ordered_with_clipboard(
        &mut self,
        effects: impl IntoIterator<Item = InteractionEffect>,
        context: &InteractionExecutionContext,
        clipboard: &mut dyn Clipboard,
    ) -> Vec<Result<EffectAcknowledgement, EffectFailure>> {
        let mut results = Vec::new();
        for effect in effects {
            let result = self.execute_interaction_with_clipboard(effect, context, clipboard);
            let failed = result.is_err();
            results.push(result);
            if failed {
                break;
            }
        }
        results
    }

    pub(crate) fn execute_production_interactions_with_clipboard(
        &mut self,
        effects: impl IntoIterator<Item = InteractionEffect>,
        context: &mut ProductionInteractionContext<'_>,
        clipboard: &mut dyn Clipboard,
    ) -> Vec<Result<EffectAcknowledgement, EffectFailure>> {
        let mut results = Vec::new();
        for effect in effects {
            let result = self.execute_production_interaction(effect, context, clipboard);
            let failed = result.is_err();
            results.push(result);
            if failed {
                break;
            }
        }
        results
    }

    fn execute_production_interaction(
        &mut self,
        effect: InteractionEffect,
        context: &mut ProductionInteractionContext<'_>,
        clipboard: &mut dyn Clipboard,
    ) -> Result<EffectAcknowledgement, EffectFailure> {
        match effect {
            InteractionEffect::AdjustSelected(delta) => {
                let automation = self.session.load().automation.clone();
                adjust_lfo_or_control(
                    self,
                    &automation,
                    context.automation_selected,
                    context.tab,
                    context.selected,
                    f32::from(delta),
                    context.beat,
                    context.flipped,
                );
                Ok(EffectAcknowledgement::Published {
                    generation: self.session.load().generation,
                })
            }
            InteractionEffect::CommitNumeric(value) => {
                let automation = self.session.load().automation.clone();
                set_modulator_or_control(
                    self,
                    &automation,
                    context.automation_selected,
                    context.tab,
                    context.selected,
                    value,
                    context.beat,
                    context.flipped,
                );
                Ok(EffectAcknowledgement::Published {
                    generation: self.session.load().generation,
                })
            }
            InteractionEffect::AutomationConfirm(kind) => {
                let id = context
                    .selected_control
                    .ok_or(EffectFailure::MissingContext("selected control"))?;
                let kind = match kind {
                    super::interaction::AutomationKind::Lfo => ModKind::Lfo,
                    super::interaction::AutomationKind::Envelope => ModKind::Envelope,
                    super::interaction::AutomationKind::Macro => ModKind::Macro,
                };
                let mut selected = context.automation_selected;
                open_modulator_effect_for_id(self, id, kind, &mut selected);
                Ok(EffectAcknowledgement::Published {
                    generation: self.session.load().generation,
                })
            }
            InteractionEffect::ResetSelected => {
                let automation = self.session.load().automation.clone();
                reset_lfo_or_control(
                    self,
                    &automation,
                    context.automation_selected,
                    context.tab,
                    context.selected,
                    context.beat,
                );
                Ok(EffectAcknowledgement::Published {
                    generation: self.session.load().generation,
                })
            }
            InteractionEffect::MaxSelected => {
                let automation = self.session.load().automation.clone();
                max_lfo_or_control(
                    self,
                    &automation,
                    context.automation_selected,
                    context.tab,
                    context.selected,
                    context.beat,
                );
                Ok(EffectAcknowledgement::Published {
                    generation: self.session.load().generation,
                })
            }
            InteractionEffect::ToggleAuto => {
                self.toggle_auto(context.beat);
                Ok(EffectAcknowledgement::Published {
                    generation: self.session.load().generation,
                })
            }
            InteractionEffect::ToggleUnits => {
                let automation = self.session.load().automation.clone();
                toggle_units_effect(
                    self,
                    &automation,
                    context.flipped,
                    context.automation_selected,
                    context.tab,
                    context.selected,
                    context.beat,
                );
                Ok(EffectAcknowledgement::Published {
                    generation: self.session.load().generation,
                })
            }
            InteractionEffect::ToggleMute { master } => {
                self.toggle_mute(if master { Tab::Master } else { context.tab }, context.mute);
                Ok(EffectAcknowledgement::Published {
                    generation: self.session.load().generation,
                })
            }
            InteractionEffect::ToggleMacro => {
                let automation = self.session.load().automation.clone();
                let position = toggle_macro_effect(
                    self,
                    &automation,
                    context.selected_control,
                    context.automation_selected,
                );
                Ok(position.map_or_else(
                    || EffectAcknowledgement::Published {
                        generation: self.session.load().generation,
                    },
                    |(depth, selected)| EffectAcknowledgement::AutomationPosition {
                        depth,
                        selected,
                    },
                ))
            }
            InteractionEffect::RemoveAutomation => {
                let automation = self.session.load().automation.clone();
                remove_automation_effect(
                    self,
                    &automation,
                    context.selected_control,
                    context.automation_selected,
                );
                Ok(EffectAcknowledgement::Published {
                    generation: self.session.load().generation,
                })
            }
            InteractionEffect::ReseedAutomation => {
                let automation = self.session.load().automation.clone();
                reseed_automation_effect(self, &automation);
                Ok(EffectAcknowledgement::Published {
                    generation: self.session.load().generation,
                })
            }
            InteractionEffect::CloseAutomationDepth => {
                let automation = self.session.load().automation.clone();
                Ok(close_one_level_effect(self, &automation).map_or(
                    EffectAcknowledgement::NoChange,
                    |selected| EffectAcknowledgement::AutomationPosition {
                        depth: LfoDepth::Editor,
                        selected,
                    },
                ))
            }
            InteractionEffect::CloseAutomationAll => {
                self.edit_navigation_automation(AutomationState::close_editor);
                Ok(EffectAcknowledgement::NoChange)
            }
            InteractionEffect::TouchSelected => {
                let id = context
                    .selected_control
                    .ok_or(EffectFailure::MissingContext("selected control"))?;
                let tab = tab_owning_control(id).unwrap_or(context.tab);
                let index = tab_specs(tab)
                    .iter()
                    .position(|spec| spec.id == id)
                    .unwrap_or(context.selected);
                self.execute(LiveEffect::SelectControl { tab, index, id })
            }
            InteractionEffect::PaletteCommitAtBar(edits) => {
                let edits = edits
                    .into_iter()
                    .map(|edit| StagedEdit {
                        id: edit.id,
                        value: f32::from_bits(edit.value_bits),
                    })
                    .collect::<Vec<_>>();
                self.execute(LiveEffect::StageForBar {
                    target_beat: next_bar_beat(context.beat),
                    edits,
                })
            }
            InteractionEffect::PaletteJump { tab, index, id } => {
                self.edit_navigation_automation(AutomationState::close_editor);
                self.execute(LiveEffect::SelectControl { tab, index, id })
            }
            InteractionEffect::PerformanceInstrument(_)
            | InteractionEffect::HoldPerformanceSelector(_)
            | InteractionEffect::ReleaseHeldSelector(_) => Ok(EffectAcknowledgement::NoChange),
            InteractionEffect::PerformanceEdit {
                targets,
                focus,
                action,
            } => {
                let edits = targets
                    .iter()
                    .map(|instrument| {
                        performance_target(instrument, action)
                            .ok_or(EffectFailure::MissingContext("performance target"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let (tab, index, focus_spec, _) = performance_target(focus, action)
                    .ok_or(EffectFailure::MissingContext("performance focus"))?;
                let snapshot = self.edit_session(AutoOwnership::TakeOver, None, |snapshot| {
                    for (_, _, spec, direction) in &edits {
                        spec.apply_delta(*direction, &mut snapshot.controls);
                    }
                });
                for (_, _, spec, _) in &edits {
                    self.recent.touch(spec.id);
                }
                self.recent.touch(focus_spec.id);
                Ok(EffectAcknowledgement::PerformanceEdited {
                    tab,
                    index,
                    id: focus_spec.id,
                    generation: snapshot.generation,
                })
            }
            other => self.execute_interaction_with_clipboard(
                other,
                &InteractionExecutionContext {
                    selected_control: context.selected_control,
                    beat: context.beat,
                },
                clipboard,
            ),
        }
    }
}

pub(crate) type MuteState = [Option<f32>; 9];

#[derive(Default)]
pub(crate) struct RecentControls {
    ids: Vec<&'static str>,
}

impl RecentControls {
    const CAPACITY: usize = 10;

    pub(crate) fn ids(&self) -> &[&'static str] {
        &self.ids
    }

    pub(crate) fn touch(&mut self, id: &'static str) {
        self.ids.retain(|&known| known != id);
        self.ids.insert(0, id);
        self.ids.truncate(Self::CAPACITY);
    }

    pub(crate) fn touch_edits(&mut self, edits: &[StagedEdit]) {
        for edit in edits.iter().rev() {
            self.touch(edit.id);
        }
    }
}

#[cfg(test)]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::fluid::interaction::PerformanceInstrument;
    use crate::fluid::widget::DialScale;

    #[derive(Default)]
    struct FakeClipboard {
        value: Option<String>,
        failure: Option<String>,
    }

    impl Clipboard for FakeClipboard {
        fn set_text(&mut self, text: String) -> Result<(), String> {
            if let Some(error) = self.failure.take() {
                Err(error)
            } else {
                self.value = Some(text);
                Ok(())
            }
        }
    }

    fn executor() -> EffectExecutor {
        let controls = FluidControls::default();
        let session = LiveSession::new(LiveSessionSnapshot::from_controls(controls));
        EffectExecutor::new(
            session,
            AutoControls::new(no_morph(), decode_auto_states(), DEFAULT_AUTO_BARS),
        )
    }

    fn executor_with(controls: FluidControls) -> EffectExecutor {
        EffectExecutor::new(
            LiveSession::new(LiveSessionSnapshot::from_controls(controls)),
            AutoControls::new(no_morph(), decode_auto_states(), DEFAULT_AUTO_BARS),
        )
    }

    /// Assert a family's field landed exactly on its own scale's ceiling and
    /// inside its declared range — the range is read from the same
    /// `DialScale` the bar draws, so a retuned range can never leave the
    /// gesture pointing at a stale constant.
    fn assert_at_ceiling(label: &str, value: f32, scale: DialScale) {
        let max = scale.max_value();
        assert!(
            (value - max).abs() < f32::EPSILON,
            "{label} did not reach its ceiling: {value} vs {max}"
        );
        assert!(
            scale.ratio(value) >= 1.0 - f32::EPSILON,
            "{label} landed outside its own throw at {value}"
        );
    }

    /// The ceiling gesture must reach every modulator field family, not fall
    /// through to the control underneath — that fall-through is the whole bug.
    /// One case per `ActiveField` arm, driven through the real
    /// `max_lfo_or_control` dispatch rather than the route methods directly.
    #[test]
    fn ceiling_gesture_tops_out_every_modulator_field_family() {
        const ID: &str = "pad.level";
        let address = ControlAddress::new(ID);
        let mut selected = 0;

        // LFO fields. Rows are `LfoField::ALL` in order while no field macro
        // is expanded, and Shape is last, so maxing it into `Steps` cannot
        // shift the rows under this loop.
        let mut effects = executor();
        open_modulator_effect_for_id(&mut effects, ID, ModKind::Lfo, &mut selected);
        for (row, field) in LfoField::ALL.iter().enumerate() {
            let automation = effects.session().load().automation.clone();
            max_lfo_or_control(&mut effects, &automation, row + 1, Tab::Chords, 0, 0.0);
            let snapshot = effects.session().load();
            let route = snapshot.automation.route(address).unwrap();
            assert_at_ceiling(field.label(), route.field_value(*field), field.scale());
        }

        // Steps staircase, now reachable because Shape maxed into `Steps`.
        for target in [StepTarget::Count, StepTarget::Glide, StepTarget::Value(0)] {
            let automation = effects.session().load().automation.clone();
            let row = lfo_submenu_rows(&automation, address)
                .iter()
                .position(|r| matches!(r, LfoSubRow::Step(t) if *t == target))
                .expect("a Steps-shaped LFO exposes its staircase rows");
            max_lfo_or_control(&mut effects, &automation, row + 1, Tab::Chords, 0, 0.0);
            let snapshot = effects.session().load();
            let route = snapshot.automation.route(address).unwrap();
            assert_at_ceiling(
                &route.step_label(target),
                route.step_value(target),
                LfoRoute::step_scale(target),
            );
        }

        // A macro stacked on an LFO field.
        let key = unit_key(ID, LfoField::Amount.macro_key());
        effects.edit_session(AutoOwnership::TakeOver, Some(ID), |snapshot| {
            snapshot.automation.toggle_open_field(key.clone());
        });
        let automation = effects.session().load().automation.clone();
        let row = lfo_submenu_rows(&automation, address)
            .iter()
            .position(|r| matches!(r, LfoSubRow::FieldMacro(LfoField::Amount, slot) if *slot == MacroField::ALL[0]))
            .expect("an expanded field macro exposes its slot rows");
        max_lfo_or_control(&mut effects, &automation, row + 1, Tab::Chords, 0, 0.0);
        let snapshot = effects.session().load();
        let route = *snapshot
            .automation
            .field_macro(&key)
            .expect("the field macro exists once expanded");
        drop(snapshot);
        assert_at_ceiling(
            "lfo field macro",
            route.field_value(MacroField::ALL[0]),
            MacroField::SCALE,
        );

        // Envelope fields.
        let mut effects = executor();
        open_modulator_effect_for_id(&mut effects, ID, ModKind::Envelope, &mut selected);
        for (row, field) in EnvField::ALL.iter().enumerate() {
            let automation = effects.session().load().automation.clone();
            max_lfo_or_control(&mut effects, &automation, row + 1, Tab::Chords, 0, 0.0);
            let snapshot = effects.session().load();
            let route = snapshot.automation.envelope(address).unwrap();
            assert_at_ceiling(field.label(), route.field_value(*field), field.scale());
        }

        // A control's own macro slots.
        let mut effects = executor();
        open_modulator_effect_for_id(&mut effects, ID, ModKind::Macro, &mut selected);
        for (row, field) in MacroField::ALL.iter().enumerate() {
            let automation = effects.session().load().automation.clone();
            max_lfo_or_control(&mut effects, &automation, row + 1, Tab::Chords, 0, 0.0);
            let snapshot = effects.session().load();
            let route = snapshot.automation.macro_route(address).unwrap();
            assert_at_ceiling(&field.label(), route.field_value(*field), MacroField::SCALE);
        }
    }

    /// Row 0 is the parent slider, so the gesture still reaches the control
    /// itself while an editor is open — the reset gesture behaves the same way.
    #[test]
    fn ceiling_gesture_still_reaches_the_control_on_the_parent_row() {
        let mut effects = executor();
        let mut selected = 0;
        open_modulator_effect_for_id(&mut effects, "pad.level", ModKind::Lfo, &mut selected);
        let automation = effects.session().load().automation.clone();
        let spec = spec_by_id("pad.level").expect("pad.level exists");

        max_lfo_or_control(&mut effects, &automation, 0, Tab::Chords, 0, 0.0);

        let level = effects.session().load().controls.pad.level;
        assert!(
            (level - spec.max).abs() < f32::EPSILON,
            "parent row should max the control, got {level}"
        );
    }

    #[test]
    fn toggle_mute_zeroes_and_restores_the_track_level() {
        let mut controls = FluidControls::default();
        controls.perc.level = 0.65;
        let mut executor = executor_with(controls);
        let mut mute: MuteState = [None; 9];

        executor.toggle_mute(Tab::Perc, &mut mute);
        assert_eq!(executor.session().load().controls.perc.level, 0.0);
        assert!(mute[Tab::Perc as usize].is_some());

        executor.toggle_mute(Tab::Perc, &mut mute);
        assert_eq!(executor.session().load().controls.perc.level, 0.65);
        assert!(mute[Tab::Perc as usize].is_none());
    }

    #[test]
    fn toggle_mute_on_master_is_independent_of_track_mute() {
        let mut controls = FluidControls::default();
        controls.master.level = 0.8;
        controls.bass.level = 0.5;
        let mut executor = executor_with(controls);
        let mut mute: MuteState = [None; 9];

        executor.toggle_mute(Tab::Master, &mut mute);
        assert_eq!(executor.session().load().controls.master.level, 0.0);
        assert_eq!(executor.session().load().controls.bass.level, 0.5);

        executor.toggle_mute(Tab::Bass, &mut mute);
        assert_eq!(executor.session().load().controls.bass.level, 0.0);
        // Master stays muted; muting bass didn't disturb it or restore it early.
        assert_eq!(executor.session().load().controls.master.level, 0.0);

        executor.toggle_mute(Tab::Master, &mut mute);
        assert_eq!(executor.session().load().controls.master.level, 0.8);
        assert_eq!(executor.session().load().controls.bass.level, 0.0);
    }

    #[test]
    fn toggle_mute_on_macros_tab_is_a_no_op() {
        let mut executor = executor_with(FluidControls::default());
        let mut mute: MuteState = [None; 9];

        executor.toggle_mute(Tab::Macros, &mut mute);
        assert!(mute[Tab::Macros as usize].is_none());
    }

    /// Muting used to stand auto mode down, because `edit_session` exited auto
    /// unconditionally. The morph then stopped mid-write and stranded whatever
    /// automation it had just installed, which kept rendering as lanes on
    /// controls that never had routes.
    #[test]
    fn muting_never_stands_auto_mode_down() {
        let mut executor = executor_with(FluidControls::default());
        let mut mute: MuteState = [None; 9];
        executor.toggle_auto(0.0);
        assert!(
            executor.auto_morph_ids(0.0).is_some(),
            "auto should be running"
        );

        executor.toggle_mute(Tab::Master, &mut mute);
        assert!(
            executor.auto_morph_ids(0.0).is_some(),
            "mute must not end the morph"
        );
        executor.toggle_mute(Tab::Master, &mut mute);
        assert!(
            executor.auto_morph_ids(0.0).is_some(),
            "unmute must not end the morph either"
        );
    }

    /// The other half of the contract: a deliberate edit *does* take over, or
    /// the morph would immediately overwrite what the user just set.
    #[test]
    fn editing_a_control_stands_auto_mode_down() {
        let mut executor = executor_with(FluidControls::default());
        executor.toggle_auto(0.0);
        assert!(executor.auto_morph_ids(0.0).is_some());

        executor
            .execute(LiveEffect::EditControl {
                id: "bass.level",
                edit: ControlEdit::Delta(1.0),
            })
            .expect("editing a real control succeeds");
        assert!(
            executor.auto_morph_ids(0.0).is_none(),
            "a deliberate edit takes the controls over"
        );
    }

    #[test]
    fn ordered_effects_publish_then_apply_ui_consequences() {
        let mut executor = executor();
        let results = executor.execute_ordered([
            LiveEffect::EditControl {
                id: "master.bpm",
                edit: ControlEdit::Value(91.0),
            },
            LiveEffect::ShowMessage("done".into()),
        ]);

        assert!(matches!(
            results[0],
            Ok(EffectAcknowledgement::Published { generation: 1 })
        ));
        assert_eq!(executor.session.load().controls.master.bpm, 91.0);
        assert_eq!(executor.recent.ids(), &["master.bpm"]);
        assert_eq!(executor.message(), Some("done"));
    }

    #[test]
    fn ordered_effects_stop_at_first_failure() {
        let mut executor = executor();
        let results = executor.execute_ordered([
            LiveEffect::EditControl {
                id: "missing.control",
                edit: ControlEdit::Value(1.0),
            },
            LiveEffect::ShowMessage("must not run".into()),
        ]);

        assert_eq!(
            results,
            vec![Err(EffectFailure::UnknownControl("missing.control"))]
        );
        assert_eq!(executor.message(), None);
        assert_eq!(executor.session.load().generation, 0);
    }

    #[test]
    fn failed_clipboard_is_reported_without_false_success_message() {
        let mut executor = executor();
        let mut clipboard = FakeClipboard {
            failure: Some("denied".into()),
            ..FakeClipboard::default()
        };
        assert_eq!(
            executor.execute_with_clipboard(LiveEffect::CopySong, &mut clipboard),
            Err(EffectFailure::Clipboard("denied".into()))
        );
        assert_eq!(executor.message(), None);
    }

    #[test]
    fn interaction_effects_require_context_and_never_silently_drop() {
        let mut executor = executor();
        assert_eq!(
            executor.execute_interaction(
                InteractionEffect::AdjustSelected(1),
                &InteractionExecutionContext::default(),
            ),
            Err(EffectFailure::MissingContext("selected control"))
        );
        assert_eq!(
            executor.execute_interaction(
                InteractionEffect::PerformanceInstrument(PerformanceInstrument::Kick),
                &InteractionExecutionContext::default(),
            ),
            Err(EffectFailure::UnsupportedInteraction(
                InteractionEffect::PerformanceInstrument(PerformanceInstrument::Kick)
            ))
        );
    }

    #[test]
    fn ordered_interaction_failure_prevents_later_clipboard_side_effect() {
        let mut executor = executor();
        let mut clipboard = FakeClipboard::default();
        let results = executor.execute_interactions_ordered_with_clipboard(
            [
                InteractionEffect::SelectPage(Page::Bass),
                InteractionEffect::PerformanceInstrument(PerformanceInstrument::Bass),
                InteractionEffect::Save,
            ],
            &InteractionExecutionContext::default(),
            &mut clipboard,
        );
        assert_eq!(results.len(), 2);
        assert_eq!(
            results[0],
            Ok(EffectAcknowledgement::PageSelected(Page::Bass))
        );
        assert_eq!(
            results[1],
            Err(EffectFailure::UnsupportedInteraction(
                InteractionEffect::PerformanceInstrument(PerformanceInstrument::Bass)
            ))
        );
        assert_eq!(clipboard.value, None);
    }

    #[test]
    fn typed_palette_jump_is_acknowledged_without_session_mutation() {
        let mut executor = executor();
        let acknowledgement = executor
            .execute_interaction(
                InteractionEffect::PaletteJump {
                    tab: Tab::Master,
                    index: 1,
                    id: "master.bpm",
                },
                &InteractionExecutionContext::default(),
            )
            .unwrap();

        assert_eq!(
            acknowledgement,
            EffectAcknowledgement::ControlSelected {
                tab: Tab::Master,
                index: 1,
                id: "master.bpm",
            }
        );
        assert_eq!(executor.recent.ids(), &["master.bpm"]);
        assert_eq!(executor.session.load().generation, 0);
    }

    #[test]
    fn typed_palette_commit_applies_payload_atomically() {
        let mut executor = executor();
        let acknowledgement = executor
            .execute_interaction(
                InteractionEffect::PaletteCommit(vec![
                    PaletteStagedEdit {
                        id: "master.bpm",
                        value_bits: 99.0f32.to_bits(),
                    },
                    PaletteStagedEdit {
                        id: "pad.level",
                        value_bits: 25.0f32.to_bits(),
                    },
                ]),
                &InteractionExecutionContext {
                    beat: 12.0,
                    ..InteractionExecutionContext::default()
                },
            )
            .unwrap();

        assert_eq!(
            acknowledgement,
            EffectAcknowledgement::Published { generation: 1 }
        );
        let session = executor.session.load();
        assert_eq!(session.controls.master.bpm, 99.0);
        assert_eq!(session.controls.pad.level, 0.25);
        assert_eq!(executor.recent.ids(), &["master.bpm", "pad.level"]);
    }

    #[test]
    fn empty_typed_palette_commit_is_an_explicit_no_change() {
        let mut executor = executor();
        assert_eq!(
            executor.execute_interaction(
                InteractionEffect::PaletteCommit(Vec::new()),
                &InteractionExecutionContext {
                    beat: 12.0,
                    ..InteractionExecutionContext::default()
                },
            ),
            Ok(EffectAcknowledgement::NoChange)
        );
        assert_eq!(executor.session.load().generation, 0);
        assert!(executor.pending().is_none());
        assert!(executor.recent.ids().is_empty());
    }

    #[test]
    fn selection_consequence_updates_mru_and_returns_navigation_target() {
        let mut executor = executor();
        let result = executor
            .execute(LiveEffect::SelectControl {
                tab: Tab::Master,
                index: 1,
                id: "master.bpm",
            })
            .unwrap();

        assert_eq!(
            result,
            EffectAcknowledgement::ControlSelected {
                tab: Tab::Master,
                index: 1,
                id: "master.bpm",
            }
        );
        assert_eq!(executor.recent.ids(), &["master.bpm"]);
        assert_eq!(executor.session.load().generation, 0);
    }

    #[test]
    fn staged_commit_is_atomic_and_updates_mru_at_due_beat() {
        let mut executor = executor();
        let edits = vec![
            StagedEdit {
                id: "master.bpm",
                value: 99.0,
            },
            StagedEdit {
                id: "pad.level",
                value: 25.0,
            },
        ];
        executor
            .execute(LiveEffect::StageForBar {
                target_beat: 8.0,
                edits,
            })
            .unwrap();
        executor
            .execute(LiveEffect::CommitPending { beat: 7.99 })
            .unwrap();
        assert_eq!(executor.session.load().generation, 0);
        executor
            .execute(LiveEffect::CommitPending { beat: 8.0 })
            .unwrap();
        let snapshot = executor.session.load();
        assert_eq!(snapshot.generation, 1);
        assert_eq!(snapshot.controls.master.bpm, 99.0);
        assert_eq!(executor.recent.ids(), &["master.bpm", "pad.level"]);
        assert!(executor.pending().is_none());
    }

    #[test]
    fn invalid_staged_edit_fails_without_publication_or_pending_state() {
        let mut executor = executor();
        let result = executor.execute(LiveEffect::StageForBar {
            target_beat: 4.0,
            edits: vec![StagedEdit {
                id: "missing.control",
                value: 1.0,
            }],
        });

        assert_eq!(
            result,
            Err(EffectFailure::UnknownControl("missing.control"))
        );
        assert_eq!(executor.session.load().generation, 0);
        assert!(executor.pending().is_none());
        assert!(executor.recent.ids().is_empty());
    }

    #[test]
    fn aggregate_edit_exits_auto_before_publishing() {
        let controls = FluidControls::default();
        let session = LiveSession::new(LiveSessionSnapshot::from_controls(controls.clone()));
        let morph = Arc::new(ArcSwap::from_pointee(Some(MorphState::from_live(
            controls,
            AutomationState::default(),
            decode_auto_states(),
            DEFAULT_AUTO_BARS,
            0.0,
        ))));
        let auto = AutoControls::new(Arc::clone(&morph), decode_auto_states(), DEFAULT_AUTO_BARS);
        let mut executor = EffectExecutor::new(session, auto);

        executor
            .execute(LiveEffect::EditControl {
                id: "master.bpm",
                edit: ControlEdit::Value(88.0),
            })
            .unwrap();

        assert!(morph.load().is_none());
        assert_eq!(executor.session.load().controls.master.bpm, 88.0);
        assert_eq!(executor.recent.ids(), &["master.bpm"]);
        let stale_publish = executor.session.transact(|snapshot| {
            if morph.load().is_none() {
                return Err("auto stopped");
            }
            snapshot.controls.master.bpm = 140.0;
            Ok(())
        });
        assert!(matches!(stale_publish, Err("auto stopped")));
        assert_eq!(executor.session.load().controls.master.bpm, 88.0);
    }

    #[test]
    fn standalone_toggle_off_fences_stale_auto_publication() {
        use std::sync::Barrier;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::thread;

        let controls = FluidControls::default();
        let session = LiveSession::new(LiveSessionSnapshot::from_controls(controls.clone()));
        let morph = Arc::new(ArcSwap::from_pointee(Some(MorphState::from_live(
            controls,
            AutomationState::default(),
            decode_auto_states(),
            DEFAULT_AUTO_BARS,
            0.0,
        ))));
        let auto = AutoControls::new(Arc::clone(&morph), decode_auto_states(), DEFAULT_AUTO_BARS);
        let mut executor = EffectExecutor::new(session, auto);
        let stale_source = morph.load_full();
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let attempts = Arc::new(AtomicUsize::new(0));
        let writer_session = executor.session.clone();
        let writer_morph = Arc::clone(&morph);
        let writer_entered = Arc::clone(&entered);
        let writer_release = Arc::clone(&release);
        let writer_attempts = Arc::clone(&attempts);
        let writer = thread::spawn(move || {
            writer_session.transact(|snapshot| {
                let source_is_current = Arc::ptr_eq(&writer_morph.load_full(), &stale_source);
                if writer_attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                    assert!(stale_source.is_some());
                    assert!(source_is_current);
                    writer_entered.wait();
                    writer_release.wait();
                }
                if !source_is_current {
                    return Err("auto stopped");
                }
                snapshot.controls.master.bpm = 140.0;
                Ok(())
            })
        });

        entered.wait();
        executor.toggle_auto(1.0);
        release.wait();
        let stale_publish = writer.join().unwrap();

        assert!(morph.load().is_none());
        assert_eq!(executor.session.load().generation, 1);
        assert!(matches!(stale_publish, Err("auto stopped")));
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert_eq!(
            executor.session.load().controls.master.bpm,
            FluidControls::default().master.bpm
        );
    }
}
