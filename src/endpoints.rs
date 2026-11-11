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
//! urn:iki:ledger:{ledger}:claim          Source Sink Delete  is an epoch current / take it / hand it back
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
//! urn:iki:ledger:{ledger}:item:{id}:history          Source       every event, oldest first
//! urn:iki:ledger:{ledger}:history                    Sink         an importer's past events
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

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use ikigai_core::{
    space_iri, ArgSpec, Description, Endpoint, EndpointSpace, Error, Exact, Invocation,
    ListSeparator, ReprType, Representation, Result, UriTemplate, Verb,
};
use oxrdf::Graph;
use sha2::{Digest, Sha256};

use crate::attribution::{self, Attribution, Dates};
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

/// The family of the alias grant (ledger #2828): giving one of this ledger's items an alias
/// at `urn:iki:alias:scheme:{scheme}:value:{value}`, and nothing else. A held grant names one
/// ledger: [`Ledger::cap_alias`]. `urn:cap:ledger:write:{ledger}` covers it too. Declared as
/// [`CAP_ANY`]; see [`CAP_CLAIM`].
///
/// ★ **It is half of what an alias needs, never all of it.** The other half is owning the
/// scheme, which is data in the aliases graph rather than a grant: see [`crate::alias`]. An
/// importer holds this grant on the ledgers it imports into, and owns its scheme.
pub const CAP_ALIAS: &str = "urn:cap:ledger:alias:*";

/// "Holds some ledger grant": the floor `claim`, `comment`, `close`, `reopen` and
/// `item:{id}:about` DECLARE,
/// because it is the only family both the narrow grant and `write:{ledger}` satisfy (see
/// [`CAP_CLAIM`]). It over-offers: a caller holding only a read or a delete grant, and a
/// store graph write grant, sees those actions in its manifold and is refused at
/// `invoke` with a `Denied` naming both tokens that would do. Declared stays a superset of
/// enforced, never the reverse.
pub const CAP_ANY: &str = "urn:cap:ledger:*";

/// Linking an item of another ledger to one of the named ledger's items (ledger #2829):
/// [`Ledger::cap_link_in`] names it for one ledger. **Not declared by any action**: the link
/// resource declares [`CAP_WRITE`], the grant every link needs, and enforces this one only
/// when the object is in another ledger, which the kernel's pre-check cannot see. A link
/// within one ledger never needs it, and `write:{ledger}` does not stand in for it.
pub const CAP_LINK_IN: &str = "urn:cap:ledger:link-in:*";

/// Removing an item from view. Separate from [`CAP_WRITE`] because a delete takes work
/// OUT of the ledger, and the everyday grant should not carry it.
pub const CAP_DELETE: &str = "urn:cap:ledger:delete:*";

/// Destroying an item's content irreversibly. Separate again, and the narrowest grant in
/// this module: a tombstone survives a purge, but nothing else does.
pub const CAP_PURGE: &str = "urn:cap:ledger:purge:*";

/// What an action needs from the ledger it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Need {
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
    /// Give an item an alias: `alias:{ledger}`, or `write:{ledger}` (ledger #2828).
    Alias,
    /// Link an item of another ledger to one of this ledger's: `link-in:{ledger}` and nothing
    /// else (ledger #2829).
    LinkIn,
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
            Need::Alias => ledger.cap_alias(),
            Need::LinkIn => ledger.cap_link_in(),
        }
    }

    /// Whether `write:{ledger}` also satisfies it: the three verbs split out of the write
    /// grant in 0.7.0 and `about` in 0.10.0, which it still covers so that no grant issued
    /// before changed meaning.
    fn write_covers(self) -> bool {
        matches!(
            self,
            Need::Claim | Need::Comment | Need::Close | Need::About | Need::Alias
        )
    }

    /// The family this crate declares for it.
    fn declared(self) -> &'static str {
        match self {
            Need::Read => CAP_READ,
            Need::Write => CAP_WRITE,
            Need::Delete => CAP_DELETE,
            Need::Purge => CAP_PURGE,
            Need::Claim | Need::Comment | Need::Close | Need::About | Need::Alias => CAP_ANY,
            Need::LinkIn => CAP_LINK_IN,
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
    grant_for(inv, Ledger::from_bindings(inv.bindings)?, need)
}

/// [`ledger_for`] over a ledger the request did not name in its IRI: the ledger of the item an
/// alias is given to, which `urn:iki:alias:scheme:{scheme}:value:{value}` learns from
/// `content=`.
pub(crate) fn grant_for(inv: &Invocation<'_>, ledger: Ledger, need: Need) -> Result<Ledger> {
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
/// [`PLAIN`], for [`crate::alias`]'s descriptions.
pub(crate) const PLAIN_FACE: &str = PLAIN;
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

mod across;
mod counts;
mod evidence;
mod flight;
mod history;
mod import;

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
    participants: Option<String>,
}

impl Default for SpaceConfig {
    fn default() -> Self {
        SpaceConfig {
            policies: Policies::default().all().to_vec(),
            lifecycles: Lifecycles::default(),
            claim_kind: crate::claim::machine_by_default(),
            participants: None,
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

    /// The host's participants graph (ledger #2827): where a committer is enrolled to act for
    /// another principal (`prov:actedOnBehalfOf`) and where an import or sync principal is
    /// typed `ledger:Importer`. With none, the only author a principal may name is itself, and
    /// nobody may claim an authored date. See [`crate::attribution`].
    pub fn participants(mut self, graph: impl Into<String>) -> Self {
        self.participants = Some(graph.into());
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
    let participants: Participants = config.participants.map(Arc::from);
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
        .bind(LedgerGrammar::action("counts"), counts::CountsEndpoint)
        .bind(
            LedgerGrammar::action("backfill-closed-at"),
            counts::BackfillEndpoint,
        )
        // ⚠ The item PARTS go ahead of `item:{id}`, whose `{id}` takes the rest of the IRI:
        // bound after it, `item:12:state` would be read as an item whose id is `12:state`.
        .bind(
            ItemPartGrammar::part("state"),
            flight::StateEndpoint {
                lifecycles: Arc::clone(&lifecycles),
                participants: participants.clone(),
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
        .bind(
            ItemPartGrammar::part("about"),
            AboutEndpoint {
                participants: participants.clone(),
            },
        )
        .bind(ItemPartGrammar::part("history"), history::HistoryEndpoint)
        .bind(
            LedgerGrammar::with_id("item"),
            ItemEndpoint {
                participants: participants.clone(),
            },
        )
        .bind(
            LedgerGrammar::action("append"),
            AppendEndpoint {
                participants: participants.clone(),
            },
        )
        .bind(
            LedgerGrammar::action("comment"),
            CommentEndpoint {
                participants: participants.clone(),
            },
        )
        .bind(
            LedgerGrammar::action("close"),
            CloseEndpoint {
                participants: participants.clone(),
            },
        )
        .bind(
            LedgerGrammar::action("reopen"),
            ReopenEndpoint {
                participants: participants.clone(),
            },
        )
        .bind(
            LedgerGrammar::action("claim"),
            ClaimEndpoint {
                stamper: config.claim_kind,
            },
        )
        .bind(
            LedgerGrammar::action("defer"),
            DeferEndpoint {
                participants: participants.clone(),
            },
        )
        .bind(
            LedgerGrammar::action("link"),
            LinkEndpoint {
                participants: participants.clone(),
            },
        )
        .bind(
            LedgerGrammar::action("label"),
            LabelEndpoint {
                participants: participants.clone(),
            },
        )
        .bind(
            LedgerGrammar::action("purge"),
            PurgeEndpoint {
                participants: participants.clone(),
            },
        )
        .bind(
            LedgerGrammar::action("doctor"),
            flight::DoctorEndpoint {
                lifecycles: Arc::clone(&lifecycles),
            },
        )
        .bind(
            LedgerGrammar::action("history"),
            import::ImportEndpoint {
                participants: participants.clone(),
            },
        )
        .bind(
            LedgerGrammar::action("next"),
            NextEndpoint {
                policies,
                lifecycles,
            },
        )
}

// ------------------------------------------------------------------------- helpers

/// The host's participants graph, shared by every endpoint that writes an author
/// ([`SpaceConfig::participants`]).
pub(crate) type Participants = Option<Arc<str>>;

/// A `text/plain` representation.
pub(crate) fn plain(text: impl Into<String>) -> Representation {
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

/// [`cached`] for a read confined to ONE ledger's graph: hung from that graph's own thread
/// in place of [`ikigai_store::GRAPH_UPDATE_THREAD`], which every scoped write cuts, so a
/// write to another ledger leaves it cached (ledger #2916). A broad update and a load can
/// write any graph, so their threads stay. ⚠ Only for a read whose every store read went
/// to `graph`: a second graph read here and not named is a write that never invalidates it.
/// `a_write_to_one_named_ledger_keeps_the_other_counts_cached` in `tests/counts.rs` keeps it.
fn cached_in(repr: Representation, graph: &str) -> Representation {
    repr.cacheable()
        .depends_on(ikigai_store::UPDATE_THREAD)
        .depends_on(ikigai_store::LOAD_THREAD)
        .depends_on(ikigai_store::graph_thread(graph))
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
pub(crate) fn now_ms(inv: &Invocation<'_>) -> Result<u64> {
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
/// one is refused rather than looked up.** The refusal is here, once, for every endpoint
/// that takes an item. The one exception is the link resource's object, which since 0.17.0
/// may be an item of another ledger under that ledger's `link-in` grant (ledger #2829): it
/// is resolved by `across::link` before it ever reaches this function.
pub(crate) async fn require_item(client: &StoreClient<'_, '_>, reference: &str) -> Result<Item> {
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
            // An alias's own IRI (ledger #2828) resolves as the alias does.
            None if crate::alias::split(reference).is_some() => {
                model::resolve_id(client, reference).await?
            }
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
    retract_when(client, item, predicate, "")
}

/// [`retract`], only while `guard` (a pattern over the ledger's graph) also holds.
fn retract_when(client: &StoreClient<'_, '_>, item: &str, predicate: &str, guard: &str) -> String {
    let pattern = format!("<{item}> <{predicate}> ?was");
    format!(
        "DELETE {{ {} }} WHERE {{ {} }}",
        client.in_graph(&pattern),
        client.in_graph(&format!("{pattern} . {guard}"))
    )
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
/// "did not land": in the first case the write was archived with the item, because a
/// recoverable delete commits only when nothing landed after its read and otherwise reads
/// again (ledger #2780, see `delete_item`), so both are reported the same way.
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
/// a live item ([`live`]), so a late write never stamps a deleted IRI, and only while `guard`
/// (a pattern over the ledger's graph) also holds. `claimed` is a date an import principal
/// claimed for the write (a comment's `created=`, a close's `closed-at=`, the `created=` of the
/// other Sinks an importer replays, ledger #2939), `None` for the kernel's time: see
/// [`stamp_terms`].
fn touch_dated(
    client: &StoreClient<'_, '_>,
    item: &str,
    now: u64,
    claimed: Option<u64>,
    guard: &str,
) -> String {
    let stamp = stamp_terms(item, now, claimed);
    format!(
        "DELETE {{ {} }} INSERT {{ {} }} WHERE {{ {} }}",
        client.in_graph(&format!(
            "<{item}> <{}> ?was{}",
            v::ext::MODIFIED,
            stamp.delete
        )),
        client.in_graph(&format!(
            "<{item}> <{}> {}{}",
            v::ext::MODIFIED,
            stamp.value,
            stamp.insert
        )),
        client.in_graph(&format!(
            "{} {guard} OPTIONAL {{ <{item}> <{}> ?was }}{}",
            live(item),
            v::ext::MODIFIED,
            stamp.filter
        ))
    )
}

/// What a write stamps as the item's `modified`: see [`stamp_terms`].
struct Stamp {
    /// The new `dcterms:modified`.
    value: String,
    /// What else the update removes: the date the previous claim recorded.
    delete: String,
    /// What else it inserts: the marks that say the date is claimed.
    insert: String,
    /// The bindings and the `FILTER` on `?was`, the value it replaces.
    filter: String,
}

/// What a write stamps as the item's `modified`.
///
/// ★ **A claimed date moves `modified` only FORWARD** (ledger #2928). An importer files an
/// item with the source's `modified` and then replays its past: comments dated at the source,
/// a close dated at the source. Until 0.18.0 each of those stamped the kernel's time, so
/// `modified` ended at the import, and `priority-recency`, whose tie-break it is, ranked the
/// item as just updated. Now a write that carries a claimed date sets `modified` to the later
/// of its current value and that date, marked `ledger:modifiedSource "import"`, and records
/// the date in `ledger:claimedModified`; a past date changes nothing. A write with no claimed
/// date stamps `now`, as every write always has, and leaves the marks: the read takes the claim
/// to hold only while `modified` still equals `claimedModified` (ledger #2932), so the kernel's
/// time over it is not claimed. An unreadable `?was` cannot be compared, and the claimed date
/// replaces it.
fn stamp_terms(item: &str, now: u64, claimed: Option<u64>) -> Stamp {
    match claimed {
        None => Stamp {
            value: datetime(now),
            delete: String::new(),
            insert: String::new(),
            filter: String::new(),
        },
        Some(claimed) => Stamp {
            value: datetime(claimed),
            delete: format!(" . <{item}> <{}> ?wasClaimed", v::CLAIMED_MODIFIED),
            insert: format!(
                " . <{item}> <{}> {} . <{item}> <{}> {}",
                v::MODIFIED_SOURCE,
                literal(attribution::IMPORT),
                v::CLAIMED_MODIFIED,
                datetime(claimed)
            ),
            filter: format!(
                " OPTIONAL {{ <{item}> <{}> ?wasClaimed }} FILTER(COALESCE(?was < {}, true))",
                v::CLAIMED_MODIFIED,
                datetime(claimed)
            ),
        },
    }
}

/// The `created` argument of a write an import principal may date at its source (ledger
/// #2939): `what` is the write, in the words its refusal uses.
fn created_arg(what: &str) -> ArgSpec {
    ArgSpec::new("created")
        .summary(format!(
            "When {what} was made at its source, for an import principal (ledger:Importer in \
             the host's participants graph) only: an xsd:dateTime no later than now and no \
             earlier than the item's filing. Dates its event, marked claimed, and moves the \
             item's modified only forward to it (ledger #2939). Anyone else sending it is \
             refused."
        ))
        .class(v::ext::XSD_DATETIME)
        .optional()
}

/// `created=` on a write an import principal replays (ledger #2934, #2939), held by
/// [`attribution::resolve_claimed`] to the one date rule, and the committer's attribution
/// carrying it: the write's event is dated with it, and so is a note it writes. `None` and
/// the plain committer when it was not sent; anyone but an import principal is refused.
async fn importer_dated(
    inv: &Invocation<'_>,
    client: &StoreClient<'_, '_>,
    participants: Option<&str>,
    what: &str,
    item: &Item,
    now: u64,
) -> Result<(Option<u64>, Attribution)> {
    let claimed = attribution::resolve_claimed(
        inv,
        client,
        participants,
        "created",
        what,
        item.created,
        now,
    )
    .await?;
    Ok((
        claimed,
        Attribution {
            created: claimed,
            ..Attribution::committer(inv)?
        },
    ))
}

/// Append a comment — the shared path behind `urn:iki:ledger:comment` and behind the
/// optional note on close / reopen / defer / claim, so *why* is always recorded in one
/// place and one shape.
async fn write_comment(
    client: &StoreClient<'_, '_>,
    item: &str,
    text: &str,
    who: &Attribution,
    now: u64,
) -> Result<String> {
    write_keyed_comment(client, item, text, who, now, None, Handback::Unchanged, "").await
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
///
/// ★ Clearing names what it answered (ledger #3071): `comment` rejects every hand-back on the
/// item nothing has rejected yet, read while `awaiting` still stands, so a comment that clears
/// nothing names nothing. Every one, not the newest: anyone may hand back on top of a worker's
/// evidence, and a reject naming only that one would leave the worker's for a reader to take.
fn handback_update(
    client: &StoreClient<'_, '_>,
    item: &str,
    comment: &str,
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
        "{} OPTIONAL {{ <{item}> <{}> ?awaiting }} OPTIONAL {{ <{item}> <{}> ?at }}",
        live(item),
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
        Handback::Clear => batch(&[
            format!(
                "INSERT {{ {} }} WHERE {{ {} {guard} }}",
                client.in_graph(&format!("<{comment}> <{}> ?handback", v::REJECTS)),
                client.in_graph(&format!(
                    "<{item}> <{awaiting}> ?awaiting . ?handback <{on}> <{item}> . \
                     {{ ?handback <{mark}> ?mark }} UNION {{ ?handback <{by}> ?mark }} \
                     FILTER NOT EXISTS {{ ?rejected <{rejects}> ?handback }}",
                    awaiting = v::AWAITING,
                    on = v::ON_ITEM,
                    mark = v::HAND_BACK,
                    by = v::HANDED_BACK_BY,
                    rejects = v::REJECTS,
                )),
            ),
            format!(
                "DELETE {{ {was} }} INSERT {{ {} }} WHERE {{ {optional} {guard} }}",
                client.in_graph(&stamp),
            ),
        ]),
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
///
/// `fence` is a fenced comment's [`fence_clause`] (ledger #3205), carried by every operation
/// with the key's and the hand-back's clauses, so a comment whose claim moved on writes nothing.
#[allow(clippy::too_many_arguments)] // reason: one guarded write's inputs, each distinct
async fn write_keyed_comment(
    client: &StoreClient<'_, '_>,
    item: &str,
    text: &str,
    who: &Attribution,
    now: u64,
    key: Option<&str>,
    handback: Handback,
    fence: &str,
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
        when = datetime(who.created_or(now)),
    );
    triples.push_str(&who.triples(&comment, now));
    triples.push_str(&who.date_sources(&comment));
    // ★ The hand-back is in the SAME update as the comment (ledger #1199), so the evidence
    // and the state it puts the item in land together or not at all.
    if key.is_none() && handback != Handback::Set && fence.is_empty() {
        guarded_update(
            client,
            &[item],
            &batch(&[
                insert_on_live(client, &[item], &triples),
                touch_dated(client, item, now, who.created, ""),
                handback_update(client, item, &comment, handback, "", now),
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
    let fence = if fence.is_empty() {
        String::new()
    } else {
        client.in_graph(fence)
    };
    let free = format!("{} {open} {free}{fence}", client.in_graph(&live(item)));
    // ★ A hand-back names the holder label of the claim it finds (ledger #2819), read in the
    // update that writes it: the principal is the comment's committer, and two workers can
    // share one, so the label is what tells them apart. Nobody holding it names nobody, so the
    // mark a reject finds it by (ledger #3071) is its own triple.
    let holder = if handback == Handback::Set {
        triples.push_str(&format!(
            "\n<{comment}> <{}> ?handedBackBy ; <{}> true .",
            v::HANDED_BACK_BY,
            v::HAND_BACK
        ));
        client.in_graph(&format!(
            "OPTIONAL {{ <{item}> <{}> ?handedBackBy }}",
            v::CLAIMED_BY
        ))
    } else {
        String::new()
    };
    // The stamp and the hand-back first: once the comment is in, the key is taken and the
    // guard is false, so a replay changes neither.
    let stamp = stamp_terms(item, now, who.created);
    client
        .update(&batch(&[
            format!(
                "DELETE {{ {} }} INSERT {{ {} }} WHERE {{ {} {free} }}",
                client.in_graph(&format!(
                    "<{item}> <{}> ?was{}",
                    v::ext::MODIFIED,
                    stamp.delete
                )),
                client.in_graph(&format!(
                    "<{item}> <{}> {}{}",
                    v::ext::MODIFIED,
                    stamp.value,
                    stamp.insert
                )),
                client.in_graph(&format!(
                    "OPTIONAL {{ <{item}> <{}> ?was }}{}",
                    v::ext::MODIFIED,
                    stamp.filter
                )),
            ),
            handback_update(client, item, &comment, handback, &free, now),
            format!(
                "INSERT {{ {} }} WHERE {{ {free} {holder} }}",
                client.in_graph(&triples)
            ),
        ]))
        .await?;
    still_live(client, item).await?;
    Ok(comment)
}

/// The comment [`write_comment`] just wrote, as the JSON face carries it — built from what
/// was written rather than read back, because the write is the authority on what it said.
fn written(iri: &str, item: &str, text: &str, who: &Attribution, now: u64) -> json::Comment {
    json::comment(&model::Comment {
        iri: iri.to_string(),
        on_item: item.to_string(),
        body: text.to_string(),
        author: who.author.clone(),
        author_source: who.source.map(|s| s.as_str().to_string()),
        committer: who.committer.clone(),
        committed_at: Some(now),
        created_claimed: who.created.is_some(),
        created: who.created_or(now),
        rejects: Vec::new(),
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
             name, from `append key=`), an alias, `{{scheme}}:{{value}}` (`gh:acme/web#12`; \
             see `urn:iki:alias:scheme:{{scheme}}`), or the item's full IRI, \
             `urn:iki:ledger:{{ledger}}:item:{{id}}` (`urn:iki:ledger:item:{{id}}` in the \
             ledger called `default`) — a human types the number and a machine carries the \
             IRI. An item of another ledger is refused, never looked up here."
        ))
        .class(v::ext::XSD_STRING)
}

pub(crate) fn unsupported(id: &str, verb: Verb) -> Error {
    Error::Endpoint(format!("`{id}` does not answer {verb:?}"))
}

/// Explicit selection is a union, not a change to the singular resource binding.
/// Check both grants before issuing any graph request; omitted ledgers reveal nothing.
fn listing_ledgers(inv: &Invocation<'_>) -> Result<Vec<Ledger>> {
    let Ok(names) = inv.inline_str("ledgers") else {
        return Ok(vec![ledger_for(inv, Need::Read)?]);
    };
    let mut ledgers = names
        .split(',')
        .map(|name| {
            Ledger::parse(name.trim()).map_err(|error| match error {
                Error::InvalidArgument { detail, .. } => Error::InvalidArgument {
                    name: "ledgers".to_string(),
                    detail,
                },
                other => other,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    ledgers.sort();
    ledgers.dedup();
    ledgers.retain(|ledger| {
        inv.capability.allows(&ledger.cap_read())
            && inv
                .capability
                .allows(&ikigai_store::cap_read_graph(&ledger.graph()))
    });
    Ok(ledgers)
}

fn ledgers_arg() -> ArgSpec {
    ArgSpec::new("ledgers")
        .summary("List the union of these comma-separated ledger names instead of the bound ledger. \
            Names follow the ledger naming rules; surrounding spaces and repeated names are ignored. \
            A ledger without both viewer read grants is silently omitted. All filters apply to \
            each selected ledger; parent numbers are resolved within each one. The items listing \
            pages the union in the requested sort (default -modified). The singular ledger \
            binding keeps its meaning. JSON adds ledgers \
            with the readable selected names (including empty ledgers).")
        .class(v::ext::XSD_STRING)
        .optional()
}

mod listing_sort;

// --------------------------------------------------------------------------- items

#[derive(Clone)]
struct ItemsEndpoint;

#[async_trait]
impl Endpoint for ItemsEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        if inv.request.verb != Verb::Source {
            return Err(unsupported("ledger-items", inv.request.verb));
        }
        let ledgers = listing_ledgers(inv)?;
        let union = inv.inline_str("ledgers").is_ok();
        let bound = Ledger::from_bindings(inv.bindings)?;
        let want = wanted_face(inv, &READ_FACES)?;
        let sort = listing_sort::Sort::parse(inv.inline_str("sort").ok())?;
        let mut filter = parsed_listing_filter(inv)?;
        filter.limit = parse_limit(inv, 50)?;
        filter.offset = parse_count(inv, "offset", 0)?;
        let limit = model::page_limit(&filter);
        let mut entries = Vec::new();
        let mut total = 0usize;
        let mut broken = Vec::new();
        for (index, ledger) in ledgers.iter().enumerate() {
            let client = StoreClient::new(inv, ledger.clone());
            let mut local = listing_filter(inv, &client).await?;
            local.limit = if union {
                filter.offset.saturating_add(limit)
            } else {
                filter.limit
            };
            local.offset = if union { 0 } else { filter.offset };
            // ★ The rows and the total come from ONE store query (ledger #1741): counted
            // apart, an append landing between the two reads answered a total no page of
            // this answer could reach.
            let (mut matching, counted) = if sort.is_default() {
                model::load_page(&client, &local).await?
            } else {
                model::load_matching_items(&client, &local).await?
            };
            if sort.has_landing() {
                // Cross-ledger edges are link records in the source graph, not direct
                // blocks triples. Read only the already authorized graph; the sorter
                // ignores targets outside the matching readable selection.
                let subjects: Vec<&str> = matching.iter().map(|item| item.iri.as_str()).collect();
                let mut links = across::outbound_of(&client, &subjects).await?;
                for item in &mut matching {
                    for (predicate, target) in links.remove(&item.iri).unwrap_or_default() {
                        if predicate == v::BLOCKS && !item.blocks.contains(&target) {
                            item.blocks.push(target);
                        }
                    }
                    item.blocks.sort();
                }
            }
            entries.extend(matching.into_iter().map(|item| (index, item)));
            if want != TURTLE {
                total = total.saturating_add(counted);
                // The corpus check scans every item, open and closed; the probe is one
                // ASK and is false on a clean graph, which is the common case (#3255).
                if model::may_have_defects(&client).await? {
                    broken.extend(model::defects(&client).await?);
                }
            }
        }
        if !sort.is_default() {
            sort.apply(&mut entries, &ledgers)?;
        } else if union {
            // Each graph contributes its first offset + limit items. No later item can
            // reach this page. Resolve ties by ledger name, then IRI, independently of
            // the order and duplication of names in the argument.
            entries.sort_by(|(ai, a), (bi, b)| {
                b.modified
                    .cmp(&a.modified)
                    .then(b.number.cmp(&a.number))
                    .then(ledgers[*ai].name().cmp(ledgers[*bi].name()))
                    .then(a.iri.cmp(&b.iri))
            });
        }
        if union || !sort.is_default() {
            entries = entries
                .into_iter()
                .skip(filter.offset)
                .take(limit)
                .collect();
        }
        let (sources, mut items): (Vec<usize>, Vec<Item>) = entries.into_iter().unzip();
        // Decorate and render only this page, through each row's own graph client.
        let mut rendered = Vec::new();
        if want != PLAIN {
            for (index, ledger) in ledgers.iter().enumerate() {
                let client = StoreClient::new(inv, ledger.clone());
                let positions: Vec<usize> = sources
                    .iter()
                    .enumerate()
                    .filter_map(|(position, source)| (*source == index).then_some(position))
                    .collect();
                if positions.is_empty() {
                    continue;
                }
                let mut group: Vec<Item> = positions.iter().map(|p| items[*p].clone()).collect();
                across::decorate_all(inv, &client, &mut group).await?;
                for (position, item) in positions.iter().zip(group) {
                    items[*position] = item;
                }
                if want == JSON {
                    let rows: Vec<&Item> = positions.iter().map(|p| &items[*p]).collect();
                    rendered.extend(
                        positions
                            .into_iter()
                            .zip(json::items(&client, &rows).await?),
                    );
                }
            }
        }
        if want == TURTLE {
            let mut graph = Graph::new();
            for item in &items {
                item.triples(&mut graph);
            }
            return face(String::new(), Some(graph), want);
        }
        if want == JSON {
            rendered.sort_by_key(|(position, _)| *position);
            let rendered: Vec<json::Item> = rendered.into_iter().map(|(_, item)| item).collect();
            let doc = json::ItemsDocument {
                schema: json::SCHEMA,
                ledger: bound.name().to_string(),
                ledgers: union.then(|| {
                    ledgers
                        .iter()
                        .map(|ledger| ledger.name().to_string())
                        .collect()
                }),
                count: rendered.len(),
                total: Some(total),
                offset: filter.offset,
                items: rendered,
                unreadable: broken
                    .iter()
                    .map(|(iri, defects)| json::Unreadable {
                        iri: iri.clone(),
                        defects: defects.clone(),
                    })
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
        if !broken.is_empty() {
            let corpus = if union { "selected graphs" } else { "graph" };
            text.push_str(&format!(
                "\n⚠ {} item(s) in the {corpus} could not be read and are not listed above \
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
                        "Every item matching the filter, ordered by sort (default -modified). The \
                         filters are evaluated in the query, which is why the ordering policy \
                         behind `urn:iki:ledger:next` can stay small.",
                    )
                    .verb(Verb::Source)
                    .verb(Verb::Meta)
                    .input(ledger_arg("items"))
                    .input(ledgers_arg()),
            )
            .input(
                ArgSpec::new("sort")
                    .summary(
                        "Comma-separated priority, created, modified, number or landing; prefix a \
                         key with - for descending order. Default -modified. Unset priority is \
                         always last. Landing orders the matching readable blocks graph before \
                         paging, with priority, ledger and number ties; a cycle is refused. \
                         Other ties use descending number, ledger and IRI.",
                    )
                    .class(v::ext::XSD_STRING)
                    .default_value("-modified")
                    .optional(),
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
                         0); the next page is at `offset` plus `limit`. Changes to sort keys \
                         or blocks between page reads can move rows, so an item can be \
                         missed or repeated: when the whole set matters, read it as one page.",
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
/// they show what it finds (since ledger #3255 behind [`model::may_have_defects`], one `ASK`
/// that skips it over a clean ledger). This is the door for a caller that wants only the
/// number.
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
        let ledgers = listing_ledgers(inv)?;
        let want = wanted_face(inv, &COUNT_FACES)?;
        parsed_listing_filter(inv)?;
        let mut total = 0usize;
        for ledger in &ledgers {
            let client = StoreClient::new(inv, ledger.clone());
            let filter = listing_filter(inv, &client).await?;
            total = total.saturating_add(model::count_items(&client, &filter).await?);
        }
        if want == JSON {
            return Ok(json_face(json::render(&json::CountDocument {
                schema: json::SCHEMA,
                ledger: Ledger::from_bindings(inv.bindings)?.name().to_string(),
                ledgers: inv.inline_str("ledgers").is_ok().then(|| {
                    ledgers
                        .iter()
                        .map(|ledger| ledger.name().to_string())
                        .collect()
                }),
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
                         costs one COUNT query per readable selected ledger. The plain face is the bare number.",
                    )
                    .verb(Verb::Source)
                    .verb(Verb::Meta)
                    .input(ledger_arg("count"))
                    .input(ledgers_arg()),
            )
            .input(as_arg(&COUNT_FACES))
            .output(PLAIN)
            .output(JSON),
        )
    }
}

/// The filter's inputs, in the order `items` has always declared them: `status`, `kind`,
/// `labels`, `without`, `priority`, `without_priority`, `about`, `parent` (0.12.0),
/// `key`, `holder`, `deferred`, `text` — what
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
        ArgSpec::new("priority")
            .summary("Comma-separated 0, 1, 2, 3, 4, or none (no priority set); match ANY. Spaces are trimmed; empty or unknown values are refused. Combines with every other filter.")
            .class(v::ext::XSD_STRING)
            .optional(),
    )
    .input(
        ArgSpec::new("without_priority")
            .summary("Comma-separated 0, 1, 2, 3, 4, or none (no priority set); exclude ANY match. Spaces are trimmed; empty or unknown values are refused. Applies alongside priority, so exclusion wins on overlap.")
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
    let mut filter = parsed_listing_filter(inv)?;
    filter.parent = parent;
    Ok(filter)
}

/// Validate filters even when no selected ledger is readable, without querying it.
fn parsed_listing_filter(inv: &Invocation<'_>) -> Result<Filter> {
    Ok(Filter {
        parent: None,
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
        priority: parse_priority_filter(inv, "priority")?,
        without_priority: parse_priority_filter(inv, "without_priority")?,
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

/// A priority list is strict: an empty entry is a typo, never an omitted constraint.
fn parse_priority_filter(inv: &Invocation<'_>, name: &str) -> Result<Vec<Option<u8>>> {
    let Some(value) = present_str(inv, name)? else {
        return Ok(Vec::new());
    };
    value
        .split(',')
        .map(str::trim)
        .map(|value| match value {
            "none" => Ok(None),
            "0" => Ok(Some(0)),
            "1" => Ok(Some(1)),
            "2" => Ok(Some(2)),
            "3" => Ok(Some(3)),
            "4" => Ok(Some(4)),
            other => Err(Error::InvalidArgument {
                name: name.to_string(),
                detail: format!("`{other}` is not one of 0, 1, 2, 3, 4, none (no priority set)"),
            }),
        })
        .collect()
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
struct ItemEndpoint {
    participants: Participants,
}

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
                let mut item = require_item(&client, id).await?;
                across::decorate(inv, &client, &mut item).await?;
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
                let mut fields = Vec::new();
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
                    let (title, body) = (literal(&title), literal(&body));
                    updates.push(replace_one(&client, &item.iri, v::ext::TITLE, &title));
                    fields.push(edited_field(&item.iri, "title", v::ext::TITLE, &title));
                    updates.push(replace_one(&client, &item.iri, v::BODY, &body));
                    fields.push(edited_field(&item.iri, "body", v::BODY, &body));
                }
                if let Ok(priority) = inv.inline_str("priority") {
                    let priority = integer(parse_priority(priority)?);
                    updates.push(replace_one(&client, &item.iri, v::PRIORITY, &priority));
                    fields.push(edited_field(&item.iri, "priority", v::PRIORITY, &priority));
                }
                if let Ok(revision) = inv.inline_str("revision") {
                    let revision = literal(revision);
                    updates.push(replace_one(&client, &item.iri, v::REVISION, &revision));
                    fields.push(edited_field(&item.iri, "revision", v::REVISION, &revision));
                }
                if let Ok(about) = inv.inline_str("about") {
                    for target in about.split_whitespace() {
                        let target = sparql::iri(target, "about")?;
                        updates.push(insert_on_live(
                            &client,
                            &[&item.iri],
                            &format!("<{}> <{}> {target} .", item.iri, v::ABOUT),
                        ));
                        fields.push(edited_field(&item.iri, "about", v::ABOUT, &target));
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
                // An edit an import principal dated at its source (ledger #2939).
                let (claimed, who) = importer_dated(
                    inv,
                    &client,
                    self.participants.as_deref(),
                    "an edit",
                    &item,
                    now,
                )
                .await?;
                updates.push(touch_dated(&client, &item.iri, now, claimed, ""));
                updates.insert(
                    0,
                    edit_event_naming(
                        &client,
                        &item.iri,
                        &[&item.iri],
                        "edit",
                        &who,
                        now,
                        &fields,
                    )?,
                );
                guarded_update(&client, &[&item.iri], &batch(&updates)).await?;
                Ok(plain(format!("updated {} {}\n", item.short(), item.iri)))
            }
            Verb::Delete => {
                let now = now_ms(inv)?;
                let item = require_item(&client, id).await?;
                across::refuse_delete_with_links(&client, &item).await?;
                let reason = inv.inline_str("reason").unwrap_or("").to_string();
                let who = attribution::resolve(
                    inv,
                    &client,
                    self.participants.as_deref(),
                    Dates::None,
                    now,
                )
                .await?;
                let Some(removed) = delete_item(&client, &item, &reason, &who, now, false).await?
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
                    .input(created_arg("an edit"))
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
                            .summary(
                                "Who deleted it. Optional: the committer, the principal this capability carries, is the author by default. Otherwise a principal's IRI the committer acts for in the host's participants graph, or, for an import principal, any actor; recorded as claimed (ledger #2827).",
                            )
                            .class(v::ext::XSD_STRING)
                            .optional(),
                    )
                    .output(PLAIN),
                CAP_DELETE,
            ))
    }
}

/// The name of a field an item edit is about to set to `value`, recorded only when that changes
/// it, and never its value (ledger #2841). An empty value and no value are the same here: an item
/// filed without a body holds no `ledger:body`, and a retitle writes an empty one.
fn edited_field(item: &str, name: &str, predicate: &'static str, value: &str) -> EditObject {
    EditObject {
        predicate: v::EDIT_FIELD,
        value: literal(name),
        changes: if value == literal("") {
            format!("<{item}> <{predicate}> ?held . FILTER(STR(?held) != \"\")")
        } else {
            absent(&format!("<{item}> <{predicate}> {value}"))
        },
    }
}

/// Replace a single-valued property of a live item ([`live`]).
fn replace_one(client: &StoreClient<'_, '_>, item: &str, predicate: &str, value: &str) -> String {
    replace_one_of(client, item, v::ITEM_CLASS, predicate, value)
}

/// [`replace_one`], only while `guard` (a pattern on the item) also holds.
fn replace_one_when(
    client: &StoreClient<'_, '_>,
    item: &str,
    predicate: &str,
    value: &str,
    guard: &str,
) -> String {
    format!(
        "DELETE {{ {} }} INSERT {{ {} }} WHERE {{ {} }}",
        client.in_graph(&format!("<{item}> <{predicate}> ?was")),
        client.in_graph(&format!("<{item}> <{predicate}> {value}")),
        client.in_graph(&format!(
            "{} {guard} OPTIONAL {{ <{item}> <{predicate}> ?was }}",
            live(item)
        ))
    )
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

/// Which of a ledger's two graphs an operation is about.
#[derive(Clone, Copy)]
enum LedgerGraph {
    Live,
    Graveyard,
}

impl LedgerGraph {
    fn wrap(self, client: &StoreClient<'_, '_>, body: &str) -> String {
        match self {
            LedgerGraph::Live => client.in_graph(body),
            LedgerGraph::Graveyard => client.in_deleted_graph(body),
        }
    }

    async fn select(self, client: &StoreClient<'_, '_>, query: &str) -> Result<Vec<sparql::Row>> {
        match self {
            LedgerGraph::Live => client.select(query).await,
            LedgerGraph::Graveyard => client.select_deleted(query).await,
        }
    }

    async fn update(self, client: &StoreClient<'_, '_>, update: &str) -> Result<()> {
        match self {
            LedgerGraph::Live => client.update(update).await,
            LedgerGraph::Graveyard => client.update_deleted(update).await,
        }
    }
}

/// Everything that goes with an item when it goes: its own triples, the triples of its
/// dependents (its comments, events and links, which hang off it by `ledger:onItem`), and every
/// edge POINTING AT it.
///
/// ⚠ An event's `ledger:editTarget` is not an edge (ledger #2841), and neither is a close's
/// `ledger:evidence` (ledger #2842): each is another item's history, saying what happened there
/// and then, and that stays when the item it names goes, as its IRI stays on a tombstone. Its own
/// events' targets go with their event, by `onItem`.
///
/// # ★ Why the dependents are read first and listed (ledger #2922, #2941)
///
/// The store's narrow write door evaluates an update against a private copy of the quads it can
/// observe, found by the constants in each triple pattern (`ikigai_store::confine`), and refuses
/// past `DurableStore::max_scoped_copy_quads`, 250,000 by default. Through 0.17.0 this set was
/// written `?s ?p ?o . FILTER(?s = <item> || …)`: a pattern with no constant, which observes the
/// graph as a whole. So a delete's commit and a purge's removals copied every quad of their graph,
/// and a ledger past the bound could delete and purge nothing. Here every pattern is anchored: the
/// item as subject, the item as object, and each dependent as subject. The store's copy is then
/// the item's own quads and nothing else; reads, which copy nothing, cost in proportion to the
/// item too, where the filter scanned the graph.
///
/// The dependents are what this cannot anchor without reading them first: `?s <onItem> <item> .
/// ?s ?p ?o` has a pattern with no constant, whatever the join says. So they are read (a narrow
/// read) and listed, in ONE `VALUES ?d { … } ?d ?p ?o` pattern inside the graph's group, which
/// the store narrows to the listed subjects since 0.3.3 (ledger #2940, #2941): one pattern for
/// any number of dependents. Until then each was a `UNION` branch of its own, three of the
/// store's 1,024 algebra operators apiece, so past 250 dependents the ledger fell back to the
/// whole-graph form below. A dependent written after that read is still seen: its own `onItem`
/// edge points at the item, so the item-as-object pattern matches it, and each use below says
/// what it does about one.
///
/// ⚠ [`Selector::Whole`] keeps the old form for the one case a list cannot serve: a dependent
/// that is a blank node, which has no name to put in it (this crate writes none; one can only
/// come from a write through the broad door). That behaves exactly as 0.17.0 did, bound
/// included.
enum Selector {
    Narrow {
        /// The item, as a SPARQL term.
        item: String,
        /// Each dependent, as a SPARQL term.
        dependents: Vec<String>,
    },
    Whole {
        item: String,
    },
}

impl Selector {
    /// Read `item`'s dependents in `graph` and list them, or fall back to [`Selector::Whole`]
    /// when one is a blank node.
    async fn read(
        client: &StoreClient<'_, '_>,
        graph: LedgerGraph,
        item: &str,
    ) -> Result<Selector> {
        let item = sparql::iri(item, "item")?;
        let rows = graph
            .select(
                client,
                &format!(
                    "SELECT DISTINCT ?s WHERE {{ {} }}",
                    graph.wrap(client, &format!("?s <{}> {item}", v::ON_ITEM))
                ),
            )
            .await?;
        let mut dependents = Vec::with_capacity(rows.len());
        for row in &rows {
            let Some(s) = row.get("s") else { continue };
            if s.kind != "uri" {
                return Ok(Selector::Whole { item });
            }
            dependents.push(sparql::term(&s.term("s")?));
        }
        Ok(Selector::Narrow { item, dependents })
    }

    /// The edges pointing at the item that go with it: every one but another item's history.
    fn inbound_filter(predicate: &str, also: &str) -> String {
        format!(
            "FILTER({predicate} NOT IN (<{}>, <{}>{also}))",
            v::EDIT_TARGET,
            v::EVIDENCE
        )
    }

    /// A pattern binding `?s ?p ?o` to each quad of the set, possibly more than once (a
    /// dependent's own `onItem` edge is also an edge pointing at the item): `SELECT DISTINCT`
    /// over it. ⚠ ONE line, like [`rows_as_values`] and for the same reason (ledger #2819).
    fn pattern(&self) -> String {
        match self {
            Selector::Narrow { item, dependents } => {
                let mut branches = vec![
                    format!("{{ {item} ?p ?o BIND({item} AS ?s) }}"),
                    format!(
                        "{{ ?s ?p {item} {} BIND({item} AS ?o) }}",
                        Self::inbound_filter("?p", "")
                    ),
                ];
                if !dependents.is_empty() {
                    branches.push(format!(
                        "{{ VALUES ?s {{ {} }} ?s ?p ?o }}",
                        dependents.join(" ")
                    ));
                }
                branches.join(" UNION ")
            }
            Selector::Whole { item } => format!(
                "?s ?p ?o . FILTER(?s = {item} || (?o = {item} && ?p NOT IN (<{target}>, \
                 <{evidence}>)) || EXISTS {{ ?s <{on}> {item} }})",
                on = v::ON_ITEM,
                target = v::EDIT_TARGET,
                evidence = v::EVIDENCE,
            ),
        }
    }

    /// `DELETE … WHERE` of the whole set in `graph`, under `guard` (a pattern over `graph`).
    ///
    /// A dependent written after [`Selector::read`] is left WHOLE, its `onItem` edge included,
    /// rather than half removed: a narrow removal names no pattern for its other quads, so taking
    /// its edge alone would orphan them where no later read could find them. With
    /// `refuse_unnamed`, the removal instead matches nothing at all while such a dependent exists,
    /// and its caller reads again.
    fn removal(
        &self,
        client: &StoreClient<'_, '_>,
        graph: LedgerGraph,
        guard: &str,
        refuse_unnamed: bool,
    ) -> String {
        match self {
            Selector::Narrow { item, dependents } => {
                let mut template = format!("{item} ?p0 ?o0 . ?s1 ?p1 {item} .");
                let mut branches = vec![
                    format!("{{ {item} ?p0 ?o0 }}"),
                    format!(
                        "{{ ?s1 ?p1 {item} {} }}",
                        Self::inbound_filter("?p1", &format!(", <{}>", v::ON_ITEM))
                    ),
                ];
                // ★ The template's `?d` is bound only by the `VALUES` branch, so the store
                // narrows it to the listed dependents too (ikigai-store 0.3.3, ledger #2940).
                if !dependents.is_empty() {
                    template.push_str(" ?d ?dp ?do .");
                    branches.push(format!(
                        "{{ VALUES ?d {{ {} }} ?d ?dp ?do }}",
                        dependents.join(" ")
                    ));
                }
                let unnamed = if refuse_unnamed {
                    self.no_unnamed_dependent()
                } else {
                    String::new()
                };
                format!(
                    "DELETE {{ {} }} WHERE {{ {} }}",
                    graph.wrap(client, &template),
                    graph.wrap(
                        client,
                        &format!("{guard} {unnamed} {}", branches.join(" UNION "))
                    )
                )
            }
            Selector::Whole { .. } => format!(
                "DELETE {{ {} }} WHERE {{ {} }}",
                graph.wrap(client, "?s ?p ?o"),
                graph.wrap(client, &format!("{guard} {}", self.pattern()))
            ),
        }
    }

    /// A filter that holds only while the item has no dependent this selector did not name: one
    /// written after [`Selector::read`]. Empty for [`Selector::Whole`], which names none and
    /// matches every dependent whenever it is evaluated.
    ///
    /// ★ A commit that removes what was read needs it as well as its rows check (ledger #2922):
    /// a dependent written between the read of the dependents and the read of the rows put its
    /// `onItem` edge, and only that, into the rows, so the rows check passed and the rest of it
    /// stayed live as an orphan. Found by `comment_delete_race`, 1 round in 300.
    ///
    /// ⚠ A sub-select counted ONCE, not a `FILTER NOT EXISTS`: a group's filter is evaluated
    /// for every solution of the group, and a purge's removal has one per quad of the item, so
    /// the check ran a thousand times over a list of hundreds (ledger #2941). A live purge of an
    /// item with 500 dependents took 77 s in a debug build that way, and under 0.3 s this way.
    fn no_unnamed_dependent(&self) -> String {
        match self {
            Selector::Narrow { item, dependents } => format!(
                "{{ SELECT (COUNT(?x) AS ?unnamedDependents) WHERE {{ ?x <{}> {item} \
                 MINUS {{ VALUES ?x {{ {} }} }} }} }} FILTER(?unnamedDependents = 0)",
                v::ON_ITEM,
                dependents.join(" ")
            ),
            Selector::Whole { .. } => String::new(),
        }
    }

    /// The quads of the set in `graph`, sorted, each once.
    async fn rows(
        &self,
        client: &StoreClient<'_, '_>,
        graph: LedgerGraph,
    ) -> Result<Vec<sparql::Row>> {
        graph
            .select(
                client,
                &format!(
                    "SELECT DISTINCT ?s ?p ?o WHERE {{ {} }} ORDER BY ?s ?p ?o",
                    graph.wrap(client, &format!("{{ {} }}", self.pattern()))
                ),
            )
            .await
    }
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

/// The same rows as a `VALUES (?s ?p ?o)` block's body, one `( s p o )` per row, all on ONE
/// line.
///
/// ⚠ **One line is load-bearing** (ledger #2819). The store's scanner reads a `<` inside
/// parentheses both as an IRI and as a less-than, and in the less-than reading the `#` of a
/// `ledger:` predicate opens a comment that runs to the end of the line. With a row per line
/// every row resumed that reading and forked it again, and past 64 open readings
/// (`MAX_SCAN_BRANCHES`) the store refused the update: an item with about twenty triples
/// beyond its own could not be deleted at all, and attribution events put that many on most
/// items that were ever claimed. On one line the comment runs past every row, so the
/// readings stay a handful whatever the row count. A literal's newline is escaped by the
/// store's term serializer, so no term breaks the line.
fn rows_as_values(rows: &[sparql::Row]) -> Result<String> {
    let mut values = Vec::new();
    for row in rows {
        let (Some(s), Some(p), Some(o)) = (row.get("s"), row.get("p"), row.get("o")) else {
            continue;
        };
        values.push(format!(
            "( {} {} {} )",
            sparql::term(&s.term("s")?),
            sparql::term(&p.term("p")?),
            sparql::term(&o.term("o")?)
        ));
    }
    Ok(values.join(" "))
}

/// How many times a recoverable delete reads and commits again when writes keep landing on
/// its item in between, before it refuses (see `delete_item`).
const DELETE_ATTEMPTS: usize = 16;

/// Move (or destroy) an item's quads and write its tombstone.
///
/// ★ The three sets that go together, because leaving any behind is a lie about what was
/// deleted: the item's own triples, the triples of its comments, and every edge POINTING
/// AT it (a `blocks` from another item would otherwise name something that no longer
/// resolves). [`Selector`] is that set.
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
/// was archived, what was removed, and what the tombstone hashes and counts.
///
/// ★ **And step 3 commits only if nothing raced it** (ledger #2780). Through 0.14.1 a write
/// that landed between the read and the commit was kept in the live graph beside the
/// tombstone, an orphan that no reader shows and that no later purge could reach, since a
/// purge of a deleted item touches only the graveyard and the tombstone: a pasted secret in a
/// racing comment could never be destroyed. Now the removal's `WHERE` holds only while the
/// item is live and nothing matches the selector beyond the rows read, and the tombstone is
/// written only once the item is no longer live. A write that raced makes the commit match
/// nothing, and the delete reads, archives and commits again, up to `DELETE_ATTEMPTS` times.
/// That retry is a guarantee rather than a loop that reopens the window, because every write
/// on an item carries [`live`] in its own update (ledger #764): a write lands before the
/// commit, and is then read by the retry, or after it, and then lands nowhere.
///
/// The same argument runs backwards for `purge`, which must clear both graphs: it empties
/// the **graveyard** first and the live graph second, so an interrupted purge leaves the
/// item fully live and re-purgeable, rather than leaving an orphan in a graveyard that no
/// resource can name once the live item it belonged to is gone. ⚠ A purge's live removal
/// stays `DELETE … WHERE`, on purpose: a purge is asked to leave nothing of the item, a
/// hand-written blank node as an object included (which `DELETE DATA` cannot name), so a write
/// racing a purge is destroyed with it and is not in the tombstone's count. Since ledger #2922
/// that removal lists the item's dependents ([`Selector`]), so it holds only while
/// the item has no dependent it did not read: a comment or event written in between makes it
/// match nothing, and the purge reads again, as a recoverable delete does.
///
/// # ★ What it costs (ledger #2922)
///
/// Every read and every update here names the item, or one of its dependents, in each
/// pattern, so the store's scoped door copies the item's quads and nothing more, and a read
/// looks up those and no others. Measured on a 996,147-quad ledger in a release build: a delete
/// in 0.07 s and about 1 MiB, a purge in 0.08 s, at the default copy bound. Through 0.17.0 the
/// same delete copied the whole graph, refused past 250,000 quads, and took 4.0 s and 1.9 GiB
/// with the bound raised to let it. Since store 0.3.3 (ledger #2941) that holds for any number
/// of dependents; through 0.18.0's first form it fell back to the whole graph past 250. Only a
/// blank-node dependent still does.
///
/// ⚠ A purge that runs whole inside a recoverable delete's window, between its read and its
/// archive, empties the graveyard before the archive lands (ledger #2779). The delete cannot
/// make its archive conditional on the graveyard, because it holds only the graveyard's write
/// grant and a `WHERE` there is a read. So a purge clears the graveyard once more after its
/// tombstone says purged, and a recoverable delete reads that mark after it archived and, if
/// it is there, removes exactly what it archived. Whichever of the two comes second cleans up.
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
    who: &Attribution,
    now: u64,
    destroy: bool,
) -> Result<Option<Removed>> {
    let id = item.iri.rsplit(':').next().unwrap_or("unknown").to_string();
    let tombstone = client.ledger().tombstone(&id);
    // What an earlier attempt archived, so a retry can take back what it no longer removes.
    let mut archived_before: Option<String> = None;
    // What a purge destroyed in the graveyard, over every attempt.
    let mut archived_total = 0;
    let mut attempt = 0;
    let (rows, digest, archived, archive) = loop {
        attempt += 1;
        // Read again on every attempt: a retry is for a write that landed, a new dependent
        // among them.
        let selector = Selector::read(client, LedgerGraph::Live, &item.iri).await?;
        let rows = selector.rows(client, LedgerGraph::Live).await?;
        let digest = content_hash(&rows);

        // Step 1 of 2 — the graveyard, which no read in this crate can see, so nothing a
        // reader observes has changed yet.
        //
        // ⚠ The DELETE TEMPLATE is a quad pattern and may not carry a FILTER — the selector
        // belongs in the WHERE clause only. Putting it in both is a parse error at the store,
        // which is at least loud.
        let mut archive = None;
        let removal = if destroy {
            // Everything an earlier recoverable delete quarantined. Read first, so the answer
            // can say how much it destroyed — and so the graveyard's READ grant, which the
            // store requires of a `DELETE … WHERE` over it, is a requirement here too rather
            // than a surprise at the next store upgrade.
            archived_total += clear_archive(client, &item.iri).await?;
            // Only while the item is live: once another delete committed, the selector's
            // `?o = <item>` would take that delete's tombstone edge with it. And only while
            // it has no dependent this attempt did not read: one written since matches
            // nothing here, and the purge reads again below.
            selector.removal(client, LedgerGraph::Live, &live(&item.iri), true)
        } else if rows.is_empty() {
            String::new()
        } else {
            // The quads read above, as data: a scoped update cannot read the live graph from
            // inside the graveyard's scope, so the WHERE clause that used to do this work is
            // now the SELECT that already ran.
            let data = rows_as_data(&rows)?;
            // A retry's archive replaces the last attempt's: what was edited away in between
            // is not what this delete removes, so it does not stay in the graveyard either.
            if let Some(before) = archived_before.take() {
                let now_read: HashSet<&str> = data.lines().collect();
                let stale: String = before
                    .lines()
                    .filter(|line| !now_read.contains(line))
                    .map(|line| format!("{line}\n"))
                    .collect();
                if !stale.is_empty() {
                    client
                        .update_deleted(&format!(
                            "DELETE DATA {{ {} }}",
                            client.in_deleted_graph(&stale)
                        ))
                        .await?;
                }
            }
            client
                .update_deleted(&format!(
                    "INSERT DATA {{ {} }}",
                    client.in_deleted_graph(&data)
                ))
                .await?;
            // ★ The commit decides inside the write (ledger #2780): it removes the quads read,
            // and only while the item is live and NOTHING else matches the selector. A write
            // that landed after the read (a comment, an edit) makes this match nothing, the
            // item stays live, and the delete reads again below.
            let removal = format!(
                "DELETE {{ {} }} WHERE {{ {} }}",
                client.in_graph(&data),
                client.in_graph(&format!(
                    "{} {} FILTER NOT EXISTS {{ {{ {} }} MINUS {{ VALUES (?s ?p ?o) {{ {} }} }} }}",
                    live(&item.iri),
                    selector.no_unnamed_dependent(),
                    selector.pattern(),
                    rows_as_values(&rows)?
                ))
            );
            archive = Some(data);
            removal
        };

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
        triples.push_str(&who.triples(&tombstone, now));
        // ★ The key goes onto the tombstone, in the same update that removes it from the item,
        // so there is no instant at which the key is free: a keyed append replayed after a
        // delete answers the deleted item instead of filing it again. Kept through a purge as
        // well — it is a name, like the number, not content.
        if let Some(key) = &item.key {
            triples.push_str(&format!("\n<{tombstone}> <{}> {} .", v::KEY, literal(key)));
        }
        // Step 2 of 2 — the live graph, which IS the commit point. Removing the item's quads
        // and writing its tombstone are one scoped update and therefore atomic: there is no
        // instant in which the item is gone and unaccounted for. The tombstone is written only
        // once the item is no longer live, so a removal that matched nothing writes none.
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
                    "INSERT {{ {} }} WHERE {{ FILTER NOT EXISTS {{ {} }} FILTER NOT EXISTS {{ {} }} }}",
                    client.in_graph(&triples),
                    client.in_graph(&format!(
                        "<{tombstone}> <{}> <{}>",
                        v::ext::TYPE,
                        v::TOMBSTONE_CLASS
                    )),
                    client.in_graph(&live(&item.iri)),
                ),
            ]))
            .await?;
        // An item still live after its commit lost the commit to a write, and is read again:
        // for a recoverable delete any write since the read, for a purge a new dependent (the
        // rest of a purge's removal is re-evaluated at its commit, so nothing else races it).
        if (!destroy && archive.is_none())
            || !client
                .ask(&format!("ASK {{ {} }}", client.in_graph(&live(&item.iri))))
                .await?
        {
            break (rows, digest, archived_total, archive);
        }
        if attempt == DELETE_ATTEMPTS {
            return Err(Error::Conflict(format!(
                "{} was not deleted: it was written to {DELETE_ATTEMPTS} times while this delete \
                 was in flight, and it is unchanged in the live graph. Delete it again",
                item.short()
            )));
        }
        archived_before = archive;
    };
    // ★ A purge that ran inside this delete's window (ledger #2779). A purge that read the
    // item live, or found another delete's tombstone, between this delete's read and its
    // archive cleared the graveyard BEFORE the archive landed, so the item's content would
    // now sit in the graveyard under a tombstone saying it was destroyed. A delete holds no
    // read grant on the graveyard, so its archive cannot be conditional there; instead every
    // purge clears the graveyard again AFTER it marks the tombstone purged, and every
    // recoverable delete looks for that mark AFTER it archived. One of the two always comes
    // second: an archive that lands after a purge's last clear also lands after its mark, and
    // then this read sees the mark and takes back exactly the quads it wrote.
    if let Some(data) = &archive {
        let purged = client
            .ask(&format!(
                "ASK {{ {} }}",
                client.in_graph(&format!("<{tombstone}> <{}> ?at", v::PURGED_AT))
            ))
            .await?;
        if purged {
            client
                .update_deleted(&format!(
                    "DELETE DATA {{ {} }}",
                    client.in_deleted_graph(data)
                ))
                .await?;
        }
    }
    if destroy {
        // The purge's half of the same handshake: a recoverable delete whose archive landed
        // after the clear above, and before the commit, is removed here.
        clear_archive(client, &item.iri).await?;
    }
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
    who: &Attribution,
    now: u64,
) -> Result<usize> {
    let archived = clear_archive(client, &found.item).await?;
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
    if let Some(author) = &who.author {
        operations.push(replace_one_of(
            client,
            &found.iri,
            v::TOMBSTONE_CLASS,
            v::PURGED_BY,
            &literal(author),
        ));
    }
    client.update(&batch(&operations)).await?;
    // ★ Again, now that the tombstone says purged (ledger #2779): a recoverable delete that
    // read the item live before another delete committed may archive it after the clear
    // above. If its archive lands before this clear, this clear removes it; if after, that
    // delete sees the mark and removes it itself (see `delete_item`).
    clear_archive(client, &found.item).await?;
    Ok(archived)
}

/// Destroy what the graveyard holds of `item` (its [`Selector`] there), and answer how many
/// quads that was when read.
///
/// Read first, so the answer can say how much it destroyed, and so the graveyard's READ grant,
/// which the store requires of a `DELETE … WHERE` over it, is a requirement here too rather than
/// a surprise at the next store upgrade. Nothing is written when nothing is there.
///
/// A dependent archived after the read is left whole, never half removed (see
/// [`Selector::removal`]): the next clear, or the recoverable delete that archived it, takes it
/// (ledger #2779's handshake, which is about the order of the read and the purge's mark).
async fn clear_archive(client: &StoreClient<'_, '_>, item: &str) -> Result<usize> {
    let selector = Selector::read(client, LedgerGraph::Graveyard, item).await?;
    let archived = selector.rows(client, LedgerGraph::Graveyard).await?.len();
    if archived > 0 {
        LedgerGraph::Graveyard
            .update(
                client,
                &selector.removal(client, LedgerGraph::Graveyard, "", false),
            )
            .await?;
    }
    Ok(archived)
}

// -------------------------------------------------------------------------- append

#[derive(Clone)]
struct AppendEndpoint {
    participants: Participants,
}

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
        // ⚠ The label grammar, as the label endpoint holds it (ledger #2634): `labels=` splits
        // only on commas, so `labels="a b"` stored ONE label with a space, and that was where
        // every out-of-grammar label in the live ledger came from. One bad label refuses the
        // whole append, before anything is written.
        let labels = split_labels(inv.inline_str("labels").ok());
        for label in &labels {
            check_label("labels", label)?;
        }
        let now = now_ms(inv)?;
        let (title, body) = split_content(inv.inline_str("content")?)?;
        // ★ Who is writing is checked before anything is written (ledger #2827): a refused
        // author or date files nothing.
        let who = attribution::resolve(
            inv,
            &client,
            self.participants.as_deref(),
            Dates::CreatedAndModified,
            now,
        )
        .await?;
        let id = mint_id(now)?;
        let iri = client.ledger().item(&id);

        let mut triples = format!(
            "<{iri}> <{type_}> <{class}> ; <{number}> ?new ; <{title_p}> {title_v} ; \
             <{status}> <{open}> ; <{created}> {filed} ; <{modified}> {changed} .",
            type_ = v::ext::TYPE,
            class = v::ITEM_CLASS,
            number = v::NUMBER,
            title_p = v::ext::TITLE,
            title_v = literal(&title),
            status = v::STATUS,
            open = v::OPEN,
            created = v::ext::CREATED,
            modified = v::ext::MODIFIED,
            filed = datetime(who.created_or(now)),
            changed = datetime(who.modified_or(now)),
        );
        triples.push_str(&who.triples(&iri, now));
        triples.push_str(&who.date_sources(&iri));
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
        for label in &labels {
            triples.push_str(&format!("\n<{iri}> <{}> {} .", v::LABEL, literal(label)));
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
                .resend_safe_with_key("key")
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
                        .summary(
                            "Comma-separated tags, each only A-Z a-z 0-9 and `.` `_` `:` `-`: \
                             no spaces.",
                        )
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
                        .summary(
                                "Who is filing it. Optional: the committer, the principal this capability carries, is the author by default. Otherwise a principal's IRI the committer acts for in the host's participants graph, or, for an import principal, any actor; recorded as claimed (ledger #2827).",
                            )
                        .class(v::ext::XSD_STRING)
                        .optional(),
                )
                .input(
                    ArgSpec::new("created")
                        .summary(
                            "An import principal's authored filing time (`xsd:dateTime`, no \
                             later than now), recorded as claimed. Refused from anyone else \
                             (ledger #2827).",
                        )
                        .class(v::ext::XSD_DATETIME)
                        .optional(),
                )
                .input(
                    ArgSpec::new("modified")
                        .summary(
                            "An import principal's authored last-change time, recorded as \
                             claimed; ranking reads it. Refused from anyone else (ledger #2827).",
                        )
                        .class(v::ext::XSD_DATETIME)
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
struct CommentEndpoint {
    participants: Participants,
}

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
        let epoch = parse_epoch(inv)?;
        let now = now_ms(inv)?;
        let item = require_item(&client, inv.inline_str("item")?).await?;
        let text = inv.inline_str("content")?;
        if text.trim().is_empty() {
            return Err(Error::InvalidArgument {
                name: "content".to_string(),
                detail: "an empty comment says nothing and cannot be edited afterwards".to_string(),
            });
        }
        let who = attribution::resolve(
            inv,
            &client,
            self.participants.as_deref(),
            Dates::Created,
            now,
        )
        .await?;
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
        // A fenced comment or hand-back (ledger #3205) lands only under the claim it names.
        check_fence(&item, epoch)?;
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
            &who,
            now,
            key.as_deref(),
            change,
            &fence_clause(&item.iri, epoch),
        )
        .await?;
        // A hand-back the write's guard refused: the item closed between the read above and the
        // write, and neither the comment nor `awaiting` landed. A keyed one whose key a racer
        // took is answered below as that racer's comment, as any taken key is.
        // A fenced one the guard refused found its claim changed, and wrote nothing either.
        if (change == Handback::Set || epoch.is_some())
            && !comment_written(&client, &comment).await?
        {
            let keyed = match &key {
                Some(key) => model::find_comment_by_key(&client, key).await?,
                None => None,
            };
            if keyed.is_none() {
                let now = model::load_item(&client, &item.iri).await?.unwrap_or(item);
                if let Some(epoch) = epoch {
                    if now.open || change != Handback::Set {
                        return Err(fence_refusal(&now, epoch));
                    }
                }
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
            let mut said = written(&comment, &item.iri, text.trim(), &who, now);
            // What it rejected is what the update found, so only the store can say.
            if change == Handback::Clear && item.awaiting.is_some() {
                said.rejects = model::load_rejects(&client, &comment).await?;
            }
            answer.comment = Some(said);
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
                    .resend_safe_with_key("key")
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
                            .summary(
                                "Who is commenting. Optional: the committer, the principal this capability carries, is the author by default. Otherwise a principal's IRI the committer acts for in the host's participants graph, or, for an import principal, any actor; recorded as claimed (ledger #2827).",
                            )
                            .class(v::ext::XSD_STRING)
                            .optional(),
                    )
                    .input(
                        ArgSpec::new("created")
                            .summary(
                                "An import principal's authored time for this comment \
                                 (`xsd:dateTime`, no later than now), recorded as claimed. \
                                 Refused from anyone else (ledger #2827).",
                            )
                            .class(v::ext::XSD_DATETIME)
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
                                 \"cleared\"` in JSON), and the comment names each hand-back \
                                 it rejected (`ledger:rejects`, `\"rejects\"` on the comment \
                                 in JSON, a `rejects:` line in text). Not with `handback=true`.",
                            )
                            .class(v::ext::XSD_BOOLEAN)
                            .one_of(["true", "false"])
                            .default_value("false")
                            .optional(),
                    )
                    .input(epoch_arg(
                        "the comment, and the hand-back it carries, is written",
                    ))
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
struct CloseEndpoint {
    participants: Participants,
}

#[async_trait]
impl Endpoint for CloseEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        if inv.request.verb != Verb::Sink {
            return Err(unsupported("ledger-close", inv.request.verb));
        }
        let client = StoreClient::new(inv, ledger_for(inv, Need::Close)?);
        let want = wanted_face(inv, &WRITE_FACES)?;
        let epoch = parse_epoch(inv)?;
        let now = now_ms(inv)?;
        let item = require_item(&client, inv.inline_str("item")?).await?;
        let name = inv.inline_str("reason").unwrap_or("done");
        // Every mirror of it in another ledger follows the close, so the caller must be able
        // to write them all; refused before anything is written (ledger #2829).
        across::require_mirror_grants(inv, &client, &item).await?;
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
        // What closed it (ledger #2842), every value checked before anything is written.
        let evidence = evidence::resolve(inv, &client, &item).await?;
        evidence::refuse_without_a_close_event(&client, &item, &evidence).await?;
        // A holder's fenced close (ledger #3205) closes only under the claim it names.
        check_fence(&item, epoch)?;
        // ★ A CLOSE RELEASES THE CLAIM (ledger #1147, decided 2026-10-10). Closed work is
        // held by nobody, and a claim left on a closed item was the doctor's orphaned
        // machine claim and a holder `items holder=` went on listing. The release is in the
        // same update as the close, so neither can land without the other, and it is
        // recorded as a comment naming who held it, so the record does not lose that.
        //
        // ★ AND THE UPDATE DECIDES, NOT THE READ ABOVE IT (ledger #2764). Until 0.14.0 the
        // close branched on `item.open` and `item.claimed_by` as read before the update, so
        // concurrent closes of one claimed item each recorded a close and each wrote the
        // release comment, and a claim that committed between the read and the update was
        // retracted with no record. Now the open-to-closed transition and the release
        // record carry their condition in their own `WHERE`, ahead of the operations that
        // change status and claim: under the store's write lock exactly one update finds
        // the item open, and only an update that finds a claim records it, naming the claim
        // it found. Each writes an IRI minted here, and reading those back is how this
        // request learns what it did.
        //
        // ★ A FENCED close (ledger #3205) carries its fence in every operation, and the claim
        // it names is retracted LAST, `claimedBy` after every other field: each operation
        // finds the claim the fence names until the close is otherwise complete, so the batch
        // lands whole or not at all.
        let who =
            attribution::resolve(inv, &client, self.participants.as_deref(), Dates::None, now)
                .await?;
        // ★ An import principal may date the close at its source (ledger #2819), as it may
        // date an item's filing; anyone else is refused, with the reason and the remedy.
        let claimed = attribution::resolve_closed_at(
            inv,
            &client,
            self.participants.as_deref(),
            item.created,
            now,
        )
        .await?;
        // ★ AND THE ITEM'S STATUS EVENTS ARE LISTED (ledger #3256): read first and named in
        // every pattern on them, so the store's scoped copy is this item's events and not the
        // ledger's ([`counts::StatusEvents`]). Every operation holds only while that list is
        // whole; a status event written since the read refuses the batch whole, and the close
        // is made again from a fresh read.
        let mut records = Vec::new();
        let mut attempt = 0;
        let closed_here = loop {
            attempt += 1;
            let mut events = counts::StatusEvents::read(&client, &item.iri).await?;
            let event_iri = events.mint(now)?;
            let fence = format!("{} {}", fence_clause(&item.iri, epoch), events.unchanged());
            let still_open = format!(
                "FILTER NOT EXISTS {{ <{}> <{}> <{}> }} {fence}",
                item.iri,
                v::STATUS,
                v::CLOSED
            );
            let event = counts::status_event_when(
                &client,
                &item.iri,
                &event_iri,
                &events,
                v::CLOSED,
                now,
                &still_open,
                &who,
                claimed,
            );
            let record = client.ledger().comment(&mint_id(now)?);
            let mut ops = vec![
                // Editing the verdict of an already closed item must not move its close time.
                event,
                replace_one_when(
                    &client,
                    &item.iri,
                    v::CLOSED_AT,
                    &datetime(claimed.unwrap_or(now)),
                    &still_open,
                ),
                format!(
                    "DELETE {{ {} }} WHERE {{ {} }}",
                    client.in_graph(&format!("<{}> <{}> ?was", item.iri, v::CLOSED_AT_SOURCE)),
                    client.in_graph(&format!(
                        "<{}> <{}> ?was . {still_open}",
                        item.iri,
                        v::CLOSED_AT_SOURCE
                    ))
                ),
                if claimed.is_some() {
                    format!(
                        "INSERT {{ {} }} WHERE {{ {} }}",
                        client.in_graph(&format!(
                            "<{}> <{}> {} .",
                            item.iri,
                            v::CLOSED_AT_SOURCE,
                            literal(attribution::IMPORT)
                        )),
                        client.in_graph(&format!("{} {still_open}", live(&item.iri)))
                    )
                } else {
                    String::new()
                },
                release_record(&client, &item.iri, &record, &who, now, &fence),
                // And the release as an event, who held it and who released it (ledger #2819),
                // under the same condition: only an update that finds a claim records one.
                release_event(&client, &item.iri, ReleaseReason::Closed, &fence, &who, now)?,
                replace_one_when(
                    &client,
                    &item.iri,
                    v::STATUS,
                    &format!("<{}>", v::CLOSED),
                    &fence,
                ),
                replace_one_when(
                    &client,
                    &item.iri,
                    v::CLOSED_REASON,
                    &format!("<{reason}>"),
                    &fence,
                ),
                // After the status, so it finds the close that stands: this one's event, or, when
                // the item was already closed, that close's.
                evidence::record(&client, &item.iri, &events, reason, &evidence, &fence),
                // A close dated at the source moves `modified` only forward (ledger #2928).
                touch_dated(&client, &item.iri, now, claimed, &fence),
                // And a hand-back is answered: closed work awaits nobody (ledger #1199).
                retract_when(&client, &item.iri, v::AWAITING, &fence),
                replace_one_when(&client, &item.iri, v::AWAITING_AT, &datetime(now), &fence),
            ];
            // The claim last, its holder after every other field: see the fence above.
            ops.extend(
                CLAIM_FIELDS
                    .iter()
                    .filter(|field| **field != v::CLAIMED_BY)
                    .chain([&v::CLAIMED_BY])
                    .map(|field| retract_when(&client, &item.iri, field, &fence)),
            );
            guarded_update(&client, &[&item.iri], &batch(&ops)).await?;
            let here = client
                .ask(&format!(
                    "ASK {{ {} }}",
                    client.in_graph(&format!("<{event_iri}> <{}> ?any", v::ext::TYPE))
                ))
                .await?;
            records.push(record);
            if here || !events.changed(&client).await? {
                break here;
            }
            if attempt == STATUS_ATTEMPTS {
                return Err(status_kept_changing(&item, "close"));
            }
        };
        // A fenced close records its own close or nothing at all: none means its claim
        // changed between the read and the write, and the fence refused every operation.
        if let Some(epoch) = epoch {
            if !closed_here {
                return Err(fence_refusal_now(&client, &item, epoch).await);
            }
        }
        // Even when another close got there first: a write from the latest snapshot is never
        // wrong, and it repairs a mirror an interrupted writer left behind.
        let unreached = across::sync(inv, &client, &item.iri, None, true, now).await?;
        let mut released = None;
        for record in &records {
            if released.is_none() {
                released = released_by(&client, record).await?;
            }
        }
        let mut note_written = None;
        if let Ok(note) = inv.inline_str("content") {
            if !note.trim().is_empty() {
                let comment = write_comment(&client, &item.iri, note.trim(), &who, now).await?;
                note_written = Some(written(&comment, &item.iri, note.trim(), &who, now));
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
            answer.already_closed = (!closed_here).then_some(true);
            evidence::answer(&mut answer, name, &evidence);
            return Ok(json_repr(json::render(&answer)?));
        }
        let head = evidence::receipt(&item, name, &evidence);
        let mut said = match (released, closed_here) {
            (Some(holder), _) => format!("{head}, released {holder}\n"),
            (None, true) => format!("{head}\n"),
            (None, false) if evidence.is_empty() => {
                format!("{head}; it was already closed, so only the verdict changed\n")
            }
            (None, false) => format!(
                "{head}; it was already closed, so the verdict changed and this evidence joined \
                 that close\n"
            ),
        };
        said.push_str(&across::unreached(&item, &unreached));
        Ok(plain(said))
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
                 hand-back awaiting the dispatcher is answered. What closed it can be recorded \
                 on the close's status event, `evidence=`.",
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
                            .summary(
                                "Who is closing it. Optional: the committer, the principal this capability carries, is the author by default. Otherwise a principal's IRI the committer acts for in the host's participants graph, or, for an import principal, any actor; recorded as claimed (ledger #2827).",
                            )
                            .class(v::ext::XSD_STRING)
                            .optional(),
                    )
                    .input(
                        ArgSpec::new("closed-at")
                            .summary(
                                "When the item closed at its source, for an import principal (ledger:Importer in the host's participants graph) only: an xsd:dateTime no later than now and no earlier than the item's filing. Dates closedAt and the close's status event, so the daily counts place it on that day, both marked closedAtSource import (ledger #2819). Anyone else sending it is refused.",
                            )
                            .class(v::ext::XSD_DATETIME)
                            .optional(),
                    )
                    .input(
                        ArgSpec::new("evidence")
                            .summary(
                                "What closed it, repeatable: recorded as ledger:evidence on this close's status event, and kept there across a reopen (ledger #2842). A commit on a `done` close is the fix; an item on a `duplicate` or `superseded` close is its target; a test is the proof. An item (`#12`, `12`, `key:{key}`, its IRI, an alias's IRI, or `{ledger}#12`) must exist and be readable by the closer, or the close is refused; one in another ledger also takes `urn:cap:ledger:link-in:{ledger}`. Any other IRI is stored as given and never fetched. On an item already closed, it joins the close that stands.",
                            )
                            .class(v::ext::XSD_STRING)
                            .list(ListSeparator::Whitespace)
                            .optional(),
                    )
                    .input(epoch_arg("the close is written"))
                    .input(as_arg(&WRITE_FACES))
                    .output(PLAIN)
                    .output(JSON),
                CAP_ANY,
            ))
    }
}

#[derive(Clone)]
struct ReopenEndpoint {
    participants: Participants,
}

#[async_trait]
impl Endpoint for ReopenEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        if inv.request.verb != Verb::Sink {
            return Err(unsupported("ledger-reopen", inv.request.verb));
        }
        let client = StoreClient::new(inv, ledger_for(inv, Need::Close)?);
        let now = now_ms(inv)?;
        let item = require_item(&client, inv.inline_str("item")?).await?;
        let mut who = attribution::resolve(
            inv,
            &client,
            self.participants.as_deref(),
            Dates::Event,
            now,
        )
        .await?;
        // A reopen an import principal dated at its source (ledger #2939), as a close may be:
        // the status event and a note carry the date, and `modified` moves only forward.
        let claimed = attribution::resolve_claimed(
            inv,
            &client,
            self.participants.as_deref(),
            "created",
            "a reopen",
            item.created,
            now,
        )
        .await?;
        who.created = claimed;
        // ★ THE UPDATE DECIDES, AS IT DOES FOR CLOSE (ledger #2766, after #2764). Until
        // 0.14.1 the reopen required the item and then wrote unconditionally, so reopening an
        // open item appended an `open` status event for a close that never happened, and
        // concurrent reopens of one closed item each appended one. Now every write carries
        // "the item is closed" in its own `WHERE`, ahead of the operation that sets it open:
        // under the store's write lock exactly one update finds it closed. Reading back the
        // event IRI minted here is how this request learns whether it was that one.
        across::require_mirror_grants(inv, &client, &item).await?;
        // ★ Its status events are listed, and the batch made again when one lands since the
        // read, as for close (ledger #3256, [`counts::StatusEvents`]).
        let mut attempt = 0;
        let reopened_here = loop {
            attempt += 1;
            let mut events = counts::StatusEvents::read(&client, &item.iri).await?;
            let preserved = events.mint(now)?;
            let event_iri = events.mint(now)?;
            let still_closed = format!(
                "<{}> <{}> <{}> . {}",
                item.iri,
                v::STATUS,
                v::CLOSED,
                events.unchanged()
            );
            let event = counts::status_event_when(
                &client,
                &item.iri,
                &event_iri,
                &events,
                v::OPEN,
                now,
                &still_closed,
                &who,
                claimed,
            );
            guarded_update(
                &client,
                &[&item.iri],
                &batch(&[
                    counts::preserve_close(
                        &client,
                        &item.iri,
                        &preserved,
                        &events,
                        &events.unchanged(),
                    ),
                    event,
                    retract_when(&client, &item.iri, v::CLOSED_REASON, &still_closed),
                    retract_when(&client, &item.iri, v::CLOSED_AT, &still_closed),
                    retract_when(&client, &item.iri, v::CLOSED_AT_SOURCE, &still_closed),
                    // A reopened item starts from "not awaiting": whatever hand-back preceded
                    // the close was answered by it (ledger #1199).
                    retract_when(&client, &item.iri, v::AWAITING, &still_closed),
                    replace_one_when(
                        &client,
                        &item.iri,
                        v::AWAITING_AT,
                        &datetime(now),
                        &still_closed,
                    ),
                    touch_dated(&client, &item.iri, now, claimed, &still_closed),
                    replace_one_when(
                        &client,
                        &item.iri,
                        v::STATUS,
                        &format!("<{}>", v::OPEN),
                        &still_closed,
                    ),
                ]),
            )
            .await?;
            let here = client
                .ask(&format!(
                    "ASK {{ {} }}",
                    client.in_graph(&format!("<{event_iri}> <{}> ?any", v::ext::TYPE))
                ))
                .await?;
            if here || !events.changed(&client).await? {
                break here;
            }
            if attempt == STATUS_ATTEMPTS {
                return Err(status_kept_changing(&item, "reopen"));
            }
        };
        let unreached = across::sync(inv, &client, &item.iri, None, true, now).await?;
        let mut noted = false;
        if let Ok(note) = inv.inline_str("content") {
            if !note.trim().is_empty() {
                write_comment(&client, &item.iri, note.trim(), &who, now).await?;
                noted = true;
            }
        }
        let mut said = match (reopened_here, noted) {
            (true, _) => format!("reopened {}\n", item.short()),
            (false, false) => format!(
                "{} was already open, so the reopen changed nothing\n",
                item.short()
            ),
            (false, true) => format!(
                "{} was already open, so the reopen only added the note\n",
                item.short()
            ),
        };
        said.push_str(&across::unreached(&item, &unreached));
        Ok(plain(said))
    }

    fn name(&self) -> &str {
        "ledger-reopen"
    }

    fn describe(&self) -> Description {
        Description::new("ledger-reopen")
            .title("Reopen a ledger item")
            .summary(
                "Undo a close: the status goes back to open, and the reason and any hand-back \
                 are dropped. Reopening an item that is already open changes nothing.",
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
                            .summary(
                                "Who is reopening it. Optional: the committer, the principal this capability carries, is the author by default. Otherwise a principal's IRI the committer acts for in the host's participants graph, or, for an import principal, any actor; recorded as claimed (ledger #2827).",
                            )
                            .class(v::ext::XSD_STRING)
                            .optional(),
                    )
                    .input(created_arg("a reopen"))
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

/// The prefix and suffix of the comment a close writes when it releases a claim.
const RELEASE_PREFIX: &str = "Closed while claimed by ";
const RELEASE_SUFFIX: &str = "; the claim is released.";

/// The comment `record` recording the release of the claim on `item`, written only when the
/// update finds one, and naming the holder it finds there as [`holder_display`] does
/// (ledger #2764). It must run ahead of the operations that retract the claim.
fn release_record(
    client: &StoreClient<'_, '_>,
    item: &str,
    record: &str,
    who: &Attribution,
    now: u64,
    guard: &str,
) -> String {
    let mut triples = format!(
        "<{record}> <{type_}> <{class}> ; <{on}> <{item}> ; <{body}> ?body ; <{created}> {when} .",
        type_ = v::ext::TYPE,
        class = v::COMMENT_CLASS,
        on = v::ON_ITEM,
        body = v::BODY,
        created = v::ext::CREATED,
        when = datetime(now),
    );
    triples.push_str(&who.triples(record, now));
    format!(
        "INSERT {{ {} }} WHERE {{ {} }}",
        client.in_graph(&triples),
        client.in_graph(&format!(
            "{live} {guard} <{item}> <{by}> ?holder . OPTIONAL {{ <{item}> <{principal}> ?principal }}\n\
             BIND(CONCAT({prefix}, STR(?holder), \
             COALESCE(CONCAT(\" (as \", STR(?principal), \")\"), \"\"), {suffix}) AS ?body)",
            live = live(item),
            by = v::CLAIMED_BY,
            principal = v::CLAIM_PRINCIPAL,
            prefix = literal(RELEASE_PREFIX),
            suffix = literal(RELEASE_SUFFIX),
        ))
    )
}

/// The filter that leaves out an imported `event` (ledger #2843): history only, so it counts
/// toward nothing a write decides. See [`import`].
///
/// ⚠ It reads `ledger:eventKey`, which only a history batch writes, never `ledger:createdSource`:
/// a link or a state move an importer dates at its source (ledger #2934) carries that mark too,
/// and it DID change its item, so it moves a mirror's version like any other link.
fn not_imported(event: &str) -> String {
    format!(
        "FILTER NOT EXISTS {{ {event} <{}> ?imported }}",
        v::EVENT_KEY
    )
}

/// The triples every attribution event carries (ledger #2819): its class, its item, when, and
/// who committed it. ★ Every event hangs off its item by `onItem`, so a delete, an archive
/// and a purge own it as they own a status event, and every one is written in the update
/// that makes the change it records, under that update's guard: a write that loses its race
/// leaves no event.
///
/// An event an import principal dated at its source (a link's or a state move's `created=`,
/// ledger #2934) carries that date as `dcterms:created`, marked `ledger:createdSource "import"`
/// beside the true `ledger:committedAt`.
fn event_triples(event: &str, class: &str, item: &str, who: &Attribution, now: u64) -> String {
    let mut out = format!(
        "<{event}> <{}> <{class}> ; <{}> <{item}> ; <{}> {} .",
        v::ext::TYPE,
        v::ON_ITEM,
        v::ext::CREATED,
        datetime(who.created_or(now))
    );
    out.push_str(&who.commit(event, now));
    out.push_str(&who.date_sources(event));
    out
}

/// A fresh event IRI beneath `item`: `{item}:{segment}:{id}`, skolemized like a status event.
fn event_iri(item: &str, segment: &str, now: u64) -> Result<String> {
    Ok(format!("{item}:{segment}:{}", mint_id(now)?))
}

/// Why a claim ended: the value of `ledger:releaseReason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReleaseReason {
    /// A release, by the holder or a dispatcher's override.
    Released,
    /// A close, which releases the claim.
    Closed,
    /// A takeover, which replaced it.
    Takeover,
}

impl ReleaseReason {
    fn as_str(self) -> &'static str {
        match self {
            ReleaseReason::Released => "released",
            ReleaseReason::Closed => "closed",
            ReleaseReason::Takeover => "takeover",
        }
    }
}

/// The release event for the claim on `item` (ledger #2819): who held it, as whom, since
/// when, why it ended, and who ended it (the committer). Written only when the update finds a
/// claim, and only while `guard` (a pattern over the ledger's graph) holds, so it lands
/// exactly when the release does. It must run ahead of the operations that retract the claim.
fn release_event(
    client: &StoreClient<'_, '_>,
    item: &str,
    reason: ReleaseReason,
    guard: &str,
    who: &Attribution,
    now: u64,
) -> Result<String> {
    let event = event_iri(item, "release", now)?;
    Ok(release_event_at(
        client, &event, item, reason, guard, who, now,
    ))
}

/// [`release_event`] under an IRI the caller minted, so it can ask afterwards whether the
/// event, and with it the release, landed.
fn release_event_at(
    client: &StoreClient<'_, '_>,
    event: &str,
    item: &str,
    reason: ReleaseReason,
    guard: &str,
    who: &Attribution,
    now: u64,
) -> String {
    let mut triples = event_triples(event, v::RELEASE_EVENT_CLASS, item, who, now);
    // An unbound variable drops only its own triple, so a claim with no principal, kind or
    // time records the rest.
    triples.push_str(&format!(
        "\n<{event}> <{by}> ?holder ; <{why}> {reason} .\n<{event}> <{principal}> ?principal .\n\
         <{event}> <{kind}> ?kind .\n<{event}> <{at}> ?claimedAt .",
        by = v::CLAIMED_BY,
        why = v::RELEASE_REASON,
        reason = literal(reason.as_str()),
        principal = v::CLAIM_PRINCIPAL,
        kind = v::CLAIM_KIND,
        at = v::CLAIMED_AT,
    ));
    format!(
        "INSERT {{ {} }} WHERE {{ {} }}",
        client.in_graph(&triples),
        client.in_graph(&format!(
            "{live} {guard}\n<{item}> <{by}> ?holder .\n\
             OPTIONAL {{ <{item}> <{principal}> ?principal }}\n\
             OPTIONAL {{ <{item}> <{kind}> ?kind }}\n\
             OPTIONAL {{ <{item}> <{at}> ?claimedAt }}",
            live = live(item),
            by = v::CLAIMED_BY,
            principal = v::CLAIM_PRINCIPAL,
            kind = v::CLAIM_KIND,
            at = v::CLAIMED_AT,
        ))
    )
}

/// An edit event (ledger #2819): an edit that creates no node of its own (`kind`: `label`,
/// `link`, `defer`, `about`, `edit` …) records who made it, as one operation of the edit's own
/// update, while the item is live.
fn edit_event(
    client: &StoreClient<'_, '_>,
    item: &str,
    kind: &str,
    who: &Attribution,
    now: u64,
) -> Result<String> {
    edit_event_naming(client, item, &[item], kind, who, now, &[])
}

/// One fact an edit event records about what it changed (ledger #2841).
struct EditObject {
    predicate: &'static str,
    value: String,
    /// The pattern that holds only while this write will change something: see
    /// [`edit_event_naming`].
    changes: String,
}

/// The pattern "`triple` is not there yet": an insert of it is a change.
fn absent(triple: &str) -> String {
    format!("FILTER NOT EXISTS {{ {triple} }}")
}

/// An [`edit_event`] that names its object (ledger #2841), live while all of `ends` are.
///
/// ★ **Whether this write changes anything is decided inside the update, never read and then
/// written** (the 0.14.x races, ledger #2764, #2766): each object is its own operation, whose
/// `WHERE` is evaluated under the store's write lock. So this goes AHEAD of the change in the
/// batch: its guards read the graph as the change finds it. A write that changes nothing still
/// has its event, since every write has a who, and names no object, so "when was this label
/// removed" is not answered by the removal of a label the item never had.
fn edit_event_naming(
    client: &StoreClient<'_, '_>,
    item: &str,
    ends: &[&str],
    kind: &str,
    who: &Attribution,
    now: u64,
    objects: &[EditObject],
) -> Result<String> {
    let event = event_iri(item, "edit", now)?;
    let guard: String = ends.iter().map(|end| live(end)).collect();
    let mut operations = vec![insert_on_live(
        client,
        ends,
        &edit_event_triples(&event, item, kind, who, now),
    )];
    operations.extend(objects.iter().map(|object| {
        format!(
            "INSERT {{ {} }} WHERE {{ {} }}",
            client.in_graph(&format!(
                "<{event}> <{}> {} .",
                object.predicate, object.value
            )),
            client.in_graph(&format!("{guard} {}", object.changes))
        )
    }));
    Ok(batch(&operations))
}

/// The triples of an [`edit_event`], for an update that carries its own guard (a lifecycle
/// transition's compare-and-set).
pub(super) fn edit_event_triples(
    event: &str,
    item: &str,
    kind: &str,
    who: &Attribution,
    now: u64,
) -> String {
    let mut triples = event_triples(event, v::EDIT_EVENT_CLASS, item, who, now);
    triples.push_str(&format!(
        "\n<{event}> <{}> {} .",
        v::EDIT_KIND,
        literal(kind)
    ));
    triples
}

/// The holder whose claim the update that wrote [`release_record`] `record` released, read
/// back from the record; `None` when that update found no claim.
async fn released_by(client: &StoreClient<'_, '_>, record: &str) -> Result<Option<String>> {
    let rows = client
        .select(&format!(
            "SELECT ?body WHERE {{ {} }}",
            client.in_graph(&format!("<{record}> <{}> ?body", v::BODY))
        ))
        .await?;
    Ok(rows
        .iter()
        .filter_map(|row| row.get("body"))
        .find_map(|body| {
            let held = body
                .value
                .strip_prefix(RELEASE_PREFIX)?
                .strip_suffix(RELEASE_SUFFIX)?;
            // `holder (as principal)` when the claim carried a principal: the label is the
            // part before it, as `released` has always named it.
            Some(
                match held.strip_suffix(')').and_then(|h| h.rsplit_once(" (as ")) {
                    Some((label, _)) => label.to_string(),
                    None => held.to_string(),
                },
            )
        }))
}

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

/// `epoch=`: the claim epoch a holder's write is fenced with (ledger #3205), or `None` when
/// the write is not fenced and behaves as it always did.
fn parse_epoch(inv: &Invocation<'_>) -> Result<Option<u64>> {
    match inv.inline_str("epoch").ok().map(str::trim) {
        None => Ok(None),
        Some(text) => match text.parse::<u64>() {
            Ok(epoch) if epoch > 0 => Ok(Some(epoch)),
            _ => Err(Error::InvalidArgument {
                name: "epoch".to_string(),
                detail: format!(
                    "`{text}` is not a claim epoch: a whole number from 1, as a claim's answer \
                     and its item's faces give it"
                ),
            }),
        },
    }
}

/// The `epoch=` argument as an action declares it, on every write a holder fences.
fn epoch_arg(what: &str) -> ArgSpec {
    ArgSpec::new("epoch")
        .summary(format!(
            "The claim epoch this holder was given (the claim's answer, `epoch:` in the item's \
             detail, `claim.epoch` in JSON). When given, {what} only while the item is claimed \
             at exactly this epoch: otherwise refused (Conflict) naming the current holder and \
             epoch, and nothing is written. The check is a clause of the write's own update, \
             so a takeover that lands first is never written past. Omit it for the unfenced \
             write (ledger #3205)."
        ))
        .class(v::ext::XSD_INTEGER)
        .optional()
}

/// The clause that holds only while `item` is claimed at `epoch`, for the `WHERE` of every
/// operation of a fenced write; empty for an unfenced one.
///
/// ★ It names the claim, not only the number: an epoch outlives its claim (a release keeps
/// it), so a fenced write after a release is refused rather than landing on a free item.
fn fence_clause(item: &str, epoch: Option<u64>) -> String {
    match epoch {
        None => String::new(),
        Some(epoch) => format!(
            "<{item}> <{}> ?fenceHolder ; <{}> {epoch} .",
            v::CLAIMED_BY,
            v::CLAIM_EPOCH
        ),
    }
}

/// How many times a close or a reopen is made when a status event of its item lands between
/// its read of them and its write (ledger #3256, [`counts::StatusEvents`]). Each such event is
/// another close or reopen of the same item, so this many in a row is a fight, not a race.
const STATUS_ATTEMPTS: usize = 8;

/// The refusal of a close or reopen whose item's status changed under it every time it read.
fn status_kept_changing(item: &Item, what: &str) -> Error {
    Error::Conflict(format!(
        "the status of {} changed {STATUS_ATTEMPTS} times while this {what} was being \
         written, so the {what} may not have been recorded. Read the item to see its status \
         now, and {what} it again if that is still wanted",
        item.short()
    ))
}

/// Whether `epoch` is the current claim's on `item`, as read.
fn epoch_is_current(item: &Item, epoch: u64) -> bool {
    item.claimed_by.is_some() && item.claim_epoch == Some(epoch)
}

/// The refusal of a write fenced with an `epoch` that is not current on `item` (ledger
/// #3205): who holds it now and at which epoch, or that nobody does.
fn fence_refusal(item: &Item, epoch: u64) -> Error {
    let short = item.short();
    Error::Conflict(match (&item.claimed_by, item.claim_epoch) {
        (Some(_), Some(current)) => format!(
            "{short} is held by {} at epoch {current}, not epoch {epoch}: a write fenced with \
             `epoch=` lands only while that claim is current, so nothing was written",
            holder_display(item)
        ),
        (Some(_), None) => format!(
            "{short} is held by {} under a claim taken before epochs, not epoch {epoch}: a \
             write fenced with `epoch=` lands only while that claim is current, so nothing \
             was written",
            holder_display(item)
        ),
        (None, Some(last)) => format!(
            "{short} is not claimed (its last claim was epoch {last}), so epoch {epoch} is not \
             current and nothing was written"
        ),
        (None, None) => format!(
            "{short} has never been claimed, so epoch {epoch} is not current and nothing was \
             written"
        ),
    })
}

/// [`fence_refusal`] when `epoch` was given and is not current on `item` as read: the cheap
/// half, so a stale holder's write is refused before anything is written. The write's own
/// [`fence_clause`] is what holds against a racer.
fn check_fence(item: &Item, epoch: Option<u64>) -> Result<()> {
    match epoch {
        Some(epoch) if !epoch_is_current(item, epoch) => Err(fence_refusal(item, epoch)),
        _ => Ok(()),
    }
}

/// [`fence_refusal`] as the item stands now, for a fenced write whose update found its
/// claim gone between the read and the write.
async fn fence_refusal_now(client: &StoreClient<'_, '_>, item: &Item, epoch: u64) -> Error {
    match model::load_item(client, &item.iri).await {
        Ok(Some(now)) => fence_refusal(&now, epoch),
        Ok(None) => Error::NotFound(format!(
            "no ledger item at `{}`: it was deleted while this write was in flight",
            item.iri
        )),
        Err(e) => e,
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
///
/// ★ And the events with it (ledger #2819), ahead of the operation that changes the claim and
/// under the same guard: a takeover's release of the claim it replaces, and a claim event for
/// a first claim or a takeover. A renewal (`renew=true`, or a plain claim by the holder that
/// already holds it) records none, which is why a first claim's event also needs the item
/// unclaimed. Of concurrent claims of one free item only the first finds it free, so one
/// claim event is written.
///
/// ★ **And the epoch with it** (ledger #3205), decided by the same update: a first claim or a
/// takeover sets `ledger:claimEpoch` to one more than the item's previous one, and a renewal
/// keeps it. It is never retracted, so a claim taken after a release still counts up. `fence`
/// is a fenced claim's [`fence_clause`], carried by every operation.
#[allow(clippy::too_many_arguments)] // reason: one SPARQL statement's inputs, each distinct
fn claim_update(
    client: &StoreClient<'_, '_>,
    item: &str,
    guard: Guard<'_>,
    fence: &str,
    holder: &str,
    principal: Option<&str>,
    purpose: Option<&str>,
    kind: crate::claim::ClaimKind,
    lease: Option<&crate::claim::Lease>,
    who: &Attribution,
    now: u64,
) -> Result<String> {
    let takeover = matches!(guard, Guard::ExpiredFrom(..));
    let taken_from = match guard {
        Guard::ExpiredFrom(from, _) => Some(from),
        _ => None,
    };
    let records_a_claim = !matches!(guard, Guard::Mine(_));
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
    let open = format!("<{item}> <{}> <{}> .\n{guard}\n{fence}", v::STATUS, v::OPEN);
    // The epoch: up by one for a first claim (the item free, `?o0`, its holder, unbound) and
    // for a takeover; kept by a renewal, which finds this holder there. A claim taken before
    // epochs has none to keep, and is given the first.
    let epoch = if takeover {
        "COALESCE(?epochWas, 0) + 1".to_string()
    } else {
        "IF(BOUND(?o0) && BOUND(?epochWas), ?epochWas, COALESCE(?epochWas, 0) + 1)".to_string()
    };
    let epoch_delete = format!("<{item}> <{}> ?epochWas .", v::CLAIM_EPOCH);
    let epoch_insert = format!("\n<{item}> <{}> ?epoch .", v::CLAIM_EPOCH);
    let epoch_where = format!(
        "OPTIONAL {{ <{item}> <{}> ?epochWas }}\nBIND({epoch} AS ?epoch)",
        v::CLAIM_EPOCH
    );
    let mut ops = Vec::new();
    if takeover {
        ops.push(release_event(
            client,
            item,
            ReleaseReason::Takeover,
            &open,
            who,
            now,
        )?);
    }
    if records_a_claim {
        let event = event_iri(item, "claim", now)?;
        let mut triples = event_triples(&event, v::CLAIM_EVENT_CLASS, item, who, now);
        triples.push_str(&format!(
            "\n<{event}> <{}> {} ; <{}> <{}> .",
            v::CLAIMED_BY,
            literal(holder),
            v::CLAIM_KIND,
            kind.iri()
        ));
        if let Some(principal) = principal {
            triples.push_str(&format!(
                "\n<{event}> <{}> {principal} .",
                v::CLAIM_PRINCIPAL
            ));
        }
        if let Some(from) = taken_from {
            triples.push_str(&format!(
                "\n<{event}> <{}> {} .",
                v::TAKEOVER_FROM,
                literal(from)
            ));
        }
        let unclaimed = if takeover {
            String::new()
        } else {
            format!(
                "FILTER NOT EXISTS {{ <{item}> <{}> ?anyHolder }}",
                v::CLAIMED_BY
            )
        };
        ops.push(format!(
            "INSERT {{ {} }} WHERE {{ {} }}",
            client.in_graph(&triples),
            client.in_graph(&format!("{} {open}\n{unclaimed}", live(item)))
        ));
    }
    ops.push(format!(
        "DELETE {{ {} }}\nINSERT {{ {} }}\nWHERE {{ {} }}",
        client.in_graph(&format!("{delete}{epoch_delete}")),
        client.in_graph(&format!("{insert}{epoch_insert}")),
        client.in_graph(&format!(
            "<{item}> <{}> <{}> .\n{open}\n{optional}{epoch_where}",
            v::ext::TYPE,
            v::ITEM_CLASS,
        )),
    ));
    Ok(batch(&ops))
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
        // The epoch check is a read (ledger #3205): anyone who may read the ledger may ask,
        // so a dispatcher or a reviewer learns whether a holder's authority stands without
        // the holder.
        if inv.request.verb == Verb::Source {
            let client = StoreClient::new(inv, ledger_for(inv, Need::Read)?);
            return epoch_check(&client, inv).await;
        }
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
                let epoch = parse_epoch(inv)?;
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
                // A fenced claim (ledger #3205) names the epoch it renews or takes over.
                check_fence(&item, epoch)?;
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
                        &fence_clause(&item.iri, epoch),
                        &holder,
                        caller_term.as_deref(),
                        purpose,
                        kind,
                        lease.as_ref(),
                        &Attribution::committer(inv)?,
                        now,
                    )?)
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
                // A fenced renewal keeps its epoch, so any other epoch now is a claim that
                // changed between the read and the write, and the update's fence refused it.
                if let Some(epoch) = epoch {
                    if !takeover && after.claim_epoch != Some(epoch) {
                        return Err(fence_refusal(&after, epoch));
                    }
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
                // ★ The epoch the holder fences its writes with (ledger #3205), as the update
                // that took the claim set it.
                let at = after
                    .claim_epoch
                    .map(|epoch| format!(" at epoch {epoch}"))
                    .unwrap_or_default();
                let until = lease
                    .as_ref()
                    .map(|l| format!(" until {}", sparql::iso8601(now + l.millis)))
                    .unwrap_or_default();
                Ok(plain(if takeover {
                    format!(
                        "{} taken over from {} by {holder}{at}{until}\n",
                        item.short(),
                        previous.unwrap_or_default()
                    )
                } else if renew {
                    format!("{} renewed by {holder}{at}{until}\n", item.short())
                } else {
                    format!("{} claimed by {holder}{at}{until}\n", item.short())
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
                let epoch = parse_epoch(inv)?;
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
                // A fenced release (ledger #3205) releases only the claim at that epoch.
                check_fence(&item, epoch)?;
                guard.push_str(&fence_clause(&item.iri, epoch));
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
                // The release event first, under the same guard, while the claim is there to
                // read (ledger #2819).
                let event = event_iri(&item.iri, "release", now)?;
                client
                    .update(&batch(&[
                        release_event_at(
                            &client,
                            &event,
                            &item.iri,
                            ReleaseReason::Released,
                            &guard,
                            &Attribution::committer(inv)?,
                            now,
                        ),
                        release_update(&client, &item.iri, &guard, now),
                    ]))
                    .await?;
                // A fenced release that found its claim gone wrote no event: the claim changed
                // between the read and the write, and the fence refused it.
                if let Some(epoch) = epoch {
                    let released = client
                        .ask(&format!(
                            "ASK {{ {} }}",
                            client.in_graph(&format!("<{event}> <{}> ?any", v::ext::TYPE))
                        ))
                        .await?;
                    if !released {
                        return Err(fence_refusal_now(&client, &item, epoch).await);
                    }
                }
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
                        write_comment(
                            &client,
                            &item.iri,
                            note.trim(),
                            &Attribution::committer(inv)?,
                            now,
                        )
                        .await?;
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
                         same principal. Resending is not declared safe: the epoch and claim \
                         event stay the same, but the lease is renewed from the new request time.",
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
                    .input(epoch_arg(
                        "a renewal extends, and a takeover takes, the claim",
                    ))
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
                    .input(epoch_arg("the release releases the claim"))
                    .output(PLAIN),
                CAP_ANY,
            ))
            .action(read_action(
                ikigai_core::ActionSpec::new(Verb::Source)
                    .input(ledger_arg("claim"))
                    .summary(
                        "Whether a claim epoch is still the current claim's on an item, for \
                         anyone who may read the ledger, not only its holder (ledger #3205): \
                         `current` (that claim holds the item), `superseded` (a later claim \
                         does, named with its epoch) or `unclaimed` (nobody does). Every grant \
                         and takeover gives a claim one more than the item's previous epoch, \
                         a renewal keeps it, and it never goes down. An epoch above any the \
                         item has granted is refused (InvalidArgument).",
                    )
                    .input(item_arg("The item to check."))
                    .input(
                        ArgSpec::new("epoch")
                            .summary(
                                "The epoch to check: a whole number from 1, as a claim's \
                                 answer gives it.",
                            )
                            .class(v::ext::XSD_INTEGER),
                    )
                    .input(as_arg(&WRITE_FACES))
                    .output(PLAIN)
                    .output(JSON),
            ))
    }
}

/// `source urn:iki:ledger:{ledger}:claim item=… epoch=…` (ledger #3205): whether `epoch` is
/// the current claim's on the item — `current`, `superseded` by a later claim that holds it
/// now, or `unclaimed`, nobody holding it — in one sentence, or as JSON. An epoch above any the
/// item has granted was never anyone's, and is refused rather than answered.
async fn epoch_check(client: &StoreClient<'_, '_>, inv: &Invocation<'_>) -> Result<Representation> {
    let want = wanted_face(inv, &WRITE_FACES)?;
    let epoch = parse_epoch(inv)?.ok_or_else(|| Error::InvalidArgument {
        name: "epoch".to_string(),
        detail: "the check needs the epoch to check: `epoch=`, a whole number from 1".to_string(),
    })?;
    let item = require_item(client, inv.inline_str("item")?).await?;
    let highest = item.claim_epoch.unwrap_or(0);
    if epoch > highest {
        return Err(Error::InvalidArgument {
            name: "epoch".to_string(),
            detail: if highest == 0 {
                format!(
                    "`{epoch}` was never granted on {}: it has had no claim with an epoch",
                    item.short()
                )
            } else {
                format!(
                    "`{epoch}` was never granted on {}: its highest epoch is {highest}",
                    item.short()
                )
            },
        });
    }
    let state = match &item.claimed_by {
        None => "unclaimed",
        Some(_) if item.claim_epoch == Some(epoch) => "current",
        Some(_) => "superseded",
    };
    if want == JSON {
        let ledger = client.ledger();
        let answer = json::EpochCheck {
            schema: json::SCHEMA,
            ledger: ledger.name().to_string(),
            item: json::item_ref(ledger, item.number, &item.iri),
            epoch,
            state: state.to_string(),
            holder: item.claimed_by.clone(),
            principal: item.claim_principal.clone(),
            current: item.claimed_by.as_ref().and(item.claim_epoch),
            highest,
        };
        return Ok(json_repr(json::render(&answer)?));
    }
    let short = item.short();
    Ok(plain(match state {
        "current" => format!(
            "{short} epoch {epoch} is current: held by {}\n",
            holder_display(&item)
        ),
        "superseded" => format!(
            "{short} epoch {epoch} is superseded: held by {} at epoch {highest}\n",
            holder_display(&item)
        ),
        _ => format!(
            "{short} epoch {epoch} is not current: nobody holds {short}, and its last claim was \
             epoch {highest}\n"
        ),
    }))
}

// --------------------------------------------------------------------------- defer

#[derive(Clone)]
struct DeferEndpoint {
    participants: Participants,
}

#[async_trait]
impl Endpoint for DeferEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let client = StoreClient::new(inv, ledger_for(inv, Need::Write)?);
        let now = now_ms(inv)?;
        let item = require_item(&client, inv.inline_str("item")?).await?;
        let (update, said, edit) = match inv.request.verb {
            Verb::Sink => (
                replace_one(&client, &item.iri, v::DEFERRED, &boolean(true)),
                "deferred",
                "defer",
            ),
            Verb::Delete => (
                retract(&client, &item.iri, v::DEFERRED),
                "resumed",
                "resume",
            ),
            other => return Err(unsupported("ledger-defer", other)),
        };
        // A deferral or a resume an import principal dated at its source (ledger #2939).
        let (claimed, who) = importer_dated(
            inv,
            &client,
            self.participants.as_deref(),
            if edit == "defer" {
                "a deferral"
            } else {
                "a resume"
            },
            &item,
            now,
        )
        .await?;
        guarded_update(
            &client,
            &[&item.iri],
            &batch(&[
                update,
                touch_dated(&client, &item.iri, now, claimed, ""),
                edit_event(&client, &item.iri, edit, &who, now)?,
            ]),
        )
        .await?;
        if let Ok(note) = inv.inline_str("content") {
            if !note.trim().is_empty() {
                write_comment(&client, &item.iri, note.trim(), &who, now).await?;
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
                    .input(created_arg("a deferral"))
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
                    .input(created_arg("a resume"))
                    .output(PLAIN),
                CAP_WRITE,
            ))
    }
}

// ---------------------------------------------------------------------------- link

/// What a link or an unlink's edit event names: the link's type and the other item, by its IRI.
fn link_objects(predicate: &str, other: &str, changes: &str) -> [EditObject; 2] {
    [
        EditObject {
            predicate: v::EDIT_LINK_TYPE,
            value: format!("<{predicate}>"),
            changes: changes.to_string(),
        },
        EditObject {
            predicate: v::EDIT_TARGET,
            value: format!("<{other}>"),
            changes: changes.to_string(),
        },
    ]
}

#[derive(Clone)]
struct LinkEndpoint {
    participants: Participants,
}

#[async_trait]
impl Endpoint for LinkEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let client = StoreClient::new(inv, ledger_for(inv, Need::Write)?);
        let want = wanted_face(inv, &WRITE_FACES)?;
        let now = now_ms(inv)?;
        let from = require_item(&client, inv.inline_str("item")?).await?;
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
        // ★ An import principal may date a link at its source (ledger #2934), and an unlink
        // (ledger #2939), as it may date a comment or a close; anyone else is refused, with the
        // reason and the remedy. Until 0.18.0 an unlink ignored a `created=` from anyone.
        let claimed = attribution::resolve_claimed(
            inv,
            &client,
            self.participants.as_deref(),
            "created",
            if inv.request.verb == Verb::Sink {
                "a link"
            } else {
                "an unlink"
            },
            from.created,
            now,
        )
        .await?;
        // ★ An object in another ledger is a typed link across ledgers (ledger #2829): admitted
        // by `link-in` on that ledger, written as a link node here and mirrored there.
        let reference = inv.inline_str("content")?;
        if let Some(there) = across::other_ledger(client.ledger(), reference) {
            if !matches!(inv.request.verb, Verb::Sink | Verb::Delete) {
                return Err(unsupported("ledger-link", inv.request.verb));
            }
            return across::link(
                inv, &client, &from, reference, there, name, predicate, want, claimed, now,
            )
            .await;
        }
        let to = require_item(&client, reference).await?;
        // A dated link is no earlier than its target's filing either (ledger #2939).
        if let (Some(claimed), Verb::Sink) = (claimed, inv.request.verb) {
            attribution::claimed_not_before_target("created", claimed, &to.iri, to.created)?;
        }
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
        let (update, said, edit) = match inv.request.verb {
            // Both ends live: an edge to a deleted item names something nothing resolves.
            Verb::Sink => (
                insert_on_live(&client, &[&from.iri, &to.iri], &triple),
                "linked",
                "link",
            ),
            Verb::Delete => (
                format!("DELETE DATA {{ {} }}", client.in_graph(&triple)),
                "unlinked",
                "unlink",
            ),
            other => return Err(unsupported("ledger-link", other)),
        };
        // An unlink checks only its source: removing an edge to a deleted item is a cleanup.
        let ends: &[&str] = if inv.request.verb == Verb::Sink {
            &[&from.iri, &to.iri]
        } else {
            &[&from.iri]
        };
        let changes = if inv.request.verb == Verb::Sink {
            absent(&triple)
        } else {
            triple.clone()
        };
        let who = Attribution {
            created: claimed,
            ..Attribution::committer(inv)?
        };
        let event = edit_event_naming(
            &client,
            &from.iri,
            ends,
            edit,
            &who,
            now,
            &link_objects(predicate, &to.iri, &changes),
        )?;
        guarded_update(
            &client,
            ends,
            // A link dated at the source moves `modified` only forward (ledger #2928, #2934).
            &batch(&[
                event,
                update,
                touch_dated(&client, &from.iri, now, claimed, ""),
            ]),
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
                    .input(
                        ArgSpec::new("created")
                            .summary(
                                "When the link was made at its source, for an import principal (ledger:Importer in the host's participants graph) only: an xsd:dateTime no later than now and no earlier than the subject's filing. Dates the link's edit event (and a link across ledgers, its link node), marked createdSource import, and moves the subject's modified only forward to it (ledger #2934). Anyone else sending it is refused.",
                            )
                            .class(v::ext::XSD_DATETIME)
                            .optional(),
                    )
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
                    .input(created_arg("an unlink"))
                    .input(as_arg(&WRITE_FACES))
                    .output(PLAIN)
                    .output(JSON),
                CAP_WRITE,
            ))
    }
}

// --------------------------------------------------------------------------- label

#[derive(Clone)]
struct LabelEndpoint {
    participants: Participants,
}

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
        // ⚠ Only the label grammar, `[A-Za-z0-9._:-]+`, the one gonk's dispatcher checks
        // (ledger #2634). A label with a space was stored whole, printed like several, and
        // broke every command built from the labels. Refused rather than split: this endpoint
        // takes one label, and splitting would add labels the caller did not name. Removing
        // one is still allowed, as with the comma above.
        if inv.request.verb == Verb::Sink {
            check_label("content", label)?;
        }
        let triple = format!("<{}> <{}> {} .", item.iri, v::LABEL, literal(label));
        let (update, said, edit) = match inv.request.verb {
            Verb::Sink => (
                insert_on_live(&client, &[&item.iri], &triple),
                "labeled",
                "label",
            ),
            Verb::Delete => (
                format!("DELETE DATA {{ {} }}", client.in_graph(&triple)),
                "unlabeled",
                "unlabel",
            ),
            other => return Err(unsupported("ledger-label", other)),
        };
        let object = EditObject {
            predicate: v::EDIT_LABEL,
            value: literal(label),
            changes: if inv.request.verb == Verb::Sink {
                absent(&triple)
            } else {
                triple.clone()
            },
        };
        // A label an import principal dated at its source (ledger #2939).
        let (claimed, who) = importer_dated(
            inv,
            &client,
            self.participants.as_deref(),
            if edit == "label" {
                "a label"
            } else {
                "an unlabel"
            },
            &item,
            now,
        )
        .await?;
        let event =
            edit_event_naming(&client, &item.iri, &[&item.iri], edit, &who, now, &[object])?;
        guarded_update(
            &client,
            &[&item.iri],
            &batch(&[
                event,
                update,
                touch_dated(&client, &item.iri, now, claimed, ""),
            ]),
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
                                "The label — where a pipe's value lands. Only A-Z a-z 0-9 \
                                 and `.` `_` `:` `-`: no spaces, and no commas, which the \
                                 `labels`/`without` filters split on.",
                            )
                            .class(v::ext::XSD_STRING),
                    )
                    .input(created_arg("a label"))
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
                    .input(created_arg("an unlabel"))
                    .output(PLAIN),
                CAP_WRITE,
            ))
    }
}

/// Refuse a label being WRITTEN outside the label grammar, `[A-Za-z0-9._:-]+`, naming the
/// first bad character (ledger #2634). Every place that adds a label calls this: the label
/// endpoint and `append labels=`. Removing one does not, so a legacy label can be taken off.
fn check_label(argument: &str, label: &str) -> Result<()> {
    match label.chars().find(|c| !is_label_char(*c)) {
        None => Ok(()),
        Some(bad) => Err(Error::InvalidArgument {
            name: argument.to_string(),
            detail: format!(
                "`{label}` contains {}, and a label may hold only the letters A-Z a-z \
                 0-9 and `.` `_` `:` `-`. Add each word as its own label",
                describe_char(bad)
            ),
        }),
    }
}

/// A character of the label grammar, `[A-Za-z0-9._:-]` (ledger #2634).
fn is_label_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-')
}

/// A character named in words, so a refusal reads the same aloud and a space is not invisible.
fn describe_char(c: char) -> String {
    match c {
        ' ' => "a space".to_string(),
        '\t' => "a tab".to_string(),
        '\n' | '\r' => "a line break".to_string(),
        c if c.is_whitespace() || c.is_control() => {
            format!("the character U+{:04X}", u32::from(c))
        }
        c => format!("`{c}`"),
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
struct AboutEndpoint {
    participants: Participants,
}

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
        let mut objects = Vec::new();
        for target in &targets {
            let target = sparql::iri(target, "content")?;
            let triple = format!("<{}> <{}> {target} .", item.iri, v::ABOUT);
            objects.push(EditObject {
                predicate: v::EDIT_TARGET,
                value: target,
                changes: if inv.request.verb == Verb::Sink {
                    absent(&triple)
                } else {
                    triple.clone()
                },
            });
            updates.push(match inv.request.verb {
                Verb::Sink => insert_on_live(&client, &[&item.iri], &triple),
                _ => format!("DELETE DATA {{ {} }}", client.in_graph(&triple)),
            });
        }
        // An about an import principal dated at its source (ledger #2939).
        let (claimed, who) = importer_dated(
            inv,
            &client,
            self.participants.as_deref(),
            if inv.request.verb == Verb::Sink {
                "an about"
            } else {
                "an unabout"
            },
            &item,
            now,
        )
        .await?;
        updates.push(touch_dated(&client, &item.iri, now, claimed, ""));
        updates.insert(
            0,
            edit_event_naming(
                &client,
                &item.iri,
                &[&item.iri],
                if inv.request.verb == Verb::Sink {
                    "about"
                } else {
                    "unabout"
                },
                &who,
                now,
                &objects,
            )?,
        );
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
                    .input(created_arg("an about"))
                    .output(PLAIN),
                CAP_ANY,
            ))
            .action(write_scopes(
                ikigai_core::ActionSpec::new(Verb::Delete)
                    .input(ledger_arg("item:{id}:about"))
                    .summary("Remove the targets. Removing one not there is not an error.")
                    .input(flight::item_id_input())
                    .input(content("The targets to remove."))
                    .input(created_arg("an unabout"))
                    .output(PLAIN),
                CAP_ANY,
            ))
    }
}

// --------------------------------------------------------------------------- purge

#[derive(Clone)]
struct PurgeEndpoint {
    participants: Participants,
}

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
        let who =
            attribution::resolve(inv, &client, self.participants.as_deref(), Dates::None, now)
                .await?;
        let author = &who;
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
                            .summary(
                                "Who purged it. Optional: the committer, the principal this capability carries, is the author by default. Otherwise a principal's IRI the committer acts for in the host's participants graph, or, for an import principal, any actor; recorded as claimed (ledger #2827).",
                            )
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
    lifecycles: Arc<Lifecycles>,
}

/// An inline argument, or `None` when it is absent.
///
/// ★ A value that cannot be read is an error here, never `None`: `drain=` and `state=` narrow what
/// a step is offered, and reading a bad one as absent would offer it the whole ledger.
fn present_str<'a>(inv: &'a Invocation<'_>, name: &str) -> Result<Option<&'a str>> {
    match inv.inline_str(name) {
        Ok(value) => Ok(Some(value)),
        Err(Error::MissingArgument(_)) => Ok(None),
        Err(unreadable) => Err(unreadable),
    }
}

/// A drain name a caller wrote, checked against the ledger's lifecycle, in the words
/// [`flight::named_state`] uses for a state.
fn named_drain<'a>(lifecycle: &crate::lifecycle::Lifecycle, name: &'a str) -> Result<&'a str> {
    let drains = lifecycle.drains();
    if drains.contains(&name) {
        return Ok(name);
    }
    Err(Error::InvalidArgument {
        name: "drain".to_string(),
        detail: format!(
            "`{name}` is not a drain of the lifecycle `{}` this ledger uses; one of {} (see `{}`)",
            lifecycle.name(),
            drains.join(", "),
            lifecycle.iri()
        ),
    })
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
        // Refused from the arguments, before anything is read.
        let lifecycle = self.lifecycles.for_ledger(client.ledger());
        let drain = present_str(inv, "drain")?
            .map(|name| named_drain(lifecycle, name))
            .transpose()?;
        let state = present_str(inv, "state")?
            .map(|name| flight::named_state(lifecycle, "state", name))
            .transpose()?;
        let filter = Filter {
            status: Status::Open,
            kind: inv.inline_str("kind").ok().map(str::to_string),
            labels: split_labels(inv.inline_str("labels").ok()),
            without: split_labels(inv.inline_str("without").ok()),
            about: inv.inline_str("about").ok().map(str::to_string),
            ..Filter::default()
        };
        let limit = parse_limit(inv, 3)?;
        // ★ Slim records unless the face shows every excluded item whole (ledger #3257).
        let mut set = if want == JSON {
            select::ready_full_at(&client, &filter, now).await?
        } else {
            select::ready_at(&client, &filter, now).await?
        };
        // ★ After the ready set, like `labels=`: blocked, the cycle check and leverage have been
        // computed over the whole ledger, so these only choose which items are being asked about.
        // Before the horizons below, because an item this leaves out cannot change this answer.
        if drain.is_some() || state.is_some() {
            set = set.in_states(lifecycle, |held| {
                drain.is_none_or(|drain| held.drain.as_deref() == Some(drain))
                    && state.is_none_or(|state| held.name == state.name)
            });
        }
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
        let mut selection = select::rank(client.ledger(), set, policy.as_ref(), now, limit);
        if want != JSON {
            select::complete(&client, &mut selection).await?;
        }
        let repr = match want {
            TURTLE => Representation::new(
                ReprType::new(TURTLE).with_param("charset", "utf-8"),
                selection.turtle()?,
            ),
            JSON => {
                // ★ A row is the item object `item:{id}` and `items` show (ledger #2931): its
                // links into other ledgers and the mirrors of theirs, for the whole selection in
                // two queries. Until 0.18.0 `next` rendered its items without them.
                across::decorate_selection(inv, &client, &mut selection).await?;
                json_repr(json::render(&json::next(&client, &selection).await?)?)
            }
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
                    ArgSpec::new("drain")
                        .summary(
                            "Only candidates whose current state drains to this step, `filed` \
                             included when its drain matches. A drain of the ledger's lifecycle \
                             (`urn:iki:ledger:lifecycle:{name}`), which is what a value is \
                             judged against. Blocked items, cycles and leverage are still \
                             computed over the whole ledger.",
                        )
                        .class(v::ext::XSD_STRING)
                        .one_of(self.lifecycles.drain_names())
                        .optional(),
                )
                .input(
                    ArgSpec::new("state")
                        .summary(
                            "Only candidates in this state, `filed` being an item that holds \
                             none. A state of the ledger's lifecycle \
                             (`urn:iki:ledger:lifecycle:{name}`), which is what a value is \
                             judged against. Blocked items, cycles and leverage are still \
                             computed over the whole ledger.",
                        )
                        .class(v::ext::XSD_STRING)
                        .one_of(self.lifecycles.state_names())
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
