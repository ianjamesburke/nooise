//! The shared production turn coordinator, and the live UI loop built on it.
//!
//! One ordering seam that both the live scheduler loop and the replay harness
//! cross: a scheduler-due tick commits before that turn's events.

use super::*;

const SAVE_MESSAGE_TTL: std::time::Duration = std::time::Duration::from_secs(3);

pub(crate) struct UiSession {
    pub(crate) live: LiveSession,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ProductionEffectRecord {
    pub(crate) effect: interaction::InteractionEffect,
    pub(crate) result: Result<EffectAcknowledgement, EffectFailure>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ProductionActionRecord {
    pub(crate) action: interaction::SemanticAction,
    pub(crate) before: interaction::InteractionModel,
    pub(crate) after: interaction::InteractionModel,
    pub(crate) effects: Vec<ProductionEffectRecord>,
}

impl ProductionActionRecord {
    pub(crate) fn requested_quit(&self) -> bool {
        self.effects
            .iter()
            .any(|record| record.result == Ok(EffectAcknowledgement::QuitRequested))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ProductionStep {
    pub(crate) mapping: runtime::InputMapping,
    pub(crate) actions: Vec<ProductionActionRecord>,
    pub(crate) quit: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ProductionTurn {
    pub(crate) steps: Vec<ProductionStep>,
    pub(crate) quit: bool,
}

pub(crate) struct ProductionCoordinatorContext<'a> {
    pub(crate) effects: &'a mut EffectExecutor,
    pub(crate) flipped: &'a mut FlippedUnits,
    pub(crate) clipboard: &'a mut dyn Clipboard,
    pub(crate) capabilities: runtime::TerminalCapabilities,
    pub(crate) beat: f64,
}

pub(crate) fn coordinate_production_tick(
    effects: &mut EffectExecutor,
    beat: f64,
) -> Result<EffectAcknowledgement, EffectFailure> {
    effects.poll_output_routing();
    effects.execute(LiveEffect::CommitPending { beat })
}

/// One pre-action session snapshot and its lightweight navigation selection.
pub(crate) struct ProductionFrame {
    pub(crate) session: Arc<LiveSessionSnapshot>,
    pub(crate) item_count: usize,
    pub(crate) selected_control: Option<&'static str>,
    pub(crate) visible_control_ids: Vec<&'static str>,
    pub(crate) randomizes_automation: bool,
    pub(crate) tab: Tab,
    pub(crate) selected: Option<usize>,
}

pub(crate) fn production_frame(
    model: &mut interaction::InteractionModel,
    context: &ProductionCoordinatorContext<'_>,
) -> ProductionFrame {
    let session = context.effects.session().load();
    let visible_control_ids = view::navigation_control_ids(model.navigation, &session.controls);
    let item_count = visible_control_ids.len();
    let selected_row = model
        .navigation
        .selected()
        .min(item_count.saturating_sub(1));
    let selected_control = visible_control_ids.get(selected_row).copied();
    let randomizes_automation = matches!(model.mode, interaction::InteractionMode::Automation(_));
    let tab = model.navigation.tab();
    let selected = selected_control.and_then(|id| spec_index(tab, id));
    model.clamp_navigation_selection(item_count);
    ProductionFrame {
        session,
        item_count,
        selected_control,
        visible_control_ids,
        randomizes_automation,
        tab,
        selected,
    }
}

pub(crate) fn coordinate_production_event(
    model: &mut interaction::InteractionModel,
    event: &runtime::TransportEvent,
    context: &mut ProductionCoordinatorContext<'_>,
) -> ProductionStep {
    let frame = production_frame(model, context);
    let mapping = runtime::map_input(&model.mode, event, context.capabilities);
    let runtime::InputMapping::Action(action) = mapping else {
        return ProductionStep {
            mapping,
            actions: Vec::new(),
            quit: false,
        };
    };

    let repeat_count = match event {
        runtime::TransportEvent::Key { repeat_count, .. } => repeat_count.max(&1),
        _ => &1,
    };
    let mut actions = Vec::new();
    let mut quit = false;
    for _ in 0..*repeat_count {
        let Some(record) = coordinate_production_action(model, action, &frame, context) else {
            break;
        };
        quit = record.requested_quit();
        actions.push(record);
        if quit {
            break;
        }
    }
    ProductionStep {
        mapping,
        actions,
        quit,
    }
}

/// Resolve navigation-dependent Touch/Cancel identically for every input source.
fn upgrade_navigation_action(
    model: &interaction::InteractionModel,
    mut action: interaction::SemanticAction,
    frame: &ProductionFrame,
) -> interaction::SemanticAction {
    if action.intent == interaction::Intent::TouchSelected {
        action.intent = match model.navigation {
            interaction::Navigation::Chords {
                drill: interaction::ChordDrill::Progression { .. },
                ..
            } => frame
                .selected_control
                .and_then(parse_chord_slot_id)
                .map_or(action.intent, |(slot, _)| {
                    interaction::Intent::EnterChordSlot(slot)
                }),
            interaction::Navigation::Hub { selected } => interaction::hub_layer(selected)
                .map_or(action.intent, interaction::Intent::EnterLayer),
            _ => action.intent,
        };
    }
    if action.intent == interaction::Intent::Cancel
        && matches!(model.mode, interaction::InteractionMode::Browsing)
        && matches!(model.navigation, interaction::Navigation::Hub { .. })
        && !model.gesture_input.has_active()
        && frame.session.gestures.has_held()
    {
        action.intent = interaction::Intent::ReleaseAllGestures;
    }
    if action.intent == interaction::Intent::TouchSelected
        && matches!(
            model.navigation,
            interaction::Navigation::Chords {
                drill: interaction::ChordDrill::None,
                ..
            }
        )
        && frame.selected_control == Some("pad.progression")
    {
        action.intent = interaction::Intent::EnterChordProgression;
    }
    if action.intent == interaction::Intent::TouchSelected
        && matches!(
            model.navigation,
            interaction::Navigation::Perc {
                drill: interaction::PercDrill::None,
                ..
            }
        )
        && frame.selected_control == Some(PERC_PATTERN_ID)
    {
        action.intent = interaction::Intent::EnterPercPattern;
    }
    if action.intent == interaction::Intent::TouchSelected
        && matches!(
            model.navigation,
            interaction::Navigation::Bass {
                drill: interaction::BassDrill::None,
                ..
            }
        )
        && frame.selected_control == Some(BASS_RHYTHM_ID)
    {
        action.intent = interaction::Intent::EnterBassPattern;
    }
    if action.intent == interaction::Intent::TouchSelected
        && matches!(
            model.navigation,
            interaction::Navigation::Kick {
                drill: interaction::KickDrill::None,
                ..
            }
        )
        && frame.selected_control == Some(KICK_PATTERN_ID)
    {
        action.intent = interaction::Intent::EnterKickPattern;
    }
    if action.intent == interaction::Intent::TouchSelected
        && matches!(
            model.navigation,
            interaction::Navigation::Tonal {
                drill: interaction::TonalDrill::None,
                ..
            }
        )
        && frame.selected_control == Some(TONAL_PHRASE_ID)
    {
        action.intent = interaction::Intent::EnterTonalPattern;
    }
    if action.intent == interaction::Intent::TouchSelected
        && matches!(
            model.navigation,
            interaction::Navigation::Clap {
                drill: interaction::ClapDrill::None,
                ..
            }
        )
        && frame.selected_control == Some(CLAP_PATTERN_ID)
    {
        action.intent = interaction::Intent::EnterClapPattern;
    }
    if action.intent == interaction::Intent::TouchSelected
        && matches!(
            model.navigation,
            interaction::Navigation::Arp {
                drill: interaction::ArpDrill::None,
                ..
            }
        )
        && frame.selected_control == Some(ARP_PATTERN_ID)
    {
        action.intent = interaction::Intent::EnterArpPattern;
    }
    if action.intent == interaction::Intent::TouchSelected
        && let Some(id) = frame.selected_control
        && let Some((slot, module)) =
            module_slot_at_collapsed_id(frame.tab, id, &frame.session.controls)
        && let Some(kind) = module.kind()
        && kind.has_detail()
    {
        action.intent = interaction::Intent::EnterModuleDetail {
            tab: frame.tab,
            slot,
            catalog_index: module.kind.round() as usize - 1,
        };
    }
    // On the Lead page Enter on the Pattern row opens the lane; anywhere else
    // that is not a module drill it opens play mode, since the page is the
    // instrument and there is nothing else to touch.
    if action.intent == interaction::Intent::TouchSelected && frame.tab == Tab::Lead {
        action.intent = if matches!(
            model.navigation,
            interaction::Navigation::Lead {
                drill: interaction::LeadDrill::None,
                ..
            }
        ) && frame.selected_control == Some(LEAD_PATTERN_ID)
        {
            interaction::Intent::EnterLeadPattern
        } else {
            interaction::Intent::EnterLeadPlay
        };
    }

    action
}

/// Runs one semantic action through the kernel and the effect executor
/// against the frame it was issued in. `None` means the action was refused
/// before reaching the kernel (an automation kind the selected control does
/// not support).
pub(crate) fn coordinate_production_action(
    model: &mut interaction::InteractionModel,
    action: interaction::SemanticAction,
    frame: &ProductionFrame,
    context: &mut ProductionCoordinatorContext<'_>,
) -> Option<ProductionActionRecord> {
    let action = upgrade_navigation_action(model, action, frame);
    let frame_session = &frame.session;
    let selected_control = frame.selected_control;
    let tab = frame.tab;
    let automation_selected = model.automation_selected();
    if let interaction::Intent::OpenAutomation(kind) | interaction::Intent::AddAutomation(kind) =
        action.intent
        && !automation_kind_is_supported(selected_control, kind)
    {
        return None;
    }
    let automation_row_count = match frame_session.automation.active_kind() {
        Some(ModKind::Lfo) => frame_session
            .automation
            .active_address()
            .map_or(LfoField::ALL.len(), |address| {
                lfo_submenu_rows(&frame_session.automation, address).len()
            }),
        Some(ModKind::Envelope) => EnvField::ALL.len(),
        None => 0,
    };
    let before = model.clone();
    let transition = model
        .clone()
        .update_bounded(action, automation_row_count, frame.item_count);
    *model = transition.model;
    if let interaction::InteractionMode::Palette(palette) = &mut model.mode
        && !matches!(before.mode, interaction::InteractionMode::Palette(_))
    {
        palette.recipe_target =
            selected_control.and_then(|id| recipe::RecipeTarget::capture(id, frame_session));
        palette.chord_target = selected_control
            .and_then(|id| operation::ChordTarget::capture(id, &frame_session.controls))
            .map(Box::new);
        palette.capture_beat_bits = context.beat.to_bits();
        palette.lane_target = palette
            .resume
            .and_then(|_| LaneTarget::capture(frame_session))
            .map(Box::new);
        palette.editor_motion_target = palette.resume.and_then(|mode| {
            let address = frame_session.automation.active_address()?;
            let lane_index = frame_session.automation.active_lane_index()?;
            let selected = mode.selected();
            let target = match frame_session.automation.active_kind()? {
                ModKind::Lfo => match lfo_submenu_rows(&frame_session.automation, address)
                    .get(selected.checked_sub(1)?)?
                {
                    LfoSubRow::Field(field) => EditorMotionField::Lfo(*field),
                    LfoSubRow::Step(step) => EditorMotionField::Step(*step),
                },
                ModKind::Envelope => EditorMotionField::Envelope(env_field_at(selected)?),
            };
            Some(Box::new(EditorMotionAddress {
                control: address,
                lane_index,
                target,
            }))
        });
    }
    model.seed_palette_recent(context.effects.recent().ids());
    let entered_modal_owner = matches!(before.mode, interaction::InteractionMode::Browsing)
        && !matches!(model.mode, interaction::InteractionMode::Browsing);
    let mut emitted = transition.effects;
    // Restored holds have no physical input latch in the interaction model,
    // but entering a modal owner must still return them before that owner
    // takes over the keyboard.
    if entered_modal_owner
        && frame_session.gestures.has_held()
        && !emitted.contains(&interaction::InteractionEffect::GestureReleaseAll)
    {
        emitted.insert(0, interaction::InteractionEffect::GestureReleaseAll);
    }
    let mut execution = ProductionInteractionContext {
        selected_control,
        visible_control_ids: &frame.visible_control_ids,
        randomizes_automation: frame.randomizes_automation,
        tab,
        selected: frame.selected,
        automation_selected,
        beat: context.beat,
        flipped: context.flipped,
    };
    let results = context
        .effects
        .execute_production_interactions_with_clipboard(
            emitted.clone(),
            &mut execution,
            context.clipboard,
        );
    let mut effect_records = Vec::new();
    for (effect, result) in emitted.into_iter().zip(results) {
        match &result {
            Ok(EffectAcknowledgement::OutputDevices(choices)) => {
                let resume = match &before.mode {
                    interaction::InteractionMode::Palette(palette) => palette.resume,
                    _ => None,
                };
                model.mode =
                    interaction::InteractionMode::Palette(Box::new(interaction::PaletteMode {
                        output_choices: Some(choices.clone()),
                        resume,
                        ..Default::default()
                    }));
            }
            Ok(EffectAcknowledgement::ControlSelected { tab, index, id }) => {
                let current_session = context.effects.session().load();
                let filter_detail_field = match effect {
                    interaction::InteractionEffect::PlaceFilterCutoff { .. } => {
                        Some(ModuleSlotField::Time)
                    }
                    interaction::InteractionEffect::PlaceModule { .. }
                        if parse_module_slot_id(id)
                            .is_some_and(|(_, _, field)| field == ModuleSlotField::Amount) =>
                    {
                        Some(ModuleSlotField::Amount)
                    }
                    _ => None,
                };
                if let Some(field) = filter_detail_field
                    && let Some((_, slot, _)) = parse_module_slot_id(id)
                    && let Some(kind) = current_session
                        .controls
                        .modules
                        .for_tab(*tab)
                        .and_then(|slots| slots.get(slot))
                        .and_then(ModuleSlot::kind)
                        .filter(|kind| kind.family == Family::Filter)
                {
                    let collapsed = module_slot_spec(*tab, slot, ModuleSlotField::Time)
                        .expect("collapsed Time control is registered");
                    let collapsed_index = spec_index(*tab, collapsed.id)
                        .expect("collapsed Time control has a tab index");
                    model.select_control(*tab, collapsed_index, &current_session.controls);
                    let return_to = model.navigation.selected();
                    let catalog_index = module_catalog_index(kind.id);
                    let selected = MODULE_CATALOG[catalog_index]
                        .parameters()
                        .iter()
                        .position(|parameter| parameter.field == field)
                        .expect("module detail contains its focused field");
                    model.navigation = interaction::Navigation::Module {
                        tab: *tab,
                        slot,
                        catalog_index,
                        selected,
                        return_to,
                    };
                    model.mode = interaction::InteractionMode::Browsing;
                    effect_records.push(ProductionEffectRecord { effect, result });
                    continue;
                }
                // Other palette-added effects stay on the page they were
                // added to; Enter opens their detail.
                if let interaction::Navigation::Module {
                    tab: scoped_tab,
                    slot,
                    selected,
                    ..
                } = &mut model.navigation
                    && *scoped_tab == *tab
                    && let Some(spec) = tab_specs(*tab).get(*index)
                {
                    if let Some((_, spec_slot, field)) = parse_module_slot_id(spec.id)
                        && spec_slot == *slot
                    {
                        if let Some(kind) = frame_session
                            .controls
                            .modules
                            .for_tab(*tab)
                            .and_then(|slots| slots.get(*slot))
                            .and_then(ModuleSlot::kind)
                        {
                            *selected = kind
                                .parameters()
                                .iter()
                                .position(|parameter| parameter.field == field)
                                .unwrap_or(*selected);
                        }
                    } else {
                        model.select_control(*tab, *index, &current_session.controls);
                    }
                } else {
                    model.select_control(*tab, *index, &current_session.controls);
                }
                model.mode = interaction::InteractionMode::Browsing;
            }
            Ok(EffectAcknowledgement::Published { .. })
                if matches!(effect, interaction::InteractionEffect::ApplyRecipe { .. }) =>
            {
                model.mode = interaction::InteractionMode::Automation(
                    interaction::AutomationMode::new(interaction::AutomationKind::Lfo),
                );
            }
            Err(error) => {
                let prefix = if effect == interaction::InteractionEffect::Save {
                    "Save failed"
                } else {
                    "Action failed"
                };
                context.effects.show_failure(format!("{prefix}: {error}"));
            }
            Ok(_) => {}
        }
        effect_records.push(ProductionEffectRecord { effect, result });
    }
    Some(ProductionActionRecord {
        action,
        before,
        after: model.clone(),
        effects: effect_records,
    })
}

pub(crate) fn coordinate_production_turn(
    model: &mut interaction::InteractionModel,
    events: &[runtime::TransportEvent],
    tick_due: bool,
    context: &mut ProductionCoordinatorContext<'_>,
) -> Result<ProductionTurn, EffectFailure> {
    if tick_due {
        coordinate_production_tick(context.effects, context.beat)?;
    }
    let mut steps = Vec::with_capacity(events.len());
    let mut quit = false;
    for event in events {
        if matches!(event, runtime::TransportEvent::Shutdown) {
            let frame = production_frame(model, context);
            if let Some(record) = coordinate_production_action(
                model,
                interaction::SemanticAction {
                    phase: interaction::InputPhase::Press,
                    intent: interaction::Intent::AbandonGestures,
                },
                &frame,
                context,
            ) {
                steps.push(ProductionStep {
                    mapping: runtime::InputMapping::Action(record.action),
                    actions: vec![record],
                    quit: false,
                });
            }
            quit = true;
            break;
        }
        let step = coordinate_production_event(model, event, context);
        quit |= step.quit;
        steps.push(step);
        if quit {
            break;
        }
    }
    Ok(ProductionTurn { steps, quit })
}

pub(crate) fn apply_osc_ui_command(
    cmd: osc::OscUiCommand,
    model: &mut interaction::InteractionModel,
    context: &mut ProductionCoordinatorContext<'_>,
) {
    use osc::{OscTransportCmd, OscUiCommand};
    match cmd {
        OscUiCommand::ControlValue { id, value } => {
            if let Some(spec) = spec_by_id(&id) {
                let _ = context.effects.execute(LiveEffect::EditControl {
                    id: spec.id,
                    edit: ControlEdit::Value(value),
                });
            }
        }
        OscUiCommand::ControlRatio { id, ratio } => {
            if let Some(spec) = spec_by_id(&id) {
                let _ = context.effects.execute(LiveEffect::EditControl {
                    id: spec.id,
                    edit: ControlEdit::Ratio(ratio),
                });
            }
        }
        OscUiCommand::ControlDelta { id, steps } => {
            if let Some(spec) = spec_by_id(&id) {
                let _ = context.effects.execute(LiveEffect::EditControl {
                    id: spec.id,
                    edit: ControlEdit::Delta(steps),
                });
            }
        }
        OscUiCommand::MuteSet { tab, muted } => {
            context.effects.set_mute(tab, muted);
        }
        OscUiCommand::MuteToggle(tab) => {
            context.effects.toggle_mute(tab);
        }
        OscUiCommand::MutePlan(tab) => {
            let _ = context.effects.plan_mute(tab, context.beat);
        }
        OscUiCommand::Transport(OscTransportCmd::Play) => {
            context.effects.start_transport();
        }
        OscUiCommand::Transport(OscTransportCmd::Stop) => {
            context.effects.stop_transport();
        }
        OscUiCommand::Transport(OscTransportCmd::Toggle) => {
            context.effects.toggle_transport();
        }
        OscUiCommand::AutoToggle => {
            context.effects.toggle_auto(context.beat);
        }
        OscUiCommand::Gesture { .. } | OscUiCommand::GestureReleaseAll => {
            let action = match cmd {
                OscUiCommand::Gesture { kind, active: true } => interaction::SemanticAction {
                    phase: interaction::InputPhase::Press,
                    intent: interaction::Intent::StartGesture(kind),
                },
                OscUiCommand::Gesture {
                    kind,
                    active: false,
                } => interaction::SemanticAction {
                    phase: interaction::InputPhase::Release,
                    intent: interaction::Intent::ReleaseGesture(kind),
                },
                _ => interaction::SemanticAction {
                    phase: interaction::InputPhase::Press,
                    intent: interaction::Intent::ReleaseAllGestures,
                },
            };
            let frame = production_frame(model, context);
            let _ = coordinate_production_action(model, action, &frame, context);
        }
        OscUiCommand::MotionGrab(length_beats) => {
            let duration = match length_beats {
                4 => super::MotionDuration::Beats4,
                8 => super::MotionDuration::Beats8,
                16 => super::MotionDuration::Beats16,
                32 => super::MotionDuration::Beats32,
                _ => return,
            };
            let frame = production_frame(model, context);
            let mut exec_ctx = ProductionInteractionContext {
                selected_control: frame.selected_control,
                visible_control_ids: &frame.visible_control_ids,
                randomizes_automation: frame.randomizes_automation,
                tab: frame.tab,
                selected: frame.selected,
                automation_selected: model.automation_selected(),
                beat: context.beat,
                flipped: context.flipped,
            };
            let effect = if matches!(model.mode, interaction::InteractionMode::Automation(_)) {
                interaction::InteractionEffect::MotionGrabAutomationSelected {
                    duration,
                    selected: model.automation_selected(),
                }
            } else {
                interaction::InteractionEffect::MotionGrabSelected(duration)
            };
            let _ = context
                .effects
                .execute_production_interactions_with_clipboard(
                    [effect],
                    &mut exec_ctx,
                    context.clipboard,
                );
        }
        OscUiCommand::NavLayer(tab) => {
            model.mode = interaction::InteractionMode::Browsing;
            let frame = production_frame(model, context);
            let _ = coordinate_production_action(
                model,
                interaction::SemanticAction {
                    phase: interaction::InputPhase::Press,
                    intent: interaction::Intent::EnterLayer(tab),
                },
                &frame,
                context,
            );
        }
        OscUiCommand::NavRow(row) => {
            *model.navigation.selected_mut() = row;
            let frame = production_frame(model, context);
            model.clamp_navigation_selection(frame.item_count);
        }
        OscUiCommand::NavMove(delta) => {
            let frame = production_frame(model, context);
            let _ = coordinate_production_action(
                model,
                interaction::SemanticAction {
                    phase: interaction::InputPhase::Press,
                    intent: interaction::Intent::MoveSelection(delta as isize),
                },
                &frame,
                context,
            );
        }
        OscUiCommand::NavPage(delta) => {
            if delta == 0 {
                return;
            }
            let direction = if delta > 0 {
                interaction::PageDirection::Next
            } else {
                interaction::PageDirection::Previous
            };
            let steps = delta.unsigned_abs();
            for _ in 0..steps {
                let frame = production_frame(model, context);
                let _ = coordinate_production_action(
                    model,
                    interaction::SemanticAction {
                        phase: interaction::InputPhase::Press,
                        intent: interaction::Intent::ChangePage(direction),
                    },
                    &frame,
                    context,
                );
            }
        }
        OscUiCommand::NavTouch | OscUiCommand::NavCancel => {
            let frame = production_frame(model, context);
            let intent = if matches!(cmd, OscUiCommand::NavTouch) {
                interaction::Intent::TouchSelected
            } else {
                interaction::Intent::Cancel
            };
            let _ = coordinate_production_action(
                model,
                interaction::SemanticAction {
                    phase: interaction::InputPhase::Press,
                    intent,
                },
                &frame,
                context,
            );
        }
        OscUiCommand::NavAdjust { steps } => {
            if steps == 0 {
                return;
            }
            let dir = if steps > 0 { 1 } else { -1 };
            for _ in 0..steps.unsigned_abs() {
                let frame = production_frame(model, context);
                let _ = coordinate_production_action(
                    model,
                    interaction::SemanticAction {
                        phase: interaction::InputPhase::Press,
                        intent: interaction::Intent::AdjustSelected(dir),
                    },
                    &frame,
                    context,
                );
            }
        }
    }
}

/// The opt-in outside sources a live UI loop drains each turn. Each is
/// launch configuration, never song state.
pub(crate) struct LiveAdapters {
    pub(crate) output_routing: audio::OutputRouting,
    pub(crate) chassis_tap: Option<ChassisTapReceiver>,
    pub(crate) osc_rx: Option<osc::OscReceiver>,
    pub(crate) osc_ui_state: Option<osc::SharedOscUiState>,
}

pub(crate) fn production_ui_loop(
    terminal: &mut runtime::TerminalSession,
    session: UiSession,
    telemetry: Arc<FluidTelemetry>,
    updates: UpdateNotice,
    auto: AutoControls,
    adapters: LiveAdapters,
) -> Result<(), Box<dyn Error>> {
    let LiveAdapters {
        output_routing,
        chassis_tap,
        osc_rx,
        osc_ui_state,
    } = adapters;
    let mut model = interaction::InteractionModel::default();
    let mut effects = EffectExecutor::new(session.live, auto).with_output_routing(output_routing);
    let mut source = runtime::CrosstermEventSource::new(terminal.capabilities());
    let clock = runtime::MonotonicClock::start();
    let mut scheduler = runtime::Scheduler::new(
        runtime::SchedulerConfig::default(),
        runtime::Clock::now(&clock),
    );
    let mut fluid = RippleField::new();
    let mut flipped = FlippedUnits::new();
    let mut clipboard = SystemClipboard;
    let started = Instant::now();
    let mut last_tick = runtime::Clock::now(&clock);
    let mut quit = false;
    let mut tap_tempo = TapTempo::default();
    let mut osc_publisher = osc::OscUiPublisher::default();

    while !quit {
        let turn = scheduler.collect_turn(&mut source, &clock)?;
        let now = runtime::Clock::now(&clock);
        let tick_due = turn.tick_due;
        let render_due = turn.render_due;
        let events = turn.events;
        if let Some(receiver) = &chassis_tap {
            for (count, bpm) in receiver.drain(&mut tap_tempo) {
                effects.execute(LiveEffect::EditControl {
                    id: "master.bpm",
                    edit: ControlEdit::Value(bpm),
                })?;
                effects.show_message(format!("● TAP {count} · {bpm:.0} BPM"));
                scheduler.request_frame();
            }
        }
        if let Some(receiver) = &osc_rx {
            let osc_cmds = receiver.drain_ui();
            if !osc_cmds.is_empty() {
                let mut ctx = ProductionCoordinatorContext {
                    effects: &mut effects,
                    flipped: &mut flipped,
                    clipboard: &mut clipboard,
                    capabilities: terminal.capabilities(),
                    beat: telemetry.beat(),
                };
                for cmd in osc_cmds {
                    apply_osc_ui_command(cmd, &mut model, &mut ctx);
                }
                scheduler.request_frame();
            }
        }
        if events
            .iter()
            .any(|event| matches!(event, runtime::TransportEvent::Resize { .. }))
        {
            scheduler.request_frame();
        }
        let production = coordinate_production_turn(
            &mut model,
            &events,
            tick_due,
            &mut ProductionCoordinatorContext {
                effects: &mut effects,
                flipped: &mut flipped,
                clipboard: &mut clipboard,
                capabilities: terminal.capabilities(),
                beat: telemetry.beat(),
            },
        )
        .expect("pending commit has no fallible effects");
        quit |= production.quit;
        if !events.is_empty() {
            scheduler.request_frame();
        }

        if tick_due {
            effects.expire_message(SAVE_MESSAGE_TTL);
            let dt = now.saturating_sub(last_tick).as_secs_f32().min(0.05);
            fluid.tick(dt, &telemetry);
            last_tick = now;
            scheduler.complete_tick(now);
            scheduler.request_frame();
        }

        if (render_due || scheduler.render_due(now)) && !quit {
            let frame_session = effects.session().load();
            let gesture_now_seconds = effects.session().audio_seconds();
            let beat = telemetry.beat();
            let active_chord = telemetry.chord_slot.load(Ordering::Relaxed);
            let pending_message = frame_session
                .planned
                .map(|action| {
                    format!(
                        "\u{25cb} {} mute lands on the next bar",
                        action.tab().name()
                    )
                })
                .or_else(|| {
                    effects.pending().map(|(_, edits)| {
                        let plural = if edits.len() == 1 { "" } else { "s" };
                        format!("\u{25cb} {} edit{plural} land on the next bar", edits.len())
                    })
                });
            let auto_pos = effects.auto_position(beat);
            let auto_message = auto_pos.map(|position| {
                let name =
                    |id: Option<usize>| id.map_or_else(|| "LIVE".to_string(), |id| id.to_string());
                // Naming the pair through the hold would announce a morph that
                // has not started; say what is sounding, and only show the
                // crossing once it is actually under way.
                let stage = match position.blend {
                    None => format!("song {}", name(position.playing)),
                    Some(blend) => format!(
                        "song {} \u{2192} {}  {:.0}%",
                        name(position.playing),
                        name(position.next),
                        blend * 100.0
                    ),
                };
                format!("\u{25cf} AUTO {stage}   a or touch any param to exit")
            });
            let view = UiViewModel::project(ViewProjection {
                interaction: &model,
                session: &frame_session,
                telemetry: TelemetryView {
                    beat,
                    active_chord,
                    chord_span: telemetry.chord_span(),
                    levels: telemetry.levels(),
                    gain_reduction_db: telemetry.gain_reduction(),
                },
                presentation: ViewPresentation {
                    fluid: &fluid,
                    flipped: &flipped,
                    cursor_visible: (started.elapsed().as_millis() / 400).is_multiple_of(2),
                    notices: ViewNotices {
                        effect: effects.message().map(str::to_string),
                        effect_failed: effects.message_failed(),
                        pending_commit: pending_message,
                        auto: auto_message,
                        update: updates.message(),
                    },
                    gesture_now_seconds,
                    gesture_holds_available: terminal.capabilities().supports_holds(),
                },
            });
            if let Some(osc_ui) = &osc_ui_state {
                osc_publisher.publish(osc_ui, &model, &frame_session, &view, auto_pos.is_some());
            }
            let item_count = view.items.len();
            terminal.terminal_mut().draw(|frame| render(frame, &view))?;
            drop(view);
            model.clamp_navigation_selection(item_count);
            scheduler.complete_frame(runtime::Clock::now(&clock));
        }
    }

    Ok(())
}
