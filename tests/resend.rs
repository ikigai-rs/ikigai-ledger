//! Resend declarations must describe observed writes, including unsafe counterexamples.
mod common;

use common::*;
use ikigai_core::{Error, Iri, Request, Resend, Resolution, Scope, Space, Verb};

#[test]
fn only_writes_with_replay_identity_declare_safe_resends() {
    let space = ikigai_ledger::space();
    let mut keyed = 0;
    for entry in space.entries().unwrap() {
        let iri = entry
            .pattern
            .replace("{ledger}", "default")
            .replace("{id}", "1")
            .replace("{name}", "kata-flight")
            .replace("{value}", "filed")
            .replace("{holder}", "worker");
        let Resolution::Hit(resolved) = space.resolve(
            &Request::new(Verb::Meta, Iri::parse(iri).unwrap()),
            &Scope::empty(),
        ) else {
            panic!("unresolved {}", entry.pattern)
        };
        for action in resolved.endpoint.describe().action_specs() {
            if action.verb == Verb::Sink && action.inputs.iter().any(|input| input.name == "key") {
                keyed += 1;
                assert_eq!(
                    action.resend,
                    Some(Resend::Key("key".into())),
                    "{}",
                    entry.pattern
                );
                assert!(!action.resend_is_safe(|_| false));
                assert!(action.resend_is_safe(|name| name == "key"));
            } else if action.verb == Verb::Sink && resolved.endpoint.name() == "ledger-history" {
                assert_eq!(action.resend, Some(Resend::Safe));
                assert!(action.resend_is_safe(|_| false));
            } else {
                assert_eq!(action.resend, None, "{} {:?}", entry.pattern, action.verb);
            }
        }
    }
    assert!(keyed >= 2);
}

fn graph(kernel: &ikigai_core::Kernel) -> String {
    select(kernel, "SELECT ?s ?p ?o WHERE { GRAPH <urn:iki:ledger:graph:default> { ?s ?p ?o } } ORDER BY ?s ?p ?o")
}

#[test]
fn keyed_writes_answer_existing_and_leave_every_triple_unchanged() {
    for endpoint in ["append", "comment"] {
        for face in ["text/plain", "application/json"] {
            let kernel = kernel();
            append(&kernel, "The target", &[]);
            let iri = format!("urn:iki:ledger:{endpoint}");
            let args = [
                ("item", "1"),
                ("content", "A keyed write"),
                ("key", "retry:1"),
                ("as", face),
            ];
            let first = sink(&kernel, &iri, &args);
            let before = graph(&kernel);
            let replay = sink(&kernel, &iri, &args);
            assert_eq!(graph(&kernel), before, "{endpoint} {face}");
            if face == "application/json" {
                let first: serde_json::Value = serde_json::from_str(&first).unwrap();
                let replay: serde_json::Value = serde_json::from_str(&replay).unwrap();
                assert_eq!(replay["outcome"], "existing");
                assert_eq!(replay["item"], first["item"]);
                assert_eq!(replay["comment"], first["comment"]);
            } else {
                assert!(replay.contains("existing"), "{replay}");
            }
        }
    }
}

#[test]
fn a_same_holder_claim_replay_changes_the_lease_but_not_the_epoch_or_history() {
    let kernel = kernel();
    append(&kernel, "Work", &[]);
    let args = [("item", "1"), ("content", "worker"), ("lease", "10m")];
    sink(&kernel, "urn:iki:ledger:claim", &args);
    let before: serde_json::Value = serde_json::from_str(&source(
        &kernel,
        "urn:iki:ledger:item:1",
        &[("as", "application/json")],
    ))
    .unwrap();
    let history = source(&kernel, "urn:iki:ledger:item:1:history", &[]);
    sink(&kernel, "urn:iki:ledger:claim", &args);
    let after: serde_json::Value = serde_json::from_str(&source(
        &kernel,
        "urn:iki:ledger:item:1",
        &[("as", "application/json")],
    ))
    .unwrap();
    assert_ne!(
        before["item"]["claim"]["expires"],
        after["item"]["claim"]["expires"]
    );
    assert_eq!(before["item"]["claim"]["epoch"], 1);
    assert_eq!(
        before["item"]["claim"]["lease"],
        after["item"]["claim"]["lease"]
    );
    assert_eq!(
        before["item"]["claim"]["epoch"],
        after["item"]["claim"]["epoch"]
    );
    assert_eq!(
        history,
        source(&kernel, "urn:iki:ledger:item:1:history", &[])
    );
}

#[test]
fn a_move_replay_conflicts_and_from_does_not_identify_a_request_after_a_cycle() {
    let kernel = kernel();
    append(&kernel, "Work", &[]);
    let iri = "urn:iki:ledger:item:1:state";
    let args = [("from", "filed"), ("to", "queued")];
    sink(&kernel, iri, &args);
    let before = graph(&kernel);
    assert!(matches!(
        try_verb(&kernel, Verb::Sink, iri, &args),
        Err(Error::Conflict(_))
    ));
    assert_eq!(before, graph(&kernel));
    sink(&kernel, iri, &[("from", "queued"), ("to", "filed")]);
    let before = graph(&kernel);
    sink(&kernel, iri, &args);
    assert_ne!(
        before,
        graph(&kernel),
        "from is a precondition, not a replay key"
    );
}

#[test]
fn turtle_meta_exposes_resend_safety_to_clients() {
    let kernel = kernel();
    for endpoint in ["append", "comment"] {
        let meta = ok(
            &kernel,
            Verb::Meta,
            &format!("urn:iki:ledger:{endpoint}"),
            &[],
        );
        assert!(meta.contains("ik:resendKey <"), "{meta}");
        assert!(meta.contains(":input:key>"), "{meta}");
        assert!(!meta.contains("ik:resendSafe true"));
    }
    let meta = ok(&kernel, Verb::Meta, "urn:iki:ledger:history", &[]);
    assert!(meta.contains("ik:resendSafe true"), "{meta}");
    for endpoint in ["claim", "item:1:state"] {
        let meta = ok(
            &kernel,
            Verb::Meta,
            &format!("urn:iki:ledger:{endpoint}"),
            &[],
        );
        assert!(!meta.contains("ik:resendKey <"));
        assert!(!meta.contains("ik:resendSafe true"));
    }
}
