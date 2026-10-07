//! **An unreadable value reported, and the audit's minor findings.** Each test pins a
//! defect an unled audit reproduced on 0.2.1 (`4e6f46e`), and failed there because of it.

mod common;

use common::*;
use ikigai_core::{Capability, Error, Verb};

const LIVE: &str = "urn:iki:ledger:graph:default";

/// An out-of-band write through the store's narrow door, as an editor would make one.
fn hand_edit(kernel: &ikigai_core::Kernel, update: &str) {
    sink(
        kernel,
        "urn:iki:store:graph-update",
        &[("graph", LIVE), ("content", update)],
    );
}

// --------------------------------------------------------------- an unreadable value

/// ★ A required value that is PRESENT but does not parse is reported, not dropped. Here an
/// editor writes `dcterms:created` as an `xsd:date` — a valid RDF date the reader cannot
/// use. On 0.2.1 the item vanished from `items` and `next`, and its own IRI answered "no
/// ledger item", while it sat in the graph. 0.3.0 reported it in a footer but still left it
/// out of the listing; since 0.4.2 (ledger #866) it is LISTED, with `dcterms:modified`
/// standing in, and the defect is said on every face — see `tests/malformed_timestamps.rs`.
#[test]
fn a_present_but_unreadable_timestamp_is_reported_not_dropped() {
    let kernel = kernel();
    let iri = append_iri(&kernel, "Edited by hand", &[]);
    hand_edit(
        &kernel,
        &format!(
            "DELETE {{ GRAPH <{LIVE}> {{ <{iri}> <http://purl.org/dc/terms/created> ?c }} }} \
             INSERT {{ GRAPH <{LIVE}> {{ <{iri}> <http://purl.org/dc/terms/created> \
             \"2026-09-15\"^^<http://www.w3.org/2001/XMLSchema#date> }} }} \
             WHERE {{ GRAPH <{LIVE}> {{ <{iri}> <http://purl.org/dc/terms/created> ?c }} }}"
        ),
    );
    let list = source(&kernel, "urn:iki:ledger:items", &[("status", "all")]);
    assert!(list.contains("Edited by hand"), "{list}");
    assert!(!list.contains("could not be read"), "{list}");
    assert!(list.contains(&iri), "{list}");
    assert!(
        list.contains("unreadable dcterms:created \"2026-09-15\""),
        "{list}"
    );
    let one = ok(&kernel, Verb::Source, &iri, &[]);
    assert!(one.contains("unreadable dcterms:created"), "{one}");
}

// --------------------------------------------------------------------------- Exists

/// `Exists` answers `false` only when there is no such item. On 0.2.1 every failure was
/// `false` — a caller the store refused, and a host with no store bound, were both told an
/// existing item did not exist.
#[test]
fn exists_is_false_only_for_a_missing_item() {
    let kernel = kernel();
    append(&kernel, "Present", &[]);
    assert_eq!(
        ok(&kernel, Verb::Exists, "urn:iki:ledger:item:1", &[]),
        "true\n"
    );
    assert_eq!(
        ok(&kernel, Verb::Exists, "urn:iki:ledger:item:99", &[]),
        "false\n"
    );

    // The ledger grant for `default`, and a store grant for ANOTHER graph: past the
    // kernel's family pre-check, refused by the store.
    let half = Capability::scoped([
        "urn:cap:ledger:read:default".to_string(),
        graph_read("other"),
    ]);
    let denied = try_as(&kernel, &half, Verb::Exists, "urn:iki:ledger:item:1", &[]);
    assert!(matches!(denied, Err(Error::Denied(_))), "{denied:?}");

    let no_store = kernel_without_store();
    let answer = try_verb(&no_store, Verb::Exists, "urn:iki:ledger:item:1", &[]);
    assert!(
        answer.is_err(),
        "a missing store is not a missing item: {answer:?}"
    );
}

// --------------------------------------------------------------- named ledgers' numbers

/// In a named ledger, `next` spells every number the way that ledger does. On 0.2.1 the
/// exclusion reasons and the nothing-ready summary printed bare `#1` — the DEFAULT
/// ledger's item.
#[test]
fn next_in_a_named_ledger_spells_its_own_numbers() {
    let kernel = kernel();
    sink(
        &kernel,
        "urn:iki:ledger:acme:append",
        &[("content", "Blocker")],
    );
    sink(
        &kernel,
        "urn:iki:ledger:acme:append",
        &[("content", "Blocked")],
    );
    sink(
        &kernel,
        "urn:iki:ledger:acme:link",
        &[
            ("item", "acme#1"),
            ("content", "acme#2"),
            ("type", "blocks"),
        ],
    );
    let next = source(&kernel, "urn:iki:ledger:acme:next", &[]);
    assert!(next.contains("blocked by acme#1"), "{next}");

    sink(
        &kernel,
        "urn:iki:ledger:acme:claim",
        &[("item", "acme#1"), ("content", "someone")],
    );
    let nothing = source(&kernel, "urn:iki:ledger:acme:next", &[]);
    assert!(nothing.contains("acme#1 claimed by someone"), "{nothing}");
    assert!(nothing.contains("acme#2 blocked by acme#1"), "{nothing}");
    let turtle = source(
        &kernel,
        "urn:iki:ledger:acme:next",
        &[("as", "text/turtle")],
    );
    assert!(turtle.contains("blocked by acme#1"), "{turtle}");
}

/// A claim refusal's remedy can be followed exactly as printed, in a named ledger too —
/// and it is a CONFLICT, which no retry changes, never a transient outage. On 0.2.1 the
/// remedy named the default ledger's claim resource (which refuses `acme#1` as another
/// ledger's item), and the refusal was `Unavailable`, which a circuit breaker counts.
#[test]
fn a_claim_refusal_is_a_conflict_whose_remedy_works_as_printed() {
    let kernel = kernel();
    sink(
        &kernel,
        "urn:iki:ledger:acme:append",
        &[("content", "Work")],
    );
    sink(
        &kernel,
        "urn:iki:ledger:acme:claim",
        &[("item", "acme#1"), ("content", "alice")],
    );
    let refused = try_verb(
        &kernel,
        Verb::Sink,
        "urn:iki:ledger:acme:claim",
        &[("item", "acme#1"), ("content", "bob")],
    )
    .expect_err("a held claim refuses a second holder");
    assert!(matches!(refused, Error::Conflict(_)), "{refused:?}");
    assert!(!refused.is_transient(), "{refused:?}");

    // `delete <resource> item=<n>`, followed verbatim.
    let message = refused.to_string();
    let remedy = message
        .split("`delete ")
        .nth(1)
        .and_then(|rest| rest.split('`').next())
        .expect("the refusal names a remedy");
    let (resource, item) = remedy.split_once(" item=").expect("resource and item");
    delete(&kernel, resource, &[("item", item)]);
    sink(
        &kernel,
        "urn:iki:ledger:acme:claim",
        &[("item", "acme#1"), ("content", "bob")],
    );
}

// ------------------------------------------------------------------------ tombstones

/// The number of a deleted item says it was deleted. On 0.2.1 `#3` resolved to the
/// tombstone (which carries the number too), and the answer was "no ledger item at
/// `…:tombstone:…`".
#[test]
fn the_number_of_a_deleted_item_says_it_was_deleted() {
    let kernel = kernel();
    append(&kernel, "one", &[]);
    append(&kernel, "two", &[]);
    delete(&kernel, "urn:iki:ledger:item:2", &[]);
    let answer = try_verb(&kernel, Verb::Source, "urn:iki:ledger:item:2", &[]);
    let Err(Error::NotFound(message)) = &answer else {
        panic!("{answer:?}");
    };
    assert!(message.contains("#2 was deleted"), "{message}");
    assert!(!message.contains("no ledger item at"), "{message}");
    assert_eq!(
        ok(&kernel, Verb::Exists, "urn:iki:ledger:item:2", &[]),
        "false\n"
    );
}

/// No property a tombstone carries is declared, by `rdfs:domain`, to belong to an item —
/// or a reasoning consumer counts every deleted item as a live one. On 0.2.1
/// `ledger:number` had `rdfs:domain ledger:Item`.
#[test]
fn a_tombstone_is_never_inferred_to_be_an_item() {
    use std::collections::BTreeMap;
    let kernel = kernel();
    append(&kernel, "Doomed", &[]);
    delete(&kernel, "urn:iki:ledger:item:1", &[]);
    let mut domain: BTreeMap<String, String> = BTreeMap::new();
    for quad in oxrdfio::RdfParser::from_format(oxrdfio::RdfFormat::Turtle)
        .for_reader(ikigai_ledger::VOCABULARY_TTL.as_bytes())
    {
        let quad = quad.expect("the vocabulary parses");
        if quad.predicate.as_str() == "http://www.w3.org/2000/01/rdf-schema#domain" {
            // Through the text form, which reads the same across oxrdf's rename of `Subject`.
            let subject = quad.subject.to_string();
            if let Some(iri) = subject.strip_prefix('<').and_then(|s| s.strip_suffix('>')) {
                domain.insert(iri.to_string(), quad.object.to_string());
            }
        }
    }
    let rows = select(
        &kernel,
        &format!(
            "SELECT ?p WHERE {{ GRAPH <{LIVE}> {{ \
             ?t a <https://ikigai-rs.dev/ns/ledger#Tombstone> ; ?p ?o }} }}"
        ),
    );
    let json: serde_json::Value = serde_json::from_str(&rows).unwrap();
    let offending: Vec<String> = json["results"]["bindings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["p"]["value"].as_str().unwrap().to_string())
        .filter(|p| {
            domain.get(p).map(String::as_str) == Some("<https://ikigai-rs.dev/ns/ledger#Item>")
        })
        .collect();
    assert!(offending.is_empty(), "{offending:?}");
}

// ----------------------------------------------------------------------- the inventory

/// A structural class is not a level of item, and the inventory counts each item once
/// however many counter subjects a graph holds. On 0.2.1 `append kind=ledger:Counter` was
/// accepted and doubled every count in `urn:iki:ledger:ledgers`.
#[test]
fn the_inventory_counts_each_item_once() {
    let kernel = kernel();
    append(&kernel, "one", &[]);
    append(&kernel, "two", &[]);
    let refused = try_verb(
        &kernel,
        Verb::Sink,
        "urn:iki:ledger:append",
        &[
            ("content", "three"),
            ("kind", "https://ikigai-rs.dev/ns/ledger#Counter"),
        ],
    );
    assert!(
        matches!(refused, Err(Error::InvalidArgument { .. })),
        "{refused:?}"
    );
    // A second counter written by hand — the other way to get two.
    hand_edit(
        &kernel,
        &format!(
            "INSERT DATA {{ GRAPH <{LIVE}> {{ <urn:x:second-counter> a \
             <https://ikigai-rs.dev/ns/ledger#Counter> }} }}"
        ),
    );
    sink(&kernel, "urn:iki:ledger:close", &[("item", "#2")]);
    let inventory = source(&kernel, "urn:iki:ledger:ledgers", &[]);
    assert!(
        inventory.contains("    1 open      2 total"),
        "two items, one open:\n{inventory}"
    );
}

// --------------------------------------------------------------------- refused, not read

/// A value outside a declared set is refused, never substituted. On 0.2.1 `deferred=exlude`
/// was read as `include` — answering with exactly the items the caller asked to leave out
/// — and `limit=ten` as the default.
#[test]
fn an_unknown_deferred_or_limit_is_refused() {
    let kernel = kernel();
    append(&kernel, "Put off", &[]);
    sink(&kernel, "urn:iki:ledger:defer", &[("item", "1")]);
    for (resource, arg, value) in [
        ("urn:iki:ledger:items", "deferred", "exlude"),
        ("urn:iki:ledger:items", "limit", "ten"),
        ("urn:iki:ledger:next", "limit", "ten"),
    ] {
        let answer = try_verb(&kernel, Verb::Source, resource, &[(arg, value)]);
        assert!(
            matches!(&answer, Err(Error::InvalidArgument { name, .. }) if name == arg),
            "{resource} {arg}={value}: {answer:?}"
        );
    }
    // And the real values still work.
    let excluded = source(&kernel, "urn:iki:ledger:items", &[("deferred", "exclude")]);
    assert!(!excluded.contains("Put off"), "{excluded}");
}

/// The remedy `next` names for a self-block can be followed. A self-block arrives only out
/// of band (a link refuses one), `next` then refuses naming `delete …:link … type=blocks`,
/// and on 0.2.1 that Delete was refused too, for the reason the Sink is.
#[test]
fn the_remedy_for_a_self_block_is_not_refused() {
    let kernel = kernel();
    let iri = append_iri(&kernel, "Blocks itself", &[]);
    hand_edit(
        &kernel,
        &format!(
            "INSERT DATA {{ GRAPH <{LIVE}> {{ <{iri}> \
             <https://ikigai-rs.dev/ns/ledger#blocks> <{iri}> }} }}"
        ),
    );
    let refused = try_verb(&kernel, Verb::Source, "urn:iki:ledger:next", &[]);
    assert!(format!("{refused:?}").contains("cycle"), "{refused:?}");
    delete(
        &kernel,
        "urn:iki:ledger:link",
        &[("item", "#1"), ("content", "#1"), ("type", "blocks")],
    );
    assert!(source(&kernel, "urn:iki:ledger:next", &[]).contains("Blocks itself"));
    // Adding one through the Sink is still refused.
    let again = try_verb(
        &kernel,
        Verb::Sink,
        "urn:iki:ledger:link",
        &[("item", "#1"), ("content", "#1"), ("type", "blocks")],
    );
    assert!(
        matches!(again, Err(Error::InvalidArgument { .. })),
        "{again:?}"
    );
}

/// A label the filters could never name is refused where it would be added — and one
/// already there can still be removed. On 0.2.1 `label content=a,b` was stored whole and
/// `labels=a,b` split it, so it matched nothing, ever.
#[test]
fn a_label_with_a_comma_is_refused_and_an_old_one_can_be_removed() {
    let kernel = kernel();
    let iri = append_iri(&kernel, "Tagged", &[]);
    let refused = try_verb(
        &kernel,
        Verb::Sink,
        "urn:iki:ledger:label",
        &[("item", "#1"), ("content", "area,core")],
    );
    assert!(
        matches!(refused, Err(Error::InvalidArgument { .. })),
        "{refused:?}"
    );
    hand_edit(
        &kernel,
        &format!(
            "INSERT DATA {{ GRAPH <{LIVE}> {{ <{iri}> \
             <https://ikigai-rs.dev/ns/ledger#label> \"area,core\" }} }}"
        ),
    );
    delete(
        &kernel,
        "urn:iki:ledger:label",
        &[("item", "#1"), ("content", "area,core")],
    );
    assert!(!source(&kernel, "urn:iki:ledger:item:1", &[]).contains("area,core"));
}

/// The `holder=` filter's keywords cannot be holders. On 0.2.1 a holder called `none` held
/// work that `holder=none` could not find — it listed the unclaimed items instead.
#[test]
fn a_holder_cannot_be_named_after_a_filter_keyword() {
    let kernel = kernel();
    append(&kernel, "Work", &[]);
    for reserved in ["none", "any"] {
        let refused = try_verb(
            &kernel,
            Verb::Sink,
            "urn:iki:ledger:claim",
            &[("item", "#1"), ("content", reserved)],
        );
        assert!(
            matches!(refused, Err(Error::InvalidArgument { .. })),
            "{reserved}: {refused:?}"
        );
    }
}

/// `urn:iki:ledger:item:1` means #1 as an argument too, because it RESOLVES as a resource.
/// On 0.2.1 the same IRI that just answered as a resource was NotFound as `item=`.
#[test]
fn an_item_iri_with_a_number_resolves_as_an_argument_too() {
    let kernel = kernel();
    append(&kernel, "An item", &[]);
    assert!(source(&kernel, "urn:iki:ledger:item:1", &[]).contains("An item"));
    sink(
        &kernel,
        "urn:iki:ledger:comment",
        &[("item", "urn:iki:ledger:item:1"), ("content", "hello")],
    );
    assert!(source(&kernel, "urn:iki:ledger:item:1", &[]).contains("hello"));
}

/// A hand-written priority the Sink would refuse is scored, not a panic. On 0.2.1
/// `ledger:priority -9223372036854775808` overflowed `(5 - p) * 4` under `leverage`.
#[test]
fn an_extreme_hand_written_priority_does_not_panic_next() {
    let kernel = kernel();
    let iri = append_iri(&kernel, "Hand-edited", &[]);
    hand_edit(
        &kernel,
        &format!(
            "INSERT DATA {{ GRAPH <{LIVE}> {{ <{iri}> \
             <https://ikigai-rs.dev/ns/ledger#priority> -9223372036854775808 }} }}"
        ),
    );
    let answered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        try_verb(
            &kernel,
            Verb::Source,
            "urn:iki:ledger:next",
            &[("policy", "leverage")],
        )
    }));
    assert!(
        matches!(answered, Ok(Ok(_))),
        "next policy=leverage over a hand-written priority: {answered:?}"
    );
}
