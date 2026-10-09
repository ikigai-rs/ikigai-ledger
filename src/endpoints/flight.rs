//! The flight seams (ledger #775): an item's **lifecycle state** as a compare-and-set atom,
//! the **lifecycles** that say which states are legal, and the **assertions** a loop checks
//! after each step.
//!
//! ```text
//! urn:iki:ledger:lifecycle:{name}                      Source Exists  the legal states, in order
//! urn:iki:ledger:{ledger}:item:{id}:state              Source Sink    the one state; a CAS transition
//! urn:iki:ledger:{ledger}:item:{id}:state:{value}      Exists         is it in that state?
//! urn:iki:ledger:{ledger}:item:{id}:holder:{holder}    Exists         is it held by that holder?
//! urn:iki:ledger:{ledger}:item:{id}:closed             Exists         is it closed?
//! ```
//!
//! # ★ A transition is ONE store update
//!
//! kata-flight keeps state in `lifecycle:*` labels and moves an item by removing one label
//! and adding the next — two writes, so a crash or a race between them leaves two labels,
//! which its reaper then hunts for. This crate has label add and label remove as separate
//! Sinks too, and composing a transition out of them would reproduce that defect exactly
//! (`tests/lifecycle_state.rs` reproduces it). So the state Sink is its own resource and is a
//! **compare-and-set inside a single SPARQL update**: the `WHERE` holds only when the item is
//! in `from=`, and with no solution the `DELETE … INSERT` does nothing. The store holds its
//! write lock across one update's evaluation and insert, so two transitions from one state
//! cannot both apply.
//!
//! The update cannot say whether it applied, so it writes a **receipt** — a `ledger:stateToken`
//! minted for this transition — and reads it back: the token is ours, or another transition
//! won and the answer is a typed [`Error::Conflict`] naming the state the item is in now. A
//! state read alone could not tell two racers apart when both asked for the same `to=`.
//!
//! # The assertions are names, and Exists answers them
//!
//! A loop that has just claimed an item, moved it, or closed it asks `exists` on the name of
//! what should now be true, and stops the wave on `false` — so a skipped step is a failed
//! check rather than a narrated success (the contract's verification over narration, ledger
//! #808). Each is a pure read of the item, cached under the store's write threads like every
//! other read here, so a write to the ledger invalidates it and an unchanged ledger answers
//! from cache.

use super::*;

use crate::lifecycle::{self, Lifecycle, LifecycleState, Lifecycles, FILED};
use crate::sparql::iso8601;

/// The faces of the state resource: the state's name for a person or a script, the
/// [`json::StateDocument`] for a machine.
const STATE_FACES: [&str; 2] = [PLAIN, JSON];

/// The `id` binding every item part declares.
fn item_id_input() -> ArgSpec {
    ArgSpec::new("id")
        .summary(
            "The item's number (`12`) or its opaque id. Not `key:{key}`: a key may contain `:`, \
             so it cannot be split from what follows it — reach a keyed item's parts through \
             its number or its IRI.",
        )
        .class(v::ext::XSD_STRING)
        .binding()
}

/// The item an item part names, from the grammar's `id` capture.
fn part_id<'a>(inv: &'a Invocation<'_>) -> Result<&'a str> {
    inv.bindings.get("id").ok_or_else(|| {
        Error::Endpoint(
            "no `id` captured: this endpoint is bound to `urn:iki:ledger:{ledger}:item:{id}:…` \
             and was invoked without the grammar's capture"
                .to_string(),
        )
    })
}

/// What `ledger:state` holds for one item, read on its own.
struct Stored {
    /// Every `ledger:state` value, as stored.
    values: Vec<sparql::Binding>,
    /// `ledger:stateAt`, when readable.
    at: Option<u64>,
    /// `ledger:stateToken` — the receipt of the transition that set the state.
    tokens: Vec<String>,
}

async fn read_state(client: &StoreClient<'_, '_>, item: &str) -> Result<Stored> {
    let query = format!(
        "SELECT ?state ?at ?token WHERE {{ {} }}",
        client.in_graph(&format!(
            "BIND({subject} AS ?item)\n\
             OPTIONAL {{ ?item <{state}> ?state }}\n\
             OPTIONAL {{ ?item <{at}> ?at }}\n\
             OPTIONAL {{ ?item <{token}> ?token }}",
            subject = sparql::iri(item, "item")?,
            state = v::STATE,
            at = v::STATE_AT,
            token = v::STATE_TOKEN,
        ))
    );
    let mut stored = Stored {
        values: Vec::new(),
        at: None,
        tokens: Vec::new(),
    };
    for row in client.select(&query).await? {
        if let Some(value) = row.get("state") {
            if !stored.values.contains(value) {
                stored.values.push(value.clone());
            }
        }
        if let Some(at) = row.get("at").and_then(|b| model::millis(&b.value)) {
            stored.at = Some(stored.at.map_or(at, |was: u64| was.max(at)));
        }
        if let Some(token) = row.get("token") {
            if !stored.tokens.contains(&token.value) {
                stored.tokens.push(token.value.clone());
            }
        }
    }
    stored.values.sort_by(|a, b| a.value.cmp(&b.value));
    Ok(stored)
}

/// The state an item is in, against its ledger's lifecycle — or why it cannot be said.
///
/// ⚠ Only an out-of-band write reaches the error: two `ledger:state` values, or one this
/// lifecycle does not declare (a hand edit, an import, a ledger whose lifecycle changed).
fn current<'l>(
    lifecycle: &'l Lifecycle,
    item: &Item,
    values: &[sparql::Binding],
) -> std::result::Result<&'l LifecycleState, String> {
    match values {
        [] => Ok(lifecycle
            .state(FILED)
            .expect("every lifecycle declares `filed`")),
        [one] if one.kind == "uri" => lifecycle.state_by_iri(&one.value).ok_or_else(|| {
            format!(
                "{} holds the state `{}`, which the lifecycle `{}` does not declare",
                item.short(),
                one.value,
                lifecycle.name()
            )
        }),
        [one] => Err(format!(
            "{} holds the state {:?} as a literal; a state is one of the lifecycle `{}`'s IRIs",
            item.short(),
            one.value,
            lifecycle.name()
        )),
        several => Err(format!(
            "{} holds {} states ({}). No ledger write produces that — a transition is one \
             compare-and-set — so an out-of-band write did",
            item.short(),
            several.len(),
            several
                .iter()
                .map(|b| format!("`{}`", lifecycle::state_name(&b.value)))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// The refusal for an item that is not where a transition expected it.
fn conflict(client: &StoreClient<'_, '_>, item: &Item, expected: &str, found: &str) -> Error {
    Error::Conflict(format!(
        "{} is in `{found}`, not `{expected}`. A transition is a compare-and-set: it names the \
         state it leaves, and another writer moved this item first. Re-read `{}` and \
         transition from there",
        item.short(),
        client
            .ledger()
            .resource(&format!("item:{}:state", item.number))
    ))
}

/// The compare-and-set, as ONE update: the `WHERE` holds only while the item is in exactly
/// `from`, and then the old state, receipt, stamp and `dcterms:modified` are replaced in the
/// same operation.
///
/// ⚠ `dcterms:modified` is in this update rather than a second operation of the batch: an
/// operation after a failed guard still runs, so a separate `touch` would stamp an item that
/// a lost race never changed.
fn cas(
    client: &StoreClient<'_, '_>,
    item: &str,
    from: &LifecycleState,
    to: &LifecycleState,
    token: &str,
    now: u64,
) -> String {
    let guard = if from.name == FILED {
        format!("FILTER NOT EXISTS {{ <{item}> <{}> ?any }}", v::STATE)
    } else {
        format!(
            "<{item}> <{state}> <{from}> .\n\
             FILTER NOT EXISTS {{ <{item}> <{state}> ?other . FILTER(?other != <{from}>) }}",
            state = v::STATE,
            from = from.iri,
        )
    };
    let mut insert = format!(
        "<{item}> <{token_p}> {token} ; <{at}> {when} ; <{modified}> {when} .",
        token_p = v::STATE_TOKEN,
        token = literal(token),
        at = v::STATE_AT,
        modified = v::ext::MODIFIED,
        when = datetime(now),
    );
    if to.name != FILED {
        insert.push_str(&format!("\n<{item}> <{}> <{}> .", v::STATE, to.iri));
    }
    format!(
        "DELETE {{ {delete} }}\nINSERT {{ {insert} }}\nWHERE {{ {where_} }}",
        delete = client.in_graph(&format!(
            "<{item}> <{state}> ?s . <{item}> <{token}> ?t . <{item}> <{at}> ?a . \
             <{item}> <{modified}> ?m .",
            state = v::STATE,
            token = v::STATE_TOKEN,
            at = v::STATE_AT,
            modified = v::ext::MODIFIED,
        )),
        insert = client.in_graph(&insert),
        where_ = client.in_graph(&format!(
            "<{item}> <{type_}> <{class}> .\n{guard}\n\
             OPTIONAL {{ <{item}> <{state}> ?s }}\n\
             OPTIONAL {{ <{item}> <{token}> ?t }}\n\
             OPTIONAL {{ <{item}> <{at}> ?a }}\n\
             OPTIONAL {{ <{item}> <{modified}> ?m }}",
            type_ = v::ext::TYPE,
            class = v::ITEM_CLASS,
            state = v::STATE,
            token = v::STATE_TOKEN,
            at = v::STATE_AT,
            modified = v::ext::MODIFIED,
        )),
    )
}

/// A state name a caller wrote, checked against the ledger's lifecycle.
fn named_state<'l>(lifecycle: &'l Lifecycle, arg: &str, name: &str) -> Result<&'l LifecycleState> {
    lifecycle.state(name).ok_or_else(|| Error::InvalidArgument {
        name: arg.to_string(),
        detail: format!(
            "`{name}` is not a state of the lifecycle `{}` this ledger uses; one of {} \
             (see `{}`)",
            lifecycle.name(),
            lifecycle.legal(),
            lifecycle.iri()
        ),
    })
}

/// The state document, for a read or a transition's answer.
fn state_document(
    client: &StoreClient<'_, '_>,
    item: &Item,
    lifecycle: &Lifecycle,
    state: &LifecycleState,
    since: Option<u64>,
) -> json::StateDocument {
    let ledger = client.ledger();
    json::StateDocument {
        schema: json::SCHEMA,
        ledger: ledger.name().to_string(),
        item: json::item_ref(ledger, item.number, &item.iri),
        lifecycle: lifecycle.name().to_string(),
        state: state.name.clone(),
        in_flight: state.in_flight,
        drain: state.drain.clone(),
        since: since.map(iso8601),
        outcome: None,
        from: None,
    }
}

// --------------------------------------------------------------------------- state

/// `urn:iki:ledger:{ledger}:item:{id}:state` — the one state an item is in, and the
/// compare-and-set that moves it.
#[derive(Clone)]
pub(super) struct StateEndpoint {
    pub(super) lifecycles: Arc<Lifecycles>,
}

#[async_trait]
impl Endpoint for StateEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let need = match inv.request.verb {
            Verb::Source => Need::Read,
            Verb::Sink => Need::Write,
            other => return Err(unsupported("ledger-item-state", other)),
        };
        let client = StoreClient::new(inv, ledger_for(inv, need)?);
        let lifecycle = self.lifecycles.for_ledger(client.ledger());
        let want = wanted_face(inv, &STATE_FACES)?;
        let id = part_id(inv)?;
        match inv.request.verb {
            Verb::Source => {
                let item = require_item(&client, id).await?;
                let stored = read_state(&client, &item.iri).await?;
                let state = current(lifecycle, &item, &stored.values).map_err(|why| {
                    Error::Endpoint(format!(
                        "{why}. The item's state cannot be answered until it is repaired, which \
                         is one SPARQL UPDATE over this ledger's graph"
                    ))
                })?;
                if want == JSON {
                    let doc = state_document(&client, &item, lifecycle, state, stored.at);
                    return Ok(json_face(json::render(&doc)?));
                }
                face(format!("{}\n", state.name), None, PLAIN)
            }
            Verb::Sink => {
                // Every argument is checked before anything is read or written.
                let to = named_state(lifecycle, "to", inv.inline_str("to")?)?;
                let from = named_state(lifecycle, "from", inv.inline_str("from")?)?;
                let now = now_ms(inv)?;
                let item = require_item(&client, id).await?;

                // The cheap half first: an item already somewhere else is refused without a
                // write, because a no-op update still cuts every cached read of the ledger.
                // This read proves nothing about the update below — the update's own guard
                // is what holds against a racer.
                let before = read_state(&client, &item.iri).await?;
                match current(lifecycle, &item, &before.values) {
                    Ok(state) if state.name == from.name => {}
                    Ok(state) => return Err(conflict(&client, &item, &from.name, &state.name)),
                    Err(why) => {
                        return Err(Error::Conflict(format!(
                            "{why}, so it is not in `{}` and no transition can name the state \
                             it leaves. Repair the item with one SPARQL UPDATE over this \
                             ledger's graph",
                            from.name
                        )))
                    }
                }

                let outcome = if to.name == from.name {
                    // Nothing to move: answered without a write, for the reason above.
                    "unchanged"
                } else {
                    let token = mint_id(now);
                    client
                        .update(&cas(&client, &item.iri, from, to, &token, now))
                        .await?;
                    let after = read_state(&client, &item.iri).await?;
                    if after.tokens != [token.clone()] {
                        let found = current(lifecycle, &item, &after.values)
                            .map(|s| s.name.clone())
                            .unwrap_or_else(|why| why);
                        return Err(conflict(&client, &item, &from.name, &found));
                    }
                    "transitioned"
                };

                if let Ok(note) = inv.inline_str("content") {
                    if !note.trim().is_empty() && outcome == "transitioned" {
                        let author = inv.inline_str("author").ok();
                        write_comment(&client, &item.iri, note.trim(), author, now).await?;
                    }
                }

                if want == JSON {
                    let since = if outcome == "unchanged" {
                        before.at
                    } else {
                        Some(now)
                    };
                    let mut doc = state_document(&client, &item, lifecycle, to, since);
                    doc.outcome = Some(outcome.to_string());
                    doc.from = Some(from.name.clone());
                    return Ok(json_repr(json::render(&doc)?));
                }
                Ok(plain(if outcome == "unchanged" {
                    format!("{} {} (unchanged)\n", item.short(), to.name)
                } else {
                    format!("{} {} (was {})\n", item.short(), to.name, from.name)
                }))
            }
            other => Err(unsupported("ledger-item-state", other)),
        }
    }

    fn name(&self) -> &str {
        "ledger-item-state"
    }

    fn describe(&self) -> Description {
        let names = self.lifecycles.state_names();
        let state_arg = |name: &'static str, summary: &str| {
            ArgSpec::new(name)
                .summary(format!(
                    "{summary} A state of the ledger's lifecycle (`urn:iki:ledger:lifecycle:{{name}}`); \
                     `{FILED}` is the state of an item that holds none."
                ))
                .class(v::ext::XSD_STRING)
                .one_of(names.iter().copied())
        };
        Description::new("ledger-item-state")
            .title("An item's lifecycle state")
            .summary(
                "The ONE lifecycle state an item is in (`filed` when it holds none), and the \
                 transition that moves it: a compare-and-set in a single store update, so two \
                 writers moving one item cannot both succeed and an interrupted transition \
                 leaves the old state or the new one, never both.",
            )
            .verb(Verb::Meta)
            .action(read_action(
                ikigai_core::ActionSpec::new(Verb::Source)
                    .input(ledger_arg())
                    .summary(
                        "The state's name in text; in JSON, also the lifecycle, whether the \
                         state is in flight, its drain, and when the item entered it.",
                    )
                    .input(item_id_input())
                    .input(as_arg(&STATE_FACES))
                    .output(PLAIN)
                    .output(JSON),
            ))
            .action(write_scopes(
                ikigai_core::ActionSpec::new(Verb::Sink)
                    .input(ledger_arg())
                    .summary(
                        "Move the item from `from` to `to` — only if it is in `from` now. A \
                         mismatch is a Conflict naming the state it is in; a `to` outside the \
                         lifecycle is refused before anything is written.",
                    )
                    .input(item_id_input())
                    .input(state_arg("to", "The state to move the item to."))
                    .input(state_arg(
                        "from",
                        "The state the item must be in now — the compare half of the \
                         compare-and-set.",
                    ))
                    .input(
                        ArgSpec::new("content")
                            .summary(
                                "An optional note, recorded as a comment when the item moves — \
                                 where a pipe's value lands.",
                            )
                            .class(v::ext::XSD_STRING)
                            .optional(),
                    )
                    .input(
                        ArgSpec::new("author")
                            .summary("Who wrote the note.")
                            .class(v::ext::XSD_STRING)
                            .optional(),
                    )
                    .input(as_arg(&STATE_FACES))
                    .output(PLAIN)
                    .output(JSON),
                CAP_WRITE,
            ))
    }
}

// ---------------------------------------------------------------------- assertions

/// Which assertion an [`AssertionEndpoint`] answers.
#[derive(Clone, Copy)]
pub(super) enum Assertion {
    /// `…:item:{id}:state:{value}`.
    State,
    /// `…:item:{id}:holder:{holder}`.
    Holder,
    /// `…:item:{id}:closed`.
    Closed,
}

impl Assertion {
    fn id(self) -> &'static str {
        match self {
            Assertion::State => "ledger-item-state-is",
            Assertion::Holder => "ledger-item-holder-is",
            Assertion::Closed => "ledger-item-closed",
        }
    }
}

/// An assertion about one item, answered by `Exists`: `true` when it holds now, `false`
/// when it does not — including when there is no such item, since "item 12 is closed" is
/// false of an item that does not exist.
///
/// ⚠ Only those two answers are booleans. A denial, a missing store, an item present but
/// unreadable, and a state name the ledger's lifecycle does not declare are ERRORS: a loop
/// reading `false` stops the wave, and a typo in the state it checks for must not read as
/// "the item is not in that state".
#[derive(Clone)]
pub(super) struct AssertionEndpoint {
    pub(super) assertion: Assertion,
    pub(super) lifecycles: Arc<Lifecycles>,
}

#[async_trait]
impl Endpoint for AssertionEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        if inv.request.verb != Verb::Exists {
            return Err(unsupported(self.assertion.id(), inv.request.verb));
        }
        let client = StoreClient::new(inv, ledger_for(inv, Need::Read)?);
        let id = part_id(inv)?;
        let lifecycle = self.lifecycles.for_ledger(client.ledger());
        // Checked before the item is read, so a typo is refused even for an item that is
        // not there.
        let wanted_state = match self.assertion {
            Assertion::State => {
                let value = inv.bindings.get("value").unwrap_or_default();
                Some(named_state(lifecycle, "value", value)?)
            }
            _ => None,
        };
        let item = match require_item(&client, id).await {
            Ok(item) => item,
            Err(Error::NotFound(_)) => return face("false\n".to_string(), None, PLAIN),
            Err(other) => return Err(other),
        };
        let holds = match self.assertion {
            Assertion::State => {
                let wanted = wanted_state.expect("checked above");
                match item.state_values.as_slice() {
                    [] => wanted.name == FILED,
                    [one] => one.kind == "uri" && one.value == wanted.iri,
                    _ => false,
                }
            }
            Assertion::Holder => {
                let holder = inv.bindings.get("holder").unwrap_or_default();
                match holder {
                    "none" => item.claimed_by.is_none(),
                    "any" => item.claimed_by.is_some(),
                    name => item.claimed_by.as_deref() == Some(name),
                }
            }
            Assertion::Closed => !item.open,
        };
        face(
            if holds { "true\n" } else { "false\n" }.to_string(),
            None,
            PLAIN,
        )
    }

    fn name(&self) -> &str {
        self.assertion.id()
    }

    fn describe(&self) -> Description {
        let (title, summary, value) = match self.assertion {
            Assertion::State => (
                "Is the item in this state?",
                "`true` when the item is in exactly this lifecycle state — `filed` when it holds \
                 none — and `false` otherwise, including when there is no such item. A state \
                 the ledger's lifecycle does not declare is refused, so a typo cannot read as \
                 `false`.",
                Some(
                    ArgSpec::new("value")
                        .summary("A state of the ledger's lifecycle.")
                        .class(v::ext::XSD_STRING)
                        .one_of(self.lifecycles.state_names())
                        .binding(),
                ),
            ),
            Assertion::Holder => (
                "Is the item held by this holder?",
                "`true` when the item's claim is held by exactly this holder, `false` otherwise. \
                 `none` asks whether it is unclaimed and `any` whether anyone holds it — the \
                 `holder=` filter's keywords, which is why no holder may be called either. An \
                 expired lease is still held: it is never silently free.",
                Some(
                    ArgSpec::new("holder")
                        .summary(
                            "The holder's name, `none` or `any`. The rest of the IRI, so a \
                             holder that is itself an IRI may be used as it stands.",
                        )
                        .class(v::ext::XSD_STRING)
                        .binding(),
                ),
            ),
            Assertion::Closed => (
                "Is the item closed?",
                "`true` when the item is closed, `false` when it is open or there is no such \
                 item.",
                None,
            ),
        };
        let mut spec = ikigai_core::ActionSpec::new(Verb::Exists)
            .input(ledger_arg())
            .summary(summary)
            .input(item_id_input());
        if let Some(value) = value {
            spec = spec.input(value);
        }
        Description::new(self.assertion.id())
            .title(title)
            .summary(format!(
                "{summary} An assertion a loop checks after each step, so a skipped step stops \
                 the wave instead of being narrated as done."
            ))
            .verb(Verb::Meta)
            .action(read_action(spec.output(PLAIN)))
    }
}

// ----------------------------------------------------------------------- lifecycle

/// `urn:iki:ledger:lifecycle:{name}` — a lifecycle this host offers, as a resource.
#[derive(Clone)]
pub(super) struct LifecycleEndpoint {
    pub(super) lifecycles: Arc<Lifecycles>,
}

#[async_trait]
impl Endpoint for LifecycleEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let name = inv.bindings.get("name").ok_or_else(|| {
            Error::Endpoint("no `name` captured from `urn:iki:ledger:lifecycle:{name}`".to_string())
        })?;
        let lifecycle = self.lifecycles.get(name).ok_or_else(|| {
            Error::NotFound(format!(
                "no lifecycle `{name}`; this host offers {}",
                self.lifecycles.names().join(", ")
            ))
        })?;
        match inv.request.verb {
            Verb::Exists => Ok(plain("true\n").cacheable()),
            Verb::Source => {
                let want = wanted_face(inv, &FACES)?;
                // Pure: a lifecycle is configuration the host registered at boot and reads
                // no state, so it is cacheable with no thread — `ledger-policy`'s shape.
                if want == TURTLE {
                    return Ok(Representation::new(
                        ReprType::new(TURTLE).with_param("charset", "utf-8"),
                        model::turtle(lifecycle.graph())?,
                    )
                    .cacheable());
                }
                Ok(plain(lifecycle.plain()).cacheable())
            }
            other => Err(unsupported("ledger-lifecycle", other)),
        }
    }

    fn name(&self) -> &str {
        "ledger-lifecycle"
    }

    fn describe(&self) -> Description {
        let name_input = || {
            ArgSpec::new("name")
                .summary(format!(
                    "The lifecycle's name. This host offers: {}.",
                    self.lifecycles.names().join(", ")
                ))
                .class(v::ext::XSD_STRING)
                .one_of(self.lifecycles.names())
                .binding()
        };
        Description::new("ledger-lifecycle")
            .title("A lifecycle")
            .summary(
                "The legal states of an item, in order: which are in flight (worked under a \
                 claim) and, for every other state, the drain that owns an item parked there. \
                 `filed` is the state of an item that holds none. Each ledger uses one, fixed \
                 by the host; `urn:iki:ledger:{ledger}:item:{id}:state` enforces it.",
            )
            .verb(Verb::Meta)
            .action(
                ikigai_core::ActionSpec::new(Verb::Source)
                    .summary("The states, in order, with what each one means.")
                    .requires(CAP_READ)
                    .input(name_input())
                    .input(as_arg(&FACES))
                    .output(PLAIN)
                    .output(TURTLE),
            )
            .action(
                ikigai_core::ActionSpec::new(Verb::Exists)
                    .summary("Whether this host offers a lifecycle by that name.")
                    .requires(CAP_READ)
                    .input(name_input())
                    .output(PLAIN),
            )
    }
}
