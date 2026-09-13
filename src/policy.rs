//! The ordering seam: what `urn:iki:ledger:next` does with the ready set once the
//! deterministic half is finished.
//!
//! # Two stages, kept apart
//!
//! 1. **The ready set is a query** — open, unclaimed, not deferred, not blocked by an
//!    open item, plus the caller's label and `about` filters. It needs no policy at all,
//!    and most of the value is here: a list that never offers you something someone else
//!    holds or something that cannot start yet is most of what "what's next" means.
//! 2. **The order within it is policy**, and that is the part people disagree about.
//!
//! # Why the policy is a trait and not a config key
//!
//! Brian asked for this shape once before, on the kernel cache: *"I want the policies to
//! be configurable, not fixed. It's ok to have only a default policy now, but create a
//! trait or something that can be configured at execution time."* The right ordering for
//! a solo backlog, a review queue and a team are different, and baking one in makes the
//! second one a rewrite.
//!
//! Each policy is also a **resource** — `urn:iki:ledger:policy:{name}` — so the manifold
//! advertises which orderings exist and `Meta` says what each one weighs. A policy is
//! then something you can version, diff and hand to another host, which a config key is
//! not.
//!
//! # What a policy may see
//!
//! [`SelectionInputs`] is a struct rather than a widening argument list, for the reason
//! the field guide gives about those: the third signal is where the list becomes a
//! breaking change. A policy sees only the ready set — it cannot re-admit something the
//! query excluded, which is what keeps "selection offers, it never authorizes" checkable
//! rather than aspirational.

use std::sync::Arc;

/// One item the ready-set query admitted, with everything a policy is allowed to weigh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The item's stable IRI.
    pub iri: String,
    /// Its short number, for tie-breaking and for the answer a human reads.
    pub number: i64,
    /// Its title.
    pub title: String,
    /// Its specific class, when it has one — its LEVEL. A policy may scope itself to a
    /// level ("what is next at the decision level" is a different question from "what is
    /// next at the finding level"), which is another reason a policy is a resource rather
    /// than one global rank.
    pub kind: Option<String>,
    /// 0 highest … 4 lowest. `None` is unset, and unset is NOT 4 — kata's rule, kept, is
    /// that any priority beats no priority.
    pub priority: Option<i64>,
    /// When it was filed (ms since the epoch), so a policy can keep old work from
    /// starving — the failure mode of a pure priority sort.
    pub created: u64,
    /// When it last changed (ms since the epoch). kata's tie-break.
    pub modified: u64,
    /// Its labels.
    pub labels: Vec<String>,
    /// The resources it is `ledger:about`.
    pub about: Vec<String>,
    /// ★ How many OPEN items this one blocks, **transitively**. It falls out of the link
    /// graph for free and is usually the best single signal, because finishing a blocker
    /// converts several unready items into ready ones — and a priority sort cannot see
    /// it at all.
    pub leverage: usize,
    /// How many open items it blocks directly.
    pub blocks_direct: usize,
}

/// Everything a policy is given: the ready set, and the moment it is ranking for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionInputs {
    /// The ready set, in the query's order (most recently updated first).
    pub candidates: Vec<Candidate>,
    /// Now, in milliseconds since the epoch, from the kernel's injected clock. A policy
    /// that weighs age reads it here rather than from the system clock, so a replay
    /// harness gets a reproducible answer.
    pub now: u64,
}

/// One ranked candidate, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ranked {
    /// The item's IRI.
    pub iri: String,
    /// The policy's own score, as a string: a score is only comparable WITHIN one policy,
    /// and publishing it as a number invites a comparison across policies that means
    /// nothing.
    pub score: String,
    /// The reasons, in the policy's own words. This is what makes an answer
    /// interrogable — and an answer that cannot be interrogated is overridden and then
    /// ignored.
    pub because: Vec<String>,
}

/// How a ready set is ordered. Implement it, hand it to
/// [`space_with_policies`](crate::space_with_policies), and it is a resource.
///
/// ⚠ **A policy must be a total order and a pure function of its inputs.** `next` is a
/// cacheable read: two calls over an unchanged graph must give the same answer, or the
/// cache turns a coin flip into a fact. Ties therefore break on something stable — the
/// item number, never iteration order of a set.
pub trait OrderingPolicy: Send + Sync {
    /// The name in `urn:iki:ledger:policy:{name}` and in `policy=`. Kebab-case.
    fn name(&self) -> &str;

    /// One line: what this policy is for.
    fn summary(&self) -> &str;

    /// The signals it weighs, in the order it weighs them — for `Meta`, for the policy
    /// resource's own face, and for the selection graph.
    fn weighs(&self) -> Vec<&'static str>;

    /// Rank the ready set, best first. Returning fewer entries than it was given is
    /// allowed and means "these are the only ones I will speak for".
    fn order(&self, inputs: &SelectionInputs) -> Vec<Ranked>;
}

/// kata's own rule, reimplemented so our behaviour can be **diffed against the tool we
/// are replacing** rather than merely claimed to be better.
///
/// From `cmd/kata/ready_client.go:153` (`selectNextReadyIssue`), a single linear pass:
/// a candidate with a priority beats one without; among those, the lower integer wins
/// (0 highest); ties keep whatever came first, which given the ready query's
/// `ORDER BY updated_at DESC, id DESC` means most recently updated.
///
/// That is the whole scheme — no age, no leverage, no effort, no locality — and it is
/// the default here because a replacement whose first answer differs from the tool it
/// replaces is a replacement nobody trusts.
#[derive(Debug, Default, Clone, Copy)]
pub struct PriorityRecency;

impl OrderingPolicy for PriorityRecency {
    fn name(&self) -> &str {
        "priority-recency"
    }

    fn summary(&self) -> &str {
        "kata's rule: any priority beats none, lower priority wins, ties go to the most \
         recently updated. The default, so behaviour can be diffed against the tool this \
         replaces."
    }

    fn weighs(&self) -> Vec<&'static str> {
        vec!["priority", "recency", "number"]
    }

    fn order(&self, inputs: &SelectionInputs) -> Vec<Ranked> {
        let mut ordered: Vec<&Candidate> = inputs.candidates.iter().collect();
        ordered.sort_by(|a, b| {
            // `None` sorts last: any priority beats no priority.
            let key = |c: &Candidate| (c.priority.is_none(), c.priority.unwrap_or(i64::MAX));
            key(a)
                .cmp(&key(b))
                .then(b.modified.cmp(&a.modified))
                .then(b.number.cmp(&a.number))
        });
        ordered
            .into_iter()
            .map(|c| Ranked {
                iri: c.iri.clone(),
                score: match c.priority {
                    Some(p) => format!("p{p}"),
                    None => "unprioritized".to_string(),
                },
                because: vec![
                    match c.priority {
                        Some(p) => format!("priority {p}"),
                        None => {
                            "no priority set, so it ranks below every item that has one".to_string()
                        }
                    },
                    format!("last updated {}", crate::sparql::iso8601(c.modified)),
                ],
            })
            .collect()
    }
}

/// The one kata cannot express: **finish what unblocks the most**, with age as the
/// anti-starvation term.
///
/// Scores are integer points, stated here because a weight nobody can read is a magic
/// number:
///
/// | signal | points |
/// |---|---|
/// | leverage | **10 per open item blocked, transitively** |
/// | priority | `(5 - p) × 4` — p0 = 20 … p4 = 4; unset = 0 |
/// | age | `min(days, 60) / 6` — 0 … 10, so nothing starves |
///
/// Ties break on priority, then on age (older first), then on number — a total order, so
/// the cached answer and the recomputed one agree.
///
/// ⚠ Not the default, deliberately: it is a better rule and an unfamiliar one, and the
/// first job of this module is to be diffable against kata.
#[derive(Debug, Default, Clone, Copy)]
pub struct Leverage;

impl Leverage {
    fn points(&self, c: &Candidate, now: u64) -> (i64, Vec<String>) {
        let mut because = Vec::new();
        let leverage_points = 10 * c.leverage as i64;
        if c.leverage > 0 {
            because.push(format!(
                "unblocks {} open item(s){} (+{leverage_points})",
                c.leverage,
                if c.blocks_direct == c.leverage {
                    String::new()
                } else {
                    format!(" ({} directly)", c.blocks_direct)
                }
            ));
        }
        let priority_points = c.priority.map(|p| (5 - p) * 4).unwrap_or(0);
        because.push(match c.priority {
            Some(p) => format!("priority {p} (+{priority_points})"),
            None => "no priority set (+0)".to_string(),
        });
        let days = now.saturating_sub(c.created) / 86_400_000;
        let age_points = (days.min(60) / 6) as i64;
        if age_points > 0 {
            because.push(format!("filed {days} day(s) ago (+{age_points})"));
        }
        (leverage_points + priority_points + age_points, because)
    }
}

impl OrderingPolicy for Leverage {
    fn name(&self) -> &str {
        "leverage"
    }

    fn summary(&self) -> &str {
        "Finish what unblocks the most: 10 points per open item blocked transitively, \
         (5-priority)×4 for priority, and up to 10 for age so nothing starves."
    }

    fn weighs(&self) -> Vec<&'static str> {
        vec!["leverage", "priority", "age", "number"]
    }

    fn order(&self, inputs: &SelectionInputs) -> Vec<Ranked> {
        let mut scored: Vec<(i64, Vec<String>, &Candidate)> = inputs
            .candidates
            .iter()
            .map(|c| {
                let (points, because) = self.points(c, inputs.now);
                (points, because, c)
            })
            .collect();
        scored.sort_by(|a, b| {
            let key = |c: &Candidate| (c.priority.is_none(), c.priority.unwrap_or(i64::MAX));
            b.0.cmp(&a.0)
                .then(key(a.2).cmp(&key(b.2)))
                .then(a.2.created.cmp(&b.2.created))
                .then(a.2.number.cmp(&b.2.number))
        });
        scored
            .into_iter()
            .map(|(points, because, c)| Ranked {
                iri: c.iri.clone(),
                score: format!("{points} points"),
                because,
            })
            .collect()
    }
}

/// The policies a space offers, and which one `next` uses when the caller names none.
pub struct Policies {
    policies: Vec<Arc<dyn OrderingPolicy>>,
}

impl Default for Policies {
    /// The two built-ins, `priority-recency` first and therefore default.
    fn default() -> Self {
        Policies::new(vec![Arc::new(PriorityRecency), Arc::new(Leverage)])
    }
}

impl Policies {
    /// A registry. **The first policy is the default.**
    ///
    /// # Panics
    ///
    /// On an empty list. That is a host wiring bug, and a ledger whose `next` has no
    /// ordering at all should refuse at boot — loudly, where the manifest is — rather
    /// than at the first request, where the error reads like a data problem.
    pub fn new(policies: Vec<Arc<dyn OrderingPolicy>>) -> Self {
        assert!(
            !policies.is_empty(),
            "a ledger space needs at least one ordering policy: `next` has nothing to rank \
             with, and the failure would otherwise surface at the first request rather than \
             here in the host's manifest"
        );
        Policies { policies }
    }

    /// The default policy's name — the `default_value` `next` declares.
    pub fn default_name(&self) -> &str {
        self.policies[0].name()
    }

    /// Every policy's name, in registration order — the `one_of` `next` declares, so the
    /// manifold advertises exactly what can be asked for.
    pub fn names(&self) -> Vec<String> {
        self.policies.iter().map(|p| p.name().to_string()).collect()
    }

    /// Look one up.
    pub fn get(&self, name: &str) -> Option<&Arc<dyn OrderingPolicy>> {
        self.policies.iter().find(|p| p.name() == name)
    }

    /// Every registered policy.
    pub fn all(&self) -> &[Arc<dyn OrderingPolicy>] {
        &self.policies
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(number: i64, priority: Option<i64>, modified: u64) -> Candidate {
        Candidate {
            iri: format!("urn:iki:ledger:item:{number}"),
            number,
            title: format!("item {number}"),
            kind: None,
            priority,
            created: 1_700_000_000_000,
            modified,
            labels: Vec::new(),
            about: Vec::new(),
            leverage: 0,
            blocks_direct: 0,
        }
    }

    fn inputs(candidates: Vec<Candidate>) -> SelectionInputs {
        SelectionInputs {
            candidates,
            now: 1_757_700_000_000,
        }
    }

    /// ★ kata parity, asserted rather than claimed: any priority beats none, lower wins,
    /// ties go to the most recently updated.
    #[test]
    fn the_default_policy_is_katas_rule() {
        let ranked = PriorityRecency.order(&inputs(vec![
            candidate(1, None, 9_000),
            candidate(2, Some(3), 1_000),
            candidate(3, Some(0), 500),
            candidate(4, Some(3), 8_000),
        ]));
        let order: Vec<&str> = ranked.iter().map(|r| r.iri.as_str()).collect();
        assert_eq!(
            order,
            vec![
                "urn:iki:ledger:item:3", // p0 wins
                "urn:iki:ledger:item:4", // p3, updated later than #2
                "urn:iki:ledger:item:2", // p3
                "urn:iki:ledger:item:1", // no priority sorts last, despite being newest
            ]
        );
    }

    #[test]
    fn leverage_beats_priority_when_it_unblocks_enough() {
        let mut blocker = candidate(1, Some(4), 1_000);
        blocker.leverage = 3;
        blocker.blocks_direct = 2;
        let ranked = Leverage.order(&inputs(vec![blocker, candidate(2, Some(1), 9_000)]));
        assert_eq!(ranked[0].iri, "urn:iki:ledger:item:1");
        assert!(
            ranked[0].because.iter().any(|r| r.contains("unblocks 3")),
            "{:?}",
            ranked[0].because
        );
        // …and the default policy would have said the opposite, which is the whole point
        // of having two.
        let kata = PriorityRecency.order(&inputs(vec![
            candidate(1, Some(4), 1_000),
            candidate(2, Some(1), 9_000),
        ]));
        assert_eq!(kata[0].iri, "urn:iki:ledger:item:2");
    }

    /// A cached answer and a recomputed one must agree, so ties cannot fall to
    /// iteration order.
    #[test]
    fn ranking_is_a_total_order_under_identical_inputs() {
        let tied = vec![
            candidate(7, Some(2), 5_000),
            candidate(8, Some(2), 5_000),
            candidate(9, Some(2), 5_000),
        ];
        let first = Leverage.order(&inputs(tied.clone()));
        let second = Leverage.order(&inputs(tied.into_iter().rev().collect()));
        assert_eq!(
            first.iter().map(|r| r.iri.clone()).collect::<Vec<_>>(),
            second.iter().map(|r| r.iri.clone()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn the_registry_defaults_to_katas_rule_and_advertises_both() {
        let policies = Policies::default();
        assert_eq!(policies.default_name(), "priority-recency");
        assert_eq!(policies.names(), vec!["priority-recency", "leverage"]);
        assert!(policies.get("leverage").is_some());
        assert!(policies.get("nonesuch").is_none());
    }

    #[test]
    #[should_panic(expected = "at least one ordering policy")]
    fn an_empty_registry_refuses_at_boot() {
        Policies::new(Vec::new());
    }
}
