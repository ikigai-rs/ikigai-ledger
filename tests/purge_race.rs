//! **A purge inside a recoverable delete's window leaves nothing in the graveyard**
//! (ledger #2779, found by the check-then-act audit #2772).
//!
//! A recoverable delete reads the item's quads, archives them into the graveyard with
//! `INSERT DATA`, and then commits in the live graph. A purge that runs entirely inside that
//! window, between the delete's read and its archive, finds the item still live: it empties
//! the graveyard, removes the item and writes a tombstone saying "purged, NOT recoverable".
//! Then the delete's archive landed in the graveyard anyway. Its live commit found the
//! tombstone and lost, but by then the item's full content, a pasted secret included, was
//! back in the graveyard under a tombstone that said it had been destroyed.
//!
//! The forced test runs the whole purge just before the delete's first write to the
//! graveyard. The unforced test sends both at once, three hundred times.

mod common;

use std::sync::{Arc, Barrier, Mutex, OnceLock};

use async_trait::async_trait;
use common::*;
use futures::executor::block_on;
use ikigai_core::{
    ArgRef, Capability, Endpoint, Fallback, Invocation, Iri, Kernel, Representation, Request,
    Resolution, Result, Scope, Space, Verb,
};
use ikigai_store::DurableStore;

const LIVE: &str = "urn:iki:ledger:graph:default";
const GRAVEYARD: &str = "urn:iki:ledger:graph:default:deleted";
const NS: &str = "https://ikigai-rs.dev/ns/ledger#";
const SECRET: &str = "token ghp_0123456789abcdef pasted by mistake";

/// Runs an armed request, once, just before the next write to the graveyard.
struct BeforeGraveyardWrite {
    inner: Arc<dyn Space>,
    kernel: Arc<OnceLock<Kernel>>,
    armed: Arc<Mutex<Option<Request>>>,
}

struct Gate {
    inner: Arc<dyn Endpoint>,
    kernel: Arc<OnceLock<Kernel>>,
    armed: Arc<Mutex<Option<Request>>>,
}

impl Space for BeforeGraveyardWrite {
    fn resolve(&self, request: &Request, scope: &Scope) -> Resolution {
        let (kernel, armed) = (Arc::clone(&self.kernel), Arc::clone(&self.armed));
        self.inner
            .resolve(request, scope)
            .map_endpoint(move |inner| {
                Arc::new(Gate {
                    inner,
                    kernel,
                    armed,
                }) as Arc<dyn Endpoint>
            })
    }
}

#[async_trait]
impl Endpoint for Gate {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let to_graveyard = inv.request.target.as_str() == "urn:iki:store:graph-update"
            && inv
                .inline_str("graph")
                .map(|g| g == GRAVEYARD)
                .unwrap_or(false);
        if to_graveyard {
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

fn gated() -> (Arc<OnceLock<Kernel>>, Arc<Mutex<Option<Request>>>) {
    let cell = Arc::new(OnceLock::new());
    let armed = Arc::new(Mutex::new(None));
    let store = Arc::new(ikigai_store::space(DurableStore::in_memory().unwrap())) as Arc<dyn Space>;
    let space = Fallback::new(vec![
        Arc::new(BeforeGraveyardWrite {
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

/// Every quad of `item` in the graveyard: its own, its comments', and the edges pointing at
/// it, the set a delete archives and a purge destroys.
fn archived(kernel: &Kernel, item: &str) -> Vec<String> {
    let json: serde_json::Value = serde_json::from_str(&select(
        kernel,
        &format!(
            "SELECT ?s ?p ?o WHERE {{ GRAPH <{GRAVEYARD}> {{ ?s ?p ?o . \
             FILTER(?s = <{item}> || ?o = <{item}> || EXISTS {{ ?s <{NS}onItem> <{item}> }}) }} }}"
        ),
    ))
    .expect("the store answers JSON");
    json["results"]["bindings"]
        .as_array()
        .expect("results.bindings")
        .iter()
        .map(|row| {
            format!(
                "{} {} {}",
                row["s"]["value"].as_str().unwrap_or_default(),
                row["p"]["value"].as_str().unwrap_or_default(),
                row["o"]["value"].as_str().unwrap_or_default()
            )
        })
        .collect()
}

/// The graphs holding the secret anywhere, as a subject's literal.
fn graphs_holding_secret(kernel: &Kernel) -> String {
    select(
        kernel,
        &format!("SELECT ?g WHERE {{ GRAPH ?g {{ ?s ?p \"{SECRET}\" }} }}"),
    )
}

/// The `recoverable` values on the item's tombstone.
fn recoverable(kernel: &Kernel, item: &str) -> String {
    select(
        kernel,
        &format!(
            "SELECT ?v WHERE {{ GRAPH <{LIVE}> {{ ?t <{NS}deletedItem> <{item}> ; \
             <{NS}recoverable> ?v }} }}"
        ),
    )
}

/// ★ The audit's schedule, forced: the delete has read the item and is about to archive it
/// when a whole purge runs. On 0.14.1 the purge answered "destroyed, NOT recoverable" and the
/// delete's archive then put every quad of the item back in the graveyard.
#[test]
fn a_purge_inside_a_deletes_window_leaves_nothing_in_the_graveyard_forced() {
    let (cell, armed) = gated();
    let kernel = cell.get().unwrap();
    let item = append_iri(kernel, SECRET, &[]);
    sink(
        kernel,
        "urn:iki:ledger:comment",
        &[("item", "#1"), ("content", "a comment on it")],
    );
    *armed.lock().unwrap() = Some(request(
        Verb::Delete,
        "urn:iki:ledger:purge",
        &[("content", "#1"), ("reason", "a pasted secret")],
    ));
    let deleted = block_on(kernel.issue(
        request(Verb::Delete, "urn:iki:ledger:item:1", &[]),
        &Capability::root(),
    ));
    assert!(
        armed.lock().unwrap().is_none(),
        "the purge ran in the window"
    );

    let left = archived(kernel, &item);
    assert!(
        left.is_empty(),
        "{} quad(s) of a purged item are in the graveyard (delete answered {:?}): {left:#?}",
        left.len(),
        deleted.map(|r| String::from_utf8_lossy(&r.bytes).into_owned())
    );
    let holding = graphs_holding_secret(kernel);
    assert!(!holding.contains("urn:iki:ledger:graph"), "{holding}");
    let state = recoverable(kernel, &item);
    assert!(
        state.contains("false") && !state.contains("true"),
        "{state}"
    );
}

/// The same race, unforced: a delete and a purge released together, three hundred times.
/// Whichever commits first, nothing of the item may be left in the graveyard after a purge.
#[test]
fn a_delete_and_a_purge_at_once_leave_nothing_in_the_graveyard() {
    let mut leaked = Vec::new();
    for round in 0..300 {
        let kernel = kernel();
        let item = append_iri(&kernel, SECRET, &[]);
        let barrier = Barrier::new(2);
        let purged = std::thread::scope(|scope| {
            let deleter = scope.spawn(|| {
                barrier.wait();
                let _ = try_verb(&kernel, Verb::Delete, "urn:iki:ledger:item:1", &[]);
            });
            let purger = scope.spawn(|| {
                barrier.wait();
                try_verb(
                    &kernel,
                    Verb::Delete,
                    "urn:iki:ledger:purge",
                    &[("content", "#1")],
                )
                .is_ok()
            });
            deleter.join().unwrap();
            purger.join().unwrap()
        });
        assert!(purged, "round {round}: the purge failed");
        let left = archived(&kernel, &item).len();
        if left > 0 {
            leaked.push((round, left));
        }
    }
    assert!(
        leaked.is_empty(),
        "{} of 300 purges left the item in the graveyard; (round, quads): {:?}",
        leaked.len(),
        &leaked[..leaked.len().min(10)]
    );
}
