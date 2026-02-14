use super::data::*;
use super::operationsystem::*;
use crate::missions::data::*;
use crate::missions::nuke_defense::*;
use crate::missions::safe_mode::*;
use crate::missions::squad_defense::*;
use crate::missions::wall_repair::*;
use crate::serialize::*;
use crate::visualization::SummaryContent;
use log::*;
use screeps::*;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;

/// Defense operation -- one per colony. Reads the ThreatMap and escalates
/// response by creating child defense missions as needed.
///
/// Creates:
/// - `SquadDefenseMission` when player hostiles are detected
/// - `NukeDefenseMission` when incoming nukes are detected
/// - `SafeModeMission` when critical structures are at risk
/// - `WallRepairMission` when walls/ramparts are under attack
#[derive(Clone, ConvertSaveload)]
pub struct DefenseOperation {
    owner: EntityOption<Entity>,
    last_run: Option<u32>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl DefenseOperation {
    pub fn build<B>(builder: B, owner: Option<Entity>) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let operation = DefenseOperation::new(owner);

        builder.with(OperationData::Defense(operation)).marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>) -> DefenseOperation {
        DefenseOperation {
            owner: owner.into(),
            last_run: None,
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Operation for DefenseOperation {
    fn get_owner(&self) -> &Option<Entity> {
        &self.owner
    }

    fn owner_complete(&mut self, owner: Entity) {
        assert!(Some(owner) == *self.owner);
        self.owner.take();
    }

    fn describe_operation(&self, _ctx: &OperationDescribeContext) -> SummaryContent {
        SummaryContent::Text("Defense".to_string())
    }

    fn run_operation(
        &mut self,
        system_data: &mut OperationExecutionSystemData,
        runtime_data: &mut OperationExecutionRuntimeData,
    ) -> Result<OperationResult, ()> {
        let features = crate::features::features();

        if !features.military.defense {
            return Ok(OperationResult::Running);
        }

        // Run every 10 ticks to avoid excessive CPU.
        let should_run = self.last_run.map(|t| game::time() - t >= 10).unwrap_or(true);

        if !should_run {
            return Ok(OperationResult::Running);
        }

        self.last_run = Some(game::time());

        // Collect home rooms (rooms with spawns) first, before the mutable loop.
        let home_rooms: Vec<Entity> = (system_data.entities, &*system_data.room_data)
            .join()
            .filter(|(_, rd)| {
                rd.get_dynamic_visibility_data()
                    .map(|d| d.owner().mine())
                    .unwrap_or(false)
                    && rd
                        .get_structures()
                        .map(|s| !s.spawns().is_empty())
                        .unwrap_or(false)
            })
            .map(|(e, _)| e)
            .collect();

        if home_rooms.is_empty() {
            return Ok(OperationResult::Running);
        }

        // ─── Phase 1: Collect rooms needing squad defense ──────────────────

        struct DefenseNeed {
            room_entity: Entity,
            estimated_dps: f32,
            estimated_heal: f32,
            hostile_count: usize,
            any_boosted: bool,
        }

        // Also track which rooms need nuke defense, safe mode, and wall repair.
        struct RoomDefenseState {
            room_entity: Entity,
            has_hostiles: bool,
            has_nukes: bool,
            has_nuke_defense_mission: bool,
            has_safe_mode_mission: bool,
            has_wall_repair_mission: bool,
        }

        let mut room_states: Vec<RoomDefenseState> = Vec::new();

        let rooms_needing_defense: Vec<DefenseNeed> = (system_data.entities, &*system_data.room_data)
            .join()
            .filter_map(|(entity, room_data)| {
                let dynamic_vis = room_data.get_dynamic_visibility_data()?;

                if !dynamic_vis.owner().mine() || !dynamic_vis.visible() {
                    return None;
                }

                let has_hostiles = dynamic_vis.hostile_creeps();

                // Check for nukes.
                let has_nukes = game::rooms()
                    .get(room_data.name)
                    .map(|room| !room.find(find::NUKES, None).is_empty())
                    .unwrap_or(false);

                // Check existing missions.
                let missions = room_data.get_missions();
                let has_nuke_defense_mission = missions.iter().any(|me| {
                    system_data
                        .mission_data
                        .get(*me)
                        .as_mission_type::<NukeDefenseMission>()
                        .is_some()
                });
                let has_safe_mode_mission = missions.iter().any(|me| {
                    system_data
                        .mission_data
                        .get(*me)
                        .as_mission_type::<SafeModeMission>()
                        .is_some()
                });
                let has_wall_repair_mission = missions.iter().any(|me| {
                    system_data
                        .mission_data
                        .get(*me)
                        .as_mission_type::<WallRepairMission>()
                        .is_some()
                });

                room_states.push(RoomDefenseState {
                    room_entity: entity,
                    has_hostiles,
                    has_nukes,
                    has_nuke_defense_mission,
                    has_safe_mode_mission,
                    has_wall_repair_mission,
                });

                // Squad defense logic only for player hostiles.
                if !has_hostiles {
                    return None;
                }

                let creeps = room_data.get_creeps()?;
                let hostiles: Vec<_> = creeps
                    .hostile()
                    .iter()
                    .filter(|c| {
                        let owner = c.owner().username();
                        owner != "Invader" && owner != "Source Keeper"
                    })
                    .collect();

                if hostiles.is_empty() {
                    return None;
                }

                // Check if there's already a SquadDefenseMission for this room.
                let has_squad_defense = room_data.get_missions().iter().any(|mission_entity| {
                    system_data
                        .mission_data
                        .get(*mission_entity)
                        .as_mission_type::<SquadDefenseMission>()
                        .is_some()
                });

                if has_squad_defense {
                    return None;
                }

                // Analyze threat.
                let mut estimated_dps: f32 = 0.0;
                let mut estimated_heal: f32 = 0.0;
                let mut any_boosted = false;

                for hostile in &hostiles {
                    for part_info in hostile.body().iter() {
                        if part_info.hits() == 0 {
                            continue;
                        }
                        if part_info.boost().is_some() {
                            any_boosted = true;
                        }
                        match part_info.part() {
                            Part::Attack => estimated_dps += 30.0,
                            Part::RangedAttack => estimated_dps += 10.0,
                            Part::Heal => estimated_heal += 12.0,
                            _ => {}
                        }
                    }
                }

                Some(DefenseNeed {
                    room_entity: entity,
                    estimated_dps,
                    estimated_heal,
                    hostile_count: hostiles.len(),
                    any_boosted,
                })
            })
            .collect();

        // ─── Phase 2: Create squad defense missions ────────────────────────

        for need in rooms_needing_defense {
            let room_data = match system_data.room_data.get_mut(need.room_entity) {
                Some(rd) => rd,
                None => continue,
            };

            // Escalation logic:
            // - Solo: low threat (single scout, low DPS)
            // - Duo: moderate threat (multiple hostiles, significant DPS, or healing)
            // - Quad: heavy threat (boosted quads, high DPS + heal, 4+ hostiles)
            let squad_size = if (need.any_boosted && need.estimated_dps > 200.0)
                || (need.estimated_heal > 100.0 && need.estimated_dps > 150.0)
                || need.hostile_count >= 4
            {
                "Quad"
            } else if need.estimated_dps > 60.0
                || need.estimated_heal > 20.0
                || need.hostile_count >= 2
                || need.any_boosted
            {
                "Duo"
            } else {
                "Solo"
            };

            info!(
                "Starting squad defense mission for room: {} (dps={:.0}, heal={:.0}, count={}, size={})",
                room_data.name, need.estimated_dps, need.estimated_heal, need.hostile_count, squad_size
            );

            let mission_entity = match squad_size {
                "Quad" => SquadDefenseMission::build_quad(
                    system_data.updater.create_entity(system_data.entities),
                    Some(runtime_data.entity),
                    need.room_entity,
                    &home_rooms,
                )
                .build(),
                "Duo" => SquadDefenseMission::build_duo(
                    system_data.updater.create_entity(system_data.entities),
                    Some(runtime_data.entity),
                    need.room_entity,
                    &home_rooms,
                )
                .build(),
                _ => SquadDefenseMission::build(
                    system_data.updater.create_entity(system_data.entities),
                    Some(runtime_data.entity),
                    need.room_entity,
                    &home_rooms,
                )
                .build(),
            };

            room_data.add_mission(mission_entity);
        }

        // ─── Phase 3: Create nuke defense, safe mode, and wall repair missions ──

        for state in room_states {
            let room_data = match system_data.room_data.get_mut(state.room_entity) {
                Some(rd) => rd,
                None => continue,
            };

            // NukeDefenseMission: create if nukes detected and no existing mission.
            if state.has_nukes && !state.has_nuke_defense_mission && features.military.nuke_defense {
                info!(
                    "Creating NukeDefenseMission for room: {}",
                    room_data.name
                );
                let mission_entity = NukeDefenseMission::build(
                    system_data.updater.create_entity(system_data.entities),
                    Some(runtime_data.entity),
                    state.room_entity,
                )
                .build();
                room_data.add_mission(mission_entity);
            }

            // SafeModeMission: create if hostiles present and no existing mission.
            if state.has_hostiles && !state.has_safe_mode_mission && features.military.safe_mode {
                info!(
                    "Creating SafeModeMission for room: {}",
                    room_data.name
                );
                let mission_entity = SafeModeMission::build(
                    system_data.updater.create_entity(system_data.entities),
                    Some(runtime_data.entity),
                    state.room_entity,
                )
                .build();
                room_data.add_mission(mission_entity);
            }

            // WallRepairMission: create if hostiles present and no existing mission.
            if state.has_hostiles && !state.has_wall_repair_mission {
                info!(
                    "Creating WallRepairMission for room: {}",
                    room_data.name
                );
                let mission_entity = WallRepairMission::build(
                    system_data.updater.create_entity(system_data.entities),
                    Some(runtime_data.entity),
                    state.room_entity,
                )
                .build();
                room_data.add_mission(mission_entity);
            }
        }

        Ok(OperationResult::Running)
    }
}
