use crate::serialize::*;
use screeps::*;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;

/// The type of squad formation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SquadType {
    /// Single combat creep.
    Solo,
    /// Two creeps (typically attacker + healer).
    Duo,
    /// Four creeps in a 2x2 formation.
    Quad,
}

/// High-level squad lifecycle state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SquadState {
    /// Waiting for all members to spawn.
    #[default]
    Forming,
    /// Members spawned, moving to rally point.
    Rallying,
    /// Squad is moving toward its objective.
    Moving,
    /// Squad is actively in combat.
    Engaged,
    /// Squad is retreating from combat.
    Retreating,
    /// Squad objective is complete.
    Complete,
}

/// Role a creep plays within a squad.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SquadRole {
    /// Front-line damage sponge with TOUGH + ATTACK.
    Tank,
    /// Dedicated healer with HEAL parts.
    Healer,
    /// Ranged damage dealer with RANGED_ATTACK.
    #[default]
    RangedDPS,
    /// Melee damage dealer with ATTACK.
    MeleeDPS,
    /// Structure destroyer with WORK (dismantle).
    Dismantler,
}

/// Formation shape for squad movement.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FormationType {
    /// No formation -- each creep moves independently.
    #[default]
    None,
    /// Single-file line behind the leader.
    Line,
    /// 2x2 box formation (for quads).
    Box2x2,
}

/// What the squad is trying to accomplish.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SquadTarget {
    /// Defend a specific room.
    DefendRoom { room: RoomName },
    /// Attack a specific room.
    AttackRoom { room: RoomName },
    /// Harass remote mining in a room.
    HarassRoom { room: RoomName },
    /// Move to a specific position.
    MoveToPosition { position: Position },
}

/// Per-member status reported back to the squad each tick.
#[derive(Clone, Debug, ConvertSaveload)]
pub struct SquadMember {
    /// The creep entity.
    pub entity: Entity,
    /// Role within the squad.
    pub role: SquadRole,
    /// Whether the creep is alive and has a job.
    pub alive: bool,
    /// Current HP (updated each tick by the job).
    pub current_hits: u32,
    /// Max HP.
    pub max_hits: u32,
    /// Current position (updated each tick by the job).
    pub position: Option<Position>,
}

/// Shared state for a squad, attached as an ECS component to a squad entity.
/// All member jobs read/write this each tick for coordination.
///
/// This component IS serialized so squads survive VM reloads.
#[derive(Component, ConvertSaveload)]
pub struct SquadContext {
    /// Type of squad (Solo, Duo, Quad).
    pub squad_type: SquadType,
    /// Formation shape.
    pub formation: FormationType,
    /// Current objective.
    pub target: Option<SquadTarget>,
    /// Assembly position before moving to objective.
    pub rally_point: Option<Position>,
    /// Squad lifecycle state.
    pub state: SquadState,
    /// Members of the squad with their status.
    pub members: EntityVec<SquadMember>,
    /// Shared attack focus position (all members target this).
    pub focus_target: Option<Position>,
    /// HP fraction below which the squad should retreat (0.0 - 1.0).
    pub retreat_threshold: f32,
    /// Entity of the member that most needs healing this tick.
    pub heal_priority: EntityOption<Entity>,
    /// Entity of the squad leader (for formation movement).
    pub leader: EntityOption<Entity>,
}

impl SquadContext {
    pub fn new(squad_type: SquadType) -> Self {
        let formation = match squad_type {
            SquadType::Solo => FormationType::None,
            SquadType::Duo => FormationType::Line,
            SquadType::Quad => FormationType::Box2x2,
        };

        SquadContext {
            squad_type,
            formation,
            target: None,
            rally_point: None,
            state: SquadState::Forming,
            members: EntityVec::new(),
            focus_target: None,
            retreat_threshold: 0.3,
            heal_priority: None.into(),
            leader: None.into(),
        }
    }

    /// Add a member to the squad.
    pub fn add_member(&mut self, entity: Entity, role: SquadRole) {
        self.members.push(SquadMember {
            entity,
            role,
            alive: true,
            current_hits: 0,
            max_hits: 0,
            position: None,
        });

        // First member is the leader by default.
        if self.leader.is_none() {
            *self.leader = Some(entity);
        }
    }

    /// Check if all expected members are present and alive.
    pub fn is_full(&self) -> bool {
        let expected = match self.squad_type {
            SquadType::Solo => 1,
            SquadType::Duo => 2,
            SquadType::Quad => 4,
        };
        self.members.len() >= expected && self.members.iter().all(|m| m.alive)
    }

    /// Get the average HP fraction across all living members.
    pub fn average_hp_fraction(&self) -> f32 {
        let living: Vec<_> = self.members.iter().filter(|m| m.alive && m.max_hits > 0).collect();
        if living.is_empty() {
            return 0.0;
        }
        let total_fraction: f32 = living.iter().map(|m| m.current_hits as f32 / m.max_hits as f32).sum();
        total_fraction / living.len() as f32
    }

    /// Check if the squad should retreat based on HP threshold.
    pub fn should_retreat(&self) -> bool {
        self.average_hp_fraction() < self.retreat_threshold
    }

    /// Find the member with the lowest HP fraction (for heal priority).
    pub fn update_heal_priority(&mut self) {
        let lowest = self
            .members
            .iter()
            .filter(|m| m.alive && m.max_hits > 0 && m.current_hits < m.max_hits)
            .min_by(|a, b| {
                let a_frac = a.current_hits as f32 / a.max_hits as f32;
                let b_frac = b.current_hits as f32 / b.max_hits as f32;
                a_frac.partial_cmp(&b_frac).unwrap_or(std::cmp::Ordering::Equal)
            });

        *self.heal_priority = lowest.map(|m| m.entity);
    }

    /// Remove dead members and update alive status.
    pub fn cleanup_dead(&mut self, entities: &specs::Entities) {
        for member in self.members.iter_mut() {
            if !entities.is_alive(member.entity) {
                member.alive = false;
            }
        }
    }

    /// Get the member info for a specific entity.
    pub fn get_member(&self, entity: Entity) -> Option<&SquadMember> {
        self.members.iter().find(|m| m.entity == entity)
    }

    /// Get mutable member info for a specific entity.
    pub fn get_member_mut(&mut self, entity: Entity) -> Option<&mut SquadMember> {
        self.members.iter_mut().find(|m| m.entity == entity)
    }

    /// Check if all living members are within the given range of a position.
    pub fn all_members_within_range(&self, pos: Position, range: u32) -> bool {
        self.members
            .iter()
            .filter(|m| m.alive)
            .all(|m| m.position.map(|p| p.get_range_to(pos) <= range).unwrap_or(false))
    }
}
