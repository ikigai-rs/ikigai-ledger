//! **The fair claim queue** (ledger #4455; the design is `docs/design/fair-claim-queue.md`,
//! ledger #4433's spike): waiters as a resource, and offers on release.
//!
//! ```text
//! urn:iki:ledger:{ledger}:wait:item:{id}   Source Sink Delete   the waiters in order / join or renew / leave
//! urn:iki:ledger:{ledger}:offers           Sink                 the dispatcher's sweep: lapsed places out, ended offers on
//! ```
//!
//! # What a queue is, in the graph
//!
//! Each waiter is a node `{item}:wait:{seq}`, a `ledger:WaitPlace`, with `ledger:waitsOn`,
//! `ledger:waiter`, `ledger:waiterPrincipal` (from the capability, never an argument),
//! `ledger:askedAt` (the kernel clock), `ledger:waitSeq` and `ledger:waitExpires`, and
//! `ledger:waitPoll` when the waiter said how often it looks. The item keeps the last sequence
//! it issued as its own `ledger:waitSeq`, which only counts up, like the claim epoch.
//!
//! ★ **A place grants no authority.** It only decides who is OFFERED the item when its claim is
//! released. So a place that stops being renewed is simply skipped once its `waitExpires` has
//! passed, by every read and every guard here, unlike an expired lease, which still holds its
//! item. The offers sweep deletes lapsed places in bounded batches.
//!
//! # The offer
//!
//! A release writes `ledger:offeredTo` (the head's place), `ledger:offerExpires` and
//! `ledger:offerAfterEpoch` (the epoch it released) in the SAME update, whenever a live waiter
//! exists ([`settle_ops`], which the release appends to its own batch). While the offer
//! **stands** (not expired, the item unclaimed, and its epoch still the item's), a claim by
//! anyone but that waiter is refused, as one more clause of the claim's own guarded update
//! ([`claim_clause`]). An empty queue writes nothing, so a free item nobody waits for is claimed
//! exactly as before.
//!
//! An offer that ends unclaimed passes on: its waiter leaves the queue (it may join again, at
//! the end) and the next live waiter is offered the item. That needs a write when nobody is
//! acting, which comes from the next claim, join or leave on the item ([`settle`]), or from the
//! dispatcher's `sink …:offers`.
//!
//! The offer's bound is the waiter's `poll=` plus [`MARGIN_MS`], at least [`FLOOR_MS`] and at
//! most [`CEILING_MS`]: a waiter that only polls still sees its offer before it ends.

use super::*;

/// The faces of the queue's read and of a join's answer.
const WAIT_FACES: [&str; 2] = [PLAIN, JSON];

/// What an offer adds to the waiter's poll.
pub(crate) const MARGIN_MS: u64 = 30_000;
/// The shortest offer.
pub(crate) const FLOOR_MS: u64 = 60_000;
/// The longest offer.
pub(crate) const CEILING_MS: u64 = 300_000;
/// A place's life without a renewal, when `ttl=` is not given.
const DEFAULT_TTL_MS: u64 = 180_000;
/// The longest `ttl=` and `poll=`: a place is for a waiter that is still there.
const MAX_WAIT_MS: u64 = 3_600_000;
/// How many lapsed places, and how many items, one sweep handles.
const SWEEP_BATCH: usize = 128;

/// How long an offer to a waiter that polls every `poll` lasts.
///
/// ```text
/// None → 60 s; 120 s → 150 s; 10 min → 5 min
/// ```
pub(crate) fn offer_bound(poll: Option<u64>) -> u64 {
    (poll.unwrap_or(0) + MARGIN_MS).clamp(FLOOR_MS, CEILING_MS)
}

/// Every property a place carries: what leaving, lapsing and claiming remove.
const PLACE_FIELDS: [&str; 8] = [
    v::ext::TYPE,
    v::WAITS_ON,
    v::WAITER,
    v::WAITER_PRINCIPAL,
    v::ASKED_AT,
    v::WAIT_SEQ,
    v::WAIT_EXPIRES,
    v::WAIT_POLL,
];

/// The three properties an offer writes on its item.
const OFFER_FIELDS: [&str; 3] = [v::OFFERED_TO, v::OFFER_EXPIRES, v::OFFER_AFTER_EPOCH];

/// A `DELETE` template and the `OPTIONAL`s that bind it, removing every quad of the place bound
/// to `?{var}`. ⚠ Every pattern names its predicate, never `?{var} ?p ?o`: a pattern with no
/// constant would make the store's scoped door copy the whole graph (ledger #2922).
fn place_removal(var: &str) -> (String, String) {
    let delete = PLACE_FIELDS
        .iter()
        .enumerate()
        .map(|(n, p)| format!("?{var} <{p}> ?{var}F{n} . "))
        .collect();
    let optional = PLACE_FIELDS
        .iter()
        .enumerate()
        .map(|(n, p)| format!("OPTIONAL {{ ?{var} <{p}> ?{var}F{n} }}\n"))
        .collect();
    (delete, optional)
}

/// The pattern "the place `?{var}` is this waiter's": its label, and exactly its principal.
fn mine(var: &str, holder: &str, principal: Option<&str>) -> String {
    let label = format!("?{var} <{}> {} .", v::WAITER, literal(holder));
    match principal {
        Some(term) => format!("{label} ?{var} <{}> {term} .", v::WAITER_PRINCIPAL),
        None => format!(
            "{label} FILTER NOT EXISTS {{ ?{var} <{}> ?{var}AnyPrincipal }}",
            v::WAITER_PRINCIPAL
        ),
    }
}

/// The pattern "an offer stands on `item` at `now`", binding the place it is to as `?{var}`.
/// ★ The place must be live: an offer to a place that lapsed is no offer, whether or not a
/// sweep has removed the place yet (the hub review of PR 157).
fn standing(item: &str, now: u64, var: &str) -> String {
    format!(
        "<{item}> <{to}> ?{var} ; <{ends}> ?{var}Ends ; <{after}> ?{var}After .\n\
         ?{var} <{on}> <{item}> ; <{live}> ?{var}Live .\n\
         FILTER(?{var}Live > {now})\n\
         OPTIONAL {{ <{item}> <{epoch}> ?{var}Epoch }}\n\
         FILTER(?{var}Ends > {now} && COALESCE(?{var}Epoch, 0) = ?{var}After)\n\
         FILTER NOT EXISTS {{ <{item}> <{by}> ?{var}Held }}",
        to = v::OFFERED_TO,
        ends = v::OFFER_EXPIRES,
        after = v::OFFER_AFTER_EPOCH,
        on = v::WAITS_ON,
        live = v::WAIT_EXPIRES,
        epoch = v::CLAIM_EPOCH,
        by = v::CLAIMED_BY,
        now = datetime(now),
    )
}

/// ★ **The claim guard's one clause**: no offer stands on `item` to another waiter than
/// `holder` (as `principal`, a SPARQL term). Carried by every operation of the claim's update,
/// so a claim racing a release that makes an offer is decided by the store, not by a read.
pub(crate) fn claim_clause(item: &str, holder: &str, principal: Option<&str>, now: u64) -> String {
    format!(
        "FILTER NOT EXISTS {{ {} FILTER NOT EXISTS {{ {} }} }}\n",
        standing(item, now, "offerTo"),
        mine("offerTo", holder, principal)
    )
}

/// What a claim that landed does to the queue, in the claim's own update: the offer is spent
/// (or void, if it was another's), and the claimant's own place goes, since it has the item.
/// Each operation matches only once `holder` holds the claim taken at `now`.
pub(crate) fn after_claim_ops(
    client: &StoreClient<'_, '_>,
    item: &str,
    holder: &str,
    principal: Option<&str>,
    now: u64,
) -> Vec<String> {
    let landed = format!(
        "<{item}> <{}> {} ; <{}> {} .",
        v::CLAIMED_BY,
        literal(holder),
        v::CLAIMED_AT,
        datetime(now)
    );
    let offer_delete: String = OFFER_FIELDS
        .iter()
        .enumerate()
        .map(|(n, p)| format!("<{item}> <{p}> ?spent{n} . "))
        .collect();
    let offer_optional: String = OFFER_FIELDS
        .iter()
        .enumerate()
        .map(|(n, p)| format!("OPTIONAL {{ <{item}> <{p}> ?spent{n} }}\n"))
        .collect();
    let (delete, optional) = place_removal("own");
    vec![
        format!(
            "DELETE {{ {} }} WHERE {{ {} }}",
            client.in_graph(&offer_delete),
            client.in_graph(&format!("{landed}\n{offer_optional}"))
        ),
        format!(
            "DELETE {{ {} }} WHERE {{ {} }}",
            client.in_graph(&delete),
            client.in_graph(&format!(
                "{landed}\n?own <{}> <{item}> .\n{}\n{optional}",
                v::WAITS_ON,
                mine("own", holder, principal)
            ))
        ),
    ]
}

/// ★ **Settle the offer on `item`**, as operations for one update: pass on an offer that ended
/// unclaimed (its waiter leaves the queue), clear one that no longer stands, and offer a free
/// open item nobody is offered to its live head. A release appends these to its own batch, so
/// the offer is written in the release's update; on a held item, or one whose offer stands,
/// they match nothing.
///
/// `queue` is the item's queue as read just before: each live place's offer end is computed
/// here, from its poll ([`offer_bound`]), and handed to the update as a `VALUES` table. ⚠ Not as
/// `dateTime + duration` in the update: through the store's scoped door that sum came out
/// unbound in this update (the insert landed without `ledger:offerExpires`), while the same
/// `WHERE` answered through the broad door. A place that joined after the read is not in the
/// table; when it is the head, no offer is made, and the next claim, join, leave or sweep
/// settles the item.
pub(crate) fn settle_ops(
    client: &StoreClient<'_, '_>,
    item: &str,
    now: u64,
    queue: &Queue,
) -> Vec<String> {
    let at = datetime(now);
    let free = format!(
        "FILTER NOT EXISTS {{ <{item}> <{}> ?settleHeld }}",
        v::CLAIMED_BY
    );
    // 1. The head let its offer pass: the offer ended, at the epoch it was made for, on an item
    //    still free. Its waiter leaves the queue.
    let (gone_delete, gone_optional) = place_removal("gone");
    let passed = format!(
        "DELETE {{ {} }} WHERE {{ {} }}",
        client.in_graph(&gone_delete),
        client.in_graph(&format!(
            "<{item}> <{to}> ?gone ; <{ends}> ?goneEnds ; <{after}> ?goneAfter .\n\
             ?gone <{on}> <{item}> .\n\
             OPTIONAL {{ <{item}> <{epoch}> ?goneEpoch }}\n\
             FILTER(?goneEnds <= {at} && COALESCE(?goneEpoch, 0) = ?goneAfter)\n\
             {free}\n{gone_optional}",
            to = v::OFFERED_TO,
            ends = v::OFFER_EXPIRES,
            after = v::OFFER_AFTER_EPOCH,
            on = v::WAITS_ON,
            epoch = v::CLAIM_EPOCH,
        ))
    );
    // 2. Whatever offer is left and does not stand is cleared.
    let offer_delete: String = OFFER_FIELDS
        .iter()
        .enumerate()
        .map(|(n, p)| format!("<{item}> <{p}> ?old{n} . "))
        .collect();
    let offer_optional: String = OFFER_FIELDS
        .iter()
        .enumerate()
        .map(|(n, p)| format!("OPTIONAL {{ <{item}> <{p}> ?old{n} }}\n"))
        .collect();
    let cleared = format!(
        "DELETE {{ {} }} WHERE {{ {} }}",
        client.in_graph(&offer_delete),
        client.in_graph(&format!(
            "<{item}> <{}> ?anyOffer .\n{offer_optional}FILTER NOT EXISTS {{ {} }}",
            v::OFFERED_TO,
            standing(item, now, "keep")
        ))
    );
    // 3. A free open item with no offer is offered to its head: the live place with the lowest
    //    sequence. ⚠ The aggregate is filtered inside its own group: `MIN` over no rows is one
    //    row with the variable unbound, which would join with every place.
    let ends: Vec<String> = queue
        .places
        .iter()
        .filter_map(|p| {
            let place = sparql::iri(&p.iri, "place").ok()?;
            let poll = p
                .poll
                .as_deref()
                .and_then(|text| crate::claim::parse_lease(text).ok())
                .map(|l| l.millis);
            Some(format!("({place} {})", datetime(now + offer_bound(poll))))
        })
        .collect();
    if ends.is_empty() {
        return vec![passed, cleared];
    }
    let offered = format!(
        "INSERT {{ {} }} WHERE {{ {} }}",
        client.in_graph(&format!(
            "<{item}> <{}> ?head ; <{}> ?headEnds ; <{}> ?headAfter .",
            v::OFFERED_TO,
            v::OFFER_EXPIRES,
            v::OFFER_AFTER_EPOCH
        )),
        client.in_graph(&format!(
            "<{item}> <{type_}> <{class}> ; <{status}> <{open}> .\n\
             {free}\n\
             FILTER NOT EXISTS {{ <{item}> <{to}> ?anyTo }}\n\
             {{ SELECT ?headSeq WHERE {{ {{ SELECT (MIN(?liveSeq) AS ?headSeq) WHERE {{ \
                ?live <{on}> <{item}> ; <{seq}> ?liveSeq ; <{expires}> ?liveEnds . \
                FILTER(?liveEnds > {at}) }} }} FILTER(BOUND(?headSeq)) }} }}\n\
             ?head <{on}> <{item}> ; <{seq}> ?headSeq .\n\
             VALUES (?head ?headEnds) {{ {ends} }}\n\
             OPTIONAL {{ <{item}> <{epoch}> ?headEpoch }}\n\
             BIND(COALESCE(?headEpoch, 0) AS ?headAfter)",
            type_ = v::ext::TYPE,
            class = v::ITEM_CLASS,
            status = v::STATUS,
            open = v::OPEN,
            to = v::OFFERED_TO,
            on = v::WAITS_ON,
            seq = v::WAIT_SEQ,
            expires = v::WAIT_EXPIRES,
            epoch = v::CLAIM_EPOCH,
            ends = ends.join(" "),
        ))
    );
    vec![passed, cleared, offered]
}

/// The record of a dispatcher's claim that passed a standing offer to `place`: the claim's own
/// event names the place as `ledger:overrodeOffer`, in the claim's update, matching only once
/// `holder` holds the claim taken at `now` and while the offer is still recorded. The event is
/// the claim event of this item, by this holder, written at `now`.
pub(crate) fn override_record(
    client: &StoreClient<'_, '_>,
    item: &str,
    place: &str,
    holder: &str,
    now: u64,
) -> Result<String> {
    let place = sparql::iri(place, "place")?;
    let holder = literal(holder);
    let at = datetime(now);
    Ok(format!(
        "INSERT {{ {} }} WHERE {{ {} }}",
        client.in_graph(&format!("?event <{}> {place} .", v::OVERRODE_OFFER)),
        client.in_graph(&format!(
            "<{item}> <{by}> {holder} ; <{claimed}> {at} ; <{to}> {place} .\n\
             ?event <{on}> <{item}> ; <{type_}> <{class}> ; <{by}> {holder} ; <{created}> {at} .\n\
             FILTER NOT EXISTS {{ ?event <{overrode}> ?anyPlace }}",
            by = v::CLAIMED_BY,
            claimed = v::CLAIMED_AT,
            to = v::OFFERED_TO,
            on = v::ON_ITEM,
            type_ = v::ext::TYPE,
            class = v::CLAIM_EVENT_CLASS,
            created = v::ext::CREATED,
            overrode = v::OVERRODE_OFFER,
        ))
    ))
}

/// What a close does to the queue, in the close's own update and under its `guard`: the offer
/// and every place on the item go, since closed work is waited for by nobody (the hub review
/// of PR 157: 300 closed items had left 302 `offeredTo` quads for every sweep to re-read).
/// The item keeps its last `waitSeq`, which only counts up.
///
/// `places` are the item's places, live or lapsed, as [`place_iris`] read them, listed in a
/// `VALUES` block: ⚠ an unlisted `?place <p> ?o` pattern would make the store's scoped door
/// copy every quad of `p` in the graph (ledger #2922). ⚠ A join CAN add a place after that read
/// and before the close's update lands, since the item is open until then: the close calls
/// [`sweep_closed`] once its update has landed, to take any such straggler.
pub(crate) fn close_ops(
    client: &StoreClient<'_, '_>,
    item: &str,
    places: &[String],
    guard: &str,
) -> Result<Vec<String>> {
    let offer_delete: String = OFFER_FIELDS
        .iter()
        .enumerate()
        .map(|(n, p)| format!("<{item}> <{p}> ?closedOffer{n} . "))
        .collect();
    let offer_optional: String = OFFER_FIELDS
        .iter()
        .enumerate()
        .map(|(n, p)| format!("OPTIONAL {{ <{item}> <{p}> ?closedOffer{n} }}\n"))
        .collect();
    let mut ops = vec![format!(
        "DELETE {{ {} }} WHERE {{ {} }}",
        client.in_graph(&offer_delete),
        client.in_graph(&format!(
            "<{item}> <{}> ?closedTo .\n{offer_optional}{guard}",
            v::OFFERED_TO
        ))
    )];
    if !places.is_empty() {
        let listed = places
            .iter()
            .map(|p| sparql::iri(p, "place"))
            .collect::<Result<Vec<_>>>()?
            .join(" ");
        let (delete, optional) = place_removal("closedPlace");
        ops.push(format!(
            "DELETE {{ {} }} WHERE {{ {} }}",
            client.in_graph(&delete),
            client.in_graph(&format!(
                "VALUES ?closedPlace {{ {listed} }} ?closedPlace <{}> <{item}> .\n{optional}{guard}",
                v::WAITS_ON
            ))
        ));
    }
    Ok(ops)
}

/// After a close has landed: the offer and every place still on `item`, read again and taken
/// in one update, which holds only while the item is closed. A join that lands after the close
/// is refused by its own open check, so no place can be added after this read.
pub(crate) async fn sweep_closed(client: &StoreClient<'_, '_>, item: &str) -> Result<()> {
    let places = place_iris(client, item).await?;
    let offered = client
        .ask(&format!(
            "ASK {{ {} }}",
            client.in_graph(&format!("<{item}> <{}> ?to", v::OFFERED_TO))
        ))
        .await?;
    if places.is_empty() && !offered {
        return Ok(());
    }
    let closed = format!("<{item}> <{}> <{}> .", v::STATUS, v::CLOSED);
    client
        .update(&batch(&close_ops(client, item, &places, &closed)?))
        .await
}

/// Every place on `item`, live or lapsed, by IRI.
pub(crate) async fn place_iris(client: &StoreClient<'_, '_>, item: &str) -> Result<Vec<String>> {
    let rows = client
        .select(&format!(
            "SELECT ?place WHERE {{ {} }}",
            client.in_graph(&format!("?place <{}> <{item}> .", v::WAITS_ON))
        ))
        .await?;
    Ok(rows
        .iter()
        .filter_map(|row| row.get("place").map(|b| b.value.clone()))
        .collect())
}

/// [`settle_ops`] as one update of its own, over the queue as it reads now.
pub(crate) async fn settle(client: &StoreClient<'_, '_>, item: &Item, now: u64) -> Result<()> {
    let queue = read(client, item, now).await?;
    client
        .update(&batch(&settle_ops(client, &item.iri, now, &queue)))
        .await
}

/// One live place, as read.
#[derive(Debug, Clone)]
pub(crate) struct Place {
    pub(crate) iri: String,
    pub(crate) seq: i64,
    pub(crate) waiter: String,
    pub(crate) principal: Option<String>,
    pub(crate) asked_at: Option<u64>,
    pub(crate) expires: u64,
    pub(crate) poll: Option<String>,
}

impl Place {
    /// The waiter as the plain faces name it.
    pub(crate) fn display(&self) -> String {
        match &self.principal {
            Some(p) => format!("{} (as {p})", self.waiter),
            None => self.waiter.clone(),
        }
    }

    fn is(&self, holder: &str, principal: Option<&str>) -> bool {
        self.waiter == holder && self.principal.as_deref() == principal
    }
}

/// The offer that stands on an item, as read.
#[derive(Debug, Clone)]
pub(crate) struct Offer {
    pub(crate) place: Place,
    pub(crate) ends: u64,
    pub(crate) after_epoch: u64,
}

/// An item's queue at one instant: its live places in order, the offer that stands, and
/// whether anything is left that [`settle`] would change.
#[derive(Debug, Clone)]
pub(crate) struct Queue {
    pub(crate) places: Vec<Place>,
    pub(crate) offer: Option<Offer>,
    /// An offer is recorded that does not stand (ended, void, or to a place that is gone or
    /// lapsed).
    stale_offer: bool,
    /// The place whose offer ended unclaimed at the item's current epoch: it leaves the queue
    /// at the next settle (step 1 of [`settle_ops`]), so it is no longer the head.
    ended_to: Option<String>,
}

impl Queue {
    /// Whether a settle would write: an offer that does not stand, or a free item whose live
    /// waiters nobody has offered it to.
    pub(crate) fn unsettled(&self, item: &Item) -> bool {
        item.open
            && item.claimed_by.is_none()
            && (self.stale_offer || (self.offer.is_none() && !self.places.is_empty()))
    }

    /// ★ **The effective offer at `now`**: the one rule the claim guard, `next` and the queue's
    /// read share (the hub review of PR 157). It is what a [`settle`] at `now` would leave: the
    /// offer that stands; else, on a free open item, an offer to the first live waiter that is
    /// not the one whose offer just ended, until `now` plus its [`offer_bound`]; else none, and
    /// the item is free for anyone. A claim settles before its guard, so the claim refuses
    /// exactly whom this names.
    pub(crate) fn effective(&self, item: &Item, now: u64) -> Option<Offer> {
        if !item.open || item.claimed_by.is_some() {
            return None;
        }
        if let Some(offer) = &self.offer {
            return Some(offer.clone());
        }
        let head = self
            .places
            .iter()
            .find(|p| self.ended_to.as_deref() != Some(p.iri.as_str()))?;
        let poll = head
            .poll
            .as_deref()
            .and_then(|text| crate::claim::parse_lease(text).ok())
            .map(|l| l.millis);
        Some(Offer {
            place: head.clone(),
            ends: now + offer_bound(poll),
            after_epoch: item.claim_epoch.unwrap_or(0),
        })
    }

    /// The queue as a [`settle`] at `now` would leave it, without writing: the waiter whose
    /// offer ended is gone, and the offer is [`Queue::effective`]. What the reads show.
    pub(crate) fn settled_view(mut self, item: &Item, now: u64) -> Queue {
        self.offer = self.effective(item, now);
        if item.open && item.claimed_by.is_none() {
            if let Some(gone) = self.ended_to.take() {
                self.places.retain(|p| p.iri != gone);
            }
        }
        self.stale_offer = false;
        self
    }

    /// The effective offer, when it is to anyone but `holder` as `principal`.
    pub(crate) fn offered_to_another(
        &self,
        item: &Item,
        now: u64,
        holder: &str,
        principal: Option<&str>,
    ) -> Option<Offer> {
        self.effective(item, now)
            .filter(|offer| !offer.place.is(holder, principal))
    }

    fn position(&self, holder: &str, principal: Option<&str>) -> Option<usize> {
        self.places.iter().position(|p| p.is(holder, principal))
    }
}

/// Read `item`'s queue at `now`. A place whose `waitExpires` has passed is not in it.
pub(crate) async fn read(client: &StoreClient<'_, '_>, item: &Item, now: u64) -> Result<Queue> {
    let rows = client
        .select(&format!(
            "SELECT ?place ?seq ?waiter ?principal ?asked ?expires ?poll WHERE {{ {} }} ORDER BY ?seq",
            client.in_graph(&format!(
                "?place <{on}> <{item}> ; <{seq}> ?seq ; <{waiter}> ?waiter ; <{expires}> ?expires .\n\
                 OPTIONAL {{ ?place <{principal}> ?principal }}\n\
                 OPTIONAL {{ ?place <{asked}> ?asked }}\n\
                 OPTIONAL {{ ?place <{poll}> ?poll }}",
                on = v::WAITS_ON,
                item = item.iri,
                seq = v::WAIT_SEQ,
                waiter = v::WAITER,
                expires = v::WAIT_EXPIRES,
                principal = v::WAITER_PRINCIPAL,
                asked = v::ASKED_AT,
                poll = v::WAIT_POLL,
            ))
        ))
        .await?;
    let mut every = Vec::new();
    for row in &rows {
        let (Some(iri), Some(seq), Some(waiter), Some(expires)) = (
            row.get("place"),
            row.get("seq").and_then(|b| b.as_i64()),
            row.get("waiter"),
            row.get("expires").and_then(|b| model::millis(&b.value)),
        ) else {
            continue;
        };
        every.push(Place {
            iri: iri.value.clone(),
            seq,
            waiter: waiter.value.clone(),
            principal: row.get("principal").map(|b| b.value.clone()),
            asked_at: row.get("asked").and_then(|b| model::millis(&b.value)),
            expires,
            poll: row
                .get("poll")
                .map(|b| b.value.clone())
                .filter(|p| p != "PT0S"),
        });
    }
    let offers = client
        .select(&format!(
            "SELECT ?to ?ends ?after WHERE {{ {} }}",
            client.in_graph(&format!(
                "<{item}> <{}> ?to . OPTIONAL {{ <{item}> <{}> ?ends }} \
                 OPTIONAL {{ <{item}> <{}> ?after }}",
                v::OFFERED_TO,
                v::OFFER_EXPIRES,
                v::OFFER_AFTER_EPOCH,
                item = item.iri,
            ))
        ))
        .await?;
    let recorded = !offers.is_empty();
    let epoch = item.claim_epoch.unwrap_or(0);
    let free = item.open && item.claimed_by.is_none();
    let ended_to = offers.iter().find_map(|row| {
        let to = &row.get("to")?.value;
        let ends = model::millis(&row.get("ends")?.value)?;
        let after = u64::try_from(row.get("after")?.as_i64()?).ok()?;
        (free && ends <= now && after == epoch).then(|| to.clone())
    });
    let offer = offers.iter().find_map(|row| {
        let to = &row.get("to")?.value;
        let ends = model::millis(&row.get("ends")?.value)?;
        let after = u64::try_from(row.get("after")?.as_i64()?).ok()?;
        // ★ Only a LIVE place: an offer to a lapsed one does not stand.
        let place = every
            .iter()
            .find(|p| &p.iri == to && p.expires > now)?
            .clone();
        let stands = ends > now
            && after == item.claim_epoch.unwrap_or(0)
            && item.claimed_by.is_none()
            && offers.len() == 1;
        stands.then_some(Offer {
            place,
            ends,
            after_epoch: after,
        })
    });
    let places = every.into_iter().filter(|p| p.expires > now).collect();
    Ok(Queue {
        places,
        stale_offer: recorded && offer.is_none(),
        offer,
        ended_to,
    })
}

/// The refusal of a claim while `offer` stands to another waiter: who it is offered to, until
/// when, and how to be offered it in turn.
pub(crate) fn offered_refusal(item: &Item, ledger: &Ledger, offer: &Offer) -> Error {
    Error::Conflict(format!(
        "{} is offered to {} until {}: it was released while waiters were queued, and the \
         first in the queue may claim it until then. Join the queue \
         (`sink {} content=<holder>`) to be offered it in turn",
        item.short(),
        offer.place.display(),
        sparql::iso8601(offer.ends),
        ledger.resource(&format!("wait:item:{}", item.number)),
    ))
}

/// The holder label a join or a claim names, checked as a claim checks it.
fn holder_of(inv: &Invocation<'_>) -> Result<String> {
    let holder = inv.inline_str("content")?.trim().to_string();
    if holder.is_empty() {
        return Err(Error::InvalidArgument {
            name: "content".to_string(),
            detail: "a waiter needs a holder: the name it will claim the item under".to_string(),
        });
    }
    if holder == "none" || holder == "any" {
        return Err(Error::InvalidArgument {
            name: "content".to_string(),
            detail: format!(
                "`{holder}` is reserved: `holder=none` and `holder=any` are the items filter's \
                 keywords, so work claimed as `{holder}` could never be listed by holder"
            ),
        });
    }
    Ok(holder)
}

/// `ttl=` or `poll=`: a length from 1 second to an hour, in a lease's spelling.
fn parse_wait(inv: &Invocation<'_>, name: &str) -> Result<Option<crate::claim::Lease>> {
    let Some(text) = inv.optional_str(name)? else {
        return Ok(None);
    };
    let bad = |detail: String| Error::InvalidArgument {
        name: name.to_string(),
        detail,
    };
    let length = crate::claim::parse_lease(text).map_err(|_| {
        bad(format!(
            "`{}` is not a length: `90s`, `3m`, `1h` or `PT3M`",
            text.trim()
        ))
    })?;
    if length.millis > MAX_WAIT_MS {
        return Err(bad(format!(
            "`{}` is longer than an hour, the longest this takes: a place is for a waiter that \
             is still there, and it renews by joining again",
            text.trim()
        )));
    }
    Ok(Some(length))
}

/// `urn:iki:ledger:{ledger}:wait:item:{id}`.
#[derive(Clone)]
pub(super) struct WaitEndpoint;

#[async_trait]
impl Endpoint for WaitEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        match inv.request.verb {
            Verb::Source => {
                let client = StoreClient::new(inv, ledger_for(inv, Need::Read)?);
                self.list(inv, &client).await
            }
            Verb::Sink => {
                let client = StoreClient::new(inv, ledger_for(inv, Need::Claim)?);
                self.join(inv, &client).await
            }
            Verb::Delete => {
                let client = StoreClient::new(inv, ledger_for(inv, Need::Claim)?);
                self.leave(inv, &client).await
            }
            other => Err(unsupported("ledger-wait", other)),
        }
    }

    fn name(&self) -> &str {
        "ledger-wait"
    }

    fn describe(&self) -> Description {
        Description::new("ledger-wait")
            .title("Wait in line for a ledger item")
            .summary(
                "An item's claim queue: who waits for it, in the order they asked. When the \
                 item's claim is released, the release offers it to the first live waiter, \
                 and only that waiter may claim it until the offer ends; one that does not \
                 claim in time leaves the queue and the offer passes on. A queue makes \
                 waiting fair and quiet, not short. A place grants no authority, and one that \
                 is not renewed lapses.",
            )
            .verb(Verb::Meta)
            .action(read_action(
                ActionSpec::new(Verb::Source)
                    .input(wait_arg())
                    .input(id_arg())
                    .summary(
                        "The live waiters in order (place, waiter and principal, when each \
                         asked, when each place lapses) and the standing offer, if any. Needs \
                         `urn:cap:ledger:read:{ledger}`.",
                    )
                    .input(as_arg(&WAIT_FACES))
                    .output(PLAIN)
                    .output(JSON),
            ))
            .action(write_scopes(
                ActionSpec::new(Verb::Sink)
                    .input(wait_arg())
                    .input(id_arg())
                    .summary(
                        "Join the queue, or renew this waiter's place (the same label and the \
                         same principal, as for a claim), which keeps its order. Needs \
                         `urn:cap:ledger:claim:{ledger}` (or `write:{ledger}`), because \
                         joining is asking for a claim. A closed item, or one this waiter \
                         already holds, is refused (Conflict). On a FREE item nobody else is \
                         offered, the join claims it at once, through the claim resource, and \
                         answers as the claim does.",
                    )
                    .input(
                        ArgSpec::new("content")
                            .summary(
                                "The holder the waiter will claim under, as `claim content=`. \
                                 Not `none` or `any`.",
                            )
                            .class(v::ext::XSD_STRING),
                    )
                    .input(
                        ArgSpec::new("ttl")
                            .summary(
                                "How long the place holds without a renewal: `90s`, `3m`, \
                                 `PT3M`, at most an hour. Default 3 minutes, from the kernel \
                                 clock. Renew by joining again.",
                            )
                            .class(v::ext::XSD_STRING)
                            .default_value("3m")
                            .optional(),
                    )
                    .input(
                        ArgSpec::new("poll")
                            .summary(
                                "How often this waiter looks, at most an hour. An offer to it \
                                 lasts this plus 30 seconds, at least 60 seconds and at most 5 \
                                 minutes. Omit it for a waiter that is woken: its offer lasts \
                                 60 seconds.",
                            )
                            .class(v::ext::XSD_STRING)
                            .optional(),
                    )
                    .input(
                        ArgSpec::new("lease")
                            .summary(
                                "The lease of the claim, when the join claims the item at once: \
                                 as `claim lease=`.",
                            )
                            .class(v::ext::XSD_STRING)
                            .optional(),
                    )
                    .input(
                        ArgSpec::new("purpose")
                            .summary(
                                "The purpose of the claim, when the join claims the item at \
                                 once: as `claim purpose=`.",
                            )
                            .class(v::ext::XSD_STRING)
                            .optional(),
                    )
                    .input(as_arg(&WAIT_FACES))
                    .output(PLAIN)
                    .output(JSON),
                CAP_ANY,
            ))
            .action(write_scopes(
                ActionSpec::new(Verb::Delete)
                    .input(wait_arg())
                    .input(id_arg())
                    .summary(
                        "Leave the queue. Needs `urn:cap:ledger:claim:{ledger}` (or \
                         `write:{ledger}`). The rules of a release apply: a place taken as a \
                         principal is left by a capability acting as that principal, or by a \
                         dispatcher holding `close:{ledger}` or `write:{ledger}`; Denied \
                         otherwise. A waiter that was offered the item passes the offer on. Leaving a \
                         queue the waiter is not in changes nothing, and says so.",
                    )
                    .input(
                        ArgSpec::new("from")
                            .summary("The waiter that leaves, by its holder label.")
                            .class(v::ext::XSD_STRING),
                    )
                    .output(PLAIN),
                CAP_ANY,
            ))
    }
}

/// The `ledger` ArgSpec of `wait:item:{id}`.
fn wait_arg() -> ArgSpec {
    ledger_arg("wait:item:{id}")
}

/// The `id` the IRI carries.
fn id_arg() -> ArgSpec {
    ArgSpec::new("id")
        .summary("The item, by its number, its id, or `key:{key}`, as for `item:{id}`.")
        .class(v::ext::XSD_STRING)
        .binding()
}

impl WaitEndpoint {
    async fn list(
        &self,
        inv: &Invocation<'_>,
        client: &StoreClient<'_, '_>,
    ) -> Result<Representation> {
        let want = wanted_face(inv, &WAIT_FACES)?;
        let now = now_ms(inv)?;
        let item = require_item(client, flight::part_id(inv)?).await?;
        // As the claim guard decides it: the one rule for an ended offer.
        let queue = read(client, &item, now).await?.settled_view(&item, now);
        let ledger = client.ledger();
        let repr = if want == JSON {
            json_repr(json::render(&json::WaitQueue {
                schema: json::SCHEMA,
                ledger: ledger.name().to_string(),
                item: json::item_ref(ledger, item.number, &item.iri),
                checked_at: sparql::iso8601(now),
                waiters: queue
                    .places
                    .iter()
                    .enumerate()
                    .map(|(n, p)| waiter_json(n + 1, p, queue.offer.as_ref()))
                    .collect(),
                offer: queue.offer.as_ref().map(|o| json::WaitOffer {
                    waiter: o.place.waiter.clone(),
                    principal: o.place.principal.clone(),
                    place: o.place.iri.clone(),
                    expires: sparql::iso8601(o.ends),
                    after_epoch: o.after_epoch,
                }),
            })?)
        } else {
            plain(queue_text(&item, &queue))
        };
        // The answer changes with nothing written when a place lapses or the offer ends.
        let deadline = queue
            .places
            .iter()
            .map(|p| p.expires)
            .chain(queue.offer.as_ref().map(|o| o.ends))
            .filter(|t| *t > now)
            .min();
        let repr = match deadline {
            None => repr.cacheable(),
            Some(deadline) => repr.cacheable_until(ikigai_core::Time::from_millis(deadline)),
        };
        Ok(repr
            .depends_on(ikigai_store::UPDATE_THREAD)
            .depends_on(ikigai_store::LOAD_THREAD)
            .depends_on(ikigai_store::GRAPH_UPDATE_THREAD))
    }

    async fn join(
        &self,
        inv: &Invocation<'_>,
        client: &StoreClient<'_, '_>,
    ) -> Result<Representation> {
        let want = wanted_face(inv, &WAIT_FACES)?;
        // Every argument is checked before anything is read or written.
        let holder = holder_of(inv)?;
        let ttl = parse_wait(inv, "ttl")?.map_or(DEFAULT_TTL_MS, |l| l.millis);
        let poll = parse_wait(inv, "poll")?;
        let lease = inv.optional_str("lease")?;
        if let Some(lease) = lease {
            crate::claim::parse_lease(lease)?;
        }
        let purpose = inv.optional_str("purpose")?;
        let now = now_ms(inv)?;
        let ledger = client.ledger().clone();
        let mut item = require_item(client, flight::part_id(inv)?).await?;
        if !item.open {
            return Err(closed_refusal(
                &item,
                &ledger,
                "closed work is held by nobody, so nobody waits for it",
            ));
        }
        let caller = inv.capability.principal();
        let caller_term = caller.map(|p| sparql::iri(p, "principal")).transpose()?;
        if item.claimed_by.as_deref() == Some(holder.as_str())
            && item.claim_principal.as_deref() == caller
        {
            return Err(Error::Conflict(format!(
                "{} is already held by {}: a waiter waits for a claim it does not hold. Renew \
                 the claim (`sink {} item={} content={holder} renew=true`) instead",
                item.short(),
                holder_display(&item),
                ledger.resource("claim"),
                item.number
            )));
        }
        let mut queue = read(client, &item, now).await?;
        if queue.unsettled(&item) {
            settle(client, &item, now).await?;
            queue = read(client, &item, now).await?;
        }
        // ★ A free item nobody else is offered is claimed at once (the design's default for
        // its open question): through the claim resource itself, so the claim is exactly a
        // claim, its guard, epoch and events included, and leaves this waiter's place.
        if item.claimed_by.is_none()
            && queue
                .offered_to_another(&item, now, &holder, caller)
                .is_none()
        {
            let mut request = ikigai_core::Request::new(
                Verb::Sink,
                Iri::parse(ledger.resource("claim")).map_err(|e| Error::Endpoint(e.to_string()))?,
            )
            .with_arg(
                "item",
                ikigai_core::ArgRef::Inline(item.iri.clone().into_bytes()),
            )
            .with_arg(
                "content",
                ikigai_core::ArgRef::Inline(holder.clone().into_bytes()),
            );
            if let Some(lease) = lease {
                request = request.with_arg(
                    "lease",
                    ikigai_core::ArgRef::Inline(lease.as_bytes().to_vec()),
                );
            }
            if let Some(purpose) = purpose {
                request = request.with_arg(
                    "purpose",
                    ikigai_core::ArgRef::Inline(purpose.as_bytes().to_vec()),
                );
            }
            match inv.issue(request).await {
                Ok(answer) => {
                    let after = require_item(client, &item.iri).await?;
                    if want == JSON {
                        return Ok(json_repr(json::render(&json::WaitAnswer {
                            schema: json::SCHEMA,
                            ledger: ledger.name().to_string(),
                            item: json::item_ref(&ledger, item.number, &item.iri),
                            waiter: holder,
                            principal: caller.map(str::to_string),
                            claimed: true,
                            epoch: after.claim_epoch,
                            place: None,
                            of: None,
                            expires: None,
                            offer: None,
                        })?));
                    }
                    return Ok(plain(String::from_utf8_lossy(&answer.bytes).into_owned()));
                }
                // Another claimant got there first: wait for it like anyone else.
                Err(Error::Conflict(_)) => {
                    item = require_item(client, &item.iri).await?;
                    if !item.open {
                        return Err(closed_refusal(
                            &item,
                            &ledger,
                            "closed work is held by nobody, so nobody waits for it",
                        ));
                    }
                }
                Err(other) => return Err(other),
            }
        }
        let poll_term = poll
            .as_ref()
            .map(|p| {
                ikigai_store::sparql::typed_literal(
                    &p.duration,
                    v::ext::XSD_DAY_TIME_DURATION,
                    "poll",
                )
            })
            .transpose()?;
        // A new place always carries its poll (see `settle_ops`): `PT0S` for a woken waiter.
        let new_poll = match &poll_term {
            Some(term) => term.clone(),
            None => {
                ikigai_store::sparql::typed_literal("PT0S", v::ext::XSD_DAY_TIME_DURATION, "poll")?
            }
        };
        let ends = now + ttl;
        let mut ops = Vec::new();
        // This waiter's own lapsed places go first: a lapsed place is no place.
        let (old_delete, old_optional) = place_removal("old");
        ops.push(format!(
            "DELETE {{ {} }} WHERE {{ {} }}",
            client.in_graph(&old_delete),
            client.in_graph(&format!(
                "?old <{on}> <{item}> ; <{expires}> ?oldEnds . FILTER(?oldEnds <= {at})\n{}\n{old_optional}",
                mine("old", &holder, caller_term.as_deref()),
                on = v::WAITS_ON,
                item = item.iri,
                expires = v::WAIT_EXPIRES,
                at = datetime(now),
            ))
        ));
        // A renewal: a live place of this waiter's keeps its order and moves its expiry.
        let (poll_delete, poll_insert, poll_where) = match &poll_term {
            Some(term) => (
                format!("?mine <{}> ?minePoll .", v::WAIT_POLL),
                format!("?mine <{}> {term} .", v::WAIT_POLL),
                format!("OPTIONAL {{ ?mine <{}> ?minePoll }}", v::WAIT_POLL),
            ),
            None => (String::new(), String::new(), String::new()),
        };
        ops.push(format!(
            "DELETE {{ {} }} INSERT {{ {} }} WHERE {{ {} }}",
            client.in_graph(&format!("?mine <{}> ?mineEnds . {poll_delete}", v::WAIT_EXPIRES)),
            client.in_graph(&format!(
                "?mine <{}> {} . {poll_insert}",
                v::WAIT_EXPIRES,
                datetime(ends)
            )),
            client.in_graph(&format!(
                "?mine <{on}> <{item}> ; <{expires}> ?mineEnds . FILTER(?mineEnds > {at})\n{}\n{poll_where}",
                mine("mine", &holder, caller_term.as_deref()),
                on = v::WAITS_ON,
                item = item.iri,
                expires = v::WAIT_EXPIRES,
                at = datetime(now),
            ))
        ));
        // A new place at the end, only when there is no live one: two joins racing take one.
        let mut place = format!(
            "?new <{type_}> <{class}> ; <{on}> <{item}> ; <{waiter}> {holder} ; <{asked}> {at} ; \
             <{seq}> ?newSeq ; <{expires}> {ends} .\n<{item}> <{seq}> ?newSeq .",
            type_ = v::ext::TYPE,
            class = v::WAIT_PLACE_CLASS,
            on = v::WAITS_ON,
            item = item.iri,
            waiter = v::WAITER,
            holder = literal(&holder),
            asked = v::ASKED_AT,
            at = datetime(now),
            seq = v::WAIT_SEQ,
            expires = v::WAIT_EXPIRES,
            ends = datetime(ends),
        );
        if let Some(term) = &caller_term {
            place.push_str(&format!("\n?new <{}> {term} .", v::WAITER_PRINCIPAL));
        }
        place.push_str(&format!("\n?new <{}> {new_poll} .", v::WAIT_POLL));
        ops.push(format!(
            "DELETE {{ {} }} INSERT {{ {} }} WHERE {{ {} }}",
            client.in_graph(&format!("<{}> <{}> ?lastSeq .", item.iri, v::WAIT_SEQ)),
            client.in_graph(&place),
            client.in_graph(&format!(
                "<{item}> <{type_}> <{class}> ; <{status}> <{open}> .\n\
                 FILTER NOT EXISTS {{ ?live <{on}> <{item}> ; <{expires}> ?liveEnds . \
                 FILTER(?liveEnds > {at}) {} }}\n\
                 OPTIONAL {{ <{item}> <{seq}> ?lastSeq }}\n\
                 BIND(COALESCE(?lastSeq, 0) + 1 AS ?newSeq)\n\
                 BIND(IRI(CONCAT({prefix}, STR(?newSeq))) AS ?new)",
                mine("live", &holder, caller_term.as_deref()),
                item = item.iri,
                type_ = v::ext::TYPE,
                class = v::ITEM_CLASS,
                status = v::STATUS,
                open = v::OPEN,
                on = v::WAITS_ON,
                expires = v::WAIT_EXPIRES,
                at = datetime(now),
                seq = v::WAIT_SEQ,
                prefix = literal(&format!("{}:wait:", item.iri)),
            ))
        ));
        // And the offer, should the item have come free since it was read.
        ops.extend(settle_ops(client, &item.iri, now, &queue));
        client.update(&batch(&ops)).await?;
        let after = require_item(client, &item.iri).await?;
        if !after.open {
            return Err(closed_refusal(
                &after,
                &ledger,
                "it was closed before the place was taken, and nobody waits for closed work",
            ));
        }
        let queue = read(client, &after, now).await?;
        let Some(at) = queue.position(&holder, caller) else {
            return Err(Error::Conflict(format!(
                "{}: {holder} could not take a place in the queue, because the item changed \
                 while the place was being written. Read it and join again if that is still \
                 wanted",
                item.short()
            )));
        };
        let mine = &queue.places[at];
        if want == JSON {
            return Ok(json_repr(json::render(&json::WaitAnswer {
                schema: json::SCHEMA,
                ledger: ledger.name().to_string(),
                item: json::item_ref(&ledger, item.number, &item.iri),
                waiter: holder,
                principal: caller.map(str::to_string),
                claimed: false,
                epoch: None,
                place: Some(at + 1),
                of: Some(queue.places.len()),
                expires: Some(sparql::iso8601(mine.expires)),
                offer: queue.offer.as_ref().map(|o| json::WaitOffer {
                    waiter: o.place.waiter.clone(),
                    principal: o.place.principal.clone(),
                    place: o.place.iri.clone(),
                    expires: sparql::iso8601(o.ends),
                    after_epoch: o.after_epoch,
                }),
            })?));
        }
        let state = match (&queue.offer, &after.claimed_by) {
            (Some(offer), _) if offer.place.iri == mine.iri => format!(
                "; offered to {holder} until {}: claim it now",
                sparql::iso8601(offer.ends)
            ),
            (Some(offer), _) => format!(
                "; offered to {} until {}",
                offer.place.display(),
                sparql::iso8601(offer.ends)
            ),
            (None, Some(_)) => format!("; held by {}", holder_display(&after)),
            (None, None) => String::new(),
        };
        Ok(plain(format!(
            "{} waited for by {}: place {} of {}, until {}{state}\n",
            item.short(),
            mine.display(),
            at + 1,
            queue.places.len(),
            sparql::iso8601(mine.expires),
        )))
    }

    async fn leave(
        &self,
        inv: &Invocation<'_>,
        client: &StoreClient<'_, '_>,
    ) -> Result<Representation> {
        let from = inv.inline_str("from")?.trim().to_string();
        if from.is_empty() {
            return Err(Error::InvalidArgument {
                name: "from".to_string(),
                detail: "leaving names the waiter that leaves (`from=`), by its holder label"
                    .to_string(),
            });
        }
        let now = now_ms(inv)?;
        let ledger = client.ledger().clone();
        let item = require_item(client, flight::part_id(inv)?).await?;
        let queue = read(client, &item, now).await?;
        let caller = inv.capability.principal();
        let dispatcher = is_dispatcher(inv, &ledger);
        let named: Vec<&Place> = queue.places.iter().filter(|p| p.waiter == from).collect();
        if named.is_empty() {
            // A Delete of what is not there changes nothing, and says so.
            return Ok(plain(format!(
                "{from} is not waiting for {}, so there was no place to leave\n",
                item.short()
            )));
        }
        // ★ The rules of a release (ledger #1199): a place taken as a principal is left by that
        // principal or by the dispatcher's override, never by another waiter naming it.
        let leaving: Vec<&Place> = if let Some(own) = named.iter().copied().find(|p| {
            p.principal
                .as_deref()
                .is_some_and(|p| inv.capability.acts_as(p))
        }) {
            vec![own]
        } else if dispatcher {
            named
        } else if let Some(free) = named.iter().copied().find(|p| p.principal.is_none()) {
            vec![free]
        } else {
            return Err(Error::Denied(
                format!(
                    "{from} waits for {} as {}, and this capability acts as {}: a place taken as \
                     a principal is left by that principal, or by a dispatcher holding `{}` or \
                     `{}` (the override). Naming the waiter does not make the place yours",
                    item.short(),
                    named[0].principal.as_deref().unwrap_or("another principal"),
                    caller.map_or("no principal".to_string(), |p| format!("`{p}`")),
                    ledger.cap_close(),
                    ledger.cap_write()
                )
                .into(),
            ));
        };
        let listed = leaving
            .iter()
            .map(|p| sparql::iri(&p.iri, "place"))
            .collect::<Result<Vec<_>>>()?
            .join(" ");
        let (delete, optional) = place_removal("leaving");
        let mut ops = vec![format!(
            "DELETE {{ {} }} WHERE {{ {} }}",
            client.in_graph(&delete),
            client.in_graph(&format!(
                "VALUES ?leaving {{ {listed} }} ?leaving <{}> <{}> .\n{optional}",
                v::WAITS_ON,
                item.iri
            ))
        )];
        // The offer to a place that left is void, and passes on in the same update.
        ops.extend(settle_ops(client, &item.iri, now, &queue));
        client.update(&batch(&ops)).await?;
        Ok(plain(format!(
            "{} no longer waited for by {from}\n",
            item.short()
        )))
    }
}

/// One waiter, for the JSON face.
fn waiter_json(place: usize, p: &Place, offer: Option<&Offer>) -> json::Waiter {
    json::Waiter {
        place,
        seq: p.seq,
        waiter: p.waiter.clone(),
        principal: p.principal.clone(),
        asked_at: p.asked_at.map(sparql::iso8601),
        expires: sparql::iso8601(p.expires),
        poll: p.poll.clone(),
        offered: offer.is_some_and(|o| o.place.iri == p.iri),
    }
}

/// The plain face of a queue.
fn queue_text(item: &Item, queue: &Queue) -> String {
    let short = item.short();
    if queue.places.is_empty() {
        return format!("{short}: nobody is waiting\n");
    }
    let mut text = format!(
        "{short}: {} waiting{}\n",
        queue.places.len(),
        match &queue.offer {
            Some(o) => format!(
                "; offered to {} until {}",
                o.place.display(),
                sparql::iso8601(o.ends)
            ),
            None => String::new(),
        }
    );
    for (n, p) in queue.places.iter().enumerate() {
        text.push_str(&format!(
            "  {}. {}{}, place until {}{}\n",
            n + 1,
            p.display(),
            p.asked_at
                .map(|t| format!(", asked {}", sparql::iso8601(t)))
                .unwrap_or_default(),
            sparql::iso8601(p.expires),
            if queue.offer.as_ref().is_some_and(|o| o.place.iri == p.iri) {
                ", offered"
            } else {
                ""
            }
        ));
    }
    text
}

/// `urn:iki:ledger:{ledger}:offers`: the dispatcher's sweep.
#[derive(Clone)]
pub(super) struct OffersEndpoint;

#[async_trait]
impl Endpoint for OffersEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        if inv.request.verb != Verb::Sink {
            return Err(unsupported("ledger-offers", inv.request.verb));
        }
        let client = StoreClient::new(inv, ledger_for(inv, Need::Close)?);
        wanted_face(inv, &[PLAIN])?;
        let now = now_ms(inv)?;
        let ledger = client.ledger().clone();
        let at = datetime(now);

        // 1. Lapsed places, in a bounded batch, rechecked under the write lock.
        let rows = client
            .select(&format!(
                "SELECT DISTINCT ?place ?item WHERE {{ {} }} LIMIT {SWEEP_BATCH}",
                client.in_graph(&format!(
                    "?place <{}> <{}> ; <{}> ?item ; <{}> ?ends . FILTER(?ends <= {at})",
                    v::ext::TYPE,
                    v::WAIT_PLACE_CLASS,
                    v::WAITS_ON,
                    v::WAIT_EXPIRES
                ))
            ))
            .await?;
        let mut items: Vec<String> = Vec::new();
        let mut lapsed = 0;
        if !rows.is_empty() {
            let mut listed = Vec::new();
            for row in &rows {
                if let (Some(place), Some(item)) = (row.get("place"), row.get("item")) {
                    listed.push(sparql::iri(&place.value, "place")?);
                    if !items.contains(&item.value) {
                        items.push(item.value.clone());
                    }
                }
            }
            lapsed = listed.len();
            let (delete, optional) = place_removal("lapsed");
            client
                .update(&format!(
                    "DELETE {{ {} }} WHERE {{ {} }}",
                    client.in_graph(&delete),
                    client.in_graph(&format!(
                        "VALUES ?lapsed {{ {} }} ?lapsed <{}> ?lapsedEnds . \
                         FILTER(?lapsedEnds <= {at})\n{optional}",
                        listed.join(" "),
                        v::WAIT_EXPIRES
                    ))
                ))
                .await?;
        }
        // 2. Items whose offer has ended, or that are free with live waiters and no offer.
        let rows = client
            .select(&format!(
                "SELECT DISTINCT ?item WHERE {{ {} }} LIMIT {SWEEP_BATCH}",
                client.in_graph(&format!(
                    "{{ ?item <{to}> ?to ; <{ends}> ?ends . FILTER(?ends <= {at}) }} UNION \
                     {{ ?place <{on}> ?item ; <{expires}> ?live . FILTER(?live > {at}) \
                        FILTER NOT EXISTS {{ ?item <{by}> ?held }} \
                        FILTER NOT EXISTS {{ ?item <{to}> ?any }} }}",
                    to = v::OFFERED_TO,
                    ends = v::OFFER_EXPIRES,
                    on = v::WAITS_ON,
                    expires = v::WAIT_EXPIRES,
                    by = v::CLAIMED_BY,
                ))
            ))
            .await?;
        for row in &rows {
            if let Some(item) = row.get("item") {
                if !items.contains(&item.value) {
                    items.push(item.value.clone());
                }
            }
        }
        let mut lines = Vec::new();
        for iri in &items {
            let Some(item) = model::load_item(&client, iri).await? else {
                continue;
            };
            let before = read(&client, &item, now).await?;
            if !before.unsettled(&item) {
                continue;
            }
            settle(&client, &item, now).await?;
            let after = read(&client, &item, now).await?;
            lines.push(match &after.offer {
                Some(offer) => format!(
                    "  {} offered to {} until {}\n",
                    item.short(),
                    offer.place.display(),
                    sparql::iso8601(offer.ends)
                ),
                None => format!("  {} no longer offered: nobody is waiting\n", item.short()),
            });
        }
        let mut text = if lapsed == 0 && lines.is_empty() {
            format!(
                "{}: no lapsed place and no ended offer, so nothing was written\n",
                ledger.name()
            )
        } else {
            format!(
                "{}: removed {lapsed} lapsed place(s); settled the offer on {} item(s)\n",
                ledger.name(),
                lines.len()
            )
        };
        for line in lines {
            text.push_str(&line);
        }
        Ok(plain(text))
    }

    fn name(&self) -> &str {
        "ledger-offers"
    }

    fn describe(&self) -> Description {
        Description::new("ledger-offers")
            .title("Pass on the ended offers of a ledger")
            .summary(
                "The dispatcher's sweep of the claim queues: removes up to 128 places that \
                 lapsed without a renewal, and settles up to 128 items whose offer ended \
                 unclaimed (its waiter leaves the queue and the next live waiter is offered the \
                 item). Nothing is claimed or released. A claim, join or leave on the item does \
                 the same for that item, so this is only for an item nobody is acting on.",
            )
            .verb(Verb::Meta)
            .action(write_scopes(
                ActionSpec::new(Verb::Sink)
                    .input(ledger_arg("offers"))
                    .summary(
                        "Sweep lapsed places and ended offers. Needs \
                         `urn:cap:ledger:close:{ledger}` or `urn:cap:ledger:write:{ledger}`: \
                         the dispatcher's grant.",
                    )
                    .input(as_arg(&[PLAIN]))
                    .output(PLAIN),
                CAP_ANY,
            ))
    }
}

/// One effective offer in a ledger, for `next`.
pub(crate) struct Standing {
    item: String,
    waiter: String,
    principal: Option<String>,
    ends: u64,
}

/// Every effective offer in this ledger at `now` ([`Queue::effective`]): the same rule the
/// claim guard applies after its settle, so `next` never shows a stranger an item whose claim
/// would then be refused. The candidates are the free open items with a recorded offer or a
/// live place; each one's queue is read and decided as a claim would decide it.
pub(crate) async fn standing_offers(
    client: &StoreClient<'_, '_>,
    now: u64,
) -> Result<Vec<Standing>> {
    let rows = client
        .select(&format!(
            "SELECT DISTINCT ?item WHERE {{ {} }}",
            client.in_graph(&format!(
                "{{ ?item <{to}> ?to }} UNION \
                 {{ ?place <{on}> ?item ; <{expires}> ?live . FILTER(?live > {at}) }}\n\
                 ?item <{status}> <{open}> .\n\
                 FILTER NOT EXISTS {{ ?item <{by}> ?held }}",
                to = v::OFFERED_TO,
                on = v::WAITS_ON,
                expires = v::WAIT_EXPIRES,
                status = v::STATUS,
                open = v::OPEN,
                by = v::CLAIMED_BY,
                at = datetime(now),
            ))
        ))
        .await?;
    let mut offers = Vec::new();
    for row in &rows {
        let Some(iri) = row.get("item") else {
            continue;
        };
        let Some(item) = model::load_item(client, &iri.value).await? else {
            continue;
        };
        let queue = read(client, &item, now).await?;
        if let Some(offer) = queue.effective(&item, now) {
            offers.push(Standing {
                item: item.iri.clone(),
                waiter: offer.place.waiter,
                principal: offer.place.principal,
                ends: offer.ends,
            });
        }
    }
    Ok(offers)
}

/// Move every ready item offered to another waiter than `waiter` (as `principal`) out of
/// `set`, excluded as offered. The earliest end among the offers, which is when the answer
/// stops being true.
pub(crate) fn exclude_offered(
    set: &mut crate::select::ReadySet,
    offers: &[Standing],
    waiter: Option<&str>,
    principal: Option<&str>,
) -> Option<u64> {
    if offers.is_empty() {
        return None;
    }
    let ready = std::mem::take(&mut set.ready);
    for item in ready {
        match offers.iter().find(|o| o.item == item.iri) {
            Some(o)
                if !(waiter == Some(o.waiter.as_str()) && o.principal.as_deref() == principal) =>
            {
                let who = match &o.principal {
                    Some(p) => format!("{} (as {p})", o.waiter),
                    None => o.waiter.clone(),
                };
                set.excluded.push((
                    item,
                    crate::select::Excluded::Offered {
                        waiter: who,
                        ends: o.ends,
                    },
                ));
            }
            _ => set.ready.push(item),
        }
    }
    offers.iter().map(|o| o.ends).min()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_offer_bound_is_the_poll_plus_a_margin_between_a_floor_and_a_ceiling() {
        assert_eq!(offer_bound(None), 60_000);
        assert_eq!(offer_bound(Some(10_000)), 60_000);
        assert_eq!(offer_bound(Some(120_000)), 150_000);
        assert_eq!(offer_bound(Some(600_000)), 300_000);
    }
}
