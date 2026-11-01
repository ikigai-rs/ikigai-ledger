//! **Aliases by scheme** (ledger #2828): an item resolvable by an external id, in a scheme one
//! principal owns.
//!
//! Before 0.15.0 an item had three names, its number, its opaque id and its key, and an
//! external id that was not the key had nowhere to go: `item=kata:01m6c5…` was looked up as an
//! opaque id and not found, and `item=gh:acme/web#12` was refused as `#12` of a ledger called
//! `gh:acme/web`. `the_defect_an_external_id_named_no_item` is that reproduction, kept as the
//! first test. The rest hold the design to what the item asks:
//!
//! - every reference form that takes an item resolves an alias, and so does its IRI;
//! - only the owner registered with a scheme adds aliases in it, and a non-owner is refused
//!   naming the owner;
//! - an alias already naming another item is refused naming that item, decided inside the
//!   write, so concurrent adds of one alias leave one;
//! - an alias is not the key: a new item carries none, and the key resolves as it did;
//! - resolution never names an item to a caller without read on its ledger.

mod common;

use std::sync::{Arc, Barrier};

use common::*;
use ikigai_core::{Capability, Error, Fallback, Kernel, Space, Verb};
use ikigai_store::DurableStore;

const LEDGER: &str = "acme";
const OTHER: &str = "beta";
const ALIASES: &str = "urn:iki:alias:graph";
const IMPORTER: &str = "urn:iki:gonk:client:kata-import";
const RIVAL: &str = "urn:iki:gonk:client:someone-else";
const ULID: &str = "01m6c5bjbm000000000000e";

fn at(ledger: &str, resource: &str) -> String {
    format!("urn:iki:ledger:{ledger}:{resource}")
}

fn scheme(name: &str) -> String {
    format!("urn:iki:alias:scheme:{name}")
}

fn alias(name: &str, encoded_value: &str) -> String {
    format!("urn:iki:alias:scheme:{name}:value:{encoded_value}")
}

/// The store, the ledgers and the alias resources, as a host binds them.
fn kernel() -> Kernel {
    let space = Fallback::new(vec![
        Arc::new(ikigai_store::space(DurableStore::in_memory().unwrap())) as Arc<dyn Space>,
        Arc::new(ikigai_ledger::space()) as Arc<dyn Space>,
        Arc::new(ikigai_ledger::alias::space()) as Arc<dyn Space>,
    ]);
    Kernel::with_meta_renderer(Arc::new(space), Arc::new(ikigai_vocab::TurtleRenderer))
        .with_clock(Arc::new(TickingClock::default()))
}

fn aliases_read() -> String {
    format!("urn:cap:store:read:graph:{ALIASES}")
}

fn aliases_write() -> String {
    format!("urn:cap:store:write:graph:{ALIASES}")
}

/// An importer's grant, as a door mints it: register `kata` and `gh`, give aliases to items of
/// `acme` (and nothing else there but read), and the aliases graph's store grants.
fn importer(principal: &str) -> Capability {
    Capability::scoped(vec![
        "urn:cap:alias:scheme:kata".to_string(),
        "urn:cap:alias:scheme:gh".to_string(),
        format!("urn:cap:ledger:read:{LEDGER}"),
        format!("urn:cap:ledger:alias:{LEDGER}"),
        graph_read(LEDGER),
        graph_write(LEDGER),
        aliases_read(),
        aliases_write(),
    ])
    .with_principal(principal)
    .unwrap()
}

/// A reader of one ledger who may also read the aliases graph.
fn reader(ledger: &str) -> Capability {
    Capability::scoped(vec![
        format!("urn:cap:ledger:read:{ledger}"),
        format!("urn:cap:ledger:comment:{ledger}"),
        graph_read(ledger),
        graph_write(ledger),
        aliases_read(),
    ])
}

/// Two items in `acme`, one in `beta`, and the `kata` scheme registered to [`IMPORTER`].
fn seeded() -> Kernel {
    let kernel = kernel();
    sink(&kernel, &at(LEDGER, "append"), &[("content", "First")]);
    sink(&kernel, &at(LEDGER, "append"), &[("content", "Second")]);
    sink(&kernel, &at(OTHER, "append"), &[("content", "Elsewhere")]);
    let answer = try_as(
        &kernel,
        &importer(IMPORTER),
        Verb::Sink,
        &scheme("kata"),
        &[],
    )
    .unwrap();
    assert_eq!(answer, format!("scheme kata owned by {IMPORTER}\n"));
    kernel
}

fn give(kernel: &Kernel, who: &Capability, iri: &str, item: &str) -> Result<String, Error> {
    try_as(kernel, who, Verb::Sink, iri, &[("content", item)])
}

fn has(message: &str, needle: &str) {
    assert!(message.contains(needle), "`{needle}` is not in: {message}");
}

/// How many `ledger:aliasOf` triples the aliases graph holds for `subject`, asked of the store.
fn holders(kernel: &Kernel, subject: &str) -> usize {
    let json: serde_json::Value = serde_json::from_str(&select(
        kernel,
        &format!(
            "SELECT ?i WHERE {{ GRAPH <{ALIASES}> {{ <{subject}> <{}> ?i }} }}",
            ikigai_ledger::vocabulary::ALIAS_OF
        ),
    ))
    .unwrap();
    json["results"]["bindings"].as_array().unwrap().len()
}

/// ★ The reproduction: before aliases, an external id that is not the key names nothing. It
/// still names nothing until someone gives it, which is the point of the next test.
#[test]
fn the_defect_an_external_id_named_no_item() {
    let kernel = seeded();
    let comment = |item: &str| {
        try_verb(
            &kernel,
            Verb::Sink,
            &at(LEDGER, "comment"),
            &[("item", item), ("content", "hello")],
        )
    };
    match comment(&format!("kata:{ULID}")) {
        Err(Error::NotFound(message)) => has(&message, &format!("kata:{ULID}")),
        other => panic!("an unknown alias is not found, got {other:?}"),
    }
    match comment("gh:acme/web#12") {
        Err(Error::NotFound(message)) => has(&message, "alias `gh:acme/web#12`"),
        other => panic!("an unknown alias is not found, got {other:?}"),
    }
}

#[test]
fn every_reference_form_resolves_an_alias() {
    let kernel = seeded();
    let me = importer(IMPORTER);
    let first = at(LEDGER, "item:1");
    let answer = give(&kernel, &me, &alias("kata", ULID), &format!("{LEDGER}#1")).unwrap();
    has(
        &answer,
        &format!("kata:{ULID} names {LEDGER}#1 urn:iki:ledger:{LEDGER}:item:"),
    );
    try_as(&kernel, &me, Verb::Sink, &scheme("gh"), &[]).unwrap();
    give(
        &kernel,
        &me,
        &alias("gh", "acme%2Fweb%2312"),
        &format!("{LEDGER}#1"),
    )
    .unwrap();

    let iri_of_first = source(&kernel, &first, &[("as", "application/json")]);
    let iri: String = serde_json::from_str::<serde_json::Value>(&iri_of_first).unwrap()["item"]
        ["iri"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| panic!("an item's JSON names its IRI: {iri_of_first}"));

    // As `item=`, in both spellings and as the alias's own IRI.
    for reference in [
        format!("kata:{ULID}"),
        "gh:acme/web#12".to_string(),
        "gh:acme%2Fweb%2312".to_string(),
        alias("gh", "acme%2Fweb%2312"),
    ] {
        let answer = sink(
            &kernel,
            &at(LEDGER, "comment"),
            &[("item", &reference), ("content", "found by alias")],
        );
        has(&answer, &format!("commented on {LEDGER}#1"));
    }
    // As the item resource's id. (An item PART takes one segment, a number or an opaque id,
    // for the reason it refuses a key: a value may hold a `:`. See `ItemPartGrammar`.)
    let by_alias = source(&kernel, &at(LEDGER, &format!("item:kata:{ULID}")), &[]);
    has(&by_alias, &iri);
    let encoded = source(&kernel, &at(LEDGER, "item:gh:acme%2Fweb%2312"), &[]);
    has(&encoded, &iri);
    // As a link's target.
    let linked = sink(
        &kernel,
        &at(LEDGER, "link"),
        &[
            ("item", "#2"),
            ("content", &format!("kata:{ULID}")),
            ("type", "blocks"),
        ],
    );
    has(&linked, &format!("{LEDGER}#2 blocks {LEDGER}#1"));
    // And at the alias's own resource.
    let resolved = try_as(&kernel, &me, Verb::Source, &alias("kata", ULID), &[]).unwrap();
    has(&resolved, &format!("kata:{ULID} names {LEDGER}#1 {iri}\n"));
    has(&resolved, "First");
}

#[test]
fn an_item_carries_many_aliases_and_a_new_item_carries_none() {
    let kernel = seeded();
    let me = importer(IMPORTER);
    let filed = sink(
        &kernel,
        &at(LEDGER, "append"),
        &[("content", "Keyed"), ("key", "urn:kata:web:3f2")],
    );
    has(&filed, &format!("{LEDGER}#3"));
    let json: serde_json::Value = serde_json::from_str(&select(
        &kernel,
        &format!("SELECT ?a WHERE {{ GRAPH <{ALIASES}> {{ ?a a ?c }} }}"),
    ))
    .unwrap();
    assert_eq!(
        json["results"]["bindings"].as_array().unwrap().len(),
        1,
        "the aliases graph holds the scheme and nothing an append wrote: {json}"
    );
    give(&kernel, &me, &alias("kata", ULID), &format!("{LEDGER}#3")).unwrap();
    give(
        &kernel,
        &me,
        &alias("kata", "web%233f2"),
        &format!("{LEDGER}#3"),
    )
    .unwrap();
    for reference in [
        format!("kata:{ULID}"),
        "kata:web#3f2".to_string(),
        "key:urn:kata:web:3f2".to_string(),
    ] {
        let answer = sink(
            &kernel,
            &at(LEDGER, "comment"),
            &[("item", &reference), ("content", "same item")],
        );
        has(&answer, &format!("commented on {LEDGER}#3"));
    }
    // The key is not an alias, and the alias is not a key.
    match try_verb(
        &kernel,
        Verb::Sink,
        &at(LEDGER, "comment"),
        &[("item", &format!("key:kata:{ULID}")), ("content", "x")],
    ) {
        Err(Error::NotFound(_)) => {}
        other => panic!("an alias is not a key: {other:?}"),
    }
    // Giving the same alias to the same item again answers the same, and writes nothing new.
    let again = give(&kernel, &me, &alias("kata", ULID), &format!("{LEDGER}#3")).unwrap();
    has(&again, &format!("kata:{ULID} names {LEDGER}#3"));
    assert_eq!(holders(&kernel, &alias("kata", ULID)), 1);
}

#[test]
fn an_alias_already_naming_another_item_is_refused_naming_that_item() {
    let kernel = seeded();
    let me = importer(IMPORTER);
    let answer = give(&kernel, &me, &alias("kata", ULID), &format!("{LEDGER}#1")).unwrap();
    let first = answer.split_whitespace().nth(3).unwrap().to_string();
    match give(&kernel, &me, &alias("kata", ULID), &format!("{LEDGER}#2")) {
        Err(Error::Conflict(message)) => {
            has(
                &message,
                &format!("the alias `kata:{ULID}` already names `{first}`"),
            );
        }
        other => panic!("expected Conflict, got {other:?}"),
    }
    assert_eq!(holders(&kernel, &alias("kata", ULID)), 1);
}

#[test]
fn only_the_owner_adds_or_removes_and_a_non_owner_is_refused_naming_the_owner() {
    let kernel = seeded();
    let rival = importer(RIVAL);
    // A second registration is refused, naming the owner.
    match try_as(&kernel, &rival, Verb::Sink, &scheme("kata"), &[]) {
        Err(Error::Conflict(message)) => has(&message, &format!("belongs to `{IMPORTER}`")),
        other => panic!("expected Conflict, got {other:?}"),
    }
    // Registering again as the owner answers the same.
    let again = try_as(
        &kernel,
        &importer(IMPORTER),
        Verb::Sink,
        &scheme("kata"),
        &[],
    )
    .unwrap();
    assert_eq!(again, format!("scheme kata owned by {IMPORTER}\n"));
    // Adding in someone else's scheme is refused, naming the owner, and writes nothing.
    match give(
        &kernel,
        &rival,
        &alias("kata", ULID),
        &format!("{LEDGER}#1"),
    ) {
        Err(Error::Denied(message)) => {
            has(
                &message,
                &format!("the scheme `kata` belongs to `{IMPORTER}`"),
            );
        }
        other => panic!("expected Denied, got {other:?}"),
    }
    assert_eq!(holders(&kernel, &alias("kata", ULID)), 0);
    // So is removing.
    give(
        &kernel,
        &importer(IMPORTER),
        &alias("kata", ULID),
        &format!("{LEDGER}#1"),
    )
    .unwrap();
    match try_as(&kernel, &rival, Verb::Delete, &alias("kata", ULID), &[]) {
        Err(Error::Denied(message)) => has(&message, &format!("belongs to `{IMPORTER}`")),
        other => panic!("expected Denied, got {other:?}"),
    }
    assert_eq!(holders(&kernel, &alias("kata", ULID)), 1);
    // The owner removes it, and it names nothing after.
    let removed = try_as(
        &kernel,
        &importer(IMPORTER),
        Verb::Delete,
        &alias("kata", ULID),
        &[],
    )
    .unwrap();
    has(
        &removed,
        &format!("kata:{ULID} no longer names urn:iki:ledger:{LEDGER}:item:"),
    );
    assert_eq!(holders(&kernel, &alias("kata", ULID)), 0);
    match try_verb(
        &kernel,
        Verb::Source,
        &at(LEDGER, &format!("item:kata:{ULID}")),
        &[],
    ) {
        Err(Error::NotFound(_)) => {}
        other => panic!("a removed alias names nothing: {other:?}"),
    }
    // An unregistered scheme says how to register it.
    match give(
        &kernel,
        &rival,
        &alias("roborev", "job%2F88"),
        &format!("{LEDGER}#1"),
    ) {
        Err(Error::NotFound(message)) => {
            has(&message, "the scheme `roborev` is not registered");
            has(&message, "urn:cap:alias:scheme:roborev");
        }
        other => panic!("expected NotFound, got {other:?}"),
    }
}

#[test]
fn registering_takes_the_schemes_grant_and_a_principal() {
    let kernel = seeded();
    match try_as(
        &kernel,
        &importer(IMPORTER),
        Verb::Sink,
        &scheme("roborev"),
        &[],
    ) {
        Err(Error::Denied(message)) => has(&message, "`urn:cap:alias:scheme:roborev`"),
        other => panic!("expected Denied, got {other:?}"),
    }
    let anonymous = Capability::scoped(vec![
        "urn:cap:alias:scheme:roborev".to_string(),
        aliases_read(),
        aliases_write(),
    ]);
    match try_as(&kernel, &anonymous, Verb::Sink, &scheme("roborev"), &[]) {
        // Declared (`urn:cap:principal:*`), so the kernel refuses it before the endpoint runs.
        Err(Error::Denied(message)) => has(&message, "urn:cap:principal:"),
        other => panic!("expected Denied, got {other:?}"),
    }
    match try_verb(&kernel, Verb::Sink, &scheme("key"), &[]) {
        Err(Error::InvalidArgument { name, .. }) => assert_eq!(name, "scheme"),
        other => panic!("a reserved scheme is refused: {other:?}"),
    }
}

/// The owner of a scheme still cannot attach its names to a ledger it was not let into.
#[test]
fn adding_takes_the_alias_grant_on_the_items_own_ledger() {
    let kernel = seeded();
    match give(
        &kernel,
        &importer(IMPORTER),
        &alias("kata", ULID),
        &format!("{OTHER}#1"),
    ) {
        Err(Error::Denied(message)) => {
            has(&message, "urn:cap:ledger:alias:beta");
            has(&message, "urn:cap:ledger:write:beta");
        }
        other => panic!("expected Denied, got {other:?}"),
    }
    assert_eq!(holders(&kernel, &alias("kata", ULID)), 0);
}

/// ★ The authority boundary: resolution never names an item to a caller without read on its
/// ledger. An alias of an item the caller cannot read answers, word for word, what an alias
/// nobody holds answers.
#[test]
fn resolution_does_not_leak_an_item_to_a_caller_without_read_on_its_ledger() {
    let kernel = seeded();
    give(
        &kernel,
        &importer(IMPORTER),
        &alias("kata", ULID),
        &format!("{LEDGER}#1"),
    )
    .unwrap();
    let outsider = reader(OTHER);
    let comment = |reference: &str| {
        try_as(
            &kernel,
            &outsider,
            Verb::Sink,
            &at(OTHER, "comment"),
            &[("item", reference), ("content", "x")],
        )
    };
    let held = match comment(&format!("kata:{ULID}")) {
        Err(Error::NotFound(message)) => message,
        other => panic!("expected NotFound, got {other:?}"),
    };
    let nobody = match comment("kata:nobody-has-this") {
        Err(Error::NotFound(message)) => message,
        other => panic!("expected NotFound, got {other:?}"),
    };
    assert_eq!(
        held.replace(ULID, "V"),
        nobody.replace("nobody-has-this", "V")
    );
    assert!(!held.contains(LEDGER), "{held}");

    let held = match try_as(&kernel, &outsider, Verb::Source, &alias("kata", ULID), &[]) {
        Err(Error::NotFound(message)) => message,
        other => panic!("expected NotFound, got {other:?}"),
    };
    let nobody = match try_as(
        &kernel,
        &outsider,
        Verb::Source,
        &alias("kata", "nobody"),
        &[],
    ) {
        Err(Error::NotFound(message)) => message,
        other => panic!("expected NotFound, got {other:?}"),
    };
    assert_eq!(held.replace(ULID, "V"), nobody.replace("nobody", "V"));

    // A caller who CAN read the item's ledger is told where it is, as for a foreign IRI.
    let both = Capability::scoped(vec![
        format!("urn:cap:ledger:read:{LEDGER}"),
        format!("urn:cap:ledger:read:{OTHER}"),
        format!("urn:cap:ledger:comment:{OTHER}"),
        graph_read(OTHER),
        graph_write(OTHER),
        aliases_read(),
    ]);
    match try_as(
        &kernel,
        &both,
        Verb::Sink,
        &at(OTHER, "comment"),
        &[("item", &format!("kata:{ULID}")), ("content", "x")],
    ) {
        Err(Error::InvalidArgument { name, detail }) => {
            assert_eq!(name, "item");
            has(
                &detail,
                "an item of the ledger `acme`, and this resource is `beta`",
            );
        }
        other => panic!("expected InvalidArgument, got {other:?}"),
    }

    // Reading the aliases graph is a grant of its own, and the refusal says which.
    let no_aliases = Capability::scoped(vec![
        format!("urn:cap:ledger:read:{LEDGER}"),
        graph_read(LEDGER),
    ]);
    match try_as(
        &kernel,
        &no_aliases,
        Verb::Source,
        &at(LEDGER, &format!("item:kata:{ULID}")),
        &[],
    ) {
        Err(Error::Denied(message)) => {
            has(&message, &format!("urn:cap:store:read:graph:{ALIASES}"));
        }
        other => panic!("expected Denied, got {other:?}"),
    }
    // And a reference that is not an alias still needs no such grant.
    try_as(
        &kernel,
        &no_aliases,
        Verb::Source,
        &at(LEDGER, "item:1"),
        &[],
    )
    .unwrap();
}

/// ★ Uniqueness is decided inside the write: many principals' adds of one alias, to different
/// items, released together, leave exactly one alias, and every loser is refused naming the
/// winner.
#[test]
fn concurrent_adds_of_one_alias_leave_one() {
    for _ in 0..8 {
        let kernel = seeded();
        let me = importer(IMPORTER);
        let n = 6;
        for k in 3..=n {
            sink(
                &kernel,
                &at(LEDGER, "append"),
                &[("content", &format!("Item {k}"))],
            );
        }
        let barrier = Barrier::new(n);
        let answers: Vec<Result<String, Error>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (1..=n)
                .map(|k| {
                    let (kernel, me, barrier) = (&kernel, &me, &barrier);
                    scope.spawn(move || {
                        barrier.wait();
                        give(kernel, me, &alias("kata", ULID), &format!("{LEDGER}#{k}"))
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let won: Vec<&String> = answers.iter().filter_map(|a| a.as_ref().ok()).collect();
        assert_eq!(won.len(), 1, "exactly one add wins: {answers:?}");
        let winner = won[0].split_whitespace().nth(3).unwrap();
        for answer in &answers {
            match answer {
                Ok(_) => {}
                Err(Error::Conflict(message)) => has(message, &format!("already names `{winner}`")),
                other => panic!("a loser is a Conflict: {other:?}"),
            }
        }
        assert_eq!(holders(&kernel, &alias("kata", ULID)), 1);
    }
}

/// And a scheme raced by two principals has one owner.
#[test]
fn concurrent_registrations_of_one_scheme_leave_one_owner() {
    for _ in 0..8 {
        let kernel = kernel();
        let barrier = Barrier::new(2);
        let answers: Vec<Result<String, Error>> = std::thread::scope(|scope| {
            let handles: Vec<_> = [IMPORTER, RIVAL]
                .into_iter()
                .map(|who| {
                    let (kernel, barrier) = (&kernel, &barrier);
                    scope.spawn(move || {
                        barrier.wait();
                        try_as(kernel, &importer(who), Verb::Sink, &scheme("gh"), &[])
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        assert_eq!(
            answers.iter().filter(|a| a.is_ok()).count(),
            1,
            "{answers:?}"
        );
        let json: serde_json::Value = serde_json::from_str(&select(
            &kernel,
            &format!(
                "SELECT ?o WHERE {{ GRAPH <{ALIASES}> {{ <{}> <{}> ?o }} }}",
                scheme("gh"),
                ikigai_ledger::vocabulary::SCHEME_OWNER
            ),
        ))
        .unwrap();
        assert_eq!(
            json["results"]["bindings"].as_array().unwrap().len(),
            1,
            "{json}"
        );
    }
}
