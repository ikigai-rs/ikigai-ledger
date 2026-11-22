# Design: decision records and quoted speakers

Status: proposed; this note changes no ledger behavior. A decision is a recorded assertion,
not proof that a person had authority or that the decision remains in force.

## Evidence and scope

On the base revision, `src/vocabulary.ttl` defines `ledger:Comment` with a body and
`ledger:onItem`, but no decision class, decision maker, decision time, or quoted-speaker
predicate. `CommentEndpoint` accepts comment text and attribution, not a decision form;
`model::load_comments` loads the body and existing provenance, not those facts. Thus two
comments with identical bodies and metadata cannot be distinguished as a decision and an
ordinary remark by a graph query; identifying a decision or its maker from those comments
requires an external convention or interpretation of prose. This is a source-level
reproduction, not a survey of live comments.

The limitation is narrower than unattributed comments in general: existing attribution can
already record a delegate's claimed author and verified committer. The existing
`an_enrolled_delegate_records_a_claimed_author_and_itself_as_committer` test exercises that
case in plain text, JSON and Turtle. It does not represent a quotation by an ordinary filer,
nor establish that an author made a decision. A principal-less write can have no committer,
and a free-text author is a claim (`src/attribution.rs`).

## Record and meaning

Use a comment subtype, not an item subtype: a decision is attached to the item it settles
and must appear wherever its comments already appear. Write both explicit RDF types,
`ledger:Comment` and `ledger:Decision`, as for dispatch notes; do not rely on RDFS inference.
The proposed vocabulary and cardinalities for *new* decision writes are:

| Predicate | Value | Meaning |
| --- | --- | --- |
| `ledger:onItem` | exactly one existing item IRI in the same ledger graph | The item this assertion says the decision settles; it does not close or change the item. |
| `ledger:body` | exactly one nonempty string | The decision itself, in the author's words. It is the answer to what was decided, not an identifier for a category. |
| `ledger:decidedBy` | exactly one actor IRI | The asserted decision maker, including a collective actor when applicable. |
| `ledger:decidedAt` | exactly one `xsd:dateTime` with an explicit timezone | When the decision was made, as asserted by the filer; distinct from the comment's creation and commit times. |

The file's existing `ledger:author`, `ledger:authorSource`, `ledger:committer`,
`ledger:committedAt`, and `dcterms:created` retain their current semantics. Neither an
admitted author nor a verified committer automatically verifies `decidedBy` or `decidedAt`.
A decision made by more than one actor can name a stable collective IRI; representing multiple
independent makers and their individual roles is a follow-up, not an implicit list in prose.
A date without a time must not be invented as midnight: defer recording it in this form until
a precision-aware representation is designed. Refuse missing, malformed, or future decision
times and invalid actor IRIs before any write. Out-of-band graphs may still contain malformed
records; readers should tolerate them and must not silently invent missing facts.

For an ordinary comment quoting someone, propose an optional single `ledger:quotedSpeaker`
actor IRI. It identifies the person whose words the body quotes, as claimed by the filer;
it is not authorization to act for that person. The comment's normal author and committer
remain the filer and verified filing principal under existing attribution rules. Quoting and
filing on another's behalf are different: `author=` continues to require the existing
delegation/import rules, while `quotedSpeaker` does not bypass them. For a quote that is only
part of a comment, the predicate identifies the source of the quoted passage, not every word
of the body. Multiple speakers and passage-level spans need a later design.

Example proposed triples (prefixes below apply throughout this note):

```turtle
@prefix ledger: <https://ikigai-rs.dev/ns/ledger#> .
@prefix dcterms: <http://purl.org/dc/terms/> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

<urn:example:decision> a ledger:Comment, ledger:Decision ;
    ledger:onItem <urn:example:item> ;
    ledger:body "Use the staged rollout." ;
    ledger:decidedBy <urn:example:actor:reviewer> ;
    ledger:decidedAt "2026-10-01T12:00:00Z"^^xsd:dateTime ;
    ledger:author <urn:example:actor:filer> ;
    ledger:authorSource "committer" ;
    ledger:committer <urn:example:actor:filer> ;
    dcterms:created "2026-10-02T12:00:00Z"^^xsd:dateTime ;
    ledger:committedAt "2026-10-02T12:00:00Z"^^xsd:dateTime .

<urn:example:quote> a ledger:Comment ;
    ledger:onItem <urn:example:item> ;
    ledger:body "The staged rollout is ready." ;
    ledger:quotedSpeaker <urn:example:actor:reviewer> ;
    ledger:author <urn:example:actor:filer> ;
    ledger:authorSource "committer" ;
    ledger:committer <urn:example:actor:filer> ;
    dcterms:created "2026-10-02T12:01:00Z"^^xsd:dateTime ;
    ledger:committedAt "2026-10-02T12:01:00Z"^^xsd:dateTime .
```

## Proposed write and read contract

Add an explicit `decision` sink with `item=`, `decided-by=`, `decided-at=` and
`content`, plus the existing comment `key=` and optional `epoch=` semantics. A separate
door keeps ordinary comments unchanged and makes the intent explicit. It uses the ledger's
comment grant and graph write grant; the grant permits recording an assertion, not deciding
on another actor's behalf. Validate all fields, item existence and any fence before the
atomic insert. Write the types, facts and existing provenance in the same guarded update;
key replay answers the existing comment and its actual item, without updating its content,
as `comment key=` does today. A stale fence or validation failure writes nothing. Implement
the same deletion, graveyard, key and cache behavior as a comment. Honor applicable comment
write shapes and define a decision-specific shape before deployment. A later implementation
must resolve the interaction with hand-back clearing under close/write grants consistently
with ordinary comments; a decision alone never closes an item.

Add `quoted-speaker=` to the ordinary comment form, optional and independent of `author=`.
Validate it as an actor IRI; keep it absent for ordinary comments. For both forms preserve a
principal-less filer's absence of verified committer and label all asserted actors and dates
as claimed. A host that needs verified decision authority must enforce its own policy; it
must not interpret this comment grant as such authority.

Existing comments stay readable without migration or prose parsing. Decision comments keep
their standard comment body and stamp in plain, JSON and Turtle; add labeled decision maker,
decision time and quoted speaker fields only when present, with claimed status in words.
Readers must not default a missing speaker or decision maker to the author. JSON changes need
an explicit compatibility review; missing fields on old records remain absent, and a reader
that ignores new fields still sees a comment. Plain output should name relationships in words,
and tests must cover the accessibility guidance in `ACCESSIBILITY.md`. No backfill should
promote prose to fact automatically. Any reviewed backfill should create a new attributed
record with its source, rather than rewrite an append-only comment.

## Query

Use the existing capability-gated `urn:iki:store:graph-select` against a graph the caller may
read; no new query endpoint or cross-graph privilege is implied. For one selected ledger:

```sparql
PREFIX ledger: <https://ikigai-rs.dev/ns/ledger#>
PREFIX dcterms: <http://purl.org/dc/terms/>
SELECT ?item ?decision ?what ?maker ?when ?filer ?source ?committer ?committed WHERE {
  GRAPH <urn:iki:ledger:graph:default> {
    ?decision a ledger:Decision ; ledger:onItem ?item ; ledger:body ?what ;
              ledger:decidedBy ?maker ; ledger:decidedAt ?when .
    OPTIONAL { ?decision ledger:author ?filer }
    OPTIONAL { ?decision ledger:authorSource ?source }
    OPTIONAL { ?decision ledger:committer ?committer }
    OPTIONAL { ?decision ledger:committedAt ?committed }
  }
}
ORDER BY ?when ?decision
```

Add `VALUES ?item { <item-iri> }` inside the graph to filter one item. This returns asserted
records, including later conflicting decisions; ordering does not establish which is current.
It excludes legacy comments and incomplete malformed decisions, so an empty result does not
mean no decision was made. A quote query can select `?comment ledger:quotedSpeaker ?speaker`
and join `ledger:onItem`, body and the same optional provenance. Consumers should handle
multiple values in externally edited graphs without treating duplicate rows as separate
verified decisions. To query several ledgers, enumerate only graphs the caller is authorized
to read, using the store's graph-scoped capability rules.

## Proposed follow-up items

- **Implement typed decision comments** — Add the vocabulary, guarded decision sink, key and
  fence behavior, graph-scoped tests, and plain/JSON/Turtle reads, including malformed data and
  existing-comment compatibility.
- **Record quoted speakers on comments** — Add the optional speaker IRI and distinguish the
  filer's provenance from the claimed quotation in all faces and tests.
- **Design decision revisions and source evidence** — Define supersession, conflicts, source
  links, collective or multiple makers, and precision-aware historical dates without inferring
  a current decision from timestamp order.
- **Evaluate a decision-trail view** — Use the scoped query with accessible labels and claimed
  provenance; test authorization, empty and incomplete records, and representative legacy data.
