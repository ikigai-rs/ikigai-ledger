//! **Leases, the claim's kind, takeover and the doctor** (ledger #775, step 2).
//!
//! Time is the KERNEL's clock here, and the clock in these tests is one the test moves: a
//! lease expires because the test says the hour passed, not because it slept through one.

mod common;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};

use common::*;
use ikigai_core::{Capability, Clock, Error, Fallback, Iri, Kernel, Request, Space, Time, Verb};
use ikigai_ledger::claim::{ClaimKind, ClaimKindStamper};
use ikigai_ledger::SpaceConfig;

/// 2026-09-15T00:00:00Z.
const T0: u64 = 1_789_430_400_000;
const MINUTE: u64 = 60_000;

/// A clock that stays where the test puts it.
#[derive(Clone, Default)]
struct HandClock(Arc<AtomicU64>);

impl HandClock {
    fn at(ms: u64) -> Self {
        HandClock(Arc::new(AtomicU64::new(ms)))
    }
    fn advance(&self, ms: u64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
}

impl Clock for HandClock {
    fn now(&self) -> Time {
        Time::from_millis(self.0.load(Ordering::SeqCst))
    }
}

fn kernel_with(config: SpaceConfig) -> (Kernel, HandClock) {
    let clock = HandClock::at(T0);
    let space = Fallback::new(vec![
        Arc::new(ikigai_store::space(
            ikigai_store::DurableStore::in_memory().expect("an in-memory store"),
        )) as Arc<dyn Space>,
        Arc::new(ikigai_ledger::space_with(config)) as Arc<dyn Space>,
    ]);
    let kernel =
        Kernel::with_meta_renderer(Arc::new(space), Arc::new(ikigai_vocab::TurtleRenderer))
            .with_clock(Arc::new(clock.clone()));
    (kernel, clock)
}

fn hand_kernel() -> (Kernel, HandClock) {
    kernel_with(SpaceConfig::default())
}

fn item_json(kernel: &Kernel, n: i64) -> serde_json::Value {
    serde_json::from_str(&source(
        kernel,
        &format!("urn:iki:ledger:item:{n}"),
        &[("as", "application/json")],
    ))
    .expect("JSON")
}

fn claim(kernel: &Kernel, args: &[(&str, &str)]) -> Result<String, Error> {
    try_verb(kernel, Verb::Sink, "urn:iki:ledger:claim", args)
}

fn next_json(kernel: &Kernel) -> serde_json::Value {
    serde_json::from_str(&source(
        kernel,
        "urn:iki:ledger:next",
        &[("as", "application/json")],
    ))
    .expect("JSON")
}

fn doctor(kernel: &Kernel) -> serde_json::Value {
    serde_json::from_str(&source(
        kernel,
        "urn:iki:ledger:doctor",
        &[("as", "application/json")],
    ))
    .expect("JSON")
}

/// `(check, number)` for every problem the doctor reported.
fn findings(kernel: &Kernel) -> Vec<(String, i64)> {
    doctor(kernel)["problems"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["check"].as_str().unwrap().to_string(),
                p["item"]["number"].as_i64().unwrap(),
            )
        })
        .collect()
}

/// A raw write to the ledger's graph — the out-of-band path the doctor exists for.
fn raw(kernel: &Kernel, triples: &str) {
    sink(
        kernel,
        "urn:iki:store:update",
        &[(
            "content",
            &format!("INSERT DATA {{ GRAPH <urn:iki:ledger:graph:default> {{ {triples} }} }}"),
        )],
    );
}

// --------------------------------------------------------------------- leases

#[test]
fn a_lease_records_its_expiry_from_the_kernel_clock() {
    let (kernel, _clock) = hand_kernel();
    append(&kernel, "Ship it", &[]);
    let answer = claim(
        &kernel,
        &[
            ("item", "1"),
            ("content", "kata-ship/0abc"),
            ("lease", "30m"),
        ],
    )
    .unwrap();
    assert_eq!(
        answer,
        "#1 claimed by kata-ship/0abc until 2026-09-15T00:30:00.000Z\n"
    );
    let claim = &item_json(&kernel, 1)["item"]["claim"];
    assert_eq!(
        claim,
        &serde_json::json!({
            "holder": "kata-ship/0abc", "purpose": null, "kind": "machine",
            "expires": "2026-09-15T00:30:00.000Z", "lease": "PT30M"
        })
    );
    let detail = source(&kernel, "urn:iki:ledger:item:1", &[]);
    assert!(
        detail.contains("  lease:    PT30M, expires 2026-09-15T00:30:00.000Z\n"),
        "{detail}"
    );
}

#[test]
fn a_claim_without_a_lease_renders_as_it_always_did() {
    let (kernel, _clock) = hand_kernel();
    append(&kernel, "Ship it", &[]);
    assert_eq!(
        claim(&kernel, &[("item", "1"), ("content", "brian")]).unwrap(),
        "#1 claimed by brian\n"
    );
    assert!(!source(&kernel, "urn:iki:ledger:item:1", &[]).contains("lease:"));
    assert_eq!(
        item_json(&kernel, 1)["item"]["claim"]["expires"],
        serde_json::Value::Null
    );
}

#[test]
fn a_malformed_lease_is_refused_before_anything_is_written() {
    let (kernel, _clock) = hand_kernel();
    append(&kernel, "Ship it", &[]);
    for lease in ["soon", "0m", "P1M"] {
        let refused = claim(
            &kernel,
            &[("item", "1"), ("content", "x"), ("lease", lease)],
        )
        .expect_err(lease);
        assert!(
            matches!(&refused, Error::InvalidArgument { name, .. } if name == "lease"),
            "{refused:?}"
        );
    }
    assert!(item_json(&kernel, 1)["item"]["claim"].is_null());
}

/// ★ An expired lease is never silently free: `next` still excludes the item, says why, and
/// names the holder and the expiry — and a cached `next` turns over AT the expiry, with
/// nothing written, because the answer changes then.
#[test]
fn an_expired_lease_is_excluded_by_next_with_its_holder_and_expiry() {
    let (kernel, clock) = hand_kernel();
    append(&kernel, "Ship it", &[]);
    claim(
        &kernel,
        &[
            ("item", "1"),
            ("content", "kata-ship/0abc"),
            ("lease", "30m"),
        ],
    )
    .unwrap();
    let before = next_json(&kernel);
    assert_eq!(before["excluded"][0]["why"], "claimed");
    assert_eq!(before["excluded"][0]["expires"], serde_json::Value::Null);

    let request = Request::new(Verb::Source, Iri::parse("urn:iki:ledger:next").unwrap());
    source(&kernel, "urn:iki:ledger:next", &[]);
    assert!(kernel.is_cached(&request, &Capability::root()));
    clock.advance(29 * MINUTE);
    assert!(
        kernel.is_cached(&request, &Capability::root()),
        "still true a minute before the lease ends"
    );
    clock.advance(MINUTE);
    assert!(
        !kernel.is_cached(&request, &Capability::root()),
        "a cached `next` outlived the lease it reported as running"
    );

    let after = next_json(&kernel);
    let excluded = &after["excluded"][0];
    assert_eq!(excluded["why"], "lease-expired");
    assert_eq!(excluded["holder"], "kata-ship/0abc");
    assert_eq!(excluded["expires"], "2026-09-15T00:30:00.000Z");
    assert_eq!(after["ready"], 0, "never silently free");
    let plain = source(&kernel, "urn:iki:ledger:next", &[]);
    assert!(
        plain
            .contains("#1 lease expired: claimed by kata-ship/0abc until 2026-09-15T00:30:00.000Z"),
        "{plain}"
    );
}

// ------------------------------------------------------------------- takeover

#[test]
fn an_expired_lease_is_taken_only_by_a_takeover_naming_its_holder() {
    let (kernel, clock) = hand_kernel();
    append(&kernel, "Ship it", &[]);
    claim(
        &kernel,
        &[
            ("item", "1"),
            ("content", "kata-ship/0abc"),
            ("lease", "10m"),
            ("purpose", "wave 3"),
        ],
    )
    .unwrap();

    // Live: neither a plain claim nor a takeover takes it.
    let live = claim(
        &kernel,
        &[
            ("item", "1"),
            ("content", "kata-ship/9fff"),
            ("takeover", "true"),
            ("from", "kata-ship/0abc"),
        ],
    )
    .expect_err("a live lease");
    assert!(
        matches!(&live, Error::Conflict(m) if m.contains("runs until")),
        "{live:?}"
    );

    clock.advance(10 * MINUTE);
    // Expired: a plain claim is refused, and told how to take it.
    let plain = claim(&kernel, &[("item", "1"), ("content", "kata-ship/9fff")])
        .expect_err("expired is not free");
    assert!(
        matches!(&plain, Error::Conflict(m) if m.contains("takeover=true from=kata-ship/0abc")),
        "{plain:?}"
    );
    // A takeover naming the wrong holder is refused.
    let wrong = claim(
        &kernel,
        &[
            ("item", "1"),
            ("content", "kata-ship/9fff"),
            ("takeover", "true"),
            ("from", "someone-else"),
        ],
    )
    .expect_err("the wrong holder");
    assert!(matches!(wrong, Error::Conflict(_)), "{wrong:?}");

    assert_eq!(
        claim(
            &kernel,
            &[
                ("item", "1"),
                ("content", "kata-ship/9fff"),
                ("takeover", "true"),
                ("from", "kata-ship/0abc"),
                ("lease", "15m"),
            ],
        )
        .unwrap(),
        "#1 taken over from kata-ship/0abc by kata-ship/9fff until 2026-09-15T00:25:00.000Z\n"
    );
    let claim_now = &item_json(&kernel, 1)["item"]["claim"];
    assert_eq!(claim_now["holder"], "kata-ship/9fff");
    assert_eq!(
        claim_now["purpose"],
        serde_json::Value::Null,
        "the old holder's purpose went with the claim"
    );
}

#[test]
fn a_claim_with_no_lease_is_released_not_taken_over() {
    let (kernel, _clock) = hand_kernel();
    append(&kernel, "Ship it", &[]);
    claim(&kernel, &[("item", "1"), ("content", "brian")]).unwrap();
    let refused = claim(
        &kernel,
        &[
            ("item", "1"),
            ("content", "loop"),
            ("takeover", "true"),
            ("from", "brian"),
        ],
    )
    .expect_err("no lease never expires");
    assert!(
        matches!(&refused, Error::Conflict(m) if m.contains("with no lease")),
        "{refused:?}"
    );
    // `from` without `takeover`, and `takeover` without `from`, are argument errors.
    assert!(matches!(
        claim(
            &kernel,
            &[("item", "1"), ("content", "x"), ("from", "brian")]
        ),
        Err(Error::InvalidArgument { .. })
    ));
    assert!(matches!(
        claim(
            &kernel,
            &[("item", "1"), ("content", "x"), ("takeover", "true")]
        ),
        Err(Error::InvalidArgument { .. })
    ));
}

#[test]
fn the_same_holder_claiming_again_renews_its_lease() {
    let (kernel, clock) = hand_kernel();
    append(&kernel, "Ship it", &[]);
    claim(
        &kernel,
        &[("item", "1"), ("content", "loop"), ("lease", "10m")],
    )
    .unwrap();
    clock.advance(20 * MINUTE);
    claim(
        &kernel,
        &[("item", "1"), ("content", "loop"), ("lease", "10m")],
    )
    .unwrap();
    assert_eq!(
        item_json(&kernel, 1)["item"]["claim"]["expires"],
        "2026-09-15T00:30:00.000Z"
    );
    // And renewing without a lease makes it a claim held until released.
    claim(&kernel, &[("item", "1"), ("content", "loop")]).unwrap();
    assert_eq!(
        item_json(&kernel, 1)["item"]["claim"]["lease"],
        serde_json::Value::Null
    );
}

#[test]
fn a_release_takes_the_lease_and_the_kind_with_it() {
    let (kernel, _clock) = hand_kernel();
    append(&kernel, "Ship it", &[]);
    claim(
        &kernel,
        &[("item", "1"), ("content", "loop"), ("lease", "10m")],
    )
    .unwrap();
    delete(&kernel, "urn:iki:ledger:claim", &[("item", "1")]);
    let turtle = source(&kernel, "urn:iki:ledger:item:1", &[("as", "text/turtle")]);
    for gone in ["claimKind", "leaseExpires", "ledger:lease "] {
        assert!(
            !turtle.contains(gone),
            "{gone} survived a release: {turtle}"
        );
    }
}

/// ★ The claim used to be read-then-write, so two claimants could both read the item free and
/// both write. It is one guarded update now: of two claimants released together, exactly one
/// holds the item and the other gets a Conflict naming them. Twenty rounds.
#[test]
fn two_claimants_racing_for_a_free_item_one_wins() {
    for round in 0..20 {
        let (kernel, _clock) = hand_kernel();
        append(&kernel, "Ship it", &[]);
        let barrier = Barrier::new(2);
        let answers: Vec<Result<String, Error>> = std::thread::scope(|scope| {
            let handles: Vec<_> = ["loop-a", "loop-b"]
                .iter()
                .map(|who| {
                    let (kernel, barrier) = (&kernel, &barrier);
                    scope.spawn(move || {
                        barrier.wait();
                        claim(kernel, &[("item", "1"), ("content", who)])
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        assert_eq!(
            answers.iter().filter(|a| a.is_ok()).count(),
            1,
            "round {round}: {answers:#?}"
        );
        let holder = item_json(&kernel, 1)["item"]["claim"]["holder"]
            .as_str()
            .unwrap()
            .to_string();
        let lost = answers.iter().find_map(|a| a.as_ref().err()).unwrap();
        assert!(
            matches!(lost, Error::Conflict(m) if m.contains(&holder)),
            "round {round}: {lost:?}"
        );
    }
}

/// Two takers after one expired lease: the takeover is a compare-and-set against the holder,
/// so the second finds the claim already changed hands.
#[test]
fn two_takeovers_of_one_expired_lease_one_wins() {
    for round in 0..20 {
        let (kernel, clock) = hand_kernel();
        append(&kernel, "Ship it", &[]);
        claim(
            &kernel,
            &[("item", "1"), ("content", "gone"), ("lease", "1m")],
        )
        .unwrap();
        clock.advance(MINUTE);
        let barrier = Barrier::new(2);
        let answers: Vec<Result<String, Error>> = std::thread::scope(|scope| {
            let handles: Vec<_> = ["taker-a", "taker-b"]
                .iter()
                .map(|who| {
                    let (kernel, barrier) = (&kernel, &barrier);
                    scope.spawn(move || {
                        barrier.wait();
                        claim(
                            kernel,
                            &[
                                ("item", "1"),
                                ("content", who),
                                ("takeover", "true"),
                                ("from", "gone"),
                            ],
                        )
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        assert_eq!(
            answers.iter().filter(|a| a.is_ok()).count(),
            1,
            "round {round}: {answers:#?}"
        );
        assert!(
            answers
                .iter()
                .filter_map(|a| a.as_ref().err())
                .all(|e| matches!(e, Error::Conflict(_))),
            "round {round}: {answers:#?}"
        );
    }
}

// ----------------------------------------------------------------------- kind

/// ★ The kind is the host's: a `kind=person` the caller sends is not read, and the default
/// stamps every claim `machine`.
#[test]
fn the_callers_kind_is_ignored_and_the_default_is_machine() {
    let (kernel, _clock) = hand_kernel();
    append(&kernel, "Ship it", &[]);
    claim(
        &kernel,
        &[("item", "1"), ("content", "loop"), ("kind", "person")],
    )
    .unwrap();
    assert_eq!(item_json(&kernel, 1)["item"]["claim"]["kind"], "machine");
}

/// A host stamps from what ITS door put on the request — here a `principal` argument, the
/// way gonk stamps one — and nothing else.
#[test]
fn a_host_stamper_decides_the_kind_from_the_invocation() {
    let stamper: ClaimKindStamper = Arc::new(|inv| match inv.inline_str("principal") {
        Ok(p) if p.starts_with("urn:test:person:") => ClaimKind::Person,
        _ => ClaimKind::Machine,
    });
    let (kernel, _clock) = kernel_with(SpaceConfig::default().claim_kind(stamper));
    append(&kernel, "One", &[]);
    append(&kernel, "Two", &[]);
    claim(
        &kernel,
        &[
            ("item", "1"),
            ("content", "brian"),
            ("principal", "urn:test:person:brian"),
        ],
    )
    .unwrap();
    claim(
        &kernel,
        &[("item", "2"), ("content", "loop"), ("kind", "person")],
    )
    .unwrap();
    assert_eq!(item_json(&kernel, 1)["item"]["claim"]["kind"], "person");
    assert_eq!(item_json(&kernel, 2)["item"]["claim"]["kind"], "machine");
}

// --------------------------------------------------------------------- doctor

#[test]
fn a_clean_ledger_has_a_clean_bill() {
    let (kernel, _clock) = hand_kernel();
    append(&kernel, "Ship it", &[]);
    // A machine claim in flight is exactly what the loop does, and is not a problem.
    sink(
        &kernel,
        "urn:iki:ledger:item:1:state",
        &[("from", "filed"), ("to", "resolving")],
    );
    claim(&kernel, &[("item", "1"), ("content", "loop")]).unwrap();
    let report = doctor(&kernel);
    assert_eq!(report["problems"], serde_json::json!([]));
    assert_eq!(report["lifecycle"], "kata-flight");
    assert_eq!(
        report["checks"],
        serde_json::json!([
            "orphaned",
            "abandoned",
            "lease-expired",
            "unknown-state",
            "two-states"
        ])
    );
    let plain = source(&kernel, "urn:iki:ledger:doctor", &[]);
    assert!(
        plain.starts_with("doctor: default (lifecycle kata-flight): no problems\n"),
        "{plain}"
    );
}

/// ★ Every check, at once, against a ledger seeded with one of each — the last two through
/// the store directly, because no ledger write can produce them.
#[test]
fn the_doctor_reports_every_problem_at_once_and_repairs_none() {
    let (kernel, clock) = hand_kernel();
    for title in [
        "orphaned",
        "abandoned",
        "leased",
        "foreign",
        "doubled",
        "held",
    ] {
        append(&kernel, title, &[]);
    }
    // #1 orphaned: a machine claim, and `filed` is not in flight.
    claim(&kernel, &[("item", "1"), ("content", "loop")]).unwrap();
    // #2 abandoned: in flight, nobody holds it.
    sink(
        &kernel,
        "urn:iki:ledger:item:2:state",
        &[("from", "filed"), ("to", "refining")],
    );
    // #3 lease expired (and in flight, so it is not also orphaned).
    sink(
        &kernel,
        "urn:iki:ledger:item:3:state",
        &[("from", "filed"), ("to", "shipping")],
    );
    claim(
        &kernel,
        &[("item", "3"), ("content", "loop"), ("lease", "5m")],
    )
    .unwrap();
    clock.advance(6 * MINUTE);
    let iri = |n: i64| {
        item_json(&kernel, n)["item"]["iri"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let state = "https://ikigai-rs.dev/ns/ledger#state";
    // #4 a state outside the lifecycle.
    raw(
        &kernel,
        &format!(
            "<{}> <{state}> <urn:iki:ledger:lifecycle:other:doing> .",
            iri(4)
        ),
    );
    // #5 two states.
    raw(
        &kernel,
        &format!(
            "<{0}> <{state}> <urn:iki:ledger:lifecycle:kata-flight:queued> . \
             <{0}> <{state}> <urn:iki:ledger:lifecycle:kata-flight:reviewed> .",
            iri(5)
        ),
    );
    // #6 a person's hold outside the flight: deliberate, never reported. A claim from before
    // kinds existed (no ledger:claimKind) is a machine's, and #1 above shows that reading.
    raw(
        &kernel,
        &format!(
            "<{}> <https://ikigai-rs.dev/ns/ledger#claimedBy> \"brian\" ; \
             <https://ikigai-rs.dev/ns/ledger#claimKind> \
             <https://ikigai-rs.dev/ns/ledger#person> .",
            iri(6)
        ),
    );

    let everything = |kernel: &Kernel| {
        select(
            kernel,
            "SELECT ?s ?p ?o WHERE { GRAPH <urn:iki:ledger:graph:default> { ?s ?p ?o } } \
             ORDER BY ?s ?p ?o",
        )
    };
    let before = everything(&kernel);
    assert_eq!(
        findings(&kernel),
        [
            ("orphaned".to_string(), 1),
            ("abandoned".to_string(), 2),
            ("lease-expired".to_string(), 3),
            ("unknown-state".to_string(), 4),
            ("two-states".to_string(), 5),
        ]
    );
    assert_eq!(before, everything(&kernel), "the doctor wrote to the graph");

    let report = doctor(&kernel);
    let lease = &report["problems"][2];
    assert_eq!(lease["holder"], "loop");
    assert_eq!(lease["expires"], "2026-09-15T00:05:00.000Z");
    assert!(
        lease["remedy"]
            .as_str()
            .unwrap()
            .contains("takeover=true from=loop"),
        "{lease}"
    );
    assert_eq!(
        report["problems"][4]["states"],
        serde_json::json!(["queued", "reviewed"])
    );
    let plain = source(&kernel, "urn:iki:ledger:doctor", &[]);
    assert!(plain.contains(": 5 problem(s)\n"), "{plain}");
    assert!(
        plain.contains("#2 is in `refining`, which is in flight, and nobody holds it"),
        "{plain}"
    );
}

/// A claim from before kinds existed is read as a machine's: reported when it is out of
/// flight, so no claim escapes the doctor by being old.
#[test]
fn a_claim_with_no_recorded_kind_is_a_machines() {
    let (kernel, _clock) = hand_kernel();
    let iri = append_iri(&kernel, "Old", &[]);
    raw(
        &kernel,
        &format!("<{iri}> <https://ikigai-rs.dev/ns/ledger#claimedBy> \"satellite\" ."),
    );
    assert_eq!(findings(&kernel), [("orphaned".to_string(), 1)]);
}

/// The doctor's answer changes when a lease ends, with nothing written, so its cache must
/// end there too.
#[test]
fn a_cached_doctor_turns_over_when_a_lease_ends() {
    let (kernel, clock) = hand_kernel();
    append(&kernel, "Ship it", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:item:1:state",
        &[("from", "filed"), ("to", "resolving")],
    );
    claim(
        &kernel,
        &[("item", "1"), ("content", "loop"), ("lease", "10m")],
    )
    .unwrap();
    assert!(findings(&kernel).is_empty());
    let request = Request::new(Verb::Source, Iri::parse("urn:iki:ledger:doctor").unwrap());
    source(&kernel, "urn:iki:ledger:doctor", &[]);
    assert!(kernel.is_cached(&request, &Capability::root()));
    clock.advance(10 * MINUTE);
    assert!(!kernel.is_cached(&request, &Capability::root()));
    assert_eq!(findings(&kernel), [("lease-expired".to_string(), 1)]);
}

/// The doctor is a read: a read grant is enough, and it is all it needs.
#[test]
fn the_doctor_needs_only_a_read_grant() {
    let (kernel, _clock) = hand_kernel();
    append(&kernel, "Ship it", &[]);
    let reader = Capability::scoped([
        "urn:cap:ledger:read:default".to_string(),
        graph_read("default"),
    ]);
    assert!(try_as(&kernel, &reader, Verb::Source, "urn:iki:ledger:doctor", &[]).is_ok());
}
