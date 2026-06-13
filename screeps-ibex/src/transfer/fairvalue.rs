//! FairValue oracle + exposure caps — ADR 0012 M0/M1 (IBEX-018).
//!
//! Pure, host-testable kernels: nothing here touches `game::`. The JS
//! `OrderHistoryRecord`s are copied into plain [`HistoryDay`]s at the call
//! site (ordersystem.rs) because wasm-bindgen types cannot be constructed on
//! the host target. No trading decision may reference a raw `.last()` history
//! day — the latest day is exactly what a wash-trader paints (threat T1).
//!
//! The chain-value anchor clamp (ADR 0012 §1) depends on ADR 0010 L0's
//! chain-math kernel and lands with the Inc-7 TradePlanner (M2); until then
//! an untrusted history yields no price at all (maker-only-from-anchors is
//! not yet possible, so we abstain).

use screeps::constants::ResourceType;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// One engine market-history day (a `getHistory` record), copied to a plain
/// struct so the oracle is host-testable.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryDay {
    /// Calendar date, ISO "YYYY-MM-DD" — lexicographic order is chronological.
    pub date: String,
    pub avg_price: f64,
    pub stddev_price: f64,
    pub volume: u32,
    pub transactions: u32,
}

/// The oracle's verdict for one resource.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FairValue {
    /// Volume-filtered trailing median of the daily average price.
    pub price: f64,
    /// The latest day deviates from the median by more than the deviation
    /// gate, or its volume z-score against the rest of the window exceeds
    /// the gate. Pricing stays on the median, but taker action (deals) and
    /// new buy orders are barred for the cadence (ADR 0012 §1).
    pub latest_day_anomalous: bool,
}

/// Minimum days surviving the volume filter before a median is trusted —
/// below this a single painted day is a large fraction of the window.
pub const MIN_WINDOW_DAYS: usize = 4;

/// Liquidity floor (threat T2): the window's median daily volume must be at
/// least this multiple of our own intended daily trade, or we would *be* the
/// market and the history says nothing about what others will pay.
pub const LIQUIDITY_FLOOR_MULTIPLE: f64 = 5.0;

/// Days with volume below this fraction of the window's median volume are
/// dropped before taking the price median (negligible-volume days are free
/// to paint).
const VOLUME_FILTER_FRACTION: f64 = 0.1;

/// Latest-day average more than this fraction away from the trailing median
/// flags the day anomalous.
const DEVIATION_GATE_FRACTION: f64 = 0.5;

/// Latest-day volume z-score (vs the rest of the window) above this flags
/// the day anomalous (spike-and-dump prints volume as well as price).
const VOLUME_ZSCORE_GATE: f64 = 3.0;

/// Passive (maker) sell orders ask above fair value — covers the 5% listing
/// fee and concedes nothing to a painted reference day.
pub const MAKER_SELL_MARKUP: f64 = 1.05;

/// Passive (maker) buy orders bid BELOW fair value (ADR 0012 §2). The old
/// formula bid `avg + 0.1σ` — *above* the daily average — donating the
/// spread to whoever filled us.
pub const MAKER_BUY_DISCOUNT: f64 = 0.95;

/// Active sells never accept an effective (energy-netted) price below this
/// fraction of fair value (ADR 0012 §2).
pub const ACTIVE_SELL_FLOOR_FRACTION: f64 = 0.8;

/// Hard ceiling on per-unit transfer cost for any market deal we initiate
/// (threat T5: far-room honeypots burn the dealer's energy). 0.25 ≈ 8.6
/// rooms of distance.
pub const MAX_DEAL_COST_PER_UNIT: f64 = 0.25;

/// Hard ceiling on per-unit transfer cost for intra-empire terminal sends —
/// the previously-unfloored `resources/cost` ranking could pick an
/// arbitrarily bad route when it was the only candidate (ADR 0012 §3).
pub const MAX_INTRA_EMPIRE_COST_PER_UNIT: f64 = 0.5;

/// Compute the manipulation-resistant fair value for one resource from its
/// history window (up to 14 days). `intended_daily_trade` is the volume we
/// expect to move per day (the 2,000-unit block for the current system).
///
/// Returns `None` when the history cannot be trusted at all: too few
/// meaningful days, or the market is too thin for our size (liquidity
/// floor). `None` means abstain — exactly what the old
/// `transactions > 100 && volume > 1000 && stddev ≤ 0.5·avg` gate failed to
/// guarantee, since a uniform-price wash day satisfies all three terms.
pub fn fair_value(days: &[HistoryDay], intended_daily_trade: f64) -> Option<FairValue> {
    // Chronological order; the engine returns it sorted but nothing here
    // relies on that (the old code's "relies on sequential date order" NOTE).
    let mut window: Vec<&HistoryDay> = days.iter().filter(|d| d.avg_price.is_finite() && d.avg_price > 0.0).collect();
    window.sort_by(|a, b| a.date.cmp(&b.date));

    let median_volume = median(window.iter().map(|d| d.volume as f64))?;

    // Liquidity floor (T2).
    if median_volume < LIQUIDITY_FLOOR_MULTIPLE * intended_daily_trade {
        return None;
    }

    // Volume filter: negligible-volume days cannot vote on the median.
    let kept: Vec<&HistoryDay> = window
        .iter()
        .filter(|d| (d.volume as f64) >= VOLUME_FILTER_FRACTION * median_volume)
        .copied()
        .collect();

    if kept.len() < MIN_WINDOW_DAYS {
        return None;
    }

    let median_price = median(kept.iter().map(|d| d.avg_price))?;
    debug_assert!(median_price.is_finite(), "fair value not finite: {median_price}");

    // Deviation + volume gates on the latest day (the attack surface).
    let latest = *window.last()?;
    let mut anomalous = (latest.avg_price - median_price).abs() > DEVIATION_GATE_FRACTION * median_price;

    let baseline: Vec<f64> = window[..window.len() - 1].iter().map(|d| d.volume as f64).collect();
    if baseline.len() >= 3 {
        let mean = baseline.iter().sum::<f64>() / baseline.len() as f64;
        let variance = baseline.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / baseline.len() as f64;
        let stddev = variance.sqrt();
        let deviation = latest.volume as f64 - mean;
        let z = if stddev > f64::EPSILON {
            deviation / stddev
        } else if deviation.abs() <= f64::EPSILON {
            0.0
        } else {
            f64::INFINITY
        };
        anomalous |= z > VOLUME_ZSCORE_GATE;
    }

    Some(FairValue {
        price: median_price,
        latest_day_anomalous: anomalous,
    })
}

fn median(values: impl Iterator<Item = f64>) -> Option<f64> {
    let mut values: Vec<f64> = values.filter(|v| v.is_finite()).collect();
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = values.len() / 2;
    if values.len() % 2 == 1 {
        Some(values[mid])
    } else {
        Some((values[mid - 1] + values[mid]) / 2.0)
    }
}

/// Rolling-window length for the exposure ledger, ≈ one real-time day.
pub const EXPOSURE_WINDOW_TICKS: u32 = 20_000;

/// Per-window buy spend ceiling as a fraction of credits above the reserve
/// (ADR 0012 §2): even a fully-fooled oracle is bounded to losing this.
pub const MAX_BUY_SPEND_FRACTION: f64 = 0.05;

/// Per-window per-resource traded-unit ceiling (≈ 5 of the 2,000-unit
/// blocks, ADR 0012 §2). Counts placed orders and deals conservatively —
/// commitments, not fills.
pub const MAX_RESOURCE_UNITS_PER_WINDOW: u32 = 10_000;

/// Rolling exposure ledger (ADR 0012 §2). Persisted inside [`MarketMemory`]
/// on the dedicated market segment so a VM reset cannot zero the counters —
/// an ephemeral ledger would let a reset double the daily spend cap.
#[derive(Default, Serialize, Deserialize)]
pub struct ExposureLedger {
    window_start: u32,
    buy_notional: f64,
    traded_units: HashMap<ResourceType, u32>,
}

impl ExposureLedger {
    /// Reset the window when it has elapsed. Call once per trading pass.
    pub fn roll(&mut self, now: u32) {
        if now.saturating_sub(self.window_start) >= EXPOSURE_WINDOW_TICKS {
            self.window_start = now;
            self.buy_notional = 0.0;
            self.traded_units.clear();
        }
    }

    /// Would a buy commitment of `notional` credits / `units` units stay
    /// inside the window caps?
    pub fn buy_within_caps(&self, notional: f64, units: u32, resource: ResourceType, credits: f64, credit_reserve: f64) -> bool {
        let budget = (credits - credit_reserve).max(0.0) * MAX_BUY_SPEND_FRACTION;
        self.buy_notional + notional <= budget && self.volume_within_cap(resource, units)
    }

    /// Would trading `units` more of `resource` stay inside the per-resource
    /// window cap?
    pub fn volume_within_cap(&self, resource: ResourceType, units: u32) -> bool {
        self.traded(resource).saturating_add(units) <= MAX_RESOURCE_UNITS_PER_WINDOW
    }

    pub fn commit_buy(&mut self, notional: f64, units: u32, resource: ResourceType) {
        self.buy_notional += notional;
        self.commit_volume(resource, units);
    }

    pub fn commit_volume(&mut self, resource: ResourceType, units: u32) {
        let traded = self.traded_units.entry(resource).or_insert(0);
        *traded = traded.saturating_add(units);
    }

    fn traded(&self, resource: ResourceType) -> u32 {
        self.traded_units.get(&resource).copied().unwrap_or(0)
    }
}

/// Bump on any shape change to [`MarketMemory`] or its fields. The segment
/// payload is positional bincode; a mismatched version decodes to a loud
/// fresh-start, never to misaligned garbage. Independent of the world's
/// `WORLD_FORMAT_VERSION` by design — reshaping the component segments must
/// never cost the market state, and vice versa.
pub const MARKET_MEMORY_VERSION: u32 = 1;

/// Days of history retained per resource — matches the engine's `getHistory`
/// window, which is also the oracle's window (ADR 0012 §1).
pub const MAX_STORED_HISTORY_DAYS: usize = 14;

/// One resource's stored history window. A `Vec` of pairs rather than a map
/// keyed by resource so the wire shape stays order-stable.
#[derive(Serialize, Deserialize)]
pub struct ResourceHistory {
    pub resource: ResourceType,
    pub days: Vec<HistoryDay>,
}

/// Everything the market subsystem persists, as one block on the dedicated
/// market segment: the per-resource history-day cache (the oracle's input
/// survives resets and backend gaps — `getHistory` is backend-side and not
/// organically populated on a private server) and the exposure ledger
/// (counters that, if reset-zeroed, would let a reset double the daily
/// caps). The interim form of ADR 0012 M3's risk ledger.
#[derive(Serialize, Deserialize)]
pub struct MarketMemory {
    /// Must equal [`MARKET_MEMORY_VERSION`]; checked at decode.
    pub version: u32,
    pub history: Vec<ResourceHistory>,
    pub exposure: ExposureLedger,
    /// True once the segment has been read (or loudly declared fresh) this
    /// VM lifetime. Trading is gated on it so commitments made before the
    /// load cannot be overwritten by it. Never serialized.
    #[serde(skip)]
    pub loaded: bool,
}

impl Default for MarketMemory {
    fn default() -> Self {
        MarketMemory {
            version: MARKET_MEMORY_VERSION,
            history: Vec::new(),
            exposure: ExposureLedger::default(),
            loaded: false,
        }
    }
}

impl MarketMemory {
    /// Merge a freshly fetched `getHistory` window into the stored cache for
    /// `resource` and return the merged window (chronological). Fetched days
    /// replace stored days with the same date — the backend's aggregate for
    /// a day grows intra-day, so the newest copy is the most complete.
    pub fn merge_history(&mut self, resource: ResourceType, fetched: &[HistoryDay]) -> &[HistoryDay] {
        let entry = match self.history.iter().position(|h| h.resource == resource) {
            Some(index) => &mut self.history[index],
            None => {
                self.history.push(ResourceHistory {
                    resource,
                    days: Vec::new(),
                });
                self.history.last_mut().expect("entry just pushed")
            }
        };

        merge_days(&mut entry.days, fetched, MAX_STORED_HISTORY_DAYS);
        &entry.days
    }
}

/// Pure merge kernel: union by date (fetched wins), chronological order,
/// keep only the most recent `max_days`.
fn merge_days(stored: &mut Vec<HistoryDay>, fetched: &[HistoryDay], max_days: usize) {
    for day in fetched {
        match stored.iter_mut().find(|s| s.date == day.date) {
            Some(existing) => *existing = day.clone(),
            None => stored.push(day.clone()),
        }
    }

    stored.sort_by(|a, b| a.date.cmp(&b.date));

    if stored.len() > max_days {
        let excess = stored.len() - max_days;
        stored.drain(..excess);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(date: &str, avg: f64, volume: u32) -> HistoryDay {
        HistoryDay {
            date: date.to_string(),
            avg_price: avg,
            stddev_price: avg * 0.05,
            volume,
            transactions: 500,
        }
    }

    /// An honest 13-day window around price 10.0 with organic volume.
    fn honest_window() -> Vec<HistoryDay> {
        (1..=13)
            .map(|i| day(&format!("2026-06-{i:02}"), 10.0 + 0.1 * (i % 5) as f64, 100_000 + 1_000 * i as u32))
            .collect()
    }

    // ADR 0012 M0 validation: the painted day moves the latest-day mean by
    // 10x while the oracle's output moves by less than 5% — and the day is
    // flagged so taker/buy action is barred (M1: "painted-day fixture
    // produces zero buy actions").
    #[test]
    fn t1_painted_day_barely_moves_the_oracle() {
        let mut honest = honest_window();
        let baseline = fair_value(&honest, 2_000.0).expect("honest window must price");

        honest.push(day("2026-06-14", 100.0, 120_000)); // wash-painted 10x day
        let painted = fair_value(&honest, 2_000.0).expect("painted window still prices");

        assert!((painted.price - baseline.price).abs() < 0.05 * baseline.price);
        assert!(painted.latest_day_anomalous);
    }

    // T2: a volume spike alone (price still plausible) is anomalous — the
    // spike day is how a thin-book pump prints its reference.
    #[test]
    fn t2_volume_spike_is_anomalous() {
        let mut window = honest_window();
        window.push(day("2026-06-14", 10.5, 2_000_000));
        let fv = fair_value(&window, 2_000.0).expect("must price");
        assert!(fv.latest_day_anomalous);
    }

    // Honest volatility must NOT trip the gate (ADR 0012: the
    // honest-volatility control asserts Normal is retained).
    #[test]
    fn honest_volatility_is_not_anomalous() {
        let mut window = honest_window();
        window.push(day("2026-06-14", 11.5, 110_000)); // +15%, organic volume
        let fv = fair_value(&window, 2_000.0).expect("must price");
        assert!(!fv.latest_day_anomalous);
        assert!((fv.price - 10.0).abs() < 1.0);
    }

    // Liquidity floor: a market thinner than 5x our own trade is untrusted
    // entirely. (The old gate's `volume > 1000` allowed this.)
    #[test]
    fn thin_market_yields_no_price() {
        let window: Vec<_> = (1..=13).map(|i| day(&format!("2026-06-{i:02}"), 10.0, 3_000)).collect();
        assert!(fair_value(&window, 2_000.0).is_none());
        // The same window IS priceable for someone trading much smaller size.
        assert!(fair_value(&window, 100.0).is_some());
    }

    // Negligible-volume days cannot vote on the median: a near-zero-volume
    // wash print is filtered out before the median is taken.
    #[test]
    fn negligible_volume_days_are_filtered_from_the_median() {
        let mut window = honest_window();
        for i in 0..6 {
            window.push(day(&format!("2026-06-{:02}", 14 + i), 1_000.0, 50)); // dust-volume paint
        }
        let fv = fair_value(&window, 2_000.0).expect("must price");
        assert!((fv.price - 10.0).abs() < 1.0, "dust days moved the median: {}", fv.price);
    }

    #[test]
    fn too_few_days_yields_no_price() {
        let window = vec![day("2026-06-01", 10.0, 100_000), day("2026-06-02", 10.0, 100_000)];
        assert!(fair_value(&window, 2_000.0).is_none());
        assert!(fair_value(&[], 2_000.0).is_none());
    }

    #[test]
    fn non_finite_and_non_positive_days_are_ignored() {
        let mut window = honest_window();
        window.push(day("2026-06-14", f64::NAN, 100_000));
        window.push(day("2026-06-15", -5.0, 100_000));
        let fv = fair_value(&window, 2_000.0).expect("must price");
        assert!(fv.price.is_finite());
        assert!((fv.price - 10.0).abs() < 1.0);
    }

    // The oracle must not depend on the engine's returned ordering.
    #[test]
    fn input_order_does_not_matter() {
        let mut window = honest_window();
        window.push(day("2026-06-14", 100.0, 120_000));
        let forward = fair_value(&window, 2_000.0).expect("must price");
        window.reverse();
        let reversed = fair_value(&window, 2_000.0).expect("must price");
        assert_eq!(forward, reversed);
    }

    // The buy-price inversion pin (ADR 0012 §2 / IBEX-018): maker buys bid
    // below fair, maker sells ask above it. Deliberately constant — it fails
    // loudly if a retune ever re-inverts the spread.
    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn maker_prices_straddle_fair_value() {
        assert!(MAKER_BUY_DISCOUNT < 1.0);
        assert!(MAKER_SELL_MARKUP > 1.0);
        assert!(ACTIVE_SELL_FLOOR_FRACTION < 1.0);
    }

    #[test]
    fn exposure_ledger_enforces_buy_budget() {
        let mut ledger = ExposureLedger::default();
        ledger.roll(100);

        // 5% of (1_000_000 above reserve) = 50_000 budget.
        let credits = 11_000_000.0;
        let reserve = 10_000_000.0;
        assert!(ledger.buy_within_caps(30_000.0, 2_000, ResourceType::Oxygen, credits, reserve));
        ledger.commit_buy(30_000.0, 2_000, ResourceType::Oxygen);
        assert!(!ledger.buy_within_caps(30_000.0, 2_000, ResourceType::Oxygen, credits, reserve));
        assert!(ledger.buy_within_caps(20_000.0, 2_000, ResourceType::Oxygen, credits, reserve));

        // No credits above the reserve -> no buy budget at all.
        assert!(!ledger.buy_within_caps(1.0, 1, ResourceType::Oxygen, reserve, reserve));
    }

    #[test]
    fn exposure_ledger_enforces_per_resource_volume() {
        let mut ledger = ExposureLedger::default();
        ledger.roll(100);

        for _ in 0..5 {
            assert!(ledger.volume_within_cap(ResourceType::Hydrogen, 2_000));
            ledger.commit_volume(ResourceType::Hydrogen, 2_000);
        }
        assert!(!ledger.volume_within_cap(ResourceType::Hydrogen, 2_000));
        // Caps are per-resource.
        assert!(ledger.volume_within_cap(ResourceType::Oxygen, 2_000));
    }

    #[test]
    fn exposure_ledger_window_rolls() {
        let mut ledger = ExposureLedger::default();
        // A fresh ledger anchors its window on the first elapsed roll.
        ledger.roll(EXPOSURE_WINDOW_TICKS);
        ledger.commit_buy(50_000.0, 10_000, ResourceType::Hydrogen);
        assert!(!ledger.volume_within_cap(ResourceType::Hydrogen, 1));

        // Same window: counters persist.
        ledger.roll(2 * EXPOSURE_WINDOW_TICKS - 1);
        assert!(!ledger.volume_within_cap(ResourceType::Hydrogen, 1));

        // Window elapsed: counters reset.
        ledger.roll(2 * EXPOSURE_WINDOW_TICKS);
        assert!(ledger.volume_within_cap(ResourceType::Hydrogen, 2_000));
        assert!(ledger.buy_within_caps(1_000.0, 100, ResourceType::Hydrogen, 11_000_000.0, 10_000_000.0));
    }

    #[test]
    fn merge_replaces_same_date_keeps_window_and_sorts() {
        let mut stored = vec![day("2026-06-02", 10.0, 100), day("2026-06-01", 9.0, 100)];

        // Fetched updates an existing date (backend day aggregate grew) and
        // adds a new one, arriving unsorted.
        merge_days(&mut stored, &[day("2026-06-03", 11.0, 300), day("2026-06-02", 10.5, 200)], 14);

        let dates: Vec<&str> = stored.iter().map(|d| d.date.as_str()).collect();
        assert_eq!(dates, ["2026-06-01", "2026-06-02", "2026-06-03"]);
        assert_eq!(stored[1].avg_price, 10.5);
        assert_eq!(stored[1].volume, 200);
    }

    #[test]
    fn merge_drops_the_oldest_days_beyond_the_cap() {
        let mut stored: Vec<HistoryDay> = (1..=14).map(|i| day(&format!("2026-06-{i:02}"), 10.0, 100)).collect();

        merge_days(&mut stored, &[day("2026-06-15", 10.0, 100)], 14);

        assert_eq!(stored.len(), 14);
        assert_eq!(stored.first().unwrap().date, "2026-06-02");
        assert_eq!(stored.last().unwrap().date, "2026-06-15");
    }

    #[test]
    fn market_memory_merges_per_resource() {
        let mut memory = MarketMemory::default();

        let merged = memory.merge_history(ResourceType::Hydrogen, &[day("2026-06-01", 10.0, 100)]);
        assert_eq!(merged.len(), 1);

        let merged = memory.merge_history(ResourceType::Oxygen, &[day("2026-06-01", 5.0, 100)]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].avg_price, 5.0);

        // Hydrogen's window is untouched by oxygen's merge.
        let merged = memory.merge_history(ResourceType::Hydrogen, &[]);
        assert_eq!(merged[0].avg_price, 10.0);
    }

    // The segment payload must round-trip through the house encoding, and a
    // fresh default must carry the current version (the decode-side check
    // depends on it).
    #[test]
    fn market_memory_round_trips_through_the_house_encoding() {
        let mut memory = MarketMemory::default();
        memory.merge_history(ResourceType::Hydrogen, &honest_window());
        memory.exposure.roll(EXPOSURE_WINDOW_TICKS);
        memory.exposure.commit_buy(1_000.0, 500, ResourceType::Hydrogen);
        memory.loaded = true;

        let encoded = crate::serialize::encode_to_string(&memory).expect("encode");
        let decoded: MarketMemory = crate::serialize::decode_from_string(&encoded).expect("decode");

        assert_eq!(decoded.version, MARKET_MEMORY_VERSION);
        assert_eq!(decoded.history.len(), 1);
        assert_eq!(decoded.history[0].days.len(), 13);
        // The ledger's committed volume survived the round trip...
        assert!(!decoded
            .exposure
            .volume_within_cap(ResourceType::Hydrogen, MAX_RESOURCE_UNITS_PER_WINDOW));
        // ...and the in-memory `loaded` flag deliberately did not.
        assert!(!decoded.loaded);
    }
}
