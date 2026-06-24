use super::bodies;
use super::squad::SquadRole;
use crate::creep::SpawnBodyDefinition;
use crate::military::force_sizing::RequiredForce;
use crate::pathing::pathfinderservice::PathfinderService;
use screeps::*;
use serde::{Deserialize, Serialize};

/// Screeps constants for lifetime calculations.
const CREEP_LIFE_TIME: u32 = 1500;
const CREEP_SPAWN_TIME: u32 = 3;

/// Most members a single force-sized squad may grow to (D3 member-count scaling). Beyond this the
/// target needs the multi-squad **G4-HEAVY** path (P5), so [`SquadComposition::sized_for`] defers
/// rather than field an unmanageable blob. 2× a quad — enough to out-heal an L1-2 stronghold /
/// multi-keeper SK at RCL7+, bounded for formation + CPU sanity.
const MAX_SIZED_MEMBERS: usize = 8;

/// Most parts of ONE role-type a single sized member can carry: a pure single-part body on plains
/// (1:1 MOVE) is `2n` parts, so the 50-part engine cap bounds `n` at 25. The upper bound of the
/// per-member capacity search in [`SquadComposition::sized_for`].
const MAX_SINGLE_ROLE_PARTS: u32 = 25;

/// Enum of body definition selectors (maps to functions in bodies.rs).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BodyType {
    SoloDefender,
    DuoRangedAttacker,
    DuoMeleeAttacker,
    DuoHealer,
    QuadMember,
    Tank,
    Drain,
    Harasser,
    Dismantler,
    // Specialized roles
    SkRangedAttacker,
    SkHealer,
    PowerBankAttacker,
    PowerBankHealer,
    SiegeDismantler,
    CoreAttacker,
    Hauler,
    // Boosted variants
    BoostedQuadMember,
    BoostedDuoHealer,
    BoostedDuoRangedAttacker,
    BoostedTank,
    /// A force-SIZED body (R3, ADR 0020 §12.6): explicit part counts from the force-sizing solver,
    /// built via `bodies::build_combat_body` rather than a static template. APPENDED LAST so existing
    /// serialized variant discriminants are unchanged (forward-compatible decode).
    Sized(bodies::CombatBodySpec),
}

impl BodyType {
    /// Get the SpawnBodyDefinition for this body type at a given energy capacity.
    pub fn body_definition(&self, max_energy: u32) -> SpawnBodyDefinition<'static> {
        match self {
            BodyType::SoloDefender => bodies::solo_defender_body(max_energy),
            BodyType::DuoRangedAttacker => bodies::duo_ranged_attacker_body(max_energy),
            BodyType::DuoMeleeAttacker => bodies::duo_melee_attacker_body(max_energy),
            BodyType::DuoHealer => bodies::duo_healer_body(max_energy),
            BodyType::QuadMember => bodies::quad_member_body(max_energy),
            BodyType::Tank => bodies::tank_body(max_energy),
            BodyType::Drain => bodies::drain_body(max_energy),
            BodyType::Harasser => bodies::harasser_body(),
            BodyType::Dismantler => bodies::dismantler_body(max_energy),
            BodyType::SkRangedAttacker => bodies::sk_ranged_attacker_body(max_energy),
            BodyType::SkHealer => bodies::sk_healer_body(max_energy),
            BodyType::PowerBankAttacker => bodies::power_bank_attacker_body(max_energy),
            BodyType::PowerBankHealer => bodies::power_bank_healer_body(max_energy),
            BodyType::SiegeDismantler => bodies::siege_dismantler_body(max_energy),
            BodyType::CoreAttacker => bodies::core_attacker_body(max_energy),
            BodyType::Hauler => bodies::hauler_body(max_energy),
            BodyType::BoostedQuadMember => bodies::boosted_quad_member_body(max_energy),
            BodyType::BoostedDuoHealer => bodies::boosted_duo_healer_body(max_energy),
            BodyType::BoostedDuoRangedAttacker => bodies::boosted_duo_ranged_attacker_body(max_energy),
            BodyType::BoostedTank => bodies::boosted_tank_body(max_energy),
            BodyType::Sized(_) => unreachable!("Sized bodies build via BodyType::build_body, not body_definition"),
        }
    }

    /// Build the spawn body for this body type at `max_energy` over `move_profile`: a `Sized` spec via
    /// the dynamic builder (R1), else the static template through `create_body`. `None` ⇒ can't build /
    /// can't afford. The single body-producing entry point for the spawn path (handles both kinds).
    pub fn build_body(&self, max_energy: u32, move_profile: bodies::MoveProfile) -> Option<Vec<Part>> {
        match self {
            BodyType::Sized(spec) => bodies::build_combat_body(spec, move_profile, max_energy),
            other => crate::creep::spawning::create_body(&other.body_definition(max_energy)).ok(),
        }
    }

    /// Estimate the body cost at a given energy capacity.
    pub fn estimated_cost(&self, max_energy: u32) -> u32 {
        if let BodyType::Sized(spec) = self {
            let moves = bodies::MoveProfile::Plains.move_parts(spec.non_move_parts());
            return spec.tough * Part::Tough.cost()
                + spec.attack * Part::Attack.cost()
                + spec.ranged_attack * Part::RangedAttack.cost()
                + spec.work * Part::Work.cost()
                + spec.carry * Part::Carry.cost()
                + spec.heal * Part::Heal.cost()
                + moves * Part::Move.cost();
        }
        let def = self.body_definition(max_energy);
        let pre_cost: u32 = def.pre_body.iter().map(|p| p.cost()).sum();
        let post_cost: u32 = def.post_body.iter().map(|p| p.cost()).sum();
        let repeat_cost: u32 = def.repeat_body.iter().map(|p| p.cost()).sum();
        let fixed_cost = pre_cost + post_cost;
        let remaining = max_energy.saturating_sub(fixed_cost);

        if repeat_cost == 0 {
            return fixed_cost;
        }

        let fixed_len = def.pre_body.len() + def.post_body.len();
        let max_by_cost = remaining / repeat_cost;
        let max_by_size = if !def.repeat_body.is_empty() {
            (50usize.saturating_sub(fixed_len)) / def.repeat_body.len()
        } else {
            0
        };
        let repeats = max_by_cost.min(max_by_size as u32);
        let repeats = match def.maximum_repeat {
            Some(max) => repeats.min(max as u32),
            None => repeats,
        };

        fixed_cost + repeats * repeat_cost
    }

    /// Estimate the number of body parts at a given energy capacity.
    pub fn estimated_part_count(&self, max_energy: u32) -> u32 {
        if let BodyType::Sized(spec) = self {
            return spec.non_move_parts() + bodies::MoveProfile::Plains.move_parts(spec.non_move_parts());
        }
        let def = self.body_definition(max_energy);
        let pre_cost: u32 = def.pre_body.iter().map(|p| p.cost()).sum();
        let post_cost: u32 = def.post_body.iter().map(|p| p.cost()).sum();
        let repeat_cost: u32 = def.repeat_body.iter().map(|p| p.cost()).sum();
        let fixed_cost = pre_cost + post_cost;
        let remaining = max_energy.saturating_sub(fixed_cost);

        if repeat_cost == 0 {
            return (def.pre_body.len() + def.post_body.len()) as u32;
        }

        let fixed_len = def.pre_body.len() + def.post_body.len();
        let max_by_cost = remaining / repeat_cost;
        let max_by_size = if !def.repeat_body.is_empty() {
            (50usize.saturating_sub(fixed_len)) / def.repeat_body.len()
        } else {
            0
        };
        let repeats = max_by_cost.min(max_by_size as u32);
        let repeats = match def.maximum_repeat {
            Some(max) => repeats.min(max as u32),
            None => repeats,
        };

        (fixed_len as u32) + repeats * (def.repeat_body.len() as u32)
    }

    /// Count of `part` in the expanded body at `max_energy` — the per-part-type input the force-sizing
    /// oracle needs (ADR 0020 §12.2). Mirrors `estimated_part_count`'s repeat math but counts one type.
    pub fn part_count(&self, max_energy: u32, part: Part) -> u32 {
        if let BodyType::Sized(spec) = self {
            return match part {
                Part::Tough => spec.tough,
                Part::Attack => spec.attack,
                Part::RangedAttack => spec.ranged_attack,
                Part::Work => spec.work,
                Part::Carry => spec.carry,
                Part::Heal => spec.heal,
                Part::Move => bodies::MoveProfile::Plains.move_parts(spec.non_move_parts()),
                _ => 0,
            };
        }
        let def = self.body_definition(max_energy);
        let in_slice = |s: &[Part]| s.iter().filter(|p| **p == part).count() as u32;
        let fixed = in_slice(def.pre_body) + in_slice(def.post_body);
        let per_repeat = in_slice(def.repeat_body);
        if per_repeat == 0 {
            return fixed;
        }

        let repeat_cost: u32 = def.repeat_body.iter().map(|p| p.cost()).sum();
        let pre_cost: u32 = def.pre_body.iter().map(|p| p.cost()).sum();
        let post_cost: u32 = def.post_body.iter().map(|p| p.cost()).sum();
        let fixed_cost = pre_cost + post_cost;
        let fixed_len = def.pre_body.len() + def.post_body.len();
        let max_by_cost = max_energy.saturating_sub(fixed_cost) / repeat_cost.max(1);
        let max_by_size = (50usize.saturating_sub(fixed_len)) / def.repeat_body.len().max(1);
        let repeats = max_by_cost.min(max_by_size as u32);
        let repeats = match def.maximum_repeat {
            Some(max) => repeats.min(max as u32),
            None => repeats,
        };
        fixed + per_repeat * repeats
    }

    /// List the boost compounds required for this body type (if boosted).
    pub fn required_boosts(&self) -> Vec<(ResourceType, u32)> {
        match self {
            BodyType::BoostedQuadMember => vec![
                (bodies::boosts::TOUGH_BOOST, 6),
                (bodies::boosts::RANGED_ATTACK_BOOST, 10),
                (bodies::boosts::HEAL_BOOST, 10),
                (bodies::boosts::MOVE_BOOST, 10),
            ],
            BodyType::BoostedDuoHealer => vec![
                (bodies::boosts::TOUGH_BOOST, 8),
                (bodies::boosts::HEAL_BOOST, 20),
                (bodies::boosts::MOVE_BOOST, 6),
            ],
            BodyType::BoostedDuoRangedAttacker => vec![
                (bodies::boosts::TOUGH_BOOST, 6),
                (bodies::boosts::RANGED_ATTACK_BOOST, 20),
                (bodies::boosts::MOVE_BOOST, 6),
            ],
            BodyType::BoostedTank => vec![
                (bodies::boosts::TOUGH_BOOST, 12),
                (bodies::boosts::ATTACK_BOOST, 15),
                (bodies::boosts::MOVE_BOOST, 8),
            ],
            _ => Vec::new(),
        }
    }
}

/// A single slot in a squad composition.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SquadSlot {
    /// What role this slot fills.
    pub role: SquadRole,
    /// Which body definition to use for spawning.
    pub body_type: BodyType,
}

/// Base formation shapes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FormationShape {
    #[default]
    None,
    Line,
    Box2x2,
    Triangle,
    WideLine,
}

/// Formation movement mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FormationMode {
    /// Virtual position only advances when all living members are in formation.
    #[default]
    Strict,
    /// Virtual position advances based on member centroid.
    Loose,
}

/// Defines what a squad should look like when fully spawned.
/// Data-driven replacement for the Solo/Duo/Quad enums.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SquadComposition {
    /// Human-readable label for logging/visualization.
    pub label: String,
    /// The slots that need to be filled.
    pub slots: Vec<SquadSlot>,
    /// Base formation shape for this composition.
    pub formation_shape: FormationShape,
    /// Default formation mode.
    pub formation_mode: FormationMode,
    /// HP fraction below which the squad should retreat (0.0 - 1.0).
    /// Defaults to 0.3 for most compositions; higher for bursty combat (e.g. SK).
    #[serde(default = "default_retreat_threshold")]
    pub retreat_threshold: f32,
}

fn default_retreat_threshold() -> f32 {
    0.3
}

impl SquadComposition {
    // ─── Predefined compositions ────────────────────────────────────────

    /// 1 ranged+heal creep, no formation.
    pub fn solo_ranged() -> Self {
        SquadComposition {
            label: "Solo Ranged".into(),
            slots: vec![SquadSlot {
                role: SquadRole::RangedDPS,
                body_type: BodyType::SoloDefender,
            }],
            formation_shape: FormationShape::None,
            formation_mode: FormationMode::Loose,
            retreat_threshold: default_retreat_threshold(),
        }
    }

    /// 1 ranged attacker + 1 healer, line formation.
    pub fn duo_attack_heal() -> Self {
        SquadComposition {
            label: "Duo Attack+Heal".into(),
            slots: vec![
                SquadSlot {
                    role: SquadRole::RangedDPS,
                    body_type: BodyType::DuoRangedAttacker,
                },
                SquadSlot {
                    role: SquadRole::Healer,
                    body_type: BodyType::DuoHealer,
                },
            ],
            formation_shape: FormationShape::Line,
            formation_mode: FormationMode::Strict,
            retreat_threshold: default_retreat_threshold(),
        }
    }

    /// 1 tank + 1 healer, line formation.
    pub fn duo_tank_heal() -> Self {
        SquadComposition {
            label: "Duo Tank+Heal".into(),
            slots: vec![
                SquadSlot {
                    role: SquadRole::Tank,
                    body_type: BodyType::Tank,
                },
                SquadSlot {
                    role: SquadRole::Healer,
                    body_type: BodyType::DuoHealer,
                },
            ],
            formation_shape: FormationShape::Line,
            formation_mode: FormationMode::Strict,
            retreat_threshold: default_retreat_threshold(),
        }
    }

    /// 2 ranged + 2 healers, box formation, strict mode.
    pub fn quad_ranged() -> Self {
        SquadComposition {
            label: "Quad Ranged".into(),
            slots: vec![
                SquadSlot {
                    role: SquadRole::RangedDPS,
                    body_type: BodyType::QuadMember,
                },
                SquadSlot {
                    role: SquadRole::RangedDPS,
                    body_type: BodyType::QuadMember,
                },
                SquadSlot {
                    role: SquadRole::Healer,
                    body_type: BodyType::DuoHealer,
                },
                SquadSlot {
                    role: SquadRole::Healer,
                    body_type: BodyType::DuoHealer,
                },
            ],
            formation_shape: FormationShape::Box2x2,
            formation_mode: FormationMode::Strict,
            retreat_threshold: default_retreat_threshold(),
        }
    }

    /// 2 dismantlers + 2 healers, box formation.
    pub fn quad_siege() -> Self {
        SquadComposition {
            label: "Quad Siege".into(),
            slots: vec![
                SquadSlot {
                    role: SquadRole::Dismantler,
                    body_type: BodyType::Dismantler,
                },
                SquadSlot {
                    role: SquadRole::Dismantler,
                    body_type: BodyType::Dismantler,
                },
                SquadSlot {
                    role: SquadRole::Healer,
                    body_type: BodyType::DuoHealer,
                },
                SquadSlot {
                    role: SquadRole::Healer,
                    body_type: BodyType::DuoHealer,
                },
            ],
            formation_shape: FormationShape::Box2x2,
            formation_mode: FormationMode::Strict,
            retreat_threshold: default_retreat_threshold(),
        }
    }

    /// 2 drain creeps, strict formation so they stay adjacent and can heal each other while tanking.
    pub fn duo_drain() -> Self {
        SquadComposition {
            label: "Duo Drain".into(),
            slots: vec![
                SquadSlot {
                    role: SquadRole::Tank,
                    body_type: BodyType::Drain,
                },
                SquadSlot {
                    role: SquadRole::Tank,
                    body_type: BodyType::Drain,
                },
            ],
            formation_shape: FormationShape::Line,
            formation_mode: FormationMode::Strict,
            retreat_threshold: default_retreat_threshold(),
        }
    }

    /// 1 cheap ranged, no formation.
    pub fn solo_harasser() -> Self {
        SquadComposition {
            label: "Solo Harasser".into(),
            slots: vec![SquadSlot {
                role: SquadRole::RangedDPS,
                body_type: BodyType::Harasser,
            }],
            formation_shape: FormationShape::None,
            formation_mode: FormationMode::Loose,
            retreat_threshold: default_retreat_threshold(),
        }
    }

    /// 1 melee attacker + 1 healer, line formation (invader core / power bank).
    pub fn duo_melee_heal() -> Self {
        SquadComposition {
            label: "Duo Melee+Heal".into(),
            slots: vec![
                SquadSlot {
                    role: SquadRole::MeleeDPS,
                    body_type: BodyType::DuoMeleeAttacker,
                },
                SquadSlot {
                    role: SquadRole::Healer,
                    body_type: BodyType::DuoHealer,
                },
            ],
            formation_shape: FormationShape::Line,
            formation_mode: FormationMode::Strict,
            retreat_threshold: default_retreat_threshold(),
        }
    }

    /// Source Keeper farming duo: 1 ranged kiter + 1 healer, line formation.
    /// The ranged attacker kites at range 3 while the healer keeps it alive.
    /// Higher retreat threshold (0.5) since SK damage is bursty.
    pub fn duo_sk_farmer() -> Self {
        SquadComposition {
            label: "SK Farmer Duo".into(),
            slots: vec![
                SquadSlot {
                    role: SquadRole::RangedDPS,
                    body_type: BodyType::SkRangedAttacker,
                },
                SquadSlot {
                    role: SquadRole::Healer,
                    body_type: BodyType::SkHealer,
                },
            ],
            formation_shape: FormationShape::Line,
            formation_mode: FormationMode::Strict,
            retreat_threshold: 0.5,
        }
    }

    /// Power bank farming duo: heavy melee attacker + heavy healer.
    /// The attacker hits the bank while the healer outheals damage reflection.
    pub fn power_bank_duo() -> Self {
        SquadComposition {
            label: "Power Bank Duo".into(),
            slots: vec![
                SquadSlot {
                    role: SquadRole::MeleeDPS,
                    body_type: BodyType::PowerBankAttacker,
                },
                SquadSlot {
                    role: SquadRole::Healer,
                    body_type: BodyType::PowerBankHealer,
                },
            ],
            formation_shape: FormationShape::Line,
            formation_mode: FormationMode::Strict,
            retreat_threshold: default_retreat_threshold(),
        }
    }

    /// Power bank hauler squad: multiple haulers to collect dropped power.
    /// Deployed after the bank is destroyed.
    pub fn power_bank_haulers(count: usize) -> Self {
        SquadComposition {
            label: format!("Power Bank Haulers x{}", count),
            slots: (0..count)
                .map(|_| SquadSlot {
                    role: SquadRole::Hauler,
                    body_type: BodyType::Hauler,
                })
                .collect(),
            formation_shape: FormationShape::None,
            formation_mode: FormationMode::Loose,
            retreat_threshold: default_retreat_threshold(),
        }
    }

    /// Siege quad: 2 dismantlers + 2 healers in box formation.
    /// Uses heavy siege dismantler bodies for maximum wall/rampart damage.
    pub fn siege_quad() -> Self {
        SquadComposition {
            label: "Siege Quad".into(),
            slots: vec![
                SquadSlot {
                    role: SquadRole::Dismantler,
                    body_type: BodyType::SiegeDismantler,
                },
                SquadSlot {
                    role: SquadRole::Dismantler,
                    body_type: BodyType::SiegeDismantler,
                },
                SquadSlot {
                    role: SquadRole::Healer,
                    body_type: BodyType::DuoHealer,
                },
                SquadSlot {
                    role: SquadRole::Healer,
                    body_type: BodyType::DuoHealer,
                },
            ],
            formation_shape: FormationShape::Box2x2,
            formation_mode: FormationMode::Strict,
            retreat_threshold: default_retreat_threshold(),
        }
    }

    /// Cheap solo attacker for level 0 invader cores.
    pub fn solo_core_attacker() -> Self {
        SquadComposition {
            label: "Solo Core Attacker".into(),
            slots: vec![SquadSlot {
                role: SquadRole::MeleeDPS,
                body_type: BodyType::CoreAttacker,
            }],
            formation_shape: FormationShape::None,
            formation_mode: FormationMode::Loose,
            retreat_threshold: default_retreat_threshold(),
        }
    }

    // ─── Cost and timing estimation ─────────────────────────────────────

    /// Estimate the total energy cost to spawn this composition
    /// at a given energy capacity.
    pub fn estimated_cost(&self, energy_capacity: u32) -> u32 {
        self.slots.iter().map(|slot| slot.body_type.estimated_cost(energy_capacity)).sum()
    }

    /// Estimate total spawn time for this composition (ticks to spawn all members).
    /// Each body part takes CREEP_SPAWN_TIME (3) ticks. With N spawns available,
    /// members can be spawned in parallel.
    pub fn estimated_spawn_time(&self, energy_capacity: u32, available_spawns: u32) -> u32 {
        if available_spawns == 0 || self.slots.is_empty() {
            return u32::MAX;
        }

        let mut part_counts: Vec<u32> = self
            .slots
            .iter()
            .map(|slot| slot.body_type.estimated_part_count(energy_capacity))
            .collect();

        // Sort descending so longest spawns go first.
        part_counts.sort_unstable_by(|a, b| b.cmp(a));

        // Simulate parallel spawning across available_spawns.
        let mut spawn_lanes = vec![0u32; available_spawns as usize];
        for parts in &part_counts {
            // Assign to the lane that finishes earliest.
            let min_lane = spawn_lanes.iter_mut().min().unwrap();
            *min_lane += parts * CREEP_SPAWN_TIME;
        }

        spawn_lanes.into_iter().max().unwrap_or(0)
    }

    /// Estimate travel time from a home room to a target room (ticks).
    /// Uses the PathfinderService route cache for accurate room-hop
    /// distance via find_route(). Returns None if unreachable.
    pub fn estimated_travel_time(pathfinder: &mut PathfinderService, home: RoomName, target: RoomName) -> Option<u32> {
        pathfinder.travel_ticks(home, target, screeps::game::time())
    }

    /// Estimate useful combat time for this composition when spawned from a
    /// given home room targeting a given room. Accounts for spawn time, travel
    /// time, and CREEP_LIFE_TIME. Returns None if unreachable.
    pub fn estimated_combat_time(
        &self,
        pathfinder: &mut PathfinderService,
        home: RoomName,
        target: RoomName,
        energy_capacity: u32,
        available_spawns: u32,
    ) -> Option<u32> {
        let spawn_time = self.estimated_spawn_time(energy_capacity, available_spawns);
        let travel_time = Self::estimated_travel_time(pathfinder, home, target)?;
        Some(CREEP_LIFE_TIME.saturating_sub(spawn_time + travel_time))
    }

    /// Check if spawning from this home room gives enough combat time to be
    /// worthwhile. Returns false if unreachable or creeps would arrive with
    /// <40% lifetime.
    pub fn is_viable_from(
        &self,
        pathfinder: &mut PathfinderService,
        home: RoomName,
        target: RoomName,
        energy_capacity: u32,
        available_spawns: u32,
    ) -> bool {
        match self.estimated_combat_time(pathfinder, home, target, energy_capacity, available_spawns) {
            Some(combat_time) => combat_time as f32 > CREEP_LIFE_TIME as f32 * 0.4,
            None => false,
        }
    }

    /// List all boost compounds required for this composition.
    pub fn required_boosts(&self) -> Vec<(ResourceType, u32)> {
        let mut boosts: Vec<(ResourceType, u32)> = Vec::new();
        for slot in &self.slots {
            for (compound, amount) in slot.body_type.required_boosts() {
                if let Some(existing) = boosts.iter_mut().find(|(c, _)| *c == compound) {
                    existing.1 += amount;
                } else {
                    boosts.push((compound, amount));
                }
            }
        }
        boosts
    }

    /// Number of creeps in this composition.
    pub fn member_count(&self) -> usize {
        self.slots.len()
    }

    /// This composition's combat capabilities at a given spawn energy — the [`force_sizing`] oracle's
    /// `ForceBudget` inputs (ADR 0020 §12.2). Bodies auto-size to `max_energy` (the same sizing the
    /// spawner uses), so the assessment reflects what we'd actually field at this RCL. Unboosted (v1).
    ///
    /// [`force_sizing`]: super::force_sizing
    pub fn capabilities(&self, max_energy: u32) -> SquadCapabilities {
        let mut heal_per_tick = 0u32;
        let mut structure_dps = 0u32;
        let mut tank_effective_hp = 0u32;
        for slot in &self.slots {
            let bt = slot.body_type;
            heal_per_tick += bt.part_count(max_energy, Part::Heal) * HEAL_POWER;
            // Structure damage: WORK dismantles (50/part), ATTACK hits a structure (30/part),
            // RANGED_ATTACK hits a structure (10/part at range ≤3). All contribute to breaching
            // ramparts and killing the core. RANGED_ATTACK MUST be counted: invader cores are
            // dismantle-IMMUNE, so a ranged comp is what actually kills them — without this the oracle
            // sees a `quad_ranged` core-attacker as 0 structure-DPS and defers every core as "breach
            // too slow".
            structure_dps += bt.part_count(max_energy, Part::Work) * DISMANTLE_POWER
                + bt.part_count(max_energy, Part::Attack) * ATTACK_POWER
                + bt.part_count(max_energy, Part::RangedAttack) * RANGED_ATTACK_POWER;
            // The tank is the toughest single member (most total HP = parts × 100, unboosted).
            tank_effective_hp = tank_effective_hp.max(bt.estimated_part_count(max_energy) * 100);
        }
        SquadCapabilities { heal_per_tick, structure_dps, tank_effective_hp }
    }

    /// Force-DRIVEN sizing (R3 + D3 member-count scaling, ADR 0020 §12.6 / ADR 0022 D3): return a copy
    /// of this composition sized to deliver `force`. Each role covered by `force` (Healer→HEAL,
    /// Dismantler→WORK, Tank→TOUGH) is sized to its even share of the required parts; when one member
    /// can't carry that share (the 50-part cap or `max_member_energy`), the role's member COUNT is
    /// GROWN (`ceil(parts / per-member-cap)`, never below the template count) and the parts
    /// re-distributed evenly across the grown count — so an UNDER-strength squad is never fielded and
    /// the runtime engage gate holds instead of retreating (the direct fix for the P2b / SK-trickle
    /// engage-retreat bug; size to hold from one calc). Returns `None` only when a required role can't
    /// field even ONE member at this energy, or the squad would exceed [`MAX_SIZED_MEMBERS`] (→ defer
    /// to the multi-squad G4-HEAVY path, P5). Roles not in `force` keep their template body; per-member
    /// MOVE is applied by [`bodies::build_combat_body`]. (Full role re-allocation across a blob is
    /// R8/0020-S5.)
    pub fn sized_for(&self, force: RequiredForce, max_member_energy: u32) -> Option<SquadComposition> {
        // A single-role part SPEC (the only roles `force` covers: HEAL / WORK / TOUGH).
        let spec_for = |role: SquadRole, n: u32| -> bodies::CombatBodySpec {
            match role {
                SquadRole::Healer => bodies::CombatBodySpec { heal: n, ..Default::default() },
                SquadRole::Dismantler => bodies::CombatBodySpec { work: n, ..Default::default() },
                SquadRole::Tank => bodies::CombatBodySpec { tough: n, ..Default::default() },
                _ => bodies::CombatBodySpec::default(),
            }
        };
        // Largest single-role part count one member can carry at this energy — reuses the real builder
        // (incl. the per-member MOVE ratio + 50-part cap) so the cap can't drift from what actually
        // spawns. 0 ⇒ can't field even one member of this role at this energy.
        let cap_for = |role: SquadRole| -> u32 {
            (1..=MAX_SINGLE_ROLE_PARTS)
                .rev()
                .find(|&n| bodies::build_combat_body(&spec_for(role, n), bodies::MoveProfile::Plains, max_member_energy).is_some())
                .unwrap_or(0)
        };
        let template_count = |r: SquadRole| self.slots.iter().filter(|s| s.role == r).count() as u32;

        // Decide member count + per-member spec for each required role present in the template.
        let roles: [(SquadRole, u32); 3] = [
            (SquadRole::Healer, force.heal_parts),
            (SquadRole::Dismantler, force.dismantle_parts),
            (SquadRole::Tank, force.tough_parts),
        ];
        let mut sized_roles: Vec<(SquadRole, u32, bodies::CombatBodySpec)> = Vec::new();
        for (role, total) in roles {
            if total == 0 || template_count(role) == 0 {
                continue; // role not required by this force, or no slot to size → keep template
            }
            let cap = cap_for(role);
            if cap == 0 {
                return None; // can't field even one member of this role at this energy → defer
            }
            // Grow the member count so each member's even share fits; never below the template count.
            let count = total.div_ceil(cap).max(template_count(role));
            let per_member = total.div_ceil(count); // ceil ⇒ Σ over members ≥ total (never under-sizes)
            sized_roles.push((role, count, spec_for(role, per_member)));
        }

        // Total members = kept (non-sized-role) slots + the grown sized-role counts; bound the blob to
        // one squad (a bigger force is the multi-squad G4-HEAVY path, P5).
        let sized_set: Vec<SquadRole> = sized_roles.iter().map(|(r, _, _)| *r).collect();
        let kept = self.slots.iter().filter(|s| !sized_set.contains(&s.role)).count();
        let grown: usize = sized_roles.iter().map(|(_, n, _)| *n as usize).sum();
        if kept + grown > MAX_SIZED_MEMBERS {
            return None;
        }

        // Rebuild: size each role's existing slots in place (order-preserving), append the grown extras
        // by cloning the role's template slot.
        let mut sized = self.clone();
        for (role, count, spec) in &sized_roles {
            let mut placed = 0u32;
            for slot in sized.slots.iter_mut() {
                if slot.role == *role && placed < *count {
                    slot.body_type = BodyType::Sized(*spec);
                    placed += 1;
                }
            }
            let template = self.slots.iter().find(|s| s.role == *role).expect("required role present (guarded above)");
            while placed < *count {
                let mut slot = template.clone();
                slot.body_type = BodyType::Sized(*spec);
                sized.slots.push(slot);
                placed += 1;
            }
        }
        Some(sized)
    }
}

/// A composition's per-tick combat output + tank HP at a spawn energy — the force-sizing oracle's
/// `ForceBudget` inputs (ADR 0020 §12.2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SquadCapabilities {
    /// Total heal/tick the squad can sustain (Σ HEAL parts × `HEAL_POWER`).
    pub heal_per_tick: u32,
    /// Structure damage/tick (Σ WORK × `DISMANTLE_POWER` + ATTACK × `ATTACK_POWER` + RANGED_ATTACK ×
    /// `RANGED_ATTACK_POWER`) — breach + core-kill (cores are dismantle-immune, so ranged/melee is what kills them).
    pub structure_dps: u32,
    /// Effective HP of the toughest single member (the tank that soaks a tower drain).
    pub tank_effective_hp: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::military::force_sizing::RequiredForce;

    // ── R3: SquadComposition::sized_for (force-driven sizing) ──
    #[test]
    fn sized_for_distributes_required_force_across_roles() {
        // siege_quad = 2 Dismantler + 2 Healer. 20 heal + 12 dismantle parts, even split, fits RCL7.
        let sized = SquadComposition::siege_quad()
            .sized_for(RequiredForce { heal_parts: 20, dismantle_parts: 12, tough_parts: 0 }, 5600)
            .expect("affordable at RCL7");
        let dismantler = sized.slots.iter().find(|s| s.role == SquadRole::Dismantler).unwrap();
        assert!(
            matches!(dismantler.body_type, BodyType::Sized(spec) if spec.work == 6 && spec.heal == 0),
            "dismantler sized to 12/2 = 6 WORK"
        );
        let healer = sized.slots.iter().find(|s| s.role == SquadRole::Healer).unwrap();
        assert!(
            matches!(healer.body_type, BodyType::Sized(spec) if spec.heal == 10 && spec.work == 0),
            "healer sized to 20/2 = 10 HEAL"
        );
    }

    #[test]
    fn sized_for_defers_when_force_exceeds_one_squad() {
        // 200 heal parts at RCL4 (1300e ⇒ ≤4 HEAL/member) would need ~50 healer members — far past
        // MAX_SIZED_MEMBERS, so it defers to the multi-squad G4-HEAVY path rather than under-size.
        assert!(SquadComposition::siege_quad()
            .sized_for(RequiredForce { heal_parts: 200, dismantle_parts: 0, tough_parts: 0 }, 1300)
            .is_none());
    }

    #[test]
    fn sized_for_grows_member_count_when_template_count_is_insufficient() {
        // D3 (ADR 0022): a force needing more HEAL than the template's 2 healers can carry GROWS the
        // healer count instead of deferring. 65 heal parts at RCL7 (≤18 HEAL/member) ⇒ ceil(65/18)=4
        // healers, each ~17 HEAL — the squad is fielded (not deferred) and out-heals the requirement.
        let sized = SquadComposition::siege_quad()
            .sized_for(RequiredForce { heal_parts: 65, dismantle_parts: 12, tough_parts: 0 }, 5600)
            .expect("grows healers to meet the force at RCL7");
        let healers = sized.slots.iter().filter(|s| s.role == SquadRole::Healer).count();
        assert_eq!(healers, 4, "the 2-healer template grew to 4 to carry 65 HEAL parts");
        // The fielded force meets-or-exceeds the requirement (ceil distribution never under-sizes).
        assert!(sized.capabilities(5600).heal_per_tick >= 65 * 12, "fielded HEAL ≥ required (12 HEAL/part)");
        // Dismantlers stay at the template count (12 WORK fits 2 dismantlers at RCL7).
        assert_eq!(sized.slots.iter().filter(|s| s.role == SquadRole::Dismantler).count(), 2);
    }

    /// SK-setup scenario across keeper strengths (operator ask): an open SK-like room (keepers doing
    /// `keeper_dps`, no towers/walls). Run the end-to-end pipeline (assess → required force → size the
    /// squad). The composition we actually FIELD must out-heal the keepers WITH the hold margin — so it
    /// maintains/holds through damage and won't early-retreat — and when no single squad at this RCL can
    /// (the heal need exceeds the budget OR a member's 50-part cap), it must DEFER, never field an
    /// undersized squad that bails. Regression guard for the live SK failure that started P2b.
    #[test]
    fn sk_setup_fields_a_holding_composition_or_defers_never_undersizes() {
        use crate::military::force_sizing::{assess, DefenseProfile, ForceBudget, HOLD_MARGIN};

        // A strong RCL7-ish home's baseline budget (2 healers cap squad heal at ~600/tick, so very
        // strong keeper sets correctly exceed what one siege quad can field).
        let budget = ForceBudget {
            max_heal_per_tick: 900.0,
            max_dismantle_dps: 300.0,
            tank_effective_hp: 30_000.0,
            onsite_budget_ticks: 1400,
        };
        // The ACTUAL field decision is sized_for(): aggregate-winnable is necessary but not sufficient —
        // a member's 50-part AND per-member energy cost cap can still force a defer.
        let field = |keeper_dps: f32| -> Option<SquadComposition> {
            let a = assess(&DefenseProfile { enemy_dps: keeper_dps, ..Default::default() }, &budget);
            if a.winnable {
                SquadComposition::siege_quad().sized_for(RequiredForce::from_assessment(&a), 5600)
            } else {
                None
            }
        };

        // INVARIANT across keeper strengths: whatever we FIELD out-heals the keepers WITH the hold
        // margin → it maintains/holds through damage instead of early-retreating.
        for &keeper_dps in &[60.0f32, 180.0, 360.0, 600.0, 2000.0] {
            if let Some(comp) = field(keeper_dps) {
                assert!(
                    comp.capabilities(5600).heal_per_tick as f32 >= keeper_dps * HOLD_MARGIN,
                    "keeper dps {keeper_dps}: fielded composition must out-heal with the hold margin"
                );
            }
        }
        // Endpoints: a weak SK room IS fielded (the bot engages it); an overwhelming keeper set DEFERS
        // (no single siege quad can out-heal it at RCL7) rather than fielding an undersized squad.
        assert!(field(60.0).is_some(), "a weak SK room is fieldable");
        assert!(field(2000.0).is_none(), "an overwhelming keeper set defers, not an undersized squad");
    }

    /// R6: the SK suppression duo force-sizes its HEALER to out-heal a Source Keeper (168 melee DPS ×
    /// the hold margin) at a high-energy home, and defers (→ template fallback at the call site) when
    /// no home affords it. The ranged kiter always stays the proven template.
    #[test]
    fn sk_duo_sizes_healer_to_outheal_a_keeper() {
        use crate::military::force_sizing::HOLD_MARGIN;
        let required = RequiredForce {
            heal_parts: crate::military::damage::defender_heal_parts_for_dps(168.0 * HOLD_MARGIN, false),
            ..Default::default()
        };
        // RCL8 energy: the healer sizes up to out-heal the keeper with margin; the ranged kiter stays template.
        let sized = SquadComposition::duo_sk_farmer()
            .sized_for(required, 12_900)
            .expect("RCL8 affords the sized SK healer");
        let healer = sized.slots.iter().find(|s| s.role == SquadRole::Healer).unwrap();
        match healer.body_type {
            BodyType::Sized(spec) => assert!(
                spec.heal as f32 * 12.0 >= 168.0 * HOLD_MARGIN,
                "the sized SK healer out-heals a keeper with the hold margin"
            ),
            other => panic!("the SK healer should be force-sized, got {other:?}"),
        }
        let ranged = sized.slots.iter().find(|s| s.role == SquadRole::RangedDPS).unwrap();
        assert_eq!(ranged.body_type, BodyType::SkRangedAttacker, "the ranged kiter stays the proven template");
        // Very low energy (RCL2, ~1 HEAL/member) → the keeper-holding heal needs more members than one
        // squad can field → defer (the mission falls back to the template duo). (At RCL4+ D3 instead
        // GROWS the healer count rather than deferring — see sized_for_grows_member_count_*.)
        assert!(SquadComposition::duo_sk_farmer().sized_for(required, 550).is_none(), "RCL2 defers (force > one squad)");
    }

    /// The oracle's structure-DPS must count RANGED_ATTACK: invader cores are dismantle-immune, so a
    /// ranged comp is what kills them. Without this the force oracle reads `quad_ranged` as 0
    /// structure-DPS and defers every core as "breach too slow" (the soak regression).
    #[test]
    fn quad_ranged_deals_structure_damage_via_ranged() {
        let caps = SquadComposition::quad_ranged().capabilities(5600);
        assert!(
            caps.structure_dps > 0,
            "quad_ranged must contribute structure damage through RANGED_ATTACK (got {})",
            caps.structure_dps
        );
    }
}
