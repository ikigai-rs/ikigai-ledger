//! **A comment that races a recoverable delete is archived with the item, and the purge that
//! follows destroys it** (ledger #2780, found by the check-then-act audit #2772).
//!
//! A recoverable delete reads the item's quads, archives them into the graveyard, and then
//! commits in the live graph by removing exactly the quads it read. A comment that landed
//! between the read and the commit was in neither set: it stayed in the live graph beside the
//! tombstone, an orphan that no reader shows and no later purge touches, since a purge of a
//! deleted item clears only the graveyard and the tombstone. A pasted secret in that comment
//! could never be destroyed through the ledger.
//!
//! The forced test runs the whole comment just before the delete's first write to the
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

const GRAVEYARD: &str = "urn:iki:ledger:graph:default:deleted";
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

/// The graphs holding the secret anywhere, as a literal, one row per quad.
fn graphs_holding_secret(kernel: &Kernel) -> Vec<String> {
    let json: serde_json::Value = serde_json::from_str(&select(
        kernel,
        &format!("SELECT ?g WHERE {{ GRAPH ?g {{ ?s ?p \"{SECRET}\" }} }}"),
    ))
    .expect("the store answers JSON");
    json["results"]["bindings"]
        .as_array()
        .expect("results.bindings")
        .iter()
        .filter_map(|row| row["g"]["value"].as_str().map(str::to_string))
        .collect()
}

/// ★ The audit's schedule, forced: the delete has read the item and is about to archive it
/// when a whole comment carrying a secret lands. The comment is answered as written, so the
/// delete must archive it with the item, and the purge that follows must destroy it. On
/// 0.14.1 the comment stayed in the live graph, and the purge left it there.
#[test]
fn a_comment_inside_a_deletes_window_is_archived_and_then_purged_forced() {
    let (cell, armed) = gated();
    let kernel = cell.get().unwrap();
    append_iri(kernel, "an item someone comments on", &[]);
    *armed.lock().unwrap() = Some(request(
        Verb::Sink,
        "urn:iki:ledger:comment",
        &[("item", "#1"), ("content", SECRET)],
    ));
    delete(kernel, "urn:iki:ledger:item:1", &[]);
    assert!(
        armed.lock().unwrap().is_none(),
        "the comment ran in the window"
    );

    let after_delete = graphs_holding_secret(kernel);
    assert_eq!(
        after_delete,
        vec![GRAVEYARD.to_string()],
        "a comment written while its item was being deleted is archived with it, and only there"
    );

    block_on(kernel.issue(
        request(
            Verb::Delete,
            "urn:iki:ledger:purge",
            &[("content", "#1"), ("reason", "a pasted secret")],
        ),
        &Capability::root(),
    ))
    .expect("the purge succeeds");
    let after_purge = graphs_holding_secret(kernel);
    assert!(
        after_purge.is_empty(),
        "the purge left the secret in {after_purge:?}"
    );
}

/// The same race, unforced: a comment and a recoverable delete released together, three
/// hundred times, then a purge. Whichever commits first, nothing of the comment may survive
/// the purge.
#[test]
fn a_comment_racing_a_delete_is_destroyed_by_the_purge_that_follows() {
    let mut leaked = Vec::new();
    for round in 0..300 {
        let kernel = kernel();
        append_iri(&kernel, "an item someone comments on", &[]);
        let barrier = Barrier::new(2);
        std::thread::scope(|scope| {
            let commenter = scope.spawn(|| {
                barrier.wait();
                let _ = try_verb(
                    &kernel,
                    Verb::Sink,
                    "urn:iki:ledger:comment",
                    &[("item", "#1"), ("content", SECRET)],
                );
            });
            let deleter = scope.spawn(|| {
                barrier.wait();
                try_verb(&kernel, Verb::Delete, "urn:iki:ledger:item:1", &[])
                    .expect("the delete succeeds");
            });
            commenter.join().unwrap();
            deleter.join().unwrap();
        });
        try_verb(
            &kernel,
            Verb::Delete,
            "urn:iki:ledger:purge",
            &[("content", "#1")],
        )
        .unwrap_or_else(|e| panic!("round {round}: the purge failed: {e}"));
        let left = graphs_holding_secret(&kernel);
        if !left.is_empty() {
            leaked.push((round, left));
        }
    }
    assert!(
        leaked.is_empty(),
        "{} of 300 purges left a racing comment behind; (round, graphs): {:?}",
        leaked.len(),
        &leaked[..leaked.len().min(10)]
    );
}

/// A session acknowledged after the delete's read must be archived whole on its retry.
#[test]
fn a_session_inside_a_deletes_window_is_archived_and_then_purged() {
    let (cell, armed) = gated();
    let kernel = cell.get().unwrap();
    append_iri(kernel, "an item with a session", &[]);
    sink(
        kernel,
        "urn:iki:ledger:comment",
        &[
            ("item", "1"),
            ("content", "handing back"),
            ("handback", "true"),
        ],
    );
    *armed.lock().unwrap() = Some(request(
        Verb::Sink,
        "urn:iki:ledger:item:1:sessions",
        &[(
            "content",
            r#"{
            "session": "during-delete",
            "machineName": "worker",
            "harness": "codex",
            "model": "model",
            "exitStatus": 0,
            "wallClockMs": 100
        }"#,
        )],
    ));
    delete(kernel, "urn:iki:ledger:item:1", &[]);
    assert!(
        armed.lock().unwrap().is_none(),
        "the session ran in the window"
    );
    let quads = || -> serde_json::Value {
        serde_json::from_str(&select(
            kernel,
            "SELECT ?g ?p ?o WHERE { GRAPH ?g {
                <urn:agent:session:id:during-delete> ?p ?o
            } }",
        ))
        .unwrap()
    };
    let after_delete = quads();
    let rows = after_delete["results"]["bindings"].as_array().unwrap();
    assert!(
        rows.len() > 10,
        "the entire summary was archived: {after_delete}"
    );
    assert!(
        rows.iter().all(|r| r["g"]["value"] == GRAVEYARD),
        "the summary must be only in the graveyard: {after_delete}"
    );
    delete(kernel, "urn:iki:ledger:purge", &[("content", "1")]);
    assert!(quads()["results"]["bindings"]
        .as_array()
        .unwrap()
        .is_empty());
}
