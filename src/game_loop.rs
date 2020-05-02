use crate::creep::*;
use crate::entitymappingsystem::*;
use crate::globals::*;
use crate::jobs::data::*;
use crate::jobs::jobsystem::*;
use crate::memorysystem::*;
use crate::missions::data::*;
use crate::missions::missionsystem::*;
use crate::operations::data::*;
use crate::operations::managersystem::*;
use crate::operations::operationsystem::*;
use crate::room::createroomsystem::*;
use crate::room::data::*;
use crate::room::roomplansystem::*;
use crate::room::updateroomsystem::*;
use crate::room::visibilitysystem::*;
use crate::serialize::*;
use crate::spawnsystem::*;
use crate::statssystem::*;
use crate::transfer::ordersystem::*;
use crate::transfer::transfersystem::*;
use crate::ui::*;
use crate::visualize::*;
use log::*;
use screeps::*;
use specs::{
    prelude::*,
    saveload::{DeserializeComponents, SerializeComponents},
};
use std::collections::HashSet;
use screeps_rover::*;
use crate::pathing::movementsystem::*;
use crate::pathing::costmatrixsystem::*;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
fn serialize_world(world: &World, segment: u32) {
    struct Serialize {
        segment: u32,
    }

    #[derive(SystemData)]
    struct SerializeSystemData<'a> {
        memory_arbiter: WriteExpect<'a, MemoryArbiter>,
        entities: Entities<'a>,
        marker_allocator: Write<'a, SerializeMarkerAllocator>,
        markers: ReadStorage<'a, SerializeMarker>,
        creep_spawnings: ReadStorage<'a, CreepSpawning>,
        creep_owners: ReadStorage<'a, CreepOwner>,
        room_data: ReadStorage<'a, RoomData>,
        room_plan_data: ReadStorage<'a, RoomPlanData>,
        job_data: ReadStorage<'a, JobData>,
        operation_data: ReadStorage<'a, OperationData>,
        mission_data: ReadStorage<'a, MissionData>,
    }

    impl<'a> System<'a> for Serialize {
        type SystemData = SerializeSystemData<'a>;

        fn run(&mut self, mut data: Self::SystemData) {
            struct BinCodeSerializerAcceptor<'a, 'b> {
                data: &'b mut SerializeSystemData<'a>
            }

            impl<'a, 'b> bincode::SerializerAcceptor for BinCodeSerializerAcceptor<'a, 'b> {
                type Output = Result<(), String>;

                fn accept<T: serde::Serializer>(self, ser: T) -> Self::Output {
                    SerializeComponents::<std::convert::Infallible, SerializeMarker>::serialize(
                        &(
                            &self.data.creep_spawnings,
                            &self.data.creep_owners,
                            &self.data.room_data,
                            &self.data.room_plan_data,
                            &self.data.job_data,
                            &self.data.operation_data,
                            &self.data.mission_data,
                        ),
                        &self.data.entities,
                        &self.data.markers,
                        ser,
                    )
                    .map(|_| ())
                    .map_err(|e| e.to_string())
                }
            }

            let mut serialized_data = Vec::<u8>::with_capacity(1024 * 100);

            bincode::with_serializer(&mut serialized_data, BinCodeSerializerAcceptor { data: &mut data })
                .unwrap_or_else(|e| error!("Failed serialization: {}", e));

            let encoded_data = base64::encode(&serialized_data);

            data.memory_arbiter.set(self.segment, &encoded_data);
        }
    }

    let mut sys = Serialize { segment };

    sys.run_now(&world);
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
fn deserialize_world(world: &World, segment: u32) {
    struct Deserialize {
        segment: u32,
    }

    #[derive(SystemData)]
    struct DeserializeSystemData<'a> {
        memory_arbiter: WriteExpect<'a, MemoryArbiter>,
        entities: Entities<'a>,
        marker_alloc: Write<'a, SerializeMarkerAllocator>,
        markers: WriteStorage<'a, SerializeMarker>,
        creep_spawnings: WriteStorage<'a, CreepSpawning>,
        creep_owners: WriteStorage<'a, CreepOwner>,
        room_data: WriteStorage<'a, RoomData>,
        room_plan_data: WriteStorage<'a, RoomPlanData>,
        job_data: WriteStorage<'a, JobData>,
        operation_data: WriteStorage<'a, OperationData>,
        mission_data: WriteStorage<'a, MissionData>,
    }

    impl<'a> System<'a> for Deserialize {
        type SystemData = DeserializeSystemData<'a>;

        fn run(&mut self, mut data: Self::SystemData) {
            //
            // NOTE: System assumes that segment is available and will panic if data is not accesible.
            //

            let encoded_data = data.memory_arbiter.get(self.segment);

            if let Some(encoded_data) = encoded_data {
                if !encoded_data.is_empty() {
                    let decoded_data = base64::decode(&encoded_data)
                        .unwrap_or_else(|_| Vec::new());

                    struct BinCodeDeserializerAcceptor<'a, 'b> {
                        data: &'b mut DeserializeSystemData<'a>
                    }

                    impl<'a, 'b, 'c> bincode::DeserializerAcceptor<'c> for BinCodeDeserializerAcceptor<'a, 'b> {
                        type Output = Result<(), String>;
        
                        fn accept<T: serde::Deserializer<'c>>(self, de: T) -> Self::Output {
                            DeserializeComponents::<std::convert::Infallible, SerializeMarker>::deserialize(
                                &mut (
                                    &mut self.data.creep_spawnings,
                                    &mut self.data.creep_owners,
                                    &mut self.data.room_data,
                                    &mut self.data.room_plan_data,
                                    &mut self.data.job_data,
                                    &mut self.data.operation_data,
                                    &mut self.data.mission_data,
                                ),
                                &self.data.entities,
                                &mut self.data.markers,
                                &mut self.data.marker_alloc,
                                de,
                            )
                            .map(|_| ())
                            .map_err(|e| e.to_string())
                        }
                    }

                    let reader = bincode::SliceReader::new(&decoded_data);
        
                    bincode::with_deserializer(reader, BinCodeDeserializerAcceptor { data: &mut data })
                        .unwrap_or_else(|e| error!("Failed deserialization: {}", e));
                }
            } else {
                panic!("Failed to get world data from segment that was expected to be active.");
            }
        }
    }

    let mut sys = Deserialize { segment };

    sys.run_now(&world);
}

struct CostMatrixStorageInterface;

impl CostMatrixStorage for CostMatrixStorageInterface {
    fn get_cache(&self, segment: u32) -> Result<CostMatrixCache, String> {
        let raw_data = raw_memory::get_segment(segment).ok_or("Memory segment not active")?;

        let res = crate::serialize::decode_from_string(&raw_data)?;

        Ok(res)
    }

    fn set_cache(&mut self, segment:u32, data: &CostMatrixCache) -> Result<(), String> {
        let encoded = crate::serialize::encode_to_string(data)?;

        raw_memory::set_segment(segment, &encoded);

        Ok(())
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn tick() {
    crate::features::js::prepare();

    let mut world = World::new();

    let mut memory_arbiter = MemoryArbiter::new();

    const COMPONENT_SEGMENT: u32 = 50;
    const COST_MATRIX_SYSTEM_SEGMENT: u32 = 55;

    memory_arbiter.request(COMPONENT_SEGMENT);
    memory_arbiter.request(COST_MATRIX_SYSTEM_SEGMENT);

    if !memory_arbiter.is_active(COMPONENT_SEGMENT) {
        world.insert(memory_arbiter);

        let mut memory_system = MemoryArbiterSystem {};

        memory_system.run_now(&world);

        return;
    } else {
        world.insert(memory_arbiter);
    }

    world.insert(SerializeMarkerAllocator::new());
    world.register::<SerializeMarker>();

    if crate::features::visualize::on() {
        world.insert(Visualizer::new());
        world.insert(UISystem::new());
    }

    let cost_matrix_storage = Box::new(CostMatrixStorageInterface);
    let cost_matrix_system = CostMatrixSystem::new(cost_matrix_storage, COST_MATRIX_SYSTEM_SEGMENT);

    world.insert(cost_matrix_system);

    let movement_data = MovementData::<Entity>::new();

    world.insert(movement_data);

    //
    // Pre-pass update
    //

    let mut pre_pass_dispatcher = DispatcherBuilder::new()
        .with(WaitForSpawnSystem, "wait_for_spawn", &[])
        .with(CleanupCreepsSystem, "cleanup_creeps", &[])
        .with(CreateRoomDataSystem, "create_room_data", &[])
        .with(UpdateRoomDataSystem, "update_room_data", &["create_room_data"])
        .with_barrier()
        .with(EntityMappingSystem, "entity_mapping", &[])
        .build();

    pre_pass_dispatcher.setup(&mut world);

    //
    // Main update
    //

    let mut main_dispatcher = DispatcherBuilder::new()
        .with(OperationManagerSystem, "operations_manager", &[])
        .with(PreRunOperationSystem, "pre_run_operations", &[])
        .with(PreRunMissionSystem, "pre_run_missions", &[])
        .with(PreRunJobSystem, "pre_run_jobs", &[])
        .with_barrier()
        .with(RunOperationSystem, "run_operations", &[])
        .with(RunMissionSystem, "run_missions", &[])
        .with(RunJobSystem, "run_jobs", &[])
        .with(MovementUpdateSystem, "movement", &["run_jobs"])
        .with_barrier()
        .with(VisibilityQueueSystem, "visibility_queue", &[])
        .with(SpawnQueueSystem, "spawn_queue", &[])
        .with(TransferQueueUpdateSystem, "transfer_queue", &[])
        .with(OrderQueueSystem, "order_queue", &[])
        .with_barrier()
        .with(RoomPlanSystem, "room_plan", &[])
        .with_barrier()
        .with(VisualizerSystem, "visualizer", &[])
        .with(StatsSystem, "stats", &[])
        .with_barrier()
        .with(CostMatrixStoreSystem, "cost_matrix_store", &[])
        .with_barrier()
        .with(MemoryArbiterSystem, "memory", &[])
        .build();

    main_dispatcher.setup(&mut world);

    //
    // Deserialize world state.
    //

    deserialize_world(&world, COMPONENT_SEGMENT);

    //
    // Prepare globals
    //

    let username = game::rooms::values()
        .iter()
        .filter_map(|room| {
            if let Some(controller) = room.controller() {
                if controller.my() {
                    return controller.owner_name();
                }
            }
            None
        })
        .next();

    if let Some(username) = username {
        user::set_name(&username);
    }

    //
    // Execution
    //

    pre_pass_dispatcher.dispatch(&world);
    world.maintain();

    main_dispatcher.dispatch(&world);
    world.maintain();

    //
    // Cleanup memory.
    //

    cleanup_memory().expect("expected Memory.creeps format to be a regular memory object");

    //
    // Serialize world state.
    //

    serialize_world(&world, COMPONENT_SEGMENT);
}

fn cleanup_memory() -> Result<(), Box<dyn (::std::error::Error)>> {
    let alive_creeps: HashSet<String> = screeps::game::creeps::keys().into_iter().collect();

    let screeps_memory = match screeps::memory::root().dict("creeps")? {
        Some(v) => v,
        None => {
            warn!("not cleaning game creep memory: no Memory.creeps dict");
            return Ok(());
        }
    };

    for mem_name in screeps_memory.keys() {
        if !alive_creeps.contains(&mem_name) {
            debug!("cleaning up creep memory of dead creep {}", mem_name);
            screeps_memory.del(&mem_name);
        }
    }

    Ok(())
}
