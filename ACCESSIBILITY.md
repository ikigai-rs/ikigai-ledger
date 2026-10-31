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
| Last reviewed | 2026-10-31 |

## What it emits that people read

This is a library: the ledger's endpoints, which a host binds. Its default face is plain text
that people read in a terminal, and it also answers JSON (`as=application/json`) and Turtle.

- **An item's line and detail** (`Item::line` and `Item::detail` in `src/model.rs`): one fixed
  line per item, `#12  open    p1  Title  [labels]  claimed:holder`, and under it, in the
  detail, labeled lines (`iri:`, `filed:`, `updated:`, `about:`, `blocks:`, `child:` …), the
  body, and the comments, each stamped with its time and author (`Comment::render`).
- **Listings** (`items`, `ledgers`, `count`): item lines and a total (`3 item(s)`); `count`
  answers a bare number.
- **`urn:iki:ledger:next`** (`Selection::plain` in `src/select.rs`): ranked item lines, each
  with its score and its reasons in words, then the policy and what was excluded and why
  (`not #1: blocked by #2`), or `nothing is ready (…)` with the reasons.
- **Write answers** (`src/endpoints.rs`): one sentence each, such as
  `#1 urn:iki:ledger:default:item:…`, `commented on #1 (…)`, `linked #1 blocks #2`,
  `closed #2 (done)`, `#1 claimed by worker-1`.
- **The doctor and lifecycles**: the doctor's findings, each with a `remedy:` line; a
  lifecycle's states, one per line (`Lifecycle::plain` in `src/lifecycle.rs`).
- **Refusals**: `InvalidArgument` naming the argument with a sentence that says what is
  accepted (`` `x` is not one of open, closed, all ``, `` `9` is not 0 (highest) … 4 (lowest) ``),
  `NotFound` for an unknown item, and errors that name what is missing (a store, a clock).

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
