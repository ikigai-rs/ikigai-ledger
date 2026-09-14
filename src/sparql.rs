//! Talking to the store: escaping, the sub-request client, and time.
//!
//! # ★ Escaping is a security boundary here, not a formatting detail
//!
//! Every write is a SPARQL UPDATE string, and the text going into it is ledger content —
//! a title, a comment, a label, a name — which is to say, whatever a caller typed. A
//! comment body of `" } ; DROP ALL ; INSERT DATA { <urn:x> <urn:y> "` interpolated
//! naively is not a rendering bug, it is the keys to the store, since `urn:iki:store:update`
//! takes an arbitrary update and this module holds `urn:cap:store:write` by the time it
//! runs. [`literal`] is the only way text becomes a SPARQL term in this crate, and
//! `an_injected_literal_cannot_escape_its_quotes` is the test that keeps it that way.
//!
//! There is no parameter binding to reach for: `urn:iki:store:update` takes a string.
//! That is the shape of the composition, and it is reported as friction rather than
//! worked around.
//!
//! ⚠ **This escaper is on its way out and is not the one to copy.** `ikigai-store` 0.2.1
//! carries `ikigai_store::sparql`, which escapes nothing: it builds
//! `oxigraph::model::Term`s and lets oxigraph serialize them, because the only correct
//! escaper for a grammar is the one that owns the grammar. Its query endpoints also take
//! a `bindings=` argument, where the value never reaches the parser at all. This crate
//! still carries its own because 0.2.1 is not on crates.io; the hostile-content test
//! below has already been upstreamed, so the two cannot quietly diverge on what they
//! promise.

use ikigai_core::ArgRef;
use ikigai_core::{Error, Invocation, Iri, Request, Result, Verb};
use std::collections::BTreeMap;

use crate::ledger::Ledger;

/// `urn:iki:store:select` — SPARQL SELECT over the host's durable store.
pub const STORE_SELECT: &str = "urn:iki:store:select";
/// `urn:iki:store:ask`
pub const STORE_ASK: &str = "urn:iki:store:ask";
/// `urn:iki:store:update` — SPARQL UPDATE; the Sink every write here goes through.
pub const STORE_UPDATE: &str = "urn:iki:store:update";

/// A SPARQL string literal, escaped so that no content can leave it.
///
/// Escapes the two characters that end a literal or start an escape (`\` and `"`) and
/// the three whitespace characters that would otherwise end the line, then anything else
/// below U+0020 as `\uXXXX` — because a raw control character in a query is accepted by
/// some parsers and rejected by others, and "accepted by some" is how a round-trip
/// silently loses a byte.
///
/// ```
/// use ikigai_ledger::sparql::literal;
/// assert_eq!(literal("a \"b\" c"), r#""a \"b\" c""#);
/// assert_eq!(literal("line\nbreak"), r#""line\nbreak""#);
/// ```
pub fn literal(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A SPARQL IRI term, `<…>`, after checking that it really is an IRI.
///
/// ⚠ Validation, not escaping: an IRI has no escape for `>`, so a value carrying one
/// cannot be written at all and must be REFUSED. Truncating or stripping would store a
/// different IRI than the caller named, silently.
pub fn iri_term(value: &str, arg: &str) -> Result<String> {
    let parsed = Iri::parse(value).map_err(|_| Error::InvalidArgument {
        name: arg.to_string(),
        detail: format!("`{value}` is not an IRI"),
    })?;
    let text = parsed.as_str();
    if text.chars().any(|c| {
        c == '<'
            || c == '>'
            || c == '"'
            || c == '{'
            || c == '}'
            || c == '|'
            || c == '^'
            || c == '`'
            || c == '\\'
            || (c as u32) <= 0x20
    }) {
        return Err(Error::InvalidArgument {
            name: arg.to_string(),
            detail: format!(
                "`{value}` contains a character an IRI term cannot carry (<>\"{{}}|^`\\ or a \
                 space). Percent-encode it; it is not escaped here because an IRI has no \
                 escape and quietly storing a different IRI is worse"
            ),
        });
    }
    Ok(format!("<{text}>"))
}

/// An `xsd:integer` term.
pub fn integer(value: i64) -> String {
    format!("\"{value}\"^^<http://www.w3.org/2001/XMLSchema#integer>")
}

/// An `xsd:boolean` term.
pub fn boolean(value: bool) -> String {
    format!("\"{value}\"^^<http://www.w3.org/2001/XMLSchema#boolean>")
}

/// An `xsd:dateTime` term from milliseconds since the Unix epoch.
pub fn datetime(millis: u64) -> String {
    format!(
        "\"{}\"^^<http://www.w3.org/2001/XMLSchema#dateTime>",
        iso8601(millis)
    )
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
}

impl Binding {
    /// The value as an `i64`, or `None` when it does not parse as one.
    pub fn as_i64(&self) -> Option<i64> {
        self.value.parse().ok()
    }
}

/// One SPARQL result row: variable name → bound value. Unbound variables are absent.
pub type Row = BTreeMap<String, Binding>;

/// The store, reached the only way an in-process consumer can reach it: through the
/// kernel, as sub-requests carrying the caller's own capability — **scoped to one
/// ledger's graph**.
///
/// ⚠ **The capability is the caller's, unchanged.** `Invocation::issue` has no
/// attenuating or elevating form, so a ledger write succeeds only for a caller who also
/// holds `urn:cap:store:write`. That is why every mutating action in this crate declares
/// the store scopes as well as its own, and it is the half of the tenancy boundary the
/// substrate does not yet provide — see `README.md`, "What is enforced, and where".
///
/// Every query and every update this client issues names [`Ledger::graph`], so an
/// endpoint cannot read or write another ledger by forgetting to say which one it meant.
/// That is a *construction*, not an enforcement: it keeps this module honest, and it is
/// not a fence against a caller who goes to `urn:iki:store:select` directly.
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

    /// Evaluate a SPARQL SELECT and return its rows.
    pub async fn select(&self, query: &str) -> Result<Vec<Row>> {
        let bytes = self.query(STORE_SELECT, query).await?;
        parse_results(&bytes)
    }

    /// Evaluate a SPARQL ASK.
    pub async fn ask(&self, query: &str) -> Result<bool> {
        let bytes = self.query(STORE_ASK, query).await?;
        let json: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|e| Error::Endpoint(format!("the store's ASK answer is not JSON: {e}")))?;
        json.get("boolean")
            .and_then(|v| v.as_bool())
            .ok_or_else(|| Error::Endpoint("the store's ASK answer has no `boolean`".to_string()))
    }

    /// Apply a SPARQL UPDATE. The store cuts `urn:iki:store:update` on success, which is
    /// what makes every cacheable read of this ledger recompute.
    pub async fn update(&self, update: &str) -> Result<()> {
        let target = Iri::parse(STORE_UPDATE).expect("a constant IRI");
        self.inv
            .issue(
                Request::new(Verb::Sink, target)
                    .with_arg("content", ArgRef::Inline(update.as_bytes().to_vec())),
            )
            .await
            .map_err(store_missing)?;
        Ok(())
    }

    async fn query(&self, endpoint: &str, query: &str) -> Result<Vec<u8>> {
        let target = Iri::parse(endpoint).expect("a constant IRI");
        let repr = self
            .inv
            .issue(
                Request::new(Verb::Source, target)
                    .with_arg("query", ArgRef::Inline(query.as_bytes().to_vec())),
            )
            .await
            .map_err(store_missing)?;
        Ok(repr.bytes)
    }
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
             is a sub-request to `urn:iki:store:*`, so this host must also bind \
             `ikigai_store::space(DurableStore::open(..))` in the same kernel. Underlying \
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
            row.insert(
                var.clone(),
                Binding {
                    kind,
                    value: lexical,
                    datatype,
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

    /// ★ The test this module exists for. A comment body that tries to close the literal
    /// and start a new statement must come back as ONE literal, quotes and all.
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
        let refused = iri_term("urn:x:a>b", "about");
        assert!(refused.is_err(), "got {refused:?}");
        // And an ordinary one passes through untouched.
        assert_eq!(
            iri_term("urn:repo:file:ikigai-cli/src/main.rs", "about").unwrap(),
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
}
