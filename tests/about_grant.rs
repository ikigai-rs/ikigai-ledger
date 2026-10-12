//! **The about grant** (ledger #1315): adding and removing an item's `ledger:about` edge under
//! a grant that carries nothing else.
//!
//! Until 0.10.0 `…:item:{id}:about` took `urn:cap:ledger:write:{ledger}` and nothing narrower,
//! and the write grant is also the close grant. So the collaboration layer's mirror, which
//! holds the worker grant (claim and comment, never close), could not record `about <offer>`
//! on the item it offered, and recorded a keyed comment instead. These tests hold the new
//! grant to what it says:
//!
//! - `urn:cap:ledger:about:{ledger}` adds and removes `about` edges, and nothing else;
//! - the worker grant does NOT gain it: whoever issues grants decides who may annotate;
//! - it names one ledger, like every grant before it;
//! - `urn:cap:ledger:write:{ledger}` still adds and removes them, so no grant changed meaning;
//! - the item Sink's own `about=` stays under write, because that door also edits the title,
//!   body and priority.

mod common;

use common::*;
use ikigai_core::{Capability, Error, Kernel, Verb};

const LEDGER: &str = "acme";
const TARGET: &str = "urn:layer:tasks:offer:01m4abc";

fn at(resource: &str) -> String {
    format!("urn:iki:ledger:{LEDGER}:{resource}")
}

fn store_grants() -> Vec<String> {
    vec![graph_read(LEDGER), graph_write(LEDGER)]
}

/// The worker grant `README.md` publishes: read, claim, comment, and the store grants.
fn worker() -> Capability {
    let mut scopes = vec![
        format!("urn:cap:ledger:read:{LEDGER}"),
        format!("urn:cap:ledger:claim:{LEDGER}"),
        format!("urn:cap:ledger:comment:{LEDGER}"),
    ];
    scopes.extend(store_grants());
    Capability::scoped(scopes)
}

/// The worker grant plus the about grant: what gonk's layer mirror would hold.
fn worker_with_about() -> Capability {
    let mut scopes = vec![
        format!("urn:cap:ledger:read:{LEDGER}"),
        format!("urn:cap:ledger:claim:{LEDGER}"),
        format!("urn:cap:ledger:comment:{LEDGER}"),
        format!("urn:cap:ledger:about:{LEDGER}"),
    ];
    scopes.extend(store_grants());
    Capability::scoped(scopes)
}

/// The about grant and a read, nothing else at the ledger.
fn annotator() -> Capability {
    let mut scopes = vec![
        format!("urn:cap:ledger:read:{LEDGER}"),
        format!("urn:cap:ledger:about:{LEDGER}"),
    ];
    scopes.extend(store_grants());
    Capability::scoped(scopes)
}

fn writer() -> Capability {
    let mut scopes = vec![
        format!("urn:cap:ledger:read:{LEDGER}"),
        format!("urn:cap:ledger:write:{LEDGER}"),
    ];
    scopes.extend(store_grants());
    Capability::scoped(scopes)
}

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

/// Whether the item is `about` the target, asked of the graph rather than of an answer.
fn is_about(kernel: &Kernel, target: &str) -> bool {
    let json: serde_json::Value = serde_json::from_str(&select(
        kernel,
        &format!(
            "SELECT ?i WHERE {{ GRAPH <urn:iki:ledger:graph:{LEDGER}> {{ \
             ?i <{about}> <{target}> }} }}",
            about = ikigai_ledger::vocabulary::ABOUT
        ),
    ))
    .expect("select answers JSON");
    !json["results"]["bindings"]
        .as_array()
        .expect("bindings")
        .is_empty()
}

/// ★ The defect: the worker grant cannot record what an item is about. That stays true —
/// the worker grant does not gain the about grant — and the next test is the fix.
#[test]
fn the_worker_grant_alone_still_cannot_add_or_remove_an_about() {
    let kernel = ledger_with_an_item();
    let w = worker();
    denied(
        try_as(
            &kernel,
            &w,
            Verb::Sink,
            &at("item:1:about"),
            &[("content", TARGET)],
        ),
        "a worker adding an about",
    );
    sink(&kernel, &at("item:1:about"), &[("content", TARGET)]);
    denied(
        try_as(
            &kernel,
            &w,
            Verb::Delete,
            &at("item:1:about"),
            &[("content", TARGET)],
        ),
        "a worker removing an about",
    );
    assert!(is_about(&kernel, TARGET));
}

/// ★ The fix: the worker grant plus `about:{ledger}` adds and removes an about, and still
/// cannot close.
#[test]
fn the_about_grant_adds_and_removes_an_about() {
    let kernel = ledger_with_an_item();
    let w = worker_with_about();
    let added = try_as(
        &kernel,
        &w,
        Verb::Sink,
        &at("item:1:about"),
        &[("content", TARGET)],
    )
    .expect("the about grant adds an about");
    assert!(added.contains(TARGET), "{added}");
    assert!(is_about(&kernel, TARGET));
    try_as(
        &kernel,
        &w,
        Verb::Delete,
        &at("item:1:about"),
        &[("content", TARGET)],
    )
    .expect("the about grant removes an about");
    assert!(!is_about(&kernel, TARGET));
    denied(
        try_as(&kernel, &w, Verb::Sink, &at("close"), &[("item", "1")]),
        "a worker with the about grant closing",
    );
}

/// The about grant carries nothing else: no close, comment, claim, label, edit, append —
/// and not the item Sink's own `about=`, which is the edit door.
#[test]
fn the_about_grant_reaches_nothing_else() {
    let kernel = ledger_with_an_item();
    let a = annotator();
    denied(
        try_as(&kernel, &a, Verb::Sink, &at("close"), &[("item", "1")]),
        "close",
    );
    denied(
        try_as(
            &kernel,
            &a,
            Verb::Sink,
            &at("comment"),
            &[("item", "1"), ("content", "hi")],
        ),
        "comment",
    );
    denied(
        try_as(
            &kernel,
            &a,
            Verb::Sink,
            &at("claim"),
            &[("item", "1"), ("content", "me")],
        ),
        "claim",
    );
    denied(
        try_as(
            &kernel,
            &a,
            Verb::Sink,
            &at("label"),
            &[("item", "1"), ("content", "urgent")],
        ),
        "label",
    );
    denied(
        try_as(&kernel, &a, Verb::Sink, &at("item:1"), &[("about", TARGET)]),
        "the item Sink's about=",
    );
    denied(
        try_as(
            &kernel,
            &a,
            Verb::Sink,
            &at("append"),
            &[("content", "A new item")],
        ),
        "append",
    );
    assert!(!is_about(&kernel, TARGET));
    try_as(
        &kernel,
        &a,
        Verb::Sink,
        &at("item:1:about"),
        &[("content", TARGET)],
    )
    .expect("an annotator adds an about");
}

/// The about grant names ONE ledger: `about:acme` is refused at `bosatsu`.
#[test]
fn the_about_grant_names_one_ledger() {
    let kernel = kernel();
    sink(
        &kernel,
        "urn:iki:ledger:bosatsu:append",
        &[("content", "Elsewhere")],
    );
    let mut scopes = vec![
        format!("urn:cap:ledger:about:{LEDGER}"),
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
            "urn:iki:ledger:bosatsu:item:1:about",
            &[("content", TARGET)],
        ),
        "about at another ledger",
    );
}

/// ★ `write:{ledger}` still adds and removes an about: no grant issued before lost anything.
#[test]
fn the_write_grant_still_adds_and_removes_an_about() {
    let kernel = ledger_with_an_item();
    let w = writer();
    try_as(
        &kernel,
        &w,
        Verb::Sink,
        &at("item:1:about"),
        &[("content", TARGET)],
    )
    .expect("write adds an about");
    assert!(is_about(&kernel, TARGET));
    try_as(
        &kernel,
        &w,
        Verb::Delete,
        &at("item:1:about"),
        &[("content", TARGET)],
    )
    .expect("write removes an about");
    assert!(!is_about(&kernel, TARGET));
}

/// The refusal names both tokens that would do, so an operator reading it knows what to
/// grant.
#[test]
fn the_refusal_names_both_grants() {
    let kernel = ledger_with_an_item();
    let refused = try_as(
        &kernel,
        &worker(),
        Verb::Sink,
        &at("item:1:about"),
        &[("content", TARGET)],
    );
    let Err(Error::Denied(why)) = refused else {
        panic!("expected Denied, got {refused:?}");
    };
    assert!(
        why.contains(&format!("urn:cap:ledger:about:{LEDGER}")),
        "{why}"
    );
    assert!(
        why.contains(&format!("urn:cap:ledger:write:{LEDGER}")),
        "{why}"
    );
}
