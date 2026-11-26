//! Ledger #4116: the transition feed, the ledger's outbox (ikigai-core
//! `docs/design/events-and-resources.md`, 8.1). Before it, `urn:iki:ledger:transition` was not
//! a resource at all, and nothing numbered a ledger's transitions in commit order.

mod common;

use common::*;

const FEED: &str = "urn:iki:ledger:transition";

fn kinds(kernel: &ikigai_core::Kernel, after: &str) -> Vec<String> {
    source(kernel, FEED, &[("after", after)])
        .lines()
        .skip(1)
        .map(|line| line.split("  ").nth(1).unwrap_or_default().to_string())
        .collect()
}

#[test]
fn every_transition_is_an_entry_in_commit_order() {
    let kernel = kernel();
    let item = append_iri(&kernel, "Fix the widget", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:claim",
        &[("item", "#1"), ("content", "worker-1")],
    );
    sink(
        &kernel,
        "urn:iki:ledger:comment",
        &[("item", "#1"), ("content", "Just a note")],
    );
    sink(&kernel, "urn:iki:ledger:close", &[("item", "#1")]);
    sink(&kernel, "urn:iki:ledger:reopen", &[("item", "#1")]);

    let text = source(&kernel, FEED, &[]);
    assert!(
        text.starts_with("5 transition(s) after 0. The last is 5.\n"),
        "{text}"
    );
    assert!(text.contains(&format!("1  filed  {item}")), "{text}");
    // A plain comment is not a transition. A close of a claimed item writes its status and
    // its release in ONE update, numbered in the order that update writes them.
    assert_eq!(
        kinds(&kernel, "0"),
        ["filed", "claimed", "closed", "released", "reopened"]
    );
    // A follower resumes after the last entry it saw, and an empty page names the head.
    assert_eq!(kinds(&kernel, "3"), ["released", "reopened"]);
    assert_eq!(
        source(&kernel, FEED, &[("after", "5")]),
        "No transitions after 5. The last is 5.\n"
    );
    // The entries are named by their sequence, in the ledger's own graph.
    let rows = select(
        &kernel,
        "SELECT ?seq WHERE { GRAPH <urn:iki:ledger:graph:default> { \
         <urn:iki:ledger:default:transition:seq:4> <https://ikigai-rs.dev/ns/ledger#transitionSeq> ?seq } }",
    );
    assert!(rows.contains("4"), "{rows}");
}

#[test]
fn each_ledger_numbers_its_own() {
    let kernel = kernel();
    append(&kernel, "In default", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:acme:append",
        &[("content", "In acme")],
    );
    let acme = source(&kernel, "urn:iki:ledger:acme:transition", &[]);
    assert!(
        acme.starts_with("1 transition(s) after 0. The last is 1.\n"),
        "{acme}"
    );
}

#[test]
fn transition_is_a_reserved_ledger_name() {
    assert!(ikigai_ledger::ledger::RESERVED.contains(&"transition"));
}

/// The sequence lives on the entry only: a claim, release or status event keeps the shape its
/// readers know (no `transitionSeq`), and an entry is not one of its item's records (no
/// `ledger:onItem`), so history, the doctor and the data-safety counts never see it.
#[test]
fn an_entry_stays_apart_from_the_records_it_numbers() {
    let kernel = kernel();
    append(&kernel, "Fix the widget", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:claim",
        &[("item", "#1"), ("content", "worker-1")],
    );
    sink(&kernel, "urn:iki:ledger:close", &[("item", "#1")]);
    assert!(source(&kernel, FEED, &[]).starts_with("4 transition(s)"));
    let numbered_nodes = select(
        &kernel,
        "SELECT ?n WHERE { GRAPH <urn:iki:ledger:graph:default> { \
         ?n <https://ikigai-rs.dev/ns/ledger#transitionSeq> ?s \
         FILTER NOT EXISTS { ?n a <https://ikigai-rs.dev/ns/ledger#Transition> } } }",
    );
    assert!(!numbered_nodes.contains("urn:"), "{numbered_nodes}");
    let entries_on_items = select(
        &kernel,
        "SELECT ?n WHERE { GRAPH <urn:iki:ledger:graph:default> { \
         ?n a <https://ikigai-rs.dev/ns/ledger#Transition> ; \
         <https://ikigai-rs.dev/ns/ledger#onItem> ?i } }",
    );
    assert!(!entries_on_items.contains("urn:"), "{entries_on_items}");
    // The same query shape does see the entries, so the two absences above are not vacuous.
    let entries = select(
        &kernel,
        "SELECT ?n WHERE { GRAPH <urn:iki:ledger:graph:default> { \
         ?n a <https://ikigai-rs.dev/ns/ledger#Transition> } }",
    );
    assert!(
        entries.contains("urn:iki:ledger:default:transition:seq:4"),
        "{entries}"
    );
}
