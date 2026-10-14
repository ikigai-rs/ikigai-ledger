//! **Awaiting the dispatcher** (ledger #1199): a worker's hand-back is a STATE, not a
//! comment-text convention.
//!
//! A worker holds the claim and comment grants and cannot label (labeling needs write), so
//! until this the only way to say "done, waiting for you to check" was a comment whose text
//! started with `Evidence:` — a convention `next` knew nothing about, so it re-offered the
//! handed-back item to the next worker. Now:
//!
//! - `comment handback=true`, under the comment grant, writes
//!   `ledger:awaiting <urn:iki:ledger:awaiting:dispatcher>` in the same update as the comment;
//! - `next` excludes an awaiting item, `why: "awaiting-dispatcher"`;
//! - close, reopen, a later comment with `rejects=true`, or any comment from a caller holding
//!   the close grant clears it;
//! - the doctor knows the state, and the plain, JSON and Turtle faces show it.

mod common;

use common::*;
use ikigai_core::{Capability, Error, Kernel, Verb};

const LEDGER: &str = "acme";
const AWAITING: &str = "https://ikigai-rs.dev/ns/ledger#awaiting";
const DISPATCHER: &str = "urn:iki:ledger:awaiting:dispatcher";

fn at(resource: &str) -> String {
    format!("urn:iki:ledger:{LEDGER}:{resource}")
}

fn grant(verbs: &[&str]) -> Capability {
    let mut scopes: Vec<String> = verbs
        .iter()
        .map(|verb| format!("urn:cap:ledger:{verb}:{LEDGER}"))
        .collect();
    scopes.push(graph_read(LEDGER));
    scopes.push(graph_write(LEDGER));
    Capability::scoped(scopes)
}

/// The worker grant `README.md` publishes: read, claim, comment.
fn worker() -> Capability {
    grant(&["read", "claim", "comment"])
}

/// A dispatcher: the worker's grants and close.
fn dispatcher() -> Capability {
    grant(&["read", "claim", "comment", "close"])
}

fn ledger_with_an_item() -> Kernel {
    let kernel = kernel();
    sink(&kernel, &at("append"), &[("content", "Fix the widget")]);
    kernel
}

fn comment(kernel: &Kernel, who: &Capability, args: &[(&str, &str)]) -> Result<String, Error> {
    let mut all = vec![("item", "1")];
    all.extend_from_slice(args);
    try_as(kernel, who, Verb::Sink, &at("comment"), &all)
}

fn item_json(kernel: &Kernel) -> serde_json::Value {
    let view = source(kernel, &at("item:1"), &[("as", "application/json")]);
    serde_json::from_str(&view).expect("JSON")
}

fn awaiting(kernel: &Kernel) -> serde_json::Value {
    item_json(kernel)["item"]["awaiting"].clone()
}

fn next_json(kernel: &Kernel) -> serde_json::Value {
    let view = source(kernel, &at("next"), &[("as", "application/json")]);
    serde_json::from_str(&view).expect("JSON")
}

fn offered(kernel: &Kernel) -> bool {
    next_json(kernel)["ranking"]
        .as_array()
        .expect("a ranking")
        .iter()
        .any(|r| r["item"]["number"] == 1)
}

/// The worker's whole loop up to the hand-back: claim, comment the evidence, release.
fn hand_back(kernel: &Kernel) {
    let w = worker();
    try_as(
        kernel,
        &w,
        Verb::Sink,
        &at("claim"),
        &[("item", "1"), ("content", "worker-1")],
    )
    .unwrap();
    comment(
        kernel,
        &w,
        &[
            ("content", "https://github.com/x/y/pull/1 CI green"),
            ("handback", "true"),
        ],
    )
    .unwrap();
    try_as(
        kernel,
        &w,
        Verb::Delete,
        &at("claim"),
        &[("item", "1"), ("from", "worker-1")],
    )
    .unwrap();
}

/// ★ THE REPRODUCTION. Before 0.8.0 `handback=true` was ignored and the item, released by
/// its worker, was offered by `next` to the next worker as if nothing had happened.
#[test]
fn a_handed_back_item_is_not_offered_by_next() {
    let kernel = ledger_with_an_item();
    assert!(offered(&kernel));
    hand_back(&kernel);
    let next = next_json(&kernel);
    assert!(
        !offered(&kernel),
        "a handed-back item was re-offered: {next}"
    );
    let excluded = next["excluded"].as_array().expect("exclusions");
    assert_eq!(excluded.len(), 1, "{next}");
    assert_eq!(excluded[0]["why"], "awaiting-dispatcher");
    assert_eq!(excluded[0]["item"]["number"], 1);
    assert!(
        excluded[0]["reason"]
            .as_str()
            .unwrap()
            .contains("dispatcher"),
        "{next}"
    );
    let plain = source(&kernel, &at("next"), &[]);
    assert!(plain.contains("awaiting the dispatcher"), "{plain}");
}

/// ★ ledger #1455: a worker that hands back WITHOUT releasing still holds the claim, and
/// through 0.11.1 `next` gave the claim as the reason ("claimed by worker-1"). The hand-back is
/// what the dispatcher acts on, so it is the reason, before the release and after it alike.
#[test]
fn a_handback_still_claimed_is_excluded_as_awaiting_not_as_claimed() {
    let kernel = ledger_with_an_item();
    let w = worker();
    try_as(
        &kernel,
        &w,
        Verb::Sink,
        &at("claim"),
        &[("item", "1"), ("content", "worker-1"), ("lease", "30m")],
    )
    .unwrap();
    // Claimed and not handed back: the claim is the reason, as before.
    let next = next_json(&kernel);
    assert_eq!(next["excluded"][0]["why"], "claimed", "{next}");
    comment(
        &kernel,
        &w,
        &[("content", "PR green, over to you"), ("handback", "true")],
    )
    .unwrap();
    // The claim is still held…
    assert_eq!(item_json(&kernel)["item"]["claim"]["holder"], "worker-1");
    // …and the reason is the hand-back.
    let next = next_json(&kernel);
    let excluded = next["excluded"].as_array().expect("exclusions");
    assert_eq!(excluded.len(), 1, "{next}");
    assert_eq!(excluded[0]["why"], "awaiting-dispatcher", "{next}");
    let plain = source(&kernel, &at("next"), &[]);
    assert!(
        plain.contains("acme#1 handed back, awaiting the dispatcher"),
        "{plain}"
    );
    assert!(!plain.contains("claimed by worker-1"), "{plain}");
    // The release changes nothing a reader of `next` sees.
    try_as(
        &kernel,
        &w,
        Verb::Delete,
        &at("claim"),
        &[("item", "1"), ("from", "worker-1")],
    )
    .unwrap();
    assert_eq!(
        next_json(&kernel)["excluded"][0]["why"],
        "awaiting-dispatcher"
    );
}

#[test]
fn the_handback_is_recorded_on_every_face() {
    let kernel = ledger_with_an_item();
    assert_eq!(awaiting(&kernel), serde_json::Value::Null);
    hand_back(&kernel);
    assert_eq!(awaiting(&kernel), "dispatcher");

    let turtle = source(&kernel, &at("item:1"), &[("as", "text/turtle")]);
    assert!(
        turtle.contains("ledger:awaiting <urn:iki:ledger:awaiting:dispatcher>"),
        "{turtle}"
    );
    let plain = source(&kernel, &at("item:1"), &[]);
    assert!(plain.contains("  awaiting: dispatcher\n"), "{plain}");
    let listing = source(&kernel, &at("items"), &[]);
    assert!(listing.contains("awaiting-dispatcher"), "{listing}");

    // In the graph itself, not just in what a face says.
    let rows = select(
        &kernel,
        &format!(
            "SELECT ?v WHERE {{ GRAPH <urn:iki:ledger:graph:{LEDGER}> {{ ?i <{AWAITING}> ?v }} }}"
        ),
    );
    assert!(rows.contains(DISPATCHER), "{rows}");
}

#[test]
fn the_handback_answer_says_so() {
    let kernel = ledger_with_an_item();
    let plain = comment(
        &kernel,
        &worker(),
        &[("content", "evidence"), ("handback", "true")],
    )
    .unwrap();
    assert!(
        plain.starts_with("commented on acme#1 (") && plain.ends_with(") awaiting-dispatcher\n"),
        "{plain}"
    );
    let json = comment(
        &kernel,
        &worker(),
        &[
            ("content", "more evidence"),
            ("handback", "true"),
            ("as", "application/json"),
        ],
    )
    .unwrap();
    let doc: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(doc["outcome"], "commented");
    assert_eq!(doc["handback"], "set");
}

/// A plain comment, a worker's, changes nothing: the worker cannot un-hand-back by talking.
#[test]
fn a_workers_plain_comment_leaves_it_awaiting() {
    let kernel = ledger_with_an_item();
    hand_back(&kernel);
    comment(&kernel, &worker(), &[("content", "one more note")]).unwrap();
    assert_eq!(awaiting(&kernel), "dispatcher");
    assert!(!offered(&kernel));
}

/// ★ The dispatcher saying the evidence did not hold makes the item ready again.
#[test]
fn a_rejecting_comment_clears_it() {
    let kernel = ledger_with_an_item();
    hand_back(&kernel);
    let answer = comment(
        &kernel,
        &worker(),
        &[("content", "CI is red on main"), ("rejects", "true")],
    )
    .unwrap();
    assert!(answer.ends_with(") cleared-awaiting\n"), "{answer}");
    assert_eq!(awaiting(&kernel), serde_json::Value::Null);
    assert!(offered(&kernel));
}

#[test]
fn any_comment_under_the_close_grant_clears_it() {
    let kernel = ledger_with_an_item();
    hand_back(&kernel);
    comment(
        &kernel,
        &dispatcher(),
        &[("content", "not quite: see review")],
    )
    .unwrap();
    assert_eq!(awaiting(&kernel), serde_json::Value::Null);
    assert!(offered(&kernel));
}

/// The write grant covers close, so its holder is a dispatcher too; and so is root.
#[test]
fn the_write_grant_and_root_clear_it_as_dispatchers() {
    let kernel = ledger_with_an_item();
    hand_back(&kernel);
    comment(&kernel, &grant(&["read", "write"]), &[("content", "no")]).unwrap();
    assert_eq!(awaiting(&kernel), serde_json::Value::Null);

    hand_back(&kernel);
    sink(
        &kernel,
        &at("comment"),
        &[("item", "1"), ("content", "still no")],
    );
    assert_eq!(awaiting(&kernel), serde_json::Value::Null);
}

/// A dispatcher handing an item back (on a worker's behalf) sets it; `handback` wins.
#[test]
fn a_dispatcher_can_hand_back_too() {
    let kernel = ledger_with_an_item();
    comment(
        &kernel,
        &dispatcher(),
        &[("content", "evidence"), ("handback", "true")],
    )
    .unwrap();
    assert_eq!(awaiting(&kernel), "dispatcher");
}

#[test]
fn close_clears_it_and_reopen_leaves_it_clear() {
    let kernel = ledger_with_an_item();
    hand_back(&kernel);
    sink(&kernel, &at("close"), &[("item", "1")]);
    assert_eq!(awaiting(&kernel), serde_json::Value::Null);
    sink(&kernel, &at("reopen"), &[("item", "1")]);
    assert_eq!(awaiting(&kernel), serde_json::Value::Null);
    assert!(offered(&kernel));
}

/// Reopen clears it on its own: a closed item can only be awaiting through an out-of-band
/// write, since close clears it, and reopening must not resurrect a stale hand-back.
#[test]
fn reopen_clears_an_out_of_band_awaiting() {
    let kernel = ledger_with_an_item();
    sink(&kernel, &at("close"), &[("item", "1")]);
    let iri = item_json(&kernel)["item"]["iri"]
        .as_str()
        .unwrap()
        .to_string();
    raw(&kernel, &format!("<{iri}> <{AWAITING}> <{DISPATCHER}> ."));
    assert_eq!(awaiting(&kernel), "dispatcher");
    sink(&kernel, &at("reopen"), &[("item", "1")]);
    assert_eq!(awaiting(&kernel), serde_json::Value::Null);
}

#[test]
fn handback_and_rejects_together_are_refused_and_nothing_is_written() {
    let kernel = ledger_with_an_item();
    let refused = comment(
        &kernel,
        &worker(),
        &[("content", "x"), ("handback", "true"), ("rejects", "true")],
    );
    assert!(
        matches!(refused, Err(Error::InvalidArgument { ref name, .. }) if name == "rejects"),
        "{refused:?}"
    );
    let refused = comment(&kernel, &worker(), &[("content", "x"), ("handback", "yes")]);
    assert!(
        matches!(refused, Err(Error::InvalidArgument { ref name, .. }) if name == "handback"),
        "{refused:?}"
    );
    assert_eq!(
        item_json(&kernel)["item"]["comments"],
        serde_json::json!([])
    );
}

/// A keyed hand-back replayed writes nothing, so it cannot re-set a state the dispatcher has
/// since cleared.
#[test]
fn a_replayed_keyed_handback_does_not_reset_a_cleared_state() {
    let kernel = ledger_with_an_item();
    let args = [
        ("content", "evidence"),
        ("handback", "true"),
        ("key", "worker-1:run-7"),
    ];
    comment(&kernel, &worker(), &args).unwrap();
    assert_eq!(awaiting(&kernel), "dispatcher");
    comment(
        &kernel,
        &worker(),
        &[("content", "no"), ("rejects", "true")],
    )
    .unwrap();
    let replay = comment(&kernel, &worker(), &args).unwrap();
    assert!(replay.ends_with(" existing\n"), "{replay}");
    assert_eq!(awaiting(&kernel), serde_json::Value::Null);
}

// ------------------------------------------------------------------------- doctor

fn doctor(kernel: &Kernel) -> serde_json::Value {
    let view = source(kernel, &at("doctor"), &[("as", "application/json")]);
    serde_json::from_str(&view).expect("JSON")
}

/// An in-flight item handed back and released is waiting on its dispatcher, not abandoned.
#[test]
fn the_doctor_does_not_call_a_handed_back_item_abandoned() {
    let kernel = ledger_with_an_item();
    sink(
        &kernel,
        &at("item:1:state"),
        &[("from", "filed"), ("to", "resolving")],
    );
    hand_back(&kernel);
    let report = doctor(&kernel);
    assert_eq!(report["problems"], serde_json::json!([]), "{report}");
    assert!(
        report["checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "awaiting-closed"),
        "{report}"
    );
}

/// What no ledger write produces (close clears it) and only an out-of-band one can.
#[test]
fn the_doctor_reports_a_closed_item_still_awaiting() {
    let kernel = ledger_with_an_item();
    sink(&kernel, &at("close"), &[("item", "1")]);
    let iri = item_json(&kernel)["item"]["iri"]
        .as_str()
        .unwrap()
        .to_string();
    raw(&kernel, &format!("<{iri}> <{AWAITING}> <{DISPATCHER}> ."));
    let report = doctor(&kernel);
    let problems = report["problems"].as_array().unwrap();
    assert_eq!(problems.len(), 1, "{report}");
    assert_eq!(problems[0]["check"], "awaiting-closed");
    assert_eq!(problems[0]["item"]["number"], 1);
}

/// A raw write to this ledger's graph.
fn raw(kernel: &Kernel, triples: &str) {
    sink(
        kernel,
        "urn:iki:store:update",
        &[(
            "content",
            &format!("INSERT DATA {{ GRAPH <urn:iki:ledger:graph:{LEDGER}> {{ {triples} }} }}"),
        )],
    );
}
