use super::missionsystem::*;

pub struct MissionTickContext<'a, 'b, 'd> {
    pub system_data: &'a mut MissionExecutionSystemData<'b, 'd>,
    pub runtime_data: &'a mut MissionExecutionRuntimeData,
}
