mod common;
use common::*;

fn numbers(kernel: &ikigai_core::Kernel, sort: &str) -> Vec<i64> {
    let doc: serde_json::Value = serde_json::from_str(&source(
        kernel,
        "urn:iki:ledger:items",
        &[("sort", sort), ("as", "application/json")],
    ))
    .unwrap();
    doc["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["number"].as_i64().unwrap())
        .collect()
}

#[test]
fn number_sort_precedes_paging() {
    let kernel = kernel();
    append(&kernel, "First", &[]);
    append(&kernel, "Second", &[]);
    assert_eq!(numbers(&kernel, "number"), [1, 2]);
}

#[test]
fn each_key_and_direction_and_comma_ties() {
    let kernel = kernel();
    append(&kernel, "Unset", &[]);
    append(&kernel, "Low", &[("priority", "4")]);
    append(&kernel, "High", &[("priority", "0")]);
    append(&kernel, "High again", &[("priority", "0")]);
    assert_eq!(numbers(&kernel, "priority"), [4, 3, 2, 1]);
    assert_eq!(numbers(&kernel, "-priority"), [2, 4, 3, 1]);
    assert_eq!(numbers(&kernel, "priority,-number"), [4, 3, 2, 1]);
    assert_eq!(numbers(&kernel, "priority,number"), [3, 4, 2, 1]);
    assert_eq!(numbers(&kernel, " priority , -created "), [4, 3, 2, 1]);
    for key in ["created", "modified", "number"] {
        assert_eq!(numbers(&kernel, key), [1, 2, 3, 4], "{key}");
        assert_eq!(numbers(&kernel, &format!("-{key}")), [4, 3, 2, 1], "{key}");
    }
    sink(&kernel, "urn:iki:ledger:item:1", &[("content", "Edited")]);
    assert_eq!(numbers(&kernel, "-modified"), [1, 4, 3, 2]);
    assert_eq!(numbers(&kernel, "created"), [1, 2, 3, 4]);
}

fn blocks(kernel: &ikigai_core::Kernel, a: &str, b: &str) {
    sink(
        kernel,
        "urn:iki:ledger:link",
        &[("item", a), ("content", b), ("type", "blocks")],
    );
}

#[test]
fn landing_is_topological_with_priority_ties_and_refuses_cycles() {
    let kernel = kernel();
    append(&kernel, "Blocked high", &[("priority", "0")]);
    append(&kernel, "Independent", &[("priority", "1")]);
    append(&kernel, "Blocker", &[("priority", "4")]);
    blocks(&kernel, "#3", "#1");
    assert_eq!(numbers(&kernel, "landing"), [2, 3, 1]);
    assert_eq!(numbers(&kernel, "-landing"), [1, 3, 2]);
    blocks(&kernel, "#1", "#3");
    let error = try_verb(
        &kernel,
        ikigai_core::Verb::Source,
        "urn:iki:ledger:items",
        &[("sort", "landing"), ("limit", "1")],
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("cycle"), "{error}");
    assert!(error.contains("sort"), "{error}");
    assert!(error.contains("#1") && error.contains("#3"), "{error}");
}

#[test]
fn bad_keys_name_the_argument_and_available_keys() {
    let kernel = kernel();
    for sort in ["", "title", "number,", "--number"] {
        let error = try_verb(
            &kernel,
            ikigai_core::Verb::Source,
            "urn:iki:ledger:items",
            &[("sort", sort)],
        )
        .unwrap_err()
        .to_string();
        for word in [
            "sort", "priority", "created", "modified", "number", "landing",
        ] {
            assert!(error.contains(word), "{sort}: {error}");
        }
    }
}

#[test]
fn sorting_happens_before_the_page_is_cut() {
    let kernel = kernel();
    for n in 1..=55 {
        append(&kernel, &format!("Item {n}"), &[]);
    }
    for sort in ["number", "created", "landing"] {
        let doc: serde_json::Value = serde_json::from_str(&source(
            &kernel,
            "urn:iki:ledger:items",
            &[
                ("sort", sort),
                ("limit", "2"),
                ("offset", "1"),
                ("as", "application/json"),
            ],
        ))
        .unwrap();
        assert_eq!(doc["items"][0]["number"], 2, "{sort}: {doc}");
        assert_eq!(doc["items"][1]["number"], 3, "{sort}: {doc}");
        assert_eq!(doc["total"], 55);
    }
}

#[test]
fn union_ties_and_cross_ledger_blocks_use_readable_graphs_only() {
    use ikigai_core::{Capability, Verb};
    let kernel = kernel();
    let mut iris = Vec::new();
    for ledger in ["alpha", "beta", "private"] {
        for title in ["One", "Two"] {
            let result = sink(
                &kernel,
                &format!("urn:iki:ledger:{ledger}:append"),
                &[("content", &format!("{ledger}-{title}")), ("priority", "1")],
            );
            iris.push(result.split_whitespace().nth(1).unwrap().to_string());
        }
    }
    // alpha#1 must land after beta#2, even though ledger-name ties favor alpha.
    sink(
        &kernel,
        "urn:iki:ledger:beta:link",
        &[
            ("item", &iris[3]),
            ("content", &iris[0]),
            ("type", "blocks"),
        ],
    );
    // A cycle in an unreadable graph must not refuse or leak into the answer.
    sink(
        &kernel,
        "urn:iki:ledger:private:link",
        &[
            ("item", &iris[4]),
            ("content", &iris[5]),
            ("type", "blocks"),
        ],
    );
    sink(
        &kernel,
        "urn:iki:ledger:private:link",
        &[
            ("item", &iris[5]),
            ("content", &iris[4]),
            ("type", "blocks"),
        ],
    );
    let cap = Capability::scoped([
        "urn:cap:ledger:read:alpha".to_string(),
        graph_read("alpha"),
        "urn:cap:ledger:read:beta".to_string(),
        graph_read("beta"),
    ]);
    for (sort, expected) in [
        ("priority", vec!["alpha#2", "beta#2", "alpha#1", "beta#1"]),
        ("landing", vec!["alpha#2", "beta#1", "beta#2", "alpha#1"]),
        ("number", vec!["alpha#1", "beta#1", "alpha#2", "beta#2"]),
    ] {
        let result = try_as(
            &kernel,
            &cap,
            Verb::Source,
            "urn:iki:ledger:items",
            &[
                ("ledgers", "private,beta,alpha,beta"),
                ("sort", sort),
                ("as", "application/json"),
            ],
        )
        .unwrap();
        assert!(!result.contains("private"), "{result}");
        let doc: serde_json::Value = serde_json::from_str(&result).unwrap();
        let displays: Vec<_> = doc["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["display"].as_str().unwrap())
            .collect();
        assert_eq!(displays, expected, "{sort}");
    }
}

#[test]
fn every_face_selects_the_same_sorted_page() {
    let kernel = kernel();
    append(&kernel, "First", &[]);
    append(&kernel, "Second", &[]);
    append(&kernel, "Third", &[]);
    for face in ["text/plain", "application/json", "text/turtle"] {
        let page = source(
            &kernel,
            "urn:iki:ledger:items",
            &[
                ("sort", "number"),
                ("limit", "1"),
                ("offset", "1"),
                ("as", face),
            ],
        );
        assert!(page.contains("Second"), "{page}");
        assert!(!page.contains("First") && !page.contains("Third"), "{page}");
        blocks(&kernel, "#1", "#3");
        blocks(&kernel, "#3", "#1");
        let error = try_verb(
            &kernel,
            ikigai_core::Verb::Source,
            "urn:iki:ledger:items",
            &[("sort", "landing"), ("as", face)],
        )
        .unwrap_err();
        assert!(error.to_string().contains("cycle"), "{error}");
        // The topology is the filtered selection, not hidden or excluded items.
        let filtered = source(
            &kernel,
            "urn:iki:ledger:items",
            &[("sort", "landing"), ("text", "Second"), ("as", face)],
        );
        assert!(filtered.contains("Second"), "{filtered}");
    }
}

#[test]
fn the_unknown_sort_refusal_is_a_sentence_with_the_remedy() {
    let kernel = kernel();
    let error = try_verb(
        &kernel,
        ikigai_core::Verb::Source,
        "urn:iki:ledger:items",
        &[("sort", "title")],
    )
    .unwrap_err();
    let ikigai_core::Error::InvalidArgument { name, detail } = error else {
        panic!("{error}");
    };
    assert_eq!(name, "sort");
    assert_eq!(detail, "`title` is not one of priority, created, modified, number, landing; use a comma-separated list with - before a key for descending order");
}

#[test]
fn timestamp_ties_and_the_default_keep_the_existing_number_order() {
    let kernel = kernel();
    append(&kernel, "First", &[]);
    append(&kernel, "Second", &[]);
    sink(&kernel, "urn:iki:store:graph-update", &[
        ("graph", "urn:iki:ledger:graph:default"),
        ("content", "DELETE { GRAPH <urn:iki:ledger:graph:default> { ?s ?p ?old } } INSERT { GRAPH <urn:iki:ledger:graph:default> { ?s ?p \"2030-01-01T00:00:00Z\"^^<http://www.w3.org/2001/XMLSchema#dateTime> } } WHERE { GRAPH <urn:iki:ledger:graph:default> { ?s ?p ?old . FILTER(?p IN (<http://purl.org/dc/terms/created>, <http://purl.org/dc/terms/modified>)) } }"),
    ]);
    for key in ["created", "-created", "modified", "-modified"] {
        assert_eq!(numbers(&kernel, key), [2, 1], "{key}");
        assert_eq!(
            numbers(&kernel, &format!("{key},number")),
            [1, 2],
            "{key},number"
        );
    }
    let default = source(&kernel, "urn:iki:ledger:items", &[]);
    assert_eq!(
        default,
        source(&kernel, "urn:iki:ledger:items", &[("sort", "-modified")])
    );
    assert_eq!(numbers(&kernel, "landing"), [1, 2]);
}
