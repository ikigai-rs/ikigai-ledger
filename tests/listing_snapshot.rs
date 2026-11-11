//! **A listing's page and its `total` come from one read of the store** (ledger #1741).
//!
//! `urn:iki:ledger:items` chose its page, loaded its rows and counted its `total` in separate
//! store queries. An append that landed between the page read and the count left an answer
//! whose `total` named an item no page of it had seen: with every item on one page, `count`
//! was 3 and `total` 4, so a client paging "until it has seen `total`" asked for a page that
//! was not in the answer it was reading.
//!
//! The schedule is forced, not slept: a wrapping store space files one armed item right after
//! the listing's Nth read of its graph, for every N the listing makes. Whatever N, the answer
//! must describe ONE ledger: `total` counts exactly the items the page would list, and the
//! page holds only items that `total` counted.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use common::*;
use ikigai_core::{
    ArgRef, Capability, Endpoint, Fallback, Invocation, Iri, Kernel, Representation, Request,
    Resolution, Result, Scope, Space, Verb,
};
use ikigai_store::DurableStore;

/// Files the armed request once, right after the `after`th graph read since arming.
struct AfterNthRead {
    inner: Arc<dyn Space>,
    shared: Arc<Shared>,
}

struct Shared {
    kernel: OnceLock<Kernel>,
    armed: Mutex<Option<Request>>,
    after: AtomicUsize,
    reads: AtomicUsize,
}

struct Gate {
    inner: Arc<dyn Endpoint>,
    shared: Arc<Shared>,
}

impl Space for AfterNthRead {
    fn resolve(&self, request: &Request, scope: &Scope) -> Resolution {
        let shared = Arc::clone(&self.shared);
        self.inner
            .resolve(request, scope)
            .map_endpoint(move |inner| Arc::new(Gate { inner, shared }) as Arc<dyn Endpoint>)
    }
}

#[async_trait]
impl Endpoint for Gate {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let read = matches!(
            inv.request.target.as_str(),
            "urn:iki:store:graph-select" | "urn:iki:store:graph-ask"
        );
        let answer = self.inner.invoke(inv).await?;
        if read && self.shared.armed.lock().unwrap().is_some() {
            let seen = self.shared.reads.fetch_add(1, Ordering::SeqCst) + 1;
            if seen == self.shared.after.load(Ordering::SeqCst) {
                let armed = self.shared.armed.lock().unwrap().take();
                if let Some(armed) = armed {
                    let kernel = self.shared.kernel.get().expect("the kernel is set");
                    kernel.issue(armed, &Capability::root()).await?;
                }
            }
        }
        Ok(answer)
    }

    fn name(&self) -> &str {
        self.inner.name()
    }

    fn describe(&self) -> ikigai_core::Description {
        self.inner.describe()
    }
}

fn gated() -> Arc<Shared> {
    let shared = Arc::new(Shared {
        kernel: OnceLock::new(),
        armed: Mutex::new(None),
        after: AtomicUsize::new(0),
        reads: AtomicUsize::new(0),
    });
    let store = Arc::new(ikigai_store::space(DurableStore::in_memory().unwrap())) as Arc<dyn Space>;
    let space = Fallback::new(vec![
        Arc::new(AfterNthRead {
            inner: store,
            shared: Arc::clone(&shared),
        }) as Arc<dyn Space>,
        Arc::new(ikigai_ledger::space()) as Arc<dyn Space>,
    ]);
    let kernel =
        Kernel::with_meta_renderer(Arc::new(space), Arc::new(ikigai_vocab::TurtleRenderer))
            .with_clock(Arc::new(TickingClock::default()));
    assert!(shared.kernel.set(kernel).is_ok());
    shared
}

fn request(verb: Verb, iri: &str, args: &[(&str, &str)]) -> Request {
    args.iter().fold(
        Request::new(verb, Iri::parse(iri).expect("a test IRI")),
        |r, (name, value)| r.with_arg(*name, ArgRef::Inline(value.as_bytes().to_vec())),
    )
}

/// The listing's JSON with one append forced after its `after`th graph read, and whether the
/// append ran inside the listing (false once `after` is past the listing's last read).
fn listing_with_append_after(
    after: usize,
    args: &[(&str, &str)],
) -> (serde_json::Value, bool, Arc<Shared>) {
    let shared = gated();
    let kernel = shared.kernel.get().unwrap();
    for title in ["first", "second", "third"] {
        append(kernel, title, &[]);
    }
    shared.reads.store(0, Ordering::SeqCst);
    shared.after.store(after, Ordering::SeqCst);
    *shared.armed.lock().unwrap() = Some(request(
        Verb::Sink,
        "urn:iki:ledger:append",
        &[("content", "filed while the listing read")],
    ));
    let mut all = vec![("as", "application/json")];
    all.extend_from_slice(args);
    let body = source(kernel, "urn:iki:ledger:items", &all);
    let ran = shared.armed.lock().unwrap().take().is_none();
    let doc = serde_json::from_str(&body).expect("the listing answers JSON");
    (doc, ran, Arc::clone(&shared))
}

fn numbers(doc: &serde_json::Value) -> Vec<i64> {
    doc["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| item["number"].as_i64().expect("a number"))
        .collect()
}

/// Every schedule, for one set of listing arguments whose page holds the whole set.
fn every_schedule_agrees(args: &[(&str, &str)]) {
    let mut disagreements = Vec::new();
    let mut schedules = 0;
    for after in 1.. {
        let (doc, ran, _) = listing_with_append_after(after, args);
        if !ran {
            break;
        }
        schedules += 1;
        let total = doc["total"].as_u64().expect("a total") as usize;
        let count = doc["count"].as_u64().expect("a count") as usize;
        let rows = numbers(&doc);
        // The page holds every match (limit 50 over at most four items), so the page and the
        // total describe the same ledger exactly when they are the same size.
        if total != count || rows.len() != count {
            disagreements.push((after, count, total, rows));
        }
    }
    assert!(schedules > 0, "the append never ran inside the listing");
    assert!(
        disagreements.is_empty(),
        "{args:?}: an append after the listing's Nth read left a page and a total that \
         disagree; (N, count, total, rows): {disagreements:?}"
    );
}

/// ★ The default listing (newest first), the one gonk's Queue and the CLI page through.
#[test]
fn an_append_during_the_default_listing_leaves_page_and_total_agreeing() {
    every_schedule_agrees(&[]);
}

/// An explicit sort reads the whole matching set before cutting the page; its total must be
/// that set's size too.
#[test]
fn an_append_during_a_sorted_listing_leaves_page_and_total_agreeing() {
    every_schedule_agrees(&[("sort", "number")]);
}

/// The union listing pages each ledger's first `offset + limit` and adds the totals.
#[test]
fn an_append_during_a_union_listing_leaves_page_and_total_agreeing() {
    every_schedule_agrees(&[("ledgers", "default")]);
}

/// A filtered listing counts over the same filter as its page.
#[test]
fn an_append_during_a_filtered_listing_leaves_page_and_total_agreeing() {
    every_schedule_agrees(&[("status", "open")]);
}

/// ★ The plain face's footer states the same pair, so it must be one read there too: `N
/// item(s)` when the page is the whole set, never `1–3 of 4` over a ledger the page saw as 3.
#[test]
fn an_append_during_the_plain_listing_leaves_the_footer_agreeing() {
    let mut wrong = Vec::new();
    for after in 1.. {
        let shared = gated();
        let kernel = shared.kernel.get().unwrap();
        for title in ["first", "second", "third"] {
            append(kernel, title, &[]);
        }
        shared.reads.store(0, Ordering::SeqCst);
        shared.after.store(after, Ordering::SeqCst);
        *shared.armed.lock().unwrap() = Some(request(
            Verb::Sink,
            "urn:iki:ledger:append",
            &[("content", "filed while the listing read")],
        ));
        let text = source(kernel, "urn:iki:ledger:items", &[]);
        if shared.armed.lock().unwrap().take().is_some() {
            break;
        }
        let rows = text
            .lines()
            .filter(|line| line.trim_start().starts_with('#'))
            .count();
        let footer = format!("\n{rows} item(s)\n");
        if !text.contains(&footer) {
            wrong.push((after, text));
        }
    }
    assert!(
        wrong.is_empty(),
        "the plain footer disagreed with its own rows: {wrong:#?}"
    );
}

/// The appended item is not lost: the next listing, from a later read, shows all four.
#[test]
fn the_item_appended_during_a_listing_is_in_the_next_one() {
    let (_, ran, shared) = listing_with_append_after(1, &[]);
    assert!(ran);
    let doc: serde_json::Value = serde_json::from_str(&source(
        shared.kernel.get().unwrap(),
        "urn:iki:ledger:items",
        &[("as", "application/json")],
    ))
    .unwrap();
    assert_eq!(doc["total"], 4);
    assert_eq!(numbers(&doc), vec![4, 3, 2, 1]);
}
