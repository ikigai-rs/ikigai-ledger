//! The shared review levels and the existing item plumbing they use.
mod common;

use common::*;
use ikigai_ledger::{LEDGER_NS, VOCABULARY_TTL};

const REVIEW_JOB: &str = "https://ikigai-rs.dev/ns/ledger#ReviewJob";
const FINDING: &str = "https://ikigai-rs.dev/ns/ledger#Finding";

#[test]
fn review_levels_are_documented_item_classes() {
    let quads: Vec<_> = oxrdfio::RdfParser::from_format(oxrdfio::RdfFormat::Turtle)
        .for_reader(VOCABULARY_TTL.as_bytes())
        .map(|quad| quad.expect("the vocabulary parses"))
        .collect();
    for (class, label) in [(REVIEW_JOB, "Review job"), (FINDING, "Finding")] {
        let objects = |predicate: &str| -> Vec<String> {
            quads
                .iter()
                .filter(|q| {
                    q.subject.to_string() == format!("<{class}>")
                        && q.predicate.as_str() == predicate
                })
                .map(|q| q.object.to_string())
                .collect()
        };
        assert_eq!(
            objects("http://www.w3.org/1999/02/22-rdf-syntax-ns#type"),
            ["<http://www.w3.org/2000/01/rdf-schema#Class>"],
            "{class} must be a defined class"
        );
        assert_eq!(
            objects("http://www.w3.org/2000/01/rdf-schema#subClassOf"),
            [format!("<{LEDGER_NS}Item>")]
        );
        assert_eq!(
            objects("http://www.w3.org/2000/01/rdf-schema#label"),
            [format!("\"{label}\"")]
        );
        let comments = objects("http://www.w3.org/2000/01/rdf-schema#comment");
        assert_eq!(comments.len(), 1);
        assert!(!comments[0].trim_matches('"').is_empty());
    }
}

#[test]
fn review_jobs_and_findings_round_trip_by_kind_and_replays_do_not_duplicate_findings() {
    let kernel = kernel();
    append(
        &kernel,
        "Review base..head\nRepository: acme/project; range: base..head",
        &[("kind", REVIEW_JOB), ("revision", "head")],
    );
    let finding_args = [
        (
            "content",
            "Unchecked input\nValidate the input in src/lib.rs:12.",
        ),
        ("kind", FINDING),
        ("revision", "head"),
        ("key", "review-head-unchecked-input"),
        ("labels", "severity:major"),
        ("as", "application/json"),
    ];
    let first: serde_json::Value =
        serde_json::from_str(&sink(&kernel, "urn:iki:ledger:append", &finding_args)).unwrap();
    let replay: serde_json::Value =
        serde_json::from_str(&sink(&kernel, "urn:iki:ledger:append", &finding_args)).unwrap();
    assert_eq!(first["outcome"], "filed");
    assert_eq!(replay["outcome"], "existing");
    assert_eq!(first["item"], replay["item"]);
    for (kind, title, other_title) in [
        (REVIEW_JOB, "Review base..head", "Unchecked input"),
        (FINDING, "Unchecked input", "Review base..head"),
    ] {
        let listing = source(&kernel, "urn:iki:ledger:items", &[("kind", kind)]);
        assert!(listing.contains(title), "{listing}");
        assert!(!listing.contains(other_title), "{listing}");
        assert!(listing.contains("1 item(s)"), "{listing}");
    }
    assert_eq!(source(&kernel, "urn:iki:ledger:count", &[]), "2\n");
}
