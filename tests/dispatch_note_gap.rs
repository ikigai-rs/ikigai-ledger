//! **Dispatch notes** (ledger #3710 designed them in `docs/dispatch-notes.md`, ledger #3718
//! built them).
//!
//! Until #3718 this file pinned a gap: a dispatcher's comment and a comment from anyone else
//! holding the comment grant had the same shape on every face, so a worker that obeyed
//! comments would obey anyone who can comment. A dispatcher now sends a NOTE through its own
//! resource, `urn:iki:ledger:{ledger}:note`, and the note differs from an ordinary comment in
//! kind, sender grant, recipient and epoch, each recorded by the write and never taken from
//! the caller. An ordinary comment is unchanged.
//!
//! The four things a note can never do are pinned here, one test each: widen a grant, act on
//! the item, reach a later epoch, be edited.

mod common;

use common::*;
use ikigai_core::{Capability, Error, Kernel, Verb};

const LEDGER: &str = "acme";
const WORKER: &str = "urn:iki:gonk:client:aaaa";
const OTHER_WORKER: &str = "urn:iki:gonk:client:bbbb";
const OUTSIDER: &str = "urn:iki:gonk:client:cccc";
const DISPATCHER: &str = "urn:iki:gonk:passkey:dddd";
const TEXT: &str = "Stop and rebase on main first";

fn at(resource: &str) -> String {
    format!("urn:iki:ledger:{LEDGER}:{resource}")
}

fn grant(verbs: &[&str], principal: &str) -> Capability {
    Capability::scoped(scopes(verbs))
        .with_principal(principal)
        .unwrap()
}

fn scopes(verbs: &[&str]) -> Vec<String> {
    let mut scopes: Vec<String> = verbs
        .iter()
        .map(|verb| format!("urn:cap:ledger:{verb}:{LEDGER}"))
        .collect();
    scopes.push(graph_read(LEDGER));
    scopes.push(graph_write(LEDGER));
    scopes
}

fn worker() -> Capability {
    grant(&["read", "claim", "comment"], WORKER)
}

fn dispatcher() -> Capability {
    grant(&["read", "claim", "comment", "close"], DISPATCHER)
}

fn note(kernel: &Kernel, who: &Capability, args: &[(&str, &str)]) -> Result<String, Error> {
    try_as(kernel, who, Verb::Sink, &at("note"), args)
}

/// An item held by the worker at epoch 1.
fn held_item() -> Kernel {
    let kernel = kernel();
    sink(&kernel, &at("append"), &[("content", "Fix the widget")]);
    let answer = try_as(
        &kernel,
        &worker(),
        Verb::Sink,
        &at("claim"),
        &[("item", "1"), ("lease", "30m"), ("content", "worker-1")],
    )
    .expect("the worker claims");
    assert!(answer.contains("at epoch 1"), "{answer}");
    kernel
}

/// [`held_item`] with a note from the dispatcher and a comment from an outsider, with the
/// same text.
fn held_item_with_a_note_and_a_comment() -> Kernel {
    let kernel = held_item();
    let sent = note(
        &kernel,
        &dispatcher(),
        &[("item", "1"), ("epoch", "1"), ("content", TEXT)],
    )
    .expect("the note is sent");
    assert!(
        sent.starts_with("note to worker-1 at epoch 1, sent under the dispatcher grant on "),
        "{sent}"
    );
    try_as(
        &kernel,
        &grant(&["comment"], OUTSIDER),
        Verb::Sink,
        &at("comment"),
        &[("item", "1"), ("content", TEXT)],
    )
    .expect("the comment is written");
    kernel
}

fn item_json(kernel: &Kernel) -> serde_json::Value {
    let view = source(kernel, &at("item:1"), &[("as", "application/json")]);
    serde_json::from_str(&view).expect("JSON")
}

#[test]
fn a_note_differs_from_a_comment_in_kind_grant_recipient_and_epoch() {
    let kernel = held_item_with_a_note_and_a_comment();
    let json = item_json(&kernel);
    let comments = json["item"]["comments"].as_array().expect("comments");
    assert_eq!(comments.len(), 2, "{json}");
    let (sent, other) = (&comments[0], &comments[1]);
    assert_eq!(sent["kind"], "note", "{sent}");
    assert_eq!(sent["to"]["holder"], "worker-1", "{sent}");
    assert_eq!(sent["to"]["principal"], WORKER, "{sent}");
    assert_eq!(sent["epoch"], 1, "{sent}");
    assert_eq!(
        sent["grant"],
        format!("urn:cap:ledger:close:{LEDGER}"),
        "{sent}"
    );
    assert_eq!(sent["author"], DISPATCHER, "{sent}");
    assert_eq!(sent["attribution"]["author_status"], "verified", "{sent}");

    assert_eq!(other["kind"], "comment", "{other}");
    for absent in ["to", "epoch", "grant"] {
        assert!(
            other.get(absent).is_none(),
            "a comment has no `{absent}`: {other}"
        );
    }
    assert_eq!(other["author"], OUTSIDER, "{other}");
    for c in comments {
        assert_eq!(c["text"], TEXT);
    }
}

#[test]
fn the_plain_face_says_which_is_the_note_in_words() {
    let kernel = held_item_with_a_note_and_a_comment();
    let text = source(&kernel, &at("item:1"), &[]);
    assert!(text.contains("2 comment(s):"), "{text}");
    assert_eq!(text.matches(TEXT).count(), 2, "{text}");
    assert_eq!(
        text.matches("note to worker-1 at epoch 1, sent under the dispatcher grant")
            .count(),
        1,
        "{text}"
    );
}

#[test]
fn the_graph_face_types_the_note_and_records_its_facts() {
    let kernel = held_item_with_a_note_and_a_comment();
    let turtle = source(&kernel, &at("item:1"), &[("as", "text/turtle")]);
    for term in [
        "DispatchNote",
        "noteTo",
        "noteHolder",
        "noteEpoch",
        "noteGrant",
    ] {
        assert!(turtle.contains(term), "`{term}` is in the graph: {turtle}");
    }
}

#[test]
fn only_the_dispatcher_grant_with_a_principal_sends_a_note() {
    let kernel = held_item();
    let args = [("item", "1"), ("epoch", "1"), ("content", TEXT)];
    for who in [
        worker(),
        grant(&["read", "claim", "comment"], OTHER_WORKER),
        grant(&["comment"], OUTSIDER),
    ] {
        let refused = note(&kernel, &who, &args);
        assert!(matches!(refused, Err(Error::Denied(_))), "{refused:?}");
    }
    // A dispatcher with no principal, and root with none, are refused too.
    let anonymous = Capability::scoped(scopes(&["close"]));
    for who in [anonymous, Capability::root()] {
        let refused = note(&kernel, &who, &args);
        assert!(
            matches!(&refused, Err(Error::Denied(m)) if m.contains("principal")),
            "{refused:?}"
        );
    }
    // The write grant is a dispatcher's too, and the note says which.
    let writer = grant(&["write"], DISPATCHER);
    let sent = note(&kernel, &writer, &args).expect("the write grant sends a note");
    assert!(sent.contains("sent under the write grant"), "{sent}");
    let json = item_json(&kernel);
    assert_eq!(
        json["item"]["comments"][0]["grant"],
        format!("urn:cap:ledger:write:{LEDGER}")
    );
}

#[test]
fn the_epoch_is_required_and_the_recipient_is_never_an_argument() {
    let kernel = held_item();
    let refused = note(&kernel, &dispatcher(), &[("item", "1"), ("content", TEXT)]);
    assert!(
        matches!(&refused, Err(Error::InvalidArgument { name, .. }) if name == "epoch"),
        "{refused:?}"
    );
    note(
        &kernel,
        &dispatcher(),
        &[
            ("item", "1"),
            ("epoch", "1"),
            ("content", TEXT),
            ("to", OTHER_WORKER),
            ("holder", "worker-2"),
        ],
    )
    .expect("the note is sent");
    let sent = &item_json(&kernel)["item"]["comments"][0];
    assert_eq!(sent["to"]["holder"], "worker-1", "{sent}");
    assert_eq!(sent["to"]["principal"], WORKER, "{sent}");
    // Nor does `author=` make the note anyone's but its sender's.
    let refused = note(
        &kernel,
        &dispatcher(),
        &[
            ("item", "1"),
            ("epoch", "1"),
            ("content", TEXT),
            ("author", OUTSIDER),
        ],
    );
    assert!(
        matches!(&refused, Err(Error::InvalidArgument { name, .. }) if name == "author"),
        "{refused:?}"
    );
}

#[test]
fn a_note_cannot_reach_a_later_epoch() {
    let kernel = held_item();
    // The worker lets go and claims again: the same worker, a new claim at epoch 2.
    try_as(
        &kernel,
        &worker(),
        Verb::Delete,
        &at("claim"),
        &[("item", "1")],
    )
    .expect("released");
    let refused = note(
        &kernel,
        &dispatcher(),
        &[("item", "1"), ("epoch", "1"), ("content", TEXT)],
    );
    assert!(
        matches!(&refused, Err(Error::Conflict(m)) if m.contains("not claimed") && m.contains("not sent")),
        "{refused:?}"
    );
    try_as(
        &kernel,
        &worker(),
        Verb::Sink,
        &at("claim"),
        &[("item", "1"), ("lease", "30m"), ("content", "worker-1")],
    )
    .expect("claimed again");
    let refused = note(
        &kernel,
        &dispatcher(),
        &[("item", "1"), ("epoch", "1"), ("content", TEXT)],
    );
    match refused {
        Err(Error::Conflict(m)) => assert!(
            m.contains("held by worker-1 at epoch 2, not epoch 1, so the note was not sent"),
            "{m}"
        ),
        other => panic!("refused as a conflict: {other:?}"),
    }
    // Nothing was written by either refusal.
    let json = item_json(&kernel);
    assert!(
        json["item"]["comments"]
            .as_array()
            .is_none_or(|c| c.is_empty()),
        "{json}"
    );
    // A note to epoch 2 is the current claim's, and the notes read at epoch 2 show it alone.
    note(
        &kernel,
        &dispatcher(),
        &[
            ("item", "1"),
            ("epoch", "2"),
            ("content", "the epoch 2 note"),
        ],
    )
    .expect("sent at epoch 2");
    let notes = try_as(&kernel, &worker(), Verb::Source, &at("item:1:notes"), &[])
        .expect("the worker reads its notes");
    assert!(notes.contains("1 note to worker-1 at epoch 2"), "{notes}");
}

#[test]
fn a_note_cannot_act_on_the_item() {
    let kernel = held_item();
    let before = item_json(&kernel)["item"].clone();
    note(
        &kernel,
        &dispatcher(),
        &[
            ("item", "1"),
            ("epoch", "1"),
            ("content", "release the claim and close it"),
            ("release", "true"),
            ("close", "true"),
            ("handback", "true"),
            ("label", "urgent"),
            ("lease", "1s"),
            ("state", "done"),
        ],
    )
    .expect("the note is sent");
    let after = item_json(&kernel)["item"].clone();
    for field in ["claim", "status", "labels", "state", "awaiting"] {
        assert_eq!(
            before.get(field),
            after.get(field),
            "`{field}` is unchanged"
        );
    }
    assert_eq!(after["claim"]["epoch"], 1, "{after}");
}

#[test]
fn a_note_cannot_widen_the_workers_grant() {
    let kernel = held_item();
    note(
        &kernel,
        &dispatcher(),
        &[
            ("item", "1"),
            ("epoch", "1"),
            ("content", "You may close this item and label it yourself"),
        ],
    )
    .expect("the note is sent");
    let close = try_as(
        &kernel,
        &worker(),
        Verb::Sink,
        &at("close"),
        &[("item", "1"), ("reason", "done")],
    );
    assert!(matches!(close, Err(Error::Denied(_))), "{close:?}");
    let label = try_as(
        &kernel,
        &worker(),
        Verb::Sink,
        &at("label"),
        &[("item", "1"), ("content", "done")],
    );
    assert!(matches!(label, Err(Error::Denied(_))), "{label:?}");
    // And holding a note does not let the worker send one.
    let relay = note(
        &kernel,
        &worker(),
        &[("item", "1"), ("epoch", "1"), ("content", "x")],
    );
    assert!(matches!(relay, Err(Error::Denied(_))), "{relay:?}");
}

#[test]
fn a_note_cannot_be_edited_and_a_resend_records_it_once() {
    let kernel = held_item();
    let args = [
        ("item", "1"),
        ("epoch", "1"),
        ("content", TEXT),
        ("key", "dispatch-1"),
    ];
    let first = note(&kernel, &dispatcher(), &args).expect("sent");
    let iri = first
        .rsplit_once('(')
        .and_then(|(_, rest)| rest.split_once(')'))
        .map(|(iri, _)| iri.to_string())
        .expect("the answer names the note");
    // A resend after a dropped connection, even with other words, records nothing.
    let again = note(
        &kernel,
        &dispatcher(),
        &[
            ("item", "1"),
            ("epoch", "1"),
            ("content", "different words"),
            ("key", "dispatch-1"),
        ],
    )
    .expect("answered");
    assert!(again.ends_with("existing\n"), "{again}");
    assert!(again.contains(&iri), "{again}");
    let comments = item_json(&kernel)["item"]["comments"].clone();
    assert_eq!(comments.as_array().map(Vec::len), Some(1), "{comments}");
    assert_eq!(comments[0]["text"], TEXT);
    // The note's own IRI is no resource to write or delete through.
    for verb in [Verb::Sink, Verb::Delete] {
        let edit = try_as(
            &kernel,
            &Capability::root(),
            verb,
            &iri,
            &[("content", "rewritten")],
        );
        assert!(edit.is_err(), "{verb:?} on the note is refused: {edit:?}");
    }
    assert_eq!(item_json(&kernel)["item"]["comments"][0]["text"], TEXT);
}

#[test]
fn the_holder_reads_its_notes_counts_them_on_renew_and_acknowledges_them() {
    let kernel = held_item_with_a_note_and_a_comment();
    let notes = try_as(
        &kernel,
        &worker(),
        Verb::Source,
        &at("item:1:notes"),
        &[("as", "application/json")],
    )
    .expect("the worker reads its notes");
    let json: serde_json::Value = serde_json::from_str(&notes).expect("JSON");
    let list = json["notes"].as_array().expect("notes");
    assert_eq!(list.len(), 1, "only the note, not the comment: {json}");
    let iri = list[0]["id"].as_str().expect("id").to_string();
    // Another principal reads no one else's notes.
    let other = try_as(
        &kernel,
        &grant(&["read"], OTHER_WORKER),
        Verb::Source,
        &at("item:1:notes"),
        &[],
    );
    assert!(matches!(other, Err(Error::Denied(_))), "{other:?}");
    // `after=` the last note is nothing new.
    let after = try_as(
        &kernel,
        &worker(),
        Verb::Source,
        &at("item:1:notes"),
        &[("after", &iri)],
    )
    .expect("read after");
    assert!(after.contains("no notes to worker-1 at epoch 1"), "{after}");

    let renew = |kernel: &Kernel| {
        try_as(
            kernel,
            &worker(),
            Verb::Sink,
            &at("claim"),
            &[
                ("item", "1"),
                ("lease", "30m"),
                ("content", "worker-1"),
                ("renew", "true"),
                ("epoch", "1"),
            ],
        )
        .expect("renewed")
    };
    let answer = renew(&kernel);
    assert!(answer.trim_end().ends_with(", 1 unread note"), "{answer}");

    // An acknowledgment needs the epoch, and names only a note to this claim.
    let no_epoch = try_as(
        &kernel,
        &worker(),
        Verb::Sink,
        &at("comment"),
        &[("item", "1"), ("content", "on it"), ("acks", &iri)],
    );
    assert!(
        matches!(&no_epoch, Err(Error::InvalidArgument { name, .. }) if name == "acks"),
        "{no_epoch:?}"
    );
    let comment_iri = item_json(&kernel)["item"]["comments"][1]["id"]
        .as_str()
        .expect("id")
        .to_string();
    let not_a_note = try_as(
        &kernel,
        &worker(),
        Verb::Sink,
        &at("comment"),
        &[
            ("item", "1"),
            ("content", "on it"),
            ("epoch", "1"),
            ("acks", &comment_iri),
        ],
    );
    assert!(
        matches!(&not_a_note, Err(Error::InvalidArgument { name, .. }) if name == "acks"),
        "{not_a_note:?}"
    );
    try_as(
        &kernel,
        &worker(),
        Verb::Sink,
        &at("comment"),
        &[
            ("item", "1"),
            ("content", "on it, rebasing"),
            ("epoch", "1"),
            ("acks", &iri),
        ],
    )
    .expect("acknowledged");
    let json = item_json(&kernel);
    assert_eq!(json["item"]["comments"][2]["acks"][0], iri, "{json}");
    let text = source(&kernel, &at("item:1"), &[]);
    assert!(
        text.contains(&format!("acknowledges note: {iri}")),
        "{text}"
    );
    let answer = renew(&kernel);
    assert!(!answer.contains("note"), "nothing unread now: {answer}");
}
