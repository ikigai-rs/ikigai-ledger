//! **The faces, as literals.** The plain face is what the gonk bridges parse (`iri:` lines,
//! status tokens, `blocks:` lines, the `#N <iri>` answer), so a change to it breaks them
//! silently; these pin it byte for byte, as 0.3.0 rendered it. The JSON face is the
//! machine contract that replaces that parsing, and its literals are the schema.
//!
//! Minted ids are random per process, so every id is replaced by `{a}`, `{b}`, … in order
//! of first appearance before comparing; everything else is compared exactly.

mod common;

use std::sync::Arc;

use common::*;
use ikigai_core::{Clock, Fallback, Kernel, Space, Time};
use ikigai_ledger::json::{Answer, ItemDocument, ItemsDocument, NextDocument};
use ikigai_store::DurableStore;

/// A clock that never moves: the literals below carry timestamps, and how many times a
/// request reads the clock is not part of any face.
struct Frozen;

impl Clock for Frozen {
    fn now(&self) -> Time {
        Time::from_millis(1_789_430_400_000) // 2026-09-15T00:00:00Z
    }
}

fn frozen() -> Kernel {
    let space = Fallback::new(vec![
        Arc::new(ikigai_store::space(DurableStore::in_memory().unwrap())) as Arc<dyn Space>,
        Arc::new(ikigai_ledger::space()) as Arc<dyn Space>,
    ]);
    Kernel::with_meta_renderer(Arc::new(space), Arc::new(ikigai_vocab::TurtleRenderer))
        .with_clock(Arc::new(Frozen))
}

/// Replace every minted id (23 Crockford characters after `:item:`, `:comment:` or
/// `:tombstone:`) with a placeholder, stable within one text.
fn normalize(text: &str) -> String {
    let mut out = text.to_string();
    let mut seen: Vec<String> = Vec::new();
    for marker in [":item:", ":comment:", ":tombstone:"] {
        let mut from = 0;
        while let Some(at) = out[from..].find(marker) {
            let start = from + at + marker.len();
            let id: String = out[start..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect();
            if id.len() != 23 {
                from = start;
                continue;
            }
            let index = match seen.iter().position(|s| *s == id) {
                Some(i) => i,
                None => {
                    seen.push(id.clone());
                    seen.len() - 1
                }
            };
            let placeholder = format!("{{{}}}", (b'a' + index as u8) as char);
            out.replace_range(start..start + id.len(), &placeholder);
            from = start + placeholder.len();
        }
    }
    // A second pass, so an id first seen under one marker is the same placeholder under
    // another (an item's tombstone keeps the item's id).
    for (index, id) in seen.iter().enumerate() {
        out = out.replace(id, &format!("{{{}}}", (b'a' + index as u8) as char));
    }
    out
}

/// A small ledger: #1 with a body, a label, a priority, an `about`, a claim and a comment,
/// blocking #2; #3 closed with a note.
fn seeded() -> Kernel {
    let kernel = frozen();
    append(
        &kernel,
        "Fix the thing\n\nIt is broken.\nBadly.",
        &[
            ("priority", "1"),
            ("labels", "rust,audit"),
            ("about", "urn:repo:file:x/src/a.rs"),
            ("author", "brian"),
            ("revision", "f41a333"),
        ],
    );
    append(&kernel, "Second", &[]);
    append(&kernel, "Third", &[]);
    sink(
        &kernel,
        "urn:iki:ledger:link",
        &[("item", "#1"), ("content", "#2"), ("type", "blocks")],
    );
    sink(
        &kernel,
        "urn:iki:ledger:claim",
        &[
            ("item", "#1"),
            ("content", "satellite"),
            ("purpose", "brief-x"),
        ],
    );
    sink(
        &kernel,
        "urn:iki:ledger:comment",
        &[
            ("item", "#1"),
            ("content", "looked at it"),
            ("author", "chris"),
        ],
    );
    sink(
        &kernel,
        "urn:iki:ledger:close",
        &[
            ("item", "#3"),
            ("reason", "wontfix"),
            ("content", "not worth it"),
        ],
    );
    kernel
}

// ------------------------------------------------------------------ the plain face, 0.3.0

#[test]
fn the_plain_item_detail_is_unchanged() {
    let kernel = seeded();
    assert_eq!(
        normalize(&source(&kernel, "urn:iki:ledger:item:1", &[])),
        "   #1  open    p1  Fix the thing  [audit rust]  claimed:satellite\n\
         \x20 iri:      urn:iki:ledger:default:item:{a}\n\
         \x20 filed:    2026-09-15T00:00:00.000Z by brian\n\
         \x20 updated:  2026-09-15T00:00:00.000Z\n\
         \x20 revision: f41a333\n\
         \x20 about:    urn:repo:file:x/src/a.rs\n\
         \x20 blocks:   urn:iki:ledger:default:item:{b}\n\
         \x20 purpose:  brief-x\n\
         \n\
         \x20 It is broken.\n\
         \x20 Badly.\n\
         \n\
         1 comment(s):\n\
         \x20 2026-09-15T00:00:00.000Z chris\n\
         \x20   looked at it\n"
    );
}

#[test]
fn the_plain_listing_is_unchanged() {
    let kernel = seeded();
    assert_eq!(
        normalize(&source(
            &kernel,
            "urn:iki:ledger:items",
            &[("status", "all")]
        )),
        "   #3  closed  p-  Third  (wontfix)\n\
         \x20  #2  open    p-  Second\n\
         \x20  #1  open    p1  Fix the thing  [audit rust]  claimed:satellite\n\
         \n\
         3 item(s)\n"
    );
}

#[test]
fn the_plain_write_answers_are_unchanged() {
    let kernel = frozen();
    assert_eq!(
        normalize(&sink(
            &kernel,
            "urn:iki:ledger:append",
            &[("content", "One")]
        )),
        "#1 urn:iki:ledger:default:item:{a}\n"
    );
    append(&kernel, "Two", &[]);
    assert_eq!(
        normalize(&sink(
            &kernel,
            "urn:iki:ledger:comment",
            &[("item", "#1"), ("content", "a note")]
        )),
        "commented on #1 (urn:iki:ledger:default:comment:{a})\n"
    );
    assert_eq!(
        sink(
            &kernel,
            "urn:iki:ledger:link",
            &[("item", "#1"), ("content", "#2"), ("type", "blocks")]
        ),
        "linked #1 blocks #2\n"
    );
    assert_eq!(
        sink(&kernel, "urn:iki:ledger:close", &[("item", "#2")]),
        "closed #2 (done)\n"
    );
}

#[test]
fn the_plain_next_is_unchanged() {
    let kernel = seeded();
    assert_eq!(
        source(&kernel, "urn:iki:ledger:next", &[]),
        "nothing is ready (2 open item(s) excluded: #2 blocked by #1; #1 claimed by satellite)\n\
         policy: priority-recency\n"
    );
}

// ------------------------------------------------------------------- the JSON face

/// A JSON body, checked to be exactly one compact line that is precisely the typed
/// document's own serialization (so no field is unknown to the type and none is out of
/// order), then pretty-printed in the type's field order for a readable literal.
fn typed<T: serde::Serialize + serde::de::DeserializeOwned>(body: &str) -> String {
    let doc: T = serde_json::from_str(body).expect("the face parses as its documented type");
    assert_eq!(
        body,
        format!("{}\n", serde_json::to_string(&doc).unwrap()),
        "the face is one compact line, byte for byte the type's serialization"
    );
    normalize(&serde_json::to_string_pretty(&doc).unwrap())
}

/// [`seeded`], plus a fourth item filed with a key.
fn seeded_with_a_key() -> Kernel {
    let kernel = seeded();
    sink(
        &kernel,
        "urn:iki:ledger:append",
        &[("content", "Keyed"), ("key", "urn:kata:issue:01JZ")],
    );
    kernel
}

/// `as=application/json` and the rest.
fn json_args<'a>(args: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
    let mut all = vec![("as", "application/json")];
    all.extend_from_slice(args);
    all
}

#[test]
fn the_json_item_document_is_schema_1() {
    let kernel = seeded_with_a_key();
    assert_eq!(
        typed::<ItemDocument>(&source(&kernel, "urn:iki:ledger:item:1", &json_args(&[]))),
        r##"{
  "schema": 1,
  "ledger": "default",
  "item": {
    "number": 1,
    "display": "#1",
    "iri": "urn:iki:ledger:default:item:{a}",
    "kind": null,
    "title": "Fix the thing",
    "body": "It is broken.\nBadly.",
    "status": "open",
    "closed_reason": null,
    "priority": 1,
    "deferred": false,
    "labels": [
      "audit",
      "rust"
    ],
    "about": [
      "urn:repo:file:x/src/a.rs"
    ],
    "key": null,
    "author": "brian",
    "revision": "f41a333",
    "claim": {
      "holder": "satellite",
      "purpose": "brief-x"
    },
    "created": "2026-09-15T00:00:00.000Z",
    "modified": "2026-09-15T00:00:00.000Z",
    "links": [
      {
        "type": "blocks",
        "target": {
          "number": 2,
          "display": "#2",
          "iri": "urn:iki:ledger:default:item:{b}"
        }
      }
    ],
    "comments": [
      {
        "id": "urn:iki:ledger:default:comment:{c}",
        "author": "chris",
        "time": "2026-09-15T00:00:00.000Z",
        "text": "looked at it"
      }
    ],
    "defects": [],
    "state": "filed"
  }
}"##
    );
}

#[test]
fn the_json_items_document_is_schema_1() {
    let kernel = seeded_with_a_key();
    assert_eq!(
        typed::<ItemsDocument>(&source(
            &kernel,
            "urn:iki:ledger:items",
            &json_args(&[("status", "all"), ("limit", "2")])
        )),
        r##"{
  "schema": 1,
  "ledger": "default",
  "count": 2,
  "items": [
    {
      "number": 4,
      "display": "#4",
      "iri": "urn:iki:ledger:default:item:{a}",
      "kind": null,
      "title": "Keyed",
      "body": "",
      "status": "open",
      "closed_reason": null,
      "priority": null,
      "deferred": false,
      "labels": [],
      "about": [],
      "key": "urn:kata:issue:01JZ",
      "author": null,
      "revision": null,
      "claim": null,
      "created": "2026-09-15T00:00:00.000Z",
      "modified": "2026-09-15T00:00:00.000Z",
      "links": [],
      "comments": [],
      "defects": [],
      "state": "filed"
    },
    {
      "number": 3,
      "display": "#3",
      "iri": "urn:iki:ledger:default:item:{b}",
      "kind": null,
      "title": "Third",
      "body": "",
      "status": "closed",
      "closed_reason": "wontfix",
      "priority": null,
      "deferred": false,
      "labels": [],
      "about": [],
      "key": null,
      "author": null,
      "revision": null,
      "claim": null,
      "created": "2026-09-15T00:00:00.000Z",
      "modified": "2026-09-15T00:00:00.000Z",
      "links": [],
      "comments": [
        {
          "id": "urn:iki:ledger:default:comment:{c}",
          "author": null,
          "time": "2026-09-15T00:00:00.000Z",
          "text": "not worth it"
        }
      ],
      "defects": [],
      "state": "filed"
    }
  ],
  "unreadable": []
}"##
    );
}

#[test]
fn the_json_next_document_is_schema_1() {
    let kernel = seeded_with_a_key();
    assert_eq!(
        typed::<NextDocument>(&source(&kernel, "urn:iki:ledger:next", &json_args(&[]))),
        r##"{
  "schema": 1,
  "ledger": "default",
  "policy": "priority-recency",
  "weighs": [
    "priority",
    "recency",
    "number"
  ],
  "generated_at": "2026-09-15T00:00:00.000Z",
  "ready": 1,
  "ranking": [
    {
      "rank": 1,
      "score": "unprioritized",
      "because": [
        "no priority set, so it ranks below every item that has one",
        "last updated 2026-09-15T00:00:00.000Z"
      ],
      "item": {
        "number": 4,
        "display": "#4",
        "iri": "urn:iki:ledger:default:item:{a}",
        "kind": null,
        "title": "Keyed",
        "body": "",
        "status": "open",
        "closed_reason": null,
        "priority": null,
        "deferred": false,
        "labels": [],
        "about": [],
        "key": "urn:kata:issue:01JZ",
        "author": null,
        "revision": null,
        "claim": null,
        "created": "2026-09-15T00:00:00.000Z",
        "modified": "2026-09-15T00:00:00.000Z",
        "links": [],
        "comments": [],
        "defects": [],
        "state": "filed"
      }
    }
  ],
  "excluded": [
    {
      "why": "blocked",
      "reason": "blocked by #1",
      "holder": null,
      "blocked_by": [
        1
      ],
      "item": {
        "number": 2,
        "display": "#2",
        "iri": "urn:iki:ledger:default:item:{b}",
        "kind": null,
        "title": "Second",
        "body": "",
        "status": "open",
        "closed_reason": null,
        "priority": null,
        "deferred": false,
        "labels": [],
        "about": [],
        "key": null,
        "author": null,
        "revision": null,
        "claim": null,
        "created": "2026-09-15T00:00:00.000Z",
        "modified": "2026-09-15T00:00:00.000Z",
        "links": [],
        "comments": [],
        "defects": [],
        "state": "filed"
      }
    },
    {
      "why": "claimed",
      "reason": "claimed by satellite",
      "holder": "satellite",
      "blocked_by": [],
      "item": {
        "number": 1,
        "display": "#1",
        "iri": "urn:iki:ledger:default:item:{c}",
        "kind": null,
        "title": "Fix the thing",
        "body": "It is broken.\nBadly.",
        "status": "open",
        "closed_reason": null,
        "priority": 1,
        "deferred": false,
        "labels": [
          "audit",
          "rust"
        ],
        "about": [
          "urn:repo:file:x/src/a.rs"
        ],
        "key": null,
        "author": "brian",
        "revision": "f41a333",
        "claim": {
          "holder": "satellite",
          "purpose": "brief-x"
        },
        "created": "2026-09-15T00:00:00.000Z",
        "modified": "2026-09-15T00:00:00.000Z",
        "links": [
          {
            "type": "blocks",
            "target": {
              "number": 2,
              "display": "#2",
              "iri": "urn:iki:ledger:default:item:{b}"
            }
          }
        ],
        "comments": [
          {
            "id": "urn:iki:ledger:default:comment:{d}",
            "author": "chris",
            "time": "2026-09-15T00:00:00.000Z",
            "text": "looked at it"
          }
        ],
        "defects": [],
        "state": "filed"
      }
    }
  ]
}"##
    );
}

/// The four write answers, in sequence over one ledger. Each carries only its own
/// outcome's fields (see `json::Answer`), so absence is pinned here as much as presence.
#[test]
fn the_json_write_answers_are_schema_1() {
    let kernel = seeded_with_a_key();
    let answer =
        |iri: &str, args: &[(&str, &str)]| typed::<Answer>(&sink(&kernel, iri, &json_args(args)));
    assert_eq!(
        answer("urn:iki:ledger:append", &[("content", "Fifth")]),
        r##"{
  "schema": 1,
  "ledger": "default",
  "outcome": "filed",
  "item": {
    "number": 5,
    "display": "#5",
    "iri": "urn:iki:ledger:default:item:{a}"
  },
  "status": "open"
}"##
    );
    assert_eq!(
        answer(
            "urn:iki:ledger:append",
            &[("content", "x"), ("key", "urn:kata:issue:01JZ")]
        ),
        r##"{
  "schema": 1,
  "ledger": "default",
  "outcome": "existing",
  "item": {
    "number": 4,
    "display": "#4",
    "iri": "urn:iki:ledger:default:item:{a}"
  },
  "status": "open",
  "key": "urn:kata:issue:01JZ"
}"##
    );
    assert_eq!(
        answer(
            "urn:iki:ledger:comment",
            &[("item", "#2"), ("content", "hi"), ("author", "brian")]
        ),
        r##"{
  "schema": 1,
  "ledger": "default",
  "outcome": "commented",
  "item": {
    "number": 2,
    "display": "#2",
    "iri": "urn:iki:ledger:default:item:{a}"
  },
  "comment": {
    "id": "urn:iki:ledger:default:comment:{b}",
    "author": "brian",
    "time": "2026-09-15T00:00:00.000Z",
    "text": "hi"
  }
}"##
    );
    assert_eq!(
        answer(
            "urn:iki:ledger:close",
            &[
                ("item", "#2"),
                ("reason", "duplicate"),
                ("content", "see #1")
            ]
        ),
        r##"{
  "schema": 1,
  "ledger": "default",
  "outcome": "closed",
  "item": {
    "number": 2,
    "display": "#2",
    "iri": "urn:iki:ledger:default:item:{a}"
  },
  "reason": "duplicate",
  "comment": {
    "id": "urn:iki:ledger:default:comment:{b}",
    "author": null,
    "time": "2026-09-15T00:00:00.000Z",
    "text": "see #1"
  }
}"##
    );
    assert_eq!(
        answer(
            "urn:iki:ledger:link",
            &[("item", "#4"), ("content", "#5"), ("type", "related")]
        ),
        r##"{
  "schema": 1,
  "ledger": "default",
  "outcome": "linked",
  "item": {
    "number": 4,
    "display": "#4",
    "iri": "urn:iki:ledger:default:item:{a}"
  },
  "type": "related",
  "target": {
    "number": 5,
    "display": "#5",
    "iri": "urn:iki:ledger:default:item:{b}"
  }
}"##
    );
}

/// A keyed item's plain detail gains exactly one line, `key:`, after `iri:` — the only
/// change to the plain face, and only for items that have a key.
#[test]
fn a_keyed_items_plain_detail_gains_one_line() {
    let kernel = seeded_with_a_key();
    assert_eq!(
        normalize(&source(
            &kernel,
            "urn:iki:ledger:item:key:urn:kata:issue:01JZ",
            &[]
        )),
        "   #4  open    p-  Keyed\n\
         \x20 iri:      urn:iki:ledger:default:item:{a}\n\
         \x20 key:      urn:kata:issue:01JZ\n\
         \x20 filed:    2026-09-15T00:00:00.000Z\n\
         \x20 updated:  2026-09-15T00:00:00.000Z\n"
    );
}

/// The JSON face is a read like the others: cached, and invalidated by a write.
#[test]
fn a_write_invalidates_a_cached_json_read() {
    let kernel = seeded_with_a_key();
    let count = || {
        let doc: serde_json::Value = serde_json::from_str(&source(
            &kernel,
            "urn:iki:ledger:items",
            &json_args(&[("status", "all")]),
        ))
        .unwrap();
        doc["count"].as_u64().unwrap()
    };
    assert_eq!(count(), 4);
    assert_eq!(count(), 4);
    append(&kernel, "Fifth", &[]);
    assert_eq!(count(), 5, "a cached JSON read survived a write");
}
