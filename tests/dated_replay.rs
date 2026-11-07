//! **The rest of an importer's replay can claim a date** (ledger #2939).
//!
//! 0.18.0 let an import principal date a comment, a close (#2928), a link and a state move
//! (#2934) at the source, and keep the source's `modified`. Every other write an importer
//! replays still stamped the kernel's time, so an imported item labeled, deferred, pointed at a
//! resource, edited, reopened or unlinked after filing ended at the import. Now each of those
//! takes `created=` from an import principal under the same rule (`resolve_claimed`): no later
//! than now, no earlier than the item's filing, on the event, and `modified` moves only forward
//! to it. Anyone else sending `created=` is refused with the sentence the link Sink gives, and an
//! unlink no longer ignores one. A dated link is also bounded by its TARGET's filing.
//!
//! One test per Sink holds the three halves: undated stamps now, an importer's date lands and
//! moves `modified` only forward, and anyone else is refused.

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
/// Every item is filed with these source dates.
const FILED: &str = "2020-01-01T00:00:00Z";
const MODIFIED: &str = "2020-02-01T00:00:00.000Z";
/// A replayed write dated before the item's `modified`, and one after it.
const EARLIER: &str = "2020-01-15T00:00:00Z";
const LATER: &str = "2020-03-01T00:00:00Z";

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

/// A writer (which covers close and about) and linker of both ledgers, as a door mints it for one principal.
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
fn file(kernel: &Kernel, ledger: &str, title: &str) -> String {
    file_at(kernel, ledger, title, FILED, MODIFIED)
}

fn file_at(kernel: &Kernel, ledger: &str, title: &str, created: &str, modified: &str) -> String {
    try_as(
        kernel,
        &writer(IMPORTER),
        Verb::Sink,
        &at(ledger, "append"),
        &[
            ("content", title),
            ("created", created),
            ("modified", modified),
        ],
    )
    .unwrap()
    .split_whitespace()
    .nth(1)
    .expect("append answers with the number and the IRI")
    .to_string()
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

fn modified(kernel: &Kernel, number: &str) -> String {
    item(kernel, LEDGER, number)["item"]["modified"]
        .as_str()
        .unwrap()
        .to_string()
}

/// The item's history events of `kind`, as JSON.
fn events(kernel: &Kernel, number: &str, kind: &str) -> Vec<serde_json::Value> {
    let doc: serde_json::Value = serde_json::from_str(&source(
        kernel,
        &at(LEDGER, &format!("item:{number}:history")),
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

/// One replayed write: its verb, the resource it is sent to, its arguments for item `number`
/// (without `created`), and the history kind its event carries.
struct Write {
    verb: Verb,
    resource: fn(&str) -> String,
    args: fn(&str) -> Vec<(&'static str, String)>,
    kind: &'static str,
}

impl Write {
    fn send(
        &self,
        kernel: &Kernel,
        who: &str,
        number: &str,
        created: Option<&str>,
    ) -> Result<String, Error> {
        let mut args = (self.args)(number);
        if let Some(created) = created {
            args.push(("created", created.to_string()));
        }
        let args: Vec<(&str, &str)> = args.iter().map(|(k, v)| (*k, v.as_str())).collect();
        try_as(
            kernel,
            &writer(who),
            self.verb,
            &(self.resource)(number),
            &args,
        )
    }
}

/// What each write needs in place before it can change anything: a label to remove, a
/// deferral to resume, an about to drop, a close to reopen, a link to take back. Done
/// undated, as the importer, so the item's `modified` is the kernel's after it; the test then
/// files its own item for the dated half.
fn prepare(kernel: &Kernel, write: &Write, number: &str) {
    let undo = match write.kind {
        "unlabel" => Some(Write {
            verb: Verb::Sink,
            ..label()
        }),
        "resume" => Some(Write {
            verb: Verb::Sink,
            ..defer()
        }),
        "unabout" => Some(Write {
            verb: Verb::Sink,
            ..about()
        }),
        "unlink" => Some(Write {
            verb: Verb::Sink,
            ..unlink()
        }),
        "reopened" => Some(Write {
            verb: Verb::Sink,
            resource: |_| at(LEDGER, "close"),
            args: |n| vec![("item", n.to_string())],
            kind: "closed",
        }),
        _ => None,
    };
    if let Some(undo) = undo {
        undo.send(kernel, IMPORTER, number, None).unwrap();
    }
}

fn label() -> Write {
    Write {
        verb: Verb::Sink,
        resource: |_| at(LEDGER, "label"),
        args: |n| vec![("item", n.to_string()), ("content", "triage".to_string())],
        kind: "label",
    }
}

fn defer() -> Write {
    Write {
        verb: Verb::Sink,
        resource: |_| at(LEDGER, "defer"),
        args: |n| vec![("item", n.to_string())],
        kind: "defer",
    }
}

fn about() -> Write {
    Write {
        verb: Verb::Sink,
        resource: |n| at(LEDGER, &format!("item:{n}:about")),
        args: |_| vec![("content", "urn:example:widget".to_string())],
        kind: "about",
    }
}

fn edit() -> Write {
    Write {
        verb: Verb::Sink,
        resource: |n| at(LEDGER, &format!("item:{n}")),
        args: |_| vec![("priority", "2".to_string())],
        kind: "edit",
    }
}

fn reopen() -> Write {
    Write {
        verb: Verb::Sink,
        resource: |_| at(LEDGER, "reopen"),
        args: |n| vec![("item", n.to_string())],
        kind: "reopened",
    }
}

/// A link to item 1 from item `n`; `unlink` is its Delete.
fn unlink() -> Write {
    Write {
        verb: Verb::Delete,
        resource: |_| at(LEDGER, "link"),
        args: |n| {
            vec![
                ("item", n.to_string()),
                ("content", "1".to_string()),
                ("type", "related".to_string()),
            ]
        },
        kind: "unlink",
    }
}

/// The three halves for one write.
fn holds(write: Write) {
    let kernel = kernel();
    // Item 1 is the anchor a link points at; each half files its own item after it.
    file(&kernel, LEDGER, "The anchor");

    // Undated: the importer's write stamps the kernel's time, and its event says nothing more.
    file(&kernel, LEDGER, "Undated");
    prepare(&kernel, &write, "2");
    write.send(&kernel, IMPORTER, "2", None).unwrap();
    assert!(
        modified(&kernel, "2").starts_with("2026-"),
        "{}: an undated write stamps now",
        write.kind
    );
    let event = events(&kernel, "2", write.kind).pop().expect("an event");
    assert!(
        event["at"].as_str().unwrap().starts_with("2026-"),
        "{}: {event}",
        write.kind
    );

    // An importer's date before `modified` lands on the event and leaves `modified`.
    file(&kernel, LEDGER, "Dated earlier");
    prepare_dated(&kernel, &write, "3");
    write.send(&kernel, IMPORTER, "3", Some(EARLIER)).unwrap();
    let doc = item(&kernel, LEDGER, "3");
    assert_eq!(doc["item"]["modified"], MODIFIED, "{}: {doc}", write.kind);
    assert_eq!(
        doc["item"]["attribution"]["claimed_dates"],
        serde_json::json!(["created", "modified"]),
        "{}",
        write.kind
    );
    let event = events(&kernel, "3", write.kind).pop().expect("an event");
    assert_eq!(
        event["at"], "2020-01-15T00:00:00.000Z",
        "{}: {event}",
        write.kind
    );

    // A date after it moves `modified` forward, still claimed.
    file(&kernel, LEDGER, "Dated later");
    prepare_dated(&kernel, &write, "4");
    write.send(&kernel, IMPORTER, "4", Some(LATER)).unwrap();
    let doc = item(&kernel, LEDGER, "4");
    assert_eq!(
        doc["item"]["modified"], "2020-03-01T00:00:00.000Z",
        "{}: {doc}",
        write.kind
    );
    assert_eq!(
        doc["item"]["attribution"]["claimed_dates"],
        serde_json::json!(["created", "modified"]),
        "{}",
        write.kind
    );

    // Anyone else sending `created=` is refused with the reason, and nothing is written.
    file(&kernel, LEDGER, "Not an importer");
    prepare_dated(&kernel, &write, "5");
    let before = events(&kernel, "5", write.kind).len();
    match write.send(&kernel, ADA, "5", Some(EARLIER)) {
        Err(Error::Denied(message)) => {
            has(&message, "only an import or sync principal");
            has(
                &message,
                &format!("{ADA} is not an import or sync principal"),
            );
            has(&message, "Send it without `created`");
        }
        other => panic!(
            "{}: Ada's dated write was not refused: {other:?}",
            write.kind
        ),
    }
    assert_eq!(
        events(&kernel, "5", write.kind).len(),
        before,
        "{}",
        write.kind
    );
    assert_eq!(modified(&kernel, "5"), MODIFIED, "{}", write.kind);

    // And the date rules are the link's: none before the item's filing.
    match write.send(&kernel, IMPORTER, "5", Some("2019-06-01T00:00:00Z")) {
        Err(Error::InvalidArgument { name, detail }) => {
            assert_eq!(name, "created");
            has(&detail, "before the item was filed");
        }
        other => panic!(
            "{}: a date before filing was not refused: {other:?}",
            write.kind
        ),
    }
}

/// The dated halves need the write's precondition WITHOUT moving `modified` off the source's
/// value, or "moves only forward" could not be seen: so the precondition is dated too, at the
/// filing.
fn prepare_dated(kernel: &Kernel, write: &Write, number: &str) {
    let undo = match write.kind {
        "unlabel" => Some(Write {
            verb: Verb::Sink,
            ..label()
        }),
        "resume" => Some(Write {
            verb: Verb::Sink,
            ..defer()
        }),
        "unabout" => Some(Write {
            verb: Verb::Sink,
            ..about()
        }),
        "unlink" => Some(Write {
            verb: Verb::Sink,
            ..unlink()
        }),
        _ => None,
    };
    if let Some(undo) = undo {
        undo.send(kernel, IMPORTER, number, Some("2020-01-02T00:00:00Z"))
            .unwrap();
    }
    if write.kind == "reopened" {
        try_as(
            kernel,
            &writer(IMPORTER),
            Verb::Sink,
            &at(LEDGER, "close"),
            &[("item", number), ("closed-at", "2020-01-02T00:00:00Z")],
        )
        .unwrap();
    }
}

#[test]
fn a_label_takes_an_importers_date() {
    holds(label());
}

#[test]
fn an_unlabel_takes_an_importers_date() {
    holds(Write {
        verb: Verb::Delete,
        kind: "unlabel",
        ..label()
    });
}

#[test]
fn a_defer_takes_an_importers_date() {
    holds(defer());
}

#[test]
fn a_resume_takes_an_importers_date() {
    holds(Write {
        verb: Verb::Delete,
        kind: "resume",
        ..defer()
    });
}

#[test]
fn an_about_takes_an_importers_date() {
    holds(about());
}

#[test]
fn an_unabout_takes_an_importers_date() {
    holds(Write {
        verb: Verb::Delete,
        kind: "unabout",
        ..about()
    });
}

#[test]
fn an_item_edit_takes_an_importers_date() {
    holds(edit());
}

#[test]
fn a_reopen_takes_an_importers_date() {
    holds(reopen());
}

/// An unlink took no date and ignored a `created=` from anyone (ledger #2939): now an importer's
/// lands, and anyone else's is refused rather than ignored.
#[test]
fn an_unlink_takes_an_importers_date() {
    holds(unlink());
}

// ------------------------------------------------- a dated link and its target's filing

/// ★ A dated link is bounded below by the LATER of its two ends' filings (ledger #2939): until
/// 0.18.0 only the subject's was checked, so a link could be dated before its target existed.
/// Within a ledger and across ledgers alike; the message names the end that was filed later.
#[test]
fn a_dated_link_cannot_come_before_its_target_was_filed() {
    let kernel = kernel();
    file(&kernel, LEDGER, "The subject");
    let here = file_at(
        &kernel,
        LEDGER,
        "Filed later here",
        "2020-06-01T00:00:00Z",
        "2020-06-01T00:00:00Z",
    );
    file(&kernel, OTHER, "The subject across");
    let there = file_at(
        &kernel,
        LEDGER,
        "Filed later there",
        "2020-06-01T00:00:00Z",
        "2020-06-01T00:00:00Z",
    );
    let link = |ledger: &str, number: &str, target: &str, created: &str| {
        try_as(
            &kernel,
            &writer(IMPORTER),
            Verb::Sink,
            &at(ledger, "link"),
            &[
                ("item", number),
                ("content", target),
                ("type", "blocks"),
                ("created", created),
            ],
        )
    };
    for (ledger, target) in [(LEDGER, &here), (OTHER, &there)] {
        match link(ledger, "1", target, "2020-03-01T00:00:00Z") {
            Err(Error::InvalidArgument { name, detail }) => {
                assert_eq!(name, "created");
                has(&detail, "before the link's target");
                has(&detail, target);
                has(&detail, "2020-06-01T00:00:00.000Z");
            }
            other => panic!("a link dated before its target was not refused: {other:?}"),
        }
    }
    assert!(events(&kernel, "1", "link").is_empty());
    // On or after both filings, it lands.
    link(LEDGER, "1", &here, "2020-06-01T00:00:00Z").unwrap();
    link(OTHER, "1", &there, "2020-07-01T00:00:00Z").unwrap();
}

/// A link across ledgers reads its target under the caller's capability to make the link at
/// all (the target must be a live item), so a target the caller cannot read refuses the link,
/// at that read, before any date is weighed. The bound never has to fall back to the subject's
/// filing alone: no link is made without reading the target's.
#[test]
fn a_link_to_a_target_the_caller_cannot_read_is_not_made_dated_or_not() {
    let kernel = kernel();
    file(&kernel, OTHER, "The subject across");
    let target = file(&kernel, LEDGER, "Unreadable to the linker");
    // `link-in` on acme, and nothing else of acme's: a `related` link needs no mirror grants.
    let blind = Capability::scoped(vec![
        format!("urn:cap:store:read:graph:{PARTICIPANTS}"),
        format!("urn:cap:ledger:read:{OTHER}"),
        format!("urn:cap:ledger:write:{OTHER}"),
        graph_read(OTHER),
        graph_write(OTHER),
        format!("urn:cap:ledger:link-in:{LEDGER}"),
    ])
    .with_principal(IMPORTER)
    .unwrap();
    for created in [None, Some("2020-03-01T00:00:00Z")] {
        let mut args = vec![
            ("item", "1"),
            ("content", target.as_str()),
            ("type", "related"),
        ];
        if let Some(created) = created {
            args.push(("created", created));
        }
        let result = try_as(&kernel, &blind, Verb::Sink, &at(OTHER, "link"), &args);
        match result {
            Err(Error::Denied(message)) => {
                has(
                    &message,
                    &format!("reading <urn:iki:ledger:graph:{LEDGER}>"),
                );
            }
            other => panic!("{created:?}: a link to an unreadable target: {other:?}"),
        }
    }
}

/// An unlink across ledgers takes an importer's date like a link does, and the mirror it leaves
/// is undated: it records the subject's open bit, not when the edge went away.
#[test]
fn a_dated_unlink_across_ledgers_lands_on_its_event() {
    let kernel = kernel();
    file(&kernel, OTHER, "The subject across");
    let target = file(&kernel, LEDGER, "The target");
    let send = |verb: Verb, who: &str, created: &str| {
        try_as(
            &kernel,
            &writer(who),
            verb,
            &at(OTHER, "link"),
            &[
                ("item", "1"),
                ("content", target.as_str()),
                ("type", "blocks"),
                ("created", created),
            ],
        )
    };
    send(Verb::Sink, IMPORTER, "2020-01-02T00:00:00Z").unwrap();
    match send(Verb::Delete, ADA, "2020-01-10T00:00:00Z") {
        Err(Error::Denied(message)) => has(&message, "only an import or sync principal"),
        other => panic!("Ada's dated unlink across was not refused: {other:?}"),
    }
    send(Verb::Delete, IMPORTER, "2020-01-10T00:00:00Z").unwrap();
    let doc: serde_json::Value = serde_json::from_str(&source(
        &kernel,
        &at(OTHER, "item:1:history"),
        &[("as", "application/json")],
    ))
    .unwrap();
    let unlinked: Vec<&serde_json::Value> = doc["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "unlink")
        .collect();
    assert_eq!(unlinked.len(), 1, "{doc}");
    assert_eq!(unlinked[0]["at"], "2020-01-10T00:00:00.000Z");
    assert_eq!(
        item(&kernel, OTHER, "1")["item"]["modified"],
        MODIFIED,
        "a past unlink leaves the claimed modified"
    );
    // The target is no longer blocked.
    has(&source(&kernel, &at(LEDGER, "next"), &[]), "The target");
}

/// Status events use closedAtSource for both closes and reopens. Both history faces
/// must distinguish their source date from the time the importer committed the write.
fn status_history_marks_claimed_dates(reopened: bool) {
    let kernel = kernel();
    file(&kernel, LEDGER, "Dated status history");
    try_as(
        &kernel,
        &writer(IMPORTER),
        Verb::Sink,
        &at(LEDGER, "close"),
        &[("item", "1"), ("closed-at", EARLIER)],
    )
    .unwrap();
    let (kind, date) = if reopened {
        reopen().send(&kernel, IMPORTER, "1", Some(LATER)).unwrap();
        ("reopened", "2020-03-01T00:00:00.000Z")
    } else {
        ("closed", "2020-01-15T00:00:00.000Z")
    };
    let history = source(&kernel, &at(LEDGER, "item:1:history"), &[]);
    has(&history, &format!("{date} (claimed)  {kind}"));
    let event = events(&kernel, "1", kind).pop().expect("a status event");
    assert_eq!(event["at"], date);
    assert!(
        event["committed"]
            .as_str()
            .is_some_and(|at| at.starts_with("2026-")),
        "the importer commit time must be shown: {event}"
    );
    assert!(
        event.get("imported").is_none(),
        "this write changed the item"
    );

    // An importer making the same writes without source dates still stamps now.
    file(&kernel, LEDGER, "Undated status history");
    try_as(
        &kernel,
        &writer(IMPORTER),
        Verb::Sink,
        &at(LEDGER, "close"),
        &[("item", "2")],
    )
    .unwrap();
    if reopened {
        reopen().send(&kernel, IMPORTER, "2", None).unwrap();
    }
    let event = events(&kernel, "2", kind)
        .pop()
        .expect("an undated status event");
    assert_eq!(event["committed"], serde_json::Value::Null);
    let history = source(&kernel, &at(LEDGER, "item:2:history"), &[]);
    assert!(!history.contains("(claimed)"), "{history}");
}

#[test]
fn a_dated_close_history_marks_its_claimed_date() {
    status_history_marks_claimed_dates(false);
}

#[test]
fn a_dated_reopen_history_marks_its_claimed_date() {
    status_history_marks_claimed_dates(true);
}
