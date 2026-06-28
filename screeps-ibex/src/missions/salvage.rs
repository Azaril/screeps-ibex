use super::constants::*;
use super::data::*;
use super::missionsystem::*;
use super::utility::*;
use crate::creep::*;
use crate::jobs::data::*;
use crate::jobs::dismantle::*;
use crate::jobs::haul::*;
use crate::jobs::utility::dismantle::*;
use crate::jobs::utility::dismantlebehavior::*;
use crate::military::objective_queue::*;
use crate::remoteobjectid::*;
use crate::room::data::*;
use crate::room::visibilitysystem::*;
use crate::serialize::*;
use crate::spawnsystem::*;
use crate::transfer::transfersystem::*;
use log::*;
use screeps::*;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use std::convert::*;

/// Loot volume one raider is assumed to clear per lifetime (≈500 capacity at
/// short range); sizes the raider roster from observed loot.
const RAIDER_LOOT_PER_LIFETIME: u32 = 25_000;
const MAX_RAIDERS: usize = 3;
/// Dismantle hit pool above which a second dismantler is worth spawning.
const SECOND_DISMANTLER_HITS: u32 = 1_000_000;

/// Salvage work observed in a room. `dismantle_hits` is decay-adjusted and
/// horizon-filtered; loot quantities are raw store contents in scope.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SalvageWork {
    pub loot_energy: u32,
    pub loot_other: u32,
    pub dismantle_hits: u32,
}

impl SalvageWork {
    pub fn loot_total(&self) -> u32 {
        self.loot_energy.saturating_add(self.loot_other)
    }
}

/// Survey a visible room for salvageable value. `lead_ticks` (travel + spawn
/// lead) discounts decaying structures — value that will rot away before our
/// creeps arrive is not value (ramparts bleed 3 hits/tick; containers 10/tick
/// in owned rooms, 50/tick in unowned). The admission EV gate passes a real
/// lead; in-mission re-surveys pass 0 (creeps are already on site).
pub fn assess_salvage_work(
    structures: &[StructureObject],
    sources: &[RemoteObjectId<Source>],
    max_structure_hits: u32,
    lead_ticks: u32,
    room_owned: bool,
) -> SalvageWork {
    let mut work = SalvageWork::default();
    let hostile_ramparts = hostile_rampart_positions(structures);

    for structure in structures.iter() {
        if is_salvage_loot_target(structure, sources, &hostile_ramparts) {
            if let Some(store) = structure.as_has_store() {
                for resource in store.store().store_types() {
                    let amount = store.store().get_used_capacity(Some(resource));
                    if resource == ResourceType::Energy {
                        work.loot_energy += amount;
                    } else {
                        work.loot_other += amount;
                    }
                }
            }
        }

        let dismantlable = structure.structure_type() != StructureType::Road
            && !ignore_for_dismantle(structure, sources)
            && can_dismantle(structure)
            && within_dismantle_hits_horizon(structure, max_structure_hits)
            && !blocked_by_hostile_rampart(structure, &hostile_ramparts);

        if dismantlable {
            if let Some(attackable) = structure.as_attackable() {
                let decay_per_tick = match structure {
                    StructureObject::StructureRampart(_) => RAMPART_DECAY_AMOUNT / RAMPART_DECAY_TIME,
                    // Containers decay 5x slower in owned rooms — and a
                    // derelict room is still owned until its controller drops.
                    StructureObject::StructureContainer(_) if room_owned => CONTAINER_DECAY / CONTAINER_DECAY_TIME_OWNED,
                    StructureObject::StructureContainer(_) => CONTAINER_DECAY / CONTAINER_DECAY_TIME,
                    _ => 0,
                };

                work.dismantle_hits += attackable.hits().saturating_sub(decay_per_tick * lead_ticks);
            }
        }
    }

    work
}

/// Per-room salvage mission: strips a militarily dead room of its resources
/// with two creep roles — raiders (pure Carry/Move running [`HaulJob`], loot
/// stores home) and dismantlers (Work-heavy running [`DismantleJob`], wreck
/// structures and recover the energy refund). Rosters are sized from the
/// observed work volume; loot-before-wreck falls out of the shared scope
/// filters (dismantle targets must have empty stores).
///
/// Created exclusively by `SalvageOperation` after the EV/strategic gate.
/// The mission owns the in-room lifecycle: it keeps intel fresh, aborts
/// loudly if the room re-arms, is serviced by its owner, or enters safe
/// mode, and completes once no enabled role has work left. Controller
/// takeover is deliberately NOT this mission's job — once the owner's
/// controller decays to neutral, the mining-outpost pipeline takes the room
/// over through its normal candidate flow.
#[derive(ConvertSaveload)]
pub struct SalvageMission {
    owner: EntityOption<Entity>,
    room_data: Entity,
    home_room_datas: EntityVec<Entity>,
    raiders: EntityVec<Entity>,
    dismantlers: EntityVec<Entity>,
}

/// EPHEMERAL (ADR 0027 v1.1 P1): the breach-blocker pos of the v1 `Dismantle`
/// objective each salvage mission last emitted (`request_breach_objective`),
/// keyed by the mission's target room. Lets `withdraw_breach_objective` scope its
/// teardown to OUR objective by `room AND pos`, so it never clobbers `war.rs`'s
/// same-room Attack-owned InvaderCore `Dismantle` (which targets the CORE tile, a
/// different pos — the FIX 2 clobber guard).
///
/// NOT serialized — the project's ephemeral-resource pattern (mirrors
/// [`SquadFormingProgress`](crate::military::squad_manager::SquadFormingProgress)):
/// a `Default`-derived, non-`Serialize` resource auto-created by specs, holding a
/// deterministic `BTreeMap` (no `HashMap` iteration → bit-deterministic). On a VM
/// reset the map starts empty, so a `withdraw` after reset matches nothing and any
/// orphaned breach objective simply TTL-expires (≤200t) — the self-heal the queue
/// already relies on. No `WORLD_FORMAT_VERSION` bump (the persisted `SalvageMission`
/// shape is unchanged; `last_breach_pos` is NOT a struct field).
#[derive(Default)]
pub struct SalvageBreachTracker {
    /// target room → the breach-blocker pos of the live v1 `Dismantle` objective.
    last_breach_pos: std::collections::BTreeMap<RoomName, Position>,
    /// ADR 0027 v1.1 P2: target room → the controller pos of the live v1 `Declaim` objective this mission
    /// emitted, the SIBLING of `last_breach_pos`. Lets `withdraw_declaim_objective` scope its teardown to OUR
    /// `Declaim { room == target AND controller == tracked-pos }` so it removes only our objective. Same
    /// ephemeral-resource discipline as `last_breach_pos` (a `BTreeMap`, NOT serialized → bit-deterministic;
    /// on a VM reset it starts empty and an orphaned `Declaim` objective TTL-expires). No WFV bump for THIS
    /// field (the persisted `SalvageMission` shape is unchanged; the WFV 20→21 bump covers the serialized
    /// `ObjectiveKind::Declaim` variant, not this transient tracker).
    last_declaim_pos: std::collections::BTreeMap<RoomName, Position>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl SalvageMission {
    pub fn build<B>(builder: B, owner: Option<Entity>, room_data: Entity, home_room_datas: &[Entity]) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = SalvageMission::new(owner, room_data, home_room_datas);

        builder
            .with(MissionData::Salvage(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>, room_data: Entity, home_room_datas: &[Entity]) -> SalvageMission {
        SalvageMission {
            owner: owner.into(),
            room_data,
            home_room_datas: home_room_datas.to_owned().into(),
            raiders: EntityVec::new(),
            dismantlers: EntityVec::new(),
        }
    }

    fn create_handle_raider_spawn(
        mission_entity: Entity,
        raid_room: Entity,
        delivery_rooms: &[Entity],
    ) -> crate::spawnsystem::SpawnQueueCallback {
        let delivery_rooms = delivery_rooms.to_owned();

        Box::new(move |spawn_system_data, name| {
            let name = name.to_string();
            let delivery_rooms = delivery_rooms.clone();

            spawn_system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::Haul(HaulJob::new(&[raid_room], &delivery_rooms, false, false));

                let creep_entity = crate::creep::spawning::build(world.create_entity(), &name).with(creep_job).build();

                if let Some(mut mission_data) = world
                    .write_storage::<MissionData>()
                    .get_mut(mission_entity)
                    .as_mission_type_mut::<SalvageMission>()
                {
                    mission_data.raiders.push(creep_entity);
                }
            });
        })
    }

    fn create_handle_dismantler_spawn(
        mission_entity: Entity,
        dismantle_room: Entity,
        delivery_room: Entity,
        max_structure_hits: u32,
    ) -> crate::spawnsystem::SpawnQueueCallback {
        Box::new(move |spawn_system_data, name| {
            let name = name.to_string();

            spawn_system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::Dismantle(DismantleJob::new(dismantle_room, delivery_room, false, max_structure_hits));

                let creep_entity = crate::creep::spawning::build(world.create_entity(), &name).with(creep_job).build();

                if let Some(mut mission_data) = world
                    .write_storage::<MissionData>()
                    .get_mut(mission_entity)
                    .as_mission_type_mut::<SalvageMission>()
                {
                    mission_data.dismantlers.push(creep_entity);
                }
            });
        })
    }

    /// Advertise in-scope loot to the transfer system so raider HaulJobs (and
    /// any other haulers passing through) pick it up.
    fn request_transfer_for_loot(transfer: &mut dyn TransferRequestSystem, room_data: &RoomData) -> Result<(), String> {
        //TODO: Fill out remaining types?
        //Structure::Ruin(s) => Ok(s.into()),
        //Structure::Tombstone(s) => Ok(s.into()),
        //Structure::Resource(s) => Ok(s.into()),

        let structures = match room_data.get_structures() {
            Some(s) => s,
            None => return Ok(()),
        };

        let sources = room_data
            .get_static_visibility_data()
            .map(|s| s.sources().as_slice())
            .unwrap_or(&[]);

        let hostile_ramparts = hostile_rampart_positions(structures.all());

        for structure in structures
            .all()
            .iter()
            .filter(|s| is_salvage_loot_target(*s, sources, &hostile_ramparts))
        {
            if let Some(store) = structure.as_has_store() {
                for resource in store.store().store_types() {
                    let resource_amount = store.store().get_used_capacity(Some(resource));

                    if resource_amount > 0 {
                        if let Ok(transfer_target) = structure.try_into() {
                            let transfer_request = TransferWithdrawRequest::new(
                                transfer_target,
                                resource,
                                TransferPriority::Low,
                                resource_amount,
                                TransferType::Haul,
                            );

                            transfer.request_withdraw(transfer_request);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn spawn_raiders(
        &self,
        system_data: &mut MissionExecutionSystemData,
        mission_entity: Entity,
        room_name: RoomName,
    ) -> Result<(), String> {
        let token = system_data.spawn_queue.token();

        for home_room_entity in self.home_room_datas.iter() {
            let home_room_data = system_data.room_data.get(*home_room_entity).ok_or("Expected home room data")?;
            let home_room = game::rooms().get(home_room_data.name).ok_or("Expected home room")?;

            let body_definition = SpawnBodyDefinition {
                maximum_energy: home_room.energy_capacity_available(),
                minimum_repeat: Some(1),
                maximum_repeat: None,
                pre_body: &[],
                repeat_body: &[Part::Carry, Part::Move],
                post_body: &[],
            };

            if let Ok(body) = crate::creep::spawning::create_body(&body_definition) {
                let spawn_request = SpawnRequest::new(
                    format!("Raider - Target Room: {}", room_name),
                    &body,
                    SPAWN_PRIORITY_LOW,
                    Some(token),
                    Self::create_handle_raider_spawn(mission_entity, self.room_data, &self.home_room_datas),
                );

                system_data.spawn_queue.request(*home_room_entity, spawn_request);
            }
        }

        Ok(())
    }

    fn spawn_dismantlers(
        &self,
        system_data: &mut MissionExecutionSystemData,
        mission_entity: Entity,
        max_structure_hits: u32,
        priority: f32,
    ) -> Result<(), String> {
        let token = system_data.spawn_queue.token();

        for home_room_entity in self.home_room_datas.iter() {
            let home_room_data = system_data.room_data.get(*home_room_entity).ok_or("Expected home room data")?;
            let home_room = game::rooms().get(home_room_data.name).ok_or("Expected home room")?;

            let body_definition = if home_room_data.get_structures().map(|s| !s.storages().is_empty()).unwrap_or(false) {
                SpawnBodyDefinition {
                    maximum_energy: home_room.energy_capacity_available(),
                    minimum_repeat: None,
                    maximum_repeat: None,
                    pre_body: &[Part::Move, Part::Move, Part::Work, Part::Work],
                    repeat_body: &[
                        Part::Work,
                        Part::Work,
                        Part::Move,
                        Part::Move,
                        Part::Carry,
                        Part::Carry,
                        Part::Move,
                        Part::Move,
                        Part::Carry,
                        Part::Carry,
                        Part::Move,
                        Part::Move,
                    ],
                    post_body: &[],
                }
            } else {
                SpawnBodyDefinition {
                    maximum_energy: home_room.energy_capacity_available(),
                    minimum_repeat: Some(1),
                    maximum_repeat: None,
                    pre_body: &[],
                    repeat_body: &[Part::Move, Part::Work],
                    post_body: &[],
                }
            };

            if let Ok(body) = crate::creep::spawning::create_body(&body_definition) {
                let spawn_request = SpawnRequest::new(
                    "Dismantler".to_string(),
                    &body,
                    priority,
                    Some(token),
                    Self::create_handle_dismantler_spawn(mission_entity, self.room_data, *home_room_entity, max_structure_hits),
                );

                system_data.spawn_queue.request(*home_room_entity, spawn_request);
            }
        }

        Ok(())
    }

    /// Withdraw the v1 `Declaim` objective THIS mission emitted (ADR 0027 v1.1 P2). Scoped to
    /// `Declaim { room == target AND controller == tracked-pos }` — the exact `(room, controller)` we last
    /// requested (from the ephemeral [`SalvageBreachTracker::last_declaim_pos`], keyed by room) — so it
    /// removes ONLY our objective. The SIBLING of [`Self::withdraw_breach_objective`]. No-op if we have no
    /// tracked controller for this room (incl. after a VM reset — the orphaned objective then TTL-expires).
    /// Clears the tracker entry. Called when the controller goes neutral, the room re-arms / stands down /
    /// goes stale / aborts, and on completion — every path where the mission stops pursuing the de-claim.
    fn withdraw_declaim_objective(&self, system_data: &mut MissionExecutionSystemData, room_name: RoomName) {
        let Some(controller_pos) = system_data.salvage_breach_tracker.last_declaim_pos.remove(&room_name) else {
            return;
        };
        let stale: Vec<_> = system_data
            .combat_objective_queue
            .objectives
            .iter()
            .filter(|o| {
                o.owner == ObjectiveOwner::Attack
                    && o.kind == ObjectiveKind::Declaim {
                        room: room_name,
                        controller: controller_pos,
                    }
            })
            .map(|o| o.id)
            .collect();
        for id in stale {
            system_data.combat_objective_queue.withdraw(id);
        }
    }

    /// Emit (or refresh) the v1 `Declaim{room, controller}` objective (ADR 0027 v1.1 P2): the salvage
    /// DECLAIM producer. Owner=Attack, LOW priority (opportunistic). Sized by the always-field `DeclaimAttack`
    /// doctrine (which fields a CLAIM `SquadRole::Declaimer` squad). Mirrors [`Self::request_breach_objective`]
    /// and `SourceKeeperFarmMission`'s `Farm{SK}` producer: build the bot-agnostic engagement context, let
    /// `decide_doctrine` pick `DeclaimAttack` + `plan_engagement` size the CLAIM squad, and request the
    /// objective. The `SquadManager` then fields, travels, and `attackController`s the controller across the
    /// 1000-tick cadence (the `declaiming` lease-hold keeps it committed until the controller is neutral).
    /// Only called once the corridor is open (`ControllerAccess::ReachableNow`) — so the CLAIM body never dies
    /// against a walled-in controller. The EV/admit decision stays in `SalvageOperation` (the mission only
    /// reaches here for a strategic, hostile-owned, sourced derelict room).
    fn request_declaim_objective(&self, system_data: &mut MissionExecutionSystemData, room_name: RoomName, controller_pos: Position) {
        use screeps_combat_decision::composition::CompositionParams;
        use screeps_combat_decision::doctrine;
        use screeps_combat_decision::force_sizing::DefenseProfile;

        let member_energy = self
            .home_room_datas
            .iter()
            .filter_map(|&e| system_data.room_data.get(e))
            .filter_map(|rd| game::rooms().get(rd.name))
            .map(|r| r.energy_capacity_available())
            .max()
            .unwrap_or(0);

        // Project into the doctrine registry: a `Declaim` objective → the always-field `DeclaimAttack`
        // doctrine, which sizes a CLAIM declaimer squad directly (a derelict controller is undefended by
        // construction — no scouted enemy/towers; the mission aborts on re-arm). No EV gate here (the gate is
        // upstream in SalvageOperation).
        let ctx = doctrine::EngagementContext {
            objective: doctrine::DoctrineObjective::Declaim,
            coordination: doctrine::EnemyCoordination::Individual,
            defense: DefenseProfile::default(),
            enemy_force: None,
            importance: 0.0,
            member_energy,
            target_value: 1_000_000.0,
            onsite_window: CREEP_LIFE_TIME,
            params: CompositionParams { member_energy, ..Default::default() },
        };
        let doctrines = doctrine::default_doctrines();
        let Some(comp) = doctrine::decide_doctrine(&ctx, &doctrines).and_then(|d| doctrine::plan_engagement(d, &ctx, None).composition) else {
            // No home affords even one declaimer at this energy → don't emit (the manager could not field it).
            return;
        };

        let kind = ObjectiveKind::Declaim {
            room: room_name,
            controller: controller_pos,
        };

        // If the tracked controller pos drifted (different controller tile — shouldn't happen, but be
        // defensive), withdraw the prior objective first so the queue never accretes stale Declaims.
        if system_data.salvage_breach_tracker.last_declaim_pos.get(&room_name) != Some(&controller_pos) {
            self.withdraw_declaim_objective(system_data, room_name);
        }
        system_data.salvage_breach_tracker.last_declaim_pos.insert(room_name, controller_pos);

        let request = ObjectiveRequest::new(kind, OBJECTIVE_PRIORITY_LOW, ForceRequirement::single(comp)).owner(ObjectiveOwner::Attack);
        system_data.combat_objective_queue.request(request, game::time());
    }

    /// Withdraw the v1 breach `Dismantle` objective THIS mission emitted (ADR 0027
    /// v1.1 P1). Scoped to `Dismantle { room == target AND pos == tracked-pos }` — the
    /// exact `(room, breach-blocker)` we last requested (from the ephemeral
    /// [`SalvageBreachTracker`], keyed by room) — so it removes ONLY our objective and
    /// never clobbers `war.rs`'s same-room Attack-owned InvaderCore `Dismantle` (which
    /// targets the CORE tile, a different pos). Matching by pos (not room alone) is the
    /// FIX-2 clobber guard; the tracked pos follows the producer as the outermost seal
    /// falls (re-stamped on each emit, withdrawing the prior pos on drift). No-op if we
    /// have no tracked pos for this room (incl. after a VM reset — the orphaned objective
    /// then TTL-expires). Clears the tracker entry. Called when the corridor opens, the
    /// room re-arms / stands down / goes stale / aborts, before re-emitting at a changed
    /// pos, and on completion — every path where the mission stops pursuing the breach.
    fn withdraw_breach_objective(&self, system_data: &mut MissionExecutionSystemData, room_name: RoomName) {
        let Some(breach_pos) = system_data.salvage_breach_tracker.last_breach_pos.remove(&room_name) else {
            return;
        };
        let stale: Vec<_> = system_data
            .combat_objective_queue
            .objectives
            .iter()
            .filter(|o| o.owner == ObjectiveOwner::Attack && o.kind == ObjectiveKind::Dismantle { room: room_name, pos: breach_pos })
            .map(|o| o.id)
            .collect();
        for id in stale {
            system_data.combat_objective_queue.withdraw(id);
        }
    }

    /// Emit (or refresh) the v1 breach `Dismantle{room, breach-blocker}` objective
    /// (ADR 0027 v1.1 P1): the salvage breach PRODUCER. Owner=Attack, LOW
    /// priority (opportunistic, surplus-only), sized by the dormant `SiegeBreach`
    /// doctrine — this is its first live producer. Mirrors `war.rs`'s InvaderCore
    /// `Dismantle` emit + `SourceKeeperFarmMission`'s `Farm{SK}` producer: build
    /// the bot-agnostic engagement context (the corridor hits as the structure
    /// `objective_hits`, member energy from the best home), let `decide_doctrine`
    /// pick `SiegeBreach` + `plan_engagement` size the WORK squad, and request the
    /// objective. The `SquadManager` then fields, sizes, travels, and razes the
    /// blocker. If the pos drifted, the prior objective is withdrawn first so the
    /// queue holds exactly one breach objective per room.
    fn request_breach_objective(
        &self,
        system_data: &mut MissionExecutionSystemData,
        room_name: RoomName,
        breach_pos: Position,
        corridor_hits: u32,
    ) {
        use screeps_combat_decision::composition::CompositionParams;
        use screeps_combat_decision::doctrine;
        use screeps_combat_decision::force_sizing::DefenseProfile;

        // Best in-range home's spawn energy sizes each squad member (the same
        // energy the manager's spawn path sizes a `Sized` body against).
        let member_energy = self
            .home_room_datas
            .iter()
            .filter_map(|&e| system_data.room_data.get(e))
            .filter_map(|rd| game::rooms().get(rd.name))
            .map(|r| r.energy_capacity_available())
            .max()
            .unwrap_or(0);

        // Project into the doctrine registry's bot-agnostic context. A breach is
        // a dismantle-able structure ring → `DismantleStructure` selects the
        // `SiegeBreach` doctrine, which sizes the WORK force from `defense`
        // (the corridor's total hits as `objective_hits`). No scouted enemy creep
        // force is folded in (a derelict room is quiet by construction — the
        // mission already aborts on re-arm).
        let ctx = doctrine::EngagementContext {
            objective: doctrine::DoctrineObjective::DismantleStructure,
            coordination: doctrine::EnemyCoordination::Individual,
            defense: DefenseProfile { objective_hits: corridor_hits, ..Default::default() },
            enemy_force: None,
            importance: 0.0,
            member_energy,
            // Opportunistic surplus chew — a large target_value so a feasible
            // breach clears the EV commit threshold (the wall-energy ROI gate is
            // upstream: the mission only calls this on `breach_surplus`).
            target_value: 1_000_000.0,
            onsite_window: CREEP_LIFE_TIME,
            params: CompositionParams { member_energy, ..Default::default() },
        };
        let doctrines = doctrine::default_doctrines();
        let Some(comp) = doctrine::decide_doctrine(&ctx, &doctrines).and_then(|d| doctrine::plan_engagement(d, &ctx, None).composition) else {
            // No home affords the sized force at this energy → don't emit (the
            // manager could not field it anyway).
            return;
        };

        let kind = ObjectiveKind::Dismantle {
            room: room_name,
            pos: breach_pos,
        };

        // The pos drifts as the outermost seal falls. If the tracker holds a PRIOR
        // breach pos for this room that differs from this one, withdraw it first
        // (pos-scoped to OUR objective, never war's core) so the queue never accretes
        // stale Dismantle objectives. (A same-pos refresh leaves the live objective in
        // place — `request` upserts by kind below.)
        if system_data.salvage_breach_tracker.last_breach_pos.get(&room_name) != Some(&breach_pos) {
            self.withdraw_breach_objective(system_data, room_name);
        }
        // Track the pos we are emitting so a later withdraw (drift / abort / re-arm /
        // stale / corridor-open / completion) removes exactly this objective.
        system_data.salvage_breach_tracker.last_breach_pos.insert(room_name, breach_pos);

        let request = ObjectiveRequest::new(kind, OBJECTIVE_PRIORITY_LOW, ForceRequirement::single(comp)).owner(ObjectiveOwner::Attack);
        system_data.combat_objective_queue.request(request, game::time());
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for SalvageMission {
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

    fn remove_creep(&mut self, entity: Entity) {
        self.raiders.retain(|e| *e != entity);
        self.dismantlers.retain(|e| *e != entity);
    }

    fn get_creeps(&self) -> Vec<Entity> {
        self.raiders.iter().chain(self.dismantlers.iter()).copied().collect()
    }

    fn describe_state(&self, _system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> String {
        // ADR 0027 v1.1 P2: declaimers are no longer mission-owned creeps — the de-claim is a v1 `Declaim`
        // objective the `SquadManager` fields. Only raiders + teardown dismantlers remain mission-owned.
        format!("Salvage - Raiders: {} Dismantlers: {}", self.raiders.len(), self.dismantlers.len())
    }

    fn summarize(&self) -> crate::visualization::SummaryContent {
        crate::visualization::SummaryContent::Text(format!(
            "Salvage - Raiders: {} Dismantlers: {}",
            self.raiders.len(),
            self.dismantlers.len()
        ))
    }

    fn pre_run_mission(&mut self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<(), String> {
        self.home_room_datas
            .retain(|entity| system_data.room_data.get(*entity).map(is_valid_home_room).unwrap_or(false));

        if self.home_room_datas.is_empty() {
            return Err("No home rooms for salvage mission".to_owned());
        }

        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;
        let room_data_entity = self.room_data;

        system_data.transfer_queue.register_generator(
            room_data.name,
            TransferTypeFlags::HAUL,
            Box::new(move |system, transfer, _room_name| {
                let room_data = system.get_room_data(room_data_entity).ok_or("Expected room")?;

                Self::request_transfer_for_loot(transfer, room_data)?;

                Ok(())
            }),
        );

        Ok(())
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<MissionResult, String> {
        let features = system_data.features;
        let derelict_features = features.derelict;

        if !derelict_features.on {
            return Err("Derelict-room handling disabled - aborting salvage".to_string());
        }

        // Phase 1: gates and work survey against an immutable room borrow.
        //
        // ADR 0027 v1.1 P1 (FIX 1): the abort / re-arm / stale / claimed early-returns
        // below must explicitly TEAR DOWN the v1 breach `Dismantle` objective — relying
        // on its 200-tick TTL is unsafe once a squad has CLAIMED it (the manager's
        // deadline lease keeps a claimed objective alive past TTL, so a squad could keep
        // dismantling a room that just RE-ARMED). The borrow held here is immutable on
        // `system_data.room_data`, which conflicts with the `&mut self`/`&mut system_data`
        // the withdraw needs, so the standdown decision is DEFERRED out of the borrow:
        // we capture it as `standdown` and handle withdraw+return after the block closes.
        let mut standdown: Option<Result<MissionResult, String>> = None;
        let survey_tuple = {
            let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;
            let dynamic_visibility_data = room_data.get_dynamic_visibility_data().ok_or("Expected dynamic visibility data")?;

            if dynamic_visibility_data.updated_within(1000) {
                if dynamic_visibility_data.owner().mine() || dynamic_visibility_data.owner().friendly() {
                    // Claimed - colony/outpost machinery owns the room now.
                    standdown = Some(Ok(MissionResult::Success));
                } else if dynamic_visibility_data.militarily_active() {
                    standdown = Some(Err("Salvage target re-armed (spawn/tower/combat creeps) - aborting".to_string()));
                } else if dynamic_visibility_data.owner().hostile() && !dynamic_visibility_data.derelict() {
                    // For hostile-owned targets, any threat-capable creep sighting
                    // (haulers refilling towers, claimers, healers) breaks the
                    // derelict classification even though it is not "militarised".
                    standdown = Some(Err("Salvage target no longer derelict (hostile activity sighted) - aborting".to_string()));
                }
            } else if !dynamic_visibility_data.updated_within(derelict_features.action_max_age) {
                standdown = Some(Err("Salvage intel too stale - aborting".to_string()));
            }

            if standdown.is_none() && dynamic_visibility_data.safe_mode_active() {
                // Safe mode blocks withdraw/dismantle for us; abort and let the
                // operation re-admit once it has expired.
                standdown = Some(Err("Salvage target under safe mode - aborting".to_string()));
            }

            if standdown.is_some() {
                // Drop the room borrow and tear the breach objective down below.
                None
            } else {

            // De-claim target (from PERSISTED intel — no live structures
            // needed): a still-hostile-owned derelict room WITH sources is
            // worth taking over, so neutralize its controller for the waiting
            // mining outpost. Skipped once the controller goes neutral (owner
            // no longer hostile) or for sourceless rooms (nothing to mine).
            let declaim_target = if derelict_features.declaim
                && dynamic_visibility_data.owner().hostile()
                && room_data
                    .get_static_visibility_data()
                    .map(|s| !s.sources().is_empty())
                    .unwrap_or(false)
            {
                room_data.get_static_visibility_data().and_then(|s| s.controller().copied())
            } else {
                None
            };

            // Work survey needs live structure data; keep eyes on the room.
            let survey = room_data.get_structures().map(|structures| {
                let sources = room_data
                    .get_static_visibility_data()
                    .map(|s| s.sources().as_slice())
                    .unwrap_or(&[]);

                let work = assess_salvage_work(
                    structures.all(),
                    sources,
                    derelict_features.max_structure_hits,
                    0,
                    dynamic_visibility_data.owner().hostile(),
                );

                // "Ready" = a target with an empty store exists right now;
                // store-full structures become ready as raiders drain them.
                let dismantle_ready = requires_dismantling(structures.all(), sources, derelict_features.max_structure_hits);

                // Can a de-claimer actually reach the controller? Gates CLAIM
                // spawning so we don't burn CLAIM bodies that die against a
                // walled-in controller before dismantlers breach the path.
                let declaim_access = declaim_target.map(|controller_id| {
                    position_access(
                        room_data.name,
                        structures.all(),
                        controller_id.pos(),
                        derelict_features.max_structure_hits,
                    )
                });

                // Breach wanted if ANY objective we must physically reach is
                // sealed at the horizon but breachable if we ignore it (=
                // over-horizon enemy walls/ramparts). Objectives: every SOURCE
                // (so miners can reach them — the cost matrix marks enemy
                // walls/ramparts impassable, so a walled source is unmineable),
                // plus the CONTROLLER when we mean to de-claim it.
                let mut breach_objectives: Vec<Position> = sources.iter().map(|s| s.pos()).collect();
                if let Some(controller_id) = declaim_target {
                    breach_objectives.push(controller_id.pos());
                }
                let breach_possible = objectives_need_breach(
                    room_data.name,
                    structures.all(),
                    &breach_objectives,
                    derelict_features.max_structure_hits,
                );

                // The single corridor blocker tile + total hits the v1
                // `Dismantle` objective targets (ADR 0027 v1.1 P1): the outermost
                // seal on the cheapest corridor from an objective out to a room
                // edge. `None` ⇒ no breach work (every objective already reaches
                // an edge). The producer below emits/withdraws on this.
                let breach_target = breach_target_tile(
                    room_data.name,
                    structures.all(),
                    &breach_objectives,
                    derelict_features.max_structure_hits,
                );

                (work, dismantle_ready, declaim_access, breach_possible, breach_target)
            });

            match survey {
                Some((work, dismantle_ready, declaim_access, breach_possible, breach_target)) => Some((
                    room_data.name,
                    work,
                    dismantle_ready,
                    declaim_target,
                    declaim_access,
                    breach_possible,
                    breach_target,
                )),
                None => {
                    system_data.visibility.request(VisibilityRequest::new(
                        room_data.name,
                        VISIBILITY_PRIORITY_MEDIUM,
                        VisibilityRequestFlags::ALL,
                    ));

                    return Ok(MissionResult::Running);
                }
            }
            } // end else (no standdown)
        };

        // FIX 1: on every standdown path (claimed / re-arm / not-derelict / stale /
        // safe mode) explicitly tear the breach AND declaim objectives down before
        // returning, so a squad that claimed one cannot keep working a re-armed room
        // under the manager's deadline lease (the TTL alone would not retire it).
        // Mirrors the completion withdraw at the end of the tick. (ADR 0027 v1.1 P1+P2.)
        if let Some(result) = standdown {
            if let Some(name) = system_data.room_data.get(self.room_data).map(|rd| rd.name) {
                self.withdraw_breach_objective(system_data, name);
                self.withdraw_declaim_objective(system_data, name);
            }
            return result;
        }
        // Safe: `None` standdown ⇒ `survey_tuple` is `Some` (the visibility-needed path
        // returned above).
        let (room_name, work, dismantle_ready, declaim_target, declaim_access, breach_possible, breach_target) =
            survey_tuple.expect("survey tuple present when not standing down");

        // Per-role desired rosters from observed work. Disabled feature flags
        // zero the role; live creeps finish their jobs and expire naturally.
        let desired_raiders = if features.raid && work.loot_total() > 0 {
            (work.loot_total().div_ceil(RAIDER_LOOT_PER_LIFETIME) as usize).clamp(1, MAX_RAIDERS)
        } else {
            0
        };

        // Breach: when an objective (source for mining, or the controller for
        // de-claim) is Sealed at the normal dismantle horizon but reachable if
        // we IGNORE the over-horizon walls (`breach_possible`), the seal is
        // walls we could chew to open a corridor. `breach_possible` already
        // aggregates all objectives — so this opens walled-off sources, not just
        // the controller. For a strategic takeover room we breach them — but
        // only on SURPLUS (excess home energy + an idle spawn), because the wall
        // energy is a net loss (per the EV analysis) and this must consume spare
        // capacity only.
        //
        // ADR 0027 v1.1 P1: the breach corridor is no longer chewed by a
        // mission-owned solo dismantler. The mission now EMITS a v1
        // `Dismantle{room, breach-blocker}` objective (owner=Attack, LOW) and the
        // unified `SquadManager` fields + force-SIZES the dismantler squad via the
        // dormant `SiegeBreach` doctrine — the producer pattern (like
        // `SourceKeeperFarmMission` emitting `Farm{SK}`). The mission keeps only
        // the within-horizon TEARDOWN dismantlers below.
        let breach_needed = derelict_features.breach_sealed && features.dismantle && breach_possible;

        let breach_surplus = breach_needed
            && self
                .home_room_datas
                .first()
                .and_then(|home| system_data.economy.rooms.get(home))
                .map(|econ| econ.stored_energy >= derelict_features.breach_min_home_energy && econ.free_spawns > 0)
                .unwrap_or(false);

        // The v1 breach objective is LIVE this tick when breach is wanted, on
        // surplus, and we have a concrete blocker tile to target. (`breach_target`
        // is `None` once the corridor is open — every objective reaches an edge —
        // which is exactly when we withdraw the objective below.)
        let breach_objective_live = breach_surplus && breach_target.is_some();

        // Dismantler roster: ONLY the within-horizon TEARDOWN (raze-for-salvage)
        // role now — the breach corridor is the v1 squad's job (above). Sized by
        // the ready dismantle work.
        let desired_dismantlers = if features.dismantle && dismantle_ready {
            if work.dismantle_hits > SECOND_DISMANTLER_HITS {
                2
            } else {
                1
            }
        } else {
            0
        };

        // ── ADR 0027 v1.1 P2: declaim producer gating ───────────────────────────
        // The de-claim is no longer a mission-owned `DeclaimJob` creep — it is a v1
        // `Declaim{room, controller}` objective the `SquadManager` fields as a CLAIM
        // declaimer squad. `declaim_target` is gated on declaim enabled + hostile-owned
        // + sources; `declaim_access` gates reachability:
        //   - ReachableNow → the corridor is OPEN (opened by the P1 breach `Dismantle`
        //     squad), so EMIT the `Declaim` objective and let the manager field +
        //     persist the declaimer across the 1000-tick cadence;
        //   - Breachable → the breach producer runs first (the P1 `Dismantle` squad
        //     opens the path); we do NOT emit declaim yet (the CLAIM body would die
        //     against the walled controller);
        //   - Sealed → likewise wait for the corridor.
        // `declaim_wanted` keeps the mission OPEN while a de-claim is still desired
        // (mirrors `breach_needed`), so it does not complete-and-cool while waiting for
        // the corridor / the squad.
        let declaim_objective_live = matches!(declaim_access, Some(ControllerAccess::ReachableNow));
        let declaim_wanted = declaim_target.is_some()
            && matches!(declaim_access, Some(ControllerAccess::ReachableNow) | Some(ControllerAccess::Breachable));

        if derelict_features.diagnostics {
            // Source reachability — answers "do leftover walls block mining".
            // Only when diagnosing (re-fetches terrain per source).
            let (reachable_sources, total_sources) = system_data
                .room_data
                .get(self.room_data)
                .map(|rd| {
                    let total = rd.get_static_visibility_data().map(|s| s.sources().len()).unwrap_or(0);
                    let reachable = rd
                        .get_structures()
                        .and_then(|structures| {
                            rd.get_static_visibility_data().map(|svd| {
                                svd.sources()
                                    .iter()
                                    .filter(|s| position_reachable_now(rd.name, structures.all(), s.pos()))
                                    .count()
                            })
                        })
                        .unwrap_or(0);
                    (reachable, total)
                })
                .unwrap_or((0, 0));

            info!(
                "[salvage-mission-diag] {} loot(e={},o={}) dismantle_hits={} dismantle_ready={} declaim_target={} declaim_access={:?} declaim_obj_live={} declaim_wanted={} breach_possible={} breach_needed={} breach_surplus={} breach_obj_live={} breach_target={:?} sources_reachable={}/{} -> desired raiders={} dismantlers={} alive r={} d={}",
                room_name,
                work.loot_energy,
                work.loot_other,
                work.dismantle_hits,
                dismantle_ready,
                declaim_target.is_some(),
                declaim_access,
                declaim_objective_live,
                declaim_wanted,
                breach_possible,
                breach_needed,
                breach_surplus,
                breach_objective_live,
                breach_target.map(|(p, h)| (p.x().u8(), p.y().u8(), h)),
                reachable_sources,
                total_sources,
                desired_raiders,
                desired_dismantlers,
                self.raiders.len(),
                self.dismantlers.len(),
            );
        }

        // ── ADR 0027 v1.1 P1: breach producer ────────────────────────────────
        // The breach corridor is opened by a v1 `Dismantle` squad the
        // `SquadManager` fields, NOT a mission-owned dismantler. Emit the
        // objective while breach is wanted on surplus + a blocker tile exists;
        // WITHDRAW it the moment the corridor is open (`breach_target` is None ⇒
        // every objective reaches an edge), surplus lapses, or the room
        // re-arms/stands-down (handled by the early returns above, which exit
        // before this point — leaving the objective to TTL-expire, plus the
        // explicit withdraw on standdown below).
        if breach_objective_live {
            if let Some((breach_pos, corridor_hits)) = breach_target {
                self.request_breach_objective(system_data, room_name, breach_pos, corridor_hits);
            }
        } else {
            // Corridor open (or surplus lapsed): drop the breach objective so the
            // manager retires the dismantler squad this tick.
            self.withdraw_breach_objective(system_data, room_name);
        }

        // ── ADR 0027 v1.1 P2: declaim producer ───────────────────────────────
        // Once the corridor is open (`ReachableNow`), EMIT the v1 `Declaim` objective
        // so the `SquadManager` fields the CLAIM declaimer + persists it across the
        // 1000-tick cadence. WITHDRAW it the moment the controller is no longer a
        // declaim target (it went neutral → `declaim_target` is None → not ReachableNow)
        // or the room re-arms / stands down (handled by the early returns above, plus
        // the explicit standdown withdraw below). The producer pattern (like the breach
        // `Dismantle` + SK `Farm`), NOT a mission-owned `DeclaimJob`.
        if declaim_objective_live {
            if let Some(controller_id) = declaim_target {
                self.request_declaim_objective(system_data, room_name, controller_id.pos());
            }
        } else {
            // Not reachable / controller neutral: drop the declaim objective so the
            // manager retires the declaimer squad (a neutral controller = de-claim done).
            self.withdraw_declaim_objective(system_data, room_name);
        }

        // Complete only when there is genuinely nothing to do. A breach-NEEDED
        // room holds the mission open (as the old breach `desired_dismantlers==1`
        // did): while breach is wanted it waits for surplus and keeps re-asserting
        // the v1 `Dismantle` objective, instead of completing-and-cooling in a
        // loop. (`breach_needed` ⊇ `breach_objective_live`.)
        if desired_raiders == 0 && desired_dismantlers == 0 && !declaim_wanted && !breach_needed {
            info!(
                "Salvage of room {} complete - no enabled work remains (loot={}, within-horizon dismantle hits={}, declaim_access={:?}, breach_possible={})",
                room_name,
                work.loot_total(),
                work.dismantle_hits,
                declaim_access,
                breach_possible
            );

            // Defensive: never leave a breach / declaim objective behind on completion.
            self.withdraw_breach_objective(system_data, room_name);
            self.withdraw_declaim_objective(system_data, room_name);
            return Ok(MissionResult::Success);
        }

        if !system_data.governor.can_execute_cpu(CpuBar::LowPriority) {
            return Ok(MissionResult::Running);
        }

        if self.raiders.len() < desired_raiders {
            self.spawn_raiders(system_data, mission_entity, room_name)?;
        }

        // Within-horizon TEARDOWN dismantlers only (raze-for-salvage). The breach
        // corridor is the v1 squad's job (above); the teardown migration is P3.
        if self.dismantlers.len() < desired_dismantlers {
            self.spawn_dismantlers(
                system_data,
                mission_entity,
                derelict_features.max_structure_hits,
                SPAWN_PRIORITY_LOW,
            )?;
        }

        // ADR 0027 v1.1 P2: no mission-owned declaimer spawn — the `Declaim` objective (emitted above) is
        // fielded by the `SquadManager`.

        Ok(MissionResult::Running)
    }
}
