//! **The work ledger for ikigai** — the scaffold.
//!
//! Items, comments, labels, links and claims as RDF in a durable store, with
//! view / query / append / comment / close / delete as capability-gated resources under
//! `urn:iki:ledger:*`, and `urn:iki:ledger:next` answering *what should I do next* as a
//! resource rather than as a sort order.
//!
//! This commit is the scaffold only: the manifest, the CI caller and the licences, so the
//! repository's gates run green before any behaviour depends on them (field guide,
//! constitution 9). The module lands in the first PR.

#![deny(missing_docs)]
