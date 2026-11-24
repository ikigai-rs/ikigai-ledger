//! **Dispatch notes** (ledger #3718; the design is `docs/dispatch-notes.md`): the dispatcher
//! speaking to the holder of an item's current claim, on the record.
//!
//! ```text
//! urn:iki:ledger:{ledger}:note              Sink    send a note to the current claim's holder
//! urn:iki:ledger:{ledger}:item:{id}:notes   Source  the notes to the caller's current claim
//! ```
//!
//! A note is a `ledger:Comment` that is also a `ledger:DispatchNote`, so every reader that
//! lists comments shows it. What makes it a note is recorded by the write itself and never
//! taken from the caller: the grant that admitted it (`close:{ledger}` or `write:{ledger}`,
//! the dispatcher's), the verified principal that sent it, and the claim it is for, read from
//! the item in the same update that writes it, at exactly the epoch the dispatcher named.
//!
//! ★ **What a note can never do**, each pinned in `tests/dispatch_note_gap.rs`: widen a grant
//! (it carries none, and no argument here changes a claim, a lease, a label, a state or a
//! grant), act on the item (it never claims, releases, renews, closes, reopens or hands
//! back), reach a later epoch (the fence refuses it and nothing is written), or be edited (it
//! is a comment, and comments are append-only).

use super::*;

/// `…:note`: send a dispatch note.
#[derive(Clone)]
pub(super) struct NoteEndpoint {
    pub(super) participants: Participants,
    /// The request tokens' window and in-flight set (ledger #4059).
    pub(super) tokens: replay::Tokens,
}

/// The faces of the notes read.
const NOTES_FACES: [&str; 3] = [PLAIN, TURTLE, JSON];

/// The refusal of a note whose epoch is not the current claim's (ledger #3718): the item,
/// the epoch asked for and the current claim, and that the note was not sent.
fn note_refusal(item: &Item, epoch: u64) -> Error {
    let short = item.short();
    Error::Conflict(match (&item.claimed_by, item.claim_epoch) {
        (Some(holder), Some(current)) => format!(
            "{short} is held by {holder} at epoch {current}, not epoch {epoch}, so the note \
             was not sent. A note is addressed to the claim at exactly the epoch it names"
        ),
        (Some(holder), None) => format!(
            "{short} is held by {holder} under a claim taken before epochs, not epoch \
             {epoch}, so the note was not sent"
        ),
        (None, _) if !item.open => format!(
            "{short} is closed and held by nobody, so the note was not sent. A note needs a \
             current claim; an ordinary comment is still written"
        ),
        (None, Some(last)) => format!(
            "{short} is not claimed (its last claim was epoch {last}), so the note was not \
             sent. A note needs a current claim; an ordinary comment is still written"
        ),
        (None, None) => format!(
            "{short} has never been claimed, so the note was not sent. A note needs a current \
             claim; an ordinary comment is still written"
        ),
    })
}

#[async_trait]
impl Endpoint for NoteEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        if inv.request.verb != Verb::Sink {
            return Err(unsupported("ledger-note", inv.request.verb));
        }
        // ★ No grant of its own: the dispatcher's close or write grant is the authority.
        let ledger = ledger_for(inv, Need::Close)?;
        let grant = if inv.capability.allows(&ledger.cap_close()) {
            ledger.cap_close()
        } else {
            ledger.cap_write()
        };
        // ★ An unattributed instruction is the record gap a note closes, so a principal is
        // required, root's included.
        if inv.capability.principal().is_none() {
            return Err(Error::Denied(
                "a dispatch note needs a principal on the capability: an unattributed \
                 instruction is not a note. A host that wants this caller to dispatch gives it \
                 a principal"
                    .to_string()
                    .into(),
            ));
        }
        let client = StoreClient::new(inv, ledger);
        // ★ The comment's token scope (ledger #4059): note and comment share `key=`'s namespace.
        under_token!(inv, &client, "comment", &self.tokens, |token| self
            .note(inv, &client, &grant, token)
            .await)
    }

    fn name(&self) -> &str {
        "ledger-note"
    }

    fn describe(&self) -> Description {
        self.contract()
    }
}

impl NoteEndpoint {
    async fn note(
        &self,
        inv: &Invocation<'_>,
        client: &StoreClient<'_, '_>,
        grant: &str,
        token: Option<&replay::Token>,
    ) -> Result<Representation> {
        let grant = grant.to_string();
        let want = wanted_face(inv, &WRITE_FACES)?;
        // The sender is the verified principal, never someone it names.
        for name in ["author", "created"] {
            if inv.inline_str(name).is_ok() {
                return Err(Error::InvalidArgument {
                    name: name.to_string(),
                    detail: format!(
                        "a dispatch note records its sender's verified principal and the \
                         kernel's time, so `{name}=` is not taken"
                    ),
                });
            }
        }
        let key = inv
            .inline_str("key")
            .ok()
            .map(model::parse_key)
            .transpose()?;
        let epoch = parse_epoch(inv)?.ok_or_else(|| Error::InvalidArgument {
            name: "epoch".to_string(),
            detail: "a note is addressed to one claim: `epoch=`, the current claim's epoch \
                     (`epoch:` in the item's detail, `claim.epoch` in JSON), is required"
                .to_string(),
        })?;
        let now = now_ms(inv)?;
        let item = require_item(client, inv.inline_str("item")?).await?;
        if let Some(token) = token {
            token.on_item(&item.iri);
        }
        let text = inv.inline_str("content")?;
        if text.trim().is_empty() {
            return Err(Error::InvalidArgument {
                name: "content".to_string(),
                detail: "an empty note says nothing and cannot be edited afterwards".to_string(),
            });
        }
        let who = attribution::resolve(
            inv,
            client,
            self.participants.as_deref(),
            Dates::Created,
            now,
        )
        .await?;
        // A claimed `created` is held to the item's filing, as a link's is (ledger #4010).
        if let Some(claimed) = who.created {
            attribution::claimed_not_before_filing("created", claimed, item.created)?;
        }
        if let Some(key) = &key {
            if let Some(found) = model::find_comment_by_key(client, key).await? {
                return existing_comment(client, want, &found, key).await;
            }
        }
        if !epoch_is_current(&item, epoch) {
            return Err(note_refusal(&item, epoch));
        }
        // The recipient is the claim the fence holds at this epoch: the label and principal as
        // read, which a renewal keeps and anything else would have refused the write.
        let holder = item.claimed_by.clone().unwrap_or_default();
        let note = model::Note {
            holder: holder.clone(),
            principal: item.claim_principal.clone(),
            epoch,
            grant: grant.clone(),
        };
        // ★ Rendered before the write (ledger #4059), so a request token's record of it rides
        // in the note's own update.
        let comment = client.ledger().comment(&mint_id(now)?);
        let answer = if want == JSON {
            let ledger = client.ledger();
            let mut answer = json::answer(
                ledger,
                "noted",
                json::item_ref(ledger, item.number, &item.iri),
            );
            let mut said = written(&comment, &item.iri, text.trim(), &who, now);
            said.note = Some(note.clone());
            answer.comment = Some(json::comment(&said));
            answer.key = key.clone();
            json_repr(json::render(&answer)?)
        } else {
            plain(format!("{} on {} ({comment})\n", note.line(), item.short()))
        };
        let record = token.map(|token| token.triples(&answer, now));
        let comment = write_keyed_comment(
            client,
            &item.iri,
            text.trim(),
            &who,
            now,
            key.as_deref(),
            Handback::Unchanged,
            &fence_clause(&item.iri, Some(epoch)),
            &CommentExtra {
                note: Some((epoch, &grant)),
                iri: Some(&comment),
                replay: record.as_deref(),
                ..CommentExtra::default()
            },
        )
        .await?;
        if !comment_written(client, &comment).await? {
            if let Some(key) = &key {
                if let Some(found) = model::find_comment_by_key(client, key).await? {
                    return existing_comment(client, want, &found, key).await;
                }
            }
            let now = model::load_item(client, &item.iri).await?.unwrap_or(item);
            return Err(note_refusal(&now, epoch));
        }
        Ok(answer)
    }

    fn contract(&self) -> Description {
        Description::new("ledger-note")
            .title("Send a dispatch note to the holder of an item's claim")
            .summary(
                "The dispatcher (a caller holding the close or write grant, with a verified \
                 principal) speaks to the worker holding an item, on the record: a comment that \
                 is also a `ledger:DispatchNote`, recording the grant that admitted it, who it is \
                 for and the claim epoch it was sent at. The recipient is read from the current \
                 claim, never from an argument. A note carries no capability and changes no \
                 claim, lease, label, state or grant; like every comment it is never edited.",
            )
            .verb(Verb::Meta)
            .action(write_scopes(
                replay::declare(
                    ikigai_core::ActionSpec::new(Verb::Sink),
                    self.tokens.window,
                    "the note",
                )
                .input(ledger_arg("note"))
                .summary("Append a dispatch note addressed to the current claim's holder.")
                .input(item_arg("The item whose holder the note is for."))
                .input(
                    ArgSpec::new("content")
                        .summary("The note text — where a pipe's value lands.")
                        .class(v::ext::XSD_STRING),
                )
                .input(
                    ArgSpec::new("epoch")
                        .summary(
                            "Required: the epoch of the claim the note is for. The note is \
                                 written only while the item is claimed at exactly this epoch, \
                                 checked in the write's own update; otherwise refused \
                                 (Conflict) naming the current holder and epoch, and nothing \
                                 is written.",
                        )
                        .class(v::ext::XSD_INTEGER),
                )
                .input(
                    ArgSpec::new("key")
                        .summary(
                            "Your own name for this note, as `comment key=`: if a comment \
                                 already carries it, NOTHING is written and the answer names \
                                 that comment, marked `existing`, so a dispatcher resending \
                                 after a dropped connection records the note once.",
                        )
                        .class(v::ext::XSD_STRING)
                        .optional(),
                )
                .input(as_arg(&WRITE_FACES))
                .output(PLAIN)
                .output(JSON),
                CAP_ANY,
            ))
    }
}

/// `…:item:{id}:notes`: the dispatch notes to the caller's current claim.
#[derive(Clone)]
pub(super) struct NotesEndpoint;

#[async_trait]
impl Endpoint for NotesEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        if inv.request.verb != Verb::Source {
            return Err(unsupported("ledger-item-notes", inv.request.verb));
        }
        let client = StoreClient::new(inv, ledger_for(inv, Need::Read)?);
        let want = wanted_face(inv, &NOTES_FACES)?;
        let epoch = parse_epoch(inv)?;
        let after = inv.inline_str("after").ok().map(str::trim);
        let item = require_item(&client, flight::part_id(inv)?).await?;
        let short = item.short();
        let (Some(holder), Some(current)) = (&item.claimed_by, item.claim_epoch) else {
            return Err(Error::Conflict(format!(
                "{short} is not claimed at an epoch, so no claim has notes to read"
            )));
        };
        if let Some(epoch) = epoch {
            if epoch != current {
                return Err(Error::Conflict(format!(
                    "{short} is held by {holder} at epoch {current}, not epoch {epoch}: the \
                     notes read are the current claim's only"
                )));
            }
        }
        // ★ Only the notes addressed to the CALLER'S claim: a claim taken as a principal is
        // read by a capability acting as it.
        if let Some(principal) = item.claim_principal.as_deref() {
            if !inv.capability.acts_as(principal) {
                return Err(Error::Denied(
                    format!(
                        "{short} is held by {holder} as {principal}, and this capability does not \
                         act as that principal: a claim's notes are read by its holder"
                    )
                    .into(),
                ));
            }
        }
        let comments = model::load_comments(&client, &item.iri).await?;
        let mut notes: Vec<model::Comment> = comments
            .into_iter()
            .filter(|c| c.note.as_ref().is_some_and(|n| n.epoch == current))
            .collect();
        if let Some(after) = after {
            match notes.iter().position(|n| n.iri == after) {
                Some(at) => {
                    notes.drain(..=at);
                }
                None => {
                    return Err(Error::InvalidArgument {
                        name: "after".to_string(),
                        detail: format!(
                            "`{after}` is not a note to {short}'s claim at epoch {current}"
                        ),
                    })
                }
            }
        }
        match want {
            JSON => {
                let ledger = client.ledger();
                let document = serde_json::json!({
                    "schema": json::SCHEMA,
                    "ledger": ledger.name(),
                    "item": json::item_ref(ledger, item.number, &item.iri),
                    "holder": holder,
                    "epoch": current,
                    "notes": notes.iter().map(json::comment).collect::<Vec<_>>(),
                });
                Ok(json_face(serde_json::to_vec(&document).map_err(|e| {
                    Error::Endpoint(format!("the notes document did not serialize: {e}"))
                })?))
            }
            TURTLE => {
                let mut graph = Graph::new();
                for note in &notes {
                    note.triples(&mut graph);
                }
                face(String::new(), Some(graph), TURTLE)
            }
            _ => {
                let mut text = match notes.len() {
                    0 => format!("{short}: no notes to {holder} at epoch {current}\n"),
                    1 => format!("{short}: 1 note to {holder} at epoch {current}:\n"),
                    n => format!("{short}: {n} notes to {holder} at epoch {current}:\n"),
                };
                for note in &notes {
                    text.push_str(&note.render());
                }
                face(text, None, PLAIN)
            }
        }
    }

    fn name(&self) -> &str {
        "ledger-item-notes"
    }

    fn describe(&self) -> Description {
        Description::new("ledger-item-notes")
            .title("The dispatch notes to an item's current claim")
            .summary(
                "The notes the dispatcher sent to the claim the caller holds on one item, \
                 oldest first. Only the current claim's: a note to an earlier epoch is never \
                 shown to a later claim, even the same worker's.",
            )
            .verb(Verb::Meta)
            .action(read_action(
                ikigai_core::ActionSpec::new(Verb::Source)
                    .input(ledger_arg("item:{id}:notes"))
                    .summary(
                        "The current claim's notes: a count line and each note's block in \
                         text; the note nodes in Turtle; `notes`, an array of comments, in \
                         JSON.",
                    )
                    .input(flight::item_id_input())
                    .input(
                        ArgSpec::new("epoch")
                            .summary(
                                "The epoch the caller holds. When given and not the current \
                                 claim's, refused (Conflict) naming the current holder and \
                                 epoch.",
                            )
                            .class(v::ext::XSD_INTEGER)
                            .optional(),
                    )
                    .input(
                        ArgSpec::new("after")
                            .summary(
                                "A note's IRI: only the notes after it, so a loop polls \
                                 without reading a note twice.",
                            )
                            .class(v::ext::XSD_STRING)
                            .optional(),
                    )
                    .input(as_arg(&NOTES_FACES))
                    .output(PLAIN)
                    .output(TURTLE)
                    .output(JSON),
            ))
    }
}

/// How many notes to the claim at `epoch` on `item` no comment acknowledges yet: the count
/// `claim renew=true` answers with.
pub(super) async fn unread(client: &StoreClient<'_, '_>, item: &str, epoch: u64) -> Result<usize> {
    let query = format!(
        "SELECT (COUNT(DISTINCT ?note) AS ?n) WHERE {{ {} }}",
        client.in_graph(&format!(
            "?note <{}> <{}> ; <{}> <{item}> ; <{}> {epoch} . \
             FILTER NOT EXISTS {{ ?ack <{}> ?note }}",
            v::ext::TYPE,
            v::DISPATCH_NOTE_CLASS,
            v::ON_ITEM,
            v::NOTE_EPOCH,
            v::ACKS
        ))
    );
    Ok(client
        .select(&query)
        .await?
        .first()
        .and_then(|row| row.get("n")?.as_i64())
        .unwrap_or(0)
        .max(0) as usize)
}

/// `acks=` on a comment: the note IRIs it acknowledges, which need `epoch=`.
pub(super) fn parse_acks(inv: &Invocation<'_>, epoch: Option<u64>) -> Result<Vec<String>> {
    let Ok(raw) = inv.inline_str("acks") else {
        return Ok(Vec::new());
    };
    let mut acks = Vec::new();
    for part in raw.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        sparql::iri(part, "acks")?;
        acks.push(part.to_string());
    }
    if acks.is_empty() {
        return Err(Error::InvalidArgument {
            name: "acks".to_string(),
            detail: "`acks=` names no note; give a note's IRI, or omit it".to_string(),
        });
    }
    if epoch.is_none() {
        return Err(Error::InvalidArgument {
            name: "acks".to_string(),
            detail: "an acknowledgment is the holder's, so `acks=` needs `epoch=`, the claim \
                     the notes were sent to"
                .to_string(),
        });
    }
    acks.sort();
    acks.dedup();
    Ok(acks)
}

/// Refuse an `acks=` naming anything but a note on `item` to the claim at `epoch`.
pub(super) async fn check_acks(
    client: &StoreClient<'_, '_>,
    item: &Item,
    epoch: u64,
    acks: &[String],
) -> Result<()> {
    for note in acks {
        let found = client
            .ask(&format!(
                "ASK {{ {} }}",
                client.in_graph(&format!(
                    "<{note}> <{}> <{}> ; <{}> <{}> ; <{}> {epoch} .",
                    v::ext::TYPE,
                    v::DISPATCH_NOTE_CLASS,
                    v::ON_ITEM,
                    item.iri,
                    v::NOTE_EPOCH
                ))
            ))
            .await?;
        if !found {
            return Err(Error::InvalidArgument {
                name: "acks".to_string(),
                detail: format!(
                    "`{note}` is not a note to {}'s claim at epoch {epoch}, so nothing was \
                     written",
                    item.short()
                ),
            });
        }
    }
    Ok(())
}

/// Refuse an `acks=` from a capability that does not act as the claim's principal: an
/// acknowledgment is the holder's (ledger #3718), as the notes read is. Without this any
/// caller allowed to comment who knows the epoch, the dispatcher included, could clear the
/// holder's unread count.
pub(super) fn check_acker(inv: &Invocation<'_>, item: &Item, acks: &[String]) -> Result<()> {
    if acks.is_empty() {
        return Ok(());
    }
    match item.claim_principal.as_deref() {
        Some(principal) if !inv.capability.acts_as(principal) => Err(Error::Denied(
            format!(
                "{} is held by {} as {principal}, and this capability does not act as that \
                 principal: a claim's notes are acknowledged by its holder, so nothing was written",
                item.short(),
                item.claimed_by.as_deref().unwrap_or("nobody"),
            )
            .into(),
        )),
        _ => Ok(()),
    }
}

/// Who may acknowledge, as the write's own update checks it: the capability's identity,
/// compared there with the claim's principal as [`Capability::acts_as`] compares it.
#[derive(Clone, Copy, Default)]
pub(crate) enum Acker<'a> {
    /// Root acts as every principal: no clause.
    #[default]
    Anyone,
    /// A capability carrying one principal: the claim's, if it has one, must be this.
    Principal(&'a str),
    /// A capability carrying no single principal: only a claim taken without one.
    Nobody,
}

impl<'a> Acker<'a> {
    pub(super) fn of(inv: &'a Invocation<'_>) -> Self {
        if inv.capability.is_root() {
            return Self::Anyone;
        }
        match inv.capability.principal() {
            Some(principal) => Self::Principal(principal),
            None => Self::Nobody,
        }
    }

    /// The clause, inside the store's graph, that holds only while this acker acts as the
    /// claim's principal on `item`: so a claim that changes hands between the endpoint's read
    /// and its write is not acknowledged by the new caller.
    pub(super) fn clause(self, item: &str) -> Result<String> {
        Ok(match self {
            Self::Anyone => String::new(),
            Self::Principal(principal) => format!(
                "FILTER NOT EXISTS {{ <{item}> <{p}> ?ackPrincipal \
                 FILTER(?ackPrincipal != {me}) }}",
                p = v::CLAIM_PRINCIPAL,
                me = sparql::iri(principal, "principal")?,
            ),
            Self::Nobody => format!(
                "FILTER NOT EXISTS {{ <{item}> <{}> ?ackPrincipal }}",
                v::CLAIM_PRINCIPAL
            ),
        })
    }
}
