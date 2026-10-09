//! The composition the brief actually asks for: a ledger over `DurableStore::open`.
//!
//! Everything else in this suite runs over the in-memory twin, which is right — every
//! property above the backing is the same. These are the three facts that are ONLY true
//! of the durable store, and each one is a thing an operator will meet:
//!
//! 1. the ledger survives a restart, which is the whole reason `ikigai-store` exists;
//! 2. **the short-id counter continues** rather than restarting at 1 — because it is a
//!    resource in the graph, not process state;
//! 3. a second process opening the same directory is refused **legibly**, since RocksDB
//!    permits one writer per directory and a panic out of its guts is not an answer.

mod common;

use std::path::PathBuf;

use common::*;
use ikigai_core::Verb;
use ikigai_store::DurableStore;

/// A scratch directory under the system temp dir, named for this test.
///
/// Deliberately not under `HOME`: a test that writes to the operator's data home is a
/// test that eats the operator's ledger.
fn scratch(name: &str) -> PathBuf {
    let path =
        std::env::temp_dir().join(format!("ikigai-ledger-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    path
}

#[test]
fn the_ledger_survives_a_restart_and_the_numbering_continues() {
    let path = scratch("restart");

    {
        let kernel = kernel_over(DurableStore::open(&path).expect("open the store"));
        assert_eq!(append(&kernel, "Filed before the restart", &[]), "#1");
        assert_eq!(append(&kernel, "And another", &[("priority", "1")]), "#2");
        sink(
            &kernel,
            "urn:iki:ledger:comment",
            &[("item", "#1"), ("content", "a note"), ("author", "brian")],
        );
        // The handle goes out of scope here, which is what releases RocksDB's lock — the
        // only way to release it, since `DurableStore::open` holds it for the life of the
        // process.
    }

    {
        let kernel = kernel_over(DurableStore::open(&path).expect("reopen the store"));
        // Evidence that this really is the durable backend and not the in-memory twin
        // wearing its name — the one thing that would make every assertion below vacuous.
        let info = source(&kernel, "urn:iki:store:info", &[]);
        assert!(info.contains("durable"), "{info}");
        assert!(
            info.contains("covered: true"),
            "reads stay cacheable: {info}"
        );

        let list = source(&kernel, "urn:iki:ledger:items", &[]);
        assert!(list.contains("Filed before the restart"), "{list}");
        assert!(list.contains("2 item(s)"), "{list}");

        let item = source(&kernel, "urn:iki:ledger:item:1", &[]);
        assert!(item.contains("a note"), "the comment survived too: {item}");

        // ★ The counter is a resource in the graph, so a restart continues where the last
        // writer stopped. Process state would have handed the next item `#1` and made two
        // different pieces of work share a name.
        assert_eq!(append(&kernel, "Filed after the restart", &[]), "#3");

        // And `next` works over the durable graph, which is the only combination the
        // in-memory suite cannot prove.
        let next = source(&kernel, "urn:iki:ledger:next", &[("limit", "1")]);
        assert!(next.contains("And another"), "p1 wins: {next}");
    }

    std::fs::remove_dir_all(&path).expect("clean up the scratch store");
}

/// ⚠ One writer per directory, and the refusal an operator sees says so.
#[test]
fn a_second_open_of_the_same_ledger_is_refused_legibly() {
    let path = scratch("one-writer");
    let held = DurableStore::open(&path).expect("the first open takes the lock");

    let refused = DurableStore::open(&path);
    match refused {
        Err(ikigai_core::Error::Unavailable(message)) => {
            assert!(
                message.contains(&path.display().to_string()),
                "the refusal must name the directory: {message}"
            );
            assert!(
                message.contains("one writer"),
                "…and say why, so an operator is not left with RocksDB's errno: {message}"
            );
        }
        other => panic!("a second open must be refused, and typed transient: {other:?}"),
    }

    drop(held);
    std::fs::remove_dir_all(&path).expect("clean up the scratch store");
}

/// A delete's quarantine graph is durable too — "recoverable" would be a lie if the
/// quarantine lived only in the process that did the deleting.
#[test]
fn quarantined_content_survives_a_restart() {
    let path = scratch("quarantine");
    let iri = {
        let kernel = kernel_over(DurableStore::open(&path).expect("open the store"));
        let iri = append_iri(&kernel, "Deleted before the restart\n\nBody.", &[]);
        delete(&kernel, "urn:iki:ledger:item:1", &[("reason", "a mistake")]);
        iri
    };

    {
        let kernel = kernel_over(DurableStore::open(&path).expect("reopen the store"));
        assert!(try_verb(&kernel, Verb::Source, "urn:iki:ledger:item:1", &[]).is_err());
        let quarantined = select(
            &kernel,
            &format!(
                "SELECT ?o WHERE {{ GRAPH <urn:iki:ledger:graph:default:deleted> {{ \
                 <{iri}> <http://purl.org/dc/terms/title> ?o }} }}"
            ),
        );
        assert!(
            quarantined.contains("Deleted before the restart"),
            "{quarantined}"
        );
    }

    std::fs::remove_dir_all(&path).expect("clean up the scratch store");
}

/// 4. **A keyed append's uniqueness holds on RocksDB**, not only in memory: the guarantee is
///    the store holding its write lock across one update's evaluation and insert, and the
///    two backends implement transactions differently. Eight concurrent appends with one
///    key, a few rounds, one item each time.
#[test]
fn concurrent_keyed_appends_file_one_item_on_the_durable_store() {
    for round in 0..5 {
        let path = scratch(&format!("keyed-{round}"));
        let kernel = kernel_over(DurableStore::open(&path).expect("open the store"));
        let barrier = std::sync::Barrier::new(8);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    barrier.wait();
                    try_verb(
                        &kernel,
                        Verb::Sink,
                        "urn:iki:ledger:append",
                        &[("content", "A finding"), ("key", "urn:kata:issue:01JZ")],
                    )
                    .expect("a keyed append succeeds whether or not it files")
                });
            }
        });
        let listing = source(&kernel, "urn:iki:ledger:items", &[("status", "all")]);
        assert!(
            listing.contains("\n1 item(s)\n"),
            "round {round}: {listing}"
        );
        drop(kernel);
        let _ = std::fs::remove_dir_all(&path);
    }
}

/// 5. **A state transition's compare-and-set holds on RocksDB** (ledger #775), for the same
///    reason as the keyed append: the guarantee is the store's write lock across one update's
///    evaluation and insert. Eight writers move one item out of `queued` at once, each to its
///    own state; exactly one succeeds, the rest are Conflicts, and one state is stored.
#[test]
fn concurrent_transitions_from_one_state_apply_once_on_the_durable_store() {
    for round in 0..5 {
        let path = scratch(&format!("state-{round}"));
        let kernel = kernel_over(DurableStore::open(&path).expect("open the store"));
        append(&kernel, "In a wave", &[]);
        sink(
            &kernel,
            "urn:iki:ledger:item:1:state",
            &[("from", "filed"), ("to", "queued")],
        );
        let barrier = std::sync::Barrier::new(8);
        let targets = [
            "reviewed",
            "resolving",
            "refining",
            "shipping",
            "filed",
            "reviewed",
            "resolving",
            "refining",
        ];
        let answers: Vec<Result<String, ikigai_core::Error>> = std::thread::scope(|scope| {
            let handles: Vec<_> = targets
                .iter()
                .map(|to| {
                    let (kernel, barrier) = (&kernel, &barrier);
                    scope.spawn(move || {
                        barrier.wait();
                        try_verb(
                            kernel,
                            Verb::Sink,
                            "urn:iki:ledger:item:1:state",
                            &[("from", "queued"), ("to", to)],
                        )
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        assert_eq!(
            answers.iter().filter(|a| a.is_ok()).count(),
            1,
            "round {round}: {answers:#?}"
        );
        assert!(
            answers
                .iter()
                .filter_map(|a| a.as_ref().err())
                .all(|e| matches!(e, ikigai_core::Error::Conflict(_))),
            "round {round}: {answers:#?}"
        );
        let stored = select(
            &kernel,
            "SELECT (COUNT(?s) AS ?n) WHERE { GRAPH <urn:iki:ledger:graph:default> \
             { ?i <https://ikigai-rs.dev/ns/ledger#state> ?s } }",
        );
        // `filed` is absence, so a winner that moved it there leaves zero; anything else, one.
        let winner_filed = answers
            .iter()
            .any(|a| a.as_ref().is_ok_and(|text| text.contains(" filed ")));
        let want = if winner_filed { "\"0\"" } else { "\"1\"" };
        assert!(stored.contains(want), "round {round}: {stored}");
        drop(kernel);
        let _ = std::fs::remove_dir_all(&path);
    }
}
