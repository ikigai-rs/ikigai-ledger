//! **`next drain=<name>` and `next state=<state>`** (ledger #2865) narrow `next` to the open items
//! whose current state drains to that step, or is that state. They apply the way `labels=` does:
//! after blocked items, cycles and leverage are computed over the whole ledger, so a filter only
//! chooses which items are being asked about, and never changes what ranks them.

mod common;

use std::sync::Arc;

use common::*;
use futures::executor::block_on;
use ikigai_core::{
    ArgRef, ArgSpec, Capability, Error, Fallback, Iri, Kernel, Request, Resolution, Scope, Space,
    Verb,
};
use ikigai_ledger::{Lifecycle, Lifecycles, SpaceConfig};
use ikigai_store::DurableStore;
use serde_json::Value;

const NEXT: &str = "urn:iki:ledger:next";
const LIVE: &str = "urn:iki:ledger:graph:default";
const STATE: &str = "https://ikigai-rs.dev/ns/ledger#state";
const KATA: &str = "urn:iki:ledger:lifecycle:kata-flight";

/// Two states drain to `gate`, so a drain is not a state by another name. `doing` is in flight and
/// drains nowhere.
const SHARED: &str = r#"
    @prefix ledger: <https://ikigai-rs.dev/ns/ledger#> .
    @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
    <urn:iki:ledger:lifecycle:shared> a ledger:Lifecycle ;
        ledger:lifecycleVersion "1" ;
        ledger:hasState <urn:iki:ledger:lifecycle:shared:filed>,
                        <urn:iki:ledger:lifecycle:shared:first>,
                        <urn:iki:ledger:lifecycle:shared:second>,
                        <urn:iki:ledger:lifecycle:shared:doing> .
    <urn:iki:ledger:lifecycle:shared:filed> rdfs:label "filed" ; ledger:order 0 ;
        ledger:inFlight false ; ledger:drain "gate" .
    <urn:iki:ledger:lifecycle:shared:first> rdfs:label "first" ; ledger:order 1 ;
        ledger:inFlight false ; ledger:drain "gate" .
    <urn:iki:ledger:lifecycle:shared:second> rdfs:label "second" ; ledger:order 2 ;
        ledger:inFlight false ; ledger:drain "ship" .
    <urn:iki:ledger:lifecycle:shared:doing> rdfs:label "doing" ; ledger:order 3 ;
        ledger:inFlight true .
"#;

/// A kernel whose default ledger uses kata-flight and whose ledger `acme` uses [`SHARED`].
fn two_lifecycles() -> Kernel {
    let lifecycles = Lifecycles::new(vec![
        Lifecycle::kata_flight(),
        Lifecycle::parse(SHARED).unwrap(),
    ])
    .assign("acme", "shared");
    let space = Fallback::new(vec![
        Arc::new(ikigai_store::space(DurableStore::in_memory().unwrap())) as Arc<dyn Space>,
        Arc::new(ikigai_ledger::endpoints::space_with(
            SpaceConfig::default().lifecycles(lifecycles),
        )) as Arc<dyn Space>,
    ]);
    Kernel::with_meta_renderer(Arc::new(space), Arc::new(ikigai_vocab::TurtleRenderer))
        .with_clock(Arc::new(TickingClock::default()))
}

fn move_to(kernel: &Kernel, item: &str, from: &str, to: &str) {
    sink(
        kernel,
        &format!("{item}:state"),
        &[("from", from), ("to", to)],
    );
}

fn blocks(kernel: &Kernel, blocker: &str, blocked: &str) {
    sink(
        kernel,
        "urn:iki:ledger:link",
        &[("item", blocker), ("content", blocked), ("type", "blocks")],
    );
}

/// `next`'s JSON document, every rank of it.
fn next_json(kernel: &Kernel, iri: &str, args: &[(&str, &str)]) -> Value {
    let mut all = vec![("as", "application/json"), ("limit", "0")];
    all.extend_from_slice(args);
    serde_json::from_str(&source(kernel, iri, &all)).unwrap()
}

fn numbers(list: &Value) -> Vec<i64> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["item"]["number"].as_i64().unwrap())
        .collect()
}

fn ranked(kernel: &Kernel, args: &[(&str, &str)]) -> Vec<i64> {
    numbers(&next_json(kernel, NEXT, args)["ranking"])
}

fn excluded(kernel: &Kernel, args: &[(&str, &str)]) -> Vec<i64> {
    let mut found = numbers(&next_json(kernel, NEXT, args)["excluded"]);
    found.sort_unstable();
    found
}

fn invalid(result: Result<String, Error>) -> (String, String) {
    match result {
        Err(Error::InvalidArgument { name, detail }) => (name, detail),
        other => panic!("expected InvalidArgument, got {other:?}"),
    }
}

/// #1 stays filed, #2 is queued, #3 is reviewed, #4 is in flight with no claim.
fn one_in_each() -> Kernel {
    let kernel = kernel();
    append(&kernel, "Filed", &[]);
    append(&kernel, "Queued", &[]);
    append(&kernel, "Reviewed", &[]);
    append(&kernel, "In flight", &[]);
    move_to(&kernel, "urn:iki:ledger:item:2", "filed", "queued");
    move_to(&kernel, "urn:iki:ledger:item:3", "filed", "reviewed");
    move_to(&kernel, "urn:iki:ledger:item:4", "filed", "resolving");
    kernel
}

// ------------------------------------------------------------------------ what each admits

/// `filed` is included when its drain matches, though a filed item holds no `ledger:state`.
#[test]
fn a_drain_admits_only_the_items_whose_state_drains_there() {
    let kernel = one_in_each();
    assert_eq!(ranked(&kernel, &[("drain", "triage")]), vec![1]);
    assert_eq!(ranked(&kernel, &[("drain", "review-gate")]), vec![2]);
    assert_eq!(ranked(&kernel, &[("drain", "ship")]), vec![3]);
    let mut all = ranked(&kernel, &[]);
    all.sort_unstable();
    assert_eq!(
        all,
        vec![1, 2, 3, 4],
        "without a filter, nothing is narrowed"
    );
}

#[test]
fn a_state_admits_only_the_items_in_that_state() {
    let kernel = one_in_each();
    assert_eq!(ranked(&kernel, &[("state", "filed")]), vec![1]);
    assert_eq!(ranked(&kernel, &[("state", "queued")]), vec![2]);
    assert_eq!(ranked(&kernel, &[("state", "reviewed")]), vec![3]);
    assert_eq!(ranked(&kernel, &[("state", "resolving")]), vec![4]);
    assert_eq!(ranked(&kernel, &[("state", "shipping")]), Vec::<i64>::new());
}

/// An in-flight state has no drain, so no `drain=` asks about it.
#[test]
fn no_drain_admits_an_item_in_flight() {
    let kernel = one_in_each();
    for drain in ["triage", "review-gate", "ship"] {
        assert!(
            !ranked(&kernel, &[("drain", drain)]).contains(&4),
            "{drain}"
        );
    }
}

#[test]
fn the_plain_face_and_the_graph_face_narrow_too() {
    let kernel = one_in_each();
    let plain = source(&kernel, NEXT, &[("drain", "review-gate")]);
    assert_eq!(
        plain.lines().filter(|l| l.contains("Queued")).count(),
        1,
        "{plain}"
    );
    for other in ["Filed", "Reviewed", "In flight"] {
        assert!(!plain.contains(other), "{other}:\n{plain}");
    }
    assert!(plain.contains("ready: 1   excluded: 0"), "{plain}");

    let turtle = source(
        &kernel,
        NEXT,
        &[("drain", "review-gate"), ("as", "text/turtle")],
    );
    assert!(turtle.contains("Queued"), "{turtle}");
    assert!(!turtle.contains("Reviewed"), "{turtle}");
}

/// Both given, both hold; and they sit beside `labels=` as one more narrowing.
#[test]
fn the_filters_compose() {
    let kernel = kernel();
    append(&kernel, "Queued rust", &[("labels", "rust")]);
    append(&kernel, "Queued docs", &[("labels", "docs")]);
    append(&kernel, "Filed rust", &[("labels", "rust")]);
    move_to(&kernel, "urn:iki:ledger:item:1", "filed", "queued");
    move_to(&kernel, "urn:iki:ledger:item:2", "filed", "queued");

    assert_eq!(
        ranked(&kernel, &[("drain", "review-gate"), ("labels", "rust")]),
        vec![1]
    );
    assert_eq!(
        ranked(&kernel, &[("state", "filed"), ("labels", "rust")]),
        vec![3]
    );
    assert_eq!(
        ranked(&kernel, &[("drain", "review-gate"), ("state", "queued")]),
        vec![2, 1]
    );
    // A pair no state satisfies is an empty answer, in the sentence an empty answer always is.
    let none = source(&kernel, NEXT, &[("drain", "triage"), ("state", "queued")]);
    assert_eq!(
        none,
        "nothing is ready (0 open item(s) excluded: the ledger has no open items matching the \
         filter)\npolicy: priority-recency\n"
    );
}

/// A closed item keeps the state it held, and `next` is about open work.
#[test]
fn a_closed_item_in_the_state_is_not_admitted() {
    let kernel = kernel();
    append(&kernel, "Closes while queued", &[]);
    append(&kernel, "Stays queued", &[]);
    move_to(&kernel, "urn:iki:ledger:item:1", "filed", "queued");
    move_to(&kernel, "urn:iki:ledger:item:2", "filed", "queued");
    sink(&kernel, "urn:iki:ledger:close", &[("item", "#1")]);

    assert_eq!(ranked(&kernel, &[("state", "queued")]), vec![2]);
    assert_eq!(excluded(&kernel, &[("state", "queued")]), Vec::<i64>::new());
}

/// A drain is shared by the states that name it, and `filed` is one of them here.
#[test]
fn a_drain_several_states_share_admits_all_of_them() {
    let kernel = two_lifecycles();
    const ACME: &str = "urn:iki:ledger:acme:next";
    let append = |title: &str| {
        sink(&kernel, "urn:iki:ledger:acme:append", &[("content", title)]);
    };
    append("Filed");
    append("First");
    append("Second");
    move_to(&kernel, "urn:iki:ledger:acme:item:2", "filed", "first");
    move_to(&kernel, "urn:iki:ledger:acme:item:3", "filed", "second");

    let gate = next_json(&kernel, ACME, &[("drain", "gate")]);
    assert_eq!(numbers(&gate["ranking"]), vec![2, 1]);
    let ship = next_json(&kernel, ACME, &[("drain", "ship")]);
    assert_eq!(numbers(&ship["ranking"]), vec![3]);
}

/// ★ Each ledger is judged by its own lifecycle, though `describe` is one contract for all of
/// them: `triage` is a drain of the default ledger's and not of `acme`'s.
#[test]
fn a_filter_is_judged_by_the_lifecycle_of_the_ledger_it_asks() {
    let kernel = two_lifecycles();
    assert!(try_verb(&kernel, Verb::Source, NEXT, &[("drain", "triage")]).is_ok());
    let (name, detail) = invalid(try_verb(
        &kernel,
        Verb::Source,
        "urn:iki:ledger:acme:next",
        &[("drain", "triage")],
    ));
    assert_eq!(name, "drain");
    assert_eq!(
        detail,
        "`triage` is not a drain of the lifecycle `shared` this ledger uses; one of gate, ship \
         (see `urn:iki:ledger:lifecycle:shared`)"
    );
    let (name, detail) = invalid(try_verb(
        &kernel,
        Verb::Source,
        NEXT,
        &[("state", "second")],
    ));
    assert_eq!(name, "state");
    assert!(detail.contains("`kata-flight`"), "{detail}");
}

// ---------------------------------------------------------- the ranking's inputs do not move

/// ★ The test the item asks for. A filtered `next` is the unfiltered one with the other items
/// taken out: the same relative order, the same scores and reasons, and the same exclusions with
/// the same reasons, whichever drain or state is asked about.
#[test]
fn a_filtered_next_keeps_the_unfiltered_ranking_and_blocked_exclusions() {
    let kernel = kernel();
    append(&kernel, "Queued, unblocks three", &[("priority", "3")]); // #1
    append(&kernel, "Queued, blocked by #1", &[("priority", "0")]); // #2
    append(&kernel, "Filed, blocked through #2", &[("priority", "0")]); // #3
    append(
        &kernel,
        "Reviewed, blocked through #3",
        &[("priority", "0")],
    ); // #4
    append(&kernel, "Queued, important", &[("priority", "1")]); // #5
    append(&kernel, "Queued, no leverage", &[("priority", "3")]); // #6
    append(&kernel, "Filed, ready", &[("priority", "2")]); // #7
    append(&kernel, "Queued, blocked by filed #7", &[("priority", "0")]); // #8
    for item in [1, 2, 5, 6, 8] {
        move_to(
            &kernel,
            &format!("urn:iki:ledger:item:{item}"),
            "filed",
            "queued",
        );
    }
    move_to(&kernel, "urn:iki:ledger:item:4", "filed", "reviewed");
    blocks(&kernel, "#1", "#2");
    blocks(&kernel, "#2", "#3");
    blocks(&kernel, "#3", "#4");
    blocks(&kernel, "#7", "#8");

    let in_queued = [1, 2, 5, 6, 8];
    let ranking = |doc: &Value| -> Vec<(i64, Value, Value)> {
        doc["ranking"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                (
                    r["item"]["number"].as_i64().unwrap(),
                    r["score"].clone(),
                    r["because"].clone(),
                )
            })
            .collect()
    };
    let why = |doc: &Value| -> Vec<(i64, Value, Value)> {
        let mut found: Vec<(i64, Value, Value)> = doc["excluded"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                (
                    e["item"]["number"].as_i64().unwrap(),
                    e["why"].clone(),
                    e["reason"].clone(),
                )
            })
            .collect();
        found.sort_by_key(|(n, _, _)| *n);
        found
    };

    for policy in ["priority-recency", "leverage"] {
        let whole = next_json(&kernel, NEXT, &[("policy", policy)]);
        for args in [
            [("policy", policy), ("drain", "review-gate")],
            [("policy", policy), ("state", "queued")],
        ] {
            let narrowed = next_json(&kernel, NEXT, &args);
            let expected: Vec<_> = ranking(&whole)
                .into_iter()
                .filter(|(n, _, _)| in_queued.contains(n))
                .collect();
            assert!(
                expected.len() >= 3,
                "the corpus ranks several: {expected:?}"
            );
            assert_eq!(ranking(&narrowed), expected, "{args:?}");
            let expected: Vec<_> = why(&whole)
                .into_iter()
                .filter(|(n, _, _)| in_queued.contains(n))
                .collect();
            assert_eq!(why(&narrowed), expected, "{args:?}");
        }
    }

    // And spelled out, so the comparison above cannot pass on two wrong answers.
    let narrowed = next_json(
        &kernel,
        NEXT,
        &[("policy", "leverage"), ("drain", "review-gate")],
    );
    assert_eq!(numbers(&narrowed["ranking"]), vec![1, 5, 6]);
    assert_eq!(narrowed["ready"], 3);
    let leader = narrowed["ranking"][0]["because"].to_string();
    assert!(
        leader.contains("unblocks 3 open item(s)"),
        "#1 is credited for #3 and #4, outside the filter: {leader}"
    );
    // #8's blocker is filed, outside this drain, and it still blocks.
    assert_eq!(excluded(&kernel, &[("drain", "review-gate")]), vec![2, 8]);
    let plain = source(&kernel, NEXT, &[("drain", "review-gate")]);
    assert!(plain.contains("not #8: blocked by #7"), "{plain}");
    assert!(!plain.contains("Filed, ready"), "{plain}");
    // The filed blocker, asked about by its own drain, is ready; the blocked filed item is not.
    assert_eq!(ranked(&kernel, &[("drain", "triage")]), vec![7]);
    assert_eq!(excluded(&kernel, &[("drain", "triage")]), vec![3]);
}

/// A cycle is a fact about the ledger, and one outside the filter still refuses.
#[test]
fn a_cycle_outside_the_filter_is_still_refused() {
    let kernel = kernel();
    append(&kernel, "A", &[]);
    append(&kernel, "B", &[]);
    append(&kernel, "Queued, in no cycle", &[]);
    move_to(&kernel, "urn:iki:ledger:item:3", "filed", "queued");
    blocks(&kernel, "#1", "#2");
    blocks(&kernel, "#2", "#1");
    let refusal = try_verb(&kernel, Verb::Source, NEXT, &[("drain", "review-gate")])
        .expect_err("the graph has a cycle");
    assert!(refusal.to_string().contains("#1 → #2 → #1"), "{refusal}");
}

// ---------------------------------------------------------------------------- the refusals

#[test]
fn an_unknown_drain_is_refused_naming_the_lifecycles_drains() {
    let kernel = one_in_each();
    for args in [
        vec![("drain", "triag")],
        vec![("drain", "triag"), ("as", "application/json")],
    ] {
        let (name, detail) = invalid(try_verb(&kernel, Verb::Source, NEXT, &args));
        assert_eq!(name, "drain");
        assert_eq!(
            detail,
            format!(
                "`triag` is not a drain of the lifecycle `kata-flight` this ledger uses; one of \
                 triage, review-gate, ship (see `{KATA}`)"
            )
        );
    }
}

#[test]
fn an_unknown_state_is_refused_naming_the_lifecycles_states() {
    let kernel = one_in_each();
    for args in [
        vec![("state", "qeued")],
        vec![("state", "qeued"), ("as", "application/json")],
    ] {
        let (name, detail) = invalid(try_verb(&kernel, Verb::Source, NEXT, &args));
        assert_eq!(name, "state");
        assert_eq!(
            detail,
            format!(
                "`qeued` is not a state of the lifecycle `kata-flight` this ledger uses; one of \
                 filed, queued, reviewed, resolving, refining, shipping (see `{KATA}`)"
            )
        );
    }
}

/// The name of a state is not a drain, nor the reverse, and an empty value names neither.
#[test]
fn a_drain_and_a_state_are_not_each_others_names() {
    let kernel = one_in_each();
    for args in [
        [("drain", "queued")],
        [("state", "triage")],
        [("drain", "")],
        [("state", "")],
    ] {
        let (name, _) = invalid(try_verb(&kernel, Verb::Source, NEXT, &args));
        assert_eq!(name, args[0].0);
    }
}

/// Refused from the arguments, so a ledger with a cycle hears about the typo first, and an empty
/// ledger hears about it at all.
#[test]
fn a_refusal_does_not_depend_on_what_the_ledger_holds() {
    let empty = kernel();
    invalid(try_verb(&empty, Verb::Source, NEXT, &[("drain", "nope")]));

    let cyclic = kernel();
    append(&cyclic, "A", &[]);
    append(&cyclic, "B", &[]);
    blocks(&cyclic, "#1", "#2");
    blocks(&cyclic, "#2", "#1");
    invalid(try_verb(&cyclic, Verb::Source, NEXT, &[("state", "nope")]));
}

/// ⚠ A value that cannot be read is refused. Quietly reading it as absent would widen the answer
/// to the whole ledger, which a dispatcher would take for its own step's work.
#[test]
fn an_unreadable_value_is_refused_rather_than_read_as_no_filter() {
    let kernel = one_in_each();
    for name in ["drain", "state"] {
        let request = Request::new(Verb::Source, Iri::parse(NEXT).unwrap())
            .with_arg(name, ArgRef::Inline(vec![0xff, 0xfe]));
        let refused = block_on(kernel.issue(request, &Capability::root()))
            .map(|repr| String::from_utf8_lossy(&repr.bytes).into_owned());
        assert_eq!(invalid(refused).0, name);
    }
}

// ------------------------------------------------------------ states the lifecycle cannot say

/// An item whose current state cannot be said drains nowhere. The doctor reports each of these,
/// and they stay what they were to the unfiltered answer, and as blockers.
#[test]
fn an_item_with_no_one_known_state_is_not_admitted_and_still_blocks() {
    let kernel = kernel();
    let unknown = append_iri(&kernel, "Holds a state no lifecycle declares", &[]); // #1
    let two = append_iri(&kernel, "Holds two states", &[]); // #2
    let literal = append_iri(&kernel, "Holds a state as a literal", &[]); // #3
    let other = append_iri(&kernel, "Holds another lifecycle's queued", &[]); // #4
    append(&kernel, "Queued, blocked by #1", &[]); // #5
    move_to(&kernel, "urn:iki:ledger:item:5", "filed", "queued");
    sink(
        &kernel,
        "urn:iki:store:update",
        &[(
            "content",
            &format!(
                "INSERT DATA {{ GRAPH <{LIVE}> {{ \
                 <{unknown}> <{STATE}> <{KATA}:vanished> . \
                 <{two}> <{STATE}> <{KATA}:queued>, <{KATA}:reviewed> . \
                 <{literal}> <{STATE}> \"queued\" . \
                 <{other}> <{STATE}> <urn:iki:ledger:lifecycle:elsewhere:queued> . }} }}"
            ),
        )],
    );
    blocks(&kernel, "#1", "#5");

    assert_eq!(ranked(&kernel, &[("state", "queued")]), Vec::<i64>::new());
    assert_eq!(excluded(&kernel, &[("state", "queued")]), vec![5]);
    assert_eq!(ranked(&kernel, &[("state", "reviewed")]), Vec::<i64>::new());
    assert_eq!(
        ranked(&kernel, &[("drain", "review-gate")]),
        Vec::<i64>::new()
    );
    assert_eq!(ranked(&kernel, &[("drain", "ship")]), Vec::<i64>::new());
    assert_eq!(ranked(&kernel, &[("drain", "triage")]), Vec::<i64>::new());
    assert_eq!(ranked(&kernel, &[("state", "filed")]), Vec::<i64>::new());
    let mut all = ranked(&kernel, &[]);
    all.sort_unstable();
    assert_eq!(all, vec![1, 2, 3, 4]);
}

// ----------------------------------------------------------------- the cache and the contract

/// Served through the kernel's cache, a filtered answer is its own answer, and a write cuts it.
#[test]
fn a_filtered_answer_follows_the_writes() {
    let kernel = kernel();
    append(&kernel, "Moves along", &[]);
    assert_eq!(ranked(&kernel, &[]), vec![1]);
    assert_eq!(ranked(&kernel, &[("drain", "triage")]), vec![1]);
    assert_eq!(
        ranked(&kernel, &[("drain", "review-gate")]),
        Vec::<i64>::new()
    );
    assert_eq!(
        ranked(&kernel, &[("drain", "review-gate")]),
        Vec::<i64>::new()
    );

    move_to(&kernel, "urn:iki:ledger:item:1", "filed", "queued");
    assert_eq!(ranked(&kernel, &[("drain", "triage")]), Vec::<i64>::new());
    assert_eq!(ranked(&kernel, &[("drain", "review-gate")]), vec![1]);
    assert_eq!(ranked(&kernel, &[]), vec![1]);
}

fn input(space: &dyn Space, name: &str) -> ArgSpec {
    let request = Request::new(Verb::Meta, Iri::parse(NEXT).unwrap());
    let Resolution::Hit(resolved) = space.resolve(&request, &Scope::empty()) else {
        panic!("next resolves");
    };
    let description = resolved.endpoint.describe();
    description
        .inputs
        .iter()
        .find(|input| input.name == name)
        .unwrap_or_else(|| panic!("next declares `{name}`"))
        .clone()
}

/// Optional, and enumerated across every lifecycle the host configured, as the state Sink's
/// arguments are: `describe` is one contract for every ledger, and the invocation holds a caller
/// to the lifecycle of the ledger it asks.
#[test]
fn the_arguments_are_declared() {
    let built_in = ikigai_ledger::space();
    let drain = input(&built_in, "drain");
    assert!(!drain.required);
    assert_eq!(drain.one_of, ["triage", "review-gate", "ship"]);
    let state = input(&built_in, "state");
    assert!(!state.required);
    assert_eq!(
        state.one_of,
        [
            "filed",
            "queued",
            "reviewed",
            "resolving",
            "refining",
            "shipping"
        ]
    );

    let both = Lifecycles::new(vec![
        Lifecycle::kata_flight(),
        Lifecycle::parse(SHARED).unwrap(),
    ]);
    let configured = ikigai_ledger::endpoints::space_with(SpaceConfig::default().lifecycles(both));
    assert_eq!(
        input(&configured, "drain").one_of,
        ["triage", "review-gate", "ship", "gate"]
    );
}
