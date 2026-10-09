//! What a claim carries beyond its holder (ledger #775, step 2): the holder's **kind** and an
//! optional **lease**.
//!
//! # ★ The kind is the host's to stamp, never the caller's
//!
//! A claim is held by a `machine` (a loop, an agent, a client certificate) or a `person`, and
//! the doctor's rules differ by kind: a machine claim on an item that is not in flight is
//! orphaned, while a person holding an item is a deliberate hold. kata-flight encodes the
//! difference in a naming convention (a `kata-ship/` prefix on the owner string), which any
//! writer can spell. Here it is **not an argument at all**: `urn:iki:ledger:{ledger}:claim`
//! declares no `kind` input and ignores one if sent, and the kind is decided by a
//! [`ClaimKindStamper`] the HOST hands to [`crate::SpaceConfig::claim_kind`] — a function of
//! the invocation, so it can read what the host's own door put there (a stamped principal,
//! the capability) and nothing the caller chose.
//!
//! **The default stamps every claim `machine`.** That is the kind the doctor reports on, so a
//! host that never decides cannot let a machine pose as a person to escape it; the cost is
//! that a person's hold on such a host reads as an orphaned machine claim until the host
//! stamps properly. A direct embedder of this crate owns that decision.
//!
//! # A lease is observable, never silent
//!
//! `lease=30m` records `ledger:leaseExpires` from the KERNEL clock (so a test pins expiry
//! without sleeping, and an as-of corridor can ask who held what when). An expired lease does
//! **not** free the item: `next` excludes it with `why: "lease-expired"`, naming the holder
//! and the expiry, and the only way to take it is `claim takeover=true from=<holder>` — a
//! compare-and-set against that holder.

use std::sync::Arc;

use ikigai_core::{Error, Invocation, Result};

use crate::vocabulary as v;

/// Who holds a claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimKind {
    /// A loop, an agent, a client — reported by the doctor when it holds an item that is not
    /// in flight.
    Machine,
    /// A person deliberately holding an item.
    Person,
}

impl ClaimKind {
    /// `machine` or `person` — the word every face uses.
    pub fn as_str(self) -> &'static str {
        match self {
            ClaimKind::Machine => "machine",
            ClaimKind::Person => "person",
        }
    }

    /// The vocabulary individual `ledger:claimKind` holds.
    pub fn iri(self) -> &'static str {
        match self {
            ClaimKind::Machine => v::MACHINE,
            ClaimKind::Person => v::PERSON,
        }
    }

    /// The kind a stored `ledger:claimKind` names.
    pub fn from_iri(iri: &str) -> Option<ClaimKind> {
        match iri {
            i if i == v::MACHINE => Some(ClaimKind::Machine),
            i if i == v::PERSON => Some(ClaimKind::Person),
            _ => None,
        }
    }
}

/// Decides the kind of every claim, from the invocation that takes it. See the module
/// documentation for why this is the host's and not the caller's.
///
/// ```
/// use ikigai_ledger::claim::{ClaimKind, ClaimKindStamper};
/// use std::sync::Arc;
/// // A host whose door stamps a `principal` argument naming a signed-in person:
/// let stamper: ClaimKindStamper = Arc::new(|inv| match inv.inline_str("principal") {
///     Ok(p) if p.starts_with("urn:example:person:") => ClaimKind::Person,
///     _ => ClaimKind::Machine,
/// });
/// # let _ = stamper;
/// ```
pub type ClaimKindStamper = Arc<dyn Fn(&Invocation<'_>) -> ClaimKind + Send + Sync>;

/// The default: every claim is a machine's. See the module documentation.
pub fn machine_by_default() -> ClaimKindStamper {
    Arc::new(|_| ClaimKind::Machine)
}

/// A lease, as asked for and as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lease {
    /// Its length in milliseconds.
    pub millis: u64,
    /// Its length as an `xsd:duration` (`PT1H30M`) — what `ledger:lease` holds and the JSON
    /// face shows.
    pub duration: String,
}

/// The longest lease: a lease is for work in flight, and a year is a hold, not a lease.
pub const MAX_LEASE_MS: u64 = 366 * 86_400_000;

/// Parse a lease: `90s`, `30m`, `2h`, `1d`, or an `xsd:duration` of days, hours, minutes and
/// seconds (`PT30M`, `P1DT2H`). Whole seconds, more than zero, at most [`MAX_LEASE_MS`].
///
/// ```
/// use ikigai_ledger::claim::parse_lease;
/// assert_eq!(parse_lease("30m").unwrap().duration, "PT30M");
/// assert_eq!(parse_lease("90m").unwrap().duration, "PT1H30M");
/// assert_eq!(parse_lease("P1DT2H").unwrap().millis, 26 * 3_600_000);
/// assert_eq!(parse_lease("1d").unwrap().duration, "P1D");
/// assert!(parse_lease("0s").is_err());
/// assert!(parse_lease("soon").is_err());
/// assert!(parse_lease("P1M").is_err()); // a month is not a fixed length
/// ```
pub fn parse_lease(text: &str) -> Result<Lease> {
    let bad = |detail: String| Error::InvalidArgument {
        name: "lease".to_string(),
        detail,
    };
    let text = text.trim();
    let seconds = if let Some(iso) = text.strip_prefix('P') {
        iso_seconds(iso).ok_or_else(|| {
            bad(format!(
                "`{text}` is not a duration of days, hours, minutes and seconds (`PT30M`, \
                 `P1DT2H`); months and years have no fixed length"
            ))
        })?
    } else {
        let split = text
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(text.len());
        let (number, unit) = text.split_at(split);
        let n: u64 = number.parse().map_err(|_| {
            bad(format!(
                "`{text}` is not a lease: `30m`, `2h`, `1d` or `PT30M`"
            ))
        })?;
        let per = match unit {
            "s" => 1,
            "m" => 60,
            "h" => 3_600,
            "d" => 86_400,
            _ => {
                return Err(bad(format!(
                    "`{text}` has no unit this understands: `s`, `m`, `h` or `d`"
                )))
            }
        };
        n.checked_mul(per)
            .ok_or_else(|| bad(format!("`{text}` is too long")))?
    };
    let millis = seconds
        .checked_mul(1000)
        .filter(|ms| *ms > 0 && *ms <= MAX_LEASE_MS)
        .ok_or_else(|| {
            bad(format!(
                "`{text}` is not a lease length this takes: more than zero and at most 366 days"
            ))
        })?;
    Ok(Lease {
        millis,
        duration: iso_duration(seconds),
    })
}

/// `1DT2H30M` (the part after `P`) → seconds.
fn iso_seconds(iso: &str) -> Option<u64> {
    let (days, time) = match iso.split_once('T') {
        Some((d, t)) => (d, Some(t)),
        None => (iso, None),
    };
    let mut total = 0u64;
    let mut take = |part: &str, units: &[(char, u64)]| -> Option<()> {
        let mut rest = part;
        let mut last = 0;
        while !rest.is_empty() {
            let split = rest.find(|c: char| !c.is_ascii_digit())?;
            let (n, tail) = rest.split_at(split);
            let unit = tail.chars().next()?;
            let position = units.iter().position(|(u, _)| *u == unit)?;
            if position < last || n.is_empty() {
                return None;
            }
            last = position + 1;
            total = total.checked_add(n.parse::<u64>().ok()?.checked_mul(units[position].1)?)?;
            rest = &tail[1..];
        }
        Some(())
    };
    take(days, &[('D', 86_400)])?;
    if let Some(time) = time {
        if time.is_empty() {
            return None;
        }
        take(time, &[('H', 3_600), ('M', 60), ('S', 1)])?;
    }
    (!iso.is_empty() && iso != "T").then_some(total)
}

/// Seconds → the canonical `xsd:duration` this crate writes.
fn iso_duration(seconds: u64) -> String {
    let (d, rem) = (seconds / 86_400, seconds % 86_400);
    let (h, m, s) = (rem / 3_600, rem / 60 % 60, rem % 60);
    let mut out = String::from("P");
    if d > 0 {
        out.push_str(&format!("{d}D"));
    }
    if h + m + s > 0 {
        out.push('T');
        for (n, unit) in [(h, 'H'), (m, 'M'), (s, 'S')] {
            if n > 0 {
                out.push_str(&format!("{n}{unit}"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lease_round_trips_through_its_canonical_form() {
        for text in ["PT30M", "PT1H30M", "P1D", "P1DT2H3M4S", "PT45S"] {
            assert_eq!(parse_lease(text).unwrap().duration, text);
        }
        assert_eq!(parse_lease("3600s").unwrap().duration, "PT1H");
    }

    #[test]
    fn a_malformed_or_unbounded_lease_is_refused() {
        for text in [
            "", "P", "PT", "30", "m", "PT30M5H", "P1Y", "-5m", "400d", "PT1.5H",
        ] {
            assert!(parse_lease(text).is_err(), "accepted `{text}`");
        }
    }
}
