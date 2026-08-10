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

/// What a palette row does when confirmed.
///
/// Entries are a pure function of the current tab, deliberately: the
/// interaction kernel and the renderer each build this list independently and
/// confirm works by index, so anything that made the list depend on live
/// controls could desync them and jump to the wrong control. Live state is
/// resolved where it exists — in the adapter, and in the value column.
pub(crate) enum PaletteEntry {
    /// Jump to a control at the tab that natively owns it.
    Control {
        tab: Tab,
        index_in_tab: usize,
        spec: &'static ControlSpec,
    },
    /// A catalog module on `tab`'s chain. Whether confirming adds it or jumps
    /// to the copy already there is decided at execution time, so the palette
    /// can never silently create a second copy.
    Module { tab: Tab, catalog_index: usize },
    /// A stable slot control projected under the active module's human-facing
    /// dotted scope; its backing id remains slot-addressed for song codes.
    ModuleControl {
        tab: Tab,
        spec: &'static ControlSpec,
        module_name: &'static str,
        parameter: &'static str,
    },
}

impl PaletteEntry {
    /// Text the query matches against. Must stay a pure function of the
    /// entry, for the reason on the enum.
    pub(crate) fn haystack(&self) -> String {
        match self {
            Self::Control { tab, spec, .. } => {
                format!("{} · {} · {}", spec.id, tab.name(), spec.label)
            }
            Self::Module { tab, catalog_index } => {
                let kind = MODULE_CATALOG[*catalog_index];
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

    pub(crate) fn spec(&self) -> Option<&'static ControlSpec> {
        match self {
            Self::Control { spec, .. } | Self::ModuleControl { spec, .. } => Some(spec),
            Self::Module { .. } => None,
        }
    }

    pub(crate) fn id(&self) -> Option<&'static str> {
        self.spec().map(|spec| spec.id)
    }

    /// Value column. This one may read live controls: it is rendered, never
    /// matched against, so it cannot affect indices.
    pub(crate) fn value(&self, c: &FluidControls) -> String {
        match self {
            Self::Control { spec, .. } | Self::ModuleControl { spec, .. } => (spec.display)(c),
            Self::Module { tab, catalog_index } => {
                let kind = MODULE_CATALOG[*catalog_index];
                c.modules
                    .for_tab(*tab)
                    .and_then(|slots| chain_amount_slot(slots, kind.id))
                    .map_or_else(
                        || "add".to_string(),
                        |slot| {
                            format!(
                                "{:.0}%",
                                c.modules.for_tab(*tab).expect("layer exists")[slot].amount * 100.0
                            )
                        },
                    )
            }
        }
    }
}

/// Flat address space the palette searches: every unique control at its
/// owning tab, plus every catalog module on every layer that has a chain.
/// Duplicated placements (e.g. `pad.level` on both Master and Chords) collapse
/// to the owning tab. Module slot rows are excluded as controls — a module is
/// reached through its `Module` entry, which knows how to find or create it.
pub(crate) fn palette_entries() -> Vec<PaletteEntry> {
    let mut entries: Vec<PaletteEntry> = Vec::new();
    for tab in Tab::all() {
        for (index_in_tab, spec) in tab_specs(tab).iter().enumerate() {
            if tab_owning_control(spec.id) == Some(tab)
                && !spec.id.contains(".slot")
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
    entries
}

/// One fuzzy candidate: which entry, how well it scored, and which characters
/// of its haystack matched (for highlighting).
pub(crate) struct PaletteMatch {
    pub(crate) entry_index: usize,
    pub(crate) score: i32,
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
        let mut state = Self {
            entries: module_scope.map_or_else(palette_entries, |scope| {
                module_palette_entries(scope.tab, scope.slot, scope.catalog_index)
            }),
            current_tab,
            recent: recent.to_vec(),
            query: String::new(),
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
                    score: 0,
                    hits: Vec::new(),
                });
            }
            self.sort_by_context();
        } else {
            for (entry_index, entry) in self.entries.iter().enumerate() {
                if let Some((score, hits)) = fuzzy_score(&self.query, &entry.haystack()) {
                    self.matches.push(PaletteMatch {
                        entry_index,
                        score,
                        hits,
                    });
                }
            }
            let entries = &self.entries;
            let recent = &self.recent;
            let current_tab = self.current_tab;
            let query = self.query.as_str();
            // The layer-primary boost outranks the fuzzy score deliberately.
            // Score alone puts `pad.stereo_width` above `pad.level` for the
            // query "pads", because its `s` follows the dot and collects a
            // word-start bonus. Typing a bare layer name must reach that
            // layer's level regardless.
            let primary_key = |entry: usize| {
                entries[entry]
                    .id()
                    .and_then(|id| layer_primary_rank(id, query))
                    .unwrap_or(usize::MAX)
            };
            self.matches.sort_by(|left, right| {
                primary_key(left.entry_index)
                    .cmp(&primary_key(right.entry_index))
                    .then_with(|| right.score.cmp(&left.score))
                    .then_with(|| {
                        context_rank(entries, left.entry_index, current_tab, recent).cmp(
                            &context_rank(entries, right.entry_index, current_tab, recent),
                        )
                    })
            });
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
            let field = parameter.field.id();
            let suffix = format!(".slot{}.{}", slot + 1, field);
            tab_specs(tab)
                .iter()
                .find(|spec| spec.id.ends_with(&suffix))
                .map(|spec| PaletteEntry::ModuleControl {
                    tab,
                    spec,
                    module_name: MODULE_CATALOG[catalog_index].display_name,
                    parameter: parameter.label,
                })
        })
        .collect()
}

/// A layer's primary control outranks everything while the query is still
/// just that layer's name, so typing `bass` always lands on Bass Level
/// whatever the MRU holds. Matches the id namespace (`bass.`) and the tab
/// name (`Bass`) alike, since `Tab::Chords` is spelled `Pads` but owns
/// `pad.*`. Returns the tab's discriminant so ambiguous prefixes (`m` hits
/// both Macros and Master) stay deterministically ordered.
///
/// The boost falls away on its own once the query grows past the namespace
/// (`bass.d` is no longer a prefix of `bass`), handing ranking back to the
/// fuzzy score.
fn layer_primary_rank(id: &str, query: &str) -> Option<usize> {
    if query.is_empty() {
        return None;
    }
    Tab::all().iter().position(|&tab| {
        tab.level_id() == Some(id) && {
            let namespace = id.split('.').next().unwrap_or(id);
            starts_with_ignore_case(namespace, query) || starts_with_ignore_case(tab.name(), query)
        }
    })
}

fn starts_with_ignore_case(haystack: &str, prefix: &str) -> bool {
    haystack.len() >= prefix.len()
        && haystack
            .chars()
            .zip(prefix.chars())
            .all(|(h, p)| h.eq_ignore_ascii_case(&p))
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
