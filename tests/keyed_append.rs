//! **The keyed append** (ledger #779): an append that carries the caller's own name for the
//! item, files at most one item per name per ledger, and answers the existing item when the
//! name is already taken — as ONE store update, so two concurrent callers cannot both file.
//!
//! The reason it exists is the first test: the pattern the gonk bridges (`roborev`, `kata`)
//! used on 0.3.0 — list the items `about` a key, and append when the list is empty — is a
//! check-then-act race. Two hooks running at once both see an empty list and both file.

mod common;

use std::sync::Barrier;

use common::*;
use ikigai_core::Verb;

const KEY: &str = "urn:roborev:finding:0123456789abcdef0123456789abcdef";

/// How many items in the default ledger are `about` a resource, closed ones included.
fn filed_about(kernel: &ikigai_core::Kernel, about: &str) -> usize {
    let listing = source(
        kernel,
        "urn:iki:ledger:items",
        &[("about", about), ("status", "all"), ("limit", "100")],
    );
    listing
        .lines()
        .filter(|line| line.trim_start().starts_with('#'))
        .count()
}

/// How many items the default ledger holds at all.
fn item_count(kernel: &ikigai_core::Kernel) -> usize {
    let json: serde_json::Value = serde_json::from_str(&select(
        kernel,
        "SELECT (COUNT(DISTINCT ?i) AS ?n) WHERE { GRAPH <urn:iki:ledger:graph:default> \
         { ?i a <https://ikigai-rs.dev/ns/ledger#Item> } }",
    ))
    .expect("the store answers JSON");
    json["results"]["bindings"][0]["n"]["value"]
        .as_str()
        .and_then(|n| n.parse().ok())
        .expect("a count")
}

/// ★ The race, reproduced: the bridges' check-then-append, with both callers' checks
/// landing before either append — which is all "two hooks at once" takes. Two items.
///
/// This is a property of the PATTERN, so it stays true after the fix; it is kept as the
/// record of why the keyed append exists, and so nobody reintroduces the pattern thinking
/// the store's serialized writes make it safe. They serialize each write, not the gap
/// between a read and a write.
#[test]
fn check_then_append_files_twice_when_two_callers_race() {
    let kernel = kernel();
    let barrier = Barrier::new(2);
    std::thread::scope(|scope| {
        for _ in 0..2 {
            scope.spawn(|| {
                let already = filed_about(&kernel, KEY) > 0;
                barrier.wait(); // both have checked; neither has filed
                if !already {
                    append(&kernel, "A finding", &[("about", KEY)]);
                }
            });
        }
    });
    assert_eq!(filed_about(&kernel, KEY), 2, "the race did not reproduce");
}

/// ★ The fix, under the same concurrency: eight callers released together, each filing with
/// the same key and no check of their own. Exactly one item, every answer names it, and
/// exactly one answer says it filed. Twenty rounds, because a race that holds once is luck.
#[test]
fn concurrent_keyed_appends_file_exactly_one_item() {
    for round in 0..20 {
        let kernel = kernel();
        let n = 8;
        let barrier = Barrier::new(n);
        let answers: Vec<String> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..n)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        try_verb(
                            &kernel,
                            Verb::Sink,
                            "urn:iki:ledger:append",
                            &[("content", "A finding"), ("key", KEY)],
                        )
                        .expect("a keyed append succeeds whether or not it files")
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        assert_eq!(item_count(&kernel), 1, "round {round}: {answers:#?}");
        let iris: std::collections::BTreeSet<&str> = answers
            .iter()
            .map(|a| a.split_whitespace().nth(1).expect("an IRI"))
            .collect();
        assert_eq!(iris.len(), 1, "round {round}: {answers:#?}");
        let filed = answers
            .iter()
            .filter(|a| a.split_whitespace().count() == 2)
            .count();
        assert_eq!(filed, 1, "round {round}: {answers:#?}");
    }
}

// ------------------------------------------------------------------ what a key means

fn keyed(kernel: &ikigai_core::Kernel, iri: &str, content: &str, key: &str) -> String {
    sink(kernel, iri, &[("content", content), ("key", key)])
}

/// The second append with a key files nothing, spends no number, ignores its own content,
/// and names the first — with a marker a filing never carries.
#[test]
fn a_second_append_with_the_same_key_answers_the_first() {
    let kernel = kernel();
    let first = keyed(&kernel, "urn:iki:ledger:append", "The original", KEY);
    let iri = first.split_whitespace().nth(1).unwrap().to_string();
    assert_eq!(
        first,
        format!("#1 {iri}\n"),
        "a filing answers as 0.3.0 did"
    );
    assert_eq!(
        keyed(
            &kernel,
            "urn:iki:ledger:append",
            "A replay, worded differently",
            KEY
        ),
        format!("#1 {iri} existing open\n")
    );
    assert_eq!(item_count(&kernel), 1);
    // The replay's content went nowhere, and no number was spent on it.
    assert!(source(&kernel, "urn:iki:ledger:item:1", &[]).contains("The original"));
    assert_eq!(append(&kernel, "Unkeyed", &[]), "#2");
}

#[test]
fn a_closed_item_still_holds_its_key() {
    let kernel = kernel();
    let iri = keyed(&kernel, "urn:iki:ledger:append", "Done already", KEY)
        .split_whitespace()
        .nth(1)
        .unwrap()
        .to_string();
    sink(&kernel, "urn:iki:ledger:close", &[("item", "#1")]);
    assert_eq!(
        keyed(&kernel, "urn:iki:ledger:append", "Again", KEY),
        format!("#1 {iri} existing closed\n")
    );
    assert_eq!(item_count(&kernel), 1);
}

/// ★ The decision for a deleted item: its key stays TAKEN. A keyed append is a replayable
/// request — a hook re-run, an importer run twice — and a replay must not resurrect what
/// someone deliberately deleted. So the tombstone keeps the key (through a purge too, as it
/// keeps the number), and the answer says `existing deleted`. Filing it again is a
/// deliberate act: a different key, or none.
#[test]
fn a_deleted_items_key_stays_taken_through_delete_and_purge() {
    let kernel = kernel();
    let iri = keyed(&kernel, "urn:iki:ledger:append", "Junk", KEY)
        .split_whitespace()
        .nth(1)
        .unwrap()
        .to_string();
    delete(&kernel, &format!("urn:iki:ledger:item:key:{KEY}"), &[]);
    assert_eq!(
        keyed(&kernel, "urn:iki:ledger:append", "Junk, replayed", KEY),
        format!("#1 {iri} existing deleted\n")
    );
    assert_eq!(
        item_count(&kernel),
        0,
        "a replay resurrected a deleted item"
    );
    let gone = try_verb(
        &kernel,
        Verb::Source,
        &format!("urn:iki:ledger:item:key:{KEY}"),
        &[],
    );
    assert!(
        matches!(&gone, Err(ikigai_core::Error::NotFound(m)) if m.contains("was deleted")),
        "{gone:?}"
    );
    // The tombstone carries the key, which is what keeps it taken.
    let tombstone_key = select(
        &kernel,
        "SELECT ?k WHERE { GRAPH <urn:iki:ledger:graph:default> { ?t a \
         <https://ikigai-rs.dev/ns/ledger#Tombstone> ; \
         <https://ikigai-rs.dev/ns/ledger#key> ?k } }",
    );
    assert!(tombstone_key.contains(KEY), "{tombstone_key}");
    // A purge, named by the key, keeps it taken too.
    delete(
        &kernel,
        "urn:iki:ledger:purge",
        &[("content", &format!("key:{KEY}"))],
    );
    assert_eq!(
        keyed(
            &kernel,
            "urn:iki:ledger:append",
            "Junk, replayed again",
            KEY
        ),
        format!("#1 {iri} existing deleted\n")
    );
}

/// Unique per LEDGER: a ledger is a boundary, so two ledgers may use one key.
#[test]
fn a_key_is_unique_per_ledger() {
    let kernel = kernel();
    let here = keyed(&kernel, "urn:iki:ledger:append", "Default's", KEY);
    let there = keyed(&kernel, "urn:iki:ledger:acme:append", "Acme's", KEY);
    assert!(
        here.starts_with("#1 ") && here.split_whitespace().count() == 2,
        "{here}"
    );
    assert!(
        there.starts_with("acme#1 ") && there.split_whitespace().count() == 2,
        "{there}"
    );
    assert!(
        source(&kernel, &format!("urn:iki:ledger:acme:item:key:{KEY}"), &[]).contains("Acme's")
    );
}

/// The lookups: the item resource by key, `item=key:…` on every write, and `items key=`.
#[test]
fn a_key_finds_its_item_everywhere_an_item_is_named() {
    let kernel = kernel();
    append(&kernel, "Unkeyed", &[]);
    keyed(&kernel, "urn:iki:ledger:append", "Keyed", KEY);
    let by_key = format!("urn:iki:ledger:item:key:{KEY}");
    let detail = source(&kernel, &by_key, &[]);
    assert!(detail.starts_with("   #2  open"), "{detail}");
    assert!(
        detail.contains(&format!("\n  key:      {KEY}\n")),
        "{detail}"
    );
    assert_eq!(ok(&kernel, Verb::Exists, &by_key, &[]), "true\n");
    assert_eq!(
        ok(
            &kernel,
            Verb::Exists,
            "urn:iki:ledger:item:key:no-such-key",
            &[]
        ),
        "false\n"
    );
    // `item=` in both spellings.
    sink(
        &kernel,
        "urn:iki:ledger:comment",
        &[("item", &format!("key:{KEY}")), ("content", "by key")],
    );
    sink(
        &kernel,
        "urn:iki:ledger:label",
        &[("item", &by_key), ("content", "found")],
    );
    let detail = source(&kernel, "urn:iki:ledger:item:2", &[]);
    assert!(
        detail.contains("by key") && detail.contains("[found]"),
        "{detail}"
    );
    // `items key=` is a filter, and finds the one.
    let listing = source(&kernel, "urn:iki:ledger:items", &[("key", KEY)]);
    assert!(
        listing.contains("#2") && !listing.contains("#1 "),
        "{listing}"
    );
    assert!(listing.contains("1 item(s)"), "{listing}");
}

/// Declared = enforced: a key outside the declared shape is refused — and refused BEFORE
/// anything is written, as is an `as` the answer cannot be given in.
#[test]
fn a_bad_key_or_face_is_refused_before_anything_is_filed() {
    let kernel = kernel();
    for bad in ["", "a b", "a/b", "a#b", "ünï", &"k".repeat(257)] {
        let refused = try_verb(
            &kernel,
            Verb::Sink,
            "urn:iki:ledger:append",
            &[("content", "X"), ("key", bad)],
        );
        assert!(
            matches!(&refused, Err(ikigai_core::Error::InvalidArgument { name, .. }) if name == "key"),
            "`{bad}`: {refused:?}"
        );
    }
    let refused = try_verb(
        &kernel,
        Verb::Sink,
        "urn:iki:ledger:append",
        &[("content", "X"), ("as", "text/turtle")],
    );
    assert!(
        matches!(&refused, Err(ikigai_core::Error::InvalidArgument { name, .. }) if name == "as"),
        "{refused:?}"
    );
    assert_eq!(item_count(&kernel), 0, "a refused append filed something");
    let refused = try_verb(
        &kernel,
        Verb::Source,
        "urn:iki:ledger:items",
        &[("key", "a/b")],
    );
    assert!(
        matches!(&refused, Err(ikigai_core::Error::InvalidArgument { name, .. }) if name == "key"),
        "{refused:?}"
    );
}

/// The keyed append in the JSON face: the outcome is a field, not a word to find.
#[test]
fn the_json_answer_says_filed_or_existing() {
    let kernel = kernel();
    let ask = |content: &str| -> serde_json::Value {
        serde_json::from_str(&sink(
            &kernel,
            "urn:iki:ledger:append",
            &[
                ("content", content),
                ("key", KEY),
                ("as", "application/json"),
            ],
        ))
        .expect("the answer is JSON")
    };
    let first = ask("First");
    assert_eq!(first["outcome"], "filed");
    assert_eq!(first["status"], "open");
    assert_eq!(first["key"], KEY);
    let second = ask("Second");
    assert_eq!(second["outcome"], "existing");
    assert_eq!(second["item"], first["item"]);
}
