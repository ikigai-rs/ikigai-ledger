//! **Every action this crate binds, under exactly the grant list `README.md` publishes.**
//!
//! # Why this file exists, which is a defect and not a plan
//!
//! 0.2.0 narrowed every store scope this crate declares from the broad dataset tokens to
//! the per-graph family, and the suite proved it — for `append`, for `items`, for
//! `ledgers`. It did not prove it for `item:{id}`, because **every `item:{id}` read in the
//! suite ran under root**, and root holds every token by definition. So `item:{id}`'s
//! `Source` and `Exists` went on declaring `urn:cap:store:read`, the whole dataset, and
//! nothing noticed: a caller holding exactly [`published_grants`] could list a ledger and
//! file into it and was **denied reading a single item out of it**, with the only
//! workaround being the grant 0.2.0 exists to stop needing.
//!
//! The one-token fix is in `endpoints.rs`. This file is the part that keeps it fixed.
//!
//! # The shape, which is the actual deliverable
//!
//! A grant list that is correct for *most* actions is not a boundary, so a hand-kept list
//! of positive tests is not the fix either — it has the same hole, one action further on.
//! [`the_published_grant_list_is_sufficient_for_every_action_this_crate_binds`] instead:
//!
//! 1. reads **every** `(endpoint, verb)` pair off `ikigai_ledger::space()` itself, by
//!    enumerating its bindings and asking each endpoint to `describe()` — so a new
//!    resource, or a new verb on an old one, joins the set the moment it is bound;
//! 2. refuses to pass if any of them has no scripted invocation below, and refuses to
//!    pass if the script names an action that is not bound (a row left behind by a
//!    rename);
//! 3. runs the whole script against a live ledger under [`published_grants`] **and
//!    nothing more**, asserting every step succeeds.
//!
//! Step 3 is what step 1 exists to make exhaustive. It is a *positive* check on purpose:
//! this suite was already thick with refusals, and the hole was on the other side — the
//! grants a caller is told to hold turning out not to be enough.
//!
//! ⚠ **What it does not prove.** That the grants are *necessary*, action by action — an
//! action declaring a scope it never needs would pass here. The negative direction is
//! `tests/ledgers.rs` (a grant for one ledger refused at another, eight doors) and
//! `tests/endpoints.rs` (read cannot write, write cannot delete). The two halves are only
//! useful together: this one says the published list is enough, those say it is not more
//! than enough.

mod common;

use std::collections::BTreeSet;

use ikigai_core::{Capability, Iri, Request, Resolution, Scope, Space, Verb};

use common::*;

/// The ledger the whole script runs in. A *named* one, never `default`, so a scope that
/// silently ignored the ledger segment would still have to name this graph.
const LEDGER: &str = "acme";

/// The grant list `README.md`'s table publishes for one ledger — **seven tokens, every one
/// of them naming that ledger, and nothing else at all.**
///
/// ★ Read it as the operator's config line, because that is what it is, and keep it
/// literal: this is the same list as `tests/ledgers.rs::grants_for`, deliberately written
/// out twice rather than shared, because the two files assert opposite things about it and
/// a shared helper would let one of them quietly widen it for the other.
fn published_grants(ledger: &str) -> Capability {
    Capability::scoped([
        format!("urn:cap:ledger:read:{ledger}"),
        format!("urn:cap:ledger:write:{ledger}"),
        format!("urn:cap:ledger:delete:{ledger}"),
        format!("urn:cap:ledger:purge:{ledger}"),
        graph_read(ledger),
        graph_write(ledger),
        graveyard_write(ledger),
    ])
}

/// One `(endpoint, verb)` pair, spelled so a failure names the thing that is missing.
fn action(endpoint: &str, verb: Verb) -> String {
    format!("{endpoint} {verb:?}")
}

/// A bound grammar's pattern, expanded into one IRI that really resolves — the template
/// variables this crate's grammars capture and nothing else. An unexpanded `{` left in the
/// result means a new kind of binding arrived and this expansion has not been taught about
/// it, which is a panic rather than a miss.
fn concrete(pattern: &str) -> String {
    let iri = pattern
        .replace("{ledger}", LEDGER)
        .replace("{id}", "1")
        .replace("{name}", "leverage");
    assert!(
        !iri.contains('{'),
        "`{pattern}` has a template variable this test cannot expand; teach `concrete` \
         about it rather than dropping the binding from the sweep"
    );
    iri
}

/// **Every action this crate binds, read off the space rather than written down here.**
///
/// `EndpointSpace` enumerates its bindings, each binding's grammar can spell its own
/// pattern, and resolving that pattern hands back the endpoint — so `describe()` is
/// reachable without this file knowing a single resource name. `Description::action_specs`
/// drops `Meta`, which is right: the kernel never gates it, so there is no grant to check.
fn bound_actions() -> BTreeSet<String> {
    let space = ikigai_ledger::space();
    let scope = Scope::empty();
    let mut actions = BTreeSet::new();
    for entry in space
        .entries()
        .expect("an EndpointSpace enumerates its bindings")
    {
        let iri = Iri::parse(concrete(&entry.pattern)).expect("an expanded pattern is an IRI");
        let request = Request::new(Verb::Meta, iri);
        let Resolution::Hit(resolved) = space.resolve(&request, &scope) else {
            panic!(
                "`{}` does not resolve its own pattern `{}`",
                entry.endpoint, entry.pattern
            );
        };
        for spec in resolved.endpoint.describe().action_specs() {
            actions.insert(action(&entry.endpoint, spec.verb));
        }
    }
    assert!(!actions.is_empty(), "the ledger space binds nothing");
    actions
}

/// One scripted invocation, in the order it has to run.
struct Step {
    /// The endpoint it exercises — matched against [`bound_actions`], so a rename here
    /// without a rename there fails.
    endpoint: &'static str,
    verb: Verb,
    iri: String,
    args: Vec<(&'static str, &'static str)>,
}

fn step(
    endpoint: &'static str,
    verb: Verb,
    iri: String,
    args: &[(&'static str, &'static str)],
) -> Step {
    Step {
        endpoint,
        verb,
        iri,
        args: args.to_vec(),
    }
}

/// The script: real work on a live ledger, in an order where each step has something to
/// act on.
///
/// ★ Every step is a *use* and not a probe — an untag removes a tag that was really added,
/// an unlink removes an edge that was really drawn, a purge destroys an item that was
/// really filed. An invocation that failed its own arguments would pass a capability check
/// and fail here, which is the point of running it rather than inspecting the contract.
fn script() -> Vec<Step> {
    let at = |resource: &str| format!("urn:iki:ledger:{LEDGER}:{resource}");
    vec![
        // Two items, so `link` has both ends.
        step(
            "ledger-append",
            Verb::Sink,
            at("append"),
            &[("content", "The first item\n\nWith a body.")],
        ),
        step(
            "ledger-append",
            Verb::Sink,
            at("append"),
            &[("content", "The second item")],
        ),
        step("ledger-items", Verb::Source, at("items"), &[]),
        step("ledger-item", Verb::Source, at("item:1"), &[]),
        step("ledger-item", Verb::Exists, at("item:1"), &[]),
        step(
            "ledger-item",
            Verb::Sink,
            at("item:1"),
            &[("content", "The first item, retitled"), ("priority", "1")],
        ),
        step(
            "ledger-comment",
            Verb::Sink,
            at("comment"),
            &[("item", "#1"), ("content", "A note on it.")],
        ),
        step(
            "ledger-label",
            Verb::Sink,
            at("label"),
            &[("item", "#1"), ("content", "rust")],
        ),
        step(
            "ledger-label",
            Verb::Delete,
            at("label"),
            &[("item", "#1"), ("content", "rust")],
        ),
        step(
            "ledger-link",
            Verb::Sink,
            at("link"),
            &[("item", "#1"), ("content", "#2"), ("type", "blocks")],
        ),
        step(
            "ledger-link",
            Verb::Delete,
            at("link"),
            &[("item", "#1"), ("content", "#2"), ("type", "blocks")],
        ),
        step(
            "ledger-claim",
            Verb::Sink,
            at("claim"),
            &[("item", "#1"), ("content", "session-a")],
        ),
        step("ledger-claim", Verb::Delete, at("claim"), &[("item", "#1")]),
        step(
            "ledger-defer",
            Verb::Sink,
            at("defer"),
            &[("item", "#1"), ("content", "not this quarter")],
        ),
        step("ledger-defer", Verb::Delete, at("defer"), &[("item", "#1")]),
        step(
            "ledger-close",
            Verb::Sink,
            at("close"),
            &[
                ("item", "#1"),
                ("reason", "done"),
                ("content", "Finished it."),
            ],
        ),
        step("ledger-reopen", Verb::Sink, at("reopen"), &[("item", "#1")]),
        step("ledger-next", Verb::Source, at("next"), &[]),
        // The two ledger-less resources. `ledgers` is the one that must not need a broad
        // grant either: it answers what this capability may read, from what it holds.
        step(
            "ledger-ledgers",
            Verb::Source,
            "urn:iki:ledger:ledgers".to_string(),
            &[],
        ),
        step(
            "ledger-policy",
            Verb::Source,
            "urn:iki:ledger:policy:leverage".to_string(),
            &[],
        ),
        step(
            "ledger-policy",
            Verb::Exists,
            "urn:iki:ledger:policy:leverage".to_string(),
            &[],
        ),
        // Last, because they destroy what the rest acted on. Both need the graveyard's
        // write grant as well as the graph's — the row of the README's table an operator
        // is likeliest to get wrong.
        step(
            "ledger-item",
            Verb::Delete,
            at("item:2"),
            &[("reason", "filed by mistake")],
        ),
        step(
            "ledger-append",
            Verb::Sink,
            at("append"),
            &[("content", "Filed in the wrong ledger")],
        ),
        step(
            "ledger-purge",
            Verb::Delete,
            at("purge"),
            &[("content", "#3"), ("reason", "wrong ledger")],
        ),
    ]
}

/// ★ **The test this arc exists for.** Every action this crate binds, exercised for real
/// under the seven tokens `README.md` tells an operator to issue — and under nothing else.
///
/// Before the fix this failed on `ledger-item Source`, with the store's denial naming
/// `urn:cap:store:read`: the one action whose contract asked for the whole dataset.
#[test]
fn the_published_grant_list_is_sufficient_for_every_action_this_crate_binds() {
    let bound = bound_actions();
    let script = script();

    // The coverage check runs FIRST and on its own, so a missing row is reported as a gap
    // in this file rather than as whatever the script happens to do without it.
    let scripted: BTreeSet<String> = script
        .iter()
        .map(|step| action(step.endpoint, step.verb))
        .collect();
    let unexercised: Vec<&String> = bound.difference(&scripted).collect();
    assert!(
        unexercised.is_empty(),
        "these actions are bound and never exercised under the published grant list — \
         add a step to `script()`, because an action tested only under root is an action \
         whose declared scopes nobody has checked: {unexercised:?}"
    );
    let stale: Vec<&String> = scripted.difference(&bound).collect();
    assert!(
        stale.is_empty(),
        "`script()` names actions this crate does not bind (a rename, or a verb that went \
         away): {stale:?}"
    );

    let kernel = kernel();
    let caller = published_grants(LEDGER);
    for (n, step) in script.iter().enumerate() {
        let answered = try_as(&kernel, &caller, step.verb, &step.iri, &step.args);
        assert!(
            answered.is_ok(),
            "step {n} ({} {}) is refused the published grant list: {answered:?}",
            action(step.endpoint, step.verb),
            step.iri,
        );
    }
}

/// ⚠ **The declaration itself, checked against the list — not only the run.**
///
/// The test above proves the seven tokens *work*; this one proves the contract *says* so,
/// which is the half a caller reads before it calls. An action declaring a scope outside
/// the published list is an over-declaration: the kernel's pre-check refuses it, selection
/// drops it from the manifold, and `urn:kernel:validate` reports it — all before `invoke`
/// runs, so no amount of correct enforcement inside makes up for it.
///
/// It is also the cheaper signal. The run needs a live store, an ordering and a fixture;
/// this needs the descriptions and a set.
#[test]
fn no_action_declares_a_scope_outside_the_published_grant_list() {
    let held = published_grants(LEDGER);
    let space = ikigai_ledger::space();
    let scope = Scope::empty();
    let mut over: Vec<String> = Vec::new();

    for entry in space
        .entries()
        .expect("an EndpointSpace enumerates its bindings")
    {
        let iri = Iri::parse(concrete(&entry.pattern)).expect("an expanded pattern is an IRI");
        let Resolution::Hit(resolved) = space.resolve(&Request::new(Verb::Meta, iri), &scope)
        else {
            panic!("`{}` does not resolve its own pattern", entry.endpoint);
        };
        for spec in resolved.endpoint.describe().action_specs() {
            for declared in &spec.requires {
                if !satisfies(&held, declared) {
                    over.push(format!(
                        "{} declares `{declared}`",
                        action(&entry.endpoint, spec.verb)
                    ));
                }
            }
        }
    }

    assert!(
        over.is_empty(),
        "the published grant list does not satisfy every declared scope — either the \
         declaration is too broad or `README.md`'s table is out of date, and one of the \
         two has to move: {over:?}"
    );
}

/// The kernel's own offer/enforce predicate, restated.
///
/// ⚠ **Restated because `ikigai_core::select::cap_satisfies` is `pub(crate)`.** The crate
/// that owns the rule will not lend it, so a module wanting to assert "this grant list
/// satisfies what I declare" has to keep a copy — and a copy of a predicate can drift in a
/// way a copy of a literal cannot. Reported upward; if core ever exports it, delete this
/// and call it.
///
/// A trailing `*` is the family form: `urn:cap:store:read:graph:*` is satisfied by holding
/// *any* grant under that prefix. Anything else is exact set membership, which is what
/// made the broad `urn:cap:store:read` unsatisfiable by a caller holding only per-graph
/// tokens — it has no `*`, so nothing but the literal token would do.
fn satisfies(capability: &Capability, declared: &str) -> bool {
    match declared.strip_suffix('*') {
        Some(prefix) => match capability.scopes() {
            None => true,
            Some(held) => held.iter().any(|s| s.starts_with(prefix)),
        },
        None => capability.allows(declared),
    }
}
