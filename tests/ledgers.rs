//! Named ledgers: what the partition actually separates, and what it does not.
//!
//! # What each half of this file is for
//!
//! The first half is **isolation** — two ledgers, two graphs, two counters, two sets of
//! resources — and it is the easy half: a query that names a graph cannot see another.
//!
//! The second half is **authority**, and it is the one worth having. A grant names one
//! ledger; the manifold declares only the family (`urn:cap:ledger:read:*`) because the
//! kernel's pre-check runs before an endpoint can read the ledger out of the IRI, and the
//! exact grant is checked inside. Both directions are asserted here: a grant for `acme`
//! opens `acme`, and a grant for `acme` is refused at `bosatsu`.
//!
//! # ⚠ And the one this file asserts is NOT true
//!
//! `a_store_read_grant_still_sees_every_ledger` pins the gap rather than the property.
//! `urn:cap:store:read` is the whole dataset — `ikigai-store` has no per-graph read scope
//! yet — so a caller holding it reads any ledger's graph directly, without passing
//! through any resource in this crate. The ledger capabilities segment the ledger's own
//! doors and are not yet a tenancy boundary, and a test that pins the limitation is how
//! that stops being a sentence somebody has to remember.

mod common;

use common::*;
use ikigai_core::{Capability, Error, Verb};

/// Grants for one ledger, plus the store scopes a sub-request transitively needs.
fn grants_for(ledger: &str) -> Capability {
    Capability::scoped([
        format!("urn:cap:ledger:read:{ledger}"),
        format!("urn:cap:ledger:write:{ledger}"),
        format!("urn:cap:ledger:delete:{ledger}"),
        format!("urn:cap:ledger:purge:{ledger}"),
        "urn:cap:store:read".to_string(),
        "urn:cap:store:write".to_string(),
    ])
}

// ------------------------------------------------------------------------ isolation

/// ★ The property the whole arc exists for: two ledgers are two partitions, with their
/// own numbering, and neither appears in the other's listing.
#[test]
fn two_ledgers_have_separate_items_and_separate_numbering() {
    let kernel = kernel();

    let acme = sink(
        &kernel,
        "urn:iki:ledger:acme:append",
        &[("content", "Acme's first")],
    );
    let bosatsu = sink(
        &kernel,
        "urn:iki:ledger:bosatsu:append",
        &[("content", "Bosatsu's first")],
    );

    // Numbers restart per ledger — there is a counter in each graph — so the display
    // form carries the name, or `#1` would mean two different pieces of work.
    assert!(acme.starts_with("acme#1 "), "{acme}");
    assert!(bosatsu.starts_with("bosatsu#1 "), "{bosatsu}");
    assert!(acme.contains("urn:iki:ledger:acme:item:"), "{acme}");

    let listing = source(&kernel, "urn:iki:ledger:acme:items", &[]);
    assert!(listing.contains("Acme's first"), "{listing}");
    assert!(!listing.contains("Bosatsu's first"), "{listing}");
    assert!(listing.contains("1 item(s)"), "{listing}");

    // And the default ledger, which the bare forms address, sees neither.
    let default = source(&kernel, "urn:iki:ledger:items", &[]);
    assert!(default.contains("no items match"), "{default}");
}

/// The sugar is a spelling, not a second ledger: the bare form and `default` are the
/// same resource, the same graph and the same counter.
#[test]
fn the_bare_form_and_the_default_ledger_are_one_ledger() {
    let kernel = kernel();
    assert_eq!(append(&kernel, "Through the short form", &[]), "#1");
    let long = sink(
        &kernel,
        "urn:iki:ledger:default:append",
        &[("content", "Through the long form")],
    );
    // One counter, so the second append is #2 and not a second #1.
    assert!(long.starts_with("#2 "), "{long}");

    for iri in ["urn:iki:ledger:items", "urn:iki:ledger:default:items"] {
        let listing = source(&kernel, iri, &[]);
        assert!(
            listing.contains("Through the short form"),
            "{iri}: {listing}"
        );
        assert!(
            listing.contains("Through the long form"),
            "{iri}: {listing}"
        );
    }

    // ...and an item resolves under either spelling of its IRI, because a reference that
    // resolves in one position and not the other is worse than no sugar at all.
    for iri in [
        "urn:iki:ledger:item:1",
        "urn:iki:ledger:default:item:1",
        "urn:iki:ledger:acme:item:1",
    ] {
        let read = try_verb(&kernel, Verb::Source, iri, &[]);
        // The third one is a DIFFERENT ledger and is empty, so only it fails.
        assert_eq!(read.is_ok(), !iri.contains("acme"), "{iri}: {read:?}");
    }
}

/// Each ledger's recoverable deletes go to its OWN graveyard. A shared one would be a
/// graph two ledgers' deletes both write — a path across the boundary the rest of this
/// is built to close.
#[test]
fn a_delete_quarantines_into_the_ledgers_own_graveyard() {
    let kernel = kernel();
    let iri = sink(
        &kernel,
        "urn:iki:ledger:acme:append",
        &[("content", "Filed then deleted")],
    )
    .split_whitespace()
    .nth(1)
    .expect("append answers with the number and the IRI")
    .to_string();

    delete(
        &kernel,
        "urn:iki:ledger:acme:item:1",
        &[("reason", "a mistake")],
    );

    let quarantined = select(
        &kernel,
        &format!(
            "SELECT ?o WHERE {{ GRAPH <urn:iki:ledger:graph:acme:deleted> {{ \
             <{iri}> <http://purl.org/dc/terms/title> ?o }} }}"
        ),
    );
    assert!(quarantined.contains("Filed then deleted"), "{quarantined}");

    // The tombstone stays in acme's LIVE graph, not in anyone else's.
    let tombstone = select(
        &kernel,
        "SELECT ?t WHERE { GRAPH <urn:iki:ledger:graph:acme> { \
         ?t a <https://ikigai-rs.dev/ns/ledger#Tombstone> } }",
    );
    assert!(
        tombstone.contains("urn:iki:ledger:acme:tombstone:"),
        "{tombstone}"
    );
}

/// Selection is per ledger, which the named IRIs give for nothing: `next` in one ledger
/// never offers work from another.
#[test]
fn selection_is_per_ledger() {
    let kernel = kernel();
    sink(
        &kernel,
        "urn:iki:ledger:acme:append",
        &[("content", "Acme work"), ("priority", "0")],
    );
    sink(
        &kernel,
        "urn:iki:ledger:bosatsu:append",
        &[("content", "Bosatsu work"), ("priority", "0")],
    );

    let next = source(&kernel, "urn:iki:ledger:acme:next", &[]);
    assert!(next.contains("Acme work"), "{next}");
    assert!(!next.contains("Bosatsu work"), "{next}");
    assert!(next.contains("acme#1"), "{next}");
    assert!(next.contains("ready: 1"), "{next}");
}

// ------------------------------------------------------------------------ authority

/// ★ A grant names ONE ledger. Holding it opens that ledger and is refused at the next
/// one — which is the difference between a partition and a boundary.
#[test]
fn a_grant_for_one_ledger_is_refused_at_another() {
    let kernel = kernel();
    let acme = grants_for("acme");

    let filed = try_as(
        &kernel,
        &acme,
        Verb::Sink,
        "urn:iki:ledger:acme:append",
        &[("content", "Allowed")],
    );
    assert!(filed.is_ok(), "{filed:?}");

    for (verb, iri, args) in [
        (
            Verb::Sink,
            "urn:iki:ledger:bosatsu:append",
            &[("content", "Refused")][..],
        ),
        (Verb::Source, "urn:iki:ledger:bosatsu:items", &[][..]),
        (Verb::Source, "urn:iki:ledger:bosatsu:next", &[][..]),
        // ...including the bare form, which is the ledger `default` and not a bypass.
        (
            Verb::Sink,
            "urn:iki:ledger:append",
            &[("content", "Refused")][..],
        ),
        (Verb::Source, "urn:iki:ledger:items", &[][..]),
    ] {
        let refused = try_as(&kernel, &acme, verb, iri, args);
        assert!(
            matches!(refused, Err(Error::Denied(_))),
            "{verb:?} {iri} should be denied: {refused:?}"
        );
    }
}

/// The four authorities are separate per ledger too: a write grant over `acme` is not a
/// delete grant over `acme`, and a delete grant is not a purge grant.
#[test]
fn write_delete_and_purge_stay_separate_within_one_ledger() {
    let kernel = kernel();
    let writer = Capability::scoped([
        "urn:cap:ledger:read:acme",
        "urn:cap:ledger:write:acme",
        "urn:cap:store:read",
        "urn:cap:store:write",
    ]);
    assert!(try_as(
        &kernel,
        &writer,
        Verb::Sink,
        "urn:iki:ledger:acme:append",
        &[("content", "Filed")]
    )
    .is_ok());

    for (verb, iri, args) in [
        (Verb::Delete, "urn:iki:ledger:acme:item:1", &[][..]),
        (
            Verb::Delete,
            "urn:iki:ledger:acme:purge",
            &[("content", "1")][..],
        ),
    ] {
        let refused = try_as(&kernel, &writer, verb, iri, args);
        assert!(
            matches!(refused, Err(Error::Denied(_))),
            "{verb:?} {iri}: {refused:?}"
        );
    }
}

/// ⚠ **The half that is NOT enforced, pinned as a test rather than left as a sentence.**
///
/// `urn:cap:store:read` is the whole dataset — there is no per-graph read scope in
/// `ikigai-store` yet — so a caller granted one ledger through this module can still read
/// every other ledger's graph by going to `urn:iki:store:select` directly. When the store
/// grows `urn:cap:store:read:graph:<iri>`, this test is the one that should fail.
#[test]
fn a_store_read_grant_still_sees_every_ledger() {
    let kernel = kernel();
    sink(
        &kernel,
        "urn:iki:ledger:bosatsu:append",
        &[("content", "Another client's work")],
    );

    let acme_only = grants_for("acme");
    // Through the ledger: refused, as it should be.
    assert!(matches!(
        try_as(
            &kernel,
            &acme_only,
            Verb::Source,
            "urn:iki:ledger:bosatsu:items",
            &[]
        ),
        Err(Error::Denied(_))
    ));

    // Around it: visible, because the store's read scope is not per graph.
    let leaked = try_as(
        &kernel,
        &acme_only,
        Verb::Source,
        "urn:iki:store:select",
        &[(
            "query",
            "SELECT ?t WHERE { GRAPH <urn:iki:ledger:graph:bosatsu> { \
             ?i <http://purl.org/dc/terms/title> ?t } }",
        )],
    )
    .expect("the store's read scope is not per graph — see this test's doc comment");
    assert!(
        leaked.contains("Another client's work"),
        "if this assertion has started failing, the store grew a per-graph read scope \
         and this crate's README must stop saying it has not: {leaked}"
    );
}

// -------------------------------------------------------------- names and boundaries

/// A ledger name that is reserved, or that could forge a capability token, is refused
/// with a sentence — not resolved as something else.
#[test]
fn an_unusable_ledger_name_is_refused_where_it_is_used() {
    let kernel = kernel();
    // `Acme` is not `acme`: case is not a distinction to rely on in a token matched
    // exactly, so it is refused rather than folded.
    let refused = try_verb(
        &kernel,
        Verb::Sink,
        "urn:iki:ledger:Acme:append",
        &[("content", "nope")],
    );
    let message = refused.expect_err("an uppercase name").to_string();
    assert!(message.contains("lowercase"), "{message}");

    // A reserved word resolves as the DEFAULT ledger's resource of that name, which is
    // what reserving it means — `urn:iki:ledger:next` is the ready set, never a ledger
    // called `next`.
    let ready = source(&kernel, "urn:iki:ledger:next", &[]);
    assert!(ready.contains("policy:"), "{ready}");
}

/// ★ **A `blocks` edge cannot cross a ledger, and the refusal says why.**
///
/// Decided rather than left emergent. A block is not a reference: it changes the OTHER
/// ledger's ready set, so a caller granted one ledger could make work in another
/// unschedulable, and the blocked ledger's `next` would have to either name an item its
/// reader cannot see or say "something blocks you" — which leaks one bit and helps
/// nobody. `ledger:about` is the edge that crosses, because it names a resource rather
/// than asserting a membership.
#[test]
fn a_blocks_edge_cannot_cross_a_ledger_boundary() {
    let kernel = kernel();
    sink(
        &kernel,
        "urn:iki:ledger:acme:append",
        &[("content", "Acme's blocker")],
    );
    let elsewhere = sink(
        &kernel,
        "urn:iki:ledger:bosatsu:append",
        &[("content", "Bosatsu's work")],
    )
    .split_whitespace()
    .nth(1)
    .expect("append answers with the number and the IRI")
    .to_string();

    let refused = try_verb(
        &kernel,
        Verb::Sink,
        "urn:iki:ledger:acme:link",
        &[
            ("item", "acme#1"),
            ("content", &elsewhere),
            ("type", "blocks"),
        ],
    );
    let message = refused.expect_err("a cross-ledger link").to_string();
    assert!(message.contains("bosatsu"), "{message}");
    assert!(message.contains("ledger:about"), "{message}");

    // ...and `about` really is the way across: it names a resource, not a membership,
    // so it is accepted and changes nobody's ready set.
    sink(
        &kernel,
        "urn:iki:ledger:acme:item:1",
        &[("about", &elsewhere)],
    );
    let detail = source(&kernel, "urn:iki:ledger:acme:item:1", &[]);
    assert!(detail.contains(&elsewhere), "{detail}");
    let next = source(&kernel, "urn:iki:ledger:bosatsu:next", &[]);
    assert!(next.contains("ready: 1"), "{next}");
}

/// A qualified number is checked against the ledger it is used in, not looked up blindly:
/// numbers restart per ledger, so `acme#1` and `#1` are different items.
#[test]
fn a_number_qualified_with_another_ledger_is_refused() {
    let kernel = kernel();
    append(&kernel, "The default ledger's first", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:acme:append",
        &[("content", "Acme's first")],
    );

    // In its own ledger the qualified form works.
    let commented = sink(
        &kernel,
        "urn:iki:ledger:acme:comment",
        &[("item", "acme#1"), ("content", "fine")],
    );
    assert!(commented.contains("acme#1"), "{commented}");

    // In another one it is refused rather than resolved to that ledger's #1.
    let refused = try_verb(
        &kernel,
        Verb::Sink,
        "urn:iki:ledger:comment",
        &[("item", "acme#1"), ("content", "wrong ledger")],
    );
    let message = refused.expect_err("a foreign number").to_string();
    assert!(message.contains("acme"), "{message}");
    assert!(message.contains("restart"), "{message}");
}

// ------------------------------------------------------------------------ inventory

/// ★ A grammar does not enumerate, so without this resource a caller who was not TOLD a
/// ledger's name could not find one — which would make the partition a secret rather
/// than a boundary. And the listing is filtered by the caller's own grants, so it never
/// hands a one-ledger agent the client list.
#[test]
fn the_inventory_lists_only_what_this_capability_may_read() {
    let kernel = kernel();
    append(&kernel, "In the default ledger", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:acme:append",
        &[("content", "In acme")],
    );
    sink(
        &kernel,
        "urn:iki:ledger:bosatsu:append",
        &[("content", "In bosatsu")],
    );
    sink(
        &kernel,
        "urn:iki:ledger:bosatsu:close",
        &[("item", "bosatsu#1")],
    );

    let all = source(&kernel, "urn:iki:ledger:ledgers", &[]);
    for name in ["default", "acme", "bosatsu"] {
        assert!(all.contains(name), "{all}");
    }
    assert!(all.contains("3 ledger(s)"), "{all}");
    // Closed items still count toward the total and not toward open.
    assert!(all.contains("0 open      1 total"), "{all}");

    let acme_only = try_as(
        &kernel,
        &grants_for("acme"),
        Verb::Source,
        "urn:iki:ledger:ledgers",
        &[],
    )
    .expect("the inventory is readable under any ledger read grant");
    assert!(acme_only.contains("acme"), "{acme_only}");
    assert!(!acme_only.contains("bosatsu"), "{acme_only}");
    assert!(acme_only.contains("1 ledger(s)"), "{acme_only}");

    // The graph face names each ledger's graph, so a consumer can go straight to
    // `urn:iki:store:select` over one ledger without rebuilding the IRI from the name.
    let turtle = source(&kernel, "urn:iki:ledger:ledgers", &[("as", "text/turtle")]);
    assert!(turtle.contains("urn:iki:ledger:graph:acme"), "{turtle}");
    assert!(!turtle.contains("_:"), "no blank nodes: {turtle}");
}
