//! One step of a progression lane: the harmony, how it is voiced, and its
//! gate. Self-contained so a whole cell can be copied to another step, which
//! is what makes rhythmic chord writing practical.
//!
//! Built-in content still arrives through `chord_from_slot`; an editor will
//! author these directly in a later stage. Nothing here touches the wire
//! format.

use super::*;

/// How a cell meets the step boundary. `Sound` voices the chord (re-attacking
/// a repeated cell), `Hold` sustains the sounding chord across the boundary,
/// and `Rest` releases it. This replaces the legacy `-9`/`-8` sentinels
/// smuggled through the old `degree` field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Gate {
    Rest,
    Hold,
    Sound,
}

/// One authored progression step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Cell {
    pub(crate) harmony: Harmony,
    pub(crate) voicing: Voicing,
    pub(crate) gate: Gate,
}

impl Cell {
    /// Express a legacy chord slot as a cell. The gate is the slot's step
    /// mode; harmony and voicing come from the same bridge every voice uses.
    pub(crate) fn from_slot(slot: &ChordSlotControls) -> Self {
        let (harmony, voicing) = chord_from_slot(slot);
        let gate = if slot.is_rest() {
            Gate::Rest
        } else if slot.is_tie() {
            Gate::Hold
        } else {
            Gate::Sound
        };
        Self {
            harmony,
            voicing,
            gate,
        }
    }

    /// The ascending MIDI notes this cell voices.
    pub(crate) fn notes(self) -> [i32; 4] {
        realize(&self.harmony, &self.voicing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_from_slot_carries_every_built_in_chord_and_gate() {
        for progression in PROGRESSIONS.iter() {
            for chord in &progression.chords {
                let cell = Cell::from_slot(&chord.preset);
                assert_eq!(cell.notes(), legacy_chord_notes(&chord.preset));
                assert_eq!(cell.gate, Gate::Sound, "{}", chord.name);
            }
        }
    }
}
