//! **A scale round: `next`, `items`, `count`, one item, `append` and `close` at 1x, 10x and
//! 100x of today's ledger** (ledger #3241, under architecture#2). A measurement, not a check:
//! the one test here is `#[ignore]`d, so the gates never pay for it. Run it by hand, in
//! release, on a quiet machine:
//!
//! ```text
//! cargo test --release --test scale_round -- --ignored --nocapture
//! IKIGAI_LEDGER_SCALES=1,10 cargo test --release --test scale_round -- --ignored --nocapture
//! ```
//!
//! ★ **The store is a scratch one.** Each size gets a fresh RocksDB `DurableStore` in a new
//! directory under the system temp dir, removed when the size is done. Nothing here opens,
//! reads or copies a live store.
//!
//! **How the ledger is built.** The 1x ledger is filed through the ledger's own Sinks
//! (`append`, `link`, `comment`, `claim`, `close`) on an in-memory kernel, so every quad in
//! it is one this crate wrote. 10x and 100x are that ledger's quads copied 10 and 100 times
//! into the scratch store through the store's narrow door: each copy's minted ids get a
//! copy-specific first two characters and its `ledger:number`s are shifted past the copies
//! before it, and the counter is set past the last. Filing 363,200 items one Sink at a time
//! would take hours (a Sink is about 10 ms here); the copies have exactly the shape the
//! Sinks write, at the price of repeating its timestamps and titles once per copy.
//!
//! **Today's shape, as chosen here** (no backup was read): 1,132 open and 2,500 closed items;
//! 70% with a priority, spread over 0-4; two labels each from 24, and `ready` on one in four
//! (the label filter measured); a body on half; every close with a note; 2,450 more
//! comments; 150 `blocks` edges among open items, 200 `parent` edges onto 20 goals, 150
//! `related` edges; 20 open items claimed. That is about 93,000 quads at 1x.
//!
//! **Cold and warm.** The kernel's cache policy is switched: *cold* reads are computed every
//! time (no entry is admitted), which is what every consumer pays for the first read after
//! any write, since every ledger read depends on the store's write threads; *warm* is a read
//! the kernel answers from its cache. Writes are never cached.
//!
//! **The parts.** After the endpoint timings, the store queries the read paths issue are
//! timed one by one, spelled as `src/model.rs` and `src/select.rs` spell them, so a slope
//! can be put on a query or on the Rust around it.

mod common;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ikigai_core::{CacheBound, CachePolicy, EntryFacts, Fallback, Kernel, Lru, Space, Verb};
use ikigai_ledger::vocabulary as v;
use ikigai_store::DurableStore;
use serde_json::Value;

use common::*;

const GRAPH: &str = "urn:iki:ledger:graph:default";
const COUNTER: &str = "urn:iki:ledger:default:counter";
const ITEM_PREFIX: &str = "urn:iki:ledger:default:";
const OPEN: usize = 1_132;
const CLOSED: usize = 2_500;
const ITEMS: usize = OPEN + CLOSED;
const COMMENTS: usize = 2_450;
const CLAIMS: usize = 20;
const LABELS: [&str; 24] = [
    "rust",
    "docs",
    "ci",
    "store",
    "ledger",
    "cli",
    "kernel",
    "security",
    "worker",
    "review",
    "bug",
    "feature",
    "perf",
    "a11y",
    "release",
    "wasm",
    "vocab",
    "sign",
    "log",
    "hub",
    "satellite",
    "dispatcher",
    "import",
    "doctor",
];
const BUDGET: Duration = Duration::from_secs(1);

/// A read measured: its name in the table, its IRI and its arguments.
type Read<'a> = (&'a str, &'a str, Vec<(&'a str, &'a str)>);

// ------------------------------------------------------------------- the cache switch

/// The kernel's default policy, admitting nothing while `on` is false.
struct Switch {
    on: AtomicBool,
    inner: Lru,
}

impl CachePolicy for Switch {
    fn capacity(&self) -> CacheBound {
        self.inner.capacity()
    }

    fn admit(&self, candidate: &EntryFacts<'_>) -> bool {
        self.on.load(Ordering::SeqCst) && self.inner.admit(candidate)
    }

    fn victim(&self, resident: &[EntryFacts<'_>]) -> Option<usize> {
        self.inner.victim(resident)
    }
}

fn switched_kernel(store: DurableStore, switch: Arc<Switch>) -> Kernel {
    let space = Fallback::new(vec![
        Arc::new(ikigai_store::space(store)) as Arc<dyn Space>,
        Arc::new(ikigai_ledger::space()) as Arc<dyn Space>,
    ]);
    Kernel::with_meta_renderer(Arc::new(space), Arc::new(ikigai_vocab::TurtleRenderer))
        .with_clock(Arc::new(TickingClock::default()))
        .with_cache_policy(switch)
}

// ------------------------------------------------------------------- the 1x ledger

/// The labels item `i` is filed with.
fn labels_of(i: usize) -> String {
    let mut labels = vec![LABELS[i % 24], LABELS[(i / 24 + 7) % 24]];
    if i.is_multiple_of(4) {
        labels.push("ready");
    }
    labels.dedup();
    labels.join(",")
}

/// Whether item `i` (1-based) is one of the 2,500 closed: a fixed permutation of the
/// numbers (7919 is prime and does not divide 3,632), so open and closed are interleaved.
fn closed(i: usize) -> bool {
    (i * 7919) % ITEMS < CLOSED
}

/// File today's shape through the ledger's own Sinks and return its quads.
fn template() -> Vec<Quad> {
    let kernel = kernel();
    let started = Instant::now();
    for i in 1..=ITEMS {
        let mut content = format!("Scale item {i}: a title of ordinary length for a ledger line");
        if i % 2 == 0 {
            content.push_str(
                "\n\nA body paragraph, as long as a usual one: what was seen, where, and what \
                 would show it fixed.",
            );
        }
        let labels = labels_of(i);
        let priority = (i % 5).to_string();
        let mut args = vec![("labels", labels.as_str())];
        if i % 10 < 7 {
            args.push(("priority", priority.as_str()));
        }
        append(&kernel, &content, &args);
    }
    let open: Vec<usize> = (1..=ITEMS).filter(|i| !closed(*i)).collect();
    assert_eq!(open.len(), OPEN);
    let link = |item: usize, target: usize, kind: &str| {
        sink(
            &kernel,
            "urn:iki:ledger:link",
            &[
                ("item", &format!("#{item}")),
                ("content", &format!("#{target}")),
                ("type", kind),
            ],
        );
    };
    // Disjoint pairs, lower number blocking higher: no cycle.
    for j in 0..150 {
        link(open[2 * j], open[2 * j + 1], "blocks");
    }
    for j in 0..200 {
        link(open[400 + j], open[1_000 + j % 20], "parent");
    }
    for j in 0..150 {
        let i = 1 + (j * 23) % (ITEMS - 1);
        link(i, i + 1, "related");
    }
    for j in 0..COMMENTS {
        sink(
            &kernel,
            "urn:iki:ledger:comment",
            &[
                ("item", &format!("#{}", 1 + (j * 13) % ITEMS)),
                (
                    "content",
                    "A comment of ordinary length: what was tried and what it showed.",
                ),
            ],
        );
    }
    for j in 0..CLAIMS {
        sink(
            &kernel,
            "urn:iki:ledger:claim",
            &[
                ("item", &format!("#{}", open[700 + j])),
                ("content", "worker"),
            ],
        );
    }
    for i in (1..=ITEMS).filter(|i| closed(*i)) {
        sink(
            &kernel,
            "urn:iki:ledger:close",
            &[
                ("item", &format!("#{i}")),
                ("reason", "done"),
                ("content", "Closed with the pull request that fixed it."),
            ],
        );
    }
    let quads = dump(&kernel);
    println!(
        "1x ledger filed through the Sinks in {:.1}s: {} quads",
        started.elapsed().as_secs_f64(),
        quads.len()
    );
    quads
}

// ------------------------------------------------------------------- copying it

/// One term of the 1x ledger, as a copy must rewrite it.
enum Term {
    Iri(String),
    /// A `ledger:number`, shifted per copy.
    Number(i64),
    /// Any other literal, already in N-Triples form.
    Literal(String),
    Blank(String),
}

struct Quad {
    s: Term,
    p: String,
    o: Term,
}

fn dump(kernel: &Kernel) -> Vec<Quad> {
    let json: Value = serde_json::from_str(&select(
        kernel,
        &format!("SELECT ?s ?p ?o WHERE {{ GRAPH <{GRAPH}> {{ ?s ?p ?o }} }}"),
    ))
    .expect("the store answers JSON");
    let term = |b: &Value, number: bool| -> Term {
        let value = b["value"].as_str().expect("a value").to_string();
        match b["type"].as_str() {
            Some("uri") => Term::Iri(value),
            Some("bnode") => Term::Blank(value),
            _ if number => Term::Number(value.parse().expect("an integer number")),
            _ => {
                let lexical = value
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"")
                    .replace('\n', "\\n")
                    .replace('\r', "\\r");
                Term::Literal(match (b["datatype"].as_str(), b["xml:lang"].as_str()) {
                    (_, Some(lang)) => format!("\"{lexical}\"@{lang}"),
                    (Some(dt), _) => format!("\"{lexical}\"^^<{dt}>"),
                    _ => format!("\"{lexical}\""),
                })
            }
        }
    };
    json["results"]["bindings"]
        .as_array()
        .expect("bindings")
        .iter()
        .map(|row| {
            let p = row["p"]["value"].as_str().expect("a predicate").to_string();
            let s = term(&row["s"], false);
            let number = p == v::NUMBER && !matches!(&s, Term::Iri(s) if s == COUNTER);
            Quad {
                o: term(&row["o"], number),
                s,
                p,
            }
        })
        .collect()
}

/// Copy `c`'s spelling of a minted IRI: every 23-character id segment gets a first two
/// characters no minted id has this century (`z?`), so copies never collide.
fn rewrite(iri: &str, copy: usize) -> String {
    if copy == 0 || !iri.starts_with(ITEM_PREFIX) || iri == COUNTER {
        return iri.to_string();
    }
    const CROCKFORD: &[u8] = b"0123456789abcdefghjkmnpqrstvwxyz";
    let code = format!(
        "{}{}",
        CROCKFORD[31 - copy / 32] as char,
        CROCKFORD[copy % 32] as char
    );
    let out = iri
        .split(':')
        .map(|segment| {
            if segment.len() == 23 {
                format!("{code}{}", &segment[2..])
            } else {
                segment.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(":");
    assert_ne!(out, iri, "an IRI with no minted id: {iri}");
    out
}

fn render(term: &Term, copy: usize, shift: i64) -> String {
    match term {
        Term::Iri(iri) => format!("<{}>", rewrite(iri, copy)),
        Term::Number(n) => (n + shift * copy as i64).to_string(),
        Term::Literal(literal) => literal.clone(),
        Term::Blank(id) => format!("_:c{copy}x{id}"),
    }
}

/// `scale` copies of the 1x ledger into `kernel`'s store, in updates under its 1 MiB bound.
fn load(kernel: &Kernel, quads: &[Quad], scale: usize) {
    let shift = ITEMS as i64;
    let update = |data: &str| {
        sink(
            kernel,
            "urn:iki:store:graph-update",
            &[
                ("graph", GRAPH),
                (
                    "content",
                    &format!("INSERT DATA {{ GRAPH <{GRAPH}> {{ {data} }} }}"),
                ),
            ],
        );
    };
    let mut data = String::new();
    for copy in 0..scale {
        for quad in quads {
            if copy > 0 && matches!(&quad.s, Term::Iri(s) if s == COUNTER) {
                continue;
            }
            data.push_str(&format!(
                "{} <{}> {} .\n",
                render(&quad.s, copy, shift),
                quad.p,
                render(&quad.o, copy, shift)
            ));
            if data.len() > 600 * 1024 {
                update(&data);
                data.clear();
            }
        }
    }
    if !data.is_empty() {
        update(&data);
    }
    update_counter(kernel, v::LAST_NUMBER, (ITEMS * scale) as i64);
}

fn update_counter(kernel: &Kernel, last: &str, to: i64) {
    sink(
        kernel,
        "urn:iki:store:graph-update",
        &[
            ("graph", GRAPH),
            (
                "content",
                &format!(
                    "DELETE {{ GRAPH <{GRAPH}> {{ <{COUNTER}> <{last}> ?n }} }} \
                     INSERT {{ GRAPH <{GRAPH}> {{ <{COUNTER}> <{last}> {to} }} }} \
                     WHERE {{ GRAPH <{GRAPH}> {{ <{COUNTER}> <{last}> ?n }} }}"
                ),
            ),
        ],
    );
}

fn number(kernel: &Kernel, query: &str) -> usize {
    let json: Value = serde_json::from_str(&select(kernel, query)).expect("JSON");
    json["results"]["bindings"][0]["n"]["value"]
        .as_str()
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("a count: {json}"))
}

// ------------------------------------------------------------------- the store queries

/// The `listed_pattern` every listing and count starts from.
fn listed() -> String {
    format!(
        "?item <{}> <{}> ; <{}> ?number ; <{}> ?title ; <{}> ?status .\n",
        v::ext::TYPE,
        v::ITEM_CLASS,
        v::NUMBER,
        v::ext::TITLE,
        v::STATUS
    )
}

fn open_clause() -> String {
    format!("?item <{}> <{}> .\n", v::STATUS, v::OPEN)
}

const RECENCY: &str = "COALESCE(<http://www.w3.org/2001/XMLSchema#dateTime>(?modified), \
                       <http://www.w3.org/2001/XMLSchema#dateTime>(?created))";

/// `model::count_items` for the open filter.
fn count_query() -> String {
    format!(
        "SELECT (COUNT(DISTINCT ?item) AS ?n) WHERE {{ GRAPH <{GRAPH}> {{ {}{} }} }}",
        listed(),
        open_clause()
    )
}

/// The original `model::page_iris` query (now its duplicate-heavy fallback): GROUP BY
/// and ORDER BY over every open item before the LIMIT. Kept for a same-store baseline.
fn page_query(limit: usize) -> String {
    format!(
        "SELECT ?item (MAX({RECENCY}) AS ?recency) (MAX(?number) AS ?n) WHERE {{ GRAPH <{GRAPH}> {{ \
         {}OPTIONAL {{ ?item <{}> ?created }}\nOPTIONAL {{ ?item <{}> ?modified }}\n{} }} }} \
         GROUP BY ?item ORDER BY DESC(?recency) DESC(?n) ?item LIMIT {limit} OFFSET 0",
        listed(),
        v::ext::CREATED,
        v::ext::MODIFIED,
        open_clause()
    )
}

/// `model::page_iris`'s bounded row sort, before deduplicating subjects. On this fixture
/// every item has one row, so its first `limit` subjects must equal the grouped page above.
fn page_rows_query(limit: usize) -> String {
    format!(
        "SELECT ?item WHERE {{ GRAPH <{GRAPH}> {{ \
         {}OPTIONAL {{ ?item <{}> ?created }}\nOPTIONAL {{ ?item <{}> ?modified }}\n{} }} }} \
         ORDER BY DESC({RECENCY}) DESC(?number) ?item LIMIT {limit}",
        listed(),
        v::ext::CREATED,
        v::ext::MODIFIED,
        open_clause()
    )
}

/// `model::load` with no page (what `next` reads through `load_open_items`): every open item
/// with its 28 optional single-valued properties, sorted.
fn load_query() -> String {
    let optionals = [
        (v::ext::CREATED, "created"),
        (v::ext::MODIFIED, "modified"),
        (v::BODY, "body"),
        (v::CLOSED_REASON, "reason"),
        (v::PRIORITY, "priority"),
        (v::DEFERRED, "deferred"),
        (v::AUTHOR, "author"),
        (v::REVISION, "revision"),
        (v::RAISED, "raised"),
        (v::KEY, "key"),
        (v::CLAIMED_BY, "holder"),
        (v::PURPOSE, "purpose"),
        (v::STATE, "state"),
        (v::CLAIM_KIND, "claimKind"),
        (v::LEASE, "lease"),
        (v::LEASE_EXPIRES, "expires"),
        (v::CLAIM_PRINCIPAL, "principal"),
        (v::CLAIM_EPOCH, "epoch"),
        (v::AWAITING, "awaiting"),
        (v::AUTHOR_SOURCE, "authorSource"),
        (v::COMMITTER, "committer"),
        (v::COMMITTED_AT, "committedAt"),
        (v::CREATED_SOURCE, "createdSource"),
        (v::MODIFIED_SOURCE, "modifiedSource"),
        (v::CLAIMED_MODIFIED, "claimedModified"),
    ]
    .iter()
    .map(|(p, var)| format!("OPTIONAL {{ ?item <{p}> ?{var} }}\n"))
    .collect::<String>();
    format!(
        "SELECT * WHERE {{ GRAPH <{GRAPH}> {{ {}{optionals}\
         OPTIONAL {{ ?item <{ty}> ?kind . FILTER(?kind != <{class}>) }}\n{} }} }} \
         ORDER BY DESC({RECENCY}) DESC(?number)",
        listed(),
        open_clause(),
        ty = v::ext::TYPE,
        class = v::ITEM_CLASS,
    )
}

/// `model::fill_multivalued`'s two queries over `iris`, in `VALUES` lists of 256 KiB.
fn fill_queries(iris: &[String]) -> Vec<String> {
    let mut lists = vec![String::new()];
    for iri in iris {
        let last = lists.last_mut().unwrap();
        if last.len() + iri.len() + 3 > 256 * 1024 {
            lists.push(String::new());
        }
        let last = lists.last_mut().unwrap();
        last.push_str(&format!("<{iri}> "));
    }
    lists
        .iter()
        .flat_map(|values| {
            [
                format!(
                    "SELECT ?item ?p ?o WHERE {{ GRAPH <{GRAPH}> {{ VALUES ?item {{ {values} }}\n\
                     ?item ?p ?o .\nFILTER(?p IN (<{}>, <{}>, <{}>, <{}>, <{}>)) }} }}",
                    v::LABEL,
                    v::ABOUT,
                    v::BLOCKS,
                    v::PARENT,
                    v::RELATED
                ),
                format!(
                    "SELECT ?item ?child WHERE {{ GRAPH <{GRAPH}> {{ VALUES ?item {{ {values} }}\n\
                     ?child <{}> ?item ; <{}> <{}> . }} }}",
                    v::PARENT,
                    v::ext::TYPE,
                    v::ITEM_CLASS
                ),
            ]
        })
        .collect()
}

/// `model::defects`' two scans, over every item, open or closed.
fn defect_queries() -> [String; 2] {
    let class = format!("?item <{}> <{}> .\n", v::ext::TYPE, v::ITEM_CLASS);
    [
        format!(
            "SELECT ?item ?missing WHERE {{ GRAPH <{GRAPH}> {{ {class}\
             VALUES ?missing {{ <{}> <{}> <{}> <{}> <{}> }}\n\
             FILTER NOT EXISTS {{ ?item ?missing ?any }} }} }} ORDER BY ?item ?missing",
            v::NUMBER,
            v::ext::TITLE,
            v::STATUS,
            v::ext::CREATED,
            v::ext::MODIFIED
        ),
        format!(
            "SELECT ?item ?p ?value WHERE {{ GRAPH <{GRAPH}> {{ {class}\
             VALUES ?p {{ <{}> <{}> <{}> }}\n?item ?p ?value . }} }} ORDER BY ?item ?p ?value",
            v::NUMBER,
            v::ext::CREATED,
            v::ext::MODIFIED
        ),
    ]
}

/// A lower bound on the quads a close's update copies through the store's scoped door: the
/// quads its patterns with a variable subject match, whatever the item. `evidence::record`
/// and `evidence::standing` find the standing close by `?standing a StatusEvent ; status
/// closed ; eventSequence ?n`, and `counts::status_event_when` counts `?past a StatusEvent`,
/// so every status event, and every closed item's `status`, is copied on every close. The
/// store refuses an update whose copy passes `MAX_SCOPED_COPY_QUADS` (250,000).
fn close_footprint(kernel: &Kernel) -> usize {
    number(
        kernel,
        &format!(
            "SELECT (COUNT(*) AS ?n) WHERE {{ SELECT DISTINCT ?s ?p ?o WHERE {{ GRAPH <{GRAPH}> {{ \
             {{ ?s <{ty}> <{event}> BIND(<{ty}> AS ?p) BIND(<{event}> AS ?o) }} UNION \
             {{ ?s <{status}> <{closed}> BIND(<{status}> AS ?p) BIND(<{closed}> AS ?o) }} UNION \
             {{ ?s <{sequence}> ?o BIND(<{sequence}> AS ?p) }} }} }} }}",
            ty = v::ext::TYPE,
            event = v::STATUS_EVENT_CLASS,
            status = v::STATUS,
            closed = v::CLOSED,
            sequence = v::EVENT_SEQUENCE,
        ),
    )
}

// ------------------------------------------------------------------- timing

fn time(read: &mut impl FnMut()) -> Duration {
    let start = Instant::now();
    read();
    start.elapsed()
}

/// The first of `runs` timings, and their median.
fn first_and_median(runs: usize, mut read: impl FnMut()) -> (Duration, Duration) {
    let mut times: Vec<Duration> = (0..runs).map(|_| time(&mut read)).collect();
    let first = times[0];
    times.sort();
    (first, times[runs / 2])
}

fn secs(d: Duration) -> String {
    let flag = if d > BUDGET { "!" } else { " " };
    format!("{:>8.3}{flag}", d.as_secs_f64())
}

fn scratch_dir(scale: usize) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ikigai-ledger-scale-round-{}-{scale}x",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// ★ The round. One line per read and size; `!` marks a time over the 1 s budget.
#[test]
#[ignore = "a measurement: run it in release, with --ignored --nocapture"]
fn scale_round() {
    let scales: Vec<usize> = std::env::var("IKIGAI_LEDGER_SCALES")
        .unwrap_or_else(|_| "1,10,100".to_string())
        .split(',')
        .map(|s| s.trim().parse().expect("a scale"))
        .collect();
    let quads = template();
    // An open item with comments, in the middle of the 1x numbering.
    let probe = (ITEMS / 2..=ITEMS)
        .find(|i| !closed(*i) && (i - 1) % 13 == 0)
        .expect("an open item with a comment");
    for scale in scales {
        let dir = scratch_dir(scale);
        let switch = Arc::new(Switch {
            on: AtomicBool::new(false),
            inner: Lru::with_bound(CacheBound::default()),
        });
        let kernel = switched_kernel(
            DurableStore::open(&dir).expect("a scratch store"),
            Arc::clone(&switch),
        );
        let started = Instant::now();
        load(&kernel, &quads, scale);
        let total = number(
            &kernel,
            &format!("SELECT (COUNT(*) AS ?n) WHERE {{ GRAPH <{GRAPH}> {{ ?s ?p ?o }} }}"),
        );
        let open = source(&kernel, "urn:iki:ledger:count", &[]);
        assert_eq!(open.trim(), (OPEN * scale).to_string());
        println!(
            "\n== {scale}x: {} open, {} closed, {total} quads, loaded in {:.1}s ==",
            OPEN * scale,
            CLOSED * scale,
            started.elapsed().as_secs_f64()
        );
        let runs = if scale >= 100 { 3 } else { 5 };
        // The probe in the LAST copy, so it is not the first row of any index.
        let item = format!("urn:iki:ledger:item:{}", probe + ITEMS * (scale - 1));
        let reads: Vec<Read> = vec![
            ("next", "urn:iki:ledger:next", vec![]),
            (
                "next as=json",
                "urn:iki:ledger:next",
                vec![("as", "application/json")],
            ),
            ("items", "urn:iki:ledger:items", vec![]),
            (
                "items labels=ready",
                "urn:iki:ledger:items",
                vec![("labels", "ready")],
            ),
            (
                "items limit=1 as=json",
                "urn:iki:ledger:items",
                vec![("limit", "1"), ("as", "application/json")],
            ),
            ("count", "urn:iki:ledger:count", vec![]),
            (
                "count labels=ready",
                "urn:iki:ledger:count",
                vec![("labels", "ready")],
            ),
            ("item:{n}", &item, vec![]),
        ];
        println!(
            "{:<24} {:>9} {:>9} {:>9}   (s; median of {runs}; ! over 1 s)",
            "read", "first", "cold", "warm"
        );
        // ⚠ Every cold read before any warm one: with the cache on, the store queries a
        // ledger read issues are cached too, and a later cold read would hit them.
        let cold: Vec<(Duration, Duration)> = reads
            .iter()
            .map(|(_, iri, args)| {
                first_and_median(runs, || {
                    source(&kernel, iri, args);
                })
            })
            .collect();
        // The parts: the store queries behind those reads, timed alone, still cold.
        let open_iris: Vec<String> = {
            let json: Value = serde_json::from_str(&select(
                &kernel,
                &format!(
                    "SELECT ?item WHERE {{ GRAPH <{GRAPH}> {{ {}{} }} }}",
                    listed(),
                    open_clause()
                ),
            ))
            .expect("JSON");
            json["results"]["bindings"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row["item"]["value"].as_str().unwrap().to_string())
                .collect()
        };
        let [missing, values] = defect_queries();
        let fills = fill_queries(&open_iris);
        for limit in [1, 50] {
            let items = |query: &str| {
                let json: Value = serde_json::from_str(&select(&kernel, query)).expect("JSON");
                json["results"]["bindings"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|row| row["item"].clone())
                    .collect::<Vec<_>>()
            };
            assert_eq!(items(&page_query(limit)), items(&page_rows_query(limit)));
        }
        let parts: Vec<(&str, Vec<String>)> = vec![
            ("q: count (open)", vec![count_query()]),
            ("q: page limit=1", vec![page_query(1)]),
            ("q: page limit=50", vec![page_query(50)]),
            ("q: page rows limit=1", vec![page_rows_query(1)]),
            ("q: page rows limit=50", vec![page_rows_query(50)]),
            ("q: load every open", vec![load_query()]),
            ("q: fill every open", fills),
            ("q: defects (2 scans)", vec![missing, values]),
        ];
        let parts: Vec<(&str, (Duration, Duration))> = parts
            .iter()
            .map(|(name, queries)| {
                let timing = first_and_median(runs, || {
                    for query in queries {
                        select(&kernel, query);
                    }
                });
                (*name, timing)
            })
            .collect();

        switch.on.store(true, Ordering::SeqCst);
        for ((name, iri, args), (first, cold)) in reads.iter().zip(cold) {
            source(&kernel, iri, args);
            let (_, warm) = first_and_median(runs, || {
                source(&kernel, iri, args);
            });
            println!("{name:<24} {} {} {}", secs(first), secs(cold), secs(warm));
        }
        for (name, (first, cold)) in parts {
            println!("{name:<24} {} {}", secs(first), secs(cold));
        }
        // The writes: never cached, so one column.
        let (_, append_time) = first_and_median(runs, || {
            append(
                &kernel,
                "A new item filed during the round",
                &[("labels", "ready")],
            );
        });
        let mut targets = (ITEMS * (scale - 1) + 1..)
            .filter(|n| !closed(n - ITEMS * (scale - 1)))
            .take(runs)
            .map(|n| n.to_string());
        // ⚠ A close can be REFUSED, not only slow: see `close_footprint`.
        let mut refusal = None;
        let (_, close_time) = first_and_median(runs, || {
            let n = targets.next().expect("an open item");
            if let Err(e) = try_verb(
                &kernel,
                Verb::Sink,
                "urn:iki:ledger:close",
                &[("item", &format!("#{n}")), ("reason", "done")],
            ) {
                refusal.get_or_insert(e.to_string());
            }
        });
        println!("{:<24} {:>9} {}", "append", "", secs(append_time));
        println!("{:<24} {:>9} {}", "close", "", secs(close_time));
        println!(
            "close copies at least {} quads (the store refuses past 250,000)",
            close_footprint(&kernel)
        );
        if let Some(refusal) = refusal {
            println!(
                "close REFUSED: {}",
                refusal.chars().take(240).collect::<String>()
            );
        }
        drop(kernel);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
