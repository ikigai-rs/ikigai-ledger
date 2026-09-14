//! The module recipe as one test: `ikigai-conformance` walks every resource this crate
//! binds and reports every violation at once.
//!
//! # The fixture is a ledger, and the walk WRITES to it
//!
//! The suite fires the mutating actions it checks, so the kernel under test is built over
//! a fresh in-memory store with three items already filed:
//!
//! - **A** is what most fixtures point at — it gets commented on, closed, reopened,
//!   claimed, released, deferred, resumed, labelled, linked and edited by the walk.
//! - **B** exists so `ledger-link` has something to link TO.
//! - **C** exists so `ledger-purge` has something to destroy that nothing else needs.
//!
//! # What the suite cannot say, and this file says instead
//!
//! - **Every action that names an item needs a real one.** The synthesized `"x"` would be
//!   a `NotFound`, and a failed firing is reported by `OUTPUTS` as a failure — correctly,
//!   since what it serves was never observed.
//! - **`ledger-item`'s `Delete` declares no `content`**, so the pipeline probe never fires
//!   it and the report says `unprobed`. That is deliberate: a Delete's target is its own
//!   IRI, it needs no piped value, and a probe that fired it would destroy the fixture
//!   every other check on that endpoint reads. The delete path is covered end to end in
//!   `tests/endpoints.rs` — the tombstone, the quarantine, the hash and the counter.
//! - **The reads are cacheable under the store's write threads**, which this module
//!   inherits rather than declaring a thread of its own; `ledger-policy` is `pure` — it
//!   reads no state at all, which is why it is the one read here with no thread.
//! - **Every per-ledger resource is walked once, not twice**, even though each answers to
//!   two spellings. The bare `urn:iki:ledger:append` and the long
//!   `urn:iki:ledger:default:append` are one grammar match, so the space holds one entry
//!   — which matters here because the walk probes per ENTRY: a second binding for the
//!   same endpoint would fire `ledger-purge` and `ledger-item`'s Delete twice, and the
//!   second firing would be reported as a failure of a module that did nothing wrong.

mod common;

use common::*;
use ikigai_conformance::{Fixture, Suite};
use ikigai_core::{Kernel, Verb};

/// A kernel with three items filed, and their ids.
fn seeded() -> (Kernel, String, String, String) {
    let kernel = kernel();
    let a = append_iri(&kernel, "The fixture item\n\nWhat the walk writes to.", &[]);
    let b = append_iri(&kernel, "A link target", &[]);
    let c = append_iri(&kernel, "A purge target", &[]);
    let id = |iri: &str| {
        iri.rsplit(':')
            .next()
            .expect("an item IRI ends in its id")
            .to_string()
    };
    (kernel, id(&a), id(&b), id(&c))
}

fn suite(a: &str, b: &str, c: &str) -> Suite {
    // The CANONICAL spelling, which is what the store holds. The bare
    // `urn:iki:ledger:item:{id}` sugar resolves to the same items and is covered in
    // `tests/endpoints.rs`; the walk uses the long form so a failure here is never
    // ambiguous about which of the two was at fault.
    let item = format!("urn:iki:ledger:default:item:{a}");
    let target = format!("urn:iki:ledger:default:item:{b}");
    let purge_target = format!("urn:iki:ledger:default:item:{c}");
    let store_owned = [
        "store-select",
        "store-ask",
        "store-construct",
        "store-describe",
        "store-info",
        "store-update",
        "store-load",
    ];
    store_owned
        .iter()
        .fold(Suite::new(), |suite, id| {
            // ⚠ The store's resources are in THIS kernel because the ledger composes over
            // them, but they are `ikigai-store`'s to conform — and its own suite hands
            // them real SPARQL, which the synthesized `"x"` never is. Walking them here
            // would report another crate's endpoints against fixtures this crate has no
            // business writing.
            suite.opt_out(
                *id,
                None,
                "ikigai-store's own conformance suite covers it; it is bound in this \
                 kernel only because every ledger read and write composes over it",
            )
        })
        // The module's own namespace: every `ledger:` term in every probed face is
        // defined in `src/vocabulary.ttl`, which `ikigai-vocab` has never heard of.
        .namespace("https://ikigai-rs.dev/ns/ledger#")
        // `urn:iki:ledger:item:{id}` — one real item, for every verb on that entry.
        .fixture(Fixture::new("ledger-item", Verb::Source).binding("id", a))
        .fixture(Fixture::new("ledger-item", Verb::Exists).binding("id", a))
        .fixture(
            Fixture::new("ledger-item", Verb::Sink)
                .binding("id", a)
                .arg("content", "An edited title\n\nAnd an edited body."),
        )
        .fixture(Fixture::new("ledger-item", Verb::Delete).binding("id", a))
        // `urn:iki:ledger:policy:{name}` — a policy this host really offers.
        .fixture(Fixture::new("ledger-policy", Verb::Source).binding("name", "leverage"))
        .fixture(Fixture::new("ledger-policy", Verb::Exists).binding("name", "leverage"))
        // The mutating actions, each pointed at an item that exists.
        .fixture(
            Fixture::new("ledger-comment", Verb::Sink)
                .arg("item", &item)
                .arg("content", "a comment from the conformance walk"),
        )
        .fixture(Fixture::new("ledger-close", Verb::Sink).arg("item", &item))
        .fixture(Fixture::new("ledger-reopen", Verb::Sink).arg("item", &item))
        .fixture(
            Fixture::new("ledger-claim", Verb::Sink)
                .arg("item", &item)
                .arg("content", "the conformance walk"),
        )
        .fixture(Fixture::new("ledger-claim", Verb::Delete).arg("item", &item))
        .fixture(Fixture::new("ledger-defer", Verb::Sink).arg("item", &item))
        .fixture(Fixture::new("ledger-defer", Verb::Delete).arg("item", &item))
        .fixture(
            Fixture::new("ledger-link", Verb::Sink)
                .arg("item", &item)
                .arg("content", &target),
        )
        .fixture(
            Fixture::new("ledger-link", Verb::Delete)
                .arg("item", &item)
                .arg("content", &target),
        )
        .fixture(
            Fixture::new("ledger-label", Verb::Sink)
                .arg("item", &item)
                .arg("content", "conformance"),
        )
        .fixture(
            Fixture::new("ledger-label", Verb::Delete)
                .arg("item", &item)
                .arg("content", "conformance"),
        )
        .fixture(Fixture::new("ledger-purge", Verb::Delete).arg("content", &purge_target))
        // What the reads promise.
        .cacheable("ledger-items")
        .cacheable("ledger-item")
        .cacheable("ledger-next")
        .pure("ledger-policy")
}

#[test]
fn conforms() {
    let (kernel, a, b, c) = seeded();
    let report = suite(&a, &b, &c).run_blocking(&kernel);
    println!("{report}");
    assert!(report.is_clean(), "{report}");
}

/// ★ The positive half: a clean report over a walk that reached nothing would look
/// exactly like a clean report over a walk that reached everything. This pins the list.
#[test]
fn the_walk_reaches_every_resource_this_crate_binds() {
    let (kernel, a, b, c) = seeded();
    let report = suite(&a, &b, &c).run_blocking(&kernel);
    // The store's seven are in the walk too (they are bound in this kernel); this test is
    // about the fourteen THIS crate binds.
    let mut walked: Vec<&str> = report
        .walked
        .iter()
        .map(String::as_str)
        .filter(|id| id.starts_with("ledger-"))
        .collect();
    walked.sort_unstable();
    assert_eq!(
        walked,
        vec![
            "ledger-append",
            "ledger-claim",
            "ledger-close",
            "ledger-comment",
            "ledger-defer",
            "ledger-item",
            "ledger-items",
            "ledger-label",
            "ledger-ledgers",
            "ledger-link",
            "ledger-next",
            "ledger-policy",
            "ledger-purge",
            "ledger-reopen",
        ],
        "{report}"
    );
}
