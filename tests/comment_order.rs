//! **Comments stamped in the same millisecond list in the order they were written**
//! (ledger #813).
//!
//! A bulk importer files comments back to back, and two of them can carry one
//! `dcterms:created`. `item:{id}` ordered its comments by that stamp alone, so a tie came
//! back in whatever order the store's evaluator produced, and a kata import showed C2 above
//! C1. The tie-break is the comment's id, whose suffix is the process's mint sequence: within
//! one process (and a store has one writer process) ids minted in one millisecond sort in
//! the order they were minted.

mod common;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use common::*;
use ikigai_core::{Clock, Fallback, Kernel, Space, Time};
use ikigai_store::DurableStore;

/// A clock that never moves: every write lands in the same millisecond.
struct Frozen(AtomicU64);

impl Clock for Frozen {
    fn now(&self) -> Time {
        Time::from_millis(self.0.load(Ordering::SeqCst))
    }
}

fn frozen_kernel() -> Kernel {
    let space = Fallback::new(vec![
        Arc::new(ikigai_store::space(DurableStore::in_memory().unwrap())) as Arc<dyn Space>,
        Arc::new(ikigai_ledger::space()) as Arc<dyn Space>,
    ]);
    Kernel::with_meta_renderer(Arc::new(space), Arc::new(ikigai_vocab::TurtleRenderer))
        .with_clock(Arc::new(Frozen(AtomicU64::new(1_789_430_400_000))))
}

/// The comment bodies in the order a face lists them.
fn order_in(text: &str, bodies: &[String]) -> Vec<String> {
    let mut found: Vec<(usize, String)> = bodies
        .iter()
        .map(|b| {
            (
                text.find(b.as_str()).expect("every comment is listed"),
                b.clone(),
            )
        })
        .collect();
    found.sort();
    found.into_iter().map(|(_, b)| b).collect()
}

/// ★ Twenty comments in one millisecond, read back through every face that lists them: the
/// plain and Turtle faces of `item:{id}` (which read `load_comments`) and its JSON face (which
/// reads `load_comments_for`). On 0.12.0 the first ordered by the stamp alone.
#[test]
fn comments_in_one_millisecond_list_in_the_order_they_were_written() {
    let kernel = frozen_kernel();
    append(&kernel, "an imported item", &[]);
    // Zero-padded, so no body is a prefix of another and `find` locates each one exactly.
    let bodies: Vec<String> = (0..20)
        .map(|n| format!("imported comment {n:02}"))
        .collect();
    for body in &bodies {
        sink(
            &kernel,
            "urn:iki:ledger:comment",
            &[("item", "#1"), ("content", body)],
        );
    }

    let plain = source(&kernel, "urn:iki:ledger:item:1", &[]);
    assert_eq!(order_in(&plain, &bodies), bodies, "plain face:\n{plain}");

    let json = source(
        &kernel,
        "urn:iki:ledger:item:1",
        &[("as", "application/json")],
    );
    assert_eq!(order_in(&json, &bodies), bodies, "JSON face:\n{json}");
}
