mod common;
use common::*;
use ikigai_core::{Error, Verb};
use serde_json::Value;

fn check(kernel: &ikigai_core::Kernel, args: &[(&str, &str)], expected: usize) {
    let mut json_args = args.to_vec();
    json_args.push(("as", "application/json"));
    let doc: Value =
        serde_json::from_str(&source(kernel, "urn:iki:ledger:items", &json_args)).unwrap();
    assert_eq!(doc["total"], expected, "{args:?}: {doc}");
    assert_eq!(doc["items"].as_array().unwrap().len(), expected);
    assert_eq!(
        source(kernel, "urn:iki:ledger:count", args),
        format!("{expected}\n")
    );
}

#[test]
fn priorities_select_values_and_absence() {
    let kernel = kernel();
    for priority in ["0", "1", "2", "3", "4"] {
        append(
            &kernel,
            &format!("Priority {priority}"),
            &[("priority", priority)],
        );
    }
    append(&kernel, "No priority", &[]);
    for priority in ["0", "1", "2", "3", "4", "none"] {
        check(&kernel, &[("priority", priority)], 1);
        let doc: Value = serde_json::from_str(&source(
            &kernel,
            "urn:iki:ledger:items",
            &[("priority", priority), ("as", "application/json")],
        ))
        .unwrap();
        let expected = priority
            .parse::<u8>()
            .ok()
            .map(Value::from)
            .unwrap_or(Value::Null);
        assert_eq!(doc["items"][0]["priority"], expected);
        check(&kernel, &[("without_priority", priority)], 5);
    }
    check(&kernel, &[("priority", " 0,2,none,2 ")], 3);
    check(&kernel, &[("without_priority", "0,2,none")], 3);
    check(
        &kernel,
        &[("priority", "1,none"), ("without_priority", "1")],
        1,
    );
    check(&kernel, &[("priority", "1"), ("without_priority", "1")], 0);
}

#[test]
fn invalid_priority_filters_name_the_accepted_values() {
    let kernel = kernel();
    for name in ["priority", "without_priority"] {
        for bad in ["", "5", "-1", "p1", "None", "1,", "1,,2", "1;2", "01"] {
            for resource in ["items", "count"] {
                let error = try_verb(
                    &kernel,
                    Verb::Source,
                    &format!("urn:iki:ledger:{resource}"),
                    &[(name, bad)],
                )
                .unwrap_err();
                let Error::InvalidArgument {
                    name: actual,
                    detail,
                } = error
                else {
                    panic!("{error}");
                };
                assert_eq!(actual, name);
                let invalid = bad
                    .split(',')
                    .find(|v| !["0", "1", "2", "3", "4", "none"].contains(v))
                    .unwrap();
                assert_eq!(
                    detail,
                    format!("`{invalid}` is not one of 0, 1, 2, 3, 4, none (no priority set)")
                );
            }
        }
    }
}

#[test]
fn priority_composes_with_other_filters_in_single_and_union_listings() {
    let kernel = kernel();
    for ledger in ["default", "alpha", "beta"] {
        for (title, labels, priority, closed) in [
            ("Needle open", "rust,ready", "1", false),
            ("Needle none", "rust,ready", "", false),
            ("Needle closed", "rust,ready", "1", true),
            ("Needle blocked", "rust,ready,blocked", "1", false),
            ("Needle label", "rust", "1", false),
            ("Other text", "rust,ready", "1", false),
            ("Needle priority", "rust,ready", "2", false),
        ] {
            let mut args = vec![("content", title), ("labels", labels)];
            if !priority.is_empty() {
                args.push(("priority", priority));
            }
            let item = sink(&kernel, &format!("urn:iki:ledger:{ledger}:append"), &args);
            if closed {
                let iri = item.split_whitespace().nth(1).unwrap();
                sink(
                    &kernel,
                    &format!("urn:iki:ledger:{ledger}:close"),
                    &[("item", iri)],
                );
            }
        }
    }
    for (selection, multiplier) in [(None, 1), (Some("alpha,beta"), 2)] {
        for (status, expected) in [("open", 2), ("closed", 1), ("all", 3)] {
            let mut args = vec![
                ("priority", "1,none"),
                ("labels", "rust,ready"),
                ("without", "blocked"),
                ("text", "nEeDlE"),
                ("status", status),
            ];
            if let Some(ledgers) = selection {
                args.push(("ledgers", ledgers));
            }
            check(&kernel, &args, expected * multiplier);
            args.push(("without_priority", "none"));
            check(
                &kernel,
                &args,
                (expected - usize::from(status != "closed")) * multiplier,
            );
            args.push(("limit", "1"));
            args.push(("as", "application/json"));
            let doc: Value =
                serde_json::from_str(&source(&kernel, "urn:iki:ledger:items", &args)).unwrap();
            assert_eq!(doc["items"].as_array().unwrap().len(), 1);
            assert_eq!(
                doc["total"],
                (expected - usize::from(status != "closed")) * multiplier
            );
        }
    }
}

#[test]
fn priority_filters_are_validated_even_when_no_selected_ledger_is_readable() {
    let kernel = kernel();
    let cap = ikigai_core::Capability::scoped([
        "urn:cap:ledger:read:alpha".to_string(),
        graph_read("alpha"),
    ]);
    for resource in ["items", "count"] {
        for name in ["priority", "without_priority"] {
            let error = try_as(
                &kernel,
                &cap,
                Verb::Source,
                &format!("urn:iki:ledger:{resource}"),
                &[("ledgers", "private"), (name, "p1")],
            )
            .unwrap_err();
            assert!(matches!(error, Error::InvalidArgument { name: actual, .. } if actual == name));
        }
    }
}

#[test]
fn both_resources_declare_optional_priority_filters() {
    use ikigai_core::{Iri, Request, Resolution, Scope, Space};
    let space = ikigai_ledger::space();
    for resource in ["items", "count"] {
        let request = Request::new(
            Verb::Meta,
            Iri::parse(format!("urn:iki:ledger:{resource}")).unwrap(),
        );
        let Resolution::Hit(resolved) = space.resolve(&request, &Scope::empty()) else {
            panic!("resource resolves");
        };
        let description = resolved.endpoint.describe();
        for name in ["priority", "without_priority"] {
            let input = description
                .inputs
                .iter()
                .find(|input| input.name == name)
                .unwrap();
            assert!(!input.required);
            let summary = &input.summary;
            assert!(summary.contains("none (no priority set)"));
            assert!(summary.contains("ANY"));
        }
    }
}
