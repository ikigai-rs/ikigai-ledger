//! A lapsed lease, made visible (ledger #3709): `urn:iki:ledger:{ledger}:stalled` lists the open
//! items held on a lease that has run out, and `urn:iki:ledger:{ledger}:lapse` records each such
//! lapse once, as a `ledger:LapseEvent` in the item's history, and routes it to a person when the
//! host asked for that.
//!
//! ```text
//! urn:iki:ledger:{ledger}:stalled   Source   open, held on a run-out lease, not handed back; oldest first
//! urn:iki:ledger:{ledger}:lapse     Sink     record every lapse not yet recorded; route it if configured
//! ```
//!
//! # Who notices a lapse
//!
//! A lease runs out at an instant nobody asks about, and the ledger acts only when asked. Three
//! candidates, and why the sweep is the one:
//!
//! - **a sweep on read** would make `next`, `items` and the doctor writes: they are reads
//!   under the read grant, and a read that writes is invalidated by its own write;
//! - **the next claim attempt** comes only when someone already wants the item, which is exactly
//!   what does not happen to a stalled one, and the takeover it ends in already records its
//!   release (`releaseReason takeover`);
//! - **the dispatcher's tick** (gonk's) already runs on a schedule, holds the dispatcher's grant,
//!   and is the reader that should act on a lapse. It sends `sink …:lapse`, and that is the
//!   detector.
//!
//! The stalled view needs no sweep: it is computed from the claims as they stand, so an item is
//! listed from the moment its lease ends, swept or not. Each row says whether its lapse is
//! recorded yet.
//!
//! # ★ Written once
//!
//! Each lapse is one store update whose `WHERE` holds only while the item is open, not handed
//! back, still held on that same run-out lease, and has no lapse event for it yet. Two sweeps at
//! once, or a sweep run every minute, record each lapse once; the store holds its write lock
//! across the update. A lapse is identified by its item and the lease end it records
//! (`ledger:lapsedAt`), so a holder that renews after a lapse and lapses again records a second.
//!
//! # Nothing is released
//!
//! The lapse event records; it does not reap (README, "Not built, on purpose"). The claim stays,
//! `next` keeps excluding the item as `lease-expired`, and the remedies are the ones the doctor
//! names: the same worker resumes with `claim renew=true`, which keeps its epoch, or another takes
//! it with `claim takeover=true from=<holder>`, which raises the epoch so the stale holder's
//! fenced writes are refused. Either ends the stall. No resume verb is added: those two already
//! carry the epoch forward.

use std::collections::BTreeMap;

use super::*;

use crate::sparql::iso8601;

const LAPSE_FACES: [&str; 2] = [PLAIN, JSON];

/// The opt-in lapse routes, by ledger name: the label a recorded lapse adds to its item.
pub(super) type Routes = Arc<BTreeMap<String, String>>;

/// One stalled item, as read.
struct Found {
    iri: String,
    number: i64,
    title: String,
    holder: String,
    principal: Option<String>,
    kind: Option<String>,
    epoch: Option<i64>,
    lapsed: u64,
    recorded: bool,
}

impl Found {
    fn json(&self, ledger: &Ledger) -> json::Stalled {
        json::Stalled {
            item: json::item_ref(ledger, self.number, &self.iri),
            title: self.title.clone(),
            holder: self.holder.clone(),
            principal: self.principal.clone(),
            kind: self.kind.clone(),
            epoch: self.epoch,
            lapsed: iso8601(self.lapsed),
            recorded: self.recorded,
        }
    }

    /// `#7 Ship it: held by worker-1 at epoch 2, whose lease ran out at …`
    fn sentence(&self, ledger: &Ledger) -> String {
        format!(
            "{} {}: held by {}{}, whose lease ran out at {}",
            ledger.number(self.number),
            self.title,
            self.holder,
            match self.epoch {
                Some(epoch) => format!(" at epoch {epoch}"),
                None => String::new(),
            },
            iso8601(self.lapsed)
        )
    }
}

/// The pattern "`item` is open, held on a lease that ran out at or before `now`, and not handed
/// back", binding `?holder` and `?expires`. Inside a ledger's graph.
fn stalled_pattern(item: &str, now: u64) -> String {
    format!(
        "{item} <{type_}> <{class}> ; <{by}> ?holder ; <{expires}> ?expires .\n\
         FILTER(?expires <= {now})\n\
         FILTER NOT EXISTS {{ {item} <{status}> <{closed}> }}\n\
         FILTER NOT EXISTS {{ {item} <{awaiting}> ?whom }}\n\
         OPTIONAL {{ {item} <{principal}> ?principal }}\n\
         OPTIONAL {{ {item} <{kind}> ?kind }}\n\
         OPTIONAL {{ {item} <{epoch}> ?epoch }}\n",
        type_ = v::ext::TYPE,
        class = v::ITEM_CLASS,
        by = v::CLAIMED_BY,
        expires = v::LEASE_EXPIRES,
        now = datetime(now),
        status = v::STATUS,
        closed = v::CLOSED,
        awaiting = v::AWAITING,
        principal = v::CLAIM_PRINCIPAL,
        kind = v::CLAIM_KIND,
        epoch = v::CLAIM_EPOCH,
    )
}

/// The pattern "`item` has a lapse event for the lease that ran out at `?expires`".
fn recorded_pattern(item: &str, event: &str) -> String {
    format!(
        "{event} <{type_}> <{class}> ; <{on}> {item} ; <{at}> ?lapsedAt .\n\
         FILTER(?lapsedAt = ?expires)",
        type_ = v::ext::TYPE,
        class = v::LAPSE_EVENT_CLASS,
        on = v::ON_ITEM,
        at = v::LAPSED_AT,
    )
}

/// Every stalled item in the ledger, the lease that ran out first leading.
async fn stalled(client: &StoreClient<'_, '_>, now: u64) -> Result<Vec<Found>> {
    let query = format!(
        "SELECT ?item ?number ?title ?holder ?principal ?kind ?epoch ?expires ?lapse WHERE {{ {} }} \
         ORDER BY ?expires ?number",
        client.in_graph(&format!(
            "{}?item <{number}> ?number .\nOPTIONAL {{ ?item <{title}> ?title }}\n\
             OPTIONAL {{ {} }}",
            stalled_pattern("?item", now),
            recorded_pattern("?item", "?lapse"),
            number = v::NUMBER,
            title = v::ext::TITLE,
        ))
    );
    let mut found: Vec<Found> = Vec::new();
    for row in client.select(&query).await? {
        let (Some(iri), Some(number), Some(holder), Some(lapsed)) = (
            row.get("item").map(|b| b.value.clone()),
            row.get("number").and_then(|b| b.as_i64()),
            row.get("holder").map(|b| b.value.clone()),
            row.get("expires").and_then(|b| model::millis(&b.value)),
        ) else {
            continue;
        };
        let recorded = row.contains_key("lapse");
        // One row per lapse event at most, but an out-of-band write can double a field.
        if let Some(seen) = found.iter_mut().find(|f| f.iri == iri) {
            seen.recorded |= recorded;
            continue;
        }
        found.push(Found {
            iri,
            number,
            title: row
                .get("title")
                .map(|b| b.value.clone())
                .unwrap_or_default(),
            holder,
            principal: row.get("principal").map(|b| b.value.clone()),
            kind: row
                .get("kind")
                .and_then(|b| crate::claim::ClaimKind::from_iri(&b.value))
                .map(|kind| kind.as_str().to_string()),
            epoch: row.get("epoch").and_then(|b| b.as_i64()),
            lapsed,
            recorded,
        });
    }
    Ok(found)
}

/// `urn:iki:ledger:{ledger}:stalled` — read-only.
#[derive(Clone)]
pub(super) struct StalledEndpoint;

#[async_trait]
impl Endpoint for StalledEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        if inv.request.verb != Verb::Source {
            return Err(unsupported("ledger-stalled", inv.request.verb));
        }
        let client = StoreClient::new(inv, ledger_for(inv, Need::Read)?);
        let want = wanted_face(inv, &LAPSE_FACES)?;
        let now = client.authority_now().await?;
        let ledger = client.ledger().clone();
        let found = stalled(&client, now).await?;
        let repr = if want == JSON {
            json_repr(json::render(&json::StalledDocument {
                schema: json::SCHEMA,
                ledger: ledger.name().to_string(),
                checked_at: iso8601(now),
                stalled: found.iter().map(|f| f.json(&ledger)).collect(),
            })?)
        } else if found.is_empty() {
            plain(format!(
                "{}: no stalled items: no open item is held on a lease that has run out\n",
                ledger.name()
            ))
        } else {
            let mut text = format!(
                "{}: {} stalled item(s), the oldest lapse first\n",
                ledger.name(),
                found.len()
            );
            for f in &found {
                text.push_str(&format!(
                    "{}; {}\n",
                    f.sentence(&ledger),
                    if f.recorded {
                        "the lapse is recorded"
                    } else {
                        "the lapse is not recorded yet"
                    }
                ));
            }
            text.push_str(&format!(
                "remedy: the holder resumes (`sink {claim} item=<n> renew=true`), another takes it \
                 over (`sink {claim} item=<n> takeover=true from=<holder>`), or a dispatcher \
                 releases it (`delete {claim} item=<n>`)\n",
                claim = ledger.resource("claim")
            ));
            plain(text)
        };
        // The caller's cache clock cannot enforce a store-clock deadline.
        Ok(repr
            .with_uncached_reason(ikigai_core::UncachedReason::Declared)
            .depends_on(ikigai_store::UPDATE_THREAD)
            .depends_on(ikigai_store::LOAD_THREAD)
            .depends_on(ikigai_store::GRAPH_UPDATE_THREAD))
    }

    fn name(&self) -> &str {
        "ledger-stalled"
    }

    fn describe(&self) -> Description {
        read_scopes(
            Description::new("ledger-stalled")
                .title("The stalled items of a ledger")
                .summary(
                    "Read-only: every open item still held on a lease that has run out and not \
                     handed back to its dispatcher, the lease that ran out first leading, with \
                     its holder, epoch, when the lease ended, and whether `lapse` has recorded \
                     the lapse yet. An expired lease is never silently free, so each stays here \
                     until its holder renews, another takes it over, or it is released.",
                )
                .verb(Verb::Source)
                .verb(Verb::Meta)
                .input(ledger_arg("stalled"))
                .input(as_arg(&LAPSE_FACES))
                .output(PLAIN)
                .output(JSON),
        )
    }
}

/// `urn:iki:ledger:{ledger}:lapse` — the dispatcher's sweep.
#[derive(Clone)]
pub(super) struct LapseEndpoint {
    pub(super) routes: Routes,
}

/// The update that records the lapse of `found` as `event`, and when `route` names a label, adds
/// it in the same update. Every operation holds only while the lapse event lands. Also the IRI
/// of the label's edit event, so the caller can ask whether the label changed anything.
fn lapse_update(
    client: &StoreClient<'_, '_>,
    found: &Found,
    event: &str,
    route: Option<&str>,
    who: &Attribution,
    now: u64,
) -> Result<(String, Option<String>)> {
    let item = sparql::iri(&found.iri, "item")?;
    let mut triples = event_triples(event, v::LAPSE_EVENT_CLASS, &found.iri, who, now);
    // An unbound variable drops only its own triple, so a claim with no principal, kind or
    // epoch records the rest.
    triples.push_str(&format!(
        "\n<{event}> <{by}> ?holder ; <{at}> ?expires .\n<{event}> <{principal}> ?principal .\n\
         <{event}> <{kind}> ?kind .\n<{event}> <{epoch}> ?epoch .",
        by = v::CLAIMED_BY,
        at = v::LAPSED_AT,
        principal = v::CLAIM_PRINCIPAL,
        kind = v::CLAIM_KIND,
        epoch = v::LAPSED_EPOCH,
    ));
    let mut operations = vec![format!(
        "INSERT {{ {} }} WHERE {{ {} }}",
        client.in_graph(&triples),
        client.in_graph(&format!(
            "{}FILTER(?expires = {lapsed})\nFILTER NOT EXISTS {{ {} }}",
            stalled_pattern(&item, now),
            recorded_pattern(&item, "?prior"),
            lapsed = datetime(found.lapsed),
        ))
    )];
    let mut label_event = None;
    if let Some(label) = route {
        let landed = format!("<{event}> <{}> {item} .", v::ON_ITEM);
        let label_triple = format!("{item} <{}> {} .", v::LABEL, literal(label));
        let edit = event_iri(&found.iri, "edit", now)?;
        operations.push(format!(
            "INSERT {{ {} }} WHERE {{ {} }}",
            client.in_graph(&format!(
                "{label_triple}\n{}\n<{edit}> <{}> {} .",
                edit_event_triples(&edit, &found.iri, "label", who, now),
                v::EDIT_LABEL,
                literal(label)
            )),
            client.in_graph(&format!("{landed} {}", absent(&label_triple)))
        ));
        // The label changes the item, so it is stamped modified; the lapse alone is history.
        operations.push(touch_dated(
            client,
            &found.iri,
            now,
            None,
            &format!("{landed} <{edit}> <{}> {item} .", v::ON_ITEM),
        ));
        label_event = Some(edit);
    }
    Ok((batch(&operations), label_event))
}

#[async_trait]
impl Endpoint for LapseEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        if inv.request.verb != Verb::Sink {
            return Err(unsupported("ledger-lapse", inv.request.verb));
        }
        let client = StoreClient::new(inv, ledger_for(inv, Need::Close)?);
        let want = wanted_face(inv, &LAPSE_FACES)?;
        let now = client.authority_now().await?;
        let who = Attribution::committer(inv)?;
        let ledger = client.ledger().clone();
        let route = self.routes.get(ledger.name()).map(String::as_str);

        // The read only finds the candidates; each update decides for itself whether its lapse
        // is still there and still unrecorded.
        let mut recorded: Vec<(Found, bool)> = Vec::new();
        let mut already = 0;
        for found in stalled(&client, now).await? {
            if found.recorded {
                already += 1;
                continue;
            }
            let event = event_iri(&found.iri, "lapse", now)?;
            let (update, label_event) = lapse_update(&client, &found, &event, route, &who, now)?;
            client.update(&update).await?;
            let landed = client
                .ask(&format!(
                    "ASK {{ {} }}",
                    client.in_graph(&format!("<{event}> <{}> <{}> .", v::ON_ITEM, found.iri))
                ))
                .await?;
            if !landed {
                // Another sweep recorded it first, or the claim moved on in between.
                continue;
            }
            let labeled = match label_event {
                Some(edit) => {
                    client
                        .ask(&format!(
                            "ASK {{ {} }}",
                            client.in_graph(&format!(
                                "<{edit}> <{}> <{}> .",
                                v::ON_ITEM,
                                found.iri
                            ))
                        ))
                        .await?
                }
                None => false,
            };
            recorded.push((found, labeled));
        }

        if want == JSON {
            return Ok(json_repr(json::render(&json::LapseDocument {
                schema: json::SCHEMA,
                ledger: ledger.name().to_string(),
                checked_at: iso8601(now),
                route_label: route.map(str::to_string),
                recorded: recorded
                    .iter()
                    .map(|(f, _)| {
                        let mut row = f.json(&ledger);
                        row.recorded = true;
                        row
                    })
                    .collect(),
                already_recorded: already,
            })?));
        }
        let mut text = match (recorded.len(), already) {
            (0, 0) => format!(
                "{}: no lapse to record: no open item is held on a lease that has run out, so \
                 nothing was written\n",
                ledger.name()
            ),
            (0, n) => format!(
                "{}: no new lapse: {n} stalled item(s) already had theirs recorded, so nothing \
                 was written\n",
                ledger.name()
            ),
            (n, 0) => format!("{}: recorded {n} lapse(s)\n", ledger.name()),
            (n, m) => format!(
                "{}: recorded {n} lapse(s); {m} more stalled item(s) already had theirs recorded\n",
                ledger.name()
            ),
        };
        for (found, labeled) in &recorded {
            text.push_str(&format!("  {}", found.sentence(&ledger)));
            if *labeled {
                text.push_str(&format!("; labeled {}", route.unwrap_or_default()));
            }
            text.push('\n');
        }
        Ok(plain(text))
    }

    fn name(&self) -> &str {
        "ledger-lapse"
    }

    fn describe(&self) -> Description {
        Description::new("ledger-lapse")
            .title("Record the lapsed leases of a ledger")
            .summary(
                "The dispatcher's sweep: for every open item held on a lease that has run out \
                 and not handed back, record the lapse once as an event in the item's history \
                 (who held it, the epoch, when the lease ended). A sweep that finds a lapse \
                 already recorded writes nothing for it. When the host routes this ledger's \
                 lapses to a person, the same update adds the route's label to the item. \
                 Nothing is released: the claim stays until it is renewed, taken over or \
                 released.",
            )
            .verb(Verb::Meta)
            .action(write_scopes(
                ikigai_core::ActionSpec::new(Verb::Sink)
                    .input(ledger_arg("lapse"))
                    .summary(
                        "Record every lapse not yet recorded. Needs \
                         `urn:cap:ledger:close:{ledger}` or `urn:cap:ledger:write:{ledger}`: \
                         the dispatcher's grant.",
                    )
                    .input(as_arg(&LAPSE_FACES))
                    .output(PLAIN)
                    .output(JSON),
                CAP_ANY,
            ))
    }
}
