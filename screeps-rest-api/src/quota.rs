//! Official-server (screeps.com) per-endpoint quota model.
//!
//! screeps.com enforces TWO layers of rate limiting on auth tokens
//! (<https://docs.screeps.com/auth-tokens.html>):
//!
//! 1. a GLOBAL cap of 120 requests/minute, and
//! 2. PER-ENDPOINT hourly/daily quotas (e.g. `POST /api/game/map-stats`:
//!    60/hour).
//!
//! The client's courtesy `min_delay` covers (1); this module models (2).
//! Pacing at 600 ms satisfies the global cap but burns a 60/hour
//! endpoint quota in 36 s — after which EVERY further call to that
//! endpoint 429s until the window resets (observed live: a
//! `scan --all` died with "Rate limit exceeded, retry after 3548835ms"
//! ≈ 59 min, a full window's wait).
//!
//! PACING IS EVIDENCE-BASED: only the server's response distinguishes
//! a normal token from one with the noratelimit opt-out (verified live
//! 2026-06-10: a noratelimit token receives NO `X-RateLimit-*` headers
//! and never 429s — any static pacing would slow it 50x for nothing).
//! A [`QuotaTracker`] therefore admits calls freely until evidence
//! arrives: `X-RateLimit-Limit` / `X-RateLimit-Remaining` /
//! `X-RateLimit-Reset` headers (reset = Unix epoch SECONDS —
//! `buildRateLimit`, dist/ScreepsAPI.js:1238-1254 computes
//! `toReset: reset - now_secs`) pace the remainder of the window, and a
//! 429's retry-after marks the window spent (the client backs off and
//! resumes — worst case for a limited token is ONE 429 per window).
//!
//! THE QUOTA TABLE is pinned from the canonical community client,
//! screepers/node-screeps-api — `src/ScreepsAPI.ts` (master, fetched
//! 2026-06-10), identical to the vendored
//! `node_modules/screeps-api@1.16.1` `dist/ScreepsAPI.js:1417-1438`
//! (READ-ONLY). It no longer brakes anything: it feeds the WORST-CASE
//! ETA planning math (prospector's `plan_scan`/`plan_fetch`) and names
//! which endpoints are worth tracking at all.
//!
//! Private servers enforce none of this (`@screeps/backend` has no
//! rate-limit middleware; verified on the live eval container
//! 2026-06-10) — [`crate::Client`] only engages this module for
//! official targets ([`crate::is_official_target`]).

use std::time::{Duration, Instant};

const HOUR: Duration = Duration::from_secs(3600);
const DAY: Duration = Duration::from_secs(86_400);

/// The sanctioned opt-out: signed-in token owners can disable rate
/// limiting per auth token at this page. Source: node-screeps-api's
/// `rateLimitResetUrl` getter (dist/ScreepsAPI.js:1454-1459 — it
/// appends `?token=<first 8 chars>`; we print the bare page and never
/// echo token material).
pub const NO_RATE_LIMIT_URL: &str = "https://screeps.com/a/#!/account/auth-tokens/noratelimit";

/// One pinned `{limit, period}` quota (node-screeps-api `defaultLimit`,
/// dist/ScreepsAPI.js:1410-1416).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EndpointQuota {
    /// Max calls per window.
    pub limit: u32,
    /// Window length (hour or day in the official table).
    pub period: Duration,
}

impl EndpointQuota {
    /// The call spacing that fits the quota indefinitely
    /// (`period / limit`, e.g. 60/hour -> one call per 60 s). Used for
    /// worst-case ETA planning, not for pacing — see [`QuotaTracker`].
    pub fn sustained_spacing(&self) -> Duration {
        self.period / self.limit.max(1)
    }
}

/// The official per-endpoint quota table. Every entry cites its line in
/// the vendored `node_modules/screeps-api@1.16.1/dist/ScreepsAPI.js`
/// (identical to `src/ScreepsAPI.ts` on master, fetched 2026-06-10).
/// Endpoints not listed here are governed only by the global
/// 120/minute cap (ScreepsAPI.js:1418), which the client's `min_delay`
/// already satisfies.
pub fn endpoint_quota(method: &str, path: &str) -> Option<EndpointQuota> {
    let quota = |limit, period| Some(EndpointQuota { limit, period });
    match (method, path) {
        // GET (ScreepsAPI.js:1419-1429)
        ("GET", "/api/game/room-terrain") => quota(360, HOUR), // :1420
        ("GET", "/api/user/code") => quota(60, HOUR),          // :1421
        ("GET", "/api/user/memory") => quota(1440, DAY),       // :1422
        ("GET", "/api/user/memory-segment") => quota(360, HOUR), // :1423
        ("GET", "/api/game/market/orders-index") // :1424
        | ("GET", "/api/game/market/orders") // :1425
        | ("GET", "/api/game/market/my-orders") // :1426
        | ("GET", "/api/game/market/stats") // :1427
        | ("GET", "/api/game/user/money-history") => quota(60, HOUR), // :1428
        // POST (ScreepsAPI.js:1430-1437)
        ("POST", "/api/user/console") => quota(360, HOUR), // :1431
        ("POST", "/api/game/map-stats") => quota(60, HOUR), // :1432
        ("POST", "/api/user/code") => quota(240, DAY),     // :1433
        ("POST", "/api/user/set-active-branch") => quota(240, DAY), // :1434
        ("POST", "/api/user/memory") => quota(240, DAY),   // :1435
        ("POST", "/api/user/memory-segment") => quota(60, HOUR), // :1436
        _ => None,
    }
}

/// Parsed `X-RateLimit-*` response headers — the server's ground truth
/// for the current window. `reset_epoch_secs` is Unix epoch seconds
/// (node-screeps-api `buildRateLimit`, dist/ScreepsAPI.js:1238-1254).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitInfo {
    pub limit: u64,
    pub remaining: u64,
    pub reset_epoch_secs: u64,
}

/// Parse the three `X-RateLimit-*` header values. All three must be
/// present and numeric (the JS client coerces with `+`; a private
/// server sends none of them and this returns `None`).
pub fn parse_rate_limit_headers(
    limit: Option<&str>,
    remaining: Option<&str>,
    reset: Option<&str>,
) -> Option<RateLimitInfo> {
    Some(RateLimitInfo {
        limit: limit?.trim().parse().ok()?,
        remaining: remaining?.trim().parse().ok()?,
        reset_epoch_secs: reset?.trim().parse().ok()?,
    })
}

/// Extract the wait from the official 429 message — observed live:
/// `Rate limit exceeded, retry after 3548835ms or disable rate
/// limiting using this link: https://screeps.com/a/#!/account/auth-tokens/noratelimit?token=...`
/// Works on the bare string and on the `{"error": "..."}` envelope
/// (the 429 classifier keeps the raw body as the message).
pub fn parse_retry_after_ms(message: &str) -> Option<u64> {
    const MARKER: &str = "retry after ";
    let start = message.find(MARKER)? + MARKER.len();
    let rest = &message[start..];
    let digits_len = rest.chars().take_while(char::is_ascii_digit).count();
    if digits_len == 0 || !rest[digits_len..].starts_with("ms") {
        return None;
    }
    rest[..digits_len].parse().ok()
}

/// Per-endpoint pacing state: EVIDENCE-BASED. Calls run free until the
/// server itself says this token is limited — via `X-RateLimit-*`
/// headers or a live 429 — because the response is the only ground
/// truth that distinguishes a normal token from one with the
/// noratelimit opt-out (verified live 2026-06-10: a noratelimit token
/// receives NO rate-limit headers and no 429s, so any static pacing
/// would slow it 50x for nothing).
///
/// - **No evidence**: zero delay. The pinned table stays the
///   worst-case ETA prior for planning ([`EndpointQuota`]), not a
///   brake. Worst case for a limited token is one 429 per window —
///   absorbed by the client's backoff-and-resume, after which this
///   tracker holds further calls until the stated reset.
/// - **Header ground truth** (fresh `X-RateLimit-*` observation):
///   space at `time_to_reset / remaining`, which fits exactly what the
///   server says is left; a hard wait until reset when
///   `remaining == 0`.
/// - **A 429** ([`QuotaTracker::observe_rate_limited`]): the window is
///   spent until the server's retry-after.
///
/// Pure math over caller-supplied [`Instant`]s — unit-tested offline.
#[derive(Debug)]
pub struct QuotaTracker {
    quota: EndpointQuota,
    last_call: Option<Instant>,
    /// Ground truth from the latest `X-RateLimit-Remaining`, decremented
    /// locally per call between observations.
    remaining: Option<u64>,
    /// When the observed window resets (converted from epoch seconds).
    reset_at: Option<Instant>,
}

impl QuotaTracker {
    pub fn new(quota: EndpointQuota) -> Self {
        QuotaTracker {
            quota,
            last_call: None,
            remaining: None,
            reset_at: None,
        }
    }

    pub fn quota(&self) -> EndpointQuota {
        self.quota
    }

    /// How long the caller must wait before the next call to this
    /// endpoint. Zero unless the server has given evidence of limiting
    /// (a fresh `X-RateLimit-*` observation or a 429) whose window is
    /// still open.
    pub fn required_delay(&self, now: Instant) -> Duration {
        // Ground truth is only trusted while its window is still open.
        let ground = match (self.remaining, self.reset_at) {
            (Some(remaining), Some(reset_at)) if reset_at > now => Some((remaining, reset_at)),
            _ => None,
        };
        let spacing = match ground {
            // Quota exhausted: nothing helps but waiting out the window.
            Some((0, reset_at)) => return reset_at.saturating_duration_since(now),
            Some((remaining, reset_at)) => {
                let to_reset = reset_at.saturating_duration_since(now);
                to_reset / u32::try_from(remaining).unwrap_or(u32::MAX)
            }
            // No evidence this token is limited: run free. A
            // noratelimit token stays here forever; a limited token
            // lands one 429 worst-case and the observation above takes
            // over for the rest of the window.
            None => return Duration::ZERO,
        };
        match self.last_call {
            Some(last) => (last + spacing).saturating_duration_since(now),
            None => Duration::ZERO,
        }
    }

    /// Record that a call is being made now.
    pub fn record_call(&mut self, now: Instant) {
        self.last_call = Some(now);
        if let Some(remaining) = self.remaining.as_mut() {
            *remaining = remaining.saturating_sub(1);
        }
    }

    /// Adopt the server's `X-RateLimit-*` ground truth.
    /// `unix_now_secs` anchors the epoch-seconds reset to `now`.
    pub fn observe_headers(&mut self, info: RateLimitInfo, now: Instant, unix_now_secs: u64) {
        self.remaining = Some(info.remaining);
        self.reset_at =
            Some(now + Duration::from_secs(info.reset_epoch_secs.saturating_sub(unix_now_secs)));
    }

    /// A 429 told us the truth the hard way: the window is spent until
    /// `retry_in` from now.
    pub fn observe_rate_limited(&mut self, retry_in: Duration, now: Instant) {
        self.remaining = Some(0);
        self.reset_at = Some(now + retry_in);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map_stats_quota() -> EndpointQuota {
        endpoint_quota("POST", "/api/game/map-stats").unwrap()
    }

    /// The pinned table: spot-check every distinct (limit, period)
    /// class against dist/ScreepsAPI.js:1417-1438.
    #[test]
    fn quota_table_matches_node_screeps_api() {
        assert_eq!(
            map_stats_quota(),
            EndpointQuota {
                limit: 60,
                period: HOUR
            },
            ":1432"
        );
        assert_eq!(
            endpoint_quota("GET", "/api/game/room-terrain").unwrap(),
            EndpointQuota {
                limit: 360,
                period: HOUR
            },
            ":1420"
        );
        // Same path, different quota per method (:1421 vs :1433).
        assert_eq!(endpoint_quota("GET", "/api/user/code").unwrap().limit, 60);
        assert_eq!(
            endpoint_quota("POST", "/api/user/code").unwrap(),
            EndpointQuota {
                limit: 240,
                period: DAY
            },
        );
        assert_eq!(
            endpoint_quota("GET", "/api/user/memory").unwrap(),
            EndpointQuota {
                limit: 1440,
                period: DAY
            },
            ":1422"
        );
        assert_eq!(
            endpoint_quota("POST", "/api/user/memory-segment")
                .unwrap()
                .limit,
            60,
            ":1436"
        );
        // Un-tabled endpoints fall back to the global cap (None here).
        assert!(endpoint_quota("GET", "/api/game/time").is_none());
        assert!(endpoint_quota("POST", "/api/game/place-spawn").is_none());
    }

    /// 60/hour means one call per 60 s sustained — the worst-case ETA
    /// math the planning printouts cite.
    #[test]
    fn sustained_spacing_math() {
        assert_eq!(
            map_stats_quota().sustained_spacing(),
            Duration::from_secs(60)
        );
        assert_eq!(
            endpoint_quota("GET", "/api/game/room-terrain")
                .unwrap()
                .sustained_spacing(),
            Duration::from_secs(10)
        );
        assert_eq!(
            endpoint_quota("POST", "/api/user/code")
                .unwrap()
                .sustained_spacing(),
            Duration::from_secs(360)
        );
    }

    /// No evidence of limiting -> no pacing, ever. This is the
    /// noratelimit-token steady state (verified live 2026-06-10: such
    /// tokens receive no rate-limit headers and never 429).
    #[test]
    fn tracker_is_permissive_without_evidence() {
        let mut tracker = QuotaTracker::new(map_stats_quota());
        let t0 = Instant::now();
        assert_eq!(
            tracker.required_delay(t0),
            Duration::ZERO,
            "first call free"
        );
        // Burst far past the table's 60/hour: still free — the table
        // is an ETA prior, not a brake.
        for _ in 0..100 {
            tracker.record_call(t0);
            assert_eq!(tracker.required_delay(t0), Duration::ZERO);
        }
    }

    /// Header ground truth engages pacing: a window with plenty
    /// remaining paces gently; near-empty slows hard.
    #[test]
    fn tracker_prefers_header_ground_truth() {
        let mut tracker = QuotaTracker::new(map_stats_quota());
        let t0 = Instant::now();
        tracker.record_call(t0);
        // 30 min to reset, 60 calls remaining -> 30 s spacing.
        tracker.observe_headers(
            RateLimitInfo {
                limit: 60,
                remaining: 60,
                reset_epoch_secs: 1_000_000 + 1800,
            },
            t0,
            1_000_000,
        );
        assert_eq!(tracker.required_delay(t0), Duration::from_secs(30));
        // Someone else burned the window: 2 remaining, 30 min left ->
        // 900 s spacing (much slower than the prior — correctly so).
        tracker.observe_headers(
            RateLimitInfo {
                limit: 60,
                remaining: 2,
                reset_epoch_secs: 1_000_000 + 1800,
            },
            t0,
            1_000_000,
        );
        assert_eq!(tracker.required_delay(t0), Duration::from_secs(900));
    }

    /// `remaining: 0` is a hard wait until the window resets — the
    /// operator's live failure state.
    #[test]
    fn tracker_waits_out_an_exhausted_window() {
        let mut tracker = QuotaTracker::new(map_stats_quota());
        let t0 = Instant::now();
        tracker.record_call(t0);
        tracker.observe_headers(
            RateLimitInfo {
                limit: 60,
                remaining: 0,
                reset_epoch_secs: 1_000_000 + 3549,
            },
            t0,
            1_000_000,
        );
        assert_eq!(tracker.required_delay(t0), Duration::from_secs(3549));
        // Once the window has passed, the evidence is stale — back to
        // running free until the server objects again.
        assert_eq!(
            tracker.required_delay(t0 + Duration::from_secs(3550)),
            Duration::ZERO
        );
    }

    /// A 429 marks the window spent for exactly the server's retry-after.
    #[test]
    fn tracker_observe_rate_limited_blocks_until_retry_after() {
        let mut tracker = QuotaTracker::new(map_stats_quota());
        let t0 = Instant::now();
        tracker.observe_rate_limited(Duration::from_millis(3_548_835), t0);
        assert_eq!(tracker.required_delay(t0), Duration::from_millis(3_548_835));
    }

    /// Local decrement between observations: header says 2 remaining,
    /// one call later only 1 is assumed left.
    #[test]
    fn tracker_decrements_remaining_per_call() {
        let mut tracker = QuotaTracker::new(map_stats_quota());
        let t0 = Instant::now();
        tracker.observe_headers(
            RateLimitInfo {
                limit: 60,
                remaining: 2,
                reset_epoch_secs: 1_000_000 + 1000,
            },
            t0,
            1_000_000,
        );
        tracker.record_call(t0);
        tracker.record_call(t0);
        // remaining hit 0 -> hard wait for the rest of the window.
        assert_eq!(tracker.required_delay(t0), Duration::from_secs(1000));
    }

    /// LITERAL HEADER SETS: the screeps.com shape (numeric strings) and
    /// the private-server shape (no headers at all).
    #[test]
    fn rate_limit_header_parsing() {
        assert_eq!(
            parse_rate_limit_headers(Some("60"), Some("58"), Some("1765400400")),
            Some(RateLimitInfo {
                limit: 60,
                remaining: 58,
                reset_epoch_secs: 1_765_400_400
            })
        );
        // Exhausted window.
        assert_eq!(
            parse_rate_limit_headers(Some("60"), Some("0"), Some("1765400400"))
                .unwrap()
                .remaining,
            0
        );
        // Private server: no headers.
        assert_eq!(parse_rate_limit_headers(None, None, None), None);
        // Partial / junk values never half-parse.
        assert_eq!(
            parse_rate_limit_headers(Some("60"), None, Some("1765400400")),
            None
        );
        assert_eq!(
            parse_rate_limit_headers(Some("60"), Some("nan"), Some("1765400400")),
            None
        );
    }

    /// THE OPERATOR'S EXACT 429 SHAPE (live screeps.com, 2026-06-10).
    #[test]
    fn retry_after_parsing_matches_the_live_failure() {
        let live = "Rate limit exceeded, retry after 3548835ms or disable rate limiting \
                    using this link: https://screeps.com/a/#!/account/auth-tokens/noratelimit?token=12345678";
        assert_eq!(parse_retry_after_ms(live), Some(3_548_835));
        // The short form documented at docs.screeps.com/auth-tokens.html.
        assert_eq!(
            parse_retry_after_ms("Rate limit exceeded, retry after 743ms"),
            Some(743)
        );
        // The envelope-wrapped form (raw 429 body kept as the message).
        assert_eq!(
            parse_retry_after_ms(r#"{"error":"Rate limit exceeded, retry after 743ms"}"#),
            Some(743)
        );
        assert_eq!(parse_retry_after_ms("Rate limit exceeded"), None);
        assert_eq!(parse_retry_after_ms("retry after ms"), None);
        assert_eq!(parse_retry_after_ms("retry after 12s"), None);
    }
}
