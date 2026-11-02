//! **A state event records the lifecycle version it was checked against** (ledger #2864):
//! `ledger:lifecycleVersion`, read from the document, in the update that makes the move.
//!
//! A lifecycle changes under a ledger that outlives it, and `stateFrom` and `stateTo` alone cannot
//! say which document allowed a move once that document is gone. The walk below is the case that
//! needs it most, a rename in two steps: one version declares both names and a move between them,
//! items cross by ordinary transitions, and the next version drops the old name.

mod common;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use common::*;
use ikigai_core::{
    Endpoint, Error, Fallback, Invocation, Kernel, Representation, Request, Resolution, Result,
    Scope, Space, Verb,
};
use ikigai_ledger::{Lifecycle, Lifecycles, SpaceConfig};
use ikigai_store::DurableStore;

const LIVE: &str = "urn:iki:ledger:graph:default";
const NS: &str = "https://ikigai-rs.dev/ns/ledger#";
const PREFIXES: &str = r#"
    @prefix ledger: <https://ikigai-rs.dev/ns/ledger#> .
    @prefix owl: <http://www.w3.org/2002/07/owl#> .
    @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
"#;

/// A state of the lifecycle `rename`, and where an item may go from it.
fn state(name: &str, order: i64, moves: &[&str]) -> String {
    let flight = match name {
        "doing" => "ledger:inFlight true".to_string(),
        _ => format!("ledger:inFlight false ; ledger:drain \"{name}-drain\""),
    };
    let moves: String = moves
        .iter()
        .map(|to| format!(" ; ledger:movesTo <urn:iki:ledger:lifecycle:rename:{to}>"))
        .collect();
    format!(
        "<urn:iki:ledger:lifecycle:rename> ledger:hasState <urn:iki:ledger:lifecycle:rename:{name}> .
         <urn:iki:ledger:lifecycle:rename:{name}> rdfs:label \"{name}\" ; ledger:order {order} ;
             {flight}{moves} .\n"
    )
}

/// The lifecycle `rename`, whose header says `version` however a test wants it said.
fn document(version: &str, states: &[String]) -> String {
    format!(
        "{PREFIXES} <urn:iki:ledger:lifecycle:rename> a ledger:Lifecycle {version} .\n{}",
        states.concat()
    )
}

/// Version 1 knows only the old name.
fn version_1() -> String {
    document(
        "; owl:versionInfo \"1\"",
        &[
            state("filed", 0, &["queue"]),
            state("queue", 1, &["doing"]),
            state("doing", 3, &[]),
        ],
    )
}

/// Version 2 declares both names and the move between them, and nothing new enters the old one.
fn version_2() -> String {
    document(
        "; owl:versionInfo \"2\"",
        &[
            state("filed", 0, &["backlog"]),
            state("queue", 1, &["backlog"]),
            state("backlog", 2, &["doing"]),
            state("doing", 3, &[]),
        ],
    )
}

/// Version 3 drops the old name.
fn version_3() -> String {
    document(
        "; owl:versionInfo \"3\"",
        &[
            state("filed", 0, &["backlog"]),
            state("backlog", 2, &["doing"]),
            state("doing", 3, &[]),
        ],
    )
}

/// One lifecycle document over one store.
fn host(store: DurableStore, clock: &Arc<TickingClock>, turtle: &str) -> Kernel {
    let lifecycle = Lifecycle::parse(turtle).expect("the fixture parses");
    let space = Fallback::new(vec![
        Arc::new(ikigai_store::space(store)) as Arc<dyn Space>,
        Arc::new(ikigai_ledger::space_with(
            SpaceConfig::default().lifecycles(Lifecycles::new(vec![lifecycle])),
        )) as Arc<dyn Space>,
    ]);
    Kernel::with_meta_renderer(Arc::new(space), Arc::new(ikigai_vocab::TurtleRenderer))
        .with_clock(clock.clone())
}

fn transition(kernel: &Kernel, n: i64, from: &str, to: &str) -> std::result::Result<String, Error> {
    try_verb(
        kernel,
        Verb::Sink,
        &format!("urn:iki:ledger:item:{n}:state"),
        &[("from", from), ("to", to)],
    )
}

fn history(kernel: &Kernel, n: i64) -> String {
    source(kernel, &format!("urn:iki:ledger:item:{n}:history"), &[])
}

/// Each line of the plain history after its count, with its time taken off.
fn history_lines(kernel: &Kernel, n: i64) -> Vec<String> {
    history(kernel, n)
        .lines()
        .skip(1)
        .map(|line| line.split_once("  ").expect("a time, then the event").1)
        .map(str::to_string)
        .collect()
}

fn history_events(kernel: &Kernel, n: i64) -> Vec<serde_json::Value> {
    let doc: serde_json::Value = serde_json::from_str(&source(
        kernel,
        &format!("urn:iki:ledger:item:{n}:history"),
        &[("as", "application/json")],
    ))
    .expect("the history answers JSON");
    doc["events"].as_array().expect("events").clone()
}

/// What the store itself holds for the state events of item `#n`: `(from, to, version)` as state
/// names, in the order they happened.
fn recorded(kernel: &Kernel, n: i64) -> Vec<(String, String, String)> {
    let json: serde_json::Value = serde_json::from_str(&select(
        kernel,
        &format!(
            "SELECT ?from ?to ?version WHERE {{ GRAPH <{LIVE}> {{ \
             ?item <{NS}number> {n} . ?e <{NS}onItem> ?item ; <{NS}stateFrom> ?from ; \
             <{NS}stateTo> ?to ; <http://purl.org/dc/terms/created> ?at . \
             OPTIONAL {{ ?e <{NS}lifecycleVersion> ?version }} }} }} ORDER BY ?at"
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
        .map(|row| {
            (
                name(row, "from"),
                name(row, "to"),
                row["version"]["value"]
                    .as_str()
                    .unwrap_or("none")
                    .to_string(),
            )
        })
        .collect()
}

fn triple(from: &str, to: &str, version: &str) -> (String, String, String) {
    (from.to_string(), to.to_string(), version.to_string())
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

// ------------------------------------------------------------------- the event

/// The defect as the item states it: the built-in lifecycle's transitions, on the store itself.
#[test]
fn a_transition_under_the_built_in_lifecycle_records_its_version() {
    let kernel = kernel();
    append(&kernel, "Ship it", &[]);
    transition(&kernel, 1, "filed", "queued").unwrap();
    transition(&kernel, 1, "queued", "reviewed").unwrap();
    assert_eq!(
        recorded(&kernel, 1),
        [
            triple("filed", "queued", "1"),
            triple("queued", "reviewed", "1"),
        ]
    );
}

/// ★ The two-step rename, each step a host restarted over the same durable store with the next
/// document. Each event names the version that judged it, and the store and both faces agree.
#[test]
fn a_rename_in_two_steps_records_each_moves_version() {
    let path = std::env::temp_dir().join(format!(
        "ikigai-ledger-test-lifecycle-version-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    let clock = Arc::new(TickingClock::default());
    let restart = |turtle: &str| {
        host(
            DurableStore::open(&path).expect("open the store"),
            &clock,
            turtle,
        )
    };

    // Version 1: three items enter the old name.
    let v1 = restart(&version_1());
    for (n, title) in [(1, "Crosses"), (2, "Crosses too"), (3, "Left behind")] {
        append(&v1, title, &[]);
        transition(&v1, n, "filed", "queue").unwrap();
    }
    drop(v1);

    // Version 2: both names are legal, and items cross by ordinary transitions. One new item goes
    // straight to the new name, and nothing goes back to the old one.
    let v2 = restart(&version_2());
    transition(&v2, 1, "queue", "backlog").unwrap();
    transition(&v2, 2, "queue", "backlog").unwrap();
    append(&v2, "Born after the rename", &[]);
    transition(&v2, 4, "filed", "backlog").unwrap();
    assert!(matches!(
        transition(&v2, 1, "backlog", "queue"),
        Err(Error::Conflict(_))
    ));
    drop(v2);

    // Version 3: the old name is gone. An item that crossed moves on as usual.
    let v3 = restart(&version_3());
    transition(&v3, 1, "backlog", "doing").unwrap();
    assert!(matches!(
        transition(&v3, 3, "queue", "backlog"),
        Err(Error::InvalidArgument { .. })
    ));

    assert_eq!(
        recorded(&v3, 1),
        [
            triple("filed", "queue", "1"),
            triple("queue", "backlog", "2"),
            triple("backlog", "doing", "3"),
        ]
    );
    assert_eq!(
        recorded(&v3, 2),
        [
            triple("filed", "queue", "1"),
            triple("queue", "backlog", "2"),
        ]
    );
    assert_eq!(recorded(&v3, 3), [triple("filed", "queue", "1")]);
    assert_eq!(recorded(&v3, 4), [triple("filed", "backlog", "2")]);

    // The refused moves recorded nothing, and an event still names a state its document dropped.
    assert_eq!(
        history_lines(&v3, 1),
        [
            "state filed to queue  lifecycle version 1",
            "state queue to backlog  lifecycle version 2",
            "state backlog to doing  lifecycle version 3",
        ]
    );
    let versions: Vec<serde_json::Value> = history_events(&v3, 1)
        .iter()
        .map(|event| event["lifecycle_version"].clone())
        .collect();
    assert_eq!(versions, ["1", "2", "3"]);

    // Whoever did not cross before version 3 is what the doctor is for.
    let doctor = source(&v3, "urn:iki:ledger:doctor", &[]);
    assert!(
        doctor.contains(
            "unknown-state  #3 holds the state `queue`, which the lifecycle `rename` does not \
             declare"
        ),
        "{doctor}"
    );
    for crossed in ["#1 holds", "#2 holds", "#4 holds"] {
        assert!(!doctor.contains(crossed), "{doctor}");
    }

    drop(v3);
    let _ = std::fs::remove_dir_all(&path);
}

/// A request to stay writes no event, so it records no version either.
#[test]
fn staying_where_it_is_records_nothing() {
    let kernel = kernel();
    append(&kernel, "Ship it", &[]);
    assert_eq!(
        transition(&kernel, 1, "filed", "filed").unwrap(),
        "#1 filed (unchanged)\n"
    );
    assert_eq!(recorded(&kernel, 1), []);
}

/// Every update the ledger sends to its live graph, in order.
#[derive(Default)]
struct Updates(Mutex<Vec<String>>);

struct Recording {
    inner: Arc<dyn Space>,
    updates: Arc<Updates>,
}

struct RecordingEndpoint {
    inner: Arc<dyn Endpoint>,
    updates: Arc<Updates>,
}

impl Space for Recording {
    fn resolve(&self, request: &Request, scope: &Scope) -> Resolution {
        let updates = Arc::clone(&self.updates);
        self.inner
            .resolve(request, scope)
            .map_endpoint(move |inner| {
                Arc::new(RecordingEndpoint { inner, updates }) as Arc<dyn Endpoint>
            })
    }
}

#[async_trait]
impl Endpoint for RecordingEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        if inv.request.target.as_str() == "urn:iki:store:graph-update"
            && inv.inline_str("graph").map(|g| g == LIVE).unwrap_or(false)
        {
            let update = inv.inline_str("content")?.to_string();
            self.updates.0.lock().unwrap().push(update);
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

/// ★ The version rides the update that makes the move, so no crash or race can leave a move
/// without it: one update names the move and the version, and no other names the version.
#[test]
fn the_version_is_written_in_the_same_update_as_the_move() {
    let updates = Arc::new(Updates::default());
    let store = Arc::new(ikigai_store::space(DurableStore::in_memory().unwrap())) as Arc<dyn Space>;
    let space = Fallback::new(vec![
        Arc::new(Recording {
            inner: store,
            updates: Arc::clone(&updates),
        }) as Arc<dyn Space>,
        Arc::new(ikigai_ledger::space()) as Arc<dyn Space>,
    ]);
    let kernel =
        Kernel::with_meta_renderer(Arc::new(space), Arc::new(ikigai_vocab::TurtleRenderer))
            .with_clock(Arc::new(TickingClock::default()));
    append(&kernel, "Ship it", &[]);
    updates.0.lock().unwrap().clear();

    transition(&kernel, 1, "filed", "queued").unwrap();

    let updates = updates.0.lock().unwrap();
    let naming = |term: &str| {
        updates
            .iter()
            .enumerate()
            .filter(|(_, update)| update.contains(term))
            .map(|(at, _)| at)
            .collect::<Vec<_>>()
    };
    let moved = naming("stateTo");
    assert_eq!(moved.len(), 1, "{updates:?}");
    assert_eq!(naming("lifecycleVersion"), moved, "{updates:?}");
}

// ------------------------------------------------------------------- reading the document

/// A document with no `owl:versionInfo` says it with the declared term, and `owl:versionInfo` wins
/// when it has both, because the term is for a document without one.
#[test]
fn the_version_comes_from_owl_version_info_or_else_the_declared_term() {
    let states = [state("filed", 0, &[]), state("doing", 3, &[])];
    let declared = document("; ledger:lifecycleVersion \"2026-11\"", &states);
    let both = document(
        "; owl:versionInfo \"7\" ; ledger:lifecycleVersion \"2026-11\"",
        &states,
    );
    let number = document("; owl:versionInfo 8", &states);
    for (turtle, version) in [(declared, "2026-11"), (both, "7"), (number, "8")] {
        let store = DurableStore::in_memory().unwrap();
        let kernel = host(store, &Arc::new(TickingClock::default()), &turtle);
        append(&kernel, "Ship it", &[]);
        transition(&kernel, 1, "filed", "doing").unwrap();
        assert_eq!(recorded(&kernel, 1), [triple("filed", "doing", version)]);
    }
}

/// A version is the document's text and goes into an update, so it must arrive as text.
#[test]
fn a_version_with_quotes_and_backslashes_survives_the_update() {
    let states = [state("filed", 0, &[]), state("doing", 3, &[])];
    let turtle = document(r#"; owl:versionInfo "v\"2\\ } . é""#, &states);
    let store = DurableStore::in_memory().unwrap();
    let kernel = host(store, &Arc::new(TickingClock::default()), &turtle);
    append(&kernel, "Ship it", &[]);
    transition(&kernel, 1, "filed", "doing").unwrap();
    assert_eq!(
        history_lines(&kernel, 1),
        [r#"state filed to doing  lifecycle version v"2\ } . é"#]
    );
    assert_eq!(
        history_events(&kernel, 1)[0]["lifecycle_version"],
        r#"v"2\ } . é"#
    );
}

/// ★ Every transition records a version only if every lifecycle has one, so one with none does not
/// load: an event with none would then mean only that it is older than this change. Each refusal
/// is a sentence, byte for byte.
#[test]
fn a_lifecycle_without_one_clean_version_is_refused_at_load() {
    let states = [state("filed", 0, &[]), state("doing", 3, &[])];
    assert_eq!(
        parse_refusal(&document("", &states)),
        "`rename` declares no version, and every transition records the version it was checked \
         against: give `urn:iki:ledger:lifecycle:rename` an owl:versionInfo or, if the document \
         has none, a ledger:lifecycleVersion"
    );
    assert_eq!(
        parse_refusal(&document("; owl:versionInfo \"1\", \"2\"", &states)),
        "`rename` declares more than one version (1, 2); a document is one version of its \
         lifecycle"
    );
    assert_eq!(
        parse_refusal(&document(
            "; ledger:lifecycleVersion <urn:example:v1>",
            &states
        )),
        "the version of `rename` is <urn:example:v1>, which is not a literal; it is text that \
         every transition records"
    );
    for blank in [r#""""#, r#""   ""#, r#""1\n2""#] {
        assert!(
            parse_refusal(&document(&format!("; owl:versionInfo {blank}"), &states))
                .contains("must be one line with something on it"),
            "{blank}"
        );
    }
    // On a state it would vanish in silence, and the document would seem to have said its version.
    let on_a_state = document(
        "; owl:versionInfo \"1\"",
        &[
            state("filed", 0, &[]),
            state("doing", 3, &[]),
            "<urn:iki:ledger:lifecycle:rename:doing> ledger:lifecycleVersion \"1\" .".to_string(),
        ],
    );
    assert_eq!(
        parse_refusal(&on_a_state),
        "`urn:iki:ledger:lifecycle:rename:doing` declares a ledger:lifecycleVersion and is not \
         the lifecycle `rename`; put it on `urn:iki:ledger:lifecycle:rename`"
    );
}

// ------------------------------------------------------------------- an older event

/// ★ An event from before this change has none, and each face says so rather than leaving a reader
/// to guess; so does one from before #2841 that names no states at all. The event a store update
/// strips here is what those releases wrote.
#[test]
fn an_event_from_before_this_change_says_it_has_no_version() {
    let kernel = kernel();
    append(&kernel, "Older", &[]);
    append(&kernel, "Oldest", &[]);
    transition(&kernel, 1, "filed", "queued").unwrap();
    transition(&kernel, 2, "filed", "queued").unwrap();
    let strip = |predicates: &str| {
        sink(
            &kernel,
            "urn:iki:store:update",
            &[(
                "content",
                &format!(
                    "DELETE {{ GRAPH <{LIVE}> {{ ?e ?p ?o }} }} WHERE {{ GRAPH <{LIVE}> {{ \
                     ?e ?p ?o ; <{NS}onItem> ?item . ?item <{NS}number> ?n . \
                     FILTER({predicates}) }} }}"
                ),
            )],
        );
    };
    strip(&format!("?p = <{NS}lifecycleVersion>"));
    strip(&format!(
        "?n = 2 && (?p = <{NS}stateFrom> || ?p = <{NS}stateTo>)"
    ));
    transition(&kernel, 1, "queued", "reviewed").unwrap();

    assert_eq!(
        history_lines(&kernel, 1),
        [
            "state filed to queued  no lifecycle version recorded (written before 0.18.0)",
            "state queued to reviewed  lifecycle version 1",
        ]
    );
    assert_eq!(
        history_lines(&kernel, 2),
        [
            "state (no object recorded: nothing changed, or written before 0.18.0)  no lifecycle \
             version recorded (written before 0.18.0)"
        ]
    );
    let events = history_events(&kernel, 1);
    assert_eq!(events[0]["lifecycle_version"], serde_json::Value::Null);
    assert!(
        events[0]
            .as_object()
            .unwrap()
            .contains_key("lifecycle_version"),
        "on a state event, none is said as null: {}",
        events[0]
    );
    assert_eq!(events[1]["lifecycle_version"], "1");

    // A Rust consumer keeps the difference between none and not applicable.
    let read = |event: &serde_json::Value| {
        serde_json::from_value::<ikigai_ledger::json::HistoryEvent>(event.clone())
            .unwrap()
            .lifecycle_version
    };
    assert_eq!(read(&events[0]), Some(None));
    assert_eq!(read(&events[1]), Some(Some("1".to_string())));
    let mut other = events[0].clone();
    other.as_object_mut().unwrap().remove("lifecycle_version");
    assert_eq!(read(&other), None);
}

/// The field is about state events: nothing else is checked against a lifecycle, so nothing else
/// says anything about one.
#[test]
fn only_a_state_event_speaks_of_a_lifecycle_version() {
    let kernel = kernel();
    append(&kernel, "Ship it", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:label",
        &[("item", "1"), ("content", "bug")],
    );
    sink(
        &kernel,
        "urn:iki:ledger:close",
        &[("item", "1"), ("reason", "done")],
    );
    let plain = history(&kernel, 1);
    assert!(!plain.contains("lifecycle version"), "{plain}");
    for event in history_events(&kernel, 1) {
        assert!(event.get("lifecycle_version").is_none(), "{event}");
    }
}
