//! **A claim carries a fencing epoch, and a stale holder's writes are refused** (ledger #3205).
//!
//! The gap, reproduced here before the fix: a claim recorded only its holder and expiry, so a
//! lane that stalled past its lease, was taken over, and then woke could still comment, hand
//! back, close, renew or release as if it held the item. Only convention (renew just before
//! acting) stopped it, and `renew` names the holder, which the woken lane still spells.
//!
//! Now every grant and every takeover sets `ledger:claimEpoch` one higher than the item's
//! previous epoch (a renewal keeps it; a release leaves it, so it never goes down), the claim
//! resource answers whether an epoch is current, and a holder's write sent with `epoch=` is
//! refused as a Conflict naming the current holder and epoch, writing nothing.

mod common;

use common::*;
use ikigai_core::{Error, Kernel, Verb};

const CLAIM: &str = "urn:iki:ledger:claim";

fn ledger_with_an_item() -> Kernel {
    let kernel = kernel();
    sink(
        &kernel,
        "urn:iki:ledger:append",
        &[("content", "Fix the widget")],
    );
    kernel
}

fn claim(kernel: &Kernel, args: &[(&str, &str)]) -> Result<String, Error> {
    let mut all = vec![("item", "1")];
    all.extend_from_slice(args);
    try_verb(kernel, Verb::Sink, CLAIM, &all)
}

fn release(kernel: &Kernel, args: &[(&str, &str)]) -> Result<String, Error> {
    let mut all = vec![("item", "1")];
    all.extend_from_slice(args);
    try_verb(kernel, Verb::Delete, CLAIM, &all)
}

fn check(kernel: &Kernel, epoch: &str) -> String {
    source(kernel, CLAIM, &[("item", "1"), ("epoch", epoch)])
}

fn item_json(kernel: &Kernel) -> serde_json::Value {
    let view = source(
        kernel,
        "urn:iki:ledger:item:1",
        &[("as", "application/json")],
    );
    serde_json::from_str::<serde_json::Value>(&view).expect("JSON")["item"].clone()
}

fn epoch(kernel: &Kernel) -> Option<u64> {
    item_json(kernel)["claim"]["epoch"].as_u64()
}

fn holder(kernel: &Kernel) -> Option<String> {
    item_json(kernel)["claim"]["holder"]
        .as_str()
        .map(str::to_string)
}

/// Every `ledger:claimEpoch` stored on the item, as the store holds them.
fn stored_epochs(kernel: &Kernel) -> Vec<String> {
    let json = select(
        kernel,
        "SELECT ?e WHERE { GRAPH ?g { ?i <https://ikigai-rs.dev/ns/ledger#claimEpoch> ?e } }",
    );
    let doc: serde_json::Value = serde_json::from_str(&json).expect("results JSON");
    doc["results"]["bindings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["e"]["value"].as_str().unwrap().to_string())
        .collect()
}

fn comments(kernel: &Kernel) -> usize {
    item_json(kernel)["comments"].as_array().map_or(0, Vec::len)
}

fn stale(result: Result<String, Error>, what: &str, names: &[&str]) {
    match result {
        Err(Error::Conflict(message)) => {
            for name in names {
                assert!(message.contains(name), "{what}: {message} lacks {name}");
            }
            assert!(message.contains("nothing was written"), "{what}: {message}");
        }
        other => panic!("{what}: expected Conflict, got {other:?}"),
    }
}

/// A claim held by `worker-1` (epoch 1) on a 1s lease, taken over by `worker-2` (epoch 2).
fn taken_over() -> Kernel {
    let kernel = ledger_with_an_item();
    claim(&kernel, &[("content", "worker-1"), ("lease", "1s")]).unwrap();
    // The test clock advances a second per reading, so the lease has run out.
    claim(
        &kernel,
        &[
            ("content", "worker-2"),
            ("takeover", "true"),
            ("from", "worker-1"),
        ],
    )
    .unwrap();
    kernel
}

// ---------------------------------------------------------------- the epoch itself

#[test]
fn a_grant_then_a_takeover_raises_the_epoch() {
    let kernel = ledger_with_an_item();
    let answer = claim(&kernel, &[("content", "worker-1"), ("lease", "1s")]).unwrap();
    assert!(answer.ends_with(", epoch 1\n"), "{answer}");
    assert!(
        answer.starts_with("#1 claimed by worker-1 until "),
        "{answer}"
    );
    assert_eq!(epoch(&kernel), Some(1));
    let answer = claim(
        &kernel,
        &[
            ("content", "worker-2"),
            ("takeover", "true"),
            ("from", "worker-1"),
        ],
    )
    .unwrap();
    assert!(answer.ends_with(", epoch 2\n"), "{answer}");
    assert_eq!(epoch(&kernel), Some(2));
    // The plain detail and the graph face carry it too.
    let plain = source(&kernel, "urn:iki:ledger:item:1", &[]);
    assert!(plain.contains("\n  epoch:    2\n"), "{plain}");
    let turtle = source(&kernel, "urn:iki:ledger:item:1", &[("as", "text/turtle")]);
    assert!(turtle.contains("claimEpoch"), "{turtle}");
}

#[test]
fn a_renewal_keeps_the_epoch() {
    let kernel = ledger_with_an_item();
    claim(&kernel, &[("content", "worker-1"), ("lease", "1h")]).unwrap();
    let answer = claim(
        &kernel,
        &[("content", "worker-1"), ("lease", "1h"), ("renew", "true")],
    )
    .unwrap();
    assert!(answer.ends_with(", epoch 1\n"), "{answer}");
    // A plain claim by the same holder is a renewal too.
    claim(&kernel, &[("content", "worker-1")]).unwrap();
    assert_eq!(epoch(&kernel), Some(1));
    assert_eq!(stored_epochs(&kernel), ["1"]);
}

#[test]
fn a_release_then_a_new_claim_raises_the_epoch_and_it_never_goes_down() {
    let kernel = ledger_with_an_item();
    claim(&kernel, &[("content", "worker-1")]).unwrap();
    release(&kernel, &[]).unwrap();
    // No claim, so no epoch on the faces, but the store keeps the last one.
    assert_eq!(epoch(&kernel), None);
    let plain = source(&kernel, "urn:iki:ledger:item:1", &[]);
    assert!(!plain.contains("epoch:"), "{plain}");
    assert_eq!(stored_epochs(&kernel), ["1"]);
    // The same holder again, and then another after it.
    claim(&kernel, &[("content", "worker-1")]).unwrap();
    assert_eq!(epoch(&kernel), Some(2));
    release(&kernel, &[]).unwrap();
    claim(&kernel, &[("content", "worker-3")]).unwrap();
    assert_eq!(epoch(&kernel), Some(3));
    assert_eq!(stored_epochs(&kernel), ["3"]);
}

#[test]
fn a_close_leaves_the_epoch_so_a_reopened_items_next_claim_is_higher() {
    let kernel = ledger_with_an_item();
    claim(&kernel, &[("content", "worker-1")]).unwrap();
    sink(&kernel, "urn:iki:ledger:close", &[("item", "1")]);
    sink(&kernel, "urn:iki:ledger:reopen", &[("item", "1")]);
    claim(&kernel, &[("content", "worker-2")]).unwrap();
    assert_eq!(epoch(&kernel), Some(2));
}

// ---------------------------------------------------------------- the check

#[test]
fn the_check_answers_current_superseded_and_no_claim() {
    let kernel = taken_over();
    assert_eq!(
        check(&kernel, "2"),
        "current: #1 is held by worker-2 at epoch 2\n"
    );
    assert_eq!(
        check(&kernel, "1"),
        "superseded: #1 is held by worker-2 at epoch 2, not epoch 1\n"
    );
    release(&kernel, &[]).unwrap();
    assert_eq!(
        check(&kernel, "2"),
        "no claim: #1 is not claimed (its last claim was epoch 2), so epoch 2 is not current\n"
    );
    // Never claimed at all.
    sink(&kernel, "urn:iki:ledger:append", &[("content", "Another")]);
    assert_eq!(
        source(&kernel, CLAIM, &[("item", "2"), ("epoch", "1")]),
        "no claim: #2 is not claimed, so epoch 1 is not current\n"
    );
}

#[test]
fn the_check_needs_only_the_read_grant() {
    let kernel = taken_over();
    let reader = ikigai_core::Capability::scoped([
        "urn:cap:ledger:read:default".to_string(),
        graph_read("default"),
    ]);
    let answer = try_as(
        &kernel,
        &reader,
        Verb::Source,
        CLAIM,
        &[("item", "1"), ("epoch", "2")],
    )
    .unwrap();
    assert!(answer.starts_with("current:"), "{answer}");
}

#[test]
fn a_malformed_epoch_is_refused_naming_the_argument() {
    let kernel = taken_over();
    for bad in ["0", "-1", "two", ""] {
        match try_verb(
            &kernel,
            Verb::Source,
            CLAIM,
            &[("item", "1"), ("epoch", bad)],
        ) {
            Err(Error::InvalidArgument { name, .. }) => assert_eq!(name, "epoch"),
            other => panic!("epoch={bad}: {other:?}"),
        }
        match try_verb(
            &kernel,
            Verb::Sink,
            "urn:iki:ledger:comment",
            &[("item", "1"), ("content", "hi"), ("epoch", bad)],
        ) {
            Err(Error::InvalidArgument { name, .. }) => assert_eq!(name, "epoch"),
            other => panic!("comment epoch={bad}: {other:?}"),
        }
    }
}

// ---------------------------------------------------------------- fenced writes

#[test]
fn every_holder_write_with_a_stale_epoch_is_refused_and_writes_nothing() {
    let kernel = taken_over();
    let before = item_json(&kernel);
    let names = ["worker-2", "epoch 2", "not epoch 1"];
    stale(
        try_verb(
            &kernel,
            Verb::Sink,
            "urn:iki:ledger:comment",
            &[("item", "1"), ("content", "still on it"), ("epoch", "1")],
        ),
        "comment",
        &names,
    );
    stale(
        try_verb(
            &kernel,
            Verb::Sink,
            "urn:iki:ledger:comment",
            &[
                ("item", "1"),
                ("content", "done, please check"),
                ("handback", "true"),
                ("epoch", "1"),
            ],
        ),
        "hand-back",
        &names,
    );
    stale(
        try_verb(
            &kernel,
            Verb::Sink,
            "urn:iki:ledger:close",
            &[("item", "1"), ("epoch", "1")],
        ),
        "close",
        &names,
    );
    stale(
        claim(
            &kernel,
            &[("content", "worker-2"), ("renew", "true"), ("epoch", "1")],
        ),
        "renewal",
        &names,
    );
    stale(release(&kernel, &[("epoch", "1")]), "release", &names);
    stale(
        release(&kernel, &[("epoch", "1"), ("content", "a release note")]),
        "release with a note",
        &names,
    );
    let after = item_json(&kernel);
    assert_eq!(before, after, "a refused write changed the item");
    assert_eq!(comments(&kernel), 0);
    assert_eq!(holder(&kernel).as_deref(), Some("worker-2"));
    assert_eq!(after["status"], "open");
    assert!(after.get("awaiting").is_none_or(|a| a.is_null()), "{after}");
}

#[test]
fn a_write_fenced_on_a_released_claim_is_refused_naming_no_holder() {
    let kernel = ledger_with_an_item();
    claim(&kernel, &[("content", "worker-1")]).unwrap();
    release(&kernel, &[]).unwrap();
    stale(
        try_verb(
            &kernel,
            Verb::Sink,
            "urn:iki:ledger:comment",
            &[("item", "1"), ("content", "late"), ("epoch", "1")],
        ),
        "comment after release",
        &["not claimed", "epoch 1"],
    );
    assert_eq!(comments(&kernel), 0);
}

#[test]
fn every_holder_write_with_the_current_epoch_succeeds() {
    let kernel = taken_over();
    try_verb(
        &kernel,
        Verb::Sink,
        "urn:iki:ledger:comment",
        &[("item", "1"), ("content", "on it"), ("epoch", "2")],
    )
    .unwrap();
    let renewed = claim(
        &kernel,
        &[
            ("content", "worker-2"),
            ("renew", "true"),
            ("lease", "1h"),
            ("epoch", "2"),
        ],
    )
    .unwrap();
    assert!(renewed.starts_with("#1 renewed by worker-2"), "{renewed}");
    try_verb(
        &kernel,
        Verb::Sink,
        "urn:iki:ledger:comment",
        &[
            ("item", "1"),
            ("content", "done, please check"),
            ("handback", "true"),
            ("epoch", "2"),
        ],
    )
    .unwrap();
    assert_eq!(comments(&kernel), 2);
    release(&kernel, &[("epoch", "2")]).unwrap();
    assert_eq!(holder(&kernel), None);

    // Close, on a claim of its own.
    claim(&kernel, &[("content", "worker-4")]).unwrap();
    try_verb(
        &kernel,
        Verb::Sink,
        "urn:iki:ledger:close",
        &[("item", "1"), ("epoch", "3")],
    )
    .unwrap();
    assert_eq!(item_json(&kernel)["status"], "closed");
}

#[test]
fn without_epoch_a_stale_holder_writes_as_before() {
    let kernel = taken_over();
    // Backward compatible: unfenced, the old holder's comment lands, as it always did.
    try_verb(
        &kernel,
        Verb::Sink,
        "urn:iki:ledger:comment",
        &[("item", "1"), ("content", "still on it")],
    )
    .unwrap();
    assert_eq!(comments(&kernel), 1);
    release(&kernel, &[]).unwrap();
    assert_eq!(holder(&kernel), None);
}

// ---------------------------------------------------------------- races

#[test]
fn concurrent_takeovers_leave_exactly_one_current_epoch() {
    const RACERS: usize = 6;
    let kernel = ledger_with_an_item();
    claim(&kernel, &[("content", "worker-0"), ("lease", "1s")]).unwrap();
    let names: Vec<String> = (1..=RACERS).map(|n| format!("worker-{n}")).collect();
    let answers: Vec<Result<String, Error>> = std::thread::scope(|scope| {
        let handles: Vec<_> = names
            .iter()
            .map(|name| {
                let kernel = &kernel;
                scope.spawn(move || {
                    claim(
                        kernel,
                        &[
                            ("content", name.as_str()),
                            ("takeover", "true"),
                            ("from", "worker-0"),
                        ],
                    )
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let won: Vec<&String> = answers.iter().filter_map(|a| a.as_ref().ok()).collect();
    assert_eq!(won.len(), 1, "{answers:?}");
    assert!(won[0].ends_with(", epoch 2\n"), "{}", won[0]);
    assert_eq!(stored_epochs(&kernel), ["2"]);
    assert_eq!(epoch(&kernel), Some(2));
    let winner = holder(&kernel).unwrap();
    assert!(won[0].contains(&format!("by {winner}")), "{}", won[0]);
    assert!(check(&kernel, "2").starts_with("current:"));
}
