//! Semantic palette matching. Display text, descriptions, and live values never rank.

use std::cmp::Reverse;

use super::palette::*;
use super::*;

#[derive(Clone)]
pub(crate) struct SearchMetadata {
    identity: String,
    stable_id: Option<&'static str>,
    scope: Option<Tab>,
    names: Vec<String>,
    group: Option<(Vec<String>, bool)>,
    scope_default: bool,
}

impl SearchMetadata {
    pub(crate) fn for_entry(entry: &PaletteEntry) -> Self {
        match entry {
            PaletteEntry::Control { tab, spec, .. } => {
                let mut names = vec![spec.search.name.to_string()];
                if let Some((_, field)) = spec.id.split_once('.') {
                    names.push(field.to_string());
                }
                if let Some(group) = spec.search.group {
                    names.push(format!("{} {}", group.name, group.member));
                }
                Self {
                    identity: format!("control:{}", spec.id),
                    stable_id: Some(spec.id),
                    scope: Some(*tab),
                    names,
                    group: spec
                        .search
                        .group
                        .map(|group| (vec![group.name.to_string()], group.is_default)),
                    scope_default: tab.level_id() == Some(spec.id),
                }
            }
            PaletteEntry::Module { tab, catalog_index } => {
                let kind = MODULE_CATALOG[*catalog_index];
                Self {
                    identity: format!("module:{}:{}", tab.search_names()[0], kind.id),
                    stable_id: None,
                    scope: Some(*tab),
                    names: kind
                        .search_names
                        .iter()
                        .map(|name| (*name).to_string())
                        .collect(),
                    group: Some((
                        kind.search_names
                            .iter()
                            .map(|name| (*name).to_string())
                            .collect(),
                        true,
                    )),
                    scope_default: false,
                }
            }
            PaletteEntry::ModuleControl {
                tab,
                spec,
                module_name: _,
                parameter: _,
                search_parameter,
                catalog_index,
            } => {
                let kind = MODULE_CATALOG[*catalog_index];
                Self {
                    identity: format!("control:{}", spec.id),
                    stable_id: Some(spec.id),
                    scope: Some(*tab),
                    names: std::iter::once(search_parameter.to_string())
                        .chain(
                            kind.search_names
                                .iter()
                                .map(|name| format!("{name} {search_parameter}")),
                        )
                        .collect(),
                    group: Some((
                        kind.search_names
                            .iter()
                            .map(|name| (*name).to_string())
                            .collect(),
                        module_slot_spec(
                            *tab,
                            parse_module_slot_id(spec.id).map_or(0, |(_, slot, _)| slot),
                            kind.collapsed_field(),
                        )
                        .is_some_and(|default| default.id == spec.id),
                    )),
                    scope_default: false,
                }
            }
            PaletteEntry::Operation(operation) => {
                let spec = operation.spec();
                Self {
                    identity: format!("operation:{operation:?}"),
                    stable_id: None,
                    scope: None,
                    names: std::iter::once(spec.label.to_string())
                        .chain(spec.aliases.iter().map(|alias| (*alias).to_string()))
                        .collect(),
                    group: None,
                    scope_default: false,
                }
            }
        }
    }
}

/// Ordered meaning classes precede context; spelling never substitutes for intent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Meaning {
    Identity,
    Named,
    Exact,
    Prefix,
    Fuzzy,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct SearchRank {
    meaning: Meaning,
    scope: u8,
    default: u8,
    spelling: Reverse<i32>,
    recent: usize,
    identity: String,
}

pub(crate) struct Query {
    raw: String,
    words: Vec<String>,
    /// Each word carries its recognized owning scopes, independent of catalog order.
    qualifiers: Vec<Vec<Tab>>,
}

fn words(text: &str) -> Vec<String> {
    text.split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

impl Query {
    pub(crate) fn new(query: &str) -> Self {
        let words = words(query);
        let qualifiers = words
            .iter()
            .map(|word| {
                let exact: Vec<_> = Tab::all()
                    .into_iter()
                    .filter(|tab| tab.search_names().contains(&word.as_str()))
                    .collect();
                if !exact.is_empty() {
                    return exact;
                }
                Tab::all()
                    .into_iter()
                    .filter(|tab| tab.search_names().iter().any(|name| name.starts_with(word)))
                    .collect()
            })
            .collect();
        Self {
            raw: query.trim().to_lowercase(),
            words,
            qualifiers,
        }
    }

    pub(crate) fn rank(
        &self,
        metadata: &SearchMetadata,
        current_tab: Tab,
        recent: &[&str],
    ) -> Option<SearchRank> {
        let identity = metadata
            .stable_id
            .is_some_and(|id| id.eq_ignore_ascii_case(&self.raw));
        // Exact operation aliases keep their complete meaning, including words that
        // also name a layer. The same rule applies to every entry's semantic name.
        let exact_name = metadata.names.iter().any(|name| words(name) == self.words);
        let mut requested = self.words.clone();
        let mut scope_quality = Meaning::Exact;
        let mut matched_scope = false;
        let mut explicit_scope = false;
        if !identity
            && !exact_name
            && let Some(scope) = metadata.scope
        {
            requested.clear();
            for (word, scopes) in self.words.iter().zip(&self.qualifiers) {
                if scopes.is_empty() {
                    requested.push(word.clone());
                } else if scopes.contains(&scope) {
                    matched_scope = true;
                    if scope.search_names().contains(&word.as_str()) {
                        explicit_scope = true;
                    } else {
                        scope_quality = Meaning::Prefix;
                    }
                } else if scopes
                    .iter()
                    .any(|scope| scope.search_names().contains(&word.as_str()))
                {
                    return None;
                } else {
                    // An incomplete layer name also competes as a semantic
                    // prefix: `c` can still find local Compression.
                    requested.push(word.clone());
                }
            }
        }
        let (meaning, spelling) = if identity {
            (Meaning::Identity, 0)
        } else if exact_name {
            (Meaning::Named, 0)
        } else if requested.is_empty() && matched_scope {
            (scope_quality, 0)
        } else {
            let candidates = metadata
                .names
                .iter()
                .chain(metadata.group.iter().flat_map(|(names, _)| names));
            let best = candidates
                .filter_map(|name| match_words(&requested, &words(name)))
                .min_by_key(|(meaning, score)| (*meaning, Reverse(*score)));
            let (meaning, spelling) = best?;
            (meaning.max(scope_quality), spelling)
        };
        let concept_default = if requested.is_empty() && matched_scope {
            metadata.scope_default
        } else if let Some((names, is_default)) = &metadata.group {
            let bare_concept = names.iter().any(|name| {
                let names = words(name);
                requested.len() == names.len()
                    && match_words(&requested, &names)
                        .is_some_and(|(meaning, _)| meaning <= Meaning::Prefix)
            });
            !bare_concept || *is_default
        } else {
            true
        };
        Some(SearchRank {
            meaning,
            scope: if metadata
                .scope
                .is_some_and(|scope| explicit_scope || scope == current_tab)
            {
                0
            } else {
                1
            },
            default: u8::from(!concept_default),
            spelling: Reverse(spelling),
            recent: recent
                .iter()
                .position(|id| Some(*id) == metadata.stable_id)
                .unwrap_or(usize::MAX),
            identity: metadata.identity.clone(),
        })
    }
}

/// Match words in either order, consuming each name word at most once. The
/// bounded catalog names are short; exploring assignments preserves an exact
/// word for a later query word instead of letting an earlier prefix steal it.
fn match_words(query: &[String], name: &[String]) -> Option<(Meaning, i32)> {
    fn assign(query: &[String], name: &[String], used: &mut [bool]) -> Option<(Meaning, i32)> {
        let Some((word, rest)) = query.split_first() else {
            return Some((Meaning::Exact, 0));
        };
        let mut best = None;
        for (index, candidate) in name.iter().enumerate() {
            if used[index] {
                continue;
            }
            let Some((score, _)) = fuzzy_score(word, candidate) else {
                continue;
            };
            let quality = if word == candidate {
                Meaning::Exact
            } else if candidate.starts_with(word) {
                Meaning::Prefix
            } else {
                Meaning::Fuzzy
            };
            used[index] = true;
            if let Some((tail_quality, tail_score)) = assign(rest, name, used) {
                let matched = (quality.max(tail_quality), score + tail_score);
                if best.is_none_or(|(quality, score)| {
                    (matched.0, Reverse(matched.1)) < (quality, Reverse(score))
                }) {
                    best = Some(matched);
                }
            }
            used[index] = false;
        }
        best
    }
    // Catalog tests bound semantic names to eight words. A longer query can
    // only use the linear fuzzy fallback, never enter the assignment search.
    let matched = (query.len() <= name.len())
        .then(|| assign(query, name, &mut vec![false; name.len()]))
        .flatten();
    matched.or_else(|| {
        fuzzy_score(&query.join(""), &name.join("")).map(|(score, _)| (Meaning::Fuzzy, score))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn winner(entries: &[PaletteEntry], tab: Tab, recent: &[&str], query: &str) -> String {
        let query = Query::new(query);
        entries
            .iter()
            .filter_map(|entry| {
                let metadata = SearchMetadata::for_entry(entry);
                query
                    .rank(&metadata, tab, recent)
                    .map(|rank| (rank, metadata.identity))
            })
            .min_by(|left, right| left.0.cmp(&right.0))
            .expect("the query has a winner")
            .1
    }

    const ADVERSE: &[&str] = &[
        "pad.midi_in",
        "arp.midi_gate_beats",
        "pad.midi_out",
        "lead.midi_in",
        "bass.decay_time",
        "master.bpm",
    ];

    #[test]
    fn palette_meaning_context_and_declared_defaults_cover_every_layer() {
        let entries = palette_entries();
        for tab in Tab::all() {
            for query in tab.search_names() {
                assert_eq!(
                    winner(&entries, Tab::Master, ADVERSE, query),
                    format!("control:{}", tab.level_id().unwrap()),
                    "{tab:?} {query}"
                );
            }
            for (index, module) in MODULE_CATALOG.iter().enumerate() {
                if !module_available_on(*module, tab) {
                    continue;
                }
                for query in module.search_names {
                    let expected = format!(
                        "module:{}:{}",
                        tab.search_names()[0],
                        MODULE_CATALOG[index].id
                    );
                    assert_eq!(
                        winner(&entries, tab, ADVERSE, query),
                        expected,
                        "{tab:?} {query}"
                    );
                }
            }
        }
        for tab in [Tab::Chords, Tab::Arp, Tab::Lead] {
            let namespace = tab.search_names()[0];
            for query in ["mi", "mid", "midi", "MIDI", "midi out", "midi_out"] {
                assert_eq!(
                    winner(&entries, tab, ADVERSE, query),
                    format!("control:{namespace}.midi_out"),
                    "{tab:?} {query}"
                );
            }
            for (query, member) in [("midi in", "in"), ("in midi", "in"), ("midi out", "out")] {
                assert_eq!(
                    winner(&entries, tab, ADVERSE, query),
                    format!("control:{namespace}.midi_{member}")
                );
            }
        }
        for (query, expected) in [
            ("midi gate", "control:lead.midi_gate_beats"),
            ("arp midi", "control:arp.midi_out"),
            ("arp midi in", "control:arp.midi_in"),
            ("midi in arp", "control:arp.midi_in"),
            ("pad.midi_out", "control:pad.midi_out"),
            ("bass", "control:bass.level"),
            ("bas", "control:bass.level"),
            ("bass.d", "control:bass.decay_time"),
            ("mute kick", "operation:Mix(MuteKick)"),
            ("kick mute", "operation:Mix(MuteKick)"),
            ("sc", "operation:Recipe(Sidechain)"),
            ("global swing", "module:master:swing"),
            ("drunken", "module:master:drunken"),
        ] {
            assert_eq!(
                winner(&entries, Tab::Lead, ADVERSE, query),
                expected,
                "{query}"
            );
        }
        assert_eq!(
            winner(&entries, Tab::Bass, &["arp.midi_out"], "midi"),
            "control:arp.midi_out",
            "unavailable local concepts retain remote destinations"
        );
    }

    #[test]
    fn palette_catalog_order_and_unrelated_entries_cannot_change_meaning() {
        let mut entries = palette_entries();
        let cases = [
            (Tab::Lead, "midi"),
            (Tab::Lead, "midi in arp"),
            (Tab::Bass, "delay"),
            (Tab::Master, "swing"),
            (Tab::Lead, "bass"),
            (Tab::Lead, "mute kick"),
        ];
        let expected: Vec<_> = cases
            .iter()
            .map(|(tab, query)| winner(&entries, *tab, ADVERSE, query))
            .collect();
        entries.reverse();
        for offset in [0, 7, 31] {
            entries.rotate_left(offset);
            for ((tab, query), expected) in cases.iter().zip(&expected) {
                assert_eq!(&winner(&entries, *tab, ADVERSE, query), expected);
            }
        }
        let mut extra = *spec_by_id("pad.level").unwrap();
        extra.id = "pad.unrelated";
        extra.search = ControlSearch {
            name: "Unrelated",
            group: None,
        };
        entries.insert(
            0,
            PaletteEntry::Control {
                tab: Tab::Chords,
                index_in_tab: 0,
                spec: Box::leak(Box::new(extra)),
            },
        );
        for ((tab, query), expected) in cases.iter().zip(&expected) {
            assert_eq!(&winner(&entries, *tab, ADVERSE, query), expected);
        }
    }

    #[test]
    fn palette_semantics_ignore_control_labels_and_operation_descriptions() {
        let mut entries = palette_entries();
        let before = winner(&entries, Tab::Lead, ADVERSE, "midi");
        for entry in &mut entries {
            if let PaletteEntry::Control { spec, .. } = entry {
                let mut edited = **spec;
                edited.label = "Different presentation · spaces and punctuation";
                *spec = Box::leak(Box::new(edited));
            }
        }
        assert_eq!(winner(&entries, Tab::Lead, ADVERSE, "midi"), before);
        // Description-only words never enter the search metadata at all.
        let operation = PaletteEntry::Operation(Operation::PlannedMute);
        assert!(operation.display_text().contains("visible"));
        assert!(
            Query::new("visible")
                .rank(&SearchMetadata::for_entry(&operation), Tab::Master, &[])
                .is_none()
        );
    }

    #[test]
    fn palette_catalog_defaults_and_exact_operation_aliases_are_unambiguous() {
        let entries = palette_entries();
        for tab in Tab::all() {
            let mut groups = std::collections::BTreeMap::<&str, Vec<&ControlSpec>>::new();
            for entry in &entries {
                if let PaletteEntry::Control {
                    tab: owner, spec, ..
                } = entry
                    && *owner == tab
                    && let Some(group) = spec.search.group
                {
                    assert!(!group.name.is_empty() && !group.member.is_empty());
                    groups.entry(group.name).or_default().push(spec);
                }
            }
            for (name, members) in groups {
                let defaults: Vec<_> = members
                    .iter()
                    .filter(|spec| spec.search.group.unwrap().is_default)
                    .collect();
                assert_eq!(defaults.len(), 1, "{tab:?} {name}");
                assert_eq!(
                    winner(&entries, tab, ADVERSE, name),
                    format!("control:{}", defaults[0].id)
                );
            }
        }
        let mut names = std::collections::BTreeMap::new();
        for operation in Operation::ALL {
            let spec = operation.spec();
            for name in std::iter::once(spec.label).chain(spec.aliases.iter().copied()) {
                let canonical = words(name).join(" ");
                if let Some(previous) = names.insert(canonical, operation) {
                    assert_eq!(previous, operation, "ambiguous operation alias {name}");
                }
                assert_eq!(
                    winner(&entries, Tab::Master, ADVERSE, name),
                    format!("operation:{operation:?}")
                );
            }
        }
    }

    #[test]
    fn palette_new_concept_uses_metadata_without_a_sorter_exception() {
        let member = |name, is_default| SearchMetadata {
            identity: format!("wobble:{name}"),
            stable_id: None,
            scope: Some(Tab::Bass),
            names: vec![format!("wobble {name}")],
            group: Some((vec!["wobble".to_string()], is_default)),
            scope_default: false,
        };
        let slow = member("slow", false);
        let fast = member("fast", true);
        let query = Query::new("wobble");
        assert!(query.rank(&fast, Tab::Bass, &[]) < query.rank(&slow, Tab::Bass, &[]));
        let explicit = Query::new("wobble slow");
        assert!(explicit.rank(&slow, Tab::Bass, &[]).is_some());
        assert!(explicit.rank(&fast, Tab::Bass, &[]).is_none());
    }
    #[test]
    fn palette_incomplete_scope_names_compete_with_local_semantic_prefixes() {
        let entries = palette_entries();
        for (tab, query, expected) in [
            (Tab::Master, "c", "module:master:compression"),
            (Tab::Bass, "l", "control:bass.interval_beats"),
            (Tab::Bass, "le", "control:bass.interval_beats"),
            (Tab::Bass, "lev", "control:bass.level"),
            (Tab::Bass, "length", "control:bass.interval_beats"),
            (Tab::Master, "bas", "control:bass.level"),
            (Tab::Lead, "ar midi", "control:arp.midi_out"),
        ] {
            assert_eq!(
                winner(&entries, tab, ADVERSE, query),
                expected,
                "{tab:?} {query}"
            );
        }
    }

    #[test]
    fn palette_scoped_members_keep_static_ids_and_ignore_display_formatting() {
        for tab in Tab::all() {
            for (catalog_index, module) in MODULE_CATALOG.iter().enumerate() {
                if !module_available_on(*module, tab) {
                    continue;
                }
                let scope = ModuleScope {
                    tab,
                    slot: 0,
                    catalog_index,
                };
                let palette = PaletteState::new(tab, &[], Some(scope));
                let mut entries: Vec<_> = palette
                    .matches
                    .iter()
                    .map(|hit| palette.entry(hit.entry_index).clone())
                    .collect();
                let default = module_slot_spec(tab, 0, module.collapsed_field())
                    .unwrap()
                    .id;
                assert_eq!(
                    winner(&entries, tab, ADVERSE, module.search_names[0]),
                    format!("control:{default}")
                );
                for parameter in module.parameters() {
                    let id = module_slot_spec(tab, 0, parameter.field).unwrap().id;
                    let before = winner(&entries, tab, ADVERSE, parameter.search_name);
                    assert_eq!(before, format!("control:{id}"));
                    assert_eq!(winner(&entries, tab, ADVERSE, id), before);
                }
                for entry in &mut entries {
                    if let PaletteEntry::ModuleControl {
                        module_name,
                        parameter,
                        ..
                    } = entry
                    {
                        *module_name = "Changed module formatting";
                        *parameter = "Changed parameter formatting";
                    }
                }
                for parameter in module.parameters() {
                    let id = module_slot_spec(tab, 0, parameter.field).unwrap().id;
                    assert_eq!(
                        winner(&entries, tab, ADVERSE, parameter.search_name),
                        format!("control:{id}")
                    );
                }
                let remote = PaletteState::resolve(tab, &[], Some(scope), "lead midi");
                assert!(
                    remote.matches.iter().all(|hit| matches!(
                        remote.entry(hit.entry_index),
                        PaletteEntry::Operation(_)
                    )),
                    "module detail never escapes into remote controls"
                );
            }
        }
    }

    #[test]
    fn palette_semantic_catalog_bounds_word_assignment_work() {
        let mut entries = palette_entries();
        for (catalog_index, _) in MODULE_CATALOG.iter().enumerate() {
            let palette = PaletteState::new(
                Tab::Bass,
                &[],
                Some(ModuleScope {
                    tab: Tab::Bass,
                    slot: 0,
                    catalog_index,
                }),
            );
            entries.extend(
                palette
                    .matches
                    .iter()
                    .map(|hit| palette.entry(hit.entry_index).clone()),
            );
        }
        for entry in &entries {
            let metadata = SearchMetadata::for_entry(entry);
            for name in metadata
                .names
                .iter()
                .chain(metadata.group.iter().flat_map(|(names, _)| names))
            {
                assert!(
                    words(name).len() <= 8,
                    "semantic name exceeds bounded assignment contract: {name}"
                );
            }
        }
        let long_query = Query::new(&"midi ".repeat(10_000));
        assert!(
            long_query
                .rank(&SearchMetadata::for_entry(&entries[0]), Tab::Bass, &[])
                .is_none()
        );
    }
}
