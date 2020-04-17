use super::actions::*;
use super::jobsystem::*;
use super::utility::controllerbehavior::*;
use super::utility::harvestbehavior::*;
use super::utility::haulbehavior::*;
use super::utility::waitbehavior::*;
use crate::remoteobjectid::*;
use crate::transfer::transfersystem::*;
use crate::visualize::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;
use screeps_machine::*;

pub struct JobTickContext<'a, 'b, 'c> {
    system_data: &'a JobExecutionSystemData<'b>,
    runtime_data: &'a mut JobExecutionRuntimeData<'c>,
    action_flags: SimultaneousActionFlags
}

#[derive(Clone, ConvertSaveload)]
pub struct UpgradeJobContext {
    home_room: Entity,
    allow_harvest: bool,
}

machine!(
    #[derive(Clone, Serialize, Deserialize)]
    enum UpgradeState {
        Idle,
        Harvest { target: RemoteObjectId<Source>, stuck_count: u8 },
        Pickup { ticket: TransferWithdrawTicket },
        FinishedPickup,
        Upgrade { target: RemoteObjectId<StructureController> },
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

        Idle, Harvest, FinishedPickup, Upgrade, Wait => fn visualize(&self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}
        
        Idle, Harvest, FinishedPickup, Upgrade, Wait => fn gather_data(&self, _system_data: &JobExecutionSystemData, _runtime_data: &mut JobExecutionRuntimeData) {}
        
        _ => fn tick(&mut self, state_context: &mut UpgradeJobContext, tick_context: &mut JobTickContext) -> Option<UpgradeState>;
    }
);

impl Idle {
    pub fn tick(&mut self, state_context: &UpgradeJobContext, tick_context: &mut JobTickContext) -> Option<UpgradeState> {
        let home_room_data = tick_context.system_data.room_data.get(state_context.home_room)?;

        get_new_pickup_state_fill_resource(
            &tick_context.runtime_data.owner,
            &[home_room_data],
            TransferPriorityFlags::ALL,
            TransferTypeFlags::HAUL | TransferTypeFlags::USE,
            ResourceType::Energy,
            tick_context.runtime_data.transfer_queue,
            UpgradeState::pickup,
        )
        .or_else(|| if state_context.allow_harvest {
            get_new_harvest_state(&tick_context.runtime_data.owner, home_room_data, |id| UpgradeState::harvest(id, 0))
        } else {
            None
        })
        .or_else(|| get_new_upgrade_state(&tick_context.runtime_data.owner, home_room_data, UpgradeState::upgrade))
        .or_else(|| Some(UpgradeState::wait(5)))
    }
}

impl Harvest {
    pub fn tick(&mut self, _state_context: &mut UpgradeJobContext, tick_context: &mut JobTickContext) -> Option<UpgradeState> {
        run_harvest_state(tick_context.runtime_data.owner, &mut tick_context.action_flags, &self.target, false, &mut self.stuck_count, UpgradeState::idle)
    }
}

impl Pickup {
    fn gather_data(&self, _system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) { 
        runtime_data.transfer_queue.register_pickup(&self.ticket, TransferType::Haul);
    }
    
    pub fn tick(&mut self, _state_context: &UpgradeJobContext, tick_context: &mut JobTickContext) -> Option<UpgradeState> {
        run_pickup_state(tick_context.runtime_data.owner, &mut tick_context.action_flags, &mut self.ticket, tick_context.runtime_data.transfer_queue, UpgradeState::finished_pickup)
    }

    pub fn visualize(&self, _system_data: &JobExecutionSystemData, describe_data: &mut JobDescribeData) {
        let pos = describe_data.owner.pos();
        let to = self.ticket.target().pos();

        if pos.room_name() == to.room_name() {
            describe_data.visualizer.get_room(pos.room_name()).line(
                (pos.x() as f32, pos.y() as f32),
                (to.x() as f32, to.y() as f32),
                Some(LineStyle::default().color("blue")),
            );
        }
    }
}

impl FinishedPickup {
    pub fn tick(&self, state_context: &UpgradeJobContext, tick_context: &mut JobTickContext) -> Option<UpgradeState> {
        let home_room_data = tick_context.system_data.room_data.get(state_context.home_room)?;

        get_new_pickup_state_fill_resource(
            &tick_context.runtime_data.owner,
            &[home_room_data],
            TransferPriorityFlags::ALL,
            TransferTypeFlags::HAUL | TransferTypeFlags::USE,
            ResourceType::Energy,
            tick_context.runtime_data.transfer_queue,
            UpgradeState::pickup,
        )
        .or_else(|| Some(UpgradeState::idle()))
    }
}

impl Upgrade {
    pub fn tick(&self, _state_context: &UpgradeJobContext, tick_context: &mut JobTickContext) -> Option<UpgradeState> {
        run_upgrade_state(&tick_context.runtime_data.owner, &self.target, UpgradeState::idle)
    }
}

impl Wait {
    pub fn tick(&mut self, _state_context: &UpgradeJobContext, _tick_context: &mut JobTickContext) -> Option<UpgradeState> {
        run_wait_state(&mut self.ticks, UpgradeState::idle)
    }
}

#[derive(Clone, ConvertSaveload)]
pub struct UpgradeJob {
    context: UpgradeJobContext,
    state: UpgradeState,    
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl UpgradeJob {
    pub fn new(home_room: Entity, allow_harvest: bool) -> UpgradeJob {
        UpgradeJob {
            context: UpgradeJobContext { 
                home_room,
                allow_harvest,
            },
            state: UpgradeState::idle()
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Job for UpgradeJob {
    fn describe(&mut self, system_data: &JobExecutionSystemData, describe_data: &mut JobDescribeData) {
        self.state.describe(system_data, describe_data);
        self.state.visualize(system_data, describe_data);
    }

    fn pre_run_job(&mut self, system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        self.state.gather_data(system_data, runtime_data);
    }

    fn run_job(&mut self, system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        let mut tick_context = JobTickContext {
            system_data: system_data,
            runtime_data: runtime_data,
            action_flags: SimultaneousActionFlags::UNSET
        };

        while let Some(tick_result) = self.state.tick(&mut self.context, &mut tick_context) {
            self.state = tick_result
        }
    }
}
