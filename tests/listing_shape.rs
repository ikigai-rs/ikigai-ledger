//! **`omit=` and `comments=` on `items` and `next`** (ledger #3781): a list row can leave out
//! the body, the comments and the attribution, and the work it leaves out is not done.
//!
//! A face drawing a list shows none of that text, and on one deployment it was 78% of 200 open
//! items' JSON. The rows stay schema 1: an omitted `body` is `""`, omitted `comments` are `[]`
//! and an omitted `attribution` is absent (`null`), and every row's new `omitted` says which
//! of those were left out rather than empty.

mod common;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use common::*;
use ikigai_core::{
    CachePolicy, Endpoint, EntryFacts, Error, Fallback, Invocation, Kernel, Representation,
    Resolution, Result, Scope, Space, Verb,
};
use ikigai_ledger::vocabulary as v;
use ikigai_store::DurableStore;
use serde_json::{json, Value};

const JSON: (&str, &str) = ("as", "application/json");

/// Records every query sent to the store's `graph-select` door.
struct Recording {
    inner: Arc<dyn Space>,
    queries: Arc<Mutex<Vec<String>>>,
}

struct RecordingEndpoint {
    inner: Arc<dyn Endpoint>,
    queries: Arc<Mutex<Vec<String>>>,
}

impl Space for Recording {
    fn resolve(&self, request: &ikigai_core::Request, scope: &Scope) -> Resolution {
        let queries = Arc::clone(&self.queries);
        self.inner
            .resolve(request, scope)
            .map_endpoint(move |inner| Arc::new(RecordingEndpoint { inner, queries }) as _)
    }
}

#[async_trait]
impl Endpoint for RecordingEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        if inv.request.target.as_str() == "urn:iki:store:graph-select" {
            if let Ok(query) = inv.inline_str("query") {
                self.queries.lock().unwrap().push(query.to_string());
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

/// A cache policy that admits nothing, so every store read reaches the recorder: a cached
/// sub-request would not, and would look like a query skipped.
struct NoCache;

impl CachePolicy for NoCache {
    fn admit(&self, _candidate: &EntryFacts<'_>) -> bool {
        false
    }

    fn victim(&self, _resident: &[EntryFacts<'_>]) -> Option<usize> {
        None
    }
}

/// A kernel whose store reads are recorded, caching nothing.
fn recorded() -> (Kernel, Arc<Mutex<Vec<String>>>) {
    let queries = Arc::new(Mutex::new(Vec::new()));
    let store = Arc::new(ikigai_store::space(DurableStore::in_memory().unwrap())) as Arc<dyn Space>;
    let space = Fallback::new(vec![
        Arc::new(Recording {
            inner: store,
            queries: Arc::clone(&queries),
        }) as Arc<dyn Space>,
        Arc::new(ikigai_ledger::space()) as Arc<dyn Space>,
    ]);
    let kernel =
        Kernel::with_meta_renderer(Arc::new(space), Arc::new(ikigai_vocab::TurtleRenderer))
            .with_clock(Arc::new(TickingClock::default()))
            .with_cache_policy(Arc::new(NoCache));
    (kernel, queries)
}

/// The comments queries among `queries`: the ones that read comments on a set of items.
fn comment_reads(queries: &[String]) -> Vec<String> {
    queries
        .iter()
        .filter(|q| q.contains(&format!("?comment <{}> ?item", v::ON_ITEM)))
        .cloned()
        .collect()
}

/// Seeds `ledger` with three items: `#1` with a body, attribution and three comments, `#2`
/// with a body and one comment, `#3` with neither.
fn seed(kernel: &Kernel, ledger: &str) {
    let append = format!("urn:iki:ledger:{ledger}:append");
    let comment = format!("urn:iki:ledger:{ledger}:comment");
    sink(
        kernel,
        &append,
        &[("content", "Many\n\nThe first body."), ("author", "brian")],
    );
    sink(
        kernel,
        &append,
        &[("content", "One\n\nThe second body."), ("author", "chris")],
    );
    sink(kernel, &append, &[("content", "None")]);
    for text in ["first", "second", "third"] {
        sink(
            kernel,
            &comment,
            &[("item", "1"), ("content", text), ("author", "chris")],
        );
    }
    sink(
        kernel,
        &comment,
        &[("item", "2"), ("content", "only"), ("author", "brian")],
    );
}

fn read(kernel: &Kernel, iri: &str, args: &[(&str, &str)]) -> Value {
    let mut all = vec![JSON];
    all.extend_from_slice(args);
    serde_json::from_str(&source(kernel, iri, &all)).expect("the JSON face parses")
}

fn refused(kernel: &Kernel, iri: &str, args: &[(&str, &str)]) -> (String, String) {
    match try_verb(kernel, Verb::Source, iri, args) {
        Err(Error::InvalidArgument { name, detail }) => (name, detail),
        other => panic!("expected InvalidArgument, got {other:?}"),
    }
}

/// The rows of an `items` document, by title.
fn rows(doc: &Value) -> Vec<(String, Value)> {
    doc["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|row| (row["title"].as_str().unwrap().to_string(), row.clone()))
        .collect()
}

fn row<'a>(rows: &'a [(String, Value)], title: &str) -> &'a Value {
    &rows
        .iter()
        .find(|(t, _)| t == title)
        .unwrap_or_else(|| panic!("no row {title}"))
        .1
}

fn texts(row: &Value) -> Vec<&str> {
    row["comments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["text"].as_str().unwrap())
        .collect()
}

#[test]
fn the_default_is_the_whole_item_with_nothing_omitted() {
    let kernel = kernel();
    seed(&kernel, "default");
    let doc = read(&kernel, "urn:iki:ledger:items", &[]);
    let rows = rows(&doc);
    let many = row(&rows, "Many");
    assert_eq!(many["body"], "The first body.");
    assert_eq!(texts(many), ["first", "second", "third"]);
    assert_eq!(many["attribution"]["author"], "brian");
    assert_eq!(many["comments"][0]["attribution"]["author"], "chris");
    for (_, row) in &rows {
        assert_eq!(row["omitted"], json!([]), "{row}");
    }
    // `omitted` is the last field of every row, in the served bytes.
    let raw = source(&kernel, "urn:iki:ledger:items", &[JSON]);
    assert_eq!(raw.matches(r#","omitted":[]}"#).count(), 3, "{raw}");
    // The defaults, named, are the same document.
    assert_eq!(
        read(
            &kernel,
            "urn:iki:ledger:items",
            &[("comments", "all"), ("omit", "")]
        ),
        doc
    );
}

#[test]
fn omit_all_three_leaves_them_out_and_says_so() {
    let kernel = kernel();
    seed(&kernel, "default");
    let doc = read(
        &kernel,
        "urn:iki:ledger:items",
        &[("omit", "body,comments,attribution")],
    );
    for (title, row) in rows(&doc) {
        assert_eq!(row["body"], "", "{title}");
        assert_eq!(row["comments"], json!([]), "{title}");
        assert_eq!(row["attribution"], Value::Null, "{title}");
        assert_eq!(
            row["omitted"],
            json!(["attribution", "body", "comments"]),
            "{title}"
        );
        // Everything else is the row as it was.
        assert!(row["title"].is_string() && row["status"] == "open", "{row}");
    }
    // The order of `omit=` does not matter; `omitted` is sorted.
    let one = read(
        &kernel,
        "urn:iki:ledger:items",
        &[("omit", " comments , body ")],
    );
    assert_eq!(
        row(&rows(&one), "Many")["omitted"],
        json!(["body", "comments"])
    );
    assert_eq!(row(&rows(&one), "Many")["attribution"]["author"], "brian");
}

#[test]
fn omit_attribution_reaches_the_comments_too() {
    let kernel = kernel();
    seed(&kernel, "default");
    let doc = read(&kernel, "urn:iki:ledger:items", &[("omit", "attribution")]);
    let rows = rows(&doc);
    let many = row(&rows, "Many");
    assert_eq!(texts(many), ["first", "second", "third"]);
    assert_eq!(many["body"], "The first body.");
    for comment in many["comments"].as_array().unwrap() {
        assert!(comment.get("attribution").is_none(), "{comment}");
        assert_eq!(comment["author"], "chris");
    }
    assert_eq!(many["omitted"], json!(["attribution"]));
}

#[test]
fn comments_last_carries_exactly_the_newest() {
    let kernel = kernel();
    seed(&kernel, "default");
    let all = read(&kernel, "urn:iki:ledger:items", &[]);
    let last = read(&kernel, "urn:iki:ledger:items", &[("comments", "last")]);
    let (all, last) = (rows(&all), rows(&last));
    // Several comments: the newest, whole, and `comments` named as left out.
    let many = row(&last, "Many");
    assert_eq!(texts(many), ["third"]);
    assert_eq!(many["comments"][0], row(&all, "Many")["comments"][2]);
    assert_eq!(many["omitted"], json!(["comments"]));
    // One comment: it is all of them, so nothing is left out.
    let one = row(&last, "One");
    assert_eq!(texts(one), ["only"]);
    assert_eq!(one["comments"], row(&all, "One")["comments"]);
    assert_eq!(one["omitted"], json!([]));
    // None: nothing to leave out.
    assert_eq!(row(&last, "None")["comments"], json!([]));
    assert_eq!(row(&last, "None")["omitted"], json!([]));
}

#[test]
fn comments_none_is_omit_comments() {
    let kernel = kernel();
    seed(&kernel, "default");
    let none = read(&kernel, "urn:iki:ledger:items", &[("comments", "none")]);
    let omit = read(&kernel, "urn:iki:ledger:items", &[("omit", "comments")]);
    assert_eq!(none, omit);
    let both = read(
        &kernel,
        "urn:iki:ledger:items",
        &[("omit", "comments"), ("comments", "none")],
    );
    assert_eq!(none, both);
    let rows = rows(&none);
    assert_eq!(row(&rows, "Many")["comments"], json!([]));
    assert_eq!(row(&rows, "Many")["omitted"], json!(["comments"]));
    assert_eq!(row(&rows, "Many")["body"], "The first body.");
}

#[test]
fn next_is_shaped_the_same_way() {
    let kernel = kernel();
    seed(&kernel, "default");
    let shaped = |args: &[(&str, &str)]| {
        let mut all = vec![("limit", "10")];
        all.extend_from_slice(args);
        let doc = read(&kernel, "urn:iki:ledger:next", &all);
        doc["ranking"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                (
                    r["item"]["title"].as_str().unwrap().to_string(),
                    r["item"].clone(),
                )
            })
            .collect::<Vec<_>>()
    };
    let whole = shaped(&[]);
    assert_eq!(whole.len(), 3);
    assert_eq!(row(&whole, "Many")["omitted"], json!([]));
    assert_eq!(texts(row(&whole, "Many")), ["first", "second", "third"]);
    for (title, item) in shaped(&[("omit", "body,comments,attribution")]) {
        assert_eq!(item["body"], "", "{title}");
        assert_eq!(item["comments"], json!([]), "{title}");
        assert_eq!(item["attribution"], Value::Null, "{title}");
        assert_eq!(item["omitted"], json!(["attribution", "body", "comments"]));
    }
    let last = shaped(&[("comments", "last")]);
    assert_eq!(texts(row(&last, "Many")), ["third"]);
    assert_eq!(row(&last, "Many")["omitted"], json!(["comments"]));
    assert_eq!(row(&last, "One")["omitted"], json!([]));
}

#[test]
fn bad_and_contradictory_values_are_refused() {
    let kernel = kernel();
    seed(&kernel, "default");
    for iri in ["urn:iki:ledger:items", "urn:iki:ledger:next"] {
        let (name, detail) = refused(&kernel, iri, &[JSON, ("omit", "body,title")]);
        assert_eq!(name, "omit");
        assert!(detail.contains("`title`"), "{detail}");
        assert!(detail.contains("body, comments, attribution"), "{detail}");

        let (name, detail) = refused(&kernel, iri, &[JSON, ("comments", "first")]);
        assert_eq!(name, "comments");
        assert!(detail.contains("all, last, none"), "{detail}");

        let (name, detail) = refused(
            &kernel,
            iri,
            &[JSON, ("omit", "comments"), ("comments", "last")],
        );
        assert_eq!(name, "omit");
        assert!(detail.contains("contradict"), "{detail}");
        // ⚠ `comments=all` is the declared default, which the kernel injects before the
        // endpoint runs (ikigai-core, ledger #19), so it cannot be told from no `comments=` at
        // all: `omit=comments` beside it is served as `comments=none`.
        assert_eq!(
            read(&kernel, iri, &[("omit", "comments"), ("comments", "all")]),
            read(&kernel, iri, &[("omit", "comments")])
        );
    }
}

/// ★ The plain and Turtle faces refuse a shape that leaves anything out, naming the face it
/// applies to; they serve the defaults, which change nothing.
#[test]
fn the_plain_and_turtle_faces_refuse_a_shape() {
    let kernel = kernel();
    seed(&kernel, "default");
    for iri in ["urn:iki:ledger:items", "urn:iki:ledger:next"] {
        for face in [None, Some(("as", "text/turtle"))] {
            let with = |args: &[(&'static str, &'static str)]| {
                let mut all: Vec<(&str, &str)> = face.into_iter().collect();
                all.extend_from_slice(args);
                all
            };
            let (name, detail) = refused(&kernel, iri, &with(&[("omit", "body")]));
            assert_eq!(name, "omit");
            assert!(detail.contains("as=application/json"), "{detail}");
            let (name, detail) = refused(&kernel, iri, &with(&[("comments", "last")]));
            assert_eq!(name, "comments");
            assert!(detail.contains("JSON face only"), "{detail}");
            assert_eq!(
                source(&kernel, iri, &with(&[("comments", "all")])),
                source(&kernel, iri, &with(&[]))
            );
        }
    }
}

#[test]
fn a_union_listing_honors_both_inputs() {
    let kernel = kernel();
    seed(&kernel, "alpha");
    seed(&kernel, "beta");
    let doc = read(
        &kernel,
        "urn:iki:ledger:items",
        &[
            ("ledgers", "alpha,beta"),
            ("omit", "body,attribution"),
            ("comments", "last"),
        ],
    );
    let items = doc["items"].as_array().unwrap();
    assert_eq!(items.len(), 6);
    for item in items {
        assert_eq!(item["body"], "");
        assert_eq!(item["attribution"], Value::Null);
        let expected = match item["title"].as_str().unwrap() {
            "Many" => {
                assert_eq!(texts(item), ["third"]);
                json!(["attribution", "body", "comments"])
            }
            "One" => {
                assert_eq!(texts(item), ["only"]);
                json!(["attribution", "body"])
            }
            _ => json!(["attribution", "body"]),
        };
        assert_eq!(item["omitted"], expected, "{item}");
    }
    let none = read(
        &kernel,
        "urn:iki:ledger:items",
        &[("ledgers", "alpha,beta"), ("omit", "comments")],
    );
    for item in none["items"].as_array().unwrap() {
        assert_eq!(item["comments"], json!([]));
        assert_eq!(item["omitted"], json!(["comments"]));
    }
}

/// ★ The work a shape skips is not done: `comments=none` and `omit=comments` issue no comments
/// query, on `items` and on `next`, and `comments=last` issues the bounded one.
#[test]
fn comments_none_issues_no_comments_query() {
    let (kernel, queries) = recorded();
    seed(&kernel, "default");
    let reads_for = |iri: &str, args: &[(&str, &str)]| {
        queries.lock().unwrap().clear();
        read(&kernel, iri, args);
        comment_reads(&queries.lock().unwrap())
    };
    for iri in ["urn:iki:ledger:items", "urn:iki:ledger:next"] {
        // The control: the recorder does see the comments query.
        let all = reads_for(iri, &[]);
        assert_eq!(all.len(), 1, "{iri}: {all:?}");
        assert!(!all[0].contains("MAX("), "{}", all[0]);
        assert_eq!(
            reads_for(iri, &[("comments", "none")]),
            Vec::<String>::new()
        );
        assert_eq!(
            reads_for(iri, &[("omit", "comments")]),
            Vec::<String>::new()
        );
        let last = reads_for(iri, &[("comments", "last")]);
        assert_eq!(last.len(), 1, "{iri}: {last:?}");
        assert!(last[0].contains("MAX(STR(?atLatest))"), "{}", last[0]);
    }
}

/// Meta describes both inputs, with `comments`'s values and default.
#[test]
fn meta_describes_the_inputs() {
    let kernel = kernel();
    for iri in ["urn:iki:ledger:items", "urn:iki:ledger:next"] {
        let meta = ok(&kernel, Verb::Meta, iri, &[]);
        assert!(meta.contains("omit"), "{meta}");
        assert!(meta.contains("comments"), "{meta}");
        assert!(meta.contains("\"last\""), "{meta}");
    }
}
