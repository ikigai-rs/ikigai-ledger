//! An importer's history (ledger #2843): `urn:iki:ledger:{ledger}:history`, a batch of past-dated
//! claim, release, edit and status events for one item, as Turtle.
//!
//! ```turtle
//! @prefix ledger: <https://ikigai-rs.dev/ns/ledger#> .
//! @prefix dcterms: <http://purl.org/dc/terms/> .
//! @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
//!
//! <urn:kata:event:01JZ> a ledger:EditEvent ;
//!     dcterms:created "2020-01-02T03:04:05Z"^^xsd:dateTime ;
//!     ledger:author <urn:example:github:user:octocat> ;
//!     ledger:editKind "label" ;
//!     ledger:editLabel "bug" .
//! ```
//!
//! An event's subject is its id at the source, which names it for good: it is kept as
//! `ledger:eventKey`, under the rules of `ledger:commentKey`, and the event is stored under a
//! minted IRI beneath its item, as every event is.
//!
//! # ★ History only
//!
//! An imported event is marked `ledger:createdSource "import"`, the mark a claimed date has
//! carried since 0.15.0, and it never touches the item: no write here names the item as a subject.
//! And what reads events to decide something does not count it:
//!
//! - `across::snapshot`, whose count of status and edit events is a mirror's version, filters it
//!   out, so importing neither moves a version nor rewrites a mirror;
//! - `counts::status_event_when`, whose count is the next `ledger:eventSequence`, filters it out,
//!   and `counts::preserve_close`, which asks whether a close already has its event;
//! - it carries no `ledger:eventSequence`, so the daily counts and the close `evidence` lands on
//!   (`evidence::standing`), which read status events through the sequence, never meet it.
//!
//! # ★ Validated whole, then written in one update
//!
//! The batch is held to the closed shapes below before anything is written, and one refused event
//! refuses the batch. Each shape names every predicate its event may carry, so a batch cannot write
//! `ledger:eventSequence`, `ledger:committer` or the item's own terms. What is parsed is bounded
//! first: its bytes, then its nesting. Whether a source id is taken is decided inside the update,
//! so concurrent re-sends record each event once.

use std::collections::{BTreeMap, HashSet};

use oxrdf::{NamedOrBlankNode, Term};

use super::*;

use crate::sparql::iso8601;

/// The most bytes of Turtle one batch may be, checked before it is parsed.
const MAX_BATCH_BYTES: usize = 512 << 10;

/// The most events one batch may hold. One batch is one store update, and an event's operation is
/// six of the store's 1024 algebra nodes (`ikigai_store::budget::MAX_ALGEBRA_NODES`), so 170 is the
/// most that could ever fit. The update's bytes are checked when it is made.
const MAX_BATCH_EVENTS: usize = 128;

/// What a predicate's value must be.
#[derive(Clone, Copy)]
enum Value {
    /// A plain string on one line, since a history face prints it in one.
    Text,
    /// Any IRI, stored as given and never fetched.
    Iri,
    /// An `xsd:dateTime`, under the date rules.
    Date,
    /// One of these IRIs.
    OneOfIris(&'static [&'static str]),
    /// One of these plain strings.
    OneOfTexts(&'static [&'static str]),
}

/// One predicate of a closed shape, and how many values it takes.
#[derive(Clone, Copy)]
struct Slot {
    predicate: &'static str,
    value: Value,
    min: usize,
    max: usize,
}

const fn slot(predicate: &'static str, value: Value, min: usize, max: usize) -> Slot {
    Slot {
        predicate,
        value,
        min,
        max,
    }
}

const CLAIM_KINDS: &[&str] = &[v::MACHINE, v::AGENT, v::PERSON];
const RELEASE_REASONS: &[&str] = &["released", "closed", "takeover"];
const STATUSES: &[&str] = &[v::OPEN, v::CLOSED];
const CLOSE_REASONS: &[&str] = &[
    v::CLOSE_REASONS[0].1,
    v::CLOSE_REASONS[1].1,
    v::CLOSE_REASONS[2].1,
    v::CLOSE_REASONS[3].1,
    v::CLOSE_REASONS[4].1,
];
const LINK_TYPES: &[&str] = &[v::LINK_TYPES[0].1, v::LINK_TYPES[1].1, v::LINK_TYPES[2].1];
const EDIT_KINDS: &[&str] = &[
    "label", "unlabel", "state", "link", "unlink", "about", "unabout", "edit", "defer", "resume",
];
const EDIT_FIELDS: &[&str] = &["title", "body", "priority", "revision", "about"];

/// Every event's own terms: when, and who at the source.
const CREATED: Slot = slot(v::ext::CREATED, Value::Date, 1, 1);
const AUTHOR: Slot = slot(v::AUTHOR, Value::Iri, 0, 1);
const STATUS: Slot = slot(v::STATUS, Value::OneOfIris(STATUSES), 1, 1);
const EDIT_KIND: Slot = slot(v::EDIT_KIND, Value::OneOfTexts(EDIT_KINDS), 1, 1);

const CLAIM_SHAPE: &[Slot] = &[
    CREATED,
    AUTHOR,
    slot(v::CLAIMED_BY, Value::Text, 1, 1),
    slot(v::CLAIM_PRINCIPAL, Value::Iri, 0, 1),
    slot(v::CLAIM_KIND, Value::OneOfIris(CLAIM_KINDS), 0, 1),
    slot(v::TAKEOVER_FROM, Value::Text, 0, 1),
];
const RELEASE_SHAPE: &[Slot] = &[
    CREATED,
    AUTHOR,
    slot(v::CLAIMED_BY, Value::Text, 1, 1),
    slot(v::RELEASE_REASON, Value::OneOfTexts(RELEASE_REASONS), 1, 1),
    slot(v::CLAIM_PRINCIPAL, Value::Iri, 0, 1),
    slot(v::CLAIM_KIND, Value::OneOfIris(CLAIM_KINDS), 0, 1),
    slot(v::CLAIMED_AT, Value::Date, 0, 1),
];
const REOPEN_SHAPE: &[Slot] = &[CREATED, AUTHOR, STATUS];
const CLOSE_SHAPE: &[Slot] = &[
    CREATED,
    AUTHOR,
    STATUS,
    slot(v::CLOSED_REASON, Value::OneOfIris(CLOSE_REASONS), 0, 1),
    slot(v::EVIDENCE, Value::Iri, 0, usize::MAX),
];
const LABEL_SHAPE: &[Slot] = &[
    CREATED,
    AUTHOR,
    EDIT_KIND,
    slot(v::EDIT_LABEL, Value::Text, 1, 1),
];
const STATE_SHAPE: &[Slot] = &[
    CREATED,
    AUTHOR,
    EDIT_KIND,
    slot(v::STATE_FROM, Value::Iri, 1, 1),
    slot(v::STATE_TO, Value::Iri, 1, 1),
];
const LINK_SHAPE: &[Slot] = &[
    CREATED,
    AUTHOR,
    EDIT_KIND,
    slot(v::EDIT_LINK_TYPE, Value::OneOfIris(LINK_TYPES), 1, 1),
    slot(v::EDIT_TARGET, Value::Iri, 1, 1),
];
const ABOUT_SHAPE: &[Slot] = &[
    CREATED,
    AUTHOR,
    EDIT_KIND,
    slot(v::EDIT_TARGET, Value::Iri, 1, usize::MAX),
];
const FIELDS_SHAPE: &[Slot] = &[
    CREATED,
    AUTHOR,
    EDIT_KIND,
    slot(v::EDIT_FIELD, Value::OneOfTexts(EDIT_FIELDS), 1, usize::MAX),
];
const DEFER_SHAPE: &[Slot] = &[CREATED, AUTHOR, EDIT_KIND];

/// One event as the batch says it: its predicates and their values.
type Told = BTreeMap<String, Vec<Term>>;

/// One event, valid, as the rows of the update that writes it.
struct Event {
    key: String,
    /// Its triples, as an update's template.
    triples: String,
}

/// A refusal of the batch, naming the event.
fn refused(source: &str, detail: String) -> Error {
    Error::InvalidArgument {
        name: "content".to_string(),
        detail: format!("event `{source}`: {detail}. Nothing was written"),
    }
}

/// A refusal of the batch as a whole.
fn refused_batch(detail: String) -> Error {
    Error::InvalidArgument {
        name: "content".to_string(),
        detail: format!("{detail}. Nothing was written"),
    }
}

/// The batch's events, in the order the Turtle first names them, with nothing checked but that
/// each subject is an IRI.
fn parse(content: &str) -> Result<Vec<(String, Told)>> {
    if content.len() > MAX_BATCH_BYTES {
        return Err(refused_batch(format!(
            "this batch is {} bytes, and a batch is at most {MAX_BATCH_BYTES}. Send the item's \
             events in smaller batches: a re-sent event changes nothing, so a batch can be \
             repeated safely",
            content.len()
        )));
    }
    ikigai_store::depth::check_rdf_nesting(
        content.as_bytes(),
        oxrdfio::RdfFormat::Turtle,
        "content",
    )?;
    let mut order: Vec<(String, Told)> = Vec::new();
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for quad in
        oxrdfio::RdfParser::from_format(oxrdfio::RdfFormat::Turtle).for_slice(content.as_bytes())
    {
        let quad = quad.map_err(|e| {
            refused_batch(format!(
                "this batch is not Turtle, or not Turtle this door reads ({e})"
            ))
        })?;
        let NamedOrBlankNode::NamedNode(subject) = &quad.subject else {
            return Err(refused_batch(
                "an event's subject is a blank node, and it must be the event's id at its \
                 source, an IRI, or a re-send could not be told from a new event"
                    .to_string(),
            ));
        };
        let at = *seen.entry(subject.as_str().to_string()).or_insert_with(|| {
            order.push((subject.as_str().to_string(), Told::new()));
            order.len() - 1
        });
        let told = &mut order[at].1;
        told.entry(quad.predicate.as_str().to_string())
            .or_default()
            .push(quad.object);
    }
    if order.is_empty() {
        return Err(refused_batch("this batch holds no events".to_string()));
    }
    if order.len() > MAX_BATCH_EVENTS {
        return Err(refused_batch(format!(
            "this batch holds {} events, and a batch is at most {MAX_BATCH_EVENTS}. Send the \
             item's events in smaller batches: a re-sent event changes nothing, so a batch can be \
             repeated safely",
            order.len()
        )));
    }
    Ok(order)
}

/// The one plain string in `told[predicate]`, if there is exactly that.
fn only_text<'t>(told: &'t Told, predicate: &str) -> Option<&'t str> {
    match told.get(predicate)?.as_slice() {
        [Term::Literal(text)] => Some(text.value()),
        _ => None,
    }
}

/// The class's segment and the closed shape this event must fit, which an edit's kind and a
/// status event's status narrow.
fn shape_of(source: &str, told: &Told) -> Result<(&'static str, &'static str, &'static [Slot])> {
    let class = match told.get(v::ext::TYPE).map(Vec::as_slice) {
        Some([Term::NamedNode(class)]) => class.as_str().to_string(),
        _ => String::new(),
    };
    match class.as_str() {
        v::CLAIM_EVENT_CLASS => Ok((v::CLAIM_EVENT_CLASS, "claim", CLAIM_SHAPE)),
        v::RELEASE_EVENT_CLASS => Ok((v::RELEASE_EVENT_CLASS, "release", RELEASE_SHAPE)),
        v::STATUS_EVENT_CLASS => {
            let closed = matches!(
                told.get(v::STATUS).map(Vec::as_slice),
                Some([Term::NamedNode(status)]) if status.as_str() == v::CLOSED
            );
            let shape = if closed { CLOSE_SHAPE } else { REOPEN_SHAPE };
            Ok((v::STATUS_EVENT_CLASS, "status", shape))
        }
        v::EDIT_EVENT_CLASS => {
            let shape = match only_text(told, v::EDIT_KIND).unwrap_or_default() {
                "label" | "unlabel" => LABEL_SHAPE,
                "state" => STATE_SHAPE,
                "link" | "unlink" => LINK_SHAPE,
                "about" | "unabout" => ABOUT_SHAPE,
                "edit" => FIELDS_SHAPE,
                _ => DEFER_SHAPE,
            };
            Ok((v::EDIT_EVENT_CLASS, "edit", shape))
        }
        _ => Err(refused(
            source,
            format!(
                "it has one class, and it is one of ledger:ClaimEvent, ledger:ReleaseEvent, \
                 ledger:EditEvent and ledger:StatusEvent; this one says {}",
                match told.get(v::ext::TYPE) {
                    Some(classes) => classes
                        .iter()
                        .map(Term::to_string)
                        .collect::<Vec<_>>()
                        .join(", "),
                    None => "nothing".to_string(),
                }
            ),
        )),
    }
}

/// `value` as `slot` takes it, as a term of the update.
fn checked(
    source: &str,
    predicate: &str,
    slot: Value,
    value: &Term,
    filed: u64,
    now: u64,
) -> Result<String> {
    let wrong = |wanted: String| {
        refused(
            source,
            format!("`{predicate}` is {wanted}, and this one is {value}"),
        )
    };
    let text = || match value {
        Term::Literal(text) if text.datatype().as_str() == v::ext::XSD_STRING => Some(text.value()),
        _ => None,
    };
    let named = || match value {
        Term::NamedNode(iri) => Some(iri.as_str()),
        _ => None,
    };
    match slot {
        Value::Text => {
            let text = text().ok_or_else(|| wrong("a plain string".to_string()))?;
            if text.trim().is_empty() || text.chars().any(char::is_control) {
                return Err(wrong(
                    "a string with words in it on one line, since a history prints it in one"
                        .to_string(),
                ));
            }
            Ok(literal(text))
        }
        Value::Iri => {
            let iri = named().ok_or_else(|| wrong("an IRI".to_string()))?;
            sparql::iri(iri, predicate)
        }
        Value::OneOfIris(allowed) => {
            let iri = named().filter(|iri| allowed.contains(iri)).ok_or_else(|| {
                wrong(format!(
                    "one of {}",
                    allowed
                        .iter()
                        .map(|iri| format!("<{iri}>"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })?;
            sparql::iri(iri, predicate)
        }
        Value::OneOfTexts(allowed) => {
            let text = text()
                .filter(|text| allowed.contains(text))
                .ok_or_else(|| wrong(format!("one of {}", allowed.join(", "))))?;
            Ok(literal(text))
        }
        Value::Date => {
            let Term::Literal(date) = value else {
                return Err(wrong("an xsd:dateTime".to_string()));
            };
            let at = (date.datatype().as_str() == v::ext::XSD_DATETIME)
                .then(|| model::millis(date.value()))
                .flatten()
                .ok_or_else(|| {
                    wrong(
                        "an xsd:dateTime with a time and a zone, written \
                         \"2026-11-01T18:30:00Z\"^^xsd:dateTime"
                            .to_string(),
                    )
                })?;
            if at > now {
                return Err(refused(
                    source,
                    format!(
                        "`{}` is after this write ({}); an event imported from its source \
                         cannot be later than its import",
                        date.value(),
                        iso8601(now)
                    ),
                ));
            }
            if at < filed {
                return Err(refused(
                    source,
                    format!(
                        "`{}` is before the item was filed ({}); nothing happens to an item \
                         before it was filed. Send the item's source dates when filing it, or a \
                         later date",
                        date.value(),
                        iso8601(filed)
                    ),
                ));
            }
            Ok(datetime(at))
        }
    }
}

/// Hold one event to its closed shape, and make its rows.
fn validate(item: &Item, source: &str, told: &Told, who: &str, now: u64) -> Result<Event> {
    let key = model::parse_key(source).map_err(|e| match e {
        Error::InvalidArgument { detail, .. } => refused(source, detail),
        other => other,
    })?;
    let (class, segment, shape) = shape_of(source, told)?;
    for predicate in told.keys() {
        let known = predicate == v::ext::TYPE || shape.iter().any(|s| s.predicate == predicate);
        if !known {
            return Err(refused(
                source,
                format!(
                    "`{predicate}` is not a term this event takes; it takes {}. Its item, its \
                     committer and its marks are the ledger's to write",
                    shape
                        .iter()
                        .map(|s| s.predicate)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }
    }
    let event = event_iri(&item.iri, segment, now)?;
    let mut rows: Vec<(&'static str, String)> = vec![
        (v::ext::TYPE, sparql::iri(class, "class")?),
        (v::ON_ITEM, sparql::iri(&item.iri, "item")?),
        (v::CREATED_SOURCE, literal(attribution::IMPORT)),
        (v::COMMITTED_AT, datetime(now)),
        (v::COMMITTER, sparql::iri(who, "principal")?),
        (v::EVENT_KEY, literal(&key)),
    ];
    for slot in shape {
        let values = told.get(slot.predicate).map(Vec::as_slice).unwrap_or(&[]);
        if values.len() < slot.min || values.len() > slot.max {
            return Err(refused(
                source,
                format!(
                    "it needs {} `{}`, and it has {}",
                    match (slot.min, slot.max) {
                        (1, 1) => "exactly one".to_string(),
                        (0, 1) => "at most one".to_string(),
                        _ => "at least one".to_string(),
                    },
                    slot.predicate,
                    values.len()
                ),
            ));
        }
        for value in values {
            let term = checked(source, slot.predicate, slot.value, value, item.created, now)?;
            rows.push((slot.predicate, term));
        }
        if slot.predicate == v::AUTHOR && !values.is_empty() {
            rows.push((v::AUTHOR_SOURCE, literal(attribution::IMPORT)));
        }
    }
    Ok(Event {
        key,
        triples: rows
            .into_iter()
            .map(|(predicate, term)| format!("<{event}> <{predicate}> {term} .\n"))
            .collect(),
    })
}

/// Which of `keys` an event in this ledger already holds.
async fn taken(client: &StoreClient<'_, '_>, keys: &[&str]) -> Result<HashSet<String>> {
    let values: String = keys.iter().map(|key| literal(key) + " ").collect();
    let rows = client
        .select(&format!(
            "SELECT DISTINCT ?key WHERE {{ {} }}",
            client.in_graph(&format!(
                "VALUES ?key {{ {values} }} ?event <{}> ?key .",
                v::EVENT_KEY
            ))
        ))
        .await?;
    Ok(rows
        .iter()
        .filter_map(|row| row.get("key").map(|key| key.value.clone()))
        .collect())
}

/// The one update that records `events` on `item`: an operation per event, since a store update
/// is one transaction. An event whose source id is taken by then adds nothing, and neither does
/// anything once the item is gone.
///
/// ⚠ Not one `INSERT` over a many-columned `VALUES`: the store's nesting scan cannot read `<` inside
/// parentheses when an IRI holds a `#`, as every `ledger:` term does, and refuses the update.
fn update(client: &StoreClient<'_, '_>, item: &str, events: &[Event]) -> String {
    events
        .iter()
        .map(|event| {
            format!(
                "INSERT {{ {} }} WHERE {{ {} }}",
                client.in_graph(&event.triples),
                client.in_graph(&format!(
                    "{} FILTER NOT EXISTS {{ ?taken <{}> {} }}",
                    live(item),
                    v::EVENT_KEY,
                    literal(&event.key)
                ))
            )
        })
        .collect::<Vec<_>>()
        .join(" ;\n")
}

/// `urn:iki:ledger:{ledger}:history`.
#[derive(Clone)]
pub(super) struct ImportEndpoint {
    pub(super) participants: Participants,
}

#[async_trait]
impl Endpoint for ImportEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        if inv.request.verb != Verb::Sink {
            return Err(unsupported("ledger-history", inv.request.verb));
        }
        let client = StoreClient::new(inv, ledger_for(inv, Need::Write)?);
        let want = wanted_face(inv, &WRITE_FACES)?;
        let reference = inv.inline_str("item")?;
        let content = inv.inline_str("content")?;
        let now = now_ms(inv)?;
        // Before the batch is parsed, so only an import principal spends the parser.
        let who = attribution::require_importer(inv, &client, self.participants.as_deref()).await?;
        let told = parse(content)?;
        let item = require_item(&client, reference).await?;
        let events = told
            .iter()
            .map(|(source, told)| validate(&item, source, told, &who, now))
            .collect::<Result<Vec<Event>>>()?;
        let keys: Vec<&str> = events.iter().map(|event| event.key.as_str()).collect();
        // A re-send is answered without a write, as a taken key is: even a no-op update cuts every
        // cached read of the ledger. The update's own guard is what holds against a racer.
        let held = taken(&client, &keys).await?;
        let new: Vec<Event> = events
            .into_iter()
            .filter(|event| !held.contains(&event.key))
            .collect();
        if !new.is_empty() {
            let update = update(&client, &item.iri, &new);
            if update.len() > ikigai_store::limits::MAX_SPARQL_BYTES {
                return Err(refused_batch(format!(
                    "these {} events make a store update of {} bytes, and the store takes at \
                     most {}. Send the item's events in smaller batches: a re-sent event changes \
                     nothing, so a batch can be repeated safely",
                    new.len(),
                    update.len(),
                    ikigai_store::limits::MAX_SPARQL_BYTES
                )));
            }
            guarded_update(&client, &[&item.iri], &update).await?;
        }
        let (recorded, existing) = (new.len(), held.len());
        if want == JSON {
            let ledger = client.ledger();
            let mut answer = json::answer(
                ledger,
                if recorded > 0 { "imported" } else { "existing" },
                json::item_ref(ledger, item.number, &item.iri),
            );
            answer.recorded = Some(recorded);
            answer.already_recorded = Some(existing);
            return Ok(json_repr(json::render(&answer)?));
        }
        Ok(plain(format!(
            "{}: imported events recorded {recorded}, already recorded {existing}; history only, \
             so the item itself did not change\n",
            item.short()
        )))
    }

    fn name(&self) -> &str {
        "ledger-history"
    }

    fn describe(&self) -> Description {
        Description::new("ledger-history")
            .title("Record an item's past history")
            .summary(
                "For an import or sync principal (ledger:Importer in the host's participants \
                 graph) only: record claim, release, edit and status events an item had at its \
                 source, with their source dates. History only: an imported event never changes \
                 the item's state, labels, claim or status, and a re-sent one changes nothing. \
                 Anyone else writes the change itself. Read them at `item:{id}:history`.",
            )
            .verb(Verb::Meta)
            .action(write_scopes(
                ikigai_core::ActionSpec::new(Verb::Sink)
                    // Every accepted event has a stable source ID in content.
                    .resend_safe()
                    .input(ledger_arg("history"))
                    .summary("Record a batch of past-dated events on one item, or none of them.")
                    .input(item_arg("The item the events happened to."))
                    .input(
                        ArgSpec::new("content")
                            .summary(
                                "The events as Turtle, at most 128 and 512 KiB: each a subject \
                                 that is its id at the source, one of the classes \
                                 ledger:ClaimEvent, ledger:ReleaseEvent, ledger:EditEvent and \
                                 ledger:StatusEvent, a dcterms:created no later than now and no \
                                 earlier than the item's filing, an optional ledger:author, and \
                                 the terms of that kind of event. Any other term is refused.",
                            )
                            .class(v::ext::XSD_STRING),
                    )
                    .input(as_arg(&WRITE_FACES))
                    .output(PLAIN)
                    .output(JSON),
                CAP_WRITE,
            ))
    }
}
