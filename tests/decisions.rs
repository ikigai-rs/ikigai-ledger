mod common;

use common::*;
use ikigai_core::{Capability, Error, Iri, Kernel, Request, Verb};

const MAKER: &str = "urn:example:review-board";
const TIME: &str = "2023-11-14T21:00:00+01:00";

const DECISION: &str = "urn:iki:ledger:acme:decision";
const ITEM: &str = "urn:iki:ledger:acme:item:1";
const PREFIX: &str = "PREFIX ledger: <https://ikigai-rs.dev/ns/ledger#>";
const GRAPH: &str = "urn:iki:ledger:graph:acme";
const VALID: [(&str, &str); 4] = [
    ("item", "1"),
    ("content", "Stage the rollout"),
    ("decided-by", MAKER),
    ("decided-at", TIME),
];

fn rollout() -> Kernel {
    let kernel = kernel();
    sink(
        &kernel,
        "urn:iki:ledger:acme:append",
        &[("content", "Rollout")],
    );
    kernel
}

fn decision(kernel: &Kernel, args: &[(&str, &str)]) -> Result<String, Error> {
    try_as(
        kernel,
        &Capability::scoped([
            "urn:cap:ledger:comment:acme".to_string(),
            graph_read("acme"),
            graph_write("acme"),
        ]),
        Verb::Sink,
        DECISION,
        args,
    )
}

#[test]
fn a_decision_is_a_typed_comment_in_its_ledger() {
    let kernel = rollout();
    decision(&kernel, &VALID).expect("the comment grant records a decision");
    let plain = detail(&kernel, "text/plain");
    assert!(plain.lines().next().unwrap().contains("open"), "{plain}");
    assert!(plain.contains(&format!(
        "  decision maker: {MAKER} (claimed)\n  decision time: {TIME} (claimed)\n    Stage the rollout"
    )), "{plain}");
    assert!(
        !plain.chars().any(|c| c.is_control() && c != '\n'),
        "{plain}"
    );

    let record = comments(&kernel).remove(0);
    assert_eq!(record["kind"], "decision");
    assert_eq!(record["decided_by_status"], "claimed");
    assert_eq!(record["decided_at_status"], "claimed");
    assert_eq!(record["decided_by"], MAKER);
    assert_eq!(record["decided_at"], TIME);
    assert_ne!(record["decided_at"], record["time"]);
    assert!(record["author"].is_null());
    assert!(record["attribution"].is_null());

    let iri = record["id"].as_str().unwrap();
    let query = format!(
        "{PREFIX} SELECT ?maker ?time ?created ?committed WHERE {{ GRAPH <{GRAPH}> {{
            <{iri}> a ledger:Decision, ledger:Comment ; ledger:decidedBy ?maker ;
              ledger:decidedAt ?time ; <http://purl.org/dc/terms/created> ?created ;
              ledger:committedAt ?committed .
        }} }}"
    );
    let rows: serde_json::Value =
        serde_json::from_str(&graph_select(&kernel, GRAPH, &query)).unwrap();
    let rows = rows["results"]["bindings"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["maker"]["type"], "uri");
    assert_eq!(rows[0]["maker"]["value"], MAKER);
    assert_eq!(
        rows[0]["time"]["datatype"],
        "http://www.w3.org/2001/XMLSchema#dateTime"
    );
    assert_eq!(rows[0]["time"]["value"], TIME);
    assert_ne!(rows[0]["time"], rows[0]["created"]);
    assert_ne!(rows[0]["time"], rows[0]["committed"]);

    let turtle = detail(&kernel, "text/turtle");
    for term in ["Decision", "Comment", "decidedAt", "decidedBy", MAKER, TIME] {
        assert!(turtle.contains(term), "{turtle}");
    }
    for mode in ["all", "last"] {
        let listing = source(
            &kernel,
            "urn:iki:ledger:acme:items",
            &[("as", "application/json"), ("comments", mode)],
        );
        assert!(
            listing.contains(MAKER) && listing.contains(TIME),
            "{listing}"
        );
    }
}

fn graph_select(kernel: &Kernel, graph: &str, query: &str) -> String {
    try_as(
        kernel,
        &Capability::scoped([format!("urn:cap:store:read:graph:{graph}")]),
        Verb::Source,
        "urn:iki:store:graph-select",
        &[("graph", graph), ("query", query)],
    )
    .unwrap()
}

fn snapshot(kernel: &Kernel) -> String {
    graph_select(
        kernel,
        GRAPH,
        &format!("SELECT ?s ?p ?o WHERE {{ GRAPH <{GRAPH}> {{ ?s ?p ?o }} }} ORDER BY ?s ?p ?o"),
    )
}

fn write_fixture(kernel: &Kernel, sparql: &str) {
    sink(
        kernel,
        "urn:iki:store:graph-update",
        &[("graph", GRAPH), ("content", sparql)],
    );
}

#[test]
fn validation_and_fence_refusals_leave_the_graph_unchanged() {
    let kernel = rollout();
    let before = snapshot(&kernel);
    for (name, value) in [
        ("item", None),
        ("item", Some("99")),
        ("content", None),
        ("content", Some(" \n")),
        ("decided-by", None),
        ("decided-by", Some("relative")),
        ("decided-by", Some("urn:one urn:two")),
        ("decided-at", None),
        ("decided-at", Some("2023-11-14")),
        ("decided-at", Some("2023-11-14T00:00:00")),
        ("decided-at", Some("2023-02-29T00:00:00Z")),
        ("decided-at", Some("2023-11-14T00:00:00+01:99")),
        ("decided-at", Some("2999-01-01T00:00:00Z")),
        ("key", Some("invalid key")),
        ("epoch", Some("bad")),
        ("as", Some("text/turtle")),
        ("epoch", Some("1")),
    ] {
        let mut args: Vec<_> = VALID
            .into_iter()
            .filter(|(field, _)| *field != name)
            .collect();
        args.extend(value.map(|v| (name, v)));
        let error = decision(&kernel, &args).expect_err("invalid request");
        match (name, value) {
            (_, None) => assert!(
                matches!(error, Error::MissingArgument(ref field) if field == name),
                "{error}"
            ),
            ("item", Some("99")) => {
                assert!(matches!(error, Error::NotFound(_)), "{error}")
            }
            ("epoch", Some("1")) => {
                assert!(matches!(error, Error::Conflict(_)), "{error}")
            }
            _ => assert!(
                matches!(error, Error::InvalidArgument { name: ref field, .. } if field == name),
                "{error}"
            ),
        }
        assert_eq!(snapshot(&kernel), before, "{name}={value:?}");
    }
}

fn detail(kernel: &Kernel, face: &str) -> String {
    source(kernel, ITEM, &[("as", face)])
}

fn comments(kernel: &Kernel) -> Vec<serde_json::Value> {
    let json: serde_json::Value =
        serde_json::from_str(&detail(kernel, "application/json")).unwrap();
    json["item"]["comments"].as_array().unwrap().clone()
}

#[test]
fn fences_and_shared_keys_preserve_the_original_comment_and_item() {
    let kernel = rollout();
    sink(
        &kernel,
        "urn:iki:ledger:acme:append",
        &[("content", "Follow-up")],
    );
    sink(
        &kernel,
        "urn:iki:ledger:acme:claim",
        &[("item", "1"), ("content", "worker")],
    );
    let mut args = VALID.to_vec();
    args.extend([("key", "rollout"), ("epoch", "1")]);
    decision(&kernel, &args).unwrap();
    sink(
        &kernel,
        "urn:iki:ledger:acme:comment",
        &[
            ("item", "2"),
            ("key", "ordinary"),
            ("content", "Ordinary remark"),
        ],
    );
    let before = snapshot(&kernel);
    let read = Request::new(Verb::Source, Iri::parse(ITEM).unwrap());
    source(&kernel, ITEM, &[]);
    assert!(kernel.is_cached(&read, &Capability::root()));
    let replay = decision(
        &kernel,
        &[
            ("item", "2"),
            ("content", "Retried content"),
            ("key", "rollout"),
            ("decided-by", "urn:example:different"),
            ("decided-at", "2023-11-14T00:00:00Z"),
            ("as", "application/json"),
        ],
    )
    .unwrap();
    let replay: serde_json::Value = serde_json::from_str(&replay).unwrap();
    assert_eq!(replay["outcome"], "existing");
    // The read carries the comment's revision (ledger #4199); a write's answer, which a
    // replay repeats, never does, since the store allocates it after the answer is rendered.
    let mut read_back = comments(&kernel)[0].clone();
    assert!(read_back["item_revision"].is_u64(), "{read_back}");
    read_back.as_object_mut().unwrap().remove("item_revision");
    assert_eq!(replay["comment"], read_back);
    assert_eq!(replay["comment"]["text"], "Stage the rollout");
    assert_eq!(replay["comment"]["decided_by"], MAKER);
    let ordinary = decision(
        &kernel,
        &[
            ("item", "1"),
            ("content", "Retried content"),
            ("key", "ordinary"),
            ("decided-by", MAKER),
            ("decided-at", TIME),
        ],
    )
    .unwrap();
    assert!(ordinary.starts_with("commented on acme#2 "), "{ordinary}");
    assert!(ordinary.ends_with(" existing\n"), "{ordinary}");
    assert_eq!(snapshot(&kernel), before);
    assert!(kernel.is_cached(&read, &Capability::root()));
}

#[test]
fn ordinary_comments_keep_their_faces_and_malformed_decisions_keep_their_body() {
    let kernel = rollout();
    sink(
        &kernel,
        "urn:iki:ledger:acme:comment",
        &[
            ("item", "1"),
            ("content", "Ordinary remark"),
            ("author", "Filer"),
        ],
    );
    let old = comments(&kernel).remove(0);
    let plain = detail(&kernel, "text/plain");
    let stamp = plain
        .lines()
        .find(|line| line.contains("comment by"))
        .unwrap();
    assert!(stamp.contains("Filer (claimed, free text)"), "{plain}");
    assert!(!plain.contains("decision maker:"), "{plain}");
    let turtle = detail(&kernel, "text/turtle");
    assert!(
        !turtle.contains("decidedBy") && !turtle.contains("decidedAt"),
        "{turtle}"
    );
    let mut args = VALID.to_vec();
    args.push(("author", "Filer"));
    decision(&kernel, &args).unwrap();
    let typed = comments(&kernel);
    assert_eq!(typed[0], old);
    let iri = typed[1]["id"].as_str().unwrap();
    write_fixture(&kernel, &format!(
        "{PREFIX} DELETE WHERE {{ GRAPH <{GRAPH}> {{ <{iri}> ledger:decidedBy ?maker ; ledger:decidedAt ?time }} }};
         INSERT DATA {{ GRAPH <{GRAPH}> {{ <{iri}> ledger:decidedBy \"Filer\" ;
           ledger:decidedAt \"2023-11-14T21:00:00\"^^<http://www.w3.org/2001/XMLSchema#dateTime> }} }}"
    ));
    let malformed = comments(&kernel);
    assert_eq!(malformed[0], old);
    assert_eq!(malformed[1]["text"], "Stage the rollout");
    assert_eq!(malformed[1]["author"], "Filer");
    for field in ["decided_by", "decided_at"] {
        assert!(malformed[1].get(field).is_none(), "{}", malformed[1]);
    }
    let plain = detail(&kernel, "text/plain");
    assert!(plain.lines().any(|line| line == stamp), "{plain}");
    assert!(plain.contains("Stage the rollout"), "{plain}");
    assert!(
        !plain.contains("decision maker:") && !plain.contains("decision time:"),
        "{plain}"
    );
    let turtle = detail(&kernel, "text/turtle");
    assert!(
        turtle.contains("Decision") && turtle.contains("Stage the rollout"),
        "{turtle}"
    );
    assert!(
        !turtle.contains("decidedBy") && !turtle.contains("decidedAt"),
        "{turtle}"
    );
}

#[test]
fn the_comment_and_correct_graph_grants_are_required() {
    let kernel = rollout();
    let mut args = VALID.to_vec();
    args.push(("key", "rollout"));
    decision(&kernel, &args).unwrap();
    let before = snapshot(&kernel);
    let comment = "urn:cap:ledger:comment:acme".to_string();
    for scopes in [
        vec![graph_read("acme"), graph_write("acme")],
        vec![comment.clone(), graph_read("acme"), graph_write("default")],
        vec![comment, graph_read("default"), graph_write("acme")],
    ] {
        let error = try_as(
            &kernel,
            &Capability::scoped(scopes),
            Verb::Sink,
            DECISION,
            &args,
        )
        .expect_err("the missing grant must be enforced");
        assert!(matches!(error, Error::Denied { .. }), "{error}");
        assert_eq!(snapshot(&kernel), before);
    }
}

#[test]
fn deleting_an_item_archives_its_decision_and_frees_the_key() {
    let kernel = rollout();
    let mut args = VALID.to_vec();
    args.push(("key", "rollout"));
    decision(&kernel, &args).unwrap();
    let record = comments(&kernel).remove(0);
    let iri = record["id"].as_str().unwrap();
    delete(&kernel, ITEM, &[("reason", "wrong item")]);
    assert!(!snapshot(&kernel).contains(iri));
    let graveyard = format!("{GRAPH}:deleted");
    let query = format!(
        "{PREFIX} SELECT ?maker ?time WHERE {{ GRAPH <{graveyard}> {{
            <{iri}> a ledger:Decision, ledger:Comment ;
                ledger:decidedBy ?maker ; ledger:decidedAt ?time .
        }} }}"
    );
    let archived = graph_select(&kernel, &graveyard, &query);
    assert!(
        archived.contains(MAKER) && archived.contains(TIME),
        "{archived}"
    );
    sink(
        &kernel,
        "urn:iki:ledger:acme:append",
        &[("content", "The right item")],
    );
    args[0].1 = "2";
    let filed = decision(&kernel, &args).unwrap();
    assert!(filed.starts_with("recorded decision on acme#2"), "{filed}");
}

#[test]
fn one_bad_fact_does_not_hide_the_other() {
    let kernel = rollout();
    decision(&kernel, &VALID).unwrap();
    let record = comments(&kernel).remove(0);
    let iri = record["id"].as_str().unwrap();
    write_fixture(&kernel, &format!(
        "{PREFIX} INSERT DATA {{ GRAPH <{GRAPH}> {{ <{iri}> ledger:decidedBy <urn:example:alternate> }} }}"
    ));
    let ambiguous = comments(&kernel);
    assert_eq!(ambiguous.len(), 1);
    assert!(ambiguous[0].get("decided_by").is_none());
    assert_eq!(ambiguous[0]["decided_at"], TIME);

    write_fixture(
        &kernel,
        &format!(
            "{PREFIX} DELETE WHERE {{ GRAPH <{GRAPH}> {{ <{iri}> ledger:decidedBy ?maker }} }}"
        ),
    );
    let absent = comments(&kernel);
    assert_eq!(absent, ambiguous);
    let plain = detail(&kernel, "text/plain");
    assert!(
        plain.contains(&format!("decision time: {TIME} (claimed)")),
        "{plain}"
    );
    assert!(!plain.contains("decision maker:"), "{plain}");
    let turtle = detail(&kernel, "text/turtle");
    assert!(
        turtle.contains("decidedAt") && !turtle.contains("decidedBy"),
        "{turtle}"
    );
}

#[test]
fn a_dispatcher_decision_answers_a_hand_back_and_leaves_the_item_open() {
    let kernel = rollout();
    sink(
        &kernel,
        "urn:iki:ledger:acme:comment",
        &[
            ("item", "1"),
            ("content", "Over to you"),
            ("handback", "true"),
        ],
    );
    let reply = sink(&kernel, DECISION, &VALID);
    assert!(reply.ends_with(" cleared-awaiting\n"), "{reply}");
    let item: serde_json::Value =
        serde_json::from_str(&detail(&kernel, "application/json")).unwrap();
    assert_eq!(item["item"]["status"], "open");
    assert!(item["item"]["awaiting"].is_null());
    let records = item["item"]["comments"].as_array().unwrap();
    assert_eq!(records[1]["rejects"][0], records[0]["id"]);
}
