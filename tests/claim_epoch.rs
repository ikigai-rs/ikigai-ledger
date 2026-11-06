//! **A claim carries a fencing epoch, and a stale holder's writes are refused** (ledger #3205).
//!
//! Until 0.19.0 a claim recorded only its holder and its expiry. A worker that stalled past
//! its lease, was taken over, and then woke could still comment, hand back, close, renew or
//! release on the authority it had lost: only a convention (renew just before acting) stood in
//! the way, and a hand-back sent after the lease was lost landed like any other.
//!
//! Now every grant and every takeover sets `ledger:claimEpoch` to one more than the item's
//! previous epoch (a renewal keeps it, and it never goes down), anyone who may read the ledger
//! can ask whether an epoch is current, and each of those writes takes `epoch=`: given and not
//! current, it is refused as `Conflict` naming the current holder and epoch, and nothing is
//! written. Without `epoch=` nothing changes.
//!
//! The races are forced, not hoped for: the store is wrapped, as in `tests/close_race.rs`, so
//! that a takeover commits just before a fenced write's update, or so that several takeovers
//! all read before any writes.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::task::Poll;

use async_trait::async_trait;
use common::*;
use futures::executor::block_on;
use ikigai_core::{
    ArgRef, Capability, Endpoint, Error, Fallback, Invocation, Iri, Kernel, Representation,
    Request, Resolution, Result, Scope, Space, Verb,
};
use ikigai_store::DurableStore;

const LIVE: &str = "urn:iki:ledger:graph:default";
const NS: &str = "https://ikigai-rs.dev/ns/ledger#";
const CLAIM: &str = "urn:iki:ledger:claim";

// ------------------------------------------------------------------ helpers

fn try_sink(
    kernel: &Kernel,
    iri: &str,
    args: &[(&str, &str)],
) -> std::result::Result<String, Error> {
    try_verb(kernel, Verb::Sink, iri, args)
}

/// An item, claimed by `worker-1` on a one-second lease: on the ticking clock it has expired
/// by the next request, so it can be taken over.
fn claimed_item(kernel: &Kernel) -> String {
    let item = append_iri(kernel, "Fix the widget", &[]);
    sink(
        kernel,
        CLAIM,
        &[("item", "#1"), ("content", "worker-1"), ("lease", "1s")],
    );
    item
}

fn take_over(kernel: &Kernel, from: &str, by: &str) -> String {
    sink(
        kernel,
        CLAIM,
        &[
            ("item", "#1"),
            ("content", by),
            ("takeover", "true"),
            ("from", from),
            ("lease", "1s"),
        ],
    )
}

/// The item's JSON face.
fn item_json(kernel: &Kernel) -> serde_json::Value {
    let view = source(
        kernel,
        "urn:iki:ledger:item:1",
        &[("as", "application/json")],
    );
    serde_json::from_str(&view).expect("JSON")
}

/// The current claim's epoch, as the item's JSON face carries it.
fn epoch(kernel: &Kernel) -> Option<u64> {
    item_json(kernel)["item"]["claim"]["epoch"].as_u64()
}

fn holder(kernel: &Kernel) -> Option<String> {
    item_json(kernel)["item"]["claim"]["holder"]
        .as_str()
        .map(str::to_string)
}

/// Every value of `?v` the query binds.
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

/// Every stored `claimEpoch` of `item`: never more than one.
fn stored_epochs(kernel: &Kernel, item: &str) -> Vec<String> {
    values(
        kernel,
        &format!("SELECT ?v WHERE {{ GRAPH <{LIVE}> {{ <{item}> <{NS}claimEpoch> ?v }} }}"),
    )
}

/// The whole ledger graph, one sorted line per triple: what "nothing was written" compares.
fn graph(kernel: &Kernel) -> Vec<String> {
    let json: serde_json::Value = serde_json::from_str(&select(
        kernel,
        &format!("SELECT ?s ?p ?o WHERE {{ GRAPH <{LIVE}> {{ ?s ?p ?o }} }}"),
    ))
    .expect("JSON");
    let mut rows: Vec<String> = json["results"]["bindings"]
        .as_array()
        .expect("bindings")
        .iter()
        .map(|row| format!("{} {} {}", row["s"], row["p"], row["o"]))
        .collect();
    rows.sort();
    rows
}

/// The refusal a stale epoch gets: a Conflict naming the holder and the epoch now.
fn stale(result: std::result::Result<String, Error>, holder: &str, current: u64, what: &str) {
    match result {
        Err(Error::Conflict(said)) => {
            assert!(
                said.contains(&format!("held by {holder} at epoch {current}"))
                    && said.contains("nothing was written"),
                "{what}: {said}"
            );
        }
        other => panic!("{what}: expected Conflict, got {other:?}"),
    }
}

/// One write a holder makes: what it is, its verb, its resource and its arguments.
type Write = (
    &'static str,
    Verb,
    &'static str,
    Vec<(&'static str, String)>,
);

/// The five writes a holder makes about its item, each fenced with `epoch`.
fn fenced_writes(epoch: &str) -> Vec<Write> {
    let e = || ("epoch", epoch.to_string());
    vec![
        (
            "comment",
            Verb::Sink,
            "urn:iki:ledger:comment",
            vec![("item", "#1".into()), ("content", "progress".into()), e()],
        ),
        (
            "hand-back",
            Verb::Sink,
            "urn:iki:ledger:comment",
            vec![
                ("item", "#1".into()),
                ("content", "done, see the PR".into()),
                ("handback", "true".into()),
                e(),
            ],
        ),
        (
            "close",
            Verb::Sink,
            "urn:iki:ledger:close",
            vec![("item", "#1".into()), ("content", "finished".into()), e()],
        ),
        (
            "renewal",
            Verb::Sink,
            CLAIM,
            vec![
                ("item", "#1".into()),
                ("content", "worker-1".into()),
                ("renew", "true".into()),
                ("lease", "1h".into()),
                e(),
            ],
        ),
        (
            "release",
            Verb::Delete,
            CLAIM,
            vec![("item", "#1".into()), ("content", "letting go".into()), e()],
        ),
    ]
}

fn run(
    kernel: &Kernel,
    verb: Verb,
    iri: &str,
    args: &[(&'static str, String)],
) -> std::result::Result<String, Error> {
    let args: Vec<(&str, &str)> = args.iter().map(|(n, v)| (*n, v.as_str())).collect();
    try_verb(kernel, verb, iri, &args)
}

// ------------------------------------------------------------------ the epoch

#[test]
fn a_grant_then_a_takeover_raises_the_epoch() {
    let kernel = kernel();
    let item = append_iri(&kernel, "Fix the widget", &[]);
    assert_eq!(epoch(&kernel), None, "an unclaimed item has no claim epoch");
    let said = sink(
        &kernel,
        CLAIM,
        &[("item", "#1"), ("content", "worker-1"), ("lease", "1s")],
    );
    assert!(
        said.starts_with("#1 claimed by worker-1 at epoch 1 until "),
        "{said}"
    );
    assert_eq!(epoch(&kernel), Some(1));
    let said = take_over(&kernel, "worker-1", "worker-2");
    assert!(
        said.starts_with("#1 taken over from worker-1 by worker-2 at epoch 2 until "),
        "{said}"
    );
    assert_eq!(epoch(&kernel), Some(2));
    take_over(&kernel, "worker-2", "worker-3");
    assert_eq!(epoch(&kernel), Some(3));
    assert_eq!(stored_epochs(&kernel, &item), ["3"], "one epoch, replaced");
}

#[test]
fn a_renewal_keeps_the_epoch() {
    let kernel = kernel();
    claimed_item(&kernel);
    take_over(&kernel, "worker-1", "worker-2");
    let said = sink(
        &kernel,
        CLAIM,
        &[("item", "#1"), ("content", "worker-2"), ("renew", "true")],
    );
    assert_eq!(said, "#1 renewed by worker-2 at epoch 2\n");
    assert_eq!(epoch(&kernel), Some(2));
    // The same holder claiming again renews too, and keeps it.
    let said = sink(&kernel, CLAIM, &[("item", "#1"), ("content", "worker-2")]);
    assert_eq!(said, "#1 claimed by worker-2 at epoch 2\n");
    assert_eq!(epoch(&kernel), Some(2));
}

#[test]
fn a_release_then_a_new_claim_raises_the_epoch() {
    let kernel = kernel();
    let item = claimed_item(&kernel);
    delete(&kernel, CLAIM, &[("item", "#1")]);
    assert_eq!(holder(&kernel), None);
    assert_eq!(
        stored_epochs(&kernel, &item),
        ["1"],
        "a release keeps the epoch, so it never goes down"
    );
    sink(&kernel, CLAIM, &[("item", "#1"), ("content", "worker-1")]);
    assert_eq!(epoch(&kernel), Some(2), "even the same holder's new claim");
    // A close releases too, and a reopened item's next claim still counts up.
    sink(&kernel, "urn:iki:ledger:close", &[("item", "#1")]);
    sink(&kernel, "urn:iki:ledger:reopen", &[("item", "#1")]);
    sink(&kernel, CLAIM, &[("item", "#1"), ("content", "worker-2")]);
    assert_eq!(epoch(&kernel), Some(3));
}

#[test]
fn the_faces_carry_the_epoch_while_claimed() {
    let kernel = kernel();
    claimed_item(&kernel);
    let detail = source(&kernel, "urn:iki:ledger:item:1", &[]);
    assert!(detail.contains("\n  epoch:    1\n"), "{detail}");
    let turtle = source(&kernel, "urn:iki:ledger:item:1", &[("as", "text/turtle")]);
    assert!(turtle.contains("claimEpoch"), "{turtle}");
    delete(&kernel, CLAIM, &[("item", "#1")]);
    let detail = source(&kernel, "urn:iki:ledger:item:1", &[]);
    assert!(
        !detail.contains("epoch:"),
        "no claim, no epoch line: {detail}"
    );
    assert!(item_json(&kernel)["item"]["claim"].is_null());
}

// ------------------------------------------------------------------ the check

#[test]
fn the_check_answers_each_state() {
    let kernel = kernel();
    claimed_item(&kernel);
    let check = |epoch: &str| {
        try_verb(
            &kernel,
            Verb::Source,
            CLAIM,
            &[("item", "#1"), ("epoch", epoch)],
        )
    };
    assert_eq!(
        check("1").unwrap(),
        "#1 epoch 1 is current: held by worker-1\n"
    );
    take_over(&kernel, "worker-1", "worker-2");
    assert_eq!(
        check("1").unwrap(),
        "#1 epoch 1 is superseded: held by worker-2 at epoch 2\n"
    );
    assert_eq!(
        check("2").unwrap(),
        "#1 epoch 2 is current: held by worker-2\n"
    );
    delete(&kernel, CLAIM, &[("item", "#1")]);
    assert_eq!(
        check("2").unwrap(),
        "#1 epoch 2 is not current: nobody holds #1, and its last claim was epoch 2\n"
    );
    // An epoch the item never granted is nobody's, and is refused rather than answered.
    let refused = check("3");
    assert!(
        matches!(&refused, Err(Error::InvalidArgument { name, detail })
            if name == "epoch" && detail.contains("its highest epoch is 2")),
        "{refused:?}"
    );
    for bad in ["0", "-1", "one", ""] {
        assert!(
            matches!(check(bad), Err(Error::InvalidArgument { ref name, .. }) if name == "epoch"),
            "accepted `{bad}`"
        );
    }
    // And as JSON.
    sink(&kernel, CLAIM, &[("item", "#1"), ("content", "worker-3")]);
    let doc: serde_json::Value = serde_json::from_str(&source(
        &kernel,
        CLAIM,
        &[("item", "#1"), ("epoch", "2"), ("as", "application/json")],
    ))
    .unwrap();
    assert_eq!(doc["schema"], 1);
    assert_eq!(doc["state"], "superseded");
    assert_eq!(doc["holder"], "worker-3");
    assert_eq!(doc["current"], 3);
    assert_eq!(doc["highest"], 3);
    assert_eq!(doc["item"]["display"], "#1");
}

/// "Without needing the claim's holder to ask": the read grant alone is enough.
#[test]
fn the_check_needs_only_the_read_grant() {
    let kernel = kernel();
    claimed_item(&kernel);
    let reader = Capability::scoped([
        "urn:cap:ledger:read:default".to_string(),
        graph_read("default"),
    ]);
    let said = try_as(
        &kernel,
        &reader,
        Verb::Source,
        CLAIM,
        &[("item", "#1"), ("epoch", "1")],
    );
    assert_eq!(said.unwrap(), "#1 epoch 1 is current: held by worker-1\n");
}

// ------------------------------------------------------------------ fenced writes

/// ★ The gap: a holder taken over while it stalled writes on the authority it lost. Every
/// one of its writes, fenced with the epoch it was given, is refused and writes nothing.
#[test]
fn each_write_with_a_stale_epoch_is_refused_and_writes_nothing() {
    for (what, verb, iri, args) in fenced_writes("1") {
        let kernel = kernel();
        claimed_item(&kernel);
        take_over(&kernel, "worker-1", "worker-2");
        let before = graph(&kernel);
        stale(run(&kernel, verb, iri, &args), "worker-2", 2, what);
        assert_eq!(graph(&kernel), before, "{what} wrote something");
    }
}

/// And after a release, with nobody holding the item, an epoch is not current either.
#[test]
fn each_write_with_the_epoch_of_a_released_claim_is_refused() {
    for (what, verb, iri, args) in fenced_writes("1") {
        let kernel = kernel();
        claimed_item(&kernel);
        delete(&kernel, CLAIM, &[("item", "#1")]);
        let before = graph(&kernel);
        match run(&kernel, verb, iri, &args) {
            Err(Error::Conflict(said)) => assert!(
                said.contains(
                    "is not claimed (its last claim was epoch 1), so epoch 1 is not current"
                ),
                "{what}: {said}"
            ),
            other => panic!("{what}: expected Conflict, got {other:?}"),
        }
        assert_eq!(graph(&kernel), before, "{what} wrote something");
    }
}

#[test]
fn each_write_with_the_current_epoch_succeeds() {
    for (what, verb, iri, args) in fenced_writes("2") {
        let kernel = kernel();
        claimed_item(&kernel);
        // worker-2's claim, at epoch 2: the renewal in the list renews it as worker-1, so it
        // is taken back by worker-1 here.
        take_over(&kernel, "worker-1", "worker-1");
        assert_eq!(epoch(&kernel), Some(2));
        let said = run(&kernel, verb, iri, &args).unwrap_or_else(|e| panic!("{what}: {e}"));
        let json = item_json(&kernel);
        match what {
            "comment" => assert!(said.starts_with("commented on #1 ("), "{said}"),
            "hand-back" => {
                assert!(said.ends_with(" awaiting-dispatcher\n"), "{said}");
                assert_eq!(json["item"]["awaiting"], "dispatcher", "{json}");
            }
            "close" => {
                assert_eq!(said, "closed #1 (done), released worker-1\n");
                assert_eq!(json["item"]["status"], "closed");
                assert!(json["item"]["claim"].is_null());
            }
            "renewal" => {
                assert!(
                    said.starts_with("#1 renewed by worker-1 at epoch 2 until "),
                    "{said}"
                );
                assert_eq!(epoch(&kernel), Some(2));
            }
            "release" => {
                assert_eq!(said, "#1 released\n");
                assert!(json["item"]["claim"].is_null());
            }
            _ => unreachable!(),
        }
    }
}

/// Backward compatible: without `epoch=`, a stale holder's comment lands as it always did.
#[test]
fn without_an_epoch_nothing_changes() {
    let kernel = kernel();
    claimed_item(&kernel);
    take_over(&kernel, "worker-1", "worker-2");
    let said = sink(
        &kernel,
        "urn:iki:ledger:comment",
        &[
            ("item", "#1"),
            ("content", "still here"),
            ("handback", "true"),
        ],
    );
    assert!(said.ends_with(" awaiting-dispatcher\n"), "{said}");
    delete(&kernel, CLAIM, &[("item", "#1")]);
    assert_eq!(holder(&kernel), None);
}

#[test]
fn a_fenced_takeover_names_the_epoch_it_takes_over() {
    let kernel = kernel();
    claimed_item(&kernel);
    let args = |epoch: &'static str| {
        [
            ("item", "#1"),
            ("content", "worker-2"),
            ("takeover", "true"),
            ("from", "worker-1"),
            ("epoch", epoch),
        ]
    };
    stale(
        try_sink(&kernel, CLAIM, &args("2")),
        "worker-1",
        1,
        "takeover",
    );
    let said = try_sink(&kernel, CLAIM, &args("1")).unwrap();
    assert!(
        said.starts_with("#1 taken over from worker-1 by worker-2 at epoch 2"),
        "{said}"
    );
}

#[test]
fn a_malformed_epoch_is_refused_before_anything_is_written() {
    let kernel = kernel();
    claimed_item(&kernel);
    let before = graph(&kernel);
    for (what, verb, iri, args) in fenced_writes("x") {
        let refused = run(&kernel, verb, iri, &args);
        assert!(
            matches!(refused, Err(Error::InvalidArgument { ref name, .. }) if name == "epoch"),
            "{what}: {refused:?}"
        );
    }
    assert_eq!(graph(&kernel), before);
}

// ------------------------------------------------------------------ races, forced

/// How the next live-graph updates are held back (`tests/close_race.rs`'s schedule).
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

/// ★ Two (here four) takeovers of one expired lease, every one reading the item before any
/// writes: exactly one lands, and the item has exactly one epoch, the next one.
#[test]
fn concurrent_takeovers_leave_exactly_one_current_epoch() {
    const TAKERS: usize = 4;
    let (cell, schedule) = scheduled();
    let kernel = cell.get().unwrap();
    let item = claimed_item(kernel);
    schedule.width.store(TAKERS, Ordering::SeqCst);
    *schedule.gated.lock().unwrap() = TAKERS;
    let takers: Vec<String> = (2..2 + TAKERS).map(|k| format!("worker-{k}")).collect();
    let root = Capability::root();
    let answers = block_on(futures::future::join_all(takers.iter().map(|taker| {
        kernel.issue(
            request(
                Verb::Sink,
                CLAIM,
                &[
                    ("item", "#1"),
                    ("content", taker),
                    ("takeover", "true"),
                    ("from", "worker-1"),
                ],
            ),
            &root,
        )
    })));
    assert_eq!(
        schedule.arrived.load(Ordering::SeqCst),
        TAKERS,
        "every takeover reached its update before any wrote"
    );
    let won: Vec<String> = answers
        .iter()
        .filter_map(|a| a.as_ref().ok())
        .map(|r| String::from_utf8_lossy(&r.bytes).into_owned())
        .collect();
    assert_eq!(won.len(), 1, "{answers:?}");
    assert!(won[0].contains(" at epoch 2"), "{won:?}");
    assert!(
        answers
            .iter()
            .filter(|a| a.is_err())
            .all(|a| matches!(a, Err(Error::Conflict(_)))),
        "{answers:?}"
    );
    assert_eq!(stored_epochs(kernel, &item), ["2"]);
    assert_eq!(
        values(
            kernel,
            &format!("SELECT ?v WHERE {{ GRAPH <{LIVE}> {{ <{item}> <{NS}claimedBy> ?v }} }}"),
        )
        .len(),
        1,
        "one holder"
    );
    let check = source(kernel, CLAIM, &[("item", "#1"), ("epoch", "2")]);
    assert!(
        check.starts_with("#1 epoch 2 is current: held by worker-"),
        "{check}"
    );
}

/// ★ The read cannot decide: each fenced write reads its claim current, and a takeover
/// commits between that read and its update. The fence in the update refuses it, and
/// nothing of it is written.
#[test]
fn a_takeover_between_the_read_and_the_write_refuses_each_fenced_write() {
    for (what, verb, iri, args) in fenced_writes("1") {
        let (cell, schedule) = scheduled();
        let kernel = cell.get().unwrap();
        claimed_item(kernel);
        // worker-1's lease (1s) has expired by now on the ticking clock.
        *schedule.armed.lock().unwrap() = Some(request(
            Verb::Sink,
            CLAIM,
            &[
                ("item", "#1"),
                ("content", "worker-2"),
                ("takeover", "true"),
                ("from", "worker-1"),
            ],
        ));
        // The armed takeover runs inside the fenced write's first live update.
        let result = run(kernel, verb, iri, &args);
        assert!(
            schedule.armed.lock().unwrap().is_none(),
            "{what}: the takeover ran"
        );
        stale(result, "worker-2", 2, what);
        assert_eq!(holder(kernel).as_deref(), Some("worker-2"), "{what}");
        assert_eq!(epoch(kernel), Some(2), "{what}");
        let json = item_json(kernel);
        assert_eq!(json["item"]["status"], "open", "{what}");
        assert!(json["item"]["awaiting"].is_null(), "{what}: {json}");
        let comments = json["item"]["comments"]
            .as_array()
            .map(Vec::len)
            .unwrap_or(0);
        assert_eq!(comments, 0, "{what} wrote a comment: {json}");
    }
}
