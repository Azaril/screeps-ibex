use screeps::*;
use crate::remoteobjectid::*;

pub struct BuildBehaviorUtility;

impl BuildBehaviorUtility {
    pub fn build_construction_site(creep: &Creep, construction_site: &ConstructionSite) {
        scope_timing!("build_construction_site");

        let creep_pos = creep.pos();
        let target_pos = construction_site.pos();

        if creep_pos.in_range_to(&target_pos, 3) && creep_pos.room_name() == target_pos.room_name()
        {
            creep.build(&construction_site);
        } else {
            creep.move_to(&target_pos);
        }
    }

    pub fn build_construction_site_id(creep: &Creep, construction_site_id: &RemoteObjectId<ConstructionSite>) {
        let target_position = construction_site_id.pos();

        if creep.pos().room_name() != target_position.room_name() {
            creep.move_to(&target_position);
        } else if let Some(construction_site) = construction_site_id.resolve() {
            //TODO: Handle error code.
            Self::build_construction_site(creep, &construction_site)
        } else {
            //TODO: Return error result.
            error!("Failed to resolve controller for upgrading. Name: {}", creep.name());
        }
    }
}