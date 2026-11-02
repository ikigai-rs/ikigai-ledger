//! **A lifecycle declares each state's legal moves** (ledger #2863): `ledger:movesTo`, repeatable
//! on a state. When a lifecycle declares any, the state Sink refuses a `(from, to)` pair it does
//! not declare, before anything is read or written; a lifecycle that declares none behaves as it
//! always did.
//!
//! The pair is checked once, against the arguments. What makes that check hold for the write is
//! the compare-and-set, which pins the item to `from`: the forced race below lets another
//! transition commit between the check and the update, where a move the lifecycle does not declare
//! would happen if anything but `from` could be left.

mod common;

use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use common::*;
use ikigai_core::{
    ArgRef, Capability, Endpoint, Error, Fallback, Invocation, Iri, Kernel, Representation,
    Request, Resolution, Result, Scope, Space, Verb,
};
use ikigai_ledger::{Lifecycle, Lifecycles, SpaceConfig};
use ikigai_store::DurableStore;

const LIVE: &str = "urn:iki:ledger:graph:default";
const NS: &str = "https://ikigai-rs.dev/ns/ledger#";

/// `filed` to `queued`, `queued` on to three places, `reviewed` to `doing`, `doing` to `done`, and
/// `done` to nowhere. `doing` to `reviewed` is the move the race would make.
const GATED: &str = r#"
    @prefix ledger: <https://ikigai-rs.dev/ns/ledger#> .
    @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
    <urn:iki:ledger:lifecycle:gated> a ledger:Lifecycle ;
        ledger:lifecycleVersion "1" ;
        ledger:hasState <urn:iki:ledger:lifecycle:gated:filed>,
                        <urn:iki:ledger:lifecycle:gated:queued>,
                        <urn:iki:ledger:lifecycle:gated:reviewed>,
                        <urn:iki:ledger:lifecycle:gated:doing>,
                        <urn:iki:ledger:lifecycle:gated:done> .
    <urn:iki:ledger:lifecycle:gated:filed> rdfs:label "filed" ; ledger:order 0 ;
        ledger:inFlight false ; ledger:drain "triage" ;
        ledger:movesTo <urn:iki:ledger:lifecycle:gated:queued> .
    <urn:iki:ledger:lifecycle:gated:queued> rdfs:label "queued" ; ledger:order 1 ;
        ledger:inFlight false ; ledger:drain "gate" ;
        ledger:movesTo <urn:iki:ledger:lifecycle:gated:doing>,
                       <urn:iki:ledger:lifecycle:gated:filed>,
                       <urn:iki:ledger:lifecycle:gated:reviewed> .
    <urn:iki:ledger:lifecycle:gated:reviewed> rdfs:label "reviewed" ; ledger:order 2 ;
        ledger:inFlight false ; ledger:drain "ship" ;
        ledger:movesTo <urn:iki:ledger:lifecycle:gated:doing> .
    <urn:iki:ledger:lifecycle:gated:doing> rdfs:label "doing" ; ledger:order 3 ;
        ledger:inFlight true ;
        ledger:movesTo <urn:iki:ledger:lifecycle:gated:done> .
    <urn:iki:ledger:lifecycle:gated:done> rdfs:label "done" ; ledger:order 4 ;
        ledger:inFlight false ; ledger:drain "archive" .
"#;

/// The same states and no `ledger:movesTo` at all.
const NO_MOVES: &str = r#"
    @prefix ledger: <https://ikigai-rs.dev/ns/ledger#> .
    @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
    <urn:iki:ledger:lifecycle:gated> a ledger:Lifecycle ;
        ledger:lifecycleVersion "1" ;
        ledger:hasState <urn:iki:ledger:lifecycle:gated:filed>,
                        <urn:iki:ledger:lifecycle:gated:queued>,
                        <urn:iki:ledger:lifecycle:gated:reviewed>,
                        <urn:iki:ledger:lifecycle:gated:doing>,
                        <urn:iki:ledger:lifecycle:gated:done> .
    <urn:iki:ledger:lifecycle:gated:filed> rdfs:label "filed" ; ledger:order 0 ;
        ledger:inFlight false ; ledger:drain "triage" .
    <urn:iki:ledger:lifecycle:gated:queued> rdfs:label "queued" ; ledger:order 1 ;
        ledger:inFlight false ; ledger:drain "gate" .
    <urn:iki:ledger:lifecycle:gated:reviewed> rdfs:label "reviewed" ; ledger:order 2 ;
        ledger:inFlight false ; ledger:drain "ship" .
    <urn:iki:ledger:lifecycle:gated:doing> rdfs:label "doing" ; ledger:order 3 ;
        ledger:inFlight true .
    <urn:iki:ledger:lifecycle:gated:done> rdfs:label "done" ; ledger:order 4 ;
        ledger:inFlight false ; ledger:drain "archive" .
"#;

fn gated() -> Lifecycles {
    Lifecycles::new(vec![Lifecycle::parse(GATED).expect("the fixture parses")])
}

fn kernel_with(lifecycles: Lifecycles) -> Kernel {
    let space = Fallback::new(vec![
        Arc::new(ikigai_store::space(DurableStore::in_memory().unwrap())) as Arc<dyn Space>,
        Arc::new(ikigai_ledger::space_with(
            SpaceConfig::default().lifecycles(lifecycles),
        )) as Arc<dyn Space>,
    ]);
    Kernel::with_meta_renderer(Arc::new(space), Arc::new(ikigai_vocab::TurtleRenderer))
        .with_clock(Arc::new(TickingClock::default()))
}

fn transition(kernel: &Kernel, n: i64, from: &str, to: &str) -> std::result::Result<String, Error> {
    try_verb(
        kernel,
        Verb::Sink,
        &format!("urn:iki:ledger:item:{n}:state"),
        &[("from", from), ("to", to)],
    )
}

fn state_of(kernel: &Kernel, n: i64) -> String {
    source(kernel, &format!("urn:iki:ledger:item:{n}:state"), &[])
        .trim()
        .to_string()
}

fn conflict(refused: std::result::Result<String, Error>) -> String {
    match refused {
        Err(Error::Conflict(message)) => message,
        other => panic!("not a Conflict: {other:?}"),
    }
}

fn parse_refusal(turtle: &str) -> String {
    match Lifecycle::parse(turtle) {
        Err(Error::InvalidArgument { name, detail }) => {
            assert_eq!(name, "lifecycle");
            detail
        }
        other => panic!("not refused at load: {other:?}"),
    }
}

// ------------------------------------------------------------------- the refusal

/// ★ The sentence, byte for byte, for a state with several legal moves, with one and with none:
/// it names the lifecycle, the pair and every legal move from `from`, and says nothing was written.
#[test]
fn an_undeclared_move_is_a_conflict_naming_the_lifecycle_and_every_legal_move() {
    let kernel = kernel_with(gated());
    append(&kernel, "Ship it", &[]);
    let see = "(see `urn:iki:ledger:lifecycle:gated`)";
    assert_eq!(
        conflict(transition(&kernel, 1, "filed", "done")),
        format!(
            "The lifecycle `gated` declares no move from `filed` to `done`, so nothing was \
             written. From `filed` it allows moves to `queued` {see}"
        )
    );
    assert_eq!(
        conflict(transition(&kernel, 1, "queued", "done")),
        format!(
            "The lifecycle `gated` declares no move from `queued` to `done`, so nothing was \
             written. From `queued` it allows moves to `filed`, `reviewed`, `doing` {see}"
        )
    );
    assert_eq!(
        conflict(transition(&kernel, 1, "done", "filed")),
        format!(
            "The lifecycle `gated` declares no move from `done` to `filed`, so nothing was \
             written. It allows no move out of `done` {see}"
        )
    );
    assert_eq!(state_of(&kernel, 1), "filed");
}

/// Every declared move goes through, `filed` as a target included, and the answers are the ones a
/// lifecycle with no moves gives.
#[test]
fn a_declared_move_goes_through() {
    let kernel = kernel_with(gated());
    append(&kernel, "Ship it", &[]);
    assert_eq!(
        transition(&kernel, 1, "filed", "queued").unwrap(),
        "#1 queued (was filed)\n"
    );
    transition(&kernel, 1, "queued", "filed").unwrap();
    assert_eq!(state_of(&kernel, 1), "filed");
    for (from, to) in [
        ("filed", "queued"),
        ("queued", "reviewed"),
        ("reviewed", "doing"),
        ("doing", "done"),
    ] {
        transition(&kernel, 1, from, to).unwrap_or_else(|e| panic!("{from} to {to}: {e}"));
        assert_eq!(state_of(&kernel, 1), to);
    }
}

/// ★ Before anything is read or written, as an undeclared state is: the same Conflict over an item
/// that does not exist, and on a kernel with no store and no clock, where anything that reached
/// either would fail differently. The control is a declared move on that same kernel.
#[test]
fn the_pair_is_refused_before_anything_is_read_or_written() {
    let kernel = kernel_with(gated());
    append(&kernel, "Ship it", &[]);
    assert!(conflict(transition(&kernel, 99, "filed", "done")).contains("declares no move"));
    assert!(matches!(
        transition(&kernel, 99, "filed", "queued"),
        Err(Error::NotFound(_))
    ));

    let bare = Kernel::with_meta_renderer(
        Arc::new(ikigai_ledger::space_with(
            SpaceConfig::default().lifecycles(gated()),
        )),
        Arc::new(ikigai_vocab::TurtleRenderer),
    );
    assert!(conflict(transition(&bare, 1, "filed", "done")).contains("declares no move"));
    let control = transition(&bare, 1, "filed", "queued").expect_err("no store and no clock");
    assert!(
        !matches!(control, Error::Conflict(_)),
        "the declared move reached what the refused one did not: {control:?}"
    );

    // No write: a cached read of the ledger survives the refusal.
    let items = Request::new(Verb::Source, Iri::parse("urn:iki:ledger:items").unwrap());
    source(&kernel, "urn:iki:ledger:items", &[]);
    assert!(kernel.is_cached(&items, &Capability::root()));
    conflict(transition(&kernel, 1, "filed", "done"));
    assert!(
        kernel.is_cached(&items, &Capability::root()),
        "the refusal reached the store"
    );
}

/// The pair is judged whatever state the item is in, and a declared pair still has to find the
/// item in `from`: the two refusals say different things.
#[test]
fn a_declared_pair_from_the_wrong_state_is_the_compare_and_sets_conflict() {
    let kernel = kernel_with(gated());
    append(&kernel, "Ship it", &[]);
    assert!(
        conflict(transition(&kernel, 1, "queued", "doing")).contains("is in `filed`, not `queued`")
    );
    assert!(conflict(transition(&kernel, 1, "queued", "done")).contains("declares no move"));
}

/// Asking to stay is not a move, so it needs no declaration: it is still answered `unchanged`
/// without a write, and it still has to find the item there.
#[test]
fn staying_where_it_is_needs_no_declaration() {
    let kernel = kernel_with(gated());
    append(&kernel, "Ship it", &[]);
    assert_eq!(
        transition(&kernel, 1, "filed", "filed").unwrap(),
        "#1 filed (unchanged)\n"
    );
    assert!(conflict(transition(&kernel, 1, "done", "done")).contains("is in `filed`, not `done`"));
}

/// A close is a different fact: a declared move into flight still refuses closed work, and one
/// out of flight still works on it.
#[test]
fn a_closed_item_keeps_the_rules_it_had() {
    let kernel = kernel_with(gated());
    append(&kernel, "Ship it", &[]);
    transition(&kernel, 1, "filed", "queued").unwrap();
    transition(&kernel, 1, "queued", "doing").unwrap();
    sink(&kernel, "urn:iki:ledger:close", &[("item", "1")]);
    transition(&kernel, 1, "doing", "done").unwrap();
    assert_eq!(state_of(&kernel, 1), "done");

    append(&kernel, "Closed early", &[]);
    transition(&kernel, 2, "filed", "queued").unwrap();
    sink(&kernel, "urn:iki:ledger:close", &[("item", "2")]);
    assert!(conflict(transition(&kernel, 2, "queued", "doing")).contains("is closed"));
}

// ---------------------------------------------------------------------- the race

/// A transition to run once, just before the next update to the live graph.
#[derive(Default)]
struct Armed(Mutex<Option<Request>>);

struct Interleave {
    inner: Arc<dyn Space>,
    kernel: Arc<OnceLock<Kernel>>,
    armed: Arc<Armed>,
}

struct BeforeLiveUpdate {
    inner: Arc<dyn Endpoint>,
    kernel: Arc<OnceLock<Kernel>>,
    armed: Arc<Armed>,
}

impl Space for Interleave {
    fn resolve(&self, request: &Request, scope: &Scope) -> Resolution {
        let (kernel, armed) = (Arc::clone(&self.kernel), Arc::clone(&self.armed));
        self.inner
            .resolve(request, scope)
            .map_endpoint(move |inner| {
                Arc::new(BeforeLiveUpdate {
                    inner,
                    kernel,
                    armed,
                }) as Arc<dyn Endpoint>
            })
    }
}

#[async_trait]
impl Endpoint for BeforeLiveUpdate {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let to_live = inv.request.target.as_str() == "urn:iki:store:graph-update"
            && inv.inline_str("graph").map(|g| g == LIVE).unwrap_or(false);
        if to_live {
            let first = self.armed.0.lock().unwrap().take();
            if let Some(first) = first {
                let kernel = self.kernel.get().expect("the kernel is set");
                kernel.issue(first, &Capability::root()).await?;
            }
        }
        self.inner.invoke(inv).await
    }

    fn name(&self) -> &str {
        self.inner.name()
    }

    fn describe(&self) -> ikigai_core::Description {
        self.inner.describe()
    }
}

/// The `(stateFrom, stateTo)` of every state event the store holds, as state names, sorted.
fn recorded_moves(kernel: &Kernel) -> Vec<(String, String)> {
    let json: serde_json::Value = serde_json::from_str(&select(
        kernel,
        &format!(
            "SELECT ?from ?to WHERE {{ GRAPH <{LIVE}> {{ ?e <{NS}stateFrom> ?from ; \
             <{NS}stateTo> ?to }} }} ORDER BY ?from ?to"
        ),
    ))
    .expect("the store answers JSON");
    let name = |row: &serde_json::Value, var: &str| {
        ikigai_ledger::lifecycle::state_name(row[var]["value"].as_str().unwrap())
    };
    json["results"]["bindings"]
        .as_array()
        .expect("bindings")
        .iter()
        .map(|row| (name(row, "from"), name(row, "to")))
        .collect()
}

/// ★ The forced race. `queued` to `reviewed` passes the pair check and the read before the update;
/// then, just before its update, `queued` to `doing` runs and commits. `doing` to `reviewed` is
/// a move the lifecycle does not declare, and it is what the first writer would make if its check
/// were all that stood behind the write. The compare-and-set pins the item to `queued`, so it
/// loses, writes nothing, and every move the store records is one the lifecycle declares.
#[test]
fn the_compare_and_set_holds_the_pair_check_for_the_write_that_follows() {
    let lifecycle = Lifecycle::parse(GATED).unwrap();
    let cell = Arc::new(OnceLock::new());
    let armed = Arc::new(Armed::default());
    let store = Arc::new(ikigai_store::space(DurableStore::in_memory().unwrap())) as Arc<dyn Space>;
    let space = Fallback::new(vec![
        Arc::new(Interleave {
            inner: store,
            kernel: Arc::clone(&cell),
            armed: Arc::clone(&armed),
        }) as Arc<dyn Space>,
        Arc::new(ikigai_ledger::space_with(
            SpaceConfig::default().lifecycles(Lifecycles::new(vec![lifecycle])),
        )) as Arc<dyn Space>,
    ]);
    let kernel =
        Kernel::with_meta_renderer(Arc::new(space), Arc::new(ikigai_vocab::TurtleRenderer))
            .with_clock(Arc::new(TickingClock::default()));
    assert!(cell.set(kernel).is_ok());
    let kernel = cell.get().unwrap();

    append(kernel, "Ship it", &[]);
    transition(kernel, 1, "filed", "queued").unwrap();
    *armed.0.lock().unwrap() = Some(
        Request::new(
            Verb::Sink,
            Iri::parse("urn:iki:ledger:item:1:state").unwrap(),
        )
        .with_arg("from", ArgRef::Inline(b"queued".to_vec()))
        .with_arg("to", ArgRef::Inline(b"doing".to_vec())),
    );

    let lost = conflict(transition(kernel, 1, "queued", "reviewed"));
    assert!(
        armed.0.lock().unwrap().is_none(),
        "the racer never ran, so nothing was forced"
    );
    assert!(lost.contains("is in `doing`, not `queued`"), "{lost}");
    assert_eq!(state_of(kernel, 1), "doing");
    assert_eq!(
        recorded_moves(kernel),
        [
            ("filed".to_string(), "queued".to_string()),
            ("queued".to_string(), "doing".to_string()),
        ],
        "the loser recorded no move"
    );
    // The check is what stood behind it: asked for directly, the same move is refused.
    assert!(conflict(transition(kernel, 1, "doing", "reviewed")).contains("declares no move"));
}

// -------------------------------------------------------- a lifecycle that declares none

/// ★ A lifecycle that declares no moves behaves exactly as it did: every ordered pair of its
/// states goes through.
#[test]
fn a_lifecycle_that_declares_no_moves_refuses_no_pair() {
    let lifecycle = Lifecycle::parse(NO_MOVES).expect("the fixture without moves parses");
    let names: Vec<String> = lifecycle.states().iter().map(|s| s.name.clone()).collect();
    assert_eq!(names, ["filed", "queued", "reviewed", "doing", "done"]);
    for from in &names {
        for to in &names {
            if from == to {
                continue;
            }
            let kernel = kernel_with(Lifecycles::new(vec![lifecycle.clone()]));
            append(&kernel, "Ship it", &[]);
            if from != "filed" {
                transition(&kernel, 1, "filed", from).unwrap();
            }
            assert_eq!(
                transition(&kernel, 1, from, to).unwrap(),
                format!("#1 {to} (was {from})\n")
            );
        }
    }
}

/// ★ kata-flight declares no moves, on purpose: gonk's kata importer moves an item from `filed`
/// straight to whichever state its label names, and this crate's own suites and the doctor's
/// remedies jump as well. Every pair therefore stays legal, and the built-in's document says so.
#[test]
fn kata_flight_declares_no_moves_and_every_pair_stays_legal() {
    assert!(!ikigai_ledger::lifecycle::KATA_FLIGHT_TTL.contains("ledger:movesTo "));
    let names: Vec<String> = Lifecycle::kata_flight()
        .states()
        .iter()
        .map(|s| s.name.clone())
        .collect();
    for from in &names {
        for to in &names {
            if from == to {
                continue;
            }
            let kernel = kernel();
            append(&kernel, "Ship it", &[]);
            if from != "filed" {
                transition(&kernel, 1, "filed", from).unwrap();
            }
            transition(&kernel, 1, from, to).unwrap_or_else(|e| panic!("{from} to {to}: {e}"));
        }
    }
}

/// One ledger's lifecycle declares moves; the next ledger's lifecycle keeps none.
#[test]
fn the_moves_are_the_lifecycle_of_that_ledger() {
    let kernel = kernel_with(
        Lifecycles::new(vec![
            Lifecycle::kata_flight(),
            Lifecycle::parse(GATED).unwrap(),
        ])
        .assign("acme", "gated"),
    );
    sink(&kernel, "urn:iki:ledger:acme:append", &[("content", "a")]);
    append(&kernel, "b", &[]);
    assert!(conflict(try_verb(
        &kernel,
        Verb::Sink,
        "urn:iki:ledger:acme:item:1:state",
        &[("from", "filed"), ("to", "doing")],
    ))
    .contains("`gated`"));
    transition(&kernel, 1, "filed", "shipping").unwrap();
}

// -------------------------------------------------------------------------- the faces

/// One line per state still, each ending with where it may go, in words; `nowhere` for a state
/// with no move out. The Turtle face is the document, so it carries the term.
#[test]
fn the_lifecycle_says_its_moves_on_each_face() {
    let kernel = kernel_with(gated());
    assert_eq!(
        source(&kernel, "urn:iki:ledger:lifecycle:gated", &[]),
        "gated\n   \
         0. filed     drain: triage  (no ledger:state)  moves to: queued\n   \
         1. queued    drain: gate  moves to: filed, reviewed, doing\n   \
         2. reviewed  drain: ship  moves to: doing\n   \
         3. doing     in flight  moves to: done\n   \
         4. done      drain: archive  moves to: nowhere\n"
    );
    let turtle = source(
        &kernel,
        "urn:iki:ledger:lifecycle:gated",
        &[("as", "text/turtle")],
    );
    assert!(turtle.contains("movesTo"), "{turtle}");
}

/// A lifecycle with no moves reads as it did, byte for byte.
#[test]
fn a_lifecycle_with_no_moves_reads_as_it_did() {
    assert_eq!(
        Lifecycle::parse(NO_MOVES).unwrap().plain(),
        "gated\n   \
         0. filed     drain: triage  (no ledger:state)\n   \
         1. queued    drain: gate\n   \
         2. reviewed  drain: ship\n   \
         3. doing     in flight\n   \
         4. done      drain: archive\n"
    );
}

// ------------------------------------------------------------------------- the loader

/// ★ A move to an undeclared state is refused at load, with a sentence, whether the state is
/// simply missing or belongs to another lifecycle.
#[test]
fn a_move_to_a_state_the_lifecycle_does_not_declare_is_refused_at_load() {
    for target in [
        "urn:iki:ledger:lifecycle:gated:shipped",
        "urn:iki:ledger:lifecycle:kata-flight:queued",
    ] {
        let turtle = GATED.replace(
            "ledger:movesTo <urn:iki:ledger:lifecycle:gated:done> .",
            &format!("ledger:movesTo <{target}> ."),
        );
        assert_eq!(
            parse_refusal(&turtle),
            format!(
                "the state `doing` moves to `{target}`, which is not a state of `gated`; its \
                 states are filed, queued, reviewed, doing, done"
            )
        );
    }
}

/// The other shapes a move can be wrong in. Each would otherwise be a declaration that silently
/// did nothing, and one on a state not listed could leave the lifecycle enforcing no moves at all.
#[test]
fn every_other_wrong_move_is_refused_at_load() {
    assert_eq!(
        parse_refusal(&GATED.replace(
            "ledger:movesTo <urn:iki:ledger:lifecycle:gated:done> .",
            "ledger:movesTo \"done\" ."
        )),
        "the state `doing` moves to \"done\", which is not an IRI; a move names a state of `gated` \
         by its IRI"
    );
    assert_eq!(
        parse_refusal(&format!(
            "{GATED}\n<urn:iki:ledger:lifecycle:gated:ghost> \
             <https://ikigai-rs.dev/ns/ledger#movesTo> <urn:iki:ledger:lifecycle:gated:done> ."
        )),
        "`urn:iki:ledger:lifecycle:gated:ghost` declares a ledger:movesTo and is not a state of \
         `gated`; list it with ledger:hasState or remove the move"
    );
    assert_eq!(
        parse_refusal(&GATED.replace(
            "ledger:movesTo <urn:iki:ledger:lifecycle:gated:done> .",
            "ledger:movesTo <urn:iki:ledger:lifecycle:gated:doing> ."
        )),
        "the state `doing` moves to itself; a request to stay is answered `unchanged` with no \
         write, so it is not a move to declare"
    );
}
