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
                let snapshot = self.edit_session(Some(id), |snapshot| match edit {
                    ControlEdit::Delta(delta) => spec.apply_delta(delta, &mut snapshot.controls),
                    ControlEdit::Value(value) => spec.apply_value(value, &mut snapshot.controls),
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

    pub(crate) fn edit_session(
        &mut self,
        recent_id: Option<&'static str>,
        mut edit: impl FnMut(&mut LiveSessionSnapshot),
    ) -> Arc<LiveSessionSnapshot> {
        self.auto.exit();
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

    pub(crate) fn toggle_mute(&mut self, tab: Tab, mute: &mut MuteState) {
        let Some(id) = tab.level_id() else { return };
        let spec = spec_by_id(id).expect("tab level_id must name a real control");
        let slot = &mut mute[tab as usize];
        match *slot {
            Some(previous) => {
                self.edit_session(Some(id), |snapshot| {
                    (spec.set)(&mut snapshot.controls, previous);
                });
                *slot = None;
            }
            None => {
                let previous = std::cell::Cell::new(0.0);
                self.edit_session(Some(id), |snapshot| {
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
            | InteractionEffect::ReleaseHeldSelector(_)) => {
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
pub(crate) fn toggle_mute(controls: &impl ControlsAccess, tab: Tab, mute: &mut MuteState) {
    let Some(id) = tab.level_id() else { return };
    let spec = spec_by_id(id).expect("tab level_id must name a real control");
    let slot = &mut mute[tab as usize];
    match *slot {
        Some(previous) => {
            controls.edit(|next| (spec.set)(next, previous));
            *slot = None;
        }
        None => {
            let previous = std::cell::Cell::new(0.0);
            controls.edit(|next| {
                previous.set((spec.get)(next));
                (spec.set)(next, 0.0);
            });
            *slot = Some(previous.get());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                InteractionEffect::PerformanceInstrument(2),
                &InteractionExecutionContext::default(),
            ),
            Err(EffectFailure::UnsupportedInteraction(
                InteractionEffect::PerformanceInstrument(2)
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
                InteractionEffect::PerformanceInstrument(1),
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
                InteractionEffect::PerformanceInstrument(1)
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
