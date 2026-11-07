# Accessibility

What `ikigai-ledger` emits that people read, the rules that output follows, how it is tested,
and how to report a barrier. It is not a conformance claim. Adapted from the
[ACCESSIBILITY.md template](https://github.com/mgifford/ACCESSIBILITY.md) (MIT).

| Field | Value |
|---|---|
| Target | WCAG 2.2 Level AA, for any page that presents this output; this repository writes text only (no HTML, SVG or color) |
| Status | Evaluation in progress. No conformance claim. |
| Owner | the maintainers of this repository |
| Report a barrier | [a GitHub issue](https://github.com/ikigai-rs/ikigai-ledger/issues/new?labels=accessibility) with the `accessibility` label; anything security-sensitive through [private vulnerability reporting](https://github.com/ikigai-rs/ikigai-ledger/security/advisories/new) |
| Last reviewed | 2026-11-06 |

## What it emits that people read

This is a library: the ledger's endpoints, which a host binds. Its default face is plain text
that people read in a terminal, and it also answers JSON (`as=application/json`) and Turtle.

- **An item's line and detail** (`Item::line` and `Item::detail` in `src/model.rs`): one fixed
  line per item, `#12  open    p1  Title  [labels]  claimed:holder`, and under it, in the
  detail, labeled lines (`iri:`, `filed:`, `updated:`, `author:`, `committer:`, `about:`,
  `blocks:`, `child:`, `blocked-by:` …), the body, and the comments, each stamped with its time and author
  (`Comment::render`); a comment that rejected a hand-back has a `rejects:` line per hand-back
  under its stamp, naming that comment. Whether an author or a date is verified or claimed is said in words in
  parentheses (`(verified)`, `(claimed by a delegate)`, `(claimed)`), never by a sign alone.
- **Listings** (`items`, `ledgers`, `count`): item lines and a total (`3 item(s)`); `count`
  answers a bare number. `items ledgers=alpha,beta` combines readable selected ledgers
  into one filtered page, keeping each row's existing `alpha#12` display (`#12` for
  `default`) and the same page and total footer. Unreadable ledgers are omitted silently;
  no names or warnings for them are emitted. `sort` orders the same rows before paging;
  `landing` places blockers first. The row shape and footer are unchanged.
- **`urn:iki:ledger:next`** (`Selection::plain` in `src/select.rs`): ranked item lines, each
  with its score and its reasons in words, then the policy and what was excluded and why
  (`not #1: blocked by #2`), or `nothing is ready (…)` with the reasons.
- **An item's history** (`src/endpoints/history.rs`): a first line with the item and a count
  (`#1: 3 event(s)`), or the sentence `#1 has no recorded events`, then one line per event,
  oldest first: its time, what happened in words (`state filed to queued`, `edit title, body`)
  and `by` who. An edit with no object says so in words. A close names its verdict and its
  evidence in words (`closed done, evidence …`, `closed duplicate of …`). An event an import
  principal recorded afterward says so in words: `(claimed)` after its time, `imported` before
  what happened, and `(claimed by an importer)` after who did it at the source. A state change
  names the lifecycle's version in words (`lifecycle version 2`), or says it has none
  (`no lifecycle version recorded (written before 0.18.0)`).
- **Write answers** (`src/endpoints.rs`): one sentence each, such as
  `#1 urn:iki:ledger:default:item:…`, `commented on #1 (…)`, `linked #1 blocks #2`,
  `closed #2 (done)`, `closed #2 done (2 evidence)`, `closed #2 duplicate of #1`,
  `#1 claimed by worker-1 at epoch 1`, `#1 renewed by worker-1 at epoch 1 until …`, `#1: imported events recorded 3, already recorded 1; history only, so
  the item itself did not change`.
- **The doctor and lifecycles**: the doctor's findings, each with a `remedy:` line; a
  lifecycle's states, one per line (`Lifecycle::plain` in `src/lifecycle.rs`), each saying in
  words where it may go when the lifecycle declares moves (`moves to: queued`, `moves to: nowhere`).
- **Refusals**: `InvalidArgument` naming the argument with a sentence that says what is
  accepted (`` `x` is not one of open, closed, all ``, `` `9` is not 0 (highest) … 4 (lowest) ``,
  `` `triag` is not a drain of the lifecycle `kata-flight` this ledger uses; one of triage, … ``),
  `` `p1` is not one of 0, 1, 2, 3, 4, none (no priority set) `` for `priority` or
  `without_priority` listing filters,
  a label outside `A-Z a-z 0-9 . _ : -` names the character in words (`` `a b` contains a space, and a
  label may hold only … Add each word as its own label ``),
  an unknown or empty `sort` key names all accepted keys (`priority`, `created`, `modified`,
  `number`, `landing`) and explains commas and descending `-` in words. A landing cycle
  names the items in or depending on it and says to remove a blocks link or select another
  sort. `NotFound` for an unknown item, and errors that name what is missing (a store, a clock).
  A write fenced with an `epoch=` that is not current is a `Conflict` sentence naming who holds
  the item now and at which epoch, and saying nothing was written (`` #1 is held by worker-2 at
  epoch 2, not epoch 1: a write fenced with `epoch=` lands only while that claim is current, so
  nothing was written ``), or that nobody holds it (`#1 is not claimed (its last claim was epoch
  2), so epoch 1 is not current and nothing was written`).
- **The epoch check** (`source urn:iki:ledger:claim item=… epoch=…`): one sentence giving the
  state in a word, `current`, `superseded` or `not current`, and who holds the item
  (`#1 epoch 1 is superseded: held by worker-2 at epoch 2`). A claimed item's detail has an
  `epoch:` line.

Not this repository's: the titles, bodies and comments people write, which are shown as
written; and how a host or a terminal presents any of this.

## The rules that output follows

Verified by reading the source:

- An item's line has a fixed shape, so it reads the same down a column and greps the same way;
  its status is a word (`open`, `closed`), and a claim, a deferral, a hand-back and a close
  reason are each words on the line (`claimed:…`, `deferred`, `awaiting-…`, `(wontfix)`).
- Every detail line starts with its label; a warning is the sign `⚠` followed by words that
  say what is wrong.
- `next` gives its answer first, then a reason in words for every ranking and every exclusion;
  an empty ledger is a sentence, never an error.
- A refusal names the argument and what it accepts; a doctor finding names a remedy.
- No color, no terminal control sequences and no markup are written.

## How it is tested

- `tests/claim_epoch.rs`: `the_check_answers_each_state` pins each sentence of the epoch check,
  and `each_write_with_a_stale_epoch_is_refused_and_writes_nothing` and
  `each_write_with_the_epoch_of_a_released_claim_is_refused` hold each fenced refusal to naming
  the current holder and epoch, or that nobody holds the item.
- `tests/faces.rs`: `the_plain_item_detail_is_unchanged`, `the_plain_listing_is_unchanged`,
  `the_plain_write_answers_are_unchanged`, `the_plain_next_is_unchanged`,
  `the_plain_count_is_the_bare_number` and `a_keyed_items_plain_detail_gains_one_line` pin the
  plain faces byte for byte.
- `tests/selection.rs`: `an_empty_ledger_says_nothing_is_ready_rather_than_failing`,
  `a_block_cycle_is_refused_with_the_cycle_named`,
  `an_unknown_policy_is_refused_and_the_known_ones_are_named`.
- `tests/leases_doctor.rs`: `a_claim_without_a_lease_renders_as_it_always_did` and the doctor
  tests; `tests/children.rs`: `every_face_of_an_item_names_its_children`;
  `tests/endpoints.rs`: `a_ledger_without_a_store_says_what_is_missing`,
  `an_unknown_item_is_not_found_rather_than_silently_ignored`.

These test the faces for correctness and stability; none was written as an accessibility test.
**No automated accessibility checker, browser or assistive technology has been used on this
output.**

## Known limitations

Found by reading the source on 2026-10-31:

- **Abbreviations on the item line.** Priority is `p0` to `p4`, and `p-` means none; nothing on
  the line says so, and a screen reader reads `p-` as a letter and a dash.
- **Parenthesized plurals**: `3 item(s)`, `1 comment(s)`, `2 open item(s) excluded`, which a
  screen reader reads with the parentheses.
- **Grouping by indentation.** A comment's body belongs to its stamp, and a `remedy:` to its
  finding, by indentation only, which a screen reader usually does not announce.
- **Long identifiers**: write answers and the `iri:` line carry the item's full IRI, whose
  opaque id of 16 or 23 characters a screen reader reads character by character; the short
  `#12` beside it is the readable handle.
- Times are UTC timestamps with milliseconds; a reader converts to local time themselves.
- Every message is English only.

## Contributing

Read this file before changing any output listed above: an item's line or detail, a listing,
`next`, a write answer, the doctor or a refusal. Keep meaning in words, keep the line's shape, and
update the golden strings in `tests/faces.rs` on purpose. Say in the pull request what was tested,
with what, and what was not.

## Reporting a barrier

[Open an issue](https://github.com/ikigai-rs/ikigai-ledger/issues/new?labels=accessibility) with
the `accessibility` label (the link sets it when GitHub lets you add labels; otherwise a
maintainer adds it): what you were doing, what happened, and the host, terminal and any assistive
technology you used, if you are comfortable sharing them. Anything security-sensitive goes through
[private vulnerability reporting](https://github.com/ikigai-rs/ikigai-ledger/security/advisories/new).
**We acknowledge a report within 7 days**, and promise nothing further.

## Change history

| Date | Change |
|---|---|
| 2026-10-31 | First version. |
| 2026-11-01 | `close closed-at=` (ledger #2819) adds refusals; each names the argument, the reason and the remedy in words. No plain face changed: `tests/faces.rs` passes unchanged. |
| 2026-11-01 | Aliases (ledger #2828) add write answers (`scheme gh owned by …`, `gh:acme/web#12 names acme#12 …`, `… no longer names …`) and refusals, each naming in words the alias, the scheme, its owner or the item that already holds it, and the grant that would do. An alias value appears percent-encoded in an IRI (`acme%2Fweb%2312`), which a screen reader reads character by character; the decoded value is what every answer shows. No existing plain face changed: `tests/faces.rs` passes unchanged. |
| 2026-11-01 | Links across ledgers (ledger #2829) add one detail line, `blocked-by: <IRI> (open; in the ledger cli, not readable under your grants)`, a `child:` line of the same shape, the `next` reason `blocked by <IRI> in the ledger cli`, a `⚠ not mirrored:` line a close or reopen adds when a target was beyond its grants, and refusals naming the grant (`urn:cap:ledger:link-in:acme`, the target graph's store grants) and the remedy. The status and whether the reader may read the other ledger are said in words. The blocker is named by its full IRI, which a screen reader reads character by character; its number is in the other ledger and is not this ledger's to show. No existing plain face changed: `tests/faces.rs` passes unchanged. |
| 2026-11-01 | An importer's history (ledger #2843) adds a write answer that says in words what was recorded, what was recorded already and that the item did not change, and it avoids a parenthesized plural; a history line for an imported event, marked in words; and refusals, each naming the event, the term or date and what is accepted, and saying nothing was written. Anyone else is told to write the change itself. A value that would break a history's one line per event is refused. The line for an event no importer wrote is unchanged, and so are the other plain faces: `tests/faces.rs` passes unchanged. `tests/imported_history.rs` pins the new line byte for byte; no assistive technology was used. |
| 2026-11-02 | A lifecycle's legal moves (ledger #2863) add a `Conflict` refusal that names in words the lifecycle, the pair, every legal move from `from` (or that there is none) and that nothing was written; refusals at load, one sentence each; and, only for a lifecycle that declares moves, `moves to: …` at the end of each state's line on the plain lifecycle face. A lifecycle with no moves, the built-in included, reads as it did, and the other plain faces did not change: `tests/faces.rs` passes unchanged. `tests/lifecycle_moves.rs` pins the new text byte for byte; no assistive technology was used. |
| 2026-11-02 | A state event's lifecycle version (ledger #2864) adds words to a state change's history line, `lifecycle version 2` or `no lifecycle version recorded (written before 0.18.0)`, before who did it, so that line reads longer; refusals at load, one sentence each, that name the lifecycle and what to add or fix. Other history lines and the other plain faces did not change: `tests/faces.rs` passes unchanged. `tests/lifecycle_version.rs` pins the new text byte for byte; no assistive technology was used. |
| 2026-11-02 | `next drain=` and `next state=` (ledger #2865) add a refusal for each, one sentence that names the value, the lifecycle, every drain or state it accepts and the lifecycle's IRI. A filtered `next` reads as `next` does, with fewer items on it, and a filter that admits nothing says the existing `nothing is ready` sentence. No plain face changed: `tests/faces.rs` passes unchanged. `tests/next_state_filter.rs` pins the refusals byte for byte; no assistive technology was used. |
| 2026-11-06 | A listing across ledgers (ledger #3190) adds `ledgers=` to `items` and `count`. Existing row shapes, page footers and single-ledger faces remain unchanged. A malformed comma-separated name is refused with the `ledgers` argument named and the ledger naming rule in words. `tests/listing_union.rs` checks the union, omitted unreadable ledgers in all three faces, shared filters, global paging, ties, and comments and link numbers from each row's graph. `tests/faces.rs` pins the existing plain and JSON output unchanged. This repository emits text and has no page, themes or browser accessibility checks; the gonk page's integration and checks in all themes belong to its follow-on. No browser, automated accessibility checker or assistive technology was used. |
| 2026-11-06 | Priority filters (ledger #3203) add `priority` and `without_priority` to `items` and `count`, including unions. Refusals name the argument and say in words which values are accepted, including `none (no priority set)`; empty comma entries are refused by the same sentence. Existing faces are unchanged. `tests/priority_filter.rs` checks the new refusal sentences and filter/count parity; `tests/faces.rs` pins existing output. No browser, automated accessibility checker or assistive technology was used. |
| 2026-11-06 | Listing sort (ledger #3192) adds `sort` to `items`. Rows and footers keep their shape. Unknown keys and landing cycles are refused in words with accepted keys or a remedy. `tests/listing_sort.rs` checks every key and direction, ties, paging, cycles, cross-ledger links and capability isolation; `tests/faces.rs` pins existing faces. This text-only library has no page, themes or browser accessibility checks; gonk's follow-on owns those checks in all themes. No browser, automated accessibility checker or assistive technology was used. |
| 2026-11-06 | Dated closes and reopens (ledger #2943) now mark their history time with `(claimed)`, matching other events with source dates; JSON history also includes their commit time. `tests/dated_replay.rs` checks both status events in plain text and JSON and keeps undated status output unchanged. No browser, automated accessibility checker or assistive technology was used. |
| 2026-11-06 | A label with a space (ledger #2634) is refused where it would be added: one sentence that names the label, the first character outside the grammar in words (`a space`, `a tab`, `a line break`, or the character itself), what a label may hold, and to add each word as its own label. No existing face changes. `tests/audit_minors.rs` checks the refusal for a space, a tab and `/`, that nothing is stored, and that an old label with a space can still be removed. No browser, automated accessibility checker or assistive technology was used. |
