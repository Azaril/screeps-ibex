use screeps::*;
use screeps_foreman::planner::*;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn select_construction_site(creep: &Creep, construction_sites: &[ConstructionSite]) -> Option<ConstructionSite> {
    let creep_pos = creep.pos();

    construction_sites
        .iter()
        .filter(|s| s.my())
        .map(|s| (s, get_build_priority(s.structure_type())))
        .max_by(|(a, a_priority), (b, b_priority)| {
            a_priority
                .cmp(b_priority)
                .then_with(|| a.progress().cmp(&b.progress()))
                .then_with(|| creep_pos.get_range_to(&a.pos()).cmp(&creep_pos.get_range_to(&b.pos())).reverse())
        })
        .map(|(s, _)| s.clone())
}
