# Plain face consumer audit for ledger #3625

External consumers audited before changing production rendering, 2026-11-15. Scope: this repository,
[ikigai-gonk at 097c575](https://github.com/ikigai-rs/ikigai-gonk/tree/097c575b6bcc969a134ac7e39b7630344a1c6c13),
and [ikigai-gonk-starter at 7ba978e](https://github.com/ikigai-rs/ikigai-gonk-starter/tree/7ba978e433ac948bfc4563a4e124f96065bd696f).
Remote source archives were inspected inside the assigned work directory; no other checkout
was used. This is an inventory of the available source, not a claim about private or untracked
operator scripts.

| Line being changed | Consumers and compatibility decision |
|---|---|
| Item row: priority column in detail, listings and ranked `next` | Local `tests/contract_and_paging.rs:titles` parses the title column; the full-suite run exposed its whitespace-token assumption. Updated it in this PR to split the existing double-space column separators. No plain-row parser found in the audited external consumers. Gonk `src/bridge.rs` (`source_json`, `lookup`, `read_json`) supplies JSON to `src/roborev.rs`, `src/kata.rs`, `src/findings.rs` and `src/taker.rs`. `src/queue.rs`, `src/agents.rs` and `src/operator.rs` deserialize JSON. Starter `tools/gonk.py:item_json`, `discover_worker_status_items`, `print_dispatch_rank`, `open_blockers`, and `tools/worker.py` selection reads request JSON. Replace the priority abbreviation with words; leave status, labels, claim suffix and metadata lines intact. |
| Empty `next`: `open item(s) excluded` | No parser found. Starter `tools/gonk.py:cmd_next` displays the plain answer through `show`; automated selection requests `as=application/json`. Use the count's singular or plural. |
| Comment stamp above its body | No plain-stamp parser found. Gonk bridges and starter read comments from JSON (`text`, `time`, attribution and structured note/rejection fields). Starter's dispatch/status/hand-back recognition parses user comment **bodies** from JSON, not rendered stamps; those bodies stay intact. Label the stamp `comment by … at …:`. |
| Doctor remedy line | No plain-remedy parser found. Gonk `web/gonk.xsl` renders the structured remedy. No starter doctor-output parser was found. Repeat the check and item in `remedy for … on …:` so a remedy can be associated with its finding without indentation. |

Searched Rust, Python, JavaScript and shell source for plain markers (`iri:`, `blocks:`,
priority abbreviations, `item(s)`, `comment(s)`, `remedy:`), token splitting, and ledger reads.
The watch inventory is gonk `src/watch.rs`, `examples/watch-cost.rs`,
`examples/dashboard-poll-cost.rs`, starter `tools/worker.py` (including `watch`), and its
`scripts/*.sh` and `worker/*.sh`. These watch file events, poll structured page/count data,
or watch child processes; none parses the four changed lines. No separate ledger-output
watch script was found in either snapshot.

Known machine-read plain lines remain stable: starter `tools/gonk.py:parse_claimed` parses
claim/renew/takeover answers; this repository's test helpers parse the `#N <iri>` append
answer. Historical gonk bridges parsed `iri:`, `blocks:` and status tokens, as documented
in `tests/faces.rs`; those fields keep their spelling. JSON and Turtle contracts do not change.
The exact affected strings in `tests/faces.rs` are updated deliberately, with new regression
checks for every priority, zero/singular/plural exclusions and groups read without indentation.
