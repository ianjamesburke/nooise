//! Dial-range epochs: the song code's second version axis.
//!
//! A value stored in its own units (Hz, ms) survives a dial's range changing,
//! but a modulation depth is a fraction of the dial's throw, so the same
//! depth sweeps a different musical span once the throw moves. Every code
//! records the epoch it was written under (`RANGE_EPOCH_RECORD`; a code
//! without one is epoch 0). Decoding refuses a code from an older epoch that
//! modulates a dial `RANGE_CHANGES` lists as changed since, the way a code
//! naming a retired control is refused; every other old code loads
//! untouched. No load ever reinterprets a depth.

use super::*;

/// Epoch this build writes. Always the newest `RANGE_CHANGES` entry
/// (`current_range_epoch_is_the_newest_change`).
pub(crate) const CURRENT_RANGE_EPOCH: u16 = 1;

/// Which dial a range change moved.
#[derive(Clone, Copy)]
enum RangeTarget {
    /// One field of any slot holding a module of this family.
    ModuleField {
        family: Family,
        field: ModuleSlotField,
    },
}

/// One dial whose range changed at `epoch`.
struct RangeChange {
    epoch: u16,
    target: RangeTarget,
}

/// Every dial-range change, oldest first. APPEND-ONLY: a code's stored epoch
/// indexes this history, so an entry is never edited or removed. A new
/// change appends an entry at `CURRENT_RANGE_EPOCH + 1`, bumps it, and
/// re-authors `AUTO_STATES` so their sweeps keep their span.
const RANGE_CHANGES: &[RangeChange] = &[
    // Filter cutoff: 80..8000 Hz became 20..20000 Hz (both Log2).
    RangeChange {
        epoch: 1,
        target: RangeTarget::ModuleField {
            family: Family::Filter,
            field: ModuleSlotField::Time,
        },
    },
];

impl RangeChange {
    fn applies(&self, id: &str, controls: &FluidControls) -> bool {
        match self.target {
            RangeTarget::ModuleField { family, field } => module_slot_row(id, controls)
                .is_some_and(|(slot, slot_field)| {
                    slot_field == field && slot.kind().is_some_and(|kind| kind.family == family)
                }),
        }
    }
}

/// The first control a code written at `epoch` modulates whose dial range has
/// changed since, or `None` when the code sweeps nothing that moved.
pub(crate) fn stale_modulation(song: &SongState, epoch: u16) -> Option<&'static str> {
    let modulated = song
        .automation
        .routes()
        .map(|(address, _)| address)
        .chain(song.automation.envelopes().map(|(address, _)| address));
    for address in modulated {
        if RANGE_CHANGES
            .iter()
            .any(|change| change.epoch > epoch && change.applies(address.id(), &song.controls))
        {
            return Some(address.id());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::super::song::SongCodeError;
    use super::*;

    /// Built-in song 2 as the last build on the 80..8000 Hz cutoff dial
    /// saved it (`a54b594`): epoch 0, with an LFO on Perc's Filter cutoff.
    const PRE_WIDENING_SONG_2: &str = "n1_Tk9PSQIAcwAAABgAAAAAj8IBAABUUwIAAEhhAwADAgQAAwIFAAMEBgADCAgAALieCQAAAAAKAACZmQ4AAwMQAAMFEwADAhgAAwMaAAMFHQADAx4AAwJ8AAB6lDMAAFwPNQAAcTk2AAIAAIhAlQABQwHFAAJKMPtEcQAARUUBOwAAAAMACgAAAMA_pDAAAABAP9XNKX4zAAAAeEEfBQAAAAAAmaB_xJUAAACAPy4FAAAAgD4AAAAAAAAAAAAA";

    #[test]
    fn an_old_code_sweeping_a_moved_dial_is_refused_by_name() {
        let Err(error) = decode_song_code(PRE_WIDENING_SONG_2) else {
            panic!("a stale code must not load");
        };
        assert_eq!(error, SongCodeError::StaleRange("perc.slot1.time"));
        assert!(
            error.to_string().contains("can no longer be loaded"),
            "{error}"
        );
    }

    #[test]
    fn an_old_code_that_sweeps_no_moved_dial_loads_untouched() {
        // Built-in song 0 on the pre-widening build: epoch 0, no automation.
        let song = decode_song_code("n1_Tk9PSQIADgAAAAIAlQAB5gPFAAJKMPtE")
            .expect("nothing it modulates has moved");
        let current = &decode_auto_states()[0];
        for spec in all_specs() {
            assert_eq!(
                spec.quantized_value(&song.controls),
                spec.quantized_value(&current.controls),
                "{}",
                spec.id
            );
        }
    }

    #[test]
    fn current_range_epoch_is_the_newest_change() {
        assert_eq!(
            RANGE_CHANGES.last().map(|change| change.epoch),
            Some(CURRENT_RANGE_EPOCH)
        );
        assert!(
            RANGE_CHANGES
                .windows(2)
                .all(|pair| pair[1].epoch == pair[0].epoch + 1),
            "epochs are consecutive and append-only"
        );
    }
}
