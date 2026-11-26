# Design: a fair claim queue

Status: **design spike** (ledger #4433). This note changes no ledger behavior and proposes no
public API yet. The prototype is test-only: `tests/fair_claim_queue.rs`.

## The claim, reproduced

A team ran several agent lanes against one ledger, with one item used as a mutex before a
release step. Each lane polled the claim every 120 s. They reported waits of 28 minutes (3
lanes) and 46 minutes with 25 refusals (4 lanes). Two gaps were named: a lane cannot **wait**
for a release, only poll; and even with a wake-up, every waiter **races**, so the lane that
asked first is not served first.

On `main` (`c2d07fb`) both gaps hold, with one correction:

- **Waking.** The kernel's cut feed already wakes a parked waiter on a release. README,
  "Waiting for a held item without repeated claim refusals" (ledger #3711), describes this, and
  `tests/claim_feed_wait.rs` tests it. So the wake gap is a gap in what lanes use, not in what
  the ledger offers. The change-wait of ledger #4112 would make that wake simpler to use. It
  would not change what the wake does.
- **Fairness.** Nothing records waiters or their order. The same README section says: "The
  feed does not provide FIFO acquisition … the first guarded claim to land wins." The
  prototype measures this below as *overtakes*: grants to a lane that asked after another lane
  that was still waiting.

## The prototype

`tests/fair_claim_queue.rs` replays the scenario on a real kernel with a hand-moved clock.
Every claim, release, refusal and epoch is the real ledger's, through `urn:iki:ledger:claim`.
Only the queue is a model: it is an in-test struct standing in for the resource proposed below.

- **The load.** N lanes (3 and 4) each work for 30 to 70 minutes, then hold the item for 8 to
  20 minutes. All lanes start within the first 10 minutes. Each run is 8 hours. Each cell of
  the table is 8 seeded runs, and every number can be reproduced.
- **Poll.** A lane claims, and after a refusal claims again every 120 s.
- **Wake.** An arriving lane reads the item and parks if it is held. A release wakes every
  parked lane within 1 to 5 s, and each one claims. This models the existing feed.
- **Queue.** A lane joins a FIFO queue and renews its place every 60 s. A place lapses after
  180 s without a renewal. On a release, the item is offered to the head, which is woken as in
  Wake. Only the head may claim, and it has 60 s to do so.
- **QueuePolled.** The same queue, but the head is never woken. It sees its offer at its next
  120 s poll, and the offer lasts 150 s.

Run `cargo test --test fair_claim_queue -- --nocapture` to print this table:

| Lanes | Strategy | Windows | Wait p50 (min) | Wait p90 (min) | Wait max (min) | Worst first wait (min) | Refusals | Refusals per window | Overtakes | Idle while wanted, per window (s) | Utilization |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 3 | Poll | 175 | 0.0 | 16.0 | 38.0 | 38.0 | 438 | 2.50 | 7 | 22 | 0.65 |
| 3 | Wake | 176 | 0.0 | 16.4 | 32.1 | 32.1 | 15 | 0.09 | 11 | 1 | 0.62 |
| 3 | Queue | 177 | 0.1 | 13.2 | 33.5 | 33.5 | 0 | 0.00 | 0 | 2 | 0.64 |
| 3 | QueuePolled | 180 | 0.0 | 16.0 | 32.0 | 28.0 | 0 | 0.00 | 0 | 26 | 0.65 |
| 4 | Poll | 222 | 4.0 | 22.0 | 56.0 | 56.0 | 923 | 4.16 | 22 | 34 | 0.79 |
| 4 | Wake | 224 | 4.7 | 24.3 | 47.5 | 47.5 | 50 | 0.22 | 22 | 1 | 0.79 |
| 4 | Queue | 224 | 4.8 | 20.6 | 47.9 | 47.9 | 0 | 0.00 | 0 | 2 | 0.82 |
| 4 | QueuePolled | 224 | 6.0 | 24.0 | 48.0 | 48.0 | 0 | 0.00 | 0 | 40 | 0.81 |

The test checks the properties that hold on every run of this table, not the exact numbers.
Poll refuses. Wake and Queue leave the item idle for at most a few seconds after a release,
against tens of seconds for Poll. Wake still refuses and overtakes. Queue and QueuePolled
refuse no claim and overtake no lane.

What the numbers say:

1. **Waiting works, and it is already available.** Waking removes 90 to 95 percent of the
   refusals (2.50 to 0.09 per window with 3 lanes, 4.16 to 0.22 with 4). It also cuts the idle
   time after a release from 22 to 34 s down to about 1 s. Lanes should use the feed now. This
   needs no new ledger behavior.
2. **Waking does not make access fair.** With a wake, 11 grants (3 lanes) and 22 grants
   (4 lanes) went to a lane that asked later. That is no better than polling. A queue brings
   both refusals and overtakes to zero.
3. **A queue does not fix the long waits.** It trims the 90th-percentile wait by about 3 to
   4 minutes against Wake, but the longest wait stays 33 to 48 minutes, the same as Wake. At
   utilization 0.62 to 0.82, the long waits come from load, not from unfairness. FIFO changes
   who waits; it does not shorten the line. The team's own order of remedies is right: change
   the plan's shape or split the mutex first. A queue makes waiting predictable and quiet; it
   does not make it short. The test does not assert that the queue shortens the longest wait,
   because at this load it does not do so reliably.
4. **Polling under a queue is still correct**: no refusals and no overtakes. It is only slower,
   because each hand-off waits up to a poll interval (26 to 40 s idle per window). It is
   correct only if the offer lasts longer than the poll interval.
5. **Waits measured by polling are overstated.** Poll's idle-while-wanted time (22 to 34 s per
   window) is time when the item was free but nobody knew yet. This is the report's "overstated
   by up to the poll interval", measured.

## Answers

### Waiters as resources

A singular, keyed resource per item: **`urn:iki:ledger:{ledger}:wait:item:{id}`**.

- **Source** lists the live waiters in order. Each entry has its place, its waiter label and
  principal, when it asked, when its place expires, and the standing offer, if there is one.
  It needs `read:{ledger}`, like the item.
- **Sink** joins or renews: `content=<holder> ttl=3m`. Joining takes a place at the end. A
  waiter that is already in the queue (same label and same principal, as for a claim) renews
  its place and keeps it. It needs **`urn:cap:ledger:claim:{ledger}`**, the same grant as
  claiming, because joining is asking for a claim. A closed item refuses with `Conflict`, as a
  claim does.
- **Delete** leaves: `from=<holder>`. The principal rules of release apply: a waiter's own
  principal, or the dispatcher's override (`close:{ledger}` or `write:{ledger}`).
- **In RDF**, each waiter is a node `{item}:wait:{seq}` with `ledger:waitsOn`, `ledger:waiter`,
  `ledger:waiterPrincipal`, `ledger:askedAt` (from the kernel clock), `ledger:waitSeq` and
  `ledger:waitExpires`. `ledger:waitSeq` is a counter per item. It breaks ties at one
  millisecond and never goes down, like the claim epoch.
- **Expiry.** A waiter that stops renewing leaves the queue. Every read and every guard skips
  an entry once its expiry has passed (by the kernel clock). Expired entries are deleted in
  bounded batches, like expired receipts (#153). This is deliberately unlike a lease. An
  expired lease does not free the item, because a lease is authority over work in flight. A
  place in a queue grants no authority, so dropping it takes nothing from anyone.

### On release: offer, not claim

**Offer the item.** Do not claim it for the head outright.

- An outright claim for a head that has gone away would hold the item for a whole lease. Only a
  takeover could undo it. An offer ends at its bound, and in the prototype that costs at most
  60 s (`a_head_that_went_away_costs_at_most_the_offer_bound`).
- An outright claim would be written under the releaser's capability. The principal recorded
  would be the releaser's, not the head's, and the head would learn its epoch only by reading.
  With an offer, the head takes an ordinary claim under its own capability, and the claim
  answer gives it its epoch.
- **The offer is a fact on the item**: `ledger:offeredTo` (the waiter node), `ledger:offerExpires`,
  and `ledger:offerAfterEpoch` (the epoch that was released). A release writes it, in the same
  guarded update, whenever a live waiter exists. While an offer stands, the claim's guarded
  update has one more clause: there is no unexpired offer to another waiter. A plain claim by
  anyone else is then a `Conflict` that names the offer and when it ends. When the queue is
  empty, nothing changes: a free item with no waiters is claimed exactly as it is today.
- **When the head has gone away.** If its place has expired, it is skipped when the offer is
  made. If its place is live but it does not claim within the bound, the offer passes to the
  next waiter, and the head leaves the queue. It may join again at the end. Passing an offer on
  needs a write when nobody is acting. That write comes from the next waiter's claim, whose
  guard works out that the earlier offer has ended. It can also come from the dispatcher's
  scheduled sweep, as `lapse` is written today: `sink …:{ledger}:offers`, under `close:` or
  `write:`.
- **The bound.** A waiter joins with its poll interval (`poll=120s`). The offer lasts that poll
  interval plus a margin, with a floor of 60 s and a ceiling of 5 minutes. QueuePolled shows
  why the offer must outlast the poll.

### Composition with leases, lapse, `next` and the dispatcher

- **Leases and lapse.** These are unchanged, because a queue acts only on a free item. An
  expired lease is still held, so no offer is made. A takeover (`takeover=true from=`) needs a
  held item, and an offer needs a free one, so the two never meet. A `lapse` record does not
  release the item, so it makes no offer.
- **`next`.** An item with a standing offer is left out for everyone except the waiter it is
  offered to, with `why: "offered"`, the waiter, and when the offer ends. That waiter sees the
  item ranked as usual.
- **Fact or policy.** The queue's **order is a fact**: who asked when, and in what sequence.
  The offer is a reservation, which the ledger enforces like it enforces a claim. Which order
  to serve is a **policy**. The ledger enforces one order only: first asked, first offered, and
  only for items that someone has queued on. A dispatcher that wants priority can use its
  override. It can remove waiters, or claim for a lane outright, as it can already release
  anyone's claim. The ledger records that act; it does not judge it. A pluggable order, like
  `next`'s policy, is not proposed until someone needs it.

### Waking

The offer is written in the release's own update, so whatever wakes a waiter on a release also
wakes it on the offer. That can be today's cut feed or the change-wait of ledger #4112. The
woken waiter reads `wait:item:{id}` and sees whether it is offered the item. A wake is a hint,
not proof, as for the feed today. **Polling stays a correct fallback**: a waiter that only polls
still claims in order, as long as the offer lasts longer than its poll (QueuePolled: 0
refusals, 0 overtakes). No push channel is needed.

### Fencing

A hand-off through the queue is an **ordinary grant**. The head's claim sets the epoch to one
more than the previous one, inside the claim's guarded update. The previous holder's writes
with `epoch=` are refused as `Conflict`, and the new holder's are accepted
(`a_queue_hand_off_is_an_ordinary_grant_so_the_old_epoch_is_fenced_out`, over the real
endpoints). Waiting and renewing a place never touch the claim or its epoch, so the epoch a
lane carries into its side effects stays current until its own release. `ledger:offerAfterEpoch`
ties an offer to the release that made it. If any other grant happens first, through the
dispatcher's override, the offer is void, and it cannot be used to take a later epoch.

## Recommendation and follow-ups

1. Use the feed now. It is the biggest gain (most refusals, all the idle time) and needs no
   change.
2. Build the queue as described: `wait:item:{id}`, offers on release, one clause in the claim
   guard, `why: "offered"` in `next`, and the `offers` sweep. Measure again with the real
   resource.
3. In gonk and the starter's lanes, say plainly that a queue makes waiting fair and quiet,
   not short. At the measured loads, splitting the mutex or changing the plan's shape is what
   shortens waits.

Open questions for the build:

- Whether a waiter that let an offer pass should keep its place once instead of leaving.
- Whether `items` and the doctor should show queue length.
- How a waiter joins and claims in one request when the item is already free (today the
  prototype claims at once).
