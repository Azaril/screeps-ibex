use super::data::*;
use super::missionsystem::*;
use crate::jobs::claim::*;
use crate::jobs::data::*;
use crate::remoteobjectid::*;
use crate::room::data::*;
use crate::serialize::*;
use crate::spawnsystem::*;
use itertools::*;
use screeps::*;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;

/// Base respawn backoff (ticks) after the first lost claimer; doubles per death.
const CLAIMER_RESPAWN_BACKOFF_BASE: u32 = 300;
/// Cap on the respawn backoff.
const CLAIMER_RESPAWN_BACKOFF_MAX: u32 = 3000;

#[derive(ConvertSaveload)]
pub struct ClaimMission {
    owner: EntityOption<Entity>,
    room_data: Entity,
    home_room_datas: EntityVec<Entity>,
    claimers: EntityVec<Entity>,
    /// Claimers lost (killed / aged out) before claiming. Drives the respawn
    /// backoff and the abort-on-budget (ADR 0017): a claimer repeatedly killed
    /// en route means the target is contested — stop feeding the meat grinder.
    claimer_deaths: u32,
    /// Tick of the last claimer spawn request, for the exponential backoff.
    last_spawn_tick: Option<u32>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl ClaimMission {
    pub fn build<B>(builder: B, owner: Option<Entity>, room_data: Entity, home_room_datas: &[Entity]) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = ClaimMission::new(owner, room_data, home_room_datas);

        builder
            .with(MissionData::Claim(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>, room_data: Entity, home_room_datas: &[Entity]) -> ClaimMission {
        ClaimMission {
            owner: owner.into(),
            room_data,
            home_room_datas: home_room_datas.into(),
            claimers: EntityVec::new(),
            claimer_deaths: 0,
            last_spawn_tick: None,
        }
    }

    pub fn home_room_datas(&self) -> &EntityVec<Entity> {
        &self.home_room_datas
    }

    fn create_handle_claimer_spawn(mission_entity: Entity, controller_id: RemoteObjectId<StructureController>) -> SpawnQueueCallback {
        Box::new(move |spawn_system_data, name| {
            let name = name.to_string();

            spawn_system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::Claim(ClaimJob::new(controller_id));

                let creep_entity = crate::creep::spawning::build(world.create_entity(), &name).with(creep_job).build();

                if let Some(mut mission_data) = world
                    .write_storage::<MissionData>()
                    .get_mut(mission_entity)
                    .as_mission_type_mut::<ClaimMission>()
                {
                    mission_data.claimers.push(creep_entity);
                }
            });
        })
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for ClaimMission {
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
        let before = self.claimers.len();
        self.claimers.retain(|e| *e != entity);
        // A tracked claimer that disappeared before the room was claimed (the
        // mission would have ended with Success on a claim) was lost — count it
        // toward the abort budget.
        if self.claimers.len() < before {
            self.claimer_deaths = self.claimer_deaths.saturating_add(1);
        }
    }

    fn repair_entity_refs(&mut self, is_valid: &dyn Fn(Entity) -> bool) {
        // Without this, a dangling claimer entity (one removed outside the
        // creep-death path) would keep `claimers` non-empty forever, silently
        // suppressing claimer respawns. Drop any reference that is no longer a
        // live, serializable entity.
        self.claimers.retain(|e| is_valid(*e));
    }

    fn describe_state(&self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> String {
        let home_room_names = self
            .home_room_datas
            .iter()
            .filter_map(|e| system_data.room_data.get(*e))
            .map(|d| d.name.to_string())
            .join("/");

        format!("Claim - Claimers: {} - Home rooms: {}", self.claimers.len(), home_room_names)
    }

    fn summarize(&self) -> crate::visualization::SummaryContent {
        crate::visualization::SummaryContent::Text(format!(
            "Claim - Claimers: {} - Homes: {}",
            self.claimers.len(),
            self.home_room_datas.len()
        ))
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<MissionResult, String> {
        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;
        let dynamic_visibility_data = room_data.get_dynamic_visibility_data().ok_or("Expected dynamic visibility data")?;

        if dynamic_visibility_data.updated_within(1000) {
            match dynamic_visibility_data.owner() {
                RoomDisposition::Mine => {
                    return Ok(MissionResult::Success);
                }
                RoomDisposition::Friendly(_) | RoomDisposition::Hostile(_) => {
                    return Err("Room already owned".to_string());
                }
                RoomDisposition::Neutral => {}
            }

            // A FOREIGN reservation DOES block claimController (the engine
            // returns ERR_INVALID_TARGET, ADR 0017) — the previous
            // "proceeding anyway" was wrong. Abort so the room is re-evaluated;
            // clearing a foreign reservation is an offensive (attackController)
            // decision, not part of the claim path.
            match dynamic_visibility_data.reservation() {
                RoomDisposition::Mine | RoomDisposition::Neutral => {}
                RoomDisposition::Friendly(ref name) | RoomDisposition::Hostile(ref name) => {
                    return Err(format!("Claim target reserved by {} — claimController would fail", name));
                }
            }
        }

        let static_visibility_data = room_data.get_static_visibility_data().ok_or("Expected static visibility data")?;
        let controller = static_visibility_data.controller().ok_or("Expected target controller")?;

        // Abort if too many claimers were lost reaching this target — it is a
        // losing battle. Tag the room in the avoid-cooldown map so the claim
        // operation does not immediately re-select it (ADR 0017).
        if system_data.features.claim.safety_gate && self.claimer_deaths >= system_data.features.claim.max_claimer_deaths {
            let until = game::time().saturating_add(system_data.features.claim.avoid_cooldown_ticks);
            system_data.expansion_avoidance.avoid(room_data.name, until);
            return Err(format!(
                "Claim aborted: {} claimer(s) lost reaching {} — avoiding for {} ticks",
                self.claimer_deaths, room_data.name, system_data.features.claim.avoid_cooldown_ticks
            ));
        }

        // Exponential respawn backoff after losses, so we don't re-feed a
        // claimer every tick into a contested approach.
        let now = game::time();
        let backoff = if self.claimer_deaths == 0 {
            0
        } else {
            CLAIMER_RESPAWN_BACKOFF_BASE
                .saturating_mul(1u32 << (self.claimer_deaths - 1).min(8))
                .min(CLAIMER_RESPAWN_BACKOFF_MAX)
        };
        let ready_to_spawn = self.last_spawn_tick.map(|t| now >= t.saturating_add(backoff)).unwrap_or(true);

        let token = system_data.spawn_queue.token();

        let mut requested = false;

        for home_room_data_entity in self.home_room_datas.iter() {
            let home_room_data = system_data.room_data.get(*home_room_data_entity).ok_or("Expected home room data")?;
            let home_room = game::rooms().get(home_room_data.name).ok_or("Expected home room")?;

            if self.claimers.is_empty() && ready_to_spawn {
                let body_definition = crate::creep::SpawnBodyDefinition {
                    maximum_energy: home_room.energy_capacity_available(),
                    minimum_repeat: None,
                    maximum_repeat: None,
                    pre_body: &[Part::Claim, Part::Move],
                    repeat_body: &[],
                    post_body: &[],
                };

                if let Ok(body) = crate::creep::spawning::create_body(&body_definition) {
                    let spawn_request = SpawnRequest::new(
                        "Claimer".to_string(),
                        &body,
                        SPAWN_PRIORITY_HIGH,
                        Some(token),
                        Self::create_handle_claimer_spawn(mission_entity, *controller),
                    );

                    system_data.spawn_queue.request(*home_room_data_entity, spawn_request);
                    requested = true;
                }
            }
        }

        if requested {
            self.last_spawn_tick = Some(now);
        }

        // If we TRIED to spawn (not just holding for backoff) but no home could
        // afford the body, the mission is a silent zombie (a [Claim, Move]
        // claimer needs ~650 energy capacity ≈ RCL 3). Fail loudly so it is
        // cleaned up; the claim operation only re-creates it once an affordable
        // home is in reach.
        if self.claimers.is_empty() && !requested && ready_to_spawn {
            return Err(format!(
                "No home room can afford a claimer (need {} energy capacity) for target {}",
                Part::Claim.cost() + Part::Move.cost(),
                room_data.name
            ));
        }

        Ok(MissionResult::Running)
    }
}
