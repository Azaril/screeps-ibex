use screeps::*;

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Ord, PartialOrd)]
pub enum BuildPriority {
    VeryLow,
    Low,
    Medium,
    High,
    Critical,
}

fn map_structure_priority(structure: StructureType) -> BuildPriority {
    match structure {
        StructureType::Spawn => BuildPriority::Critical,
        StructureType::Storage => BuildPriority::High,
        StructureType::Container => BuildPriority::High,
        StructureType::Tower => BuildPriority::High,
        StructureType::Wall => BuildPriority::Low,
        StructureType::Rampart => BuildPriority::Low,
        StructureType::Road => BuildPriority::VeryLow,
        _ => BuildPriority::Medium,
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn select_construction_site(creep: &Creep, room: &Room) -> Option<ConstructionSite> {
    let construction_sites = room.find(find::MY_CONSTRUCTION_SITES);

    let mut priority_construction_sites: Vec<_> = construction_sites
        .iter()
        .map(|s| (s, map_structure_priority(s.structure_type())))
        .collect();

    let creep_pos = creep.pos();

    priority_construction_sites.sort_by(|(a, a_priority), (b, b_priority)| {
        a_priority
            .cmp(b_priority)
            .then_with(|| a.progress().cmp(&b.progress()))
            .then_with(|| creep_pos.get_range_to(&a.pos()).cmp(&creep_pos.get_range_to(&b.pos())).reverse())
    });

    priority_construction_sites.pop().take().map(|(s, _)| s.clone())
}
