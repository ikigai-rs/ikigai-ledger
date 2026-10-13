//! `urn:iki:ledger:next`: the ready set, the cycle refusal, and an answer that can say
//! why.
//!
//! # The shape
//!
//! 1. Load **every** open item — unfiltered and unbounded.
//! 2. Build the `blocks` graph over them and **refuse if it has a cycle**, naming the
//!    cycle — because a cycle makes the ready set silently empty, which is
//!    indistinguishable from a finished backlog.
//! 3. Compute leverage — how many open items each one transitively unblocks.
//! 4. Narrow to the caller's filters (level, labels, `about`), evaluated **in the query**
//!    by the same clauses `urn:iki:ledger:items` uses.
//! 5. Partition what is left: ready (unclaimed, not awaiting its dispatcher, not deferred,
//!    no open blocker) versus excluded, and keep the exclusion reasons.
//! 6. Hand the ready set to the [`OrderingPolicy`].
//!
//! ⚠ **Steps 2 and 3 run over the whole ledger and step 4 comes after them, and that
//! order is the fix for two defects.** Until 0.2.1 the pool was loaded with the caller's
//! filters applied and through the listing's 500-row bound, and "blocked" was computed over
//! that pool. So a blocker that did not carry the filter's label was invisible and the item
//! it blocked was offered as ready; and with more than 500 open items the oldest fell out,
//! so an older p0 was never offered and an item whose blocker fell out was offered as
//! ready. Blocked, cyclic and leverage are facts about the ledger; a filter only chooses
//! which of the ledger's items you are asking about.
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
use sha2::{Digest, Sha256};

use crate::ledger::Ledger;
use crate::model::{self, Filter, Item};
use crate::policy::{Candidate, OrderingPolicy, Ranked, SelectionInputs};
use crate::sparql::{self, StoreClient};
use crate::vocabulary as v;

/// Why the ready set would not offer an open item.
///
/// `#[non_exhaustive]` since 0.5.0, which added [`Excluded::LeaseExpired`]: a reason is the
/// kind of thing that keeps arriving.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Excluded {
    /// Someone holds it.
    Claimed(String),
    /// Someone held it on a lease that has run out. ★ Still excluded: an expired lease is
    /// never silently free, and `claim takeover=true from=<holder>` is the only way to take
    /// it — so this reason names the holder and when the lease ended, which is what the
    /// taker needs.
    LeaseExpired {
        /// Who held it.
        holder: String,
        /// When the lease ran out (ms since the epoch).
        expires: u64,
    },
    /// Handed back by its worker (`comment handback=true`) and waiting for the dispatcher to
    /// check the evidence and close it, or reject it (ledger #1199). Added in 0.8.0.
    AwaitingDispatcher,
    /// Deliberately not now.
    Deferred,
    /// One or more open items block it.
    Blocked(Vec<i64>),
}

impl Excluded {
    /// The sentence a human reads and the `ledger:reason` a graph carries, with blockers
    /// spelled as the DEFAULT ledger's numbers (`#12`).
    ///
    /// ⚠ Right only in the default ledger; [`Excluded::reason_in`] is the one to call.
    pub fn reason(&self) -> String {
        self.reason_in(&Ledger::default())
    }

    /// The same sentence with blockers in `ledger`'s own spelling — `acme#12` outside the
    /// default ledger. A bare `#12` names the default ledger's item, so printing it for
    /// `acme`'s blocker (as 0.2.1 did) named a different piece of work.
    pub fn reason_in(&self, ledger: &Ledger) -> String {
        match self {
            Excluded::Claimed(holder) => format!("claimed by {holder}"),
            Excluded::LeaseExpired { holder, expires } => format!(
                "lease expired: claimed by {holder} until {}",
                sparql::iso8601(*expires)
            ),
            Excluded::AwaitingDispatcher => "handed back, awaiting the dispatcher".to_string(),
            Excluded::Deferred => "deferred".to_string(),
            Excluded::Blocked(numbers) => format!(
                "blocked by {}",
                numbers
                    .iter()
                    .map(|n| ledger.number(*n))
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

/// Compute the ready set among the open items a filter admits — with "blocked", the cycle
/// check and leverage computed over **every** open item, never only the admitted ones.
///
/// # Errors
///
/// [`Error::Endpoint`] naming the cycle when the `blocks` graph over open items is not
/// a DAG. A cycle is not exotic — it is what happens when someone links carelessly — and
/// the alternative answer, "nothing is ready", is indistinguishable from having finished.
pub async fn ready(client: &StoreClient<'_, '_>, filter: &Filter) -> Result<ReadySet> {
    ready_inner(client, filter, None).await
}

/// [`ready`], judging leases at `now` (ms since the epoch, from the kernel's clock): a claim
/// whose lease has run out is excluded as [`Excluded::LeaseExpired`] rather than
/// [`Excluded::Claimed`]. It is excluded either way — an expired lease is never free.
pub async fn ready_at(client: &StoreClient<'_, '_>, filter: &Filter, now: u64) -> Result<ReadySet> {
    ready_inner(client, filter, Some(now)).await
}

/// The earliest lease, among the items `set` excluded as claimed, that is still running at
/// `now` — the moment a ranking of `set` stops being true even if nothing is written,
/// because that exclusion's reason turns into `lease-expired`.
pub fn next_lease_expiry(set: &ReadySet, now: u64) -> Option<u64> {
    set.excluded
        .iter()
        .filter(|(_, why)| matches!(why, Excluded::Claimed(_)))
        .filter_map(|(item, _)| item.lease_expires)
        .filter(|expires| *expires > now)
        .min()
}

async fn ready_inner(
    client: &StoreClient<'_, '_>,
    filter: &Filter,
    now: Option<u64>,
) -> Result<ReadySet> {
    let open = model::load_open_items(client).await?;
    // Which of them the caller asked about — the filter's own clauses, evaluated by the
    // store, so `labels=rust` means here what it means to `items`. `None` is "all of them".
    let mut narrowing = filter.clone();
    narrowing.status = model::Status::Open;
    // Readiness itself is evaluated below, in Rust, so the exclusions can be explained.
    narrowing.holder = model::Holder::Any;
    narrowing.deferred = model::Deferred::Include;
    let narrows = narrowing.kind.is_some()
        || !narrowing.labels.is_empty()
        || !narrowing.without.is_empty()
        || narrowing.about.is_some()
        || narrowing.text.is_some();
    let admitted = if narrows {
        Some(model::matching_iris(client, &narrowing).await?)
    } else {
        None
    };

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

    // ★ Once for the whole graph, never one walk per item (ledger #1422): a walk per item is
    // quadratic on a chain, and a 10,000-item chain took 20 s against 0.25 s unlinked.
    let downstream = downstream_counts(&edges);
    let leverage: BTreeMap<String, (usize, usize)> = open
        .iter()
        .map(|item| {
            let direct = edges
                .get(item.iri.as_str())
                .map(|t| t.len())
                .unwrap_or_default();
            let reach = downstream
                .get(item.iri.as_str())
                .copied()
                .unwrap_or_default();
            (item.iri.clone(), (reach, direct))
        })
        .collect();

    // The borrows of `open` end here, so the items can be moved into the partition below.
    drop(edges);
    drop(numbers);
    drop(open_iris);

    let mut ready = Vec::new();
    let mut excluded = Vec::new();
    for item in open {
        if admitted
            .as_ref()
            .is_some_and(|admitted| !admitted.contains(&item.iri))
        {
            continue;
        }
        let why = if let Some(holder) = item.claimed_by.clone() {
            match (now, item.lease_expires) {
                (Some(now), Some(expires)) if expires <= now => {
                    Some(Excluded::LeaseExpired { holder, expires })
                }
                _ => Some(Excluded::Claimed(holder)),
            }
        } else if item.awaiting.is_some() {
            // ★ Any `ledger:awaiting` value, not only the dispatcher's: an item waiting on
            // someone is not ready for a worker, whoever the out-of-band write named.
            Some(Excluded::AwaitingDispatcher)
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
    let candidates = candidates(&set);
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

/// What a policy is handed for a ready set.
fn candidates(set: &ReadySet) -> Vec<Candidate> {
    set.ready
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
        .collect()
}

/// Until when a ranking of `set` by `policy` at `now` stays true while the ledger does
/// not change — [`OrderingPolicy::valid_until`] over the same inputs [`rank`] hands the
/// policy. `None`: the ranking does not depend on the clock.
pub fn valid_until(set: &ReadySet, policy: &dyn OrderingPolicy, now: u64) -> Option<u64> {
    policy.valid_until(&SelectionInputs {
        candidates: candidates(set),
        now,
    })
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
                        .map(|(item, why)| {
                            format!("{} {}", item.short(), why.reason_in(&self.ledger))
                        })
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
            out.push_str(&format!(
                "  not {}: {}\n",
                item.short(),
                why.reason_in(&self.ledger)
            ));
        }
        out
    }

    /// The graph face: the reasoning, skolemized, so "why this one" is queryable and not
    /// just printable.
    ///
    /// ★ **The selection's IRI is its moment AND a digest of what it says.** It used to be
    /// the ledger and the millisecond alone, so two different selections in one
    /// millisecond — two policies, two filters — were one subject, and the union idiom
    /// (`urn:rdf:union`) merged two rankings into a selection claiming two policies. The
    /// digest is over the graph this method would write under a placeholder subject, so
    /// two selections share an IRI exactly when they say the same thing — content-addressed,
    /// which also keeps the face deterministic for the cache.
    pub fn turtle(&self) -> Result<Vec<u8>> {
        let placeholder = format!("{}_", self.ledger.selection(self.generated_at));
        let digest = Sha256::digest(model::turtle(&self.graph(&placeholder)?)?);
        let selection = format!(
            "{}-{}",
            self.ledger.selection(self.generated_at),
            digest[..8]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        model::turtle(&self.graph(&selection)?)
    }

    /// The selection as a graph under one subject IRI.
    fn graph(&self, selection: &str) -> Result<Graph> {
        let subject = NamedNode::new(selection)
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
            push(
                &mut graph,
                &node,
                v::REASON,
                model::plain(&why.reason_in(&self.ledger)),
            );
        }
        Ok(graph)
    }
}

/// The `blocks` graph over integer indices, so a walk is array work: `name[i]` is a node,
/// `next[i]` its targets in the order the map gives them. The map's keys come first, in
/// sorted order, then any target that is not a key (a node with no edges of its own).
struct Indexed<'a> {
    name: Vec<&'a str>,
    next: Vec<Vec<usize>>,
    keys: usize,
}

fn indexed<'a>(edges: &BTreeMap<&'a str, Vec<&'a str>>) -> Indexed<'a> {
    let mut at: BTreeMap<&'a str, usize> = BTreeMap::new();
    let mut name: Vec<&'a str> = Vec::with_capacity(edges.len());
    for key in edges.keys() {
        at.insert(key, name.len());
        name.push(key);
    }
    let keys = name.len();
    let mut next: Vec<Vec<usize>> = Vec::with_capacity(keys);
    for targets in edges.values() {
        let mut out = Vec::with_capacity(targets.len());
        for target in targets {
            let i = *at.entry(target).or_insert_with(|| {
                name.push(target);
                name.len() - 1
            });
            out.push(i);
        }
        next.push(out);
    }
    next.resize(name.len(), Vec::new());
    Indexed { name, next, keys }
}

/// The `blocks` subgraph's first cycle, as the path that closes it, or `None` when it is
/// a DAG.
///
/// ⚠ Walked by hand rather than asked of SPARQL, and the ecosystem has met this shape
/// before: the process-vocabulary shapes record that a cycle through a name hop is
/// invisible to property paths. `?a ledger:blocks+ ?a` would answer *whether* there is a
/// cycle over a direct predicate but never *which* — and a refusal that cannot name the
/// cycle leaves the operator exactly where the silent empty answer did.
///
/// ★ **An explicit stack, never recursion** (ledger #1422). The graph is anyone's with a
/// ledger's write grant, so its depth is request input: until 0.11.1 this recursed once per
/// link, and a chain about 15,000 long overflowed a 2 MiB stack, the size of a tokio
/// worker's, aborting the whole host on every `next`. The walk is the same depth-first one
/// in the same order, so the cycle reported for a given graph is the one it always was.
fn find_cycle(edges: &BTreeMap<&str, Vec<&str>>) -> Option<Vec<String>> {
    const NEW: u8 = 0;
    const OPEN: u8 = 1;
    const DONE: u8 = 2;
    let graph = indexed(edges);
    let mut mark = vec![NEW; graph.name.len()];
    // The path from the root, each node with the index of its next target to try.
    let mut path: Vec<(usize, usize)> = Vec::new();
    // Sorted iteration: the cycle reported for a given graph is always the same one.
    for root in 0..graph.keys {
        if mark[root] != NEW {
            continue;
        }
        mark[root] = OPEN;
        path.push((root, 0));
        while let Some(top) = path.last_mut() {
            let (node, tried) = *top;
            let Some(&next) = graph.next[node].get(tried) else {
                mark[node] = DONE;
                path.pop();
                continue;
            };
            top.1 += 1;
            match mark[next] {
                DONE => {}
                OPEN => {
                    // The cycle is the suffix of the path from this node, closed by it.
                    let at = path.iter().position(|(n, _)| *n == next)?;
                    let mut cycle: Vec<String> = path[at..]
                        .iter()
                        .map(|(n, _)| graph.name[*n].to_string())
                        .collect();
                    cycle.push(graph.name[next].to_string());
                    return Some(cycle);
                }
                _ => {
                    mark[next] = OPEN;
                    path.push((next, 0));
                }
            }
        }
    }
    None
}

/// For every node of a DAG, how many distinct nodes it blocks, transitively (not counting
/// itself): the leverage `next` ranks by, computed once for the whole graph.
///
/// Children are counted before their parents (a depth-first postorder, with an explicit
/// stack), and then:
///
/// - a node with ONE target counts that target and the target's own count. Exact, because
///   in a DAG a node is never downstream of itself, so nothing is counted twice. This is the
///   chain, linear however long.
/// - a node with SEVERAL targets walks its downstream once, counting each node once. Adding
///   its targets' counts instead would count a diamond's far corner twice
///   (`a_diamond_counts_each_item_once`).
///
/// ⚠ So it is linear on chains and trees of single links, and quadratic only where many
/// nodes each block several items with a large shared downstream. Exact distinct counts in a
/// general DAG cost a reachability computation; the walk here is array work over integer
/// indices, which keeps that case cheap where it does arise.
///
/// ⚠ Only for a DAG: called after [`find_cycle`] found none. On a graph with a cycle it still
/// terminates, but its counts mean nothing.
fn downstream_counts<'a>(edges: &BTreeMap<&'a str, Vec<&'a str>>) -> BTreeMap<&'a str, usize> {
    let graph = indexed(edges);
    let n = graph.name.len();
    // Distinct targets per node: a repeated link is one link.
    let next: Vec<Vec<usize>> = graph
        .next
        .iter()
        .map(|targets| {
            let mut unique = targets.clone();
            unique.sort_unstable();
            unique.dedup();
            unique
        })
        .collect();

    // A postorder of every node: each after everything it reaches.
    let mut order: Vec<usize> = Vec::with_capacity(n);
    let mut visited = vec![false; n];
    let mut path: Vec<(usize, usize)> = Vec::new();
    for root in 0..n {
        if visited[root] {
            continue;
        }
        visited[root] = true;
        path.push((root, 0));
        while let Some(top) = path.last_mut() {
            let (node, tried) = *top;
            match next[node].get(tried) {
                Some(&target) => {
                    top.1 += 1;
                    if !visited[target] {
                        visited[target] = true;
                        path.push((target, 0));
                    }
                }
                None => {
                    order.push(node);
                    path.pop();
                }
            }
        }
    }

    let mut count = vec![0usize; n];
    // The walk's visited set, by generation: `seen[m] == pass` means seen in this pass.
    let mut seen = vec![0usize; n];
    let mut pass = 0usize;
    let mut queue: Vec<usize> = Vec::new();
    for &node in &order {
        count[node] = match next[node].as_slice() {
            [] => 0,
            [only] => count[*only] + 1,
            targets => {
                pass += 1;
                let mut reached = 0;
                queue.clear();
                queue.extend_from_slice(targets);
                while let Some(m) = queue.pop() {
                    if m == node || seen[m] == pass {
                        continue;
                    }
                    seen[m] = pass;
                    reached += 1;
                    queue.extend_from_slice(&next[m]);
                }
                reached
            }
        };
    }
    graph
        .name
        .iter()
        .zip(count)
        .map(|(name, count)| (*name, count))
        .collect()
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
        let counts = downstream_counts(&graph);
        assert_eq!(counts["a"], 3);
        assert_eq!(counts["c"], 1);
        assert_eq!(counts["d"], 0);
        assert_eq!(reachable(&graph, "a"), 3);
    }

    #[test]
    fn a_diamond_counts_each_item_once() {
        let graph = edges(&[("a", "b"), ("a", "c"), ("b", "d"), ("c", "d")]);
        assert_eq!(downstream_counts(&graph)["a"], 3);
        assert_eq!(reachable(&graph, "a"), 3);
    }

    /// How many distinct nodes `from` reaches: 0.11.0's per-item walk, kept as the
    /// reference the whole-graph count must agree with.
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

    /// 0.11.0's recursive cycle walk, kept as the reference the iterative one must agree with:
    /// the same cycle, not merely a cycle, since the refusal names it.
    fn find_cycle_recursive(edges: &BTreeMap<&str, Vec<&str>>) -> Option<Vec<String>> {
        #[derive(Clone, Copy, PartialEq)]
        enum Mark {
            Open,
            Done,
        }
        fn walk<'a>(
            node: &'a str,
            edges: &BTreeMap<&'a str, Vec<&'a str>>,
            marks: &mut BTreeMap<&'a str, Mark>,
            stack: &mut Vec<&'a str>,
        ) -> Option<Vec<String>> {
            match marks.get(node) {
                Some(Mark::Done) => return None,
                Some(Mark::Open) => {
                    let at = stack.iter().position(|n| *n == node)?;
                    let mut cycle: Vec<String> =
                        stack[at..].iter().map(|n| n.to_string()).collect();
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
        let mut marks = BTreeMap::new();
        let mut stack = Vec::new();
        for node in edges.keys() {
            if let Some(cycle) = walk(node, edges, &mut marks, &mut stack) {
                return Some(cycle);
            }
        }
        None
    }

    /// A small deterministic generator, so a failure names a seed that reproduces it.
    struct Lcg(u64);
    impl Lcg {
        fn below(&mut self, n: usize) -> usize {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((self.0 >> 33) as usize) % n
        }
    }

    /// `n` named nodes with about `links` random edges. With `dag`, every edge runs from a
    /// lower index to a higher one, so the graph has no cycle.
    fn random_graph(seed: u64, n: usize, links: usize, dag: bool) -> Vec<(String, String)> {
        let mut rng = Lcg(seed);
        let mut pairs = Vec::new();
        for _ in 0..links {
            let (a, b) = (rng.below(n), rng.below(n));
            let (a, b) = if dag { (a.min(b), a.max(b)) } else { (a, b) };
            if dag && a == b {
                continue;
            }
            pairs.push((format!("n{a:03}"), format!("n{b:03}")));
        }
        // Every node is a key, as in `next`, where every open item is one.
        for i in 0..n {
            pairs.push((format!("n{i:03}"), String::new()));
        }
        pairs
    }

    fn graph_of(pairs: &[(String, String)]) -> BTreeMap<&str, Vec<&str>> {
        let mut map: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (from, to) in pairs {
            let targets = map.entry(from.as_str()).or_default();
            if !to.is_empty() {
                targets.push(to.as_str());
            }
        }
        for targets in map.values_mut() {
            targets.sort_unstable();
        }
        map
    }

    /// ★ `next`'s ranking is unchanged (ledger #1422): on random DAGs, dense and sparse, the
    /// whole-graph count is the per-item walk's count for every node.
    #[test]
    fn whole_graph_leverage_matches_the_per_item_walk_on_random_dags() {
        for seed in 0..200 {
            let n = 2 + (seed as usize % 40);
            let links = (seed as usize * 7) % (3 * n);
            let pairs = random_graph(seed, n, links, true);
            let graph = graph_of(&pairs);
            assert!(find_cycle(&graph).is_none(), "seed {seed}");
            let counts = downstream_counts(&graph);
            for node in graph.keys() {
                assert_eq!(
                    counts.get(node).copied().unwrap_or_default(),
                    reachable(&graph, node),
                    "seed {seed}, node {node}"
                );
            }
        }
    }

    /// ★ The refusal names the same cycle it always did: on random graphs with and without
    /// cycles, the iterative walk answers exactly what the recursive one did.
    #[test]
    fn the_iterative_cycle_walk_names_the_recursive_walks_cycle() {
        let mut cyclic = 0;
        for seed in 0..300 {
            let n = 2 + (seed as usize % 30);
            let links = 1 + (seed as usize * 5) % (2 * n);
            let pairs = random_graph(seed, n, links, seed % 3 == 0);
            let graph = graph_of(&pairs);
            let found = find_cycle(&graph);
            cyclic += usize::from(found.is_some());
            assert_eq!(found, find_cycle_recursive(&graph), "seed {seed}");
        }
        assert!(cyclic > 50, "the corpus has cycles to find: {cyclic}");
    }

    /// A chain far deeper than any stack would hold one frame per link: the walk is an
    /// explicit stack, so this runs on the test thread (2 MiB) and answers.
    #[test]
    fn a_deep_chain_is_walked_without_recursion() {
        let names: Vec<String> = (0..200_000).map(|i| format!("n{i:06}")).collect();
        let mut graph: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for pair in names.windows(2) {
            graph.insert(pair[0].as_str(), vec![pair[1].as_str()]);
        }
        graph.insert(names[names.len() - 1].as_str(), Vec::new());
        assert!(find_cycle(&graph).is_none());
        let counts = downstream_counts(&graph);
        assert_eq!(counts[names[0].as_str()], names.len() - 1);
        // Closed into a ring, it is one cycle, named from the first node.
        graph.insert(names[names.len() - 1].as_str(), vec![names[0].as_str()]);
        let cycle = find_cycle(&graph).expect("a ring");
        assert_eq!(cycle.len(), names.len() + 1);
        assert_eq!(cycle.first(), cycle.last());
    }
}
