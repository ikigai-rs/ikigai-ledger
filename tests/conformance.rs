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
use ikigai_core::{space_iri, Capability, Fallback, Kernel, Space, Verb};
use ikigai_ledger::policy::Leverage;
use ikigai_ledger::SpaceConfig;
use ikigai_store::DurableStore;

const PARTICIPANTS: &str = "urn:collab:conformance:participants";
const IMPORTER: &str = "urn:iki:gonk:client:conformance-importer";

/// A kernel with three items filed, and their ids. Its ledger reads enrollment from
/// [`PARTICIPANTS`], where [`IMPORTER`] is an import principal, for `ledger-history`.
fn seeded() -> (Kernel, String, String, String) {
    let space = Fallback::new(vec![
        Arc::new(ikigai_store::space(DurableStore::in_memory().unwrap())) as Arc<dyn Space>,
        Arc::new(ikigai_ledger::space_with(
            SpaceConfig::default().participants(PARTICIPANTS),
        )) as Arc<dyn Space>,
    ]);
    let kernel =
        Kernel::with_meta_renderer(Arc::new(space), Arc::new(ikigai_vocab::TurtleRenderer))
            .with_clock(Arc::new(TickingClock::default()));
    sink(
        &kernel,
        "urn:iki:store:update",
        &[(
            "content",
            &format!(
                "INSERT DATA {{ GRAPH <{PARTICIPANTS}> {{ <{IMPORTER}> a \
                 <https://ikigai-rs.dev/ns/ledger#Importer> }} }}"
            ),
        )],
    );
    let a = append_iri(&kernel, "The fixture item\n\nWhat the walk writes to.", &[]);
    let b = append_iri(&kernel, "A link target", &[]);
    let c = append_iri(&kernel, "A purge target", &[]);
    let migration = append_iri(
        &kernel,
        "Migration fixture",
        &[("key", "urn:test:backfill")],
    );
    sink(&kernel, "urn:iki:ledger:close", &[("item", &migration)]);
    // Last, so no fixture's dates move: a claim taken and released, so `a` has granted epoch 1 whatever order the walk fires
    // the claim writes in: the epoch check's fixture asks about it (ledger #3205).
    sink(
        &kernel,
        "urn:iki:ledger:claim",
        &[("item", &a), ("content", "the conformance seed")],
    );
    delete(&kernel, "urn:iki:ledger:claim", &[("item", &a)]);
    // Item #5, handed back, so a session summary has a hand-back to be recorded on
    // (ledger #3022). Filed after everything else, so no other fixture's dates move.
    let sessions = append_iri(&kernel, "A session fixture", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:comment",
        &[
            ("item", &sessions),
            ("content", "over to you"),
            ("handback", "true"),
        ],
    );
    // #6 is held by a principal at epoch 1 and nothing in the walk moves its claim: the
    // dispatch note (ledger #3718) is sent to that claim, and its holder reads the notes.
    append_iri(&kernel, "A dispatch note fixture", &[]);
    try_as(
        &kernel,
        &note_holder(),
        Verb::Sink,
        "urn:iki:ledger:claim",
        &[("item", "6"), ("content", "the conformance worker")],
    )
    .expect("the note fixture is claimed");
    let id = |iri: &str| {
        iri.rsplit(':')
            .next()
            .expect("an item IRI ends in its id")
            .to_string()
    };
    (kernel, id(&a), id(&b), id(&c))
}

/// The holder of #6's claim, a principal, who reads the notes to it.
fn note_holder() -> Capability {
    Capability::scoped(vec![
        "urn:cap:ledger:read:default".to_string(),
        "urn:cap:ledger:claim:default".to_string(),
        graph_read("default"),
        graph_write("default"),
    ])
    .with_principal("urn:conformance:worker")
    .unwrap()
}

/// The dispatcher who sends #6's holder a note: the close grant, with a principal.
fn note_sender() -> Capability {
    Capability::scoped(vec![
        "urn:cap:ledger:read:default".to_string(),
        "urn:cap:ledger:close:default".to_string(),
        graph_read("default"),
        graph_write("default"),
    ])
    .with_principal("urn:conformance:dispatcher")
    .unwrap()
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
        .fixture(Fixture::new("ledger-item-history", Verb::Source).binding("id", b))
        // An importer's history (ledger #2843) answers about its caller, so it is walked under
        // the principal [`PARTICIPANTS`] enrolls. Its date is after B's filing, in the seeding's
        // first ticks of the clock, and before the walk reaches it; a second firing is a re-send,
        // which changes nothing.
        .fixture(
            Fixture::new("ledger-history", Verb::Sink)
                .capability(
                    Capability::scoped(vec![
                        "urn:cap:ledger:write:default".to_string(),
                        graph_read("default"),
                        graph_write("default"),
                        format!("urn:cap:store:read:graph:{PARTICIPANTS}"),
                    ])
                    .with_principal(IMPORTER)
                    .unwrap(),
                )
                .arg("item", b)
                .arg(
                    "content",
                    "<urn:conformance:event:1> \
                     a <https://ikigai-rs.dev/ns/ledger#EditEvent> ; \
                     <http://purl.org/dc/terms/created> \
                     \"2026-09-15T00:00:30Z\"^^<http://www.w3.org/2001/XMLSchema#dateTime> ; \
                     <https://ikigai-rs.dev/ns/ledger#editKind> \"defer\" .",
                ),
        )
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
        // `item:{id}:sessions` (ledger #3022) points at #5, the handed-back fixture. A second
        // firing of the Sink is a resend of the same session, which writes nothing.
        .fixture(Fixture::new("ledger-item-sessions", Verb::Source).binding("id", "5"))
        // The migration of ledger #3661: a plan writes nothing, so a walk may fire it freely.
        .fixture(Fixture::new("ledger-sessions", Verb::Sink).arg("content", "plan"))
        .fixture(
            Fixture::new("ledger-item-sessions", Verb::Sink)
                .binding("id", "5")
                .arg(
                    "content",
                    r#"{"session": "conformance", "machineName": "walk", "harness": "codex", "model": "m", "exitStatus": 0, "wallClockMs": 1}"#,
                ),
        )
        // `urn:iki:ledger:lifecycle:{name}` — the built-in.
        .fixture(Fixture::new("ledger-lifecycle", Verb::Source).binding("name", "kata-flight"))
        .fixture(Fixture::new("ledger-lifecycle", Verb::Exists).binding("name", "kata-flight"))
        // `urn:iki:ledger:policy:{name}` — a policy this host really offers.
        .fixture(Fixture::new("ledger-policy", Verb::Source).binding("name", "leverage"))
        .fixture(Fixture::new("ledger-policy", Verb::Exists).binding("name", "leverage"))
        // A dispatch note (ledger #3718) to #6's claim at epoch 1, and that holder's read.
        .fixture(
            Fixture::new("ledger-note", Verb::Sink)
                .capability(note_sender())
                .arg("item", "6")
                .arg("epoch", "1")
                .arg("content", "a note from the conformance walk"),
        )
        .fixture(
            Fixture::new("ledger-item-notes", Verb::Source)
                .capability(note_holder())
                .binding("id", "6"),
        )
        // The mutating actions, each pointed at an item that exists.
        .fixture(
            Fixture::new("ledger-comment", Verb::Sink)
                .arg("item", &item)
                .arg("content", "a comment from the conformance walk"),
        )
        .fixture(
            Fixture::new("ledger-decision", Verb::Sink)
                .arg("item", &item)
                .arg("content", "a decision from the conformance walk")
                .arg("decided-by", "urn:example:review-board")
                .arg("decided-at", "2023-11-14T21:00:00Z"),
        )
        .fixture(Fixture::new("ledger-close", Verb::Sink).arg("item", &item))
        .fixture(Fixture::new("ledger-reopen", Verb::Sink).arg("item", &item))
        .fixture(
            Fixture::new("ledger-claim", Verb::Sink)
                .arg("item", &item)
                .arg("content", "the conformance walk"),
        )
        .fixture(Fixture::new("ledger-claim", Verb::Delete).arg("item", &item))
        .fixture(
            Fixture::new("ledger-claim", Verb::Source)
                .arg("item", &item)
                .arg("epoch", "1"),
        )
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
        .fixture(
            Fixture::new("ledger-backfill-closed-at", Verb::Sink)
                .arg("content", "key:urn:test:backfill"),
        )
        .fixture(Fixture::new("ledger-sweep-replays", Verb::Sink))
        .fixture(Fixture::new("ledger-offers", Verb::Sink))
        // The claim queue (ledger #4455) on #6, which stays held: a join takes a place (a
        // renewal when fired again) and a leave gives it up (nothing to do when fired again).
        .fixture(Fixture::new("ledger-wait", Verb::Source).binding("id", "6"))
        .fixture(
            Fixture::new("ledger-wait", Verb::Sink)
                .binding("id", "6")
                .arg("content", "the conformance walk"),
        )
        .fixture(
            Fixture::new("ledger-wait", Verb::Delete)
                .binding("id", "6")
                .arg("from", "the conformance walk"),
        )
        // What the reads promise.
        .cacheable("ledger-items")
        .cacheable("ledger-count")
        .cacheable("ledger-counts")
        .cacheable("ledger-item")
        .cacheable("ledger-next")
        .cacheable("ledger-item-state")
        .cacheable("ledger-item-state-is")
        .cacheable("ledger-item-holder-is")
        .cacheable("ledger-item-closed")
        .cacheable("ledger-item-history")
        .cacheable("ledger-item-sessions")
        .cacheable("ledger-sessions")
        .cacheable("ledger-doctor")
        .cacheable("ledger-stalled")
        .pure("ledger-policy")
        .pure("ledger-lifecycle")
        // RECOMPUTE (conformance 0.9, ledger #1567 — decided 2026-10-15): the doctor's text
        // face records when the check ran, so a forced recomputation differs by design.
        // PIPELINE (ledger #3709): the lapse sweep is a Sink with no body. It records every
        // lapse in its ledger, so there is nothing for a pipe to carry; its one by-value input
        // is the face, `as`.
        .opt_out_check(
            "ledger-lapse",
            Check::Pipeline,
            "the sweep takes no body: it records every lapse in its ledger",
        )
        .opt_out_check(
            "ledger-sweep-replays",
            Check::Pipeline,
            "the receipt sweep takes no body",
        )
        // PIPELINE (ledger #4455): leaving a claim queue names its waiter in `from=`, as a
        // compare-and-set release does, and carries no body for a pipe to deliver.
        .opt_out_check(
            "ledger-wait",
            Check::Pipeline,
            "a leave names its waiter in `from=` and takes no body",
        )
        .opt_out_check(
            "ledger-offers",
            Check::Pipeline,
            "the offers sweep takes no body: it settles every queue in its ledger",
        )
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
    // Conformance 0.11 enables DATATYPES and FAILURES by default. Require every
    // check to run so a future default change cannot silently reduce coverage.
    assert!(
        report.checks.skipped().next().is_none(),
        "every check runs: {report}"
    );
    assert_eq!(space_iri("ledger").as_str(), ikigai_ledger::SPACE_ID);
}

/// ★ The positive half: a clean report over a walk that reached nothing would look
/// exactly like a clean report over a walk that reached everything. This pins the list.
#[test]
fn the_walk_reaches_every_resource_this_crate_binds() {
    let (kernel, a, b, c) = seeded();
    let report = suite(&a, &b, &c).run_blocking(&kernel);
    // `Suite::own("ledger-*")` walks only the thirty THIS crate binds, so the whole of
    // `walked` is the module; no filter stands between this list and what was checked.
    let mut walked: Vec<&str> = report.walked.iter().map(String::as_str).collect();
    walked.sort_unstable();
    assert_eq!(
        walked,
        vec![
            "ledger-append",
            "ledger-backfill-closed-at",
            "ledger-claim",
            "ledger-close",
            "ledger-comment",
            "ledger-count",
            "ledger-counts",
            "ledger-decision",
            "ledger-defer",
            "ledger-doctor",
            "ledger-history",
            "ledger-item",
            "ledger-item-about",
            "ledger-item-closed",
            "ledger-item-history",
            "ledger-item-holder-is",
            "ledger-item-notes",
            "ledger-item-sessions",
            "ledger-item-state",
            "ledger-item-state-is",
            "ledger-items",
            "ledger-label",
            "ledger-lapse",
            "ledger-ledgers",
            "ledger-lifecycle",
            "ledger-link",
            "ledger-next",
            "ledger-note",
            "ledger-offers",
            "ledger-policy",
            "ledger-purge",
            "ledger-reopen",
            "ledger-sessions",
            "ledger-stalled",
            "ledger-sweep-replays",
            "ledger-wait",
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
            "store-dump",
            "store-graph-ask",
            "store-graph-construct",
            "store-graph-describe",
            "store-graph-info",
            "store-graph-select",
            "store-graph-update",
            "store-graph-written",
            "store-graphs",
            "store-info",
            "store-load",
            "store-select",
            "store-update",
        ],
        "{report}"
    );
}
