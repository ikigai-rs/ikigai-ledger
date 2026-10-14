//! **`labels=` takes any number of labels** (ledger #1452).
//!
//! Through 0.11.1 each label in `labels=` added one triple pattern to the main join of every
//! query a filter becomes, and the store refuses a join wider than 32 operands before planning
//! it. So `items` and `count` refused at 28 labels (24 with `kind`, `about`, `key` and `holder`
//! also set) and `next` at 30, with an error about SPARQL patterns that never mentioned
//! `labels=`. Now the first label is a pattern and the rest one `VALUES` list inside a
//! `FILTER NOT EXISTS`, which adds nothing to the main join however long the list is. These
//! hold that at 1, 28 and 100 labels on all three resources, with every other filter set at
//! once. (`model.rs` measures the join width itself, by the store's own walk.)

mod common;

use common::*;
use ikigai_core::Kernel;

fn json(kernel: &Kernel, iri: &str, args: &[(&str, &str)]) -> serde_json::Value {
    let mut all = vec![("as", "application/json")];
    all.extend_from_slice(args);
    serde_json::from_str(&source(kernel, iri, &all)).expect("the JSON face parses")
}

/// `l0,l1,…` — the first `n` of the labels #1 carries.
fn labels(n: usize) -> String {
    (0..n)
        .map(|i| format!("l{i}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// #1 carries 100 labels; #2 carries only `l0`; #3 carries none.
fn ledger() -> Kernel {
    let kernel = kernel();
    append(
        &kernel,
        "Carries a hundred labels",
        &[
            ("labels", &labels(100)),
            ("about", "urn:repo:file:x/src/a.rs"),
            ("key", "urn:test:hundred"),
        ],
    );
    append(&kernel, "Carries one", &[("labels", "l0")]);
    append(&kernel, "Carries none", &[]);
    kernel
}

/// The numbers a listing's JSON face names, in order.
fn numbers(doc: &serde_json::Value, field: &str) -> Vec<i64> {
    doc[field]
        .as_array()
        .expect("a list")
        .iter()
        .map(|row| {
            row.get("item")
                .unwrap_or(row)
                .get("number")
                .and_then(|n| n.as_i64())
                .expect("a number")
        })
        .collect()
}

#[test]
fn items_count_and_next_take_one_twenty_eight_and_a_hundred_labels() {
    let kernel = ledger();
    for n in [1, 28, 100] {
        let wanted = labels(n);
        let args = [("labels", wanted.as_str())];
        let expected: Vec<i64> = if n == 1 { vec![2, 1] } else { vec![1] };

        let items = json(&kernel, "urn:iki:ledger:items", &args);
        assert_eq!(numbers(&items, "items"), expected, "items, {n} label(s)");
        assert_eq!(items["total"], expected.len(), "items total, {n} label(s)");

        let count = json(&kernel, "urn:iki:ledger:count", &args);
        assert_eq!(count["total"], expected.len(), "count, {n} label(s)");

        let next = json(
            &kernel,
            "urn:iki:ledger:next",
            &[("labels", wanted.as_str()), ("limit", "10")],
        );
        let mut offered = numbers(&next, "ranking");
        offered.sort_unstable();
        let mut want = expected.clone();
        want.sort_unstable();
        assert_eq!(offered, want, "next, {n} label(s)");
    }
}

/// The worst case the item measured: every other filter that adds a pattern, set at once,
/// beside a hundred labels.
#[test]
fn a_hundred_labels_beside_every_other_filter() {
    let kernel = ledger();
    sink(
        &kernel,
        "urn:iki:ledger:claim",
        &[("item", "#1"), ("content", "satellite")],
    );
    let wanted = labels(100);
    let args = [
        ("labels", wanted.as_str()),
        ("kind", "https://ikigai-rs.dev/ns/ledger#Item"),
        ("about", "urn:repo:file:x/src/a.rs"),
        ("key", "urn:test:hundred"),
        ("holder", "satellite"),
        ("deferred", "exclude"),
        ("text", "hundred"),
        ("without", "never,nor-this"),
    ];
    let items = json(&kernel, "urn:iki:ledger:items", &args);
    assert_eq!(numbers(&items, "items"), vec![1], "{items}");
    assert_eq!(json(&kernel, "urn:iki:ledger:count", &args)["total"], 1);
    // `next` explains why it does not offer the claimed item, which means it admitted it.
    let next = source(
        &kernel,
        "urn:iki:ledger:next",
        &[
            ("labels", wanted.as_str()),
            ("about", "urn:repo:file:x/src/a.rs"),
            ("kind", "https://ikigai-rs.dev/ns/ledger#Item"),
        ],
    );
    assert!(next.contains("#1 claimed by satellite"), "{next}");
}

/// One label missing from a hundred is enough to exclude the item, and a label named twice is
/// one label (a `COUNT` over the wanted list would have been fooled by the duplicate).
#[test]
fn every_label_is_required_and_a_repeat_counts_once() {
    let kernel = ledger();
    let mut missing = labels(99);
    missing.push_str(",l-absent");
    for iri in ["urn:iki:ledger:items", "urn:iki:ledger:count"] {
        assert_eq!(
            json(&kernel, iri, &[("labels", &missing)])["total"],
            0,
            "{iri}"
        );
        assert_eq!(
            json(&kernel, iri, &[("labels", "l0,l0,l1,l1")])["total"],
            1,
            "{iri}"
        );
    }
    let next = json(&kernel, "urn:iki:ledger:next", &[("labels", &missing)]);
    assert!(numbers(&next, "ranking").is_empty(), "{next}");
}

/// A label that needs escaping is still a label, inside the list as it was as a pattern.
#[test]
fn a_hostile_label_in_the_list_is_a_literal() {
    let kernel = kernel();
    append(
        &kernel,
        "Hostile",
        &[("labels", "a\"b,c\\d,e } FILTER(false) {")],
    );
    append(&kernel, "Plain", &[("labels", "a")]);
    let doc = json(
        &kernel,
        "urn:iki:ledger:items",
        &[("labels", "a\"b,c\\d,e } FILTER(false) {")],
    );
    assert_eq!(numbers(&doc, "items"), vec![1], "{doc}");
}
