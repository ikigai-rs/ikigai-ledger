//! Ordering of the shared listing, before its page is selected.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use ikigai_core::{Error, Result};

use crate::{ledger::Ledger, model::Item};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Key {
    Priority,
    Created,
    Modified,
    Number,
    Landing,
}

pub(super) struct Sort(Vec<(Key, bool)>);

impl Sort {
    pub(super) fn parse(value: Option<&str>) -> Result<Self> {
        let mut keys = Vec::new();
        for part in value.unwrap_or("-modified").split(',') {
            let part = part.trim();
            let (name, descending) = part
                .strip_prefix('-')
                .map_or((part, false), |name| (name, true));
            let key = match name {
                "priority" => Key::Priority,
                "created" => Key::Created,
                "modified" => Key::Modified,
                "number" => Key::Number,
                "landing" => Key::Landing,
                _ => return Err(Error::InvalidArgument {
                    name: "sort".to_string(),
                    detail: format!("`{part}` is not one of priority, created, modified, number, landing; use a comma-separated list with - before a key for descending order"),
                }),
            };
            keys.push((key, descending));
        }
        Ok(Self(keys))
    }

    pub(super) fn is_default(&self) -> bool {
        self.0 == [(Key::Modified, true)]
    }

    pub(super) fn has_landing(&self) -> bool {
        self.0.iter().any(|(key, _)| *key == Key::Landing)
    }

    pub(super) fn apply(&self, entries: &mut [(usize, Item)], ledgers: &[Ledger]) -> Result<()> {
        let ranks = if self.has_landing() {
            landing_ranks(entries, ledgers)?
        } else {
            BTreeMap::new()
        };
        entries.sort_by(|(ai, a), (bi, b)| {
            for (key, descending) in &self.0 {
                let order = match key {
                    // Unset stays last in either direction, just as in next.
                    Key::Priority => match (a.priority, b.priority) {
                        (None, None) => Ordering::Equal,
                        (None, _) => return Ordering::Greater,
                        (_, None) => return Ordering::Less,
                        (Some(a), Some(b)) => a.cmp(&b),
                    },
                    Key::Created => a.created.cmp(&b.created),
                    Key::Modified => a.modified.cmp(&b.modified),
                    Key::Number => a.number.cmp(&b.number),
                    Key::Landing => ranks[&a.iri].cmp(&ranks[&b.iri]),
                };
                let order = if *descending { order.reverse() } else { order };
                if order != Ordering::Equal {
                    return order;
                }
            }
            // Preserve the listing's historical tie breaks, including -modified.
            b.number
                .cmp(&a.number)
                .then(ledgers[*ai].name().cmp(ledgers[*bi].name()))
                .then(a.iri.cmp(&b.iri))
        });
        Ok(())
    }
}

/// Kahn's algorithm over the matching readable items. The ready queue is ordered by
/// priority, ledger, number and IRI; no recursion or repeated scan of the remaining set.
fn landing_ranks(entries: &[(usize, Item)], ledgers: &[Ledger]) -> Result<BTreeMap<String, usize>> {
    let indices: BTreeMap<_, _> = entries
        .iter()
        .enumerate()
        .map(|(index, (_, item))| (item.iri.as_str(), index))
        .collect();
    let mut incoming = vec![0usize; entries.len()];
    let mut edges = vec![Vec::new(); entries.len()];
    for (index, (_, item)) in entries.iter().enumerate() {
        for target in &item.blocks {
            if let Some(&target) = indices.get(target.as_str()) {
                edges[index].push(target);
                incoming[target] += 1;
            }
        }
    }
    let ready_key = |index: usize| {
        let (ledger, item) = &entries[index];
        (
            item.priority.unwrap_or(i64::MAX),
            ledgers[*ledger].name(),
            item.number,
            item.iri.as_str(),
            index,
        )
    };
    let mut ready = BTreeSet::new();
    for (index, degree) in incoming.iter().enumerate() {
        if *degree == 0 {
            ready.insert(ready_key(index));
        }
    }
    let mut ranks = BTreeMap::new();
    while let Some((_, _, _, _, index)) = ready.pop_first() {
        ranks.insert(entries[index].1.iri.clone(), ranks.len());
        for &target in &edges[index] {
            incoming[target] -= 1;
            if incoming[target] == 0 {
                ready.insert(ready_key(target));
            }
        }
    }
    if ranks.len() != entries.len() {
        let affected = entries
            .iter()
            .enumerate()
            .filter(|(index, _)| incoming[*index] != 0)
            .map(|(_, (_, item))| item.short())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(Error::InvalidArgument {
            name: "sort".to_string(),
            detail: format!("landing order cannot be computed because the matching readable blocks graph has a cycle; these items are in or depend on it: {affected}. Remove a blocks link at urn:iki:ledger:link or choose priority, created, modified or number"),
        });
    }
    Ok(ranks)
}
