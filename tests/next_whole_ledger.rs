//! **`next` over the whole ledger, and over time.** Each test pins a defect an unled audit
//! reproduced on 0.2.1 (`4e6f46e`), and failed there because of it.
//!
//! - The ready pool was loaded through the listing's 500-row bound, newest first, so with
//!   more than 500 open items an older p0 was never offered and an item whose blocker fell
//!   out of the pool was offered as ready.
//! - The pool was loaded with the caller's filters applied and "blocked" was computed over
//!   it, so a blocker outside the filter was invisible. Leverage and the cycle check had the
//!   same blind spot.
//! - `next` was cached under the store's write threads alone, and reading the clock
//!   records no dependency, so an idle ledger served a ranking computed at an earlier
//!   moment for as long as nothing was written.

mod common;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use common::*;
use ikigai_core::{Clock, Fallback, Kernel, Space, Time, Verb};
use ikigai_ledger::{OrderingPolicy, Ranked, SelectionInputs};
use ikigai_store::DurableStore;

const T0: u64 = 1_789_430_400_000; // 2026-09-15T00:00:00Z
const DAY: u64 = 86_400_000;

/// A clock the test moves by hand.
struct HandClock(Arc<AtomicU64>);

impl Clock for HandClock {
    fn now(&self) -> Time {
        Time::from_millis(self.0.load(Ordering::SeqCst))
    }
}

fn kernel_at(now: &Arc<AtomicU64>, policies: Option<Vec<Arc<dyn OrderingPolicy>>>) -> Kernel {
    let ledger = match policies {
        Some(policies) => ikigai_ledger::space_with_policies(policies),
        None => ikigai_ledger::space(),
    };
    let space = Fallback::new(vec![
        Arc::new(ikigai_store::space(DurableStore::in_memory().unwrap())) as Arc<dyn Space>,
        Arc::new(ledger) as Arc<dyn Space>,
    ]);
    Kernel::with_meta_renderer(Arc::new(space), Arc::new(ikigai_vocab::TurtleRenderer))
        .with_clock(Arc::new(HandClock(Arc::clone(now))))
}

/// The first ranked line of `next`'s plain face, or "".
fn first_ranked(next: &str) -> String {
    next.lines()
        .find(|l| l.starts_with(" 1. "))
        .unwrap_or("")
        .to_string()
}

fn blocks(kernel: &Kernel, blocker: &str, blocked: &str) {
    sink(
        kernel,
        "urn:iki:ledger:link",
        &[("item", blocker), ("content", blocked), ("type", "blocks")],
    );
}

// ------------------------------------------------------------------- the filters

/// ★ A blocker that does not carry the filter's label still blocks. On 0.2.1
/// `next labels=frontend` offered #2, because the open #1 was outside the pool.
#[test]
fn a_blocker_outside_the_filter_still_blocks() {
    let kernel = kernel();
    append(&kernel, "Backend schema migration", &[]); // #1, no label
    append(
        &kernel,
        "Frontend form for the new schema",
        &[("labels", "frontend"), ("priority", "0")],
    ); // #2
    blocks(&kernel, "#1", "#2");
    let all = source(&kernel, "urn:iki:ledger:next", &[]);
    assert!(all.contains("blocked by #1"), "unfiltered: {all}");

    let filtered = source(&kernel, "urn:iki:ledger:next", &[("labels", "frontend")]);
    assert!(
        !first_ranked(&filtered).contains("Frontend form"),
        "next labels=frontend offered #2, which the open #1 blocks:\n{filtered}"
    );
    assert!(filtered.contains("#2 blocked by #1"), "{filtered}");
    // The filter still chooses what is ASKED about: the blocker is not offered either.
    assert!(!filtered.contains("Backend schema"), "{filtered}");
}

/// Leverage is a fact about the ledger too: a candidate inside the filter that unblocks
/// work outside it is credited for that work.
#[test]
fn leverage_counts_what_a_candidate_unblocks_outside_the_filter() {
    let kernel = kernel();
    append(&kernel, "Rust blocker", &[("labels", "rust")]); // #1
    append(&kernel, "Docs that wait on it", &[]); // #2
    append(&kernel, "More docs that wait", &[]); // #3
    blocks(&kernel, "#1", "#2");
    blocks(&kernel, "#2", "#3");
    let next = source(
        &kernel,
        "urn:iki:ledger:next",
        &[("labels", "rust"), ("policy", "leverage")],
    );
    assert!(
        next.contains("unblocks 2 open item(s)"),
        "#1 transitively unblocks #2 and #3:\n{next}"
    );
}

/// A cycle upstream of an item the filter admits makes that item's readiness unknowable,
/// so `next` refuses and names the cycle, filtered or not — rather than offering
/// nothing and looking finished, or offering the item as if its blockers were not there.
#[test]
fn a_cycle_outside_the_filter_is_still_refused() {
    let kernel = kernel();
    append(&kernel, "A", &[]); // #1
    append(&kernel, "B", &[]); // #2
    append(&kernel, "Labeled work", &[("labels", "rust")]); // #3
    blocks(&kernel, "#1", "#2");
    blocks(&kernel, "#2", "#1");
    blocks(&kernel, "#2", "#3");
    let refused = try_verb(
        &kernel,
        Verb::Source,
        "urn:iki:ledger:next",
        &[("labels", "rust")],
    );
    assert!(
        format!("{refused:?}").contains("cycle"),
        "a filtered next answered over a cycle: {refused:?}"
    );
}

// ------------------------------------------------------------------- the 500 bound

/// ★ An open blocker that is not among the 500 most recently touched open items still
/// blocks. On 0.2.1 it fell out of the pool and the p0 it blocks was offered first.
#[test]
fn an_old_blocker_still_blocks_beyond_five_hundred_newer_items() {
    let kernel = kernel();
    append(&kernel, "Old blocker", &[]); // #1
    append(&kernel, "Urgent but blocked", &[("priority", "0")]); // #2
    blocks(&kernel, "#1", "#2");
    // Touch #2 so it is newer than #1 (the link touched #1).
    sink(
        &kernel,
        "urn:iki:ledger:label",
        &[("item", "#2"), ("content", "x")],
    );
    for n in 0..499 {
        append(&kernel, &format!("filler {n}"), &[]);
    }
    let next = source(&kernel, "urn:iki:ledger:next", &[("limit", "1")]);
    assert!(
        !first_ranked(&next).contains("Urgent but blocked"),
        "#2 is blocked by the open #1 and was offered:\n{}",
        next.lines().take(4).collect::<Vec<_>>().join("\n")
    );
    assert!(next.contains("not #2: blocked by #1"), "{}", next.len());
}

/// An older p0 is offered first however many newer items there are. On 0.2.1 it was the
/// 501st most recent and never seen.
#[test]
fn an_old_p0_is_offered_first_beyond_five_hundred_newer_items() {
    let kernel = kernel();
    append(&kernel, "The urgent one", &[("priority", "0")]); // #1
    for n in 0..500 {
        append(&kernel, &format!("filler {n}"), &[]);
    }
    let next = source(&kernel, "urn:iki:ledger:next", &[("limit", "1")]);
    assert!(
        first_ranked(&next).contains("The urgent one"),
        "the only prioritized open item is not offered first:\n{}",
        next.lines().take(4).collect::<Vec<_>>().join("\n")
    );
    assert!(
        next.contains("ready: 501"),
        "{}",
        next.lines().last().unwrap_or("")
    );
}

// --------------------------------------------------------------------------- time

/// ★ An idle ledger is re-ranked as time passes. B (p4) was filed on day 0 and A (p3) on
/// day 30; under `leverage` B leads on day 30 (4 + 5 age points against 8) and A leads on
/// day 90 (8 + 10 against 4 + 10). On 0.2.1 nothing was written in between, so the day-30
/// answer was served on day 90.
#[test]
fn an_idle_ledger_is_re_ranked_as_time_passes() {
    let now = Arc::new(AtomicU64::new(T0));
    let kernel = kernel_at(&now, None);
    append(&kernel, "Old low-priority B", &[("priority", "4")]);
    now.store(T0 + 30 * DAY, Ordering::SeqCst);
    append(&kernel, "New higher-priority A", &[("priority", "3")]);
    let day30 = source(&kernel, "urn:iki:ledger:next", &[("policy", "leverage")]);
    assert!(
        first_ranked(&day30).contains("Old low-priority B"),
        "{day30}"
    );

    now.store(T0 + 90 * DAY, Ordering::SeqCst);
    let day90 = source(&kernel, "urn:iki:ledger:next", &[("policy", "leverage")]);
    assert!(
        first_ranked(&day90).contains("New higher-priority A"),
        "day 90, nothing written since day 30, still ranked as of day 30:\n{day90}"
    );
    assert!(day90.contains("filed 90 day(s) ago"), "{day90}");
}

/// The other half, and the reason `next` is not simply uncacheable: a policy that never
/// reads the clock keeps its cached answer across time, because nothing about it can have
/// changed. Observed through the Turtle face, whose `generatedAtTime` is the moment it was
/// computed.
#[test]
fn a_policy_that_ignores_the_clock_stays_cached_across_time() {
    let now = Arc::new(AtomicU64::new(T0));
    let kernel = kernel_at(&now, None);
    append(&kernel, "Something", &[("priority", "1")]);
    let ask = || {
        source(
            &kernel,
            "urn:iki:ledger:next",
            &[("as", "text/turtle"), ("policy", "priority-recency")],
        )
    };
    let first = ask();
    now.store(T0 + 90 * DAY, Ordering::SeqCst);
    assert_eq!(
        first,
        ask(),
        "priority-recency was recomputed with nothing changed"
    );
}

/// A host's own policy that does not say until when its answer holds is never served from
/// the cache — the safe default, since only the policy knows what it does with `now`.
#[test]
fn a_policy_that_does_not_say_is_recomputed_every_time() {
    /// Ranks nothing differently, but says what time it thinks it is.
    struct Clockwatcher;
    impl OrderingPolicy for Clockwatcher {
        fn name(&self) -> &str {
            "clockwatcher"
        }
        fn summary(&self) -> &str {
            "reports the moment it ranked"
        }
        fn weighs(&self) -> Vec<&'static str> {
            vec!["now"]
        }
        fn order(&self, inputs: &SelectionInputs) -> Vec<Ranked> {
            inputs
                .candidates
                .iter()
                .map(|c| Ranked {
                    iri: c.iri.clone(),
                    score: format!("at {}", inputs.now),
                    because: vec!["it is now".to_string()],
                })
                .collect()
        }
    }
    let now = Arc::new(AtomicU64::new(T0));
    let kernel = kernel_at(&now, Some(vec![Arc::new(Clockwatcher)]));
    append(&kernel, "Something", &[]);
    let first = source(&kernel, "urn:iki:ledger:next", &[]);
    now.store(T0 + 1, Ordering::SeqCst);
    let second = source(&kernel, "urn:iki:ledger:next", &[]);
    assert!(first.contains(&format!("at {T0}")), "{first}");
    assert!(second.contains(&format!("at {}", T0 + 1)), "{second}");
}
