//! The store's optional word index. The RDF filters remain the definition of a match.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;

use ikigai_core::{ArgRef, Error, Invocation, Iri, Representation, Request, Result, Verb};
use serde::Deserialize;

use crate::ledger::Ledger;
use crate::model::{self, Fields, Filter};
use crate::sparql::{self, StoreClient};
use crate::vocabulary as v;

const GRAPH_TEXT: &str = "urn:iki:store:graph-text";
const ENROLLMENT: &str = "urn:iki:store:text-enrollment";
const ANALYZER: &str = "analyzer: words: split on non-alphanumerics, lowercased, no stemming";
const LIMIT: usize = 1000;

#[derive(Deserialize)]
struct Answer {
    schema: u32,
    hits: Vec<Hit>,
}

#[derive(Deserialize)]
struct Hit {
    graph: String,
    subject: String,
    predicate: String,
    score: f64,
}

fn graph_index(graph: &str) -> String {
    let thread = ikigai_store::graph_thread(graph);
    let encoded = thread
        .strip_prefix("urn:iki:store:graph-written:")
        .expect("the store's graph thread prefix");
    format!("urn:iki:store:text-index:graph:{encoded}")
}

async fn source(inv: &Invocation<'_>, target: &str) -> Result<Representation> {
    inv.source(&Iri::parse(target).expect("a store resource IRI"))
        .await
}

async fn enrollment(inv: &Invocation<'_>) -> Option<BTreeMap<String, f64>> {
    let bytes = source(inv, ENROLLMENT).await.ok()?.bytes;
    let text = std::str::from_utf8(&bytes).ok()?;
    if !text.lines().any(|line| line == ANALYZER) {
        return None;
    }
    text.lines()
        .filter_map(|line| line.strip_prefix("predicate: <"))
        .map(|line| {
            let (predicate, weight) = line.split_once("> weight ")?;
            let weight: f64 = weight.parse().ok()?;
            (weight.is_finite() && weight > 0.0).then(|| (predicate.to_string(), weight))
        })
        .collect()
}

fn predicates(fields: Fields) -> Vec<&'static str> {
    [
        (fields.title, v::ext::TITLE),
        (fields.body || fields.comments, v::BODY),
    ]
    .into_iter()
    .filter_map(|(selected, predicate)| selected.then_some(predicate))
    .collect()
}

async fn hits(
    inv: &Invocation<'_>,
    graphs: &[String],
    predicates: &[&str],
    query: (&str, &str),
) -> Option<Vec<Hit>> {
    let mut request = Request::new(Verb::Source, Iri::parse(GRAPH_TEXT).expect("constant IRI"));
    for (name, value) in [
        ("graph", graphs.join(" ")),
        ("predicate", predicates.join(" ")),
        ("limit", LIMIT.to_string()),
        ("mode", "any".to_string()),
        (query.0, query.1.to_string()),
    ] {
        request = request.with_arg(name, ArgRef::Inline(value.into_bytes()));
    }
    let response = inv.issue(request).await.ok()?;
    // A shared writer can change RDF without informing the index. The store only
    // attaches these threads when its writes cover the named graphs.
    if !graphs.iter().all(|graph| {
        response
            .threads()
            .contains(&ikigai_store::graph_thread(graph).into())
    }) {
        return None;
    }
    let answer: Answer = serde_json::from_slice(&response.bytes).ok()?;
    // There is no continuation or total. A full response can be a truncated one.
    if answer.schema != 1 || answer.hits.len() >= LIMIT {
        return None;
    }
    answer
        .hits
        .iter()
        .all(|hit| {
            graphs.contains(&hit.graph)
                && predicates.contains(&hit.predicate.as_str())
                && hit.score.is_finite()
                && hit.score >= 0.0
        })
        .then_some(answer.hits)
}

/// Short ASCII terms have the same word boundaries in the two analyzers whenever the
/// ledger's whole-word regex matches. Unicode simple folding also matches long s and the
/// Kelvin sign. The index lowercases the Kelvin sign to k, but keeps long s distinct.
fn candidate_words(term: &str) -> Option<Vec<String>> {
    if term.is_empty() || !term.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    let lower = term.to_ascii_lowercase();
    let long_s: Vec<_> = lower.match_indices('s').map(|(index, _)| index).collect();
    let kelvin = lower.matches('k').count();
    // The tokenizer drops a token of 40 bytes or more, before lowercasing. A
    // folded spelling can use two bytes for long s or three for the Kelvin sign.
    if long_s.len() > 4 || lower.len() + long_s.len() + 2 * kelvin >= 40 {
        return None;
    }
    let mut variants = vec![lower];
    for index in long_s.into_iter().rev() {
        let additional = variants
            .iter()
            .map(|word| {
                let mut variant = word.clone();
                variant.replace_range(index..index + 1, "ſ");
                variant
            })
            .collect::<Vec<_>>();
        variants.extend(additional);
    }
    Some(variants)
}

fn candidate_query(filter: &Filter) -> Option<String> {
    let words = if let Some(phrase) = &filter.search.phrase {
        // One mandatory word supplies a superset, even if the phrase includes words
        // longer than the index's tokenizer accepts.
        model::text_words(phrase).find_map(candidate_words)?
    } else if filter.search.whole_word {
        let mut words = filter.search.words.iter().map(String::as_str);
        if let Some(text) = &filter.text {
            candidate_words(text)?
        } else if filter.search.any {
            words
                .map(candidate_words)
                .collect::<Option<Vec<_>>>()?
                .into_iter()
                .flatten()
                .collect()
        } else {
            candidate_words(words.next()?)?
        }
    } else {
        return None;
    };
    (!words.is_empty()).then(|| words.join(" "))
}

/// A ready 0.6.1 index can still omit nonliterals and coalesce literals with the same
/// lexical value whose RDF datatype or language differs. A count with no text predicates
/// in its filter guards candidate pruning against either case and against incomplete
/// rebuilds. The check is independent of the term and does no regex or excerpt work.
async fn complete(
    client: &StoreClient<'_, '_>,
    graph: &str,
    weights: &BTreeMap<String, f64>,
) -> Option<()> {
    let bytes = source(client.invocation(), &graph_index(graph))
        .await
        .ok()?
        .bytes;
    let status = std::str::from_utf8(&bytes).ok()?;
    if !status.lines().any(|line| line == "state: ready")
        || !status.lines().any(|line| line == "pending: 0")
    {
        return None;
    }
    let documents: i64 = status
        .lines()
        .find_map(|line| line.strip_prefix("documents: "))?
        .parse()
        .ok()?;
    let values = weights
        .keys()
        .map(|predicate| sparql::iri(predicate, "predicate"))
        .collect::<Result<Vec<_>>>()
        .ok()?
        .join(" ");
    let query = format!(
        "SELECT (COUNT(*) AS ?count) WHERE {{ {} }}",
        client.in_graph(&format!(
            "VALUES ?predicate {{ {values} }} ?s ?predicate ?o ."
        ))
    );
    let rows = client.select(&query).await.ok()?;
    (rows.first()?.get("count")?.as_i64()? == documents).then_some(())
}

/// Recheck after the RDF read so a graph write between the index and the filter does
/// not hide newly matching subjects. An unsupported or oversized candidate query can
/// always use the original filter.
pub(crate) async fn narrowed<T, Fut>(
    client: &StoreClient<'_, '_>,
    filter: Filter,
    read: impl Fn(Filter) -> Fut,
) -> Result<T>
where
    Fut: Future<Output = Result<T>>,
{
    let Some(values) = candidates(client, &filter).await else {
        return read(filter).await;
    };
    let mut narrowed = filter.clone();
    narrowed.search.candidates = Some(values.clone());
    let result = read(narrowed).await;
    if result.is_ok() && candidates(client, &filter).await.as_deref() == Some(&values) {
        client
            .invocation()
            .trace_note("text-index", "checked candidates");
        return result;
    }
    read(filter).await
}

async fn candidates(client: &StoreClient<'_, '_>, filter: &Filter) -> Option<String> {
    let query = candidate_query(filter)?;
    let predicates = predicates(filter.search.fields);
    let weights = enrollment(client.invocation()).await?;
    if !predicates.iter().all(|p| weights.contains_key(*p)) {
        return None;
    }
    let graph = client.ledger().graph();
    let hits = hits(
        client.invocation(),
        std::slice::from_ref(&graph),
        &predicates,
        ("words", &query),
    )
    .await?;
    complete(client, &graph, &weights).await?;
    // The tokenizer uses Rust's Unicode tables, which can know about alphanumerics
    // that REGEX still treats as unassigned separators. For whole-word searches,
    // REGEX also treats some assigned numbers (such as ²) as outside of \w.
    let boundary = if filter.search.phrase.is_some() {
        r"\p{Unassigned}"
    } else {
        r"[\p{Unassigned}[\p{Number}--\w]]"
    };
    let values = predicates
        .iter()
        .map(|p| format!("<{p}>"))
        .collect::<Vec<_>>()
        .join(" ");
    let pattern = sparql::literal(boundary);
    let query = format!("ASK {{ {} }}", client.in_graph(&format!(
        "VALUES ?predicate {{ {values} }} ?subject ?predicate ?text . FILTER(REGEX(STR(?text), {pattern}))"
    )));
    if client.ask(&query).await.ok()? {
        return None;
    }
    let subjects: BTreeSet<_> = hits.iter().map(|hit| hit.subject.as_str()).collect();
    // Blank subjects and unusually large IRI lists are handled by the ordinary scan.
    let mut lists = model::values_lists(subjects).ok()?;
    match lists.len() {
        0 => Some(String::new()),
        1 => lists.pop(),
        _ => None,
    }
}

pub(crate) type Scores = BTreeMap<usize, BTreeMap<String, f64>>;

pub(crate) fn score(scores: &Scores, index: usize, item: &model::Item) -> f64 {
    scores
        .get(&index)
        .and_then(|items| items.get(&item.iri))
        .copied()
        .unwrap_or(0.0)
}

pub(crate) async fn relevance(
    inv: &Invocation<'_>,
    ledgers: &[Ledger],
    bound: &Ledger,
    filter: &Filter,
) -> Result<Scores> {
    let names = if ledgers.is_empty() {
        bound.name().to_string()
    } else {
        ledgers
            .iter()
            .map(Ledger::name)
            .collect::<Vec<_>>()
            .join(", ")
    };
    if !filter.searches() {
        return Err(Error::InvalidArgument {
            name: "sort".to_string(),
            detail: format!("Give text=, search= or phrase= to sort by relevance in {names}, where nothing was listed"),
        });
    }
    ranked_scores(inv, ledgers, filter).await.ok_or_else(|| {
        Error::Unavailable(format!(
            "Choose another sort or narrow your search because the text index cannot rank this search in {names}, where nothing was listed"
        ))
    })
}

async fn ranked_scores(
    inv: &Invocation<'_>,
    ledgers: &[Ledger],
    filter: &Filter,
) -> Option<Scores> {
    if ledgers.is_empty() {
        return Some(Scores::new());
    }
    let graphs: Vec<_> = ledgers.iter().map(Ledger::graph).collect();
    let predicates = predicates(filter.search.fields);
    let weights = enrollment(inv).await?;
    if !predicates.iter().all(|p| weights.contains_key(*p)) {
        return None;
    }
    let terms = filter.terms();
    let phrase = filter.search.phrase.as_deref();
    // Deduplicate as the analyzer lowercases, but give it the original spellings
    // so it tokenizes before lowercasing (İ becomes i plus a combining dot).
    let mut distinct = BTreeMap::new();
    for word in terms
        .iter()
        .copied()
        .chain(phrase)
        .flat_map(model::text_words)
    {
        let lower = word
            .chars()
            .flat_map(char::to_lowercase)
            .collect::<String>();
        let spelling = distinct.entry(lower).or_insert(word);
        // Its length filter also runs before lowercasing.
        if word.len() < spelling.len() {
            *spelling = word;
        }
    }
    let words = distinct.into_values().collect::<Vec<_>>().join(" ");
    let query = match phrase {
        Some(phrase) if terms.is_empty() => ("phrase", phrase),
        _ => ("words", words.as_str()),
    };
    let hits = hits(inv, &graphs, &predicates, query).await?;
    let mut scores = Scores::new();
    for (index, ledger) in ledgers.iter().enumerate() {
        let client = StoreClient::new(inv, ledger.clone());
        complete(&client, &graphs[index], &weights).await?;
        let mut docs = BTreeMap::new();
        for hit in hits.iter().filter(|hit| hit.graph == graphs[index]) {
            let weight = if hit.predicate == v::ext::TITLE {
                2.0
            } else {
                1.0
            };
            let score = hit.score / weights[&hit.predicate] * weight;
            if !score.is_finite() {
                return None;
            }
            docs.entry((&hit.subject, &hit.predicate))
                .and_modify(|best: &mut f64| *best = best.max(score))
                .or_insert(score);
        }
        let fields = filter.search.fields;
        let test = filter.hit_test("?text");
        let mut checked = BTreeMap::new();
        for values in model::values_lists(docs.keys().map(|(subject, _)| subject.as_str())).ok()? {
            let query = format!(
                "SELECT DISTINCT ?subject ?predicate ?item ?matched WHERE {{ {} }}",
                client.in_graph(&format!(
                    "VALUES ?subject {{ {values} }} VALUES ?predicate {{ {predicates} }} \
                     ?subject ?predicate ?text . BIND(({test}) AS ?matched) \
                     OPTIONAL {{ ?subject <{on_item}> ?item }}",
                    predicates = predicates
                        .iter()
                        .map(|p| format!("<{p}>"))
                        .collect::<Vec<_>>()
                        .join(" "),
                    on_item = v::ON_ITEM,
                ))
            );
            for row in client.select(&query).await.ok()? {
                let subject = &row.get("subject")?.value;
                let predicate = &row.get("predicate")?.value;
                let Some(&score) = docs.get(&(subject, predicate)) else {
                    continue;
                };
                let matched = row
                    .get("matched")
                    .is_some_and(|value| value.value == "true");
                // 0.6.1 identifies hits by subject and predicate only. If one value
                // passes and another fails, their scores cannot be attributed safely.
                let previous = checked
                    .entry((subject.clone(), predicate.clone()))
                    .or_insert(matched);
                if *previous != matched {
                    return None;
                }
                if !matched {
                    continue;
                }
                let mut record = |item: &str| {
                    scores
                        .entry(index)
                        .or_default()
                        .entry(item.to_string())
                        .and_modify(|best| *best = best.max(score))
                        .or_insert(score);
                };
                if predicate == v::ext::TITLE || fields.body {
                    record(subject);
                }
                if predicate == v::BODY && fields.comments {
                    if let Some(item) = row.get("item") {
                        record(&item.value);
                    }
                }
            }
        }
    }
    Some(scores)
}

/// A purge can reach all three graphs. Hosts can enroll the two auxiliary graphs by
/// exact name, so compact those too when present. Other failures must reach the caller
/// so it can retry the purge by its tombstone.
pub(crate) async fn compact(client: &StoreClient<'_, '_>) -> Result<()> {
    let ledger = client.ledger();
    for graph in [
        ledger.graph(),
        ledger.deleted_graph(),
        ledger.session_graph(),
    ] {
        let iri =
            Iri::parse(format!("{}:compact", graph_index(&graph))).expect("a store resource IRI");
        match client
            .invocation()
            .issue(Request::new(Verb::Sink, iri))
            .await
        {
            Ok(_) => {}
            Err(Error::Unresolved(_)) => return Ok(()),
            Err(Error::InvalidArgument { name, detail })
                if name == "graph"
                    && detail.starts_with(&format!("graph <{graph}> is not enrolled")) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}
