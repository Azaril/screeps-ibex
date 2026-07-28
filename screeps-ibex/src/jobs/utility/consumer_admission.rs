//! ADR 0044 A3 (Defect 2) — the LIVE Use-lane withdraw admission for consumer creeps.
//!
//! Consumer creeps (upgraders/builders/repairers) are *second-class* Use-lane withdrawers: under a
//! refill deficit their draw should be SHED so they stop draining the container the refill hauler
//! needs. The mechanism is the opportunity floor — a consumer whose downstream sink bid clears the
//! room's floor is admitted; one whose bid falls below it is priced out. This mirrors the sim's
//! consumer loop (`screeps-econ-eval::runner` `Role::Upgrader`/`Role::Builder`:
//! `rt.veto || admit_use_withdraw(rt.upgrade_sink_bid(world), rt.floor)`), which was the design all
//! along — the wrappers in `market_adapter` were only ever called under `#[cfg(test)]`, so the live
//! bot never shed. These helpers wire the SAME `econ::admit_use_withdraw` on the RAW (unscaled)
//! sink bid, gated against the floor published by `publish_market_floor` at the top of the hauling
//! pass (read here from [`MarketBidSummary`]).
//!
//! This is the SHIPPED, always-on live behavior (no config toggle). The sim's `a3_live_control`
//! flag is a validation-only inverse control that reverts it — never a live switch.
//!
//! The gate is behavior-only: it stores NO serialized floor snapshot (it reads the per-tick floor
//! at selection time), so it carries no WFV bump.

use crate::room::data::RoomData;
use crate::transfer::transfersystem::MarketBidSummary;
use screeps::*;
use screeps_econ_decision::sink_economics::{self as econ, MarketConsts};

/// The published opportunity floor for a room (milli-e/t), or 0 when nothing is materially unmet
/// (⇒ every withdraw admits). Read straight off the per-tick [`MarketBidSummary`].
pub fn room_floor(market_bids: &MarketBidSummary, room_name: RoomName) -> u32 {
    market_bids.rooms.get(&room_name).map(|r| r.opportunity_floor).unwrap_or(0)
}

/// The RAW (fullness-UNSCALED) upgrade sink bid for a room — `V_UPGRADE` + the near-level-up step
/// premium — mirroring the sim's `upgrade_sink_bid`. This is the bid an upgrader's own draw is
/// admitted against (NOT the buffer-scaled deposit bid; ADR 0044 §1 — do not merge the two).
/// `None` when there is no visible owned controller.
pub fn upgrade_sink_bid(room_data: &RoomData, consts: &MarketConsts) -> Option<u32> {
    let structures = room_data.get_structures()?;
    let controller = structures.controllers().iter().max_by_key(|c| c.level())?;
    let near_level_up = controller_levels(controller.level() as u32)
        .is_some_and(|need| need.saturating_sub(controller.progress().unwrap_or(0)) <= consts.upgrade_step_window_progress);
    Some(econ::upgrade_bid(consts, near_level_up))
}

/// The downgrade survival veto (§D1 guardrail): a room whose controller clock is below
/// `downgrade_veto_q` of the full clock admits controller supply regardless of the floor (a veto
/// OUTSIDE the market — the upgrader must never starve into a downgrade). Mirrors the sim's
/// `rt.veto`.
pub fn downgrade_veto(room_data: &RoomData, consts: &MarketConsts) -> bool {
    let Some(structures) = room_data.get_structures() else {
        return false;
    };
    structures.controllers().iter().any(|c| {
        if c.level() == 0 {
            return false;
        }
        match (c.ticks_to_downgrade(), controller_downgrade(c.level())) {
            (Some(ttd), Some(full_clock)) => econ::downgrade_veto(consts, ttd, full_clock),
            _ => false,
        }
    })
}

/// True iff an UPGRADER's Use-lane draw is admitted this tick: the downgrade veto fires, or the raw
/// upgrade sink bid clears the room's opportunity floor. When the upgrade sink bid is unknown (no
/// visible controller) the draw admits (fail-open — the sim's `market == None ⇒ true`).
pub fn upgrader_withdraw_admitted(room_data: &RoomData, market_bids: &MarketBidSummary) -> bool {
    let consts = MarketConsts::default();
    if downgrade_veto(room_data, &consts) {
        return true;
    }
    match upgrade_sink_bid(room_data, &consts) {
        Some(bid) => econ::admit_use_withdraw(bid, room_floor(market_bids, room_data.name)),
        None => true,
    }
}

/// Map a live screeps `StructureType` to the sim's `BuildClass`, mirroring `market::build_class`.
/// `None` for structure types outside the priced vocabulary (walls, ramparts, links, labs, …) —
/// the builder self-fetch treats an unpriced site as fail-open (admits), never shedding work whose
/// EV this currency does not model.
fn build_class(structure_type: StructureType) -> Option<econ::BuildClass> {
    Some(match structure_type {
        StructureType::Spawn => econ::BuildClass::Spawn,
        StructureType::Extension => econ::BuildClass::Extension,
        StructureType::Road => econ::BuildClass::Road,
        StructureType::Container => econ::BuildClass::Container,
        StructureType::Storage => econ::BuildClass::Storage,
        StructureType::Tower => econ::BuildClass::Tower,
        _ => return None,
    })
}

/// True iff a BUILDER's Use-lane self-fetch is admitted this tick — the builder's best downstream
/// sink bid clears the room's opportunity floor (ADR 0044 §2 "gate the build self-fetch pickup").
///
/// The builder's sink is build/repair. The live path prices the BUILD side exactly by the per-class
/// `build_bid` over the pending construction sites (cheap, matches the sim's `site_build_bid`).
/// `has_repair_target` reports whether the repair selection would pick a target this tick — a
/// selected (or survival-override) repair clears the floor by construction (mirroring the sim's
/// `b.max(rt.floor)` for an admitted repair target), so it always admits. An unpriced-class site
/// (wall/rampart/link/…) also admits (fail-open — this currency does not model its EV). The full
/// per-structure `repair_bid` is a separate, larger adapter (see the ADR 0044 §2 residual note);
/// this gate deliberately does NOT invent live repair economics.
pub fn builder_withdraw_admitted(
    room_data: &RoomData,
    market_bids: &MarketBidSummary,
    best_site_type: Option<StructureType>,
    has_repair_target: bool,
) -> bool {
    builder_admitted(
        best_site_type,
        has_repair_target,
        room_floor(market_bids, room_data.name),
        &MarketConsts::default(),
    )
}

/// The pure builder-admission decision (floor passed in — testable without a live `RoomData`).
pub fn builder_admitted(best_site_type: Option<StructureType>, has_repair_target: bool, floor: u32, consts: &MarketConsts) -> bool {
    // A pending repair target always clears the floor (it was chosen by the repair selection's own
    // minimum priorities / survival overrides).
    if has_repair_target {
        return true;
    }
    match best_site_type {
        // An unpriced-class site: fail-open (don't shed EV we can't price).
        Some(t) => match build_class(t) {
            Some(class) => econ::admit_use_withdraw(econ::build_bid(consts, class), floor),
            None => true,
        },
        // No pending build site AND no repair target: nothing to fetch for — admit (the pickup
        // path is only reached when the builder has no energy; if there is genuinely no work the
        // downstream cascade falls through to harvest/wait anyway).
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use screeps_econ_decision::sink_economics::{refill_bid, upgrade_bid, SURVIVAL_BID};

    /// (b) The UPGRADE draw is DECLINED under a deep refill deficit (the stressed refill bid is the
    /// floor and out-prices the upgrade sink), and ADMITTED when the room is healthy (floor at/below
    /// the upgrade bid). This is the exact `admit_use_withdraw(upgrade_sink_bid, floor)` the live
    /// upgrader gate applies.
    #[test]
    fn upgrade_withdraw_declined_under_deep_deficit_admitted_when_healthy() {
        let consts = MarketConsts::default();
        let up_bid = upgrade_bid(&consts, false); // V_UPGRADE, no step premium
        // A deep post-wipe refill deficit: the refill bid caps well above the upgrade bid.
        let deep_floor = refill_bid(&consts, Some(12_000), 550, 250);
        assert!(deep_floor > up_bid, "sanity: the stressed refill floor exceeds the upgrade bid");
        assert!(
            !econ::admit_use_withdraw(up_bid, deep_floor),
            "the upgrade draw is SHED under a deep refill deficit"
        );

        // Healthy room: floor at par (storage), below the upgrade bid ⇒ admitted.
        let healthy_floor = econ::STORAGE_BID;
        assert!(healthy_floor < up_bid);
        assert!(
            econ::admit_use_withdraw(up_bid, healthy_floor),
            "the upgrade draw is admitted when the floor is low"
        );

        // A survival-priced sink (the downgrade-veto analog: SURVIVAL_BID) is above any floor.
        assert!(econ::admit_use_withdraw(SURVIVAL_BID, deep_floor), "survival bids clear any floor");
    }

    /// (b) A BUILDER shed decision: a low-value ROAD build is DECLINED under a deep deficit but a
    /// pending repair target (or an unpriced-class site) always ADMITS.
    #[test]
    fn builder_shed_and_bypass() {
        let consts = MarketConsts::default();
        let deep_floor = refill_bid(&consts, Some(12_000), 550, 250);

        // A road build (a low per-class bid) is shed under the deep deficit.
        assert!(
            !builder_admitted(Some(StructureType::Road), false, deep_floor, &consts),
            "a low-value road build is shed under a deep refill deficit"
        );
        // The same road build admits when the floor is low (healthy room).
        assert!(builder_admitted(Some(StructureType::Road), false, 0, &consts));
        // A pending repair target always admits (it cleared the repair selection's own gate).
        assert!(builder_admitted(Some(StructureType::Road), true, deep_floor, &consts));
        // An unpriced-class site (rampart) fails open (EV not modelled in this currency).
        assert!(builder_admitted(Some(StructureType::Rampart), false, deep_floor, &consts));
        // No work at all: admit (nothing to shed; the cascade falls through downstream).
        assert!(builder_admitted(None, false, deep_floor, &consts));
    }

    /// `build_class` mirrors the sim's `build_class` for the priced vocabulary and returns `None`
    /// (fail-open) for anything outside it.
    #[test]
    fn build_class_maps_priced_vocabulary() {
        assert_eq!(build_class(StructureType::Spawn), Some(econ::BuildClass::Spawn));
        assert_eq!(build_class(StructureType::Tower), Some(econ::BuildClass::Tower));
        assert_eq!(build_class(StructureType::Road), Some(econ::BuildClass::Road));
        assert_eq!(build_class(StructureType::Wall), None);
        assert_eq!(build_class(StructureType::Rampart), None);
    }
}
