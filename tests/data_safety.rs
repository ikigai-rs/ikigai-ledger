//! **Data safety: identity, purge, and a delete racing a write.** Each test here pins a
//! defect an unled audit reproduced on 0.2.1 (`4e6f46e`), and failed there because of it.
//!
//! - Minted ids were `(millisecond, digest of the content)`, so two identical filings in
//!   one millisecond were one item, and the same note on two items was one comment node
//!   that died with whichever item was deleted first.
//! - `purge` looked only in the live graph, so an item that had been deleted could never be
//!   purged: a pasted secret, once deleted, stayed in the graveyard for good.
//! - A recoverable delete removed `DELETE … WHERE { selector }` after archiving what an
//!   earlier read returned, so a comment that landed between the two was removed without
//!   ever being archived.

mod common;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Barrier, OnceLock};

use async_trait::async_trait;
use common::*;
use ikigai_core::{
    ArgRef, Capability, Clock, Endpoint, Error, Fallback, Invocation, Iri, Kernel, Representation,
    Request, Resolution, Result, Scope, Space, SystemClock, Time, Verb,
};
use ikigai_store::DurableStore;

const T0: u64 = 1_789_430_400_000; // 2026-09-15T00:00:00Z
const LIVE: &str = "urn:iki:ledger:graph:default";
const GRAVEYARD: &str = "urn:iki:ledger:graph:default:deleted";

/// A clock that does not move unless the test moves it — what a replay harness uses, and
/// what a real clock looks like to two requests inside one millisecond.
struct HandClock(Arc<AtomicU64>);

impl Clock for HandClock {
    fn now(&self) -> Time {
        Time::from_millis(self.0.load(Ordering::SeqCst))
    }
}

fn kernel_with(clock: Arc<dyn Clock>) -> Kernel {
    let space = Fallback::new(vec![
        Arc::new(ikigai_store::space(DurableStore::in_memory().unwrap())) as Arc<dyn Space>,
        Arc::new(ikigai_ledger::space()) as Arc<dyn Space>,
    ]);
    Kernel::with_meta_renderer(Arc::new(space), Arc::new(ikigai_vocab::TurtleRenderer))
        .with_clock(clock)
}

fn frozen_kernel() -> Kernel {
    kernel_with(Arc::new(HandClock(Arc::new(AtomicU64::new(T0)))))
}

/// The values of one variable across a SELECT's rows, as a sorted list.
fn column(kernel: &Kernel, query: &str, var: &str) -> Vec<String> {
    let json: serde_json::Value =
        serde_json::from_str(&select(kernel, query)).expect("the store answers JSON");
    let mut values: Vec<String> = json["results"]["bindings"]
        .as_array()
        .expect("results.bindings")
        .iter()
        .filter_map(|row| row[var]["value"].as_str().map(str::to_string))
        .collect();
    values.sort();
    values
}

fn distinct_items(kernel: &Kernel) -> Vec<String> {
    column(
        kernel,
        &format!(
            "SELECT DISTINCT ?i WHERE {{ GRAPH <{LIVE}> {{ \
             ?i a <https://ikigai-rs.dev/ns/ledger#Item> }} }}"
        ),
        "i",
    )
}

fn comment_edges(kernel: &Kernel) -> Vec<String> {
    column(
        kernel,
        &format!(
            "SELECT ?c WHERE {{ GRAPH <{LIVE}> {{ \
             ?c <https://ikigai-rs.dev/ns/ledger#onItem> ?i }} }}"
        ),
        "c",
    )
}

// ------------------------------------------------------------------------- identity

/// ★ Two filings with the same title and author, in one millisecond, are two items. On
/// 0.2.1 they minted ONE IRI, so the second append landed on the first item: one subject
/// with two numbers.
#[test]
fn two_identical_filings_in_one_millisecond_are_two_items() {
    let kernel = frozen_kernel();
    let first = append_iri(&kernel, "Flaky test in CI", &[("author", "bot")]);
    let second = append_iri(&kernel, "Flaky test in CI", &[("author", "bot")]);
    assert_ne!(first, second, "two filed items share one IRI");
    assert_eq!(distinct_items(&kernel).len(), 2);
}

/// The same, under the REAL clock and concurrent callers — a host serving several agents.
/// 0.2.1 minted one IRI for all eight.
#[test]
fn identical_filings_from_concurrent_callers_are_distinct_items() {
    let kernel = kernel_with(Arc::new(SystemClock));
    let n = 8;
    let barrier = Barrier::new(n);
    std::thread::scope(|scope| {
        for _ in 0..n {
            scope.spawn(|| {
                barrier.wait();
                sink(
                    &kernel,
                    "urn:iki:ledger:append",
                    &[("content", "flaky test"), ("author", "ci-bot")],
                );
            });
        }
    });
    assert_eq!(distinct_items(&kernel).len(), n);
    // And every item carries exactly one number.
    let numbers = column(
        &kernel,
        &format!(
            "SELECT ?i ?n WHERE {{ GRAPH <{LIVE}> {{ \
             ?i a <https://ikigai-rs.dev/ns/ledger#Item> ; \
             <https://ikigai-rs.dev/ns/ledger#number> ?n }} }}"
        ),
        "n",
    );
    assert_eq!(numbers.len(), n, "{numbers:?}");
}

/// ★ The same note on two DIFFERENT items, in one millisecond, is two comments — and
/// deleting one item leaves the other's comment where it was. On 0.2.1 the two were one
/// comment node with two `ledger:onItem` edges, and the delete of #1 archived it whole, so
/// #2 lost a comment nobody deleted.
#[test]
fn the_same_note_on_two_items_is_two_comments_and_survives_the_others_delete() {
    let kernel = frozen_kernel();
    append(&kernel, "First item", &[]);
    append(&kernel, "Second item", &[]);
    for item in ["#1", "#2"] {
        sink(
            &kernel,
            "urn:iki:ledger:comment",
            &[("item", item), ("content", "lgtm"), ("author", "bot")],
        );
    }
    let edges = comment_edges(&kernel);
    let mut nodes = edges.clone();
    nodes.dedup();
    assert_eq!(
        (edges.len(), nodes.len()),
        (2, 2),
        "two comments, one node each: {edges:?}"
    );

    delete(&kernel, "urn:iki:ledger:item:1", &[("reason", "dup")]);
    let second = source(&kernel, "urn:iki:ledger:item:2", &[]);
    assert!(second.contains("lgtm"), "#2 lost its comment:\n{second}");
}

/// The comment half under the real clock and concurrent callers.
#[test]
fn identical_notes_from_concurrent_callers_are_distinct_comments() {
    let kernel = kernel_with(Arc::new(SystemClock));
    let n = 8;
    for i in 0..n {
        append(&kernel, &format!("item {i}"), &[]);
    }
    let barrier = Barrier::new(n);
    std::thread::scope(|scope| {
        for i in 1..=n {
            let (kernel, barrier) = (&kernel, &barrier);
            scope.spawn(move || {
                barrier.wait();
                sink(
                    kernel,
                    "urn:iki:ledger:comment",
                    &[("item", &i.to_string()), ("content", "superseded")],
                );
            });
        }
    });
    let edges = comment_edges(&kernel);
    let mut nodes = edges.clone();
    nodes.dedup();
    assert_eq!((edges.len(), nodes.len()), (n, n), "{edges:?}");
}

/// Two DIFFERENT selections in one millisecond are two subjects; the same selection twice
/// is one, because the IRI is a digest of what it says. On 0.2.1 the IRI was the
/// millisecond alone, so a union of the two claimed two policies.
#[test]
fn a_selections_iri_names_what_it_says_and_not_only_when() {
    let kernel = frozen_kernel();
    append(&kernel, "Something", &[("priority", "1")]);
    let subject = |turtle: &str| -> String {
        let start = turtle
            .find("urn:iki:ledger:default:selection:")
            .expect("a selection subject");
        // Up to the closing bracket, then without any `:rank:{n}` / `:excluded:{n}` tail.
        let rest = &turtle[start..];
        let end = rest.find('>').unwrap_or(rest.len());
        let iri = &rest[..end];
        iri.split(":rank:")
            .next()
            .and_then(|s| s.split(":excluded:").next())
            .unwrap()
            .to_string()
    };
    let as_turtle = |policy: &str| {
        source(
            &kernel,
            "urn:iki:ledger:next",
            &[("as", "text/turtle"), ("policy", policy)],
        )
    };
    let by_priority = subject(&as_turtle("priority-recency"));
    let by_leverage = subject(&as_turtle("leverage"));
    assert_ne!(by_priority, by_leverage);
    assert_eq!(by_priority, subject(&as_turtle("priority-recency")));
}

// ---------------------------------------------------------------------------- purge

/// ★ **The secret case.** A token pasted into an item is deleted first — what anyone does
/// on noticing — and then purged. On 0.2.1 the purge answered NotFound, because it looked
/// only in the live graph, and the token stayed in the graveyard for good.
#[test]
fn a_deleted_item_can_be_purged_and_nothing_of_it_is_left_in_any_graph() {
    for reference in ["iri", "#1", "1"] {
        let kernel = kernel();
        let iri = append_iri(&kernel, "Pasted a token\n\nghp_secretsecretsecret", &[]);
        sink(
            &kernel,
            "urn:iki:ledger:comment",
            &[
                ("item", "#1"),
                ("content", "the token again: ghp_secretsecretsecret"),
            ],
        );
        delete(
            &kernel,
            "urn:iki:ledger:item:1",
            &[("reason", "oops"), ("author", "alice")],
        );
        let named = if reference == "iri" {
            iri.clone()
        } else {
            reference.to_string()
        };
        let purged = delete(
            &kernel,
            "urn:iki:ledger:purge",
            &[
                ("content", &named),
                ("reason", "it was a secret"),
                ("author", "bob"),
            ],
        );
        assert!(purged.contains("archived quad(s) destroyed"), "{purged}");

        let everything = select(&kernel, "SELECT ?o WHERE { GRAPH ?g { ?s ?p ?o } }");
        assert!(
            !everything.contains("ghp_secret"),
            "purge of `{named}` left the secret in the store: {everything}"
        );

        // The tombstone says what happened, the delete's words and the purge's kept apart.
        let tombstone = select(
            &kernel,
            &format!(
                "SELECT ?p ?o WHERE {{ GRAPH <{LIVE}> {{ \
                 ?t <https://ikigai-rs.dev/ns/ledger#deletedItem> <{iri}> ; ?p ?o }} }}"
            ),
        );
        for expected in [
            "purgedAt",
            "it was a secret",
            "bob",
            "oops",
            "alice",
            "sha256:",
        ] {
            assert!(tombstone.contains(expected), "no `{expected}`: {tombstone}");
        }
        let recoverable = column(
            &kernel,
            &format!(
                "SELECT ?r WHERE {{ GRAPH <{LIVE}> {{ ?t \
                 <https://ikigai-rs.dev/ns/ledger#deletedItem> <{iri}> ; \
                 <https://ikigai-rs.dev/ns/ledger#recoverable> ?r }} }}"
            ),
            "r",
        );
        assert_eq!(recoverable, vec!["false".to_string()], "{tombstone}");

        // And a name that was never filed is still not found.
        let nothing = try_verb(
            &kernel,
            Verb::Delete,
            "urn:iki:ledger:purge",
            &[("content", "#99")],
        );
        assert!(matches!(nothing, Err(Error::NotFound(_))), "{nothing:?}");
    }
}

/// A purge reads the graveyard, so it needs the graveyard's READ grant — and is refused
/// without it, naming the token, BEFORE anything changes.
#[test]
fn purge_needs_the_graveyards_read_grant_and_says_so_before_touching_anything() {
    let kernel = kernel();
    sink(
        &kernel,
        "urn:iki:ledger:acme:append",
        &[("content", "Still here")],
    );
    let without_read = Capability::scoped([
        "urn:cap:ledger:read:acme".to_string(),
        "urn:cap:ledger:purge:acme".to_string(),
        graph_read("acme"),
        graph_write("acme"),
        graveyard_write("acme"),
    ]);
    let refused = try_as(
        &kernel,
        &without_read,
        Verb::Delete,
        "urn:iki:ledger:acme:purge",
        &[("content", "acme#1")],
    );
    let Err(Error::Denied(message)) = &refused else {
        panic!("purge without the graveyard read grant: {refused:?}");
    };
    assert!(message.contains(&graveyard_read("acme")), "{message}");
    assert!(
        source(&kernel, "urn:iki:ledger:acme:items", &[]).contains("Still here"),
        "a refused purge changed something"
    );
}

// ------------------------------------------------------------ a delete racing a write

static RACE_KERNEL: OnceLock<Kernel> = OnceLock::new();
static ARMED: AtomicBool = AtomicBool::new(false);

/// Wraps the store so that, right after a delete's graveyard write commits, a second
/// request — a comment on the item being deleted — commits too: the interleaving two
/// concurrent requests produce at the delete's await points, forced so it happens every
/// time rather than one run in a few.
struct Interleave(Arc<dyn Space>);
struct AfterGraveyardWrite(Arc<dyn Endpoint>);

impl Space for Interleave {
    fn resolve(&self, request: &Request, scope: &Scope) -> Resolution {
        self.0
            .resolve(request, scope)
            .map_endpoint(|endpoint| Arc::new(AfterGraveyardWrite(endpoint)) as Arc<dyn Endpoint>)
    }
}

#[async_trait]
impl Endpoint for AfterGraveyardWrite {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let answer = self.0.invoke(inv).await?;
        let to_graveyard = inv.request.target.as_str() == "urn:iki:store:graph-update"
            && inv
                .inline_str("graph")
                .map(|g| g.ends_with(":deleted"))
                .unwrap_or(false);
        if to_graveyard && ARMED.swap(false, Ordering::SeqCst) {
            let kernel = RACE_KERNEL.get().expect("the race kernel");
            kernel
                .issue(
                    Request::new(
                        Verb::Sink,
                        Iri::parse("urn:iki:ledger:comment").expect("an IRI"),
                    )
                    .with_arg("item", ArgRef::Inline(b"#1".to_vec()))
                    .with_arg("content", ArgRef::Inline(b"concurrent comment".to_vec())),
                    &Capability::root(),
                )
                .await?;
        }
        Ok(answer)
    }

    fn name(&self) -> &str {
        self.0.name()
    }

    fn describe(&self) -> ikigai_core::Description {
        self.0.describe()
    }
}

/// ★ A comment that lands between the archive and the removal is KEPT. On 0.2.1 the
/// removal re-ran the selector, so it removed the comment from the live graph without it
/// ever having been archived — in no graph at all. And the tombstone counts exactly what
/// the graveyard holds, because what was archived and what was removed are one set now.
#[test]
fn a_write_between_the_archive_and_the_removal_is_kept_not_destroyed() {
    let store = Arc::new(ikigai_store::space(DurableStore::in_memory().unwrap())) as Arc<dyn Space>;
    let space = Fallback::new(vec![
        Arc::new(Interleave(store)) as Arc<dyn Space>,
        Arc::new(ikigai_ledger::space()) as Arc<dyn Space>,
    ]);
    let kernel =
        Kernel::with_meta_renderer(Arc::new(space), Arc::new(ikigai_vocab::TurtleRenderer))
            .with_clock(Arc::new(TickingClock::default()));
    assert!(RACE_KERNEL.set(kernel).is_ok());
    let kernel = RACE_KERNEL.get().unwrap();
    let iri = append_iri(kernel, "to be deleted", &[]);
    sink(
        kernel,
        "urn:iki:ledger:comment",
        &[("item", "#1"), ("content", "earlier comment")],
    );
    ARMED.store(true, Ordering::SeqCst);
    delete(kernel, "urn:iki:ledger:item:1", &[]);
    assert!(!ARMED.load(Ordering::SeqCst), "the interleaved write ran");

    let graphs_holding = |text: &str| {
        column(
            kernel,
            &format!(
                "SELECT ?g WHERE {{ GRAPH ?g {{ ?c \
                 <https://ikigai-rs.dev/ns/ledger#body> \"{text}\" }} }}"
            ),
            "g",
        )
    };
    assert_eq!(
        graphs_holding("earlier comment"),
        vec![GRAVEYARD.to_string()]
    );
    assert!(
        !graphs_holding("concurrent comment").is_empty(),
        "an acknowledged comment is in no graph: the delete destroyed it"
    );

    // The tombstone's count is the size of what the graveyard holds for this item.
    let counted = column(
        kernel,
        &format!(
            "SELECT ?n WHERE {{ GRAPH <{LIVE}> {{ ?t \
             <https://ikigai-rs.dev/ns/ledger#deletedItem> <{iri}> ; \
             <https://ikigai-rs.dev/ns/ledger#quadCount> ?n }} }}"
        ),
        "n",
    );
    let archived = column(
        kernel,
        &format!("SELECT ?s ?p ?o WHERE {{ GRAPH <{GRAVEYARD}> {{ ?s ?p ?o }} }}"),
        "p",
    );
    assert_eq!(counted, vec![archived.len().to_string()]);
}

/// The same race, unforced: a delete and a comment released together, three hundred
/// times. A comment the ledger acknowledged must end up live or archived, never neither.
#[test]
fn a_comment_racing_a_delete_is_never_destroyed() {
    let mut lost = Vec::new();
    let mut acknowledged = 0;
    for round in 0..300 {
        let kernel = kernel();
        append(&kernel, "doomed", &[]);
        let marker = format!("marker-{round}");
        let barrier = Barrier::new(2);
        let commented = std::thread::scope(|scope| {
            let deleter = scope.spawn(|| {
                barrier.wait();
                try_verb(&kernel, Verb::Delete, "urn:iki:ledger:item:1", &[]).is_ok()
            });
            let commenter = scope.spawn(|| {
                barrier.wait();
                try_verb(
                    &kernel,
                    Verb::Sink,
                    "urn:iki:ledger:comment",
                    &[("item", "#1"), ("content", &marker)],
                )
                .is_ok()
            });
            let _ = deleter.join().unwrap();
            commenter.join().unwrap()
        });
        if commented {
            acknowledged += 1;
            let anywhere = select(
                &kernel,
                &format!("SELECT ?g WHERE {{ GRAPH ?g {{ ?c ?p \"{marker}\" }} }}"),
            );
            if !anywhere.contains("urn:iki:ledger:graph") {
                lost.push(round);
            }
        }
    }
    assert!(
        lost.is_empty(),
        "{} of {acknowledged} acknowledged comments are in neither graph; first rounds: {:?}",
        lost.len(),
        &lost[..lost.len().min(10)]
    );
}
