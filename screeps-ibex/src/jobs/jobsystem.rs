use super::data::JobData;
use crate::creep::CreepOwner;
use crate::entitymappingsystem::*;
use crate::room::data::*;
use crate::transfer::transfersystem::*;
use crate::visualization::SummaryContent;
use screeps::*;
use screeps_rover::*;
use specs::prelude::*;

#[derive(specs::SystemData)]
pub struct JobSystemData<'a> {
    creep_owners: ReadStorage<'a, CreepOwner>,
    jobs: WriteStorage<'a, JobData>,
    updater: Read<'a, LazyUpdate>,
    entities: Entities<'a>,
    transfer_queue: Write<'a, TransferQueue>,
    room_data: ReadStorage<'a, RoomData>,
    movement: WriteExpect<'a, MovementData<Entity>>,
    mapping: Read<'a, EntityMappingData>,
}

pub struct JobExecutionSystemData<'a> {
    pub updater: &'a Read<'a, LazyUpdate>,
    pub entities: &'a Entities<'a>,
    pub room_data: &'a ReadStorage<'a, RoomData>,
}

pub struct JobExecutionRuntimeData<'a> {
    pub creep_entity: Entity,
    pub owner: &'a Creep,
    pub mapping: &'a EntityMappingData,
    pub transfer_queue: &'a mut TransferQueue,
    pub movement: &'a mut MovementData<Entity>,
}

pub struct JobDescribeData<'a> {
    pub _owner: &'a Creep,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub trait Job {
    fn describe(&mut self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}

    /// Produce a structured summary for the visualization overlay.
    /// Reads only `self`; no system data required.
    fn summarize(&self) -> SummaryContent {
        SummaryContent::Text("Job".to_string())
    }

    fn pre_run_job(&mut self, _system_data: &JobExecutionSystemData, _runtime_data: &mut JobExecutionRuntimeData) {}

    fn run_job(&mut self, system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData);
}

pub struct PreRunJobSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for PreRunJobSystem {
    type SystemData = JobSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        let system_data = JobExecutionSystemData {
            updater: &data.updater,
            entities: &data.entities,
            room_data: &data.room_data,
        };

        for (creep_entity, creep, job_data) in (&data.entities, &data.creep_owners, &mut data.jobs).join() {
            if let Some(owner) = creep.owner.resolve() {
                let mut runtime_data = JobExecutionRuntimeData {
                    creep_entity,
                    owner: &owner,
                    mapping: &data.mapping,
                    transfer_queue: &mut data.transfer_queue,
                    movement: &mut data.movement,
                };

                job_data.as_job().pre_run_job(&system_data, &mut runtime_data);
            }
        }
    }
}

pub struct RunJobSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for RunJobSystem {
    type SystemData = JobSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        let system_data = JobExecutionSystemData {
            updater: &data.updater,
            entities: &data.entities,
            room_data: &data.room_data,
        };

        for (creep_entity, creep, job_data) in (&data.entities, &data.creep_owners, &mut data.jobs).join() {
            if let Some(owner) = creep.owner.resolve() {
                let mut runtime_data = JobExecutionRuntimeData {
                    creep_entity,
                    owner: &owner,
                    mapping: &data.mapping,
                    transfer_queue: &mut data.transfer_queue,
                    movement: &mut data.movement,
                };

                job_data.as_job().run_job(&system_data, &mut runtime_data);
            }
        }
    }
}
