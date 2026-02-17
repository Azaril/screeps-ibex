use super::claim::*;
use super::colony::*;
use super::data::*;
use super::miningoutpost::*;
use super::scout::*;
use super::war::*;
use log::*;
use specs::*;

pub struct OperationManagerSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for OperationManagerSystem {
    type SystemData = (Entities<'a>, ReadStorage<'a, OperationData>, Read<'a, LazyUpdate>);

    fn run(&mut self, (entities, operations, updater): Self::SystemData) {
        let mut has_mining_outpost = false;
        let mut has_claim = false;
        let mut has_colony = false;
        let mut has_scout = false;
        let mut has_war = false;

        for (_, operation) in (&entities, &operations).join() {
            match operation {
                OperationData::MiningOutpost(_) => has_mining_outpost = true,
                OperationData::Claim(_) => has_claim = true,
                OperationData::Colony(_) => has_colony = true,
                OperationData::Attack(_) => {}
                OperationData::Scout(_) => has_scout = true,
                OperationData::War(_) => has_war = true,
            }
        }

        if !has_mining_outpost {
            info!("Mining outpost operation does not exist, creating.");

            MiningOutpostOperation::build(updater.create_entity(&entities), None).build();
        }

        if !has_claim {
            info!("Claim operation does not exist, creating.");

            ClaimOperation::build(updater.create_entity(&entities), None).build();
        }

        if !has_colony {
            info!("Colony operation does not exist, creating.");

            ColonyOperation::build(updater.create_entity(&entities), None).build();
        }

        if !has_war {
            info!("War operation does not exist, creating.");

            WarOperation::build(updater.create_entity(&entities), None).build();
        }

        if !has_scout {
            info!("Scout operation does not exist, creating.");

            ScoutOperation::build(updater.create_entity(&entities), None).build();
        }
    }
}
