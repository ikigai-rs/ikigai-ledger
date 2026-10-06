//! **The JSON face**: `as=application/json` on `items`, `item:{id}` and `next`, and on the
//! answers of `append`, `comment`, `close` and `link`.
//!
//! # ★ Why this exists
//!
//! The plain face is for a human at a REPL, and machines were parsing it: gonk's `roborev`
//! and `kata` bridges read `iri:` lines, status tokens, `blocks:` lines and the `#N <iri>`
//! answer out of it, so any change to how a line is laid out broke them silently. This face
//! is the contract they parse instead, and the plain face stays exactly as it was.
//!
//! # The shape, and how it is versioned
//!
//! Every document is an object carrying `"schema": 1` and `"ledger"`, then its payload. The
//! number is [`SCHEMA`]. **Adding a field does not change it** (a reader ignores what it does
//! not know, and these types deserialize that way); **renaming, removing or retyping a field
//! does**. Field order is fixed (the order below), absent values are `null` rather than
//! missing — except the per-outcome fields of an [`Answer`], listed there — and times are
//! `xsd:dateTime` lexical forms in UTC (`2026-09-15T00:00:00.000Z`), the same strings the
//! Turtle face carries.
//!
//! One item, as `item:{id}` serves it (formatted here; the face is one compact line):
//!
//! ```
//! use ikigai_ledger::json::{ItemDocument, SCHEMA};
//! let served = r##"{
//!   "schema": 1,
//!   "ledger": "default",
//!   "item": {
//!     "number": 12, "display": "#12",
//!     "iri": "urn:iki:ledger:default:item:01m491qbh6e51p14abcdefg",
//!     "kind": null,
//!     "title": "Fix the thing", "body": "It is broken.",
//!     "status": "open", "closed_reason": null,
//!     "priority": 1, "deferred": false,
//!     "labels": ["rust"],
//!     "about": ["urn:repo:file:x/src/a.rs"],
//!     "key": "urn:roborev:finding:0123",
//!     "author": "brian", "revision": null,
//!     "claim": {"holder": "satellite", "purpose": "brief-x"},
//!     "created": "2026-09-15T00:00:00.000Z", "modified": "2026-09-15T00:00:00.000Z",
//!     "links": [{"type": "blocks", "target": {"number": 13, "display": "#13",
//!                "iri": "urn:iki:ledger:default:item:01m491qbh6e51p14abcdefh"}}],
//!     "comments": [{"id": "urn:iki:ledger:default:comment:01m491qbh6e51p14abcdefj",
//!                   "author": "chris", "time": "2026-09-15T00:00:00.000Z",
//!                   "text": "looked at it"}]
//!   }
//! }"##;
//! let doc: ItemDocument = serde_json::from_str(served).unwrap();
//! assert_eq!(doc.schema, SCHEMA);
//! assert_eq!(doc.item.number, 12);
//! assert_eq!(doc.item.key.as_deref(), Some("urn:roborev:finding:0123"));
//! assert_eq!(doc.item.links[0].kind, "blocks");
//! assert_eq!(doc.item.links[0].target.number, Some(13));
//! assert_eq!(doc.item.claim.as_ref().unwrap().holder, "satellite");
//! // A field this version does not know is ignored, which is what makes adding one safe.
//! let newer = served.replacen("\"schema\": 1,", "\"schema\": 1, \"later\": true,", 1);
//! assert!(serde_json::from_str::<ItemDocument>(&newer).is_ok());
//! ```
//!
//! The other documents, field by field: [`ItemsDocument`] (`items`), [`NextDocument`]
//! (`next`) and [`Answer`] (the four writes). `tests/faces.rs` pins each one as a literal.
//!
//! ⚠ **The types are `#[non_exhaustive]`**: read them and deserialize into them, but they
//! cannot be built outside this crate — which is what lets schema 1 gain a field in a patch
//! release without breaking anyone who uses them.

use std::collections::BTreeMap;

use ikigai_core::{Error, Result};
use serde::{Deserialize, Serialize};

use crate::ledger::Ledger;
use crate::model;
use crate::select::{Excluded, Selection};
use crate::sparql;

/// The media type of this face.
pub const MEDIA_TYPE: &str = "application/json";

/// The schema version every document carries. Bumped by a rename, a removal or a retype —
/// never by an addition.
pub const SCHEMA: u32 = 1;

/// A reference to an item: its number, its display form and its IRI.
///
/// `number` and `display` are `null` only for a link whose target is not a live item in
/// this ledger (an edge written out of band, or one that raced a delete).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ItemRef {
    /// The item's number in its ledger.
    pub number: Option<i64>,
    /// `#12` in the default ledger, `acme#12` elsewhere.
    pub display: Option<String>,
    /// The item's IRI — the identity.
    pub iri: String,
}

/// Who holds an item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Claim {
    /// The holder's name.
    pub holder: String,
    /// Why they took it, when they said.
    pub purpose: Option<String>,
}

/// One outbound edge from an item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Link {
    /// `blocks`, `parent` or `related` — the words `urn:iki:ledger:link type=` takes.
    #[serde(rename = "type")]
    pub kind: String,
    /// The item it points at.
    pub target: ItemRef,
}

/// One comment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Comment {
    /// The comment's IRI.
    pub id: String,
    /// Who wrote it, when they said.
    pub author: Option<String>,
    /// When it was written.
    pub time: String,
    /// What it says.
    pub text: String,
}

/// One item, the same object wherever it appears — `item:{id}`, each row of `items`, each
/// ranking and exclusion of `next`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Item {
    /// The item's number in its ledger.
    pub number: i64,
    /// `#12` in the default ledger, `acme#12` elsewhere.
    pub display: String,
    /// The item's IRI — the identity.
    pub iri: String,
    /// The item's level (a subclass IRI of `ledger:Item`), when it has one.
    pub kind: Option<String>,
    /// The first line of what was filed.
    pub title: String,
    /// The rest; `""` when there is none.
    pub body: String,
    /// `open` or `closed`.
    pub status: String,
    /// `done`, `wontfix`, `duplicate`, `superseded` or `audit-no-change`, when closed.
    pub closed_reason: Option<String>,
    /// 0 highest … 4 lowest; `null` is unset, which is not 4.
    pub priority: Option<i64>,
    /// Deliberately not offered by `next`.
    pub deferred: bool,
    /// Free tags, sorted.
    pub labels: Vec<String>,
    /// The resources it is about, sorted.
    pub about: Vec<String>,
    /// The caller's own name for it (`append key=`).
    pub key: Option<String>,
    /// Who filed it.
    pub author: Option<String>,
    /// The revision it was filed against.
    pub revision: Option<String>,
    /// Who holds it, if anyone.
    pub claim: Option<Claim>,
    /// When it was filed.
    pub created: String,
    /// When it last changed.
    pub modified: String,
    /// Outbound edges: `blocks` first, then `parent`, then `related`, each sorted by IRI.
    pub links: Vec<Link>,
    /// Its comments, oldest first.
    pub comments: Vec<Comment>,
}

/// `item:{id}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ItemDocument {
    /// [`SCHEMA`].
    pub schema: u32,
    /// The ledger's name.
    pub ledger: String,
    /// The item.
    pub item: Item,
}

/// An item in the graph that nothing here can read, and what is wrong with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Unreadable {
    /// The subject's IRI.
    pub iri: String,
    /// `missing ledger:number`, `unreadable dcterms:created "…"`, …
    pub defects: Vec<String>,
}

/// `items`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ItemsDocument {
    /// [`SCHEMA`].
    pub schema: u32,
    /// The ledger's name.
    pub ledger: String,
    /// How many items are in `items` (after `limit`).
    pub count: usize,
    /// The items, most recently updated first — the plain face's order.
    pub items: Vec<Item>,
    /// Items in the graph that could not be read and are therefore not in `items` — the
    /// plain face's ⚠ footer, as data.
    pub unreadable: Vec<Unreadable>,
}

/// One ranked candidate of `next`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Ranking {
    /// 1 is the answer.
    pub rank: usize,
    /// The policy's own score — comparable only within one policy.
    pub score: String,
    /// The reasons, in the policy's words.
    pub because: Vec<String>,
    /// The item.
    pub item: Item,
}

/// One open item `next` would not offer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Exclusion {
    /// `claimed`, `deferred` or `blocked`.
    pub why: String,
    /// The plain face's sentence (`blocked by #3`).
    pub reason: String,
    /// The holder, when `why` is `claimed`.
    pub holder: Option<String>,
    /// The open blockers' numbers, when `why` is `blocked`.
    pub blocked_by: Vec<i64>,
    /// The item.
    pub item: Item,
}

/// `next`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct NextDocument {
    /// [`SCHEMA`].
    pub schema: u32,
    /// The ledger's name.
    pub ledger: String,
    /// The policy that ranked it.
    pub policy: String,
    /// What that policy weighs, in order.
    pub weighs: Vec<String>,
    /// When the ranking was computed.
    pub generated_at: String,
    /// How many items were ready before `limit`.
    pub ready: usize,
    /// The ranking, best first, at most `limit` long.
    pub ranking: Vec<Ranking>,
    /// The open items not offered, and why.
    pub excluded: Vec<Exclusion>,
}

/// The answer of `append`, `comment`, `close` or `link`.
///
/// `schema`, `ledger`, `outcome` and `item` are always present. The rest appear only with
/// the outcomes they belong to, and are missing (not `null`) otherwise:
///
/// | `outcome` | also carries |
/// | --- | --- |
/// | `filed` | `status` (`open`), and `key` when the append carried one |
/// | `existing` | `status` (`open`, `closed` or `deleted`), `key` |
/// | `commented` | `comment` |
/// | `closed` | `reason`, and `comment` when a note was given |
/// | `linked`, `unlinked` | `type`, `target` |
///
/// `existing` is the keyed append's "nothing was filed": an item with that key was already
/// in the ledger, and `item` names it. `status: deleted` means it was filed and then deleted
/// — the key stays taken, so a replayed request cannot resurrect it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Answer {
    /// [`SCHEMA`].
    pub schema: u32,
    /// The ledger's name.
    pub ledger: String,
    /// What happened: `filed`, `existing`, `commented`, `closed`, `linked` or `unlinked`.
    pub outcome: String,
    /// The item it happened to.
    pub item: ItemRef,
    /// The item's status after the write (`append` only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// The key the append carried (`append` with `key=` only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// The close reason (`close` only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The link type (`link` only).
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub link_type: Option<String>,
    /// The link's object (`link` only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<ItemRef>,
    /// The comment written (`comment`, and `close` with a note).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<Comment>,
}

// ------------------------------------------------------------------ building them

/// One compact line of JSON and a newline — what every document here is served as.
pub(crate) fn render<T: Serialize>(doc: &T) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(doc)
        .map_err(|e| Error::Endpoint(format!("serializing the JSON face: {e}")))?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// A reference to an item whose number is known.
pub(crate) fn item_ref(ledger: &Ledger, number: i64, iri: &str) -> ItemRef {
    ItemRef {
        number: Some(number),
        display: Some(ledger.number(number)),
        iri: iri.to_string(),
    }
}

/// A comment as this face carries it.
pub(crate) fn comment(comment: &model::Comment) -> Comment {
    Comment {
        id: comment.iri.clone(),
        author: comment.author.clone(),
        time: sparql::iso8601(comment.created),
        text: comment.body.clone(),
    }
}

/// An item as this face carries it. `numbers` names the link targets (from
/// [`model::numbers_of`]); a target missing from it is a reference without a number.
pub(crate) fn item(
    item: &model::Item,
    numbers: &BTreeMap<String, i64>,
    comments: &[model::Comment],
) -> Item {
    let ledger = Ledger::of_subject(&item.iri).unwrap_or_default();
    let target = |iri: &String| ItemRef {
        number: numbers.get(iri).copied(),
        display: numbers.get(iri).map(|n| ledger.number(*n)),
        iri: iri.clone(),
    };
    let links = [
        ("blocks", &item.blocks),
        ("parent", &item.parents),
        ("related", &item.related),
    ]
    .into_iter()
    .flat_map(|(kind, targets)| {
        targets
            .iter()
            .map(move |iri| (kind, iri))
            .collect::<Vec<_>>()
    })
    .map(|(kind, iri)| Link {
        kind: kind.to_string(),
        target: target(iri),
    })
    .collect();
    Item {
        number: item.number,
        display: item.short(),
        iri: item.iri.clone(),
        kind: item.kind.clone(),
        title: item.title.clone(),
        body: item.body.clone(),
        status: if item.open { "open" } else { "closed" }.to_string(),
        closed_reason: item.closed_reason.clone(),
        priority: item.priority,
        deferred: item.deferred,
        labels: item.labels.clone(),
        about: item.about.clone(),
        key: item.key.clone(),
        author: item.author.clone(),
        revision: item.revision.clone(),
        claim: item.claimed_by.as_ref().map(|holder| Claim {
            holder: holder.clone(),
            purpose: item.purpose.clone(),
        }),
        created: sparql::iso8601(item.created),
        modified: sparql::iso8601(item.modified),
        links,
        comments: comments.iter().map(self::comment).collect(),
    }
}

/// Items as this face carries them, with their link targets' numbers and their comments —
/// two queries for the whole set, whatever its size.
pub(crate) async fn items(
    client: &sparql::StoreClient<'_, '_>,
    items: &[&model::Item],
) -> Result<Vec<Item>> {
    let targets = items
        .iter()
        .flat_map(|i| i.blocks.iter().chain(&i.parents).chain(&i.related))
        .cloned()
        .collect();
    let numbers = model::numbers_of(client, &targets).await?;
    let iris: Vec<String> = items.iter().map(|i| i.iri.clone()).collect();
    let comments = model::load_comments_for(client, &iris).await?;
    Ok(items
        .iter()
        .map(|i| {
            item(
                i,
                &numbers,
                comments.get(&i.iri).map(Vec::as_slice).unwrap_or_default(),
            )
        })
        .collect())
}

/// `next`'s document.
pub(crate) async fn next(
    client: &sparql::StoreClient<'_, '_>,
    selection: &Selection,
) -> Result<NextDocument> {
    let all: Vec<&model::Item> = selection
        .ranked
        .iter()
        .map(|(_, item)| item)
        .chain(selection.excluded.iter().map(|(item, _)| item))
        .collect();
    let mut rendered = items(client, &all).await?.into_iter();
    let ranking = selection
        .ranked
        .iter()
        .enumerate()
        .map(|(position, (ranked, _))| Ranking {
            rank: position + 1,
            score: ranked.score.clone(),
            because: ranked.because.clone(),
            item: rendered.next().expect("one rendered item per ranked item"),
        })
        .collect();
    let excluded = selection
        .excluded
        .iter()
        .map(|(_, why)| {
            let (word, holder, blocked_by) = match why {
                Excluded::Claimed(holder) => ("claimed", Some(holder.clone()), Vec::new()),
                Excluded::Deferred => ("deferred", None, Vec::new()),
                Excluded::Blocked(numbers) => ("blocked", None, numbers.clone()),
            };
            Exclusion {
                why: word.to_string(),
                reason: why.reason_in(&selection.ledger),
                holder,
                blocked_by,
                item: rendered
                    .next()
                    .expect("one rendered item per excluded item"),
            }
        })
        .collect();
    Ok(NextDocument {
        schema: SCHEMA,
        ledger: selection.ledger.name().to_string(),
        policy: selection.policy.clone(),
        weighs: selection.weighs.clone(),
        generated_at: sparql::iso8601(selection.generated_at),
        ready: selection.ready_count,
        ranking,
        excluded,
    })
}

/// An answer with only the always-present fields; the caller fills its outcome's own.
pub(crate) fn answer(ledger: &Ledger, outcome: &str, item: ItemRef) -> Answer {
    Answer {
        schema: SCHEMA,
        ledger: ledger.name().to_string(),
        outcome: outcome.to_string(),
        item,
        status: None,
        key: None,
        reason: None,
        link_type: None,
        target: None,
        comment: None,
    }
}
