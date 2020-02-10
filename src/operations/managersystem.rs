use specs::*;

use super::data::*;
use super::localsupply::*;

pub struct OperationManagerSystem;

impl<'a> System<'a> for OperationManagerSystem {
    type SystemData = (
        Entities<'a>,
        ReadStorage<'a, OperationData>,
        Read<'a, LazyUpdate>
    );

    fn run(&mut self, (entities, operations, updater): Self::SystemData) {
        scope_timing!("OperationManagerSystem");

        let mut has_local_supply = false;

        for (_, operation) in (&entities, &operations).join() {
            match operation {
                OperationData::LocalSupply(_) => {
                    has_local_supply = true;
                }
            }
        }

        if !has_local_supply {
            info!("Local supply operation does not exist, creating.");

            LocalSupplyOperation::build(updater.create_entity(&entities)).build();
        }
    }
}