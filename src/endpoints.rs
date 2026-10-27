//! The resources this crate binds, each of them **per ledger** but the three a host
//! configures (`ledgers`, `policy:{name}`, `lifecycle:{name}`). The lifecycle state and the
//! assertions are in the `flight` submodule.
//!
//! ```text
//! urn:iki:ledger:{ledger}:items          Source              the list, filtered
//! urn:iki:ledger:{ledger}:item:{id}      Source Sink Delete  one item, edit it, delete it
//! urn:iki:ledger:{ledger}:append         Sink                file a new item
//! urn:iki:ledger:{ledger}:comment        Sink                append a comment
//! urn:iki:ledger:{ledger}:close          Sink                close with a reason
//! urn:iki:ledger:{ledger}:reopen         Sink                undo a close
//! urn:iki:ledger:{ledger}:claim          Sink Delete         take it / hand it back
//! urn:iki:ledger:{ledger}:defer          Sink Delete         not now / now again
//! urn:iki:ledger:{ledger}:link           Sink Delete         blocks / parent / related
//! urn:iki:ledger:{ledger}:label          Sink Delete         tag / untag
//! urn:iki:ledger:{ledger}:purge          Delete              destroy, leaving a tombstone
//! urn:iki:ledger:{ledger}:next           Source              the ready set, ranked
//! urn:iki:ledger:{ledger}:doctor         Source              what is wrong; no repair
//! urn:iki:ledger:ledgers                 Source              which ledgers exist
//! urn:iki:ledger:policy:{name}           Source              what a policy weighs
//! urn:iki:ledger:lifecycle:{name}        Source Exists       the legal states, in order
//! urn:iki:ledger:{ledger}:item:{id}:state            Source Sink  one state; a CAS
//! urn:iki:ledger:{ledger}:item:{id}:state:{value}    Exists       is it in that state?
//! urn:iki:ledger:{ledger}:item:{id}:holder:{holder}  Exists       is it held by them?
//! urn:iki:ledger:{ledger}:item:{id}:closed           Exists       is it closed?
//! ```
//!
//! Omit the `{ledger}` segment and you address the ledger called `default`, through the
//! same grammar match and therefore the same capability — see [`crate::ledger`].
//!
//! `ledgers`, `policy:{name}` and `lifecycle:{name}` have no `{ledger}` segment because none
//! is a ledger's own state: `ledgers` is the inventory across them, and a policy or a
//! lifecycle is a property of the host's configuration.
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
//! The same reasoning is why the ledger is a name and not a `ledger=` argument: an
//! argument is a value, and a value cannot be what a capability binds to.
//!
//! # Capabilities: a wildcard is declared, an exact grant is enforced
//!
//! The ledger is in the IRI, and the kernel's capability pre-check runs before `invoke`
//! can read it. So each action declares the **family** — `urn:cap:ledger:write:*`,
//! meaning "holds some ledger write grant" — and the exact scope for the ledger actually
//! named is checked here, in `ledger_for`. Declared and enforced are the same scope;
//! the wildcard is the only form the pre-check can express. `ikigai-store`'s per-graph
//! write door is shaped identically, and for the same reason.
//!
//! ⚠ **The name goes LAST in the token** — `urn:cap:ledger:read:acme`, not
//! `urn:cap:ledger:acme:read` — because `ikigai-core` matches a wildcard only as a
//! trailing `*`. There is no infix form, so a parameter that is not last cannot be
//! declared as a family at all.
//!
//! # The store scopes every action also declares, and the boundary they now make
//!
//! A sub-request carries the **caller's** capability unchanged — `Invocation::issue` has
//! no attenuating or elevating form — so whatever this module asks the store for, the
//! caller must hold. It asks only the **narrow** doors (`urn:iki:store:graph-{select,ask}`
//! and `urn:iki:store:graph-update`, each naming one graph), so what a caller must hold is
//! `urn:cap:store:{read,write}:graph:urn:iki:ledger:graph:{ledger}` — plus the graveyard's
//! write token for delete and purge. Every action declares those families too, because an
//! action that enforces a scope it does not declare makes the manifold over-offer.
//!
//! ★ **So the ledger capabilities are a tenancy boundary now**, where before 0.2.0 they
//! segmented this module's own doors and the store underneath was one open room. The
//! one resource that asks a cross-graph question, `urn:iki:ledger:ledgers` (*which graphs
//! exist*), asks it of `urn:iki:store:graphs` and then reads each ledger through the narrow
//! door; until ledger #272 its root path resolved the broad `urn:iki:store:select`.
//!
//! ⚠ A host that hands a ledger caller `urn:cap:store:read` anyway has still given it every
//! graph in the store. Nothing here asks for it, so that is a configuration decision rather
//! than a requirement — the distinction, and both halves as tests, are in `README.md`.
//!
//! # Freshness
//!
//! Reads are `.cacheable()` and depend on the store's **three** write threads —
//! `urn:iki:store:{update,load,graph-update}` — the third because that is the door this
//! module's own writes go through. This module cuts **no thread of its own**, deliberately:
//! the kernel cuts the thread named after a mutating request's target, so
//! `urn:iki:ledger:append` is already cut on every append, and the store cuts its own on the
//! write this module actually performs. A ledger-specific thread could only be cut by
//! resolving `urn:kernel:cut`, which needs `urn:cap:kernel:cut` — authority this module has
//! no business holding for a name nothing would gain by.
//!
//! ⚠ The `depends_on` calls in `endpoints::face` are belt-and-braces, not the mechanism: a read here
//! is derived from a sub-request to the store, whose own representation declares all three
//! threads, and the kernel unions a dependency's threads into the derived one. Measured
//! 2026-09-13 — removing the declarations leaves every freshness test green. They stay
//! because they are correct and because the day a read stops going through the store is the
//! day they become load-bearing, but do not read that list as what keeps the cache honest.

use std::sync::Arc;

use async_trait::async_trait;
use ikigai_core::{
    space_iri, ArgSpec, Description, Endpoint, EndpointSpace, Error, Exact, Invocation, ReprType,
    Representation, Result, UriTemplate, Verb,
};
use oxrdf::Graph;
use sha2::{Digest, Sha256};

use crate::json;
use crate::ledger::{ItemPartGrammar, Ledger, LedgerGrammar};
use crate::lifecycle::Lifecycles;
use crate::model::{self, Deferred, Filter, Holder, Item, Status};
use crate::policy::{OrderingPolicy, Policies};
use crate::select;
use crate::sparql::{self, boolean, datetime, integer, literal, StoreClient};
use crate::vocabulary as v;

/// Reading a ledger, as **declared**: the family, meaning "holds some ledger read grant".
/// A held grant names one ledger — [`Ledger::cap_read`].
///
/// A ledger holds whatever anyone filed, so an unrestricted read is not free — the same
/// argument `ikigai-store` makes for gating its query face, now per partition.
pub const CAP_READ: &str = "urn:cap:ledger:read:*";

/// Every ordinary mutation — filing, commenting, closing, claiming, linking, labeling,
/// deferring, editing, saying what an item is about — as declared. A held grant names one
/// ledger: [`Ledger::cap_write`], and it still covers claiming, commenting and closing,
/// which since 0.7.0 also have grants of their own ([`CAP_CLAIM`], [`CAP_COMMENT`],
/// [`CAP_CLOSE`]), and adding or removing an `about`, which has had one since 0.10.0
/// ([`CAP_ABOUT`]).
pub const CAP_WRITE: &str = "urn:cap:ledger:write:*";

/// The family of the claim grant: taking, renewing and releasing claims, and nothing else.
/// A held grant names one ledger: [`Ledger::cap_claim`].
///
/// ⚠ **This is the family an operator GRANTS from, not what `claim` declares.** `claim`
/// must also admit a holder of `write:{ledger}` (no grant changes meaning), and
/// `ikigai-core` ANDs every `requires` and matches a family only as a trailing-`*` prefix,
/// so the one declaration both `claim:acme` and `write:acme` satisfy is their common
/// prefix, [`CAP_ANY`]. The exact either-or is enforced in `ledger_for`. The same holds
/// for [`CAP_COMMENT`] and [`CAP_CLOSE`].
pub const CAP_CLAIM: &str = "urn:cap:ledger:claim:*";

/// The family of the comment grant: appending a comment, and nothing else. A held grant
/// names one ledger: [`Ledger::cap_comment`]. Declared as [`CAP_ANY`]; see [`CAP_CLAIM`].
pub const CAP_COMMENT: &str = "urn:cap:ledger:comment:*";

/// The family of the close grant: closing and reopening, and nothing else. A held grant
/// names one ledger: [`Ledger::cap_close`]. Declared as [`CAP_ANY`]; see [`CAP_CLAIM`].
pub const CAP_CLOSE: &str = "urn:cap:ledger:close:*";

/// The family of the about grant: adding and removing an item's `ledger:about` edges at
/// `…:item:{id}:about`, and nothing else (ledger #1315). A held grant names one ledger:
/// [`Ledger::cap_about`]. Declared as [`CAP_ANY`]; see [`CAP_CLAIM`].
///
/// ★ **Why it is its own grant.** `about` is how an item names what it is filed against,
/// and a process that RECORDS where work went (a layer mirror noting the offer it published,
/// a hook noting the session that delivered) needs to write that edge without being able to
/// close the item. Before 0.10.0 the only grant that reached it was `write:{ledger}`, which
/// is the close grant too. ⚠ The worker grant does NOT include it: an about edge is visible
/// to every `items about=` query, so whoever issues grants decides who may add one.
///
/// The item Sink's own `about=` stays under [`CAP_WRITE`]: that door also edits the title,
/// body and priority, and a per-argument grant is not a thing a declaration can say.
pub const CAP_ABOUT: &str = "urn:cap:ledger:about:*";

/// "Holds some ledger grant": the floor `claim`, `comment`, `close`, `reopen` and
/// `item:{id}:about` DECLARE,
/// because it is the only family both the narrow grant and `write:{ledger}` satisfy (see
/// [`CAP_CLAIM`]). It over-offers: a caller holding only a read or a delete grant, and a
/// store graph write grant, sees those actions in its manifold and is refused at
/// `invoke` with a `Denied` naming both tokens that would do. Declared stays a superset of
/// enforced, never the reverse.
pub const CAP_ANY: &str = "urn:cap:ledger:*";

/// Removing an item from view. Separate from [`CAP_WRITE`] because a delete takes work
/// OUT of the ledger, and the everyday grant should not carry it.
pub const CAP_DELETE: &str = "urn:cap:ledger:delete:*";

/// Destroying an item's content irreversibly. Separate again, and the narrowest grant in
/// this module: a tombstone survives a purge, but nothing else does.
pub const CAP_PURGE: &str = "urn:cap:ledger:purge:*";

/// What an action needs from the ledger it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Need {
    Read,
    Write,
    Delete,
    Purge,
    /// Take, renew or release a claim: `claim:{ledger}`, or `write:{ledger}`.
    Claim,
    /// Append a comment: `comment:{ledger}`, or `write:{ledger}`.
    Comment,
    /// Close or reopen: `close:{ledger}`, or `write:{ledger}`.
    Close,
    /// Add or remove an item's `about` edges: `about:{ledger}`, or `write:{ledger}`.
    About,
}

impl Need {
    /// The exact grant for one ledger.
    fn scope(self, ledger: &Ledger) -> String {
        match self {
            Need::Read => ledger.cap_read(),
            Need::Write => ledger.cap_write(),
            Need::Delete => ledger.cap_delete(),
            Need::Purge => ledger.cap_purge(),
            Need::Claim => ledger.cap_claim(),
            Need::Comment => ledger.cap_comment(),
            Need::Close => ledger.cap_close(),
            Need::About => ledger.cap_about(),
        }
    }

    /// Whether `write:{ledger}` also satisfies it: the three verbs split out of the write
    /// grant in 0.7.0 and `about` in 0.10.0, which it still covers so that no grant issued
    /// before changed meaning.
    fn write_covers(self) -> bool {
        matches!(
            self,
            Need::Claim | Need::Comment | Need::Close | Need::About
        )
    }

    /// The family this crate declares for it.
    fn declared(self) -> &'static str {
        match self {
            Need::Read => CAP_READ,
            Need::Write => CAP_WRITE,
            Need::Delete => CAP_DELETE,
            Need::Purge => CAP_PURGE,
            Need::Claim | Need::Comment | Need::Close | Need::About => CAP_ANY,
        }
    }
}

/// The ledger a request named, once the caller's grant for **that ledger** is checked.
///
/// ★ This is the parameterized half of the capability, and it is the reason named
/// ledgers are a boundary rather than a filter. The kernel's pre-check saw only
/// [`Need::declared`] — that the caller holds *some* grant under the family — because the
/// ledger is in the IRI and the check runs before `invoke` can read it. Declared and
/// enforced name the same scope; only the exactness differs, and the exact half is here.
///
/// ★ **Claim, comment, close and about take either of two grants**: their own
/// (`claim:{ledger}`, `comment:{ledger}`, `close:{ledger}`, `about:{ledger}`) or
/// `write:{ledger}`, which meant all four before 0.7.0 and 0.10.0 split them out and still
/// does. A worker holding claim and comment is refused here
/// at close, label, edit and every other write, by name.
fn ledger_for(inv: &Invocation<'_>, need: Need) -> Result<Ledger> {
    let ledger = Ledger::from_bindings(inv.bindings)?;
    let scope = need.scope(&ledger);
    if inv.capability.allows(&scope) {
        return Ok(ledger);
    }
    if need.write_covers() {
        let write = ledger.cap_write();
        if inv.capability.allows(&write) {
            return Ok(ledger);
        }
        return Err(Error::Denied(format!(
            "this capability holds neither `{scope}` nor `{write}`, and this resource takes \
             either. A ledger grant names exactly one ledger, and a grant for another verb \
             (a worker's claim and comment, say) does not reach this one"
        )));
    }
    Err(Error::Denied(format!(
        "this capability does not hold `{scope}`. A ledger grant names exactly one \
         ledger — holding a grant over another ledger satisfies the declared family \
         `{}` but not this resource, which is the point of naming them",
        need.declared()
    )))
}

/// `text/plain` — the default face, and what "view ASAP" actually needs.
const PLAIN: &str = "text/plain";
/// `text/turtle` — the graph face.
const TURTLE: &str = "text/turtle";
/// `application/json` — the machine face, [`crate::json`].
const JSON: &str = json::MEDIA_TYPE;
/// The faces of the two reads that are not about items (`ledgers`, `policy:{name}`).
const FACES: [&str; 2] = [PLAIN, TURTLE];
/// The faces of the item reads: `items`, `item:{id}`, `next`.
const READ_FACES: [&str; 3] = [PLAIN, TURTLE, JSON];
/// The faces of a write's ANSWER: `append`, `comment`, `close`, `link`. No Turtle — an
/// answer is a receipt, not a graph, and the graph is one read away.
const WRITE_FACES: [&str; 2] = [PLAIN, JSON];

mod flight;

/// The name [`space`] claims: `urn:iki:space:ledger`.
pub const SPACE_ID: &str = "urn:iki:space:ledger";

/// Bind the ledger with the built-in ordering policies (`priority-recency`, then
/// `leverage`) and the built-in lifecycle (`kata-flight`).
///
/// ★ **The one constructor here that names itself** ([`SPACE_ID`]): it takes no
/// configuration and reads nothing while it is built, so every call holds the same doors.
/// [`space_with`] and [`space_with_policies`] stay anonymous — a host's policies, lifecycles
/// and claim stamper are different doors behind the same IRIs, so only the host that chose
/// them can name what it built. The name goes on LAST: binding a door after it drops it.
pub fn space() -> EndpointSpace {
    space_with(SpaceConfig::default()).named(space_iri("ledger"))
}

/// What a host configures when it binds the ledger: the ordering policies `next` offers and
/// the lifecycles item states are checked against. Built from [`SpaceConfig::default`] — the
/// built-ins — and handed to [`space_with`].
///
/// ★ A builder rather than another `space_with_*` function per knob, because the knobs keep
/// arriving (policies in 0.1, lifecycles in 0.5) and a function per combination is how an
/// API grows a flag day. It is `#[non_exhaustive]` and built through its methods, so the next
/// knob is additive.
#[derive(Clone)]
#[non_exhaustive]
pub struct SpaceConfig {
    policies: Vec<Arc<dyn OrderingPolicy>>,
    lifecycles: Lifecycles,
    claim_kind: crate::claim::ClaimKindStamper,
}

impl Default for SpaceConfig {
    fn default() -> Self {
        SpaceConfig {
            policies: Policies::default().all().to_vec(),
            lifecycles: Lifecycles::default(),
            claim_kind: crate::claim::machine_by_default(),
        }
    }
}

impl SpaceConfig {
    /// The ordering policies `next` offers. **The first is the default.**
    pub fn policies(mut self, policies: Vec<Arc<dyn OrderingPolicy>>) -> Self {
        self.policies = policies;
        self
    }

    /// The lifecycles, and which ledger uses which — see [`Lifecycles`].
    pub fn lifecycles(mut self, lifecycles: Lifecycles) -> Self {
        self.lifecycles = lifecycles;
        self
    }

    /// How a claim's kind — `machine`, `agent` or `person` — is decided: by the HOST, from the
    /// invocation (what its own door stamped on the request, the capability), and never from
    /// an argument the caller sends. The default stamps `machine` on every claim. See
    /// [`crate::claim`].
    pub fn claim_kind(mut self, stamper: crate::claim::ClaimKindStamper) -> Self {
        self.claim_kind = stamper;
        self
    }
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
    space_with(SpaceConfig::default().policies(policies))
}

/// Bind the ledger with a host's own configuration — see [`SpaceConfig`].
///
/// # Panics
///
/// On an empty policy list, as [`space_with_policies`] does.
pub fn space_with(config: SpaceConfig) -> EndpointSpace {
    let policies = Arc::new(Policies::new(config.policies));
    let lifecycles = Arc::new(config.lifecycles);
    // ⚠ **Bind order is resolution order** (`EndpointSpace` takes the first grammar that
    // matches), and the two ledger-less resources go FIRST. `urn:iki:ledger:policy:next`
    // would otherwise be read as the `next` resource of a ledger called `policy` — which
    // is also why `policy` and `ledgers` are in `ledger::RESERVED`. Putting them ahead
    // makes the tie deterministic instead of alphabetical.
    EndpointSpace::new()
        .bind(Exact::new("urn:iki:ledger:ledgers"), LedgersEndpoint)
        .bind(
            UriTemplate::parse("urn:iki:ledger:policy:{name}").expect("a constant template"),
            PolicyEndpoint {
                policies: Arc::clone(&policies),
            },
        )
        .bind(
            UriTemplate::parse("urn:iki:ledger:lifecycle:{name}").expect("a constant template"),
            flight::LifecycleEndpoint {
                lifecycles: Arc::clone(&lifecycles),
            },
        )
        .bind(LedgerGrammar::action("items"), ItemsEndpoint)
        .bind(LedgerGrammar::action("count"), CountEndpoint)
        // ⚠ The item PARTS go ahead of `item:{id}`, whose `{id}` takes the rest of the IRI:
        // bound after it, `item:12:state` would be read as an item whose id is `12:state`.
        .bind(
            ItemPartGrammar::part("state"),
            flight::StateEndpoint {
                lifecycles: Arc::clone(&lifecycles),
            },
        )
        .bind(
            ItemPartGrammar::with_value("state", "value"),
            flight::AssertionEndpoint {
                assertion: flight::Assertion::State,
                lifecycles: Arc::clone(&lifecycles),
            },
        )
        .bind(
            ItemPartGrammar::with_value("holder", "holder"),
            flight::AssertionEndpoint {
                assertion: flight::Assertion::Holder,
                lifecycles: Arc::clone(&lifecycles),
            },
        )
        .bind(
            ItemPartGrammar::part("closed"),
            flight::AssertionEndpoint {
                assertion: flight::Assertion::Closed,
                lifecycles: Arc::clone(&lifecycles),
            },
        )
        .bind(ItemPartGrammar::part("about"), AboutEndpoint)
        .bind(LedgerGrammar::with_id("item"), ItemEndpoint)
        .bind(LedgerGrammar::action("append"), AppendEndpoint)
        .bind(LedgerGrammar::action("comment"), CommentEndpoint)
        .bind(LedgerGrammar::action("close"), CloseEndpoint)
        .bind(LedgerGrammar::action("reopen"), ReopenEndpoint)
        .bind(
            LedgerGrammar::action("claim"),
            ClaimEndpoint {
                stamper: config.claim_kind,
            },
        )
        .bind(LedgerGrammar::action("defer"), DeferEndpoint)
        .bind(LedgerGrammar::action("link"), LinkEndpoint)
        .bind(LedgerGrammar::action("label"), LabelEndpoint)
        .bind(LedgerGrammar::action("purge"), PurgeEndpoint)
        .bind(
            LedgerGrammar::action("doctor"),
            flight::DoctorEndpoint {
                lifecycles: Arc::clone(&lifecycles),
            },
        )
        .bind(LedgerGrammar::action("next"), NextEndpoint { policies })
}

// ------------------------------------------------------------------------- helpers

/// A `text/plain` representation.
fn plain(text: impl Into<String>) -> Representation {
    Representation::new(
        ReprType::new(PLAIN).with_param("charset", "utf-8"),
        text.into().into_bytes(),
    )
}

/// An `application/json` representation — [`crate::json`]'s documents.
fn json_repr(bytes: Vec<u8>) -> Representation {
    Representation::new(ReprType::new(JSON), bytes)
}

/// A JSON read, cacheable under the same threads as every other read here.
fn json_face(bytes: Vec<u8>) -> Representation {
    cached(json_repr(bytes))
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
    Ok(cached(repr))
}

/// Cacheable, hung from the store's three write threads.
fn cached(repr: Representation) -> Representation {
    repr.cacheable()
        .depends_on(ikigai_store::UPDATE_THREAD)
        .depends_on(ikigai_store::LOAD_THREAD)
        // ★ The third writing IRI means a third thread, and this crate's own writes all go
        // through it. Depending only on the first two would leave every cacheable read
        // here serving stale bytes after every ledger write — silently, on the branch that
        // looks like success. `a_write_through_the_narrow_door_invalidates_a_cached_read`
        // in `tests/endpoints.rs` is what keeps it true.
        .depends_on(ikigai_store::GRAPH_UPDATE_THREAD)
}

/// Which face was asked for. An `as` this endpoint cannot serve is **refused**, never
/// substituted — the caller asked a question and a different answer is not a better one.
fn wanted_face(inv: &Invocation<'_>, faces: &[&'static str]) -> Result<&'static str> {
    match inv.inline_str("as").ok() {
        None => Ok(PLAIN),
        Some(asked) => faces
            .iter()
            .find(|face| bare(asked) == **face)
            .copied()
            .ok_or_else(|| Error::InvalidArgument {
                name: "as".to_string(),
                detail: format!(
                    "`{asked}` is not a face this resource serves; one of {}",
                    faces.join(", ")
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

/// Crockford base32, lowercase: no `i`, `l`, `o` or `u`, so an id read aloud or retyped
/// cannot turn into a different one.
const CROCKFORD: &[u8] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// Mint an opaque, time-ordered id for an item or a comment.
///
/// ★ **Identity is the IRI; `#12` is a label on it.** The id is 10 characters of
/// Crockford base32 over the low 48 bits of the millisecond clock — so ids sort in
/// filing order, which makes a listing of raw IRIs readable — followed by 13 characters
/// that make it **unique by construction**.
///
/// # ⚠ Why the second half is a sequence and not a digest of the content
///
/// Until 0.2.1 it was 6 characters of SHA-256 over `(millisecond, title, author)`, on the
/// premise that "one store has one writer, so the clock alone is unique here". One writer
/// *process* makes many writes per millisecond: eight concurrent appends with the same
/// title minted ONE IRI between them (each `INSERT` landing on the same subject, one item
/// with eight numbers), and the same note left on two items minted one comment node with
/// two `ledger:onItem` edges — which then died with whichever item was deleted first. A
/// fixed clock, the kind every replay harness uses, did it with two sequential calls.
///
/// So the suffix is `nonce + sequence`: a process-wide counter that every mint
/// increments, offset by a per-process random nonce. Within one process two mints cannot
/// collide at all — the map from sequence to suffix is a bijection on 64 bits. The nonce
/// is what keeps two *processes* apart (two ledgers merged later, a restart under a fixed
/// clock), and there it is a 64-bit chance rather than a guarantee, which is the honest
/// strength of anything that does not consult the store.
///
/// Ids minted before 0.3 keep the 16-character shape and stay valid: nothing parses an id,
/// it is only ever compared whole.
///
/// Fallible only because the nonce is: see [`process_nonce`].
fn mint_id(now: u64) -> Result<String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let suffix = process_nonce()?.wrapping_add(SEQUENCE.fetch_add(1, Ordering::Relaxed));
    let mut id = String::with_capacity(23);
    for shift in (0..10).rev() {
        id.push(CROCKFORD[((now >> (shift * 5)) & 31) as usize] as char);
    }
    // 13 × 5 = 65 bits, so the top character carries the high bit and every suffix is
    // distinct — the property the doc above relies on.
    for shift in (0..13).rev() {
        id.push(CROCKFORD[((suffix >> (shift * 5)) & 31) as usize] as char);
    }
    Ok(id)
}

/// A random number fixed for the life of the process (or of the wasm instance).
///
/// Drawn once, on the first mint. If two first mints race, both draw and one value wins;
/// every mint after that sees the winner, which is all "fixed for the life of the
/// process" has to mean.
fn process_nonce() -> Result<u64> {
    static NONCE: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    if let Some(nonce) = NONCE.get() {
        return Ok(*nonce);
    }
    let drawn = entropy()?;
    Ok(*NONCE.get_or_init(|| drawn))
}

/// Native (and WASI): `RandomState`, the one source of randomness in `std` that needs no
/// dependency and no filesystem. Its keys are seeded from the operating system once per
/// thread, so hashing a constant through it yields a per-process nonce.
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
fn entropy() -> Result<u64> {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    // What is hashed does not matter; the keys are the randomness.
    hasher.write_u8(0);
    Ok(hasher.finish())
}

/// The browser (`wasm32-unknown-unknown`): `getrandom`, over the JS `crypto` object.
///
/// ⚠ **Not `RandomState` here: on this target its keys are not random.** `std` has no
/// entropy source on `wasm32-unknown-unknown`, so `RandomState` is seeded from values that
/// are the same in every instance of the same module. Before 0.6.1 this function did not
/// exist and every wasm load drew the SAME nonce: two loads filing one item under one
/// clock reading minted the same IRI (ledger #797, reproduced under node). A failed draw is
/// refused rather than papered over, because a constant fallback is exactly that defect.
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
fn entropy() -> Result<u64> {
    getrandom::u64().map_err(|e| {
        Error::Endpoint(format!(
            "this host has no entropy source, so a ledger id cannot be minted safely: \
             getrandom failed ({e}). In a browser or node this is the JS `crypto` object"
        ))
    })
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

/// Resolve an item reference — `#12`, `12`, `acme#12`, an opaque id, or a full IRI — to
/// its IRI, and fail with a sentence naming what was looked for when there is no such
/// item.
///
/// ★ **Every reference resolves inside THIS client's ledger, and a reference to another
/// one is refused rather than looked up.** That is where the cross-ledger question is
/// actually settled: a `blocks` edge to an item in another ledger cannot be filed,
/// because the object cannot be named here. The reasoning is in
/// [`LinkEndpoint::describe`]; the refusal is here, once, for every endpoint that takes
/// an item.
async fn require_item(client: &StoreClient<'_, '_>, reference: &str) -> Result<Item> {
    let iri = item_iri(client, reference, "item").await?;
    if let Some(item) = model::load_item(client, &iri).await? {
        return Ok(item);
    }
    // Present but unreadable is a different fact from absent, and only one of them is
    // fixable by the person reading the error.
    let wrong = model::defects_of(client, &iri).await?;
    if wrong.is_empty() {
        Err(Error::NotFound(format!("no ledger item at `{iri}`")))
    } else {
        Err(Error::Endpoint(format!(
            "`{iri}` is in the ledger graph but nothing here can read it: {}. Some write \
             did not pass through a ledger Sink (an editor, a merge, a bulk load); fix \
             {} or delete the subject",
            wrong.join(", "),
            if wrong.len() == 1 {
                "that property"
            } else {
                "those properties"
            }
        )))
    }
}

/// The IRI an item reference names in this ledger — a number (`12`, `#12`), a key
/// (`key:…`), an opaque id or an item IRI — without reading the item. `arg` is the argument
/// a refusal names (`item`, or `parent` for the filter). An unknown number or key is
/// `NotFound`; an item of another ledger is refused.
async fn item_iri(client: &StoreClient<'_, '_>, reference: &str, arg: &str) -> Result<String> {
    let here = client.ledger();
    Ok(if reference.starts_with("urn:") {
        match Ledger::item_iri(reference) {
            Some((other, _)) if &other != here => {
                return Err(Error::InvalidArgument {
                    name: arg.to_string(),
                    detail: format!(
                        "`{reference}` is an item of the ledger `{}`, and this resource is \
                         `{}`. A ledger is a boundary: an item is read, changed and linked \
                         within its own ledger, and `ledger:about` is how a reference \
                         crosses one. Address it at `{}`",
                        other.name(),
                        here.name(),
                        other.prefix()
                    ),
                })
            }
            // ★ The id tail goes through the same resolution the `item:{id}` resource
            // gives it, so `urn:iki:ledger:item:1` — which RESOLVES as a resource, the
            // number in the id position — means #1 here too. Until 0.2.1 it was looked up
            // verbatim as an item minted with the id `1`, and was not found.
            // `split_once`: a ledger name cannot contain `:item:`, and a key after it can.
            Some((_, canonical)) => match canonical.split_once(":item:") {
                Some((_, tail)) => model::resolve_id(client, tail).await?,
                None => canonical,
            },
            // Not a ledger item IRI. Left verbatim so the error can name what was asked
            // for rather than a guess at what was meant.
            None => reference.to_string(),
        }
    } else {
        model::resolve_id(client, reference).await?
    })
}

/// Several SPARQL operations as ONE update request.
///
/// ★ Not a round-trip optimization (though it is one): a state change that takes three
/// statements can be observed half-applied between them — an item closed but with no
/// reason, a claim held by nobody. A SPARQL 1.1 update request is a SEQUENCE of
/// operations and the store applies the request as one, so the intermediate states are
/// never visible and a parse error in the third operation means the first two did not
/// happen either.
fn batch(operations: &[String]) -> String {
    operations
        .iter()
        .filter(|op| !op.trim().is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(" ;\n")
}

/// Remove every value of a property.
fn retract(client: &StoreClient<'_, '_>, item: &str, predicate: &str) -> String {
    let pattern = client.in_graph(&format!("<{item}> <{predicate}> ?was"));
    format!("DELETE {{ {pattern} }} WHERE {{ {pattern} }}")
}

/// The pattern "`item` is a live item": the condition every write on an item carries in its
/// `WHERE` (ledger #764).
///
/// ★ **A write reads its item and then writes, and a delete can commit in between.** Until
/// 0.12.0 a write's update carried no condition on the item, so one that lost that race
/// landed on the deleted IRI anyway: a comment, a `modified` stamp, a status or an edge, live
/// beside the tombstone, read by nothing and counted by nothing (`tests/write_after_delete.rs`
/// forces the schedule for every write). With this clause in the same update the store
/// evaluates it under its write lock, so the write lands while the item is live or not at
/// all. A `DELETE` alone needs no clause: removing a triple a delete already removed leaves
/// nothing behind.
fn live(item: &str) -> String {
    format!("<{item}> <{}> <{}> .", v::ext::TYPE, v::ITEM_CLASS)
}

/// `INSERT` the given triples into this ledger's graph only while every one of `items` is a
/// live item: the guarded form of `INSERT DATA` for a write on an item (see [`live`]).
fn insert_on_live(client: &StoreClient<'_, '_>, items: &[&str], triples: &str) -> String {
    let guard: String = items.iter().map(|item| live(item)).collect();
    format!(
        "INSERT {{ {} }} WHERE {{ {} }}",
        client.in_graph(triples),
        client.in_graph(&guard)
    )
}

/// Apply a write on `items` and refuse when any of them is no longer live afterwards.
///
/// ★ The update's own [`live`] clause is what keeps a late write off a deleted item; this is
/// how the CALLER learns that it happened, since an update that matched nothing still
/// succeeds. A write that lost the race to a delete is answered `NotFound`, the answer the
/// same request gets once the delete has committed, rather than a success for a write that
/// is in no live graph. ⚠ The read after cannot tell "landed, then the delete took it" from
/// "did not land": in the first case the write was archived with the item, which is the
/// delete's to account for, so both are reported the same way.
async fn guarded_update(client: &StoreClient<'_, '_>, items: &[&str], update: &str) -> Result<()> {
    client.update(update).await?;
    for item in items {
        still_live(client, item).await?;
    }
    Ok(())
}

/// `NotFound` naming the in-flight delete when `item` is not a live item now.
async fn still_live(client: &StoreClient<'_, '_>, item: &str) -> Result<()> {
    if client
        .ask(&format!("ASK {{ {} }}", client.in_graph(&live(item))))
        .await?
    {
        return Ok(());
    }
    Err(Error::NotFound(format!(
        "no ledger item at `{item}`: it was deleted while this write was in flight, and \
         nothing of the write is in the live graph"
    )))
}

/// Stamp an item as modified, as part of the same update that changed it — only while it is
/// a live item ([`live`]), so a late write never stamps a deleted IRI.
fn touch(client: &StoreClient<'_, '_>, item: &str, now: u64) -> String {
    format!(
        "DELETE {{ {} }} INSERT {{ {} }} WHERE {{ {} }}",
        client.in_graph(&format!("<{item}> <{}> ?was", v::ext::MODIFIED)),
        client.in_graph(&format!(
            "<{item}> <{}> {}",
            v::ext::MODIFIED,
            datetime(now)
        )),
        client.in_graph(&format!(
            "{} OPTIONAL {{ <{item}> <{}> ?was }}",
            live(item),
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
    write_keyed_comment(client, item, text, author, now, None, Handback::Unchanged).await
}

/// What a comment does to its item's hand-back state (`ledger:awaiting`, ledger #1199).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Handback {
    /// Nothing: an ordinary comment from a worker.
    Unchanged,
    /// `handback=true`: the item now awaits its dispatcher.
    Set,
    /// `rejects=true`, or any comment from a caller holding the close grant: the item no
    /// longer awaits anyone.
    Clear,
}

/// Whether this caller is the ledger's DISPATCHER: holds `close:{ledger}` or
/// `write:{ledger}` (root holds both). The one who checks a hand-back's evidence, so any
/// comment of theirs clears it, and whose override releases any claim.
fn is_dispatcher(inv: &Invocation<'_>, ledger: &Ledger) -> bool {
    inv.capability.allows(&ledger.cap_close()) || inv.capability.allows(&ledger.cap_write())
}

/// The update that applies a [`Handback`] to `item`, carrying `guard` (a keyed comment's
/// "the key is free" clause, and a hand-back's "the item is open") so it lands exactly when
/// the comment does. Empty for [`Handback::Unchanged`].
fn handback_update(
    client: &StoreClient<'_, '_>,
    item: &str,
    handback: Handback,
    guard: &str,
    now: u64,
) -> String {
    // Keep the latest decision even after clearing: legacy evidence before an answer
    // must not reappear after a close/reopen or a rejection.
    let was = client.in_graph(&format!(
        "<{item}> <{}> ?awaiting . <{item}> <{}> ?at",
        v::AWAITING,
        v::AWAITING_AT
    ));
    let optional = client.in_graph(&format!(
        "OPTIONAL {{ <{item}> <{}> ?awaiting }} OPTIONAL {{ <{item}> <{}> ?at }}",
        v::AWAITING,
        v::AWAITING_AT
    ));
    let stamp = format!("<{item}> <{}> {}", v::AWAITING_AT, datetime(now));
    match handback {
        Handback::Unchanged => String::new(),
        Handback::Set => format!(
            "DELETE {{ {was} }} INSERT {{ {} }} WHERE {{ {optional} {guard} }}",
            client.in_graph(&format!(
                "<{item}> <{}> <{}> . {stamp}",
                v::AWAITING,
                v::iri::AWAITING_DISPATCHER
            )),
        ),
        Handback::Clear => format!(
            "DELETE {{ {was} }} INSERT {{ {} }} WHERE {{ {optional} {guard} }}",
            client.in_graph(&stamp),
        ),
    }
}

/// [`write_comment`] with an optional key (ledger #1178): with one, the comment is written
/// only while no comment in this ledger carries that key, and the modified stamp only with
/// it.
///
/// ★ **The check is a clause of the write**, the keyed append's shape: both operations of
/// the batch carry `FILTER NOT EXISTS` over the key, and the store holds its write lock
/// across one update request, so of two writers racing with one key exactly one writes. The
/// answer is the IRI minted here whether or not it was written; the caller reads the key
/// back to learn which comment it names.
async fn write_keyed_comment(
    client: &StoreClient<'_, '_>,
    item: &str,
    text: &str,
    author: Option<&str>,
    now: u64,
    key: Option<&str>,
    handback: Handback,
) -> Result<String> {
    let id = mint_id(now)?;
    let comment = client.ledger().comment(&id);
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
    // ★ The hand-back is in the SAME update as the comment (ledger #1199), so the evidence
    // and the state it puts the item in land together or not at all.
    if key.is_none() && handback != Handback::Set {
        guarded_update(
            client,
            &[item],
            &batch(&[
                insert_on_live(client, &[item], &triples),
                touch(client, item, now),
                handback_update(client, item, handback, "", now),
            ]),
        )
        .await?;
        return Ok(comment);
    }
    // ★ A hand-back lands only on an OPEN item (ledger #1418): the endpoint refuses a closed
    // one before writing, and this clause holds against a close that lands between that read
    // and this write, so the comment and its `awaiting` are both written or neither is.
    let open = if handback == Handback::Set {
        client.in_graph(&format!("<{item}> <{}> <{}>", v::STATUS, v::OPEN))
    } else {
        String::new()
    };
    let free = match key {
        Some(key) => {
            triples.push_str(&format!(
                "\n<{comment}> <{}> {} .",
                v::COMMENT_KEY,
                literal(key)
            ));
            format!(
                "FILTER NOT EXISTS {{ {} }}",
                client.in_graph(&format!("?taken <{}> {}", v::COMMENT_KEY, literal(key)))
            )
        }
        None => String::new(),
    };
    // And only on a live item (ledger #764), like every write on one.
    let free = format!("{} {open} {free}", client.in_graph(&live(item)));
    // The stamp and the hand-back first: once the comment is in, the key is taken and the
    // guard is false, so a replay changes neither.
    client
        .update(&batch(&[
            format!(
                "DELETE {{ {} }} INSERT {{ {} }} WHERE {{ {} {free} }}",
                client.in_graph(&format!("<{item}> <{}> ?was", v::ext::MODIFIED)),
                client.in_graph(&format!(
                    "<{item}> <{}> {}",
                    v::ext::MODIFIED,
                    datetime(now)
                )),
                client.in_graph(&format!(
                    "OPTIONAL {{ <{item}> <{}> ?was }}",
                    v::ext::MODIFIED
                )),
            ),
            handback_update(client, item, handback, &free, now),
            format!(
                "INSERT {{ {} }} WHERE {{ {free} }}",
                client.in_graph(&triples)
            ),
        ]))
        .await?;
    still_live(client, item).await?;
    Ok(comment)
}

/// The comment [`write_comment`] just wrote, as the JSON face carries it — built from what
/// was written rather than read back, because the write is the authority on what it said.
fn written(iri: &str, item: &str, text: &str, author: Option<&str>, now: u64) -> json::Comment {
    json::comment(&model::Comment {
        iri: iri.to_string(),
        on_item: item.to_string(),
        body: text.to_string(),
        author: author.map(str::to_string),
        created: now,
    })
}

/// Every scope an action that reads the ledger transitively needs: this module's own read
/// grant, and the store's **per-graph** read family.
///
/// ★ **The per-graph family, never the broad `urn:cap:store:read`.** A read here is one
/// `urn:iki:store:graph-select` naming this ledger's graph, so the grant a caller must
/// actually hold is `urn:cap:store:read:graph:urn:iki:ledger:graph:{name}` — which grants
/// nothing over any other ledger. The wildcard is the *declared* half (the kernel's
/// pre-check runs before an endpoint can read the ledger out of its own IRI); the exact
/// half is enforced by the store, against the graph named in the sub-request.
///
/// ⚠ **One list, because there are two authoring forms and they drifted.** A read is
/// declared flatly by [`read_scopes`] on the single-authority endpoints and per-verb by
/// [`read_action`] on `item:{id}`, whose four verbs carry three different authorities —
/// and until 0.2.1 the second spelled the store's half `ikigai_store::CAP_READ`, the broad
/// dataset token. A caller holding exactly the grant list `README.md` publishes could list
/// a ledger and file into it and was **denied reading a single item out of it**, with the
/// only workaround being the whole-dataset grant this crate exists to stop needing.
/// Neither form gets its own literal again, and `tests/grants.rs` fails if either drops a
/// scope the store really enforces.
const READ_SCOPES: [&str; 2] = [CAP_READ, ikigai_store::CAP_READ_GRAPH];

/// [`READ_SCOPES`] on a flatly-authored description — the endpoints whose every verb is a
/// read (`items`, `next`, `ledgers`).
fn read_scopes(desc: Description) -> Description {
    READ_SCOPES
        .iter()
        .fold(desc, |desc, scope| desc.requires(*scope))
}

/// [`READ_SCOPES`] on one explicitly-authored action — `item:{id}`, which cannot declare
/// flatly because its `Source` and `Exists` read where its `Sink` writes and its `Delete`
/// deletes.
fn read_action(spec: ikigai_core::ActionSpec) -> ikigai_core::ActionSpec {
    READ_SCOPES
        .iter()
        .fold(spec, |spec, scope| spec.requires(*scope))
}

/// The store scopes a mutating action transitively needs. Every write here reads first
/// (to resolve `#12`, to check the item exists), so both store scopes are real.
///
/// ⚠ **Delete and purge need a write grant for a SECOND graph** — the ledger's graveyard
/// — because archiving is a write into it, and purge needs its READ grant too, because it
/// finds what a delete archived. One family covers each in the declaration; the operator's
/// grant list does not, and the README's grant table says so per verb.
fn write_scopes(spec: ikigai_core::ActionSpec, own: &str) -> ikigai_core::ActionSpec {
    spec.requires(own)
        .requires(ikigai_store::CAP_READ_GRAPH)
        .requires(ikigai_store::CAP_WRITE_GRAPH)
}

/// The `{ledger}` binding every per-ledger resource declares.
///
/// ★ Declared as a binding with a DEFAULT, which is exactly what the bare-form sugar is:
/// the segment may be absent, and absent means `default`. A caller reading only the
/// contract — the engine, the MCP projection, an agent — can form either spelling from
/// it, and a catalog probe expands the template with the default rather than a
/// placeholder.
/// The `ledger` ArgSpec of the resource at `urn:iki:ledger:{ledger}:{path}`.
///
/// ★ **`path` is THIS resource's own tail, never a sibling's.** Until 0.6.1 the summary was
/// one fixed string naming `urn:iki:ledger:append` as "the" default-ledger form, on every
/// endpoint that shares this helper — so the manifold told a reader of `comment` to write to
/// `append`, and one did: the comment became a junk item whose title was the argument string
/// (ledger #425). A summary is contract an agent is handed, not decoration.
/// `every_ledger_summary_names_the_resource_it_is_on` in `tests/contract_and_paging.rs` holds
/// each binding's summary to that binding's own pattern.
fn ledger_arg(path: &str) -> ArgSpec {
    ArgSpec::new("ledger")
        .summary(format!(
            "Which ledger: the `{{ledger}}` segment of `urn:iki:ledger:{{ledger}}:{path}`. Omit \
             the segment entirely (`urn:iki:ledger:{path}`) for the ledger called `default` — \
             the same resource under a shorter name, gated by the same capability. Lowercase \
             letters, digits, `-` and `_`."
        ))
        .class(v::ext::XSD_STRING)
        .default_value(crate::ledger::DEFAULT)
        .binding()
        .optional()
}

/// The `as` ArgSpec, over the faces this action really serves.
fn as_arg(faces: &[&'static str]) -> ArgSpec {
    let json = if faces.contains(&JSON) {
        " `application/json` is the versioned machine contract (`\"schema\": 1`, see the \
         crate's `json` module); the plain face is for people and is not a format to parse."
    } else {
        ""
    };
    ArgSpec::new("as")
        .summary(format!(
            "The face to serve; one of {}. An `as` this resource cannot answer in is \
             refused, never substituted.{json}",
            faces.join(", ")
        ))
        .class(v::ext::XSD_STRING)
        .one_of(faces.iter().copied())
        .default_value(PLAIN)
        .optional()
}

/// The `item` ArgSpec: a short number, an opaque id, or a full IRI.
///
/// Shared by every action that takes `item=`, so its summary names only what is true of all
/// of them: the item IRI in its named form, with the short spelling marked as the `default`
/// ledger's (the class of ledger #425 — a shared summary naming one concrete case).
fn item_arg(summary: &str) -> ArgSpec {
    ArgSpec::new("item")
        .summary(format!(
            "{summary} Accepts `#12`, `12`, an opaque id, `key:{{key}}` (the caller's own \
             name, from `append key=`), or the item's full IRI, \
             `urn:iki:ledger:{{ledger}}:item:{{id}}` (`urn:iki:ledger:item:{{id}}` in the \
             ledger called `default`) — a human types the number and a machine carries the \
             IRI. An item of another ledger is refused, never looked up here."
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
        let client = StoreClient::new(inv, ledger_for(inv, Need::Read)?);
        let want = wanted_face(inv, &READ_FACES)?;
        let mut filter = listing_filter(inv, &client).await?;
        filter.limit = parse_limit(inv, 50)?;
        filter.offset = parse_count(inv, "offset", 0)?;
        let items = model::load_items(&client, &filter).await?;
        if want == TURTLE {
            let mut graph = Graph::new();
            for item in &items {
                item.triples(&mut graph);
            }
            return face(String::new(), Some(graph), want);
        }
        if want == JSON {
            let rows: Vec<&Item> = items.iter().collect();
            let rendered = json::items(&client, &rows).await?;
            let doc = json::ItemsDocument {
                schema: json::SCHEMA,
                ledger: client.ledger().name().to_string(),
                count: rendered.len(),
                total: Some(model::count_items(&client, &filter).await?),
                offset: filter.offset,
                items: rendered,
                unreadable: model::defects(&client)
                    .await?
                    .into_iter()
                    .map(|(iri, defects)| json::Unreadable { iri, defects })
                    .collect(),
            };
            return Ok(json_face(json::render(&doc)?));
        }
        // ★ **The footer states the PAGE and the TOTAL, and they are different numbers**
        // (ledger #419). Until 0.6.1 it said `50 item(s)` — the length of a page worded as a
        // count of items — over a ledger of 385, and a reader reported the ledger as seven
        // times smaller than it was. The total is a COUNT query, never `items.len()`, and a
        // page smaller than the set names the argument that reaches the rest. The whole set
        // on one page keeps the old `N item(s)` line, which is grepped.
        let total = model::count_items(&client, &filter).await?;
        let limit = model::page_limit(&filter);
        let mut text = if total == 0 {
            "no items match\n".to_string()
        } else if items.is_empty() {
            format!(
                "no items on this page: {total} item(s) match, and offset={} is past the \
                 last of them\n",
                filter.offset
            )
        } else {
            let mut out: String = items
                .iter()
                .map(|item| format!("{}\n", item.line()))
                .collect();
            // ★ Saturating (ledger #1420): `limit` and `offset` take any count, and the store
            // answers `LIMIT 18446744073709551615`, so an unchecked sum panicked in a debug
            // build and, in a release one, wrapped into a footer that sent the reader back to
            // the start.
            let last = filter.offset.saturating_add(limit).min(total);
            if filter.offset == 0 && last == total {
                out.push_str(&format!("\n{total} item(s)\n"));
            } else {
                out.push_str(&format!(
                    "\n{}–{last} of {total} item(s)",
                    filter.offset.saturating_add(1)
                ));
                if last < total {
                    out.push_str(&format!(
                        " — pass offset={last} for the next page, or a larger limit="
                    ));
                }
                out.push('\n');
            }
            out
        };
        // ★ The corpus check, on the read side. Anyone holding the store's write scope can
        // change this graph without passing through a ledger Sink — an editor, a merge, a
        // bulk load — and that is a supported path, not corruption. What is NOT supported
        // is an item quietly disappearing from every listing because a hand edit left it
        // unreadable, so a malformed item is REPORTED here rather than skipped.
        // Listed, but read around: a stand-in timestamp is shown above, so say which items
        // carry one. The line format above is untouched (it is grepped); this is a footer.
        let flawed: Vec<&Item> = items.iter().filter(|i| !i.defects.is_empty()).collect();
        if !flawed.is_empty() {
            text.push_str(&format!(
                "\n⚠ {} item(s) listed above carry a timestamp nothing here can read; each is \
                 listed with its other timestamp standing in (a write that did not pass \
                 through a ledger Sink):\n",
                flawed.len()
            ));
            for item in flawed {
                text.push_str(&format!(
                    "  {} {} — {}\n",
                    item.short(),
                    item.iri,
                    item.defects.join(", ")
                ));
            }
        }
        let broken = model::defects(&client).await?;
        if !broken.is_empty() {
            text.push_str(&format!(
                "\n⚠ {} item(s) in the graph could not be read and are not listed above \
                 (a write that did not pass through a ledger Sink):\n",
                broken.len()
            ));
            for (iri, wrong) in &broken {
                text.push_str(&format!("  {iri} — {}\n", wrong.join(", ")));
            }
        }
        face(text, None, want)
    }

    fn name(&self) -> &str {
        "ledger-items"
    }

    fn describe(&self) -> Description {
        read_scopes(
            filter_inputs(
                Description::new("ledger-items")
                    .title("The ledger, filtered")
                    .summary(
                        "Every item matching the filter, most recently updated first. The \
                         filters are evaluated in the query, which is why the ordering policy \
                         behind `urn:iki:ledger:next` can stay small.",
                    )
                    .verb(Verb::Source)
                    .verb(Verb::Meta)
                    .input(ledger_arg("items")),
            )
            .input(
                ArgSpec::new("limit")
                    .summary(
                        "At most this many items on the page (default 50; `0` means 500). \
                         A page is not the set: the plain face's footer and the JSON \
                         face's `total` give how many items match, and `offset=` reaches \
                         the rest. The Turtle face is the page's items alone. For the total \
                         alone, read `urn:iki:ledger:count` with the same filter: it skips \
                         the page and the corpus check.",
                    )
                    .class(v::ext::XSD_INTEGER)
                    .default_value("50")
                    .optional(),
            )
            .input(
                ArgSpec::new("offset")
                    .summary(
                        "How many matching items to skip, in the listing's order (default \
                         0); the next page is at `offset` plus `limit`. ⚠ The order is \
                         most recently updated first, so an item updated between two page \
                         reads moves to the front and is missed or repeated: when the \
                         whole set matters, read it as one page.",
                    )
                    .class(v::ext::XSD_INTEGER)
                    .default_value("0")
                    .optional(),
            )
            .input(as_arg(&READ_FACES))
            .output(PLAIN)
            .output(TURTLE)
            .output(JSON),
        )
    }
}

// --------------------------------------------------------------------------- count

/// The faces of `count`: the number, and the number as a [`json::CountDocument`]. No Turtle —
/// a count is not a graph, and the vocabulary has no term for it.
const COUNT_FACES: [&str; 2] = [PLAIN, JSON];

/// `urn:iki:ledger:{ledger}:count` — how many items a filter admits, and nothing else
/// (ledger #1128).
///
/// ★ **The total has its own name because reading it through `items` paid for everything
/// else.** gonk's ledger page read `items limit=1 as=application/json` for its `total`, and
/// that face always ran the corpus check ([`model::defects`]): two scans of the whole graph,
/// returning a row per item per timestamp and number, to fill an `unreadable` list the caller
/// never read. Measured over an uncached kernel at 3000 items, the check was ~60% of the read
/// and the COUNT ~6% (`tests/listing_cost.rs`). The listing faces keep the check, unchanged:
/// they show what it finds. This is the door for a caller that wants only the number.
///
/// The same filter, parsed by the same function ([`listing_filter`]), counted by the same
/// query ([`model::count_items`]) — so this is `items`' `total` for the same arguments, an
/// item a hand edit made unreadable included (counted there too, reported only by the
/// listing's footer). `tests/count.rs` holds the two equal over every filter.
#[derive(Clone)]
struct CountEndpoint;

#[async_trait]
impl Endpoint for CountEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        if inv.request.verb != Verb::Source {
            return Err(unsupported("ledger-count", inv.request.verb));
        }
        let client = StoreClient::new(inv, ledger_for(inv, Need::Read)?);
        let want = wanted_face(inv, &COUNT_FACES)?;
        let filter = listing_filter(inv, &client).await?;
        let total = model::count_items(&client, &filter).await?;
        if want == JSON {
            return Ok(json_face(json::render(&json::CountDocument {
                schema: json::SCHEMA,
                ledger: client.ledger().name().to_string(),
                total,
            })?));
        }
        face(format!("{total}\n"), None, want)
    }

    fn name(&self) -> &str {
        "ledger-count"
    }

    fn describe(&self) -> Description {
        read_scopes(
            filter_inputs(
                Description::new("ledger-count")
                    .title("How many items match")
                    .summary(
                        "How many items the filter admits: `items`' total for the same \
                         arguments, without the page and without the corpus check, so it \
                         costs one COUNT query. The plain face is the bare number.",
                    )
                    .verb(Verb::Source)
                    .verb(Verb::Meta)
                    .input(ledger_arg("count")),
            )
            .input(as_arg(&COUNT_FACES))
            .output(PLAIN)
            .output(JSON),
        )
    }
}

/// The filter's inputs, in the order `items` has always declared them: `status`, `kind`,
/// `labels`, `without`, `about`, `parent` (0.12.0), `key`, `holder`, `deferred`, `text` — what
/// [`listing_filter`] parses, declared once for `items` and `count`.
fn filter_inputs(desc: Description) -> Description {
    desc.input(
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
        ArgSpec::new("parent")
            .summary(
                "An item (`12`, `#12` or its IRI): only its children, the items \
                 linked to it with `link type=parent`. Subject to `status` like every \
                 filter here, so open children by default and `status=all` for every \
                 child. An unknown item is not found, never an empty answer.",
            )
            .class(v::ext::XSD_STRING)
            .optional(),
    )
    .input(
        ArgSpec::new("key")
            .summary(
                "The caller's own name for an item (`append key=`). At most one \
                 item in a ledger carries a key, so this finds it — subject to \
                 `status` like every filter here (`status=all` to include a \
                 closed one). `urn:iki:ledger:item:key:{key}` is the same lookup \
                 as a resource.",
            )
            .class(v::ext::XSD_STRING)
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
}

/// The filter `items` and `count` share — every argument but the page (`limit`, `offset`),
/// which only a listing has. One parser, so `count`'s number is the listing's `total` for the
/// same arguments, refusals included.
async fn listing_filter(inv: &Invocation<'_>, client: &StoreClient<'_, '_>) -> Result<Filter> {
    // Resolved, so `parent=12`, `parent=#12` and the IRI mean one item, as `item=` does.
    let parent = match inv.inline_str("parent").ok().map(str::trim) {
        None | Some("") => None,
        Some(reference) => Some(item_iri(client, reference, "parent").await?),
    };
    Ok(Filter {
        parent,
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
        key: inv
            .inline_str("key")
            .ok()
            .map(model::parse_key)
            .transpose()?,
        holder: match inv.inline_str("holder").ok() {
            None | Some("any") => Holder::Any,
            Some("none") => Holder::None,
            Some(name) => Holder::Named(name.to_string()),
        },
        deferred: match inv.inline_str("deferred").unwrap_or("include") {
            "include" => Deferred::Include,
            "exclude" => Deferred::Exclude,
            "only" => Deferred::Only,
            // Refused, never substituted: a typo (`exlude`) read as `include` answered
            // a different question, with exactly the items the caller asked to leave out.
            other => {
                return Err(Error::InvalidArgument {
                    name: "deferred".to_string(),
                    detail: format!("`{other}` is not one of include, exclude, only"),
                })
            }
        },
        text: inv.inline_str("text").ok().map(str::to_string),
        limit: 0,
        offset: 0,
    })
}

/// `limit=`, or `default` when absent — and **refused** when present but not a count,
/// rather than quietly read as the default (`limit=ten` answered with 50 rows).
fn parse_limit(inv: &Invocation<'_>, default: usize) -> Result<usize> {
    parse_count(inv, "limit", default)
}

/// A count of items named `name`, or `default` when absent — refused when present but not a
/// count, as [`parse_limit`] is (`offset=ten` must not quietly read as the first page).
fn parse_count(inv: &Invocation<'_>, name: &str, default: usize) -> Result<usize> {
    match inv.inline_str(name) {
        Err(_) => Ok(default),
        Ok(text) => text.trim().parse().map_err(|_| Error::InvalidArgument {
            name: name.to_string(),
            detail: format!("`{text}` is not a count of items"),
        }),
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
        let need = match inv.request.verb {
            Verb::Source | Verb::Exists => Need::Read,
            Verb::Sink => Need::Write,
            // ⚠ A Delete needs the delete grant, and an unsupported verb must not be
            // able to probe a ledger's existence under a read grant — so it is refused
            // here, before any capability is checked at all.
            Verb::Delete => Need::Delete,
            other => return Err(unsupported("ledger-item", other)),
        };
        let client = StoreClient::new(inv, ledger_for(inv, need)?);
        let id = inv.bindings.get("id").ok_or_else(|| {
            Error::Endpoint(
                "no `id` captured: this endpoint is bound to \
                 `urn:iki:ledger:{ledger}:item:{id}` and was invoked without the grammar's \
                 capture"
                    .to_string(),
            )
        })?;
        match inv.request.verb {
            Verb::Source => {
                let want = wanted_face(inv, &READ_FACES)?;
                let item = require_item(&client, id).await?;
                if want == JSON {
                    let doc = json::ItemDocument {
                        schema: json::SCHEMA,
                        ledger: client.ledger().name().to_string(),
                        item: json::items(&client, &[&item])
                            .await?
                            .pop()
                            .expect("one rendered item"),
                    };
                    return Ok(json_face(json::render(&doc)?));
                }
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
                // ⚠ **Only "not found" is `false`.** Until 0.2.1 every failure was — so a
                // caller the store refused was told an existing item did not exist, and a
                // host with no store bound was told the same. A denial is a denial, and an
                // item present but unreadable is present: both are answered as errors.
                let exists = match require_item(&client, id).await {
                    Ok(_) => true,
                    Err(Error::NotFound(_)) => false,
                    Err(other) => return Err(other),
                };
                face(
                    if exists { "true\n" } else { "false\n" }.to_string(),
                    None,
                    PLAIN,
                )
            }
            Verb::Sink => {
                let now = now_ms(inv)?;
                let item = require_item(&client, id).await?;
                let mut updates = Vec::new();
                // ★ **An EMPTY `content` is no content** (ledger #398). The engine's `sink`
                // always sends `content` — the piped value, or the empty string when nothing
                // was piped (`ikigai-engine`'s `write_request`) — so `sink …:item:12
                // priority=3` arrives carrying `content=""`. Read as "replace the title with
                // nothing" it was refused, which made every scalar edit from the command line
                // impossible without resending the title and body verbatim. A blank title is
                // refused anyway, so a blank `content` cannot mean a title edit: it means none.
                let given = inv.inline_str("content").ok();
                if let Some(content) = given.filter(|c| !c.trim().is_empty()) {
                    let (title, body) = split_content(content)?;
                    updates.push(replace_one(
                        &client,
                        &item.iri,
                        v::ext::TITLE,
                        &literal(&title),
                    ));
                    updates.push(replace_one(&client, &item.iri, v::BODY, &literal(&body)));
                }
                if let Ok(priority) = inv.inline_str("priority") {
                    updates.push(replace_one(
                        &client,
                        &item.iri,
                        v::PRIORITY,
                        &integer(parse_priority(priority)?),
                    ));
                }
                if let Ok(revision) = inv.inline_str("revision") {
                    updates.push(replace_one(
                        &client,
                        &item.iri,
                        v::REVISION,
                        &literal(revision),
                    ));
                }
                if let Ok(about) = inv.inline_str("about") {
                    for target in about.split_whitespace() {
                        updates.push(insert_on_live(
                            &client,
                            &[&item.iri],
                            &format!(
                                "<{}> <{}> {} .",
                                item.iri,
                                v::ABOUT,
                                sparql::iri(target, "about")?
                            ),
                        ));
                    }
                }
                if updates.is_empty() {
                    // Nothing to change. A blank `content` alone still gets the title's
                    // sentence, since that is what its sender most likely meant to send.
                    if let Some(blank) = given {
                        split_content(blank)?;
                    }
                    return Err(Error::MissingArgument("content".to_string()));
                }
                updates.push(touch(&client, &item.iri, now));
                guarded_update(&client, &[&item.iri], &batch(&updates)).await?;
                Ok(plain(format!("updated {} {}\n", item.short(), item.iri)))
            }
            Verb::Delete => {
                let now = now_ms(inv)?;
                let item = require_item(&client, id).await?;
                let reason = inv.inline_str("reason").unwrap_or("").to_string();
                let author = inv.inline_str("author").ok().map(str::to_string);
                let Some(removed) =
                    delete_item(&client, &item, &reason, author.as_deref(), now, false).await?
                else {
                    return Err(Error::NotFound(format!(
                        "no ledger item at `{}`: another delete removed it while this one was \
                         in flight, and its tombstone is that delete's",
                        item.iri
                    )));
                };
                Ok(plain(format!(
                    "deleted {} {}\n  {} quad(s) moved to <{}> and recoverable\n  tombstone: \
                     {}\n",
                    item.short(),
                    item.iri,
                    removed.quads,
                    client.ledger().deleted_graph(),
                    removed.tombstone
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
                    "The item's opaque id, its short number, or `key:{key}` — the caller's \
                     own name for it (`append key=`). All resolve, because a human types \
                     `12`, a machine carries the IRI, and a bridge carries its own id.",
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
                    .input(ledger_arg("item:{id}"))
                    .summary("The item, its metadata, its links and its comments.")
                    .input(id_input())
                    .input(as_arg(&READ_FACES))
                    .output(PLAIN)
                    .output(TURTLE)
                    .output(JSON),
            ))
            .action(read_action(
                ikigai_core::ActionSpec::new(Verb::Exists)
                    .input(ledger_arg("item:{id}"))
                    .summary("Whether the item exists.")
                    .input(id_input())
                    .output(PLAIN),
            ))
            .action(write_scopes(
                ikigai_core::ActionSpec::new(Verb::Sink)
                    .input(ledger_arg("item:{id}"))
                    .summary(
                        "Edit the item: `content` replaces the title and body (first line, \
                         then the rest), `priority` replaces the priority, `about` ADDS a \
                         target, `revision` replaces the filed-against revision. A blank \
                         `content` — what `sink` sends when nothing is piped — changes \
                         neither title nor body, so each of the others can be set alone.",
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
                                "Whitespace-separated resource IRIs to ADD. \
                                 `urn:iki:ledger:{ledger}:about` adds and removes them on \
                                 their own.",
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
                    .input(ledger_arg("item:{id}"))
                    .summary(
                        "Delete the item: its quads and its comments MOVE to \
                         `urn:iki:ledger:graph:{ledger}:deleted`, and a tombstone stays behind \
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

/// Replace a single-valued property of a live item ([`live`]).
fn replace_one(client: &StoreClient<'_, '_>, item: &str, predicate: &str, value: &str) -> String {
    replace_one_of(client, item, v::ITEM_CLASS, predicate, value)
}

/// Replace a single-valued property of `subject`, only while it is typed `class`: an item for
/// every write on one, a tombstone for a purge's stamps on it.
fn replace_one_of(
    client: &StoreClient<'_, '_>,
    subject: &str,
    class: &str,
    predicate: &str,
    value: &str,
) -> String {
    format!(
        "DELETE {{ {} }} INSERT {{ {} }} WHERE {{ {} }}",
        client.in_graph(&format!("<{subject}> <{predicate}> ?was")),
        client.in_graph(&format!("<{subject}> <{predicate}> {value}")),
        client.in_graph(&format!(
            "<{subject}> <{}> <{class}> . OPTIONAL {{ <{subject}> <{predicate}> ?was }}",
            v::ext::TYPE
        ))
    )
}

/// `raised=` as an `xsd:date` lexical form: `YYYY-MM-DD`, a real calendar date, no time and
/// no zone (ledger #240). Refused rather than normalized, so what is stored is what was sent.
///
/// ```text
/// 2026-07-21   accepted
/// 2024-02-29   accepted (a leap year)
/// 2026-02-30   refused, and so are 2026-7-21, 2026-13-01 and 2026-07-21T00:00:00Z
/// ```
fn parse_date(value: &str) -> Result<String> {
    let refused = |why: &str| Error::InvalidArgument {
        name: "raised".to_string(),
        detail: format!(
            "`{value}` is not a date: {why}. `raised` is when the claim arose, as \
             `YYYY-MM-DD` (an `xsd:date`, no time and no zone)"
        ),
    };
    let value = value.trim();
    let bytes = value.as_bytes();
    let shaped = bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit());
    if !shaped {
        return Err(refused("the shape is YYYY-MM-DD"));
    }
    let number = |range: std::ops::Range<usize>| value[range].parse::<u32>().unwrap_or(0);
    let (year, month, day) = (number(0..4), number(5..7), number(8..10));
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return Err(refused("there is no such month")),
    };
    if day == 0 || day > days {
        return Err(refused("there is no such day in that month"));
    }
    Ok(value.to_string())
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
    /// The tombstone's IRI, in the ledger the item was in.
    tombstone: String,
    /// How many quads left the live graph.
    quads: usize,
    /// How many quads an earlier recoverable delete had archived and a purge destroyed.
    archived: usize,
}

/// Everything that goes with an item when it goes: its own triples, the triples of its
/// comments, and every edge POINTING AT it.
fn removal_selector(subject: &str) -> String {
    format!(
        "?s ?p ?o . FILTER(?s = {subject} || ?o = {subject} || EXISTS {{ ?s <{on}> {subject} }})",
        on = v::ON_ITEM,
    )
}

/// The tombstone's hash: a canonical form of exactly the rows given — sorted triples,
/// each term with its kind, datatype and language tag.
///
/// There are no blank nodes in this graph — every node this module writes is skolemized —
/// so sorted triples ARE a canonical form here, and the heavyweight RDF Dataset
/// Canonicalization would buy nothing.
fn content_hash(rows: &[sparql::Row]) -> String {
    let mut hasher = Sha256::new();
    for row in rows {
        for var in ["s", "p", "o"] {
            if let Some(binding) = row.get(var) {
                hasher.update(binding.kind.as_bytes());
                hasher.update([0x1f]);
                hasher.update(binding.value.as_bytes());
                hasher.update([0x1f]);
                hasher.update(binding.datatype.clone().unwrap_or_default().as_bytes());
                hasher.update([0x1f]);
                // The language tag is in the canonical form because it is in the term:
                // `"titre"@fr` and `"titre"` are different statements, and a digest that
                // could not tell them apart would certify the wrong one.
                hasher.update(binding.lang.clone().unwrap_or_default().as_bytes());
                hasher.update([0x1e]);
            }
        }
        hasher.update([0x1d]);
    }
    format!("sha256:{:x}", hasher.finalize())
}

/// Rows read out of the store, written back as SPARQL data — through the store's own term
/// serializer, never a spelling of ours, and refusing a blank node (see
/// [`sparql::Binding::term`]).
fn rows_as_data(rows: &[sparql::Row]) -> Result<String> {
    let mut triples = String::new();
    for row in rows {
        let (Some(s), Some(p), Some(o)) = (row.get("s"), row.get("p"), row.get("o")) else {
            continue;
        };
        triples.push_str(&format!(
            "{} {} {} .\n",
            sparql::term(&s.term("s")?),
            sparql::term(&p.term("p")?),
            sparql::term(&o.term("o")?)
        ));
    }
    Ok(triples)
}

/// Move (or destroy) an item's quads and write its tombstone.
///
/// ★ The three sets that go together, because leaving any behind is a lie about what was
/// deleted: the item's own triples, the triples of its comments, and every edge POINTING
/// AT it (a `blocks` from another item would otherwise name something that no longer
/// resolves). [`removal_selector`] is that set.
///
/// # ★ Why this is TWO updates, and what a reader sees between them
///
/// A delete moves quads between the ledger's graph and its graveyard, and **a scoped
/// update can neither read nor write across graphs** — that is the whole point of the
/// narrow door (`ikigai_store::confine`). So the single
/// `DELETE { GRAPH live } INSERT { GRAPH deleted } WHERE { GRAPH live }` this used to be
/// cannot exist, and the move becomes:
///
/// 1. a scoped **read** of the live graph — the quads, which this already did for the
///    tombstone's hash;
/// 2. a scoped **write to the graveyard**, inserting them as data;
/// 3. a scoped **write to the live graph**, removing **exactly those quads** and writing
///    the tombstone — which is still one update, so *that* pair is atomic.
///
/// ⚠ **The graveyard is touched first and the live graph last, deliberately: the live
/// graph is the commit point.** Every read in this crate looks at the live graph and none
/// looks at the graveyard, so a crash between the two writes leaves the item *entirely
/// present and undeleted* — with a copy already archived, which no reader can see. A
/// reader therefore never observes a half-deleted item: it sees the item, whole, until the
/// moment it does not. The state is re-runnable rather than merely recoverable, because
/// step 2 is `INSERT DATA` of quads the graph may already hold, and a store is a set.
/// The other order — remove first, archive second — would put the window on the side where
/// a crash destroys data, which is not a trade worth making for one fewer sentence here.
///
/// # ★ Why step 3 is `DELETE DATA` of the rows read, and not the selector again
///
/// Because the selector is re-evaluated at step 3, and the store does not hold still
/// between step 1 and step 3. In 0.2.1 the removal was `DELETE … WHERE { selector }`, so a
/// comment that landed between the read and the removal matched the selector at step 3 and
/// was removed from the live graph **without ever having been archived** — a "recoverable"
/// delete that destroyed an acknowledged write, and a tombstone whose hash and count did not
/// cover it. It lost the comment in 300 of 300 raced rounds. (The single cross-graph update before
/// 0.2.0 was atomic and had no such window; narrowing the doors opened it.)
///
/// Removing exactly the rows read makes three things the same set by construction: what
/// was archived, what was removed, and what the tombstone hashes and counts. A write that
/// races the delete is **kept**, in the live graph, beside the tombstone — an orphan of
/// the deleted item rather than a loss, and the same place a write that arrives just after
/// the delete has always landed. The alternative considered was a guarded removal that
/// re-reads and retries until nothing raced it; it closes the window only to reopen it one
/// round trip later, because a write's own existence check and its insert are also two
/// steps, so it would buy a retry loop and no guarantee.
///
/// The same argument runs backwards for `purge`, which must clear both graphs: it empties
/// the **graveyard** first and the live graph second, so an interrupted purge leaves the
/// item fully live and re-purgeable, rather than leaving an orphan in a graveyard that no
/// resource can name once the live item it belonged to is gone. ⚠ A purge's live removal
/// stays `DELETE … WHERE`, on purpose: a purge is asked to leave nothing of the item, a
/// hand-written blank node included (which `DELETE DATA` cannot name), so a write racing a
/// purge is destroyed with it and is not in the tombstone's count.
///
/// ⚠ The one wrinkle the re-runnability does not cover: if the item is *edited* between an
/// interrupted delete and its retry, the graveyard ends up holding the union of both
/// versions. The tombstone's hash then describes the retry's quads, which is what was
/// removed and is the honest answer; the archive is a superset. There is no in-band signal
/// for it, and inventing one would mean a marker quad written before the move that a
/// reader of the live graph would have to learn to ignore.
///
/// # ★ Two deletes of one item write one tombstone (ledger #764)
///
/// Two deletes that both read the item live used to both write the tombstone, which is one
/// IRI per item, so it carried both deletes' times, reasons and counts. Now the commit point
/// carries the item-still-live condition: a purge's removal runs only while the item is live,
/// and the tombstone is written only while there is none, which is the same fact (a ledger
/// has no undelete). The delete that loses is answered `None` and its caller says so; what it
/// already did to the graveyard is harmless (a recoverable delete re-archives quads that are
/// there already) or exactly what was asked (a purge destroyed the archive, and its caller
/// then stamps the tombstone as a purge of a deleted item does).
async fn delete_item(
    client: &StoreClient<'_, '_>,
    item: &Item,
    reason: &str,
    author: Option<&str>,
    now: u64,
    destroy: bool,
) -> Result<Option<Removed>> {
    let subject = sparql::iri(&item.iri, "item")?;
    let selector = removal_selector(&subject);
    let rows = client
        .select(&format!(
            "SELECT ?s ?p ?o WHERE {{ {} }} ORDER BY ?s ?p ?o",
            client.in_graph(&selector)
        ))
        .await?;
    let digest = content_hash(&rows);

    // Step 1 of 2 — the graveyard, which no read in this crate can see, so nothing a
    // reader observes has changed yet.
    //
    // ⚠ The DELETE TEMPLATE is a quad pattern and may not carry a FILTER — the selector
    // belongs in the WHERE clause only. Putting it in both is a parse error at the store,
    // which is at least loud.
    let mut archived = 0;
    let removal = if destroy {
        // Everything an earlier recoverable delete quarantined. Read first, so the answer
        // can say how much it destroyed — and so the graveyard's READ grant, which the
        // store requires of a `DELETE … WHERE` over it, is a requirement here too rather
        // than a surprise at the next store upgrade.
        archived = client
            .select_deleted(&format!(
                "SELECT ?s ?p ?o WHERE {{ {} }}",
                client.in_deleted_graph(&selector)
            ))
            .await?
            .len();
        client
            .update_deleted(&format!(
                "DELETE {{ {} }} WHERE {{ {} }}",
                client.in_deleted_graph("?s ?p ?o"),
                client.in_deleted_graph(&selector),
            ))
            .await?;
        // Only while the item is live: once another delete committed, the selector's
        // `?o = <item>` would take that delete's tombstone edge with it.
        format!(
            "DELETE {{ {} }} WHERE {{ {} }}",
            client.in_graph("?s ?p ?o"),
            client.in_graph(&format!("{} {selector}", live(&item.iri)))
        )
    } else if rows.is_empty() {
        String::new()
    } else {
        // The quads read above, as data: a scoped update cannot read the live graph from
        // inside the graveyard's scope, so the WHERE clause that used to do this work is
        // now the SELECT that already ran.
        let data = rows_as_data(&rows)?;
        client
            .update_deleted(&format!(
                "INSERT DATA {{ {} }}",
                client.in_deleted_graph(&data)
            ))
            .await?;
        format!("DELETE DATA {{ {} }}", client.in_graph(&data))
    };

    let id = item.iri.rsplit(':').next().unwrap_or("unknown").to_string();
    let tombstone = client.ledger().tombstone(&id);
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
    if destroy {
        triples.push_str(&format!(
            "\n<{tombstone}> <{}> {} .",
            v::PURGED_AT,
            datetime(now)
        ));
    }
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
    // ★ The key goes onto the tombstone, in the same update that removes it from the item,
    // so there is no instant at which the key is free: a keyed append replayed after a
    // delete answers the deleted item instead of filing it again. Kept through a purge as
    // well — it is a name, like the number, not content.
    if let Some(key) = &item.key {
        triples.push_str(&format!("\n<{tombstone}> <{}> {} .", v::KEY, literal(key)));
    }
    // Step 2 of 2 — the live graph, which IS the commit point. Removing the item's quads
    // and writing its tombstone are one scoped update and therefore atomic: there is no
    // instant in which the item is gone and unaccounted for.
    //
    // ★ The order of the two statements is load-bearing for a purge. Its selector matches
    // `?o = <item>`, and the tombstone's `ledger:deletedItem <item>` is exactly that shape —
    // so an INSERT before the DELETE would write the tombstone and then remove it. SPARQL
    // runs `;`-separated operations in order, which is what makes this safe and also what
    // makes it fragile enough to say out loud.
    client
        .update(&batch(&[
            removal,
            format!(
                "INSERT {{ {} }} WHERE {{ FILTER NOT EXISTS {{ {} }} }}",
                client.in_graph(&triples),
                client.in_graph(&format!(
                    "<{tombstone}> <{}> <{}>",
                    v::ext::TYPE,
                    v::TOMBSTONE_CLASS
                )),
            ),
        ]))
        .await?;
    // Whose tombstone it is: this delete's stamp and hash, or another delete's that committed
    // first. Two deletes in one millisecond over the same quads are indistinguishable here,
    // and then either answer is true.
    let ours = client
        .ask(&format!(
            "ASK {{ {} }}",
            client.in_graph(&format!(
                "<{tombstone}> <{}> {} ; <{}> {}",
                v::ext::INVALIDATED_AT,
                datetime(now),
                v::ext::CONTENT_HASH,
                literal(&digest)
            ))
        ))
        .await?;
    if !ours {
        return Ok(None);
    }
    Ok(Some(Removed {
        tombstone,
        quads: rows.len(),
        archived,
    }))
}

/// Destroy what a recoverable delete archived, for an item that is no longer live.
///
/// ★ **This is the path that makes a pasted secret destroyable after it was deleted.**
/// Until 0.2.1 `purge` began with the same live-graph lookup every other action uses, so
/// once an item had been deleted — the first thing anyone does on noticing a token in a
/// title — purge answered "no ledger item", and the content sat in the graveyard for good,
/// reachable by anyone holding the graveyard's read grant and by no resource here at all.
///
/// The item is found through its **tombstone**, which stays in the live graph and names
/// both the number and the IRI. Same order as every other destructive path here: the
/// graveyard first, the tombstone second — an interruption leaves a tombstone still saying
/// `recoverable true` over an empty archive, which a second purge corrects, rather than a
/// tombstone saying "destroyed" over content that is still there.
///
/// The tombstone keeps what the delete wrote (its reason, its author, its hash of what
/// left the live graph) and gains `ledger:purgedAt`, plus `ledger:purgeReason` and
/// `ledger:purgedBy` when given — the purge's own words, kept apart from the delete's.
async fn purge_archived(
    client: &StoreClient<'_, '_>,
    found: &model::Tombstone,
    reason: &str,
    author: Option<&str>,
    now: u64,
) -> Result<usize> {
    let subject = sparql::iri(&found.item, "item")?;
    let selector = removal_selector(&subject);
    let archived = client
        .select_deleted(&format!(
            "SELECT ?s ?p ?o WHERE {{ {} }}",
            client.in_deleted_graph(&selector)
        ))
        .await?
        .len();
    if archived > 0 {
        client
            .update_deleted(&format!(
                "DELETE {{ {} }} WHERE {{ {} }}",
                client.in_deleted_graph("?s ?p ?o"),
                client.in_deleted_graph(&selector),
            ))
            .await?;
    }
    let mut operations = vec![
        replace_one_of(
            client,
            &found.iri,
            v::TOMBSTONE_CLASS,
            v::RECOVERABLE,
            &boolean(false),
        ),
        replace_one_of(
            client,
            &found.iri,
            v::TOMBSTONE_CLASS,
            v::PURGED_AT,
            &datetime(now),
        ),
    ];
    if !reason.is_empty() {
        operations.push(replace_one_of(
            client,
            &found.iri,
            v::TOMBSTONE_CLASS,
            v::PURGE_REASON,
            &literal(reason),
        ));
    }
    if let Some(author) = author {
        operations.push(replace_one_of(
            client,
            &found.iri,
            v::TOMBSTONE_CLASS,
            v::PURGED_BY,
            &literal(author),
        ));
    }
    client.update(&batch(&operations)).await?;
    Ok(archived)
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
        let client = StoreClient::new(inv, ledger_for(inv, Need::Write)?);
        // Every argument is checked before anything is written: a bad `as` or a bad key
        // refused AFTER filing would leave an item the caller was told was not filed.
        let want = wanted_face(inv, &WRITE_FACES)?;
        let key = inv
            .inline_str("key")
            .ok()
            .map(model::parse_key)
            .transpose()?;
        let raised = inv.inline_str("raised").ok().map(parse_date).transpose()?;
        let now = now_ms(inv)?;
        let (title, body) = split_content(inv.inline_str("content")?)?;
        let author = inv.inline_str("author").ok();
        let id = mint_id(now)?;
        let iri = client.ledger().item(&id);

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
            // ⚠ A LEVEL is a subclass of `ledger:Item`, never one of this crate's own
            // bookkeeping classes. An item typed `ledger:Counter` was a second counter
            // to `urn:iki:ledger:ledgers`, which then reported every count doubled — an
            // ordinary validated argument falsifying the inventory.
            if let Some(class) = v::STRUCTURAL_CLASSES.iter().find(|c| **c == kind) {
                return Err(Error::InvalidArgument {
                    name: "kind".to_string(),
                    detail: format!(
                        "`{class}` is one of this ledger's own structural classes, not a \
                         level of item; a `kind` names a subclass of `{}`",
                        v::ITEM_CLASS
                    ),
                });
            }
            triples.push_str(&format!(
                "\n<{iri}> <{}> {} .",
                v::ext::TYPE,
                sparql::iri(kind, "kind")?
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
        if let Some(raised) = &raised {
            triples.push_str(&format!(
                "\n<{iri}> <{}> {} .",
                v::RAISED,
                ikigai_store::sparql::typed_literal(raised, v::ext::XSD_DATE, "raised")?
            ));
        }
        for label in split_labels(inv.inline_str("labels").ok()) {
            triples.push_str(&format!("\n<{iri}> <{}> {} .", v::LABEL, literal(&label)));
        }
        for target in inv.inline_str("about").unwrap_or("").split_whitespace() {
            triples.push_str(&format!(
                "\n<{iri}> <{}> {} .",
                v::ABOUT,
                sparql::iri(target, "about")?
            ));
        }
        // ★ THE KEY'S UNIQUENESS IS A CLAUSE OF THE SAME STATEMENT, not a check before it.
        // The WHERE below yields exactly one solution when the key is free and NONE when
        // any subject in this ledger's graph already carries it — an item, or the tombstone
        // of a deleted one — and with no solution the DELETE/INSERT does nothing: no item,
        // no number spent. The store holds its write lock across one update's evaluation
        // AND its insert, so two keyed appends cannot both see the key free; the gonk
        // bridges' read-then-append could, and filed twice (`tests/keyed_append.rs`).
        let key_guard = match &key {
            Some(key) => {
                triples.push_str(&format!("\n<{iri}> <{}> {} .", v::KEY, literal(key)));
                format!(
                    "\n  FILTER NOT EXISTS {{ {} }}",
                    client.in_graph(&format!("?taken <{}> {}", v::KEY, literal(key)))
                )
            }
            None => String::new(),
        };

        // ★ A TAKEN KEY IS ANSWERED WITHOUT A WRITE (ledger #822). The guarded update below
        // would change nothing, but it is still a successful Sink at
        // `urn:iki:store:graph-update`, and the kernel cuts that thread after every
        // successful mutating request whether or not anything changed — so each replay
        // invalidated every cached read of every ledger, and a re-sync that sends one keyed
        // append per task it knows about was a cache flush. Reading first is sound only
        // because a key, once taken, is taken for good: a delete moves it onto the tombstone
        // in the same update and a purge keeps it, so "taken" here cannot be "free" by the
        // time the caller reads the answer. A key read as free proves nothing, which is why
        // the update keeps its own `FILTER NOT EXISTS` — that clause, not this read, is what
        // stops two concurrent appends from both filing (`tests/keyed_append.rs`).
        //
        // ⚠ Only a race reaches the update with a taken key now, and that no-op still cuts:
        // skipping it would take the kernel answering "unchanged" for a Sink, which is a
        // core question (see the report on ledger #822), not this module's.
        // `tests/noop_keyed_append.rs` keeps both halves.
        if let Some(key) = &key {
            if let Some(found) = model::find_by_key(&client, key).await? {
                return existing_answer(&client, want, &found, key).await;
            }
        }

        // ★ ONE statement, so the number cannot be allocated twice. The counter is read,
        // incremented, rewritten and stamped onto the new item inside a single SPARQL
        // UPDATE — a read-modify-write across two round trips would race two concurrent
        // appends in the same process, and the store's one-writer rule says nothing about
        // that (it excludes other PROCESSES, not other requests).
        //
        // ★ The counter is PER LEDGER — a subject in the ledger's own graph — so numbers
        // restart at 1 in each one, which is why a display number carries the ledger's
        // name (`acme#12`) as soon as there is more than the default. Sharing one counter
        // across ledgers would have leaked the other ledgers' activity through the gaps
        // in the sequence, which is a small thing to leak and an unnecessary one.
        let counter = client.ledger().counter();
        let update = format!(
            "DELETE {{ {delete} }}\nINSERT {{ {insert} }}\nWHERE {{\n  \
             {{ {{ SELECT ?last WHERE {{ {last_q} }} ORDER BY DESC(?last) LIMIT 1 }}\n    \
             UNION\n    {{ BIND({zero} AS ?last) FILTER NOT EXISTS {{ {any_q} }} }} }}\n  \
             OPTIONAL {{ {old_q} }}\n  BIND(?last + 1 AS ?new){key_guard}\n}}",
            delete = client.in_graph(&format!("<{counter}> <{}> ?old", v::LAST_NUMBER)),
            insert = client.in_graph(&format!(
                "<{counter}> <{type_}> <{class}> ; <{last}> ?new .\n{triples}",
                type_ = v::ext::TYPE,
                class = v::COUNTER_CLASS,
                last = v::LAST_NUMBER,
            )),
            last_q = client.in_graph(&format!("<{counter}> <{}> ?last", v::LAST_NUMBER)),
            zero = integer(0),
            any_q = client.in_graph(&format!("<{counter}> <{}> ?any", v::LAST_NUMBER)),
            old_q = client.in_graph(&format!("<{counter}> <{}> ?old", v::LAST_NUMBER)),
        );
        client.update(&update).await?;

        // Which item the key names now says what happened: the one just minted (filed), or
        // one that was there first (existing, and nothing was written).
        if let Some(key) = &key {
            let found = model::find_by_key(&client, key).await?.ok_or_else(|| {
                Error::Endpoint(format!(
                    "a keyed append for `{key}` neither filed an item nor found one: the store \
                     accepted an update that changed nothing, which should be impossible"
                ))
            })?;
            if found.item != iri {
                return existing_answer(&client, want, &found, key).await;
            }
        }

        let item = model::load_item(&client, &iri).await?.ok_or_else(|| {
            Error::Endpoint(format!(
                "the item was written but does not read back at {iri}: the store accepted \
                 an update that changed nothing, which should be impossible"
            ))
        })?;
        append_answer(
            &client,
            want,
            "filed",
            "open",
            item.number,
            &item.iri,
            key.as_deref(),
        )
    }

    fn name(&self) -> &str {
        "ledger-append"
    }

    fn describe(&self) -> Description {
        let spec = write_scopes(
            ikigai_core::ActionSpec::new(Verb::Sink)
                .input(ledger_arg("append"))
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
                    ArgSpec::new("raised")
                        .summary(
                            "When the claim AROSE, `YYYY-MM-DD` — the review or audit it came \
                             from — when that is not when it is filed: a migration or an \
                             import. Stored as `ledger:raised` (an `xsd:date`) beside \
                             `dcterms:created`; \"oldest first\" orders by \
                             `COALESCE(?raised, ?created)`. Absent means raised when filed.",
                        )
                        .class(v::ext::XSD_DATE)
                        .optional(),
                )
                .input(
                    ArgSpec::new("author")
                        .summary("Who is filing it.")
                        .class(v::ext::XSD_STRING)
                        .optional(),
                )
                .input(
                    ArgSpec::new("key")
                        .summary(
                            "Your own name for this item — a hook's finding id, an importer's \
                             issue id — unique in this ledger. If an item already carries it \
                             (or carried it and was deleted), NOTHING is filed and the answer \
                             names that item, marked `existing`: `#12 <iri> existing open` in \
                             text, `\"outcome\": \"existing\"` in JSON. The check and the \
                             filing are one store update, so concurrent appends with one key \
                             file one item. ASCII letters, digits, `-` `.` `_` `~` `:`, at \
                             most 256; find it later at `urn:iki:ledger:item:key:{key}`.",
                        )
                        .class(v::ext::XSD_STRING)
                        .optional(),
                )
                .input(as_arg(&WRITE_FACES))
                .output(PLAIN)
                .output(JSON),
            CAP_WRITE,
        );
        Description::new("ledger-append")
            .title("File a ledger item")
            .summary(
                "Append a new item to the ledger and answer with its number and IRI — or, \
                 with `key`, answer the item already filed under that key and file nothing.",
            )
            .verb(Verb::Meta)
            .action(spec)
    }
}

/// The keyed append's "nothing was filed": the item the key already names, with its
/// status — `deleted` when the key is on a tombstone, else whether the item is open.
async fn existing_answer(
    client: &StoreClient<'_, '_>,
    want: &str,
    found: &model::Keyed,
    key: &str,
) -> Result<Representation> {
    let status = match &found.tombstone {
        Some(_) => "deleted",
        None => match model::load_item(client, &found.item).await? {
            Some(item) if !item.open => "closed",
            _ => "open",
        },
    };
    append_answer(
        client,
        want,
        "existing",
        status,
        found.number,
        &found.item,
        Some(key),
    )
}

/// The append's answer, in the face asked for.
///
/// The plain face is `#12 <iri>` for a filing — byte for byte what 0.3.0 answered, so a
/// caller reading the first two words is unaffected — and `#12 <iri> existing <status>`
/// when a key named an item already there: the marker is a third word that a filing never
/// has, so it cannot be mistaken for one.
fn append_answer(
    client: &StoreClient<'_, '_>,
    want: &str,
    outcome: &str,
    status: &str,
    number: i64,
    iri: &str,
    key: Option<&str>,
) -> Result<Representation> {
    let ledger = client.ledger();
    if want == JSON {
        let mut answer = json::answer(ledger, outcome, json::item_ref(ledger, number, iri));
        answer.status = Some(status.to_string());
        answer.key = key.map(str::to_string);
        return Ok(json_repr(json::render(&answer)?));
    }
    Ok(plain(if outcome == "filed" {
        format!("{} {iri}\n", ledger.number(number))
    } else {
        format!("{} {iri} {outcome} {status}\n", ledger.number(number))
    }))
}

/// The refusal for an act closed work does not admit (a hand-back, a claim; ledger #1418,
/// #1419): a `Conflict`, because the request is fine and the item's state does not permit it,
/// naming the item, its state and how to reopen it.
fn closed_refusal(item: &Item, ledger: &Ledger, why: &str) -> Error {
    Error::Conflict(format!(
        "{} is closed{}: {why}. Reopen it first (`sink {} item={}`) if there is more to do",
        item.short(),
        item.closed_reason
            .as_deref()
            .map(|reason| format!(" ({})", reason.rsplit(['#', '/']).next().unwrap_or(reason)))
            .unwrap_or_default(),
        ledger.resource("reopen"),
        item.number
    ))
}

/// Whether the comment minted as `comment` is in the ledger: how a guarded write learns
/// whether it landed.
async fn comment_written(client: &StoreClient<'_, '_>, comment: &str) -> Result<bool> {
    client
        .ask(&format!(
            "ASK {{ {} }}",
            client.in_graph(&format!(
                "<{comment}> <{}> <{}>",
                v::ext::TYPE,
                v::COMMENT_CLASS
            ))
        ))
        .await
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
        let client = StoreClient::new(inv, ledger_for(inv, Need::Comment)?);
        // Every argument is checked before anything is written, the keyed append's rule.
        let want = wanted_face(inv, &WRITE_FACES)?;
        let handback = parse_flag(inv, "handback")?;
        let rejects = parse_flag(inv, "rejects")?;
        if handback && rejects {
            return Err(Error::InvalidArgument {
                name: "rejects".to_string(),
                detail: "`handback=true` hands the item back to its dispatcher and \
                         `rejects=true` takes a hand-back off it; a comment is one or the \
                         other"
                    .to_string(),
            });
        }
        let key = inv
            .inline_str("key")
            .ok()
            .map(model::parse_key)
            .transpose()?;
        let now = now_ms(inv)?;
        let item = require_item(&client, inv.inline_str("item")?).await?;
        let text = inv.inline_str("content")?;
        if text.trim().is_empty() {
            return Err(Error::InvalidArgument {
                name: "content".to_string(),
                detail: "an empty comment says nothing and cannot be edited afterwards".to_string(),
            });
        }
        let author = inv.inline_str("author").ok();
        // ★ A hand-back asks the dispatcher to check work that is still open; closed work
        // awaits nobody (ledger #1418). Until 0.11.1 this wrote `awaiting` on a closed item,
        // the state the doctor's `awaiting-closed` says only a write around the ledger leaves.
        // The write carries the same condition, against a close that lands in between.
        if handback && !item.open {
            return Err(closed_refusal(
                &item,
                client.ledger(),
                "a hand-back asks the dispatcher to check work that is still open, and closed \
                 work awaits nobody; a comment without `handback=true` is still written",
            ));
        }
        // A taken key is answered without a write, for the reason the keyed append gives
        // (ledger #822): even a no-op update cuts every cached read of the ledger. Sound only
        // because a key, once on a live comment, stays there while its item lives; the
        // write's own guard is what holds against a racer.
        if let Some(key) = &key {
            if let Some(found) = model::find_comment_by_key(&client, key).await? {
                return existing_comment(&client, want, &found, key).await;
            }
        }
        // ★ The dispatcher (the close grant) speaking on a handed-back item is the answer to
        // it, whatever the words: any comment of theirs clears it. A worker clears it only by
        // saying so (`rejects=true`); its ordinary comment leaves the hand-back standing.
        let change = if handback {
            Handback::Set
        } else if rejects || is_dispatcher(inv, client.ledger()) {
            Handback::Clear
        } else {
            Handback::Unchanged
        };
        let comment = write_keyed_comment(
            &client,
            &item.iri,
            text.trim(),
            author,
            now,
            key.as_deref(),
            change,
        )
        .await?;
        // A hand-back the write's guard refused: the item closed between the read above and the
        // write, and neither the comment nor `awaiting` landed. A keyed one whose key a racer
        // took is answered below as that racer's comment, as any taken key is.
        if change == Handback::Set && !comment_written(&client, &comment).await? {
            let keyed = match &key {
                Some(key) => model::find_comment_by_key(&client, key).await?,
                None => None,
            };
            if keyed.is_none() {
                let now = model::load_item(&client, &item.iri).await?.unwrap_or(item);
                return Err(closed_refusal(
                    &now,
                    client.ledger(),
                    "it was closed before the hand-back landed, and nothing was written. Closed \
                     work awaits nobody",
                ));
            }
        }
        if let Some(key) = &key {
            let found = model::find_comment_by_key(&client, key)
                .await?
                .ok_or_else(|| {
                    Error::Endpoint(format!(
                        "a keyed comment for `{key}` neither wrote a comment nor found one: the \
                         store accepted an update that changed nothing, which should be \
                         impossible"
                    ))
                })?;
            if found.iri != comment {
                return existing_comment(&client, want, &found, key).await;
            }
        }
        if want == JSON {
            let ledger = client.ledger();
            let mut answer = json::answer(
                ledger,
                "commented",
                json::item_ref(ledger, item.number, &item.iri),
            );
            answer.comment = Some(written(&comment, &item.iri, text.trim(), author, now));
            answer.key = key;
            answer.handback = match change {
                Handback::Set => Some("set".to_string()),
                Handback::Clear if item.awaiting.is_some() => Some("cleared".to_string()),
                _ => None,
            };
            return Ok(json_repr(json::render(&answer)?));
        }
        // The suffix in the shape `existing` set: a word after the IRI, so a reader matching
        // `commented on #N (<iri>)` still matches.
        let suffix = match change {
            Handback::Set => " awaiting-dispatcher",
            Handback::Clear if item.awaiting.is_some() => " cleared-awaiting",
            _ => "",
        };
        Ok(plain(format!(
            "commented on {} ({comment}){suffix}\n",
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
                 revised is not a record. A worker hands an item back with \
                 `handback=true`, which `next` honors until the dispatcher answers.",
            )
            .verb(Verb::Meta)
            .action(write_scopes(
                ikigai_core::ActionSpec::new(Verb::Sink)
                    .input(ledger_arg("comment"))
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
                    .input(
                        ArgSpec::new("key")
                            .summary(
                                "Your own name for this comment — a message IRI, a hand-back \
                                 id — unique among this ledger's comments. If a comment \
                                 already carries it, NOTHING is written and the answer names \
                                 that comment, marked `existing`: `commented on #12 (<iri>) \
                                 existing` in text, `\"outcome\": \"existing\"` in JSON, \
                                 and the item named is the one the comment is ON. The check \
                                 and the write are one store update, so a replay comments \
                                 once. The append key's shape: ASCII letters, digits, `-` \
                                 `.` `_` `~` `:`, at most 256; an IRI that does not fit is \
                                 keyed by its digest, `{prefix}sha256:{hex}`, with the IRI \
                                 in the text. Comment keys and item keys are separate names.",
                            )
                            .class(v::ext::XSD_STRING)
                            .optional(),
                    )
                    .input(
                        ArgSpec::new("handback")
                            .summary(
                                "`true` to hand the item back to its dispatcher with this \
                                 comment as the evidence: in the same store update, the item \
                                 is marked `ledger:awaiting <urn:iki:ledger:awaiting:dispatcher>` \
                                 and `next` stops offering it (`why: awaiting-dispatcher`). \
                                 Needs only the comment grant, and an open item: refused \
                                 (Conflict) on a closed one. Cleared by close, reopen, a \
                                 comment with `rejects=true`, or any comment from a caller \
                                 holding the close grant. The answer ends ` awaiting-dispatcher` \
                                 (`\"handback\": \"set\"` in JSON).",
                            )
                            .class(v::ext::XSD_BOOLEAN)
                            .one_of(["true", "false"])
                            .default_value("false")
                            .optional(),
                    )
                    .input(
                        ArgSpec::new("rejects")
                            .summary(
                                "`true` when this comment says a hand-back's evidence did not \
                                 hold: the item stops awaiting its dispatcher and `next` offers \
                                 it again. A comment from a caller holding the close grant \
                                 does this without being asked. The answer ends \
                                 ` cleared-awaiting` when it cleared one (`\"handback\": \
                                 \"cleared\"` in JSON). Not with `handback=true`.",
                            )
                            .class(v::ext::XSD_BOOLEAN)
                            .one_of(["true", "false"])
                            .default_value("false")
                            .optional(),
                    )
                    .input(as_arg(&WRITE_FACES))
                    .output(PLAIN)
                    .output(JSON),
                CAP_ANY,
            ))
    }
}

/// A keyed comment's "nothing was written": the comment the key already names, and the
/// item it is on — which is the answer's item even if the request named another.
async fn existing_comment(
    client: &StoreClient<'_, '_>,
    want: &str,
    found: &model::Comment,
    key: &str,
) -> Result<Representation> {
    let item = model::load_item(client, &found.on_item)
        .await?
        .ok_or_else(|| {
            Error::Endpoint(format!(
                "the comment `{}` carrying the key `{key}` is on `{}`, which does not read as \
                 an item",
                found.iri, found.on_item
            ))
        })?;
    if want == JSON {
        let ledger = client.ledger();
        let mut answer = json::answer(
            ledger,
            "existing",
            json::item_ref(ledger, item.number, &item.iri),
        );
        answer.comment = Some(json::comment(found));
        answer.key = Some(key.to_string());
        return Ok(json_repr(json::render(&answer)?));
    }
    Ok(plain(format!(
        "commented on {} ({}) existing\n",
        item.short(),
        found.iri
    )))
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
        let client = StoreClient::new(inv, ledger_for(inv, Need::Close)?);
        let want = wanted_face(inv, &WRITE_FACES)?;
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
        // ★ A CLOSE RELEASES THE CLAIM (ledger #1147, decided 2026-10-10). Closed work is
        // held by nobody, and a claim left on a closed item was the doctor's orphaned
        // machine claim and a holder `items holder=` went on listing. The release is in the
        // same update as the close, so neither can land without the other, and it is
        // recorded as a comment naming who held it, so the record does not lose that.
        let mut ops = vec![
            replace_one(&client, &item.iri, v::STATUS, &format!("<{}>", v::CLOSED)),
            replace_one(&client, &item.iri, v::CLOSED_REASON, &format!("<{reason}>")),
            touch(&client, &item.iri, now),
        ];
        ops.extend(
            CLAIM_FIELDS
                .iter()
                .map(|field| retract(&client, &item.iri, field)),
        );
        // And a hand-back is answered: closed work awaits nobody (ledger #1199).
        ops.push(retract(&client, &item.iri, v::AWAITING));
        ops.push(replace_one(
            &client,
            &item.iri,
            v::AWAITING_AT,
            &datetime(now),
        ));
        guarded_update(&client, &[&item.iri], &batch(&ops)).await?;
        let author = inv.inline_str("author").ok();
        let released = item.claimed_by.clone();
        if released.is_some() {
            let record = format!(
                "Closed while claimed by {}; the claim is released.",
                holder_display(&item)
            );
            write_comment(&client, &item.iri, &record, author, now).await?;
        }
        let mut note_written = None;
        if let Ok(note) = inv.inline_str("content") {
            if !note.trim().is_empty() {
                let comment = write_comment(&client, &item.iri, note.trim(), author, now).await?;
                note_written = Some(written(&comment, &item.iri, note.trim(), author, now));
            }
        }
        if want == JSON {
            let ledger = client.ledger();
            let mut answer = json::answer(
                ledger,
                "closed",
                json::item_ref(ledger, item.number, &item.iri),
            );
            answer.reason = Some(name.to_string());
            answer.comment = note_written;
            answer.released = released;
            return Ok(json_repr(json::render(&answer)?));
        }
        Ok(plain(match released {
            Some(holder) => format!("closed {} ({name}), released {holder}\n", item.short()),
            None => format!("closed {} ({name})\n", item.short()),
        }))
    }

    fn name(&self) -> &str {
        "ledger-close"
    }

    fn describe(&self) -> Description {
        Description::new("ledger-close")
            .title("Close a ledger item")
            .summary(
                "Close with a reason. Closing is NOT deleting: the item stays in the graph, \
                 stays queryable, and stops blocking whatever it blocked. A claim on it is \
                 released, and the release recorded as a comment naming the holder; a \
                 hand-back awaiting the dispatcher is answered.",
            )
            .verb(Verb::Meta)
            .action(write_scopes(
                ikigai_core::ActionSpec::new(Verb::Sink)
                    .input(ledger_arg("close"))
                    .summary(
                        "Set the status to closed, record why, and release any claim. Needs \
                         `urn:cap:ledger:close:{ledger}` or `urn:cap:ledger:write:{ledger}`.",
                    )
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
                    .input(as_arg(&WRITE_FACES))
                    .output(PLAIN)
                    .output(JSON),
                CAP_ANY,
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
        let client = StoreClient::new(inv, ledger_for(inv, Need::Close)?);
        let now = now_ms(inv)?;
        let item = require_item(&client, inv.inline_str("item")?).await?;
        guarded_update(
            &client,
            &[&item.iri],
            &batch(&[
                replace_one(&client, &item.iri, v::STATUS, &format!("<{}>", v::OPEN)),
                retract(&client, &item.iri, v::CLOSED_REASON),
                // A reopened item starts from "not awaiting": whatever hand-back preceded
                // the close was answered by it (ledger #1199).
                retract(&client, &item.iri, v::AWAITING),
                replace_one(&client, &item.iri, v::AWAITING_AT, &datetime(now)),
                touch(&client, &item.iri, now),
            ]),
        )
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
        Ok(plain(format!("reopened {}\n", item.short())))
    }

    fn name(&self) -> &str {
        "ledger-reopen"
    }

    fn describe(&self) -> Description {
        Description::new("ledger-reopen")
            .title("Reopen a ledger item")
            .summary(
                "Undo a close: the status goes back to open, and the reason and any hand-back \
                 are dropped.",
            )
            .verb(Verb::Meta)
            .action(write_scopes(
                ikigai_core::ActionSpec::new(Verb::Sink)
                    .input(ledger_arg("reopen"))
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
                CAP_ANY,
            ))
    }
}

// --------------------------------------------------------------------------- claim

/// `urn:iki:ledger:{ledger}:claim` — take an item, hand it back, or take over an expired
/// lease.
///
/// ★ **Every take is ONE guarded store update** (0.5.0). Until then a claim was
/// read-then-write — "safe for one operator", the README said, "and the first thing to
/// harden for more than one" — so two claimants could both read the item free and both write.
/// Now the update's `WHERE` carries the condition (free, held by this same holder, or — for a
/// takeover — held by `from=` on a lease that has run out), and the holder is read back: ours,
/// or another claimant won and the answer is a typed `Conflict` naming them.
#[derive(Clone)]
struct ClaimEndpoint {
    stamper: crate::claim::ClaimKindStamper,
}

/// What a claim may replace, for [`claim_update`].
enum Guard<'a> {
    /// Free, or already held by this holder (a renewal).
    FreeOrMine(&'a str),
    /// Held by this holder on a lease that ran out at or before `now`.
    ExpiredFrom(&'a str, u64),
    /// Held by this holder now, and nothing else: `renew=true` (ledger #1178). Unlike
    /// [`Guard::FreeOrMine`], a FREE item does not match, so a renewal never re-takes a claim
    /// that was released or taken over in the meantime.
    Mine(&'a str),
}

/// A release as ONE update: every claim field out and `modified` stamped — and only while
/// `guard` holds (the `from` holder, by label or principal, still holds the claim; the caller
/// is still the claimant), so a release racing a takeover drops nothing it did not mean to
/// (ledger #1178, #1199).
fn release_update(client: &StoreClient<'_, '_>, item: &str, guard: &str, now: u64) -> String {
    let mut owned: Vec<&str> = CLAIM_FIELDS.to_vec();
    owned.push(v::ext::MODIFIED);
    let delete: String = owned
        .iter()
        .enumerate()
        .map(|(n, p)| format!("<{item}> <{p}> ?o{n} . "))
        .collect();
    let optional: String = owned
        .iter()
        .enumerate()
        .map(|(n, p)| format!("OPTIONAL {{ <{item}> <{p}> ?o{n} }}\n"))
        .collect();
    format!(
        "DELETE {{ {} }}\nINSERT {{ {} }}\nWHERE {{ {} }}",
        client.in_graph(&delete),
        client.in_graph(&format!(
            "<{item}> <{}> {} .",
            v::ext::MODIFIED,
            datetime(now)
        )),
        client.in_graph(&format!(
            "<{item}> <{}> <{}> .\n{guard}\n{optional}",
            v::ext::TYPE,
            v::ITEM_CLASS
        )),
    )
}

/// Every field a claim writes on its item — what a release retracts, and what a close
/// retracts too (ledger #1147).
const CLAIM_FIELDS: [&str; 7] = [
    v::CLAIMED_BY,
    v::CLAIMED_AT,
    v::PURPOSE,
    v::CLAIM_KIND,
    v::LEASE,
    v::LEASE_EXPIRES,
    v::CLAIM_PRINCIPAL,
];

/// Whether a caller acting as `caller` (its capability's one principal, if any) may treat a
/// claim recorded with `recorded` as its own — renew it, re-take it under the same label.
///
/// ★ A claim recorded WITHOUT a principal is anyone's to adopt under its label, exactly as
/// every claim was before 0.8.0 (a host without principals, root, an older claim); one
/// recorded WITH a principal is only that principal's.
fn same_claimant(recorded: Option<&str>, caller: Option<&str>) -> bool {
    match (recorded, caller) {
        (None, _) => true,
        (Some(recorded), Some(caller)) => recorded == caller,
        (Some(_), None) => false,
    }
}

/// [`same_claimant`] as a clause of a guarded update, so a claim that changed principals
/// between the read and the write is not touched. `caller` is the principal as a SPARQL term.
fn same_claimant_clause(item: &str, caller: Option<&str>) -> String {
    match caller {
        Some(term) => format!(
            "FILTER NOT EXISTS {{ <{item}> <{p}> ?heldAs . FILTER(?heldAs != {term}) }}",
            p = v::CLAIM_PRINCIPAL
        ),
        None => format!(
            "FILTER NOT EXISTS {{ <{item}> <{p}> ?heldAs }}",
            p = v::CLAIM_PRINCIPAL
        ),
    }
}

/// The holder as the plain faces name it: the label, and the principal when there is one.
fn holder_display(item: &Item) -> String {
    match (&item.claimed_by, &item.claim_principal) {
        (Some(holder), Some(principal)) => format!("{holder} (as {principal})"),
        (Some(holder), None) => holder.clone(),
        (None, _) => "nobody".to_string(),
    }
}

/// The claim as ONE update: the old claim's every field out, the new one in, `modified`
/// stamped — all under the guard, so a lost race changes nothing at all.
#[allow(clippy::too_many_arguments)] // reason: one SPARQL statement's inputs, each distinct
fn claim_update(
    client: &StoreClient<'_, '_>,
    item: &str,
    guard: Guard<'_>,
    holder: &str,
    principal: Option<&str>,
    purpose: Option<&str>,
    kind: crate::claim::ClaimKind,
    lease: Option<&crate::claim::Lease>,
    now: u64,
) -> String {
    let takeover = matches!(guard, Guard::ExpiredFrom(..));
    // ★ "Mine" is the label AND the principal (ledger #1199): another principal spelling this
    // holder's label neither renews nor re-takes its claim.
    let claimant = same_claimant_clause(item, principal);
    let guard = match guard {
        Guard::Mine(holder) => format!(
            "<{item}> <{by}> {mine} .\n\
             FILTER NOT EXISTS {{ <{item}> <{by}> ?other . FILTER(?other != {mine}) }}\n\
             {claimant}",
            by = v::CLAIMED_BY,
            mine = literal(holder),
        ),
        Guard::FreeOrMine(holder) => format!(
            "FILTER NOT EXISTS {{ <{item}> <{by}> ?other . FILTER(?other != {mine}) }}\n\
             {claimant}",
            by = v::CLAIMED_BY,
            mine = literal(holder),
        ),
        Guard::ExpiredFrom(from, now) => format!(
            "<{item}> <{by}> {from} ; <{expires}> ?ends .\n\
             FILTER NOT EXISTS {{ <{item}> <{by}> ?other . FILTER(?other != {from}) }}\n\
             FILTER(?ends <= {now})",
            by = v::CLAIMED_BY,
            from = literal(from),
            expires = v::LEASE_EXPIRES,
            now = datetime(now),
        ),
    };
    // Every field a claim owns. `purpose` is replaced only when one is given and a renewal
    // keeps the holder's own; a takeover always clears it, because it was someone else's.
    let mut owned = vec![
        v::CLAIMED_BY,
        v::CLAIMED_AT,
        v::CLAIM_KIND,
        v::LEASE,
        v::LEASE_EXPIRES,
        v::CLAIM_PRINCIPAL,
        v::ext::MODIFIED,
    ];
    if purpose.is_some() || takeover {
        owned.push(v::PURPOSE);
    }
    let delete: String = owned
        .iter()
        .enumerate()
        .map(|(n, p)| format!("<{item}> <{p}> ?o{n} . "))
        .collect();
    let optional: String = owned
        .iter()
        .enumerate()
        .map(|(n, p)| format!("OPTIONAL {{ <{item}> <{p}> ?o{n} }}\n"))
        .collect();
    let mut insert = format!(
        "<{item}> <{by}> {holder} ; <{at}> {when} ; <{kind_p}> <{kind}> ; <{modified}> {when} .",
        by = v::CLAIMED_BY,
        holder = literal(holder),
        at = v::CLAIMED_AT,
        when = datetime(now),
        kind_p = v::CLAIM_KIND,
        kind = kind.iri(),
        modified = v::ext::MODIFIED,
    );
    if let Some(purpose) = purpose {
        insert.push_str(&format!(
            "\n<{item}> <{}> {} .",
            v::PURPOSE,
            literal(purpose)
        ));
    }
    if let Some(principal) = principal {
        insert.push_str(&format!(
            "\n<{item}> <{}> {principal} .",
            v::CLAIM_PRINCIPAL
        ));
    }
    if let Some(lease) = lease {
        insert.push_str(&format!(
            "\n<{item}> <{}> {} ; <{}> {} .",
            v::LEASE,
            ikigai_store::sparql::typed_literal(&lease.duration, v::ext::XSD_DURATION, "lease")
                .expect("a constant datatype IRI"),
            v::LEASE_EXPIRES,
            datetime(now + lease.millis),
        ));
    }
    // ★ Only an OPEN item (ledger #1419): the endpoint refuses a closed one before writing,
    // and this holds against a close that lands between that read and this write.
    format!(
        "DELETE {{ {} }}\nINSERT {{ {} }}\nWHERE {{ {} }}",
        client.in_graph(&delete),
        client.in_graph(&insert),
        client.in_graph(&format!(
            "<{item}> <{}> <{}> ; <{}> <{}> .\n{guard}\n{optional}",
            v::ext::TYPE,
            v::ITEM_CLASS,
            v::STATUS,
            v::OPEN
        )),
    )
}

/// `true`, `false`, or a refusal naming the argument.
fn parse_flag(inv: &Invocation<'_>, name: &str) -> Result<bool> {
    match inv.inline_str(name).ok() {
        None | Some("false") => Ok(false),
        Some("true") => Ok(true),
        Some(other) => Err(Error::InvalidArgument {
            name: name.to_string(),
            detail: format!("`{other}` is not `true` or `false`"),
        }),
    }
}

#[async_trait]
impl Endpoint for ClaimEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let client = StoreClient::new(inv, ledger_for(inv, Need::Claim)?);
        match inv.request.verb {
            Verb::Sink => {
                let holder = inv.inline_str("content")?.trim().to_string();
                // `none` and `any` are the `holder=` filter's keywords, so a holder by either
                // name could claim work that no filter could then find.
                if holder == "none" || holder == "any" {
                    return Err(Error::InvalidArgument {
                        name: "content".to_string(),
                        detail: format!(
                            "`{holder}` is reserved: `holder=none` and `holder=any` are the \
                             items filter's keywords, so work held by `{holder}` could never be \
                             listed by holder"
                        ),
                    });
                }
                if holder.is_empty() {
                    return Err(Error::InvalidArgument {
                        name: "content".to_string(),
                        detail: "a claim needs a holder: an unattributed claim is \
                                 indistinguishable from no claim"
                            .to_string(),
                    });
                }
                // Every argument is checked before anything is read or written.
                let lease = inv
                    .inline_str("lease")
                    .ok()
                    .map(crate::claim::parse_lease)
                    .transpose()?;
                let takeover = parse_flag(inv, "takeover")?;
                let renew = parse_flag(inv, "renew")?;
                if renew && takeover {
                    return Err(Error::InvalidArgument {
                        name: "renew".to_string(),
                        detail: "`renew=true` extends a claim this holder already holds, and \
                                 `takeover=true` takes one from another holder; a request is \
                                 one or the other"
                            .to_string(),
                    });
                }
                let from = inv.inline_str("from").ok().map(str::trim);
                match (takeover, from) {
                    (true, None) | (true, Some("")) => {
                        return Err(Error::InvalidArgument {
                            name: "from".to_string(),
                            detail: "a takeover names the holder it takes from (`from=`): it is \
                                     a compare-and-set against that holder, so a claim that \
                                     changed hands in the meantime is not taken"
                                .to_string(),
                        })
                    }
                    (false, Some(_)) => {
                        return Err(Error::InvalidArgument {
                            name: "from".to_string(),
                            detail: "`from` names the holder of an expired lease to take over, \
                                     and means nothing without `takeover=true`"
                                .to_string(),
                        })
                    }
                    _ => {}
                }
                let purpose = inv.inline_str("purpose").ok();
                // ★ The host's decision, never the caller's: no `kind` input is declared, and
                // one sent anyway is not read. See `crate::claim`.
                let kind = (self.stamper)(inv);
                let now = now_ms(inv)?;
                let item = require_item(&client, inv.inline_str("item")?).await?;
                // ★ Closed work is held by nobody (ledger #1147), so a closed item cannot be
                // claimed, renewed or taken over (ledger #1419). Until 0.11.1 a claim took it,
                // recreating the held-closed state a close removes: listed under `holder=`
                // again, and an orphaned claim to the doctor. The update carries the same
                // condition, against a close that lands between this read and the write.
                if !item.open {
                    return Err(closed_refusal(
                        &item,
                        client.ledger(),
                        "closed work is held by nobody",
                    ));
                }
                let resource = client.ledger().resource("claim");
                let expired = item.lease_expires.is_some_and(|ends| ends <= now);
                // ★ The claimant's principal, from the capability its door minted — never an
                // argument (ledger #1199). `None` on a host without principals and for root.
                let caller = inv.capability.principal();
                let caller_term = caller.map(|p| sparql::iri(p, "principal")).transpose()?;
                let recorded = item.claim_principal.as_deref();
                let another_principal = || {
                    Error::Conflict(format!(
                        "{} is held by {} — the same holder name under another principal \
                         than this capability's ({}). A claim taken as a principal is that \
                         principal's: spelling its holder's name does not make it yours",
                        item.short(),
                        holder_display(&item),
                        caller.unwrap_or("none")
                    ))
                };

                // The cheap half first, so a refusal writes nothing (a no-op update still cuts
                // every cached read). It proves nothing about the update, whose own guard is
                // what holds against a racer.
                let guard = if takeover {
                    let from = from.expect("checked above");
                    match &item.claimed_by {
                        None => {
                            return Err(Error::Conflict(format!(
                                "{} is not claimed, so there is nothing to take over; claim it \
                                 (`sink {resource} item={}`)",
                                item.short(),
                                item.number
                            )))
                        }
                        Some(held) if held != from => {
                            return Err(Error::Conflict(format!(
                                "{} is held by {held}, not {from}: a takeover is a \
                                 compare-and-set against the holder it names, and the claim \
                                 has changed hands",
                                item.short()
                            )))
                        }
                        Some(_) if !expired => {
                            return Err(Error::Conflict(match item.lease_expires {
                                Some(ends) => format!(
                                    "{}'s lease for {from} runs until {}; only an expired lease \
                                     can be taken over",
                                    item.short(),
                                    sparql::iso8601(ends)
                                ),
                                None => format!(
                                    "{} is held by {from} with no lease, which never expires; \
                                     it is released (`delete {resource} item={}`), not taken \
                                     over",
                                    item.short(),
                                    item.number
                                ),
                            }))
                        }
                        Some(_) => Guard::ExpiredFrom(from, now),
                    }
                } else if renew {
                    // ★ A renewal names a claim this holder HOLDS. A worker whose claim was
                    // released, or taken over and released, must learn that it lost the
                    // item, not silently take it back on its next heartbeat (ledger #1178).
                    // An expired lease still held is still held, and renews.
                    match &item.claimed_by {
                        Some(held) if held == &holder && same_claimant(recorded, caller) => {
                            Guard::Mine(&holder)
                        }
                        Some(held) if held == &holder => return Err(another_principal()),
                        Some(held) => {
                            return Err(Error::Conflict(format!(
                                "{} is held by {held}, not {holder}: a renewal extends a claim \
                                 its holder still holds, and this one changed hands",
                                item.short()
                            )))
                        }
                        None => {
                            return Err(Error::Conflict(format!(
                                "{} is not claimed, so {holder} has nothing to renew: the claim \
                                 was released or never taken. Claim it again (without \
                                 `renew`) if the work is still yours to do",
                                item.short()
                            )))
                        }
                    }
                } else {
                    match &item.claimed_by {
                        Some(held) if held != &holder => {
                            // ★ `Conflict`, not `Unavailable`: the request is fine and the
                            // state does not permit it, which no retry changes.
                            return Err(Error::Conflict(if expired {
                                format!(
                                    "{}'s lease for {held} expired at {}, and an expired lease \
                                     is never silently free: take it with `sink {resource} \
                                     item={} takeover=true from={held}`",
                                    item.short(),
                                    sparql::iso8601(item.lease_expires.unwrap_or(now)),
                                    item.number
                                )
                            } else {
                                format!(
                                    "{} is already claimed by {held}. Release it first \
                                     (`delete {resource} item={}`), or take it up with them — \
                                     a claim is a fence, and stealing one silently is how two \
                                     workers end up in the same tree",
                                    item.short(),
                                    item.number
                                )
                            }));
                        }
                        Some(_) if !same_claimant(recorded, caller) => {
                            return Err(another_principal())
                        }
                        _ => Guard::FreeOrMine(&holder),
                    }
                };
                let previous = item.claimed_by.clone();
                client
                    .update(&claim_update(
                        &client,
                        &item.iri,
                        guard,
                        &holder,
                        caller_term.as_deref(),
                        purpose,
                        kind,
                        lease.as_ref(),
                        now,
                    ))
                    .await?;
                let after = model::load_item(&client, &item.iri)
                    .await?
                    .ok_or_else(|| Error::NotFound(format!("no ledger item at `{}`", item.iri)))?;
                if !after.open {
                    // The update's guard refused it: a close landed after the read above.
                    return Err(closed_refusal(
                        &after,
                        client.ledger(),
                        "it was closed before the claim landed, and closed work is held by nobody",
                    ));
                }
                if after.claimed_by.as_deref() == Some(holder.as_str())
                    && after.claim_principal.as_deref() != caller
                {
                    // The label is ours and the principal is not: another principal took
                    // the claim under this name between the read and the write.
                    return Err(Error::Conflict(format!(
                        "{} is held by {}: the claim changed principals before this one landed",
                        item.short(),
                        holder_display(&after)
                    )));
                }
                if after.claimed_by.as_deref() != Some(holder.as_str()) {
                    if renew {
                        return Err(Error::Conflict(format!(
                            "{} is no longer held by {holder}: the claim changed hands before \
                             the renewal landed",
                            item.short()
                        )));
                    }
                    return Err(Error::Conflict(format!(
                        "{} was claimed by {} first",
                        item.short(),
                        after
                            .claimed_by
                            .as_deref()
                            .unwrap_or("someone who has since released it")
                    )));
                }
                let until = lease
                    .as_ref()
                    .map(|l| format!(" until {}", sparql::iso8601(now + l.millis)))
                    .unwrap_or_default();
                Ok(plain(if takeover {
                    format!(
                        "{} taken over from {} by {holder}{until}\n",
                        item.short(),
                        previous.unwrap_or_default()
                    )
                } else if renew {
                    format!("{} renewed by {holder}{until}\n", item.short())
                } else {
                    format!("{} claimed by {holder}{until}\n", item.short())
                }))
            }
            Verb::Delete => {
                let from = match inv.inline_str("from").ok().map(str::trim) {
                    Some("") => {
                        return Err(Error::InvalidArgument {
                            name: "from".to_string(),
                            detail: "an empty `from` names no holder; omit it to release \
                                     whoever holds the claim"
                                .to_string(),
                        })
                    }
                    other => other,
                };
                let now = now_ms(inv)?;
                let item = require_item(&client, inv.inline_str("item")?).await?;
                let resource = client.ledger().resource("claim");
                let ledger = client.ledger();
                let caller = inv.capability.principal();
                let dispatcher = is_dispatcher(inv, ledger);
                // ★ HOLDER = PRINCIPAL (ledger #1199). A claim taken as a principal is released
                // by a capability acting as it, or by the dispatcher's override — never by
                // another worker spelling the holder's name, with `from=` or without it.
                if let Some(recorded) = item.claim_principal.as_deref() {
                    if !dispatcher && !inv.capability.acts_as(recorded) {
                        return Err(Error::Denied(format!(
                            "{} is held by {}, and this capability acts as {}: a claim taken \
                             as a principal is released by that principal, or by a dispatcher \
                             holding `{}` or `{}` (the override). Naming the holder does not \
                             make the claim yours",
                            item.short(),
                            holder_display(&item),
                            caller.map_or("no principal".to_string(), |p| format!("`{p}`")),
                            ledger.cap_close(),
                            ledger.cap_write()
                        )));
                    }
                }
                // What the update must still find, so a release racing a takeover drops
                // nothing it did not mean to.
                let mut guard = String::new();
                if let Some(from) = from {
                    // The cheap half first, so a refusal writes nothing; the update's own
                    // guard is what holds against a racer. `from` names the holder by its
                    // label or by the principal it was taken as.
                    match &item.claimed_by {
                        Some(held) if held == from => {
                            guard.push_str(&format!(
                                "<{}> <{}> {} .\n",
                                item.iri,
                                v::CLAIMED_BY,
                                literal(from)
                            ));
                        }
                        Some(_) if item.claim_principal.as_deref() == Some(from) => {
                            guard.push_str(&format!(
                                "<{}> <{}> {} .\n",
                                item.iri,
                                v::CLAIM_PRINCIPAL,
                                sparql::iri(from, "from")?
                            ));
                        }
                        Some(held) => {
                            return Err(Error::Conflict(format!(
                                "{} is held by {held}, not {from}: a release with `from` is a \
                                 compare-and-set against that holder, and the claim has \
                                 changed hands. Nothing was released",
                                item.short()
                            )))
                        }
                        None => {
                            return Err(Error::Conflict(format!(
                                "{} is not claimed, so {from} holds nothing to release",
                                item.short()
                            )))
                        }
                    }
                }
                // A claimant that is not the dispatcher releases only a claim that is still
                // its own (or no principal's): the check above, as a clause of the write.
                if !dispatcher {
                    if let Some(caller) = caller {
                        guard.push_str(&same_claimant_clause(
                            &item.iri,
                            Some(&sparql::iri(caller, "principal")?),
                        ));
                    }
                }
                client
                    .update(&release_update(&client, &item.iri, &guard, now))
                    .await?;
                if !guard.is_empty() {
                    let after = model::load_item(&client, &item.iri).await?.ok_or_else(|| {
                        Error::NotFound(format!("no ledger item at `{}`", item.iri))
                    })?;
                    if after.claimed_by.is_some() {
                        return Err(Error::Conflict(format!(
                            "{} is held by {}: the claim changed hands before the release{} \
                             landed, and nothing was released. Release it \
                             (`delete {resource} item={}`) only if that is meant",
                            item.short(),
                            holder_display(&after),
                            from.map(|f| format!(" from {f}")).unwrap_or_default(),
                            item.number
                        )));
                    }
                }
                if let Ok(note) = inv.inline_str("content") {
                    if !note.trim().is_empty() {
                        write_comment(&client, &item.iri, note.trim(), None, now).await?;
                    }
                }
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
                 worker shares a ledger. A claim may carry a lease; an expired lease is never \
                 silently free, and `takeover=true from=<holder>` is the only way to take it. \
                 The holder's kind (machine, agent or person) is stamped by the host, never \
                 taken from the caller.",
            )
            .verb(Verb::Meta)
            .action(write_scopes(
                ikigai_core::ActionSpec::new(Verb::Sink)
                    .input(ledger_arg("claim"))
                    .summary(
                        "Claim the item for a holder, in one guarded store update; refuses \
                         (Conflict) if someone else holds it or the item is closed (closed \
                         work is held by nobody). The same holder claiming again \
                         renews the claim and its lease. When the capability carries a \
                         principal, the claim records it (`ledger:claimPrincipal`) beside the \
                         holder's name, and \"the same holder\" means the same name AND the \
                         same principal.",
                    )
                    .input(item_arg("The item to claim."))
                    .input(
                        ArgSpec::new("content")
                            .summary(
                                "The holder — where a pipe's value lands. Not `none` or `any`, \
                                 which are the `holder=` filter's keywords.",
                            )
                            .class(v::ext::XSD_STRING),
                    )
                    .input(
                        ArgSpec::new("purpose")
                            .summary("Why the holder took it — a brief name, a session id.")
                            .class(v::ext::XSD_STRING)
                            .optional(),
                    )
                    .input(
                        ArgSpec::new("lease")
                            .summary(
                                "How long the claim holds: `90s`, `30m`, `2h`, `1d`, or an \
                                 xsd:duration (`PT30M`). The expiry is taken from the kernel's \
                                 clock. Omit it for a claim held until released.",
                            )
                            .class(v::ext::XSD_STRING)
                            .optional(),
                    )
                    .input(
                        ArgSpec::new("takeover")
                            .summary(
                                "`true` to take an item whose lease has EXPIRED from the holder \
                                 named in `from` — a compare-and-set against that holder. A \
                                 live lease, or a claim with no lease, is never taken over.",
                            )
                            .class(v::ext::XSD_BOOLEAN)
                            .one_of(["true", "false"])
                            .default_value("false")
                            .optional(),
                    )
                    .input(
                        ArgSpec::new("from")
                            .summary("The holder a takeover takes from; required with it.")
                            .class(v::ext::XSD_STRING)
                            .optional(),
                    )
                    .input(
                        ArgSpec::new("renew")
                            .summary(
                                "`true` to extend a claim this holder ALREADY holds, and \
                                 nothing else: refused (Conflict) when the item is free or held \
                                 by someone else, so a worker that lost its claim learns it \
                                 rather than silently re-taking the item. Without it, the same \
                                 holder claiming a free item takes it.",
                            )
                            .class(v::ext::XSD_BOOLEAN)
                            .one_of(["true", "false"])
                            .default_value("false")
                            .optional(),
                    )
                    .output(PLAIN),
                CAP_ANY,
            ))
            .action(write_scopes(
                ikigai_core::ActionSpec::new(Verb::Delete)
                    .input(ledger_arg("claim"))
                    .summary(
                        "Release the claim, lease and all. Needs \
                         `urn:cap:ledger:claim:{ledger}` (or `write:{ledger}`). A claim taken \
                         AS A PRINCIPAL (the capability carried one, minted by the host's \
                         door) is released only by a capability acting as that principal, \
                         or by a dispatcher holding `close:{ledger}` or `write:{ledger}`: \
                         Denied otherwise, so one worker cannot release another's claim by \
                         naming it. A claim taken without a principal is free text, released \
                         by any claim-grant holder.",
                    )
                    .input(item_arg("The item to release."))
                    .input(
                        ArgSpec::new("from")
                            .summary(
                                "Release only if THIS holder still holds the claim — its \
                                 name, or the principal it was taken as; a compare-and-set, \
                                 refused (Conflict) and nothing released when the claim changed \
                                 hands. Without it the release drops the claim, which for a \
                                 claim taken as another principal is the dispatcher's override.",
                            )
                            .class(v::ext::XSD_STRING)
                            .optional(),
                    )
                    .input(
                        ArgSpec::new("content")
                            .summary("An optional release note, recorded as a comment.")
                            .class(v::ext::XSD_STRING)
                            .optional(),
                    )
                    .output(PLAIN),
                CAP_ANY,
            ))
    }
}

// --------------------------------------------------------------------------- defer

#[derive(Clone)]
struct DeferEndpoint;

#[async_trait]
impl Endpoint for DeferEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let client = StoreClient::new(inv, ledger_for(inv, Need::Write)?);
        let now = now_ms(inv)?;
        let item = require_item(&client, inv.inline_str("item")?).await?;
        let (update, said) = match inv.request.verb {
            Verb::Sink => (
                replace_one(&client, &item.iri, v::DEFERRED, &boolean(true)),
                "deferred",
            ),
            Verb::Delete => (retract(&client, &item.iri, v::DEFERRED), "resumed"),
            other => return Err(unsupported("ledger-defer", other)),
        };
        guarded_update(
            &client,
            &[&item.iri],
            &batch(&[update, touch(&client, &item.iri, now)]),
        )
        .await?;
        if let Ok(note) = inv.inline_str("content") {
            if !note.trim().is_empty() {
                write_comment(&client, &item.iri, note.trim(), None, now).await?;
            }
        }
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
                 ready set. A modeled property rather than kata's `someday` flag inside a \
                 metadata blob that one query understands — and a boolean rather than a \
                 date, because readiness that turns on the wall clock goes stale in the \
                 cache with no thread to cut it.",
            )
            .verb(Verb::Meta)
            .action(write_scopes(
                ikigai_core::ActionSpec::new(Verb::Sink)
                    .input(ledger_arg("defer"))
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
                    .input(ledger_arg("defer"))
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
        let client = StoreClient::new(inv, ledger_for(inv, Need::Write)?);
        let want = wanted_face(inv, &WRITE_FACES)?;
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
        // Refused on Sink only. A self-block can still arrive out of band, and `next` then
        // refuses naming this resource's Delete as the remedy — which must not be refused
        // for the same reason (0.2.1 refused it, so the ledger's own fix could not run).
        if from.iri == to.iri && name == "blocks" && inv.request.verb == Verb::Sink {
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
            // Both ends live: an edge to a deleted item names something nothing resolves.
            Verb::Sink => (
                insert_on_live(&client, &[&from.iri, &to.iri], &triple),
                "linked",
            ),
            Verb::Delete => (
                format!("DELETE DATA {{ {} }}", client.in_graph(&triple)),
                "unlinked",
            ),
            other => return Err(unsupported("ledger-link", other)),
        };
        // An unlink checks only its source: removing an edge to a deleted item is a cleanup.
        let ends: &[&str] = if inv.request.verb == Verb::Sink {
            &[&from.iri, &to.iri]
        } else {
            &[&from.iri]
        };
        guarded_update(
            &client,
            ends,
            &batch(&[update, touch(&client, &from.iri, now)]),
        )
        .await?;
        if want == JSON {
            let ledger = client.ledger();
            let mut answer =
                json::answer(ledger, said, json::item_ref(ledger, from.number, &from.iri));
            answer.link_type = Some(name.to_string());
            answer.target = Some(json::item_ref(ledger, to.number, &to.iri));
            return Ok(json_repr(json::render(&answer)?));
        }
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
                    .input(ledger_arg("link"))
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
                    .input(as_arg(&WRITE_FACES))
                    .output(PLAIN)
                    .output(JSON),
                CAP_WRITE,
            ))
            .action(write_scopes(
                ikigai_core::ActionSpec::new(Verb::Delete)
                    .input(ledger_arg("link"))
                    .summary("Remove the edge — how a block cycle is broken.")
                    .input(item_arg("The subject item."))
                    .input(
                        ArgSpec::new("content")
                            .summary("The object item.")
                            .class(v::ext::XSD_STRING),
                    )
                    .input(type_arg())
                    .input(as_arg(&WRITE_FACES))
                    .output(PLAIN)
                    .output(JSON),
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
        let client = StoreClient::new(inv, ledger_for(inv, Need::Write)?);
        let now = now_ms(inv)?;
        let item = require_item(&client, inv.inline_str("item")?).await?;
        let label = inv.inline_str("content")?.trim();
        if label.is_empty() {
            return Err(Error::InvalidArgument {
                name: "content".to_string(),
                detail: "a label needs text".to_string(),
            });
        }
        // ⚠ No comma in a label added here: `labels=` and `without=` split on commas, so a
        // label carrying one could be set and never filtered on. Removing one is still
        // allowed, so a label written before this check (or out of band) can be taken off.
        if label.contains(',') && inv.request.verb == Verb::Sink {
            return Err(Error::InvalidArgument {
                name: "content".to_string(),
                detail: format!(
                    "`{label}` contains a comma, and the `labels`/`without` filters split on \
                     commas, so it could never be filtered on. Add each part as its own label"
                ),
            });
        }
        let triple = format!("<{}> <{}> {} .", item.iri, v::LABEL, literal(label));
        let (update, said) = match inv.request.verb {
            Verb::Sink => (insert_on_live(&client, &[&item.iri], &triple), "labeled"),
            Verb::Delete => (
                format!("DELETE DATA {{ {} }}", client.in_graph(&triple)),
                "unlabeled",
            ),
            other => return Err(unsupported("ledger-label", other)),
        };
        guarded_update(
            &client,
            &[&item.iri],
            &batch(&[update, touch(&client, &item.iri, now)]),
        )
        .await?;
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
                    .input(ledger_arg("label"))
                    .summary("Add the label.")
                    .input(item_arg("The item to tag."))
                    .input(
                        ArgSpec::new("content")
                            .summary(
                                "The label — where a pipe's value lands. No commas: the \
                                 `labels`/`without` filters split on them.",
                            )
                            .class(v::ext::XSD_STRING),
                    )
                    .output(PLAIN),
                CAP_WRITE,
            ))
            .action(write_scopes(
                ikigai_core::ActionSpec::new(Verb::Delete)
                    .input(ledger_arg("label"))
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

// --------------------------------------------------------------------------- about

/// `…:item:{id}:about`: the item's `ledger:about` targets, added and removed on their own
/// (ledger #398).
///
/// ★ **Why a resource and not only the item Sink's `about=`.** `about` is how an item names
/// the resources it is filed against — a file, a PR, the session that delivered it — and the
/// convention of recording the session IRI at delivery lands on an item being CLOSED, never
/// one being filed. An additive edit should not have to touch the title and body at all, and
/// a removal had no door anywhere (the README listed it as an omission). The value is
/// `content`, so it is where the engine routes a pipe or a trailing word:
/// `sink urn:iki:ledger:item:12:about urn:agents:session:…`.
///
/// ⚠ An item PART, not a `…:{ledger}:about` action beside `label` and `link`: a ledger may be
/// named anything not in [`crate::ledger::RESERVED`], so a new top-level action word would
/// have to be reserved first — a change to that public constant, and a ledger already called
/// `about` would stop parsing. `item` is reserved already, so a part costs nothing.
#[derive(Clone)]
struct AboutEndpoint;

#[async_trait]
impl Endpoint for AboutEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let said = match inv.request.verb {
            Verb::Sink => "about",
            Verb::Delete => "no longer about",
            other => return Err(unsupported("ledger-item-about", other)),
        };
        let client = StoreClient::new(inv, ledger_for(inv, Need::About)?);
        let now = now_ms(inv)?;
        let item = require_item(&client, flight::part_id(inv)?).await?;
        let targets: Vec<&str> = inv.inline_str("content")?.split_whitespace().collect();
        if targets.is_empty() {
            return Err(Error::InvalidArgument {
                name: "content".to_string(),
                detail: "name at least one resource IRI".to_string(),
            });
        }
        let mut updates = Vec::new();
        for target in &targets {
            let triple = format!(
                "<{}> <{}> {} .",
                item.iri,
                v::ABOUT,
                sparql::iri(target, "content")?
            );
            updates.push(match inv.request.verb {
                Verb::Sink => insert_on_live(&client, &[&item.iri], &triple),
                _ => format!("DELETE DATA {{ {} }}", client.in_graph(&triple)),
            });
        }
        updates.push(touch(&client, &item.iri, now));
        guarded_update(&client, &[&item.iri], &batch(&updates)).await?;
        Ok(plain(format!(
            "{} {said} {}\n",
            item.short(),
            targets.join(" ")
        )))
    }

    fn name(&self) -> &str {
        "ledger-item-about"
    }

    fn describe(&self) -> Description {
        let content = |summary: &str| {
            ArgSpec::new("content")
                .summary(format!(
                    "{summary} Whitespace-separated resource IRIs — where a pipe's value or a \
                     trailing word lands."
                ))
                .class(v::ext::XSD_STRING)
        };
        Description::new("ledger-item-about")
            .title("What an item is about")
            .summary(
                "The resources an item is filed against (`ledger:about`) — a file, a PR, the \
                 session that delivered it — added or removed without touching the item's \
                 title or body. `urn:iki:ledger:items about=` finds items by them. Takes \
                 `urn:cap:ledger:about:{ledger}` or `urn:cap:ledger:write:{ledger}`; the \
                 worker grant (claim and comment) does not reach it.",
            )
            .verb(Verb::Meta)
            .action(write_scopes(
                ikigai_core::ActionSpec::new(Verb::Sink)
                    .input(ledger_arg("item:{id}:about"))
                    .summary("Add the targets. Adding one already there is not an error.")
                    .input(flight::item_id_input())
                    .input(content("The targets to add."))
                    .output(PLAIN),
                CAP_ANY,
            ))
            .action(write_scopes(
                ikigai_core::ActionSpec::new(Verb::Delete)
                    .input(ledger_arg("item:{id}:about"))
                    .summary("Remove the targets. Removing one not there is not an error.")
                    .input(flight::item_id_input())
                    .input(content("The targets to remove."))
                    .output(PLAIN),
                CAP_ANY,
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
        let client = StoreClient::new(inv, ledger_for(inv, Need::Purge)?);
        let now = now_ms(inv)?;
        // ★ Both graveyard grants, checked before anything is touched. A purge reads the
        // graveyard (to find and count what an earlier delete archived) and destroys in it,
        // so it needs the graveyard's READ grant as well as its write grant — and the store
        // refuses a `DELETE … WHERE` over a graph without the read grant for it, because the
        // WHERE clause is a read. Refusing here names both tokens in one sentence, before
        // any graph has changed, rather than surfacing the store's denial halfway through.
        let deleted = client.ledger().deleted_graph();
        let missing: Vec<String> = [
            ikigai_store::cap_read_graph(&deleted),
            ikigai_store::cap_write_graph(&deleted),
        ]
        .into_iter()
        .filter(|scope| !inv.capability.allows(scope))
        .collect();
        if !missing.is_empty() {
            return Err(Error::Denied(format!(
                "this capability does not hold {}. A purge reads and destroys in the \
                 ledger's graveyard `{deleted}` as well as its live graph, so it needs the \
                 graveyard's read AND write grants beside the live graph's",
                missing
                    .iter()
                    .map(|s| format!("`{s}`"))
                    .collect::<Vec<_>>()
                    .join(" or ")
            )));
        }
        let reference = inv.inline_str("content")?;
        let reason = inv.inline_str("reason").unwrap_or("");
        let author = inv.inline_str("author").ok();
        // Not live: it may have been deleted, and then the content is in the graveyard,
        // findable through the tombstone the delete left in the live graph.
        let client = &client;
        let deleted_earlier = |absent: String| async move {
            let Some(found) = model::find_tombstone(client, reference).await? else {
                return Err(Error::NotFound(absent));
            };
            let archived = purge_archived(client, &found, reason, author, now).await?;
            Ok(plain(format!(
                "purged {} {} (deleted earlier)\n  {archived} archived quad(s) destroyed, \
                 NOT recoverable\n  tombstone: {}\n",
                client.ledger().number(found.number),
                found.item,
                found.iri
            )))
        };
        let item = match require_item(client, reference).await {
            Ok(item) => item,
            Err(Error::NotFound(absent)) => return deleted_earlier(absent).await,
            Err(other) => return Err(other),
        };
        // ★ A delete that committed while this purge was in flight (ledger #764): the
        // tombstone is that delete's, and what it archived is purged as for any item deleted
        // earlier, so the answer is the one a purge sent a moment later would have had.
        let Some(removed) = delete_item(client, &item, reason, author, now, true).await? else {
            return deleted_earlier(format!("no ledger item at `{}`", item.iri)).await;
        };
        Ok(plain(format!(
            "purged {} {}\n  {} quad(s) destroyed, NOT recoverable{}\n  tombstone: {}\n",
            item.short(),
            item.iri,
            removed.quads,
            if removed.archived > 0 {
                format!(" (and {} archived by an earlier delete)", removed.archived)
            } else {
                String::new()
            },
            removed.tombstone
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
                    .input(ledger_arg("purge"))
                    .summary(
                        "Destroy the item's content, leaving only its tombstone — whether \
                         the item is live or was deleted earlier (then it is found by its \
                         tombstone, and what the delete archived is destroyed).",
                    )
                    .input(
                        ArgSpec::new("content")
                            .summary(
                                "The item — where a pipe's value lands. `#12`, an opaque id, \
                                 or the full IRI; a deleted item is named the same way.",
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
        let client = StoreClient::new(inv, ledger_for(inv, Need::Read)?);
        let want = wanted_face(inv, &READ_FACES)?;
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
        let limit = parse_limit(inv, 3)?;
        let set = select::ready_at(&client, &filter, now).await?;
        // ★ A lease running out changes this answer with nothing written — an exclusion's
        // `claimed` becomes `lease-expired` — so the answer holds only until the earliest
        // running lease ends, whatever the policy says. Same trap as leverage's age points.
        let valid_until = match (
            select::valid_until(&set, policy.as_ref(), now),
            select::next_lease_expiry(&set, now),
        ) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        let selection = select::rank(client.ledger(), set, policy.as_ref(), now, limit);
        let repr = match want {
            TURTLE => Representation::new(
                ReprType::new(TURTLE).with_param("charset", "utf-8"),
                selection.turtle()?,
            ),
            JSON => json_repr(json::render(&json::next(&client, &selection).await?)?),
            _ => plain(selection.plain()),
        };
        // ★ Cached under the store's write threads AND until the policy's own horizon — the
        // half a thread cannot carry. Reading the clock records no dependency, so a ranking
        // that turns on `now` (leverage's age points) used to be served unchanged for as long
        // as nothing was written: an idle ledger answered with the day it was first asked.
        // The policy says until when its answer holds; `None` is "it does not read the
        // clock", and an instant already past is "do not cache this at all".
        //
        // ⚠ The Turtle face's `prov:generatedAtTime` is the moment the cached answer was
        // COMPUTED, which stays true of it: within the horizon the ranking is the one a fresh
        // computation would give, and the selection's IRI is a digest of what it says.
        let repr = match valid_until {
            None => repr.cacheable(),
            Some(deadline) if deadline > now => {
                repr.cacheable_until(ikigai_core::Time::from_millis(deadline))
            }
            Some(_) => repr,
        };
        Ok(repr
            .depends_on(ikigai_store::UPDATE_THREAD)
            .depends_on(ikigai_store::LOAD_THREAD)
            .depends_on(ikigai_store::GRAPH_UPDATE_THREAD))
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
                .input(ledger_arg("next"))
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
                .input(as_arg(&READ_FACES))
                .output(PLAIN)
                .output(TURTLE)
                .output(JSON),
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
                let want = wanted_face(inv, &FACES)?;
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
                    .input(as_arg(&FACES))
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

// ------------------------------------------------------------------------- ledgers

/// `urn:iki:ledger:ledgers` — which ledgers exist, filtered to the ones this caller may
/// read.
///
/// ★ **Naming ledgers costs discoverability, and this is what buys it back.** The
/// per-ledger resources are bound by a grammar, and a grammar does not enumerate: a
/// catalog can say `urn:iki:ledger:{ledger}:items` exists, and cannot say that `acme` and
/// `bosatsu` are the ledgers it can be asked about. Without this resource a caller who
/// was not *told* a name could not find one, which would make the partition a secret
/// rather than a boundary.
///
/// ⚠ The listing is filtered by the caller's own read grants, so it says what this
/// capability may read and not what the store holds. That is the honest answer to the
/// question asked, and it is also the only one that does not leak a client list to a
/// caller granted one ledger.
///
/// # ★ The only resource here that cannot be answered by one scoped read
///
/// Every other read in this crate names its graph and asks the store for that graph alone.
/// This one asks *which graphs are there* — and a graph-scoped read is confined to a graph
/// the caller already named, so it can enumerate nothing. Two paths, asking **the same
/// query**, differing only in which door:
///
/// - A **scoped** capability carries the answer already: its `urn:cap:ledger:read:{name}`
///   grants ARE the set of ledgers it may see. So the candidates come from the capability,
///   and each one is confirmed with one narrow read of its own graph. A ledger this caller
///   holds no grant for is never asked about, which is the same answer the old filter gave
///   and reaches it without the store ever seeing a cross-graph query.
/// - A **root** capability enumerates nothing — `Capability::scopes()` is `None` for root,
///   which is what root means — so the store enumerates for it: `urn:iki:store:graphs` names
///   the graphs that exist, and each ledger graph among them is read through the same narrow
///   door the scoped path uses ([`crate::sparql::graph_names`]). Until ledger #272 root asked
///   the whole store once through the broad `urn:iki:store:select`, the one broad door this
///   crate resolved; it resolves none now.
///
/// ⚠ A ledger granted at THIS module but not at the store **refuses the whole listing**,
/// naming the token that is missing. A half-granted config is the most likely operator
/// error now that a ledger needs two tokens per direction, and a ledger quietly left out of
/// the answer is indistinguishable from one with nothing filed in it.
#[derive(Clone)]
struct LedgersEndpoint;

#[async_trait]
impl Endpoint for LedgersEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        if inv.request.verb != Verb::Source {
            return Err(unsupported("ledger-ledgers", inv.request.verb));
        }
        let want = wanted_face(inv, &FACES)?;
        // A ledger exists once something has been filed in it — the counter is what
        // survives closing and deleting everything, so it, and not the item count, is
        // what says a ledger is there at all.
        //
        // ★ ONE query, one door, for every caller. Through the narrow door the dataset is
        // exactly `FROM <G> FROM NAMED <G>`, so `GRAPH ?g` can only bind G and the grouping
        // yields the one row for that ledger. The branch below is about where the candidate
        // ledgers come from, never about what is asked of them.
        //
        // ⚠ The counter is found by a DISTINCT subquery, not joined: a join multiplies
        // every item row by the number of counter subjects, and a second one (a hand edit,
        // or an item typed `ledger:Counter` before append refused that) doubled every count.
        let query = format!(
            "SELECT ?g (COUNT(DISTINCT ?item) AS ?items) \
             (COUNT(DISTINCT ?openItem) AS ?open) WHERE {{\n  \
             {{ SELECT DISTINCT ?g WHERE {{ GRAPH ?g {{ ?counter <{type_}> <{counter_class}> }} }} }}\n  \
             OPTIONAL {{ GRAPH ?g {{ ?item <{type_}> <{item_class}> ; <{status}> ?status }}\n    \
             BIND(IF(?status = <{open}>, ?item, ?unbound) AS ?openItem) }}\n\
             }} GROUP BY ?g ORDER BY ?g",
            open = v::OPEN,
            type_ = v::ext::TYPE,
            counter_class = v::COUNTER_CLASS,
            item_class = v::ITEM_CLASS,
            status = v::STATUS,
        );

        // The candidates: for root, the ledger graphs the store holds; for anyone else, the
        // ledgers its own grants name, each checked for its store half first.
        let candidates: Vec<Ledger> = if inv.capability.is_root() {
            // The graph IRI is the ledger's name spelled one way; a graveyard graph and
            // anything else the host keeps in this store are not ledgers and are skipped
            // rather than guessed at. A graph holding no counter yields no row below.
            crate::sparql::graph_names(inv)
                .await?
                .iter()
                .filter_map(|graph| ledger_of_graph(graph))
                .collect()
        } else {
            let granted = granted_ledgers(inv);
            for ledger in &granted {
                // ⚠ **A half-granted ledger stops the listing rather than thinning it.**
                // Two tokens per ledger per direction is the shape an operator will get
                // half right, and a ledger left out of the answer is indistinguishable
                // from one with nothing filed in it — a wrong answer that looks right.
                // The alternative considered was a warning line beside the results, and it
                // was rejected because `as=text/turtle` has nowhere to put one without
                // inventing a class for "a ledger you cannot see", so the two faces would
                // have disagreed about the same question. This is a host misconfiguration
                // with a mechanical fix, and the message names the exact token.
                let scope = ikigai_store::cap_read_graph(&ledger.graph());
                if !inv.capability.allows(&scope) {
                    return Err(Error::Denied(format!(
                        "this capability holds `{}` but not `{scope}`, so the ledger `{}` can \
                         be addressed here and not read in the store, and this listing would \
                         have to omit it — which is indistinguishable from an empty ledger. \
                         Grant the store scope too, or drop the ledger scope: a ledger needs \
                         both halves, delete needs `{}` as well, and purge needs that and \
                         `{}`",
                        ledger.cap_read(),
                        ledger.name(),
                        ikigai_store::cap_write_graph(&ledger.deleted_graph()),
                        ikigai_store::cap_read_graph(&ledger.deleted_graph()),
                    )));
                }
            }
            granted
        };

        let mut found: Vec<(Ledger, i64, i64)> = Vec::new();
        for ledger in candidates {
            let client = StoreClient::new(inv, ledger.clone());
            // At most one row: the scoped dataset holds one graph.
            if let Some(row) = client.select(&query).await?.first() {
                found.push((ledger, count(row, "items"), count(row, "open")));
            }
        }
        found.sort_by(|a, b| a.0.name().cmp(b.0.name()));

        if want == TURTLE {
            let mut graph = Graph::new();
            for (ledger, items, open) in &found {
                let Ok(subject) = oxrdf::NamedNode::new(ledger.prefix().trim_end_matches(':'))
                else {
                    continue;
                };
                let mut push = |p: &str, o: oxrdf::Term| {
                    if let Ok(predicate) = oxrdf::NamedNode::new(p) {
                        graph.insert(&oxrdf::Triple::new(subject.clone(), predicate, o));
                    }
                };
                push(v::ext::TYPE, model::named(v::LEDGER_CLASS));
                push(v::ext::LABEL, model::plain(ledger.name()));
                push(v::LEDGER_GRAPH, model::named(&ledger.graph()));
                push(
                    v::ITEM_COUNT,
                    model::typed(&items.to_string(), v::ext::XSD_INTEGER),
                );
                push(
                    v::OPEN_COUNT,
                    model::typed(&open.to_string(), v::ext::XSD_INTEGER),
                );
            }
            return face(String::new(), Some(graph), want);
        }

        let mut text = if found.is_empty() {
            "no ledgers this capability may read\n".to_string()
        } else {
            found
                .iter()
                .map(|(ledger, items, open)| {
                    format!(
                        "{:<24}  {open:>5} open  {items:>5} total  {}\n",
                        ledger.name(),
                        ledger.graph()
                    )
                })
                .collect()
        };
        text.push_str(&format!("\n{} ledger(s)\n", found.len()));
        face(text, None, want)
    }

    fn name(&self) -> &str {
        "ledger-ledgers"
    }

    fn describe(&self) -> Description {
        read_scopes(
            Description::new("ledger-ledgers")
                .title("Which ledgers exist")
                .summary(
                    "Every ledger this capability may read, with how many items each holds \
                     and the named graph it lives in. A ledger exists once something has \
                     been filed in it; there is no create and no destroy, because the \
                     capability is what makes one real. The listing is FILTERED by the \
                     caller's own read grants, so it answers what you may read rather than \
                     what the store holds.",
                )
                .verb(Verb::Source)
                .verb(Verb::Meta)
                .input(as_arg(&FACES))
                .output(PLAIN)
                .output(TURTLE),
        )
    }
}

/// An aggregate column, or zero. An aggregate over no solutions is unbound rather than 0.
fn count(row: &crate::sparql::Row, var: &str) -> i64 {
    row.get(var).and_then(|b| b.as_i64()).unwrap_or(0)
}

/// The ledgers a non-root capability names in its own read grants, in name order.
///
/// ★ **The grant list IS the candidate list**, and that is not a shortcut — it is the
/// same answer `urn:iki:ledger:ledgers` has always given (it filtered a store-wide query
/// by exactly this predicate), reached without asking the store a question that crosses
/// graphs. A ledger with no grant was never going to be listed.
///
/// ⚠ A **family** grant (`urn:cap:ledger:read:*`) enumerates nothing and is skipped: a
/// wildcard says what may be reached, not what exists, and guessing names from it is not
/// a thing this can do. A host granting the family and nothing else gets an empty listing
/// — which is why the family is a *declaration* form in this ecosystem and not a grant
/// anyone should hold.
fn granted_ledgers(inv: &Invocation<'_>) -> Vec<Ledger> {
    let Some(scopes) = inv.capability.scopes() else {
        return Vec::new();
    };
    let prefix = format!("{}:", CAP_READ.trim_end_matches('*').trim_end_matches(':'));
    let mut ledgers: Vec<Ledger> = scopes
        .iter()
        .filter_map(|scope| scope.strip_prefix(&prefix))
        .filter_map(|name| Ledger::parse(name).ok())
        .collect();
    ledgers.sort_by(|a, b| a.name().cmp(b.name()));
    ledgers.dedup_by(|a, b| a.name() == b.name());
    ledgers
}

/// The ledger a graph IRI names, if it names one at all.
///
/// ⚠ A graveyard (`…:graph:{name}:deleted`) is deliberately NOT a ledger: it is one
/// ledger's quarantine, and listing it would invent a partition that has no resources,
/// no counter and no capability.
fn ledger_of_graph(graph: &str) -> Option<Ledger> {
    let name = graph.strip_prefix(&format!("{}graph:", crate::ledger::PREFIX))?;
    Ledger::parse(name).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_is_time_ordered_and_unique_per_millisecond() {
        let earlier = mint_id(1_757_700_000_000).unwrap();
        let later = mint_id(1_757_700_000_001).unwrap();
        assert!(earlier < later, "{earlier} should sort before {later}");
        assert_eq!(earlier.len(), 23);
        // ★ The same millisecond, minted twice: distinct by construction, with no content
        // to tell them apart. Until 0.2.1 the second half was a digest of the content, so
        // two identical filings in one millisecond were one item.
        let ids: std::collections::BTreeSet<String> = (0..10_000)
            .map(|_| mint_id(1_757_700_000_000).unwrap())
            .collect();
        assert_eq!(ids.len(), 10_000);
        // Every character is Crockford, so an id is safe in an IRI segment as it stands.
        assert!(earlier.bytes().all(|b| CROCKFORD.contains(&b)), "{earlier}");
    }

    /// ★ The property `model::load_comments` breaks a same-millisecond tie on (ledger #813):
    /// ids minted by this process in one millisecond sort in the order they were minted,
    /// because the suffix is the mint sequence offset by a fixed nonce.
    #[test]
    fn ids_minted_in_one_millisecond_sort_in_mint_order() {
        let minted: Vec<String> = (0..1_000)
            .map(|_| mint_id(1_757_700_000_000).unwrap())
            .collect();
        let mut sorted = minted.clone();
        sorted.sort();
        assert_eq!(sorted, minted);
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
