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

## Daily counts and legacy close times

`urn:iki:ledger:{name}:counts` (bare `urn:iki:ledger:counts` means `default`)
serves `text/csv` by default and `text/turtle` with `as=text/turtle`:

```text
$ source urn:iki:ledger:acme:counts days=3
date,open,closed
2026-10-27,12,5
2026-10-28,10,8
2026-10-29,11,8
```

Each UTC date runs from the first filed item
through today; `days=N` keeps only the most recent N calendar days, for positive N.
An empty ledger returns only the CSV header (an empty Turtle graph). A same-day close
counts as closed that day. `all = open + closed`: only items filed by that day count.
Today's row is provisional. Turtle has one `ledger:DailyCounts` observation per day at
`urn:iki:ledger:{name}:counts:YYYY-MM-DD`, with `ledger:date`, `ledger:openCount` and
`ledger:closedCount`, and no blank nodes.

Close now records `ledger:closedAt` from the kernel clock beside its reason. Editing an
already closed verdict preserves the original close time. Reopen removes the item's
`closedAt` and `closedAtSource`; dated `ledger:StatusEvent` nodes retain close/reopen
history, including the backfill provenance. Events carry `onItem`, `status`,
`dcterms:created` and `eventSequence` (ordering transitions at the same millisecond).
Deleting/archiving/purging an item includes its events. The series describes the current
ledger graph: deleted items leave the series, just as they leave the ledger's listing.

Counts requires `urn:cap:ledger:read:{name}` and the same graph-read grant as `items`.
Reads are cached under the store's write threads; a write invalidates the answer and
the next read recomputes it. The cache also expires at the next UTC midnight, so an idle
ledger still extends through today. This is the graph's dated history projected onto the
current UTC date range; it does not scan comments or guess close dates on a read.

### Explicit one-time backfill (never at startup)

Old closed items have no reliable close timestamp. Counts refuses them until the operator
runs `backfill-closed-at`, under `urn:cap:ledger:write:{name}` and that
ledger's store graph read/write grants. To date every undated closed item in one command:

```text
$ sink urn:iki:ledger:backfill-closed-at <<< all
$ sink urn:iki:ledger:acme:backfill-closed-at <<< all
```

`content=all` uses each item's `updated` (`dcterms:modified`) as its estimated
`closedAt`, marks the item and close event `closedAtSource "backfill"`, and reports
`backfilled N items`. A second run reports `backfilled 0 items (unchanged)` and writes
nothing. Existing close dates (including dates selected by the per-item command) and
open items are left alone; `updated` is never changed. The command validates all candidate
timestamps before writing; missing, unreadable, multiple or pre-filing dates abort the
command without changes. Items concurrently edited, dated, reopened or deleted are
skipped by the write guard and excluded from the reported count; rerun to pick up
remaining undated closes. Writes use bounded batches to stay below the store's SPARQL
size limit; if a later store request fails, earlier batches remain dated and rerunning
safely completes the migration. A `comment` argument with `all` is refused.

For a reviewed per-item estimate, inspect the item's comments first. When its close
posted a comment, supply **that close's own comment IRI**, not a later comment:

```text
$ sink urn:iki:ledger:backfill-closed-at comment=urn:iki:ledger:default:comment:THE_CLOSE_COMMENT <<< '#12'
$ sink urn:iki:ledger:acme:backfill-closed-at <<< 'acme#7'
```

The per-item form without `comment` is for a close with no comment: it uses the item's `updated`
(`dcterms:modified`). Legacy comments have no originating-action marker, so the command
cannot identify the close's own comment automatically; the operator supplies that
association. There is no `prefer=last-comment` mode: the newest comment can be an
unrelated note or edit after the close, so that heuristic cannot identify a closing
comment reliably. It checks that the supplied comment belongs to the item and has a readable
time. Both paths write `ledger:closedAtSource "backfill"` on the item and its close event,
leave `updated` untouched, and preserve any existing `closedAt` on reruns. Open items,
invalid timestamps, and comments on another item are refused. No migration runs merely
by binding the space or reading counts.

These values are **estimates**, even when a closing note exists. Turtle observations
through the latest estimated backfilled close day carry `closedAtSource "backfill"`,
so a chart can label that historical interval approximate. Item and event markers retain
the provenance too; CSV intentionally retains exactly its three columns. Record the
migration date with the deployment to label the interval “approximate before <migration date>”. Close/reopen history before
this release cannot be recovered from legacy snapshots: backfill assumes an item was
open from filing until its estimated current close. An item already reopened before the
migration is treated as open throughout its legacy history. New transitions retain the
history needed to avoid that limitation going forward.

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

"The oldest open items" is a question about when each claim AROSE, which is not when it
was filed: a migration files hundreds of items in minutes. `append raised=YYYY-MM-DD`
records the first as `ledger:raised` (an `xsd:date`) beside `dcterms:created`, and an item
filed without it was raised when it was filed (ledger #240):

```sparql
# the oldest open items in one ledger, oldest claim first
SELECT ?number ?title WHERE {
  GRAPH <urn:iki:ledger:graph:default> {
    ?item a ledger:Item ; ledger:number ?number ; dcterms:title ?title ;
          ledger:status ledger:open ; dcterms:created ?created .
    OPTIONAL { ?item ledger:raised ?raised }
  }
}
ORDER BY COALESCE(?raised, ?created)
```

⚠ The store compares the `xsd:date` with the `xsd:dateTime` as points in time
(`tests/raised.rs` pins it, with a claim raised after everything was filed). SPARQL 1.1's
operator table does not define that comparison, so another engine may order the two kinds
apart: `ORDER BY COALESCE(STR(?raised), STR(?created))` is the portable spelling, since the
lexical forms (`2026-07-21`, `2026-09-16T02:03:13.608Z`) sort correctly as strings.

## Composition: this crate owns no bytes

Every read here is a SPARQL query issued at `urn:iki:store:graph-select`, and every write is
a SPARQL UPDATE at `urn:iki:store:graph-update` — the **narrow** doors, each naming one
ledger's graph and gated by a grant for that graph.
[`ikigai-store`](https://crates.io/crates/ikigai-store) (**0.3.0 or later**: the doors
arrived in 0.2.2, 0.2.10 is where they stopped being network egress (see "SPARQL never
leaves the process" below), and 0.2.21 is where a query inside every bound stopped planning
uncancellably for minutes) owns the dataset, the write lock and the golden threads; this
module owns the **domain**.

```rust
let store = DurableStore::open(&StoreConfig::load(Some("gonk"))?.path)?;  // owned
let space = Fallback::new(vec![
    Arc::new(ikigai_store::space(store)) as Arc<dyn Space>,
    Arc::new(ikigai_ledger::space()),
]);
let kernel = Kernel::new(Arc::new(space)).with_clock(Arc::new(SystemClock));
```

`space()` names itself `urn:iki:space:ledger` (`ikigai_ledger::SPACE_ID`); `space_with(…)` and
`space_with_policies(…)` claim no name, because the host chose their doors and names them.

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
| `…:{ledger}:counts` | Source | daily open/closed history, CSV or Turtle | `read:{ledger}` |
| `…:{ledger}:backfill-closed-at` | Sink | explicitly migrate one or all legacy close times | `write:{ledger}` |
| `…:{ledger}:count` | Source | how many items the same filter admits, and nothing else | `read:{ledger}` |
| `…:{ledger}:item:{id}` | Source · Sink · Delete · Exists | one item, edit it, delete it | `read` / `write` / `delete` of `{ledger}` |
| `…:{ledger}:append` | Sink | file a new item | `write:{ledger}` |
| `…:{ledger}:comment` | Sink | append a comment, optionally under your own key | `comment:{ledger}` or `write:{ledger}` |
| `…:{ledger}:close` | Sink | close with a reason and any evidence, releasing any claim | `close:{ledger}` or `write:{ledger}` |
| `…:{ledger}:reopen` | Sink | undo a close | `close:{ledger}` or `write:{ledger}` |
| `…:{ledger}:claim` | Sink · Delete | take it (optionally on a lease) / hand it back / take over an expired lease | `claim:{ledger}` or `write:{ledger}` |
| `…:{ledger}:defer` | Sink · Delete | not now / now again | `write:{ledger}` |
| `…:{ledger}:link` | Sink · Delete | blocks / parent / related | `write:{ledger}` |
| `…:{ledger}:label` | Sink · Delete | tag / untag | `write:{ledger}` |
| `…:{ledger}:purge` | Delete | destroy, leaving a tombstone | `purge:{ledger}` |
| `…:{ledger}:next` | Source | the ready set, ranked | `read:{ledger}` |
| `…:{ledger}:doctor` | Source | what is structurally wrong, every problem at once; repairs nothing | `read:{ledger}` |
| `…:{ledger}:item:{id}:state` | Source · Sink | the one lifecycle state; a compare-and-set transition | `read` / `write` of `{ledger}` |
| `…:{ledger}:item:{id}:state:{value}` | Exists | is it in that state? | `read:{ledger}` |
| `…:{ledger}:item:{id}:holder:{holder}` | Exists | is it held by that holder? | `read:{ledger}` |
| `…:{ledger}:item:{id}:closed` | Exists | is it closed? | `read:{ledger}` |
| `…:{ledger}:item:{id}:about` | Sink · Delete | add / remove what it is about, title and body untouched | `about:{ledger}` or `write:{ledger}` |
| `…:{ledger}:item:{id}:history` | Source | every event on the item, oldest first, with what each edit changed | `read:{ledger}` |
| `…:{ledger}:history` | Sink | an import principal's past claim, release, edit and status events for one item; history only | `write:{ledger}`, and enrolled as `ledger:Importer` |
| `urn:iki:ledger:ledgers` | Source | which ledgers exist | any `read:*` |
| `urn:iki:ledger:policy:{name}` | Source · Exists | what a policy weighs | any `read:*` |
| `urn:iki:ledger:lifecycle:{name}` | Source · Exists | the legal states, in order | any `read:*` |
| `urn:iki:alias:scheme:{scheme}` | Source · Sink | who owns a scheme / register it (0.15.0, `alias::space()`) | `urn:cap:alias:scheme:{scheme}` to register |
| `urn:iki:alias:scheme:{scheme}:value:{value}` | Source · Sink · Delete | which item an alias names / give it / remove it | the scheme's owner, and `alias:{ledger}` or `write:{ledger}` to give |

The two `urn:iki:alias:` rows are a separate space, `ikigai_ledger::alias::space()`, which a host
binds beside this one; see "Aliases" below.

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
below. `count` is the exception: a number is not a graph, so it serves `text/plain` (the bare
number, which pipes) and `application/json`, and no Turtle. An `as=` this module cannot
answer in is **refused**, never substituted.

## Ledgers: a name, not a tag and not an argument

A label is *data*; a capability binds to a *resource name*. There is nothing for
`urn:cap:…` to attach to in "items labeled acme", so enforcement would have to live
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
be true — so `append`, `items`, `next`, `policy`, `ledgers`, `lifecycle` and eighteen others are not
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
Comments are minted the same way. The nonce comes from the operating system through `std`
natively and under WASI, and from `getrandom` over the JS `crypto` object in the browser
(`wasm32-unknown-unknown`), where `std` has no entropy source: before 0.6.1 every browser
load of the same module drew the same nonce, so two loads filing under one clock reading
minted the same IRI.

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

★ **An IRI that does not fit is keyed by its digest, and that is the contract** (ledger
#1315). The shape is the same for a comment key, so an arbitrary IRI minted elsewhere (a
taker's report IRI, say) may not be spellable as one: `/`, `?`, `#` and `%` are refused, and so
is anything past 256 characters. The answer is NOT a wider alphabet, for three reasons:
no alphabet removes the need, since an IRI has no length bound and a key must; `#` and
whitespace cannot cross the engine grammar as a `key=` value at all; and widening is additive
while narrowing would strand keys already filed, so the narrow set is the reversible choice. A
caller with such an IRI keys it as `{its own prefix}sha256:{64 lowercase hex of the IRI's UTF-8
bytes}`, which fits with up to 185 characters of prefix, and keeps the IRI itself in the comment
text. `parse_key` refuses with that advice, and a test pins that the digest
form is a legal comment key and that the IRIs it stands for are not (`tests/worker_grant.rs`).

Finding an item by its key:

| form | where |
| --- | --- |
| `urn:iki:ledger:{ledger}:item:key:{key}` | the item resource itself — Source, Exists, Sink, Delete |
| `item=key:{key}` | every write that names an item (`comment`, `close`, `link`, `claim`, …) and `purge` |
| `items key={key}` | a filter, composing with the others — `status=all` to include a closed item |

### Keyed comments: comment once, however often the request arrives

`comment key=<k>` is the same idea for a comment (since 0.7.0, ledger #1178): if a comment in
the ledger already carries the key, **nothing is written** and the answer names that comment,
marked `existing`. A hand-back relayed by a mirror, or a hook that reports and then re-runs,
is exactly the request that arrives twice:

```text
$ sink urn:iki:ledger:comment item=12 key=urn:collab:bobby:msg:01m4… <<< "Handed back: PR 41 merged"
commented on #12 (urn:iki:ledger:default:comment:01m4…)
$ sink urn:iki:ledger:comment item=12 key=urn:collab:bobby:msg:01m4… <<< "Handed back: PR 41 merged"
commented on #12 (urn:iki:ledger:default:comment:01m4…) existing
```

- **The check is a clause of the write**, as for the append: both halves of the comment's
  update carry `FILTER NOT EXISTS` over the key, so of concurrent keyed comments exactly one
  writes (`tests/worker_grant.rs`). A taken key is answered with no write at all, so a replay
  invalidates nothing.
- **The item in the answer is the one the existing comment is ON**, which a replay naming a
  different item would otherwise hide. In JSON: `"outcome": "existing"`, `key`, and `comment`
  as it was written (the replay's own text is not stored).
- **A comment key is a different name from an item key** (`ledger:commentKey`, not
  `ledger:key`): one string may name an item and a comment, and neither blocks the other.
- ⚠ **Unlike an item's key, a comment's leaves with its item.** Deleting an item moves its
  comments, keys and all, to the graveyard; a replay naming the deleted item is refused as not
  found, but one naming a different live item would write. Comments have no tombstones.

### Handing back: an item awaiting its dispatcher

A worker that finishes hands the item back with its evidence, and from then on the item waits
for whoever dispatched it to check the evidence and close it. `comment handback=true` (since
0.8.0, ledger #1199) makes that a STATE of the item rather than a convention in a comment's
text:

```text
$ sink urn:iki:ledger:comment item=12 handback=true <<< "https://github.com/x/y/pull/41 — CI green"
commented on #12 (urn:iki:ledger:default:comment:01m4…) awaiting-dispatcher
$ source urn:iki:ledger:next
…
  not #12: handed back, awaiting the dispatcher
$ sink urn:iki:ledger:comment item=12 rejects=true <<< "CI is red on main; see the run"
commented on #12 (urn:iki:ledger:default:comment:01m4…) cleared-awaiting
```

- **Under the comment grant, in the same update as the comment.** A worker cannot label (that
  needs `write:{ledger}`), so the hand-back rides on the one write it has: the comment and
  `ledger:awaiting <urn:iki:ledger:awaiting:dispatcher>` land together or not at all. A keyed
  hand-back replayed writes neither, so a replay cannot re-set a state the dispatcher has since
  cleared.
- **`next` does not offer it**: `why: "awaiting-dispatcher"` in JSON, `handed back, awaiting the
  dispatcher` in text. Before this, the worker released the item and `next` offered it to the
  next worker as if nothing had happened (`tests/awaiting.rs` is the reproduction).
- **The hand-back is the reason even while the worker still holds the claim** (0.12.0, ledger
  #1455). A worker that hands back without releasing was reported as `claimed by worker-1`
  through 0.11.1, which is true and is not what anyone should act on; the hand-back is what the
  dispatcher acts on, and it outlasts the release, so `next` gives it first and the reason does
  not change when the worker lets go.
- **What clears it**: close (closed work awaits nobody), reopen, a comment with `rejects=true`
  (the evidence did not hold; the item is ready again), and **any comment from a caller holding
  the close grant** (`close:{ledger}` or `write:{ledger}`, which root holds): the dispatcher
  speaking on a handed-back item is the answer to it. A worker's ordinary comment leaves it
  standing; a worker clears it only by saying `rejects=true`. `handback=true` and
  `rejects=true` together are refused.
- **Only on open work** (since 0.11.1, ledger #1418): `handback=true` on a closed item is
  refused with a `Conflict` naming the item and its state, and writes nothing. Until 0.11.1 it
  wrote `awaiting` on the closed item. The comment and its `awaiting` carry the same condition
  in the store update, so a hand-back racing a close writes neither. An ordinary comment on a
  closed item is still written.
- **On every face**: `awaiting: dispatcher` in the item's text (and `awaiting-dispatcher` at the
  end of its line in a listing), `"awaiting": "dispatcher"` in JSON (`null` otherwise), the
  triple in Turtle, and `"handback": "set"` or `"cleared"` on the comment's JSON answer. The
  doctor does not call a handed-back in-flight item `abandoned`, and reports a closed item
  still awaiting (`awaiting-closed`), which only a write around the ledger leaves.
- ⚠ **A claim is untouched by either**: the worker releases its claim as before. An item
  awaiting its dispatcher can still be claimed (the dispatcher may, to review it); `next` simply
  does not offer it.

## Aliases: an external id, in a scheme one principal owns

Since 0.15.0 (ledger #2828) an item may carry many **aliases**, each a `(scheme, value)` pair
unique per scheme across the deployment: `kata:01m6c5bjbm…` and `kata:web#3f2` from an import,
`roborev:job/88`, `gh:acme/web#12` from a GitHub sync, or an item's handle from a ledger it was
moved out of. Every reference that takes an item resolves one: `item=gh:acme/web#12`,
`urn:iki:ledger:acme:item:kata:01m6c5bjbm…`, and the alias's own IRI. An item PART
(`…:item:{id}:state`) does not, for the reason it refuses a key: a value may hold a `:`.

**An alias is not a key.** The idempotency key (`append key=`) is unchanged, an item's primary
name stays `ledger#N`, and a new item carries no alias.

```text
$ sink urn:iki:alias:scheme:gh                                 # as the sync's principal
scheme gh owned by urn:iki:gonk:client:gh-sync
$ sink urn:iki:alias:scheme:gh:value:acme%2Fweb%2312 acme#12
gh:acme/web#12 names acme#12 urn:iki:ledger:acme:item:01m4a2…
$ source urn:iki:ledger:acme:item:gh:acme%2Fweb%2312           # or item=gh:acme/web#12
```

Every scheme and alias is in one deployment-wide graph, `urn:iki:alias:graph`, skolemized as
`urn:iki:alias:scheme:{scheme}` and `urn:iki:alias:scheme:{scheme}:value:{value}`, with the value
percent-encoded outside `A-Z a-z 0-9 - . _ ~`. The resources are a space of their own,
`ikigai_ledger::alias::space()`, bound beside `space()`.

★ **It is a new authority boundary, and these are its rules:**

| to | you need |
| --- | --- |
| register a scheme | `urn:cap:alias:scheme:{scheme}` and a principal; the principal becomes the owner. The first registration wins, inside the write; another principal is refused naming the owner |
| give an item an alias | to own the scheme (data, checked inside the write) **and** `urn:cap:ledger:alias:{ledger}` or `write:{ledger}` on the item's ledger. A non-owner is refused naming the owner; an alias already naming another item is refused naming that item |
| remove an alias | to own the scheme |
| read the aliases graph | `urn:cap:store:read:graph:urn:iki:alias:graph` (and `…:write:graph:…` to write it) |
| resolve an alias to an item | the aliases graph's read grant **and** read on the item's ledger |

⚠ **Reading the aliases graph reveals that items exist**: an alias names an item IRI, which
carries its ledger and opaque id (never its title or body). Issue that grant to an importer, a
sync or an operator, and to nobody you would not show the list of ledgers. Resolution never
names an item to a caller without read on its ledger: an alias of such an item answers word for
word what an alias nobody holds answers, and an alias of an item in another ledger the caller
can read is refused the way a foreign item IRI is. A reference that is not an alias reads
nothing new and needs no new grant.

Uniqueness and ownership are decided in the update that adds the alias (the check-then-act
lesson of ledger #2772): racing adds of one alias leave one, and racing registrations of one
scheme leave one owner. `tests/aliases.rs` holds all of this, both races included. Not built: no
item face lists its aliases, a scheme has no transfer, and deleting an item leaves its aliases,
which then resolve to an item that is not found, the way a key stays taken.

## A goal and its arcs: `parent=` and an item's children

`link type=parent` says one item is part of another, and until 0.12.0 only the child said so:
its faces showed `parent:`, and the goal showed nothing, so a hub listing a goal's arcs needed
SPARQL (ledger #1454). Now both ends do:

```text
$ source urn:iki:ledger:items parent=12 status=all
#14  open    p-  Arc one
#15  closed  p-  Arc two
…
$ source urn:iki:ledger:count parent=12
1
$ source urn:iki:ledger:item:12
#12  open    p1  The goal
  …
  child:    urn:iki:ledger:default:item:01m7…
  child:    urn:iki:ledger:default:item:01m8…
```

- **`parent=` on `items` and `count`** takes what `item=` takes (`12`, `#12`, the item's IRI)
  and is subject to `status` like every filter, so `status=all` lists every arc and the default
  lists the open ones. **An unknown parent is not found**, never an empty answer: "no children"
  and "no such item" are different facts. An item of another ledger is refused by name.
- **Every face of an item names its children**, every status: `child:` lines in the plain face
  (only when there are some), `children` in JSON, and in Turtle the `<child> ledger:parent
  <item>` triples themselves, since the vocabulary has no inverse property and one fact should
  have one spelling. A listing's items carry them too: the item object is the same everywhere.
- `tests/children.rs`. On 0.11.1 `parent=` was not an argument, and an unknown argument is
  ignored, so `items parent=1` listed every item.

## Links across ledgers: an edge admitted by `link-in`, mirrored in the target

Since 0.17.0 (ledger #2829) `blocks`, `parent` and `related` may point at an item of ANOTHER
ledger. A ledger stays a boundary for content; only the edge crosses, and the target ledger learns
the subject's IRI and whether it is open, nothing else:

```text
$ sink urn:iki:ledger:cli:link item=4 content=acme#7 type=blocks      # or content=<acme#7's IRI>
linked cli#4 blocks acme#7
$ source urn:iki:ledger:acme:next
nothing is ready (1 open item(s) excluded: acme#7 blocked by urn:iki:ledger:cli:item:01m6… in the ledger cli)
$ source urn:iki:ledger:acme:item:7                                    # under acme's grants only
 acme#7  open    p-  Document the flag
  …
  blocked-by: urn:iki:ledger:cli:item:01m6… (open; in the ledger cli, not readable under your grants)
```

- **The edge** is the subject's: a `ledger:Link` node in the subject's graph (`onItem` the
  subject, `linkType`, `linkTarget`, `committer`, `committedAt`), archived with the subject. An
  edge within one ledger is the plain triple it always was.
- **`urn:cap:ledger:link-in:{target}`** admits it, beside `write` on the subject's ledger, and
  the store's read and write grants for the target's graph (the mirror's update reads and writes
  it). `write:{target}` does NOT stand in: a link in decides what the target's `next` offers, so
  whoever issues grants decides who may add one. It is checked before the object is looked up,
  so a caller without it learns nothing of the other ledger. Unlinking takes the same grants; without the target's store grants a `blocks` or `parent` link or unlink is refused before
  anything changes, naming them, because the edge and its mirror are two writes.
- **The mirror** is a `ledger:Mirror` in the target's graph (`onItem` the target, `linkType`,
  `mirrorOf` the subject's IRI, `subjectOpen`, `linkPresent`, `mirrorVersion`), written by the
  ledger after every link, unlink, close and reopen of the subject. `blocks` and `parent` are
  mirrored and `related` is not. The target's `next` reads its own graph only and excludes an item
  with an open blocker elsewhere (`blocked_elsewhere` in JSON); a store write cuts its cache like
  any other. The target's item faces show `blocked-by:` and `child:` lines (`mirrored` in JSON, on
  `item:{id}` and, since 0.18.0, on every `items` row),
  saying "not readable under your grants" when the reader holds no read grant on the subject's
  ledger; the subject's faces show the edge among its `blocks:`, `parent:` and `related:` lines,
  and its JSON and Turtle `items` rows carry it too (since 0.18.0).
- **Closing or reopening the subject writes every mirror**, so a closer needs the target's store
  grants too; without them the close is refused before anything changes, naming the grants. A
  subject that carries a `blocks` or `parent` link across is not deleted until it is unlinked. Deleting the target
  archives its mirrors with it.

★ **Two graphs cannot be one update, so the mirror is a compare-and-set on a version.**
`urn:iki:store:graph-update` changes one graph and its `WHERE` cannot read another, so the
subject's status and the target's mirror are always two writes. What is decided inside the
subject's update is the VERSION: every close and reopen adds a `ledger:StatusEvent` and every
link and unlink a `ledger:EditEvent` in the same update as the change, and the version is the
count of those events. After its update a writer reads the status, the version and the links in
one query, and writes each mirror only when the mirror holds no greater version. Whatever order
the mirror writes land in, the latest snapshot wins; an unlink leaves the mirror with
`linkPresent false` so an older write cannot bring the edge back. `tests/cross_ledger_links.rs`
forces all four races (a close's mirror landing after a whole reopen, and the reverse; a link's
after a close; a close's after an unlink).

⚠ **Not covered, and why**: a process that stops between the subject's update and the mirror's
leaves the mirror at its previous version until the subject's next close, reopen, link or unlink
(a close of an already closed item rewrites it, so that is the repair). The same holds for a
target linked while a close was in flight by a caller whose grants do not reach it; that close
says so in a `⚠ not mirrored:` line. A purge does not check for links across. And a `blocks`
cycle that runs through two ledgers is not refused: each ledger's `next` sees only its half, so
both items are simply excluded, each naming the other.

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
  "children":[{"number":14,"display":"#14","iri":"urn:iki:ledger:default:item:01m7…"}],
  "comments":[{"id":"urn:iki:ledger:default:comment:01m6…","author":"chris",
               "time":"2026-09-15T00:00:00.000Z","text":"looked at it"}],
  "defects":[],"state":"filed","awaiting":null}}
```

`state` is the item's lifecycle state: `filed` when it holds none, and `null` only when an
out-of-band write left it holding several (see "Lifecycle state"). `awaiting` is `dispatcher`
while the item is handed back (see "Handing back"), and a claim also carries `kind`, `expires`,
`lease` and, since 0.8.0, `principal`: the principal it was taken as, or `null`.

`defects` is empty for every item written through a ledger Sink. When an out-of-band write
left one of its timestamps unreadable or missing, it names that value
(`unreadable dcterms:modified "yesterday"`) and the other timestamp stands in for it in
`created`/`modified` — so both stay `xsd:dateTime` strings. See "Assume an editor got there
first".

| resource | document |
| --- | --- |
| `item:{id}` | `{schema, ledger, item}` |
| `items` | `{schema, ledger, count, total, offset, items: [item…], unreadable: [{iri, defects}]}` — `count` is the PAGE (`items`' length), `total` how many items the filter admits, `offset` how many the page skipped; `count < total` says the page is not the set. `unreadable` is the plain face's ⚠ footer, as data: the items NOT in `items`. An item listed with a stand-in timestamp carries its own `defects` instead |
| `count` | `{schema, ledger, total}` — `items`' `total` for the same filter, without the page and without the corpus check behind `unreadable` |
| `next` | `{schema, ledger, policy, weighs, generated_at, ready, ranking: [{rank, score, because, item}], excluded: [{why, reason, holder, expires, blocked_by, item}]}` — `why` is `claimed`, `lease-expired`, `awaiting-dispatcher`, `deferred` or `blocked` |
| `doctor` | `{schema, ledger, lifecycle, checked_at, checks, problems: [{check, item: {number, display, iri}, detail, remedy, holder, states, expires}]}` |
| `append`, `comment`, `close`, `link` | `{schema, ledger, outcome, item: {number, display, iri}, …}` — `outcome` is `filed`, `existing`, `commented`, `closed`, `linked` or `unlinked`, and only that outcome's own fields follow: `status` and `key` (append), `comment` (comment, and close with a note), `handback` (`set` or `cleared`, a comment that changed a hand-back), `reason` (close), `type` and `target` (link) |

`links` are the item's **outbound** edges — `blocks`, then `parent`, then `related` — and a
target that is not a live item in the ledger has a `null` number. `children` (0.12.0) is the
one **inbound** edge a face carries: the items whose `parent` is this one, every status, sorted
by IRI (see "A goal and its arcs"). The documents are Rust
types in `ikigai_ledger::json`, `Deserialize` as well as `Serialize` and `#[non_exhaustive]`,
so a Rust consumer reads the same type this crate writes and a later field does not break
it. `tests/faces.rs` pins every document as a literal, and pins the plain face byte for
byte as 0.3.0 rendered it — the changes there are a `key:` line in an item's detail, only for
an item that has a key, and (0.12.0) a `child:` line per child, only for an item that has
children.

★ **For the total alone, read `count`, not `items limit=1`** (ledger #1128). Both faces of
`items` that state a total also run the corpus check — two scans of the whole graph — because
they show what it finds (`unreadable`, and the plain face's ⚠ footer); that check is most of
what a one-row listing costs. `count` takes the same filter, parsed by the same function and
counted by the same query, so its number is `total` exactly, and it pays for the count alone.
Measured over a kernel that caches nothing, at 3000 open items (`tests/listing_cost.rs`, run by
hand, on a machine with other builds running, so read the ratios rather than the
milliseconds): `items limit=1 as=application/json` 480 ms debug / 55 ms release, of which the
check alone is 289 / 31 ms; `count as=application/json` 31 / 3.3 ms. Every read here is
cacheable, so these are the cold reads a consumer pays after each write.

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
and what the tombstone hashes and counts one set. And that removal commits only while
nothing beyond those quads matches the item (ledger #2780): a write that lands between the
read and the commit makes the commit match nothing, and the delete reads, archives and
commits again, so the write is archived with the item. Through 0.14.1 it was kept in the live
graph beside the tombstone instead, an orphan that no later purge could reach. A purge
still removes by selector — it is asked to leave nothing of the item — so a write racing a
purge goes with it, except a new comment or event, which makes the purge read again (below).

### What a delete and a purge cost: the item, not the ledger

Since 0.18.0 (ledger #2922) every read and update a delete or a purge sends names the item, or one
of its dependents (the comments, events and links that hang off it by `ledger:onItem`), in each
pattern. The store's scoped door copies only the quads an update's patterns can match, so it copies
the item and nothing else, and a read looks up those quads and no others. Through 0.17.0 the set was
written `?s ?p ?o . FILTER(…)`, a pattern with no constant in it, so the door copied the WHOLE graph
for a delete's commit and for both of a purge's removals, and refused past 250,000 quads
(`DurableStore::with_max_scoped_copy_quads`): a large ledger could delete and purge nothing, a pasted
secret included.

Measured in a release build on the in-memory store, one run on a shared machine, an item of about 66
quads (ten comments, a label, a link) in a ledger of 996,147 quads with 120,000 in its graveyard
(`cargo test --release --test delete_at_scale -- --ignored --nocapture measure`; the 0.17.0 columns
ran the same test on that release):

| | quads the store copies | peak memory | time |
| --- | --- | --- | --- |
| 0.17.0, default bound: delete | the whole graph, refused at 250,001 | +326 MiB | 0.69 s, refused |
| 0.17.0, default bound: purge | the whole graph, refused at 250,001 | +248 MiB | 1.06 s, refused |
| 0.17.0, bound raised to 2,000,000: delete | 996,147 | +1,940 MiB | 4.04 s |
| 0.17.0, bound raised: purge of the deleted item / of a live one | 120,000 / 996,147 + 120,000 | +148 / +1,107 MiB | 0.75 / 4.67 s |
| 0.18.0, default bound: delete | 67 | +1 MiB | 0.07 s |
| 0.18.0, default bound: purge of the deleted item / of a live one | 66 / 65 | under 1 MiB | 0.07 / 0.08 s |

The bound stays a safety net for an update that really does observe the whole graph (`CLEAR`,
`DROP`, a pattern with no constant): the scoped door still refuses one past it.

⚠ **What naming the dependents costs.** They are read first (one narrow read), then named, three of
the store's 1,024 algebra operators each. A dependent written after that read is still caught: its
own `onItem` edge points at the item, and every commit holds only while the item has no dependent it
did not name. A recoverable delete then reads and commits again, as for any racing write, and so
does a purge. Past 250 dependents, or with a dependent that is a blank node (which has no name to put
in a pattern, and which this crate never writes), a delete and a purge fall back to the 0.17.0 form,
which copies the whole graph and works only in a graph inside the bound. Nothing is batched: each
removal is still one update, so it is atomic, and the ~340-event limit below did not move.

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
   blocked by an open item, plus the caller's label / `about` / level / drain / state filters. No policy is
   involved, and most of the value is here. ⚠ **"Blocked", the cycle check and leverage
   are computed over EVERY open item, and the filters narrow afterwards** — a blocker
   without the filter's label still blocks, and a ledger of any size is read whole. Until
   0.2.1 both were computed over the filtered pool, loaded through a listing's 500-row
   bound, so a filter or a long backlog could make blocked work look ready.
2. **The order within it is policy**, and that is the part people disagree about.

Each policy is a **resource** — `urn:iki:ledger:policy:{name}` — so the manifold advertises
which orderings exist and `Meta` says what each one weighs. Two ship:

- **`priority-recency`** (the default) is **kata's own rule**, reimplemented so behavior can
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

### Asking one step for its work: `drain=` and `state=`

Since 0.18.0 (ledger #2865) `next drain=<name>` admits only the open items whose current state's
`ledger:drain` is `<name>`, and `next state=<state>` only those in that state. So a step of a loop
can ask what is waiting for it without labeling anything:

```text
$ source urn:iki:ledger:next drain=review-gate
 1.    #1  open    p1  Review the schema change
    p1 — priority 1; last updated 2026-09-15T00:00:43.000Z

policy: priority-recency (weighs priority, recency, number)
ready: 1   excluded: 1
  not #2: blocked by #3
$ source urn:iki:ledger:next drain=triag
invalid argument `drain`: `triag` is not a drain of the lifecycle `kata-flight` this ledger uses; one of triage, review-gate, ship (see `urn:iki:ledger:lifecycle:kata-flight`)
```

- ★ **They apply the way `labels=` does: after blocked items, cycles and leverage are computed over
  the whole ledger.** A filter chooses which items you are asking about and never changes what ranks
  them. A blocker in another state still blocks, a candidate is credited for what it unblocks in
  another state, and a cycle anywhere still refuses. The answer is the unfiltered ranking with the
  other items taken out (`tests/next_state_filter.rs`).
- **`filed` counts.** An item that holds no `ledger:state` is in `filed`, so `drain=triage` and
  `state=filed` admit it. A state in flight has no drain, so only `state=` asks about one.
- **A drain may be shared.** It admits every state that names it. Given both, an item must satisfy
  both, and a pair no state satisfies is the usual `nothing is ready` sentence.
- **Each value is judged against the lifecycle of the ledger being asked**, from the arguments
  alone, before anything is read. An unknown name or an empty value is refused with a sentence
  naming the lifecycle's drains or states, and a value that cannot be read is refused too, so neither
  is read as "no filter". The contract's `one_of` is the union across the lifecycles a host configured,
  as the state Sink's is, because one contract describes every ledger. So a value the contract accepts
  can still be refused by the ledger it was sent to.
- ⚠ **An item whose current state cannot be said is admitted by no filter**: two `ledger:state`
  values, a literal, or a value this lifecycle does not declare. The unfiltered `next` still offers
  it, it still blocks what it blocks, and the doctor reports it (`two-states`, `unknown-state`).

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
- **A move the lifecycle does not declare is refused** (`Conflict`, naming the lifecycle and every
  legal move from `from`), also before anything is read, when the lifecycle declares any move. See
  "A lifecycle may declare its moves".
- `content=` is an optional note, recorded as a comment when the item moves; `as=application/json`
  answers `{schema, ledger, item, lifecycle, state, in_flight, drain, since, outcome, from}`.
- `close` does not touch the state, and the state does not touch `close`: they are different
  facts, and a loop that wants an item closed AND out of the flight asks for both. A closed
  item refuses a transition into an in-flight state with a `Conflict` naming the item, close
  reason, and close time. The compare-and-set checks status atomically with state. Reopen
  the item before entering flight; a transition to a state outside flight is still allowed.

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
names or orders, a state outside the flight with no drain, a state in flight with one, a
lifecycle that does not declare `filed` first, a `ledger:movesTo` that is not a move between
two of its states, and a document that does not say its one version.

### A lifecycle may declare its moves

Since 0.18.0 (ledger #2863) a state may list where an item can go from it, with `ledger:movesTo`,
repeatable:

```turtle
<urn:iki:ledger:lifecycle:gated:filed> a ledger:LifecycleState ;
    rdfs:label "filed" ; ledger:order 0 ; ledger:inFlight false ; ledger:drain "triage" ;
    ledger:movesTo <urn:iki:ledger:lifecycle:gated:queued> .
```

When a lifecycle declares **any** move, the state resource refuses every `(from, to)` pair it does
not declare, and the plain face of the lifecycle ends each state's line with `moves to: …`
(`moves to: nowhere` for a state an item cannot leave):

```text
$ sink urn:iki:ledger:item:12:state from=filed to=done
conflict: The lifecycle `gated` declares no move from `filed` to `done`, so nothing was written. From `filed` it allows moves to `queued` (see `urn:iki:ledger:lifecycle:gated`)
```

- **A `Conflict`, not an `InvalidArgument`.** Each argument is a good state of this lifecycle, and
  it is the state machine that says no, as it does when a closed item asks to enter flight. A state
  the lifecycle does not have is still `InvalidArgument`.
- **Checked from the arguments alone**, before anything is read or written, so an undeclared pair
  gets this answer about an item that does not exist as well. That one check holds for the write
  that follows because the compare-and-set pins the item to `from`: if a racer moves the item after
  the check, the transition is the usual `Conflict` naming where the item is now, and no undeclared
  move happens. `tests/lifecycle_moves.rs` forces that schedule.
- **`from=X to=X` is not a move**: it needs no declaration, and is answered `unchanged` as ever. A
  state may not declare a move to itself.
- **A lifecycle that declares no moves behaves as it always did**: any state may move to any other,
  and its plain face does not change.
- **The built-in `kata-flight` declares none.** gonk's dispatcher and workers make no state move
  today: they claim, comment and close. What does move state is gonk's kata importer, from `filed`
  straight to whichever of the five states a kata label names, and this crate's doctor tells people
  to move an item out of flight from wherever it stands. A table that kept all of that legal would
  refuse almost nothing, and a narrower one would break the importer.
- `Lifecycle::parse` refuses, at boot and with a sentence, a move to a state the lifecycle does not
  declare (another lifecycle's included), a move that is not an IRI, a move to the state itself, and
  a move on anything that is not one of its states. That last one would otherwise vanish in silence,
  and, if it were the only move, would turn enforcement off.
- `close` and `reopen` do not move state, and an importer's history records what happened elsewhere
  as history only, so neither is checked against the moves.

### A state event names the lifecycle's version

Since 0.18.0 (ledger #2864) every transition's edit event records the version of the lifecycle
document the move was checked against, as `ledger:lifecycleVersion`, in the update that makes the
move. So when a document changes, an old event can still say which document allowed it. The version
comes from the document: its `owl:versionInfo`, or a `ledger:lifecycleVersion` on the lifecycle if
the document has none.

```turtle
<urn:iki:ledger:lifecycle:gated> a ledger:Lifecycle ;
    owl:versionInfo "2" ;
    ledger:hasState <urn:iki:ledger:lifecycle:gated:filed> , <urn:iki:ledger:lifecycle:gated:doing> .
```

⚠ **A lifecycle must say its version.** `Lifecycle::parse` refuses, at boot and with a sentence, a
document with no version, with more than one, with one that is not a literal or not one line with
something on it, and a `ledger:lifecycleVersion` on anything but the lifecycle. A host with its own
lifecycle adds one line when it upgrades. Refusing is what makes "every transition" true: an event
with no version can only be older than the term. The built-in `kata-flight` is version `1`.

The version is whatever text the document says. Nothing here compares versions or orders them, and
nothing notices a document that changed and kept its version, so change it with the document's states
or moves. A host loads one document per lifecycle at a time, so the version an event records is the
one the host was running.

**Renaming a state takes two versions**, and items move across by ordinary transitions:

1. Version 2 declares both names and a move from the old to the new. With `ledger:movesTo`, it can
   also stop anything else from entering the old name.
2. Move each item with `from=old to=new`. Each event records version 2.
3. Version 3 drops the old name. The events keep it, and the history still reads
   `state queue to backlog`, because the name comes from the IRI the event holds.

```text
$ source urn:iki:ledger:item:1:history
#1: 3 event(s)
2026-11-01T09:00:00.000Z  state filed to queue  lifecycle version 1  by urn:iki:gonk:client:aaaa
2026-11-02T09:00:00.000Z  state queue to backlog  lifecycle version 2  by urn:iki:gonk:client:aaaa
2026-11-03T09:00:00.000Z  state backlog to doing  lifecycle version 3  by urn:iki:gonk:client:aaaa
```

An item still in the old name when version 3 loads cannot be moved by the state resource, because
`queue` is no longer a state to name. The doctor reports it as `unknown-state`, and it takes one
SPARQL update over the ledger's graph, so check with the doctor before dropping a name.
`tests/lifecycle_version.rs` walks the whole sequence across restarts over one durable store.

- **An event from before 0.18.0 has none**, cannot be given one, and says so: the plain history ends
  its object with `no lifecycle version recorded (written before 0.18.0)`, and JSON has
  `"lifecycle_version": null`.
- **Only a state event this ledger wrote speaks of one.** On any other event, the field is absent
  from JSON and the plain line does not mention it.
- **An importer's history carries none.** Its events were not checked against this ledger's lifecycle,
  so a batch naming `ledger:lifecycleVersion` is refused, and an imported state event's line and JSON
  do not mention one.

## Claims: a lease, a kind, and a takeover

```text
$ sink urn:iki:ledger:claim item=12 lease=30m <<< "kata-ship/0abc1234"
#12 claimed by kata-ship/0abc1234 until 2026-10-09T15:30:00.000Z
$ source urn:iki:ledger:next
…
  not #12: lease expired: claimed by kata-ship/0abc1234 until 2026-10-09T15:30:00.000Z
$ sink urn:iki:ledger:claim item=12 takeover=true from=kata-ship/0abc1234 <<< "kata-ship/9fff0000"
#12 taken over from kata-ship/0abc1234 by kata-ship/9fff0000
```

- **Every take is one guarded store update** (since this release; until now a claim was
  read-then-write, and two claimants could both read the item free and both write). The update
  holds only while the item is free or already this holder's, and the holder is read back: of
  two claimants racing for one item, exactly one holds it and the other gets a `Conflict`
  naming them. Claiming again as the same holder renews the claim and its lease.
- **`lease=`** (`90s`, `30m`, `2h`, `1d`, or an `xsd:duration` like `PT30M`) records
  `ledger:leaseExpires` from the **kernel's** clock, so a test pins expiry without sleeping and
  an as-of corridor can ask who held what when. Omit it for a claim held until released.
- ★ **An expired lease is never silently free.** The claim stays. `next` excludes the item with
  `why: "lease-expired"`, the holder and the expiry, and a cached `next` (and doctor) holds
  only until the earliest running lease ends, because that moment changes the answer with
  nothing written. A plain claim on it is a `Conflict` that says how to take it, and the only
  way to take it is **`takeover=true from=<holder>`**: a compare-and-set against that holder,
  so of two takers exactly one wins. A live lease, or a claim with no lease, is never taken
  over; it is released (`delete …:claim`), which is the deliberate act.
- **The claim's kind**, `machine`, `agent` (since 0.11.0, ledger #1329) or `person`, is in the
  JSON face (`claim.kind`) and in the graph (`ledger:claimKind`). The doctor reports a
  machine's or an agent's claim on an item that is not in flight, naming which, and leaves a
  person's hold alone. An agent acting for a person is stamped `agent`: the hold is the
  agent's, and only a person's own hold escapes the doctor.
- ★ **A close releases the claim** (since 0.7.0, ledger #1147). Closed work is held by nobody;
  until 0.7.0 a close left the claim in place, so a closed item went on showing under
  `items holder=` and a machine's claim on it was the doctor's orphan. The release is in the
  same store update as the close, and it is recorded: a comment `Closed while claimed by
  <holder>; the claim is released.` (attributed to the closer's `author`), `released <holder>`
  at the end of the plain answer, and `"released": "<holder>"` in JSON. Reopening does not
  restore it; whoever picks the work up again claims it.
- ★ **And the close's update decides what it did, not the read before it** (since 0.14.1,
  ledger #2764). Only the update that finds the item open records the close (its
  `ledger:StatusEvent` and `closedAt`), and only one that finds a claim records the release,
  naming the claim it found. Concurrent closes of one claimed item record one close and one
  release; the others are verdict edits, answered `closed #12 (done); it was already closed,
  so only the verdict changed` and `"already_closed": true` in JSON. A claim that lands
  between a close's read and its update is released and recorded like any other.
- ★ **And so does the reopen's** (ledger #2766). Only the update that finds the item closed
  records the reopen (its `open` status event), clears the close fields and the hand-back,
  and sets it open. Reopening an open item changes nothing and says so: `#12 was already
  open, so the reopen changed nothing` (or `…, so the reopen only added the note` when it
  carried one). Concurrent reopens of one closed item record one reopen, and a close and a
  reopen at once leave one of the two orders.
- **And it can say what closed it** (since 0.18.0, ledger #2842): `evidence=<iri>`, repeatable, on
  that close's status event. See "Evidence on a close".
- ★ **And a closed item cannot be claimed** (since 0.11.1, ledger #1419). A claim, a renewal
  and a takeover on a closed item are each refused with a `Conflict` naming the item and its
  state; until 0.11.1 a claim took it, and the held-closed state the close had just removed
  came back. The store update carries the same condition, so a claim racing a close does not
  land either. Reopen the item first if there is more to do.
- ★ **The holder is the caller's PRINCIPAL when there is one** (since 0.8.0, ledger #1199). When
  the claimant's capability carries a principal (`urn:cap:principal:<iri>`, minted by the host's
  door: gonk's QUIC door names every client `urn:iki:gonk:client:<fingerprint>`, its HTTP door
  the signed-in passkey), the claim records it as `ledger:claimPrincipal` beside the holder's
  name, which stays the free-text label every answer prints. From then on the claim is that
  principal's:
  - **a release of it needs a capability that ACTS AS that principal**, with `from=` or
    without it, or the dispatcher's override (`close:{ledger}` or `write:{ledger}`, which root
    holds). Anyone else is `Denied`, so **one worker cannot release another's claim by spelling
    its name**, nor by leaving `from=` off. Until 0.8.0 a holder was only free text and the
    claim grant released anyone's claim (`tests/holder_principal.rs` is the reproduction);
  - **another principal claiming or renewing under the same name is a `Conflict`**: "the same
    holder" means the same name AND the same principal;
  - **a takeover of an expired lease is unchanged** (`from=` names the holder it takes from) and
    records the taker's principal.

  **A host without principals behaves exactly as 0.7.0 did**: no principal is recorded, a holder
  is free text, and the claim grant releases any claim. The same goes for a claim taken without
  one (root, or before 0.8.0): any claim-grant holder may release it, and a principal renewing
  it under its name adopts it. ⚠ The collaboration layer's mirror, which holds claim and comment
  and never close, releases the claims IT took (they carry its principal) and no longer a
  claim a worker took directly as itself; that one is the worker's, or the dispatcher's.
- ★ **`from=<holder>` makes a release a compare-and-set** (since 0.7.0, ledger #1178): it
  releases only while that holder still holds the claim, in one guarded update, and otherwise
  is a `Conflict` that releases nothing. A worker releasing "its" claim after a takeover no
  longer drops the new holder's. Since 0.8.0 `from=` may name the holder by its label or by the
  principal it was taken as. **A release without `from=` keeps working** for the claim's own
  principal (and for any caller on a claim taken without one); for someone else's claim it is
  the dispatcher's override. A worker should always send `from=`.
- ★ **`renew=true` renews and never takes** (since 0.7.0, ledger #1178). A plain claim by the
  same holder takes a FREE item, so a worker whose claim was released, or taken over and then
  released, silently re-acquired the item on its next heartbeat. With `renew=true` the claim
  must be held by that holder right now, or the answer is a `Conflict` saying it changed hands
  or was released; an expired lease still held is still held, and renews. A worker's
  heartbeat should always send it. It cannot be combined with `takeover=true`.

```text
$ sink urn:iki:ledger:claim item=12 renew=true lease=30m <<< "worker-1"
#12 renewed by worker-1 until 2026-10-09T16:00:00.000Z
$ delete urn:iki:ledger:claim item=12 from=worker-1
#12 released
```

### ★ The kind is the HOST's to stamp, and a direct embedder owns that

`claim` declares **no `kind` argument and ignores one if it is sent**: a kind the caller could
choose would let a machine pose as a person to escape the doctor. The kind comes from a
function the host gives the space, which sees the invocation (what the host's own door put on
the request, and the capability), never a caller's choice:

```rust
let stamper: ClaimKindStamper = Arc::new(|inv| match inv.inline_str("principal") {
    Ok(p) if is_a_person(p) => ClaimKind::Person,
    Ok(p) if is_an_agent(p) => ClaimKind::Agent,
    _ => ClaimKind::Machine,
});
let space = ikigai_ledger::space_with(SpaceConfig::default().claim_kind(stamper));
```

**The default stamps every claim `machine`**, the kind the doctor reports on, so a host that
never decides cannot let anything escape it; the cost is that a person's hold reads as an
orphaned machine claim until the host stamps properly. ⚠ **A stamper is only as good as what it
reads**: gonk's reads the `principal` its door stamps on every request, which works because the
door OVERWRITES that argument. A host whose door merely fills it in when absent has handed the
choice back to the caller. A claim taken before kinds existed carries no `ledger:claimKind`; the
JSON face says `null` and the doctor reads it as a machine's.

## Who wrote it: a committer and a verified or claimed author

Every item, comment and tombstone a write creates records who made it the way git does (ledger
#2827):

- **`ledger:committer`**: the one principal (`urn:cap:principal:<iri>`) the writer's
  capability carried, as the host's door minted it, and **`ledger:committedAt`**, the kernel
  clock. Neither is an argument, so a caller cannot supply either. A capability with no
  principal (root, a host whose door mints none) records the time and no committer.
- **`ledger:author`**: optional. With no `author=` the committer is the author, and that
  author is **verified**. An `author=` is accepted only when it names the committer, a
  principal the committer acts for (`prov:actedOnBehalfOf` in the host's participants graph),
  or, when the committer is an import or sync principal (typed `ledger:Importer` there), a
  foreign actor. The last two are **claimed**. `ledger:authorSource` says which:
  `committer`, `delegate` or `import`. Anything else is refused with the reason and the remedy.
- **Authored dates**: only an import principal may send `created` and `modified` when filing,
  and `created` on a comment, as `xsd:dateTime`s no later than the import. They go into
  `dcterms:created` and `dcterms:modified`, so ranking tie-breaks imported items as their
  source did, marked `ledger:createdSource` / `ledger:modifiedSource` `"import"` beside the
  true `ledger:committedAt`. Any other caller that sends a date is refused, and told why.
- **Which writes move `modified`, and how** (since 0.18.0, ledger #2928): a write that carries a
  claimed date (a comment's `created=`, a close's `closed-at=`) sets the item's `modified` to the
  LATER of its current value and that date, marked `ledger:modifiedSource "import"`, so an
  importer that files with the source's `modified` and then replays the item's past leaves it at
  the source's value. Every other write, an importer's undated one included, stamps the kernel's
  time, and a `modified` at or after the commit reads as not claimed. An imported history
  (`…:history`) touches no item at all.

The host names the participants graph with `SpaceConfig::participants(graph)`. It is read
through the store's narrow door with the caller's own capability, so a delegate or an importer
needs that graph's read grant; writing as yourself reads nothing. Without one, the only author
a principal may name is itself, and nobody may claim a date. Who may claim whom is enrollment
data in that graph, not a capability per pair.

```text
  filed:    2020-01-02T03:04:05.000Z (claimed) by urn:example:github:user:octocat
  updated:  2020-02-03T04:05:06.000Z (claimed)
  author:   urn:example:github:user:octocat (claimed by an importer)
  committer: urn:iki:gonk:client:importer at 2026-11-01T18:30:00.000Z
```

The JSON face carries the same as `attribution` on an item and on a comment (`author_status`
`verified` or `claimed`, `author_source`, `committer`, `committed`, `claimed_dates`). ⚠ A
write without a principal still records `author=` as the free text it always was; every face
reads that as claimed (`claimed, free text`). `tests/attribution.rs` has one test per path.

## Who worked on it: claim, release, hand-back and edit events

A claim is current state: a release or a close retracts `claimedBy` and the rest, so until
0.15.0 the graph forgot who had worked on an item the moment it closed, and a status event
said when an item closed but never who closed it. Now each of these writes records its "who"
as a node of its own (ledger #2819), so "what did this agent do" is a query over the events:

| event | written by | carries |
|---|---|---|
| `ledger:ClaimEvent` `{item}:claim:{id}` | a first claim, or a takeover (never a renewal) | `claimedBy` (the holder label), `claimPrincipal`, `claimKind`, and `takeoverFrom` on a takeover |
| `ledger:ReleaseEvent` `{item}:release:{id}` | a release (by the holder or a dispatcher), a close, a takeover, when there was a claim | the released claim's `claimedBy`, `claimPrincipal`, `claimKind` and `claimedAt`, and `releaseReason` `released`, `closed` or `takeover` |
| `ledger:EditEvent` `{item}:edit:{id}` | `label`, `link`, `defer`, `item:{id}:about`, an item edit, a `state` transition | `editKind`: `label`, `unlabel`, `link`, `unlink`, `defer`, `resume`, `about`, `unabout`, `edit` or `state`, and what it changed (below) |
| `ledger:StatusEvent` (since 0.14.0) | a close or a reopen | now also its committer |
| `ledger:handedBackBy` on the hand-back comment | `comment handback=true` | the holder label of the claim the hand-back found |

Every event carries `onItem`, `dcterms:created`, `ledger:committedAt` and, when the capability
carried a principal, `ledger:committer`: the releaser of a release, the closer of a close. The
model is the one "Who wrote it" describes, with no second one beside it.

- ★ **Each event rides the update that makes its change, under the same guard.** No new check
  then act: a claim that loses its race writes no claim event, so concurrent claims of one free
  item leave one, and a release with `from=` that finds the claim changed hands records nothing.
- **The holder label is what tells two workers apart that share a principal** (both lawnboy
  workers share one QUIC principal). The claim event keeps it after the release, and a
  hand-back records it in `ledger:handedBackBy`.
- **Events hang off their item** by `onItem` and are skolemized beneath it, so a delete, an
  archive and a purge own them as they own comments and status events. No property they carry
  has an `rdfs:domain`, so a reasoner does not take an event for an item.
- **A renewal records nothing**: the claim it extends already has its event.

```sparql
# How many items has each holder claimed, in one ledger, whatever is still claimed now
PREFIX ledger: <https://ikigai-rs.dev/ns/ledger#>
SELECT ?holder (COUNT(DISTINCT ?item) AS ?items) WHERE {
  GRAPH <urn:iki:ledger:graph:default> {
    ?claim a ledger:ClaimEvent ; ledger:claimedBy ?holder ; ledger:onItem ?item .
  }
} GROUP BY ?holder
```

### What an edit changed, and an item's history

Since 0.18.0 (ledger #2841) an edit event names its object, because the state and the labels are
replaced in place and nothing else remembers them:

| `editKind` | carries |
|---|---|
| `label`, `unlabel` | `editLabel`: the label |
| `state` | `stateFrom`, `stateTo`: the states before and after, as IRIs (`filed` included, though an item in it holds no state), and `lifecycleVersion`: the version of the lifecycle it was checked against (see "A state event names the lifecycle's version") |
| `link`, `unlink` | `editLinkType`: the link predicate, and `editTarget`: the other item's IRI, possibly in another ledger |
| `about`, `unabout` | `editTarget`: each target IRI |
| `edit` | `editField`: the name of each field changed (`title`, `body`, `priority`, `revision`, `about`), never its value |

- ★ **Decided inside the update, never read and then written.** A transition's `stateFrom` is what
  its compare-and-set's `WHERE` bound, so the events of an item's transitions form one unbroken
  chain whatever races (`tests/edit_event_objects.rs` forces two).
- **An object means something changed.** Each is guarded by the change it records, so removing a
  label the item does not have, or sending a title it already has, still writes its event (every
  write has a who) and names no object. A query over objects counts only real changes.
- **An event written before 0.18.0 has no object** and cannot be given one: nothing kept it.
- **`editTarget` is history, not an edge.** A delete takes the edges pointing at its item, but an
  event on another item keeps naming the deleted item's IRI, as a tombstone does. An item's own
  events go with it.

`…:item:{id}:history` shows them all under `read:{ledger}`, in plain text and JSON, oldest first:
each claim, release, close and reopen as well as each edit. It names the other item of a link
across ledgers by its IRI, which the link already shows this ledger's readers, and looks up
nothing else of it; the target's history has no event for a link it only mirrors.

```text
$ source urn:iki:ledger:item:12:history
#12: 3 event(s)
2026-11-01T09:00:00.000Z  label bug  by urn:iki:gonk:client:aaaa
2026-11-01T09:05:00.000Z  state filed to queued  lifecycle version 1  by urn:iki:gonk:client:aaaa
2026-11-01T09:06:00.000Z  edit title, body  by urn:iki:gonk:client:aaaa
```

```sparql
# When did item N leave state S
PREFIX ledger: <https://ikigai-rs.dev/ns/ledger#>
PREFIX dcterms: <http://purl.org/dc/terms/>
SELECT ?when ?to WHERE {
  GRAPH <urn:iki:ledger:graph:default> {
    ?item ledger:number 12 .
    ?event ledger:onItem ?item ; dcterms:created ?when ;
           ledger:stateFrom <urn:iki:ledger:lifecycle:kata-flight:queued> ; ledger:stateTo ?to .
  }
} ORDER BY ?when
```

```sparql
# When was label L removed
PREFIX ledger: <https://ikigai-rs.dev/ns/ledger#>
PREFIX dcterms: <http://purl.org/dc/terms/>
SELECT ?when ?who WHERE {
  GRAPH <urn:iki:ledger:graph:default> {
    ?item ledger:number 12 .
    ?event ledger:onItem ?item ; dcterms:created ?when ;
           ledger:editKind "unlabel" ; ledger:editLabel "wip" .
    OPTIONAL { ?event ledger:committer ?who }
  }
} ORDER BY ?when
```

### An importer's close date

An import principal (`ledger:Importer`, as in "Who wrote it") may send `closed-at=` on `close`:
an `xsd:dateTime` no later than now and no earlier than the item's filing. It dates `closedAt`
and the `StatusEvent` that `counts` reads, so the daily series places the close on its source
date, and both are marked `ledger:closedAtSource "import"`; the event's `committedAt` keeps the
true time of the write. Anyone else sending it is refused with the reason and the remedy:

```text
`closed-at=2026-01-05T10:00:00Z` refused: only an import or sync principal may claim an
authored date, and urn:iki:gonk:passkey:dispatcher is not an import or sync principal
(ledger:Importer in <urn:collab:participants>). Send it without `closed-at`, and the ledger
stamps the time itself
```

### An importer's history

Since 0.18.0 (ledger #2843) an import principal can bring an item's history across, not only its
dates. `…:{ledger}:history` takes one item's events as Turtle: `item=` names the item, and `content`
holds up to 128 events and 512 KiB. A batch is recorded whole or not at all.

```turtle
@prefix ledger: <https://ikigai-rs.dev/ns/ledger#> .
@prefix dcterms: <http://purl.org/dc/terms/> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

<urn:kata:event:01JZ> a ledger:EditEvent ;
    dcterms:created "2020-02-03T00:00:00Z"^^xsd:dateTime ;
    ledger:author <urn:example:github:user:octocat> ;
    ledger:editKind "label" ; ledger:editLabel "bug" .
```

```text
$ sink urn:iki:ledger:acme:history item=1 < events.ttl
acme#1: imported events recorded 3, already recorded 1; history only, so the item itself did not change
$ source urn:iki:ledger:acme:item:1:history
acme#1: 4 event(s)
2020-02-03T00:00:00.000Z (claimed)  imported label bug  by urn:example:github:user:octocat (claimed by an importer)  committer urn:iki:gonk:client:importer
2020-02-04T00:00:00.000Z (claimed)  imported state filed to queued  by urn:iki:gonk:client:importer
```

- ★ **History only.** No write here names the item as a subject, so its state, labels, claim, status
  and `modified` stay as they are, and so do its faces, `next` and the daily counts. An imported
  `link` writes no edge and no mirror. To change the item, write the change itself.
- ★ **It moves no version.** A mirror's version counts status and edit events (#2829), and that
  count leaves out imported ones. So an import writes no mirror, and the next real close or link
  lands on the version it would have without the import. An imported status event carries no
  `eventSequence` and does not count toward the next one, so a real close keeps its number, its
  evidence goes on the real close, and `counts` never reads the imported one. A query that wants
  only what happened in this ledger filters on `ledger:createdSource`; a query over `eventSequence`
  sees real status events alone.
- **Marked the way 0.15.0's claimed dates are**: `ledger:createdSource "import"` beside the true
  `ledger:committedAt` and the `ledger:committer` the door stamped. `ledger:author` (optional, an
  IRI) says who did it at the source, marked `ledger:authorSource "import"`. In JSON an event says
  `imported`, `source_id`, `author` and `committed`.
- **The date rules are `closed-at=`'s**: no later than now and no earlier than the item's filing,
  for `dcterms:created` and a release's `ledger:claimedAt`.
- ★ **A re-send changes nothing.** An event's subject is its id at the source, kept as
  `ledger:eventKey` under the rules of a comment's key (ASCII letters, digits, `-`, `.`, `_`, `~`,
  `:`, at most 256 characters) and unique in the ledger, whichever item it is sent for and whatever
  else it says. It is checked inside the update, so concurrent re-sends record it once, and a batch
  that is all re-sends is answered without a write. A delete frees its events' ids with them.
- **Closed shapes, checked before the write.** Each class and each `editKind` names every term it
  takes, and any other term is refused by name, so a batch cannot set `eventSequence`, `committer`
  or `onItem`. Each `editKind` needs the object an 0.18.0 edit would name (`defer` and `resume` have
  none), and `editField` names a field, never its value. A value is stored as given and never
  fetched: an `editTarget` or `evidence` need not resolve. A string on more than one line is refused,
  since a history prints one line per event. Events of one moment read in the batch's order.
- **Bounded before it is parsed**: bytes first, then the store's nesting scan. A blank node is
  refused, since it could not be re-sent as the same event. A batch whose update would pass the
  store's 1 MiB bound is refused by name.
- **Anyone else is refused, and told what to do**, before the batch is parsed:

```text
only an import or sync principal may record past-dated history, and
urn:iki:gonk:passkey:dispatcher is not one (ledger:Importer in <urn:collab:participants>). Write
the change itself instead (claim, release, label, link, state, close, reopen), and the ledger
records its event as it happens
```

⚠ **At scale.** Measured in a release build on the in-memory store, one run on a shared machine
(`cargo test --release --test imported_history -- --ignored --nocapture measure`):

| 30,000 events, batches of 100 | on 300 items | on one item |
| --- | --- | --- |
| import | 4.9 s (about 6,100 events a second) | 4.7 s |
| triples in the graph after (about 9 an event) | 273,602 | 270,014 |
| `next` (one item, no events: 6 to 11 ms) | 23 ms | 6 ms |
| item read, plain / JSON | 10 ms / under 1 ms | 56 ms / 15 ms |
| `items` / `counts` / `doctor` | 75 / 3 / 23 ms | 26 / 1 / 17 ms |
| a re-sent batch of ten | 15 ms | 6 ms |
| `close` / `reopen` of an item | 177 / 374 ms | 496 / 613 ms |

At ledger #2922's profile, in one ledger: 3,690 items, 10,940 comments and 33,000 past events,
the share of one ledger of 4,200 items, 12,461 comments and about 37,600 events in all (`cargo test
--release --test imported_history -- --ignored --nocapture profile`):

| 33,000 events on 3,690 items, with 10,940 comments | |
| --- | --- |
| import: items and comments one write each, events one batch of 8 or 9 an item | 3.9 s / 22.7 s / 9.6 s (about 3,400 events a second) |
| triples in the graph after | 447,332 |
| `next`, first / again (cached) | 100 ms / under 1 ms |
| item read / `items` | 4 ms / 98 ms |
| `counts` | 5.0 s |
| `close` / `reopen` / another `close` | 160 / 286 / 147 ms |
| `delete`, then `purge` of the deleted item, at the default bound | 6 ms, 3 ms |
| `purge` of a live item, at the default bound | 5 ms |

Some limits of the store's are easier to reach with this much history. None loses data, and each
is refused with a sentence naming its bound:

- **A graph past 250,000 quads deletes and purges as any other since 0.18.0** (ledger #2922): their
  updates copy the item, not the graph, and the 447,332-triple ledger above deleted and purged at the
  default bound. Through 0.17.0 it refused every `delete` and `purge`. What remains is an item with
  more than 250 comments and events in such a graph, which falls back to copying the whole graph and
  is refused there (see "What a delete and a purge cost").
- **An item with more than about 340 events cannot be deleted.** Its delete inlines its quads into
  one update, about 3 KB an event, and the store takes 1 MiB: 300 events deleted, 350 were refused.
  The item stays whole and live, and that refused delete leaves a copy in the graveyard. Comments
  cost a delete the same way. A purge inlines nothing, and took an item of 5,000 events in a graph
  inside the copy bound.
- **An item with somewhere between 5,000 and 7,500 events cannot show its history.** The answer
  passes the store's 16 MiB bound for an ordinary caller. A query with `LIMIT` still reads them. At
  30,000 events on one item, a delete and a purge refuse on the same bound.

### Evidence on a close

Since 0.18.0 (ledger #2842) a close says what closed it. `evidence=<iri>` on `close`, repeatable,
is recorded as `ledger:evidence` on that close's `ledger:StatusEvent`, in the same update as the
close, beside the close's `ledger:closedReason`, so a reopen, which takes the reason off the item,
leaves both on the event.

```text
$ sink urn:iki:ledger:cli:close item=12 evidence=https://github.com/o/r/commit/abc evidence=https://github.com/o/r/pull/9
closed cli#12 done (2 evidence)
$ sink urn:iki:ledger:cli:close item=13 reason=duplicate evidence=cli#12
closed cli#13 duplicate of cli#12
$ source urn:iki:ledger:cli:item:13:history
cli#13: 1 event(s)
2026-11-01T09:00:00.000Z  closed duplicate of urn:iki:ledger:cli:item:01M…  by urn:iki:gonk:client:aaaa
```

**The role is read, never stored:** from where it hangs and the target's kind. A commit on a `done`
close is the fix, an item on a `duplicate` or `superseded` close is its target, a test is the
proof. A `duplicate` or `superseded` close needs no target (a note can still say why), and a target
no longer has to be written into the note, `about` and `related` as well.

- ★ **An item must exist, and the closer must be able to read it, or the close is refused** and
  nothing is written. That is decided for every reason, not only `duplicate` and `superseded`. It is
  named as a link's object is: `#12`, `12`, `key:{key}`, its IRI, an alias's IRI, or `acme#12`, and
  stored as the item's IRI. An item is not evidence of its own close. In another ledger it is
  #2829's rule: `urn:cap:ledger:link-in:{ledger}` and that graph's read grant, with the grant
  checked before the lookup, so a caller without it learns nothing and `write` there does not stand
  in. There is no mirror: nothing in that ledger reads a close here, and nothing is written there.
- ★ **Any other IRI is stored as given and never fetched**, under no grant beyond the close. That
  includes the short alias spelling `gh:acme/web#12`, which cannot be told from a foreign IRI
  without reading the aliases graph. An alias's own IRI resolves.
- ★ **A close of an item already closed loses nothing.** It records no second close (0.14.1), so
  its evidence joins the close that stands, and that event's reason follows the verdict, decided in
  the update: closes that race record one event holding all their evidence, and each loser says
  `closed #1 done (1 evidence); it was already closed, so the verdict changed and this evidence
  joined that close`. After a reopen, the earlier close is no longer the one that stands. A close
  recorded with no status event (written before they existed, or by a raw store write) has nothing
  to hold evidence, so that is refused, with the remedy: reopen it and close it again.
- **A reopen keeps each close's evidence on that close's event**, and a second close starts its own.
- **Evidence is history, not an edge.** Deleting the item it names leaves the close and the IRI,
  including when that delete lands while the close is in flight, as `editTarget` does (a delete and
  a close in two ledgers cannot be one update). Deleting the closed item takes its events with it.
- **An import principal sends `closed-at=` and `evidence=` together**: the dated event holds both.
- **A close written before 0.18.0 holds no reason and no evidence** on its event, and its history
  line says only `closed`.

`…:item:{id}:history` shows each close's verdict and evidence, plain and JSON (`reason`, `evidence`),
by IRI and looking up nothing. The close's JSON answer adds `evidence` (the IRIs stored) and `targets`
(the items a `duplicate` or `superseded` close names). A close with no evidence answers as it always
did: `closed #12 (done)`.

```sparql
# Fixes that did not hold: closes whose evidence commit was followed by a reopen
PREFIX ledger: <https://ikigai-rs.dev/ns/ledger#>
PREFIX dcterms: <http://purl.org/dc/terms/>
SELECT DISTINCT ?item ?commit ?closed ?reopened WHERE {
  GRAPH <urn:iki:ledger:graph:default> {
    ?close a ledger:StatusEvent ; ledger:onItem ?item ; ledger:status ledger:closed ;
           ledger:closedReason ledger:done ; ledger:evidence ?commit ;
           ledger:eventSequence ?closing ; dcterms:created ?closed .
    FILTER(CONTAINS(STR(?commit), "/commit/"))
    ?reopen a ledger:StatusEvent ; ledger:onItem ?item ; ledger:status ledger:open ;
            ledger:eventSequence ?reopening ; dcterms:created ?reopened .
    FILTER(?reopening > ?closing)
    # The first reopen after that close, not every one since.
    FILTER NOT EXISTS {
      ?earlier a ledger:StatusEvent ; ledger:onItem ?item ; ledger:status ledger:open ;
               ledger:eventSequence ?between .
      FILTER(?between > ?closing && ?between < ?reopening)
    }
  }
} ORDER BY ?reopened
```

`eventSequence` and not the dates, because an importer's close carries its source date. What counts
as a commit is the query's to say; this one reads GitHub's `/commit/` from the IRI.

### Explicit one-time backfill of `handedBackBy` (never at startup)

A hand-back written before 0.15.0 names no holder in the graph, but gonk's worker loop wrote it
as ``gonk worker `X` handed this back``. `ikigai_ledger::attribution::handed_back_by_backfill`
returns two SPARQL texts over one ledger's graph: a `SELECT` counting what it would recover, and
the `INSERT` that copies `X` into `ledger:handedBackBy`. Run the first through
`urn:iki:store:select` and the second through `urn:iki:store:update`, once, by hand. A comment
that does not name its worker that way is left alone, and a second run recovers nothing.

`tests/attribution_events.rs` has one test per event and per path.

## The doctor: everything wrong, at once, and nothing repaired

```text
$ source urn:iki:ledger:doctor
doctor: default (lifecycle kata-flight): 2 problem(s)

  orphaned       #3 is held by kata-ship/0abc (a machine claim) in `filed`, which is not in flight
                 remedy: release it (`delete urn:iki:ledger:claim item=3`), or move it into flight …

  lease-expired  #7's lease for kata-ship/9fff expired at 2026-10-09T15:30:00.000Z
                 remedy: take it over (`sink urn:iki:ledger:claim item=7 takeover=true from=kata-ship/9fff`), …
```

Seven checks, each a **stored SPARQL query** (`src/doctor/*.rq`) over the ledger's graph, with the
ledger's lifecycle filled in as terms:

| check | what it finds |
| --- | --- |
| `orphaned` | a machine's or an agent's claim (or one with no recorded kind) on an item whose state is not in flight; the detail names the kind |
| `abandoned` | an item in an in-flight state that nobody holds and that is not awaiting its dispatcher (a hand-back is waiting, not abandoned) |
| `lease-expired` | a claim whose lease ran out, judged on the kernel's clock |
| `unknown-state` | a `ledger:state` the ledger's lifecycle does not declare (a hand edit, an import, a ledger whose lifecycle changed) |
| `two-states` | an item holding more than one state, which only a raw store write can produce |
| `awaiting-closed` | a closed item still marked `ledger:awaiting` (since 0.8.0): a close clears a hand-back, so only a raw store write leaves one |
| `awaiting-stale` | an open hand-back waiting strictly longer than `awaiting-age` (default `24h`, since 0.13.1); names its wait and asks the dispatcher to check the evidence, then close or reject |

`source urn:iki:ledger:doctor awaiting-age=3d` changes the wait threshold. It accepts
positive durations (`30m`, `24h`, `3d`, `PT24H`) up to 366 days, like `lease=`. The age
comes from `ledger:awaitingAt`, written with the hand-back; ordinary worker comments,
claim releases and other item edits do not reset it. A new hand-back does. A keyed replay
does not. The cache expires when a wait crosses the threshold even with no intervening write.

For older awaiting markers without a timestamp, the newest comment's time is a conservative
lower bound. For an unmarked item, a newest comment beginning `Evidence:` (the old worker
convention) is a possible hand-back. These findings explicitly say **inferred**: historical
comments record text and an optional author, not the caller's dispatcher grants, so the
ledger cannot establish the author's role or recover an unrecorded hand-back flag. A later
comment suppresses this unmarked fallback. New dispatcher comments, rejections, closes and
reopens record an `awaitingAt` decision even when clearing the state, preventing earlier
legacy evidence from resurfacing. The doctor reports candidates for human review and never
sets an awaiting marker itself.

It is a read under the ledger's read grant, it reports every problem in one answer with a
remedy for each, and it **never repairs**: what to do about an orphan is a judgment, and a
detector that acts on its own findings is a reaper with a confidence problem. `as=application/json`
is the machine face. A `closed` item is checked like any other: a closed item still held, or
still in flight, is exactly the kind of leftover the doctor exists to show.

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

**Since 0.7.0 three verbs also have grants of their own** (ledger #1178, #1143), so that a
worker can be handed the work without the verdict on it:

| grant | reaches | and nothing else |
| --- | --- | --- |
| `urn:cap:ledger:claim:{ledger}` | `claim` Sink (take, renew, take over) and Delete (release) | |
| `urn:cap:ledger:comment:{ledger}` | `comment`, keyed or not | |
| `urn:cap:ledger:close:{ledger}` | `close` and `reopen` | |
| `urn:cap:ledger:about:{ledger}` (0.10.0) | `item:{id}:about` Sink (add) and Delete (remove) | not the item Sink's own `about=`, which is the edit door |
| `urn:cap:ledger:alias:{ledger}` (0.15.0) | giving one of this ledger's items an alias, in a scheme the caller owns | see "Aliases" |
| `urn:cap:ledger:link-in:{ledger}` (0.17.0) | linking an item of ANOTHER ledger to one of this ledger's items, and unlinking it | **not** covered by `write:{ledger}`; see "Links across ledgers" |
| `urn:cap:ledger:write:{ledger}` | **all four of the above**, and filing, linking, labeling, deferring, editing, `about=` on the item, state | unchanged: no grant issued before 0.7.0 or 0.10.0 changed meaning |

A claim's or a close's note, and the release record a close writes, are part of that verb and
need no comment grant.

**Since 0.10.0 `about` has one too** (ledger #1315): a process that records where work went (a
layer mirror noting the offer it published, a hook noting the session that delivered) can add
and remove an item's `about` edges without the close grant that `write:{ledger}` carries. ⚠
**The worker grant below does not include it**: an `about` edge is what `items about=` finds
items by, so whoever issues grants decides who may add one, and adds the line
`urn:cap:ledger:about:acme` for that caller. `tests/about_grant.rs` holds it to its row: it adds
and removes, it is refused at close, comment, claim, label, edit, append and at another ledger,
and `write:{ledger}` still does both.

★ **The worker grant**, as an operator's config line (here for ledger `acme`): claim, renew,
release, comment and read, and **no close, no relabel, no edit, no filing**:

```text
urn:cap:ledger:read:acme
urn:cap:ledger:claim:acme
urn:cap:ledger:comment:acme
urn:cap:store:read:graph:urn:iki:ledger:graph:acme
urn:cap:store:write:graph:urn:iki:ledger:graph:acme
```

The dispatcher who checks the evidence and closes adds `urn:cap:ledger:close:acme` (or holds
`write:acme`). The worker hands an item back with `comment handback=true` under its comment
grant, and the close grant is also what makes the dispatcher's own comment answer a hand-back
(see "Handing back"). When the host's door mints a principal for each worker, a worker's claim
is its own: another worker holding the same lines cannot release it (see "Claims"). `tests/worker_grant.rs` runs exactly the worker lines above: claim, renew,
comment and release succeed; close, reopen, label, edit, append, defer and delete are each a
typed `Denied` naming the grant that would do.

★ **Routing a filed item to another worker is a label edit** (ledger #1234). A worker reads
`next labels=worker:<name>`, so the dispatcher moves an item by removing one label and adding
the other, under `write:{ledger}`:

```text
$ delete urn:iki:ledger:acme:label item=12 worker:lawnboy
unlabeled acme#12 worker:lawnboy
$ sink urn:iki:ledger:acme:label item=12 worker:bug
labeled acme#12 worker:bug
```

Remove first, so the item is never offered to two workers. Each write stamps `modified`, and
`next`, `count`, `items` and the item see it on the very next read, cached or not
(`tests/label_edit.rs`). The worker grant can do neither half, so a worker cannot route work to
itself. This is two writes, which is safe with one dispatcher; two dispatchers racing would be
the double-label defect of "Lifecycle state" above, and the answer then is a single-valued
compare-and-set like `state`, not a label "replace".

⚠ **What `claim`, `comment`, `close` and `reopen` DECLARE is `urn:cap:ledger:*`**, "holds some
ledger grant", not their own family. They must admit both their own grant and `write:{ledger}`,
and `ikigai-core` ANDs every `requires` and matches a family only as a trailing-`*` prefix, so
the one family both satisfy is the common prefix. The exact either-or is enforced in the
endpoint, and a refusal names both tokens. The cost is an over-offer: a caller holding only a
read or delete grant (and a store graph write grant) sees those four actions in its manifold and
is refused on invoking them. Declared stays a superset of enforced, never the reverse. A core
`requires` that takes one of several scopes would let each declare exactly its own two.

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
| read (`items`, `count`, `counts`, `item`, `next`, `ledgers`) | `urn:cap:ledger:read:L` | `urn:cap:store:read:graph:urn:iki:ledger:graph:L` |
| write (`append`, `backfill-closed-at`, `comment`, `close`, `reopen`, `claim`, `defer`, `link`, `label`, editing an item) | `urn:cap:ledger:write:L` | the read grant above **and** `urn:cap:store:write:graph:urn:iki:ledger:graph:L` |
| claim, comment, or close and reopen, alone | `urn:cap:ledger:claim:L`, `…:comment:L` or `…:close:L` | the same two as write |
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

⚠ **A family HELD is not a grant.** A capability that carries the literal
`urn:cap:ledger:read:*` or `urn:cap:store:read:graph:*` (a script host's marker, say) names no
ledger and no graph, so every read here refuses it: the kernel's floor when the store family is
missing, the exact per-ledger check when it is not. A caller needs the exact tokens of the read
row above, and those satisfy every read, `next` included. `tests/read_grants.rs` pins both
(ledger #1220, where a root-run script on gonk carried only the ledger marker and read the
refusal as `next` over-declaring).

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

⚠ **And the per-verb grants are a boundary only where the store's door is closed to the
caller.** A worker holds `urn:cap:store:write:graph:urn:iki:ledger:graph:L` because every ledger
write goes to the store under the CALLER's capability; in a kernel where it can also resolve
`urn:iki:store:graph-update` directly, that token is raw quad authority, and one SPARQL update
closes an item without the close grant.
`tests/worker_grant.rs::the_store_token_a_worker_holds_is_still_raw_authority_without_a_door_rule`
pins it on a bare kernel. This crate cannot close it (a sub-request has no elevating form), and
a host's door can: gonk's refuses a raw `graph-update` against a ledger graph unless the
capability carries an explicit raw grant for it (ledger #878), so behind gonk's doors the worker
grant is what it says.

Two consequences worth stating plainly:

- **This is a tenancy boundary now**, where before it was a set of doors on one side of an
  open room. An agent's grants really are the set of ledgers it can touch — through this
  module and through the store underneath it.
- **It is not a boundary against the host itself.** Whoever configures the kernel chooses
  what every caller holds, and a host that binds the store's broad doors on a served
  transport has made a different decision about a different resource. That is the store's
  surface to reason about, not this one's.

### SPARQL never leaves the process: `SERVICE` and `LOAD` are refused

⚠ **The table above is also a grant to evaluate SPARQL**, and that was a network hole until
`ikigai-store` 0.2.10 (ledger #1085, #1083). A caller holding exactly this table can resolve
`urn:iki:store:graph-select`, `graph-ask` and `graph-update` on its own ledger's graph
directly, not only through this module. In any host where `oxigraph/http-client` is unified
on (`ikigai-cli` has it, through rudof), oxigraph installs an HTTP handler on every evaluator
that does not refuse one, so a `SERVICE <http://…>` sent there, or a `LOAD <url> INTO GRAPH`,
was an outbound request with no `urn:cap:net:*` anywhere near it. Reproduced against 0.2.9:
three `POST`s from the read doors and a `POST` plus two `GET`s from the update door, to a
local stub, under these grants and nothing else.

This crate itself never evaluates SPARQL (no `SparqlEvaluator`, no oxigraph dependency) and
never puts caller text into a query except as a term the store builds, so no ledger input can
spell a `SERVICE`. The fix is the store's, and this crate's part is its **floor, 0.2.10**, so
a host composing this ledger cannot resolve a store that still fetches.
`tests/service_egress.rs` holds both halves under the published grants: the store doors
refuse `SERVICE` and `LOAD` as a typed `InvalidArgument` and nothing reaches the stub, and
ledger content that spells them is stored and read back as text. Run it with
`--features http-client` (CI does) to build the crate the way those hosts are built; its
control, cfg'd on that feature, shows raw oxigraph really fetching in that build.

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
  disagree. ⚠ The exception is attribution (since 0.15.0, ledger #2819): who claimed,
  released, handed back and edited an item is a domain fact a work query asks for ("what did
  this agent do"), so those events are in the ledger's graph. They record who and when, never
  a before and after. An import principal can record such events after the fact (ledger #2843),
  marked as imported.
- **No reaper.** A lease that runs out is reported (by `next` and the doctor) and taken over
  by an explicit act; nothing releases or reassigns a claim on its own, because "owned" has to
  mean something a reader can check.
- **No key on an existing item.** A key is given at filing and never changed, so an item
  filed before keys existed cannot gain one through a resource here — that is one SPARQL
  UPDATE over the graph, and a migration's job rather than an endpoint's.
- **No comment editing.** Comments are append-only by design.
- **No `deferred-until` date.** Readiness that turns on the wall clock would go stale in the
  cache with no golden thread to cut it, so resuming is an act rather than the passage of
  time.

## Accessibility

The ledger's default face is plain text people read in a terminal: item lines, `next`, write
answers, the doctor and every refusal. [ACCESSIBILITY.md](ACCESSIBILITY.md) says what that text
is, the rules it follows, what is not tested, and how to report a barrier.

## Status

All twenty-six resources are bound, tested, and walked clean by `ikigai-conformance`
(`AUTHORITY` included — every mutating action declares the scope it enforces). So are the two
alias resources of `alias::space()` (`tests/alias_conformance.rs`).

**A host must bind this crate's space for the resources to resolve.** It composes with
`ikigai-store`'s space — store first, ledger second, behind a `Fallback` — and the store's
`DurableStore::open` is what names the dataset on disk. See "Composition" above.

### 0.18.0: an edit event names what it changed (ledger #2841)

**What an edit changed (#2841).** A `ledger:EditEvent` now records its object in the update that
makes the change: the label, the states before and after, the link type and the other item, the
about target, the names of the fields and never their values. A write that changed nothing keeps
its event and names no object. New: the resource `…:item:{id}:history` (plain and JSON, under
`read:{ledger}`), `json::HistoryDocument`, and the terms `stateFrom`, `stateTo`, `editLabel`,
`editTarget`, `editLinkType` and `editField`. A delete leaves `editTarget` on other items' events.
See "What an edit changed, and an item's history" above. Redeploy `/ns/ledger` after this release:
it gains the six terms.

**Evidence on a close (#2842).** `close` takes `evidence=<iri>`, repeatable, recorded as
`ledger:evidence` on that close's status event, with `ledger:closedReason` beside it, in the same
update. A reopen keeps both there. An item as evidence must resolve or the close is refused (across
ledgers, `link-in`); any other IRI is stored as given and never fetched. A close with evidence
answers `closed #1 done (2 evidence)` or `closed #2 duplicate of #1`; JSON adds `evidence` and
`targets`. A close's history line now names its verdict (`closed done`), where #2841 said `closed`;
`json::HistoryEvent` gains `evidence`, and its `reason` holds a close's verdict too. See "Evidence on
a close". `ledger:closedReason` drops its `rdfs:domain`, and the vocabulary gains `ledger:evidence`:
redeploy `/ns/ledger` for those as well.

**`next drain=` and `next state=` (#2865).** `next` narrows to the open items whose current state
drains to a step, or is a state, after blocked items, cycles and leverage are computed over the
whole ledger, so the filtered ranking is the unfiltered one with the other items taken out. An
unknown name is refused with a sentence naming the lifecycle's drains or states. New:
`Lifecycle::drains` and `Lifecycles::drain_names`. See "Asking one step for its work". Without either
argument `next` answers as it did.

**An importer's history (#2843).** `…:{ledger}:history` takes one item's past claim, release, edit
and status events as Turtle from a `ledger:Importer`, marked `ledger:createdSource "import"`, under
the date rules of `closed-at=`, and a re-sent source id changes nothing. History only: the item, its
mirrors' versions, the status sequence and the daily counts do not move, and each history face says
imported. Anyone else is told to write the change itself. New: `ledger:eventKey` (redeploy
`/ns/ledger`), `json::HistoryEvent`'s `imported`, `source_id`, `author` and `committed`,
`json::Answer`'s `recorded` and `already_recorded`. ⚠ `history` is now a reserved ledger name, so a
ledger already called that can no longer be addressed. See "An importer's history", with its
measurements and the store limits it reaches.

**A lifecycle's legal moves (#2863).** A `ledger:LifecycleState` may list `ledger:movesTo`. When a
lifecycle declares any, the state Sink refuses a pair it does not declare with a `Conflict` naming
the lifecycle and every legal move from `from`, before anything is read or written, and the plain
lifecycle face names each state's moves. `Lifecycle::parse` refuses a move that is not between two of
its states. A lifecycle with none behaves as it did, and built-in `kata-flight` declares none. New:
`LifecycleState::moves_to`, `Lifecycle::declares_moves`, `Lifecycle::allows_move` and the term
`ledger:movesTo` (redeploy `/ns/ledger`). See "A lifecycle may declare its moves".

**A state event names the lifecycle's version (#2864).** Every transition's `ledger:EditEvent` records
`ledger:lifecycleVersion`, read from the document's `owl:versionInfo` or, if it has none, a
`ledger:lifecycleVersion` on the lifecycle, in the update that makes the move. The history shows it:
`state filed to queued  lifecycle version 1`, and an older event says it has none (`null` in JSON).
⚠ `Lifecycle::parse` now refuses a document with no version, so a host's own lifecycle needs one line
added on upgrade; built-in `kata-flight` is version `1`. New: `Lifecycle::version`,
`json::HistoryEvent`'s `lifecycle_version` and the term `ledger:lifecycleVersion` (redeploy
`/ns/ledger`). See "A state event names the lifecycle's version".

**Delete and purge in proportion to the item (#2922).** A delete and a purge name the item, or one of
its comments, events and links, in every pattern they send, so the store's scoped door copies the
item and nothing else: on a 996,147-quad ledger a delete takes 0.07 s and about 1 MiB at the default
bound, where 0.17.0 refused past 250,000 quads and, with the bound raised, took 4.0 s and 1.9 GiB.
The removal is not batched, and each update stays atomic. An item with more than 250 dependents, or a
blank-node one, falls back to the old form, inside the bound only. See "What a delete and a purge
cost".

**A listing row is the item its own read shows (#2927).** The JSON and Turtle `items` faces now
carry an item's links into other ledgers and, in JSON, its `mirrored` edges, as `item:{id}` does;
until 0.18.0 a row held only the links within its ledger and no `mirrored`. They are read for the
whole page in two queries (links out, mirrors in), never one per item, with the same
`not readable under your grants` rule. At #2922's profile (3,690 items, 447,332 triples) a JSON
page of 50 took 0.017 s, against 0.019 s before, which is noise. ⚠ `next`'s items do not carry them
yet. The plain face is unchanged.

**An importer's past keeps the source's `modified` (#2928).** A comment with `created=` or a close
with `closed-at=` from an import principal moves the item's `modified` only forward, to the later of
its current value and the claimed date, instead of stamping the kernel's time; undated writes, and
every write by anyone else, stamp the time as before. On 0.17.0 an importer that filed with the
source's `modified` and then replayed comments and a close ended every such item at the import time,
so `priority-recency` ranked imported open items as just updated. See "Which writes move
`modified`" under attribution.

### 0.17.0: typed links across ledgers (ledger #2829)

**Typed links across ledgers (#2829).** `blocks`, `parent` and `related` may point at an item of
another ledger, admitted by the new `urn:cap:ledger:link-in:{ledger}` on the target's ledger
beside `write` on the subject's. The edge is a `ledger:Link` in the subject's graph, and the ledger
keeps a `ledger:Mirror` in the target's (the subject's IRI and open bit only), versioned so that
no race leaves it stale. The target's `next` excludes an item with an open blocker elsewhere. New:
`CAP_LINK_IN`, `select::Excluded::BlockedAcross`, `model::Mirrored`, the JSON fields
`blocked_elsewhere` and `mirrored`, and the terms `Link`, `Mirror`, `linkType`, `linkTarget`,
`mirrorOf`, `subjectOpen`, `linkPresent` and `mirrorVersion`. See "Links across ledgers" above. Redeploy `/ns/ledger` after this release: it gains the eight terms.

### 0.16.0 (published 2026-11-01): aliases by scheme (ledger #2828)

An item can be named by an external id: a `(scheme, value)` alias such as `kata:<uid>` or
`gh:<owner>/<repo>#<n>`, unique per scheme across the deployment, resolved by every item reference
(`item=`, the item IRI, and the alias's own IRI `urn:iki:alias:scheme:{scheme}:value:{value}`). A
scheme is registered by one principal, its owner, and only the owner adds or removes its aliases; the
owner check and uniqueness are decided inside the write. Resolution never names an item to a caller who
cannot read its ledger. New: `alias::space()` (bind it beside `space()`), the grants
`urn:cap:alias:scheme:{scheme}` and `urn:cap:ledger:alias:{ledger}` (which `write:{ledger}` covers), and
the vocabulary terms `AliasScheme`, `Alias`, `schemeName`, `schemeOwner`, `aliasScheme`, `aliasValue`,
`aliasOf`. See "Aliases" above. Not a replacement for `key`.

### 0.15.0 (published 2026-11-01): who wrote it, and who worked on it (ledger #2827, #2819)

**A committer the door stamps, and a verified or claimed author (#2827).** Every item, comment
and tombstone records `ledger:committer` and `ledger:committedAt`; an author is verified (the
committer) or claimed (an enrolled delegate, or an import principal's foreign actor); an import
principal may claim authored dates. A principal can no longer send a free-text `author=`, and
every face tells verified from claimed. See "Who wrote it" above.

**Attribution events (#2819).** A first claim and a takeover write a `ledger:ClaimEvent`, every
release (by the holder, a dispatcher, a close or a takeover) a `ledger:ReleaseEvent`, every edit
that creates no node a `ledger:EditEvent`, and a hand-back names its holder
(`ledger:handedBackBy`); each event, and every status event, carries its committer. An import
principal may date a close (`closed-at=`). An explicit one-time backfill recovers
`handedBackBy` from legacy hand-backs. See "Who worked on it" above.

**Vocabulary.** New: `ClaimEvent`, `ReleaseEvent`, `EditEvent`, `takeoverFrom`,
`releaseReason`, `handedBackBy`, `editKind`. `claimedBy`, `claimedAt`, `claimKind` and
`claimPrincipal` lose `rdfs:domain ledger:Item`, because events carry them too. `closedAtSource`
may say `import`. The vocabulary now carries an `owl:Ontology` header whose `owl:versionInfo`
is the crate version (test-pinned), which devtools' `ns-drift` compares with the deployed
`/ns/ledger`: redeploy it after this release.

**Aliases by scheme (#2828).** An item may carry many `(scheme, value)` aliases, unique per
scheme across the deployment, and every item reference resolves one. A scheme is registered
with one owning principal, and only it adds aliases in it. New: `ikigai_ledger::alias` and its
space (`urn:iki:alias:scheme:{scheme}`, `…:value:{value}`), the `urn:cap:ledger:alias:{ledger}`
and `urn:cap:alias:scheme:{scheme}` grants, the deployment graph `urn:iki:alias:graph`, and the
terms `AliasScheme`, `Alias`, `schemeName`, `schemeOwner`, `aliasScheme`, `aliasValue` and
`aliasOf`. See "Aliases" above.

**An item with many quads deletes again.** A delete's commit guard listed the quads it read one
`VALUES` row per line, and the store's scanner refused that past about twenty triples beyond the
item's own (`MAX_SCAN_BRANCHES`: in the less-than reading of `<`, each row's `#` opened a comment
that the next line closed). Any item with that many labels, comments or links could not be
deleted, and the events above would have put most items that were ever claimed there. The rows
are now one line; an item with hundreds of quads, its literals full of `#`, `<`, quotes and newlines, deletes
(`tests/attribution_events.rs`, `an_item_with_hundreds_of_quads_still_deletes`).

### 0.14.2: one reopen per close, and a delete or purge leaves nothing behind (ledger #2766, #2779, #2780)

Three races found by the check-then-act audit (ledger #2772), each reproduced before its fix.

**One reopen per close (#2766).** 
`reopen` required its item and then wrote unconditionally. Reopening an item that was
already open appended an `open` status event for a close that never happened and reset its
hand-back decision time; concurrent reopens of one closed item each appended an `open`
event; and a reopen landing inside a close's read and update put an `open` event ahead of
the close. Every write of a reopen now carries "the item is closed" in its own `WHERE`, and
the reopen reads back the event it minted to learn whether it was the one. A reopen of an
open item answers `#12 was already open, so the reopen changed nothing`.
`tests/close_race.rs` forces each schedule.

**A purge inside a recoverable delete's window (#2779).** A purge that landed between a delete's read
and its archive left the item's full content in the graveyard under a tombstone that said purged.
Whichever of the two finishes second now cleans up: the purge clears the graveyard again after its
mark, and the delete removes its own archive when it finds the mark. `tests/purge_race.rs`.

**A comment racing a recoverable delete (#2780).** A comment that landed between a delete's read and
its commit stayed in the live graph, where no later purge reached it. A delete now removes the
item's rows only when nothing beyond them matches, and otherwise reads, archives and commits again (at
most 16 attempts, then a `Conflict`). Comments orphaned by earlier versions are not swept up.
`tests/comment_delete_race.rs`.

### 0.14.1: one close per item, and closed items cannot enter flight (ledger #2764, #2765)

A close decides inside its own update: only the update that finds the item open records the
close (its status event and `closedAt`), and only one that finds a claim records the release,
so concurrent closes of one claimed item record one close and one release; the others are
verdict edits and say so (`"already_closed": true` in JSON).

The state Sink now requires open status in its compare-and-set guard when the destination
is in flight. A closed item, including one closed between the read and write, receives a
`Conflict` naming its close reason and time. Transitions to states outside flight still
succeed on closed items. Regression tests pin the refusal and force the close/write race.

### 0.14.0 (published 2026-10-29): close timestamps and daily count series (ledger #2541)

Minor release: additive vocabulary terms, dated status events, `counts` CSV/Turtle
resources, and the explicit `backfill-closed-at` migration command. Reopen removes the
item's current close timestamp while preserving history in events. New bare resource
names `counts` and `backfill-closed-at` are reserved ledger names. No publication is part
of this change. The ledger #2602 follow-up adds `content=all` to migrate every
undated closed item from `updated` in one command, with a count and an idempotent rerun.

Fixed in the same release (ledger #2764): `close` decided from its read of the item, and its
update re-checked only that the item was still live. Concurrent closes of one claimed item
each wrote the release comment, and each appended a `closed` status event and set
`closedAt`; a claim that committed between the read and the update was retracted with no
record. The open-to-closed transition and the release record now carry their condition in
the update's own `WHERE`, and the close reads back what it wrote. A close of an already
closed item still edits the verdict without moving the close time, and now says so: the
plain answer ends `; it was already closed, so only the verdict changed`, and the JSON
answer carries `"already_closed": true`. `tests/close_race.rs` forces both schedules.

### 0.13.1 (2026-10-27): overdue dispatcher hand-backs (ledger #2395)

- Doctor adds `awaiting-stale`, with `awaiting-age=24h` by default and a remedy to check the
  evidence, then close or reject. Hand-backs now stamp `ledger:awaitingAt` atomically; cache
  validity ends when the threshold is crossed.
- Old awaiting markers and unmarked newest `Evidence:` comments have a conservative,
  explicitly inferred fallback. Dispatcher answers, rejections, close and reopen record the
  decision time so older evidence does not reappear after an answer.

### 0.13.0 (2026-10-26): core 0.2.0 streams release (ledger #2261, #2347)

A breaking minor: public API types now come from `ikigai-core` 0.2.0, the streams
release. The old 0.1 requirements excluded 0.2.0 under Cargo's 0.x compatibility rules.
Both store dependencies move to 0.3.0, and the vocabulary to 0.2.0, keeping one core
across normal and development dependencies. Conformance 0.11.0 enables DATATYPES and
FAILURES by default; the suite now requires every check to run instead of expecting
FAILURES to be skipped. No endpoint changes or datatype waivers were needed.

### 0.12.0 (2026-10-13): `labels=` at any length, a goal's children, the hand-back reason (ledger #1452, #1454, #1455)

A minor, because it adds public API: `parent=` on `items` and `count`, the public fields
`Filter::parent` and `Item::children`, and `children` in every item object of the JSON face.
All additive: the JSON schema stays 1, a field is `#[serde(default)]`, and the plain face only
gains `child:` lines on an item that has children. Each item was reproduced first as a test
that failed on 0.11.1.

- ★ **`labels=` takes any number of labels** (ledger #1452). Each label was a triple pattern in
  the main join, and the store refuses a join wider than 32 operands, so `items` and `count`
  refused at 28 labels and `next` at 30, with an error about SPARQL patterns that never named
  `labels=`. The first label is still a pattern, and the rest are one `VALUES` list inside a
  `FILTER NOT EXISTS`, which adds nothing to the join it sits in: `labels=` widens the main
  join by one at any length, measured by the store's own walk. `tests/labels_at_scale.rs` runs
  1, 28 and 100 labels on all three.
- **`parent=` and an item's children** (ledger #1454). See "A goal and its arcs". On 0.11.1
  `parent=` was not an argument and was ignored, so `items parent=1` listed every item.
- **`next` names a hand-back before a claim** (ledger #1455): a handed-back item whose worker
  still holds the claim is `awaiting the dispatcher`, not `claimed by`. See "Handing back".
- ⚠ Not in this release: the dev-dependency `ikigai-conformance = "0.6.0"` is a ceiling below
  0.7.x (ledger #1452's note), left for its own change.

### 0.11.1 (2026-10-13): a deep `blocks` chain, a large ledger, closed items (ledger #1418, #1419, #1420, #1422, #1423)

A patch: five defects an unled audit of 0.11.0 reproduced, each pinned by a test that failed on
0.11.0 because of it, and a new `ikigai-store` floor. No public API changes.

- ★ **`ikigai-store` 0.2.21 is the floor**, a SECURITY floor like 0.2.10 before it, not an API
  one (ledger #1426, #1427, #1428, #1429). A ledger caller holds this ledger's store graph
  grants and can resolve the store's doors directly, and before 0.2.21 a `GRAPH` block (every
  query here is one) and three other wrappers the planner erases hid their patterns from the
  store's join bound, so a query inside every bound could plan uncancellably for minutes. 0.2.21
  also reserves the overdue-evaluation slot at start and bounds `urn:iki:store:load`'s document
  size and XML entity expansion. A caret on 0.2.10 admitted 0.2.21 but did not require it, so a
  consumer's lock could keep an older store under this release. This crate's own queries are
  counted the same under 0.2.21 as under 0.2.19 (each is one `GRAPH` block, whose inside was
  already a join of its own), so nothing here that answered before is refused now.

- ★ **A long `blocks` chain no longer aborts the host** (ledger #1422). The cycle check
  recursed once per link, so a chain about 15,000 long overflowed a 2 MiB stack (a tokio
  worker's) and aborted the whole process on every `next`. It walks with an explicit stack
  now, in the same order, so it names the same cycle. Leverage is computed once for the whole
  graph rather than one walk per item, which was quadratic on a chain (20 s for 10,000 linked
  items against 0.25 s unlinked); the counts, and so `next`'s ranking, are unchanged.
- **`next` answers past about 19,400 open items** (ledger #1423). It listed every open item's
  IRI in one `VALUES` list, which passed the store's 1 MiB SPARQL bound, and refused on every
  call. The lists are split under the bound now (also the JSON face's link numbers and
  comments, and a listing's page); the store's bound is unchanged. Loading labels and links
  is also linear now: two `VALUES` lists in one query were joined in time quadratic in the
  items once they carried links or labels.
- **A hand-back on a closed item is refused** (ledger #1418), `Conflict`, naming the item and
  its state, and writes nothing. It used to write `awaiting` on closed work. See "Handing
  back".
- **A closed item cannot be claimed** (ledger #1419): claim, renew and takeover are refused,
  and the store update requires an open item, so a claim racing a close does not land
  either. See "Claims".
- **`items limit=<the largest count> offset=1`** no longer panics in a debug build or prints
  a wrapped footer in a release one (ledger #1420).

### 0.11.0 (2026-10-12): an agent is its own claim kind (ledger #1329)

A minor, because a public enum gains a variant: `ClaimKind::Agent`, so a consumer's exhaustive
`match` on `ClaimKind` stops compiling. `ClaimKind` is `#[non_exhaustive]` from now on, so the
next kind is not a flag day. Additive otherwise: `ClaimKind::ALL`, `ClaimKind::from_word`, and
`ledger:agent` in this crate's own vocabulary.

- **`agent`** is stamped by a host for an LLM agent, such as a collaborator's Claude in gonk's
  layer mirror, which until now could only be stamped `machine`. An agent acting for a person
  is still `agent`. JSON `claim.kind` says `"agent"`; the graph says `ledger:claimKind
  ledger:agent`. See "Claims".
- **The doctor reports an agent's claim out of flight** as `orphaned`, exactly as it does a
  machine's, and the detail names the kind: `(an agent claim)`. Only a person's own hold
  escapes it, so the new kind lets nothing escape.
- **Fixed on the way:** the Turtle face matched the kind word against a list of its own, so a
  kind it did not list was silently dropped from the graph face. It now parses through
  `ClaimKind`.

### 0.10.0 (2026-10-11): an `about` grant without close; digest-keyed IRIs are the key contract (ledger #1315)

A minor, because `item:{id}:about` declares a different floor (`urn:cap:ledger:*`, as `claim`,
`comment`, `close` and `reopen` do since 0.7.0), and that over-offers it to read and delete
holders; enforcement is exact. Additive otherwise: a grant, `CAP_ABOUT`, `Ledger::cap_about`.

- **`urn:cap:ledger:about:{ledger}`** adds and removes an item's `about` edges at
  `…:item:{id}:about`, and nothing else, so a process that records where work went (gonk's
  layer mirror noting its offer) needs no close grant. `write:{ledger}` still covers it; the
  worker grant does not include it. The item Sink's own `about=` stays under write.
  `tests/about_grant.rs`.
- **Keys: the digest fallback is the contract**, not a wider alphabet. An IRI that is not a
  legal key is keyed as `{prefix}sha256:{hex}` with the IRI in the text; `parse_key`'s refusal
  says so. See "Keys: file once, under your own name".
- Since 0.9.0, tests and documentation only: the read row alone reads every read, `next`
  included (ledger #1220, `tests/read_grants.rs`); routing a filed item is a label edit
  (ledger #1234, `tests/label_edit.rs`).

### 0.9.0 (2026-10-10): `count`, the total without the corpus check (ledger #1128)

A minor, because it adds a resource and reserves a ledger name (`count`; `ledger::RESERVED` is
now 21 words). Additive otherwise: every existing face is byte for byte what 0.8.0 served.
⚠ Neither 0.7.0 nor 0.8.0 was published to crates.io (the latest there is 0.6.2), so 0.9.0
carries both of them as well.

- **`urn:iki:ledger:{ledger}:count`** (read grants): how many items a filter admits, and
  nothing else. The same filter arguments as `items`, parsed by the same function and counted
  by the same query, so the number is `items`' `total` exactly; plain face the bare number,
  JSON face `{schema, ledger, total}` (`json::CountDocument`), no Turtle. Reading the total
  through `items limit=1` ran the corpus check behind `unreadable`, two scans of the whole
  graph and ~60% of the read; `count` costs the COUNT alone, ~16x less at 3000 items. See
  "The JSON face".
- **`tests/listing_cost.rs`**, an ignored harness that times each listing face and its parts
  over a kernel that caches nothing.

### 0.8.0 (2026-10-10): a hand-back is a state, and the holder is the caller's principal (ledger #1199)

A minor, because behavior changes: `next` no longer offers an item awaiting its dispatcher, a
dispatcher's comment clears a hand-back, and a claim taken as a principal is no longer released
by any claim-grant holder. Additive otherwise: two arguments on `comment`, two terms, three JSON
fields, a doctor check. `ikigai-core` 0.1.93 is the floor (`Capability::principal`).

- **`comment handback=true`** (comment grant) marks the item `ledger:awaiting
  <urn:iki:ledger:awaiting:dispatcher>` in the same update as the comment; `next` excludes it
  (`why: "awaiting-dispatcher"`). Close, reopen, `rejects=true`, or any comment from a caller
  holding the close grant clears it. On every face; the doctor no longer calls a handed-back
  in-flight item `abandoned`, and reports `awaiting-closed`. See "Handing back".
- **Holder = principal**: a claim under a capability carrying a principal records
  `ledger:claimPrincipal`; releasing it needs a capability acting as that principal, or the
  dispatcher's override (`close:{ledger}` or `write:{ledger}`), with or without `from=`. Another
  principal under the same holder name cannot claim or renew it. A host without principals is
  unchanged. See "Claims".
- **American spelling** in output: the label answer says `labeled`/`unlabeled`.

### 0.7.0 (2026-10-10): per-verb grants for claim, comment and close; keyed comments; a close releases the claim (ledger #1178, #1143, #1147)

A minor, because behavior changes: a close now releases the claim it finds (recorded as a
comment), and `claim`, `comment`, `close` and `reopen` declare a different floor. Additive
otherwise: three grants, a `key` on `comment`, a `released` field on the close answer, a term.

- **`urn:cap:ledger:{claim,comment,close}:{ledger}`**, so a worker can claim, renew, release and
  comment without being able to close, relabel, edit or file; `write:{ledger}` still covers all
  three, so no grant changes meaning. The worker grant is published line by line in "What is
  enforced, and where". ⚠ Those four actions now DECLARE `urn:cap:ledger:*` (core has no
  one-of `requires`), which over-offers them to read and delete holders; enforcement is exact.
- **`comment key=<k>`**: a replay writes nothing and answers `existing`, in one guarded update
  (`ledger:commentKey`, a name apart from an item's `ledger:key`).
- **A close releases the claim** in the same update; `released <holder>` in the plain answer,
  `released` in JSON, and a comment naming who held it. A release needs only the claim grant,
  whoever holds the claim (a holder is free text).
- **Claim compare-and-sets**: `delete …:claim from=<holder>` releases only while that holder
  still holds the claim, and `claim renew=true` extends only a claim the holder holds now, so a
  worker that lost its claim cannot drop the new holder's or silently re-take the item. Both
  opt-in; a release without `from=` still drops whoever holds the claim.

### 0.6.2 (2026-10-10): `ikigai-store` 0.2.10 is the floor, so a ledger's grants are no network egress (ledger #1085)

A patch: no resource, face, argument or byte changes, and no `src/` change. The `ikigai-store`
floor moves from 0.2.2 to 0.2.10, a SECURITY floor. Every ledger caller holds this ledger's
store graph grants, and with them can resolve the store's graph doors directly; up to 0.2.9,
in a host with `oxigraph/http-client` unified on (`ikigai-cli`), a `SERVICE` or a
`LOAD … INTO GRAPH` sent there was an outbound request. Reproduced under exactly the
published grant list against a local stub (three `POST`s from the read doors, a `POST` and
two `GET`s from the update door); none on 0.2.10. See "SPARQL never leaves the process" above.
New test gate `http-client` (no code path is cfg'd on it), which CI runs.

### 0.6.1 (2026-10-10): a real id nonce in the browser (ledger #797); a page says it is one, `about` on its own, true `ledger` summaries (ledger #419, #398, #425)

A patch, by the JSON face's rule: fields and a resource ADDED, nothing renamed, removed or
retyped (`model::Filter` and `json::ItemsDocument` are `#[non_exhaustive]`).

- **`items` states the page and the total** (ledger #419). It said `50 item(s)` — a page's
  length worded as a count — over a ledger of 385, and a reader reported it seven times
  smaller than it was. A page smaller than the set now ends `1–50 of 385 item(s) — pass
  offset=50 for the next page, or a larger limit=`; the whole set on one page keeps the
  `N item(s)` line. The total is a COUNT query (`model::count_items`), never the page's
  length. New `offset=` (refused when not a count), counted in ITEMS: the page's IRIs are
  grouped by subject before `LIMIT`/`OFFSET`, so an item a hand edit gave two rows takes one
  slot on one page. JSON gains `total` and `offset` (`#[serde(default)]`, so a 0.6.0 document
  still deserializes, with `total: None`). The Turtle face is still the page's items alone:
  it has no count, because carrying one needs a vocabulary term. `next` already stated its
  ready count beside the ranking and is unchanged.
- **`about` without resending the body** (ledger #398). The engine's `sink` always sends
  `content`, empty when nothing is piped, so `sink …:item:12 priority=3` arrived with
  `content=""` and was refused as a blank title: no scalar edit was possible from the command
  line without retransmitting the title and body. A blank `content` now changes neither, so
  `priority=`, `about=` and `revision=` can each be set alone; a blank `content` with nothing
  else is still refused. And **`…:item:{id}:about`** (Sink, Delete) adds and removes targets
  as a resource of their own — `sink urn:iki:ledger:item:12:about urn:agents:session:…` —
  closing the "no `about` removal" omission. An item PART, so no new word is reserved.
- **Every `ledger` summary names its own resource** (ledger #425). One shared summary named
  `urn:iki:ledger:append` as the default-ledger form on every endpoint, and an agent that
  believed it on `comment` filed a junk item instead of a comment. `item=`'s summary now names
  the item IRI in its named form too. `tests/contract_and_paging.rs` holds every binding's
  summary to that binding's own pattern.

**The id nonce (ledger #797).**

On `wasm32-unknown-unknown` the id nonce now comes from `getrandom` over the JS `crypto`
object. `std` has no entropy source there, so `RandomState` was seeded identically in every
instance, and every browser load of the same module drew the same nonce: two loads filing one
item under one frozen clock minted the same IRI (reproduced under node). Native and WASI are
unchanged, byte for byte. A failed draw is refused as an endpoint error rather than replaced by
a constant. New target-only dependency: `getrandom` 0.3.4 with `wasm_js`, already in the graph
through oxigraph's `js` face.

### 0.6.0 (2026-10-09): `space()` names itself `urn:iki:space:ledger` (ledger #987)

No resource, face or byte changes. `space()` now claims `urn:iki:space:ledger`
(`ikigai_ledger::SPACE_ID`), so `urn:kernel:topology`, the space diagrams and
`Resolved::answered_by` name it instead of a skolem, and the cache partitions on that name.
**A minor release** because every host that binds `space()` sees that change; a host that
binds `space_with(…)` or `space_with_policies(…)` sees none, since those claim no name. Floors:
`ikigai-core` 0.1.89, where a later `bind` drops a space's name.

### 0.5.0 (2026-10-09): lifecycle state, assertions, leases and the doctor (ledger #775)

kata-flight's ledger seams, steps 1 and 2. Additive in behavior — every request 0.4.2 accepted
answers the same bytes in the plain face, and the JSON face gains one field — but **a minor
release**, because three public constants change type: `ledger::RESERVED` gains `lifecycle`
(20 words, with `doctor`), `vocabulary::STRUCTURAL_CLASSES` gains `Lifecycle`, `LifecycleState`
and `ClaimKind` (13), and
a `[&str; 18]` binding of the old array stops compiling; `select::Excluded` gains a variant
(and becomes `#[non_exhaustive]`). And `lifecycle` and `doctor` are now reserved ledger names:
a ledger called `lifecycle` could not be told apart from `urn:iki:ledger:lifecycle:{name}`,
and `doctor` joins every other action word.

- **`…:item:{id}:state`** (Source, Sink): one state or none (`filed`), moved by a compare-and-set
  in one store update; a lost race is a typed `Conflict`. See "Lifecycle state".
- **`urn:iki:ledger:lifecycle:{name}`** (Source, Exists): the lifecycles a host offers; the
  built-in is `kata-flight`. `SpaceConfig` and `space_with` take a host's own.
- **Claims** (step 2): `lease=`, a host-stamped kind (`SpaceConfig::claim_kind`, default
  `machine`), `takeover=true from=<holder>` for an expired lease, and every take is now one
  guarded store update. `next` excludes an expired lease as `lease-expired`. See "Claims".
- **`…:{ledger}:doctor`** (Source): orphaned, abandoned, lease-expired, unknown-state and
  two-states, as stored SPARQL, read-only. See "The doctor".
- **Assertions** (Exists): `…:item:{id}:state:{value}`, `…:holder:{holder}`, `…:closed`.
- **JSON**: every item object gains `state`; `claim` gains `kind`, `expires` and `lease`;
  `next`'s exclusions gain `expires` and the `why` `lease-expired`; the doctor serves its own
  document. The plain item detail gains a `state:` line, only
  for an item that has one.
- Vocabulary: `ledger:Lifecycle`, `LifecycleState`, `hasState`, `order`, `inFlight`, `drain`,
  `state`, `stateAt`, `stateToken`; `ClaimKind`, `machine`, `person`, `claimKind`, `lease`,
  `leaseExpires`.

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
