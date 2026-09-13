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
                "SELECT ?o WHERE {{ GRAPH <urn:iki:ledger:graph:deleted> {{ \
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
