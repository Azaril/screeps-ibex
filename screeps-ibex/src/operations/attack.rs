use super::data::*;
use super::operationsystem::*;
use crate::missions::squad_assault::*;
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

/// Attack operation -- coordinates offensive campaigns against a target room.
/// Created via feature flag or manual trigger.
///
/// Supports multi-squad coordination: a primary assault squad is launched first,
/// and a secondary support squad can be launched once the primary is engaged.
#[derive(Clone, ConvertSaveload)]
pub struct AttackOperation {
    owner: EntityOption<Entity>,
    target_room: RoomName,
    last_run: Option<u32>,
    phase: AttackPhase,
    /// Entity of the primary assault mission.
    assault_mission: EntityOption<Entity>,
    /// Entity of the secondary support mission (multi-squad coordination).
    support_mission: EntityOption<Entity>,
    /// Desired squad size for the primary assault.
    squad_size: AssaultSquadSize,
    /// Tick when recon was last requested.
    recon_requested: Option<u32>,
    /// Number of towers detected during recon (used for multi-squad decisions).
    detected_towers: u32,
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

    pub fn new(owner: Option<Entity>, target_room: RoomName) -> AttackOperation {
        AttackOperation {
            owner: owner.into(),
            target_room,
            last_run: None,
            phase: AttackPhase::Recon,
            assault_mission: None.into(),
            support_mission: None.into(),
            squad_size: AssaultSquadSize::Solo,
            recon_requested: None,
            detected_towers: 0,
        }
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

    fn child_complete(&mut self, child: Entity) {
        if self.assault_mission.as_ref() == Some(&child) {
            self.assault_mission.take();
            // Primary mission completed -- if support is also done, move to exploit.
            if self.phase == AttackPhase::Execute && self.support_mission.is_none() {
                self.phase = AttackPhase::Exploit;
            }
        }
        if self.support_mission.as_ref() == Some(&child) {
            self.support_mission.take();
            // Support mission completed -- if primary is also done, move to exploit.
            if self.phase == AttackPhase::Execute && self.assault_mission.is_none() {
                self.phase = AttackPhase::Exploit;
            }
        }
    }

    fn describe_operation(&self, _ctx: &OperationDescribeContext) -> SummaryContent {
        SummaryContent::Text(format!("Attack({:?}) - {}", self.phase, self.target_room))
    }

    fn run_operation(
        &mut self,
        system_data: &mut OperationExecutionSystemData,
        runtime_data: &mut OperationExecutionRuntimeData,
    ) -> Result<OperationResult, ()> {
        let features = crate::features::features();

        if !features.military.attack {
            return Ok(OperationResult::Running);
        }

        let should_run = self.last_run.map(|t| game::time() - t >= 20).unwrap_or(true);
        if !should_run {
            return Ok(OperationResult::Running);
        }
        self.last_run = Some(game::time());

        match self.phase {
            AttackPhase::Recon => {
                // Request visibility of the target room.
                system_data.visibility.request(VisibilityRequest::new(
                    self.target_room,
                    VISIBILITY_PRIORITY_HIGH,
                    VisibilityRequestFlags::ALL,
                ));

                // Check if we have visibility data.
                if let Some(room_entity) = system_data.mapping.get_room(&self.target_room) {
                    if let Some(room_data) = system_data.room_data.get(room_entity) {
                        if let Some(dynamic_vis) = room_data.get_dynamic_visibility_data() {
                            if dynamic_vis.visible() {
                                // Analyze defenses to determine squad size.
                                let tower_count = room_data
                                    .get_structures()
                                    .map(|s| s.towers().iter().filter(|t| !t.my()).count())
                                    .unwrap_or(0);

                                let hostile_count = room_data.get_creeps().map(|c| c.hostile().len()).unwrap_or(0);

                                // Determine squad size based on defense analysis.
                                self.detected_towers = tower_count as u32;
                                self.squad_size = if tower_count >= 4 || hostile_count >= 4 {
                                    AssaultSquadSize::Quad
                                } else if tower_count >= 2 || hostile_count >= 2 {
                                    AssaultSquadSize::Duo
                                } else {
                                    AssaultSquadSize::Solo
                                };

                                info!(
                                    "Attack recon complete for {}: {} towers, {} hostiles -> {:?} squad",
                                    self.target_room, tower_count, hostile_count, self.squad_size
                                );

                                self.phase = AttackPhase::Prepare;
                            }
                        }
                    }
                }

                // Timeout: if recon takes too long, proceed with solo.
                if let Some(recon_tick) = self.recon_requested {
                    if game::time() - recon_tick > 100 {
                        info!("Attack recon timeout for {}, proceeding with solo", self.target_room);
                        self.phase = AttackPhase::Prepare;
                    }
                } else {
                    self.recon_requested = Some(game::time());
                }
            }
            AttackPhase::Prepare => {
                // For now, skip boost preparation and go straight to execute.
                // Boost integration will be added in Phase 5.
                self.phase = AttackPhase::Execute;
            }
            AttackPhase::Execute => {
                // Find home rooms for spawning.
                let home_rooms: Vec<Entity> = (system_data.entities, &*system_data.room_data)
                    .join()
                    .filter(|(_, rd)| {
                        rd.get_dynamic_visibility_data().map(|d| d.owner().mine()).unwrap_or(false)
                            && rd.get_structures().map(|s| !s.spawns().is_empty()).unwrap_or(false)
                    })
                    .map(|(e, _)| e)
                    .collect();

                // Create primary assault mission if not already running.
                let primary_alive = self
                    .assault_mission
                    .as_ref()
                    .map(|e| system_data.entities.is_alive(*e))
                    .unwrap_or(false);

                if !primary_alive {
                    if let Some(&anchor_room) = home_rooms.first() {
                        info!("Launching {:?} primary assault on {}", self.squad_size, self.target_room);

                        let mission_entity = SquadAssaultMission::build(
                            system_data.updater.create_entity(system_data.entities),
                            Some(runtime_data.entity),
                            anchor_room,
                            self.target_room,
                            &home_rooms,
                            self.squad_size,
                        )
                        .build();

                        if let Some(room_data) = system_data.room_data.get_mut(anchor_room) {
                            room_data.add_mission(mission_entity);
                        }

                        *self.assault_mission = Some(mission_entity);
                    }
                }

                // Multi-squad coordination: launch a secondary support squad for
                // heavily defended rooms (4+ towers or quad-level threat).
                // The support squad uses a smaller composition to flank or distract.
                if self.detected_towers >= 4 && self.squad_size == AssaultSquadSize::Quad {
                    let support_alive = self
                        .support_mission
                        .as_ref()
                        .map(|e| system_data.entities.is_alive(*e))
                        .unwrap_or(false);

                    if !support_alive && primary_alive {
                        // Only launch support after primary is deployed.
                        // Use a duo as the support squad to divide defender attention.
                        if let Some(&anchor_room) = home_rooms.last() {
                            info!("Launching Duo support squad on {} (multi-squad coordination)", self.target_room);

                            let support_entity = SquadAssaultMission::build(
                                system_data.updater.create_entity(system_data.entities),
                                Some(runtime_data.entity),
                                anchor_room,
                                self.target_room,
                                &home_rooms,
                                AssaultSquadSize::Duo,
                            )
                            .build();

                            if let Some(room_data) = system_data.room_data.get_mut(anchor_room) {
                                room_data.add_mission(support_entity);
                            }

                            *self.support_mission = Some(support_entity);
                        }
                    }
                }
            }
            AttackPhase::Exploit => {
                // Exploitation phase: could create RaidMission/DismantleMission here.
                // For now, mark complete.
                self.phase = AttackPhase::Complete;
            }
            AttackPhase::Complete => {
                return Ok(OperationResult::Success);
            }
        }

        Ok(OperationResult::Running)
    }
}
