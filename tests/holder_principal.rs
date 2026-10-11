//! **The holder is the caller's principal** (ledger #1199, #1143): one worker cannot release
//! another's claim by spelling its name.
//!
//! A holder was free text, so `from=<holder>` on a release guarded a RACE and authorized
//! nothing: worker B, holding the same claim grant as worker A, released A's claim by naming
//! A's holder, or by omitting `from=` altogether. When the capability names a principal
//! (`urn:cap:principal:<iri>`, minted by a host's door — gonk's QUIC door names every client
//! `urn:iki:gonk:client:<fingerprint>`), a claim now records it beside the holder's label as
//! `ledger:claimPrincipal`, and:
//!
//! - releasing a claim recorded with a principal needs a capability that ACTS AS that
//!   principal, with or without `from=`, or the dispatcher's override (`close:{ledger}` or
//!   `write:{ledger}`, which root holds);
//! - claiming or renewing under the same label from another principal is a `Conflict`.
//!
//! A host without principals is unchanged: nothing is recorded, and a holder is free text.

mod common;

use common::*;
use ikigai_core::{Capability, Error, Kernel, Verb};

const LEDGER: &str = "acme";
const A: &str = "urn:iki:gonk:client:aaaa";
const B: &str = "urn:iki:gonk:client:bbbb";
const D: &str = "urn:iki:gonk:passkey:dispatcher";

fn at(resource: &str) -> String {
    format!("urn:iki:ledger:{LEDGER}:{resource}")
}

fn grant(verbs: &[&str]) -> Capability {
    let mut scopes: Vec<String> = verbs
        .iter()
        .map(|verb| format!("urn:cap:ledger:{verb}:{LEDGER}"))
        .collect();
    scopes.push(graph_read(LEDGER));
    scopes.push(graph_write(LEDGER));
    Capability::scoped(scopes)
}

/// The worker grant, as a door mints it for one client.
fn worker(principal: &str) -> Capability {
    grant(&["read", "claim", "comment"])
        .with_principal(principal)
        .unwrap()
}

fn dispatcher() -> Capability {
    grant(&["read", "claim", "comment", "close"])
        .with_principal(D)
        .unwrap()
}

fn ledger_with_an_item() -> Kernel {
    let kernel = kernel();
    sink(&kernel, &at("append"), &[("content", "Fix the widget")]);
    kernel
}

fn claim(kernel: &Kernel, who: &Capability, args: &[(&str, &str)]) -> Result<String, Error> {
    let mut all = vec![("item", "1")];
    all.extend_from_slice(args);
    try_as(kernel, who, Verb::Sink, &at("claim"), &all)
}

fn release(kernel: &Kernel, who: &Capability, args: &[(&str, &str)]) -> Result<String, Error> {
    let mut all = vec![("item", "1")];
    all.extend_from_slice(args);
    try_as(kernel, who, Verb::Delete, &at("claim"), &all)
}

fn claim_json(kernel: &Kernel) -> serde_json::Value {
    let view = source(kernel, &at("item:1"), &[("as", "application/json")]);
    let doc: serde_json::Value = serde_json::from_str(&view).expect("JSON");
    doc["item"]["claim"].clone()
}

fn denied(result: Result<String, Error>, what: &str) {
    match result {
        Err(Error::Denied(_)) => {}
        other => panic!("{what}: expected Denied, got {other:?}"),
    }
}

fn conflict(result: Result<String, Error>, what: &str) {
    match result {
        Err(Error::Conflict(_)) => {}
        other => panic!("{what}: expected Conflict, got {other:?}"),
    }
}

/// ★ THE REPRODUCTION. Before 0.8.0 worker B released worker A's claim by spelling A's
/// holder: `#1 released`, and A's claim was gone.
#[test]
fn a_worker_cannot_release_anothers_claim_by_spelling_its_name() {
    let kernel = ledger_with_an_item();
    claim(&kernel, &worker(A), &[("content", "worker-1")]).unwrap();
    denied(
        release(&kernel, &worker(B), &[("from", "worker-1")]),
        "B releasing A's claim by name",
    );
    assert_eq!(claim_json(&kernel)["holder"], "worker-1");
    assert_eq!(claim_json(&kernel)["principal"], A);
}

/// The override without `from=` is the dispatcher's, not every claim holder's.
#[test]
fn a_worker_cannot_release_anothers_claim_without_from_either() {
    let kernel = ledger_with_an_item();
    claim(&kernel, &worker(A), &[("content", "worker-1")]).unwrap();
    denied(
        release(&kernel, &worker(B), &[]),
        "B releasing A's claim outright",
    );
    assert_eq!(claim_json(&kernel)["principal"], A);
}

#[test]
fn the_holder_releases_its_own_claim_by_label_or_by_principal() {
    let kernel = ledger_with_an_item();
    claim(&kernel, &worker(A), &[("content", "worker-1")]).unwrap();
    release(&kernel, &worker(A), &[("from", "worker-1")]).unwrap();
    assert_eq!(claim_json(&kernel), serde_json::Value::Null);

    claim(&kernel, &worker(A), &[("content", "worker-1")]).unwrap();
    release(&kernel, &worker(A), &[("from", A)]).unwrap();
    assert_eq!(claim_json(&kernel), serde_json::Value::Null);

    claim(&kernel, &worker(A), &[("content", "worker-1")]).unwrap();
    release(&kernel, &worker(A), &[]).unwrap();
    assert_eq!(claim_json(&kernel), serde_json::Value::Null);
}

/// A `from=` naming neither the label nor the principal is still the compare-and-set it was.
#[test]
fn a_from_naming_someone_else_is_a_conflict_even_for_the_holder() {
    let kernel = ledger_with_an_item();
    claim(&kernel, &worker(A), &[("content", "worker-1")]).unwrap();
    conflict(
        release(&kernel, &worker(A), &[("from", "worker-2")]),
        "the holder naming another holder",
    );
    assert_eq!(claim_json(&kernel)["holder"], "worker-1");
}

/// ★ The dispatcher's override is kept: the close grant releases anyone's claim.
#[test]
fn the_dispatcher_and_root_release_anyones_claim() {
    let kernel = ledger_with_an_item();
    claim(&kernel, &worker(A), &[("content", "worker-1")]).unwrap();
    release(&kernel, &dispatcher(), &[]).unwrap();
    assert_eq!(claim_json(&kernel), serde_json::Value::Null);

    claim(&kernel, &worker(A), &[("content", "worker-1")]).unwrap();
    release(&kernel, &dispatcher(), &[("from", "worker-1")]).unwrap();
    assert_eq!(claim_json(&kernel), serde_json::Value::Null);

    claim(&kernel, &worker(A), &[("content", "worker-1")]).unwrap();
    delete(&kernel, &at("claim"), &[("item", "1")]);
    assert_eq!(claim_json(&kernel), serde_json::Value::Null);
}

/// Spelling the label does not renew or re-take another principal's claim either.
#[test]
fn another_principal_under_the_same_label_cannot_claim_or_renew() {
    let kernel = ledger_with_an_item();
    claim(&kernel, &worker(A), &[("content", "worker-1")]).unwrap();
    conflict(
        claim(&kernel, &worker(B), &[("content", "worker-1")]),
        "B claiming under A's label",
    );
    conflict(
        claim(
            &kernel,
            &worker(B),
            &[("content", "worker-1"), ("renew", "true")],
        ),
        "B renewing under A's label",
    );
    assert_eq!(claim_json(&kernel)["principal"], A);
    // A renews its own.
    claim(
        &kernel,
        &worker(A),
        &[("content", "worker-1"), ("renew", "true")],
    )
    .unwrap();
    assert_eq!(claim_json(&kernel)["principal"], A);
}

#[test]
fn the_principal_is_on_every_face() {
    let kernel = ledger_with_an_item();
    claim(&kernel, &worker(A), &[("content", "worker-1")]).unwrap();
    assert_eq!(claim_json(&kernel)["principal"], A);
    let turtle = source(&kernel, &at("item:1"), &[("as", "text/turtle")]);
    assert!(
        turtle.contains(&format!("ledger:claimPrincipal <{A}>")),
        "{turtle}"
    );
    let plain = source(&kernel, &at("item:1"), &[]);
    assert!(plain.contains(&format!("  principal: {A}\n")), "{plain}");
    // The claim answer keeps its shape: gonk's starter parses `claimed by <holder>`.
    let answer = claim(
        &kernel,
        &worker(A),
        &[("content", "worker-1"), ("renew", "true")],
    )
    .unwrap();
    assert_eq!(answer, "acme#1 renewed by worker-1\n");
}

/// A release takes the principal with every other claim field.
#[test]
fn a_release_and_a_close_take_the_principal_with_them() {
    let kernel = ledger_with_an_item();
    claim(&kernel, &worker(A), &[("content", "worker-1")]).unwrap();
    release(&kernel, &worker(A), &[]).unwrap();
    claim(&kernel, &grant(&["read", "claim"]), &[("content", "plain")]).unwrap();
    assert_eq!(claim_json(&kernel)["principal"], serde_json::Value::Null);
    release(&kernel, &grant(&["read", "claim"]), &[]).unwrap();

    claim(&kernel, &worker(A), &[("content", "worker-1")]).unwrap();
    sink(&kernel, &at("close"), &[("item", "1")]);
    let rows = select(
        &kernel,
        "SELECT ?p WHERE { GRAPH ?g { ?i <https://ikigai-rs.dev/ns/ledger#claimPrincipal> ?p } }",
    );
    assert!(!rows.contains(A), "a close left the principal: {rows}");
}

/// An expired lease is still taken over by another worker, and the claim becomes theirs.
#[test]
fn a_takeover_records_the_takers_principal() {
    let kernel = ledger_with_an_item();
    claim(
        &kernel,
        &worker(A),
        &[("content", "worker-1"), ("lease", "1s")],
    )
    .unwrap();
    // The fixture clock advances a second per reading, so the lease has run out by now.
    source(&kernel, &at("items"), &[]);
    claim(
        &kernel,
        &worker(B),
        &[
            ("content", "worker-2"),
            ("takeover", "true"),
            ("from", "worker-1"),
        ],
    )
    .unwrap();
    assert_eq!(claim_json(&kernel)["holder"], "worker-2");
    assert_eq!(claim_json(&kernel)["principal"], B);
    denied(
        release(&kernel, &worker(A), &[("from", "worker-2")]),
        "the old holder releasing the taker's claim",
    );
}

/// A claim recorded with no principal (a host without them, an older claim, root) is
/// adopted by a principal renewing it under its label, as any caller could before.
#[test]
fn a_principal_adopts_a_claim_recorded_without_one() {
    let kernel = ledger_with_an_item();
    sink(
        &kernel,
        &at("claim"),
        &[("item", "1"), ("content", "worker-1")],
    );
    assert_eq!(claim_json(&kernel)["principal"], serde_json::Value::Null);
    claim(
        &kernel,
        &worker(A),
        &[("content", "worker-1"), ("renew", "true")],
    )
    .unwrap();
    assert_eq!(claim_json(&kernel)["principal"], A);
}

/// ★ A host without principals behaves exactly as 0.7.0 did: a holder is free text, and any
/// claim-grant holder releases any claim.
#[test]
fn a_host_without_principals_is_unchanged() {
    let kernel = ledger_with_an_item();
    let w1 = grant(&["read", "claim", "comment"]);
    let w2 = grant(&["read", "claim", "comment"]);
    claim(&kernel, &w1, &[("content", "worker-1")]).unwrap();
    assert_eq!(claim_json(&kernel)["principal"], serde_json::Value::Null);
    release(&kernel, &w2, &[("from", "worker-1")]).unwrap();
    assert_eq!(claim_json(&kernel), serde_json::Value::Null);
    claim(&kernel, &w1, &[("content", "worker-1")]).unwrap();
    claim(&kernel, &w2, &[("content", "worker-1"), ("renew", "true")]).unwrap();
    release(&kernel, &w2, &[]).unwrap();
    assert_eq!(claim_json(&kernel), serde_json::Value::Null);
}
