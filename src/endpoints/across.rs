//! Typed links across ledgers (ledger #2829): an edge admitted by a `link-in` grant, with a
//! mirror the ledger writes into the target's graph.
//!
//! ```text
//! subject's graph (cli)                         target's graph (acme)
//! <cli#4:link:blocks:acme:{id}>                 <acme#7:mirror:blocks:cli:{id}>
//!     a ledger:Link ; onItem cli#4                  a ledger:Mirror ; onItem acme#7
//!     linkType ledger:blocks                        linkType ledger:blocks
//!     linkTarget acme#7                             mirrorOf cli#4            (its IRI only)
//!     committer … ; committedAt …                   subjectOpen true ; linkPresent true
//!                                                   mirrorVersion 3
//! ```
//!
//! A ledger stays a boundary for content: the target's graph learns the subject's IRI and
//! whether it is open, and nothing else, so `next` and a reader there read their own graph
//! only. The edge itself is the subject's, written beside it and archived with it.
//!
//! # ★ Two graphs cannot be one update, so the mirror is a compare-and-set on a version
//!
//! `urn:iki:store:graph-update` changes exactly one graph, and its `WHERE` cannot read another
//! (`crate::sparql` has the reason), so the subject's status and the target's mirror are two
//! updates whatever this crate does. What must not happen is the 0.14.0 race (ledger #2764,
//! #2766) in a new place: a writer that read "open", lost the race to a close, and then wrote
//! "open" over the close's mirror.
//!
//! So nothing writes the mirror from a value it read and then decided on. **The version is
//! decided inside the subject's own update**: every close and reopen adds a
//! `ledger:StatusEvent`, and every link and unlink a `ledger:EditEvent`, in the same update as
//! the change, so the subject's count of those events grows with every change a mirror reflects.
//! An importer's events are not counted (ledger #2843): they change nothing a mirror reflects.
//! After its update, a writer reads the subject's status, that count and its links in ONE query
//! (one snapshot), and writes each mirror only when the version the mirror already holds is no
//! greater. Whatever order the mirror writes land in, the one read from the latest snapshot
//! wins, and every change is followed by a write from a snapshot that includes it. An unlink
//! leaves the mirror with `linkPresent false` rather than removing it, so a late write from an
//! older snapshot cannot bring the edge back.
//!
//! ⚠ **What it does not cover**: a process that stops between the subject's update and the
//! mirror's leaves the mirror at its previous version until the next close, reopen, link or
//! unlink of the subject writes it again, and so does a close by a caller whose grants did not
//! reach a target linked while that close was in flight (the close says so). A host's doctor
//! sees both graphs and this crate does not; see `README.md`, "Links across ledgers".

use std::collections::BTreeMap;

use super::*;

/// The link types a mirror is kept for: `related` needs none, because nothing in the target
/// ledger reads it.
fn mirrored(predicate: &str) -> bool {
    predicate == v::BLOCKS || predicate == v::PARENT
}

/// An item IRI's ledger and id as one IRI-safe tail: `cli:01m…`.
fn tail(item: &str) -> String {
    item.strip_prefix(crate::ledger::PREFIX)
        .unwrap_or(item)
        .replacen(":item:", ":", 1)
}

/// The short name of a link predicate: `blocks`, `parent`, `related`.
pub(super) fn short(predicate: &str) -> &'static str {
    v::LINK_TYPES
        .iter()
        .find(|(_, iri)| *iri == predicate)
        .map(|(short, _)| *short)
        .unwrap_or("related")
}

/// `{subject}:link:{type}:{target ledger}:{target id}`, in the subject's graph.
fn link_iri(subject: &str, predicate: &str, target: &str) -> String {
    format!("{subject}:link:{}:{}", short(predicate), tail(target))
}

/// `{target}:mirror:{type}:{subject ledger}:{subject id}`, in the target's graph.
fn mirror_iri(target: &str, predicate: &str, subject: &str) -> String {
    format!("{target}:mirror:{}:{}", short(predicate), tail(subject))
}

/// The ledger a link's object names when it is NOT `here`: an item IRI of another ledger, or
/// a number qualified with another ledger's name (`acme#7`). `None` for everything else,
/// which resolves inside `here` as it always did.
pub(super) fn other_ledger(here: &Ledger, reference: &str) -> Option<Ledger> {
    let reference = reference.trim();
    let there = if reference.starts_with("urn:") {
        Ledger::item_iri(reference).map(|(ledger, _)| ledger)
    } else {
        match reference.split_once('#') {
            Some((name, number))
                if !name.is_empty() && !name.contains(':') && number.parse::<i64>().is_ok() =>
            {
                Ledger::parse(name).ok()
            }
            _ => None,
        }
    }?;
    (&there != here).then_some(there)
}

/// The store grants a writer of `ledger`'s mirrors must hold: the mirror's update reads the
/// target's graph (its `WHERE`) and writes it.
fn store_grants(ledger: &Ledger) -> [String; 2] {
    [
        ikigai_store::cap_read_graph(&ledger.graph()),
        ikigai_store::cap_write_graph(&ledger.graph()),
    ]
}

/// The `(predicate, target)` of every cross-ledger link `subject` carries.
pub(crate) async fn outbound(
    client: &StoreClient<'_, '_>,
    subject: &str,
) -> Result<Vec<(String, String)>> {
    Ok(outbound_of(client, &[subject])
        .await?
        .remove(subject)
        .unwrap_or_default())
}

/// [`outbound`] for a whole page of `subjects` at once, by subject: one query per `VALUES`
/// list (one for any page a listing serves), never one per item (ledger #2927).
pub(crate) async fn outbound_of(
    client: &StoreClient<'_, '_>,
    subjects: &[&str],
) -> Result<BTreeMap<String, Vec<(String, String)>>> {
    let mut by_subject: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for values in model::values_lists(subjects)? {
        let rows = client
            .select(&format!(
                "SELECT ?item ?type ?target WHERE {{ {} }} ORDER BY ?item ?type ?target",
                client.in_graph(&format!(
                    "VALUES ?item {{ {values} }}\n\
                     ?link <{}> <{}> ; <{}> ?item ; <{}> ?type ; <{}> ?target .",
                    v::ext::TYPE,
                    v::LINK_CLASS,
                    v::ON_ITEM,
                    v::LINK_TYPE,
                    v::LINK_TARGET
                ))
            ))
            .await?;
        for row in &rows {
            if let (Some(item), Some(predicate), Some(target)) =
                (row.get("item"), row.get("type"), row.get("target"))
            {
                by_subject
                    .entry(item.value.clone())
                    .or_default()
                    .push((predicate.value.clone(), target.value.clone()));
            }
        }
    }
    Ok(by_subject)
}

/// The present mirrors on each of `items`, by item: `(predicate, subject, open)`, sorted. One
/// query per `VALUES` list, as [`outbound_of`].
pub(crate) async fn inbound_of(
    client: &StoreClient<'_, '_>,
    items: &[&str],
) -> Result<BTreeMap<String, Vec<(String, String, bool)>>> {
    let mut by_item: BTreeMap<String, Vec<(String, String, bool)>> = BTreeMap::new();
    for values in model::values_lists(items)? {
        let rows = client
            .select(&format!(
                "SELECT ?item ?type ?subject ?open WHERE {{ {} }} ORDER BY ?item ?type ?subject",
                client.in_graph(&format!(
                    "VALUES ?item {{ {values} }}\n\
                     ?mirror <{}> <{}> ; <{}> ?item ; <{}> ?type ; <{}> ?subject ; <{}> ?open ; \
                     <{}> true .",
                    v::ext::TYPE,
                    v::MIRROR_CLASS,
                    v::ON_ITEM,
                    v::LINK_TYPE,
                    v::MIRROR_OF,
                    v::SUBJECT_OPEN,
                    v::LINK_PRESENT
                ))
            ))
            .await?;
        for row in &rows {
            if let (Some(item), Some(predicate), Some(subject), Some(open)) = (
                row.get("item"),
                row.get("type"),
                row.get("subject"),
                row.get("open"),
            ) {
                by_item.entry(item.value.clone()).or_default().push((
                    predicate.value.clone(),
                    subject.value.clone(),
                    open.value == "true",
                ));
            }
        }
    }
    Ok(by_item)
}

/// An item as its own read shows it: its links into other ledgers beside its local edges, and
/// the mirrors of links from other ledgers into it.
pub(super) async fn decorate(
    inv: &Invocation<'_>,
    client: &StoreClient<'_, '_>,
    item: &mut Item,
) -> Result<()> {
    decorate_all(inv, client, std::slice::from_mut(item)).await
}

/// [`decorate`] for a page of items, in two queries whatever its length: one for the links
/// out and one for the mirrors in (ledger #2927). A listing row is the same item object its
/// own read shows.
pub(super) async fn decorate_all(
    inv: &Invocation<'_>,
    client: &StoreClient<'_, '_>,
    items: &mut [Item],
) -> Result<()> {
    decorate_each(inv, client, items.iter_mut().collect()).await
}

/// [`decorate_all`] over every item a `next` selection names, ranked and excluded alike, in the
/// same two queries (ledger #2931): a `next` row is the item object its own read shows.
pub(super) async fn decorate_selection(
    inv: &Invocation<'_>,
    client: &StoreClient<'_, '_>,
    selection: &mut crate::select::Selection,
) -> Result<()> {
    let items = selection
        .ranked
        .iter_mut()
        .map(|(_, item)| item)
        .chain(selection.excluded.iter_mut().map(|(item, _)| item))
        .collect();
    decorate_each(inv, client, items).await
}

async fn decorate_each(
    inv: &Invocation<'_>,
    client: &StoreClient<'_, '_>,
    mut items: Vec<&mut Item>,
) -> Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    let iris: Vec<String> = items.iter().map(|i| i.iri.clone()).collect();
    let iris: Vec<&str> = iris.iter().map(String::as_str).collect();
    let mut outbound = outbound_of(client, &iris).await?;
    let mut inbound = inbound_of(client, &iris).await?;
    for item in items.iter_mut() {
        for (predicate, target) in outbound.remove(&item.iri).unwrap_or_default() {
            let list = match predicate.as_str() {
                p if p == v::BLOCKS => &mut item.blocks,
                p if p == v::PARENT => &mut item.parents,
                _ => &mut item.related,
            };
            if !list.contains(&target) {
                list.push(target);
                list.sort();
            }
        }
        // The caller's read on the subject's ledger decides whether the line says where to
        // read more, as it always has on `item:{id}`.
        item.mirrored = inbound
            .remove(&item.iri)
            .unwrap_or_default()
            .into_iter()
            .map(|(predicate, subject, open)| model::Mirrored {
                kind: short(&predicate).to_string(),
                readable: Ledger::of_subject(&subject)
                    .is_some_and(|l| inv.capability.allows(&l.cap_read())),
                subject,
                open,
            })
            .collect();
    }
    Ok(())
}

/// What one snapshot of the subject says: whether it is open, its version, and its mirrored
/// links. `None` when the subject is not a live item.
struct Snapshot {
    open: bool,
    version: i64,
    links: Vec<(String, String)>,
}

/// The subject's status, version and links, in ONE query, so the three are one moment.
async fn snapshot(client: &StoreClient<'_, '_>, subject: &str) -> Result<Option<Snapshot>> {
    let rows = client
        .select(&format!(
            "SELECT ?status ?version ?type ?target WHERE {{ {} }}",
            client.in_graph(&format!(
                "{{ SELECT ?status (COUNT(DISTINCT ?event) AS ?version) WHERE {{ \
                   <{subject}> <{status}> ?status . \
                   OPTIONAL {{ ?event <{on}> <{subject}> ; <{type_}> ?class . \
                     FILTER(?class = <{status_event}> || ?class = <{edit_event}>) {imported} }} \
                 }} GROUP BY ?status }} \
                 OPTIONAL {{ ?link <{type_}> <{link}> ; <{on}> <{subject}> ; \
                   <{link_type}> ?type ; <{link_target}> ?target . }}",
                status = v::STATUS,
                imported = not_imported("?event"),
                on = v::ON_ITEM,
                type_ = v::ext::TYPE,
                status_event = v::STATUS_EVENT_CLASS,
                edit_event = v::EDIT_EVENT_CLASS,
                link = v::LINK_CLASS,
                link_type = v::LINK_TYPE,
                link_target = v::LINK_TARGET,
            ))
        ))
        .await?;
    let Some(first) = rows.first() else {
        return Ok(None);
    };
    let open = first.get("status").map(|b| b.value.as_str()) != Some(v::CLOSED);
    let version = first.get("version").and_then(|b| b.as_i64()).unwrap_or(0);
    let links = rows
        .iter()
        .filter_map(|row| {
            Some((
                row.get("type")?.value.clone(),
                row.get("target")?.value.clone(),
            ))
        })
        .filter(|(predicate, _)| mirrored(predicate))
        .collect();
    Ok(Some(Snapshot {
        open,
        version,
        links,
    }))
}

/// Write the mirror of `subject → target` in the target's graph: present or not, the subject
/// open or not, at `version` — only while the target is a live item and the mirror holds no
/// greater version. The compare-and-set is the whole of the race argument above.
async fn write_mirror(
    inv: &Invocation<'_>,
    subject: &str,
    predicate: &str,
    target: &str,
    state: (bool, bool, i64),
    now: u64,
) -> Result<()> {
    let (present, open, version) = state;
    let Some(ledger) = Ledger::of_subject(target) else {
        return Ok(());
    };
    let there = StoreClient::new(inv, ledger);
    let mirror = mirror_iri(target, predicate, subject);
    let template = format!(
        "<{mirror}> <{}> <{}> ; <{}> <{target}> ; <{}> <{predicate}> ; <{}> <{subject}> ; \
         <{}> {} ; <{}> {} ; <{}> {} ; <{}> {} .",
        v::ext::TYPE,
        v::MIRROR_CLASS,
        v::ON_ITEM,
        v::LINK_TYPE,
        v::MIRROR_OF,
        v::SUBJECT_OPEN,
        boolean(open),
        v::LINK_PRESENT,
        boolean(present),
        v::MIRROR_VERSION,
        integer(version),
        v::COMMITTED_AT,
        datetime(now),
    );
    let update = format!(
        "DELETE {{ {} }} INSERT {{ {} }} WHERE {{ {} }}",
        there.in_graph(&format!("<{mirror}> ?p ?o")),
        there.in_graph(&template),
        there.in_graph(&format!(
            "{} OPTIONAL {{ <{mirror}> <{}> ?held }} \
             FILTER(!BOUND(?held) || ?held <= {}) \
             OPTIONAL {{ <{mirror}> ?p ?o }}",
            live(target),
            v::MIRROR_VERSION,
            integer(version)
        ))
    );
    there.update(&update).await
}

/// Bring every mirror of `subject` up to its current snapshot, and also the mirror of
/// `extra` (the pair an unlink just removed, which the snapshot no longer lists).
///
/// `granted_only` skips a target ledger whose store grants the caller does not hold: a close
/// checked its targets before its update, and a target linked after that check is written by
/// the link's own write. The skipped ledgers come back so the answer can name them.
pub(super) async fn sync(
    inv: &Invocation<'_>,
    client: &StoreClient<'_, '_>,
    subject: &str,
    extra: Option<(&str, &str)>,
    granted_only: bool,
    now: u64,
) -> Result<Vec<Ledger>> {
    let Some(snapshot) = snapshot(client, subject).await? else {
        return Ok(Vec::new());
    };
    let mut pairs: Vec<(String, String, bool)> = snapshot
        .links
        .iter()
        .map(|(p, t)| (p.clone(), t.clone(), true))
        .collect();
    if let Some((predicate, target)) = extra {
        if mirrored(predicate) && !pairs.iter().any(|(p, t, _)| p == predicate && t == target) {
            pairs.push((predicate.to_string(), target.to_string(), false));
        }
    }
    let mut skipped = Vec::new();
    for (predicate, target, present) in pairs {
        if let Some(ledger) = Ledger::of_subject(&target) {
            if granted_only
                && !store_grants(&ledger)
                    .iter()
                    .all(|scope| inv.capability.allows(scope))
            {
                if !skipped.contains(&ledger) {
                    skipped.push(ledger);
                }
                continue;
            }
        }
        write_mirror(
            inv,
            subject,
            &predicate,
            &target,
            (present, snapshot.open, snapshot.version),
            now,
        )
        .await?;
    }
    Ok(skipped)
}

/// The line a close or reopen adds when a target linked while it was in flight was beyond the
/// caller's grants: empty when every mirror was written.
pub(super) fn unreached(subject: &Item, ledgers: &[Ledger]) -> String {
    ledgers
        .iter()
        .map(|ledger| {
            format!(
                "  ⚠ not mirrored: {} was linked into the ledger {} while this was in flight, and \
                 this capability cannot write there; the next close, reopen or link of {} by \
                 a holder of its grants brings that mirror up to date\n",
                subject.short(),
                ledger.name(),
                subject.short()
            )
        })
        .collect()
}

/// Refuse a close or reopen of `subject` the caller could not carry into every target's
/// mirror, before anything is written: a status change whose mirror cannot follow it is the
/// stale mirror this design exists to prevent.
pub(super) async fn require_mirror_grants(
    inv: &Invocation<'_>,
    client: &StoreClient<'_, '_>,
    subject: &Item,
) -> Result<()> {
    let mut missing: Vec<String> = Vec::new();
    for (predicate, target) in outbound(client, &subject.iri).await? {
        let Some(ledger) = Ledger::of_subject(&target).filter(|_| mirrored(&predicate)) else {
            continue;
        };
        for scope in store_grants(&ledger) {
            if !inv.capability.allows(&scope) && !missing.contains(&scope) {
                missing.push(scope);
            }
        }
    }
    if missing.is_empty() {
        return Ok(());
    }
    Err(Error::Denied(format!(
        "{} has links into other ledgers, and its status is mirrored there, so changing it \
         also writes those ledgers' graphs. This capability does not hold {}. Nothing was \
         changed. Add the grant, or remove the link first with \
         `delete {} item={} content=<the other item> type=<the type>`",
        subject.short(),
        missing
            .iter()
            .map(|s| format!("`{s}`"))
            .collect::<Vec<_>>()
            .join(" or "),
        client.ledger().resource("link"),
        subject.short()
    )))
}

/// Refuse to delete an item that carries a `blocks` or `parent` link into another ledger: the
/// mirror there would go on naming it, open, with nothing left to close.
pub(super) async fn refuse_delete_with_links(
    client: &StoreClient<'_, '_>,
    item: &Item,
) -> Result<()> {
    let links = outbound(client, &item.iri).await?;
    // A `related` edge has no mirror, and is archived with its subject like any other edge.
    let Some((predicate, target)) = links.iter().find(|(p, _)| mirrored(p)) else {
        return Ok(());
    };
    Err(Error::InvalidArgument {
        name: "item".to_string(),
        detail: format!(
            "{} {} `{target}`, an item of another ledger, which keeps a mirror of it. Remove \
             the link first, with `delete {} item={} content={target} type={}`, and then \
             delete the item. Nothing was deleted",
            item.short(),
            short(predicate),
            client.ledger().resource("link"),
            item.short(),
            short(predicate)
        ),
    })
}

/// `link` and `unlink` when the object is an item of `there`, another ledger (ledger #2829).
///
/// The caller holds `write` on the subject's ledger (checked before this is called) and must
/// hold `link-in` on `there`, checked here before the object is looked up, so a caller without
/// it learns nothing about the other ledger, not even whether the item exists.
#[allow(clippy::too_many_arguments)]
pub(super) async fn link(
    inv: &Invocation<'_>,
    client: &StoreClient<'_, '_>,
    from: &Item,
    reference: &str,
    there: Ledger,
    name: &str,
    predicate: &str,
    want: &str,
    claimed: Option<u64>,
    now: u64,
) -> Result<Representation> {
    let there = grant_for(inv, there, Need::LinkIn)?;
    let sink = inv.request.verb == Verb::Sink;
    // The edge and its mirror are two writes, so a caller that cannot write the mirror is
    // refused before the edge is: otherwise a link lands that the target never learns of, or
    // an unlink leaves a mirror naming a removed edge.
    if mirrored(predicate) {
        let missing: Vec<String> = store_grants(&there)
            .into_iter()
            .filter(|scope| !inv.capability.allows(scope))
            .collect();
        if !missing.is_empty() {
            return Err(Error::Denied(format!(
                "{} a `{name}` link {} the ledger {} also writes its mirror in that ledger's \
                 graph, and this capability does not hold {}. Nothing was changed",
                if sink { "Adding" } else { "Removing" },
                if sink { "into" } else { "from" },
                there.name(),
                missing
                    .iter()
                    .map(|s| format!("`{s}`"))
                    .collect::<Vec<_>>()
                    .join(" or ")
            )));
        }
    }
    let target_client = StoreClient::new(inv, there.clone());
    // The object must be a live item to link to it; an unlink only needs its IRI, because
    // removing an edge to a deleted item is a cleanup.
    // ★ A dated link is no earlier than its target's filing (ledger #2939), read here under the
    // caller's capability. A target this caller cannot read is no live item to it, so the link
    // is refused before the date matters: the bound never falls back to the subject alone.
    let (target, number) = if sink {
        let to = require_item(&target_client, reference).await?;
        if let Some(claimed) = claimed {
            attribution::claimed_not_before_target("created", claimed, &to.iri, to.created)?;
        }
        (to.iri, Some(to.number))
    } else {
        (item_iri(&target_client, reference, "content").await?, None)
    };
    let node = link_iri(&from.iri, predicate, &target);
    let committer = Attribution::committer(inv)?;
    // A link an import principal dated at its source (ledger #2934): the date goes on the link
    // node and its event, marked claimed, and moves `modified` only forward.
    let who = Attribution {
        created: claimed,
        ..committer.clone()
    };
    let (change, said, edit) = if sink {
        let mut triples = format!(
            "<{node}> <{}> <{}> ; <{}> <{}> ; <{}> <{predicate}> ; <{}> <{target}> .",
            v::ext::TYPE,
            v::LINK_CLASS,
            v::ON_ITEM,
            from.iri,
            v::LINK_TYPE,
            v::LINK_TARGET
        );
        triples.push_str(&who.commit(&node, now));
        if let Some(claimed) = claimed {
            triples.push_str(&format!(
                "\n<{node}> <{}> {} .",
                v::ext::CREATED,
                datetime(claimed)
            ));
            triples.push_str(&who.date_sources(&node));
        }
        (
            format!(
                "DELETE {{ {} }} WHERE {{ {} }} ;\n{}",
                client.in_graph(&format!("<{node}> ?p ?o")),
                client.in_graph(&format!("<{node}> ?p ?o")),
                insert_on_live(client, &[&from.iri], &triples)
            ),
            "linked",
            "link",
        )
    } else {
        (
            format!(
                "DELETE {{ {} }} WHERE {{ {} }}",
                client.in_graph(&format!("<{node}> ?p ?o")),
                client.in_graph(&format!("<{node}> ?p ?o"))
            ),
            "unlinked",
            "unlink",
        )
    };
    // The edge, the stamp and the edit event in one update: the event is what moves the
    // subject's version, so the change and its version are one write. It names the other item
    // by its IRI, which the link itself shows this ledger's readers, and nothing else of it.
    let edge = format!("<{node}> <{}> <{}> .", v::ext::TYPE, v::LINK_CLASS);
    let changes = if sink { absent(&edge) } else { edge.clone() };
    let event = edit_event_naming(
        client,
        &from.iri,
        &[&from.iri],
        edit,
        &who,
        now,
        &link_objects(predicate, &target, &changes),
    )?;
    guarded_update(
        client,
        &[&from.iri],
        &batch(&[
            event,
            change,
            touch_dated(client, &from.iri, now, claimed, ""),
        ]),
    )
    .await?;
    sync(
        inv,
        client,
        &from.iri,
        Some((predicate, &target)),
        false,
        now,
    )
    .await?;
    if sink && mirrored(predicate) {
        // The object was deleted while this write was in flight: the mirror did not land, so
        // the edge names nothing, and is taken back.
        if !target_client
            .ask(&format!(
                "ASK {{ {} }}",
                target_client.in_graph(&live(&target))
            ))
            .await?
        {
            // The link's event stands, so the taking back is an unlink beside it: otherwise the
            // history says this item is linked to something it no longer is.
            let event = edit_event_naming(
                client,
                &from.iri,
                &[&from.iri],
                "unlink",
                &committer,
                now,
                &link_objects(predicate, &target, &edge),
            )?;
            client
                .update(&batch(&[
                    event,
                    format!(
                        "DELETE {{ {} }} WHERE {{ {} }}",
                        client.in_graph(&format!("<{node}> ?p ?o")),
                        client.in_graph(&format!("<{node}> ?p ?o"))
                    ),
                ]))
                .await?;
            return Err(Error::NotFound(format!(
                "no ledger item at `{target}`: it was deleted while this link was in flight, \
                 and the link was taken back"
            )));
        }
    }
    let display = number
        .map(|n| there.number(n))
        .unwrap_or_else(|| target.clone());
    if want == JSON {
        let ledger = client.ledger();
        let mut answer = json::answer(ledger, said, json::item_ref(ledger, from.number, &from.iri));
        answer.link_type = Some(name.to_string());
        answer.target = Some(json::ItemRef {
            number,
            display: number.map(|n| there.number(n)),
            iri: target.clone(),
        });
        return Ok(json_repr(json::render(&answer)?));
    }
    Ok(plain(format!("{said} {} {name} {display}\n", from.short())))
}
