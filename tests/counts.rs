mod common;
use common::*;
use ikigai_core::Verb;

#[test]
fn close_records_its_own_time_and_counts_are_a_resource() {
    let k = kernel();
    let item = append_iri(&k, "A day's work", &[]);
    sink(
        &k,
        "urn:iki:ledger:close",
        &[("item", &item), ("content", "done")],
    );
    let evidence = select(&k, &format!("SELECT ?time WHERE {{ GRAPH <urn:iki:ledger:graph:default> {{ <{item}> <https://ikigai-rs.dev/ns/ledger#closedAt> ?time }} }}"));
    assert!(
        evidence.contains("2026-09-15"),
        "close has no timestamp: {evidence}"
    );
    let csv = source(&k, "urn:iki:ledger:counts", &[]);
    assert_eq!(csv, "date,open,closed\n2026-09-15,0,1\n");
    assert!(try_verb(
        &k,
        Verb::Source,
        "urn:iki:ledger:counts",
        &[("as", "text/turtle")]
    )
    .is_ok());
}

#[test]
fn counts_resource_exists_before_any_items() {
    assert_eq!(
        source(&kernel(), "urn:iki:ledger:counts", &[]),
        "date,open,closed\n"
    );
}

use ikigai_core::{Capability, Clock, Fallback, Kernel, Space, Time};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
struct ManualClock(AtomicU64);
impl Clock for ManualClock {
    fn now(&self) -> Time {
        Time::from_millis(self.0.load(Ordering::SeqCst))
    }
}
const DAY: u64 = 86_400_000;
const START: u64 = 1_789_430_400_000;
fn dated_kernel() -> (Kernel, Arc<ManualClock>) {
    let clock = Arc::new(ManualClock(AtomicU64::new(START)));
    let spaces = Fallback::new(vec![
        Arc::new(ikigai_store::space(
            ikigai_store::DurableStore::in_memory().unwrap(),
        )) as Arc<dyn Space>,
        Arc::new(ikigai_ledger::space()) as Arc<dyn Space>,
    ]);
    (
        Kernel::new(Arc::new(spaces)).with_clock(clock.clone()),
        clock,
    )
}
fn day(clock: &ManualClock, offset: u64) {
    clock.0.store(START + offset * DAY, Ordering::SeqCst);
}

#[test]
fn daily_series_retains_close_and_reopen_history_and_faces_agree() {
    let (k, clock) = dated_kernel();
    let a = append_iri(&k, "A", &[]);
    day(&clock, 1);
    let b = append_iri(&k, "B", &[]);
    sink(&k, "urn:iki:ledger:close", &[("item", &b)]);
    day(&clock, 2);
    sink(&k, "urn:iki:ledger:close", &[("item", &a)]);
    day(&clock, 3);
    sink(&k, "urn:iki:ledger:reopen", &[("item", &a)]);
    let csv = source(&k, "urn:iki:ledger:counts", &[]);
    assert_eq!(
        csv,
        "date,open,closed\n2026-09-15,1,0\n2026-09-16,1,1\n2026-09-17,0,2\n2026-09-18,1,1\n"
    );
    assert_eq!(
        source(&k, "urn:iki:ledger:counts", &[("days", "2")]),
        "date,open,closed\n2026-09-17,0,2\n2026-09-18,1,1\n"
    );
    let ttl = source(&k, "urn:iki:ledger:counts", &[("as", "text/turtle")]);
    let quads: Vec<_> = oxrdfio::RdfParser::from_format(oxrdfio::RdfFormat::Turtle)
        .for_reader(ttl.as_bytes())
        .collect::<Result<_, _>>()
        .unwrap();
    for row in csv.lines().skip(1) {
        let cells: Vec<_> = row.split(',').collect();
        let subject = format!("urn:iki:ledger:default:counts:{}", cells[0]);
        for (predicate, value) in [
            ("date", cells[0]),
            ("openCount", cells[1]),
            ("closedCount", cells[2]),
        ] {
            assert!(
                quads
                    .iter()
                    .any(|q| q.subject.to_string() == format!("<{subject}>")
                        && q.predicate.as_str()
                            == format!("https://ikigai-rs.dev/ns/ledger#{predicate}")
                        && matches!(&q.object, oxrdf::Term::Literal(l) if l.value() == value)),
                "missing {predicate} {value}: {ttl}"
            );
        }
    }
    assert!(quads
        .iter()
        .all(|q| matches!(q.subject, oxrdf::NamedOrBlankNode::NamedNode(_))));
    let timestamp = select(&k, &format!("SELECT ?t WHERE {{ GRAPH <urn:iki:ledger:graph:default> {{ <{a}> <https://ikigai-rs.dev/ns/ledger#closedAt> ?t }} }}"));
    assert!(
        !timestamp.contains("2026-09-17"),
        "reopen removes current closedAt"
    );
}

#[test]
fn authority_is_per_ledger_and_days_are_checked() {
    let k = kernel();
    for cap in [
        Capability::scoped([graph_read("default")]),
        Capability::scoped([
            "urn:cap:ledger:read:other".to_string(),
            graph_read("default"),
        ]),
    ] {
        assert!(matches!(
            try_as(&k, &cap, Verb::Source, "urn:iki:ledger:counts", &[]),
            Err(ikigai_core::Error::Denied(_))
        ));
    }
    let reader = Capability::scoped([
        "urn:cap:ledger:read:default".to_string(),
        graph_read("default"),
    ]);
    assert!(try_as(&k, &reader, Verb::Source, "urn:iki:ledger:counts", &[]).is_ok());
    for value in ["0", "-1", "no", "18446744073709551616"] {
        assert!(try_verb(
            &k,
            Verb::Source,
            "urn:iki:ledger:counts",
            &[("days", value)]
        )
        .is_err());
    }
}

fn update(k: &Kernel, content: &str) {
    sink(
        k,
        "urn:iki:store:graph-update",
        &[
            ("graph", "urn:iki:ledger:graph:default"),
            ("content", content),
        ],
    );
}
fn legacy_close(k: &Kernel, item: &str) {
    update(k, &format!("DELETE {{ GRAPH <urn:iki:ledger:graph:default> {{ <{item}> <https://ikigai-rs.dev/ns/ledger#status> ?old }} }} INSERT {{ GRAPH <urn:iki:ledger:graph:default> {{ <{item}> <https://ikigai-rs.dev/ns/ledger#status> <https://ikigai-rs.dev/ns/ledger#closed> }} }} WHERE {{ GRAPH <urn:iki:ledger:graph:default> {{ <{item}> <https://ikigai-rs.dev/ns/ledger#status> ?old }} }}"));
}
fn close_time(k: &Kernel, item: &str) -> String {
    select(k, &format!("SELECT ?at ?source WHERE {{ GRAPH <urn:iki:ledger:graph:default> {{ <{item}> <https://ikigai-rs.dev/ns/ledger#closedAt> ?at . OPTIONAL {{ <{item}> <https://ikigai-rs.dev/ns/ledger#closedAtSource> ?source }} }} }}"))
}

#[test]
fn explicit_backfill_uses_reviewed_close_comment_or_updated_and_is_marked() {
    let (k, clock) = dated_kernel();
    let a = append_iri(&k, "Legacy, with close note", &[]);
    let b = append_iri(&k, "Legacy, no close note", &[]);
    day(&clock, 1);
    let receipt = sink(
        &k,
        "urn:iki:ledger:comment",
        &[
            ("item", &a),
            ("content", "closed then"),
            ("as", "application/json"),
        ],
    );
    let receipt: serde_json::Value = serde_json::from_str(&receipt).unwrap();
    let comment = receipt["comment"]["id"].as_str().expect("comment receipt");
    legacy_close(&k, &a);
    legacy_close(&k, &b);
    day(&clock, 2);
    // Later unrelated edits move updated, but the supplied close comment wins.
    sink(&k, &a, &[("content", "Later title")]);
    sink(&k, &b, &[("content", "Later title, no note")]);
    assert!(
        try_verb(&k, Verb::Source, "urn:iki:ledger:counts", &[]).is_err(),
        "legacy close must not be guessed at read/start time"
    );
    sink(
        &k,
        "urn:iki:ledger:backfill-closed-at",
        &[("content", &a), ("comment", comment)],
    );
    sink(&k, "urn:iki:ledger:backfill-closed-at", &[("content", &b)]);
    let at = close_time(&k, &a);
    assert!(at.contains("2026-09-16") && at.contains("backfill"), "{at}");
    let at_b = close_time(&k, &b);
    assert!(
        at_b.contains("2026-09-17") && at_b.contains("backfill"),
        "{at_b}"
    );
    assert_eq!(
        source(&k, "urn:iki:ledger:counts", &[]),
        "date,open,closed\n2026-09-15,2,0\n2026-09-16,1,1\n2026-09-17,0,2\n"
    );
    let turtle = source(&k, "urn:iki:ledger:counts", &[("as", "text/turtle")]);
    assert!(
        turtle.contains("closedAtSource") && turtle.contains("backfill"),
        "the series must expose approximate history: {turtle}"
    );
    // Rerunning the one-time migration cannot overwrite the chosen estimate.
    sink(&k, "urn:iki:ledger:backfill-closed-at", &[("content", &a)]);
    assert_eq!(close_time(&k, &a), at);
    day(&clock, 3);
    sink(&k, "urn:iki:ledger:reopen", &[("item", &a)]);
    assert!(!close_time(&k, &a).contains("backfill"));
    let events = select(&k, "SELECT ?s WHERE { GRAPH <urn:iki:ledger:graph:default> { ?s <https://ikigai-rs.dev/ns/ledger#closedAtSource> \"backfill\" } }");
    assert!(
        events.contains(":status:"),
        "backfill provenance survives reopen: {events}"
    );
    assert!(source(&k, "urn:iki:ledger:counts", &[]).ends_with("2026-09-18,1,1\n"));
}

#[derive(Default)]
struct Trace(std::sync::Mutex<Vec<ikigai_core::TraceEvent>>);
impl ikigai_core::Tracer for Trace {
    fn record(&self, event: ikigai_core::TraceEvent) {
        self.0.lock().unwrap().push(event);
    }
}
#[test]
fn second_read_is_a_cache_hit_writes_invalidate_and_midnight_extends_series() {
    let (k, clock) = dated_kernel();
    let item = append_iri(&k, "Cache me", &[]);
    let request = || {
        ikigai_core::Request::new(
            Verb::Source,
            ikigai_core::Iri::parse("urn:iki:ledger:counts").unwrap(),
        )
    };
    let trace = Arc::new(Trace::default());
    let first =
        futures::executor::block_on(k.issue_traced(request(), &Capability::root(), trace.clone()))
            .unwrap();
    trace.0.lock().unwrap().clear();
    let second =
        futures::executor::block_on(k.issue_traced(request(), &Capability::root(), trace.clone()))
            .unwrap();
    assert_eq!(first.bytes, second.bytes);
    let events = trace.0.lock().unwrap();
    assert!(
        events.iter().any(|e| e.cache_hit),
        "second read must be served from cache: {events:?}"
    );
    assert_eq!(events.len(), 1, "a hit must issue no store sub-requests");
    drop(events);
    sink(&k, "urn:iki:ledger:close", &[("item", &item)]);
    assert!(source(&k, "urn:iki:ledger:counts", &[]).ends_with(",0,1\n"));
    day(&clock, 1);
    assert!(source(&k, "urn:iki:ledger:counts", &[]).ends_with("2026-09-16,0,1\n"));
}

#[test]
fn frozen_clock_transitions_and_reclosing_preserve_history() {
    let (k, clock) = dated_kernel();
    let item = append_iri(&k, "Frozen clock", &[]);
    sink(&k, "urn:iki:ledger:close", &[("item", &item)]);
    sink(&k, "urn:iki:ledger:reopen", &[("item", &item)]);
    sink(&k, "urn:iki:ledger:close", &[("item", &item)]);
    assert!(source(&k, "urn:iki:ledger:counts", &[]).ends_with("2026-09-15,0,1\n"));
    day(&clock, 1);
    sink(
        &k,
        "urn:iki:ledger:close",
        &[("item", &item), ("reason", "wontfix")],
    );
    assert!(close_time(&k, &item).contains("2026-09-15"));
    sink(&k, "urn:iki:ledger:reopen", &[("item", &item)]);
    assert_eq!(
        source(&k, "urn:iki:ledger:counts", &[]),
        "date,open,closed\n2026-09-15,0,1\n2026-09-16,1,0\n"
    );
}

#[test]
fn named_ledgers_are_isolated_and_imported_close_survives_reopen() {
    let (k, clock) = dated_kernel();
    let item = append_iri(&k, "Imported", &[]);
    sink(
        &k,
        "urn:iki:ledger:acme:append",
        &[("content", "Other ledger")],
    );
    day(&clock, 1);
    legacy_close(&k, &item);
    update(&k, &format!("INSERT DATA {{ GRAPH <urn:iki:ledger:graph:default> {{ <{item}> <https://ikigai-rs.dev/ns/ledger#closedAt> \"2026-09-16T00:00:00Z\"^^<http://www.w3.org/2001/XMLSchema#dateTime> }} }}"));
    day(&clock, 2);
    sink(&k, "urn:iki:ledger:reopen", &[("item", &item)]);
    assert_eq!(
        source(&k, "urn:iki:ledger:counts", &[]),
        "date,open,closed\n2026-09-15,1,0\n2026-09-16,0,1\n2026-09-17,1,0\n"
    );
    assert_eq!(
        source(&k, "urn:iki:ledger:default:counts", &[]),
        source(&k, "urn:iki:ledger:counts", &[])
    );
    assert_eq!(
        source(&k, "urn:iki:ledger:acme:counts", &[]),
        "date,open,closed\n2026-09-15,1,0\n2026-09-16,1,0\n2026-09-17,1,0\n"
    );
    let reader = Capability::scoped([
        "urn:cap:ledger:read:default".to_string(),
        graph_read("default"),
    ]);
    assert!(matches!(
        try_as(&k, &reader, Verb::Source, "urn:iki:ledger:acme:counts", &[]),
        Err(ikigai_core::Error::Denied(_))
    ));
    delete(&k, &item, &[]);
    assert_eq!(
        source(&k, "urn:iki:ledger:counts", &[]),
        "date,open,closed\n"
    );
    let events = select(&k, "SELECT ?event WHERE { GRAPH <urn:iki:ledger:graph:default> { ?event a <https://ikigai-rs.dev/ns/ledger#StatusEvent> } }");
    assert!(
        !events.contains(":status:"),
        "delete must take events too: {events}"
    );
}

#[test]
fn backfill_refuses_wrong_comment_and_readers_cannot_migrate() {
    let k = kernel();
    let a = append_iri(&k, "Legacy", &[]);
    let b = append_iri(&k, "Another item", &[]);
    let note = sink(
        &k,
        "urn:iki:ledger:comment",
        &[
            ("item", &b),
            ("content", "Unrelated"),
            ("as", "application/json"),
        ],
    );
    let note: serde_json::Value = serde_json::from_str(&note).unwrap();
    let comment = note["comment"]["id"].as_str().unwrap();
    legacy_close(&k, &a);
    assert!(try_verb(
        &k,
        Verb::Sink,
        "urn:iki:ledger:backfill-closed-at",
        &[("content", &a), ("comment", comment)]
    )
    .is_err());
    assert!(!close_time(&k, &a).contains("backfill"));
    let reader = Capability::scoped([
        "urn:cap:ledger:read:default".to_string(),
        graph_read("default"),
        graph_write("default"),
    ]);
    assert!(matches!(
        try_as(
            &k,
            &reader,
            Verb::Sink,
            "urn:iki:ledger:backfill-closed-at",
            &[("content", &a)]
        ),
        Err(ikigai_core::Error::Denied(_))
    ));
    assert!(try_verb(
        &k,
        Verb::Sink,
        "urn:iki:ledger:backfill-closed-at",
        &[("content", &b)]
    )
    .is_err());
}

#[test]
fn shared_properties_do_not_infer_items_comments_or_ledgers() {
    let vocabulary: Vec<_> = oxrdfio::RdfParser::from_format(oxrdfio::RdfFormat::Turtle)
        .for_reader(ikigai_ledger::VOCABULARY_TTL.as_bytes())
        .collect::<Result<_, _>>()
        .unwrap();
    for shared in ["status", "onItem", "openCount"] {
        let property = format!("<https://ikigai-rs.dev/ns/ledger#{shared}>");
        assert!(
            !vocabulary.iter().any(|q| q.subject.to_string() == property
                && q.predicate.as_str() == "http://www.w3.org/2000/01/rdf-schema#domain"),
            "{shared} is shared by events or observations: a domain would misclassify them"
        );
    }
}

#[test]
fn bulk_backfill_dates_every_legacy_close_once() {
    let (k, clock) = dated_kernel();
    let items: Vec<_> = (0..4)
        .map(|n| append_iri(&k, &format!("Legacy {n}"), &[]))
        .collect();
    let open = append_iri(&k, "Still open", &[]);
    day(&clock, 1);
    for item in &items {
        legacy_close(&k, item);
        sink(&k, item, &[("content", "Updated legacy")]);
    }
    sink(
        &k,
        "urn:iki:ledger:backfill-closed-at",
        &[("content", &items[0])],
    );
    let preserved = close_time(&k, &items[0]);
    let updated = select(&k, "SELECT ?item ?at WHERE { GRAPH <urn:iki:ledger:graph:default> { ?item <http://purl.org/dc/terms/modified> ?at } } ORDER BY ?item");
    assert!(try_verb(&k, Verb::Source, "urn:iki:ledger:counts", &[]).is_err());
    let reader = Capability::scoped([
        "urn:cap:ledger:read:default".to_string(),
        graph_read("default"),
        graph_write("default"),
    ]);
    assert!(matches!(
        try_as(
            &k,
            &reader,
            Verb::Sink,
            "urn:iki:ledger:backfill-closed-at",
            &[("content", "all")]
        ),
        Err(ikigai_core::Error::Denied(_))
    ));
    assert_eq!(
        sink(
            &k,
            "urn:iki:ledger:backfill-closed-at",
            &[("content", "all")]
        ),
        "backfilled 3 items\n"
    );
    for item in &items {
        let at = close_time(&k, item);
        assert!(at.contains("2026-09-16") && at.contains("backfill"), "{at}");
    }
    assert_eq!(close_time(&k, &items[0]), preserved);
    assert_eq!(select(&k, "SELECT ?item ?at WHERE { GRAPH <urn:iki:ledger:graph:default> { ?item <http://purl.org/dc/terms/modified> ?at } } ORDER BY ?item"), updated);
    assert!(!close_time(&k, &open).contains("backfill"));
    assert_eq!(
        source(&k, "urn:iki:ledger:counts", &[]),
        "date,open,closed\n2026-09-15,5,0\n2026-09-16,1,4\n"
    );
    let before = select(&k, "SELECT ?s ?p ?o WHERE { GRAPH <urn:iki:ledger:graph:default> { ?s ?p ?o } } ORDER BY ?s ?p ?o");
    assert_eq!(
        sink(
            &k,
            "urn:iki:ledger:backfill-closed-at",
            &[("content", "all")]
        ),
        "backfilled 0 items (unchanged)\n"
    );
    assert_eq!(select(&k, "SELECT ?s ?p ?o WHERE { GRAPH <urn:iki:ledger:graph:default> { ?s ?p ?o } } ORDER BY ?s ?p ?o"), before);
    day(&clock, 2);
    let ttl = source(&k, "urn:iki:ledger:counts", &[("as", "text/turtle")]);
    let quads: Vec<_> = oxrdfio::RdfParser::from_format(oxrdfio::RdfFormat::Turtle)
        .for_reader(ttl.as_bytes())
        .collect::<Result<_, _>>()
        .unwrap();
    for date in ["2026-09-15", "2026-09-16", "2026-09-17"] {
        assert_eq!(
            quads.iter().any(|q| q.subject.to_string()
                == format!("<urn:iki:ledger:default:counts:{date}>")
                && q.predicate.as_str().ends_with("#closedAtSource")),
            date != "2026-09-17"
        );
    }
}

#[test]
fn bulk_backfill_is_ledger_local_and_rejects_ambiguous_options() {
    let k = kernel();
    let item = append_iri(&k, "Default legacy", &[]);
    legacy_close(&k, &item);
    sink(
        &k,
        "urn:iki:ledger:acme:append",
        &[("content", "Other legacy")],
    );
    sink(&k, "urn:iki:ledger:acme:close", &[("item", "acme#1")]);
    sink(&k, "urn:iki:store:graph-update", &[("graph", "urn:iki:ledger:graph:acme"), ("content", "DELETE WHERE { GRAPH <urn:iki:ledger:graph:acme> { ?item <https://ikigai-rs.dev/ns/ledger#closedAt> ?at } }")]);
    assert!(try_verb(
        &k,
        Verb::Sink,
        "urn:iki:ledger:backfill-closed-at",
        &[("content", "all"), ("comment", "urn:test:comment")]
    )
    .is_err());
    assert_eq!(
        sink(
            &k,
            "urn:iki:ledger:acme:backfill-closed-at",
            &[("content", "all")]
        ),
        "backfilled 1 items\n"
    );
    assert!(!close_time(&k, &item).contains("backfill"));
    assert!(try_verb(&k, Verb::Source, "urn:iki:ledger:counts", &[]).is_err());
    assert!(source(&k, "urn:iki:ledger:acme:counts", &[]).ends_with(",0,1\n"));
}

#[test]
fn bulk_backfill_validates_all_dates_before_writing() {
    let k = kernel();
    let a = append_iri(&k, "Valid legacy", &[]);
    let b = append_iri(&k, "Invalid legacy", &[]);
    legacy_close(&k, &a);
    legacy_close(&k, &b);
    update(&k, &format!("DELETE WHERE {{ GRAPH <urn:iki:ledger:graph:default> {{ <{b}> <http://purl.org/dc/terms/modified> ?at }} }}"));
    assert!(try_verb(
        &k,
        Verb::Sink,
        "urn:iki:ledger:backfill-closed-at",
        &[("content", "all")]
    )
    .is_err());
    assert!(!close_time(&k, &a).contains("backfill"));
    assert!(!close_time(&k, &b).contains("backfill"));
}

#[test]
fn bulk_backfill_handles_a_5000_item_ledger() {
    let k = kernel();
    let mut triples = String::new();
    for n in 1..=5000 {
        triples.push_str(&format!("<urn:iki:ledger:default:item:bulk{n:06}0000000000000> a <https://ikigai-rs.dev/ns/ledger#Item> ; <https://ikigai-rs.dev/ns/ledger#status> <https://ikigai-rs.dev/ns/ledger#closed> ; <http://purl.org/dc/terms/created> \"2026-09-15T00:00:00Z\"^^<http://www.w3.org/2001/XMLSchema#dateTime> ; <http://purl.org/dc/terms/modified> \"2026-09-15T01:00:00Z\"^^<http://www.w3.org/2001/XMLSchema#dateTime> .\n"));
        if n % 400 == 0 || n == 5000 {
            update(
                &k,
                &format!("INSERT DATA {{ GRAPH <urn:iki:ledger:graph:default> {{ {triples} }} }}"),
            );
            triples.clear();
        }
    }
    assert!(try_verb(&k, Verb::Source, "urn:iki:ledger:counts", &[]).is_err());
    assert_eq!(
        sink(
            &k,
            "urn:iki:ledger:backfill-closed-at",
            &[("content", "all")]
        ),
        "backfilled 5000 items\n"
    );
    assert_eq!(
        source(&k, "urn:iki:ledger:counts", &[]),
        "date,open,closed\n2026-09-15,0,5000\n"
    );
    assert_eq!(
        sink(
            &k,
            "urn:iki:ledger:backfill-closed-at",
            &[("content", "all")]
        ),
        "backfilled 0 items (unchanged)\n"
    );
}
