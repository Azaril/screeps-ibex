use screeps::*;

pub fn get_desired_storage_amount(resource: ResourceType) -> u32 {
    match resource {
        ResourceType::Energy => 200_000,
        _ => 10_000,
    }
}

/// Bucket-multiplier bars consumed by
/// [`crate::cpugovernor::GovernorSnapshot::can_execute_cpu`]: work runs
/// only while `bucket >= tick_limit * bar`. Read the snapshot from your
/// execution system data (`system_data.governor`) — there is no global
/// reader (statics-review M1).
#[allow(clippy::enum_variant_names)]
#[derive(Clone, Copy)]
pub enum CpuBar {
    IdlePriority = 7,
    LowPriority = 5,
    MediumPriority = 4,
    HighPriority = 3,
    CriticalPriority = 2,
}
