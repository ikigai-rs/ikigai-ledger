//! **An edit event names what it changed** (ledger #2841): the label, the states it moved
//! between, the link and its target, the about target, the names of the fields.
//!
//! Since 0.15.0 a `ledger:EditEvent` recorded the kind of an edit, its committer and its time,
//! but not its object, and the state and the labels are replaced in place, so "when did this
//! item leave that state" and "when was that label removed" had no answer even for new work.
//! The object now rides the update that makes the change. Each test reads the event back from
//! the store. The README's two queries are run here as they are printed there.

mod common;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Poll;

use async_trait::async_trait;
use common::*;
use futures::executor::block_on;
use ikigai_core::{
    ArgRef, Capability, Endpoint, Error, Fallback, Invocation, Iri, Kernel, Representation,
    Request, Resolution, Scope, Space, Verb,
};
use ikigai_store::DurableStore;

const LEDGER: &str = "acme";
const LIVE: &str = "urn:iki:ledger:graph:acme";
const NS: &str = "https://ikigai-rs.dev/ns/ledger#";
const CREATED: &str = "http://purl.org/dc/terms/created";
const LIFECYCLE: &str = "urn:iki:ledger:lifecycle:kata-flight";

fn at(resource: &str) -> String {
    format!("urn:iki:ledger:{LEDGER}:{resource}")
}

fn state(name: &str) -> String {
    format!("{LIFECYCLE}:{name}")
}

fn kernel() -> Kernel {
    let kernel = common::kernel();
    sink(&kernel, &at("append"), &[("content", "Fix the widget")]);
    sink(&kernel, &at("append"), &[("content", "Another")]);
    kernel
}

fn rows(kernel: &Kernel, query: &str) -> Vec<BTreeMap<String, String>> {
    let json: serde_json::Value =
        serde_json::from_str(&select(kernel, query)).expect("the store answers JSON");
    json["results"]["bindings"]
        .as_array()
        .expect("bindings")
        .iter()
        .map(|row| {
            row.as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), v["value"].as_str().unwrap().to_string()))
                .collect()
        })
        .collect()
}

fn item_iri(kernel: &Kernel, number: i64) -> String {
    rows(
        kernel,
        &format!("SELECT ?i WHERE {{ GRAPH <{LIVE}> {{ ?i <{NS}number> {number} }} }}"),
    )[0]["i"]
        .clone()
}

/// One edit event: its kind, and each `ledger:` object term it carries, values sorted.
#[derive(Debug, PartialEq)]
struct Recorded {
    kind: String,
    object: BTreeMap<String, Vec<String>>,
}

/// Every edit event on item `#number` of [`LIVE`], oldest first.
fn edits(kernel: &Kernel, number: i64) -> Vec<Recorded> {
    edits_in(kernel, LIVE, number)
}

fn edits_in(kernel: &Kernel, graph: &str, number: i64) -> Vec<Recorded> {
    let mut by_event: BTreeMap<(String, String), BTreeMap<String, Vec<String>>> = BTreeMap::new();
    for row in rows(
        kernel,
        &format!(
            "SELECT ?e ?at ?p ?o WHERE {{ GRAPH <{graph}> {{ \
             ?item <{NS}number> {number} . \
             ?e a <{NS}EditEvent> ; <{NS}onItem> ?item ; <{CREATED}> ?at ; ?p ?o }} }}"
        ),
    ) {
        let terms = by_event
            .entry((row["at"].clone(), row["e"].clone()))
            .or_default();
        if let Some(name) = row["p"].strip_prefix(NS) {
            terms
                .entry(name.to_string())
                .or_default()
                .push(row["o"].clone());
        }
    }
    by_event
        .into_values()
        .map(|mut terms| {
            let kind = terms.remove("editKind").unwrap().remove(0);
            for bookkeeping in ["onItem", "committer", "committedAt"] {
                terms.remove(bookkeeping);
            }
            for values in terms.values_mut() {
                values.sort();
            }
            Recorded {
                kind,
                object: terms,
            }
        })
        .collect()
}

fn object(pairs: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
    pairs
        .iter()
        .map(|(k, vs)| (k.to_string(), vs.iter().map(|v| v.to_string()).collect()))
        .collect()
}

fn last(kernel: &Kernel, number: i64) -> Recorded {
    edits(kernel, number).pop().expect("an edit event")
}

// ------------------------------------------------------------------- one test per kind

#[test]
fn a_label_names_the_label() {
    let kernel = kernel();
    sink(
        &kernel,
        &at("label"),
        &[("item", "1"), ("content", "needs#triage")],
    );
    assert_eq!(
        last(&kernel, 1),
        Recorded {
            kind: "label".to_string(),
            object: object(&[("editLabel", &["needs#triage"])]),
        }
    );
}

#[test]
fn an_unlabel_names_the_label() {
    let kernel = kernel();
    sink(&kernel, &at("label"), &[("item", "1"), ("content", "bug")]);
    try_verb(
        &kernel,
        Verb::Delete,
        &at("label"),
        &[("item", "1"), ("content", "bug")],
    )
    .unwrap();
    assert_eq!(
        last(&kernel, 1),
        Recorded {
            kind: "unlabel".to_string(),
            object: object(&[("editLabel", &["bug"])]),
        }
    );
}

#[test]
fn a_state_change_names_the_state_before_and_after_as_iris() {
    let kernel = kernel();
    for (from, to) in [("filed", "queued"), ("queued", "filed")] {
        sink(&kernel, &at("item:1:state"), &[("from", from), ("to", to)]);
        assert_eq!(
            last(&kernel, 1),
            Recorded {
                kind: "state".to_string(),
                object: object(&[("stateFrom", &[&state(from)]), ("stateTo", &[&state(to)])]),
            }
        );
    }
}

/// ★ A link names its type and the OTHER item, on the subject's event only.
#[test]
fn a_link_names_its_type_and_the_other_item() {
    let kernel = kernel();
    let other = item_iri(&kernel, 2);
    let blocks = format!("{NS}blocks");
    sink(
        &kernel,
        &at("link"),
        &[("item", "1"), ("content", "2"), ("type", "blocks")],
    );
    let linked = Recorded {
        kind: "link".to_string(),
        object: object(&[("editLinkType", &[&blocks]), ("editTarget", &[&other])]),
    };
    assert_eq!(last(&kernel, 1), linked);
    assert!(edits(&kernel, 2).is_empty(), "the other item has no event");
}

#[test]
fn an_unlink_names_its_type_and_the_other_item() {
    let kernel = kernel();
    let other = item_iri(&kernel, 2);
    let related = format!("{NS}related");
    let link = [("item", "1"), ("content", "2"), ("type", "related")];
    sink(&kernel, &at("link"), &link);
    try_verb(&kernel, Verb::Delete, &at("link"), &link).unwrap();
    assert_eq!(
        last(&kernel, 1),
        Recorded {
            kind: "unlink".to_string(),
            object: object(&[("editLinkType", &[&related]), ("editTarget", &[&other])]),
        }
    );
}

#[test]
fn an_about_names_every_target() {
    let kernel = kernel();
    sink(
        &kernel,
        &at("item:1:about"),
        &[("content", "urn:example:a urn:example:b")],
    );
    assert_eq!(
        last(&kernel, 1),
        Recorded {
            kind: "about".to_string(),
            object: object(&[("editTarget", &["urn:example:a", "urn:example:b"])]),
        }
    );
}

#[test]
fn an_unabout_names_the_target() {
    let kernel = kernel();
    sink(
        &kernel,
        &at("item:1:about"),
        &[("content", "urn:example:a urn:example:b")],
    );
    try_verb(
        &kernel,
        Verb::Delete,
        &at("item:1:about"),
        &[("content", "urn:example:a")],
    )
    .unwrap();
    assert_eq!(
        last(&kernel, 1),
        Recorded {
            kind: "unabout".to_string(),
            object: object(&[("editTarget", &["urn:example:a"])]),
        }
    );
}

/// ★ An edit names the fields that changed, and no value of any of them.
#[test]
fn an_edit_names_the_fields_and_never_their_values() {
    let kernel = kernel();
    sink(
        &kernel,
        &at("item:1"),
        &[("content", "A SECRET TITLE\n\nA SECRET BODY")],
    );
    assert_eq!(
        last(&kernel, 1),
        Recorded {
            kind: "edit".to_string(),
            object: object(&[("editField", &["body", "title"])]),
        }
    );
    let everything = select(
        &kernel,
        &format!("SELECT ?p ?o WHERE {{ GRAPH <{LIVE}> {{ ?e a <{NS}EditEvent> ; ?p ?o }} }}"),
    );
    assert!(!everything.contains("SECRET"), "{everything}");
}

/// Only what changed: the same title under a new body names the body alone, and an item
/// filed with no body that is only retitled did not change its body.
#[test]
fn an_edit_names_only_the_fields_it_changed() {
    let kernel = kernel();
    sink(&kernel, &at("item:1"), &[("content", "Retitled")]);
    assert_eq!(
        last(&kernel, 1).object,
        object(&[("editField", &["title"])])
    );
    sink(&kernel, &at("item:1"), &[("content", "Retitled\n\nA body")]);
    assert_eq!(last(&kernel, 1).object, object(&[("editField", &["body"])]));
    sink(&kernel, &at("item:1"), &[("priority", "1")]);
    assert_eq!(
        last(&kernel, 1).object,
        object(&[("editField", &["priority"])])
    );
    sink(&kernel, &at("item:1"), &[("revision", "abc123")]);
    assert_eq!(
        last(&kernel, 1).object,
        object(&[("editField", &["revision"])])
    );
    sink(&kernel, &at("item:1"), &[("about", "urn:example:a")]);
    assert_eq!(
        last(&kernel, 1).object,
        object(&[("editField", &["about"])])
    );
}

/// ★ A write that changed nothing keeps its event (every write has a who) and names no object,
/// so "when was this label removed" is not answered by the removal of a label never there.
#[test]
fn a_write_that_changed_nothing_names_no_object() {
    let kernel = kernel();
    let item = at("item:1");
    let label = [("item", "1"), ("content", "bug")];
    try_verb(&kernel, Verb::Delete, &at("label"), &label).unwrap();
    assert_eq!(last(&kernel, 1).kind, "unlabel");
    assert_eq!(last(&kernel, 1).object, object(&[]));

    sink(&kernel, &at("label"), &label);
    sink(&kernel, &at("label"), &label);
    assert_eq!(last(&kernel, 1).kind, "label");
    assert_eq!(last(&kernel, 1).object, object(&[]));

    let link = [("item", "1"), ("content", "2"), ("type", "blocks")];
    try_verb(&kernel, Verb::Delete, &at("link"), &link).unwrap();
    assert_eq!(last(&kernel, 1).kind, "unlink");
    assert_eq!(last(&kernel, 1).object, object(&[]));
    sink(&kernel, &at("link"), &link);
    sink(&kernel, &at("link"), &link);
    assert_eq!(last(&kernel, 1).kind, "link");
    assert_eq!(last(&kernel, 1).object, object(&[]));

    sink(
        &kernel,
        &at("item:1:about"),
        &[("content", "urn:example:a urn:example:a")],
    );
    sink(
        &kernel,
        &at("item:1:about"),
        &[("content", "urn:example:a urn:example:b")],
    );
    assert_eq!(
        last(&kernel, 1).object,
        object(&[("editTarget", &["urn:example:b"])])
    );

    sink(&kernel, &item, &[("content", "Fix the widget")]);
    assert_eq!(last(&kernel, 1).kind, "edit");
    assert_eq!(last(&kernel, 1).object, object(&[]));
}

/// A link refused because its object is gone records nothing about that object.
#[test]
fn a_link_to_a_deleted_item_records_nothing() {
    let kernel = kernel();
    let other = item_iri(&kernel, 2);
    try_verb(&kernel, Verb::Delete, &at("item:2"), &[]).unwrap();
    assert!(try_verb(
        &kernel,
        Verb::Sink,
        &at("link"),
        &[("item", "1"), ("content", &other), ("type", "related")],
    )
    .is_err());
    assert_eq!(edits(&kernel, 1), []);
}

// ---------------------------------------------------------------- what outlives a change

/// ★ A delete takes every edge pointing at its item, and an event's `editTarget` is not an
/// edge: deleting the OTHER item must not rewrite this item's history.
#[test]
fn deleting_the_other_item_keeps_the_history_that_names_it() {
    let kernel = kernel();
    let other = item_iri(&kernel, 2);
    sink(
        &kernel,
        &at("link"),
        &[("item", "1"), ("content", "2"), ("type", "related")],
    );
    try_verb(&kernel, Verb::Delete, &at("item:2"), &[]).unwrap();
    assert_eq!(
        last(&kernel, 1).object["editTarget"],
        std::slice::from_ref(&other),
        "the event still names the deleted item"
    );
    let edge = rows(
        &kernel,
        &format!("SELECT ?s WHERE {{ GRAPH <{LIVE}> {{ ?s <{NS}related> <{other}> }} }}"),
    );
    assert!(edge.is_empty(), "the edge went with it: {edge:?}");
}

/// A delete still owns the events hanging off its own item, with their objects.
#[test]
fn deleting_the_item_takes_its_events_and_their_objects() {
    let kernel = kernel();
    sink(&kernel, &at("label"), &[("item", "1"), ("content", "a#b")]);
    sink(
        &kernel,
        &at("item:1:state"),
        &[("from", "filed"), ("to", "queued")],
    );
    let iri = item_iri(&kernel, 1);
    try_verb(&kernel, Verb::Delete, &at("item:1"), &[]).unwrap();
    let left = rows(
        &kernel,
        &format!(
            "SELECT ?s ?p ?o WHERE {{ GRAPH <{LIVE}> {{ ?s ?p ?o . \
             FILTER(STRSTARTS(STR(?s), \"{iri}\") && ?p != <{NS}deletedItem>) }} }}"
        ),
    );
    assert!(left.is_empty(), "{left:?}");
}

// ------------------------------------------------------------------------- the forced race

/// Holds the next `width` live-graph updates until all have arrived, so every transition has
/// read the item in its `from` before any writes.
#[derive(Default)]
struct Gate {
    gated: Mutex<usize>,
    arrived: AtomicUsize,
    width: AtomicUsize,
}

struct Gated {
    inner: Arc<dyn Space>,
    gate: Arc<Gate>,
}

struct GatedEndpoint {
    inner: Arc<dyn Endpoint>,
    gate: Arc<Gate>,
}

impl Space for Gated {
    fn resolve(&self, request: &Request, scope: &Scope) -> Resolution {
        let gate = Arc::clone(&self.gate);
        self.inner
            .resolve(request, scope)
            .map_endpoint(move |inner| Arc::new(GatedEndpoint { inner, gate }) as Arc<dyn Endpoint>)
    }
}

#[async_trait]
impl Endpoint for GatedEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> ikigai_core::Result<Representation> {
        let to_live = inv.request.target.as_str() == "urn:iki:store:graph-update"
            && inv.inline_str("graph").map(|g| g == LIVE).unwrap_or(false);
        if to_live {
            let held = {
                let mut gated = self.gate.gated.lock().unwrap();
                let held = *gated > 0;
                *gated = gated.saturating_sub(1);
                held
            };
            if held {
                self.gate.arrived.fetch_add(1, Ordering::SeqCst);
                let width = self.gate.width.load(Ordering::SeqCst);
                futures::future::poll_fn(|cx| {
                    if self.gate.arrived.load(Ordering::SeqCst) >= width {
                        Poll::Ready(())
                    } else {
                        cx.waker().wake_by_ref();
                        Poll::Pending
                    }
                })
                .await;
            }
        }
        self.inner.invoke(inv).await
    }

    fn name(&self) -> &str {
        self.inner.name()
    }

    fn describe(&self) -> ikigai_core::Description {
        self.inner.describe()
    }
}

fn moved(from: &str, to: &str) -> Request {
    [("from", from), ("to", to)].iter().fold(
        Request::new(Verb::Sink, Iri::parse(at("item:1:state")).unwrap()),
        |r, (name, value)| r.with_arg(*name, ArgRef::Inline(value.as_bytes().to_vec())),
    )
}

/// ★ Two state moves forced to race, each having read the item in `queued`: one wins, and the
/// events are one unbroken chain from `filed` to where the item is now.
#[test]
fn two_concurrent_state_moves_leave_a_consistent_chain() {
    let gate = Arc::new(Gate::default());
    let store = Arc::new(ikigai_store::space(DurableStore::in_memory().unwrap())) as Arc<dyn Space>;
    let space = Fallback::new(vec![
        Arc::new(Gated {
            inner: store,
            gate: Arc::clone(&gate),
        }) as Arc<dyn Space>,
        Arc::new(ikigai_ledger::space()) as Arc<dyn Space>,
    ]);
    let kernel =
        Kernel::with_meta_renderer(Arc::new(space), Arc::new(ikigai_vocab::TurtleRenderer))
            .with_clock(Arc::new(TickingClock::default()));
    sink(&kernel, &at("append"), &[("content", "Contended")]);
    sink(
        &kernel,
        &at("item:1:state"),
        &[("from", "filed"), ("to", "queued")],
    );

    gate.width.store(2, Ordering::SeqCst);
    *gate.gated.lock().unwrap() = 2;
    let root = Capability::root();
    let answers = block_on(futures::future::join(
        kernel.issue(moved("queued", "reviewed"), &root),
        kernel.issue(moved("queued", "refining"), &root),
    ));
    assert_eq!(
        gate.arrived.load(Ordering::SeqCst),
        2,
        "both moves reached their update before either wrote"
    );
    let won = [answers.0.is_ok(), answers.1.is_ok()];
    assert_eq!(
        won.iter().filter(|w| **w).count(),
        1,
        "one move wins: {won:?}"
    );
    assert!(matches!(
        if won[0] { &answers.1 } else { &answers.0 },
        Err(Error::Conflict(_))
    ));
    let winner = if won[0] { "reviewed" } else { "refining" };

    // Whichever won, the item moves on from there and the chain keeps its shape.
    sink(
        &kernel,
        &at("item:1:state"),
        &[("from", winner), ("to", "filed")],
    );
    let chain: Vec<(String, String)> = edits(&kernel, 1)
        .iter()
        .map(|e| {
            assert_eq!(e.kind, "state");
            (
                e.object["stateFrom"][0].clone(),
                e.object["stateTo"][0].clone(),
            )
        })
        .collect();
    assert_eq!(
        chain,
        [
            (state("filed"), state("queued")),
            (state("queued"), state(winner)),
            (state(winner), state("filed")),
        ]
    );
}

// ------------------------------------------------------------------- the README's queries

/// The README's fenced query whose first comment line starts with `heading`, with its item and
/// its state or label filled in.
fn readme_query(heading: &str) -> String {
    let readme = include_str!("../README.md");
    readme
        .split("```sparql\n")
        .skip(1)
        .map(|fence| fence.split("```").next().unwrap())
        .find(|query| query.starts_with(heading))
        .unwrap_or_else(|| panic!("no README query starts with {heading:?}"))
        .replace("urn:iki:ledger:graph:default", LIVE)
}

#[test]
fn the_readme_says_when_an_item_left_a_state() {
    let kernel = kernel();
    let transition = at("item:1:state");
    sink(&kernel, &transition, &[("from", "filed"), ("to", "queued")]);
    sink(
        &kernel,
        &transition,
        &[("from", "queued"), ("to", "reviewed")],
    );
    sink(
        &kernel,
        &transition,
        &[("from", "reviewed"), ("to", "queued")],
    );
    sink(&kernel, &transition, &[("from", "queued"), ("to", "filed")]);
    // Item 2 leaving the same state is not an answer about item 1.
    sink(
        &kernel,
        &at("item:2:state"),
        &[("from", "filed"), ("to", "queued")],
    );
    sink(
        &kernel,
        &at("item:2:state"),
        &[("from", "queued"), ("to", "filed")],
    );
    let query = readme_query("# When did item N leave state S")
        .replace("ledger:number 12", "ledger:number 1");
    let left = rows(&kernel, &query);
    assert_eq!(left.len(), 2, "it left `queued` twice: {left:?}");
    assert_eq!(left[0]["to"], state("reviewed"));
    assert_eq!(left[1]["to"], state("filed"));
    assert!(left[0]["when"] < left[1]["when"]);
}

#[test]
fn the_readme_says_when_a_label_was_removed() {
    let kernel = kernel();
    let label = [("item", "1"), ("content", "wip")];
    try_verb(&kernel, Verb::Delete, &at("label"), &label).unwrap();
    assert!(
        rows(
            &kernel,
            &readme_query("# When was label L removed")
                .replace("ledger:number 12", "ledger:number 1"),
        )
        .is_empty(),
        "removing a label the item never had is not an answer"
    );
    sink(&kernel, &at("label"), &label);
    try_verb(&kernel, Verb::Delete, &at("label"), &label).unwrap();
    sink(&kernel, &at("label"), &[("item", "2"), ("content", "wip")]);
    try_verb(
        &kernel,
        Verb::Delete,
        &at("label"),
        &[("item", "2"), ("content", "wip")],
    )
    .unwrap();
    let removed = rows(
        &kernel,
        &readme_query("# When was label L removed").replace("ledger:number 12", "ledger:number 1"),
    );
    assert_eq!(removed.len(), 1, "{removed:?}");
    assert!(removed[0].contains_key("when"));
}

// -------------------------------------------------------------------------- the faces

fn history_json(kernel: &Kernel, who: &Capability, number: i64) -> serde_json::Value {
    let answer = try_as(
        kernel,
        who,
        Verb::Source,
        &at(&format!("item:{number}:history")),
        &[("as", "application/json")],
    )
    .unwrap();
    serde_json::from_str(&answer).expect("JSON")
}

fn reader() -> Capability {
    Capability::scoped(vec![
        format!("urn:cap:ledger:read:{LEDGER}"),
        graph_read(LEDGER),
    ])
}

/// Every edit, one of each kind.
fn one_of_each(kernel: &Kernel) {
    let label = [("item", "1"), ("content", "bug")];
    let link = [("item", "1"), ("content", "2"), ("type", "blocks")];
    sink(kernel, &at("label"), &label);
    try_verb(kernel, Verb::Delete, &at("label"), &label).unwrap();
    sink(
        kernel,
        &at("item:1:state"),
        &[("from", "filed"), ("to", "queued")],
    );
    sink(kernel, &at("link"), &link);
    try_verb(kernel, Verb::Delete, &at("link"), &link).unwrap();
    sink(kernel, &at("item:1:about"), &[("content", "urn:example:a")]);
    try_verb(
        kernel,
        Verb::Delete,
        &at("item:1:about"),
        &[("content", "urn:example:a")],
    )
    .unwrap();
    sink(
        kernel,
        &at("item:1"),
        &[("content", "SECRET\n\nSECRET too")],
    );
    sink(kernel, &at("defer"), &[("item", "1")]);
}

/// ★ The plain face, in full: one line per event, oldest first, under nothing but `read`.
#[test]
fn the_plain_history_shows_each_object_in_words() {
    let kernel = kernel();
    one_of_each(&kernel);
    let other = item_iri(&kernel, 2);
    let answer = try_as(&kernel, &reader(), Verb::Source, &at("item:1:history"), &[]).unwrap();
    let mut lines = answer.lines();
    assert_eq!(lines.next(), Some("acme#1: 9 event(s)"));
    // Each line starts with its time; what follows is the fixed part.
    let shown: Vec<&str> = lines
        .map(|line| line.split_once("  ").expect("a time, then the event").1)
        .collect();
    assert_eq!(
        shown,
        [
            "label bug",
            "unlabel bug",
            "state filed to queued",
            format!("link blocks {other}").as_str(),
            format!("unlink blocks {other}").as_str(),
            "about urn:example:a",
            "unabout urn:example:a",
            "edit title, body",
            "defer",
        ]
    );
    assert!(!answer.contains("SECRET"), "{answer}");
}

#[test]
fn the_json_history_shows_each_object() {
    let kernel = kernel();
    one_of_each(&kernel);
    let other = item_iri(&kernel, 2);
    let doc = history_json(&kernel, &reader(), 1);
    assert_eq!(doc["ledger"], LEDGER);
    assert_eq!(doc["item"]["number"], 1);
    let events = doc["events"].as_array().unwrap();
    let of = |kind: &str| {
        events
            .iter()
            .find(|e| e["kind"] == kind)
            .unwrap_or_else(|| panic!("{kind}"))
    };
    assert_eq!(of("label")["label"], "bug");
    assert_eq!(of("unlabel")["label"], "bug");
    assert_eq!(of("state")["state_from"], state("filed"));
    assert_eq!(of("state")["state_to"], state("queued"));
    assert_eq!(of("link")["link_type"], "blocks");
    assert_eq!(of("link")["targets"][0], other.as_str());
    assert_eq!(of("unlink")["link_type"], "blocks");
    assert_eq!(of("about")["targets"][0], "urn:example:a");
    assert_eq!(of("unabout")["targets"][0], "urn:example:a");
    assert_eq!(of("edit")["fields"], serde_json::json!(["title", "body"]));
    assert!(of("defer").get("label").is_none());
    assert!(of("label")["at"].as_str().unwrap().ends_with('Z'));
    assert!(!doc.to_string().contains("SECRET"), "{doc}");
}

/// Every kind of event is in the history, not just the edits, so it reads as one story.
#[test]
fn the_history_tells_the_claim_the_close_and_the_reopen_too() {
    let kernel = kernel();
    sink(
        &kernel,
        &at("claim"),
        &[("item", "1"), ("content", "worker-1")],
    );
    sink(&kernel, &at("close"), &[("item", "1"), ("reason", "done")]);
    sink(&kernel, &at("reopen"), &[("item", "1")]);
    let plain = source(&kernel, &at("item:1:history"), &[]);
    let shown: Vec<&str> = plain
        .lines()
        .skip(1)
        .map(|line| line.split_once("  ").unwrap().1)
        .collect();
    assert_eq!(
        shown,
        [
            "claim worker-1",
            "closed",
            "release worker-1 (closed)",
            "reopened",
        ],
        "{plain}"
    );
}

/// An event written before objects were recorded says so, rather than reading as complete.
#[test]
fn an_event_with_no_object_says_so() {
    let kernel = kernel();
    try_verb(
        &kernel,
        Verb::Delete,
        &at("label"),
        &[("item", "1"), ("content", "absent")],
    )
    .unwrap();
    let plain = source(&kernel, &at("item:1:history"), &[]);
    assert!(
        plain.contains("unlabel (no object recorded: nothing changed, or written before 0.18.0)"),
        "{plain}"
    );
}

#[test]
fn the_history_of_an_item_with_none_is_a_sentence() {
    let kernel = kernel();
    assert_eq!(
        source(&kernel, &at("item:1:history"), &[]),
        "acme#1 has no recorded events\n"
    );
    assert_eq!(
        history_json(&kernel, &reader(), 1)["events"],
        serde_json::json!([])
    );
}

#[test]
fn the_history_needs_the_read_grant() {
    let kernel = kernel();
    let other = Capability::scoped(vec![
        "urn:cap:ledger:read:other".to_string(),
        graph_read("other"),
    ]);
    assert!(matches!(
        try_as(&kernel, &other, Verb::Source, &at("item:1:history"), &[]),
        Err(Error::Denied(_))
    ));
}

#[test]
fn the_history_of_no_item_is_not_found() {
    let kernel = kernel();
    assert!(matches!(
        try_verb(&kernel, Verb::Source, &at("item:99:history"), &[]),
        Err(Error::NotFound(_))
    ));
}

/// ★ Across ledgers: the subject's history names the other item by its IRI, which its readers
/// can already see on the link, and learns nothing else of it; the target's history has no
/// event for a link it only mirrors.
#[test]
fn a_cross_ledger_history_shows_an_iri_and_nothing_more() {
    let kernel = common::kernel();
    sink(
        &kernel,
        "urn:iki:ledger:cli:append",
        &[("content", "Ship the flag")],
    );
    let target = sink(
        &kernel,
        &at("append"),
        &[("content", "A PRIVATE TITLE\n\nA PRIVATE BODY")],
    )
    .split_whitespace()
    .nth(1)
    .unwrap()
    .to_string();
    sink(
        &kernel,
        &at("label"),
        &[("item", "1"), ("content", "PRIVATELABEL")],
    );
    let link = [("item", "1"), ("content", &target), ("type", "blocks")];
    sink(&kernel, "urn:iki:ledger:cli:link", &link);
    try_verb(&kernel, Verb::Delete, "urn:iki:ledger:cli:link", &link).unwrap();
    assert_eq!(
        edits_in(&kernel, "urn:iki:ledger:graph:cli", 1)
            .iter()
            .map(|e| &e.object["editTarget"])
            .collect::<Vec<_>>(),
        [std::slice::from_ref(&target), std::slice::from_ref(&target)]
    );
    assert_eq!(
        edits_in(&kernel, "urn:iki:ledger:graph:cli", 1)[0].object["editLinkType"],
        [format!("{NS}blocks")]
    );

    let cli_reader = Capability::scoped(vec![
        "urn:cap:ledger:read:cli".to_string(),
        graph_read("cli"),
    ]);
    let plain = try_as(
        &kernel,
        &cli_reader,
        Verb::Source,
        "urn:iki:ledger:cli:item:1:history",
        &[],
    )
    .unwrap();
    assert!(plain.contains(&format!("link blocks {target}")), "{plain}");
    assert!(
        plain.contains(&format!("unlink blocks {target}")),
        "{plain}"
    );
    assert!(!plain.contains("PRIVATE"), "{plain}");
    let json = try_as(
        &kernel,
        &cli_reader,
        Verb::Source,
        "urn:iki:ledger:cli:item:1:history",
        &[("as", "application/json")],
    )
    .unwrap();
    assert!(!json.contains("PRIVATE"), "{json}");

    // The target's own history: its label, and nothing of the link it only mirrors.
    let there = source(&kernel, &at("item:1:history"), &[]);
    assert_eq!(there.lines().count(), 2, "{there}");
    assert!(there.contains("label PRIVATELABEL"), "{there}");
}
