//! A **named ledger**: the partition, the graph it lives in, and the capability that
//! gates it.
//!
//! # ★ Why a name and not a tag
//!
//! A label is *data*; a capability binds to a *resource name*. There is nothing for
//! `urn:cap:…` to attach to in "items labelled acme", so enforcement would have to live
//! inside the endpoint — declared-but-not-enforced, which this ecosystem refuses
//! everywhere else. A tag partition is a **view**; only a name can be a **boundary**.
//!
//! A `ledger=` *argument* has the same defect and is not the answer either: an argument
//! is a value, the manifold still offers one action, and a capability still cannot
//! distinguish one ledger from another.
//!
//! So a ledger is a segment of the IRI — `urn:iki:ledger:acme:append` — which makes it a
//! different resource, gated by a different capability, backed by a different named
//! graph.
//!
//! # The sugar is an ALIAS, not a second binding
//!
//! `urn:iki:ledger:append` resolves to the ledger called `default`, and it does so
//! through the *same* `LedgerGrammar` match that `urn:iki:ledger:default:append` takes
//! — one grammar, one binding, one entry in the manifold, **one capability**
//! (`urn:cap:ledger:write:default`). That is the whole point of building it this way: a
//! short form implemented as a separate binding would carry a separate capability, and a
//! short form that quietly widens authority is exactly the hole named ledgers exist to
//! close.
//!
//! The canonical form is the long one. An item filed through the short form is minted at
//! `urn:iki:ledger:default:item:{id}`, because the *data* must say which ledger it is in
//! even when the *request* did not.
//!
//! # The graphs
//!
//! | thing | IRI |
//! | --- | --- |
//! | the ledger's graph | `urn:iki:ledger:graph:{name}` |
//! | its graveyard (recoverable deletes) | `urn:iki:ledger:graph:{name}:deleted` |
//! | its counter | `urn:iki:ledger:{name}:counter` |
//! | an item | `urn:iki:ledger:{name}:item:{id}` |
//!
//! One store, one write lock, one process, many ledgers.

use ikigai_core::{Bindings, Error, Grammar, Iri, Result};

/// The IRI prefix every ledger resource and every ledger subject sits under.
pub const PREFIX: &str = "urn:iki:ledger:";

/// The ledger the bare `urn:iki:ledger:*` forms name.
pub const DEFAULT: &str = "default";

/// Names a ledger may not take, because the bare-form sugar spends them on resources
/// and on subject prefixes.
///
/// ★ This list is the cost of the sugar, stated where it is paid. `urn:iki:ledger:items`
/// has to mean *the default ledger's listing* rather than *a ledger called `items`*, and
/// only one of those can be true. Reserving is the cheaper half: a ledger can be called
/// almost anything, and the refusal says which word it must not use.
pub const RESERVED: [&str; 18] = [
    "append",
    "claim",
    "close",
    "comment",
    "counter",
    "defer",
    "graph",
    "item",
    "items",
    "label",
    "ledgers",
    "link",
    "next",
    "policy",
    "purge",
    "reopen",
    "selection",
    "tombstone",
];

/// The longest a ledger name may be. Not a storage limit — a legibility one: the name
/// appears in every display number (`acme#12`) and in every capability token.
pub const MAX_NAME: usize = 64;

/// One named ledger.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Ledger {
    name: String,
}

impl Default for Ledger {
    fn default() -> Self {
        Ledger {
            name: DEFAULT.to_string(),
        }
    }
}

impl Ledger {
    /// Validate a ledger name and make a [`Ledger`] of it.
    ///
    /// Lowercase ASCII letters, digits, `-` and `_`, starting and ending with a letter or
    /// a digit, at most [`MAX_NAME`] characters, and not one of [`RESERVED`].
    ///
    /// ⚠ The restriction is not decoration. The name goes into an IRI segment, a
    /// capability token and a display number, and **only a fixed-shape name keeps the
    /// capability token unambiguous**: `urn:cap:ledger:read:{name}` is matched exactly, so
    /// a name containing a `:` could spell a token that was meant for something else.
    ///
    /// ```
    /// use ikigai_ledger::Ledger;
    /// assert_eq!(Ledger::parse("acme").unwrap().name(), "acme");
    /// assert!(Ledger::parse("Acme").is_err());     // case is not a distinction to rely on
    /// assert!(Ledger::parse("a:b").is_err());      // would forge a capability token
    /// assert!(Ledger::parse("items").is_err());    // reserved by the bare-form sugar
    /// ```
    pub fn parse(name: &str) -> Result<Ledger> {
        let bad = |detail: String| Error::InvalidArgument {
            name: "ledger".to_string(),
            detail,
        };
        if name.is_empty() {
            return Err(bad(
                "a ledger name is empty; the bare `urn:iki:ledger:…` forms already mean the \
                 ledger called `default`"
                    .to_string(),
            ));
        }
        if name.len() > MAX_NAME {
            return Err(bad(format!(
                "`{name}` is {} characters; a ledger name is at most {MAX_NAME}, because it \
                 appears in every display number and every capability token",
                name.len()
            )));
        }
        let ok = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
        let shaped = name.chars().all(|c| ok(c) || c == '-' || c == '_')
            && name.starts_with(ok)
            && name.ends_with(ok);
        if !shaped {
            return Err(bad(format!(
                "`{name}` is not a ledger name: lowercase letters, digits, `-` and `_`, \
                 starting and ending with a letter or a digit. The shape is fixed because \
                 the name becomes an IRI segment AND a capability token \
                 (`urn:cap:ledger:read:{name}`), and a token matched exactly must not be \
                 forgeable by spelling"
            )));
        }
        if RESERVED.contains(&name) {
            return Err(bad(format!(
                "`{name}` is reserved: `urn:iki:ledger:{name}` already names a resource of \
                 the default ledger, so a ledger by that name could not be addressed. \
                 Reserved: {}",
                RESERVED.join(", ")
            )));
        }
        Ok(Ledger {
            name: name.to_string(),
        })
    }

    /// The ledger a request named, from a grammar's `ledger` binding — or `default` when
    /// the bare form was used and there is no binding at all.
    pub fn from_bindings(bindings: &Bindings) -> Result<Ledger> {
        match bindings.get("ledger") {
            Some(name) => Ledger::parse(name),
            None => Ok(Ledger::default()),
        }
    }

    /// The name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether this is the ledger the bare forms address.
    pub fn is_default(&self) -> bool {
        self.name == DEFAULT
    }

    /// `urn:iki:ledger:{name}:` — the prefix every resource and subject of this ledger
    /// carries.
    pub fn prefix(&self) -> String {
        format!("{PREFIX}{}:", self.name)
    }

    /// The named graph this ledger's quads live in.
    ///
    /// ★ Not the default graph, deliberately: the store holds whatever the host put in
    /// it, and a ledger writing into the default graph would make every
    /// `SELECT * WHERE { ?s ?p ?o }` in the host a ledger query too. One graph per
    /// ledger is also what makes a per-graph write scope able to fence it.
    pub fn graph(&self) -> String {
        format!("{PREFIX}graph:{}", self.name)
    }

    /// Where a recoverable delete puts this ledger's quads: out of every ledger query,
    /// still in the store, still recoverable by hand.
    ///
    /// ⚠ **Per ledger, not shared.** A single graveyard would mean a delete in one
    /// ledger wrote into a graph another ledger's deletes also write — one graph, one
    /// write scope, and therefore a path across the boundary that the rest of this
    /// module is built to keep closed.
    pub fn deleted_graph(&self) -> String {
        format!("{PREFIX}graph:{}:deleted", self.name)
    }

    /// The short-id allocator, as a resource in the graph rather than as process state —
    /// which is why a restart continues the numbering.
    pub fn counter(&self) -> String {
        format!("{}counter", self.prefix())
    }

    /// `urn:iki:ledger:{name}:item:{id}`.
    pub fn item(&self, id: &str) -> String {
        format!("{}item:{id}", self.prefix())
    }

    /// One of this ledger's resources, in the spelling an operator would actually type:
    /// the bare form for `default`, the named form for everything else.
    ///
    /// Error messages use this rather than the canonical form. A hint that tells a solo
    /// operator to type `urn:iki:ledger:default:link` when `urn:iki:ledger:link` is what
    /// they have been typing all day is a hint that reads as a change of subject.
    ///
    /// ```
    /// use ikigai_ledger::Ledger;
    /// assert_eq!(Ledger::default().resource("link"), "urn:iki:ledger:link");
    /// assert_eq!(Ledger::parse("acme")?.resource("link"), "urn:iki:ledger:acme:link");
    /// # Ok::<_, ikigai_core::Error>(())
    /// ```
    pub fn resource(&self, action: &str) -> String {
        if self.is_default() {
            format!("{PREFIX}{action}")
        } else {
            format!("{}{action}", self.prefix())
        }
    }

    /// `urn:iki:ledger:{name}:comment:{id}`.
    pub fn comment(&self, id: &str) -> String {
        format!("{}comment:{id}", self.prefix())
    }

    /// `urn:iki:ledger:{name}:tombstone:{id}` — the deleted item's own id, so the two
    /// join.
    pub fn tombstone(&self, id: &str) -> String {
        format!("{}tombstone:{id}", self.prefix())
    }

    /// `urn:iki:ledger:{name}:selection:{stamp}`.
    pub fn selection(&self, stamp: u64) -> String {
        format!("{}selection:{stamp}", self.prefix())
    }

    /// The display form of an item number: `#12` in the default ledger, `acme#12`
    /// elsewhere.
    ///
    /// Numbers are per ledger — there is a counter in every graph — so a bare `#12` is
    /// ambiguous the moment a second ledger exists. The default ledger keeps the bare
    /// form for the same reason it keeps the bare IRIs: it is the one a solo operator
    /// types all day.
    pub fn number(&self, number: i64) -> String {
        if self.is_default() {
            format!("#{number}")
        } else {
            format!("{}#{number}", self.name)
        }
    }

    /// Which ledger an item (or comment, or tombstone) IRI belongs to, if any.
    ///
    /// Used to turn "no such item" into "that item is in another ledger", which is a
    /// different fact and the only one the reader can act on.
    pub fn of_subject(iri: &str) -> Option<Ledger> {
        let rest = iri.strip_prefix(PREFIX)?;
        let (name, _) = rest.split_once(':')?;
        Ledger::parse(name).ok()
    }

    /// An item IRI in **either** spelling, as the ledger it is in and its canonical form.
    ///
    /// ★ The sugar has to reach this far or it is a trap. `urn:iki:ledger:item:{id}`
    /// *resolves* as a resource — the grammar accepts it — so a caller who has that IRI
    /// will also hand it to `item=`, and a reference that resolves in one position and
    /// not the other is worse than no sugar at all. Both spellings come back as the
    /// canonical `urn:iki:ledger:default:item:{id}`, which is what the store holds.
    ///
    /// `None` for anything that is not a ledger item IRI — an arbitrary `urn:` a caller
    /// passed by mistake is left alone so the error can name it verbatim.
    ///
    /// ```
    /// use ikigai_ledger::Ledger;
    /// let (ledger, iri) = Ledger::item_iri("urn:iki:ledger:item:01abc").unwrap();
    /// assert!(ledger.is_default());
    /// assert_eq!(iri, "urn:iki:ledger:default:item:01abc");
    /// assert_eq!(
    ///     Ledger::item_iri("urn:iki:ledger:acme:item:01abc").unwrap().1,
    ///     "urn:iki:ledger:acme:item:01abc"
    /// );
    /// assert!(Ledger::item_iri("urn:repo:file:x").is_none());
    /// ```
    pub fn item_iri(reference: &str) -> Option<(Ledger, String)> {
        let rest = reference.strip_prefix(PREFIX)?;
        if let Some(id) = rest.strip_prefix("item:") {
            let ledger = Ledger::default();
            let iri = ledger.item(id);
            return Some((ledger, iri));
        }
        let (name, tail) = rest.split_once(':')?;
        let id = tail.strip_prefix("item:")?;
        if id.is_empty() {
            return None;
        }
        let ledger = Ledger::parse(name).ok()?;
        let iri = ledger.item(id);
        Some((ledger, iri))
    }

    /// `urn:cap:ledger:read:{name}` — reading anything in this ledger through its own
    /// resources.
    pub fn cap_read(&self) -> String {
        format!("{CAP_READ_PREFIX}{}", self.name)
    }

    /// `urn:cap:ledger:write:{name}` — filing, commenting, closing, claiming, linking,
    /// labelling, deferring, editing.
    pub fn cap_write(&self) -> String {
        format!("{CAP_WRITE_PREFIX}{}", self.name)
    }

    /// `urn:cap:ledger:delete:{name}` — moving an item out of view, recoverably.
    pub fn cap_delete(&self) -> String {
        format!("{CAP_DELETE_PREFIX}{}", self.name)
    }

    /// `urn:cap:ledger:purge:{name}` — destroying an item's content.
    pub fn cap_purge(&self) -> String {
        format!("{CAP_PURGE_PREFIX}{}", self.name)
    }
}

/// The prefix a held read grant carries; the declared form is `…:read:*`.
pub(crate) const CAP_READ_PREFIX: &str = "urn:cap:ledger:read:";
pub(crate) const CAP_WRITE_PREFIX: &str = "urn:cap:ledger:write:";
pub(crate) const CAP_DELETE_PREFIX: &str = "urn:cap:ledger:delete:";
pub(crate) const CAP_PURGE_PREFIX: &str = "urn:cap:ledger:purge:";

/// A grammar matching **both** `urn:iki:ledger:{ledger}:{action}` and the bare
/// `urn:iki:ledger:{action}`, capturing `ledger` either way.
///
/// ★ **One grammar rather than two bindings, and the reason is not tidiness.** Two
/// bindings would be two entries in the space: two rows in the catalog for one resource,
/// two capabilities to keep in step, and — since a conformance walk probes per *entry*,
/// not per endpoint — every destructive action fired twice by the suite that is supposed
/// to be checking it. Matching both spellings in one grammar makes the short form what it
/// claims to be: a spelling, not a second door.
pub(crate) struct LedgerGrammar {
    action: &'static str,
    /// Whether a trailing `:{id}` segment is captured (`…:item:{id}`).
    id: bool,
}

impl LedgerGrammar {
    /// `urn:iki:ledger:{ledger}:{action}`, plus the bare spelling.
    pub(crate) fn action(action: &'static str) -> Self {
        LedgerGrammar { action, id: false }
    }

    /// `urn:iki:ledger:{ledger}:{action}:{id}`, plus the bare spelling.
    pub(crate) fn with_id(action: &'static str) -> Self {
        LedgerGrammar { action, id: true }
    }

    /// Match the part after the ledger segment, binding `ledger` to `name`.
    ///
    /// ⚠ **Permissive about the name on purpose.** A grammar answers "is this IRI
    /// mine?"; whether `PROBE` or `Acme` is a *legal* ledger name is the endpoint's
    /// question, answered by [`Ledger::parse`] with a sentence. A grammar that validated
    /// here would make `Meta` on the template unresolvable, since the catalog probes it
    /// with a placeholder.
    fn match_tail(&self, tail: &str, name: &str) -> Option<Bindings> {
        let mut bindings = Bindings::new();
        bindings.insert("ledger", name);
        if !self.id {
            return (tail == self.action).then_some(bindings);
        }
        let id = tail.strip_prefix(self.action)?.strip_prefix(':')?;
        if id.is_empty() {
            return None;
        }
        bindings.insert("id", id);
        Some(bindings)
    }
}

impl Grammar for LedgerGrammar {
    fn match_iri(&self, iri: &Iri) -> Option<Bindings> {
        let rest = iri.as_str().strip_prefix(PREFIX)?;
        // The bare form first: its first segment IS the action, so trying it first means
        // `urn:iki:ledger:item:abc` can never be read as a ledger called `item` — which
        // is also why `item` is reserved.
        if let Some(bindings) = self.match_tail(rest, DEFAULT) {
            return Some(bindings);
        }
        let (name, tail) = rest.split_once(':')?;
        if name.is_empty() {
            return None;
        }
        self.match_tail(tail, name)
    }

    fn pattern(&self) -> String {
        if self.id {
            format!("{PREFIX}{{ledger}}:{}:{{id}}", self.action)
        } else {
            format!("{PREFIX}{{ledger}}:{}", self.action)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iri(s: &str) -> Iri {
        Iri::parse(s).expect("a test IRI")
    }

    #[test]
    fn a_name_that_could_forge_a_capability_token_is_refused() {
        // `:` would let a name spell a token meant for something else; the rest are
        // refused so that one ledger has exactly one spelling.
        for hostile in [
            "a:b", "Acme", "acme ", "-acme", "acme-", "", "a/b", "acme#1",
        ] {
            assert!(Ledger::parse(hostile).is_err(), "accepted `{hostile}`");
        }
        for fine in ["acme", "a", "acme-corp", "client_2", "x9"] {
            assert!(Ledger::parse(fine).is_ok(), "refused `{fine}`");
        }
    }

    #[test]
    fn every_reserved_word_is_refused_with_the_list() {
        for reserved in RESERVED {
            let err = Ledger::parse(reserved).unwrap_err().to_string();
            assert!(err.contains("reserved"), "{reserved}: {err}");
        }
    }

    /// ★ The alias property, as a test: the two spellings produce the SAME binding, so
    /// they cannot drift into two capabilities.
    #[test]
    fn the_bare_form_binds_the_default_ledger() {
        let grammar = LedgerGrammar::action("append");
        let bare = grammar.match_iri(&iri("urn:iki:ledger:append")).unwrap();
        let long = grammar
            .match_iri(&iri("urn:iki:ledger:default:append"))
            .unwrap();
        assert_eq!(bare.get("ledger"), Some("default"));
        assert_eq!(bare, long);
    }

    #[test]
    fn a_named_ledger_is_captured_and_a_stranger_is_not() {
        let grammar = LedgerGrammar::action("items");
        assert_eq!(
            grammar
                .match_iri(&iri("urn:iki:ledger:acme:items"))
                .unwrap()
                .get("ledger"),
            Some("acme")
        );
        assert!(grammar
            .match_iri(&iri("urn:iki:ledger:acme:next"))
            .is_none());
        assert!(grammar.match_iri(&iri("urn:iki:store:select")).is_none());
        assert!(grammar.match_iri(&iri("urn:iki:ledger:items:x")).is_none());
    }

    #[test]
    fn an_item_iri_captures_the_ledger_and_the_id_in_both_spellings() {
        let grammar = LedgerGrammar::with_id("item");
        let bare = grammar
            .match_iri(&iri("urn:iki:ledger:item:01abc"))
            .unwrap();
        assert_eq!(bare.get("ledger"), Some("default"));
        assert_eq!(bare.get("id"), Some("01abc"));
        let named = grammar
            .match_iri(&iri("urn:iki:ledger:acme:item:01abc"))
            .unwrap();
        assert_eq!(named.get("ledger"), Some("acme"));
        assert_eq!(named.get("id"), Some("01abc"));
        // No id at all is not this resource; it is the (unbound) ledger itself.
        assert!(grammar
            .match_iri(&iri("urn:iki:ledger:acme:item"))
            .is_none());
    }

    /// ⚠ The catalog probes a template by expanding it with a placeholder and resolving
    /// `Meta` on the result. A grammar that validated the name would make every ledger
    /// resource undescribable.
    #[test]
    fn the_catalogs_placeholder_still_matches() {
        for placeholder in ["probe", "x"] {
            let target = format!("urn:iki:ledger:{placeholder}:append");
            assert!(LedgerGrammar::action("append")
                .match_iri(&iri(&target))
                .is_some());
        }
    }

    #[test]
    fn a_ledgers_graphs_and_tokens_all_carry_its_name() {
        let acme = Ledger::parse("acme").unwrap();
        assert_eq!(acme.graph(), "urn:iki:ledger:graph:acme");
        assert_eq!(acme.deleted_graph(), "urn:iki:ledger:graph:acme:deleted");
        assert_eq!(acme.counter(), "urn:iki:ledger:acme:counter");
        assert_eq!(acme.item("01abc"), "urn:iki:ledger:acme:item:01abc");
        assert_eq!(acme.cap_write(), "urn:cap:ledger:write:acme");
        assert_eq!(acme.number(12), "acme#12");
        // Two ledgers never share a graph, a counter or a token.
        let other = Ledger::parse("other").unwrap();
        assert_ne!(acme.graph(), other.graph());
        assert_ne!(acme.counter(), other.counter());
        assert_ne!(acme.cap_read(), other.cap_read());
    }

    #[test]
    fn the_default_ledger_keeps_the_bare_number() {
        let default = Ledger::default();
        assert!(default.is_default());
        assert_eq!(default.number(12), "#12");
        assert_eq!(default.graph(), "urn:iki:ledger:graph:default");
        assert_eq!(default.item("01abc"), "urn:iki:ledger:default:item:01abc");
    }

    #[test]
    fn a_subjects_ledger_is_readable_from_its_iri() {
        assert_eq!(
            Ledger::of_subject("urn:iki:ledger:acme:item:01abc")
                .unwrap()
                .name(),
            "acme"
        );
        assert!(Ledger::of_subject("urn:iki:ledger:item:01abc").is_none());
        assert!(Ledger::of_subject("urn:repo:file:x").is_none());
    }
}
