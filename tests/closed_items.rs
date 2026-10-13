//! **Closed work is held by nobody and awaits nobody** (ledger #1418, #1419). Each test
//! failed on 0.11.0 (`8a7ce7c`) because of the defect it names.
//!
//! - A worker's `comment handback=true` on a CLOSED item left it closed and awaiting its
//!   dispatcher: the state the doctor's `awaiting-closed` says only a write around the ledger
//!   leaves, made by the ledger's own door under the published worker grant.
//! - The claim Sink took a closed item, recreating the held-closed state ledger #1147 removed:
//!   listed under `items holder=` again, and reported by the doctor as an orphaned claim.
//!
//! Both are refused now, typed (`Conflict`) and naming the item and its state, and both are
//! refused in the WRITE as well as before it: a close that lands between the read and the
//! write (forced here by a store wrapper, so it happens every run) leaves the item closed,
//! unclaimed and awaiting nobody.

mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use common::*;
use ikigai_core::{
    ArgRef, Capability, Endpoint, Error, Fallback, Invocation, Iri, Kernel, Representation,
    Request, Resolution, Result, Scope, Space, Verb,
};
use ikigai_store::DurableStore;

const LEDGER: &str = "acme";

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

/// The worker grant `README.md` publishes: read, claim, comment.
fn worker() -> Capability {
    grant(&["read", "claim", "comment"])
}

fn item_json(kernel: &Kernel) -> serde_json::Value {
    let view = source(kernel, &at("item:1"), &[("as", "application/json")]);
    serde_json::from_str::<serde_json::Value>(&view).expect("JSON")["item"].clone()
}

fn comments(kernel: &Kernel) -> usize {
    item_json(kernel)["comments"]
        .as_array()
        .expect("comments")
        .len()
}

fn closed_item(kernel: &Kernel) {
    sink(kernel, &at("append"), &[("content", "Fix the widget")]);
    sink(kernel, &at("close"), &[("item", "1")]);
}

fn assert_closed_refusal(answer: &std::result::Result<String, Error>, what: &str) {
    let Err(Error::Conflict(message)) = answer else {
        panic!("{what} on a closed item: {answer:?}");
    };
    assert!(
        message.contains("acme#1") && message.contains("closed"),
        "the refusal names the item and its state: {message}"
    );
}

// --------------------------------------------------------------------- hand-back (#1418)

/// ★ Refused, and nothing written: no comment, no `awaiting`, and the doctor has nothing to
/// report.
#[test]
fn a_hand_back_on_a_closed_item_is_refused() {
    let kernel = kernel();
    closed_item(&kernel);
    let before = comments(&kernel);
    let answer = try_as(
        &kernel,
        &worker(),
        Verb::Sink,
        &at("comment"),
        &[("item", "1"), ("content", "evidence"), ("handback", "true")],
    );
    assert_closed_refusal(&answer, "comment handback=true");
    let item = item_json(&kernel);
    assert_eq!(item["status"], "closed", "{item}");
    assert_eq!(item["awaiting"], serde_json::Value::Null, "{item}");
    assert_eq!(
        comments(&kernel),
        before,
        "a refused hand-back wrote its comment"
    );
    let doctor = source(&kernel, &at("doctor"), &[]);
    assert!(doctor.contains("no problems"), "{doctor}");
}

/// An ordinary comment on a closed item is still a comment: the refusal is the hand-back's.
#[test]
fn an_ordinary_comment_on_a_closed_item_is_still_written() {
    let kernel = kernel();
    closed_item(&kernel);
    let before = comments(&kernel);
    try_as(
        &kernel,
        &worker(),
        Verb::Sink,
        &at("comment"),
        &[("item", "1"), ("content", "a note after the fact")],
    )
    .expect("an ordinary comment on a closed item");
    assert_eq!(comments(&kernel), before + 1);
}

// ------------------------------------------------------------------------ claim (#1419)

/// ★ Refused, and the item stays held by nobody: not under `items holder=`, nothing for the
/// doctor.
#[test]
fn a_closed_item_cannot_be_claimed() {
    let kernel = kernel();
    closed_item(&kernel);
    let answer = try_as(
        &kernel,
        &worker(),
        Verb::Sink,
        &at("claim"),
        &[("item", "1"), ("content", "worker-7")],
    );
    assert_closed_refusal(&answer, "claim");
    assert_eq!(item_json(&kernel)["claim"], serde_json::Value::Null);
    let held = source(
        &kernel,
        &at("items"),
        &[("holder", "worker-7"), ("status", "all")],
    );
    assert!(held.starts_with("no items match"), "{held}");
    let doctor = source(&kernel, &at("doctor"), &[]);
    assert!(doctor.contains("no problems"), "{doctor}");
}

/// Reopened, it can be claimed again: the refusal is about the state, not the item.
#[test]
fn a_reopened_item_can_be_claimed() {
    let kernel = kernel();
    closed_item(&kernel);
    sink(&kernel, &at("reopen"), &[("item", "1")]);
    try_as(
        &kernel,
        &worker(),
        Verb::Sink,
        &at("claim"),
        &[("item", "1"), ("content", "worker-7")],
    )
    .expect("a claim on a reopened item");
}

// ------------------------------------------------------- a close between read and write

/// Three race kernels, one per test, each wrapped so that the write carrying `MARKERS[i]`
/// lands only after a close of `acme#1` has: the interleaving a concurrent close produces at
/// the endpoint's await points, forced so it happens every time.
static RACE_KERNELS: [OnceLock<Kernel>; 3] = [OnceLock::new(), OnceLock::new(), OnceLock::new()];
static ARMED: [AtomicBool; 3] = [
    AtomicBool::new(false),
    AtomicBool::new(false),
    AtomicBool::new(false),
];
const MARKERS: [&str; 3] = [
    "\"worker-7\"",
    "urn:iki:ledger:awaiting:dispatcher",
    "urn:iki:ledger:awaiting:dispatcher",
];
const CLAIM: usize = 0;
const HANDBACK: usize = 1;
const KEYED_HANDBACK: usize = 2;

struct CloseFirst(Arc<dyn Space>, usize);
struct CloseFirstEndpoint(Arc<dyn Endpoint>, usize);

impl Space for CloseFirst {
    fn resolve(&self, request: &Request, scope: &Scope) -> Resolution {
        let which = self.1;
        self.0
            .resolve(request, scope)
            .map_endpoint(move |endpoint| {
                Arc::new(CloseFirstEndpoint(endpoint, which)) as Arc<dyn Endpoint>
            })
    }
}

#[async_trait]
impl Endpoint for CloseFirstEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let marked = inv.request.target.as_str() == "urn:iki:store:graph-update"
            && inv
                .inline_str("content")
                .map(|update| update.contains(MARKERS[self.1]))
                .unwrap_or(false);
        if marked && ARMED[self.1].swap(false, Ordering::SeqCst) {
            let kernel = RACE_KERNELS[self.1].get().expect("the race kernel");
            kernel
                .issue(
                    Request::new(Verb::Sink, Iri::parse(at("close")).expect("an IRI"))
                        .with_arg("item", ArgRef::Inline(b"1".to_vec())),
                    &Capability::root(),
                )
                .await?;
        }
        self.0.invoke(inv).await
    }

    fn name(&self) -> &str {
        self.0.name()
    }

    fn describe(&self) -> ikigai_core::Description {
        self.0.describe()
    }
}

fn race_kernel(which: usize) -> &'static Kernel {
    let store = Arc::new(ikigai_store::space(DurableStore::in_memory().unwrap())) as Arc<dyn Space>;
    let space = Fallback::new(vec![
        Arc::new(CloseFirst(store, which)) as Arc<dyn Space>,
        Arc::new(ikigai_ledger::space()) as Arc<dyn Space>,
    ]);
    let kernel =
        Kernel::with_meta_renderer(Arc::new(space), Arc::new(ikigai_vocab::TurtleRenderer))
            .with_clock(Arc::new(TickingClock::default()));
    assert!(RACE_KERNELS[which].set(kernel).is_ok());
    RACE_KERNELS[which].get().unwrap()
}

/// ★ The claim read the item open, and a close landed before its write: the write's own
/// guard refuses it, so the item is closed and held by nobody, and the answer says why.
#[test]
fn a_claim_racing_a_close_does_not_land() {
    let kernel = race_kernel(CLAIM);
    sink(kernel, &at("append"), &[("content", "Fix the widget")]);
    ARMED[CLAIM].store(true, Ordering::SeqCst);
    let answer = try_as(
        kernel,
        &worker(),
        Verb::Sink,
        &at("claim"),
        &[("item", "1"), ("content", "worker-7")],
    );
    assert!(!ARMED[CLAIM].load(Ordering::SeqCst), "the close ran first");
    let item = item_json(kernel);
    assert_eq!(item["status"], "closed", "{item}");
    assert_eq!(
        item["claim"],
        serde_json::Value::Null,
        "a claim landed on closed work: {item}"
    );
    assert_closed_refusal(&answer, "a claim racing a close");
}

/// ★ The hand-back read the item open, and a close landed before its write: neither the
/// comment nor the `awaiting` lands (they land together or not at all), and the answer says
/// why.
#[test]
fn a_hand_back_racing_a_close_does_not_land() {
    let kernel = race_kernel(HANDBACK);
    sink(kernel, &at("append"), &[("content", "Fix the widget")]);
    let before = comments(kernel);
    ARMED[HANDBACK].store(true, Ordering::SeqCst);
    let answer = try_as(
        kernel,
        &worker(),
        Verb::Sink,
        &at("comment"),
        &[("item", "1"), ("content", "evidence"), ("handback", "true")],
    );
    assert!(
        !ARMED[HANDBACK].load(Ordering::SeqCst),
        "the close ran first"
    );
    let item = item_json(kernel);
    assert_eq!(item["status"], "closed", "{item}");
    assert_eq!(item["awaiting"], serde_json::Value::Null, "{item}");
    assert_eq!(
        comments(kernel),
        before,
        "the hand-back's comment landed alone"
    );
    assert_closed_refusal(&answer, "a hand-back racing a close");
}

/// The same with a key: the keyed comment's own guard ("the key is free") and the hand-back's
/// ("the item is open") are one condition, so neither the comment nor its key lands.
#[test]
fn a_keyed_hand_back_racing_a_close_does_not_land() {
    let kernel = race_kernel(KEYED_HANDBACK);
    sink(kernel, &at("append"), &[("content", "Fix the widget")]);
    let before = comments(kernel);
    ARMED[KEYED_HANDBACK].store(true, Ordering::SeqCst);
    let answer = try_as(
        kernel,
        &worker(),
        Verb::Sink,
        &at("comment"),
        &[
            ("item", "1"),
            ("content", "evidence"),
            ("handback", "true"),
            ("key", "evidence-1"),
        ],
    );
    assert!(
        !ARMED[KEYED_HANDBACK].load(Ordering::SeqCst),
        "the close ran first"
    );
    let item = item_json(kernel);
    assert_eq!(item["status"], "closed", "{item}");
    assert_eq!(item["awaiting"], serde_json::Value::Null, "{item}");
    assert_eq!(
        comments(kernel),
        before,
        "the hand-back's comment landed alone"
    );
    assert_closed_refusal(&answer, "a keyed hand-back racing a close");
}
