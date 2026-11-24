//! Commit order never depends on a host clock (ledger #4199).
//!
//! Two independently clocked hosts share one store. Through ledger #4199 this file was the
//! research proof (`docs/research/clock-ordering.md`): a comment written SECOND, from the host
//! whose clock runs a minute slow, listed FIRST, because comments were ordered by their
//! authored `created`. Now every comment and event carries `ledger:itemRevision`, allocated by
//! the store in the update that writes it, and every authoritative order reads that: the
//! item's comments, `comments=last`, its history and the hand-back a session summary attaches
//! to. The tests below are the regression, a restart, and an item that mixes records written
//! before revisions with ones written after.
mod common;

use common::*;
use ikigai_core::{Capability, Clock, Fallback, Kernel, Space, Time, Verb};
use ikigai_store::DurableStore;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

/// The fast host's clock, 2026-09-15 00:01:00 UTC, and the slow host's, a minute earlier.
const FAST: u64 = 1_789_430_460_000;
const SLOW: u64 = 1_789_430_400_000;

struct Fixed(u64);
impl Clock for Fixed {
    fn now(&self) -> Time {
        Time::from_millis(self.0)
    }
}
fn host(store: DurableStore, time: u64) -> Kernel {
    let spaces = Fallback::new(vec![
        Arc::new(ikigai_store::space(store)) as Arc<dyn Space>,
        Arc::new(ikigai_ledger::space()) as Arc<dyn Space>,
    ]);
    Kernel::with_meta_renderer(Arc::new(spaces), Arc::new(ikigai_vocab::TurtleRenderer))
        .with_clock(Arc::new(Fixed(time)))
}

fn comment(kernel: &Kernel, content: &str) {
    sink(
        kernel,
        "urn:iki:ledger:comment",
        &[("item", "#1"), ("content", content)],
    );
}

/// Where each of `bodies` is in `text`, which must hold them in this order.
fn in_order(text: &str, bodies: &[&str]) {
    let at: Vec<usize> = bodies
        .iter()
        .map(|body| text.find(body).unwrap_or_else(|| panic!("{body}: {text}")))
        .collect();
    assert!(at.windows(2).all(|w| w[0] < w[1]), "{bodies:?}: {text}");
}

/// The JSON face of #1, `comments=all` or `comments=last`.
fn item_json(kernel: &Kernel, comments: &str) -> Value {
    let text = source(
        kernel,
        "urn:iki:ledger:items",
        &[("as", "application/json"), ("comments", comments)],
    );
    let doc: Value = serde_json::from_str(&text).unwrap();
    doc["items"][0].clone()
}

#[test]
fn a_later_comment_from_a_slow_host_is_listed_last() {
    let store = DurableStore::in_memory().unwrap();
    let fast = host(store.clone(), FAST);
    let slow = host(store, SLOW);
    append(&fast, "clock skew proof", &[]);
    comment(&fast, "first write");
    comment(&slow, "second write");
    // Both faces that list an item's comments in order: the plain detail (`load_comments`)
    // and the JSON listing (`load_comments_for`).
    in_order(
        &source(&fast, "urn:iki:ledger:item:1", &[]),
        &["first write", "second write"],
    );
    let item = item_json(&fast, "all");
    let comments = item["comments"].as_array().unwrap();
    assert_eq!(comments[0]["text"], "first write", "{item}");
    assert_eq!(comments[1]["text"], "second write", "{item}");
    // The authored times still say what each host's clock said: wall time is kept for
    // display, and only the revision orders.
    assert_eq!(comments[0]["time"], "2026-09-15T00:01:00.000Z");
    assert_eq!(comments[1]["time"], "2026-09-15T00:00:00.000Z");
    assert_eq!(comments[0]["item_revision"], 1, "{item}");
    assert_eq!(comments[1]["item_revision"], 2, "{item}");
    assert!(comments[0].get("order").is_none(), "{item}");
    // ★ `comments=last` is the last committed, not the latest stamp.
    let last = item_json(&fast, "last");
    assert_eq!(last["comments"][0]["text"], "second write", "{last}");
}

#[test]
fn history_lists_a_slow_hosts_later_event_after_the_fast_hosts() {
    let store = DurableStore::in_memory().unwrap();
    let fast = host(store.clone(), FAST);
    let slow = host(store, SLOW);
    append(&fast, "clock skew proof", &[]);
    sink(
        &fast,
        "urn:iki:ledger:claim",
        &[("item", "#1"), ("content", "worker-1"), ("lease", "1h")],
    );
    delete(&slow, "urn:iki:ledger:claim", &[("item", "#1")]);
    sink(&slow, "urn:iki:ledger:close", &[("item", "#1")]);
    let text = source(
        &fast,
        "urn:iki:ledger:item:1:history",
        &[("as", "application/json")],
    );
    let doc: Value = serde_json::from_str(&text).unwrap();
    let kinds: Vec<&str> = doc["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["claim", "release", "closed"], "{text}");
    let revisions: Vec<u64> = doc["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["item_revision"].as_u64().unwrap())
        .collect();
    assert!(revisions.windows(2).all(|w| w[0] < w[1]), "{text}");
}

/// A worker grant over the default ledger, enough to hand back and record a session.
fn worker() -> Capability {
    Capability::scoped(vec![
        "urn:cap:ledger:read:default".to_string(),
        "urn:cap:ledger:claim:default".to_string(),
        "urn:cap:ledger:comment:default".to_string(),
        "urn:cap:ledger:session:default".to_string(),
        graph_read("default"),
        graph_write("default"),
        session_read("default"),
        session_write("default"),
    ])
}

#[test]
fn a_session_attaches_to_the_last_committed_hand_back() {
    let store = DurableStore::in_memory().unwrap();
    let fast = host(store.clone(), FAST);
    let slow = host(store, SLOW);
    append(&fast, "clock skew proof", &[]);
    try_as(
        &fast,
        &worker(),
        Verb::Sink,
        "urn:iki:ledger:claim",
        &[("item", "1"), ("content", "worker-1"), ("lease", "1h")],
    )
    .unwrap();
    for (kernel, words) in [(&fast, "first hand-back"), (&slow, "second hand-back")] {
        try_as(
            kernel,
            &worker(),
            Verb::Sink,
            "urn:iki:ledger:comment",
            &[("item", "1"), ("content", words), ("handback", "true")],
        )
        .unwrap();
    }
    let item = item_json(&fast, "all");
    let second = item["comments"][1]["id"].as_str().unwrap().to_string();
    assert_eq!(item["comments"][1]["text"], "second hand-back", "{item}");
    try_as(
        &slow,
        &worker(),
        Verb::Sink,
        "urn:iki:ledger:item:1:sessions",
        &[(
            "content",
            r#"{"session": "s1", "machineName": "m", "harness": "claude", "model": "x",
                "exitStatus": 0, "wallClockMs": 1, "turns": 1, "toolCalls": 1,
                "inputTokens": 1, "cachedInputTokens": 0, "cacheCreationTokens": 0,
                "outputTokens": 1}"#,
        )],
    )
    .unwrap();
    let sessions = try_as(
        &fast,
        &worker(),
        Verb::Source,
        "urn:iki:ledger:item:1:sessions",
        &[("as", "text/plain")],
    )
    .unwrap();
    assert!(sessions.contains(&second), "{sessions}");
}

fn scratch(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "ikigai-ledger-test-skew-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    path
}

/// ★ The revision is in the graph, not in a process: after a restart the next comment, from a
/// host whose clock runs a minute slow, takes the next revision and lists last.
#[test]
fn the_revision_continues_across_a_restart() {
    let path = scratch("restart");
    {
        let fast = host(DurableStore::open(&path).unwrap(), FAST);
        append(&fast, "clock skew proof", &[]);
        comment(&fast, "before the restart");
        comment(&fast, "also before");
    }
    {
        let slow = host(DurableStore::open(&path).unwrap(), SLOW);
        comment(&slow, "after the restart");
        in_order(
            &source(&slow, "urn:iki:ledger:item:1", &[]),
            &["before the restart", "also before", "after the restart"],
        );
        let item = item_json(&slow, "all");
        let revisions: Vec<u64> = item["comments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["item_revision"].as_u64().unwrap())
            .collect();
        assert_eq!(revisions, [1, 2, 3], "{item}");
        let last = item_json(&slow, "last");
        assert_eq!(last["comments"][0]["text"], "after the restart", "{last}");
    }
    let _ = std::fs::remove_dir_all(&path);
}

/// An item with comments written before ledger #4199 (no revision, here made by removing every
/// revision from the graph) and one written after: the legacy ones keep their old order, time
/// then IRI, and say so (`"order": "approximate"`); the new one lists after them all, though
/// its host's clock puts it earlier than both, and `comments=last` names it.
#[test]
fn legacy_comments_keep_their_order_ahead_of_new_ones() {
    let store = DurableStore::in_memory().unwrap();
    let fast = host(store.clone(), FAST);
    let slow = host(store, SLOW);
    append(&fast, "clock skew proof", &[]);
    comment(&fast, "legacy one");
    comment(&fast, "legacy two");
    sink(
        &fast,
        "urn:iki:store:update",
        &[(
            "content",
            "DELETE WHERE { GRAPH <urn:iki:ledger:graph:default> \
             { ?s <https://ikigai-rs.dev/ns/ledger#itemRevision> ?r } }",
        )],
    );
    // Before any new write the item is all legacy: today's order, labeled approximate, and
    // `comments=last` falls back to the latest stamp.
    let item = item_json(&fast, "all");
    for c in item["comments"].as_array().unwrap() {
        assert_eq!(c["order"], "approximate", "{item}");
        assert!(c.get("item_revision").is_none(), "{item}");
    }
    comment(&slow, "new one");
    in_order(
        &source(&fast, "urn:iki:ledger:item:1", &[]),
        &["legacy one", "legacy two", "new one"],
    );
    let item = item_json(&fast, "all");
    let comments = item["comments"].as_array().unwrap();
    assert_eq!(comments[0]["text"], "legacy one", "{item}");
    assert_eq!(comments[1]["text"], "legacy two", "{item}");
    assert_eq!(comments[2]["text"], "new one", "{item}");
    assert_eq!(comments[0]["order"], "approximate", "{item}");
    assert_eq!(comments[1]["order"], "approximate", "{item}");
    // No revision is invented for the legacy ones: the first allocated on the item is 1.
    assert_eq!(comments[2]["item_revision"], 1, "{item}");
    assert!(comments[2].get("order").is_none(), "{item}");
    let last = item_json(&fast, "last");
    assert_eq!(last["comments"][0]["text"], "new one", "{last}");
}
