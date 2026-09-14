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
//! # ★ And since 0.2.0 the authority half closes
//!
//! [`grants_for`] is the whole grant list a caller working in one ledger needs, and every
//! token in it names that ledger: four at this module's doors, three at the store's. There
//! is no `urn:cap:store:read` and no `urn:cap:store:write` in it, because nothing in this
//! crate resolves a broad store door — so
//! `a_caller_granted_one_ledger_cannot_reach_another_by_any_route` is now the property and
//! not the gap.
//!
//! ⚠ **What it does not claim.** A *host* that hands a ledger caller the broad
//! `urn:cap:store:read` anyway has given it every graph in the store, and the store is
//! right to answer — that grant means the whole dataset and always did.
//! `a_host_that_hands_out_the_broad_store_grant_still_has_a_bypass` pins that too, in the
//! same file, because the two facts are only useful together: the substrate hole is closed
//! and a configuration can still open one. Nothing this crate declares asks for the broad
//! grant, which is what makes handing it out a decision rather than a requirement.

mod common;

use common::*;
use ikigai_core::{Capability, Error, Verb};

/// Every grant a caller working in ONE ledger needs — all seven, and every one of them
/// names that ledger.
///
/// ★ Read it as the operator's config line, because that is what it is. Four tokens at
/// this module's doors and three at the store's: the ledger's graph must be readable (to
/// resolve `#12`, to check an item exists, to list) and writable, and its **graveyard**
/// must be writable, because a delete archives into a second graph and a scoped write
/// cannot reach across. A caller that only reads needs the first and the fifth.
fn grants_for(ledger: &str) -> Capability {
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
        "urn:cap:ledger:read:acme".to_string(),
        "urn:cap:ledger:write:acme".to_string(),
        graph_read("acme"),
        graph_write("acme"),
        // ⚠ Deliberately granted: the graveyard write is what a delete would ALSO need,
        // and giving it here proves the refusals below come from this module's own
        // delete/purge grants and not from a missing store token further down.
        graveyard_write("acme"),
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

/// ★ **The one this arc exists for, and the one that used to assert the leak.**
///
/// Until 0.2.0 this file carried `a_store_read_grant_still_sees_every_ledger`, which
/// resolved `urn:iki:store:select` under a one-ledger grant and asserted that another
/// ledger's titles came back — with a message telling whoever made it fail to go fix the
/// README. `ikigai-store` 0.2.2 is what made it fail: the grant list a ledger caller needs
/// no longer contains `urn:cap:store:read` at all.
///
/// ⚠ **Be exact about what is proved here.** Not "the data is unreachable" — a store
/// holds what a store holds. What is proved is that **the grants this crate's actions
/// declare are sufficient to use one ledger and insufficient to reach any other**, by any
/// route this crate offers or composes over. The caller below holds every token
/// [`grants_for`] lists for `acme`, tries eight doors at `bosatsu` — four of this module's
/// and four of the store's, broad and narrow — and is refused at all eight.
#[test]
fn a_caller_granted_one_ledger_cannot_reach_another_by_any_route() {
    let kernel = kernel();
    sink(
        &kernel,
        "urn:iki:ledger:bosatsu:append",
        &[("content", "Another client's work")],
    );
    let acme_only = grants_for("acme");
    let title_query = "SELECT ?t WHERE { GRAPH <urn:iki:ledger:graph:bosatsu> { \
                       ?i <http://purl.org/dc/terms/title> ?t } }";

    for (what, verb, iri, args) in [
        // Through this module: the ledger's own grants.
        (
            "the other ledger's listing",
            Verb::Source,
            "urn:iki:ledger:bosatsu:items",
            &[][..],
        ),
        (
            "the other ledger's ready set",
            Verb::Source,
            "urn:iki:ledger:bosatsu:next",
            &[][..],
        ),
        (
            "filing into the other ledger",
            Verb::Sink,
            "urn:iki:ledger:bosatsu:append",
            &[("content", "Refused")][..],
        ),
        // Around it, at the store's BROAD doors: not held, because nothing here asks for
        // them. This is the assertion that used to run the other way.
        (
            "the broad read door",
            Verb::Source,
            "urn:iki:store:select",
            &[("query", title_query)][..],
        ),
        (
            "the broad write door",
            Verb::Sink,
            "urn:iki:store:update",
            &[("content", "DROP GRAPH <urn:iki:ledger:graph:bosatsu>")][..],
        ),
        // Around it, at the store's NARROW doors, naming the other ledger's graphs: the
        // grant is exact, so a token for `acme` is not a token for `bosatsu`, and nothing
        // is a prefix of anything.
        (
            "the narrow read door aimed elsewhere",
            Verb::Source,
            "urn:iki:store:graph-select",
            &[
                ("graph", "urn:iki:ledger:graph:bosatsu"),
                ("query", title_query),
            ][..],
        ),
        (
            "the narrow write door aimed elsewhere",
            Verb::Sink,
            "urn:iki:store:graph-update",
            &[
                ("graph", "urn:iki:ledger:graph:bosatsu"),
                ("content", "DROP GRAPH <urn:iki:ledger:graph:bosatsu>"),
            ][..],
        ),
        (
            "the other ledger's graveyard",
            Verb::Sink,
            "urn:iki:store:graph-update",
            &[
                ("graph", "urn:iki:ledger:graph:bosatsu:deleted"),
                (
                    "content",
                    "DROP GRAPH <urn:iki:ledger:graph:bosatsu:deleted>",
                ),
            ][..],
        ),
    ] {
        let refused = try_as(&kernel, &acme_only, verb, iri, args);
        assert!(
            matches!(refused, Err(Error::Denied(_))),
            "{what} ({verb:?} {iri}) should be denied: {refused:?}"
        );
    }

    // And the same capability still does its own job — a boundary that also broke the
    // work it fences would prove nothing.
    let filed = try_as(
        &kernel,
        &acme_only,
        Verb::Sink,
        "urn:iki:ledger:acme:append",
        &[("content", "Allowed")],
    );
    assert!(filed.is_ok(), "{filed:?}");
    let listing = try_as(
        &kernel,
        &acme_only,
        Verb::Source,
        "urn:iki:ledger:acme:items",
        &[],
    )
    .expect("its own ledger");
    assert!(listing.contains("Allowed"), "{listing}");
    assert!(!listing.contains("Another client's work"), "{listing}");
}

/// ⚠ **What the boundary above does NOT cover, pinned in the same file so the pair is
/// read together.**
///
/// The store's broad read door means what it has always meant: the whole dataset. A host
/// that grants `urn:cap:store:read` to a ledger caller — out of habit, or to make some
/// other module work — has handed it every ledger in the store, and no amount of care in
/// this crate can take that back.
///
/// The difference 0.2.2 makes is that this is now a **host-configuration decision** rather
/// than a **substrate hole**. Nothing this crate declares requires the broad grant; a host
/// reading the manifold sees `urn:cap:store:{read,write}:graph:*` on every action and has
/// no reason to issue the broad one. The bypass survives the decision to ignore that,
/// which is a different and much smaller claim than the one this file used to make.
#[test]
fn a_host_that_hands_out_the_broad_store_grant_still_has_a_bypass() {
    let kernel = kernel();
    sink(
        &kernel,
        "urn:iki:ledger:bosatsu:append",
        &[("content", "Another client's work")],
    );

    let mut tokens: Vec<String> = grants_for("acme")
        .scopes()
        .expect("a scoped capability")
        .iter()
        .cloned()
        .collect();
    tokens.push("urn:cap:store:read".to_string());
    let over_granted = Capability::scoped(tokens);

    // This module still refuses: its own grants are exact and unaffected.
    assert!(matches!(
        try_as(
            &kernel,
            &over_granted,
            Verb::Source,
            "urn:iki:ledger:bosatsu:items",
            &[]
        ),
        Err(Error::Denied(_))
    ));

    // The store answers, because it was asked by something holding the whole dataset.
    let leaked = try_as(
        &kernel,
        &over_granted,
        Verb::Source,
        "urn:iki:store:select",
        &[(
            "query",
            "SELECT ?t WHERE { GRAPH <urn:iki:ledger:graph:bosatsu> { \
             ?i <http://purl.org/dc/terms/title> ?t } }",
        )],
    )
    .expect("the broad read grant is the whole dataset, by definition");
    assert!(
        leaked.contains("Another client's work"),
        "if this has started failing, `urn:cap:store:read` narrowed and the README's \
         paragraph about host configuration needs rewriting: {leaked}"
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
    // `urn:iki:store:graph-select` over one ledger without rebuilding the IRI from the
    // name — which is the IRI its read grant names, too.
    let turtle = source(&kernel, "urn:iki:ledger:ledgers", &[("as", "text/turtle")]);
    assert!(turtle.contains("urn:iki:ledger:graph:acme"), "{turtle}");
    assert!(!turtle.contains("_:"), "no blank nodes: {turtle}");
}

/// ★ **The inventory is the one resource no scoped read can answer**, so it takes two
/// paths — the capability's own grants when it has any, the whole store under root — and
/// they must agree. Root's answer above and a scoped answer here are the same listing for
/// the same ledger, counts included.
#[test]
fn the_inventory_agrees_between_the_root_path_and_the_scoped_path() {
    let kernel = kernel();
    sink(
        &kernel,
        "urn:iki:ledger:acme:append",
        &[("content", "Open work")],
    );
    sink(
        &kernel,
        "urn:iki:ledger:acme:append",
        &[("content", "Finished work")],
    );
    sink(&kernel, "urn:iki:ledger:acme:close", &[("item", "acme#2")]);
    sink(
        &kernel,
        "urn:iki:ledger:bosatsu:append",
        &[("content", "Elsewhere")],
    );

    let root = source(&kernel, "urn:iki:ledger:ledgers", &[]);
    assert!(root.contains("acme") && root.contains("bosatsu"), "{root}");

    let scoped = try_as(
        &kernel,
        &grants_for("acme"),
        Verb::Source,
        "urn:iki:ledger:ledgers",
        &[],
    )
    .expect("the inventory under a scoped grant");
    // The same row root produced for `acme`, arrived at without the store ever seeing a
    // query that crosses graphs.
    let row = root
        .lines()
        .find(|line| line.starts_with("acme "))
        .expect("acme's row under root");
    assert!(
        scoped.contains(row),
        "root said `{row}`, scoped said {scoped}"
    );
    assert!(!scoped.contains("bosatsu"), "{scoped}");
}

/// ⚠ Two grants per ledger per direction is the shape an operator will get half right, and
/// a ledger granted at this module but not at the store **stops the listing** rather than
/// thinning it — because a missing row is indistinguishable from an empty ledger, and a
/// wrong answer that looks right is the worst available one. The refusal names the exact
/// token to add.
#[test]
fn a_ledger_granted_here_but_not_in_the_store_refuses_the_listing() {
    let kernel = kernel();
    sink(
        &kernel,
        "urn:iki:ledger:acme:append",
        &[("content", "Acme work")],
    );
    sink(
        &kernel,
        "urn:iki:ledger:bosatsu:append",
        &[("content", "Bosatsu work")],
    );

    let half_granted = Capability::scoped([
        "urn:cap:ledger:read:acme".to_string(),
        graph_read("acme"),
        // `bosatsu` is granted here and nowhere else: the config error this catches.
        "urn:cap:ledger:read:bosatsu".to_string(),
    ]);
    let refused = try_as(
        &kernel,
        &half_granted,
        Verb::Source,
        "urn:iki:ledger:ledgers",
        &[],
    );
    let message = match refused {
        Err(Error::Denied(message)) => message,
        other => panic!("a half-granted ledger must stop the listing: {other:?}"),
    };
    assert!(message.contains("bosatsu"), "{message}");
    assert!(
        message.contains(&graph_read("bosatsu")),
        "…naming the exact token to add: {message}"
    );
    // …and the graveyard token, which is the half of the fix an operator would otherwise
    // discover on their first delete.
    assert!(message.contains(":deleted"), "{message}");

    // Grant the missing half and the same call answers.
    let whole = Capability::scoped([
        "urn:cap:ledger:read:acme".to_string(),
        graph_read("acme"),
        "urn:cap:ledger:read:bosatsu".to_string(),
        graph_read("bosatsu"),
    ]);
    let listing = try_as(&kernel, &whole, Verb::Source, "urn:iki:ledger:ledgers", &[])
        .expect("both halves granted");
    assert!(listing.contains("2 ledger(s)"), "{listing}");
}
