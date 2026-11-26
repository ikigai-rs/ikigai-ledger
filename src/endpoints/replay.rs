//! **Request tokens** (ledger #4059, from the research note `docs/research/sink-idempotency.md`
//! in ikigai-core, section 5.2): `idempotency-key=` on `comment`, `note`, `decision`, `close`,
//! `reopen`, `defer` and `claim`'s release, and (ledger #4061) the state compare-and-set,
//! `item:{id}:state`, whose record rides in the compare-and-set's own update.
//!
//! Append (ledger #4057) guards the filing and its receipt in the same update, and keeps
//! that receipt independent of the item so that a retry cannot recreate a deleted filing.
//!
//! A client that sent one of these writes and does not know whether it landed (a timeout, a
//! dropped connection) sends it again under the same token. The retry is answered with the
//! FIRST answer, byte for byte, and writes nothing: no second comment, no second note, no
//! second edit event. The contract says so with `Resend::Key("idempotency-key")`, so
//! `ikigai-throttle`'s `Retry` resends it unchanged, and publishes the window as
//! `ik:resendWindow`.
//!
//! The rules, each core's ([`ikigai_core::idempotency`]) and each kept here:
//!
//! - **Scope**: per (endpoint resource, principal, token). `comment`, `note` and `decision`
//!   share one resource, `urn:iki:ledger:{ledger}:comment`, as they share `key=`'s namespace;
//!   each other endpoint has its own. Two principals may use one token without seeing each
//!   other's answer.
//! - **Fingerprint**: `Request::fingerprint_without("idempotency-key")`. A repeat with a
//!   different fingerprint is `InvalidArgument` ("was used for a different request").
//! - **What is recorded**: a success, and a `Conflict`, `NotFound` or `Gone` the endpoint
//!   produced. Never `Denied`, `InvalidArgument`, a limiter or a transient failure: those
//!   wrote nothing, so a retry runs again.
//! - **In flight**: a repeat while the first attempt holds the token is `RetryAfter`.
//! - **Window**: at least 24 hours, configurable with `SpaceConfig::resend_window`. A token
//!   after its window is a new request.
//!
//! ★ **The record lands in the SAME store update as the comment it guards.** The endpoint
//! renders its answer before it writes, and the record's triples ride in the comment's own
//! `INSERT`, under the same `WHERE`: the comment and its record land together or not at all,
//! so a retry can never find the comment without the record and write it again. Never a
//! kernel overlay or the cache, which neither commit with the write nor survive a restart.
//! What the endpoint learns only after the write (a racer took the comment's `key=`, the
//! hand-backs a clearing comment rejected, a refusal) is then [settled](Token::settle) into
//! the record by a second update; that update guards nothing, since the write it describes
//! has already landed or did not happen.
//!
//! An item-bound `ledger:Replay` in the ledger's graph belongs to its item by
//! `ledger:onItem`, so a delete archives it and a purge destroys it with the item: it can hold
//! the comment's text (the JSON face says it), and a purge is the promise that the text is gone.

use std::collections::HashSet;
use std::sync::Mutex;
use std::time::Duration;

use ikigai_core::idempotency::{
    IdempotencyKey, Replay, ReplayScope, IDEMPOTENCY_KEY_ARG, MIN_WINDOW,
};
use ikigai_core::{RequestId, Time};

use super::*;

/// The argument's name, `idempotency-key`: the HTTP header's, so nobody learns two names.
pub(crate) const ARG: &str = IDEMPOTENCY_KEY_ARG;

/// The window a host gets unless it configures one: core's floor, 24 hours.
pub(crate) const DEFAULT_WINDOW: Duration = MIN_WINDOW;

/// How long a repeat that found the first attempt in flight is told to wait.
const IN_FLIGHT_DELAY: Duration = Duration::from_millis(250);

/// The `idempotency-key` input, as each endpoint that takes it declares it.
pub(crate) fn arg(what: &str) -> ArgSpec {
    ArgSpec::new(ARG)
        .summary(format!(
            "A request token for this one attempt (ASCII letters, digits, `-` `.` `_` `~` `:`, at \
             most 256), so a retry after a lost answer is safe: a repeat under the same token, \
             from the same principal, within the window (`ik:resendWindow`) answers the first \
             answer byte for byte and writes nothing, so {what} is recorded once. A repeat with \
             any other argument changed is refused (InvalidArgument), and one while the first \
             is still running is answered RetryAfter. Not the same as `key=`, which names the \
             item or comment itself for as long as it lives."
        ))
        .class(v::ext::XSD_STRING)
        .optional()
}

/// Declare a request token on `action`: the input, resend safety under it, and the window.
pub(crate) fn declare(action: ActionSpec, window: Duration, what: &str) -> ActionSpec {
    action
        .input(arg(what))
        .resend_safe_with_key(ARG)
        .resend_window(window)
}

/// What [`begin`] found.
pub(crate) enum Begin {
    /// The token answered: the first answer, a recorded refusal, or the token's own refusal
    /// (a different request under it). The endpoint returns this and does nothing else.
    Answered(Result<Representation>),
    /// Run the write. `Some` when the call carries a token, which records the answer.
    Run(Option<Token>),
}

/// A request token this attempt holds: where its record lives and what it must match.
pub(crate) struct Token {
    scope: ReplayScope,
    fingerprint: RequestId,
    /// `urn:iki:ledger:{ledger}:replay:{digest}`.
    node: String,
    /// The item the record belongs to, once the endpoint knows it.
    item: Mutex<Option<String>>,
    /// Held while this attempt runs; a repeat that finds it is answered `RetryAfter`.
    _flight: Flight,
}

/// What every endpoint that takes a token shares within one ledger space: the window, and the
/// tokens attempts hold right now.
///
/// ⚠ **In flight is per space.** Two spaces bound over one store (two kernels in one host) do
/// not see each other's attempts, so a repeat that reaches the other one while the first runs
/// is not answered `RetryAfter`. The store's write lock still orders the two writes, and the
/// record's guard is the comment's, so a keyed comment (`key=`) still lands once; an unkeyed
/// one may land twice in that schedule only.
#[derive(Clone)]
pub(crate) struct Tokens {
    pub(crate) window: Duration,
    flights: Arc<Mutex<HashSet<String>>>,
}

impl Tokens {
    pub(crate) fn new(window: Duration) -> Self {
        Tokens {
            window: window.max(MIN_WINDOW),
            flights: Arc::new(Mutex::new(HashSet::new())),
        }
    }
}

/// One attempt's hold on its token, released when the attempt ends however it ends.
struct Flight(Arc<Mutex<HashSet<String>>>, String);

impl Flight {
    fn take(flights: &Arc<Mutex<HashSet<String>>>, name: String) -> Option<Flight> {
        let mut held = flights.lock().unwrap_or_else(|e| e.into_inner());
        if !held.insert(name.clone()) {
            return None;
        }
        drop(held);
        Some(Flight(Arc::clone(flights), name))
    }
}

impl Drop for Flight {
    fn drop(&mut self) {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.1);
    }
}

/// Read the call's token and answer from its record when one stands.
///
/// `endpoint` is the resource the token is scoped to, under this ledger's prefix (`comment`
/// for comment, note and decision). Call it after the capability checks that refuse the
/// caller outright, so a replay is admitted only to a caller that may make the write now, and
/// before anything is read about the item, so a retry is answered the same way whatever has
/// happened to it since.
pub(crate) async fn begin(
    inv: &Invocation<'_>,
    client: &StoreClient<'_, '_>,
    endpoint: &str,
    tokens: &Tokens,
) -> Result<Begin> {
    begin_with(inv, client, endpoint, tokens, false).await
}

/// Append retires an expired receipt only in the update that installs its replacement.
pub(super) async fn begin_append(
    inv: &Invocation<'_>,
    client: &StoreClient<'_, '_>,
    tokens: &Tokens,
) -> Result<Begin> {
    begin_with(inv, client, "append", tokens, true).await
}

async fn begin_with(
    inv: &Invocation<'_>,
    client: &StoreClient<'_, '_>,
    endpoint: &str,
    tokens: &Tokens,
    replace_at_commit: bool,
) -> Result<Begin> {
    let Some(key) = IdempotencyKey::from_request(inv.request, ARG)? else {
        return Ok(Begin::Run(None));
    };
    let now = now_ms(inv)?;
    let ledger = client.ledger();
    let scope = ReplayScope::new(
        format!("{}{endpoint}", ledger.prefix()),
        inv.capability.principal(),
        key,
    );
    let digest = scope.content_id().to_string();
    let digest = digest.strip_prefix("b3:").unwrap_or(&digest);
    let node = format!("{}replay:{digest}", ledger.prefix());
    let Some(flight) = Flight::take(&tokens.flights, format!("{} {node}", ledger.graph())) else {
        return Ok(Begin::Answered(Err(scope
            .key
            .in_flight(Some(IN_FLIGHT_DELAY)))));
    };
    let fingerprint = inv.request.fingerprint_without(ARG);
    let token = Token {
        scope,
        fingerprint,
        node,
        item: Mutex::new(None),
        _flight: flight,
    };
    match token.stored(client).await? {
        Some(record) if record.is_live(Time::from_millis(now), tokens.window) => {
            Ok(Begin::Answered(token.answer(&record)))
        }
        Some(_) if !replace_at_commit => {
            client.update(&token.removal(client)).await?;
            Ok(Begin::Run(Some(token)))
        }
        _ => Ok(Begin::Run(Some(token))),
    }
}

impl Token {
    fn answer(&self, record: &Replay) -> Result<Representation> {
        if !record.matches(self.fingerprint) {
            return Err(self.scope.key.mismatch(ARG));
        }
        match record.representation() {
            Some(repr) => Ok(repr),
            None => Err(record.error().unwrap_or_else(|| {
                Error::Endpoint("a replay record holds neither an answer nor an error".into())
            })),
        }
    }

    /// Read the receipt committed under this token, including a winner in another space.
    pub(super) async fn live_answer(
        &self,
        client: &StoreClient<'_, '_>,
        now: u64,
        window: Duration,
    ) -> Result<Option<Result<Representation>>> {
        Ok(self
            .stored(client)
            .await?
            .filter(|record| record.is_live(Time::from_millis(now), window))
            .map(|record| self.answer(&record)))
    }

    /// Append's DELETE and WHERE fragments: retire an expired receipt at commit, but
    /// never overwrite one still inside its window, even across independent spaces.
    pub(super) fn replacement(
        &self,
        client: &StoreClient<'_, '_>,
        now: u64,
        window: Duration,
    ) -> (String, String) {
        let old = client.in_graph(&format!("<{}> ?receiptP ?receiptO", self.node));
        let recent = now
            .checked_sub(u64::try_from(window.as_millis()).unwrap_or(u64::MAX))
            .map(|cutoff| format!("FILTER(?recorded > {})", datetime(cutoff)))
            .unwrap_or_default();
        let live = client.in_graph(&format!(
            "<{}> <{}> ?recorded . {recent}",
            self.node,
            v::ext::CREATED
        ));
        let guard = format!("FILTER NOT EXISTS {{ {live} }} OPTIONAL {{ {old} }}");
        (old, guard)
    }

    /// Say which item the record belongs to, so it goes with the item on a delete or a purge.
    pub(crate) fn on_item(&self, item: &str) {
        *self.item.lock().unwrap_or_else(|e| e.into_inner()) = Some(item.to_string());
    }

    /// The record of `answer` as triples for the write's own `INSERT` (see the module doc).
    pub(crate) fn triples(&self, answer: &Representation, now: u64) -> String {
        let record = Replay::success(
            self.scope.clone(),
            self.fingerprint,
            answer,
            Time::from_millis(now),
        );
        self.record_triples(&record, now)
    }

    fn record_triples(&self, record: &Replay, now: u64) -> String {
        let mut triples = format!(
            "\n<{node}> <{type_}> <{class}> ; <{record_p}> {hex} ; <{created}> {when} .",
            node = self.node,
            type_ = v::ext::TYPE,
            class = v::REPLAY_CLASS,
            record_p = v::REPLAY_RECORD,
            hex = literal(&hex(&record.to_bytes())),
            created = v::ext::CREATED,
            when = datetime(now),
        );
        if let Some(item) = self
            .item
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_deref()
        {
            triples.push_str(&format!("\n<{}> <{}> <{item}> .", self.node, v::ON_ITEM));
        }
        triples
    }

    /// The record under this token's node, if one decodes. One that does not (written by a
    /// later encoding, or damaged) is treated as absent and replaced by this attempt's.
    async fn stored(&self, client: &StoreClient<'_, '_>) -> Result<Option<Replay>> {
        let rows = client
            .select(&format!(
                "SELECT ?record WHERE {{ {} }}",
                client.in_graph(&format!("<{}> <{}> ?record", self.node, v::REPLAY_RECORD))
            ))
            .await?;
        Ok(rows
            .iter()
            .filter_map(|row| row.get("record"))
            .filter_map(|record| unhex(&record.value))
            .filter_map(|bytes| Replay::from_bytes(&bytes).ok())
            .find(|record| record.scope == self.scope))
    }

    fn removal(&self, client: &StoreClient<'_, '_>) -> String {
        let pattern = format!("<{}> ?p ?o", self.node);
        format!(
            "DELETE {{ {} }} WHERE {{ {} }}",
            client.in_graph(&pattern),
            client.in_graph(&pattern)
        )
    }

    /// Make the record say what this attempt really answered, once it has answered.
    ///
    /// Nothing to do in the common case, where the record the write committed already says
    /// it. Otherwise (the write's guard refused it, a racer's comment answered instead, the
    /// answer read something back after the write) the record is replaced. A refusal that
    /// core does not record (`Denied`, `InvalidArgument`, a transient failure) leaves the
    /// record as it is: when the write landed, its record is the answer a retry should get.
    ///
    /// Never fails the request: the write it describes has landed or has not, and the record
    /// the write committed already stops a retry from writing again. A store error here only
    /// costs a retry the exact first bytes.
    pub(crate) async fn settle(
        self,
        client: &StoreClient<'_, '_>,
        result: &Result<Representation>,
        now: u64,
    ) {
        let at = Time::from_millis(now);
        let record = match result {
            Ok(answer) => Replay::success(self.scope.clone(), self.fingerprint, answer, at),
            Err(error) => match Replay::failure(self.scope.clone(), self.fingerprint, error, at) {
                Some(record) => record,
                None => return,
            },
        };
        let Ok(stored) = self.stored(client).await else {
            return;
        };
        let same = stored.as_ref().is_some_and(|stored| {
            stored.fingerprint == record.fingerprint
                && stored.outcome == record.outcome
                && stored.repr_type == record.repr_type
                && stored.bytes == record.bytes
        });
        if same {
            return;
        }
        let update = batch(&[
            self.removal(client),
            format!(
                "INSERT DATA {{ {} }}",
                client.in_graph(&self.record_triples(&record, now))
            ),
        ]);
        let _ = client.update(&update).await;
    }
}

/// Lowercase hex.
fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[usize::from(byte >> 4)] as char);
        out.push(DIGITS[usize::from(byte & 0xf)] as char);
    }
    out
}

/// [`hex`] back, or `None` for anything that is not lowercase hex of whole bytes.
fn unhex(text: &str) -> Option<Vec<u8>> {
    let digit = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    bytes
        .chunks(2)
        .map(|pair| Some(digit(pair[0])? << 4 | digit(pair[1])?))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips_every_byte() {
        let bytes: Vec<u8> = (0..=255).collect();
        assert_eq!(unhex(&hex(&bytes)).unwrap(), bytes);
        assert!(unhex("abc").is_none());
        assert!(unhex("zz").is_none());
    }

    #[test]
    fn a_held_token_is_refused_until_its_attempt_ends() {
        let flights = Tokens::new(DEFAULT_WINDOW).flights;
        let first = Flight::take(&flights, "g n".into()).expect("free");
        assert!(Flight::take(&flights, "g n".into()).is_none(), "in flight");
        assert!(Flight::take(&flights, "g other".into()).is_some());
        drop(first);
        assert!(Flight::take(&flights, "g n".into()).is_some(), "released");
    }
}
