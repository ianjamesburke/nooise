//! Immutable projection consumed by the Ratatui renderer.
//!
//! This is the only place that combines interaction ownership, one coherent
//! live-session generation, telemetry, and presentation-only UI state.

use super::*;
use crate::fluid::interaction::{
    AutomationMode, ChordDrill, InteractionMode, InteractionModel, MasterDrill, Navigation,
    PerformanceMode, SequenceStage,
};

pub(crate) const MIN_TERMINAL_WIDTH: u16 = 46;
pub(crate) const MIN_TERMINAL_HEIGHT: u16 = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeyboardOwner {
    Browsing,
    Numeric,
    Palette,
    Lfo,
    Envelope,
    Macro,
    PerformanceDeck,
    PerformanceSequence,
}

impl KeyboardOwner {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Browsing => "BROWSE",
            Self::Numeric => "NUMERIC",
            Self::Palette => "PALETTE",
            Self::Lfo => "LFO",
            Self::Envelope => "ENV",
            Self::Macro => "MACRO",
            Self::PerformanceDeck => "DECK",
            Self::PerformanceSequence => "SEQUENCE",
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
    pub(crate) comp_drill: MasterDrill,
    pub(crate) selected: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct TelemetryView {
    pub(crate) beat: f64,
    pub(crate) active_chord: u64,
}

pub(crate) struct ViewPresentation<'a> {
    pub(crate) fluid: &'a FluidState,
    pub(crate) flipped: &'a FlippedUnits,
    pub(crate) mute: &'a MuteState,
    pub(crate) cursor_visible: bool,
    pub(crate) notices: ViewNotices,
}

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
    pub(crate) fluid: &'a FluidState,
    pub(crate) flipped: &'a FlippedUnits,
    pub(crate) mute: &'a MuteState,
    pub(crate) cursor_visible: bool,
    pub(crate) help: HelpSurface,
}

pub(crate) enum ModeSurface<'a> {
    Browsing,
    Numeric { entry: String },
    Palette(PaletteSurface),
    Automation(AutomationSurface<'a>),
    Performance(PerformanceSurface),
}

pub(crate) struct PaletteSurface {
    pub(crate) state: PaletteState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AutomationUnavailable {
    NoOpenEditor,
    KindMismatch { active: ModKind },
    MissingRoute,
    DepthMismatch,
}

pub(crate) enum AutomationSurface<'a> {
    Lfo {
        depth: crate::fluid::interaction::LfoDepth,
        selected: usize,
        address: ControlAddress,
        route: &'a LfoRoute,
        state: &'a AutomationState,
    },
    Envelope {
        selected: usize,
        address: ControlAddress,
        route: &'a EnvelopeRoute,
    },
    Macro {
        selected: usize,
        address: ControlAddress,
        route: &'a MacroRoute,
    },
    Unavailable {
        requested: crate::fluid::interaction::AutomationKind,
        selected: usize,
        reason: AutomationUnavailable,
    },
}

impl AutomationSurface<'_> {
    pub(crate) fn selected(&self) -> usize {
        match self {
            Self::Lfo { selected, .. }
            | Self::Envelope { selected, .. }
            | Self::Macro { selected, .. }
            | Self::Unavailable { selected, .. } => *selected,
        }
    }

    pub(crate) fn active_address(&self) -> Option<ControlAddress> {
        match self {
            Self::Lfo { address, .. }
            | Self::Envelope { address, .. }
            | Self::Macro { address, .. } => Some(*address),
            Self::Unavailable { .. } => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PerformanceSurface {
    Deck {
        selected: Option<usize>,
        held_selector: Option<usize>,
    },
    SequenceChoose {
        held_selector: Option<usize>,
    },
    SequencePerform {
        instrument: Option<usize>,
        held_selector: Option<usize>,
    },
    SequenceComplete {
        instrument: Option<usize>,
        release_pending: bool,
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
        let items = match navigation.tab {
            Tab::Chords => chords_tab_controls(&session.controls, navigation.chord_drill),
            Tab::Master => master_tab_controls(&session.controls, navigation.comp_drill),
            tab => tab_controls(tab, &session.controls),
        };
        let mut navigation = navigation;
        navigation.selected = navigation.selected.min(items.len().saturating_sub(1));
        let owner = keyboard_owner(&interaction.mode);
        let mode = mode_surface(&interaction.mode, navigation.tab, &session.automation);
        let help = help_surface(owner, &mode, navigation, presentation.notices);

        Self {
            owner,
            mode,
            navigation,
            items,
            session,
            telemetry,
            fluid: presentation.fluid,
            flipped: presentation.flipped,
            mute: presentation.mute,
            cursor_visible: presentation.cursor_visible,
            help,
        }
    }
}

fn mode_surface<'a>(
    mode: &InteractionMode,
    tab: Tab,
    automation: &'a AutomationState,
) -> ModeSurface<'a> {
    match mode {
        InteractionMode::Browsing => ModeSurface::Browsing,
        InteractionMode::Numeric(entry) => ModeSurface::Numeric {
            entry: entry.buffer.clone(),
        },
        InteractionMode::Palette(palette) => {
            let mut state = PaletteState::new(tab, &palette.recent);
            for character in palette.query.chars() {
                state.push_char(character);
            }
            state.selected = palette.selected.min(state.matches.len().saturating_sub(1));
            state.locked = palette.locked.filter(|&index| state.contains_entry(index));
            if state.locked.is_some() {
                state.value_buf.clone_from(&palette.value_buffer);
            }
            state.staged = palette
                .staged
                .iter()
                .map(|edit| StagedEdit {
                    id: edit.id,
                    value: f32::from_bits(edit.value_bits),
                })
                .collect();
            ModeSurface::Palette(PaletteSurface { state })
        }
        InteractionMode::Automation(mode) => {
            ModeSurface::Automation(automation_surface(*mode, automation))
        }
        InteractionMode::Performance(PerformanceMode::Deck {
            selected,
            held_selector,
        }) => ModeSurface::Performance(PerformanceSurface::Deck {
            selected: selected.map(crate::fluid::interaction::PerformanceInstrument::index),
            held_selector: held_selector
                .map(crate::fluid::interaction::PerformanceInstrument::index),
        }),
        InteractionMode::Performance(PerformanceMode::Sequence {
            stage: SequenceStage::ChooseInstrument,
            held_selector,
        }) => ModeSurface::Performance(PerformanceSurface::SequenceChoose {
            held_selector: held_selector
                .map(crate::fluid::interaction::PerformanceInstrument::index),
        }),
        InteractionMode::Performance(PerformanceMode::Sequence {
            stage: SequenceStage::Perform { instrument },
            held_selector,
        }) => ModeSurface::Performance(PerformanceSurface::SequencePerform {
            instrument: Some(instrument.index()),
            held_selector: held_selector
                .map(crate::fluid::interaction::PerformanceInstrument::index),
        }),
        InteractionMode::Performance(PerformanceMode::Sequence {
            stage: SequenceStage::AwaitActionRelease { instrument, .. },
            ..
        }) => ModeSurface::Performance(PerformanceSurface::SequenceComplete {
            instrument: Some(instrument.index()),
            release_pending: true,
        }),
        InteractionMode::Performance(PerformanceMode::Sequence {
            stage: SequenceStage::CompletedFallback { instrument },
            ..
        }) => ModeSurface::Performance(PerformanceSurface::SequenceComplete {
            instrument: Some(instrument.index()),
            release_pending: false,
        }),
    }
}

fn automation_surface(mode: AutomationMode, automation: &AutomationState) -> AutomationSurface<'_> {
    let (requested, selected) = match mode {
        AutomationMode::Lfo { selected, .. } => {
            (crate::fluid::interaction::AutomationKind::Lfo, selected)
        }
        AutomationMode::Envelope { selected } => (
            crate::fluid::interaction::AutomationKind::Envelope,
            selected,
        ),
        AutomationMode::Macro { selected } => {
            (crate::fluid::interaction::AutomationKind::Macro, selected)
        }
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
    let expected = match requested {
        crate::fluid::interaction::AutomationKind::Lfo => ModKind::Lfo,
        crate::fluid::interaction::AutomationKind::Envelope => ModKind::Envelope,
        crate::fluid::interaction::AutomationKind::Macro => ModKind::Macro,
    };
    if active != expected {
        return AutomationSurface::Unavailable {
            requested,
            selected,
            reason: AutomationUnavailable::KindMismatch { active },
        };
    }

    match mode {
        AutomationMode::Lfo { depth, selected } => {
            let field_is_open = automation.field_macro_owner(address).is_some();
            let depth_matches = matches!(
                (depth, field_is_open),
                (crate::fluid::interaction::LfoDepth::Editor, false)
                    | (crate::fluid::interaction::LfoDepth::NestedField, true)
            );
            if !depth_matches {
                return AutomationSurface::Unavailable {
                    requested,
                    selected,
                    reason: AutomationUnavailable::DepthMismatch,
                };
            }
            automation.route(address).map_or(
                AutomationSurface::Unavailable {
                    requested,
                    selected,
                    reason: AutomationUnavailable::MissingRoute,
                },
                |route| AutomationSurface::Lfo {
                    depth,
                    selected,
                    address,
                    route,
                    state: automation,
                },
            )
        }
        AutomationMode::Envelope { selected } => automation.envelope(address).map_or(
            AutomationSurface::Unavailable {
                requested,
                selected,
                reason: AutomationUnavailable::MissingRoute,
            },
            |route| AutomationSurface::Envelope {
                selected,
                address,
                route,
            },
        ),
        AutomationMode::Macro { selected } => automation.macro_route(address).map_or(
            AutomationSurface::Unavailable {
                requested,
                selected,
                reason: AutomationUnavailable::MissingRoute,
            },
            |route| AutomationSurface::Macro {
                selected,
                address,
                route,
            },
        ),
    }
}

fn navigation_view(navigation: Navigation) -> NavigationView {
    match navigation {
        Navigation::Chords { selected, drill } => NavigationView {
            tab: Tab::Chords,
            chord_drill: drill,
            comp_drill: MasterDrill::None,
            selected,
        },
        Navigation::Standard { page, selected } => NavigationView {
            tab: match page {
                crate::fluid::interaction::StandardPage::Perc => Tab::Perc,
                crate::fluid::interaction::StandardPage::Bass => Tab::Bass,
                crate::fluid::interaction::StandardPage::Kick => Tab::Kick,
                crate::fluid::interaction::StandardPage::Tonal => Tab::Tonal,
                crate::fluid::interaction::StandardPage::Clap => Tab::Clap,
                crate::fluid::interaction::StandardPage::Arp => Tab::Arp,
                crate::fluid::interaction::StandardPage::Macros => Tab::Macros,
            },
            chord_drill: ChordDrill::None,
            comp_drill: MasterDrill::None,
            selected,
        },
        Navigation::Master { selected, drill } => NavigationView {
            tab: Tab::Master,
            chord_drill: ChordDrill::None,
            comp_drill: drill,
            selected,
        },
    }
}

pub(crate) fn keyboard_owner(mode: &InteractionMode) -> KeyboardOwner {
    match mode {
        InteractionMode::Browsing => KeyboardOwner::Browsing,
        InteractionMode::Numeric(_) => KeyboardOwner::Numeric,
        InteractionMode::Palette(_) => KeyboardOwner::Palette,
        InteractionMode::Automation(AutomationMode::Lfo { .. }) => KeyboardOwner::Lfo,
        InteractionMode::Automation(AutomationMode::Envelope { .. }) => KeyboardOwner::Envelope,
        InteractionMode::Automation(AutomationMode::Macro { .. }) => KeyboardOwner::Macro,
        InteractionMode::Performance(PerformanceMode::Deck { .. }) => {
            KeyboardOwner::PerformanceDeck
        }
        InteractionMode::Performance(PerformanceMode::Sequence { .. }) => {
            KeyboardOwner::PerformanceSequence
        }
    }
}

fn help_surface(
    owner: KeyboardOwner,
    mode: &ModeSurface<'_>,
    navigation: NavigationView,
    notices: ViewNotices,
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

    let local_help = match (
        navigation.tab,
        navigation.chord_drill,
        navigation.comp_drill,
    ) {
        (Tab::Chords, ChordDrill::Progression { .. }, _) => {
            Some("BROWSE · Progression   Enter: open chord   Esc: back".to_string())
        }
        (Tab::Chords, ChordDrill::Slot { slot, .. }, _) => {
            Some(format!("BROWSE · Chord {}   Esc: back", slot + 1))
        }
        (Tab::Master, _, MasterDrill::Compression { .. }) => {
            Some("BROWSE · Compression   Esc: back".to_string())
        }
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
        text:
            "BROWSE · jk select   h/l adjust   / find   f LFO   v macro   a auto   T units   q quit"
                .to_string(),
    }
}

fn owner_help(owner: KeyboardOwner, mode: &ModeSurface<'_>) -> String {
    match owner {
        KeyboardOwner::Browsing => unreachable!("browsing help is resolved separately"),
        KeyboardOwner::Numeric => "NUMERIC · type value   Enter: apply   Esc: cancel".to_string(),
        KeyboardOwner::Palette => {
            "PALETTE · type to find   Tab: complete   Enter: stage   Esc: cancel".to_string()
        }
        KeyboardOwner::Lfo | KeyboardOwner::Envelope | KeyboardOwner::Macro => match mode {
            ModeSurface::Automation(surface) => automation_owner_help(surface),
            _ => unreachable!("automation owner requires automation surface"),
        },
        KeyboardOwner::PerformanceDeck => match mode {
            ModeSurface::Performance(PerformanceSurface::Deck {
                selected,
                held_selector,
            }) => format!(
                "DECK · selected {}   held {}   a/s/d/f choose   h/l j/k u/i act   Esc",
                selector_text(*selected),
                selector_text(*held_selector)
            ),
            _ => unreachable!("deck owner requires deck mode"),
        },
        KeyboardOwner::PerformanceSequence => match mode {
            ModeSurface::Performance(PerformanceSurface::SequenceChoose { held_selector }) => {
                format!(
                    "SEQUENCE · a/s/d/f choose   held {}   Esc",
                    selector_text(*held_selector)
                )
            }
            ModeSurface::Performance(PerformanceSurface::SequencePerform {
                instrument,
                held_selector,
            }) => format!(
                "SEQUENCE · instrument {}   h/l j/k u/i act once   held {}   Esc",
                selector_text(*instrument),
                selector_text(*held_selector)
            ),
            ModeSurface::Performance(PerformanceSurface::SequenceComplete {
                instrument,
                release_pending,
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

fn selector_text(selector: Option<usize>) -> String {
    selector
        .and_then(|index| index.checked_add(1))
        .map_or_else(|| "none".to_string(), |index| index.to_string())
}

fn automation_owner_help(surface: &AutomationSurface<'_>) -> String {
    match surface {
        AutomationSurface::Lfo {
            depth,
            address,
            route,
            ..
        } => {
            let reseed = if route.shape.is_random() {
                "   r reseed"
            } else {
                ""
            };
            let text = format!(
                "LFO · {}   {}   {:.2} beats   depth {:.0}%{reseed}   x remove   Esc close",
                address.id(),
                route.shape.label(),
                route.cycle_beats,
                route.depth_ratio * 100.0
            );
            if *depth == crate::fluid::interaction::LfoDepth::NestedField {
                format!("LFO FIELD · {}", text.trim_start_matches("LFO · "))
            } else {
                text
            }
        }
        AutomationSurface::Envelope { address, route, .. } => format!(
            "ENV · {}   {}   amount {:+.0}%   x remove   Esc close",
            address.id(),
            route.field_display(EnvField::Trigger),
            route.amount * 100.0
        ),
        AutomationSurface::Macro { address, route, .. } => format!(
            "MACRO · {}   {}   x remove   Esc close",
            address.id(),
            route.summary()
        ),
        AutomationSurface::Unavailable {
            requested, reason, ..
        } => {
            let label = match requested {
                crate::fluid::interaction::AutomationKind::Lfo => "LFO",
                crate::fluid::interaction::AutomationKind::Envelope => "ENV",
                crate::fluid::interaction::AutomationKind::Macro => "MACRO",
            };
            let reason = match reason {
                AutomationUnavailable::NoOpenEditor => "editor unavailable",
                AutomationUnavailable::KindMismatch { .. } => "editor kind mismatch",
                AutomationUnavailable::MissingRoute => "route unavailable",
                AutomationUnavailable::DepthMismatch => "editor depth mismatch",
            };
            format!("{label} · {reason}   Esc: close")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fluid::interaction::{
        AutomationMode, InputPhase, Intent, LfoDepth, NumericEntry, PaletteMode,
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
        let fluid = FluidState::new();
        let flipped = FlippedUnits::new();
        let mute = [None; 9];
        let view = UiViewModel::project(ViewProjection {
            interaction,
            session,
            telemetry: TelemetryView::default(),
            presentation: ViewPresentation {
                fluid: &fluid,
                flipped: &flipped,
                mute: &mute,
                cursor_visible: true,
                notices: ViewNotices::default(),
            },
        });
        let mut terminal =
            Terminal::new(TestBackend::new(MIN_TERMINAL_WIDTH, MIN_TERMINAL_HEIGHT)).unwrap();
        terminal.draw(|frame| render(frame, &view)).unwrap();
        snapshot_symbols(terminal.backend().buffer())
    }

    fn render_model(interaction: &InteractionModel) -> String {
        let mut session = session();
        let address = ControlAddress::new("pad.level");
        match &interaction.mode {
            InteractionMode::Automation(AutomationMode::Lfo {
                depth: crate::fluid::interaction::LfoDepth::NestedField,
                ..
            }) => {
                session.automation.open_or_create(address);
                let key = unit_key("pad.level", Some("lfo.amount"));
                session.automation.toggle_open_field(key.clone());
                session
                    .automation
                    .field_macro_mut(&key)
                    .expect("opening a field creates its macro")
                    .amounts[0] = 0.25;
            }
            InteractionMode::Automation(AutomationMode::Lfo { .. }) => {
                session.automation.open_or_create(address);
            }
            InteractionMode::Automation(AutomationMode::Envelope { .. }) => {
                session.automation.open_or_create_envelope(address);
            }
            InteractionMode::Automation(AutomationMode::Macro { .. }) => {
                session.automation.open_or_create_macro(address);
            }
            _ => {}
        }
        render_model_with_session(interaction, &session)
    }

    fn render_mode(mode: InteractionMode) -> String {
        render_model(&InteractionModel {
            navigation: Navigation::default(),
            mode,
        })
    }

    fn minimum_snapshot(header: &str, value_row: &str, footer: &str) -> String {
        [
            header,
            "│␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠│",
            "│␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠│",
            "│[Chords]␠␠Perc␠␠Bass␠␠Kick␠␠Tonal␠␠Clap␠␠Arp│",
            "│␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠│",
            value_row,
            "│␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠│",
            "│␠␠Attack␠␠␠␠␠␠␠␠␠␠█████░░░░░␠6.00␠s␠␠␠␠␠␠␠␠␠│",
            footer,
            "└────────────────────────────────────────────┘",
        ]
        .join("\n")
    }

    fn palette_snapshot() -> String {
        let header = format!(
            "┌␠nooise␠v{}␠·␠PALETTE␠───────────────────┐",
            env!("CARGO_PKG_VERSION")
        );
        [
            header.as_str(),
            "│␠␠┌──────────────────────────────────────┐␠␠│",
            "│␠␠│/bass▌␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠│␠␠│",
            "│[C│␠␠bass.level␠·␠Bass␠·␠Level␠␠0%␠␠␠␠␠␠␠│rp│",
            "│␠␠│▸␠bass.cutoff␠·␠Bass␠·␠Cutoff␠␠8000␠Hz│␠␠│",
            "│▶␠│␠␠bass.attack_time␠·␠Bass␠·␠Attack␠␠10│␠␠│",
            "│␠␠│␠␠bass.decay_time␠·␠Bass␠·␠Decay␠␠300␠│␠␠│",
            "│␠␠│⇥␠complete␠␠␠type␠value␠␠␠↵␠stage/jump│␠␠│",
            "│PA└──────────────────────────────────────┘nt│",
            "└────────────────────────────────────────────┘",
        ]
        .join("\n")
    }

    fn owner_surface_snapshot(header: &str, body: [&str; 3], footer: &str) -> String {
        [
            header,
            "│␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠│",
            "│␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠│",
            "│[Chords]␠␠Perc␠␠Bass␠␠Kick␠␠Tonal␠␠Clap␠␠Arp│",
            "│␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠│",
            body[0],
            body[1],
            body[2],
            footer,
            "└────────────────────────────────────────────┘",
        ]
        .join("\n")
    }

    fn performance_snapshot(header: &str, body: [&str; 3], footer: &str) -> String {
        let row = |text: &str| {
            let encoded = text.replace(' ', "␠");
            format!(
                "│{encoded}{}│",
                "␠".repeat(44usize.saturating_sub(text.chars().count()))
            )
        };
        [
            header.to_string(),
            "│␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠│".to_string(),
            "│␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠│".to_string(),
            "│[Chords]␠␠Perc␠␠Bass␠␠Kick␠␠Tonal␠␠Clap␠␠Arp│".to_string(),
            "│␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠│".to_string(),
            row(body[0]),
            row(body[1]),
            row(body[2]),
            footer.to_string(),
            "└────────────────────────────────────────────┘".to_string(),
        ]
        .join("\n")
    }

    #[test]
    fn projection_reads_one_session_generation_and_clamps_navigation() {
        let mut session = session();
        session.generation = 42;
        session.controls.master.bpm = 91.0;
        let interaction = InteractionModel {
            navigation: Navigation::Master {
                selected: usize::MAX,
                drill: MasterDrill::None,
            },
            mode: InteractionMode::Browsing,
        };
        let fluid = FluidState::new();
        let flipped = FlippedUnits::new();
        let mute = [None; 9];
        let view = UiViewModel::project(ViewProjection {
            interaction: &interaction,
            session: &session,
            telemetry: TelemetryView::default(),
            presentation: ViewPresentation {
                fluid: &fluid,
                flipped: &flipped,
                mute: &mute,
                cursor_visible: false,
                notices: ViewNotices::default(),
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
            }),
        };
        let transition = model.update(SemanticAction::press(Intent::TypeCharacter('5')));
        let session = session();
        let fluid = FluidState::new();
        let flipped = FlippedUnits::new();
        let mute = [None; 9];
        let view = UiViewModel::project(ViewProjection {
            interaction: &transition.model,
            session: &session,
            telemetry: TelemetryView::default(),
            presentation: ViewPresentation {
                fluid: &fluid,
                flipped: &flipped,
                mute: &mute,
                cursor_visible: false,
                notices: ViewNotices::default(),
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
        let ModeSurface::Palette(palette) =
            mode_surface(&invalid_palette, Tab::Bass, &session.automation)
        else {
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
        };
        let fluid = FluidState::new();
        let flipped = FlippedUnits::new();
        let mute = [None; 9];
        let view = UiViewModel::project(ViewProjection {
            interaction: &interaction,
            session: &session,
            telemetry: TelemetryView::default(),
            presentation: ViewPresentation {
                fluid: &fluid,
                flipped: &flipped,
                mute: &mute,
                cursor_visible: false,
                notices: ViewNotices {
                    effect: Some("saved".to_string()),
                    pending_commit: Some("pending".to_string()),
                    auto: Some("auto".to_string()),
                    update: Some("update".to_string()),
                },
            },
        });

        assert_eq!(view.owner, KeyboardOwner::Numeric);
        assert!(matches!(
            &view.mode,
            ModeSurface::Numeric { entry } if entry == "12"
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
    fn lfo_mode_never_borrows_a_different_automation_editor() {
        fn unavailable_reason(automation: &AutomationState) -> AutomationUnavailable {
            match automation_surface(
                AutomationMode::Lfo {
                    depth: LfoDepth::Editor,
                    selected: 2,
                },
                automation,
            ) {
                AutomationSurface::Unavailable {
                    requested: crate::fluid::interaction::AutomationKind::Lfo,
                    selected: 2,
                    reason,
                } => reason,
                _ => panic!("mismatched LFO mode must project unavailable"),
            }
        }

        assert_eq!(
            unavailable_reason(&AutomationState::default()),
            AutomationUnavailable::NoOpenEditor
        );

        let address = ControlAddress::new("pad.level");
        let mut envelope = AutomationState::default();
        envelope.open_or_create_envelope(address);
        assert_eq!(
            unavailable_reason(&envelope),
            AutomationUnavailable::KindMismatch {
                active: ModKind::Envelope
            }
        );
        let lfo_model = InteractionModel {
            navigation: Navigation::default(),
            mode: InteractionMode::Automation(AutomationMode::Lfo {
                depth: LfoDepth::Editor,
                selected: 1,
            }),
        };
        let mut envelope_session = session();
        envelope_session.automation = envelope;
        let envelope_frame = render_model_with_session(&lfo_model, &envelope_session);
        assert!(envelope_frame.contains("editor␠kind␠mismatch"));
        assert!(!envelope_frame.contains("␠␠␠attack"));

        let mut macro_state = AutomationState::default();
        macro_state.open_or_create_macro(address);
        assert_eq!(
            unavailable_reason(&macro_state),
            AutomationUnavailable::KindMismatch {
                active: ModKind::Macro
            }
        );
        let mut macro_session = session();
        macro_session.automation = macro_state;
        let macro_frame = render_model_with_session(&lfo_model, &macro_session);
        assert!(macro_frame.contains("editor␠kind␠mismatch"));
        assert!(!macro_frame.contains("␠␠␠macro␠1"));

        let mut foreign_field = AutomationState::default();
        foreign_field.open_or_create(address);
        foreign_field.toggle_open_field(unit_key("bass.level", Some("lfo.amount")));
        let mut ineligible_field = AutomationState::default();
        ineligible_field.open_or_create(address);
        ineligible_field.toggle_open_field(unit_key("pad.level", Some("lfo.shape")));
        let macro_address = ControlAddress::new("macro.1");
        let mut macro_slider_field = AutomationState::default();
        macro_slider_field.open_or_create(macro_address);
        macro_slider_field.toggle_open_field(unit_key("macro.1", Some("lfo.amount")));
        for automation in [foreign_field, ineligible_field, macro_slider_field] {
            let surface = automation_surface(
                AutomationMode::Lfo {
                    depth: LfoDepth::NestedField,
                    selected: 0,
                },
                &automation,
            );
            assert!(matches!(
                surface,
                AutomationSurface::Unavailable {
                    reason: AutomationUnavailable::DepthMismatch,
                    ..
                }
            ));

            let mut mismatched_session = session();
            mismatched_session.automation = automation;
            let nested_model = InteractionModel {
                navigation: Navigation::default(),
                mode: InteractionMode::Automation(AutomationMode::Lfo {
                    depth: LfoDepth::NestedField,
                    selected: 0,
                }),
            };
            let frame = render_model_with_session(&nested_model, &mismatched_session);
            assert!(frame.contains("editor␠depth␠mismatch"));
            assert!(!frame.contains("LFO␠FIELD"));
        }

        let mut lfo = AutomationState::default();
        lfo.open_or_create(address);
        let surface = automation_surface(
            AutomationMode::Lfo {
                depth: LfoDepth::NestedField,
                selected: 0,
            },
            &lfo,
        );
        assert!(matches!(
            surface,
            AutomationSurface::Unavailable {
                reason: AutomationUnavailable::DepthMismatch,
                ..
            }
        ));
        assert_eq!(
            automation_owner_help(&surface),
            "LFO · editor depth mismatch   Esc: close"
        );
    }

    #[test]
    fn every_top_level_owner_has_a_full_minimum_size_snapshot() {
        const VALUE: &str = "│▶␠Level␠␠␠␠␠␠␠␠␠␠␠███████░░░␠70%␠␠␠␠␠␠␠␠␠␠␠␠│";
        const NUMERIC_VALUE: &str = "│▶␠Level␠␠␠␠␠␠␠␠␠␠␠███████░░░␠>␠12_␠␠␠␠␠␠␠␠␠␠│";
        let cases = [
            (
                "browsing",
                InteractionMode::Browsing,
                minimum_snapshot(
                    &format!(
                        "┌␠nooise␠v{}␠·␠BROWSE␠────────────────────┐",
                        env!("CARGO_PKG_VERSION")
                    ),
                    VALUE,
                    "│BROWSE␠·␠jk␠select␠␠␠h/l␠adjust␠␠␠/␠find␠␠␠f│",
                ),
            ),
            (
                "numeric",
                InteractionMode::Numeric(NumericEntry {
                    buffer: "12".to_string(),
                    resume: None,
                }),
                minimum_snapshot(
                    &format!(
                        "┌␠nooise␠v{}␠·␠NUMERIC␠───────────────────┐",
                        env!("CARGO_PKG_VERSION")
                    ),
                    NUMERIC_VALUE,
                    "│NUMERIC␠·␠type␠value␠␠␠Enter:␠apply␠␠␠Esc:␠c│",
                ),
            ),
            (
                "palette",
                InteractionMode::Palette(PaletteMode {
                    query: "bass".to_string(),
                    selected: 1,
                    ..PaletteMode::default()
                }),
                palette_snapshot(),
            ),
            (
                "automation_lfo",
                InteractionMode::Automation(AutomationMode::Lfo {
                    depth: LfoDepth::Editor,
                    selected: 1,
                }),
                owner_surface_snapshot(
                    &format!(
                        "┌␠nooise␠v{}␠·␠LFO␠───────────────────────┐",
                        env!("CARGO_PKG_VERSION")
                    ),
                    [
                        "│␠␠Level␠␠␠␠␠␠␠␠␠␠␠███████░░░␠70%␠␠␠␠␠␠␠␠␠␠␠␠│",
                        "│▶␠␠␠amount␠␠␠␠␠␠␠␠░░░░░░░░░░␠0%␠␠␠␠␠␠␠␠␠␠␠␠␠│",
                        "│␠␠␠␠rate␠␠␠␠␠␠␠␠␠␠████░░░░░░␠2.00␠beats␠␠␠␠␠│",
                    ],
                    "│LFO␠·␠pad.level␠␠␠sine␠␠␠2.00␠beats␠␠␠depth␠│",
                ),
            ),
            (
                "automation_lfo_nested",
                InteractionMode::Automation(AutomationMode::Lfo {
                    depth: LfoDepth::NestedField,
                    selected: 1,
                }),
                owner_surface_snapshot(
                    &format!(
                        "┌␠nooise␠v{}␠·␠LFO␠───────────────────────┐",
                        env!("CARGO_PKG_VERSION")
                    ),
                    [
                        "│␠␠Level␠␠␠␠␠␠␠␠␠␠␠███████░░░␠70%␠␠␠␠␠␠␠␠␠␠␠␠│",
                        "│▶␠␠␠amount␠␠␠␠␠␠␠␠░░░░░░░░░░␠0%␠␠␠␠␠␠␠␠␠␠␠␠␠│",
                        "│␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠⇒␠m1␠+25%␠␠␠␠␠␠␠␠␠␠␠␠␠␠␠│",
                    ],
                    "│LFO␠FIELD␠·␠pad.level␠␠␠sine␠␠␠2.00␠beats␠␠␠│",
                ),
            ),
            (
                "automation_envelope",
                InteractionMode::Automation(AutomationMode::Envelope { selected: 1 }),
                owner_surface_snapshot(
                    &format!(
                        "┌␠nooise␠v{}␠·␠ENV␠───────────────────────┐",
                        env!("CARGO_PKG_VERSION")
                    ),
                    [
                        "│␠␠Level␠␠␠␠␠␠␠␠␠␠␠███████░░░␠70%␠␠␠␠␠␠␠␠␠␠␠␠│",
                        "│▶␠␠␠amount␠␠␠␠␠␠␠␠█████░░░░░␠+0%␠␠␠␠␠␠␠␠␠␠␠␠│",
                        "│␠␠␠␠attack␠␠␠␠␠␠␠␠░░░░░░░░░░␠1.00␠beats␠␠␠␠␠│",
                    ],
                    "│ENV␠·␠pad.level␠␠␠every␠4␠beats␠␠␠amount␠+0%│",
                ),
            ),
            (
                "automation_macro",
                InteractionMode::Automation(AutomationMode::Macro { selected: 1 }),
                owner_surface_snapshot(
                    &format!(
                        "┌␠nooise␠v{}␠·␠MACRO␠─────────────────────┐",
                        env!("CARGO_PKG_VERSION")
                    ),
                    [
                        "│␠␠Level␠␠␠␠␠␠␠␠␠␠␠███████░░░␠70%␠␠␠␠␠␠␠␠␠␠␠␠│",
                        "│▶␠␠␠macro␠1␠␠␠␠␠␠␠█████░░░░░␠+0%␠␠␠␠␠␠␠␠␠␠␠␠│",
                        "│␠␠␠␠macro␠2␠␠␠␠␠␠␠█████░░░░░␠+0%␠␠␠␠␠␠␠␠␠␠␠␠│",
                    ],
                    "│MACRO␠·␠pad.level␠␠␠none␠␠␠x␠remove␠␠␠Esc␠cl│",
                ),
            ),
            (
                "performance_deck",
                InteractionMode::Performance(PerformanceMode::Deck {
                    selected: Some(PerformanceInstrument::Kick),
                    held_selector: Some(PerformanceInstrument::Perc),
                }),
                performance_snapshot(
                    &format!(
                        "┌␠nooise␠v{}␠·␠DECK␠──────────────────────┐",
                        env!("CARGO_PKG_VERSION")
                    ),
                    ["PERFORMANCE DECK", "selected · 3", "held · 4"],
                    "│DECK␠·␠selected␠3␠␠␠held␠4␠␠␠a/s/d/f␠choose␠│",
                ),
            ),
            (
                "performance_sequence_choose",
                InteractionMode::Performance(PerformanceMode::Sequence {
                    stage: SequenceStage::ChooseInstrument,
                    held_selector: Some(PerformanceInstrument::Bass),
                }),
                performance_snapshot(
                    &format!(
                        "┌␠nooise␠v{}␠·␠SEQUENCE␠──────────────────┐",
                        env!("CARGO_PKG_VERSION")
                    ),
                    [
                        "SEQUENCE · CHOOSE INSTRUMENT",
                        "instrument · waiting",
                        "held · 2",
                    ],
                    "│␠␠SEQUENCE␠·␠a/s/d/f␠choose␠␠␠held␠2␠␠␠Esc␠␠│",
                ),
            ),
            (
                "performance_sequence_perform",
                InteractionMode::Performance(PerformanceMode::Sequence {
                    stage: SequenceStage::Perform {
                        instrument: PerformanceInstrument::Kick,
                    },
                    held_selector: Some(PerformanceInstrument::Perc),
                }),
                performance_snapshot(
                    &format!(
                        "┌␠nooise␠v{}␠·␠SEQUENCE␠──────────────────┐",
                        env!("CARGO_PKG_VERSION")
                    ),
                    ["SEQUENCE · PERFORM", "instrument · 3", "held · 4"],
                    "│SEQUENCE␠·␠instrument␠3␠␠␠h/l␠j/k␠u/i␠act␠on│",
                ),
            ),
        ];

        for (name, mode, expected) in cases {
            let actual = render_mode(mode);
            if actual != expected {
                let mismatch = actual
                    .chars()
                    .zip(expected.chars())
                    .position(|(actual, expected)| actual != expected);
                panic!(
                    "snapshot changed for {name} at {mismatch:?}\nactual: {actual:?}\nexpected: {expected:?}"
                );
            }
        }
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
}
