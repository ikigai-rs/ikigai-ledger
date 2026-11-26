//! The transition feed, the ledger's outbox (ledger #4116; ikigai-core
//! `docs/design/events-and-resources.md`, 8.1).
//!
//! ★ **Written in the same store update as the state it records.** [`record`] is one more
//! operation in the batch that writes a transition's node (the item on a filing, or a claim,
//! release, lapse or status event, or a hand-back comment), so the state and its entry commit
//! together or not at all: an outbox, not a dual write. The sequence is allocated under the
//! store's write lock from `ledger:transitionHead`, so it is COMMIT order, per ledger, and
//! never authored time: an entry also carries its node's `dcterms:created`, which a host whose
//! clock runs slow can make earlier than the entry before it (the clock research, rule 3).
//!
//! `urn:iki:ledger:{ledger}:transition after={n} limit={k}` reads the entries past `n`, in
//! sequence order, under the ledger's read grant. ⚠ Not yet built: `wait=` (a long poll), the
//! entries as their own resources, a renewal (which writes no node of its own) and the claim's
//! epoch on the entry itself (it is on the item, and on a lapse event).

use super::*;

const FEED_FACES: [&str; 2] = [PLAIN, JSON];

/// The most entries one read returns, and its default.
const MAX_LIMIT: usize = 500;

/// The operation that gives `node`, just written on `item`, its entry in the ledger's
/// transition feed, when `node` records a transition. It goes after [`super::revise`]'s
/// revision in the same update. The guard is the node itself, as there: a node that is
/// already in the feed, or is not in the graph (its write's own guards refused it), or is
/// not a transition (a plain comment, an edit), allocates nothing.
pub(crate) fn record(client: &StoreClient<'_, '_>, item: &str, node: &str) -> String {
    let ledger = client.ledger();
    let feed = format!("{}transition", ledger.prefix());
    let on = if node == item {
        String::new()
    } else {
        format!("<{node}> <{}> <{item}> .", v::ON_ITEM)
    };
    format!(
        "DELETE {{ {delete} }} INSERT {{ {insert} }} WHERE {{ {where_} }}",
        delete = client.in_graph(&format!(
            "<{feed}> <{head}> ?headWas",
            head = v::TRANSITION_HEAD
        )),
        insert = client.in_graph(&format!(
            "<{feed}> <{head}> ?seq . <{node}> <{seq_p}> ?seq .\n\
             ?entry <{type_}> <{class}> ; <{seq_p}> ?seq ; <{on_p}> <{item}> ; \
             <{kind_p}> ?kind ; <{event_p}> <{node}> ; <{created}> ?at .",
            head = v::TRANSITION_HEAD,
            seq_p = v::TRANSITION_SEQ,
            type_ = v::ext::TYPE,
            class = v::TRANSITION_CLASS,
            on_p = v::ON_ITEM,
            kind_p = v::TRANSITION_KIND,
            event_p = v::TRANSITION_EVENT,
            created = v::ext::CREATED,
        )),
        where_ = client.in_graph(&format!(
            "{on} <{node}> <{type_}> ?class .\n\
             FILTER NOT EXISTS {{ <{node}> <{seq_p}> ?seqAny }}\n\
             OPTIONAL {{ <{node}> <{status}> ?status }}\n\
             OPTIONAL {{ <{node}> <{hand_back}> ?handBack }}\n\
             OPTIONAL {{ <{node}> <{created}> ?at }}\n\
             BIND(IF(?class = <{item_c}>, \"filed\", \
                  IF(?class = <{claim_c}>, \"claimed\", \
                  IF(?class = <{release_c}>, \"released\", \
                  IF(?class = <{lapse_c}>, \"lapsed\", \
                  IF(?class = <{status_c}> && ?status = <{closed}>, \"closed\", \
                  IF(?class = <{status_c}> && ?status = <{open}>, \"reopened\", \
                  IF(?class = <{comment_c}> && BOUND(?handBack), \"handed back\", \
                  ?none))))))) AS ?kind)\n\
             FILTER(BOUND(?kind))\n\
             OPTIONAL {{ <{feed}> <{head}> ?headWas }}\n\
             BIND(COALESCE(?headWas, 0) + 1 AS ?seq)\n\
             BIND(IRI(CONCAT(\"{feed}:seq:\", STR(?seq))) AS ?entry)",
            type_ = v::ext::TYPE,
            seq_p = v::TRANSITION_SEQ,
            status = v::STATUS,
            hand_back = v::HAND_BACK,
            created = v::ext::CREATED,
            item_c = v::ITEM_CLASS,
            claim_c = v::CLAIM_EVENT_CLASS,
            release_c = v::RELEASE_EVENT_CLASS,
            lapse_c = v::LAPSE_EVENT_CLASS,
            status_c = v::STATUS_EVENT_CLASS,
            comment_c = v::COMMENT_CLASS,
            closed = v::CLOSED,
            open = v::OPEN,
            head = v::TRANSITION_HEAD,
        )),
    )
}

/// One entry as read back.
#[derive(serde::Serialize)]
struct Entry {
    seq: u64,
    iri: String,
    item: String,
    kind: String,
    event: String,
    /// Authored time, for display; never the order.
    at: Option<String>,
}

#[derive(serde::Serialize)]
struct FeedDocument {
    schema: u32,
    ledger: String,
    after: u64,
    head: u64,
    entries: Vec<Entry>,
}

async fn read(client: &StoreClient<'_, '_>, after: u64, limit: usize) -> Result<(u64, Vec<Entry>)> {
    let feed = format!("{}transition", client.ledger().prefix());
    let head = client
        .select(&format!(
            "SELECT ?head WHERE {{ {} }}",
            client.in_graph(&format!("<{feed}> <{}> ?head", v::TRANSITION_HEAD))
        ))
        .await?
        .first()
        .and_then(|row| row.get("head")?.value.parse().ok())
        .unwrap_or(0);
    // The entries are named by their sequence, so the page is a VALUES list over the
    // subjects: the read names every subject it touches.
    let last = head.min(after.saturating_add(limit as u64));
    if last <= after {
        return Ok((head, Vec::new()));
    }
    let subjects: String = (after + 1..=last)
        .map(|n| format!("<{feed}:seq:{n}> "))
        .collect();
    let rows = client
        .select(&format!(
            "SELECT ?entry ?seq ?item ?kind ?event ?at WHERE {{ {} }} ORDER BY ?seq",
            client.in_graph(&format!(
                "VALUES ?entry {{ {subjects}}} ?entry <{seq}> ?seq ; <{on}> ?item ; \
                 <{kind}> ?kind ; <{event}> ?event . OPTIONAL {{ ?entry <{created}> ?at }}",
                seq = v::TRANSITION_SEQ,
                on = v::ON_ITEM,
                kind = v::TRANSITION_KIND,
                event = v::TRANSITION_EVENT,
                created = v::ext::CREATED,
            ))
        ))
        .await?;
    let mut entries = Vec::with_capacity(rows.len());
    for row in rows {
        let get = |name: &str| row.get(name).map(|b| b.value.clone());
        let seq = get("seq")
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| Error::Endpoint("a transition entry has no integer sequence".into()))?;
        entries.push(Entry {
            seq,
            iri: get("entry").unwrap_or_default(),
            item: get("item").unwrap_or_default(),
            kind: get("kind").unwrap_or_default(),
            event: get("event").unwrap_or_default(),
            at: get("at"),
        });
    }
    Ok((head, entries))
}

/// `urn:iki:ledger:{ledger}:transition`.
#[derive(Clone)]
pub(super) struct TransitionFeedEndpoint;

#[async_trait]
impl Endpoint for TransitionFeedEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        if inv.request.verb != Verb::Source {
            return Err(unsupported("ledger-transition", inv.request.verb));
        }
        let client = StoreClient::new(inv, ledger_for(inv, Need::Read)?);
        let want = wanted_face(inv, &FEED_FACES)?;
        let after = parse_count(inv, "after", 0)? as u64;
        let limit = match parse_count(inv, "limit", MAX_LIMIT)? {
            0 => MAX_LIMIT,
            n => n.min(MAX_LIMIT),
        };
        let (head, entries) = read(&client, after, limit).await?;
        if want == JSON {
            let doc = serde_json::to_vec_pretty(&FeedDocument {
                schema: json::SCHEMA,
                ledger: client.ledger().name().to_string(),
                after,
                head,
                entries,
            })
            .map_err(|e| Error::Endpoint(format!("the transition feed did not serialize: {e}")))?;
            return Ok(json_face(doc));
        }
        let mut text = match entries.len() {
            0 => format!("No transitions after {after}. The last is {head}.\n"),
            n => format!("{n} transition(s) after {after}. The last is {head}.\n"),
        };
        for entry in &entries {
            let at = entry.at.as_deref().unwrap_or("an unknown time");
            text.push_str(&format!(
                "{}  {}  {}  at {at}\n",
                entry.seq, entry.kind, entry.item
            ));
        }
        face(text, None, PLAIN)
    }

    fn name(&self) -> &str {
        "ledger-transition"
    }

    fn describe(&self) -> Description {
        Description::new("ledger-transition")
            .title("The ledger's transition feed")
            .summary(
                "The ledger's transitions in commit order: each filing, claim, release, \
                 hand-back, lapse, close and reopen, numbered from 1 in the same store update \
                 that made it. Read the entries after a position with `after=`. The time on \
                 an entry is when it was authored, for display: order by the number.",
            )
            .verb(Verb::Meta)
            .action(read_action(
                ikigai_core::ActionSpec::new(Verb::Source)
                    .input(ledger_arg("transition"))
                    .summary(
                        "One line per entry in text (its number, kind, item and time), under a \
                         count and the last number; in JSON, the entries and `head`.",
                    )
                    .input(
                        ArgSpec::new("after")
                            .summary("Read the entries numbered after this one (default 0, from the start).")
                            .class(v::ext::XSD_INTEGER)
                            .default_value("0")
                            .optional(),
                    )
                    .input(
                        ArgSpec::new("limit")
                            .summary("At most this many entries (default and most 500; `0` means 500).")
                            .class(v::ext::XSD_INTEGER)
                            .default_value("500")
                            .optional(),
                    )
                    .input(as_arg(&FEED_FACES))
                    .output(PLAIN)
                    .output(JSON),
            ))
    }
}
