//! The module recipe over the alias resources (ledger #2828): `ikigai-conformance` walks
//! `ikigai_ledger::alias::space()`, bound beside the store and the ledgers as a host binds it.
//!
//! Every probe that writes runs as a principal that owns the scheme, because every alias write
//! is made as one; `tests/aliases.rs` holds the refusals.

mod common;

use std::sync::Arc;

use common::*;
use ikigai_conformance::{Fixture, Suite};
use ikigai_core::{Capability, Fallback, Kernel, Space, Verb};
use ikigai_store::DurableStore;

const OWNER: &str = "urn:iki:gonk:client:kata-import";
const ALIASES: &str = "urn:iki:alias:graph";

fn owner() -> Capability {
    Capability::scoped(vec![
        "urn:cap:alias:scheme:kata".to_string(),
        "urn:cap:ledger:read:default".to_string(),
        "urn:cap:ledger:alias:default".to_string(),
        graph_read("default"),
        graph_write("default"),
        format!("urn:cap:store:read:graph:{ALIASES}"),
        format!("urn:cap:store:write:graph:{ALIASES}"),
    ])
    .with_principal(OWNER)
    .unwrap()
}

fn seeded() -> (Kernel, String) {
    let space = Fallback::new(vec![
        Arc::new(ikigai_store::space(DurableStore::in_memory().unwrap())) as Arc<dyn Space>,
        Arc::new(ikigai_ledger::space()) as Arc<dyn Space>,
        Arc::new(ikigai_ledger::alias::space()) as Arc<dyn Space>,
    ]);
    let kernel =
        Kernel::with_meta_renderer(Arc::new(space), Arc::new(ikigai_vocab::TurtleRenderer))
            .with_clock(Arc::new(TickingClock::default()));
    let item = append_iri(&kernel, "The aliased item", &[]);
    try_as(
        &kernel,
        &owner(),
        Verb::Sink,
        "urn:iki:alias:scheme:kata",
        &[],
    )
    .unwrap();
    try_as(
        &kernel,
        &owner(),
        Verb::Sink,
        "urn:iki:alias:scheme:kata:value:01m6c5",
        &[("content", &item)],
    )
    .unwrap();
    (kernel, item)
}

fn suite(item: &str) -> Suite {
    Suite::new()
        .own("alias")
        .own("alias-scheme")
        .required_on_mutating()
        .fixture(
            Fixture::new("alias-scheme", Verb::Source)
                .binding("scheme", "kata")
                .capability(owner()),
        )
        .fixture(
            Fixture::new("alias", Verb::Source)
                .binding("scheme", "kata")
                .binding("value", "01m6c5")
                .capability(owner()),
        )
        .fixture(
            Fixture::new("alias", Verb::Sink)
                .binding("scheme", "kata")
                .binding("value", "01m6c5")
                .arg("content", item)
                .capability(owner()),
        )
        .self_named_space("ledger-alias", ikigai_ledger::alias::space)
}

#[test]
fn the_alias_resources_conform() {
    let (kernel, item) = seeded();
    let report = suite(&item).run_blocking(&kernel);
    println!("{report}");
    assert!(report.is_clean(), "{report}");
    let mut walked: Vec<&str> = report.walked.iter().map(String::as_str).collect();
    walked.sort_unstable();
    assert_eq!(walked, vec!["alias", "alias-scheme"], "{report}");
    assert_eq!(
        ikigai_core::space_iri("ledger-alias").as_str(),
        ikigai_ledger::alias::SPACE_ID
    );
}
