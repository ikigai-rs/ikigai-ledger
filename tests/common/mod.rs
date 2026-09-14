//! The fixture every suite shares: a kernel with a store space, a ledger space and a
//! clock.
//!
//! ⚠ **The clock is not optional.** Every write here is stamped and this module refuses
//! to write without one, so a kernel built for these tests carries a clock — an advancing
//! one, because `created`/`modified` ordering is load-bearing for the default policy and a
//! frozen clock would make every item a tie.

#![allow(dead_code)] // each suite uses a different subset of these helpers

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use futures::executor::block_on;
use ikigai_core::{
    ArgRef, Capability, Clock, Error, Fallback, Iri, Kernel, Request, Space, Time, Verb,
};
use ikigai_store::DurableStore;

/// A clock that advances one second per reading, starting at 2026-09-15T00:00:00Z.
///
/// Advancing rather than fixed: two items filed "at the same instant" would tie on every
/// recency rule, so a frozen clock would quietly make the default policy's tie-break
/// untested.
pub struct TickingClock(AtomicU64);

impl Default for TickingClock {
    fn default() -> Self {
        TickingClock(AtomicU64::new(1_789_430_400_000))
    }
}

impl Clock for TickingClock {
    fn now(&self) -> Time {
        Time::from_millis(self.0.fetch_add(1_000, Ordering::SeqCst))
    }
}

/// A kernel over an in-memory store with the ledger bound beside it.
pub fn kernel() -> Kernel {
    kernel_over(DurableStore::in_memory().expect("an in-memory store"))
}

/// A kernel over a caller-supplied store — the durable suite passes `DurableStore::open`.
pub fn kernel_over(store: DurableStore) -> Kernel {
    let space = Fallback::new(vec![
        Arc::new(ikigai_store::space(store)) as Arc<dyn Space>,
        Arc::new(ikigai_ledger::space()) as Arc<dyn Space>,
    ]);
    Kernel::with_meta_renderer(Arc::new(space), Arc::new(ikigai_vocab::TurtleRenderer))
        .with_clock(Arc::new(TickingClock::default()))
}

/// A kernel with the ledger bound and NO store — the composition failure a host hits.
pub fn kernel_without_store() -> Kernel {
    Kernel::new(Arc::new(ikigai_ledger::space())).with_clock(Arc::new(TickingClock::default()))
}

fn request(verb: Verb, iri: &str, args: &[(&str, &str)]) -> Request {
    args.iter().fold(
        Request::new(verb, Iri::parse(iri).expect("a test IRI")),
        |request, (name, value)| request.with_arg(*name, ArgRef::Inline(value.as_bytes().to_vec())),
    )
}

/// Resolve under root, expecting success, and return the body as text.
pub fn ok(kernel: &Kernel, verb: Verb, iri: &str, args: &[(&str, &str)]) -> String {
    try_verb(kernel, verb, iri, args)
        .unwrap_or_else(|e| panic!("{verb:?} {iri} {args:?} failed: {e}"))
}

/// Resolve under root and hand back the result.
pub fn try_verb(
    kernel: &Kernel,
    verb: Verb,
    iri: &str,
    args: &[(&str, &str)],
) -> Result<String, Error> {
    try_as(kernel, &Capability::root(), verb, iri, args)
}

/// Resolve under a specific capability — the ablation tests' door.
pub fn try_as(
    kernel: &Kernel,
    capability: &Capability,
    verb: Verb,
    iri: &str,
    args: &[(&str, &str)],
) -> Result<String, Error> {
    block_on(kernel.issue(request(verb, iri, args), capability))
        .map(|repr| String::from_utf8_lossy(&repr.bytes).into_owned())
}

/// `Source` under root.
pub fn source(kernel: &Kernel, iri: &str, args: &[(&str, &str)]) -> String {
    ok(kernel, Verb::Source, iri, args)
}

/// `Sink` under root.
pub fn sink(kernel: &Kernel, iri: &str, args: &[(&str, &str)]) -> String {
    ok(kernel, Verb::Sink, iri, args)
}

/// `Delete` under root.
pub fn delete(kernel: &Kernel, iri: &str, args: &[(&str, &str)]) -> String {
    ok(kernel, Verb::Delete, iri, args)
}

/// File an item and return its `#N` number as text (`"#1"`).
pub fn append(kernel: &Kernel, content: &str, args: &[(&str, &str)]) -> String {
    let mut all = vec![("content", content)];
    all.extend_from_slice(args);
    let answer = sink(kernel, "urn:iki:ledger:append", &all);
    answer
        .split_whitespace()
        .next()
        .expect("append answers with the number and the IRI")
        .to_string()
}

/// The full IRI of a filed item, from the same answer.
pub fn append_iri(kernel: &Kernel, content: &str, args: &[(&str, &str)]) -> String {
    let mut all = vec![("content", content)];
    all.extend_from_slice(args);
    let answer = sink(kernel, "urn:iki:ledger:append", &all);
    answer
        .split_whitespace()
        .nth(1)
        .expect("append answers with the number and the IRI")
        .to_string()
}

/// A SPARQL SELECT straight at the store — how a test asserts what is really in the
/// graph, rather than what an endpoint says is.
pub fn select(kernel: &Kernel, query: &str) -> String {
    source(kernel, "urn:iki:store:select", &[("query", query)])
}

// ------------------------------------------------------------------- the store grants
//
// ★ **Spelled out, never computed.** Every token below could be produced by calling
// `ikigai_store::cap_read_graph(&Ledger::parse(name)?.graph())` — which is exactly what
// the code under test does, and a test that derives the same string the same way asserts
// only that a function is deterministic. These are literals so that a change to how a
// graph IRI or a grant token is spelled fails here, in the place an operator's config
// file would have to change too.

/// The store grant a caller needs to READ one ledger's graph.
pub fn graph_read(ledger: &str) -> String {
    format!("urn:cap:store:read:graph:urn:iki:ledger:graph:{ledger}")
}

/// The store grant a caller needs to WRITE one ledger's graph.
pub fn graph_write(ledger: &str) -> String {
    format!("urn:cap:store:write:graph:urn:iki:ledger:graph:{ledger}")
}

/// The store grant a delete or a purge needs **in addition**: the ledger's graveyard is a
/// second graph and therefore a second token.
pub fn graveyard_write(ledger: &str) -> String {
    format!("urn:cap:store:write:graph:urn:iki:ledger:graph:{ledger}:deleted")
}
