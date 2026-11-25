mod common;

use futures::executor::block_on;
use ikigai_core::{ArgRef, Capability, Error, Iri, Request, Verb};

type Args<'a> = &'a [(&'a str, &'a str)];

#[test]
fn invalid_face_is_refused_instead_of_selecting_plain() {
    let kernel = common::kernel();
    let request = Request::new(Verb::Source, Iri::parse("urn:iki:ledger:items").unwrap())
        .with_arg("as", ArgRef::Inline(vec![0xff]));
    let result = block_on(kernel.issue(request, &Capability::root()));
    assert!(
        matches!(result, Err(Error::InvalidArgument { ref name, .. }) if name == "as"),
        "{result:?}"
    );
}

fn request(verb: Verb, iri: &str, args: &[(&str, &str)], extra: Option<(&str, ArgRef)>) -> Request {
    let request = args.iter().fold(
        Request::new(verb, Iri::parse(iri).unwrap()),
        |r, (name, value)| r.with_arg(*name, ArgRef::Inline(value.as_bytes().to_vec())),
    );
    match extra {
        Some((name, arg)) => request.with_arg(name, arg),
        None => request,
    }
}

fn issue(
    kernel: &ikigai_core::Kernel,
    verb: Verb,
    iri: &str,
    args: &[(&str, &str)],
    extra: Option<(&str, ArgRef)>,
) -> Result<ikigai_core::Representation, Error> {
    block_on(kernel.issue(request(verb, iri, args, extra), &Capability::root()))
}

#[test]
fn read_filters_accept_absence_and_valid_values_and_refuse_invalid_text() {
    let kernel = common::kernel();
    common::append(&kernel, "First", &[]);
    let cases: &[(&str, &str, &str)] = &[
        ("items", "as", "text/plain"),
        ("items", "ledgers", "default"),
        ("items", "sort", "number"),
        ("items", "parent", "#1"),
        ("items", "status", "open"),
        ("items", "kind", "urn:example:kind"),
        ("items", "labels", "bug"),
        ("items", "without", "bug"),
        ("items", "about", "urn:example:thing"),
        ("items", "key", "key"),
        ("items", "holder", "any"),
        ("items", "deferred", "include"),
        ("items", "text", "word"),
        ("items", "limit", "5"),
        ("items", "offset", "0"),
        ("count", "ledgers", "default"),
        ("count", "status", "open"),
        ("next", "policy", "priority-recency"),
        ("next", "kind", "urn:example:kind"),
        ("next", "labels", "bug"),
        ("next", "without", "bug"),
        ("next", "about", "urn:example:thing"),
        ("next", "text", "word"),
        ("counts", "days", "2"),
        ("doctor", "awaiting-age", "2h"),
    ];
    for &(resource, name, valid) in cases {
        let iri = format!("urn:iki:ledger:{resource}");
        assert!(
            issue(&kernel, Verb::Source, &iri, &[], None).is_ok(),
            "{resource} absent {name}"
        );
        assert!(
            issue(
                &kernel,
                Verb::Source,
                &iri,
                &[],
                Some((name, ArgRef::Inline(valid.as_bytes().to_vec())))
            )
            .is_ok(),
            "{resource} valid {name}"
        );
        let result = issue(
            &kernel,
            Verb::Source,
            &iri,
            &[],
            Some((name, ArgRef::Inline(vec![0xff]))),
        );
        assert!(
            matches!(result, Err(Error::InvalidArgument { name: ref got, ref detail }) if got == name && !detail.is_empty()),
            "{resource} {name}: {result:?}"
        );
    }
}

#[test]
fn invalid_write_arguments_do_not_change_the_item() {
    let cases: &[(Verb, &str, Args<'_>, &str)] = &[
        (Verb::Sink, "close", &[("item", "#1")], "content"),
        (Verb::Sink, "close", &[("item", "#1")], "evidence"),
        (Verb::Sink, "close", &[("item", "#1")], "reason"),
        (Verb::Sink, "defer", &[("item", "#1")], "content"),
        (
            Verb::Sink,
            "claim",
            &[("item", "#1"), ("content", "worker")],
            "epoch",
        ),
        (
            Verb::Sink,
            "claim",
            &[("item", "#1"), ("content", "worker")],
            "lease",
        ),
        (
            Verb::Sink,
            "claim",
            &[("item", "#1"), ("content", "worker")],
            "purpose",
        ),
        (
            Verb::Sink,
            "comment",
            &[("item", "#1"), ("content", "note")],
            "acks",
        ),
        (
            Verb::Sink,
            "comment",
            &[("item", "#1"), ("content", "note")],
            "key",
        ),
        (
            Verb::Sink,
            "comment",
            &[("item", "#1"), ("content", "note")],
            "author",
        ),
        (
            Verb::Sink,
            "comment",
            &[("item", "#1"), ("content", "note")],
            "created",
        ),
        (
            Verb::Sink,
            "link",
            &[("item", "#1"), ("content", "#2")],
            "type",
        ),
        (Verb::Sink, "item:1", &[("revision", "rev")], "priority"),
        (Verb::Delete, "item:1", &[], "reason"),
    ];
    for &(verb, resource, args, name) in cases {
        let kernel = common::kernel();
        common::append(&kernel, "First", &[]);
        common::append(&kernel, "Second", &[]);
        let before = common::source(&kernel, "urn:iki:ledger:item:1", &[]);
        let iri = format!("urn:iki:ledger:{resource}");
        let result = issue(
            &kernel,
            verb,
            &iri,
            args,
            Some((name, ArgRef::Inline(vec![0xff]))),
        );
        assert!(
            matches!(result, Err(Error::InvalidArgument { name: ref got, .. }) if got == name),
            "{resource} {name}: {result:?}"
        );
        let after = common::source(&kernel, "urn:iki:ledger:item:1", &[]);
        assert_eq!(before, after, "{resource} {name} changed the item");
    }
}

#[test]
fn write_endpoints_accept_absent_and_valid_optional_values() {
    // A fresh ledger per call avoids a successful write affecting the next case.
    let cases: &[(Verb, &str, Args<'_>, &str, &str)] = &[
        (Verb::Sink, "append", &[("content", "New")], "labels", "bug"),
        (
            Verb::Sink,
            "item:1",
            &[("revision", "rev")],
            "priority",
            "2",
        ),
        (
            Verb::Sink,
            "comment",
            &[("item", "#1"), ("content", "note")],
            "key",
            "key",
        ),
        (Verb::Sink, "close", &[("item", "#1")], "reason", "done"),
        (
            Verb::Sink,
            "claim",
            &[("item", "#1"), ("content", "worker")],
            "lease",
            "1h",
        ),
        (Verb::Sink, "defer", &[("item", "#1")], "content", "later"),
        (
            Verb::Sink,
            "link",
            &[("item", "#1"), ("content", "#2")],
            "type",
            "related",
        ),
        (Verb::Delete, "item:1", &[], "reason", "cleanup"),
    ];
    for &(verb, resource, args, name, valid) in cases {
        for value in [None, Some(valid)] {
            let kernel = common::kernel();
            common::append(&kernel, "First", &[]);
            common::append(&kernel, "Second", &[]);
            let iri = format!("urn:iki:ledger:{resource}");
            let extra = value.map(|v| (name, ArgRef::Inline(v.as_bytes().to_vec())));
            let result = issue(&kernel, verb, &iri, args, extra);
            assert!(result.is_ok(), "{resource} {name}={value:?}: {result:?}");
        }
    }
}

#[test]
fn attached_body_is_refused_for_an_inline_filter() {
    let kernel = common::kernel();
    let result = issue(
        &kernel,
        Verb::Source,
        "urn:iki:ledger:items",
        &[],
        Some((
            "sort",
            ArgRef::Content(ikigai_core::ContentId::of(b"number")),
        )),
    );
    assert!(
        matches!(result, Err(Error::InvalidArgument { ref name, .. }) if name == "sort"),
        "{result:?}"
    );
}

#[test]
fn unreadable_append_arguments_never_file_an_item() {
    for name in [
        "key", "raised", "labels", "kind", "priority", "revision", "about", "author", "created",
        "modified",
    ] {
        let kernel = common::kernel();
        let result = issue(
            &kernel,
            Verb::Sink,
            "urn:iki:ledger:append",
            &[("content", "New")],
            Some((name, ArgRef::Inline(vec![0xff]))),
        );
        assert!(
            matches!(result, Err(Error::InvalidArgument { name: ref got, .. }) if got == name),
            "{name}: {result:?}"
        );
        assert_eq!(
            common::source(&kernel, "urn:iki:ledger:count", &[]).trim(),
            "0",
            "{name}"
        );
    }
}

#[test]
fn invalid_notes_on_reopen_release_and_state_move_change_nothing() {
    for resource in ["reopen", "claim", "item:1:state"] {
        let kernel = common::kernel();
        common::append(&kernel, "First", &[]);
        let (verb, args): (Verb, &[(&str, &str)]) = match resource {
            "reopen" => {
                common::sink(&kernel, "urn:iki:ledger:close", &[("item", "#1")]);
                (Verb::Sink, &[("item", "#1")])
            }
            "claim" => {
                common::sink(
                    &kernel,
                    "urn:iki:ledger:claim",
                    &[("item", "#1"), ("content", "worker")],
                );
                (Verb::Delete, &[("item", "#1")])
            }
            _ => (Verb::Sink, &[("from", "filed"), ("to", "queued")]),
        };
        let before = common::source(&kernel, "urn:iki:ledger:item:1", &[]);
        let iri = format!("urn:iki:ledger:{resource}");
        let result = issue(
            &kernel,
            verb,
            &iri,
            args,
            Some(("content", ArgRef::Inline(vec![0xff]))),
        );
        assert!(
            matches!(result, Err(Error::InvalidArgument { ref name, .. }) if name == "content"),
            "{resource}: {result:?}"
        );
        assert_eq!(
            before,
            common::source(&kernel, "urn:iki:ledger:item:1", &[]),
            "{resource}"
        );
    }
}

#[test]
fn dispatch_note_and_cursor_refuse_invalid_arguments() {
    let kernel = common::kernel();
    common::append(&kernel, "First", &[]);
    let principal = Capability::scoped(vec![
        "urn:cap:ledger:claim:default".to_string(),
        "urn:cap:ledger:close:default".to_string(),
        "urn:cap:ledger:read:default".to_string(),
        common::graph_read("default"),
        common::graph_write("default"),
    ])
    .with_principal("urn:example:dispatcher")
    .unwrap();
    block_on(kernel.issue(
        request(
            Verb::Sink,
            "urn:iki:ledger:claim",
            &[("item", "#1"), ("content", "worker")],
            None,
        ),
        &principal,
    ))
    .unwrap();
    let note_args = [("item", "#1"), ("epoch", "1"), ("content", "message")];
    let make = |extra| request(Verb::Sink, "urn:iki:ledger:note", &note_args, extra);
    let invalid =
        block_on(kernel.issue(make(Some(("key", ArgRef::Inline(vec![0xff])))), &principal));
    assert!(
        matches!(invalid, Err(Error::InvalidArgument { ref name, .. }) if name == "key"),
        "{invalid:?}"
    );
    for key in [None, Some("note-key")] {
        let extra = key.map(|key| ("key", ArgRef::Inline(key.as_bytes().to_vec())));
        block_on(kernel.issue(make(extra), &principal)).expect("note sent");
    }
    let iri = "urn:iki:ledger:item:1:notes";
    let doc = block_on(kernel.issue(
        request(Verb::Source, iri, &[("as", "application/json")], None),
        &principal,
    ))
    .unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&doc.bytes).unwrap();
    let cursor = doc["notes"][0]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("note IRI: {doc}"));
    assert!(block_on(kernel.issue(request(Verb::Source, iri, &[], None), &principal)).is_ok());
    assert!(block_on(kernel.issue(
        request(
            Verb::Source,
            iri,
            &[],
            Some(("after", ArgRef::Inline(cursor.as_bytes().to_vec())))
        ),
        &principal
    ))
    .is_ok());
    let result = block_on(kernel.issue(
        request(
            Verb::Source,
            iri,
            &[],
            Some(("after", ArgRef::Inline(vec![0xff]))),
        ),
        &principal,
    ));
    assert!(
        matches!(result, Err(Error::InvalidArgument { ref name, .. }) if name == "after"),
        "{result:?}"
    );
}

#[test]
fn backfill_and_purge_refuse_unreadable_arguments() {
    for (verb, resource, args, name) in [
        (
            Verb::Sink,
            "backfill-closed-at",
            &[("content", "all")][..],
            "comment",
        ),
        (Verb::Delete, "purge", &[("content", "#1")][..], "reason"),
    ] {
        let kernel = common::kernel();
        common::append(&kernel, "First", &[]);
        let before = common::source(&kernel, "urn:iki:ledger:item:1", &[]);
        let iri = format!("urn:iki:ledger:{resource}");
        let result = issue(
            &kernel,
            verb,
            &iri,
            args,
            Some((name, ArgRef::Inline(vec![0xff]))),
        );
        assert!(
            matches!(result, Err(Error::InvalidArgument { name: ref got, .. }) if got == name),
            "{resource}: {result:?}"
        );
        assert_eq!(
            before,
            common::source(&kernel, "urn:iki:ledger:item:1", &[])
        );
    }
}

#[test]
fn readable_resources_refuse_an_invalid_face() {
    let kernel = common::kernel();
    common::append(&kernel, "First", &[]);
    for resource in [
        "item:1",
        "item:1:history",
        "item:1:state",
        "item:1:sessions",
        "sessions",
        "ledgers",
        "policy:priority-recency",
        "lifecycle:kata-flight",
        "stalled",
        "doctor",
        "count",
        "next",
    ] {
        let iri = format!("urn:iki:ledger:{resource}");
        assert!(
            issue(&kernel, Verb::Source, &iri, &[], None).is_ok(),
            "{resource} default"
        );
        let face = if resource.starts_with("policy:") || resource.starts_with("lifecycle:") {
            "text/turtle"
        } else {
            "application/json"
        };
        assert!(
            issue(
                &kernel,
                Verb::Source,
                &iri,
                &[],
                Some(("as", ArgRef::Inline(face.as_bytes().to_vec())))
            )
            .is_ok(),
            "{resource} {face}"
        );
        let result = issue(
            &kernel,
            Verb::Source,
            &iri,
            &[],
            Some(("as", ArgRef::Inline(vec![0xff]))),
        );
        assert!(
            matches!(result, Err(Error::InvalidArgument { ref name, .. }) if name == "as"),
            "{resource}: {result:?}"
        );
    }
}

#[test]
fn optional_notes_accept_absence_and_text_on_reopen_release_and_state_move() {
    for resource in ["reopen", "claim", "item:1:state"] {
        for value in [None, Some("explanation")] {
            let kernel = common::kernel();
            common::append(&kernel, "First", &[]);
            let (verb, args): (Verb, &[(&str, &str)]) = match resource {
                "reopen" => {
                    common::sink(&kernel, "urn:iki:ledger:close", &[("item", "#1")]);
                    (Verb::Sink, &[("item", "#1")])
                }
                "claim" => {
                    common::sink(
                        &kernel,
                        "urn:iki:ledger:claim",
                        &[("item", "#1"), ("content", "worker")],
                    );
                    (Verb::Delete, &[("item", "#1")])
                }
                _ => (Verb::Sink, &[("from", "filed"), ("to", "queued")]),
            };
            let iri = format!("urn:iki:ledger:{resource}");
            let extra = value.map(|v| ("content", ArgRef::Inline(v.as_bytes().to_vec())));
            let result = issue(&kernel, verb, &iri, args, extra);
            assert!(result.is_ok(), "{resource} {value:?}: {result:?}");
        }
    }
}

#[test]
fn write_resources_refuse_an_unreadable_face_before_writing() {
    let cases: &[(Verb, &str, Args<'_>)] = &[
        (Verb::Sink, "append", &[("content", "New")]),
        (
            Verb::Sink,
            "comment",
            &[("item", "#1"), ("content", "note")],
        ),
        (Verb::Sink, "close", &[("item", "#1")]),
        (Verb::Sink, "link", &[("item", "#1"), ("content", "#2")]),
        (Verb::Sink, "lapse", &[]),
        (Verb::Sink, "history", &[("item", "#1"), ("content", "")]),
        (
            Verb::Sink,
            "item:1:state",
            &[("from", "filed"), ("to", "queued")],
        ),
    ];
    for &(verb, resource, args) in cases {
        let kernel = common::kernel();
        common::append(&kernel, "First", &[]);
        common::append(&kernel, "Second", &[]);
        let before = common::source(&kernel, "urn:iki:ledger:item:1", &[]);
        let iri = format!("urn:iki:ledger:{resource}");
        let result = issue(
            &kernel,
            verb,
            &iri,
            args,
            Some(("as", ArgRef::Inline(vec![0xff]))),
        );
        assert!(
            matches!(result, Err(Error::InvalidArgument { ref name, .. }) if name == "as"),
            "{resource}: {result:?}"
        );
        assert_eq!(
            before,
            common::source(&kernel, "urn:iki:ledger:item:1", &[]),
            "{resource}"
        );
    }
}
