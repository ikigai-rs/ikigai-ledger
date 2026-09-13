//! **The work ledger**: items, comments, labels, links and claims as RDF in the durable
//! store, with view / query / append / comment / close / delete as capability-gated
//! resources — and `urn:iki:ledger:next`, which answers *what should I do next* as a
//! resource rather than as a sort order.
//!
//! ```text
//! urn:iki:ledger:items          Source              the list, filtered            read
//! urn:iki:ledger:item:{id}      Source Sink Delete  one item, edit, delete        read/write/delete
//! urn:iki:ledger:append         Sink                file a new item               write
//! urn:iki:ledger:comment        Sink                append a comment              write
//! urn:iki:ledger:close          Sink                close with a reason           write
//! urn:iki:ledger:reopen         Sink                undo a close                  write
//! urn:iki:ledger:claim          Sink Delete         take it / hand it back        write
//! urn:iki:ledger:defer          Sink Delete         not now / now again           write
//! urn:iki:ledger:link           Sink Delete         blocks / parent / related     write
//! urn:iki:ledger:label          Sink Delete         tag / untag                   write
//! urn:iki:ledger:next           Source              the ready set, ranked         read
//! urn:iki:ledger:policy:{name}  Source              what a policy weighs          read
//! ```
//!
//! # Composition: this crate owns no bytes
//!
//! Every read here is a SPARQL query issued at `urn:iki:store:select` or
//! `urn:iki:store:construct`, and every write is a SPARQL UPDATE at
//! `urn:iki:store:update`. [`ikigai-store`] owns the dataset, the RocksDB write lock and
//! the golden threads; this module owns the *domain* — what an item is, what a delete
//! leaves behind, and what "ready" means.
//!
//! That is not a layering preference. `DurableStore`'s handle is `pub(crate)` by design
//! (handing it out forfeits cacheable reads for the life of the store), so an in-process
//! consumer reaches the data the same way a remote one does: through the kernel.
//!
//! **The consequence a host must know: this module is inert unless a store space is
//! bound in the same kernel.** A ledger resource resolved without one fails with the
//! kernel's own "no endpoint" error naming `urn:iki:store:select`, which is legible but
//! is not this module's error. See `README.md`.
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
//! # Capabilities, and the one thing that surprises a first reader
//!
//! `urn:cap:ledger:{read,write,delete,purge}` gate this module's own actions. But a
//! sub-request issued from inside an endpoint carries **the caller's** capability
//! unchanged — `Invocation::issue` has no attenuating or elevating form — so a ledger
//! write is only possible for a caller who ALSO holds `urn:cap:store:write`, which is
//! the keys to the whole store. Every action here therefore declares the store scopes it
//! transitively needs, because an action that enforces a scope it does not declare makes
//! the manifold over-offer. See `README.md`, "What we could not make narrower".
//!
//! # The graphs
//!
//! Everything this module writes is in the named graph [`GRAPH`], so a ledger shares a
//! store with a host's other standing state and neither pollutes the other's queries. A
//! recoverable delete moves an item's quads to [`DELETED_GRAPH`].
//!
//! [`ikigai-store`]: https://crates.io/crates/ikigai-store

#![deny(missing_docs)]

pub mod endpoints;
pub mod model;
pub mod policy;
pub mod select;
pub mod sparql;
pub mod vocabulary;

pub use endpoints::{space, space_with_policies, CAP_DELETE, CAP_PURGE, CAP_READ, CAP_WRITE};
pub use policy::{Candidate, OrderingPolicy, Ranked, SelectionInputs};
pub use select::{ready, Selection};
pub use vocabulary::{DELETED_GRAPH, GRAPH, LEDGER_NS, VOCABULARY_TTL};
