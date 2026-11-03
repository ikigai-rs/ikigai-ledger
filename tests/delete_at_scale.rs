//! **Delete and purge cost in proportion to the item, not the ledger** (ledger #2922).
//!
//! The store's narrow write door (`urn:iki:store:graph-update`) evaluates an update against a
//! private copy of the quads it can observe, found by the constants in its patterns, and refuses
//! past `DurableStore::with_max_scoped_copy_quads` (250,000 by default). Through 0.17.0 the
//! removal set of a delete and a purge was written as `?s ?p ?o . FILTER(…)`: a pattern with no
//! constant in it, which observes the graph as a whole. So the recoverable delete's commit (its
//! race guard), the purge's live removal and the purge's graveyard clear each copied every quad
//! of their graph, and a ledger past the bound could delete and purge nothing, a leaked secret
//! included.
//!
//! These tests run the same shape at a small bound, so a debug build can afford it: a graph
//! several times past the bound, and an item well inside it. Each failed on 0.17.0 because of
//! the defect, refused with "observes the graph as a whole". The measurement at a million quads
//! is `measure_delete_and_purge_on_a_million_quads` (ignored; run it in release).

mod common;

use std::sync::Arc;
use std::time::Instant;

use common::*;
use ikigai_core::{Error, Fallback, Kernel, Space, Verb};
use ikigai_store::DurableStore;

const LIVE: &str = "urn:iki:ledger:graph:default";
const GRAVEYARD: &str = "urn:iki:ledger:graph:default:deleted";
const NS: &str = "https://ikigai-rs.dev/ns/ledger#";
const SECRET: &str = "token ghp_0123456789abcdef pasted by mistake";

/// The copy bound in the debug-sized tests: a few thousand quads, against a graph of 12,000.
const BOUND: usize = 2_000;

/// A kernel whose store copies at most `bound` quads into a scoped update.
fn kernel_bounded(bound: usize) -> Kernel {
    kernel_over(
        DurableStore::in_memory()
            .expect("an in-memory store")
            .with_max_scoped_copy_quads(bound),
    )
}

/// `items` filler items of six quads each, written straight into `graph` as data (which the
/// scoped door copies nothing for), 400 to an update.
fn fill(kernel: &Kernel, graph: &str, items: usize, tag: &str) {
    const PER_UPDATE: usize = 400;
    let mut first = 0;
    while first < items {
        let last = (first + PER_UPDATE).min(items);
        let mut data = String::new();
        for k in first..last {
            data.push_str(&format!(
                "<urn:iki:ledger:default:item:{tag}{k:08}> a <{NS}Item> ; <{NS}number> {n} ; \
                 <http://purl.org/dc/terms/title> \"filler {k}\" ; <{NS}status> <{NS}open> ; \
                 <http://purl.org/dc/terms/created> \"2020-02-01T00:00:00Z\"^^<http://www.w3.org/2001/XMLSchema#dateTime> ; \
                 <http://purl.org/dc/terms/modified> \"2020-02-01T00:00:00Z\"^^<http://www.w3.org/2001/XMLSchema#dateTime> .\n",
                n = 1_000_000 + k
            ));
        }
        sink(
            kernel,
            "urn:iki:store:graph-update",
            &[
                ("graph", graph),
                (
                    "content",
                    &format!("INSERT DATA {{ GRAPH <{graph}> {{ {data} }} }}"),
                ),
            ],
        );
        first = last;
    }
}

/// The first row's `?n`, of a query that counts.
fn number(kernel: &Kernel, query: &str) -> usize {
    let json: serde_json::Value =
        serde_json::from_str(&select(kernel, query)).expect("the store answers JSON");
    json["results"]["bindings"][0]["n"]["value"]
        .as_str()
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("a count: {json}"))
}

fn count(kernel: &Kernel, graph: &str) -> usize {
    number(
        kernel,
        &format!("SELECT (COUNT(*) AS ?n) WHERE {{ GRAPH <{graph}> {{ ?s ?p ?o }} }}"),
    )
}

fn mentions(kernel: &Kernel, graph: &str, text: &str) -> bool {
    select(
        kernel,
        &format!("SELECT ?s ?p ?o WHERE {{ GRAPH <{graph}> {{ ?s ?p ?o }} }}"),
    )
    .contains(text)
}

/// A ledger whose graph is six times past the bound, with #1 a real item: comments (one of them
/// the secret), a label, a claim and its events, and #2 blocking it. The graveyard holds three
/// thousand quads of other deleted items.
fn large_ledger(bound: usize, filler: usize) -> (Kernel, String) {
    let kernel = kernel_bounded(bound);
    let target = append_iri(&kernel, "The target", &[]);
    append(&kernel, "A survivor that blocks it", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:link",
        &[("item", "#2"), ("content", "#1"), ("type", "blocks")],
    );
    for note in ["first note", SECRET, "third note"] {
        sink(
            &kernel,
            "urn:iki:ledger:comment",
            &[("item", "#1"), ("content", note)],
        );
    }
    sink(
        &kernel,
        "urn:iki:ledger:label",
        &[("item", "#1"), ("content", "urgent")],
    );
    sink(
        &kernel,
        "urn:iki:ledger:claim",
        &[("item", "#1"), ("content", "worker-1")],
    );
    fill(&kernel, LIVE, filler, "bulk");
    fill(&kernel, GRAVEYARD, filler / 4, "gone");
    (kernel, target)
}

/// A recoverable delete of one item, in a graph past the bound: its quads go to the graveyard,
/// the live graph keeps everything else, and the tombstone counts what was moved.
#[test]
fn a_delete_in_a_graph_past_the_bound_moves_the_item_and_nothing_else() {
    let (kernel, target) = large_ledger(BOUND, 2_000);
    let live_before = count(&kernel, LIVE);
    let graveyard_before = count(&kernel, GRAVEYARD);
    assert!(live_before > 6 * BOUND, "{live_before}");

    let answer = try_verb(&kernel, Verb::Delete, "urn:iki:ledger:item:1", &[])
        .unwrap_or_else(|e| panic!("delete in a graph of {live_before} quads: {e}"));
    assert!(answer.contains("deleted"), "{answer}");

    let moved = count(&kernel, GRAVEYARD) - graveyard_before;
    assert!(moved > 20, "the item, its comments and events: {moved}");
    assert!(mentions(&kernel, GRAVEYARD, SECRET));
    assert!(!mentions(&kernel, LIVE, SECRET));
    // Everything that left the live graph is archived; the tombstone is what was added.
    let tombstone = select(
        &kernel,
        &format!(
            "SELECT ?n WHERE {{ GRAPH <{LIVE}> {{ ?t <{NS}deletedItem> <{target}> ; <{NS}quadCount> ?n }} }}"
        ),
    );
    assert!(
        tombstone.contains(&moved.to_string()),
        "{tombstone} vs {moved}"
    );
    // The survivor's link to it went too, and the survivor stayed.
    assert!(source(&kernel, "urn:iki:ledger:item:2", &[]).contains("A survivor"));
    assert_eq!(
        edges_to(&kernel, LIVE, &target),
        "",
        "edges left pointing at it"
    );
}

/// The edges still pointing at `target` in `graph`, but its tombstone's and another item's
/// history (`editTarget`, `evidence`), which stay by design.
fn edges_to(kernel: &Kernel, graph: &str, target: &str) -> String {
    let json: serde_json::Value = serde_json::from_str(&select(
        kernel,
        &format!(
            "SELECT ?s ?p WHERE {{ GRAPH <{graph}> {{ ?s ?p <{target}> }} \
             FILTER(?p NOT IN (<{NS}deletedItem>, <{NS}editTarget>, <{NS}evidence>)) }}"
        ),
    ))
    .expect("JSON");
    json["results"]["bindings"]
        .as_array()
        .expect("bindings")
        .iter()
        .map(|row| format!("{} {}\n", row["s"]["value"], row["p"]["value"]))
        .collect()
}

/// A purge of a live item, in a graph past the bound: nothing of it is left in either graph.
#[test]
fn a_purge_in_a_graph_past_the_bound_leaves_nothing_of_the_item() {
    let (kernel, target) = large_ledger(BOUND, 2_000);
    let answer = try_verb(
        &kernel,
        Verb::Delete,
        "urn:iki:ledger:purge",
        &[("content", "1")],
    )
    .unwrap_or_else(|e| panic!("purge in a graph past the bound: {e}"));
    assert!(answer.contains("purged"), "{answer}");
    assert!(!mentions(&kernel, LIVE, SECRET));
    assert!(!mentions(&kernel, GRAVEYARD, SECRET));
    assert_eq!(
        edges_to(&kernel, LIVE, &target),
        "",
        "edges left pointing at it"
    );
    assert_eq!(
        edges_to(&kernel, GRAVEYARD, &target),
        "",
        "in the graveyard"
    );
    let own = select(
        &kernel,
        &format!("SELECT ?p WHERE {{ GRAPH ?g {{ <{target}> ?p ?o }} }}"),
    );
    assert!(!own.contains(NS), "{own}");
    assert!(source(&kernel, "urn:iki:ledger:item:2", &[]).contains("A survivor"));
}

/// The path that makes a pasted secret destroyable after it was deleted, in a graph and a
/// graveyard past the bound.
#[test]
fn a_deleted_item_is_purged_in_a_graph_past_the_bound() {
    let (kernel, _) = large_ledger(BOUND, 10_000);
    assert!(count(&kernel, GRAVEYARD) > BOUND);
    delete(&kernel, "urn:iki:ledger:item:1", &[]);
    assert!(mentions(&kernel, GRAVEYARD, SECRET));
    let answer = try_verb(
        &kernel,
        Verb::Delete,
        "urn:iki:ledger:purge",
        &[("content", "1")],
    )
    .unwrap_or_else(|e| panic!("purge of a deleted item: {e}"));
    assert!(answer.contains("archived quad(s) destroyed"), "{answer}");
    assert!(!mentions(&kernel, GRAVEYARD, SECRET));
    assert!(!mentions(&kernel, LIVE, SECRET));
}

/// The bound stays the safety net it was for an update that really does observe the whole
/// graph.
#[test]
fn the_bound_still_refuses_an_update_over_the_whole_graph() {
    let (kernel, _) = large_ledger(BOUND, 2_000);
    let refused = try_verb(
        &kernel,
        Verb::Sink,
        "urn:iki:store:graph-update",
        &[
            ("graph", LIVE),
            (
                "content",
                &format!("DELETE {{ GRAPH <{LIVE}> {{ ?s ?p ?o }} }} WHERE {{ GRAPH <{LIVE}> {{ ?s ?p ?o }} }}"),
            ),
        ],
    )
    .unwrap_err();
    assert!(
        matches!(&refused, Error::InvalidArgument { detail, .. } if detail.contains("as a whole")),
        "{refused:?}"
    );
}

/// `n` comments on `item`, written as data in one update: each one a dependent, as a comment
/// filed through the door is.
fn comments(kernel: &Kernel, item: &str, n: usize) {
    let tag = item.rsplit(':').next().unwrap_or("x");
    let data: String = (0..n)
        .map(|k| {
            format!(
                "<urn:iki:ledger:default:comment:{tag}{k:06}> a <{NS}Comment> ; <{NS}onItem> <{item}> ; \
                 <{NS}body> \"note {k}\" ; <http://purl.org/dc/terms/created> \
                 \"2020-02-01T00:00:00Z\"^^<http://www.w3.org/2001/XMLSchema#dateTime> .\n"
            )
        })
        .collect();
    sink(
        kernel,
        "urn:iki:store:graph-update",
        &[
            ("graph", LIVE),
            (
                "content",
                &format!("INSERT DATA {{ GRAPH <{LIVE}> {{ {data} }} }}"),
            ),
        ],
    );
}

/// An item with 240 dependents, which through 0.18.0's first form was just under the 250 the
/// ledger named one by one: the update that lists them stays inside the store's algebra and text
/// bounds, and a delete then a purge succeed in a graph past the copy bound.
#[test]
fn an_item_with_hundreds_of_dependents_is_deleted_and_purged_past_the_bound() {
    let (kernel, target) = large_ledger(BOUND, 2_000);
    comments(&kernel, &target, 225);
    let answer = try_verb(&kernel, Verb::Delete, "urn:iki:ledger:item:1", &[])
        .unwrap_or_else(|e| panic!("delete of an item with ~240 dependents: {e}"));
    assert!(answer.contains("deleted"), "{answer}");
    assert_eq!(edges_to(&kernel, LIVE, &target), "");
    let answer = try_verb(
        &kernel,
        Verb::Delete,
        "urn:iki:ledger:purge",
        &[("content", "1")],
    )
    .unwrap_or_else(|e| panic!("purge of it: {e}"));
    assert!(answer.contains("archived quad(s) destroyed"), "{answer}");
    assert!(!mentions(&kernel, GRAVEYARD, "note 17"));
    assert!(!mentions(&kernel, GRAVEYARD, SECRET));
}

/// An item with 300 dependents in a graph inside the bound: through 0.18.0's first form this was
/// past the 250 named one by one and fell back to the form that observes the whole graph; now it
/// is one `VALUES` list like any other (ledger #2941), and the answers are the same.
#[test]
fn an_item_with_hundreds_of_dependents_goes_inside_the_bound_too() {
    let kernel = kernel_bounded(250_000);
    let target = append_iri(&kernel, "The target", &[]);
    append(&kernel, "A survivor", &[]);
    comments(&kernel, &target, 300);
    delete(&kernel, "urn:iki:ledger:item:1", &[]);
    assert!(mentions(&kernel, GRAVEYARD, "note 299"));
    assert!(!mentions(&kernel, LIVE, "note 299"));
    let answer = delete(&kernel, "urn:iki:ledger:purge", &[("content", "1")]);
    assert!(answer.contains("archived quad(s) destroyed"), "{answer}");
    assert!(!mentions(&kernel, GRAVEYARD, "note 299"));
    append(&kernel, "Another", &[]);
    let third = append_iri(&kernel, "A third target", &[]);
    comments(&kernel, &third, 300);
    let answer = delete(&kernel, "urn:iki:ledger:purge", &[("content", "4")]);
    assert!(answer.contains("destroyed, NOT recoverable"), "{answer}");
    assert!(!mentions(&kernel, LIVE, "note 299"));
}

/// ★ The reproduction (ledger #2941): an item with 500 dependents, in a graph past the copy
/// bound, with the item's own quads well inside it. Through 0.18.0's first form the ledger named
/// at most 250 dependents one by one and fell back past that to the form that observes the graph
/// as a whole, so this delete was refused. Since store 0.3.3 narrows a variable an inline
/// `VALUES` binds to IRIs, the dependents are one `VALUES ?d { … }` pattern, of any length.
#[test]
fn an_item_with_five_hundred_dependents_is_deleted_and_purged_past_the_bound() {
    // 500 comments are about 2,000 quads, so the bound is set above the item and well below the
    // graph (over 12,000 quads).
    let (kernel, target) = large_ledger(5_000, 2_000);
    comments(&kernel, &target, 500);
    assert!(count(&kernel, LIVE) > 2 * 5_000);
    let answer = try_verb(&kernel, Verb::Delete, "urn:iki:ledger:item:1", &[])
        .unwrap_or_else(|e| panic!("delete of an item with ~515 dependents: {e}"));
    assert!(answer.contains("deleted"), "{answer}");
    assert!(mentions(&kernel, GRAVEYARD, "note 499"));
    assert!(!mentions(&kernel, LIVE, "note 499"));
    assert_eq!(edges_to(&kernel, LIVE, &target), "");
    let answer = try_verb(
        &kernel,
        Verb::Delete,
        "urn:iki:ledger:purge",
        &[("content", "1")],
    )
    .unwrap_or_else(|e| panic!("purge of it: {e}"));
    assert!(answer.contains("archived quad(s) destroyed"), "{answer}");
    assert!(!mentions(&kernel, GRAVEYARD, "note 499"));
    assert!(!mentions(&kernel, GRAVEYARD, SECRET));

    // And a purge of a LIVE item with as many, which removes from the live graph directly.
    let third = append_iri(&kernel, "A third target", &[]);
    comments(&kernel, &third, 500);
    let answer = try_verb(
        &kernel,
        Verb::Delete,
        "urn:iki:ledger:purge",
        &[("content", "3")],
    )
    .unwrap_or_else(|e| panic!("purge of a live item with 500 dependents: {e}"));
    assert!(answer.contains("destroyed, NOT recoverable"), "{answer}");
    assert!(!mentions(&kernel, LIVE, "note 499"));
}

/// A dependent that is a blank node has no name to list, so the one case the ledger still sends
/// the form that observes the whole graph (this crate writes none; one comes only from a write
/// through the broad door). A recoverable delete refuses it, as it always has, since archiving
/// would mint a different node; a purge, inside the bound, takes it with its item.
#[test]
fn a_blank_node_dependent_goes_with_its_item_inside_the_bound() {
    let kernel = kernel_bounded(250_000);
    let target = append_iri(&kernel, "The target", &[]);
    append(&kernel, "A survivor", &[]);
    sink(
        &kernel,
        "urn:iki:store:graph-update",
        &[
            ("graph", LIVE),
            (
                "content",
                &format!(
                    "INSERT DATA {{ GRAPH <{LIVE}> {{ _:note a <{NS}Comment> ; \
                     <{NS}onItem> <{target}> ; <{NS}body> \"an anonymous note\" . }} }}"
                ),
            ),
        ],
    );
    let refused = try_verb(&kernel, Verb::Delete, "urn:iki:ledger:item:1", &[])
        .expect_err("a recoverable delete cannot archive a blank node");
    assert!(refused.to_string().contains("blank node"), "{refused}");
    let answer = delete(&kernel, "urn:iki:ledger:purge", &[("content", "1")]);
    assert!(answer.contains("destroyed, NOT recoverable"), "{answer}");
    assert!(!mentions(&kernel, LIVE, "an anonymous note"));
    assert!(source(&kernel, "urn:iki:ledger:item:2", &[]).contains("A survivor"));
}

// ---------------------------------------------------------------------------------- measure

/// A kernel over `store`, for the measurement.
fn measured_kernel(store: DurableStore) -> Kernel {
    let space = Fallback::new(vec![
        Arc::new(ikigai_store::space(store)) as Arc<dyn Space>,
        Arc::new(ikigai_ledger::space()) as Arc<dyn Space>,
    ]);
    Kernel::with_meta_renderer(Arc::new(space), Arc::new(ikigai_vocab::TurtleRenderer))
        .with_clock(Arc::new(TickingClock::default()))
}

/// This process's resident set, in KiB, as `ps` reports it.
fn rss_kib() -> u64 {
    std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()
        .and_then(|out| String::from_utf8_lossy(&out.stdout).trim().parse().ok())
        .unwrap_or(0)
}

/// Run `f`, sampling the resident set every 10 ms: the answer, and how far the peak rose above
/// where it started, in MiB.
fn peak_rise<R>(f: impl FnOnce() -> R) -> (R, f64) {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    let before = rss_kib();
    let done = Arc::new(AtomicBool::new(false));
    let peak = Arc::new(AtomicU64::new(before));
    let sampler = {
        let (done, peak) = (Arc::clone(&done), Arc::clone(&peak));
        std::thread::spawn(move || {
            while !done.load(Ordering::SeqCst) {
                peak.fetch_max(rss_kib(), Ordering::SeqCst);
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        })
    };
    let answer = f();
    peak.fetch_max(rss_kib(), Ordering::SeqCst);
    done.store(true, Ordering::SeqCst);
    sampler.join().unwrap();
    let rise = peak.load(Ordering::SeqCst).saturating_sub(before);
    (answer, rise as f64 / 1024.0)
}

/// The quads a narrow removal of item `n` lets the store copy: the item as subject, the item
/// as object, and each dependent as subject. (A refusal names the whole graph instead.)
fn footprint(kernel: &Kernel, n: usize) -> usize {
    number(
        kernel,
        &format!(
            "SELECT (COUNT(*) AS ?n) WHERE {{ SELECT DISTINCT ?s ?p ?o WHERE {{ GRAPH <{LIVE}> {{ \
             ?i <{NS}number> {n} . \
             {{ ?i ?p ?o BIND(?i AS ?s) }} UNION {{ ?s ?p ?i BIND(?i AS ?o) }} UNION \
             {{ ?s <{NS}onItem> ?i . ?s ?p ?o }} }} }} }}"
        ),
    )
}

/// ★ The numbers the README and the PR state. Not in the default run:
/// `cargo test --release --test delete_at_scale -- --ignored --nocapture measure`. The copy
/// bound is the store's default unless `IKIGAI_LEDGER_MEASURE_BOUND` names another (to time
/// 0.17.0's whole-graph copy, which the default refuses).
#[test]
#[ignore = "a measurement: run it in release, with --nocapture"]
fn measure_delete_and_purge_on_a_million_quads() {
    let store = DurableStore::in_memory().unwrap();
    let store = match std::env::var("IKIGAI_LEDGER_MEASURE_BOUND") {
        Ok(bound) => store.with_max_scoped_copy_quads(bound.parse().expect("a number")),
        Err(_) => store,
    };
    let kernel = measured_kernel(store);
    let started = Instant::now();
    // #1 and #3 are the targets, #2 blocks #1. Ten comments and a label each.
    append(&kernel, "The target", &[]);
    append(&kernel, "A survivor that blocks it", &[]);
    append(&kernel, "Another target", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:link",
        &[("item", "#2"), ("content", "#1"), ("type", "blocks")],
    );
    for item in ["#1", "#3"] {
        for k in 0..10 {
            sink(
                &kernel,
                "urn:iki:ledger:comment",
                &[("item", item), ("content", &format!("note {k}"))],
            );
        }
        sink(
            &kernel,
            "urn:iki:ledger:label",
            &[("item", item), ("content", "urgent")],
        );
    }
    fill(&kernel, LIVE, 166_000, "bulk");
    fill(&kernel, GRAVEYARD, 20_000, "gone");
    println!(
        "fixture: {} quads live, {} in the graveyard, built in {:.1}s, resident {:.0} MiB",
        count(&kernel, LIVE),
        count(&kernel, GRAVEYARD),
        started.elapsed().as_secs_f64(),
        rss_kib() as f64 / 1024.0
    );
    println!(
        "footprint of a narrow removal: #1 {} quads, #3 {} quads",
        footprint(&kernel, 1),
        footprint(&kernel, 3)
    );
    for (what, iri, args) in [
        ("delete #1", "urn:iki:ledger:item:1", vec![]),
        (
            "purge #1 (deleted)",
            "urn:iki:ledger:purge",
            vec![("content", "1")],
        ),
        (
            "purge #3 (live)",
            "urn:iki:ledger:purge",
            vec![("content", "3")],
        ),
    ] {
        let started = Instant::now();
        let (answer, rise) = peak_rise(|| try_verb(&kernel, Verb::Delete, iri, &args));
        println!(
            "{what}: {:.3}s, peak +{rise:.0} MiB, {}",
            started.elapsed().as_secs_f64(),
            match answer {
                Ok(answer) => answer
                    .trim()
                    .lines()
                    .take(2)
                    .collect::<Vec<_>>()
                    .join(" / "),
                Err(e) => format!(
                    "REFUSED {}",
                    e.to_string().chars().take(120).collect::<String>()
                ),
            }
        );
    }
}
