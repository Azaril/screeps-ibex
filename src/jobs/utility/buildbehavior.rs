use screeps::*;

pub struct BuildBehaviorUtility;

impl BuildBehaviorUtility {
    pub fn build_construction_site(creep: &Creep, construction_site: &ConstructionSite) {
        scope_timing!("build_construction_site");

        let creep_pos = creep.pos();
        let target_pos = construction_site.pos();

        if creep_pos.in_range_to(&target_pos, 3) && creep_pos.room_name() == target_pos.room_name() {
            creep.build(&construction_site);
        } else {
            creep.move_to(&target_pos);
        }
    }
}