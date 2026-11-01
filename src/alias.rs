//! Aliases by scheme (ledger #2828): an item resolvable by an external id, in a scheme one
//! principal owns.
//!
//! ```text
//! urn:iki:alias:scheme:{scheme}                  Source Sink         who owns it / register it
//! urn:iki:alias:scheme:{scheme}:value:{value}    Source Sink Delete  which item / give / remove
//! ```
//!
//! An item may carry many aliases, each a `(scheme, value)` pair: `kata:01m6c5…`,
//! `kata:web#3f2`, `roborev:job/88`, `gh:acme/web#12`. Every reference form that takes an item
//! resolves one, `item=gh:acme/web#12` and `urn:iki:ledger:item:gh:acme%2Fweb%2312` alike, and
//! so does the alias's own IRI. **An alias is not a key**: the idempotency key (`append key=`)
//! is unchanged, an item's primary name stays its number, and a new item carries no alias.
//!
//! # One graph for the deployment, and one update decides
//!
//! An alias is unique per scheme across EVERY ledger, so it cannot live in a ledger's graph:
//! deciding uniqueness there would read every ledger and write one, and the store's narrow
//! door names one graph per update. Every scheme and every alias is in [`GRAPH`],
//! `urn:iki:alias:graph`, and the update that adds an alias carries both of its conditions in
//! its own `WHERE` — the caller owns the scheme, and nothing holds the alias yet — so the
//! store decides them under its write lock (the check-then-act lesson of ledger #2772). Two
//! principals racing for one alias leave one alias; `tests/aliases.rs` races them.
//!
//! # ★ Who may do what, which is a new authority boundary
//!
//! - **Register a scheme**: `urn:cap:alias:scheme:{scheme}`, a grant per scheme, and a
//!   capability that carries a principal. The principal becomes the scheme's owner, for good:
//!   the first registration wins, inside the write, and a second is refused naming the owner.
//! - **Give an item an alias**: own the scheme (data, checked inside the write, never a
//!   grant), AND hold `urn:cap:ledger:alias:{ledger}` (or `write:{ledger}`) on the item's own
//!   ledger, so a scheme owner cannot attach its names to a ledger it was never let into. A
//!   non-owner is refused naming the owner.
//! - **Remove an alias**: own the scheme.
//! - **Read the aliases graph**: the store's own grant for it,
//!   `urn:cap:store:read:graph:urn:iki:alias:graph`, which every alias resource and every
//!   alias reference needs. ⚠ **That grant reveals that items exist**: an alias names an item
//!   IRI, and the IRI carries its ledger and its opaque id (never its title, body or anything
//!   else). A host issues it to whoever it trusts with that map — an importer, a sync, the
//!   operator — and to nobody it would not show the list of ledgers.
//! - **Resolve an alias to an item**: the aliases graph's read grant AND read on the item's
//!   ledger. Resolution never names an item to a caller without read on its ledger: an alias
//!   of an item the caller cannot read answers exactly what an alias nobody holds answers.
//!   Inside a ledger resource the same holds, and an alias of an item in another ledger the
//!   caller CAN read is refused the way a foreign item IRI is, naming where to address it.
//!
//! Writes into the aliases graph need its store write grant,
//! `urn:cap:store:write:graph:urn:iki:alias:graph`, for the reason every write here needs its
//! graph's: a sub-request carries the caller's capability unchanged.
//!
//! # Not built, on purpose
//!
//! No face of an item lists its aliases (the item faces read one ledger's graph, and adding a
//! second graph to every read would put the aliases graph's grant on every reader); a scheme
//! has no transfer; deleting or purging an item leaves its aliases, which then resolve to an
//! item that is not found, the way a key stays taken.

use async_trait::async_trait;
use ikigai_core::{
    ArgSpec, Description, Endpoint, EndpointSpace, Error, Invocation, Representation, Result,
    UriTemplate, Verb,
};

use crate::attribution::Attribution;
use crate::endpoints::{
    grant_for, now_ms, plain, require_item, unsupported, Need, CAP_ANY, CAP_READ,
};
use crate::ledger::Ledger;
use crate::sparql::{self, literal, StoreClient};
use crate::vocabulary as v;

/// The deployment's one aliases graph: every scheme and every alias, for every ledger.
pub const GRAPH: &str = "urn:iki:alias:graph";

/// `urn:iki:alias:scheme:` — a scheme's IRI is this and its name; an alias's is the scheme's,
/// `:value:` and its value, percent-encoded.
pub const SCHEME_PREFIX: &str = "urn:iki:alias:scheme:";

/// Registering a scheme, as declared: the family. A held grant names one scheme,
/// `urn:cap:alias:scheme:{scheme}` ([`cap_scheme`]).
pub const CAP_SCHEME: &str = "urn:cap:alias:scheme:*";

/// The longest a scheme name may be.
pub const MAX_SCHEME: usize = 32;

/// The longest an alias value may be, in characters: the bound a key has
/// ([`crate::model::MAX_KEY`]), for the same reasons.
pub const MAX_VALUE: usize = 256;

/// Scheme names that would make a reference mean two things: `key:` is the idempotency key's
/// spelling and `urn:` an IRI's.
pub const RESERVED_SCHEMES: [&str; 3] = ["key", "urn", "alias"];

/// The grant that registers `scheme`: `urn:cap:alias:scheme:{scheme}`.
///
/// ```
/// assert_eq!(ikigai_ledger::alias::cap_scheme("kata"), "urn:cap:alias:scheme:kata");
/// ```
pub fn cap_scheme(scheme: &str) -> String {
    format!("urn:cap:alias:scheme:{scheme}")
}

/// `urn:iki:alias:scheme:{scheme}`.
pub fn scheme_iri(scheme: &str) -> String {
    format!("{SCHEME_PREFIX}{scheme}")
}

/// `urn:iki:alias:scheme:{scheme}:value:{value}`, the value percent-encoded outside the
/// unreserved characters (`A-Z a-z 0-9 - . _ ~`), so one alias has one IRI however it was
/// spelled and survives a door that maps `:` and `/` to path segments.
///
/// ```
/// assert_eq!(
///     ikigai_ledger::alias::alias_iri("gh", "acme/web#12"),
///     "urn:iki:alias:scheme:gh:value:acme%2Fweb%2312"
/// );
/// ```
pub fn alias_iri(scheme: &str, value: &str) -> String {
    format!("{SCHEME_PREFIX}{scheme}:value:{}", encode(value))
}

fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Percent-decode a value as it arrives in a reference or an IRI. A `%` not followed by two
/// hex digits is refused rather than kept, so `%` itself is always `%25`.
fn decode(raw: &str, arg: &str) -> Result<String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = raw
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok());
            match hex {
                Some(byte) => {
                    out.push(byte);
                    i += 3;
                    continue;
                }
                None => {
                    return Err(Error::InvalidArgument {
                        name: arg.to_string(),
                        detail: format!(
                            "`{raw}` has a `%` that is not followed by two hex digits; an alias \
                             value is percent-decoded, so a `%` in the value is written `%25`"
                        ),
                    })
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).map_err(|_| Error::InvalidArgument {
        name: arg.to_string(),
        detail: format!("`{raw}` does not decode to UTF-8 text"),
    })
}

/// Whether `name` has a scheme's shape: lowercase letters, digits and `-`, starting with a
/// letter, at most [`MAX_SCHEME`], and not one of [`RESERVED_SCHEMES`].
fn is_scheme(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_SCHEME
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !RESERVED_SCHEMES.contains(&name)
}

/// A scheme name, or a refusal naming the shape.
pub fn parse_scheme(name: &str) -> Result<&str> {
    if is_scheme(name) {
        return Ok(name);
    }
    Err(Error::InvalidArgument {
        name: "scheme".to_string(),
        detail: format!(
            "`{name}` is not a scheme name: lowercase letters, digits and `-`, starting with a \
             letter, at most {MAX_SCHEME} characters, and not one of {}, which already mean \
             something in an item reference",
            RESERVED_SCHEMES.join(", ")
        ),
    })
}

/// An alias value, decoded, or a refusal naming what is accepted: 1 to [`MAX_VALUE`]
/// characters, no whitespace and no control characters.
pub fn parse_value(raw: &str, arg: &str) -> Result<String> {
    let value = decode(raw, arg)?;
    let count = value.chars().count();
    if count == 0 || count > MAX_VALUE {
        return Err(Error::InvalidArgument {
            name: arg.to_string(),
            detail: format!(
                "an alias value is 1 to {MAX_VALUE} characters, and `{raw}` is {count}"
            ),
        });
    }
    if value.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(Error::InvalidArgument {
            name: arg.to_string(),
            detail: format!(
                "`{raw}` holds whitespace or a control character, and an alias value holds \
                 neither: it is typed as one word, `{{scheme}}:{{value}}`"
            ),
        });
    }
    Ok(value)
}

/// `(scheme, value)` when `reference` is an alias: `{scheme}:{value}` with a scheme's shape,
/// or the alias's own IRI. `None` for anything else, which resolves as it always did.
pub(crate) fn split(reference: &str) -> Option<(&str, &str)> {
    if let Some(rest) = reference.strip_prefix(SCHEME_PREFIX) {
        let (scheme, value) = rest.split_once(":value:")?;
        return is_scheme(scheme).then_some((scheme, value));
    }
    let (scheme, value) = reference.split_once(':')?;
    (is_scheme(scheme) && !value.is_empty()).then_some((scheme, value))
}

/// The item `scheme:value` names, from the aliases graph, if any.
async fn holder_of(inv: &Invocation<'_>, scheme: &str, value: &str) -> Result<Option<String>> {
    let rows = select(
        inv,
        &format!(
            "SELECT ?item WHERE {{ GRAPH <{GRAPH}> {{ <{}> <{}> ?item }} }} ORDER BY ?item LIMIT 1",
            alias_iri(scheme, value),
            v::ALIAS_OF
        ),
        &format!("`{scheme}:{value}`"),
    )
    .await?;
    Ok(rows
        .first()
        .and_then(|row| row.get("item"))
        .map(|b| b.value.clone()))
}

/// A SELECT over [`GRAPH`], with the store's refusal turned into the sentence a caller can act
/// on: which grant reads the aliases graph, and what reading it reveals.
async fn select(inv: &Invocation<'_>, query: &str, what: &str) -> Result<Vec<sparql::Row>> {
    sparql::select_in(inv, GRAPH, query)
        .await
        .map_err(|e| match e {
            Error::Denied(_) => Error::Denied(format!(
                "{what} is an alias, and aliases are read from the deployment's aliases graph \
                 `{GRAPH}`, which this capability may not read: that takes \
                 `urn:cap:store:read:graph:{GRAPH}`, a grant a host issues on purpose because \
                 the graph shows which items exist"
            )),
            other => other,
        })
}

/// An UPDATE on [`GRAPH`], with the store's refusal turned into a sentence.
async fn update(inv: &Invocation<'_>, update: &str) -> Result<()> {
    sparql::update_in(inv, GRAPH, update)
        .await
        .map_err(|e| match e {
            Error::Denied(_) => Error::Denied(format!(
                "this write goes into the deployment's aliases graph `{GRAPH}`, and this \
                 capability may not write it: that takes `urn:cap:store:write:graph:{GRAPH}`"
            )),
            other => other,
        })
}

/// Resolve `reference` in this client's ledger when it is an alias: `Ok(None)` when it is not
/// one, the item's IRI when it names an item of this ledger, and otherwise a refusal.
///
/// ★ **The refusal never names an item the caller may not read.** An alias of an item in
/// another ledger is `NotFound` with the same sentence as an alias nobody holds, unless the
/// caller holds read on that ledger, when it is refused the way a foreign item IRI is.
pub(crate) async fn resolve(
    client: &StoreClient<'_, '_>,
    reference: &str,
) -> Result<Option<String>> {
    let Some((scheme, raw)) = split(reference) else {
        return Ok(None);
    };
    let value = parse_value(raw, "item")?;
    let inv = client.invocation();
    let here = client.ledger();
    let unknown = || {
        Error::NotFound(format!(
            "no item in the ledger `{}` has the alias `{scheme}:{value}`",
            here.name()
        ))
    };
    let Some(item) = holder_of(inv, scheme, &value).await? else {
        return Err(unknown());
    };
    match Ledger::of_subject(&item) {
        Some(ledger) if &ledger == here => Ok(Some(item)),
        Some(other) if inv.capability.allows(&other.cap_read()) => Err(Error::InvalidArgument {
            name: "item".to_string(),
            detail: format!(
                "the alias `{scheme}:{value}` names `{item}`, an item of the ledger `{}`, \
                     and this resource is `{}`. A ledger is a boundary: address it at `{}`",
                other.name(),
                here.name(),
                other.prefix()
            ),
        }),
        _ => Err(unknown()),
    }
}

/// The scheme and value an alias resource's IRI names.
fn bound(inv: &Invocation<'_>) -> Result<(String, Option<String>)> {
    let scheme = inv.bindings.get("scheme").ok_or_else(|| {
        Error::Endpoint("no `scheme` captured from `urn:iki:alias:scheme:{scheme}`".to_string())
    })?;
    let scheme = parse_scheme(scheme)?.to_string();
    let value = match inv.bindings.get("value") {
        Some(raw) => Some(parse_value(raw, "value")?),
        None => None,
    };
    Ok((scheme, value))
}

/// The principal a write is made as, or a refusal saying an alias write needs one.
fn principal(inv: &Invocation<'_>, doing: &str) -> Result<String> {
    let who = Attribution::committer(inv)?;
    who.committer.ok_or_else(|| {
        Error::Denied(format!(
            "{doing} needs a capability that carries a principal (`urn:cap:principal:…`), \
             because a scheme belongs to one principal and only it may add aliases in it; \
             this one carries none"
        ))
    })
}

/// A scheme's owner, if it is registered.
async fn owner_of(inv: &Invocation<'_>, scheme: &str) -> Result<Option<(String, String)>> {
    let rows = select(
        inv,
        &format!(
            "SELECT ?owner ?at WHERE {{ GRAPH <{GRAPH}> {{ <{}> <{}> ?owner \
             OPTIONAL {{ <{}> <{}> ?at }} }} }} ORDER BY ?owner LIMIT 1",
            scheme_iri(scheme),
            v::SCHEME_OWNER,
            scheme_iri(scheme),
            v::COMMITTED_AT
        ),
        &format!("the scheme `{scheme}`"),
    )
    .await?;
    Ok(rows.first().and_then(|row| {
        let owner = row.get("owner")?.value.clone();
        let at = row.get("at").map(|b| b.value.clone()).unwrap_or_default();
        Some((owner, at))
    }))
}

fn unregistered(scheme: &str) -> Error {
    Error::NotFound(format!(
        "the scheme `{scheme}` is not registered: its owner registers it once with a Sink at \
         `{}`, holding `{}`",
        scheme_iri(scheme),
        cap_scheme(scheme)
    ))
}

fn not_owner(scheme: &str, owner: &str) -> Error {
    Error::Denied(format!(
        "the scheme `{scheme}` belongs to `{owner}`, and only its owner may add or remove \
         aliases in it"
    ))
}

// ---------------------------------------------------------------------------- space

/// Bind the alias resources: `urn:iki:alias:scheme:{scheme}` and
/// `urn:iki:alias:scheme:{scheme}:value:{value}`.
///
/// ★ **A space of its own, beside [`crate::space`]**, because the aliases graph is a
/// deployment-wide authority with grants of its own, not a part of any ledger: a host binds it
/// when it issues those grants. Resolving an alias inside a ledger resource needs only the
/// graph's read grant, whether or not this space is bound.
/// It takes no configuration, so it names itself, [`SPACE_ID`].
pub fn space() -> EndpointSpace {
    // ⚠ The value form goes FIRST: `{scheme}` would otherwise capture `kata:value:01m…`.
    EndpointSpace::new()
        .bind(
            UriTemplate::parse("urn:iki:alias:scheme:{scheme}:value:{value}")
                .expect("a constant template"),
            AliasEndpoint,
        )
        .bind(
            UriTemplate::parse("urn:iki:alias:scheme:{scheme}").expect("a constant template"),
            SchemeEndpoint,
        )
        .named(ikigai_core::space_iri("ledger-alias"))
}

/// The name [`space`] claims: `urn:iki:space:ledger-alias`.
pub const SPACE_ID: &str = "urn:iki:space:ledger-alias";

fn scheme_input() -> ArgSpec {
    ArgSpec::new("scheme")
        .summary(
            "The scheme's name: lowercase letters, digits and `-`, starting with a letter \
             (`kata`, `roborev`, `gh`). `key`, `urn` and `alias` are reserved.",
        )
        .class(v::ext::XSD_STRING)
        .binding()
}

fn read_graph(spec: ikigai_core::ActionSpec) -> ikigai_core::ActionSpec {
    spec.requires(ikigai_store::CAP_READ_GRAPH)
}

/// A write into the aliases graph: its store grants, and a principal, since every write here
/// is made AS one (`urn:cap:principal:*` is core's presence test).
fn write_graph(spec: ikigai_core::ActionSpec) -> ikigai_core::ActionSpec {
    spec.requires(ikigai_store::CAP_READ_GRAPH)
        .requires(ikigai_store::CAP_WRITE_GRAPH)
        .requires(PRINCIPAL)
}

/// "Carries a principal", as declared.
const PRINCIPAL: &str = "urn:cap:principal:*";

/// `urn:iki:alias:scheme:{scheme}`: who owns a scheme, and registering it.
#[derive(Clone)]
struct SchemeEndpoint;

#[async_trait]
impl Endpoint for SchemeEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let (scheme, _) = bound(inv)?;
        match inv.request.verb {
            Verb::Source => match owner_of(inv, &scheme).await? {
                Some((owner, at)) => Ok(plain(format!(
                    "scheme {scheme} owned by {owner}{}\n",
                    if at.is_empty() {
                        String::new()
                    } else {
                        format!(" since {at}")
                    }
                ))),
                None => Err(unregistered(&scheme)),
            },
            Verb::Sink => {
                let grant = cap_scheme(&scheme);
                if !inv.capability.allows(&grant) {
                    return Err(Error::Denied(format!(
                        "registering the scheme `{scheme}` takes `{grant}`, a grant per scheme, \
                         and this capability does not hold it"
                    )));
                }
                let me = principal(inv, &format!("registering the scheme `{scheme}`"))?;
                let now = now_ms(inv)?;
                let subject = scheme_iri(&scheme);
                // ★ The first registration wins INSIDE the write: an owner already there makes
                // the WHERE match nothing.
                update(
                    inv,
                    &format!(
                        "INSERT {{ GRAPH <{GRAPH}> {{ <{subject}> <{}> <{}> ; <{}> {} ; \
                         <{}> <{me}> ; <{}> <{me}> ; <{}> {} . }} }} \
                         WHERE {{ FILTER NOT EXISTS {{ GRAPH <{GRAPH}> {{ <{subject}> <{}> ?any }} }} }}",
                        v::ext::TYPE,
                        v::ALIAS_SCHEME_CLASS,
                        v::SCHEME_NAME,
                        literal(&scheme),
                        v::SCHEME_OWNER,
                        v::COMMITTER,
                        v::COMMITTED_AT,
                        sparql::datetime(now),
                        v::SCHEME_OWNER,
                    ),
                )
                .await?;
                match owner_of(inv, &scheme).await? {
                    Some((owner, _)) if owner == me => {
                        Ok(plain(format!("scheme {scheme} owned by {owner}\n")))
                    }
                    Some((owner, _)) => Err(Error::Conflict(format!(
                        "the scheme `{scheme}` is already registered, and it belongs to \
                         `{owner}`; a scheme has one owner"
                    ))),
                    None => Err(Error::Endpoint(format!(
                        "the scheme `{scheme}` was registered and then not found; the aliases \
                         graph changed under this write"
                    ))),
                }
            }
            other => Err(unsupported("alias-scheme", other)),
        }
    }

    fn name(&self) -> &str {
        "alias-scheme"
    }

    fn describe(&self) -> Description {
        Description::new("alias-scheme")
            .title("An alias scheme")
            .summary(
                "A namespace of external ids (`kata`, `roborev`, `gh`) and the one principal \
                 that may add aliases in it. Every scheme is in the deployment's aliases graph, \
                 `urn:iki:alias:graph`, which takes its own store grants.",
            )
            .verb(Verb::Meta)
            .action(read_graph(
                ikigai_core::ActionSpec::new(Verb::Source)
                    .summary("Who owns the scheme, and since when; not found when unregistered.")
                    .input(scheme_input())
                    .output(crate::endpoints::PLAIN_FACE),
            ))
            .action(write_graph(
                ikigai_core::ActionSpec::new(Verb::Sink)
                    .summary(
                        "Register the scheme, owned by the caller's principal. Takes \
                         `urn:cap:alias:scheme:{scheme}`. The first registration wins; \
                         registering again as the owner answers the same, and as anyone else \
                         is refused naming the owner.",
                    )
                    .requires(CAP_SCHEME)
                    .input(scheme_input())
                    .output(crate::endpoints::PLAIN_FACE),
            ))
    }
}

/// `urn:iki:alias:scheme:{scheme}:value:{value}`: which item an alias names, giving it
/// (`content=` the item), and removing it.
#[derive(Clone)]
struct AliasEndpoint;

#[async_trait]
impl Endpoint for AliasEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let (scheme, value) = bound(inv)?;
        let value = value.ok_or_else(|| {
            Error::Endpoint("no `value` captured from `…:value:{value}`".to_string())
        })?;
        let name = format!("{scheme}:{value}");
        let subject = alias_iri(&scheme, &value);
        match inv.request.verb {
            Verb::Source => {
                let nobody = || {
                    Error::NotFound(format!(
                        "no item this capability may read has the alias `{name}`"
                    ))
                };
                let item = holder_of(inv, &scheme, &value).await?.ok_or_else(nobody)?;
                // ★ Read on the item's ledger, or the same answer as no item at all.
                let ledger = Ledger::of_subject(&item)
                    .filter(|ledger| inv.capability.allows(&ledger.cap_read()))
                    .ok_or_else(nobody)?;
                let client = StoreClient::new(inv, ledger);
                let found = require_item(&client, &item).await?;
                Ok(plain(format!(
                    "{name} names {} {}\n{}\n",
                    found.short(),
                    found.iri,
                    found.line()
                )))
            }
            Verb::Sink => {
                let me = principal(inv, &format!("adding the alias `{name}`"))?;
                let reference = inv.inline_str("content")?;
                let ledger = ledger_of(reference)?;
                let client = StoreClient::new(inv, grant_for(inv, ledger, Need::Alias)?);
                let item = require_item(&client, reference).await?;
                let now = now_ms(inv)?;
                let scheme_node = scheme_iri(&scheme);
                // ★ Both conditions in the write's own WHERE: the caller owns the scheme, and
                // nothing holds the alias yet. The store evaluates them under its write lock,
                // so two adds of one alias leave one, and a read before could only race.
                update(
                    inv,
                    &format!(
                        "INSERT {{ GRAPH <{GRAPH}> {{ <{subject}> <{}> <{}> ; <{}> <{scheme_node}> ; \
                         <{}> {} ; <{}> <{}> ; <{}> <{me}> ; <{}> {} . }} }} \
                         WHERE {{ GRAPH <{GRAPH}> {{ <{scheme_node}> <{}> <{me}> . \
                         FILTER NOT EXISTS {{ <{subject}> <{}> ?taken }} }} }}",
                        v::ext::TYPE,
                        v::ALIAS_CLASS,
                        v::ALIAS_SCHEME,
                        v::ALIAS_VALUE,
                        literal(&value),
                        v::ALIAS_OF,
                        item.iri,
                        v::COMMITTER,
                        v::COMMITTED_AT,
                        sparql::datetime(now),
                        v::SCHEME_OWNER,
                        v::ALIAS_OF,
                    ),
                )
                .await?;
                // What the write found, read after it: the reason it did nothing, if it did.
                match owner_of(inv, &scheme).await? {
                    None => return Err(unregistered(&scheme)),
                    Some((owner, _)) if owner != me => return Err(not_owner(&scheme, &owner)),
                    Some(_) => {}
                }
                match holder_of(inv, &scheme, &value).await? {
                    Some(holder) if holder == item.iri => Ok(plain(format!(
                        "{name} names {} {}\n",
                        item.short(),
                        item.iri
                    ))),
                    Some(holder) => Err(Error::Conflict(format!(
                        "the alias `{name}` already names `{holder}`, and an alias names one \
                         item; remove it there first (a Delete at `{subject}`) to move it"
                    ))),
                    None => Err(Error::Endpoint(format!(
                        "the alias `{name}` was added and then not found; the aliases graph \
                         changed under this write"
                    ))),
                }
            }
            Verb::Delete => {
                let me = principal(inv, &format!("removing the alias `{name}`"))?;
                match owner_of(inv, &scheme).await? {
                    None => return Err(unregistered(&scheme)),
                    Some((owner, _)) if owner != me => return Err(not_owner(&scheme, &owner)),
                    Some(_) => {}
                }
                let had = holder_of(inv, &scheme, &value).await?;
                update(
                    inv,
                    &format!(
                        "DELETE {{ GRAPH <{GRAPH}> {{ <{subject}> ?p ?o }} }} \
                         WHERE {{ GRAPH <{GRAPH}> {{ <{}> <{}> <{me}> . <{subject}> ?p ?o }} }}",
                        scheme_iri(&scheme),
                        v::SCHEME_OWNER,
                    ),
                )
                .await?;
                Ok(plain(match had {
                    Some(item) => format!("{name} no longer names {item}\n"),
                    None => format!("{name} named no item\n"),
                }))
            }
            other => Err(unsupported("alias", other)),
        }
    }

    fn name(&self) -> &str {
        "alias"
    }

    fn describe(&self) -> Description {
        let value_input = || {
            ArgSpec::new("value")
                .summary(format!(
                    "The alias's value within its scheme, percent-encoded outside `A-Z a-z \
                     0-9 - . _ ~` (`acme%2Fweb%2312` for `acme/web#12`): 1 to {MAX_VALUE} \
                     characters, no whitespace."
                ))
                .class(v::ext::XSD_STRING)
                .binding()
        };
        Description::new("alias")
            .title("An alias")
            .summary(
                "One `(scheme, value)` pair naming one item, unique per scheme across the \
                 deployment: an external id (`kata:01m6c5…`, `gh:acme/web#12`) every item \
                 reference resolves. Not the idempotency key, which is unchanged.",
            )
            .verb(Verb::Meta)
            .action(read_graph(
                ikigai_core::ActionSpec::new(Verb::Source)
                    .summary(
                        "The item the alias names, as its number, IRI and line. Takes read on \
                         the item's ledger; without it the answer is the one an unknown alias \
                         gets.",
                    )
                    .requires(CAP_READ)
                    .requires(ikigai_store::CAP_READ_GRAPH)
                    .input(scheme_input())
                    .input(value_input())
                    .output(crate::endpoints::PLAIN_FACE),
            ))
            .action(write_graph(
                ikigai_core::ActionSpec::new(Verb::Sink)
                    .summary(
                        "Give the item this alias. Only the scheme's owner may, holding \
                         `urn:cap:ledger:alias:{ledger}` or `urn:cap:ledger:write:{ledger}` on \
                         the item's ledger. Giving it again to the same item answers the same; \
                         an alias already naming another item is refused, naming that item.",
                    )
                    .requires(CAP_ANY)
                    .input(scheme_input())
                    .input(value_input())
                    .input(
                        ArgSpec::new("content")
                            .summary(
                                "The item: its full IRI, `urn:iki:ledger:{ledger}:item:{id}`, \
                                 or `{ledger}#12` (`#12` in the ledger called `default`). \
                                 `content`, so a pipe's value or a trailing word lands here.",
                            )
                            .class(v::ext::XSD_STRING),
                    )
                    .output(crate::endpoints::PLAIN_FACE),
            ))
            .action(write_graph(
                ikigai_core::ActionSpec::new(Verb::Delete)
                    .summary(
                        "Remove the alias. Only the scheme's owner may; removing one that \
                         names nothing is not an error.",
                    )
                    .input(scheme_input())
                    .input(value_input())
                    .output(crate::endpoints::PLAIN_FACE),
            ))
    }
}

/// The ledger a `content=` names on an alias resource, which has no ledger segment: an item
/// IRI's, `{ledger}#12`'s, or `default` for anything else (`#12`, `12`, an opaque id).
fn ledger_of(reference: &str) -> Result<Ledger> {
    if reference.starts_with("urn:") {
        return Ledger::item_iri(reference)
            .map(|(ledger, _)| ledger)
            .ok_or_else(|| Error::InvalidArgument {
                name: "content".to_string(),
                detail: format!(
                    "`{reference}` is not a ledger item IRI: an alias is given to \
                     `urn:iki:ledger:{{ledger}}:item:{{id}}` or `{{ledger}}#12`"
                ),
            });
    }
    match reference.split_once('#') {
        Some((name, _)) if !name.is_empty() => Ledger::parse(name),
        _ => Ok(Ledger::default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_round_trips_through_its_iri() {
        for value in [
            "01m6c5bjbm",
            "acme/web#12",
            "web#3f2",
            "job/88",
            "100%",
            "naïve",
        ] {
            let iri = alias_iri("gh", value);
            assert!(ikigai_core::Iri::parse(iri.clone()).is_ok(), "{iri}");
            let (scheme, raw) = split(&iri).expect("an alias IRI splits");
            assert_eq!(scheme, "gh");
            assert_eq!(parse_value(raw, "value").unwrap(), value);
        }
    }

    #[test]
    fn only_a_scheme_shaped_prefix_is_an_alias() {
        assert_eq!(split("gh:acme/web#12"), Some(("gh", "acme/web#12")));
        assert_eq!(split("kata:web#3f2"), Some(("kata", "web#3f2")));
        assert_eq!(split("key:urn:x"), None);
        assert_eq!(split("urn:iki:ledger:item:1"), None);
        assert_eq!(split("acme#12"), None);
        assert_eq!(split("12"), None);
        assert_eq!(split("Gh:x"), None);
        assert_eq!(split("gh:"), None);
    }

    #[test]
    fn a_bad_value_is_refused_with_what_is_accepted() {
        assert!(parse_value("a b", "value").is_err());
        assert!(parse_value("50%", "value").is_err());
        assert!(parse_value(&"x".repeat(MAX_VALUE + 1), "value").is_err());
        assert!(parse_scheme("key").is_err());
        assert!(parse_scheme("k:x").is_err());
    }
}
