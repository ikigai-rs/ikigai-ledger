//! **The gap a dispatch note would close** (ledger #3710; the design is
//! `docs/dispatch-notes.md`).
//!
//! A dispatcher that wants to tell the worker holding an item something mid-item has only
//! `comment`. This test pins what a worker then sees: the dispatcher's comment and a comment
//! from anyone else holding the comment grant have the same shape on every face. Neither
//! carries the grant it was written under, a recipient, or the claim epoch it was meant for,
//! so a worker that obeyed comments would obey anyone who can comment. That is why workers
//! today treat every comment as data. Only the author's IRI differs, and nothing in the record
//! says that IRI held the dispatcher grant.
//!
//! When the dispatch note is built, this test is where it shows: the note must differ from
//! an ordinary comment in kind, sender grant, recipient and epoch, and this test will change.

mod common;

use common::*;
use ikigai_core::{Capability, Kernel, Verb};

const LEDGER: &str = "acme";
const WORKER: &str = "urn:iki:gonk:client:aaaa";
const OUTSIDER: &str = "urn:iki:gonk:client:cccc";
const DISPATCHER: &str = "urn:iki:gonk:passkey:dddd";

fn at(resource: &str) -> String {
    format!("urn:iki:ledger:{LEDGER}:{resource}")
}

fn grant(verbs: &[&str], principal: &str) -> Capability {
    let mut scopes: Vec<String> = verbs
        .iter()
        .map(|verb| format!("urn:cap:ledger:{verb}:{LEDGER}"))
        .collect();
    scopes.push(graph_read(LEDGER));
    scopes.push(graph_write(LEDGER));
    Capability::scoped(scopes)
        .with_principal(principal)
        .unwrap()
}

/// An item held by a worker at epoch 1, with one comment from the dispatcher (who holds
/// close) and one from an outsider (who holds only comment), with the same text.
fn held_item_with_two_comments() -> Kernel {
    let kernel = kernel();
    sink(&kernel, &at("append"), &[("content", "Fix the widget")]);
    let worker = grant(&["read", "claim", "comment"], WORKER);
    let answer = try_as(
        &kernel,
        &worker,
        Verb::Sink,
        &at("claim"),
        &[("item", "1"), ("lease", "30m"), ("content", "worker-1")],
    )
    .expect("the worker claims");
    assert!(answer.contains("at epoch 1"), "{answer}");

    let dispatcher = grant(&["read", "claim", "comment", "close"], DISPATCHER);
    let outsider = grant(&["comment"], OUTSIDER);
    for who in [&dispatcher, &outsider] {
        try_as(
            &kernel,
            who,
            Verb::Sink,
            &at("comment"),
            &[("item", "1"), ("content", "Stop and rebase on main first")],
        )
        .expect("the comment is written");
    }
    kernel
}

#[test]
fn a_dispatchers_comment_has_the_same_shape_as_anyones() {
    let kernel = held_item_with_two_comments();
    let view = source(&kernel, &at("item:1"), &[("as", "application/json")]);
    let json: serde_json::Value = serde_json::from_str(&view).expect("JSON");
    let comments = json["item"]["comments"].as_array().expect("comments");
    assert_eq!(comments.len(), 2, "{view}");

    let keys = |c: &serde_json::Value| {
        let mut k: Vec<String> = c.as_object().expect("object").keys().cloned().collect();
        k.sort();
        k
    };
    assert_eq!(
        keys(&comments[0]),
        keys(&comments[1]),
        "the two comments have the same fields"
    );
    for c in comments {
        for absent in ["kind", "to", "recipient", "epoch", "grant", "sender"] {
            assert!(c.get(absent).is_none(), "a comment has no `{absent}`: {c}");
        }
        assert_eq!(c["text"], "Stop and rebase on main first");
    }
    // The only difference is who wrote it, and the record does not say what that author
    // could do.
    assert_eq!(comments[0]["author"], DISPATCHER, "{view}");
    assert_eq!(comments[1]["author"], OUTSIDER, "{view}");
}

#[test]
fn the_plain_face_shows_two_comments_alike() {
    let kernel = held_item_with_two_comments();
    let text = source(&kernel, &at("item:1"), &[]);
    assert!(text.contains("2 comment(s):"), "{text}");
    assert_eq!(
        text.matches("Stop and rebase on main first").count(),
        2,
        "{text}"
    );
    for word in ["dispatch", "addressed", "instruction"] {
        assert!(
            !text.contains(word),
            "nothing marks one as `{word}`: {text}"
        );
    }
}
