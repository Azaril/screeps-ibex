use super::data::*;
use super::operationsystem::*;
use crate::military::composition::*;
use crate::military::squad::*;
use crate::military::threatmap::RoomThreatData;
use crate::missions::attack_mission::*;
use crate::missions::data::*;
use crate::room::visibilitysystem::*;
use crate::serialize::*;
use crate::visualization::SummaryContent;
use log::*;
use screeps::*;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;

/// Phase of the attack campaign.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttackPhase {
    /// Scout target room, analyze defenses.
    #[default]
    Recon,
    /// Queue boost production, build squad.
    Prepare,
    /// Deploy squad(s), assault target.
    Execute,
    /// Raid/dismantle after defenses fall.
    Exploit,
    /// Campaign complete.
    Complete,
}

/// Why this attack was launched (from WarOperation target selection).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub enum AttackReason {
    /// Manual 'attack' flag.
    #[default]
    Flag,
    /// Player hostiles near our territory.
    ThreatResponse,
    /// Room we want to expand to.
    Expansion,
    /// Deny enemy remote mining.
    ResourceDenial,
    /// Invader core/stronghold.
    InvaderCore { level: u8 },
    /// Invader creeps in remote mining room.
    InvaderCreeps,
    /// Source Keeper room farming.
    SourceKeeper,
    /// Power bank farming.
    PowerBank { power: u32 },
    /// Proactive defense.
    ProactiveDefense,
}


/// Attack operation -- coordinates offensive campaigns against a target room.
/// Created by WarOperation for each target. Supports multi-squad coordination.
#[derive(Clone, ConvertSaveload)]
pub struct AttackOperation {
    owner: EntityOption<Entity>,
    target_room: RoomName,
    attack_reason: AttackReason,
    /// Home rooms assigned by WarOperation for spawning.
    assigned_home_rooms: EntityVec<Entity>,
    phase: AttackPhase,
    /// All child missions.
    missions: EntityVec<Entity>,
    /// Tick when recon was last requested.
    recon_requested: Option<u32>,
    last_run: Option<u32>,
    /// Number of towers detected during recon.
    detected_towers: u32,
    /// Detected enemy DPS from recon.
    detected_enemy_dps: f32,
    /// Detected enemy healing from recon.
    detected_enemy_heal: f32,
    /// Number of hostile creeps detected during recon.
    detected_hostile_count: u32,
    /// Whether enemy has safe mode available.
    detected_safe_mode: bool,
    /// Estimated total energy cost of the force plan.
    estimated_total_cost: u32,
    /// Actual energy spent so far.
    total_energy_invested: u32,
    /// How many squad waves sent.
    total_waves: u32,
    /// Give up after N failed waves.
    max_waves: u32,
    /// Whether any mission reported success (defenses cleared).
    /// Set during Execute phase by polling mission state.
    attack_succeeded: bool,
    /// Tick when the Prepare phase first started waiting for economy.
    /// Reset to None when the economy gate passes. Used to enforce a
    /// maximum patience window so we don't hold an attack slot forever.
    economy_wait_since: Option<u32>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl AttackOperation {
    pub fn build<B>(builder: B, owner: Option<Entity>, target_room: RoomName) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let operation = AttackOperation::new(owner, target_room);

        builder.with(OperationData::Attack(operation)).marked::<SerializeMarker>()
    }

    pub fn build_with_context<B>(
        builder: B,
        owner: Option<Entity>,
        target_room: RoomName,
        reason: AttackReason,
    ) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let operation = AttackOperation::new_with_context(owner, target_room, reason);

        builder.with(OperationData::Attack(operation)).marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>, target_room: RoomName) -> AttackOperation {
        AttackOperation {
            owner: owner.into(),
            target_room,
            attack_reason: AttackReason::Flag,
            assigned_home_rooms: EntityVec::new(),
            phase: AttackPhase::Recon,
            missions: EntityVec::new(),
            recon_requested: None,
            last_run: None,
            detected_towers: 0,
            detected_enemy_dps: 0.0,
            detected_enemy_heal: 0.0,
            detected_hostile_count: 0,
            detected_safe_mode: false,
            estimated_total_cost: 0,
            total_energy_invested: 0,
            total_waves: 0,
            max_waves: 3,
            attack_succeeded: false,
            economy_wait_since: None,
        }
    }

    /// Create with a specific attack reason. Home rooms are assigned lazily
    /// by `WarOperation::reassign_home_rooms` for global balance.
    pub fn new_with_context(
        owner: Option<Entity>,
        target_room: RoomName,
        reason: AttackReason,
    ) -> AttackOperation {
        let mut op = AttackOperation::new(owner, target_room);
        op.attack_reason = reason;
        op
    }

    /// Update the assigned home rooms for spawning.
    /// Called by WarOperation during periodic rebalancing.
    pub fn set_home_rooms(&mut self, rooms: EntityVec<Entity>) {
        self.assigned_home_rooms = rooms;
    }

    /// Build a force plan based on the attack reason and recon data.
    /// Returns a list of PlannedSquads for the AttackMission.
    fn build_force_plan(&self) -> Vec<PlannedSquad> {
        match &self.attack_reason {
            AttackReason::InvaderCore { .. } => {
                // Scale composition based on detected DPS/healing from recon,
                // not the core level. A level-1 core with strong invader
                // creeps needs the same response as any other high-DPS room.
                self.plan_by_detected_threat()
            }
            AttackReason::InvaderCreeps => {
                // Remote mining invader cleanup.
                vec![PlannedSquad {
                    composition: SquadComposition::solo_ranged(),
                    target: SquadTarget::DefendRoom {
                        room: self.target_room,
                    },
                    deploy_condition: DeployCondition::Immediate,
                }]
            }
            AttackReason::SourceKeeper => {
                // Source Keeper farming: ranged kiter + healer duo.
                vec![PlannedSquad {
                    composition: SquadComposition::duo_sk_farmer(),
                    target: SquadTarget::AttackRoom {
                        room: self.target_room,
                    },
                    deploy_condition: DeployCondition::Immediate,
                }]
            }
            AttackReason::PowerBank { .. } => {
                // Power bank farming: melee attacker + healer duo.
                vec![PlannedSquad {
                    composition: SquadComposition::duo_melee_heal(),
                    target: SquadTarget::AttackRoom {
                        room: self.target_room,
                    },
                    deploy_condition: DeployCondition::Immediate,
                }]
            }
            AttackReason::ResourceDenial => {
                // Harassment: cheap solo harasser.
                vec![PlannedSquad {
                    composition: SquadComposition::solo_harasser(),
                    target: SquadTarget::HarassRoom {
                        room: self.target_room,
                    },
                    deploy_condition: DeployCondition::Immediate,
                }]
            }
            _ => {
                // General attack: scale based on detected defenses.
                self.plan_by_detected_threat()
            }
        }
    }

    /// Select composition based on detected DPS, healing, and tower count.
    /// Used for invader cores, general attacks, and any other reason where
    /// the right response depends on what the room actually contains rather
    /// than a static label.
    fn plan_by_detected_threat(&self) -> Vec<PlannedSquad> {
        let total_dps = self.detected_enemy_dps;
        let total_heal = self.detected_enemy_heal;
        let towers = self.detected_towers;

        if towers >= 4 {
            // Heavy defense: drain + quad assault.
            vec![
                PlannedSquad {
                    composition: SquadComposition::duo_drain(),
                    target: SquadTarget::AttackRoom {
                        room: self.target_room,
                    },
                    deploy_condition: DeployCondition::Immediate,
                },
                PlannedSquad {
                    composition: SquadComposition::quad_ranged(),
                    target: SquadTarget::AttackRoom {
                        room: self.target_room,
                    },
                    deploy_condition: DeployCondition::AfterSquad {
                        index: 0,
                        state: SquadState::Engaged,
                    },
                },
            ]
        } else if towers >= 2 || total_dps > 200.0 || total_heal > 100.0 {
            // Significant defense: quad assault.
            vec![PlannedSquad {
                composition: SquadComposition::quad_ranged(),
                target: SquadTarget::AttackRoom {
                    room: self.target_room,
                },
                deploy_condition: DeployCondition::Immediate,
            }]
        } else if towers >= 1 || total_dps > 0.0 {
            // Light defense: duo.
            vec![PlannedSquad {
                composition: SquadComposition::duo_attack_heal(),
                target: SquadTarget::AttackRoom {
                    room: self.target_room,
                },
                deploy_condition: DeployCondition::Immediate,
            }]
        } else {
            // No defense detected: solo.
            vec![PlannedSquad {
                composition: SquadComposition::solo_ranged(),
                target: SquadTarget::AttackRoom {
                    room: self.target_room,
                },
                deploy_condition: DeployCondition::Immediate,
            }]
        }
    }

    /// Analyze target room from recon data. Called when visibility is first obtained.
    fn analyze_target(&mut self, room_data: &crate::room::data::RoomData) {
        let tower_count = room_data
            .get_structures()
            .map(|s| s.towers().iter().filter(|t| !t.my()).count())
            .unwrap_or(0);

        let hostile_count = room_data
            .get_creeps()
            .map(|c| c.hostile().len())
            .unwrap_or(0);

        // Analyze hostile DPS and healing.
        let mut estimated_dps: f32 = 0.0;
        let mut estimated_heal: f32 = 0.0;
        if let Some(creeps) = room_data.get_creeps() {
            for hostile in creeps.hostile().iter() {
                for part_info in hostile.body().iter() {
                    if part_info.hits() == 0 {
                        continue;
                    }
                    match part_info.part() {
                        Part::Attack => estimated_dps += 30.0,
                        Part::RangedAttack => estimated_dps += 10.0,
                        Part::Heal => estimated_heal += 12.0,
                        _ => {}
                    }
                }
            }
        }

        // Check for safe mode using cached structure data.
        let has_safe_mode = room_data
            .get_structures()
            .map(|s| {
                s.controllers()
                    .first()
                    .map(|c| c.safe_mode().unwrap_or(0) > 0 || c.safe_mode_available() > 0)
                    .unwrap_or(false)
            })
            .unwrap_or(false);

        // Add tower DPS estimate (600 DPS per tower at close range).
        estimated_dps += tower_count as f32 * 600.0;

        self.detected_towers = tower_count as u32;
        self.detected_hostile_count = hostile_count as u32;
        self.detected_enemy_dps = estimated_dps;
        self.detected_enemy_heal = estimated_heal;
        self.detected_safe_mode = has_safe_mode;

        info!(
            "[Attack] Recon complete for {}: towers={}, hostiles={}, dps={:.0}, heal={:.0}, safe_mode={}",
            room_data.name, tower_count, hostile_count, estimated_dps, estimated_heal, has_safe_mode
        );
    }

    /// Populate detection fields from persisted `RoomThreatData` when live
    /// visibility is unavailable. This allows the operation to proceed with
    /// recent-but-not-live intel rather than stalling in Recon.
    fn analyze_target_from_threat_data(&mut self, threat_data: &RoomThreatData) {
        self.detected_towers = threat_data.hostile_tower_positions.len() as u32;
        self.detected_hostile_count = threat_data.hostile_creeps.len() as u32;
        self.detected_enemy_dps = threat_data.estimated_dps + self.detected_towers as f32 * 600.0;
        self.detected_enemy_heal = threat_data.estimated_heal;
        self.detected_safe_mode = threat_data.safe_mode_active || threat_data.safe_mode_available;

        info!(
            "[Attack] Recon from threat data for {}: towers={}, hostiles={}, dps={:.0}, heal={:.0}, safe_mode={} (age={})",
            self.target_room,
            self.detected_towers,
            self.detected_hostile_count,
            self.detected_enemy_dps,
            self.detected_enemy_heal,
            self.detected_safe_mode,
            game::time().saturating_sub(threat_data.last_seen)
        );
    }

    /// Update threat estimates from current intel. Called by WarOperation
    /// when threat data changes for our target room.
    pub fn update_threat_intel(
        &mut self,
        towers: u32,
        enemy_dps: f32,
        enemy_heal: f32,
        hostile_count: u32,
        safe_mode_active: bool,
        safe_mode_available: bool,
    ) {
        self.detected_towers = towers;
        self.detected_enemy_dps = enemy_dps;
        self.detected_enemy_heal = enemy_heal;
        self.detected_hostile_count = hostile_count;
        self.detected_safe_mode = safe_mode_active || safe_mode_available;
    }

    /// Get the target room for this operation.
    pub fn target_room(&self) -> RoomName {
        self.target_room
    }

    /// Get the child mission entities.
    pub fn mission_entities(&self) -> &[Entity] {
        &self.missions
    }

    /// Maximum ticks the Prepare phase will wait for economy before cancelling.
    /// Cheap/urgent attacks get less patience; expensive campaigns get more.
    fn economy_patience(&self) -> u32 {
        match &self.attack_reason {
            // Invader creeps are time-sensitive (they'll despawn or finish
            // killing our miners). Short patience.
            AttackReason::InvaderCreeps => 100,
            // Invader cores have a timer but it's long. Moderate patience.
            AttackReason::InvaderCore { level } => {
                if *level == 0 { 200 } else { 500 }
            }
            // Resource denial / harassment -- not urgent, but don't wait forever.
            AttackReason::ResourceDenial => 300,
            // Power banks decay -- moderate patience.
            AttackReason::PowerBank { .. } => 400,
            // Manual flag -- player wants this, be patient.
            AttackReason::Flag => 1000,
            // Threat response -- moderately urgent.
            AttackReason::ThreatResponse | AttackReason::ProactiveDefense => 300,
            // Expansion / general -- patient.
            _ => 500,
        }
    }

    /// Check if this operation should abort (too many failed waves, economy collapsed).
    fn should_abort(&self, system_data: &OperationExecutionSystemData) -> bool {
        if self.total_waves >= self.max_waves {
            info!(
                "[Attack] Aborting attack on {} -- max waves ({}) reached",
                self.target_room, self.max_waves
            );
            return true;
        }

        // Abort if economy has deteriorated below the actual cost and we've
        // already invested energy (i.e. we started but can't continue).
        if self.total_energy_invested > 0 && self.estimated_total_cost > 0 {
            let home_rooms: Vec<Entity> = self.assigned_home_rooms.iter().copied().collect();
            let can_afford = if home_rooms.is_empty() {
                system_data.economy.can_afford_military(self.estimated_total_cost)
            } else {
                system_data
                    .economy
                    .can_rooms_afford_military(&home_rooms, self.estimated_total_cost)
            };
            if !can_afford {
                let surplus = system_data.economy.rooms_surplus(&home_rooms);
                info!(
                    "[Attack] Aborting attack on {} -- economy too weak to continue (need {}, surplus {})",
                    self.target_room, self.estimated_total_cost, surplus
                );
                return true;
            }
        }

        false
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Operation for AttackOperation {
    fn get_owner(&self) -> &Option<Entity> {
        &self.owner
    }

    fn owner_complete(&mut self, owner: Entity) {
        assert!(Some(owner) == *self.owner);
        self.owner.take();
    }

    fn repair_entity_refs(&mut self, is_valid: &dyn Fn(Entity) -> bool) {
        let before_missions = self.missions.len();
        self.missions.retain(|e| {
            let ok = is_valid(*e);
            if !ok {
                error!("INTEGRITY: dead mission entity {:?} removed from AttackOperation {}", e, self.target_room);
            }
            ok
        });
        if self.missions.len() < before_missions {
            warn!(
                "INTEGRITY: removed {} dead mission ref(s) from AttackOperation {}",
                before_missions - self.missions.len(),
                self.target_room
            );
        }

        let before_homes = self.assigned_home_rooms.len();
        self.assigned_home_rooms.retain(|e| {
            let ok = is_valid(*e);
            if !ok {
                error!("INTEGRITY: dead home room entity {:?} removed from AttackOperation {}", e, self.target_room);
            }
            ok
        });
        if self.assigned_home_rooms.len() < before_homes {
            warn!(
                "INTEGRITY: removed {} dead home room ref(s) from AttackOperation {}",
                before_homes - self.assigned_home_rooms.len(),
                self.target_room
            );
        }
    }

    fn child_complete(&mut self, child: Entity) {
        self.missions.retain(|e| *e != child);

        // If all missions are done during Execute, decide next phase based on outcome.
        if self.phase == AttackPhase::Execute && self.missions.is_empty() {
            if self.attack_succeeded {
                info!(
                    "[Attack] Missions complete for {} -- attack succeeded, moving to Exploit",
                    self.target_room
                );
                self.phase = AttackPhase::Exploit;
            } else {
                info!(
                    "[Attack] Missions complete for {} -- attack failed, skipping exploit",
                    self.target_room
                );
                self.phase = AttackPhase::Complete;
            }
        }
    }

    fn describe_operation(&self, ctx: &OperationDescribeContext) -> SummaryContent {
        let mut children = Vec::new();

        // Reason for the attack.
        let reason_label = match &self.attack_reason {
            AttackReason::Flag => "flag".to_string(),
            AttackReason::ThreatResponse => "threat".to_string(),
            AttackReason::Expansion => "expansion".to_string(),
            AttackReason::ResourceDenial => "denial".to_string(),
            AttackReason::InvaderCore { level } => format!("core L{}", level),
            AttackReason::InvaderCreeps => "invaders".to_string(),
            AttackReason::SourceKeeper => "SK".to_string(),
            AttackReason::PowerBank { power } => format!("power {}pw", power),
            AttackReason::ProactiveDefense => "proactive".to_string(),
        };
        children.push(SummaryContent::Text(format!("reason: {}", reason_label)));

        // Wave progress.
        children.push(SummaryContent::Text(format!(
            "waves: {}/{}",
            self.total_waves, self.max_waves
        )));

        // Assigned home rooms for spawning.
        if !self.assigned_home_rooms.is_empty() {
            let home_names: Vec<String> = self
                .assigned_home_rooms
                .iter()
                .filter_map(|e| ctx.room_data.get(*e))
                .map(|rd| rd.name.to_string())
                .collect();
            if !home_names.is_empty() {
                children.push(SummaryContent::Text(format!(
                    "spawn: {}",
                    home_names.join(", ")
                )));
            }
        }

        // Threat intel (only if non-zero).
        if self.detected_towers > 0
            || self.detected_enemy_dps > 0.0
            || self.detected_hostile_count > 0
        {
            children.push(SummaryContent::Text(format!(
                "threat: {}T {:.0}dps {:.0}heal",
                self.detected_towers,
                self.detected_enemy_dps,
                self.detected_enemy_heal
            )));
        }

        // Cost tracking.
        if self.estimated_total_cost > 0 || self.total_energy_invested > 0 {
            children.push(SummaryContent::Text(format!(
                "cost: {} est, {} spent",
                self.estimated_total_cost, self.total_energy_invested
            )));
        }

        // Economy wait status.
        if let Some(wait_since) = self.economy_wait_since {
            let elapsed = game::time().saturating_sub(wait_since);
            let patience = self.economy_patience();
            children.push(SummaryContent::Text(format!(
                "econ wait: {}/{} ticks",
                elapsed, patience
            )));
        }

        // Missions count.
        let alive_missions = self.missions.len();
        if alive_missions > 0 {
            children.push(SummaryContent::Text(format!("missions: {}", alive_missions)));
        }

        SummaryContent::Tree {
            label: format!("Attack {} ({:?})", self.target_room, self.phase),
            children,
        }
    }

    fn run_operation(
        &mut self,
        system_data: &mut OperationExecutionSystemData,
        runtime_data: &mut OperationExecutionRuntimeData,
    ) -> Result<OperationResult, ()> {
        let features = crate::features::features();

        if !features.military.offense {
            return Ok(OperationResult::Running);
        }

        // Recon runs every tick (cheap: just requests visibility and checks data).
        // Prepare and Execute use the 20-tick cadence since they involve heavier work.
        let is_recon = self.phase == AttackPhase::Recon;
        let should_run = is_recon || self.last_run.map(|t| game::time() - t >= 20).unwrap_or(true);
        if !should_run {
            return Ok(OperationResult::Running);
        }
        self.last_run = Some(game::time());

        // Check abort conditions.
        if self.should_abort(system_data) {
            return Ok(OperationResult::Success);
        }

        // Clean up dead missions.
        self.missions.retain(|e| system_data.entities.is_alive(*e));

        // Phase loop: allow same-tick transitions (e.g. Recon → Prepare → Execute)
        // to avoid wasting 20+ ticks per phase boundary.
        loop {
            match self.phase {
                AttackPhase::Recon => {
                    // Request visibility of the target room.
                    system_data.visibility.request(VisibilityRequest::new(
                        self.target_room,
                        VISIBILITY_PRIORITY_HIGH,
                        VisibilityRequestFlags::ALL,
                    ));

                    let room_entity = system_data.mapping.get_room(&self.target_room);

                    // Prefer live visibility for the most accurate intel.
                    let have_live_intel = room_entity
                        .and_then(|e| system_data.room_data.get(e))
                        .and_then(|rd| rd.get_dynamic_visibility_data())
                        .map(|d| d.visible())
                        .unwrap_or(false);

                    if have_live_intel {
                        let room_data = system_data.room_data.get(room_entity.unwrap()).unwrap();
                        self.analyze_target(room_data);
                        self.phase = AttackPhase::Prepare;
                        // Fall through to Prepare in the same tick.
                        continue;
                    } else {
                        // Fall back to persisted RoomThreatData if available and
                        // recent (< 200 ticks old). This covers the case where the
                        // scout that provided initial visibility has died.
                        let threat_data = room_entity.and_then(|e| system_data.threat_data.get(e));
                        let recent_threat = threat_data.filter(|td| {
                            game::time().saturating_sub(td.last_seen) < 200
                        });

                        if let Some(td) = recent_threat {
                            self.analyze_target_from_threat_data(td);
                            self.phase = AttackPhase::Prepare;
                            // Fall through to Prepare in the same tick.
                            continue;
                        }
                    }

                    // Recon timeout handling: don't blindly attack.
                    if let Some(recon_tick) = self.recon_requested {
                        if game::time() - recon_tick > 200 {
                            // For manual flags, keep waiting -- the player wants this.
                            // For automated targets, abort if we can't get intel.
                            match &self.attack_reason {
                                AttackReason::Flag => {
                                    info!(
                                        "[Attack] Recon timeout for {} (flag target), continuing to wait",
                                        self.target_room
                                    );
                                    // Reset timer to avoid spamming.
                                    self.recon_requested = Some(game::time());
                                }
                                _ => {
                                    info!(
                                        "[Attack] Recon timeout for {} ({:?}), aborting -- no intel",
                                        self.target_room, self.attack_reason
                                    );
                                    return Ok(OperationResult::Success);
                                }
                            }
                        }
                    } else {
                        self.recon_requested = Some(game::time());
                    }

                    break;
                }
                AttackPhase::Prepare => {
                    // Refresh safe mode status from threat data if available.
                    let room_entity = system_data.mapping.get_room(&self.target_room);
                    if let Some(td) = room_entity.and_then(|e| system_data.threat_data.get(e)) {
                        self.detected_safe_mode = td.safe_mode_active || td.safe_mode_available;
                    }

                    // If safe mode is currently active, delay the attack -- our
                    // creeps would be killed instantly. Wait for it to expire.
                    if let Some(td) = room_entity.and_then(|e| system_data.threat_data.get(e)) {
                        if td.safe_mode_active {
                            info!(
                                "[Attack] Safe mode active on {} -- delaying attack",
                                self.target_room
                            );
                            break;
                        }
                    }

                    // Build force plan and estimate cost.
                    let force_plan = self.build_force_plan();
                    let spawn_capacity = self
                        .assigned_home_rooms
                        .iter()
                        .filter_map(|e| system_data.economy.room(e))
                        .map(|r| r.spawn_energy_capacity)
                        .max()
                        .unwrap_or(0);
                    let estimated_cost: u32 = force_plan
                        .iter()
                        .map(|p| p.composition.estimated_cost(spawn_capacity))
                        .sum();
                    self.estimated_total_cost = estimated_cost;

                    // Economy gate: check whether assigned home rooms can
                    // collectively fund the attack from their surplus energy.
                    let home_rooms: Vec<Entity> = self.assigned_home_rooms.iter().copied().collect();
                    let can_afford = if home_rooms.is_empty() {
                        // No home rooms assigned yet -- fall back to global check.
                        system_data.economy.can_afford_military(estimated_cost)
                    } else {
                        system_data.economy.can_rooms_afford_military(&home_rooms, estimated_cost)
                    };

                    if !can_afford {
                        let waited = self.economy_wait_since.get_or_insert(game::time());
                        let elapsed = game::time().saturating_sub(*waited);
                        let patience = self.economy_patience();

                        if elapsed >= patience {
                            let surplus = system_data.economy.rooms_surplus(&home_rooms);
                            info!(
                                "[Attack] Cancelling attack on {} -- economy wait expired \
                                 (waited {} ticks, need {}, surplus {})",
                                self.target_room, elapsed, estimated_cost, surplus
                            );
                            return Ok(OperationResult::Success);
                        }

                        // Log periodically (every ~100 ticks) to avoid spam.
                        if elapsed % 100 < 20 {
                            let surplus = system_data.economy.rooms_surplus(&home_rooms);
                            info!(
                                "[Attack] Economy gate: cannot afford {} for {} \
                                 (waiting {}/{} ticks, surplus {} from {} rooms)",
                                estimated_cost, self.target_room, elapsed, patience,
                                surplus, home_rooms.len()
                            );
                        }
                        break;
                    }

                    // Economy is sufficient -- clear the wait timer and proceed.
                    self.economy_wait_since = None;

                    self.phase = AttackPhase::Execute;
                    // Fall through to Execute in the same tick.
                    continue;
                }
                AttackPhase::Execute => {
                    // Poll active missions for success signal.
                    for mission_entity in self.missions.iter() {
                        if let Some(mission_data) = system_data.mission_data.get(*mission_entity) {
                            if let Some(attack_mission) = mission_data.as_mission_type::<AttackMission>() {
                                if attack_mission.mission_succeeded() {
                                    self.attack_succeeded = true;
                                }
                            }
                        }
                    }

                    // Only use explicitly assigned home rooms for spawning.
                    if self.assigned_home_rooms.is_empty() {
                        info!(
                            "[Attack] No home rooms assigned for {} -- waiting for rebalance",
                            self.target_room
                        );
                        break;
                    }
                    let home_rooms: Vec<Entity> = self.assigned_home_rooms.iter().copied().collect();

                    // Create primary mission if none are running.
                    if self.missions.is_empty() {
                        self.total_waves += 1;

                        let force_plan = self.build_force_plan();

                        info!(
                            "[Attack] Launching AttackMission on {} with {} squads, {} home rooms (wave {}, est. cost {})",
                            self.target_room,
                            force_plan.len(),
                            home_rooms.len(),
                            self.total_waves,
                            self.estimated_total_cost
                        );

                        let mission_entity = AttackMission::build(
                            system_data.updater.create_entity(system_data.entities),
                            Some(runtime_data.entity),
                            self.target_room,
                            force_plan,
                            self.max_waves,
                        )
                        .build();

                        // Push current home rooms to the mission via LazyUpdate.
                        // The mission starts with empty home rooms; this ensures
                        // they are set from the same source as reassign_home_rooms.
                        let rooms_for_mission = self.assigned_home_rooms.clone();
                        system_data.updater.exec_mut(move |world| {
                            if let Some(MissionData::AttackMission(ref cell)) =
                                world.read_storage::<MissionData>().get(mission_entity)
                            {
                                cell.borrow_mut().set_home_rooms(rooms_for_mission);
                            }
                        });

                        // Attach the mission to the target room.
                        if let Some(target_entity) = system_data.mapping.get_room(&self.target_room) {
                            if let Some(room_data) = system_data.room_data.get_mut(target_entity) {
                                room_data.add_mission(mission_entity);
                            }
                        }

                        self.missions.push(mission_entity);
                    }

                    break;
                }
                AttackPhase::Exploit => {
                    // The exploit phase is handled by the AttackMission's Exploiting state.
                    // The mission spawns haulers and manages resource collection.
                    // We just wait for the mission to complete.
                    // If all missions have already completed (via child_complete), move on.
                    if self.missions.is_empty() {
                        self.phase = AttackPhase::Complete;
                        continue;
                    }
                    break;
                }
                AttackPhase::Complete => {
                    return Ok(OperationResult::Success);
                }
            }
        }

        Ok(OperationResult::Running)
    }
}
