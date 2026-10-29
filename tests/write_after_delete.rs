//! **A write that lands after its item was deleted writes nothing, and says so**
//! (ledger #764).
//!
//! Every write reads its item first (`require_item`) and writes second, and a delete can
//! commit in between. Until this suite the write's update carried no condition on the item,
//! so it landed on the deleted IRI anyway: a comment, a `modified` stamp, a status, an
//! edge, all live in the ledger's graph beside the tombstone, invisible to every reader and
//! counted by nothing. And two deletes of one item that both read it live wrote ONE
//! tombstone holding both deletes' stamps, counts and reasons.
//!
//! The interleaving is forced, not hoped for: the store is wrapped so that, just before the
//! write's first update to the live graph reaches the store, a whole delete of the item runs
//! and commits. That is exactly the schedule two concurrent requests produce at the write's
//! await point between its read and its update.

mod common;

use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use common::*;
use ikigai_core::{
    ArgRef, Capability, Endpoint, Fallback, Invocation, Iri, Kernel, Representation, Request,
    Resolution, Result, Scope, Space, Verb,
};
use ikigai_store::DurableStore;

const LIVE: &str = "urn:iki:ledger:graph:default";

/// The request to run first, once, when the next live-graph update arrives.
type Armed = Arc<Mutex<Option<Request>>>;

/// One write under test: its name, verb, target and arguments.
type Case = (
    &'static str,
    Verb,
    &'static str,
    &'static [(&'static str, &'static str)],
);

struct Interleave {
    inner: Arc<dyn Space>,
    kernel: Arc<OnceLock<Kernel>>,
    armed: Armed,
}

struct BeforeLiveUpdate {
    inner: Arc<dyn Endpoint>,
    kernel: Arc<OnceLock<Kernel>>,
    armed: Armed,
}

impl Space for Interleave {
    fn resolve(&self, request: &Request, scope: &Scope) -> Resolution {
        let (kernel, armed) = (Arc::clone(&self.kernel), Arc::clone(&self.armed));
        self.inner
            .resolve(request, scope)
            .map_endpoint(move |inner| {
                Arc::new(BeforeLiveUpdate {
                    inner,
                    kernel,
                    armed,
                }) as Arc<dyn Endpoint>
            })
    }
}

#[async_trait]
impl Endpoint for BeforeLiveUpdate {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let to_live = inv.request.target.as_str() == "urn:iki:store:graph-update"
            && inv.inline_str("graph").map(|g| g == LIVE).unwrap_or(false);
        if to_live {
            let first = self.armed.lock().unwrap().take();
            if let Some(first) = first {
                let kernel = self.kernel.get().expect("the kernel is set");
                kernel.issue(first, &Capability::root()).await?;
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

/// A kernel whose store runs `armed` (once) ahead of the next live-graph update.
fn interleaved() -> (Arc<OnceLock<Kernel>>, Armed) {
    let cell = Arc::new(OnceLock::new());
    let armed: Armed = Arc::new(Mutex::new(None));
    let store = Arc::new(ikigai_store::space(DurableStore::in_memory().unwrap())) as Arc<dyn Space>;
    let space = Fallback::new(vec![
        Arc::new(Interleave {
            inner: store,
            kernel: Arc::clone(&cell),
            armed: Arc::clone(&armed),
        }) as Arc<dyn Space>,
        Arc::new(ikigai_ledger::space()) as Arc<dyn Space>,
    ]);
    let kernel =
        Kernel::with_meta_renderer(Arc::new(space), Arc::new(ikigai_vocab::TurtleRenderer))
            .with_clock(Arc::new(TickingClock::default()));
    assert!(cell.set(kernel).is_ok());
    (cell, armed)
}

fn request(verb: Verb, iri: &str, args: &[(&str, &str)]) -> Request {
    args.iter().fold(
        Request::new(verb, Iri::parse(iri).expect("a test IRI")),
        |r, (name, value)| r.with_arg(*name, ArgRef::Inline(value.as_bytes().to_vec())),
    )
}

/// Every live triple that names `item`, except the tombstone's `ledger:deletedItem` edge:
/// what a deleted item may leave behind is its tombstone, and nothing else.
fn leftovers(kernel: &Kernel, item: &str) -> Vec<String> {
    let json: serde_json::Value = serde_json::from_str(&select(
        kernel,
        &format!(
            "SELECT ?s ?p ?o WHERE {{ GRAPH <{LIVE}> {{ ?s ?p ?o . \
             FILTER(?s = <{item}> || ?o = <{item}> || \
             EXISTS {{ ?s <https://ikigai-rs.dev/ns/ledger#onItem> <{item}> }}) \
             FILTER(?p != <https://ikigai-rs.dev/ns/ledger#deletedItem>) }} }}"
        ),
    ))
    .expect("the store answers JSON");
    json["results"]["bindings"]
        .as_array()
        .expect("bindings")
        .iter()
        .map(|row| {
            format!(
                "{} {} {}",
                row["s"]["value"], row["p"]["value"], row["o"]["value"]
            )
        })
        .collect()
}

/// ★ Every write endpoint, its update landing just after a delete of its item committed.
/// Each must write nothing on the deleted IRI and refuse, so its caller is not told that a
/// write landed on an item that no longer exists. On 0.12.0 every one but `claim` (whose
/// update already required the item) answered success and left live triples behind.
#[test]
fn a_write_landing_after_its_items_delete_writes_nothing_and_refuses() {
    let cases: &[Case] = &[
        (
            "comment",
            Verb::Sink,
            "urn:iki:ledger:comment",
            &[("item", "#1"), ("content", "too late")],
        ),
        (
            "keyed comment",
            Verb::Sink,
            "urn:iki:ledger:comment",
            &[("item", "#1"), ("content", "too late"), ("key", "k-1")],
        ),
        (
            "hand-back",
            Verb::Sink,
            "urn:iki:ledger:comment",
            &[("item", "#1"), ("content", "done"), ("handback", "true")],
        ),
        (
            "close",
            Verb::Sink,
            "urn:iki:ledger:close",
            &[("item", "#1")],
        ),
        (
            "reopen",
            Verb::Sink,
            "urn:iki:ledger:reopen",
            &[("item", "#1")],
        ),
        (
            "edit",
            Verb::Sink,
            "urn:iki:ledger:item:1",
            &[("priority", "2")],
        ),
        (
            "defer",
            Verb::Sink,
            "urn:iki:ledger:defer",
            &[("item", "#1")],
        ),
        (
            "label",
            Verb::Sink,
            "urn:iki:ledger:label",
            &[("item", "#1"), ("content", "late")],
        ),
        (
            "about",
            Verb::Sink,
            "urn:iki:ledger:item:1:about",
            &[("content", "urn:x:late")],
        ),
        (
            "link from it",
            Verb::Sink,
            "urn:iki:ledger:link",
            &[("item", "#1"), ("content", "#2"), ("type", "blocks")],
        ),
        (
            "link to it",
            Verb::Sink,
            "urn:iki:ledger:link",
            &[("item", "#2"), ("content", "#1"), ("type", "blocks")],
        ),
        (
            "claim",
            Verb::Sink,
            "urn:iki:ledger:claim",
            &[("item", "#1"), ("content", "worker")],
        ),
    ];
    let mut wrong = Vec::new();
    for (name, verb, iri, args) in cases {
        let (cell, armed) = interleaved();
        let kernel = cell.get().unwrap();
        let item = append_iri(kernel, "doomed", &[]);
        append(kernel, "bystander", &[]);
        *armed.lock().unwrap() = Some(request(Verb::Delete, "urn:iki:ledger:item:1", &[]));
        let answer = try_verb(kernel, *verb, iri, args);
        assert!(
            armed.lock().unwrap().is_none(),
            "{name}: the delete never ran"
        );
        let left = leftovers(kernel, &item);
        if answer.is_ok() || !left.is_empty() {
            wrong.push(format!("{name}: answered {answer:?}, left {left:?}"));
        }
    }
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
}

/// ★ Two deletes of one item that both read it live: the second's update lands after the
/// first committed. One tombstone, holding ONE delete's state; a late delete refuses, and a
/// late purge purges what the first delete archived. On 0.12.0 the tombstone carried both
/// requests' `invalidatedAt` and both reasons.
#[test]
fn a_second_delete_landing_after_the_first_writes_no_second_tombstone_state() {
    for purge in [false, true] {
        let (cell, armed) = interleaved();
        let kernel = cell.get().unwrap();
        let item = append_iri(kernel, "doomed", &[]);
        sink(
            kernel,
            "urn:iki:ledger:comment",
            &[("item", "#1"), ("content", "a note")],
        );
        *armed.lock().unwrap() = Some(request(
            Verb::Delete,
            "urn:iki:ledger:item:1",
            &[("reason", "first")],
        ));
        let late = if purge {
            try_verb(
                kernel,
                Verb::Delete,
                "urn:iki:ledger:purge",
                &[("content", "#1"), ("reason", "second")],
            )
        } else {
            try_verb(
                kernel,
                Verb::Delete,
                "urn:iki:ledger:item:1",
                &[("reason", "second")],
            )
        };
        assert!(
            armed.lock().unwrap().is_none(),
            "the first delete never ran"
        );
        let json: serde_json::Value = serde_json::from_str(&select(
            kernel,
            &format!(
                "SELECT ?p ?o WHERE {{ GRAPH <{LIVE}> {{ ?t \
                 <https://ikigai-rs.dev/ns/ledger#deletedItem> <{item}> ; ?p ?o }} }}"
            ),
        ))
        .unwrap();
        let mut per_predicate = std::collections::BTreeMap::<String, Vec<String>>::new();
        for row in json["results"]["bindings"].as_array().unwrap() {
            per_predicate
                .entry(row["p"]["value"].as_str().unwrap().to_string())
                .or_default()
                .push(row["o"]["value"].as_str().unwrap().to_string());
        }
        let doubled: Vec<_> = per_predicate.iter().filter(|(_, v)| v.len() > 1).collect();
        assert!(
            doubled.is_empty(),
            "purge={purge}: a tombstone with two states: {doubled:?}"
        );
        assert!(
            per_predicate
                .get("https://ikigai-rs.dev/ns/ledger#reason")
                .is_some_and(|r| r == &vec!["first".to_string()]),
            "purge={purge}: the tombstone is not the first delete's: {per_predicate:?}"
        );
        if purge {
            // ★ A purge that finds the item already deleted is a purge of the archive: it
            // destroys what the first delete archived and says so on the tombstone, exactly
            // as a purge sent after the delete would have.
            let late = late.expect("a late purge purges what the delete archived");
            assert!(late.contains("deleted earlier"), "{late}");
            assert_eq!(
                per_predicate.get("https://ikigai-rs.dev/ns/ledger#recoverable"),
                Some(&vec!["false".to_string()]),
                "{per_predicate:?}"
            );
            let archived = select(
                kernel,
                "SELECT ?s WHERE { GRAPH <urn:iki:ledger:graph:default:deleted> { ?s ?p ?o } }",
            );
            assert!(!archived.contains("urn:iki:ledger"), "{archived}");
        } else {
            assert!(late.is_err(), "the late delete answered success: {late:?}");
        }
    }
}

/// Bulk migration counts only its guarded writes when another request changes an item.
#[test]
fn bulk_backfill_skips_concurrently_changed_items() {
    for (verb, target, args) in [
        (Verb::Delete, "urn:iki:ledger:item:1", vec![]),
        (Verb::Sink, "urn:iki:ledger:reopen", vec![("item", "#1")]),
        (
            Verb::Sink,
            "urn:iki:ledger:backfill-closed-at",
            vec![("content", "#1")],
        ),
        (
            Verb::Sink,
            "urn:iki:ledger:item:1",
            vec![("content", "edited")],
        ),
    ] {
        let (cell, armed) = interleaved();
        let k = cell.get().unwrap();
        let item = append_iri(k, "Legacy", &[]);
        sink(k, "urn:iki:store:graph-update", &[("graph", LIVE), ("content", &format!(
            "DELETE {{ GRAPH <{LIVE}> {{ <{item}> <https://ikigai-rs.dev/ns/ledger#status> ?old }} }} INSERT {{ GRAPH <{LIVE}> {{ <{item}> <https://ikigai-rs.dev/ns/ledger#status> <https://ikigai-rs.dev/ns/ledger#closed> }} }} WHERE {{ GRAPH <{LIVE}> {{ <{item}> <https://ikigai-rs.dev/ns/ledger#status> ?old }} }}"
        ))]);
        *armed.lock().unwrap() = Some(request(verb, target, &args));
        assert_eq!(
            sink(
                k,
                "urn:iki:ledger:backfill-closed-at",
                &[("content", "all")]
            ),
            "backfilled 0 items (unchanged)\n"
        );
        assert!(armed.lock().unwrap().is_none());
        if verb == Verb::Delete {
            assert!(leftovers(k, &item).is_empty());
        }
        let events = select(k, &format!("SELECT ?event WHERE {{ GRAPH <{LIVE}> {{ ?event a <https://ikigai-rs.dev/ns/ledger#StatusEvent> . FILTER(CONTAINS(STR(?event), \"status:backfill:\")) }} }}"));
        assert!(
            !events.contains(":status:backfill:"),
            "bulk migration wrote despite {target}: {events}"
        );
    }
}
