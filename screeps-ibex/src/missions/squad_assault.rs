use super::data::*;
use super::missionsystem::*;
use crate::creep::*;
use crate::jobs::data::*;
use crate::jobs::heal::*;
use crate::jobs::ranged::*;
use crate::jobs::tank::*;
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

/// Desired assault squad composition.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AssaultSquadSize {
    /// Single ranged attacker for weak targets.
    #[default]
    Solo,
    /// Tank + healer pair.
    Duo,
    /// Four creeps in 2x2 formation (ranged + heal mix).
    Quad,
}

#[derive(Clone, ConvertSaveload)]
pub struct SquadAssaultMissionContext {
    target_room: RoomName,
    home_room_datas: EntityVec<Entity>,
    room_data_entity: Entity,
    /// Attacker/ranged creep entities.
    attackers: EntityVec<Entity>,
    /// Healer creep entities.
    healers: EntityVec<Entity>,
    /// Tank creep entities.
    tanks: EntityVec<Entity>,
    /// Desired squad size.
    squad_size: AssaultSquadSize,
    /// Rally point where squad members gather before attacking.
    /// Chosen as a safe position near the target room (typically in an
    /// adjacent room or just outside the target room border).
    rally_room: Option<RoomName>,
    /// Tick when rallying started, used to timeout if members take too long.
    rally_start_tick: Option<u32>,
}

machine!(
    #[derive(Clone, ConvertSaveload)]
    enum SquadAssaultState {
        Spawning {
            phantom: std::marker::PhantomData<Entity>
        },
        Rallying {
            phantom: std::marker::PhantomData<Entity>
        },
        Attacking {
            phantom: std::marker::PhantomData<Entity>
        },
        Complete {
            phantom: std::marker::PhantomData<Entity>
        }
    }

    impl {
        * => fn describe_state(&self, _system_data: &MissionExecutionSystemData, _mission_entity: Entity, _state_context: &SquadAssaultMissionContext) -> String {
            format!("SquadAssault - {}", self.status_description())
        }

        * => fn status_description(&self) -> String {
            std::any::type_name::<Self>().to_string()
        }

        * => fn visualize(&self, _system_data: &MissionExecutionSystemData, _mission_entity: Entity, _state_context: &SquadAssaultMissionContext) {}

        * => fn gather_data(&self, _system_data: &MissionExecutionSystemData, _mission_entity: Entity) {}

        _ => fn tick(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity, state_context: &mut SquadAssaultMissionContext) -> Result<Option<SquadAssaultState>, String>;
    }
);

impl SquadAssaultMissionContext {
    fn cleanup_dead(&mut self, system_data: &MissionExecutionSystemData) {
        let alive = |entity: &&Entity| system_data.entities.is_alive(**entity) && system_data.job_data.get(**entity).is_some();
        self.attackers.retain(|e| alive(&e));
        self.healers.retain(|e| alive(&e));
        self.tanks.retain(|e| alive(&e));
    }

    fn total_alive(&self) -> usize {
        self.attackers.len() + self.healers.len() + self.tanks.len()
    }

    fn expected_count(&self) -> usize {
        match self.squad_size {
            AssaultSquadSize::Solo => 1,
            AssaultSquadSize::Duo => 2,
            AssaultSquadSize::Quad => 4,
        }
    }

    fn is_full(&self) -> bool {
        self.total_alive() >= self.expected_count()
    }
}

impl Spawning {
    fn tick(
        &mut self,
        system_data: &mut MissionExecutionSystemData,
        mission_entity: Entity,
        state_context: &mut SquadAssaultMissionContext,
    ) -> Result<Option<SquadAssaultState>, String> {
        state_context.cleanup_dead(system_data);

        if state_context.is_full() {
            return Ok(Some(SquadAssaultState::rallying(std::marker::PhantomData)));
        }

        let target_room = state_context.target_room;

        for home_room_entity in state_context.home_room_datas.iter() {
            let home_room_data = system_data.room_data.get(*home_room_entity).ok_or("Expected home room data")?;

            if let Some(room) = game::rooms().get(home_room_data.name) {
                let energy_capacity = room.energy_capacity_available();

                match state_context.squad_size {
                    AssaultSquadSize::Solo => {
                        if state_context.attackers.is_empty() {
                            Self::spawn_ranged(system_data, mission_entity, *home_room_entity, target_room, energy_capacity);
                        }
                    }
                    AssaultSquadSize::Duo => {
                        if state_context.tanks.is_empty() {
                            Self::spawn_tank(system_data, mission_entity, *home_room_entity, target_room, energy_capacity);
                        }
                        if state_context.healers.is_empty() {
                            Self::spawn_healer(system_data, mission_entity, *home_room_entity, target_room, energy_capacity);
                        }
                    }
                    AssaultSquadSize::Quad => {
                        // Quad: 2 ranged + 2 healers. One spawn request per tick per room.
                        if state_context.attackers.len() < 2 {
                            Self::spawn_quad_member(system_data, mission_entity, *home_room_entity, target_room, energy_capacity);
                        } else if state_context.healers.len() < 2 {
                            Self::spawn_healer(system_data, mission_entity, *home_room_entity, target_room, energy_capacity);
                        }
                    }
                }
            }
        }

        Ok(None)
    }
}

impl Spawning {
    fn spawn_ranged(
        system_data: &mut MissionExecutionSystemData,
        mission_entity: Entity,
        home_room_entity: Entity,
        target_room: RoomName,
        energy_capacity: u32,
    ) {
        let body_def = bodies::solo_defender_body(energy_capacity);
        if let Ok(body) = spawning::create_body(&body_def) {
            let token = system_data.spawn_queue.token();
            let spawn_request = SpawnRequest::new(
                format!("Assault-R {}", target_room),
                &body,
                SPAWN_PRIORITY_MEDIUM,
                Some(token),
                Self::create_ranged_callback(mission_entity, target_room),
            );
            system_data.spawn_queue.request(home_room_entity, spawn_request);
        }
    }

    fn spawn_tank(
        system_data: &mut MissionExecutionSystemData,
        mission_entity: Entity,
        home_room_entity: Entity,
        target_room: RoomName,
        energy_capacity: u32,
    ) {
        let body_def = bodies::tank_body(energy_capacity);
        if let Ok(body) = spawning::create_body(&body_def) {
            let token = system_data.spawn_queue.token();
            let spawn_request = SpawnRequest::new(
                format!("Assault-T {}", target_room),
                &body,
                SPAWN_PRIORITY_MEDIUM,
                Some(token),
                Self::create_tank_callback(mission_entity, target_room),
            );
            system_data.spawn_queue.request(home_room_entity, spawn_request);
        }
    }

    fn spawn_healer(
        system_data: &mut MissionExecutionSystemData,
        mission_entity: Entity,
        home_room_entity: Entity,
        target_room: RoomName,
        energy_capacity: u32,
    ) {
        let body_def = bodies::duo_healer_body(energy_capacity);
        if let Ok(body) = spawning::create_body(&body_def) {
            let token = system_data.spawn_queue.token();
            let spawn_request = SpawnRequest::new(
                format!("Assault-H {}", target_room),
                &body,
                SPAWN_PRIORITY_MEDIUM,
                Some(token),
                Self::create_healer_callback(mission_entity, target_room),
            );
            system_data.spawn_queue.request(home_room_entity, spawn_request);
        }
    }

    fn spawn_quad_member(
        system_data: &mut MissionExecutionSystemData,
        mission_entity: Entity,
        home_room_entity: Entity,
        target_room: RoomName,
        energy_capacity: u32,
    ) {
        let body_def = bodies::quad_member_body(energy_capacity);
        if let Ok(body) = spawning::create_body(&body_def) {
            let token = system_data.spawn_queue.token();
            let spawn_request = SpawnRequest::new(
                format!("Assault-Q {}", target_room),
                &body,
                SPAWN_PRIORITY_MEDIUM,
                Some(token),
                Self::create_ranged_callback(mission_entity, target_room),
            );
            system_data.spawn_queue.request(home_room_entity, spawn_request);
        }
    }

    fn create_ranged_callback(mission_entity: Entity, target_room: RoomName) -> SpawnQueueCallback {
        Box::new(move |system_data, name| {
            let name = name.to_string();
            system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::RangedAttack(RangedAttackJob::new(target_room));
                let creep_entity = spawning::build(world.create_entity(), &name).with(creep_job).build();
                if let Some(mission_data) = world.write_storage::<MissionData>().get(mission_entity) {
                    if let Ok(mut mission) = <std::cell::RefMut<'_, SquadAssaultMission>>::try_from(mission_data) {
                        mission.context.attackers.push(creep_entity);
                    }
                }
            });
        })
    }

    fn create_tank_callback(mission_entity: Entity, target_room: RoomName) -> SpawnQueueCallback {
        Box::new(move |system_data, name| {
            let name = name.to_string();
            system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::Tank(TankJob::new(target_room));
                let creep_entity = spawning::build(world.create_entity(), &name).with(creep_job).build();
                if let Some(mission_data) = world.write_storage::<MissionData>().get(mission_entity) {
                    if let Ok(mut mission) = <std::cell::RefMut<'_, SquadAssaultMission>>::try_from(mission_data) {
                        mission.context.tanks.push(creep_entity);
                    }
                }
            });
        })
    }

    fn create_healer_callback(mission_entity: Entity, target_room: RoomName) -> SpawnQueueCallback {
        Box::new(move |system_data, name| {
            let name = name.to_string();
            system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::Heal(HealJob::new(target_room));
                let creep_entity = spawning::build(world.create_entity(), &name).with(creep_job).build();
                if let Some(mission_data) = world.write_storage::<MissionData>().get(mission_entity) {
                    if let Ok(mut mission) = <std::cell::RefMut<'_, SquadAssaultMission>>::try_from(mission_data) {
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
        state_context: &mut SquadAssaultMissionContext,
    ) -> Result<Option<SquadAssaultState>, String> {
        state_context.cleanup_dead(system_data);

        // If we lost too many creeps during rally, go back to spawning.
        if state_context.total_alive() == 0 {
            return Ok(Some(SquadAssaultState::spawning(std::marker::PhantomData)));
        }

        // Initialize rally start tick.
        if state_context.rally_start_tick.is_none() {
            state_context.rally_start_tick = Some(game::time());
        }

        // Select rally room if not set. Pick the closest home room to the target.
        if state_context.rally_room.is_none() {
            state_context.rally_room = select_rally_room(system_data, state_context);
        }

        let rally_room = state_context.rally_room.unwrap_or(state_context.target_room);

        // Check if all members are in the rally room.
        let all_members: Vec<Entity> = state_context
            .attackers
            .iter()
            .chain(state_context.healers.iter())
            .chain(state_context.tanks.iter())
            .copied()
            .collect();

        let all_in_rally_room = all_members.iter().all(|&member_entity| {
            system_data
                .job_data
                .get(member_entity)
                .and_then(|_| {
                    // Check if the creep is in the rally room by looking at creep owners.
                    // We can't directly access creep positions from mission system data,
                    // so we check if the rally room is visible and if creeps are there.
                    game::creeps()
                        .values()
                        .find(|c| {
                            // Match by checking if any creep is in the rally room.
                            c.pos().room_name() == rally_room
                        })
                })
                .is_some()
        });

        // Timeout: if we've been rallying for more than 100 ticks, go anyway.
        let rally_timeout = state_context
            .rally_start_tick
            .map(|start| game::time() - start > 100)
            .unwrap_or(false);

        if state_context.is_full() && (all_in_rally_room || rally_timeout) {
            info!(
                "[SquadAssault] Squad rallied at {} (timeout={}), moving to attack {}",
                rally_room, rally_timeout, state_context.target_room
            );
            state_context.rally_start_tick = None;
            return Ok(Some(SquadAssaultState::attacking(std::marker::PhantomData)));
        }

        // If not full but we've been waiting a very long time (200 ticks), go with what we have.
        if rally_timeout && state_context.total_alive() > 0 {
            let start = state_context.rally_start_tick.unwrap_or(0);
            if game::time() - start > 200 {
                info!(
                    "[SquadAssault] Rally timeout exceeded, attacking with {} members",
                    state_context.total_alive()
                );
                state_context.rally_start_tick = None;
                return Ok(Some(SquadAssaultState::attacking(std::marker::PhantomData)));
            }
        }

        Ok(None)
    }
}

/// Select a rally room for the squad -- the home room closest to the target.
fn select_rally_room(
    system_data: &MissionExecutionSystemData,
    state_context: &SquadAssaultMissionContext,
) -> Option<RoomName> {
    let target = state_context.target_room;

    state_context
        .home_room_datas
        .iter()
        .filter_map(|entity| {
            system_data.room_data.get(*entity).map(|rd| rd.name)
        })
        .min_by_key(|room_name| {
            // Simple distance heuristic based on room name coordinates.
            let (tx, ty) = room_name_to_coords(target);
            let (rx, ry) = room_name_to_coords(*room_name);
            (tx - rx).unsigned_abs() + (ty - ry).unsigned_abs()
        })
}

/// Convert a RoomName to approximate (x, y) coordinates for distance calculation.
fn room_name_to_coords(name: RoomName) -> (i32, i32) {
    let s = name.to_string();
    let mut x: i32 = 0;
    let mut y: i32 = 0;
    let mut x_neg = false;
    let mut y_neg = false;
    let mut parsing_x = true;
    let mut num_buf = String::new();

    for ch in s.chars() {
        match ch {
            'W' => {
                x_neg = true;
                parsing_x = true;
            }
            'E' => {
                x_neg = false;
                parsing_x = true;
            }
            'N' => {
                if !num_buf.is_empty() {
                    x = num_buf.parse().unwrap_or(0);
                    if x_neg {
                        x = -x - 1;
                    }
                    num_buf.clear();
                }
                y_neg = true;
                parsing_x = false;
            }
            'S' => {
                if !num_buf.is_empty() {
                    x = num_buf.parse().unwrap_or(0);
                    if x_neg {
                        x = -x - 1;
                    }
                    num_buf.clear();
                }
                y_neg = false;
                parsing_x = false;
            }
            _ if ch.is_ascii_digit() => {
                num_buf.push(ch);
            }
            _ => {}
        }
    }

    if !num_buf.is_empty() {
        let val: i32 = num_buf.parse().unwrap_or(0);
        if parsing_x {
            x = if x_neg { -val - 1 } else { val };
        } else {
            y = if y_neg { -val - 1 } else { val };
        }
    }

    (x, y)
}

impl Attacking {
    fn tick(
        &mut self,
        system_data: &mut MissionExecutionSystemData,
        _mission_entity: Entity,
        state_context: &mut SquadAssaultMissionContext,
    ) -> Result<Option<SquadAssaultState>, String> {
        state_context.cleanup_dead(system_data);

        // If all creeps are dead, go back to spawning or complete.
        if state_context.total_alive() == 0 {
            return Ok(Some(SquadAssaultState::spawning(std::marker::PhantomData)));
        }

        // Jobs handle their own combat logic; the mission just monitors status.
        Ok(None)
    }
}

impl Complete {
    fn tick(
        &mut self,
        _system_data: &mut MissionExecutionSystemData,
        _mission_entity: Entity,
        _state_context: &mut SquadAssaultMissionContext,
    ) -> Result<Option<SquadAssaultState>, String> {
        Ok(None)
    }
}

#[derive(ConvertSaveload)]
pub struct SquadAssaultMission {
    owner: EntityOption<Entity>,
    context: SquadAssaultMissionContext,
    state: SquadAssaultState,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl SquadAssaultMission {
    pub fn build<B>(
        builder: B,
        owner: Option<Entity>,
        room_data_entity: Entity,
        target_room: RoomName,
        home_room_datas: &[Entity],
        squad_size: AssaultSquadSize,
    ) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = SquadAssaultMission::new(owner, room_data_entity, target_room, home_room_datas, squad_size);

        builder
            .with(MissionData::SquadAssault(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn new(
        owner: Option<Entity>,
        room_data_entity: Entity,
        target_room: RoomName,
        home_room_datas: &[Entity],
        squad_size: AssaultSquadSize,
    ) -> SquadAssaultMission {
        SquadAssaultMission {
            owner: owner.into(),
            context: SquadAssaultMissionContext {
                target_room,
                home_room_datas: home_room_datas.to_owned().into(),
                room_data_entity,
                attackers: EntityVec::new(),
                healers: EntityVec::new(),
                tanks: EntityVec::new(),
                squad_size,
                rally_room: None,
                rally_start_tick: None,
            },
            state: SquadAssaultState::spawning(std::marker::PhantomData),
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for SquadAssaultMission {
    fn get_owner(&self) -> &Option<Entity> {
        &self.owner
    }

    fn owner_complete(&mut self, owner: Entity) {
        assert!(Some(owner) == *self.owner);
        self.owner.take();
    }

    fn get_room(&self) -> Entity {
        self.context.room_data_entity
    }

    fn describe_state(&self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> String {
        self.state.describe_state(system_data, mission_entity, &self.context)
    }

    fn summarize(&self) -> crate::visualization::SummaryContent {
        let squad_desc = match self.context.squad_size {
            AssaultSquadSize::Solo => "Solo",
            AssaultSquadSize::Duo => "Duo",
            AssaultSquadSize::Quad => "Quad",
        };
        crate::visualization::SummaryContent::Text(format!(
            "SquadAssault({}) -> {} - {}atk/{}heal/{}tank - {}",
            squad_desc,
            self.context.target_room,
            self.context.attackers.len(),
            self.context.healers.len(),
            self.context.tanks.len(),
            self.state.status_description()
        ))
    }

    fn pre_run_mission(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<(), String> {
        self.state.gather_data(system_data, mission_entity);
        Ok(())
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<MissionResult, String> {
        while let Some(tick_result) = self.state.tick(system_data, mission_entity, &mut self.context)? {
            self.state = tick_result
        }

        self.state.visualize(system_data, mission_entity, &self.context);

        if matches!(self.state, SquadAssaultState::Complete(_)) {
            return Ok(MissionResult::Success);
        }

        Ok(MissionResult::Running)
    }
}
