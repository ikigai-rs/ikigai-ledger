//! **An importer dates a link and a state move at its source** (ledger #2934).
//!
//! 0.18.0 lets an import principal replay an item's past with its source dates and keep the
//! source's `modified` (#2928): a comment's `created=` and a close's `closed-at=` move it only
//! forward. Two writes an importer replays could carry no date at all, the link Sink and the
//! state Sink, so each stamped the kernel's time and an imported item linked or moved after
//! filing ended at the import. Now both take `created=` from an import principal, under the
//! same rules: no later than now, no earlier than the item's filing, recorded on the event (and
//! on a link across ledgers, on the link), and `modified` moves only forward to it. Anyone else
//! sending `created=` is refused with a sentence.

mod common;

use std::sync::Arc;

use common::*;
use ikigai_core::{Capability, Error, Fallback, Kernel, Space, Verb};
use ikigai_ledger::SpaceConfig;
use ikigai_store::DurableStore;

const NS: &str = "https://ikigai-rs.dev/ns/ledger#";
const LEDGER: &str = "acme";
const OTHER: &str = "cli";
const PARTICIPANTS: &str = "urn:collab:test:participants";
const ADA: &str = "urn:collab:participant:ada";
const IMPORTER: &str = "urn:iki:gonk:client:importer";

fn at(ledger: &str, resource: &str) -> String {
    format!("urn:iki:ledger:{ledger}:{resource}")
}

/// A kernel whose ledgers read enrollment from [`PARTICIPANTS`], where the importer is an
/// import principal.
fn kernel() -> Kernel {
    let ledger = ikigai_ledger::space_with(SpaceConfig::default().participants(PARTICIPANTS));
    let space = Fallback::new(vec![
        Arc::new(ikigai_store::space(DurableStore::in_memory().unwrap())) as Arc<dyn Space>,
        Arc::new(ledger) as Arc<dyn Space>,
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
                "INSERT DATA {{ GRAPH <{PARTICIPANTS}> {{ \
                 <{IMPORTER}> a <{NS}Importer> . }} }}"
            ),
        )],
    );
    kernel
}

/// A writer of both ledgers, who may link `cli` items into `acme`, as a door mints it.
fn writer(principal: &str) -> Capability {
    let mut scopes = vec![format!("urn:cap:store:read:graph:{PARTICIPANTS}")];
    for ledger in [LEDGER, OTHER] {
        scopes.extend([
            format!("urn:cap:ledger:read:{ledger}"),
            format!("urn:cap:ledger:write:{ledger}"),
            graph_read(ledger),
            graph_write(ledger),
        ]);
    }
    scopes.push(format!("urn:cap:ledger:link-in:{LEDGER}"));
    Capability::scoped(scopes)
        .with_principal(principal)
        .unwrap()
}

/// File an item in `ledger` as the importer, with the source's dates; its IRI.
fn file(kernel: &Kernel, ledger: &str, title: &str, modified: &str) -> String {
    try_as(
        kernel,
        &writer(IMPORTER),
        Verb::Sink,
        &at(ledger, "append"),
        &[
            ("content", title),
            ("created", "2020-01-01T00:00:00Z"),
            ("modified", modified),
        ],
    )
    .unwrap()
    .split_whitespace()
    .nth(1)
    .expect("append answers with the number and the IRI")
    .to_string()
}

fn link(kernel: &Kernel, who: &str, ledger: &str, args: &[(&str, &str)]) -> Result<String, Error> {
    try_as(kernel, &writer(who), Verb::Sink, &at(ledger, "link"), args)
}

fn state(kernel: &Kernel, who: &str, item: &str, args: &[(&str, &str)]) -> Result<String, Error> {
    try_as(
        kernel,
        &writer(who),
        Verb::Sink,
        &at(LEDGER, &format!("item:{item}:state")),
        args,
    )
}

/// The item's JSON document.
fn item(kernel: &Kernel, ledger: &str, number: &str) -> serde_json::Value {
    serde_json::from_str(&source(
        kernel,
        &at(ledger, &format!("item:{number}")),
        &[("as", "application/json")],
    ))
    .expect("JSON")
}

fn modified(kernel: &Kernel, ledger: &str, number: &str) -> String {
    item(kernel, ledger, number)["item"]["modified"]
        .as_str()
        .unwrap()
        .to_string()
}

/// The item's history events of `kind`, as JSON.
fn events(kernel: &Kernel, ledger: &str, number: &str, kind: &str) -> Vec<serde_json::Value> {
    let doc: serde_json::Value = serde_json::from_str(&source(
        kernel,
        &at(ledger, &format!("item:{number}:history")),
        &[("as", "application/json")],
    ))
    .expect("JSON");
    doc["events"]
        .as_array()
        .expect("events")
        .iter()
        .filter(|event| event["kind"] == kind)
        .cloned()
        .collect()
}

fn has(text: &str, wanted: &str) {
    assert!(text.contains(wanted), "{wanted:?} is not in:\n{text}");
}

/// ★ The reproduction: an importer files an item with the source's `modified`, then replays a
/// link and a state move dated at the source. Until this change neither write took a date, so
/// each stamped the kernel's time and `modified` ended at the import.
#[test]
fn an_importers_past_link_and_state_move_leave_the_claimed_modified() {
    let kernel = kernel();
    file(&kernel, LEDGER, "Fix the widget", "2020-02-03T04:05:06Z");
    file(
        &kernel,
        LEDGER,
        "Document the widget",
        "2020-02-03T04:05:06Z",
    );
    link(
        &kernel,
        IMPORTER,
        LEDGER,
        &[
            ("item", "1"),
            ("content", "2"),
            ("type", "related"),
            ("created", "2020-01-10T00:00:00Z"),
        ],
    )
    .unwrap();
    assert_eq!(
        modified(&kernel, LEDGER, "1"),
        "2020-02-03T04:05:06.000Z",
        "a past link leaves the claimed modified"
    );
    state(
        &kernel,
        IMPORTER,
        "1",
        &[
            ("from", "filed"),
            ("to", "queued"),
            ("created", "2020-01-15T00:00:00Z"),
        ],
    )
    .unwrap();
    let doc = item(&kernel, LEDGER, "1");
    assert_eq!(doc["item"]["modified"], "2020-02-03T04:05:06.000Z");
    assert_eq!(
        doc["item"]["attribution"]["claimed_dates"],
        serde_json::json!(["created", "modified"])
    );
    has(
        &source(&kernel, &at(LEDGER, "item:1"), &[]),
        "  updated:  2020-02-03T04:05:06.000Z (claimed)\n",
    );

    // The events carry the claimed dates, and say when the importer recorded them.
    let linked = &events(&kernel, LEDGER, "1", "link")[0];
    assert_eq!(linked["at"], "2020-01-10T00:00:00.000Z");
    assert!(linked["committed"].as_str().unwrap().starts_with("2026-"));
    assert_eq!(
        linked["imported"],
        serde_json::Value::Null,
        "it is no history-only event"
    );
    let moved = &events(&kernel, LEDGER, "1", "state")[0];
    assert_eq!(moved["at"], "2020-01-15T00:00:00.000Z");
    assert!(moved["committed"].as_str().unwrap().starts_with("2026-"));
    assert!(
        moved["lifecycle_version"].is_string(),
        "a real state move keeps its lifecycle version: {moved}"
    );
    let history = source(&kernel, &at(LEDGER, "item:1:history"), &[]);
    has(&history, "2020-01-10T00:00:00.000Z (claimed)  link related");
    has(
        &history,
        "2020-01-15T00:00:00.000Z (claimed)  state filed to queued",
    );

    // The state says when the item entered it at the source.
    let doc: serde_json::Value = serde_json::from_str(&source(
        &kernel,
        &at(LEDGER, "item:1:state"),
        &[("as", "application/json")],
    ))
    .unwrap();
    assert_eq!(doc["since"], "2020-01-15T00:00:00.000Z");
}

/// A claimed date after the current `modified` moves it there, on both Sinks.
#[test]
fn a_dated_link_or_state_move_after_the_modified_moves_it_forward() {
    let kernel = kernel();
    file(&kernel, LEDGER, "Fix the widget", "2020-02-03T04:05:06Z");
    file(
        &kernel,
        LEDGER,
        "Document the widget",
        "2020-02-03T04:05:06Z",
    );
    link(
        &kernel,
        IMPORTER,
        LEDGER,
        &[
            ("item", "1"),
            ("content", "2"),
            ("type", "blocks"),
            ("created", "2020-03-01T00:00:00Z"),
        ],
    )
    .unwrap();
    assert_eq!(modified(&kernel, LEDGER, "1"), "2020-03-01T00:00:00.000Z");
    state(
        &kernel,
        IMPORTER,
        "1",
        &[
            ("from", "filed"),
            ("to", "queued"),
            ("created", "2020-04-01T00:00:00Z"),
        ],
    )
    .unwrap();
    assert_eq!(modified(&kernel, LEDGER, "1"), "2020-04-01T00:00:00.000Z");
    // An earlier move after that leaves it.
    state(
        &kernel,
        IMPORTER,
        "1",
        &[
            ("from", "queued"),
            ("to", "filed"),
            ("created", "2020-03-15T00:00:00Z"),
        ],
    )
    .unwrap();
    assert_eq!(modified(&kernel, LEDGER, "1"), "2020-04-01T00:00:00.000Z");
}

/// Without `created=` both Sinks stamp the kernel's time, the importer's writes included.
#[test]
fn an_undated_link_or_state_move_still_stamps_now() {
    let kernel = kernel();
    file(&kernel, LEDGER, "Fix the widget", "2020-02-03T04:05:06Z");
    file(
        &kernel,
        LEDGER,
        "Document the widget",
        "2020-02-03T04:05:06Z",
    );
    link(
        &kernel,
        IMPORTER,
        LEDGER,
        &[("item", "1"), ("content", "2")],
    )
    .unwrap();
    assert!(modified(&kernel, LEDGER, "1").starts_with("2026-"));
    state(
        &kernel,
        IMPORTER,
        "2",
        &[("from", "filed"), ("to", "queued")],
    )
    .unwrap();
    assert!(modified(&kernel, LEDGER, "2").starts_with("2026-"));
    let moved = &events(&kernel, LEDGER, "2", "state")[0];
    assert!(moved["at"].as_str().unwrap().starts_with("2026-"));
    assert_eq!(moved["committed"], serde_json::Value::Null);
}

/// ★ A link across ledgers: the claimed date is on the link and its event, the subject keeps
/// its claimed `modified`, and the dated event still moves the mirror's version, so the mirror
/// lands and a later close carries over it.
#[test]
fn a_dated_link_across_ledgers_keeps_the_claimed_modified_and_its_mirror() {
    let kernel = kernel();
    let subject = file(&kernel, OTHER, "Ship the flag", "2020-02-03T04:05:06Z");
    let target = file(&kernel, LEDGER, "Document the flag", "2020-02-03T04:05:06Z");
    link(
        &kernel,
        IMPORTER,
        OTHER,
        &[
            ("item", "1"),
            ("content", &target),
            ("type", "blocks"),
            ("created", "2020-01-10T00:00:00Z"),
        ],
    )
    .unwrap();
    assert_eq!(modified(&kernel, OTHER, "1"), "2020-02-03T04:05:06.000Z");
    let linked = &events(&kernel, OTHER, "1", "link")[0];
    assert_eq!(linked["at"], "2020-01-10T00:00:00.000Z");

    // The link node carries the claimed date, marked as claimed.
    let rows = select(
        &kernel,
        &format!(
            "SELECT ?created ?source WHERE {{ GRAPH <urn:iki:ledger:graph:{OTHER}> {{ \
             ?link a <{NS}Link> ; <{NS}onItem> <{subject}> ; \
             <http://purl.org/dc/terms/created> ?created ; <{NS}createdSource> ?source }} }}"
        ),
    );
    has(&rows, "2020-01-10T00:00:00");
    has(&rows, "\"import\"");

    // The mirror landed, at a version the dated event counted.
    let mirror = |kernel: &Kernel| {
        select(
            kernel,
            &format!(
                "SELECT ?open ?version WHERE {{ GRAPH <urn:iki:ledger:graph:{LEDGER}> {{ \
                 ?m a <{NS}Mirror> ; <{NS}onItem> <{target}> ; <{NS}subjectOpen> ?open ; \
                 <{NS}linkPresent> true ; <{NS}mirrorVersion> ?version }} }}"
            ),
        )
    };
    let before = mirror(&kernel);
    has(&before, "\"true\"");
    has(&before, "\"1\"");
    let next = source(&kernel, &at(LEDGER, "next"), &[]);
    assert!(!next.contains("Document the flag"), "blocked: {next}");

    try_as(
        &kernel,
        &writer(IMPORTER),
        Verb::Sink,
        &at(OTHER, "close"),
        &[("item", "1"), ("closed-at", "2020-01-20T00:00:00Z")],
    )
    .unwrap();
    let after = mirror(&kernel);
    has(&after, "\"false\"");
    has(&after, "\"2\"");
    assert_eq!(modified(&kernel, OTHER, "1"), "2020-02-03T04:05:06.000Z");
}

/// Anyone but an import principal sending `created=` is refused with the reason, and nothing
/// is written.
#[test]
fn a_date_on_a_link_or_state_move_from_anyone_else_is_refused() {
    let kernel = kernel();
    file(&kernel, LEDGER, "Fix the widget", "2020-02-03T04:05:06Z");
    let target = file(
        &kernel,
        OTHER,
        "Document the widget",
        "2020-02-03T04:05:06Z",
    );
    file(&kernel, LEDGER, "Document it here", "2020-02-03T04:05:06Z");
    for content in ["2", target.as_str()] {
        match link(
            &kernel,
            ADA,
            LEDGER,
            &[
                ("item", "1"),
                ("content", content),
                ("type", "related"),
                ("created", "2020-01-10T00:00:00Z"),
            ],
        ) {
            Err(Error::Denied(message)) => {
                has(&message, "only an import or sync principal");
                has(
                    &message,
                    &format!("{ADA} is not an import or sync principal"),
                );
                has(&message, "Send it without `created`");
            }
            other => panic!("Ada's dated link was not refused: {other:?}"),
        }
    }
    match state(
        &kernel,
        ADA,
        "1",
        &[
            ("from", "filed"),
            ("to", "queued"),
            ("created", "2020-01-10T00:00:00Z"),
        ],
    ) {
        Err(Error::Denied(message)) => has(&message, "only an import or sync principal"),
        other => panic!("Ada's dated state move was not refused: {other:?}"),
    }
    assert!(events(&kernel, LEDGER, "1", "link").is_empty());
    assert!(events(&kernel, LEDGER, "1", "state").is_empty());
    assert_eq!(modified(&kernel, LEDGER, "1"), "2020-02-03T04:05:06.000Z");
}

/// The date rules are `closed-at=`'s: an `xsd:dateTime`, no later than now, no earlier than the
/// item's filing.
#[test]
fn an_importers_link_or_state_date_is_held_to_the_date_rules() {
    let kernel = kernel();
    file(&kernel, LEDGER, "Fix the widget", "2020-02-03T04:05:06Z");
    file(
        &kernel,
        LEDGER,
        "Document the widget",
        "2020-02-03T04:05:06Z",
    );
    for (date, why) in [
        ("2020-01-10", "is not an xsd:dateTime"),
        ("2999-01-01T00:00:00Z", "cannot be later than its import"),
        ("2019-06-01T00:00:00Z", "before the item was filed"),
    ] {
        for result in [
            link(
                &kernel,
                IMPORTER,
                LEDGER,
                &[("item", "1"), ("content", "2"), ("created", date)],
            ),
            state(
                &kernel,
                IMPORTER,
                "1",
                &[("from", "filed"), ("to", "queued"), ("created", date)],
            ),
        ] {
            match result {
                Err(Error::InvalidArgument { name, detail }) => {
                    assert_eq!(name, "created");
                    has(&detail, why);
                }
                other => panic!("{date} was not refused: {other:?}"),
            }
        }
    }
    assert!(events(&kernel, LEDGER, "1", "link").is_empty());
    assert!(events(&kernel, LEDGER, "1", "state").is_empty());
}

/// `next` under `priority-recency` ranks an imported set as the source did, however its links
/// and moves were replayed.
#[test]
fn next_ranks_an_imported_set_with_links_and_moves_as_the_source_did() {
    let kernel = kernel();
    file(
        &kernel,
        LEDGER,
        "Older at the source",
        "2020-01-02T00:00:00Z",
    );
    file(
        &kernel,
        LEDGER,
        "Newer at the source",
        "2020-06-01T00:00:00Z",
    );
    file(
        &kernel,
        LEDGER,
        "Related at the source",
        "2020-01-01T00:00:00Z",
    );
    link(
        &kernel,
        IMPORTER,
        LEDGER,
        &[
            ("item", "1"),
            ("content", "3"),
            ("type", "related"),
            ("created", "2020-01-02T00:00:00Z"),
        ],
    )
    .unwrap();
    state(
        &kernel,
        IMPORTER,
        "1",
        &[
            ("from", "filed"),
            ("to", "queued"),
            ("created", "2020-01-02T00:00:00Z"),
        ],
    )
    .unwrap();
    let next = source(&kernel, &at(LEDGER, "next"), &[]);
    let newer = next.find("Newer at the source").expect(&next);
    let older = next.find("Older at the source").expect(&next);
    assert!(newer < older, "the source's order: {next}");
}
