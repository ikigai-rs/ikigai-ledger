//! A held claim must park a waiting claimant until release, rather than refuse it.
mod common;

use common::*;
use futures::{executor::block_on, FutureExt};
use ikigai_core::{ArgRef, Capability, Fallback, FixedClock, Iri, Kernel, Request, Space, Verb};
use std::sync::Arc;

fn waiting_kernel() -> Kernel {
    Kernel::new(Arc::new(Fallback::new(vec![
        Arc::new(ikigai_store::space(
            ikigai_store::DurableStore::in_memory().unwrap(),
        )) as Arc<dyn Space>,
        Arc::new(ikigai_ledger::space()) as Arc<dyn Space>,
    ])))
    .with_clock(Arc::new(FixedClock::at(1_789_430_400_000)))
}

fn waiting_request(holder: &str) -> Request {
    Request::new(Verb::Sink, Iri::parse("urn:iki:ledger:claim").unwrap())
        .with_arg("item", ArgRef::Inline(b"1".to_vec()))
        .with_arg("content", ArgRef::Inline(holder.as_bytes().to_vec()))
        .with_arg("wait", ArgRef::Inline(b"5".to_vec()))
}

#[test]
fn a_waiting_claim_parks_then_takes_the_released_item() {
    let kernel = waiting_kernel();
    append(&kernel, "Shared files", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:claim",
        &[("item", "1"), ("content", "first")],
    );
    let cap = Capability::root();
    let mut waiting = Box::pin(kernel.issue(waiting_request("second"), &cap));
    assert!(
        waiting.as_mut().now_or_never().is_none(),
        "a held claim must wait, not refuse"
    );
    delete(
        &kernel,
        "urn:iki:ledger:claim",
        &[("item", "1"), ("from", "first")],
    );
    let answer = block_on(waiting).unwrap();
    assert!(String::from_utf8_lossy(&answer.bytes).contains("claimed by second at epoch 2"));
}

#[test]
fn waiters_take_turns_in_registration_order_even_when_polled_in_reverse() {
    let kernel = waiting_kernel();
    append(&kernel, "Shared files", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:claim",
        &[("item", "1"), ("content", "first")],
    );
    let cap = Capability::root();
    let mut second = Box::pin(kernel.issue(waiting_request("second"), &cap));
    let mut third = Box::pin(kernel.issue(waiting_request("third"), &cap));
    assert!(second.as_mut().now_or_never().is_none());
    assert!(third.as_mut().now_or_never().is_none());
    delete(
        &kernel,
        "urn:iki:ledger:claim",
        &[("item", "1"), ("from", "first")],
    );
    assert!(third.as_mut().now_or_never().is_none());
    block_on(second).unwrap();
    assert!(third.as_mut().now_or_never().is_none());
    delete(
        &kernel,
        "urn:iki:ledger:claim",
        &[("item", "1"), ("from", "second")],
    );
    let answer = block_on(third).unwrap();
    assert!(String::from_utf8_lossy(&answer.bytes).contains("claimed by third at epoch 3"));
}

#[test]
fn canceling_the_first_waiter_does_not_strand_the_next() {
    let kernel = waiting_kernel();
    append(&kernel, "Shared files", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:claim",
        &[("item", "1"), ("content", "first")],
    );
    let cap = Capability::root();
    let mut canceled = Box::pin(kernel.issue(waiting_request("second"), &cap));
    let mut next = Box::pin(kernel.issue(waiting_request("third"), &cap));
    assert!(canceled.as_mut().now_or_never().is_none());
    assert!(next.as_mut().now_or_never().is_none());
    drop(canceled);
    delete(
        &kernel,
        "urn:iki:ledger:claim",
        &[("item", "1"), ("from", "first")],
    );
    let answer = block_on(next).unwrap();
    assert!(String::from_utf8_lossy(&answer.bytes).contains("claimed by third at epoch 2"));
}

#[test]
fn release_wakes_the_registered_executor() {
    use futures::task::{waker_ref, ArcWake};
    use std::future::Future;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Context, Poll};
    #[derive(Default)]
    struct Wakes(AtomicUsize);
    impl ArcWake for Wakes {
        fn wake_by_ref(this: &Arc<Self>) {
            this.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let kernel = waiting_kernel();
    append(&kernel, "Shared files", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:claim",
        &[("item", "1"), ("content", "first")],
    );
    let cap = Capability::root();
    let mut waiting = Box::pin(kernel.issue(waiting_request("second"), &cap));
    let wakes = Arc::new(Wakes::default());
    let waker = waker_ref(&wakes);
    assert!(matches!(
        waiting.as_mut().poll(&mut Context::from_waker(&waker)),
        Poll::Pending
    ));
    let before = wakes.0.load(Ordering::SeqCst);
    delete(
        &kernel,
        "urn:iki:ledger:claim",
        &[("item", "1"), ("from", "first")],
    );
    assert!(
        wakes.0.load(Ordering::SeqCst) > before,
        "release must wake the parked executor"
    );
    block_on(waiting).unwrap();
}

#[test]
fn closure_and_deletion_wake_waiters_without_claiming() {
    use ikigai_core::Error;
    for (verb, target) in [
        (Verb::Sink, "urn:iki:ledger:close"),
        (Verb::Delete, "urn:iki:ledger:item:1"),
    ] {
        let kernel = waiting_kernel();
        append(&kernel, "Shared files", &[]);
        sink(
            &kernel,
            "urn:iki:ledger:claim",
            &[("item", "1"), ("content", "first")],
        );
        let cap = Capability::root();
        let mut waiting = Box::pin(kernel.issue(waiting_request("second"), &cap));
        assert!(waiting.as_mut().now_or_never().is_none());
        ok(&kernel, verb, target, &[("item", "1")]);
        let result = block_on(waiting);
        if verb == Verb::Sink {
            assert!(matches!(result, Err(Error::Conflict(_))), "{result:?}");
        } else {
            assert!(matches!(result, Err(Error::NotFound(_))), "{result:?}");
        }
    }
}

#[test]
fn a_shorter_caller_deadline_expires_and_removes_its_waiter() {
    use ikigai_core::{Error, IssueContext};
    use std::time::{Duration, Instant};
    let kernel = waiting_kernel();
    append(&kernel, "Shared files", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:claim",
        &[("item", "1"), ("content", "first")],
    );
    let cap = Capability::root();
    // The fixed kernel clock is deliberate: the real timer must bound the wait too.
    let context =
        IssueContext::new().with_not_after(ikigai_core::Time::from_millis(1_789_430_400_200));
    let start = Instant::now();
    let mut first = Box::pin(kernel.issue_with_context(waiting_request("second"), &cap, context));
    assert!(first.as_mut().now_or_never().is_none());
    let mut next = Box::pin(kernel.issue(waiting_request("third"), &cap));
    assert!(next.as_mut().now_or_never().is_none());
    let result = block_on(first);
    assert!(
        matches!(result, Err(Error::OverBudget { .. })),
        "{result:?}"
    );
    assert!(start.elapsed() < Duration::from_secs(3));
    delete(
        &kernel,
        "urn:iki:ledger:claim",
        &[("item", "1"), ("from", "first")],
    );
    let answer = block_on(next).unwrap();
    assert!(String::from_utf8_lossy(&answer.bytes).contains("claimed by third at epoch 2"));
}

fn live_kernel() -> Kernel {
    Kernel::new(Arc::new(Fallback::new(vec![
        Arc::new(ikigai_store::space(
            ikigai_store::DurableStore::in_memory().unwrap(),
        )) as Arc<dyn Space>,
        Arc::new(ikigai_ledger::space()) as Arc<dyn Space>,
    ])))
    .with_clock(Arc::new(ikigai_core::SystemClock))
}

#[test]
fn a_waiting_claim_wakes_at_lease_expiry_and_takes_over_with_a_new_epoch() {
    use std::time::{Duration, Instant};
    let kernel = live_kernel();
    append(&kernel, "Shared files", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:claim",
        &[("item", "1"), ("content", "first"), ("lease", "1s")],
    );
    let cap = Capability::root();
    let mut waiting = Box::pin(kernel.issue(waiting_request("second"), &cap));
    assert!(waiting.as_mut().now_or_never().is_none());
    let start = Instant::now();
    let answer = block_on(waiting).unwrap();
    assert!(start.elapsed() < Duration::from_secs(4));
    assert!(String::from_utf8_lossy(&answer.bytes)
        .contains("taken over from first by second at epoch 2"));
}

#[test]
fn renewal_moves_the_expiry_a_waiter_observes() {
    use std::time::Duration;
    let kernel = live_kernel();
    append(&kernel, "Shared files", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:claim",
        &[("item", "1"), ("content", "first"), ("lease", "1s")],
    );
    let cap = Capability::root();
    let mut waiting = Box::pin(kernel.issue(waiting_request("second"), &cap));
    assert!(waiting.as_mut().now_or_never().is_none());
    sink(
        &kernel,
        "urn:iki:ledger:claim",
        &[
            ("item", "1"),
            ("content", "first"),
            ("renew", "true"),
            ("lease", "3s"),
        ],
    );
    std::thread::sleep(Duration::from_millis(1100));
    assert!(
        waiting.as_mut().now_or_never().is_none(),
        "renewal must prevent takeover at the old expiry"
    );
    delete(
        &kernel,
        "urn:iki:ledger:claim",
        &[("item", "1"), ("from", "first")],
    );
    block_on(waiting).unwrap();
}

#[test]
fn the_requested_wait_expires_even_without_a_caller_deadline() {
    use ikigai_core::Error;
    use std::time::{Duration, Instant};
    let kernel = waiting_kernel();
    append(&kernel, "Shared files", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:claim",
        &[("item", "1"), ("content", "first")],
    );
    let cap = Capability::root();
    let request = waiting_request("second").with_arg("wait", ArgRef::Inline(b"1".to_vec()));
    let start = Instant::now();
    let result = block_on(kernel.issue(request, &cap));
    assert!(
        matches!(result, Err(Error::OverBudget { .. })),
        "{result:?}"
    );
    if let Err(Error::OverBudget { ref detail }) = result {
        assert_eq!(detail, "the claim wait reached its wait limit or the caller's deadline; the waiter left the queue, so read the item before trying again");
    }
    assert!(start.elapsed() >= Duration::from_millis(900));
    assert!(start.elapsed() < Duration::from_secs(4));
    let item = source(
        &kernel,
        "urn:iki:ledger:item:1",
        &[("as", "application/json")],
    );
    assert!(item.contains("first"));
    assert!(!item.contains("second"));
}

#[test]
fn malformed_and_incompatible_waits_are_refused_before_parking() {
    use ikigai_core::Error;
    let kernel = waiting_kernel();
    append(&kernel, "Shared files", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:claim",
        &[("item", "1"), ("content", "first")],
    );
    let cap = Capability::root();
    for value in ["-1", "1.5", "3601", "", "soon"] {
        let request =
            waiting_request("second").with_arg("wait", ArgRef::Inline(value.as_bytes().to_vec()));
        let result = block_on(kernel.issue(request, &cap));
        assert!(
            matches!(result, Err(Error::InvalidArgument { ref name, .. }) if name == "wait"),
            "{result:?}"
        );
        if let Err(Error::InvalidArgument { detail, .. }) = result {
            assert_eq!(detail, format!("`{value}` is not a whole number of seconds from 0 to 3600; use 0 to refuse immediately or a positive wait bounded by the caller's deadline"));
        }
    }
    for (arg, value) in [
        ("renew", "true"),
        ("takeover", "true"),
        ("from", "first"),
        ("epoch", "1"),
    ] {
        let request =
            waiting_request("second").with_arg(arg, ArgRef::Inline(value.as_bytes().to_vec()));
        let result = block_on(kernel.issue(request, &cap));
        assert!(
            matches!(result, Err(Error::InvalidArgument { ref name, .. }) if name == "wait"),
            "{result:?}"
        );
        if let Err(Error::InvalidArgument { detail, .. }) = result {
            assert_eq!(detail, "a positive `wait` takes the next available claim; omit renew, takeover, from, and epoch, or use wait=0 for an immediate claim operation");
        }
    }
    let request = waiting_request("second").with_arg("wait", ArgRef::Inline(b"0".to_vec()));
    assert!(matches!(
        block_on(kernel.issue(request, &cap)),
        Err(Error::Conflict(_))
    ));
}

#[test]
fn item_references_share_one_queue_and_other_items_remain_available() {
    let kernel = waiting_kernel();
    let item_iri = append_iri(&kernel, "Shared files", &[("key", "mutex")]);
    append(&kernel, "Other files", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:claim",
        &[("item", "1"), ("content", "first")],
    );
    let cap = Capability::root();
    let second_request =
        waiting_request("second").with_arg("item", ArgRef::Inline(b"key:mutex".to_vec()));
    let third_request =
        waiting_request("third").with_arg("item", ArgRef::Inline(item_iri.as_bytes().to_vec()));
    let mut second = Box::pin(kernel.issue(second_request, &cap));
    let mut third = Box::pin(kernel.issue(third_request, &cap));
    assert!(second.as_mut().now_or_never().is_none());
    assert!(third.as_mut().now_or_never().is_none());
    let other = waiting_request("independent").with_arg("item", ArgRef::Inline(b"2".to_vec()));
    block_on(kernel.issue(other, &cap)).unwrap();
    delete(
        &kernel,
        "urn:iki:ledger:claim",
        &[("item", "1"), ("from", "first")],
    );
    assert!(third.as_mut().now_or_never().is_none());
    block_on(second).unwrap();
    delete(
        &kernel,
        "urn:iki:ledger:claim",
        &[("item", "1"), ("from", "second")],
    );
    block_on(third).unwrap();
}

#[test]
fn waiting_under_the_same_holder_name_does_not_bypass_principal_ownership() {
    let kernel = waiting_kernel();
    append(&kernel, "Shared files", &[]);
    let grants = Capability::scoped([
        "urn:cap:ledger:claim:default".to_string(),
        graph_read("default"),
        graph_write("default"),
    ]);
    let first = grants.with_principal("urn:worker:first").unwrap();
    let second = grants.with_principal("urn:worker:second").unwrap();
    try_as(
        &kernel,
        &first,
        Verb::Sink,
        "urn:iki:ledger:claim",
        &[("item", "1"), ("content", "shared-name")],
    )
    .unwrap();
    let mut waiting = Box::pin(kernel.issue(waiting_request("shared-name"), &second));
    assert!(waiting.as_mut().now_or_never().is_none());
    try_as(
        &kernel,
        &first,
        Verb::Delete,
        "urn:iki:ledger:claim",
        &[("item", "1")],
    )
    .unwrap();
    let answer = block_on(waiting).unwrap();
    assert!(String::from_utf8_lossy(&answer.bytes).contains("at epoch 2"));
    let item = source(
        &kernel,
        "urn:iki:ledger:item:1",
        &[("as", "application/json")],
    );
    assert!(item.contains("urn:worker:second"));
}

#[test]
fn the_current_holder_can_extend_its_lease_while_a_waiter_is_queued() {
    let kernel = waiting_kernel();
    append(&kernel, "Shared files", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:claim",
        &[("item", "1"), ("content", "first")],
    );
    let cap = Capability::root();
    let mut waiting = Box::pin(kernel.issue(waiting_request("second"), &cap));
    assert!(waiting.as_mut().now_or_never().is_none());
    let mine = waiting_request("first").with_arg("lease", ArgRef::Inline(b"1h".to_vec()));
    let answer = block_on(kernel.issue(mine, &cap)).unwrap();
    assert!(String::from_utf8_lossy(&answer.bytes).contains("claimed by first at epoch 1"));
    assert!(waiting.as_mut().now_or_never().is_none());
    delete(
        &kernel,
        "urn:iki:ledger:claim",
        &[("item", "1"), ("from", "first")],
    );
    block_on(waiting).unwrap();
}

#[test]
fn an_expired_wait_does_not_acquire_when_a_release_is_also_ready() {
    use ikigai_core::{Error, IssueContext, Time};
    use std::time::Duration;
    let kernel = waiting_kernel();
    append(&kernel, "Shared files", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:claim",
        &[("item", "1"), ("content", "first")],
    );
    let cap = Capability::root();
    let context = IssueContext::new().with_not_after(Time::from_millis(1_789_430_400_200));
    let mut waiting = Box::pin(kernel.issue_with_context(waiting_request("second"), &cap, context));
    assert!(waiting.as_mut().now_or_never().is_none());
    std::thread::sleep(Duration::from_millis(300));
    delete(
        &kernel,
        "urn:iki:ledger:claim",
        &[("item", "1"), ("from", "first")],
    );
    let result = block_on(waiting);
    assert!(
        matches!(result, Err(Error::OverBudget { .. })),
        "{result:?}"
    );
    let item = source(
        &kernel,
        "urn:iki:ledger:item:1",
        &[("as", "application/json")],
    );
    let item: serde_json::Value = serde_json::from_str(&item).unwrap();
    assert!(item["item"]["claim"].is_null());
}
