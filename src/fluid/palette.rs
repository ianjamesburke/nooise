use super::*;

// ============================================================
// Control palette
//
// `/` opens a fuzzy-find prompt over every control id in the registry.
// Typing filters; Tab locks the highlighted control; digits typed after the
// lock become a value. Enter stages the edit (or jumps when no value was
// typed); Enter on an empty prompt commits every staged edit at once, and
// Ctrl+B commits them on the next bar downbeat instead. The registry tables
// stay the single source of truth — the palette only builds a flat address
// space over them, exactly one entry per unique control id at its native
// (deepest) editing surface.
// ============================================================

/// One addressable control: the tab that natively owns it plus its index in
/// that tab's registry table, so a jump can restore the exact row (including
/// the Chords/Master drill state mapped by `chords_drill_for_index` /
/// `master_drill_for_index`).
pub(crate) struct PaletteEntry {
    pub(crate) tab: Tab,
    pub(crate) index_in_tab: usize,
    pub(crate) spec: &'static ControlSpec,
}

impl PaletteEntry {
    /// Display string the query matches against: id first (so dotted paths
    /// like `bass.decay_time` are directly typeable), then tab and label.
    pub(crate) fn haystack(&self) -> String {
        format!(
            "{} \u{b7} {} \u{b7} {}",
            self.spec.id,
            self.tab.name(),
            self.spec.label
        )
    }
}

/// Flat address space over every unique control id. Duplicated placements
/// (e.g. `pad.level` on both Master and Chords) collapse to the owning tab so
/// a jump always lands on the control's deepest editing surface.
pub(crate) fn palette_entries() -> Vec<PaletteEntry> {
    let mut entries: Vec<PaletteEntry> = Vec::new();
    for tab in Tab::all() {
        for (index_in_tab, spec) in tab_specs(tab).iter().enumerate() {
            if tab_owning_control(spec.id) == Some(tab)
                && !entries.iter().any(|e| e.spec.id == spec.id)
            {
                entries.push(PaletteEntry {
                    tab,
                    index_in_tab,
                    spec,
                });
            }
        }
    }
    entries
}

/// One fuzzy candidate: which entry, how well it scored, and which characters
/// of its haystack matched (for highlighting).
pub(crate) struct PaletteMatch {
    pub(crate) entry: usize,
    pub(crate) score: i32,
    pub(crate) hits: Vec<usize>,
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
    pub(crate) fn new(current_tab: Tab, recent: &[&'static str]) -> Self {
        let mut state = Self {
            entries: palette_entries(),
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
            for entry in 0..self.entries.len() {
                self.matches.push(PaletteMatch {
                    entry,
                    score: 0,
                    hits: Vec::new(),
                });
            }
            self.sort_by_context();
        } else {
            for (entry, e) in self.entries.iter().enumerate() {
                if let Some((score, hits)) = fuzzy_score(&self.query, &e.haystack()) {
                    self.matches.push(PaletteMatch { entry, score, hits });
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
                layer_primary_rank(entries[entry].spec.id, query).unwrap_or(usize::MAX)
            };
            self.matches.sort_by(|left, right| {
                primary_key(left.entry)
                    .cmp(&primary_key(right.entry))
                    .then_with(|| right.score.cmp(&left.score))
                    .then_with(|| {
                        context_rank(entries, left.entry, current_tab, recent).cmp(&context_rank(
                            entries,
                            right.entry,
                            current_tab,
                            recent,
                        ))
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
            .sort_by_key(|m| context_rank(entries, m.entry, current_tab, recent));
    }
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
    let recent_rank = recent.iter().position(|&id| id == palette_entry.spec.id);
    let page_index = tab_specs(current_tab)
        .iter()
        .position(|spec| spec.id == palette_entry.spec.id);
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
/// deep the first match sits, so "rev" ranks "Reverb Mix" above a scattered
/// hit inside another label.
pub(crate) fn fuzzy_score(query: &str, haystack: &str) -> Option<(i32, Vec<usize>)> {
    let needle: Vec<char> = query.chars().flat_map(|c| c.to_lowercase()).collect();
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
        let idx = found?;
        // Contiguous with previous match.
        if prev_hit == Some(idx.wrapping_sub(1)) {
            score += 8;
        }
        // Word-start bonus (start of string or preceded by a separator).
        let at_word_start =
            idx == 0 || matches!(hay.get(idx - 1), Some(' ') | Some('\u{b7}') | Some('.'));
        if at_word_start {
            score += 6;
        }
        if hits.is_empty() {
            // Penalise a deep first match so shallow, leading matches win.
            score -= idx as i32;
        }
        hits.push(idx);
        prev_hit = Some(idx);
        hay_i = idx + 1;
    }
    // Reward matching a larger fraction of the label.
    score += (needle.len() as i32) * 2;
    Some((score, hits))
}
