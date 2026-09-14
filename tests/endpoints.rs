//! The five verbs Brian asked for, over the real composition: view, query, append,
//! delete, comment — plus the ones that make a ready set mean anything.

mod common;

use common::*;
use ikigai_core::{Capability, Error, Verb};

#[test]
fn append_then_view_then_query() {
    let kernel = kernel();
    let first = append(
        &kernel,
        "Fix the thing\n\nIt is broken.",
        &[("author", "brian")],
    );
    assert_eq!(first, "#1", "the first item is #1");
    let second = append(&kernel, "Ship the other thing", &[("priority", "1")]);
    assert_eq!(second, "#2");

    // VIEW: the list.
    let list = source(&kernel, "urn:iki:ledger:items", &[]);
    assert!(list.contains("#1"), "{list}");
    assert!(list.contains("Fix the thing"), "{list}");
    assert!(list.contains("2 item(s)"), "{list}");

    // VIEW: one item, with its body and metadata.
    let item = source(&kernel, "urn:iki:ledger:item:1", &[]);
    assert!(item.contains("It is broken."), "{item}");
    assert!(item.contains("by brian"), "{item}");
    // The CANONICAL IRI: an item filed through the bare sugar is still minted in the
    // ledger the sugar names, because the data has to say which ledger it is in even
    // when the request did not.
    assert!(item.contains("urn:iki:ledger:default:item:"), "{item}");

    // QUERY: through the store's own face, which is the whole point of not binding a
    // second query surface.
    let answer = select(
        &kernel,
        "SELECT ?t WHERE { GRAPH <urn:iki:ledger:graph:default> { \
         ?i <https://ikigai-rs.dev/ns/ledger#priority> 1 ; \
         <http://purl.org/dc/terms/title> ?t } }",
    );
    assert!(answer.contains("Ship the other thing"), "{answer}");
}

#[test]
fn the_short_id_is_a_label_and_the_iri_is_the_identity() {
    let kernel = kernel();
    let iri = append_iri(&kernel, "An item", &[]);
    // Both address the same resource.
    let by_number = source(&kernel, "urn:iki:ledger:item:1", &[]);
    let by_id = source(&kernel, &iri, &[]);
    assert_eq!(by_number, by_id);
    // And the IRI carries no number: renumbering could never invalidate it.
    assert!(!iri.contains(":1"), "{iri} should not encode the short id");
}

#[test]
fn a_comment_is_appended_stamped_and_attributed() {
    let kernel = kernel();
    append(&kernel, "An item", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:comment",
        &[
            ("item", "#1"),
            ("content", "Looked at it"),
            ("author", "brian"),
        ],
    );
    sink(
        &kernel,
        "urn:iki:ledger:comment",
        &[("item", "#1"), ("content", "Still broken")],
    );
    let view = source(&kernel, "urn:iki:ledger:item:1", &[]);
    assert!(view.contains("2 comment(s)"), "{view}");
    assert!(view.contains("Looked at it"), "{view}");
    assert!(view.contains("brian"), "{view}");
    assert!(view.contains("(unattributed)"), "{view}");
    // Oldest first: a log is read forwards.
    let first = view.find("Looked at it").unwrap();
    let second = view.find("Still broken").unwrap();
    assert!(first < second, "{view}");
}

#[test]
fn closing_is_not_deleting() {
    let kernel = kernel();
    append(&kernel, "An item", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:close",
        &[
            ("item", "#1"),
            ("reason", "audit-no-change"),
            ("content", "Looked, found it fine"),
        ],
    );
    // Still there, still queryable, and the verdict is recorded.
    let view = source(&kernel, "urn:iki:ledger:item:1", &[]);
    assert!(view.contains("closed"), "{view}");
    assert!(view.contains("audit-no-change"), "{view}");
    assert!(view.contains("Looked, found it fine"), "{view}");
    // …and out of the default (open) listing.
    let open = source(&kernel, "urn:iki:ledger:items", &[]);
    assert!(open.contains("no items match"), "{open}");
    let all = source(&kernel, "urn:iki:ledger:items", &[("status", "all")]);
    assert!(all.contains("#1"), "{all}");

    sink(&kernel, "urn:iki:ledger:reopen", &[("item", "#1")]);
    let reopened = source(&kernel, "urn:iki:ledger:item:1", &[]);
    assert!(reopened.contains("open"), "{reopened}");
    assert!(!reopened.contains("(audit-no-change)"), "{reopened}");
}

#[test]
fn an_unknown_close_reason_is_refused() {
    let kernel = kernel();
    append(&kernel, "An item", &[]);
    let refused = try_verb(
        &kernel,
        Verb::Sink,
        "urn:iki:ledger:close",
        &[("item", "#1"), ("reason", "because")],
    );
    assert!(
        matches!(refused, Err(Error::InvalidArgument { ref name, .. }) if name == "reason"),
        "{refused:?}"
    );
}

#[test]
fn a_claim_is_a_fence_and_stealing_one_is_refused() {
    let kernel = kernel();
    append(&kernel, "An item", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:claim",
        &[
            ("item", "#1"),
            ("content", "session-a"),
            ("purpose", "the arc"),
        ],
    );
    let view = source(&kernel, "urn:iki:ledger:item:1", &[]);
    assert!(view.contains("claimed:session-a"), "{view}");
    assert!(view.contains("the arc"), "{view}");

    let stolen = try_verb(
        &kernel,
        Verb::Sink,
        "urn:iki:ledger:claim",
        &[("item", "#1"), ("content", "session-b")],
    );
    match stolen {
        Err(Error::Unavailable(message)) => {
            assert!(message.contains("session-a"), "{message}");
        }
        other => panic!("a held claim must be refused, naming the holder: {other:?}"),
    }

    // The holder re-claiming is not a theft.
    sink(
        &kernel,
        "urn:iki:ledger:claim",
        &[("item", "#1"), ("content", "session-a")],
    );
    delete(&kernel, "urn:iki:ledger:claim", &[("item", "#1")]);
    let released = source(&kernel, "urn:iki:ledger:item:1", &[]);
    assert!(!released.contains("claimed:"), "{released}");
}

#[test]
fn labels_and_links_are_added_and_removed() {
    let kernel = kernel();
    append(&kernel, "Blocker", &[]);
    append(&kernel, "Blocked", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:label",
        &[("item", "#1"), ("content", "rust")],
    );
    sink(
        &kernel,
        "urn:iki:ledger:link",
        &[("item", "#1"), ("content", "#2"), ("type", "blocks")],
    );
    let view = source(&kernel, "urn:iki:ledger:item:1", &[]);
    assert!(view.contains("[rust]"), "{view}");
    assert!(view.contains("blocks:"), "{view}");

    let filtered = source(&kernel, "urn:iki:ledger:items", &[("labels", "rust")]);
    assert!(
        filtered.contains("Blocker") && !filtered.contains("Blocked"),
        "{filtered}"
    );
    let excluded = source(&kernel, "urn:iki:ledger:items", &[("without", "rust")]);
    assert!(
        excluded.contains("Blocked") && !excluded.contains("Blocker"),
        "{excluded}"
    );

    delete(
        &kernel,
        "urn:iki:ledger:label",
        &[("item", "#1"), ("content", "rust")],
    );
    delete(
        &kernel,
        "urn:iki:ledger:link",
        &[("item", "#1"), ("content", "#2"), ("type", "blocks")],
    );
    let bare = source(&kernel, "urn:iki:ledger:item:1", &[]);
    assert!(!bare.contains("[rust]"), "{bare}");
    assert!(!bare.contains("blocks:"), "{bare}");
}

#[test]
fn an_item_cannot_block_itself() {
    let kernel = kernel();
    append(&kernel, "An item", &[]);
    let refused = try_verb(
        &kernel,
        Verb::Sink,
        "urn:iki:ledger:link",
        &[("item", "#1"), ("content", "#1"), ("type", "blocks")],
    );
    assert!(refused.is_err(), "a self-block is a cycle: {refused:?}");
}

#[test]
fn about_makes_what_is_open_against_this_file_a_query() {
    let kernel = kernel();
    append(
        &kernel,
        "The enumerator outruns the resolver",
        &[
            ("about", "urn:repo:file:ikigai-browse/src/tree.rs"),
            ("revision", "f41a333"),
        ],
    );
    append(&kernel, "Unrelated", &[]);
    let against = source(
        &kernel,
        "urn:iki:ledger:items",
        &[("about", "urn:repo:file:ikigai-browse/src/tree.rs")],
    );
    assert!(against.contains("enumerator"), "{against}");
    assert!(!against.contains("Unrelated"), "{against}");

    let view = source(&kernel, "urn:iki:ledger:item:1", &[]);
    assert!(view.contains("revision: f41a333"), "{view}");
}

#[test]
fn editing_replaces_fields_and_stamps_the_item() {
    let kernel = kernel();
    append(&kernel, "Wrong title", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:item:1",
        &[("content", "Right title\n\nAnd a body."), ("priority", "0")],
    );
    let view = source(&kernel, "urn:iki:ledger:item:1", &[]);
    assert!(view.contains("Right title"), "{view}");
    assert!(!view.contains("Wrong title"), "{view}");
    assert!(view.contains("p0"), "{view}");
    assert!(view.contains("And a body."), "{view}");
}

#[test]
fn deferring_takes_an_item_out_of_the_ready_set_without_closing_it() {
    let kernel = kernel();
    append(&kernel, "Someday", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:defer",
        &[("item", "#1"), ("content", "not this quarter")],
    );
    let view = source(&kernel, "urn:iki:ledger:item:1", &[]);
    assert!(view.contains("deferred"), "{view}");
    assert!(view.contains("open"), "{view}");
    let next = source(&kernel, "urn:iki:ledger:next", &[]);
    assert!(next.contains("deferred"), "{next}");
    assert!(next.contains("nothing is ready"), "{next}");

    delete(&kernel, "urn:iki:ledger:defer", &[("item", "#1")]);
    let next = source(&kernel, "urn:iki:ledger:next", &[]);
    assert!(next.contains("Someday"), "{next}");
}

// ------------------------------------------------------------------ delete and purge

#[test]
fn a_delete_leaves_a_tombstone_and_the_content_is_recoverable() {
    let kernel = kernel();
    let iri = append_iri(&kernel, "Filed by mistake\n\nOops.", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:comment",
        &[("item", "#1"), ("content", "a comment that goes with it")],
    );
    let answer = delete(
        &kernel,
        "urn:iki:ledger:item:1",
        &[("reason", "filed by mistake"), ("author", "brian")],
    );
    assert!(answer.contains("recoverable"), "{answer}");

    // Gone from every ledger read.
    let list = source(&kernel, "urn:iki:ledger:items", &[("status", "all")]);
    assert!(list.contains("no items match"), "{list}");
    assert!(matches!(
        try_verb(&kernel, Verb::Source, "urn:iki:ledger:item:1", &[]),
        Err(Error::NotFound(_))
    ));

    // The tombstone says what went, and it is queryable.
    let tombstone = select(
        &kernel,
        "SELECT ?hash ?quads ?reason ?recoverable WHERE { GRAPH <urn:iki:ledger:graph:default> { \
         ?t a <https://ikigai-rs.dev/ns/ledger#Tombstone> ; \
         <https://ikigai-rs.dev/ns/sign#contentHash> ?hash ; \
         <https://ikigai-rs.dev/ns/ledger#quadCount> ?quads ; \
         <https://ikigai-rs.dev/ns/ledger#reason> ?reason ; \
         <https://ikigai-rs.dev/ns/ledger#recoverable> ?recoverable } }",
    );
    assert!(tombstone.contains("sha256:"), "{tombstone}");
    assert!(tombstone.contains("filed by mistake"), "{tombstone}");
    assert!(tombstone.contains("true"), "{tombstone}");

    // …and the content really is still there, in quarantine, comment included.
    let quarantined = select(
        &kernel,
        &format!(
            "SELECT ?p ?o WHERE {{ GRAPH <urn:iki:ledger:graph:default:deleted> {{ <{iri}> ?p ?o }} }}"
        ),
    );
    assert!(quarantined.contains("Filed by mistake"), "{quarantined}");
    let comments = select(
        &kernel,
        &format!(
            "SELECT ?c WHERE {{ GRAPH <urn:iki:ledger:graph:default:deleted> {{ \
             ?c <https://ikigai-rs.dev/ns/ledger#onItem> <{iri}> }} }}"
        ),
    );
    assert!(
        comments.contains("urn:iki:ledger:default:comment:"),
        "{comments}"
    );
}

/// ★ **A delete now re-serializes the quads it archives, so every term shape has to
/// survive the trip.** Before 0.2.0 the move was one `DELETE … INSERT … WHERE` and the
/// store never handed a term back to this crate; a scoped update cannot span two graphs,
/// so the quads go out through a SELECT and back in as `INSERT DATA`, and anything this
/// crate cannot round-trip is quietly changed at exactly the moment it is least
/// recoverable.
///
/// This ledger writes no language tags — but an out-of-band write is a **supported path**
/// here (see the README), so a hand-edited `"titre"@fr` is a real case and the archive
/// must keep the tag rather than flattening it to a plain literal, which is a different
/// statement.
#[test]
fn a_term_this_crate_never_writes_survives_being_archived() {
    let kernel = kernel();
    let iri = append_iri(&kernel, "Filed by hand", &[]);

    // The editor, the merge, the bulk load — whatever wrote it, it did not come through a
    // ledger Sink.
    sink(
        &kernel,
        "urn:iki:store:graph-update",
        &[
            ("graph", "urn:iki:ledger:graph:default"),
            (
                "content",
                &format!(
                    "INSERT DATA {{ GRAPH <urn:iki:ledger:graph:default> {{ \
                     <{iri}> <https://ikigai-rs.dev/ns/ledger#label> \"étiquette\"@fr }} }}"
                ),
            ),
        ],
    );

    delete(&kernel, "urn:iki:ledger:item:1", &[("reason", "done")]);

    let archived = select(
        &kernel,
        &format!(
            "SELECT ?o WHERE {{ GRAPH <urn:iki:ledger:graph:default:deleted> {{ \
             <{iri}> <https://ikigai-rs.dev/ns/ledger#label> ?o \
             FILTER(LANG(?o) = \"fr\") }} }}"
        ),
    );
    assert!(
        archived.contains("étiquette"),
        "the language tag did not survive the archive: {archived}"
    );
}

#[test]
fn a_purge_destroys_the_content_and_keeps_the_evidence() {
    let kernel = kernel();
    let iri = append_iri(&kernel, "Secret\n\nShould not survive.", &[]);
    let answer = delete(
        &kernel,
        "urn:iki:ledger:purge",
        &[("content", "#1"), ("reason", "filed in the wrong ledger")],
    );
    assert!(answer.contains("NOT recoverable"), "{answer}");

    let anywhere = select(
        &kernel,
        &format!("SELECT ?p ?o WHERE {{ GRAPH ?g {{ <{iri}> ?p ?o }} }}"),
    );
    assert!(!anywhere.contains("Should not survive"), "{anywhere}");
    assert!(!anywhere.contains("\"Secret\""), "{anywhere}");

    let tombstone = select(
        &kernel,
        "SELECT ?hash ?recoverable ?n WHERE { GRAPH <urn:iki:ledger:graph:default> { \
         ?t a <https://ikigai-rs.dev/ns/ledger#Tombstone> ; \
         <https://ikigai-rs.dev/ns/sign#contentHash> ?hash ; \
         <https://ikigai-rs.dev/ns/ledger#number> ?n ; \
         <https://ikigai-rs.dev/ns/ledger#recoverable> ?recoverable } }",
    );
    assert!(tombstone.contains("sha256:"), "{tombstone}");
    assert!(tombstone.contains("false"), "{tombstone}");
}

/// ★ A purge must not lower the counter: reusing `#1` would make two different pieces of
/// work share a name, and every reference written down before the purge would silently
/// point at the wrong one.
#[test]
fn a_number_is_never_reused_after_a_delete_or_a_purge() {
    let kernel = kernel();
    append(&kernel, "First", &[]);
    append(&kernel, "Second", &[]);
    delete(&kernel, "urn:iki:ledger:item:2", &[]);
    delete(&kernel, "urn:iki:ledger:purge", &[("content", "#1")]);
    assert_eq!(append(&kernel, "Third", &[]), "#3");
}

#[test]
fn deleting_an_item_takes_the_edges_pointing_at_it_too() {
    let kernel = kernel();
    let blocker = append_iri(&kernel, "Blocker", &[]);
    append(&kernel, "Blocked", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:link",
        &[("item", "#1"), ("content", "#2"), ("type", "blocks")],
    );
    delete(&kernel, "urn:iki:ledger:item:2", &[]);
    let dangling = select(
        &kernel,
        &format!(
            "SELECT ?o WHERE {{ GRAPH <urn:iki:ledger:graph:default> {{ \
             <{blocker}> <https://ikigai-rs.dev/ns/ledger#blocks> ?o }} }}"
        ),
    );
    assert!(
        !dangling.contains("urn:iki:ledger:item:"),
        "an edge pointing at a deleted item is a reference to nothing: {dangling}"
    );
}

// ------------------------------------------------------------------------- the faces

#[test]
fn the_turtle_face_is_a_graph_and_the_plain_face_is_a_list() {
    let kernel = kernel();
    append(
        &kernel,
        "An item\n\nWith a body.",
        &[("labels", "rust,docs"), ("about", "urn:repo:file:x.rs")],
    );
    let turtle = source(&kernel, "urn:iki:ledger:items", &[("as", "text/turtle")]);
    assert!(turtle.contains("ledger:Item"), "{turtle}");
    assert!(turtle.contains("urn:repo:file:x.rs"), "{turtle}");
    assert!(!turtle.contains("_:"), "no blank nodes: {turtle}");

    let refused = try_verb(
        &kernel,
        Verb::Source,
        "urn:iki:ledger:items",
        &[("as", "application/json")],
    );
    assert!(
        matches!(refused, Err(Error::InvalidArgument { ref name, .. }) if name == "as"),
        "an `as` this resource cannot serve is refused, never substituted: {refused:?}"
    );
}

// -------------------------------------------------------------- capabilities & safety

/// Declared = enforced, both ways: the ledger's own scope AND the store scope a
/// sub-request really needs — which is now a grant naming this ledger's graph, not the
/// whole dataset.
#[test]
fn a_read_capability_cannot_write_and_a_write_capability_cannot_purge() {
    let kernel = kernel();
    append(&kernel, "An item", &[]);

    let reader = Capability::scoped([
        "urn:cap:ledger:read:default".to_string(),
        graph_read("default"),
    ]);
    assert!(try_as(&kernel, &reader, Verb::Source, "urn:iki:ledger:items", &[]).is_ok());
    assert!(matches!(
        try_as(
            &kernel,
            &reader,
            Verb::Sink,
            "urn:iki:ledger:append",
            &[("content", "nope")]
        ),
        Err(Error::Denied(_))
    ));

    let writer = Capability::scoped([
        "urn:cap:ledger:write:default".to_string(),
        graph_read("default"),
        graph_write("default"),
    ]);
    assert!(try_as(
        &kernel,
        &writer,
        Verb::Sink,
        "urn:iki:ledger:append",
        &[("content", "fine")]
    )
    .is_ok());
    assert!(matches!(
        try_as(
            &kernel,
            &writer,
            Verb::Delete,
            "urn:iki:ledger:purge",
            &[("content", "#1")]
        ),
        Err(Error::Denied(_))
    ));
    assert!(matches!(
        try_as(&kernel, &writer, Verb::Delete, "urn:iki:ledger:item:1", &[]),
        Err(Error::Denied(_))
    ));
}

/// ⚠ The friction this composition really has: a ledger write needs the store's write
/// grant **for this ledger's graph**, and holding `urn:cap:ledger:write:default` alone is
/// not enough. Asserted rather than described, so the day it stops being true a test says
/// so.
///
/// ★ And the grant that is not enough is no longer the one that was too much. A holder of
/// the broad `urn:cap:store:write` — `DROP ALL` over every graph in the host — is refused
/// here too, because `urn:iki:store:graph-update` takes the per-graph grant and nothing
/// else. Declared and enforced are the same scope in both directions: the narrow door does
/// not accept the broad key, which is what keeps the *declaration* on this crate's actions
/// honest.
#[test]
fn a_ledger_write_also_needs_the_stores_write_scope_for_this_graph() {
    let kernel = kernel();
    for half in [
        Capability::scoped([
            "urn:cap:ledger:write:default".to_string(),
            graph_read("default"),
        ]),
        // The right shape, the wrong ledger.
        Capability::scoped([
            "urn:cap:ledger:write:default".to_string(),
            graph_read("default"),
            graph_write("acme"),
        ]),
        // Broader than needed, and still not this door's key.
        Capability::scoped([
            "urn:cap:ledger:write:default".to_string(),
            graph_read("default"),
            "urn:cap:store:write".to_string(),
        ]),
    ] {
        let refused = try_as(
            &kernel,
            &half,
            Verb::Sink,
            "urn:iki:ledger:append",
            &[("content", "an item")],
        );
        assert!(matches!(refused, Err(Error::Denied(_))), "{refused:?}");
    }
}

/// ★ **A third writing IRI means a third golden thread**, and every write in this crate
/// now goes through `urn:iki:store:graph-update`. A cached listing that did not depend on
/// it would serve pre-write bytes for ever — silently, on the branch that looks like
/// success — and nothing else in this suite would notice, because every other test builds
/// its own kernel and reads each resource once.
///
/// The write below goes **straight at the store's narrow door**, not through a ledger
/// Sink, so the only thread cut is `urn:iki:store:graph-update`: a ledger endpoint would
/// also cut its own target's thread and the test would pass either way.
///
/// ⚠ **What this does not prove, stated because it would be easy to assume otherwise.**
/// Ablating `.depends_on(GRAPH_UPDATE_THREAD)` from `endpoints::face` leaves this test
/// green — measured, not guessed. The invalidation actually arrives by *propagation*: the
/// listing is derived from a sub-request to `urn:iki:store:graph-select`, whose own
/// representation declares all three threads (`ikigai_store::with_freshness`), and the
/// kernel unions a dependency's threads into the derived one. The explicit declarations in
/// `face` are therefore belt-and-braces — correct, and load-bearing only if a read here
/// ever stops going through the store. What this test pins is the property an operator
/// cares about: a write through the narrow door is visible to the next ledger read.
#[test]
fn a_write_through_the_narrow_door_invalidates_a_cached_read() {
    let kernel = kernel();
    let iri = append_iri(&kernel, "The first item", &[]);

    let before = source(&kernel, "urn:iki:ledger:items", &[]);
    assert!(before.contains("1 item(s)"), "{before}");

    // The listing really is in the cache, with golden threads on it — otherwise there
    // would be nothing to go stale and this test would pass vacuously.
    let cache = source(&kernel, "urn:kernel:cache", &[]);
    assert!(cache.contains("urn:iki:ledger:items"), "{cache}");

    sink(
        &kernel,
        "urn:iki:store:graph-update",
        &[
            ("graph", "urn:iki:ledger:graph:default"),
            (
                "content",
                &format!(
                    "INSERT DATA {{ GRAPH <urn:iki:ledger:graph:default> {{ \
                     <{iri}> <https://ikigai-rs.dev/ns/ledger#label> \"out-of-band\" }} }}"
                ),
            ),
        ],
    );

    let after = source(&kernel, "urn:iki:ledger:items", &[]);
    assert!(
        after.contains("out-of-band"),
        "the cached listing did not recompute after a write through the narrow door: {after}"
    );
}

/// ★ The one that matters most. Every write here is a SPARQL string, and ledger content
/// is whatever a caller typed.
#[test]
fn content_that_looks_like_sparql_is_stored_as_text_not_executed() {
    let kernel = kernel();
    append(&kernel, "A real item", &[]);
    let hostile = "\" } ; DROP ALL ; INSERT DATA { GRAPH <urn:iki:ledger:graph:default> { \
                   <urn:x> <urn:y> \"pwned";
    append(&kernel, hostile, &[]);
    sink(
        &kernel,
        "urn:iki:ledger:comment",
        &[("item", "#1"), ("content", hostile)],
    );

    // The first item survived (a DROP ALL would have taken it).
    let list = source(&kernel, "urn:iki:ledger:items", &[]);
    assert!(list.contains("A real item"), "{list}");
    assert!(list.contains("2 item(s)"), "{list}");
    // And nothing was injected.
    let injected = select(&kernel, "ASK { <urn:x> <urn:y> ?o }");
    assert!(!injected.contains("true"), "{injected}");
    // The text is there, as text.
    let view = source(&kernel, "urn:iki:ledger:item:2", &[]);
    assert!(view.contains("DROP ALL"), "{view}");
}

#[test]
fn an_about_iri_that_cannot_be_written_is_refused_rather_than_mangled() {
    let kernel = kernel();
    let refused = try_verb(
        &kernel,
        Verb::Sink,
        "urn:iki:ledger:append",
        &[("content", "An item"), ("about", "urn:x:a>b")],
    );
    assert!(
        matches!(refused, Err(Error::InvalidArgument { ref name, .. }) if name == "about"),
        "{refused:?}"
    );
}

/// The composition failure a host will actually make, and the sentence it gets.
#[test]
fn a_ledger_without_a_store_says_what_is_missing() {
    let kernel = kernel_without_store();
    let failed = try_verb(&kernel, Verb::Source, "urn:iki:ledger:items", &[]);
    let message = failed.expect_err("no store, no ledger").to_string();
    assert!(
        message.contains("urn:iki:store") && message.contains("bind"),
        "the error must name the missing store and what to do: {message}"
    );
}

/// A ledger entry with no timestamp is not a ledger entry, so a kernel with no clock
/// refuses the write instead of writing an unstamped one.
#[test]
fn a_kernel_without_a_clock_refuses_to_write() {
    use ikigai_core::{Fallback, Kernel, Space};
    use ikigai_store::DurableStore;
    use std::sync::Arc;

    let space = Fallback::new(vec![
        Arc::new(ikigai_store::space(DurableStore::in_memory().unwrap())) as Arc<dyn Space>,
        Arc::new(ikigai_ledger::space()) as Arc<dyn Space>,
    ]);
    let clockless = Kernel::new(Arc::new(space));
    let refused = try_verb(
        &clockless,
        Verb::Sink,
        "urn:iki:ledger:append",
        &[("content", "An item")],
    );
    let message = refused.expect_err("no clock, no write").to_string();
    assert!(message.contains("clock"), "{message}");
}

#[test]
fn an_unknown_item_is_not_found_rather_than_silently_ignored() {
    let kernel = kernel();
    assert!(matches!(
        try_verb(&kernel, Verb::Source, "urn:iki:ledger:item:404", &[]),
        Err(Error::NotFound(_))
    ));
    assert!(matches!(
        try_verb(
            &kernel,
            Verb::Sink,
            "urn:iki:ledger:comment",
            &[("item", "#404"), ("content", "hello")]
        ),
        Err(Error::NotFound(_))
    ));
}

// ------------------------------------------------- writes that bypassed the ledger Sink

/// ★ **Assume an editor got there first.** The ledger's graph is reachable by anything
/// holding the store's write scope — an editor, a merge, a bulk load — and that is a
/// supported path rather than corruption. The Sink's refusals never ran on such a write,
/// so the model is checked on READ too, and an item a hand edit left unreadable is
/// REPORTED rather than quietly dropped from every listing.
#[test]
fn an_item_written_around_the_sink_is_reported_not_silently_skipped() {
    let kernel = kernel();
    append(&kernel, "Filed properly", &[]);
    // Straight into the store, bypassing `urn:iki:ledger:append` entirely: typed as an
    // item, and missing everything a reader needs.
    sink(
        &kernel,
        "urn:iki:store:load",
        &[
            (
                "content",
                "<urn:iki:ledger:default:item:handedited> \
                 <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> \
                 <https://ikigai-rs.dev/ns/ledger#Item> ; \
                 <http://purl.org/dc/terms/title> \"Edited in by hand\" .",
            ),
            ("graph", "urn:iki:ledger:graph:default"),
        ],
    );

    let list = source(&kernel, "urn:iki:ledger:items", &[("status", "all")]);
    assert!(list.contains("Filed properly"), "{list}");
    assert!(list.contains("could not be read"), "{list}");
    assert!(
        list.contains("urn:iki:ledger:default:item:handedited"),
        "{list}"
    );
    assert!(list.contains("ledger:number"), "{list}");

    // …and asking for it directly says what is wrong with it, which "not found" would not.
    let failed = try_verb(
        &kernel,
        Verb::Source,
        "urn:iki:ledger:default:item:handedited",
        &[],
    );
    let message = failed.expect_err("unreadable").to_string();
    assert!(message.contains("missing"), "{message}");
    assert!(message.contains("ledger:status"), "{message}");
    assert!(message.contains("Sink"), "{message}");
}

/// The other half of the same rule: an item's IRI must survive editing. It is minted from
/// the clock and a digest of what was filed, and then STORED — never derived from the
/// item's content, its number or its position — so renaming the title, renumbering, or
/// rewriting the body in an editor cannot silently rename the thing.
#[test]
fn an_items_identity_survives_having_its_content_rewritten() {
    let kernel = kernel();
    let before = append_iri(&kernel, "The original title\n\nOriginal body.", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:item:1",
        &[("content", "A completely different title\n\nAnd body.")],
    );
    let after = source(&kernel, "urn:iki:ledger:item:1", &[]);
    assert!(after.contains(&before), "the IRI is unchanged: {after}");
    assert!(after.contains("A completely different title"), "{after}");
}

/// ★ Reverse-engineered and pinned, because nothing in `ikigai-store`'s documentation says
/// it and this module now depends on it: a **multi-operation** SPARQL UPDATE sent as one
/// `urn:iki:store:update` is one request — so a malformed operation anywhere in the
/// sequence means NONE of it ran.
///
/// That is what makes `batch()` an atomicity device rather than only a round-trip saving:
/// a close is "status, reason and stamp" in one statement, and an item cannot be observed
/// closed-with-no-reason between them. (It pins the parse boundary specifically, which is
/// the failure this code can actually produce — every operation here is generated, so a
/// runtime failure mid-sequence would be exotic.)
#[test]
fn a_multi_operation_update_is_refused_whole() {
    let kernel = kernel();
    let refused = try_verb(
        &kernel,
        Verb::Sink,
        "urn:iki:store:update",
        &[(
            "content",
            "INSERT DATA { GRAPH <urn:iki:ledger:graph:default> { \
             <urn:example:first> <urn:example:p> \"landed\" } } ;\n\
             THIS IS NOT SPARQL",
        )],
    );
    assert!(refused.is_err(), "a malformed request must be refused");
    let after = select(
        &kernel,
        "SELECT ?o WHERE { GRAPH ?g { <urn:example:first> ?p ?o } }",
    );
    assert!(
        !after.contains("landed"),
        "the first operation must not have run: {after}"
    );
}
