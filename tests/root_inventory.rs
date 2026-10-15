//! **The root inventory resolves no broad store door** (ledger #272).
//!
//! `urn:iki:ledger:ledgers` asks which ledgers exist. A scoped capability carries the answer
//! in its grants; root carries none, so until 0.12.0 the root path asked the whole store
//! through the broad `urn:iki:store:select` — an authority no action here declares. The store
//! now answers the question itself (`urn:iki:store:graphs`, the named graphs that exist and
//! that the caller may read), so root takes the same narrow per-graph query the scoped path
//! runs, over that list.

mod common;

use std::sync::Arc;

use async_trait::async_trait;
use common::*;
use ikigai_core::{
    Description, Endpoint, EndpointSpace, Error, Exact, Fallback, Invocation, Kernel,
    Representation, Result, Space, Verb,
};
use ikigai_store::DurableStore;

/// Stands in front of the broad read door and refuses every request to it, so a listing that
/// still resolves it fails here instead of quietly succeeding.
struct NoBroadDoor;

#[async_trait]
impl Endpoint for NoBroadDoor {
    async fn invoke(&self, _inv: &Invocation<'_>) -> Result<Representation> {
        Err(Error::Denied(
            "urn:iki:store:select was resolved: the inventory used the broad door".to_string(),
        ))
    }

    fn name(&self) -> &str {
        "no-broad-door"
    }

    fn describe(&self) -> Description {
        Description::new("no-broad-door").verb(Verb::Source)
    }
}

fn kernel_without_the_broad_door() -> Kernel {
    let space = Fallback::new(vec![
        Arc::new(
            EndpointSpace::new().bind(Exact::new("urn:iki:store:select"), NoBroadDoor),
        ) as Arc<dyn Space>,
        Arc::new(ikigai_store::space(DurableStore::in_memory().unwrap())) as Arc<dyn Space>,
        Arc::new(ikigai_ledger::space()) as Arc<dyn Space>,
    ]);
    Kernel::with_meta_renderer(Arc::new(space), Arc::new(ikigai_vocab::TurtleRenderer))
        .with_clock(Arc::new(TickingClock::default()))
}

/// ★ Root lists every ledger, counts and all, with the broad door refusing. On 0.12.0 this
/// was refused, because root's listing was one query through that door. The graveyard a
/// delete creates is in `urn:iki:store:graphs` too, and is still not listed as a ledger.
#[test]
fn root_lists_every_ledger_without_the_broad_store_door() {
    let kernel = kernel_without_the_broad_door();
    append(&kernel, "In the default ledger", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:acme:append",
        &[("content", "Open in acme")],
    );
    sink(
        &kernel,
        "urn:iki:ledger:acme:append",
        &[("content", "Closed in acme")],
    );
    sink(&kernel, "urn:iki:ledger:acme:close", &[("item", "acme#2")]);
    sink(
        &kernel,
        "urn:iki:ledger:bosatsu:append",
        &[("content", "Deleted in bosatsu")],
    );
    ok(&kernel, Verb::Delete, "urn:iki:ledger:bosatsu:item:1", &[]);

    let listing = try_verb(&kernel, Verb::Source, "urn:iki:ledger:ledgers", &[])
        .expect("root lists without the broad door");
    assert!(listing.contains("3 ledger(s)"), "{listing}");
    let row = |name: &str| {
        listing
            .lines()
            .find(|line| line.starts_with(&format!("{name} ")))
            .unwrap_or_else(|| panic!("no row for {name}: {listing}"))
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    assert_eq!(
        row("acme"),
        "acme 1 open 2 total urn:iki:ledger:graph:acme",
        "{listing}"
    );
    assert_eq!(
        row("bosatsu"),
        "bosatsu 0 open 0 total urn:iki:ledger:graph:bosatsu",
        "a ledger whose only item was deleted still exists: {listing}"
    );
    assert!(!listing.contains(":deleted"), "{listing}");

    let turtle = source(&kernel, "urn:iki:ledger:ledgers", &[("as", "text/turtle")]);
    assert!(turtle.contains("urn:iki:ledger:graph:default"), "{turtle}");
}
