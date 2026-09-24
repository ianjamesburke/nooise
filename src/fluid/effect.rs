//! Ordered interaction-effect execution: the adapter-side consequences of a
//! kernel transition.
//!
//! Effects run in order and stop at the first failure. Control and automation
//! mutations call *down* into `edit.rs`; this module must never call into
//! `ui.rs`.

use std::error::Error;
use std::fmt;

use super::interaction::{InteractionEffect, Page, PaletteStagedEdit};
use super::song::SongCodeError;
use super::*;
use rand::{Rng, SeedableRng, rngs::StdRng};

/// Why one effect in a transition's ordered list did not run.
///
/// Execution stops at the first failure, so a failure is also the reason the
/// effects behind it never ran.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum EffectFailure {
    /// The effect named a control id no registry spec claims.
    UnknownControl(&'static str),
    /// The song state could not be encoded, so there was nothing to copy.
    SongEncode(SongCodeError),
    /// The clipboard refused the write.
    Clipboard(ClipboardError),
    /// The effect needs a piece of live context the frame did not carry.
    MissingContext(&'static str),
    /// The selected control already carries the maximum lanes of this family.
    AutomationLaneLimit,
    /// The kernel emitted an effect this executor does not implement. New
    /// effects are rejected explicitly rather than silently dropped.
    UnsupportedInteraction(InteractionEffect),
}

impl fmt::Display for EffectFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownControl(id) => write!(f, "no control named {id}"),
            Self::SongEncode(error) => write!(f, "{error}"),
            Self::Clipboard(error) => write!(f, "{error}"),
            Self::MissingContext(what) => write!(f, "no {what} in this frame"),
            Self::AutomationLaneLimit => {
                write!(f, "this slider already has four lanes of that kind")
            }
            Self::UnsupportedInteraction(effect) => {
                write!(f, "unsupported interaction effect {effect:?}")
            }
        }
    }
}

impl Error for EffectFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::SongEncode(error) => Some(error),
            Self::Clipboard(error) => Some(error),
            _ => None,
        }
    }
}

/// Why a clipboard write did not land. The detail string is the system
/// backend's own message; the variant names which step it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ClipboardError {
    /// No system clipboard could be opened at all.
    Unavailable(String),
    /// The clipboard opened but rejected the text.
    WriteRejected(String),
}

impl fmt::Display for ClipboardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(detail) => write!(f, "clipboard unavailable: {detail}"),
            Self::WriteRejected(detail) => write!(f, "clipboard write rejected: {detail}"),
        }
    }
}

impl Error for ClipboardError {}

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

/// The control an effect targets, or the failure every selection-scoped
/// effect reports when nothing is selected.
fn selected_control(selected_control: Option<&'static str>) -> Result<&'static str, EffectFailure> {
    selected_control.ok_or(EffectFailure::MissingContext("selected control"))
}

/// Palette edits cross the kernel boundary as value bits; this is where they
/// become session edits.
fn staged_edits(edits: Vec<PaletteStagedEdit>) -> Vec<StagedEdit> {
    edits
        .into_iter()
        .map(|edit| StagedEdit {
            id: edit.id,
            value: f32::from_bits(edit.value_bits),
        })
        .collect()
}

pub(crate) struct ProductionInteractionContext<'a> {
    pub(crate) selected_control: Option<&'static str>,
    pub(crate) visible_control_ids: &'a [&'static str],
    pub(crate) randomizes_automation: bool,
    pub(crate) tab: Tab,
    pub(crate) selected: usize,
    pub(crate) automation_selected: usize,
    pub(crate) beat: f64,
    pub(crate) flipped: &'a mut FlippedUnits,
}

/// The one seam that reaches the system clipboard, so replay can substitute a
/// deterministic one.
pub(crate) trait Clipboard {
    fn set_text(&mut self, text: String) -> Result<(), ClipboardError>;
}

pub(crate) struct SystemClipboard;

impl Clipboard for SystemClipboard {
    fn set_text(&mut self, text: String) -> Result<(), ClipboardError> {
        let mut clipboard = arboard::Clipboard::new()
            .map_err(|error| ClipboardError::Unavailable(error.to_string()))?;
        clipboard
            .set_text(text)
            .map_err(|error| ClipboardError::WriteRejected(error.to_string()))
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
    /// The one source of randomness an effect may draw on (`r` rolls a
    /// control). Seeded from entropy in production and from a fixed seed in
    /// replay, so a replayed trace rolls the same values twice.
    rng: StdRng,
    /// What play mode has played lately, for `LeadCapture`. Live-only.
    phrase: LeadPhraseBuffer,
}

impl EffectExecutor {
    pub(crate) fn new(session: LiveSession, auto: AutoControls) -> Self {
        Self::seeded(session, auto, rand::random())
    }

    pub(crate) fn seeded(session: LiveSession, auto: AutoControls, seed: u64) -> Self {
        Self {
            session,
            auto,
            recent: RecentControls::default(),
            pending: None,
            message: None,
            rng: StdRng::seed_from_u64(seed),
            phrase: LeadPhraseBuffer::default(),
        }
    }

    pub(crate) fn session(&self) -> &LiveSession {
        &self.session
    }

    pub(crate) fn auto_position(&self, beat: f64) -> Option<MorphPosition> {
        self.auto.position_at(beat)
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
                let now_seconds = self.session.audio_seconds();
                let code = encode_song_code(&SongState {
                    controls: snapshot.controls.clone(),
                    automation: snapshot.automation.clone(),
                    tonal_sequence: Some(snapshot.tonal_sequence.clone()),
                    muted: snapshot.muted,
                    gestures: snapshot.gestures.snapshot_at(now_seconds),
                })
                .map_err(EffectFailure::SongEncode)?;
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

    /// Ordered generic bridge. Production drives effects through
    /// `execute_interaction_with_clipboard`; this shape exists for the
    /// adapter tests that assert stop-at-first-failure ordering.
    #[cfg(test)]
    pub(crate) fn execute_ordered(
        &mut self,
        effects: impl IntoIterator<Item = LiveEffect>,
    ) -> Vec<Result<EffectAcknowledgement, EffectFailure>> {
        run_ordered(effects, |effect| self.execute(effect))
    }

    /// Apply an edit to the live session.
    ///
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

    /// Acknowledge the generation the session currently holds. Every effect
    /// that publishes reports the generation its own edit produced, so this
    /// is only ever read after the edit.
    fn published(&self) -> EffectAcknowledgement {
        EffectAcknowledgement::Published {
            generation: self.session.load().generation,
        }
    }

    /// Run an automation edit against a detached copy of the automation the
    /// frame started from, then acknowledge what it published. The copy is
    /// what lets the edit read the route it is about to replace while the
    /// executor holds the session mutably.
    fn with_automation(
        &mut self,
        edit: impl FnOnce(&mut Self, &AutomationState),
    ) -> Result<EffectAcknowledgement, EffectFailure> {
        let automation = self.session.load().automation.clone();
        edit(self, &automation);
        Ok(self.published())
    }

    pub(crate) fn edit_navigation_automation(
        &mut self,
        mut edit: impl FnMut(&mut AutomationState),
    ) {
        self.session
            .update(|snapshot| edit(&mut snapshot.automation));
    }

    /// Resolve a palette module row. The chain already holding this module
    /// means the user wants the one that is there, not a second copy, so this
    /// jumps instead of adding. A full chain fails loudly with the count
    /// rather than silently doing nothing.
    fn place_module(
        &mut self,
        tab: Tab,
        catalog_index: usize,
    ) -> Result<EffectAcknowledgement, EffectFailure> {
        let kind = MODULE_CATALOG
            .get(catalog_index)
            .ok_or(EffectFailure::MissingContext("catalog module"))?;
        if !module_available_on(*kind, tab) {
            return Err(EffectFailure::MissingContext(
                "module is unavailable on this layer",
            ));
        }
        let existing = self
            .session
            .load()
            .controls
            .modules
            .for_tab(tab)
            .and_then(|slots| chain_amount_slot(slots, kind.id));
        let slot = match existing {
            Some(slot) => slot,
            None => {
                let placed = self.edit_session(None, |snapshot| {
                    if let Some(slots) = snapshot.controls.modules.for_tab_mut(tab)
                        && let Some(free) = slots.iter().position(ModuleSlot::is_empty)
                    {
                        // Added modules always start inert, whatever a
                        // pre-loaded slot's factory amount happens to be, so
                        // adding one is audibly free and needs no confirming.
                        slots[free] = preset_slot(kind.id, 0.0);
                    }
                });
                let Some(slot) = placed
                    .controls
                    .modules
                    .for_tab(tab)
                    .and_then(|slots| chain_amount_slot(slots, kind.id))
                else {
                    return Ok(EffectAcknowledgement::Message(format!(
                        "{} chain full ({}/{})",
                        tab.name(),
                        MODULE_SLOTS,
                        MODULE_SLOTS
                    )));
                };
                slot
            }
        };
        let id = module_slot_collapsed_id(tab, slot, &self.session.load().controls)
            .ok_or(EffectFailure::MissingContext("module slot control"))?;
        let index = spec_index(tab, id).ok_or(EffectFailure::UnknownControl(id))?;
        self.recent.touch(id);
        self.execute(LiveEffect::SelectControl { tab, index, id })
    }

    /// Mute is a session overlay, so automation and auto morph keep driving
    /// the authored level while the engine gates the final layer output.
    pub(crate) fn toggle_mute(&mut self, tab: Tab) {
        let Some(id) = tab.level_id() else { return };
        self.recent.touch(id);
        self.session
            .update(|snapshot| snapshot.muted[tab as usize] = !snapshot.muted[tab as usize]);
    }

    /// Stop or start the beat clock. Like mute it is an overlay on the song,
    /// not an edit of it, so it neither exits auto nor touches the MRU; the
    /// morph simply waits on the held beat.
    pub(crate) fn toggle_transport(&mut self) {
        self.session
            .update(|snapshot| snapshot.transport = snapshot.transport.toggled());
    }

    /// Typed bridge from the pure interaction kernel to effect execution.
    /// Effects needing adapter-owned data must receive it explicitly through
    /// `context`; unsupported staged performance effects fail visibly.
    #[cfg(test)]
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
                let id = selected_control(context.selected_control)?;
                self.execute(LiveEffect::EditControl {
                    id,
                    edit: ControlEdit::Delta(f32::from(delta)),
                })
            }
            InteractionEffect::CommitNumeric(value) => {
                let id = selected_control(context.selected_control)?;
                self.execute(LiveEffect::EditControl {
                    id,
                    edit: ControlEdit::Value(value),
                })
            }
            InteractionEffect::JumpToControl { tab, index, id } => {
                self.execute(LiveEffect::SelectControl { tab, index, id })
            }
            // Needs only the session, so the generic bridge can resolve it;
            // the production path adds closing the open editor first.
            InteractionEffect::PlaceModule { tab, catalog_index } => {
                self.place_module(tab, catalog_index)
            }
            InteractionEffect::PaletteCommit(edits) => {
                let edits = staged_edits(edits);
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
            | InteractionEffect::AddAutomation(_)
            | InteractionEffect::ResetSelected
            | InteractionEffect::ToggleAuto
            | InteractionEffect::ToggleUnits
            | InteractionEffect::ToggleMute { .. }
            | InteractionEffect::ToggleTransport
            | InteractionEffect::RemoveAutomation
            | InteractionEffect::ReseedAutomation
            | InteractionEffect::RandomizeSelected
            | InteractionEffect::RandomizeScope
            | InteractionEffect::CloseAutomationAll
            | InteractionEffect::TouchSelected
            | InteractionEffect::PaletteCommitAtBar(_)
            | InteractionEffect::LeadTone { .. }
            | InteractionEffect::LeadRelease
            | InteractionEffect::LeadNudge { .. }
            | InteractionEffect::LeadPattern
            | InteractionEffect::LeadCapture
            | InteractionEffect::GesturePress { .. }
            | InteractionEffect::GestureRelease(_)
            | InteractionEffect::GestureReleaseAll) => {
                Err(EffectFailure::UnsupportedInteraction(unsupported))
            }
        }
    }

    /// Execute one kernel transition's ordered effects and stop at the first
    /// failure. Later effects are never attempted. The production path has its
    /// own bridge below, so this generic one exists only for the tests that
    /// drive ordering without adapter-owned context.
    #[cfg(test)]
    pub(crate) fn execute_interactions_ordered_with_clipboard(
        &mut self,
        effects: impl IntoIterator<Item = InteractionEffect>,
        context: &InteractionExecutionContext,
        clipboard: &mut dyn Clipboard,
    ) -> Vec<Result<EffectAcknowledgement, EffectFailure>> {
        run_ordered(effects, |effect| {
            self.execute_interaction_with_clipboard(effect, context, clipboard)
        })
    }

    pub(crate) fn execute_production_interactions_with_clipboard(
        &mut self,
        effects: impl IntoIterator<Item = InteractionEffect>,
        context: &mut ProductionInteractionContext<'_>,
        clipboard: &mut dyn Clipboard,
    ) -> Vec<Result<EffectAcknowledgement, EffectFailure>> {
        run_ordered(effects, |effect| {
            self.execute_production_interaction(effect, context, clipboard)
        })
    }

    fn execute_production_interaction(
        &mut self,
        effect: InteractionEffect,
        context: &mut ProductionInteractionContext<'_>,
        clipboard: &mut dyn Clipboard,
    ) -> Result<EffectAcknowledgement, EffectFailure> {
        match effect {
            InteractionEffect::AdjustSelected(delta) => {
                self.with_automation(|executor, automation| {
                    adjust_lfo_or_control(
                        executor,
                        automation,
                        context.automation_selected,
                        context.tab,
                        context.selected,
                        f32::from(delta),
                        context.beat,
                        context.flipped,
                    );
                })
            }
            InteractionEffect::CommitNumeric(value) => {
                self.with_automation(|executor, automation| {
                    set_modulator_or_control(
                        executor,
                        automation,
                        context.automation_selected,
                        context.tab,
                        context.selected,
                        value,
                        context.beat,
                        context.flipped,
                    );
                })
            }
            InteractionEffect::AutomationConfirm(kind) => {
                let id = selected_control(context.selected_control)?;
                let kind = ModKind::from(kind);
                let mut selected = context.automation_selected;
                open_modulator_effect_for_id(self, id, kind, &mut selected);
                Ok(self.published())
            }
            InteractionEffect::AddAutomation(kind) => {
                let id = selected_control(context.selected_control)?;
                let kind = ModKind::from(kind);
                if !add_modulator_effect_for_id(self, id, kind) {
                    return Err(EffectFailure::AutomationLaneLimit);
                }
                Ok(self.published())
            }
            InteractionEffect::ResetSelected => self.with_automation(|executor, automation| {
                reset_lfo_or_control(
                    executor,
                    automation,
                    context.automation_selected,
                    context.tab,
                    context.selected,
                    context.beat,
                );
            }),
            InteractionEffect::ToggleAuto => {
                self.toggle_auto(context.beat);
                Ok(self.published())
            }
            InteractionEffect::ToggleUnits => self.with_automation(|executor, automation| {
                toggle_units_effect(
                    executor,
                    automation,
                    context.flipped,
                    context.automation_selected,
                    context.tab,
                    context.selected,
                    context.beat,
                );
            }),
            InteractionEffect::ToggleMute { master } => {
                self.toggle_mute(if master { Tab::Master } else { context.tab });
                Ok(self.published())
            }
            InteractionEffect::ToggleTransport => {
                self.toggle_transport();
                Ok(self.published())
            }
            InteractionEffect::RemoveAutomation => self.with_automation(|executor, automation| {
                remove_automation_effect(
                    executor,
                    automation,
                    context.selected_control,
                    context.automation_selected,
                );
            }),
            InteractionEffect::ReseedAutomation => self.with_automation(reseed_automation_effect),
            InteractionEffect::RandomizeSelected => {
                let id = selected_control(context.selected_control)?;
                let spec = spec_by_id(id).ok_or(EffectFailure::MissingContext("control"))?;
                let ratio = self.rng.r#gen::<f32>();
                let snapshot = self.edit_session(Some(spec.id), |snapshot| {
                    spec.apply_ratio(ratio, &mut snapshot.controls);
                });
                Ok(EffectAcknowledgement::Published {
                    generation: snapshot.generation,
                })
            }
            InteractionEffect::RandomizeScope => {
                let mut rng = self.rng.clone();
                let snapshot = self.edit_session(None, |snapshot| {
                    if context.randomizes_automation {
                        match snapshot.automation.active_kind() {
                            Some(ModKind::Lfo) => {
                                if let Some(address) = snapshot.automation.active_address() {
                                    let on_steps = lfo_steps_selected(
                                        &snapshot.automation,
                                        address,
                                        context.automation_selected,
                                    );
                                    if let Some(route) = snapshot.automation.route_mut(address) {
                                        if on_steps {
                                            route.randomize_step_values(&mut rng);
                                        } else {
                                            route.randomize(&mut rng, context.beat);
                                        }
                                    }
                                }
                            }
                            Some(ModKind::Envelope) => {
                                if let Some(address) = snapshot.automation.active_address()
                                    && let Some(route) = snapshot.automation.envelope_mut(address)
                                {
                                    route.randomize(&mut rng);
                                }
                            }
                            None => {}
                        }
                    } else {
                        for id in context.visible_control_ids {
                            if let Some(spec) = spec_by_id(id) {
                                spec.apply_ratio(rng.r#gen(), &mut snapshot.controls);
                            }
                        }
                    }
                });
                self.rng = rng;
                Ok(EffectAcknowledgement::Published {
                    generation: snapshot.generation,
                })
            }
            InteractionEffect::CloseAutomationAll => {
                self.edit_navigation_automation(AutomationState::close_editor);
                Ok(EffectAcknowledgement::NoChange)
            }
            InteractionEffect::TouchSelected => {
                let id = selected_control(context.selected_control)?;
                let tab = tab_owning_control(id).unwrap_or(context.tab);
                let index = spec_index(tab, id).unwrap_or(context.selected);
                self.execute(LiveEffect::SelectControl { tab, index, id })
            }
            InteractionEffect::PaletteCommitAtBar(edits) => {
                let edits = staged_edits(edits);
                self.execute(LiveEffect::StageForBar {
                    target_beat: next_bar_beat(context.beat),
                    edits,
                })
            }
            InteractionEffect::JumpToControl { tab, index, id } => {
                self.edit_navigation_automation(AutomationState::close_editor);
                self.execute(LiveEffect::SelectControl { tab, index, id })
            }
            // A played note is a gesture over the song, not an edit of it:
            // it publishes through the session so the audio thread sees it,
            // but never exits auto — soloing over a morph is the point. The
            // phrase buffer remembers it so `c` can keep it afterwards.
            InteractionEffect::LeadTone { tone, hold } => {
                self.phrase.push(LeadPress {
                    beat: context.beat,
                    tone,
                });
                let snapshot = self.session.update(|snapshot| {
                    snapshot.lead_play.presses = snapshot.lead_play.presses.wrapping_add(1);
                    snapshot.lead_play.tone = tone;
                    snapshot.lead_play.held = hold;
                });
                Ok(EffectAcknowledgement::Published {
                    generation: snapshot.generation,
                })
            }
            InteractionEffect::LeadRelease => {
                let snapshot = self
                    .session
                    .update(|snapshot| snapshot.lead_play.held = false);
                Ok(EffectAcknowledgement::Published {
                    generation: snapshot.generation,
                })
            }
            InteractionEffect::LeadNudge { id, delta } => {
                let spec = spec_by_id(id).ok_or(EffectFailure::MissingContext(id))?;
                let snapshot = self.edit_session(Some(spec.id), |snapshot| {
                    spec.apply_delta(f32::from(delta), &mut snapshot.controls);
                });
                Ok(EffectAcknowledgement::Published {
                    generation: snapshot.generation,
                })
            }
            InteractionEffect::LeadPattern => {
                let spec = spec_by_id(LEAD_PATTERN_ID)
                    .ok_or(EffectFailure::MissingContext(LEAD_PATTERN_ID))?;
                let snapshot = self.edit_session(Some(spec.id), |snapshot| {
                    let next = LeadPattern::from_value(snapshot.controls.lead.pattern).toggled();
                    spec.apply_value(next.value(), &mut snapshot.controls);
                });
                Ok(EffectAcknowledgement::Published {
                    generation: snapshot.generation,
                })
            }
            InteractionEffect::LeadCapture => {
                let lead = &self.session.load().controls.lead;
                let Some(capture) = lead_capture(
                    self.phrase.presses(),
                    context.beat,
                    lead.rate_beats,
                    lead.offset_beats,
                ) else {
                    self.message = Some(("nothing played yet".to_string(), Instant::now()));
                    return Ok(EffectAcknowledgement::NoChange);
                };
                let steps = spec_by_id(LEAD_STEPS_ID)
                    .ok_or(EffectFailure::MissingContext(LEAD_STEPS_ID))?;
                let pattern = spec_by_id(LEAD_PATTERN_ID)
                    .ok_or(EffectFailure::MissingContext(LEAD_PATTERN_ID))?;
                let snapshot = self.edit_session(Some(steps.id), |snapshot| {
                    snapshot.controls.lead.steps = capture.steps;
                    steps.apply_value(capture.count as f32, &mut snapshot.controls);
                    pattern.apply_value(LeadPattern::Play.value(), &mut snapshot.controls);
                });
                self.message = Some((format!("kept {} steps", capture.count), Instant::now()));
                Ok(EffectAcknowledgement::Published {
                    generation: snapshot.generation,
                })
            }
            InteractionEffect::GesturePress { kind, tab } => {
                let now_seconds = self.session.audio_seconds();
                let snapshot = self
                    .session
                    .update(|snapshot| snapshot.gestures.press(kind, tab, now_seconds));
                Ok(EffectAcknowledgement::Published {
                    generation: snapshot.generation,
                })
            }
            InteractionEffect::GestureRelease(kind) => {
                let now_seconds = self.session.audio_seconds();
                let snapshot = self
                    .session
                    .update(|snapshot| snapshot.gestures.release(kind, now_seconds));
                Ok(EffectAcknowledgement::Published {
                    generation: snapshot.generation,
                })
            }
            InteractionEffect::GestureReleaseAll => {
                if !self.session.load().gestures.has_held() {
                    return Ok(EffectAcknowledgement::NoChange);
                }
                let now_seconds = self.session.audio_seconds();
                let snapshot = self
                    .session
                    .update(|snapshot| snapshot.gestures.release_all(now_seconds));
                Ok(EffectAcknowledgement::Published {
                    generation: snapshot.generation,
                })
            }
            InteractionEffect::PlaceModule { tab, catalog_index } => {
                self.edit_navigation_automation(AutomationState::close_editor);
                self.place_module(tab, catalog_index)
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

/// Run an ordered effect list and stop at the first failure: every bridge
/// executes its transition's effects in order, and none of them may attempt
/// an effect after one has failed.
fn run_ordered<T>(
    effects: impl IntoIterator<Item = T>,
    mut execute: impl FnMut(T) -> Result<EffectAcknowledgement, EffectFailure>,
) -> Vec<Result<EffectAcknowledgement, EffectFailure>> {
    let mut results = Vec::new();
    for effect in effects {
        let result = execute(effect);
        let failed = result.is_err();
        results.push(result);
        if failed {
            break;
        }
    }
    results
}

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

    #[derive(Default)]
    struct FakeClipboard {
        value: Option<String>,
        failure: Option<ClipboardError>,
    }

    impl Clipboard for FakeClipboard {
        fn set_text(&mut self, text: String) -> Result<(), ClipboardError> {
            if let Some(error) = self.failure.take() {
                Err(error)
            } else {
                self.value = Some(text);
                Ok(())
            }
        }
    }

    fn executor() -> EffectExecutor {
        executor_with(FluidControls::default())
    }

    fn executor_with(controls: FluidControls) -> EffectExecutor {
        let session = LiveSession::new(LiveSessionSnapshot::from_controls(controls));
        EffectExecutor::new(
            session,
            AutoControls::new(no_morph(), decode_auto_states(), DEFAULT_AUTO_BARS),
        )
    }

    #[test]
    fn toggle_mute_preserves_the_authored_level_and_toggles_the_session_gate() {
        let mut controls = FluidControls::default();
        controls.perc.level = 0.65;
        let mut executor = executor_with(controls);

        executor.toggle_mute(Tab::Perc);
        assert_eq!(executor.session().load().controls.perc.level, 0.65);
        assert!(executor.session().load().muted[Tab::Perc as usize]);

        executor.toggle_mute(Tab::Perc);
        assert_eq!(executor.session().load().controls.perc.level, 0.65);
        assert!(!executor.session().load().muted[Tab::Perc as usize]);
    }

    #[test]
    fn toggle_mute_on_master_is_independent_of_track_mute() {
        let mut controls = FluidControls::default();
        controls.master.level = 0.8;
        controls.bass.level = 0.5;
        let mut executor = executor_with(controls);

        executor.toggle_mute(Tab::Master);
        assert!(executor.session().load().muted[Tab::Master as usize]);
        assert_eq!(executor.session().load().controls.bass.level, 0.5);

        executor.toggle_mute(Tab::Bass);
        assert!(executor.session().load().muted[Tab::Bass as usize]);
        // Master stays muted; muting bass didn't disturb it or restore it early.
        assert!(executor.session().load().muted[Tab::Master as usize]);

        executor.toggle_mute(Tab::Master);
        assert!(!executor.session().load().muted[Tab::Master as usize]);
        assert_eq!(executor.session().load().controls.master.level, 0.8);
        assert!(executor.session().load().muted[Tab::Bass as usize]);
    }

    /// Muting used to stand auto mode down, because `edit_session` exited auto
    /// unconditionally. The morph then stopped mid-write and stranded whatever
    /// automation it had just installed, which kept rendering as lanes on
    /// controls that never had routes.
    #[test]
    fn muting_never_stands_auto_mode_down() {
        let mut executor = executor_with(FluidControls::default());
        executor.toggle_auto(0.0);
        assert!(
            executor.auto_position(0.0).is_some(),
            "auto should be running"
        );

        executor.toggle_mute(Tab::Master);
        assert!(
            executor.auto_position(0.0).is_some(),
            "mute must not end the morph"
        );
        executor.toggle_mute(Tab::Master);
        assert!(
            executor.auto_position(0.0).is_some(),
            "unmute must not end the morph either"
        );
    }

    /// The other half of the contract: a deliberate edit *does* take over, or
    /// the morph would immediately overwrite what the user just set.
    #[test]
    fn editing_a_control_stands_auto_mode_down() {
        let mut executor = executor_with(FluidControls::default());
        executor.toggle_auto(0.0);
        assert!(executor.auto_position(0.0).is_some());

        executor
            .execute(LiveEffect::EditControl {
                id: "bass.level",
                edit: ControlEdit::Delta(1.0),
            })
            .expect("editing a real control succeeds");
        assert!(
            executor.auto_position(0.0).is_none(),
            "a deliberate edit takes the controls over"
        );
    }

    /// Typing a module name adds it to the current layer, inert, and lands
    /// the cursor on the row it just created.
    #[test]
    fn confirming_a_module_row_adds_it_inert_and_selects_it() {
        let mut executor = executor_with(FluidControls::default());
        let delay = module_catalog_index("delay");

        let ack = executor
            .execute_interaction(
                InteractionEffect::PlaceModule {
                    tab: Tab::Bass,
                    catalog_index: delay,
                },
                &InteractionExecutionContext::default(),
            )
            .expect("adding to a layer with free slots succeeds");

        let slots = executor.session().load().controls.modules.bass;
        let placed = chain_amount_slot(&slots, "delay").expect("delay was placed");
        // Added modules start silent whatever the factory preset does, so
        // adding one can never change what is playing.
        assert_eq!(slots[placed].amount, 0.0);
        assert!(matches!(
            ack,
            EffectAcknowledgement::ControlSelected { tab: Tab::Bass, .. }
        ));
    }

    /// The rule that stops three compressors piling up: a layer that already
    /// holds the module gets a jump, never a second copy.
    #[test]
    fn confirming_a_module_already_on_the_layer_jumps_instead_of_duplicating() {
        let mut executor = executor_with(FluidControls::default());
        let drive = module_catalog_index("drive");

        // Kick ships with Drive pre-loaded at 0.2.
        let before = executor.session().load().controls.modules.kick;
        executor
            .execute_interaction(
                InteractionEffect::PlaceModule {
                    tab: Tab::Kick,
                    catalog_index: drive,
                },
                &InteractionExecutionContext::default(),
            )
            .expect("jumping to an existing module succeeds");

        let after = executor.session().load().controls.modules.kick;
        assert_eq!(
            before, after,
            "no second Drive, and the amount is untouched"
        );
    }

    /// A full chain says so with the count rather than silently doing nothing.
    #[test]
    fn a_full_chain_refuses_loudly() {
        let mut controls = FluidControls::default();
        for slot in controls.modules.pad.iter_mut() {
            *slot = preset_slot("room", 0.0);
        }
        let mut executor = executor_with(controls);
        let delay = module_catalog_index("delay");

        let ack = executor
            .execute_interaction(
                InteractionEffect::PlaceModule {
                    tab: Tab::Chords,
                    catalog_index: delay,
                },
                &InteractionExecutionContext::default(),
            )
            .expect("a full chain is a message, not a failure");
        match ack {
            EffectAcknowledgement::Message(text) => {
                assert!(text.contains("full"), "unhelpful message: {text}");
                assert!(text.contains(&MODULE_SLOTS.to_string()), "no count: {text}");
            }
            other => panic!("expected a full-chain message, got {other:?}"),
        }
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
            failure: Some(ClipboardError::WriteRejected("denied".into())),
            ..FakeClipboard::default()
        };
        assert_eq!(
            executor.execute_with_clipboard(LiveEffect::CopySong, &mut clipboard),
            Err(EffectFailure::Clipboard(ClipboardError::WriteRejected(
                "denied".into()
            )))
        );
        assert_eq!(executor.message(), None);
    }

    /// A Delay time row steps through the registry: `contextual` hands the
    /// loaded clock's grid to `apply_delta`, so Sync moves on the beat grid
    /// and Free by 10 ms with no Delay-specific edit path in between.
    #[test]
    fn delay_time_rows_step_on_the_loaded_clocks_grid() {
        let mut controls = FluidControls::default();
        controls.modules.kick[2] = preset_slot("delay", 0.5);
        controls.modules.kick[2].time = 0.5;
        let mut executor = executor_with(controls);
        let selected = tab_specs(Tab::Kick)
            .iter()
            .position(|spec| spec.id == "kick.slot3.time")
            .expect("kick slot 3 has a time row");
        let mut flipped = FlippedUnits::default();
        let mut context = ProductionInteractionContext {
            selected_control: Some("kick.slot3.time"),
            visible_control_ids: &[],
            randomizes_automation: false,
            tab: Tab::Kick,
            selected,
            automation_selected: 0,
            beat: 0.0,
            flipped: &mut flipped,
        };
        let mut clipboard = FakeClipboard::default();

        executor.execute_production_interactions_with_clipboard(
            [InteractionEffect::AdjustSelected(1)],
            &mut context,
            &mut clipboard,
        );
        assert_eq!(
            executor.session().load().controls.modules.kick[2].time,
            0.75
        );

        executor.edit_session(None, |snapshot| {
            switch_delay_clock(&mut snapshot.controls.modules.kick[2], false, 120.0);
        });
        // Sync -> Free keeps the audible length, snapped to the 10 ms grid.
        assert_eq!(
            executor.session().load().controls.modules.kick[2].time,
            380.0
        );
        executor.execute_production_interactions_with_clipboard(
            [InteractionEffect::AdjustSelected(1)],
            &mut context,
            &mut clipboard,
        );
        assert_eq!(
            executor.session().load().controls.modules.kick[2].time,
            390.0
        );
    }

    /// With the LFO editor cursor inside the Steps staircase, `Shift+R` rolls
    /// only the live step values: the route's rate, depth, shape, seed, count,
    /// and glide, and every control, stay as they were. On a field row it
    /// still rolls the whole route.
    #[test]
    fn randomize_scope_inside_lfo_steps_rolls_only_the_step_values() {
        let address = ControlAddress::new("pad.level");
        let mut executor = executor();
        executor.edit_session(None, |snapshot| {
            let route = snapshot.automation.open_or_create(address);
            route.shape = LfoShape::Steps;
            route.set_step(StepTarget::Count, 4.0);
        });
        let before = executor.session().load();
        let before_route = *before.automation.route(address).unwrap();
        let rows = lfo_submenu_rows(&before.automation, address);
        let first_value_row = rows
            .iter()
            .position(|row| matches!(row, LfoSubRow::Step(StepTarget::Value(0))))
            .unwrap();
        let mut flipped = FlippedUnits::default();
        let mut clipboard = FakeClipboard::default();
        let mut randomize_at = |executor: &mut EffectExecutor, automation_selected| {
            let mut context = ProductionInteractionContext {
                selected_control: Some("pad.level"),
                visible_control_ids: &[],
                randomizes_automation: true,
                tab: Tab::Chords,
                selected: 0,
                automation_selected,
                beat: 0.0,
                flipped: &mut flipped,
            };
            executor.execute_production_interactions_with_clipboard(
                [InteractionEffect::RandomizeScope],
                &mut context,
                &mut clipboard,
            );
            *executor.session().load().automation.route(address).unwrap()
        };

        let rolled = randomize_at(&mut executor, first_value_row + 1);
        assert_ne!(rolled.steps, before_route.steps, "the live steps reroll");
        assert!(
            rolled.steps[..4].iter().any(|value| value.abs() > 0.1),
            "steps reroll across the whole dial, not a nudge: {:?}",
            &rolled.steps[..4]
        );
        let mut expected = before_route;
        expected.steps[..4].copy_from_slice(&rolled.steps[..4]);
        assert_eq!(rolled, expected, "nothing but the live steps moved");
        let after = executor.session().load();
        for spec in all_specs() {
            assert_eq!(
                (spec.get)(&after.controls),
                (spec.get)(&before.controls),
                "{} is untouched",
                spec.id
            );
        }

        let whole = randomize_at(&mut executor, 1);
        assert_ne!(
            (whole.cycle_beats, whole.depth_ratio, whole.seed),
            (rolled.cycle_beats, rolled.depth_ratio, rolled.seed),
            "a field row still rolls the whole route"
        );
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
                InteractionEffect::ToggleAuto,
                &InteractionExecutionContext::default(),
            ),
            Err(EffectFailure::UnsupportedInteraction(
                InteractionEffect::ToggleAuto
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
                InteractionEffect::ToggleAuto,
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
                InteractionEffect::ToggleAuto
            ))
        );
        assert_eq!(clipboard.value, None);
    }

    #[test]
    fn typed_jump_to_control_is_acknowledged_without_session_mutation() {
        let mut executor = executor();
        let acknowledgement = executor
            .execute_interaction(
                InteractionEffect::JumpToControl {
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

    /// Play mode never arms anything: presses only publish play state and
    /// feed the phrase buffer. `c` then lifts the phrase into the lane as
    /// one auto-exiting edit and sets the lane playing; with nothing played
    /// it is an explicit no-change with a notice.
    #[test]
    fn lead_capture_keeps_the_played_phrase_as_the_lane() {
        let mut executor = executor();
        let mut flipped = FlippedUnits::default();
        let mut clipboard = FakeClipboard::default();
        let mut run = |executor: &mut EffectExecutor, effect, beat| {
            let mut context = ProductionInteractionContext {
                selected_control: None,
                visible_control_ids: &[],
                randomizes_automation: false,
                tab: Tab::Lead,
                selected: 0,
                automation_selected: 0,
                beat,
                flipped: &mut flipped,
            };
            executor
                .execute_production_interactions_with_clipboard(
                    [effect],
                    &mut context,
                    &mut clipboard,
                )
                .pop()
                .unwrap()
                .unwrap()
        };
        let press = |tone| InteractionEffect::LeadTone { tone, hold: false };

        assert_eq!(
            run(&mut executor, InteractionEffect::LeadCapture, 0.0),
            EffectAcknowledgement::NoChange
        );
        assert_eq!(executor.message(), Some("nothing played yet"));

        run(&mut executor, InteractionEffect::LeadPattern, 0.0);
        assert_eq!(
            LeadPattern::from_value(executor.session.load().controls.lead.pattern),
            LeadPattern::Off
        );
        // Rate 0.5: beats 8, 8.5, 9 are steps 16, 17, 18 -> lane 0, 1, 2.
        run(&mut executor, press(1), 8.0);
        run(&mut executor, press(3), 8.5);
        run(&mut executor, press(5), 9.0);
        assert_eq!(
            executor.session.load().controls.lead.steps,
            DEFAULT_LEAD_STEPS,
            "playing edits nothing"
        );

        run(&mut executor, InteractionEffect::LeadCapture, 9.4);
        let session = executor.session.load();
        assert_eq!(session.controls.lead.step_count, 4.0);
        assert_eq!(&session.controls.lead.steps[..4], &[1.0, 3.0, 5.0, 0.0]);
        assert_eq!(
            LeadPattern::from_value(session.controls.lead.pattern),
            LeadPattern::Play,
            "keeping a phrase sets the lane playing"
        );
        assert!(!executor.auto.is_running());
        assert_eq!(executor.message(), Some("kept 4 steps"));
        assert_eq!(executor.recent.ids()[0], LEAD_STEPS_ID);
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
