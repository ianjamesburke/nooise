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
    pub(crate) mute: &'a mut MuteState,
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

pub(crate) fn coordinate_production_event(
    model: &mut interaction::InteractionModel,
    event: &runtime::TransportEvent,
    context: &mut ProductionCoordinatorContext<'_>,
) -> ProductionStep {
    let frame_session = context.effects.session().load();
    let view = UiViewModel::project(ViewProjection {
        interaction: model,
        session: &frame_session,
        telemetry: TelemetryView {
            beat: context.beat,
            active_chord: context.active_chord,
        },
        presentation: ViewPresentation {
            fluid: context.fluid,
            flipped: context.flipped,
            mute: context.mute,
            cursor_visible: true,
            notices: ViewNotices::default(),
        },
    });
    let item_count = view.items.len();
    let selected_control = view.items.get(view.navigation.selected).map(|item| item.id);
    let selected = selected_control
        .and_then(|id| {
            tab_specs(view.navigation.tab)
                .iter()
                .position(|spec| spec.id == id)
        })
        .unwrap_or(view.navigation.selected);
    let tab = view.navigation.tab;
    drop(view);
    model.clamp_navigation_selection(item_count);

    let mapping = runtime::map_input(&model.mode, model.navigation, event, context.capabilities);
    let runtime::InputMapping::Action(mut action) = mapping else {
        return ProductionStep {
            mapping,
            actions: Vec::new(),
            quit: false,
        };
    };
    if action.intent == interaction::Intent::TouchSelected
        && selected_control == Some("pad.progression")
        && is_custom_progression(progression_index(frame_session.controls.pad.progression))
    {
        action.intent = interaction::Intent::EnterChordProgression;
    }
    if action.intent == interaction::Intent::TouchSelected
        && let Some(id) = selected_control
        && let Some((slot, module)) = module_slot_at_amount_id(tab, id, &frame_session.controls)
        && let Some(kind) = module.kind()
        && kind.parameters().len() > 1
    {
        action.intent = interaction::Intent::EnterModuleDetail {
            tab,
            slot,
            catalog: module.kind.round() as usize - 1,
        };
    }

    let repeat_count = match event {
        runtime::TransportEvent::Key { repeat_count, .. } => repeat_count.max(&1),
        _ => &1,
    };
    let mut actions = Vec::new();
    let mut quit = false;
    for _ in 0..*repeat_count {
        let automation_selected = model.automation_selected();
        if action.intent == interaction::Intent::ToggleMacro
            && !macro_toggle_is_supported(
                &frame_session.automation,
                automation_selected,
                selected_control,
            )
        {
            break;
        }
        if let interaction::Intent::OpenAutomation(kind) = action.intent
            && !automation_kind_is_supported(selected_control, kind)
        {
            break;
        }
        let automation_row_count = match frame_session.automation.active_kind() {
            Some(ModKind::Lfo) => frame_session
                .automation
                .active_address()
                .map_or(LfoField::ALL.len(), |address| {
                    lfo_submenu_rows(&frame_session.automation, address).len()
                }),
            Some(ModKind::Envelope) => EnvField::ALL.len(),
            Some(ModKind::Macro) => MacroField::ALL.len(),
            None => 0,
        };
        let before = model.clone();
        let transition = model
            .clone()
            .update_bounded(action, automation_row_count, item_count);
        *model = transition.model;
        model.seed_palette_recent(context.effects.recent().ids());
        let emitted = transition.effects;
        let mut execution = ProductionInteractionContext {
            selected_control,
            tab,
            selected,
            automation_selected,
            beat: context.beat,
            flipped: context.flipped,
            mute: context.mute,
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
                    let selected_spec = tab_specs(*tab).get(*index);
                    let current_session = context.effects.session().load();
                    let mut opened_module = false;
                    if let Some(spec) = selected_spec
                        && let Some((slot, module)) =
                            module_slot_at_amount_id(*tab, spec.id, &current_session.controls)
                        && let Some(kind) = module.kind()
                        && kind.parameters().len() > 1
                    {
                        let return_to = match *tab {
                            Tab::Chords => chords_tab_controls(
                                &current_session.controls,
                                interaction::ChordDrill::None,
                            ),
                            _ => tab_controls(*tab, &current_session.controls),
                        }
                        .iter()
                        .position(|item| item.id == spec.id)
                        .unwrap_or(0);
                        model.navigation = interaction::Navigation::Module {
                            tab: *tab,
                            slot,
                            catalog: module.kind.round() as usize - 1,
                            selected: 0,
                            return_to,
                        };
                        model.mode = interaction::InteractionMode::Browsing;
                        opened_module = true;
                    }
                    if !opened_module
                        && let interaction::Navigation::Module {
                            tab: scoped_tab,
                            slot,
                            selected,
                            ..
                        } = &mut model.navigation
                        && *scoped_tab == *tab
                        && let Some(spec) = tab_specs(*tab).get(*index)
                    {
                        let suffix = format!(".slot{}.", *slot + 1);
                        if spec.id.contains(&suffix) {
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
                                    .position(|parameter| {
                                        spec.id.rsplit('.').next() == Some(parameter.field.id())
                                    })
                                    .unwrap_or(*selected);
                            }
                        } else {
                            model.select_control(*tab, *index, &current_session.controls);
                        }
                    } else if !opened_module {
                        model.select_control(*tab, *index, &current_session.controls);
                    }
                    model.mode = interaction::InteractionMode::Browsing;
                }
                Ok(EffectAcknowledgement::PerformanceEdited { tab, index, .. }) => {
                    let current_session = context.effects.session().load();
                    model.select_control(*tab, *index, &current_session.controls);
                }
                Ok(EffectAcknowledgement::AutomationPosition { depth, selected }) => {
                    model.apply_lfo_position(*depth, *selected);
                }
                Ok(EffectAcknowledgement::QuitRequested) => quit = true,
                Err(error) => {
                    let prefix = if effect == interaction::InteractionEffect::Save {
                        "Save failed"
                    } else {
                        "Action failed"
                    };
                    context
                        .effects
                        .execute(LiveEffect::ShowMessage(format!("{prefix}: {error:?}")))
                        .expect("message is infallible");
                }
                Ok(_) => {}
            }
            effect_records.push(ProductionEffectRecord { effect, result });
        }
        actions.push(ProductionActionRecord {
            action,
            before,
            after: model.clone(),
            effects: effect_records,
        });
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
    let mut mute: MuteState = [None; 9];
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
                mute: &mut mute,
                clipboard: &mut clipboard,
                capabilities: terminal.capabilities(),
                beat: telemetry.beat(),
                active_chord: telemetry.chord_index.load(Ordering::Relaxed),
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
            let beat = telemetry.beat();
            let pending_message = effects.pending().map(|(_, edits)| {
                let plural = if edits.len() == 1 { "" } else { "s" };
                format!("\u{25cb} {} edit{plural} land on the next bar", edits.len())
            });
            let auto_message = effects.auto_morph_ids(beat).map(|(from, to)| {
                let from = from.map_or_else(|| "LIVE".to_string(), |id| id.to_string());
                let to = to.map_or_else(|| "LIVE".to_string(), |id| id.to_string());
                format!("\u{25cf} AUTO morph {from} \u{2192} {to}   a or touch any param to exit")
            });
            let view = UiViewModel::project(ViewProjection {
                interaction: &model,
                session: &frame_session,
                telemetry: TelemetryView {
                    beat,
                    active_chord: telemetry.chord_index.load(Ordering::Relaxed),
                },
                presentation: ViewPresentation {
                    fluid: &fluid,
                    flipped: &flipped,
                    mute: &mute,
                    cursor_visible: (started.elapsed().as_millis() / 400).is_multiple_of(2),
                    notices: ViewNotices {
                        effect: effects.message().map(str::to_string),
                        pending_commit: pending_message,
                        auto: auto_message,
                        update: updates.message(),
                    },
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
