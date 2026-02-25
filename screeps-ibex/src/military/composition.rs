use super::bodies;
use super::squad::SquadRole;
use crate::creep::SpawnBodyDefinition;
use crate::military::economy::RoomRouteCache;
use screeps::*;
use serde::{Deserialize, Serialize};

/// Screeps constants for lifetime calculations.
const CREEP_LIFE_TIME: u32 = 1500;
const CREEP_SPAWN_TIME: u32 = 3;

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
        }
    }

    /// Estimate the body cost at a given energy capacity.
    pub fn estimated_cost(&self, max_energy: u32) -> u32 {
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
    /// Uses RoomRouteCache for accurate room-hop distance via find_route().
    /// Returns None if the rooms are unreachable.
    pub fn estimated_travel_time(route_cache: &mut RoomRouteCache, home: RoomName, target: RoomName) -> Option<u32> {
        route_cache.travel_ticks(home, target, screeps::game::time())
    }

    /// Estimate useful combat time for this composition when spawned from a
    /// given home room targeting a given room. Accounts for spawn time, travel
    /// time, and CREEP_LIFE_TIME. Returns None if unreachable.
    pub fn estimated_combat_time(
        &self,
        route_cache: &mut RoomRouteCache,
        home: RoomName,
        target: RoomName,
        energy_capacity: u32,
        available_spawns: u32,
    ) -> Option<u32> {
        let spawn_time = self.estimated_spawn_time(energy_capacity, available_spawns);
        let travel_time = Self::estimated_travel_time(route_cache, home, target)?;
        Some(CREEP_LIFE_TIME.saturating_sub(spawn_time + travel_time))
    }

    /// Check if spawning from this home room gives enough combat time to be
    /// worthwhile. Returns false if unreachable or creeps would arrive with
    /// <40% lifetime.
    pub fn is_viable_from(
        &self,
        route_cache: &mut RoomRouteCache,
        home: RoomName,
        target: RoomName,
        energy_capacity: u32,
        available_spawns: u32,
    ) -> bool {
        match self.estimated_combat_time(route_cache, home, target, energy_capacity, available_spawns) {
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
}
