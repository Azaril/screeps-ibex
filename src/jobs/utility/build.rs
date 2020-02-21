use crate::findnearest::*;
use crate::remoteobjectid::*;
use screeps::*;

pub struct BuildUtility;

impl BuildUtility {
    pub fn select_construction_site(creep: &Creep, room: &Room) -> Option<ConstructionSite> {
        scope_timing!("select_construction_site");

        let construction_sites = room.find(find::MY_CONSTRUCTION_SITES);

        let in_progress_construction_site_id = construction_sites
            .iter()
            .cloned()
            .filter(|site| site.progress() > 0)
            .max_by_key(|site| site.progress());

        in_progress_construction_site_id.or_else(|| {
            construction_sites
                .iter()
                .cloned()
                .find_nearest_from(creep.pos(), PathFinderHelpers::same_room_ignore_creeps_range_3)
        })
    }
}

pub trait ValidateBuildTarget {
    fn is_valid_build_target(&self) -> bool;
}

impl ValidateBuildTarget for ObjectId<ConstructionSite> {
    fn is_valid_build_target(&self) -> bool {
        self.resolve().is_some()
    }
}

impl ValidateBuildTarget for RemoteObjectId<ConstructionSite> {
    fn is_valid_build_target(&self) -> bool {
        if game::rooms::get(self.pos().room_name()).is_some() {
            self.resolve().is_some()
        } else {
            true
        }
    }
}
