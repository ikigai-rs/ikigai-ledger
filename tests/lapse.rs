//! **A lapsed lease is recorded once, listed, and optionally routed to a person** (ledger #3709).
//!
//! The report: a worker stalled, its lease ran out, and the item sat for about 20 hours because
//! nothing in the ledger reacted. Checked against this crate, half of that held. An expired lease
//! was already reported, by `next` (`lease-expired`) and the doctor, and never freed. But nothing
//! RECORDED the lapse: the item's history showed the claim and then nothing, so "which items
//! stalled, and how often" was not a query, and no lapse ever queued a person.
//!
//! The store and kernel clocks are moved together by the test, as in `tests/leases_doctor.rs`.

mod common;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};

use common::*;
use ikigai_core::{Capability, Clock, Error, Fallback, Kernel, Space, Time, Verb};
use ikigai_ledger::SpaceConfig;

/// 2026-09-15T00:00:00Z.
const T0: u64 = 1_789_430_400_000;
const MINUTE: u64 = 60_000;

#[derive(Clone, Default)]
struct HandClock(Arc<AtomicU64>);

impl HandClock {
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
    let clock = HandClock(Arc::new(AtomicU64::new(T0)));
    let space = Fallback::new(vec![
        store_with_clock(
            ikigai_store::DurableStore::in_memory().unwrap(),
            Arc::new(clock.clone()),
        ),
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

fn claim(kernel: &Kernel, args: &[(&str, &str)]) -> String {
    sink(kernel, "urn:iki:ledger:claim", args)
}

fn lease(kernel: &Kernel, item: &str, holder: &str, length: &str) {
    claim(
        kernel,
        &[("item", item), ("content", holder), ("lease", length)],
    );
}

fn sweep(kernel: &Kernel) -> String {
    sink(kernel, "urn:iki:ledger:lapse", &[])
}

fn history(kernel: &Kernel, n: i64) -> String {
    source(kernel, &format!("urn:iki:ledger:item:{n}:history"), &[])
}

fn stalled_json(kernel: &Kernel) -> serde_json::Value {
    serde_json::from_str(&source(
        kernel,
        "urn:iki:ledger:stalled",
        &[("as", "application/json")],
    ))
    .expect("JSON")
}

/// `(number, recorded)` for every stalled item, in the order the view lists them.
fn stalled(kernel: &Kernel) -> Vec<(i64, bool)> {
    stalled_json(kernel)["stalled"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["item"]["number"].as_i64().unwrap(),
                s["recorded"].as_bool().unwrap(),
            )
        })
        .collect()
}

/// How many lapse events the graph holds, straight from the store.
fn lapse_events(kernel: &Kernel) -> usize {
    let rows: serde_json::Value = serde_json::from_str(&select(
        kernel,
        "SELECT ?e WHERE { GRAPH <urn:iki:ledger:graph:default> { \
         ?e a <https://ikigai-rs.dev/ns/ledger#LapseEvent> } }",
    ))
    .expect("SPARQL JSON");
    rows["results"]["bindings"].as_array().unwrap().len()
}

// ------------------------------------------------------------------ the reproduction

/// ★ THE REPRODUCTION. A lease runs out on an open item: `next` and the doctor say so, but the
/// item's history has the claim and nothing after it. Before this change there was no sweep to
/// send (`urn:iki:ledger:lapse` was NotFound) and no event to find.
#[test]
fn a_lapse_is_recorded_in_the_items_history_with_its_holder_and_epoch() {
    let (kernel, clock) = hand_kernel();
    append(&kernel, "Ship it", &[]);
    lease(&kernel, "1", "worker-1", "30m");
    clock.advance(31 * MINUTE);

    let before = history(&kernel, 1);
    assert_eq!(before.lines().count(), 2, "only the claim: {before}");
    assert!(!before.contains("lapse"), "{before}");

    let answer = sweep(&kernel);
    assert_eq!(
        answer,
        "default: recorded 1 lapse(s)\n  #1 Ship it: held by worker-1 at epoch 1, whose lease \
         ran out at 2026-09-15T00:30:00.000Z\n"
    );
    let after = history(&kernel, 1);
    assert!(
        after.ends_with(
            "2026-09-15T00:31:00.000Z  lapse worker-1 at epoch 1, whose lease ran out at \
             2026-09-15T00:30:00.000Z\n"
        ),
        "{after}"
    );
    let json: serde_json::Value = serde_json::from_str(&source(
        &kernel,
        "urn:iki:ledger:item:1:history",
        &[("as", "application/json")],
    ))
    .unwrap();
    let lapse = &json["events"].as_array().unwrap()[1];
    assert_eq!(lapse["kind"], "lapse");
    assert_eq!(lapse["holder"], "worker-1");
    assert_eq!(lapse["epoch"], 1);
    assert_eq!(lapse["lapsed"], "2026-09-15T00:30:00.000Z");

    // ★ Nothing is released: the claim stays, and `next` still excludes it as lease-expired.
    let item: serde_json::Value = serde_json::from_str(&source(
        &kernel,
        "urn:iki:ledger:item:1",
        &[("as", "application/json")],
    ))
    .unwrap();
    assert_eq!(item["item"]["claim"]["holder"], "worker-1");
    let next: serde_json::Value = serde_json::from_str(&source(
        &kernel,
        "urn:iki:ledger:next",
        &[("as", "application/json")],
    ))
    .unwrap();
    assert_eq!(next["excluded"][0]["why"], "lease-expired");
}

// ------------------------------------------------------------------------ written once

#[test]
fn a_second_sweep_records_nothing_and_says_so() {
    let (kernel, clock) = hand_kernel();
    append(&kernel, "Ship it", &[]);
    lease(&kernel, "1", "worker-1", "30m");
    clock.advance(31 * MINUTE);
    sweep(&kernel);
    clock.advance(MINUTE);
    assert_eq!(
        sweep(&kernel),
        "default: no new lapse: 1 stalled item(s) already had theirs recorded, so nothing was \
         written\n"
    );
    assert_eq!(lapse_events(&kernel), 1);
    assert_eq!(stalled(&kernel), vec![(1, true)]);
}

#[test]
fn concurrent_sweeps_record_one_lapse() {
    for _ in 0..8 {
        let (kernel, clock) = hand_kernel();
        append(&kernel, "Ship it", &[]);
        lease(&kernel, "1", "worker-1", "30m");
        clock.advance(31 * MINUTE);
        let barrier = Barrier::new(2);
        std::thread::scope(|scope| {
            for _ in 0..2 {
                let (kernel, barrier) = (&kernel, &barrier);
                scope.spawn(move || {
                    barrier.wait();
                    sweep(kernel)
                });
            }
        });
        assert_eq!(lapse_events(&kernel), 1);
    }
}

#[test]
fn a_renewal_after_a_lapse_that_lapses_again_records_a_second() {
    let (kernel, clock) = hand_kernel();
    append(&kernel, "Ship it", &[]);
    lease(&kernel, "1", "worker-1", "30m");
    clock.advance(31 * MINUTE);
    sweep(&kernel);
    // The worker wakes and resumes: an expired lease still held renews, at the same epoch.
    claim(
        &kernel,
        &[
            ("item", "1"),
            ("content", "worker-1"),
            ("renew", "true"),
            ("lease", "30m"),
        ],
    );
    assert_eq!(stalled(&kernel), vec![]);
    assert!(sweep(&kernel).starts_with("default: no lapse to record"));
    clock.advance(31 * MINUTE);
    assert_eq!(stalled(&kernel), vec![(1, false)]);
    sweep(&kernel);
    assert_eq!(lapse_events(&kernel), 2);
}

#[test]
fn a_takeover_ends_the_stall() {
    let (kernel, clock) = hand_kernel();
    append(&kernel, "Ship it", &[]);
    lease(&kernel, "1", "worker-1", "30m");
    clock.advance(31 * MINUTE);
    sweep(&kernel);
    claim(
        &kernel,
        &[
            ("item", "1"),
            ("content", "worker-2"),
            ("takeover", "true"),
            ("from", "worker-1"),
            ("lease", "30m"),
        ],
    );
    assert_eq!(stalled(&kernel), vec![]);
    let history = history(&kernel, 1);
    let kinds: Vec<&str> = history
        .lines()
        .skip(1)
        .map(|line| line.split_whitespace().nth(1).unwrap())
        .collect();
    assert_eq!(kinds, ["claim", "lapse", "release", "claim"], "{history}");
}

// ----------------------------------------------------------------------- the stalled view

#[test]
fn stalled_lists_open_lapsed_items_not_handed_back_oldest_first() {
    let (kernel, clock) = hand_kernel();
    for title in ["Thirty", "Ten", "Live", "Handed back", "No lease", "Closed"] {
        append(&kernel, title, &[]);
    }
    lease(&kernel, "1", "worker-1", "30m");
    lease(&kernel, "2", "worker-2", "10m");
    lease(&kernel, "3", "worker-3", "2h");
    lease(&kernel, "4", "worker-4", "5m");
    sink(
        &kernel,
        "urn:iki:ledger:comment",
        &[("item", "4"), ("content", "PR green"), ("handback", "true")],
    );
    claim(&kernel, &[("item", "5"), ("content", "worker-5")]);
    lease(&kernel, "6", "worker-6", "5m");
    sink(&kernel, "urn:iki:ledger:close", &[("item", "6")]);

    assert_eq!(
        source(&kernel, "urn:iki:ledger:stalled", &[]),
        "default: no stalled items: no open item is held on a lease that has run out\n"
    );
    clock.advance(45 * MINUTE);
    // ★ Listed from the moment the lease ends, with nothing written: the cached answer above
    // held only until the earliest running lease ran out.
    assert_eq!(stalled(&kernel), vec![(2, false), (1, false)]);
    let text = source(&kernel, "urn:iki:ledger:stalled", &[]);
    assert!(
        text.starts_with(
            "default: 2 stalled item(s), the oldest lapse first\n\
             #2 Ten: held by worker-2 at epoch 1, whose lease ran out at \
             2026-09-15T00:10:00.000Z; the lapse is not recorded yet\n\
             #1 Thirty: held by worker-1 at epoch 1, whose lease ran out at \
             2026-09-15T00:30:00.000Z; the lapse is not recorded yet\n\
             remedy: "
        ),
        "{text}"
    );
    let json = stalled_json(&kernel);
    let first = &json["stalled"][0];
    assert_eq!(first["holder"], "worker-2");
    assert_eq!(first["kind"], "machine");
    assert_eq!(first["epoch"], 1);
    assert_eq!(first["lapsed"], "2026-09-15T00:10:00.000Z");

    sweep(&kernel);
    assert_eq!(stalled(&kernel), vec![(2, true), (1, true)]);
    assert_eq!(lapse_events(&kernel), 2, "nothing for #3 to #6");
}

// ------------------------------------------------------------------------- the route

#[test]
fn without_a_route_a_lapse_adds_no_label() {
    let (kernel, clock) = hand_kernel();
    append(&kernel, "Ship it", &[]);
    lease(&kernel, "1", "worker-1", "30m");
    clock.advance(31 * MINUTE);
    sweep(&kernel);
    let listed = source(
        &kernel,
        "urn:iki:ledger:items",
        &[("labels", "needs-person")],
    );
    assert!(!listed.contains("#1"), "{listed}");
}

#[test]
fn a_route_labels_the_item_once_in_the_same_update() {
    let (kernel, clock) =
        kernel_with(SpaceConfig::default().lapse_label("default", "needs-person"));
    append(&kernel, "Ship it", &[]);
    lease(&kernel, "1", "worker-1", "30m");
    clock.advance(31 * MINUTE);
    assert_eq!(
        sweep(&kernel),
        "default: recorded 1 lapse(s)\n  #1 Ship it: held by worker-1 at epoch 1, whose lease \
         ran out at 2026-09-15T00:30:00.000Z; labeled needs-person\n"
    );
    let listed = source(
        &kernel,
        "urn:iki:ledger:items",
        &[("labels", "needs-person")],
    );
    assert!(listed.contains("#1"), "{listed}");
    let history = history(&kernel, 1);
    assert!(history.contains("label needs-person"), "{history}");
    // A second sweep finds the lapse recorded and writes neither the event nor the label.
    clock.advance(MINUTE);
    sweep(&kernel);
    assert_eq!(
        history.lines().count(),
        self::history(&kernel, 1).lines().count()
    );

    // The person takes the label off after the recovery; a renewal and a second lapse route
    // it again.
    delete(
        &kernel,
        "urn:iki:ledger:label",
        &[("item", "1"), ("content", "needs-person")],
    );
    claim(
        &kernel,
        &[
            ("item", "1"),
            ("content", "worker-1"),
            ("renew", "true"),
            ("lease", "30m"),
        ],
    );
    clock.advance(31 * MINUTE);
    assert!(sweep(&kernel).ends_with("; labeled needs-person\n"));
}

#[test]
fn a_route_is_per_ledger() {
    let (kernel, clock) = kernel_with(SpaceConfig::default().lapse_label("acme", "needs-person"));
    append(&kernel, "Ship it", &[]);
    lease(&kernel, "1", "worker-1", "30m");
    clock.advance(31 * MINUTE);
    assert!(!sweep(&kernel).contains("labeled"));
}

#[test]
#[should_panic(expected = "a lapse route's label is not a label")]
fn a_route_with_a_label_that_cannot_be_written_fails_at_configuration() {
    let _ = SpaceConfig::default().lapse_label("default", "needs person");
}

// ------------------------------------------------------------------------------ grants

fn scoped(ledger: &str, verbs: &[&str]) -> Capability {
    let mut scopes: Vec<String> = verbs
        .iter()
        .map(|verb| format!("urn:cap:ledger:{verb}:{ledger}"))
        .collect();
    scopes.push(graph_read(ledger));
    scopes.push(graph_write(ledger));
    Capability::scoped(scopes)
}

#[test]
fn the_sweep_is_the_dispatchers_and_the_view_is_any_readers() {
    let kernel = kernel();
    let sweep =
        |cap: &Capability| try_as(&kernel, cap, Verb::Sink, "urn:iki:ledger:acme:lapse", &[]);
    match sweep(&scoped("acme", &["read", "claim", "comment"])) {
        Err(Error::Denied(_)) => {}
        other => panic!("a worker swept: {other:?}"),
    }
    assert_eq!(
        sweep(&scoped("acme", &["read", "close"])).unwrap(),
        "acme: no lapse to record: no open item is held on a lease that has run out, so nothing \
         was written\n"
    );
    assert!(sweep(&scoped("acme", &["read", "write"])).is_ok());
    assert!(try_as(
        &kernel,
        &scoped("acme", &["read"]),
        Verb::Source,
        "urn:iki:ledger:acme:stalled",
        &[],
    )
    .is_ok());
}

#[test]
fn lapse_and_stalled_are_reserved_ledger_names() {
    for name in ["lapse", "stalled"] {
        assert!(
            ikigai_ledger::ledger::Ledger::parse(name).is_err(),
            "{name}"
        );
    }
}
