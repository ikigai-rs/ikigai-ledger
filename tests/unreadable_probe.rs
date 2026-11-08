//! **The probe that keeps the corpus check off the listing path** (ledger #3255).
//!
//! Measured in the scale round (`tests/scale_round.rs`, ledger #3241): every `items` face
//! that states a total ran `model::defects`, two ordered scans of every item, open and
//! closed, whatever the filter and the limit — 5.6 s of 6.4 s at 10x. Now `items` asks
//! `model::may_have_defects` first, one `ASK`, and runs the check only when it says yes.
//!
//! The ⚠ footer and JSON `unreadable` are unchanged, so the probe must never say "clean"
//! over a graph the check reports on. Every shape below is held both ways: through the
//! probe's own query at the store's door, and through what `items` reports.

mod common;

use common::*;

const LIVE: &str = "urn:iki:ledger:graph:default";
const NUMBER: &str = "https://ikigai-rs.dev/ns/ledger#number";
const STATUS: &str = "https://ikigai-rs.dev/ns/ledger#status";
const TITLE: &str = "http://purl.org/dc/terms/title";
const CREATED: &str = "http://purl.org/dc/terms/created";
const MODIFIED: &str = "http://purl.org/dc/terms/modified";
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

fn hand_edit(kernel: &ikigai_core::Kernel, update: &str) {
    sink(
        kernel,
        "urn:iki:store:graph-update",
        &[("graph", LIVE), ("content", update)],
    );
}

/// Replace every value of `predicate` on `iri` with `object` (or with nothing).
fn replace(kernel: &ikigai_core::Kernel, iri: &str, predicate: &str, object: Option<&str>) {
    let insert = object
        .map(|o| format!("INSERT {{ GRAPH <{LIVE}> {{ <{iri}> <{predicate}> {o} }} }} "))
        .unwrap_or_default();
    hand_edit(
        kernel,
        &format!(
            "DELETE {{ GRAPH <{LIVE}> {{ <{iri}> <{predicate}> ?o }} }} {insert}\
             WHERE {{ GRAPH <{LIVE}> {{ <{iri}> <{predicate}> ?o }} }}"
        ),
    );
}

/// What the probe answers, asked through the store's narrow door.
fn probe(kernel: &ikigai_core::Kernel) -> bool {
    let answer = source(
        kernel,
        "urn:iki:store:graph-ask",
        &[
            ("graph", LIVE),
            ("query", &ikigai_ledger::model::defect_probe_query(LIVE)),
        ],
    );
    let json: serde_json::Value = serde_json::from_str(&answer).expect("an ASK answer");
    json["boolean"].as_bool().expect("a boolean")
}

/// What `items` reports as unreadable, in JSON.
fn unreadable(kernel: &ikigai_core::Kernel) -> Vec<serde_json::Value> {
    let doc: serde_json::Value = serde_json::from_str(&source(
        kernel,
        "urn:iki:ledger:items",
        &[("as", "application/json"), ("state", "all")],
    ))
    .expect("the JSON face parses");
    doc["unreadable"].as_array().cloned().unwrap_or_default()
}

/// A ledger of two items, one open and one closed, written only through the ledger.
fn clean() -> (ikigai_core::Kernel, String) {
    let kernel = kernel();
    let iri = append_iri(&kernel, "Probed", &[]);
    append_iri(&kernel, "Closed one", &[]);
    sink(&kernel, "urn:iki:ledger:close", &[("item", "2")]);
    (kernel, iri)
}

/// A ledger the ledger wrote is clean, and the probe says so: this is the path that no
/// longer pays for the corpus check.
#[test]
fn a_ledger_written_through_its_sinks_needs_no_corpus_check() {
    let (kernel, _) = clean();
    assert!(!probe(&kernel));
    assert!(unreadable(&kernel).is_empty());
}

/// A named hand edit: each predicate replaced by the object, or removed.
type Shape = (&'static str, Vec<(&'static str, Option<String>)>);

fn dt(lexical: &str) -> String {
    format!("\"{lexical}\"^^<{XSD}dateTime>")
}

/// Every hand edit the check reports on, each of which the probe must see.
#[test]
fn the_probe_sees_every_item_the_corpus_check_reports() {
    let shapes: Vec<Shape> = vec![
        ("missing title", vec![(TITLE, None)]),
        ("missing status", vec![(STATUS, None)]),
        ("missing number", vec![(NUMBER, None)]),
        ("number in words", vec![(NUMBER, Some("\"twelve\"".into()))]),
        (
            "number past i64",
            vec![(NUMBER, Some("\"99999999999999999999\"".into()))],
        ),
        (
            "both timestamps missing",
            vec![(CREATED, None), (MODIFIED, None)],
        ),
        (
            "both timestamps xsd:date",
            vec![
                (CREATED, Some(format!("\"2026-11-07\"^^<{XSD}date>"))),
                (MODIFIED, Some(format!("\"2026-11-07\"^^<{XSD}date>"))),
            ],
        ),
        (
            "both timestamps in words",
            vec![(CREATED, Some("\"yesterday\"".into())), (MODIFIED, None)],
        ),
        (
            "both timestamps before the epoch",
            vec![
                (CREATED, Some(dt("1969-12-31T23:59:59Z"))),
                (MODIFIED, Some(dt("1969-12-31T23:59:59Z"))),
            ],
        ),
    ];
    for (name, edits) in shapes {
        let (kernel, iri) = clean();
        for (predicate, object) in &edits {
            replace(&kernel, &iri, predicate, object.as_deref());
        }
        let reported = unreadable(&kernel);
        assert_eq!(
            reported.len(),
            1,
            "{name}: the check reports it: {reported:?}"
        );
        assert_eq!(reported[0]["iri"], iri, "{name}");
        assert!(probe(&kernel), "{name}: the probe must see it");
    }
}

/// The probe is narrower than the reader on purpose, so a value the reader reads but the
/// probe does not vouch for costs the full check and reports nothing: a false alarm,
/// never a missed report.
#[test]
fn a_value_the_probe_cannot_vouch_for_only_costs_the_check() {
    let shapes: Vec<Shape> = vec![
        (
            "timestamps with an offset",
            vec![
                (CREATED, Some(dt("2026-11-07T10:00:00+02:00"))),
                (MODIFIED, Some(dt("2026-11-07T10:00:00+02:00"))),
            ],
        ),
        (
            "one timestamp in words",
            vec![(MODIFIED, Some("\"yesterday\"".into()))],
        ),
    ];
    for (name, edits) in shapes {
        let (kernel, iri) = clean();
        for (predicate, object) in &edits {
            replace(&kernel, &iri, predicate, object.as_deref());
        }
        assert!(
            unreadable(&kernel).is_empty(),
            "{name}: listed, not reported"
        );
        let _ = probe(&kernel);
    }
}
