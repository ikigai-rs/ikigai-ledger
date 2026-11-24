//! Ledger #4139: `items` and `count` hang on their ledgers' own graph threads, not on
//! `urn:iki:store:graph-update`, which every scoped write to ANY graph cuts. On a live server
//! (lease renewals and comments in every ledger) the general thread is cut many times a minute,
//! so a listing hung from it was almost never served from the cache.
//!
//! Each read below goes through `Kernel::issue_traced`, so a hit is a hit: one trace event,
//! marked `cache_hit`, and no store sub-request. Equal bytes alone could not tell a hit from
//! a recomputation.

mod common;
use common::*;
use ikigai_core::{Capability, Iri, Request, TraceEvent, Tracer, Verb};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Trace(Mutex<Vec<TraceEvent>>);
impl Tracer for Trace {
    fn record(&self, event: TraceEvent) {
        self.0.lock().unwrap().push(event);
    }
}

/// The reads that must stay cached, for the bound ledger `{ledger}`.
fn reads(ledger: &str) -> Vec<(String, Vec<(&'static str, &'static str)>)> {
    let items = format!("urn:iki:ledger:{ledger}:items");
    let count = format!("urn:iki:ledger:{ledger}:count");
    vec![
        (items.clone(), vec![]),
        (items.clone(), vec![("as", "application/json")]),
        (items.clone(), vec![("as", "text/turtle")]),
        (items, vec![("as", "application/json"), ("limit", "1")]),
        (count.clone(), vec![]),
        (count, vec![("as", "application/json")]),
    ]
}

/// The bytes, whether the read was a cache hit, and how many store sub-requests it issued.
fn read(kernel: &ikigai_core::Kernel, iri: &str, args: &[(&str, &str)]) -> (Vec<u8>, bool, usize) {
    let trace = Arc::new(Trace::default());
    let mut request = Request::new(Verb::Source, Iri::parse(iri).unwrap());
    for (name, value) in args {
        request = request.with_arg(
            *name,
            ikigai_core::ArgRef::Inline(value.as_bytes().to_vec()),
        );
    }
    let repr = futures::executor::block_on(kernel.issue_traced(
        request,
        &Capability::root(),
        trace.clone(),
    ))
    .unwrap();
    let events = trace.0.lock().unwrap();
    let hit = events.len() == 1 && events[0].cache_hit;
    (repr.bytes, hit, events.len().saturating_sub(1))
}

fn two_ledgers() -> ikigai_core::Kernel {
    let kernel = kernel();
    for ledger in ["alpha", "beta"] {
        sink(
            &kernel,
            &format!("urn:iki:ledger:{ledger}:append"),
            &[("content", "Cache me")],
        );
    }
    kernel
}

#[test]
fn a_write_to_another_ledger_leaves_items_and_count_cached() {
    let kernel = two_ledgers();
    let before: Vec<_> = reads("alpha")
        .iter()
        .map(|(iri, args)| read(&kernel, iri, args).0)
        .collect();
    for (iri, args) in reads("alpha") {
        let (_, hit, _) = read(&kernel, &iri, &args);
        assert!(hit, "{iri} {args:?} must be cached before the write");
    }
    // The writes a live server makes all the time, in ANOTHER ledger: a comment and a claim.
    sink(
        &kernel,
        "urn:iki:ledger:beta:comment",
        &[("item", "1"), ("content", "noise elsewhere")],
    );
    sink(
        &kernel,
        "urn:iki:ledger:beta:claim",
        &[("item", "1"), ("content", "worker-1"), ("lease", "10m")],
    );
    for ((iri, args), bytes) in reads("alpha").into_iter().zip(before) {
        let (after, hit, store) = read(&kernel, &iri, &args);
        assert_eq!(after, bytes, "{iri} {args:?} answered differently");
        assert!(
            hit,
            "{iri} {args:?} recomputed ({store} store sub-requests) after a write to beta"
        );
    }
}

#[test]
fn a_write_to_the_ledger_itself_still_recomputes_items_and_count() {
    let kernel = two_ledgers();
    let before: Vec<_> = reads("alpha")
        .iter()
        .map(|(iri, args)| read(&kernel, iri, args).0)
        .collect();
    sink(
        &kernel,
        "urn:iki:ledger:alpha:append",
        &[("content", "Only alpha changes")],
    );
    for ((iri, args), bytes) in reads("alpha").into_iter().zip(before) {
        let (after, hit, _) = read(&kernel, &iri, &args);
        assert!(!hit, "{iri} {args:?} must recompute after a write to alpha");
        assert_ne!(after, bytes, "{iri} {args:?} served the pre-write answer");
    }
}

/// A union listing reads every selected graph, so a write to any of them recomputes it, and a
/// write to a ledger outside the selection does not.
#[test]
fn a_union_listing_hangs_on_every_selected_ledger_and_no_other() {
    let kernel = two_ledgers();
    sink(
        &kernel,
        "urn:iki:ledger:gamma:append",
        &[("content", "Outside the union")],
    );
    let union = [
        ("urn:iki:ledger:items", vec![("ledgers", "alpha,beta")]),
        (
            "urn:iki:ledger:count",
            vec![("ledgers", "alpha,beta"), ("as", "application/json")],
        ),
    ];
    for (iri, args) in &union {
        read(&kernel, iri, args);
    }
    sink(
        &kernel,
        "urn:iki:ledger:gamma:comment",
        &[("item", "1"), ("content", "noise outside")],
    );
    for (iri, args) in &union {
        assert!(
            read(&kernel, iri, args).1,
            "{iri} {args:?} recomputed after gamma"
        );
    }
    sink(
        &kernel,
        "urn:iki:ledger:beta:append",
        &[("content", "Inside the union")],
    );
    for (iri, args) in &union {
        let (bytes, hit, _) = read(&kernel, iri, args);
        assert!(!hit, "{iri} {args:?} must recompute after beta");
        let text = String::from_utf8(bytes).unwrap();
        assert!(
            text.contains("Inside the union") || text.contains("\"total\":3"),
            "{iri} {args:?}: {text}"
        );
    }
}

/// A write straight at the store's narrow door into this ledger's graph, not through a ledger
/// Sink, still recomputes the listing: the graph's own thread is cut by the store itself.
#[test]
fn a_write_through_the_narrow_door_still_recomputes_the_listing() {
    let kernel = kernel();
    let iri = append_iri(&kernel, "The first item", &[]);
    read(&kernel, "urn:iki:ledger:items", &[]);
    sink(
        &kernel,
        "urn:iki:store:graph-update",
        &[
            ("graph", "urn:iki:ledger:graph:default"),
            (
                "content",
                &format!(
                    "INSERT DATA {{ GRAPH <urn:iki:ledger:graph:default> {{ \
                     <{iri}> <https://ikigai-rs.dev/ns/ledger#label> \"out-of-band\" }} }}"
                ),
            ),
        ],
    );
    let (bytes, hit, _) = read(&kernel, "urn:iki:ledger:items", &[]);
    assert!(!hit);
    assert!(String::from_utf8(bytes).unwrap().contains("out-of-band"));
}

/// The measurement behind ledger #4139, as a test: a home page reading one ledger's listing
/// and count between writes to ANOTHER ledger, as a live server's lease renewals and comments
/// make them. Every load after the first is served from the cache, with no store sub-request.
#[test]
fn a_home_page_load_between_writes_elsewhere_issues_no_store_reads() {
    let kernel = two_ledgers();
    for n in 0..20 {
        sink(
            &kernel,
            "urn:iki:ledger:alpha:append",
            &[("content", &format!("Alpha item {n}"))],
        );
    }
    let page = [
        (
            "urn:iki:ledger:alpha:items",
            vec![("as", "application/json")],
        ),
        ("urn:iki:ledger:alpha:count", vec![]),
    ];
    let load = || {
        page.iter()
            .map(|(iri, args)| read(&kernel, iri, args).2)
            .sum::<usize>()
    };
    let first = load();
    let mut store = 0;
    let mut loading = std::time::Duration::ZERO;
    for n in 0..50 {
        sink(
            &kernel,
            "urn:iki:ledger:beta:comment",
            &[("item", "1"), ("content", &format!("renewal {n}"))],
        );
        let started = std::time::Instant::now();
        store += load();
        loading += started.elapsed();
    }
    eprintln!(
        "first load: {first} store sub-requests; 50 loads between beta writes: {store} store \
         sub-requests, {loading:?} loading"
    );
    assert!(first > 0);
    assert_eq!(
        store, 0,
        "a load recomputed after a write to another ledger"
    );
}
