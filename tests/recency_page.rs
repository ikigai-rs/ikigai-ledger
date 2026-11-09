//! Recency pages keep the grouped query's item boundaries when hand edits multiply rows.
mod common;

use common::*;
use ikigai_ledger::vocabulary as v;
use serde_json::Value;

const GRAPH: &str = "urn:iki:ledger:graph:default";
const XSD: &str = "http://www.w3.org/2001/XMLSchema#dateTime";

fn iris(json: &Value, key: &str) -> Vec<String> {
    json[key]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["iri"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn row_prefixes_preserve_grouped_pages_with_duplicates_and_ties() {
    // One row takes the fast path, three require expansion, and eighty force the
    // grouped fallback on a small page. None may spend item slots on duplicate rows.
    for copies in [1, 3, 80] {
        let kernel = kernel();
        let mut triples = String::new();
        for n in 0..8 {
            let item = format!("urn:test:page:{n}");
            let number = n / 2; // Duplicate numbers also exercise the IRI tie-break.
            triples.push_str(&format!(
                "<{item}> <{}> <{}> ; <{}> {number} ; <{}> \"Item{n}\" ; \
                 <{}> <{}> ; <{}> \"2026-01-01T00:00:00Z\"^^<{XSD}> ; \
                 <{}> \"2026-02-01T00:00:00Z\"^^<{XSD}> .\n",
                v::ext::TYPE,
                v::ITEM_CLASS,
                v::NUMBER,
                v::ext::TITLE,
                v::STATUS,
                v::OPEN,
                v::ext::CREATED,
                v::ext::MODIFIED,
            ));
            if n == 0 && copies > 1 {
                // Both recency and number maxima must be taken independently.
                triples.push_str(&format!(
                    "<{item}> <{}> 99 ; <{}> \"2030-01-01T00:00:00Z\"^^<{XSD}> .\n",
                    v::NUMBER,
                    v::ext::MODIFIED,
                ));
                for c in 1..copies {
                    triples.push_str(&format!(
                        "<{item}> <{}> \"Other title {c}\" .\n",
                        v::ext::TITLE,
                    ));
                }
            }
        }
        sink(
            &kernel,
            "urn:iki:store:graph-update",
            &[
                ("graph", GRAPH),
                (
                    "content",
                    &format!("INSERT DATA {{ GRAPH <{GRAPH}> {{ {triples} }} }}"),
                ),
            ],
        );
        let reference_query = format!(
            "SELECT ?item (MAX(COALESCE(<{XSD}>(?modified), <{XSD}>(?created))) AS ?recency) \
             (MAX(?number) AS ?n) WHERE {{ GRAPH <{GRAPH}> {{ \
             ?item <{}> <{}> ; <{}> ?number ; <{}> ?title ; <{}> ?status . \
             OPTIONAL {{ ?item <{}> ?created }} OPTIONAL {{ ?item <{}> ?modified }} \
             ?item <{}> <{}> . }} }} GROUP BY ?item \
             ORDER BY DESC(?recency) DESC(?n) ?item",
            v::ext::TYPE,
            v::ITEM_CLASS,
            v::NUMBER,
            v::ext::TITLE,
            v::STATUS,
            v::ext::CREATED,
            v::ext::MODIFIED,
            v::STATUS,
            v::OPEN,
        );
        let reference: Value = serde_json::from_str(&select(&kernel, &reference_query)).unwrap();
        let reference: Vec<String> = reference["results"]["bindings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["item"]["value"].as_str().unwrap().to_owned())
            .collect();
        for limit in [1, 3, 50] {
            for offset in [0, 1, 4, 7, 10] {
                let page: Value = serde_json::from_str(&source(
                    &kernel,
                    "urn:iki:ledger:items",
                    &[
                        ("as", "application/json"),
                        ("limit", &limit.to_string()),
                        ("offset", &offset.to_string()),
                    ],
                ))
                .unwrap();
                let expected: Vec<_> = reference.iter().skip(offset).take(limit).cloned().collect();
                assert_eq!(
                    iris(&page, "items"),
                    expected,
                    "copies={copies}, limit={limit}, offset={offset}"
                );
                assert_eq!(page["total"], 8);
            }
        }
    }
}
