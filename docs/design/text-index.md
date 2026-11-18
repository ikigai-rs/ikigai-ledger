# Design: a full-text index for ledger item search

Status: **proposed** (ledger #3804). This is a design only: nothing here is built, and no
dependency has been added. Brian decides whether and how to implement it. The decisions he
needs to make are listed at the end.

Related: PR #130 (ledger #3803), which extends today's scan to item bodies and comments with
`match=`, `search=`, `search_mode=`, `whole_word=` and a per-row `matches` excerpt. This design
keeps those arguments exactly as that PR defines them.

## Recommendation, in one paragraph

Build the index **inside `ikigai-store`**, behind a feature that is off by default, using the
[tantivy](https://crates.io/crates/tantivy) crate. Keep **one index per named graph**, and
index only the graphs and predicates the host enrolls. For the ledger, that is each ledger's
own graph, with `dcterms:title` and `ledger:body`. Serve it through one new scoped door,
`urn:iki:store:graph-text`, gated by the same `urn:cap:store:read:graph:<iri>` tokens as
`graph-select`. Its answers are cached under the same per-graph threads. In the ledger, the
index is only ever an **accelerator**. It returns candidate items, and the ledger checks every
candidate with the filter it already runs today, so `text=`, `search=` and `whole_word=` give
exactly the same answers with or without the index. When the index is missing, stale or still
being built, the ledger falls back to the scan. Ranking and phrase matching are new and
additive: `sort=relevance` and `phrase=`.

## What is true today

- `items text=` adds one filter, `FILTER(CONTAINS(LCASE(STR(?title)), LCASE(…)))`
  (`src/model.rs`, the `filter.text` clause). I checked this on `main` at `655e4da`. PR #130
  adds bodies and comments. Bodies and comments share `ledger:body`, so the scan reads one
  predicate's literals for both.
- Every read goes through the store's scoped doors (`urn:iki:store:graph-select`) with the
  caller's capability. Every write goes through `urn:iki:store:graph-update`. This crate owns
  no bytes (`Cargo.toml`, `README.md` "Composition").
- Writes can also arrive **out of band**: through `graph-update` from anyone who holds the
  graph's write grant, and through the broad `urn:iki:store:update` and `urn:iki:store:load`
  from anyone who holds the store's broad grant (`README.md`, "Assume an editor got there
  first"). An index that sees only the ledger's own writes would go stale on that path.
- Cached reads depend on `urn:iki:store:update`, `urn:iki:store:load` and each graph's own
  thread, `urn:iki:store:graph-written:{graph}` (`ikigai_store::graph_thread`, store 0.5.0).
  The kernel passes a sub-request's threads up to whatever issued it, so a ledger read is
  recomputed after any write to its graph without declaring anything itself.
- Oxigraph has no full-text index. Its `CONTAINS` and `REGEX` read every literal they are
  given.

The reporter measured a scan for `the` at 0.73 s on a ledger of 3,682 items. I did not
reproduce that figure. It comes from a live ledger on RocksDB, through the whole stack. My
measurement below covers only store evaluation, in memory, so it is a lower bound on what a
caller waits for.

## 1. Where the index lives

| option | sees every write? | authority | verdict |
| --- | --- | --- | --- |
| **beside the store, inside `ikigai-store`** | yes: all three write doors are its own | the existing per-graph read tokens, checked at the door | **recommended** |
| inside `ikigai-ledger` alone | no: only the ledger's own writes; out-of-band edits go stale | the ledger's grants only | rejected |
| a separate module serving text search over any graph | no: it can only learn *that* a graph changed (a thread cut), not *what* changed | it would have to repeat the store's checks | rejected for now |

**Why the store.** The index must change whenever the indexed text changes, and only the store
sees every change. The scoped door already computes the exact difference it applies: it runs
the update on a private copy and then "appl[ies] the difference to the real store's `G`, in one
transaction" (`ikigai-store`, `confine.rs`). That difference is exactly the input an index
update needs. Two more things come with the store:

- the per-graph read check (`urn:cap:store:read:graph:<iri>`), with its rule that a request
  naming an ungranted graph is refused, never answered over the subset it holds;
- cache invalidation under `graph_thread`, through `cached_under(…, Some(graphs))`.

Putting the index anywhere else means rebuilding both.

**Why not the ledger alone.** This crate owns no bytes by design. A second store inside it
would need its own durable place and its own lock. It would also miss every out-of-band
write, which this ledger treats as a first-class path.

**Why not a separate module, yet.** A module beside the store could subscribe to
`graph-written:{graph}`, but a thread cut does not say which quads changed. Catching up would
mean re-reading every indexed literal in the graph after each write: about 4 s of rebuild per
write on the 10,000-item ledger measured below. That is the linear cost this design exists to
remove. If the store ever publishes a change feed, this option reopens. Until then it is the
store or nothing. (`urn:text:*` also already belongs to `ikigai-text`'s coreutils endpoints, so
a separate module would need another name.)

**Why tantivy.** The item asks for a vetted crate, not a hand-written tokenizer or ranking.
tantivy is a Lucene-style engine (MIT, used in production by Quickwit and others). It
provides:

- BM25 scoring;
- phrase queries over stored positions;
- tokenizers, lowercasing, n-grams and stemmers;
- segment merging and garbage collection;
- `Bm25StatisticsProvider`, which ranks across several indexes as if they were one. The union
  section below relies on it.

I checked these against tantivy 0.27.0's source. The costs:

- **Dependencies:** 116 crates in its normal dependency tree with default features. The store
  would carry them only under the feature.
- **Write cost:** a commit costs real time. See "Cost".
- **Browser:** with default features off, it compiles for `wasm32-unknown-unknown` once
  `getrandom` and `uuid` get their JS randomness features. Its index writer spawns threads,
  though, so I expect it not to run in a browser. I did not try. The browser build keeps the
  scan.

The alternatives I looked at were weaker. SQLite FTS5 (through `rusqlite`) brings in a C
library and a second storage engine. The small BM25 crates are in-memory only, with no
positions, merging or durable format. This is the same arrangement Apache Jena uses: a Lucene
index beside TDB, kept current by the dataset's own write path.

## 2. Keeping it current

### What a document is

The store indexes **literals, not items**. A document is one (subject, predicate, literal) in
an enrolled graph, under an enrolled predicate. Its key is a hash of the three, so the same
quad always has the same key. It holds:

| field | kind | why |
| --- | --- | --- |
| `key` | exact term, not tokenized | delete and replace one document |
| `subject` | exact term, stored | what a hit names (an item, or a comment) |
| `predicate` | exact term, stored | which field matched (`title` or `body`) |
| `word` | tokenized words, with positions | ranking, phrases, whole-word candidates |
| `gram` | lowercase trigrams, with positions | substring candidates for `text=` and `search=` |

**No text is stored in the index.** Excerpts (`matches`) still come from the store, as in PR
#130. That keeps two promises simple:

- an excerpt can only come from a read the caller is allowed to make;
- a purge does not have to chase a second copy of the text.

The store stays generic. It knows nothing about items or comments. The ledger maps a comment
hit to its item through `ledger:onItem`, in the check query it runs anyway.

A comment is its own document, so **appending a comment adds one document and rewrites
nothing**. The alternative, one document per item with all of its comments, would rewrite the
whole item on every comment. That cost would grow with every comment, which is the growth
the item asks to avoid.

### Each write

| write | quads it changes | index change |
| --- | --- | --- |
| `append` | a new item's title and body | add 2 documents |
| `comment`, `note` | a new comment's body | add 1 document |
| edit of title or body | remove the old literal, insert the new one | delete 1 key, add 1 document |
| claim, label, link, close, state | no indexed predicate | none; the graph's thread is still cut, as today |
| `delete` | the item and its dependents leave the ledger's graph and enter `…:deleted` | delete their keys; the graveyard is not enrolled, so nothing is added |
| `purge` | removed from the graveyard and, for a live item, from the ledger's graph | delete their keys, then **compact** (below) |
| `graph-update` from out of band | whatever the update's difference says | the same rule, applied to the difference |
| broad `urn:iki:store:update` | unknown: the broad door computes no difference | mark every enrolled graph stale and rebuild it; searches scan until each rebuild is done |
| `urn:iki:store:load`, including a restore | the loaded quads, insert only | add their documents; for a large load (a whole-dataset restore), drop the enrolled graphs' indexes and rebuild |

The rule for every row is the same: the store turns the difference it is about to commit into
index changes. Re-adding a quad that is already present replaces its document under the same
key, so a retried delete, an `INSERT DATA` of quads already there, or a load that repeats a
backup all converge on the same index.

### When an index change becomes visible

A tantivy write is searchable only after a commit, and a commit costs time (measured below: 141
ms with `fsync` on this machine, about 5 ms on a RAM directory). Paying that inside the store's
write lock on every comment would make every write slower. Skipping it would let a read
recomputed just after a write miss the new text and cache that wrong answer.

The design avoids both because **the ledger checks every candidate against the store anyway**:

1. Each graph's index keeps a **pending set**: the subjects whose documents changed since its
   last commit. Adding to it costs nothing measurable.
2. A search answers *index candidates ∪ pending subjects*. The ledger's check (the existing
   filter, over `VALUES ?item { … }`) keeps only the ones that really match in the store as it
   is now. A deleted subject fails the check, and a new one passes it.
3. The store commits each graph's index on a short timer (for example, every second), or
   immediately when a **ranked** query finds a non-empty pending set, because ranking needs
   scores, and pending subjects have none.

So a write pays for adding documents, never for a commit. Unranked searches are exact at every
moment. A ranked search pays a commit only when a write happened since the last one.

### Losing the index, changing its format, restoring

The index is **derived**. Nothing in it is the only copy of anything, so it is never backed
up, and losing it costs a rebuild, never data.

- **Where it lives:** in a subdirectory of the store's directory, so replacing or restoring the
  store's directory carries the index with it, or leaves none.
- **What it records:** each graph's index metadata records:
  - the index format version;
  - a fingerprint of the analyzer configuration (tokenizers, enrolled predicates);
  - a clean-close marker, written when the store closes with nothing pending.
- **When it rebuilds:** on open, any mismatch, a missing marker or a missing index marks that
  graph's index stale. It is then rebuilt in the background from a `SnapshotReader`, the
  store's own consistent read, restricted to the enrolled graphs and predicates.
- **While it rebuilds:** searches of that graph use the scan, which returns the same results.
  Only `sort=relevance` is refused until the rebuild finishes (see "The contract").
- **What it costs:** a full rebuild of the 10,000-item, 50,000-comment ledger took 3.9 s for
  the word field and 2.7 s for the trigram field (below).

### Compaction, for a purge

tantivy deletes a document by marking it. Its terms stay in the segment files until a merge
rewrites them. Storing no text helps, but the tokens of a pasted secret would still sit on disk
after a purge said they were gone. So a purge finishes with
`urn:iki:store:text-index:graph:{graph}:compact`. That call:

1. commits;
2. merges every segment that holds a deleted document;
3. collects the old files;
4. returns only after all of that.

It needs the graph's write grant, which a purge already holds for both graphs it touches.
Edits and recoverable deletes do not compact. Their old text is still in the record (the
graveyard, an edit event), so there is nothing to destroy.

### Cached search answers

A search answer from `graph-text` is cacheable under exactly the threads a `graph-select` over
the same graphs would use:

- `urn:iki:store:update`;
- `urn:iki:store:load`;
- `urn:iki:store:graph-written:{graph}` for each graph it read (`cached_under(…, Some(graphs))`).

The ledger's `items`, `count` and `next` inherit these through the kernel, as they already do.
So **any write to a ledger's graph recomputes every cached search of that ledger**, and a write
to another ledger leaves them alone. A rebuild or a compaction finishing also cuts the graph's
thread, through the `graph-written:{graph}` resource the store already binds. That is because
ranked scores can change then, even though the set of matches cannot.

Per-term threads would recompute less. A comment without the word "parser" would then leave a
cached search for "parser" alone. I recommend against them:

- Every write would have to cut a thread for every token it added or removed: hundreds of cuts
  for one comment.
- BM25 scores depend on the whole graph's statistics, so every write changes every ranked
  answer anyway.
- Today, any write to a ledger already recomputes all of its cached listings. A search is no
  worse than that.

## 3. Authority

A search must never reveal an item, or an excerpt of one, that the caller cannot read. The
design does not rely on the ledger to keep that promise: the store's door would refuse the
request anyway.

```text
items ledgers=alpha,beta search=parser
  ├─ ledger: for each named ledger, check read:{ledger} and the store read token
  │          for its graph; a ledger missing either is silently omitted (as today)
  ├─ store:  urn:iki:store:graph-text graph=<alpha graph> graph=<beta graph> …
  │          refused unless the caller holds urn:cap:store:read:graph:<iri> for EVERY
  │          graph named (the graph-select rule); answers subjects, predicates, scores
  ├─ ledger: check the candidates with urn:iki:store:graph-select over the same graphs
  │          (the same token check again), and map comments to their items
  └─ ledger: excerpts (matches) from the store's text of the rows it is returning
```

- **The check is at the door, per graph.** `graph-text` takes the caller's capability
  unchanged, as every sub-request does, and checks it exactly the way `graph-select` checks
  it. The ledger only names graphs that passed its own two checks, so the door's refusal is a
  second line of defense, not the only one.
- **One index per graph, not one shared index with a graph field.** A shared index leaks even
  when every query filters by graph. BM25 scores use document frequencies over the whole
  index, so a caller who reads only `alpha` could watch the score of `acquisition` change as
  `beta` fills up with documents containing it. Separate indexes have separate statistics.
  Dropping, rebuilding and compacting one ledger also touches nothing else.
- **A union ranks over the readable graphs only.** For `ledgers=alpha,beta`, the door searches
  each graph's index. It gets document counts and frequencies from those same indexes through
  tantivy's `Bm25StatisticsProvider`, so scores are comparable across the union and say
  nothing about any other graph.
- **No text in the index, no excerpt from it.** `matches` is computed from literals the ledger
  read through `graph-select`. A row the caller cannot read never reaches that step.
- **Enrollment is the host's.** The graveyard (`…:deleted`) and the session-summary graph
  (`…:session`) are not enrolled. Session summaries have their own `summaries:{ledger}` grant,
  and putting them in a ledger's item search would bypass it.

The per-graph indexes cost something too. Each open tantivy writer reserves memory (15 MB at
the smallest budget I used). A host with many ledgers should open writers on demand and close
idle ones.

## 4. The contract

The index sits behind the same `items` arguments as the scan, and the same `count` and `next`
arguments. A caller who uses none of the new arguments below gets the same rows, in the same
order, with the same bytes, whether the index is there or not.

| argument | meaning, unchanged by the index | how the index helps |
| --- | --- | --- |
| `text=` | one case-insensitive substring | trigram candidates, then the existing filter on those only |
| `search=`, `search_mode=all\|any` | several substrings, all or any | one trigram candidate query per word, intersected or united, then the existing filter |
| `whole_word=true` | REGEX `\b…\b` | word candidates (the term's tokens as a phrase), then the existing REGEX filter |
| `match=title,body,comments` | which fields | restricts the `predicate` field; comments are `ledger:body` on a subject with `ledger:onItem` |

A term shorter than three characters, or one with no word characters (`++`), has no useful
candidates, so it scans, as today.

**Why the index never answers alone.** The index's idea of a word is not the same as a
substring, and not quite the same as a regex `\b`. On the measured corpus below, a word search
for `staff` found 2,537 documents. The substring scan found 2,800, because `staffs` and
`flagstaff` also contain it. tantivy's default tokenizer also splits words at `_`, but `\b`
does not. If the index answered alone, results would change when it arrived, which the item
rules out. Checking candidates instead keeps the scan as the definition, and the index only
decides where to look.

**What is new, and additive.** All of these keep the JSON face on schema 1:

- **`sort=relevance`**: a new value for the existing `sort`. It orders the checked rows by
  score, with the current default order as the tie-break.
  - An item's score is its best document's BM25 score, with a title match weighted higher
    (the weight is a decision below).
  - It is refused without `text=`, `search=` or `phrase=`, and it is refused when the
    ledger's index is missing or still building. In both cases the refusal is one sentence
    that names the ledger and says that nothing was listed. That follows this crate's rule
    that a face it cannot produce is refused, never replaced with another.
  - The default order does not change.
- **`phrase=`**: one new, singular argument. Its words must appear in order and next to each
  other, case-insensitively, in one field (`phrase="lease ran out"`). I chose a new argument
  over quoting inside `search=` because PR #130 treats a quote in `search=` as an ordinary
  character. Giving it a new meaning would change the results for a caller who already sends
  one.
  - With the index, it is a phrase query on `word`.
  - Without the index, it is a REGEX built from the same tokens.
  - Tests must check that both paths give the same answer on the same corpus.
  - It combines with the other arguments like any filter.
- **`score`** in JSON: a number on each row, present only under `sort=relevance`, and left
  out of the bytes otherwise, the way `matches` is when there is no search.
- **Plain face:** rows are unchanged. Under `sort=relevance`, the footer says the order in
  words: `40 item(s), most relevant to "parser" first`. Order is never shown only by position
  (ACCESSIBILITY.md).
- **`next`:** text arguments filter the ready set, as in PR #130. Relevance never changes
  `next`'s ranking, which stays its policy's, so `sort=relevance` is not offered there.

## 5. Names

All of these belong to `ikigai-store`. The ledger adds no resource, only the arguments above.

| resource | verbs | holds state? | gated by |
| --- | --- | --- | --- |
| `urn:iki:store:graph-text` | Source | **computed**: a query over the named graphs' indexes; holds nothing of its own | `urn:cap:store:read:graph:<iri>`, every graph named |
| `urn:iki:store:text-index:graph:{graph}` | Source | **state**: the graph's index (format, fingerprint, documents, size, pending count, and whether it is ready, stale or building) | `urn:cap:store:read:graph:<iri>` |
| `urn:iki:store:text-index:graph:{graph}:rebuild` | Sink | computed: rebuilds the state above from the graph's quads | `urn:cap:store:write:graph:<iri>` |
| `urn:iki:store:text-index:graph:{graph}:compact` | Sink | computed: merges away deleted documents of the state above | `urn:cap:store:write:graph:<iri>` |
| `urn:iki:store:text-enrollment` | Source | **computed** from the host's configuration: which graph patterns and predicates are indexed, with which analyzers | any `urn:cap:store:read*` |

- Each name puts its type first and is singular (`text-index`, never `text-indexes`).
- The one partition is by a named key, `graph:{graph}`, with the graph IRI percent-encoded the
  way `graph_thread` encodes it.
- `graph-text` follows the store's existing `graph-select` and `graph-ask` family: one IRI for
  each query form.
- No new thread: invalidation reuses `urn:iki:store:graph-written:{graph}`.

## 6. Cost

**How I measured it.** I wrote a throwaway Rust program outside every repository. Its source is
in the PR that adds this document. It used tantivy 0.27.0 and oxigraph 0.5.13 (in memory,
default features off), in a release build, on a shared 16-core Linux machine with 62 GiB of
memory. Medians of 7 runs (the scan) or 21 runs (everything else), after one warm-up.

**The corpus.** 10,000 items and 50,000 comments, assigned to items at random, 58.0 MiB of
text in all:

- titles of 4–11 words, bodies of 40–399 words and comments of 20–249 words;
- words drawn from a Zipf distribution (s = 1.07) over 30,025 words: 25 English stop words
  ranked first, then 30,000 dictionary words.

The test words were `the` (in every item), `staff` (2,441 items) and `healthiness` (121
items).

| measurement | result |
| --- | --- |
| scan, title + body + comments, `the` | 90.6 ms |
| scan, `staff` | 80.7 ms |
| scan, `healthiness` | 81.2 ms |
| word index on disk (`word` field, positions, no stored text) | 22.0 MiB (38% of the text) |
| trigram index on disk (`gram` field) | 34.4 MiB (59% of the text) |
| full rebuild: word field / trigram field | 3.9 s / 2.7 s |
| one comment: add and commit, on disk with `fsync` | 141 ms |
| one edit: delete a key, add and commit, on disk | 184 ms |
| 100 comments added, then one commit, on disk | 153 ms |
| one comment: add and commit, RAM directory, one thread | 5.0 ms |
| word query, top 50 by BM25 plus an exact count: `the` | 0.71 ms (61,101 documents) |
| … `staff` | 0.10 ms (2,537 documents) |
| … `healthiness` | 0.03 ms (127 documents) |
| … phrase `"the staff"` | 0.38 ms (347 documents) |
| … both `staff` and `healthiness` | 0.05 ms (6 documents) |
| trigram candidates for substring `staff` | 0.30 ms (2,969 candidates for 2,800 true documents) |
| … `healthiness` | 0.61 ms (956 candidates for 122 true documents) |

**What these numbers say:**

- **The scan grows with the text.** It cost about the same whatever the term, because it
  reads every literal: about 80 ms per 58 MiB, before RocksDB and the rest of the stack.
- **Index queries cost microseconds to under a millisecond**, and grow with the number of
  matches, not the size of the ledger.
- **The end-to-end cost was not measured.** It is the index query, plus the ledger's check of
  the candidates and its page read. The check grows with the number of candidates. For a rare
  or mid-frequency term, it touches a few thousand literals or fewer, instead of 60,000.
- **The index does not help an exact count of a term in every item.** For `the`, every item
  is a candidate, and an exact `total` for a substring in every item still costs a scan.
  Ranking fixes the order for such a term, not the cost of counting it.
- **Trigram candidates are a loose superset.** For `healthiness`, 956 candidates contained the
  122 true matches, because tantivy's n-gram positions do not line up as a phrase needs. That
  is why candidates are always checked. Tighter trigram positions would need a custom
  tokenizer, which this design avoids.
- **Write overhead is the commit, and the design keeps it off the write path.** Adding a
  document costs about 0.1 ms: 100 adds plus a commit cost 12 ms more than one add plus a
  commit. With the pending set, a write only adds. The commit (141 ms with `fsync` here, about
  5 ms without) runs on the timer, or once before a ranked query. Because the index is derived,
  it does not need `fsync`: a crash costs a rebuild. That choice is a decision below.
- **Total size:** for this ledger, about 56 MiB of index beside 58 MiB of text: 22 MiB for
  words and 34 MiB for trigrams. Without the trigram field, `text=` and `search=` would keep
  scanning, and only `whole_word=`, `phrase=` and ranking would use the index.

I did not measure the store's own size on RocksDB, the end-to-end latency of a ledger read, or
a merge after a purge on this corpus.

## 7. Wider use

Because the index lives in the store and indexes enrolled literals in enrolled graphs, it is
not specific to the ledger. Any module that keeps text as literals in a named graph can enroll
that graph and predicate, and gets search under its own graph's read token.

- **Documentation retrieval:** yes, if the documentation is in a graph. BM25 is also a good
  first stage for retrieval that a language model reads, and it can sit beside embeddings
  later.
- **Source code search:** not now. Code is not stored in graphs. It also needs its own
  tokenizing (identifiers, symbols, case splitting), and its scale is different. The trigram
  field is the standard basis for code search, so the format would not stand in the way. But
  I would not build anything for code until something stores code as literals.

So the recommendation is a generic store feature, used first and only by the ledger, with
enrollment as the line between them.

## Decisions for Brian

1. **Where it lives.** A feature of `ikigai-store` (recommended), or one of the two rejected
   options. Building it in the store is a store release first, and then a ledger release that
   needs that store version.
2. **tantivy as a dependency.** 116 crates under a feature that is off by default. The browser
   build keeps the scan. tantivy compiles there, but its writer needs threads, and I did not
   try running it.
3. **The trigram field.** Index substrings (34 MiB on the measured corpus) so `text=` and
   `search=` get faster, or index only words (22 MiB) and leave substring searches on the
   scan.
4. **Durability of the index directory.** Commit without `fsync` (about 5 ms) and rebuild after
   an unclean shutdown (recommended, since the index is derived), or `fsync` every commit
   (141 ms here).
5. **The commit timer.** How long the pending set may grow before a commit (one second is
   proposed), given that a ranked query commits first anyway.
6. **Ranking.** BM25 per document. Whether an item's score is its best document's score or a
   sum, how much a title match weighs more, and whether to stem English words for ranking
   only.
7. **The new names.** `sort=relevance`, `phrase=`, the JSON `score`, and the five store
   resources in section 5.
8. **Ranked reads without an index.** Refuse `sort=relevance` (recommended, and consistent
   with this crate's refusals), or answer in the default order and say so.
9. **Compaction on purge.** An explicit `compact` call that a purge waits for (recommended), or
   a store that compacts on every removal of indexed text.
10. **Invalidation granularity.** The per-graph threads that exist today (recommended), or
    per-term threads.
11. **The broad doors.** After a broad `urn:iki:store:update`, mark every enrolled graph stale
    and rebuild it (recommended), or teach the broad door to compute a difference.
12. **Order of work.** This builds on PR #130's arguments, so that PR lands first, unchanged.
