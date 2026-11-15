//! Ledger #3711: check the existing cut feed before adding a waiting claim operation.
//! A host can park while an item is held, then read and claim on release. Notifications
//! are hints, not reservations: expiry alone emits none, and a lapse does not release.

mod common;

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

use common::*;
use futures::task::{waker, ArcWake};
use futures::{pin_mut, Future, FutureExt};
use ikigai_core::{
    Capability, Clock, CutListener, Error, Fallback, Kernel, ListenSpec, Space, Time, Verb,
};

const CLAIM: &str = "urn:iki:ledger:claim";
const ITEM: &str = "urn:iki:ledger:item:1";
const GRAPH: &str = "urn:iki:ledger:graph:default";
const T0: u64 = 1_789_430_400_000;

struct HandClock(Arc<AtomicU64>);

impl Clock for HandClock {
    fn now(&self) -> Time {
        Time::from_millis(self.0.load(Ordering::SeqCst))
    }
}

fn fixture() -> (Kernel, Arc<AtomicU64>) {
    let now = Arc::new(AtomicU64::new(T0));
    let space = Fallback::new(vec![
        Arc::new(ikigai_store::space(
            ikigai_store::DurableStore::in_memory().unwrap(),
        )) as Arc<dyn Space>,
        Arc::new(ikigai_ledger::space()) as Arc<dyn Space>,
    ]);
    let kernel = Kernel::new(Arc::new(space)).with_clock(Arc::new(HandClock(now.clone())));
    append(&kernel, "Shared files", &[]);
    sink(
        &kernel,
        CLAIM,
        &[("item", "1"), ("content", "worker-1"), ("lease", "1s")],
    );
    (kernel, now)
}

fn reader() -> Capability {
    Capability::scoped([
        "urn:cap:kernel:listen".to_string(),
        "urn:cap:ledger:read:default".to_string(),
        graph_read("default"),
    ])
}

fn listen(kernel: &Kernel, cap: &Capability) -> CutListener {
    kernel
        .listen(
            ListenSpec::new()
                .exact(ikigai_store::graph_thread(GRAPH))
                .exact(ikigai_store::GRAPH_UPDATE_THREAD)
                .exact(ikigai_store::UPDATE_THREAD)
                .exact(ikigai_store::LOAD_THREAD),
            cap,
        )
        .unwrap()
}

fn read(kernel: &Kernel, cap: &Capability) -> serde_json::Value {
    serde_json::from_str(
        &try_as(
            kernel,
            cap,
            Verb::Source,
            ITEM,
            &[("as", "application/json")],
        )
        .unwrap(),
    )
    .unwrap()
}

#[derive(Default)]
struct WakeCount(AtomicUsize);

impl ArcWake for WakeCount {
    fn wake_by_ref(arc_self: &Arc<Self>) {
        arc_self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn a_held_claim_refuses_repeated_attempts_but_a_feed_wait_parks_until_release() {
    let (kernel, _) = fixture();
    // Reproduce the report's noisy approach: each attempt is another refusal.
    for _ in 0..19 {
        assert!(matches!(
            try_verb(
                &kernel,
                Verb::Sink,
                CLAIM,
                &[("item", "1"), ("content", "worker-2")]
            ),
            Err(Error::Conflict(_))
        ));
    }

    let cap = reader();
    let listener = listen(&kernel, &cap);
    assert_eq!(read(&kernel, &cap)["item"]["claim"]["holder"], "worker-1");
    assert!(listener.drain().is_empty());
    let wakes = Arc::new(WakeCount::default());
    let task_waker = waker(wakes.clone());
    let mut cx = Context::from_waker(&task_waker);
    let wait = listener.wait();
    pin_mut!(wait);
    assert!(wait.as_mut().poll(&mut cx).is_pending());
    assert_eq!(wakes.0.load(Ordering::SeqCst), 0);

    delete(&kernel, CLAIM, &[("item", "1")]);
    assert!(
        wakes.0.load(Ordering::SeqCst) > 0,
        "release wakes the parked task"
    );
    let Poll::Ready(batch) = wait.as_mut().poll(&mut cx) else {
        panic!("release must be queued")
    };
    assert!(batch
        .events
        .iter()
        .any(|event| event.invalidated.iter().any(|name| name == ITEM)));
    assert!(read(&kernel, &cap)["item"]["claim"].is_null());
    assert!(
        sink(&kernel, CLAIM, &[("item", "1"), ("content", "worker-2")])
            .contains("claimed by worker-2 at epoch 2")
    );
}

#[test]
fn release_between_the_read_and_wait_is_already_queued() {
    let (kernel, _) = fixture();
    let cap = reader();
    let listener = listen(&kernel, &cap);
    assert_eq!(read(&kernel, &cap)["item"]["claim"]["holder"], "worker-1");
    delete(&kernel, CLAIM, &[("item", "1")]);
    // The host has not polled wait yet. Registration before the read closes this gap.
    assert!(!listener
        .wait()
        .now_or_never()
        .expect("release is queued")
        .is_empty());
    assert!(read(&kernel, &cap)["item"]["claim"].is_null());
}

#[test]
fn expiry_is_silent_and_a_lapse_wakes_without_freeing_the_item() {
    let (kernel, now) = fixture();
    let cap = reader();
    let listener = listen(&kernel, &cap);
    read(&kernel, &cap);
    now.store(T0 + 1_000, Ordering::SeqCst);
    assert!(
        listener.drain().is_empty(),
        "time passing does not cut a thread"
    );
    sink(&kernel, "urn:iki:ledger:lapse", &[]);
    assert!(
        !listener.drain().is_empty(),
        "recording a lapse writes the graph"
    );
    assert_eq!(read(&kernel, &cap)["item"]["claim"]["holder"], "worker-1");
    let refusal = try_verb(
        &kernel,
        Verb::Sink,
        CLAIM,
        &[("item", "1"), ("content", "worker-2")],
    )
    .unwrap_err();
    assert!(matches!(refusal, Error::Conflict(_)));
    assert!(refusal.to_string().contains("takeover=true"));
}

#[test]
fn two_waiters_are_both_notified_and_a_notice_does_not_reserve_a_claim() {
    let (kernel, _) = fixture();
    let cap = reader();
    let first = listen(&kernel, &cap);
    let second = listen(&kernel, &cap);
    read(&kernel, &cap);
    delete(&kernel, CLAIM, &[("item", "1")]);
    assert!(!first.drain().is_empty());
    assert!(!second.drain().is_empty());
    // Claim in reverse registration order: the feed orders events, not claimants.
    sink(&kernel, CLAIM, &[("item", "1"), ("content", "worker-3")]);
    assert!(matches!(
        try_verb(
            &kernel,
            Verb::Sink,
            CLAIM,
            &[("item", "1"), ("content", "worker-2")]
        ),
        Err(Error::Conflict(_))
    ));
}

#[test]
fn unrelated_writes_and_close_require_reading_the_item_again() {
    let (kernel, _) = fixture();
    let cap = reader();
    let listener = listen(&kernel, &cap);
    read(&kernel, &cap);
    sink(
        &kernel,
        "urn:iki:ledger:comment",
        &[("item", "1"), ("content", "Still working")],
    );
    assert!(!listener.drain().is_empty());
    assert_eq!(read(&kernel, &cap)["item"]["claim"]["holder"], "worker-1");
    sink(&kernel, "urn:iki:ledger:close", &[("item", "1")]);
    assert!(!listener.drain().is_empty());
    let closed = read(&kernel, &cap);
    assert!(closed["item"]["claim"].is_null());
    assert_eq!(closed["item"]["status"], "closed");
    assert!(matches!(
        try_verb(
            &kernel,
            Verb::Sink,
            CLAIM,
            &[("item", "1"), ("content", "worker-2")]
        ),
        Err(Error::Conflict(_))
    ));
}

#[test]
fn watching_only_the_item_thread_does_not_hear_a_release() {
    let (kernel, _) = fixture();
    let cap = reader();
    let item_only = kernel.listen(ListenSpec::new().exact(ITEM), &cap).unwrap();
    let store_feed = listen(&kernel, &cap);
    read(&kernel, &cap);
    delete(&kernel, CLAIM, &[("item", "1")]);
    assert!(item_only.drain().is_empty());
    assert!(!store_feed.drain().is_empty());
}
