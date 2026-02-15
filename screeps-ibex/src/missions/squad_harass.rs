use super::data::*;
use super::missionsystem::*;
use crate::creep::*;
use crate::jobs::data::*;
use crate::jobs::ranged::*;
use crate::military::bodies;
use crate::serialize::*;
use crate::spawnsystem::*;
use log::*;
use screeps::*;
use screeps_machine::*;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;

/// Mission to harass enemy remote mining with cheap solo/duo units.
///
/// Sends a fast, cheap ranged attacker to an enemy remote mining room to
/// kill miners and disrupt economy. The creep is expendable -- if it dies,
/// the mission spawns a replacement. The mission completes when the parent
/// operation signals it or when the target room is no longer interesting.
///
/// Tracks energy spent on spawning and deaths vs estimated damage inflicted.
/// If the harassment is not cost-effective after several cycles, the mission
/// abandons the target room.
#[derive(Clone, ConvertSaveload)]
pub struct SquadHarassMissionContext {
    target_room: RoomName,
    /// Rooms that can spawn harassers.
    home_rooms: EntityVec<Entity>,
    /// Tracked harasser creep entities.
    attackers: EntityVec<Entity>,
    /// Total energy spent on spawning harassers for this target.
    total_energy_spent: u32,
    /// Number of harassers that have died (each death = wasted spawn cost).
    total_deaths: u32,
    /// Number of successful kills (enemy creeps killed or structures destroyed).
    /// Updated when the harasser reports kills or when we observe enemy losses.
    total_kills: u32,
    /// Tick when the mission started, for calculating ROI over time.
    mission_start_tick: Option<u32>,
    /// Number of consecutive spawn cycles where the harasser died without getting kills.
    consecutive_failures: u32,
}

machine!(
    #[derive(Clone, ConvertSaveload)]
    enum SquadHarassState {
        Spawning {
            phantom: std::marker::PhantomData<Entity>
        },
        Harassing {
            phantom: std::marker::PhantomData<Entity>
        },
        Complete {
            phantom: std::marker::PhantomData<Entity>
        },
    }

    impl {
        * => fn describe_state(&self, _system_data: &MissionExecutionSystemData, _mission_entity: Entity, _state_context: &SquadHarassMissionContext) -> String {
            format!("SquadHarass - {}", self.status_description())
        }

        _ => fn status_description(&self) -> String;

        * => fn visualize(&self, _system_data: &MissionExecutionSystemData, _mission_entity: Entity, _state_context: &SquadHarassMissionContext) {}

        * => fn gather_data(&self, _system_data: &MissionExecutionSystemData, _mission_entity: Entity) {}

        _ => fn tick(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity, state_context: &mut SquadHarassMissionContext) -> Result<Option<SquadHarassState>, String>;
    }
);

impl Spawning {
    fn status_description(&self) -> String {
        "Spawning".to_string()
    }

    fn tick(
        &mut self,
        system_data: &mut MissionExecutionSystemData,
        mission_entity: Entity,
        state_context: &mut SquadHarassMissionContext,
    ) -> Result<Option<SquadHarassState>, String> {
        // Initialize mission start tick.
        if state_context.mission_start_tick.is_none() {
            state_context.mission_start_tick = Some(game::time());
        }

        // Check if harassment is cost-effective before spawning another.
        if should_abandon_harassment(state_context) {
            info!(
                "[SquadHarass] Abandoning harassment of {} - not cost-effective (spent={}, deaths={}, kills={}, failures={})",
                state_context.target_room,
                state_context.total_energy_spent,
                state_context.total_deaths,
                state_context.total_kills,
                state_context.consecutive_failures,
            );
            return Ok(Some(SquadHarassState::complete(std::marker::PhantomData)));
        }

        // If we already have an attacker, go to harassing.
        if !state_context.attackers.is_empty() {
            return Ok(Some(SquadHarassState::harassing(std::marker::PhantomData)));
        }

        let target_room = state_context.target_room;

        // Try to spawn from each home room.
        for home_room_entity in state_context.home_rooms.iter() {
            let _home_room_data = system_data.room_data.get(*home_room_entity).ok_or("Expected home room data")?;

            let body_def = bodies::harasser_body();

            if let Ok(body) = spawning::create_body(&body_def) {
                // Track energy cost of this spawn.
                let spawn_cost: u32 = body.iter().map(|p| p.cost()).sum();
                state_context.total_energy_spent += spawn_cost;

                let token = system_data.spawn_queue.token();

                let spawn_request = SpawnRequest::new(
                    format!("Harasser - {}", target_room),
                    &body,
                    SPAWN_PRIORITY_LOW,
                    Some(token),
                    Self::create_harasser_callback(mission_entity, target_room),
                );

                system_data.spawn_queue.request(*home_room_entity, spawn_request);
                break; // Only request from one room.
            }
        }

        Ok(None)
    }
}

impl Spawning {
    fn create_harasser_callback(mission_entity: Entity, target_room: RoomName) -> SpawnQueueCallback {
        Box::new(move |system_data, name| {
            let name = name.to_string();

            system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::RangedAttack(RangedAttackJob::new(target_room));
                let creep_entity = spawning::build(world.create_entity(), &name).with(creep_job).build();

                if let Some(mission_data) = world.write_storage::<MissionData>().get(mission_entity) {
                    if let Ok(mut mission) = <std::cell::RefMut<'_, SquadHarassMission>>::try_from(mission_data) {
                        mission.context.attackers.push(creep_entity);
                    }
                }
            });
        })
    }
}

impl Harassing {
    fn status_description(&self) -> String {
        "Harassing".to_string()
    }

    fn tick(
        &mut self,
        system_data: &mut MissionExecutionSystemData,
        _mission_entity: Entity,
        state_context: &mut SquadHarassMissionContext,
    ) -> Result<Option<SquadHarassState>, String> {
        // Count dead attackers (game object gone but entity still alive until cleanup).
        let deaths_this_tick = state_context
            .attackers
            .iter()
            .filter(|e| {
                system_data
                    .creep_owner
                    .get(**e)
                    .map(|co| co.owner.resolve().is_none())
                    .unwrap_or(true)
            })
            .count();

        // Track deaths.
        if deaths_this_tick > 0 {
            state_context.total_deaths += deaths_this_tick as u32;
        }

        // Check for kills: observe the target room for enemy losses.
        // We approximate this by checking if the target room has fewer enemy creeps
        // than expected for a remote mining operation. If the room is visible and
        // has no enemy creeps, count it as disruption success.
        if let Some(target_room_entity) = system_data.room_data.join().find(|rd| rd.name == state_context.target_room) {
            if let Some(dynamic_vis) = target_room_entity.get_dynamic_visibility_data() {
                if !dynamic_vis.hostile_creeps() && dynamic_vis.visible() {
                    // Room is clear -- our harassment is working.
                    // Count each tick the room is clear as partial success.
                    // We increment kills periodically (every 100 ticks of clear room).
                    if game::time().is_multiple_of(100) {
                        state_context.total_kills += 1;
                    }
                }
            }
        }

        // If all attackers are dead, go back to spawning.
        if state_context.attackers.is_empty() {
            // Check if this death was without any kills since last spawn.
            // If we had kills, reset consecutive failures.
            if state_context.total_kills > 0 {
                state_context.consecutive_failures = 0;
            } else {
                state_context.consecutive_failures += 1;
            }

            info!(
                "[SquadHarass] Harasser died targeting {} (deaths={}, kills={}, failures={}). Respawning.",
                state_context.target_room, state_context.total_deaths, state_context.total_kills, state_context.consecutive_failures,
            );
            return Ok(Some(SquadHarassState::spawning(std::marker::PhantomData)));
        }

        Ok(None)
    }
}

impl Complete {
    fn status_description(&self) -> String {
        "Complete".to_string()
    }

    fn tick(
        &mut self,
        _system_data: &mut MissionExecutionSystemData,
        _mission_entity: Entity,
        _state_context: &mut SquadHarassMissionContext,
    ) -> Result<Option<SquadHarassState>, String> {
        Ok(None)
    }
}

#[derive(ConvertSaveload)]
pub struct SquadHarassMission {
    owner: EntityOption<Entity>,
    room_data: Entity,
    context: SquadHarassMissionContext,
    state: SquadHarassState,
}

/// Determine if harassment of this target should be abandoned.
///
/// Criteria for abandonment:
/// - 3+ consecutive deaths without any kills (enemy is too strong)
/// - Energy spent exceeds a threshold with poor kill ratio
/// - Mission has been running for a long time with no results
fn should_abandon_harassment(ctx: &SquadHarassMissionContext) -> bool {
    // Abandon after 3 consecutive failures (deaths without kills).
    if ctx.consecutive_failures >= 3 {
        return true;
    }

    // Abandon if we've spent a lot of energy with very poor results.
    // A harasser body costs ~500-800 energy. If we've spent 5000+ energy
    // and have fewer kills than deaths, it's not worth it.
    if ctx.total_energy_spent > 5000 && ctx.total_kills < ctx.total_deaths {
        return true;
    }

    // Abandon if the mission has been running for 5000+ ticks with no kills at all.
    if let Some(start) = ctx.mission_start_tick {
        let elapsed = game::time().saturating_sub(start);
        if elapsed > 5000 && ctx.total_kills == 0 && ctx.total_deaths >= 2 {
            return true;
        }
    }

    false
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl SquadHarassMission {
    pub fn build<B>(builder: B, owner: Option<Entity>, room_data: Entity, target_room: RoomName, home_rooms: &[Entity]) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let context = SquadHarassMissionContext {
            target_room,
            home_rooms: home_rooms.into(),
            attackers: EntityVec::new(),
            total_energy_spent: 0,
            total_deaths: 0,
            total_kills: 0,
            mission_start_tick: None,
            consecutive_failures: 0,
        };

        let mission = SquadHarassMission {
            owner: owner.into(),
            room_data,
            context,
            state: SquadHarassState::spawning(std::marker::PhantomData),
        };

        builder
            .with(MissionData::SquadHarass(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for SquadHarassMission {
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

    fn remove_creep(&mut self, entity: Entity) {
        self.context.attackers.retain(|e| *e != entity);
    }

    fn describe_state(&self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> String {
        self.state.describe_state(system_data, mission_entity, &self.context)
    }

    fn summarize(&self) -> crate::visualization::SummaryContent {
        crate::visualization::SummaryContent::Text(format!(
            "Harass {} ({} alive, {}E spent, {} kills, {} deaths)",
            self.context.target_room,
            self.context.attackers.len(),
            self.context.total_energy_spent,
            self.context.total_kills,
            self.context.total_deaths,
        ))
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<MissionResult, String> {
        if let Some(new_state) = self.state.tick(system_data, mission_entity, &mut self.context)? {
            self.state = new_state;
        }

        if matches!(self.state, SquadHarassState::Complete(_)) {
            return Ok(MissionResult::Success);
        }

        Ok(MissionResult::Running)
    }
}
