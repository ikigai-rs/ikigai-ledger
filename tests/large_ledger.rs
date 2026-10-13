//! **A large ledger, and a long `blocks` chain** (ledger #1422, #1423). Each test failed on
//! 0.11.0 (`8a7ce7c`) because of the defect it names.
//!
//! - A chain of `blocks` links made `next` recurse once per link (`select::find_cycle`), so a
//!   chain about 15,000 long overflowed a 2 MiB stack, the size of a tokio worker's, and
//!   ABORTED the whole process. Below that, leverage walked the downstream once per item:
//!   O(N²), 20 s for a 10,000-item chain against 0.25 s for 10,000 unlinked items (release).
//! - `next` and the JSON faces put every IRI they had loaded into ONE `VALUES` list, so about
//!   19,400 open items passed the store's 1 MiB SPARQL bound and `next` refused on every call.
//!
//! ★ The items here are written straight into the ledger's graph through the store's narrow
//! door, a few hundred per update, because filing 25,000 items one Sink at a time takes
//! minutes. They are exactly what `append` writes (type, number, title, status, created,
//! modified); the links are what `link` writes. Nothing here depends on how they got there.
//!
//! ⚠ The deep chain runs in a CHILD PROCESS (this test binary re-executed with the probe
//! named in its environment) on a 2 MiB thread: a stack overflow aborts the process, which
//! cannot be caught in-process and would read here as a crashed test runner.

mod common;

use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use common::*;
use ikigai_core::{Kernel, Verb};

const LIVE: &str = "urn:iki:ledger:graph:default";
const NS: &str = "https://ikigai-rs.dev/ns/ledger#";
const PROBE: &str = "IKIGAI_LEDGER_CHAIN_PROBE";

/// The IRI of bulk item `n` (1-based): an id as long as a minted one (23 characters), so a
/// list of them is as long as a real ledger's. Zero-padded, so IRI order is filing order,
/// which is the order the cycle check visits items in: the walk from #1 descends the whole
/// chain.
fn item(n: usize) -> String {
    format!("urn:iki:ledger:default:item:bulk{n:06}0000000000000")
}

/// How the bulk items are linked.
#[derive(Clone, Copy)]
enum Links {
    None,
    /// `#k blocks #k+1`: one chain, the oldest at its head.
    Chain,
    /// `#k related #k+1`: as many link targets as items, and no `blocks` graph at all.
    Related,
}

/// `n` open items, written in updates well under the store's 1 MiB bound.
fn bulk(kernel: &Kernel, n: usize, links: Links) {
    const PER_UPDATE: usize = 400;
    let mut first = 1;
    while first <= n {
        let last = (first + PER_UPDATE - 1).min(n);
        let mut data = String::new();
        for k in first..=last {
            // Distinct, increasing stamps: the default policy orders by them.
            let when = format!(
                "\"2026-09-15T{:02}:{:02}:{:02}.{:03}Z\"^^<http://www.w3.org/2001/XMLSchema#dateTime>",
                (k / 3_600_000) % 24,
                (k / 60_000) % 60,
                (k / 1000) % 60,
                k % 1000
            );
            data.push_str(&format!(
                "<{iri}> a <{NS}Item> ; <{NS}number> {k} ; \
                 <http://purl.org/dc/terms/title> \"step {k}\" ; <{NS}status> <{NS}open> ; \
                 <http://purl.org/dc/terms/created> {when} ; \
                 <http://purl.org/dc/terms/modified> {when} .\n",
                iri = item(k)
            ));
            if k < n {
                match links {
                    Links::None => {}
                    Links::Chain => {
                        data.push_str(&format!("<{}> <{NS}blocks> <{}> .\n", item(k), item(k + 1)))
                    }
                    Links::Related => data.push_str(&format!(
                        "<{}> <{NS}related> <{}> .\n",
                        item(k),
                        item(k + 1)
                    )),
                }
            }
        }
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
        first = last + 1;
    }
}

// ------------------------------------------------------------ the VALUES bound (#1423)

/// 25,000 open items, each `related` to the next: past the ~19,400 at which one `VALUES`
/// list of every loaded IRI passed the store's 1 MiB bound. Built once for the suite.
fn large() -> &'static Kernel {
    static LARGE: OnceLock<Kernel> = OnceLock::new();
    LARGE.get_or_init(|| {
        let kernel = kernel();
        bulk(&kernel, 25_000, Links::Related);
        kernel
    })
}

/// `next` loads every open item and fills their labels, links and `about` in one more query.
/// On 0.11.0 that query listed all 25,000 IRIs and the store refused it, so `next` failed
/// on every call, whatever its arguments.
#[test]
fn next_answers_over_twenty_five_thousand_open_items() {
    let answer = try_verb(large(), Verb::Source, "urn:iki:ledger:next", &[]);
    let text = answer.unwrap_or_else(|e| panic!("next over 25,000 items: {e}"));
    assert!(text.contains("ready: 25000"), "{text}");
}

/// The JSON face renders every ranked item with its link targets' numbers and its comments,
/// two more lists of every IRI. `limit=0` ranks the whole ready set.
#[test]
fn next_as_json_answers_over_twenty_five_thousand_open_items() {
    let answer = try_verb(
        large(),
        Verb::Source,
        "urn:iki:ledger:next",
        &[("as", "application/json"), ("limit", "0")],
    );
    let text = answer.unwrap_or_else(|e| panic!("next as JSON over 25,000 items: {e}"));
    let doc: serde_json::Value = serde_json::from_str(&text).expect("JSON");
    let ranking = doc["ranking"].as_array().expect("a ranking");
    assert_eq!(ranking.len(), 25_000);
    // The link targets were numbered: every item but the last is related to the next.
    let linked = ranking
        .iter()
        .find(|r| r["item"]["number"] == 1)
        .expect("#1 is ranked");
    assert_eq!(
        linked["item"]["links"][0]["target"]["number"], 2,
        "{}",
        linked["item"]["links"]
    );
}

/// A listing's page is loaded by IRI too: `items limit=25000` listed them all in one
/// `VALUES` and was refused the same way.
#[test]
fn a_listing_page_of_twenty_five_thousand_items_answers() {
    let plain = try_verb(
        large(),
        Verb::Source,
        "urn:iki:ledger:items",
        &[("limit", "25000")],
    )
    .unwrap_or_else(|e| panic!("items limit=25000: {e}"));
    assert!(
        plain.contains("\n25000 item(s)\n"),
        "{}",
        &plain[plain.len() - 200..]
    );
    let json = try_verb(
        large(),
        Verb::Source,
        "urn:iki:ledger:items",
        &[("limit", "25000"), ("as", "application/json")],
    )
    .unwrap_or_else(|e| panic!("items limit=25000 as JSON: {e}"));
    let doc: serde_json::Value = serde_json::from_str(&json).expect("JSON");
    assert_eq!(doc["count"], 25_000);
}

// ------------------------------------------------------------- the deep chain (#1422)

/// The child's half: inert unless a parent named a probe. Builds the chain, then resolves
/// `next` on a 2 MiB thread, prints the outcome, and exits before the harness can.
#[test]
fn probe_child() {
    let Ok(spec) = std::env::var(PROBE) else {
        return;
    };
    let n: usize = spec.parse().expect("a chain length");
    let kernel = kernel();
    bulk(&kernel, n, Links::Chain);
    let outcome = std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(
            move || match try_verb(&kernel, Verb::Source, "urn:iki:ledger:next", &[]) {
                Ok(text) => format!("ok {}", text.lines().next().unwrap_or_default()),
                Err(e) => format!("err {}", e.to_string().replace('\n', " ")),
            },
        )
        .expect("spawn")
        .join()
        .expect("the next thread");
    println!("\nOUTCOME {outcome}");
    std::process::exit(0);
}

/// The parent's half: run the probe in a child and return what it said, or panic naming how
/// it died. An abort is the defect.
fn probe(n: usize) -> String {
    let out = Command::new(std::env::current_exe().expect("this test binary"))
        .args(["--exact", "probe_child", "--nocapture", "--test-threads=1"])
        .env(PROBE, n.to_string())
        .output()
        .expect("the child ran");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "a {n}-item blocks chain did not survive `next` on a 2 MiB thread: {} — {}",
        out.status,
        stderr
            .lines()
            .find(|l| l.contains("overflow"))
            .unwrap_or(&stderr)
    );
    let at = stdout
        .find("\nOUTCOME ")
        .unwrap_or_else(|| panic!("the probe reported nothing: {stdout}"));
    stdout[at + "\nOUTCOME ".len()..]
        .lines()
        .next()
        .unwrap_or("")
        .to_string()
}

/// ★ On 0.11.0 a 15,000-item chain aborted the process (SIGABRT, "has overflowed its
/// stack"); 20,000 is past it with room. The head of the chain is the one ready item.
///
/// ⚠ 15,000 is here as well because at 20,000 0.11.0 failed first on the other defect:
/// its one `VALUES` list of every open item passed the store's bound (ledger #1423) before
/// the cycle check ran. 15,000 items are under that bound, so on 0.11.0 they reach the
/// recursion, and the child aborts.
#[test]
fn a_twenty_thousand_item_chain_answers_and_aborts_nothing() {
    for n in [15_000, 20_000] {
        let outcome = probe(n);
        assert!(outcome.starts_with("ok "), "{n}: {outcome}");
        assert!(
            outcome.contains("step 1"),
            "{n}: the head of the chain is ready: {outcome}"
        );
    }
}

/// ★ A guard, not a benchmark: 10,000 linked items rank in the same order of time as
/// 10,000 unlinked ones. On 0.11.0 leverage walked the downstream once per item, and the
/// chain took 80 times as long (20 s against 0.25 s, release). The bound is generous on
/// purpose: a busy CI machine must not fail it, and a quadratic walk cannot pass it.
#[test]
fn a_long_chain_ranks_in_about_the_time_of_unlinked_items() {
    fn timed(links: Links) -> Duration {
        let kernel = kernel();
        bulk(&kernel, 10_000, links);
        let started = Instant::now();
        let answer = try_verb(&kernel, Verb::Source, "urn:iki:ledger:next", &[]);
        let took = started.elapsed();
        answer.unwrap_or_else(|e| panic!("next over 10,000 items: {e}"));
        took
    }
    let unlinked = timed(Links::None);
    let linked = timed(Links::Chain);
    assert!(
        linked <= unlinked * 8 + Duration::from_secs(3),
        "10,000 chained items took {linked:?} against {unlinked:?} unlinked"
    );
}
