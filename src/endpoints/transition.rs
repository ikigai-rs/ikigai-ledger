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
//! sequence order, under the ledger's read grant, in the layer feed's shape: `last` is how far
//! the read got (a follower's next `after=`, past any gap), `epoch` names the feed's
//! incarnation, and a cursor past the head or from another epoch is refused as `Conflict`.
//! Each entry carries the item's claim epoch and the principal of the write. ⚠ Not yet built:
//! `wait=` (a long poll), the entries as their own resources, and a renewal (which writes no
//! node of its own).
//!
//! ★ Entries are immutable: delete and purge take an item's records, never the entries that
//! name it (`Selector::inbound_filter` leaves `transitionItem` and `transitionEvent` alone).

use super::*;

const FEED_FACES: [&str; 2] = [PLAIN, JSON];

/// The most entries one read returns, and its default.
const MAX_LIMIT: usize = 500;

/// The operation that gives `node`, just written on `item`, its entry in the ledger's
/// transition feed, when `node` records a transition. It goes after [`super::revise`]'s
/// revision in the same update. A node that is already in the feed (an entry names it as its
/// `transitionEvent`), or is not in the graph (its write's own guards refused it), or is not
/// a transition (a plain comment, an edit), allocates nothing.
///
/// The sequence lives on the entry only, never on the node: an event keeps the shape its
/// readers (history, attribution) already know, and the entry's `transitionEvent` is the one
/// link between them.
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
            "<{feed}> <{head}> ?seq ; <{feed_epoch}> ?feedEpoch .\n\
             ?entry <{type_}> <{class}> ; <{seq_p}> ?seq ; <{on_p}> <{item}> ; \
             <{kind_p}> ?kind ; <{event_p}> <{node}> ; <{created}> ?at ; \
             <{epoch_p}> ?epoch ; <{principal_p}> ?principal .",
            head = v::TRANSITION_HEAD,
            feed_epoch = v::TRANSITION_FEED_EPOCH,
            epoch_p = v::TRANSITION_EPOCH,
            principal_p = v::TRANSITION_PRINCIPAL,
            seq_p = v::TRANSITION_SEQ,
            type_ = v::ext::TYPE,
            class = v::TRANSITION_CLASS,
            on_p = v::TRANSITION_ITEM,
            kind_p = v::TRANSITION_KIND,
            event_p = v::TRANSITION_EVENT,
            created = v::ext::CREATED,
        )),
        where_ = client.in_graph(&format!(
            "{on} <{node}> <{type_}> ?class .\n\
             FILTER NOT EXISTS {{ ?seen <{event_p}> <{node}> }}\n\
             OPTIONAL {{ <{node}> <{status}> ?status }}\n\
             OPTIONAL {{ <{node}> <{hand_back}> ?handBack }}\n\
             OPTIONAL {{ <{node}> <{created}> ?at }}\n\
             OPTIONAL {{ <{node}> <{committer}> ?principal }}\n\
             OPTIONAL {{ <{item}> <{claim_epoch}> ?claimEpoch }}\n\
             OPTIONAL {{ <{node}> <{lapsed_epoch}> ?lapsedEpoch }}\n\
             BIND(COALESCE(?lapsedEpoch, ?claimEpoch) AS ?epoch)\n\
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
             OPTIONAL {{ <{feed}> <{feed_epoch}> ?feedEpochWas }}\n\
             BIND(COALESCE(?headWas, 0) + 1 AS ?seq)\n\
             BIND(COALESCE(?feedEpochWas, STRUUID()) AS ?feedEpoch)\n\
             BIND(IRI(CONCAT(\"{feed}:seq:\", STR(?seq))) AS ?entry)",
            type_ = v::ext::TYPE,
            event_p = v::TRANSITION_EVENT,
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
            feed_epoch = v::TRANSITION_FEED_EPOCH,
            committer = v::COMMITTER,
            claim_epoch = v::CLAIM_EPOCH,
            lapsed_epoch = v::LAPSED_EPOCH,
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
    /// The item's claim epoch when the transition was written; none for an item never claimed.
    #[serde(skip_serializing_if = "Option::is_none")]
    epoch: Option<u64>,
    /// The principal the door minted for the write.
    #[serde(skip_serializing_if = "Option::is_none")]
    principal: Option<String>,
    /// Authored time, for display; never the order.
    at: Option<String>,
}

/// The layer feed's shape (design 8.1): `last` is how far the read got, which a follower
/// passes back as its next `after=` whatever entries the range held.
#[derive(serde::Serialize)]
struct FeedDocument {
    schema: u32,
    ledger: String,
    /// The feed's incarnation, minted with its first entry; none while it has none.
    epoch: Option<String>,
    after: u64,
    last: u64,
    head: u64,
    entries: Vec<Entry>,
}

/// What a read found: the feed's head and epoch, how far the read got, and the entries.
struct Page {
    head: u64,
    epoch: Option<String>,
    last: u64,
    entries: Vec<Entry>,
}

async fn read(
    client: &StoreClient<'_, '_>,
    after: u64,
    limit: usize,
    epoch: Option<&str>,
) -> Result<Page> {
    let feed = format!("{}transition", client.ledger().prefix());
    let rows = client
        .select(&format!(
            "SELECT ?head ?epoch WHERE {{ {} }}",
            client.in_graph(&format!(
                "<{feed}> <{}> ?head OPTIONAL {{ <{feed}> <{}> ?epoch }}",
                v::TRANSITION_HEAD,
                v::TRANSITION_FEED_EPOCH
            ))
        ))
        .await?;
    let row = rows.first();
    let head: u64 = row
        .and_then(|row| row.get("head")?.value.parse().ok())
        .unwrap_or(0);
    let feed_epoch = row.and_then(|row| Some(row.get("epoch")?.value.clone()));
    let ledger = client.ledger().name().to_string();
    if let Some(asked) = epoch {
        if Some(asked) != feed_epoch.as_deref() {
            return Err(Error::Conflict(format!(
                "the cursor on ledger `{ledger}`'s transitions is from feed epoch {asked}, and \
                 this feed is {}: it is not the feed the cursor came from (restored, or another \
                 store), so resynchronize from 0{}",
                feed_epoch
                    .as_deref()
                    .map_or("empty, with no epoch yet".to_string(), |e| format!(
                        "epoch {e}"
                    )),
                feed_epoch
                    .as_deref()
                    .map_or(String::new(), |e| format!(" with epoch={e}")),
            )));
        }
    }
    if after > head {
        return Err(Error::Conflict(format!(
            "cursor {after} is past ledger `{ledger}`'s last transition, {head}: this feed is not \
             the one the cursor came from (restored, or another store), so resynchronize from 0"
        )));
    }
    // How far this read gets: a gap in the range (an entry a write through the broad door
    // removed) still moves the follower on, so it never stalls below the head.
    let last = head.min(after.saturating_add(limit as u64));
    if last <= after {
        return Ok(Page {
            head,
            epoch: feed_epoch,
            last,
            entries: Vec::new(),
        });
    }
    // The entries are named by their sequence, so the page is a VALUES list over the
    // subjects: the read names every subject it touches.
    let subjects: String = (after + 1..=last)
        .map(|n| format!("<{feed}:seq:{n}> "))
        .collect();
    let rows = client
        .select(&format!(
            "SELECT ?entry ?seq ?item ?kind ?event ?epoch ?principal ?at WHERE {{ {} }} ORDER BY ?seq",
            client.in_graph(&format!(
                "VALUES ?entry {{ {subjects}}} ?entry <{seq}> ?seq ; <{on}> ?item ; \
                 <{kind}> ?kind ; <{event}> ?event . OPTIONAL {{ ?entry <{created}> ?at }} \
                 OPTIONAL {{ ?entry <{epoch}> ?epoch }} OPTIONAL {{ ?entry <{principal}> ?principal }}",
                seq = v::TRANSITION_SEQ,
                on = v::TRANSITION_ITEM,
                kind = v::TRANSITION_KIND,
                event = v::TRANSITION_EVENT,
                created = v::ext::CREATED,
                epoch = v::TRANSITION_EPOCH,
                principal = v::TRANSITION_PRINCIPAL,
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
            epoch: get("epoch").and_then(|e| e.parse().ok()),
            principal: get("principal"),
            at: get("at"),
        });
    }
    Ok(Page {
        head,
        epoch: feed_epoch,
        last,
        entries,
    })
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
        let epoch = inv.optional_str("epoch")?.map(str::trim);
        let page = read(&client, after, limit, epoch).await?;
        if want == JSON {
            let doc = serde_json::to_vec_pretty(&FeedDocument {
                schema: json::SCHEMA,
                ledger: client.ledger().name().to_string(),
                epoch: page.epoch,
                after,
                last: page.last,
                head: page.head,
                entries: page.entries,
            })
            .map_err(|e| Error::Endpoint(format!("the transition feed did not serialize: {e}")))?;
            return Ok(json_face(doc));
        }
        let (head, last) = (page.head, page.last);
        let count = match page.entries.len() {
            0 => "No transitions".to_string(),
            1 => "1 transition".to_string(),
            n => format!("{n} transitions"),
        };
        let mut text = if last == after {
            format!("{count} after {after}. The last is {head}.\n")
        } else {
            format!("{count} after {after}, read through {last}. The last is {head}.\n")
        };
        for entry in &page.entries {
            let at = entry.at.as_deref().unwrap_or("an unknown time");
            text.push_str(&format!(
                "{}  {}  {}  at {at}",
                entry.seq, entry.kind, entry.item
            ));
            if let Some(epoch) = entry.epoch {
                text.push_str(&format!("  epoch {epoch}"));
            }
            if let Some(principal) = &entry.principal {
                text.push_str(&format!("  by {principal}"));
            }
            text.push('\n');
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
                        "One line per entry in text (its number, kind, item, time, epoch and \
                         principal), under a count, how far the read got and the last number; in \
                         JSON, `ledger`, `epoch`, `last` (pass it back as `after=`), `head` and \
                         the entries.",
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
                    .input(
                        ArgSpec::new("epoch")
                            .summary(
                                "The feed epoch the cursor came from, as the JSON face named it: \
                                 a cursor from another epoch is refused as Conflict, as is one \
                                 past the last entry, so resynchronize from 0.",
                            )
                            .class(v::ext::XSD_STRING)
                            .optional(),
                    )
                    .input(as_arg(&FEED_FACES))
                    .output(PLAIN)
                    .output(JSON),
            ))
    }
}
