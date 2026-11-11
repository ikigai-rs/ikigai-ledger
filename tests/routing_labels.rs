//! Audit the label comparisons that decide which worker receives an item.

mod common;

use common::*;
use ikigai_core::Verb;

#[test]
fn next_requires_every_label_and_matches_routing_labels_exactly() {
    let kernel = kernel();
    for (title, labels) in [
        ("Both", "worker:pool,agent:claude"),
        ("Worker only", "worker:pool"),
        ("Agent only", "agent:claude"),
        ("Longer agent", "worker:pool,agent:claude-x"),
        ("Longer pool", "worker:pool-x,agent:claude"),
    ] {
        append(&kernel, title, &[("labels", labels)]);
    }
    let next: serde_json::Value = serde_json::from_str(&source(
        &kernel,
        "urn:iki:ledger:next",
        &[
            ("labels", "worker:pool,agent:claude"),
            ("limit", "0"),
            ("as", "application/json"),
        ],
    ))
    .unwrap();
    let ranking = next["ranking"].as_array().unwrap();
    assert_eq!(ranking.len(), 1, "{next}");
    assert_eq!(ranking[0]["item"]["title"], "Both");
}

#[test]
fn repeated_label_adds_and_removals_leave_one_value_then_none() {
    let kernel = kernel();
    append(&kernel, "Route it", &[]);
    let args = [("item", "1"), ("content", "worker:pool._-9")];
    for (verb, expected) in [(Verb::Sink, 1), (Verb::Delete, 0)] {
        for _ in 0..2 {
            ok(&kernel, verb, "urn:iki:ledger:label", &args);
            let item: serde_json::Value = serde_json::from_str(&source(
                &kernel,
                "urn:iki:ledger:item:1",
                &[("as", "application/json")],
            ))
            .unwrap();
            assert_eq!(item["item"]["labels"].as_array().unwrap().len(), expected);
        }
    }
}
