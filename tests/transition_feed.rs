//! Ledger #4116: the transition feed, the ledger's outbox (ikigai-core
//! `docs/design/events-and-resources.md`, 8.1). Before it, `urn:iki:ledger:transition` was not
//! a resource at all, and nothing numbered a ledger's transitions in commit order.

mod common;

use common::*;

const FEED: &str = "urn:iki:ledger:transition";

fn kinds(kernel: &ikigai_core::Kernel, after: &str) -> Vec<String> {
    source(kernel, FEED, &[("after", after)])
        .lines()
        .skip(1)
        .map(|line| line.split("  ").nth(1).unwrap_or_default().to_string())
        .collect()
}

#[test]
fn every_transition_is_an_entry_in_commit_order() {
    let kernel = kernel();
    let item = append_iri(&kernel, "Fix the widget", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:claim",
        &[("item", "#1"), ("content", "worker-1")],
    );
    sink(
        &kernel,
        "urn:iki:ledger:comment",
        &[("item", "#1"), ("content", "Just a note")],
    );
    sink(&kernel, "urn:iki:ledger:close", &[("item", "#1")]);
    sink(&kernel, "urn:iki:ledger:reopen", &[("item", "#1")]);

    let text = source(&kernel, FEED, &[]);
    assert!(
        text.starts_with("5 transitions after 0, read through 5. The last is 5.\n"),
        "{text}"
    );
    assert!(text.contains(&format!("1  filed  {item}")), "{text}");
    // A plain comment is not a transition. A close of a claimed item writes its status and
    // its release in ONE update, numbered in the order that update writes them.
    assert_eq!(
        kinds(&kernel, "0"),
        ["filed", "claimed", "closed", "released", "reopened"]
    );
    // A follower resumes after the last entry it saw, and an empty page names the head.
    assert_eq!(kinds(&kernel, "3"), ["released", "reopened"]);
    assert_eq!(
        source(&kernel, FEED, &[("after", "5")]),
        "No transitions after 5. The last is 5.\n"
    );
    // The entries are named by their sequence, in the ledger's own graph.
    let rows = select(
        &kernel,
        "SELECT ?seq WHERE { GRAPH <urn:iki:ledger:graph:default> { \
         <urn:iki:ledger:default:transition:seq:4> <https://ikigai-rs.dev/ns/ledger#transitionSeq> ?seq } }",
    );
    assert!(rows.contains("4"), "{rows}");
}

#[test]
fn each_ledger_numbers_its_own() {
    let kernel = kernel();
    append(&kernel, "In default", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:acme:append",
        &[("content", "In acme")],
    );
    let acme = source(&kernel, "urn:iki:ledger:acme:transition", &[]);
    assert!(
        acme.starts_with("1 transition after 0, read through 1. The last is 1.\n"),
        "{acme}"
    );
}

#[test]
fn transition_is_a_reserved_ledger_name() {
    assert!(ikigai_ledger::ledger::RESERVED.contains(&"transition"));
}

/// The sequence lives on the entry only: a claim, release or status event keeps the shape its
/// readers know (no `transitionSeq`), and an entry is not one of its item's records (no
/// `ledger:onItem`), so history, the doctor and the data-safety counts never see it.
#[test]
fn an_entry_stays_apart_from_the_records_it_numbers() {
    let kernel = kernel();
    append(&kernel, "Fix the widget", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:claim",
        &[("item", "#1"), ("content", "worker-1")],
    );
    sink(&kernel, "urn:iki:ledger:close", &[("item", "#1")]);
    assert!(source(&kernel, FEED, &[]).starts_with("4 transitions"));
    let numbered_nodes = select(
        &kernel,
        "SELECT ?n WHERE { GRAPH <urn:iki:ledger:graph:default> { \
         ?n <https://ikigai-rs.dev/ns/ledger#transitionSeq> ?s \
         FILTER NOT EXISTS { ?n a <https://ikigai-rs.dev/ns/ledger#Transition> } } }",
    );
    assert!(!numbered_nodes.contains("urn:"), "{numbered_nodes}");
    let entries_on_items = select(
        &kernel,
        "SELECT ?n WHERE { GRAPH <urn:iki:ledger:graph:default> { \
         ?n a <https://ikigai-rs.dev/ns/ledger#Transition> ; \
         <https://ikigai-rs.dev/ns/ledger#onItem> ?i } }",
    );
    assert!(!entries_on_items.contains("urn:"), "{entries_on_items}");
    // The same query shape does see the entries, so the two absences above are not vacuous.
    let entries = select(
        &kernel,
        "SELECT ?n WHERE { GRAPH <urn:iki:ledger:graph:default> { \
         ?n a <https://ikigai-rs.dev/ns/ledger#Transition> } }",
    );
    assert!(
        entries.contains("urn:iki:ledger:default:transition:seq:4"),
        "{entries}"
    );
}

const NS: &str = "https://ikigai-rs.dev/ns/ledger#";
const LIVE: &str = "urn:iki:ledger:graph:default";

fn json_feed(kernel: &ikigai_core::Kernel, args: &[(&str, &str)]) -> serde_json::Value {
    let mut all = vec![("as", "application/json")];
    all.extend_from_slice(args);
    serde_json::from_str(&source(kernel, FEED, &all)).expect("the feed's JSON face")
}

/// Every quad of the entries, so a test can say the feed is unchanged quad for quad.
fn entry_quads(kernel: &ikigai_core::Kernel) -> String {
    select(
        kernel,
        &format!(
            "SELECT ?e ?p ?o WHERE {{ GRAPH <{LIVE}> {{ ?e a <{NS}Transition> ; ?p ?o }} }} \
             ORDER BY ?e ?p ?o"
        ),
    )
}

fn filed_claimed_closed(kernel: &ikigai_core::Kernel) {
    append(kernel, "Fix the widget", &[]);
    append(kernel, "A survivor", &[]);
    sink(
        kernel,
        "urn:iki:ledger:claim",
        &[("item", "#1"), ("content", "worker-1")],
    );
    sink(kernel, "urn:iki:ledger:close", &[("item", "#1")]);
}

/// ★ Ledger #4116, the hub's reproduction: a recoverable delete took the `transitionItem` and
/// (for the filing) `transitionEvent` edges with the item, so its entries dropped out of the
/// feed. Entries are immutable: the feed reads the same before and after.
#[test]
fn a_delete_leaves_the_feed_unchanged() {
    let kernel = kernel();
    filed_claimed_closed(&kernel);
    let before = source(&kernel, FEED, &[]);
    let quads = entry_quads(&kernel);
    assert!(before.starts_with("5 transitions"), "{before}");
    delete(&kernel, "urn:iki:ledger:item:1", &[]);
    assert_eq!(source(&kernel, FEED, &[]), before);
    assert_eq!(entry_quads(&kernel), quads);
}

/// ★ And a purge, of a deleted item or a live one, left them as orphaned half-entries.
#[test]
fn a_purge_leaves_the_feed_unchanged() {
    let kernel = kernel();
    filed_claimed_closed(&kernel);
    let before = source(&kernel, FEED, &[]);
    let quads = entry_quads(&kernel);
    delete(&kernel, "urn:iki:ledger:item:1", &[]);
    delete(&kernel, "urn:iki:ledger:purge", &[("content", "1")]);
    assert_eq!(source(&kernel, FEED, &[]), before);
    assert_eq!(entry_quads(&kernel), quads);
    // A live item purged outright.
    delete(&kernel, "urn:iki:ledger:purge", &[("content", "2")]);
    assert_eq!(source(&kernel, FEED, &[]), before);
    assert_eq!(entry_quads(&kernel), quads);
}

/// The answer has the layer feed's shape, `{ledger, epoch, last, entries}`, and each entry the
/// item's epoch and the principal, written in the same update.
#[test]
fn the_json_face_has_the_layer_feed_shape() {
    let kernel = kernel();
    assert_eq!(json_feed(&kernel, &[])["epoch"], serde_json::Value::Null);
    filed_claimed_closed(&kernel);
    let doc = json_feed(&kernel, &[]);
    assert_eq!(doc["ledger"], "default");
    assert_eq!(doc["last"], 5);
    assert_eq!(doc["head"], 5);
    let epoch = doc["epoch"]
        .as_str()
        .expect("an epoch once there is an entry");
    assert!(!epoch.is_empty() && epoch.len() <= 64, "{epoch}");
    let entries = doc["entries"].as_array().unwrap();
    let kinds: Vec<_> = entries
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["filed", "filed", "claimed", "closed", "released"]);
    assert!(entries[0].get("epoch").is_none(), "{doc}");
    for claimed in &entries[2..] {
        assert_eq!(claimed["epoch"], 1, "{doc}");
    }
    // Root carries no principal, so its writes name none.
    assert!(
        entries.iter().all(|e| e.get("principal").is_none()),
        "{doc}"
    );
    // The epoch is the feed's incarnation, kept from then on.
    sink(&kernel, "urn:iki:ledger:reopen", &[("item", "#1")]);
    assert_eq!(json_feed(&kernel, &[])["epoch"], epoch);
    assert_eq!(
        json_feed(&kernel, &[("after", "6"), ("epoch", epoch)])["last"],
        6
    );
    let refused = try_verb(
        &kernel,
        ikigai_core::Verb::Source,
        FEED,
        &[("after", "2"), ("epoch", "another-epoch")],
    );
    assert!(
        matches!(&refused, Err(ikigai_core::Error::Conflict(why)) if why.contains("resynchronize from 0")),
        "{refused:?}"
    );
}

/// ★ A gap stalled a follower: `after=0 limit=1` over a missing entry 1 came back empty with
/// nothing to resume from. `last` says how far the read got, past the gap.
#[test]
fn a_gap_does_not_stall_a_follower() {
    let kernel = kernel();
    filed_claimed_closed(&kernel);
    sink(
        &kernel,
        "urn:iki:store:update",
        &[(
            "content",
            &format!(
                "DELETE WHERE {{ GRAPH <{LIVE}> {{ <urn:iki:ledger:default:transition:seq:1> ?p ?o }} }}"
            ),
        )],
    );
    let page = json_feed(&kernel, &[("after", "0"), ("limit", "1")]);
    assert_eq!(page["last"], 1, "{page}");
    assert_eq!(page["entries"].as_array().unwrap().len(), 0, "{page}");
    let next = json_feed(&kernel, &[("after", "1"), ("limit", "1")]);
    assert_eq!(next["entries"][0]["seq"], 2, "{next}");
    assert_eq!(
        source(&kernel, FEED, &[("after", "0"), ("limit", "1")]),
        "No transitions after 0, read through 1. The last is 5.\n"
    );
}

/// A cursor past the head came from another feed (a restored store, or another one).
#[test]
fn a_cursor_past_the_head_is_a_conflict() {
    let kernel = kernel();
    append(&kernel, "Fix the widget", &[]);
    let refused = try_verb(&kernel, ikigai_core::Verb::Source, FEED, &[("after", "2")]);
    assert!(
        matches!(&refused, Err(ikigai_core::Error::Conflict(why))
            if why == "cursor 2 is past ledger `default`'s last transition, 1: this feed is not \
                       the one the cursor came from (restored, or another store), so \
                       resynchronize from 0"),
        "{refused:?}"
    );
}

/// An entry names the principal the door minted for its write: a takeover by another worker
/// records the release at the epoch it ended and the claim at the epoch it began.
#[test]
fn an_entry_names_the_principal_and_the_epoch_of_its_write() {
    const A: &str = "urn:iki:gonk:client:aaaa";
    const B: &str = "urn:iki:gonk:client:bbbb";
    let worker = |principal: &str| {
        ikigai_core::Capability::scoped(vec![
            "urn:cap:ledger:read:default".to_string(),
            "urn:cap:ledger:claim:default".to_string(),
            graph_read("default"),
            graph_write("default"),
        ])
        .with_principal(principal)
        .unwrap()
    };
    let kernel = kernel();
    append(&kernel, "Fix the widget", &[]);
    let claim = |who: &str, holder: &str, takeover: bool| {
        // A lease the test clock outruns, so the second claim may take it over.
        let mut args = vec![("item", "1"), ("content", holder), ("lease", "1s")];
        if takeover {
            args.extend([("takeover", "true"), ("from", "worker-a")]);
        }
        try_as(
            &kernel,
            &worker(who),
            ikigai_core::Verb::Sink,
            "urn:iki:ledger:claim",
            &args,
        )
        .unwrap_or_else(|e| panic!("claim as {who}: {e}"))
    };
    claim(A, "worker-a", false);
    claim(B, "worker-b", true);
    let doc = json_feed(&kernel, &[("after", "1")]);
    let entries = doc["entries"].as_array().unwrap();
    let seen: Vec<_> = entries
        .iter()
        .map(|e| {
            (
                e["kind"].as_str().unwrap(),
                e["epoch"].as_u64().unwrap(),
                e["principal"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        seen,
        [("claimed", 1, A), ("released", 1, B), ("claimed", 2, B)],
        "{doc}"
    );
    let text = source(&kernel, FEED, &[("after", "1"), ("limit", "1")]);
    assert!(text.contains("  claimed  "), "{text}");
    assert!(text.ends_with(&format!("  epoch 1  by {A}\n")), "{text}");
}
