//! **Per-verb ledger grants and keyed comments** (ledger #1178, #1143, #1147).
//!
//! A remote worker and the collaboration layer's mirror must claim and comment WITHOUT being
//! able to close. Until 0.7.0 `urn:cap:ledger:write:{ledger}` was the only grant that reached
//! any of those, so a grant without close could not be written down at all. These tests hold
//! the narrow grants to what they say:
//!
//! - `urn:cap:ledger:claim:{ledger}` takes, renews and releases claims, and nothing else;
//! - `urn:cap:ledger:comment:{ledger}` comments, and nothing else;
//! - `urn:cap:ledger:close:{ledger}` closes and reopens, and nothing else;
//! - `urn:cap:ledger:write:{ledger}` still means all of them, so no grant changed meaning.
//!
//! And the two record-keeping fixes that ride with them: a keyed comment replayed files
//! nothing (`existing`), and a close releases the claim it finds, recorded as a comment.

mod common;

use common::*;
use ikigai_core::{Capability, Error, Kernel, Verb};

const LEDGER: &str = "acme";

fn at(resource: &str) -> String {
    format!("urn:iki:ledger:{LEDGER}:{resource}")
}

/// The store grants every ledger caller holds, whatever it may do at the ledger: the
/// ledger reads and writes its graph under the CALLER's capability.
fn store_grants() -> Vec<String> {
    vec![graph_read(LEDGER), graph_write(LEDGER)]
}

/// The worker grant `README.md` publishes, spelled out as an operator's config line:
/// read, claim, comment, and the two store grants. No write, no close, no delete.
fn worker() -> Capability {
    let mut scopes = vec![
        format!("urn:cap:ledger:read:{LEDGER}"),
        format!("urn:cap:ledger:claim:{LEDGER}"),
        format!("urn:cap:ledger:comment:{LEDGER}"),
    ];
    scopes.extend(store_grants());
    Capability::scoped(scopes)
}

/// A dispatcher who closes and reopens and does nothing else.
fn closer() -> Capability {
    let mut scopes = vec![
        format!("urn:cap:ledger:read:{LEDGER}"),
        format!("urn:cap:ledger:close:{LEDGER}"),
    ];
    scopes.extend(store_grants());
    Capability::scoped(scopes)
}

/// The everyday writer, as every grant list before 0.7.0 spelled it.
fn writer() -> Capability {
    let mut scopes = vec![
        format!("urn:cap:ledger:read:{LEDGER}"),
        format!("urn:cap:ledger:write:{LEDGER}"),
    ];
    scopes.extend(store_grants());
    Capability::scoped(scopes)
}

/// A ledger with one open item in it, filed under root.
fn ledger_with_an_item() -> Kernel {
    let kernel = kernel();
    sink(&kernel, &at("append"), &[("content", "Fix the widget")]);
    kernel
}

fn denied(result: Result<String, Error>, what: &str) {
    match result {
        Err(Error::Denied(_)) => {}
        other => panic!("{what}: expected Denied, got {other:?}"),
    }
}

/// ★ The worker grant does what a worker does: claim, renew, comment, release.
#[test]
fn a_worker_grant_claims_renews_comments_and_releases() {
    let kernel = ledger_with_an_item();
    let w = worker();
    let claimed = try_as(
        &kernel,
        &w,
        Verb::Sink,
        &at("claim"),
        &[("item", "1"), ("content", "worker-1"), ("lease", "1h")],
    )
    .expect("a worker claims");
    assert!(claimed.contains("claimed by worker-1"), "{claimed}");
    try_as(
        &kernel,
        &w,
        Verb::Sink,
        &at("claim"),
        &[("item", "1"), ("content", "worker-1"), ("lease", "2h")],
    )
    .expect("a worker renews its own claim");
    let commented = try_as(
        &kernel,
        &w,
        Verb::Sink,
        &at("comment"),
        &[("item", "1"), ("content", "PR is up")],
    )
    .expect("a worker comments");
    assert!(commented.starts_with("commented on"), "{commented}");
    try_as(&kernel, &w, Verb::Delete, &at("claim"), &[("item", "1")]).expect("a worker releases");
    let view = source(&kernel, &at("item:1"), &[]);
    assert!(view.contains("PR is up"), "{view}");
}

/// ★ ...and nothing a worker must not: close, reopen, relabel, edit, link, defer, file,
/// delete. Every one is a typed `Denied`, and the item is unchanged.
#[test]
fn a_worker_grant_cannot_close_relabel_edit_or_file() {
    let kernel = ledger_with_an_item();
    let w = worker();
    denied(
        try_as(&kernel, &w, Verb::Sink, &at("close"), &[("item", "1")]),
        "close",
    );
    denied(
        try_as(
            &kernel,
            &w,
            Verb::Sink,
            &at("label"),
            &[("item", "1"), ("content", "urgent")],
        ),
        "label",
    );
    denied(
        try_as(
            &kernel,
            &w,
            Verb::Sink,
            &at("item:1"),
            &[("title", "Something else")],
        ),
        "edit",
    );
    denied(
        try_as(
            &kernel,
            &w,
            Verb::Sink,
            &at("append"),
            &[("content", "A new item")],
        ),
        "append",
    );
    denied(
        try_as(
            &kernel,
            &w,
            Verb::Sink,
            &at("defer"),
            &[("item", "1"), ("content", "not now")],
        ),
        "defer",
    );
    denied(
        try_as(&kernel, &w, Verb::Delete, &at("item:1"), &[]),
        "delete",
    );
    sink(&kernel, &at("close"), &[("item", "1")]);
    denied(
        try_as(&kernel, &w, Verb::Sink, &at("reopen"), &[("item", "1")]),
        "reopen",
    );
    let view = source(&kernel, &at("item:1"), &[]);
    assert!(view.contains("Fix the widget"), "{view}");
    assert!(!view.contains("urgent"), "{view}");
}

/// The three narrow grants are disjoint: a closer cannot claim or comment, a commenter
/// cannot claim, a claimer cannot comment.
#[test]
fn the_narrow_grants_do_not_imply_each_other() {
    let kernel = ledger_with_an_item();
    let c = closer();
    denied(
        try_as(
            &kernel,
            &c,
            Verb::Sink,
            &at("claim"),
            &[("item", "1"), ("content", "me")],
        ),
        "a closer claiming",
    );
    denied(
        try_as(
            &kernel,
            &c,
            Verb::Sink,
            &at("comment"),
            &[("item", "1"), ("content", "hi")],
        ),
        "a closer commenting",
    );
    let only = |verb: &str| {
        let mut scopes = vec![
            format!("urn:cap:ledger:read:{LEDGER}"),
            format!("urn:cap:ledger:{verb}:{LEDGER}"),
        ];
        scopes.extend(store_grants());
        Capability::scoped(scopes)
    };
    denied(
        try_as(
            &kernel,
            &only("comment"),
            Verb::Sink,
            &at("claim"),
            &[("item", "1"), ("content", "me")],
        ),
        "a commenter claiming",
    );
    denied(
        try_as(
            &kernel,
            &only("claim"),
            Verb::Sink,
            &at("comment"),
            &[("item", "1"), ("content", "hi")],
        ),
        "a claimer commenting",
    );
    let closed =
        try_as(&kernel, &c, Verb::Sink, &at("close"), &[("item", "1")]).expect("a closer closes");
    assert!(closed.starts_with("closed"), "{closed}");
    try_as(&kernel, &c, Verb::Sink, &at("reopen"), &[("item", "1")]).expect("a closer reopens");
}

/// A narrow grant names ONE ledger, exactly like the four before it: a worker for `acme`
/// is refused at `bosatsu`.
#[test]
fn a_narrow_grant_names_one_ledger() {
    let kernel = kernel();
    sink(
        &kernel,
        "urn:iki:ledger:bosatsu:append",
        &[("content", "Elsewhere")],
    );
    let mut scopes = vec![
        format!("urn:cap:ledger:claim:{LEDGER}"),
        format!("urn:cap:ledger:comment:{LEDGER}"),
        "urn:cap:ledger:read:bosatsu".to_string(),
        graph_read("bosatsu"),
        graph_write("bosatsu"),
    ];
    scopes.extend(store_grants());
    let w = Capability::scoped(scopes);
    denied(
        try_as(
            &kernel,
            &w,
            Verb::Sink,
            "urn:iki:ledger:bosatsu:claim",
            &[("item", "1"), ("content", "me")],
        ),
        "claim at another ledger",
    );
    denied(
        try_as(
            &kernel,
            &w,
            Verb::Sink,
            "urn:iki:ledger:bosatsu:comment",
            &[("item", "1"), ("content", "hi")],
        ),
        "comment at another ledger",
    );
}

/// ★ `write:{ledger}` keeps meaning every one of them: no grant issued before 0.7.0 lost
/// anything.
#[test]
fn the_write_grant_still_claims_comments_and_closes() {
    let kernel = ledger_with_an_item();
    let w = writer();
    try_as(
        &kernel,
        &w,
        Verb::Sink,
        &at("claim"),
        &[("item", "1"), ("content", "me")],
    )
    .expect("write claims");
    try_as(&kernel, &w, Verb::Delete, &at("claim"), &[("item", "1")]).expect("write releases");
    try_as(
        &kernel,
        &w,
        Verb::Sink,
        &at("comment"),
        &[("item", "1"), ("content", "hi")],
    )
    .expect("write comments");
    try_as(&kernel, &w, Verb::Sink, &at("close"), &[("item", "1")]).expect("write closes");
    try_as(&kernel, &w, Verb::Sink, &at("reopen"), &[("item", "1")]).expect("write reopens");
}

/// How many comments the item carries, counted in the graph rather than in an answer.
fn comment_count(kernel: &Kernel) -> usize {
    let json: serde_json::Value = serde_json::from_str(&select(
        kernel,
        &format!(
            "SELECT (COUNT(?c) AS ?n) WHERE {{ GRAPH <urn:iki:ledger:graph:{LEDGER}> \
             {{ ?c a <https://ikigai-rs.dev/ns/ledger#Comment> }} }}"
        ),
    ))
    .expect("the store answers JSON");
    json["results"]["bindings"][0]["n"]["value"]
        .as_str()
        .and_then(|n| n.parse().ok())
        .expect("a count")
}

const HANDBACK: &str = "urn:collab:bobby:msg:01m4handback";

/// ★ A keyed comment replayed files nothing and answers the comment it names, `existing`
/// — so a hand-back seen twice comments once.
#[test]
fn a_keyed_comment_replayed_is_existing() {
    let kernel = ledger_with_an_item();
    let w = worker();
    let args = [
        ("item", "1"),
        ("content", "Handed back: PR 12 merged"),
        ("key", HANDBACK),
    ];
    let first = try_as(&kernel, &w, Verb::Sink, &at("comment"), &args).expect("first");
    assert!(first.starts_with("commented on"), "{first}");
    assert!(!first.contains("existing"), "{first}");
    let second = try_as(&kernel, &w, Verb::Sink, &at("comment"), &args).expect("replay");
    assert!(second.trim_end().ends_with("existing"), "{second}");
    // The same comment IRI in both answers.
    let iri = |answer: &str| {
        answer
            .split(['(', ')'])
            .nth(1)
            .expect("the answer names the comment")
            .to_string()
    };
    assert_eq!(iri(&first), iri(&second));
    assert_eq!(comment_count(&kernel), 1);

    // The JSON face says so in its outcome, and carries the key and the comment as written.
    let json = try_as(
        &kernel,
        &w,
        Verb::Sink,
        &at("comment"),
        &[
            ("item", "1"),
            ("content", "a different body"),
            ("key", HANDBACK),
            ("as", "application/json"),
        ],
    )
    .expect("replay as JSON");
    let answer: serde_json::Value = serde_json::from_str(&json).expect("JSON");
    assert_eq!(answer["outcome"], "existing", "{json}");
    assert_eq!(answer["key"], HANDBACK, "{json}");
    assert_eq!(
        answer["comment"]["text"], "Handed back: PR 12 merged",
        "{json}"
    );
    assert_eq!(comment_count(&kernel), 1);
}

/// A keyed comment's key is checked before anything is written, with the append's shape.
#[test]
fn a_bad_comment_key_is_refused_before_anything_is_written() {
    let kernel = ledger_with_an_item();
    let refused = try_verb(
        &kernel,
        Verb::Sink,
        &at("comment"),
        &[("item", "1"), ("content", "hi"), ("key", "a/b")],
    );
    assert!(
        matches!(refused, Err(Error::InvalidArgument { ref name, .. }) if name == "key"),
        "{refused:?}"
    );
    assert_eq!(comment_count(&kernel), 0);
}

/// Concurrent keyed comments with one key file one comment: the check and the write are
/// one store update.
#[test]
fn concurrent_keyed_comments_file_exactly_one() {
    for _ in 0..10 {
        let kernel = ledger_with_an_item();
        let n = 6;
        let barrier = std::sync::Barrier::new(n);
        let answers: Vec<String> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..n)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        sink(
                            &kernel,
                            &at("comment"),
                            &[("item", "1"), ("content", "once"), ("key", HANDBACK)],
                        )
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        assert_eq!(comment_count(&kernel), 1, "{answers:?}");
        let fresh = answers.iter().filter(|a| !a.contains("existing")).count();
        assert_eq!(fresh, 1, "{answers:?}");
    }
}

/// A comment key and an item key live apart: the same string may name an item and a
/// comment, and neither shadows the other.
#[test]
fn a_comment_key_does_not_collide_with_an_item_key() {
    let kernel = ledger_with_an_item();
    sink(
        &kernel,
        &at("comment"),
        &[("item", "1"), ("content", "keyed"), ("key", "shared")],
    );
    let filed = sink(
        &kernel,
        &at("append"),
        &[("content", "Keyed item"), ("key", "shared")],
    );
    assert!(!filed.contains("existing"), "{filed}");
    let again = sink(
        &kernel,
        &at("comment"),
        &[("item", "2"), ("content", "keyed"), ("key", "shared")],
    );
    assert!(again.trim_end().ends_with("existing"), "{again}");
    assert!(
        again.contains("acme#1"),
        "the answer names the item it is ON: {again}"
    );
}

/// ★ A close releases the claim it finds (ledger #1147), and the release is recorded as a
/// comment, so who held the work when it closed is not lost.
#[test]
fn a_close_releases_the_claim_and_records_it() {
    let kernel = ledger_with_an_item();
    sink(
        &kernel,
        &at("claim"),
        &[("item", "1"), ("content", "worker-1"), ("lease", "1h")],
    );
    let closed = sink(&kernel, &at("close"), &[("item", "1")]);
    assert!(closed.contains("released worker-1"), "{closed}");
    let view = source(&kernel, &at("item:1"), &[("as", "application/json")]);
    let doc: serde_json::Value = serde_json::from_str(&view).expect("JSON");
    let item = &doc["item"];
    assert!(item["claim"].is_null(), "{view}");
    let comments = item["comments"].as_array().expect("comments");
    assert!(
        comments
            .iter()
            .any(|c| c["text"].as_str().is_some_and(|t| t.contains("worker-1"))),
        "{view}"
    );
    let holders = source(
        &kernel,
        &at("items"),
        &[("holder", "worker-1"), ("status", "all")],
    );
    assert!(!holders.contains("#1"), "{holders}");
}

/// An unclaimed close writes no release record.
#[test]
fn an_unclaimed_close_records_no_release() {
    let kernel = ledger_with_an_item();
    let closed = sink(&kernel, &at("close"), &[("item", "1")]);
    assert!(!closed.contains("released"), "{closed}");
    assert_eq!(comment_count(&kernel), 0);
}

/// ⚠ **What the narrow grants do NOT close, pinned so nobody believes otherwise.** A worker
/// must hold this ledger's store graph-write token, because the ledger writes under the
/// CALLER's capability. In a kernel that also lets the caller resolve the store's narrow door
/// directly, that token is raw quad authority: one `graph-update` closes the item without the
/// close grant. The ledger cannot stop that; a host's door can, and gonk's does (it refuses a
/// raw `urn:iki:store:graph-update` against a ledger graph without an explicit raw grant,
/// ledger #878). This test is the bare kernel, where nothing does.
#[test]
fn the_store_token_a_worker_holds_is_still_raw_authority_without_a_door_rule() {
    let kernel = ledger_with_an_item();
    let iri = source(&kernel, &at("items"), &[("as", "application/json")]);
    let doc: serde_json::Value = serde_json::from_str(&iri).expect("JSON");
    let item = doc["items"][0]["iri"]
        .as_str()
        .expect("an item IRI")
        .to_string();
    let update = format!(
        "DELETE {{ GRAPH <urn:iki:ledger:graph:{LEDGER}> {{ <{item}> \
         <https://ikigai-rs.dev/ns/ledger#status> ?s }} }} \
         INSERT {{ GRAPH <urn:iki:ledger:graph:{LEDGER}> {{ <{item}> \
         <https://ikigai-rs.dev/ns/ledger#status> <https://ikigai-rs.dev/ns/ledger#closed> }} }} \
         WHERE {{ GRAPH <urn:iki:ledger:graph:{LEDGER}> {{ <{item}> \
         <https://ikigai-rs.dev/ns/ledger#status> ?s }} }}"
    );
    try_as(
        &kernel,
        &worker(),
        Verb::Sink,
        "urn:iki:store:graph-update",
        &[
            ("graph", &format!("urn:iki:ledger:graph:{LEDGER}")),
            ("content", &update),
        ],
    )
    .expect("the bare kernel admits a raw write under the worker's store token");
    let view = source(&kernel, &at("item:1"), &[("as", "application/json")]);
    let doc: serde_json::Value = serde_json::from_str(&view).expect("JSON");
    assert_eq!(doc["item"]["status"], "closed", "{view}");
}
