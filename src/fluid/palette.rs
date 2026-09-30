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
/// Entries derive only from registry/catalog/recipe constants and scope: the
/// interaction kernel and the renderer each build this list independently and
/// confirm works by index, so anything that made the list depend on live
/// controls could desync them and jump to the wrong control. Live state is
/// resolved where it exists — in the adapter, and in the value column.
pub(crate) enum PaletteEntry {
    Capture(CaptureAction),
    MixAction(super::mix_action::MixAction),
    Recipe(super::recipe::RecipeId),
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
    },
}

impl PaletteEntry {
    /// Text the query matches against. Must stay a pure function of the
    /// entry, for the reason on the enum.
    pub(crate) fn haystack(&self) -> String {
        match self {
            Self::Capture(action) => format!("{} · {}", action.name(), action.description()),
            Self::MixAction(action) => {
                format!("{} · {}", action.name(), action.aliases().join(" "))
            }
            Self::Recipe(id) => {
                let recipe = id.recipe();
                format!("{} · {}", self.display_text(), recipe.aliases.join(" "))
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
            Self::MixAction(action) => action.name().to_string(),
            Self::Recipe(id) => format!("{} · {}", id.recipe().name, id.recipe().description),
            _ => self.haystack(),
        }
    }

    fn match_query(&self, query: &str) -> Option<(i32, Vec<usize>)> {
        if let Self::MixAction(action) = self
            && action
                .aliases()
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(query))
        {
            return Some((i32::MAX, (0..action.name().chars().count()).collect()));
        }
        if let Self::Recipe(id) = self
            && id
                .recipe()
                .aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(query))
        {
            // An exact alias wins over scattered matches. Highlight the name,
            // never alias offsets that have no corresponding display text.
            return Some((i32::MAX, (0..id.recipe().name.chars().count()).collect()));
        }
        fuzzy_score(query, &self.haystack()).map(|(score, hits)| {
            let display_len = self.display_text().chars().count();
            (
                score,
                hits.into_iter().filter(|&hit| hit < display_len).collect(),
            )
        })
    }

    pub(crate) fn spec(&self) -> Option<&'static ControlSpec> {
        match self {
            Self::Control { spec, .. } | Self::ModuleControl { spec, .. } => Some(spec),
            Self::Module { .. } | Self::Recipe(_) | Self::MixAction(_) | Self::Capture(_) => None,
        }
    }

    pub(crate) fn id(&self) -> Option<&'static str> {
        self.spec().map(|spec| spec.id)
    }

    /// Value column. This one may read live controls: it is rendered, never
    /// matched against, so it cannot affect indices.
    pub(crate) fn value(&self, c: &FluidControls) -> String {
        match self {
            Self::Capture(_) => "selected knob".to_string(),
            Self::MixAction(action) => action.description().to_string(),
            Self::Recipe(_) => "add lane".to_string(),
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
    entries.extend(
        super::recipe::RECIPES
            .iter()
            .map(|recipe| PaletteEntry::Recipe(recipe.id)),
    );
    entries.extend(
        super::mix_action::MIX_ACTIONS
            .iter()
            .copied()
            .map(PaletteEntry::MixAction),
    );
    entries.extend(CaptureAction::ALL.into_iter().map(PaletteEntry::Capture));
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
                if let Some((score, hits)) = entry.match_query(&self.query) {
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
                    .then_with(|| {
                        matching_module_context_rank(entries, left.entry_index, query, current_tab)
                            .cmp(&matching_module_context_rank(
                                entries,
                                right.entry_index,
                                query,
                                current_tab,
                            ))
                    })
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

/// A canonical module query belongs first to the module on the page in front
/// of the player. Fuzzy-score ordering alone makes "Swing" on Pads outrank
/// "Global Swing · Master" because the latter's first match sits later in its
/// label, even while Master is the active page.
fn matching_module_context_rank(
    entries: &[PaletteEntry],
    entry: usize,
    query: &str,
    current_tab: Tab,
) -> u8 {
    match entries[entry] {
        PaletteEntry::Module { tab, catalog_index }
            if starts_with_ignore_case(MODULE_CATALOG[catalog_index].id, query) =>
        {
            if tab == current_tab { 0 } else { 1 }
        }
        _ => 2,
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
            })
        })
        .chain(
            super::recipe::RECIPES
                .iter()
                .map(|recipe| PaletteEntry::Recipe(recipe.id)),
        )
        .chain(
            super::mix_action::MIX_ACTIONS
                .iter()
                .copied()
                .map(PaletteEntry::MixAction),
        )
        .chain(CaptureAction::ALL.into_iter().map(PaletteEntry::Capture))
        .collect()
}

/// A layer's primary control outranks everything while the query is still
/// just that layer's name, so typing `bass` always lands on Bass Level
/// whatever the MRU holds. Matches the id namespace (`bass.`) and the tab
/// name (`Bass`) alike, since `Tab::Chords` is spelled `Pads` but owns
/// `pad.*`. Returns the tab's discriminant so ambiguous prefixes (`m` hits
/// both Arp and Master) stay deterministically ordered.
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
        PaletteEntry::Recipe(_) | PaletteEntry::MixAction(_) | PaletteEntry::Capture(_) => None,
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
