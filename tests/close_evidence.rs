//! **Evidence on a close** (ledger #2842): what closed an item, recorded on that close's
//! `ledger:StatusEvent` in the same update as the close.
//!
//! Until 0.18.0 a close took a reason and a note and nothing said WHAT closed it, so a
//! duplicate's target was written into a note and "fixes that did not hold" was no query. These
//! tests hold the item's acceptance:
//!
//! - evidence lands on the right event, and is repeatable;
//! - it stays on its close across a reopen and a second close;
//! - a duplicate close's target resolves, or the close is refused, and it is shown;
//! - the README's query lists closes whose evidence commit was followed by a reopen.

mod common;

use std::sync::Arc;

use common::*;
use ikigai_core::{
    ActionSpec, Capability, Error, Fallback, Kernel, ProposedArgs, Request, Resolution, Scope,
    Space, Verb,
};
use ikigai_ledger::SpaceConfig;
use ikigai_store::DurableStore;

const NS: &str = "https://ikigai-rs.dev/ns/ledger#";
const LIVE: &str = "urn:iki:ledger:graph:default";
const COMMIT: &str = "https://github.com/o/r/commit/abc123";
const PULL: &str = "https://github.com/o/r/pull/9";
const TEST: &str = "urn:test:o:r:a_test_that_proves_it";

const CLOSE: &str = "urn:iki:ledger:close";
const REOPEN: &str = "urn:iki:ledger:reopen";

/// A status event: its status, its reason and its evidence.
type Event = (String, String, Vec<String>);

/// Every status event on the item numbered `number`, in the order written: its status, its
/// reason and its evidence, each by its last IRI segment or as stored.
fn status_events(kernel: &Kernel, graph: &str, number: i64) -> Vec<Event> {
    let json: serde_json::Value = serde_json::from_str(&select(
        kernel,
        &format!(
            "SELECT ?event ?status ?reason ?evidence WHERE {{ GRAPH <{graph}> {{ \
             ?item <{NS}number> {number} ; a <{NS}Item> . \
             ?event a <{NS}StatusEvent> ; <{NS}onItem> ?item ; <{NS}status> ?status ; \
             <{NS}eventSequence> ?n . \
             OPTIONAL {{ ?event <{NS}closedReason> ?reason }} \
             OPTIONAL {{ ?event <{NS}evidence> ?evidence }} }} }} ORDER BY ?n ?evidence"
        ),
    ))
    .expect("the store answers JSON");
    let mut out: Vec<(String, Event)> = Vec::new();
    for row in json["results"]["bindings"].as_array().expect("bindings") {
        let value = |name: &str| row[name]["value"].as_str().unwrap_or("").to_string();
        let local = |iri: String| iri.rsplit('#').next().unwrap_or("").to_string();
        let event = value("event");
        if out.last().map(|(iri, _)| iri) != Some(&event) {
            out.push((
                event,
                (local(value("status")), local(value("reason")), Vec::new()),
            ));
        }
        let evidence = value("evidence");
        if !evidence.is_empty() {
            out.last_mut().unwrap().1 .2.push(evidence);
        }
    }
    out.into_iter().map(|(_, event)| event).collect()
}

fn events(kernel: &Kernel, number: i64) -> Vec<Event> {
    status_events(kernel, LIVE, number)
}

fn owned(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

/// The lines of an item's plain history that say what happened, without their times, and only
/// the closes and reopens.
fn history(kernel: &Kernel, id: &str) -> Vec<String> {
    source(kernel, &format!("urn:iki:ledger:item:{id}:history"), &[])
        .lines()
        .skip(1)
        .map(|line| line.split_once("  ").unwrap().1.to_string())
        .filter(|what| what.starts_with("closed") || what.starts_with("reopened"))
        .collect()
}

/// ★ The first line of the acceptance: evidence on the right event, repeatable, with the reason
/// beside it, because the item's own reason goes on a reopen and the role is read from the
/// reason plus the target's kind.
#[test]
fn evidence_lands_on_that_closes_status_event_and_the_receipt_counts_it() {
    let kernel = kernel();
    append(&kernel, "Fix the widget", &[]);
    let answer = sink(
        &kernel,
        CLOSE,
        &[
            ("item", "1"),
            ("reason", "done"),
            ("evidence", &format!("{COMMIT} {TEST}")),
        ],
    );
    assert_eq!(answer, "closed #1 done (2 evidence)\n");
    let mut want = owned(&[COMMIT, TEST]);
    want.sort();
    assert_eq!(
        events(&kernel, 1),
        [("closed".to_string(), "done".to_string(), want)]
    );
}

/// `evidence=A evidence=B` is one argument: the contract declares a list, so an adapter joins
/// the repeats instead of refusing them as `given twice`. Whitespace and never a comma, which an
/// IRI may hold.
#[test]
fn the_contract_lets_evidence_repeat() {
    let space = ikigai_ledger::space();
    let request = Request::new(Verb::Meta, ikigai_core::Iri::parse(CLOSE).unwrap());
    let Resolution::Hit(resolved) = space.resolve(&request, &Scope::empty()) else {
        panic!("close resolves");
    };
    let actions: Vec<ActionSpec> = resolved.endpoint.describe().action_specs();
    let contract = actions
        .iter()
        .find(|action| action.verb == Verb::Sink)
        .expect("close declares its sink");
    let joined = contract
        .join_repeats(&ProposedArgs::parse(
            "item=1&evidence=https://x/a,b&evidence=urn:y",
        ))
        .unwrap();
    assert_eq!(joined.get("evidence"), Some("https://x/a,b urn:y"));

    let kernel = kernel();
    append(&kernel, "Fix the widget", &[]);
    let answer = sink_joined(&kernel, joined.get("evidence").unwrap());
    assert_eq!(answer, "closed #1 done (2 evidence)\n");
    assert_eq!(
        events(&kernel, 1)[0].2,
        ["https://x/a,b", "urn:y"],
        "the comma stayed inside its IRI"
    );
}

fn sink_joined(kernel: &Kernel, evidence: &str) -> String {
    sink(kernel, CLOSE, &[("item", "1"), ("evidence", evidence)])
}

/// A value named twice is one fact, and is counted once.
#[test]
fn the_same_evidence_twice_counts_once() {
    let kernel = kernel();
    append(&kernel, "Fix the widget", &[]);
    assert_eq!(
        sink_joined(&kernel, &format!("{COMMIT} {COMMIT}")),
        "closed #1 done (1 evidence)\n"
    );
}

/// ★ A reopen keeps each close's evidence on that close's event, and a second close starts its
/// own: that is what makes "fixes that did not hold" a query.
#[test]
fn each_close_keeps_its_evidence_across_a_reopen_and_a_second_close() {
    let kernel = kernel();
    append(&kernel, "Fix the widget", &[]);
    sink_joined(&kernel, COMMIT);
    sink(&kernel, REOPEN, &[("item", "1")]);
    sink(
        &kernel,
        CLOSE,
        &[("item", "1"), ("reason", "wontfix"), ("evidence", PULL)],
    );
    assert_eq!(
        events(&kernel, 1),
        [
            ("closed".to_string(), "done".to_string(), owned(&[COMMIT])),
            ("open".to_string(), String::new(), Vec::new()),
            ("closed".to_string(), "wontfix".to_string(), owned(&[PULL])),
        ]
    );
    assert_eq!(
        history(&kernel, "1"),
        [
            format!("closed done, evidence {COMMIT}"),
            "reopened".to_string(),
            format!("closed wontfix, evidence {PULL}"),
        ]
    );
    let json: serde_json::Value = serde_json::from_str(&source(
        &kernel,
        "urn:iki:ledger:item:1:history",
        &[("as", "application/json")],
    ))
    .unwrap();
    let closes: Vec<&serde_json::Value> = json["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["kind"] == "closed")
        .collect();
    assert_eq!(closes[0]["reason"], "done");
    assert_eq!(closes[0]["evidence"], serde_json::json!([COMMIT]));
    assert_eq!(closes[1]["reason"], "wontfix");
    assert_eq!(closes[1]["evidence"], serde_json::json!([PULL]));
}

/// A close with no evidence reads as it always did, and its event still says why.
#[test]
fn a_close_with_no_evidence_answers_as_it_always_did() {
    let kernel = kernel();
    append(&kernel, "Fix the widget", &[]);
    assert_eq!(
        sink(&kernel, CLOSE, &[("item", "1"), ("reason", "wontfix")]),
        "closed #1 (wontfix)\n"
    );
    assert_eq!(
        events(&kernel, 1),
        [("closed".to_string(), "wontfix".to_string(), Vec::new())]
    );
    assert_eq!(history(&kernel, "1"), ["closed wontfix"]);
    // An empty argument is no evidence: an adapter may send one.
    append(&kernel, "Another", &[]);
    assert_eq!(
        sink(&kernel, CLOSE, &[("item", "2"), ("evidence", "  ")]),
        "closed #2 (done)\n"
    );
}

/// ★ A duplicate gains a real target: named however a link's object is, stored as the item's
/// IRI, and shown as the receipt and the history name it.
#[test]
fn a_duplicate_or_superseded_close_resolves_its_target_and_shows_it() {
    let kernel = kernel();
    let survivor = append_iri(&kernel, "The survivor", &[]);
    append(&kernel, "A duplicate", &[]);
    append(&kernel, "An older plan", &[]);
    let answer = sink(
        &kernel,
        CLOSE,
        &[("item", "2"), ("reason", "duplicate"), ("evidence", "#1")],
    );
    assert_eq!(answer, "closed #2 duplicate of #1\n");
    assert_eq!(events(&kernel, 2)[0].2, std::slice::from_ref(&survivor));
    let answer = sink(
        &kernel,
        CLOSE,
        &[
            ("item", "3"),
            ("reason", "superseded"),
            ("evidence", &format!("{survivor} {COMMIT}")),
        ],
    );
    assert_eq!(answer, "closed #3 superseded by #1 (1 more evidence)\n");

    assert_eq!(
        history(&kernel, "2"),
        [format!("closed duplicate of {survivor}")]
    );
    assert_eq!(
        history(&kernel, "3"),
        [format!(
            "closed superseded by {survivor}, evidence {COMMIT}"
        )]
    );

    // A duplicate closed on a commit alone has no target to name, and says what it has.
    append(&kernel, "Another duplicate", &[]);
    assert_eq!(
        sink(
            &kernel,
            CLOSE,
            &[("item", "4"), ("reason", "duplicate"), ("evidence", COMMIT)],
        ),
        "closed #4 duplicate (1 evidence)\n"
    );
    // And an item on a `done` close is evidence like any other, and still has to exist.
    append(&kernel, "A fix", &[]);
    assert_eq!(
        sink(&kernel, CLOSE, &[("item", "5"), ("evidence", "#1")]),
        "closed #5 done (1 evidence)\n"
    );
    assert_eq!(
        history(&kernel, "5"),
        [format!("closed done, evidence {survivor}")]
    );
}

/// The JSON receipt: the IRIs as stored, and the targets as items.
#[test]
fn the_json_receipt_carries_the_evidence_and_the_targets() {
    let kernel = kernel();
    let survivor = append_iri(&kernel, "The survivor", &[]);
    append(&kernel, "A duplicate", &[]);
    let json: serde_json::Value = serde_json::from_str(&sink(
        &kernel,
        CLOSE,
        &[
            ("item", "2"),
            ("reason", "duplicate"),
            ("evidence", &format!("1 {COMMIT}")),
            ("as", "application/json"),
        ],
    ))
    .unwrap();
    assert_eq!(json["outcome"], "closed");
    assert_eq!(json["reason"], "duplicate");
    assert_eq!(
        json["evidence"],
        serde_json::json!([survivor.clone(), COMMIT])
    );
    assert_eq!(json["targets"][0]["display"], "#1");
    assert_eq!(json["targets"][0]["iri"], survivor);
    assert_eq!(json["targets"].as_array().unwrap().len(), 1);
}

/// What does not resolve is refused, the refusal says it was the evidence, and nothing closes.
#[test]
fn evidence_that_does_not_resolve_is_refused_and_nothing_closes() {
    let kernel = kernel();
    append(&kernel, "The target", &[]);
    append(&kernel, "To close", &[]);
    let gone = append(&kernel, "Deleted", &[]);
    delete(&kernel, "urn:iki:ledger:item:3", &[]);
    assert_eq!(gone, "#3");
    let close = |evidence: &str| {
        try_verb(
            &kernel,
            Verb::Sink,
            CLOSE,
            &[
                ("item", "2"),
                ("reason", "duplicate"),
                ("evidence", evidence),
            ],
        )
    };
    for missing in ["99", "#3", "urn:iki:ledger:item:NOPE", "key:never-given"] {
        match close(missing) {
            Err(Error::NotFound(message)) => {
                assert!(message.contains("evidence"), "{message}");
                assert!(message.contains(missing), "{message}");
            }
            other => panic!("{missing}: expected NotFound, got {other:?}"),
        }
    }
    // Itself, and something that is neither an item nor an IRI.
    match close("#2") {
        Err(Error::InvalidArgument { name, detail }) => {
            assert_eq!(name, "evidence");
            assert!(detail.contains("its own close"), "{detail}");
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    match close("https://x/a>b") {
        Err(Error::InvalidArgument { name, .. }) => assert_eq!(name, "evidence"),
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert!(events(&kernel, 2).is_empty(), "nothing closed");
    assert!(source(&kernel, "urn:iki:ledger:item:2", &[]).contains("open"));
}

/// ★ A close of an item already closed writes no second event, and it must not lose what it was
/// given: the evidence goes on the close that stands, decided in the update, and the event's
/// reason follows the verdict so the role stays right.
#[test]
fn evidence_on_an_already_closed_item_goes_on_the_close_that_stands() {
    let kernel = kernel();
    let survivor = append_iri(&kernel, "The survivor", &[]);
    append(&kernel, "To close", &[]);
    sink(&kernel, CLOSE, &[("item", "2"), ("evidence", COMMIT)]);
    let answer = sink(
        &kernel,
        CLOSE,
        &[("item", "2"), ("reason", "duplicate"), ("evidence", "1")],
    );
    assert_eq!(
        answer,
        "closed #2 duplicate of #1; it was already closed, so the verdict changed and this \
         evidence joined that close\n"
    );
    let mut want = owned(&[COMMIT, &survivor]);
    want.sort();
    assert_eq!(
        events(&kernel, 2),
        [("closed".to_string(), "duplicate".to_string(), want)]
    );
    // A verdict edit with no evidence moves the event's reason too, and says what it always said.
    assert_eq!(
        sink(&kernel, CLOSE, &[("item", "2"), ("reason", "wontfix")]),
        "closed #2 (wontfix); it was already closed, so only the verdict changed\n"
    );
    assert_eq!(events(&kernel, 2)[0].1, "wontfix");
    assert_eq!(events(&kernel, 2)[0].2.len(), 2, "the evidence stayed");

    // After a reopen the earlier close is not the one that stands.
    sink(&kernel, REOPEN, &[("item", "2")]);
    sink(&kernel, CLOSE, &[("item", "2")]);
    sink(&kernel, CLOSE, &[("item", "2"), ("evidence", PULL)]);
    let all = events(&kernel, 2);
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].2.len(), 2, "the first close kept its own");
    assert_eq!(all[2].2, [PULL.to_string()]);
}

/// A close written before status events has nothing to hang evidence on. Refused with the
/// remedy, and nothing changes: losing it quietly is the one wrong answer.
#[test]
fn a_close_with_no_event_refuses_evidence_and_names_the_remedy() {
    let kernel = kernel();
    let item = append_iri(&kernel, "Closed long ago", &[]);
    sink(
        &kernel,
        "urn:iki:store:update",
        &[(
            "content",
            &format!(
                "WITH <{LIVE}> DELETE {{ <{item}> <{NS}status> ?s }} \
                 INSERT {{ <{item}> <{NS}status> <{NS}closed> ; \
                 <{NS}closedReason> <{NS}done> }} WHERE {{ <{item}> <{NS}status> ?s }}"
            ),
        )],
    );
    match try_verb(
        &kernel,
        Verb::Sink,
        CLOSE,
        &[("item", "1"), ("reason", "wontfix"), ("evidence", COMMIT)],
    ) {
        Err(Error::InvalidArgument { name, detail }) => {
            assert_eq!(name, "evidence");
            assert!(detail.contains("reopen"), "{detail}");
            assert!(detail.contains("Nothing was changed"), "{detail}");
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert!(source(&kernel, "urn:iki:ledger:item:1", &[]).contains("(done)"));
    // Without evidence it is the verdict edit it always was.
    assert_eq!(
        sink(&kernel, CLOSE, &[("item", "1"), ("reason", "wontfix")]),
        "closed #1 (wontfix); it was already closed, so only the verdict changed\n"
    );
    // And the remedy works.
    sink(&kernel, REOPEN, &[("item", "1")]);
    assert_eq!(
        sink_joined(&kernel, COMMIT),
        "closed #1 done (1 evidence)\n"
    );
}

/// ★ Evidence is history, not an edge: deleting a duplicate's target takes neither the close nor
/// the IRI it names, and an item's own events go with it.
#[test]
fn deleting_the_target_leaves_the_evidence_and_deleting_the_item_takes_it() {
    let kernel = kernel();
    let survivor = append_iri(&kernel, "The survivor", &[]);
    append(&kernel, "A duplicate", &[]);
    sink(
        &kernel,
        CLOSE,
        &[("item", "2"), ("reason", "duplicate"), ("evidence", "1")],
    );
    delete(&kernel, "urn:iki:ledger:item:1", &[]);
    assert_eq!(events(&kernel, 2)[0].2, std::slice::from_ref(&survivor));
    assert_eq!(
        history(&kernel, "2"),
        [format!("closed duplicate of {survivor}")]
    );
    delete(&kernel, "urn:iki:ledger:item:2", &[]);
    assert!(events(&kernel, 2).is_empty());
    let json: serde_json::Value = serde_json::from_str(&select(
        &kernel,
        &format!("SELECT ?s WHERE {{ GRAPH <{LIVE}> {{ ?s <{NS}evidence> ?o }} }}"),
    ))
    .unwrap();
    assert!(
        json["results"]["bindings"].as_array().unwrap().is_empty(),
        "{json}"
    );
}

/// An import principal sends `closed-at=` and `evidence=` together: the dated event holds both.
#[test]
fn an_importer_sends_closed_at_and_evidence_together() {
    const PARTICIPANTS: &str = "urn:collab:participants";
    const IMPORTER: &str = "urn:iki:gonk:client:importer";
    let space = Fallback::new(vec![
        Arc::new(ikigai_store::space(DurableStore::in_memory().unwrap())) as Arc<dyn Space>,
        Arc::new(ikigai_ledger::space_with(
            SpaceConfig::default().participants(PARTICIPANTS),
        )) as Arc<dyn Space>,
    ]);
    let kernel = Kernel::new(Arc::new(space)).with_clock(Arc::new(TickingClock::default()));
    sink(
        &kernel,
        "urn:iki:store:update",
        &[(
            "content",
            &format!(
                "INSERT DATA {{ GRAPH <{PARTICIPANTS}> {{ <{IMPORTER}> a <{NS}Importer> . }} }}"
            ),
        )],
    );
    let who = Capability::scoped(vec![
        "urn:cap:ledger:read:default".to_string(),
        "urn:cap:ledger:write:default".to_string(),
        graph_read("default"),
        graph_write("default"),
        format!("urn:cap:store:read:graph:{PARTICIPANTS}"),
    ])
    .with_principal(IMPORTER)
    .unwrap();
    try_as(
        &kernel,
        &who,
        Verb::Sink,
        "urn:iki:ledger:append",
        &[("content", "Imported"), ("created", "2026-01-02T03:04:05Z")],
    )
    .unwrap();
    let answer = try_as(
        &kernel,
        &who,
        Verb::Sink,
        CLOSE,
        &[
            ("item", "1"),
            ("closed-at", "2026-01-05T10:00:00Z"),
            ("evidence", COMMIT),
        ],
    )
    .unwrap();
    assert_eq!(answer, "closed #1 done (1 evidence)\n");
    let plain = source(&kernel, "urn:iki:ledger:item:1:history", &[]);
    assert!(
        plain.contains(&format!(
            "2026-01-05T10:00:00.000Z (claimed)  closed done, evidence {COMMIT}"
        )),
        "{plain}"
    );
}

fn cross_kernel() -> (Kernel, String) {
    let kernel = kernel();
    sink(
        &kernel,
        "urn:iki:ledger:cli:append",
        &[("content", "A duplicate")],
    );
    let answer = sink(
        &kernel,
        "urn:iki:ledger:acme:append",
        &[("content", "The survivor, over there")],
    );
    let target = answer.split_whitespace().nth(1).unwrap().to_string();
    (kernel, target)
}

fn cli_closer(extra: &[String]) -> Capability {
    let mut scopes = vec![
        "urn:cap:ledger:read:cli".to_string(),
        "urn:cap:ledger:close:cli".to_string(),
        graph_read("cli"),
        graph_write("cli"),
    ];
    scopes.extend_from_slice(extra);
    Capability::scoped(scopes)
}

/// ★ A target in another ledger is resolved under #2829's rules: the closer needs `link-in` on
/// it, like a link's author, and needs to be able to read it. There is no mirror, because
/// nothing over there reads a close here, so nothing is written there.
#[test]
fn a_target_in_another_ledger_takes_link_in_and_writes_nothing_there() {
    let (kernel, target) = cross_kernel();
    let before = source(&kernel, "urn:iki:ledger:acme:item:1", &[]);
    let closer = cli_closer(&[
        "urn:cap:ledger:link-in:acme".to_string(),
        graph_read("acme"),
    ]);
    for reference in ["acme#1", target.as_str()] {
        let answer = try_as(
            &kernel,
            &closer,
            Verb::Sink,
            "urn:iki:ledger:cli:close",
            &[
                ("item", "1"),
                ("reason", "duplicate"),
                ("evidence", reference),
            ],
        )
        .unwrap();
        assert!(
            answer.starts_with("closed cli#1 duplicate of acme#1"),
            "{answer}"
        );
    }
    assert_eq!(
        status_events(&kernel, "urn:iki:ledger:graph:cli", 1)[0].2,
        std::slice::from_ref(&target)
    );
    assert_eq!(
        source(&kernel, "urn:iki:ledger:acme:item:1", &[]),
        before,
        "the target's ledger learned nothing"
    );
}

/// ★ Without `link-in` the refusal comes before the lookup, so it says the same whether the item
/// exists or not, and `write` on the target does not stand in. Nothing closes.
#[test]
fn a_target_in_another_ledger_without_link_in_is_refused_before_the_lookup() {
    let (kernel, target) = cross_kernel();
    let closer = cli_closer(&[
        "urn:cap:ledger:read:acme".to_string(),
        "urn:cap:ledger:write:acme".to_string(),
        graph_read("acme"),
    ]);
    let close = |evidence: &str| {
        try_as(
            &kernel,
            &closer,
            Verb::Sink,
            "urn:iki:ledger:cli:close",
            &[
                ("item", "1"),
                ("reason", "duplicate"),
                ("evidence", evidence),
            ],
        )
    };
    let (Err(Error::Denied(there)), Err(Error::Denied(absent))) =
        (close(&target), close("urn:iki:ledger:acme:item:NOPE"))
    else {
        panic!("expected two denials");
    };
    assert!(there.contains("urn:cap:ledger:link-in:acme"), "{there}");
    assert_eq!(there, absent, "the refusal says nothing about the item");
    assert!(status_events(&kernel, "urn:iki:ledger:graph:cli", 1).is_empty());
}

/// ★ A non-item IRI is stored as given and never fetched: no grant is asked for, and nothing is
/// resolved through the aliases graph, whose read this closer does not hold.
#[test]
fn a_non_item_iri_is_stored_as_given_with_no_grant_beyond_the_close() {
    let (kernel, _) = cross_kernel();
    let answer = try_as(
        &kernel,
        &cli_closer(&[]),
        Verb::Sink,
        "urn:iki:ledger:cli:close",
        &[("item", "1"), ("evidence", COMMIT)],
    )
    .unwrap();
    assert_eq!(answer, "closed cli#1 done (1 evidence)\n");
    assert_eq!(
        status_events(&kernel, "urn:iki:ledger:graph:cli", 1)[0].2,
        [COMMIT.to_string()]
    );
}

/// ★ The README's query is the acceptance's last line, read from the README so the two cannot
/// drift: closes whose evidence commit was followed by a reopen, and nothing else.
#[test]
fn the_readmes_query_lists_the_fixes_that_did_not_hold() {
    let readme = include_str!("../README.md");
    let block = readme
        .split("```sparql")
        .find(|block| block.trim_start().starts_with("# Fixes that did not hold"))
        .expect("the README has the query");
    let query = block.split("```").next().unwrap();

    let kernel = kernel();
    // Did not hold: closed on a commit, then reopened, and then closed again on another.
    let failed = append_iri(&kernel, "A fix that did not hold", &[]);
    sink_joined(&kernel, COMMIT);
    sink(&kernel, REOPEN, &[("item", "1")]);
    sink_joined(&kernel, "https://github.com/o/r/commit/def456");
    // Held: closed on a commit, never reopened.
    append(&kernel, "A fix that held", &[]);
    sink(
        &kernel,
        CLOSE,
        &[
            ("item", "2"),
            ("evidence", "https://github.com/o/r/commit/999"),
        ],
    );
    // Not a fix: a wontfix with a commit on it, and a duplicate of an item, each reopened.
    append(&kernel, "Declined", &[]);
    sink(
        &kernel,
        CLOSE,
        &[
            ("item", "3"),
            ("reason", "wontfix"),
            ("evidence", "https://github.com/o/r/commit/888"),
        ],
    );
    sink(&kernel, REOPEN, &[("item", "3")]);
    append(&kernel, "Duplicate", &[]);
    sink(
        &kernel,
        CLOSE,
        &[("item", "4"), ("reason", "duplicate"), ("evidence", "1")],
    );
    sink(&kernel, REOPEN, &[("item", "4")]);

    let json: serde_json::Value = serde_json::from_str(&select(&kernel, query)).unwrap();
    let rows = json["results"]["bindings"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{json}");
    assert_eq!(rows[0]["item"]["value"], failed);
    assert_eq!(rows[0]["commit"]["value"], COMMIT);
}
