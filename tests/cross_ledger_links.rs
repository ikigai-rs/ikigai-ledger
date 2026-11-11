//! **Typed links across ledgers** (ledger #2829): a `blocks`, `parent` or `related` edge from an
//! item of one ledger to an item of another, admitted by `urn:cap:ledger:link-in:{target}`
//! beside `write` on the subject's ledger, with a mirror the ledger writes into the target's
//! graph: the subject's IRI and whether it is open, and nothing else.
//!
//! Until 0.17.0 every such edge was refused (`a ledger is a boundary`), so a `cli` item could
//! not keep an `acme` item out of `acme`'s `next`. These tests hold the item's acceptance:
//!
//! - a cross-ledger `blocks` excludes the target from `next` until the subject closes, and a
//!   concurrent close (or reopen, link or unlink) does not leave the mirror stale: the races
//!   are forced, not hoped for;
//! - a reader with only the target's grant sees the IRI and the open bit and nothing else;
//! - a caller without `link-in` is refused, and `write` on the target does not stand in.

mod common;

use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use common::*;
use ikigai_core::{
    ArgRef, Capability, Endpoint, Error, Fallback, Invocation, Iri, Kernel, Representation,
    Request, Resolution, Result, Scope, Space, Verb,
};
use ikigai_store::DurableStore;

const NS: &str = "https://ikigai-rs.dev/ns/ledger#";
const SUBJECT: &str = "cli";
const TARGET: &str = "acme";

fn at(ledger: &str, resource: &str) -> String {
    format!("urn:iki:ledger:{ledger}:{resource}")
}

/// File an item in `ledger` and return its IRI.
fn file(kernel: &Kernel, ledger: &str, title: &str) -> String {
    sink(kernel, &at(ledger, "append"), &[("content", title)])
        .split_whitespace()
        .nth(1)
        .expect("append answers with the number and the IRI")
        .to_string()
}

/// `cli#1` blocks `acme#1`, under root.
fn blocked_pair(kernel: &Kernel) -> (String, String) {
    let subject = file(kernel, SUBJECT, "Ship the CLI flag");
    let target = file(kernel, TARGET, "Document the flag");
    let answer = sink(
        kernel,
        &at(SUBJECT, "link"),
        &[("item", "1"), ("content", &target), ("type", "blocks")],
    );
    assert_eq!(answer, "linked cli#1 blocks acme#1\n");
    (subject, target)
}

fn next(kernel: &Kernel, ledger: &str) -> String {
    source(kernel, &at(ledger, "next"), &[])
}

/// The capability that may link `cli` items into `acme`: write on `cli`, `link-in` on `acme`,
/// and the store grants of both graphs (the mirror's update reads and writes `acme`'s).
fn linker() -> Capability {
    Capability::scoped(vec![
        format!("urn:cap:ledger:read:{SUBJECT}"),
        format!("urn:cap:ledger:write:{SUBJECT}"),
        graph_read(SUBJECT),
        graph_write(SUBJECT),
        format!("urn:cap:ledger:link-in:{TARGET}"),
        graph_read(TARGET),
        graph_write(TARGET),
    ])
}

/// What only reads `acme`: its ledger read grant and its graph's store read grant.
fn target_reader() -> Capability {
    Capability::scoped(vec![
        format!("urn:cap:ledger:read:{TARGET}"),
        graph_read(TARGET),
    ])
}

/// The mirror's state, asked of the store: `(present, open)` for every mirror on `target`.
fn mirrors(kernel: &Kernel, target: &str) -> Vec<(String, String)> {
    let json: serde_json::Value = serde_json::from_str(&select(
        kernel,
        &format!(
            "SELECT ?present ?open WHERE {{ GRAPH <urn:iki:ledger:graph:{TARGET}> {{ \
             ?m a <{NS}Mirror> ; <{NS}onItem> <{target}> ; <{NS}linkPresent> ?present ; \
             <{NS}subjectOpen> ?open }} }}"
        ),
    ))
    .expect("the store answers JSON");
    json["results"]["bindings"]
        .as_array()
        .expect("bindings")
        .iter()
        .map(|row| {
            (
                row["present"]["value"].as_str().unwrap().to_string(),
                row["open"]["value"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

/// ★ The acceptance's first line: `acme#1` is out of `acme`'s `next` while `cli#1` is open, is
/// offered once it closes, and is out again when it reopens. `next` in `acme` reads only
/// `acme`'s graph; the mirror is what carries the open bit across.
#[test]
fn a_cross_ledger_block_keeps_the_target_out_of_next_until_the_subject_closes() {
    let kernel = kernel();
    let (subject, _) = blocked_pair(&kernel);
    let blocked = next(&kernel, TARGET);
    assert!(
        blocked.starts_with("nothing is ready"),
        "acme#1 is blocked: {blocked}"
    );
    assert!(
        blocked.contains(&format!("acme#1 blocked by {subject} in the ledger cli")),
        "the reason names the blocker's IRI and its ledger: {blocked}"
    );

    sink(&kernel, &at(SUBJECT, "close"), &[("item", "1")]);
    let ready = next(&kernel, TARGET);
    assert!(
        ready.starts_with(" 1. acme#1"),
        "closing the blocker releases the target: {ready}"
    );

    sink(&kernel, &at(SUBJECT, "reopen"), &[("item", "1")]);
    assert!(
        next(&kernel, TARGET).starts_with("nothing is ready"),
        "reopening it blocks the target again"
    );

    // The JSON face says the same, in its own field.
    let json: serde_json::Value = serde_json::from_str(&source(
        &kernel,
        &at(TARGET, "next"),
        &[("as", "application/json")],
    ))
    .unwrap();
    assert_eq!(json["excluded"][0]["why"], "blocked");
    assert_eq!(
        json["excluded"][0]["blocked_elsewhere"][0],
        subject.as_str()
    );
}

/// The `next` of the subject's own ledger is untouched by the edge: `cli#1` is ready.
#[test]
fn the_subjects_own_next_is_unchanged() {
    let kernel = kernel();
    blocked_pair(&kernel);
    assert!(next(&kernel, SUBJECT).starts_with(" 1. cli#1"));
}

/// ★ The acceptance's second line: a reader holding only `acme`'s grants sees the blocker's IRI
/// and whether it is open, and none of its content; `cli` itself stays closed to them.
#[test]
fn a_reader_with_only_the_targets_grant_sees_the_iri_and_the_open_bit_and_nothing_else() {
    let kernel = kernel();
    let (subject, _) = blocked_pair(&kernel);
    let reader = target_reader();
    let detail = try_as(&kernel, &reader, Verb::Source, &at(TARGET, "item:1"), &[])
        .expect("the target's reader reads the target");
    assert!(
        detail.contains(&format!(
            "  blocked-by: {subject} (open; in the ledger cli, not readable under your grants)\n"
        )),
        "{detail}"
    );
    assert!(
        !detail.contains("Ship the CLI flag"),
        "no content: {detail}"
    );

    // The target's graph holds nothing of the subject but its IRI and the bit.
    let raw = select(
        &kernel,
        &format!(
            "SELECT ?s ?p ?o WHERE {{ GRAPH <urn:iki:ledger:graph:{TARGET}> {{ ?s ?p ?o }} }}"
        ),
    );
    assert!(!raw.contains("Ship the CLI flag"), "{raw}");

    // And the subject's ledger is no more reachable than before.
    assert!(matches!(
        try_as(&kernel, &reader, Verb::Source, &subject, &[]),
        Err(Error::Denied(_))
    ));
    assert!(matches!(
        try_as(&kernel, &reader, Verb::Source, &at(SUBJECT, "item:1"), &[]),
        Err(Error::Denied(_))
    ));

    sink(&kernel, &at(SUBJECT, "close"), &[("item", "1")]);
    let detail = try_as(&kernel, &reader, Verb::Source, &at(TARGET, "item:1"), &[]).unwrap();
    assert!(
        detail.contains(&format!(
            "  blocked-by: {subject} (closed; in the ledger cli, not"
        )),
        "{detail}"
    );

    // Root reads both, so the line says only where the blocker is.
    let detail = source(&kernel, &at(TARGET, "item:1"), &[]);
    assert!(
        detail.contains(&format!(
            "  blocked-by: {subject} (closed; in the ledger cli)\n"
        )),
        "{detail}"
    );
    let json: serde_json::Value = serde_json::from_str(&source(
        &kernel,
        &at(TARGET, "item:1"),
        &[("as", "application/json")],
    ))
    .unwrap();
    assert_eq!(
        json["item"]["mirrored"],
        serde_json::json!([{"type": "blocks", "iri": subject, "ledger": "cli", "status": "closed"}])
    );
}

/// The subject's own detail names its target among its `blocks:` lines, like a local edge.
#[test]
fn the_subject_names_its_target_in_its_blocks_lines() {
    let kernel = kernel();
    let (_, target) = blocked_pair(&kernel);
    let detail = source(&kernel, &at(SUBJECT, "item:1"), &[]);
    assert!(
        detail.contains(&format!("  blocks:   {target}\n")),
        "{detail}"
    );
}

/// ★ The acceptance's third line: without `link-in` on the target, the link is refused, before
/// the target is looked up and with nothing written in either graph; `write` on the target
/// does not stand in for it. With it, the same caller links.
#[test]
fn a_caller_without_link_in_is_refused() {
    let kernel = kernel();
    file(&kernel, SUBJECT, "Ship the CLI flag");
    let target = file(&kernel, TARGET, "Document the flag");
    let without = Capability::scoped(vec![
        format!("urn:cap:ledger:read:{SUBJECT}"),
        format!("urn:cap:ledger:write:{SUBJECT}"),
        graph_read(SUBJECT),
        graph_write(SUBJECT),
        format!("urn:cap:ledger:read:{TARGET}"),
        format!("urn:cap:ledger:write:{TARGET}"),
        graph_read(TARGET),
        graph_write(TARGET),
    ]);
    for (content, what) in [(target.as_str(), "the IRI"), ("acme#1", "the number")] {
        match try_as(
            &kernel,
            &without,
            Verb::Sink,
            &at(SUBJECT, "link"),
            &[("item", "1"), ("content", content), ("type", "blocks")],
        ) {
            Err(Error::Denied(why)) => {
                assert!(why.contains("urn:cap:ledger:link-in:acme"), "{what}: {why}")
            }
            other => panic!("{what}: expected Denied, got {other:?}"),
        }
    }
    // An item that does not exist is refused the same way: nothing is learned of `acme`.
    assert!(matches!(
        try_as(
            &kernel,
            &without,
            Verb::Sink,
            &at(SUBJECT, "link"),
            &[("item", "1"), ("content", "acme#99"), ("type", "blocks")],
        ),
        Err(Error::Denied(_))
    ));
    assert!(mirrors(&kernel, &target).is_empty());
    assert!(next(&kernel, TARGET).starts_with(" 1. acme#1"));
    assert!(!source(&kernel, &at(SUBJECT, "item:1"), &[]).contains("blocks:"));

    let answer = try_as(
        &kernel,
        &linker(),
        Verb::Sink,
        &at(SUBJECT, "link"),
        &[("item", "1"), ("content", "acme#1"), ("type", "blocks")],
    )
    .expect("link-in admits it");
    assert_eq!(answer, "linked cli#1 blocks acme#1\n");
    assert!(next(&kernel, TARGET).starts_with("nothing is ready"));
}

/// `link-in` is for the target's ledger: holding it for `acme` does not admit a link into
/// another ledger.
#[test]
fn link_in_names_one_ledger() {
    let kernel = kernel();
    file(&kernel, SUBJECT, "Ship the CLI flag");
    file(&kernel, "bosatsu", "Elsewhere");
    let mut scopes = vec![graph_read("bosatsu"), graph_write("bosatsu")];
    scopes.extend([
        format!("urn:cap:ledger:read:{SUBJECT}"),
        format!("urn:cap:ledger:write:{SUBJECT}"),
        graph_read(SUBJECT),
        graph_write(SUBJECT),
        format!("urn:cap:ledger:link-in:{TARGET}"),
    ]);
    match try_as(
        &kernel,
        &Capability::scoped(scopes),
        Verb::Sink,
        &at(SUBJECT, "link"),
        &[("item", "1"), ("content", "bosatsu#1"), ("type", "blocks")],
    ) {
        Err(Error::Denied(why)) => assert!(why.contains("urn:cap:ledger:link-in:bosatsu"), "{why}"),
        other => panic!("expected Denied, got {other:?}"),
    }
}

/// An unlink removes the edge, and the target is offered again.
#[test]
fn an_unlink_releases_the_target() {
    let kernel = kernel();
    let (_, target) = blocked_pair(&kernel);
    let answer = delete(
        &kernel,
        &at(SUBJECT, "link"),
        &[("item", "1"), ("content", &target), ("type", "blocks")],
    );
    assert_eq!(answer, format!("unlinked cli#1 blocks {target}\n"));
    assert!(next(&kernel, TARGET).starts_with(" 1. acme#1"));
    assert_eq!(mirrors(&kernel, &target), [("false".into(), "true".into())]);
    let detail = source(&kernel, &at(TARGET, "item:1"), &[]);
    assert!(!detail.contains("blocked-by"), "{detail}");
    assert!(!source(&kernel, &at(SUBJECT, "item:1"), &[]).contains("blocks:"));
    // And linking again brings the mirror back.
    sink(
        &kernel,
        &at(SUBJECT, "link"),
        &[("item", "1"), ("content", &target), ("type", "blocks")],
    );
    assert!(next(&kernel, TARGET).starts_with("nothing is ready"));
}

/// `parent` follows the same pattern, for an umbrella item in another ledger: the umbrella
/// names its child and whether it is open, and is not blocked by it. `related` needs no mirror.
#[test]
fn parent_is_mirrored_and_related_is_not() {
    let kernel = kernel();
    let child = file(&kernel, SUBJECT, "An arc in the CLI");
    let umbrella = file(&kernel, TARGET, "The goal");
    sink(
        &kernel,
        &at(SUBJECT, "link"),
        &[("item", "1"), ("content", &umbrella), ("type", "parent")],
    );
    let detail = source(&kernel, &at(TARGET, "item:1"), &[]);
    assert!(
        detail.contains(&format!("  child:    {child} (open; in the ledger cli)\n")),
        "{detail}"
    );
    assert!(
        next(&kernel, TARGET).starts_with(" 1. acme#1"),
        "a parent does not block"
    );
    sink(&kernel, &at(SUBJECT, "close"), &[("item", "1")]);
    let detail = source(&kernel, &at(TARGET, "item:1"), &[]);
    assert!(
        detail.contains(&format!(
            "  child:    {child} (closed; in the ledger cli)\n"
        )),
        "{detail}"
    );

    let other = file(&kernel, TARGET, "See also");
    let answer = sink(
        &kernel,
        &at(SUBJECT, "link"),
        &[("item", "1"), ("content", &other), ("type", "related")],
    );
    assert_eq!(answer, "linked cli#1 related acme#2\n");
    assert!(
        mirrors(&kernel, &other).is_empty(),
        "related writes no mirror"
    );
    let detail = source(&kernel, &at(SUBJECT, "item:1"), &[]);
    assert!(
        detail.contains(&format!("  related:  {other}\n")),
        "{detail}"
    );
}

/// A close the caller could not carry into the target's mirror is refused before anything
/// changes, naming the grants: a status change whose mirror cannot follow is a stale mirror.
#[test]
fn a_close_that_cannot_reach_the_mirror_is_refused_and_changes_nothing() {
    let kernel = kernel();
    blocked_pair(&kernel);
    let closer = Capability::scoped(vec![
        format!("urn:cap:ledger:read:{SUBJECT}"),
        format!("urn:cap:ledger:close:{SUBJECT}"),
        graph_read(SUBJECT),
        graph_write(SUBJECT),
    ]);
    match try_as(
        &kernel,
        &closer,
        Verb::Sink,
        &at(SUBJECT, "close"),
        &[("item", "1")],
    ) {
        Err(Error::Denied(why)) => {
            assert!(why.contains(&graph_write(TARGET)), "{why}");
            assert!(why.contains("Nothing was changed"), "{why}");
        }
        other => panic!("expected Denied, got {other:?}"),
    }
    assert!(source(&kernel, &at(SUBJECT, "item:1"), &[]).contains("  open  "));
    // With the target's store grants, the same close goes through and the mirror follows.
    let closer = Capability::scoped(vec![
        format!("urn:cap:ledger:read:{SUBJECT}"),
        format!("urn:cap:ledger:close:{SUBJECT}"),
        graph_read(SUBJECT),
        graph_write(SUBJECT),
        graph_read(TARGET),
        graph_write(TARGET),
    ]);
    try_as(
        &kernel,
        &closer,
        Verb::Sink,
        &at(SUBJECT, "close"),
        &[("item", "1")],
    )
    .expect("the close reaches the mirror");
    assert!(next(&kernel, TARGET).starts_with(" 1. acme#1"));
}

/// Deleting a subject that still carries a link into another ledger is refused: the mirror
/// would go on naming it, open, with nothing left to close.
#[test]
fn a_subject_with_a_link_across_is_not_deleted_until_unlinked() {
    let kernel = kernel();
    let (_, target) = blocked_pair(&kernel);
    match try_verb(&kernel, Verb::Delete, &at(SUBJECT, "item:1"), &[]) {
        Err(Error::InvalidArgument { name, detail }) => {
            assert_eq!(name, "item");
            assert!(detail.contains("Remove the link first"), "{detail}");
        }
        other => panic!("expected InvalidArgument, got {other:?}"),
    }
    delete(
        &kernel,
        &at(SUBJECT, "link"),
        &[("item", "1"), ("content", &target), ("type", "blocks")],
    );
    delete(&kernel, &at(SUBJECT, "item:1"), &[]);
    assert!(next(&kernel, TARGET).starts_with(" 1. acme#1"));
}

/// Deleting the TARGET archives its mirror with it, like its comments.
#[test]
fn deleting_the_target_takes_its_mirror_with_it() {
    let kernel = kernel();
    let (_, target) = blocked_pair(&kernel);
    delete(&kernel, &at(TARGET, "item:1"), &[]);
    assert!(mirrors(&kernel, &target).is_empty());
}

// ------------------------------------------------------------------- the forced races

/// What runs, once, just before the next update to the target's graph: the mirror write of
/// whichever request gets there first.
#[derive(Default)]
struct Armed(Mutex<Option<Request>>);

struct Interleave {
    inner: Arc<dyn Space>,
    kernel: Arc<OnceLock<Kernel>>,
    armed: Arc<Armed>,
}

struct BeforeMirror {
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
                Arc::new(BeforeMirror {
                    inner,
                    kernel,
                    armed,
                }) as Arc<dyn Endpoint>
            })
    }
}

#[async_trait]
impl Endpoint for BeforeMirror {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let to_target = inv.request.target.as_str() == "urn:iki:store:graph-update"
            && inv
                .inline_str("graph")
                .map(|g| g == format!("urn:iki:ledger:graph:{TARGET}"))
                .unwrap_or(false);
        if to_target {
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

fn scheduled() -> (Arc<OnceLock<Kernel>>, Arc<Armed>) {
    let cell = Arc::new(OnceLock::new());
    let armed = Arc::new(Armed::default());
    let store = Arc::new(ikigai_store::space(DurableStore::in_memory().unwrap())) as Arc<dyn Space>;
    let space = Fallback::new(vec![
        Arc::new(Interleave {
            inner: store,
            kernel: Arc::clone(&cell),
            armed: Arc::clone(&armed),
        }) as Arc<dyn Space>,
        Arc::new(ikigai_ledger::space()) as Arc<dyn Space>,
    ]);
    let kernel =
        Kernel::with_meta_renderer(Arc::new(space), Arc::new(ikigai_vocab::TurtleRenderer))
            .with_clock(Arc::new(TickingClock::default()));
    assert!(cell.set(kernel).is_ok());
    (cell, armed)
}

fn request(verb: Verb, iri: &str, args: &[(&str, &str)]) -> Request {
    args.iter().fold(
        Request::new(verb, Iri::parse(iri).expect("a test IRI")),
        |r, (name, value)| r.with_arg(*name, ArgRef::Inline(value.as_bytes().to_vec())),
    )
}

fn arm(armed: &Armed, verb: Verb, iri: &str, args: &[(&str, &str)]) {
    *armed.0.lock().unwrap() = Some(request(verb, iri, args));
}

/// The delete checked links before reading its archive. A new link at its commit must not
/// be swallowed by its retry, leaving a mirror whose subject can never be closed.
#[test]
fn a_cross_ledger_link_racing_a_delete_keeps_its_subject_live() {
    for kind in ["blocks", "parent"] {
        let (cell, armed) = scheduled();
        let kernel = cell.get().unwrap();
        // The gate is on acme's graph, so acme is the subject in this schedule.
        file(kernel, TARGET, "Blocker");
        let target = file(kernel, SUBJECT, "Blocked");
        arm(
            &armed,
            Verb::Sink,
            &at(TARGET, "link"),
            &[("item", "1"), ("content", &target), ("type", kind)],
        );
        let result = try_verb(kernel, Verb::Delete, &at(TARGET, "item:1"), &[]);
        assert!(armed.0.lock().unwrap().is_none(), "the link ran in between");
        assert!(
            matches!(result, Err(Error::InvalidArgument { .. })),
            "the new link must refuse the delete, got {result:?}; target next: {}",
            next(kernel, SUBJECT)
        );
        assert!(source(kernel, &at(TARGET, "item:1"), &[]).contains("Blocker"));
        // Once the edge is removed, the delete can finish and the target is ready.
        delete(
            kernel,
            &at(TARGET, "link"),
            &[("item", "1"), ("content", &target), ("type", kind)],
        );
        delete(kernel, &at(TARGET, "item:1"), &[]);
        assert!(next(kernel, SUBJECT).starts_with(" 1. cli#1"));
    }
}

/// ★ A close whose mirror write lands LAST, after a whole reopen ran in between: the close read
/// "closed", the reopen read "open" and wrote it, and then the close's write arrives. A mirror
/// written from what each writer read would end "closed" with the subject open, and `acme#1`
/// would be offered while it is blocked. The version decides instead.
#[test]
fn a_close_whose_mirror_lands_after_a_reopen_does_not_leave_it_stale() {
    let (cell, armed) = scheduled();
    let kernel = cell.get().unwrap();
    let (_, target) = blocked_pair(kernel);
    arm(&armed, Verb::Sink, &at(SUBJECT, "reopen"), &[("item", "1")]);
    sink(kernel, &at(SUBJECT, "close"), &[("item", "1")]);
    assert!(
        armed.0.lock().unwrap().is_none(),
        "the reopen ran in between"
    );
    assert!(source(kernel, &at(SUBJECT, "item:1"), &[]).contains("  open  "));
    assert_eq!(mirrors(kernel, &target), [("true".into(), "true".into())]);
    assert!(next(kernel, TARGET).starts_with("nothing is ready"));
}

/// The other order: a reopen whose mirror write lands after a whole close.
#[test]
fn a_reopen_whose_mirror_lands_after_a_close_does_not_leave_it_stale() {
    let (cell, armed) = scheduled();
    let kernel = cell.get().unwrap();
    let (_, target) = blocked_pair(kernel);
    sink(kernel, &at(SUBJECT, "close"), &[("item", "1")]);
    arm(&armed, Verb::Sink, &at(SUBJECT, "close"), &[("item", "1")]);
    // The armed close finds the item open only if the reopen's own update ran first, which it
    // has by the time the reopen writes its mirror.
    sink(kernel, &at(SUBJECT, "reopen"), &[("item", "1")]);
    assert!(armed.0.lock().unwrap().is_none());
    assert!(source(kernel, &at(SUBJECT, "item:1"), &[]).contains("  closed  "));
    assert_eq!(mirrors(kernel, &target), [("true".into(), "false".into())]);
    assert!(next(kernel, TARGET).starts_with(" 1. acme#1"));
}

/// ★ A link racing a close: the link read the subject open, a whole close ran before the
/// link's mirror landed, and the close's mirror (closed) must survive the link's (open).
#[test]
fn a_link_whose_mirror_lands_after_a_close_does_not_leave_it_stale() {
    let (cell, armed) = scheduled();
    let kernel = cell.get().unwrap();
    file(kernel, SUBJECT, "Ship the CLI flag");
    let target = file(kernel, TARGET, "Document the flag");
    arm(&armed, Verb::Sink, &at(SUBJECT, "close"), &[("item", "1")]);
    sink(
        kernel,
        &at(SUBJECT, "link"),
        &[("item", "1"), ("content", &target), ("type", "blocks")],
    );
    assert!(armed.0.lock().unwrap().is_none());
    assert_eq!(mirrors(kernel, &target), [("true".into(), "false".into())]);
    assert!(next(kernel, TARGET).starts_with(" 1. acme#1"));
}

/// ★ A link whose target is deleted while it is in flight is taken back, and its history says
/// so (ledger #2841): the `link` event stands, with an `unlink` beside it naming the same item.
#[test]
fn a_link_taken_back_is_an_unlink_in_the_history() {
    let (cell, armed) = scheduled();
    let kernel = cell.get().unwrap();
    file(kernel, SUBJECT, "Ship the CLI flag");
    let target = file(kernel, TARGET, "Document the flag");
    arm(&armed, Verb::Delete, &at(TARGET, "item:1"), &[]);
    let refused = try_verb(
        kernel,
        Verb::Sink,
        &at(SUBJECT, "link"),
        &[("item", "1"), ("content", &target), ("type", "blocks")],
    );
    assert!(matches!(refused, Err(Error::NotFound(_))), "{refused:?}");
    let history = source(kernel, &at(SUBJECT, "item:1:history"), &[]);
    let lines: Vec<&str> = history.lines().collect();
    assert_eq!(lines.len(), 3, "{history}");
    assert!(
        lines[1].ends_with(&format!("  link blocks {target}")),
        "{history}"
    );
    assert!(
        lines[2].ends_with(&format!("  unlink blocks {target}")),
        "{history}"
    );
}

/// ★ A close racing an unlink: the close read the link present, a whole unlink ran before the
/// close's mirror landed, and the edge must stay removed rather than come back from the
/// close's older snapshot.
#[test]
fn an_unlink_is_not_undone_by_a_close_that_read_the_link() {
    let (cell, armed) = scheduled();
    let kernel = cell.get().unwrap();
    let (_, target) = blocked_pair(kernel);
    sink(kernel, &at(SUBJECT, "close"), &[("item", "1")]);
    arm(
        &armed,
        Verb::Delete,
        &at(SUBJECT, "link"),
        &[("item", "1"), ("content", &target), ("type", "blocks")],
    );
    sink(kernel, &at(SUBJECT, "reopen"), &[("item", "1")]);
    assert!(armed.0.lock().unwrap().is_none());
    assert!(source(kernel, &at(SUBJECT, "item:1"), &[]).contains("  open  "));
    assert_eq!(mirrors(kernel, &target), [("false".into(), "true".into())]);
    assert!(next(kernel, TARGET).starts_with(" 1. acme#1"));
}

/// A `next` in the target that was cached before the close is not served after it: the
/// mirror's write is a store write, and the store's golden thread cuts the cached read.
#[test]
fn a_cached_next_in_the_target_sees_the_close() {
    let kernel = kernel();
    blocked_pair(&kernel);
    let reader = target_reader();
    let before = try_as(&kernel, &reader, Verb::Source, &at(TARGET, "next"), &[]).unwrap();
    let again = try_as(&kernel, &reader, Verb::Source, &at(TARGET, "next"), &[]).unwrap();
    assert_eq!(before, again);
    assert!(before.starts_with("nothing is ready"));
    sink(&kernel, &at(SUBJECT, "close"), &[("item", "1")]);
    let after = try_as(&kernel, &reader, Verb::Source, &at(TARGET, "next"), &[]).unwrap();
    assert!(after.starts_with(" 1. acme#1"), "{after}");
}

/// The reproduction, kept: an object in another ledger named by a number qualified with its
/// ledger resolves THERE, never in the subject's ledger, so `acme#1` is not `cli#1`.
#[test]
fn a_qualified_number_names_the_other_ledgers_item() {
    let kernel = kernel();
    file(&kernel, SUBJECT, "Ship the CLI flag");
    let target = file(&kernel, TARGET, "Document the flag");
    sink(
        &kernel,
        &at(SUBJECT, "link"),
        &[("item", "1"), ("content", "acme#1"), ("type", "blocks")],
    );
    assert_eq!(mirrors(&kernel, &target).len(), 1);
}

/// A `related` link across has no mirror, so it does not hold its subject back from a delete.
#[test]
fn a_related_link_across_does_not_hold_back_a_delete() {
    let kernel = kernel();
    file(&kernel, SUBJECT, "Ship the CLI flag");
    let other = file(&kernel, TARGET, "See also");
    sink(
        &kernel,
        &at(SUBJECT, "link"),
        &[("item", "1"), ("content", &other), ("type", "related")],
    );
    delete(&kernel, &at(SUBJECT, "item:1"), &[]);
}

/// A link or unlink across is refused before anything is written when the caller cannot write
/// the mirror: holding `link-in` and the target's store READ grant is not enough. Without this
/// check the edge was committed and the mirror's write then refused, leaving an edge the target
/// never learned of (a link) or a mirror that goes on naming a removed edge (an unlink).
#[test]
fn a_link_that_cannot_reach_the_mirror_is_refused_and_changes_nothing() {
    let kernel = kernel();
    file(&kernel, SUBJECT, "Ship the CLI flag");
    let target = file(&kernel, TARGET, "Document the flag");
    let reads_only = Capability::scoped(vec![
        format!("urn:cap:ledger:read:{SUBJECT}"),
        format!("urn:cap:ledger:write:{SUBJECT}"),
        graph_read(SUBJECT),
        graph_write(SUBJECT),
        format!("urn:cap:ledger:link-in:{TARGET}"),
        graph_read(TARGET),
    ]);
    match try_as(
        &kernel,
        &reads_only,
        Verb::Sink,
        &at(SUBJECT, "link"),
        &[("item", "1"), ("content", "acme#1"), ("type", "blocks")],
    ) {
        Err(Error::Denied(why)) => {
            assert!(why.contains(&graph_write(TARGET)), "{why}");
            assert!(why.contains("Nothing was changed"), "{why}");
        }
        other => panic!("expected Denied, got {other:?}"),
    }
    assert!(!source(&kernel, &at(SUBJECT, "item:1"), &[]).contains("blocks:"));
    assert!(mirrors(&kernel, &target).is_empty());

    // The unlink is held to the same rule: linked by a holder of every grant, it is not
    // removed by a caller that could not clear the mirror.
    try_as(
        &kernel,
        &linker(),
        Verb::Sink,
        &at(SUBJECT, "link"),
        &[("item", "1"), ("content", "acme#1"), ("type", "blocks")],
    )
    .expect("the linker holds every grant");
    match try_as(
        &kernel,
        &reads_only,
        Verb::Delete,
        &at(SUBJECT, "link"),
        &[("item", "1"), ("content", "acme#1"), ("type", "blocks")],
    ) {
        Err(Error::Denied(why)) => assert!(why.contains(&graph_write(TARGET)), "{why}"),
        other => panic!("expected Denied, got {other:?}"),
    }
    assert!(source(&kernel, &at(SUBJECT, "item:1"), &[]).contains("blocks:"));
    assert!(next(&kernel, TARGET).starts_with("nothing is ready"));
}

/// ★ The item object is the same wherever it appears (ledger #2927): a listing row carries the
/// links into other ledgers and the mirrored edges that the item's own read shows. Until 0.18.0
/// only `item:{id}` added them, so a row of `items` held only the links within its ledger and
/// no `mirrored` at all. Several items on one page, so the batched read is checked per item.
#[test]
fn a_listing_row_carries_the_links_across_and_the_mirrors_its_item_face_shows() {
    let kernel = kernel();
    let (subject, target) = blocked_pair(&kernel);
    let umbrella = file(&kernel, TARGET, "The goal");
    let other = file(&kernel, TARGET, "See also");
    let lone = file(&kernel, SUBJECT, "Nothing across");
    for (kind, to) in [("parent", &umbrella), ("related", &other)] {
        sink(
            &kernel,
            &at(SUBJECT, "link"),
            &[("item", "1"), ("content", to), ("type", kind)],
        );
    }
    for (ledger, item) in [
        (SUBJECT, &subject),
        (SUBJECT, &lone),
        (TARGET, &target),
        (TARGET, &umbrella),
        (TARGET, &other),
    ] {
        let face: serde_json::Value =
            serde_json::from_str(&source(&kernel, item, &[("as", "application/json")])).unwrap();
        let listing: serde_json::Value = serde_json::from_str(&source(
            &kernel,
            &at(ledger, "items"),
            &[("as", "application/json")],
        ))
        .unwrap();
        let row = listing["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["iri"] == face["item"]["iri"])
            .unwrap_or_else(|| panic!("{item} is listed: {listing}"));
        assert_eq!(row["links"], face["item"]["links"], "links of {item}");
        assert_eq!(
            row["mirrored"], face["item"]["mirrored"],
            "mirrored of {item}"
        );
    }
    // And the rows say what the faces say, not merely the same nothing.
    let listing: serde_json::Value = serde_json::from_str(&source(
        &kernel,
        &at(SUBJECT, "items"),
        &[("as", "application/json")],
    ))
    .unwrap();
    let row = listing["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["iri"] == serde_json::json!(subject))
        .unwrap();
    let targets: Vec<&str> = row["links"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["target"]["iri"].as_str().unwrap())
        .collect();
    assert_eq!(
        targets,
        [target.as_str(), umbrella.as_str(), other.as_str()]
    );
    let listing: serde_json::Value = serde_json::from_str(&source(
        &kernel,
        &at(TARGET, "items"),
        &[("as", "application/json")],
    ))
    .unwrap();
    let mirrored: Vec<&serde_json::Value> = listing["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| &row["mirrored"])
        .filter(|m| !m.is_null())
        .collect();
    assert_eq!(
        mirrored.len(),
        2,
        "acme#1 and acme#2 each hold one: {listing}"
    );
}

/// ★ `next`'s rows are the same item object too (ledger #2931): a ranked row and an excluded
/// row carry the links across and the mirrored edges their item faces show. Until 0.18.0 `next`
/// rendered its items undecorated, so the two disagreed on any item with an edge across.
#[test]
fn a_next_row_carries_the_links_across_and_the_mirrors_its_item_face_shows() {
    let kernel = kernel();
    let (subject, target) = blocked_pair(&kernel);
    let umbrella = file(&kernel, TARGET, "The goal");
    let other = file(&kernel, TARGET, "See also");
    for (kind, to) in [("parent", &umbrella), ("related", &other)] {
        sink(
            &kernel,
            &at(SUBJECT, "link"),
            &[("item", "1"), ("content", to), ("type", kind)],
        );
    }
    let mut seen = Vec::new();
    for ledger in [SUBJECT, TARGET] {
        let doc: serde_json::Value = serde_json::from_str(&source(
            &kernel,
            &at(ledger, "next"),
            &[("as", "application/json"), ("limit", "10")],
        ))
        .unwrap();
        let rows = doc["ranking"]
            .as_array()
            .unwrap()
            .iter()
            .chain(doc["excluded"].as_array().unwrap())
            .map(|row| &row["item"]);
        for row in rows {
            let iri = row["iri"].as_str().unwrap().to_string();
            let face: serde_json::Value =
                serde_json::from_str(&source(&kernel, &iri, &[("as", "application/json")]))
                    .unwrap();
            assert_eq!(row["links"], face["item"]["links"], "links of {iri}");
            assert_eq!(
                row["mirrored"], face["item"]["mirrored"],
                "mirrored of {iri}"
            );
            seen.push(iri);
        }
    }
    // Every item was a row, and the ones with edges across say so, not merely the same nothing.
    for item in [&subject, &target, &umbrella, &other] {
        assert!(seen.contains(item), "{item} is a row of next: {seen:?}");
    }
    let doc: serde_json::Value = serde_json::from_str(&source(
        &kernel,
        &at(SUBJECT, "next"),
        &[("as", "application/json")],
    ))
    .unwrap();
    assert_eq!(
        doc["ranking"][0]["item"]["links"].as_array().unwrap().len(),
        3,
        "{doc}"
    );
    let doc: serde_json::Value = serde_json::from_str(&source(
        &kernel,
        &at(TARGET, "next"),
        &[("as", "application/json")],
    ))
    .unwrap();
    let excluded = &doc["excluded"][0]["item"];
    assert_eq!(excluded["iri"], serde_json::json!(target), "{doc}");
    assert_eq!(excluded["mirrored"][0]["type"], "blocks", "{doc}");
}
