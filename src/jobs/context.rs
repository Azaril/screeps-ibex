use super::actions::*;
use super::jobsystem::*;

pub struct JobTickContext<'a, 'b, 'c> {
    pub system_data: &'a JobExecutionSystemData<'b>,
    pub runtime_data: &'a mut JobExecutionRuntimeData<'c>,
    pub action_flags: SimultaneousActionFlags,
}
