mod common;

use common::*;

#[test]
fn named_ledgers_are_listed_as_one_union() {
    let kernel = kernel();
    for (ledger, title) in [("alpha", "First"), ("beta", "Second")] {
        sink(
            &kernel,
            &format!("urn:iki:ledger:{ledger}:append"),
            &[("content", title)],
        );
    }
    let listing = source(
        &kernel,
        "urn:iki:ledger:items",
        &[("ledgers", "alpha,beta")],
    );
    assert!(listing.contains("First"), "{listing}");
    assert!(listing.contains("Second"), "{listing}");
    assert!(listing.contains("2 item(s)"), "{listing}");
}

use ikigai_core::{Capability, Verb};
use serde_json::{json, Value};

fn file(kernel: &ikigai_core::Kernel, ledger: &str, title: &str, labels: &str) -> String {
    sink(
        kernel,
        &format!("urn:iki:ledger:{ledger}:append"),
        &[("content", title), ("labels", labels)],
    )
    .split_whitespace()
    .nth(1)
    .unwrap()
    .to_string()
}

fn document(kernel: &ikigai_core::Kernel, args: &[(&str, &str)]) -> Value {
    let mut args = args.to_vec();
    args.push(("as", "application/json"));
    serde_json::from_str(&source(kernel, "urn:iki:ledger:items", &args)).unwrap()
}

fn titles(doc: &Value) -> Vec<&str> {
    doc["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["title"].as_str().unwrap())
        .collect()
}

#[test]
fn union_selection_replaces_the_bound_ledger_and_deduplicates_names() {
    let kernel = kernel();
    file(&kernel, "alpha", "Alpha", "");
    file(&kernel, "beta", "Beta", "");
    append(&kernel, "Default", &[]);
    let doc = document(&kernel, &[("ledgers", " beta,alpha, beta ")]);
    assert_eq!(titles(&doc), ["Beta", "Alpha"]);
    assert_eq!(doc["ledgers"], json!(["alpha", "beta"]));
    assert_eq!(doc["ledger"], "default");
    assert_eq!(doc["total"], 2);
    let single = document(&kernel, &[]);
    assert_eq!(titles(&single), ["Default"]);
    assert!(single.get("ledgers").is_none());
    let named = source(
        &kernel,
        "urn:iki:ledger:alpha:items",
        &[("ledgers", "beta"), ("as", "application/json")],
    );
    let named: Value = serde_json::from_str(&named).unwrap();
    assert_eq!(named["ledger"], "alpha");
    assert_eq!(titles(&named), ["Beta"]);
}

#[test]
fn union_omits_each_kind_of_unreadable_ledger_without_requiring_the_bound_ledger() {
    let kernel = kernel();
    for ledger in ["alpha", "private", "module-only", "store-only"] {
        file(&kernel, ledger, &format!("Title-{ledger}"), "");
    }
    let cap = Capability::scoped([
        "urn:cap:ledger:read:alpha".to_string(),
        graph_read("alpha"),
        "urn:cap:ledger:read:module-only".to_string(),
        graph_read("store-only"),
    ]);
    for face in ["text/plain", "application/json", "text/turtle"] {
        let listing = try_as(
            &kernel,
            &cap,
            Verb::Source,
            "urn:iki:ledger:items",
            &[
                ("ledgers", "private,module-only,alpha,store-only"),
                ("as", face),
            ],
        )
        .unwrap();
        assert!(listing.contains("Title-alpha"), "{listing}");
        for hidden in ["private", "module-only", "store-only"] {
            assert!(!listing.contains(hidden), "{listing}");
        }
    }
    let count = try_as(
        &kernel,
        &cap,
        Verb::Source,
        "urn:iki:ledger:count",
        &[("ledgers", "private,module-only,alpha,store-only")],
    )
    .unwrap();
    assert_eq!(count, "1\n");
    // All selected ledgers may be unreadable. Do not reveal whether they exist.
    let empty = try_as(
        &kernel,
        &cap,
        Verb::Source,
        "urn:iki:ledger:items",
        &[("ledgers", "private,missing"), ("as", "application/json")],
    )
    .unwrap();
    let empty: Value = serde_json::from_str(&empty).unwrap();
    assert_eq!(empty["items"], json!([]));
    assert_eq!(empty["ledgers"], json!([]));
    assert_eq!(empty["total"], 0);
    // Omitting the plural argument still enforces the resource's singular binding.
    assert!(try_as(&kernel, &cap, Verb::Source, "urn:iki:ledger:items", &[]).is_err());
}

#[test]
fn every_requested_filter_applies_across_the_union_and_count_agrees() {
    let kernel = kernel();
    for ledger in ["alpha", "beta"] {
        file(&kernel, ledger, &format!("Needle-{ledger}"), "rust,ready");
        file(&kernel, ledger, "Needle-blocked", "rust,ready,blocked");
        file(&kernel, ledger, "Needle-other-label", "rust");
        file(&kernel, ledger, "Other-text", "rust,ready");
        let closed = file(&kernel, ledger, "Needle-closed", "rust,ready");
        sink(
            &kernel,
            &format!("urn:iki:ledger:{ledger}:close"),
            &[("item", &closed)],
        );
    }
    let base = [
        ("ledgers", "alpha,beta"),
        ("text", "nEeDlE"),
        ("labels", "rust,ready"),
        ("without", "blocked"),
    ];
    for (status, expected) in [("open", 2), ("closed", 2), ("all", 4)] {
        let mut args = base.to_vec();
        args.push(("status", status));
        let doc = document(&kernel, &args);
        assert_eq!(doc["total"], expected, "{doc}");
        assert_eq!(doc["count"], expected, "{doc}");
        let count = source(&kernel, "urn:iki:ledger:count", &args);
        assert_eq!(count, format!("{expected}\n"));
        args.push(("as", "application/json"));
        let count: Value =
            serde_json::from_str(&source(&kernel, "urn:iki:ledger:count", &args)).unwrap();
        assert_eq!(count["total"], expected);
        assert_eq!(count["ledgers"], json!(["alpha", "beta"]));
    }
    let empty = document(&kernel, &[("ledgers", "alpha,beta"), ("labels", "absent")]);
    assert_eq!(empty["count"], 0);
    assert_eq!(empty["total"], 0);
}

#[test]
fn paging_is_global_and_reaches_interleaved_ledgers_exactly_once() {
    let kernel = kernel();
    for (ledger, title) in [
        ("alpha", "A1"),
        ("beta", "B1"),
        ("alpha", "A2"),
        ("beta", "B2"),
        ("alpha", "A3"),
    ] {
        file(&kernel, ledger, title, "");
    }
    let mut seen = Vec::new();
    for (offset, expected) in [
        ("0", vec!["A3", "B2"]),
        ("2", vec!["A2", "B1"]),
        ("4", vec!["A1"]),
    ] {
        let args = [
            ("ledgers", "beta,alpha"),
            ("limit", "2"),
            ("offset", offset),
        ];
        let doc = document(&kernel, &args);
        assert_eq!(titles(&doc), expected);
        assert_eq!(doc["total"], 5);
        assert_eq!(doc["offset"], offset.parse::<usize>().unwrap());
        seen.extend(titles(&doc).into_iter().map(str::to_string));
        let plain = source(&kernel, "urn:iki:ledger:items", &args);
        assert!(plain.contains("of 5 item(s)"), "{plain}");
        if offset != "4" {
            assert!(plain.contains("for the next page"), "{plain}");
        }
    }
    assert_eq!(seen, ["A3", "B2", "A2", "B1", "A1"]);
    for offset in ["5", &usize::MAX.to_string()] {
        let doc = document(
            &kernel,
            &[
                ("ledgers", "alpha,beta"),
                ("limit", "2"),
                ("offset", offset),
            ],
        );
        assert_eq!(doc["count"], 0);
        assert_eq!(doc["total"], 5);
    }
    let whole = document(&kernel, &[("ledgers", "alpha,beta"), ("limit", "0")]);
    assert_eq!(whole["count"], 5);
}

#[test]
fn ties_use_number_then_ledger_name_independent_of_selection_order() {
    let kernel = kernel();
    for ledger in ["alpha", "beta"] {
        for title in ["One", "Two"] {
            file(&kernel, ledger, &format!("{ledger}-{title}"), "");
        }
        sink(&kernel, "urn:iki:store:graph-update", &[
            ("graph", &format!("urn:iki:ledger:graph:{ledger}")),
            ("content", &format!("DELETE {{ GRAPH <urn:iki:ledger:graph:{ledger}> {{ ?s <http://purl.org/dc/terms/modified> ?old }} }} INSERT {{ GRAPH <urn:iki:ledger:graph:{ledger}> {{ ?s <http://purl.org/dc/terms/modified> \"2030-01-01T00:00:00Z\"^^<http://www.w3.org/2001/XMLSchema#dateTime> }} }} WHERE {{ GRAPH <urn:iki:ledger:graph:{ledger}> {{ ?s <http://purl.org/dc/terms/modified> ?old }} }}"))
        ]);
    }
    for ledgers in ["alpha,beta", "beta,alpha,beta"] {
        let whole = document(&kernel, &[("ledgers", ledgers)]);
        assert_eq!(
            titles(&whole),
            ["alpha-Two", "beta-Two", "alpha-One", "beta-One"]
        );
        for (offset, expected) in [
            ("0", "alpha-Two"),
            ("1", "beta-Two"),
            ("2", "alpha-One"),
            ("3", "beta-One"),
        ] {
            let page = document(
                &kernel,
                &[("ledgers", ledgers), ("limit", "1"), ("offset", offset)],
            );
            assert_eq!(titles(&page), [expected]);
        }
    }
}

#[test]
fn json_rows_keep_their_own_graphs_comments_and_link_numbers() {
    let kernel = kernel();
    for ledger in ["alpha", "beta"] {
        file(&kernel, ledger, "Parent", "");
        let child = file(&kernel, ledger, &format!("Child-{ledger}"), "");
        sink(
            &kernel,
            &format!("urn:iki:ledger:{ledger}:comment"),
            &[("item", &child), ("content", &format!("Comment-{ledger}"))],
        );
        sink(
            &kernel,
            &format!("urn:iki:ledger:{ledger}:link"),
            &[("item", &child), ("type", "parent"), ("content", "1")],
        );
    }
    let doc = document(&kernel, &[("ledgers", "alpha,beta"), ("text", "Child")]);
    for item in doc["items"].as_array().unwrap() {
        let ledger = item["display"].as_str().unwrap().split('#').next().unwrap();
        assert_eq!(item["comments"][0]["text"], format!("Comment-{ledger}"));
        assert_eq!(item["links"][0]["target"]["number"], 1);
        assert_eq!(item["links"][0]["target"]["display"], format!("{ledger}#1"));
    }
}

#[test]
fn invalid_plural_names_and_filters_are_refused_in_words() {
    let kernel = kernel();
    for bad in ["", "alpha,", "Alpha", "alpha/beta", "items", "alpha,,beta"] {
        for resource in ["items", "count"] {
            let error = try_verb(
                &kernel,
                Verb::Source,
                &format!("urn:iki:ledger:{resource}"),
                &[("ledgers", bad)],
            )
            .unwrap_err();
            let ikigai_core::Error::InvalidArgument { name, detail } = error else {
                panic!("{error}");
            };
            assert_eq!(name, "ledgers");
            assert!(detail.contains("ledger"), "{detail}");
        }
    }
    // Validate even when the selection has no readable member.
    let cap = Capability::scoped(["urn:cap:ledger:read:alpha".to_string(), graph_read("alpha")]);
    for (arg, value) in [
        ("status", "oops"),
        ("deferred", "oops"),
        ("offset", "oops"),
        ("limit", "oops"),
    ] {
        assert!(try_as(
            &kernel,
            &cap,
            Verb::Source,
            "urn:iki:ledger:items",
            &[("ledgers", "private"), (arg, value)]
        )
        .is_err());
    }
}

#[test]
fn the_union_reports_defects_only_from_readable_selected_graphs() {
    let kernel = kernel();
    for ledger in ["alpha", "beta", "private"] {
        let item = file(&kernel, ledger, "Missing-title", "");
        sink(&kernel, "urn:iki:store:graph-update", &[
            ("graph", &format!("urn:iki:ledger:graph:{ledger}")),
            ("content", &format!("DELETE WHERE {{ GRAPH <urn:iki:ledger:graph:{ledger}> {{ <{item}> <http://purl.org/dc/terms/title> ?title }} }}"))
        ]);
        file(&kernel, ledger, &format!("Readable-{ledger}"), "");
    }
    let cap = Capability::scoped([
        "urn:cap:ledger:read:alpha".to_string(),
        graph_read("alpha"),
        "urn:cap:ledger:read:beta".to_string(),
        graph_read("beta"),
    ]);
    let text = try_as(
        &kernel,
        &cap,
        Verb::Source,
        "urn:iki:ledger:items",
        &[
            ("ledgers", "private,beta,alpha"),
            ("as", "application/json"),
        ],
    )
    .unwrap();
    let doc: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(doc["count"], 2);
    assert_eq!(doc["total"], 2);
    assert_eq!(doc["unreadable"].as_array().unwrap().len(), 2);
    assert!(!text.contains("private"), "{text}");
    for ledger in ["alpha", "beta"] {
        assert!(doc["unreadable"]
            .as_array()
            .unwrap()
            .iter()
            .any(|defect| defect["iri"]
                .as_str()
                .unwrap()
                .contains(&format!(":{ledger}:item:"))));
    }
}

#[test]
fn turtle_contains_only_the_combined_page() {
    let kernel = kernel();
    file(&kernel, "alpha", "Oldest", "");
    file(&kernel, "beta", "Middle", "");
    file(&kernel, "alpha", "Newest", "");
    let turtle = source(
        &kernel,
        "urn:iki:ledger:items",
        &[
            ("ledgers", "alpha,beta"),
            ("limit", "1"),
            ("offset", "1"),
            ("as", "text/turtle"),
        ],
    );
    assert!(turtle.contains("Middle"), "{turtle}");
    assert!(!turtle.contains("Oldest"), "{turtle}");
    assert!(!turtle.contains("Newest"), "{turtle}");
}

#[test]
fn a_duplicate_number_in_one_graph_still_has_stable_tied_pages() {
    let kernel = kernel();
    let mut iris = Vec::new();
    for title in ["First", "Second"] {
        iris.push(file(&kernel, "alpha", title, ""));
    }
    sink(&kernel, "urn:iki:store:graph-update", &[
        ("graph", "urn:iki:ledger:graph:alpha"),
        ("content", "DELETE { GRAPH <urn:iki:ledger:graph:alpha> { ?s <https://ikigai-rs.dev/ns/ledger#number> ?n ; <http://purl.org/dc/terms/modified> ?old } } INSERT { GRAPH <urn:iki:ledger:graph:alpha> { ?s <https://ikigai-rs.dev/ns/ledger#number> 1 ; <http://purl.org/dc/terms/modified> \"2030-01-01T00:00:00Z\"^^<http://www.w3.org/2001/XMLSchema#dateTime> } } WHERE { GRAPH <urn:iki:ledger:graph:alpha> { ?s <https://ikigai-rs.dev/ns/ledger#number> ?n ; <http://purl.org/dc/terms/modified> ?old } }")
    ]);
    iris.sort();
    for (offset, iri) in iris.iter().enumerate() {
        let doc = document(
            &kernel,
            &[
                ("ledgers", "alpha,beta"),
                ("limit", "1"),
                ("offset", &offset.to_string()),
            ],
        );
        assert_eq!(doc["items"][0]["iri"], *iri);
        assert_eq!(doc["total"], 2);
    }
}
