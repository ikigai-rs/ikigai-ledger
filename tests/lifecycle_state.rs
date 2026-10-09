//! **An item's lifecycle state** (ledger #775, step 1): one value or none, moved by a
//! compare-and-set in ONE store update, checked against a lifecycle resource, and asserted
//! by name.
//!
//! The reason it is its own resource is the first test. kata-flight keeps its state in
//! `lifecycle:*` labels and moves an item with a label remove followed by a label add; this
//! ledger has `label` Sink and Delete as well, and composing a transition out of them is the
//! same two writes with the same gap between them. Two writers moving one item both read the
//! old label, both remove it, both add their own: two lifecycle labels, which is the defect
//! kata's reaper hunts for.

mod common;

use std::sync::Barrier;

use common::*;
use ikigai_core::{Capability, Error, Iri, Kernel, Request, Verb};

/// How many `ledger:state` values the store holds for item `#n` of the default ledger —
/// asked of the store directly, not of an endpoint that might summarize.
fn states_in_store(kernel: &Kernel, n: i64) -> Vec<String> {
    let json: serde_json::Value = serde_json::from_str(&select(
        kernel,
        &format!(
            "SELECT ?s WHERE {{ GRAPH <urn:iki:ledger:graph:default> {{ \
             ?i <https://ikigai-rs.dev/ns/ledger#number> {n} ; \
                <https://ikigai-rs.dev/ns/ledger#state> ?s }} }} ORDER BY ?s"
        ),
    ))
    .expect("the store answers JSON");
    json["results"]["bindings"]
        .as_array()
        .expect("bindings")
        .iter()
        .map(|b| b["s"]["value"].as_str().unwrap().to_string())
        .collect()
}

fn transition(kernel: &Kernel, n: i64, from: &str, to: &str) -> Result<String, Error> {
    try_verb(
        kernel,
        Verb::Sink,
        &format!("urn:iki:ledger:item:{n}:state"),
        &[("from", from), ("to", to)],
    )
}

fn exists(kernel: &Kernel, iri: &str) -> Result<bool, Error> {
    try_verb(kernel, Verb::Exists, iri, &[]).map(|answer| answer.trim() == "true")
}

fn state_of(kernel: &Kernel, n: i64) -> String {
    source(kernel, &format!("urn:iki:ledger:item:{n}:state"), &[])
        .trim()
        .to_string()
}

// --------------------------------------------------------------------- the race

/// ★ The defect, reproduced with this ledger's own resources: a transition composed of a
/// label remove and a label add, with both writers' reads landing before either write. The
/// item ends with TWO lifecycle labels.
///
/// A property of the PATTERN, so it stays true; it is kept as the record of why the state is
/// a resource of its own, and so nobody builds a transition out of labels thinking the
/// store's serialized writes make it safe. They serialize each write, not the gap between a
/// read and the write after it.
#[test]
fn a_transition_composed_of_label_writes_double_labels_when_two_writers_race() {
    let kernel = kernel();
    append(&kernel, "In a wave", &[("labels", "lifecycle-queued")]);
    let barrier = Barrier::new(2);
    std::thread::scope(|scope| {
        for next in ["lifecycle-reviewed", "lifecycle-resolving"] {
            let (kernel, barrier) = (&kernel, &barrier);
            scope.spawn(move || {
                let item = source(kernel, "urn:iki:ledger:item:1", &[]);
                let queued = item.contains("lifecycle-queued");
                barrier.wait(); // both have read the label; neither has written
                if queued {
                    delete(
                        kernel,
                        "urn:iki:ledger:label",
                        &[("item", "1"), ("content", "lifecycle-queued")],
                    );
                    sink(
                        kernel,
                        "urn:iki:ledger:label",
                        &[("item", "1"), ("content", next)],
                    );
                }
            });
        }
    });
    let line = source(&kernel, "urn:iki:ledger:items", &[]);
    assert!(
        line.contains("lifecycle-reviewed") && line.contains("lifecycle-resolving"),
        "the race did not reproduce: {line}"
    );
}

/// ★ The fix, under the same concurrency: two transitions out of ONE state, released
/// together. Exactly one succeeds, the other is a typed `Conflict` naming the state the item
/// is in, and the store holds exactly one state — twenty rounds, because a race that holds
/// once is luck. Run both ways: to two different states, and to the SAME state, which is
/// the case a read-back of the state alone could not tell apart.
#[test]
fn two_transitions_from_one_state_one_wins_and_the_other_is_a_conflict() {
    for round in 0..20 {
        for targets in [["reviewed", "resolving"], ["reviewed", "reviewed"]] {
            let kernel = kernel();
            append(&kernel, "In a wave", &[]);
            transition(&kernel, 1, "filed", "queued").expect("the first transition");
            let barrier = Barrier::new(2);
            let answers: Vec<Result<String, Error>> = std::thread::scope(|scope| {
                let handles: Vec<_> = targets
                    .iter()
                    .map(|to| {
                        let (kernel, barrier) = (&kernel, &barrier);
                        scope.spawn(move || {
                            barrier.wait();
                            transition(kernel, 1, "queued", to)
                        })
                    })
                    .collect();
                handles.into_iter().map(|h| h.join().unwrap()).collect()
            });
            let won: Vec<&String> = answers.iter().filter_map(|a| a.as_ref().ok()).collect();
            assert_eq!(won.len(), 1, "round {round} {targets:?}: {answers:#?}");
            let lost = answers
                .iter()
                .find_map(|a| a.as_ref().err())
                .expect("one loser");
            let Error::Conflict(message) = lost else {
                panic!("round {round} {targets:?}: the loser is not a Conflict: {lost:?}");
            };
            let now = state_of(&kernel, 1);
            assert!(
                message.contains(&format!("is in `{now}`")),
                "the Conflict names the state the item is in ({now}): {message}"
            );
            let stored = states_in_store(&kernel, 1);
            assert_eq!(stored.len(), 1, "round {round} {targets:?}: {stored:?}");
            assert!(
                won[0].contains(&now),
                "the winner's answer names the state that stuck: {} vs {now}",
                won[0]
            );
        }
    }
}

// ------------------------------------------------------------------ the atom

#[test]
fn an_item_with_no_state_is_filed_and_moves_one_state_at_a_time() {
    let kernel = kernel();
    append(&kernel, "Ship it", &[]);
    assert_eq!(state_of(&kernel, 1), "filed");
    assert!(states_in_store(&kernel, 1).is_empty(), "filed is ABSENCE");

    assert_eq!(
        transition(&kernel, 1, "filed", "queued").unwrap(),
        "#1 queued (was filed)\n"
    );
    assert_eq!(state_of(&kernel, 1), "queued");
    assert_eq!(
        states_in_store(&kernel, 1),
        ["urn:iki:ledger:lifecycle:kata-flight:queued"],
        "the state is the lifecycle's IRI for it"
    );
    transition(&kernel, 1, "queued", "reviewed").unwrap();
    transition(&kernel, 1, "reviewed", "resolving").unwrap();
    // Back to filed removes the state rather than writing a value for "none".
    transition(&kernel, 1, "resolving", "filed").unwrap();
    assert_eq!(state_of(&kernel, 1), "filed");
    assert!(states_in_store(&kernel, 1).is_empty());
}

#[test]
fn a_transition_from_the_wrong_state_is_a_conflict_and_writes_nothing() {
    let kernel = kernel();
    append(&kernel, "Ship it", &[]);
    transition(&kernel, 1, "filed", "queued").unwrap();
    let before = source(
        &kernel,
        "urn:iki:ledger:item:1",
        &[("as", "application/json")],
    );
    let refused = transition(&kernel, 1, "reviewed", "resolving").expect_err("wrong from");
    let Error::Conflict(message) = &refused else {
        panic!("not a Conflict: {refused:?}");
    };
    assert!(
        message.contains("is in `queued`, not `reviewed`"),
        "{message}"
    );
    assert!(message.contains("urn:iki:ledger:item:1:state"), "{message}");
    let after = source(
        &kernel,
        "urn:iki:ledger:item:1",
        &[("as", "application/json")],
    );
    assert_eq!(
        before, after,
        "a refused transition changes nothing, not even the stamp"
    );
}

#[test]
fn a_state_outside_the_lifecycle_is_refused_before_anything_is_read() {
    let kernel = kernel();
    append(&kernel, "Ship it", &[]);
    for (from, to, arg) in [("filed", "shipped", "to"), ("done", "queued", "from")] {
        let refused = transition(&kernel, 1, from, to).expect_err("not in the vocabulary");
        let Error::InvalidArgument { name, detail } = &refused else {
            panic!("not InvalidArgument: {refused:?}");
        };
        assert_eq!(name, arg);
        assert!(detail.contains("kata-flight"), "{detail}");
        assert!(detail.contains("queued, reviewed"), "{detail}");
    }
    // Even for an item that does not exist: the argument is wrong whatever the item is.
    assert!(matches!(
        transition(&kernel, 99, "filed", "shipped"),
        Err(Error::InvalidArgument { .. })
    ));
    assert!(matches!(
        transition(&kernel, 99, "filed", "queued"),
        Err(Error::NotFound(_))
    ));
}

#[test]
fn moving_to_the_state_it_is_in_writes_nothing() {
    let kernel = kernel();
    append(&kernel, "Ship it", &[]);
    transition(&kernel, 1, "filed", "queued").unwrap();
    let request = Request::new(Verb::Source, Iri::parse("urn:iki:ledger:items").unwrap());
    source(&kernel, "urn:iki:ledger:items", &[]);
    assert!(kernel.is_cached(&request, &Capability::root()));
    assert_eq!(
        transition(&kernel, 1, "queued", "queued").unwrap(),
        "#1 queued (unchanged)\n"
    );
    assert!(
        kernel.is_cached(&request, &Capability::root()),
        "an unchanged transition did not reach the store"
    );
}

#[test]
fn a_note_on_a_transition_is_a_comment() {
    let kernel = kernel();
    append(&kernel, "Ship it", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:item:1:state",
        &[
            ("from", "filed"),
            ("to", "queued"),
            ("content", "wave 3"),
            ("author", "kata-ship/0abc1234"),
        ],
    );
    let item = source(&kernel, "urn:iki:ledger:item:1", &[]);
    assert!(item.contains("wave 3"), "{item}");
    assert!(item.contains("kata-ship/0abc1234"), "{item}");
    assert!(item.contains("  state:    queued\n"), "{item}");
}

/// The state's JSON face is the machine contract a loop reads after a step.
#[test]
fn the_state_reads_as_json_with_its_lifecycle_role() {
    let kernel = kernel();
    let iri = append_iri(&kernel, "Ship it", &[]);
    let filed: serde_json::Value = serde_json::from_str(&source(
        &kernel,
        "urn:iki:ledger:item:1:state",
        &[("as", "application/json")],
    ))
    .unwrap();
    assert_eq!(
        filed,
        serde_json::json!({
            "schema": 1, "ledger": "default",
            "item": {"number": 1, "display": "#1", "iri": iri},
            "lifecycle": "kata-flight", "state": "filed",
            "in_flight": false, "drain": "triage", "since": null
        })
    );
    let moved: serde_json::Value = serde_json::from_str(
        &try_verb(
            &kernel,
            Verb::Sink,
            "urn:iki:ledger:item:1:state",
            &[
                ("from", "filed"),
                ("to", "resolving"),
                ("as", "application/json"),
            ],
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(moved["outcome"], "transitioned");
    assert_eq!(moved["from"], "filed");
    assert_eq!(moved["state"], "resolving");
    assert_eq!(moved["in_flight"], true);
    assert_eq!(moved["drain"], serde_json::Value::Null);
    assert!(moved["since"].as_str().unwrap().ends_with('Z'));
}

/// The item's own JSON gains `state` — `filed` for an item that never entered a lifecycle.
#[test]
fn the_item_json_face_carries_the_state() {
    let kernel = kernel();
    append(&kernel, "Ship it", &[]);
    let read = |kernel: &Kernel| -> serde_json::Value {
        serde_json::from_str(&source(
            kernel,
            "urn:iki:ledger:item:1",
            &[("as", "application/json")],
        ))
        .unwrap()
    };
    assert_eq!(read(&kernel)["item"]["state"], "filed");
    transition(&kernel, 1, "filed", "queued").unwrap();
    assert_eq!(read(&kernel)["item"]["state"], "queued");
}

/// ⚠ Two states can only come from a write around the Sink. The item reads with `state:
/// null`, the state resource refuses to name one, and no transition can name the state it
/// leaves — every face says the same thing about it.
#[test]
fn two_states_written_around_the_sink_are_reported_on_every_face() {
    let kernel = kernel();
    let iri = append_iri(&kernel, "Ship it", &[]);
    sink(
        &kernel,
        "urn:iki:store:update",
        &[(
            "content",
            &format!(
                "INSERT DATA {{ GRAPH <urn:iki:ledger:graph:default> {{ <{iri}> \
                 <https://ikigai-rs.dev/ns/ledger#state> \
                 <urn:iki:ledger:lifecycle:kata-flight:queued>, \
                 <urn:iki:ledger:lifecycle:kata-flight:resolving> }} }}"
            ),
        )],
    );
    let json: serde_json::Value = serde_json::from_str(&source(
        &kernel,
        "urn:iki:ledger:item:1",
        &[("as", "application/json")],
    ))
    .unwrap();
    assert_eq!(json["item"]["state"], serde_json::Value::Null);
    let read = try_verb(&kernel, Verb::Source, "urn:iki:ledger:item:1:state", &[])
        .expect_err("two states have no one name");
    assert!(read.to_string().contains("2 states"), "{read}");
    let moved = transition(&kernel, 1, "queued", "reviewed").expect_err("no from fits");
    assert!(matches!(moved, Error::Conflict(_)), "{moved:?}");
    assert!(!exists(&kernel, "urn:iki:ledger:item:1:state:queued").unwrap());
}

// ----------------------------------------------------------------- assertions

#[test]
fn the_assertions_answer_what_is_true_now() {
    let kernel = kernel();
    append(&kernel, "Ship it", &[]);
    assert!(exists(&kernel, "urn:iki:ledger:item:1:state:filed").unwrap());
    assert!(!exists(&kernel, "urn:iki:ledger:item:1:state:queued").unwrap());
    assert!(exists(&kernel, "urn:iki:ledger:item:1:holder:none").unwrap());
    assert!(!exists(&kernel, "urn:iki:ledger:item:1:holder:any").unwrap());
    assert!(!exists(&kernel, "urn:iki:ledger:item:1:closed").unwrap());

    transition(&kernel, 1, "filed", "resolving").unwrap();
    sink(
        &kernel,
        "urn:iki:ledger:claim",
        &[("item", "1"), ("content", "urn:agents:session:abc")],
    );
    assert!(exists(&kernel, "urn:iki:ledger:item:1:state:resolving").unwrap());
    assert!(!exists(&kernel, "urn:iki:ledger:item:1:state:filed").unwrap());
    // A holder that is itself an IRI is named as it stands.
    assert!(exists(
        &kernel,
        "urn:iki:ledger:item:1:holder:urn:agents:session:abc"
    )
    .unwrap());
    assert!(exists(&kernel, "urn:iki:ledger:item:1:holder:any").unwrap());
    assert!(!exists(&kernel, "urn:iki:ledger:item:1:holder:someone-else").unwrap());

    sink(&kernel, "urn:iki:ledger:close", &[("item", "1")]);
    assert!(exists(&kernel, "urn:iki:ledger:item:1:closed").unwrap());
}

/// "Item 99 is closed" is false of an item that does not exist; a state the lifecycle does
/// not declare is a typo, and a typo must not read as "not in that state".
#[test]
fn an_assertion_about_nothing_is_false_and_a_misspelled_state_is_refused() {
    let kernel = kernel();
    append(&kernel, "Ship it", &[]);
    assert!(!exists(&kernel, "urn:iki:ledger:item:99:closed").unwrap());
    assert!(!exists(&kernel, "urn:iki:ledger:item:99:state:filed").unwrap());
    assert!(matches!(
        exists(&kernel, "urn:iki:ledger:item:1:state:qeued"),
        Err(Error::InvalidArgument { .. })
    ));
}

/// ★ Cached, and invalidated by a write to the item: an assertion answered from a cache that
/// a transition did not cut would tell the loop a step happened that had been undone.
#[test]
fn an_assertion_is_cached_and_a_write_to_the_item_invalidates_it() {
    let kernel = kernel();
    append(&kernel, "Ship it", &[]);
    let names = [
        "urn:iki:ledger:item:1:state:filed",
        "urn:iki:ledger:item:1:holder:none",
        "urn:iki:ledger:item:1:closed",
    ];
    let request = |iri: &str| Request::new(Verb::Exists, Iri::parse(iri).unwrap());
    for name in names {
        exists(&kernel, name).unwrap();
        assert!(
            kernel.is_cached(&request(name), &Capability::root()),
            "{name} did not cache"
        );
    }
    transition(&kernel, 1, "filed", "queued").unwrap();
    for name in names {
        assert!(
            !kernel.is_cached(&request(name), &Capability::root()),
            "{name} survived a write to its item"
        );
    }
    assert!(
        !exists(&kernel, names[0]).unwrap(),
        "and re-reads the truth"
    );
}

/// The parts take an item's number or opaque id, in a named ledger too.
#[test]
fn the_parts_work_in_a_named_ledger_and_by_opaque_id() {
    let kernel = kernel();
    let iri = sink(
        &kernel,
        "urn:iki:ledger:acme:append",
        &[("content", "Their work")],
    )
    .split_whitespace()
    .nth(1)
    .unwrap()
    .to_string();
    let id = iri.rsplit(':').next().unwrap();
    sink(
        &kernel,
        &format!("urn:iki:ledger:acme:item:{id}:state"),
        &[("from", "filed"), ("to", "queued")],
    );
    assert!(exists(&kernel, "urn:iki:ledger:acme:item:1:state:queued").unwrap());
    // The default ledger's #1 does not exist, and is not acme's.
    assert!(!exists(&kernel, "urn:iki:ledger:item:1:state:queued").unwrap());
    assert_eq!(
        try_verb(&kernel, Verb::Source, &format!("{iri}:state"), &[]).unwrap(),
        "queued\n"
    );
}

// ------------------------------------------------------------------ lifecycles

#[test]
fn the_lifecycle_is_a_resource() {
    let kernel = kernel();
    let plain = source(&kernel, "urn:iki:ledger:lifecycle:kata-flight", &[]);
    assert!(plain.starts_with("kata-flight\n"), "{plain}");
    for line in [
        "filed      drain: triage  (no ledger:state)",
        "queued     drain: review-gate",
        "resolving  in flight",
    ] {
        assert!(plain.contains(line), "{line:?} in {plain}");
    }
    let turtle = source(
        &kernel,
        "urn:iki:ledger:lifecycle:kata-flight",
        &[("as", "text/turtle")],
    );
    assert!(turtle.contains("ledger:inFlight"), "{turtle}");
    assert!(!turtle.contains("_:"), "{turtle}");
    assert!(matches!(
        try_verb(&kernel, Verb::Source, "urn:iki:ledger:lifecycle:nope", &[]),
        Err(Error::NotFound(_))
    ));
}

/// A host's own lifecycle, assigned to one ledger: that ledger's transitions are checked
/// against it, and every other ledger keeps the default.
#[test]
fn a_host_assigns_its_own_lifecycle_to_a_ledger() {
    use std::sync::Arc;
    let tiny = ikigai_ledger::Lifecycle::parse(
        r#"@prefix ledger: <https://ikigai-rs.dev/ns/ledger#> .
           @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
           <urn:iki:ledger:lifecycle:tiny> a ledger:Lifecycle ;
               ledger:hasState <urn:iki:ledger:lifecycle:tiny:filed>, <urn:iki:ledger:lifecycle:tiny:doing> .
           <urn:iki:ledger:lifecycle:tiny:filed> rdfs:label "filed" ; ledger:order 0 ;
               ledger:inFlight false ; ledger:drain "me" .
           <urn:iki:ledger:lifecycle:tiny:doing> rdfs:label "doing" ; ledger:order 1 ;
               ledger:inFlight true ."#,
    )
    .unwrap();
    let config = ikigai_ledger::SpaceConfig::default().lifecycles(
        ikigai_ledger::Lifecycles::new(vec![ikigai_ledger::Lifecycle::kata_flight(), tiny])
            .assign("acme", "tiny"),
    );
    let space = ikigai_core::Fallback::new(vec![
        Arc::new(ikigai_store::space(
            ikigai_store::DurableStore::in_memory().unwrap(),
        )) as Arc<dyn ikigai_core::Space>,
        Arc::new(ikigai_ledger::space_with(config)) as Arc<dyn ikigai_core::Space>,
    ]);
    let kernel =
        Kernel::with_meta_renderer(Arc::new(space), Arc::new(ikigai_vocab::TurtleRenderer))
            .with_clock(Arc::new(TickingClock::default()));
    sink(&kernel, "urn:iki:ledger:acme:append", &[("content", "a")]);
    append(&kernel, "b", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:acme:item:1:state",
        &[("from", "filed"), ("to", "doing")],
    );
    assert!(matches!(
        try_verb(
            &kernel,
            Verb::Sink,
            "urn:iki:ledger:acme:item:1:state",
            &[("from", "doing"), ("to", "queued")],
        ),
        Err(Error::InvalidArgument { .. })
    ));
    assert!(matches!(
        transition(&kernel, 1, "filed", "doing"),
        Err(Error::InvalidArgument { .. })
    ));
    transition(&kernel, 1, "filed", "queued").unwrap();
    assert!(source(&kernel, "urn:iki:ledger:lifecycle:tiny", &[]).contains("doing"));
}

/// The state Sink is gated like every other write: a read grant can read the state and
/// cannot move it.
#[test]
fn a_read_grant_reads_the_state_and_cannot_move_it() {
    let kernel = kernel();
    append(&kernel, "Ship it", &[]);
    let reader = Capability::scoped([
        "urn:cap:ledger:read:default".to_string(),
        graph_read("default"),
    ]);
    assert_eq!(
        try_as(
            &kernel,
            &reader,
            Verb::Source,
            "urn:iki:ledger:item:1:state",
            &[]
        )
        .unwrap(),
        "filed\n"
    );
    let refused = try_as(
        &kernel,
        &reader,
        Verb::Sink,
        "urn:iki:ledger:item:1:state",
        &[("from", "filed"), ("to", "queued")],
    )
    .expect_err("a reader cannot transition");
    assert!(matches!(refused, Error::Denied(_)), "{refused:?}");
}
