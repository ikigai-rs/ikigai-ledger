//! **Relabeling an item that is already filed** (ledger #1234).
//!
//! Remote workers are routed by label (`worker:<name>`), and #1234 reported that an item could
//! not be handed to a worker after filing because "labels are set only at append". That is not
//! so: `urn:iki:ledger:{ledger}:label` has added (Sink) and removed (Delete) a label on an
//! existing item since the first release, and `tests/endpoints.rs` covers the round trip. What
//! no test said is what a DISPATCHER needs from it, so these pin that:
//!
//! - a re-route (remove `worker:a`, add `worker:b`) is visible at once to `next`, `count`,
//!   `items` and the item itself, in the plain and JSON faces, over reads that were CACHED
//!   before the edit (the golden thread, not a cold cache, is what is under test);
//! - the edit stamps the item's `modified` time;
//! - the worker grant can do neither half: adding and removing are both `Denied`.

mod common;

use common::*;
use ikigai_core::{Capability, Error, Kernel, Verb};

const LEDGER: &str = "acme";

fn at(resource: &str) -> String {
    format!("urn:iki:ledger:{LEDGER}:{resource}")
}

/// The worker grant `README.md` publishes: read, claim, comment, and the graph's store grants.
fn worker() -> Capability {
    Capability::scoped(vec![
        format!("urn:cap:ledger:read:{LEDGER}"),
        format!("urn:cap:ledger:claim:{LEDGER}"),
        format!("urn:cap:ledger:comment:{LEDGER}"),
        graph_read(LEDGER),
        graph_write(LEDGER),
    ])
}

fn json(kernel: &Kernel, iri: &str, args: &[(&str, &str)]) -> serde_json::Value {
    let mut all = vec![("as", "application/json")];
    all.extend_from_slice(args);
    serde_json::from_str(&source(kernel, iri, &all)).expect("the JSON face parses")
}

fn count(kernel: &Kernel, label: &str) -> serde_json::Value {
    json(kernel, &at("count"), &[("labels", label)])["total"].clone()
}

/// Every read a dispatcher or a worker routes by, for one label, as text.
fn routed(kernel: &Kernel, label: &str) -> [String; 3] {
    [
        source(kernel, &at("next"), &[("labels", label)]),
        source(kernel, &at("items"), &[("labels", label)]),
        json(kernel, &at("next"), &[("labels", label)]).to_string(),
    ]
}

#[test]
fn a_filed_item_is_rerouted_and_every_read_sees_it_at_once() {
    let kernel = kernel();
    sink(
        &kernel,
        &at("append"),
        &[("content", "Port the widget"), ("labels", "worker:lawnboy")],
    );
    sink(&kernel, &at("append"), &[("content", "Unrouted")]);

    // Read everything first, so each answer is in the cache when the edit lands.
    for read in routed(&kernel, "worker:lawnboy") {
        assert!(read.contains("Port the widget"), "{read}");
    }
    for read in routed(&kernel, "worker:bug") {
        assert!(!read.contains("Port the widget"), "{read}");
    }
    assert_eq!(count(&kernel, "worker:lawnboy"), 1);
    assert_eq!(count(&kernel, "worker:bug"), 0);
    let before = json(&kernel, &at("item:1"), &[]);
    assert_eq!(
        before["item"]["labels"],
        serde_json::json!(["worker:lawnboy"])
    );
    let cache = source(&kernel, "urn:kernel:cache", &[]);
    assert!(cache.contains(&at("next")), "next was not cached: {cache}");
    assert!(
        cache.contains(&at("count")),
        "count was not cached: {cache}"
    );

    // Remove first, then add: the item is never routed to two workers at once.
    let removed = delete(
        &kernel,
        &at("label"),
        &[("item", "1"), ("content", "worker:lawnboy")],
    );
    assert!(removed.starts_with("unlabeled acme#1"), "{removed}");
    let added = sink(
        &kernel,
        &at("label"),
        &[("item", "1"), ("content", "worker:bug")],
    );
    assert!(added.starts_with("labeled acme#1"), "{added}");

    for read in routed(&kernel, "worker:bug") {
        assert!(
            read.contains("Port the widget"),
            "stale after the edit: {read}"
        );
    }
    for read in routed(&kernel, "worker:lawnboy") {
        assert!(
            !read.contains("Port the widget"),
            "stale after the edit: {read}"
        );
    }
    assert_eq!(count(&kernel, "worker:bug"), 1);
    assert_eq!(count(&kernel, "worker:lawnboy"), 0);

    let plain = source(&kernel, &at("item:1"), &[]);
    assert!(plain.contains("[worker:bug]"), "{plain}");
    assert!(!plain.contains("worker:lawnboy"), "{plain}");
    let after = json(&kernel, &at("item:1"), &[]);
    assert_eq!(after["item"]["labels"], serde_json::json!(["worker:bug"]));
    assert!(
        after["item"]["modified"].as_str() > before["item"]["modified"].as_str(),
        "the edit did not stamp `modified`: {before} -> {after}"
    );
}

fn denied(result: Result<String, Error>, what: &str) {
    match result {
        Err(Error::Denied(why)) => assert!(why.contains("urn:cap:ledger:write:"), "{what}: {why}"),
        other => panic!("{what}: expected Denied, got {other:?}"),
    }
}

/// A worker is routed by its label, so it must not be able to route itself: neither half of a
/// relabel is open to the worker grant, and the item keeps its labels.
#[test]
fn a_worker_can_neither_add_nor_remove_a_label() {
    let kernel = kernel();
    sink(
        &kernel,
        &at("append"),
        &[("content", "Port the widget"), ("labels", "worker:lawnboy")],
    );
    let w = worker();
    denied(
        try_as(
            &kernel,
            &w,
            Verb::Sink,
            &at("label"),
            &[("item", "1"), ("content", "worker:bug")],
        ),
        "a worker adding a label",
    );
    denied(
        try_as(
            &kernel,
            &w,
            Verb::Delete,
            &at("label"),
            &[("item", "1"), ("content", "worker:lawnboy")],
        ),
        "a worker removing a label",
    );
    let item = json(&kernel, &at("item:1"), &[]);
    assert_eq!(
        item["item"]["labels"],
        serde_json::json!(["worker:lawnboy"])
    );
}
