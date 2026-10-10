//! Three defects found by USING the ledger, each of which produced a confident wrong answer
//! rather than an error (ledger #425, #398, #419).
//!
//! 1. **A shared ArgSpec summary named one endpoint on every endpoint.** `ledger`'s summary
//!    said the default-ledger form was `urn:iki:ledger:append` on `comment`, `close`, `next`
//!    and the rest, and a satellite that believed it filed a junk item instead of a comment.
//!    A summary is part of the contract the manifold, MCP and selection hand an agent, so
//!    [`every_ledger_summary_names_the_resource_it_is_on`] reads every bound action off the
//!    space and holds each one to its own binding.
//! 2. **Adding one `about` meant resending the whole body.** The engine's `sink` always
//!    sends `content`, empty when nothing was piped (`ikigai-engine`, `write_request`), and
//!    the item Sink read an empty `content` as "set the title to nothing" and refused. So a
//!    scalar edit from the command line was impossible without retransmitting the title and
//!    body verbatim, and no read face returned them in that shape.
//! 3. **`items` reported a page as a total.** `limit=50` printed fifty rows and `50 item(s)`
//!    over a ledger of 385, and nothing said the list was cut.

mod common;

use ikigai_core::{Iri, Request, Resolution, Scope, Space, Verb};

use common::*;

// ------------------------------------------------------------------- ledger #425

/// A bound pattern with its template variables filled, so it resolves.
fn concrete(pattern: &str) -> String {
    pattern
        .replace("{ledger}", "acme")
        .replace("{id}", "1")
        .replace("{name}", "leverage")
        .replace("{value}", "filed")
        .replace("{holder}", "none")
}

/// Every `urn:iki:ledger:` IRI a summary names, up to the closing backtick or whitespace.
fn named_iris(summary: &str) -> Vec<String> {
    summary
        .match_indices("urn:iki:ledger:")
        .map(|(at, _)| {
            summary[at..]
                .split(|c: char| c == '`' || c.is_whitespace() || c == ')')
                .next()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// ★ **A summary is TRUE of the endpoint it is attached to, or it is not there.** The
/// conformance suite checks that inputs are named and typed; nothing checked that what a
/// summary SAYS is true of its own endpoint. This does, for the one input every ledger
/// action shares: its summary names this action's own two spellings and no other resource.
#[test]
fn every_ledger_summary_names_the_resource_it_is_on() {
    let space = ikigai_ledger::space();
    let scope = Scope::empty();
    let mut checked = 0;
    for entry in space
        .entries()
        .expect("an EndpointSpace enumerates its bindings")
    {
        let Some(path) = entry.pattern.strip_prefix("urn:iki:ledger:{ledger}:") else {
            continue; // `ledgers`, `policy:{name}`, `lifecycle:{name}`: no ledger segment
        };
        let named = entry.pattern.clone();
        let bare = format!("urn:iki:ledger:{path}");
        let iri = Iri::parse(concrete(&entry.pattern)).expect("an expanded pattern is an IRI");
        let Resolution::Hit(resolved) = space.resolve(&Request::new(Verb::Meta, iri), &scope)
        else {
            panic!("`{}` does not resolve its own pattern", entry.pattern);
        };
        let description = resolved.endpoint.describe();
        let mut inputs = description.inputs.clone();
        for spec in description.action_specs() {
            inputs.extend(spec.inputs);
        }
        for input in inputs.into_iter().filter(|input| input.name == "ledger") {
            checked += 1;
            assert!(
                input.summary.contains(&format!("`{bare}`")),
                "`{}`'s `ledger` summary does not name its own default-ledger form \
                 `{bare}`: {}",
                entry.endpoint,
                input.summary
            );
            for iri in named_iris(&input.summary) {
                assert!(
                    iri == bare || iri == named,
                    "`{}`'s `ledger` summary names `{iri}`, which is not this resource \
                     (`{named}` or `{bare}`): {}",
                    entry.endpoint,
                    input.summary
                );
            }
        }
    }
    assert!(checked > 10, "only {checked} ledger inputs were checked");
}

// ------------------------------------------------------------------- ledger #398

/// The item's title, body and `about` targets, read straight from the graph.
fn stored(kernel: &ikigai_core::Kernel, number: &str) -> (String, String, String) {
    let json = source(
        kernel,
        &format!("urn:iki:ledger:item:{}", number.trim_start_matches('#')),
        &[("as", "application/json")],
    );
    let doc: serde_json::Value = serde_json::from_str(&json).expect("the JSON face parses");
    let item = &doc["item"];
    (
        item["title"].as_str().unwrap_or_default().to_string(),
        item["body"].as_str().unwrap_or_default().to_string(),
        item["about"].to_string(),
    )
}

/// ★ **The engine's shape, not the test harness's.** `sink <iri> about=…` from the command
/// line sends `content` too — empty, because nothing was piped. An empty `content` cannot
/// be a title (a title is refused empty), so it means "no title or body given", and the
/// scalar edit beside it goes through with the title and body untouched.
#[test]
fn a_scalar_edit_with_the_engines_empty_content_keeps_the_title_and_body() {
    let kernel = kernel();
    let n = append(
        &kernel,
        "Sixty lines of argument\n\nThe valuable part.",
        &[],
    );
    let item = format!("urn:iki:ledger:item:{}", n.trim_start_matches('#'));

    sink(
        &kernel,
        &item,
        &[("about", "urn:agents:session:abc"), ("content", "")],
    );
    sink(&kernel, &item, &[("priority", "3"), ("content", "\n")]);

    let (title, body, about) = stored(&kernel, &n);
    assert_eq!(title, "Sixty lines of argument");
    assert_eq!(body, "The valuable part.");
    assert!(about.contains("urn:agents:session:abc"), "{about}");
    let listing = source(&kernel, &item, &[]);
    assert!(listing.contains("p3"), "{listing}");

    // An empty content with NOTHING else is still refused: it asks for no change at all,
    // and reading it as a request to blank the title is the hazard this closes.
    let refused = try_verb(&kernel, Verb::Sink, &item, &[("content", "")])
        .expect_err("an edit that changes nothing is refused");
    assert!(refused.to_string().contains("title"), "{refused}");
}

/// `about` is additive AND removable on its own resource, an item part: the value is
/// `content`, which is where the engine routes a pipe or a trailing word.
#[test]
fn about_is_added_and_removed_on_its_own_resource() {
    let kernel = kernel();
    let n = append(&kernel, "Delivered\n\nThe body survives.", &[]);
    let about = format!("urn:iki:ledger:item:{}:about", n.trim_start_matches('#'));
    let said = sink(
        &kernel,
        &about,
        &[("content", "urn:agents:session:s1 urn:repo:file:x/a.rs")],
    );
    assert!(said.contains("urn:agents:session:s1"), "{said}");
    let (title, body, targets) = stored(&kernel, &n);
    assert_eq!(
        (title.as_str(), body.as_str()),
        ("Delivered", "The body survives.")
    );
    assert!(targets.contains("urn:agents:session:s1"), "{targets}");
    assert!(targets.contains("urn:repo:file:x/a.rs"), "{targets}");

    // The filter finds it by the session IRI — the convention this exists for.
    let found = source(
        &kernel,
        "urn:iki:ledger:items",
        &[("about", "urn:agents:session:s1")],
    );
    assert!(found.contains("Delivered"), "{found}");

    delete(&kernel, &about, &[("content", "urn:repo:file:x/a.rs")]);
    let (_, _, targets) = stored(&kernel, &n);
    assert!(targets.contains("urn:agents:session:s1"), "{targets}");
    assert!(!targets.contains("urn:repo:file:x/a.rs"), "{targets}");

    // A named ledger's item part, under that ledger's spelling.
    let acme = sink(
        &kernel,
        "urn:iki:ledger:acme:append",
        &[("content", "Elsewhere")],
    );
    let number = acme.split_whitespace().next().unwrap_or_default();
    let number = number.rsplit('#').next().unwrap_or_default();
    sink(
        &kernel,
        &format!("urn:iki:ledger:acme:item:{number}:about"),
        &[("content", "urn:agents:session:s2")],
    );
    let found = source(
        &kernel,
        "urn:iki:ledger:acme:items",
        &[("about", "urn:agents:session:s2")],
    );
    assert!(found.contains("Elsewhere"), "{found}");

    // Not an IRI, or nothing at all: refused, never stored as a literal.
    for bad in ["", "  ", "not an iri"] {
        assert!(
            try_verb(&kernel, Verb::Sink, &about, &[("content", bad)]).is_err(),
            "`{bad}` was accepted"
        );
    }
    // Neither verb reads or probes: anything else is refused.
    assert!(try_verb(&kernel, Verb::Source, &about, &[]).is_err());
}

// ------------------------------------------------------------------- ledger #419

fn titles(listing: &str) -> Vec<String> {
    listing
        .lines()
        .map(str::trim_start)
        .filter(|line| line.starts_with('#'))
        .map(|line| {
            line.split_whitespace()
                .nth(3)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// ★ **A page is never reported as a total.** The footer states both, and names the
/// argument that reaches the rest.
#[test]
fn a_page_says_it_is_a_page_and_how_to_reach_the_rest() {
    let kernel = kernel();
    for n in 1..=5 {
        append(&kernel, &format!("Item{n}"), &[]);
    }
    let page = source(&kernel, "urn:iki:ledger:items", &[("limit", "2")]);
    assert_eq!(titles(&page), ["Item5", "Item4"], "{page}");
    assert!(page.contains("2 of 5 item(s)"), "{page}");
    assert!(page.contains("offset=2"), "{page}");
    assert!(
        !page.contains("\n2 item(s)\n"),
        "the page is worded as a total: {page}"
    );

    // Paging reaches every item exactly once, in the listing's order.
    let second = source(
        &kernel,
        "urn:iki:ledger:items",
        &[("limit", "2"), ("offset", "2")],
    );
    assert_eq!(titles(&second), ["Item3", "Item2"], "{second}");
    assert!(second.contains("3–4 of 5"), "{second}");
    let last = source(
        &kernel,
        "urn:iki:ledger:items",
        &[("limit", "2"), ("offset", "4")],
    );
    assert_eq!(titles(&last), ["Item1"], "{last}");
    assert!(last.contains("5–5 of 5"), "{last}");
    assert!(
        !last.contains("offset=6"),
        "the last page offers no next: {last}"
    );

    // A page past the end is empty and says how many there are.
    let past = source(&kernel, "urn:iki:ledger:items", &[("offset", "9")]);
    assert!(past.contains("5 item(s) match"), "{past}");

    // The whole set on one page keeps the line every existing reader greps.
    let whole = source(&kernel, "urn:iki:ledger:items", &[]);
    assert!(whole.contains("\n5 item(s)\n"), "{whole}");

    // A malformed offset is refused, never read as zero.
    assert!(try_verb(
        &kernel,
        Verb::Source,
        "urn:iki:ledger:items",
        &[("offset", "ten")]
    )
    .is_err());
}

/// The JSON face carries the total beside the page's count, so a machine reader can tell
/// them apart too — `count` was always the page's length and said so only in a doc comment.
#[test]
fn the_json_face_carries_the_total_and_the_offset() {
    let kernel = kernel();
    for n in 1..=5 {
        append(&kernel, &format!("Item{n}"), &[]);
    }
    let json = source(
        &kernel,
        "urn:iki:ledger:items",
        &[("limit", "2"), ("offset", "1"), ("as", "application/json")],
    );
    let doc: serde_json::Value = serde_json::from_str(&json).expect("the JSON face parses");
    assert_eq!(doc["count"], 2, "{json}");
    assert_eq!(doc["total"], 5, "{json}");
    assert_eq!(doc["offset"], 1, "{json}");
    assert_eq!(doc["items"][0]["title"], "Item4", "{json}");

    // The typed document reads both, and an older document without them still parses.
    let typed: ikigai_ledger::json::ItemsDocument =
        serde_json::from_str(&json).expect("the typed document parses");
    assert_eq!((typed.count, typed.total, typed.offset), (2, Some(5), 1));
    let older = r#"{"schema":1,"ledger":"default","count":0,"items":[],"unreadable":[]}"#;
    let older: ikigai_ledger::json::ItemsDocument =
        serde_json::from_str(older).expect("a document from before `total` still parses");
    assert_eq!((older.total, older.offset), (None, 0));
}

/// A filter's total is the filter's, not the ledger's.
#[test]
fn the_total_counts_what_the_filter_admits() {
    let kernel = kernel();
    for n in 1..=4 {
        append(&kernel, &format!("Item{n}"), &[("labels", "rust")]);
    }
    append(&kernel, "Other", &[]);
    let page = source(
        &kernel,
        "urn:iki:ledger:items",
        &[("labels", "rust"), ("limit", "1")],
    );
    assert!(page.contains("1 of 4 item(s)"), "{page}");
}

/// ★ **A page counts ITEMS, not result rows.** A hand edit that leaves an item two
/// `dcterms:modified` values makes it two rows of the listing query; a `LIMIT`/`OFFSET` over
/// rows would spend two slots on it, or put its halves on different pages. Paging groups by
/// subject first, so every item is on exactly one page, once.
#[test]
fn an_item_with_two_rows_takes_one_slot_on_one_page() {
    let kernel = kernel();
    let first = append_iri(&kernel, "First", &[]);
    append(&kernel, "Second", &[]);
    append(&kernel, "Third", &[]);
    // Two readable modified values on the oldest item: the later one makes it the newest.
    sink(
        &kernel,
        "urn:iki:store:graph-update",
        &[
            ("graph", "urn:iki:ledger:graph:default"),
            (
                "content",
                &format!(
                    "INSERT DATA {{ GRAPH <urn:iki:ledger:graph:default> {{ <{first}> \
                     <http://purl.org/dc/terms/modified> \
                     \"2030-01-01T00:00:00Z\"^^<http://www.w3.org/2001/XMLSchema#dateTime> }} }}"
                ),
            ),
        ],
    );
    let mut seen = Vec::new();
    for offset in ["0", "1", "2"] {
        let page = source(
            &kernel,
            "urn:iki:ledger:items",
            &[("limit", "1"), ("offset", offset)],
        );
        let on_page = titles(&page);
        assert_eq!(on_page.len(), 1, "offset {offset}: {page}");
        seen.extend(on_page);
    }
    assert_eq!(seen, ["First", "Third", "Second"]);
}
