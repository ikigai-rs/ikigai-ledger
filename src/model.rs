//! What an item is, how one is read out of the store, and the two faces it wears.
//!
//! The filters live HERE, in the query — not in an ordering policy. That is kata's one
//! structural idea worth copying outright: readiness (open, unclaimed, unblocked, not
//! deferred, has these labels, has none of those) is a *filter*, and keeping it there is
//! why a ranking policy can be a dozen lines instead of a rules engine.

use std::collections::BTreeMap;

use ikigai_core::{Error, Result};
use oxrdf::{Graph, Literal, NamedNode, Triple};

use crate::ledger::Ledger;
use crate::sparql::{self, literal, Row, StoreClient};
use crate::vocabulary as v;

/// Which items a read is asking for. Every field narrows; an empty filter is
/// "everything, open first".
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
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
    /// Must carry this `ledger:key` — the caller's own name for the item. At most one item
    /// in a ledger carries a given key, so this is a lookup wearing a filter's clothes; it
    /// is a filter so that `items key=` composes with the others (`status` included: the
    /// default `open` hides a closed item, exactly as it does for `about`).
    pub key: Option<String>,
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
#[non_exhaustive]
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
    /// The caller's own name for the item (`append key=`), unique per ledger.
    pub key: Option<String>,
    /// Who holds it, if anyone.
    pub claimed_by: Option<String>,
    /// Why the holder took it.
    pub purpose: Option<String>,
    /// Milliseconds since the epoch.
    ///
    /// ⚠ **A stand-in when [`defects`](Item::defects) names `dcterms:created`**: the item's
    /// `dcterms:modified` in its place, which is the latest it can have been filed.
    pub created: u64,
    /// Milliseconds since the epoch.
    ///
    /// ⚠ **A stand-in when [`defects`](Item::defects) names `dcterms:modified`**: the
    /// item's `dcterms:created` in its place, which is the earliest it can have changed.
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
    /// What the reader read AROUND to list this item — `unreadable dcterms:modified
    /// "yesterday"`, `missing dcterms:created` — in the words [`defects`] uses. Empty for
    /// every item written only through a ledger Sink.
    ///
    /// ★ **One malformed timestamp never drops an item** (ledger #866). Through 0.4.1 an
    /// out-of-band write of `dcterms:modified "yesterday"` took the item out of `items`
    /// (reported in a footer), out of `next` (silently — and with it every `blocks` edge it
    /// carried, so what it blocked was offered as ready), and out of the Turtle face. Now
    /// an unreadable or missing timestamp is treated as ABSENT and the other one stands in
    /// for it, and this list says so on every face. Only an item with NO readable
    /// timestamp is still unlistable, because no time it could be shown at is true.
    pub defects: Vec<String>,
    /// The NAMES of the lifecycle states the item holds (`queued`), sorted — empty means the
    /// state named `filed`. Through a ledger Sink there is at most one, because the state
    /// resource sets it by compare-and-set in one store update; two can only come from an
    /// out-of-band write, which the doctor reports. [`Item::state`] reads it.
    pub states: Vec<String>,
    /// The `ledger:state` values as stored, for the Turtle face and for comparing against a
    /// lifecycle's IRIs (a name alone would match another lifecycle's state of that name).
    pub(crate) state_values: Vec<sparql::Binding>,
    /// The unreadable timestamp values themselves, so the Turtle face can carry them
    /// as stored instead of asserting the stand-in as `dcterms:created`/`modified`.
    pub(crate) unread: Vec<(&'static str, sparql::Binding)>,
    /// Whether `created` / `modified` were read, rather than stood in for.
    pub(crate) read: (bool, bool),
}

impl Item {
    /// The one state this item is in: [`FILED`](crate::lifecycle::FILED) when it holds none,
    /// the state's name when it holds one, and `None` when it holds several — which no ledger
    /// write produces, and which the doctor reports.
    pub fn state(&self) -> Option<&str> {
        match self.states.as_slice() {
            [] => Some(crate::lifecycle::FILED),
            [one] => Some(one),
            _ => None,
        }
    }

    /// The `ledger:state` values exactly as stored — IRIs for every state a transition wrote.
    pub fn state_iris(&self) -> Vec<&str> {
        self.state_values.iter().map(|b| b.value.as_str()).collect()
    }

    /// The display number: `#12` in the default ledger, `acme#12` elsewhere.
    ///
    /// ★ Read from the item's own IRI rather than passed in, because the IRI is
    /// canonical: an item minted in `acme` carries `acme` in its name forever, so its
    /// display form cannot drift from the ledger it is actually in.
    pub fn short(&self) -> String {
        crate::ledger::Ledger::of_subject(&self.iri)
            .unwrap_or_default()
            .number(self.number)
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
        // Only when there is one, so every item filed without a key renders exactly as it
        // did in 0.3.0 — the gonk bridges parse this face.
        if let Some(key) = &self.key {
            out.push_str(&format!("  key:      {key}\n"));
        }
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
        // Only when there is one, so an item written through the Sink renders exactly as
        // before. The sentence says which line above is a stand-in, not merely that one is.
        for defect in &self.defects {
            out.push_str(&format!("  ⚠ defect: {defect}\n"));
        }
        match self.read {
            (false, _) => out.push_str("  ⚠ `filed` above is dcterms:modified standing in\n"),
            (_, false) => out.push_str("  ⚠ `updated` above is dcterms:created standing in\n"),
            _ => {}
        }
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
        // Only when there is one, like `key:`, so an item that never entered a lifecycle
        // renders exactly as it did in 0.4.2.
        for state in &self.states {
            out.push_str(&format!("  state:    {state}\n"));
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
        if let Some(key) = &self.key {
            push(v::KEY, plain(key));
        }
        if let Some(holder) = &self.claimed_by {
            push(v::CLAIMED_BY, plain(holder));
        }
        if let Some(purpose) = &self.purpose {
            push(v::PURPOSE, plain(purpose));
        }
        // ⚠ A stand-in is never asserted: the graph face says what the store holds, so an
        // unreadable value goes out AS STORED and a missing one goes out as nothing.
        if self.read.0 {
            push(
                v::ext::CREATED,
                typed(&sparql::iso8601(self.created), v::ext::XSD_DATETIME),
            );
        }
        if self.read.1 {
            push(
                v::ext::MODIFIED,
                typed(&sparql::iso8601(self.modified), v::ext::XSD_DATETIME),
            );
        }
        for (predicate, value) in &self.unread {
            if let Some(term) = stored(value) {
                push(predicate, term);
            }
        }
        // As stored: a state is an IRI when a transition wrote it, and whatever an
        // out-of-band write left when one did not.
        for value in &self.state_values {
            if let Some(term) = stored(value) {
                push(v::STATE, term);
            }
        }
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

/// A value read out of the store, as the store holds it — for a value this crate cannot
/// parse and so must not restate. A blank node is `None`: its label is scoped to the result
/// set it came in, so restating it would name a different node.
fn stored(value: &sparql::Binding) -> Option<oxrdf::Term> {
    match value.kind.as_str() {
        "uri" => NamedNode::new(&value.value).ok().map(Into::into),
        "bnode" => None,
        _ => match (&value.lang, &value.datatype) {
            (Some(tag), _) => Literal::new_language_tagged_literal(&value.value, tag)
                .ok()
                .map(Into::into),
            (None, Some(datatype)) => NamedNode::new(datatype)
                .ok()
                .map(|d| Literal::new_typed_literal(&value.value, d).into()),
            (None, None) => Some(plain(&value.value)),
        },
    }
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

/// The WHERE-clause fragment a [`Filter`] becomes.
///
/// # ⚠ Why these values are still built as terms and not passed as `bindings=`
///
/// `urn:iki:store:graph-select` takes a `bindings=` argument where a value never reaches
/// the SPARQL parser at all — strictly stronger than building a term, and the right default
/// for a query. This function does not use it, and the reason is one clause:
///
/// **`FILTER NOT EXISTS` is unreachable by binding.** Measured against `ikigai-store` 0.2.2
/// on 2026-09-13: a variable occurring only inside `FILTER NOT EXISTS { … }` is not in the
/// projection *even under `SELECT *`*, and the endpoint **refuses** the binding rather than
/// ignoring it. The refusal is the right behaviour — a filter you thought was applied can
/// never silently not be — but it means the `without` filter here (`FILTER NOT EXISTS
/// { ?item ledger:label "x" }`) cannot take that door at all.
///
/// So adopting it would bind `kind`, `about`, `labels` and `holder` and interpolate
/// `without`, leaving **two mechanisms in one query string** — and a reader could no longer
/// tell which path a value took by looking at it. One mechanism used everywhere is worth
/// more here than a stronger one used in most places, because the value of the strong door
/// is that you never have to check. Every value below becomes an RDF term through
/// `ikigai_store::sparql`, which is the crate that owns the grammar, and the hostile-content
/// test in `crate::sparql` pins the composition.
///
/// What would change the answer: a way to bind into a `NOT EXISTS` subpattern (upstream, in
/// oxigraph), or dropping `without` in favour of a shape this crate writes differently —
/// `MINUS`, which has the same scoping problem, or a client-side exclusion, which moves work
/// out of the store for no gain.
fn filter_clauses(filter: &Filter) -> Result<String> {
    let mut clauses = String::new();
    if let Some(kind) = &filter.kind {
        clauses.push_str(&format!(
            "?item <{}> {} .\n",
            v::ext::TYPE,
            sparql::iri(kind, "kind")?
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
            sparql::iri(about, "about")?
        ));
    }
    if let Some(key) = &filter.key {
        clauses.push_str(&format!("?item <{}> {} .\n", v::KEY, literal(key)));
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

/// The properties an item must carry for anything here to read it.
///
/// ★ **Assume an editor got there first.** A ledger whose items can only be changed
/// through its own Sink is not the thing anyone wants: the point of durable, inspectable
/// state is that a human in an editor — or an LLM harness, or a merge — can touch it out
/// of band, and this backend is no different (`urn:iki:store:graph-update` is open to
/// anyone holding this graph's write grant, and `urn:iki:store:{update,load}` to anyone
/// holding the store's broad one). So an
/// out-of-band write is a FIRST-CLASS PATH, not corruption, and the consequence is that
/// the model has to be checked on READ as well as on write: the Sink's refusal never ran.
///
/// This list is that hook, used by both directions — the reader ([`defects`]) and any
/// future lint over the corpus.
pub const REQUIRED: [&str; 5] = [
    v::NUMBER,
    v::ext::TITLE,
    v::STATUS,
    v::ext::CREATED,
    v::ext::MODIFIED,
];

/// Items in the graph that the readers here cannot see, and what is wrong with each —
/// `missing ledger:number`, or `unreadable ledger:number "twelve"`.
///
/// Reported rather than silently skipped: an item a hand-edit made unreadable would
/// otherwise vanish from every listing while still being *in* the ledger, which is the
/// worst of both — the work is neither visible nor gone.
///
/// ⚠ **Present is not the same as readable.** Until 0.2.1 this asked only whether each
/// required property was there. An editor writing `dcterms:created` as an `xsd:date` — a
/// perfectly good RDF date — left it present, so nothing was reported, and the reader,
/// which needs an `xsd:dateTime`, dropped the item: gone from `items` and `next`, and "no
/// ledger item" from its own IRI, while it sat in the graph. So the values the reader
/// parses are parsed here too, by the same functions.
///
/// ★ **And readable is not the same as listable** (ledger #866). Since 0.4.2 a timestamp
/// the reader cannot use is treated as absent and the other one stands in for it, so an
/// item with ONE readable timestamp is listed — carrying its defects in
/// [`Item::defects`] — and is not reported here. What is reported here is exactly what
/// the readers drop: no title or status, no readable `ledger:number` (the item's name in
/// every face, which nothing can stand in for), or no readable timestamp at all. Each such
/// item is reported with EVERY defect it has, so one edit can fix it.
pub async fn defects(client: &StoreClient<'_, '_>) -> Result<Vec<(String, Vec<String>)>> {
    /// What one subject has, as the two queries below see it.
    #[derive(Default)]
    struct Seen {
        defects: Vec<String>,
        unnamed: bool,
        number: bool,
        time: bool,
    }
    let values = REQUIRED
        .iter()
        .map(|p| format!("<{p}>"))
        .collect::<Vec<_>>()
        .join(" ");
    let query = format!(
        "SELECT ?item ?missing WHERE {{ {} }} ORDER BY ?item ?missing",
        client.in_graph(&format!(
            "?item <{type_}> <{class}> .\nVALUES ?missing {{ {values} }}\n\
             FILTER NOT EXISTS {{ ?item ?missing ?any }}",
            type_ = v::ext::TYPE,
            class = v::ITEM_CLASS,
        ))
    );
    let mut by_item: BTreeMap<String, Seen> = BTreeMap::new();
    for row in client.select(&query).await? {
        let (Some(item), Some(missing)) = (row.get("item"), row.get("missing")) else {
            continue;
        };
        let seen = by_item.entry(item.value.clone()).or_default();
        seen.defects
            .push(format!("missing {}", short_name(&missing.value)));
        if missing.value == v::ext::TITLE || missing.value == v::STATUS {
            seen.unnamed = true;
        }
    }
    // The values the reader parses, parsed. A property with several values (another
    // hand edit) is checked value by value.
    let query = format!(
        "SELECT ?item ?p ?value WHERE {{ {} }} ORDER BY ?item ?p ?value",
        client.in_graph(&format!(
            "?item <{type_}> <{class}> .\nVALUES ?p {{ <{number}> <{created}> <{modified}> }}\n\
             ?item ?p ?value .",
            type_ = v::ext::TYPE,
            class = v::ITEM_CLASS,
            number = v::NUMBER,
            created = v::ext::CREATED,
            modified = v::ext::MODIFIED,
        ))
    );
    for row in client.select(&query).await? {
        let (Some(item), Some(p), Some(value)) = (row.get("item"), row.get("p"), row.get("value"))
        else {
            continue;
        };
        let seen = by_item.entry(item.value.clone()).or_default();
        let readable = if p.value == v::NUMBER {
            let readable = value.as_i64().is_some();
            seen.number |= readable;
            readable
        } else {
            let readable = millis(&value.value).is_some();
            seen.time |= readable;
            readable
        };
        if !readable {
            seen.defects.push(unreadable(&p.value, &value.value));
        }
    }
    Ok(by_item
        .into_iter()
        .filter(|(_, seen)| seen.unnamed || !seen.number || !seen.time)
        .map(|(item, seen)| (item, seen.defects))
        .collect())
}

/// One unreadable value, in the words every face reports it in.
fn unreadable(predicate: &str, value: &str) -> String {
    format!("unreadable {} {value:?}", short_name(predicate))
}

/// A predicate IRI as a reader recognizes it.
fn short_name(iri: &str) -> String {
    match iri {
        p if p == v::NUMBER => "ledger:number".to_string(),
        p if p == v::STATUS => "ledger:status".to_string(),
        p if p == v::ext::TITLE => "dcterms:title".to_string(),
        p if p == v::ext::CREATED => "dcterms:created".to_string(),
        p if p == v::ext::MODIFIED => "dcterms:modified".to_string(),
        other => other.to_string(),
    }
}

/// What is wrong with one subject, for the error a single-item read gives.
pub async fn defects_of(client: &StoreClient<'_, '_>, iri: &str) -> Result<Vec<String>> {
    Ok(defects(client)
        .await?
        .into_iter()
        .find(|(item, _)| item == iri)
        .map(|(_, missing)| missing)
        .unwrap_or_default())
}

/// Load the items a filter selects, newest-updated first (kata's order, and the one a
/// human reading a list expects). `limit: 0` means 500 — a LISTING's bound, for a human
/// reading down a page.
///
/// ⚠ **Never the pool for a computation over the ledger.** A bound that is right for a
/// page is wrong for "is this blocked": `urn:iki:ledger:next` read its pool through here
/// until 0.2.1, so with more than 500 open items the oldest fell out — an older p0 was never
/// offered, and an item whose blocker fell out was offered as ready. That reads
/// [`load_open_items`], which has no bound.
pub async fn load_items(client: &StoreClient<'_, '_>, filter: &Filter) -> Result<Vec<Item>> {
    let limit = if filter.limit == 0 { 500 } else { filter.limit };
    load(client, filter, Some(limit)).await
}

/// **Every** open item in the ledger, unfiltered and unbounded — the pool a computation
/// over the whole `blocks` graph needs (readiness, cycles, leverage), because each of
/// those is a property of the ledger and not of a page of it.
pub async fn load_open_items(client: &StoreClient<'_, '_>) -> Result<Vec<Item>> {
    load(client, &Filter::default(), None).await
}

/// The IRIs a filter admits, unbounded — so a caller can compute over the whole ledger
/// and narrow afterwards, rather than computing over the narrowed set.
///
/// ★ The same clauses `urn:iki:ledger:items` filters with (`filter_clauses`), so "the
/// items labeled `rust`" means the same thing to `next` as to a listing.
pub async fn matching_iris(
    client: &StoreClient<'_, '_>,
    filter: &Filter,
) -> Result<std::collections::BTreeSet<String>> {
    let query = format!(
        "SELECT DISTINCT ?item WHERE {{ {} }}",
        client.in_graph(&format!(
            "?item <{type_}> <{item_class}> ; <{title}> ?title .\n{filters}",
            type_ = v::ext::TYPE,
            item_class = v::ITEM_CLASS,
            title = v::ext::TITLE,
            filters = filter_clauses(filter)?,
        ))
    );
    Ok(client
        .select(&query)
        .await?
        .iter()
        .filter_map(|row| row.get("item").map(|b| b.value.clone()))
        .collect())
}

async fn load(
    client: &StoreClient<'_, '_>,
    filter: &Filter,
    limit: Option<usize>,
) -> Result<Vec<Item>> {
    let limit = limit.map(|n| format!(" LIMIT {n}")).unwrap_or_default();
    let query = format!(
        "SELECT ?item ?kind ?number ?title ?body ?status ?reason ?priority ?deferred ?author \
         ?revision ?key ?holder ?purpose ?created ?modified ?state WHERE {{ {} }} \
         ORDER BY DESC({RECENCY}) DESC(?number){limit}",
        client.in_graph(&format!(
            "?item <{type_}> <{item_class}> ;\n  <{number}> ?number ;\n  <{title}> ?title ;\n  \
             <{status}> ?status .\n\
             OPTIONAL {{ ?item <{created}> ?created }}\n\
             OPTIONAL {{ ?item <{modified}> ?modified }}\n\
             OPTIONAL {{ ?item <{type_}> ?kind . FILTER(?kind != <{item_class}>) }}\n\
             OPTIONAL {{ ?item <{body}> ?body }}\n\
             OPTIONAL {{ ?item <{reason}> ?reason }}\n\
             OPTIONAL {{ ?item <{priority}> ?priority }}\n\
             OPTIONAL {{ ?item <{deferred}> ?deferred }}\n\
             OPTIONAL {{ ?item <{author}> ?author }}\n\
             OPTIONAL {{ ?item <{revision}> ?revision }}\n\
             OPTIONAL {{ ?item <{key}> ?key }}\n\
             OPTIONAL {{ ?item <{holder}> ?holder }}\n\
             OPTIONAL {{ ?item <{purpose}> ?purpose }}\n\
             OPTIONAL {{ ?item <{state}> ?state }}\n{filters}",
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
            key = v::KEY,
            holder = v::CLAIMED_BY,
            purpose = v::PURPOSE,
            state = v::STATE,
            filters = filter_clauses(filter)?,
        ))
    );
    let rows = client.select(&query).await?;
    let mut items = items_from_rows(&rows);
    fill_multivalued(client, &mut items).await?;
    Ok(items)
}

/// Load one item by its IRI, with its multi-valued edges.
pub async fn load_item(client: &StoreClient<'_, '_>, iri: &str) -> Result<Option<Item>> {
    let query = format!(
        "SELECT ?item ?kind ?number ?title ?body ?status ?reason ?priority ?deferred ?author \
         ?revision ?key ?holder ?purpose ?created ?modified ?state WHERE {{ {} }} \
         ORDER BY DESC({RECENCY})",
        client.in_graph(&format!(
            "BIND({subject} AS ?item)\n\
             ?item <{type_}> <{item_class}> ;\n  <{number}> ?number ;\n  <{title}> ?title ;\n  \
             <{status}> ?status .\n\
             OPTIONAL {{ ?item <{created}> ?created }}\n\
             OPTIONAL {{ ?item <{modified}> ?modified }}\n\
             OPTIONAL {{ ?item <{type_}> ?kind . FILTER(?kind != <{item_class}>) }}\n\
             OPTIONAL {{ ?item <{body}> ?body }}\n\
             OPTIONAL {{ ?item <{reason}> ?reason }}\n\
             OPTIONAL {{ ?item <{priority}> ?priority }}\n\
             OPTIONAL {{ ?item <{deferred}> ?deferred }}\n\
             OPTIONAL {{ ?item <{author}> ?author }}\n\
             OPTIONAL {{ ?item <{revision}> ?revision }}\n\
             OPTIONAL {{ ?item <{key}> ?key }}\n\
             OPTIONAL {{ ?item <{holder}> ?holder }}\n\
             OPTIONAL {{ ?item <{purpose}> ?purpose }}\n\
             OPTIONAL {{ ?item <{state}> ?state }}",
            subject = sparql::iri(iri, "item")?,
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
            key = v::KEY,
            holder = v::CLAIMED_BY,
            purpose = v::PURPOSE,
            state = v::STATE,
        ))
    );
    let rows = client.select(&query).await?;
    let mut items = items_from_rows(&rows);
    fill_multivalued(client, &mut items).await?;
    Ok(items.into_iter().next())
}

/// The number in a short reference — `12`, `#12`, `acme#12` — or `None` when the reference
/// is not a number at all (an opaque id).
///
/// ⚠ **A number qualified with another ledger's name is refused, not looked up.**
/// Numbers restart per ledger, so `acme#12` and `#12` are different items; silently
/// resolving `acme#12` inside the default ledger would act on the wrong one, which is the
/// worst available answer.
fn number_in(ledger: &Ledger, id: &str) -> Result<Option<i64>> {
    let bare = match id.split_once('#') {
        Some((name, rest)) if !name.is_empty() => {
            if name != ledger.name() {
                return Err(Error::InvalidArgument {
                    name: "item".to_string(),
                    detail: format!(
                        "`{id}` names the ledger `{name}`, and this resource is \
                         `{}`. Numbers restart per ledger, so they are different items — \
                         address it at `urn:iki:ledger:{name}:…` instead",
                        ledger.name()
                    ),
                });
            }
            rest
        }
        _ => id.trim_start_matches('#'),
    };
    Ok(bare.parse::<i64>().ok())
}

/// The longest a key may be. Generous for a foreign id (`urn:roborev:finding:` and 32 hex
/// digits is 52), and bounded because a key is stored, indexed and echoed in every answer.
pub const MAX_KEY: usize = 256;

/// The spelling that makes an item reference a KEY rather than a number or an opaque id:
/// `key:{key}` as `item=`, and `urn:iki:ledger:{ledger}:item:key:{key}` as a resource.
///
/// Unambiguous by construction: an opaque id is Crockford base32 and a number is digits, so
/// neither can contain a `:`.
pub const KEY_PREFIX: &str = "key:";

/// Validate a caller's key — the name `append key=` files under and every lookup finds by.
///
/// ASCII letters, digits, `-`, `.`, `_`, `~` and `:`; 1 to [`MAX_KEY`] characters. Compared
/// exactly, case included, and never rewritten: a key that came back different from the one
/// sent would defeat the only thing it is for.
///
/// ⚠ **The shape is fixed so the key is a legal IRI segment AND survives a URL path.** It is
/// addressable as `urn:iki:ledger:item:key:{key}`, and gonk's HTTP door maps an IRI to a
/// path by turning every `:` into a `/` — so a `/` in a key would come back as a `:`, a
/// different key. `#` is refused for the same family of reason: it starts a comment in the
/// engine's grammar, and an IRI fragment in a URL. Both foreign ids the gonk bridges carry
/// (`urn:kata:issue:{uid}`, `urn:roborev:finding:{hex}`) fit as they stand. Widening the set
/// later is additive; narrowing it would strand keys already filed, which is why it starts
/// narrow.
///
/// ```
/// use ikigai_ledger::model::parse_key;
/// assert_eq!(parse_key("urn:kata:issue:01JZ0ABC").unwrap(), "urn:kata:issue:01JZ0ABC");
/// assert!(parse_key("").is_err());
/// assert!(parse_key("a/b").is_err());     // would not survive gonk's path mapping
/// assert!(parse_key("a b").is_err());
/// assert!(parse_key("a#b").is_err());     // a comment in the engine grammar
/// assert!(parse_key(&"k".repeat(257)).is_err());
/// ```
pub fn parse_key(key: &str) -> Result<String> {
    let bad = |detail: String| Error::InvalidArgument {
        name: "key".to_string(),
        detail,
    };
    if key.is_empty() {
        return Err(bad(
            "an empty key names nothing; omit `key` to file without one".to_string(),
        ));
    }
    if key.len() > MAX_KEY {
        return Err(bad(format!(
            "a key is at most {MAX_KEY} characters, and this one is {}",
            key.len()
        )));
    }
    if let Some(c) = key
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~' | ':')))
    {
        return Err(bad(format!(
            "`{key}` contains {c:?}; a key is ASCII letters, digits, `-`, `.`, `_`, `~` and \
             `:`, so that it is a legal IRI segment (`urn:iki:ledger:item:key:{{key}}`) and \
             survives a URL path unchanged"
        )));
    }
    Ok(key.to_string())
}

/// What carries a key in a ledger's live graph: the item, or — once the item is deleted —
/// its tombstone, which keeps the key so it stays taken.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Keyed {
    /// The item's IRI (for a deleted item, the IRI it had).
    pub item: String,
    /// The item's number.
    pub number: i64,
    /// The tombstone's IRI when the item was deleted; `None` while it is live.
    pub tombstone: Option<String>,
}

/// The item (or the deleted item's tombstone) carrying `key` in this client's ledger.
///
/// A live item wins over a tombstone, and the lowest IRI over the rest: at most one subject
/// can carry a key through this crate's own writes, so a tie is an out-of-band edit, and
/// the answer is at least the same one every time.
pub async fn find_by_key(client: &StoreClient<'_, '_>, key: &str) -> Result<Option<Keyed>> {
    let query = format!(
        "SELECT ?s ?class ?number ?deleted WHERE {{ {} }} ORDER BY ?class ?s LIMIT 1",
        client.in_graph(&format!(
            "?s <{key_p}> {key} ; <{type_}> ?class ; <{number}> ?number .\n\
             VALUES ?class {{ <{item}> <{tombstone}> }}\n\
             OPTIONAL {{ ?s <{deleted}> ?deleted }}",
            key_p = v::KEY,
            key = literal(key),
            type_ = v::ext::TYPE,
            number = v::NUMBER,
            item = v::ITEM_CLASS,
            tombstone = v::TOMBSTONE_CLASS,
            deleted = v::DELETED_ITEM,
        ))
    );
    Ok(client.select(&query).await?.first().and_then(|row| {
        let subject = row.get("s")?.value.clone();
        let number = row.get("number")?.as_i64()?;
        if row.get("class")?.value == v::TOMBSTONE_CLASS {
            Some(Keyed {
                item: row.get("deleted")?.value.clone(),
                number,
                tombstone: Some(subject),
            })
        } else {
            Some(Keyed {
                item: subject,
                number,
                tombstone: None,
            })
        }
    }))
}

/// The numbers of whichever of `iris` are items in this client's ledger — how a link's
/// target is named in the JSON face. An IRI that is not a live item is simply absent.
pub async fn numbers_of(
    client: &StoreClient<'_, '_>,
    iris: &std::collections::BTreeSet<String>,
) -> Result<BTreeMap<String, i64>> {
    if iris.is_empty() {
        return Ok(BTreeMap::new());
    }
    let values = iris
        .iter()
        .map(|iri| sparql::iri(iri, "item"))
        .collect::<Result<Vec<_>>>()?
        .join(" ");
    let query = format!(
        "SELECT ?i ?n WHERE {{ {} }}",
        client.in_graph(&format!(
            "VALUES ?i {{ {values} }}\n?i <{}> <{}> ; <{}> ?n .",
            v::ext::TYPE,
            v::ITEM_CLASS,
            v::NUMBER
        ))
    );
    Ok(client
        .select(&query)
        .await?
        .iter()
        .filter_map(|row| Some((row.get("i")?.value.clone(), row.get("n")?.as_i64()?)))
        .collect())
}

/// The comments on several items at once, each item's oldest first (ties broken by the
/// comment's IRI, whose id is time-ordered) — one query for a whole page of the JSON face.
pub async fn load_comments_for(
    client: &StoreClient<'_, '_>,
    items: &[String],
) -> Result<BTreeMap<String, Vec<Comment>>> {
    let mut by_item: BTreeMap<String, Vec<Comment>> = BTreeMap::new();
    if items.is_empty() {
        return Ok(by_item);
    }
    let values = items
        .iter()
        .map(|iri| sparql::iri(iri, "item"))
        .collect::<Result<Vec<_>>>()?
        .join(" ");
    let query = format!(
        "SELECT ?item ?comment ?body ?author ?created WHERE {{ {} }} ORDER BY ?created ?comment",
        client.in_graph(&format!(
            "VALUES ?item {{ {values} }}\n\
             ?comment <{on_item}> ?item ;\n  <{body}> ?body ;\n  <{created}> ?created .\n\
             OPTIONAL {{ ?comment <{author}> ?author }}",
            on_item = v::ON_ITEM,
            body = v::BODY,
            created = v::ext::CREATED,
            author = v::AUTHOR,
        ))
    );
    for row in client.select(&query).await? {
        let Some(comment) = (|| {
            Some(Comment {
                iri: row.get("comment")?.value.clone(),
                on_item: row.get("item")?.value.clone(),
                body: row.get("body")?.value.clone(),
                author: row.get("author").map(|b| b.value.clone()),
                created: millis(row.get("created")?.value.as_str())?,
            })
        })() else {
            continue;
        };
        by_item
            .entry(comment.on_item.clone())
            .or_default()
            .push(comment);
    }
    Ok(by_item)
}

/// Resolve `{id}` — a short number (`12`, `#12`, `acme#12`) or an opaque id (`01k5…`) —
/// to an item IRI **in this client's ledger**. All the forms are accepted because a human
/// types the number and a machine carries the IRI, and refusing the human form would make
/// the resource unusable from the REPL.
///
/// ⚠ **A number qualified with another ledger's name is refused, not looked up.**
/// Numbers restart per ledger, so `acme#12` and `#12` are different items; silently
/// resolving `acme#12` inside the default ledger would act on the wrong one, which is the
/// worst available answer.
pub async fn resolve_id(client: &StoreClient<'_, '_>, id: &str) -> Result<String> {
    let ledger = client.ledger();
    if let Some(key) = id.strip_prefix(KEY_PREFIX) {
        let key = parse_key(key)?;
        return match find_by_key(client, &key).await? {
            Some(Keyed {
                item,
                tombstone: None,
                ..
            }) => Ok(item),
            Some(Keyed {
                number,
                tombstone: Some(tombstone),
                ..
            }) => Err(Error::NotFound(format!(
                "{} (key `{key}`) was deleted: its tombstone is `{tombstone}` and its content, \
                 unless purged, is in `{}`. The key stays taken",
                ledger.number(number),
                ledger.deleted_graph()
            ))),
            None => Err(Error::NotFound(format!(
                "no ledger item with the key `{key}` in `{}`",
                ledger.name()
            ))),
        };
    }
    if let Some(number) = number_in(ledger, id)? {
        // ⚠ **An ITEM with that number.** A tombstone carries its item's number too, so
        // the bare pattern resolved a deleted item's number to its tombstone, and the
        // answer was "no ledger item at `…:tombstone:…`" — a NotFound naming a subject
        // nobody asked for.
        let query = format!(
            "SELECT ?item WHERE {{ {} }} ORDER BY ?item LIMIT 1",
            client.in_graph(&format!(
                "?item <{}> <{}> ; <{}> {} .",
                v::ext::TYPE,
                v::ITEM_CLASS,
                v::NUMBER,
                sparql::integer(number)
            ))
        );
        if let Some(found) = client
            .select(&query)
            .await?
            .first()
            .and_then(|row| row.get("item"))
        {
            return Ok(found.value.clone());
        }
        return Err(Error::NotFound(
            match find_tombstone(client, &number.to_string()).await? {
                Some(tombstone) => format!(
                    "{} was deleted: its tombstone is `{}` and its content, unless purged, \
                     is in `{}`",
                    ledger.number(number),
                    tombstone.iri,
                    ledger.deleted_graph()
                ),
                None => format!("no ledger item {}", ledger.number(number)),
            },
        ));
    }
    Ok(ledger.item(id))
}

/// A deleted item's tombstone, as the live graph holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tombstone {
    /// The tombstone's own IRI.
    pub iri: String,
    /// The IRI of the item it stands for.
    pub item: String,
    /// The item's number.
    pub number: i64,
}

/// Find the tombstone a delete left for `reference` — a number, an opaque id, or an item
/// IRI in either spelling — in this client's ledger, or `None` when nothing by that name
/// was ever deleted here.
///
/// ★ The tombstone is the only thing a deleted item leaves in the live graph, and it
/// carries both of the item's names (its number and its IRI), so it is how a resource
/// reaches an item that no longer resolves. `purge` is the reader: a secret pasted into an
/// item and then deleted is still in the graveyard, and the tombstone is how purge finds it.
pub async fn find_tombstone(
    client: &StoreClient<'_, '_>,
    reference: &str,
) -> Result<Option<Tombstone>> {
    let ledger = client.ledger();
    let pattern = if let Some((other, iri)) = Ledger::item_iri(reference) {
        if &other != ledger {
            return Ok(None);
        }
        // The id position may hold a number (`urn:iki:ledger:item:3` resolves as #3), so
        // it is read the way `resolve_id` reads it.
        // `split_once`, not `rsplit_once`: the ledger name cannot contain `:item:` (`item`
        // is reserved and a name has no `:`), but a key after it can.
        let tail = iri.split_once(":item:").map(|(_, t)| t).unwrap_or_default();
        if let Some(key) = tail.strip_prefix(KEY_PREFIX) {
            format!("?t <{}> {} .", v::KEY, literal(&parse_key(key)?))
        } else {
            match number_in(ledger, tail)? {
                Some(number) => format!("?t <{}> {} .", v::NUMBER, sparql::integer(number)),
                None => format!("?t <{}> {} .", v::DELETED_ITEM, sparql::iri(&iri, "item")?),
            }
        }
    } else if reference.starts_with("urn:") {
        // Not a ledger item IRI at all, so nothing here was ever deleted under it.
        return Ok(None);
    } else if let Some(key) = reference.strip_prefix(KEY_PREFIX) {
        format!("?t <{}> {} .", v::KEY, literal(&parse_key(key)?))
    } else if let Some(number) = number_in(ledger, reference)? {
        format!("?t <{}> {} .", v::NUMBER, sparql::integer(number))
    } else {
        format!(
            "?t <{}> {} .",
            v::DELETED_ITEM,
            sparql::iri(&ledger.item(reference), "item")?
        )
    };
    let query = format!(
        "SELECT ?t ?item ?number WHERE {{ {} }} ORDER BY ?t LIMIT 1",
        client.in_graph(&format!(
            "?t <{type_}> <{class}> ; <{deleted}> ?item ; <{number}> ?number .\n{pattern}",
            type_ = v::ext::TYPE,
            class = v::TOMBSTONE_CLASS,
            deleted = v::DELETED_ITEM,
            number = v::NUMBER,
        ))
    );
    Ok(client.select(&query).await?.first().and_then(|row| {
        Some(Tombstone {
            iri: row.get("t")?.value.clone(),
            item: row.get("item")?.value.clone(),
            number: row.get("number")?.as_i64()?,
        })
    }))
}

/// Load an item's comments, oldest first — a log is read forwards.
pub async fn load_comments(client: &StoreClient<'_, '_>, item: &str) -> Result<Vec<Comment>> {
    let query = format!(
        "SELECT ?comment ?body ?author ?created WHERE {{ {} }} ORDER BY ?created",
        client.in_graph(&format!(
            "?comment <{on_item}> {subject} ;\n  <{body}> ?body ;\n  <{created}> ?created .\n\
             OPTIONAL {{ ?comment <{author}> ?author }}",
            on_item = v::ON_ITEM,
            subject = sparql::iri(item, "item")?,
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
        client.in_graph(&format!(
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

/// The recency the item queries order by: `dcterms:modified`, or `dcterms:created` standing
/// in for it — the rule [`items_from_rows`] applies, in SPARQL, so a `LIMIT` keeps the
/// same items the reader would. A subject with neither readable sorts last (unbound sorts
/// first ascending, so last under `DESC`).
///
/// ⚠ The two parsers are the store's `xsd:dateTime` cast here and [`millis`] in Rust, and
/// they can disagree at the edges (`24:00:00`, a year past 9999). The only consequence is
/// where such an item sits relative to a `LIMIT`; the rows are re-sorted by the Rust
/// reading afterwards.
const RECENCY: &str = "COALESCE(<http://www.w3.org/2001/XMLSchema#dateTime>(?modified), \
                       <http://www.w3.org/2001/XMLSchema#dateTime>(?created))";

/// The rows of an item query → [`Item`]s, one per subject, most recently modified first.
///
/// ★ **Grouped by subject, because the timestamps are OPTIONAL.** A hand edit can leave a
/// property with two values (an `INSERT` with no `DELETE`), and every value is a row.
/// Until 0.4.2 each row became an item, so two readable `dcterms:modified` listed the item
/// twice, and an unreadable one beside a readable one listed it once by luck. Now the
/// rows of one subject are ONE item: the latest readable `modified`, the earliest readable
/// `created`, and every value the reader could not use named in [`Item::defects`].
fn items_from_rows(rows: &[Row]) -> Vec<Item> {
    let mut order: Vec<&str> = Vec::new();
    let mut groups: BTreeMap<&str, Vec<&Row>> = BTreeMap::new();
    for row in rows {
        let Some(iri) = row.get("item") else { continue };
        groups
            .entry(iri.value.as_str())
            .or_insert_with(|| {
                order.push(iri.value.as_str());
                Vec::new()
            })
            .push(row);
    }
    let mut items: Vec<Item> = order
        .iter()
        .filter_map(|iri| item_from_rows(&groups[iri]))
        .collect();
    // Stable, so equal keys keep the store's order.
    items.sort_by(|a, b| b.modified.cmp(&a.modified).then(b.number.cmp(&a.number)));
    items
}

/// One subject's rows → an [`Item`], or `None` when the reader cannot list it: no readable
/// number, or no readable timestamp at all — exactly what [`defects`] reports.
fn item_from_rows(rows: &[&Row]) -> Option<Item> {
    let row = rows
        .iter()
        .find(|row| row.get("number").and_then(|n| n.as_i64()).is_some())?;
    // Every distinct value of one timestamp across the rows, read or not.
    let read =
        |var: &str, predicate: &'static str, unread: &mut Vec<(&'static str, sparql::Binding)>| {
            let mut parsed = Vec::new();
            for value in rows.iter().filter_map(|row| row.get(var)) {
                match millis(&value.value) {
                    Some(ms) => parsed.push(ms),
                    None if unread.iter().any(|(p, b)| *p == predicate && b == value) => {}
                    None => unread.push((predicate, value.clone())),
                }
            }
            parsed
        };
    let mut unread = Vec::new();
    let created = read("created", v::ext::CREATED, &mut unread)
        .into_iter()
        .min();
    let modified = read("modified", v::ext::MODIFIED, &mut unread)
        .into_iter()
        .max();
    let mut defects: Vec<String> = unread
        .iter()
        .map(|(predicate, value)| unreadable(predicate, &value.value))
        .collect();
    for (predicate, present) in [
        (
            v::ext::CREATED,
            rows.iter().any(|r| r.get("created").is_some()),
        ),
        (
            v::ext::MODIFIED,
            rows.iter().any(|r| r.get("modified").is_some()),
        ),
    ] {
        if !present {
            defects.push(format!("missing {}", short_name(predicate)));
        }
    }
    defects.sort();
    // Every distinct `ledger:state` across the subject's rows: the OPTIONAL multiplies rows
    // when there are several, exactly as a doubled timestamp does.
    let mut state_values: Vec<sparql::Binding> = Vec::new();
    for value in rows.iter().filter_map(|row| row.get("state")) {
        if !state_values.contains(value) {
            state_values.push(value.clone());
        }
    }
    state_values.sort_by(|a, b| a.value.cmp(&b.value));
    let mut states: Vec<String> = state_values
        .iter()
        .map(|b| crate::lifecycle::state_name(&b.value))
        .collect();
    states.sort();
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
        key: row.get("key").map(|b| b.value.clone()),
        claimed_by: row.get("holder").map(|b| b.value.clone()),
        purpose: row.get("purpose").map(|b| b.value.clone()),
        // Each stands in for the other: created ≤ modified, so a missing `created` is at
        // most the `modified`, and a missing `modified` is at least the `created`. Neither
        // is invented, and with neither readable there is no true time to show.
        created: created.or(modified)?,
        modified: modified.or(created)?,
        labels: Vec::new(),
        about: Vec::new(),
        blocks: Vec::new(),
        parents: Vec::new(),
        related: Vec::new(),
        defects,
        states,
        state_values,
        unread,
        read: (created.is_some(), modified.is_some()),
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
pub(crate) fn millis(iso: &str) -> Option<u64> {
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
            key: None,
            claimed_by: None,
            purpose: None,
            created: 1_757_700_000_000,
            modified: 1_757_700_000_000,
            labels: vec!["module".to_string()],
            about: vec!["urn:repo:file:ikigai-cli/src/main.rs".to_string()],
            blocks: Vec::new(),
            parents: Vec::new(),
            related: Vec::new(),
            defects: Vec::new(),
            states: Vec::new(),
            state_values: Vec::new(),
            unread: Vec::new(),
            read: (true, true),
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
