//! **`count` is the listing's `total`, and costs only the count** (ledger #1128).
//!
//! gonk's ledger page read `items limit=1 as=application/json` for its `total`, and that face
//! always ran the corpus check behind `unreadable` — two scans of the whole graph — so the
//! number cost several times what counting it does (`tests/listing_cost.rs` measures it).
//! `urn:iki:ledger:count` answers the number alone. These hold it to being the SAME number:
//! the same filter parser and the same COUNT query, over every filter, a hand-edited item
//! the listing cannot read, and the same refusals.

mod common;

use common::*;
use ikigai_core::{Error, Verb};

const LIVE: &str = "urn:iki:ledger:graph:default";
const NUMBER: &str = "https://ikigai-rs.dev/ns/ledger#number";

fn json(kernel: &ikigai_core::Kernel, iri: &str, args: &[(&str, &str)]) -> serde_json::Value {
    let mut all = vec![("as", "application/json")];
    all.extend_from_slice(args);
    serde_json::from_str(&source(kernel, iri, &all)).expect("the JSON face parses")
}

/// A small ledger with something for every filter to select: labels, an `about`, a key, a
/// claim, a deferral, a closed item and a kind-less title to search.
fn ledger() -> ikigai_core::Kernel {
    let kernel = kernel();
    append(
        &kernel,
        "Fix the parser",
        &[
            ("labels", "rust,audit"),
            ("about", "urn:repo:file:x/src/a.rs"),
        ],
    );
    append(&kernel, "Write the docs", &[("labels", "docs")]);
    append(&kernel, "Keyed", &[("key", "urn:kata:issue:01JZ")]);
    append(&kernel, "Parser, again", &[("labels", "rust")]);
    append(&kernel, "Closed one", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:claim",
        &[("item", "#2"), ("content", "satellite")],
    );
    sink(
        &kernel,
        "urn:iki:ledger:defer",
        &[("item", "#4"), ("content", "not this quarter")],
    );
    sink(
        &kernel,
        "urn:iki:ledger:close",
        &[("item", "#5"), ("reason", "done"), ("content", "finished")],
    );
    kernel
}

/// Every filter the two share, as argument lists.
fn filters() -> Vec<Vec<(&'static str, &'static str)>> {
    vec![
        vec![],
        vec![("status", "all")],
        vec![("status", "closed")],
        vec![("labels", "rust")],
        vec![("labels", "rust,audit")],
        vec![("without", "rust")],
        vec![("about", "urn:repo:file:x/src/a.rs")],
        vec![("key", "urn:kata:issue:01JZ")],
        vec![("holder", "none")],
        vec![("holder", "satellite")],
        vec![("deferred", "exclude")],
        vec![("deferred", "only")],
        vec![("text", "parser")],
        vec![
            ("status", "all"),
            ("labels", "rust"),
            ("deferred", "exclude"),
        ],
    ]
}

#[test]
fn count_is_the_listing_total_for_every_filter() {
    let kernel = ledger();
    for args in filters() {
        let listed = json(&kernel, "urn:iki:ledger:items", &args)["total"].clone();
        let counted = json(&kernel, "urn:iki:ledger:count", &args)["total"].clone();
        assert_eq!(counted, listed, "filter {args:?}");
        let plain = source(&kernel, "urn:iki:ledger:count", &args);
        assert_eq!(plain, format!("{counted}\n"), "filter {args:?}");
    }
    // Not vacuous: the filters really select different sets.
    assert_eq!(json(&kernel, "urn:iki:ledger:count", &[])["total"], 4);
    assert_eq!(
        json(&kernel, "urn:iki:ledger:count", &[("labels", "rust")])["total"],
        2
    );
    assert_eq!(
        json(&kernel, "urn:iki:ledger:count", &[("deferred", "only")])["total"],
        1
    );
}

/// An item a hand edit made unreadable is counted by both — the listing's `total` counts it
/// and reports it in `unreadable` rather than listing it — so `count` counts it too. What
/// `count` does not do is the reporting: that is the part that cost two scans.
#[test]
fn an_unreadable_item_is_counted_as_the_listing_counts_it() {
    let kernel = ledger();
    let iri = append_iri(&kernel, "Renumbered by hand", &[]);
    sink(
        &kernel,
        "urn:iki:store:graph-update",
        &[
            ("graph", LIVE),
            (
                "content",
                &format!(
                    "DELETE {{ GRAPH <{LIVE}> {{ <{iri}> <{NUMBER}> ?o }} }} \
                     INSERT {{ GRAPH <{LIVE}> {{ <{iri}> <{NUMBER}> \"twelve\" }} }} \
                     WHERE {{ GRAPH <{LIVE}> {{ <{iri}> <{NUMBER}> ?o }} }}"
                ),
            ),
        ],
    );
    let listing = json(&kernel, "urn:iki:ledger:items", &[]);
    assert_eq!(listing["unreadable"][0]["iri"], iri.as_str(), "{listing}");
    assert_eq!(listing["total"], 5, "{listing}");
    assert_eq!(json(&kernel, "urn:iki:ledger:count", &[])["total"], 5);
}

/// The same parser, so the same refusals — and a FACE `count` cannot give (Turtle: a number
/// is not a graph) is refused, never substituted, as is a verb it does not answer.
#[test]
fn count_refuses_what_the_listing_refuses_and_a_face_it_does_not_have() {
    let kernel = ledger();
    for bad in [("status", "opne"), ("deferred", "exlude")] {
        let listed = try_verb(&kernel, Verb::Source, "urn:iki:ledger:items", &[bad]);
        let counted = try_verb(&kernel, Verb::Source, "urn:iki:ledger:count", &[bad]);
        assert!(
            matches!(&counted, Err(Error::InvalidArgument { name, .. }) if name == bad.0),
            "{bad:?}: {counted:?}"
        );
        assert_eq!(format!("{listed:?}"), format!("{counted:?}"), "{bad:?}");
    }
    let turtle = try_verb(
        &kernel,
        Verb::Source,
        "urn:iki:ledger:count",
        &[("as", "text/turtle")],
    );
    assert!(turtle.is_err(), "{turtle:?}");
    for verb in [Verb::Sink, Verb::Delete] {
        assert!(
            try_verb(&kernel, verb, "urn:iki:ledger:count", &[]).is_err(),
            "{verb:?}"
        );
    }
}

/// A named ledger has its own count, and the grant for one ledger does not read another's.
#[test]
fn a_named_ledger_counts_its_own_items() {
    let kernel = ledger();
    sink(
        &kernel,
        "urn:iki:ledger:acme:append",
        &[("content", "Acme's only item")],
    );
    assert_eq!(json(&kernel, "urn:iki:ledger:acme:count", &[])["total"], 1);
    assert_eq!(
        json(&kernel, "urn:iki:ledger:acme:count", &[])["ledger"],
        "acme"
    );
    assert_eq!(json(&kernel, "urn:iki:ledger:count", &[])["total"], 4);
    let denied = try_as(
        &kernel,
        &ikigai_core::Capability::scoped([
            "urn:cap:ledger:read:default".to_string(),
            graph_read("default"),
        ]),
        Verb::Source,
        "urn:iki:ledger:acme:count",
        &[],
    );
    assert!(matches!(denied, Err(Error::Denied(_))), "{denied:?}");
}
