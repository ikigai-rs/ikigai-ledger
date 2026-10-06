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
        &[("item", "#1"), ("content", "satellite"), ("purpose", "brief-x")],
    );
    sink(
        &kernel,
        "urn:iki:ledger:comment",
        &[("item", "#1"), ("content", "looked at it"), ("author", "chris")],
    );
    sink(
        &kernel,
        "urn:iki:ledger:close",
        &[("item", "#3"), ("reason", "wontfix"), ("content", "not worth it")],
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
