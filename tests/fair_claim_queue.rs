//! **Design spike, test-only: a fair claim queue** (ledger #4433). See
//! `docs/design/fair-claim-queue.md`, which this file's measurements answer.
//!
//! Nothing here is ledger API. The queue (`WaitQueue`) is a MODEL of the proposed
//! `urn:iki:ledger:{ledger}:wait:item:{id}` resource, kept in the test; every claim, release,
//! refusal and epoch below is the real ledger's, through the real endpoints, on a kernel whose
//! clock the simulation moves by hand. So a "refusal" is a real `Conflict` from
//! `urn:iki:ledger:claim`, and an epoch is the one the ledger granted.
//!
//! The scenario replays the reported run: N lanes (3 and 4) share one item as a mutex; each
//! lane works, then needs the item for a release step, then works again. Four ways to wait:
//!
//! - `Poll`: try, and on a refusal try again every 120 s (what the team ran);
//! - `Wake`: try once, then park until a release wakes every parked lane, which all race
//!   (the existing cut feed, `tests/claim_feed_wait.rs`, ledger #3711);
//! - `Queue`: join a FIFO queue; on release the item is OFFERED to the head, which alone may
//!   claim it within a bound, woken as in `Wake`;
//! - `QueuePolled`: the same queue, but the head learns of its offer only at its next poll,
//!   showing polling stays a correct (slower) fallback when the offer bound exceeds the poll.
//!
//! `cargo test --test fair_claim_queue -- --nocapture` prints the table the design note quotes.

mod common;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use common::*;
use ikigai_core::{Clock, Error, Fallback, Kernel, Space, Time, Verb};

const CLAIM: &str = "urn:iki:ledger:claim";
const COMMENT: &str = "urn:iki:ledger:comment";
/// The real queue's resource for the one item (ledger #4455).
const WAIT: &str = "urn:iki:ledger:wait:item:1";
const T0: u64 = 1_789_430_400_000;

/// How often a polling lane asks again: the reported run's interval.
const POLL_S: u64 = 120;
/// How long the head has to take an offer when woken.
const OFFER_S: u64 = 60;
/// How long the head has to take an offer when it only polls: above one poll interval.
const OFFER_POLLED_S: u64 = POLL_S + 30;
/// A waiter's entry lapses unless renewed within this; lanes renew every third of it.
const WAIT_TTL_S: u64 = 180;
/// The simulated run.
const RUN_S: u64 = 8 * 3600;
/// Seeds per cell of the table.
const SEEDS: u64 = 8;

// ------------------------------------------------------------------ the ledger, by hand

struct HandClock(Arc<AtomicU64>);

impl Clock for HandClock {
    fn now(&self) -> Time {
        Time::from_millis(self.0.load(Ordering::SeqCst))
    }
}

struct Ledger {
    kernel: Kernel,
    now: Arc<AtomicU64>,
}

impl Ledger {
    fn new() -> Ledger {
        let now = Arc::new(AtomicU64::new(T0));
        let space = Fallback::new(vec![
            Arc::new(ikigai_store::space(
                ikigai_store::DurableStore::in_memory().unwrap(),
            )) as Arc<dyn Space>,
            Arc::new(ikigai_ledger::space()) as Arc<dyn Space>,
        ]);
        let kernel = Kernel::new(Arc::new(space)).with_clock(Arc::new(HandClock(now.clone())));
        append(&kernel, "Release mutex", &[]);
        Ledger { kernel, now }
    }

    fn at(&self, t: u64) {
        self.now.store(T0 + t * 1000, Ordering::SeqCst);
    }

    /// A real claim. `Ok(epoch)`, or `Err(())` for the ledger's `Conflict`.
    fn claim(&self, lane: &str) -> std::result::Result<u64, ()> {
        match try_verb(
            &self.kernel,
            Verb::Sink,
            CLAIM,
            &[("item", "1"), ("content", lane), ("lease", "30m")],
        ) {
            Ok(answer) => Ok(epoch_of(&answer)),
            Err(Error::Conflict(_)) => Err(()),
            Err(other) => panic!("claim by {lane}: {other}"),
        }
    }

    /// A real compare-and-set release, fenced by the epoch the lane carries.
    fn release(&self, lane: &str, epoch: u64) {
        let epoch = epoch.to_string();
        delete(
            &self.kernel,
            CLAIM,
            &[("item", "1"), ("from", lane), ("epoch", &epoch)],
        );
    }
}

impl Ledger {
    /// The real queue (ledger #4455): join or renew a place. `Ok(Some(epoch))` when the join
    /// claimed the item at once (it was free and nobody else was offered it), `Ok(None)` for a
    /// place, `Err(())` for a `Conflict`.
    fn join(&self, lane: &str, poll: Option<&str>) -> std::result::Result<Option<u64>, ()> {
        let mut args = vec![("content", lane), ("ttl", "3m")];
        if let Some(poll) = poll {
            args.push(("poll", poll));
        }
        match try_verb(&self.kernel, Verb::Sink, WAIT, &args) {
            Ok(answer) if answer.contains(" claimed by ") => Ok(Some(epoch_of(&answer))),
            Ok(_) => Ok(None),
            Err(Error::Conflict(_)) => Err(()),
            Err(other) => panic!("join by {lane}: {other}"),
        }
    }

    /// Who the real queue says the item is offered to, as a woken lane reads it.
    fn offered_to(&self) -> Option<String> {
        let text = source(&self.kernel, WAIT, &[("as", "application/json")]);
        let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
        doc["offer"]["waiter"].as_str().map(str::to_string)
    }
}

fn epoch_of(answer: &str) -> u64 {
    answer
        .split("at epoch ")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|n| {
            n.trim_end_matches(|c: char| !c.is_ascii_digit())
                .parse()
                .ok()
        })
        .unwrap_or_else(|| panic!("no epoch in {answer:?}"))
}

// ------------------------------------------------------------------ the queue, modeled

/// One waiter: who, when it asked (its place), and when its entry lapses unless renewed.
#[derive(Clone, Debug)]
struct Waiter {
    lane: usize,
    asked: u64,
    expires: u64,
}

/// The proposed `wait:item:{id}` resource, in memory. Order is by `asked`, a fact; an
/// offer is a reservation of the next claim for the head, until the time it holds.
#[derive(Default)]
struct WaitQueue {
    waiters: VecDeque<Waiter>,
    offer: Option<(usize, u64)>,
}

impl WaitQueue {
    fn join(&mut self, lane: usize, t: u64) {
        // A place is taken at the end, so the queue stays in the order its waiters asked.
        assert!(self.waiters.back().is_none_or(|w| w.asked <= t));
        if !self.waiters.iter().any(|w| w.lane == lane) {
            self.waiters.push_back(Waiter {
                lane,
                asked: t,
                expires: t + WAIT_TTL_S,
            });
        }
    }

    fn renew(&mut self, lane: usize, t: u64) {
        if let Some(w) = self.waiters.iter_mut().find(|w| w.lane == lane) {
            w.expires = t + WAIT_TTL_S;
        }
    }

    fn leave(&mut self, lane: usize) {
        self.waiters.retain(|w| w.lane != lane);
        if matches!(self.offer, Some((l, _)) if l == lane) {
            self.offer = None;
        }
    }

    /// Drop lapsed waiters, and pass a lapsed offer on. Returns the lane newly offered to.
    fn sweep(&mut self, t: u64, free: bool, bound: u64) -> Option<usize> {
        self.waiters.retain(|w| w.expires > t);
        if let Some((lane, until)) = self.offer {
            if until <= t || !self.waiters.iter().any(|w| w.lane == lane) {
                // The head let the offer pass, or went away: it leaves the queue.
                self.waiters.retain(|w| w.lane != lane);
                self.offer = None;
            }
        }
        if free && self.offer.is_none() {
            if let Some(head) = self.waiters.front() {
                self.offer = Some((head.lane, t + bound));
                return Some(head.lane);
            }
        }
        None
    }

    /// The claim's guard, as the design puts it in the claim's own update: while an offer
    /// stands, only its lane may take the item.
    fn may_claim(&self, lane: usize, t: u64) -> bool {
        match self.offer {
            Some((l, until)) => l == lane && t < until,
            None => false,
        }
    }
}

// ------------------------------------------------------------------ the simulation

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Strategy {
    Poll,
    Wake,
    Queue,
    QueuePolled,
    /// The REAL queue (ledger #4455): `urn:iki:ledger:wait:item:1`, renewed every 60 s; the
    /// release writes the offer; the woken lane reads the queue and claims if it is offered.
    Resource,
    /// The real queue, never woken: each lane reads the queue at its 120 s poll and joins with
    /// `poll=120s`, so its offer lasts 150 s.
    ResourcePolled,
}

#[derive(Clone, Copy, Debug)]
enum State {
    Working { until: u64 },
    Waiting { since: u64, next_try: u64 },
    Holding { until: u64, epoch: u64 },
}

/// A small deterministic generator, so every number in the note can be reproduced.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
    fn between(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next() % (hi - lo + 1)
    }
}

#[derive(Default, Debug)]
struct Outcome {
    /// Seconds from asking to holding, one per window.
    waits: Vec<u64>,
    /// The first window's wait, per lane.
    first_waits: Vec<u64>,
    /// Real `Conflict`s from the claim endpoint.
    refusals: usize,
    /// Grants to a lane that asked after another lane still waiting.
    inversions: usize,
    /// Seconds the item sat free while someone was waiting.
    idle_while_wanted: u64,
    held: u64,
}

const NEVER: u64 = u64::MAX;

fn run(strategy: Strategy, lanes: usize, seed: u64) -> Outcome {
    let ledger = Ledger::new();
    let mut rng = Rng(seed * 7919 + lanes as u64);
    let names: Vec<String> = (0..lanes).map(|i| format!("lane-{i}")).collect();
    // Everyone starts near the same time, as in the reported run.
    let mut state: Vec<State> = (0..lanes)
        .map(|_| State::Working {
            until: rng.between(0, 600),
        })
        .collect();
    let mut queue = WaitQueue::default();
    let mut out = Outcome::default();
    let mut first_done = vec![false; lanes];
    let mut holder: Option<usize> = None;
    let bound = if strategy == Strategy::QueuePolled {
        OFFER_POLLED_S
    } else {
        OFFER_S
    };
    let queued = matches!(strategy, Strategy::Queue | Strategy::QueuePolled);

    for t in 0..RUN_S {
        ledger.at(t);
        // Lanes act in a different order each second: no lane is favored by its index.
        let mut order: Vec<usize> = (0..lanes).collect();
        for i in (1..lanes).rev() {
            order.swap(i, (rng.next() as usize) % (i + 1));
        }
        for &i in &order {
            match state[i] {
                State::Holding { until, epoch } if t >= until => {
                    ledger.release(&names[i], epoch);
                    holder = None;
                    state[i] = State::Working {
                        until: t + rng.between(30 * 60, 70 * 60),
                    };
                    match strategy {
                        Strategy::Poll => {}
                        // A release wakes every parked lane; each reacts within seconds.
                        Strategy::Wake => {
                            for lane in state.iter_mut() {
                                if let State::Waiting { since, .. } = *lane {
                                    *lane = State::Waiting {
                                        since,
                                        next_try: t + rng.between(1, 5),
                                    };
                                }
                            }
                        }
                        Strategy::Queue | Strategy::QueuePolled => {}
                        // The release wrote the offer; the lane it names is woken.
                        Strategy::Resource => {
                            if let Some(lane) = ledger.offered_to() {
                                let head = names.iter().position(|n| *n == lane).unwrap();
                                if let State::Waiting { since, .. } = state[head] {
                                    state[head] = State::Waiting {
                                        since,
                                        next_try: t + rng.between(1, 5),
                                    };
                                }
                            }
                        }
                        Strategy::ResourcePolled => {}
                    }
                }
                State::Working { until } if t >= until => {
                    state[i] = State::Waiting {
                        since: t,
                        next_try: t,
                    };
                    if queued {
                        queue.join(i, t);
                    }
                }
                _ => {}
            }
        }

        if queued {
            for &i in &order {
                if let State::Waiting { since, .. } = state[i] {
                    if (t - since) % (WAIT_TTL_S / 3) == 0 {
                        queue.renew(i, t);
                    }
                }
            }
            if let Some(head) = queue.sweep(t, holder.is_none(), bound) {
                if let State::Waiting { since, next_try } = state[head] {
                    let learns = match strategy {
                        // Woken: within seconds.
                        Strategy::Queue => t + rng.between(1, 5),
                        // Not woken: at its next poll, which has not moved.
                        _ => {
                            if next_try >= t && next_try != NEVER {
                                next_try
                            } else {
                                t + POLL_S - ((t - since) % POLL_S)
                            }
                        }
                    };
                    state[head] = State::Waiting {
                        since,
                        next_try: learns,
                    };
                }
            }
        }

        for &i in &order {
            let State::Waiting { since, next_try } = state[i] else {
                continue;
            };
            let real = matches!(strategy, Strategy::Resource | Strategy::ResourcePolled);
            let poll = (strategy == Strategy::ResourcePolled).then_some("120s");
            let attempt = if real && since == t {
                // Arriving: a join, which claims a free item nobody else is offered.
                Some(ledger.join(&names[i], poll).map(|e| e.ok_or(())))
            } else if real && t >= next_try {
                // Woken, or at its poll: read the queue, and claim only if offered.
                if ledger.offered_to().as_deref() == Some(names[i].as_str()) {
                    Some(ledger.claim(&names[i]).map_err(|()| ()).map(Ok))
                } else {
                    state[i] = State::Waiting {
                        since,
                        next_try: if poll.is_some() { t + POLL_S } else { NEVER },
                    };
                    None
                }
            } else if real && (t - since) % 60 == 0 {
                // The renewal is a join: on an item offered to this lane, it claims.
                Some(ledger.join(&names[i], poll).map(|e| e.ok_or(())))
            } else {
                None
            };
            if real {
                match attempt {
                    None => {}
                    // A place taken or renewed: wait (to be woken, or to the next poll).
                    Some(Ok(Err(()))) => {
                        if since == t {
                            state[i] = State::Waiting {
                                since,
                                next_try: if poll.is_some() { t + POLL_S } else { NEVER },
                            };
                        }
                    }
                    Some(Err(())) => {
                        out.refusals += 1;
                        state[i] = State::Waiting {
                            since,
                            next_try: if poll.is_some() { t + POLL_S } else { NEVER },
                        };
                    }
                    Some(Ok(Ok(epoch))) => {
                        let wait = t - since;
                        out.waits.push(wait);
                        if !first_done[i] {
                            first_done[i] = true;
                            out.first_waits.push(wait);
                        }
                        out.inversions += state
                            .iter()
                            .filter(
                                |s| matches!(s, State::Waiting { since: other, .. } if *other < since),
                            )
                            .count();
                        let hold = rng.between(8 * 60, 20 * 60);
                        out.held += hold.min(RUN_S - t);
                        state[i] = State::Holding {
                            until: t + hold,
                            epoch,
                        };
                        holder = Some(i);
                    }
                }
                continue;
            }
            if t < next_try {
                continue;
            }
            if strategy == Strategy::Wake && since == t && holder.is_some() {
                // Arriving, a waking lane reads first and parks without a claim; once woken
                // it claims straight away, and that is where the race is.
                state[i] = State::Waiting {
                    since,
                    next_try: NEVER,
                };
                continue;
            }
            if queued && !queue.may_claim(i, t) {
                // Not offered: a queued lane does not race. A polled lane keeps its schedule.
                state[i] = State::Waiting {
                    since,
                    next_try: if strategy == Strategy::QueuePolled {
                        t + POLL_S
                    } else {
                        NEVER
                    },
                };
                continue;
            }
            match ledger.claim(&names[i]) {
                Ok(epoch) => {
                    let wait = t - since;
                    out.waits.push(wait);
                    if !first_done[i] {
                        first_done[i] = true;
                        out.first_waits.push(wait);
                    }
                    out.inversions += state
                        .iter()
                        .filter(
                            |s| matches!(s, State::Waiting { since: other, .. } if *other < since),
                        )
                        .count();
                    let hold = rng.between(8 * 60, 20 * 60);
                    out.held += hold.min(RUN_S - t);
                    state[i] = State::Holding {
                        until: t + hold,
                        epoch,
                    };
                    holder = Some(i);
                    if queued {
                        queue.leave(i);
                    }
                }
                Err(()) => {
                    out.refusals += 1;
                    state[i] = State::Waiting {
                        since,
                        next_try: match strategy {
                            Strategy::Poll => t + POLL_S,
                            Strategy::Wake => NEVER,
                            Strategy::Queue | Strategy::QueuePolled => {
                                panic!("an offered claim was refused at {t}")
                            }
                            Strategy::Resource | Strategy::ResourcePolled => unreachable!(),
                        },
                    };
                }
            }
        }

        if holder.is_none() && state.iter().any(|s| matches!(s, State::Waiting { .. })) {
            out.idle_while_wanted += 1;
        }
    }
    out
}

fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = ((p * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
    sorted[rank - 1]
}

#[derive(Debug)]
struct Row {
    strategy: Strategy,
    lanes: usize,
    windows: usize,
    p50: u64,
    p90: u64,
    max: u64,
    worst_first: u64,
    refusals: usize,
    inversions: usize,
    idle_per_window: u64,
    utilization: f64,
}

fn measure(strategy: Strategy, lanes: usize) -> Row {
    let mut waits = Vec::new();
    let mut worst_first = 0;
    let (mut refusals, mut inversions, mut idle, mut held) = (0, 0, 0, 0);
    for seed in 0..SEEDS {
        let o = run(strategy, lanes, seed);
        waits.extend(o.waits);
        worst_first = worst_first.max(o.first_waits.into_iter().max().unwrap_or(0));
        refusals += o.refusals;
        inversions += o.inversions;
        idle += o.idle_while_wanted;
        held += o.held;
    }
    waits.sort_unstable();
    Row {
        strategy,
        lanes,
        windows: waits.len(),
        p50: percentile(&waits, 0.5),
        p90: percentile(&waits, 0.9),
        max: *waits.last().unwrap(),
        worst_first,
        refusals,
        inversions,
        idle_per_window: idle / waits.len() as u64,
        utilization: held as f64 / (RUN_S * SEEDS) as f64,
    }
}

fn mins(s: u64) -> String {
    format!("{:.1}", s as f64 / 60.0)
}

#[test]
fn replay_the_reported_run_poll_wake_and_queue() {
    let mut rows = Vec::new();
    for lanes in [3, 4] {
        for strategy in [
            Strategy::Poll,
            Strategy::Wake,
            Strategy::Queue,
            Strategy::QueuePolled,
            Strategy::Resource,
            Strategy::ResourcePolled,
        ] {
            rows.push(measure(strategy, lanes));
        }
    }
    println!(
        "| lanes | strategy | windows | wait p50 (min) | wait p90 (min) | wait max (min) | worst first wait (min) | refusals | refusals per window | overtakes | idle while wanted per window (s) | utilization |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|---|---|");
    for r in &rows {
        println!(
            "| {} | {:?} | {} | {} | {} | {} | {} | {} | {:.2} | {} | {} | {:.2} |",
            r.lanes,
            r.strategy,
            r.windows,
            mins(r.p50),
            mins(r.p90),
            mins(r.max),
            mins(r.worst_first),
            r.refusals,
            r.refusals as f64 / r.windows as f64,
            r.inversions,
            r.idle_per_window,
            r.utilization,
        );
    }

    for lanes in [3, 4] {
        let row = |s| {
            rows.iter()
                .find(|r| r.lanes == lanes && r.strategy == s)
                .unwrap()
        };
        let (poll, wake, queue, polled) = (
            row(Strategy::Poll),
            row(Strategy::Wake),
            row(Strategy::Queue),
            row(Strategy::QueuePolled),
        );
        let (resource, resource_polled) = (row(Strategy::Resource), row(Strategy::ResourcePolled));
        // The load is the reported one: the team's 0.7 utilization rule fires.
        assert!(poll.utilization > 0.6, "{poll:?}");
        // Polling refuses, and leaves the item idle up to a poll after each release.
        assert!(poll.refusals > 0, "{poll:?}");
        assert!(
            poll.idle_per_window > 10 * wake.idle_per_window,
            "{poll:?} {wake:?}"
        );
        // A wake closes the idle gap, but everyone races again: refusals and overtakes.
        assert!(wake.idle_per_window <= 5, "{wake:?}");
        assert!(wake.refusals > 0 && wake.inversions > 0, "{wake:?}");
        // The queue: nobody is refused and nobody is overtaken, with a wake's idle gap.
        assert_eq!((queue.refusals, queue.inversions), (0, 0), "{queue:?}");
        assert!(queue.idle_per_window <= 5, "{queue:?}");
        // ⚠ No assertion that the queue shortens the longest wait: at this load it does not
        // reliably (see the note). Order removes refusals and overtakes, not load.
        // Polling under a queue is still correct (no refusals, no overtakes), only slower.
        assert_eq!((polled.refusals, polled.inversions), (0, 0), "{polled:?}");
        assert!(polled.idle_per_window <= POLL_S, "{polled:?}");
        // ★ The real resource (ledger #4455) keeps the model's promise: no refusal, no
        // overtake, and a wake's idle gap; polled, still no refusal and no overtake.
        assert_eq!(
            (resource.refusals, resource.inversions),
            (0, 0),
            "{resource:?}"
        );
        assert!(resource.idle_per_window <= 5, "{resource:?}");
        assert_eq!(
            (resource_polled.refusals, resource_polled.inversions),
            (0, 0),
            "{resource_polled:?}"
        );
        assert!(
            resource_polled.idle_per_window <= POLL_S,
            "{resource_polled:?}"
        );
    }
}

// ------------------------------------------------------------------ the hand-off's edges

#[test]
fn a_queue_hand_off_is_an_ordinary_grant_so_the_old_epoch_is_fenced_out() {
    let ledger = Ledger::new();
    let mut queue = WaitQueue::default();
    ledger.at(0);
    let a = ledger.claim("lane-a").unwrap();
    queue.join(1, 10);
    // Waiting renews only the entry: the holder's epoch is untouched.
    queue.renew(1, 60);
    let current = source(
        &ledger.kernel,
        CLAIM,
        &[("item", "1"), ("epoch", &a.to_string())],
    );
    assert!(current.contains("current"), "{current}");

    ledger.at(120);
    ledger.release("lane-a", a);
    assert_eq!(queue.sweep(120, true, OFFER_S), Some(1));
    assert!(
        !queue.may_claim(2, 121),
        "a lane not at the head may not take the offer"
    );
    let b = ledger.claim("lane-b").unwrap();
    assert_eq!(
        b,
        a + 1,
        "the hand-off is a grant: the epoch counts up by one"
    );

    // The old holder, waking late, is fenced out of every write it carries the old epoch to.
    let stale = try_verb(
        &ledger.kernel,
        Verb::Sink,
        COMMENT,
        &[
            ("item", "1"),
            ("epoch", &a.to_string()),
            ("content", "released v1"),
        ],
    );
    assert!(matches!(stale, Err(Error::Conflict(_))), "{stale:?}");
    let fresh = try_verb(
        &ledger.kernel,
        Verb::Sink,
        COMMENT,
        &[
            ("item", "1"),
            ("epoch", &b.to_string()),
            ("content", "released v2"),
        ],
    );
    assert!(fresh.is_ok(), "{fresh:?}");
}

#[test]
fn a_head_that_went_away_costs_at_most_the_offer_bound() {
    let mut queue = WaitQueue::default();
    queue.join(0, 0); // will stop renewing
    queue.join(1, 5);
    queue.join(2, 9); // will take the offer late

    // The item is held; lane 0 stops renewing and lapses out of the queue unseen.
    queue.renew(1, 150);
    queue.renew(2, 150);
    assert_eq!(queue.sweep(181, false, OFFER_S), None);
    assert_eq!(queue.waiters.front().map(|w| w.lane), Some(1));

    // On release lane 1 is offered, keeps renewing its entry, but never claims.
    queue.renew(1, 290);
    queue.renew(2, 290);
    assert_eq!(queue.sweep(300, true, OFFER_S), Some(1));
    assert!(queue.may_claim(1, 359) && !queue.may_claim(2, 359));
    // At the bound the offer passes on, and lane 1 has left the queue.
    assert_eq!(queue.sweep(360, true, OFFER_S), Some(2));
    assert!(queue.may_claim(2, 361));
    assert!(!queue.waiters.iter().any(|w| w.lane == 1));
}
