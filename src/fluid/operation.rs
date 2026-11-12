//! Typed, static musical and application operations projected into control surfaces.

use super::*;

/// A named action, distinct from an editable musical control.
///
/// Operations are a closed vocabulary. Their palette metadata and typed
/// execution payload stay together here; no surface dispatches a string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Operation {
    Motion(MotionAction),
    Mix(mix_action::MixAction),
    Recipe(recipe::RecipeId),
    OutputDevice,
    Chord(ChordAction),
    ToggleAuto,
    ToggleVamp,
    ToggleKickBass,
    ToggleBeatRepeat,
    ResetDownbeat,
    /// `/chug`: the swung off-beat Perc groove (`EffectExecutor::chug`).
    Chug,
    ChordNames(ChordNames),
    ClearSequence,
    State(StateAction),
    Harmony(HarmonyAction),
    HarmonyQuantize(PushQuantize),
    /// Set `master.key_mode` to this `MASTER_MODES` row.
    SetMode(u8),
    /// Set `master.key`: 0..=11 are C..B.
    SetKey(u8),
}

const MODE_LABELS: [&str; MASTER_MODES.len()] = [
    "Mode major",
    "Mode minor",
    "Mode dorian",
    "Mode phrygian",
    "Mode mixolydian",
];
/// Black keys are spelled as flats: palette search reads `#` as a word
/// break, so `Key C#` would read as `Key C`. The sharp names are aliases.
const KEY_LABELS: [&str; 12] = [
    "Key C", "Key Db", "Key D", "Key Eb", "Key E", "Key F", "Key Gb", "Key G", "Key Ab", "Key A",
    "Key Bb", "Key B",
];
const KEY_ALIASES: [&[&str]; 12] = [
    &[],
    &["key c sharp"],
    &[],
    &["key d sharp"],
    &[],
    &[],
    &["key f sharp"],
    &[],
    &["key g sharp"],
    &[],
    &["key a sharp"],
    &[],
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StateAction {
    /// Keep the current song in slot `0..STATE_COUNT`, overwriting it.
    Store(usize),
    Recall(usize),
    Next,
    Previous,
    Cancel,
}

const STORE_LABELS: [&str; states::STATE_COUNT] = [
    "Store state 1",
    "Store state 2",
    "Store state 3",
    "Store state 4",
    "Store state 5",
    "Store state 6",
    "Store state 7",
    "Store state 8",
    "Store state 9",
];
const QUEUE_LABELS: [&str; states::STATE_COUNT] = [
    "Queue state 1",
    "Queue state 2",
    "Queue state 3",
    "Queue state 4",
    "Queue state 5",
    "Queue state 6",
    "Queue state 7",
    "Queue state 8",
    "Queue state 9",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChordAction {
    AddExtension,
    Restore,
    Copy,
    Paste,
    PasteAll,
}

/// A palette action keeps its original preset and slot, and refuses a slot
/// that changed while the palette was open (including an auto transition).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ChordTarget {
    pub(crate) progression: i8,
    pub(crate) slot: usize,
    fields: [u32; 9],
}

impl ChordTarget {
    pub(crate) fn capture(id: &str, controls: &FluidControls) -> Option<Self> {
        let (slot, _) = parse_chord_slot_id(id)?;
        let progression = progression_index(controls.pad.progression);
        Some(Self {
            progression: PROGRESSION_SONG_VALUES[progression],
            slot,
            fields: controls
                .pad
                .chord_slot(progression, slot)
                .values()
                .map(f32::to_bits),
        })
    }

    pub(crate) fn resolve(self, controls: &FluidControls) -> Option<usize> {
        let progression = progression_index(controls.pad.progression);
        (PROGRESSION_SONG_VALUES[progression] == self.progression
            && self.slot < CHORD_SLOT_COUNT
            && controls
                .pad
                .chord_slot(progression, self.slot)
                .values()
                .map(f32::to_bits)
                == self.fields)
            .then_some(progression)
    }
}

/// The display metadata belonging to one [`Operation`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OperationSpec {
    pub(crate) operation: Operation,
    pub(crate) label: &'static str,
    pub(crate) aliases: &'static [&'static str],
    pub(crate) description: &'static str,
}

impl Operation {
    pub(crate) const ALL: [Self; 88] = [
        Self::Motion(MotionAction::Grab(MotionDuration::Beats4)),
        Self::Motion(MotionAction::Grab(MotionDuration::Beats8)),
        Self::Motion(MotionAction::Grab(MotionDuration::Beats16)),
        Self::Motion(MotionAction::Grab(MotionDuration::Beats32)),
        Self::Motion(MotionAction::Bypass),
        Self::Motion(MotionAction::Resume),
        Self::Motion(MotionAction::Delete),
        Self::Mix(mix_action::MixAction::KickOnly),
        Self::Mix(mix_action::MixAction::MuteKick),
        Self::Recipe(recipe::RecipeId::Sway),
        Self::Recipe(recipe::RecipeId::Tremolo),
        Self::Recipe(recipe::RecipeId::Sidechain),
        Self::Recipe(recipe::RecipeId::Pulse),
        Self::Recipe(recipe::RecipeId::Drift),
        Self::Recipe(recipe::RecipeId::Rise),
        Self::Recipe(recipe::RecipeId::Rhythm),
        Self::OutputDevice,
        Self::Chord(ChordAction::AddExtension),
        Self::Chord(ChordAction::Restore),
        Self::Chord(ChordAction::Copy),
        Self::Chord(ChordAction::Paste),
        Self::Chord(ChordAction::PasteAll),
        Self::ToggleAuto,
        Self::ToggleVamp,
        Self::ToggleKickBass,
        Self::ToggleBeatRepeat,
        Self::ResetDownbeat,
        Self::Chug,
        Self::ChordNames(ChordNames::Roman),
        Self::ChordNames(ChordNames::Letters),
        Self::ClearSequence,
        Self::State(StateAction::Store(0)),
        Self::State(StateAction::Store(1)),
        Self::State(StateAction::Store(2)),
        Self::State(StateAction::Store(3)),
        Self::State(StateAction::Store(4)),
        Self::State(StateAction::Store(5)),
        Self::State(StateAction::Store(6)),
        Self::State(StateAction::Store(7)),
        Self::State(StateAction::Store(8)),
        Self::State(StateAction::Recall(0)),
        Self::State(StateAction::Recall(1)),
        Self::State(StateAction::Recall(2)),
        Self::State(StateAction::Recall(3)),
        Self::State(StateAction::Recall(4)),
        Self::State(StateAction::Recall(5)),
        Self::State(StateAction::Recall(6)),
        Self::State(StateAction::Recall(7)),
        Self::State(StateAction::Recall(8)),
        Self::State(StateAction::Next),
        Self::State(StateAction::Previous),
        Self::State(StateAction::Cancel),
        Self::Harmony(HarmonyAction::Move(HarmonyMove::StepDown)),
        Self::Harmony(HarmonyAction::Move(HarmonyMove::StepUp)),
        Self::Harmony(HarmonyAction::Move(HarmonyMove::FifthDown)),
        Self::Harmony(HarmonyAction::Move(HarmonyMove::FifthUp)),
        Self::Harmony(HarmonyAction::Move(HarmonyMove::Relative)),
        Self::Harmony(HarmonyAction::Move(HarmonyMove::Parallel)),
        Self::Harmony(HarmonyAction::Move(HarmonyMove::LeadingTone)),
        Self::Harmony(HarmonyAction::Move(HarmonyMove::Home)),
        Self::Harmony(HarmonyAction::Move(HarmonyMove::StepDownTwo)),
        Self::Harmony(HarmonyAction::Move(HarmonyMove::StepDownThree)),
        Self::Harmony(HarmonyAction::Move(HarmonyMove::StepUpTwo)),
        Self::Harmony(HarmonyAction::Move(HarmonyMove::StepUpThree)),
        Self::Harmony(HarmonyAction::Move(HarmonyMove::RaiseRoot)),
        Self::Harmony(HarmonyAction::Undo),
        Self::HarmonyQuantize(PushQuantize::Beat),
        Self::HarmonyQuantize(PushQuantize::Off),
        Self::HarmonyQuantize(PushQuantize::OneBar),
        Self::HarmonyQuantize(PushQuantize::TwoBars),
        Self::HarmonyQuantize(PushQuantize::FourBars),
        Self::SetMode(0),
        Self::SetMode(1),
        Self::SetMode(2),
        Self::SetMode(3),
        Self::SetMode(4),
        Self::SetKey(0),
        Self::SetKey(1),
        Self::SetKey(2),
        Self::SetKey(3),
        Self::SetKey(4),
        Self::SetKey(5),
        Self::SetKey(6),
        Self::SetKey(7),
        Self::SetKey(8),
        Self::SetKey(9),
        Self::SetKey(10),
        Self::SetKey(11),
    ];

    pub(crate) fn spec(self) -> OperationSpec {
        match self {
            Self::Motion(action) => OperationSpec {
                operation: self,
                label: action.name(),
                aliases: match action {
                    MotionAction::Grab(MotionDuration::Beats4) => {
                        &["grab 1 bar", "motion grab 1 bar"]
                    }
                    MotionAction::Grab(MotionDuration::Beats8) => {
                        &["grab 2 bars", "motion grab 2 bars"]
                    }
                    MotionAction::Grab(MotionDuration::Beats16) => {
                        &["grab 4 bars", "motion grab 4 bars"]
                    }
                    MotionAction::Grab(MotionDuration::Beats32) => {
                        &["grab 8 bars", "motion grab 8 bars"]
                    }
                    _ => &[],
                },
                description: action.description(),
            },
            Self::Mix(action) => OperationSpec {
                operation: self,
                label: action.name(),
                aliases: action.aliases(),
                description: action.description(),
            },
            Self::Recipe(id) => {
                let recipe = id.recipe();
                OperationSpec {
                    operation: self,
                    label: recipe.name,
                    aliases: recipe.aliases,
                    description: recipe.description,
                }
            }
            Self::Chord(action) => OperationSpec {
                operation: self,
                label: match action {
                    ChordAction::AddExtension => "Add extension",
                    ChordAction::Restore => "Restore chord",
                    ChordAction::Copy => "Copy chord",
                    ChordAction::Paste => "Paste chord",
                    ChordAction::PasteAll => "Paste chord to all steps",
                },
                aliases: match action {
                    ChordAction::Copy => &["copy step", "copy this chord"],
                    ChordAction::Paste => &["paste step", "paste this chord"],
                    ChordAction::PasteAll => &["paste all", "fill with chord"],
                    ChordAction::AddExtension | ChordAction::Restore => &[],
                },
                description: match action {
                    ChordAction::AddExtension => "open the chord's second extension",
                    ChordAction::Restore => "restore this chord's original voicing",
                    ChordAction::Copy => "keep this chord for pasting onto other steps",
                    ChordAction::Paste => "write the copied chord onto this step",
                    ChordAction::PasteAll => "write the copied chord onto every step of the phrase",
                },
            },
            Self::ChordNames(ChordNames::Roman) => OperationSpec {
                operation: self,
                label: "Roman chords",
                aliases: &["roman", "numerals", "roman numerals"],
                description: "write chords as Roman numerals in the home key",
            },
            Self::ChordNames(ChordNames::Letters) => OperationSpec {
                operation: self,
                label: "Letter chords",
                aliases: &["letters", "chord letters", "chord names"],
                description: "write chords as letter names",
            },
            Self::OutputDevice => OperationSpec {
                operation: self,
                label: "Output device",
                aliases: &["output", "device", "audio output", "routing"],
                description: "choose where live audio plays",
            },
            Self::ToggleAuto => OperationSpec {
                operation: self,
                label: "Toggle auto",
                aliases: &["auto", "auto morph", "morph"],
                description: "start or stop auto-morphing between songs",
            },
            Self::ToggleVamp => OperationSpec {
                operation: self,
                label: "Toggle vamp",
                aliases: &["vamp", "chord vamp"],
                description: "vamp the current chord or cue release",
            },
            Self::ToggleKickBass => OperationSpec {
                operation: self,
                label: "Toggle group mute",
                aliases: &["group mute", "q mute"],
                description: "toggle the Kick and Bass group gate",
            },
            Self::ToggleBeatRepeat => OperationSpec {
                operation: self,
                label: "Beat repeat",
                aliases: &["repeat", "beat repeat", "stutter"],
                description: "loop the current beat of live audio; Escape releases",
            },
            Self::Chug => OperationSpec {
                operation: self,
                label: "Chug",
                aliases: &["yeehaw", "boogie"],
                description: "swung 16th Perc, 60% swing, level accents on the off-beat",
            },
            Self::ResetDownbeat => OperationSpec {
                operation: self,
                label: "Reset downbeat",
                aliases: &["downbeat", "beat one", "bar one", "realign beat"],
                description: "make the nearest beat beat one of the bar; tempo and chord stay",
            },
            Self::State(action) => OperationSpec {
                operation: self,
                label: match action {
                    StateAction::Store(index) => STORE_LABELS[index],
                    StateAction::Recall(index) => QUEUE_LABELS[index],
                    StateAction::Next => "Queue next state",
                    StateAction::Previous => "Queue previous state",
                    StateAction::Cancel => "Cancel state queue",
                },
                aliases: &[],
                description: match action {
                    StateAction::Store(_) => "save current song, replacing any stored one",
                    StateAction::Recall(_) | StateAction::Next | StateAction::Previous => {
                        "change when the phrase ends"
                    }
                    StateAction::Cancel => "keep playing current state",
                },
            },
            Self::Harmony(action) => OperationSpec {
                operation: self,
                label: match action {
                    HarmonyAction::Move(HarmonyMove::StepDown) => "Harmony step down",
                    HarmonyAction::Move(HarmonyMove::StepUp) => "Harmony step up",
                    HarmonyAction::Move(HarmonyMove::FifthDown) => "Harmony resolve",
                    HarmonyAction::Move(HarmonyMove::FifthUp) => "Harmony lift",
                    HarmonyAction::Move(HarmonyMove::Relative) => "Harmony relative",
                    HarmonyAction::Move(HarmonyMove::Parallel) => "Harmony parallel",
                    HarmonyAction::Move(HarmonyMove::LeadingTone) => "Harmony leading tone",
                    HarmonyAction::Move(HarmonyMove::Home) => "Harmony home",
                    HarmonyAction::Move(HarmonyMove::StepDownTwo) => "Harmony down 2 steps",
                    HarmonyAction::Move(HarmonyMove::StepDownThree) => "Harmony down 3 steps",
                    HarmonyAction::Move(HarmonyMove::StepUpTwo) => "Harmony up 2 steps",
                    HarmonyAction::Move(HarmonyMove::StepUpThree) => "Harmony up 3 steps",
                    HarmonyAction::Move(HarmonyMove::RaiseRoot) => "Harmony raise root",
                    HarmonyAction::Undo => "Harmony undo",
                },
                aliases: match action {
                    HarmonyAction::Move(HarmonyMove::StepDown) => &["push j", "push step down"],
                    HarmonyAction::Move(HarmonyMove::StepUp) => &["push k", "push step up"],
                    HarmonyAction::Move(HarmonyMove::FifthDown) => {
                        &["push h", "down a fifth", "push resolve"]
                    }
                    HarmonyAction::Move(HarmonyMove::FifthUp) => {
                        &["push l", "up a fifth", "push lift"]
                    }
                    HarmonyAction::Move(HarmonyMove::Relative) => &["push f", "relative swap"],
                    HarmonyAction::Move(HarmonyMove::Parallel) => {
                        &["push d", "major minor flip", "parallel flip"]
                    }
                    HarmonyAction::Move(HarmonyMove::LeadingTone) => {
                        &["push s", "leading tone swap"]
                    }
                    HarmonyAction::Move(HarmonyMove::Home) => &["push a", "push home", "tonic"],
                    HarmonyAction::Move(HarmonyMove::StepDownTwo) => &["push u", "down a third"],
                    HarmonyAction::Move(HarmonyMove::StepDownThree) => &["push m", "down a fourth"],
                    HarmonyAction::Move(HarmonyMove::StepUpTwo) => &["push i", "up a third"],
                    HarmonyAction::Move(HarmonyMove::StepUpThree) => &["push comma", "up a fourth"],
                    HarmonyAction::Move(HarmonyMove::RaiseRoot) => {
                        &["push o", "sharp root", "half step up"]
                    }
                    HarmonyAction::Undo => &["push undo", "push backspace"],
                },
                description: match action {
                    HarmonyAction::Move(_) => "push the harmony (hold Space, then the key)",
                    HarmonyAction::Undo => "walk back the last harmony push",
                },
            },
            Self::HarmonyQuantize(quantize) => OperationSpec {
                operation: self,
                label: match quantize {
                    PushQuantize::Beat => "Harmony quantize 1 beat",
                    PushQuantize::Off => "Harmony quantize off",
                    PushQuantize::OneBar => "Harmony quantize 1 bar",
                    PushQuantize::TwoBars => "Harmony quantize 2 bars",
                    PushQuantize::FourBars => "Harmony quantize 4 bars",
                },
                aliases: match quantize {
                    PushQuantize::Beat => &[
                        "harmony quantize beat",
                        "push quantize beat",
                        "chord quantize beat",
                    ],
                    PushQuantize::Off => &[
                        "harmony quantize 0",
                        "push quantize off",
                        "chord quantize off",
                    ],
                    PushQuantize::OneBar => {
                        &["harmony quantize 1", "push quantize 1", "chord quantize 1"]
                    }
                    PushQuantize::TwoBars => {
                        &["harmony quantize 2", "push quantize 2", "chord quantize 2"]
                    }
                    PushQuantize::FourBars => {
                        &["harmony quantize 4", "push quantize 4", "chord quantize 4"]
                    }
                },
                description: match quantize {
                    PushQuantize::Off => "harmony pushes land at once",
                    PushQuantize::Beat => "harmony pushes land on the next beat (the default)",
                    _ => "harmony pushes wait for the next bar line of this length",
                },
            },
            Self::SetMode(index) => OperationSpec {
                operation: self,
                label: MODE_LABELS[usize::from(index)],
                aliases: &[],
                description: "re-read every progression in this mode; pushes move in it",
            },
            Self::SetKey(index) => OperationSpec {
                operation: self,
                label: KEY_LABELS[usize::from(index)],
                aliases: KEY_ALIASES[usize::from(index)],
                description: "the key every progression plays in",
            },
            Self::ClearSequence => OperationSpec {
                operation: self,
                label: "Clear sequence",
                aliases: &[
                    "clear steps",
                    "clear pattern",
                    "clear phrase",
                    "clear progression",
                    "reset sequence",
                ],
                description: "reset the active sequence steps (Pads to root, notes to rest)",
            },
        }
    }

    pub(crate) const fn capture_action(self) -> Option<CaptureAction> {
        match self {
            Self::Motion(action) => match action {
                MotionAction::Grab(_) => None,
                MotionAction::Bypass => Some(CaptureAction::Bypass),
                MotionAction::Resume => Some(CaptureAction::Resume),
                MotionAction::Delete => Some(CaptureAction::Delete),
            },
            Self::Mix(_)
            | Self::Recipe(_)
            | Self::OutputDevice
            | Self::Chord(_)
            | Self::ChordNames(_)
            | Self::ClearSequence
            | Self::State(_)
            | Self::Harmony(_)
            | Self::HarmonyQuantize(_)
            | Self::SetMode(_)
            | Self::SetKey(_)
            | Self::ToggleVamp
            | Self::ToggleKickBass
            | Self::ToggleBeatRepeat
            | Self::ResetDownbeat
            | Self::Chug
            | Self::ToggleAuto => None,
        }
    }

    pub(crate) const fn is_mix(self) -> bool {
        matches!(self, Self::Mix(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_operation_has_a_nonempty_label_and_description() {
        for operation in Operation::ALL {
            let spec = operation.spec();
            assert_eq!(spec.operation, operation);
            assert!(!spec.label.is_empty());
            assert!(!spec.description.is_empty());
        }
    }

    #[test]
    fn every_operation_projects_into_global_and_module_palettes() {
        let scope = ModuleScope {
            tab: Tab::Bass,
            slot: 0,
            catalog_index: module_catalog_index("filter"),
        };
        for module_scope in [None, Some(scope)] {
            for operation in Operation::ALL {
                let mut palette = PaletteState::new(Tab::Bass, &[], module_scope);
                for character in operation.spec().label.chars() {
                    palette.push_char(character);
                }
                assert!(palette.matches.iter().any(|found| {
                    matches!(palette.entry(found.entry_index), PaletteEntry::Operation(candidate) if *candidate == operation)
                }));
            }
        }
    }
}
