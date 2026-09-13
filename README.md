# ikigai-ledger

**The work ledger for [ikigai](https://github.com/ikigai-rs)**: items, comments, labels,
links and claims as RDF in a durable store — with view, query, append, comment, close and
delete as capability-gated resources, and **`urn:iki:ledger:next`**, which answers *what
should I do next* as a resource rather than as a sort order.

```text
$ sink urn:iki:ledger:append <<< "Wire the ledger into the embedded host

It needs urn:iki:store:* bound in the same kernel."
#1 urn:iki:ledger:item:01m2h5t1z80m3b2f

$ sink urn:iki:ledger:append priority=0 labels=core <<< "Publish ikigai-store 0.2.0"
#2 urn:iki:ledger:item:01m2h5tcq09zs3r3

$ sink urn:iki:ledger:link item=#2 type=blocks <<< "#1"
linked #2 blocks #1

$ source urn:iki:ledger:next
 1.    #2  open    p0  Publish ikigai-store 0.2.0  [core]
    p0 — priority 0; last updated 2026-09-15T00:00:04.000Z

policy: priority-recency (weighs priority, recency, number)
ready: 1   excluded: 1
  not #1: blocked by #2
```

## Why an RDF ledger and not a table

Because the questions people actually ask cross tools. *What is open against this file?*
*Which review findings on which commits are still unresolved, by author, across repos?*
Those are joins between an issue, a commit, a PR, a review and an annotation — five things
that live in five databases everywhere else, and in **one graph** here, joined by resource
IRIs. `ledger:about <urn:repo:file:…>` is the whole trick, and it is why an item can be
filed *against* something rather than merely mentioning it.

The corollary: there is **no query endpoint in this crate**. Query is
`urn:iki:store:select` over the same dataset — four typed SPARQL forms, already
capability-gated, already conformance-walked. A second query surface would be a second
thing to secure and a second thing to get wrong.

```sparql
# what is open against this file, with who filed it and when
SELECT ?number ?title ?author WHERE {
  GRAPH <urn:iki:ledger:graph> {
    ?item ledger:about <urn:repo:file:ikigai-cli/src/main.rs> ;
          ledger:status ledger:open ;
          ledger:number ?number ;
          dcterms:title ?title .
    OPTIONAL { ?item ledger:author ?author }
  }
}
```

## Composition: this crate owns no bytes

Every read here is a SPARQL query issued at `urn:iki:store:select`, and every write is a
SPARQL UPDATE at `urn:iki:store:update`. [`ikigai-store`](https://crates.io/crates/ikigai-store)
owns the dataset, the write lock and the golden threads; this module owns the **domain**.

```rust
let store = DurableStore::open(&StoreConfig::load(Some("gonk"))?.path)?;  // owned
let space = Fallback::new(vec![
    Arc::new(ikigai_store::space(store)) as Arc<dyn Space>,
    Arc::new(ikigai_ledger::space()),
]);
let kernel = Kernel::new(Arc::new(space)).with_clock(Arc::new(SystemClock));
```

Three things a host must know, each of which is a real failure otherwise:

- **The ledger is inert without a store space in the same kernel.** Resolving a ledger IRI
  then fails with a sentence naming `urn:iki:store:*` and what to bind — not the kernel's
  generic "no endpoint".
- **`DurableStore::open`, never `open_shared`.** The shared constructor hands out the raw
  handle and makes every read `Expiry::Always` for the life of the store; owned reads are
  cacheable under the store's write threads, and this module's reads inherit that.
- **A kernel with no clock cannot write here.** Every item, comment, claim and tombstone is
  stamped, and an entry without a timestamp is not a ledger entry — so the write is
  refused rather than made unstamped.

⚠ **One writer per directory, across processes.** RocksDB enforces it with a lock file, so
one process owns the ledger and a second is refused with a legible `Unavailable` naming the
path. That is fine for one operator today; a second reaches the data **over the wire**
(IPC, QUIC, mount-over-wire), which is the ikigai answer and needs nothing new here.

## The resources

| resource | verbs | what it does | capability |
| --- | --- | --- | --- |
| `urn:iki:ledger:items` | Source | the list, filtered | `ledger:read` |
| `urn:iki:ledger:item:{id}` | Source · Sink · Delete · Exists | one item, edit it, delete it | `read` / `write` / `delete` |
| `urn:iki:ledger:append` | Sink | file a new item | `ledger:write` |
| `urn:iki:ledger:comment` | Sink | append a comment | `ledger:write` |
| `urn:iki:ledger:close` | Sink | close with a reason | `ledger:write` |
| `urn:iki:ledger:reopen` | Sink | undo a close | `ledger:write` |
| `urn:iki:ledger:claim` | Sink · Delete | take it / hand it back | `ledger:write` |
| `urn:iki:ledger:defer` | Sink · Delete | not now / now again | `ledger:write` |
| `urn:iki:ledger:link` | Sink · Delete | blocks / parent / related | `ledger:write` |
| `urn:iki:ledger:label` | Sink · Delete | tag / untag | `ledger:write` |
| `urn:iki:ledger:purge` | Delete | destroy, leaving a tombstone | `ledger:purge` |
| `urn:iki:ledger:next` | Source | the ready set, ranked | `ledger:read` |
| `urn:iki:ledger:policy:{name}` | Source · Exists | what a policy weighs | `ledger:read` |

Every read serves `text/plain` (the default — a line per item, greppable) and
`text/turtle` (the graph). An `as=` this module cannot answer in is **refused**, never
substituted.

## Identity: the IRI is the name, `#12` is a label on it

An item is `urn:iki:ledger:item:{id}`, where `{id}` is 10 characters of Crockford base32
over the millisecond clock — so ids sort in filing order — plus 6 derived from a SHA-256 of
what was filed. The clock half makes a raw IRI listing readable; the digest half is what
keeps two ledgers merged later from colliding on a shared millisecond.

`ledger:number` — `#12` — is the human handle, and every endpoint accepts either form,
because a person types `12` and a machine carries the IRI. It is allocated from
`urn:iki:ledger:counter` **inside the same SPARQL UPDATE that writes the item**, so two
concurrent appends in one process cannot take the same number (the store's one-writer rule
excludes other *processes*, not other *requests* — a read-modify-write across two round
trips would have raced). The counter is a resource in the graph rather than process state,
which is why a **restart continues the numbering** and why a delete or a purge can never
make a number be reused: two pieces of work sharing a name would invalidate every
reference anybody wrote down.

Numbers are per **store**. There are no projects yet; when there are, this is where that
decision lands.

## Assume an editor got there first

A ledger whose items can only change through its own `Sink` is not what anyone wants from
durable, inspectable state: the value of a record you can keep is that a human in an editor
— or a merge, or an LLM harness, or a bulk load — can touch it out of band. Here that is
literally true already: anything holding `urn:cap:store:write` can rewrite this graph
without passing through a ledger endpoint. So an **out-of-band write is a supported path,
not corruption**, and two things follow.

**Identity survives editing.** An item's IRI is minted once from the clock and a digest of
what was filed, and then *stored*. It is never derived from the item's content, its number
or its position, so rewriting a title, renumbering, or reformatting in an editor cannot
silently rename the thing. This is the decision that would be worst to retrofit and it is
made here, at the first commit.

**The model is checked on read, not only on write.** A hand edit bypassed the Sink's
refusals, so `urn:iki:ledger:items` runs a corpus check beside the listing: an item typed
`ledger:Item` that is missing anything a reader needs is **reported**, with the properties
it lacks, rather than quietly dropped — the worst outcome being work that is neither
visible nor gone. Asking for such an item directly says what is wrong with it, which "not
found" would not. `model::REQUIRED` is the one list both directions use.

## What `Delete` does — the part worth arguing about

A ledger that silently forgets is not a ledger. So there are three different acts, and they
are three different resources, because they are three different authorities:

| act | resource | what survives |
| --- | --- | --- |
| **close** | `urn:iki:ledger:close` | everything. The item is still in the graph, still queryable, and stops blocking what it blocked. This is the normal end of work. |
| **delete** | `Delete urn:iki:ledger:item:{id}` (`urn:cap:ledger:delete`) | the item's quads, its comments and the edges pointing at it MOVE to `urn:iki:ledger:graph:deleted`. Out of every ledger read; still there; recoverable. A **tombstone** stays in the live graph. |
| **purge** | `urn:iki:ledger:purge` (`urn:cap:ledger:purge`) | only the tombstone. The content is destroyed in both graphs. |

The tombstone carries the number, the time, the actor, the reason, the quad count and the
`sig:contentHash` (`sha256:…`) of exactly what was removed — so a later claim about what an
item said is checkable, and a ledger that forgot something still records that it did. The
hash is over sorted triples with their term kinds and datatypes: there are no blank nodes
in this graph, so that IS a canonical form and the heavyweight dataset canonicalization
would buy nothing.

**Why purge is a resource and not a `purge=true` argument.** A flag could only be enforced
at runtime, and an action that enforces a scope it does not declare makes the manifold lie —
worse than the converse. Declaring both scopes on one action would demand purge authority
for every ordinary delete. The authority difference gets its own IRI, exactly as
`ikigai-store` gives each SPARQL form its own.

## Selection: `next` is a resource, and its policy is a seam

Two stages, kept apart:

1. **The ready set is deterministic and is a query**: open, unclaimed, not deferred, not
   blocked by an open item, plus the caller's label / `about` / level filters. No policy is
   involved, and most of the value is here.
2. **The order within it is policy**, and that is the part people disagree about.

Each policy is a **resource** — `urn:iki:ledger:policy:{name}` — so the manifold advertises
which orderings exist and `Meta` says what each one weighs. Two ship:

- **`priority-recency`** (the default) is **kata's own rule**, reimplemented so behaviour can
  be *diffed* against the tool being replaced: any priority beats none, lower wins, ties go
  to the most recently updated.
- **`leverage`** is the one kata cannot express: 10 points per open item unblocked
  **transitively**, `(5 − priority) × 4` for priority, and up to 10 for age so nothing
  starves. Finishing a blocker converts several unready items into ready ones, and a
  priority sort cannot see that at all.

A host supplies its own with `space_with_policies(…)` — the `CachePolicy` shape, configured
at execution time rather than compiled in. The first registered policy is the default; an
empty list refuses at boot, where the manifest is.

Two things that were designed in rather than discovered later:

- ⚠ **A `blocks` cycle makes the ready set silently empty**, which is indistinguishable from
  a finished backlog. `next` walks the open subgraph by hand and **refuses, naming the
  cycle** (`#3 → #7 → #12 → #3`) and the command that breaks it. A SPARQL property path can
  say *whether* there is a cycle but not *which*, and a refusal that cannot name it leaves
  the operator exactly where the silent empty answer did.
- ★ **The answer can say why.** `text/plain` gives the ranking with the policy's reasons;
  `as=text/turtle` gives a `ledger:Selection` graph — policy, ranked items with scores and
  reasons, and every *exclusion* with its reason. "Do this next" that cannot be interrogated
  gets overridden and then ignored.

⚠ **Selection OFFERS; it never authorizes.** `next` naming an item is an affordance. Acting
on it still requires the actor's own capability, checked by the kernel at that action.

## Levels: different types, shared plumbing

Every item asserts `ledger:Item`, and may **also** assert a more specific class for its
level — a decision record, a review finding, a state transition — passed as `kind=` on
append and filtered with `kind=` on `items` and `next`. Different granularity really is a
different shape, so levels are different *classes*; what they share is the plumbing: one
identity scheme, five verbs, one manifold, one capability model, one set of threads, one
query surface, one selection surface.

Asserting the base class on every item is what makes that cheap in both directions —
"everything in the ledger" stays one triple pattern and needs no reasoner, and a new level
is additive: a class IRI and a shape. Nothing in this crate enumerates the subclasses.

⚠ A **level** is not a **layer**. Levels are scope and granularity; layers are permissioned
per-participant graphs. Conflating them would produce a model where granting someone access
changes what granularity they see.

## Capabilities, and what we could not make narrower

`urn:cap:ledger:{read,write,delete,purge}` gate this module. A ledger holds whatever anyone
filed, so an unrestricted read is not free — and delete and purge are separated from the
everyday write grant because they take work *out* of the record.

⚠ **Every action here also declares the store scopes it transitively needs**, and that is
the honest part of a coarser story than we would like: a sub-request carries the **caller's**
capability unchanged (`Invocation::issue` has no attenuating or elevating form), so a ledger
write is only possible for a caller who also holds `urn:cap:store:write` — which is the keys
to the whole store, `DROP ALL` included. Declaring it is right (an action that enforces a
scope it does not declare makes the manifold over-offer); *needing* it is a limitation of
the composition, not of this model. Fixing it properly needs a way for a module to issue a
sub-request under an authority it holds rather than one its caller does — a kernel question,
reported rather than worked around.

## The vocabulary

`https://ikigai-rs.dev/ns/ledger#`, self-contained in `src/vocabulary.ttl` (the `sig:` and
`log:` precedent), so nothing here waits on a manual `/ns` deploy. Reused where a term
already exists: `dcterms:title` / `created` / `modified`, `prov:Entity` / `Activity` /
`generatedAtTime` / `invalidatedAtTime`, and `sig:contentHash` for the tombstone digest —
the same predicate `ikigai-sign` and `ikigai-log` write, so the three join in a query
instead of colliding.

⚠ One term probably does not belong here: **`ledger:about`**. `ikigai-browse` already has the
same edge under another name (`ik:annotates`, in the kernel's vocabulary), so today
"everything filed against this file" is a union of two predicates that mean the same thing.
The right fix is a general `ik:about` in `ikigai-vocab` with both as sub-properties. That is
a core change, so it is reported rather than reached; migrating is one SPARQL UPDATE over
one graph.

## Not built, on purpose

- **No HTML face.** The browse/XSLT/htmx register is real and is a separate arc; a
  `text/plain` list you can read in the REPL is what "view ASAP" needs.
- **No event graph.** kata records every mutation as an event row. Here the mutation history
  is the kernel's to tell — `ikigai-log`'s tracer already writes resolutions, cache hits and
  capability denials — and duplicating it in the domain graph would be two records to
  disagree.
- **No timed claims.** kata has hard and timed claims; this has hard ones only. The 90-minute
  lapse our own dispatch fence uses has no analogue here yet, and a claim race between two
  requests in one process is possible: RDF has no partial unique index and SHACL cannot see
  a race. Safe for one operator, and the first thing to harden when there are two.
- **No `about` removal, no comment editing.** Comments are append-only by design; `about`
  removal is an omission, not a principle.
- **No `deferred-until` date.** Readiness that turns on the wall clock would go stale in the
  cache with no golden thread to cut it, so resuming is an act rather than the passage of
  time.

## Status

Bound and tested; **not yet wired into a host** — that is a follow-on `ikigai-cli` arc.
`ikigai-conformance` walks all thirteen resources clean. Publishing to crates.io is Brian's.
