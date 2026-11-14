//! **Session summaries in their own graph** (ledger #3661).
//!
//! A ledger's summaries are in `urn:iki:ledger:graph:{ledger}:session`, so the store grant that
//! reads them reads nothing else. These tests hold the rest of that move:
//!
//! - the migration: summaries an earlier version left in the ledger graph are listed by `plan`,
//!   which writes nothing, and moved by `move`, which a second run finds nothing left for;
//! - the worker grant: a worker records into the new graph, and a worker grant from before it is
//!   refused by name, with nothing written;
//! - delete and purge carry an item's summaries out of the new graph.

mod common;

use common::*;
use ikigai_core::{Capability, Error, Kernel, Verb};

const LEDGER: &str = "acme";
const LEDGER_GRAPH: &str = "urn:iki:ledger:graph:acme";
const SESSION_GRAPH: &str = "urn:iki:ledger:graph:acme:session";
const GRAVEYARD: &str = "urn:iki:ledger:graph:acme:deleted";

fn at(resource: &str) -> String {
    format!("urn:iki:ledger:{LEDGER}:{resource}")
}

/// The published worker grant since ledger #3661.
fn worker() -> Capability {
    Capability::scoped(vec![
        format!("urn:cap:ledger:read:{LEDGER}"),
        format!("urn:cap:ledger:claim:{LEDGER}"),
        format!("urn:cap:ledger:comment:{LEDGER}"),
        format!("urn:cap:ledger:session:{LEDGER}"),
        graph_read(LEDGER),
        graph_write(LEDGER),
        session_read(LEDGER),
        session_write(LEDGER),
    ])
}

/// The worker grant as published in 0.22.0, without the session graph's lines.
fn old_worker() -> Capability {
    Capability::scoped(vec![
        format!("urn:cap:ledger:read:{LEDGER}"),
        format!("urn:cap:ledger:claim:{LEDGER}"),
        format!("urn:cap:ledger:comment:{LEDGER}"),
        format!("urn:cap:ledger:session:{LEDGER}"),
        graph_read(LEDGER),
        graph_write(LEDGER),
    ])
}

fn summaries_only() -> Capability {
    Capability::scoped(vec![
        format!("urn:cap:ledger:summaries:{LEDGER}"),
        session_read(LEDGER),
    ])
}

fn summary(session: &str) -> String {
    format!(
        r#"{{"session": "{session}", "machineName": "lawnboy", "harness": "claude",
             "model": "claude-opus-5-5", "exitStatus": 0, "wallClockMs": 1000}}"#
    )
}

/// The namespace the ledger's terms are in, read from a recorded summary rather than assumed.
fn ledger_ns(kernel: &Kernel) -> String {
    let rows = select(
        kernel,
        &format!("SELECT ?c WHERE {{ GRAPH <{SESSION_GRAPH}> {{ ?s a ?c }} }} LIMIT 1"),
    );
    let json: serde_json::Value = serde_json::from_str(&rows).unwrap();
    let class = json["results"]["bindings"][0]["c"]["value"]
        .as_str()
        .expect("a recorded summary")
        .to_string();
    class.trim_end_matches("SessionSummary").to_string()
}

/// How many summaries `graph` holds, asked of the store directly.
fn summaries_in(kernel: &Kernel, graph: &str, ns: &str) -> usize {
    let rows = select(
        kernel,
        &format!(
            "SELECT (COUNT(DISTINCT ?s) AS ?n) WHERE {{ GRAPH <{graph}> {{ ?s a <{ns}SessionSummary> }} }}"
        ),
    );
    let json: serde_json::Value = serde_json::from_str(&rows).unwrap();
    json["results"]["bindings"][0]["n"]["value"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}

/// An item, handed back by the worker.
fn handed_back(kernel: &Kernel, title: &str) {
    sink(kernel, &at("append"), &[("content", title)]);
    let id = format!("{}", count_items(kernel));
    try_as(
        kernel,
        &worker(),
        Verb::Sink,
        &at("comment"),
        &[("item", &id), ("content", "PR is up"), ("handback", "true")],
    )
    .expect("a worker hands back");
}

fn count_items(kernel: &Kernel) -> usize {
    source(kernel, &at("count"), &[]).trim().parse().unwrap()
}

fn record(kernel: &Kernel, id: &str, session: &str) -> Result<String, Error> {
    try_as(
        kernel,
        &worker(),
        Verb::Sink,
        &at(&format!("item:{id}:sessions")),
        &[("content", &summary(session))],
    )
}

/// Move a recorded summary back into the ledger graph, as 0.22.0 wrote it: no item number.
fn as_if_recorded_by_0_22(kernel: &Kernel, session: &str, ns: &str) {
    let iri = format!("urn:agent:session:id:{session}");
    let rows = select(
        kernel,
        &format!("SELECT ?p ?o WHERE {{ GRAPH <{SESSION_GRAPH}> {{ <{iri}> ?p ?o }} }}"),
    );
    let json: serde_json::Value = serde_json::from_str(&rows).unwrap();
    let mut data = String::new();
    for row in json["results"]["bindings"].as_array().unwrap() {
        let p = row["p"]["value"].as_str().unwrap();
        if p == format!("{ns}sessionItemNumber") {
            continue;
        }
        let o = &row["o"];
        let object = match o["type"].as_str().unwrap() {
            "uri" => format!("<{}>", o["value"].as_str().unwrap()),
            _ => format!(
                "{}^^<{}>",
                serde_json::to_string(o["value"].as_str().unwrap()).unwrap(),
                o["datatype"]
                    .as_str()
                    .unwrap_or("http://www.w3.org/2001/XMLSchema#string")
            ),
        };
        data.push_str(&format!("<{iri}> <{p}> {object} .\n"));
    }
    sink(
        kernel,
        "urn:iki:store:update",
        &[(
            "content",
            &format!(
                "DELETE WHERE {{ GRAPH <{SESSION_GRAPH}> {{ <{iri}> ?p ?o }} }} ; \
                 INSERT DATA {{ GRAPH <{LEDGER_GRAPH}> {{ {data} }} }}"
            ),
        )],
    );
}

/// ★ The migration: `plan` lists and writes nothing; `move` moves, adding each item's number;
/// a second `move` finds nothing. Until it runs, the summaries grant does not see them.
#[test]
fn the_migration_plans_first_and_moves_only_when_asked() {
    let kernel = kernel();
    handed_back(&kernel, "Secret title one");
    handed_back(&kernel, "Secret title two");
    record(&kernel, "1", "legacy-one").unwrap();
    record(&kernel, "2", "legacy-two").unwrap();
    let ns = ledger_ns(&kernel);
    as_if_recorded_by_0_22(&kernel, "legacy-one", &ns);
    as_if_recorded_by_0_22(&kernel, "legacy-two", &ns);
    assert_eq!(summaries_in(&kernel, LEDGER_GRAPH, &ns), 2);
    assert_eq!(summaries_in(&kernel, SESSION_GRAPH, &ns), 0);

    // Not yet moved: the summaries grant sees none of them.
    let before = try_as(
        &kernel,
        &summaries_only(),
        Verb::Source,
        &at("sessions"),
        &[],
    )
    .unwrap();
    assert_eq!(before, "acme has no recorded sessions\n");

    // The plan: each summary, and nothing written.
    let plan = sink(&kernel, &at("sessions"), &[("content", "plan")]);
    assert!(
        plan.starts_with(
            "acme: 2 session summaries (22 triples) would move from urn:iki:ledger:graph:acme \
             to urn:iki:ledger:graph:acme:session. This is a plan: nothing was written\n"
        ),
        "{plan}"
    );
    assert!(
        plan.contains("  session legacy-one on acme#1, 11 triples\n"),
        "{plan}"
    );
    assert!(
        plan.contains("  session legacy-two on acme#2, 11 triples\n"),
        "{plan}"
    );
    assert!(
        plan.ends_with("To move them, send `move` to this resource\n"),
        "{plan}"
    );
    assert!(!plan.contains("Secret"), "{plan}");
    assert_eq!(
        summaries_in(&kernel, LEDGER_GRAPH, &ns),
        2,
        "a plan moves nothing"
    );
    assert_eq!(
        summaries_in(&kernel, SESSION_GRAPH, &ns),
        0,
        "a plan moves nothing"
    );

    // The move.
    let moved = sink(&kernel, &at("sessions"), &[("content", "move")]);
    assert_eq!(
        moved,
        "acme: moved 2 session summaries (22 triples) from urn:iki:ledger:graph:acme to \
         urn:iki:ledger:graph:acme:session\n"
    );
    assert_eq!(summaries_in(&kernel, LEDGER_GRAPH, &ns), 0);
    assert_eq!(summaries_in(&kernel, SESSION_GRAPH, &ns), 2);

    // Read under exactly the summaries grant, each with its item's number added by the move.
    let after = try_as(
        &kernel,
        &summaries_only(),
        Verb::Source,
        &at("sessions"),
        &[],
    )
    .unwrap();
    assert!(after.contains("session legacy-one"), "{after}");
    assert!(
        after.contains("  item: acme#1 urn:iki:ledger:acme:item:"),
        "{after}"
    );
    assert!(
        after.contains("  item: acme#2 urn:iki:ledger:acme:item:"),
        "{after}"
    );

    // Again: nothing to move.
    let again = sink(&kernel, &at("sessions"), &[("content", "move")]);
    assert_eq!(
        again,
        "acme: no session summaries in urn:iki:ledger:graph:acme to move; nothing written\n"
    );

    match try_verb(&kernel, Verb::Sink, &at("sessions"), &[("content", "yes")]) {
        Err(Error::InvalidArgument { name, .. }) => assert_eq!(name, "content"),
        other => panic!("expected InvalidArgument, got {other:?}"),
    }
}

/// The migration is a write: the summaries grant cannot run it, nor a plan of it.
#[test]
fn the_migration_takes_the_write_grant() {
    let kernel = kernel();
    for content in ["plan", "move"] {
        match try_as(
            &kernel,
            &summaries_only(),
            Verb::Sink,
            &at("sessions"),
            &[("content", content)],
        ) {
            Err(Error::Denied(_)) => {}
            other => panic!("{content} under the summaries grant: expected Denied, got {other:?}"),
        }
    }
}

/// ★ A worker records into the session graph, and nothing of a summary is in the ledger graph. A
/// worker grant from before ledger #3661 is refused by name, and writes nothing.
#[test]
fn a_worker_records_into_the_session_graph_and_an_old_grant_is_refused_by_name() {
    let kernel = kernel();
    handed_back(&kernel, "Secret title");
    let answer = record(&kernel, "1", "new-grant").unwrap();
    assert_eq!(answer, "recorded session new-grant on acme#1's hand-back\n");
    let ns = ledger_ns(&kernel);
    assert_eq!(summaries_in(&kernel, SESSION_GRAPH, &ns), 1);
    assert_eq!(summaries_in(&kernel, LEDGER_GRAPH, &ns), 0);

    match try_as(
        &kernel,
        &old_worker(),
        Verb::Sink,
        &at("item:1:sessions"),
        &[("content", &summary("old-grant"))],
    ) {
        Err(Error::Denied(why)) => {
            assert!(
                why.contains(
                    "`urn:cap:store:read:graph:urn:iki:ledger:graph:acme:session` or \
                     `urn:cap:store:write:graph:urn:iki:ledger:graph:acme:session`"
                ),
                "{why}"
            );
            assert!(
                why.contains("A grant written before then, a worker's among them"),
                "{why}"
            );
        }
        other => panic!("the 0.22.0 worker grant: expected Denied, got {other:?}"),
    }
    assert_eq!(
        summaries_in(&kernel, SESSION_GRAPH, &ns),
        1,
        "nothing written"
    );
    assert_eq!(
        summaries_in(&kernel, LEDGER_GRAPH, &ns),
        0,
        "nothing written"
    );

    // The rest of the old grant still works: claim, comment and hand back.
    try_as(
        &kernel,
        &old_worker(),
        Verb::Sink,
        &at("comment"),
        &[
            ("item", "1"),
            ("content", "back again"),
            ("handback", "true"),
        ],
    )
    .expect("the old grant still hands back");
}

/// A delete moves an item's summaries from the session graph to the graveyard, and a purge
/// destroys them; neither leaves one in the session graph.
#[test]
fn delete_moves_and_purge_destroys_the_items_summaries() {
    let kernel = kernel();
    handed_back(&kernel, "One");
    handed_back(&kernel, "Two");
    record(&kernel, "1", "on-one").unwrap();
    record(&kernel, "2", "on-two").unwrap();
    let ns = ledger_ns(&kernel);

    delete(&kernel, &at("item:1"), &[]);
    assert_eq!(summaries_in(&kernel, SESSION_GRAPH, &ns), 1, "#2's stays");
    assert_eq!(summaries_in(&kernel, GRAVEYARD, &ns), 1, "#1's is archived");
    let set = source(&kernel, &at("sessions"), &[]);
    assert!(!set.contains("on-one"), "{set}");
    assert!(set.contains("on-two"), "{set}");

    // Purge of the deleted item destroys the archived summary.
    ok(&kernel, Verb::Delete, &at("purge"), &[("content", "1")]);
    assert_eq!(summaries_in(&kernel, GRAVEYARD, &ns), 0);

    // Purge of a live item destroys its summary outright.
    ok(&kernel, Verb::Delete, &at("purge"), &[("content", "2")]);
    assert_eq!(summaries_in(&kernel, SESSION_GRAPH, &ns), 0);
    assert_eq!(summaries_in(&kernel, GRAVEYARD, &ns), 0);
}

/// A delete under grants without the session graph's is refused by name, before anything moves.
#[test]
fn a_delete_without_the_session_graphs_grants_is_refused_by_name() {
    let kernel = kernel();
    handed_back(&kernel, "One");
    let deleter = Capability::scoped(vec![
        format!("urn:cap:ledger:delete:{LEDGER}"),
        graph_read(LEDGER),
        graph_write(LEDGER),
        graveyard_write(LEDGER),
    ]);
    match try_as(&kernel, &deleter, Verb::Delete, &at("item:1"), &[]) {
        Err(Error::Denied(why)) => assert!(why.contains(":acme:session`"), "{why}"),
        other => panic!("expected Denied, got {other:?}"),
    }
    assert_eq!(count_items(&kernel), 1, "nothing was deleted");
}
