use specs::*;

use super::claim::*;
use super::construction::*;
use super::data::*;
use super::localbuild::*;
use super::localsupply::*;
use super::remotemine::*;
use super::tower::*;
use super::upgrade::*;

pub struct OperationManagerSystem;

impl<'a> System<'a> for OperationManagerSystem {
    type SystemData = (
        Entities<'a>,
        ReadStorage<'a, OperationData>,
        Read<'a, LazyUpdate>,
    );

    fn run(&mut self, (entities, operations, updater): Self::SystemData) {
        scope_timing!("OperationManagerSystem");

        //TODO: Come up with a better way of doing this for always-running operations.
        let mut has_local_supply = false;
        let mut has_upgrade = false;
        let mut has_local_build = false;
        let mut has_tower = false;
        let mut has_remote_mine = false;
        let mut has_construction = false;
        let mut has_claim = false;

        for (_, operation) in (&entities, &operations).join() {
            match operation {
                OperationData::LocalSupply(_) => has_local_supply = true,
                OperationData::Upgrade(_) => has_upgrade = true,
                OperationData::LocalBuild(_) => has_local_build = true,
                OperationData::Tower(_) => has_tower = true,
                OperationData::RemoteMine(_) => has_remote_mine = true,
                OperationData::Construction(_) => has_construction = true,
                OperationData::Claim(_) => has_claim = true,
            }
        }

        if !has_local_supply {
            info!("Local supply operation does not exist, creating.");

            LocalSupplyOperation::build(updater.create_entity(&entities)).build();
        }

        if !has_upgrade {
            info!("Upgrade operation does not exist, creating.");

            UpgradeOperation::build(updater.create_entity(&entities)).build();
        }

        if !has_local_build {
            info!("Local build operation does not exist, creating.");

            LocalBuildOperation::build(updater.create_entity(&entities)).build();
        }

        if !has_tower {
            info!("Tower operation does not exist, creating.");

            TowerOperation::build(updater.create_entity(&entities)).build();
        }

        if !has_remote_mine {
            info!("Remote mine operation does not exist, creating.");

            RemoteMineOperation::build(updater.create_entity(&entities)).build();
        }

        if !has_construction {
            info!("Construction operation does not exist, creating.");

            ConstructionOperation::build(updater.create_entity(&entities)).build();
        }

        if !has_claim {
            info!("Claim operation does not exist, creating.");

            ClaimOperation::build(updater.create_entity(&entities)).build();
        }
    }
}
