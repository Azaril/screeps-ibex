//! Source Keeper room exploitation — the economy-family operation (ADR 0018).
//!
//! SK rooms are unclaimable, controller-less rooms (the ring around each sector
//! centre) holding 3 sources (4000/regen — the highest-yield remote) + 1
//! mineral, each guarded by a 5000-HP keeper that respawns 300t after damage.
//! This operation decides *which* adjacent SK rooms are worth farming (the pure
//! ROI scorer below, P2.K2a) and owns a persistent `SourceKeeperFarmMission`
//! that suppresses the keepers and mines around them (P2.K2b/c).
//!
//! The scorer is **pure arithmetic** (no `game::*`) so the commit/withhold/veto
//! gate is unit-testable against hand-computed energy/tick — see ADR 0018 §3.2.

// ─── ROI scorer (P2.K2a) — pure, kernel-testable ────────────────────────────

// Engine values (mirrored locally so the scorer stays a pure kernel; provenance
// in the doc names). See `docs/references/engine-mechanics.md`.
/// `SOURCE_ENERGY_KEEPER_CAPACITY` — an SK source holds 4000 per regen cycle.
const SK_SOURCE_CAPACITY: f64 = 4000.0;
/// `ENERGY_REGEN_TIME` — sources refill on a 300-tick timer.
const SOURCE_REGEN_TICKS: f64 = 300.0;
/// `HARVEST_POWER` — energy harvested per WORK part per tick.
const HARVEST_POWER: f64 = 2.0;
/// `CARRY_CAPACITY` — resources a CARRY part holds.
const CARRY_CAPACITY: f64 = 50.0;
/// `CREEP_LIFE_TIME` — body-spawn cost amortizes over this many ticks.
const CREEP_LIFETIME: f64 = 1500.0;
/// `BODYPART_COST` for WORK / CARRY / MOVE (the parts the model sizes).
const WORK_COST: f64 = 100.0;
const CARRY_COST: f64 = 50.0;
const MOVE_COST: f64 = 50.0;
/// Rough per-tile pathfinding-CPU charge (in energy-equivalent e/t) so distant
/// farms are penalised even when the haul body alone would pencil out.
const CPU_PENALTY_PER_TILE: f64 = 0.02;

/// Net e/t a candidate must clear to *start* a farm. Tunable (sim-calibrated).
pub const MIN_SK_ROI: f64 = 5.0;
/// Hysteresis band: an already-committed farm is only withdrawn once net falls
/// below `MIN_SK_ROI − SK_ROI_HYSTERESIS`, so a farm never flaps on a marginal
/// estimate or a transient spawn-pressure blip.
pub const SK_ROI_HYSTERESIS: f64 = 3.0;

/// Inputs to the SK-farm decision — all derivable from intel + already-tracked
/// state by the operation (§3.2). No `game::*`.
#[derive(Debug, Clone, Copy)]
pub struct SkRoiInputs {
    /// Harvestable sources in the SK room (typically 3).
    pub live_sources: u32,
    /// One-way path length (tiles) to the nearest capable home.
    pub haul_tiles: u32,
    /// Total body-energy of the suppression duo (`duo_sk_farmer`) — the op
    /// computes this from the composition; amortized over a creep lifetime.
    pub suppression_cost: u32,
    /// The nearest home can actually spawn the farm's creeps (op-computed).
    pub affordable: bool,
    /// Another player is farming it / hostile reservation / non-keeper
    /// hostiles — a hard veto (we don't contest an SK farm).
    pub contested: bool,
    /// CPU tier is not critical (ADR 0004).
    pub cpu_ok: bool,
    /// Military capacity is not wanted for active defense / declared war (ADR 0014).
    pub military_free: bool,
    /// Below `max_concurrent_sk_farms` (only gates *new* commitments).
    pub under_farm_cap: bool,
    /// We are already farming this room — applies the withdraw-hysteresis floor.
    pub already_committed: bool,
    /// An invader **stronghold** (level≥1 core) is present in the SK room — its
    /// towers + defenders make the whole room lethal. A hard veto on *starting*
    /// a farm; an already-committed farm pauses itself in-mission (it stays
    /// alive through the transient stronghold rather than tearing down).
    pub stronghold_present: bool,
}

/// The commit/withhold/veto verdict for one candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkRoiDecision {
    /// Start (or keep) the farm.
    Commit,
    /// Profitable enough is not met — don't farm (but no hard blocker).
    Withhold,
    /// A hard gate failed (contested / CPU / military / unaffordable / over cap).
    Veto,
}

/// Scored candidate: the gross + net energy/tick and the verdict.
#[derive(Debug, Clone, Copy)]
pub struct SkRoiScore {
    pub gross_per_tick: f64,
    pub net_per_tick: f64,
    pub decision: SkRoiDecision,
}

/// Score one SK-room candidate. Pure: hard gates first, then the energy/tick
/// model `net = gross − suppression − mining − haul − cpu`, then the
/// floor + hysteresis decision (ADR 0018 §3.2).
pub fn score_sk_farm(inp: &SkRoiInputs) -> SkRoiScore {
    // Hard gates — no farm regardless of yield.
    let vetoed = inp.contested
        || inp.stronghold_present
        || !inp.cpu_ok
        || !inp.military_free
        || !inp.affordable
        || (!inp.already_committed && !inp.under_farm_cap);
    if vetoed {
        return SkRoiScore { gross_per_tick: 0.0, net_per_tick: 0.0, decision: SkRoiDecision::Veto };
    }

    let n = inp.live_sources as f64;
    let gross = n * SK_SOURCE_CAPACITY / SOURCE_REGEN_TICKS;

    // Suppression: the duo's body cost, renewed each lifetime.
    let suppression = inp.suppression_cost as f64 / CREEP_LIFETIME;

    // Mining: WORK to saturate the yield + ~2 MOVE per source-miner, amortized.
    let work_parts = (gross / HARVEST_POWER).ceil();
    let mining_body = work_parts * WORK_COST + n * 2.0 * MOVE_COST;
    let mining = mining_body / CREEP_LIFETIME;

    // Haul: CARRY to move `gross` over a `2 × dist` round trip (+ matched MOVE),
    // amortized. This is the term that grows with distance and kills far farms.
    let carry_parts = gross * 2.0 * inp.haul_tiles as f64 / CARRY_CAPACITY;
    let haul_body = carry_parts * (CARRY_COST + MOVE_COST);
    let haul = haul_body / CREEP_LIFETIME;

    let cpu_penalty = inp.haul_tiles as f64 * CPU_PENALTY_PER_TILE;

    let net = gross - suppression - mining - haul - cpu_penalty;

    let floor = if inp.already_committed { MIN_SK_ROI - SK_ROI_HYSTERESIS } else { MIN_SK_ROI };
    let decision = if net >= floor { SkRoiDecision::Commit } else { SkRoiDecision::Withhold };

    SkRoiScore { gross_per_tick: gross, net_per_tick: net, decision }
}

// ─── Operation (P2.K2b) ─────────────────────────────────────────────────────

use super::data::*;
use super::operationsystem::*;
use crate::missions::data::*;
use crate::missions::sourcekeeperfarm::{sk_room_has_stronghold, SourceKeeperFarmMission};
use crate::room::gather::*;
use crate::room::visibilitysystem::*;
use crate::serialize::*;
use crate::visualization::SummaryContent;
use log::*;
use screeps::*;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;

/// Estimated total body-energy of the `duo_sk_farmer` (SK ranged attacker
/// 10RA+10MOVE+1HEAL ≈ 2250, SK healer 10HEAL+12MOVE ≈ 3100), for the
/// suppression e/t term. Refined to the real composition cost in K2c.
const SK_DUO_BODY_COST: u32 = 5350;
/// Largest single duo body (the healer) — the home must spawn it in one piece.
const SK_DUO_MAX_BODY_COST: u32 = 3100;
/// Min home RCL to consider an SK farm at all (the affordability check below is
/// the real gate; this just trims the home set cheaply).
const SK_HOME_MIN_RCL: u32 = 6;
/// Scan cadence offset (spread CPU vs other throttled operations).
const SK_SCAN_OFFSET: u32 = 35;
/// Tiles per room-hop, for the haul-distance estimate.
const TILES_PER_ROOM: u32 = 50;

#[derive(Clone, ConvertSaveload)]
pub struct SourceKeeperOperation {
    owner: EntityOption<Entity>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl SourceKeeperOperation {
    pub fn build<B>(builder: B, owner: Option<Entity>) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let operation = SourceKeeperOperation::new(owner);
        builder.with(OperationData::SourceKeeper(operation)).marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>) -> SourceKeeperOperation {
        SourceKeeperOperation { owner: owner.into() }
    }

    /// `SkFrontier` candidate (ADR 0018 §3.1): a scouted SK room (static lairs)
    /// with sources is a viable farm target; the BFS never expands *through* an
    /// SK or hostile-owned room (they are targets/walls), but does traverse
    /// neutral rooms to reach an SK ring a couple hops out.
    fn gather_candidate_room_data(gather_system_data: &GatherSystemData, room_name: RoomName) -> Option<CandidateRoomData> {
        let room_entity = gather_system_data.mapping.get_room(&room_name)?;
        let room_data = gather_system_data.room_data.get(room_entity)?;

        let static_visibility_data = room_data.get_static_visibility_data()?;
        let dynamic_visibility_data = room_data.get_dynamic_visibility_data()?;

        let is_sk = static_visibility_data.is_source_keeper();
        let has_sources = !static_visibility_data.sources().is_empty();

        let viable = is_sk && has_sources;
        let can_expand = !is_sk && !dynamic_visibility_data.owner().hostile();

        Some(CandidateRoomData::new(room_entity, viable, can_expand))
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Operation for SourceKeeperOperation {
    fn get_owner(&self) -> &Option<Entity> {
        &self.owner
    }

    fn owner_complete(&mut self, owner: Entity) {
        assert!(Some(owner) == *self.owner);
        self.owner.take();
    }

    fn describe_operation(&self, _ctx: &OperationDescribeContext) -> SummaryContent {
        SummaryContent::Text("Source Keeper".to_string())
    }

    fn run_operation(
        &mut self,
        system_data: &mut OperationExecutionSystemData,
        runtime_data: &mut OperationExecutionRuntimeData,
    ) -> Result<OperationResult, ()> {
        // Master kill-switch (ADR 0018 §3.5) — default TRUE (validated behaviors run on).
        let sk_features = system_data.features.source_keeper;
        // Observability: the SK scan was previously silent unless the (exec-only) `diagnostics` flag was
        // set, making "why isn't it farming?" undebuggable. Mirror the War scan — log the scan shape +
        // per-candidate decision under the always-available `military.debug_log` toggle too.
        let sk_debug = sk_features.diagnostics || system_data.features.military.debug_log;
        if !sk_features.farming {
            if sk_debug {
                info!("[SK] scan: farming feature OFF (Memory._features.source_keeper.farming)");
            }
            return Ok(OperationResult::Running);
        }
        if game::time() % 50 != SK_SCAN_OFFSET {
            return Ok(OperationResult::Running);
        }

        let gather_system_data = GatherSystemData {
            entities: system_data.entities,
            mapping: system_data.mapping,
            room_data: system_data.room_data,
            room_plan_data: system_data.room_plan_data,
            room_status_cache: system_data.room_status_cache,
            derelict_features: system_data.features.derelict,
        };

        let home_rooms = gather_home_rooms(&gather_system_data, SK_HOME_MIN_RCL);
        if home_rooms.is_empty() {
            if sk_debug {
                info!("[SK] scan: no home rooms >= RCL{} -> no farming (need a higher-RCL colony room)", SK_HOME_MIN_RCL);
            }
            return Ok(OperationResult::Running);
        }

        let gathered = gather_candidate_rooms(&gather_system_data, &home_rooms, sk_features.max_range, Self::gather_candidate_room_data);

        if sk_debug {
            info!(
                "[SK] scan: {} home(s) >= RCL{}, range {}, {} viable candidate(s), {} unscouted frontier room(s)",
                home_rooms.len(),
                SK_HOME_MIN_RCL,
                sk_features.max_range,
                gathered.candidate_rooms().len(),
                gathered.unknown_rooms().len()
            );
        }

        // Scout the frontier so unscouted ring rooms become viable candidates.
        for unknown_room in gathered.unknown_rooms().iter() {
            system_data.visibility.request(VisibilityRequest::new(
                unknown_room.room_name(),
                VISIBILITY_PRIORITY_MEDIUM,
                VisibilityRequestFlags::ALL,
            ));
        }

        // Tick-wide CPU gate (same for every candidate this scan).
        let cpu_ok = system_data.governor.tier != crate::cpugovernor::Tier::Critical;

        // Farms already committed on the rooms we can see this scan — the
        // concurrency cap. Within-scan creations bump it so we never over-commit
        // in one pass.
        let mut active_farm_count = gathered
            .candidate_rooms()
            .iter()
            .filter(|candidate| {
                system_data
                    .room_data
                    .get(candidate.room_data_entity())
                    .map(|room_data| {
                        room_data
                            .get_missions()
                            .iter()
                            .any(|m| system_data.mission_data.get(*m).as_mission_type::<SourceKeeperFarmMission>().is_some())
                    })
                    .unwrap_or(false)
            })
            .count() as u32;

        for candidate in gathered.candidate_rooms().iter() {
            let candidate_entity = candidate.room_data_entity();

            // Read intel into copies (immutable borrows) so the `get_mut` to
            // attach the mission below does not overlap.
            let intel = (|| {
                let room_data = system_data.room_data.get(candidate_entity)?;
                let static_visibility_data = room_data.get_static_visibility_data()?;
                let dynamic_visibility_data = room_data.get_dynamic_visibility_data();

                let live_sources = static_visibility_data.sources().len() as u32;
                let contested = dynamic_visibility_data
                    .map(|d| d.owner().hostile() || d.reservation().hostile())
                    .unwrap_or(false);
                let stronghold = sk_room_has_stronghold(room_data);
                let already_committed = room_data
                    .get_missions()
                    .iter()
                    .any(|m| system_data.mission_data.get(*m).as_mission_type::<SourceKeeperFarmMission>().is_some());
                let home_capacity = candidate
                    .home_room_data_entities()
                    .iter()
                    .filter_map(|e| system_data.room_data.get(*e))
                    .filter_map(|home| game::rooms().get(home.name))
                    .map(|home| home.energy_capacity_available())
                    .max()
                    .unwrap_or(0);

                Some((room_data.name, live_sources, contested, stronghold, home_capacity, already_committed))
            })();
            let Some((room_name, live_sources, contested, stronghold, home_capacity, already_committed)) = intel else {
                continue;
            };

            let inputs = SkRoiInputs {
                live_sources,
                haul_tiles: candidate.distance() * TILES_PER_ROOM,
                suppression_cost: SK_DUO_BODY_COST,
                affordable: home_capacity >= SK_DUO_MAX_BODY_COST,
                contested,
                cpu_ok,
                military_free: true, // TODO(K2c-2/W): yield to active defense / declared war
                under_farm_cap: active_farm_count < sk_features.max_concurrent_farms,
                already_committed,
                stronghold_present: stronghold,
            };
            let score = score_sk_farm(&inputs);

            if sk_debug {
                info!(
                    "[SK] candidate {}: {} sources @ {} tiles, gross {:.1} net {:.1} e/t | affordable={} contested={} stronghold={} cpu_ok={} under_cap={} committed={} -> {:?}",
                    room_name,
                    live_sources,
                    inputs.haul_tiles,
                    score.gross_per_tick,
                    score.net_per_tick,
                    inputs.affordable,
                    inputs.contested,
                    inputs.stronghold_present,
                    inputs.cpu_ok,
                    inputs.under_farm_cap,
                    inputs.already_committed,
                    score.decision
                );
            }

            // Commit → create the persistent farm mission (idempotent: only if the
            // room has none yet). Withhold/Veto retirement of an existing farm is
            // the next increment (K2c-2).
            if score.decision == SkRoiDecision::Commit && !already_committed {
                info!("Starting source keeper farm for room {}", room_name);

                let mission_entity = SourceKeeperFarmMission::build(
                    system_data.updater.create_entity(system_data.entities),
                    Some(runtime_data.entity),
                    candidate_entity,
                    candidate.home_room_data_entities(),
                )
                .build();

                if let Some(room_data) = system_data.room_data.get_mut(candidate_entity) {
                    room_data.add_mission(mission_entity);
                }
                active_farm_count += 1;
            }
        }

        Ok(OperationResult::Running)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A clean 3-source candidate with all gates open and a typical duo cost.
    fn nearby() -> SkRoiInputs {
        SkRoiInputs {
            live_sources: 3,
            haul_tiles: 50, // one room away
            suppression_cost: 5350,
            affordable: true,
            contested: false,
            cpu_ok: true,
            military_free: true,
            under_farm_cap: true,
            already_committed: false,
            stronghold_present: false,
        }
    }

    #[test]
    fn a_close_three_source_room_commits_and_is_strongly_positive() {
        let s = score_sk_farm(&nearby());
        assert_eq!(s.decision, SkRoiDecision::Commit);
        // gross = 3 * 4000 / 300 = 40 e/t; net well above the floor.
        assert!((s.gross_per_tick - 40.0).abs() < 1e-9);
        assert!(s.net_per_tick > 20.0, "net {}", s.net_per_tick);
    }

    #[test]
    fn distance_eventually_makes_it_unprofitable() {
        let close = score_sk_farm(&nearby());
        let far = score_sk_farm(&SkRoiInputs { haul_tiles: 600, ..nearby() });
        assert!(far.net_per_tick < close.net_per_tick, "haul cost must grow with distance");
        assert_eq!(far.decision, SkRoiDecision::Withhold, "a far SK room is not worth the haul");
    }

    #[test]
    fn hard_gates_veto_regardless_of_yield() {
        for bad in [
            SkRoiInputs { contested: true, ..nearby() },
            SkRoiInputs { stronghold_present: true, ..nearby() },
            SkRoiInputs { cpu_ok: false, ..nearby() },
            SkRoiInputs { military_free: false, ..nearby() },
            SkRoiInputs { affordable: false, ..nearby() },
            SkRoiInputs { under_farm_cap: false, ..nearby() }, // not yet committed + over cap
        ] {
            assert_eq!(score_sk_farm(&bad).decision, SkRoiDecision::Veto);
        }
    }

    #[test]
    fn an_existing_farm_keeps_its_slot_over_the_concurrency_cap() {
        // Over the cap but already committed → the cap no longer vetoes it.
        let existing = SkRoiInputs { under_farm_cap: false, already_committed: true, ..nearby() };
        assert_eq!(score_sk_farm(&existing).decision, SkRoiDecision::Commit);
    }

    #[test]
    fn a_stronghold_vetoes_even_an_established_farm() {
        // Unlike the concurrency cap (which an existing farm is grandfathered
        // past), an invader stronghold is a hard veto regardless of commitment:
        // the operation must never (re)create a farm into a lethal room. The
        // running mission pauses itself in-mission rather than relying on this.
        let committed = SkRoiInputs { stronghold_present: true, already_committed: true, ..nearby() };
        assert_eq!(score_sk_farm(&committed).decision, SkRoiDecision::Veto);
    }

    #[test]
    fn hysteresis_keeps_a_marginal_farm_but_will_not_start_one() {
        // ~250 tiles lands net ≈ 3.2 e/t — inside the band [MIN−h, MIN) = [2, 5).
        let marginal = SkRoiInputs { haul_tiles: 250, ..nearby() };
        let fresh = score_sk_farm(&marginal);
        let committed = score_sk_farm(&SkRoiInputs { already_committed: true, ..marginal });

        assert!(
            fresh.net_per_tick >= MIN_SK_ROI - SK_ROI_HYSTERESIS && fresh.net_per_tick < MIN_SK_ROI,
            "expected a band net, got {}",
            fresh.net_per_tick
        );
        assert_eq!(fresh.decision, SkRoiDecision::Withhold, "won't START a marginal farm");
        assert_eq!(committed.decision, SkRoiDecision::Commit, "won't DROP an existing marginal farm");
    }
}
