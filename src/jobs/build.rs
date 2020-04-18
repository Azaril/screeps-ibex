use super::actions::*;
use super::jobsystem::*;
use super::context::*;
use super::utility::repair::*;
use super::utility::buildbehavior::*;
use super::utility::harvestbehavior::*;
use super::utility::haulbehavior::*;
use super::utility::repairbehavior::*;
use super::utility::waitbehavior::*;
use crate::remoteobjectid::*;
use crate::structureidentifier::*;
use crate::transfer::transfersystem::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;
use screeps_machine::*;

#[derive(Clone, ConvertSaveload)]
pub struct BuildJobContext {
    home_room: Entity,
    build_room: Entity,
    allow_harvest: bool,
}

machine!(
    #[derive(Clone, Serialize, Deserialize)]
    enum BuildState {
        Idle,
        Pickup { ticket: TransferWithdrawTicket },
        FinishedPickup,
        Harvest { target: RemoteObjectId<Source> },
        Build { target: RemoteObjectId<ConstructionSite> },
        Repair { target: RemoteStructureIdentifier },
        Wait { ticks: u32 }
    }

    impl {
        * => fn describe(&self, _system_data: &JobExecutionSystemData, describe_data: &mut JobDescribeData) {
            let room = { describe_data.owner.room() };

            if let Some(room) = room {
                let name = describe_data.owner.name();
                let room_name = room.name();

                describe_data
                    .ui
                    .with_room(room_name, &mut describe_data.visualizer, |room_ui| {
                        let description = self.status_description();

                        room_ui.jobs().add_text(format!("{} - {}", name, description), None);
                    });
            }
        }

        * => fn status_description(&self) -> String {
            std::any::type_name::<Self>().to_string()
        }

        Idle, FinishedPickup, Harvest, Build, Repair, Wait => fn visualize(&self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}
        
        Idle, FinishedPickup, Harvest, Build, Repair, Wait => fn gather_data(&self, _system_data: &JobExecutionSystemData, _runtime_data: &mut JobExecutionRuntimeData) {}
        
        _ => fn tick(&mut self, state_context: &mut BuildJobContext, tick_context: &mut JobTickContext) -> Option<BuildState>;
    }
);

impl Idle {
    pub fn tick(&mut self, state_context: &BuildJobContext, tick_context: &mut JobTickContext) -> Option<BuildState> {
        let creep = tick_context.runtime_data.owner;
        let build_room_data = tick_context.system_data.room_data.get(state_context.build_room)?;

        get_new_repair_state(creep, build_room_data, Some(RepairPriority::High), BuildState::repair)
            .or_else(|| get_new_build_state(creep, build_room_data, BuildState::build))
            .or_else(|| get_new_repair_state(creep, build_room_data, None, BuildState::repair))
            .or_else(|| {
                get_new_pickup_state_fill_resource(
                    creep,
                    &[build_room_data],
                    TransferPriorityFlags::ALL,
                    TransferTypeFlags::HAUL | TransferTypeFlags::USE,
                    ResourceType::Energy,
                    tick_context.runtime_data.transfer_queue,
                    BuildState::pickup,
                )
            })
            .or_else(|| if state_context.allow_harvest {
                get_new_harvest_state(creep, build_room_data, BuildState::harvest)
            } else {
                None
            })
            .or_else(|| Some(BuildState::wait(5)))
    }
}

impl Pickup {
    fn gather_data(&self, _system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) { 
        runtime_data.transfer_queue.register_pickup(&self.ticket, TransferType::Haul);
    }
    
    pub fn tick(&mut self, _state_context: &BuildJobContext, tick_context: &mut JobTickContext) -> Option<BuildState> {
        tick_pickup(tick_context, &mut self.ticket, BuildState::finished_pickup)
    }

    pub fn visualize(&self, _system_data: &JobExecutionSystemData, describe_data: &mut JobDescribeData) {
        visualize_pickup(describe_data, &self.ticket);
    }
}

impl FinishedPickup {
    pub fn tick(&self, state_context: &BuildJobContext, tick_context: &mut JobTickContext) -> Option<BuildState> {
        let build_room_data = tick_context.system_data.room_data.get(state_context.build_room)?;

        get_new_pickup_state_fill_resource(
            &tick_context.runtime_data.owner,
            &[build_room_data],
            TransferPriorityFlags::ALL,
            TransferTypeFlags::HAUL | TransferTypeFlags::USE,
            ResourceType::Energy,
            tick_context.runtime_data.transfer_queue,
            BuildState::pickup,
        )
        .or_else(|| Some(BuildState::idle()))
    }
}

impl Harvest {
    pub fn tick(&mut self, _state_context: &mut BuildJobContext, tick_context: &mut JobTickContext) -> Option<BuildState> {
        tick_harvest(tick_context, self.target, false, false, BuildState::idle)
    }
}

impl Build {
    pub fn tick(&mut self, _state_context: &mut BuildJobContext, tick_context: &mut JobTickContext) -> Option<BuildState> {
        tick_build(tick_context, self.target, BuildState::idle)
    }
}

impl Repair {
    pub fn tick(&mut self, _state_context: &mut BuildJobContext, tick_context: &mut JobTickContext) -> Option<BuildState> {
        tick_repair(tick_context, self.target, BuildState::idle)
    }
}

impl Wait {
    pub fn tick(&mut self, _state_context: &BuildJobContext, _tick_context: &mut JobTickContext) -> Option<BuildState> {
       tick_wait(&mut self.ticks, BuildState::idle)
    }
}

#[derive(Clone, ConvertSaveload)]
pub struct BuildJob {
    context: BuildJobContext,
    state: BuildState,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl BuildJob {
    pub fn new(home_room: Entity, build_room: Entity, allow_harvest: bool) -> BuildJob {
        BuildJob {
            context: BuildJobContext {
                home_room,
                build_room,
                allow_harvest
            },
            state: BuildState::idle(),
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Job for BuildJob {
    fn describe(&mut self, system_data: &JobExecutionSystemData, describe_data: &mut JobDescribeData) {
        self.state.describe(system_data, describe_data);
        self.state.visualize(system_data, describe_data);
    }

    fn pre_run_job(&mut self, system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        self.state.gather_data(system_data, runtime_data);
    }

    fn run_job(&mut self, system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        let mut tick_context = JobTickContext {
            system_data,
            runtime_data,
            action_flags: SimultaneousActionFlags::UNSET
        };

        while let Some(tick_result) = self.state.tick(&mut self.context, &mut tick_context) {
            self.state = tick_result
        }
    }
}
