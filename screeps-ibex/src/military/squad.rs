use crate::creep::CreepOwner;
use crate::military::composition::*;
use crate::serialize::*;
use screeps::*;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;

/// High-level squad lifecycle state.
/// Ordered by lifecycle progression for comparison (Forming < Rallying < ... < Complete).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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
    /// Resource hauler with CARRY.
    Hauler,
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
    /// Attack a specific structure (invader core, power bank).
    AttackStructure { position: Position },
    /// Collect dropped resources in a room (post-destruction exploitation).
    CollectResources { room: RoomName },
    /// Escort/defend another squad or position (power bank defense).
    EscortPosition { position: Position },
}

// ─── Virtual anchor path ────────────────────────────────────────────────────

/// Strategic path owned by the squad, not by any individual creep.
/// Stored on SquadContext and survives individual creep deaths.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SquadPath {
    /// The strategic destination the squad is moving toward.
    pub destination: Position,
    /// Room-level route to the destination (from find_route / RoomRouteCache).
    pub room_route: Vec<RoomName>,
    /// The virtual position -- where the squad "is" on the strategic path.
    /// Advanced each tick based on the actual movement of squad members.
    pub virtual_pos: Position,
    /// How many ticks the virtual position has not advanced.
    pub stuck_ticks: u16,
}

// ─── Dynamic formation layout ───────────────────────────────────────────────

/// The active formation layout -- stores the actual offsets being used this tick.
/// Recomputed by the mission when conditions change (member death, rotation,
/// formation type switch).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FormationLayout {
    /// The base formation shape.
    pub shape: FormationShape,
    /// Active offsets indexed by formation slot. Slot 0 is always (0,0).
    /// Other slots are relative to the virtual position.
    pub offsets: Vec<(i32, i32)>,
}

impl FormationLayout {
    /// No formation -- single position.
    pub fn none() -> Self {
        FormationLayout {
            shape: FormationShape::None,
            offsets: vec![(0, 0)],
        }
    }

    /// 2x2 box formation.
    pub fn box_2x2() -> Self {
        FormationLayout {
            shape: FormationShape::Box2x2,
            offsets: vec![(0, 0), (1, 0), (0, 1), (1, 1)],
        }
    }

    /// Line formation with N members.
    pub fn line(count: usize) -> Self {
        FormationLayout {
            shape: FormationShape::Line,
            offsets: (0..count).map(|i| (0, i as i32)).collect(),
        }
    }

    /// Triangle formation (3 members).
    pub fn triangle() -> Self {
        FormationLayout {
            shape: FormationShape::Triangle,
            offsets: vec![(0, 0), (-1, 1), (1, 1)],
        }
    }

    /// Wide line formation with N members.
    pub fn wide_line(count: usize) -> Self {
        FormationLayout {
            shape: FormationShape::WideLine,
            offsets: (0..count).map(|i| (i as i32, 0)).collect(),
        }
    }

    /// Create a default layout from a FormationShape and member count.
    pub fn from_shape(shape: FormationShape, count: usize) -> Self {
        match shape {
            FormationShape::None => FormationLayout::none(),
            FormationShape::Line => FormationLayout::line(count),
            FormationShape::Box2x2 => FormationLayout::box_2x2(),
            FormationShape::Triangle => FormationLayout::triangle(),
            FormationShape::WideLine => FormationLayout::wide_line(count),
        }
    }

    /// Rotate all offsets 90 degrees clockwise.
    pub fn rotate_cw(&mut self) {
        for offset in self.offsets.iter_mut() {
            let (x, y) = *offset;
            *offset = (-y, x);
        }
    }

    /// Rotate offsets to face a given direction.
    /// The default orientation has slot 0 at origin and the formation
    /// extends "south" (positive Y). This rotates so the formation
    /// extends toward the given direction.
    pub fn orient_toward(&mut self, direction: Direction) {
        let rotations = match direction {
            Direction::Top => 2,
            Direction::TopRight => 1,
            Direction::Right => 1,
            Direction::BottomRight => 0,
            Direction::Bottom => 0,
            Direction::BottomLeft => 3,
            Direction::Left => 3,
            Direction::TopLeft => 2,
        };
        for _ in 0..rotations {
            self.rotate_cw();
        }
    }

    /// Mirror offsets along the Y axis (for retreat -- flip front/back).
    pub fn mirror_y(&mut self) {
        for offset in self.offsets.iter_mut() {
            offset.1 = -offset.1;
        }
    }

    /// Get the offset for a given formation slot.
    pub fn get_offset(&self, slot: usize) -> (i32, i32) {
        self.offsets.get(slot).copied().unwrap_or((0, 0))
    }

    /// Number of slots in this layout.
    pub fn slot_count(&self) -> usize {
        self.offsets.len()
    }
}

// ─── Tick orders (per-member orders from mission to job) ────────────────────

/// Movement intent for a single tick.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TickMovement {
    /// Follow formation movement (default).
    Formation,
    /// Move to specific position (override formation).
    MoveTo(Position),
    /// Flee from threats.
    Flee,
    /// Stay put.
    Hold,
}

/// Per-creep orders from the mission to the job for a single tick.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TickOrders {
    /// Target to attack this tick (if any).
    pub attack_target: Option<Position>,
    /// Target to heal this tick (if any -- entity ID as u32 for serialization).
    pub heal_target_pos: Option<Position>,
    /// Whether to use ranged heal for the heal target.
    pub heal_ranged: bool,
    /// Movement intent for this tick.
    pub movement: TickMovement,
    /// Whether this creep should be renewing at a spawn.
    pub renewing: bool,
}

impl Default for TickOrders {
    fn default() -> Self {
        TickOrders {
            attack_target: None,
            heal_target_pos: None,
            heal_ranged: false,
            movement: TickMovement::Formation,
            renewing: false,
        }
    }
}

// ─── Heal assignment ────────────────────────────────────────────────────────

/// A computed heal assignment for one healer creep this tick.
#[derive(Clone, Debug)]
pub struct HealAssignment {
    /// Entity of the healer creep.
    pub healer: Entity,
    /// Entity of the target to heal.
    pub target: Entity,
    /// Position of the target (for range checks in the job).
    pub target_pos: Position,
    /// Whether to use ranged heal (range 2-3) vs adjacent heal (range 0-1).
    pub ranged: bool,
    /// Expected heal amount (12 per HEAL part adjacent, 4 per HEAL part ranged).
    pub expected_heal: u32,
}

// ─── Squad member ───────────────────────────────────────────────────────────

/// Per-member status reported back to the squad each tick.
#[derive(Clone, Debug, ConvertSaveload)]
pub struct SquadMember {
    /// The creep entity.
    pub entity: Entity,
    /// Role within the squad.
    pub role: SquadRole,
    /// Which composition slot this member fills (index into the
    /// `SquadComposition::slots` array). Immutable after spawn.
    pub slot_index: usize,
    /// Current HP (updated each tick by the job).
    pub current_hits: u32,
    /// Max HP.
    pub max_hits: u32,
    /// Current position (updated each tick by the job).
    pub position: Option<Position>,
    /// Which formation slot this member currently occupies (index into
    /// the formation offset array). Can be reassigned each tick by the
    /// mission to rotate creeps within the formation.
    pub formation_slot: usize,
    /// Per-tick orders from the mission (populated during pre_run).
    pub tick_orders: Option<TickOrders>,
    /// Number of active HEAL body parts (updated when member is added or refreshed).
    /// Used for heal assignment optimization.
    pub heal_power: u32,
    /// Damage taken since last tick (current_hits delta). Used to predict
    /// incoming damage for proactive healing.
    pub damage_taken_last_tick: u32,
}

// ─── Squad context ──────────────────────────────────────────────────────────

/// Anti-deadlock: max ticks to wait for stragglers in strict mode before quorum.
pub const STRICT_QUORUM_TICKS: u16 = 3;
/// Anti-deadlock: fraction of living members needed for quorum advance.
pub const STRICT_QUORUM_RATIO: f32 = 0.75;
/// Anti-deadlock: max ticks before forcing loose mode.
pub const STRICT_HOLD_MAX_TICKS: u16 = 8;

/// Shared state for a squad, attached as an ECS component to a squad entity.
/// All member jobs read/write this each tick for coordination.
///
/// This component IS serialized so squads survive VM reloads.
#[derive(Component, ConvertSaveload)]
pub struct SquadContext {
    /// Dynamic formation layout (used by virtual anchor movement).
    pub layout: Option<FormationLayout>,
    /// Strategic path owned by the squad (virtual anchor).
    pub squad_path: Option<SquadPath>,
    /// Current formation mode (Strict or Loose).
    pub formation_mode: FormationMode,
    /// The mode the squad *wants* to be in (may differ temporarily due to stuck).
    pub desired_formation_mode: FormationMode,
    /// Ticks the virtual position has been held waiting for stragglers (strict mode).
    pub strict_hold_ticks: u16,
    /// Max spread in loose mode (tiles).
    pub loose_range: u32,
    /// Direction from which threats are approaching (for formation orientation).
    pub threat_direction: Option<Direction>,
    /// Cooldown tick to prevent slot swap oscillation.
    pub last_slot_swap_tick: Option<u32>,
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
    /// Total number of members ever added (monotonically increasing).
    /// Used to detect "this squad ever had members" even after dead
    /// members are removed from the `members` vec.
    pub total_members_added: u32,
}

impl SquadContext {
    /// Create a new SquadContext from a SquadComposition.
    pub fn from_composition(composition: &SquadComposition) -> Self {
        let layout = FormationLayout::from_shape(
            composition.formation_shape,
            composition.member_count(),
        );

        SquadContext {
            layout: Some(layout),
            squad_path: None,
            formation_mode: composition.formation_mode,
            desired_formation_mode: composition.formation_mode,
            strict_hold_ticks: 0,
            loose_range: 3,
            threat_direction: None,
            last_slot_swap_tick: None,
            target: None,
            rally_point: None,
            state: SquadState::Forming,
            members: EntityVec::new(),
            focus_target: None,
            retreat_threshold: composition.retreat_threshold,
            heal_priority: None.into(),
            total_members_added: 0,
        }
    }

    /// Add a member to the squad for a specific composition slot.
    pub fn add_member(&mut self, entity: Entity, role: SquadRole, slot_index: usize) {
        let formation_slot = self.members.len();

        self.members.push(SquadMember {
            entity,
            role,
            slot_index,
            current_hits: 0,
            max_hits: 0,
            position: None,
            formation_slot,
            tick_orders: None,
            heal_power: 0,
            damage_taken_last_tick: 0,
        });
        self.total_members_added += 1;
    }

    /// Whether any member has ever been added to this squad.
    ///
    /// Unlike `!members.is_empty()`, this remains true after dead members
    /// are removed from the vec, which is important for lifecycle checks
    /// that need to distinguish "never spawned" from "all died".
    pub fn ever_had_members(&self) -> bool {
        self.total_members_added > 0
    }

    /// Check if a specific composition slot has been filled.
    pub fn is_slot_filled(&self, slot_index: usize) -> bool {
        self.members.iter().any(|m| m.slot_index == slot_index)
    }

    /// Count how many composition slots have been filled (living members).
    pub fn filled_slot_count(&self) -> usize {
        self.members.len()
    }

    /// Check if all expected members are present and alive.
    pub fn is_full(&self) -> bool {
        let expected = self
            .layout
            .as_ref()
            .map(|l| l.offsets.len())
            .unwrap_or(1);
        self.members.len() >= expected
    }

    /// Get the average HP fraction across all living members.
    pub fn average_hp_fraction(&self) -> f32 {
        let living: Vec<_> = self.members.iter().filter(|m| m.max_hits > 0).collect();
        if living.is_empty() {
            return 0.0;
        }
        let total_fraction: f32 = living.iter().map(|m| m.current_hits as f32 / m.max_hits as f32).sum();
        total_fraction / living.len() as f32
    }

    /// Check if the squad should retreat based on HP thresholds.
    ///
    /// Uses both average HP and per-member checks:
    /// - Retreat if average HP fraction is below the threshold.
    /// - Retreat if any individual member is critically damaged (below 25% HP),
    ///   since a single heavily-targeted creep weakens the whole squad.
    /// - Retreat if total squad HP deficit exceeds what healers can recover
    ///   in a reasonable number of ticks.
    pub fn should_retreat(&self) -> bool {
        let living: Vec<_> = self.members.iter().filter(|m| m.max_hits > 0).collect();
        if living.is_empty() {
            return false;
        }

        // Check average HP.
        let avg_hp = self.average_hp_fraction();
        if avg_hp < self.retreat_threshold {
            return true;
        }

        // Check if any individual member is critically damaged.
        let any_critical = living.iter().any(|m| {
            let frac = m.current_hits as f32 / m.max_hits as f32;
            frac < 0.25
        });
        if any_critical {
            return true;
        }

        // Check if total damage exceeds heal capacity by a wide margin.
        let total_deficit: u32 = living.iter().map(|m| m.max_hits - m.current_hits).sum();
        let total_heal_per_tick: u32 = living.iter().map(|m| m.heal_power * 12).sum();
        // If it would take more than 10 ticks to heal back, consider retreating.
        if total_heal_per_tick > 0 && total_deficit > total_heal_per_tick * 10 {
            return true;
        }

        false
    }

    /// Update member HP tracking and compute damage taken since last tick.
    /// Call this each tick before computing heal assignments.
    pub fn update_member_hp(&mut self, entity: Entity, hits: u32, hits_max: u32) {
        if let Some(member) = self.members.iter_mut().find(|m| m.entity == entity) {
            let prev_hits = member.current_hits;
            member.current_hits = hits;
            member.max_hits = hits_max;
            // Track damage taken (only if we had a previous reading).
            if prev_hits > 0 && hits < prev_hits {
                member.damage_taken_last_tick = prev_hits - hits;
            } else {
                member.damage_taken_last_tick = 0;
            }
        }
    }

    /// Compute optimal heal assignments for this tick.
    ///
    /// Algorithm:
    /// 1. Collect all members that need healing, sorted by urgency.
    ///    Urgency = damage deficit + predicted incoming damage (from last tick).
    /// 2. Collect all healers with their heal capacity and position.
    /// 3. Greedily assign healers to the most urgent targets, preferring
    ///    adjacent heal (12 HP/part) over ranged heal (4 HP/part).
    /// 4. Avoid over-healing: once a target's deficit is covered, move on.
    pub fn compute_heal_assignments(&self) -> Vec<HealAssignment> {
        let mut assignments = Vec::new();

        // Collect healers.
        let healers: Vec<_> = self
            .members
            .iter()
            .filter(|m| m.heal_power > 0 && m.position.is_some())
            .collect();

        if healers.is_empty() {
            return assignments;
        }

        // Collect targets needing healing, with their urgency score.
        struct HealTarget {
            entity: Entity,
            pos: Position,
            deficit: u32,
            predicted_damage: u32,
            remaining_deficit: u32,
        }

        let mut targets: Vec<HealTarget> = self
            .members
            .iter()
            .filter(|m| m.max_hits > 0 && m.position.is_some())
            .filter(|m| m.current_hits < m.max_hits || m.damage_taken_last_tick > 0)
            .map(|m| {
                let deficit = m.max_hits - m.current_hits;
                let predicted = m.damage_taken_last_tick;
                HealTarget {
                    entity: m.entity,
                    pos: m.position.unwrap(),
                    deficit,
                    predicted_damage: predicted,
                    remaining_deficit: deficit + predicted,
                }
            })
            .collect();

        // Sort by urgency: highest remaining deficit first.
        targets.sort_by_key(|t| std::cmp::Reverse(t.remaining_deficit));

        // Track which healers have been assigned.
        let mut assigned_healers: Vec<bool> = vec![false; healers.len()];

        for target in targets.iter_mut() {
            if target.remaining_deficit == 0 {
                continue;
            }

            // Find the best available healer for this target.
            let mut best_healer_idx: Option<usize> = None;
            let mut best_heal_amount: u32 = 0;
            let mut best_ranged = false;

            for (i, healer) in healers.iter().enumerate() {
                if assigned_healers[i] {
                    continue;
                }

                let healer_pos = healer.position.unwrap();
                let range = healer_pos.get_range_to(target.pos);

                let (heal_amount, ranged) = if range <= 1 {
                    (healer.heal_power * 12, false)
                } else if range <= 3 {
                    (healer.heal_power * 4, true)
                } else {
                    continue; // Out of range.
                };

                // Prefer the healer that provides the most healing.
                // Break ties by preferring adjacent heal.
                if heal_amount > best_heal_amount
                    || (heal_amount == best_heal_amount && !ranged && best_ranged)
                {
                    best_healer_idx = Some(i);
                    best_heal_amount = heal_amount;
                    best_ranged = ranged;
                }
            }

            if let Some(idx) = best_healer_idx {
                assigned_healers[idx] = true;

                // Cap heal amount to avoid over-healing.
                let effective_heal = best_heal_amount.min(target.remaining_deficit);
                target.remaining_deficit = target.remaining_deficit.saturating_sub(effective_heal);

                assignments.push(HealAssignment {
                    healer: healers[idx].entity,
                    target: target.entity,
                    target_pos: target.pos,
                    ranged: best_ranged,
                    expected_heal: effective_heal,
                });
            }
        }

        // Any unassigned healers with heal power should pre-heal the member
        // taking the most predicted damage (proactive healing).
        for (i, healer) in healers.iter().enumerate() {
            if assigned_healers[i] {
                continue;
            }

            // Find the member taking the most predicted damage that isn't fully healed.
            let best_preemptive = self
                .members
                .iter()
                .filter(|m| m.position.is_some() && m.entity != healer.entity)
                .filter(|m| {
                    let healer_pos = healer.position.unwrap();
                    healer_pos.get_range_to(m.position.unwrap()) <= 3
                })
                .max_by_key(|m| m.damage_taken_last_tick);

            if let Some(target) = best_preemptive {
                if target.damage_taken_last_tick > 0 || target.current_hits < target.max_hits {
                    let healer_pos = healer.position.unwrap();
                    let range = healer_pos.get_range_to(target.position.unwrap());
                    let ranged = range > 1;

                    assignments.push(HealAssignment {
                        healer: healer.entity,
                        target: target.entity,
                        target_pos: target.position.unwrap(),
                        ranged,
                        expected_heal: if ranged {
                            healer.heal_power * 4
                        } else {
                            healer.heal_power * 12
                        },
                    });
                }
            }
        }

        assignments
    }

    /// Find the member with the lowest HP fraction (legacy simple priority).
    pub fn update_heal_priority(&mut self) {
        let lowest = self
            .members
            .iter()
            .filter(|m| m.max_hits > 0 && m.current_hits < m.max_hits)
            .min_by(|a, b| {
                let a_frac = a.current_hits as f32 / a.max_hits as f32;
                let b_frac = b.current_hits as f32 / b.max_hits as f32;
                a_frac.partial_cmp(&b_frac).unwrap_or(std::cmp::Ordering::Equal)
            });

        *self.heal_priority = lowest.map(|m| m.entity);
    }

    /// Apply computed heal assignments to member tick orders.
    /// Call after `compute_heal_assignments()` and after tick orders have been
    /// initialized for all members.
    pub fn apply_heal_assignments(&mut self, assignments: &[HealAssignment]) {
        for assignment in assignments {
            if let Some(member) = self.members.iter_mut().find(|m| m.entity == assignment.healer) {
                if let Some(ref mut orders) = member.tick_orders {
                    orders.heal_target_pos = Some(assignment.target_pos);
                    orders.heal_ranged = assignment.ranged;
                } else {
                    member.tick_orders = Some(TickOrders {
                        heal_target_pos: Some(assignment.target_pos),
                        heal_ranged: assignment.ranged,
                        ..Default::default()
                    });
                }
            }
        }
    }

    /// Remove members whose entity is no longer alive.
    pub fn cleanup_dead(&mut self, entities: &specs::Entities) {
        self.members.retain(|m| entities.is_alive(m.entity));
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
            .all(|m| m.position.map(|p| p.get_range_to(pos) <= range).unwrap_or(false))
    }

    // ─── Retreat coordination ─────────────────────────────────────────────

    /// Compute a shared retreat position for the squad.
    /// Returns the centroid of all living members, biased away from hostiles.
    /// This keeps the squad together instead of scattering.
    pub fn compute_retreat_centroid(&self) -> Option<Position> {
        let living: Vec<_> = self
            .members
            .iter()
            .filter(|m| m.position.is_some())
            .collect();

        if living.is_empty() {
            return None;
        }

        // Compute centroid of living members.
        let room_name = living[0].position.unwrap().room_name();
        let sum_x: i32 = living.iter().map(|m| m.position.unwrap().x().u8() as i32).sum();
        let sum_y: i32 = living.iter().map(|m| m.position.unwrap().y().u8() as i32).sum();
        let count = living.len() as i32;
        let cx = (sum_x / count).clamp(1, 48) as u8;
        let cy = (sum_y / count).clamp(1, 48) as u8;

        Some(Position::new(
            RoomCoordinate::new(cx).unwrap_or(RoomCoordinate::new(25).unwrap()),
            RoomCoordinate::new(cy).unwrap_or(RoomCoordinate::new(25).unwrap()),
            room_name,
        ))
    }

    /// Issue retreat tick orders for all members.
    /// Members move toward the retreat rally point (or centroid) to stay together,
    /// with heal assignments applied so healers prioritize damaged squad members.
    pub fn issue_retreat_orders(&mut self, rally_point: Option<Position>) {
        let retreat_pos = rally_point
            .or(self.rally_point)
            .or_else(|| self.compute_retreat_centroid());

        // Compute heal assignments for the retreat.
        let heal_assignments = self.compute_heal_assignments();

        // Set movement orders: all members move toward the retreat position.
        for member in self.members.iter_mut() {
            let movement = if let Some(pos) = retreat_pos {
                TickMovement::MoveTo(pos)
            } else {
                TickMovement::Flee
            };

            member.tick_orders = Some(TickOrders {
                movement,
                ..Default::default()
            });
        }

        // Apply heal assignments on top of movement orders.
        self.apply_heal_assignments(&heal_assignments);
    }

    // ─── Formation management ───────────────────────────────────────────

    /// Update the formation layout when a member dies.
    /// Degrades the formation shape based on the number of living members.
    pub fn update_formation_for_living_count(&mut self) {
        let living_count = self.members.len();

        let base_shape = self
            .layout
            .as_ref()
            .map(|l| l.shape)
            .unwrap_or(FormationShape::None);

        let new_layout = match (base_shape, living_count) {
            (_, 0) => FormationLayout::none(),
            (_, 1) => FormationLayout::none(),
            (FormationShape::Box2x2, 3) => FormationLayout::triangle(),
            (FormationShape::Box2x2, 2) => FormationLayout::line(2),
            (FormationShape::Triangle, 2) => FormationLayout::line(2),
            (FormationShape::Line, n) => FormationLayout::line(n),
            (FormationShape::WideLine, n) => FormationLayout::wide_line(n),
            (shape, n) if n >= 4 && shape == FormationShape::Box2x2 => FormationLayout::box_2x2(),
            (_, n) => FormationLayout::line(n),
        };

        self.layout = Some(new_layout);

        if let Some(dir) = self.threat_direction {
            if let Some(layout) = self.layout.as_mut() {
                layout.orient_toward(dir);
            }
        }

        self.compact_formation_slots();
    }

    /// Reassign formation slots so members get consecutive slots.
    pub fn compact_formation_slots(&mut self) {
        for (i, member) in self.members.iter_mut().enumerate() {
            member.formation_slot = i;
        }
    }

    /// Reassign formation slots based on tactical conditions.
    /// Called by the mission each tick during Engaged/Retreating states.
    pub fn reassign_slots(&mut self) {
        let living: Vec<usize> = (0..self.members.len()).collect();

        if living.len() <= 1 {
            return;
        }

        // Determine which slots face the threat.
        let threat_slots = self.threat_facing_slots();
        let safe_slots = self.safe_slots();

        if threat_slots.is_empty() || safe_slots.is_empty() {
            return;
        }

        // Score each living member for "should be in front" (facing threat):
        // Higher HP fraction = more front-worthy, Tank role = more front-worthy,
        // Healer role = less front-worthy.
        let mut scored: Vec<(usize, f32)> = living
            .iter()
            .map(|&idx| {
                let m = &self.members[idx];
                let hp_score = if m.max_hits > 0 {
                    m.current_hits as f32 / m.max_hits as f32
                } else {
                    0.5
                };
                let role_score = match m.role {
                    SquadRole::Tank => 1.0,
                    SquadRole::MeleeDPS => 0.8,
                    SquadRole::RangedDPS => 0.6,
                    SquadRole::Dismantler => 0.5,
                    SquadRole::Hauler => 0.2,
                    SquadRole::Healer => 0.1,
                };
                (idx, hp_score * 0.6 + role_score * 0.4)
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Assign highest-scored members to threat-facing slots.
        let all_slots: Vec<usize> = threat_slots
            .iter()
            .chain(safe_slots.iter())
            .copied()
            .collect();

        for (i, &(member_idx, _)) in scored.iter().enumerate() {
            if let Some(&slot) = all_slots.get(i) {
                self.members[member_idx].formation_slot = slot;
            }
        }
    }

    /// Get formation slot indices that face the threat direction.
    fn threat_facing_slots(&self) -> Vec<usize> {
        let layout = match &self.layout {
            Some(l) => l,
            None => return Vec::new(),
        };

        let _direction = match self.threat_direction {
            Some(d) => d,
            None => return (0..layout.slot_count()).collect(),
        };

        // For a box formation, the "front" slots depend on threat direction.
        // Simplified: slots with the smallest Y offset face "forward" (threat).
        if layout.offsets.len() <= 1 {
            return vec![0];
        }

        let min_y = layout.offsets.iter().map(|(_, y)| *y).min().unwrap_or(0);
        layout
            .offsets
            .iter()
            .enumerate()
            .filter(|(_, (_, y))| *y == min_y)
            .map(|(i, _)| i)
            .collect()
    }

    /// Get formation slot indices that are "safe" (away from threat).
    fn safe_slots(&self) -> Vec<usize> {
        let layout = match &self.layout {
            Some(l) => l,
            None => return Vec::new(),
        };

        if layout.offsets.len() <= 1 {
            return Vec::new();
        }

        let max_y = layout.offsets.iter().map(|(_, y)| *y).max().unwrap_or(0);
        layout
            .offsets
            .iter()
            .enumerate()
            .filter(|(_, (_, y))| *y == max_y)
            .map(|(i, _)| i)
            .collect()
    }
}

// ─── Squad update systems ───────────────────────────────────────────────────

/// Pre-run pass: gather fresh state from live game objects so that missions
/// see accurate HP, position, and alive status when they compute tick orders.
///
/// Runs before `RunMissionSystem`.
///
/// Responsibilities:
/// - Clear stale tick orders from the previous tick.
/// - Mark members as dead when their entity is deleted or creep is gone.
/// - Update `position`, `current_hits`, `max_hits`, `damage_taken_last_tick`.
/// - Initialize `heal_power` from body parts (once, when first seen alive).
pub struct PreRunSquadUpdateSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for PreRunSquadUpdateSystem {
    type SystemData = (
        Entities<'a>,
        WriteStorage<'a, SquadContext>,
        ReadStorage<'a, CreepOwner>,
    );

    fn run(&mut self, (entities, mut squad_contexts, creep_owners): Self::SystemData) {
        for (_, squad_ctx) in (&entities, &mut squad_contexts).join() {
            // Clear previous tick's orders so missions start from a clean slate.
            for member in squad_ctx.members.iter_mut() {
                member.tick_orders = None;
            }

            // Remove dead members (entity deleted or creep gone).
            squad_ctx.members.retain(|m| {
                if !entities.is_alive(m.entity) {
                    return false;
                }
                if let Some(creep_owner) = creep_owners.get(m.entity) {
                    creep_owner.owner.resolve().is_some()
                } else {
                    false
                }
            });

            // Update live member state from the game world.
            for member in squad_ctx.members.iter_mut() {
                let creep = creep_owners
                    .get(member.entity)
                    .and_then(|co| co.owner.resolve());

                if let Some(creep) = creep {
                    member.position = Some(creep.pos());
                    let prev_hits = member.current_hits;
                    member.current_hits = creep.hits();
                    member.max_hits = creep.hits_max();

                    if prev_hits > 0 && creep.hits() < prev_hits {
                        member.damage_taken_last_tick = prev_hits - creep.hits();
                    } else {
                        member.damage_taken_last_tick = 0;
                    }

                    if member.heal_power == 0 {
                        member.heal_power = creep
                            .body()
                            .iter()
                            .filter(|p| p.part() == Part::Heal && p.hits() > 0)
                            .count() as u32;
                    }
                }
            }
        }
    }
}

/// Run pass: apply post-mission state changes before jobs execute.
///
/// Runs after `RunMissionSystem` and before `RunJobSystem`.
///
/// Responsibilities:
/// - Degrade formation layout when members have died (detected by pre-run).
pub struct RunSquadUpdateSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for RunSquadUpdateSystem {
    type SystemData = (
        Entities<'a>,
        WriteStorage<'a, SquadContext>,
    );

    fn run(&mut self, (entities, mut squad_contexts): Self::SystemData) {
        for (_, squad_ctx) in (&entities, &mut squad_contexts).join() {
            let living_count = squad_ctx.members.len();
            let slot_count = squad_ctx
                .layout
                .as_ref()
                .map(|l| l.slot_count())
                .unwrap_or(1);

            // Only update if the formation no longer fits the living count.
            if living_count < slot_count {
                squad_ctx.update_formation_for_living_count();
            }
        }
    }
}
