//! **The work ledger**: items, comments, labels, links and claims as RDF in the durable
//! store, with view / query / append / comment / close / delete as capability-gated
//! resources — and `urn:iki:ledger:next`, which answers *what should I do next* as a
//! resource rather than as a sort order.
//!
//! A ledger is **named**, and the name is a segment of the IRI rather than an argument,
//! so a capability can bind to it. `{ledger}` may be omitted, which names the ledger
//! called `default`.
//!
//! ```text
//! urn:iki:ledger:{ledger}:items      Source              the list, filtered         read
//! urn:iki:ledger:{ledger}:item:{id}  Source Sink Delete  one item, edit, delete     read/write/delete
//! urn:iki:ledger:{ledger}:append     Sink                file a new item            write
//! urn:iki:ledger:{ledger}:comment    Sink                append a comment           write
//! urn:iki:ledger:{ledger}:close      Sink                close with a reason        write
//! urn:iki:ledger:{ledger}:reopen     Sink                undo a close               write
//! urn:iki:ledger:{ledger}:claim      Sink Delete         take it / hand it back     write
//! urn:iki:ledger:{ledger}:defer      Sink Delete         not now / now again        write
//! urn:iki:ledger:{ledger}:link       Sink Delete         blocks / parent / related  write
//! urn:iki:ledger:{ledger}:label      Sink Delete         tag / untag                write
//! urn:iki:ledger:{ledger}:purge      Delete              destroy, leaving evidence  purge
//! urn:iki:ledger:{ledger}:next       Source              the ready set, ranked      read
//! urn:iki:ledger:ledgers             Source              which ledgers exist        read
//! urn:iki:ledger:policy:{name}       Source              what a policy weighs       read
//! ```
//!
//! # Composition: this crate owns no bytes
//!
//! Every read here is a SPARQL query issued at `urn:iki:store:graph-select` and every
//! write is a SPARQL UPDATE at `urn:iki:store:graph-update` — the **narrow** doors, each
//! naming one ledger's graph. [`ikigai-store`] (**0.2.2 or later**, where those doors
//! arrive) owns the dataset, the RocksDB write lock and the golden threads; this module
//! owns the *domain* — what an item is, what a delete leaves behind, and what "ready"
//! means.
//!
//! That is not a layering preference. `DurableStore`'s handle is `pub(crate)` by design
//! (handing it out forfeits cacheable reads for the life of the store), so an in-process
//! consumer reaches the data the same way a remote one does: through the kernel.
//!
//! **The consequence a host must know: this module is inert unless a store space is
//! bound in the same kernel.** A ledger resource resolved without one fails with the
//! kernel's own "no endpoint" error naming `urn:iki:store:graph-select`, which is legible
//! but is not this module's error. See `README.md`.
//!
//! ```no_run
//! use ikigai_core::Kernel;
//! use ikigai_store::{DurableStore, StoreConfig};
//! use std::sync::Arc;
//!
//! # fn demo() -> ikigai_core::Result<()> {
//! # #[cfg(feature = "persistent")] {
//! let config = StoreConfig::load(Some("gonk"))?;
//! let store = DurableStore::open(&config.path)?;      // owned: reads stay cacheable
//! let space = ikigai_core::Fallback::new(vec![
//!     Arc::new(ikigai_store::space(store)) as Arc<dyn ikigai_core::Space>,
//!     Arc::new(ikigai_ledger::space()),
//! ]);
//! let kernel = Kernel::new(Arc::new(space));
//! # }
//! # Ok(()) }
//! ```
//!
//! # Capabilities: per ledger, at this module's doors AND at the store's
//!
//! `urn:cap:ledger:{read,write,delete,purge}:{ledger}` gate this module's actions — one
//! grant per ledger, matched exactly. An action declares the family
//! (`urn:cap:ledger:write:*`, "holds some ledger write grant") because the ledger is in
//! the IRI and the kernel's pre-check runs before `invoke` can read it; the exact scope
//! for the ledger actually named is enforced inside. So **an agent's grants are the set
//! of ledgers it may touch**, which is what naming them was for.
//!
//! A sub-request issued from inside an endpoint carries **the caller's** capability
//! unchanged — `Invocation::issue` has no attenuating or elevating form — so whatever this
//! crate asks the store for, the caller must hold. It asks only the narrow doors, so what
//! the caller must hold names one graph:
//!
//! - `urn:cap:store:read:graph:urn:iki:ledger:graph:{ledger}` to read that ledger;
//! - `urn:cap:store:write:graph:urn:iki:ledger:graph:{ledger}` to write it;
//! - ⚠ **and `…:graph:{ledger}:deleted` as well, for delete and purge** — the graveyard is
//!   a second graph, and a scoped write cannot reach across.
//!
//! Every action declares the store families it transitively needs, because an action that
//! enforces a scope it does not declare makes the manifold over-offer. **The result is a
//! tenancy boundary**: a caller holding only the grants above for one ledger cannot reach
//! another's graph by any route this crate offers or composes over.
//!
//! ⚠ What that does *not* claim: a host that hands a ledger caller the store's broad
//! `urn:cap:store:read` anyway has given it the whole dataset. Nothing here asks for that
//! grant, so issuing it is now a configuration decision rather than a requirement — and
//! both facts are pinned as tests. See `README.md`, "What is enforced, and where".
//!
//! # The graphs
//!
//! Each ledger's quads live in its own named graph, [`Ledger::graph`] — so a ledger
//! shares a store with a host's other standing state and with other ledgers, and none of
//! them pollutes another's queries. A recoverable delete moves an item's quads to that
//! ledger's own [`Ledger::deleted_graph`].
//!
//! [`ikigai-store`]: https://crates.io/crates/ikigai-store

#![deny(missing_docs)]

pub mod endpoints;
pub mod ledger;
pub mod model;
pub mod policy;
pub mod select;
pub mod sparql;
pub mod vocabulary;

pub use endpoints::{space, space_with_policies, CAP_DELETE, CAP_PURGE, CAP_READ, CAP_WRITE};
pub use ledger::Ledger;
pub use policy::{Candidate, OrderingPolicy, Ranked, SelectionInputs};
pub use select::{ready, Selection};
pub use vocabulary::{LEDGER_NS, VOCABULARY_TTL};
