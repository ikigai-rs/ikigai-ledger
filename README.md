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
| `…:{ledger}:item:{id}:state` | Source · Sink | the one lifecycle state; a compare-and-set transition | `read` / `write` of `{ledger}` |
| `…:{ledger}:item:{id}:state:{value}` | Exists | is it in that state? | `read:{ledger}` |
| `…:{ledger}:item:{id}:holder:{holder}` | Exists | is it held by that holder? | `read:{ledger}` |
| `…:{ledger}:item:{id}:closed` | Exists | is it closed? | `read:{ledger}` |
| `urn:iki:ledger:ledgers` | Source | which ledgers exist | any `read:*` |
| `urn:iki:ledger:policy:{name}` | Source · Exists | what a policy weighs | any `read:*` |
| `urn:iki:ledger:lifecycle:{name}` | Source · Exists | the legal states, in order | any `read:*` |

The last three carry no ledger segment because neither is a ledger's own state: the
inventory spans them, and a policy is a property of the host's configuration. **The
capability column is only half the grant** — every row but the last also needs the store's
per-graph token for that ledger; see "What is enforced, and where" for the whole list.
`urn:iki:ledger:policy:{name}` and `urn:iki:ledger:lifecycle:{name}` are the exceptions that
need no store grant at all: each is configuration the host registered at boot, and nothing
about it is in the graph.

Every read serves `text/plain` (the default — a line per item, greppable) and
`text/turtle` (the graph); `items`, `item:{id}` and `next` also serve `application/json`,
and so do the answers of `append`, `comment`, `close` and `link` — see "The JSON face"
below. An `as=` this module cannot answer in is **refused**, never substituted.

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
be true — so `append`, `items`, `next`, `policy`, `ledgers`, `lifecycle` and thirteen others are not
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
base32 over the millisecond clock — so ids sort in filing order — plus 13 more that make it
unique by construction: a process-wide sequence every mint increments, offset by a
per-process random nonce. The clock half makes a raw IRI listing readable; the sequence
means two ids minted by one process can never collide, and the nonce keeps two processes
(two ledgers merged later, a restart under a fixed clock) apart with 64 bits of chance.
Comments are minted the same way.

⚠ **Until 0.2.1 the second half was 6 characters of SHA-256 over the title and author**, on
the premise that one writer meant one write per millisecond. It does not: eight concurrent
filings of the same title minted one IRI between them (one item, eight numbers), and the
same note left on two items was one comment node that died with whichever item was deleted
first. Ids minted before 0.3 keep their 16-character shape and stay valid — an id is only
ever compared whole, never parsed.

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

## Keys: file once, under your own name

`append key=<k>` files an item under **the caller's own name for it** — a hook's finding id,
an importer's issue id — and that name is unique in the ledger. If an item already carries
the key, **nothing is filed** and the answer names that item, marked:

```text
$ sink urn:iki:ledger:append key=urn:kata:issue:01JZ <<< "Port the importer"
#7 urn:iki:ledger:default:item:01m4a2…
$ sink urn:iki:ledger:append key=urn:kata:issue:01JZ <<< "Port the importer (re-run)"
#7 urn:iki:ledger:default:item:01m4a2… existing open
```

A filing answers exactly as before, `#N <iri>`; `existing` is a third word a filing never
has, followed by the item's status (`open`, `closed` or `deleted`). In the JSON face it is
`"outcome": "existing"`.

★ **The check and the filing are one store update.** The append's SPARQL carries
`FILTER NOT EXISTS { ?taken ledger:key "<k>" }` beside the counter's allocation, so when the
key is taken the update matches nothing and writes nothing — no item, no number spent — and
the store holds its write lock across one update's evaluation and insert, so two appends
cannot both see the key free. That is the whole point: the pattern it replaces (list the
items `about` the key, append when the list is empty) is a check-then-act race, and two
hooks running at once both filed. `tests/keyed_append.rs` reproduces that race and pins the
fix — eight concurrent keyed appends, twenty rounds, one item each time — and
`tests/durable.rs` runs the same on RocksDB.

**A deleted item's key stays taken.** The tombstone a delete leaves carries the key (and
keeps it through a purge, as it keeps the number: a key is a name, not content), so a keyed
append replayed after a delete answers `existing deleted` rather than filing the item
again. A keyed append is exactly the request that gets replayed — a hook re-run, an import
run twice — and a replay must not resurrect what someone deliberately deleted. Filing it
again is a deliberate act: another key, or none.

**A replay does not write, so it does not invalidate.** Because a taken key stays taken, the
append looks the key up first and answers `existing` without sending the store an update at
all. That matters more than the round trip it saves: the kernel cuts
`urn:iki:store:graph-update` after every successful write *whether or not it changed
anything*, so a no-op update still invalidated every cached `next`, `items` and item read,
and a re-sync that sends one keyed append per task it knows about was a cache flush. A key
read as free proves nothing, so a free key still goes through the guarded update above. The
one remaining no-op write is the race the guard exists for: two appends that both read the
key as free, where the second one's update finds it taken and still cuts.
`tests/noop_keyed_append.rs` pins both halves — a replay leaves cached reads cached, a
filing still invalidates them.

A key is ASCII letters, digits, `-`, `.`, `_`, `~` and `:`, at most 256 characters, compared
exactly. The shape is fixed so a key is a legal IRI segment **and survives gonk's HTTP
door**, which maps an IRI to a URL path by turning every `:` into a `/` — so a `/` in a key
would come back as a different key. Both foreign ids the gonk bridges carry fit as they
stand. It is unique **per ledger**: two ledgers may use one key.

Finding an item by its key:

| form | where |
| --- | --- |
| `urn:iki:ledger:{ledger}:item:key:{key}` | the item resource itself — Source, Exists, Sink, Delete |
| `item=key:{key}` | every write that names an item (`comment`, `close`, `link`, `claim`, …) and `purge` |
| `items key={key}` | a filter, composing with the others — `status=all` to include a closed item |

## The JSON face

`as=application/json` is the machine contract; the plain face is for people and is not a
format to parse. Every document is one compact line carrying `"schema": 1` and `"ledger"`.
**Adding a field does not change the schema number; renaming, removing or retyping one
does.** Absent values are `null`, times are `xsd:dateTime` strings in UTC, and the item
object is the same wherever it appears:

```json
{"schema":1,"ledger":"default","item":{
  "number":12,"display":"#12","iri":"urn:iki:ledger:default:item:01m4…",
  "kind":null,"title":"Fix the thing","body":"It is broken.",
  "status":"open","closed_reason":null,"priority":1,"deferred":false,
  "labels":["rust"],"about":["urn:repo:file:x/src/a.rs"],"key":"urn:roborev:finding:0123…",
  "author":"brian","revision":null,"claim":{"holder":"satellite","purpose":"brief-x"},
  "created":"2026-09-15T00:00:00.000Z","modified":"2026-09-15T00:00:00.000Z",
  "links":[{"type":"blocks","target":{"number":13,"display":"#13","iri":"urn:iki:ledger:default:item:01m5…"}}],
  "comments":[{"id":"urn:iki:ledger:default:comment:01m6…","author":"chris",
               "time":"2026-09-15T00:00:00.000Z","text":"looked at it"}],
  "defects":[],"state":"filed"}}
```

`state` is the item's lifecycle state: `filed` when it holds none, and `null` only when an
out-of-band write left it holding several (see "Lifecycle state").

`defects` is empty for every item written through a ledger Sink. When an out-of-band write
left one of its timestamps unreadable or missing, it names that value
(`unreadable dcterms:modified "yesterday"`) and the other timestamp stands in for it in
`created`/`modified` — so both stay `xsd:dateTime` strings. See "Assume an editor got there
first".

| resource | document |
| --- | --- |
| `item:{id}` | `{schema, ledger, item}` |
| `items` | `{schema, ledger, count, items: [item…], unreadable: [{iri, defects}]}` — `unreadable` is the plain face's ⚠ footer, as data: the items NOT in `items`. An item listed with a stand-in timestamp carries its own `defects` instead |
| `next` | `{schema, ledger, policy, weighs, generated_at, ready, ranking: [{rank, score, because, item}], excluded: [{why, reason, holder, blocked_by, item}]}` — `why` is `claimed`, `deferred` or `blocked` |
| `append`, `comment`, `close`, `link` | `{schema, ledger, outcome, item: {number, display, iri}, …}` — `outcome` is `filed`, `existing`, `commented`, `closed`, `linked` or `unlinked`, and only that outcome's own fields follow: `status` and `key` (append), `comment` (comment, and close with a note), `reason` (close), `type` and `target` (link) |

`links` are the item's **outbound** edges — `blocks`, then `parent`, then `related` — and a
target that is not a live item in the ledger has a `null` number. The documents are Rust
types in `ikigai_ledger::json`, `Deserialize` as well as `Serialize` and `#[non_exhaustive]`,
so a Rust consumer reads the same type this crate writes and a later field does not break
it. `tests/faces.rs` pins every document as a literal, and pins the plain face byte for
byte as 0.3.0 rendered it — the one change there is a `key:` line in an item's detail, and
only for an item that has a key.

## Assume an editor got there first

A ledger whose items can only change through its own `Sink` is not what anyone wants from
durable, inspectable state: the value of a record you can keep is that a human in an editor
— or a merge, or an LLM harness, or a bulk load — can touch it out of band. Here that is
literally true already: anything holding this graph's write grant — or the store's broad one
— can rewrite it without passing through a ledger endpoint. So an **out-of-band write is a
supported path, not corruption**, and three things follow.

**Identity survives editing.** An item's IRI is minted once, from the clock and a sequence,
and then *stored*. It is never derived from the item's content, its number
or its position, so rewriting a title, renumbering, or reformatting in an editor cannot
silently rename the thing. This is the decision that would be worst to retrofit and it is
made here, at the first commit.

**The model is checked on read, not only on write.** A hand edit bypassed the Sink's
refusals, so `urn:iki:ledger:items` runs a corpus check beside the listing: an item typed
`ledger:Item` that is missing anything a reader needs — or carries it in a form the reader
cannot parse, such as a `dcterms:created` written as an `xsd:date` — is **reported**, with
what is wrong, rather than quietly dropped — the worst outcome being work that is neither
visible nor gone. Asking for such an item directly says what is wrong with it, which "not
found" would not. `model::REQUIRED` is the one list both directions use.

**And one bad timestamp does not take an item out of the list.** An unreadable or missing
`dcterms:created` or `dcterms:modified` is read as ABSENT, and the other one stands in for it:
created ≤ modified, so a missing `modified` is at least the `created` and a missing `created`
is at most the `modified`, and neither is invented. The item is listed, offered by `next`
(and its `blocks` edges still hold), and readable at its own IRI, and every face says which
value was read around: the item's `defects` in JSON, a `⚠ defect:` line in its detail, a ⚠
footer under `items`, and the value as stored, never the stand-in, in Turtle. Only an item
with no readable timestamp at all, or no readable `ledger:number` (its name on every face,
which nothing can stand in for), is still left out and reported. Any write through a ledger
Sink re-stamps `dcterms:modified`, so the ledger's own path repairs that one; nothing here
ever writes a bad timestamp, so this is only ever an out-of-band edit.

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
| **purge** | `…:{ledger}:purge` (`urn:cap:ledger:purge:{ledger}`) | only the tombstone. The content is destroyed in both graphs — and an item that was already deleted is purged the same way, found by its tombstone, so a secret deleted first and purged second is really gone. |

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
then remove **exactly those quads** from the live graph and write the tombstone (those last
two are one update, so *that* pair is atomic). A purge clears the graveyard first, then the
live graph.

⚠ **"Exactly those quads" is the fix for a real loss.** Until 0.2.1 the removal re-ran the
selector, so a comment that landed between the read and the removal was removed without
ever being archived — a recoverable delete that destroyed an acknowledged write, every time
the two raced. Removing the rows that were read makes what was archived, what was removed,
and what the tombstone hashes and counts one set. A write that races a delete is kept, in
the live graph beside the tombstone: an orphan of the deleted item, which is where a write
arriving just after a delete has always landed, and never a loss. A purge still removes by
selector — it is asked to leave nothing of the item — so a write racing a purge goes with
it.

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
graveyard's — and a purge needs to READ the graveyard too, because it finds and counts what
an earlier delete archived. The grant table below says so per verb.

**Why purge is a resource and not a `purge=true` argument.** A flag could only be enforced
at runtime, and an action that enforces a scope it does not declare makes the manifold lie —
worse than the converse. Declaring both scopes on one action would demand purge authority
for every ordinary delete. The authority difference gets its own IRI, exactly as
`ikigai-store` gives each SPARQL form its own.

## Selection: `next` is a resource, and its policy is a seam

Two stages, kept apart:

1. **The ready set is deterministic and is a query**: open, unclaimed, not deferred, not
   blocked by an open item, plus the caller's label / `about` / level filters. No policy is
   involved, and most of the value is here. ⚠ **"Blocked", the cycle check and leverage
   are computed over EVERY open item, and the filters narrow afterwards** — a blocker
   without the filter's label still blocks, and a ledger of any size is read whole. Until
   0.2.1 both were computed over the filtered pool, loaded through a listing's 500-row
   bound, so a filter or a long backlog could make blocked work look ready.
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

★ **`next` is cached until the ledger changes OR its policy's answer could, whichever is
first.** A write cuts the store's threads, but the passage of time cuts nothing, and reading
the clock records no dependency — so until 0.2.1 an idle ledger served `leverage`'s day-zero
age points indefinitely. `OrderingPolicy::valid_until` says until when a policy's answer
holds: `priority-recency` never reads the clock (cached until a write, as before), `leverage`
holds until the next day boundary of an age it prints, and a host policy that does not
implement it is **never** served from cache — the safe default, since only the policy knows
what it does with `now`. Measured over 500 open items in a debug build: a cached `next` reads
in ~15 µs before and after; a recomputation costs ~12 ms, paid once per day under `leverage`
and on every read under a policy that does not say.

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

## Lifecycle state: one value, moved by one update

kata-flight keeps an item's place in its shipping loop in `lifecycle:*` labels, "at most one" by
convention, and moves it by removing one label and adding the next. That is two writes, so a
crash or a race between them leaves two labels, which its reaper then hunts for. This ledger
has label add and label remove too, and **composing a transition out of them would reproduce
that defect exactly** (`tests/lifecycle_state.rs` reproduces it, as a record). So the state is
its own resource:

```text
$ source urn:iki:ledger:item:12:state
filed
$ sink urn:iki:ledger:item:12:state from=filed to=queued
#12 queued (was filed)
$ sink urn:iki:ledger:item:12:state from=filed to=reviewed
conflict: #12 is in `queued`, not `filed`. A transition is a compare-and-set: …
```

- **One value or none.** An item holds at most one `ledger:state`, and an item that holds none
  is in the state named **`filed`**, so absence is a state with an owner and not a residue.
- **A transition is a compare-and-set in ONE store update.** `to=` and `from=` are both
  required; the update's `WHERE` holds only while the item is in exactly `from`, and with no
  solution the `DELETE … INSERT` does nothing. The store holds its write lock across one
  update's evaluation and insert, so an interrupted transition leaves the old state or the new
  one, never both and never neither, and of two writers moving one item out of one state
  **exactly one wins and the other gets a typed `Conflict` naming the state the item is in
  now**. The update cannot report whether it applied, so it writes a receipt
  (`ledger:stateToken`, minted per transition) and reads it back; a read of the state alone
  could not tell two racers apart when both asked for the same `to=`. Twenty rounds of that
  race run in memory and five on RocksDB.
- **A state outside the lifecycle is refused** (`InvalidArgument`, naming the legal ones)
  before anything is read. `from=X to=X` is answered `unchanged` without a write, because a
  no-op update still invalidates every cached read of the ledger. A refused transition writes
  nothing, not even `dcterms:modified`.
- `content=` is an optional note, recorded as a comment when the item moves; `as=application/json`
  answers `{schema, ledger, item, lifecycle, state, in_flight, drain, since, outcome, from}`.
- `close` does not touch the state, and the state does not touch `close`: they are different
  facts, and a loop that wants an item closed AND out of the flight asks for both.

### Lifecycles are resources

`urn:iki:ledger:lifecycle:{name}` is a small Turtle document: the states in order, which are
**in flight** (worked under a claim) and, for every state that is not, the **drain** that owns
an item parked there. One ships built in, **`kata-flight`**:

| state | in flight | drain |
| --- | --- | --- |
| `filed` (no `ledger:state`) | no | `triage` |
| `queued` | no | `review-gate` |
| `reviewed` | no | `ship` |
| `resolving` | yes | — |
| `refining` | yes | — |
| `shipping` | yes | — |

The state names are kata-flight's; the drain names are ours. A state's IRI is
`urn:iki:ledger:lifecycle:{lifecycle}:{state}` and that IRI is what `ledger:state` holds, so a
state from another lifecycle can never pass for one of this one's. A host brings its own the
way it brings its own ordering policies:

```rust
let config = ikigai_ledger::SpaceConfig::default().lifecycles(
    Lifecycles::new(vec![Lifecycle::kata_flight(), Lifecycle::parse(MY_TTL)?])
        .assign("acme", "my-lifecycle"),
);
let space = ikigai_ledger::space_with(config);
```

**One lifecycle per ledger, fixed by the host** — the first registered, unless `assign` names
another — where an ordering policy is chosen per request. A policy only orders an answer; a
lifecycle decides which values a state may hold, and two of them in one ledger would let two
writers disagree about what is legal. `Lifecycle::parse` refuses, at boot, every shape the
state resource relies on not having: a state whose IRI is not `{lifecycle}:{name}`, duplicate
names or orders, a state outside the flight with no drain, a state in flight with one, and a
lifecycle that does not declare `filed` first.

### Assertions: ask whether a step happened

```text
$ exists urn:iki:ledger:item:12:state:resolving
true
$ exists urn:iki:ledger:item:12:holder:urn:agents:session:4f2a
true
$ exists urn:iki:ledger:item:12:closed
false
```

A loop that has just claimed, moved or closed an item asks `exists` on the NAME of what should
now be true, and stops on `false` — so a skipped step is a failed check rather than a narrated
success. `holder:none` asks whether it is unclaimed and `holder:any` whether anyone holds it
(the `holder=` filter's keywords). An assertion about an item that does not exist is `false`; a
state name the ledger's lifecycle does not declare is **refused**, so a typo cannot read as "not
in that state". Each is a cacheable read under the store's write threads, so an unchanged
ledger answers from cache and **a write to the item invalidates it** (pinned).

⚠ **An item part takes a number or an opaque id, never `key:{key}`.** A key may contain `:`, so
`item:key:a:state` could be the state of the item keyed `a` or the item keyed `a:state` — and
the second is what `item:{id}` has answered since 0.4.0, so it keeps it. Reach a keyed item's
parts through its number or its IRI (`{iri}:state` works, since the IRI's id is opaque).

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
| purge | `urn:cap:ledger:purge:L` | the same three as delete **and** `urn:cap:store:read:graph:urn:iki:ledger:graph:L:deleted` |

⚠ **Delete and purge need a write grant for TWO graphs**, because the graveyard is a
second graph and a scoped write cannot reach across. That is the one line of this table an
operator will get wrong, which is why it is a row and not a footnote. ⚠ **And purge needs the
graveyard's READ grant as well** (0.3): it looks there for what an earlier delete archived,
and the store refuses a `DELETE … WHERE` over a graph without the read grant for it. Without
it, purge refuses before touching anything and names the token.

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

- **No HTML face.** Items serve `text/plain`, `text/turtle` and `application/json`. A rendered face belongs with
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
- **No key on an existing item.** A key is given at filing and never changed, so an item
  filed before keys existed cannot gain one through a resource here — that is one SPARQL
  UPDATE over the graph, and a migration's job rather than an endpoint's.
- **No `about` removal, no comment editing.** Comments are append-only by design; `about`
  removal is an omission, not a principle.
- **No `deferred-until` date.** Readiness that turns on the wall clock would go stale in the
  cache with no golden thread to cut it, so resuming is an act rather than the passage of
  time.

## Status

All nineteen resources are bound, tested, and walked clean by `ikigai-conformance`
(`AUTHORITY` included — every mutating action declares the scope it enforces).

**A host must bind this crate's space for the resources to resolve.** It composes with
`ikigai-store`'s space — store first, ledger second, behind a `Fallback` — and the store's
`DurableStore::open` is what names the dataset on disk. See "Composition" above.

### Next (a minor release): lifecycle state and assertions (ledger #775, step 1)

The first of kata-flight's ledger seams. Additive in behavior — every request 0.4.2 accepted
answers the same bytes in the plain face, and the JSON face gains one field — but **a minor
release**, because three public constants change type: `ledger::RESERVED` gains `lifecycle`
(19 words), `vocabulary::STRUCTURAL_CLASSES` gains `Lifecycle` and `LifecycleState` (12), and
a `[&str; 18]` binding of the old array stops compiling. And `lifecycle` is now a reserved
ledger name: a ledger called that could not be told apart from `urn:iki:ledger:lifecycle:{name}`.

- **`…:item:{id}:state`** (Source, Sink): one state or none (`filed`), moved by a compare-and-set
  in one store update; a lost race is a typed `Conflict`. See "Lifecycle state".
- **`urn:iki:ledger:lifecycle:{name}`** (Source, Exists): the lifecycles a host offers; the
  built-in is `kata-flight`. `SpaceConfig` and `space_with` take a host's own.
- **Assertions** (Exists): `…:item:{id}:state:{value}`, `…:holder:{holder}`, `…:closed`.
- **JSON**: every item object gains `state`. The plain item detail gains a `state:` line, only
  for an item that has one.
- Vocabulary: `ledger:Lifecycle`, `LifecycleState`, `hasState`, `order`, `inFlight`, `drain`,
  `state`, `stateAt`, `stateToken`.

### 0.4.2 (2026-10-07): one bad timestamp no longer drops an item (ledger #866)

A patch, by this crate's reading of the JSON face's rule (a field ADDED, nothing renamed,
removed or retyped). On 0.4.1 an item whose `dcterms:modified` or `dcterms:created` was not a
readable `xsd:dateTime` (a hand edit through `urn:iki:store:graph-update`, say
`dcterms:modified "yesterday"`) was left out of `items` with a footer, left out of the Turtle
face and out of `next` with no word at all, and refused at its own IRI. Out of `next` was the
expensive one: the item's `blocks` edges went with it, so what it blocked was offered as
ready. Now the timestamp is read as absent and the other one stands in (see "Assume an editor
got there first"), the item is listed everywhere, and every face names the defect: JSON items
gain a `defects` array (`#[serde(default)]`, so a 0.4.1 document still deserializes), and
`model::Item` gains `defects` (it is `#[non_exhaustive]`, so adding a field breaks no one).
`model::defects` now reports only what the readers drop: no readable timestamp, no readable
number, no title or status. Two side effects, both fixes: an item with two `dcterms:modified`
values was listed twice and is now listed once, with the latest; and a ledger `Sink` on such
an item now succeeds and repairs the stamp, where 0.4.1 refused it.

### 0.4.1: a keyed append whose key is taken answers without a write (ledger #822)

A patch. On 0.4.0 a keyed append whose key was already taken still sent its guarded update; the
update wrote nothing, but the kernel cuts the target of every SUCCESSFUL mutating request, so
every replay (a hook re-run, an import or a spec sync run again) invalidated every cached read
of the ledger (`next`, `items`, item faces). `append key=` now looks the key up first and, when
it is taken, answers `#N <iri> existing <state>` (JSON `outcome: existing`) with no store write
at all. Safe because a key never becomes free again (a delete moves it onto the tombstone, a
purge keeps it). A key read as free still goes through the guarded update, so concurrent appends
still file exactly once; only the race's loser still cuts (rare, and correct). A real filing
does one extra cacheable read. No API change.

### 0.4.0: a keyed append and a JSON face (ledger #779)

Additive in behavior — every request 0.3.0 accepted answers the same bytes in the plain
face — but **a minor release, not a patch**, for one reason: `model::Item` gains a public
`key` field and `model::Filter` a public `key` filter, and both are constructible structs,
so a struct literal outside this crate stops compiling. No consumer in the ecosystem builds
either (searched), but a `"0.3"` pin will not pick this up, which is the point of the rule.
And since this release breaks that pin anyway, `model::Item` and `model::Filter` are now
`#[non_exhaustive]`: outside this crate build a `Filter` from `Filter::default()` and set
its fields, and read an `Item` rather than constructing one, so the next field is a patch.

- **`append key=`**: at most one item per key per ledger, checked and filed in one store
  update; a taken key answers the existing item (`#N <iri> existing <status>`). A deleted
  item's key stays taken. See "Keys" above.
- **Lookup by key**: `urn:iki:ledger:{ledger}:item:key:{key}`, `item=key:{key}`, and the
  `items key=` filter. `ledger:key` is in the vocabulary, with no `rdfs:domain` (the item and
  its tombstone both carry it, as with `ledger:number`).
- **`as=application/json`** on `items`, `item:{id}` and `next`, and on the answers of
  `append`, `comment`, `close` and `link`: schema 1, typed in `ikigai_ledger::json`.
- An item's plain detail shows `key:` after `iri:` when it has one; nothing else in the
  plain face changed.

### 0.3.0: the fixes from an unled audit of 0.2.1

Every item below was reproduced against 0.2.1 by a test that failed because of it
(`tests/data_safety.rs`, `tests/next_whole_ledger.rs`, `tests/audit_minors.rs`). What an
operator or a host has to know:

- **Grants.** Purge now needs the graveyard's READ grant too —
  `urn:cap:store:read:graph:urn:iki:ledger:graph:{ledger}:deleted` — eight tokens per ledger
  instead of seven. Without it purge refuses before touching anything, naming the token.
- **`ikigai-core` 0.1.80 is the floor**, for `Error::Conflict`: a claim held by someone else
  is now a conflict, not the transient `Unavailable` a circuit breaker counted.
- **Ids.** New ids are 23 characters (time, then a per-process sequence) and unique by
  construction; 0.2.1's identical filings or notes in one millisecond collided. Old ids stay
  valid. A selection's IRI carries a digest of its content.
- **Purge reaches a deleted item**, through its tombstone, which gains `ledger:purgedAt`,
  `ledger:purgeReason` and `ledger:purgedBy`.
- **A delete removes exactly what it archived**; a write that races it is kept, live.
- **`next`** computes blocked, cycles and leverage over every open item and narrows by the
  filters afterwards; it is cached until a write or until its policy's answer could change
  (`OrderingPolicy::valid_until`, whose default — for a host policy that does not implement
  it — is "do not cache").
- **Refused now, where 0.2.1 substituted or accepted:** an unknown `deferred=`, a `limit=`
  that is not a count, a label containing a comma (adding one; removing one still works), a
  claim holder named `none` or `any`, and `append kind=` naming one of this crate's own
  structural classes.
- **Answered differently:** `Exists` is `false` only for a missing item — a denial or a
  missing store is an error; a deleted item's number answers NotFound naming its
  tombstone; `urn:iki:ledger:item:{n}` works as an `item=` argument; a named ledger's `next`
  spells its own numbers (`acme#1`); `ledger:number` has no `rdfs:domain`, so a tombstone is
  never inferred to be an item.

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
