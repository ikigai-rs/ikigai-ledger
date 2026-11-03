//! Cold counts on a default-sized ledger, through the signed-in reader's store budget.
mod common;

use common::*;
use ikigai_core::{CachePolicy, Capability, EntryFacts, Verb};
use ikigai_ledger::vocabulary as v;
use std::sync::Arc;
use std::time::{Duration, Instant};

const GRAPH: &str = "urn:iki:ledger:graph:default";
const NS: &str = "https://ikigai-rs.dev/ns/ledger#";
const DATE: &str = "http://www.w3.org/2001/XMLSchema#dateTime";

struct NoCache;
impl CachePolicy for NoCache {
    fn admit(&self, _: &EntryFacts<'_>) -> bool {
        false
    }
    fn victim(&self, _: &[EntryFacts<'_>]) -> Option<usize> {
        None
    }
}

// Clean up even when an assertion fails; each process owns its own scratch store.
struct Scratch(std::path::PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn default_sized_counts_stay_well_under_the_signed_in_budget() {
    let scratch = Scratch(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            "target/counts-at-scale-store-{}",
            std::process::id()
        )),
    );
    let k = kernel_over(ikigai_store::DurableStore::open(&scratch.0).unwrap())
        .with_cache_policy(Arc::new(NoCache));
    // 2,900 items, 2,200 closed (1,781 backfilled), three comments and claim/state/edit
    // history per item. Imported status events have no sequence and do not affect counts.
    for first in (0..2900).step_by(40) {
        let mut data = String::new();
        for n in first..(first + 40).min(2900) {
            let item = format!("urn:iki:ledger:default:item:fixture{n}");
            let filed = ikigai_ledger::sparql::iso8601(1_788_220_800_000 + n as u64 * 1000);
            let close_time = ikigai_ledger::sparql::iso8601(1_788_998_400_000 + n as u64 * 1000);
            let closed = n < 2200;
            let status = if closed { v::CLOSED } else { v::OPEN };
            data.push_str(&format!(
                "<{item}> a <{NS}Item> ; <{NS}number> {n} ; \
                 <http://purl.org/dc/terms/title> \"Item {n}\" ; <{NS}status> <{status}> ; \
                 <http://purl.org/dc/terms/created> \"{filed}\"^^<{DATE}> ; \
                 <http://purl.org/dc/terms/modified> \"{close_time}\"^^<{DATE}> ; \
                 <{NS}blocks> <urn:iki:ledger:other:item:{n}> .\n"
            ));
            let sequence = if n < 1781 { 0 } else { 1 };
            if closed {
                data.push_str(&format!(
                    "<{item}> <{NS}closedAt> \"{close_time}\"^^<{DATE}> .\n\
                     <{item}:close> a <{NS}StatusEvent> ; <{NS}onItem> <{item}> ; \
                     <{NS}status> <{status}> ; <{NS}eventSequence> {sequence} ; \
                     <http://purl.org/dc/terms/created> \"{close_time}\"^^<{DATE}> .\n"
                ));
                if n < 1781 {
                    data.push_str(&format!(
                        "<{item}> <{NS}closedAtSource> \"backfill\" . \
                         <{item}:close> <{NS}closedAtSource> \"backfill\" .\n"
                    ));
                }
            }
            if n == 2200 {
                for (suffix, status, day, sequence) in
                    [("close", v::CLOSED, "06", 1), ("reopen", v::OPEN, "07", 2)]
                {
                    data.push_str(&format!(
                        "<{item}:{suffix}> a <{NS}StatusEvent> ; <{NS}onItem> <{item}> ; \
                         <{NS}status> <{status}> ; <{NS}eventSequence> {sequence} ; \
                         <http://purl.org/dc/terms/created> \"2026-09-{day}T00:00:00Z\"^^<{DATE}> .\n"
                    ));
                }
            }
            for (suffix, status) in [("import-close", v::CLOSED), ("import-open", v::OPEN)] {
                data.push_str(&format!(
                    "<{item}:{suffix}> a <{NS}StatusEvent> ; <{NS}onItem> <{item}> ; \
                     <{NS}status> <{status}> ; <{NS}createdSource> \"import\" ; \
                     <http://purl.org/dc/terms/created> \"{filed}\"^^<{DATE}> .\n"
                ));
            }
            for h in 0..9 {
                let (class, fields) = match h {
                    0..=2 => ("Comment", format!("<{NS}body> \"Note for {n}\"")),
                    3 => ("ClaimEvent", format!("<{NS}claimedBy> \"worker\" ; <{NS}claimKind> <{NS}agent>")),
                    4 => ("ReleaseEvent", format!("<{NS}claimedBy> \"worker\" ; <{NS}releaseReason> \"released\"")),
                    5 => ("EditEvent", format!("<{NS}editKind> \"state\" ; <{NS}stateFrom> \"filed\" ; <{NS}stateTo> \"queued\"")),
                    _ => ("EditEvent", format!("<{NS}editKind> \"edit\" ; <{NS}editField> \"title\"")),
                };
                data.push_str(&format!(
                    "<{item}:history{h}> a <{NS}{class}> ; <{NS}onItem> <{item}> ; {fields} ; \
                     <{NS}createdSource> \"import\" ; <{NS}eventKey> \"{n}:{h}\" ; \
                     <{NS}author> <urn:fixture:author> ; <{NS}authorSource> \"import\" ; \
                     <{NS}committer> <urn:fixture:importer> ; \
                     <{NS}committedAt> \"{close_time}\"^^<{DATE}> ; \
                     <http://purl.org/dc/terms/created> \"{filed}\"^^<{DATE}> .\n"
                ));
            }
        }
        sink(
            &k,
            "urn:iki:store:graph-update",
            &[
                ("graph", GRAPH),
                (
                    "content",
                    &format!("INSERT DATA {{ GRAPH <{GRAPH}> {{ {data} }} }}"),
                ),
            ],
        );
    }
    let reader = Capability::scoped([
        "urn:cap:ledger:read:default".to_string(),
        graph_read("default"),
    ]);
    for run in 1..=2 {
        let start = Instant::now();
        let answer = try_as(
            &k,
            &reader,
            Verb::Source,
            "urn:iki:ledger:default:counts",
            &[],
        );
        let elapsed = start.elapsed();
        eprintln!(
            "cold non-root counts, run {run}: {elapsed:?}; {}",
            match &answer {
                Ok(_) => "success".to_string(),
                Err(e) => format!("{e:?}"),
            }
        );
        let csv = answer.expect("counts must fit the non-root store's default five-second budget");
        assert!(elapsed < Duration::from_secs(2), "counts took {elapsed:?}");
        assert_eq!(csv.lines().nth(1), Some("2026-09-01,2900,0"));
        assert!(csv.contains("2026-09-06,2899,1\n2026-09-07,2900,0\n"));
        assert!(csv.contains("2026-09-09,2900,0\n2026-09-10,700,2200\n"));
        assert!(csv.ends_with(",700,2200\n"));
    }
    let turtle = try_as(
        &k,
        &reader,
        Verb::Source,
        "urn:iki:ledger:default:counts",
        &[("as", "text/turtle")],
    )
    .unwrap();
    let quads = oxrdfio::RdfParser::from_format(oxrdfio::RdfFormat::Turtle)
        .for_reader(turtle.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        quads
            .iter()
            .filter(|q| q.predicate.as_str() == v::CLOSED_AT_SOURCE)
            .count(),
        10
    );
    assert!(quads.iter().any(|q| q.predicate.as_str() == v::CLOSED_COUNT
        && q.subject.to_string() == "<urn:iki:ledger:default:counts:2026-09-15>"
        && q.object.to_string().starts_with("\"2200\"")));
    drop(k);
}
