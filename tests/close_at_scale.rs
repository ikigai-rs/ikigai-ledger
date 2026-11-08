//! **A close and a reopen cost in proportion to the item, not the ledger's closed history**
//! (ledger #3256).
//!
//! The store's narrow write door (`urn:iki:store:graph-update`) evaluates an update against a
//! private copy of the quads its patterns can match, found by their constants, and refuses past
//! `DurableStore::with_max_scoped_copy_quads` (250,000 by default). Through 0.20.0 a close found
//! the close that stands with `?standing a StatusEvent ; status closed ; eventSequence ?n`, and
//! both a close and a reopen numbered their event by counting `?past a StatusEvent`: patterns
//! whose only constants are a predicate and a class. So every close and reopen copied every
//! status event of the ledger and the status of every closed item, at least four quads per
//! closed item, and a ledger of about 62,500 closed items could close nothing (measured in
//! ikigai-ledger PR 97, `tests/scale_round.rs`). The legacy `backfill-closed-at content=all`
//! copied every `closedAt` the same way.
//!
//! These tests run the same shape at a small bound, so a debug build can afford it: a ledger of
//! closed items whose status events alone are past the bound, and an item well inside it. Each
//! failed on 0.20.0 because of the defect, refused with "this update has patterns that match
//! that many of the graph's quads".

mod common;

use common::*;
use ikigai_core::Kernel;
use ikigai_store::DurableStore;

const LIVE: &str = "urn:iki:ledger:graph:default";
const NS: &str = "https://ikigai-rs.dev/ns/ledger#";
const DC: &str = "http://purl.org/dc/terms/";
const XSD_DATETIME: &str = "http://www.w3.org/2001/XMLSchema#dateTime";

/// The copy bound: a few thousand quads.
const BOUND: usize = 2_000;

/// Closed filler items: their status events, their `status closed` and their `closedAt` are
/// each past [`BOUND`] on their own.
const CLOSED: usize = 2_500;

/// A kernel whose store copies at most `bound` quads into a scoped update.
fn kernel_bounded(bound: usize) -> Kernel {
    kernel_over(
        DurableStore::in_memory()
            .expect("an in-memory store")
            .with_max_scoped_copy_quads(bound),
    )
}

/// `items` closed filler items, each with the status event a close writes and an imported
/// status event beside it, written straight into the live graph as data (which the scoped door
/// copies nothing for), 200 to an update. `dated` gives each its `closedAt`.
fn fill_closed(kernel: &Kernel, items: usize, tag: &str, dated: bool) {
    const PER_UPDATE: usize = 200;
    let at = format!("\"2020-02-01T00:00:00Z\"^^<{XSD_DATETIME}>");
    let mut first = 0;
    while first < items {
        let last = (first + PER_UPDATE).min(items);
        let mut data = String::new();
        for k in first..last {
            let item = format!("urn:iki:ledger:default:item:{tag}{k:08}");
            data.push_str(&format!(
                "<{item}> a <{NS}Item> ; <{NS}number> {n} ; <{DC}title> \"filler {k}\" ; \
                 <{NS}status> <{NS}closed> ; <{NS}closedReason> <{NS}done> ; \
                 <{DC}created> {at} ; <{DC}modified> {at} .\n\
                 <{item}:status:a> a <{NS}StatusEvent> ; <{NS}onItem> <{item}> ; \
                 <{NS}status> <{NS}closed> ; <{DC}created> {at} ; <{NS}eventSequence> 1 .\n\
                 <{item}:status:b> a <{NS}StatusEvent> ; <{NS}onItem> <{item}> ; \
                 <{NS}status> <{NS}closed> ; <{DC}created> {at} ; <{NS}eventKey> \"src-{k}\" .\n",
                n = 1_000_000 + k
            ));
            if dated {
                data.push_str(&format!("<{item}> <{NS}closedAt> {at} .\n"));
            }
        }
        sink(
            kernel,
            "urn:iki:store:graph-update",
            &[
                ("graph", LIVE),
                (
                    "content",
                    &format!("INSERT DATA {{ GRAPH <{LIVE}> {{ {data} }} }}"),
                ),
            ],
        );
        first = last;
    }
}

/// A ledger of [`CLOSED`] closed items and one open item, #1.
fn closed_ledger() -> Kernel {
    let kernel = kernel_bounded(BOUND);
    append(&kernel, "The one we close", &[]);
    append(&kernel, "Its evidence", &[]);
    fill_closed(&kernel, CLOSED, "done", true);
    kernel
}

/// #`number`'s non-imported status events, in sequence order: `status reason evidence`.
fn status_events(kernel: &Kernel, number: i64) -> Vec<String> {
    let json: serde_json::Value = serde_json::from_str(&select(
        kernel,
        &format!(
            "SELECT ?seq ?status ?reason (GROUP_CONCAT(STR(?evidence)) AS ?ev) WHERE {{ \
             GRAPH <{LIVE}> {{ ?item <{NS}number> {number} . ?e a <{NS}StatusEvent> ; \
             <{NS}onItem> ?item ; <{NS}status> ?status ; <{NS}eventSequence> ?seq . \
             OPTIONAL {{ ?e <{NS}closedReason> ?reason }} OPTIONAL {{ ?e <{NS}evidence> ?evidence }} \
             }} }} GROUP BY ?seq ?status ?reason ORDER BY ?seq"
        ),
    ))
    .expect("JSON");
    json["results"]["bindings"]
        .as_array()
        .expect("bindings")
        .iter()
        .map(|row| {
            let tail = |var: &str| {
                row[var]["value"]
                    .as_str()
                    .map(|v| v.rsplit(['#', ':']).next().unwrap_or(v).to_string())
                    .unwrap_or_default()
            };
            format!(
                "{} {} {} {}",
                row["seq"]["value"].as_str().unwrap_or("?"),
                tail("status"),
                tail("reason"),
                row["ev"]["value"].as_str().unwrap_or("")
            )
            .trim()
            .to_string()
        })
        .collect()
}

/// Close, close again with a new verdict, reopen and close again, in a ledger whose closed
/// history is past the bound: each lands, numbered in turn, and the evidence and the verdict
/// go on the close that stands.
#[test]
fn close_and_reopen_in_a_ledger_past_the_bound_land_and_number_their_events() {
    let kernel = closed_ledger();
    let closed = try_verb(
        &kernel,
        ikigai_core::Verb::Sink,
        "urn:iki:ledger:close",
        &[("item", "#1"), ("evidence", "#2")],
    )
    .unwrap_or_else(|e| panic!("close with {CLOSED} closed items: {e}"));
    assert!(closed.starts_with("closed #1 done"), "{closed}");

    // Already closed: the verdict changes on the close that stands, and the evidence joins it.
    let again = try_verb(
        &kernel,
        ikigai_core::Verb::Sink,
        "urn:iki:ledger:close",
        &[
            ("item", "#1"),
            ("reason", "wontfix"),
            ("evidence", "urn:example:test:proof"),
        ],
    )
    .unwrap_or_else(|e| panic!("a verdict edit with {CLOSED} closed items: {e}"));
    assert!(again.contains("already closed"), "{again}");

    let reopened = try_verb(
        &kernel,
        ikigai_core::Verb::Sink,
        "urn:iki:ledger:reopen",
        &[("item", "#1")],
    )
    .unwrap_or_else(|e| panic!("reopen with {CLOSED} closed items: {e}"));
    assert_eq!(reopened, "reopened #1\n");

    try_verb(
        &kernel,
        ikigai_core::Verb::Sink,
        "urn:iki:ledger:close",
        &[("item", "#1"), ("reason", "duplicate"), ("evidence", "#2")],
    )
    .unwrap_or_else(|e| panic!("a second close with {CLOSED} closed items: {e}"));

    let events = status_events(&kernel, 1);
    assert_eq!(events.len(), 3, "{events:?}");
    assert!(events[0].starts_with("1 closed wontfix "), "{events:?}");
    assert!(
        events[0].contains("proof"),
        "the edit's evidence joins the first close: {events:?}"
    );
    assert_eq!(events[1], "2 open", "{events:?}");
    assert!(events[2].starts_with("3 closed duplicate "), "{events:?}");
}

/// A reopen of an item closed through the store with no status event preserves that close as
/// an event first, as it always did, in a ledger past the bound: the close at sequence 0, and
/// the reopen counting it.
#[test]
fn a_reopen_preserves_an_eventless_close_in_a_ledger_past_the_bound() {
    let kernel = closed_ledger();
    let item = "urn:iki:ledger:default:item:legacy";
    sink(
        &kernel,
        "urn:iki:store:graph-update",
        &[
            ("graph", LIVE),
            (
                "content",
                &format!(
                    "INSERT DATA {{ GRAPH <{LIVE}> {{ <{item}> a <{NS}Item> ; <{NS}number> 7 ; \
                     <{DC}title> \"legacy\" ; <{NS}status> <{NS}closed> ; \
                     <{DC}created> \"2020-01-01T00:00:00Z\"^^<{XSD_DATETIME}> ; \
                     <{DC}modified> \"2020-01-01T00:00:00Z\"^^<{XSD_DATETIME}> ; \
                     <{NS}closedAt> \"2020-01-02T00:00:00Z\"^^<{XSD_DATETIME}> }} }}"
                ),
            ),
        ],
    );
    try_verb(
        &kernel,
        ikigai_core::Verb::Sink,
        "urn:iki:ledger:reopen",
        &[("item", "#7")],
    )
    .unwrap_or_else(|e| panic!("reopen with {CLOSED} closed items: {e}"));
    let events = status_events(&kernel, 7);
    assert_eq!(events, ["0 closed", "2 open"], "{events:?}");
}

/// `backfill-closed-at content=all` dates the undated closed items, in a ledger whose dated
/// ones are past the bound.
#[test]
fn backfill_all_in_a_ledger_past_the_bound_dates_the_undated() {
    let kernel = closed_ledger();
    fill_closed(&kernel, 3, "undated", false);
    let answer = try_verb(
        &kernel,
        ikigai_core::Verb::Sink,
        "urn:iki:ledger:backfill-closed-at",
        &[("content", "all")],
    )
    .unwrap_or_else(|e| panic!("backfill with {CLOSED} dated closed items: {e}"));
    assert_eq!(answer, "backfilled 3 items\n");
}
