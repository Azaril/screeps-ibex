use super::data::*;
use super::missionsystem::*;
use crate::energy_stress::*;
use crate::jobs::build::*;
use crate::jobs::data::*;
use crate::jobs::utility::repair::*;
use crate::repairqueue::*;
use crate::room::data::*;
use crate::serialize::*;
use crate::spawnsystem::*;
use crate::structureidentifier::*;
use screeps::*;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;

#[derive(ConvertSaveload)]
pub struct LocalBuildMission {
    owner: EntityOption<Entity>,
    room_data: Entity,
    builders: EntityVec<Entity>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl LocalBuildMission {
    pub fn build<B>(builder: B, owner: Option<Entity>, room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = LocalBuildMission::new(owner, room_data);

        builder
            .with(MissionData::LocalBuild(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>, room_data: Entity) -> LocalBuildMission {
        LocalBuildMission {
            owner: owner.into(),
            room_data,
            builders: EntityVec::new(),
        }
    }

    // K4 policy (ADR 0040 M3): the builder count table, priority bands, repairer arm and body
    // cap live in `screeps_econ_decision::spawn_policy` (consumed here and by the economy
    // sim); this mission keeps the ECS/roster bookkeeping and the room reads.
    fn get_builder_priority(&self, room_data: &RoomData, has_sufficient_energy: bool) -> Option<(u32, f32)> {
        let structures = room_data.get_structures()?;
        let controller_level = structures.controllers().iter().map(|c| c.level()).max().unwrap_or(0);
        let construction_sites = room_data.get_construction_sites()?;

        if !construction_sites.is_empty() {
            let required_progress: u32 = construction_sites
                .iter()
                .map(|construction_site| construction_site.progress_total() - construction_site.progress())
                .sum();

            let desired_builders_for_progress: u32 =
                screeps_econ_decision::spawn_policy::builder_desired_for_progress(controller_level, required_progress);

            let desired_builders = if has_sufficient_energy { desired_builders_for_progress } else { 1 };

            if desired_builders > 0 {
                let priority = if self.builders.is_empty() {
                    screeps_econ_decision::spawn_policy::FIRST_BUILDER_PRIORITY
                } else {
                    let any_spawn_or_storage_site = construction_sites
                        .iter()
                        .any(|site| matches!(site.structure_type(), StructureType::Spawn | StructureType::Storage));
                    screeps_econ_decision::spawn_policy::builder_priority_with_builders(any_spawn_or_storage_site)
                };

                Some((desired_builders, priority))
            } else {
                None
            }
        } else {
            None
        }
    }

    fn get_repairer_priority(&self, room_data: &RoomData, repair_queue: &RepairQueue, allowance: RepairAllowance) -> Option<(u32, f32)> {
        // S1 repair stress gate (ADR 0040 §D6): under CriticalOnly, only a
        // Critical repair target justifies a repairer spawn.
        let minimum_priority = effective_min_repair_priority(None, allowance);

        let (priority, _) = select_repair_structure_and_priority(room_data, repair_queue, minimum_priority, true)?;

        screeps_econ_decision::spawn_policy::repairer_spawn_priority(priority)
    }

    fn create_handle_builder_spawn(
        mission_entity: Entity,
        room_entity: Entity,
        allow_harvest: bool,
    ) -> crate::spawnsystem::SpawnQueueCallback {
        Box::new(move |spawn_system_data, name| {
            let name = name.to_string();

            spawn_system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::Build(BuildJob::new(room_entity, room_entity, allow_harvest));

                let creep_entity = crate::creep::spawning::build(world.create_entity(), &name).with(creep_job).build();

                if let Some(mut mission_data) = world
                    .write_storage::<MissionData>()
                    .get_mut(mission_entity)
                    .as_mission_type_mut::<LocalBuildMission>()
                {
                    mission_data.builders.push(creep_entity);
                }
            });
        })
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for LocalBuildMission {
    fn get_owner(&self) -> &Option<Entity> {
        &self.owner
    }

    fn owner_complete(&mut self, owner: Entity) {
        assert!(Some(owner) == *self.owner);

        self.owner.take();
    }

    fn get_room(&self) -> Option<Entity> {
        Some(self.room_data)
    }

    fn remove_creep(&mut self, entity: Entity) {
        self.builders.retain(|e| *e != entity);
    }

    fn get_creeps(&self) -> Vec<Entity> {
        self.builders.iter().copied().collect()
    }

    fn describe_state(&self, _system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> String {
        format!("Local Build - Builders: {}", self.builders.len())
    }

    fn summarize(&self) -> crate::visualization::SummaryContent {
        crate::visualization::SummaryContent::Text(format!("Local Build - Builders: {}", self.builders.len()))
    }

    fn pre_run_mission(&mut self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<(), String> {
        //
        // Populate the repair queue with non-wall structures that need repair.
        // This makes the repair queue the single source of truth for repair
        // targets, so jobs don't need to do their own room scans.
        //

        if let Some(room_data) = system_data.room_data.get(self.room_data) {
            if let Some(structures) = room_data.get_structures() {
                let are_hostile_creeps = room_data.get_creeps().map(|c| !c.hostile().is_empty()).unwrap_or(false);

                let available_energy = structures
                    .storages()
                    .iter()
                    .map(|s| s.store().get_used_capacity(Some(ResourceType::Energy)))
                    .sum::<u32>();

                // Enqueue non-wall/rampart structures (roads, containers, spawns, etc.)
                // Walls and ramparts are handled by the WallRepairMission.
                for (structure, hits, hits_max) in get_repair_targets(structures.all(), false) {
                    if let Some(priority) =
                        map_structure_repair_priority(structure, hits, hits_max, Some(available_energy), are_hostile_creeps)
                    {
                        system_data.repair_queue.request_repair(RepairRequest {
                            structure_id: RemoteStructureIdentifier::new(structure),
                            priority,
                            current_hits: hits,
                            max_hits: hits_max,
                            room: room_data.name,
                        });
                    }
                }
            }
        }

        Ok(())
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<MissionResult, String> {
        let room_data_storage = &*system_data.room_data;
        let room_data = room_data_storage.get(self.room_data).ok_or("Expected room data")?;
        let room = game::rooms().get(room_data.name).ok_or("Expected room")?;
        let structure_data = room_data.get_structures().ok_or("Expected structure data")?;

        // K4 policy (ADR 0040 M3): the sufficient-energy threshold lives in
        // `screeps_econ_decision::spawn_policy`.
        let has_sufficient_energy = {
            let storage_energies: Vec<u32> = structure_data
                .storages()
                .iter()
                .map(|storage| storage.store().get(ResourceType::Energy).unwrap_or(0))
                .collect();
            let container_energies: Vec<u32> = structure_data
                .containers()
                .iter()
                .map(|container| container.store().get(ResourceType::Energy).unwrap_or(0))
                .collect();
            screeps_econ_decision::spawn_policy::has_sufficient_energy(
                !structure_data.storages().is_empty(),
                &storage_energies,
                &container_energies,
            )
        };

        let mut spawn_count = 0;
        let mut spawn_priority = SPAWN_PRIORITY_NONE;

        if let Some((desired_builders, build_priority)) = self.get_builder_priority(room_data, has_sufficient_energy) {
            spawn_count = spawn_count.max(desired_builders);
            spawn_priority = spawn_priority.max(build_priority);
        }

        let allowance = repair_allowance_for(system_data.economy, &system_data.features, Some(self.room_data));

        if let Some((desired_repairers, repair_priority)) = self.get_repairer_priority(room_data, system_data.repair_queue, allowance) {
            spawn_count = spawn_count.max(desired_repairers);
            spawn_priority = spawn_priority.max(repair_priority);
        }

        if self.builders.len() < spawn_count as usize {
            let use_energy_max = if self.builders.is_empty() && spawn_priority >= SPAWN_PRIORITY_HIGH {
                room.energy_available().max(SPAWN_ENERGY_CAPACITY)
            } else {
                room.energy_capacity_available()
            };

            let body_definition = screeps_econ_decision::spawn_policy::builder_body(use_energy_max, spawn_priority);

            if let Ok(body) = crate::creep::spawning::create_body(&body_definition) {
                let allow_harvest = room.storage().is_none();

                let spawn_request = SpawnRequest::new(
                    "Local Builder".to_string(),
                    &body,
                    spawn_priority,
                    None,
                    Self::create_handle_builder_spawn(mission_entity, self.room_data, allow_harvest),
                );

                system_data.spawn_queue.request(self.room_data, spawn_request);
            }
        }

        Ok(MissionResult::Running)
    }
}
