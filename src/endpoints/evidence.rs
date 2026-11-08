//! Evidence on a close (ledger #2842): what closed an item, recorded as `ledger:evidence` on
//! that close's `ledger:StatusEvent`, in the same update as the close.
//!
//! The role is read, never stored: from the reason beside it on the event and the target's kind.
//! A commit on a `done` close is the fix, an item on a `duplicate` or `superseded` close is its
//! target, a test is the proof. So the event carries its `ledger:closedReason` too, because the
//! item's own goes on a reopen.
//!
//! - ★ **An item is resolved like a link's object, or the close is refused.** It must exist and
//!   the closer must be able to read it, whatever the reason. In another ledger that is #2829's
//!   rule: `link-in` on that ledger, checked before the lookup, so a caller without it learns
//!   nothing. There is no mirror, because nothing in the target's ledger reads a close here.
//! - ★ **Anything else is stored as given, and never fetched.** That covers `gh:acme/web#12`
//!   too: a short alias cannot be told from a foreign IRI without reading the aliases graph, a
//!   grant no plain close should need. The alias's own IRI, `urn:iki:alias:scheme:…`, resolves.
//! - ★ **Which close it lands on is decided inside the update** (the 0.14.x races, ledger #2764,
//!   #2766): the one that stands once the close has run. A first close finds the event it just
//!   wrote. A close that lost a race, or that found the item already closed, finds that close,
//!   so nothing it was given is lost and no second close is recorded.
//! - **Evidence is history, not an edge.** A target deleted later, or while the close was in
//!   flight, goes on being named by its IRI, as `ledger:editTarget` names one
//!   ([`Selector`]).

use super::*;

/// One value of `evidence=`, resolved.
pub(super) struct Evidence {
    /// What is stored: the item's IRI, or the IRI as given.
    pub(super) iri: String,
    /// The item it names, with the ledger that holds it; `None` for anything else.
    item: Option<(Ledger, i64)>,
}

/// Whether `reference` names an item, by its spelling alone and with no lookup: a number
/// (`12`, `#12`, `acme#7`), a bare id or a key, an item's IRI, or an alias's IRI.
fn names_item(reference: &str) -> bool {
    !reference.contains(':')
        || reference.starts_with(model::KEY_PREFIX)
        || Ledger::item_iri(reference).is_some()
        || (reference.starts_with(crate::alias::SCHEME_PREFIX)
            && crate::alias::split(reference).is_some())
}

/// `evidence=` as the close will record it, every value checked and nothing written: a refusal
/// here leaves the item as it was. Empty when the argument is absent or blank.
pub(super) async fn resolve(
    inv: &Invocation<'_>,
    client: &StoreClient<'_, '_>,
    closing: &Item,
) -> Result<Vec<Evidence>> {
    let mut resolved: Vec<Evidence> = Vec::new();
    for reference in inv.inline_str("evidence").unwrap_or("").split_whitespace() {
        let one = one(inv, client, reference).await?;
        if one.iri == closing.iri {
            return Err(Error::InvalidArgument {
                name: "evidence".to_string(),
                detail: format!(
                    "`{reference}` is {}, the item being closed: an item is not evidence of \
                     its own close. Nothing was changed",
                    closing.short()
                ),
            });
        }
        if !resolved.iter().any(|held| held.iri == one.iri) {
            resolved.push(one);
        }
    }
    Ok(resolved)
}

async fn one(
    inv: &Invocation<'_>,
    client: &StoreClient<'_, '_>,
    reference: &str,
) -> Result<Evidence> {
    if let Some(there) = across::other_ledger(client.ledger(), reference) {
        let there = grant_for(inv, there, Need::LinkIn)?;
        let target = StoreClient::new(inv, there.clone());
        let item = require_target(&target, reference).await?;
        return Ok(Evidence {
            iri: item.iri,
            item: Some((there, item.number)),
        });
    }
    if names_item(reference) {
        let item = require_target(client, reference).await?;
        return Ok(Evidence {
            iri: item.iri,
            item: Some((client.ledger().clone(), item.number)),
        });
    }
    sparql::iri(reference, "evidence")?;
    Ok(Evidence {
        iri: reference.to_string(),
        item: None,
    })
}

/// [`require_item`], with a refusal that says it was the evidence and not the item being closed.
async fn require_target(client: &StoreClient<'_, '_>, reference: &str) -> Result<Item> {
    let said = |detail: String| format!("evidence `{reference}`: {detail}. Nothing was changed");
    require_item(client, reference).await.map_err(|e| match e {
        Error::NotFound(detail) => Error::NotFound(said(detail)),
        Error::InvalidArgument { detail, .. } => Error::InvalidArgument {
            name: "evidence".to_string(),
            detail: said(detail),
        },
        other => other,
    })
}

/// The pattern binding `?standing`, the close `item` stands on: its latest status event, when
/// that is a close. Status events are numbered by `ledger:eventSequence`, because their dates
/// can be an importer's.
///
/// ★ In a write, `events` lists the item's status events beside each pattern on them (ledger
/// #3256): unlisted, `?standing <status> <closed>` copied every closed item's status into the
/// store's scoped update, and `?later <eventSequence> ?m` every status event's sequence. A read
/// copies nothing, and passes `None`.
fn standing(item: &str, events: Option<&counts::StatusEvents>) -> String {
    let listed = |var: &str| events.map(|events| events.values(var)).unwrap_or_default();
    format!(
        "{standing_values} ?standing <{type_}> <{class}> ; <{on}> <{item}> ; \
         <{status}> <{closed}> ; <{sequence}> ?n . \
         FILTER NOT EXISTS {{ {later_values} ?later <{type_}> <{class}> ; <{on}> <{item}> ; \
         <{sequence}> ?m . FILTER(?m > ?n) }}",
        standing_values = listed("?standing"),
        later_values = listed("?later"),
        type_ = v::ext::TYPE,
        class = v::STATUS_EVENT_CLASS,
        on = v::ON_ITEM,
        status = v::STATUS,
        closed = v::CLOSED,
        sequence = v::EVENT_SEQUENCE,
    )
}

/// A close written before status events, or dropped in through the store, has no event to hang
/// evidence on: refused before anything is written, because the update would quietly record
/// none. This one may be read first, since nothing but a reopen and a new close ends the state,
/// and then a retry finds an event.
pub(super) async fn refuse_without_a_close_event(
    client: &StoreClient<'_, '_>,
    item: &Item,
    evidence: &[Evidence],
) -> Result<()> {
    if evidence.is_empty() || item.open {
        return Ok(());
    }
    let found = client
        .ask(&format!(
            "ASK {{ {} }}",
            client.in_graph(&standing(&item.iri, None))
        ))
        .await?;
    if found {
        return Ok(());
    }
    Err(Error::InvalidArgument {
        name: "evidence".to_string(),
        detail: format!(
            "{} is closed, but its close was recorded with no status event, so there is nothing \
             to put the evidence on. Nothing was changed. To record it, reopen the item and \
             close it again with this evidence",
            item.short()
        ),
    })
}

/// The operation that puts `reason` and `evidence` on the close that stands. It must run after
/// the operations that write the close, so a first close finds the event it just wrote, and it
/// replaces the event's reason because a verdict edit changes that close's verdict. `events`
/// must list the event the close mints.
pub(super) fn record(
    client: &StoreClient<'_, '_>,
    item: &str,
    events: &counts::StatusEvents,
    reason: &str,
    evidence: &[Evidence],
    guard: &str,
) -> String {
    let mut insert = format!("?standing <{}> <{reason}> .", v::CLOSED_REASON);
    for one in evidence {
        insert.push_str(&format!(" ?standing <{}> <{}> .", v::EVIDENCE, one.iri));
    }
    format!(
        "DELETE {{ {} }} INSERT {{ {} }} WHERE {{ {} }}",
        client.in_graph(&format!("?standing <{}> ?was", v::CLOSED_REASON)),
        client.in_graph(&insert),
        client.in_graph(&format!(
            "{} {guard} <{item}> <{}> <{}> . {} OPTIONAL {{ {} ?standing <{}> ?was }}",
            live(item),
            v::STATUS,
            v::CLOSED,
            standing(item, Some(events)),
            // An `OPTIONAL` is narrowed only by a list inside it.
            events.values("?standing"),
            v::CLOSED_REASON,
        ))
    )
}

/// How a close of this reason names its target: `duplicate of`, `superseded by`; `None` for a
/// reason that has no target.
pub(super) fn target_words(reason: &str) -> Option<&'static str> {
    match reason {
        "duplicate" => Some("duplicate of"),
        "superseded" => Some("superseded by"),
        _ => None,
    }
}

/// The items a close of `reason` has as its targets; empty for a reason that has none.
fn targets<'a>(reason: &str, evidence: &'a [Evidence]) -> Vec<&'a Evidence> {
    if target_words(reason).is_none() {
        return Vec::new();
    }
    evidence.iter().filter(|one| one.item.is_some()).collect()
}

/// The start of a close's answer: `closed #1 (done)` with no evidence, as it always was;
/// otherwise the verdict in words and the count: `closed #1 done (2 evidence)`,
/// `closed #2 duplicate of #1`, `closed #2 duplicate of #1 (1 more evidence)`.
pub(super) fn receipt(item: &Item, reason: &str, evidence: &[Evidence]) -> String {
    if evidence.is_empty() {
        return format!("closed {} ({reason})", item.short());
    }
    let targets = targets(reason, evidence);
    let rest = evidence.len() - targets.len();
    let verdict = match target_words(reason) {
        Some(words) if !targets.is_empty() => {
            let names: Vec<String> = targets
                .iter()
                .filter_map(|one| one.item.as_ref())
                .map(|(ledger, number)| ledger.number(*number))
                .collect();
            format!("{words} {}", names.join(", "))
        }
        _ => reason.to_string(),
    };
    let count = match (targets.is_empty(), rest) {
        (true, _) => format!(" ({rest} evidence)"),
        (false, 0) => String::new(),
        (false, _) => format!(" ({rest} more evidence)"),
    };
    format!("closed {} {verdict}{count}", item.short())
}

/// What the JSON answer of a close adds: the IRIs stored, and the targets as items.
pub(super) fn answer(answer: &mut json::Answer, reason: &str, evidence: &[Evidence]) {
    answer.evidence = evidence.iter().map(|one| one.iri.clone()).collect();
    answer.targets = targets(reason, evidence)
        .iter()
        .filter_map(|one| {
            one.item
                .as_ref()
                .map(|(ledger, number)| json::item_ref(ledger, *number, &one.iri))
        })
        .collect();
}
