//! **A reused key with different content says so** (ledger #4058; the design is §5.4 of
//! ikigai-core's `docs/research/sink-idempotency.md`).
//!
//! A keyed append whose key is taken files nothing and answers `existing <status>`. Before
//! this, it answered that whether the second filing said the same thing or something else
//! entirely, so a fan-out reusing fixed keys across waves could not tell that it had landed
//! on last wave's closed item. The append now stores a fingerprint of what it filed (the
//! title, the body and the labels it set) beside the key. A taken key whose fingerprint
//! matches answers `existing <status>` as before; one whose fingerprint differs answers
//! `existing <status> differs`, `"differs": true` in JSON. It is not refused: a re-sync that
//! legitimately edits a title upstream must keep working.

mod common;

use common::*;

const KEY: &str = "urn:kata:issue:01JZ";
const LIVE: &str = "urn:iki:ledger:graph:default";

fn keyed(kernel: &ikigai_core::Kernel, content: &str, labels: Option<&str>) -> String {
    let mut args = vec![("content", content), ("key", KEY)];
    if let Some(labels) = labels {
        args.push(("labels", labels));
    }
    sink(kernel, "urn:iki:ledger:append", &args)
}

fn keyed_json(
    kernel: &ikigai_core::Kernel,
    content: &str,
    labels: Option<&str>,
) -> serde_json::Value {
    let mut args = vec![
        ("content", content),
        ("key", KEY),
        ("as", "application/json"),
    ];
    if let Some(labels) = labels {
        args.push(("labels", labels));
    }
    serde_json::from_str(&sink(kernel, "urn:iki:ledger:append", &args)).expect("JSON")
}

fn iri_of(answer: &str) -> String {
    answer
        .split_whitespace()
        .nth(1)
        .expect("an IRI")
        .to_string()
}

const FIRST: &str = "Port the importer\n\nIt still reads the old format.";

#[test]
fn the_same_filing_again_answers_existing_with_no_flag() {
    let kernel = kernel();
    let iri = iri_of(&keyed(&kernel, FIRST, Some("bug,import")));
    assert_eq!(
        keyed(&kernel, FIRST, Some("bug,import")),
        format!("#1 {iri} existing open\n")
    );
    // Labels are a set: their order and repeats are not a difference.
    assert_eq!(
        keyed(&kernel, FIRST, Some("import, bug,bug")),
        format!("#1 {iri} existing open\n")
    );
    // Neither is whitespace the append trims before it files.
    assert_eq!(
        keyed(&kernel, &format!("  {FIRST}\n\n"), Some("bug,import")),
        format!("#1 {iri} existing open\n")
    );
}

#[test]
fn a_different_title_body_or_labels_each_answer_differs() {
    let kernel = kernel();
    let iri = iri_of(&keyed(&kernel, FIRST, Some("bug,import")));
    let differs = format!("#1 {iri} existing open differs\n");
    for (what, content, labels) in [
        (
            "title",
            "Port the importer (renamed)\n\nIt still reads the old format.",
            Some("bug,import"),
        ),
        (
            "body",
            "Port the importer\n\nIt reads the new format now.",
            Some("bug,import"),
        ),
        ("no body", "Port the importer", Some("bug,import")),
        ("labels", FIRST, Some("bug")),
        ("no labels", FIRST, None),
        ("an added label", FIRST, Some("bug,import,p1")),
    ] {
        assert_eq!(
            keyed(&kernel, content, labels),
            differs,
            "a different {what}"
        );
    }
    // Nothing was filed or changed by any of them.
    let detail = source(&kernel, "urn:iki:ledger:item:1", &[]);
    assert!(detail.contains("Port the importer"), "{detail}");
    assert!(!detail.contains("renamed"), "{detail}");
    assert_eq!(append(&kernel, "Unkeyed", &[]), "#2");
}

/// ★ The case §5.4 exists for: a fan-out reusing a fixed key lands on last wave's closed item.
#[test]
fn a_closed_or_deleted_item_says_differs_too() {
    let kernel = kernel();
    let iri = iri_of(&keyed(&kernel, FIRST, None));
    sink(&kernel, "urn:iki:ledger:close", &[("item", "#1")]);
    assert_eq!(
        keyed(&kernel, "Next wave's task", None),
        format!("#1 {iri} existing closed differs\n")
    );
    assert_eq!(
        keyed(&kernel, FIRST, None),
        format!("#1 {iri} existing closed\n")
    );
    // A delete moves the fingerprint onto the tombstone with the key.
    delete(&kernel, &format!("urn:iki:ledger:item:key:{KEY}"), &[]);
    assert_eq!(
        keyed(&kernel, "Next wave's task", None),
        format!("#1 {iri} existing deleted differs\n")
    );
    assert_eq!(
        keyed(&kernel, FIRST, None),
        format!("#1 {iri} existing deleted\n")
    );
    // And a purge keeps it, as it keeps the key.
    delete(
        &kernel,
        "urn:iki:ledger:purge",
        &[("content", &format!("key:{KEY}"))],
    );
    assert_eq!(
        keyed(&kernel, "Next wave's task", None),
        format!("#1 {iri} existing deleted differs\n")
    );
    assert_eq!(
        keyed(&kernel, FIRST, None),
        format!("#1 {iri} existing deleted\n")
    );
}

#[test]
fn the_json_face_carries_differs_true_only_on_a_mismatch() {
    let kernel = kernel();
    let first = keyed_json(&kernel, FIRST, Some("bug"));
    assert_eq!(first["outcome"], "filed");
    assert!(first.get("differs").is_none(), "{first}");
    let same = keyed_json(&kernel, FIRST, Some("bug"));
    assert_eq!(same["outcome"], "existing");
    assert!(same.get("differs").is_none(), "{same}");
    let other = keyed_json(&kernel, "Something else", Some("bug"));
    assert_eq!(other["outcome"], "existing");
    assert_eq!(other["status"], "open");
    assert_eq!(other["differs"], true);
    assert_eq!(other["item"], first["item"]);
}

/// An item filed before ledger #4058 has a key and no fingerprint. What it was filed with is not
/// known, and its title and body today may be edits made in the ledger, so it is not
/// compared: it answers `existing <status>` as it always did, whatever the content.
#[test]
fn a_key_filed_before_the_fingerprint_answers_existing_with_no_flag() {
    let kernel = kernel();
    let iri = iri_of(&keyed(&kernel, FIRST, None));
    sink(
        &kernel,
        "urn:iki:store:graph-update",
        &[
            ("graph", LIVE),
            (
                "content",
                "WITH <urn:iki:ledger:graph:default> DELETE { ?s <https://ikigai-rs.dev/ns/ledger#keyFingerprint> ?f } WHERE { ?s <https://ikigai-rs.dev/ns/ledger#keyFingerprint> ?f }",
            ),
        ],
    );
    assert_eq!(
        keyed(&kernel, "Anything at all", None),
        format!("#1 {iri} existing open\n")
    );
    let json = keyed_json(&kernel, "Anything at all", None);
    assert_eq!(json["outcome"], "existing");
    assert!(json.get("differs").is_none(), "{json}");
    // And after a delete, with no fingerprint to move, the same.
    delete(&kernel, &format!("urn:iki:ledger:item:key:{KEY}"), &[]);
    assert_eq!(
        keyed(&kernel, "Anything at all", None),
        format!("#1 {iri} existing deleted\n")
    );
}
