use screeps::*;

pub fn get_desired_storage_amount(resource: ResourceType) -> u32 {
    match resource {
        ResourceType::Energy => 200_000,
        _ => 10_000,
    }
}