//! `urn:iki:ledger:next`: the deterministic ready set, the cycle refusal, the policy
//! seam, and an answer that says why.

mod common;

use common::*;
use ikigai_core::Verb;

/// Block `blocked` on `blocker` (`#n` references).
fn blocks(kernel: &ikigai_core::Kernel, blocker: &str, blocked: &str) {
    sink(
        kernel,
        "urn:iki:ledger:link",
        &[("item", blocker), ("content", blocked), ("type", "blocks")],
    );
}

/// ★ The narrowing is where most of the value is: a list that never offers you something
/// someone else holds or something that cannot start yet.
#[test]
fn the_ready_set_never_offers_blocked_claimed_or_deferred_work() {
    let kernel = kernel();
    append(&kernel, "Blocker", &[]); // #1
    append(&kernel, "Blocked", &[]); // #2
    append(&kernel, "Held", &[]); // #3
    append(&kernel, "Deferred", &[]); // #4
    append(&kernel, "Ready", &[]); // #5
    blocks(&kernel, "#1", "#2");
    sink(
        &kernel,
        "urn:iki:ledger:claim",
        &[("item", "#3"), ("content", "session-a")],
    );
    sink(&kernel, "urn:iki:ledger:defer", &[("item", "#4")]);

    let next = source(&kernel, "urn:iki:ledger:next", &[("limit", "10")]);
    assert!(next.contains("Blocker"), "{next}");
    assert!(next.contains("Ready"), "{next}");
    assert!(
        !next.contains("1. #2"),
        "a blocked item is not ready: {next}"
    );
    // …and every refusal says why, because "why not that one" is the question a
    // selection gets asked.
    assert!(next.contains("not #2: blocked by #1"), "{next}");
    assert!(next.contains("not #3: claimed by session-a"), "{next}");
    assert!(next.contains("not #4: deferred"), "{next}");

    // Closing the blocker releases the blocked item — that is what closing is FOR.
    sink(&kernel, "urn:iki:ledger:close", &[("item", "#1")]);
    let next = source(&kernel, "urn:iki:ledger:next", &[("limit", "10")]);
    assert!(next.contains("Blocked"), "{next}");
    assert!(!next.contains("not #2"), "{next}");
}

/// ★ A cycle makes the ready set silently empty, which is indistinguishable from a
/// finished backlog. Refuse, and name it.
#[test]
fn a_block_cycle_is_refused_with_the_cycle_named() {
    let kernel = kernel();
    append(&kernel, "A", &[]);
    append(&kernel, "B", &[]);
    append(&kernel, "C", &[]);
    blocks(&kernel, "#1", "#2");
    blocks(&kernel, "#2", "#3");
    blocks(&kernel, "#3", "#1");

    let refused = try_verb(&kernel, Verb::Source, "urn:iki:ledger:next", &[]);
    let message = refused.expect_err("a cycle must refuse").to_string();
    assert!(message.contains("cycle"), "{message}");
    for number in ["#1", "#2", "#3"] {
        assert!(
            message.contains(number),
            "the cycle must be named: {message}"
        );
    }
    assert!(
        message.contains("urn:iki:ledger:link"),
        "and it must say how to break it: {message}"
    );

    // Breaking it makes the answer computable again.
    delete(
        &kernel,
        "urn:iki:ledger:link",
        &[("item", "#3"), ("content", "#1"), ("type", "blocks")],
    );
    let next = source(&kernel, "urn:iki:ledger:next", &[]);
    assert!(next.contains("#1"), "{next}");
}

/// The default is kata's rule, so behaviour is diffable against the tool being replaced.
#[test]
fn the_default_policy_is_katas_priority_then_recency() {
    let kernel = kernel();
    append(&kernel, "No priority but newest", &[]);
    append(&kernel, "Priority four", &[("priority", "4")]);
    append(&kernel, "Priority zero", &[("priority", "0")]);

    let next = source(&kernel, "urn:iki:ledger:next", &[("limit", "3")]);
    let first = next.lines().next().unwrap_or_default();
    assert!(first.contains("Priority zero"), "{next}");
    assert!(next.contains("policy: priority-recency"), "{next}");
    // Any priority beats none, even when the unprioritized item is newer.
    let p4 = next.find("Priority four").expect("ranked");
    let none = next.find("No priority but newest").expect("ranked");
    assert!(p4 < none, "{next}");
}

/// The seam, exercised: the same ready set, a different order, because the policy is an
/// argument and not a rebuild.
#[test]
fn a_different_policy_gives_a_different_answer_over_the_same_ready_set() {
    let kernel = kernel();
    append(&kernel, "Unblocks three", &[("priority", "4")]); // #1
    append(&kernel, "Urgent but isolated", &[("priority", "0")]); // #2
    append(&kernel, "Downstream A", &[]); // #3
    append(&kernel, "Downstream B", &[]); // #4
    append(&kernel, "Downstream C", &[]); // #5
    for blocked in ["#3", "#4", "#5"] {
        blocks(&kernel, "#1", blocked);
    }

    let kata = source(&kernel, "urn:iki:ledger:next", &[("limit", "1")]);
    assert!(kata.contains("Urgent but isolated"), "{kata}");

    let leverage = source(
        &kernel,
        "urn:iki:ledger:next",
        &[("policy", "leverage"), ("limit", "1")],
    );
    assert!(leverage.contains("Unblocks three"), "{leverage}");
    assert!(leverage.contains("unblocks 3 open item(s)"), "{leverage}");
}

#[test]
fn an_unknown_policy_is_refused_and_the_known_ones_are_named() {
    let kernel = kernel();
    let refused = try_verb(
        &kernel,
        Verb::Source,
        "urn:iki:ledger:next",
        &[("policy", "vibes")],
    );
    let message = refused.expect_err("no such policy").to_string();
    assert!(message.contains("priority-recency"), "{message}");
    assert!(message.contains("leverage"), "{message}");
}

/// A policy is a resource: resolvable, describable, and what `Meta` and the manifold
/// advertise.
#[test]
fn a_policy_is_a_resource_that_says_what_it_weighs() {
    let kernel = kernel();
    let plain = source(&kernel, "urn:iki:ledger:policy:leverage", &[]);
    assert!(plain.contains("weighs: leverage, priority, age"), "{plain}");

    let turtle = source(
        &kernel,
        "urn:iki:ledger:policy:priority-recency",
        &[("as", "text/turtle")],
    );
    assert!(turtle.contains("ledger:Policy"), "{turtle}");
    assert!(turtle.contains("ledger:weighs"), "{turtle}");
    assert!(!turtle.contains("_:"), "{turtle}");

    assert!(try_verb(&kernel, Verb::Source, "urn:iki:ledger:policy:nonesuch", &[]).is_err());
}

/// ★ The explanation face. "Do this next" that cannot be interrogated gets overridden
/// and then ignored.
#[test]
fn the_selection_can_emit_its_reasoning_as_a_graph() {
    let kernel = kernel();
    append(&kernel, "Do this", &[("priority", "0")]);
    append(&kernel, "Then this", &[("priority", "2")]);
    append(&kernel, "Not this", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:claim",
        &[("item", "#3"), ("content", "session-a")],
    );

    let turtle = source(
        &kernel,
        "urn:iki:ledger:next",
        &[("as", "text/turtle"), ("limit", "2")],
    );
    assert!(turtle.contains("ledger:Selection"), "{turtle}");
    assert!(turtle.contains("ledger:policy"), "{turtle}");
    assert!(turtle.contains("ledger:ranking"), "{turtle}");
    assert!(turtle.contains("ledger:because"), "{turtle}");
    assert!(turtle.contains("ledger:Exclusion"), "{turtle}");
    assert!(turtle.contains("claimed by session-a"), "{turtle}");
    assert!(turtle.contains("prov:generatedAtTime"), "{turtle}");
    assert!(!turtle.contains("_:"), "skolemized, always: {turtle}");
    // The ranked items are IN the graph, so "what is it" needs no second read.
    assert!(turtle.contains("Do this"), "{turtle}");
}

/// kata's `ready --limit N` truncates by recency and THEN ranks, so "the highest-priority
/// ready issue" silently means "the best of the N most recently touched". Ours ranks
/// first.
#[test]
fn the_limit_is_applied_after_ranking_not_before() {
    let kernel = kernel();
    append(&kernel, "Oldest and most urgent", &[("priority", "0")]);
    for n in 1..=5 {
        append(&kernel, &format!("Newer noise {n}"), &[]);
    }
    let next = source(&kernel, "urn:iki:ledger:next", &[("limit", "1")]);
    assert!(next.contains("Oldest and most urgent"), "{next}");
    assert!(
        next.contains("ready: 6"),
        "the count is before the limit: {next}"
    );
}

/// Level-scoped selection: "what is next" among findings is a different question from
/// "what is next" among decisions, and the scoping is a filter in the ready query.
#[test]
fn selection_can_be_scoped_to_one_level() {
    let kernel = kernel();
    append(
        &kernel,
        "A review finding",
        &[("kind", "urn:example:ledger:Finding"), ("priority", "3")],
    );
    append(&kernel, "An ordinary item", &[("priority", "0")]);

    let everything = source(&kernel, "urn:iki:ledger:next", &[("limit", "1")]);
    assert!(everything.contains("An ordinary item"), "{everything}");

    let findings = source(
        &kernel,
        "urn:iki:ledger:next",
        &[("kind", "urn:example:ledger:Finding"), ("limit", "5")],
    );
    assert!(findings.contains("A review finding"), "{findings}");
    assert!(!findings.contains("An ordinary item"), "{findings}");

    // …and the level is a TYPE in the graph, beside the base class, so a new level is
    // additive rather than a schema change.
    let turtle = source(&kernel, "urn:iki:ledger:item:1", &[("as", "text/turtle")]);
    assert!(turtle.contains("urn:example:ledger:Finding"), "{turtle}");
    assert!(turtle.contains("ledger:Item"), "{turtle}");
}

#[test]
fn an_empty_ledger_says_nothing_is_ready_rather_than_failing() {
    let kernel = kernel();
    let next = source(&kernel, "urn:iki:ledger:next", &[]);
    assert!(next.contains("nothing is ready"), "{next}");
    assert!(next.contains("no open items"), "{next}");
}
