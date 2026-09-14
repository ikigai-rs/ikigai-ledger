//! The terms this module writes.
//!
//! The *graphs* they are written in are not here: a ledger's graph, graveyard, counter
//! and subject IRIs all carry its name and are derived from it — see [`crate::ledger`].
//!
//! The vocabulary is embedded from [`vocabulary.ttl`](https://github.com/ikigai-rs/ikigai-ledger/blob/main/src/vocabulary.ttl)
//! so the table and its documentation cannot drift apart — there is only one artifact,
//! and the constants below are checked against it by
//! `every_term_this_module_writes_is_defined_in_the_vocabulary`.
//!
//! Unlike `ikigai-log`, nothing here is *driven* by the graph at runtime: the ledger's
//! terms are a fixed domain model, not an extensible table, so parsing the Turtle on
//! every call would buy nothing. It is embedded for the test, for the `/ns` publication
//! that a second consumer would trigger, and because a vocabulary that lives only in
//! string constants has no comments.

/// The namespace this crate owns. Self-contained — the `sig:`/`log:` precedent — so no
/// arc here blocks on a manual `ikigai-rs.dev/ns` deploy.
pub const LEDGER_NS: &str = "https://ikigai-rs.dev/ns/ledger#";

/// The vocabulary source, embedded.
pub const VOCABULARY_TTL: &str = include_str!("vocabulary.ttl");

/// IRI prefixes for the skolemized nodes this module mints that belong to NO ledger.
/// Every emitted node has a stable IRI; nothing here is ever a blank node.
///
/// A ledger's own subjects — items, comments, tombstones, its counter and its selections
/// — are minted by [`Ledger`](crate::Ledger), because they carry its name.
pub mod iri {
    /// `urn:iki:ledger:policy:{name}` — a property of the host's configuration rather
    /// than of any one ledger, which is why it has no ledger segment.
    pub const POLICY: &str = "urn:iki:ledger:policy:";
}

/// External terms, reused rather than reinvented.
pub mod ext {
    /// `dcterms:title`
    pub const TITLE: &str = "http://purl.org/dc/terms/title";
    /// `dcterms:created`
    pub const CREATED: &str = "http://purl.org/dc/terms/created";
    /// `dcterms:modified`
    pub const MODIFIED: &str = "http://purl.org/dc/terms/modified";
    /// `rdf:type`
    pub const TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
    /// `rdfs:label`
    pub const LABEL: &str = "http://www.w3.org/2000/01/rdf-schema#label";
    /// `rdfs:comment`
    pub const COMMENT: &str = "http://www.w3.org/2000/01/rdf-schema#comment";
    /// `prov:generatedAtTime`
    pub const GENERATED_AT: &str = "http://www.w3.org/ns/prov#generatedAtTime";
    /// `prov:invalidatedAtTime`
    pub const INVALIDATED_AT: &str = "http://www.w3.org/ns/prov#invalidatedAtTime";
    /// `sig:contentHash` — ikigai-sign's term, and the tagged-digest convention with it,
    /// so a tombstone's hash joins with a signature graph instead of colliding with one.
    /// Named rather than depended on: one `&str` does not justify pulling
    /// ed25519-dalek, p256 and base64 into this crate.
    pub const CONTENT_HASH: &str = "https://ikigai-rs.dev/ns/sign#contentHash";
    /// The namespace of the term above, for the conformance walk's registration.
    pub const SIGN_NS: &str = "https://ikigai-rs.dev/ns/sign#";
    /// `xsd:dateTime`
    pub const XSD_DATETIME: &str = "http://www.w3.org/2001/XMLSchema#dateTime";
    /// `xsd:integer`
    pub const XSD_INTEGER: &str = "http://www.w3.org/2001/XMLSchema#integer";
    /// `xsd:boolean`
    pub const XSD_BOOLEAN: &str = "http://www.w3.org/2001/XMLSchema#boolean";
    /// `xsd:string`
    pub const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";
    /// `xsd:anyURI`
    pub const XSD_ANY_URI: &str = "http://www.w3.org/2001/XMLSchema#anyURI";
}

/// One `ledger:` term, as a full IRI.
macro_rules! term {
    ($(#[$doc:meta] $name:ident => $local:literal;)*) => {
        $(
            #[$doc]
            pub const $name: &str = concat!("https://ikigai-rs.dev/ns/ledger#", $local);
        )*
        /// Every `ledger:` term this module can write — the list the vocabulary test
        /// holds `vocabulary.ttl` to, so a term added in code and forgotten in the
        /// graph fails a test rather than shipping as an undefined predicate.
        pub const ALL: &[&str] = &[$($name),*];
    };
}

term! {
    /// `ledger:Item`
    ITEM_CLASS => "Item";
    /// `ledger:Comment`
    COMMENT_CLASS => "Comment";
    /// `ledger:Tombstone`
    TOMBSTONE_CLASS => "Tombstone";
    /// `ledger:Counter`
    COUNTER_CLASS => "Counter";
    /// `ledger:Ledger`
    LEDGER_CLASS => "Ledger";
    /// `ledger:Policy`
    POLICY_CLASS => "Policy";
    /// `ledger:Selection`
    SELECTION_CLASS => "Selection";
    /// `ledger:Ranking`
    RANKING_CLASS => "Ranking";
    /// `ledger:Exclusion`
    EXCLUSION_CLASS => "Exclusion";
    /// `ledger:Status`
    STATUS_CLASS => "Status";
    /// `ledger:CloseReason`
    CLOSE_REASON_CLASS => "CloseReason";
    /// `ledger:open` — the status value.
    OPEN => "open";
    /// `ledger:closed` — the status value.
    CLOSED => "closed";
    /// `ledger:done`
    DONE => "done";
    /// `ledger:wontfix`
    WONTFIX => "wontfix";
    /// `ledger:duplicate`
    DUPLICATE => "duplicate";
    /// `ledger:superseded`
    SUPERSEDED => "superseded";
    /// `ledger:auditNoChange`
    AUDIT_NO_CHANGE => "auditNoChange";
    /// `ledger:number`
    NUMBER => "number";
    /// `ledger:body`
    BODY => "body";
    /// `ledger:status`
    STATUS => "status";
    /// `ledger:closedReason`
    CLOSED_REASON => "closedReason";
    /// `ledger:priority`
    PRIORITY => "priority";
    /// `ledger:deferred`
    DEFERRED => "deferred";
    /// `ledger:about`
    ABOUT => "about";
    /// `ledger:revision`
    REVISION => "revision";
    /// `ledger:label`
    LABEL => "label";
    /// `ledger:author`
    AUTHOR => "author";
    /// `ledger:claimedBy`
    CLAIMED_BY => "claimedBy";
    /// `ledger:claimedAt`
    CLAIMED_AT => "claimedAt";
    /// `ledger:purpose`
    PURPOSE => "purpose";
    /// `ledger:onItem`
    ON_ITEM => "onItem";
    /// `ledger:blocks`
    BLOCKS => "blocks";
    /// `ledger:parent`
    PARENT => "parent";
    /// `ledger:related`
    RELATED => "related";
    /// `ledger:deletedItem`
    DELETED_ITEM => "deletedItem";
    /// `ledger:recoverable`
    RECOVERABLE => "recoverable";
    /// `ledger:quadCount`
    QUAD_COUNT => "quadCount";
    /// `ledger:reason`
    REASON => "reason";
    /// `ledger:lastNumber`
    LAST_NUMBER => "lastNumber";
    /// `ledger:policy`
    POLICY => "policy";
    /// `ledger:weighs`
    WEIGHS => "weighs";
    /// `ledger:graph`
    LEDGER_GRAPH => "graph";
    /// `ledger:itemCount`
    ITEM_COUNT => "itemCount";
    /// `ledger:openCount`
    OPEN_COUNT => "openCount";
    /// `ledger:readyCount`
    READY_COUNT => "readyCount";
    /// `ledger:excludedCount`
    EXCLUDED_COUNT => "excludedCount";
    /// `ledger:ranking`
    RANKING => "ranking";
    /// `ledger:excluded`
    EXCLUDED => "excluded";
    /// `ledger:rank`
    RANK => "rank";
    /// `ledger:item`
    ITEM => "item";
    /// `ledger:score`
    SCORE => "score";
    /// `ledger:because`
    BECAUSE => "because";
    /// `ledger:leverage`
    LEVERAGE => "leverage";
}

/// The `@prefix` header every Turtle face this module emits carries.
pub const PREFIXES: &str = concat!(
    "@prefix ledger: <https://ikigai-rs.dev/ns/ledger#> .\n",
    "@prefix dcterms: <http://purl.org/dc/terms/> .\n",
    "@prefix prov: <http://www.w3.org/ns/prov#> .\n",
    "@prefix sig: <https://ikigai-rs.dev/ns/sign#> .\n",
    "@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n",
);

/// The close reasons, as `(short name, IRI)` — the `one_of` an ArgSpec declares and the
/// value a caller writes.
pub const CLOSE_REASONS: [(&str, &str); 5] = [
    ("done", DONE),
    ("wontfix", WONTFIX),
    ("duplicate", DUPLICATE),
    ("superseded", SUPERSEDED),
    ("audit-no-change", AUDIT_NO_CHANGE),
];

/// The link types, as `(short name, predicate IRI)`.
pub const LINK_TYPES: [(&str, &str); 3] =
    [("blocks", BLOCKS), ("parent", PARENT), ("related", RELATED)];

/// The IRI for a close reason's short name.
pub fn close_reason(name: &str) -> Option<&'static str> {
    CLOSE_REASONS
        .iter()
        .find(|(short, _)| *short == name)
        .map(|(_, iri)| *iri)
}

/// The short name for a close-reason IRI.
pub fn close_reason_name(iri: &str) -> Option<&'static str> {
    CLOSE_REASONS
        .iter()
        .find(|(_, term)| *term == iri)
        .map(|(short, _)| *short)
}

/// The predicate for a link type's short name.
pub fn link_predicate(name: &str) -> Option<&'static str> {
    LINK_TYPES
        .iter()
        .find(|(short, _)| *short == name)
        .map(|(_, iri)| *iri)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ★ The constants and the graph are one artifact or they are two, and two is how a
    /// module ships a predicate nothing defines. The conformance walk's `VOCABULARY`
    /// check would catch an undefined term only in a face it actually probes; this
    /// catches every term the module can write, including the ones on paths a probe
    /// never takes (a tombstone, an exclusion).
    #[test]
    fn every_term_this_module_writes_is_defined_in_the_vocabulary() {
        let missing: Vec<&str> = ALL
            .iter()
            .copied()
            .filter(|iri| {
                let local = iri.trim_start_matches(LEDGER_NS);
                !VOCABULARY_TTL.contains(&format!("ledger:{local} a "))
            })
            .collect();
        assert!(
            missing.is_empty(),
            "these terms are written by the code and defined nowhere in vocabulary.ttl: \
             {missing:?}"
        );
    }

    #[test]
    fn the_detector_would_notice_an_undefined_term() {
        assert!(!VOCABULARY_TTL.contains("ledger:invented a "));
    }
}
