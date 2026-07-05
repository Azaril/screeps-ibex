use super::structure_data::*;
use crate::missions::data::*;
use crate::missions::missionsystem::*;
use crate::remoteobjectid::*;
use crate::serialize::*;
use crate::transfer::transfersystem::*;
use screeps::*;
use screeps_cache::*;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use std::cell::*;
use std::rc::*;

// The K1 demand-registration policy (the per-structure tier ladders, `controller_link_deposit`
// + its buffer/defer constants, and the link withdraw ladders) lives in
// `screeps_econ_decision::demand` since ADR 0040 M3 — one implementation, consumed by this
// mission's generator adapters AND the economy sim (`screeps-econ-eval`). The generators below
// build a [`RoomEconDto`] from live handles and execute the returned [`Demand`]s; their pinned
// tests moved with the kernel.
use screeps_econ_decision::demand::{
    controller_link_deposit, room_haul_demand, ContainerDto, ContainerRole, Demand, DemandSide, DroppedDto, ItemRef, LootDto,
    RefillStructDto, RoomEconDto, StorageDto, source_link_withdraw_priority, storage_link_withdraw_priority,
};

pub struct RoomTransferMission {
    owner: EntityOption<Entity>,
    room_data: Entity,
    room_name: RoomName,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(bound = "MA: Marker")]
pub struct RoomTransferMissionSaveloadData<MA>
where
    MA: Marker + Serialize,
    for<'deser> MA: Deserialize<'deser>,
{
    owner: <EntityOption<Entity> as ConvertSaveload<MA>>::Data,
    room_data: <Entity as ConvertSaveload<MA>>::Data,
    room_name: <RoomName as ConvertSaveload<MA>>::Data,
}

impl<MA> ConvertSaveload<MA> for RoomTransferMission
where
    MA: Marker + Serialize,
    for<'deser> MA: Deserialize<'deser>,
{
    type Data = RoomTransferMissionSaveloadData<MA>;
    #[allow(deprecated)]
    type Error = NoError;

    fn convert_into<F>(&self, mut ids: F) -> Result<Self::Data, Self::Error>
    where
        F: FnMut(Entity) -> Option<MA>,
    {
        Ok(RoomTransferMissionSaveloadData {
            owner: ConvertSaveload::convert_into(&self.owner, &mut ids)?,
            room_data: ConvertSaveload::convert_into(&self.room_data, &mut ids)?,
            room_name: ConvertSaveload::convert_into(&self.room_name, &mut ids)?,
        })
    }

    fn convert_from<F>(data: Self::Data, mut ids: F) -> Result<Self, Self::Error>
    where
        F: FnMut(MA) -> Option<Entity>,
    {
        Ok(RoomTransferMission {
            owner: ConvertSaveload::convert_from(data.owner, &mut ids)?,
            room_data: ConvertSaveload::convert_from(data.room_data, &mut ids)?,
            room_name: ConvertSaveload::convert_from(data.room_name, &mut ids)?,
        })
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl RoomTransferMission {
    pub fn build<B>(builder: B, owner: Option<Entity>, room_data: Entity, room_name: RoomName) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = RoomTransferMission {
            owner: owner.into(),
            room_data,
            room_name,
        };

        builder
            .with(MissionData::RoomTransfer(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    fn get_all_links(&mut self, system_data: &mut MissionExecutionSystemData) -> Result<Vec<RemoteObjectId<StructureLink>>, String> {
        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;
        let has_visibility = room_data.get_dynamic_visibility_data().map(|v| v.visible()).unwrap_or(false);

        let pathfinder = &mut *system_data.pathfinder;
        let structure_data_rc = system_data.supply_structure_cache.get_room(self.room_name);
        let mut structure_data = structure_data_rc.maybe_access(
            |d| game::time().saturating_sub(d.last_updated) >= 10 && has_visibility,
            || create_structure_data(room_data, Some(pathfinder)),
        );
        let structure_data = structure_data.get().ok_or("Expected structure data")?;

        let all_links = structure_data
            .sources_to_links
            .values()
            .flatten()
            .chain(structure_data.storage_links.iter())
            .cloned()
            .collect();

        Ok(all_links)
    }

    fn link_transfer(&mut self, system_data: &mut MissionExecutionSystemData) -> Result<(), String> {
        if let Ok(all_links) = self.get_all_links(system_data) {
            let transfer_queue = &mut system_data.transfer_queue;

            let transfer_queue_data = TransferQueueGeneratorData {
                cause: "Link Transfer",
                room_data: &*system_data.room_data,
            };

            for link_id in all_links {
                if let Some(link) = link_id.resolve() {
                    if link.cooldown() == 0 && link.store().get(ResourceType::Energy).unwrap_or(0) > 0 {
                        let link_pos = link.pos();
                        let room_name = link_pos.room_name();

                        //TODO: Potentially use active priority pairs to iterate here.
                        let best_transfer = ALL_TRANSFER_PRIORITIES
                            .iter()
                            .filter_map(|priority| {
                                transfer_queue.get_delivery_from_target(
                                    &transfer_queue_data,
                                    &[room_name],
                                    &TransferTarget::Link(link_id),
                                    TransferPriorityFlags::ACTIVE,
                                    priority.into(),
                                    TransferType::Link,
                                    TransferCapacity::Infinite,
                                    link_pos,
                                    target_filters::link,
                                )
                            })
                            .next();

                        if let Some((pickup, delivery)) = best_transfer {
                            transfer_queue.register_pickup(&pickup);
                            transfer_queue.register_delivery(&delivery);

                            //TODO: Validate there isn't non-energy in here?
                            let transfer_amount = delivery
                                .resources()
                                .get(&ResourceType::Energy)
                                .map(|entries| entries.iter().map(|entry| entry.amount()).sum())
                                .unwrap_or(0);

                            let _ = delivery.target().link_transfer_energy_amount(&link, transfer_amount);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn transfer_request_haul_generator(room_entity: Entity, structure_data: Rc<RefCell<Option<StructureData>>>) -> TransferQueueGenerator {
        Box::new(move |system, transfer, _room_name| {
            let room_data = system.get_room_data(room_entity).ok_or("Expected room data")?;
            let has_visibility = room_data.get_dynamic_visibility_data().map(|v| v.visible()).unwrap_or(false);

            // Boxed generator, flushed lazily — no &mut service handle can
            // ride here; None = plain per-search cap (see create_structure_data).
            let mut structure_data = structure_data.maybe_access(
                |d| game::time().saturating_sub(d.last_updated) >= 10 && has_visibility,
                || create_structure_data(room_data, None),
            );
            let Some(structure_data) = structure_data.get() else {
                return Ok(());
            };

            // Build the K1 view (RoomEconDto) from live handles, run the kernel, execute the
            // demands (ADR 0040 M3 — the policy lives in screeps_econ_decision::demand).
            let (dto, targets) = Self::build_room_econ_dto(structure_data, room_data.name);
            Self::execute_demands(transfer, &targets, room_haul_demand(&dto));

            Ok(())
        })
    }

    /// Build the K1 [`RoomEconDto`] + the aligned target table from the live structure cache
    /// (unresolvable ids — stale cache entries — are skipped, exactly like the pre-move
    /// per-structure `resolve()` guards).
    fn build_room_econ_dto(structure_data: &StructureData, room_name: RoomName) -> (RoomEconDto, Vec<TransferTarget>) {
        let mut targets: Vec<TransferTarget> = Vec::new();
        let mut dto = RoomEconDto::default();

        fn item(targets: &mut Vec<TransferTarget>, target: TransferTarget) -> ItemRef {
            targets.push(target);
            ItemRef(targets.len() as u32 - 1)
        }

        for spawn_id in &structure_data.spawns {
            if let Some(spawn) = spawn_id.resolve() {
                dto.spawns.push(RefillStructDto {
                    item: item(&mut targets, TransferTarget::Spawn(*spawn_id)),
                    free_energy: spawn.store().get_free_capacity(Some(ResourceType::Energy)).max(0) as u32,
                });
            }
        }

        for extension_id in &structure_data.extensions {
            if let Some(extension) = extension_id.resolve() {
                dto.extensions.push(RefillStructDto {
                    item: item(&mut targets, TransferTarget::Extension(*extension_id)),
                    free_energy: extension.store().get_free_capacity(Some(ResourceType::Energy)).max(0) as u32,
                });
            }
        }

        // Containers, grouped by role exactly as the pre-move arms classified them: provider
        // (source + mineral), controller, then everything else.
        let push_container = |targets: &mut Vec<TransferTarget>,
                                  dto: &mut RoomEconDto,
                                  container_id: &RemoteObjectId<StructureContainer>,
                                  role: ContainerRole| {
            if let Some(container) = container_id.resolve() {
                let store = container
                    .store()
                    .store_types()
                    .into_iter()
                    .map(|r| (r, container.store().get_used_capacity(Some(r))))
                    .collect();
                dto.containers.push(ContainerDto {
                    item: item(targets, TransferTarget::Container(*container_id)),
                    role,
                    store,
                    capacity: container.store().get_capacity(None),
                });
            }
        };

        let provider_containers = structure_data
            .sources_to_containers
            .values()
            .chain(structure_data.mineral_extractors_to_containers.values());
        for containers in provider_containers {
            for container_id in containers {
                push_container(&mut targets, &mut dto, container_id, ContainerRole::Provider);
            }
        }
        for containers in structure_data.controllers_to_containers.values() {
            for container_id in containers {
                push_container(&mut targets, &mut dto, container_id, ContainerRole::Controller);
            }
        }
        let storage_containers = structure_data.containers.iter().filter(|container| {
            !structure_data.sources_to_containers.values().any(|c| c.contains(container))
                && !structure_data.controllers_to_containers.values().any(|c| c.contains(container))
                && !structure_data
                    .mineral_extractors_to_containers
                    .values()
                    .any(|c| c.contains(container))
        });
        for container_id in storage_containers {
            push_container(&mut targets, &mut dto, container_id, ContainerRole::Other);
        }

        for storage_id in &structure_data.storage {
            if let Some(storage) = storage_id.resolve() {
                let store = storage
                    .store()
                    .store_types()
                    .into_iter()
                    .map(|r| (r, storage.store().get_used_capacity(Some(r))))
                    .collect();
                dto.storage.push(StorageDto {
                    item: item(&mut targets, TransferTarget::Storage(*storage_id)),
                    store,
                    capacity: storage.store().get_capacity(None),
                });
            }
        }

        if let Some(room) = game::rooms().get(room_name) {
            for ruin in room.find(find::RUINS, None) {
                let store = ruin
                    .store()
                    .store_types()
                    .into_iter()
                    .map(|r| (r, ruin.store().get_used_capacity(Some(r))))
                    .collect();
                dto.ruins.push(LootDto {
                    item: item(&mut targets, TransferTarget::Ruin(ruin.remote_id())),
                    store,
                });
            }
            for tombstone in room.find(find::TOMBSTONES, None) {
                let store = tombstone
                    .store()
                    .store_types()
                    .into_iter()
                    .map(|r| (r, tombstone.store().get_used_capacity(Some(r))))
                    .collect();
                dto.tombstones.push(LootDto {
                    item: item(&mut targets, TransferTarget::Tombstone(tombstone.remote_id())),
                    store,
                });
            }
            for dropped_resource in room.find(find::DROPPED_RESOURCES, None) {
                dto.dropped.push(DroppedDto {
                    item: item(&mut targets, TransferTarget::Resource(dropped_resource.remote_id())),
                    resource: dropped_resource.resource_type(),
                    amount: dropped_resource.amount(),
                });
            }
        }

        (dto, targets)
    }

    /// Execute K1 demands against the transfer queue (the write half of the seam —
    /// `RegisterWithdraw`/`RegisterDeposit` intents).
    fn execute_demands(transfer: &mut dyn TransferRequestSystem, targets: &[TransferTarget], demands: Vec<Demand>) {
        for demand in demands {
            let target = targets[demand.item.0 as usize];
            match demand.side {
                DemandSide::Withdraw => transfer.request_withdraw(TransferWithdrawRequest::new(
                    target,
                    demand.resource.expect("withdraw demands always carry a resource"),
                    demand.priority,
                    demand.amount,
                    demand.transfer_type,
                )),
                DemandSide::Deposit => transfer.request_deposit(TransferDepositRequest::new(
                    target,
                    demand.resource,
                    demand.priority,
                    demand.amount,
                    demand.transfer_type,
                )),
            }
        }
    }

    fn transfer_request_link_generator(room_entity: Entity, structure_data: Rc<RefCell<Option<StructureData>>>) -> TransferQueueGenerator {
        Box::new(move |system, transfer, _room_name| {
            let room_data = system.get_room_data(room_entity).ok_or("Expected room data")?;
            let has_visibility = room_data.get_dynamic_visibility_data().map(|v| v.visible()).unwrap_or(false);

            // Boxed generator, flushed lazily — None = plain per-search cap.
            let mut structure_data = structure_data.maybe_access(
                |d| game::time().saturating_sub(d.last_updated) >= 10 && has_visibility,
                || create_structure_data(room_data, None),
            );
            let Some(structure_data) = structure_data.get() else {
                return Ok(());
            };

            Self::request_transfer_for_source_links(transfer, structure_data);
            Self::request_transfer_for_storage_links(transfer, structure_data);

            // Gate the controller link's active-priority intake to the
            // controller's expected drain. At max RCL the engine caps upgrade
            // at CONTROLLER_MAX_UPGRADE_PER_TICK e/t, so only that much needs
            // buffering and the surplus can overflow to storage; below max the
            // controller is the growth bottleneck so keep it fully fed (None).
            let expected_drain_per_tick = room_data
                .get_structures()
                .and_then(|structures| structures.controllers().iter().map(|controller| controller.level()).max())
                .filter(|level| controller_levels(*level as u32).is_none())
                .map(|_| CONTROLLER_MAX_UPGRADE_PER_TICK);

            Self::request_transfer_for_controller_links(transfer, structure_data, expected_drain_per_tick);

            Ok(())
        })
    }

    fn request_transfer_for_storage_links(transfer: &mut dyn TransferRequestSystem, structure_data: &StructureData) {
        for link_id in &structure_data.storage_links {
            if let Some(link) = link_id.resolve() {
                let free_capacity = link.store().get_free_capacity(Some(ResourceType::Energy));

                if free_capacity > 1 {
                    let transfer_request = TransferDepositRequest::new(
                        TransferTarget::Link(link.remote_id()),
                        Some(ResourceType::Energy),
                        TransferPriority::None,
                        free_capacity as u32,
                        TransferType::Link,
                    );

                    transfer.request_deposit(transfer_request);
                }

                let used_capacity = link.store().get_used_capacity(Some(ResourceType::Energy));

                if used_capacity > 0 {
                    let available_capacity = link.store().get_capacity(Some(ResourceType::Energy));
                    // The fill ladder lives in the K1 kernel (ADR 0040 M3).
                    let priority = storage_link_withdraw_priority(used_capacity, available_capacity);

                    let transfer_request = TransferWithdrawRequest::new(
                        TransferTarget::Link(link.remote_id()),
                        ResourceType::Energy,
                        priority,
                        used_capacity,
                        TransferType::Haul,
                    );

                    transfer.request_withdraw(transfer_request);
                }
            }
        }
    }

    fn request_transfer_for_source_links(transfer: &mut dyn TransferRequestSystem, structure_data: &StructureData) {
        for link_id in structure_data.sources_to_links.values().flatten() {
            if let Some(link) = link_id.resolve() {
                let used_capacity = link.store().get_used_capacity(Some(ResourceType::Energy));

                if used_capacity > 0 {
                    let available_capacity = link.store().get_capacity(Some(ResourceType::Energy));
                    // The fill ladder lives in the K1 kernel (ADR 0040 M3).
                    let priority = source_link_withdraw_priority(used_capacity, available_capacity);

                    let transfer_request = TransferWithdrawRequest::new(
                        TransferTarget::Link(link.remote_id()),
                        ResourceType::Energy,
                        priority,
                        used_capacity,
                        TransferType::Link,
                    );

                    transfer.request_withdraw(transfer_request);
                }
            }
        }
    }

    fn request_transfer_for_controller_links(
        transfer: &mut dyn TransferRequestSystem,
        structure_data: &StructureData,
        expected_drain_per_tick: Option<u32>,
    ) {
        for link_id in &structure_data.controller_links {
            if let Some(link) = link_id.resolve() {
                let capacity = link.store().get_capacity(Some(ResourceType::Energy));
                let used_capacity = link.store().get_used_capacity(Some(ResourceType::Energy));
                // Safe on general stores (engine-mechanics folklore row 26).
                let free_capacity = link.store().get_free_capacity(Some(ResourceType::Energy)).max(0) as u32;

                // Demand is gated to the expected drain and escalates as the
                // buffer runs low (see `controller_link_deposit`).
                if let Some((priority, amount)) =
                    controller_link_deposit(capacity, used_capacity, free_capacity, expected_drain_per_tick)
                {
                    let transfer_request = TransferDepositRequest::new(
                        TransferTarget::Link(link.remote_id()),
                        Some(ResourceType::Energy),
                        priority,
                        amount,
                        TransferType::Link,
                    );

                    transfer.request_deposit(transfer_request);
                }

                let transfer_request = TransferWithdrawRequest::new(
                    TransferTarget::Link(link.remote_id()),
                    ResourceType::Energy,
                    TransferPriority::None,
                    used_capacity,
                    TransferType::Use,
                );

                transfer.request_withdraw(transfer_request);
            }
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for RoomTransferMission {
    fn get_owner(&self) -> &Option<Entity> {
        &self.owner
    }

    fn owner_complete(&mut self, owner: Entity) {
        assert!(Some(owner) == *self.owner);
        self.owner.take();
    }

    fn get_room(&self) -> Option<Entity> {
        Some(self.room_data)
    }

    fn describe_state(&self, _system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> String {
        "Room Transfer".to_string()
    }

    fn summarize(&self) -> crate::visualization::SummaryContent {
        crate::visualization::SummaryContent::Text("Room Transfer".to_string())
    }

    fn pre_run_mission(&mut self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<(), String> {
        let structure_data_rc = system_data.supply_structure_cache.get_room(self.room_name);

        system_data.transfer_queue.register_generator(
            self.room_name,
            TransferTypeFlags::HAUL | TransferTypeFlags::USE,
            Self::transfer_request_haul_generator(self.room_data, structure_data_rc.clone()),
        );

        system_data.transfer_queue.register_generator(
            self.room_name,
            TransferTypeFlags::HAUL | TransferTypeFlags::LINK | TransferTypeFlags::USE,
            Self::transfer_request_link_generator(self.room_data, structure_data_rc),
        );

        Ok(())
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<MissionResult, String> {
        self.link_transfer(system_data)?;

        Ok(MissionResult::Running)
    }
}

