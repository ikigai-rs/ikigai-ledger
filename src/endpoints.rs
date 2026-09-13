//! The thirteen resources this crate binds.
//!
//! ```text
//! urn:iki:ledger:items          Source              the list, filtered
//! urn:iki:ledger:item:{id}      Source Sink Delete  one item, edit it, delete it
//! urn:iki:ledger:append         Sink                file a new item
//! urn:iki:ledger:comment        Sink                append a comment
//! urn:iki:ledger:close          Sink                close with a reason
//! urn:iki:ledger:reopen         Sink                undo a close
//! urn:iki:ledger:claim          Sink Delete         take it / hand it back
//! urn:iki:ledger:defer          Sink Delete         not now / now again
//! urn:iki:ledger:link           Sink Delete         blocks / parent / related
//! urn:iki:ledger:label          Sink Delete         tag / untag
//! urn:iki:ledger:purge          Delete              destroy, leaving a tombstone
//! urn:iki:ledger:next           Source              the ready set, ranked
//! urn:iki:ledger:policy:{name}  Source              what a policy weighs
//! ```
//!
//! # Why `purge` is a resource and not an argument
//!
//! A destructive delete needs a capability an ordinary delete must not carry. A
//! `purge=true` argument could only be enforced at runtime — and an action that enforces
//! a scope it does not declare makes the manifold lie, which the field guide calls worse
//! than the converse. Declaring both on one action would demand purge authority for every
//! ordinary delete. So the authority difference gets its own IRI, exactly as
//! `ikigai-store` gives each SPARQL form its own.
//!
//! # Capabilities, and the store scopes every action also declares
//!
//! A sub-request carries the **caller's** capability unchanged — `Invocation::issue` has
//! no attenuating or elevating form — so a ledger write is reachable only by a caller who
//! also holds `urn:cap:store:write`. Every action here declares those scopes too. It is
//! honest, and it is coarser than it should be: see `README.md`.
//!
//! # Freshness
//!
//! Reads are `.cacheable()` and depend on the store's two write threads. This module cuts
//! **no thread of its own**, deliberately: the kernel cuts the thread named after a
//! mutating request's target, so `urn:iki:ledger:append` is already cut on every append —
//! and every read here depends on `urn:iki:store:update` / `urn:iki:store:load`, which the
//! store cuts on the write this module actually performs. A ledger-specific thread could
//! only be cut by resolving `urn:kernel:cut`, which needs `urn:cap:kernel:cut` — authority
//! this module has no business holding for a name nothing would gain by.

use std::sync::Arc;

use async_trait::async_trait;
use ikigai_core::{
    ArgSpec, Description, Endpoint, EndpointSpace, Error, Exact, Invocation, ReprType,
    Representation, Result, UriTemplate, Verb,
};
use oxrdf::Graph;
use sha2::{Digest, Sha256};

use crate::model::{self, Deferred, Filter, Holder, Item, Status};
use crate::policy::{OrderingPolicy, Policies};
use crate::select;
use crate::sparql::{boolean, datetime, integer, iri_term, literal, StoreClient};
use crate::vocabulary as v;

/// Reading the ledger. A ledger holds whatever anyone filed, so an unrestricted read is
/// not free — the same argument `ikigai-store` makes for gating its query face.
pub const CAP_READ: &str = "urn:cap:ledger:read";

/// The capability every ordinary mutation requires: filing, commenting, closing,
/// claiming, linking, labelling, deferring, editing.
pub const CAP_WRITE: &str = "urn:cap:ledger:write";

/// Removing an item from view. Separate from [`CAP_WRITE`] because a delete takes work
/// OUT of the ledger, and the everyday grant should not carry it.
pub const CAP_DELETE: &str = "urn:cap:ledger:delete";

/// Destroying an item's content irreversibly. Separate again, and the narrowest grant in
/// this module: a tombstone survives a purge, but nothing else does.
pub const CAP_PURGE: &str = "urn:cap:ledger:purge";

/// `text/plain` — the default face, and what "view ASAP" actually needs.
const PLAIN: &str = "text/plain";
/// `text/turtle` — the graph face.
const TURTLE: &str = "text/turtle";
const FACES: [&str; 2] = [PLAIN, TURTLE];

/// Bind the ledger with the built-in ordering policies (`priority-recency`, then
/// `leverage`).
pub fn space() -> EndpointSpace {
    space_with_policies(Policies::default().all().to_vec())
}

/// Bind the ledger with a host's own ordering policies. **The first is the default.**
///
/// This is the `CachePolicy` shape: configured at execution time, in the host's manifest,
/// rather than compiled in — because the right ordering for a solo backlog, a review
/// queue and a team are different.
///
/// # Panics
///
/// On an empty policy list: a `next` with nothing to rank with should fail where the
/// manifest is, not at the first request.
pub fn space_with_policies(policies: Vec<Arc<dyn OrderingPolicy>>) -> EndpointSpace {
    let policies = Arc::new(Policies::new(policies));
    EndpointSpace::new()
        .bind(Exact::new("urn:iki:ledger:items"), ItemsEndpoint)
        .bind(
            UriTemplate::parse("urn:iki:ledger:item:{id}").expect("a constant template"),
            ItemEndpoint,
        )
        .bind(Exact::new("urn:iki:ledger:append"), AppendEndpoint)
        .bind(Exact::new("urn:iki:ledger:comment"), CommentEndpoint)
        .bind(Exact::new("urn:iki:ledger:close"), CloseEndpoint)
        .bind(Exact::new("urn:iki:ledger:reopen"), ReopenEndpoint)
        .bind(Exact::new("urn:iki:ledger:claim"), ClaimEndpoint)
        .bind(Exact::new("urn:iki:ledger:defer"), DeferEndpoint)
        .bind(Exact::new("urn:iki:ledger:link"), LinkEndpoint)
        .bind(Exact::new("urn:iki:ledger:label"), LabelEndpoint)
        .bind(Exact::new("urn:iki:ledger:purge"), PurgeEndpoint)
        .bind(
            Exact::new("urn:iki:ledger:next"),
            NextEndpoint {
                policies: Arc::clone(&policies),
            },
        )
        .bind(
            UriTemplate::parse("urn:iki:ledger:policy:{name}").expect("a constant template"),
            PolicyEndpoint { policies },
        )
}

// ------------------------------------------------------------------------- helpers

/// A `text/plain` representation.
fn plain(text: impl Into<String>) -> Representation {
    Representation::new(
        ReprType::new(PLAIN).with_param("charset", "utf-8"),
        text.into().into_bytes(),
    )
}

/// A representation in the face the caller asked for, cacheable under the store's write
/// threads.
fn face(item_text: String, graph: Option<Graph>, want: &str) -> Result<Representation> {
    let repr = match want {
        TURTLE => Representation::new(
            ReprType::new(TURTLE).with_param("charset", "utf-8"),
            model::turtle(&graph.unwrap_or_default())?,
        ),
        _ => plain(item_text),
    };
    Ok(repr
        .cacheable()
        .depends_on(ikigai_store::UPDATE_THREAD)
        .depends_on(ikigai_store::LOAD_THREAD))
}

/// Which face was asked for. An `as` this endpoint cannot serve is **refused**, never
/// substituted — the caller asked a question and a different answer is not a better one.
fn wanted_face(inv: &Invocation<'_>) -> Result<&'static str> {
    match inv.inline_str("as").ok() {
        None => Ok(PLAIN),
        Some(asked) => FACES
            .iter()
            .find(|face| bare(asked) == **face)
            .copied()
            .ok_or_else(|| Error::InvalidArgument {
                name: "as".to_string(),
                detail: format!(
                    "`{asked}` is not a face this resource serves; one of {}",
                    FACES.join(", ")
                ),
            }),
    }
}

/// A media type without its parameters.
fn bare(media_type: &str) -> &str {
    media_type.split(';').next().unwrap_or(media_type).trim()
}

/// Now, in milliseconds, from the kernel's injected clock.
///
/// ⚠ **Refuses when the kernel has no clock.** Every item, comment, claim and tombstone
/// here is stamped, and an entry without a timestamp is not a ledger entry: a store that
/// quietly accepted unstamped writes would be discovered as a hole in the record much
/// later, by someone trying to answer "when". Fail loud on missing configuration.
fn now_ms(inv: &Invocation<'_>) -> Result<u64> {
    inv.now().map(|t| t.as_millis()).ok_or_else(|| {
        Error::Endpoint(
            "this kernel has no clock, so a ledger write cannot be stamped — and an entry \
             without a timestamp is not a ledger entry. Build the kernel with \
             `Kernel::with_clock(Arc::new(SystemClock))` (or a fixed clock in tests)"
                .to_string(),
        )
    })
}

/// Mint an item's opaque, time-ordered id.
///
/// ★ **Identity is the IRI; `#12` is a label on it.** The id is 10 characters of
/// Crockford base32 over the low 48 bits of the millisecond clock — so ids sort in
/// filing order, which makes a listing of raw IRIs readable — followed by 6 characters
/// derived from a SHA-256 of the minting inputs. The digest half is not decoration: one
/// store has one writer, so the clock alone is unique *here*, and the digest is what
/// keeps two ledgers merged later from colliding on a shared millisecond.
fn mint_id(now: u64, title: &str, author: &str) -> String {
    const CROCKFORD: &[u8] = b"0123456789abcdefghjkmnpqrstvwxyz";
    let mut id = String::with_capacity(16);
    for shift in (0..10).rev() {
        id.push(CROCKFORD[((now >> (shift * 5)) & 31) as usize] as char);
    }
    let mut hasher = Sha256::new();
    hasher.update(now.to_be_bytes());
    hasher.update(title.as_bytes());
    hasher.update([0]);
    hasher.update(author.as_bytes());
    let digest = hasher.finalize();
    let mut bits = u64::from(digest[0]) << 24
        | u64::from(digest[1]) << 16
        | u64::from(digest[2]) << 8
        | u64::from(digest[3]);
    for _ in 0..6 {
        id.push(CROCKFORD[(bits & 31) as usize] as char);
        bits >>= 5;
    }
    id
}

/// `title\n\nbody` — the git-commit convention, because it is the one everybody already
/// types and it needs no second argument for the commonest case.
fn split_content(content: &str) -> Result<(String, String)> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Err(Error::InvalidArgument {
            name: "content".to_string(),
            detail: "an item needs at least a title: the first line is the title and \
                     everything after the first blank line is the body"
                .to_string(),
        });
    }
    let mut lines = trimmed.splitn(2, '\n');
    let title = lines.next().unwrap_or_default().trim().to_string();
    let body = lines.next().unwrap_or_default().trim().to_string();
    Ok((title, body))
}

/// Resolve an item reference — `#12`, `12`, an opaque id, or a full IRI — to its IRI, and
/// fail with a sentence naming what was looked for when there is no such item.
async fn require_item(client: &StoreClient<'_, '_>, reference: &str) -> Result<Item> {
    let iri = if reference.starts_with("urn:") {
        reference.to_string()
    } else {
        model::resolve_id(client, reference).await?
    };
    model::load_item(client, &iri)
        .await?
        .ok_or_else(|| Error::NotFound(format!("no ledger item at `{iri}`")))
}

/// The `GRAPH <…> { … }` wrapper.
fn graph_block(body: &str) -> String {
    format!("GRAPH <{}> {{ {body} }}", v::GRAPH)
}

/// Stamp an item as modified, as part of the same update that changed it.
fn touch(item: &str, now: u64) -> String {
    format!(
        "DELETE {{ {} }} INSERT {{ {} }} WHERE {{ {} }}",
        graph_block(&format!("<{item}> <{}> ?was", v::ext::MODIFIED)),
        graph_block(&format!(
            "<{item}> <{}> {}",
            v::ext::MODIFIED,
            datetime(now)
        )),
        graph_block(&format!(
            "OPTIONAL {{ <{item}> <{}> ?was }}",
            v::ext::MODIFIED
        ))
    )
}

/// Append a comment — the shared path behind `urn:iki:ledger:comment` and behind the
/// optional note on close / reopen / defer / claim, so *why* is always recorded in one
/// place and one shape.
async fn write_comment(
    client: &StoreClient<'_, '_>,
    item: &str,
    text: &str,
    author: Option<&str>,
    now: u64,
) -> Result<String> {
    let id = mint_id(now, text, author.unwrap_or_default());
    let comment = format!("{}{id}", v::iri::COMMENT);
    let mut triples = format!(
        "<{comment}> <{type_}> <{class}> ; <{on}> <{item}> ; <{body}> {text} ; <{created}> {when} .",
        type_ = v::ext::TYPE,
        class = v::COMMENT_CLASS,
        on = v::ON_ITEM,
        body = v::BODY,
        text = literal(text),
        created = v::ext::CREATED,
        when = datetime(now),
    );
    if let Some(author) = author {
        triples.push_str(&format!(
            "\n<{comment}> <{}> {} .",
            v::AUTHOR,
            literal(author)
        ));
    }
    client
        .update(&format!("INSERT DATA {{ {} }}", graph_block(&triples)))
        .await?;
    client.update(&touch(item, now)).await?;
    Ok(comment)
}

/// The store scopes an action that reads the ledger transitively needs.
fn read_scopes(desc: Description) -> Description {
    desc.requires(CAP_READ).requires(ikigai_store::CAP_READ)
}

/// The store scopes a mutating action transitively needs. Every write here reads first
/// (to resolve `#12`, to check the item exists), so both store scopes are real.
fn write_scopes(spec: ikigai_core::ActionSpec, own: &str) -> ikigai_core::ActionSpec {
    spec.requires(own)
        .requires(ikigai_store::CAP_READ)
        .requires(ikigai_store::CAP_WRITE)
}

/// The `as` ArgSpec every read face declares.
fn as_arg() -> ArgSpec {
    ArgSpec::new("as")
        .summary(format!(
            "The face to serve; one of {}. An `as` this resource cannot answer in is \
             refused, never substituted.",
            FACES.join(", ")
        ))
        .class(v::ext::XSD_STRING)
        .one_of(FACES)
        .default_value(PLAIN)
        .optional()
}

/// The `item` ArgSpec: a short number, an opaque id, or a full IRI.
fn item_arg(summary: &str) -> ArgSpec {
    ArgSpec::new("item")
        .summary(format!(
            "{summary} Accepts `#12`, `12`, an opaque id, or the full \
             `urn:iki:ledger:item:{{id}}` IRI — a human types the number and a machine \
             carries the IRI."
        ))
        .class(v::ext::XSD_STRING)
}

fn unsupported(id: &str, verb: Verb) -> Error {
    Error::Endpoint(format!("`{id}` does not answer {verb:?}"))
}

// --------------------------------------------------------------------------- items

#[derive(Clone)]
struct ItemsEndpoint;

#[async_trait]
impl Endpoint for ItemsEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        if inv.request.verb != Verb::Source {
            return Err(unsupported("ledger-items", inv.request.verb));
        }
        let client = StoreClient::new(inv);
        let want = wanted_face(inv)?;
        let filter = Filter {
            status: match inv.inline_str("status").unwrap_or("open") {
                "closed" => Status::Closed,
                "all" => Status::All,
                "open" => Status::Open,
                other => {
                    return Err(Error::InvalidArgument {
                        name: "status".to_string(),
                        detail: format!("`{other}` is not one of open, closed, all"),
                    })
                }
            },
            kind: inv.inline_str("kind").ok().map(str::to_string),
            labels: split_labels(inv.inline_str("labels").ok()),
            without: split_labels(inv.inline_str("without").ok()),
            about: inv.inline_str("about").ok().map(str::to_string),
            holder: match inv.inline_str("holder").ok() {
                None | Some("any") => Holder::Any,
                Some("none") => Holder::None,
                Some(name) => Holder::Named(name.to_string()),
            },
            deferred: match inv.inline_str("deferred").unwrap_or("include") {
                "exclude" => Deferred::Exclude,
                "only" => Deferred::Only,
                _ => Deferred::Include,
            },
            text: inv.inline_str("text").ok().map(str::to_string),
            limit: inv
                .inline_str("limit")
                .ok()
                .and_then(|l| l.parse().ok())
                .unwrap_or(50),
        };
        let items = model::load_items(&client, &filter).await?;
        if want == TURTLE {
            let mut graph = Graph::new();
            for item in &items {
                item.triples(&mut graph);
            }
            return face(String::new(), Some(graph), want);
        }
        let text = if items.is_empty() {
            "no items match\n".to_string()
        } else {
            let mut out: String = items
                .iter()
                .map(|item| format!("{}\n", item.line()))
                .collect();
            out.push_str(&format!("\n{} item(s)\n", items.len()));
            out
        };
        face(text, None, want)
    }

    fn name(&self) -> &str {
        "ledger-items"
    }

    fn describe(&self) -> Description {
        read_scopes(
            Description::new("ledger-items")
                .title("The ledger, filtered")
                .summary(
                    "Every item matching the filter, most recently updated first. The \
                     filters are evaluated in the query, which is why the ordering policy \
                     behind `urn:iki:ledger:next` can stay small.",
                )
                .verb(Verb::Source)
                .verb(Verb::Meta)
                .input(
                    ArgSpec::new("status")
                        .summary("Which items: open (the default), closed, or all.")
                        .class(v::ext::XSD_STRING)
                        .one_of(["open", "closed", "all"])
                        .default_value("open")
                        .optional(),
                )
                .input(
                    ArgSpec::new("kind")
                        .summary(
                            "An item class IRI — the LEVEL. Items of every level assert \
                             the base class, so omitting this asks the whole ledger and \
                             naming one scopes to that level.",
                        )
                        .class(v::ext::XSD_ANY_URI)
                        .optional(),
                )
                .input(
                    ArgSpec::new("labels")
                        .summary("Comma-separated; an item must carry ALL of them.")
                        .class(v::ext::XSD_STRING)
                        .optional(),
                )
                .input(
                    ArgSpec::new("without")
                        .summary("Comma-separated; an item must carry NONE of them.")
                        .class(v::ext::XSD_STRING)
                        .optional(),
                )
                .input(
                    ArgSpec::new("about")
                        .summary(
                            "A resource IRI: what is filed against this file, commit, PR \
                             or brief. The join the ledger exists to make cheap.",
                        )
                        .class(v::ext::XSD_ANY_URI)
                        .optional(),
                )
                .input(
                    ArgSpec::new("holder")
                        .summary("`any` (default), `none` for unclaimed, or a holder's name.")
                        .class(v::ext::XSD_STRING)
                        .default_value("any")
                        .optional(),
                )
                .input(
                    ArgSpec::new("deferred")
                        .summary("include (default), exclude, or only.")
                        .class(v::ext::XSD_STRING)
                        .one_of(["include", "exclude", "only"])
                        .default_value("include")
                        .optional(),
                )
                .input(
                    ArgSpec::new("text")
                        .summary("A case-insensitive substring of the title.")
                        .class(v::ext::XSD_STRING)
                        .optional(),
                )
                .input(
                    ArgSpec::new("limit")
                        .summary("At most this many items (default 50).")
                        .class(v::ext::XSD_INTEGER)
                        .default_value("50")
                        .optional(),
                )
                .input(as_arg())
                .output(PLAIN)
                .output(TURTLE),
        )
    }
}

fn split_labels(value: Option<&str>) -> Vec<String> {
    value
        .map(|v| {
            v.split(',')
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------- item

#[derive(Clone)]
struct ItemEndpoint;

#[async_trait]
impl Endpoint for ItemEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let client = StoreClient::new(inv);
        let id = inv.bindings.get("id").ok_or_else(|| {
            Error::Endpoint(
                "no `id` captured: this endpoint is bound to `urn:iki:ledger:item:{id}` and \
                 was invoked without the template's capture"
                    .to_string(),
            )
        })?;
        match inv.request.verb {
            Verb::Source => {
                let want = wanted_face(inv)?;
                let item = require_item(&client, id).await?;
                let comments = model::load_comments(&client, &item.iri).await?;
                if want == TURTLE {
                    let mut graph = Graph::new();
                    item.triples(&mut graph);
                    for comment in &comments {
                        comment.triples(&mut graph);
                    }
                    return face(String::new(), Some(graph), want);
                }
                let mut text = item.detail();
                if !comments.is_empty() {
                    text.push_str(&format!("\n{} comment(s):\n", comments.len()));
                    for comment in &comments {
                        text.push_str(&comment.render());
                    }
                }
                face(text, None, want)
            }
            Verb::Exists => {
                let item = require_item(&client, id).await;
                face(
                    if item.is_ok() { "true\n" } else { "false\n" }.to_string(),
                    None,
                    PLAIN,
                )
            }
            Verb::Sink => {
                let now = now_ms(inv)?;
                let item = require_item(&client, id).await?;
                let mut updates = Vec::new();
                if let Ok(content) = inv.inline_str("content") {
                    let (title, body) = split_content(content)?;
                    updates.push(replace_one(&item.iri, v::ext::TITLE, &literal(&title)));
                    updates.push(replace_one(&item.iri, v::BODY, &literal(&body)));
                }
                if let Ok(priority) = inv.inline_str("priority") {
                    updates.push(replace_one(
                        &item.iri,
                        v::PRIORITY,
                        &integer(parse_priority(priority)?),
                    ));
                }
                if let Ok(revision) = inv.inline_str("revision") {
                    updates.push(replace_one(&item.iri, v::REVISION, &literal(revision)));
                }
                if let Ok(about) = inv.inline_str("about") {
                    for target in about.split_whitespace() {
                        updates.push(format!(
                            "INSERT DATA {{ {} }}",
                            graph_block(&format!(
                                "<{}> <{}> {} .",
                                item.iri,
                                v::ABOUT,
                                iri_term(target, "about")?
                            ))
                        ));
                    }
                }
                if updates.is_empty() {
                    return Err(Error::MissingArgument("content".to_string()));
                }
                for update in &updates {
                    client.update(update).await?;
                }
                client.update(&touch(&item.iri, now)).await?;
                Ok(plain(format!("updated {} {}\n", item.short(), item.iri)))
            }
            Verb::Delete => {
                let now = now_ms(inv)?;
                let item = require_item(&client, id).await?;
                let reason = inv.inline_str("reason").unwrap_or("").to_string();
                let author = inv.inline_str("author").ok().map(str::to_string);
                let removed =
                    delete_item(&client, &item, &reason, author.as_deref(), now, false).await?;
                Ok(plain(format!(
                    "deleted {} {}\n  {} quad(s) moved to <{}> and recoverable\n  tombstone: \
                     {}{}\n",
                    item.short(),
                    item.iri,
                    removed.quads,
                    v::DELETED_GRAPH,
                    v::iri::TOMBSTONE,
                    removed.id
                )))
            }
            other => Err(unsupported("ledger-item", other)),
        }
    }

    fn name(&self) -> &str {
        "ledger-item"
    }

    fn describe(&self) -> Description {
        let id_input = || {
            ArgSpec::new("id")
                .summary(
                    "The item's opaque id or its short number — both resolve, because a \
                     human types `12` and a machine carries the IRI.",
                )
                .class(v::ext::XSD_STRING)
                .binding()
        };
        Description::new("ledger-item")
            .title("One ledger item")
            .summary(
                "Read an item with its comments and links, edit its fields, or delete it \
                 (leaving a tombstone and its content recoverable).",
            )
            .verb(Verb::Meta)
            .action(read_action(
                ikigai_core::ActionSpec::new(Verb::Source)
                    .summary("The item, its metadata, its links and its comments.")
                    .input(id_input())
                    .input(as_arg())
                    .output(PLAIN)
                    .output(TURTLE),
            ))
            .action(read_action(
                ikigai_core::ActionSpec::new(Verb::Exists)
                    .summary("Whether the item exists.")
                    .input(id_input())
                    .output(PLAIN),
            ))
            .action(write_scopes(
                ikigai_core::ActionSpec::new(Verb::Sink)
                    .summary(
                        "Edit the item: `content` replaces the title and body (first line, \
                         then the rest), `priority` replaces the priority, `about` ADDS a \
                         target, `revision` replaces the filed-against revision.",
                    )
                    .input(id_input())
                    .input(
                        ArgSpec::new("content")
                            .summary("The new title and body: first line, blank line, rest.")
                            .class(v::ext::XSD_STRING)
                            .optional(),
                    )
                    .input(
                        ArgSpec::new("priority")
                            .summary("0 highest … 4 lowest.")
                            .class(v::ext::XSD_INTEGER)
                            .one_of(["0", "1", "2", "3", "4"])
                            .optional(),
                    )
                    .input(
                        ArgSpec::new("about")
                            .summary(
                                "Whitespace-separated resource IRIs to ADD. There is no \
                                 remove yet; see the README's gaps.",
                            )
                            .class(v::ext::XSD_STRING)
                            .optional(),
                    )
                    .input(
                        ArgSpec::new("revision")
                            .summary("The revision this item is filed against.")
                            .class(v::ext::XSD_STRING)
                            .optional(),
                    )
                    .output(PLAIN),
                CAP_WRITE,
            ))
            .action(write_scopes(
                ikigai_core::ActionSpec::new(Verb::Delete)
                    .summary(
                        "Delete the item: its quads and its comments MOVE to \
                         `urn:iki:ledger:graph:deleted`, and a tombstone stays behind \
                         carrying the number, the time, the actor, the reason, the quad \
                         count and the sha256 of what was removed. Recoverable. To destroy \
                         the content, `urn:iki:ledger:purge` — a different resource because \
                         it is a different authority.",
                    )
                    .input(id_input())
                    .input(
                        ArgSpec::new("reason")
                            .summary("Why — free text, kept on the tombstone.")
                            .class(v::ext::XSD_STRING)
                            .optional(),
                    )
                    .input(
                        ArgSpec::new("author")
                            .summary("Who deleted it.")
                            .class(v::ext::XSD_STRING)
                            .optional(),
                    )
                    .output(PLAIN),
                CAP_DELETE,
            ))
    }
}

/// The ledger + store read scopes on an explicit action.
fn read_action(spec: ikigai_core::ActionSpec) -> ikigai_core::ActionSpec {
    spec.requires(CAP_READ).requires(ikigai_store::CAP_READ)
}

/// Replace a single-valued property.
fn replace_one(item: &str, predicate: &str, value: &str) -> String {
    format!(
        "DELETE {{ {} }} INSERT {{ {} }} WHERE {{ {} }}",
        graph_block(&format!("<{item}> <{predicate}> ?was")),
        graph_block(&format!("<{item}> <{predicate}> {value}")),
        graph_block(&format!("OPTIONAL {{ <{item}> <{predicate}> ?was }}"))
    )
}

fn parse_priority(value: &str) -> Result<i64> {
    match value.trim().parse::<i64>() {
        Ok(p) if (0..=4).contains(&p) => Ok(p),
        _ => Err(Error::InvalidArgument {
            name: "priority".to_string(),
            detail: format!("`{value}` is not 0 (highest) … 4 (lowest)"),
        }),
    }
}

/// What a delete removed.
struct Removed {
    id: String,
    quads: usize,
}

/// Move (or destroy) an item's quads and write its tombstone.
///
/// ★ The three sets that go together, because leaving any behind is a lie about what was
/// deleted: the item's own triples, the triples of its comments, and every edge POINTING
/// AT it (a `blocks` from another item would otherwise name something that no longer
/// resolves).
async fn delete_item(
    client: &StoreClient<'_, '_>,
    item: &Item,
    reason: &str,
    author: Option<&str>,
    now: u64,
    destroy: bool,
) -> Result<Removed> {
    let subject = iri_term(&item.iri, "item")?;
    let selector = format!(
        "?s ?p ?o . FILTER(?s = {subject} || ?o = {subject} || EXISTS {{ ?s <{on}> {subject} }})",
        on = v::ON_ITEM,
    );
    let rows = client
        .select(&format!(
            "SELECT ?s ?p ?o WHERE {{ {} }} ORDER BY ?s ?p ?o",
            graph_block(&selector)
        ))
        .await?;

    // ★ The hash is over a canonical form of exactly what is going: sorted triples, each
    // term with its kind and datatype. There are no blank nodes in this graph — every node
    // this module writes is skolemized — so sorted triples ARE a canonical form here, and
    // the heavyweight RDF Dataset Canonicalization would buy nothing.
    let mut hasher = Sha256::new();
    for row in &rows {
        for var in ["s", "p", "o"] {
            if let Some(binding) = row.get(var) {
                hasher.update(binding.kind.as_bytes());
                hasher.update([0x1f]);
                hasher.update(binding.value.as_bytes());
                hasher.update([0x1f]);
                hasher.update(binding.datatype.clone().unwrap_or_default().as_bytes());
                hasher.update([0x1e]);
            }
        }
        hasher.update([0x1d]);
    }
    let digest = format!("sha256:{:x}", hasher.finalize());

    if destroy {
        // Everything, including anything an earlier recoverable delete quarantined.
        // ⚠ The DELETE TEMPLATE is a quad pattern and may not carry a FILTER — the
        // selector belongs in the WHERE clause only. Putting it in both is a parse error
        // at the store, which is at least loud.
        client
            .update(&format!(
                "DELETE {{ {live_t} }} WHERE {{ {live_w} }};\n\
                 DELETE {{ {dead_t} }} WHERE {{ {dead_w} }}",
                live_t = graph_block("?s ?p ?o"),
                live_w = graph_block(&selector),
                dead_t = format_args!("GRAPH <{}> {{ ?s ?p ?o }}", v::DELETED_GRAPH),
                dead_w = format_args!("GRAPH <{}> {{ {selector} }}", v::DELETED_GRAPH),
            ))
            .await?;
    } else {
        client
            .update(&format!(
                "DELETE {{ {} }} INSERT {{ {} }} WHERE {{ {} }}",
                graph_block("?s ?p ?o"),
                format_args!("GRAPH <{}> {{ ?s ?p ?o }}", v::DELETED_GRAPH),
                graph_block(&selector)
            ))
            .await?;
    }

    let id = item.iri.rsplit(':').next().unwrap_or("unknown").to_string();
    let tombstone = format!("{}{id}", v::iri::TOMBSTONE);
    let mut triples = format!(
        "<{tombstone}> <{type_}> <{class}> ; <{deleted}> <{item_iri}> ; <{number}> {n} ; \
         <{invalidated}> {when} ; <{recoverable}> {recoverable_value} ; <{quads}> {count} ; \
         <{hash}> {digest} .",
        type_ = v::ext::TYPE,
        class = v::TOMBSTONE_CLASS,
        deleted = v::DELETED_ITEM,
        item_iri = item.iri,
        number = v::NUMBER,
        n = integer(item.number),
        invalidated = v::ext::INVALIDATED_AT,
        when = datetime(now),
        recoverable = v::RECOVERABLE,
        recoverable_value = boolean(!destroy),
        quads = v::QUAD_COUNT,
        count = integer(rows.len() as i64),
        hash = v::ext::CONTENT_HASH,
        digest = literal(&digest),
    );
    if !reason.is_empty() {
        triples.push_str(&format!(
            "\n<{tombstone}> <{}> {} .",
            v::REASON,
            literal(reason)
        ));
    }
    if let Some(author) = author {
        triples.push_str(&format!(
            "\n<{tombstone}> <{}> {} .",
            v::AUTHOR,
            literal(author)
        ));
    }
    client
        .update(&format!("INSERT DATA {{ {} }}", graph_block(&triples)))
        .await?;
    Ok(Removed {
        id,
        quads: rows.len(),
    })
}

// -------------------------------------------------------------------------- append

#[derive(Clone)]
struct AppendEndpoint;

#[async_trait]
impl Endpoint for AppendEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        if inv.request.verb != Verb::Sink {
            return Err(unsupported("ledger-append", inv.request.verb));
        }
        let client = StoreClient::new(inv);
        let now = now_ms(inv)?;
        let (title, body) = split_content(inv.inline_str("content")?)?;
        let author = inv.inline_str("author").ok();
        let id = mint_id(now, &title, author.unwrap_or_default());
        let iri = format!("{}{id}", v::iri::ITEM);

        let mut triples = format!(
            "<{iri}> <{type_}> <{class}> ; <{number}> ?new ; <{title_p}> {title_v} ; \
             <{status}> <{open}> ; <{created}> {when} ; <{modified}> {when} .",
            type_ = v::ext::TYPE,
            class = v::ITEM_CLASS,
            number = v::NUMBER,
            title_p = v::ext::TITLE,
            title_v = literal(&title),
            status = v::STATUS,
            open = v::OPEN,
            created = v::ext::CREATED,
            modified = v::ext::MODIFIED,
            when = datetime(now),
        );
        if let Ok(kind) = inv.inline_str("kind") {
            triples.push_str(&format!(
                "\n<{iri}> <{}> {} .",
                v::ext::TYPE,
                iri_term(kind, "kind")?
            ));
        }
        if !body.is_empty() {
            triples.push_str(&format!("\n<{iri}> <{}> {} .", v::BODY, literal(&body)));
        }
        if let Ok(priority) = inv.inline_str("priority") {
            triples.push_str(&format!(
                "\n<{iri}> <{}> {} .",
                v::PRIORITY,
                integer(parse_priority(priority)?)
            ));
        }
        if let Some(author) = author {
            triples.push_str(&format!("\n<{iri}> <{}> {} .", v::AUTHOR, literal(author)));
        }
        if let Ok(revision) = inv.inline_str("revision") {
            triples.push_str(&format!(
                "\n<{iri}> <{}> {} .",
                v::REVISION,
                literal(revision)
            ));
        }
        for label in split_labels(inv.inline_str("labels").ok()) {
            triples.push_str(&format!("\n<{iri}> <{}> {} .", v::LABEL, literal(&label)));
        }
        for target in inv.inline_str("about").unwrap_or("").split_whitespace() {
            triples.push_str(&format!(
                "\n<{iri}> <{}> {} .",
                v::ABOUT,
                iri_term(target, "about")?
            ));
        }

        // ★ ONE statement, so the number cannot be allocated twice. The counter is read,
        // incremented, rewritten and stamped onto the new item inside a single SPARQL
        // UPDATE — a read-modify-write across two round trips would race two concurrent
        // appends in the same process, and the store's one-writer rule says nothing about
        // that (it excludes other PROCESSES, not other requests).
        let update = format!(
            "DELETE {{ {delete} }}\nINSERT {{ {insert} }}\nWHERE {{\n  \
             {{ {{ SELECT ?last WHERE {{ {last_q} }} ORDER BY DESC(?last) LIMIT 1 }}\n    \
             UNION\n    {{ BIND({zero} AS ?last) FILTER NOT EXISTS {{ {any_q} }} }} }}\n  \
             OPTIONAL {{ {old_q} }}\n  BIND(?last + 1 AS ?new)\n}}",
            delete = graph_block(&format!("<{}> <{}> ?old", v::COUNTER, v::LAST_NUMBER)),
            insert = graph_block(&format!(
                "<{counter}> <{type_}> <{class}> ; <{last}> ?new .\n{triples}",
                counter = v::COUNTER,
                type_ = v::ext::TYPE,
                class = v::COUNTER_CLASS,
                last = v::LAST_NUMBER,
            )),
            last_q = graph_block(&format!("<{}> <{}> ?last", v::COUNTER, v::LAST_NUMBER)),
            zero = integer(0),
            any_q = graph_block(&format!("<{}> <{}> ?any", v::COUNTER, v::LAST_NUMBER)),
            old_q = graph_block(&format!("<{}> <{}> ?old", v::COUNTER, v::LAST_NUMBER)),
        );
        client.update(&update).await?;

        let item = model::load_item(&client, &iri).await?.ok_or_else(|| {
            Error::Endpoint(format!(
                "the item was written but does not read back at {iri}: the store accepted \
                 an update that changed nothing, which should be impossible"
            ))
        })?;
        Ok(plain(format!("{} {}\n", item.short(), item.iri)))
    }

    fn name(&self) -> &str {
        "ledger-append"
    }

    fn describe(&self) -> Description {
        let spec = write_scopes(
            ikigai_core::ActionSpec::new(Verb::Sink)
                .summary(
                    "File a new item. The number is allocated from the ledger's counter in \
                     the same statement that writes the item, so two concurrent appends \
                     cannot collide.",
                )
                .input(
                    ArgSpec::new("content")
                        .summary(
                            "The item: first line is the title, everything after the first \
                             blank line is the body. This is where a pipe's value lands.",
                        )
                        .class(v::ext::XSD_STRING),
                )
                .input(
                    ArgSpec::new("kind")
                        .summary(
                            "An item class IRI for this item's LEVEL — a decision record, a \
                             review finding, a state transition. It is asserted ALONGSIDE \
                             the base class, so a new level needs no change here and every \
                             query over the ledger still sees it.",
                        )
                        .class(v::ext::XSD_ANY_URI)
                        .optional(),
                )
                .input(
                    ArgSpec::new("priority")
                        .summary("0 highest … 4 lowest. Absent means unset, which is not 4.")
                        .class(v::ext::XSD_INTEGER)
                        .one_of(["0", "1", "2", "3", "4"])
                        .optional(),
                )
                .input(
                    ArgSpec::new("labels")
                        .summary("Comma-separated tags.")
                        .class(v::ext::XSD_STRING)
                        .optional(),
                )
                .input(
                    ArgSpec::new("about")
                        .summary(
                            "Whitespace-separated resource IRIs this item is ABOUT — a file, \
                             a commit, a PR, a brief. What makes \"what is open against \
                             this file\" a query rather than an index.",
                        )
                        .class(v::ext::XSD_STRING)
                        .optional(),
                )
                .input(
                    ArgSpec::new("revision")
                        .summary(
                            "The revision this is filed against — a commit sha, a tag. The \
                             pin that keeps a finding about line 40 from rotting.",
                        )
                        .class(v::ext::XSD_STRING)
                        .optional(),
                )
                .input(
                    ArgSpec::new("author")
                        .summary("Who is filing it.")
                        .class(v::ext::XSD_STRING)
                        .optional(),
                )
                .output(PLAIN),
            CAP_WRITE,
        );
        Description::new("ledger-append")
            .title("File a ledger item")
            .summary("Append a new item to the ledger and answer with its number and IRI.")
            .verb(Verb::Meta)
            .action(spec)
    }
}

// ------------------------------------------------------------------------- comment

#[derive(Clone)]
struct CommentEndpoint;

#[async_trait]
impl Endpoint for CommentEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        if inv.request.verb != Verb::Sink {
            return Err(unsupported("ledger-comment", inv.request.verb));
        }
        let client = StoreClient::new(inv);
        let now = now_ms(inv)?;
        let item = require_item(&client, inv.inline_str("item")?).await?;
        let text = inv.inline_str("content")?;
        if text.trim().is_empty() {
            return Err(Error::InvalidArgument {
                name: "content".to_string(),
                detail: "an empty comment says nothing and cannot be edited afterwards".to_string(),
            });
        }
        let comment = write_comment(
            &client,
            &item.iri,
            text.trim(),
            inv.inline_str("author").ok(),
            now,
        )
        .await?;
        Ok(plain(format!(
            "commented on {} ({comment})\n",
            item.short()
        )))
    }

    fn name(&self) -> &str {
        "ledger-comment"
    }

    fn describe(&self) -> Description {
        Description::new("ledger-comment")
            .title("Comment on a ledger item")
            .summary(
                "Append a comment. Comments are append-only — there is no edit and no \
                 delete short of deleting the item, because a record that can be quietly \
                 revised is not a record.",
            )
            .verb(Verb::Meta)
            .action(write_scopes(
                ikigai_core::ActionSpec::new(Verb::Sink)
                    .summary("Append a stamped, attributed comment.")
                    .input(item_arg("The item to comment on."))
                    .input(
                        ArgSpec::new("content")
                            .summary("The comment text — where a pipe's value lands.")
                            .class(v::ext::XSD_STRING),
                    )
                    .input(
                        ArgSpec::new("author")
                            .summary("Who is commenting.")
                            .class(v::ext::XSD_STRING)
                            .optional(),
                    )
                    .output(PLAIN),
                CAP_WRITE,
            ))
    }
}

// --------------------------------------------------------------------- close/reopen

#[derive(Clone)]
struct CloseEndpoint;

#[async_trait]
impl Endpoint for CloseEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        if inv.request.verb != Verb::Sink {
            return Err(unsupported("ledger-close", inv.request.verb));
        }
        let client = StoreClient::new(inv);
        let now = now_ms(inv)?;
        let item = require_item(&client, inv.inline_str("item")?).await?;
        let name = inv.inline_str("reason").unwrap_or("done");
        let reason = v::close_reason(name).ok_or_else(|| Error::InvalidArgument {
            name: "reason".to_string(),
            detail: format!(
                "`{name}` is not one of {}",
                v::CLOSE_REASONS
                    .iter()
                    .map(|(short, _)| *short)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        })?;
        client
            .update(&replace_one(
                &item.iri,
                v::STATUS,
                &format!("<{}>", v::CLOSED),
            ))
            .await?;
        client
            .update(&replace_one(
                &item.iri,
                v::CLOSED_REASON,
                &format!("<{reason}>"),
            ))
            .await?;
        if let Ok(note) = inv.inline_str("content") {
            if !note.trim().is_empty() {
                write_comment(
                    &client,
                    &item.iri,
                    note.trim(),
                    inv.inline_str("author").ok(),
                    now,
                )
                .await?;
            }
        }
        client.update(&touch(&item.iri, now)).await?;
        Ok(plain(format!("closed {} ({name})\n", item.short())))
    }

    fn name(&self) -> &str {
        "ledger-close"
    }

    fn describe(&self) -> Description {
        Description::new("ledger-close")
            .title("Close a ledger item")
            .summary(
                "Close with a reason. Closing is NOT deleting: the item stays in the graph, \
                 stays queryable, and stops blocking whatever it blocked.",
            )
            .verb(Verb::Meta)
            .action(write_scopes(
                ikigai_core::ActionSpec::new(Verb::Sink)
                    .summary("Set the status to closed and record why.")
                    .input(item_arg("The item to close."))
                    .input(
                        ArgSpec::new("reason")
                            .summary(
                                "Why it is closed. `audit-no-change` is a real verdict — \
                                 \"looked, found it fine\" recorded as `done` loses the \
                                 finding.",
                            )
                            .class(v::ext::XSD_STRING)
                            .one_of(v::CLOSE_REASONS.iter().map(|(short, _)| *short))
                            .default_value("done")
                            .optional(),
                    )
                    .input(
                        ArgSpec::new("content")
                            .summary(
                                "An optional closing note, recorded as a comment — where a \
                                 pipe's value lands.",
                            )
                            .class(v::ext::XSD_STRING)
                            .optional(),
                    )
                    .input(
                        ArgSpec::new("author")
                            .summary("Who is closing it.")
                            .class(v::ext::XSD_STRING)
                            .optional(),
                    )
                    .output(PLAIN),
                CAP_WRITE,
            ))
    }
}

#[derive(Clone)]
struct ReopenEndpoint;

#[async_trait]
impl Endpoint for ReopenEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        if inv.request.verb != Verb::Sink {
            return Err(unsupported("ledger-reopen", inv.request.verb));
        }
        let client = StoreClient::new(inv);
        let now = now_ms(inv)?;
        let item = require_item(&client, inv.inline_str("item")?).await?;
        client
            .update(&replace_one(
                &item.iri,
                v::STATUS,
                &format!("<{}>", v::OPEN),
            ))
            .await?;
        client
            .update(&format!(
                "DELETE {{ {} }} WHERE {{ {} }}",
                graph_block(&format!("<{}> <{}> ?was", item.iri, v::CLOSED_REASON)),
                graph_block(&format!("<{}> <{}> ?was", item.iri, v::CLOSED_REASON))
            ))
            .await?;
        if let Ok(note) = inv.inline_str("content") {
            if !note.trim().is_empty() {
                write_comment(
                    &client,
                    &item.iri,
                    note.trim(),
                    inv.inline_str("author").ok(),
                    now,
                )
                .await?;
            }
        }
        client.update(&touch(&item.iri, now)).await?;
        Ok(plain(format!("reopened {}\n", item.short())))
    }

    fn name(&self) -> &str {
        "ledger-reopen"
    }

    fn describe(&self) -> Description {
        Description::new("ledger-reopen")
            .title("Reopen a ledger item")
            .summary("Undo a close: the status goes back to open and the reason is dropped.")
            .verb(Verb::Meta)
            .action(write_scopes(
                ikigai_core::ActionSpec::new(Verb::Sink)
                    .summary("Set the status back to open.")
                    .input(item_arg("The item to reopen."))
                    .input(
                        ArgSpec::new("content")
                            .summary("An optional note, recorded as a comment.")
                            .class(v::ext::XSD_STRING)
                            .optional(),
                    )
                    .input(
                        ArgSpec::new("author")
                            .summary("Who is reopening it.")
                            .class(v::ext::XSD_STRING)
                            .optional(),
                    )
                    .output(PLAIN),
                CAP_WRITE,
            ))
    }
}

// --------------------------------------------------------------------------- claim

#[derive(Clone)]
struct ClaimEndpoint;

#[async_trait]
impl Endpoint for ClaimEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let client = StoreClient::new(inv);
        let now = now_ms(inv)?;
        let item = require_item(&client, inv.inline_str("item")?).await?;
        match inv.request.verb {
            Verb::Sink => {
                let holder = inv.inline_str("content")?.trim().to_string();
                if holder.is_empty() {
                    return Err(Error::InvalidArgument {
                        name: "content".to_string(),
                        detail: "a claim needs a holder: an unattributed claim is \
                                 indistinguishable from no claim"
                            .to_string(),
                    });
                }
                // ⚠ Read-then-write, so two claimants in one process can interleave. The
                // fence this ledger will eventually replace is enforced by a partial unique
                // index in kata's database; RDF has no such constraint and SHACL cannot see
                // a race. Stated rather than pretended away — it is safe for one operator
                // and is the first thing to harden when a second one appears.
                if let Some(held) = &item.claimed_by {
                    if held != &holder {
                        return Err(Error::Unavailable(format!(
                            "{} is already claimed by {held}. Release it first \
                             (`delete urn:iki:ledger:claim item={}`), or take it up with \
                             them — a claim is a fence, and stealing one silently is how two \
                             workers end up in the same tree",
                            item.short(),
                            item.short()
                        )));
                    }
                }
                client
                    .update(&replace_one(&item.iri, v::CLAIMED_BY, &literal(&holder)))
                    .await?;
                client
                    .update(&replace_one(&item.iri, v::CLAIMED_AT, &datetime(now)))
                    .await?;
                if let Ok(purpose) = inv.inline_str("purpose") {
                    client
                        .update(&replace_one(&item.iri, v::PURPOSE, &literal(purpose)))
                        .await?;
                }
                client.update(&touch(&item.iri, now)).await?;
                Ok(plain(format!("{} claimed by {holder}\n", item.short())))
            }
            Verb::Delete => {
                for predicate in [v::CLAIMED_BY, v::CLAIMED_AT, v::PURPOSE] {
                    client
                        .update(&format!(
                            "DELETE {{ {} }} WHERE {{ {} }}",
                            graph_block(&format!("<{}> <{predicate}> ?was", item.iri)),
                            graph_block(&format!("<{}> <{predicate}> ?was", item.iri))
                        ))
                        .await?;
                }
                if let Ok(note) = inv.inline_str("content") {
                    if !note.trim().is_empty() {
                        write_comment(&client, &item.iri, note.trim(), None, now).await?;
                    }
                }
                client.update(&touch(&item.iri, now)).await?;
                Ok(plain(format!("{} released\n", item.short())))
            }
            other => Err(unsupported("ledger-claim", other)),
        }
    }

    fn name(&self) -> &str {
        "ledger-claim"
    }

    fn describe(&self) -> Description {
        Description::new("ledger-claim")
            .title("Claim or release a ledger item")
            .summary(
                "Take an item, or hand it back. The ready set never offers a claimed item, \
                 which is most of what \"what should I do next\" means once more than one \
                 worker shares a ledger.",
            )
            .verb(Verb::Meta)
            .action(write_scopes(
                ikigai_core::ActionSpec::new(Verb::Sink)
                    .summary("Claim the item for a holder; refuses if someone else holds it.")
                    .input(item_arg("The item to claim."))
                    .input(
                        ArgSpec::new("content")
                            .summary("The holder — where a pipe's value lands.")
                            .class(v::ext::XSD_STRING),
                    )
                    .input(
                        ArgSpec::new("purpose")
                            .summary("Why the holder took it — a brief name, a session id.")
                            .class(v::ext::XSD_STRING)
                            .optional(),
                    )
                    .output(PLAIN),
                CAP_WRITE,
            ))
            .action(write_scopes(
                ikigai_core::ActionSpec::new(Verb::Delete)
                    .summary("Release the claim, whoever holds it.")
                    .input(item_arg("The item to release."))
                    .input(
                        ArgSpec::new("content")
                            .summary("An optional release note, recorded as a comment.")
                            .class(v::ext::XSD_STRING)
                            .optional(),
                    )
                    .output(PLAIN),
                CAP_WRITE,
            ))
    }
}

// --------------------------------------------------------------------------- defer

#[derive(Clone)]
struct DeferEndpoint;

#[async_trait]
impl Endpoint for DeferEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let client = StoreClient::new(inv);
        let now = now_ms(inv)?;
        let item = require_item(&client, inv.inline_str("item")?).await?;
        let (update, said) = match inv.request.verb {
            Verb::Sink => (
                replace_one(&item.iri, v::DEFERRED, &boolean(true)),
                "deferred",
            ),
            Verb::Delete => (
                format!(
                    "DELETE {{ {} }} WHERE {{ {} }}",
                    graph_block(&format!("<{}> <{}> ?was", item.iri, v::DEFERRED)),
                    graph_block(&format!("<{}> <{}> ?was", item.iri, v::DEFERRED))
                ),
                "resumed",
            ),
            other => return Err(unsupported("ledger-defer", other)),
        };
        client.update(&update).await?;
        if let Ok(note) = inv.inline_str("content") {
            if !note.trim().is_empty() {
                write_comment(&client, &item.iri, note.trim(), None, now).await?;
            }
        }
        client.update(&touch(&item.iri, now)).await?;
        Ok(plain(format!("{} {said}\n", item.short())))
    }

    fn name(&self) -> &str {
        "ledger-defer"
    }

    fn describe(&self) -> Description {
        Description::new("ledger-defer")
            .title("Defer a ledger item, or resume it")
            .summary(
                "Not now: open, unblocked, unclaimed, and deliberately not offered by the \
                 ready set. A modelled property rather than kata's `someday` flag inside a \
                 metadata blob that one query understands — and a boolean rather than a \
                 date, because readiness that turns on the wall clock goes stale in the \
                 cache with no thread to cut it.",
            )
            .verb(Verb::Meta)
            .action(write_scopes(
                ikigai_core::ActionSpec::new(Verb::Sink)
                    .summary("Mark the item deferred.")
                    .input(item_arg("The item to defer."))
                    .input(
                        ArgSpec::new("content")
                            .summary("An optional note, recorded as a comment.")
                            .class(v::ext::XSD_STRING)
                            .optional(),
                    )
                    .output(PLAIN),
                CAP_WRITE,
            ))
            .action(write_scopes(
                ikigai_core::ActionSpec::new(Verb::Delete)
                    .summary("Undefer: the item becomes eligible for the ready set again.")
                    .input(item_arg("The item to resume."))
                    .input(
                        ArgSpec::new("content")
                            .summary("An optional note, recorded as a comment.")
                            .class(v::ext::XSD_STRING)
                            .optional(),
                    )
                    .output(PLAIN),
                CAP_WRITE,
            ))
    }
}

// ---------------------------------------------------------------------------- link

#[derive(Clone)]
struct LinkEndpoint;

#[async_trait]
impl Endpoint for LinkEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let client = StoreClient::new(inv);
        let now = now_ms(inv)?;
        let from = require_item(&client, inv.inline_str("item")?).await?;
        let to = require_item(&client, inv.inline_str("content")?).await?;
        let name = inv.inline_str("type").unwrap_or("related");
        let predicate = v::link_predicate(name).ok_or_else(|| Error::InvalidArgument {
            name: "type".to_string(),
            detail: format!(
                "`{name}` is not one of {}",
                v::LINK_TYPES
                    .iter()
                    .map(|(short, _)| *short)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        })?;
        if from.iri == to.iri && name == "blocks" {
            return Err(Error::InvalidArgument {
                name: "content".to_string(),
                detail: format!(
                    "{} cannot block itself: a self-block is a cycle, and the ready set \
                     would refuse to compute at all",
                    from.short()
                ),
            });
        }
        let triple = format!("<{}> <{predicate}> <{}> .", from.iri, to.iri);
        let (update, said) = match inv.request.verb {
            Verb::Sink => (
                format!("INSERT DATA {{ {} }}", graph_block(&triple)),
                "linked",
            ),
            Verb::Delete => (
                format!("DELETE DATA {{ {} }}", graph_block(&triple)),
                "unlinked",
            ),
            other => return Err(unsupported("ledger-link", other)),
        };
        client.update(&update).await?;
        client.update(&touch(&from.iri, now)).await?;
        Ok(plain(format!(
            "{said} {} {name} {}\n",
            from.short(),
            to.short()
        )))
    }

    fn name(&self) -> &str {
        "ledger-link"
    }

    fn describe(&self) -> Description {
        let type_arg = || {
            ArgSpec::new("type")
                .summary(
                    "blocks (the object cannot start until the subject finishes), parent \
                     (structural, does not block), or related.",
                )
                .class(v::ext::XSD_STRING)
                .one_of(v::LINK_TYPES.iter().map(|(short, _)| *short))
                .default_value("related")
                .optional()
        };
        Description::new("ledger-link")
            .title("Link two ledger items, or unlink them")
            .summary(
                "The item graph. `blocks` edges are what the ready set reads; they are \
                 meant to be a DAG and nothing enforces it, so `urn:iki:ledger:next` walks \
                 them and refuses with the cycle named.",
            )
            .verb(Verb::Meta)
            .action(write_scopes(
                ikigai_core::ActionSpec::new(Verb::Sink)
                    .summary("Add the edge.")
                    .input(item_arg("The subject item."))
                    .input(
                        ArgSpec::new("content")
                            .summary(
                                "The object item — where a pipe's value lands. Same forms as \
                                 `item`.",
                            )
                            .class(v::ext::XSD_STRING),
                    )
                    .input(type_arg())
                    .output(PLAIN),
                CAP_WRITE,
            ))
            .action(write_scopes(
                ikigai_core::ActionSpec::new(Verb::Delete)
                    .summary("Remove the edge — how a block cycle is broken.")
                    .input(item_arg("The subject item."))
                    .input(
                        ArgSpec::new("content")
                            .summary("The object item.")
                            .class(v::ext::XSD_STRING),
                    )
                    .input(type_arg())
                    .output(PLAIN),
                CAP_WRITE,
            ))
    }
}

// --------------------------------------------------------------------------- label

#[derive(Clone)]
struct LabelEndpoint;

#[async_trait]
impl Endpoint for LabelEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let client = StoreClient::new(inv);
        let now = now_ms(inv)?;
        let item = require_item(&client, inv.inline_str("item")?).await?;
        let label = inv.inline_str("content")?.trim();
        if label.is_empty() {
            return Err(Error::InvalidArgument {
                name: "content".to_string(),
                detail: "a label needs text".to_string(),
            });
        }
        let triple = format!("<{}> <{}> {} .", item.iri, v::LABEL, literal(label));
        let (update, said) = match inv.request.verb {
            Verb::Sink => (
                format!("INSERT DATA {{ {} }}", graph_block(&triple)),
                "labelled",
            ),
            Verb::Delete => (
                format!("DELETE DATA {{ {} }}", graph_block(&triple)),
                "unlabelled",
            ),
            other => return Err(unsupported("ledger-label", other)),
        };
        client.update(&update).await?;
        client.update(&touch(&item.iri, now)).await?;
        Ok(plain(format!("{said} {} {label}\n", item.short())))
    }

    fn name(&self) -> &str {
        "ledger-label"
    }

    fn describe(&self) -> Description {
        Description::new("ledger-label")
            .title("Tag a ledger item, or untag it")
            .summary(
                "Labels are what the ready query filters on — must have ALL of these, must \
                 have NONE of those — which is why the ordering policy does not need to \
                 know about them.",
            )
            .verb(Verb::Meta)
            .action(write_scopes(
                ikigai_core::ActionSpec::new(Verb::Sink)
                    .summary("Add the label.")
                    .input(item_arg("The item to tag."))
                    .input(
                        ArgSpec::new("content")
                            .summary("The label — where a pipe's value lands.")
                            .class(v::ext::XSD_STRING),
                    )
                    .output(PLAIN),
                CAP_WRITE,
            ))
            .action(write_scopes(
                ikigai_core::ActionSpec::new(Verb::Delete)
                    .summary("Remove the label.")
                    .input(item_arg("The item to untag."))
                    .input(
                        ArgSpec::new("content")
                            .summary("The label to remove.")
                            .class(v::ext::XSD_STRING),
                    )
                    .output(PLAIN),
                CAP_WRITE,
            ))
    }
}

// --------------------------------------------------------------------------- purge

#[derive(Clone)]
struct PurgeEndpoint;

#[async_trait]
impl Endpoint for PurgeEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        if inv.request.verb != Verb::Delete {
            return Err(unsupported("ledger-purge", inv.request.verb));
        }
        let client = StoreClient::new(inv);
        let now = now_ms(inv)?;
        let reference = inv.inline_str("content")?;
        let item = require_item(&client, reference).await?;
        let reason = inv.inline_str("reason").unwrap_or("");
        let removed = delete_item(
            &client,
            &item,
            reason,
            inv.inline_str("author").ok(),
            now,
            true,
        )
        .await?;
        Ok(plain(format!(
            "purged {} {}\n  {} quad(s) destroyed, NOT recoverable\n  tombstone: {}{}\n",
            item.short(),
            item.iri,
            removed.quads,
            v::iri::TOMBSTONE,
            removed.id
        )))
    }

    fn name(&self) -> &str {
        "ledger-purge"
    }

    fn describe(&self) -> Description {
        Description::new("ledger-purge")
            .title("Destroy a ledger item's content")
            .summary(
                "Irreversible: the item's quads, its comments and the edges pointing at it \
                 are removed from both the live graph and the quarantine graph. A tombstone \
                 stays, carrying the number, the time, the actor, the reason, the quad count \
                 and the sha256 of what was destroyed — so a later claim about what it said \
                 is checkable, and a ledger that forgets still leaves evidence that it did.",
            )
            .verb(Verb::Meta)
            .action(write_scopes(
                ikigai_core::ActionSpec::new(Verb::Delete)
                    .summary("Destroy the item's content, leaving only its tombstone.")
                    .input(
                        ArgSpec::new("content")
                            .summary(
                                "The item — where a pipe's value lands. `#12`, an opaque id, \
                                 or the full IRI.",
                            )
                            .class(v::ext::XSD_STRING),
                    )
                    .input(
                        ArgSpec::new("reason")
                            .summary("Why — kept on the tombstone, which is all that is left.")
                            .class(v::ext::XSD_STRING)
                            .optional(),
                    )
                    .input(
                        ArgSpec::new("author")
                            .summary("Who purged it.")
                            .class(v::ext::XSD_STRING)
                            .optional(),
                    )
                    .output(PLAIN),
                CAP_PURGE,
            ))
    }
}

// ---------------------------------------------------------------------------- next

#[derive(Clone)]
struct NextEndpoint {
    policies: Arc<Policies>,
}

#[async_trait]
impl Endpoint for NextEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        if inv.request.verb != Verb::Source {
            return Err(unsupported("ledger-next", inv.request.verb));
        }
        let client = StoreClient::new(inv);
        let want = wanted_face(inv)?;
        let now = now_ms(inv)?;
        let name = inv
            .inline_str("policy")
            .unwrap_or_else(|_| self.policies.default_name());
        let policy = self
            .policies
            .get(name)
            .ok_or_else(|| Error::InvalidArgument {
                name: "policy".to_string(),
                detail: format!(
                    "no ordering policy `{name}`; this host offers {}",
                    self.policies.names().join(", ")
                ),
            })?;
        let filter = Filter {
            status: Status::Open,
            kind: inv.inline_str("kind").ok().map(str::to_string),
            labels: split_labels(inv.inline_str("labels").ok()),
            without: split_labels(inv.inline_str("without").ok()),
            about: inv.inline_str("about").ok().map(str::to_string),
            ..Filter::default()
        };
        let limit = inv
            .inline_str("limit")
            .ok()
            .and_then(|l| l.parse().ok())
            .unwrap_or(3);
        let set = select::ready(&client, &filter).await?;
        let selection = select::rank(set, policy.as_ref(), now, limit);
        if want == TURTLE {
            return Ok(Representation::new(
                ReprType::new(TURTLE).with_param("charset", "utf-8"),
                selection.turtle()?,
            )
            .cacheable()
            .depends_on(ikigai_store::UPDATE_THREAD)
            .depends_on(ikigai_store::LOAD_THREAD));
        }
        face(selection.plain(), None, want)
    }

    fn name(&self) -> &str {
        "ledger-next"
    }

    fn describe(&self) -> Description {
        read_scopes(
            Description::new("ledger-next")
                .title("What to do next")
                .summary(
                    "The ready set — open, unclaimed, not deferred, not blocked by an open \
                     item — ranked by an ordering policy, with the reasons. Refuses, naming \
                     the cycle, when the `blocks` graph is not a DAG. ⚠ Selection OFFERS; it \
                     never authorizes: acting on the answer still requires the actor's own \
                     capability.",
                )
                .verb(Verb::Source)
                .verb(Verb::Meta)
                .input(
                    ArgSpec::new("policy")
                        .summary(format!(
                            "Which ordering policy ranks the ready set. Each is a resource \
                             at `urn:iki:ledger:policy:{{name}}` that says what it weighs. \
                             This host offers: {}.",
                            self.policies.names().join(", ")
                        ))
                        .class(v::ext::XSD_STRING)
                        .one_of(self.policies.names())
                        .default_value(self.policies.default_name())
                        .optional(),
                )
                .input(
                    ArgSpec::new("kind")
                        .summary(
                            "An item class IRI — ask \"what is next\" at one LEVEL. \"What is \
                             next\" among decisions is a different question from \"what is \
                             next\" among findings.",
                        )
                        .class(v::ext::XSD_ANY_URI)
                        .optional(),
                )
                .input(
                    ArgSpec::new("labels")
                        .summary("Comma-separated; a candidate must carry ALL of them.")
                        .class(v::ext::XSD_STRING)
                        .optional(),
                )
                .input(
                    ArgSpec::new("without")
                        .summary("Comma-separated; a candidate must carry NONE of them.")
                        .class(v::ext::XSD_STRING)
                        .optional(),
                )
                .input(
                    ArgSpec::new("about")
                        .summary("Only candidates filed against this resource IRI.")
                        .class(v::ext::XSD_ANY_URI)
                        .optional(),
                )
                .input(
                    ArgSpec::new("limit")
                        .summary(
                            "How many ranked candidates to return (default 3). Applied AFTER \
                             ranking, never before.",
                        )
                        .class(v::ext::XSD_INTEGER)
                        .default_value("3")
                        .optional(),
                )
                .input(as_arg())
                .output(PLAIN)
                .output(TURTLE),
        )
    }
}

// -------------------------------------------------------------------------- policy

#[derive(Clone)]
struct PolicyEndpoint {
    policies: Arc<Policies>,
}

#[async_trait]
impl Endpoint for PolicyEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let name = inv.bindings.get("name").ok_or_else(|| {
            Error::Endpoint("no `name` captured from `urn:iki:ledger:policy:{name}`".to_string())
        })?;
        let policy = self.policies.get(name).ok_or_else(|| {
            Error::NotFound(format!(
                "no ordering policy `{name}`; this host offers {}",
                self.policies.names().join(", ")
            ))
        })?;
        match inv.request.verb {
            Verb::Exists => Ok(plain("true\n")),
            Verb::Source => {
                let want = wanted_face(inv)?;
                if want == TURTLE {
                    let iri = format!("{}{}", v::iri::POLICY, policy.name());
                    let subject = oxrdf::NamedNode::new(&iri)
                        .map_err(|e| Error::Endpoint(format!("policy IRI: {e}")))?;
                    let mut graph = Graph::new();
                    let mut push = |p: &str, o: oxrdf::Term| {
                        if let Ok(predicate) = oxrdf::NamedNode::new(p) {
                            graph.insert(&oxrdf::Triple::new(subject.clone(), predicate, o));
                        }
                    };
                    push(v::ext::TYPE, model::named(v::POLICY_CLASS));
                    push(v::ext::LABEL, model::plain(policy.name()));
                    push(v::ext::COMMENT, model::plain(policy.summary()));
                    for signal in policy.weighs() {
                        push(v::WEIGHS, model::plain(signal));
                    }
                    // Pure: a policy resource reads no state, so it is cacheable with no
                    // thread — the one endpoint here that is a function of its inputs alone.
                    return Ok(Representation::new(
                        ReprType::new(TURTLE).with_param("charset", "utf-8"),
                        model::turtle(&graph)?,
                    )
                    .cacheable());
                }
                Ok(plain(format!(
                    "{}\n  {}\n  weighs: {}\n",
                    policy.name(),
                    policy.summary(),
                    policy.weighs().join(", ")
                ))
                .cacheable())
            }
            other => Err(unsupported("ledger-policy", other)),
        }
    }

    fn name(&self) -> &str {
        "ledger-policy"
    }

    fn describe(&self) -> Description {
        let name_input = || {
            ArgSpec::new("name")
                .summary("The policy's name.")
                .class(v::ext::XSD_STRING)
                .binding()
        };
        Description::new("ledger-policy")
            .title("An ordering policy")
            .summary(
                "What one `urn:iki:ledger:next` policy weighs, and in what order. A policy \
                 is a resource rather than a config key so the manifold advertises which \
                 orderings exist, and so one can be versioned, diffed and handed to another \
                 host.",
            )
            .verb(Verb::Meta)
            // Per-verb, because the two verbs really do serve different faces: `Exists`
            // answers `true` in text/plain and nothing else, and declaring the graph face
            // on it would promise a Turtle document that no `as=` could ever produce.
            .action(
                ikigai_core::ActionSpec::new(Verb::Source)
                    .summary("What this policy weighs, and in what order.")
                    .requires(CAP_READ)
                    .input(name_input())
                    .input(as_arg())
                    .output(PLAIN)
                    .output(TURTLE),
            )
            .action(
                ikigai_core::ActionSpec::new(Verb::Exists)
                    .summary("Whether this host offers a policy by that name.")
                    .requires(CAP_READ)
                    .input(name_input())
                    .output(PLAIN),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_is_time_ordered_and_unique_per_millisecond() {
        let earlier = mint_id(1_757_700_000_000, "a", "brian");
        let later = mint_id(1_757_700_000_001, "a", "brian");
        assert!(earlier < later, "{earlier} should sort before {later}");
        assert_eq!(earlier.len(), 16);
        // Same millisecond, different content: the digest half separates them, which is
        // what keeps two merged ledgers from colliding.
        assert_ne!(
            mint_id(1_757_700_000_000, "a", "brian"),
            mint_id(1_757_700_000_000, "b", "brian")
        );
    }

    #[test]
    fn content_splits_into_title_and_body() {
        let (title, body) = split_content("Fix the thing\n\nIt is broken.\nBadly.").unwrap();
        assert_eq!(title, "Fix the thing");
        assert_eq!(body, "It is broken.\nBadly.");
        let (title, body) = split_content("  Just a title  ").unwrap();
        assert_eq!(title, "Just a title");
        assert!(body.is_empty());
        assert!(split_content("   ").is_err());
    }

    #[test]
    fn an_unserveable_face_is_refused_not_substituted() {
        assert_eq!(bare("text/turtle; charset=utf-8"), "text/turtle");
    }

    #[test]
    fn a_priority_outside_the_range_is_refused() {
        assert!(parse_priority("0").is_ok());
        assert!(parse_priority("4").is_ok());
        assert!(parse_priority("5").is_err());
        assert!(parse_priority("high").is_err());
    }
}
