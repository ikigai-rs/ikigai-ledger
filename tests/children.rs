//! **`parent=` and an item's children** (ledger #1454).
//!
//! Through 0.11.1 an item showed only its OUTBOUND `parent:` edge, so a hub could not list a
//! goal's arcs without SPARQL. Now `items` and `count` take `parent=` (a number, `#N` or an
//! item IRI, resolved as `item=` is), and every face of an item names its children: the plain
//! face as `child:` lines, the JSON face as `children`, and the Turtle face as the
//! `<child> ledger:parent <item>` triples themselves.

mod common;

use common::*;
use ikigai_core::{Error, Kernel, Verb};

const PARENT: &str = "https://ikigai-rs.dev/ns/ledger#parent";

fn json(kernel: &Kernel, iri: &str, args: &[(&str, &str)]) -> serde_json::Value {
    let mut all = vec![("as", "application/json")];
    all.extend_from_slice(args);
    serde_json::from_str(&source(kernel, iri, &all)).expect("the JSON face parses")
}

fn part_of(kernel: &Kernel, child: &str, parent: &str) {
    sink(
        kernel,
        "urn:iki:ledger:link",
        &[("item", child), ("content", parent), ("type", "parent")],
    );
}

/// #1 a goal with three arcs: #2 open, #3 closed, #5 open and also blocked by #4 (an edge that
/// is not `parent`, so not a child). #4 is unrelated.
fn ledger() -> (Kernel, Vec<String>) {
    let kernel = kernel();
    let iris: Vec<String> = ["Goal", "Arc one", "Arc two", "Unrelated", "Arc three"]
        .iter()
        .map(|title| append_iri(&kernel, title, &[]))
        .collect();
    part_of(&kernel, "#2", "#1");
    part_of(&kernel, "#3", "#1");
    part_of(&kernel, "#5", "#1");
    sink(
        &kernel,
        "urn:iki:ledger:link",
        &[("item", "#4"), ("content", "#5"), ("type", "blocks")],
    );
    sink(&kernel, "urn:iki:ledger:close", &[("item", "#3")]);
    (kernel, iris)
}

fn numbers(doc: &serde_json::Value) -> Vec<i64> {
    let mut found: Vec<i64> = doc["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|row| row["number"].as_i64().expect("a number"))
        .collect();
    found.sort_unstable();
    found
}

#[test]
fn items_and_count_list_the_children_of_an_item() {
    let (kernel, iris) = ledger();
    // Every spelling of the parent names the same item.
    for parent in ["1", "#1", iris[0].as_str()] {
        let open = json(&kernel, "urn:iki:ledger:items", &[("parent", parent)]);
        assert_eq!(numbers(&open), vec![2, 5], "parent={parent}: {open}");
        assert_eq!(open["total"], 2);
        let all = json(
            &kernel,
            "urn:iki:ledger:items",
            &[("parent", parent), ("status", "all")],
        );
        assert_eq!(numbers(&all), vec![2, 3, 5], "parent={parent}: {all}");
        assert_eq!(
            source(&kernel, "urn:iki:ledger:count", &[("parent", parent)]),
            "2\n"
        );
        assert_eq!(
            json(
                &kernel,
                "urn:iki:ledger:count",
                &[("parent", parent), ("status", "closed")]
            )["total"],
            1
        );
    }
    // It composes with the other filters, and an item with no children has none.
    let plain = source(
        &kernel,
        "urn:iki:ledger:items",
        &[("parent", "1"), ("text", "three")],
    );
    assert!(plain.contains("Arc three"), "{plain}");
    assert!(!plain.contains("Arc one"), "{plain}");
    assert_eq!(
        source(&kernel, "urn:iki:ledger:count", &[("parent", "4")]),
        "0\n"
    );
}

/// An unknown parent is not found, never an empty answer: "no children" and "no such item"
/// are different facts. An item of another ledger is refused by name, as `item=` is.
#[test]
fn an_unknown_or_foreign_parent_is_refused() {
    let (kernel, _) = ledger();
    for iri in ["urn:iki:ledger:items", "urn:iki:ledger:count"] {
        let missing = try_verb(&kernel, Verb::Source, iri, &[("parent", "99")]);
        assert!(matches!(missing, Err(Error::NotFound(_))), "{missing:?}");
        let foreign = try_verb(
            &kernel,
            Verb::Source,
            iri,
            &[("parent", "urn:iki:ledger:acme:item:01k5x")],
        );
        assert!(
            matches!(&foreign, Err(Error::InvalidArgument { name, .. }) if name == "parent"),
            "{foreign:?}"
        );
    }
}

#[test]
fn every_face_of_an_item_names_its_children() {
    let (kernel, iris) = ledger();
    let children = [&iris[1], &iris[2], &iris[4]];

    let plain = source(&kernel, "urn:iki:ledger:item:1", &[]);
    for child in children {
        assert!(plain.contains(&format!("  child:    {child}\n")), "{plain}");
    }
    assert!(!plain.contains(&iris[3]), "{plain}");

    let doc = json(&kernel, "urn:iki:ledger:item:1", &[]);
    let mut named: Vec<i64> = doc["item"]["children"]
        .as_array()
        .expect("children")
        .iter()
        .map(|c| c["number"].as_i64().expect("a number"))
        .collect();
    named.sort_unstable();
    assert_eq!(named, vec![2, 3, 5], "{doc}");
    assert_eq!(
        doc["item"]["children"][0]["display"]
            .as_str()
            .map(|d| d.starts_with('#')),
        Some(true)
    );

    let turtle = source(&kernel, "urn:iki:ledger:item:1", &[("as", "text/turtle")]);
    let mut edges = parent_edges(&turtle);
    edges.sort();
    let mut want: Vec<(String, String)> = children
        .iter()
        .map(|child| (child.to_string(), iris[0].clone()))
        .collect();
    want.sort();
    assert_eq!(edges, want, "{turtle}");

    // A child names its parent and has no children of its own; an item with neither edge
    // renders as it always has.
    let child = json(&kernel, "urn:iki:ledger:item:2", &[]);
    assert_eq!(child["item"]["children"], serde_json::json!([]), "{child}");
    assert_eq!(child["item"]["links"][0]["type"], "parent", "{child}");
    let plain = source(&kernel, "urn:iki:ledger:item:4", &[]);
    assert!(!plain.contains("child:"), "{plain}");
}

/// The listing faces carry the children too — the same `Item` object wherever it appears —
/// and the Turtle face's graph says exactly the triples the store holds, once each.
#[test]
fn a_listing_carries_the_children_and_its_graph_is_the_stored_edges() {
    let (kernel, iris) = ledger();
    let doc = json(&kernel, "urn:iki:ledger:items", &[("status", "all")]);
    let goal = doc["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["number"] == 1)
        .expect("the goal is listed");
    assert_eq!(goal["children"].as_array().unwrap().len(), 3, "{goal}");

    // In the Turtle face the children are triples about the CHILD, and the graph is a set:
    // the listing holds each `parent` edge once although both of its ends are listed.
    let turtle = source(
        &kernel,
        "urn:iki:ledger:items",
        &[("status", "all"), ("as", "text/turtle")],
    );
    let edges = parent_edges(&turtle);
    assert_eq!(edges.len(), 3, "{turtle}");
    for (_, parent) in edges {
        assert_eq!(parent, iris[0]);
    }
}

/// The `ledger:parent` triples of a Turtle face, as (child, parent) IRIs.
fn parent_edges(turtle: &str) -> Vec<(String, String)> {
    oxrdfio::RdfParser::from_format(oxrdfio::RdfFormat::Turtle)
        .for_reader(turtle.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .expect("the Turtle face parses")
        .into_iter()
        .filter(|t| t.predicate.as_str() == PARENT)
        .map(|t| {
            let subject = match t.subject {
                oxrdf::NamedOrBlankNode::NamedNode(n) => n.into_string(),
                other => panic!("a blank subject: {other}"),
            };
            let object = match t.object {
                oxrdf::Term::NamedNode(n) => n.into_string(),
                other => panic!("not an IRI: {other}"),
            };
            (subject, object)
        })
        .collect()
}
