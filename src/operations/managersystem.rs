use specs::*;

use super::data::*;
use super::bootstrap::*;

pub struct OperationManagerSystem;

impl<'a> System<'a> for OperationManagerSystem {
    type SystemData = (
        Entities<'a>,
        ReadStorage<'a, OperationData>,
        Read<'a, LazyUpdate>
    );

    fn run(&mut self, (entities, operations, updater): Self::SystemData) {
        scope_timing!("OperationManagerSystem");

        let mut operation_entities = (&entities, &operations).join();

        if !operation_entities.any(|_| true) {
            info!("No operations exist - creating bootstrap operation.");

            let _entity = BootstrapOperation::build(updater.create_entity(&entities)).build();
        }
    }
}