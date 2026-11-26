# Changelog

## 0.26.3

- A `caused-by` link kind: a defect item can name the item that introduced it, in the same ledger or across ledgers (an informational link; it never blocks), with the vocabulary term `ledger:causedBy`, and a close can record its causes (ledger #4337).
- Search: an indexed phrase falls back to the RDF scan when a field holds a character the regex tables do not yet classify, so a phrase next to a newly assigned letter is still found; relevance lets the store tokenize the original query, so a word such as `İstanbul` scores above zero. Face and refusal text are unchanged.

## 0.26.2

- Refuse unreadable optional arguments instead of silently using defaults. Callers that sent an invalid value and previously received the default now receive a refusal. No persisted state changes.
