//! **Who claimed, released and handed back each item, so "what did this agent do" is a query**
//! (ledger #2819).
//!
//! Until 0.15.0 a claim was current state only: `claimedBy`, `claimPrincipal` and the rest were
//! retracted by a release or a close, so once an item closed the graph no longer said who had
//! worked on it, and a status event said when an item closed but never who closed it. Now
//! every first claim and takeover writes a `ledger:ClaimEvent`, every release (by the holder,
//! by a dispatcher, by a close, by a takeover) a `ledger:ReleaseEvent`, a hand-back names its
//! holder (`ledger:handedBackBy`), every edit that creates no node a `ledger:EditEvent`, and
//! each carries the committer the door stamped. Each is written in the update that makes the
//! change, under its guard, so a lost race writes no event. An import principal may date a
//! close (`closed-at=`), and nobody else may.
//!
//! Each test reads the event nodes back from the store and holds them to exactly the triples
//! the item's resource map names.

mod common;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Poll;

use async_trait::async_trait;
use common::*;
use futures::executor::block_on;
use ikigai_core::{
    ArgRef, Capability, Endpoint, Error, Fallback, Invocation, Iri, Kernel, Representation,
    Request, Resolution, Scope, Space, Verb,
};
use ikigai_ledger::SpaceConfig;
use ikigai_store::DurableStore;

const LEDGER: &str = "acme";
const LIVE: &str = "urn:iki:ledger:graph:acme";
const NS: &str = "https://ikigai-rs.dev/ns/ledger#";
const PARTICIPANTS: &str = "urn:collab:test:participants";
const A: &str = "urn:iki:gonk:client:aaaa";
const B: &str = "urn:iki:gonk:client:bbbb";
const D: &str = "urn:iki:gonk:passkey:dispatcher";
const IMPORTER: &str = "urn:iki:gonk:client:importer";

fn at(resource: &str) -> String {
    format!("urn:iki:ledger:{LEDGER}:{resource}")
}

fn grant(verbs: &[&str]) -> Capability {
    let mut scopes: Vec<String> = verbs
        .iter()
        .map(|verb| format!("urn:cap:ledger:{verb}:{LEDGER}"))
        .collect();
    scopes.push(graph_read(LEDGER));
    scopes.push(graph_write(LEDGER));
    scopes.push(format!("urn:cap:store:read:graph:{PARTICIPANTS}"));
    Capability::scoped(scopes)
}

/// The worker grant, as a door mints it for one client.
fn worker(principal: &str) -> Capability {
    grant(&["read", "claim", "comment"])
        .with_principal(principal)
        .unwrap()
}

/// The dispatcher: the write grant, which covers close and every edit.
fn dispatcher(principal: &str) -> Capability {
    grant(&["read", "write"]).with_principal(principal).unwrap()
}

/// A kernel whose ledger reads enrollment from [`PARTICIPANTS`], where [`IMPORTER`] is an
/// import principal, with one item filed.
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
    sink(&kernel, &at("append"), &[("content", "Fix the widget")]);
    kernel
}

fn claim(kernel: &Kernel, who: &Capability, args: &[(&str, &str)]) -> Result<String, Error> {
    let mut all = vec![("item", "1")];
    all.extend_from_slice(args);
    try_as(kernel, who, Verb::Sink, &at("claim"), &all)
}

fn release(kernel: &Kernel, who: &Capability, args: &[(&str, &str)]) -> Result<String, Error> {
    let mut all = vec![("item", "1")];
    all.extend_from_slice(args);
    try_as(kernel, who, Verb::Delete, &at("claim"), &all)
}

fn close(kernel: &Kernel, who: &Capability, args: &[(&str, &str)]) -> Result<String, Error> {
    let mut all = vec![("item", "1")];
    all.extend_from_slice(args);
    try_as(kernel, who, Verb::Sink, &at("close"), &all)
}

fn rows(kernel: &Kernel, query: &str) -> Vec<BTreeMap<String, String>> {
    let json: serde_json::Value =
        serde_json::from_str(&select(kernel, query)).expect("the store answers JSON");
    json["results"]["bindings"]
        .as_array()
        .expect("bindings")
        .iter()
        .map(|row| {
            row.as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), v["value"].as_str().unwrap().to_string()))
                .collect()
        })
        .collect()
}

/// A predicate as these tests name it: the local name of a `ledger:` term, and the two
/// external terms an event carries.
fn short(predicate: &str) -> String {
    match predicate {
        "http://www.w3.org/1999/02/22-rdf-syntax-ns#type" => "type".to_string(),
        "http://purl.org/dc/terms/created" => "created".to_string(),
        other => other
            .strip_prefix(NS)
            .unwrap_or_else(|| panic!("an event carries an unexpected predicate {other}"))
            .to_string(),
    }
}

/// Every node of `class` in the live graph, oldest first, each as its predicates and values
/// (a predicate written twice would show as a doubled value, `a | b`).
fn events(kernel: &Kernel, class: &str) -> Vec<BTreeMap<String, String>> {
    let mut by_node: BTreeMap<(String, String), BTreeMap<String, String>> = BTreeMap::new();
    for row in rows(
        kernel,
        &format!(
            "SELECT ?e ?at ?p ?o WHERE {{ GRAPH <{LIVE}> {{ ?e a <{NS}{class}> ; \
             <http://purl.org/dc/terms/created> ?at ; ?p ?o }} }}"
        ),
    ) {
        let node = by_node
            .entry((row["at"].clone(), row["e"].clone()))
            .or_default();
        let key = short(&row["p"]);
        let value = row["o"].clone();
        node.entry(key)
            .and_modify(|was| *was = format!("{was} | {value}"))
            .or_insert(value);
    }
    by_node.into_values().collect()
}

fn keys(event: &BTreeMap<String, String>) -> Vec<&str> {
    event.keys().map(String::as_str).collect()
}

fn item_iri(kernel: &Kernel) -> String {
    rows(
        kernel,
        &format!("SELECT ?i WHERE {{ GRAPH <{LIVE}> {{ ?i <{NS}number> 1 }} }}"),
    )[0]["i"]
        .clone()
}

// --------------------------------------------------------------------------- claims

/// ★ A first claim writes one claim event: the holder label, the principal, the kind, the
/// time and the committer. On 0.14.2 it wrote none, and the release erased the claim.
#[test]
fn a_first_claim_records_one_claim_event() {
    let kernel = kernel();
    let item = item_iri(&kernel);
    claim(
        &kernel,
        &worker(A),
        &[("content", "lawnboy-claude"), ("lease", "30m")],
    )
    .unwrap();

    let claims = events(&kernel, "ClaimEvent");
    assert_eq!(claims.len(), 1, "{claims:?}");
    let event = &claims[0];
    assert_eq!(
        keys(event),
        [
            "claimKind",
            "claimPrincipal",
            "claimedBy",
            "committedAt",
            "committer",
            "created",
            "onItem",
            "type"
        ]
    );
    assert_eq!(event["onItem"], item);
    assert_eq!(event["claimedBy"], "lawnboy-claude");
    assert_eq!(event["claimPrincipal"], A);
    assert_eq!(event["committer"], A);
    assert_eq!(event["claimKind"], format!("{NS}machine"));
    assert_eq!(event["created"], event["committedAt"]);
    let iri = rows(
        &kernel,
        &format!("SELECT ?e WHERE {{ GRAPH <{LIVE}> {{ ?e a <{NS}ClaimEvent> }} }}"),
    )[0]["e"]
        .clone();
    assert!(
        iri.starts_with(&format!("{item}:claim:")),
        "skolemized beneath the item: {iri}"
    );
}

/// A renewal, with `renew=true` or by claiming again as the same holder, extends the claim
/// and records no event: the claim it extends already has one.
#[test]
fn a_renewal_records_no_claim_event() {
    let kernel = kernel();
    claim(&kernel, &worker(A), &[("content", "w"), ("lease", "30m")]).unwrap();
    claim(
        &kernel,
        &worker(A),
        &[("content", "w"), ("renew", "true"), ("lease", "30m")],
    )
    .unwrap();
    claim(&kernel, &worker(A), &[("content", "w")]).unwrap();
    assert_eq!(events(&kernel, "ClaimEvent").len(), 1);
    assert!(events(&kernel, "ReleaseEvent").is_empty());
}

/// ★ A takeover records the claim it took (`takeoverFrom` naming the holder it took from)
/// and the release of the claim it replaced, reason `takeover`.
#[test]
fn a_takeover_records_a_claim_from_and_a_release() {
    let kernel = kernel();
    claim(
        &kernel,
        &worker(A),
        &[("content", "lawnboy"), ("lease", "1s")],
    )
    .unwrap();
    // The test clock moves a second a reading, so the lease has run out by now.
    claim(
        &kernel,
        &worker(B),
        &[
            ("content", "lawnboy-claude"),
            ("takeover", "true"),
            ("from", "lawnboy"),
        ],
    )
    .unwrap();

    let claims = events(&kernel, "ClaimEvent");
    assert_eq!(claims.len(), 2, "{claims:?}");
    assert!(!claims[0].contains_key("takeoverFrom"));
    let taken = &claims[1];
    assert_eq!(
        keys(taken),
        [
            "claimKind",
            "claimPrincipal",
            "claimedBy",
            "committedAt",
            "committer",
            "created",
            "onItem",
            "takeoverFrom",
            "type"
        ]
    );
    assert_eq!(taken["claimedBy"], "lawnboy-claude");
    assert_eq!(taken["claimPrincipal"], B);
    assert_eq!(taken["committer"], B);
    assert_eq!(taken["takeoverFrom"], "lawnboy");

    let releases = events(&kernel, "ReleaseEvent");
    assert_eq!(releases.len(), 1, "{releases:?}");
    let released = &releases[0];
    assert_eq!(released["releaseReason"], "takeover");
    assert_eq!(released["claimedBy"], "lawnboy");
    assert_eq!(released["claimPrincipal"], A);
    assert_eq!(released["committer"], B, "the taker released it");
    assert_eq!(
        released["created"], taken["created"],
        "one update wrote both"
    );
}

// ------------------------------------------------------------------------- releases

/// ★ A release by the holder records who held it, as whom, since when, who released it and
/// why. On 0.14.2 nothing of the claim survived it.
#[test]
fn a_release_by_the_holder_records_the_claim_it_released() {
    let kernel = kernel();
    let item = item_iri(&kernel);
    claim(&kernel, &worker(A), &[("content", "lawnboy")]).unwrap();
    release(&kernel, &worker(A), &[("from", "lawnboy")]).unwrap();

    let claimed_at = events(&kernel, "ClaimEvent")[0]["created"].clone();
    let releases = events(&kernel, "ReleaseEvent");
    assert_eq!(releases.len(), 1, "{releases:?}");
    let event = &releases[0];
    assert_eq!(
        keys(event),
        [
            "claimKind",
            "claimPrincipal",
            "claimedAt",
            "claimedBy",
            "committedAt",
            "committer",
            "created",
            "onItem",
            "releaseReason",
            "type"
        ]
    );
    assert_eq!(event["onItem"], item);
    assert_eq!(event["claimedBy"], "lawnboy");
    assert_eq!(event["claimPrincipal"], A);
    assert_eq!(event["claimedAt"], claimed_at);
    assert_eq!(event["committer"], A);
    assert_eq!(event["releaseReason"], "released");
}

/// A dispatcher's override is a release like any other, and its committer is the dispatcher.
#[test]
fn a_release_by_the_dispatcher_names_the_dispatcher() {
    let kernel = kernel();
    claim(&kernel, &worker(A), &[("content", "lawnboy")]).unwrap();
    release(&kernel, &dispatcher(D), &[]).unwrap();
    let releases = events(&kernel, "ReleaseEvent");
    assert_eq!(releases.len(), 1);
    assert_eq!(releases[0]["claimPrincipal"], A);
    assert_eq!(releases[0]["committer"], D);
    assert_eq!(releases[0]["releaseReason"], "released");
}

/// Releasing a free item releases nothing, and records nothing.
#[test]
fn a_release_of_a_free_item_records_nothing() {
    let kernel = kernel();
    release(&kernel, &dispatcher(D), &[]).unwrap();
    assert!(events(&kernel, "ReleaseEvent").is_empty());
}

/// ★ A close releases the claim and records it, reason `closed`, and the close's status event
/// carries its committer: who closed it, not only when.
#[test]
fn a_release_by_close_records_reason_closed_and_the_closer() {
    let kernel = kernel();
    claim(&kernel, &worker(A), &[("content", "lawnboy")]).unwrap();
    close(&kernel, &dispatcher(D), &[]).unwrap();

    let releases = events(&kernel, "ReleaseEvent");
    assert_eq!(releases.len(), 1, "{releases:?}");
    let event = &releases[0];
    assert_eq!(event["claimedBy"], "lawnboy");
    assert_eq!(event["claimPrincipal"], A);
    assert_eq!(event["committer"], D);
    assert_eq!(event["releaseReason"], "closed");

    let status = events(&kernel, "StatusEvent");
    assert_eq!(status.len(), 1);
    assert_eq!(status[0]["committer"], D);
    assert_eq!(status[0]["status"], format!("{NS}closed"));

    // A second close finds no claim and no open item: it records neither again.
    close(&kernel, &dispatcher(D), &[("reason", "wontfix")]).unwrap();
    assert_eq!(events(&kernel, "ReleaseEvent").len(), 1);
    assert_eq!(events(&kernel, "StatusEvent").len(), 1);
}

/// A close of an unclaimed item releases nothing and records no release.
#[test]
fn a_close_of_a_free_item_records_no_release() {
    let kernel = kernel();
    close(&kernel, &dispatcher(D), &[]).unwrap();
    assert!(events(&kernel, "ReleaseEvent").is_empty());
}

// ------------------------------------------------------------------------ hand-back

/// ★ A hand-back names the holder label at the time, which is what tells two workers apart
/// that share one principal. An ordinary comment names nobody.
#[test]
fn a_handback_names_the_holder() {
    let kernel = kernel();
    claim(&kernel, &worker(A), &[("content", "lawnboy-claude")]).unwrap();
    try_as(
        &kernel,
        &worker(A),
        Verb::Sink,
        &at("comment"),
        &[("item", "1"), ("content", "a note")],
    )
    .unwrap();
    try_as(
        &kernel,
        &worker(A),
        Verb::Sink,
        &at("comment"),
        &[
            ("item", "1"),
            ("content", "Evidence: PR 64"),
            ("handback", "true"),
        ],
    )
    .unwrap();
    let named = rows(
        &kernel,
        &format!(
            "SELECT ?body ?by WHERE {{ GRAPH <{LIVE}> {{ ?c a <{NS}Comment> ; <{NS}body> ?body \
             OPTIONAL {{ ?c <{NS}handedBackBy> ?by }} }} }} ORDER BY ?body"
        ),
    );
    assert_eq!(named.len(), 2, "{named:?}");
    assert_eq!(named[0]["body"], "Evidence: PR 64");
    assert_eq!(named[0]["by"], "lawnboy-claude");
    assert_eq!(named[1]["body"], "a note");
    assert!(!named[1].contains_key("by"), "{named:?}");
}

/// ★ The explicit one-time backfill recovers the holder from a legacy hand-back whose text
/// names its worker the way gonk's loop wrote it, leaves every other comment alone, and
/// recovers nothing when run again.
#[test]
fn the_backfill_recovers_the_holder_from_a_legacy_handback() {
    let kernel = kernel();
    for text in [
        "gonk worker `lawnboy-claude` handed this back: PR 61 is green",
        "gonk worker `lawnboy` handed this back",
        "Evidence: a hand-back that names no worker",
        "gonk worker `` handed this back",
    ] {
        try_verb(
            &kernel,
            Verb::Sink,
            &at("comment"),
            &[("item", "1"), ("content", text)],
        )
        .unwrap();
    }
    let (count, update) = ikigai_ledger::attribution::handed_back_by_backfill(LIVE);
    let recovered = |kernel: &Kernel| rows(kernel, &count)[0]["recovered"].clone();
    assert_eq!(recovered(&kernel), "2");
    sink(&kernel, "urn:iki:store:update", &[("content", &update)]);
    let named = rows(
        &kernel,
        &format!(
            "SELECT ?by WHERE {{ GRAPH <{LIVE}> {{ ?c <{NS}handedBackBy> ?by }} }} ORDER BY ?by"
        ),
    );
    let named: Vec<&str> = named.iter().map(|r| r["by"].as_str()).collect();
    assert_eq!(named, ["lawnboy", "lawnboy-claude"]);
    assert_eq!(recovered(&kernel), "0", "a second run recovers nothing");
}

/// A hand-back on an item nobody holds names nobody.
#[test]
fn a_handback_of_an_unclaimed_item_names_nobody() {
    let kernel = kernel();
    try_as(
        &kernel,
        &worker(A),
        Verb::Sink,
        &at("comment"),
        &[
            ("item", "1"),
            ("content", "Evidence: PR 64"),
            ("handback", "true"),
            ("key", "hb-1"),
        ],
    )
    .unwrap();
    let named = rows(
        &kernel,
        &format!("SELECT ?by WHERE {{ GRAPH <{LIVE}> {{ ?c <{NS}handedBackBy> ?by }} }}"),
    );
    assert!(named.is_empty(), "{named:?}");
}

// --------------------------------------------------------------------------- edits

/// One edit: the verb, the resource and its arguments.
type Edit<'a> = (Verb, String, Vec<(&'a str, &'a str)>);

/// ★ Every edit that creates no node records who made it: one edit event per write, naming
/// the edit and its committer.
#[test]
fn every_edit_that_creates_no_node_records_its_committer() {
    let kernel = kernel();
    let item = item_iri(&kernel);
    sink(&kernel, &at("append"), &[("content", "Another")]);
    let d = dispatcher(D);
    let edits: [Edit; 9] = [
        (
            Verb::Sink,
            at("label"),
            vec![("item", "1"), ("content", "p1")],
        ),
        (
            Verb::Delete,
            at("label"),
            vec![("item", "1"), ("content", "p1")],
        ),
        (
            Verb::Sink,
            at("link"),
            vec![("item", "1"), ("content", "2"), ("type", "blocks")],
        ),
        (
            Verb::Delete,
            at("link"),
            vec![("item", "1"), ("content", "2"), ("type", "blocks")],
        ),
        (Verb::Sink, at("defer"), vec![("item", "1")]),
        (Verb::Delete, at("defer"), vec![("item", "1")]),
        (
            Verb::Sink,
            at("item:1:about"),
            vec![("content", "urn:example:file")],
        ),
        (Verb::Sink, at("item:1"), vec![("priority", "2")]),
        (
            Verb::Sink,
            at("item:1:state"),
            vec![("from", "filed"), ("to", "queued")],
        ),
    ];
    for (verb, iri, args) in &edits {
        try_as(&kernel, &d, *verb, iri, args).unwrap_or_else(|e| panic!("{iri}: {e}"));
    }
    let recorded = events(&kernel, "EditEvent");
    let kinds: Vec<&str> = recorded.iter().map(|e| e["editKind"].as_str()).collect();
    assert_eq!(
        kinds,
        ["label", "unlabel", "link", "unlink", "defer", "resume", "about", "edit", "state"]
    );
    for event in &recorded {
        // What each edit changed is `tests/edit_event_objects.rs`'s.
        let bookkeeping: Vec<&str> = keys(event)
            .into_iter()
            .filter(|key| {
                !key.starts_with("state")
                    && !["editLabel", "editTarget", "editLinkType", "editField"].contains(key)
            })
            .collect();
        assert_eq!(
            bookkeeping,
            [
                "committedAt",
                "committer",
                "created",
                "editKind",
                "onItem",
                "type"
            ]
        );
        assert_eq!(event["committer"], D);
        assert_eq!(event["onItem"], item);
    }
}

/// A transition that loses its compare-and-set changes nothing, and records nothing.
#[test]
fn a_refused_transition_records_no_edit() {
    let kernel = kernel();
    let refused = try_as(
        &kernel,
        &dispatcher(D),
        Verb::Sink,
        &at("item:1:state"),
        &[("from", "queued"), ("to", "reviewed")],
    );
    assert!(matches!(refused, Err(Error::Conflict(_))), "{refused:?}");
    assert!(events(&kernel, "EditEvent").is_empty());
}

// ------------------------------------------------------------------ delete owns them

/// Events hang off their item, so deleting it takes them with it, as it takes comments.
#[test]
fn a_delete_takes_the_events_with_the_item() {
    let kernel = kernel();
    claim(&kernel, &worker(A), &[("content", "lawnboy")]).unwrap();
    try_verb(
        &kernel,
        Verb::Sink,
        &at("label"),
        &[("item", "1"), ("content", "x")],
    )
    .unwrap();
    close(&kernel, &dispatcher(D), &[]).unwrap();
    try_verb(&kernel, Verb::Delete, &at("item:1"), &[]).unwrap();
    for class in ["ClaimEvent", "ReleaseEvent", "EditEvent", "StatusEvent"] {
        assert!(
            events(&kernel, class).is_empty(),
            "{class} survived the delete"
        );
    }
}

/// ★ An item worked on for a while still deletes. Every claim, release and edit now adds an
/// event to the item, and until 0.15.0 a delete's commit guard listed the quads it read one
/// `VALUES` row per line, which the store refused past about twenty extra triples as too
/// ambiguous to scan (`MAX_SCAN_BRANCHES`), whatever the item carried. Literals here hold
/// what that scan reads specially: `#`, `<`, quotes, parentheses and newlines.
#[test]
fn an_item_with_hundreds_of_quads_still_deletes() {
    let kernel = kernel();
    for n in 0..40 {
        claim(&kernel, &worker(A), &[("content", "lawnboy")]).unwrap();
        release(&kernel, &worker(A), &[("from", "lawnboy")]).unwrap();
        try_verb(
            &kernel,
            Verb::Sink,
            &at("label"),
            &[("item", "1"), ("content", &format!("l{n}#'<x>"))],
        )
        .unwrap();
        try_verb(
            &kernel,
            Verb::Sink,
            &at("comment"),
            &[
                ("item", "1"),
                ("content", &format!("#{n} <b> 'q' (\"r\") )\n( < # \\ end")),
            ],
        )
        .unwrap();
    }
    let answer = try_verb(&kernel, Verb::Delete, &at("item:1"), &[]).unwrap();
    assert!(answer.contains("recoverable"), "{answer}");
    let left = rows(
        &kernel,
        &format!("SELECT ?s WHERE {{ GRAPH <{LIVE}> {{ ?s <{NS}onItem> ?item }} }}"),
    );
    assert!(
        left.is_empty(),
        "{} owned nodes survived the delete",
        left.len()
    );
}

// ----------------------------------------------------------------- the agent query

/// ★ What the gonk dashboard asks: how many items has each holder claimed, released and
/// handed back. One query over the events, after every claim is gone.
#[test]
fn what_an_agent_did_is_one_query_after_the_claims_are_gone() {
    let kernel = kernel();
    sink(&kernel, &at("append"), &[("content", "Two")]);
    claim(&kernel, &worker(A), &[("content", "lawnboy-claude")]).unwrap();
    claim(
        &kernel,
        &worker(A),
        &[("item", "2"), ("content", "lawnboy")],
    )
    .unwrap();
    close(&kernel, &dispatcher(D), &[]).unwrap();
    close(&kernel, &dispatcher(D), &[("item", "2")]).unwrap();
    let counted = rows(
        &kernel,
        &format!(
            "SELECT ?holder (COUNT(DISTINCT ?item) AS ?n) WHERE {{ GRAPH <{LIVE}> {{ \
             ?e a <{NS}ClaimEvent> ; <{NS}claimedBy> ?holder ; <{NS}onItem> ?item }} }} \
             GROUP BY ?holder ORDER BY ?holder"
        ),
    );
    let counted: Vec<(String, String)> = counted
        .into_iter()
        .map(|r| (r["holder"].clone(), r["n"].clone()))
        .collect();
    assert_eq!(
        counted,
        [
            ("lawnboy".to_string(), "1".to_string()),
            ("lawnboy-claude".to_string(), "1".to_string())
        ]
    );
}

// ------------------------------------------------------------------- import closedAt

fn importer() -> Capability {
    dispatcher(IMPORTER)
}

/// ★ An import principal may date a close at its source: `closedAt`, the status event `counts`
/// reads, and `closedAtSource "import"` on both.
#[test]
fn an_importer_dates_a_close_and_counts_place_it_on_that_day() {
    let kernel = kernel();
    try_as(
        &kernel,
        &importer(),
        Verb::Sink,
        &at("append"),
        &[("content", "Imported"), ("created", "2026-01-02T03:04:05Z")],
    )
    .unwrap();
    let answer = try_as(
        &kernel,
        &importer(),
        Verb::Sink,
        &at("close"),
        &[("item", "2"), ("closed-at", "2026-01-05T10:00:00Z")],
    )
    .unwrap();
    assert_eq!(answer, "closed acme#2 (done)\n");
    let item = rows(
        &kernel,
        &format!(
            "SELECT ?at ?source WHERE {{ GRAPH <{LIVE}> {{ ?i <{NS}number> 2 ; \
             <{NS}closedAt> ?at ; <{NS}closedAtSource> ?source }} }}"
        ),
    );
    assert_eq!(item.len(), 1, "{item:?}");
    assert!(item[0]["at"].starts_with("2026-01-05T10:00:00"), "{item:?}");
    assert_eq!(item[0]["source"], "import");
    let status = events(&kernel, "StatusEvent");
    assert_eq!(status.len(), 1);
    assert!(
        status[0]["created"].starts_with("2026-01-05T10:00:00"),
        "{status:?}"
    );
    assert_eq!(status[0]["closedAtSource"], "import");
    assert_eq!(status[0]["committer"], IMPORTER);
    assert_ne!(status[0]["committedAt"], status[0]["created"]);

    let csv = source(&kernel, &at("counts"), &[]);
    assert!(csv.contains("2026-01-04,1,0\n"), "{csv}");
    assert!(csv.contains("2026-01-05,0,1\n"), "{csv}");
}

/// Anyone else sending `closed-at` is refused with the reason and the remedy, and nothing
/// closes.
#[test]
fn a_closed_at_from_anyone_else_is_refused_with_the_remedy() {
    let kernel = kernel();
    for who in [dispatcher(D), Capability::root()] {
        match close(&kernel, &who, &[("closed-at", "2026-01-05T10:00:00Z")]) {
            Err(Error::Denied(message)) => {
                for word in ["closed-at", "import or sync principal", "Send it without"] {
                    assert!(message.contains(word), "{word:?} is not in {message}");
                }
            }
            other => panic!("expected a denial, got {other:?}"),
        }
    }
    assert!(events(&kernel, "StatusEvent").is_empty(), "nothing closed");
}

/// An importer's close date is no later than the import and no earlier than the filing.
#[test]
fn an_imported_close_date_must_fall_between_filing_and_now() {
    let kernel = kernel();
    for date in ["2999-01-01T00:00:00Z", "2001-01-01T00:00:00Z", "yesterday"] {
        match close(&kernel, &importer(), &[("closed-at", date)]) {
            Err(Error::InvalidArgument { name, .. }) => assert_eq!(name, "closed-at"),
            other => panic!("{date}: expected a refusal, got {other:?}"),
        }
    }
    assert!(events(&kernel, "StatusEvent").is_empty(), "nothing closed");
}

// ------------------------------------------------------------------- the vocabulary

/// No property an event carries is declared, by `rdfs:domain`, to belong to an item, or a
/// reasoning consumer would count every claim event as an item.
#[test]
fn no_event_property_infers_an_item() {
    let kernel = kernel();
    claim(&kernel, &worker(A), &[("content", "w"), ("lease", "1s")]).unwrap();
    claim(
        &kernel,
        &worker(B),
        &[("content", "v"), ("takeover", "true"), ("from", "w")],
    )
    .unwrap();
    try_verb(
        &kernel,
        Verb::Sink,
        &at("label"),
        &[("item", "1"), ("content", "x")],
    )
    .unwrap();
    close(&kernel, &dispatcher(D), &[]).unwrap();
    let mut domains = BTreeMap::new();
    for quad in oxrdfio::RdfParser::from_format(oxrdfio::RdfFormat::Turtle)
        .for_reader(ikigai_ledger::VOCABULARY_TTL.as_bytes())
    {
        let quad = quad.expect("the vocabulary parses");
        if quad.predicate.as_str() == "http://www.w3.org/2000/01/rdf-schema#domain" {
            domains.insert(quad.subject.to_string(), quad.object.to_string());
        }
    }
    for class in ["ClaimEvent", "ReleaseEvent", "EditEvent", "StatusEvent"] {
        for row in rows(
            &kernel,
            &format!(
                "SELECT DISTINCT ?p WHERE {{ GRAPH <{LIVE}> {{ ?e a <{NS}{class}> ; ?p ?o }} }}"
            ),
        ) {
            let p = format!("<{}>", row["p"]);
            assert!(
                !domains.contains_key(&p),
                "{class} carries {p}, whose rdfs:domain is {}",
                domains[&p]
            );
        }
    }
}

/// `ns-drift` compares the published vocabulary's `owl:versionInfo` with this repository's,
/// so the line must name this crate's version.
#[test]
fn the_vocabulary_names_this_version_for_ns_drift() {
    let line = format!("    owl:versionInfo \"{}\" .", env!("CARGO_PKG_VERSION"));
    let found: Vec<&str> = ikigai_ledger::VOCABULARY_TTL
        .lines()
        .filter(|l| l.trim_start().starts_with("owl:versionInfo"))
        .collect();
    assert_eq!(found, [line.as_str()]);
}

// ------------------------------------------------------------ concurrent claims

/// Holds the next `width` live-graph updates until all have arrived, so every claim has read
/// the item free before any writes.
#[derive(Default)]
struct Gate {
    gated: Mutex<usize>,
    arrived: AtomicUsize,
    width: AtomicUsize,
}

struct Gated {
    inner: Arc<dyn Space>,
    gate: Arc<Gate>,
}

struct GatedEndpoint {
    inner: Arc<dyn Endpoint>,
    gate: Arc<Gate>,
}

impl Space for Gated {
    fn resolve(&self, request: &Request, scope: &Scope) -> Resolution {
        let gate = Arc::clone(&self.gate);
        self.inner
            .resolve(request, scope)
            .map_endpoint(move |inner| Arc::new(GatedEndpoint { inner, gate }) as Arc<dyn Endpoint>)
    }
}

#[async_trait]
impl Endpoint for GatedEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> ikigai_core::Result<Representation> {
        let to_live = inv.request.target.as_str() == "urn:iki:store:graph-update"
            && inv.inline_str("graph").map(|g| g == LIVE).unwrap_or(false);
        if to_live {
            let held = {
                let mut gated = self.gate.gated.lock().unwrap();
                let held = *gated > 0;
                *gated = gated.saturating_sub(1);
                held
            };
            if held {
                self.gate.arrived.fetch_add(1, Ordering::SeqCst);
                let width = self.gate.width.load(Ordering::SeqCst);
                futures::future::poll_fn(|cx| {
                    if self.gate.arrived.load(Ordering::SeqCst) >= width {
                        Poll::Ready(())
                    } else {
                        cx.waker().wake_by_ref();
                        Poll::Pending
                    }
                })
                .await;
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

/// ★ Concurrent claims of one free item, every one of them reading it free before any
/// writes: one claim wins, and one claim event is written.
#[test]
fn concurrent_claims_of_a_free_item_leave_one_claim_event() {
    const CLAIMS: usize = 4;
    let gate = Arc::new(Gate::default());
    let store = Arc::new(ikigai_store::space(DurableStore::in_memory().unwrap())) as Arc<dyn Space>;
    let space = Fallback::new(vec![
        Arc::new(Gated {
            inner: store,
            gate: Arc::clone(&gate),
        }) as Arc<dyn Space>,
        Arc::new(ikigai_ledger::space()) as Arc<dyn Space>,
    ]);
    let kernel =
        Kernel::with_meta_renderer(Arc::new(space), Arc::new(ikigai_vocab::TurtleRenderer))
            .with_clock(Arc::new(TickingClock::default()));
    sink(&kernel, &at("append"), &[("content", "Contended")]);
    gate.width.store(CLAIMS, Ordering::SeqCst);
    *gate.gated.lock().unwrap() = CLAIMS;
    let holders: Vec<String> = (1..=CLAIMS).map(|k| format!("worker-{k}")).collect();
    let root = Capability::root();
    let answers = block_on(futures::future::join_all(holders.iter().map(|holder| {
        let request = [("item", "1"), ("content", holder.as_str())].iter().fold(
            Request::new(Verb::Sink, Iri::parse(at("claim")).unwrap()),
            |r, (name, value)| r.with_arg(*name, ArgRef::Inline(value.as_bytes().to_vec())),
        );
        kernel.issue(request, &root)
    })));
    assert_eq!(
        gate.arrived.load(Ordering::SeqCst),
        CLAIMS,
        "every claim reached its update before any wrote"
    );
    let won = answers.iter().filter(|a| a.is_ok()).count();
    assert_eq!(won, 1, "one claim wins");
    let claims = events(&kernel, "ClaimEvent");
    assert_eq!(claims.len(), 1, "one claim event: {claims:?}");
    let holder = rows(
        &kernel,
        &format!(
            "SELECT ?h WHERE {{ GRAPH <{LIVE}> {{ ?i <{NS}number> 1 ; <{NS}claimedBy> ?h }} }}"
        ),
    );
    assert_eq!(
        claims[0]["claimedBy"], holder[0]["h"],
        "the event names the winner"
    );
}
