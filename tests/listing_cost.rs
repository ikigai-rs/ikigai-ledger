//! What a listing read COSTS, by face and by part (ledger #1128). A measurement, not a check:
//! every test here is `#[ignore]`d, so the gates never pay for it. Run it by hand, in the
//! build you want numbers for:
//!
//! ```text
//! cargo test --test listing_cost -- --ignored --nocapture
//! cargo test --release --test listing_cost -- --ignored --nocapture
//! ```
//!
//! ⚠ **Over a kernel that caches nothing.** Every read here is cacheable, so on a default
//! kernel the second read of a face is a cache hit and measures the cache. [`NoCache`] admits
//! no entry, so every read is the cold read a consumer pays after each write.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use ikigai_core::{CachePolicy, EntryFacts, Fallback, Kernel, Space, Verb};
use ikigai_ledger::vocabulary as v;
use ikigai_store::DurableStore;

use common::*;

/// A cache policy that admits nothing: every read is computed.
struct NoCache;

impl CachePolicy for NoCache {
    fn admit(&self, _candidate: &EntryFacts<'_>) -> bool {
        false
    }

    fn victim(&self, _resident: &[EntryFacts<'_>]) -> Option<usize> {
        None
    }
}

fn uncached_kernel() -> Kernel {
    let space = Fallback::new(vec![
        Arc::new(ikigai_store::space(
            DurableStore::in_memory().expect("an in-memory store"),
        )) as Arc<dyn Space>,
        Arc::new(ikigai_ledger::space()) as Arc<dyn Space>,
    ]);
    Kernel::with_meta_renderer(Arc::new(space), Arc::new(ikigai_vocab::TurtleRenderer))
        .with_clock(Arc::new(TickingClock::default()))
        .with_cache_policy(Arc::new(NoCache))
}

/// The median of `runs` timings of `read`.
fn median(runs: usize, mut read: impl FnMut()) -> Duration {
    let mut times: Vec<Duration> = (0..runs)
        .map(|_| {
            let start = Instant::now();
            read();
            start.elapsed()
        })
        .collect();
    times.sort();
    times[runs / 2]
}

fn ms(d: Duration) -> String {
    format!("{:>9.2}", d.as_secs_f64() * 1000.0)
}

/// The two scans `model::defects` runs, as written there, at the broad door.
fn defect_queries() -> [String; 2] {
    let graph = "urn:iki:ledger:graph:default";
    let missing = format!(
        "SELECT ?item ?missing WHERE {{ GRAPH <{graph}> {{ ?item <{ty}> <{class}> .\n\
         VALUES ?missing {{ <{number}> <{title}> <{status}> <{created}> <{modified}> }}\n\
         FILTER NOT EXISTS {{ ?item ?missing ?any }} }} }} ORDER BY ?item ?missing",
        ty = v::ext::TYPE,
        class = v::ITEM_CLASS,
        number = v::NUMBER,
        title = v::ext::TITLE,
        status = v::STATUS,
        created = v::ext::CREATED,
        modified = v::ext::MODIFIED,
    );
    let values = format!(
        "SELECT ?item ?p ?value WHERE {{ GRAPH <{graph}> {{ ?item <{ty}> <{class}> .\n\
         VALUES ?p {{ <{number}> <{created}> <{modified}> }}\n?item ?p ?value . }} }} \
         ORDER BY ?item ?p ?value",
        ty = v::ext::TYPE,
        class = v::ITEM_CLASS,
        number = v::NUMBER,
        created = v::ext::CREATED,
        modified = v::ext::MODIFIED,
    );
    [missing, values]
}

/// The COUNT `model::count_items` runs for the default (open) filter.
fn count_query() -> String {
    format!(
        "SELECT (COUNT(DISTINCT ?item) AS ?n) WHERE {{ GRAPH <urn:iki:ledger:graph:default> {{ \
         ?item <{ty}> <{class}> ; <{number}> ?number ; <{title}> ?title ; <{status}> ?status . \
         ?item <{status}> <{open}> . }} }}",
        ty = v::ext::TYPE,
        class = v::ITEM_CLASS,
        number = v::NUMBER,
        title = v::ext::TITLE,
        status = v::STATUS,
        open = v::OPEN,
    )
}

/// ★ The table ledger #1128 asked for: each face read at `limit=1` (all a consumer wanting
/// only the total reads), and the parts it is made of, at 100, 750 and 3000 items.
#[test]
#[ignore = "a measurement: run by hand with --ignored --nocapture"]
fn what_a_one_row_listing_costs() {
    let kernel = uncached_kernel();
    let runs = 7;
    let mut filed = 0;
    println!(
        "\n{:>6} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9}   (median of {runs}, ms)",
        "items", "json", "plain", "turtle", "count", "defects", "count-q"
    );
    for n in [100, 750, 3000] {
        while filed < n {
            append(&kernel, &format!("item {filed}"), &[]);
            filed += 1;
        }
        let json = median(runs, || {
            source(
                &kernel,
                "urn:iki:ledger:items",
                &[("limit", "1"), ("as", "application/json")],
            );
        });
        let plain = median(runs, || {
            source(&kernel, "urn:iki:ledger:items", &[("limit", "1")]);
        });
        let turtle = median(runs, || {
            source(
                &kernel,
                "urn:iki:ledger:items",
                &[("limit", "1"), ("as", "text/turtle")],
            );
        });
        // Absent before the fix: a dash, not a failure, so one harness measures both.
        let count = try_verb(&kernel, Verb::Source, "urn:iki:ledger:count", &[])
            .is_ok()
            .then(|| {
                median(runs, || {
                    source(
                        &kernel,
                        "urn:iki:ledger:count",
                        &[("as", "application/json")],
                    );
                })
            });
        let [missing, values] = defect_queries();
        let defects = median(runs, || {
            select(&kernel, &missing);
            select(&kernel, &values);
        });
        let count_q = count_query();
        let count_only = median(runs, || {
            select(&kernel, &count_q);
        });
        println!(
            "{n:>6} {} {} {} {} {} {}",
            ms(json),
            ms(plain),
            ms(turtle),
            count.map_or_else(|| format!("{:>9}", "-"), ms),
            ms(defects),
            ms(count_only)
        );
    }
}
