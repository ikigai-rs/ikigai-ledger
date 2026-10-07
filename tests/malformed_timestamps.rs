//! **One malformed timestamp never drops an item** (ledger #866).
//!
//! Reproduced on 0.4.1 (`e85273f`): a `dcterms:modified "yesterday"` written through
//! `urn:iki:store:graph-update` took the item out of `items` (reported in a footer as "could
//! not be read and are not listed"), out of the Turtle face of `items` with no word at all,
//! and out of `next` with no word at all — and with it every `blocks` edge it carried, so
//! the item it blocked was offered as READY. `item:{id}` refused to read it. Each test below
//! failed on 0.4.1 for that reason.
//!
//! The rule now: an unreadable or missing timestamp is treated as absent and the other one
//! stands in for it (created ≤ modified, so neither is invented), and every face says so.
//! Only an item with no readable timestamp at all, or no readable number, is still dropped,
//! and that is still reported.

mod common;

use common::*;
use ikigai_core::{Error, Verb};

const LIVE: &str = "urn:iki:ledger:graph:default";
const MODIFIED: &str = "http://purl.org/dc/terms/modified";
const CREATED: &str = "http://purl.org/dc/terms/created";
const XSD_DATE_TIME: &str = "http://www.w3.org/2001/XMLSchema#dateTime";

/// An out-of-band write through the store's narrow door, as audit round 4 made one.
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

/// Add a value beside the ones already there.
fn add(kernel: &ikigai_core::Kernel, iri: &str, predicate: &str, object: &str) {
    hand_edit(
        kernel,
        &format!("INSERT DATA {{ GRAPH <{LIVE}> {{ <{iri}> <{predicate}> {object} }} }}"),
    );
}

fn json(kernel: &ikigai_core::Kernel, iri: &str, args: &[(&str, &str)]) -> serde_json::Value {
    let mut all = vec![("as", "application/json")];
    all.extend_from_slice(args);
    serde_json::from_str(&source(kernel, iri, &all)).expect("the JSON face parses")
}

/// The claim exactly: a plain-string `dcterms:modified`. Listed, on every face, and said.
#[test]
fn an_unreadable_modified_leaves_the_item_listed_and_says_so() {
    let kernel = kernel();
    append(&kernel, "Filed properly", &[]);
    let iri = append_iri(&kernel, "Edited by hand", &[]);
    replace(&kernel, &iri, MODIFIED, Some("\"yesterday\""));

    let list = source(&kernel, "urn:iki:ledger:items", &[]);
    assert!(list.contains("Edited by hand"), "{list}");
    assert!(list.contains("2 item(s)"), "{list}");
    assert!(!list.contains("could not be read"), "{list}");
    assert!(
        list.contains("carry a timestamp nothing here can read"),
        "{list}"
    );
    assert!(
        list.contains("#2 ") && list.contains("unreadable dcterms:modified \"yesterday\""),
        "{list}"
    );

    // The JSON face: still schema 1, the item present, its stand-in a dateTime, and the
    // defect on the item itself rather than in `unreadable` (which means "not in items").
    let doc = json(&kernel, "urn:iki:ledger:items", &[]);
    assert_eq!(doc["schema"], 1);
    assert_eq!(doc["count"], 2, "{doc}");
    assert_eq!(doc["unreadable"], serde_json::json!([]), "{doc}");
    let edited = doc["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["number"] == 2)
        .unwrap_or_else(|| panic!("#2 is listed: {doc}"));
    assert_eq!(
        edited["defects"],
        serde_json::json!(["unreadable dcterms:modified \"yesterday\""])
    );
    assert_eq!(edited["modified"], edited["created"], "{edited}");
    assert!(edited["modified"].as_str().unwrap().ends_with('Z'));
    // ...and it still deserializes into the published type.
    let typed: ikigai_ledger::json::ItemsDocument =
        serde_json::from_value(doc.clone()).expect("schema 1");
    assert_eq!(typed.items.len(), 2);

    // The Turtle face carries the value AS STORED and never asserts the stand-in.
    let turtle = source(&kernel, "urn:iki:ledger:items", &[("as", "text/turtle")]);
    assert!(turtle.contains(&iri), "{turtle}");
    assert!(turtle.contains("\"yesterday\""), "{turtle}");
    let modified_lines = turtle
        .split(&format!("<{iri}>"))
        .nth(1)
        .unwrap()
        .split(" .\n")
        .next()
        .unwrap()
        .matches("dcterms:modified")
        .count();
    assert_eq!(modified_lines, 1, "only the stored value: {turtle}");

    // `next` offers it.
    let next = source(&kernel, "urn:iki:ledger:next", &[("limit", "10")]);
    assert!(next.contains("Edited by hand"), "{next}");

    // Its own IRI reads, and says which line is a stand-in.
    let one = source(&kernel, &iri, &[]);
    assert!(
        one.contains("⚠ defect: unreadable dcterms:modified \"yesterday\""),
        "{one}"
    );
    assert!(one.contains("`updated` above is dcterms:created"), "{one}");
    let one = json(&kernel, &iri, &[]);
    assert_eq!(
        one["item"]["defects"],
        serde_json::json!(["unreadable dcterms:modified \"yesterday\""])
    );
    assert_eq!(
        ok(&kernel, Verb::Exists, "urn:iki:ledger:item:2", &[]),
        "true\n"
    );
}

/// The same for a value typed `xsd:dateTime` whose lexical form is not one, and for
/// `dcterms:created`, where `modified` stands in.
#[test]
fn an_ill_typed_datetime_and_an_unreadable_created_are_read_around_too() {
    let kernel = kernel();
    let iri = append_iri(&kernel, "Ill typed", &[]);
    replace(
        &kernel,
        &iri,
        MODIFIED,
        Some(&format!("\"garbage\"^^<{XSD_DATE_TIME}>")),
    );
    let doc = json(&kernel, "urn:iki:ledger:items", &[]);
    assert_eq!(doc["count"], 1, "{doc}");
    assert_eq!(
        doc["items"][0]["defects"],
        serde_json::json!(["unreadable dcterms:modified \"garbage\""])
    );

    let kernel = self::kernel();
    let iri = append_iri(&kernel, "Created by hand", &[]);
    replace(&kernel, &iri, CREATED, Some("\"yesterday\""));
    let doc = json(&kernel, "urn:iki:ledger:items", &[]);
    assert_eq!(doc["count"], 1, "{doc}");
    let item = &doc["items"][0];
    assert_eq!(
        item["defects"],
        serde_json::json!(["unreadable dcterms:created \"yesterday\""])
    );
    assert_eq!(item["created"], item["modified"]);
    let one = source(&kernel, &iri, &[]);
    assert!(one.contains("`filed` above is dcterms:modified"), "{one}");
}

/// ★ The consequence that made the silent drop worse than a missing row: an item that
/// vanished from `next`'s pool took its `blocks` edges with it, so what it blocked was
/// offered as ready.
#[test]
fn a_blocker_with_an_unreadable_timestamp_still_blocks() {
    let kernel = kernel();
    let blocker = append_iri(&kernel, "The blocker", &[]);
    append(&kernel, "The blocked", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:link",
        &[("item", "#1"), ("content", "#2"), ("type", "blocks")],
    );
    replace(&kernel, &blocker, MODIFIED, Some("\"yesterday\""));
    let next = source(&kernel, "urn:iki:ledger:next", &[("limit", "10")]);
    assert!(next.contains("The blocker"), "{next}");
    assert!(next.contains("not #2: blocked by #1"), "{next}");
}

/// A MISSING timestamp is the same fact as an unreadable one: absent.
#[test]
fn a_missing_modified_is_read_as_absent() {
    let kernel = kernel();
    let iri = append_iri(&kernel, "Lost its stamp", &[]);
    replace(&kernel, &iri, MODIFIED, None);
    let doc = json(&kernel, "urn:iki:ledger:items", &[]);
    assert_eq!(doc["count"], 1, "{doc}");
    assert_eq!(
        doc["items"][0]["defects"],
        serde_json::json!(["missing dcterms:modified"])
    );
}

/// With NO readable timestamp there is no true time to show, so the item is still not
/// listed — and still reported, with every defect, and its own IRI still says why.
#[test]
fn an_item_with_no_readable_timestamp_is_still_reported_not_listed() {
    let kernel = kernel();
    let iri = append_iri(&kernel, "Both gone", &[]);
    replace(&kernel, &iri, MODIFIED, Some("\"yesterday\""));
    replace(&kernel, &iri, CREATED, None);
    let doc = json(&kernel, "urn:iki:ledger:items", &[]);
    assert_eq!(doc["count"], 0, "{doc}");
    assert_eq!(
        doc["unreadable"],
        serde_json::json!([{"iri": iri, "defects": [
            "missing dcterms:created",
            "unreadable dcterms:modified \"yesterday\""
        ]}])
    );
    let list = source(&kernel, "urn:iki:ledger:items", &[]);
    assert!(list.contains("could not be read"), "{list}");
    let one = try_verb(&kernel, Verb::Source, &iri, &[]);
    let Err(Error::Endpoint(message)) = &one else {
        panic!("reading it directly must say what is wrong: {one:?}");
    };
    assert!(message.contains("missing dcterms:created"), "{message}");
}

/// The number is the item's name on every face, and nothing can stand in for it, so an
/// unreadable number still drops the item — reported, as before.
#[test]
fn an_unreadable_number_is_still_reported_not_listed() {
    let kernel = kernel();
    let iri = append_iri(&kernel, "Renumbered by hand", &[]);
    replace(
        &kernel,
        &iri,
        "https://ikigai-rs.dev/ns/ledger#number",
        Some("\"twelve\""),
    );
    let doc = json(&kernel, "urn:iki:ledger:items", &[]);
    assert_eq!(doc["count"], 0, "{doc}");
    assert_eq!(
        doc["unreadable"][0]["defects"],
        serde_json::json!(["unreadable ledger:number \"twelve\""])
    );
}

/// Two values of one timestamp are two rows, and were two LISTINGS of one item. Now the
/// rows of one subject are one item, with the latest readable `modified`; a bad value
/// beside a good one is named, not lost.
#[test]
fn a_second_modified_value_lists_the_item_once() {
    let kernel = kernel();
    let iri = append_iri(&kernel, "Doubly stamped", &[]);
    add(
        &kernel,
        &iri,
        MODIFIED,
        &format!("\"2030-01-01T00:00:00.000Z\"^^<{XSD_DATE_TIME}>"),
    );
    let doc = json(&kernel, "urn:iki:ledger:items", &[]);
    assert_eq!(doc["count"], 1, "{doc}");
    assert_eq!(doc["items"][0]["modified"], "2030-01-01T00:00:00.000Z");
    assert_eq!(doc["items"][0]["defects"], serde_json::json!([]));

    add(&kernel, &iri, MODIFIED, "\"yesterday\"");
    let doc = json(&kernel, "urn:iki:ledger:items", &[]);
    assert_eq!(doc["count"], 1, "{doc}");
    assert_eq!(doc["items"][0]["modified"], "2030-01-01T00:00:00.000Z");
    assert_eq!(
        doc["items"][0]["defects"],
        serde_json::json!(["unreadable dcterms:modified \"yesterday\""])
    );
    let one = json(&kernel, &iri, &[]);
    assert_eq!(one["item"]["modified"], "2030-01-01T00:00:00.000Z");
}

/// Any write through the ledger re-stamps `dcterms:modified` — every value of it — so the
/// ledger's own write path repairs this defect rather than preserving it.
#[test]
fn a_ledger_write_repairs_an_unreadable_modified() {
    let kernel = kernel();
    let iri = append_iri(&kernel, "Repair me", &[]);
    replace(&kernel, &iri, MODIFIED, Some("\"yesterday\""));
    sink(&kernel, "urn:iki:ledger:item:1", &[("priority", "2")]);
    let doc = json(&kernel, "urn:iki:ledger:items", &[]);
    assert_eq!(doc["items"][0]["defects"], serde_json::json!([]), "{doc}");
    assert!(!source(&kernel, &iri, &[]).contains("defect"));
}

/// The same bad value on both timestamps is two defects, not one: a value is only the same
/// value when it is on the same property.
#[test]
fn one_bad_value_on_both_timestamps_is_named_twice() {
    let kernel = kernel();
    let iri = append_iri(&kernel, "Twice yesterday", &[]);
    replace(&kernel, &iri, CREATED, Some("\"yesterday\""));
    add(&kernel, &iri, MODIFIED, "\"yesterday\"");
    let doc = json(&kernel, "urn:iki:ledger:items", &[]);
    assert_eq!(doc["count"], 1, "{doc}");
    assert_eq!(
        doc["items"][0]["defects"],
        serde_json::json!([
            "unreadable dcterms:created \"yesterday\"",
            "unreadable dcterms:modified \"yesterday\""
        ])
    );
}
