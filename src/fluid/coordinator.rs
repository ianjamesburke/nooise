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
    pub(crate) fluid: &'a RippleField,
    pub(crate) flipped: &'a mut FlippedUnits,
    pub(crate) clipboard: &'a mut dyn Clipboard,
    pub(crate) capabilities: runtime::TerminalCapabilities,
    pub(crate) beat: f64,
    pub(crate) active_chord: u64,
}

pub(crate) fn coordinate_production_tick(
    effects: &mut EffectExecutor,
    beat: f64,
) -> Result<EffectAcknowledgement, EffectFailure> {
    effects.execute(LiveEffect::CommitPending { beat })
}

/// What the pre-event view projection settled before any action ran: the
/// session snapshot the frame was drawn from and the selection it resolved.
pub(crate) struct ProductionFrame {
    pub(crate) session: Arc<LiveSessionSnapshot>,
    pub(crate) item_count: usize,
    pub(crate) selected_control: Option<&'static str>,
    pub(crate) visible_control_ids: Vec<&'static str>,
    pub(crate) randomizes_automation: bool,
    pub(crate) tab: Tab,
    pub(crate) selected: usize,
}

pub(crate) fn production_frame(
    model: &mut interaction::InteractionModel,
    context: &ProductionCoordinatorContext<'_>,
) -> ProductionFrame {
    let session = context.effects.session().load();
    let gesture_now_seconds = context.effects.session().audio_seconds();
    let view = UiViewModel::project(ViewProjection {
        interaction: model,
        session: &session,
        telemetry: TelemetryView {
            beat: context.beat,
            active_chord: context.active_chord,
        },
        presentation: ViewPresentation {
            fluid: context.fluid,
            flipped: context.flipped,
            cursor_visible: true,
            notices: ViewNotices::default(),
            gesture_now_seconds,
            gesture_holds_available: context.capabilities.supports_holds(),
        },
    });
    let item_count = view.items.len();
    let selected_control = view.items.get(view.navigation.selected).map(|item| item.id);
    let visible_control_ids = view.items.iter().map(|item| item.id).collect();
    let randomizes_automation = matches!(model.mode, interaction::InteractionMode::Automation(_));
    let selected = selected_control
        .and_then(|id| spec_index(view.navigation.tab, id))
        .unwrap_or(view.navigation.selected);
    let tab = view.navigation.tab;
    drop(view);
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
    let mapping = runtime::map_input(&model.mode, model.navigation, event, context.capabilities);
    let runtime::InputMapping::Action(mut action) = mapping else {
        return ProductionStep {
            mapping,
            actions: Vec::new(),
            quit: false,
        };
    };
    if action.intent == interaction::Intent::Cancel
        && matches!(model.mode, interaction::InteractionMode::Browsing)
        && frame.session.gestures.has_held()
    {
        action.intent = interaction::Intent::ReleaseAllGestures;
    }
    if action.intent == interaction::Intent::TouchSelected
        && frame.selected_control == Some("pad.progression")
        && is_custom_progression(progression_index(frame.session.controls.pad.progression))
    {
        action.intent = interaction::Intent::EnterChordProgression;
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
    // On the Lead page Enter on the Steps row opens the lane; anywhere else
    // that is not a module drill it opens play mode, since the page is the
    // instrument and there is nothing else to touch.
    if action.intent == interaction::Intent::TouchSelected && frame.tab == Tab::Lead {
        action.intent = if frame.selected_control == Some(LEAD_STEPS_ID) {
            interaction::Intent::EnterLeadPattern
        } else {
            interaction::Intent::EnterLeadPlay
        };
    }

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
            Ok(EffectAcknowledgement::ControlSelected { tab, index, .. }) => {
                let current_session = context.effects.session().load();
                // Landing on a module row never opens its detail: a
                // palette-added effect stays on the page it was added to,
                // and Enter is the one way into a drill.
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
            Err(error) => {
                let prefix = if effect == interaction::InteractionEffect::Save {
                    "Save failed"
                } else {
                    "Action failed"
                };
                context
                    .effects
                    .execute(LiveEffect::ShowMessage(format!("{prefix}: {error}")))
                    .expect("message is infallible");
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

pub(crate) fn production_ui_loop(
    terminal: &mut runtime::TerminalSession,
    session: UiSession,
    telemetry: Arc<FluidTelemetry>,
    updates: UpdateNotice,
    auto: AutoControls,
) -> Result<(), Box<dyn Error>> {
    let mut model = interaction::InteractionModel::default();
    let mut effects = EffectExecutor::new(session.live, auto);
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

    while !quit {
        let turn = scheduler.collect_turn(&mut source, &clock)?;
        let now = runtime::Clock::now(&clock);
        let tick_due = turn.tick_due;
        let render_due = turn.render_due;
        let events = turn.events;
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
                fluid: &fluid,
                flipped: &mut flipped,
                clipboard: &mut clipboard,
                capabilities: terminal.capabilities(),
                beat: telemetry.beat(),
                active_chord: telemetry.chord_slot.load(Ordering::Relaxed),
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
            let pending_message = effects.pending().map(|(_, edits)| {
                let plural = if edits.len() == 1 { "" } else { "s" };
                format!("\u{25cb} {} edit{plural} land on the next bar", edits.len())
            });
            let auto_message = effects.auto_position(beat).map(|position| {
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
                    active_chord: telemetry.chord_slot.load(Ordering::Relaxed),
                },
                presentation: ViewPresentation {
                    fluid: &fluid,
                    flipped: &flipped,
                    cursor_visible: (started.elapsed().as_millis() / 400).is_multiple_of(2),
                    notices: ViewNotices {
                        effect: effects.message().map(str::to_string),
                        pending_commit: pending_message,
                        auto: auto_message,
                        update: updates.message(),
                    },
                    gesture_now_seconds,
                    gesture_holds_available: terminal.capabilities().supports_holds(),
                },
            });
            let item_count = view.items.len();
            terminal.terminal_mut().draw(|frame| render(frame, &view))?;
            drop(view);
            model.clamp_navigation_selection(item_count);
            scheduler.complete_frame(runtime::Clock::now(&clock));
        }
    }

    Ok(())
}
