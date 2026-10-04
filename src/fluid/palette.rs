//! The `/` control palette.
//!
//! `/` opens a fuzzy-find prompt over every control id in the registry.
//! Typing filters; Tab locks the highlighted control; digits typed after the
//! lock become a value. Enter stages the edit (or jumps when no value was
//! typed); Enter on an empty prompt commits every staged edit at once, and
//! Ctrl+B commits them on the next bar downbeat instead. The registry tables
//! stay the single source of truth — the palette only builds a flat address
//! space over them, exactly one entry per unique control id at its native
//! (deepest) editing surface.

use super::module::{MODULE_CATALOG, chain_amount_slot, module_available_on, tab_has_module_chain};
use super::*;

use super::palette_search::{Query, SearchMetadata, SearchRank};

/// What a palette row does when confirmed.
///
/// Entries derive only from registry/catalog/recipe constants and scope: the
/// interaction kernel and the renderer each build this list independently and
/// confirm works by index, so anything that made the list depend on live
/// controls could desync them and jump to the wrong control. Live state is
/// resolved where it exists — in the adapter, and in the value column.
#[derive(Clone)]
pub(crate) enum PaletteEntry {
    Operation(Operation),
    /// Jump to a control at the tab that natively owns it.
    Control {
        tab: Tab,
        index_in_tab: usize,
        spec: &'static ControlSpec,
    },
    /// A catalog module on `tab`'s chain. Whether confirming adds it or jumps
    /// to the copy already there is decided at execution time, so the palette
    /// can never silently create a second copy.
    Module {
        tab: Tab,
        catalog_index: usize,
    },
    /// A stable slot control projected under the active module's human-facing
    /// dotted scope; its backing id remains slot-addressed for song codes.
    ModuleControl {
        tab: Tab,
        spec: &'static ControlSpec,
        module_name: &'static str,
        parameter: &'static str,
        search_parameter: &'static str,
        catalog_index: usize,
    },
}

impl PaletteEntry {
    /// Text the query matches against. Must stay a pure function of the
    /// entry, for the reason on the enum.
    pub(crate) fn haystack(&self) -> String {
        match self {
            Self::Operation(operation) => {
                let spec = operation.spec();
                format!(
                    "{} · {} · {}",
                    spec.label,
                    spec.description,
                    spec.aliases.join(" ")
                )
            }
            Self::Control { tab, spec, .. } => {
                format!("{} · {} · {}", spec.id, tab.name(), spec.label)
            }
            Self::Module { tab, catalog_index } => {
                let kind = MODULE_CATALOG[*catalog_index];
                if *tab == Tab::Master && kind.id == "swing" {
                    return "Global Swing · Master · module".to_string();
                }
                format!(
                    "{} · {} · {} · module",
                    kind.id,
                    kind.display_name,
                    tab.name()
                )
            }
            Self::ModuleControl {
                tab,
                spec,
                module_name,
                parameter,
                ..
            } => {
                format!(
                    "{}.{}.{} · {}",
                    tab.name().to_lowercase(),
                    module_name.to_lowercase(),
                    parameter.to_lowercase().replace(' ', "_"),
                    spec.label
                )
            }
        }
    }

    pub(crate) fn display_text(&self) -> String {
        match self {
            Self::Operation(operation) => {
                let spec = operation.spec();
                format!("{} · {}", spec.label, spec.description)
            }
            _ => self.haystack(),
        }
    }

    pub(crate) fn spec(&self) -> Option<&'static ControlSpec> {
        match self {
            Self::Control { spec, .. } | Self::ModuleControl { spec, .. } => Some(spec),
            Self::Module { .. } | Self::Operation(_) => None,
        }
    }

    pub(crate) fn id(&self) -> Option<&'static str> {
        self.spec().map(|spec| spec.id)
    }

    /// Value column. This one may read live controls: it is rendered, never
    /// matched against, so it cannot affect indices.
    pub(crate) fn value(&self, c: &FluidControls) -> String {
        match self {
            Self::Operation(operation) => match operation {
                Operation::Motion(_) => "selected knob".to_string(),
                Operation::Mix(_) => operation.spec().description.to_string(),
                Operation::Recipe(_) => "add lane".to_string(),
                Operation::PlannedMute => "visible layer".to_string(),
                Operation::Chord(_) => "selected chord".to_string(),
            },
            Self::Control { spec, .. } | Self::ModuleControl { spec, .. } => {
                if super::midi_row_bit(spec.id).is_some_and(|bit| c.midi_rows & bit == 0)
                    || super::pad_rhythm_row_bit(spec.id)
                        .is_some_and(|bit| c.hidden_pad_rhythm_rows & bit != 0)
                {
                    "add".to_string()
                } else {
                    (spec.display)(c)
                }
            }
            // A loaded module shows its collapsed row's value, the same
            // knob its page row shows (a Filter's cutoff, not its mix).
            Self::Module { tab, catalog_index } => {
                let kind = MODULE_CATALOG[*catalog_index];
                c.modules
                    .for_tab(*tab)
                    .and_then(|slots| chain_amount_slot(slots, kind.id))
                    .and_then(|slot| module_slot_collapsed_id(*tab, slot, c))
                    .and_then(spec_by_id)
                    .map_or_else(|| "add".to_string(), |spec| (spec.display)(c))
            }
        }
    }
}

/// Flat address space the palette searches: every unique control at its
/// owning tab, every available catalog module, and the static lane recipes.
/// Duplicated placements (e.g. `pad.level` on both Master and Chords) collapse
/// to the owning tab. Module slot rows are excluded as controls — a module is
/// reached through its `Module` entry, which knows how to find or create it.
/// MIDI Trigger is excluded because no page surfaces it; it only decodes.
pub(crate) fn palette_entries() -> Vec<PaletteEntry> {
    let mut entries: Vec<PaletteEntry> = Vec::new();
    for tab in Tab::all() {
        for (index_in_tab, spec) in tab_specs(tab).iter().enumerate() {
            if tab_owning_control(spec.id) == Some(tab)
                && parse_module_slot_id(spec.id).is_none()
                && spec.id != super::PAD_MIDI_TRIGGER_ID
                && !entries.iter().any(|e| e.id() == Some(spec.id))
            {
                entries.push(PaletteEntry::Control {
                    tab,
                    index_in_tab,
                    spec,
                });
            }
        }
    }
    for tab in Tab::all() {
        if !tab_has_module_chain(tab) {
            continue;
        }
        for (catalog_index, kind) in MODULE_CATALOG.iter().copied().enumerate() {
            if module_available_on(kind, tab) {
                entries.push(PaletteEntry::Module { tab, catalog_index });
            }
        }
    }
    entries.extend(Operation::ALL.into_iter().map(PaletteEntry::Operation));
    entries
}

#[cfg(test)]
mod global_groove_tests {
    use super::*;

    #[test]
    fn global_swing_and_drunken_are_addable_from_any_page() {
        for (query, id) in [("global swing", "swing"), ("drunken", "drunken")] {
            let mut palette = PaletteState::new(Tab::Kick, &[], None);
            for character in query.chars() {
                palette.push_char(character);
            }
            assert!(palette.matches.iter().any(|hit| matches!(
                palette.entry(hit.entry_index),
                PaletteEntry::Module { tab: Tab::Master, catalog_index }
                    if MODULE_CATALOG[*catalog_index].id == id
            )));
        }
    }

    #[test]
    fn matching_module_on_the_current_page_outranks_another_pages_fuzzy_hit() {
        let mut palette = PaletteState::new(Tab::Master, &[], None);
        for character in "swing".chars() {
            palette.push_char(character);
        }
        assert!(matches!(
            palette.entry(palette.matches[0].entry_index),
            PaletteEntry::Module { tab: Tab::Master, catalog_index }
                if MODULE_CATALOG[*catalog_index].id == "swing"
        ));
    }
}

/// One resolved candidate with deterministic semantic rank and display highlights.
pub(crate) struct PaletteMatch {
    pub(crate) entry_index: usize,
    rank: Option<SearchRank>,
    pub(crate) hits: Vec<usize>,
}

/// The module detail surface the palette was opened from.
///
/// While this is set the palette lists that module's own parameters instead of
/// the whole registry, so `/` inside a module drill stays inside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ModuleScope {
    pub(crate) tab: Tab,
    /// Storage slot on `tab`'s chain, not a catalog position.
    pub(crate) slot: usize,
    pub(crate) catalog_index: usize,
}

/// A value edit waiting in the palette's stage.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StagedEdit {
    pub(crate) id: &'static str,
    pub(crate) value: f32,
}

pub(crate) struct PaletteState {
    entries: Vec<PaletteEntry>,
    current_tab: Tab,
    recent: Vec<&'static str>,
    pub(crate) query: String,
    pub(crate) matches: Vec<PaletteMatch>,
    pub(crate) selected: usize,
    /// Entry locked by Tab-autocomplete; digits typed afterwards build
    /// `value_buf` for that control.
    pub(crate) locked: Option<usize>,
    pub(crate) value_buf: String,
    pub(crate) staged: Vec<StagedEdit>,
}

impl PaletteState {
    /// `recent` is most-recent-first and intentionally has no usage count:
    /// touching a control again simply promotes it to the front.
    pub(crate) fn new(
        current_tab: Tab,
        recent: &[&'static str],
        module_scope: Option<ModuleScope>,
    ) -> Self {
        Self::resolve(current_tab, recent, module_scope, "")
    }

    pub(crate) fn resolve(
        current_tab: Tab,
        recent: &[&'static str],
        module_scope: Option<ModuleScope>,
        query: &str,
    ) -> Self {
        let mut state = Self {
            entries: module_scope.map_or_else(palette_entries, |scope| {
                module_palette_entries(scope.tab, scope.slot, scope.catalog_index)
            }),
            current_tab,
            recent: recent.to_vec(),
            query: query.to_string(),
            matches: Vec::new(),
            selected: 0,
            locked: None,
            value_buf: String::new(),
            staged: Vec::new(),
        };
        state.recompute();
        state
    }

    pub(crate) fn entry(&self, index: usize) -> &PaletteEntry {
        &self.entries[index]
    }

    pub(crate) fn contains_entry(&self, index: usize) -> bool {
        index < self.entries.len()
    }

    #[cfg(test)]
    pub(crate) fn push_char(&mut self, c: char) {
        if self.locked.is_some() {
            if c.is_ascii_digit() || c == '.' || c == '-' {
                self.value_buf.push(c);
            }
        } else {
            self.query.push(c);
            self.recompute();
        }
    }

    fn recompute(&mut self) {
        self.matches.clear();
        if self.query.is_empty() {
            // Empty query is a global MRU navigator: every recent control,
            // regardless of page, then unused current-page controls, then
            // the remaining registry order.
            for entry_index in 0..self.entries.len() {
                self.matches.push(PaletteMatch {
                    entry_index,
                    rank: None,
                    hits: Vec::new(),
                });
            }
            self.sort_by_context();
        } else {
            let query = Query::new(&self.query);
            for (entry_index, entry) in self.entries.iter().enumerate() {
                let metadata = SearchMetadata::for_entry(entry);
                if let Some(rank) = query.rank(&metadata, self.current_tab, &self.recent) {
                    // Highlight display positions only after meaning has resolved.
                    let hits = fuzzy_score(&self.query, &entry.display_text())
                        .map_or_else(Vec::new, |(_, hits)| hits);
                    self.matches.push(PaletteMatch {
                        entry_index,
                        rank: Some(rank),
                        hits,
                    });
                }
            }
            self.matches
                .sort_by(|left, right| left.rank.cmp(&right.rank));
        }
        self.selected = 0;
    }

    fn sort_by_context(&mut self) {
        let entries = &self.entries;
        let recent = &self.recent;
        let current_tab = self.current_tab;
        self.matches
            .sort_by_key(|m| context_rank(entries, m.entry_index, current_tab, recent));
    }
}

fn module_palette_entries(tab: Tab, slot: usize, catalog_index: usize) -> Vec<PaletteEntry> {
    MODULE_CATALOG[catalog_index]
        .parameters()
        .iter()
        .filter_map(|parameter| {
            module_slot_spec(tab, slot, parameter.field).map(|spec| PaletteEntry::ModuleControl {
                tab,
                spec,
                module_name: MODULE_CATALOG[catalog_index].display_name,
                parameter: parameter.label,
                search_parameter: parameter.search_name,
                catalog_index,
            })
        })
        .chain(Operation::ALL.into_iter().map(PaletteEntry::Operation))
        .collect()
}

/// Recent controls always win, independent of page. Page order only breaks
/// ties once the user has exhausted their global MRU list.
fn context_rank(
    entries: &[PaletteEntry],
    entry: usize,
    current_tab: Tab,
    recent: &[&'static str],
) -> (u8, usize, usize) {
    let palette_entry = &entries[entry];
    let recent_rank = recent.iter().position(|&id| Some(id) == palette_entry.id());
    // A module offered for the page you are on ranks with that page's own
    // controls, so "swing" on Bass reaches Bass before it reaches Tonal.
    let page_index = match palette_entry {
        PaletteEntry::Operation(_) => None,
        PaletteEntry::Module { tab, catalog_index } => {
            (*tab == current_tab).then_some(tab_specs(current_tab).len() + catalog_index)
        }
        PaletteEntry::Control { spec, .. } => tab_specs(current_tab)
            .iter()
            .position(|other| other.id == spec.id),
        PaletteEntry::ModuleControl { tab, spec, .. } => (*tab == current_tab)
            .then(|| {
                tab_specs(current_tab)
                    .iter()
                    .position(|other| other.id == spec.id)
            })
            .flatten(),
    };
    let group = match (recent_rank, page_index) {
        (Some(_), _) => 0,
        (None, Some(_)) => 1,
        (None, None) => 2,
    };
    let order = match group {
        0 => recent_rank.unwrap_or(usize::MAX),
        1 => page_index.unwrap_or(usize::MAX),
        _ => entry,
    };
    (group, order, entry)
}

/// Beat position of the next bar downbeat (4/4 throughout the engine), for
/// the palette's quantized commit.
pub(crate) fn next_bar_beat(beat: f64) -> f64 {
    const BEATS_PER_BAR: f64 = 4.0;
    (beat / BEATS_PER_BAR).floor() * BEATS_PER_BAR + BEATS_PER_BAR
}

/// Greedy subsequence fuzzy match. Returns `None` if `query` is not a
/// subsequence of `haystack` (case-insensitive), otherwise a score (higher is
/// better) plus the matched char indices into `haystack` for highlighting.
///
/// Scoring rewards contiguous runs and word-start matches, and penalises how
/// deep the first match sits, so "rev" ranks a Reverb module above a scattered
/// hit inside another label.
pub(crate) fn fuzzy_score(query: &str, haystack: &str) -> Option<(i32, Vec<usize>)> {
    let needle: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    if needle.is_empty() {
        return Some((0, Vec::new()));
    }
    let hay: Vec<char> = haystack.chars().collect();
    // Compare against a lowercased view that stays index-aligned with the
    // original haystack (single-char lowercase), so hit indices highlight the
    // right characters.
    let lower_at = |i: usize| -> char {
        hay.get(i)
            .and_then(|c| c.to_lowercase().next())
            .unwrap_or(' ')
    };

    let mut hits = Vec::with_capacity(needle.len());
    let mut score = 0i32;
    let mut hay_i = 0usize;
    let mut prev_hit: Option<usize> = None;
    for &nc in &needle {
        let mut found = None;
        while hay_i < hay.len() {
            if lower_at(hay_i) == nc {
                found = Some(hay_i);
                break;
            }
            hay_i += 1;
        }
        let hit_index = found?;
        // Contiguous with previous match.
        if prev_hit == Some(hit_index.wrapping_sub(1)) {
            score += 8;
        }
        // Word-start bonus (start of string or preceded by a separator).
        let at_word_start =
            hit_index == 0 || matches!(hay.get(hit_index - 1), Some(' ') | Some('·') | Some('.'));
        if at_word_start {
            score += 6;
        }
        if hits.is_empty() {
            // Penalise a deep first match so shallow, leading matches win.
            score -= hit_index as i32;
        }
        hits.push(hit_index);
        prev_hit = Some(hit_index);
        hay_i = hit_index + 1;
    }
    // Reward matching a larger fraction of the label.
    score += (needle.len() as i32) * 2;
    Some((score, hits))
}
