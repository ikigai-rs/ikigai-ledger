//! **A close decides inside its update, so a race cannot duplicate it or lose a claim**
//! (ledger #2764).
//!
//! `close` reads its item and then writes. Until this suite it decided from that read: the
//! item was open, so the write appended a `closed` status event and set `closedAt`; the item
//! was claimed, so it wrote "Closed while claimed by …". The update re-checked only that the
//! item was still live. So N concurrent closes of one claimed item each released the claim,
//! each wrote the release comment and each recorded a close; and a claim that committed
//! between a close's read and its update was retracted with no record at all.
//!
//! Both schedules are forced, not hoped for: the store is wrapped so that a close's update
//! to the live graph waits until every close has read (the concurrent case), or so that a
//! whole claim runs and commits just before it (the late claim).

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::task::Poll;

use async_trait::async_trait;
use common::*;
use futures::executor::block_on;
use ikigai_core::{
    ArgRef, Capability, Endpoint, Fallback, Invocation, Iri, Kernel, Representation, Request,
    Resolution, Result, Scope, Space, Verb,
};
use ikigai_store::DurableStore;

const LIVE: &str = "urn:iki:ledger:graph:default";
const NS: &str = "https://ikigai-rs.dev/ns/ledger#";

/// How the next live-graph updates are held back.
#[derive(Default)]
struct Schedule {
    /// A request to run first, once, when the next live-graph update arrives.
    armed: Mutex<Option<Request>>,
    /// How many live-graph updates still to hold at the gate.
    gated: Mutex<usize>,
    /// How many have arrived at it.
    arrived: AtomicUsize,
    /// How many it holds until all have arrived.
    width: AtomicUsize,
}

struct Interleave {
    inner: Arc<dyn Space>,
    kernel: Arc<OnceLock<Kernel>>,
    schedule: Arc<Schedule>,
}

struct BeforeLiveUpdate {
    inner: Arc<dyn Endpoint>,
    kernel: Arc<OnceLock<Kernel>>,
    schedule: Arc<Schedule>,
}

impl Space for Interleave {
    fn resolve(&self, request: &Request, scope: &Scope) -> Resolution {
        let (kernel, schedule) = (Arc::clone(&self.kernel), Arc::clone(&self.schedule));
        self.inner
            .resolve(request, scope)
            .map_endpoint(move |inner| {
                Arc::new(BeforeLiveUpdate {
                    inner,
                    kernel,
                    schedule,
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
            let first = self.schedule.armed.lock().unwrap().take();
            if let Some(first) = first {
                let kernel = self.kernel.get().expect("the kernel is set");
                kernel.issue(first, &Capability::root()).await?;
            }
            let held = {
                let mut gated = self.schedule.gated.lock().unwrap();
                let held = *gated > 0;
                *gated = gated.saturating_sub(1);
                held
            };
            if held {
                self.schedule.arrived.fetch_add(1, Ordering::SeqCst);
                let width = self.schedule.width.load(Ordering::SeqCst);
                futures::future::poll_fn(|cx| {
                    if self.schedule.arrived.load(Ordering::SeqCst) >= width {
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

fn scheduled() -> (Arc<OnceLock<Kernel>>, Arc<Schedule>) {
    let cell = Arc::new(OnceLock::new());
    let schedule = Arc::new(Schedule::default());
    let store = Arc::new(ikigai_store::space(DurableStore::in_memory().unwrap())) as Arc<dyn Space>;
    let space = Fallback::new(vec![
        Arc::new(Interleave {
            inner: store,
            kernel: Arc::clone(&cell),
            schedule: Arc::clone(&schedule),
        }) as Arc<dyn Space>,
        Arc::new(ikigai_ledger::space()) as Arc<dyn Space>,
    ]);
    let kernel =
        Kernel::with_meta_renderer(Arc::new(space), Arc::new(ikigai_vocab::TurtleRenderer))
            .with_clock(Arc::new(TickingClock::default()));
    assert!(cell.set(kernel).is_ok());
    (cell, schedule)
}

fn request(verb: Verb, iri: &str, args: &[(&str, &str)]) -> Request {
    args.iter().fold(
        Request::new(verb, Iri::parse(iri).expect("a test IRI")),
        |r, (name, value)| r.with_arg(*name, ArgRef::Inline(value.as_bytes().to_vec())),
    )
}

/// Every value of `?v` the query binds, in order.
fn values(kernel: &Kernel, query: &str) -> Vec<String> {
    let json: serde_json::Value =
        serde_json::from_str(&select(kernel, query)).expect("the store answers JSON");
    json["results"]["bindings"]
        .as_array()
        .expect("bindings")
        .iter()
        .map(|row| row["v"]["value"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Every comment body on `item`.
fn comments(kernel: &Kernel, item: &str) -> Vec<String> {
    values(
        kernel,
        &format!(
            "SELECT ?v WHERE {{ GRAPH <{LIVE}> {{ ?c <{NS}onItem> <{item}> ; \
             <{NS}body> ?v ; a <{NS}Comment> }} }}"
        ),
    )
}

/// The release comments on `item`.
fn releases(kernel: &Kernel, item: &str) -> Vec<String> {
    comments(kernel, item)
        .into_iter()
        .filter(|body| body.starts_with("Closed while claimed by "))
        .collect()
}

/// The `closed` status events of `item`.
fn close_events(kernel: &Kernel, item: &str) -> Vec<String> {
    values(
        kernel,
        &format!(
            "SELECT ?v WHERE {{ GRAPH <{LIVE}> {{ ?v a <{NS}StatusEvent> ; \
             <{NS}onItem> <{item}> ; <{NS}status> <{NS}closed> }} }}"
        ),
    )
}

/// Every `closedAt` of `item`.
fn closed_at(kernel: &Kernel, item: &str) -> Vec<String> {
    values(
        kernel,
        &format!("SELECT ?v WHERE {{ GRAPH <{LIVE}> {{ <{item}> <{NS}closedAt> ?v }} }}"),
    )
}

/// Every claim holder on `item`.
fn holders(kernel: &Kernel, item: &str) -> Vec<String> {
    values(
        kernel,
        &format!("SELECT ?v WHERE {{ GRAPH <{LIVE}> {{ <{item}> <{NS}claimedBy> ?v }} }}"),
    )
}

/// ★ The reporter's shape: claim, then four closes at once, every one of them reading the
/// item open and claimed before any writes. Exactly one close releases the claim and records
/// it, and exactly one close is recorded; the other three are verdict edits and say so. On
/// 0.14.0 all four wrote the release comment, a `closed` event and a `closedAt`.
#[test]
fn concurrent_closes_of_a_claimed_item_release_and_record_once() {
    const CLOSES: usize = 4;
    let (cell, schedule) = scheduled();
    let kernel = cell.get().unwrap();
    let item = append_iri(kernel, "probe", &[]);
    sink(
        kernel,
        "urn:iki:ledger:claim",
        &[("item", "#1"), ("content", "h1")],
    );
    schedule.width.store(CLOSES, Ordering::SeqCst);
    *schedule.gated.lock().unwrap() = CLOSES;
    let notes: Vec<String> = (1..=CLOSES).map(|k| format!("close {k}")).collect();
    let root = Capability::root();
    let answers = block_on(futures::future::join_all(notes.iter().map(|note| {
        kernel.issue(
            request(
                Verb::Sink,
                "urn:iki:ledger:close",
                &[("item", "#1"), ("reason", "done"), ("content", note)],
            ),
            &root,
        )
    })));
    assert_eq!(
        schedule.arrived.load(Ordering::SeqCst),
        CLOSES,
        "every close reached its update before any wrote"
    );
    let answers: Vec<String> = answers
        .into_iter()
        .map(|a| String::from_utf8_lossy(&a.expect("every close succeeds").bytes).into_owned())
        .collect();

    assert_eq!(
        releases(kernel, &item),
        ["Closed while claimed by h1; the claim is released."],
        "one release, recorded once; answers: {answers:?}"
    );
    assert_eq!(close_events(kernel, &item).len(), 1, "one close event");
    assert_eq!(closed_at(kernel, &item).len(), 1, "one closedAt");
    assert!(holders(kernel, &item).is_empty(), "the claim is released");
    let mut bodies = comments(kernel, &item);
    bodies.sort();
    let mut want: Vec<String> = notes.clone();
    want.push("Closed while claimed by h1; the claim is released.".to_string());
    want.sort();
    assert_eq!(bodies, want, "every note, and one release");

    let released: Vec<&String> = answers.iter().filter(|a| a.contains("released")).collect();
    assert_eq!(released, ["closed #1 (done), released h1\n"], "{answers:?}");
    let edits = answers
        .iter()
        .filter(|a| {
            a.as_str() == "closed #1 (done); it was already closed, so only the verdict changed\n"
        })
        .count();
    assert_eq!(edits, CLOSES - 1, "{answers:?}");
}

/// ★ A claim that commits between a close's read (the item open and free) and its update is
/// released by that update, and the release is recorded, naming the claim it released. On
/// 0.14.0 it was retracted with no comment, and the answer named nobody.
#[test]
fn a_claim_landing_between_a_closes_read_and_its_update_is_recorded() {
    let (cell, schedule) = scheduled();
    let kernel = cell.get().unwrap();
    let item = append_iri(kernel, "probe", &[]);
    *schedule.armed.lock().unwrap() = Some(request(
        Verb::Sink,
        "urn:iki:ledger:claim",
        &[("item", "#1"), ("content", "late-worker")],
    ));
    let answer = sink(kernel, "urn:iki:ledger:close", &[("item", "#1")]);
    assert!(
        schedule.armed.lock().unwrap().is_none(),
        "the claim never ran"
    );
    assert!(holders(kernel, &item).is_empty(), "the claim is released");
    assert_eq!(
        releases(kernel, &item),
        ["Closed while claimed by late-worker; the claim is released."]
    );
    assert_eq!(answer, "closed #1 (done), released late-worker\n");
    assert_eq!(close_events(kernel, &item).len(), 1);
}

/// The JSON face of the same race names the claim this update released.
#[test]
fn the_json_answer_names_the_claim_the_update_released() {
    let (cell, schedule) = scheduled();
    let kernel = cell.get().unwrap();
    append_iri(kernel, "probe", &[]);
    *schedule.armed.lock().unwrap() = Some(request(
        Verb::Sink,
        "urn:iki:ledger:claim",
        &[("item", "#1"), ("content", "late-worker")],
    ));
    let answer = sink(
        kernel,
        "urn:iki:ledger:close",
        &[("item", "#1"), ("as", "application/json")],
    );
    let doc: serde_json::Value = serde_json::from_str(&answer).expect("JSON");
    assert_eq!(doc["outcome"], "closed", "{doc}");
    assert_eq!(doc["released"], "late-worker", "{doc}");
    assert!(doc.get("already_closed").is_none(), "{doc}");
}

/// Closing an already closed item stays allowed: it edits the verdict, keeps the close time,
/// writes no second event and no release, and says it was a verdict edit.
#[test]
fn closing_a_closed_item_edits_the_verdict_and_says_so() {
    let kernel = kernel();
    let item = append_iri(&kernel, "probe", &[]);
    assert_eq!(
        sink(&kernel, "urn:iki:ledger:close", &[("item", "#1")]),
        "closed #1 (done)\n"
    );
    let at = closed_at(&kernel, &item);
    assert_eq!(
        sink(
            &kernel,
            "urn:iki:ledger:close",
            &[("item", "#1"), ("reason", "wontfix")]
        ),
        "closed #1 (wontfix); it was already closed, so only the verdict changed\n"
    );
    assert_eq!(
        closed_at(&kernel, &item),
        at,
        "the close time does not move"
    );
    assert_eq!(close_events(&kernel, &item).len(), 1);
    let doc: serde_json::Value = serde_json::from_str(&sink(
        &kernel,
        "urn:iki:ledger:close",
        &[
            ("item", "#1"),
            ("reason", "done"),
            ("as", "application/json"),
        ],
    ))
    .expect("JSON");
    assert_eq!(doc["outcome"], "closed", "{doc}");
    assert_eq!(doc["reason"], "done", "{doc}");
    assert_eq!(doc["already_closed"], true, "{doc}");
    assert!(
        source(&kernel, "urn:iki:ledger:item:1", &[]).contains("(done)"),
        "the verdict is the latest"
    );
}
