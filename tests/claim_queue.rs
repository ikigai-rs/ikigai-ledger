//! **The fair claim queue, over the real endpoints** (ledger #4455; the design is
//! `docs/design/fair-claim-queue.md`, ledger #4433's spike).
//!
//! `urn:iki:ledger:{ledger}:wait:item:{id}` lists, joins and leaves an item's queue; a release
//! offers the item to the first live waiter in the same update; the claim guard refuses
//! anyone else while the offer stands; `next` leaves an offered item out for everyone but its
//! waiter; and `urn:iki:ledger:{ledger}:offers` passes on offers nobody took. Every test runs
//! under the narrow grants the README publishes: the worker's `read`, `claim` and `comment`,
//! and the dispatcher's `close`, each with its principal, on a kernel whose clock the test
//! moves by hand.

mod common;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use common::*;
use ikigai_core::{Capability, Clock, Error, Fallback, Kernel, Space, Time, Verb};

const LEDGER: &str = "acme";
const A: &str = "urn:iki:gonk:client:aaaa";
const B: &str = "urn:iki:gonk:client:bbbb";
const C: &str = "urn:iki:gonk:client:cccc";
const D: &str = "urn:iki:gonk:passkey:dispatcher";
const T0: u64 = 1_789_430_400_000;

struct HandClock(Arc<AtomicU64>);

impl Clock for HandClock {
    fn now(&self) -> Time {
        Time::from_millis(self.0.load(Ordering::SeqCst))
    }
}

struct Ledger {
    kernel: Kernel,
    now: Arc<AtomicU64>,
}

fn at(resource: &str) -> String {
    format!("urn:iki:ledger:{LEDGER}:{resource}")
}

fn wait_iri(item: u64) -> String {
    at(&format!("wait:item:{item}"))
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

/// The published worker grant, as a door mints it for one client.
fn worker(principal: &str) -> Capability {
    grant(&["read", "claim", "comment"])
        .with_principal(principal)
        .unwrap()
}

/// The dispatcher: the worker grant and close.
fn dispatcher() -> Capability {
    grant(&["read", "claim", "comment", "close"])
        .with_principal(D)
        .unwrap()
}

impl Ledger {
    fn new() -> Ledger {
        let now = Arc::new(AtomicU64::new(T0));
        let space = Fallback::new(vec![
            Arc::new(ikigai_store::space(
                ikigai_store::DurableStore::in_memory().unwrap(),
            )) as Arc<dyn Space>,
            Arc::new(ikigai_ledger::space()) as Arc<dyn Space>,
        ]);
        let kernel = Kernel::new(Arc::new(space)).with_clock(Arc::new(HandClock(now.clone())));
        let ledger = Ledger { kernel, now };
        // Filed under root: the lanes below hold only the narrow grants.
        sink(
            &ledger.kernel,
            &at("append"),
            &[("content", "Release mutex")],
        );
        sink(
            &ledger.kernel,
            &at("append"),
            &[("content", "Another item")],
        );
        ledger
    }

    /// Seconds since the start.
    fn at(&self, seconds: u64) {
        self.now.store(T0 + seconds * 1000, Ordering::SeqCst);
    }

    fn try_(
        &self,
        cap: &Capability,
        verb: Verb,
        iri: &str,
        args: &[(&str, &str)],
    ) -> Result<String, Error> {
        try_as(&self.kernel, cap, verb, iri, args)
    }

    fn sink(&self, cap: &Capability, resource: &str, args: &[(&str, &str)]) -> String {
        self.try_(cap, Verb::Sink, &at(resource), args)
            .unwrap_or_else(|e| panic!("sink {resource} {args:?}: {e}"))
    }

    fn join(&self, cap: &Capability, holder: &str, args: &[(&str, &str)]) -> Result<String, Error> {
        let mut all = vec![("content", holder)];
        all.extend_from_slice(args);
        self.try_(cap, Verb::Sink, &wait_iri(1), &all)
    }

    fn claim(&self, cap: &Capability, holder: &str) -> Result<String, Error> {
        self.try_(
            cap,
            Verb::Sink,
            &at("claim"),
            &[("item", "1"), ("content", holder), ("lease", "30m")],
        )
    }

    fn release(&self, cap: &Capability, holder: &str) -> String {
        self.try_(
            cap,
            Verb::Delete,
            &at("claim"),
            &[("item", "1"), ("from", holder)],
        )
        .unwrap_or_else(|e| panic!("release by {holder}: {e}"))
    }

    fn queue(&self) -> serde_json::Value {
        let text = self
            .try_(
                &worker(A),
                Verb::Source,
                &wait_iri(1),
                &[("as", "application/json")],
            )
            .unwrap();
        serde_json::from_str(&text).unwrap()
    }

    fn waiters(&self) -> Vec<String> {
        self.queue()["waiters"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["waiter"].as_str().unwrap().to_string())
            .collect()
    }

    fn offered_to(&self) -> Option<String> {
        self.queue()["offer"]["waiter"].as_str().map(str::to_string)
    }

    /// The number of quads of a predicate in the ledger's graph.
    fn count(&self, local: &str) -> usize {
        let text = select(
            &self.kernel,
            &format!(
                "SELECT ?s WHERE {{ GRAPH <urn:iki:ledger:graph:{LEDGER}> {{ \
                 ?s <https://ikigai-rs.dev/ns/ledger#{local}> ?o }} }}"
            ),
        );
        let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
        doc["results"]["bindings"].as_array().unwrap().len()
    }
}

fn epoch_of(answer: &str) -> u64 {
    answer
        .split("at epoch ")
        .nth(1)
        .and_then(|rest| {
            rest.split(|c: char| !c.is_ascii_digit())
                .next()
                .and_then(|n| n.parse().ok())
        })
        .unwrap_or_else(|| panic!("no epoch in {answer:?}"))
}

/// A ledger whose item 1 is held by A, with B then C queued behind it.
fn held_by_a_with_b_and_c_waiting() -> Ledger {
    let l = Ledger::new();
    l.at(0);
    l.claim(&worker(A), "lane-a").unwrap();
    l.at(10);
    l.join(&worker(B), "lane-b", &[("poll", "120s")]).unwrap();
    l.at(20);
    l.join(&worker(C), "lane-c", &[]).unwrap();
    l
}

#[test]
fn waiters_are_listed_in_the_order_they_asked() {
    let l = held_by_a_with_b_and_c_waiting();
    l.at(30);
    let q = l.queue();
    assert_eq!(l.waiters(), ["lane-b", "lane-c"]);
    let first = &q["waiters"][0];
    assert_eq!(first["place"], 1);
    assert_eq!(first["seq"], 1);
    assert_eq!(first["principal"], B);
    assert_eq!(first["asked_at"], "2026-09-15T00:00:10.000Z");
    assert_eq!(first["expires"], "2026-09-15T00:03:10.000Z");
    assert_eq!(first["poll"], "PT2M");
    assert!(q["offer"].is_null(), "no offer while the item is held: {q}");
    let plain = l.try_(&worker(A), Verb::Source, &wait_iri(1), &[]).unwrap();
    assert!(plain.starts_with("acme#1: 2 waiting\n"), "{plain}");
    assert!(
        plain.contains("  1. lane-b (as urn:iki:gonk:client:bbbb), asked "),
        "{plain}"
    );
}

#[test]
fn a_renewal_keeps_the_place_and_moves_only_its_expiry() {
    let l = held_by_a_with_b_and_c_waiting();
    l.at(100);
    let answer = l.join(&worker(B), "lane-b", &[]).unwrap();
    assert!(
        answer.starts_with("acme#1 waited for by lane-b (as urn:iki:gonk:client:bbbb): place 1 of 2, until 2026-09-15T00:04:40.000Z; held by lane-a"),
        "{answer}"
    );
    let q = l.queue();
    assert_eq!(l.waiters(), ["lane-b", "lane-c"]);
    assert_eq!(q["waiters"][0]["seq"], 1, "the order is kept");
    assert_eq!(q["waiters"][0]["asked_at"], "2026-09-15T00:00:10.000Z");
    assert_eq!(
        q["waiters"][0]["poll"], "PT2M",
        "a renewal without poll= keeps it"
    );
}

#[test]
fn a_release_offers_the_item_to_the_head_and_only_the_head_may_claim() {
    let l = held_by_a_with_b_and_c_waiting();
    l.at(60);
    l.release(&worker(A), "lane-a");
    // Offered in the release's own update, for B's poll plus the margin: 120 s + 30 s.
    assert_eq!(l.offered_to().as_deref(), Some("lane-b"));
    let q = l.queue();
    assert_eq!(q["offer"]["expires"], "2026-09-15T00:03:30.000Z");
    assert_eq!(q["offer"]["after_epoch"], 1);
    // C, a stranger and A itself are refused, by name and with the end of the offer.
    l.at(61);
    for (cap, holder) in [
        (worker(C), "lane-c"),
        (worker(A), "lane-a"),
        (worker(A), "lane-z"),
    ] {
        match l.claim(&cap, holder) {
            Err(Error::Conflict(why)) => assert_eq!(
                why,
                "acme#1 is offered to lane-b (as urn:iki:gonk:client:bbbb) until \
                 2026-09-15T00:03:30.000Z: it was released while waiters were queued, and the \
                 first in the queue may claim it until then. Join the queue (`sink \
                 urn:iki:ledger:acme:wait:item:1 content=<holder>`) to be offered it in turn"
            ),
            other => panic!("{holder} claimed an item offered to lane-b: {other:?}"),
        }
    }
    // The label alone is not the waiter: lane-b under another principal is refused too.
    assert!(matches!(
        l.claim(&worker(C), "lane-b"),
        Err(Error::Conflict(_))
    ));
    // The head claims: an ordinary claim, the next epoch, and it leaves the queue.
    let answer = l.claim(&worker(B), "lane-b").unwrap();
    assert_eq!(epoch_of(&answer), 2);
    assert_eq!(l.waiters(), ["lane-c"]);
    assert!(l.offered_to().is_none());
    assert_eq!(l.count("offeredTo"), 0, "the spent offer is gone");
}

#[test]
fn an_empty_queue_changes_nothing() {
    let l = Ledger::new();
    l.at(0);
    let a = epoch_of(&l.claim(&worker(A), "lane-a").unwrap());
    l.release(&worker(A), "lane-a");
    for local in [
        "offeredTo",
        "offerExpires",
        "offerAfterEpoch",
        "waitsOn",
        "waitSeq",
    ] {
        assert_eq!(l.count(local), 0, "{local} written with nobody waiting");
    }
    // Anyone claims the free item as before.
    l.at(1);
    assert_eq!(epoch_of(&l.claim(&worker(C), "lane-c").unwrap()), a + 1);
}

#[test]
fn a_join_on_a_free_item_nobody_is_offered_claims_it_at_once() {
    let l = Ledger::new();
    l.at(0);
    let answer = l.join(&worker(A), "lane-a", &[("lease", "30m")]).unwrap();
    assert!(
        answer.starts_with("acme#1 claimed by lane-a at epoch 1 until "),
        "{answer}"
    );
    assert!(l.waiters().is_empty(), "the claim takes no place");
    let json = l
        .join(&worker(B), "lane-b", &[("as", "application/json")])
        .unwrap();
    let doc: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(doc["claimed"], false);
    assert_eq!(doc["place"], 1);
    assert_eq!(doc["of"], 1);
}

#[test]
fn a_join_on_a_closed_or_already_held_item_is_a_conflict() {
    let l = Ledger::new();
    l.at(0);
    l.claim(&worker(A), "lane-a").unwrap();
    match l.join(&worker(A), "lane-a", &[]) {
        Err(Error::Conflict(why)) => assert!(
            why.starts_with("acme#1 is already held by lane-a (as urn:iki:gonk:client:aaaa): a waiter waits for a claim it does not hold"),
            "{why}"
        ),
        other => panic!("{other:?}"),
    }
    l.sink(&dispatcher(), "close", &[("item", "1")]);
    match l.join(&worker(B), "lane-b", &[]) {
        Err(Error::Conflict(why)) => assert!(why.contains("nobody waits for it"), "{why}"),
        other => panic!("a closed item was joined: {other:?}"),
    }
}

#[test]
fn bad_arguments_are_refused_before_anything_is_written() {
    let l = Ledger::new();
    for (args, name) in [
        (vec![("content", "")], "content"),
        (vec![("content", "none")], "content"),
        (vec![("content", "x"), ("ttl", "soon")], "ttl"),
        (vec![("content", "x"), ("ttl", "2h")], "ttl"),
        (vec![("content", "x"), ("poll", "0s")], "poll"),
    ] {
        match l.try_(&worker(A), Verb::Sink, &wait_iri(1), &args) {
            Err(Error::InvalidArgument { name: got, .. }) => assert_eq!(got, name, "{args:?}"),
            other => panic!("{args:?}: {other:?}"),
        }
    }
    assert_eq!(l.count("waitsOn"), 0);
}

#[test]
fn a_head_that_does_not_claim_in_time_leaves_and_the_next_claim_passes_the_offer_on() {
    let l = held_by_a_with_b_and_c_waiting();
    l.at(60);
    l.join(&worker(C), "lane-c", &[]).unwrap(); // C renews; B's place still holds
    l.release(&worker(A), "lane-a");
    assert_eq!(l.offered_to().as_deref(), Some("lane-b"));
    // B never claims. At the end of its offer (150 s after the release), C's claim settles the
    // offer: B leaves the queue, C is the head and is offered the item, and its claim lands.
    // (B's own join would claim it: a join by the waiter an offer stands to is a claim.)
    l.at(100);
    l.join(&worker(C), "lane-c", &[]).unwrap();
    l.at(209);
    assert!(matches!(
        l.claim(&worker(C), "lane-c"),
        Err(Error::Conflict(_))
    ));
    l.at(210);
    let answer = l.claim(&worker(C), "lane-c").unwrap();
    assert_eq!(epoch_of(&answer), 2);
    assert!(
        l.waiters().is_empty(),
        "B left with its offer, and C has the item"
    );
}

#[test]
fn the_offers_sweep_passes_an_ended_offer_on_and_needs_the_dispatchers_grant() {
    let l = held_by_a_with_b_and_c_waiting();
    l.at(60);
    l.join(&worker(C), "lane-c", &[]).unwrap();
    l.release(&worker(A), "lane-a");
    l.at(100);
    l.join(&worker(C), "lane-c", &[]).unwrap();
    l.at(211);
    // Nobody acts. A worker may not sweep; the dispatcher may.
    match l.try_(&worker(C), Verb::Sink, &at("offers"), &[]) {
        Err(Error::Denied(_)) => {}
        other => panic!("a worker swept: {other:?}"),
    }
    let answer = l.sink(&dispatcher(), "offers", &[]);
    assert!(
        answer.contains("  acme#1 offered to lane-c (as urn:iki:gonk:client:cccc) until 2026-09-15T00:04:31.000Z"),
        "{answer}"
    );
    assert_eq!(l.waiters(), ["lane-c"]);
    // A second sweep finds nothing to do.
    let again = l.sink(&dispatcher(), "offers", &[]);
    assert!(again.contains("nothing was written"), "{again}");
}

#[test]
fn a_lapsed_place_is_skipped_at_once_and_swept_in_batches() {
    let l = held_by_a_with_b_and_c_waiting();
    // B and C stop renewing; their places lapse (3 minutes) with nothing written.
    l.at(200);
    assert!(l.waiters().is_empty(), "{:?}", l.queue());
    // A release then offers nothing: nobody live is waiting.
    l.release(&worker(A), "lane-a");
    assert!(l.offered_to().is_none());
    assert_eq!(l.count("offeredTo"), 0);
    // The sweep deletes the lapsed places, every quad of them.
    let answer = l.sink(&dispatcher(), "offers", &[]);
    assert!(
        answer.starts_with("acme: removed 2 lapsed place(s)"),
        "{answer}"
    );
    for local in [
        "waitsOn",
        "waiter",
        "waiterPrincipal",
        "askedAt",
        "waitExpires",
        "waitPoll",
    ] {
        assert_eq!(l.count(local), 0, "{local} left behind");
    }
    // The item keeps its last sequence, so a later place still counts up.
    l.at(201);
    l.claim(&worker(A), "lane-a").unwrap();
    l.join(&worker(B), "lane-b", &[]).unwrap();
    assert_eq!(l.queue()["waiters"][0]["seq"], 3);
}

#[test]
fn leaving_follows_the_principal_rules_of_a_release() {
    let l = held_by_a_with_b_and_c_waiting();
    l.at(30);
    // C may not remove B's place by naming it.
    match l.try_(
        &worker(C),
        Verb::Delete,
        &wait_iri(1),
        &[("from", "lane-b")],
    ) {
        Err(Error::Denied(why)) => assert!(
            why.to_string()
                .contains("a place taken as a principal is left by that principal"),
            "{why}"
        ),
        other => panic!("{other:?}"),
    }
    // B leaves on its own; the dispatcher removes C by its override.
    let answer = l
        .try_(
            &worker(B),
            Verb::Delete,
            &wait_iri(1),
            &[("from", "lane-b")],
        )
        .unwrap();
    assert_eq!(answer, "acme#1 no longer waited for by lane-b\n");
    l.try_(
        &dispatcher(),
        Verb::Delete,
        &wait_iri(1),
        &[("from", "lane-c")],
    )
    .unwrap();
    assert!(l.waiters().is_empty());
    let again = l
        .try_(
            &worker(B),
            Verb::Delete,
            &wait_iri(1),
            &[("from", "lane-b")],
        )
        .unwrap();
    assert_eq!(
        again,
        "lane-b is not waiting for acme#1, so there was no place to leave\n"
    );
}

#[test]
fn an_offered_waiter_that_leaves_passes_the_offer_on() {
    let l = held_by_a_with_b_and_c_waiting();
    l.at(60);
    l.release(&worker(A), "lane-a");
    assert_eq!(l.offered_to().as_deref(), Some("lane-b"));
    l.try_(
        &worker(B),
        Verb::Delete,
        &wait_iri(1),
        &[("from", "lane-b")],
    )
    .unwrap();
    assert_eq!(l.offered_to().as_deref(), Some("lane-c"));
}

#[test]
fn reading_and_joining_need_the_published_grants_and_nothing_more() {
    let l = held_by_a_with_b_and_c_waiting();
    // A read grant alone reads the queue and cannot join it.
    let reader = grant(&["read"]).with_principal(C).unwrap();
    assert!(l.try_(&reader, Verb::Source, &wait_iri(1), &[]).is_ok());
    assert!(matches!(
        l.try_(&reader, Verb::Sink, &wait_iri(1), &[("content", "lane-r")]),
        Err(Error::Denied(_))
    ));
    // A claim grant without read cannot read it.
    let claimer = grant(&["claim"]).with_principal(C).unwrap();
    assert!(matches!(
        l.try_(&claimer, Verb::Source, &wait_iri(1), &[]),
        Err(Error::Denied(_))
    ));
    // A grant over another ledger reaches nothing here.
    let elsewhere = Capability::scoped(vec![
        "urn:cap:ledger:read:other".to_string(),
        "urn:cap:ledger:claim:other".to_string(),
        graph_read(LEDGER),
        graph_write(LEDGER),
    ]);
    assert!(matches!(
        l.try_(&elsewhere, Verb::Source, &wait_iri(1), &[]),
        Err(Error::Denied(_))
    ));
}

#[test]
fn next_leaves_an_offered_item_out_for_everyone_but_its_waiter() {
    let l = held_by_a_with_b_and_c_waiting();
    l.at(60);
    l.release(&worker(A), "lane-a");
    let next = |cap: &Capability, args: &[(&str, &str)]| -> serde_json::Value {
        let mut all = vec![("as", "application/json")];
        all.extend_from_slice(args);
        serde_json::from_str(&l.try_(cap, Verb::Source, &at("next"), &all).unwrap()).unwrap()
    };
    let ranked = |doc: &serde_json::Value| -> Vec<i64> {
        doc["ranking"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["item"]["number"].as_i64().unwrap())
            .collect()
    };
    for (cap, args) in [
        (worker(C), vec![("waiter", "lane-c")]),
        (worker(C), vec![]),
        (worker(C), vec![("waiter", "lane-b")]), // the label under another principal
    ] {
        let doc = next(&cap, &args);
        assert_eq!(ranked(&doc), [2], "{args:?}: {doc}");
        let out = doc["excluded"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["item"]["number"] == 1)
            .unwrap();
        assert_eq!(out["why"], "offered");
        assert_eq!(out["holder"], "lane-b (as urn:iki:gonk:client:bbbb)");
        assert_eq!(out["expires"], "2026-09-15T00:03:30.000Z");
    }
    let doc = next(&worker(B), &[("waiter", "lane-b")]);
    assert_eq!(
        ranked(&doc),
        [1, 2],
        "the offered waiter sees it ranked: {doc}"
    );
    let plain = l.try_(&worker(C), Verb::Source, &at("next"), &[]).unwrap();
    assert!(
        plain.contains(
            "offered to lane-b (as urn:iki:gonk:client:bbbb) until 2026-09-15T00:03:30.000Z"
        ),
        "{plain}"
    );
    // Once the offer ends, `next` offers it to everyone again (the claim then settles it).
    l.at(211);
    assert_eq!(ranked(&next(&worker(C), &[])), [1, 2]);
}

#[test]
fn a_hand_off_is_an_ordinary_grant_and_the_old_epoch_is_fenced_out() {
    let l = Ledger::new();
    l.at(0);
    let a = epoch_of(&l.claim(&worker(A), "lane-a").unwrap());
    l.at(5);
    l.join(&worker(B), "lane-b", &[]).unwrap();
    // Waiting and renewing never touch the claim or its epoch.
    l.at(50);
    l.join(&worker(B), "lane-b", &[]).unwrap();
    let check = l
        .try_(
            &worker(A),
            Verb::Source,
            &at("claim"),
            &[("item", "1"), ("epoch", &a.to_string())],
        )
        .unwrap();
    assert!(check.contains("is current"), "{check}");
    l.at(60);
    l.release(&worker(A), "lane-a");
    let b = epoch_of(&l.claim(&worker(B), "lane-b").unwrap());
    assert_eq!(b, a + 1);
    let stale = l.try_(
        &worker(A),
        Verb::Sink,
        &at("comment"),
        &[
            ("item", "1"),
            ("epoch", &a.to_string()),
            ("content", "released v1"),
        ],
    );
    assert!(matches!(stale, Err(Error::Conflict(_))), "{stale:?}");
    let fresh = l.try_(
        &worker(B),
        Verb::Sink,
        &at("comment"),
        &[
            ("item", "1"),
            ("epoch", &b.to_string()),
            ("content", "released v2"),
        ],
    );
    assert!(fresh.is_ok(), "{fresh:?}");
}

#[test]
fn an_offer_is_void_once_another_grant_has_happened() {
    let l = held_by_a_with_b_and_c_waiting();
    l.at(60);
    l.release(&worker(A), "lane-a");
    assert_eq!(l.offered_to().as_deref(), Some("lane-b"));
    // Another grant lands first (here, the epoch an older release's claim would raise),
    // past the offer.
    ok(
        &l.kernel,
        Verb::Sink,
        "urn:iki:store:update",
        &[(
            "content",
            "PREFIX ledger: <https://ikigai-rs.dev/ns/ledger#> \
             DELETE { GRAPH <urn:iki:ledger:graph:acme> { ?i ledger:claimEpoch ?e } } \
             INSERT { GRAPH <urn:iki:ledger:graph:acme> { ?i ledger:claimEpoch 2 } } \
             WHERE { GRAPH <urn:iki:ledger:graph:acme> { ?i ledger:offeredTo ?w ; ledger:claimEpoch ?e } }",
        )],
    );
    // At epoch 2 the offer made after epoch 1 no longer stands: nobody is offered the item.
    assert!(l.offered_to().is_none(), "{}", l.queue());
}

#[test]
fn deleting_an_item_takes_its_queue_with_it() {
    let l = held_by_a_with_b_and_c_waiting();
    l.at(60);
    l.release(&worker(A), "lane-a");
    delete(&l.kernel, &at("item:1"), &[]);
    for local in ["waitsOn", "waiter", "waitExpires", "askedAt", "offeredTo"] {
        assert_eq!(l.count(local), 0, "{local} left live after the delete");
    }
}

/// ★ **Upgrade**: a store written by 0.26.3, the release before the queue, loads and behaves
/// exactly as it did. `tests/fixtures/ledger-0.26.3.nq` is that release's own dump (made by
/// 0.26.3 with `urn:iki:store:dump`): item 1 claimed, released, claimed at epoch 2 and
/// released; item 2 held by lane-c without a lease; item 3 closed.
#[test]
fn a_store_written_before_the_queue_loads_and_behaves_as_before() {
    let kernel = kernel();
    ok(
        &kernel,
        Verb::Sink,
        "urn:iki:store:load",
        &[
            ("content", include_str!("fixtures/ledger-0.26.3.nq")),
            ("format", "application/n-quads"),
        ],
    );
    let claim = "urn:iki:ledger:claim";
    // Item 2 is still held, and a claim by another is refused as it was.
    match try_verb(
        &kernel,
        Verb::Sink,
        claim,
        &[("item", "2"), ("content", "lane-x")],
    ) {
        Err(Error::Conflict(why)) => assert!(why.contains("already claimed by lane-c"), "{why}"),
        other => panic!("{other:?}"),
    }
    // Item 1 is free with nobody waiting: it is claimed exactly as before, at the next epoch.
    let queue = source(&kernel, "urn:iki:ledger:wait:item:1", &[]);
    assert_eq!(queue, "#1: nobody is waiting\n");
    let next = source(&kernel, "urn:iki:ledger:next", &[]);
    assert!(next.contains("#1"), "{next}");
    let answer = sink(&kernel, claim, &[("item", "1"), ("content", "lane-a")]);
    assert_eq!(epoch_of(&answer), 3);
    // A release with nobody waiting writes no offer; item 3 stays closed and cannot be joined.
    delete(&kernel, claim, &[("item", "1"), ("from", "lane-a")]);
    let offers = select(
        &kernel,
        "SELECT ?s WHERE { GRAPH ?g { ?s <https://ikigai-rs.dev/ns/ledger#offeredTo> ?o } }",
    );
    assert!(offers.contains("\"bindings\":[]"), "{offers}");
    assert!(matches!(
        try_verb(
            &kernel,
            Verb::Sink,
            "urn:iki:ledger:wait:item:3",
            &[("content", "lane-a")]
        ),
        Err(Error::Conflict(_))
    ));
    // And the queue works on the old store: join behind lane-c, offered on its release.
    sink(
        &kernel,
        "urn:iki:ledger:wait:item:2",
        &[("content", "lane-a")],
    );
    delete(&kernel, claim, &[("item", "2"), ("from", "lane-c")]);
    let queue = source(&kernel, "urn:iki:ledger:wait:item:2", &[]);
    assert!(
        queue.starts_with("#2: 1 waiting; offered to lane-a until "),
        "{queue}"
    );
}
