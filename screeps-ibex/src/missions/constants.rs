use screeps::*;

pub fn get_desired_storage_amount(resource: ResourceType) -> u32 {
    match resource {
        ResourceType::Energy => 200_000,
        _ => 10_000,
    }
}

pub enum CpuBar {
    IdlePriority = 7,
    LowPriority = 5,
    MediumPriority = 4,
    HighPriority = 3,
    CriticalPriority = 2
}

pub fn can_execute_cpu(bar: CpuBar) -> bool {
    game::cpu::bucket() >= (game::cpu::tick_limit() * bar as u32)
}