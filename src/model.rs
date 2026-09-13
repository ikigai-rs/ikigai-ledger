//! What an item is, how one is read out of the store, and the two faces it wears.
//!
//! The filters live HERE, in the query — not in an ordering policy. That is kata's one
//! structural idea worth copying outright: readiness (open, unclaimed, unblocked, not
//! deferred, has these labels, has none of those) is a *filter*, and keeping it there is
//! why a ranking policy can be a dozen lines instead of a rules engine.

use std::collections::BTreeMap;

use ikigai_core::{Error, Result};
use oxrdf::{Graph, Literal, NamedNode, Triple};

use crate::sparql::{self, literal, Row, StoreClient};
use crate::vocabulary as v;

/// Which items a read is asking for. Every field narrows; an empty filter is
/// "everything, open first".
#[derive(Debug, Clone, Default)]
pub struct Filter {
    /// `open`, `closed`, or `all`.
    pub status: Status,
    /// A specific item class — the LEVEL. `None` means every level, which is what
    /// asserting the base class on every item buys: "everything in the ledger" is one
    /// triple pattern, and scoping to a level is one more.
    pub kind: Option<String>,
    /// Must carry ALL of these labels.
    pub labels: Vec<String>,
    /// Must carry NONE of these labels.
    pub without: Vec<String>,
    /// Must be `ledger:about` this resource IRI.
    pub about: Option<String>,
    /// `any` (no constraint), `none` (unclaimed only), or a holder's name.
    pub holder: Holder,
    /// Whether deferred items are included.
    pub deferred: Deferred,
    /// A case-insensitive substring of the title.
    pub text: Option<String>,
    /// At most this many rows. Applied by the query; a ranking applies its own limit
    /// AFTER ranking, never before (kata's `ready --limit N` truncates by recency and
    /// then ranks, so "highest priority" silently means "best of the N most recent").
    pub limit: usize,
}

/// The status half of a [`Filter`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    /// Open items only — the default, because a ledger's default question is "what is
    /// still true".
    #[default]
    Open,
    /// Closed items only.
    Closed,
    /// Both.
    All,
}

/// The claim half of a [`Filter`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Holder {
    /// No constraint.
    #[default]
    Any,
    /// Unclaimed only — what the ready set means.
    None,
    /// Held by this holder.
    Named(String),
}

/// The deferral half of a [`Filter`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Deferred {
    /// No constraint.
    #[default]
    Include,
    /// Not deferred — what the ready set means.
    Exclude,
    /// Deferred only, so "what did I put off" is answerable.
    Only,
}

/// One item, as the store holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// The stable IRI. The identity.
    pub iri: String,
    /// The item's specific class, when it has one beyond `ledger:Item` — its LEVEL.
    ///
    /// ★ The hierarchy is types, not a column. A decision record, an issue, a review
    /// finding and a state transition have different scope and granularity and therefore
    /// different shapes; what they share is the plumbing. Every item also asserts the
    /// base class, so a query over "the ledger" needs no union and a new level needs no
    /// change here.
    pub kind: Option<String>,
    /// The human-facing short id. A label, never the identity.
    pub number: i64,
    /// The first line of what was filed.
    pub title: String,
    /// Everything after the first blank line of what was filed.
    pub body: String,
    /// Open or closed.
    pub open: bool,
    /// The close reason's short name, when closed.
    pub closed_reason: Option<String>,
    /// 0 highest … 4 lowest. `None` is unset, and unset is not 4.
    pub priority: Option<i64>,
    /// Open, unblocked, unclaimed — and still deliberately not offered.
    pub deferred: bool,
    /// Who filed it.
    pub author: Option<String>,
    /// The revision the item was filed against.
    pub revision: Option<String>,
    /// Who holds it, if anyone.
    pub claimed_by: Option<String>,
    /// Why the holder took it.
    pub purpose: Option<String>,
    /// Milliseconds since the epoch.
    pub created: u64,
    /// Milliseconds since the epoch.
    pub modified: u64,
    /// Free tags.
    pub labels: Vec<String>,
    /// The resources this item is ABOUT.
    pub about: Vec<String>,
    /// Items this one blocks.
    pub blocks: Vec<String>,
    /// Items this one is part of.
    pub parents: Vec<String>,
    /// See-also.
    pub related: Vec<String>,
}

impl Item {
    /// `#12`.
    pub fn short(&self) -> String {
        format!("#{}", self.number)
    }

    /// One greppable line: number, status, priority, labels, title, holder.
    ///
    /// The shape is fixed on purpose — this is what "view ASAP" means in the REPL, and a
    /// line that changes shape per item cannot be read down a column or grepped.
    pub fn line(&self) -> String {
        let status = if self.open { "open  " } else { "closed" };
        let priority = match self.priority {
            Some(p) => format!("p{p}"),
            None => "p-".to_string(),
        };
        let mut line = format!("{:>5}  {status}  {priority}  {}", self.short(), self.title);
        if !self.labels.is_empty() {
            line.push_str(&format!("  [{}]", self.labels.join(" ")));
        }
        if let Some(holder) = &self.claimed_by {
            line.push_str(&format!("  claimed:{holder}"));
        }
        if self.deferred {
            line.push_str("  deferred");
        }
        if let Some(reason) = &self.closed_reason {
            line.push_str(&format!("  ({reason})"));
        }
        line
    }

    /// The multi-line form: the line above, then the metadata a reader needs before
    /// touching the item, then the body.
    pub fn detail(&self) -> String {
        let mut out = self.line();
        out.push('\n');
        out.push_str(&format!("  iri:      {}\n", self.iri));
        if let Some(kind) = &self.kind {
            out.push_str(&format!("  kind:     {kind}\n"));
        }
        out.push_str(&format!(
            "  filed:    {}{}\n",
            sparql::iso8601(self.created),
            self.author
                .as_ref()
                .map(|a| format!(" by {a}"))
                .unwrap_or_default()
        ));
        out.push_str(&format!("  updated:  {}\n", sparql::iso8601(self.modified)));
        if let Some(revision) = &self.revision {
            out.push_str(&format!("  revision: {revision}\n"));
        }
        for about in &self.about {
            out.push_str(&format!("  about:    {about}\n"));
        }
        for target in &self.blocks {
            out.push_str(&format!("  blocks:   {target}\n"));
        }
        for target in &self.parents {
            out.push_str(&format!("  parent:   {target}\n"));
        }
        for target in &self.related {
            out.push_str(&format!("  related:  {target}\n"));
        }
        if let Some(purpose) = &self.purpose {
            out.push_str(&format!("  purpose:  {purpose}\n"));
        }
        if !self.body.is_empty() {
            out.push('\n');
            for line in self.body.lines() {
                out.push_str(&format!("  {line}\n"));
            }
        }
        out
    }

    /// This item's triples, for the Turtle face.
    pub fn triples(&self, graph: &mut Graph) {
        let subject = match NamedNode::new(&self.iri) {
            Ok(node) => node,
            // Unreachable in practice (the IRI came out of the store), and a panic in a
            // read face would be the worst possible answer to a malformed row.
            Err(_) => return,
        };
        let mut push = |predicate: &str, object: oxrdf::Term| {
            if let Ok(p) = NamedNode::new(predicate) {
                graph.insert(&Triple::new(subject.clone(), p, object));
            }
        };
        push(v::ext::TYPE, named(v::ITEM_CLASS));
        if let Some(kind) = &self.kind {
            if let Ok(node) = NamedNode::new(kind) {
                push(v::ext::TYPE, node.into());
            }
        }
        push(
            v::NUMBER,
            typed(&self.number.to_string(), v::ext::XSD_INTEGER),
        );
        push(v::ext::TITLE, plain(&self.title));
        if !self.body.is_empty() {
            push(v::BODY, plain(&self.body));
        }
        push(
            v::STATUS,
            named(if self.open { v::OPEN } else { v::CLOSED }),
        );
        if let Some(reason) = self.closed_reason.as_deref().and_then(v::close_reason) {
            push(v::CLOSED_REASON, named(reason));
        }
        if let Some(priority) = self.priority {
            push(
                v::PRIORITY,
                typed(&priority.to_string(), v::ext::XSD_INTEGER),
            );
        }
        if self.deferred {
            push(v::DEFERRED, typed("true", v::ext::XSD_BOOLEAN));
        }
        if let Some(author) = &self.author {
            push(v::AUTHOR, plain(author));
        }
        if let Some(revision) = &self.revision {
            push(v::REVISION, plain(revision));
        }
        if let Some(holder) = &self.claimed_by {
            push(v::CLAIMED_BY, plain(holder));
        }
        if let Some(purpose) = &self.purpose {
            push(v::PURPOSE, plain(purpose));
        }
        push(
            v::ext::CREATED,
            typed(&sparql::iso8601(self.created), v::ext::XSD_DATETIME),
        );
        push(
            v::ext::MODIFIED,
            typed(&sparql::iso8601(self.modified), v::ext::XSD_DATETIME),
        );
        for label in &self.labels {
            push(v::LABEL, plain(label));
        }
        for (predicate, values) in [
            (v::ABOUT, &self.about),
            (v::BLOCKS, &self.blocks),
            (v::PARENT, &self.parents),
            (v::RELATED, &self.related),
        ] {
            for value in values {
                if let Ok(node) = NamedNode::new(value) {
                    push(predicate, node.into());
                }
            }
        }
    }
}

/// One comment on an item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    /// The stable IRI.
    pub iri: String,
    /// The item it is on.
    pub on_item: String,
    /// The text.
    pub body: String,
    /// Who wrote it.
    pub author: Option<String>,
    /// Milliseconds since the epoch.
    pub created: u64,
}

impl Comment {
    /// One block of text, stamped and attributed.
    pub fn render(&self) -> String {
        format!(
            "  {} {}\n{}\n",
            sparql::iso8601(self.created),
            self.author.as_deref().unwrap_or("(unattributed)"),
            self.body
                .lines()
                .map(|l| format!("    {l}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    }

    /// This comment's triples, for the Turtle face.
    pub fn triples(&self, graph: &mut Graph) {
        let (Ok(subject), Ok(item)) = (NamedNode::new(&self.iri), NamedNode::new(&self.on_item))
        else {
            return;
        };
        let mut push = |predicate: &str, object: oxrdf::Term| {
            if let Ok(p) = NamedNode::new(predicate) {
                graph.insert(&Triple::new(subject.clone(), p, object));
            }
        };
        push(v::ext::TYPE, named(v::COMMENT_CLASS));
        push(v::ON_ITEM, item.into());
        push(v::BODY, plain(&self.body));
        if let Some(author) = &self.author {
            push(v::AUTHOR, plain(author));
        }
        push(
            v::ext::CREATED,
            typed(&sparql::iso8601(self.created), v::ext::XSD_DATETIME),
        );
    }
}

/// A `NamedNode` term.
pub(crate) fn named(iri: &str) -> oxrdf::Term {
    NamedNode::new(iri).expect("a constant IRI").into()
}

/// A plain string literal term.
pub(crate) fn plain(text: &str) -> oxrdf::Term {
    Literal::new_simple_literal(text).into()
}

/// A typed literal term.
pub(crate) fn typed(lexical: &str, datatype: &str) -> oxrdf::Term {
    Literal::new_typed_literal(lexical, NamedNode::new(datatype).expect("a constant IRI")).into()
}

/// Serialize a graph as Turtle with this module's prefixes.
///
/// ⚠ Serialized, never formatted by hand. A title carrying a quote, a newline or a
/// backslash is ordinary ledger content, and the one thing here that is not guessing
/// about how to write it is `oxrdfio`.
pub fn turtle(graph: &Graph) -> Result<Vec<u8>> {
    let mut serializer = oxrdfio::RdfSerializer::from_format(oxrdfio::RdfFormat::Turtle);
    for (prefix, iri) in [
        ("ledger", v::LEDGER_NS),
        ("dcterms", "http://purl.org/dc/terms/"),
        ("prov", "http://www.w3.org/ns/prov#"),
        ("sig", v::ext::SIGN_NS),
    ] {
        serializer = serializer
            .with_prefix(prefix, iri)
            .map_err(|e| Error::Endpoint(format!("prefix {prefix}: {e}")))?;
    }
    let mut writer = serializer.for_writer(Vec::new());
    // Sorted, so the same graph serializes to the same bytes — which is what makes a
    // cached representation a byte-identical cache hit rather than a coin flip.
    let mut triples: Vec<_> = graph.iter().map(|t| t.into_owned()).collect();
    triples.sort_by_key(|t| format!("{} {} {}", t.subject, t.predicate, t.object));
    for triple in &triples {
        writer
            .serialize_triple(triple)
            .map_err(|e| Error::Endpoint(format!("serializing Turtle: {e}")))?;
    }
    writer
        .finish()
        .map_err(|e| Error::Endpoint(format!("serializing Turtle: {e}")))
}

// ------------------------------------------------------------------ reading the store

/// The `GRAPH <…> { … }` wrapper every ledger query carries.
fn in_graph(body: &str) -> String {
    format!("GRAPH <{}> {{ {body} }}", v::GRAPH)
}

/// The WHERE-clause fragment a [`Filter`] becomes.
fn filter_clauses(filter: &Filter) -> Result<String> {
    let mut clauses = String::new();
    if let Some(kind) = &filter.kind {
        clauses.push_str(&format!(
            "?item <{}> {} .\n",
            v::ext::TYPE,
            sparql::iri_term(kind, "kind")?
        ));
    }
    match filter.status {
        Status::Open => clauses.push_str(&format!("?item <{}> <{}> .\n", v::STATUS, v::OPEN)),
        Status::Closed => clauses.push_str(&format!("?item <{}> <{}> .\n", v::STATUS, v::CLOSED)),
        Status::All => {}
    }
    for label in &filter.labels {
        clauses.push_str(&format!("?item <{}> {} .\n", v::LABEL, literal(label)));
    }
    for label in &filter.without {
        clauses.push_str(&format!(
            "FILTER NOT EXISTS {{ ?item <{}> {} }}\n",
            v::LABEL,
            literal(label)
        ));
    }
    if let Some(about) = &filter.about {
        clauses.push_str(&format!(
            "?item <{}> {} .\n",
            v::ABOUT,
            sparql::iri_term(about, "about")?
        ));
    }
    match &filter.holder {
        Holder::Any => {}
        Holder::None => clauses.push_str(&format!(
            "FILTER NOT EXISTS {{ ?item <{}> ?anyHolder }}\n",
            v::CLAIMED_BY
        )),
        Holder::Named(name) => {
            clauses.push_str(&format!("?item <{}> {} .\n", v::CLAIMED_BY, literal(name)))
        }
    }
    match filter.deferred {
        Deferred::Include => {}
        Deferred::Exclude => clauses.push_str(&format!(
            "FILTER NOT EXISTS {{ ?item <{}> true }}\n",
            v::DEFERRED
        )),
        Deferred::Only => clauses.push_str(&format!("?item <{}> true .\n", v::DEFERRED)),
    }
    if let Some(text) = &filter.text {
        clauses.push_str(&format!(
            "FILTER(CONTAINS(LCASE(STR(?title)), LCASE({})))\n",
            literal(text)
        ));
    }
    Ok(clauses)
}

/// Load the items a filter selects, newest-updated first (kata's order, and the one a
/// human reading a list expects).
pub async fn load_items(client: &StoreClient<'_, '_>, filter: &Filter) -> Result<Vec<Item>> {
    let limit = if filter.limit == 0 { 500 } else { filter.limit };
    let query = format!(
        "SELECT ?item ?kind ?number ?title ?body ?status ?reason ?priority ?deferred ?author \
         ?revision ?holder ?purpose ?created ?modified WHERE {{ {} }} \
         ORDER BY DESC(?modified) DESC(?number) LIMIT {limit}",
        in_graph(&format!(
            "?item <{type_}> <{item_class}> ;\n  <{number}> ?number ;\n  <{title}> ?title ;\n  \
             <{status}> ?status ;\n  <{created}> ?created ;\n  <{modified}> ?modified .\n\
             OPTIONAL {{ ?item <{type_}> ?kind . FILTER(?kind != <{item_class}>) }}\n\
             OPTIONAL {{ ?item <{body}> ?body }}\n\
             OPTIONAL {{ ?item <{reason}> ?reason }}\n\
             OPTIONAL {{ ?item <{priority}> ?priority }}\n\
             OPTIONAL {{ ?item <{deferred}> ?deferred }}\n\
             OPTIONAL {{ ?item <{author}> ?author }}\n\
             OPTIONAL {{ ?item <{revision}> ?revision }}\n\
             OPTIONAL {{ ?item <{holder}> ?holder }}\n\
             OPTIONAL {{ ?item <{purpose}> ?purpose }}\n{filters}",
            type_ = v::ext::TYPE,
            item_class = v::ITEM_CLASS,
            number = v::NUMBER,
            title = v::ext::TITLE,
            status = v::STATUS,
            created = v::ext::CREATED,
            modified = v::ext::MODIFIED,
            body = v::BODY,
            reason = v::CLOSED_REASON,
            priority = v::PRIORITY,
            deferred = v::DEFERRED,
            author = v::AUTHOR,
            revision = v::REVISION,
            holder = v::CLAIMED_BY,
            purpose = v::PURPOSE,
            filters = filter_clauses(filter)?,
        ))
    );
    let rows = client.select(&query).await?;
    let mut items: Vec<Item> = rows.iter().filter_map(item_from_row).collect();
    fill_multivalued(client, &mut items).await?;
    Ok(items)
}

/// Load one item by its IRI, with its multi-valued edges.
pub async fn load_item(client: &StoreClient<'_, '_>, iri: &str) -> Result<Option<Item>> {
    let query = format!(
        "SELECT ?item ?kind ?number ?title ?body ?status ?reason ?priority ?deferred ?author \
         ?revision ?holder ?purpose ?created ?modified WHERE {{ {} }} LIMIT 1",
        in_graph(&format!(
            "BIND({subject} AS ?item)\n\
             ?item <{type_}> <{item_class}> ;\n  <{number}> ?number ;\n  <{title}> ?title ;\n  \
             <{status}> ?status ;\n  <{created}> ?created ;\n  <{modified}> ?modified .\n\
             OPTIONAL {{ ?item <{type_}> ?kind . FILTER(?kind != <{item_class}>) }}\n\
             OPTIONAL {{ ?item <{body}> ?body }}\n\
             OPTIONAL {{ ?item <{reason}> ?reason }}\n\
             OPTIONAL {{ ?item <{priority}> ?priority }}\n\
             OPTIONAL {{ ?item <{deferred}> ?deferred }}\n\
             OPTIONAL {{ ?item <{author}> ?author }}\n\
             OPTIONAL {{ ?item <{revision}> ?revision }}\n\
             OPTIONAL {{ ?item <{holder}> ?holder }}\n\
             OPTIONAL {{ ?item <{purpose}> ?purpose }}",
            subject = sparql::iri_term(iri, "item")?,
            type_ = v::ext::TYPE,
            item_class = v::ITEM_CLASS,
            number = v::NUMBER,
            title = v::ext::TITLE,
            status = v::STATUS,
            created = v::ext::CREATED,
            modified = v::ext::MODIFIED,
            body = v::BODY,
            reason = v::CLOSED_REASON,
            priority = v::PRIORITY,
            deferred = v::DEFERRED,
            author = v::AUTHOR,
            revision = v::REVISION,
            holder = v::CLAIMED_BY,
            purpose = v::PURPOSE,
        ))
    );
    let rows = client.select(&query).await?;
    let mut items: Vec<Item> = rows.iter().filter_map(item_from_row).collect();
    fill_multivalued(client, &mut items).await?;
    Ok(items.into_iter().next())
}

/// Resolve `{id}` — either a short number (`12`) or an opaque id (`01k5…`) — to an item
/// IRI. Both are accepted because a human types `#12` and a machine carries the IRI, and
/// refusing the human form would make the resource unusable from the REPL.
pub async fn resolve_id(client: &StoreClient<'_, '_>, id: &str) -> Result<String> {
    if let Ok(number) = id.trim_start_matches('#').parse::<i64>() {
        let query = format!(
            "SELECT ?item WHERE {{ {} }} LIMIT 1",
            in_graph(&format!(
                "?item <{}> {} .",
                v::NUMBER,
                sparql::integer(number)
            ))
        );
        let rows = client.select(&query).await?;
        return rows
            .first()
            .and_then(|row| row.get("item"))
            .map(|binding| binding.value.clone())
            .ok_or_else(|| Error::NotFound(format!("no ledger item #{number}")));
    }
    Ok(format!("{}{id}", v::iri::ITEM))
}

/// Load an item's comments, oldest first — a log is read forwards.
pub async fn load_comments(client: &StoreClient<'_, '_>, item: &str) -> Result<Vec<Comment>> {
    let query = format!(
        "SELECT ?comment ?body ?author ?created WHERE {{ {} }} ORDER BY ?created",
        in_graph(&format!(
            "?comment <{on_item}> {subject} ;\n  <{body}> ?body ;\n  <{created}> ?created .\n\
             OPTIONAL {{ ?comment <{author}> ?author }}",
            on_item = v::ON_ITEM,
            subject = sparql::iri_term(item, "item")?,
            body = v::BODY,
            created = v::ext::CREATED,
            author = v::AUTHOR,
        ))
    );
    Ok(client
        .select(&query)
        .await?
        .iter()
        .filter_map(|row| {
            Some(Comment {
                iri: row.get("comment")?.value.clone(),
                on_item: item.to_string(),
                body: row.get("body")?.value.clone(),
                author: row.get("author").map(|b| b.value.clone()),
                created: millis(row.get("created")?.value.as_str())?,
            })
        })
        .collect())
}

/// Fill in labels, about, blocks, parent and related for a loaded set.
///
/// One extra query rather than `GROUP_CONCAT` in the first: concatenating multi-valued
/// columns means choosing a separator no value can contain, and a label or an IRI can
/// contain anything.
async fn fill_multivalued(client: &StoreClient<'_, '_>, items: &mut [Item]) -> Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    let values = items
        .iter()
        .map(|item| format!("<{}>", item.iri))
        .collect::<Vec<_>>()
        .join(" ");
    let query = format!(
        "SELECT ?item ?p ?o WHERE {{ {} }}",
        in_graph(&format!(
            "VALUES ?item {{ {values} }}\nVALUES ?p {{ <{}> <{}> <{}> <{}> <{}> }}\n?item ?p ?o .",
            v::LABEL,
            v::ABOUT,
            v::BLOCKS,
            v::PARENT,
            v::RELATED,
        ))
    );
    let mut by_item: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for row in client.select(&query).await? {
        let (Some(item), Some(p), Some(o)) = (row.get("item"), row.get("p"), row.get("o")) else {
            continue;
        };
        by_item
            .entry(item.value.clone())
            .or_default()
            .push((p.value.clone(), o.value.clone()));
    }
    for item in items.iter_mut() {
        let Some(pairs) = by_item.get(&item.iri) else {
            continue;
        };
        for (predicate, object) in pairs {
            match predicate.as_str() {
                p if p == v::LABEL => item.labels.push(object.clone()),
                p if p == v::ABOUT => item.about.push(object.clone()),
                p if p == v::BLOCKS => item.blocks.push(object.clone()),
                p if p == v::PARENT => item.parents.push(object.clone()),
                p if p == v::RELATED => item.related.push(object.clone()),
                _ => {}
            }
        }
        item.labels.sort();
        item.about.sort();
        item.blocks.sort();
        item.parents.sort();
        item.related.sort();
    }
    Ok(())
}

/// One row of the item query → an [`Item`], or `None` for a row missing something the
/// model requires (which a ledger written only through these endpoints cannot produce,
/// but a hand-edited store can).
fn item_from_row(row: &Row) -> Option<Item> {
    Some(Item {
        iri: row.get("item")?.value.clone(),
        kind: row.get("kind").map(|b| b.value.clone()),
        number: row.get("number")?.as_i64()?,
        title: row.get("title")?.value.clone(),
        body: row.get("body").map(|b| b.value.clone()).unwrap_or_default(),
        open: row.get("status")?.value == v::OPEN,
        closed_reason: row
            .get("reason")
            .and_then(|b| v::close_reason_name(&b.value))
            .map(str::to_string),
        priority: row.get("priority").and_then(|b| b.as_i64()),
        deferred: row
            .get("deferred")
            .map(|b| b.value == "true")
            .unwrap_or(false),
        author: row.get("author").map(|b| b.value.clone()),
        revision: row.get("revision").map(|b| b.value.clone()),
        claimed_by: row.get("holder").map(|b| b.value.clone()),
        purpose: row.get("purpose").map(|b| b.value.clone()),
        created: millis(row.get("created")?.value.as_str())?,
        modified: millis(row.get("modified")?.value.as_str())?,
        labels: Vec::new(),
        about: Vec::new(),
        blocks: Vec::new(),
        parents: Vec::new(),
        related: Vec::new(),
    })
}

/// An `xsd:dateTime` lexical form → milliseconds since the epoch.
///
/// ⚠ **Tolerant on purpose, and this cost a debugging session.** This module writes
/// `2026-09-13T00:00:00.000Z`, but what comes back out of the store is whatever the SPARQL
/// engine's canonical form is — oxigraph drops a zero fraction, so the value READ is
/// `2026-09-13T00:00:00Z`. A parser that insisted on the form it wrote returned `None`,
/// `item_from_row` dropped the row, and every newly filed item read back as if it had
/// never been written. Accept an optional fraction and an optional numeric offset, which
/// is what `xsd:dateTime` actually permits.
fn millis(iso: &str) -> Option<u64> {
    let text = iso.trim();
    let bytes = text.as_bytes();
    if text.len() < 19 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
        return None;
    }
    let num = |from: usize, to: usize| text.get(from..to)?.parse::<u64>().ok();
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hh, mm, ss) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    let mut rest = &text[19..];
    let mut fraction = 0u64;
    if let Some(after_dot) = rest.strip_prefix('.') {
        let digits: String = after_dot.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() {
            return None;
        }
        let mut ms = digits.clone();
        ms.truncate(3);
        while ms.len() < 3 {
            ms.push('0');
        }
        fraction = ms.parse().ok()?;
        rest = &after_dot[digits.len()..];
    }
    // `Z`, absent (local time, read as UTC), or ±HH:MM.
    let offset_minutes: i64 = match rest {
        "" | "Z" | "z" => 0,
        other => {
            let sign = match other.as_bytes().first()? {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let hours: i64 = other.get(1..3)?.parse().ok()?;
            let minutes: i64 = other.get(4..6)?.parse().ok()?;
            sign * (hours * 60 + minutes)
        }
    };
    let days = days_from_civil(y as i64, mo as u32, d as u32);
    let total = days * 86_400_000 + (hh * 3_600_000 + mm * 60_000 + ss * 1000 + fraction) as i64
        - offset_minutes * 60_000;
    u64::try_from(total).ok()
}

/// (year, month, day) → days since the Unix epoch. Howard Hinnant's `days_from_civil`.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let mp = if m > 2 { m - 3 } else { m + 9 } as u64;
    let doy = (153 * mp + 2) / 5 + d as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item() -> Item {
        Item {
            iri: "urn:iki:ledger:item:01k5x".to_string(),
            kind: None,
            number: 12,
            title: "Fix the thing".to_string(),
            body: "It is broken.".to_string(),
            open: true,
            closed_reason: None,
            priority: Some(1),
            deferred: false,
            author: Some("brian".to_string()),
            revision: Some("f41a333".to_string()),
            claimed_by: None,
            purpose: None,
            created: 1_757_700_000_000,
            modified: 1_757_700_000_000,
            labels: vec!["module".to_string()],
            about: vec!["urn:repo:file:ikigai-cli/src/main.rs".to_string()],
            blocks: Vec::new(),
            parents: Vec::new(),
            related: Vec::new(),
        }
    }

    #[test]
    fn the_line_is_one_line_and_carries_the_number_status_and_priority() {
        let line = item().line();
        assert!(!line.contains('\n'));
        assert!(line.contains("#12"), "{line}");
        assert!(line.contains("open"), "{line}");
        assert!(line.contains("p1"), "{line}");
        assert!(line.contains("[module]"), "{line}");
    }

    /// ★ The canonicalization trap: what the store hands back is not the lexical form
    /// this module wrote.
    #[test]
    fn a_canonicalized_timestamp_still_parses() {
        assert_eq!(millis("2026-09-13T00:00:00Z"), Some(1_789_257_600_000));
        assert_eq!(millis("2026-09-13T00:00:00.000Z"), Some(1_789_257_600_000));
        assert_eq!(millis("2026-09-13T00:00:00.5Z"), Some(1_789_257_600_500));
        assert_eq!(millis("2026-09-13T01:00:00+01:00"), Some(1_789_257_600_000));
        assert_eq!(millis("not a time"), None);
    }

    #[test]
    fn a_timestamp_round_trips_through_the_lexical_form_the_store_holds() {
        let ms = 1_757_700_123_456 % 100_000_000_000;
        assert_eq!(millis(&sparql::iso8601(ms)), Some(ms));
    }

    #[test]
    fn the_turtle_face_has_no_blank_nodes_and_names_the_about_target() {
        let mut graph = Graph::new();
        item().triples(&mut graph);
        let turtle = String::from_utf8(turtle(&graph).unwrap()).unwrap();
        assert!(!turtle.contains("_:"), "{turtle}");
        assert!(
            turtle.contains("urn:repo:file:ikigai-cli/src/main.rs"),
            "{turtle}"
        );
        assert!(turtle.contains("ledger:number"), "{turtle}");
    }

    /// ★ The escaping path, end to end: a title that would break a hand-written
    /// serializer comes back as one literal.
    #[test]
    fn a_hostile_title_survives_serialization() {
        let mut hostile = item();
        hostile.title = "a \"quote\" and a \\ and a\nnewline".to_string();
        let mut graph = Graph::new();
        hostile.triples(&mut graph);
        let bytes = turtle(&graph).unwrap();
        // It parses back, which is the only real assertion available.
        let parsed: Vec<_> = oxrdfio::RdfParser::from_format(oxrdfio::RdfFormat::Turtle)
            .for_reader(bytes.as_slice())
            .collect::<std::result::Result<Vec<_>, _>>()
            .expect("the serialized graph parses");
        assert!(parsed
            .iter()
            .any(|t| t.object.to_string().contains("newline")));
    }

    #[test]
    fn the_filter_puts_readiness_in_the_query() {
        let filter = Filter {
            status: Status::Open,
            holder: Holder::None,
            deferred: Deferred::Exclude,
            labels: vec!["rust".to_string()],
            without: vec!["someday".to_string()],
            ..Filter::default()
        };
        let clauses = filter_clauses(&filter).unwrap();
        assert!(clauses.contains("ledger#open"), "{clauses}");
        assert!(clauses.contains("NOT EXISTS"), "{clauses}");
        assert!(clauses.contains("\"rust\""), "{clauses}");
    }
}
