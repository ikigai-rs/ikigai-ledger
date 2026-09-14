# ikigai-ledger

**The work ledger for [ikigai](https://github.com/ikigai-rs)**: items, comments, labels,
links and claims as RDF in a durable store — with view, query, append, comment, close and
delete as capability-gated resources, and **`urn:iki:ledger:next`**, which answers *what
should I do next* as a resource rather than as a sort order.

A ledger is **named**, and the name is part of the IRI: `urn:iki:ledger:acme:append` is a
different resource from `urn:iki:ledger:bosatsu:append`, backed by a different named graph
and gated by a different capability. Omit the segment and you address the ledger called
`default`.

```text
$ sink urn:iki:ledger:append <<< "Wire the ledger into the embedded host

It needs urn:iki:store:* bound in the same kernel."
#1 urn:iki:ledger:default:item:01m2h5t1z80m3b2f

$ sink urn:iki:ledger:append priority=0 labels=core <<< "Publish ikigai-store 0.2.1"
#2 urn:iki:ledger:default:item:01m2h5tcq09zs3r3

$ sink urn:iki:ledger:link item=#2 type=blocks <<< "#1"
linked #2 blocks #1

$ source urn:iki:ledger:next
 1.    #2  open    p0  Publish ikigai-store 0.2.1  [core]
    p0 — priority 0; last updated 2026-09-15T00:00:04.000Z

policy: priority-recency (weighs priority, recency, number)
ready: 1   excluded: 1
  not #1: blocked by #2

$ sink urn:iki:ledger:acme:append <<< "Their Q4 migration"
acme#1 urn:iki:ledger:acme:item:01m2h6b41k0we8r2

$ source urn:iki:ledger:ledgers
default                       2 open      2 total  urn:iki:ledger:graph:default
acme                          1 open      1 total  urn:iki:ledger:graph:acme

2 ledger(s)
```

## Why an RDF ledger and not a table

Because the questions people actually ask cross tools. *What is open against this file?*
*Which review findings on which commits are still unresolved, by author, across repos?*
Those are joins between an issue, a commit, a PR, a review and an annotation — five things
that live in five databases everywhere else, and in **one graph** here, joined by resource
IRIs. `ledger:about <urn:repo:file:…>` is the whole trick, and it is why an item can be
filed *against* something rather than merely mentioning it.

The corollary: there is **no query endpoint in this crate**. Query is
`urn:iki:store:graph-select` over one ledger's graph — or `urn:iki:store:select` over the
whole dataset, for a caller a host has given the whole dataset. Eight typed SPARQL forms,
already capability-gated, already conformance-walked. A second query surface would be a
second thing to secure and a second thing to get wrong.

```sparql
# what is open against this file, in every ledger at once, with who filed it
SELECT ?ledger ?number ?title ?author WHERE {
  GRAPH ?ledger {
    ?item ledger:about <urn:repo:file:ikigai-cli/src/main.rs> ;
          ledger:status ledger:open ;
          ledger:number ?number ;
          dcterms:title ?title .
    OPTIONAL { ?item ledger:author ?author }
  }
}
```

Name the graph (`GRAPH <urn:iki:ledger:graph:acme>`) to ask one ledger, or bind it as a
variable to ask across them — which is the second reason a ledger is a named graph rather
than a column: partitioning by graph costs nothing when you want the whole picture.

## Composition: this crate owns no bytes

Every read here is a SPARQL query issued at `urn:iki:store:graph-select`, and every write is
a SPARQL UPDATE at `urn:iki:store:graph-update` — the **narrow** doors, each naming one
ledger's graph and gated by a grant for that graph.
[`ikigai-store`](https://crates.io/crates/ikigai-store) (**0.2.2 or later**, which is where
those doors arrive) owns the dataset, the write lock and the golden threads; this module owns
the **domain**.

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

Everything below takes a `{ledger}` segment, and the segment may be omitted for the ledger
called `default`. The capability column names the grant for **that** ledger.

| resource | verbs | what it does | capability |
| --- | --- | --- | --- |
| `…:{ledger}:items` | Source | the list, filtered | `read:{ledger}` |
| `…:{ledger}:item:{id}` | Source · Sink · Delete · Exists | one item, edit it, delete it | `read` / `write` / `delete` of `{ledger}` |
| `…:{ledger}:append` | Sink | file a new item | `write:{ledger}` |
| `…:{ledger}:comment` | Sink | append a comment | `write:{ledger}` |
| `…:{ledger}:close` | Sink | close with a reason | `write:{ledger}` |
| `…:{ledger}:reopen` | Sink | undo a close | `write:{ledger}` |
| `…:{ledger}:claim` | Sink · Delete | take it / hand it back | `write:{ledger}` |
| `…:{ledger}:defer` | Sink · Delete | not now / now again | `write:{ledger}` |
| `…:{ledger}:link` | Sink · Delete | blocks / parent / related | `write:{ledger}` |
| `…:{ledger}:label` | Sink · Delete | tag / untag | `write:{ledger}` |
| `…:{ledger}:purge` | Delete | destroy, leaving a tombstone | `purge:{ledger}` |
| `…:{ledger}:next` | Source | the ready set, ranked | `read:{ledger}` |
| `urn:iki:ledger:ledgers` | Source | which ledgers exist | any `read:*` |
| `urn:iki:ledger:policy:{name}` | Source · Exists | what a policy weighs | any `read:*` |

The last two carry no ledger segment because neither is a ledger's own state: the
inventory spans them, and a policy is a property of the host's configuration. **The
capability column is only half the grant** — every row but the last also needs the store's
per-graph token for that ledger; see "What is enforced, and where" for the whole list.
`urn:iki:ledger:policy:{name}` is the exception that needs no store grant at all: a policy
is code the host registered at boot and nothing about it is in the graph.

Every read serves `text/plain` (the default — a line per item, greppable) and
`text/turtle` (the graph). An `as=` this module cannot answer in is **refused**, never
substituted.

## Ledgers: a name, not a tag and not an argument

A label is *data*; a capability binds to a *resource name*. There is nothing for
`urn:cap:…` to attach to in "items labelled acme", so enforcement would have to live
inside the endpoint — declared-but-not-enforced, which this ecosystem refuses everywhere
else. A tag partition is a **view**; only a name can be a **boundary**. A `ledger=`
*argument* has the same defect: an argument is a value, the manifold still offers one
action, and a capability still cannot distinguish one ledger from another.

So a ledger is a segment of the IRI, and underneath it is a named graph:

| thing | IRI |
| --- | --- |
| the ledger's resources | `urn:iki:ledger:{name}:*` |
| its graph | `urn:iki:ledger:graph:{name}` |
| its graveyard | `urn:iki:ledger:graph:{name}:deleted` |
| its counter | `urn:iki:ledger:{name}:counter` |
| its items | `urn:iki:ledger:{name}:item:{id}` |
| its grants | `urn:cap:ledger:{read,write,delete,purge}:{name}` |

One store, one write lock, one process, many ledgers. There is **no create and no
destroy**: a ledger exists once something is filed in it, and the capability is what makes
one real. `urn:iki:ledger:ledgers` lists the ones a caller may read — a grammar does not
enumerate, so without that resource a partition would be a secret rather than a boundary.

**The short form is an alias, not a second door.** `urn:iki:ledger:append` and
`urn:iki:ledger:default:append` are the *same grammar match*: one binding, one row in the
catalog, one capability (`urn:cap:ledger:write:default`). A short form implemented as a
separate binding would carry a separate capability, and a short form that quietly widens
authority is the hole named ledgers exist to close. The canonical form is the long one —
an item filed through the sugar is still minted at `urn:iki:ledger:default:item:{id}`,
because the data has to say which ledger it is in even when the request did not.

⚠ **The sugar costs a reserved-word list.** `urn:iki:ledger:items` has to mean *the
default ledger's listing* rather than *a ledger called `items`*, and only one of those can
be true — so `append`, `items`, `next`, `policy`, `ledgers` and eleven others are not
available as ledger names, and a name that is refused says which word it must not use.
Names are otherwise lowercase letters, digits, `-` and `_`; the shape is fixed because the
name becomes a capability token that is matched **exactly**, and a token must not be
forgeable by spelling.

**A `blocks` edge cannot cross a ledger.** A link is not a reference: it changes the other
ledger's ready set, so a caller granted one ledger could make work in another
unschedulable, and the blocked ledger's `next` would have to either name an item its
reader cannot see or say "something blocks you" — one bit leaked, and no help to anybody.
`ledger:about` is the edge that crosses, because it names a resource rather than asserting
a membership, and nothing computes readiness from it.

**Ordering policies are global**, not per ledger: `urn:iki:ledger:policy:{name}` describes
code the host registered at boot, which is the same for every ledger it serves. Per-ledger
policy would be configuration living in data with no capability story, and a host that
really wants different orderings for different partitions already has the seam — `next`
takes `policy=`.

## Identity: the IRI is the name, `#12` is a label on it

An item is `urn:iki:ledger:{ledger}:item:{id}`, where `{id}` is 10 characters of Crockford
base32 over the millisecond clock — so ids sort in filing order — plus 6 derived from a
SHA-256 of what was filed. The clock half makes a raw IRI listing readable; the digest half
is what keeps two ledgers merged later from colliding on a shared millisecond.

`ledger:number` — `#12` — is the human handle, and every endpoint accepts either form,
because a person types `12` and a machine carries the IRI. It is allocated from the
ledger's own `urn:iki:ledger:{ledger}:counter` **inside the same SPARQL UPDATE that writes
the item**, so two concurrent appends in one process cannot take the same number (the
store's one-writer rule excludes other *processes*, not other *requests* — a
read-modify-write across two round trips would have raced). The counter is a resource in
the graph rather than process state, which is why a **restart continues the numbering** and
why a delete or a purge can never make a number be reused: two pieces of work sharing a
name would invalidate every reference anybody wrote down.

**Numbers are per ledger**, so the display form carries the name once there is more than
one: `acme#12`, and a bare `#12` in the default ledger. Sharing a counter would have made
`#12` ambiguous, and would have leaked one ledger's activity to another through the gaps
in its sequence. A number qualified with a *different* ledger's name is refused rather
than looked up: `acme#12` and `#12` are different items, and silently resolving the wrong
one is the worst available answer.

## Assume an editor got there first

A ledger whose items can only change through its own `Sink` is not what anyone wants from
durable, inspectable state: the value of a record you can keep is that a human in an editor
— or a merge, or an LLM harness, or a bulk load — can touch it out of band. Here that is
literally true already: anything holding this graph's write grant — or the store's broad one
— can rewrite it without passing through a ledger endpoint. So an **out-of-band write is a
supported path, not corruption**, and three things follow.

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

**And a term this crate never writes still has to survive a delete.** Archiving re-serializes
the quads it moves (a scoped update cannot span two graphs, so they go out through a query
and back in as data), which means a hand-written `"étiquette"@fr` keeps its language tag
rather than flattening to a plain literal — a different statement, changed at exactly the
moment it is least recoverable. A **blank node** is the one thing that cannot survive: its
label does not carry through `INSERT DATA`, and archiving it would mint a different node, so
the delete is **refused** with a sentence naming it. Nothing here writes one — everything is
skolemized — so this can only ever be an out-of-band edit, and telling that operator beats
losing an edge.

## What `Delete` does — the part worth arguing about

A ledger that silently forgets is not a ledger. So there are three different acts, and they
are three different resources, because they are three different authorities:

| act | resource | what survives |
| --- | --- | --- |
| **close** | `…:{ledger}:close` | everything. The item is still in the graph, still queryable, and stops blocking what it blocked. This is the normal end of work. |
| **delete** | `Delete …:{ledger}:item:{id}` (`urn:cap:ledger:delete:{ledger}`) | the item's quads, its comments and the edges pointing at it MOVE to that ledger's own `urn:iki:ledger:graph:{ledger}:deleted`. Out of every ledger read; still there; recoverable. A **tombstone** stays in the live graph. |
| **purge** | `…:{ledger}:purge` (`urn:cap:ledger:purge:{ledger}`) | only the tombstone. The content is destroyed in both graphs. |

The tombstone carries the number, the time, the actor, the reason, the quad count and the
`sig:contentHash` (`sha256:…`) of exactly what was removed — so a later claim about what an
item said is checkable, and a ledger that forgot something still records that it did. The
hash is over sorted triples with their term kinds and datatypes: there are no blank nodes
in this graph, so that IS a canonical form and the heavyweight dataset canonicalization
would buy nothing.

The graveyard is **per ledger**, not shared. One graveyard would be a graph that every
ledger's deletes write into — one graph, one write scope, and therefore a path across the
boundary the rest of this is built to keep closed.

### ★ A delete is two writes, and the window between them has a name

Moving quads from the live graph to the graveyard **cannot be one update**, because a
scoped write can neither read nor write across graphs — that is what makes it a boundary
rather than a filter. So a delete is: read the live quads, write them into the graveyard,
then remove them from the live graph and write the tombstone (those last two are one
update, so *that* pair is atomic). A purge clears the graveyard first, then the live graph.

⚠ **The graveyard is touched first and the live graph last, deliberately: the live graph is
the commit point.** Every read here looks at the live graph and none looks at the graveyard,
so a process that dies between the two writes leaves the item **entirely present and
undeleted**, with a copy already archived that no reader can see. A reader never observes a
half-deleted item — it sees the item, whole, until the moment it does not. And the state is
*re-runnable*, not merely recoverable: the archive step is `INSERT DATA` of quads the graph
may already hold, and a store is a set, so issuing the same delete again converges. The
other order would have put the window on the side where a crash destroys data.

⚠ The one wrinkle: if an item is *edited* between an interrupted delete and its retry, the
graveyard ends up holding the union of both versions. The tombstone's hash then covers the
retry's quads, which is what was actually removed and is the honest answer; the archive is
a superset of it.

⚠ **A delete therefore needs write authority over two graphs** — the ledger's and its
graveyard's — and the grant table below says so per verb.

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

⚠ A **level** is not a **ledger** and neither is a **layer**. Three axes, and collapsing
any two would be a mistake:

| axis | what it separates | mechanism |
| --- | --- | --- |
| **level** | granularity — a decision vs an issue vs a finding | an RDF class and its shape |
| **ledger** | partition and authority — a project, a client | a named graph and a capability |
| **layer** | participant — a person, a reviewer, an agent, a peer host | a named graph and a capability |

Ledger and layer are the *same mechanism on different axes*, which is a unification and
not a conflation: this crate builds nothing called a layer, but the primitive one would
need is here and has been driven by a concrete need rather than a design note. A graph that
is trying to be both is a graph nobody can reason about, so a host that adds the other axis
gives it its own graphs rather than overloading these.

## What is enforced, and where

`urn:cap:ledger:{read,write,delete,purge}:{ledger}` gate this module — **one grant per
ledger, matched exactly**. Delete and purge are separated from the everyday write grant
because they take work *out* of the record, and a ledger holds whatever anyone filed, so an
unrestricted read is not free either.

An action declares the **family** (`urn:cap:ledger:write:*`, "holds some ledger write
grant") and enforces the exact scope for the ledger the IRI named. The two halves are the
same scope; only the exactness differs, and the split exists because the kernel's
capability pre-check runs before an endpoint can read the ledger out of its own target.
`ikigai-store`'s per-graph write door is shaped identically.

⚠ The name goes **last** in the token — `urn:cap:ledger:read:acme`, never
`urn:cap:ledger:acme:read` — because `ikigai-core` matches a wildcard only as a trailing
`*`. There is no infix form, so a parameter that is not last cannot be declared as a family
at all.

### The store scopes: every door this crate goes through names one graph

A sub-request carries the *caller's* capability unchanged — `Invocation::issue` has no
attenuating or elevating form — so whatever this crate asks the store for, the caller must
hold. That used to mean `urn:cap:store:read` (the whole dataset) and `urn:cap:store:write`
(`DROP ALL`), which made "may file an item" and "holds the keys to the store" the same
grant, and let anyone holding the read grant query another ledger's graph directly, going
around every capability checked here.

**`ikigai-store` 0.2.2 closed both halves and this crate takes them.** Nothing here
resolves `urn:iki:store:select` or `urn:iki:store:update`; every read is
`urn:iki:store:graph-select` and every write is `urn:iki:store:graph-update`, each naming
one graph, each under a grant that names that same graph. So the grants an operator issues
are these — **per ledger, and a grant for `acme` is worth nothing at `bosatsu`**:

| to do this in ledger `L` | grant at this module | …and at the store |
| --- | --- | --- |
| read (`items`, `item`, `next`, `ledgers`) | `urn:cap:ledger:read:L` | `urn:cap:store:read:graph:urn:iki:ledger:graph:L` |
| write (`append`, `comment`, `close`, `reopen`, `claim`, `defer`, `link`, `label`, editing an item) | `urn:cap:ledger:write:L` | the read grant above **and** `urn:cap:store:write:graph:urn:iki:ledger:graph:L` |
| delete (`Delete` on an item) | `urn:cap:ledger:delete:L` | both of the above **and** `urn:cap:store:write:graph:urn:iki:ledger:graph:L:deleted` |
| purge | `urn:cap:ledger:purge:L` | the same three as delete |

⚠ **Delete and purge need a write grant for TWO graphs**, because the graveyard is a
second graph and a scoped write cannot reach across. That is the one line of this table an
operator will get wrong, which is why it is a row and not a footnote.

⚠ **The two halves are separate grants and neither implies the other.**
`urn:cap:ledger:read:acme` without the store's graph-read token is a ledger you may address
and cannot read, and `urn:iki:ledger:ledgers` **refuses** rather than leaving it out — a
ledger missing from the inventory is indistinguishable from one with nothing filed in it, and
the refusal names the exact token to add. That is a host misconfiguration with a mechanical
fix, and it is the one this table exists to prevent.

Each action **declares** the family (`urn:cap:store:read:graph:*` — "holds some grant under
this prefix") and the store **enforces** the exact graph, because the kernel's capability
pre-check runs before an endpoint can read the ledger out of its own target. The same split
the ledger's own grants use, for the same reason.

### What this closes, and the one thing it does not

⚠ **First, the direction that is easy to forget: the table above has to be ENOUGH.** A
boundary is two claims — these grants reach no further, *and* these grants reach this far —
and only the first of them is interesting to write a test for, which is why 0.2.0 shipped
with the second one holed. `item:{id}`'s `Source` and `Exists` went on declaring the broad
`urn:cap:store:read`, so a caller holding exactly this table could list a ledger and file
into it and was **denied reading a single item out of it**; the only workaround was the
whole-dataset grant this section exists to stop needing. Nothing caught it because every
`item:{id}` read in the suite ran under root, where a broad token is held by definition —
the boundary was tested everywhere except the action that broke it. Fixed in 0.2.1, and
`tests/grants.rs` is now the shape that keeps it fixed: it reads **every** `(resource,
verb)` pair off the bound space itself, refuses to pass if any of them is unexercised, and
runs all of them against a live ledger under exactly this table and nothing more.

**And the direction that is easy to remember: a caller holding only the grants above cannot
reach another ledger by any route this crate offers or composes over.** Eight doors are
tried in
`tests/ledgers.rs::a_caller_granted_one_ledger_cannot_reach_another_by_any_route` — this
module's listing, ready set and append at the other ledger, the store's two broad doors,
and the store's narrow doors aimed at the other ledger's graph and at its graveyard — and
all eight are refused. Until 0.2.0 that file carried the opposite test, asserting the leak
and telling whoever made it fail to come and fix this paragraph.

⚠ **A host that hands a ledger caller `urn:cap:store:read` anyway has given it every graph
in the store**, and the store is right to answer: that grant means the whole dataset and
always did. The change is *what kind of fact that is*. It was a *substrate hole* — the
narrow grant did not exist, so the broad one was the only way to run a ledger at all. It is
now a *host-configuration decision*: nothing this crate declares asks for the broad grant
(**since 0.2.1**, and `tests/grants.rs::no_action_declares_a_scope_outside_the_published_grant_list`
is what makes that sentence checkable rather than merely written down), a host reading the
manifold sees only the per-graph families, and issuing the broad one is a choice with no
reason behind it. `a_host_that_hands_out_the_broad_store_grant_still_has_a_bypass` pins the
other half in `tests/ledgers.rs`, because the two facts are only useful together.

Two consequences worth stating plainly:

- **This is a tenancy boundary now**, where before it was a set of doors on one side of an
  open room. An agent's grants really are the set of ledgers it can touch — through this
  module and through the store underneath it.
- **It is not a boundary against the host itself.** Whoever configures the kernel chooses
  what every caller holds, and a host that binds the store's broad doors on a served
  transport has made a different decision about a different resource. That is the store's
  surface to reason about, not this one's.

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

- **No HTML face.** Items serve `text/turtle` and `text/plain`. A rendered face belongs with
  the rest of the browse stack, which dispatches XSLT on `rdf:type`; nothing here forecloses
  it.
- **No event graph.** kata records every mutation as an event row. Here the mutation history
  is the kernel's to tell — `ikigai-log`'s tracer already writes resolutions, cache hits and
  capability denials — and duplicating it in the domain graph would be two records to
  disagree.
- **No timed claims.** Claims are held until released; nothing expires them. A claim race
  between two requests in one process is also possible — RDF has no partial unique index, and
  SHACL cannot see a race. Both are safe for a single operator and are the first things to
  harden for more than one.
- **No `about` removal, no comment editing.** Comments are append-only by design; `about`
  removal is an omission, not a principle.
- **No `deferred-until` date.** Readiness that turns on the wall clock would go stale in the
  cache with no golden thread to cut it, so resuming is an act rather than the passage of
  time.

## Status

All fourteen resources are bound, tested, and walked clean by `ikigai-conformance`
(`AUTHORITY` included — every mutating action declares the scope it enforces).

**A host must bind this crate's space for the resources to resolve.** It composes with
`ikigai-store`'s space — store first, ledger second, behind a `Fallback` — and the store's
`DurableStore::open` is what names the dataset on disk. See "Composition" above.

### 0.2.1 made the published grant list true for `item:{id}` as well

A patch, and a narrowing: `urn:iki:ledger:{ledger}:item:{id}`'s `Source` and `Exists`
declared the broad `urn:cap:store:read` where every other read declared the per-graph
family. A caller who held the broad token still works — it is a *declaration* that got
narrower, not an enforcement that got stricter — and a caller holding only what the grant
table above publishes starts working, which it should have done in 0.2.0.

The line was one token. The reason it survived is the part worth recording: the two
authoring forms of the same idea (flat on a single-authority endpoint, per-verb on
`item:{id}`, whose four verbs carry three authorities) each held their own literal, and one
of them was never revisited when 0.2.0 narrowed the other. They share one list now, and
`tests/grants.rs` checks every action against the published table instead of against root.

### 0.2.0 renamed every resource and every grant, and narrowed the store's

Named ledgers moved the IRIs and this crate's capability tokens; taking `ikigai-store`'s
narrow doors replaced the store tokens a caller must hold. 0.2.0 is a breaking change to
both sets, and the two landed together **on purpose** — 0.1.0 was published the day before,
so doing them in one version changes an operator's grant list once instead of twice:

| 0.1.0 | 0.2.0 |
| --- | --- |
| `urn:iki:ledger:append` | unchanged — it now means the ledger `default` |
| `urn:iki:ledger:item:{id}` | resolves, but items are *minted* at `urn:iki:ledger:default:item:{id}` |
| `urn:iki:ledger:graph` | `urn:iki:ledger:graph:default` |
| `urn:iki:ledger:graph:deleted` | `urn:iki:ledger:graph:default:deleted` |
| `urn:iki:ledger:counter` | `urn:iki:ledger:default:counter` |
| `urn:cap:ledger:write` | `urn:cap:ledger:write:default` |
| `urn:cap:store:read` | `urn:cap:store:read:graph:urn:iki:ledger:graph:{ledger}` |
| `urn:cap:store:write` | `urn:cap:store:write:graph:urn:iki:ledger:graph:{ledger}` — plus the `…:deleted` twin for delete and purge |

⚠ The last two rows are **not** a rename: a host that leaves the old broad tokens in place
finds that ledger writes stop working, because `urn:iki:store:graph-update` takes the
per-graph grant and nothing else. That is deliberate on the store's side — a narrow door
accepting a broad key would make every declaration in this crate a lie. `ikigai-store`
**0.2.2 is the minimum**; against 0.2.0 or 0.2.1 the reads have no door to go through.

There is **no migration code**, deliberately: nothing depends on 0.1.0, so a host with data
rewrites the graph and the subject prefix by hand rather than carrying a converter nobody
will ever run twice.
