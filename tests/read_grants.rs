//! **Every READ this crate binds, under exactly the read row of `README.md`'s grant table**,
//! and what a script run on a host actually carries when it is refused (ledger #1220).
//!
//! # Why this file exists
//!
//! A published plan whose one step is `Source urn:iki:ledger:library:next` was refused at run
//! on gonk with `capability does not grant urn:cap:store:read:graph:* (declared by
//! urn:iki:ledger:library:next)`, and the report read it as `next` declaring a scope that the
//! run's exact grants could not satisfy. It does not, and this file is the evidence:
//!
//! 1. [`the_read_row_alone_reads_every_read_resource`] runs every `Source` and `Exists` this
//!    crate binds — `next` among them — under the TWO tokens the read row names for one
//!    ledger, and nothing else. A caller holding the exact per-ledger tokens satisfies every
//!    read's declaration, because a declared family (`…:*`) is satisfied by any grant under
//!    its prefix, which is what an exact token is.
//! 2. [`every_read_declares_what_the_read_row_satisfies`] is the same claim against the
//!    contracts alone, through core's own predicate rather than a copy of it. `next` declares
//!    exactly what `items`, `count` and `item:{id}` declare; no read is singled out.
//! 3. [`a_family_marker_is_not_a_grant`] pins what the refused run really carried. Derived
//!    with `ikigai-script` 0.3.0 under gonk's ceiling (the reproduction is in the PR that
//!    added this file): a plan published by ROOT and run by ROOT (the owner's socket) keeps
//!    each derived family as a marker, and `plan::expand_families` then has nothing to expand
//!    it into — root holds no exact grant to pick, and gonk's ceiling names only families — so
//!    the run is attenuated to the literal `urn:cap:ledger:read:*`, and the store's family,
//!    which the ceiling's narrower `…:graph:urn:iki:ledger:graph:*` does not `allows`, is
//!    dropped entirely. The kernel's floor then refuses `next` with the reported message.
//!    Holding BOTH markers gets past the floor and is refused inside, exactly, because a
//!    marker names no ledger: so no declaration this crate could make would admit that run.
//!    The defect is where the run's capability is derived, not here.
//!
//! The exact-grant runs (root publisher with a non-root runner, or an exact publisher with
//! either runner) carry `urn:cap:ledger:read:{L}` and the ledger graph's exact read token, and
//! test 1 is the proof those read `next`.

mod common;

use std::collections::BTreeSet;

use ikigai_core::{Capability, Error, Iri, Request, Resolution, Scope, Space, Verb};

use common::*;

/// A named ledger, never `default`, so a scope that ignored the ledger segment would still
/// have to name this graph.
const LEDGER: &str = "library";

/// The read row of `README.md`'s grant table for one ledger, literally: the module's read
/// grant and the store's read grant for that ledger's graph. **Two tokens, nothing else.**
fn read_row(ledger: &str) -> Capability {
    Capability::scoped([format!("urn:cap:ledger:read:{ledger}"), graph_read(ledger)])
}

fn is_read(verb: Verb) -> bool {
    matches!(verb, Verb::Source | Verb::Exists)
}

/// A bound pattern expanded into an IRI that resolves (the same expansion `tests/grants.rs`
/// uses; an unexpanded `{` is a new binding this file has to be taught about).
fn concrete(pattern: &str) -> String {
    let iri = pattern
        .replace("{ledger}", LEDGER)
        .replace("{id}", "1")
        .replace("{name}", "leverage")
        .replace("{value}", "filed")
        .replace("{holder}", "none");
    assert!(
        !iri.contains('{'),
        "`{pattern}` has a template variable this test cannot expand"
    );
    iri
}

/// Every bound `(endpoint, verb)` whose verb reads, with the description that declares it,
/// read off the space rather than written down here.
fn bound_reads() -> Vec<(String, Verb, ikigai_core::Description)> {
    let space = ikigai_ledger::space();
    let scope = Scope::empty();
    let mut reads = Vec::new();
    for entry in space
        .entries()
        .expect("an EndpointSpace enumerates its bindings")
    {
        let iri = Iri::parse(concrete(&entry.pattern)).expect("an expanded pattern is an IRI");
        let Resolution::Hit(resolved) = space.resolve(&Request::new(Verb::Meta, iri), &scope)
        else {
            panic!("`{}` does not resolve its own pattern", entry.endpoint);
        };
        let description = resolved.endpoint.describe();
        for spec in description.action_specs() {
            if is_read(spec.verb) {
                reads.push((entry.endpoint.clone(), spec.verb, description.clone()));
            }
        }
    }
    assert!(!reads.is_empty(), "the ledger space binds no read");
    reads
}

fn action(endpoint: &str, verb: Verb) -> String {
    format!("{endpoint} {verb:?}")
}

/// One read per bound read action, in the ledger the fixture files into.
fn read_script() -> Vec<(&'static str, Verb, String)> {
    let at = |resource: &str| format!("urn:iki:ledger:{LEDGER}:{resource}");
    vec![
        ("ledger-items", Verb::Source, at("items")),
        ("ledger-count", Verb::Source, at("count")),
        ("ledger-next", Verb::Source, at("next")),
        ("ledger-item", Verb::Source, at("item:1")),
        ("ledger-item", Verb::Exists, at("item:1")),
        ("ledger-item-state", Verb::Source, at("item:1:state")),
        (
            "ledger-item-state-is",
            Verb::Exists,
            at("item:1:state:filed"),
        ),
        (
            "ledger-item-holder-is",
            Verb::Exists,
            at("item:1:holder:none"),
        ),
        ("ledger-item-closed", Verb::Exists, at("item:1:closed")),
        ("ledger-doctor", Verb::Source, at("doctor")),
        (
            "ledger-ledgers",
            Verb::Source,
            "urn:iki:ledger:ledgers".to_string(),
        ),
        (
            "ledger-policy",
            Verb::Source,
            "urn:iki:ledger:policy:leverage".to_string(),
        ),
        (
            "ledger-policy",
            Verb::Exists,
            "urn:iki:ledger:policy:leverage".to_string(),
        ),
        (
            "ledger-lifecycle",
            Verb::Source,
            "urn:iki:ledger:lifecycle:kata-flight".to_string(),
        ),
        (
            "ledger-lifecycle",
            Verb::Exists,
            "urn:iki:ledger:lifecycle:kata-flight".to_string(),
        ),
    ]
}

/// A kernel with two items filed in [`LEDGER`] (as root: filing is not what is under test).
fn seeded() -> ikigai_core::Kernel {
    let kernel = kernel();
    let append = format!("urn:iki:ledger:{LEDGER}:append");
    sink(&kernel, &append, &[("content", "The first item")]);
    sink(&kernel, &append, &[("content", "The second item")]);
    kernel
}

/// ★ **The test ledger #1220 asked for.** Every bound read, `next` included, exercised for real
/// under the read row's two tokens and nothing more.
#[test]
fn the_read_row_alone_reads_every_read_resource() {
    let bound: BTreeSet<String> = bound_reads()
        .iter()
        .map(|(endpoint, verb, _)| action(endpoint, *verb))
        .collect();
    let script = read_script();
    let scripted: BTreeSet<String> = script
        .iter()
        .map(|(endpoint, verb, _)| action(endpoint, *verb))
        .collect();
    let unexercised: Vec<&String> = bound.difference(&scripted).collect();
    assert!(
        unexercised.is_empty(),
        "these reads are bound and never exercised under the read row — add them to \
         `read_script()`: {unexercised:?}"
    );
    let stale: Vec<&String> = scripted.difference(&bound).collect();
    assert!(
        stale.is_empty(),
        "`read_script()` names reads this crate does not bind: {stale:?}"
    );

    let kernel = seeded();
    let caller = read_row(LEDGER);
    for (endpoint, verb, iri) in &script {
        let answered = try_as(&kernel, &caller, *verb, iri, &[]);
        assert!(
            answered.is_ok(),
            "{} at {iri} is refused the read row: {answered:?}",
            action(endpoint, *verb)
        );
    }
    // And `next` answers the ledger's own work, not an empty list a refusal could hide in.
    let next = try_as(
        &kernel,
        &caller,
        Verb::Source,
        &format!("urn:iki:ledger:{LEDGER}:next"),
        &[],
    )
    .expect("next under the read row");
    assert!(next.contains("The first item"), "{next}");
}

/// The contract half: every read's declared scopes are satisfied by the read row, asked
/// through core's own predicate (`Description::unsatisfied_scopes`, the one the floor,
/// `urn:kernel:actions` and `urn:kernel:validate` share). And `next` declares the same scopes
/// as the other item reads, so no read carries a broader declaration than its siblings.
#[test]
fn every_read_declares_what_the_read_row_satisfies() {
    let caller = read_row(LEDGER);
    let mut unmet = Vec::new();
    let mut declared_by = std::collections::BTreeMap::new();
    for (endpoint, verb, description) in bound_reads() {
        let missing = description.unsatisfied_scopes(verb, &caller);
        if !missing.is_empty() {
            unmet.push(format!("{} lacks {missing:?}", action(&endpoint, verb)));
        }
        declared_by.insert(action(&endpoint, verb), description.required_scopes(verb));
    }
    assert!(
        unmet.is_empty(),
        "the read row does not satisfy these reads' declarations: {unmet:?}"
    );
    let next = &declared_by[&action("ledger-next", Verb::Source)];
    for sibling in ["ledger-items", "ledger-count", "ledger-item"] {
        assert_eq!(
            &declared_by[&action(sibling, Verb::Source)],
            next,
            "`next` declares something its sibling `{sibling}` does not"
        );
    }
}

/// ⚠ **What the refused script run carried, and why it is refused correctly here.**
///
/// A capability holding a FAMILY literally holds no grant: core's `Capability::allows` is
/// exact set membership, and a family in `requires` asks for "some grant under this prefix",
/// which the marker itself only appears to be. So:
///
/// - the literal `urn:cap:ledger:read:*` alone (what a root-published plan run by root carries
///   under gonk's ceiling) is refused by the kernel's floor with the message ledger #1220
///   reported, naming the store family the run's derivation dropped;
/// - both markers pass the floor (each is "under" its own prefix) and are refused inside, at
///   the exact per-ledger check, because neither names `library`.
///
/// Both refusals are this crate behaving as documented. The run needs exact grants, and
/// deriving them is the script host's job.
#[test]
fn a_family_marker_is_not_a_grant() {
    let kernel = seeded();
    let next = format!("urn:iki:ledger:{LEDGER}:next");

    let as_run_on_gonk = Capability::scoped(["urn:cap:ledger:read:*"]);
    match try_as(&kernel, &as_run_on_gonk, Verb::Source, &next, &[]) {
        Err(Error::Denied(message)) => assert!(
            message.contains("does not grant `urn:cap:store:read:graph:*`")
                && message.contains(&next),
            "{message}"
        ),
        other => panic!("a lone ledger family marker read next: {other:?}"),
    }

    let both_markers = Capability::scoped(["urn:cap:ledger:read:*", "urn:cap:store:read:graph:*"]);
    match try_as(&kernel, &both_markers, Verb::Source, &next, &[]) {
        Err(Error::Denied(message)) => assert!(
            message.contains(&format!("urn:cap:ledger:read:{LEDGER}")),
            "{message}"
        ),
        other => panic!("two family markers read next: {other:?}"),
    }
}
