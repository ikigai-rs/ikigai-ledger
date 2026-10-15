//! **`ledger:raised`: when the claim arose, apart from when it was filed** (ledger #240).
//!
//! `dcterms:created` is when an item was FILED, and a migration files hundreds of items in
//! minutes, so "the oldest open p1" sorted by it sorts by migration order. `append raised=`
//! records the date the claim arose (an `xsd:date`), and the honest ordering is
//! `COALESCE(?raised, ?created)`: an item filed by hand, with no `raised`, falls back to when
//! it was filed.

mod common;

use common::*;
use ikigai_core::{Error, Verb};

/// ★ The date is written as an `xsd:date`, and every face of the item carries it.
#[test]
fn append_records_when_the_claim_arose() {
    let kernel = kernel();
    append(
        &kernel,
        "Found in the July review",
        &[("raised", "2026-07-21")],
    );
    let turtle = source(&kernel, "urn:iki:ledger:item:1", &[("as", "text/turtle")]);
    assert!(
        turtle.contains("\"2026-07-21\"^^<http://www.w3.org/2001/XMLSchema#date>")
            || turtle.contains("\"2026-07-21\"^^xsd:date"),
        "{turtle}"
    );
    assert!(turtle.contains("raised"), "{turtle}");
    let plain = source(&kernel, "urn:iki:ledger:item:1", &[]);
    assert!(plain.contains("raised:   2026-07-21"), "{plain}");
    let json = source(
        &kernel,
        "urn:iki:ledger:item:1",
        &[("as", "application/json")],
    );
    assert!(json.contains("\"raised\":\"2026-07-21\""), "{json}");

    // Absent unless given: an item filed without it says nothing about it.
    append(&kernel, "Filed by hand", &[]);
    let json = source(
        &kernel,
        "urn:iki:ledger:item:2",
        &[("as", "application/json")],
    );
    assert!(json.contains("\"raised\":null"), "{json}");
    assert!(!source(&kernel, "urn:iki:ledger:item:2", &[]).contains("raised:"));
}

/// Not a calendar date, not filed: the refusal names `raised`, and no number is spent.
#[test]
fn a_raised_that_is_not_a_date_is_refused_before_anything_is_written() {
    let kernel = kernel();
    for bad in [
        "yesterday",
        "2026-13-01",
        "2026-02-30",
        "2026-7-21",
        "2026-07-21T00:00:00Z",
    ] {
        let refused = try_verb(
            &kernel,
            Verb::Sink,
            "urn:iki:ledger:append",
            &[("content", "dated badly"), ("raised", bad)],
        );
        assert!(
            matches!(refused, Err(Error::InvalidArgument { ref name, .. }) if name == "raised"),
            "{bad}: {refused:?}"
        );
    }
    // 2024 is a leap year, so its 29 February is a date.
    append(&kernel, "leap", &[("raised", "2024-02-29")]);
    assert!(source(&kernel, "urn:iki:ledger:item:1", &[]).contains("leap"));
}

/// ★ The question the predicate exists to answer: the open items, oldest CLAIM first. Items
/// 1 and 4 are filed by hand; item 2 is filed after 1 but was raised months earlier, so it
/// leads; item 3 was raised AFTER everything here was filed, so it comes last — the case that
/// shows the `xsd:date` and the `xsd:dateTime` are compared as times, not ordered apart.
#[test]
fn the_oldest_open_items_sort_by_when_they_were_raised() {
    let kernel = kernel();
    append(&kernel, "Filed first, raised when filed", &[]);
    append(
        &kernel,
        "Migrated, raised in July",
        &[("raised", "2026-07-21")],
    );
    append(
        &kernel,
        "Raised after all of it",
        &[("raised", "2026-12-01")],
    );
    append(&kernel, "Filed fourth, raised when filed", &[]);
    // The query README.md gives for it, with its prefixes.
    let query = "PREFIX ledger: <https://ikigai-rs.dev/ns/ledger#>\n\
                 PREFIX dcterms: <http://purl.org/dc/terms/>\n\
                 SELECT ?number ?title WHERE {\n  \
                 GRAPH <urn:iki:ledger:graph:default> {\n    \
                 ?item a ledger:Item ; ledger:number ?number ; dcterms:title ?title ;\n          \
                 ledger:status ledger:open ; dcterms:created ?created .\n    \
                 OPTIONAL { ?item ledger:raised ?raised }\n  }\n}\n\
                 ORDER BY COALESCE(?raised, ?created)";
    let json: serde_json::Value = serde_json::from_str(&select(&kernel, query)).unwrap();
    let numbers: Vec<&str> = json["results"]["bindings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["number"]["value"].as_str().unwrap())
        .collect();
    assert_eq!(numbers, ["2", "1", "4", "3"], "{json}");
}
