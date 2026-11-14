//! **A ledger's session summaries as one set** (ledger #3647).
//!
//! `urn:iki:ledger:{ledger}:sessions` is every summary in the ledger, newest first, paged, raw:
//! no totals and no cost, and each summary names its item by id alone. It is readable under
//! `urn:cap:ledger:summaries:{ledger}`, a grant that admits this set and no item text, and is
//! named outside the session WRITE family `urn:cap:ledger:session:*`. These tests run under
//! exactly that grant and the store read grant of the ledger's SESSION graph, which holds the
//! summaries and nothing else (ledger #3661):
//!
//! - the set reads, newest first, in text, Turtle and JSON, and pages with `limit` and `offset`;
//! - an item's title is refused at `item:{id}`, and so is every other read of item text;
//! - and at the store too: `urn:iki:store:graph-select` over the ledger graph is `Denied`, and no
//!   store door answers a title;
//! - the grant records nothing, and the session grant does not read the set.

mod common;

use common::*;
use ikigai_core::{Capability, Error, Kernel, Verb};

const LEDGER: &str = "acme";

fn at(resource: &str) -> String {
    format!("urn:iki:ledger:{LEDGER}:{resource}")
}

/// The summaries grant, and the store read grant of the session graph. Nothing else: not the
/// ledger graph's store read grant, which reads every title (ledger #3661).
fn summaries_only() -> Capability {
    Capability::scoped(vec![
        format!("urn:cap:ledger:summaries:{LEDGER}"),
        session_read(LEDGER),
    ])
}

/// The published worker grant, which carries the session WRITE grant.
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

fn summary(session: &str, input_tokens: u64) -> String {
    format!(
        r#"{{"session": "{session}", "machineName": "lawnboy", "harness": "claude",
             "model": "claude-opus-5-5", "exitStatus": 0, "wallClockMs": 1000,
             "inputTokens": {input_tokens}}}"#
    )
}

/// Hand item `id` back as the worker and record `session` on that hand-back.
fn hand_back_and_record(kernel: &Kernel, id: &str, session: &str, input_tokens: u64) {
    let w = worker();
    try_as(
        kernel,
        &w,
        Verb::Sink,
        &at("comment"),
        &[("item", id), ("content", "PR is up"), ("handback", "true")],
    )
    .expect("a worker hands back");
    try_as(
        kernel,
        &w,
        Verb::Sink,
        &at(&format!("item:{id}:sessions")),
        &[("content", &summary(session, input_tokens))],
    )
    .expect("a worker records its session");
}

/// Three items whose titles are secrets, and three sessions recorded in order: `s-one` on #1,
/// `s-two` on #2, `s-three` on #1 again. Each a millisecond apart, so newest first is fixed.
fn seeded() -> Kernel {
    let kernel = kernel();
    for title in ["Secret title one", "Secret title two", "Secret title three"] {
        sink(&kernel, &at("append"), &[("content", title)]);
    }
    hand_back_and_record(&kernel, "1", "s-one", 100);
    std::thread::sleep(std::time::Duration::from_millis(3));
    hand_back_and_record(&kernel, "2", "s-two", 200);
    std::thread::sleep(std::time::Duration::from_millis(3));
    hand_back_and_record(&kernel, "1", "s-three", 300);
    kernel
}

fn set(kernel: &Kernel, args: &[(&str, &str)]) -> Result<String, Error> {
    try_as(
        kernel,
        &summaries_only(),
        Verb::Source,
        &at("sessions"),
        args,
    )
}

fn denied(result: Result<String, Error>, what: &str) {
    match result {
        Err(Error::Denied(_)) => {}
        other => panic!("{what}: expected Denied, got {other:?}"),
    }
}

/// The new grant is named outside the session write family, and outside the read family.
#[test]
fn the_grant_falls_inside_neither_the_session_nor_the_read_family() {
    let token = ikigai_ledger::Ledger::parse(LEDGER)
        .unwrap()
        .cap_summaries();
    assert_eq!(token, "urn:cap:ledger:summaries:acme");
    let held = Capability::scoped(vec![token]);
    assert!(!held.satisfies(ikigai_ledger::CAP_SESSION));
    assert!(!held.satisfies(ikigai_ledger::CAP_READ));
    assert!(held.satisfies(ikigai_ledger::CAP_SUMMARIES));
}

/// ★ Under exactly the summaries grant: the set reads, and an item's title is refused.
#[test]
fn the_summaries_grant_reads_the_set_and_is_refused_an_items_title() {
    let kernel = seeded();

    let text = set(&kernel, &[]).expect("the summaries grant reads the set");
    assert!(
        text.starts_with("acme: sessions, newest first, from offset 0\n"),
        "{text}"
    );
    let sessions: Vec<&str> = text
        .lines()
        .filter_map(|line| line.strip_prefix("session "))
        .map(|rest| rest.split_whitespace().next().unwrap())
        .collect();
    assert_eq!(sessions, ["s-three", "s-two", "s-one"], "{text}");
    assert!(
        text.contains("  item: acme#1 urn:iki:ledger:acme:item:"),
        "{text}"
    );
    assert!(
        text.contains("  item: acme#2 urn:iki:ledger:acme:item:"),
        "{text}"
    );
    assert!(!text.contains("Secret"), "no item text in the set: {text}");
    assert!(!text.to_lowercase().contains("cost"), "no cost: {text}");
    assert!(!text.to_lowercase().contains("total"), "no totals: {text}");

    // An item's title, by every door that serves one.
    denied(
        try_as(&kernel, &summaries_only(), Verb::Source, &at("item:1"), &[]),
        "item:1",
    );
    denied(
        try_as(&kernel, &summaries_only(), Verb::Source, &at("items"), &[]),
        "items",
    );
    denied(
        try_as(&kernel, &summaries_only(), Verb::Source, &at("next"), &[]),
        "next",
    );
    denied(
        try_as(
            &kernel,
            &summaries_only(),
            Verb::Source,
            &at("item:1:history"),
            &[],
        ),
        "item:1:history",
    );
    denied(
        try_as(
            &kernel,
            &summaries_only(),
            Verb::Source,
            &at("item:1:sessions"),
            &[],
        ),
        "item:1:sessions",
    );
}

#[test]
fn the_json_face_is_raw_with_item_ids_only() {
    let kernel = seeded();
    let text = set(&kernel, &[("as", "application/json")]).unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["ledger"], "acme");
    let sessions = json["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 3);
    assert_eq!(sessions[0]["session"], "s-three");
    assert_eq!(sessions[0]["inputTokens"], 300);
    assert_eq!(sessions[0]["item"]["display"], "acme#1");
    assert!(sessions[0]["item"].get("title").is_none(), "{text}");
    assert_eq!(sessions[1]["item"]["display"], "acme#2");
    for key in ["total", "totals", "count", "cost"] {
        assert!(json.get(key).is_none(), "{key} in {text}");
    }
    assert!(!text.contains("Secret"), "{text}");
    assert_eq!(json["next_offset"], serde_json::Value::Null);
}

#[test]
fn the_set_pages_newest_first() {
    let kernel = seeded();
    let page = |offset: &str| {
        let text = set(
            &kernel,
            &[
                ("limit", "2"),
                ("offset", offset),
                ("as", "application/json"),
            ],
        )
        .unwrap();
        serde_json::from_str::<serde_json::Value>(&text).unwrap()
    };
    let first = page("0");
    let ids: Vec<&str> = first["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["session"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["s-three", "s-two"]);
    assert_eq!(first["next_offset"], 2);
    let second = page("2");
    let ids: Vec<&str> = second["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["session"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["s-one"]);
    assert_eq!(second["next_offset"], serde_json::Value::Null);

    let text = set(&kernel, &[("limit", "2")]).unwrap();
    assert!(
        text.ends_with("more sessions follow: read again with offset=2\n"),
        "{text}"
    );
    let text = set(&kernel, &[("limit", "2"), ("offset", "2")]).unwrap();
    assert!(!text.contains("more sessions follow"), "{text}");

    match set(&kernel, &[("limit", "ten")]) {
        Err(Error::InvalidArgument { name, .. }) => assert_eq!(name, "limit"),
        other => panic!("expected InvalidArgument on limit, got {other:?}"),
    }
}

#[test]
fn the_turtle_face_holds_the_summaries_and_no_item_text() {
    let kernel = seeded();
    let turtle = set(&kernel, &[("as", "text/turtle")]).unwrap();
    assert!(turtle.contains("urn:agent:session:id:s-one"), "{turtle}");
    assert!(turtle.contains("urn:agent:session:id:s-three"), "{turtle}");
    assert!(!turtle.contains("Secret"), "{turtle}");
}

#[test]
fn an_empty_ledger_says_so() {
    let kernel = kernel();
    sink(
        &kernel,
        &at("append"),
        &[("content", "Nothing handed back")],
    );
    let text = set(&kernel, &[]).unwrap();
    assert_eq!(text, "acme has no recorded sessions\n");
}

/// The summaries grant writes nothing; the session WRITE grant does not read the set; the read
/// grant does; a summaries grant for another ledger does not.
#[test]
fn each_grant_reaches_only_its_own_door() {
    let kernel = seeded();
    // Records nothing.
    try_as(
        &kernel,
        &worker(),
        Verb::Sink,
        &at("comment"),
        &[("item", "3"), ("content", "back"), ("handback", "true")],
    )
    .unwrap();
    denied(
        try_as(
            &kernel,
            &summaries_only(),
            Verb::Sink,
            &at("item:3:sessions"),
            &[("content", &summary("s-four", 1))],
        ),
        "recording a session",
    );

    // The session write grant, alone with its store grants, does not read the set.
    let session_only = Capability::scoped(vec![
        format!("urn:cap:ledger:session:{LEDGER}"),
        graph_read(LEDGER),
        graph_write(LEDGER),
    ]);
    match try_as(&kernel, &session_only, Verb::Source, &at("sessions"), &[]) {
        Err(Error::Denied(why)) => assert!(
            why.contains("`urn:cap:ledger:summaries:acme` nor `urn:cap:ledger:read:acme`"),
            "{why}"
        ),
        other => panic!("the session write grant reading the set: expected Denied, got {other:?}"),
    }

    // The ledger read grant covers it, with the session graph's store read grant.
    let reader = Capability::scoped(vec![
        format!("urn:cap:ledger:read:{LEDGER}"),
        graph_read(LEDGER),
        session_read(LEDGER),
    ]);
    try_as(&kernel, &reader, Verb::Source, &at("sessions"), &[])
        .expect("the read grant reads the set");

    // Another ledger's summaries grant does not.
    let elsewhere = Capability::scoped(vec![
        "urn:cap:ledger:summaries:bosatsu".to_string(),
        graph_read(LEDGER),
    ]);
    denied(
        try_as(&kernel, &elsewhere, Verb::Source, &at("sessions"), &[]),
        "another ledger's summaries grant",
    );
}

/// Resolve a store door under exactly the summaries grant.
fn store(kernel: &Kernel, door: &str, args: &[(&str, &str)]) -> Result<String, Error> {
    try_as(
        kernel,
        &summaries_only(),
        Verb::Source,
        &format!("urn:iki:store:{door}"),
        args,
    )
}

const LEDGER_GRAPH: &str = "urn:iki:ledger:graph:acme";
const SESSION_GRAPH: &str = "urn:iki:ledger:graph:acme:session";
const TITLES: &str = "SELECT ?t WHERE { ?s <http://purl.org/dc/terms/title> ?t }";

/// ★ Ledger #3661: under exactly the summaries grant and the session graph's store grant, the set
/// reads, the store's `graph-select` over the LEDGER graph is `Denied`, and an item's title is
/// unreachable by any door, the ledger's or the store's.
#[test]
fn the_summaries_grant_reads_no_title_at_the_store_either() {
    let kernel = seeded();
    let text = set(&kernel, &[]).expect("the set reads");
    assert!(text.contains("session s-three"), "{text}");
    assert!(
        text.contains("  item: acme#1 urn:iki:ledger:acme:item:"),
        "{text}"
    );

    // The ledger graph, through the narrow doors: Denied.
    denied(
        store(
            &kernel,
            "graph-select",
            &[("graph", LEDGER_GRAPH), ("query", TITLES)],
        ),
        "graph-select over the ledger graph",
    );
    denied(
        store(
            &kernel,
            "graph-ask",
            &[("graph", LEDGER_GRAPH), ("query", "ASK { ?s ?p ?o }")],
        ),
        "graph-ask over the ledger graph",
    );
    denied(
        store(
            &kernel,
            "graph-construct",
            &[
                ("graph", LEDGER_GRAPH),
                ("query", "CONSTRUCT WHERE { ?s ?p ?o }"),
            ],
        ),
        "graph-construct over the ledger graph",
    );
    denied(
        store(
            &kernel,
            "graph-select",
            &[
                ("graph", "urn:iki:ledger:graph:acme:deleted"),
                ("query", TITLES),
            ],
        ),
        "graph-select over the graveyard",
    );

    // The session graph reads, and holds summaries and no item text.
    let all = store(
        &kernel,
        "graph-select",
        &[
            ("graph", SESSION_GRAPH),
            ("query", "SELECT ?s ?p ?o WHERE { ?s ?p ?o }"),
        ],
    )
    .expect("the session graph's store grant reads the session graph");
    assert!(all.contains("urn:agent:session:id:s-one"), "{all}");
    assert!(!all.contains("Secret"), "{all}");
    assert!(
        !all.contains("http://purl.org/dc/terms/title"),
        "no title triple in the session graph: {all}"
    );

    // Every other door, the store's broad ones and the ledger's own: none answers a title.
    let doors: Vec<(String, Vec<(&str, &str)>)> = vec![
        ("urn:iki:store:select".into(), vec![("query", TITLES)]),
        (
            "urn:iki:store:ask".into(),
            vec![("query", "ASK { ?s ?p ?o }")],
        ),
        (
            "urn:iki:store:construct".into(),
            vec![("query", "CONSTRUCT WHERE { ?s ?p ?o }")],
        ),
        (
            "urn:iki:store:describe".into(),
            vec![("query", "DESCRIBE ?s WHERE { ?s ?p ?o }")],
        ),
        ("urn:iki:store:dump".into(), vec![]),
        ("urn:iki:store:info".into(), vec![]),
        ("urn:iki:store:graphs".into(), vec![]),
        (
            "urn:iki:store:graph-info".into(),
            vec![("graph", LEDGER_GRAPH)],
        ),
        (
            "urn:iki:store:graph-select".into(),
            vec![
                ("graph", SESSION_GRAPH),
                (
                    "query",
                    "SELECT ?t WHERE { GRAPH <urn:iki:ledger:graph:acme> { ?s ?p ?t } }",
                ),
            ],
        ),
        (at("item:1"), vec![]),
        (at("items"), vec![]),
        (at("next"), vec![]),
        (at("count"), vec![]),
        (at("counts"), vec![]),
        (at("doctor"), vec![]),
        (at("item:1:history"), vec![]),
        (at("item:1:sessions"), vec![]),
        ("urn:iki:ledger:ledgers".into(), vec![]),
    ];
    for (door, args) in &doors {
        for face in ["text/plain", "text/turtle", "application/json"] {
            let mut with = args.clone();
            if door.starts_with("urn:iki:ledger:") {
                with.push(("as", face));
            }
            if let Ok(text) = try_as(&kernel, &summaries_only(), Verb::Source, door, &with) {
                assert!(!text.contains("Secret"), "{door} answered a title: {text}");
            }
        }
    }
}

/// The ledger graph's store read grant alone, beside the summaries grant, no longer reads the
/// set: the summaries are not in that graph, and the refusal names the line to add.
#[test]
fn the_ledger_graphs_store_grant_does_not_read_the_set() {
    let kernel = seeded();
    let old_pair = Capability::scoped(vec![
        format!("urn:cap:ledger:summaries:{LEDGER}"),
        graph_read(LEDGER),
    ]);
    match try_as(&kernel, &old_pair, Verb::Source, &at("sessions"), &[]) {
        Err(Error::Denied(why)) => assert!(
            why.contains("`urn:cap:store:read:graph:urn:iki:ledger:graph:acme:session`"),
            "{why}"
        ),
        other => panic!("expected Denied naming the session graph's grant, got {other:?}"),
    }
}
