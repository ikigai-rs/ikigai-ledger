//! `urn:iki:ledger:next`: the ready set, the cycle refusal, and an answer that can say
//! why.
//!
//! # The shape
//!
//! 1. Load the open items (the caller's label / `about` filters applied **in the
//!    query**).
//! 2. Build the `blocks` graph over them and **refuse if it has a cycle**, naming the
//!    cycle — because a cycle makes the ready set silently empty, which is
//!    indistinguishable from a finished backlog.
//! 3. Partition: ready (unclaimed, not deferred, no open blocker) versus excluded, and
//!    keep the exclusion reasons.
//! 4. Compute leverage — how many open items each candidate transitively unblocks.
//! 5. Hand the ready set to the [`OrderingPolicy`].
//!
//! ⚠ **Steps 3 and 4 are in Rust, not in SPARQL, and that is a choice.** The three
//! readiness predicates are perfectly expressible as query clauses — [`Filter`] has them,
//! and `urn:iki:ledger:items` uses them — but a query that filters them out cannot then
//! say *why not that one*, and "why not" is the question a selection gets asked. The cost
//! is that this reads every open item into memory once per call; at a backlog's scale
//! that is nothing, and the honest trigger for revisiting it is an open set large enough
//! that the read shows up, not a rule of thumb.
//!
//! ⚠ **Selection OFFERS; it never authorizes.** `next` naming an item is an affordance.
//! Acting on it — claiming it, closing it, changing it — still requires the actor's own
//! capability, checked by the kernel at that action. Nothing here mints authority, and
//! nothing downstream should read a ranking as one.

use std::collections::{BTreeMap, BTreeSet};

use ikigai_core::{Error, Result};
use oxrdf::{Graph, NamedNode, Triple};

use crate::ledger::Ledger;
use crate::model::{self, Filter, Item};
use crate::policy::{Candidate, OrderingPolicy, Ranked, SelectionInputs};
use crate::sparql::{self, StoreClient};
use crate::vocabulary as v;

/// Why the ready set would not offer an open item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Excluded {
    /// Someone holds it.
    Claimed(String),
    /// Deliberately not now.
    Deferred,
    /// One or more open items block it.
    Blocked(Vec<i64>),
}

impl Excluded {
    /// The sentence a human reads and the `ledger:reason` a graph carries.
    pub fn reason(&self) -> String {
        match self {
            Excluded::Claimed(holder) => format!("claimed by {holder}"),
            Excluded::Deferred => "deferred".to_string(),
            Excluded::Blocked(numbers) => format!(
                "blocked by {}",
                numbers
                    .iter()
                    .map(|n| format!("#{n}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
}

/// The deterministic half: what is ready, what is not, and why not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadySet {
    /// The items a policy may order.
    pub ready: Vec<Item>,
    /// The open items the query refused to offer, with the reason.
    pub excluded: Vec<(Item, Excluded)>,
    /// Leverage per item IRI: how many open items it transitively unblocks.
    pub leverage: BTreeMap<String, (usize, usize)>,
}

/// Compute the ready set over the open items a filter admits.
///
/// # Errors
///
/// [`Error::Endpoint`] naming the cycle when the `blocks` graph over open items is not
/// a DAG. A cycle is not exotic — it is what happens when someone links carelessly — and
/// the alternative answer, "nothing is ready", is indistinguishable from having finished.
pub async fn ready(client: &StoreClient<'_, '_>, filter: &Filter) -> Result<ReadySet> {
    let mut pool_filter = filter.clone();
    pool_filter.status = model::Status::Open;
    // Readiness itself is evaluated below, in Rust, so the exclusions can be explained.
    pool_filter.holder = model::Holder::Any;
    pool_filter.deferred = model::Deferred::Include;
    let open = model::load_items(client, &pool_filter).await?;

    let open_iris: BTreeSet<&str> = open.iter().map(|i| i.iri.as_str()).collect();
    let numbers: BTreeMap<&str, i64> = open.iter().map(|i| (i.iri.as_str(), i.number)).collect();
    // Edges: blocker → blocked, restricted to open items. A closed blocker blocks
    // nothing, which is what makes closing an item the act that releases work.
    let edges: BTreeMap<&str, Vec<&str>> = open
        .iter()
        .map(|item| {
            let targets = item
                .blocks
                .iter()
                .map(String::as_str)
                .filter(|t| open_iris.contains(*t))
                .collect();
            (item.iri.as_str(), targets)
        })
        .collect();

    if let Some(cycle) = find_cycle(&edges) {
        let ledger = client.ledger();
        return Err(Error::Endpoint(format!(
            "the `blocks` graph has a cycle and the ready set cannot be computed: {}. \
             Refusing rather than answering \"nothing is ready\", which is \
             indistinguishable from a finished backlog. Break it with \
             `delete {} item=<a> content=<b> type=blocks`.",
            cycle
                .iter()
                .map(|iri| numbers
                    .get(iri.as_str())
                    .map(|n| ledger.number(*n))
                    .unwrap_or_else(|| (*iri).to_string()))
                .collect::<Vec<_>>()
                .join(" → "),
            ledger.resource("link")
        )));
    }

    let blockers: BTreeMap<String, Vec<i64>> = {
        let mut map: BTreeMap<String, Vec<i64>> = BTreeMap::new();
        for (blocker, targets) in &edges {
            for target in targets {
                map.entry((*target).to_string())
                    .or_default()
                    .push(*numbers.get(blocker).unwrap_or(&0));
            }
        }
        for list in map.values_mut() {
            list.sort_unstable();
        }
        map
    };

    let leverage: BTreeMap<String, (usize, usize)> = open
        .iter()
        .map(|item| {
            let direct = edges
                .get(item.iri.as_str())
                .map(|t| t.len())
                .unwrap_or_default();
            (item.iri.clone(), (reachable(&edges, &item.iri), direct))
        })
        .collect();

    // The borrows of `open` end here, so the items can be moved into the partition below.
    drop(edges);
    drop(numbers);
    drop(open_iris);

    let mut ready = Vec::new();
    let mut excluded = Vec::new();
    for item in open {
        let why = if let Some(holder) = item.claimed_by.clone() {
            Some(Excluded::Claimed(holder))
        } else if item.deferred {
            Some(Excluded::Deferred)
        } else {
            blockers
                .get(&item.iri)
                .map(|numbers| Excluded::Blocked(numbers.clone()))
        };
        match why {
            Some(why) => excluded.push((item, why)),
            None => ready.push(item),
        }
    }
    Ok(ReadySet {
        ready,
        excluded,
        leverage,
    })
}

/// One run of `next`: which policy, over what, in what order, and what it refused.
#[derive(Debug, Clone)]
pub struct Selection {
    /// The ledger this selection is over. A selection is only true of one ledger, and
    /// its skolemized IRI carries the name so two ledgers' selections never collide.
    pub ledger: Ledger,
    /// The policy that ranked it.
    pub policy: String,
    /// What that policy weighs, in order.
    pub weighs: Vec<String>,
    /// When this ran (ms since the epoch) — a selection is only true of its moment.
    pub generated_at: u64,
    /// The ranking, best first, paired with the item.
    pub ranked: Vec<(Ranked, Item)>,
    /// How many items were ready before the limit was applied.
    pub ready_count: usize,
    /// The open items the ready query refused, with reasons.
    pub excluded: Vec<(Item, Excluded)>,
}

/// Rank a ready set. `limit` is applied **after** ranking, never before — kata's
/// `ready --limit N` truncates by recency and then ranks, so "the highest-priority ready
/// issue" silently means "the best of the N most recently touched".
pub fn rank(
    ledger: &Ledger,
    set: ReadySet,
    policy: &dyn OrderingPolicy,
    now: u64,
    limit: usize,
) -> Selection {
    let candidates: Vec<Candidate> = set
        .ready
        .iter()
        .map(|item| {
            let (leverage, blocks_direct) = set.leverage.get(&item.iri).copied().unwrap_or((0, 0));
            Candidate {
                iri: item.iri.clone(),
                number: item.number,
                title: item.title.clone(),
                kind: item.kind.clone(),
                priority: item.priority,
                created: item.created,
                modified: item.modified,
                labels: item.labels.clone(),
                about: item.about.clone(),
                leverage,
                blocks_direct,
            }
        })
        .collect();
    let ready_count = candidates.len();
    let by_iri: BTreeMap<String, Item> = set
        .ready
        .into_iter()
        .map(|item| (item.iri.clone(), item))
        .collect();
    let ranked = policy
        .order(&SelectionInputs { candidates, now })
        .into_iter()
        .filter_map(|r| by_iri.get(&r.iri).cloned().map(|item| (r, item)))
        .take(if limit == 0 { usize::MAX } else { limit })
        .collect();
    Selection {
        ledger: ledger.clone(),
        policy: policy.name().to_string(),
        weighs: policy.weighs().into_iter().map(str::to_string).collect(),
        generated_at: now,
        ranked,
        ready_count,
        excluded: set.excluded,
    }
}

impl Selection {
    /// The human face: the answer first, then what it beat, then what was not offered.
    pub fn plain(&self) -> String {
        if self.ranked.is_empty() {
            return format!(
                "nothing is ready ({} open item(s) excluded: {})\npolicy: {}\n",
                self.excluded.len(),
                if self.excluded.is_empty() {
                    "the ledger has no open items matching the filter".to_string()
                } else {
                    self.excluded
                        .iter()
                        .map(|(item, why)| format!("#{} {}", item.number, why.reason()))
                        .collect::<Vec<_>>()
                        .join("; ")
                },
                self.policy
            );
        }
        let mut out = String::new();
        for (rank, (ranked, item)) in self.ranked.iter().enumerate() {
            out.push_str(&format!("{:>2}. {}\n", rank + 1, item.line()));
            out.push_str(&format!(
                "    {} — {}\n",
                ranked.score,
                ranked.because.join("; ")
            ));
        }
        out.push_str(&format!(
            "\npolicy: {} (weighs {})\nready: {}   excluded: {}\n",
            self.policy,
            self.weighs.join(", "),
            self.ready_count,
            self.excluded.len()
        ));
        for (item, why) in &self.excluded {
            out.push_str(&format!("  not {}: {}\n", item.short(), why.reason()));
        }
        out
    }

    /// The graph face: the reasoning, skolemized, so "why this one" is queryable and not
    /// just printable.
    pub fn turtle(&self) -> Result<Vec<u8>> {
        let selection = self.ledger.selection(self.generated_at);
        let subject = NamedNode::new(&selection)
            .map_err(|e| Error::Endpoint(format!("selection IRI: {e}")))?;
        let mut graph = Graph::new();
        let push = |graph: &mut Graph, s: &NamedNode, p: &str, o: oxrdf::Term| {
            if let Ok(predicate) = NamedNode::new(p) {
                graph.insert(&Triple::new(s.clone(), predicate, o));
            }
        };
        push(
            &mut graph,
            &subject,
            v::ext::TYPE,
            model::named(v::SELECTION_CLASS),
        );
        push(
            &mut graph,
            &subject,
            v::ext::GENERATED_AT,
            model::typed(&sparql::iso8601(self.generated_at), v::ext::XSD_DATETIME),
        );
        push(
            &mut graph,
            &subject,
            v::POLICY,
            model::named(&format!("{}{}", v::iri::POLICY, self.policy)),
        );
        push(
            &mut graph,
            &subject,
            v::READY_COUNT,
            model::typed(&self.ready_count.to_string(), v::ext::XSD_INTEGER),
        );
        push(
            &mut graph,
            &subject,
            v::EXCLUDED_COUNT,
            model::typed(&self.excluded.len().to_string(), v::ext::XSD_INTEGER),
        );

        for (position, (ranked, item)) in self.ranked.iter().enumerate() {
            let node = NamedNode::new(format!("{selection}:rank:{}", position + 1))
                .map_err(|e| Error::Endpoint(format!("ranking IRI: {e}")))?;
            push(&mut graph, &subject, v::RANKING, node.clone().into());
            push(
                &mut graph,
                &node,
                v::ext::TYPE,
                model::named(v::RANKING_CLASS),
            );
            push(
                &mut graph,
                &node,
                v::RANK,
                model::typed(&(position + 1).to_string(), v::ext::XSD_INTEGER),
            );
            push(&mut graph, &node, v::ITEM, model::named(&item.iri));
            push(&mut graph, &node, v::SCORE, model::plain(&ranked.score));
            for reason in &ranked.because {
                push(&mut graph, &node, v::BECAUSE, model::plain(reason));
            }
            // The item itself, so the graph answers "what is it" without a second read.
            item.triples(&mut graph);
        }

        for (position, (item, why)) in self.excluded.iter().enumerate() {
            let node = NamedNode::new(format!("{selection}:excluded:{}", position + 1))
                .map_err(|e| Error::Endpoint(format!("exclusion IRI: {e}")))?;
            push(&mut graph, &subject, v::EXCLUDED, node.clone().into());
            push(
                &mut graph,
                &node,
                v::ext::TYPE,
                model::named(v::EXCLUSION_CLASS),
            );
            push(&mut graph, &node, v::ITEM, model::named(&item.iri));
            push(&mut graph, &node, v::REASON, model::plain(&why.reason()));
        }
        model::turtle(&graph)
    }
}

/// The `blocks` subgraph's first cycle, as the path that closes it, or `None` when it is
/// a DAG.
///
/// ⚠ Walked by hand rather than asked of SPARQL, and the ecosystem has met this shape
/// before: the process-vocabulary shapes record that a cycle through a name hop is
/// invisible to property paths. `?a ledger:blocks+ ?a` would answer *whether* there is a
/// cycle over a direct predicate but never *which* — and a refusal that cannot name the
/// cycle leaves the operator exactly where the silent empty answer did.
fn find_cycle(edges: &BTreeMap<&str, Vec<&str>>) -> Option<Vec<String>> {
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        Open,
        Done,
    }
    let mut marks: BTreeMap<&str, Mark> = BTreeMap::new();
    let mut stack: Vec<&str> = Vec::new();

    fn walk<'a>(
        node: &'a str,
        edges: &BTreeMap<&'a str, Vec<&'a str>>,
        marks: &mut BTreeMap<&'a str, Mark>,
        stack: &mut Vec<&'a str>,
    ) -> Option<Vec<String>> {
        match marks.get(node) {
            Some(Mark::Done) => return None,
            Some(Mark::Open) => {
                // The cycle is the suffix of the stack from this node, closed by it.
                let at = stack.iter().position(|n| *n == node)?;
                let mut cycle: Vec<String> = stack[at..].iter().map(|n| n.to_string()).collect();
                cycle.push(node.to_string());
                return Some(cycle);
            }
            None => {}
        }
        marks.insert(node, Mark::Open);
        stack.push(node);
        for next in edges.get(node).into_iter().flatten() {
            if let Some(cycle) = walk(next, edges, marks, stack) {
                return Some(cycle);
            }
        }
        stack.pop();
        marks.insert(node, Mark::Done);
        None
    }

    // Sorted iteration: the cycle reported for a given graph is always the same one.
    for node in edges.keys() {
        if let Some(cycle) = walk(node, edges, &mut marks, &mut stack) {
            return Some(cycle);
        }
    }
    None
}

/// How many distinct open items this one blocks, transitively (not counting itself).
fn reachable(edges: &BTreeMap<&str, Vec<&str>>, from: &str) -> usize {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut queue: Vec<&str> = edges.get(from).cloned().unwrap_or_default();
    while let Some(node) = queue.pop() {
        if node == from || !seen.insert(node) {
            continue;
        }
        queue.extend(edges.get(node).cloned().unwrap_or_default());
    }
    seen.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edges<'a>(pairs: &[(&'a str, &'a str)]) -> BTreeMap<&'a str, Vec<&'a str>> {
        let mut map: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (from, to) in pairs {
            map.entry(from).or_default().push(to);
            map.entry(to).or_default();
        }
        map
    }

    #[test]
    fn a_dag_has_no_cycle() {
        assert!(find_cycle(&edges(&[("a", "b"), ("b", "c"), ("a", "c")])).is_none());
    }

    /// ★ The three-item cycle the brief names: not exotic, and silently fatal to a ready
    /// set that does not look for it.
    #[test]
    fn a_three_item_cycle_is_found_and_named() {
        let cycle = find_cycle(&edges(&[("a", "b"), ("b", "c"), ("c", "a")])).expect("a cycle");
        assert_eq!(cycle.first(), cycle.last());
        assert!(cycle.contains(&"a".to_string()));
        assert!(cycle.contains(&"b".to_string()));
        assert!(cycle.contains(&"c".to_string()));
    }

    #[test]
    fn a_self_block_is_a_cycle() {
        assert!(find_cycle(&edges(&[("a", "a")])).is_some());
    }

    #[test]
    fn leverage_counts_the_whole_downstream_not_just_the_next_hop() {
        let graph = edges(&[("a", "b"), ("b", "c"), ("c", "d")]);
        assert_eq!(reachable(&graph, "a"), 3);
        assert_eq!(reachable(&graph, "c"), 1);
        assert_eq!(reachable(&graph, "d"), 0);
    }

    #[test]
    fn a_diamond_counts_each_item_once() {
        let graph = edges(&[("a", "b"), ("a", "c"), ("b", "d"), ("c", "d")]);
        assert_eq!(reachable(&graph, "a"), 3);
    }
}
