//! Pure interaction state and transitions.
//!
//! This module names semantic UI behavior without depending on terminal
//! events, rendering, clocks, shared publication, or effect execution.

use super::Tab;
use super::palette::{PaletteEntry, PaletteState, StagedEdit};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum InputPhase {
    #[default]
    Press,
    Repeat,
    Release,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PhasePolicy {
    Edge,
    Repeatable,
    ReleaseOnly,
}

impl PhasePolicy {
    pub(crate) fn accepts(self, phase: InputPhase) -> bool {
        matches!(
            (self, phase),
            (Self::Edge | Self::Repeatable, InputPhase::Press)
                | (Self::Repeatable, InputPhase::Repeat)
                | (Self::ReleaseOnly, InputPhase::Release)
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Page {
    #[default]
    Chords,
    Perc,
    Bass,
    Kick,
    Tonal,
    Clap,
    Arp,
    Macros,
    Master,
}

impl Page {
    const ALL: [Self; 9] = [
        Self::Chords,
        Self::Perc,
        Self::Bass,
        Self::Kick,
        Self::Tonal,
        Self::Clap,
        Self::Arp,
        Self::Macros,
        Self::Master,
    ];

    fn next(self) -> Self {
        Self::ALL[(self as usize + 1) % Self::ALL.len()]
    }

    fn previous(self) -> Self {
        Self::ALL[(self as usize + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ChordDrill {
    #[default]
    None,
    Progression {
        return_to: usize,
    },
    Slot {
        slot: usize,
        return_to: usize,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum MasterDrill {
    #[default]
    None,
    Compression {
        return_to: usize,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StandardPage {
    Perc,
    Bass,
    Kick,
    Tonal,
    Clap,
    Arp,
    Macros,
}

impl StandardPage {
    fn page(self) -> Page {
        match self {
            Self::Perc => Page::Perc,
            Self::Bass => Page::Bass,
            Self::Kick => Page::Kick,
            Self::Tonal => Page::Tonal,
            Self::Clap => Page::Clap,
            Self::Arp => Page::Arp,
            Self::Macros => Page::Macros,
        }
    }
}

/// Page-local navigation makes a Chords drill on Master, or a Master drill on
/// any other page, impossible to construct.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Navigation {
    Chords { selected: usize, drill: ChordDrill },
    Standard { page: StandardPage, selected: usize },
    Master { selected: usize, drill: MasterDrill },
}

impl Default for Navigation {
    fn default() -> Self {
        Self::for_page(Page::Chords)
    }
}

impl Navigation {
    pub(crate) fn for_page(page: Page) -> Self {
        match page {
            Page::Chords => Self::Chords {
                selected: 0,
                drill: ChordDrill::None,
            },
            Page::Master => Self::Master {
                selected: 0,
                drill: MasterDrill::None,
            },
            Page::Perc => Self::Standard {
                page: StandardPage::Perc,
                selected: 0,
            },
            Page::Bass => Self::Standard {
                page: StandardPage::Bass,
                selected: 0,
            },
            Page::Kick => Self::Standard {
                page: StandardPage::Kick,
                selected: 0,
            },
            Page::Tonal => Self::Standard {
                page: StandardPage::Tonal,
                selected: 0,
            },
            Page::Clap => Self::Standard {
                page: StandardPage::Clap,
                selected: 0,
            },
            Page::Arp => Self::Standard {
                page: StandardPage::Arp,
                selected: 0,
            },
            Page::Macros => Self::Standard {
                page: StandardPage::Macros,
                selected: 0,
            },
        }
    }

    pub(crate) fn page(self) -> Page {
        match self {
            Self::Chords { .. } => Page::Chords,
            Self::Standard { page, .. } => page.page(),
            Self::Master { .. } => Page::Master,
        }
    }

    pub(crate) fn selected(self) -> usize {
        match self {
            Self::Chords { selected, .. }
            | Self::Standard { selected, .. }
            | Self::Master { selected, .. } => selected,
        }
    }

    fn move_selection(&mut self, delta: isize) {
        let selected = self.selected();
        let next = if delta.is_negative() {
            selected.saturating_sub(delta.unsigned_abs())
        } else {
            selected.saturating_add(delta as usize)
        };
        match self {
            Self::Chords { selected, .. }
            | Self::Standard { selected, .. }
            | Self::Master { selected, .. } => *selected = next,
        }
    }

    fn cancel_one_depth(&mut self) {
        match self {
            Self::Chords { selected, drill } => match *drill {
                ChordDrill::Slot { slot, return_to } => {
                    *selected = slot;
                    *drill = ChordDrill::Progression { return_to };
                }
                ChordDrill::Progression { return_to } => {
                    *selected = return_to;
                    *drill = ChordDrill::None;
                }
                ChordDrill::None => {}
            },
            Self::Master { selected, drill } => {
                if let MasterDrill::Compression { return_to } = *drill {
                    *selected = return_to;
                    *drill = MasterDrill::None;
                }
            }
            Self::Standard { .. } => {}
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct NumericEntry {
    pub(crate) buffer: String,
    pub(crate) resume: Option<AutomationMode>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PaletteMode {
    pub(crate) query: String,
    pub(crate) selected: usize,
    pub(crate) recent: Vec<&'static str>,
    pub(crate) locked: Option<usize>,
    pub(crate) value_buffer: String,
    pub(crate) staged: Vec<PaletteStagedEdit>,
    pub(crate) resume: Option<AutomationMode>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PaletteStagedEdit {
    pub(crate) id: &'static str,
    pub(crate) value_bits: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AutomationKind {
    Lfo,
    Envelope,
    Macro,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum LfoDepth {
    #[default]
    Editor,
    NestedField,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AutomationMode {
    Lfo { depth: LfoDepth, selected: usize },
    Envelope { selected: usize },
    Macro { selected: usize },
}

impl AutomationMode {
    fn new(kind: AutomationKind) -> Self {
        match kind {
            AutomationKind::Lfo => Self::Lfo {
                depth: LfoDepth::Editor,
                selected: 1,
            },
            AutomationKind::Envelope => Self::Envelope { selected: 1 },
            AutomationKind::Macro => Self::Macro { selected: 1 },
        }
    }

    fn kind(self) -> AutomationKind {
        match self {
            Self::Lfo { .. } => AutomationKind::Lfo,
            Self::Envelope { .. } => AutomationKind::Envelope,
            Self::Macro { .. } => AutomationKind::Macro,
        }
    }

    fn selected_mut(&mut self) -> &mut usize {
        match self {
            Self::Lfo { selected, .. } | Self::Envelope { selected } | Self::Macro { selected } => {
                selected
            }
        }
    }

    pub(crate) fn selected(self) -> usize {
        match self {
            Self::Lfo { selected, .. } | Self::Envelope { selected } | Self::Macro { selected } => {
                selected
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PerformanceKind {
    Deck,
    Sequence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PerformanceInstrument {
    Pads,
    Bass,
    Kick,
    Perc,
}

impl PerformanceInstrument {
    pub(crate) const ALL: [Self; 4] = [Self::Pads, Self::Bass, Self::Kick, Self::Perc];

    pub(crate) const fn index(self) -> usize {
        self as usize
    }

    pub(crate) const fn page(self) -> Page {
        match self {
            Self::Pads => Page::Chords,
            Self::Bass => Page::Bass,
            Self::Kick => Page::Kick,
            Self::Perc => Page::Perc,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PerformanceTargets(u8);

impl PerformanceTargets {
    pub(crate) fn single(instrument: PerformanceInstrument) -> Self {
        let mut targets = Self::default();
        targets.insert(instrument);
        targets
    }

    pub(crate) fn insert(&mut self, instrument: PerformanceInstrument) {
        self.0 |= 1 << instrument.index();
    }

    pub(crate) fn remove(&mut self, instrument: PerformanceInstrument) -> bool {
        let mask = 1 << instrument.index();
        let contained = self.0 & mask != 0;
        self.0 &= !mask;
        contained
    }

    pub(crate) fn contains(self, instrument: PerformanceInstrument) -> bool {
        self.0 & (1 << instrument.index()) != 0
    }

    pub(crate) fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub(crate) fn iter(self) -> impl Iterator<Item = PerformanceInstrument> {
        PerformanceInstrument::ALL
            .into_iter()
            .filter(move |instrument| self.contains(*instrument))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SequenceStage {
    #[default]
    ChooseInstrument,
    Perform {
        instrument: PerformanceInstrument,
    },
    AwaitActionRelease {
        instrument: PerformanceInstrument,
        action: PerformanceAction,
    },
    CompletedFallback {
        instrument: PerformanceInstrument,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PerformanceAction {
    Shorter,
    Longer,
    Quieter,
    Louder,
    Sparser,
    Denser,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PerformanceMode {
    Deck {
        selected: Option<PerformanceInstrument>,
        held_selectors: PerformanceTargets,
    },
    Sequence {
        stage: SequenceStage,
        held_selector: Option<PerformanceInstrument>,
    },
}

impl PerformanceMode {
    fn new(kind: PerformanceKind) -> Self {
        match kind {
            PerformanceKind::Deck => Self::Deck {
                selected: None,
                held_selectors: PerformanceTargets::default(),
            },
            PerformanceKind::Sequence => Self::Sequence {
                stage: SequenceStage::ChooseInstrument,
                held_selector: None,
            },
        }
    }

    fn kind(self) -> PerformanceKind {
        match self {
            Self::Deck { .. } => PerformanceKind::Deck,
            Self::Sequence { .. } => PerformanceKind::Sequence,
        }
    }
}

/// Exactly one variant owns the keyboard. Mode-local data cannot coexist with
/// another owner.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum InteractionMode {
    #[default]
    Browsing,
    Numeric(NumericEntry),
    Palette(PaletteMode),
    Automation(AutomationMode),
    Performance(PerformanceMode),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct InteractionModel {
    pub(crate) navigation: Navigation,
    pub(crate) mode: InteractionMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PageDirection {
    Next,
    Previous,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Intent {
    MoveSelection(isize),
    ChangePage(PageDirection),
    Cancel,
    EnterChordProgression,
    EnterChordSlot(usize),
    EnterMasterCompression,
    BeginNumeric(char),
    TypeCharacter(char),
    Backspace,
    PaletteAutocomplete,
    Confirm,
    OpenPalette,
    OpenAutomation(AutomationKind),
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "typed nested automation activation is kernel-tested"
        )
    )]
    OpenAutomationField,
    ActivatePerformance(PerformanceKind),
    SelectPerformanceInstrument {
        instrument: PerformanceInstrument,
        hold: bool,
    },
    ApplyPerformanceAction {
        action: PerformanceAction,
        release_available: bool,
    },
    FinishPerformanceSequence(PerformanceAction),
    AdjustSelected(i8),
    ResetSelected,
    /// Jump the selected control to the top of its range. Mirror of the
    /// reset gesture, which already owns the bottom.
    MaxSelected,
    ToggleAuto,
    ToggleUnits,
    ToggleMute {
        master: bool,
    },
    ToggleMacro,
    RemoveAutomation,
    ReseedAutomation,
    TouchSelected,
    CommitPaletteAtBar,
    Save,
    Quit,
    ReleaseHeldSelector(PerformanceInstrument),
}

impl Intent {
    pub(crate) fn phase_policy(self) -> PhasePolicy {
        match self {
            Self::MoveSelection(_)
            | Self::ChangePage(_)
            | Self::TypeCharacter(_)
            | Self::Backspace
            | Self::AdjustSelected(_)
            | Self::ApplyPerformanceAction { .. } => PhasePolicy::Repeatable,
            Self::ReleaseHeldSelector(_) | Self::FinishPerformanceSequence(_) => {
                PhasePolicy::ReleaseOnly
            }
            Self::Cancel
            | Self::EnterChordProgression
            | Self::EnterChordSlot(_)
            | Self::EnterMasterCompression
            | Self::BeginNumeric(_)
            | Self::PaletteAutocomplete
            | Self::Confirm
            | Self::OpenPalette
            | Self::OpenAutomation(_)
            | Self::OpenAutomationField
            | Self::ActivatePerformance(_)
            | Self::SelectPerformanceInstrument { .. }
            | Self::ResetSelected
            | Self::MaxSelected
            | Self::ToggleAuto
            | Self::ToggleUnits
            | Self::ToggleMute { .. }
            | Self::ToggleMacro
            | Self::RemoveAutomation
            | Self::ReseedAutomation
            | Self::TouchSelected
            | Self::CommitPaletteAtBar
            | Self::Save
            | Self::Quit => PhasePolicy::Edge,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SemanticAction {
    pub(crate) phase: InputPhase,
    pub(crate) intent: Intent,
}

impl SemanticAction {
    #[cfg(test)]
    pub(crate) fn press(intent: Intent) -> Self {
        Self {
            phase: InputPhase::Press,
            intent,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum InteractionEffect {
    AdjustSelected(i8),
    CommitNumeric(f32),
    PaletteJump {
        tab: Tab,
        index: usize,
        id: &'static str,
    },
    PaletteCommit(Vec<PaletteStagedEdit>),
    AutomationConfirm(AutomationKind),
    ResetSelected,
    MaxSelected,
    ToggleAuto,
    ToggleUnits,
    ToggleMute {
        master: bool,
    },
    ToggleMacro,
    RemoveAutomation,
    ReseedAutomation,
    CloseAutomationDepth,
    CloseAutomationAll,
    TouchSelected,
    PaletteCommitAtBar(Vec<PaletteStagedEdit>),
    SelectPage(Page),
    PerformanceInstrument(PerformanceInstrument),
    HoldPerformanceSelector(PerformanceInstrument),
    ReleaseHeldSelector(PerformanceInstrument),
    PerformanceEdit {
        targets: PerformanceTargets,
        focus: PerformanceInstrument,
        action: PerformanceAction,
    },
    Save,
    Quit,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Transition {
    pub(crate) model: InteractionModel,
    pub(crate) effects: Vec<InteractionEffect>,
}

impl InteractionModel {
    pub(crate) fn update_bounded(
        mut self,
        action: SemanticAction,
        automation_row_count: usize,
        item_count: usize,
    ) -> Transition {
        if !action.intent.phase_policy().accepts(action.phase) {
            return Transition {
                model: self,
                effects: Vec::new(),
            };
        }
        let Intent::MoveSelection(delta) = action.intent else {
            return self.update(action);
        };
        let InteractionMode::Automation(automation) = &mut self.mode else {
            return self.update(action);
        };
        match automation {
            AutomationMode::Lfo { selected, .. } => {
                *selected = move_unbounded(*selected, delta).clamp(1, automation_row_count.max(1));
                Transition {
                    model: self,
                    effects: Vec::new(),
                }
            }
            AutomationMode::Envelope { selected } | AutomationMode::Macro { selected } => {
                if delta.is_negative() && *selected <= 1 {
                    self.mode = InteractionMode::Browsing;
                    Transition {
                        model: self,
                        effects: vec![InteractionEffect::CloseAutomationAll],
                    }
                } else if !delta.is_negative() && *selected >= automation_row_count {
                    self.mode = InteractionMode::Browsing;
                    self.navigation.move_selection(1);
                    self.clamp_navigation_selection(item_count);
                    Transition {
                        model: self,
                        effects: vec![InteractionEffect::CloseAutomationAll],
                    }
                } else {
                    *selected = move_unbounded(*selected, delta);
                    Transition {
                        model: self,
                        effects: Vec::new(),
                    }
                }
            }
        }
    }

    pub(crate) fn clamp_navigation_selection(&mut self, item_count: usize) {
        let last = item_count.saturating_sub(1);
        match &mut self.navigation {
            Navigation::Chords { selected, .. }
            | Navigation::Standard { selected, .. }
            | Navigation::Master { selected, .. } => *selected = (*selected).min(last),
        }
    }

    pub(crate) fn seed_palette_recent(&mut self, recent: &[&'static str]) {
        if let InteractionMode::Palette(palette) = &mut self.mode
            && palette.recent.is_empty()
        {
            palette.recent.extend_from_slice(recent);
        }
    }

    pub(crate) fn automation_selected(&self) -> usize {
        match &self.mode {
            InteractionMode::Automation(mode) => mode.selected(),
            InteractionMode::Numeric(NumericEntry {
                resume: Some(mode), ..
            })
            | InteractionMode::Palette(PaletteMode {
                resume: Some(mode), ..
            }) => mode.selected(),
            _ => 0,
        }
    }

    pub(crate) fn apply_lfo_position(&mut self, depth: LfoDepth, selected: usize) {
        self.mode = InteractionMode::Automation(AutomationMode::Lfo { depth, selected });
    }

    pub(crate) fn select_control(&mut self, tab: Tab, index: usize) {
        self.navigation = match tab {
            Tab::Chords => {
                let (drill, selected) = super::chords_drill_for_index(index);
                Navigation::Chords { selected, drill }
            }
            Tab::Master => {
                let (drill, selected) = super::master_drill_for_index(index);
                Navigation::Master { selected, drill }
            }
            _ => {
                let mut navigation = Navigation::for_page(page_for_tab(tab));
                if let Navigation::Standard { selected, .. } = &mut navigation {
                    *selected = index;
                }
                navigation
            }
        };
    }

    pub(crate) fn update(mut self, action: SemanticAction) -> Transition {
        if !action.intent.phase_policy().accepts(action.phase) {
            return Transition {
                model: self,
                effects: Vec::new(),
            };
        }

        let mut effects = Vec::new();
        match &mut self.mode {
            InteractionMode::Browsing => {
                update_browsing(
                    &mut self.navigation,
                    action.intent,
                    &mut self.mode,
                    &mut effects,
                );
            }
            InteractionMode::Numeric(entry) => match action.intent {
                Intent::Cancel => {
                    self.mode = entry
                        .resume
                        .map_or(InteractionMode::Browsing, InteractionMode::Automation);
                }
                Intent::TypeCharacter(character) => push_numeric(&mut entry.buffer, character),
                Intent::Backspace => {
                    entry.buffer.pop();
                }
                Intent::Confirm => {
                    if let Ok(value) = entry.buffer.parse::<f32>() {
                        effects.push(InteractionEffect::CommitNumeric(value));
                    }
                    self.mode = entry
                        .resume
                        .map_or(InteractionMode::Browsing, InteractionMode::Automation);
                }
                Intent::MoveSelection(_)
                | Intent::ChangePage(_)
                | Intent::EnterChordProgression
                | Intent::EnterChordSlot(_)
                | Intent::EnterMasterCompression
                | Intent::BeginNumeric(_)
                | Intent::PaletteAutocomplete
                | Intent::OpenPalette
                | Intent::OpenAutomation(_)
                | Intent::OpenAutomationField
                | Intent::ActivatePerformance(_)
                | Intent::SelectPerformanceInstrument { .. }
                | Intent::ApplyPerformanceAction { .. }
                | Intent::FinishPerformanceSequence(_)
                | Intent::AdjustSelected(_)
                | Intent::ResetSelected
                | Intent::MaxSelected
                | Intent::ToggleAuto
                | Intent::ToggleUnits
                | Intent::ToggleMute { .. }
                | Intent::ToggleMacro
                | Intent::RemoveAutomation
                | Intent::ReseedAutomation
                | Intent::TouchSelected
                | Intent::CommitPaletteAtBar
                | Intent::Save
                | Intent::Quit
                | Intent::ReleaseHeldSelector(_) => {}
            },
            InteractionMode::Palette(palette) => {
                let entries = PaletteState::new(tab_for_page(self.navigation.page()), &[]);
                if palette
                    .locked
                    .is_some_and(|index| !entries.contains_entry(index))
                {
                    palette.locked = None;
                    palette.value_buffer.clear();
                }
                match action.intent {
                    Intent::Cancel => {
                        self.mode = palette
                            .resume
                            .map_or(InteractionMode::Browsing, InteractionMode::Automation);
                    }
                    Intent::TypeCharacter(character) => {
                        if palette.locked.is_some() {
                            push_numeric(&mut palette.value_buffer, character);
                        } else {
                            palette.query.push(character);
                            palette.selected = 0;
                        }
                    }
                    Intent::Backspace => {
                        if palette.locked.is_some() {
                            if palette.value_buffer.is_empty() {
                                palette.locked = None;
                            } else {
                                palette.value_buffer.pop();
                            }
                        } else {
                            palette.query.pop();
                            palette.selected = 0;
                        }
                    }
                    Intent::MoveSelection(delta) => {
                        let state = project_palette(palette, self.navigation.page());
                        if palette.locked.is_none() && !state.matches.is_empty() {
                            palette.selected = (state.selected as isize + delta)
                                .rem_euclid(state.matches.len() as isize)
                                as usize;
                        }
                    }
                    Intent::PaletteAutocomplete => {
                        let state = project_palette(palette, self.navigation.page());
                        if palette.locked.is_none()
                            && let Some(found) = state.matches.get(state.selected)
                        {
                            palette.locked = Some(found.entry);
                        }
                    }
                    Intent::Confirm => {
                        let state = project_palette(palette, self.navigation.page());
                        if let Some(entry_index) = palette.locked {
                            let entry = state.entry(entry_index);
                            if let Ok(value) = palette.value_buffer.parse::<f32>() {
                                palette.staged.push(PaletteStagedEdit {
                                    id: entry.spec.id,
                                    value_bits: value.to_bits(),
                                });
                                palette.locked = None;
                                palette.value_buffer.clear();
                                palette.query.clear();
                                palette.selected = 0;
                            } else {
                                effects.push(palette_jump(entry));
                                self.mode = InteractionMode::Browsing;
                            }
                        } else if !palette.staged.is_empty() && palette.query.is_empty() {
                            effects.push(InteractionEffect::PaletteCommit(std::mem::take(
                                &mut palette.staged,
                            )));
                            self.mode = palette
                                .resume
                                .map_or(InteractionMode::Browsing, InteractionMode::Automation);
                        } else if let Some(found) = state.matches.get(state.selected) {
                            effects.push(palette_jump(state.entry(found.entry)));
                            self.mode = InteractionMode::Browsing;
                        }
                    }
                    Intent::CommitPaletteAtBar => {
                        let state = project_palette(palette, self.navigation.page());
                        if let Some(entry_index) = palette.locked
                            && let Ok(value) = palette.value_buffer.parse::<f32>()
                        {
                            let entry = state.entry(entry_index);
                            palette.staged.push(PaletteStagedEdit {
                                id: entry.spec.id,
                                value_bits: value.to_bits(),
                            });
                        }
                        if !palette.staged.is_empty() {
                            effects.push(InteractionEffect::PaletteCommitAtBar(std::mem::take(
                                &mut palette.staged,
                            )));
                            self.mode = palette
                                .resume
                                .map_or(InteractionMode::Browsing, InteractionMode::Automation);
                        }
                    }
                    Intent::ChangePage(_)
                    | Intent::EnterChordProgression
                    | Intent::EnterChordSlot(_)
                    | Intent::EnterMasterCompression
                    | Intent::BeginNumeric(_)
                    | Intent::OpenPalette
                    | Intent::OpenAutomation(_)
                    | Intent::OpenAutomationField
                    | Intent::ActivatePerformance(_)
                    | Intent::SelectPerformanceInstrument { .. }
                    | Intent::ApplyPerformanceAction { .. }
                    | Intent::FinishPerformanceSequence(_)
                    | Intent::AdjustSelected(_)
                    | Intent::ResetSelected
                    | Intent::MaxSelected
                    | Intent::ToggleAuto
                    | Intent::ToggleUnits
                    | Intent::ToggleMute { .. }
                    | Intent::ToggleMacro
                    | Intent::RemoveAutomation
                    | Intent::ReseedAutomation
                    | Intent::TouchSelected
                    | Intent::Save
                    | Intent::Quit
                    | Intent::ReleaseHeldSelector(_) => {}
                }
            }
            InteractionMode::Automation(automation) => match action.intent {
                Intent::Cancel
                    if matches!(
                        automation,
                        AutomationMode::Lfo {
                            depth: LfoDepth::NestedField,
                            ..
                        }
                    ) =>
                {
                    effects.push(InteractionEffect::CloseAutomationDepth);
                    *automation = AutomationMode::Lfo {
                        depth: LfoDepth::Editor,
                        selected: 0,
                    };
                }
                Intent::Cancel => {
                    effects.push(InteractionEffect::CloseAutomationDepth);
                    self.mode = InteractionMode::Browsing;
                }
                Intent::OpenAutomationField => {
                    if matches!(automation, AutomationMode::Lfo { .. }) {
                        *automation = AutomationMode::Lfo {
                            depth: LfoDepth::NestedField,
                            selected: 0,
                        };
                    }
                }
                Intent::MoveSelection(delta) => {
                    let selected = automation.selected_mut();
                    *selected = move_unbounded(*selected, delta);
                }
                Intent::Confirm => {
                    effects.push(InteractionEffect::AutomationConfirm(automation.kind()));
                }
                Intent::ChangePage(direction) => {
                    let page = match direction {
                        PageDirection::Next => self.navigation.page().next(),
                        PageDirection::Previous => self.navigation.page().previous(),
                    };
                    self.navigation = Navigation::for_page(page);
                    self.mode = InteractionMode::Browsing;
                    effects.push(InteractionEffect::CloseAutomationAll);
                }
                Intent::OpenAutomation(kind) => {
                    let same = automation.kind() == kind;
                    effects.push(InteractionEffect::AutomationConfirm(kind));
                    self.mode = if same {
                        InteractionMode::Browsing
                    } else {
                        InteractionMode::Automation(AutomationMode::new(kind))
                    };
                }
                Intent::AdjustSelected(delta) => {
                    effects.push(InteractionEffect::AdjustSelected(delta));
                }
                Intent::ResetSelected => effects.push(InteractionEffect::ResetSelected),
                Intent::MaxSelected => effects.push(InteractionEffect::MaxSelected),
                Intent::ToggleAuto => effects.push(InteractionEffect::ToggleAuto),
                Intent::ToggleUnits => effects.push(InteractionEffect::ToggleUnits),
                Intent::ToggleMute { master } => {
                    effects.push(InteractionEffect::ToggleMute { master });
                }
                Intent::ToggleMacro => {
                    match automation {
                        AutomationMode::Lfo { depth, selected: _ } => {
                            *depth = match depth {
                                LfoDepth::Editor => LfoDepth::NestedField,
                                LfoDepth::NestedField => LfoDepth::Editor,
                            };
                        }
                        AutomationMode::Envelope { .. } => {
                            self.mode = InteractionMode::Automation(AutomationMode::new(
                                AutomationKind::Macro,
                            ));
                        }
                        AutomationMode::Macro { .. } => {
                            self.mode = InteractionMode::Browsing;
                        }
                    }
                    effects.push(InteractionEffect::ToggleMacro);
                }
                Intent::RemoveAutomation => {
                    self.mode = InteractionMode::Browsing;
                    effects.push(InteractionEffect::RemoveAutomation);
                }
                Intent::ReseedAutomation => effects.push(InteractionEffect::ReseedAutomation),
                Intent::BeginNumeric(character) => {
                    let mut entry = NumericEntry::default();
                    push_numeric(&mut entry.buffer, character);
                    entry.resume = Some(*automation);
                    self.mode = InteractionMode::Numeric(entry);
                }
                Intent::OpenPalette => {
                    self.mode = InteractionMode::Palette(PaletteMode {
                        recent: Vec::new(),
                        resume: Some(*automation),
                        ..PaletteMode::default()
                    });
                }
                Intent::Save => effects.push(InteractionEffect::Save),
                Intent::Quit => effects.push(InteractionEffect::Quit),
                Intent::TouchSelected => effects.push(InteractionEffect::TouchSelected),
                Intent::EnterChordProgression
                | Intent::EnterChordSlot(_)
                | Intent::EnterMasterCompression
                | Intent::TypeCharacter(_)
                | Intent::Backspace
                | Intent::PaletteAutocomplete
                | Intent::ActivatePerformance(_)
                | Intent::SelectPerformanceInstrument { .. }
                | Intent::ApplyPerformanceAction { .. }
                | Intent::FinishPerformanceSequence(_)
                | Intent::CommitPaletteAtBar
                | Intent::ReleaseHeldSelector(_) => {}
            },
            InteractionMode::Performance(performance) => match action.intent {
                Intent::Cancel => {
                    match performance {
                        PerformanceMode::Deck { held_selectors, .. } => {
                            effects.extend(
                                held_selectors
                                    .iter()
                                    .map(InteractionEffect::ReleaseHeldSelector),
                            );
                        }
                        PerformanceMode::Sequence { held_selector, .. } => {
                            if let Some(selector) = held_selector.take() {
                                effects.push(InteractionEffect::ReleaseHeldSelector(selector));
                            }
                        }
                    }
                    self.mode = InteractionMode::Browsing;
                }
                Intent::ActivatePerformance(kind) if kind == performance.kind() => {
                    if matches!(
                        performance,
                        PerformanceMode::Sequence {
                            stage: SequenceStage::CompletedFallback { .. },
                            ..
                        }
                    ) {
                        *performance = PerformanceMode::new(PerformanceKind::Sequence);
                    }
                }
                Intent::ActivatePerformance(_) => {}
                Intent::SelectPerformanceInstrument { instrument, hold } => {
                    match performance {
                        PerformanceMode::Deck {
                            selected,
                            held_selectors,
                        } => {
                            *selected = Some(instrument);
                            if hold {
                                held_selectors.insert(instrument);
                            }
                        }
                        PerformanceMode::Sequence {
                            stage,
                            held_selector,
                        } => {
                            if hold
                                && let Some(previous) = held_selector.replace(instrument)
                                && previous != instrument
                            {
                                effects.push(InteractionEffect::ReleaseHeldSelector(previous));
                            }
                            *stage = SequenceStage::Perform { instrument };
                        }
                    }
                    let page = instrument.page();
                    self.navigation = Navigation::for_page(page);
                    effects.extend([
                        InteractionEffect::SelectPage(page),
                        InteractionEffect::PerformanceInstrument(instrument),
                    ]);
                    if hold {
                        effects.push(InteractionEffect::HoldPerformanceSelector(instrument));
                    }
                }
                Intent::ReleaseHeldSelector(released) => match performance {
                    PerformanceMode::Deck { held_selectors, .. } => {
                        if held_selectors.remove(released) {
                            effects.push(InteractionEffect::ReleaseHeldSelector(released));
                        }
                    }
                    PerformanceMode::Sequence { held_selector, .. } => {
                        if *held_selector == Some(released) {
                            held_selector.take();
                            effects.push(InteractionEffect::ReleaseHeldSelector(released));
                        }
                    }
                },
                Intent::ApplyPerformanceAction {
                    action,
                    release_available,
                } => {
                    let edit = match *performance {
                        PerformanceMode::Deck {
                            selected: Some(instrument),
                            held_selectors,
                        } => Some((
                            if held_selectors.is_empty() {
                                PerformanceTargets::single(instrument)
                            } else {
                                held_selectors
                            },
                            instrument,
                        )),
                        PerformanceMode::Sequence {
                            stage: SequenceStage::Perform { instrument },
                            ..
                        } => Some((PerformanceTargets::single(instrument), instrument)),
                        _ => None,
                    };
                    if let Some((targets, focus)) = edit {
                        effects.push(InteractionEffect::PerformanceEdit {
                            targets,
                            focus,
                            action,
                        });
                        if let PerformanceMode::Sequence { stage, .. } = performance {
                            *stage = if release_available {
                                SequenceStage::AwaitActionRelease {
                                    instrument: focus,
                                    action,
                                }
                            } else {
                                SequenceStage::CompletedFallback { instrument: focus }
                            };
                        }
                    }
                }
                Intent::FinishPerformanceSequence(released) => {
                    if matches!(
                        performance,
                        PerformanceMode::Sequence {
                            stage: SequenceStage::AwaitActionRelease { action, .. },
                            ..
                        } if *action == released
                    ) {
                        let PerformanceMode::Sequence { held_selector, .. } = performance else {
                            unreachable!("completion is sequence-only");
                        };
                        if let Some(selector) = held_selector.take() {
                            effects.push(InteractionEffect::ReleaseHeldSelector(selector));
                        }
                        self.mode = InteractionMode::Browsing;
                    }
                }
                Intent::MoveSelection(_)
                | Intent::ChangePage(_)
                | Intent::EnterChordProgression
                | Intent::EnterChordSlot(_)
                | Intent::EnterMasterCompression
                | Intent::BeginNumeric(_)
                | Intent::TypeCharacter(_)
                | Intent::Backspace
                | Intent::PaletteAutocomplete
                | Intent::Confirm
                | Intent::OpenPalette
                | Intent::OpenAutomation(_)
                | Intent::OpenAutomationField
                | Intent::AdjustSelected(_)
                | Intent::ResetSelected
                | Intent::MaxSelected
                | Intent::ToggleAuto
                | Intent::ToggleUnits
                | Intent::ToggleMute { .. }
                | Intent::ToggleMacro
                | Intent::RemoveAutomation
                | Intent::ReseedAutomation
                | Intent::TouchSelected
                | Intent::CommitPaletteAtBar
                | Intent::Save
                | Intent::Quit => {}
            },
        }

        Transition {
            model: self,
            effects,
        }
    }
}

fn update_browsing(
    navigation: &mut Navigation,
    intent: Intent,
    mode: &mut InteractionMode,
    effects: &mut Vec<InteractionEffect>,
) {
    match intent {
        Intent::MoveSelection(delta) => navigation.move_selection(delta),
        Intent::ChangePage(direction) => {
            let page = match direction {
                PageDirection::Next => navigation.page().next(),
                PageDirection::Previous => navigation.page().previous(),
            };
            *navigation = Navigation::for_page(page);
        }
        Intent::Cancel => navigation.cancel_one_depth(),
        Intent::EnterChordProgression => {
            if let Navigation::Chords { selected, drill } = navigation {
                let return_to = *selected;
                *selected = 0;
                *drill = ChordDrill::Progression { return_to };
            }
        }
        Intent::EnterChordSlot(slot) => {
            if let Navigation::Chords { selected, drill } = navigation
                && let ChordDrill::Progression { return_to } = *drill
            {
                *selected = 0;
                *drill = ChordDrill::Slot { slot, return_to };
            }
        }
        Intent::EnterMasterCompression => {
            if let Navigation::Master { selected, drill } = navigation {
                let return_to = *selected;
                *selected = 0;
                *drill = MasterDrill::Compression { return_to };
            }
        }
        Intent::BeginNumeric(character) => {
            let mut entry = NumericEntry::default();
            push_numeric(&mut entry.buffer, character);
            *mode = InteractionMode::Numeric(entry);
        }
        Intent::OpenPalette => *mode = InteractionMode::Palette(PaletteMode::default()),
        Intent::OpenAutomation(kind) => {
            *mode = InteractionMode::Automation(AutomationMode::new(kind));
            effects.push(InteractionEffect::AutomationConfirm(kind));
        }
        Intent::ActivatePerformance(kind) => {
            *mode = InteractionMode::Performance(PerformanceMode::new(kind));
        }
        Intent::AdjustSelected(delta) => effects.push(InteractionEffect::AdjustSelected(delta)),
        Intent::ResetSelected => effects.push(InteractionEffect::ResetSelected),
        Intent::MaxSelected => effects.push(InteractionEffect::MaxSelected),
        Intent::ToggleAuto => effects.push(InteractionEffect::ToggleAuto),
        Intent::ToggleUnits => effects.push(InteractionEffect::ToggleUnits),
        Intent::ToggleMute { master } => {
            effects.push(InteractionEffect::ToggleMute { master });
        }
        Intent::ToggleMacro => {
            *mode = InteractionMode::Automation(AutomationMode::new(AutomationKind::Macro));
            effects.push(InteractionEffect::ToggleMacro);
        }
        Intent::RemoveAutomation => effects.push(InteractionEffect::RemoveAutomation),
        Intent::ReseedAutomation => effects.push(InteractionEffect::ReseedAutomation),
        Intent::TouchSelected => effects.push(InteractionEffect::TouchSelected),
        Intent::CommitPaletteAtBar => {}
        Intent::Save => effects.push(InteractionEffect::Save),
        Intent::Quit => effects.push(InteractionEffect::Quit),
        Intent::TypeCharacter(_)
        | Intent::Backspace
        | Intent::PaletteAutocomplete
        | Intent::Confirm
        | Intent::OpenAutomationField
        | Intent::SelectPerformanceInstrument { .. }
        | Intent::ApplyPerformanceAction { .. }
        | Intent::FinishPerformanceSequence(_)
        | Intent::ReleaseHeldSelector(_) => {}
    }
}

fn move_unbounded(selected: usize, delta: isize) -> usize {
    if delta.is_negative() {
        selected.saturating_sub(delta.unsigned_abs())
    } else {
        selected.saturating_add(delta as usize)
    }
}

fn push_numeric(buffer: &mut String, character: char) {
    let valid = character.is_ascii_digit()
        || (character == '.' && !buffer.contains('.'))
        || (character == '-' && buffer.is_empty());
    if valid {
        buffer.push(character);
    }
}

fn project_palette(palette: &PaletteMode, page: Page) -> PaletteState {
    let mut state = PaletteState::new(tab_for_page(page), &palette.recent);
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
    state
}

fn palette_jump(entry: &PaletteEntry) -> InteractionEffect {
    InteractionEffect::PaletteJump {
        tab: entry.tab,
        index: entry.index_in_tab,
        id: entry.spec.id,
    }
}

fn tab_for_page(page: Page) -> Tab {
    match page {
        Page::Chords => Tab::Chords,
        Page::Perc => Tab::Perc,
        Page::Bass => Tab::Bass,
        Page::Kick => Tab::Kick,
        Page::Tonal => Tab::Tonal,
        Page::Clap => Tab::Clap,
        Page::Arp => Tab::Arp,
        Page::Macros => Tab::Macros,
        Page::Master => Tab::Master,
    }
}

fn page_for_tab(tab: Tab) -> Page {
    match tab {
        Tab::Chords => Page::Chords,
        Tab::Perc => Page::Perc,
        Tab::Bass => Page::Bass,
        Tab::Kick => Page::Kick,
        Tab::Tonal => Page::Tonal,
        Tab::Clap => Page::Clap,
        Tab::Arp => Page::Arp,
        Tab::Macros => Page::Macros,
        Tab::Master => Page::Master,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(model: InteractionModel, intent: Intent) -> Transition {
        model.update(SemanticAction::press(intent))
    }

    #[test]
    fn every_keyboard_owner_cancels_one_depth() {
        let cases = [
            (
                InteractionMode::Numeric(NumericEntry {
                    buffer: "12".into(),
                    resume: None,
                }),
                InteractionMode::Browsing,
            ),
            (
                InteractionMode::Palette(PaletteMode::default()),
                InteractionMode::Browsing,
            ),
            (
                InteractionMode::Automation(AutomationMode::Lfo {
                    depth: LfoDepth::Editor,
                    selected: 2,
                }),
                InteractionMode::Browsing,
            ),
            (
                InteractionMode::Automation(AutomationMode::Lfo {
                    depth: LfoDepth::NestedField,
                    selected: 2,
                }),
                InteractionMode::Automation(AutomationMode::Lfo {
                    depth: LfoDepth::Editor,
                    selected: 0,
                }),
            ),
            (
                InteractionMode::Performance(PerformanceMode::Deck {
                    selected: Some(PerformanceInstrument::Kick),
                    held_selectors: PerformanceTargets::default(),
                }),
                InteractionMode::Browsing,
            ),
            (
                InteractionMode::Performance(PerformanceMode::Sequence {
                    stage: SequenceStage::ChooseInstrument,
                    held_selector: None,
                }),
                InteractionMode::Browsing,
            ),
        ];

        for (mode, expected) in cases {
            let model = InteractionModel {
                mode,
                ..InteractionModel::default()
            };
            assert_eq!(update(model, Intent::Cancel).model.mode, expected);
        }
    }

    #[test]
    fn browsing_cancel_walks_chord_drill_outward() {
        let model = InteractionModel {
            navigation: Navigation::Chords {
                selected: 3,
                drill: ChordDrill::Slot {
                    slot: 3,
                    return_to: 4,
                },
            },
            ..InteractionModel::default()
        };

        let progression = update(model, Intent::Cancel).model;
        assert_eq!(
            progression.navigation,
            Navigation::Chords {
                selected: 3,
                drill: ChordDrill::Progression { return_to: 4 },
            }
        );
        assert_eq!(
            update(progression, Intent::Cancel).model.navigation,
            Navigation::Chords {
                selected: 4,
                drill: ChordDrill::None,
            }
        );
    }

    #[test]
    fn page_change_resets_selection_and_page_local_drill() {
        let model = InteractionModel {
            navigation: Navigation::Chords {
                selected: 4,
                drill: ChordDrill::Progression { return_to: 4 },
            },
            ..InteractionModel::default()
        };

        assert_eq!(
            update(model, Intent::ChangePage(PageDirection::Previous))
                .model
                .navigation,
            Navigation::Master {
                selected: 0,
                drill: MasterDrill::None,
            }
        );
    }

    #[test]
    fn browsing_cancel_walks_master_drill_outward_and_then_stops() {
        let model = InteractionModel {
            navigation: Navigation::Master {
                selected: 2,
                drill: MasterDrill::Compression { return_to: 10 },
            },
            ..InteractionModel::default()
        };

        let browsing = update(model, Intent::Cancel).model;
        assert_eq!(
            browsing.navigation,
            Navigation::Master {
                selected: 10,
                drill: MasterDrill::None,
            }
        );
        assert_eq!(update(browsing.clone(), Intent::Cancel).model, browsing);
    }

    #[test]
    fn every_intent_declares_press_repeat_release_behavior() {
        let repeatable = [
            Intent::MoveSelection(1),
            Intent::ChangePage(PageDirection::Next),
            Intent::TypeCharacter('1'),
            Intent::Backspace,
            Intent::AdjustSelected(1),
        ];
        for intent in repeatable {
            assert_eq!(intent.phase_policy(), PhasePolicy::Repeatable);
        }

        let edge_triggered = [
            Intent::Cancel,
            Intent::EnterChordProgression,
            Intent::EnterChordSlot(0),
            Intent::EnterMasterCompression,
            Intent::BeginNumeric('1'),
            Intent::PaletteAutocomplete,
            Intent::Confirm,
            Intent::OpenPalette,
            Intent::OpenAutomation(AutomationKind::Lfo),
            Intent::OpenAutomationField,
            Intent::ActivatePerformance(PerformanceKind::Deck),
            Intent::SelectPerformanceInstrument {
                instrument: PerformanceInstrument::Pads,
                hold: false,
            },
            Intent::Save,
            Intent::Quit,
        ];
        for intent in edge_triggered {
            assert_eq!(intent.phase_policy(), PhasePolicy::Edge);
        }

        assert_eq!(
            Intent::ReleaseHeldSelector(PerformanceInstrument::Pads).phase_policy(),
            PhasePolicy::ReleaseOnly
        );
    }

    #[test]
    fn one_shot_actions_ignore_repeat_and_release() {
        for phase in [InputPhase::Repeat, InputPhase::Release] {
            let action = SemanticAction {
                phase,
                intent: Intent::ActivatePerformance(PerformanceKind::Deck),
            };
            assert_eq!(
                InteractionModel::default().update(action).model.mode,
                InteractionMode::Browsing
            );
        }
    }

    #[test]
    fn repeatable_actions_accept_press_and_repeat() {
        let mut model = InteractionModel::default();
        for phase in [InputPhase::Press, InputPhase::Repeat] {
            model = model
                .update(SemanticAction {
                    phase,
                    intent: Intent::MoveSelection(1),
                })
                .model;
        }
        assert_eq!(model.navigation.selected(), 2);
    }

    #[test]
    fn release_only_action_requires_and_ends_an_explicit_hold() {
        let model = update(
            InteractionModel::default(),
            Intent::ActivatePerformance(PerformanceKind::Deck),
        )
        .model;
        let pressed_release =
            model
                .clone()
                .update(SemanticAction::press(Intent::ReleaseHeldSelector(
                    PerformanceInstrument::Perc,
                )));
        assert!(pressed_release.effects.is_empty());

        let bare_release = model.clone().update(SemanticAction {
            phase: InputPhase::Release,
            intent: Intent::ReleaseHeldSelector(PerformanceInstrument::Perc),
        });
        assert!(bare_release.effects.is_empty());

        let held = update(
            model,
            Intent::SelectPerformanceInstrument {
                instrument: PerformanceInstrument::Perc,
                hold: true,
            },
        );
        assert_eq!(
            held.effects.last(),
            Some(&InteractionEffect::HoldPerformanceSelector(
                PerformanceInstrument::Perc
            ))
        );

        let released = held.model.update(SemanticAction {
            phase: InputPhase::Release,
            intent: Intent::ReleaseHeldSelector(PerformanceInstrument::Perc),
        });
        assert_eq!(
            released.effects,
            vec![InteractionEffect::ReleaseHeldSelector(
                PerformanceInstrument::Perc
            )]
        );
        assert!(
            released
                .model
                .update(SemanticAction {
                    phase: InputPhase::Release,
                    intent: Intent::ReleaseHeldSelector(PerformanceInstrument::Perc),
                })
                .effects
                .is_empty()
        );
    }

    #[test]
    fn cancel_releases_an_active_hold_before_leaving_performance() {
        let performance = update(
            InteractionModel::default(),
            Intent::ActivatePerformance(PerformanceKind::Sequence),
        )
        .model;
        let held = update(
            performance,
            Intent::SelectPerformanceInstrument {
                instrument: PerformanceInstrument::Perc,
                hold: true,
            },
        )
        .model;

        let cancelled = update(held, Intent::Cancel);
        assert_eq!(
            cancelled.effects,
            vec![InteractionEffect::ReleaseHeldSelector(
                PerformanceInstrument::Perc
            )]
        );
        assert_eq!(cancelled.model.mode, InteractionMode::Browsing);

        let late_release = cancelled.model.update(SemanticAction {
            phase: InputPhase::Release,
            intent: Intent::ReleaseHeldSelector(PerformanceInstrument::Perc),
        });
        assert!(late_release.effects.is_empty());
        assert_eq!(late_release.model.mode, InteractionMode::Browsing);
    }

    #[test]
    fn duplicate_performance_activation_is_idempotent() {
        for kind in [PerformanceKind::Deck, PerformanceKind::Sequence] {
            let first = update(
                InteractionModel::default(),
                Intent::ActivatePerformance(kind),
            );
            let second = update(first.model.clone(), Intent::ActivatePerformance(kind));
            assert_eq!(second.model, first.model);
            assert!(second.effects.is_empty());
        }
    }

    #[test]
    fn current_mode_rejects_actions_owned_by_other_modes() {
        let cases = [
            (
                InteractionMode::Numeric(NumericEntry::default()),
                Intent::OpenPalette,
            ),
            (
                InteractionMode::Numeric(NumericEntry::default()),
                Intent::ActivatePerformance(PerformanceKind::Deck),
            ),
            (
                InteractionMode::Palette(PaletteMode::default()),
                Intent::BeginNumeric('1'),
            ),
            (
                InteractionMode::Palette(PaletteMode::default()),
                Intent::OpenAutomation(AutomationKind::Lfo),
            ),
            (
                InteractionMode::Automation(AutomationMode::Macro { selected: 0 }),
                Intent::ActivatePerformance(PerformanceKind::Deck),
            ),
            (
                InteractionMode::Performance(PerformanceMode::Sequence {
                    stage: SequenceStage::ChooseInstrument,
                    held_selector: None,
                }),
                Intent::OpenAutomation(AutomationKind::Lfo),
            ),
            (
                InteractionMode::Performance(PerformanceMode::Deck {
                    selected: None,
                    held_selectors: PerformanceTargets::default(),
                }),
                Intent::ActivatePerformance(PerformanceKind::Sequence),
            ),
        ];

        for (mode, intent) in cases {
            let model = InteractionModel {
                mode: mode.clone(),
                ..InteractionModel::default()
            };
            let transition = update(model, intent);
            assert_eq!(transition.model.mode, mode);
            assert!(transition.effects.is_empty());
        }
    }

    #[test]
    fn numeric_entry_emits_only_valid_commits() {
        let valid = update(
            update(InteractionModel::default(), Intent::BeginNumeric('-')).model,
            Intent::TypeCharacter('2'),
        )
        .model;
        assert_eq!(
            update(valid, Intent::Confirm).effects,
            vec![InteractionEffect::CommitNumeric(-2.0)]
        );

        let invalid = update(
            update(InteractionModel::default(), Intent::BeginNumeric('.')).model,
            Intent::Confirm,
        );
        assert!(invalid.effects.is_empty());
        assert_eq!(invalid.model.mode, InteractionMode::Browsing);
    }

    fn palette_model(palette: PaletteMode) -> InteractionModel {
        InteractionModel {
            navigation: Navigation::Standard {
                page: StandardPage::Bass,
                selected: 0,
            },
            mode: InteractionMode::Palette(palette),
        }
    }

    #[test]
    fn palette_selection_wraps_projected_matches_and_freezes_while_locked() {
        let palette = PaletteMode {
            query: "bass".to_string(),
            ..PaletteMode::default()
        };
        let projected = project_palette(&palette, Page::Bass);
        assert!(projected.matches.len() > 1);

        let wrapped = update(palette_model(palette), Intent::MoveSelection(-1)).model;
        let InteractionMode::Palette(wrapped_palette) = wrapped.mode else {
            panic!("selection keeps palette open");
        };
        assert_eq!(wrapped_palette.selected, projected.matches.len() - 1);

        let locked = update(
            palette_model(PaletteMode {
                query: "bass".to_string(),
                selected: 1,
                ..PaletteMode::default()
            }),
            Intent::PaletteAutocomplete,
        )
        .model;
        let frozen = update(locked.clone(), Intent::MoveSelection(1)).model;
        assert_eq!(frozen, locked);
    }

    #[test]
    fn palette_confirm_stages_valid_locked_values_and_resets_for_next_edit() {
        let locked = update(
            palette_model(PaletteMode {
                query: "bass".to_string(),
                ..PaletteMode::default()
            }),
            Intent::PaletteAutocomplete,
        )
        .model;
        let typed = update(
            update(locked, Intent::TypeCharacter('4')).model,
            Intent::TypeCharacter('2'),
        )
        .model;
        let transition = update(typed, Intent::Confirm);
        assert!(transition.effects.is_empty());
        let InteractionMode::Palette(palette) = transition.model.mode else {
            panic!("staging a valid value keeps palette open");
        };
        assert_eq!(palette.query, "");
        assert_eq!(palette.selected, 0);
        assert_eq!(palette.locked, None);
        assert_eq!(palette.value_buffer, "");
        assert_eq!(palette.staged.len(), 1);
        assert_eq!(f32::from_bits(palette.staged[0].value_bits), 42.0);
    }

    #[test]
    fn palette_confirm_emits_typed_jump_for_locked_or_selected_control() {
        let base = PaletteMode {
            query: "bass".to_string(),
            selected: 1,
            ..PaletteMode::default()
        };
        let projected = project_palette(&base, Page::Bass);
        let expected = palette_jump(projected.entry(projected.matches[1].entry));

        let ordinary = update(palette_model(base.clone()), Intent::Confirm);
        assert_eq!(ordinary.effects, vec![expected.clone()]);
        assert_eq!(ordinary.model.mode, InteractionMode::Browsing);

        let locked = update(palette_model(base), Intent::PaletteAutocomplete).model;
        let invalid_locked = update(locked, Intent::Confirm);
        assert_eq!(invalid_locked.effects, vec![expected]);
        assert_eq!(invalid_locked.model.mode, InteractionMode::Browsing);
    }

    #[test]
    fn palette_confirm_commits_existing_stage_only_from_empty_query() {
        let staged = PaletteStagedEdit {
            id: "master.bpm",
            value_bits: 91.0f32.to_bits(),
        };
        let transition = update(
            palette_model(PaletteMode {
                staged: vec![staged.clone()],
                ..PaletteMode::default()
            }),
            Intent::Confirm,
        );
        assert_eq!(
            transition.effects,
            vec![InteractionEffect::PaletteCommit(vec![staged])]
        );
        assert_eq!(transition.model.mode, InteractionMode::Browsing);
    }

    #[test]
    fn palette_update_safely_unlocks_an_invalid_entry_index() {
        let transition = update(
            palette_model(PaletteMode {
                query: "bass".to_string(),
                locked: Some(usize::MAX),
                value_buffer: "42".to_string(),
                ..PaletteMode::default()
            }),
            Intent::MoveSelection(1),
        );
        assert!(transition.effects.is_empty());
        let InteractionMode::Palette(palette) = transition.model.mode else {
            panic!("selection keeps palette open");
        };
        assert_eq!(palette.locked, None);
        assert_eq!(palette.value_buffer, "");
        assert_eq!(palette.selected, 1);
    }

    #[test]
    fn performance_selection_emits_effects_in_order() {
        let model = update(
            InteractionModel::default(),
            Intent::ActivatePerformance(PerformanceKind::Sequence),
        )
        .model;
        let transition = update(
            model,
            Intent::SelectPerformanceInstrument {
                instrument: PerformanceInstrument::Perc,
                hold: false,
            },
        );

        assert_eq!(
            transition.effects,
            vec![
                InteractionEffect::SelectPage(Page::Perc),
                InteractionEffect::PerformanceInstrument(PerformanceInstrument::Perc),
            ]
        );
        assert_eq!(
            transition.model.navigation,
            Navigation::Standard {
                page: StandardPage::Perc,
                selected: 0,
            }
        );
    }

    #[test]
    fn deck_accumulates_held_selectors_without_releasing_the_previous_one() {
        let deck = update(
            InteractionModel::default(),
            Intent::ActivatePerformance(PerformanceKind::Deck),
        )
        .model;
        let pads = update(
            deck,
            Intent::SelectPerformanceInstrument {
                instrument: PerformanceInstrument::Pads,
                hold: true,
            },
        )
        .model;
        let bass = update(
            pads,
            Intent::SelectPerformanceInstrument {
                instrument: PerformanceInstrument::Bass,
                hold: true,
            },
        );
        assert_eq!(
            bass.effects,
            vec![
                InteractionEffect::SelectPage(Page::Bass),
                InteractionEffect::PerformanceInstrument(PerformanceInstrument::Bass),
                InteractionEffect::HoldPerformanceSelector(PerformanceInstrument::Bass),
            ]
        );
        assert!(matches!(
            bass.model.mode,
            InteractionMode::Performance(PerformanceMode::Deck {
                held_selectors,
                ..
            }) if held_selectors.contains(PerformanceInstrument::Pads)
                && held_selectors.contains(PerformanceInstrument::Bass)
        ));
    }

    #[test]
    fn performance_instruments_are_a_closed_four_choice_grammar() {
        let deck = update(
            InteractionModel::default(),
            Intent::ActivatePerformance(PerformanceKind::Deck),
        )
        .model;
        for instrument in [
            PerformanceInstrument::Pads,
            PerformanceInstrument::Bass,
            PerformanceInstrument::Kick,
            PerformanceInstrument::Perc,
        ] {
            let transition = update(
                deck.clone(),
                Intent::SelectPerformanceInstrument {
                    instrument,
                    hold: false,
                },
            );
            assert_eq!(transition.model.navigation.page(), instrument.page());
        }
    }

    #[test]
    fn identical_inputs_are_deterministic() {
        let model = InteractionModel::default();
        let action = SemanticAction::press(Intent::OpenAutomation(AutomationKind::Macro));
        assert_eq!(model.clone().update(action), model.update(action),);
    }
}
