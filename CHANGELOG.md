# Changelog

## 0.26.4

- The transition feed, the ledger's outbox (ledger #4116): `source urn:iki:ledger:{ledger}:transition after={n} limit={k} epoch={e}`, under the ledger's read grant. Each filing, claim, release, hand-back, lapse, close and reopen writes an immutable entry `…:transition:seq:{n}` in the same store update as the state it records, numbered from 1 in commit order per ledger, with the item, the kind, the item's claim epoch, the principal of the write and the authored time (for display only). The JSON face has the layer feed's shape, `{ledger, epoch, last, entries}`: `last` is how far the read got, so a follower moves past a gap, and a cursor past the head or from another feed epoch is refused as Conflict. Delete and purge leave the entries whole.
- `transition` is a reserved ledger name.
- Upgrade: a store written before this release starts its feed at head 0 with no backfill of earlier transitions; a ledger already named `transition` can no longer be reached by name.

## 0.26.3

- A `caused-by` link kind: a defect item can name the item that introduced it, in the same ledger or across ledgers (an informational link; it never blocks), with the vocabulary term `ledger:causedBy`, and a close can record its causes (ledger #4337).
- Search: an indexed phrase falls back to the RDF scan when a field holds a character the regex tables do not yet classify, so a phrase next to a newly assigned letter is still found; relevance lets the store tokenize the original query, so a word such as `İstanbul` scores above zero. Face and refusal text are unchanged.

## 0.26.2

- Refuse unreadable optional arguments instead of silently using defaults. Callers that sent an invalid value and previously received the default now receive a refusal. No persisted state changes.
