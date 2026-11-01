//! Who wrote a ledger entry, the way git says it (ledger #2827): a **committer** the door
//! stamps, and an **author** that is either verified or a claim.
//!
//! # ★ The committer is the door's, never the caller's
//!
//! Every item, comment and tombstone a write creates records `ledger:committer`, the one
//! principal (`urn:cap:principal:<iri>`) the writer's capability carried, and
//! `ledger:committedAt`, the kernel clock. Neither is an argument. A capability with no
//! principal (root, a host whose door mints none) records no committer, and the time alone.
//!
//! # The author is optional, and checked
//!
//! With no `author=` the committer is the author, and that author is **verified**. An
//! `author=` is accepted only when it names:
//!
//! - the committer itself: verified (`ledger:authorSource "committer"`);
//! - a principal the committer acts for, by `prov:actedOnBehalfOf` in the host's
//!   participants graph ([`crate::SpaceConfig::participants`]): claimed (`"delegate"`);
//! - any foreign actor, when the committer is an import or sync principal (typed
//!   `ledger:Importer` in the same graph): claimed (`"import"`).
//!
//! Anything else is [`Error::Denied`], with the reason and what to do instead. Who may claim
//! whom is enrollment data in that graph, not a capability per pair.
//!
//! ⚠ **A write without a principal is unchanged**: there is no committer to check an author
//! against, so `author=` is recorded as the free text it always was, with no source, and
//! every face reads it as claimed.
//!
//! # Authored dates are an importer's alone
//!
//! An import principal may claim an item's `created` and `modified` when filing it, and a
//! comment's `created`, as `xsd:dateTime`s no later than the commit. They are stored in
//! `dcterms:created` and `dcterms:modified` (so ranking, which reads `modified`, tie-breaks
//! imported items as their source did), marked `ledger:createdSource` / `ledger:modifiedSource`
//! `"import"`, beside the true `ledger:committedAt`. Any other caller that sends a date is
//! refused.
//!
//! ⚠ The participants graph is read through the store's narrow door with the CALLER's
//! capability, as every read here is, so a delegate or an importer needs that graph's read
//! grant (`urn:cap:store:read:graph:<participants>`). Writing as yourself reads nothing.

use ikigai_core::{Error, Invocation, Result};

use crate::sparql::{self, datetime, literal, StoreClient};
use crate::vocabulary as v;

/// How an author was admitted: the value of `ledger:authorSource`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorSource {
    /// The author is the committer: verified.
    Committer,
    /// A principal the committer acts for: claimed.
    Delegate,
    /// A foreign actor an import principal named: claimed.
    Import,
}

impl AuthorSource {
    /// The literal `ledger:authorSource` holds.
    pub fn as_str(self) -> &'static str {
        match self {
            AuthorSource::Committer => "committer",
            AuthorSource::Delegate => "delegate",
            AuthorSource::Import => "import",
        }
    }

    /// The source a stored literal names.
    pub fn from_word(word: &str) -> Option<AuthorSource> {
        [
            AuthorSource::Committer,
            AuthorSource::Delegate,
            AuthorSource::Import,
        ]
        .into_iter()
        .find(|s| s.as_str() == word)
    }
}

/// The literal that marks a claimed date (`ledger:createdSource`, `ledger:modifiedSource`).
pub const IMPORT: &str = "import";

/// How a face says whether an author is verified: `verified` for the committer, and for
/// everything else (a delegate, an import, or free text with no source) the word `claimed`
/// with how.
pub fn author_status(source: Option<&str>) -> &'static str {
    match source.and_then(AuthorSource::from_word) {
        Some(AuthorSource::Committer) => "verified",
        Some(AuthorSource::Delegate) => "claimed by a delegate",
        Some(AuthorSource::Import) => "claimed by an importer",
        None => "claimed, free text",
    }
}

/// Who wrote one entry, resolved and checked before anything is written.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Attribution {
    /// The principal the door minted, if the capability carried one.
    pub committer: Option<String>,
    /// The author: an IRI when [`source`](Attribution::source) is set, free text when not.
    pub author: Option<String>,
    /// How [`author`](Attribution::author) was admitted.
    pub source: Option<AuthorSource>,
    /// An authored `created` an importer claimed (ms since the epoch).
    pub created: Option<u64>,
    /// An authored `modified` an importer claimed (ms since the epoch).
    pub modified: Option<u64>,
}

impl Attribution {
    /// Nobody: a write the ledger makes on no caller's behalf (a note with no author).
    #[cfg(test)]
    pub fn none() -> Attribution {
        Attribution::default()
    }

    /// The committer alone, as the author, for a write that takes no `author=` (a claim's or
    /// a deferral's note): verified when the capability carries a principal, and nobody when
    /// it does not.
    pub fn committer(inv: &Invocation<'_>) -> Result<Attribution> {
        let committer = inv.capability.principal().map(str::to_string);
        // The door minted it, but it is still spliced into an update: hold it to the IRI
        // grammar.
        if let Some(committer) = &committer {
            sparql::iri(committer, "principal")?;
        }
        Ok(Attribution {
            source: committer.as_ref().map(|_| AuthorSource::Committer),
            author: committer.clone(),
            committer,
            ..Attribution::default()
        })
    }

    /// The committer and the author as triples on `subject`, stamped `now`. The dates are
    /// the caller's to place, because where `dcterms:created` goes differs per entry.
    pub fn triples(&self, subject: &str, now: u64) -> String {
        let mut out = format!("\n<{subject}> <{}> {} .", v::COMMITTED_AT, datetime(now));
        if let Some(committer) = &self.committer {
            out.push_str(&format!("\n<{subject}> <{}> <{committer}> .", v::COMMITTER));
        }
        match (&self.author, self.source) {
            (Some(author), Some(source)) => out.push_str(&format!(
                "\n<{subject}> <{}> <{author}> ; <{}> {} .",
                v::AUTHOR,
                v::AUTHOR_SOURCE,
                literal(source.as_str())
            )),
            (Some(author), None) => out.push_str(&format!(
                "\n<{subject}> <{}> {} .",
                v::AUTHOR,
                literal(author)
            )),
            _ => {}
        }
        out
    }

    /// `dcterms:created` for an entry committed `now`: the claimed date when there is one.
    pub fn created_or(&self, now: u64) -> u64 {
        self.created.unwrap_or(now)
    }

    /// `dcterms:modified` for an item committed `now`: the claimed date when there is one.
    pub fn modified_or(&self, now: u64) -> u64 {
        self.modified.unwrap_or(now)
    }

    /// The source markers for the claimed dates on `subject`.
    pub fn date_sources(&self, subject: &str) -> String {
        let mut out = String::new();
        for (claimed, predicate) in [
            (self.created, v::CREATED_SOURCE),
            (self.modified, v::MODIFIED_SOURCE),
        ] {
            if claimed.is_some() {
                out.push_str(&format!(
                    "\n<{subject}> <{predicate}> {} .",
                    literal(IMPORT)
                ));
            }
        }
        out
    }
}

/// Which authored dates a write accepts: an item's filing takes both, a comment `created`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dates {
    /// No date argument at all (a delete, a release note): any date sent is refused.
    None,
    /// `created` only (a comment).
    Created,
    /// `created` and `modified` (filing an item).
    CreatedAndModified,
}

/// Resolve and check who is writing, from the capability and the `author`, `created` and
/// `modified` arguments, before anything is written. `participants` is the host's
/// participants graph, if it named one.
pub async fn resolve(
    inv: &Invocation<'_>,
    client: &StoreClient<'_, '_>,
    participants: Option<&str>,
    dates: Dates,
    now: u64,
) -> Result<Attribution> {
    let committer = Attribution::committer(inv)?.committer;
    let author = inv
        .inline_str("author")
        .ok()
        .map(str::trim)
        .filter(|a| !a.is_empty());
    let date_args: Vec<(&str, &str)> = ["created", "modified"]
        .into_iter()
        .filter_map(|name| inv.inline_str(name).ok().map(|value| (name, value)))
        .collect();

    // An importer is looked up at most once, and only when something needs it.
    let mut importer: Option<bool> = None;

    let mut out = Attribution {
        committer: committer.clone(),
        ..Attribution::default()
    };

    match (&committer, author) {
        // No principal: free text, as before — there is nothing to check it against.
        (None, author) => out.author = author.map(str::to_string),
        (Some(committer), None) => {
            out.author = Some(committer.clone());
            out.source = Some(AuthorSource::Committer);
        }
        (Some(committer), Some(author)) if author == committer => {
            out.author = Some(committer.clone());
            out.source = Some(AuthorSource::Committer);
        }
        (Some(committer), Some(author)) => {
            let as_iri = sparql::iri(author, "author").map_err(|_| {
                refused(
                    "author",
                    author,
                    committer,
                    "an author is a principal's IRI (urn:collab:participant:{name}), not a \
                     name",
                    "send the IRI of the principal you write for, or no author to be the \
                     author yourself",
                )
            })?;
            let delegate = acts_for(client, participants, committer, &as_iri).await?;
            if !delegate {
                importer = Some(enrolled_as_importer(client, participants, committer).await?);
            }
            if delegate {
                out.author = Some(author.to_string());
                out.source = Some(AuthorSource::Delegate);
            } else if importer == Some(true) {
                out.author = Some(author.to_string());
                out.source = Some(AuthorSource::Import);
            } else {
                return Err(refused(
                    "author",
                    author,
                    committer,
                    &match participants {
                        Some(graph) => format!(
                            "it is not the committer, the participants graph <{graph}> does \
                             not enroll the committer to act for it (prov:actedOnBehalfOf), and \
                             the committer is not an import principal (ledger:Importer)"
                        ),
                        None => "it is not the committer, and this host names no participants \
                                 graph to enroll a delegate or an import principal in"
                            .to_string(),
                    },
                    "send no author to be the author yourself, or have the host enroll you to \
                     act for that principal",
                ));
            }
        }
    }

    for (name, value) in date_args {
        let allowed = match dates {
            Dates::None => false,
            Dates::Created => name == "created",
            Dates::CreatedAndModified => true,
        };
        if !allowed {
            return Err(Error::InvalidArgument {
                name: name.to_string(),
                detail: format!(
                    "this write takes no authored `{name}`: an import principal may claim an \
                     item's created and modified when filing it, and a comment's created. \
                     Send it without `{name}`, and the ledger stamps the time itself"
                ),
            });
        }
        let Some(committer) = &committer else {
            return Err(refused_date(
                name,
                value,
                "this capability carries no principal, so nothing says it is an import \
                 principal",
            ));
        };
        let answer = match importer {
            Some(answer) => answer,
            None => enrolled_as_importer(client, participants, committer).await?,
        };
        importer = Some(answer);
        if !answer {
            return Err(refused_date(
                name,
                value,
                &format!(
                    "{committer} is not an import or sync principal{}",
                    match participants {
                        Some(graph) => format!(" (ledger:Importer in <{graph}>)"),
                        None => ", and this host names no participants graph".to_string(),
                    }
                ),
            ));
        }
        let millis = crate::model::millis(value).ok_or_else(|| Error::InvalidArgument {
            name: name.to_string(),
            detail: format!(
                "`{value}` is not an xsd:dateTime; an authored date is written \
                 2026-11-01T18:30:00Z, with a time and a zone"
            ),
        })?;
        if millis > now {
            return Err(Error::InvalidArgument {
                name: name.to_string(),
                detail: format!(
                    "`{value}` is after this write ({}); an authored date is when the entry was \
                     written at its source, which cannot be later than its import",
                    sparql::iso8601(now)
                ),
            });
        }
        match name {
            "created" => out.created = Some(millis),
            _ => out.modified = Some(millis),
        }
    }
    if let (Some(created), Some(modified)) = (out.created, out.modified) {
        if modified < created {
            return Err(Error::InvalidArgument {
                name: "modified".to_string(),
                detail: format!(
                    "`{}` is before `created` ({}); an item cannot change before it was filed",
                    sparql::iso8601(modified),
                    sparql::iso8601(created)
                ),
            });
        }
    }
    // A claimed `created` alone moves `modified` with it, so `modified` is never earlier
    // than `created`.
    if out.created.is_some() && out.modified.is_none() && dates == Dates::CreatedAndModified {
        out.modified = out.created;
    }
    Ok(out)
}

/// A refused author: the reason and the remedy, and the principal it was checked against.
fn refused(name: &str, value: &str, committer: &str, why: &str, remedy: &str) -> Error {
    Error::Denied(format!(
        "`{name}={value}` refused for committer {committer}: {why}. To write anyway, {remedy}"
    ))
}

/// A refused authored date.
fn refused_date(name: &str, value: &str, why: &str) -> Error {
    Error::Denied(format!(
        "`{name}={value}` refused: only an import or sync principal may claim an authored date, \
         and {why}. Send it without `{name}`, and the ledger stamps the time itself"
    ))
}

/// Whether the participants graph enrolls `committer` to act for `author` (an IRI term).
async fn acts_for(
    client: &StoreClient<'_, '_>,
    participants: Option<&str>,
    committer: &str,
    author: &str,
) -> Result<bool> {
    let Some(graph) = participants else {
        return Ok(false);
    };
    client
        .ask_graph(
            graph,
            &format!(
                "ASK {{ GRAPH <{graph}> {{ <{committer}> <{}> {author} }} }}",
                v::ext::ACTED_ON_BEHALF_OF
            ),
        )
        .await
}

/// Whether the participants graph types `committer` an import or sync principal.
async fn enrolled_as_importer(
    client: &StoreClient<'_, '_>,
    participants: Option<&str>,
    committer: &str,
) -> Result<bool> {
    let Some(graph) = participants else {
        return Ok(false);
    };
    client
        .ask_graph(
            graph,
            &format!(
                "ASK {{ GRAPH <{graph}> {{ <{committer}> <{}> <{}> }} }}",
                v::ext::TYPE,
                v::IMPORTER_CLASS
            ),
        )
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_free_text_author_is_written_as_a_literal_with_no_source() {
        let who = Attribution {
            author: Some("brian".to_string()),
            ..Attribution::none()
        };
        let triples = who.triples("urn:x", 0);
        assert!(triples.contains("\"brian\""), "{triples}");
        assert!(!triples.contains("authorSource"), "{triples}");
        assert!(!triples.contains("committer>"), "{triples}");
    }

    #[test]
    fn every_source_reads_back_as_a_word() {
        for source in [
            AuthorSource::Committer,
            AuthorSource::Delegate,
            AuthorSource::Import,
        ] {
            assert_eq!(AuthorSource::from_word(source.as_str()), Some(source));
        }
        assert_eq!(author_status(Some("committer")), "verified");
        assert_eq!(author_status(None), "claimed, free text");
    }
}
