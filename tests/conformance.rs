//! The module recipe as one test: `ikigai-conformance` walks every resource this crate
//! binds and reports every violation at once.
//!
//! # The fixture is a ledger, and the walk WRITES to it
//!
//! The suite fires the mutating actions it checks, so the kernel under test is built over
//! a fresh in-memory store with three items already filed:
//!
//! - **A** is what most fixtures point at — it gets commented on, closed, reopened,
//!   claimed, released, deferred, resumed, labeled, linked and edited by the walk.
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
use std::sync::Arc;

use ikigai_conformance::{Check, Fixture, Suite};
use ikigai_core::{space_iri, Kernel, Verb};
use ikigai_ledger::policy::Leverage;
use ikigai_ledger::SpaceConfig;

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
    // ★ **The walk is scoped to THIS crate's endpoints** (`Suite::own`, conformance 0.8).
    // The store's resources are in this kernel because every ledger read and write
    // composes over them, but they are `ikigai-store`'s to conform, and its own suite hands
    // them real SPARQL, which the synthesized `"x"` never is. Until 0.8 this file opted out
    // twelve of them by hand; that list had already fallen one short (`store-graphs` was
    // walked here unasked), which is the drift a hand partition invites. Now everything
    // the store binds is INHERITED (not walked, no check reads it, no probe fires it), and
    // `the_walk_reaches_every_resource_this_crate_binds` pins the inherited list exactly,
    // so a store that gains a resource turns that test red by name instead of joining the
    // pile unseen.
    Suite::new()
        .own("ledger-*")
        // REQUIRED on the mutating actions too (conformance 0.8, ledger #329): every
        // required by-value input of every Sink and Delete is dropped once and the firing
        // must be refused with a typed `MissingArgument` or `InvalidArgument`. Off by
        // default because a wrongly accepted drop has WRITTEN; this kernel is a fixture
        // over an in-memory store, so a write here costs nothing but the finding. It is
        // where the check bites for this crate: the reads take their item by binding,
        // while `item`, `from`/`to` and `content` arrive by value on the writes.
        .required_on_mutating()
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
        // The item PARTS (ledger #775) point at B, which nothing in the walk deletes. The
        // transition is the identity (`filed` → `filed`), which is answered without a write:
        // the walk may fire a Sink more than once, and a real transition fired twice is a
        // Conflict the second time — correctly, and not a fact about the contract.
        .fixture(Fixture::new("ledger-item-state", Verb::Source).binding("id", b))
        .fixture(
            Fixture::new("ledger-item-state", Verb::Sink)
                .binding("id", b)
                .arg("from", "filed")
                .arg("to", "filed"),
        )
        .fixture(
            Fixture::new("ledger-item-state-is", Verb::Exists)
                .binding("id", b)
                .binding("value", "filed"),
        )
        .fixture(
            Fixture::new("ledger-item-holder-is", Verb::Exists)
                .binding("id", b)
                .binding("holder", "none"),
        )
        .fixture(Fixture::new("ledger-item-closed", Verb::Exists).binding("id", b))
        // `about` (ledger #398) points at B too: adding and removing a target leaves the
        // title, body and state the other parts read untouched.
        .fixture(
            Fixture::new("ledger-item-about", Verb::Sink)
                .binding("id", b)
                .arg("content", "urn:agents:session:conformance"),
        )
        .fixture(
            Fixture::new("ledger-item-about", Verb::Delete)
                .binding("id", b)
                .arg("content", "urn:agents:session:conformance"),
        )
        // `urn:iki:ledger:lifecycle:{name}` — the built-in.
        .fixture(Fixture::new("ledger-lifecycle", Verb::Source).binding("name", "kata-flight"))
        .fixture(Fixture::new("ledger-lifecycle", Verb::Exists).binding("name", "kata-flight"))
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
        .cacheable("ledger-count")
        .cacheable("ledger-item")
        .cacheable("ledger-next")
        .cacheable("ledger-item-state")
        .cacheable("ledger-item-state-is")
        .cacheable("ledger-item-holder-is")
        .cacheable("ledger-item-closed")
        .cacheable("ledger-doctor")
        .pure("ledger-policy")
        .pure("ledger-lifecycle")
        // RECOMPUTE (conformance 0.9, ledger #1567 — decided 2026-10-15): the doctor's text
        // face records when the check ran, so a forced recomputation differs by design.
        .opt_out_check(
            "ledger-doctor",
            Check::Recompute,
            "the text face records when the check ran",
        )
        // SPACE-NAME (ledger #987). `space()` takes no configuration and reads nothing while
        // it is built, so it names itself. The two configured constructors bind a host's own
        // policies, lifecycles and claim stamper behind the same IRIs, so they claim nothing
        // and the host names what it built.
        .self_named_space("ledger", ikigai_ledger::space)
        .host_named_space(
            "ikigai_ledger::space_with(SpaceConfig::default())",
            ikigai_ledger::space_with(SpaceConfig::default()),
        )
        .host_named_space(
            "ikigai_ledger::space_with_policies(policies)",
            ikigai_ledger::space_with_policies(vec![Arc::new(Leverage)]),
        )
}

#[test]
fn conforms() {
    let (kernel, a, b, c) = seeded();
    let report = suite(&a, &b, &c).run_blocking(&kernel);
    println!("{report}");
    assert!(report.is_clean(), "{report}");
    // FAILURES (conformance 0.10) is off unless a suite opts in (`check_failures()`, the
    // next wave); it is the only check allowed to be skipped.
    assert_eq!(
        report
            .checks
            .skipped()
            .map(|c| c.to_string())
            .collect::<Vec<_>>(),
        ["FAILURES"],
        "every check but FAILURES runs: {report}"
    );
    assert_eq!(space_iri("ledger").as_str(), ikigai_ledger::SPACE_ID);
}

/// ★ The positive half: a clean report over a walk that reached nothing would look
/// exactly like a clean report over a walk that reached everything. This pins the list.
#[test]
fn the_walk_reaches_every_resource_this_crate_binds() {
    let (kernel, a, b, c) = seeded();
    let report = suite(&a, &b, &c).run_blocking(&kernel);
    // `Suite::own("ledger-*")` walks only the twenty-two THIS crate binds, so the whole of
    // `walked` is the module; no filter stands between this list and what was checked.
    let mut walked: Vec<&str> = report.walked.iter().map(String::as_str).collect();
    walked.sort_unstable();
    assert_eq!(
        walked,
        vec![
            "ledger-append",
            "ledger-claim",
            "ledger-close",
            "ledger-comment",
            "ledger-count",
            "ledger-defer",
            "ledger-doctor",
            "ledger-item",
            "ledger-item-about",
            "ledger-item-closed",
            "ledger-item-holder-is",
            "ledger-item-state",
            "ledger-item-state-is",
            "ledger-items",
            "ledger-label",
            "ledger-ledgers",
            "ledger-lifecycle",
            "ledger-link",
            "ledger-next",
            "ledger-policy",
            "ledger-purge",
            "ledger-reopen",
        ],
        "{report}"
    );
    // And the other half, EXACTLY: what the store binds beside the ledger, inherited and
    // not walked. A store release that adds a resource turns this red by name, rather than
    // the resource joining the inherited pile unseen (the hand-written opt-out list this
    // replaced had already missed `store-graphs`).
    let mut inherited: Vec<&str> = report
        .declared
        .inherited
        .iter()
        .map(String::as_str)
        .collect();
    inherited.sort_unstable();
    assert_eq!(
        inherited,
        vec![
            "store-ask",
            "store-construct",
            "store-describe",
            "store-graph-ask",
            "store-graph-construct",
            "store-graph-describe",
            "store-graph-select",
            "store-graph-update",
            "store-graphs",
            "store-info",
            "store-load",
            "store-select",
            "store-update",
        ],
        "{report}"
    );
}
