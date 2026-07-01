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

/// The controller link's active-priority intake is gated to a horizon of this
/// many ticks of expected drain. A link refills at most once per cooldown
/// (= Chebyshev distance to the sending link, ≤ ~25 for a hub↔controller pair),
/// so 30 ticks of drain comfortably covers a full refill cycle without
/// starving the upgrader — while keeping the advertised demand small enough
/// that surplus link energy overflows to the storage link instead of soaking
/// the 800-capacity controller buffer.
const CONTROLLER_LINK_BUFFER_TICKS: u32 = 30;

/// At or above this fraction of its (gated) buffer the controller link defers to
/// storage: it advertises its remaining deficit at `None` priority instead of
/// `Low`. A `Low` deposit unconditionally out-ranks the storage link's `None`
/// deposit in the link router, so without this a nearly-full link keeps winning
/// the small top-offs and pins itself full — and the surplus never reaches
/// storage. Demoted to `None`, those top-offs lose the router's value ranking to
/// storage's much larger free capacity, so the link hovers at the threshold and
/// the surplus flows to storage. Most visible at RCL≤7, where there is no drain
/// gate and the buffer is the link's full 800 capacity.
const CONTROLLER_LINK_DEFER_FILL: f32 = 0.75;

/// Pure decision for what (if anything) a controller link should advertise as a
/// `Link` deposit, given its energy `capacity`/`used`/`free` and the
/// controller's expected per-tick drain.
///
/// `expected_drain_per_tick`:
///   - `Some(rate)` — controller at max RCL, where the engine caps upgrade at
///     `CONTROLLER_MAX_UPGRADE_PER_TICK` e/t. Only request enough to keep
///     `rate × CONTROLLER_LINK_BUFFER_TICKS` buffered so the surplus overflows
///     to the storage link's `None` deposit instead of being soaked into the
///     controller buffer (the RCL8 storage-link starvation root cause).
///   - `None` — below max RCL, where upgrading is the growth bottleneck and the
///     real drain is bounded by upgrader WORK (not the engine cap); keep the
///     whole link topped.
///
/// Priority escalates as the buffer runs low (so a starving upgrader still wins
/// energy under contention) and de-escalates to `None` once the buffer is mostly
/// full (see [`CONTROLLER_LINK_DEFER_FILL`]), so a nearly-topped link no longer
/// out-prioritizes storage and the surplus flows there. Returns `None` (no
/// request at all) only once the buffer is at or over target.
fn controller_link_deposit(
    capacity: u32,
    used: u32,
    free: u32,
    expected_drain_per_tick: Option<u32>,
) -> Option<(TransferPriority, u32)> {
    let target_buffer = match expected_drain_per_tick {
        Some(drain) => drain.saturating_mul(CONTROLLER_LINK_BUFFER_TICKS).min(capacity),
        None => capacity,
    };

    let deficit = target_buffer.saturating_sub(used).min(free);

    if deficit == 0 {
        return None;
    }

    let fill_fraction = if target_buffer == 0 {
        1.0
    } else {
        (used as f32) / (target_buffer as f32)
    };

    let priority = if fill_fraction < 0.25 {
        TransferPriority::High
    } else if fill_fraction < 0.5 {
        TransferPriority::Medium
    } else if fill_fraction < CONTROLLER_LINK_DEFER_FILL {
        TransferPriority::Low
    } else {
        // Mostly full: defer to storage (still advertise the deficit so the link
        // can top off if storage can't take the energy, but at `None` so it no
        // longer out-prioritizes storage).
        TransferPriority::None
    };

    Some((priority, deficit))
}

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

            // Boxed generator, flushed lazily — no &mut service handle can
            // ride here; None = plain per-search cap (see create_structure_data).
            let mut structure_data = structure_data.maybe_access(
                |d| game::time().saturating_sub(d.last_updated) >= 10 && has_visibility,
                || create_structure_data(room_data, None),
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
                // Safe on general stores (engine-mechanics folklore row 26).
                let container_free_capacity = container.store().get_free_capacity(None).max(0) as u32;
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

#[cfg(test)]
mod tests {
    use super::*;

    // At max RCL the controller drains only CONTROLLER_MAX_UPGRADE_PER_TICK (15)
    // e/t, so the gated buffer is 15 * CONTROLLER_LINK_BUFFER_TICKS (30) = 450,
    // below the 800 link capacity.
    const MAX_LEVEL_DRAIN: Option<u32> = Some(CONTROLLER_MAX_UPGRADE_PER_TICK);

    // Pin (RCL8 storage-link starvation fix): once the controller link holds its
    // gated buffer, it advertises NO active deposit, so source links fall through
    // to the storage link (None) in link_transfer and the surplus reaches storage.
    // This is the regression the operator observed: the controller link soaking
    // all link energy via its old unconditional free-capacity Low deposit.
    #[test]
    fn controller_link_full_gated_buffer_requests_nothing() {
        assert_eq!(controller_link_deposit(800, 450, 350, MAX_LEVEL_DRAIN), None);
        // Over-buffered (e.g. just before an upgrader drains it) — still nothing.
        assert_eq!(controller_link_deposit(800, 600, 200, MAX_LEVEL_DRAIN), None);
    }

    // Pin: below the buffer, the controller link tops up only its (small) gated
    // deficit — not the full free capacity — so the rest overflows to storage.
    #[test]
    fn controller_link_tops_up_only_the_gated_deficit() {
        // used 300 of a 450 buffer -> deficit 150, fill 0.667 -> Low.
        assert_eq!(controller_link_deposit(800, 300, 500, MAX_LEVEL_DRAIN), Some((TransferPriority::Low, 150)));
    }

    // Pin (operator requirement): priority escalates as the buffer runs low so a
    // starving upgrader still wins energy under contention.
    #[test]
    fn controller_link_escalates_priority_when_low() {
        // < 25% of buffer (used 100/450 = 0.22) -> High.
        assert_eq!(controller_link_deposit(800, 100, 700, MAX_LEVEL_DRAIN), Some((TransferPriority::High, 350)));
        // Empty -> High, request the whole buffer.
        assert_eq!(controller_link_deposit(800, 0, 800, MAX_LEVEL_DRAIN), Some((TransferPriority::High, 450)));
        // 25%-50% (used 200/450 = 0.44) -> Medium.
        assert_eq!(controller_link_deposit(800, 200, 600, MAX_LEVEL_DRAIN), Some((TransferPriority::Medium, 250)));
    }

    // Pin: below max RCL (None) the controller is the growth bottleneck, so
    // while the link is below the defer threshold it out-prioritizes storage,
    // escalating as it empties.
    #[test]
    fn controller_link_below_max_prioritizes_until_threshold() {
        // 12.5% full -> High, full deficit.
        assert_eq!(controller_link_deposit(800, 100, 700, None), Some((TransferPriority::High, 700)));
        // 37.5% full -> Medium.
        assert_eq!(controller_link_deposit(800, 300, 500, None), Some((TransferPriority::Medium, 500)));
        // 62.5% full (below the 0.75 defer threshold) -> Low.
        assert_eq!(controller_link_deposit(800, 500, 300, None), Some((TransferPriority::Low, 300)));
    }

    // Pin (operator requirement, RCL≤7): once the link is mostly full it defers
    // to storage. The remaining (small) deficit is advertised at None, NOT Low,
    // so source links stop pinning the link full and the surplus reaches storage.
    #[test]
    fn controller_link_defers_to_storage_when_nearly_full() {
        // At the 0.75 defer threshold -> None (not Low).
        assert_eq!(controller_link_deposit(800, 600, 200, None), Some((TransferPriority::None, 200)));
        // A small top-off near the brim -> None.
        assert_eq!(controller_link_deposit(800, 720, 80, None), Some((TransferPriority::None, 80)));
        // Completely full -> no request at all.
        assert_eq!(controller_link_deposit(800, 800, 0, None), None);
        // Same defer behaviour at max RCL: used 400 of the 450 buffer -> None.
        assert_eq!(controller_link_deposit(800, 400, 400, MAX_LEVEL_DRAIN), Some((TransferPriority::None, 50)));
    }
}
