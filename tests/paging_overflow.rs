//! **A listing's footer at the edge of `usize`** (ledger #1420). `limit` and `offset` take
//! any count, and the store accepts `LIMIT 18446744073709551615`, so the listing itself
//! succeeds. On 0.11.0 (`8a7ce7c`) the footer then added them unchecked: a panic ("attempt
//! to add with overflow") in a debug build, and in a release build a wrapped footer,
//! `2–0 of 2 item(s) — pass offset=0 for the next page`, that sent the reader back to the
//! start. These tests run in the debug build CI makes, so the first failed by panicking.

mod common;

use common::*;

fn two_items() -> ikigai_core::Kernel {
    let kernel = kernel();
    append(&kernel, "one", &[]);
    append(&kernel, "two", &[]);
    kernel
}

/// The largest limit with an offset: the page is the last item, and the footer says it is
/// the end of the list.
#[test]
fn the_largest_limit_with_an_offset_has_a_true_footer() {
    let kernel = two_items();
    let max = usize::MAX.to_string();
    let listed = source(
        &kernel,
        "urn:iki:ledger:items",
        &[("limit", &max), ("offset", "1")],
    );
    assert!(listed.contains("one"), "{listed}");
    assert!(!listed.contains("two"), "{listed}");
    assert!(listed.ends_with("\n2–2 of 2 item(s)\n"), "{listed}");
    assert!(!listed.contains("pass offset="), "{listed}");
}

/// Both at the largest count: past the end, said as such, and nothing panics.
#[test]
fn the_largest_offset_and_limit_answer_past_the_end() {
    let kernel = two_items();
    let max = usize::MAX.to_string();
    let listed = source(
        &kernel,
        "urn:iki:ledger:items",
        &[("limit", &max), ("offset", &max)],
    );
    assert!(
        listed.starts_with("no items on this page: 2 item(s) match"),
        "{listed}"
    );
    let json = source(
        &kernel,
        "urn:iki:ledger:items",
        &[
            ("limit", &max),
            ("offset", &max),
            ("as", "application/json"),
        ],
    );
    let doc: serde_json::Value = serde_json::from_str(&json).expect("JSON");
    assert_eq!(doc["count"], 0, "{doc}");
    assert_eq!(doc["total"], 2, "{doc}");
}

/// The store's row and byte bounds are refusals, never partial successful pages.
#[test]
fn an_oversized_store_answer_does_not_become_a_partial_listing() {
    use std::sync::Arc;

    use ikigai_core::{Capability, Fallback, Kernel, Space, Verb};
    use ikigai_store::budget::{AnswerBound, AnswerBudget};
    use ikigai_store::DurableStore;

    for (rows, bytes, title, bound_name) in [
        (2, 1 << 20, "small".to_string(), "max_rows"),
        (100, 1024, "large".repeat(1000), "max_bytes"),
    ] {
        let store = DurableStore::in_memory()
            .unwrap()
            .with_answer_budget(AnswerBudget::new(AnswerBound::new(rows, bytes).unwrap()));
        let space = Fallback::new(vec![
            Arc::new(ikigai_store::space(store)) as Arc<dyn Space>,
            Arc::new(ikigai_ledger::space()) as Arc<dyn Space>,
        ]);
        let kernel =
            Kernel::with_meta_renderer(Arc::new(space), Arc::new(ikigai_vocab::TurtleRenderer))
                .with_clock(Arc::new(TickingClock::default()));
        for _ in 0..3 {
            append(&kernel, &title, &[]);
        }
        let cap = Capability::scoped([
            "urn:cap:ledger:read:default".to_string(),
            graph_read("default"),
        ]);
        for sort in ["-modified", "number"] {
            let error = try_as(
                &kernel,
                &cap,
                Verb::Source,
                "urn:iki:ledger:items",
                &[("sort", sort), ("as", "application/json")],
            )
            .expect_err("an oversized answer must fail as a whole")
            .to_string();
            assert!(error.contains(bound_name), "{sort}: {error}");
        }
        // Counting needs one small row and still answers the complete total.
        assert_eq!(
            try_as(&kernel, &cap, Verb::Source, "urn:iki:ledger:count", &[]).unwrap(),
            "3\n"
        );
    }
}
