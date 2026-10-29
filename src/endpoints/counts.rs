//! Daily UTC counts and the explicitly invoked legacy close-time migration.
use super::*;
use oxrdf::{NamedNode, Triple};
use std::collections::{BTreeMap, BTreeSet};

const DAY: u64 = 86_400_000;
const CSV: &str = "text/csv";
const FACES: [&str; 2] = [CSV, TURTLE];

/// A status event belongs to the item via onItem, so delete/archive/purge owns it.
/// A graph-local sequence orders transitions even when the kernel clock is frozen.
pub(super) fn status_event(
    client: &StoreClient<'_, '_>,
    item: &str,
    status: &str,
    now: u64,
) -> Result<String> {
    let event = format!("{item}:status:{}", mint_id(now)?);
    let triples = client.in_graph(&format!(
        "<{event}> <{}> <{}> ; <{}> <{item}> ; <{}> <{status}> ; <{}> {} ; <{}> ?sequence .",
        v::ext::TYPE,
        v::STATUS_EVENT_CLASS,
        v::ON_ITEM,
        v::STATUS,
        v::ext::CREATED,
        datetime(now),
        v::EVENT_SEQUENCE
    ));
    let guard = client.in_graph(&live(item));
    let previous = client.in_graph(&format!(
        "?past <{}> <{}> ; <{}> <{item}> .",
        v::ext::TYPE,
        v::STATUS_EVENT_CLASS,
        v::ON_ITEM
    ));
    Ok(format!("INSERT {{ {triples} }} WHERE {{ {guard} {{ SELECT (COUNT(?past) + 1 AS ?sequence) WHERE {{ {previous} }} }} }}"))
}

/// Preserve a close stamp imported directly through the store before reopen removes it.
/// Normal endpoint closes already have their event, so this is then a no-op.
pub(super) fn preserve_close(client: &StoreClient<'_, '_>, item: &str, now: u64) -> Result<String> {
    let event = format!("{item}:status:{}", mint_id(now)?);
    let triples = client.in_graph(&format!(
        "<{event}> <{}> <{}> ; <{}> <{item}> ; <{}> <{}> ; <{}> ?at ; <{}> 0 ; <{}> ?source .",
        v::ext::TYPE,
        v::STATUS_EVENT_CLASS,
        v::ON_ITEM,
        v::STATUS,
        v::CLOSED,
        v::ext::CREATED,
        v::EVENT_SEQUENCE,
        v::CLOSED_AT_SOURCE
    ));
    let guard = client.in_graph(&format!(
        "{} <{item}> <{}> <{}> ; <{}> ?at . OPTIONAL {{ <{item}> <{}> ?source }} \
         FILTER NOT EXISTS {{ ?past <{}> <{}> ; <{}> <{item}> ; <{}> <{}> ; <{}> ?at }}",
        live(item),
        v::STATUS,
        v::CLOSED,
        v::CLOSED_AT,
        v::CLOSED_AT_SOURCE,
        v::ext::TYPE,
        v::STATUS_EVENT_CLASS,
        v::ON_ITEM,
        v::STATUS,
        v::CLOSED,
        v::ext::CREATED
    ));
    Ok(format!("INSERT {{ {triples} }} WHERE {{ {guard} }}"))
}

#[derive(Default)]
struct History {
    filed: u64,
    events: BTreeSet<(u64, u64, bool)>,
}

pub(super) struct CountsEndpoint;

#[async_trait]
impl Endpoint for CountsEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        if inv.request.verb != Verb::Source {
            return Err(unsupported("ledger-counts", inv.request.verb));
        }
        let client = StoreClient::new(inv, ledger_for(inv, Need::Read)?);
        let want = if inv.inline_str("as").is_ok() {
            wanted_face(inv, &FACES)?
        } else {
            CSV
        };
        let days = inv
            .inline_str("days")
            .ok()
            .map(|s| {
                s.parse::<u64>()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| Error::InvalidArgument {
                        name: "days".into(),
                        detail: "expected a positive number of UTC calendar days".into(),
                    })
            })
            .transpose()?;
        let now = now_ms(inv)?;
        let today = now / DAY;
        let query = format!("SELECT ?item ?filed ?status ?closed ?eventTime ?eventStatus ?eventOrder ?eventSource ?closeSource WHERE {{ {} }}",
            client.in_graph(&format!(
                "?item <{}> <{}> . OPTIONAL {{ ?item <{}> ?filed }} OPTIONAL {{ ?item <{}> ?status }} \
                 OPTIONAL {{ ?item <{}> ?closed }} OPTIONAL {{ ?item <{}> ?closeSource }} \
                 OPTIONAL {{ ?event <{}> <{}> ; <{}> ?item ; <{}> ?eventTime ; <{}> ?eventStatus ; <{}> ?eventOrder . OPTIONAL {{ ?event <{}> ?eventSource }} }}",
                v::ext::TYPE, v::ITEM_CLASS, v::ext::CREATED, v::STATUS, v::CLOSED_AT, v::CLOSED_AT_SOURCE,
                v::ext::TYPE, v::STATUS_EVENT_CLASS, v::ON_ITEM, v::ext::CREATED, v::STATUS, v::EVENT_SEQUENCE, v::CLOSED_AT_SOURCE
            )));
        let mut items: BTreeMap<String, History> = BTreeMap::new();
        let mut approximate_through: Option<u64> = None;
        for row in client.select(&query).await? {
            let stamp = |key: &str| -> Result<u64> {
                row.get(key)
                    .and_then(|b| model::millis(&b.value))
                    .ok_or_else(|| {
                        Error::Endpoint(format!(
                    "counts cannot date `{key}` for {}; repair the timestamp before charting",
                    row.get("item").map(|b| b.value.as_str()).unwrap_or("unknown item")
                ))
                    })
            };
            let status = row.get("status").map(|b| b.value.as_str());
            if !matches!(status, Some(v::OPEN) | Some(v::CLOSED)) {
                return Err(Error::Endpoint(
                    "counts requires one known item status".into(),
                ));
            }
            for (source, time) in [("eventSource", "eventTime"), ("closeSource", "closed")] {
                if row.get(source).is_some_and(|b| b.value == "backfill") {
                    let day = stamp(time)? / DAY;
                    approximate_through =
                        Some(approximate_through.map_or(day, |prior| prior.max(day)));
                }
            }
            let filed = stamp("filed")?;
            let item = items
                .entry(row["item"].value.clone())
                .or_insert_with(|| History {
                    filed,
                    ..History::default()
                });
            if item.filed != filed {
                return Err(Error::Endpoint(
                    "counts refuses multiple filed times for one item".into(),
                ));
            }
            if row["status"].value == v::CLOSED {
                // Never quietly count an undated legacy close as open forever.
                let closed = stamp("closed").map_err(|_| Error::Endpoint(
                    "a closed item has no readable closedAt; run backfill-closed-at explicitly before charting".into()
                ))?;
                item.events.insert((closed, 0, false));
            }
            if row.contains_key("eventTime") {
                let status = &row["eventStatus"].value;
                if status != v::OPEN && status != v::CLOSED {
                    return Err(Error::Endpoint(
                        "counts found an unknown status event".into(),
                    ));
                }
                item.events.insert((
                    stamp("eventTime")?,
                    row["eventOrder"]
                        .value
                        .parse::<u64>()
                        .map_err(|_| Error::Endpoint("invalid status event sequence".into()))?,
                    status == v::OPEN,
                ));
            }
        }
        // Sweep changes once: O(items + events + days), not items times days.
        let mut changes: BTreeMap<u64, (i64, i64)> = BTreeMap::new();
        for item in items.values() {
            changes.entry(item.filed / DAY).or_default().0 += 1;
            let mut was_open = true;
            for &(at, _, open) in &item.events {
                if at < item.filed {
                    return Err(Error::Endpoint(
                        "counts found a status event before the item was filed".into(),
                    ));
                }
                if was_open != open {
                    let delta = if open { 1 } else { -1 };
                    let change = changes.entry(at / DAY).or_default();
                    change.0 += delta;
                    change.1 -= delta;
                    was_open = open;
                }
            }
        }
        let first = items
            .values()
            .map(|i| i.filed / DAY)
            .min()
            .unwrap_or(today + 1);
        let start = days
            .map(|n| today.saturating_sub(n - 1))
            .unwrap_or(first)
            .max(first);
        let mut open = 0i64;
        let mut closed = 0i64;
        let mut changes = changes.into_iter().peekable();
        let mut csv = String::from("date,open,closed\n");
        let mut graph = Graph::new();
        for date in start..=today {
            while let Some(&(at, (o, c))) = changes.peek() {
                if at > date {
                    break;
                }
                open += o;
                closed += c;
                changes.next();
            }
            let approximate = approximate_through.is_some_and(|last| date <= last);
            let date = crate::sparql::iso8601(date * DAY)[..10].to_string();
            csv.push_str(&format!("{date},{open},{closed}\n"));
            if want == TURTLE {
                let subject = NamedNode::new(format!("{}counts:{date}", client.ledger().prefix()))
                    .expect("validated ledger and date");
                if approximate {
                    graph.insert(&Triple::new(
                        subject.clone(),
                        NamedNode::new(v::CLOSED_AT_SOURCE).expect("constant predicate"),
                        model::plain("backfill"),
                    ));
                }
                for (p, o) in [
                    (v::ext::TYPE, model::named(v::DAILY_COUNTS_CLASS)),
                    (v::DATE, model::typed(&date, v::ext::XSD_DATE)),
                    (
                        v::OPEN_COUNT,
                        model::typed(&open.to_string(), v::ext::XSD_INTEGER),
                    ),
                    (
                        v::CLOSED_COUNT,
                        model::typed(&closed.to_string(), v::ext::XSD_INTEGER),
                    ),
                ] {
                    graph.insert(&Triple::new(
                        subject.clone(),
                        NamedNode::new(p).expect("constant predicate"),
                        o,
                    ));
                }
            }
        }
        let repr = if want == TURTLE {
            Representation::new(ReprType::new(TURTLE), model::turtle(&graph)?)
        } else {
            Representation::new(ReprType::new(CSV), csv.into_bytes())
        };
        // A graph write invalidates it; midnight also extends the requested date domain.
        Ok(cached(repr).cacheable_until(ikigai_core::Time::from_millis((today + 1) * DAY)))
    }
    fn name(&self) -> &str {
        "ledger-counts"
    }
    fn describe(&self) -> Description {
        read_scopes(Description::new("ledger-counts")
            .title("Daily open and closed item counts")
            .summary("UTC end-of-day counts from the first filed item through today (today is provisional). Closed items without closedAt require the explicit backfill command. Turtle observations through the latest backfilled close day carry closedAtSource backfill (approximate). all = open + closed. Reopen history is retained from this release onward.")
            .verb(Verb::Source).verb(Verb::Meta)
            .input(ledger_arg("counts"))
            .input(ArgSpec::new("days").summary("Only the most recent N UTC days; a positive integer.").class(v::ext::XSD_INTEGER).optional())
            .input(ArgSpec::new("as").summary("Representation media type.").class(v::ext::XSD_STRING).one_of(FACES).default_value(CSV).optional()).output(CSV).output(TURTLE))
    }
}

/// Validate the whole snapshot before writing, then migrate it in one guarded store
/// update. VALUES bounds the write to the reviewed snapshot; a concurrent edit, close,
/// reopen or delete cannot make us stamp a different state of an item.
async fn backfill_all(
    client: &StoreClient<'_, '_>,
    inv: &Invocation<'_>,
) -> Result<Representation> {
    if inv.inline_str("comment").is_ok() {
        return Err(Error::InvalidArgument {
            name: "comment".into(),
            detail: "comment is only supported for a single item; content=all uses updated".into(),
        });
    }
    let candidates = client.in_graph(&format!(
        "?item <{}> <{}> ; <{}> <{}> . \
         FILTER NOT EXISTS {{ ?item <{}> ?existing }} \
         OPTIONAL {{ ?item <{}> ?at }} OPTIONAL {{ ?item <{}> ?filed }}",
        v::ext::TYPE,
        v::ITEM_CLASS,
        v::STATUS,
        v::CLOSED,
        v::CLOSED_AT,
        v::ext::MODIFIED,
        v::ext::CREATED
    ));
    let rows = client
        .select(&format!("SELECT ?item ?at ?filed WHERE {{ {candidates} }}"))
        .await?;
    if rows.is_empty() {
        return Ok(plain("backfilled 0 items (unchanged)\n"));
    }
    let suffix = format!(":status:backfill:{}", mint_id(now_ms(inv)?)?);
    let mut seen = BTreeSet::new();
    let mut values = String::new();
    for row in rows {
        let item = &row["item"].value;
        let at = row.get("at").and_then(|b| model::millis(&b.value));
        let filed = row.get("filed").and_then(|b| model::millis(&b.value));
        let (Some(at), Some(filed)) = (at, filed) else {
            return Err(Error::Endpoint(format!(
                "backfill requires one readable updated and filed time for {item}; nothing changed"
            )));
        };
        if !seen.insert(item.clone()) || at < filed {
            return Err(Error::Endpoint(format!("backfill refuses multiple timestamps or updated before filing for {item}; nothing changed")));
        }
        values.push_str(&format!(
            "({} {} {} {} <{item}{suffix}>)\n",
            row["item"].term("item")?,
            datetime(at),
            row["at"].term("updated")?,
            row["filed"].term("filed")?
        ));
    }
    let triples = client.in_graph(&format!(
        "?item <{}> ?at ; <{}> \"backfill\" . \
         ?event <{}> <{}> ; <{}> ?item ; <{}> <{}> ; <{}> ?at ; <{}> 0 ; <{}> \"backfill\" .",
        v::CLOSED_AT,
        v::CLOSED_AT_SOURCE,
        v::ext::TYPE,
        v::STATUS_EVENT_CLASS,
        v::ON_ITEM,
        v::STATUS,
        v::CLOSED,
        v::ext::CREATED,
        v::EVENT_SEQUENCE,
        v::CLOSED_AT_SOURCE
    ));
    let guard = client.in_graph(&format!(
        "?item <{}> <{}> ; <{}> <{}> ; <{}> ?updated ; <{}> ?created . \
         FILTER NOT EXISTS {{ ?item <{}> ?existing }}",
        v::ext::TYPE,
        v::ITEM_CLASS,
        v::STATUS,
        v::CLOSED,
        v::ext::MODIFIED,
        v::ext::CREATED,
        v::CLOSED_AT
    ));
    client.update(&format!("INSERT {{ {triples} }} WHERE {{ VALUES (?item ?at ?updated ?created ?event) {{ {values} }} {guard} }}")).await?;
    // Count this invocation's events, rather than claiming every candidate was written
    // when another writer may have changed an item between our read and update.
    let dated = client
        .select(&format!(
            "SELECT ?event WHERE {{ {} FILTER(STRENDS(STR(?event), {})) }}",
            client.in_graph(&format!(
                "?event <{}> <{}>",
                v::ext::TYPE,
                v::STATUS_EVENT_CLASS
            )),
            crate::sparql::literal(&suffix)
        ))
        .await?
        .len();
    Ok(plain(format!(
        "backfilled {dated} items{}\n",
        if dated == 0 { " (unchanged)" } else { "" }
    )))
}

pub(super) struct BackfillEndpoint;

#[async_trait]
impl Endpoint for BackfillEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        if inv.request.verb != Verb::Sink {
            return Err(unsupported("ledger-backfill-closed-at", inv.request.verb));
        }
        let client = StoreClient::new(inv, ledger_for(inv, Need::Write)?);
        if inv.inline_str("content")? == "all" {
            return backfill_all(&client, inv).await;
        }
        let item = require_item(&client, inv.inline_str("content")?).await?;
        if item.open {
            return Err(Error::InvalidArgument {
                name: "content".into(),
                detail: "only a currently closed item can be backfilled".into(),
            });
        }
        if client
            .ask(&format!(
                "ASK {{ {} }}",
                client.in_graph(&format!("<{}> <{}> ?at", item.iri, v::CLOSED_AT))
            ))
            .await?
        {
            return Ok(plain(format!(
                "unchanged {} (existing closedAt preserved)\n",
                item.short()
            )));
        }
        // No inference from arbitrary comment prose: legacy comments do not record their
        // originating action. The operator identifies the close's OWN comment explicitly.
        let at = if let Ok(comment) = inv.inline_str("comment") {
            let comment = crate::sparql::iri(comment, "comment")?;
            let rows = client
                .select(&format!(
                    "SELECT ?at WHERE {{ {} }}",
                    client.in_graph(&format!(
                        "{comment} <{}> <{}> ; <{}> <{}> ; <{}> ?at",
                        v::ext::TYPE,
                        v::COMMENT_CLASS,
                        v::ON_ITEM,
                        item.iri,
                        v::ext::CREATED
                    ))
                ))
                .await?;
            if rows.len() != 1 {
                return Err(Error::InvalidArgument {
                    name: "comment".into(),
                    detail: "expected one dated closing comment belonging to this item".into(),
                });
            }
            rows[0]
                .get("at")
                .and_then(|b| model::millis(&b.value))
                .ok_or_else(|| Error::InvalidArgument {
                    name: "comment".into(),
                    detail: "the comment has no readable timestamp".into(),
                })?
        } else {
            // Read the real modified value, never model::Item's malformed-time stand-in.
            let rows = client
                .select(&format!(
                    "SELECT ?at WHERE {{ {} }}",
                    client.in_graph(&format!("<{}> <{}> ?at", item.iri, v::ext::MODIFIED))
                ))
                .await?;
            if rows.len() != 1 {
                return Err(Error::Endpoint(
                    "backfill requires one readable updated time".into(),
                ));
            }
            rows[0]
                .get("at")
                .and_then(|b| model::millis(&b.value))
                .ok_or_else(|| {
                    Error::Endpoint("backfill requires a readable updated time".into())
                })?
        };
        if at < item.created {
            return Err(Error::InvalidArgument {
                name: "comment".into(),
                detail: "close time precedes filing".into(),
            });
        }
        let event = format!("{}:status:{at}:closed", item.iri);
        // Atomic, idempotent, and never overwrites a precise close or races a reopen.
        let guard = client.in_graph(&format!(
            "{} <{}> <{}> <{}> . FILTER NOT EXISTS {{ <{}> <{}> ?existing }}",
            live(&item.iri),
            item.iri,
            v::STATUS,
            v::CLOSED,
            item.iri,
            v::CLOSED_AT
        ));
        let sequence = format!("<{event}> <{}> 0 .", v::EVENT_SEQUENCE);
        let triples = client.in_graph(&format!(
            "{sequence} <{}> <{}> {} ; <{}> \"backfill\" . \
             <{event}> <{}> <{}> ; <{}> <{}> ; <{}> <{}> ; <{}> {} ; <{}> \"backfill\" .",
            item.iri,
            v::CLOSED_AT,
            datetime(at),
            v::CLOSED_AT_SOURCE,
            v::ext::TYPE,
            v::STATUS_EVENT_CLASS,
            v::ON_ITEM,
            item.iri,
            v::STATUS,
            v::CLOSED,
            v::ext::CREATED,
            datetime(at),
            v::CLOSED_AT_SOURCE
        ));
        guarded_update(
            &client,
            &[&item.iri],
            &format!("INSERT {{ {triples} }} WHERE {{ {guard} }}"),
        )
        .await?;
        Ok(plain(format!(
            "backfill checked {} (existing closedAt preserved)\n",
            item.short()
        )))
    }
    fn name(&self) -> &str {
        "ledger-backfill-closed-at"
    }
    fn describe(&self) -> Description {
        Description::new("ledger-backfill-closed-at")
            .title("Explicitly backfill legacy close times")
            .summary("One-time migration, never run at startup. content=all dates every undated closed item from updated and reports how many changed. For one item, supply the close's own comment IRI when one exists; otherwise uses updated. Both are approximate and marked closedAtSource backfill. Existing closedAt values are preserved; updated is untouched.")
            .verb(Verb::Meta)
            .action(write_scopes(ikigai_core::ActionSpec::new(Verb::Sink)
                .input(ledger_arg("backfill-closed-at"))
                .input(ArgSpec::new("content").summary("A currently closed legacy item reference (#number, key, or IRI), or all to migrate this ledger using updated.").class(v::ext::XSD_STRING))
                .input(ArgSpec::new("comment").summary("The reviewed closing comment IRI, if the close posted one. Legacy data cannot identify this automatically.").class(v::ext::XSD_ANY_URI).optional())
                .output(PLAIN), CAP_WRITE))
    }
}
