use screeps::*;

pub fn get_desired_storage_amount(resource: ResourceType) -> u32 {
    match resource {
        ResourceType::Energy => 200_000,
        _ => 10_000,
    }
}

/// Bucket-multiplier bars consumed by [`can_execute_cpu`]: work runs
/// only while `bucket >= tick_limit * bar`.
#[allow(clippy::enum_variant_names)]
#[derive(Clone, Copy)]
pub enum CpuBar {
    IdlePriority = 7,
    LowPriority = 5,
    MediumPriority = 4,
    HighPriority = 3,
    CriticalPriority = 2,
}

/// Reads the CpuGovernor's tick-start snapshot (P1.B3) instead of the
/// live game API — same formula, one tick-consistent source. New code
/// wanting tiered behavior should read `cpugovernor::tier()` directly.
pub fn can_execute_cpu(bar: CpuBar) -> bool {
    crate::cpugovernor::can_execute_cpu(bar)
}
