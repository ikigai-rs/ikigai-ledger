//! **A release and a renewal that name the holder they mean** (ledger #1178, found by the
//! gonk starter's worker loop).
//!
//! Two gaps, each reproduced here before the fix:
//!
//! 1. `delete …:claim` released WHOEVER held the claim. A worker releasing "its" claim after
//!    a takeover dropped the new holder's. `from=<holder>` makes the release a
//!    compare-and-set: it refuses unless that holder still holds the claim.
//! 2. A same-holder claim of a FREE item took it, so a worker whose claim was released (or
//!    taken over and released) silently re-acquired the item on its next "renewal".
//!    `renew=true` refuses unless the claim is currently held by that holder.
//!
//! Both are opt-in: a release without `from=` and a claim without `renew=` behave as before.

mod common;

use common::*;
use ikigai_core::{Error, Kernel, Verb};

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
    try_verb(kernel, Verb::Sink, "urn:iki:ledger:claim", args)
}

fn release(kernel: &Kernel, args: &[(&str, &str)]) -> Result<String, Error> {
    try_verb(kernel, Verb::Delete, "urn:iki:ledger:claim", args)
}

fn holder(kernel: &Kernel) -> Option<String> {
    let view = source(
        kernel,
        "urn:iki:ledger:item:1",
        &[("as", "application/json")],
    );
    let doc: serde_json::Value = serde_json::from_str(&view).expect("JSON");
    doc["item"]["claim"]["holder"].as_str().map(str::to_string)
}

fn conflict(result: Result<String, Error>, what: &str) {
    match result {
        Err(Error::Conflict(_)) => {}
        other => panic!("{what}: expected Conflict, got {other:?}"),
    }
}

// ------------------------------------------------------------------ release from=

/// ★ The gap: a worker releasing what it believes is its own claim, after the claim changed
/// hands, must not drop the new holder's.
#[test]
fn a_release_from_another_holder_is_refused_and_changes_nothing() {
    let kernel = ledger_with_an_item();
    claim(&kernel, &[("item", "1"), ("content", "worker-2")]).unwrap();
    conflict(
        release(&kernel, &[("item", "1"), ("from", "worker-1")]),
        "release from the wrong holder",
    );
    assert_eq!(holder(&kernel).as_deref(), Some("worker-2"));
}

#[test]
fn a_release_from_the_holder_releases() {
    let kernel = ledger_with_an_item();
    claim(
        &kernel,
        &[("item", "1"), ("content", "worker-1"), ("lease", "1h")],
    )
    .unwrap();
    let released = release(&kernel, &[("item", "1"), ("from", "worker-1")]).unwrap();
    assert!(released.contains("released"), "{released}");
    assert_eq!(holder(&kernel), None);
    let view = source(
        &kernel,
        "urn:iki:ledger:item:1",
        &[("as", "application/json")],
    );
    let doc: serde_json::Value = serde_json::from_str(&view).unwrap();
    assert!(
        doc["item"]["claim"].is_null(),
        "every claim field went: {view}"
    );
}

#[test]
fn a_release_from_a_holder_of_an_unclaimed_item_is_refused() {
    let kernel = ledger_with_an_item();
    conflict(
        release(&kernel, &[("item", "1"), ("from", "worker-1")]),
        "release from on a free item",
    );
}

/// The release without `from=` is unchanged: it drops whoever holds the claim.
#[test]
fn a_release_without_from_still_releases_whoever_holds_it() {
    let kernel = ledger_with_an_item();
    claim(&kernel, &[("item", "1"), ("content", "worker-2")]).unwrap();
    release(&kernel, &[("item", "1")]).unwrap();
    assert_eq!(holder(&kernel), None);
}

#[test]
fn an_empty_from_is_refused() {
    let kernel = ledger_with_an_item();
    claim(&kernel, &[("item", "1"), ("content", "worker-1")]).unwrap();
    let refused = release(&kernel, &[("item", "1"), ("from", " ")]);
    assert!(
        matches!(refused, Err(Error::InvalidArgument { ref name, .. }) if name == "from"),
        "{refused:?}"
    );
    assert_eq!(holder(&kernel).as_deref(), Some("worker-1"));
}

// -------------------------------------------------------------------- renew=true

/// ★ The gap: a renewal of a claim that was released in the meantime must not re-take it.
#[test]
fn a_renewal_of_a_released_claim_is_refused() {
    let kernel = ledger_with_an_item();
    claim(
        &kernel,
        &[("item", "1"), ("content", "worker-1"), ("lease", "1h")],
    )
    .unwrap();
    release(&kernel, &[("item", "1")]).unwrap();
    conflict(
        claim(
            &kernel,
            &[
                ("item", "1"),
                ("content", "worker-1"),
                ("renew", "true"),
                ("lease", "1h"),
            ],
        ),
        "renewing a released claim",
    );
    assert_eq!(holder(&kernel), None);
}

#[test]
fn a_renewal_of_someone_elses_claim_is_refused() {
    let kernel = ledger_with_an_item();
    claim(&kernel, &[("item", "1"), ("content", "worker-2")]).unwrap();
    conflict(
        claim(
            &kernel,
            &[("item", "1"), ("content", "worker-1"), ("renew", "true")],
        ),
        "renewing another holder's claim",
    );
    assert_eq!(holder(&kernel).as_deref(), Some("worker-2"));
}

#[test]
fn a_renewal_by_the_holder_extends_the_lease() {
    let kernel = ledger_with_an_item();
    claim(
        &kernel,
        &[("item", "1"), ("content", "worker-1"), ("lease", "1h")],
    )
    .unwrap();
    let renewed = claim(
        &kernel,
        &[
            ("item", "1"),
            ("content", "worker-1"),
            ("renew", "true"),
            ("lease", "2h"),
        ],
    )
    .unwrap();
    assert!(renewed.contains("renewed"), "{renewed}");
    assert_eq!(holder(&kernel).as_deref(), Some("worker-1"));
}

/// A claim without `renew=` keeps its old meaning: a free item is taken.
#[test]
fn a_claim_without_renew_still_takes_a_free_item() {
    let kernel = ledger_with_an_item();
    claim(&kernel, &[("item", "1"), ("content", "worker-1")]).unwrap();
    release(&kernel, &[("item", "1")]).unwrap();
    claim(&kernel, &[("item", "1"), ("content", "worker-1")]).unwrap();
    assert_eq!(holder(&kernel).as_deref(), Some("worker-1"));
}

#[test]
fn renew_with_takeover_is_refused() {
    let kernel = ledger_with_an_item();
    let refused = claim(
        &kernel,
        &[
            ("item", "1"),
            ("content", "worker-1"),
            ("renew", "true"),
            ("takeover", "true"),
            ("from", "worker-2"),
        ],
    );
    assert!(
        matches!(refused, Err(Error::InvalidArgument { ref name, .. }) if name == "renew"),
        "{refused:?}"
    );
}
