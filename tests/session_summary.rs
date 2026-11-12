//! **An agent session's summary on its hand-back** (ledger #3022).
//!
//! A worker hands an item back with `comment handback=true` and then records its session's
//! content-free summary at `…:item:{id}:sessions`, under `urn:cap:ledger:session:{ledger}`,
//! which the published worker grant now carries. These tests run under EXACTLY those lines:
//!
//! - the summary is recorded on the newest hand-back and read back, newest first, in text,
//!   Turtle and JSON;
//! - a summary the closed `ledger:SessionSummaryShape` refuses (a transcript, a cost, a wrong
//!   harness, a negative count, a missing field) is `InvalidArgument`, writes nothing, and the
//!   hand-back still stands;
//! - the worker still cannot write `about`, and without a hand-back there is nothing to record
//!   a session on.

mod common;

use common::*;
use ikigai_core::{Capability, Error, Kernel, Verb};

const LEDGER: &str = "acme";

fn at(resource: &str) -> String {
    format!("urn:iki:ledger:{LEDGER}:{resource}")
}

/// The worker grant `README.md` publishes, line for line: read, claim, comment, session, and
/// the two store grants. No write, no close, no about.
fn worker() -> Capability {
    Capability::scoped(vec![
        format!("urn:cap:ledger:read:{LEDGER}"),
        format!("urn:cap:ledger:claim:{LEDGER}"),
        format!("urn:cap:ledger:comment:{LEDGER}"),
        format!("urn:cap:ledger:session:{LEDGER}"),
        graph_read(LEDGER),
        graph_write(LEDGER),
    ])
}

/// The worker grant as it was before the session line.
fn old_worker() -> Capability {
    Capability::scoped(vec![
        format!("urn:cap:ledger:read:{LEDGER}"),
        format!("urn:cap:ledger:claim:{LEDGER}"),
        format!("urn:cap:ledger:comment:{LEDGER}"),
        graph_read(LEDGER),
        graph_write(LEDGER),
    ])
}

const SUMMARY: &str = r#"{
  "session": "0b5e-41aa_7.c",
  "machineName": "lawnboy",
  "harness": "claude",
  "model": "claude-opus-5-5",
  "exitStatus": 0,
  "wallClockMs": 1830000,
  "turns": 41,
  "toolCalls": 118,
  "inputTokens": 5120,
  "cachedInputTokens": 880000,
  "cacheCreationTokens": 64000,
  "outputTokens": 21000
}"#;

/// A ledger with item #1, claimed and handed back by the worker.
fn handed_back() -> Kernel {
    let kernel = kernel();
    sink(&kernel, &at("append"), &[("content", "Fix the widget")]);
    let w = worker();
    try_as(
        &kernel,
        &w,
        Verb::Sink,
        &at("claim"),
        &[("item", "1"), ("content", "worker-1"), ("lease", "1h")],
    )
    .expect("a worker claims");
    try_as(
        &kernel,
        &w,
        Verb::Sink,
        &at("comment"),
        &[("item", "1"), ("content", "PR is up"), ("handback", "true")],
    )
    .expect("a worker hands back");
    kernel
}

fn record(kernel: &Kernel, capability: &Capability, summary: &str) -> Result<String, Error> {
    try_as(
        kernel,
        capability,
        Verb::Sink,
        &at("item:1:sessions"),
        &[("content", summary)],
    )
}

fn sessions(kernel: &Kernel, face: &str) -> String {
    try_as(
        kernel,
        &worker(),
        Verb::Source,
        &at("item:1:sessions"),
        &[("as", face)],
    )
    .expect("a worker reads the sessions")
}

fn invalid(result: Result<String, Error>, mentions: &str) {
    match result {
        Err(Error::InvalidArgument { name, detail }) => {
            assert_eq!(name, "content");
            assert!(detail.contains(mentions), "{detail}");
        }
        other => panic!("expected InvalidArgument mentioning {mentions}, got {other:?}"),
    }
}

fn awaiting(kernel: &Kernel) -> bool {
    source(kernel, &at("item:1"), &[("as", "application/json")]).contains("\"dispatcher\"")
}

/// ★ Under exactly the published worker grant: hand back, record, read in three faces.
#[test]
fn a_worker_records_its_session_on_its_hand_back_and_reads_it_back() {
    let kernel = handed_back();
    let said = record(&kernel, &worker(), SUMMARY).expect("the worker grant records a session");
    assert_eq!(
        said,
        "recorded session 0b5e-41aa_7.c on acme#1's hand-back\n"
    );

    let text = sessions(&kernel, "text/plain");
    assert!(
        text.starts_with("acme#1: 1 session(s), newest first\n"),
        "{text}"
    );
    assert!(
        text.contains("harness: claude, model: claude-opus-5-5, machine: lawnboy"),
        "{text}"
    );
    assert!(
        text.contains("exit status: 0, wall clock: 1830000 ms"),
        "{text}"
    );
    assert!(text.contains("turns: 41, tool calls: 118"), "{text}");
    assert!(
        text.contains(
            "tokens: input 5120, cached input 880000, cache creation 64000, output 21000"
        ),
        "{text}"
    );
    assert!(
        text.contains("iri: urn:agent:session:id:0b5e-41aa_7.c"),
        "{text}"
    );

    let turtle = sessions(&kernel, "text/turtle");
    assert!(
        turtle.contains("urn:agent:session:id:0b5e-41aa_7.c"),
        "{turtle}"
    );
    assert!(turtle.contains("SessionSummary"), "{turtle}");
    assert!(
        turtle.contains("urn:agents:session:0b5e-41aa_7.c"),
        "the legacy alias: {turtle}"
    );

    let json: serde_json::Value =
        serde_json::from_str(&sessions(&kernel, "application/json")).unwrap();
    let first = &json["sessions"][0];
    assert_eq!(first["session"], "0b5e-41aa_7.c");
    assert_eq!(first["iri"], "urn:agent:session:id:0b5e-41aa_7.c");
    assert_eq!(first["outputTokens"], 21000);
    assert_eq!(first["harness"], "claude");
    assert!(first["handBackEvent"]
        .as_str()
        .unwrap()
        .contains(":comment:"));
    assert!(first.get("cost").is_none());

    // A resend of the same session on the same hand-back writes nothing.
    let again = record(&kernel, &worker(), SUMMARY).expect("a resend is answered");
    assert!(again.contains("already recorded"), "{again}");
    assert!(sessions(&kernel, "text/plain").starts_with("acme#1: 1 session(s)"));
}

/// ★ The closed shape refuses what a summary must never hold, and the hand-back stands.
#[test]
fn a_summary_the_shape_refuses_writes_nothing_and_the_hand_back_stands() {
    let kernel = handed_back();
    let with = |extra: &str| SUMMARY.replacen("{", &format!("{{ {extra},"), 1);
    invalid(
        record(&kernel, &worker(), &with(r#""transcript": "user: fix it""#)),
        "ledger:transcript is not part of a session summary",
    );
    invalid(
        record(&kernel, &worker(), &with(r#""costUsd": 3"#)),
        "ledger:costUsd is not part of a session summary",
    );
    invalid(
        record(
            &kernel,
            &worker(),
            &SUMMARY.replace("\"claude\"", "\"gpt\""),
        ),
        "ledger:harness must be one of claude, codex",
    );
    invalid(
        record(&kernel, &worker(), &SUMMARY.replace("41,", "-1,")),
        "ledger:turns must be 0 or more",
    );
    invalid(
        record(
            &kernel,
            &worker(),
            &SUMMARY.replace("\"exitStatus\": 0,", ""),
        ),
        "needs ledger:exitStatus",
    );
    invalid(
        record(&kernel, &worker(), &SUMMARY.replace("21000", "\"lots\"")),
        "ledger:outputTokens must be a whole number",
    );
    invalid(
        record(&kernel, &worker(), &with(r#""sessionItem": "x""#)),
        "`sessionItem` is not a key",
    );
    invalid(
        record(&kernel, &worker(), &SUMMARY.replace("0b5e-41aa_7.c", "a b")),
        "contains ` `",
    );
    invalid(record(&kernel, &worker(), "not json"), "not JSON");
    assert_eq!(
        sessions(&kernel, "text/plain"),
        "acme#1 has no recorded sessions\n"
    );
    assert!(awaiting(&kernel), "the hand-back still stands");
}

/// The worker grant gained the session line and nothing else: `about` is still refused, and
/// the grant as it was before cannot record a session.
#[test]
fn the_session_grant_is_narrow() {
    let kernel = handed_back();
    match try_as(
        &kernel,
        &worker(),
        Verb::Sink,
        &at("item:1:about"),
        &[("content", "urn:agents:session:x")],
    ) {
        Err(Error::Denied(_)) => {}
        other => panic!("the worker grant must not write about: {other:?}"),
    }
    match record(&kernel, &old_worker(), SUMMARY) {
        Err(Error::Denied(why)) => assert!(why.contains("urn:cap:ledger:session:acme"), "{why}"),
        other => panic!("expected Denied, got {other:?}"),
    }
    let session_only = Capability::scoped(vec![
        format!("urn:cap:ledger:session:{LEDGER}"),
        graph_read(LEDGER),
        graph_write(LEDGER),
    ]);
    match try_as(
        &kernel,
        &session_only,
        Verb::Sink,
        &at("comment"),
        &[("item", "1"), ("content", "hi")],
    ) {
        Err(Error::Denied(_)) => {}
        other => panic!("the session grant must not comment: {other:?}"),
    }
    record(&kernel, &session_only, SUMMARY).expect("the session grant alone records");
}

/// No hand-back, no session: the summary is written on a hand-back event or not at all.
#[test]
fn without_a_hand_back_there_is_nothing_to_record_on() {
    let kernel = kernel();
    sink(&kernel, &at("append"), &[("content", "Fix the widget")]);
    match record(&kernel, &worker(), SUMMARY) {
        Err(Error::Conflict(why)) => assert!(why.contains("no hand-back"), "{why}"),
        other => panic!("expected Conflict, got {other:?}"),
    }
}

/// Newest first, and one session cannot move to another hand-back.
#[test]
fn sessions_read_newest_first_and_a_session_hands_back_once() {
    let kernel = handed_back();
    record(&kernel, &worker(), SUMMARY).unwrap();
    try_as(
        &kernel,
        &worker(),
        Verb::Sink,
        &at("comment"),
        &[("item", "1"), ("content", "again"), ("handback", "true")],
    )
    .unwrap();
    let second = SUMMARY
        .replace("0b5e-41aa_7.c", "second")
        .replace("claude\"", "codex\"");
    record(&kernel, &worker(), &second).unwrap();
    let text = sessions(&kernel, "text/plain");
    let newer = text.find("session second").expect("the second session");
    let older = text
        .find("session 0b5e-41aa_7.c")
        .expect("the first session");
    assert!(newer < older, "newest first: {text}");
    match record(&kernel, &worker(), SUMMARY) {
        Err(Error::Conflict(why)) => assert!(why.contains("another hand-back"), "{why}"),
        other => panic!("expected Conflict, got {other:?}"),
    }
}
