# Lease time comes from the store

A ledger has one writable store and one lease clock. Use the store evaluator's
`NOW()` on the graph-scoped store door, not the clock of the kernel asking the
question. All doors for the same ledger must reach that same authority, including
readers on other hosts. Do not use independently writable replicas or a
follower's wall clock for lease operations. This is not a replication protocol.

## Expiry and recovery

Grant and renew calculate their deadline from a fresh store time sample.
`next`, `stalled`, `doctor`, the lapse sweep, and takeover use that same
authority. A lease is expired at or after its recorded deadline. The sample
describes the operation as of the store's evaluation, not its arrival at the
client. Time in transit can shorten the usable portion of a lease. Takeover and
lapse writes keep their conditional store updates, so the sample alone cannot
override a claim that has changed in the meantime.

Expiry does not make an item free or invalidate its epoch. The same holder may
renew a claim it still holds. Takeover advances the epoch in the same write and
a release does not reset it. Workers must carry their epoch on fenced writes.
A worker unable to confirm its hold must stop treating it as authority for
external work. Ledger fencing does not protect external systems that do not
check the epoch themselves.

This chooses safety over availability during a partition. If a reader or writer
cannot reach the store, it cannot determine expiry or act on an expiry
decision. It gets the store failure, not a verdict from its own clock. An
offline display may show the last recorded holder, epoch and UTC deadline as
historical data, but must say that lease status is unknown. Online readers show
the store's classification as of the check. They may format UTC timestamps for
display, but must not derive live/expired status from their local clock.

## Caching and older records

Do not transfer a store deadline to the invoking kernel's wall-clock cache.
Lease-sensitive reads bypass that cache and ask the authority again, even if no
triples changed. `next` can still cache answers whose reasons do not depend on
a lease, using its existing local-clock ranking policy horizon. Item details
and lists show recorded claim fields, not an expiry verdict.

The authority clock must be maintained on the store host. Offset between a
client and that host does not affect decisions, but steps in the authority clock
can move decisions early or late relative to elapsed time. Do not fail over to a
different clock while leases are outstanding without preserving its time basis
or reconciling the outstanding claims. Stop old ledger endpoints that still
use observer clocks before serving the new endpoints. Existing lease deadlines
are honored as recorded. Any old grant's skew is already embedded in its
deadline, so inspect or drain those claims during cutover if that skew matters.

New hand-back waits start at the store's time, and the doctor measures their
age there too. Legacy wait timestamps and inferred comment timestamps are
still read as recorded and may reflect an old writer's clock. Their age is
approximate. This does not change authored comment time, history order,
calendar reports, or the clock used by ranking policies.
