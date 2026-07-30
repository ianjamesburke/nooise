//! Stable numeric indexes for every control id a song code can carry.
//!
//! Song codes intern control ids as `u16` indexes into `SONG_ID_TABLE`
//! instead of spelling each id out, so the table's order is part of the
//! on-disk format.
//!
//! APPEND-ONLY. Never reorder an entry, never remove one, never reuse an
//! index for a different control. A new control appends its id at the end;
//! a retired control keeps its slot forever so old codes keep decoding.
//! The table is deliberately NOT derived from `all_specs()` — registry
//! order is free to change, this order is not.
//!
//! `song_ids_cover_every_registry_control` fails the build if a registry
//! control is missing here, so a new control can never become unsaveable.
const SONG_ID_TABLE: &[&str] = &[
    "pad.level",
    "pad.attack_time",
    "pad.release_time",
    "pad.type",
    "pad.chord_bars",
    "pad.chord_count",
    "pad.progression",
    "pad.reverb_mix",
    "pad.stereo_width",
    "pad.detune",
    "pad.octave_mix",
    "pad.chord1_degree",
    "pad.chord1_accidental",
    "pad.chord1_quality",
    "pad.chord1_extension",
    "pad.chord1_inversion",
    "pad.chord2_degree",
    "pad.chord2_accidental",
    "pad.chord2_quality",
    "pad.chord2_extension",
    "pad.chord2_inversion",
    "pad.chord3_degree",
    "pad.chord3_accidental",
    "pad.chord3_quality",
    "pad.chord3_extension",
    "pad.chord3_inversion",
    "pad.chord4_degree",
    "pad.chord4_accidental",
    "pad.chord4_quality",
    "pad.chord4_extension",
    "pad.chord4_inversion",
    "pad.chord5_degree",
    "pad.chord5_accidental",
    "pad.chord5_quality",
    "pad.chord5_extension",
    "pad.chord5_inversion",
    "pad.chord6_degree",
    "pad.chord6_accidental",
    "pad.chord6_quality",
    "pad.chord6_extension",
    "pad.chord6_inversion",
    "pad.chord7_degree",
    "pad.chord7_accidental",
    "pad.chord7_quality",
    "pad.chord7_extension",
    "pad.chord7_inversion",
    "pad.chord8_degree",
    "pad.chord8_accidental",
    "pad.chord8_quality",
    "pad.chord8_extension",
    "pad.chord8_inversion",
    "perc.level",
    "perc.filter",
    "perc.decay_ms",
    "perc.interval_beats",
    "perc.offset_beats",
    "perc.swing",
    "bass.level",
    "bass.cutoff",
    "bass.attack_time",
    "bass.decay_time",
    "bass.type",
    "bass.interval_beats",
    "bass.offset_beats",
    "bass.rhythm",
    "bass.octave",
    "bass.drive",
    "kick.level",
    "kick.filter",
    "kick.pitch_decay_ms",
    "kick.amp_decay_ms",
    "kick.type",
    "kick.interval_beats",
    "kick.offset_beats",
    "kick.start_freq",
    "kick.click",
    "kick.drive",
    "tonal.level",
    "tonal.attack",
    "tonal.decay",
    "tonal.synth_type",
    "tonal.octave",
    "tonal.phrase",
    "tonal.rate_beats",
    "tonal.step_interval_beats",
    "tonal.offset_beats",
    "tonal.swing",
    "tonal.randomness",
    "tonal.evolve_rate",
    "tonal.reverb_mix",
    "clap.level",
    "clap.filter",
    "clap.decay_ms",
    "clap.interval_beats",
    "clap.offset_beats",
    "clap.slap_count",
    "clap.slap_spread_ms",
    "clap.room",
    "clap.body",
    "arp.gain",
    "arp.attack",
    "arp.decay",
    "arp.type",
    "arp.rate_beats",
    "arp.offset_beats",
    "arp.swing",
    "arp.pattern",
    "arp.octaves",
    "arp.reverb_mix",
    "macro.1",
    "macro.2",
    "macro.3",
    "macro.4",
    "master.bpm",
    "master.level",
    "master.drive",
    "master.comp_amount",
    "master.comp_release_ms",
    "master.tone",
    "master.tune",
    "master.comp_threshold",
    "master.comp_ratio",
    "master.comp_makeup",
];

/// Index of `id` in the song-code id table, or `None` if the control has
/// never been assigned one.
pub(crate) fn song_id_index(id: &str) -> Option<u16> {
    SONG_ID_TABLE
        .iter()
        .position(|entry| *entry == id)
        .map(|index| index as u16)
}

/// The control id at `index`, or `None` if a code names a slot this build
/// does not know about.
pub(crate) fn song_id_at(index: u16) -> Option<&'static str> {
    SONG_ID_TABLE.get(index as usize).copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fluid::registry::all_specs;
    use std::collections::BTreeSet;

    #[test]
    fn song_ids_cover_every_registry_control() {
        let missing: Vec<_> = all_specs()
            .map(|spec| spec.id)
            .filter(|id| song_id_index(id).is_none())
            .collect();
        assert!(
            missing.is_empty(),
            "append these control ids to SONG_ID_TABLE so they can be saved: {missing:?}"
        );
    }

    #[test]
    fn song_id_table_has_no_duplicates() {
        let mut seen = BTreeSet::new();
        for id in SONG_ID_TABLE {
            assert!(seen.insert(*id), "duplicate song id table entry: {id}");
        }
    }

    #[test]
    fn song_id_index_and_lookup_are_inverses() {
        for (index, id) in SONG_ID_TABLE.iter().enumerate() {
            let index = index as u16;
            assert_eq!(song_id_index(id), Some(index));
            assert_eq!(song_id_at(index), Some(*id));
        }
        assert_eq!(song_id_at(SONG_ID_TABLE.len() as u16), None);
    }
}
