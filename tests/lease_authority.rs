//! One store, three independently clocked kernels.
mod common;

use common::*;
use ikigai_core::{Capability, Clock, Fallback, Iri, Kernel, Request, Space, Time, Verb};
use ikigai_store::DurableStore;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

const T0: u64 = 1_789_430_400_000;
const MINUTE: u64 = 60_000;

struct HandClock(AtomicU64);
impl Clock for HandClock {
    fn now(&self) -> Time {
        Time::from_millis(self.0.load(Ordering::SeqCst))
    }
}

fn host(store: Arc<dyn Space>, clock: Arc<HandClock>) -> Kernel {
    let spaces = Fallback::new(vec![
        store,
        Arc::new(ikigai_ledger::space()) as Arc<dyn Space>,
    ]);
    Kernel::with_meta_renderer(Arc::new(spaces), Arc::new(ikigai_vocab::TurtleRenderer))
        .with_clock(clock)
}

fn setup() -> (Kernel, Kernel, Arc<HandClock>) {
    let authority = Arc::new(HandClock(AtomicU64::new(T0)));
    let store = store_with_clock(DurableStore::in_memory().unwrap(), authority.clone());
    let grantor = host(store.clone(), authority.clone());
    let fast = host(
        store.clone(),
        Arc::new(HandClock(AtomicU64::new(T0 + 10 * MINUTE))),
    );
    let slow = host(store, Arc::new(HandClock(AtomicU64::new(T0 - 10 * MINUTE))));
    append(&grantor, "Shared lease", &[]);
    sink(
        &grantor,
        "urn:iki:ledger:claim",
        &[("item", "1"), ("content", "worker-1"), ("lease", "1m")],
    );
    (fast, slow, authority)
}

#[test]
fn fast_observer_does_not_see_a_live_lease_as_expired() {
    let (fast, _, _) = setup();
    let answer = source(&fast, "urn:iki:ledger:stalled", &[]);
    assert!(answer.contains("no stalled items"), "{answer}");
}

#[test]
fn slow_observer_sees_expiry_when_the_authority_reaches_it() {
    let (_, slow, authority) = setup();
    authority.0.store(T0 + MINUTE, Ordering::SeqCst);
    let answer = source(&slow, "urn:iki:ledger:stalled", &[]);
    assert!(answer.contains("Shared lease"), "{answer}");
}

#[test]
fn slow_observer_cannot_cache_past_authoritative_expiry() {
    let (_, slow, authority) = setup();
    let iri = "urn:iki:ledger:stalled";
    let live = source(&slow, iri, &[]);
    assert!(live.contains("no stalled items"), "{live}");
    authority.0.store(T0 + MINUTE, Ordering::SeqCst);
    let request = Request::new(Verb::Source, Iri::parse(iri).unwrap());
    assert!(!slow.is_cached(&request, &Capability::root()));
    let expired = source(&slow, iri, &[]);
    assert!(expired.contains("Shared lease"), "{expired}");
}

#[test]
fn grants_and_renewals_ignore_the_writers_clock() {
    let (fast, slow, authority) = setup();
    append(&fast, "Another lease", &[]);
    let initial = sink(
        &fast,
        "urn:iki:ledger:claim",
        &[("item", "2"), ("content", "worker-2"), ("lease", "1m")],
    );
    assert!(
        initial.contains("epoch 1 until 2026-09-15T00:01:00.000Z"),
        "{initial}"
    );
    authority.0.store(T0 + 30_000, Ordering::SeqCst);
    let renewed = sink(
        &slow,
        "urn:iki:ledger:claim",
        &[
            ("item", "2"),
            ("content", "worker-2"),
            ("lease", "1m"),
            ("renew", "true"),
            ("epoch", "1"),
        ],
    );
    assert!(
        renewed.contains("epoch 1 until 2026-09-15T00:01:30.000Z"),
        "{renewed}"
    );
}

#[test]
fn lapse_and_takeover_follow_store_time_and_takeover_keeps_fencing() {
    let (fast, slow, authority) = setup();
    let claim = "urn:iki:ledger:claim";
    let takeover = [
        ("item", "1"),
        ("content", "worker-2"),
        ("takeover", "true"),
        ("from", "worker-1"),
        ("epoch", "1"),
    ];
    assert!(matches!(
        try_verb(&fast, Verb::Sink, claim, &takeover),
        Err(ikigai_core::Error::Conflict(_))
    ));
    let early = sink(&fast, "urn:iki:ledger:lapse", &[]);
    assert!(early.contains("no lapse to record"), "{early}");

    authority.0.store(T0 + MINUTE, Ordering::SeqCst);
    let lapse = sink(&slow, "urn:iki:ledger:lapse", &[]);
    assert!(lapse.contains("recorded 1 lapse"), "{lapse}");
    let taken = sink(&slow, claim, &takeover);
    assert!(taken.contains("at epoch 2"), "{taken}");
    let stale = try_verb(
        &fast,
        Verb::Sink,
        "urn:iki:ledger:comment",
        &[("item", "1"), ("content", "stale work"), ("epoch", "1")],
    );
    assert!(matches!(stale, Err(ikigai_core::Error::Conflict(_))));
}

fn json_source(kernel: &Kernel, iri: &str, args: &[(&str, &str)]) -> serde_json::Value {
    let mut args = args.to_vec();
    args.push(("as", "application/json"));
    serde_json::from_str(&source(kernel, iri, &args)).unwrap()
}

fn has_problem(doc: &serde_json::Value, check: &str) -> bool {
    doc["problems"]
        .as_array()
        .unwrap()
        .iter()
        .any(|problem| problem["check"] == check)
}

#[test]
fn next_and_doctor_check_the_authority_again_without_a_write() {
    let (_, slow, authority) = setup();
    let next = || json_source(&slow, "urn:iki:ledger:next", &[]);
    let doctor = || json_source(&slow, "urn:iki:ledger:doctor", &[]);
    assert_eq!(next()["excluded"][0]["why"], "claimed");
    assert!(!has_problem(&doctor(), "lease-expired"));
    authority.0.store(T0 + MINUTE, Ordering::SeqCst);
    assert_eq!(next()["excluded"][0]["why"], "lease-expired");
    assert!(has_problem(&doctor(), "lease-expired"));
}

#[test]
fn stale_handback_age_starts_at_the_store_not_the_comment_clock() {
    let (fast, slow, authority) = setup();
    sink(
        &fast,
        "urn:iki:ledger:comment",
        &[
            ("item", "1"),
            ("content", "Ready for review"),
            ("handback", "true"),
        ],
    );
    let stale = |kernel: &Kernel| {
        has_problem(
            &json_source(kernel, "urn:iki:ledger:doctor", &[("awaiting-age", "1m")]),
            "awaiting-stale",
        )
    };
    assert!(!stale(&fast));
    authority.0.store(T0 + MINUTE, Ordering::SeqCst);
    assert!(!stale(&slow));
    authority.0.store(T0 + MINUTE + 1, Ordering::SeqCst);
    assert!(stale(&slow));
}

#[test]
fn the_real_store_evaluator_supplies_the_lease_clock() {
    let store = Arc::new(ikigai_store::space(DurableStore::in_memory().unwrap()));
    let kernel = host(store, Arc::new(HandClock(AtomicU64::new(T0))));
    append(&kernel, "Real store clock", &[]);
    let epoch_ms = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    };
    let before = epoch_ms();
    sink(
        &kernel,
        "urn:iki:ledger:claim",
        &[("item", "1"), ("content", "worker-1"), ("lease", "1m")],
    );
    let after = epoch_ms();
    let item = source(
        &kernel,
        "urn:iki:ledger:item:1",
        &[("as", "application/json")],
    );
    let doc: serde_json::Value = serde_json::from_str(&item).unwrap();
    let expiry = doc["item"]["claim"]["expires"].as_str().unwrap();
    assert!(ikigai_ledger::sparql::iso8601(before + MINUTE).as_str() <= expiry);
    assert!(expiry <= ikigai_ledger::sparql::iso8601(after + MINUTE).as_str());
}

#[test]
fn losing_the_authority_does_not_turn_a_cached_or_local_read_into_a_verdict() {
    use ikigai_core::Error;
    use std::sync::atomic::AtomicBool;

    let partitioned = Arc::new(AtomicBool::new(false));
    let loss = partitioned.clone();
    let store = store_with_time(DurableStore::in_memory().unwrap(), move || {
        if loss.load(Ordering::SeqCst) {
            Err(Error::Endpoint("store clock unreachable".into()))
        } else {
            Ok(Time::from_millis(T0))
        }
    });
    let kernel = host(store, Arc::new(HandClock(AtomicU64::new(T0 + 10 * MINUTE))));
    append(&kernel, "Shared lease", &[]);
    let claim = "urn:iki:ledger:claim";
    sink(
        &kernel,
        claim,
        &[("item", "1"), ("content", "worker-1"), ("lease", "1m")],
    );
    let reads = [
        "urn:iki:ledger:next",
        "urn:iki:ledger:doctor",
        "urn:iki:ledger:stalled",
    ];
    for iri in reads {
        source(&kernel, iri, &[]);
    }
    partitioned.store(true, Ordering::SeqCst);

    let expect_partition = |verb, iri, args: &[(&str, &str)]| {
        let error = try_verb(&kernel, verb, iri, args).unwrap_err();
        assert!(
            error.to_string().contains("store clock unreachable"),
            "{error}"
        );
    };
    for iri in reads {
        expect_partition(Verb::Source, iri, &[]);
    }
    expect_partition(Verb::Sink, "urn:iki:ledger:lapse", &[]);
    expect_partition(
        Verb::Sink,
        claim,
        &[
            ("item", "1"),
            ("content", "worker-2"),
            ("takeover", "true"),
            ("from", "worker-1"),
        ],
    );
    let item = json_source(&kernel, "urn:iki:ledger:item:1", &[]);
    assert_eq!(item["item"]["claim"]["holder"], "worker-1");
}
