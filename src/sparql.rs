//! Talking to the store: the narrow doors, the sub-request client, and time.
//!
//! # ★ Every read and every write here goes through ONE graph
//!
//! `urn:iki:store:{select,update}` are the broad doors: `urn:cap:store:read` is the whole
//! dataset and `urn:cap:store:write` is `DROP ALL`. A sub-request carries the *caller's*
//! capability unchanged ([`Invocation::issue`] has no attenuating form), so a ledger built
//! on the broad doors would make everyone who may file an item hold the keys to the entire
//! store — and would leave anyone holding the broad read grant able to query another
//! ledger's graph directly, going around every capability this crate checks.
//!
//! So nothing here resolves a broad door. [`StoreClient`] issues
//! `urn:iki:store:graph-{select,ask}` and `urn:iki:store:graph-update`, each naming
//! [`Ledger::graph`] or [`Ledger::deleted_graph`], under grants that name that one graph:
//!
//! | what | door | grant a caller must hold |
//! | --- | --- | --- |
//! | read this ledger | `urn:iki:store:graph-select` / `graph-ask` | `urn:cap:store:read:graph:urn:iki:ledger:graph:{name}` |
//! | write this ledger | `urn:iki:store:graph-update` | `urn:cap:store:write:graph:urn:iki:ledger:graph:{name}` |
//! | archive / destroy | `urn:iki:store:graph-update` | `urn:cap:store:write:graph:urn:iki:ledger:graph:{name}:deleted` |
//!
//! The one exception is `urn:iki:ledger:ledgers`, which is inherently cross-graph and
//! resolves the broad `urn:iki:store:select` **only under a root capability** — see
//! `select_every_graph`, which is the only function in this crate that names a broad
//! door and says at length why.
//!
//! ⚠ **A scoped read sees exactly one graph and a scoped write can affect exactly one.**
//! `graph=G` is the evaluator's dataset specification — precisely `FROM <G> FROM NAMED
//! <G>` — so a `GRAPH <other>` block matches nothing rather than erroring, and the same
//! query through the broad door would answer differently. The consequence this crate
//! actually meets is in [`crate::endpoints`]'s delete path: **moving quads from the live
//! graph to the graveyard cannot be one update**, because no single scoped update can
//! touch both.
//!
//! # Escaping is not ours any more, and that is the improvement
//!
//! Text going into an update is ledger content — a title, a comment, a label — which is to
//! say whatever a caller typed. A comment body of
//! `" } ; DROP ALL ; INSERT DATA { <urn:x> <urn:y> "` interpolated naively is not a
//! rendering bug; it is an arbitrary update under whatever authority this crate is holding.
//! The terms re-exported below come from [`ikigai_store::sparql`], which **escapes
//! nothing**: it builds `oxigraph::model::Term`s and lets oxigraph serialize them, because
//! the only correct escaper for a grammar is the one that owns the grammar. This crate
//! wrote its own until 0.2.2 was on crates.io; `an_injected_literal_cannot_escape_its_quotes`
//! moved with it and is kept here too, over the re-export, so the composition is pinned and
//! not only the upstream function.

use ikigai_core::ArgRef;
use ikigai_core::{Error, Invocation, Request, Result, Verb};
use ikigai_store::sparql::{typed_literal, Literal, NamedNode, Term};
use std::collections::BTreeMap;

use crate::ledger::Ledger;

/// `urn:iki:store:graph-select` — SELECT confined to one named graph.
pub const STORE_GRAPH_SELECT: &str = "urn:iki:store:graph-select";
/// `urn:iki:store:graph-ask` — ASK confined to one named graph.
pub const STORE_GRAPH_ASK: &str = "urn:iki:store:graph-ask";
/// `urn:iki:store:graph-update` — SPARQL UPDATE confined to one named graph; the Sink
/// every write in this crate goes through.
pub const STORE_GRAPH_UPDATE: &str = "urn:iki:store:graph-update";
/// `urn:iki:store:select` — the BROAD read door, resolved by exactly one caller in this
/// crate and only under a root capability. See `select_every_graph`.
pub const STORE_SELECT: &str = "urn:iki:store:select";

/// A SPARQL string literal, and an IRI term, an integer, a boolean — built as RDF terms
/// by the crate that owns the grammar, never escaped by this one.
///
/// ```
/// use ikigai_ledger::sparql::{iri, literal};
/// assert_eq!(literal("a \"b\" c"), r#""a \"b\" c""#);
/// // ⚠ Validation, not escaping: an IRI has no escape for `>`, so a value carrying one
/// // is REFUSED. Percent-encoding it here would store a different IRI, silently.
/// assert!(iri("urn:x:a>b", "about").is_err());
/// ```
pub use ikigai_store::sparql::{boolean, integer, iri, literal, term};

/// The `xsd:dateTime` datatype, spelled once.
const XSD_DATE_TIME: &str = "http://www.w3.org/2001/XMLSchema#dateTime";

/// An `xsd:dateTime` term from milliseconds since the Unix epoch.
///
/// The store has no `datetime` constructor — it has [`typed_literal`], and the lexical
/// form is this crate's ([`iso8601`], pinned against `ikigai-log`'s) — so this is the
/// composition of the two rather than a fourth hand-built term.
pub fn datetime(millis: u64) -> String {
    typed_literal(&iso8601(millis), XSD_DATE_TIME, "time").expect("a constant datatype IRI parses")
}

/// `YYYY-MM-DDTHH:MM:SS.mmmZ` — the same lexical form `ikigai-log` writes, so a ledger
/// stamp and a log stamp sort and compare as strings without conversion.
pub fn iso8601(millis: u64) -> String {
    let days = (millis / 86_400_000) as i64;
    let rem = millis % 86_400_000;
    let (y, m, d) = civil_from_days(days);
    let (hh, mm, ss, ms) = (
        rem / 3_600_000,
        rem / 60_000 % 60,
        rem / 1000 % 60,
        rem % 1000,
    );
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}.{ms:03}Z")
}

/// Days since the Unix epoch → (year, month, day). Howard Hinnant's `civil_from_days`,
/// exact for the proleptic Gregorian calendar and needing no leap-year table. The same
/// implementation `ikigai-log` carries — copied rather than depended on, since one
/// function does not justify a crate edge, and the two are pinned by a shared test
/// vector (`the_epoch_and_a_leap_day_render_exactly`).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// One bound value in a SPARQL result row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    /// `uri`, `literal` or `bnode`, as the results format reports it.
    pub kind: String,
    /// The lexical value.
    pub value: String,
    /// The datatype IRI, when the value is a typed literal.
    pub datatype: Option<String>,
    /// The language tag, when the value is a language-tagged literal.
    ///
    /// ⚠ **Nothing in this crate writes one**, and it is carried anyway because a delete
    /// reads quads back out and writes them into the graveyard: an out-of-band editor is
    /// a supported path here (see the README), so a hand-written `"titre"@fr` must
    /// survive being archived. Dropping the tag would store a *different* statement under
    /// the name of the one that was deleted.
    pub lang: Option<String>,
}

impl Binding {
    /// The value as an `i64`, or `None` when it does not parse as one.
    pub fn as_i64(&self) -> Option<i64> {
        self.value.parse().ok()
    }

    /// This binding as an RDF [`Term`], for writing back out.
    ///
    /// ⚠ **A blank node is REFUSED rather than round-tripped.** A bnode label in a result
    /// row is scoped to that result set, and re-inserting it through `INSERT DATA` mints a
    /// *fresh* blank node — SPARQL says so explicitly — so archiving one would silently
    /// substitute a different node for the one being deleted. This module skolemizes and
    /// writes no blank nodes; one can only be here because something wrote the graph out
    /// of band, and telling that operator is better than quietly losing the edge.
    pub fn term(&self, arg: &str) -> Result<Term> {
        match self.kind.as_str() {
            "uri" => Ok(Term::from(NamedNode::new(&self.value).map_err(|e| {
                Error::InvalidArgument {
                    name: arg.to_string(),
                    detail: format!(
                        "the store returned `{}`, which is not an IRI: {e}",
                        self.value
                    ),
                }
            })?)),
            "bnode" => Err(Error::Endpoint(format!(
                "this ledger's graph holds a blank node (`_:{}`), which this crate never \
                 writes — every node it mints is skolemized. Archiving it would mint a \
                 different node, because a blank-node label does not survive `INSERT DATA`, \
                 so the delete is refused instead. Replace it with an IRI (one SPARQL \
                 UPDATE over this graph) and delete again",
                self.value
            ))),
            _ => match (&self.lang, &self.datatype) {
                (Some(tag), _) => Ok(Term::from(
                    Literal::new_language_tagged_literal(&self.value, tag).map_err(|e| {
                        Error::InvalidArgument {
                            name: arg.to_string(),
                            detail: format!("the store returned the language tag `{tag}`: {e}"),
                        }
                    })?,
                )),
                (None, Some(datatype)) => Ok(Term::from(Literal::new_typed_literal(
                    &self.value,
                    NamedNode::new(datatype).map_err(|e| Error::InvalidArgument {
                        name: arg.to_string(),
                        detail: format!("the store returned the datatype `{datatype}`: {e}"),
                    })?,
                ))),
                (None, None) => Ok(Term::from(Literal::new_simple_literal(&self.value))),
            },
        }
    }
}

/// One SPARQL result row: variable name → bound value. Unbound variables are absent.
pub type Row = BTreeMap<String, Binding>;

/// The store, reached the only way an in-process consumer can reach it: through the
/// kernel, as sub-requests carrying the caller's own capability — **through the narrow
/// doors, naming one ledger's graph**.
///
/// ⚠ **The capability is the caller's, unchanged.** `Invocation::issue` has no
/// attenuating or elevating form, so a ledger read succeeds only for a caller who also
/// holds the store's grant for this ledger's graph, and a write only for one holding the
/// store's write grant for it. That is why every action in this crate declares the store
/// scopes as well as its own — and because those scopes now name a *graph*, holding them
/// for `acme` grants nothing over `bosatsu`. See `README.md`, "What is enforced, and
/// where".
///
/// Every query and every update this client issues names [`Ledger::graph`] or
/// [`Ledger::deleted_graph`] **twice over**: in the `GRAPH <…>` block of the SPARQL, and
/// in the `graph=` argument that fixes the store's dataset. The first keeps this module
/// honest; the second is the fence, and it holds against a caller who tries to go around
/// this module entirely.
pub struct StoreClient<'a, 'i> {
    inv: &'a Invocation<'i>,
    ledger: Ledger,
}

impl<'a, 'i> StoreClient<'a, 'i> {
    /// A client over this invocation's kernel, reading and writing one ledger's graph.
    pub fn new(inv: &'a Invocation<'i>, ledger: Ledger) -> Self {
        StoreClient { inv, ledger }
    }

    /// The ledger this client is scoped to.
    pub fn ledger(&self) -> &Ledger {
        &self.ledger
    }

    /// The `GRAPH <…> { … }` wrapper every query and update of this ledger carries.
    ///
    /// One method rather than a free function taking a graph IRI: a caller that has to
    /// *pass* the graph is a caller that can pass the wrong one.
    pub fn in_graph(&self, body: &str) -> String {
        format!("GRAPH <{}> {{ {body} }}", self.ledger.graph())
    }

    /// The same wrapper over this ledger's graveyard.
    pub fn in_deleted_graph(&self, body: &str) -> String {
        format!("GRAPH <{}> {{ {body} }}", self.ledger.deleted_graph())
    }

    /// Evaluate a SPARQL SELECT over this ledger's graph and return its rows.
    pub async fn select(&self, query: &str) -> Result<Vec<Row>> {
        let bytes = self
            .query(STORE_GRAPH_SELECT, &self.ledger.graph(), query)
            .await?;
        parse_results(&bytes)
    }

    /// Evaluate a SPARQL ASK over this ledger's graph.
    pub async fn ask(&self, query: &str) -> Result<bool> {
        let bytes = self
            .query(STORE_GRAPH_ASK, &self.ledger.graph(), query)
            .await?;
        let json: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|e| Error::Endpoint(format!("the store's ASK answer is not JSON: {e}")))?;
        json.get("boolean")
            .and_then(|v| v.as_bool())
            .ok_or_else(|| Error::Endpoint("the store's ASK answer has no `boolean`".to_string()))
    }

    /// Apply a SPARQL UPDATE to this ledger's graph. The store cuts
    /// `urn:iki:store:graph-update` on success, which is what makes every cacheable read
    /// of this ledger recompute.
    pub async fn update(&self, update: &str) -> Result<()> {
        self.update_graph(&self.ledger.graph(), update).await
    }

    /// Apply a SPARQL UPDATE to this ledger's **graveyard**.
    ///
    /// ★ A second method rather than a graph argument, for the reason [`in_graph`] gives:
    /// a caller that has to *pass* the graph is a caller that can pass the wrong one. The
    /// two graphs are two write grants, and the only code that needs this one is the
    /// delete path — see `endpoints::delete_item`, which explains why archiving and
    /// removing cannot be one update.
    ///
    /// [`in_graph`]: StoreClient::in_graph
    pub async fn update_deleted(&self, update: &str) -> Result<()> {
        self.update_graph(&self.ledger.deleted_graph(), update)
            .await
    }

    async fn update_graph(&self, graph: &str, update: &str) -> Result<()> {
        let target = ikigai_core::Iri::parse(STORE_GRAPH_UPDATE).expect("a constant IRI");
        self.inv
            .issue(
                Request::new(Verb::Sink, target)
                    .with_arg("content", ArgRef::Inline(update.as_bytes().to_vec()))
                    .with_arg("graph", ArgRef::Inline(graph.as_bytes().to_vec())),
            )
            .await
            .map_err(store_missing)?;
        Ok(())
    }

    async fn query(&self, endpoint: &str, graph: &str, query: &str) -> Result<Vec<u8>> {
        let target = ikigai_core::Iri::parse(endpoint).expect("a constant IRI");
        let repr = self
            .inv
            .issue(
                Request::new(Verb::Source, target)
                    .with_arg("query", ArgRef::Inline(query.as_bytes().to_vec()))
                    .with_arg("graph", ArgRef::Inline(graph.as_bytes().to_vec())),
            )
            .await
            .map_err(store_missing)?;
        Ok(repr.bytes)
    }
}

/// One SELECT over **every** graph in the store, through the broad
/// `urn:iki:store:select` door.
///
/// ⚠ **This is the one place in this crate that resolves a broad store door, it has one
/// caller, and that caller resolves it only when `inv.capability.is_root()`.** Read that
/// as a precondition, not a convention: under any other capability this issues a request
/// requiring `urn:cap:store:read` — the whole dataset — from an action that declares only
/// the per-graph family, which is exactly the over-offer the module recipe forbids.
///
/// It exists because `urn:iki:ledger:ledgers` asks a question no scoped read can answer:
/// *which* graphs are there. A scoped read is confined to a graph the caller must already
/// have named, so enumeration through it is only possible from a set of names known in
/// advance — which is what a non-root capability carries (its `urn:cap:ledger:read:{name}`
/// grants) and what a root capability, by construction, does not. Root holds every grant
/// there is, so asking the whole store under it widens nothing; it is simply the only way
/// the question can be answered at all.
///
/// The alternative considered and rejected was a shared registry graph listing the
/// ledgers: every ledger write would then need write access to one graph all the others
/// write too — the same path across the boundary the per-ledger graveyard exists to
/// avoid — and its read grant would hand out the whole client list.
pub(crate) async fn select_every_graph(inv: &Invocation<'_>, query: &str) -> Result<Vec<Row>> {
    debug_assert!(
        inv.capability.is_root(),
        "select_every_graph is root-only: see its doc comment"
    );
    let target = ikigai_core::Iri::parse(STORE_SELECT).expect("a constant IRI");
    let repr = inv
        .issue(
            Request::new(Verb::Source, target)
                .with_arg("query", ArgRef::Inline(query.as_bytes().to_vec())),
        )
        .await
        .map_err(store_missing)?;
    parse_results(&repr.bytes)
}

/// Make the one composition failure legible rather than letting it surface as the
/// kernel's generic "no endpoint bound".
///
/// A host that binds `urn:iki:ledger:*` without a store space gets a resolution failure
/// naming `urn:iki:store:select`, which is true and says nothing about what to do. This
/// is the module's own sentence about its own precondition — the same move
/// `ikigai-store` makes for RocksDB's raw lock string.
fn store_missing(e: Error) -> Error {
    let text = e.to_string();
    if text.contains("urn:iki:store:") && (text.contains("no endpoint") || text.contains("unbound"))
    {
        Error::Endpoint(format!(
            "the ledger is bound but the durable store is not: every ledger read and write \
             is a sub-request to `urn:iki:store:graph-*`, so this host must also bind \
             `ikigai_store::space(DurableStore::open(..))` — version 0.2.2 or later, which \
             is where the graph-scoped doors arrive — in the same kernel. Underlying \
             error: {text}"
        ))
    } else {
        e
    }
}

/// Parse `application/sparql-results+json` into rows.
fn parse_results(bytes: &[u8]) -> Result<Vec<Row>> {
    let json: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|e| Error::Endpoint(format!("the store's SELECT answer is not JSON: {e}")))?;
    let bindings = json
        .get("results")
        .and_then(|r| r.get("bindings"))
        .and_then(|b| b.as_array())
        .ok_or_else(|| {
            Error::Endpoint("the store's SELECT answer has no results.bindings".to_string())
        })?;
    let mut rows = Vec::with_capacity(bindings.len());
    for binding in bindings {
        let object = binding.as_object().ok_or_else(|| {
            Error::Endpoint("a SELECT result binding is not an object".to_string())
        })?;
        let mut row = Row::new();
        for (var, value) in object {
            let kind = value
                .get("type")
                .and_then(|t| t.as_str())
                .unwrap_or("literal")
                .to_string();
            let lexical = value
                .get("value")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let datatype = value
                .get("datatype")
                .and_then(|d| d.as_str())
                .map(str::to_string);
            // The SPARQL results JSON spells it `xml:lang`, and a tagged literal carries
            // no `datatype` key at all — so the two are alternatives, not both.
            let lang = value
                .get("xml:lang")
                .and_then(|l| l.as_str())
                .map(str::to_string);
            row.insert(
                var.clone(),
                Binding {
                    kind,
                    value: lexical,
                    datatype,
                    lang,
                },
            );
        }
        rows.push(row);
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ★ The test this module exists for, kept after the escaper moved upstream.
    ///
    /// It now pins the **composition** rather than an implementation: whatever
    /// `ikigai_store::sparql::literal` does, a comment body that tries to close the
    /// literal and start a new statement comes back as ONE literal, quotes and all. An
    /// upstream regression fails here, in the crate that would be exploited by it, and not
    /// only in the crate that would have caused it.
    #[test]
    fn an_injected_literal_cannot_escape_its_quotes() {
        let hostile = r#"" } ; DROP ALL ; INSERT DATA { <urn:x> <urn:y> ""#;
        let escaped = literal(hostile);
        // Exactly two unescaped quotes: the ones this function added.
        let unescaped_quotes = escaped
            .char_indices()
            .filter(|(i, c)| *c == '"' && (*i == 0 || escaped.as_bytes()[i - 1] != b'\\'))
            .count();
        assert_eq!(unescaped_quotes, 2, "escaped form was {escaped}");
        assert!(escaped.starts_with('"') && escaped.ends_with('"'));
        // Every interior quote is behind a backslash — the statement never closes early.
        assert!(escaped[1..escaped.len() - 1].contains("\\\""));
    }

    #[test]
    fn a_backslash_is_escaped_before_a_quote_can_hide_behind_it() {
        // `\"` typed by a user must not become an escape for OUR closing quote.
        assert_eq!(literal("a\\\"b"), r#""a\\\"b""#);
    }

    /// A raw control character in a query is accepted by some parsers and rejected by
    /// others, and "accepted by some" is how a round-trip silently loses a byte.
    #[test]
    fn control_characters_become_escapes() {
        let bell = literal("a\u{7}b");
        assert_eq!(bell, "\"a\\u0007b\"");
        assert!(!bell.chars().any(|c| (c as u32) < 0x20));
        assert_eq!(literal("a\u{0}b"), "\"a\\u0000b\"");
    }

    #[test]
    fn an_iri_with_a_closing_bracket_is_refused_not_mangled() {
        let refused = iri("urn:x:a>b", "about");
        assert!(refused.is_err(), "got {refused:?}");
        // And an ordinary one passes through untouched.
        assert_eq!(
            iri("urn:repo:file:ikigai-cli/src/main.rs", "about").unwrap(),
            "<urn:repo:file:ikigai-cli/src/main.rs>"
        );
    }

    #[test]
    fn the_epoch_and_a_leap_day_render_exactly() {
        assert_eq!(iso8601(0), "1970-01-01T00:00:00.000Z");
        // 2024-02-29T12:34:56.789Z
        assert_eq!(iso8601(1_709_210_096_789), "2024-02-29T12:34:56.789Z");
    }

    #[test]
    fn results_parse_into_typed_bindings() {
        let json = br#"{"head":{"vars":["s","n"]},"results":{"bindings":[
            {"s":{"type":"uri","value":"urn:iki:ledger:item:abc"},
             "n":{"type":"literal","value":"3",
                  "datatype":"http://www.w3.org/2001/XMLSchema#integer"}}]}}"#;
        let rows = parse_results(json).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["s"].value, "urn:iki:ledger:item:abc");
        assert_eq!(rows[0]["n"].as_i64(), Some(3));
        assert_eq!(rows[0]["s"].kind, "uri");
    }

    /// ★ A delete reads quads out and writes them into the graveyard, so every term
    /// **this crate never writes** still has to survive the round trip — an out-of-band
    /// editor is a supported path here. The language tag is the one that would have been
    /// dropped silently: it arrives as `xml:lang` and not as a datatype.
    #[test]
    fn a_language_tag_survives_being_read_back_out() {
        let json = br#"{"head":{"vars":["o"]},"results":{"bindings":[
            {"o":{"type":"literal","value":"titre","xml:lang":"fr"}}]}}"#;
        let rows = parse_results(json).unwrap();
        assert_eq!(rows[0]["o"].lang.as_deref(), Some("fr"));
        assert_eq!(rows[0]["o"].term("o").unwrap().to_string(), r#""titre"@fr"#);

        // A typed literal and an IRI keep their exact form too.
        let json = br#"{"head":{"vars":["o"]},"results":{"bindings":[
            {"o":{"type":"literal","value":"3",
                  "datatype":"http://www.w3.org/2001/XMLSchema#integer"}}]}}"#;
        let rows = parse_results(json).unwrap();
        assert_eq!(
            rows[0]["o"].term("o").unwrap().to_string(),
            r#""3"^^<http://www.w3.org/2001/XMLSchema#integer>"#
        );
    }

    /// A blank node cannot be archived without becoming a *different* blank node, so it
    /// is refused with a sentence rather than silently substituted.
    #[test]
    fn a_blank_node_is_refused_rather_than_reminted() {
        let binding = Binding {
            kind: "bnode".to_string(),
            value: "b0".to_string(),
            datatype: None,
            lang: None,
        };
        let message = binding.term("o").expect_err("a bnode").to_string();
        assert!(message.contains("skolemized"), "{message}");
    }
}
