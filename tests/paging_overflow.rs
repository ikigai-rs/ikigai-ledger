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
