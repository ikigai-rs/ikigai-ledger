//! An item's agent sessions (ledger #3022): `urn:iki:ledger:{ledger}:item:{id}:sessions`.
//!
//! A SESSION SUMMARY is the content-free record a worker attaches to its hand-back: the
//! harness's session id, the item, the machine, the harness, the model, the exit status, the
//! wall-clock time, turns and tool calls, and tokens by category. Never transcript text,
//! prompts, tool results or file contents, and no cost.
//!
//! - **Sink** records one, under `urn:cap:ledger:session:{ledger}` (in the published worker
//!   grant) or `write:{ledger}`, on the item's newest hand-back comment. It is a request of its
//!   own, after the hand-back, so a summary that is refused leaves the hand-back standing.
//! - **Source** reads one item's summaries, newest first, in text, Turtle and JSON. Nothing
//!   here totals across items.
//!
//! And the ledger's whole set (ledger #3647): `urn:iki:ledger:{ledger}:sessions`, every summary
//! in the ledger, newest first, paged, raw. Each names its item by id alone, never by title, and
//! nothing totals or prices. It reads under `urn:cap:ledger:summaries:{ledger}`, which admits
//! this set and no item text, or under `read:{ledger}`.
//!
//! ★ **The shape decides, and it is closed.** The summary arrives as one JSON object; each key
//! becomes the `ledger:` property of the same name, and the node is checked against
//! `ledger:SessionSummaryShape` in `vocabulary.ttl` before anything is written. A key the shape
//! does not name (`transcript`, `cost`, `prompt`) is a property the closed shape refuses, so the
//! refusal is the shape's, not a list kept in code beside it.
//!
//! The session is named `urn:agent:session:id:{id}`, singular and type first. The older
//! `urn:agents:session:{id}` that `about` edges carry is linked with `owl:sameAs`, and no
//! existing `about` value is rewritten.

use std::collections::BTreeMap;

use oxrdf::{Literal, NamedNode, NamedOrBlankNode, Term, Triple};
use serde_json::Value;

use super::*;

use crate::sparql::iso8601;

const SESSION_FACES: [&str; 3] = [PLAIN, TURTLE, JSON];

/// What a session IRI starts with.
pub(crate) const SESSION_PREFIX: &str = "urn:agent:session:id:";
/// The older convention an `about` edge may carry, aliased and never rewritten.
pub(crate) const LEGACY_SESSION_PREFIX: &str = "urn:agents:session:";

/// The session graph's store grants this caller lacks, of its read grant and, when `write`, its
/// write grant, refused by name before anything is read or written (ledger #3661).
///
/// ★ **This is how a worker grant from before ledger #3661 is answered.** Its lines name the
/// ledger's own graph and not the summaries graph, so recording a summary under it is `Denied`
/// here, with the two lines to add, rather than written where the summaries grant cannot read
/// it or refused halfway by the store.
pub(crate) fn require_session_graph(inv: &Invocation<'_>, write: bool, doing: &str) -> Result<()> {
    let ledger = Ledger::from_bindings(inv.bindings)?;
    let graph = ledger.session_graph();
    let mut scopes = vec![ikigai_store::cap_read_graph(&graph)];
    if write {
        scopes.push(ikigai_store::cap_write_graph(&graph));
    }
    let missing: Vec<String> = scopes
        .into_iter()
        .filter(|scope| !inv.capability.allows(scope))
        .map(|scope| format!("`{scope}`"))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    Err(Error::Denied(format!(
        "this capability does not hold {}. Since ledger #3661 a ledger's session summaries are in \
         a graph of their own, `{graph}`, so {doing} needs that graph's store {} beside the \
         ledger grant. A grant written before then, a worker's among them, lacks these lines: add them",
        missing.join(" or "),
        if write {
            "read and write grants"
        } else {
            "read grant"
        },
    )))
}

const SH: &str = "http://www.w3.org/ns/shacl#";
const RDF_FIRST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
const RDF_REST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
const RDF_NIL: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";
const XSD_DECIMAL: &str = "http://www.w3.org/2001/XMLSchema#decimal";

/// The longest session id accepted: a harness id is a UUID or close to it.
const MAX_ID: usize = 128;

/// One property constraint of the shape.
#[derive(Debug, Default, Clone)]
pub(crate) struct Property {
    pub(crate) path: String,
    datatype: Option<String>,
    iri: bool,
    min_count: usize,
    max_count: Option<usize>,
    min_inclusive: Option<i64>,
    one_of: Vec<Term>,
    prefix: Option<String>,
}

/// `ledger:SessionSummaryShape`, read from the vocabulary.
#[derive(Debug)]
pub(crate) struct Shape {
    pub(crate) closed: bool,
    pub(crate) properties: Vec<Property>,
}

fn named(iri: &str) -> NamedNode {
    NamedNode::new_unchecked(iri)
}

fn sh(local: &str) -> NamedNode {
    named(&format!("{SH}{local}"))
}

fn as_subject(term: &Term) -> Option<NamedOrBlankNode> {
    match term {
        Term::NamedNode(n) => Some(n.clone().into()),
        Term::BlankNode(b) => Some(b.clone().into()),
        _ => None,
    }
}

fn int_of(term: &Term) -> Option<i64> {
    match term {
        Term::Literal(l) => l.value().parse().ok(),
        _ => None,
    }
}

/// Parse `ledger:SessionSummaryShape` out of the vocabulary document. It reads `sh:path`,
/// `sh:datatype`, `sh:nodeKind`, `sh:minCount`, `sh:maxCount`, `sh:minInclusive` and `sh:in`; a
/// shape using any other constraint is an error, never a constraint silently skipped.
///
/// Why not ikigai-shacl, the ecosystem's SHACL validator: it links the rudof stack, which this
/// crate's wasm build and its two-crate dependency set (core and store) cannot carry, for one
/// closed shape of seven constraint kinds. This checker reads only those and refuses the rest,
/// so a shape that grows past it fails loudly; move to ikigai-shacl if the ledger ever validates
/// more than this one shape.
pub(crate) fn shape() -> Result<Shape> {
    let broken = |why: String| Error::Endpoint(format!("ledger:SessionSummaryShape: {why}"));
    let mut graph = Graph::new();
    for quad in oxrdfio::RdfParser::from_format(oxrdfio::RdfFormat::Turtle)
        .for_slice(crate::vocabulary::VOCABULARY_TTL.as_bytes())
    {
        let quad = quad.map_err(|e| broken(e.to_string()))?;
        graph.insert(&Triple::new(quad.subject, quad.predicate, quad.object));
    }
    let node = named(v::SESSION_SUMMARY_SHAPE);
    let closed = matches!(
        graph.object_for_subject_predicate(&node, &sh("closed")),
        Some(oxrdf::TermRef::Literal(l)) if l.value() == "true"
    );
    let mut properties = Vec::new();
    for spec in graph.objects_for_subject_predicate(&node, &sh("property")) {
        let spec = as_subject(&spec.into_owned()).ok_or_else(|| broken("a property".into()))?;
        let mut property = Property::default();
        for triple in graph.triples_for_subject(&spec) {
            let local = triple
                .predicate
                .as_str()
                .strip_prefix(SH)
                .unwrap_or(triple.predicate.as_str());
            let object = triple.object.into_owned();
            match local {
                "path" => match &object {
                    Term::NamedNode(n) => property.path = n.as_str().to_string(),
                    _ => return Err(broken("a path that is not an IRI".into())),
                },
                "datatype" => match &object {
                    Term::NamedNode(n) => property.datatype = Some(n.as_str().to_string()),
                    _ => return Err(broken("a datatype that is not an IRI".into())),
                },
                "nodeKind" => property.iri = object == Term::from(sh("IRI")),
                "minCount" => property.min_count = int_of(&object).unwrap_or(0) as usize,
                "maxCount" => property.max_count = int_of(&object).map(|n| n as usize),
                "minInclusive" => property.min_inclusive = int_of(&object),
                "name" => {}
                "in" => {
                    let mut at = object;
                    while at != Term::from(named(RDF_NIL)) {
                        let cell = as_subject(&at).ok_or_else(|| broken("an sh:in list".into()))?;
                        if let Some(first) =
                            graph.object_for_subject_predicate(&cell, &named(RDF_FIRST))
                        {
                            property.one_of.push(first.into_owned());
                        }
                        at = graph
                            .object_for_subject_predicate(&cell, &named(RDF_REST))
                            .ok_or_else(|| broken("an unterminated sh:in list".into()))?
                            .into_owned();
                    }
                }
                other => {
                    return Err(broken(format!(
                        "uses sh:{other}, which this checker does not read"
                    )))
                }
            }
        }
        if property.path.is_empty() {
            return Err(broken("a property with no path".into()));
        }
        if property.path == "http://www.w3.org/2002/07/owl#sameAs" {
            property.prefix = Some(LEGACY_SESSION_PREFIX.to_string());
        }
        properties.push(property);
    }
    Ok(Shape { closed, properties })
}

/// A short name for a predicate in a refusal: `ledger:model`, or the IRI.
fn said(predicate: &str) -> String {
    match predicate.strip_prefix(v::LEDGER_NS) {
        Some(local) => format!("ledger:{local}"),
        None => format!("<{predicate}>"),
    }
}

/// Check one summary node against the shape: every refusal is `InvalidArgument` on `content`,
/// naming the property and saying what is accepted.
pub(crate) fn check(shape: &Shape, triples: &[(String, Term)]) -> Result<()> {
    let refuse = |detail: String| Error::InvalidArgument {
        name: "content".to_string(),
        detail,
    };
    if shape.closed {
        for (predicate, _) in triples {
            if !shape.properties.iter().any(|p| &p.path == predicate) {
                let allowed: Vec<String> = shape
                    .properties
                    .iter()
                    .filter_map(|p| p.path.strip_prefix(v::LEDGER_NS).map(str::to_string))
                    .filter(|local| !WRITTEN_HERE.contains(&local.as_str()))
                    .collect();
                return Err(refuse(format!(
                    "{} is not part of a session summary, whose shape is closed: a summary holds \
                     counts and names only, never transcript text, prompts, tool results, file \
                     contents or a cost. The accepted keys are {}",
                    said(predicate),
                    allowed.join(", ")
                )));
            }
        }
    }
    for property in &shape.properties {
        let values: Vec<&Term> = triples
            .iter()
            .filter(|(p, _)| p == &property.path)
            .map(|(_, o)| o)
            .collect();
        let name = said(&property.path);
        if values.len() < property.min_count {
            return Err(refuse(format!(
                "a session summary needs {name}, and this one has none"
            )));
        }
        if let Some(max) = property.max_count {
            if values.len() > max {
                return Err(refuse(format!("{name} may appear at most {max} time(s)")));
            }
        }
        for value in values {
            if property.iri {
                let Term::NamedNode(n) = value else {
                    return Err(refuse(format!("{name} must be an IRI")));
                };
                if let Some(prefix) = &property.prefix {
                    if !n.as_str().starts_with(prefix.as_str()) {
                        return Err(refuse(format!("{name} must start with `{prefix}`")));
                    }
                }
            }
            if let Some(datatype) = &property.datatype {
                let ok = matches!(value, Term::Literal(l) if l.datatype().as_str() == datatype);
                if !ok {
                    let kind = match datatype.as_str() {
                        d if d == v::ext::XSD_INTEGER => "a whole number",
                        d if d == v::ext::XSD_STRING => "a string",
                        d if d == v::ext::XSD_DATETIME => "a date and time",
                        _ => datatype.as_str(),
                    };
                    return Err(refuse(format!(
                        "{name} must be {kind}, and `{value}` is not"
                    )));
                }
            }
            if let Some(floor) = property.min_inclusive {
                if int_of(value).is_some_and(|n| n < floor) {
                    return Err(refuse(format!(
                        "{name} must be {floor} or more, and `{value}` is not"
                    )));
                }
            }
            if !property.one_of.is_empty() && !property.one_of.contains(value) {
                let options: Vec<String> = property
                    .one_of
                    .iter()
                    .map(|t| match t {
                        Term::Literal(l) => l.value().to_string(),
                        other => other.to_string(),
                    })
                    .collect();
                return Err(refuse(format!(
                    "{name} must be one of {}, and `{value}` is not",
                    options.join(", ")
                )));
            }
        }
    }
    Ok(())
}

/// The shape's properties the ledger writes itself; a caller's key naming one is refused.
const WRITTEN_HERE: [&str; 4] = [
    "sessionItem",
    "sessionItemNumber",
    "handBackEvent",
    "sessionId",
];

/// A session id: letters, digits, `-`, `_` and `.`, at most [`MAX_ID`] long. Kept to what is
/// safe in an IRI's last segment without escaping, which is all a harness's ids use.
fn session_id(value: &Value) -> Result<String> {
    let refuse = |detail: String| Error::InvalidArgument {
        name: "content".to_string(),
        detail,
    };
    let Some(id) = value.as_str() else {
        return Err(refuse(
            "`session` must be the harness's session id, as a string".into(),
        ));
    };
    if id.is_empty() || id.len() > MAX_ID {
        return Err(refuse(format!(
            "`session` must be 1 to {MAX_ID} characters, and this one is {}",
            id.len()
        )));
    }
    if let Some(c) = id
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
    {
        return Err(refuse(format!(
            "`session` may hold only A-Z a-z 0-9 - _ and ., and `{id}` contains `{c}`"
        )));
    }
    Ok(id.to_string())
}

/// The summary a caller sent, as `(predicate, value)` pairs, plus its session id. Each JSON key
/// is the `ledger:` property of that name; the shape decides which are allowed.
fn parse(content: &str) -> Result<(String, Vec<(String, Term)>)> {
    let refuse = |detail: String| Error::InvalidArgument {
        name: "content".to_string(),
        detail,
    };
    let value: Value = serde_json::from_str(content).map_err(|e| {
        refuse(format!(
            "a session summary is one JSON object, and this is not JSON: {e}"
        ))
    })?;
    let Value::Object(fields) = value else {
        return Err(refuse("a session summary is one JSON object".into()));
    };
    let id = session_id(fields.get("session").unwrap_or(&Value::Null))?;
    let mut triples = Vec::new();
    for (key, value) in &fields {
        if key == "session" {
            continue;
        }
        let local_ok = key.chars().next().is_some_and(|c| c.is_ascii_lowercase())
            && key.chars().all(|c| c.is_ascii_alphanumeric());
        if !local_ok || WRITTEN_HERE.contains(&key.as_str()) {
            return Err(refuse(format!(
                "`{key}` is not a key of a session summary; see the accepted keys in this \
                 resource's description"
            )));
        }
        let object: Term = match value {
            Value::String(s) => Literal::new_typed_literal(s, named(v::ext::XSD_STRING)).into(),
            Value::Number(n) if n.is_i64() => {
                Literal::new_typed_literal(n.to_string(), named(v::ext::XSD_INTEGER)).into()
            }
            Value::Number(n) => {
                Literal::new_typed_literal(n.to_string(), named(XSD_DECIMAL)).into()
            }
            _ => {
                return Err(refuse(format!(
                    "`{key}` must be a string or a number; a summary holds no nested values"
                )))
            }
        };
        triples.push((format!("{}{key}", v::LEDGER_NS), object));
    }
    Ok((id, triples))
}

/// The item's newest hand-back comment, or a `Conflict` saying there is none.
async fn newest_handback(client: &StoreClient<'_, '_>, item: &Item) -> Result<String> {
    let rows = client
        .select(&format!(
            "SELECT ?comment WHERE {{ {} }} ORDER BY DESC(?at) DESC(STR(?comment)) LIMIT 1",
            client.in_graph(&format!(
                "?comment <{on}> <{iri}> ; <{created}> ?at . \
                 {{ ?comment <{mark}> true }} UNION {{ ?comment <{by}> ?holder }}",
                on = v::ON_ITEM,
                iri = item.iri,
                created = v::ext::CREATED,
                mark = v::HAND_BACK,
                by = v::HANDED_BACK_BY,
            ))
        ))
        .await?;
    rows.first()
        .and_then(|row| row.get("comment"))
        .map(|b| b.value.clone())
        .ok_or_else(|| {
            Error::Conflict(format!(
                "{} has no hand-back to record a session on. Hand it back first with \
                 `comment handback=true`, then send the summary",
                item.short()
            ))
        })
}

/// One recorded summary, as read back.
struct Summary {
    iri: String,
    values: BTreeMap<String, Vec<Term>>,
}

impl Summary {
    fn one(&self, local: &str) -> Option<String> {
        self.values
            .get(&format!("{}{local}", v::LEDGER_NS))
            .or_else(|| self.values.get(local))
            .and_then(|all| all.first())
            .map(|t| match t {
                Term::Literal(l) => l.value().to_string(),
                Term::NamedNode(n) => n.as_str().to_string(),
                other => other.to_string(),
            })
    }

    fn created(&self) -> String {
        self.one(v::ext::CREATED).unwrap_or_default()
    }
}

fn term_of(binding: &sparql::Binding, arg: &str) -> Result<Term> {
    let term = binding.term(arg)?;
    Ok(match term {
        ikigai_store::sparql::Term::NamedNode(n) => named(n.as_str()).into(),
        ikigai_store::sparql::Term::Literal(l) => match l.language() {
            Some(lang) => Literal::new_language_tagged_literal_unchecked(l.value(), lang).into(),
            None => Literal::new_typed_literal(l.value(), named(l.datatype().as_str())).into(),
        },
        other => Literal::new_simple_literal(other.to_string()).into(),
    })
}

/// Every summary on `item`, newest first; one millisecond's in session order. Read from the
/// session graph alone.
async fn read(client: &StoreClient<'_, '_>, item: &str) -> Result<Vec<Summary>> {
    let rows = client
        .select_session(&format!(
            "SELECT ?s ?p ?o WHERE {{ {} }}",
            client.in_session_graph(&format!(
                "?s <{type_}> <{class}> ; <{on}> <{item}> ; ?p ?o .",
                type_ = v::ext::TYPE,
                class = v::SESSION_SUMMARY_CLASS,
                on = v::SESSION_ITEM,
            ))
        ))
        .await?;
    let mut by: BTreeMap<String, Summary> = BTreeMap::new();
    for row in &rows {
        let (Some(s), Some(p), Some(o)) = (row.get("s"), row.get("p"), row.get("o")) else {
            continue;
        };
        by.entry(s.value.clone())
            .or_insert_with(|| Summary {
                iri: s.value.clone(),
                values: BTreeMap::new(),
            })
            .values
            .entry(p.value.clone())
            .or_default()
            .push(term_of(o, "o")?);
    }
    let mut all: Vec<Summary> = by.into_values().collect();
    for summary in &mut all {
        for values in summary.values.values_mut() {
            values.sort_by_key(|t| t.to_string());
        }
    }
    all.sort_by(|a, b| {
        b.created()
            .cmp(&a.created())
            .then_with(|| a.iri.cmp(&b.iri))
    });
    Ok(all)
}

/// One page of the ledger's summaries, newest first, each with its item's number when the summary
/// carries one, and whether a later page has more. One query over the session graph alone, so the
/// page reads nothing of any item but the IRI and number each summary names (ledger #3661); one
/// more row than the page is asked for, to answer "more" without a total.
async fn read_page(
    client: &StoreClient<'_, '_>,
    limit: usize,
    offset: usize,
) -> Result<(Vec<(Summary, Option<i64>)>, bool)> {
    let rows = client
        .select_session(&format!(
            "SELECT ?s ?p ?o ?n WHERE {{ {} }}",
            client.in_session_graph(&format!(
                "{{ SELECT ?s (MAX(?at) AS ?latest) WHERE {{ ?s <{type_}> <{class}> ; \
                 <{created}> ?at }} GROUP BY ?s ORDER BY DESC(?latest) ASC(STR(?s)) \
                 LIMIT {fetch} OFFSET {offset} }} \
                 ?s ?p ?o . \
                 OPTIONAL {{ ?s <{number}> ?n }}",
                type_ = v::ext::TYPE,
                class = v::SESSION_SUMMARY_CLASS,
                created = v::ext::CREATED,
                number = v::SESSION_ITEM_NUMBER,
                fetch = limit.saturating_add(1),
            ))
        ))
        .await?;
    let mut by: BTreeMap<String, (Summary, Option<i64>)> = BTreeMap::new();
    for row in &rows {
        let (Some(s), Some(p), Some(o)) = (row.get("s"), row.get("p"), row.get("o")) else {
            continue;
        };
        let entry = by.entry(s.value.clone()).or_insert_with(|| {
            (
                Summary {
                    iri: s.value.clone(),
                    values: BTreeMap::new(),
                },
                None,
            )
        });
        if let Some(n) = row.get("n").and_then(|n| n.value.parse().ok()) {
            entry.1 = Some(n);
        }
        entry
            .0
            .values
            .entry(p.value.clone())
            .or_default()
            .push(term_of(o, "o")?);
    }
    let mut all: Vec<(Summary, Option<i64>)> = by.into_values().collect();
    for (summary, _) in &mut all {
        for values in summary.values.values_mut() {
            values.sort_by_key(|t| t.to_string());
            values.dedup();
        }
    }
    all.sort_by(|(a, _), (b, _)| {
        b.created()
            .cmp(&a.created())
            .then_with(|| a.iri.cmp(&b.iri))
    });
    let more = all.len() > limit;
    all.truncate(limit);
    Ok((all, more))
}

/// The text block for one summary.
fn block(summary: &Summary) -> String {
    let or_none = |local: &str| summary.one(local).unwrap_or_else(|| "not reported".into());
    let mut text = format!(
        "session {} recorded {}\n",
        or_none("sessionId"),
        summary.created()
    );
    text.push_str(&format!(
        "  harness: {}, model: {}, machine: {}\n",
        or_none("harness"),
        or_none("model"),
        or_none("machineName")
    ));
    text.push_str(&format!(
        "  exit status: {}, wall clock: {} ms\n",
        or_none("exitStatus"),
        or_none("wallClockMs")
    ));
    text.push_str(&format!(
        "  turns: {}, tool calls: {}\n",
        or_none("turns"),
        or_none("toolCalls")
    ));
    text.push_str(&format!(
        "  tokens: input {}, cached input {}, cache creation {}, output {}\n",
        or_none("inputTokens"),
        or_none("cachedInputTokens"),
        or_none("cacheCreationTokens"),
        or_none("outputTokens")
    ));
    text.push_str(&format!("  hand-back: {}\n", or_none("handBackEvent")));
    text.push_str(&format!("  iri: {}\n", summary.iri));
    text
}

/// One summary as a JSON object: each property by its local name, numbers as numbers.
fn json_of(summary: &Summary) -> Value {
    let mut object = serde_json::Map::new();
    object.insert("iri".into(), Value::String(summary.iri.clone()));
    for (predicate, values) in &summary.values {
        if predicate == v::ext::TYPE || predicate == v::SESSION_ITEM_NUMBER {
            continue;
        }
        let key = predicate
            .strip_prefix(v::LEDGER_NS)
            .map(str::to_string)
            .unwrap_or_else(|| match predicate.as_str() {
                p if p == v::ext::CREATED => "recorded".to_string(),
                "http://www.w3.org/2002/07/owl#sameAs" => "sameAs".to_string(),
                other => other.to_string(),
            });
        let Some(value) = values.first() else {
            continue;
        };
        let value = match value {
            Term::Literal(l) if l.datatype().as_str() == v::ext::XSD_INTEGER => l
                .value()
                .parse::<i64>()
                .map(Value::from)
                .unwrap_or_else(|_| Value::String(l.value().to_string())),
            Term::Literal(l) => Value::String(l.value().to_string()),
            Term::NamedNode(n) => Value::String(n.as_str().to_string()),
            other => Value::String(other.to_string()),
        };
        let key = if key == "sessionId" {
            "session".to_string()
        } else {
            key
        };
        object.insert(key, value);
    }
    Value::Object(object)
}

/// `urn:iki:ledger:{ledger}:item:{id}:sessions`.
#[derive(Clone)]
pub(super) struct SessionsEndpoint;

#[async_trait]
impl Endpoint for SessionsEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        match inv.request.verb {
            Verb::Source => self.source(inv).await,
            Verb::Sink => self.sink(inv).await,
            other => Err(unsupported("ledger-item-sessions", other)),
        }
    }

    fn name(&self) -> &str {
        "ledger-item-sessions"
    }

    fn describe(&self) -> Description {
        Description::new("ledger-item-sessions")
            .title("An item's agent sessions")
            .summary(
                "The content-free summaries of the agent sessions that worked on one item, each \
                 recorded on that session's hand-back: session id, machine, harness, model, exit \
                 status, wall-clock time, turns, tool calls and tokens by category. Never \
                 transcript text, prompts, tool results, file contents or a cost, and nothing \
                 that totals across items.",
            )
            .verb(Verb::Meta)
            .action(read_action(
                ikigai_core::ActionSpec::new(Verb::Source)
                    .input(ledger_arg("item:{id}:sessions"))
                    .summary(
                        "One item's session summaries, newest first: a block of labeled lines \
                         each in text; the summary nodes in Turtle; an array of objects in JSON.",
                    )
                    .input(flight::item_id_input())
                    .input(as_arg(&SESSION_FACES))
                    .output(PLAIN)
                    .output(TURTLE)
                    .output(JSON),
            ))
            .action(write_scopes(
                ikigai_core::ActionSpec::new(Verb::Sink)
                    .input(ledger_arg("item:{id}:sessions"))
                    .summary(
                        "Record a session summary on the item's newest hand-back. Takes \
                         `urn:cap:ledger:session:{ledger}` (in the worker grant) or \
                         `urn:cap:ledger:write:{ledger}`. Sent after the hand-back, so a refused \
                         summary leaves the hand-back standing. The same session sent again on \
                         the same hand-back writes nothing.",
                    )
                    .input(flight::item_id_input())
                    .input(
                        ArgSpec::new("content")
                            .summary(
                                "One JSON object, checked against the closed \
                                 `ledger:SessionSummaryShape`. Required: `session` (the \
                                 harness's own session id), `machineName`, `harness` (`claude` or \
                                 `codex`), `model`, `exitStatus`, `wallClockMs`. Optional whole \
                                 numbers: `turns`, `toolCalls`, `inputTokens`, \
                                 `cachedInputTokens`, `cacheCreationTokens`, `outputTokens`. Any \
                                 other key is refused.",
                            )
                            .class(v::ext::XSD_STRING),
                    )
                    .output(PLAIN),
                CAP_ANY,
            ))
    }
}

impl SessionsEndpoint {
    async fn source(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let client = StoreClient::new(inv, ledger_for(inv, Need::Read)?);
        require_session_graph(inv, false, "reading an item's sessions")?;
        let want = wanted_face(inv, &SESSION_FACES)?;
        let item = require_item(&client, flight::part_id(inv)?).await?;
        let summaries = read(&client, &item.iri).await?;
        match want {
            JSON => {
                let ledger = client.ledger();
                let document = serde_json::json!({
                    "schema": json::SCHEMA,
                    "ledger": ledger.name(),
                    "item": json::item_ref(ledger, item.number, &item.iri),
                    "sessions": summaries.iter().map(json_of).collect::<Vec<_>>(),
                });
                Ok(json_face(serde_json::to_vec(&document).map_err(|e| {
                    Error::Endpoint(format!("the sessions document did not serialize: {e}"))
                })?))
            }
            TURTLE => {
                let mut graph = Graph::new();
                for summary in &summaries {
                    let subject = named(&summary.iri);
                    for (predicate, values) in &summary.values {
                        for value in values {
                            graph.insert(&Triple::new(
                                subject.clone(),
                                named(predicate),
                                value.clone(),
                            ));
                        }
                    }
                }
                face(String::new(), Some(graph), TURTLE)
            }
            _ => {
                let mut text = if summaries.is_empty() {
                    format!("{} has no recorded sessions\n", item.short())
                } else {
                    format!(
                        "{}: {} session(s), newest first\n",
                        item.short(),
                        summaries.len()
                    )
                };
                for summary in &summaries {
                    text.push_str(&block(summary));
                }
                face(text, None, PLAIN)
            }
        }
    }

    /// ★ **Two graphs, so two steps** (ledger #3661). The summary goes into the session graph,
    /// and the item and its hand-back are in the ledger's own graph, which no scoped update can
    /// read from inside the session graph's scope. So the summary is written guarded only by
    /// "this session is not recorded yet", and the item is checked again AFTER it lands: a
    /// delete, or a hand-back removed, in between makes this request take its own summary back
    /// and answer `Conflict`. A delete that commits after that check moves the summary to the
    /// graveyard itself (see `endpoints::move_summaries`), so a summary is never left in the
    /// session graph for an item that is gone.
    async fn sink(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let client = StoreClient::new(inv, ledger_for(inv, Need::Session)?);
        require_session_graph(inv, true, "recording a session summary")?;
        let now = now_ms(inv)?;
        let item = require_item(&client, flight::part_id(inv)?).await?;
        let (id, mut triples) = parse(inv.inline_str("content")?)?;
        let handback = newest_handback(&client, &item).await?;
        let session = format!("{SESSION_PREFIX}{id}");
        triples.push((
            v::ext::TYPE.to_string(),
            named(v::SESSION_SUMMARY_CLASS).into(),
        ));
        triples.push((
            v::SESSION_ID.to_string(),
            Literal::new_typed_literal(&id, named(v::ext::XSD_STRING)).into(),
        ));
        triples.push((v::SESSION_ITEM.to_string(), named(&item.iri).into()));
        triples.push((
            v::SESSION_ITEM_NUMBER.to_string(),
            Literal::new_typed_literal(item.number.to_string(), named(v::ext::XSD_INTEGER)).into(),
        ));
        triples.push((v::HAND_BACK_EVENT.to_string(), named(&handback).into()));
        triples.push((
            v::ext::CREATED.to_string(),
            Literal::new_typed_literal(iso8601(now), named(v::ext::XSD_DATETIME)).into(),
        ));
        triples.push((
            "http://www.w3.org/2002/07/owl#sameAs".to_string(),
            named(&format!("{LEGACY_SESSION_PREFIX}{id}")).into(),
        ));
        check(&shape()?, &triples)?;

        // A summary already recorded for this session: the same hand-back is a resend, and
        // writes nothing; another is a conflict, because one session hands back once.
        let recorded = |rows: &[sparql::Row]| {
            rows.first()
                .and_then(|row| row.get("on"))
                .map(|b| b.value.clone())
        };
        let lookup = format!(
            "SELECT ?on WHERE {{ {} }} LIMIT 1",
            client.in_session_graph(&format!("<{session}> <{}> ?on", v::HAND_BACK_EVENT))
        );
        let answer = |on: String| -> Result<Representation> {
            if on == handback {
                Ok(plain(format!(
                    "{} session {id} already recorded on its hand-back; nothing written\n",
                    item.short()
                )))
            } else {
                Err(Error::Conflict(format!(
                    "session {id} is already recorded on another hand-back ({on}); one session \
                     hands back once, so this summary was not written"
                )))
            }
        };
        if let Some(on) = recorded(&client.select_session(&lookup).await?) {
            return answer(on);
        }
        let body: String = triples
            .iter()
            .map(|(p, o)| format!("<{session}> <{p}> {o} .\n"))
            .collect();
        client
            .update_session(&format!(
                "INSERT {{ {} }} WHERE {{ FILTER NOT EXISTS {{ {} }} }}",
                client.in_session_graph(&body),
                client.in_session_graph(&format!("<{session}> <{}> ?any", v::HAND_BACK_EVENT)),
            ))
            .await?;
        match recorded(&client.select_session(&lookup).await?) {
            Some(on) if on == handback => {}
            Some(on) => return answer(on),
            None => {
                return Err(Error::Conflict(format!(
                    "{} changed while the summary was written; nothing was written",
                    item.short()
                )))
            }
        }
        // The item and its hand-back, read again now that the summary is in.
        let standing = client
            .ask(&format!(
                "ASK {{ {} }}",
                client.in_graph(&format!(
                    "{} <{handback}> <{}> <{}>",
                    live(&item.iri),
                    v::ON_ITEM,
                    item.iri
                ))
            ))
            .await?;
        if !standing {
            client
                .update_session(&format!(
                    "DELETE DATA {{ {} }}",
                    client.in_session_graph(&body)
                ))
                .await?;
            return Err(Error::Conflict(format!(
                "{} changed while the summary was written (deleted, or its hand-back removed); \
                 the summary was taken back",
                item.short()
            )));
        }
        Ok(plain(format!(
            "recorded session {id} on {}'s hand-back\n",
            item.short()
        )))
    }
}

/// `urn:iki:ledger:{ledger}:sessions`: every summary in the ledger (ledger #3647).
#[derive(Clone)]
pub(super) struct SessionSetEndpoint;

#[async_trait]
impl Endpoint for SessionSetEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        match inv.request.verb {
            Verb::Source => self.source(inv).await,
            Verb::Sink => self.migrate(inv).await,
            other => Err(unsupported("ledger-sessions", other)),
        }
    }

    fn name(&self) -> &str {
        "ledger-sessions"
    }

    fn describe(&self) -> Description {
        Description::new("ledger-sessions")
            .title("A ledger's agent sessions")
            .summary(
                "Every session summary recorded in one ledger, newest first, paged and raw: the \
                 same content-free fields as an item's sessions, each naming its item by id \
                 only. No item title or text, no totals and no cost. Readable under \
                 `urn:cap:ledger:summaries:{ledger}`, which reads nothing else, or \
                 `urn:cap:ledger:read:{ledger}`.",
            )
            .verb(Verb::Meta)
            // Declares the family both `summaries:{ledger}` and `read:{ledger}` satisfy, and the
            // store read every ledger read needs; the exact either-or is in `ledger_for`.
            .action(
                ikigai_core::ActionSpec::new(Verb::Source)
                    .requires(CAP_ANY)
                    .requires(ikigai_store::CAP_READ_GRAPH)
                    .input(ledger_arg("sessions"))
                    .summary(
                        "One page of the ledger's session summaries, newest first: a block of \
                         labeled lines each in text, with a closing line when more follow; the \
                         summary nodes in Turtle; an array of objects and `next_offset` in JSON.",
                    )
                    .input(
                        ArgSpec::new("limit")
                            .summary(
                                "At most this many summaries on the page (default 50; `0` \
                                 means 500). No total is given: the text face's last line and \
                                 the JSON face's `next_offset` say where the next page starts.",
                            )
                            .class(v::ext::XSD_INTEGER)
                            .default_value("50")
                            .optional(),
                    )
                    .input(
                        ArgSpec::new("offset")
                            .summary(
                                "How many summaries to skip, newest first (default 0). A \
                                 summary recorded between page reads shifts the pages by one.",
                            )
                            .class(v::ext::XSD_INTEGER)
                            .default_value("0")
                            .optional(),
                    )
                    .input(as_arg(&SESSION_FACES))
                    .output(PLAIN)
                    .output(TURTLE)
                    .output(JSON),
            )
            .action(write_scopes(
                ikigai_core::ActionSpec::new(Verb::Sink)
                    .input(ledger_arg("sessions"))
                    .summary(
                        "Move the session summaries an earlier version recorded in the \
                             ledger's own graph into its session graph (ledger #3661). `plan` \
                             writes nothing and lists what `move` would move; `move` moves them \
                             and says how many. Takes `urn:cap:ledger:write:{ledger}` and the \
                             store read and write grants of both graphs. A second `move` moves \
                             nothing.",
                    )
                    .input(
                        ArgSpec::new("content")
                            .summary(
                                "`plan` (a dry run: nothing is written) or `move` (the \
                                     migration). Read the plan first.",
                            )
                            .class(v::ext::XSD_STRING),
                    )
                    .output(PLAIN),
                CAP_ANY,
            ))
    }
}

/// How many summaries one `move` batch writes: bounded, so no update nears the store's SPARQL
/// size limit, as `backfill-closed-at` does.
const MOVE_BATCH: usize = 100;

/// `1 session summary`, `2 session summaries`: a count in words, never `summary(ies)`.
fn summaries_counted(n: usize) -> String {
    if n == 1 {
        "1 session summary".to_string()
    } else {
        format!("{n} session summaries")
    }
}

/// `1 triple`, `11 triples`.
fn triples_counted(n: usize) -> String {
    if n == 1 {
        "1 triple".to_string()
    } else {
        format!("{n} triples")
    }
}

impl SessionSetEndpoint {
    /// ★ **The migration** (ledger #3661): summaries recorded before their own graph existed are
    /// in the ledger's graph, where the summaries grant's store line cannot read them and the
    /// ledger graph's store line reads everything else too. Nothing moves until an operator sends
    /// `move`; `plan` lists each summary and writes nothing. Each batch is written to the
    /// session graph first and removed from the ledger graph second, as exactly the triples read,
    /// so an interrupted `move` leaves a summary in both graphs (readers see the session graph's
    /// copy) and running it again finishes. The item number each summary needs is added from the
    /// item, when the item has one.
    async fn migrate(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let client = StoreClient::new(inv, ledger_for(inv, Need::Write)?);
        let apply = match inv.inline_str("content")?.trim() {
            "plan" => false,
            "move" => true,
            other => {
                return Err(Error::InvalidArgument {
                    name: "content".to_string(),
                    detail: format!(
                        "`{other}` is neither `plan` (a dry run that writes nothing) nor `move`"
                    ),
                })
            }
        };
        require_session_graph(inv, true, "moving the ledger's summaries")?;
        let ledger = client.ledger().clone();
        let rows = client
            .select(&format!(
                "SELECT ?s ?p ?o ?n WHERE {{ {} }} ORDER BY ?s ?p ?o",
                client.in_graph(&format!(
                    "?s <{type_}> <{class}> ; ?p ?o . \
                     OPTIONAL {{ ?s <{on}> ?item . ?item <{number}> ?n }}",
                    type_ = v::ext::TYPE,
                    class = v::SESSION_SUMMARY_CLASS,
                    on = v::SESSION_ITEM,
                    number = v::NUMBER,
                ))
            ))
            .await?;
        // Each summary: its own triples as read, the triple to add for its number, and its
        // session id and number for the plan.
        struct Found {
            triples: Vec<String>,
            add: Option<String>,
            id: String,
            number: Option<i64>,
        }
        let mut found: BTreeMap<String, Found> = BTreeMap::new();
        for row in &rows {
            let (Some(s), Some(p), Some(o)) = (row.get("s"), row.get("p"), row.get("o")) else {
                continue;
            };
            let entry = found.entry(s.value.clone()).or_insert_with(|| Found {
                triples: Vec::new(),
                add: None,
                id: s
                    .value
                    .strip_prefix(SESSION_PREFIX)
                    .unwrap_or(&s.value)
                    .to_string(),
                number: None,
            });
            let triple = format!(
                "{} {} {} .",
                sparql::term(&s.term("s")?),
                sparql::term(&p.term("p")?),
                sparql::term(&o.term("o")?)
            );
            if !entry.triples.contains(&triple) {
                entry.triples.push(triple);
            }
            if p.value == v::SESSION_ITEM_NUMBER {
                entry.number = o.value.parse().ok();
                entry.add = None;
            } else if entry.number.is_none() {
                if let Some(n) = row.get("n").and_then(|n| n.value.parse::<i64>().ok()) {
                    entry.number = Some(n);
                    entry.add = Some(format!(
                        "{} <{}> \"{n}\"^^<{}> .",
                        sparql::term(&s.term("s")?),
                        v::SESSION_ITEM_NUMBER,
                        v::ext::XSD_INTEGER
                    ));
                }
            }
        }
        let from = ledger.graph();
        let to = ledger.session_graph();
        if found.is_empty() {
            return Ok(plain(format!(
                "{}: no session summaries in {from} to move; nothing written\n",
                ledger.name()
            )));
        }
        let quads: usize = found.values().map(|f| f.triples.len()).sum();
        if !apply {
            let mut text = format!(
                "{}: {} ({}) would move from {from} to {to}. This is a plan: nothing was \
                 written\n",
                ledger.name(),
                summaries_counted(found.len()),
                triples_counted(quads)
            );
            for summary in found.values() {
                let item = summary
                    .number
                    .map(|n| ledger.number(n))
                    .unwrap_or_else(|| "an item with no number".to_string());
                text.push_str(&format!(
                    "  session {} on {item}, {}\n",
                    summary.id,
                    triples_counted(summary.triples.len())
                ));
            }
            text.push_str("To move them, send `move` to this resource\n");
            return Ok(plain(text));
        }
        let all: Vec<&Found> = found.values().collect();
        for chunk in all.chunks(MOVE_BATCH) {
            let read: String = chunk
                .iter()
                .flat_map(|f| f.triples.iter())
                .map(|t| format!("{t}\n"))
                .collect();
            let added: String = chunk
                .iter()
                .filter_map(|f| f.add.as_ref())
                .map(|t| format!("{t}\n"))
                .collect();
            client
                .update_session(&format!(
                    "INSERT DATA {{ {} }}",
                    client.in_session_graph(&format!("{read}{added}"))
                ))
                .await?;
            client
                .update(&format!("DELETE DATA {{ {} }}", client.in_graph(&read)))
                .await?;
        }
        Ok(plain(format!(
            "{}: moved {} ({}) from {from} to {to}\n",
            ledger.name(),
            summaries_counted(found.len()),
            triples_counted(quads)
        )))
    }

    async fn source(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let client = StoreClient::new(inv, ledger_for(inv, Need::Summaries)?);
        require_session_graph(inv, false, "reading the ledger's sessions")?;
        let want = wanted_face(inv, &SESSION_FACES)?;
        let limit = match parse_limit(inv, 50)? {
            0 => 500,
            n => n,
        };
        let offset = parse_count(inv, "offset", 0)?;
        let (page, more) = read_page(&client, limit, offset).await?;
        let ledger = client.ledger();
        let next_offset = more.then(|| offset.saturating_add(page.len()));
        let item_of = |summary: &Summary| summary.one("sessionItem").unwrap_or_default();
        match want {
            JSON => {
                let sessions: Vec<Value> = page
                    .iter()
                    .map(|(summary, number)| {
                        let mut object = json_of(summary);
                        if let Value::Object(fields) = &mut object {
                            let iri = item_of(summary);
                            let item = match number {
                                Some(n) => serde_json::to_value(json::item_ref(ledger, *n, &iri)),
                                None => Ok(serde_json::json!({ "iri": iri })),
                            };
                            if let Ok(item) = item {
                                fields.insert("item".into(), item);
                            }
                        }
                        object
                    })
                    .collect();
                let document = serde_json::json!({
                    "schema": json::SCHEMA,
                    "ledger": ledger.name(),
                    "offset": offset,
                    "sessions": sessions,
                    "next_offset": next_offset,
                });
                Ok(json_face(serde_json::to_vec(&document).map_err(|e| {
                    Error::Endpoint(format!("the sessions document did not serialize: {e}"))
                })?))
            }
            TURTLE => {
                let mut graph = Graph::new();
                for (summary, _) in &page {
                    let subject = named(&summary.iri);
                    for (predicate, values) in &summary.values {
                        for value in values {
                            graph.insert(&Triple::new(
                                subject.clone(),
                                named(predicate),
                                value.clone(),
                            ));
                        }
                    }
                }
                face(String::new(), Some(graph), TURTLE)
            }
            _ => {
                let mut text = if page.is_empty() && offset == 0 {
                    format!("{} has no recorded sessions\n", ledger.name())
                } else if page.is_empty() {
                    format!(
                        "{} has no recorded sessions at offset {offset}\n",
                        ledger.name()
                    )
                } else {
                    format!(
                        "{}: sessions, newest first, from offset {offset}\n",
                        ledger.name()
                    )
                };
                for (summary, number) in &page {
                    let item = match number {
                        Some(n) => format!("{} {}", ledger.number(*n), item_of(summary)),
                        None => item_of(summary),
                    };
                    let block = block(summary);
                    let (first, rest) = block.split_once('\n').unwrap_or((&block, ""));
                    text.push_str(&format!("{first}\n  item: {item}\n{rest}"));
                }
                if let Some(next) = next_offset {
                    text.push_str(&format!(
                        "more sessions follow: read again with offset={next}\n"
                    ));
                }
                face(text, None, PLAIN)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shape_uses_only_what_the_checker_reads_and_is_closed() {
        let shape = shape().expect("the shape parses");
        assert!(shape.closed, "the summary shape is closed");
        assert!(shape.properties.len() >= 15, "{shape:?}");
    }

    #[test]
    fn a_cost_or_a_transcript_is_not_a_property_of_the_shape() {
        let shape = shape().unwrap();
        for forbidden in ["cost", "costUsd", "transcript", "prompt", "toolResult"] {
            let path = format!("{}{forbidden}", v::LEDGER_NS);
            assert!(
                !shape.properties.iter().any(|p| p.path == path),
                "{forbidden}"
            );
        }
    }
}
