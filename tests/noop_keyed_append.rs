//! **A keyed append that finds its key taken writes nothing, so it must not invalidate
//! anything** (ledger #822).
//!
//! A re-sync — the gonk Book's OpenSpec bridge, the roborev and kata bridges — sends one
//! keyed append per thing it knows about, every time, and most of them find their key
//! taken. If each of those still reached the store as a write, the kernel's auto-cut on
//! that successful Sink (`urn:iki:store:graph-update`) would invalidate every cached read
//! of every ledger, and an idempotent sync would be a cache flush.
//!
//! Observed with `Kernel::is_cached`, the kernel's own answer to "would this be served from
//! the cache", rather than by timing.

mod common;

use common::*;
use ikigai_core::{ArgRef, Capability, Iri, Kernel, Request, Verb};

const KEY: &str = "urn:openspec:task:add-auth:1.2";

/// The three reads a re-sync's caller (and everyone else) keeps warm.
const READS: [(&str, &[(&str, &str)]); 3] = [
    ("urn:iki:ledger:next", &[("policy", "priority-recency")]),
    ("urn:iki:ledger:items", &[]),
    ("urn:iki:ledger:item:1", &[]),
];

fn request(iri: &str, args: &[(&str, &str)]) -> Request {
    args.iter().fold(
        Request::new(Verb::Source, Iri::parse(iri).expect("a test IRI")),
        |request, (name, value)| request.with_arg(*name, ArgRef::Inline(value.as_bytes().to_vec())),
    )
}

fn warm(kernel: &Kernel) {
    for (iri, args) in READS {
        source(kernel, iri, args);
    }
}

fn cached(kernel: &Kernel) -> Vec<(&'static str, bool)> {
    READS
        .iter()
        .map(|(iri, args)| {
            (
                *iri,
                kernel.is_cached(&request(iri, args), &Capability::root()),
            )
        })
        .collect()
}

fn keyed(kernel: &Kernel, content: &str) -> String {
    sink(
        kernel,
        "urn:iki:ledger:append",
        &[("content", content), ("key", KEY)],
    )
}

/// ★ The defect: a replay of a keyed append must leave every cached read cached.
#[test]
fn a_keyed_append_whose_key_is_taken_leaves_cached_reads_cached() {
    let kernel = kernel();
    let first = keyed(&kernel, "Add the login form");
    warm(&kernel);
    assert!(
        cached(&kernel).iter().all(|(_, hit)| *hit),
        "the reads did not cache in the first place: {:?}",
        cached(&kernel)
    );

    let replay = keyed(&kernel, "Add the login form, reworded");
    assert!(replay.contains("existing open"), "{replay}");
    assert_eq!(
        replay.split_whitespace().nth(1),
        first.split_whitespace().nth(1),
        "the replay names the item the first append filed"
    );

    let after = cached(&kernel);
    assert!(
        after.iter().all(|(_, hit)| *hit),
        "a keyed append that wrote nothing invalidated cached reads: {after:?}"
    );
}

/// The other face: a keyed append that DOES file — a new key — still invalidates, so the
/// fix is "skip the write that changes nothing", never "skip the cut".
#[test]
fn a_keyed_append_that_files_still_invalidates_cached_reads() {
    let kernel = kernel();
    keyed(&kernel, "Add the login form");
    warm(&kernel);
    let filed = sink(
        &kernel,
        "urn:iki:ledger:append",
        &[
            ("content", "Add the logout button"),
            ("key", "urn:openspec:task:add-auth:1.3"),
        ],
    );
    assert!(!filed.contains("existing"), "{filed}");
    let after = cached(&kernel);
    for (name, hit) in &after {
        if *name != "urn:iki:ledger:item:1" {
            assert!(!hit, "a filing left {name} cached: {after:?}");
        }
    }
}
