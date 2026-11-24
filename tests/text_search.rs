//! **`text=` over bodies and comments, several words, and where each row matched** (ledger
//! #3803), on `items` (single-ledger and `ledgers=`), `count` and `next`.
//!
//! Until 0.23.1 `items text=` matched a case-insensitive substring of the TITLE only, so a
//! collaborator's agents searching items by what they say ran their own SPARQL, one call per
//! ledger per word, and joined the answers client-side. `match=` names the fields (`title`,
//! the default, `body`, `comments`), `search=` takes several words (`search_mode=all|any`),
//! `whole_word=true` matches at word boundaries, and each JSON row's `matches` says which
//! fields matched, with an excerpt, even when `omit=` leaves that text out.
//!
//! `repro_*` failed on main (`655e4da`) because of the defect: `match=` was ignored and the
//! body word was not found.

mod common;

use std::time::Instant;

use common::*;
use ikigai_core::{Capability, Error, Iri, Kernel, Request, Resolution, Scope, Space, Verb};
use serde_json::{json, Value};

const JSON: (&str, &str) = ("as", "application/json");

fn read(kernel: &Kernel, iri: &str, args: &[(&str, &str)]) -> Value {
    let mut all = vec![JSON];
    all.extend_from_slice(args);
    serde_json::from_str(&source(kernel, iri, &all)).expect("the JSON face parses")
}

fn items(kernel: &Kernel, args: &[(&str, &str)]) -> Value {
    read(kernel, "urn:iki:ledger:items", args)
}

fn titles(doc: &Value) -> Vec<String> {
    let mut titles: Vec<String> = doc["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|row| row["title"].as_str().unwrap().to_string())
        .collect();
    titles.sort();
    titles
}

fn row<'a>(doc: &'a Value, title: &str) -> &'a Value {
    doc["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["title"] == title)
        .unwrap_or_else(|| panic!("no row {title} in {doc}"))
}

fn comment(kernel: &Kernel, ledger: &str, item: &str, text: &str) {
    sink(
        kernel,
        &format!("urn:iki:ledger:{ledger}:comment"),
        &[("item", item), ("content", text)],
    );
}

fn refused(kernel: &Kernel, iri: &str, args: &[(&str, &str)]) -> (String, String) {
    match try_verb(kernel, Verb::Source, iri, args) {
        Err(Error::InvalidArgument { name, detail }) => (name, detail),
        other => panic!("expected InvalidArgument, got {other:?}"),
    }
}

/// `#1 Ruler` says `parser` in its body, `#2 Lexer` in a comment, `#3 Parser crash` in its
/// title, and `#4 Other` nowhere.
fn seeded() -> Kernel {
    let kernel = kernel();
    append(&kernel, "Ruler\n\nThe parser panics on a tab.", &[]);
    append(&kernel, "Lexer\n\nTokens only.", &[]);
    append(&kernel, "Parser crash", &[]);
    append(&kernel, "Other\n\nNothing here.", &[]);
    comment(
        &kernel,
        "default",
        "2",
        "seen in the parser too, on aarch64",
    );
    comment(&kernel, "default", "4", "unrelated");
    kernel
}

#[test]
fn repro_body_and_comment_words_are_not_found() {
    let kernel = seeded();
    let doc = items(&kernel, &[("text", "parser"), ("match", "body,comments")]);
    assert_eq!(doc["total"], 2, "{doc}");
    assert_eq!(titles(&doc), ["Lexer", "Ruler"]);
}

#[test]
fn match_defaults_to_the_title_and_names_each_field() {
    let kernel = seeded();
    let cases: [(&str, &[&str]); 6] = [
        ("title", &["Parser crash"]),
        ("body", &["Ruler"]),
        ("comments", &["Lexer"]),
        ("title,body", &["Parser crash", "Ruler"]),
        ("body,comments", &["Lexer", "Ruler"]),
        ("comments, title ,body", &["Lexer", "Parser crash", "Ruler"]),
    ];
    for (fields, want) in cases {
        let doc = items(&kernel, &[("text", "PARSER"), ("match", fields)]);
        assert_eq!(titles(&doc), want, "match={fields}");
        assert_eq!(doc["total"], want.len(), "match={fields}");
        let count = source(
            &kernel,
            "urn:iki:ledger:count",
            &[("text", "PARSER"), ("match", fields)],
        );
        assert_eq!(count.trim(), want.len().to_string(), "count match={fields}");
    }
    // No `match=` is the title, as it always was.
    assert_eq!(
        titles(&items(&kernel, &[("text", "parser")])),
        ["Parser crash"]
    );
}

#[test]
fn matches_names_one_hit_per_field_with_an_excerpt() {
    let kernel = kernel();
    let long = format!(
        "Needle in the title\n\n{} the needle sits here {}",
        "lead ".repeat(20),
        "tail ".repeat(20)
    );
    append(&kernel, &long, &[]);
    comment(&kernel, "default", "1", "no hit in this one");
    comment(&kernel, "default", "1", "a NEEDLE\nacross two lines");
    comment(&kernel, "default", "1", "a later needle, not shown");
    let doc = items(
        &kernel,
        &[("text", "needle"), ("match", "title,body,comments")],
    );
    let row = row(&doc, "Needle in the title");
    assert_eq!(
        row["matches"],
        json!([
            {"field": "title", "excerpt": "Needle in the title"},
            {"field": "body",
             "excerpt": "…lead lead lead lead lead lead lead the needle sits here tail tail tail tail tail tail…"},
            {"field": "comments", "excerpt": "a NEEDLE across two lines"},
        ]),
        "{row}"
    );
    // A field searched but not holding the term has no entry.
    let doc = items(
        &kernel,
        &[("text", "title"), ("match", "title,body,comments")],
    );
    assert_eq!(
        row_matches(&doc, "Needle in the title"),
        json!([{"field": "title", "excerpt": "Needle in the title"}])
    );
}

fn row_matches(doc: &Value, title: &str) -> Value {
    row(doc, title)["matches"].clone()
}

#[test]
fn matches_hold_when_the_row_leaves_the_text_out() {
    let kernel = seeded();
    let doc = items(
        &kernel,
        &[
            ("text", "parser"),
            ("match", "body,comments"),
            ("omit", "body,comments"),
        ],
    );
    let ruler = row(&doc, "Ruler");
    assert_eq!(ruler["body"], "");
    assert_eq!(ruler["comments"], json!([]));
    assert_eq!(
        ruler["matches"],
        json!([{"field": "body", "excerpt": "The parser panics on a tab."}])
    );
    let lexer = row(&doc, "Lexer");
    assert_eq!(lexer["comments"], json!([]));
    assert_eq!(
        lexer["matches"],
        json!([{"field": "comments", "excerpt": "seen in the parser too, on aarch64"}])
    );
    // And under comments=last, when the matching comment is not the one shown.
    comment(&kernel, "default", "2", "a later word");
    let doc = items(
        &kernel,
        &[
            ("text", "parser"),
            ("match", "comments"),
            ("comments", "last"),
        ],
    );
    let lexer = row(&doc, "Lexer");
    assert_eq!(lexer["comments"][0]["text"], "a later word");
    assert_eq!(lexer["matches"][0]["field"], "comments");
}

#[test]
fn several_words_must_all_match_in_any_field_unless_any() {
    let kernel = seeded();
    let both = |words: &str, mode: &str| {
        titles(&items(
            &kernel,
            &[
                ("search", words),
                ("search_mode", mode),
                ("match", "title,body,comments"),
            ],
        ))
    };
    // `lexer` is a title and `aarch64` a comment: different fields, one item.
    assert_eq!(both("lexer aarch64", "all"), ["Lexer"]);
    assert_eq!(both("  lexer   AARCH64 ", "all"), ["Lexer"]);
    assert_eq!(both("lexer tab", "all"), Vec::<String>::new());
    assert_eq!(both("lexer tab", "any"), ["Lexer", "Ruler"]);
    assert_eq!(both("parser", "all"), ["Lexer", "Parser crash", "Ruler"]);
    // `text=` stays ONE substring: the words in that order, spaces and all.
    let text = |value: &str| {
        titles(&items(
            &kernel,
            &[("text", value), ("match", "title,body,comments")],
        ))
    };
    assert_eq!(text("parser panics"), ["Ruler"]);
    assert_eq!(text("panics parser"), Vec::<String>::new());
    // Beside `search=`, both must match.
    let doc = items(
        &kernel,
        &[
            ("text", "parser"),
            ("search", "tab nothing"),
            ("search_mode", "any"),
            ("match", "title,body,comments"),
        ],
    );
    assert_eq!(titles(&doc), ["Ruler"]);
    // A row's matches name every field holding any of the terms.
    let doc = items(
        &kernel,
        &[
            ("search", "lexer aarch64"),
            ("match", "title,body,comments"),
        ],
    );
    assert_eq!(
        row_matches(&doc, "Lexer"),
        json!([
            {"field": "title", "excerpt": "Lexer"},
            {"field": "comments", "excerpt": "seen in the parser too, on aarch64"},
        ])
    );
}

#[test]
fn whole_word_matches_at_word_boundaries_and_reads_no_pattern() {
    let kernel = kernel();
    append(&kernel, "The rule\n\nA ruler, a c++ note and a.b.", &[]);
    append(&kernel, "Rulers\n\naxb and c+++", &[]);
    let found = |term: &str, whole: &str| {
        titles(&items(
            &kernel,
            &[
                ("text", term),
                ("match", "title,body"),
                ("whole_word", whole),
            ],
        ))
    };
    assert_eq!(found("rule", "false"), ["Rulers", "The rule"]);
    assert_eq!(found("rule", "true"), ["The rule"]);
    assert_eq!(found("RULER", "true"), ["The rule"]);
    // `.` and `+` are characters, never a pattern.
    assert_eq!(found("a.b", "true"), ["The rule"]);
    assert_eq!(found("c++", "true"), ["Rulers", "The rule"]);
    assert_eq!(found("c+++", "true"), ["Rulers"]);
    let doc = items(
        &kernel,
        &[
            ("text", "rule"),
            ("match", "title,body"),
            ("whole_word", "true"),
        ],
    );
    // The body says `ruler`, which is not the word `rule`: no body entry.
    assert_eq!(
        row_matches(&doc, "The rule"),
        json!([{"field": "title", "excerpt": "The rule"}])
    );
    let doc = items(
        &kernel,
        &[
            ("text", "c++"),
            ("match", "title,body"),
            ("whole_word", "true"),
        ],
    );
    assert_eq!(
        row_matches(&doc, "The rule"),
        json!([{"field": "body", "excerpt": "A ruler, a c++ note and a.b."}])
    );
    // Words too.
    let doc = items(
        &kernel,
        &[
            ("search", "rule note"),
            ("match", "title,body"),
            ("whole_word", "true"),
        ],
    );
    assert_eq!(titles(&doc), ["The rule"]);
}

/// ★ A term reaches the query as a literal (`literal()`), and under `whole_word` as a
/// regex-escaped literal: one holding `"`, `\`, `)` and a newline is matched as those
/// characters, and one written to close the literal and add `|| true` matches nothing.
#[test]
fn a_hostile_term_is_matched_literally_and_cannot_change_the_query() {
    let kernel = kernel();
    let hostile = "say \"hi\" \\ (x) )\nnext";
    append(&kernel, &format!("Hostile\n\nbefore {hostile} after"), &[]);
    append(&kernel, "Plain\n\nsay hi x next", &[]);
    comment(&kernel, "default", "2", "nothing (x) here");
    for whole in ["false", "true"] {
        for fields in ["body", "comments", "body,comments", "title,body,comments"] {
            let doc = items(
                &kernel,
                &[("text", hostile), ("match", fields), ("whole_word", whole)],
            );
            let want: &[&str] = if fields.contains("body") {
                &["Hostile"]
            } else {
                &[]
            };
            assert_eq!(titles(&doc), want, "match={fields} whole_word={whole}");
            for escape in ["\")) || true || ((\"", "x\\\") || true #", ".*", "\\b|"] {
                let doc = items(
                    &kernel,
                    &[("search", escape), ("match", fields), ("whole_word", whole)],
                );
                let found = titles(&doc);
                assert!(
                    found.len() < 2,
                    "{escape:?} match={fields} whole_word={whole} matched {found:?}"
                );
            }
        }
    }
    // A `)` is a character: it matches where one is written, under both modes.
    for whole in ["false", "true"] {
        let doc = items(
            &kernel,
            &[
                ("text", "(x)"),
                ("match", "body,comments"),
                ("whole_word", whole),
            ],
        );
        assert_eq!(titles(&doc), ["Hostile", "Plain"], "whole_word={whole}");
    }
    let doc = items(&kernel, &[("text", hostile), ("match", "body")]);
    assert_eq!(
        row_matches(&doc, "Hostile"),
        json!([{"field": "body", "excerpt": "before say \"hi\" \\ (x) ) next after"}])
    );
}

/// ★ A caller passing none of the new arguments gets what it got before, byte for byte: no
/// `matches` in a document read without a text search, and the same bytes from every face
/// when the new arguments are given as their defaults.
#[test]
fn the_default_is_byte_identical() {
    let kernel = seeded();
    for face in ["text/plain", "text/turtle", "application/json"] {
        for base in [
            vec![],
            vec![("text", "parser")],
            vec![("text", "PARSER"), ("status", "all")],
        ] {
            let mut plain = base.clone();
            plain.push(("as", face));
            let mut named = plain.clone();
            named.extend([
                ("match", "title"),
                ("search_mode", "all"),
                ("whole_word", "false"),
            ]);
            for resource in ["items", "count", "next"] {
                if resource == "count" && face == "text/turtle" {
                    continue;
                }
                let iri = format!("urn:iki:ledger:{resource}");
                assert_eq!(
                    source(&kernel, &iri, &plain),
                    source(&kernel, &iri, &named),
                    "{resource} {face} {base:?}"
                );
            }
        }
    }
    // Read without a search, no row carries the field at all.
    for resource in ["items", "next", "item:1"] {
        let raw = source(&kernel, &format!("urn:iki:ledger:{resource}"), &[JSON]);
        assert!(!raw.contains("\"matches\""), "{resource}: {raw}");
    }
    // `text=` over titles keeps its plain face: no `matched in` lines.
    let plain = source(&kernel, "urn:iki:ledger:items", &[("text", "parser")]);
    assert!(!plain.contains("matched in"), "{plain}");
    // And with a search, every JSON row says where it matched.
    let doc = items(&kernel, &[("text", "parser")]);
    assert_eq!(
        row_matches(&doc, "Parser crash"),
        json!([{"field": "title", "excerpt": "Parser crash"}])
    );
}

#[test]
fn the_plain_face_says_where_a_body_or_comment_matched() {
    let kernel = seeded();
    let plain = source(
        &kernel,
        "urn:iki:ledger:items",
        &[("text", "parser"), ("match", "title,body,comments")],
    );
    let lines: Vec<&str> = plain.lines().collect();
    let at = |prefix: &str| {
        lines
            .iter()
            .position(|l| l.trim_start().starts_with(prefix))
            .unwrap_or_else(|| panic!("no line {prefix}: {plain}"))
    };
    let ruler = at("#1 ");
    assert_eq!(
        lines[ruler + 1],
        "  #1 matched in the body: The parser panics on a tab."
    );
    let lexer = at("#2 ");
    assert_eq!(
        lines[lexer + 1],
        "  #2 matched in a comment: seen in the parser too, on aarch64"
    );
    // A title match is the item's line itself, with no line of its own.
    let crash = at("#3 ");
    assert!(!lines[crash + 1].contains("matched in"), "{plain}");
    assert!(plain.contains("3 item(s)"), "{plain}");
}

#[test]
fn the_search_composes_with_the_other_filters_and_the_union() {
    let kernel = kernel();
    for ledger in ["alpha", "beta"] {
        let append = format!("urn:iki:ledger:{ledger}:append");
        sink(&kernel, &append, &[("content", "Parent")]);
        sink(
            &kernel,
            &append,
            &[("content", "Child\n\nwidget broke"), ("labels", "bug")],
        );
        sink(
            &kernel,
            &append,
            &[("content", "Other child"), ("labels", "bug,later")],
        );
        sink(
            &kernel,
            &format!("urn:iki:ledger:{ledger}:link"),
            &[("item", "2"), ("type", "parent"), ("content", "1")],
        );
        sink(
            &kernel,
            &format!("urn:iki:ledger:{ledger}:link"),
            &[("item", "3"), ("type", "parent"), ("content", "1")],
        );
        comment(&kernel, ledger, "3", &format!("the widget in {ledger}"));
    }
    let union = |extra: &[(&str, &str)]| {
        let mut args = vec![
            ("ledgers", "alpha,beta"),
            ("text", "widget"),
            ("match", "body,comments"),
        ];
        args.extend_from_slice(extra);
        let doc = items(&kernel, &args);
        let mut displays: Vec<String> = doc["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["display"].as_str().unwrap().to_string())
            .collect();
        displays.sort();
        if !extra.iter().any(|(name, _)| *name == "limit") {
            assert_eq!(doc["total"], displays.len());
        }
        displays
    };
    assert_eq!(union(&[]), ["alpha#2", "alpha#3", "beta#2", "beta#3"]);
    assert_eq!(union(&[("labels", "later")]), ["alpha#3", "beta#3"]);
    assert_eq!(union(&[("without", "later")]), ["alpha#2", "beta#2"]);
    assert_eq!(
        union(&[("parent", "1")]),
        ["alpha#2", "alpha#3", "beta#2", "beta#3"]
    );
    assert_eq!(union(&[("sort", "number")]).len(), 4);
    assert_eq!(union(&[("limit", "1"), ("offset", "1")]).len(), 1);
    sink(&kernel, "urn:iki:ledger:beta:close", &[("item", "3")]);
    assert_eq!(union(&[("status", "closed")]), ["beta#3"]);
    assert_eq!(union(&[("status", "all")]).len(), 4);
    // Each row's comments are read through its own ledger's graph.
    let doc = items(
        &kernel,
        &[
            ("ledgers", "alpha,beta"),
            ("text", "widget"),
            ("match", "comments"),
            ("omit", "comments"),
            ("status", "all"),
        ],
    );
    for row in doc["items"].as_array().unwrap() {
        let ledger = row["display"].as_str().unwrap().split('#').next().unwrap();
        assert_eq!(
            row["matches"],
            json!([{"field": "comments", "excerpt": format!("the widget in {ledger}")}])
        );
    }
    // `about` and `parent` on a single ledger.
    let about = "urn:repo:file:x/src/a.rs";
    sink(
        &kernel,
        "urn:iki:ledger:append",
        &[("content", "Filed\n\nwidget here"), ("about", about)],
    );
    append(&kernel, "Elsewhere\n\nwidget there", &[]);
    let doc = items(
        &kernel,
        &[("text", "widget"), ("match", "body"), ("about", about)],
    );
    assert_eq!(titles(&doc), ["Filed"]);
}

#[test]
fn next_takes_the_search_and_says_where_each_row_matched() {
    let kernel = seeded();
    let doc = read(
        &kernel,
        "urn:iki:ledger:next",
        &[
            ("text", "parser"),
            ("match", "body,comments"),
            ("omit", "body,comments"),
            ("limit", "0"),
        ],
    );
    let ranked: Vec<(&str, &Value)> = doc["ranking"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["item"]["title"].as_str().unwrap(), &r["item"]["matches"]))
        .collect();
    let mut names: Vec<&str> = ranked.iter().map(|(t, _)| *t).collect();
    names.sort();
    assert_eq!(names, ["Lexer", "Ruler"], "{doc}");
    for (title, matches) in ranked {
        let field = if title == "Ruler" { "body" } else { "comments" };
        assert_eq!(matches[0]["field"], field, "{title}");
    }
    let plain = source(
        &kernel,
        "urn:iki:ledger:next",
        &[("search", "lexer aarch64"), ("match", "title,comments")],
    );
    assert!(plain.contains("Lexer"), "{plain}");
    assert!(!plain.contains("Ruler"), "{plain}");
    // An exclusion is matched too.
    sink(
        &kernel,
        "urn:iki:ledger:claim",
        &[("item", "1"), ("content", "worker-1")],
    );
    let doc = read(
        &kernel,
        "urn:iki:ledger:next",
        &[("text", "tab"), ("match", "body")],
    );
    assert_eq!(doc["ranking"], json!([]));
    assert_eq!(
        doc["excluded"][0]["item"]["matches"],
        json!([{"field": "body", "excerpt": "The parser panics on a tab."}])
    );
}

#[test]
fn each_new_argument_is_declared_and_refused_in_words() {
    let kernel = seeded();
    for resource in ["items", "count", "next"] {
        let iri = format!("urn:iki:ledger:{resource}");
        for (arg, value, says) in [
            ("match", "titles", "`titles` is not a field"),
            ("match", "title,", "`` is not a field"),
            ("search_mode", "most", "`most` is not one of all, any"),
            ("whole_word", "yes", "`yes` is not `true` or `false`"),
        ] {
            let (name, detail) = refused(&kernel, &iri, &[("text", "x"), (arg, value)]);
            assert_eq!(name, arg, "{resource}");
            assert!(detail.contains(says), "{resource} {arg}: {detail}");
        }
    }
    // Declared on each resource that takes them, the enums with their values.
    let space = ikigai_ledger::space();
    for resource in ["items", "count", "next"] {
        let request = Request::new(
            Verb::Meta,
            Iri::parse(format!("urn:iki:ledger:{resource}")).unwrap(),
        );
        let Resolution::Hit(resolved) = space.resolve(&request, &Scope::empty()) else {
            panic!("{resource} resolves");
        };
        let description = resolved.endpoint.describe();
        let input = |name: &str| {
            description
                .inputs
                .iter()
                .find(|input| input.name == name)
                .unwrap_or_else(|| panic!("{resource} declares {name}"))
        };
        for name in ["text", "match", "search", "search_mode", "whole_word"] {
            assert!(!input(name).required, "{resource} {name}");
        }
        assert_eq!(input("match").default.as_deref(), Some("title"));
        assert_eq!(input("search_mode").one_of, ["all", "any"]);
        assert_eq!(input("search_mode").default.as_deref(), Some("all"));
        assert_eq!(input("whole_word").one_of, ["true", "false"]);
        assert_eq!(input("whole_word").default.as_deref(), Some("false"));
    }
}

/// ★ Comments are read under the grant the listing already needs: the module's read grant and
/// the ledger graph's store read grant, nothing else.
#[test]
fn the_narrow_read_grants_search_comments() {
    let kernel = kernel();
    sink(
        &kernel,
        "urn:iki:ledger:library:append",
        &[("content", "Shelf\n\nOak.")],
    );
    comment(&kernel, "library", "1", "the hinge squeaks");
    let caller = Capability::scoped([
        "urn:cap:ledger:read:library".to_string(),
        graph_read("library"),
    ]);
    for resource in ["items", "next"] {
        let answer = try_as(
            &kernel,
            &caller,
            Verb::Source,
            &format!("urn:iki:ledger:library:{resource}"),
            &[
                ("search", "hinge oak"),
                ("match", "body,comments"),
                ("omit", "comments"),
                JSON,
            ],
        )
        .unwrap_or_else(|e| panic!("{resource} under the read row: {e}"));
        let doc: Value = serde_json::from_str(&answer).unwrap();
        let row = if resource == "items" {
            &doc["items"][0]
        } else {
            &doc["ranking"][0]["item"]
        };
        assert_eq!(
            row["matches"],
            json!([
                {"field": "body", "excerpt": "Oak."},
                {"field": "comments", "excerpt": "the hinge squeaks"},
            ]),
            "{resource}: {doc}"
        );
    }
    // Without the graph's store grant it is refused, as any listing is.
    let module_only = Capability::scoped(["urn:cap:ledger:read:library".to_string()]);
    assert!(try_as(
        &kernel,
        &module_only,
        Verb::Source,
        "urn:iki:ledger:library:items",
        &[("text", "hinge"), ("match", "comments")],
    )
    .is_err());
}

// ---------------------------------------------------------------------- at scale

const LIVE: &str = "urn:iki:ledger:graph:default";
const NS: &str = "https://ikigai-rs.dev/ns/ledger#";
const DC: &str = "http://purl.org/dc/terms/";
const XSD_DATE_TIME: &str = "http://www.w3.org/2001/XMLSchema#dateTime";

/// `items` open items, each with a body, and `comments` comments spread over them, written
/// straight into the graph a few hundred at a time, as `append` and `comment` write them.
/// Every 500th item's body and every 700th comment hold `needle`.
fn bulk(kernel: &Kernel, items: usize, comments: usize) {
    let when = |k: usize| {
        format!(
            "\"2026-09-15T{:02}:{:02}:{:02}.{:03}Z\"^^<{XSD_DATE_TIME}>",
            (k / 3_600_000) % 24,
            (k / 60_000) % 60,
            (k / 1000) % 60,
            k % 1000
        )
    };
    let item = |k: usize| format!("urn:iki:ledger:default:item:bulk{k:06}0000000000000");
    let mut data = Vec::new();
    for k in 1..=items {
        let word = if k % 500 == 0 { "needle" } else { "hay" };
        data.push(format!(
            "<{iri}> a <{NS}Item> ; <{NS}number> {k} ; <{DC}title> \"step {k}\" ; \
             <{NS}body> \"the body of step {k} holds {word} and more words besides\" ; \
             <{NS}status> <{NS}open> ; <{DC}created> {t} ; <{DC}modified> {t} .\n",
            iri = item(k),
            t = when(k)
        ));
    }
    for c in 1..=comments {
        let word = if c % 700 == 0 { "needle" } else { "straw" };
        data.push(format!(
            "<urn:iki:ledger:default:comment:bulk{c:06}000000000000> a <{NS}Comment> ; \
             <{NS}onItem> <{on}> ; <{NS}body> \"comment {c} says {word} at some length\" ; \
             <{DC}created> {t} .\n",
            on = item(1 + c % items),
            t = when(items + c)
        ));
    }
    for chunk in data.chunks(400) {
        sink(
            kernel,
            "urn:iki:store:graph-update",
            &[
                ("graph", LIVE),
                (
                    "content",
                    &format!("INSERT DATA {{ GRAPH <{LIVE}> {{ {} }} }}", chunk.concat()),
                ),
            ],
        );
    }
}

/// ★ The constraint the collaborator's own query broke: over bodies and comments, a search
/// must start from the text, or it runs past the store's 5 s budget. 4,000 items and 4,000
/// comments, more than the 664 and 2,454 that took theirs past it; each read must stay well
/// inside the budget. The timings are printed (`--nocapture`) for the pull request.
#[test]
fn a_search_of_bodies_and_comments_stays_well_inside_the_budget_at_scale() {
    let kernel = kernel();
    bulk(&kernel, 4_000, 4_000);
    // 8 bodies and 5 comments hold the needle; the comments are on 5 other items.
    for (label, args, want) in [
        (
            "items text=needle match=body,comments",
            vec![("text", "needle"), ("match", "body,comments")],
            13,
        ),
        (
            "items search=\"needle says\" match=body,comments",
            // `says` is in every comment and every item has one, so this is `needle`'s 13.
            vec![("search", "needle says"), ("match", "body,comments")],
            13,
        ),
        (
            "items search=\"needle step\" match=body search_mode=all",
            vec![("search", "needle step"), ("match", "body")],
            8,
        ),
        (
            "items text=needle match=title,body,comments whole_word=true",
            vec![
                ("text", "needle"),
                ("match", "title,body,comments"),
                ("whole_word", "true"),
            ],
            13,
        ),
        (
            "items text=needle match=comments sort=number",
            vec![
                ("text", "needle"),
                ("match", "comments"),
                ("sort", "number"),
            ],
            5,
        ),
    ] {
        let mut all = args.clone();
        all.push(JSON);
        let start = Instant::now();
        let doc: Value =
            serde_json::from_str(&source(&kernel, "urn:iki:ledger:items", &all)).unwrap();
        let took = start.elapsed();
        println!("{label}: {:.0} ms", took.as_secs_f64() * 1000.0);
        assert_eq!(doc["total"], want, "{label}");
        assert!(
            doc["items"]
                .as_array()
                .unwrap()
                .iter()
                .all(|row| !row["matches"].as_array().unwrap().is_empty()),
            "{label}"
        );
        assert!(took.as_secs_f64() < 2.5, "{label} took {took:?}");
    }
    let start = Instant::now();
    let doc = read(
        &kernel,
        "urn:iki:ledger:next",
        &[
            ("text", "needle"),
            ("match", "body,comments"),
            ("limit", "0"),
        ],
    );
    let took = start.elapsed();
    println!(
        "next text=needle match=body,comments: {:.0} ms",
        took.as_secs_f64() * 1000.0
    );
    assert_eq!(doc["ready"], 13);
    assert!(took.as_secs_f64() < 2.5, "next took {took:?}");
}

#[test]
fn phrase_requires_words_in_order_and_adjacent() {
    let kernel = kernel();
    append(&kernel, "Lease ran out", &[]);
    append(&kernel, "Lease nearly ran out", &[]);
    append(&kernel, "Ran out of the lease", &[]);
    assert_eq!(
        titles(&items(&kernel, &[("phrase", "lease ran out")])),
        ["Lease ran out"]
    );
}

#[test]
fn relevance_requires_a_ready_text_index() {
    let kernel = seeded();
    let result = try_verb(
        &kernel,
        Verb::Source,
        "urn:iki:ledger:items",
        &[("sort", "relevance"), ("text", "parser")],
    );
    match result {
        Err(Error::Unavailable(detail)) => assert_eq!(
            detail,
            "Choose another sort or narrow your search because the text index cannot rank this search in default, where nothing was listed"
        ),
        other => panic!("expected the relevance refusal, got {other:?}"),
    }
    assert_eq!(
        refused(&kernel, "urn:iki:ledger:items", &[("sort", "relevance")]),
        ("sort".into(), "Give text=, search= or phrase= to sort by relevance in default, where nothing was listed".into())
    );
}

const DEFAULT_GRAPH: &str = "urn:iki:ledger:graph:default";
const INDEX_STATUS: &str = "urn:iki:store:text-index:graph:urn%3Aiki%3Aledger%3Agraph%3Adefault";
const GRAPH_TEXT: &str = "urn:iki:store:graph-text";

fn indexed(store: ikigai_store::DurableStore) -> Kernel {
    use ikigai_ledger::vocabulary as v;
    kernel_over(
        store
            .with_text_index(
                ikigai_store::TextEnrollment::new()
                    .graph(DEFAULT_GRAPH)
                    .predicate_weighted(v::ext::TITLE, 2.0)
                    .predicate(v::BODY),
            )
            .unwrap(),
    )
}

fn wait_for_index(kernel: &Kernel) {
    let until = Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let status = source(kernel, INDEX_STATUS, &[]);
        if status.lines().any(|line| line == "state: ready") {
            break;
        }
        assert!(Instant::now() < until, "{status}");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn commit_index(kernel: &Kernel, documents: usize) {
    let until = Instant::now() + std::time::Duration::from_secs(5);
    let expected = format!("documents: {documents}\n");
    loop {
        source(
            kernel,
            GRAPH_TEXT,
            &[("graph", DEFAULT_GRAPH), ("words", "indexwarmup")],
        );
        let status = source(kernel, INDEX_STATUS, &[]);
        if status.contains(&expected) && status.contains("pending: 0\n") {
            return;
        }
        assert!(Instant::now() < until, "{status}");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn indexed_and_scanned_searches_agree() {
    let scan = kernel();
    let index = indexed(ikigai_store::DurableStore::in_memory().unwrap());
    wait_for_index(&index);
    for kernel in [&scan, &index] {
        append(kernel, "Lease ran out", &[("priority", "3")]);
        append(kernel, "Lease nearly ran out\n\nA parser module", &[]);
        append(kernel, "ParserCode\n\nLease ran out", &[("priority", "0")]);
        append(kernel, "Leaſe and parser", &[]);
        append(kernel, "parser_module", &[]);
        comment(kernel, "default", "4", "The lease—ran out yesterday");
    }
    commit_index(&index, 8);
    let cases: &[&[(&str, &str)]] = &[
        &[("text", "parser")],
        &[("text", "lease"), ("whole_word", "true")],
        &[
            ("search", "lease parser"),
            ("whole_word", "true"),
            ("match", "title,body,comments"),
        ],
        &[
            ("search", "lease parser"),
            ("search_mode", "any"),
            ("whole_word", "true"),
        ],
        &[("search", "lease parser"), ("search_mode", "any")],
        &[
            ("phrase", "lease ran out"),
            ("match", "title,body,comments"),
        ],
        &[
            ("phrase", "lease ran out"),
            ("text", "parser"),
            ("match", "title,body,comments"),
        ],
        &[("phrase", "lease ran out"), ("match", "comments")],
        &[("phrase", "no matches")],
    ];
    for args in cases {
        for endpoint in ["urn:iki:ledger:items", "urn:iki:ledger:count"] {
            assert_eq!(
                source(&scan, endpoint, args),
                source(&index, endpoint, args),
                "{endpoint} {args:?}"
            );
        }
        let scan_next = read(&scan, "urn:iki:ledger:next", args);
        let index_next = read(&index, "urn:iki:ledger:next", args);
        let ranking = |doc: Value| {
            doc["ranking"]
                .as_array()
                .expect("next ranking")
                .iter()
                .map(|entry| entry["item"]["number"].as_i64().expect("item number"))
                .collect::<Vec<_>>()
        };
        assert_eq!(ranking(scan_next), ranking(index_next), "{args:?}");
    }

    // The equality checks include queries that must scan. Verify one supported phrase
    // actually ran the RDF check over index candidates.
    use ikigai_core::{ArgRef, TraceEvent, Tracer};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Trace(Mutex<Vec<TraceEvent>>);
    impl Tracer for Trace {
        fn record(&self, event: TraceEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    let trace = Arc::new(Trace::default());
    let request = Request::new(Verb::Source, Iri::parse("urn:iki:ledger:count").unwrap())
        .with_arg("phrase", ArgRef::Inline(b"lease ran out".to_vec()));
    futures::executor::block_on(index.issue_traced(request, &Capability::root(), trace.clone()))
        .unwrap();
    assert!(trace.0.lock().unwrap().iter().any(|event| {
        event
            .notes
            .iter()
            .any(|(name, value)| name == "text-index" && value == "checked candidates")
    }));
}

#[test]
fn relevance_uses_the_best_document_with_double_weight_for_title() {
    let kernel = indexed(ikigai_store::DurableStore::in_memory().unwrap());
    wait_for_index(&kernel);
    append(&kernel, "Marker", &[("priority", "4")]);
    append(&kernel, "Newer\n\nMarker", &[("priority", "0")]);
    comment(&kernel, "default", "2", "Marker");
    append(&kernel, "MarkerCode", &[]);
    commit_index(&kernel, 5);

    let search = [("text", "marker"), ("match", "title,body,comments")];
    let mut ranked = search.to_vec();
    ranked.push(("sort", "relevance"));
    let doc = items(&kernel, &ranked);
    let rows = doc["items"].as_array().unwrap();
    assert_eq!(doc["total"], 3);
    assert_eq!(rows[0]["number"], 1);
    assert_eq!(rows[1]["number"], 2);
    let title = rows[0]["score"].as_f64().unwrap();
    let body = rows[1]["score"].as_f64().unwrap();
    assert!(body > 0.0);
    assert_eq!(title, 2.0 * body);
    assert_eq!(rows[2]["score"], 0.0);
    assert!(
        source(&kernel, "urn:iki:ledger:items", &ranked).ends_with("Most relevant items first.\n")
    );
    assert!(items(&kernel, &search)["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|row| row.get("score").is_none()));
    assert_eq!(
        source(&kernel, "urn:iki:ledger:next", &ranked),
        source(&kernel, "urn:iki:ledger:next", &search)
    );
}

#[test]
fn purge_waits_for_on_disk_compaction_of_live_and_earlier_deleted_text() {
    let dir =
        std::env::temp_dir().join(format!("ikigai-ledger-index-purge-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    {
        let kernel = indexed(ikigai_store::DurableStore::open(&dir).unwrap());
        wait_for_index(&kernel);
        for (item, secret, recoverable) in [
            ("2", "zqxjvsecretword", false),
            ("4", "obscureindexmarker", true),
        ] {
            append(&kernel, "Keep another document alive", &[]);
            append(&kernel, &format!("Keep another document\n\n{secret}"), &[]);
            commit_index(&kernel, if recoverable { 4 } else { 3 });
            let args = [("graph", DEFAULT_GRAPH), ("words", secret)];
            let result: Value = read(&kernel, GRAPH_TEXT, &args);
            assert_eq!(result["hits"].as_array().unwrap().len(), 1);
            let terms_before = term_files(&dir.join("text-index"));
            assert!(!terms_before.is_empty());
            if recoverable {
                delete(&kernel, &format!("urn:iki:ledger:item:{item}"), &[]);
            }
            delete(&kernel, "urn:iki:ledger:purge", &[("content", item)]);
            let terms_after = term_files(&dir.join("text-index"));
            assert!(
                terms_before.iter().all(|file| !terms_after.contains(file)),
                "old term dictionaries survived compaction: {terms_before:?} and {terms_after:?}"
            );
            assert!(
                !files_contain(&dir.join("text-index"), secret.as_bytes()),
                "{secret} still appears in an index file after purge"
            );
            assert_eq!(read(&kernel, GRAPH_TEXT, &args)["hits"], json!([]));
            let status = source(&kernel, INDEX_STATUS, &[]);
            assert!(
                status.contains("deleted documents awaiting compaction: 0\n"),
                "{status}"
            );
        }
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

fn files_contain(dir: &std::path::Path, needle: &[u8]) -> bool {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .any(|entry| {
            let path = entry.path();
            if path.is_dir() {
                files_contain(&path, needle)
            } else {
                std::fs::read(path)
                    .is_ok_and(|bytes| bytes.windows(needle.len()).any(|chunk| chunk == needle))
            }
        })
}

fn term_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .flat_map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                term_files(&path)
            } else if path.extension().is_some_and(|ext| ext == "term") {
                vec![path]
            } else {
                Vec::new()
            }
        })
        .collect()
}
