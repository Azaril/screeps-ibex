use crate::findnearest::*;
use screeps::*;
#[cfg(feature = "time")]
use timing_annotate::*;

#[cfg_attr(feature = "time", timing)]
pub fn select_construction_site(creep: &Creep, room: &Room) -> Option<ConstructionSite> {
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
