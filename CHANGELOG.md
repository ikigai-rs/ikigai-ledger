# Changelog

## 0.26.4

- A fair claim queue (ledger #4455; the design is `docs/design/fair-claim-queue.md`): `urn:iki:ledger:{ledger}:wait:item:{id}` lists an item's live waiters in order (under `read:`), joins or renews a place (under `claim:` or `write:`; a free item nobody else is offered is claimed at once) and leaves one (with a release's principal rules). A release offers the item to the first live waiter in the same update, and the claim guard refuses anyone else while the offer stands; an offer nobody takes passes to the next waiter, written by the next claim, join or leave, or by the dispatcher's new `sink urn:iki:ledger:{ledger}:offers` (under `close:` or `write:`). `next` leaves an offered item out with `why: "offered"`, and takes `waiter=`. New vocabulary terms: `ledger:WaitPlace`, `ledger:waitsOn`, `ledger:waiter`, `ledger:waiterPrincipal`, `ledger:askedAt`, `ledger:waitSeq`, `ledger:waitExpires`, `ledger:waitPoll`, `ledger:offeredTo`, `ledger:offerExpires`, `ledger:offerAfterEpoch`. `wait` and `offers` are reserved ledger names. Additive: a store written by an older release loads and behaves as before (`tests/claim_queue.rs`, over a 0.26.3 dump), and an empty queue changes nothing.

## 0.26.3

- A `caused-by` link kind: a defect item can name the item that introduced it, in the same ledger or across ledgers (an informational link; it never blocks), with the vocabulary term `ledger:causedBy`, and a close can record its causes (ledger #4337).
- Search: an indexed phrase falls back to the RDF scan when a field holds a character the regex tables do not yet classify, so a phrase next to a newly assigned letter is still found; relevance lets the store tokenize the original query, so a word such as `İstanbul` scores above zero. Face and refusal text are unchanged.

## 0.26.2

- Refuse unreadable optional arguments instead of silently using defaults. Callers that sent an invalid value and previously received the default now receive a refusal. No persisted state changes.
