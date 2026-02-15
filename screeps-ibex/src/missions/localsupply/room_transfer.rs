use super::structure_data::*;
use crate::missions::data::*;
use crate::missions::missionsystem::*;
use crate::remoteobjectid::*;
use crate::serialize::*;
use crate::store::*;
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

        let structure_data_rc = system_data.supply_structure_cache.get_room(self.room_name);
        let mut structure_data = structure_data_rc.maybe_access(
            |d| game::time() - d.last_updated >= 10 && has_visibility,
            || create_structure_data(room_data),
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
                                    link_pos.into(),
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

            let mut structure_data = structure_data.maybe_access(
                |d| game::time() - d.last_updated >= 10 && has_visibility,
                || create_structure_data(room_data),
            );
            let Some(structure_data) = structure_data.get() else {
                return Ok(());
            };

            Self::request_transfer_for_spawns(transfer, &structure_data.spawns);
            Self::request_transfer_for_extension(transfer, &structure_data.extensions);
            Self::request_transfer_for_storage(transfer, &structure_data.storage);
            Self::request_transfer_for_containers(transfer, structure_data);

            if let Some(room) = game::rooms().get(room_data.name) {
                Self::request_transfer_for_ruins(transfer, &room);
                Self::request_transfer_for_tombstones(transfer, &room);
                Self::request_transfer_for_dropped_resources(transfer, &room);
            }

            Ok(())
        })
    }

    fn transfer_request_link_generator(room_entity: Entity, structure_data: Rc<RefCell<Option<StructureData>>>) -> TransferQueueGenerator {
        Box::new(move |system, transfer, _room_name| {
            let room_data = system.get_room_data(room_entity).ok_or("Expected room data")?;
            let has_visibility = room_data.get_dynamic_visibility_data().map(|v| v.visible()).unwrap_or(false);

            let mut structure_data = structure_data.maybe_access(
                |d| game::time() - d.last_updated >= 10 && has_visibility,
                || create_structure_data(room_data),
            );
            let Some(structure_data) = structure_data.get() else {
                return Ok(());
            };

            Self::request_transfer_for_source_links(transfer, structure_data);
            Self::request_transfer_for_storage_links(transfer, structure_data);
            Self::request_transfer_for_controller_links(transfer, structure_data);

            Ok(())
        })
    }

    fn request_transfer_for_containers(transfer: &mut dyn TransferRequestSystem, structure_data: &StructureData) {
        let provider_containers = structure_data
            .sources_to_containers
            .values()
            .chain(structure_data.mineral_extractors_to_containers.values());

        for containers in provider_containers {
            for container_id in containers {
                if let Some(container) = container_id.resolve() {
                    let container_used_capacity = container.store().get_used_capacity(None);
                    if container_used_capacity > 0 {
                        let container_store_capacity = container.store().get_capacity(None);

                        let storage_fraction = (container_used_capacity as f32) / (container_store_capacity as f32);
                        let priority = if storage_fraction > 0.75 {
                            TransferPriority::Medium
                        } else if storage_fraction > 0.5 {
                            TransferPriority::Low
                        } else {
                            TransferPriority::None
                        };

                        for resource in container.store().store_types() {
                            let resource_amount = container.store().get_used_capacity(Some(resource));
                            let transfer_request = TransferWithdrawRequest::new(
                                TransferTarget::Container(*container_id),
                                resource,
                                priority,
                                resource_amount,
                                TransferType::Haul,
                            );

                            transfer.request_withdraw(transfer_request);
                        }
                    }
                }
            }
        }

        for containers in structure_data.controllers_to_containers.values() {
            for container_id in containers {
                if let Some(container) = container_id.resolve() {
                    let container_used_capacity = container.store().get_used_capacity(Some(ResourceType::Energy));
                    let container_available_capacity = container.store().get_capacity(Some(ResourceType::Energy));
                    let container_free_capacity = container_available_capacity - container_used_capacity;

                    let storage_fraction = container_used_capacity as f32 / container_available_capacity as f32;

                    if container_free_capacity > 0 {
                        let priority = if storage_fraction < 0.75 {
                            TransferPriority::Low
                        } else {
                            TransferPriority::None
                        };

                        let transfer_request = TransferDepositRequest::new(
                            TransferTarget::Container(*container_id),
                            Some(ResourceType::Energy),
                            priority,
                            container_free_capacity,
                            TransferType::Haul,
                        );

                        transfer.request_deposit(transfer_request);
                    }

                    let container_used_capacity = container.store().get_used_capacity(Some(ResourceType::Energy));
                    if container_used_capacity > 0 {
                        let transfer_request = TransferWithdrawRequest::new(
                            TransferTarget::Container(*container_id),
                            ResourceType::Energy,
                            TransferPriority::None,
                            container_used_capacity,
                            TransferType::Use,
                        );

                        transfer.request_withdraw(transfer_request);
                    }
                }
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
            if let Some(container) = container_id.resolve() {
                let container_free_capacity = container.expensive_store_free_capacity();
                if container_free_capacity > 0 {
                    let transfer_request = TransferDepositRequest::new(
                        TransferTarget::Container(*container_id),
                        None,
                        TransferPriority::None,
                        container_free_capacity,
                        TransferType::Haul,
                    );

                    transfer.request_deposit(transfer_request);
                }

                for resource in container.store().store_types() {
                    let resource_amount = container.store().get_used_capacity(Some(resource));
                    let transfer_request = TransferWithdrawRequest::new(
                        TransferTarget::Container(*container_id),
                        resource,
                        TransferPriority::None,
                        resource_amount,
                        TransferType::Haul,
                    );

                    transfer.request_withdraw(transfer_request);
                }
            }
        }
    }

    fn request_transfer_for_spawns(transfer: &mut dyn TransferRequestSystem, spawns: &[RemoteObjectId<StructureSpawn>]) {
        for spawn_id in spawns.iter() {
            if let Some(spawn) = spawn_id.resolve() {
                let free_capacity = spawn.store().get_free_capacity(Some(ResourceType::Energy));
                if free_capacity > 0 {
                    let transfer_request = TransferDepositRequest::new(
                        TransferTarget::Spawn(*spawn_id),
                        Some(ResourceType::Energy),
                        TransferPriority::High,
                        free_capacity as u32,
                        TransferType::Haul,
                    );

                    transfer.request_deposit(transfer_request);
                }
            }
        }
    }

    fn request_transfer_for_extension(transfer: &mut dyn TransferRequestSystem, extensions: &[RemoteObjectId<StructureExtension>]) {
        for extension_id in extensions.iter() {
            if let Some(extension) = extension_id.resolve() {
                let free_capacity = extension.store().get_free_capacity(Some(ResourceType::Energy));
                if free_capacity > 0 {
                    let transfer_request = TransferDepositRequest::new(
                        TransferTarget::Extension(*extension_id),
                        Some(ResourceType::Energy),
                        TransferPriority::High,
                        free_capacity as u32,
                        TransferType::Haul,
                    );

                    transfer.request_deposit(transfer_request);
                }
            }
        }
    }

    fn request_transfer_for_storage(transfer: &mut dyn TransferRequestSystem, stores: &[RemoteObjectId<StructureStorage>]) {
        for storage_id in stores.iter() {
            if let Some(storage) = storage_id.resolve() {
                let mut used_capacity = 0;

                for resource in storage.store().store_types() {
                    let resource_amount = storage.store().get_used_capacity(Some(resource));
                    let transfer_request = TransferWithdrawRequest::new(
                        TransferTarget::Storage(*storage_id),
                        resource,
                        TransferPriority::None,
                        resource_amount,
                        TransferType::Haul,
                    );

                    transfer.request_withdraw(transfer_request);

                    used_capacity += resource_amount;
                }

                let free_capacity = storage.store().get_capacity(None) - used_capacity;

                if free_capacity > 0 {
                    let transfer_request = TransferDepositRequest::new(
                        TransferTarget::Storage(*storage_id),
                        None,
                        TransferPriority::None,
                        free_capacity,
                        TransferType::Haul,
                    );

                    transfer.request_deposit(transfer_request);
                }
            }
        }
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
                    let storage_fraction = (used_capacity as f32) / (available_capacity as f32);

                    let priority = if storage_fraction > 0.5 {
                        TransferPriority::High
                    } else if storage_fraction > 0.25 {
                        TransferPriority::Low
                    } else {
                        TransferPriority::None
                    };

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
                    let storage_fraction = (used_capacity as f32) / (available_capacity as f32);

                    let priority = if storage_fraction > 0.5 {
                        TransferPriority::High
                    } else if storage_fraction > 0.25 {
                        TransferPriority::Medium
                    } else {
                        TransferPriority::Low
                    };

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

    fn request_transfer_for_controller_links(transfer: &mut dyn TransferRequestSystem, structure_data: &StructureData) {
        for link_id in &structure_data.controller_links {
            if let Some(link) = link_id.resolve() {
                let free_capacity = link.store().get_free_capacity(Some(ResourceType::Energy));

                if free_capacity > 1 {
                    let transfer_request = TransferDepositRequest::new(
                        TransferTarget::Link(link.remote_id()),
                        Some(ResourceType::Energy),
                        TransferPriority::Low,
                        free_capacity as u32,
                        TransferType::Link,
                    );

                    transfer.request_deposit(transfer_request);
                }

                let used_capacity = link.store().get_used_capacity(Some(ResourceType::Energy));

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

    fn request_transfer_for_ruins(transfer: &mut dyn TransferRequestSystem, room: &Room) {
        for ruin in room.find(find::RUINS, None) {
            let ruin_id = ruin.remote_id();

            for resource in ruin.store().store_types() {
                let resource_amount = ruin.store().get_used_capacity(Some(resource));
                let transfer_request = TransferWithdrawRequest::new(
                    TransferTarget::Ruin(ruin_id),
                    resource,
                    TransferPriority::Medium,
                    resource_amount,
                    TransferType::Haul,
                );

                transfer.request_withdraw(transfer_request);
            }
        }
    }

    fn request_transfer_for_tombstones(transfer: &mut dyn TransferRequestSystem, room: &Room) {
        for tombstone in room.find(find::TOMBSTONES, None) {
            let tombstone_id = tombstone.remote_id();

            for resource in tombstone.store().store_types() {
                let resource_amount = tombstone.store().get_used_capacity(Some(resource));

                //TODO: Only apply this if no hostiles in the room?
                let priority = if resource_amount > 200 || resource != ResourceType::Energy {
                    TransferPriority::High
                } else {
                    TransferPriority::Medium
                };

                let transfer_request = TransferWithdrawRequest::new(
                    TransferTarget::Tombstone(tombstone_id),
                    resource,
                    priority,
                    resource_amount,
                    TransferType::Haul,
                );

                transfer.request_withdraw(transfer_request);
            }
        }
    }

    fn request_transfer_for_dropped_resources(transfer: &mut dyn TransferRequestSystem, room: &Room) {
        for dropped_resource in room.find(find::DROPPED_RESOURCES, None) {
            let dropped_resource_id = dropped_resource.remote_id();

            let resource = dropped_resource.resource_type();
            let resource_amount = dropped_resource.amount();

            //TODO: Only apply this if no hostiles in the room?
            let priority = if resource_amount > 500 || resource != ResourceType::Energy {
                TransferPriority::High
            } else {
                TransferPriority::Medium
            };

            let transfer_request = TransferWithdrawRequest::new(
                TransferTarget::Resource(dropped_resource_id),
                resource,
                priority,
                resource_amount,
                TransferType::Haul,
            );

            transfer.request_withdraw(transfer_request);
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

    fn get_room(&self) -> Entity {
        self.room_data
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
