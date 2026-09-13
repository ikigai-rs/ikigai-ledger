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
    assert!(item.contains("urn:iki:ledger:item:"), "{item}");

    // QUERY: through the store's own face, which is the whole point of not binding a
    // second query surface.
    let answer = select(
        &kernel,
        "SELECT ?t WHERE { GRAPH <urn:iki:ledger:graph> { \
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
        "SELECT ?hash ?quads ?reason ?recoverable WHERE { GRAPH <urn:iki:ledger:graph> { \
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
            "SELECT ?p ?o WHERE {{ GRAPH <urn:iki:ledger:graph:deleted> {{ <{iri}> ?p ?o }} }}"
        ),
    );
    assert!(quarantined.contains("Filed by mistake"), "{quarantined}");
    let comments = select(
        &kernel,
        &format!(
            "SELECT ?c WHERE {{ GRAPH <urn:iki:ledger:graph:deleted> {{ \
             ?c <https://ikigai-rs.dev/ns/ledger#onItem> <{iri}> }} }}"
        ),
    );
    assert!(comments.contains("urn:iki:ledger:comment:"), "{comments}");
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
        "SELECT ?hash ?recoverable ?n WHERE { GRAPH <urn:iki:ledger:graph> { \
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
            "SELECT ?o WHERE {{ GRAPH <urn:iki:ledger:graph> {{ \
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
/// sub-request really needs.
#[test]
fn a_read_capability_cannot_write_and_a_write_capability_cannot_purge() {
    let kernel = kernel();
    append(&kernel, "An item", &[]);

    let reader = Capability::scoped(["urn:cap:ledger:read", "urn:cap:store:read"]);
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
        "urn:cap:ledger:write",
        "urn:cap:store:read",
        "urn:cap:store:write",
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

/// ⚠ The friction this composition really has: a ledger write needs the store's coarse
/// write scope, and holding `urn:cap:ledger:write` alone is not enough. Asserted rather
/// than described, so the day it stops being true a test says so.
#[test]
fn a_ledger_write_also_needs_the_stores_write_scope() {
    let kernel = kernel();
    let half = Capability::scoped(["urn:cap:ledger:write", "urn:cap:store:read"]);
    let refused = try_as(
        &kernel,
        &half,
        Verb::Sink,
        "urn:iki:ledger:append",
        &[("content", "an item")],
    );
    assert!(matches!(refused, Err(Error::Denied(_))), "{refused:?}");
}

/// ★ The one that matters most. Every write here is a SPARQL string, and ledger content
/// is whatever a caller typed.
#[test]
fn content_that_looks_like_sparql_is_stored_as_text_not_executed() {
    let kernel = kernel();
    append(&kernel, "A real item", &[]);
    let hostile = "\" } ; DROP ALL ; INSERT DATA { GRAPH <urn:iki:ledger:graph> { \
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
