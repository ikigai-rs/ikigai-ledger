# Design: a dispatch note, from the dispatcher to the claim's holder

Status: **built** (ledger #3718), from this design (ledger #3710), with its two open
questions decided: `note` has no grant of its own, and notes accept `key=`. Built as written
except where noted: the renew answer is plain text only (`claim` has no JSON answer), and its
count is of notes not yet acknowledged (`, 1 unread note`). Related: the
stalled-claim item, which is the main case where a dispatcher needs to say something to a
running worker.

## The gap

A collaborator nudged a stalled agent by typing into its terminal, and that turn was recorded
nowhere. The ledger offers a dispatcher one way to speak about an item while a worker holds it:
`comment`. `tests/dispatch_note_gap.rs` pins what a worker then reads. A comment from the
dispatcher (who holds `close:{ledger}`) and one from an outsider who holds only
`comment:{ledger}` have the same fields in JSON, the same stamp in the plain face and the same
triples in Turtle. Neither records the grant it was written under, who it is for, or which claim
epoch it was meant for. The only difference is the author's IRI, and nothing in the record says
that IRI could dispatch.

So a worker that obeyed comments would obey anyone who can comment. Our workers avoid that by
treating every comment after the dispatch as data. That is correct, but it means a dispatcher
cannot correct a worker mid-item through the ledger at all.

## What a note is

A **dispatch note** is a comment with three extra facts that the ledger checks and records
itself, never taking them from the caller's text:

1. **Who sent it, and that they held the dispatcher grant.** The sender's principal, verified
   by the door, and the grant the write was admitted under.
2. **Who it is for.** The claim it was sent to: the holder's label and principal.
3. **Which claim.** The claim epoch it was sent at. A note for epoch 2 is never addressed to
   the holder at epoch 3, even when the same worker claims again.

Everything else stays as it is for a comment: append-only, stamped, attributed, in the item's
comments and history, moved with the item on a delete.

## Decisions

### Shape: a comment kind, written through its own resource

- **In the graph**: `ledger:DispatchNote rdfs:subClassOf ledger:Comment`, with
  `ledger:noteTo` (the claim principal, or the holder label when the claim has no principal),
  `ledger:noteHolder` (the holder label), `ledger:noteEpoch` (an integer) and
  `ledger:noteGrant` (the grant IRI that admitted the write). Because it is a comment, every
  reader that lists comments already shows it, and none drops it.
- **The write**: a new Sink, `urn:iki:ledger:{ledger}:note item=<id> epoch=<n>`, with the
  text as content. It is not a flag on `comment`. A flag would put the instruction under the
  comment grant's door, where a mistake in one argument check turns every commenter into a
  dispatcher. Its own resource means its own capability row, its own contract and its own
  tests, and `comment` stays exactly as it is. ⚠ Like `sessions`, `note` would become a
  reserved ledger name.
- **Not its own item resource.** A note is about one item and one claim and is read with the
  item. A separate store would be a second record to disagree with the comments.

### Who may send it: the dispatcher grant only

- `note` needs `close:{ledger}` or `write:{ledger}`. That is already what this ledger treats
  as the dispatcher: the grant whose comment clears a hand-back and whose release overrides
  another principal's claim. A worker grant (`read`, `claim`, `comment`) cannot send a note,
  so one worker cannot instruct another, and the collaboration mirror (claim and comment, never
  close) cannot either.
- **A principal is required.** A note with no principal on the capability is refused as
  `Denied`: an unattributed instruction is the record gap this item is about. Root with no
  principal cannot send one either; a host that wants root to dispatch gives root a principal.
- "Signed" means what it means for every write here: the host's door verifies the principal
  (gonk's QUIC door by client key, its HTTP door by passkey), and the ledger records it as
  `verified`. The ledger holds no signatures of its own. If a door later carries a detached
  signature over the note text, it fits as one more property on the note; this design does not
  depend on it.

### Addressing: the current claim, checked inside the write

- `epoch=` is **required**, and the write is refused unless the item is claimed at exactly that
  epoch, by the same clause in the store update that fences a holder's writes today
  (`tests/claim_epoch.rs`). A takeover or release that commits between the dispatcher's read
  and the note's write refuses the note, and **nothing is written**. A note can never land
  addressed to a claim that is gone, so it can never reach a successor it was not meant for.
- The recipient is read in the same update from the claim at that epoch, never from an
  argument. A dispatcher names the epoch; the ledger names the holder.
- A note on a closed or unclaimed item is a `Conflict` that says so, as a fenced write is now.

### How a worker reads it between steps

- **Read face**: `source urn:iki:ledger:{ledger}:item:{id}:notes epoch=<n>`, under
  `read:{ledger}`. It answers the notes for that epoch, oldest first, in text, JSON and
  Turtle. `after=<note iri>` returns only later ones, so a loop polls without re-reading.
- **The heartbeat says when to look.** A worker loop already renews its lease with
  `claim renew=true epoch=<n>`. The renew answer gains a count of that epoch's notes, which costs
  the worker loop nothing new: `#12 renewed by worker-1 at epoch 2 until …, 1 note` in text and
  `"notes": 1` in JSON. The loop reads `notes` only when the count moved.
- **Acknowledgment is recorded too**: `comment item=<id> epoch=<n> acks=<note iri>` writes
  `ledger:acks` on the worker's comment, under the comment grant, fenced by the epoch, and
  only by the holder: a capability that does not act as the claim's principal is refused. The
  dispatcher can then see that the note was read, and if it was not acknowledged by the next
  renewals, it knows the lane is stalled and takes over the expired lease as it does today. That
  is where this meets the stalled-claim item.
- **What the worker loop does with it** belongs to the loop (gonk), not this crate: it hands
  the note to the running agent between steps, marked as the dispatcher's, and keeps the
  rule that every ordinary comment is data. The loop identifies a note by its kind and its
  recorded grant, never by its text.

### What a note can never do

- **It cannot widen the worker's grant.** A note is text with recorded metadata. The ledger
  attaches no capability to it, interprets none of its words, and has no argument on `note`
  that changes a claim, a lease, a label, a state or a grant. The worker's capabilities are
  minted by the worker loop's door when it starts, and nothing the ledger serves can change
  them. A worker loop must present a note as subordinate to its fixed rules: a note may stop,
  redirect or narrow the work inside the dispatched task, and it never authorizes an action
  the worker's grant or rules forbid.
- **It cannot act.** Sending a note never claims, releases, renews, closes, reopens or hands
  back. Those keep their own resources and grants.
- **It cannot be addressed past its claim.** No epoch but the current one, no recipient but
  that claim's holder, and no broadcast to every worker.
- **It cannot be edited or retracted.** Notes are comments, and comments are append-only. A
  dispatcher who changes their mind sends another note.
- **It does not clear a hand-back by itself.** A note needs a current claim; on a handed-back
  item the dispatcher's ordinary comment is still the answer, as it is today.

## Faces (proposed text)

Following `ACCESSIBILITY.md`: the kind and recipient are said in words, never by a sign alone.

```text
2 comment(s):
  2026-11-14T22:40:00.000Z urn:iki:gonk:passkey:dddd (verified)
    note to worker-1 at epoch 2, sent under the dispatcher grant
    Stop and rebase on main first
```

In JSON, a note's comment entry gains `"kind": "note"`, `"to": {"holder", "principal"}`,
`"epoch"` and `"grant"`; an ordinary comment gains `"kind": "comment"` and none of the rest.
In history, a note is an event: `note to worker-1 at epoch 2 by urn:iki:gonk:passkey:dddd`.
The refusals name the item, the epoch asked for and the current claim, as a fenced write's do:
`Conflict: #12 is held by worker-2 at epoch 3, not epoch 2, so the note was not sent`.

## Not decided here

- Whether `note` gets a dedicated grant (`urn:cap:ledger:dispatch:{ledger}`) instead of reusing
  close. Reusing close matches how the ledger already recognizes a dispatcher. A dedicated
  grant would let a host give someone "may instruct workers" without "may close". It can be
  added later with `close` and `write` still covering it, without changing a note's record.
- Keyed notes (`key=`) for resend safety, which would follow `comment key=` exactly.

## Building it

A first PR would add the vocabulary terms and the `note` Sink with its fenced write, and change
`tests/dispatch_note_gap.rs` so it tells a note from a comment. A second would add the `notes`
read face, the count in the renew answer and `acks=`. The worker loop's side goes in gonk.
