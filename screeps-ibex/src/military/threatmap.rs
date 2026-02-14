use screeps::*;
use std::collections::HashMap;

/// Analyzed information about a single hostile creep.
#[derive(Clone, Debug)]
pub struct HostileCreepInfo {
    pub position: Position,
    pub owner: String,
    pub hits: u32,
    pub hits_max: u32,
    /// Melee damage per tick (ATTACK parts * 30).
    pub melee_dps: f32,
    /// Ranged damage per tick (RANGED_ATTACK parts * 10).
    pub ranged_dps: f32,
    /// Heal per tick (HEAL parts * 12 for adjacent, * 4 for ranged).
    pub heal_per_tick: f32,
    /// Total effective HP from TOUGH parts (accounting for boosts).
    pub tough_hp: f32,
    /// Number of WORK parts (relevant for dismantle damage).
    pub work_parts: u32,
    /// Whether any body part is boosted.
    pub boosted: bool,
}

/// Information about an incoming nuke.
#[derive(Clone, Debug)]
pub struct NukeInfo {
    /// Game tick when the nuke will land.
    pub landing_tick: u32,
    /// Position the nuke is targeted at.
    pub impact_position: Position,
}

/// Threat classification for a room, driving defense escalation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum ThreatLevel {
    /// No threats detected.
    #[default]
    None,
    /// NPC invaders only.
    Invader,
    /// Player scout (unarmed creep, no attack parts).
    PlayerScout,
    /// Player raid (armed creeps, limited force).
    PlayerRaid,
    /// Player siege (sustained attack with significant force).
    PlayerSiege,
    /// Incoming nuke detected.
    NukeIncoming,
}

/// Per-room threat intelligence, rebuilt each tick from visibility data.
#[derive(Clone, Debug, Default)]
pub struct RoomThreatData {
    pub threat_level: ThreatLevel,
    pub hostile_creeps: Vec<HostileCreepInfo>,
    pub hostile_tower_positions: Vec<Position>,
    pub incoming_nukes: Vec<NukeInfo>,
    /// Game tick when this data was last updated.
    pub last_seen: u32,
    /// Total hostile damage per tick across all hostiles.
    pub estimated_dps: f32,
    /// Total hostile healing per tick across all hostiles.
    pub estimated_heal: f32,
}

/// Global threat intelligence resource, keyed by room name.
/// Rebuilt each tick (ephemeral -- not serialized).
#[derive(Default)]
pub struct ThreatMap {
    rooms: HashMap<RoomName, RoomThreatData>,
}

impl ThreatMap {
    pub fn new() -> Self {
        ThreatMap { rooms: HashMap::new() }
    }

    pub fn clear(&mut self) {
        self.rooms.clear();
    }

    pub fn get(&self, room: &RoomName) -> Option<&RoomThreatData> {
        self.rooms.get(room)
    }

    pub fn get_threat_level(&self, room: &RoomName) -> ThreatLevel {
        self.rooms.get(room).map(|d| d.threat_level).unwrap_or(ThreatLevel::None)
    }

    pub fn insert(&mut self, room: RoomName, data: RoomThreatData) {
        self.rooms.insert(room, data);
    }

    pub fn rooms(&self) -> impl Iterator<Item = (&RoomName, &RoomThreatData)> {
        self.rooms.iter()
    }

    /// Return all rooms at or above the given threat level.
    pub fn rooms_at_threat(&self, min_level: ThreatLevel) -> Vec<(RoomName, &RoomThreatData)> {
        self.rooms
            .iter()
            .filter(|(_, d)| d.threat_level >= min_level)
            .map(|(name, d)| (*name, d))
            .collect()
    }
}

/// Analyze a hostile creep's body to produce a `HostileCreepInfo`.
pub fn analyze_hostile_creep(creep: &Creep) -> HostileCreepInfo {
    let body = creep.body();

    let mut melee_dps: f32 = 0.0;
    let mut ranged_dps: f32 = 0.0;
    let mut heal_per_tick: f32 = 0.0;
    let mut tough_hp: f32 = 0.0;
    let mut work_parts: u32 = 0;
    let mut boosted = false;

    for part_info in body.iter() {
        // Only count parts that still have HP.
        if part_info.hits() == 0 {
            continue;
        }

        let boost_multiplier = part_info
            .boost()
            .map(|_| {
                boosted = true;
                // Conservative estimate: assume T3 boosts for threat assessment.
                // Real boost detection would require checking the specific compound.
                4.0_f32
            })
            .unwrap_or(1.0);

        match part_info.part() {
            Part::Attack => {
                melee_dps += 30.0 * boost_multiplier;
            }
            Part::RangedAttack => {
                ranged_dps += 10.0 * boost_multiplier;
            }
            Part::Heal => {
                heal_per_tick += 12.0 * boost_multiplier;
            }
            Part::Tough => {
                // Boosted tough reduces damage taken. T3 XGHO2 = 70% reduction.
                // Effective HP of the tough part itself.
                if part_info.boost().is_some() {
                    tough_hp += 100.0 / 0.3; // ~333 effective HP per boosted tough
                } else {
                    tough_hp += 100.0;
                }
            }
            Part::Work => {
                work_parts += 1;
            }
            _ => {}
        }
    }

    let owner = creep.owner().username();

    HostileCreepInfo {
        position: creep.pos(),
        owner,
        hits: creep.hits(),
        hits_max: creep.hits_max(),
        melee_dps,
        ranged_dps,
        heal_per_tick,
        tough_hp,
        work_parts,
        boosted,
    }
}

/// Classify the threat level of a set of hostile creeps.
pub fn classify_threat(hostile_creeps: &[HostileCreepInfo], has_nukes: bool) -> ThreatLevel {
    if has_nukes {
        return ThreatLevel::NukeIncoming;
    }

    if hostile_creeps.is_empty() {
        return ThreatLevel::None;
    }

    let total_dps: f32 = hostile_creeps.iter().map(|c| c.melee_dps + c.ranged_dps).sum();
    let total_heal: f32 = hostile_creeps.iter().map(|c| c.heal_per_tick).sum();
    let any_boosted = hostile_creeps.iter().any(|c| c.boosted);
    let has_combat_parts = hostile_creeps
        .iter()
        .any(|c| c.melee_dps > 0.0 || c.ranged_dps > 0.0 || c.work_parts > 0);

    // Check if all hostiles are NPC invaders (username "Invader" or "Source Keeper").
    let all_npc = hostile_creeps.iter().all(|c| c.owner == "Invader" || c.owner == "Source Keeper");

    if all_npc {
        return ThreatLevel::Invader;
    }

    if !has_combat_parts {
        return ThreatLevel::PlayerScout;
    }

    // Siege: boosted attackers, or high sustained DPS with healing support.
    if any_boosted || (total_dps > 200.0 && total_heal > 100.0) || hostile_creeps.len() >= 4 {
        return ThreatLevel::PlayerSiege;
    }

    ThreatLevel::PlayerRaid
}

/// System that populates the ThreatMap each tick from visible room data.
///
/// This runs in the pre-pass dispatcher after room data is updated.
pub struct ThreatAssessmentSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> specs::System<'a> for ThreatAssessmentSystem {
    type SystemData = (specs::ReadStorage<'a, crate::room::data::RoomData>, specs::Write<'a, ThreatMap>);

    fn run(&mut self, (room_data_storage, mut threat_map): Self::SystemData) {
        threat_map.clear();

        use specs::Join;

        for room_data in room_data_storage.join() {
            let dynamic_vis = match room_data.get_dynamic_visibility_data() {
                Some(d) => d,
                None => continue,
            };

            // Only process rooms we can currently see.
            if !dynamic_vis.visible() {
                continue;
            }

            let mut hostile_creep_infos = Vec::new();
            let mut estimated_dps: f32 = 0.0;
            let mut estimated_heal: f32 = 0.0;

            if let Some(creeps) = room_data.get_creeps() {
                for hostile in creeps.hostile() {
                    let info = analyze_hostile_creep(hostile);
                    estimated_dps += info.melee_dps + info.ranged_dps;
                    estimated_heal += info.heal_per_tick;
                    hostile_creep_infos.push(info);
                }
            }

            // Gather hostile tower positions from structures.
            let mut hostile_tower_positions = Vec::new();
            if let Some(structures) = room_data.get_structures() {
                for tower in structures.towers() {
                    if !tower.my() {
                        hostile_tower_positions.push(tower.pos());
                    }
                }
            }

            // Detect incoming nukes via the game room object.
            let mut incoming_nukes = Vec::new();
            if let Some(room) = game::rooms().get(room_data.name) {
                let nukes = room.find(find::NUKES, None);
                for nuke in &nukes {
                    incoming_nukes.push(NukeInfo {
                        landing_tick: nuke.time_to_land() + game::time(),
                        impact_position: nuke.pos(),
                    });
                }
            }

            let has_nukes = !incoming_nukes.is_empty();
            let threat_level = classify_threat(&hostile_creep_infos, has_nukes);

            // Only store data if there is something interesting.
            if threat_level != ThreatLevel::None || !incoming_nukes.is_empty() {
                threat_map.insert(
                    room_data.name,
                    RoomThreatData {
                        threat_level,
                        hostile_creeps: hostile_creep_infos,
                        hostile_tower_positions,
                        incoming_nukes,
                        last_seen: game::time(),
                        estimated_dps,
                        estimated_heal,
                    },
                );
            }
        }
    }
}
