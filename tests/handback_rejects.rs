//! **Which hand-back a reject answers** (ledger #3071). Until this the ledger cleared
//! `ledger:awaiting` and said no more, so anyone who may comment could post a new hand-back
//! after a reject, and a reader of the JSON could not tell the rejected evidence from evidence
//! nobody had answered: gonk's `findings` filed the rejected worker's block, and `--close` would
//! have closed on it.
//!
//! Now the comment that takes an item off `awaiting` names every hand-back it answered
//! (`ledger:rejects`), in the update that clears it, and each face shows them.

mod common;

use common::*;
use ikigai_core::{Capability, Kernel, Verb};
use serde_json::{json, Value};

const LEDGER: &str = "acme";
const LIVE: &str = "urn:iki:ledger:graph:acme";
const NS: &str = "https://ikigai-rs.dev/ns/ledger#";

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

fn worker() -> Capability {
    grant(&["read", "claim", "comment"])
}

fn dispatcher() -> Capability {
    grant(&["read", "claim", "comment", "close"])
}

fn ledger_with_an_item() -> Kernel {
    let kernel = kernel();
    sink(&kernel, &at("append"), &[("content", "Fix the widget")]);
    kernel
}

/// A comment on #1, answered in JSON, as the answer's document.
fn comment(kernel: &Kernel, who: &Capability, args: &[(&str, &str)]) -> Value {
    let mut all = vec![("item", "1"), ("as", "application/json")];
    all.extend_from_slice(args);
    let answer = try_as(kernel, who, Verb::Sink, &at("comment"), &all).unwrap();
    serde_json::from_str(&answer).expect("JSON")
}

/// A comment on #1, and its IRI.
fn said(kernel: &Kernel, who: &Capability, args: &[(&str, &str)]) -> String {
    comment(kernel, who, args)["comment"]["id"]
        .as_str()
        .expect("a comment's id")
        .to_string()
}

fn hand_back(kernel: &Kernel, text: &str) -> String {
    said(
        kernel,
        &worker(),
        &[("content", text), ("handback", "true")],
    )
}

fn item_comments(kernel: &Kernel) -> Vec<Value> {
    let view = source(kernel, &at("item:1"), &[("as", "application/json")]);
    let doc: Value = serde_json::from_str(&view).expect("JSON");
    doc["item"]["comments"]
        .as_array()
        .expect("comments")
        .clone()
}

/// The `rejects` of the comment `iri` on the item's JSON face; `None` when it has no field.
fn rejects_of(kernel: &Kernel, iri: &str) -> Option<Value> {
    item_comments(kernel)
        .into_iter()
        .find(|c| c["id"] == iri)
        .expect("the comment is on the item")
        .get("rejects")
        .cloned()
}

/// ★ THE REPRODUCTION. A reject names the hand-back it rejected, in its own answer and on the
/// item's JSON, so a reader can tell the rejected evidence from the hand-back posted after it.
#[test]
fn a_reject_records_the_handback_it_rejected_and_the_json_shows_it() {
    let kernel = ledger_with_an_item();
    let first = hand_back(&kernel, "https://github.com/x/y/pull/1 CI green");
    let answer = comment(
        &kernel,
        &dispatcher(),
        &[("content", "CI is red on main"), ("rejects", "true")],
    );
    assert_eq!(answer["handback"], "cleared", "{answer}");
    assert_eq!(answer["comment"]["rejects"], json!([first]), "{answer}");
    let reject = answer["comment"]["id"].as_str().unwrap().to_string();
    assert_eq!(rejects_of(&kernel, &reject), Some(json!([first])));

    // The gap the item names: a new hand-back marks the item awaiting again, and the JSON still
    // says the first is the rejected one and this one is not.
    let second = hand_back(&kernel, "the same evidence, posted again");
    let doc = source(&kernel, &at("item:1"), &[("as", "application/json")]);
    let doc: Value = serde_json::from_str(&doc).unwrap();
    assert_eq!(doc["item"]["awaiting"], "dispatcher");
    let rejected: Vec<Value> = item_comments(&kernel)
        .iter()
        .flat_map(|c| c["rejects"].as_array().cloned().unwrap_or_default())
        .collect();
    assert_eq!(rejected, [json!(first)]);
    assert!(!rejected.contains(&json!(second)));
    assert_eq!(
        rejects_of(&kernel, &first),
        None,
        "a hand-back rejects nothing"
    );
}

/// The listing carries it too, and a reject of two hand-backs is still one comment there.
#[test]
fn a_reject_of_every_standing_handback_is_one_comment_on_every_json_face() {
    let kernel = ledger_with_an_item();
    let real = hand_back(&kernel, "a worker's evidence");
    let later = hand_back(&kernel, "someone else's, on top of it");
    let reject = said(
        &kernel,
        &dispatcher(),
        &[("content", "no"), ("rejects", "true")],
    );
    let mut both = vec![real.clone(), later.clone()];
    both.sort();
    assert_eq!(rejects_of(&kernel, &reject), Some(json!(both)));
    assert_eq!(item_comments(&kernel).len(), 3);

    let listing = source(&kernel, &at("items"), &[("as", "application/json")]);
    let listing: Value = serde_json::from_str(&listing).unwrap();
    let comments = listing["items"][0]["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 3, "{listing}");
    assert_eq!(comments[2]["id"], reject.as_str());
    assert_eq!(comments[2]["rejects"], json!(both));
}

/// A later reject answers only what the earlier one had not.
#[test]
fn a_second_reject_names_only_the_handback_since_the_first() {
    let kernel = ledger_with_an_item();
    let first = hand_back(&kernel, "one");
    let no = said(
        &kernel,
        &worker(),
        &[("content", "no"), ("rejects", "true")],
    );
    let second = hand_back(&kernel, "two");
    let again = said(
        &kernel,
        &worker(),
        &[("content", "no"), ("rejects", "true")],
    );
    assert_eq!(rejects_of(&kernel, &no), Some(json!([first])));
    assert_eq!(rejects_of(&kernel, &again), Some(json!([second])));
}

/// ★ The dispatcher's own comment answers a hand-back "without being asked", so it records what
/// it answered exactly as `rejects=true` does. Otherwise a reject said in plain words would
/// leave the hole this closes.
#[test]
fn a_dispatchers_ordinary_comment_records_the_handback_it_cleared() {
    let kernel = ledger_with_an_item();
    let handback = hand_back(&kernel, "evidence");
    let answer = said(&kernel, &dispatcher(), &[("content", "this does not hold")]);
    assert_eq!(rejects_of(&kernel, &answer), Some(json!([handback])));
}

/// A comment that takes nothing off `awaiting` answers nothing, whoever wrote it and whatever
/// it says: the older hand-back stays unanswered until a reject lands on a waiting item.
#[test]
fn a_comment_that_clears_nothing_records_nothing() {
    let kernel = ledger_with_an_item();
    let quiet = said(&kernel, &dispatcher(), &[("content", "nothing to answer")]);
    let empty = said(
        &kernel,
        &worker(),
        &[("content", "no"), ("rejects", "true")],
    );
    let handback = hand_back(&kernel, "evidence");
    let note = said(&kernel, &worker(), &[("content", "a worker's note")]);
    for iri in [&quiet, &empty, &handback, &note] {
        assert_eq!(rejects_of(&kernel, iri), None, "{iri}");
    }
    let answered = said(&kernel, &dispatcher(), &[("content", "no")]);
    let after = said(&kernel, &dispatcher(), &[("content", "and another thing")]);
    assert_eq!(rejects_of(&kernel, &answered), Some(json!([handback])));
    assert_eq!(rejects_of(&kernel, &after), None);
}

/// A close clears a hand-back too, and it is no reject: it does not say the evidence failed.
#[test]
fn a_close_records_no_reject() {
    let kernel = ledger_with_an_item();
    hand_back(&kernel, "evidence");
    sink(
        &kernel,
        &at("close"),
        &[("item", "1"), ("reason", "wontfix"), ("content", "fine")],
    );
    let rejected: Vec<Value> = item_comments(&kernel)
        .into_iter()
        .filter_map(|c| c.get("rejects").cloned())
        .collect();
    assert!(rejected.is_empty(), "{rejected:?}");
}

/// ★ An item with no reject is unchanged: no `rejects` key on any comment's JSON, not `null`
/// and not `[]`, and the plain comment block byte for byte as it was.
#[test]
fn an_item_with_no_reject_is_unchanged() {
    let kernel = ledger_with_an_item();
    let handback = hand_back(&kernel, "evidence");
    for comment in item_comments(&kernel) {
        let keys: Vec<&str> = comment
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, ["author", "id", "text", "time"], "{comment}");
    }
    let answer = comment(&kernel, &worker(), &[("content", "a note")]);
    assert!(answer["comment"].get("rejects").is_none(), "{answer}");
    let plain = source(&kernel, &at("item:1"), &[]);
    assert!(!plain.contains("rejects"), "{plain}");
    let turtle = source(&kernel, &at("item:1"), &[("as", "text/turtle")]);
    assert!(!turtle.contains("rejects"), "{turtle}");
    assert_eq!(rejects_of(&kernel, &handback), None);
}

/// The plain face says it on a labeled line of its own, indented less than the comment's text,
/// so no comment's words can pass for it; Turtle carries the triple.
#[test]
fn the_plain_and_turtle_faces_name_the_rejected_handback() {
    let kernel = ledger_with_an_item();
    let handback = hand_back(&kernel, "evidence");
    let reject = said(
        &kernel,
        &dispatcher(),
        &[("content", "  rejects: not this one"), ("rejects", "true")],
    );
    let plain = source(&kernel, &at("item:1"), &[]);
    assert!(
        plain.contains(&format!("\n  rejects: {handback}\n")),
        "{plain}"
    );
    assert_eq!(plain.matches("\n  rejects: ").count(), 1, "{plain}");
    let turtle = source(&kernel, &at("item:1"), &[("as", "text/turtle")]);
    assert!(
        turtle.contains(&format!("ledger:rejects <{handback}>")),
        "{turtle}"
    );
    assert!(turtle.contains(&reject), "{turtle}");
}

/// A keyed reject records once, and its replay answers the same comment with the same names.
#[test]
fn a_keyed_reject_replayed_names_what_it_rejected() {
    let kernel = ledger_with_an_item();
    let handback = hand_back(&kernel, "evidence");
    let args = [("content", "no"), ("rejects", "true"), ("key", "reject-1")];
    let first = comment(&kernel, &dispatcher(), &args);
    assert_eq!(first["comment"]["rejects"], json!([handback]), "{first}");
    let replay = comment(&kernel, &dispatcher(), &args);
    assert_eq!(replay["outcome"], "existing", "{replay}");
    assert_eq!(replay["comment"]["rejects"], json!([handback]), "{replay}");

    // A key already taken writes nothing, so a replay cannot reject a later hand-back.
    let later = hand_back(&kernel, "more evidence");
    comment(&kernel, &dispatcher(), &args);
    let view = source(&kernel, &at("item:1"), &[("as", "application/json")]);
    let view: Value = serde_json::from_str(&view).unwrap();
    assert_eq!(view["item"]["awaiting"], "dispatcher");
    let rejected: Vec<Value> = item_comments(&kernel)
        .iter()
        .flat_map(|c| c["rejects"].as_array().cloned().unwrap_or_default())
        .collect();
    assert!(!rejected.contains(&json!(later)), "{rejected:?}");
}

/// A hand-back on an item nobody holds has no `handedBackBy`, and is still a hand-back.
#[test]
fn a_reject_names_a_handback_of_an_unclaimed_item() {
    let kernel = ledger_with_an_item();
    let handback = hand_back(&kernel, "evidence");
    let holders = select(
        &kernel,
        &format!("SELECT ?by WHERE {{ GRAPH <{LIVE}> {{ ?c <{NS}handedBackBy> ?by }} }}"),
    );
    assert!(!holders.contains("\"by\":"), "{holders}");
    let reject = said(
        &kernel,
        &worker(),
        &[("content", "no"), ("rejects", "true")],
    );
    assert_eq!(rejects_of(&kernel, &reject), Some(json!([handback])));
}

/// ★ A hand-back written before this names a holder and carries no `ledger:handBack`. One still
/// awaiting when the ledger is upgraded is what a reject most needs to find, so the holder
/// counts as the mark. Written here as such a hand-back was, by taking the new mark off.
#[test]
fn a_reject_names_a_handback_written_before_the_mark() {
    let kernel = ledger_with_an_item();
    try_as(
        &kernel,
        &worker(),
        Verb::Sink,
        &at("claim"),
        &[("item", "1"), ("content", "worker-1")],
    )
    .unwrap();
    let handback = hand_back(&kernel, "evidence");
    sink(
        &kernel,
        "urn:iki:store:update",
        &[(
            "content",
            &format!("DELETE WHERE {{ GRAPH <{LIVE}> {{ ?c <{NS}handBack> ?mark }} }}"),
        )],
    );
    let reject = said(
        &kernel,
        &dispatcher(),
        &[("content", "no"), ("rejects", "true")],
    );
    assert_eq!(rejects_of(&kernel, &reject), Some(json!([handback])));
}
