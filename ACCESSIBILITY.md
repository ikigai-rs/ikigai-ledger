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
| Last reviewed | 2026-11-15 |

## What it emits that people read

This is a library: the ledger's endpoints, which a host binds. Its default face is plain text
that people read in a terminal, and it also answers JSON (`as=application/json`) and Turtle.

- **An item's line and detail** (`Item::line` and `Item::detail` in `src/model.rs`): one fixed
  line per item, `#12  open    priority 1  Title  [labels]  claimed:holder`, and under it, in the
  detail, labeled lines (`iri:`, `filed:`, `updated:`, `author:`, `committer:`, `about:`,
  `blocks:`, `child:`, `blocked-by:` …), the body, and the comments, each stamped with its time and author
  (`comment by … at …:`, `Comment::render`); a comment that rejected a hand-back has a
  `rejects:` line per hand-back
  under its stamp, naming that comment. A dispatch note (ledger #3718) has the line `note to
  worker-1 at epoch 2, sent under the dispatcher grant` under its stamp, saying in words what
  kind of comment it is, who it is for and under which grant; a comment that acknowledges a
  note has an `acknowledges note:` line naming it. Whether an author or a date is verified or claimed is said in words in
  parentheses (`(verified)`, `(claimed by a delegate)`, `(claimed)`), never by a sign alone.
  Under a decision comment's stamp, `decision maker:` and `decision time:` each name the
  relationship in words and end with `(claimed)` when that fact is present.
- **Dispatch notes** (`src/endpoints/note.rs`): sending one answers `note to worker-1 at epoch 2,
  sent under the dispatcher grant on acme#12 (<iri>)`. A refused note names the item, the epoch
  asked for and the current claim, and says that the note was not sent (`acme#12 is held by
  worker-2 at epoch 3, not epoch 2, so the note was not sent`). The worker's read,
  `item:{id}:notes`, starts with a count line in words (`acme#12: 1 note to worker-1 at epoch
  2:`, or `acme#12: no notes to worker-1 at epoch 2`), then each note's block as in the item's
  detail. A renewal with notes nobody acknowledged ends `, 1 unread note` (`, 2 unread notes`).
  An acknowledgment is the holder's: `acks=` from anyone else is refused in one sentence naming
  the item, the holder and its principal, and saying why and that nothing was written
  (`acme#12 is held by worker-1 as <principal>, and this capability does not act as that
  principal: a claim's notes are acknowledged by its holder, so nothing was written`).
- **Listings** (`items`, `ledgers`, `count`): item lines and a total (`3 item(s)`); `count`
  answers a bare number. `items ledgers=alpha,beta` combines readable selected ledgers
  into one filtered page, keeping each row's existing `alpha#12` display (`#12` for
  `default`) and the same page and total footer. Unreadable ledgers are omitted silently;
  no names or warnings for them are emitted. `sort` orders the same rows before paging;
  `landing` places blockers first. The row shape and footer are unchanged. A text search of
  bodies or comments (`match=body,comments`, ledger #3803) adds a line under an item for each
  of those fields that matched, naming the item and the field in words and quoting an excerpt
  on one line (`  #1 matched in the body: …the parser panics…`, `  #2 matched in a comment:
  …`); a title match adds none, since the title is on the item's line.
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
- **An item's sessions** (`src/endpoints/session.rs`): a first line with the item and a count
  (`acme#1: 2 session(s), newest first`), or the sentence `acme#1 has no recorded sessions`, then
  a block per session: `session {id} recorded {time}` and labeled lines in words (`harness:`,
  `model:`, `machine:`, `exit status:`, `wall clock: … ms`, `turns:`, `tool calls:`, `tokens:
  input …, cached input …, cache creation …, output …`, `hand-back:`, `iri:`). A count the harness
  did not report says `not reported`. Recording one answers
  `recorded session {id} on acme#1's hand-back`; a refused summary names the property and what is
  accepted (`ledger:harness must be one of claude, codex, and … is not`).
- **A ledger's sessions** (`SessionSetEndpoint` in `src/endpoints/session.rs`): a first line
  `acme: sessions, newest first, from offset 0`, or the sentence `acme has no recorded sessions`
  (`… at offset 50` past the last page), then the same block per session as an item's, with an
  `item: acme#1 urn:…` line under its first line. No count and no total: when more follow, the
  last line says so in words, `more sessions follow: read again with offset=50`.
- **Moving a ledger's sessions** (`SessionSetEndpoint::migrate`, ledger #3661): `plan` answers a
  first sentence saying how many summaries would move, from which graph to which, and that this
  is a plan and nothing was written; one indented line per summary, `session {id} on acme#12, 11
  triples`; and a closing sentence saying to send `move`. `move` answers one sentence with how
  many moved; a run with nothing to move says so and that nothing was written.
- **Write answers** (`src/endpoints.rs`): one sentence each, such as
  `#1 urn:iki:ledger:default:item:…`, `commented on #1 (…)`, `linked #1 blocks #2`,
  `closed #2 (done)`, `closed #2 done (2 evidence)`, `closed #2 duplicate of #1`,
  `#1 claimed by worker-1 at epoch 1`, `#1 renewed by worker-1 at epoch 1 until …`, `#1: imported events recorded 3, already recorded 1; history only, so
  the item itself did not change`.
- **Stalled items and lapses** (`src/endpoints/lapse.rs`, ledger #3709): `stalled` answers a
  first line with the ledger and a count (`default: 2 stalled item(s), the oldest lapse first`),
  or the sentence `default: no stalled items: no open item is held on a lease that has run out`,
  then one sentence per item (`#2 Ten: held by worker-2 at epoch 1, whose lease ran out at …; the
  lapse is not recorded yet`) and a `remedy:` line. The sweep, `lapse`, answers `default:
  recorded 1 lapse(s)` and the same sentence per lapse, indented, ending `; labeled needs-person`
  when the ledger's route added a label; a sweep that wrote nothing says so and why. An item's
  history names a lapse in words: `lapse worker-1 at epoch 1, whose lease ran out at …`.
- **The doctor and lifecycles**: the doctor's findings, each with a
  `remedy for <check> on <item>:` line; a
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

Source review of `src/model.rs`, `src/select.rs`, `src/lifecycle.rs`, and
`src/endpoints/flight.rs` on 2026-11-12 supports these output rules for library-authored text.
It does not evaluate user-authored content or a host's presentation:

- An item's line has a fixed shape, so it reads the same down a column and greps the same way;
  its status is a word (`open`, `closed`), and a claim, a deferral, a hand-back and a close
  reason are each words on the line (`claimed:…`, `deferred`, `awaiting-…`, `(wontfix)`).
- Detail metadata uses labeled lines; a warning is the sign `⚠` followed by words that
  say what is wrong. Bodies and comments also contain unlabeled text.
- `next` gives its answer first, then a reason in words for every ranking and every exclusion;
  an empty ledger is a sentence, never an error.
- A refusal names the argument and what it accepts; a doctor finding names a remedy.
- The plain renderers add no color, terminal control sequences or presentation markup.
  User-authored content is outside that source-review claim.

## How it is tested

Every named check has a result: Pass, Fail or Blocked. Pass covers only the stated check,
not overall accessibility. The Rust checks below were rerun on 2026-11-15 for ledger #3625,
using `cargo test`, Cargo 1.99.1 (5f94df478 2026-08-27) and rustc 1.99.1
(58cf8f9c3 2026-10-13). The new readability checks also ran separately with
`cargo test --test faces`. The source-review rows say what was inspected and when.
Pass applies to executed tests only; ignored performance measurements were not run.
These are correctness and stability tests, not an automated accessibility checker. CI
repeats the Rust tests through `.github/workflows/ci.yml`; it does not repeat the manual
source review or run assistive technology. The result format follows the
[starter accessibility update](https://github.com/ikigai-rs/ikigai-gonk-starter/pull/91).

| Check | Result | Evidence |
|---|---|---|
| Plain item detail, listing, write answers, next, count and keyed detail | Pass | `cargo test`: `tests/faces.rs` pins the plain faces byte for byte, including `the_plain_item_detail_uses_readable_priority_and_comment_groups`, `the_plain_listing_uses_readable_priority`, `the_plain_write_answers_are_unchanged`, `the_plain_next_uses_real_plurals`, `the_plain_count_is_the_bare_number` and `a_keyed_items_plain_detail_gains_one_line`. Does not test how a terminal or assistive technology presents them. |
| Epoch check and fenced refusals | Pass | `cargo test`: `tests/claim_epoch.rs`, including `the_check_answers_each_state`, `each_write_with_a_stale_epoch_is_refused_and_writes_nothing` and `each_write_with_the_epoch_of_a_released_claim_is_refused`. Checks wording and write safety; no usability evaluation. |
| Empty selection, cycle and policy refusals | Pass | `cargo test`: `tests/selection.rs`, including `an_empty_ledger_says_nothing_is_ready_rather_than_failing`, `a_block_cycle_is_refused_with_the_cycle_named` and `an_unknown_policy_is_refused_and_the_known_ones_are_named`. Checks explanations for these cases, not all possible refusals. |
| Lease faces, doctor remedies, children and missing-resource errors | Pass | `cargo test`: `tests/leases_doctor.rs` (including `a_claim_without_a_lease_renders_as_it_always_did` and doctor tests), `tests/children.rs` (`every_face_of_an_item_names_its_children`) and `tests/endpoints.rs` (`a_ledger_without_a_store_says_what_is_missing`, `an_unknown_item_is_not_found_rather_than_silently_ignored`). Assertions cover specific output and behavior, not comprehension. |
| Additional output and refusal regressions named in change history | Pass | `cargo test`: `src/lifecycle.rs` unit tests and `tests/cross_ledger_links.rs`, `tests/imported_history.rs`, `tests/lifecycle_moves.rs`, `tests/lifecycle_version.rs`, `tests/next_state_filter.rs`, `tests/listing_union.rs`, `tests/priority_filter.rs`, `tests/listing_sort.rs`, `tests/dated_replay.rs`, `tests/audit_minors.rs`, `tests/listing_shape.rs` and `tests/text_search.rs`. Current rerun of those tests; historical observations below are not separate current checks. |
| Source review of plain output rules | Pass | Read `Item::line`, `Item::detail` and `Comment::render` in `src/model.rs`, `Selection::plain` in `src/select.rs`, `Lifecycle::plain` in `src/lifecycle.rs` and doctor rendering in `src/endpoints/flight.rs`, on 2026-11-15. Supports the scoped rules above; does not establish every endpoint's wording or accessibility. |
| Item-line priorities, empty-selection plurals and comment/doctor grouping | Pass | Reproduced as failing tests before the fix on 2026-11-15. `cargo test --test faces`: `plain_readability_priority_words_cover_every_value_and_unset`, `plain_readability_empty_selection_uses_the_counts_plural`, `plain_readability_groups_name_their_relationships_without_indentation` and `plain_readability_doctor_remedies_name_the_finding_without_indentation`. Item lines say `priority 0`–`priority 4` or `no priority set`; empty `next` says `1 open item` or `0/2/3 open items`; comment stamps say `comment by … at …:` and doctor remedies name their check and item. Reviewed the three renderers on 2026-11-15; consumer inventory: [plain-face-consumers.md](docs/plain-face-consumers.md). Cargo/rustc 1.99.1. This scoped check does not cover other parenthesized plurals or measure assistive technology behavior. |
| Browser presentation, zoom, reflow, contrast and themes | Blocked | No browser evaluation run. This library emits text, JSON and Turtle rather than a page; evaluation requires a host's rendered interface. No browser or version to report. |
| Manual keyboard | Blocked | No person performed a keyboard pass in a host or terminal for this review. Library tests do not establish keyboard usability. |
| Automated accessibility checker | Blocked | No axe or other accessibility checker run; no rendered host interface was evaluated. Rust tests are not an accessibility checker. |
| Screen reader | Blocked | No screen reader session available or used for this review; pronunciation and reading order remain unverified. |
| Screen magnifier | Blocked | No magnifier session available or used for this review. |
| Speech input | Blocked | No speech-input session available or used for this review. |
| Switch access | Blocked | No switch-access session available or used for this review. |
| Braille display | Blocked | No braille display available or used for this review. |

## Known limitations

Identified by source review on 2026-10-31 and rechecked on 2026-11-15.
Assistive technology effects below are potential barriers, not observed results:

- **Other parenthesized plurals** remain, including listing totals (`3 item(s)`), comment
  counts (`1 comment(s)`) and doctor totals (`1 problem(s)`); punctuation may interrupt
  reading aloud. The empty-selection count now uses a real singular or plural.
- **Long identifiers**: write answers and the `iri:` line carry the item's full IRI, whose
  opaque id can be cumbersome to hear or navigate; the short
  `#12` beside it is the readable handle.
- Ledger clock times are UTC timestamps with milliseconds. Claimed decision times can carry
  a numeric timezone offset. Readers convert these times to their local zone.
- Every message is English only.

## Contributing

Read this file before changing any output listed above: an item's line or detail, a listing,
`next`, a write answer, the doctor or a refusal. Keep meaning in words, keep the line's shape, and
update the golden strings in `tests/faces.rs` on purpose. In the pull request, give each check a
`Check | Result | Evidence` row with Pass, Fail or Blocked, the command or test and tool
version, or why it could not run. Keep unrun checks as Blocked, including every untested
assistive technology. A historical run CI does not repeat stays Blocked with its date;
never present it as a current Pass.

## Reporting a barrier

[Open an issue](https://github.com/ikigai-rs/ikigai-ledger/issues/new?labels=accessibility) with
the `accessibility` label (the link sets it when GitHub lets you add labels; otherwise a
maintainer adds it): what you were doing, what happened, and the host, terminal and any assistive
technology you used, if you are comfortable sharing them. Anything security-sensitive goes through
[private vulnerability reporting](https://github.com/ikigai-rs/ikigai-ledger/security/advisories/new).
**We acknowledge a report within 7 days**, and promise nothing further.

## Change history

Historical implementation notes below describe their dated changes, not current accessibility
verification. The table above records current checks and their limits. No historical
accessibility run is promoted to Pass.

| Date | Change |
|---|---|
| 2026-11-15 | Ledger #3625 expands item-line priorities, uses real plurals in empty `next`, labels comment stamps and names each doctor remedy's check and item. Updated `tests/faces.rs` golden strings deliberately after auditing gonk, starter and watch consumers; JSON, Turtle and machine-read write answers stay stable. New regression tests failed before the fix and pass afterward. No assistive technology was used. |
| 2026-11-12 | Converted the evaluation record to explicit Pass, Fail and Blocked results, reran the named Rust tests on main, and recorded source-review limits and unrun assistive technology checks. |
| 2026-11-11 | Lifecycle loading refuses multiple values or a nonliteral value for a state's name, order, flight flag or drain. Each refusal names the state and field and says to keep or give it one literal value. The unit tests in `src/lifecycle.rs` reproduce the previously accepted documents and pin both new refusal sentences. A recoverable delete racing a cross-ledger link now gives the existing link refusal; `tests/cross_ledger_links.rs` forces the race for blocks and parent links. Existing plain faces are covered by `tests/faces.rs`; no browser, automated accessibility checker or assistive technology was used. |
| 2026-11-14 | Dispatch notes (ledger #3718) add a write answer, a read face, a renewal suffix and refusals, each saying in words the kind of comment, its recipient, its epoch and its grant; no sign alone marks a note. `tests/dispatch_note_gap.rs` pins the text. Existing plain faces are unchanged except that a renewal with unread notes gains its suffix. An acknowledgment (`acks=`) is the holder's; anyone else's is refused in one sentence naming the holder and its principal, saying that acknowledgments are the holder's and that nothing was written. No browser, automated accessibility checker or assistive technology was used. |
| 2026-11-16 | `omit=` and `comments=` on `items` and `next` (ledger #3781) shape the JSON face only; every JSON item gains the `omitted` field. No plain face changed: `tests/faces.rs` passes with only `omitted` added to its JSON literals. New refusals, one sentence each, name the argument and say in words what it accepts (`omit` names body, comments, attribution; `comments` names all, last, none), that `omit=comments` and `comments=last` contradict, or that a shape applies to the JSON face only and nothing was read. `tests/listing_shape.rs` checks them. No browser, automated accessibility checker or assistive technology was used. |
| 2026-11-17 | Text search over bodies and comments (ledger #3803): `match=`, `search=`, `search_mode=` and `whole_word=` on `items`, `count` and `next`. The plain `items` face gains, only under a search of bodies or comments, a line per matching field that names the item and says in words where it matched (`#1 matched in the body:`, `#2 matched in a comment:`), so the line is not tied to its item by indentation alone; the excerpt folds every run of whitespace, newlines included, to one space, so a comment's words cannot make a line of their own. No existing plain face changed: `tests/faces.rs` passes unchanged, and `tests/text_search.rs` pins the new lines and that a read naming none of the new inputs is byte-identical on every face. New refusals, one sentence each, name the argument and what it accepts (`match` names title, body, comments; `search_mode` names all, any; `whole_word` says `true` or `false`). No browser, automated accessibility checker or assistive technology was used. |
| 2026-10-31 | First version. |
| 2026-11-01 | `close closed-at=` (ledger #2819) adds refusals; each names the argument, the reason and the remedy in words. No plain face changed: `tests/faces.rs` passes unchanged. |
| 2026-11-01 | Aliases (ledger #2828) add write answers (`scheme gh owned by …`, `gh:acme/web#12 names acme#12 …`, `… no longer names …`) and refusals, each naming in words the alias, the scheme, its owner or the item that already holds it, and the grant that would do. An alias value appears percent-encoded in an IRI (`acme%2Fweb%2312`), which may be cumbersome when read aloud; the decoded value is what every answer shows. No existing plain face changed: `tests/faces.rs` passes unchanged. |
| 2026-11-01 | Links across ledgers (ledger #2829) add one detail line, `blocked-by: <IRI> (open; in the ledger cli, not readable under your grants)`, a `child:` line of the same shape, the `next` reason `blocked by <IRI> in the ledger cli`, a `⚠ not mirrored:` line a close or reopen adds when a target was beyond its grants, and refusals naming the grant (`urn:cap:ledger:link-in:acme`, the target graph's store grants) and the remedy. The status and whether the reader may read the other ledger are said in words. The blocker is named by its full IRI, which may be cumbersome when read aloud; its number is in the other ledger and is not this ledger's to show. No existing plain face changed: `tests/faces.rs` passes unchanged. |
| 2026-11-01 | An importer's history (ledger #2843) adds a write answer that says in words what was recorded, what was recorded already and that the item did not change, and it avoids a parenthesized plural; a history line for an imported event, marked in words; and refusals, each naming the event, the term or date and what is accepted, and saying nothing was written. Anyone else is told to write the change itself. A value that would break a history's one line per event is refused. The line for an event no importer wrote is unchanged, and so are the other plain faces: `tests/faces.rs` passes unchanged. `tests/imported_history.rs` pins the new line byte for byte; no assistive technology was used. |
| 2026-11-02 | A lifecycle's legal moves (ledger #2863) add a `Conflict` refusal that names in words the lifecycle, the pair, every legal move from `from` (or that there is none) and that nothing was written; refusals at load, one sentence each; and, only for a lifecycle that declares moves, `moves to: …` at the end of each state's line on the plain lifecycle face. A lifecycle with no moves, the built-in included, reads as it did, and the other plain faces did not change: `tests/faces.rs` passes unchanged. `tests/lifecycle_moves.rs` pins the new text byte for byte; no assistive technology was used. |
| 2026-11-02 | A state event's lifecycle version (ledger #2864) adds words to a state change's history line, `lifecycle version 2` or `no lifecycle version recorded (written before 0.18.0)`, before who did it, so that line reads longer; refusals at load, one sentence each, that name the lifecycle and what to add or fix. Other history lines and the other plain faces did not change: `tests/faces.rs` passes unchanged. `tests/lifecycle_version.rs` pins the new text byte for byte; no assistive technology was used. |
| 2026-11-02 | `next drain=` and `next state=` (ledger #2865) add a refusal for each, one sentence that names the value, the lifecycle, every drain or state it accepts and the lifecycle's IRI. A filtered `next` reads as `next` does, with fewer items on it, and a filter that admits nothing says the existing `nothing is ready` sentence. No plain face changed: `tests/faces.rs` passes unchanged. `tests/next_state_filter.rs` pins the refusals byte for byte; no assistive technology was used. |
| 2026-11-06 | A listing across ledgers (ledger #3190) adds `ledgers=` to `items` and `count`. Existing row shapes, page footers and single-ledger faces remain unchanged. A malformed comma-separated name is refused with the `ledgers` argument named and the ledger naming rule in words. `tests/listing_union.rs` checks the union, omitted unreadable ledgers in all three faces, shared filters, global paging, ties, and comments and link numbers from each row's graph. `tests/faces.rs` pins the existing plain and JSON output unchanged. This repository emits text and has no page, themes or browser accessibility checks; the gonk page's integration and checks in all themes belong to its follow-on. No browser, automated accessibility checker or assistive technology was used. |
| 2026-11-06 | Priority filters (ledger #3203) add `priority` and `without_priority` to `items` and `count`, including unions. Refusals name the argument and say in words which values are accepted, including `none (no priority set)`; empty comma entries are refused by the same sentence. Existing faces are unchanged. `tests/priority_filter.rs` checks the new refusal sentences and filter/count parity; `tests/faces.rs` pins existing output. No browser, automated accessibility checker or assistive technology was used. |
| 2026-11-06 | Listing sort (ledger #3192) adds `sort` to `items`. Rows and footers keep their shape. Unknown keys and landing cycles are refused in words with accepted keys or a remedy. `tests/listing_sort.rs` checks every key and direction, ties, paging, cycles, cross-ledger links and capability isolation; `tests/faces.rs` pins existing faces. This text-only library has no page, themes or browser accessibility checks; gonk's follow-on owns those checks in all themes. No browser, automated accessibility checker or assistive technology was used. |
| 2026-11-06 | Dated closes and reopens (ledger #2943) now mark their history time with `(claimed)`, matching other events with source dates; JSON history also includes their commit time. `tests/dated_replay.rs` checks both status events in plain text and JSON and keeps undated status output unchanged. No browser, automated accessibility checker or assistive technology was used. |
| 2026-11-06 | A label with a space (ledger #2634) is refused wherever it would be added, by the label endpoint (argument `content`) or by `append labels=` (argument `labels`, refusing the whole append): one sentence that names the label, the first character outside the grammar in words (`a space`, `a tab`, `a line break`, or the character itself), what a label may hold, and to add each word as its own label. No existing face changes. `tests/audit_minors.rs` checks the refusal on both paths for a space, a tab and `/`, that nothing is stored, and that an old label with a space can still be removed. No browser, automated accessibility checker or assistive technology was used. |
| 2026-11-07 | A close or a reopen in a ledger with many closed items (ledger #3256) lands where it used to be refused by the store's copy bound. It adds one `Conflict` refusal, given only when the item's status changed eight times while the close or reopen was being written: one sentence naming the item, saying in words that the write may not have been recorded, and that the remedy is to read the item and send the close or reopen again. No existing face changes: `tests/faces.rs` passes unchanged. No browser, automated accessibility checker or assistive technology was used. |
| 2026-11-13 | A ledger's sessions (ledger #3647) add a plain face: a first line naming the ledger, the order and the offset in words, or the sentence `acme has no recorded sessions`; the per-session block of an item's sessions, with an `item: acme#1 urn:…` line; and, when more follow, a closing sentence `more sessions follow: read again with offset=50`. It states no count, so it adds no parenthesized plural. The item is named by its short number and full IRI, the IRI cumbersome to hear as noted above. A refusal under the wrong grant names both grants that would do. No existing plain face changed: `tests/faces.rs` passes unchanged. `tests/session_set.rs` checks the first line, the item line, the closing sentence and the empty sentence; no browser, automated accessibility checker or assistive technology was used. |
| 2026-11-13 | The ledger inventory (ledger #3654) adds a JSON face with a schema version and each readable ledger's name, graph IRI, open count and total. It uses the same rows, order and grant checks as the existing plain and Turtle faces. No plain output or refusal wording changes. `tests/ledgers.rs` pins the JSON, capability filtering, empty results and missing-store-grant refusal; `tests/faces.rs` passes unchanged. No browser, automated accessibility checker or assistive technology was used. |
| 2026-11-13 | Session summaries in their own graph (ledger #3661) add the migration's plain answers on `sessions` (`plan`, `move`): sentences stating in words how many summaries, which graphs, and whether anything was written, and one indented line per summary in a plan. Counts are written as numbers with a singular or plural noun as the count needs (`1 session summary`), never a parenthesized plural. Graphs are named by full IRI, cumbersome to hear as noted above. Three refusals are added, each one sentence naming the missing store tokens, the session graph and why it needs them, and for a worker that a grant written before ledger #3661 lacks them: recording a summary, reading sessions, and deleting or purging without the session graph's grants. No existing plain face changed: `tests/faces.rs` passes unchanged. `tests/session_graph.rs` checks the plan, move and nothing-to-move answers and the worker refusal; no browser, automated accessibility checker or assistive technology was used. |

| 2026-11-14 | Removing a label the item does not hold (ledger #3672) now answers `NotFound` with the item and label in words (`#1 has no label worker:pool`), without changing the item or its history. Delete refuses `label=` as `InvalidArgument`, naming `label` and saying to pipe the label text or send it as `content`. Successful removal keeps its existing `unlabeled` sentence. `tests/label_edit.rs` pins these refusals, successful removal, and unchanged item and history on refusal; `tests/write_after_delete.rs` forces two removals to race and checks that only one succeeds and records an event. No browser, automated accessibility checker or assistive technology was used. |
| 2026-11-24 | Request tokens (ledger #4059) add `idempotency-key=` to comment, note, decision, close, reopen, defer and a claim's release. A retry under a token answers the first answer byte for byte, so it reads exactly as the first did and adds no marker. Three refusals are added, each one sentence from ikigai-core naming the token: a malformed token says in words what is wrong and which characters it takes (`character 4 (' ') is not allowed in an idempotency key, which takes ASCII letters, digits and - . _ ~ :`), a token reused for another request says `` `retry-7f3a` was used for a different request ``, and a repeat while the first still runs says the request under that key is still in progress (RetryAfter). No existing plain face changed: `tests/faces.rs` passes unchanged. `tests/idempotency_key.rs` checks the replayed bytes and the refusals. No browser, automated accessibility checker or assistive technology was used. |
