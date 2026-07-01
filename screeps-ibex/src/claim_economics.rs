//! Pure, world-free **claim-selection value** kernel (ADR 0038 §2 Part B).
//!
//! The unified value of *claiming* a room, composing the shared economic kernel
//! [`crate::room_economics::room_net_roi`] with the marginal-economics of expansion:
//!
//! ```text
//! claim_value(R, d) = intrinsic_roi(R) · unlock_fraction(d) · support_decay(d) · plan_quality(R)
//! ```
//!
//! - **`intrinsic_roi`** — the room's OWNED-colony net-ROI ([`RoomEconomyFacts::owned_colony`]), evaluated with
//!   a fixed *internal* haul so it is **distance-independent** (ADR 0038 D4 / C-DEFECT-1: passing the claim
//!   distance as `haul_tiles` — the remote adapter's pattern — drives `net_roi` to 0 within claimer reach and,
//!   times a rising unlock fraction, re-creates the expansion stall).
//! - **`unlock_fraction(d)`** — the SPRAWL term, grounded in the radius-1 remote-mining reach (a room a colony
//!   already remote-mines unlocks ~0 *new* economy). Floored nonzero so a cannibalizing room stays
//!   low-but-claimable — never a hard stall. Replaces the old ad-hoc `distance_score` peak-at-4 curve + the
//!   `adjacent_claim_penalty`.
//! - **`support_decay(d)`** — a mild reciprocal far-room establishment/logistics penalty; strictly positive,
//!   never a gate (the hard reach cutoff is `is_claim_feasible`).
//! - **`plan_quality(R)`** — the SOFT plan-quality multiplier (floored; missing = neutral). The HARD "no valid
//!   plan ⇒ no claim" gate lives in the pipeline (the viable `can_plan` + the mission-creation defer), NOT here.
//!
//! Pure + bit-deterministic (scalar `f64`, no `HashMap`, no `game::*`); the claim adapter in `operations::claim`
//! gathers the facts and calls this — mirroring the `room_economics`/`war.rs` split ADR 0032 sanctioned.

use crate::room_economics::{room_net_roi, RoomEconomyFacts};

/// Intra-room one-way haul distance (tiles) for a claimed, self-hauling colony — ~half a room. **Distance-
/// INDEPENDENT** (ADR 0038 D4): a claimed room hauls to its own storage, so its intrinsic value must not carry
/// the parent-distance haul the remote adapter uses.
pub const INTERNAL_HAUL_TILES: u32 = 25;

/// Tunables for the claim value model (ADR 0038 D5–D7). The claim adapter fills these from `ClaimFeatures`.
#[derive(Debug, Clone, Copy)]
pub struct ClaimValueParams {
    /// Distance (hops) at which [`unlock_fraction`] reaches 1.0 — two colonies' radius-1 remote rings become
    /// disjoint (`2·remote_range + 2`, currently 4). Reuses the `ring_separation_hops` config so the remote-ring
    /// math and this curve can never drift.
    pub ring_separation_hops: u32,
    /// Nonzero floor of [`unlock_fraction`] at distance 1 — the ANTI-STALL constant (a cannibalizing room stays
    /// low-but-claimable, never hard-zero).
    pub unlock_floor: f64,
    /// Support-decay rate per hop (reciprocal form).
    pub support_decay_k: f64,
    /// Intra-room haul tiles for the intrinsic owned-colony ROI (see [`INTERNAL_HAUL_TILES`]).
    pub internal_haul_tiles: u32,
    /// Net-ROI (energy-equivalent) normaliser → a ~0–1 score for the `max_score_delta` gate + viz. Purely a
    /// scale factor; it does NOT affect ranking (monotone), only the delta-gate strictness.
    pub roi_reference: f64,
}

/// The scored claim value + its sub-scores (for logging/viz).
#[derive(Debug, Clone, Copy)]
pub struct ClaimValue {
    /// Normalised composite value (~0–1), the sort key.
    pub value: f32,
    /// Normalised intrinsic owned-colony ROI (0–1ish), distance-independent (viz).
    pub roi: f32,
    /// Unlock fraction (0–1) (viz).
    pub unlock: f32,
    /// Support decay (0–1) (viz).
    pub decay: f32,
}

/// `unlock_fraction(d)`: 0 at the home room; floored at `unlock_floor` at d=1; **monotone-nondecreasing** to
/// 1.0 at `ring_separation_hops` and beyond. Grounds the sprawl preference in remote-ring separation (ADR 0038
/// D5) — a room a colony already remote-mines (d ≤ 1) unlocks ~0 new economy; past the ring-separation distance
/// the economy is fully new.
pub fn unlock_fraction(distance: u32, ring_separation_hops: u32, unlock_floor: f64) -> f64 {
    if distance == 0 {
        return 0.0;
    }
    let ring = ring_separation_hops.max(1);
    if distance >= ring || ring <= 1 {
        return 1.0;
    }
    // Linear ramp from `unlock_floor` at d=1 to 1.0 at d=ring.
    let t = (distance - 1) as f64 / (ring - 1) as f64;
    unlock_floor + (1.0 - unlock_floor) * t
}

/// `support_decay(d) = 1 / (1 + k·d)`: strictly positive and monotone-decreasing for all finite d. A mild tilt
/// against far rooms (establishment/logistics), **never a gate** (ADR 0038 D6) — the hard reach cutoff is
/// `missions::utility::is_claim_feasible` (~11 hops) upstream. The reciprocal form is mandatory: a linear
/// `1 − k·d` would reach 0 within reach and re-introduce a stall.
pub fn support_decay(distance: u32, k: f64) -> f64 {
    1.0 / (1.0 + k * distance as f64)
}

/// SOFT plan-quality multiplier (ADR 0038 D7): floored to `[0.1, 1.0]` for a present plan (a low-but-valid plan
/// must not hard-zero an otherwise-claimable room); neutral `1.0` for a not-yet-planned room (so it stays in
/// the ranked set and its plan gets requested). The HARD "no valid plan ⇒ no claim" gate is a pipeline
/// invariant (the viable `can_plan` exclusion + the mission-creation defer), NOT this multiplier.
pub fn plan_quality(plan_total: Option<f32>) -> f64 {
    match plan_total {
        Some(p) => 0.1 + 0.9 * (p.clamp(0.0, 1.0) as f64),
        None => 1.0,
    }
}

/// Deterministic ranking key (ADR 0038 D8, [`sim-determinism-fence`]): quantize the value so f64 rounding can't
/// split a genuine tie. The caller breaks remaining ties on the room identifier for a total,
/// HashMap-iteration-order-independent order.
pub fn claim_rank_quantize(value: f32) -> i64 {
    (value as f64 * 1000.0).round() as i64
}

/// THE unified claim value (ADR 0038 §2 Part B). Intrinsic owned-colony ROI (distance-independent) × unlock ×
/// support_decay × plan_quality, normalised by `roi_reference`.
pub fn claim_value(source_count: u32, distance: u32, plan_total: Option<f32>, p: &ClaimValueParams) -> ClaimValue {
    let intrinsic = room_net_roi(&RoomEconomyFacts::owned_colony(source_count, p.internal_haul_tiles)).net_roi;
    let roi_norm = intrinsic / p.roi_reference.max(1.0);
    let unlock = unlock_fraction(distance, p.ring_separation_hops, p.unlock_floor);
    let decay = support_decay(distance, p.support_decay_k);
    let plan_q = plan_quality(plan_total);
    let value = roi_norm * unlock * decay * plan_q;
    ClaimValue {
        value: value as f32,
        roi: roi_norm as f32,
        unlock: unlock as f32,
        decay: decay as f32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Default model params for tests — mirrors the intended `ClaimFeatures` defaults (ring 4, floor 0.05,
    /// k 0.05, internal haul 25, roi_reference ≈ a 2-source owned ring room's net-ROI).
    fn params() -> ClaimValueParams {
        ClaimValueParams {
            ring_separation_hops: 4,
            unlock_floor: 0.05,
            support_decay_k: 0.05,
            internal_haul_tiles: INTERNAL_HAUL_TILES,
            roi_reference: 26_000.0,
        }
    }

    /// The dynamic claimer-reach ceiling (`max_claim_radius_hops()` = 11) mirrored locally so this pure kernel
    /// test stays world-free (no `missions::utility` import).
    const REACH_CEILING: u32 = 11;

    #[test]
    fn unlock_zero_at_home_floored_positive_for_all_reachable() {
        assert_eq!(unlock_fraction(0, 4, 0.05), 0.0);
        for d in 1..=REACH_CEILING {
            assert!(unlock_fraction(d, 4, 0.05) >= 0.05, "d={d} below floor");
        }
        for d in 4..=REACH_CEILING {
            assert_eq!(unlock_fraction(d, 4, 0.05), 1.0, "d={d} should be fully unlocked");
        }
    }

    #[test]
    fn unlock_is_monotone_nondecreasing() {
        let u: Vec<f64> = (1..=6).map(|d| unlock_fraction(d, 4, 0.05)).collect();
        for w in u.windows(2) {
            assert!(w[1] >= w[0], "unlock must be non-decreasing: {:?}", u);
        }
        // Anti-peak-at-4 regression guard: the old distance_score PEAKED at 4 then declined; the new curve only
        // rises (decline is support_decay's job).
        assert!(unlock_fraction(1, 4, 0.05) < unlock_fraction(4, 4, 0.05));
    }

    #[test]
    fn support_decay_strictly_positive_and_monotone_decreasing_over_reach() {
        assert_eq!(support_decay(0, 0.05), 1.0);
        let mut prev = 2.0;
        for d in 0..=REACH_CEILING {
            let s = support_decay(d, 0.05);
            assert!(s > 0.0, "support must never gate (d={d})");
            assert!(s < prev, "support must be strictly decreasing (d={d})");
            prev = s;
        }
    }

    #[test]
    fn intrinsic_roi_is_distance_independent() {
        let p = params();
        // The `roi` field is the intrinsic owned-colony ROI; it must not vary with the claim distance
        // (C-DEFECT-1 fix — distance sensitivity lives only in unlock×support).
        assert_eq!(claim_value(2, 2, None, &p).roi, claim_value(2, 9, None, &p).roi);
    }

    #[test]
    fn w13n51_two_source_d1_is_low_but_strictly_nonzero() {
        let p = params();
        let d1 = claim_value(2, 1, None, &p).value;
        let ring = claim_value(2, p.ring_separation_hops, None, &p).value;
        assert!(d1 > 0.0, "a cannibalizing room must stay claimable (anti-stall floor)");
        assert!((d1 as f64) < (ring as f64) / 5.0, "d1={d1} ring={ring}: adjacent must be far below the ring");
    }

    #[test]
    fn far_single_source_beats_near_cannibalizing_double_source() {
        let p = params();
        // The operator's sprawl intent: a fully-unlocked far single source out-values a partially-cannibalizing
        // near double source.
        assert!(claim_value(1, 8, None, &p).value > claim_value(2, 2, None, &p).value);
    }

    #[test]
    fn ring_room_dominates_adjacent() {
        let p = params();
        let ring = claim_value(2, 4, None, &p).value as f64;
        let adj = claim_value(2, 1, None, &p).value as f64;
        assert!(ring >= 10.0 * adj, "cannibalization discount must be decisive: ring={ring} adj={adj}");
    }

    #[test]
    fn zero_source_room_scores_zero() {
        // Score-0 is defense-in-depth; the viable gate (has_sources) is the PRIMARY exclusion.
        assert_eq!(claim_value(0, 4, None, &params()).value, 0.0);
    }

    #[test]
    fn plan_zero_does_not_hard_zero() {
        let p = params();
        // A low-but-VALID plan (soft quality) must not hard-zero a claimable room (C-DEFECT-2)...
        assert!(claim_value(2, 4, Some(0.0), &p).value > 0.0);
        // ...and a not-yet-planned room scores neutral (the HARD gate is the pipeline, not this multiplier).
        assert_eq!(plan_quality(None), 1.0);
    }

    #[test]
    fn claim_value_is_deterministic() {
        let p = params();
        let a = claim_value(2, 5, Some(0.7), &p);
        let b = claim_value(2, 5, Some(0.7), &p);
        assert_eq!(a.value, b.value);
        assert!(a.value.is_finite());
    }

    #[test]
    fn selection_is_total_and_stable() {
        // (name, value) stands in for (RoomName, claim_value). Select by (quantize desc, name asc); the result
        // must be independent of input order — the claim-side analogue of `sim_is_deterministic_over_rounds`,
        // guarding against the HashMap-seeded BFS feeding equal-scored candidates in a flaky order.
        fn winner(mut v: Vec<(&'static str, f32)>) -> &'static str {
            v.sort_by(|a, b| claim_rank_quantize(b.1).cmp(&claim_rank_quantize(a.1)).then(a.0.cmp(b.0)));
            v[0].0
        }
        // W1N1 and W2N2 tie at 0.5 (equal quantize) → broken by name asc → W1N1, regardless of input order.
        let base = vec![("W1N1", 0.5), ("W2N2", 0.5), ("W3N3", 0.3341), ("W4N4", 0.3475)];
        let forward = winner(base.clone());
        let mut reversed = base.clone();
        reversed.reverse();
        assert_eq!(forward, winner(reversed));
        assert_eq!(forward, "W1N1");
    }

    #[test]
    fn ring_sep_tracks_config_default() {
        // Pins the model's default RING_SEP to 4 (= `2·remote_range(1) + 2`); a config change to
        // `ring_separation_hops` must be a deliberate edit that flows through the adapter (see the
        // features.rs default test that pins the config side).
        assert_eq!(params().ring_separation_hops, 4);
    }

    #[test]
    fn support_never_gates_at_reach_ceiling() {
        // The farthest feasible room (d = max_claim_radius_hops() = 11) is still strictly claimable.
        assert!(claim_value(1, REACH_CEILING, None, &params()).value > 0.0);
    }
}
