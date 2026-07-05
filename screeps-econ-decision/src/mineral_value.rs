//! The shared **`mineral_value_e`** valuation kernel + the **lab-loading bid** (ADR 0040 §D1 M6;
//! ADR 0041 §8 O4 — the trust-gated compound valuation the boost cost term consumes).
//!
//! ## The coordination (ADR 0040 §D4 / ADR 0041 O4)
//!
//! ADR 0041's O4 decision defines a *"small pure `mineral_value_e(compound) -> f64` valuation
//! kernel — base-mineral prices + chain recipe in, energy-equivalent out — game-free,
//! deterministic, unit-testable"* whose live form is trust-gated: use the market price when it is
//! trustworthy, else fall back to a **cost-of-production floor** derived from the base-mineral
//! prices + the reagent-chain energy. **ADR 0041 P0 is not yet built** (Accepted-but-unbuilt in
//! the tree), so per the M6 spec this crate implements the **cost-of-production floor variant**
//! here, as the ONE shared kernel — when ADR 0041's combat boost layer lands, its war.rs producer
//! feeds this kernel the resolved base-mineral prices + a trust verdict and threads the single
//! scalar into `optimize_composition` (0041 D-O4: the decision crate stays game-free). This module
//! is the seam both consumers meet at; there is no second, drifting compound valuation.
//!
//! ## The cost-of-production floor (what "value" means with no trustworthy market)
//!
//! A compound's floor value in energy-equivalent is *what it cost the colony to make it*: the
//! energy-equivalent of every base mineral consumed down its full reaction tree PLUS the
//! reaction-chain energy overhead (the amortized lab-tick opportunity of each reaction step — a
//! lab reacting is a lab not boosting, and the fill haul + the REACTION_TIME cooldown are real
//! throughput costs). Base minerals are priced at a conservative mining floor
//! ([`BASE_MINERAL_VALUE_E`]); each `runReaction` step adds [`REACTION_STEP_OVERHEAD_E`] per unit
//! of product (amortized). The result is a **loose lower bound on what the compound is worth** —
//! never wildly over- or under-valuing a boost when the order book is empty (the O4 rationale).
//!
//! **Determinism (EP-6.13):** integer arithmetic in milli-e (`f64` only at the public boundary
//! for the O4 signature); the recipe tree is a static match, no HashMap, no ambient state.

/// A boost-relevant compound the M6 economy prices. Deliberately a SELF-CONTAINED vocabulary (this
/// crate is game-free and does not depend on the sim engine's `SimResource`): the adapter maps its
/// resource type onto this enum. The base minerals + catalyst are the leaves; the compounds carry
/// their reagent pair so [`mineral_value_e`] can walk the tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Compound {
    // Base minerals (the seven ores + catalyst) — the reagent-tree leaves.
    Hydrogen,
    Oxygen,
    Utrium,
    Lemergium,
    Keanium,
    Zynthium,
    Ghodium,
    Catalyst,
    // Compounds on the WORK-upgrade boost chain + the shared intermediates.
    Hydroxide,       // OH
    ZynthiumKeanite, // ZK
    UtriumLemergite, // UL
    GH,
    GH2O,
    XGH2O,
}

impl Compound {
    /// The reagent pair this compound is made from, or `None` for a base mineral (a tree leaf).
    /// Mirrors the engine `REACTIONS` inverse (the recipe that PRODUCES this compound).
    pub fn reagents(self) -> Option<(Compound, Compound)> {
        use Compound::*;
        Some(match self {
            Hydroxide => (Hydrogen, Oxygen),
            ZynthiumKeanite => (Zynthium, Keanium),
            UtriumLemergite => (Utrium, Lemergium),
            Ghodium => (ZynthiumKeanite, UtriumLemergite), // the COMPOUND G (ZK+UL); base ore G is a leaf priced identically
            GH => (Ghodium, Hydrogen),
            GH2O => (GH, Hydroxide),
            XGH2O => (Catalyst, GH2O),
            _ => return None, // base minerals: leaves
        })
    }

    /// Whether this is a base mineral (a reagent-tree leaf).
    pub fn is_base(self) -> bool {
        self.reagents().is_none()
    }
}

/// The conservative base-mineral mining floor (milli-e per unit) — the energy-equivalent it costs
/// the colony to extract one unit of a base mineral (a WORK-tick of extractor harvest = 1 mineral,
/// so the floor is the amortized cost of fielding + running the miner, priced conservatively at
/// par with energy). Deliberately a floor, not a market price (ADR 0041 O4 cold-start default).
pub const BASE_MINERAL_VALUE_E_MILLI: u32 = 1_000;

/// The amortized per-unit-product overhead of one `runReaction` step (milli-e) — the lab-tick
/// opportunity + fill-haul + REACTION_TIME cooldown cost, spread over the 5 product units a
/// reaction yields. A conservative fixed constant (the O4 "validated approximation" is validated
/// by the M6 bench's boost e/t diagnostic staying in a sane band). Seeded modest: a boost's value
/// is dominated by its base-mineral content, not the lab labor.
pub const REACTION_STEP_OVERHEAD_E_MILLI: u32 = 200;

/// **`mineral_value_e(compound)`** — the cost-of-production floor value of a compound in
/// energy-equivalent (ADR 0041 O4 signature: `-> f64`). Walks the reagent tree to the base
/// minerals, summing each leaf's mining floor + one [`REACTION_STEP_OVERHEAD_E_MILLI`] per
/// interior (reaction) node. Base minerals price at [`BASE_MINERAL_VALUE_E_MILLI`]. Deterministic,
/// game-free. Returns whole energy-equivalent (the milli value / 1000).
pub fn mineral_value_e(compound: Compound) -> f64 {
    mineral_value_e_milli(compound) as f64 / 1000.0
}

/// The exact-integer milli-e form (the arithmetic all comparisons should use — the `f64` public
/// form is the O4 signature only). One unit of `compound` costs: Σ base leaves × base floor +
/// (number of reaction steps in its tree) × the per-step overhead.
pub fn mineral_value_e_milli(compound: Compound) -> u32 {
    match compound.reagents() {
        None => BASE_MINERAL_VALUE_E_MILLI, // a base mineral: its mining floor
        Some((a, b)) => {
            // The two reagents' values + this reaction step's overhead (each interior node adds one).
            mineral_value_e_milli(a)
                .saturating_add(mineral_value_e_milli(b))
                .saturating_add(REACTION_STEP_OVERHEAD_E_MILLI)
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// The §D1 lab-loading bid (M6): "lab loading bids the compound pipeline's value."
// ═════════════════════════════════════════════════════════════════════════════════════════════

/// The e/t bid (milli, [`crate::sink_economics::BID_SCALE`]-scaled) for LOADING a lab's input
/// mineral toward a target compound (ADR 0040 §D1: "lab loading bids the compound pipeline's
/// value"). The energy a hauler spends carrying a reagent to a lab is worth what the compound it
/// enables is worth per unit of energy invested — i.e. the compound's cost-of-production value
/// relative to the numeraire (storage = 1.000).
///
/// `pipeline_value_milli` = [`mineral_value_e_milli`] of the TARGET compound (what the loaded
/// reagent chains into). The bid is that value scaled to the currency, capped so a deep chain
/// cannot outbid the survival/refill lanes: `min(pipeline_value_milli / BASE, cap) × BID_SCALE`,
/// then floored at the numeraire (loading a lab is never worth LESS than storing the reagent — it
/// unlocks a strictly more valuable product). Exact integer.
pub fn lab_load_bid(target: Compound, cap_milli: u32) -> u32 {
    let value = mineral_value_e_milli(target);
    // The compound's value as a multiple of a single base mineral, in the bid currency: a
    // 3-reaction-deep boost compound (base ×~6 + overhead) prices ~6× the numeraire — real but
    // bounded demand, capped so it slots below refill/survival.
    let bid = value.max(crate::sink_economics::STORAGE_BID);
    bid.min(cap_milli.max(crate::sink_economics::STORAGE_BID))
}

/// The default cap for [`lab_load_bid`] — a lab load is real infrastructure demand but must lose to
/// the collapse rebootstrap chain (refill's ROI clamps at ~10×). Seeded at the container-build
/// class so a lab fill prices like stocking a provider container: real, non-existential.
pub const LAB_LOAD_BID_CAP_MILLI: u32 = 4_000;

#[cfg(test)]
mod tests {
    use super::*;

    /// The reagent tree is well-formed: every compound's reagents resolve, base minerals are
    /// leaves, and the XGH2O chain has exactly the expected depth.
    #[test]
    fn reagent_tree_is_well_formed() {
        use Compound::*;
        assert!(Hydrogen.is_base() && Catalyst.is_base() && Ghodium.reagents().is_some());
        assert_eq!(Hydroxide.reagents(), Some((Hydrogen, Oxygen)));
        assert_eq!(XGH2O.reagents(), Some((Catalyst, GH2O)));
        // Ghodium the COMPOUND (ZK+UL) — note the engine shares the id with the base ore; the
        // valuation treats the compound recipe (its reagents are ZK+UL).
        assert_eq!(Ghodium.reagents(), Some((ZynthiumKeanite, UtriumLemergite)));
    }

    /// The cost-of-production floor: a base mineral is one floor unit; each reaction step adds its
    /// two reagents + one overhead. XGH2O = X + GH2O; GH2O = GH + OH; GH = G + H; G = ZK + UL;
    /// ZK = Z + K; UL = U + L; OH = H + O — 6 base leaves under XGH2O's own subtree via G-compound,
    /// plus the direct H/O/X. The value is deterministic + monotone up the chain.
    #[test]
    fn mineral_value_is_a_monotone_cost_of_production_floor() {
        use Compound::*;
        assert_eq!(mineral_value_e_milli(Hydrogen), BASE_MINERAL_VALUE_E_MILLI, "a base mineral is its mining floor");
        // OH = H + O + 1 step.
        assert_eq!(mineral_value_e_milli(Hydroxide), 2 * BASE_MINERAL_VALUE_E_MILLI + REACTION_STEP_OVERHEAD_E_MILLI);
        // A boost compound is strictly more valuable than any of its reagents (monotone up the tree).
        assert!(mineral_value_e_milli(XGH2O) > mineral_value_e_milli(GH2O));
        assert!(mineral_value_e_milli(GH2O) > mineral_value_e_milli(GH));
        assert!(mineral_value_e_milli(GH) > mineral_value_e_milli(Ghodium));
        // The f64 boundary form is the milli value / 1000.
        assert!((mineral_value_e(XGH2O) - mineral_value_e_milli(XGH2O) as f64 / 1000.0).abs() < 1e-9);

        // XGH2O's exact leaf/step count: leaves = X + (G:{ZK:{Z,K}, UL:{U,L}} + H) + (OH:{H,O})
        // → X, Z, K, U, L, H, H, O = 8 base leaves; interior reaction nodes = OH, ZK, UL, G, GH,
        // GH2O, XGH2O = 7 steps.
        let expected = 8 * BASE_MINERAL_VALUE_E_MILLI + 7 * REACTION_STEP_OVERHEAD_E_MILLI;
        assert_eq!(mineral_value_e_milli(XGH2O), expected, "the full XGH2O tree = 8 leaves + 7 steps");
    }

    /// The lab-load bid (§D1): a boost compound prices real infrastructure demand above the
    /// numeraire but bounded by the cap; a base mineral loads at (at least) par.
    #[test]
    fn lab_load_bid_is_bounded_and_above_par() {
        use crate::sink_economics::STORAGE_BID;
        let xg = lab_load_bid(Compound::XGH2O, LAB_LOAD_BID_CAP_MILLI);
        assert!(xg > STORAGE_BID, "loading toward a boost compound bids above the numeraire");
        assert!(xg <= LAB_LOAD_BID_CAP_MILLI, "…but is capped below the refill/survival lanes");
        // A base mineral load is never below par (it still unlocks a product).
        assert!(lab_load_bid(Compound::Hydrogen, LAB_LOAD_BID_CAP_MILLI) >= STORAGE_BID);
        // The cap floors at the numeraire even if passed something tiny (defensive).
        assert_eq!(lab_load_bid(Compound::Hydrogen, 0), STORAGE_BID);
    }
}
