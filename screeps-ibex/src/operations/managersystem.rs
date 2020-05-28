use super::claim::*;
use super::colony::*;
use super::data::*;
use super::miningoutpost::*;
use log::*;
use specs::*;

pub struct OperationManagerSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for OperationManagerSystem {
    type SystemData = (Entities<'a>, ReadStorage<'a, OperationData>, Read<'a, LazyUpdate>);

    fn run(&mut self, (entities, operations, updater): Self::SystemData) {
        //TODO: Come up with a better way of doing this for always-running operations.
        let mut has_mining_outpost = false;
        let mut has_claim = false;
        let mut has_colony = false;

        for (_, operation) in (&entities, &operations).join() {
            match operation {
                OperationData::MiningOutpost(_) => has_mining_outpost = true,
                OperationData::Claim(_) => has_claim = true,
                OperationData::Colony(_) => has_colony = true,
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
    }
}
