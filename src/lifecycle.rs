//! **Lifecycles**: the legal states an item moves through, as a resource.
//!
//! A lifecycle is a small Turtle document — `urn:iki:ledger:lifecycle:{name}` — naming the
//! states in order, which of them are **in flight** (held by a machine claim while the work
//! happens), and for every state that is not, the **drain** that owns an item parked there.
//! It is data, not code: a host hands its own to [`crate::space_with`] the way it hands its
//! own ordering policies, and one ships built in — [`Lifecycle::kata_flight`], the states
//! Chris Wensel's kata-flight loop uses (`queued, reviewed, resolving, refining, shipping`).
//!
//! # ★ Absence is a state, and it is called `filed`
//!
//! An item that holds no `ledger:state` is in the state named [`FILED`]. Every lifecycle
//! declares it — not in flight, with a drain — because an item outside the flight must say
//! who picks it up, and "nobody has put a state on it yet" is the commonest place to be. The
//! name is the same in every lifecycle, so the JSON face can say `"state": "filed"` without
//! knowing which lifecycle a ledger uses.
//!
//! # What the state resource does with it
//!
//! `urn:iki:ledger:{ledger}:item:{id}:state` checks `to=` and `from=` against the ledger's
//! lifecycle and writes the state's IRI — `{lifecycle IRI}:{name}`, which [`Lifecycle::parse`]
//! enforces, so the IRI a transition writes is the one this document declares. The doctor
//! reports a state outside it (a hand edit, an import, or a ledger whose lifecycle changed).
//!
//! ```
//! use ikigai_ledger::lifecycle::{Lifecycle, FILED};
//! let flight = Lifecycle::kata_flight();
//! assert_eq!(flight.name(), "kata-flight");
//! let names: Vec<&str> = flight.states().iter().map(|s| s.name.as_str()).collect();
//! assert_eq!(names, [FILED, "queued", "reviewed", "resolving", "refining", "shipping"]);
//! assert!(flight.state("resolving").unwrap().in_flight);
//! assert_eq!(flight.state("queued").unwrap().drain.as_deref(), Some("review-gate"));
//! assert_eq!(
//!     flight.state("queued").unwrap().iri,
//!     "urn:iki:ledger:lifecycle:kata-flight:queued"
//! );
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use ikigai_core::{Error, Result};
use oxrdf::{Graph, NamedNode, NamedOrBlankNodeRef, Term};

use crate::ledger::Ledger;
use crate::vocabulary as v;

/// The name of the state an item is in when it holds no `ledger:state`.
pub const FILED: &str = "filed";

/// `urn:iki:ledger:lifecycle:` — where every lifecycle, and every state of one, is named.
pub const PREFIX: &str = "urn:iki:ledger:lifecycle:";

/// The kata-flight lifecycle's source, embedded.
pub const KATA_FLIGHT_TTL: &str = include_str!("lifecycles/kata-flight.ttl");

/// One state of a [`Lifecycle`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct LifecycleState {
    /// The short name a caller writes (`to=queued`) and an assertion names
    /// (`…:item:{id}:state:queued`).
    pub name: String,
    /// `{lifecycle IRI}:{name}` — the value `ledger:state` holds.
    pub iri: String,
    /// Its position: the states in order are the lifecycle read left to right.
    pub order: i64,
    /// Whether an item in this state is being worked — held by a claim while it is. The
    /// doctor reports an in-flight item with no claim as abandoned, and a machine claim on an
    /// item that is not in flight as orphaned.
    pub in_flight: bool,
    /// Who picks an item up from this state — `None` exactly when the state is in flight.
    pub drain: Option<String>,
    /// What the state means, in a sentence.
    pub summary: Option<String>,
}

/// A lifecycle: its states, in order. See the module documentation.
#[derive(Debug, Clone)]
pub struct Lifecycle {
    name: String,
    iri: String,
    summary: Option<String>,
    states: Vec<LifecycleState>,
    graph: Graph,
}

impl Lifecycle {
    /// Parse a lifecycle from Turtle.
    ///
    /// The document names exactly one `ledger:Lifecycle` at `urn:iki:ledger:lifecycle:{name}`
    /// and lists its states with `ledger:hasState`. Each state carries `rdfs:label` (its name),
    /// `ledger:order`, `ledger:inFlight` and — when it is not in flight — `ledger:drain`.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidArgument`] naming what is wrong, for each of the rules the state
    /// resource relies on: a state's IRI is `{lifecycle IRI}:{name}`; names and orders are
    /// unique; a state is in flight or names a drain, never both and never neither; and
    /// [`FILED`] is declared, first, and not in flight.
    pub fn parse(turtle: &str) -> Result<Lifecycle> {
        let bad = |detail: String| Error::InvalidArgument {
            name: "lifecycle".to_string(),
            detail,
        };
        let mut graph = Graph::new();
        for triple in oxrdfio::RdfParser::from_format(oxrdfio::RdfFormat::Turtle)
            .for_reader(turtle.as_bytes())
        {
            let quad = triple.map_err(|e| bad(format!("the lifecycle is not Turtle: {e}")))?;
            graph.insert(&oxrdf::Triple::new(
                quad.subject,
                quad.predicate,
                quad.object,
            ));
        }
        let rdf_type = NamedNode::new_unchecked(v::ext::TYPE);
        let class = NamedNode::new_unchecked(v::LIFECYCLE_CLASS);
        let subjects: Vec<NamedNode> = graph
            .subjects_for_predicate_object(rdf_type.as_ref(), class.as_ref())
            .filter_map(|s| match s {
                NamedOrBlankNodeRef::NamedNode(n) => Some(n.into_owned()),
                NamedOrBlankNodeRef::BlankNode(_) => None,
            })
            .collect();
        let subject = match subjects.as_slice() {
            [one] => one.clone(),
            [] => {
                return Err(bad(format!(
                    "no `{}` with an IRI in the document",
                    v::LIFECYCLE_CLASS
                )))
            }
            _ => {
                return Err(bad(
                    "the document declares more than one lifecycle; one per document".to_string(),
                ))
            }
        };
        let iri = subject.as_str().to_string();
        let name = iri
            .strip_prefix(PREFIX)
            .ok_or_else(|| bad(format!("`{iri}` is not under `{PREFIX}`")))?
            .to_string();
        check_name(&name, "lifecycle")?;

        let literal = |s: &NamedNode, p: &str| -> Option<String> {
            match graph
                .object_for_subject_predicate(s.as_ref(), NamedNode::new_unchecked(p).as_ref())?
            {
                oxrdf::TermRef::Literal(l) => Some(l.value().to_string()),
                _ => None,
            }
        };
        let summary = literal(&subject, v::ext::COMMENT);

        let mut states = Vec::new();
        let has_state = NamedNode::new_unchecked(v::HAS_STATE);
        let listed: Vec<Term> = graph
            .objects_for_subject_predicate(subject.as_ref(), has_state.as_ref())
            .map(|t| t.into_owned())
            .collect();
        for object in listed {
            let Term::NamedNode(state) = object else {
                return Err(bad(format!(
                    "a state of `{name}` is not an IRI ({object}); every state is named \
                     `{iri}:{{state}}`"
                )));
            };
            let label = literal(&state, v::ext::LABEL)
                .ok_or_else(|| bad(format!("`{state}` has no rdfs:label (its name)")))?;
            check_name(&label, "state")?;
            let expected = format!("{iri}:{label}");
            if state.as_str() != expected {
                return Err(bad(format!(
                    "the state `{label}` is named `{}`; it must be `{expected}`, because the \
                     state Sink writes the IRI it builds from the name",
                    state.as_str()
                )));
            }
            let order = literal(&state, v::ORDER)
                .and_then(|o| o.parse::<i64>().ok())
                .ok_or_else(|| bad(format!("`{label}` has no integer ledger:order")))?;
            let in_flight = match literal(&state, v::IN_FLIGHT).as_deref() {
                Some("true") | Some("1") => true,
                Some("false") | Some("0") => false,
                _ => {
                    return Err(bad(format!(
                        "`{label}` has no boolean ledger:inFlight; every state says whether it \
                         is in flight"
                    )))
                }
            };
            let drain = literal(&state, v::DRAIN);
            match (in_flight, &drain) {
                (false, None) => {
                    return Err(bad(format!(
                        "`{label}` is not in flight and names no ledger:drain: an item parked \
                         outside the flight must say who picks it up"
                    )))
                }
                (true, Some(_)) => {
                    return Err(bad(format!(
                        "`{label}` is in flight and also names a drain; an in-flight item is \
                         owned by its claim, not by a drain"
                    )))
                }
                _ => {}
            }
            states.push(LifecycleState {
                name: label,
                iri: expected,
                order,
                in_flight,
                drain,
                summary: literal(&state, v::ext::COMMENT),
            });
        }
        states.sort_by(|a, b| a.order.cmp(&b.order).then(a.name.cmp(&b.name)));
        let names: BTreeSet<&str> = states.iter().map(|s| s.name.as_str()).collect();
        if names.len() != states.len() {
            return Err(bad(format!("`{name}` declares a state name twice")));
        }
        let orders: BTreeSet<i64> = states.iter().map(|s| s.order).collect();
        if orders.len() != states.len() {
            return Err(bad(format!(
                "`{name}` gives two states the same ledger:order; the states are an ORDER"
            )));
        }
        match states.first() {
            Some(first) if first.name == FILED && !first.in_flight => {}
            _ => {
                return Err(bad(format!(
                    "`{name}` must declare `{FILED}` as its first state, not in flight and with a \
                     drain: an item that holds no state is in `{FILED}`, and that state needs an \
                     owner like any other"
                )))
            }
        }
        Ok(Lifecycle {
            name,
            iri,
            summary,
            states,
            graph,
        })
    }

    /// The built-in lifecycle: kata-flight's `queued, reviewed, resolving, refining,
    /// shipping`, after [`FILED`].
    pub fn kata_flight() -> Lifecycle {
        Lifecycle::parse(KATA_FLIGHT_TTL).expect("the built-in lifecycle parses")
    }

    /// Its name — the `{name}` of `urn:iki:ledger:lifecycle:{name}`.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// `urn:iki:ledger:lifecycle:{name}`.
    pub fn iri(&self) -> &str {
        &self.iri
    }

    /// What it is for, in a sentence, when the document said.
    pub fn summary(&self) -> Option<&str> {
        self.summary.as_deref()
    }

    /// Every state, in order — [`FILED`] first.
    pub fn states(&self) -> &[LifecycleState] {
        &self.states
    }

    /// The state by that name.
    pub fn state(&self, name: &str) -> Option<&LifecycleState> {
        self.states.iter().find(|s| s.name == name)
    }

    /// The state a stored `ledger:state` value names, when it is one of this lifecycle's.
    pub fn state_by_iri(&self, iri: &str) -> Option<&LifecycleState> {
        self.states.iter().find(|s| s.iri == iri)
    }

    /// The document as a graph, for the Turtle face.
    pub fn graph(&self) -> &Graph {
        &self.graph
    }

    /// One line per state: the plain face.
    pub fn plain(&self) -> String {
        let mut out = format!("{}\n", self.name);
        if let Some(summary) = &self.summary {
            out.push_str(&format!("  {summary}\n"));
        }
        let width = self.states.iter().map(|s| s.name.len()).max().unwrap_or(0);
        for state in &self.states {
            let role = match &state.drain {
                None => "in flight".to_string(),
                Some(drain) => format!("drain: {drain}"),
            };
            out.push_str(&format!(
                "  {:>2}. {:<width$}  {role}",
                state.order, state.name
            ));
            if state.name == FILED {
                out.push_str("  (no ledger:state)");
            }
            out.push('\n');
        }
        out
    }

    /// A sentence listing the legal state names, for a refusal.
    pub(crate) fn legal(&self) -> String {
        self.states
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// A lifecycle or state name: the shape a ledger name has (lowercase letters, digits, `-` and
/// `_`), because both end up as IRI segments a caller types.
fn check_name(name: &str, what: &str) -> Result<()> {
    let ok = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
    let shaped = !name.is_empty()
        && name.len() <= crate::ledger::MAX_NAME
        && name.chars().all(|c| ok(c) || c == '-' || c == '_')
        && name.starts_with(ok)
        && name.ends_with(ok);
    if shaped {
        Ok(())
    } else {
        Err(Error::InvalidArgument {
            name: "lifecycle".to_string(),
            detail: format!(
                "`{name}` is not a {what} name: lowercase letters, digits, `-` and `_`, starting \
                 and ending with a letter or a digit — it becomes an IRI segment"
            ),
        })
    }
}

/// The lifecycles a host offers, and which one each ledger uses.
///
/// The FIRST is every ledger's lifecycle unless [`assign`](Lifecycles::assign) names another
/// for it. The default is [`Lifecycle::kata_flight`] alone.
///
/// ★ **One lifecycle per ledger, fixed by the host, not chosen per request** — unlike an
/// ordering policy, which `next` takes as `policy=`. A policy only orders an answer; a
/// lifecycle decides which values a state may hold, and two lifecycles in one ledger would
/// let two writers disagree about what is legal.
#[derive(Debug, Clone)]
pub struct Lifecycles {
    all: Vec<Arc<Lifecycle>>,
    assigned: BTreeMap<String, String>,
}

impl Default for Lifecycles {
    fn default() -> Self {
        Lifecycles::new(vec![Lifecycle::kata_flight()])
    }
}

impl Lifecycles {
    /// These lifecycles; the first is the default.
    ///
    /// # Panics
    ///
    /// On an empty list, or two lifecycles with one name: both are boot-time configuration
    /// errors, and a ledger with no answer to "which states are legal" should fail where the
    /// manifest is, not at the first transition.
    pub fn new(lifecycles: Vec<Lifecycle>) -> Lifecycles {
        assert!(
            !lifecycles.is_empty(),
            "a ledger host needs at least one lifecycle"
        );
        let names: BTreeSet<&str> = lifecycles.iter().map(|l| l.name()).collect();
        assert_eq!(names.len(), lifecycles.len(), "two lifecycles share a name");
        Lifecycles {
            all: lifecycles.into_iter().map(Arc::new).collect(),
            assigned: BTreeMap::new(),
        }
    }

    /// Give the ledger `ledger` the lifecycle named `lifecycle` instead of the default.
    ///
    /// # Panics
    ///
    /// When `ledger` is not a legal ledger name or `lifecycle` is not one of these.
    pub fn assign(mut self, ledger: &str, lifecycle: &str) -> Lifecycles {
        let ledger = Ledger::parse(ledger).expect("a legal ledger name");
        assert!(
            self.get(lifecycle).is_some(),
            "no lifecycle `{lifecycle}` to assign"
        );
        self.assigned
            .insert(ledger.name().to_string(), lifecycle.to_string());
        self
    }

    /// The lifecycle by that name.
    pub fn get(&self, name: &str) -> Option<&Lifecycle> {
        self.all.iter().find(|l| l.name() == name).map(|l| &**l)
    }

    /// The lifecycle `ledger` uses.
    pub fn for_ledger(&self, ledger: &Ledger) -> &Lifecycle {
        self.assigned
            .get(ledger.name())
            .and_then(|name| self.get(name))
            .unwrap_or(&self.all[0])
    }

    /// Every lifecycle's name, the default first.
    pub fn names(&self) -> Vec<&str> {
        self.all.iter().map(|l| l.name()).collect()
    }

    /// Every state name any of these lifecycles declares, in first-seen order — the
    /// `one_of` the state Sink advertises. The ledger's own lifecycle is what it enforces.
    pub fn state_names(&self) -> Vec<&str> {
        let mut seen = Vec::new();
        for lifecycle in &self.all {
            for state in lifecycle.states() {
                if !seen.contains(&state.name.as_str()) {
                    seen.push(state.name.as_str());
                }
            }
        }
        seen
    }
}

/// The name a stored `ledger:state` value shows as: the state's own name for one of a
/// lifecycle's IRIs (`…:lifecycle:kata-flight:queued` → `queued`), the value itself otherwise
/// (a literal or a foreign IRI that some out-of-band write put there).
pub fn state_name(value: &str) -> String {
    match value.strip_prefix(PREFIX) {
        Some(rest) => rest
            .rsplit_once(':')
            .map(|(_, name)| name.to_string())
            .unwrap_or_else(|| value.to_string()),
        None => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"
        @prefix ledger: <https://ikigai-rs.dev/ns/ledger#> .
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
        <urn:iki:ledger:lifecycle:tiny> a ledger:Lifecycle ;
            ledger:hasState <urn:iki:ledger:lifecycle:tiny:filed>, <urn:iki:ledger:lifecycle:tiny:doing> .
        <urn:iki:ledger:lifecycle:tiny:filed> rdfs:label "filed" ; ledger:order 0 ;
            ledger:inFlight false ; ledger:drain "me" .
        <urn:iki:ledger:lifecycle:tiny:doing> rdfs:label "doing" ; ledger:order 1 ;
            ledger:inFlight true .
    "#;

    #[test]
    fn a_minimal_lifecycle_parses() {
        let tiny = Lifecycle::parse(MINIMAL).unwrap();
        assert_eq!(tiny.name(), "tiny");
        assert_eq!(tiny.states().len(), 2);
        assert!(tiny.state("doing").unwrap().in_flight);
    }

    #[test]
    fn every_rule_the_state_sink_relies_on_is_refused_at_load() {
        let refused = |from: &str, to: &str| {
            let text = MINIMAL.replace(from, to);
            Lifecycle::parse(&text).expect_err(&format!("accepted with `{to}`"))
        };
        // A state not named {lifecycle}:{name}.
        refused("rdfs:label \"doing\"", "rdfs:label \"done\"");
        // A non-flight state with no drain.
        refused("ledger:drain \"me\" .", ".");
        // An in-flight state with a drain.
        refused(
            "ledger:inFlight true .",
            "ledger:inFlight true ; ledger:drain \"x\" .",
        );
        // Two states with one order.
        refused("ledger:order 1", "ledger:order 0");
        // No `filed` first.
        refused(
            "rdfs:label \"filed\" ; ledger:order 0",
            "rdfs:label \"filed\" ; ledger:order 9",
        );
        // A name that is not an IRI segment a caller can type.
        refused("lifecycle:tiny", "lifecycle:Tiny");
    }

    #[test]
    fn a_stored_value_shows_as_its_state_name() {
        assert_eq!(
            state_name("urn:iki:ledger:lifecycle:kata-flight:queued"),
            "queued"
        );
        assert_eq!(state_name("queued"), "queued");
        assert_eq!(state_name("urn:other:x"), "urn:other:x");
    }

    #[test]
    fn a_ledger_uses_the_default_unless_assigned() {
        let tiny = Lifecycle::parse(MINIMAL).unwrap();
        let all = Lifecycles::new(vec![Lifecycle::kata_flight(), tiny]).assign("acme", "tiny");
        assert_eq!(all.for_ledger(&Ledger::default()).name(), "kata-flight");
        assert_eq!(
            all.for_ledger(&Ledger::parse("acme").unwrap()).name(),
            "tiny"
        );
        assert_eq!(all.state_names()[0], FILED);
        assert!(all.state_names().contains(&"doing"));
    }
}
