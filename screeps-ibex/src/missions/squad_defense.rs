use super::data::*;
use super::missionsystem::*;
use crate::creep::*;
use crate::jobs::data::*;
use crate::jobs::heal::*;
use crate::jobs::ranged::*;
use crate::military::bodies;
use crate::serialize::*;
use crate::spawnsystem::*;
use screeps::*;
use screeps_machine::*;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;

/// Desired squad composition for this defense mission.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DefenseSquadSize {
    /// Single ranged+heal defender.
    #[default]
    Solo,
    /// Attacker + healer pair.
    Duo,
    /// Four creeps in 2x2 formation (for fighting enemy quads/siege).
    Quad,
}

#[derive(Clone, ConvertSaveload)]
pub struct SquadDefenseMissionContext {
    defend_room_data: Entity,
    home_room_datas: EntityVec<Entity>,
    /// The squad entity that holds the SquadContext component.
    squad_entity: EntityOption<Entity>,
    /// Tracked defender creep entities.
    defenders: EntityVec<Entity>,
    /// Tracked healer creep entities (for duo/quad squads).
    healers: EntityVec<Entity>,
    /// Desired squad size (Solo, Duo, or Quad).
    squad_size: DefenseSquadSize,
    /// Tick when spawning completed and we started waiting for rally.
    rally_start_tick: Option<u32>,
}

machine!(
    #[derive(Clone, ConvertSaveload)]
    enum SquadDefenseState {
        Spawning {
            phantom: std::marker::PhantomData<Entity>
        },
        Rallying {
            phantom: std::marker::PhantomData<Entity>
        },
        Defending {
            phantom: std::marker::PhantomData<Entity>
        },
        Cleanup {
            phantom: std::marker::PhantomData<Entity>
        }
    }

    impl {
        * => fn describe_state(&self, _system_data: &MissionExecutionSystemData, _mission_entity: Entity, _state_context: &SquadDefenseMissionContext) -> String {
            format!("SquadDefense - {}", self.status_description())
        }

        * => fn status_description(&self) -> String {
            std::any::type_name::<Self>().to_string()
        }

        * => fn visualize(&self, _system_data: &MissionExecutionSystemData, _mission_entity: Entity, _state_context: &SquadDefenseMissionContext) {}

        * => fn gather_data(&self, _system_data: &MissionExecutionSystemData, _mission_entity: Entity) {}

        _ => fn tick(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity, state_context: &mut SquadDefenseMissionContext) -> Result<Option<SquadDefenseState>, String>;
    }
);

impl Spawning {
    fn tick(
        &mut self,
        system_data: &mut MissionExecutionSystemData,
        mission_entity: Entity,
        state_context: &mut SquadDefenseMissionContext,
    ) -> Result<Option<SquadDefenseState>, String> {
        // Clean up dead defenders and healers.
        state_context
            .defenders
            .retain(|entity| system_data.entities.is_alive(*entity) && system_data.job_data.get(*entity).is_some());
        state_context
            .healers
            .retain(|entity| system_data.entities.is_alive(*entity) && system_data.job_data.get(*entity).is_some());

        // Check if we have enough creeps to start defending.
        let ready = match state_context.squad_size {
            DefenseSquadSize::Solo => !state_context.defenders.is_empty(),
            DefenseSquadSize::Duo => !state_context.defenders.is_empty() && !state_context.healers.is_empty(),
            DefenseSquadSize::Quad => state_context.defenders.len() >= 2 && state_context.healers.len() >= 2,
        };

        if ready {
            // Solo defenders go straight to defending; multi-member squads rally first.
            return match state_context.squad_size {
                DefenseSquadSize::Solo => Ok(Some(SquadDefenseState::defending(std::marker::PhantomData))),
                DefenseSquadSize::Duo | DefenseSquadSize::Quad => {
                    state_context.rally_start_tick = Some(game::time());
                    Ok(Some(SquadDefenseState::rallying(std::marker::PhantomData)))
                }
            };
        }

        let defend_room_entity = state_context.defend_room_data;
        let defend_room_name = system_data
            .room_data
            .get(defend_room_entity)
            .map(|rd| rd.name)
            .ok_or("Expected defend room data")?;

        // Try to spawn from each home room.
        for home_room_entity in state_context.home_room_datas.iter() {
            let home_room_data = system_data.room_data.get(*home_room_entity).ok_or("Expected home room data")?;

            if let Some(room) = game::rooms().get(home_room_data.name) {
                let energy_capacity = room.energy_capacity_available();

                // Determine how many attackers and healers we need.
                let (needed_attackers, needed_healers) = match state_context.squad_size {
                    DefenseSquadSize::Solo => (1, 0),
                    DefenseSquadSize::Duo => (1, 1),
                    DefenseSquadSize::Quad => (2, 2),
                };

                // Spawn attackers if needed.
                if state_context.defenders.len() < needed_attackers {
                    let body_def = match state_context.squad_size {
                        DefenseSquadSize::Solo => bodies::solo_defender_body(energy_capacity),
                        DefenseSquadSize::Duo => bodies::duo_ranged_attacker_body(energy_capacity),
                        DefenseSquadSize::Quad => bodies::quad_member_body(energy_capacity),
                    };

                    if let Ok(body) = spawning::create_body(&body_def) {
                        let token = system_data.spawn_queue.token();

                        let spawn_request = SpawnRequest::new(
                            format!("Defender - {}", defend_room_name),
                            &body,
                            SPAWN_PRIORITY_HIGH,
                            Some(token),
                            Self::create_attacker_callback(mission_entity, defend_room_name),
                        );

                        system_data.spawn_queue.request(*home_room_entity, spawn_request);
                    }
                }

                // Spawn healers if needed (duo and quad).
                if needed_healers > 0 && state_context.healers.len() < needed_healers {
                    let body_def = match state_context.squad_size {
                        DefenseSquadSize::Quad => bodies::quad_member_body(energy_capacity),
                        _ => bodies::duo_healer_body(energy_capacity),
                    };

                    if let Ok(body) = spawning::create_body(&body_def) {
                        let token = system_data.spawn_queue.token();

                        let spawn_request = SpawnRequest::new(
                            format!("DefHealer - {}", defend_room_name),
                            &body,
                            SPAWN_PRIORITY_HIGH,
                            Some(token),
                            Self::create_healer_callback(mission_entity, defend_room_name),
                        );

                        system_data.spawn_queue.request(*home_room_entity, spawn_request);
                    }
                }
            }
        }

        Ok(None)
    }
}

impl Spawning {
    fn create_attacker_callback(mission_entity: Entity, defend_room: RoomName) -> SpawnQueueCallback {
        Box::new(move |system_data, name| {
            let name = name.to_string();

            system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::RangedAttack(RangedAttackJob::new(defend_room));
                let creep_entity = spawning::build(world.create_entity(), &name).with(creep_job).build();

                if let Some(mission_data) = world.write_storage::<MissionData>().get(mission_entity) {
                    if let Ok(mut mission) = <std::cell::RefMut<'_, SquadDefenseMission>>::try_from(mission_data) {
                        mission.context.defenders.push(creep_entity);
                    }
                }
            });
        })
    }

    fn create_healer_callback(mission_entity: Entity, defend_room: RoomName) -> SpawnQueueCallback {
        Box::new(move |system_data, name| {
            let name = name.to_string();

            system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::Heal(HealJob::new(defend_room));
                let creep_entity = spawning::build(world.create_entity(), &name).with(creep_job).build();

                if let Some(mission_data) = world.write_storage::<MissionData>().get(mission_entity) {
                    if let Ok(mut mission) = <std::cell::RefMut<'_, SquadDefenseMission>>::try_from(mission_data) {
                        mission.context.healers.push(creep_entity);
                    }
                }
            });
        })
    }
}

impl Rallying {
    fn tick(
        &mut self,
        system_data: &mut MissionExecutionSystemData,
        _mission_entity: Entity,
        state_context: &mut SquadDefenseMissionContext,
    ) -> Result<Option<SquadDefenseState>, String> {
        // Clean up dead members.
        state_context
            .defenders
            .retain(|entity| system_data.entities.is_alive(*entity) && system_data.job_data.get(*entity).is_some());
        state_context
            .healers
            .retain(|entity| system_data.entities.is_alive(*entity) && system_data.job_data.get(*entity).is_some());

        // If all members died during rally, go back to spawning.
        if state_context.defenders.is_empty() && state_context.healers.is_empty() {
            return Ok(Some(SquadDefenseState::spawning(std::marker::PhantomData)));
        }

        let defend_room_name = system_data
            .room_data
            .get(state_context.defend_room_data)
            .map(|rd| rd.name)
            .ok_or("Expected defend room data")?;

        // Check if all members are in the defend room (or an adjacent room).
        let _all_members_ready = game::creeps().values().all(|_| true); // Placeholder: we can't easily check positions from mission data.

        // Use a timeout: for defense, we can't wait too long.
        // Rally timeout is shorter for defense (50 ticks) since the room is under attack.
        let rally_timeout = state_context
            .rally_start_tick
            .map(|start| game::time() - start > 50)
            .unwrap_or(true);

        // Check if the defend room is visible and if we have members there.
        let members_in_room = if let Some(room) = game::rooms().get(defend_room_name) {
            let my_creeps = room.find(find::MY_CREEPS, None);
            let total_expected = state_context.defenders.len() + state_context.healers.len();
            // Count how many of our squad members are in the room.
            // This is approximate -- we count all our creeps in the room.
            my_creeps.len() >= total_expected
        } else {
            false
        };

        if members_in_room || rally_timeout {
            state_context.rally_start_tick = None;
            return Ok(Some(SquadDefenseState::defending(std::marker::PhantomData)));
        }

        Ok(None)
    }
}

impl Defending {
    fn tick(
        &mut self,
        system_data: &mut MissionExecutionSystemData,
        _mission_entity: Entity,
        state_context: &mut SquadDefenseMissionContext,
    ) -> Result<Option<SquadDefenseState>, String> {
        // Clean up dead defenders and healers.
        state_context
            .defenders
            .retain(|entity| system_data.entities.is_alive(*entity) && system_data.job_data.get(*entity).is_some());
        state_context
            .healers
            .retain(|entity| system_data.entities.is_alive(*entity) && system_data.job_data.get(*entity).is_some());

        // Check if the room is still under threat.
        let defend_room_data = system_data
            .room_data
            .get(state_context.defend_room_data)
            .ok_or("Expected defend room data")?;

        let has_hostiles = defend_room_data
            .get_dynamic_visibility_data()
            .map(|d| d.hostile_creeps() || d.hostile_structures())
            .unwrap_or(false);

        let all_dead = state_context.defenders.is_empty() && state_context.healers.is_empty();

        if !has_hostiles && all_dead {
            return Ok(Some(SquadDefenseState::cleanup(std::marker::PhantomData)));
        }

        // If all combat creeps are dead and there are still hostiles, respawn.
        if all_dead && has_hostiles {
            return Ok(Some(SquadDefenseState::spawning(std::marker::PhantomData)));
        }

        // Escalation: upgrade squad size based on threat level.
        if has_hostiles && state_context.squad_size != DefenseSquadSize::Quad {
            let creep_data = defend_room_data.get_creeps();
            let (estimated_dps, estimated_heal, hostile_count, any_boosted) = creep_data
                .map(|creeps| {
                    let hostiles = creeps.hostile();
                    let mut dps: f32 = 0.0;
                    let mut heal: f32 = 0.0;
                    let mut boosted = false;
                    for c in hostiles {
                        for p in c.body().iter() {
                            if p.hits() == 0 {
                                continue;
                            }
                            if p.boost().is_some() {
                                boosted = true;
                            }
                            let boost_mult = if p.boost().is_some() { 4.0 } else { 1.0 };
                            match p.part() {
                                Part::Attack => dps += 30.0 * boost_mult,
                                Part::RangedAttack => dps += 10.0 * boost_mult,
                                Part::Heal => heal += 12.0 * boost_mult,
                                _ => {}
                            }
                        }
                    }
                    (dps, heal, hostiles.len(), boosted)
                })
                .unwrap_or((0.0, 0.0, 0, false));

            // Determine what squad size the current threat warrants.
            let warranted_size =
                if (any_boosted && estimated_dps > 200.0) || (estimated_heal > 100.0 && estimated_dps > 150.0) || hostile_count >= 4 {
                    // Enemy quad or heavy siege -- need our own quad.
                    DefenseSquadSize::Quad
                } else if estimated_dps > 100.0 || estimated_heal > 20.0 || hostile_count >= 2 {
                    DefenseSquadSize::Duo
                } else {
                    DefenseSquadSize::Solo
                };

            // Only escalate (never downgrade mid-fight).
            let should_escalate = matches!(
                (state_context.squad_size, warranted_size),
                (DefenseSquadSize::Solo, DefenseSquadSize::Duo | DefenseSquadSize::Quad) | (DefenseSquadSize::Duo, DefenseSquadSize::Quad)
            );

            if should_escalate {
                state_context.squad_size = warranted_size;
                // Go back to spawning to fill the new roster.
                let (needed_atk, needed_heal) = match warranted_size {
                    DefenseSquadSize::Solo => (1, 0),
                    DefenseSquadSize::Duo => (1, 1),
                    DefenseSquadSize::Quad => (2, 2),
                };
                if state_context.defenders.len() < needed_atk || state_context.healers.len() < needed_heal {
                    return Ok(Some(SquadDefenseState::spawning(std::marker::PhantomData)));
                }
            }
        }

        Ok(None)
    }
}

impl Cleanup {
    fn tick(
        &mut self,
        _system_data: &mut MissionExecutionSystemData,
        _mission_entity: Entity,
        _state_context: &mut SquadDefenseMissionContext,
    ) -> Result<Option<SquadDefenseState>, String> {
        Ok(None)
    }
}

#[derive(ConvertSaveload)]
pub struct SquadDefenseMission {
    owner: EntityOption<Entity>,
    context: SquadDefenseMissionContext,
    state: SquadDefenseState,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl SquadDefenseMission {
    pub fn build<B>(builder: B, owner: Option<Entity>, defend_room_data: Entity, home_room_datas: &[Entity]) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = SquadDefenseMission::new(owner, defend_room_data, home_room_datas, DefenseSquadSize::Solo);

        builder
            .with(MissionData::SquadDefense(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn build_duo<B>(builder: B, owner: Option<Entity>, defend_room_data: Entity, home_room_datas: &[Entity]) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = SquadDefenseMission::new(owner, defend_room_data, home_room_datas, DefenseSquadSize::Duo);

        builder
            .with(MissionData::SquadDefense(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn build_quad<B>(builder: B, owner: Option<Entity>, defend_room_data: Entity, home_room_datas: &[Entity]) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = SquadDefenseMission::new(owner, defend_room_data, home_room_datas, DefenseSquadSize::Quad);

        builder
            .with(MissionData::SquadDefense(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn new(
        owner: Option<Entity>,
        defend_room_data: Entity,
        home_room_datas: &[Entity],
        squad_size: DefenseSquadSize,
    ) -> SquadDefenseMission {
        SquadDefenseMission {
            owner: owner.into(),
            context: SquadDefenseMissionContext {
                defend_room_data,
                home_room_datas: home_room_datas.to_owned().into(),
                squad_entity: None.into(),
                defenders: EntityVec::new(),
                healers: EntityVec::new(),
                squad_size,
                rally_start_tick: None,
            },
            state: SquadDefenseState::spawning(std::marker::PhantomData),
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for SquadDefenseMission {
    fn get_owner(&self) -> &Option<Entity> {
        &self.owner
    }

    fn owner_complete(&mut self, owner: Entity) {
        assert!(Some(owner) == *self.owner);
        self.owner.take();
    }

    fn get_room(&self) -> Entity {
        self.context.defend_room_data
    }

    fn describe_state(&self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> String {
        self.state.describe_state(system_data, mission_entity, &self.context)
    }

    fn summarize(&self) -> crate::visualization::SummaryContent {
        let squad_desc = match self.context.squad_size {
            DefenseSquadSize::Solo => "Solo",
            DefenseSquadSize::Duo => "Duo",
            DefenseSquadSize::Quad => "Quad",
        };
        crate::visualization::SummaryContent::Text(format!(
            "SquadDefense({}) - {}atk/{}heal - {}",
            squad_desc,
            self.context.defenders.len(),
            self.context.healers.len(),
            self.state.status_description()
        ))
    }

    fn pre_run_mission(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<(), String> {
        self.state.gather_data(system_data, mission_entity);

        // Cleanup home rooms that no longer exist.
        self.context
            .home_room_datas
            .retain(|entity| system_data.room_data.get(*entity).is_some());

        if self.context.home_room_datas.is_empty() {
            return Err("No home rooms for squad defense mission".to_owned());
        }

        Ok(())
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<MissionResult, String> {
        while let Some(tick_result) = self.state.tick(system_data, mission_entity, &mut self.context)? {
            self.state = tick_result
        }

        self.state.visualize(system_data, mission_entity, &self.context);

        if matches!(self.state, SquadDefenseState::Cleanup(_)) {
            return Ok(MissionResult::Success);
        }

        Ok(MissionResult::Running)
    }
}
