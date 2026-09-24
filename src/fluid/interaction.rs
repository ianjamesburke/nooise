//! Pure interaction state and transitions.
//!
//! This module names semantic UI behavior without depending on terminal
//! events, rendering, clocks, shared publication, or effect execution.

use super::ModKind;
use super::Tab;
use super::palette::{ModuleScope, PaletteEntry, PaletteState, StagedEdit};
use super::{FluidControls, GESTURE_COUNT, GestureKind};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
/// Which edge of a physical key produced this event. A legacy terminal can
/// only report `Press`; `Repeat` and `Release` exist solely where keyboard
/// enhancement is negotiated.
pub(crate) enum InputPhase {
    #[default]
    Press,
    Repeat,
    Release,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Which phases an intent accepts, declared per `Intent` so a new variant
/// must decide before it can be dispatched. Checked *before* any transition
/// runs, so no transition function restates it.
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
    Lead,
    Master,
}

impl Page {
    fn next(self) -> Self {
        LAYERS[(self as usize + 1) % LAYERS.len()].page
    }

    fn previous(self) -> Self {
        LAYERS[(self as usize + LAYERS.len() - 1) % LAYERS.len()].page
    }
}

/// The navigation a layer opens on. Chords, Lead, and Master own page-local
/// navigation no other layer can construct; the rest share one standard list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LayerNavigation {
    Chords,
    Standard(StandardPage),
    Lead,
    Master,
}

/// One row of `LAYERS`: a page, the tab it shows, and the navigation opening
/// it creates. The table is indexed in `Page`/`Tab` discriminant order, which
/// is test-enforced, so page-tab translation stays a lookup rather than a
/// match that could drift.
struct Layer {
    page: Page,
    tab: Tab,
    navigation: LayerNavigation,
}

/// One row per layer in `Page`/`Tab` discriminant order (test-enforced), so
/// page order, page↔tab translation, and the navigation each page opens all
/// index this single table instead of restating the layer list.
const LAYERS: [Layer; 9] = [
    Layer {
        page: Page::Chords,
        tab: Tab::Chords,
        navigation: LayerNavigation::Chords,
    },
    Layer {
        page: Page::Perc,
        tab: Tab::Perc,
        navigation: LayerNavigation::Standard(StandardPage::Perc),
    },
    Layer {
        page: Page::Bass,
        tab: Tab::Bass,
        navigation: LayerNavigation::Standard(StandardPage::Bass),
    },
    Layer {
        page: Page::Kick,
        tab: Tab::Kick,
        navigation: LayerNavigation::Standard(StandardPage::Kick),
    },
    Layer {
        page: Page::Tonal,
        tab: Tab::Tonal,
        navigation: LayerNavigation::Standard(StandardPage::Tonal),
    },
    Layer {
        page: Page::Clap,
        tab: Tab::Clap,
        navigation: LayerNavigation::Standard(StandardPage::Clap),
    },
    Layer {
        page: Page::Arp,
        tab: Tab::Arp,
        navigation: LayerNavigation::Standard(StandardPage::Arp),
    },
    Layer {
        page: Page::Lead,
        tab: Tab::Lead,
        navigation: LayerNavigation::Lead,
    },
    Layer {
        page: Page::Master,
        tab: Tab::Master,
        navigation: LayerNavigation::Master,
    },
];

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

/// The Lead page's one drill: its step lane, opened from the Steps row.
/// `return_to` restores that row on Esc.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum LeadDrill {
    #[default]
    None,
    Pattern {
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
}

impl StandardPage {
    fn page(self) -> Page {
        LAYERS
            .iter()
            .find(|layer| layer.navigation == LayerNavigation::Standard(self))
            .expect("every standard page owns a layer row")
            .page
    }
}

/// Page-local navigation makes a Chords drill on Master, a Lead drill on
/// Chords, or a Master drill on any other page, impossible to construct.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Navigation {
    Chords {
        selected: usize,
        drill: ChordDrill,
    },
    Standard {
        page: StandardPage,
        selected: usize,
    },
    Lead {
        selected: usize,
        drill: LeadDrill,
    },
    Master {
        selected: usize,
    },
    /// A module owns a reusable detail scope while keeping its layer as the
    /// active page. `return_to` restores the collapsed module row on Esc.
    Module {
        tab: Tab,
        slot: usize,
        catalog_index: usize,
        selected: usize,
        return_to: usize,
    },
}

impl Default for Navigation {
    fn default() -> Self {
        Self::for_page(Page::Chords)
    }
}

impl Navigation {
    pub(crate) fn for_page(page: Page) -> Self {
        match LAYERS[page as usize].navigation {
            LayerNavigation::Chords => Self::Chords {
                selected: 0,
                drill: ChordDrill::None,
            },
            LayerNavigation::Standard(page) => Self::Standard { page, selected: 0 },
            LayerNavigation::Lead => Self::Lead {
                selected: 0,
                drill: LeadDrill::None,
            },
            LayerNavigation::Master => Self::Master { selected: 0 },
        }
    }

    pub(crate) fn page(self) -> Page {
        match self {
            Self::Chords { .. } => Page::Chords,
            Self::Standard { page, .. } => page.page(),
            Self::Lead { .. } => Page::Lead,
            Self::Master { .. } => Page::Master,
            Self::Module { tab, .. } => page_for_tab(tab),
        }
    }

    pub(crate) fn tab(self) -> Tab {
        tab_for_page(self.page())
    }

    pub(crate) fn selected(self) -> usize {
        match self {
            Self::Chords { selected, .. }
            | Self::Standard { selected, .. }
            | Self::Lead { selected, .. }
            | Self::Master { selected, .. }
            | Self::Module { selected, .. } => selected,
        }
    }

    fn selected_mut(&mut self) -> &mut usize {
        match self {
            Self::Chords { selected, .. }
            | Self::Standard { selected, .. }
            | Self::Lead { selected, .. }
            | Self::Master { selected, .. }
            | Self::Module { selected, .. } => selected,
        }
    }

    fn move_selection(&mut self, delta: isize) {
        let selected = self.selected_mut();
        *selected = selected.saturating_add_signed(delta);
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
            Self::Lead { selected, drill } => {
                if let LeadDrill::Pattern { return_to } = *drill {
                    *selected = return_to;
                    *drill = LeadDrill::None;
                }
            }
            Self::Module { tab, return_to, .. } => {
                let mut parent = Self::for_page(page_for_tab(*tab));
                match &mut parent {
                    Self::Chords { selected, .. }
                    | Self::Standard { selected, .. }
                    | Self::Lead { selected, .. }
                    | Self::Master { selected, .. } => *selected = *return_to,
                    Self::Module { .. } => {
                        unreachable!("page navigation cannot create a module drill")
                    }
                }
                *self = parent;
            }
            Self::Master { .. } | Self::Standard { .. } => {}
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
    pub(crate) module_scope: Option<ModuleScope>,
}

impl PaletteMode {
    /// The registry-backed palette view of this mode's query, selection, locked
    /// entry, and staged edits. Kernel confirm and the renderer both resolve
    /// rows by index through this one projection, so they cannot desync.
    pub(crate) fn project(&self, tab: Tab) -> PaletteState {
        let mut state = PaletteState::new(tab, &self.recent, self.module_scope);
        for character in self.query.chars() {
            state.push_char(character);
        }
        state.selected = self.selected.min(state.matches.len().saturating_sub(1));
        state.locked = self.locked.filter(|&index| state.contains_entry(index));
        if state.locked.is_some() {
            state.value_buf.clone_from(&self.value_buffer);
        }
        state.staged = self
            .staged
            .iter()
            .map(|edit| StagedEdit {
                id: edit.id,
                value: f32::from_bits(edit.value_bits),
            })
            .collect();
        state
    }
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
}

impl AutomationKind {
    /// Title-bar label for the editor family; `KeyboardOwner::label` and the
    /// unavailable-editor footer both read it.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Lfo => "LFO",
            Self::Envelope => "ENV",
        }
    }
}

impl From<AutomationKind> for ModKind {
    fn from(kind: AutomationKind) -> Self {
        match kind {
            AutomationKind::Lfo => Self::Lfo,
            AutomationKind::Envelope => Self::Envelope,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum LfoDepth {
    #[default]
    Editor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AutomationMode {
    Lfo { depth: LfoDepth, selected: usize },
    Envelope { selected: usize },
}

impl AutomationMode {
    fn new(kind: AutomationKind) -> Self {
        match kind {
            AutomationKind::Lfo => Self::Lfo {
                depth: LfoDepth::Editor,
                selected: 1,
            },
            AutomationKind::Envelope => Self::Envelope { selected: 1 },
        }
    }

    fn kind(self) -> AutomationKind {
        match self {
            Self::Lfo { .. } => AutomationKind::Lfo,
            Self::Envelope { .. } => AutomationKind::Envelope,
        }
    }

    fn selected_mut(&mut self) -> &mut usize {
        match self {
            Self::Lfo { selected, .. } | Self::Envelope { selected } => selected,
        }
    }

    pub(crate) fn selected(self) -> usize {
        match self {
            Self::Lfo { selected, .. } | Self::Envelope { selected } => selected,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PerformanceKind {
    Jump,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PerformanceInstrument {
    Pads,
    Perc,
    Bass,
    Kick,
    Tonal,
    Clap,
    Arp,
    Master,
}

/// Everything one performance instrument is: the page it lives on and the
/// selector key that reaches it from the Jump leader.
pub(crate) struct InstrumentRow {
    pub(crate) instrument: PerformanceInstrument,
    pub(crate) page: Page,
    pub(crate) key: char,
}

/// One row per instrument in `PerformanceInstrument` discriminant order
/// (test-enforced), so the key map, page, and display name all index this
/// single table.
///
/// The keys read left to right across the tab strip: the home row `asdf`
/// for the first four pages, the row above it `qwer` for the rest, so `r`
/// lands on Master. Lead has no selector key, since `i` already enters it
/// and the no-layer-key shorthand reaches it from its own page.
pub(crate) const INSTRUMENTS: [InstrumentRow; 8] = [
    InstrumentRow {
        instrument: PerformanceInstrument::Pads,
        page: Page::Chords,
        key: 'a',
    },
    InstrumentRow {
        instrument: PerformanceInstrument::Perc,
        page: Page::Perc,
        key: 's',
    },
    InstrumentRow {
        instrument: PerformanceInstrument::Bass,
        page: Page::Bass,
        key: 'd',
    },
    InstrumentRow {
        instrument: PerformanceInstrument::Kick,
        page: Page::Kick,
        key: 'f',
    },
    InstrumentRow {
        instrument: PerformanceInstrument::Tonal,
        page: Page::Tonal,
        key: 'q',
    },
    InstrumentRow {
        instrument: PerformanceInstrument::Clap,
        page: Page::Clap,
        key: 'w',
    },
    InstrumentRow {
        instrument: PerformanceInstrument::Arp,
        page: Page::Arp,
        key: 'e',
    },
    InstrumentRow {
        instrument: PerformanceInstrument::Master,
        page: Page::Master,
        key: 'r',
    },
];

impl PerformanceInstrument {
    pub(crate) const ALL: [Self; 8] = [
        Self::Pads,
        Self::Perc,
        Self::Bass,
        Self::Kick,
        Self::Tonal,
        Self::Clap,
        Self::Arp,
        Self::Master,
    ];

    pub(crate) const fn index(self) -> usize {
        self as usize
    }

    pub(crate) const fn row(self) -> &'static InstrumentRow {
        &INSTRUMENTS[self.index()]
    }

    pub(crate) const fn page(self) -> Page {
        self.row().page
    }

    pub(crate) fn tab(self) -> Tab {
        tab_for_page(self.page())
    }

    /// The selector key that chooses this instrument under the Jump leader.
    pub(crate) const fn key(self) -> char {
        self.row().key
    }

    pub(crate) fn from_key(key: char) -> Option<Self> {
        INSTRUMENTS
            .iter()
            .find(|row| row.key == key)
            .map(|row| row.instrument)
    }

    /// Display name, which is the owning tab's name.
    pub(crate) fn name(self) -> &'static str {
        self.tab().name()
    }
}

/// The catalog module `PerformanceParameter::Filter` reaches. Named once
/// here so the leader and the catalog cannot drift apart.
pub(crate) const FILTER_MODULE_ID: &str = "filter";

/// One parameter the Jump leader reaches, and the key that reaches it.
///
/// Arrival is the whole gesture: the leader moves the cursor onto the row
/// and hands the keyboard straight back to browsing, where the ordinary
/// `h`/`l` adjust it. Nothing here edits a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PerformanceParameter {
    Volume,
    Filter,
}

/// Everything one jump parameter is: the key that reaches it and the word
/// the footer shows. The runtime key map and the footer read this table
/// rather than restating the pair.
pub(crate) struct ParameterRow {
    pub(crate) parameter: PerformanceParameter,
    pub(crate) key: char,
    pub(crate) label: &'static str,
}

/// One row per parameter in `PerformanceParameter` discriminant order
/// (test-enforced). `j`/`k` keep the browse row's vim geometry.
pub(crate) const PARAMETERS: [ParameterRow; 2] = [
    ParameterRow {
        parameter: PerformanceParameter::Volume,
        key: 'j',
        label: "volume",
    },
    ParameterRow {
        parameter: PerformanceParameter::Filter,
        key: 'k',
        label: "filter",
    },
];

impl PerformanceParameter {
    pub(crate) const ALL: [Self; 2] = [Self::Volume, Self::Filter];

    pub(crate) const fn index(self) -> usize {
        self as usize
    }

    pub(crate) const fn row(self) -> &'static ParameterRow {
        &PARAMETERS[self.index()]
    }

    pub(crate) const fn key(self) -> char {
        self.row().key
    }

    pub(crate) const fn label(self) -> &'static str {
        self.row().label
    }

    pub(crate) fn from_key(key: char) -> Option<Self> {
        PARAMETERS
            .iter()
            .find(|row| row.key == key)
            .map(|row| row.parameter)
    }
}

/// Where a pending Jump is: waiting for a layer, or waiting for the
/// parameter on a layer already chosen. There is no third stage, because
/// the parameter key completes the leader and returns to browsing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum JumpStage {
    #[default]
    ChooseLayer,
    ChooseParameter {
        instrument: PerformanceInstrument,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PerformanceMode {
    Jump { stage: JumpStage },
}

impl PerformanceMode {
    fn new(kind: PerformanceKind) -> Self {
        match kind {
            PerformanceKind::Jump => Self::Jump {
                stage: JumpStage::ChooseLayer,
            },
        }
    }
}

/// A keyboard owner without its mode-local data, so an intent can name the
/// owners that act on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModeKind {
    Browsing,
    Numeric,
    Palette,
    Automation,
    Performance,
    Lead,
    Help,
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
    Lead(LeadPlay),
    /// The full keyboard-shortcut map, opened with `?` from Browsing. Carries
    /// no data: its content is static, so nothing needs projecting per frame.
    Help,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum GestureKeyState {
    #[default]
    Up,
    Active,
    Quarantined,
}

/// Physical gesture-key ownership is additive to the current keyboard mode.
/// A key released by Escape or a mode transition stays quarantined until its
/// real key-up arrives, so returning to Browse cannot retrigger it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct GestureInputState {
    keys: [GestureKeyState; GESTURE_COUNT],
}

impl GestureInputState {
    fn press(&mut self, kind: GestureKind) -> bool {
        let state = &mut self.keys[kind as usize];
        if *state != GestureKeyState::Up {
            return false;
        }
        *state = GestureKeyState::Active;
        true
    }

    fn release(&mut self, kind: GestureKind) -> bool {
        let state = &mut self.keys[kind as usize];
        let was_active = *state == GestureKeyState::Active;
        *state = GestureKeyState::Up;
        was_active
    }

    fn has_active(&self) -> bool {
        self.keys.contains(&GestureKeyState::Active)
    }

    fn quarantine_all(&mut self) {
        for state in &mut self.keys {
            if *state == GestureKeyState::Active {
                *state = GestureKeyState::Quarantined;
            }
        }
    }

    fn clear(&mut self) {
        self.keys.fill(GestureKeyState::Up);
    }
}

/// The letter row is the Lead keyboard: `a`–`l` play tones 1–9
/// (the four chord tones, the same four an octave up, the root two up). The
/// runtime maps keys through it and the view labels keys from it, so neither
/// restates the row.
pub(crate) const LEAD_PLAY_KEYS: [char; 9] = ['a', 's', 'd', 'f', 'g', 'h', 'j', 'k', 'l'];

/// A Lead nudge pair: two neighbouring keys that step one Lead row down and
/// up without leaving play mode. The runtime maps keys through the table,
/// the footer labels it, and the executor edits `id` through the registry,
/// so no one restates the pairs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LeadNudge {
    pub(crate) down: char,
    pub(crate) up: char,
    pub(crate) id: &'static str,
    pub(crate) label: &'static str,
}

impl LeadNudge {
    /// The row a key nudges and its direction, `-1` down or `1` up.
    pub(crate) fn from_key(key: char) -> Option<(Self, i8)> {
        LEAD_NUDGES.iter().find_map(|nudge| {
            if nudge.down == key {
                Some((*nudge, -1))
            } else if nudge.up == key {
                Some((*nudge, 1))
            } else {
                None
            }
        })
    }
}

/// Left key down, right key up: `z`/`x` for the octave and the top row for
/// the rows you reach for mid-phrase. Footer order, octave first so it
/// survives the narrowest frame.
pub(crate) const LEAD_NUDGES: [LeadNudge; 4] = [
    LeadNudge {
        down: 'z',
        up: 'x',
        id: "lead.octave",
        label: "oct",
    },
    LeadNudge {
        down: 'q',
        up: 'w',
        id: "lead.level",
        label: "lvl",
    },
    LeadNudge {
        down: 'e',
        up: 'r',
        id: "lead.decay",
        label: "dec",
    },
    LeadNudge {
        down: 't',
        up: 'y',
        id: "lead.glide",
        label: "gld",
    },
];

/// Lead play mode: the letter row plays chord tones on the Lead voice and
/// `LEAD_NUDGES` step its rows. Opened by Enter on the Lead page or `i`
/// from any page, closed by Esc.
/// `held` is the tone whose key is down, when the terminal reports
/// releases; `holds` remembers whether the last press could be held at
/// all, so the surface can say when a terminal cannot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct LeadPlay {
    pub(crate) last_tone: Option<usize>,
    pub(crate) held: Option<usize>,
    pub(crate) holds: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// The entire interaction state: where the cursor is, who owns the keyboard,
/// and which gesture keys have semantic physical ownership. It carries no
/// terminal capabilities, clock, or session data.
///
/// `update` is a deterministic pure function of this plus one action, and
/// emits ordered data-only effects for an adapter to execute. Two runs of the
/// same action sequence must produce identical models and identical effects;
/// the replay property tests assert exactly that.
pub(crate) struct InteractionModel {
    pub(crate) navigation: Navigation,
    pub(crate) mode: InteractionMode,
    pub(crate) gesture_input: GestureInputState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PageDirection {
    Next,
    Previous,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// The closed vocabulary of things a user can mean. Terminal keys map onto
/// these; nothing downstream of the mapper looks at a key again.
///
/// Every variant declares its accepted phases (`phase_policy`) and its owning
/// modes (`handled_by`), both exhaustive, so adding one forces both decisions.
pub(crate) enum Intent {
    MoveSelection(isize),
    ChangePage(PageDirection),
    Cancel,
    EnterChordProgression,
    EnterChordSlot(usize),
    /// Open the Lead's step lane from its Steps row.
    EnterLeadPattern,
    EnterModuleDetail {
        tab: Tab,
        slot: usize,
        catalog_index: usize,
    },
    BeginNumeric(char),
    TypeCharacter(char),
    Backspace,
    PaletteAutocomplete,
    Confirm,
    OpenPalette,
    /// Open the full keyboard-shortcut map.
    OpenHelp,
    OpenAutomation(AutomationKind),
    AddAutomation(AutomationKind),
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "typed nested automation activation is kernel-tested"
        )
    )]
    OpenAutomationField,
    ActivatePerformance(PerformanceKind),
    /// Choose the layer the pending Jump lands on. Opens its page so the
    /// parameter key has somewhere visible to arrive.
    SelectPerformanceInstrument {
        instrument: PerformanceInstrument,
    },
    /// Complete the pending Jump and hand the keyboard back to browsing.
    /// Without a layer key it aims at the page already open, so reaching a
    /// knob on the layer in front of you is two keys. Moves the cursor;
    /// never edits a value.
    JumpToParameter(PerformanceParameter),
    EnterLeadPlay,
    /// A 1-based Lead tone, from the letter row. `hold` is whether the
    /// terminal will report the key's release, so the note can sustain.
    PlayLeadTone {
        tone: usize,
        hold: bool,
    },
    /// The letter row key for a 1-based tone came up.
    ReleaseLeadTone(usize),
    /// Step one Lead row (`LEAD_NUDGES`) by `delta` dial steps.
    NudgeLead {
        id: &'static str,
        delta: i8,
    },
    /// Drop the lane out or bring it back (`lead.pattern` Off/Play) without
    /// leaving the keys.
    ToggleLeadPattern,
    /// Keep the phrase just played: it becomes the lane, and the lane plays.
    CaptureLeadPhrase,
    StartGesture(GestureKind),
    ReleaseGesture(GestureKind),
    ReleaseAllGestures,
    AbandonGestures,
    AdjustSelected(i8),
    ResetSelected,
    ToggleAuto,
    ToggleUnits,
    ToggleMute {
        master: bool,
    },
    /// Stop the beat clock so tails ring out, or start it again.
    ToggleTransport,
    RemoveAutomation,
    /// Roll the automation editor row under the cursor: one field, one step,
    /// or the parent control; a random shape's Shape row rerolls its seed.
    RandomizeAutomationRow,
    /// Set the selected control to a random point on its own dial.
    RandomizeSelected,
    /// Set every control in the open surface to a random point on its own dial.
    RandomizeScope,
    TouchSelected,
    CommitPaletteAtBar,
    Save,
    Quit,
}

impl Intent {
    /// The keyboard owners whose transition table acts on this intent; every
    /// other owner ignores it. Exhaustive over `Intent`, so a new variant
    /// cannot compile without deciding which modes own it.
    fn handled_by(self) -> &'static [ModeKind] {
        match self {
            Self::Cancel => &[
                ModeKind::Browsing,
                ModeKind::Numeric,
                ModeKind::Palette,
                ModeKind::Automation,
                ModeKind::Performance,
                ModeKind::Lead,
                ModeKind::Help,
            ],
            Self::MoveSelection(_) => &[
                ModeKind::Browsing,
                ModeKind::Palette,
                ModeKind::Automation,
                ModeKind::Lead,
            ],
            Self::Confirm => &[ModeKind::Numeric, ModeKind::Palette, ModeKind::Automation],
            Self::TypeCharacter(_) | Self::Backspace => &[ModeKind::Numeric, ModeKind::Palette],
            Self::PaletteAutocomplete | Self::CommitPaletteAtBar => &[ModeKind::Palette],
            Self::OpenAutomationField => &[ModeKind::Automation],
            Self::EnterChordProgression
            | Self::EnterChordSlot(_)
            | Self::EnterLeadPattern
            | Self::EnterModuleDetail { .. }
            | Self::EnterLeadPlay
            | Self::RandomizeSelected => &[ModeKind::Browsing],
            Self::RandomizeScope => &[ModeKind::Browsing, ModeKind::Automation],
            Self::AdjustSelected(_) => &[ModeKind::Browsing, ModeKind::Automation, ModeKind::Lead],
            Self::PlayLeadTone { .. }
            | Self::ReleaseLeadTone(_)
            | Self::NudgeLead { .. }
            | Self::ToggleLeadPattern
            | Self::CaptureLeadPhrase => &[ModeKind::Lead],
            Self::StartGesture(_) => &[ModeKind::Browsing],
            Self::OpenHelp => &[ModeKind::Browsing],
            Self::ReleaseGesture(_) | Self::ReleaseAllGestures | Self::AbandonGestures => &[
                ModeKind::Browsing,
                ModeKind::Numeric,
                ModeKind::Palette,
                ModeKind::Automation,
                ModeKind::Performance,
                ModeKind::Lead,
                ModeKind::Help,
            ],
            Self::ChangePage(_)
            | Self::BeginNumeric(_)
            | Self::OpenPalette
            | Self::OpenAutomation(_)
            | Self::AddAutomation(_)
            | Self::ResetSelected
            | Self::ToggleAuto
            | Self::ToggleUnits
            | Self::ToggleMute { .. }
            | Self::ToggleTransport
            | Self::RemoveAutomation
            | Self::RandomizeAutomationRow
            | Self::TouchSelected => &[ModeKind::Browsing, ModeKind::Automation],
            // Ctrl+S / Ctrl+Q reach every owner but the palette (its own
            // control chords) and numeric entry (swallows every chord).
            Self::Save | Self::Quit => &[
                ModeKind::Browsing,
                ModeKind::Automation,
                ModeKind::Performance,
                ModeKind::Lead,
                ModeKind::Help,
            ],
            Self::ActivatePerformance(_) => &[ModeKind::Browsing, ModeKind::Performance],
            Self::SelectPerformanceInstrument { .. } | Self::JumpToParameter(_) => {
                &[ModeKind::Performance]
            }
        }
    }

    fn is_handled_by(self, mode: ModeKind) -> bool {
        self.handled_by().contains(&mode)
    }

    pub(crate) fn phase_policy(self) -> PhasePolicy {
        match self {
            Self::MoveSelection(_)
            | Self::ChangePage(_)
            | Self::TypeCharacter(_)
            | Self::Backspace
            | Self::AdjustSelected(_) => PhasePolicy::Repeatable,
            Self::ReleaseLeadTone(_) | Self::ReleaseGesture(_) => PhasePolicy::ReleaseOnly,
            Self::Cancel
            | Self::EnterChordProgression
            | Self::EnterChordSlot(_)
            | Self::EnterLeadPattern
            | Self::EnterModuleDetail { .. }
            | Self::BeginNumeric(_)
            | Self::PaletteAutocomplete
            | Self::Confirm
            | Self::OpenPalette
            | Self::OpenHelp
            | Self::OpenAutomation(_)
            | Self::AddAutomation(_)
            | Self::OpenAutomationField
            | Self::ActivatePerformance(_)
            | Self::SelectPerformanceInstrument { .. }
            | Self::JumpToParameter(_)
            | Self::EnterLeadPlay
            | Self::PlayLeadTone { .. }
            | Self::NudgeLead { .. }
            | Self::ToggleLeadPattern
            | Self::CaptureLeadPhrase
            | Self::StartGesture(_)
            | Self::ReleaseAllGestures
            | Self::AbandonGestures
            | Self::ResetSelected
            | Self::ToggleAuto
            | Self::ToggleUnits
            | Self::ToggleMute { .. }
            | Self::ToggleTransport
            | Self::RemoveAutomation
            | Self::RandomizeAutomationRow
            | Self::RandomizeSelected
            | Self::RandomizeScope
            | Self::TouchSelected
            | Self::CommitPaletteAtBar
            | Self::Save
            | Self::Quit => PhasePolicy::Edge,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// One intent at one phase — the only input the kernel accepts.
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
/// A data-only consequence of a transition, for an adapter to execute.
///
/// Effects carry their complete typed payload so no adapter has to
/// reconstruct what the kernel already knew. They are ordered and execution
/// stops at the first failure; an executor that does not implement one must
/// reject it explicitly rather than drop it.
pub(crate) enum InteractionEffect {
    AdjustSelected(i8),
    CommitNumeric(f32),
    /// Put the cursor on control `id`, which is row `index` of `tab`.
    JumpToControl {
        tab: Tab,
        index: usize,
        id: &'static str,
    },
    PaletteCommit(Vec<PaletteStagedEdit>),
    /// Put catalog module `catalog_index` on `tab`'s chain, or jump to it when the
    /// chain already holds it. The kernel cannot tell which, so it says what
    /// was asked for and lets the adapter resolve it.
    PlaceModule {
        tab: Tab,
        catalog_index: usize,
    },
    AutomationConfirm(AutomationKind),
    AddAutomation(AutomationKind),
    ResetSelected,
    ToggleAuto,
    ToggleUnits,
    ToggleMute {
        master: bool,
    },
    ToggleTransport,
    RemoveAutomation,
    RandomizeAutomationRow,
    RandomizeSelected,
    RandomizeScope,
    CloseAutomationAll,
    TouchSelected,
    PaletteCommitAtBar(Vec<PaletteStagedEdit>),
    SelectPage(Page),
    /// Sound one Lead tone (1-based) now; a held tone sustains until
    /// `LeadRelease`.
    LeadTone {
        tone: usize,
        hold: bool,
    },
    /// Let the held Lead tone go.
    LeadRelease,
    /// Step one Lead row by `delta` dial steps.
    LeadNudge {
        id: &'static str,
        delta: i8,
    },
    /// Flip `lead.pattern` between Off and Play.
    LeadPattern,
    /// Write the phrase just played into the lane and set it playing.
    LeadCapture,
    GesturePress {
        kind: GestureKind,
        tab: Tab,
    },
    GestureRelease(GestureKind),
    GestureReleaseAll,
    Save,
    Quit,
}

#[derive(Clone, Debug, PartialEq)]
/// The result of one `update`: the next model, and the effects that must run
/// in this order for the model to be true.
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
                *selected = selected
                    .saturating_add_signed(delta)
                    .clamp(1, automation_row_count.max(1));
                Transition {
                    model: self,
                    effects: Vec::new(),
                }
            }
            AutomationMode::Envelope { selected } => {
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
                    *selected = selected.saturating_add_signed(delta);
                    Transition {
                        model: self,
                        effects: Vec::new(),
                    }
                }
            }
        }
    }

    pub(crate) fn clamp_navigation_selection(&mut self, item_count: usize) {
        let selected = self.navigation.selected_mut();
        *selected = (*selected).min(item_count.saturating_sub(1));
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

    pub(crate) fn select_control(&mut self, tab: Tab, index: usize, controls: &FluidControls) {
        self.navigation = match tab {
            Tab::Chords => {
                let (drill, selected) = super::chords_drill_for_index(index, controls);
                Navigation::Chords { selected, drill }
            }
            Tab::Lead => {
                let (drill, selected) = super::lead_drill_for_index(index, controls);
                Navigation::Lead { selected, drill }
            }
            Tab::Master => Navigation::Master { selected: index },
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

        let intent = action.intent;
        if let Intent::ReleaseGesture(kind) = intent {
            let effects = self
                .gesture_input
                .release(kind)
                .then_some(InteractionEffect::GestureRelease(kind))
                .into_iter()
                .collect();
            return Transition {
                model: self,
                effects,
            };
        }
        if matches!(intent, Intent::ReleaseAllGestures | Intent::AbandonGestures) {
            if intent == Intent::AbandonGestures {
                self.gesture_input.clear();
            } else {
                self.gesture_input.quarantine_all();
            }
            return Transition {
                model: self,
                effects: vec![InteractionEffect::GestureReleaseAll],
            };
        }
        if intent == Intent::Cancel
            && matches!(self.mode, InteractionMode::Browsing)
            && self.gesture_input.has_active()
        {
            self.gesture_input.quarantine_all();
            return Transition {
                model: self,
                effects: vec![InteractionEffect::GestureReleaseAll],
            };
        }
        let page = self.navigation.page();
        let mut effects = Vec::new();
        let mut next_mode = None;
        match &mut self.mode {
            InteractionMode::Browsing => {
                update_browsing(
                    &mut self.navigation,
                    &mut self.gesture_input,
                    intent,
                    &mut next_mode,
                    &mut effects,
                );
            }
            InteractionMode::Numeric(entry) => {
                update_numeric(entry, intent, &mut next_mode, &mut effects);
            }
            InteractionMode::Palette(palette) => {
                update_palette(palette, page, intent, &mut next_mode, &mut effects);
            }
            InteractionMode::Automation(automation) => update_automation(
                automation,
                &mut self.navigation,
                intent,
                &mut next_mode,
                &mut effects,
            ),
            InteractionMode::Performance(performance) => update_performance(
                performance,
                &mut self.navigation,
                intent,
                &mut next_mode,
                &mut effects,
            ),
            InteractionMode::Lead(play) => update_lead(
                play,
                &mut self.navigation,
                intent,
                &mut next_mode,
                &mut effects,
            ),
            InteractionMode::Help => update_help(intent, &mut next_mode, &mut effects),
        }
        if self.gesture_input.has_active()
            && next_mode
                .as_ref()
                .is_some_and(|mode| !matches!(mode, InteractionMode::Browsing))
            && matches!(self.mode, InteractionMode::Browsing)
        {
            self.gesture_input.quarantine_all();
            effects.insert(0, InteractionEffect::GestureReleaseAll);
        }
        if let Some(mode) = next_mode {
            self.mode = mode;
        }

        Transition {
            model: self,
            effects,
        }
    }
}

/// The editor a nested mode was opened from, or browsing when it was opened
/// from the top level.
fn resume_mode(resume: Option<AutomationMode>) -> InteractionMode {
    resume.map_or(InteractionMode::Browsing, InteractionMode::Automation)
}

fn update_browsing(
    navigation: &mut Navigation,
    gesture_input: &mut GestureInputState,
    intent: Intent,
    next_mode: &mut Option<InteractionMode>,
    effects: &mut Vec<InteractionEffect>,
) {
    if !intent.is_handled_by(ModeKind::Browsing) {
        return;
    }
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
        Intent::EnterLeadPattern => {
            if let Navigation::Lead { selected, drill } = navigation {
                let return_to = *selected;
                *selected = 0;
                *drill = LeadDrill::Pattern { return_to };
            }
        }
        Intent::EnterModuleDetail {
            tab,
            slot,
            catalog_index,
        } => {
            let return_to = navigation.selected();
            *navigation = Navigation::Module {
                tab,
                slot,
                catalog_index,
                selected: 0,
                return_to,
            };
        }
        Intent::BeginNumeric(character) => {
            let mut entry = NumericEntry::default();
            push_numeric(&mut entry.buffer, character);
            *next_mode = Some(InteractionMode::Numeric(entry));
        }
        Intent::OpenPalette => {
            *next_mode = Some(InteractionMode::Palette(PaletteMode {
                module_scope: match navigation {
                    Navigation::Module {
                        tab,
                        slot,
                        catalog_index,
                        ..
                    } => Some(ModuleScope {
                        tab: *tab,
                        slot: *slot,
                        catalog_index: *catalog_index,
                    }),
                    _ => None,
                },
                ..PaletteMode::default()
            }));
        }
        Intent::OpenHelp => {
            *next_mode = Some(InteractionMode::Help);
        }
        Intent::OpenAutomation(kind) => {
            *next_mode = Some(InteractionMode::Automation(AutomationMode::new(kind)));
            effects.push(InteractionEffect::AutomationConfirm(kind));
        }
        Intent::AddAutomation(kind) => {
            *next_mode = Some(InteractionMode::Automation(AutomationMode::new(kind)));
            effects.push(InteractionEffect::AddAutomation(kind));
        }
        Intent::ActivatePerformance(kind) => {
            *next_mode = Some(InteractionMode::Performance(PerformanceMode::new(kind)));
        }
        Intent::EnterLeadPlay => {
            // The Lead page is the instrument: entering from elsewhere lands
            // on it so the rows the nudges edit are in view.
            if navigation.page() != Page::Lead {
                *navigation = Navigation::for_page(Page::Lead);
            }
            *next_mode = Some(InteractionMode::Lead(LeadPlay::default()));
        }
        Intent::StartGesture(kind) => {
            if gesture_input.press(kind) {
                effects.push(InteractionEffect::GesturePress {
                    kind,
                    tab: tab_for_page(navigation.page()),
                });
            }
        }
        Intent::AdjustSelected(delta) => effects.push(InteractionEffect::AdjustSelected(delta)),
        Intent::ResetSelected => effects.push(InteractionEffect::ResetSelected),
        Intent::ToggleAuto => effects.push(InteractionEffect::ToggleAuto),
        Intent::ToggleUnits => effects.push(InteractionEffect::ToggleUnits),
        Intent::ToggleMute { master } => {
            effects.push(InteractionEffect::ToggleMute { master });
        }
        Intent::ToggleTransport => effects.push(InteractionEffect::ToggleTransport),
        Intent::RemoveAutomation => effects.push(InteractionEffect::RemoveAutomation),
        Intent::RandomizeAutomationRow => effects.push(InteractionEffect::RandomizeAutomationRow),
        Intent::RandomizeSelected => effects.push(InteractionEffect::RandomizeSelected),
        Intent::RandomizeScope => effects.push(InteractionEffect::RandomizeScope),
        Intent::TouchSelected => effects.push(InteractionEffect::TouchSelected),
        Intent::Save => effects.push(InteractionEffect::Save),
        Intent::Quit => effects.push(InteractionEffect::Quit),
        _ => {}
    }
}

/// The shortcut map's own table: it owns nothing but Esc-to-close and the
/// global save/quit chords, since its content is static.
fn update_help(
    intent: Intent,
    next_mode: &mut Option<InteractionMode>,
    effects: &mut Vec<InteractionEffect>,
) {
    if !intent.is_handled_by(ModeKind::Help) {
        return;
    }
    match intent {
        Intent::Cancel => *next_mode = Some(InteractionMode::Browsing),
        Intent::Save => effects.push(InteractionEffect::Save),
        Intent::Quit => effects.push(InteractionEffect::Quit),
        _ => {}
    }
}

fn update_numeric(
    entry: &mut NumericEntry,
    intent: Intent,
    next_mode: &mut Option<InteractionMode>,
    effects: &mut Vec<InteractionEffect>,
) {
    if !intent.is_handled_by(ModeKind::Numeric) {
        return;
    }
    match intent {
        Intent::Cancel => *next_mode = Some(resume_mode(entry.resume)),
        Intent::TypeCharacter(character) => push_numeric(&mut entry.buffer, character),
        Intent::Backspace => {
            entry.buffer.pop();
        }
        Intent::Confirm => {
            if let Ok(value) = entry.buffer.parse::<f32>() {
                effects.push(InteractionEffect::CommitNumeric(value));
            }
            *next_mode = Some(resume_mode(entry.resume));
        }
        _ => {}
    }
}

fn update_palette(
    palette: &mut PaletteMode,
    page: Page,
    intent: Intent,
    next_mode: &mut Option<InteractionMode>,
    effects: &mut Vec<InteractionEffect>,
) {
    let entries = PaletteState::new(tab_for_page(page), &[], palette.module_scope);
    if palette
        .locked
        .is_some_and(|index| !entries.contains_entry(index))
    {
        palette.locked = None;
        palette.value_buffer.clear();
    }
    if !intent.is_handled_by(ModeKind::Palette) {
        return;
    }
    match intent {
        Intent::Cancel => *next_mode = Some(resume_mode(palette.resume)),
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
            let state = palette.project(tab_for_page(page));
            if palette.locked.is_none() && !state.matches.is_empty() {
                palette.selected = (state.selected as isize + delta)
                    .rem_euclid(state.matches.len() as isize)
                    as usize;
            }
        }
        Intent::PaletteAutocomplete => {
            let state = palette.project(tab_for_page(page));
            if palette.locked.is_none()
                && let Some(found) = state.matches.get(state.selected)
            {
                palette.locked = Some(found.entry_index);
            }
        }
        Intent::Confirm => {
            let state = palette.project(tab_for_page(page));
            if let Some(entry_index) = palette.locked {
                let entry = state.entry(entry_index);
                if let (Some(id), Ok(value)) = (entry.id(), palette.value_buffer.parse::<f32>()) {
                    palette.staged.push(PaletteStagedEdit {
                        id,
                        value_bits: value.to_bits(),
                    });
                    palette.locked = None;
                    palette.value_buffer.clear();
                    palette.query.clear();
                    palette.selected = 0;
                } else {
                    effects.push(palette_confirm(entry));
                    *next_mode = Some(InteractionMode::Browsing);
                }
            } else if !palette.staged.is_empty() && palette.query.is_empty() {
                effects.push(InteractionEffect::PaletteCommit(std::mem::take(
                    &mut palette.staged,
                )));
                *next_mode = Some(resume_mode(palette.resume));
            } else if let Some(found) = state.matches.get(state.selected) {
                effects.push(palette_confirm(state.entry(found.entry_index)));
                *next_mode = Some(InteractionMode::Browsing);
            }
        }
        Intent::CommitPaletteAtBar => {
            let state = palette.project(tab_for_page(page));
            if let Some(entry_index) = palette.locked
                && let Ok(value) = palette.value_buffer.parse::<f32>()
            {
                let entry = state.entry(entry_index);
                if let Some(id) = entry.id() {
                    palette.staged.push(PaletteStagedEdit {
                        id,
                        value_bits: value.to_bits(),
                    });
                }
            }
            if !palette.staged.is_empty() {
                effects.push(InteractionEffect::PaletteCommitAtBar(std::mem::take(
                    &mut palette.staged,
                )));
                *next_mode = Some(resume_mode(palette.resume));
            }
        }
        _ => {}
    }
}

fn update_automation(
    automation: &mut AutomationMode,
    navigation: &mut Navigation,
    intent: Intent,
    next_mode: &mut Option<InteractionMode>,
    effects: &mut Vec<InteractionEffect>,
) {
    if !intent.is_handled_by(ModeKind::Automation) {
        return;
    }
    match intent {
        Intent::Cancel => {
            effects.push(InteractionEffect::CloseAutomationAll);
            *next_mode = Some(InteractionMode::Browsing);
        }
        Intent::OpenAutomationField => {
            if matches!(automation, AutomationMode::Lfo { .. }) {
                *automation = AutomationMode::Lfo {
                    depth: LfoDepth::Editor,
                    selected: 0,
                };
            }
        }
        Intent::MoveSelection(delta) => {
            let selected = automation.selected_mut();
            *selected = selected.saturating_add_signed(delta);
        }
        Intent::Confirm => {
            effects.push(InteractionEffect::AutomationConfirm(automation.kind()));
        }
        Intent::ChangePage(direction) => {
            let page = match direction {
                PageDirection::Next => navigation.page().next(),
                PageDirection::Previous => navigation.page().previous(),
            };
            *navigation = Navigation::for_page(page);
            *next_mode = Some(InteractionMode::Browsing);
            effects.push(InteractionEffect::CloseAutomationAll);
        }
        Intent::OpenAutomation(kind) => {
            effects.push(InteractionEffect::AutomationConfirm(kind));
            *next_mode = Some(InteractionMode::Automation(AutomationMode::new(kind)));
        }
        Intent::AddAutomation(kind) => {
            effects.push(InteractionEffect::AddAutomation(kind));
            *next_mode = Some(InteractionMode::Automation(AutomationMode::new(kind)));
        }
        Intent::AdjustSelected(delta) => {
            effects.push(InteractionEffect::AdjustSelected(delta));
        }
        Intent::ResetSelected => effects.push(InteractionEffect::ResetSelected),
        Intent::ToggleAuto => effects.push(InteractionEffect::ToggleAuto),
        Intent::ToggleUnits => effects.push(InteractionEffect::ToggleUnits),
        Intent::ToggleMute { master } => {
            effects.push(InteractionEffect::ToggleMute { master });
        }
        Intent::ToggleTransport => effects.push(InteractionEffect::ToggleTransport),
        Intent::RemoveAutomation => {
            *next_mode = Some(InteractionMode::Browsing);
            effects.push(InteractionEffect::RemoveAutomation);
        }
        Intent::RandomizeAutomationRow => effects.push(InteractionEffect::RandomizeAutomationRow),
        Intent::RandomizeScope => effects.push(InteractionEffect::RandomizeScope),
        Intent::BeginNumeric(character) => {
            let mut entry = NumericEntry::default();
            push_numeric(&mut entry.buffer, character);
            entry.resume = Some(*automation);
            *next_mode = Some(InteractionMode::Numeric(entry));
        }
        Intent::OpenPalette => {
            *next_mode = Some(InteractionMode::Palette(PaletteMode {
                recent: Vec::new(),
                resume: Some(*automation),
                ..PaletteMode::default()
            }));
        }
        Intent::Save => effects.push(InteractionEffect::Save),
        Intent::Quit => effects.push(InteractionEffect::Quit),
        Intent::TouchSelected => effects.push(InteractionEffect::TouchSelected),
        _ => {}
    }
}

fn update_lead(
    play: &mut LeadPlay,
    navigation: &mut Navigation,
    intent: Intent,
    next_mode: &mut Option<InteractionMode>,
    effects: &mut Vec<InteractionEffect>,
) {
    if !intent.is_handled_by(ModeKind::Lead) {
        return;
    }
    match intent {
        Intent::MoveSelection(delta) => navigation.move_selection(delta),
        Intent::Cancel => {
            if play.held.take().is_some() {
                effects.push(InteractionEffect::LeadRelease);
            }
            *next_mode = Some(InteractionMode::Browsing);
        }
        Intent::PlayLeadTone { tone, hold } => {
            play.last_tone = Some(tone);
            play.held = hold.then_some(tone);
            play.holds = Some(hold);
            effects.push(InteractionEffect::LeadTone { tone, hold });
        }
        // Only the key that owns the sounding note releases it: lifting an
        // earlier key of a legato run leaves the newer note ringing.
        Intent::ReleaseLeadTone(tone) => {
            if play.held == Some(tone) {
                play.held = None;
                effects.push(InteractionEffect::LeadRelease);
            }
        }
        Intent::NudgeLead { id, delta } => {
            effects.push(InteractionEffect::LeadNudge { id, delta });
        }
        Intent::AdjustSelected(delta) => effects.push(InteractionEffect::AdjustSelected(delta)),
        Intent::ToggleLeadPattern => effects.push(InteractionEffect::LeadPattern),
        Intent::CaptureLeadPhrase => effects.push(InteractionEffect::LeadCapture),
        Intent::Save => effects.push(InteractionEffect::Save),
        Intent::Quit => effects.push(InteractionEffect::Quit),
        _ => {}
    }
}

/// The Jump leader: `Space`, a layer key, a parameter key, and the cursor
/// is on that row in browsing. It resolves an address and moves the
/// selection; it never changes a value, so it needs no key releases, no
/// capability branch, and no completion stage.
fn update_performance(
    performance: &mut PerformanceMode,
    navigation: &mut Navigation,
    intent: Intent,
    next_mode: &mut Option<InteractionMode>,
    effects: &mut Vec<InteractionEffect>,
) {
    if !intent.is_handled_by(ModeKind::Performance) {
        return;
    }
    match intent {
        Intent::Cancel => *next_mode = Some(InteractionMode::Browsing),
        // Space while the leader is already pending leaves its stage alone.
        Intent::ActivatePerformance(_) => {}
        Intent::Save => effects.push(InteractionEffect::Save),
        Intent::Quit => effects.push(InteractionEffect::Quit),
        Intent::SelectPerformanceInstrument { instrument } => {
            let PerformanceMode::Jump { stage } = performance;
            *stage = JumpStage::ChooseParameter { instrument };
            let page = instrument.page();
            *navigation = Navigation::for_page(page);
            effects.push(InteractionEffect::SelectPage(page));
        }
        Intent::JumpToParameter(parameter) => {
            let PerformanceMode::Jump { stage } = *performance;
            let tab = match stage {
                // No layer key: the page already open is the layer, so
                // `Space k` reaches the filter on whatever is in front of
                // you, on any page rather than only the four layer keys.
                JumpStage::ChooseLayer => navigation.tab(),
                JumpStage::ChooseParameter { instrument } => instrument.tab(),
            };
            let Some(effect) = jump_effect(tab, parameter) else {
                // No address to move to. Stay in the leader rather than
                // dropping the player somewhere they did not ask for.
                return;
            };
            effects.push(effect);
            *next_mode = Some(InteractionMode::Browsing);
        }
        _ => {}
    }
}

/// Where one layer's parameter lives. Volume is the layer's own level row.
/// Filter is the catalog filter module, which the adapter adds inert when
/// the chain has none, so the leader reaches it whether or not it is loaded.
///
/// Takes a `Tab` rather than a `PerformanceInstrument` because the leader
/// also aims at the open page, which can be a layer no selector key names.
fn jump_effect(tab: Tab, parameter: PerformanceParameter) -> Option<InteractionEffect> {
    match parameter {
        PerformanceParameter::Volume => {
            let id = tab.level_id()?;
            let index = super::spec_index(tab, id)?;
            Some(InteractionEffect::JumpToControl { tab, index, id })
        }
        PerformanceParameter::Filter => Some(InteractionEffect::PlaceModule {
            tab,
            catalog_index: super::module_catalog_index(FILTER_MODULE_ID),
        }),
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

/// What confirming a palette row does. A module row resolves to add-or-jump
/// in the adapter, which is the only place that can see whether the layer
/// already holds it.
fn palette_confirm(entry: &PaletteEntry) -> InteractionEffect {
    match entry {
        PaletteEntry::Control {
            tab,
            index_in_tab,
            spec,
        } => InteractionEffect::JumpToControl {
            tab: *tab,
            index: *index_in_tab,
            id: spec.id,
        },
        PaletteEntry::Module { tab, catalog_index } => InteractionEffect::PlaceModule {
            tab: *tab,
            catalog_index: *catalog_index,
        },
        PaletteEntry::ModuleControl { tab, spec, .. } => InteractionEffect::JumpToControl {
            tab: *tab,
            index: super::tab_specs(*tab)
                .iter()
                .position(|candidate| candidate.id == spec.id)
                .expect("scoped module entry uses its owning tab spec"),
            id: spec.id,
        },
    }
}

fn tab_for_page(page: Page) -> Tab {
    LAYERS[page as usize].tab
}

fn page_for_tab(tab: Tab) -> Page {
    LAYERS[tab as usize].page
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(model: InteractionModel, intent: Intent) -> Transition {
        model.update(SemanticAction::press(intent))
    }

    /// Page and tab lookups index `LAYERS` directly, so a row out of
    /// discriminant order would silently translate a page to another layer's
    /// tab.
    #[test]
    fn every_layer_row_sits_at_its_page_and_tab_discriminant() {
        for (index, layer) in LAYERS.iter().enumerate() {
            assert_eq!(layer.page as usize, index);
            assert_eq!(layer.tab as usize, index);
            assert_eq!(tab_for_page(layer.page), layer.tab);
            assert_eq!(page_for_tab(layer.tab), layer.page);
        }
    }

    #[test]
    fn every_instrument_row_sits_at_its_discriminant() {
        for (index, row) in INSTRUMENTS.iter().enumerate() {
            assert_eq!(row.instrument.index(), index);
            assert_eq!(
                PerformanceInstrument::from_key(row.key),
                Some(row.instrument)
            );
            assert_eq!(row.instrument.page(), row.page);
        }
    }

    #[test]
    fn every_standard_page_round_trips_through_its_layer_row() {
        for page in [
            StandardPage::Perc,
            StandardPage::Bass,
            StandardPage::Kick,
            StandardPage::Tonal,
            StandardPage::Clap,
            StandardPage::Arp,
        ] {
            assert_eq!(
                Navigation::for_page(page.page()),
                Navigation::Standard { page, selected: 0 }
            );
        }
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
                InteractionMode::Performance(PerformanceMode::Jump {
                    stage: JumpStage::ChooseLayer,
                }),
                InteractionMode::Browsing,
            ),
            (
                InteractionMode::Lead(LeadPlay {
                    last_tone: Some(3),
                    ..LeadPlay::default()
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
    fn lead_pattern_drill_opens_from_the_steps_row_and_cancels_back_to_it() {
        let model = InteractionModel {
            navigation: Navigation::Lead {
                selected: 8,
                drill: LeadDrill::None,
            },
            ..InteractionModel::default()
        };
        let opened = update(model, Intent::EnterLeadPattern).model;
        assert_eq!(
            opened.navigation,
            Navigation::Lead {
                selected: 0,
                drill: LeadDrill::Pattern { return_to: 8 },
            }
        );
        assert_eq!(
            update(opened, Intent::Cancel).model.navigation,
            Navigation::Lead {
                selected: 8,
                drill: LeadDrill::None,
            }
        );
    }

    #[test]
    fn select_control_lands_a_lead_step_inside_the_pattern_drill() {
        let controls = super::super::FluidControls::default();
        let step = super::super::spec_index(Tab::Lead, "lead.step3").expect("step 3 is a row");
        let steps = super::super::spec_index(Tab::Lead, "lead.steps").expect("steps is a row");
        let mut model = InteractionModel::default();
        model.select_control(Tab::Lead, step, &controls);
        assert_eq!(
            model.navigation,
            Navigation::Lead {
                selected: 2,
                drill: LeadDrill::Pattern { return_to: steps },
            }
        );
        model.select_control(Tab::Lead, steps, &controls);
        assert_eq!(
            model.navigation,
            Navigation::Lead {
                selected: steps,
                drill: LeadDrill::None,
            }
        );
    }

    /// Regression test for the "adding Drive to Pads sometimes opens inside
    /// the custom chord progression" bug: jumping `select_control` straight
    /// to a module-slot row's flat index must land on the Pads root
    /// (`ChordDrill::None`), never inside a chord-slot drill, regardless of
    /// which chord drill the model happened to be in before the jump.
    #[test]
    fn select_control_on_a_module_slot_row_stays_out_of_the_chord_drill() {
        let mut controls = super::super::FluidControls::default();
        controls.modules.pad[1] = super::super::preset_slot("drive", 0.0);
        let id = super::super::module_slot_collapsed_id(Tab::Chords, 1, &controls)
            .expect("pads has a slot 2");
        let flat =
            super::super::spec_index(Tab::Chords, id).expect("slot 2 amount is a real control");

        let mut model = InteractionModel {
            navigation: Navigation::Chords {
                selected: 4,
                drill: ChordDrill::Slot {
                    slot: 3,
                    return_to: 4,
                },
            },
            ..InteractionModel::default()
        };
        model.select_control(Tab::Chords, flat, &controls);

        let expected_selected = super::super::chords_tab_controls(&controls, ChordDrill::None)
            .iter()
            .position(|item| item.id == id)
            .expect("slot 2 amount renders once occupied");
        assert_eq!(
            model.navigation,
            Navigation::Chords {
                selected: expected_selected,
                drill: ChordDrill::None,
            }
        );
    }

    #[test]
    fn module_detail_enters_and_returns_to_its_collapsed_row() {
        let model = InteractionModel {
            navigation: Navigation::Standard {
                page: StandardPage::Clap,
                selected: 6,
            },
            ..InteractionModel::default()
        };
        let detail = update(
            model,
            Intent::EnterModuleDetail {
                tab: Tab::Clap,
                slot: 2,
                catalog_index: 5,
            },
        )
        .model;
        assert_eq!(
            detail.navigation,
            Navigation::Module {
                tab: Tab::Clap,
                slot: 2,
                catalog_index: 5,
                selected: 0,
                return_to: 6,
            }
        );
        assert_eq!(
            update(detail, Intent::Cancel).model.navigation,
            Navigation::Standard {
                page: StandardPage::Clap,
                selected: 6,
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
            Navigation::Master { selected: 0 }
        );
    }

    #[test]
    fn browsing_cancel_on_master_is_a_no_op() {
        let model = InteractionModel {
            navigation: Navigation::Master { selected: 2 },
            ..InteractionModel::default()
        };

        assert_eq!(update(model.clone(), Intent::Cancel).model, model);
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
            Intent::BeginNumeric('1'),
            Intent::PaletteAutocomplete,
            Intent::Confirm,
            Intent::OpenPalette,
            Intent::OpenAutomation(AutomationKind::Lfo),
            Intent::OpenAutomationField,
            Intent::ActivatePerformance(PerformanceKind::Jump),
            Intent::SelectPerformanceInstrument {
                instrument: PerformanceInstrument::Pads,
            },
            Intent::JumpToParameter(PerformanceParameter::Volume),
            Intent::Save,
            Intent::Quit,
        ];
        for intent in edge_triggered {
            assert_eq!(intent.phase_policy(), PhasePolicy::Edge);
        }

        assert_eq!(
            Intent::ReleaseLeadTone(1).phase_policy(),
            PhasePolicy::ReleaseOnly
        );
    }

    #[test]
    fn one_shot_actions_ignore_repeat_and_release() {
        for phase in [InputPhase::Repeat, InputPhase::Release] {
            let action = SemanticAction {
                phase,
                intent: Intent::ActivatePerformance(PerformanceKind::Jump),
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
        let model = update(InteractionModel::default(), Intent::EnterLeadPlay).model;
        // A ReleaseOnly intent is inert on Press, whatever the mode holds.
        let pressed_release = model
            .clone()
            .update(SemanticAction::press(Intent::ReleaseLeadTone(1)));
        assert!(pressed_release.effects.is_empty());

        let bare_release = model.clone().update(SemanticAction {
            phase: InputPhase::Release,
            intent: Intent::ReleaseLeadTone(1),
        });
        assert!(bare_release.effects.is_empty());

        let held = update(
            model,
            Intent::PlayLeadTone {
                tone: 1,
                hold: true,
            },
        );
        assert_eq!(
            held.effects,
            vec![InteractionEffect::LeadTone {
                tone: 1,
                hold: true
            }]
        );

        let released = held.model.update(SemanticAction {
            phase: InputPhase::Release,
            intent: Intent::ReleaseLeadTone(1),
        });
        assert_eq!(released.effects, vec![InteractionEffect::LeadRelease]);
        assert!(
            released
                .model
                .update(SemanticAction {
                    phase: InputPhase::Release,
                    intent: Intent::ReleaseLeadTone(1),
                })
                .effects
                .is_empty()
        );
    }

    /// The leader owns no physical hold, so leaving it is a plain mode
    /// change with nothing to hand back.
    #[test]
    fn cancel_leaves_the_leader_without_emitting_effects() {
        let aimed = update(
            update(
                InteractionModel::default(),
                Intent::ActivatePerformance(PerformanceKind::Jump),
            )
            .model,
            Intent::SelectPerformanceInstrument {
                instrument: PerformanceInstrument::Perc,
            },
        )
        .model;

        let cancelled = update(aimed, Intent::Cancel);
        assert!(cancelled.effects.is_empty());
        assert_eq!(cancelled.model.mode, InteractionMode::Browsing);
    }

    #[test]
    fn duplicate_performance_activation_is_idempotent() {
        let kind = PerformanceKind::Jump;
        let first = update(
            InteractionModel::default(),
            Intent::ActivatePerformance(kind),
        );
        let second = update(first.model.clone(), Intent::ActivatePerformance(kind));
        assert_eq!(second.model, first.model);
        assert!(second.effects.is_empty());
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
                Intent::ActivatePerformance(PerformanceKind::Jump),
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
                InteractionMode::Performance(PerformanceMode::Jump {
                    stage: JumpStage::ChooseLayer,
                }),
                Intent::OpenAutomation(AutomationKind::Lfo),
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
            ..InteractionModel::default()
        }
    }

    #[test]
    fn palette_selection_wraps_projected_matches_and_freezes_while_locked() {
        let palette = PaletteMode {
            query: "bass".to_string(),
            ..PaletteMode::default()
        };
        let projected = palette.project(Tab::Bass);
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
        let projected = base.project(Tab::Bass);
        let expected = palette_confirm(projected.entry(projected.matches[1].entry_index));

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
    fn choosing_a_layer_opens_its_page_and_waits_for_a_parameter() {
        let model = update(
            InteractionModel::default(),
            Intent::ActivatePerformance(PerformanceKind::Jump),
        )
        .model;
        let transition = update(
            model,
            Intent::SelectPerformanceInstrument {
                instrument: PerformanceInstrument::Perc,
            },
        );

        assert_eq!(
            transition.effects,
            vec![InteractionEffect::SelectPage(Page::Perc)]
        );
        assert_eq!(
            transition.model.navigation,
            Navigation::Standard {
                page: StandardPage::Perc,
                selected: 0,
            }
        );
        assert_eq!(
            transition.model.mode,
            InteractionMode::Performance(PerformanceMode::Jump {
                stage: JumpStage::ChooseParameter {
                    instrument: PerformanceInstrument::Perc
                }
            })
        );
    }

    #[test]
    fn performance_instruments_are_a_closed_four_choice_grammar() {
        let leader = update(
            InteractionModel::default(),
            Intent::ActivatePerformance(PerformanceKind::Jump),
        )
        .model;
        for instrument in PerformanceInstrument::ALL {
            let transition = update(
                leader.clone(),
                Intent::SelectPerformanceInstrument { instrument },
            );
            assert_eq!(transition.model.navigation.page(), instrument.page());
        }
    }

    /// Arrival is the whole gesture: one effect that moves the cursor, and
    /// the keyboard back in browsing so `h`/`l` adjust what it landed on.
    #[test]
    fn every_layer_and_parameter_jumps_and_returns_to_browsing() {
        for instrument in PerformanceInstrument::ALL {
            for parameter in PerformanceParameter::ALL {
                let aimed = update(
                    update(
                        InteractionModel::default(),
                        Intent::ActivatePerformance(PerformanceKind::Jump),
                    )
                    .model,
                    Intent::SelectPerformanceInstrument { instrument },
                )
                .model;
                let arrived = update(aimed, Intent::JumpToParameter(parameter));

                let expected = match parameter {
                    PerformanceParameter::Volume => InteractionEffect::JumpToControl {
                        tab: instrument.tab(),
                        index: super::super::spec_index(
                            instrument.tab(),
                            instrument.tab().level_id().expect("layer has a level"),
                        )
                        .expect("level row is on its own tab"),
                        id: instrument.tab().level_id().expect("layer has a level"),
                    },
                    PerformanceParameter::Filter => InteractionEffect::PlaceModule {
                        tab: instrument.tab(),
                        catalog_index: super::super::module_catalog_index(FILTER_MODULE_ID),
                    },
                };
                assert_eq!(arrived.effects, vec![expected]);
                assert_eq!(arrived.model.mode, InteractionMode::Browsing);
            }
        }
    }

    /// A parameter key with no layer key aims at the page already open, so
    /// reaching a knob on the layer in front of you is two keys. It works on
    /// every page, including the ones no layer key names.
    #[test]
    fn a_parameter_key_without_a_layer_aims_at_the_open_page() {
        for tab in Tab::all() {
            let browsing = InteractionModel {
                navigation: Navigation::for_page(page_for_tab(tab)),
                ..InteractionModel::default()
            };
            let leader = update(browsing, Intent::ActivatePerformance(PerformanceKind::Jump)).model;
            let arrived = update(
                leader,
                Intent::JumpToParameter(PerformanceParameter::Volume),
            );

            let id = tab.level_id().expect("every tab has a level row");
            assert_eq!(
                arrived.effects,
                vec![InteractionEffect::JumpToControl {
                    tab,
                    index: super::super::spec_index(tab, id).expect("level row is on its own tab"),
                    id,
                }],
                "{tab:?}"
            );
            assert_eq!(arrived.model.mode, InteractionMode::Browsing, "{tab:?}");
        }
    }

    #[test]
    fn identical_inputs_are_deterministic() {
        let model = InteractionModel::default();
        let action = SemanticAction::press(Intent::OpenAutomation(AutomationKind::Lfo));
        assert_eq!(model.clone().update(action), model.update(action),);
    }

    #[test]
    fn automation_key_cycles_without_closing_and_shift_adds_a_lane() {
        let opened = update(
            InteractionModel::default(),
            Intent::OpenAutomation(AutomationKind::Lfo),
        );
        let cycled = update(opened.model, Intent::OpenAutomation(AutomationKind::Lfo));
        let added = update(
            cycled.model.clone(),
            Intent::AddAutomation(AutomationKind::Lfo),
        );

        assert!(matches!(
            cycled.model.mode,
            InteractionMode::Automation(AutomationMode::Lfo { .. })
        ));
        assert_eq!(
            added.effects,
            vec![InteractionEffect::AddAutomation(AutomationKind::Lfo)]
        );
    }
}
