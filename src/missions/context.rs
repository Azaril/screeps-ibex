use super::missionsystem::*;

pub struct MissionTickContext<'a, 'b, 'c, 'd> {
    pub system_data: &'a mut MissionExecutionSystemData<'b, 'd>,
    pub runtime_data: &'a mut MissionExecutionRuntimeData<'c>,
}
