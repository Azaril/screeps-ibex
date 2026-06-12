use super::data::*;
use super::operationsystem::*;
use crate::missions::constants::*;
use crate::missions::salvage::*;
use crate::missions::utility::*;
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

/// Route-hop distance within which a salvage target with sources counts as
/// "strategic": clearing it enables a future mining outpost, so the energy EV
/// margin is bypassed. Matches `MiningOutpostOperation`'s exit-BFS search
/// depth (gather max_distance 1) — measured in route hops rather than linear
/// distance because a diagonal neighbour is 2 exits away and NOT outpostable.
const STRATEGIC_OUTPOST_HOPS: u32 = 1;

/// EV cost-model assumptions, deliberately coarse (EP-4.6: named, reviewable).
/// Raider: ~10×[Carry, Move] → 500 capacity for ~1000 energy.
const ASSUMED_RAIDER_CAPACITY: u32 = 500;
const ASSUMED_RAIDER_BODY_COST: u32 = 1_000;
/// Dismantler: ~10 Work plus carry/move support per the salvage dismantler
/// body definition (~310 energy per Work part all-in).
const ASSUMED_DISMANTLER_WORK_PARTS: u32 = 10;
const ASSUMED_DISMANTLER_BODY_COST: u32 = 3_100;
/// Spawn lead applied on top of travel when estimating how much decaying
/// value will still exist by the time creeps arrive.
const ASSUMED_SPAWN_LEAD_TICKS: u32 = 150;

/// Pure EV gate (host-tested): is the recoverable value worth the creep
/// spawn energy, given travel distance? Strategic rooms (future mining
/// outposts) bypass the margin — clearing them has value beyond the energy
/// recovered — but still require some actionable work.
pub(crate) fn salvage_worthwhile(
    work: &SalvageWork,
    travel_ticks: u32,
    strategic: bool,
    margin: f32,
    raid_enabled: bool,
    dismantle_enabled: bool,
) -> bool {
    let loot_total = work.loot_total();
    let lootable = raid_enabled && loot_total > 0;
    let dismantlable = dismantle_enabled && work.dismantle_hits > 0;

    if !lootable && !dismantlable {
        return false;
    }

    if strategic {
        return true;
    }

    // Value: stores at face value (minerals weighted as energy — they are at
    // least terminal-sellable), dismantled hits refund DISMANTLE_COST each.
    let mut value = 0.0f32;
    let mut cost = 0.0f32;

    if lootable {
        value += loot_total as f32;

        let round_trip_ticks = 2 * travel_ticks + 50;
        let trips = loot_total.div_ceil(ASSUMED_RAIDER_CAPACITY);
        let lifetimes = (trips * round_trip_ticks).div_ceil(CREEP_LIFE_TIME).max(1);
        cost += (lifetimes * ASSUMED_RAIDER_BODY_COST) as f32;
    }

    if dismantlable {
        value += work.dismantle_hits as f32 * DISMANTLE_COST;

        let work_ticks = work.dismantle_hits / (ASSUMED_DISMANTLER_WORK_PARTS * DISMANTLE_POWER);
        let effective_lifetime = CREEP_LIFE_TIME.saturating_sub(travel_ticks).max(300);
        let lifetimes = work_ticks.div_ceil(effective_lifetime).max(1);
        cost += (lifetimes * ASSUMED_DISMANTLER_BODY_COST) as f32;
    }

    value >= margin * cost
}

/// Memo of rooms that recently failed the EV gate, so the scan does not keep
/// re-scouting and re-evaluating them every pass.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SalvageRejection {
    room_name: RoomName,
    until_tick: u32,
}

/// Singleton operation and sole owner of loot/dismantle work (EP-2.7): scans
/// known rooms for salvage candidates — confirmed-derelict rooms and neutral
/// rooms with foreign remnants — applies the EV/strategic gate, and runs one
/// `SalvageMission` per admitted room.
#[derive(Clone, ConvertSaveload)]
pub struct SalvageOperation {
    owner: EntityOption<Entity>,
    salvage_missions: EntityVec<Entity>,
    rejected: Vec<SalvageRejection>,
    /// Rooms with a currently-running salvage mission. When the mission ends
    /// — success OR abort — the room moves onto the rejection memo for a
    /// cooldown: completed rooms have nothing worth re-evaluating soon, and
    /// abort conditions (re-armed, safe mode) need time to change. Bounded
    /// retry with backoff per EP-4.5; prevents create/abort churn.
    admitted: Vec<RoomName>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl SalvageOperation {
    pub fn build<B>(builder: B, owner: Option<Entity>) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let operation = SalvageOperation::new(owner);

        builder.with(OperationData::Salvage(operation)).marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>) -> SalvageOperation {
        SalvageOperation {
            owner: owner.into(),
            salvage_missions: EntityVec::new(),
            rejected: Vec::new(),
            admitted: Vec::new(),
        }
    }

    /// Home rooms able to spawn salvage creeps (own spawn, RCL >= 2).
    /// Mirrors `ScoutOperation::gather_home_rooms` rather than reusing
    /// `room::gather::gather_home_rooms`, which requires a full
    /// `GatherSystemData` this operation otherwise has no use for.
    fn gather_home_rooms(system_data: &OperationExecutionSystemData) -> Vec<(Entity, RoomName)> {
        let mut result = Vec::new();

        for (entity, room_data) in (system_data.entities, &*system_data.room_data).join() {
            if !is_valid_home_room(room_data) {
                continue;
            }

            let rcl = room_data
                .get_structures()
                .iter()
                .flat_map(|s| s.controllers())
                .map(|c| c.level() as u32)
                .max()
                .unwrap_or(0);

            if rcl >= 2 {
                result.push((entity, room_data.name));
            }
        }

        result
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Operation for SalvageOperation {
    fn get_owner(&self) -> &Option<Entity> {
        &self.owner
    }

    fn owner_complete(&mut self, owner: Entity) {
        assert!(Some(owner) == *self.owner);

        self.owner.take();
    }

    fn child_complete(&mut self, child: Entity) {
        self.salvage_missions.retain(|e| *e != child);
    }

    fn repair_entity_refs(&mut self, is_valid: &dyn Fn(Entity) -> bool) {
        self.salvage_missions.retain(|e| {
            let ok = is_valid(*e);
            if !ok {
                error!("INTEGRITY: dead salvage mission entity {:?} removed from SalvageOperation", e);
            }
            ok
        });
    }

    fn describe_operation(&self, _ctx: &OperationDescribeContext) -> SummaryContent {
        SummaryContent::Text(format!("Salvage - Missions: {}", self.salvage_missions.len()))
    }

    fn run_operation(
        &mut self,
        system_data: &mut OperationExecutionSystemData,
        runtime_data: &mut OperationExecutionRuntimeData,
    ) -> Result<OperationResult, ()> {
        let features = system_data.features;

        if !features.derelict.on {
            return Ok(OperationResult::Running);
        }

        // Neither child mission could spawn a creep — don't scan for work.
        if !features.raid && !features.dismantle {
            return Ok(OperationResult::Running);
        }

        if game::time() % 50 != 10 {
            return Ok(OperationResult::Running);
        }

        if !system_data.governor.can_execute_cpu(CpuBar::LowPriority) {
            return Ok(OperationResult::Running);
        }

        self.rejected.retain(|r| r.until_tick > game::time());

        // Rooms that already have an active salvage mission.
        let mut rooms_with_missions: std::collections::HashSet<RoomName> = std::collections::HashSet::new();
        for mission_entity in self.salvage_missions.iter() {
            if let Some(mission) = system_data.mission_data.get(*mission_entity) {
                let room_entity = mission.as_mission().get_room();
                if let Some(room_data) = room_entity.and_then(|e| system_data.room_data.get(e)) {
                    rooms_with_missions.insert(room_data.name);
                }
            }
        }

        // Missions that ended since the last scan (success or abort) put
        // their room on cooldown so persistent abort conditions cannot churn
        // a create/abort cycle every scan.
        let rejected = &mut self.rejected;
        let reject_cooldown = features.derelict.reject_cooldown;
        self.admitted.retain(|room_name| {
            if rooms_with_missions.contains(room_name) {
                true
            } else {
                info!("Salvage mission for room {} ended - cooling down for {} ticks", room_name, reject_cooldown);
                rejected.push(SalvageRejection {
                    room_name: *room_name,
                    until_tick: game::time() + reject_cooldown,
                });
                false
            }
        });

        if self.salvage_missions.len() >= features.derelict.salvage_max_missions as usize {
            return Ok(OperationResult::Running);
        }

        let home_rooms = Self::gather_home_rooms(system_data);

        if home_rooms.is_empty() {
            return Ok(OperationResult::Running);
        }

        // Pass 1: classify candidates from intel (immutable scan).
        let mut candidates: Vec<(Entity, RoomName)> = Vec::new();

        for (entity, room_data) in (system_data.entities, &*system_data.room_data).join() {
            if rooms_with_missions.contains(&room_data.name) {
                continue;
            }

            if self.rejected.iter().any(|r| r.room_name == room_data.name) {
                continue;
            }

            let Some(dynamic_visibility_data) = room_data.get_dynamic_visibility_data() else {
                continue;
            };

            if dynamic_visibility_data.source_keeper() || dynamic_visibility_data.safe_mode_active() {
                continue;
            }

            let min_range = home_rooms
                .iter()
                .map(|(_, home_name)| game::map::get_room_linear_distance(*home_name, room_data.name, false))
                .min()
                .unwrap_or(u32::MAX);

            if min_range > features.derelict.salvage_max_range {
                continue;
            }

            // Hostile-owned rooms must be confirmed derelict; neutral rooms
            // qualify when militarily quiet with foreign remnants observed.
            // Reserved neutral rooms are excluded: a hostile reservation means
            // invader cores or a defended enemy remote, a friendly one is not
            // ours to strip.
            let eligible = if dynamic_visibility_data.owner().hostile() {
                dynamic_visibility_data.confirmed_derelict(features.derelict.confirm_ticks, features.derelict.action_max_age)
            } else if dynamic_visibility_data.owner().neutral() {
                !dynamic_visibility_data.reservation().hostile()
                    && !dynamic_visibility_data.reservation().friendly()
                    && !dynamic_visibility_data.militarily_active()
                    && dynamic_visibility_data.updated_within(features.derelict.action_max_age)
                    && dynamic_visibility_data.hostile_structures()
            } else {
                false
            };

            if eligible {
                candidates.push((entity, room_data.name));
            }
        }

        // Pass 2: evaluate work + EV; admit up to the mission cap.
        let mut slots = (features.derelict.salvage_max_missions as usize).saturating_sub(self.salvage_missions.len());

        for (room_entity, room_name) in candidates {
            if slots == 0 {
                break;
            }

            let Some((home_entity, home_name)) = home_rooms
                .iter()
                .min_by_key(|(_, home_name)| game::map::get_room_linear_distance(*home_name, room_name, false))
                .copied()
            else {
                continue;
            };

            let Some(travel_ticks) = system_data.pathfinder.travel_ticks(home_name, room_name, game::time()) else {
                // Unreachable from the nearest home room; check back later.
                self.rejected.push(SalvageRejection {
                    room_name,
                    until_tick: game::time() + features.derelict.reject_cooldown,
                });
                continue;
            };

            // Work assessment needs live structure data.
            let work = {
                let Some(room_data) = system_data.room_data.get(room_entity) else {
                    continue;
                };

                match room_data.get_structures() {
                    Some(structures) => {
                        let sources = room_data
                            .get_static_visibility_data()
                            .map(|s| s.sources().as_slice())
                            .unwrap_or(&[]);

                        let room_owned = room_data
                            .get_dynamic_visibility_data()
                            .map(|d| d.owner().hostile())
                            .unwrap_or(false);

                        let lead_ticks = travel_ticks + ASSUMED_SPAWN_LEAD_TICKS;
                        let work = assess_salvage_work(
                            structures.all(),
                            sources,
                            features.derelict.max_structure_hits,
                            lead_ticks,
                            room_owned,
                        );
                        let strategic = !sources.is_empty() && travel_ticks <= 50 * STRATEGIC_OUTPOST_HOPS;

                        Some((work, strategic))
                    }
                    None => {
                        // Ask for eyes; a later pass evaluates while visible.
                        system_data.visibility.request(VisibilityRequest::new(
                            room_name,
                            VISIBILITY_PRIORITY_LOW,
                            VisibilityRequestFlags::ALL,
                        ));

                        None
                    }
                }
            };

            let Some((work, strategic)) = work else {
                continue;
            };

            if !salvage_worthwhile(
                &work,
                travel_ticks,
                strategic,
                features.derelict.dismantle_margin,
                features.raid,
                features.dismantle,
            ) {
                self.rejected.push(SalvageRejection {
                    room_name,
                    until_tick: game::time() + features.derelict.reject_cooldown,
                });
                continue;
            }

            info!(
                "Starting salvage mission for room {} (loot energy: {}, loot other: {}, dismantle hits: {}, travel: {}, strategic: {})",
                room_name, work.loot_energy, work.loot_other, work.dismantle_hits, travel_ticks, strategic
            );

            let mission_entity = SalvageMission::build(
                system_data.updater.create_entity(system_data.entities),
                Some(runtime_data.entity),
                room_entity,
                &[home_entity],
            )
            .build();

            self.salvage_missions.push(mission_entity);
            self.admitted.push(room_name);

            if let Some(room_data) = system_data.room_data.get_mut(room_entity) {
                room_data.add_mission(mission_entity);
            }

            slots -= 1;
        }

        Ok(OperationResult::Running)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work(loot_energy: u32, loot_other: u32, dismantle_hits: u32) -> SalvageWork {
        SalvageWork {
            loot_energy,
            loot_other,
            dismantle_hits,
        }
    }

    #[test]
    fn no_work_is_never_worthwhile() {
        assert!(!salvage_worthwhile(&work(0, 0, 0), 50, true, 2.0, true, true));
    }

    #[test]
    fn disabled_features_remove_value() {
        // Loot exists but raiding is disabled; dismantling enabled but no hits.
        assert!(!salvage_worthwhile(&work(50_000, 0, 0), 50, false, 2.0, false, true));
        // Hits exist but dismantling disabled.
        assert!(!salvage_worthwhile(&work(0, 0, 1_000_000), 50, false, 2.0, true, false));
    }

    #[test]
    fn strategic_rooms_bypass_margin_but_need_work() {
        // Tiny loot, adjacent future outpost: clearing it is worth it.
        assert!(salvage_worthwhile(&work(100, 0, 0), 50, true, 2.0, true, true));
        // Strategic but nothing actionable: still no.
        assert!(!salvage_worthwhile(&work(0, 0, 0), 50, true, 2.0, true, true));
    }

    #[test]
    fn rich_adjacent_room_clears_margin() {
        // 50k energy one room over: one raider lifetime (~1000 energy) recovers it.
        assert!(salvage_worthwhile(&work(50_000, 0, 0), 50, false, 2.0, true, true));
    }

    #[test]
    fn tiny_loot_far_away_fails_margin() {
        // 500 energy, 8 rooms away: a raider costs more than the loot is worth.
        assert!(!salvage_worthwhile(&work(500, 0, 0), 400, false, 2.0, true, true));
    }

    #[test]
    fn dismantle_energy_scales_with_hits() {
        // 2M hits -> ~10k energy refund vs 3 dismantler lifetimes (~9.3k
        // body cost): fails a 2.0 margin...
        assert!(!salvage_worthwhile(&work(0, 0, 2_000_000), 50, false, 2.0, true, true));
        // ...but the same hits WITH substantial loot in the same trip passes.
        assert!(salvage_worthwhile(&work(60_000, 0, 2_000_000), 50, false, 2.0, true, true));
    }

    #[test]
    fn minerals_count_as_loot_value() {
        assert!(salvage_worthwhile(&work(0, 50_000, 0), 50, false, 2.0, true, true));
    }
}
