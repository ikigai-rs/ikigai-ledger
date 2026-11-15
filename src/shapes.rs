//! **Write shapes** (ledger #3724): a pre-write check, so a SHACL shape can refuse or warn on a
//! hand-back, a comment or a close before it lands.
//!
//! A host lists shapes per ledger with [`crate::SpaceConfig::write_shape`]. Each one names a
//! shapes graph by IRI (a resource, read through the kernel when the write is made), the
//! [`WriteKind`]s it governs and its [`Outcome`]. Today a shape can govern `report`, `close` and
//! `comment`; `claim` and `label` are refused when the manifest is built (see below). **Off by default**: with no shape configured a
//! write does exactly what it did before, and issues nothing more.
//!
//! # What is validated: the write's would-be graph
//!
//! Before the update, the write is described as the triples it would add, in the ledger's own
//! vocabulary, and that graph is validated through `urn:shacl:validate` in the same request
//! (bind `ikigai-shacl`, or another implementation of that resource, in the same kernel):
//!
//! | kind | the would-be graph |
//! | --- | --- |
//! | `close` | `<{item}:status:pending> a ledger:StatusEvent ; ledger:onItem <item> ; ledger:status ledger:closed ; ledger:closedReason <reason> ; ledger:evidence <iri>…` |
//! | `comment`, `report` | `<{item}:comment:pending> a ledger:Comment ; ledger:onItem <item> ; ledger:body "…"` (`report` is a comment with `handback=true`) |
//!
//! So "a close carries evidence" is a shape on `ledger:StatusEvent` with `sh:minCount 1` on
//! `ledger:evidence`.
//!
//! # `claim` and `label` are not checked yet, so they are refused at the manifest
//!
//! [`WriteKind::Claim`] and [`WriteKind::Label`] exist so the words are reserved, but no would-be
//! graph is built for those writes yet. A shape that listed them would look enforced and not be,
//! so [`crate::SpaceConfig::write_shape`] panics when the manifest is built if a shape lists
//! either one, the same way it refuses a shape that is not an IRI.
//!
//! # Outcomes
//!
//! - [`Outcome::Refuse`]: a write that does not conform is refused with
//!   [`ikigai_core::Error::Conflict`], naming the shape and each violated constraint in words,
//!   and nothing is written.
//! - [`Outcome::Warn`]: the write lands, and the firing is recorded on this request's trace span
//!   as the note [`WARN_NOTE`], whose value names the control (the shape's IRI) and the
//!   constraint. The tracer writes that span as an ikigai-log line.
//!
//! # ⚠ A shape that fails to load refuses nothing
//!
//! A shapes resource that cannot be read, or a validator that is not bound or refuses the shapes
//! graph, refuses nothing: the write goes on, and the failure is recorded on the trace span as
//! [`UNLOADED_NOTE`], naming the shape and the error. It fails loud in the log, not at the
//! caller.

use std::collections::BTreeSet;

use ikigai_core::{ArgRef, Error, Invocation, Iri, Request, Result, Verb};

/// The trace note a [`Outcome::Warn`] shape's firing is recorded under. Its value is
/// `{shape IRI} {kind}: {what was violated}`.
pub const WARN_NOTE: &str = "ledger-shape-warning";

/// The trace note a shape that could not be loaded or run is recorded under. Its value is
/// `{shape IRI} {kind}: {the error}`.
pub const UNLOADED_NOTE: &str = "ledger-shape-unloaded";

/// The resource a write's graph is validated at.
pub const VALIDATE: &str = "urn:shacl:validate";

/// The writes a shape can govern.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum WriteKind {
    /// Taking a claim. **Not checked yet**: a shape that lists it is refused when the manifest
    /// is built.
    Claim,
    /// A hand-back: a comment with `handback=true`.
    Report,
    /// A close.
    Close,
    /// Adding a label. **Not checked yet**: a shape that lists it is refused when the manifest
    /// is built.
    Label,
    /// Any comment, a hand-back included.
    Comment,
}

impl WriteKind {
    /// The word a configuration and a message use: `claim`, `report`, `close`, `label`,
    /// `comment`.
    pub fn name(self) -> &'static str {
        match self {
            WriteKind::Claim => "claim",
            WriteKind::Report => "report",
            WriteKind::Close => "close",
            WriteKind::Label => "label",
            WriteKind::Comment => "comment",
        }
    }

    /// Whether the ledger builds a would-be graph for this kind of write and checks it: `true`
    /// for `report`, `close` and `comment`, `false` for `claim` and `label`.
    pub fn is_checked(self) -> bool {
        match self {
            WriteKind::Report | WriteKind::Close | WriteKind::Comment => true,
            WriteKind::Claim | WriteKind::Label => false,
        }
    }
}

/// What a shape does with a write that does not conform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Outcome {
    /// Let it land, and record the firing.
    Warn,
    /// Refuse it, naming the shape and the constraint.
    Refuse,
}

/// One configured shape.
#[derive(Debug, Clone)]
pub(crate) struct WriteShape {
    pub(crate) ledger: String,
    pub(crate) shape: String,
    pub(crate) kinds: BTreeSet<WriteKind>,
    pub(crate) outcome: Outcome,
}

/// Every configured shape, for every ledger.
pub(crate) type WriteShapes = std::sync::Arc<Vec<WriteShape>>;

/// One violation, as the JSON face of `urn:shacl:validate` reports it.
#[derive(serde::Deserialize)]
struct Violation {
    #[serde(default)]
    path: Option<String>,
    component: String,
    #[serde(default)]
    message: Option<serde_json::Value>,
}

#[derive(serde::Deserialize)]
struct Report {
    conforms: bool,
    #[serde(default)]
    violations: Vec<Violation>,
}

/// The last segment of an IRI, for a sentence: `…#MinCountConstraintComponent` → `MinCount`.
fn local(iri: &str) -> &str {
    let tail = iri.rsplit(['#', '/', ':']).next().unwrap_or(iri);
    tail.strip_suffix("ConstraintComponent").unwrap_or(tail)
}

/// One violation in words: `MinCount on evidence: the message`.
fn say(violation: &Violation) -> String {
    let mut words = local(&violation.component).to_string();
    if let Some(path) = &violation.path {
        words.push_str(" on ");
        words.push_str(local(path));
    }
    let message = violation.message.as_ref().and_then(|m| match m {
        serde_json::Value::Object(o) => o.get("value").and_then(|v| v.as_str()),
        serde_json::Value::String(s) => Some(s.as_str()),
        _ => None,
    });
    if let Some(message) = message {
        words.push_str(": ");
        words.push_str(message);
    }
    words
}

/// Validate one shape; `Ok(None)` when the write conforms.
async fn run(inv: &Invocation<'_>, shape: &str, data: &str) -> Result<Option<Vec<Violation>>> {
    let iri = Iri::parse(shape)
        .map_err(|e| Error::Endpoint(format!("the shape `{shape}` is not an IRI: {e}")))?;
    let shapes = inv.source(&iri).await?;
    let shapes = String::from_utf8(shapes.bytes)
        .map_err(|e| Error::Endpoint(format!("the shape `{shape}` is not UTF-8: {e}")))?;
    let request = Request::new(Verb::Source, Iri::parse(VALIDATE).expect("a constant IRI"))
        .with_arg("data", ArgRef::Inline(data.as_bytes().to_vec()))
        .with_arg("shapes", ArgRef::Inline(shapes.into_bytes()))
        .with_arg("as", ArgRef::Inline(b"application/json".to_vec()));
    let answer = inv.issue(request).await?;
    let report: Report = serde_json::from_slice(&answer.bytes)
        .map_err(|e| Error::Endpoint(format!("`{VALIDATE}` answered no report: {e}")))?;
    Ok((!report.conforms).then_some(report.violations))
}

/// Check a write's would-be graph (`data`, N-Triples) against every shape `ledger` configures
/// for `kinds`. Refuses with [`Error::Conflict`] when a refusing shape does not conform; records
/// a warning shape's firing, and a shape that fails to load, on the trace span. With no shape
/// configured it returns at once and issues nothing.
pub(crate) async fn check(
    inv: &Invocation<'_>,
    shapes: &[WriteShape],
    ledger: &crate::Ledger,
    kinds: &[WriteKind],
    subject: &str,
    data: impl FnOnce() -> String,
) -> Result<()> {
    let governing: Vec<(&WriteShape, WriteKind)> = shapes
        .iter()
        .filter(|s| s.ledger == ledger.name())
        .filter_map(|s| {
            kinds
                .iter()
                .find(|k| s.kinds.contains(k))
                .map(|kind| (s, *kind))
        })
        .collect();
    if governing.is_empty() {
        return Ok(());
    }
    let data = data();
    for (rule, kind) in governing {
        let violations = match run(inv, &rule.shape, &data).await {
            Ok(None) => continue,
            Ok(Some(violations)) => violations,
            Err(error) => {
                inv.trace_note(
                    UNLOADED_NOTE,
                    format!("{} {}: {error}", rule.shape, kind.name()),
                );
                continue;
            }
        };
        let words: Vec<String> = violations.iter().map(say).collect();
        let words = if words.is_empty() {
            "it does not conform".to_string()
        } else {
            words.join("; ")
        };
        match rule.outcome {
            Outcome::Warn => inv.trace_note(
                WARN_NOTE,
                format!("{} {}: {words}", rule.shape, kind.name()),
            ),
            Outcome::Refuse => {
                return Err(Error::Conflict(format!(
                    "this {} of {subject} does not meet the shape `{}`, which this ledger \
                     requires of every {}: {words}. Nothing was changed",
                    kind.name(),
                    rule.shape,
                    kind.name(),
                )))
            }
        }
    }
    Ok(())
}

/// The would-be graph as N-Triples, one triple per line.
pub(crate) fn ntriples(triples: &[oxrdf::Triple]) -> String {
    triples.iter().map(|t| format!("{t} .\n")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_violation_is_said_in_words() {
        let v = Violation {
            path: Some("https://ikigai-rs.dev/ns/ledger#evidence".into()),
            component: "http://www.w3.org/ns/shacl#MinCountConstraintComponent".into(),
            message: Some(
                serde_json::json!({"type": "literal", "value": "a close needs evidence"}),
            ),
        };
        assert_eq!(say(&v), "MinCount on evidence: a close needs evidence");
    }
}
