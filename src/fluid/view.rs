//! Immutable projection consumed by the Ratatui renderer.
//!
//! This is the only place that combines interaction ownership, one coherent
//! live-session generation, telemetry, and presentation-only UI state.

use super::*;
use crate::fluid::interaction::{
    AutomationKind, AutomationMode, ChordDrill, InteractionMode, InteractionModel, LEAD_NUDGES,
    LeadDrill, Navigation, PerformanceAction, PerformanceInstrument, PerformanceMode,
    SequenceStage,
};

/// The minimum supported frame. Every top-level and nested owner must render
/// a full buffer at this size.
pub(crate) const MIN_TERMINAL_WIDTH: u16 = 46;
pub(crate) const MIN_TERMINAL_HEIGHT: u16 = 11;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Who the keyboard belongs to this frame. Exactly one owner is live, which
/// is what makes mode-local key handling unambiguous.
pub(crate) enum KeyboardOwner {
    Browsing,
    Numeric,
    Palette,
    Lfo,
    Envelope,
    PerformanceSequence,
    Lead,
}

impl KeyboardOwner {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Browsing => "BROWSE",
            Self::Numeric => "NUMERIC",
            Self::Palette => "PALETTE",
            Self::Lfo => AutomationKind::Lfo.label(),
            Self::Envelope => AutomationKind::Envelope.label(),
            Self::PerformanceSequence => "SEQUENCE",
            Self::Lead => "LEAD",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NoticeKind {
    Effect,
    PendingCommit,
    Auto,
    Update,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ViewNotices {
    pub(crate) effect: Option<String>,
    pub(crate) pending_commit: Option<String>,
    pub(crate) auto: Option<String>,
    pub(crate) update: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// The footer's typed content. Precedence between notices and help is
/// decided here, once — `ui::render` must not recreate it.
pub(crate) enum HelpSurface {
    Owner { owner: KeyboardOwner, text: String },
    Notice { kind: NoticeKind, text: String },
    Browsing { text: String },
}

impl HelpSurface {
    pub(crate) fn text(&self) -> &str {
        match self {
            Self::Owner { text, .. } | Self::Notice { text, .. } | Self::Browsing { text } => text,
        }
    }

    pub(crate) fn emphasized(&self) -> bool {
        !matches!(self, Self::Browsing { .. })
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct NavigationView {
    pub(crate) tab: Tab,
    pub(crate) chord_drill: ChordDrill,
    pub(crate) lead_drill: LeadDrill,
    pub(crate) module_slot: Option<usize>,
    pub(crate) selected: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct TelemetryView {
    pub(crate) beat: f64,
    pub(crate) active_chord: u64,
}

/// Presentation-only state: never read by the engine, never persisted, and
/// free to differ between two views of the same session generation.
pub(crate) struct ViewPresentation<'a> {
    pub(crate) fluid: &'a RippleField,
    pub(crate) flipped: &'a FlippedUnits,
    pub(crate) cursor_visible: bool,
    pub(crate) notices: ViewNotices,
    pub(crate) gesture_now_seconds: f64,
    pub(crate) gesture_holds_available: bool,
}

/// Everything a projection consumes. The session is *one* generation: fields
/// are never combined across generations, which is what keeps a rendered
/// frame internally consistent.
pub(crate) struct ViewProjection<'a> {
    pub(crate) interaction: &'a InteractionModel,
    pub(crate) session: &'a LiveSessionSnapshot,
    pub(crate) telemetry: TelemetryView,
    pub(crate) presentation: ViewPresentation<'a>,
}

/// One complete render generation. All fields are derived before Ratatui sees
/// them; rendering receives no mutable application or domain state.
pub(crate) struct UiViewModel<'a> {
    pub(crate) owner: KeyboardOwner,
    pub(crate) mode: ModeSurface<'a>,
    pub(crate) navigation: NavigationView,
    pub(crate) items: Vec<ControlItem>,
    pub(crate) session: &'a LiveSessionSnapshot,
    pub(crate) telemetry: TelemetryView,
    pub(crate) fluid: &'a RippleField,
    pub(crate) flipped: &'a FlippedUnits,
    pub(crate) mute: &'a MuteState,
    pub(crate) cursor_visible: bool,
    pub(crate) help: HelpSurface,
    /// The gesture-activity row's text: a held/returning readout, or an idle
    /// key hint (`z bloom  c submerge  ...`) when nothing is held. Empty only
    /// when the terminal cannot support holds at all.
    pub(crate) activity: String,
    /// True while `activity` is a live held/returning readout rather than the
    /// idle key-hint list, so the row can render with different emphasis.
    pub(crate) activity_live: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GestureDirection {
    Rising,
    Held,
    Returning,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GestureActivity {
    pub(crate) tab: Tab,
    pub(crate) kind: GestureKind,
    pub(crate) amount_pct: u8,
    pub(crate) direction: GestureDirection,
    pub(crate) restored: bool,
}

/// The active mode's own render state, and only that mode's.
///
/// One variant is live per frame, matching the single keyboard owner. Render
/// and help must read this rather than re-deriving a mode from session state.
pub(crate) enum ModeSurface<'a> {
    Browsing,
    /// Numeric entry keeps whatever editor it was opened from projected in
    /// `resume`, so typing into a drilled-down field renders the buffer in
    /// place instead of collapsing the editor back to the parent row.
    Numeric {
        entry: String,
        resume: Option<AutomationSurface<'a>>,
    },
    Palette(PaletteSurface),
    Automation(AutomationSurface<'a>),
    Performance(PerformanceSurface),
    Lead(LeadSurface),
}

/// Lead play-mode render state: the tone last played, the live octave, and
/// whether the layer can be heard at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LeadSurface {
    pub(crate) last_tone: Option<usize>,
    pub(crate) octave: i32,
    pub(crate) level_pct: u8,
    /// `Some(false)` once a press showed the terminal reports no releases.
    pub(crate) holds: Option<bool>,
    /// Notes in the reach the keys index, so the row labels its degrees.
    pub(crate) reach_len: usize,
    /// The lane transport, so the footer says whether keys are being kept.
    pub(crate) pattern: LeadPattern,
}

/// Everything the palette overlay draws: the projected match list plus the
/// query, lock, and staged edits it was built from.
pub(crate) struct PaletteSurface {
    pub(crate) state: PaletteState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Why an automation editor could not be projected. Kept explicit so the
/// renderer shows the absence rather than falling back to another editor.
pub(crate) enum AutomationUnavailable {
    NoOpenEditor,
    KindMismatch { active: ModKind },
    MissingRoute,
}

/// The one place automation resolves for a frame.
///
/// A mode/session mismatch, or a nested LFO key that identifies no eligible
/// field on the active address, becomes `Unavailable` — never a silently
/// substituted editor. Render and help consume this; they must not infer a
/// different editor from the session.
pub(crate) enum AutomationSurface<'a> {
    Lfo {
        selected: usize,
        address: ControlAddress,
        lane_index: usize,
        lane_count: usize,
        route: &'a LfoRoute,
        state: &'a AutomationState,
    },
    Envelope {
        selected: usize,
        address: ControlAddress,
        lane_index: usize,
        lane_count: usize,
        route: &'a EnvelopeRoute,
    },
    Unavailable {
        requested: AutomationKind,
        selected: usize,
        reason: AutomationUnavailable,
    },
}

impl AutomationSurface<'_> {
    pub(crate) fn selected(&self) -> usize {
        match self {
            Self::Lfo { selected, .. }
            | Self::Envelope { selected, .. }
            | Self::Unavailable { selected, .. } => *selected,
        }
    }

    pub(crate) fn active_address(&self) -> Option<ControlAddress> {
        match self {
            Self::Lfo { address, .. } | Self::Envelope { address, .. } => Some(*address),
            Self::Unavailable { .. } => None,
        }
    }
}

/// One instrument's live row in the performance sequence. Level, length, and
/// density come from the registry, so they read exactly what the control rows
/// would.
pub(crate) struct PerformanceInstrumentSurface {
    pub(crate) instrument: PerformanceInstrument,
    pub(crate) focused: bool,
    pub(crate) held: bool,
    pub(crate) level: ControlItem,
    pub(crate) length: ControlItem,
    pub(crate) density: ControlItem,
}

/// Performance render state: the closed instrument choices, which selectors
/// are held, and how the current gesture completes under full versus reduced
/// terminal capability.
pub(crate) enum PerformanceSurface {
    Choose {
        held_selector: Option<usize>,
    },
    Perform {
        instrument: Option<usize>,
        held_selector: Option<usize>,
        values: Option<PerformanceInstrumentSurface>,
    },
    Complete {
        instrument: Option<usize>,
        release_pending: bool,
        values: Option<PerformanceInstrumentSurface>,
    },
}

impl<'a> UiViewModel<'a> {
    pub(crate) fn project(projection: ViewProjection<'a>) -> Self {
        let ViewProjection {
            interaction,
            session,
            telemetry,
            presentation,
        } = projection;
        let navigation = navigation_view(interaction.navigation);
        let items = match navigation.module_slot {
            Some(slot) => module_detail_controls(navigation.tab, slot, &session.controls),
            None => match navigation.tab {
                Tab::Chords => chords_tab_controls(&session.controls, navigation.chord_drill),
                Tab::Lead => lead_tab_controls(&session.controls, navigation.lead_drill),
                tab => tab_controls(tab, &session.controls),
            },
        };
        let mut navigation = navigation;
        navigation.selected = navigation.selected.min(items.len().saturating_sub(1));
        let owner = keyboard_owner(&interaction.mode);
        let mode = mode_surface(
            &interaction.mode,
            navigation.tab,
            &session.controls,
            &session.automation,
        );
        let gestures = gesture_activities(session, presentation.gesture_now_seconds);
        let holding_gesture = !gestures.is_empty();
        let activity_live = holding_gesture;
        let activity = if holding_gesture {
            gesture_activity_line(&gestures)
        } else if presentation.gesture_holds_available {
            gesture_idle_hint()
        } else {
            String::new()
        };
        let help = help_surface(
            owner,
            &mode,
            navigation,
            presentation.notices,
            holding_gesture,
            presentation.gesture_holds_available,
        );

        Self {
            owner,
            mode,
            navigation,
            items,
            session,
            telemetry,
            fluid: presentation.fluid,
            flipped: presentation.flipped,
            mute: &session.muted,
            cursor_visible: presentation.cursor_visible,
            help,
            activity,
            activity_live,
        }
    }
}

/// The gesture-activity row's text: one entry per held/returning envelope,
/// or empty when none are active. Kept separate from `help_surface` so a
/// gesture readout never crowds out the exits/mode help sharing the footer.
fn gesture_activity_line(gestures: &[GestureActivity]) -> String {
    gestures
        .iter()
        .map(|gesture| {
            let direction = match gesture.direction {
                GestureDirection::Rising => "↑",
                GestureDirection::Held => "●",
                GestureDirection::Returning => "↓",
            };
            let restored = if gesture.restored { "R" } else { "" };
            format!(
                "{} {} {}%{direction}{restored}",
                gesture.tab.name(),
                gesture.kind.name(),
                gesture.amount_pct
            )
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The gesture-activity row's idle text: the key/name for every hold
/// gesture, so the row that shows a live readout while holding still tells
/// you what's available when nothing is held.
fn gesture_idle_hint() -> String {
    GestureKind::ALL
        .iter()
        .map(|kind| format!("{} {}", kind.key(), kind.name().to_ascii_lowercase()))
        .collect::<Vec<_>>()
        .join("  ")
}

fn gesture_activities(session: &LiveSessionSnapshot, now_seconds: f64) -> Vec<GestureActivity> {
    let mut activities = Vec::new();
    for tab in Tab::all() {
        for kind in GestureKind::ALL {
            let envelope = session.gestures.envelope(tab, kind);
            let amount = envelope.amount_at(kind, now_seconds);
            if !envelope.held && amount <= f32::EPSILON {
                continue;
            }
            let direction = if !envelope.held {
                GestureDirection::Returning
            } else if amount >= 1.0 - f32::EPSILON {
                GestureDirection::Held
            } else {
                GestureDirection::Rising
            };
            activities.push(GestureActivity {
                tab,
                kind,
                amount_pct: (amount * 100.0).round() as u8,
                direction,
                restored: envelope.restored,
            });
        }
    }
    activities
}

fn mode_surface<'a>(
    mode: &InteractionMode,
    tab: Tab,
    controls: &FluidControls,
    automation: &'a AutomationState,
) -> ModeSurface<'a> {
    match mode {
        InteractionMode::Browsing => ModeSurface::Browsing,
        InteractionMode::Numeric(entry) => ModeSurface::Numeric {
            entry: entry.buffer.clone(),
            resume: entry
                .resume
                .map(|mode| automation_surface(mode, automation)),
        },
        InteractionMode::Palette(palette) => ModeSurface::Palette(PaletteSurface {
            state: palette.project(tab),
        }),
        InteractionMode::Automation(mode) => {
            ModeSurface::Automation(automation_surface(*mode, automation))
        }
        InteractionMode::Lead(play) => ModeSurface::Lead(LeadSurface {
            last_tone: play.last_tone,
            holds: play.holds,
            octave: controls.lead.octave.round() as i32,
            level_pct: (controls.lead.level * 100.0).round() as u8,
            reach_len: lead_page_reach(&controls.lead, &controls.pad).len(),
            pattern: LeadPattern::from_value(controls.lead.pattern),
        }),
        InteractionMode::Performance(PerformanceMode::Sequence {
            stage: SequenceStage::ChooseInstrument,
            held_selector,
        }) => ModeSurface::Performance(PerformanceSurface::Choose {
            held_selector: held_selector
                .map(crate::fluid::interaction::PerformanceInstrument::index),
        }),
        InteractionMode::Performance(PerformanceMode::Sequence {
            stage: SequenceStage::Perform { instrument },
            held_selector,
        }) => ModeSurface::Performance(PerformanceSurface::Perform {
            instrument: Some(instrument.index()),
            held_selector: held_selector
                .map(crate::fluid::interaction::PerformanceInstrument::index),
            values: Some(performance_instrument_surface(
                controls,
                *instrument,
                Some(*instrument),
                held_selector.is_some_and(|held| held == *instrument),
            )),
        }),
        InteractionMode::Performance(PerformanceMode::Sequence {
            stage: SequenceStage::AwaitActionRelease { instrument, .. },
            ..
        }) => ModeSurface::Performance(PerformanceSurface::Complete {
            instrument: Some(instrument.index()),
            release_pending: true,
            values: Some(performance_instrument_surface(
                controls,
                *instrument,
                Some(*instrument),
                false,
            )),
        }),
        InteractionMode::Performance(PerformanceMode::Sequence {
            stage: SequenceStage::CompletedFallback { instrument },
            ..
        }) => ModeSurface::Performance(PerformanceSurface::Complete {
            instrument: Some(instrument.index()),
            release_pending: false,
            values: Some(performance_instrument_surface(
                controls,
                *instrument,
                Some(*instrument),
                false,
            )),
        }),
    }
}

fn performance_instrument_surface(
    controls: &FluidControls,
    instrument: PerformanceInstrument,
    selected: Option<PerformanceInstrument>,
    held: bool,
) -> PerformanceInstrumentSurface {
    let item = |action| {
        let (_, _, spec, _) = performance_target(instrument, action)
            .expect("closed performance grammar has every target");
        spec.item(controls)
    };
    PerformanceInstrumentSurface {
        instrument,
        focused: selected == Some(instrument),
        held,
        level: item(PerformanceAction::Louder),
        length: item(PerformanceAction::Longer),
        density: item(PerformanceAction::Denser),
    }
}

fn automation_surface(mode: AutomationMode, automation: &AutomationState) -> AutomationSurface<'_> {
    let (requested, selected) = match mode {
        AutomationMode::Lfo { selected, .. } => (AutomationKind::Lfo, selected),
        AutomationMode::Envelope { selected } => (AutomationKind::Envelope, selected),
    };
    let Some(address) = automation.active_address() else {
        return AutomationSurface::Unavailable {
            requested,
            selected,
            reason: AutomationUnavailable::NoOpenEditor,
        };
    };
    let Some(active) = automation.active_kind() else {
        return AutomationSurface::Unavailable {
            requested,
            selected,
            reason: AutomationUnavailable::NoOpenEditor,
        };
    };
    let expected = ModKind::from(requested);
    if active != expected {
        return AutomationSurface::Unavailable {
            requested,
            selected,
            reason: AutomationUnavailable::KindMismatch { active },
        };
    }

    match mode {
        AutomationMode::Lfo { selected, .. } => automation.route(address).map_or(
            AutomationSurface::Unavailable {
                requested,
                selected,
                reason: AutomationUnavailable::MissingRoute,
            },
            |route| AutomationSurface::Lfo {
                selected,
                address,
                lane_index: automation.active_lane_index().unwrap_or(0),
                lane_count: automation.active_lane_count().unwrap_or(1),
                route,
                state: automation,
            },
        ),
        AutomationMode::Envelope { selected } => automation.envelope(address).map_or(
            AutomationSurface::Unavailable {
                requested,
                selected,
                reason: AutomationUnavailable::MissingRoute,
            },
            |route| AutomationSurface::Envelope {
                selected,
                address,
                lane_index: automation.active_lane_index().unwrap_or(0),
                lane_count: automation.active_lane_count().unwrap_or(1),
                route,
            },
        ),
    }
}

fn navigation_view(navigation: Navigation) -> NavigationView {
    let mut view = NavigationView {
        tab: navigation.tab(),
        chord_drill: ChordDrill::None,
        lead_drill: LeadDrill::None,
        module_slot: None,
        selected: navigation.selected(),
    };
    match navigation {
        Navigation::Chords { drill, .. } => view.chord_drill = drill,
        Navigation::Lead { drill, .. } => view.lead_drill = drill,
        Navigation::Module { slot, .. } => view.module_slot = Some(slot),
        Navigation::Standard { .. } | Navigation::Master { .. } => {}
    }
    view
}

/// The single keyboard owner for a mode. Exhaustive over `InteractionMode` on
/// purpose: a new mode cannot compile without naming who owns the keyboard
/// while it is open.
pub(crate) fn keyboard_owner(mode: &InteractionMode) -> KeyboardOwner {
    match mode {
        InteractionMode::Browsing => KeyboardOwner::Browsing,
        InteractionMode::Numeric(_) => KeyboardOwner::Numeric,
        InteractionMode::Palette(_) => KeyboardOwner::Palette,
        InteractionMode::Automation(AutomationMode::Lfo { .. }) => KeyboardOwner::Lfo,
        InteractionMode::Automation(AutomationMode::Envelope { .. }) => KeyboardOwner::Envelope,
        InteractionMode::Performance(PerformanceMode::Sequence { .. }) => {
            KeyboardOwner::PerformanceSequence
        }
        InteractionMode::Lead(_) => KeyboardOwner::Lead,
    }
}

fn help_surface(
    owner: KeyboardOwner,
    mode: &ModeSurface<'_>,
    navigation: NavigationView,
    notices: ViewNotices,
    holding_gesture: bool,
    gesture_holds_available: bool,
) -> HelpSurface {
    if owner != KeyboardOwner::Browsing {
        return HelpSurface::Owner {
            owner,
            text: owner_help(owner, mode),
        };
    }

    let ViewNotices {
        effect,
        pending_commit,
        auto,
        update,
    } = notices;
    if let Some((kind, text)) = effect
        .map(|text| (NoticeKind::Effect, text))
        .or_else(|| pending_commit.map(|text| (NoticeKind::PendingCommit, text)))
    {
        return HelpSurface::Notice { kind, text };
    }

    if holding_gesture {
        // The activity row (rendered above this one) carries the expandable
        // per-gesture detail; this row only needs to name the two exits.
        return HelpSurface::Owner {
            owner: KeyboardOwner::Browsing,
            text: "Esc release · ^Q quit".to_string(),
        };
    }

    let local_help = match (
        navigation.tab,
        navigation.chord_drill,
        navigation.module_slot,
    ) {
        (_, _, Some(_)) => {
            Some("BROWSE · Module detail   Shift+R randomize set   Esc: back".to_string())
        }
        (Tab::Chords, ChordDrill::Progression { .. }, None) => Some(
            "BROWSE · Progression   Shift+R randomize set   Enter: open chord   Esc: back"
                .to_string(),
        ),
        (Tab::Chords, ChordDrill::Slot { slot, .. }, None) => Some(format!(
            "BROWSE · Chord {}   Shift+R randomize set   Esc: back",
            slot + 1
        )),
        (Tab::Lead, _, None) if navigation.lead_drill != LeadDrill::None => Some(
            "BROWSE · Pattern   r random   Shift+R randomize set   Enter: play   Esc: back"
                .to_string(),
        ),
        _ => None,
    };
    if let Some(text) = local_help {
        return HelpSurface::Browsing { text };
    }
    if let Some((kind, text)) = auto
        .map(|text| (NoticeKind::Auto, text))
        .or_else(|| update.map(|text| (NoticeKind::Update, text)))
    {
        return HelpSurface::Notice { kind, text };
    }
    HelpSurface::Browsing {
        text: if gesture_holds_available {
            "BROWSE · jk select   h/l adjust   r random   Shift+R randomize set   / find   f LFO   e ENV   a auto   T units   ^Q quit"
                .to_string()
        } else {
            "BROWSE · hold gestures require key-up support".to_string()
        },
    }
}

/// The play-mode footer. Names the level when it is 0 so a silent Lead is
/// not mistaken for a dead keyboard.
pub(crate) fn lead_owner_help(lead: LeadSurface) -> String {
    if lead.level_pct == 0 {
        return "LEAD · level is 0 · arrows knobs · Esc, raise Level, Enter".to_string();
    }
    let lane = match lead.pattern {
        LeadPattern::Off => "lane off",
        LeadPattern::Play => "lane on",
    };
    let nudges = LEAD_NUDGES
        .iter()
        .map(|nudge| {
            let value = if nudge.id == "lead.octave" {
                format!(" {:+}", lead.octave)
            } else {
                String::new()
            };
            format!("{}{} {}{value}", nudge.down, nudge.up, nudge.label)
        })
        .collect::<Vec<_>>()
        .join("  ");
    if lead.holds == Some(false) {
        return format!(
            "LEAD · arrows knobs  taps only (no key-up)  {nudges}  Space {lane}  c keep"
        );
    }
    format!("LEAD · arrows knobs  {nudges}  Space {lane}  c keep  Esc")
}

fn owner_help(owner: KeyboardOwner, mode: &ModeSurface<'_>) -> String {
    match owner {
        KeyboardOwner::Browsing => unreachable!("browsing help is resolved separately"),
        KeyboardOwner::Numeric => "NUMERIC · type value   Enter: apply   Esc: cancel".to_string(),
        KeyboardOwner::Palette => {
            "PALETTE · type to find   Tab: complete   Enter: stage   Esc: cancel".to_string()
        }
        KeyboardOwner::Lfo | KeyboardOwner::Envelope => match mode {
            ModeSurface::Automation(surface) => automation_owner_help(surface),
            _ => unreachable!("automation owner requires automation surface"),
        },
        KeyboardOwner::Lead => match mode {
            ModeSurface::Lead(lead) => lead_owner_help(*lead),
            _ => unreachable!("lead owner requires lead surface"),
        },
        KeyboardOwner::PerformanceSequence => match mode {
            ModeSurface::Performance(PerformanceSurface::Choose { held_selector }) => {
                format!(
                    "SEQUENCE · a/s/d/f choose   held {}   Esc",
                    selector_text(*held_selector)
                )
            }
            ModeSurface::Performance(PerformanceSurface::Perform {
                instrument,
                held_selector,
                ..
            }) => format!(
                "SEQUENCE · instrument {}   h/l j/k u/i act once   held {}   Esc",
                selector_text(*instrument),
                selector_text(*held_selector)
            ),
            ModeSurface::Performance(PerformanceSurface::Complete {
                instrument,
                release_pending,
                ..
            }) => {
                if *release_pending {
                    format!(
                        "SEQUENCE · {} applied   release action to return",
                        selector_text(*instrument)
                    )
                } else {
                    format!(
                        "SEQUENCE · {} applied   Space: rearm   Esc: back",
                        selector_text(*instrument)
                    )
                }
            }
            _ => unreachable!("sequence owner requires sequence mode"),
        },
    }
}

/// One-based selector index, or `none`; shared with the Sequence body in ui.rs.
pub(crate) fn selector_text(selector: Option<usize>) -> String {
    selector
        .and_then(|index| index.checked_add(1))
        .map_or_else(|| "none".to_string(), |index| index.to_string())
}

fn automation_owner_help(surface: &AutomationSurface<'_>) -> String {
    match surface {
        AutomationSurface::Lfo {
            address,
            lane_index,
            lane_count,
            route,
            ..
        } => {
            let reseed = if route.shape.is_random() {
                "   r reseed"
            } else {
                ""
            };
            let text = format!(
                "LFO {}/{} · {}   {}   {:.2} beats   depth {:.0}%{reseed}   Shift+R randomize   f next   Shift+F add   x remove   Esc close",
                lane_index + 1,
                lane_count,
                address.id(),
                route.shape.label(),
                route.cycle_beats,
                route.depth_ratio * 100.0
            );
            text
        }
        AutomationSurface::Envelope {
            address,
            lane_index,
            lane_count,
            route,
            ..
        } => format!(
            "ENV {}/{} · {}   {}   amount {:+.0}%   Shift+R randomize   e next   Shift+E add   x remove   Esc close",
            lane_index + 1,
            lane_count,
            address.id(),
            route.field_display(EnvField::Trigger),
            route.amount * 100.0
        ),
        AutomationSurface::Unavailable {
            requested, reason, ..
        } => {
            let label = requested.label();
            let reason = match reason {
                AutomationUnavailable::NoOpenEditor => "editor unavailable",
                AutomationUnavailable::KindMismatch { .. } => "editor kind mismatch",
                AutomationUnavailable::MissingRoute => "route unavailable",
            };
            format!("{label} · {reason}   Esc: close")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fluid::interaction::{
        AutomationMode, InputPhase, Intent, LeadPlay, LfoDepth, NumericEntry, Page, PaletteMode,
        PerformanceInstrument, PerformanceMode, SemanticAction,
    };
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    fn session() -> LiveSessionSnapshot {
        LiveSessionSnapshot::from_controls(FluidControls::default())
    }

    fn snapshot_symbols(buffer: &Buffer) -> String {
        let area = buffer.area;
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| {
                        let symbol = buffer[(x, y)].symbol();
                        if symbol == " " { "␠" } else { symbol }
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn render_model_with_session(
        interaction: &InteractionModel,
        session: &LiveSessionSnapshot,
    ) -> String {
        render_model_with_session_at(interaction, session, TelemetryView::default())
    }

    fn render_model_with_session_at(
        interaction: &InteractionModel,
        session: &LiveSessionSnapshot,
        telemetry: TelemetryView,
    ) -> String {
        render_model_with_session_at_size(
            interaction,
            session,
            telemetry,
            MIN_TERMINAL_WIDTH,
            MIN_TERMINAL_HEIGHT,
        )
    }

    fn render_model_with_session_at_size(
        interaction: &InteractionModel,
        session: &LiveSessionSnapshot,
        telemetry: TelemetryView,
        width: u16,
        height: u16,
    ) -> String {
        let fluid = RippleField::new();
        let flipped = FlippedUnits::new();
        let view = UiViewModel::project(ViewProjection {
            interaction,
            session,
            telemetry,
            presentation: ViewPresentation {
                fluid: &fluid,
                flipped: &flipped,
                cursor_visible: true,
                notices: ViewNotices::default(),
                gesture_now_seconds: 0.0,
                gesture_holds_available: true,
            },
        });
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| render(frame, &view)).unwrap();
        snapshot_symbols(terminal.backend().buffer())
    }

    fn render_model(interaction: &InteractionModel) -> String {
        let mut session = session();
        let address = ControlAddress::new("pad.level");
        match &interaction.mode {
            InteractionMode::Automation(AutomationMode::Lfo { .. }) => {
                session.automation.open_or_create(address);
            }
            InteractionMode::Automation(AutomationMode::Envelope { .. }) => {
                session.automation.open_or_create_envelope(address);
            }
            _ => {}
        }
        render_model_with_session(interaction, &session)
    }

    fn render_mode(mode: InteractionMode) -> String {
        render_model(&InteractionModel {
            navigation: Navigation::default(),
            mode,
            ..InteractionModel::default()
        })
    }

    #[test]
    fn projection_reads_one_session_generation_and_clamps_navigation() {
        let mut session = session();
        session.generation = 42;
        session.controls.master.bpm = 91.0;
        let interaction = InteractionModel {
            navigation: Navigation::Master {
                selected: usize::MAX,
            },
            mode: InteractionMode::Browsing,
            ..InteractionModel::default()
        };
        let fluid = RippleField::new();
        let flipped = FlippedUnits::new();
        let view = UiViewModel::project(ViewProjection {
            interaction: &interaction,
            session: &session,
            telemetry: TelemetryView::default(),
            presentation: ViewPresentation {
                fluid: &fluid,
                flipped: &flipped,
                cursor_visible: false,
                notices: ViewNotices::default(),
                gesture_now_seconds: 0.0,
                gesture_holds_available: true,
            },
        });

        assert_eq!(view.session.generation, 42);
        assert_eq!(view.session.controls.master.bpm, 91.0);
        assert_eq!(view.navigation.selected, view.items.len() - 1);
        assert!(matches!(view.mode, ModeSurface::Browsing));
    }

    #[test]
    fn palette_owner_projects_complete_mode_local_render_state() {
        let model = InteractionModel {
            navigation: Navigation::default(),
            mode: InteractionMode::Palette(PaletteMode {
                query: "bass".to_string(),
                selected: 1,
                recent: vec!["master.bpm"],
                locked: Some(1),
                value_buffer: "42".to_string(),
                staged: vec![crate::fluid::interaction::PaletteStagedEdit {
                    id: "master.bpm",
                    value_bits: 91.0f32.to_bits(),
                }],
                resume: None,
                module_scope: None,
            }),
            ..InteractionModel::default()
        };
        let transition = model.update(SemanticAction::press(Intent::TypeCharacter('5')));
        let session = session();
        let fluid = RippleField::new();
        let flipped = FlippedUnits::new();
        let view = UiViewModel::project(ViewProjection {
            interaction: &transition.model,
            session: &session,
            telemetry: TelemetryView::default(),
            presentation: ViewPresentation {
                fluid: &fluid,
                flipped: &flipped,
                cursor_visible: false,
                notices: ViewNotices::default(),
                gesture_now_seconds: 0.0,
                gesture_holds_available: true,
            },
        });

        let ModeSurface::Palette(palette) = view.mode else {
            panic!("palette interaction must always project a palette surface");
        };
        assert_eq!(palette.state.query, "bass");
        assert_eq!(palette.state.selected, 1);
        assert_eq!(palette.state.locked, Some(1));
        assert_eq!(palette.state.value_buf, "425");
        assert_eq!(
            palette.state.staged,
            vec![StagedEdit {
                id: "master.bpm",
                value: 91.0,
            }]
        );

        let mut model = transition.model;
        for _ in 0..3 {
            model = model.update(SemanticAction::press(Intent::Backspace)).model;
        }
        let InteractionMode::Palette(palette) = &model.mode else {
            panic!("deleting a value keeps the palette open");
        };
        assert_eq!(palette.value_buffer, "");
        assert_eq!(palette.locked, Some(1));
        model = model.update(SemanticAction::press(Intent::Backspace)).model;
        let InteractionMode::Palette(palette) = model.mode else {
            panic!("empty-value backspace keeps the palette open");
        };
        assert_eq!(palette.locked, None);
    }

    #[test]
    fn projection_and_render_sanitize_arbitrary_mode_indices() {
        let invalid_palette = InteractionMode::Palette(PaletteMode {
            query: "bass".to_string(),
            locked: Some(usize::MAX),
            value_buffer: "42".to_string(),
            ..PaletteMode::default()
        });
        let session = session();
        let ModeSurface::Palette(palette) = mode_surface(
            &invalid_palette,
            Tab::Bass,
            &session.controls,
            &session.automation,
        ) else {
            panic!("palette mode projects a palette surface");
        };
        assert_eq!(palette.state.locked, None);
        assert_eq!(palette.state.value_buf, "");
        assert!(render_mode(invalid_palette).contains("/bass"));
    }

    #[test]
    fn keyboard_owner_help_preempts_browsing_notices() {
        let session = session();
        let interaction = InteractionModel {
            navigation: Navigation::default(),
            mode: InteractionMode::Numeric(NumericEntry {
                buffer: "12".to_string(),
                resume: None,
            }),
            ..InteractionModel::default()
        };
        let fluid = RippleField::new();
        let flipped = FlippedUnits::new();
        let view = UiViewModel::project(ViewProjection {
            interaction: &interaction,
            session: &session,
            telemetry: TelemetryView::default(),
            presentation: ViewPresentation {
                fluid: &fluid,
                flipped: &flipped,
                cursor_visible: false,
                notices: ViewNotices {
                    effect: Some("saved".to_string()),
                    pending_commit: Some("pending".to_string()),
                    auto: Some("auto".to_string()),
                    update: Some("update".to_string()),
                },
                gesture_now_seconds: 0.0,
                gesture_holds_available: true,
            },
        });

        assert_eq!(view.owner, KeyboardOwner::Numeric);
        assert!(matches!(
            &view.mode,
            ModeSurface::Numeric { entry, resume: None } if entry == "12"
        ));
        assert!(matches!(
            view.help,
            HelpSurface::Owner {
                owner: KeyboardOwner::Numeric,
                ..
            }
        ));
        assert!(view.help.text().starts_with("NUMERIC"));
    }

    #[test]
    fn numeric_entry_opened_inside_an_editor_keeps_that_editor_on_screen() {
        let address = ControlAddress::new("pad.level");
        let mut session = session();
        session.automation.open_or_create(address);
        // Second submenu row (`LfoField::Interval`, labelled `rate`).
        let model = InteractionModel {
            navigation: Navigation::default(),
            mode: InteractionMode::Numeric(NumericEntry {
                buffer: "42".to_string(),
                resume: Some(AutomationMode::Lfo {
                    depth: LfoDepth::Editor,
                    selected: 2,
                }),
            }),
            ..InteractionModel::default()
        };
        let frame = render_model_with_session(&model, &session);

        // The editor stays drawn instead of collapsing to the parent row...
        assert!(frame.contains("rate"), "editor collapsed:\n{frame}");
        // ...and the typed buffer lands on the drilled-down field, not the
        // control row above it.
        let entry_row = frame
            .lines()
            .find(|line| line.contains(">␠42"))
            .unwrap_or_else(|| panic!("numeric buffer missing:\n{frame}"));
        assert!(
            entry_row.contains("rate"),
            "buffer rendered off its field:\n{frame}"
        );
    }

    #[test]
    fn performance_sequence_shows_live_values_for_its_held_instrument() {
        let interaction = InteractionModel {
            mode: InteractionMode::Performance(PerformanceMode::Sequence {
                stage: SequenceStage::Perform {
                    instrument: PerformanceInstrument::Kick,
                },
                held_selector: Some(PerformanceInstrument::Kick),
            }),
            ..InteractionModel::default()
        };
        let mut session = session();
        session.controls.kick.interval_beats = 1.25;

        let rendered = render_model_with_session(&interaction, &session);

        assert!(
            rendered.contains("●␠d␠Kick") && rendered.contains("D█░░1.25b"),
            "Kick's live density is visible: {rendered:?}"
        );
    }

    /// Sequence rows use the app's colour language rather than the terminal
    /// terminal default and read as a different application. Snapshots only
    /// capture symbols, so only a colour assertion catches a regression.
    #[test]
    fn performance_sequence_rows_carry_the_apps_colour_language() {
        let interaction = InteractionModel {
            mode: InteractionMode::Performance(PerformanceMode::Sequence {
                stage: SequenceStage::Perform {
                    instrument: PerformanceInstrument::Kick,
                },
                held_selector: Some(PerformanceInstrument::Kick),
            }),
            ..InteractionModel::default()
        };
        let session = session();
        let fluid = RippleField::new();
        let flipped = FlippedUnits::new();
        let view = UiViewModel::project(ViewProjection {
            interaction: &interaction,
            session: &session,
            telemetry: TelemetryView::default(),
            presentation: ViewPresentation {
                fluid: &fluid,
                flipped: &flipped,
                cursor_visible: true,
                notices: ViewNotices::default(),
                gesture_now_seconds: 0.0,
                gesture_holds_available: true,
            },
        });
        let mut terminal =
            Terminal::new(TestBackend::new(MIN_TERMINAL_WIDTH, MIN_TERMINAL_HEIGHT)).unwrap();
        terminal.draw(|frame| render(frame, &view)).unwrap();

        let buffer = terminal.backend().buffer();
        let area = buffer.area;
        // Anchor on the held marker, not the name: "Kick" also appears in the
        // tab bar, which is styled independently.
        let row = (0..area.height)
            .find(|&y| {
                let text = (0..area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>();
                text.contains('●') && text.contains("Kick")
            })
            .expect("the held Kick row is drawn");
        // A held instrument is amber, distinct from focused cyan and idle
        // grey, so the player can see what their fingers are on.
        let held = (0..area.width)
            .map(|x| buffer[(x, row)].fg)
            .any(|fg| fg == Color::Rgb(255, 200, 90));
        assert!(
            held,
            "held Sequence row must be amber, not the terminal default"
        );
    }

    #[test]
    fn accepted_transition_is_visible_in_the_immediately_following_frame() {
        let transition = InteractionModel::default().update(SemanticAction {
            phase: InputPhase::Press,
            intent: Intent::OpenPalette,
        });
        assert!(transition.effects.is_empty());

        let frame = render_model(&transition.model);
        let browsing = render_mode(InteractionMode::Browsing);
        assert!(frame.contains("PALETTE"));
        assert!(frame.contains("/▌"));
        assert!(frame.contains("complete"));
        assert_ne!(frame, browsing);
    }

    #[test]
    fn active_gesture_keeps_release_and_quit_visible_at_minimum_width() {
        let mut session = session();
        session.gestures.press(GestureKind::Bloom, Tab::Chords, 0.0);

        let frame = render_model_with_session(&InteractionModel::default(), &session);
        assert!(frame.contains("Esc␠release␠·␠^Q␠quit"));
        assert!(frame.contains("Pads␠Bloom␠0%↑"));
    }

    #[test]
    fn idle_browsing_shows_general_shortcuts_and_gesture_hints_on_separate_rows() {
        let frame = render_model_with_session_at_size(
            &InteractionModel::default(),
            &session(),
            TelemetryView::default(),
            260,
            MIN_TERMINAL_HEIGHT,
        );
        assert!(
            frame.contains("jk␠select"),
            "general shortcuts must stay on the footer row: {frame}"
        );
        assert!(
            frame.contains("^Q␠quit"),
            "general shortcuts must stay on the footer row: {frame}"
        );
        assert!(
            frame.contains("z␠bloom"),
            "gesture key hints must show on the activity row when idle: {frame}"
        );
        assert!(
            frame.contains("x␠lift"),
            "gesture key hints must show on the activity row when idle: {frame}"
        );
    }

    #[test]
    fn lead_owner_renders_its_keyboard_at_the_minimum_frame() {
        let mut session = session();
        session.controls.lead.level = 0.5;
        session.controls.lead.octave = -1.0;
        let model = InteractionModel {
            navigation: Navigation::for_page(Page::Lead),
            mode: InteractionMode::Lead(LeadPlay {
                last_tone: Some(3),
                ..LeadPlay::default()
            }),
            ..InteractionModel::default()
        };
        let frame = render_model_with_session(&model, &session);
        assert!(frame.contains("LEAD"), "{frame}");
        assert!(frame.contains("arrows␠knobs"), "{frame}");
        assert!(frame.contains("a1␠"), "{frame}");
        assert!(frame.contains("oct␠-1"), "{frame}");

        session.controls.lead.level = 0.0;
        let silent = render_model_with_session(&model, &session);
        assert!(silent.contains("level␠is␠0"), "{silent}");
        assert!(silent.contains("arrows␠knobs"), "{silent}");
    }

    #[test]
    fn step_patterns_mark_the_transport_row_playing_now() {
        let mut session = session();
        session.controls.lead.rate_beats = 0.5;
        session.controls.lead.step_count = 4.0;
        let lead = InteractionModel {
            navigation: Navigation::Lead {
                selected: 2,
                drill: LeadDrill::Pattern { return_to: 0 },
            },
            mode: InteractionMode::Browsing,
            ..InteractionModel::default()
        };
        let lead_frame = render_model_with_session_at_size(
            &lead,
            &session,
            TelemetryView {
                beat: 1.0,
                active_chord: 0,
            },
            120,
            24,
        );
        assert!(lead_frame.contains("Lead␠›␠Pattern␠♪"), "{lead_frame}");
        let lead_step = lead_frame
            .lines()
            .find(|line| line.contains("Step␠3"))
            .unwrap_or_else(|| panic!("Lead step 3 missing:\n{lead_frame}"));
        assert!(
            lead_step.contains('♪'),
            "active Lead step missing badge: {lead_step}"
        );

        let address = ControlAddress::new("pad.level");
        let route = session.automation.open_or_create(address);
        route.shape = LfoShape::Steps;
        route.cycle_beats = 0.5;
        route.step_count = 4;
        let lfo = InteractionModel {
            navigation: Navigation::Master { selected: 0 },
            mode: InteractionMode::Automation(AutomationMode::Lfo {
                depth: LfoDepth::Editor,
                selected: 9,
            }),
            ..InteractionModel::default()
        };
        let lfo_frame = render_model_with_session_at_size(
            &lfo,
            &session,
            TelemetryView {
                beat: 1.0,
                active_chord: 0,
            },
            120,
            24,
        );
        let lfo_step = lfo_frame
            .lines()
            .find(|line| line.contains("step␠3"))
            .unwrap_or_else(|| panic!("LFO step 3 missing:\n{lfo_frame}"));
        assert!(
            lfo_step.contains('♪'),
            "active LFO step missing badge: {lfo_step}"
        );
    }
}
