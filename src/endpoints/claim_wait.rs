//! Ephemeral FIFO claim waiters, scoped to one bound ledger space. Durable ownership
//! still comes exclusively from the store's guarded claim update.
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::Duration;

use futures_timer::Delay;
use futures_util::future::{pending, poll_fn, select, Either};
use futures_util::task::AtomicWaker;

use super::*;

pub(super) fn seconds(inv: &Invocation<'_>) -> Result<u64> {
    if !inv.request.args.contains_key("wait") {
        return Ok(0);
    }
    let value = inv.inline_str("wait")?;
    value.parse::<u64>().ok().filter(|n| *n <= 3600).ok_or_else(|| Error::InvalidArgument {
        name: "wait".into(),
        detail: format!("`{value}` is not a whole number of seconds from 0 to 3600; use 0 to refuse immediately or a positive wait bounded by the caller's deadline"),
    })
}

pub(super) fn holder(inv: &Invocation<'_>) -> Result<String> {
    let holder = inv.inline_str("content")?.trim().to_string();
    if holder == "none" || holder == "any" {
        return Err(Error::InvalidArgument {
            name: "content".to_string(),
            detail: format!(
                "`{holder}` is reserved: `holder=none` and `holder=any` are the \
                 items filter's keywords, so work held by `{holder}` could never be \
                 listed by holder"
            ),
        });
    }
    if holder.is_empty() {
        return Err(Error::InvalidArgument {
            name: "content".to_string(),
            detail: "a claim needs a holder: an unattributed claim is \
                     indistinguishable from no claim"
                .to_string(),
        });
    }
    Ok(holder)
}

#[derive(Default)]
struct Slot {
    head: AtomicBool,
    generation: AtomicU64,
    waker: AtomicWaker,
}

impl Slot {
    fn signal(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.waker.wake();
    }
}

type Key = (String, String);

#[derive(Default)]
pub(super) struct Queues {
    items: Mutex<BTreeMap<Key, VecDeque<Arc<Slot>>>>,
}

impl Queues {
    fn join(self: &Arc<Self>, ledger: &Ledger, item: &Item) -> Entry {
        let key = (ledger.name().to_string(), item.iri.clone());
        let slot = Arc::new(Slot::default());
        let mut items = self.items.lock().expect("claim wait queues");
        let queue = items.entry(key.clone()).or_default();
        slot.head.store(queue.is_empty(), Ordering::SeqCst);
        queue.push_back(Arc::clone(&slot));
        Entry {
            queues: Arc::clone(self),
            key,
            slot,
        }
    }

    // Like a graph feed, a mutation is a hint to read again, never an ownership grant.
    // Wake only queue heads; later waiters need no reads until their turn arrives.
    fn changed(&self, ledger: &str) {
        let heads: Vec<_> = self
            .items
            .lock()
            .expect("claim wait queues")
            .iter()
            .filter(|((name, _), _)| name == ledger)
            .filter_map(|(_, queue)| queue.front().cloned())
            .collect();
        for slot in heads {
            slot.signal();
        }
    }

    pub(super) fn wrap<E: Endpoint>(self: &Arc<Self>, inner: E) -> WakeClaims<E> {
        WakeClaims {
            inner,
            queues: Arc::clone(self),
        }
    }
}

struct Entry {
    queues: Arc<Queues>,
    key: Key,
    slot: Arc<Slot>,
}

impl Entry {
    async fn changed(&self, seen: u64) {
        poll_fn(|cx| {
            self.slot.waker.register(cx.waker());
            if self.slot.generation.load(Ordering::SeqCst) != seen {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await
    }
}

impl Drop for Entry {
    fn drop(&mut self) {
        let next = {
            let mut items = self.queues.items.lock().expect("claim wait queues");
            let queue = items.get_mut(&self.key).expect("registered claim waiter");
            let index = queue
                .iter()
                .position(|slot| Arc::ptr_eq(slot, &self.slot))
                .expect("registered claim waiter");
            queue.remove(index);
            let next = if index == 0 {
                queue.front().cloned()
            } else {
                None
            };
            if queue.is_empty() {
                items.remove(&self.key);
            }
            next
        };
        if let Some(next) = next {
            next.head.store(true, Ordering::SeqCst);
            next.signal();
        }
    }
}

/// Wake on release, renewal, takeover, closure, deletion, or cancellation during
/// one of those writes. No lock is held while an endpoint runs or while waking it.
pub(super) struct WakeClaims<E> {
    inner: E,
    queues: Arc<Queues>,
}

struct Notify<'a> {
    queues: &'a Queues,
    ledger: String,
}

impl Drop for Notify<'_> {
    fn drop(&mut self) {
        self.queues.changed(&self.ledger);
    }
}

#[async_trait]
impl<E: Endpoint> Endpoint for WakeClaims<E> {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let _notify = if matches!(inv.request.verb, Verb::Sink | Verb::Delete) {
            Some(Notify {
                queues: &self.queues,
                ledger: Ledger::from_bindings(inv.bindings)?.name().into(),
            })
        } else {
            None
        };
        self.inner.invoke(inv).await
    }

    fn name(&self) -> &str {
        self.inner.name()
    }
    fn describe(&self) -> Description {
        self.inner.describe()
    }
}

impl ClaimEndpoint {
    pub(super) async fn wait_for_claim(
        &self,
        inv: &Invocation<'_>,
        seconds: u64,
    ) -> Result<Representation> {
        let ledger = ledger_for(inv, Need::Claim)?;
        let holder = holder(inv)?;
        // Refuse incompatible or malformed options before joining a queue.
        inv.inline_str("lease")
            .ok()
            .map(crate::claim::parse_lease)
            .transpose()?;
        let renew = parse_flag(inv, "renew")?;
        let takeover = parse_flag(inv, "takeover")?;
        let epoch = parse_epoch(inv)?;
        if renew || takeover || inv.request.args.contains_key("from") || epoch.is_some() {
            return Err(Error::InvalidArgument {
                name: "wait".into(),
                detail: "a positive `wait` takes the next available claim; omit renew, takeover, from, and epoch, or use wait=0 for an immediate claim operation".into(),
            });
        }
        Attribution::committer(inv)?;
        let now = now_ms(inv)?;
        let duration = Duration::from_secs(seconds);
        let duration = inv.time_left().map_or(duration, |left| duration.min(left));
        let bounded = inv.with_not_after(ikigai_core::Time::from_millis(
            now.saturating_add(duration.as_millis() as u64),
        ));
        let client = StoreClient::new(&bounded, ledger);
        let waiting = async {
            let item = require_item(&client, inv.inline_str("item")?).await?;
            // Extending an already owned claim must not queue behind a claimant
            // waiting for that very holder to release it.
            if item.claimed_by.as_deref() == Some(holder.as_str())
                && same_claimant(item.claim_principal.as_deref(), inv.capability.principal())
            {
                match self.invoke_once(&bounded, None).await {
                    Err(Error::Conflict(_)) => {} // ownership changed; join normally
                    result => return result,
                }
            }
            let entry = self.waiters.join(client.ledger(), &item);
            loop {
                // Sample before reading: a release between the read and parking must
                // make changed() ready, even if no waker was registered at release.
                let seen = entry.slot.generation.load(Ordering::SeqCst);
                let mut expires_in = None;
                if entry.slot.head.load(Ordering::SeqCst) {
                    let item = require_item(&client, &item.iri).await?;
                    if !item.open {
                        return Err(closed_refusal(
                            &item,
                            client.ledger(),
                            "closed work is held by nobody",
                        ));
                    }
                    let now = now_ms(&bounded)?;
                    let mine = item.claimed_by.as_deref() == Some(holder.as_str())
                        && same_claimant(
                            item.claim_principal.as_deref(),
                            inv.capability.principal(),
                        );
                    let expired = item.lease_expires.is_some_and(|ends| ends <= now);
                    if item.claimed_by.is_none() || mine || expired {
                        // A positive wait explicitly permits taking the observed expired
                        // holder. The existing compare-and-set still decides who wins.
                        let from = if expired && !mine {
                            item.claimed_by.as_deref()
                        } else {
                            None
                        };
                        match self.invoke_once(&bounded, from).await {
                            Err(Error::Conflict(_)) => continue, // a contender won; read its claim
                            result => return result,
                        }
                    }
                    expires_in = item
                        .lease_expires
                        .map(|ends| Duration::from_millis(ends.saturating_sub(now)));
                }
                let expiry = async {
                    match expires_in {
                        Some(duration) => Delay::new(duration).await,
                        None => pending::<()>().await,
                    }
                };
                let _ = select(Box::pin(entry.changed(seen)), Box::pin(expiry)).await;
            }
        };
        // This timer is created once, so notifications and queue movement never renew
        // the wait budget. Dropping waiting removes its ticket and wakes its successor.
        let result = match select(Box::pin(Delay::new(duration)), Box::pin(waiting)).await {
            Either::Right((result, _)) => result,
            Either::Left(_) => Err(Error::OverBudget {
                detail: "the claim wait reached its wait limit or the caller's deadline; the waiter left the queue, so read the item before trying again".into(),
            }),
        };
        result
    }
}
