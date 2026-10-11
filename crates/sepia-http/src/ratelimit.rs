//! In-memory per-IP sliding-window rate limiting — guards the
//! credential-bootstrapping `POST /api/pair` (the one route that mints
//! a bearer token without prior auth) against online code guessing.
//! No deps: a bounded `HashMap<IpAddr, …>` of attempt timestamps.

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// One window rule: at most `max` hits inside `window`. `max == 0`
/// (or an empty quota set) disables enforcement.
#[derive(Clone, Copy, Debug)]
pub struct Quota {
    pub max: u32,
    pub window: Duration,
}

/// Default cap on tracked IPs — memory stays bounded even when an
/// attacker sprays source addresses; at capacity the idlest entry is
/// evicted (expired-hit entries first, then least-recently-seen).
pub const DEFAULT_MAX_KEYS: usize = 4096;

struct Entry {
    /// Recorded attempt times, oldest → newest; pruned past the
    /// longest quota window.
    hits: VecDeque<Instant>,
    /// Last touch — the eviction tiebreak when the table is full.
    last_seen: Instant,
}

/// A multi-window, per-key limiter. `check` records admitted attempts;
/// rejected ones don't extend the window, so `Retry-After` is honest —
/// a flooding IP drains its quota rather than starving forever.
///
/// `Clone` shares the table (`AppState` clones per request).
#[derive(Clone)]
pub struct RateLimiter {
    quotas: Vec<Quota>,
    max_keys: usize,
    entries: Arc<Mutex<HashMap<IpAddr, Entry>>>,
}

impl RateLimiter {
    /// Enforce `quotas` (sorted internally; empty/all-zero = unlimited).
    /// `max_keys` bounds the per-IP table; clamped to ≥1.
    pub fn new(quotas: impl IntoIterator<Item = Quota>, max_keys: usize) -> Self {
        let mut quotas: Vec<Quota> = quotas
            .into_iter()
            .filter(|q| q.max > 0 && q.window > Duration::ZERO)
            .collect();
        quotas.sort_by_key(|q| q.window);
        Self {
            quotas,
            max_keys: max_keys.max(1),
            entries: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// The `POST /api/pair` quotas: `per_min` attempts/minute plus
    /// `per_day` attempts/24h per client IP (`0` disables each window).
    pub fn pair(per_min: u32, per_day: u32) -> Self {
        Self::new(
            [
                Quota {
                    max: per_min,
                    window: Duration::from_secs(60),
                },
                Quota {
                    max: per_day,
                    window: Duration::from_secs(86_400),
                },
            ],
            DEFAULT_MAX_KEYS,
        )
    }

    /// The longest quota window — hits older than this are dead weight.
    fn horizon(&self) -> Duration {
        self.quotas.last().map_or(Duration::ZERO, |q| q.window)
    }

    /// Record an attempt for `key`: `Ok` admits it, `Err(retry_after)`
    /// is the wait until enough in-window hits age out.
    pub fn check(&self, key: IpAddr) -> Result<(), Duration> {
        self.check_at(key, Instant::now())
    }

    /// The clock-injectable core of [`Self::check`].
    pub fn check_at(&self, key: IpAddr, now: Instant) -> Result<(), Duration> {
        if self.quotas.is_empty() {
            return Ok(());
        }
        let horizon = self.horizon();
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(entry) = entries.get_mut(&key) {
            entry.last_seen = now;
            prune(&mut entry.hits, now, horizon);
            if let Some(wait) = retry_after(&entry.hits, &self.quotas, now) {
                return Err(wait);
            }
            entry.hits.push_back(now);
            return Ok(());
        }
        if entries.len() >= self.max_keys {
            evict(&mut entries, now, horizon, self.max_keys);
        }
        entries.insert(
            key,
            Entry {
                hits: VecDeque::from([now]),
                last_seen: now,
            },
        );
        Ok(())
    }
}

/// Drop hits older than `horizon` — they can't count against any quota.
fn prune(hits: &mut VecDeque<Instant>, now: Instant, horizon: Duration) {
    while hits
        .front()
        .is_some_and(|t| now.duration_since(*t) >= horizon)
    {
        hits.pop_front();
    }
}

/// `None` under every quota; `Some(wait)` when one or more windows are
/// saturated — the longest wait among the violated windows.
fn retry_after(hits: &VecDeque<Instant>, quotas: &[Quota], now: Instant) -> Option<Duration> {
    let mut wait = Duration::ZERO;
    let mut limited = false;
    for quota in quotas {
        // Hits are chronological: find the first still inside this window.
        let first_in = hits
            .iter()
            .position(|t| now.duration_since(*t) < quota.window)
            .unwrap_or(hits.len());
        let count = hits.len() - first_in;
        if count >= quota.max as usize {
            // Admit again once enough of the oldest in-window hits age out.
            let pivot = hits[first_in + (count - quota.max as usize)];
            let w = pivot
                .checked_add(quota.window)
                .map_or(quota.window, |until| until.saturating_duration_since(now));
            wait = wait.max(w);
            limited = true;
        }
    }
    limited.then_some(wait)
}

/// Table at capacity: drop entries with no live hits first; if none are
/// idle, evict the least-recently-seen (LRU-ish). Runs only when an
/// attacker is already spraying keys, so the linear scans are fine.
fn evict(entries: &mut HashMap<IpAddr, Entry>, now: Instant, horizon: Duration, max_keys: usize) {
    entries.retain(|_, e| {
        prune(&mut e.hits, now, horizon);
        !e.hits.is_empty()
    });
    if entries.len() >= max_keys {
        if let Some(oldest) = entries
            .iter()
            .min_by_key(|(_, e)| e.last_seen)
            .map(|(k, _)| *k)
        {
            entries.remove(&oldest);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;

    const MIN: Duration = Duration::from_secs(60);
    const DAY: Duration = Duration::from_secs(86_400);

    fn ip(last: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(10, 0, 0, last))
    }

    #[test]
    fn admits_up_to_max_then_limits() {
        let limiter = RateLimiter::new(
            [Quota {
                max: 2,
                window: MIN,
            }],
            16,
        );
        let now = Instant::now();
        assert!(limiter.check_at(ip(1), now).is_ok());
        assert!(limiter.check_at(ip(1), now).is_ok());
        let err = limiter.check_at(ip(1), now).unwrap_err();
        assert_eq!(err, MIN, "wait until the oldest hit ages out");
    }

    #[test]
    fn window_slides() {
        let limiter = RateLimiter::new(
            [Quota {
                max: 2,
                window: MIN,
            }],
            16,
        );
        let t0 = Instant::now();
        assert!(limiter.check_at(ip(1), t0).is_ok());
        assert!(
            limiter
                .check_at(ip(1), t0 + Duration::from_secs(30))
                .is_ok()
        );
        // Inside the window: limited, retry after the oldest hit exits.
        let err = limiter
            .check_at(ip(1), t0 + Duration::from_secs(45))
            .unwrap_err();
        assert_eq!(err, Duration::from_secs(15));
        // The oldest hit aged out: the window slid.
        assert!(limiter.check_at(ip(1), t0 + MIN).is_ok());
    }

    #[test]
    fn quotas_are_independent() {
        // 5/min AND 20/day — the tighter window limits first.
        let limiter = RateLimiter::new(
            [
                Quota {
                    max: 5,
                    window: MIN,
                },
                Quota {
                    max: 20,
                    window: DAY,
                },
            ],
            16,
        );
        let mut now = Instant::now();
        // Four windows of 5 → 20 hits; the 5th minute is capped by the day.
        for _ in 0..4 {
            for _ in 0..5 {
                assert!(limiter.check_at(ip(1), now).is_ok());
            }
            now += MIN;
        }
        let err = limiter.check_at(ip(1), now).unwrap_err();
        assert!(err > Duration::ZERO && err <= DAY);
    }

    #[test]
    fn keys_are_independent() {
        let limiter = RateLimiter::new(
            [Quota {
                max: 1,
                window: MIN,
            }],
            16,
        );
        let now = Instant::now();
        assert!(limiter.check_at(ip(1), now).is_ok());
        assert!(limiter.check_at(ip(1), now).is_err());
        assert!(limiter.check_at(ip(2), now).is_ok(), "other IPs unaffected");
    }

    #[test]
    fn rejected_attempts_do_not_extend_the_window() {
        let limiter = RateLimiter::new(
            [Quota {
                max: 1,
                window: MIN,
            }],
            16,
        );
        let t0 = Instant::now();
        assert!(limiter.check_at(ip(1), t0).is_ok());
        // Hammering inside the window keeps 429ing without recording —
        // the wait never grows past the original hit's expiry.
        for s in [10u64, 20, 30] {
            let err = limiter
                .check_at(ip(1), t0 + Duration::from_secs(s))
                .unwrap_err();
            assert_eq!(err, MIN.checked_sub(Duration::from_secs(s)).unwrap());
        }
        assert!(limiter.check_at(ip(1), t0 + MIN).is_ok());
    }

    #[test]
    fn table_is_bounded_and_evicts_idle() {
        let limiter = RateLimiter::new(
            [Quota {
                max: 5,
                window: MIN,
            }],
            3,
        );
        let t0 = Instant::now();
        // Distinct last_seen so the LRU pick is deterministic.
        for i in 1..=3u8 {
            assert!(
                limiter
                    .check_at(ip(i), t0 + Duration::from_secs(u64::from(i)))
                    .is_ok()
            );
        }
        // A 4th key at capacity: all entries' hits are live, so the
        // least-recently-seen (key 1) is evicted — its limit resets.
        assert!(limiter.check_at(ip(4), t0 + Duration::from_secs(4)).is_ok());
        let entries = limiter.entries.lock().unwrap();
        assert_eq!(entries.len(), 3);
        assert!(entries.get(&ip(1)).is_none(), "oldest key evicted");
    }

    #[test]
    fn expired_entries_are_evicted_before_live_ones() {
        let limiter = RateLimiter::new(
            [Quota {
                max: 5,
                window: MIN,
            }],
            2,
        );
        let t0 = Instant::now();
        assert!(limiter.check_at(ip(1), t0).is_ok());
        assert!(limiter.check_at(ip(2), t0 + MIN).is_ok());
        // ip(1)'s hits are stale; at capacity it evicts, not live ip(2).
        assert!(limiter.check_at(ip(3), t0 + MIN).is_ok());
        let entries = limiter.entries.lock().unwrap();
        assert!(entries.get(&ip(1)).is_none());
        assert!(entries.get(&ip(2)).is_some());
    }

    #[test]
    fn empty_quotas_mean_unlimited() {
        let limiter = RateLimiter::new([], 1);
        let now = Instant::now();
        for _ in 0..100 {
            assert!(limiter.check_at(ip(1), now).is_ok());
        }
        let zero = RateLimiter::new(
            [Quota {
                max: 0,
                window: MIN,
            }],
            1,
        );
        assert!(zero.check_at(ip(1), now).is_ok());
    }
}
