//! An item's history (ledger #2841): `urn:iki:ledger:{ledger}:item:{id}:history`, every event
//! on one item, oldest first, under the item's read grant.
//!
//! The events are the ones the item's updates wrote: claims, releases, closes and reopens, and
//! edits with what they changed. This reads them and decides nothing.
//!
//! ⚠ **A ledger stays a boundary.** An event on a cross-ledger link names the other item by its
//! IRI, which the link itself shows this ledger's readers, and this face looks up nothing of it,
//! neither its title nor whether it still exists. The target's own history has no event for a
//! link it only mirrors. A close's evidence is the same (ledger #2842): its IRIs, as the close
//! stored them.

use std::collections::{BTreeMap, BTreeSet};

use super::*;

use crate::lifecycle;
use crate::sparql::iso8601;

const HISTORY_FACES: [&str; 2] = [PLAIN, JSON];

/// One event node as the store holds it: its class and, per predicate, its values.
#[derive(Default)]
struct Node {
    class: String,
    values: BTreeMap<String, BTreeSet<String>>,
}

impl Node {
    fn all(&self, predicate: &str) -> Vec<String> {
        self.values
            .get(predicate)
            .map(|held| held.iter().cloned().collect())
            .unwrap_or_default()
    }

    fn one(&self, predicate: &str) -> Option<String> {
        self.values.get(predicate)?.iter().next().cloned()
    }
}

/// The events of `item`, in ONE query, oldest first; events of one millisecond in the order
/// their ids were minted.
async fn read(client: &StoreClient<'_, '_>, item: &str) -> Result<Vec<json::HistoryEvent>> {
    let rows = client
        .select(&format!(
            "SELECT ?event ?class ?p ?o WHERE {{ {} }}",
            client.in_graph(&format!(
                "?event <{on}> <{item}> ; <{type_}> ?class ; ?p ?o . \
                 FILTER(?class IN (<{claim}>, <{release}>, <{status}>, <{edit}>))",
                on = v::ON_ITEM,
                type_ = v::ext::TYPE,
                claim = v::CLAIM_EVENT_CLASS,
                release = v::RELEASE_EVENT_CLASS,
                status = v::STATUS_EVENT_CLASS,
                edit = v::EDIT_EVENT_CLASS,
            ))
        ))
        .await?;
    let mut nodes: BTreeMap<String, Node> = BTreeMap::new();
    for row in &rows {
        let (Some(event), Some(class), Some(p), Some(o)) = (
            row.get("event"),
            row.get("class"),
            row.get("p"),
            row.get("o"),
        ) else {
            continue;
        };
        let node = nodes.entry(event.value.clone()).or_default();
        node.class = class.value.clone();
        node.values
            .entry(p.value.clone())
            .or_default()
            .insert(o.value.clone());
    }
    let mut events: Vec<(u64, String, json::HistoryEvent)> = nodes
        .into_iter()
        .map(|(iri, node)| {
            let at = node
                .one(v::ext::CREATED)
                .and_then(|when| model::millis(&when));
            let id = iri.rsplit(':').next().unwrap_or(&iri).to_string();
            (at.unwrap_or(0), id, event(iri, &node, at))
        })
        .collect();
    events.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    Ok(events.into_iter().map(|(_, _, event)| event).collect())
}

fn event(iri: String, node: &Node, at: Option<u64>) -> json::HistoryEvent {
    let kind = if node.class == v::CLAIM_EVENT_CLASS {
        "claim".to_string()
    } else if node.class == v::RELEASE_EVENT_CLASS {
        "release".to_string()
    } else if node.class == v::STATUS_EVENT_CLASS {
        if node.one(v::STATUS).as_deref() == Some(v::CLOSED) {
            "closed".to_string()
        } else {
            "reopened".to_string()
        }
    } else {
        node.one(v::EDIT_KIND).unwrap_or_else(|| "edit".to_string())
    };
    // An edit's fields, in the order an item reads rather than the order the store sorts them.
    let mut fields = node.all(v::EDIT_FIELD);
    fields.sort_by_key(|field| match field.as_str() {
        "title" => 0,
        "body" => 1,
        "priority" => 2,
        "revision" => 3,
        _ => 4,
    });
    json::HistoryEvent {
        iri,
        kind,
        at: at.map(iso8601),
        committer: node.one(v::COMMITTER),
        label: node.one(v::EDIT_LABEL),
        state_from: node.one(v::STATE_FROM),
        state_to: node.one(v::STATE_TO),
        link_type: node
            .one(v::EDIT_LINK_TYPE)
            .map(|predicate| across::short(&predicate).to_string()),
        targets: node.all(v::EDIT_TARGET),
        fields,
        holder: node.one(v::CLAIMED_BY),
        reason: node.one(v::RELEASE_REASON).or_else(|| {
            let reason = node.one(v::CLOSED_REASON)?;
            v::close_reason_name(&reason).map(str::to_string)
        }),
        evidence: node.all(v::EVIDENCE),
    }
}

/// What a close names: its verdict and its evidence, with an item on a `duplicate` or
/// `superseded` close named as its target. Empty for a close written before 0.18.0, which holds
/// neither.
fn closed(event: &json::HistoryEvent) -> String {
    let Some(reason) = &event.reason else {
        return String::new();
    };
    let words = evidence::target_words(reason);
    let (targets, rest): (Vec<&String>, Vec<&String>) = event
        .evidence
        .iter()
        .partition(|iri| words.is_some() && Ledger::item_iri(iri).is_some());
    let join = |iris: &[&String]| {
        iris.iter()
            .map(|iri| iri.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut said = match words {
        Some(words) if !targets.is_empty() => format!("{words} {}", join(&targets)),
        _ => reason.clone(),
    };
    if !rest.is_empty() {
        said.push_str(&format!(", evidence {}", join(&rest)));
    }
    said
}

/// An event's line: its time, then what happened in words, then who.
fn line(event: &json::HistoryEvent) -> String {
    let mut what = event.kind.clone();
    let targets = event.targets.join(" ");
    let object = match event.kind.as_str() {
        "label" | "unlabel" => event.label.clone(),
        "state" => event
            .state_from
            .as_ref()
            .zip(event.state_to.as_ref())
            .map(|(from, to)| {
                format!(
                    "{} to {}",
                    lifecycle::state_name(from),
                    lifecycle::state_name(to)
                )
            }),
        "link" | "unlink" => event
            .link_type
            .as_ref()
            .map(|link_type| format!("{link_type} {targets}")),
        "about" | "unabout" => (!targets.is_empty()).then_some(targets),
        "edit" => (!event.fields.is_empty()).then(|| event.fields.join(", ")),
        "claim" => event.holder.clone(),
        "release" => event.holder.as_ref().map(|holder| match &event.reason {
            Some(reason) => format!("{holder} ({reason})"),
            None => holder.clone(),
        }),
        "closed" => Some(closed(event)),
        // A defer, a resume and a reopen have nothing more to name.
        _ => Some(String::new()),
    };
    match object {
        Some(object) if object.is_empty() => {}
        Some(object) => what.push_str(&format!(" {object}")),
        None => what.push_str(" (no object recorded: nothing changed, or written before 0.18.0)"),
    }
    if let Some(committer) = &event.committer {
        what.push_str(&format!("  by {committer}"));
    }
    format!(
        "{}  {what}\n",
        event.at.as_deref().unwrap_or("an unknown time")
    )
}

/// `urn:iki:ledger:{ledger}:item:{id}:history`.
#[derive(Clone)]
pub(super) struct HistoryEndpoint;

#[async_trait]
impl Endpoint for HistoryEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        if inv.request.verb != Verb::Source {
            return Err(unsupported("ledger-item-history", inv.request.verb));
        }
        let client = StoreClient::new(inv, ledger_for(inv, Need::Read)?);
        let want = wanted_face(inv, &HISTORY_FACES)?;
        let item = require_item(&client, flight::part_id(inv)?).await?;
        let events = read(&client, &item.iri).await?;
        if want == JSON {
            let ledger = client.ledger();
            return Ok(json_face(json::render(&json::HistoryDocument {
                schema: json::SCHEMA,
                ledger: ledger.name().to_string(),
                item: json::item_ref(ledger, item.number, &item.iri),
                events,
            })?));
        }
        let mut text = if events.is_empty() {
            format!("{} has no recorded events\n", item.short())
        } else {
            format!("{}: {} event(s)\n", item.short(), events.len())
        };
        for event in &events {
            text.push_str(&line(event));
        }
        face(text, None, PLAIN)
    }

    fn name(&self) -> &str {
        "ledger-item-history"
    }

    fn describe(&self) -> Description {
        Description::new("ledger-item-history")
            .title("An item's history")
            .summary(
                "Every event on one item, oldest first: its claims, releases, closes with their \
                 verdict and evidence, reopens, and its edits with what each changed (the label, \
                 the states before and after, the link and the other item, the about target, the \
                 names of the fields, never their values). The other item of a link across \
                 ledgers, and a close's evidence, are named by IRI and nothing else.",
            )
            .verb(Verb::Meta)
            .action(read_action(
                ikigai_core::ActionSpec::new(Verb::Source)
                    .input(ledger_arg("item:{id}:history"))
                    .summary(
                        "One line per event in text, under a count; in JSON, the events with the \
                         states as IRIs.",
                    )
                    .input(flight::item_id_input())
                    .input(as_arg(&HISTORY_FACES))
                    .output(PLAIN)
                    .output(JSON),
            ))
    }
}
