//! Persistent Source Keeper farm mission (ADR 0018 §3.3, P2.K2c).
//!
//! Owns a `duo_sk_farmer` squad that suppresses the keepers of one SK room so
//! K3 mining can harvest around them. UNLIKE [`AttackMission`](super::attack_mission)
//! (one-shot — it completes and deletes itself once the target is clear), this
//! mission runs **indefinitely**: keepers respawn every 300t, so suppression is
//! a standing commitment with per-creep TTL renewal and no completion-on-clear.
//! It is created/retired by `SourceKeeperOperation` per the ROI decision.
//!
//! The mission is a **thin coordinator** (reconciliation §2.0 (ad)): it owns no
//! squad. Each viable tick it (1) **requests** a low-priority `Farm{sk}` objective
//! on the [`CombatObjectiveQueue`](crate::military::objective_queue) — the
//! `SquadManager` fields the `duo_sk_farmer` that suppresses the keepers — and (2)
//! owns the **K3 mining**: a per-source [`SourceMiningMission`] child + a long-haul
//! [`HaulMission`] child, each gated on a **per-source suppression signal** (no live
//! keeper near that source). The duo creates the dead-keeper windows; mining
//! exploits them per source. Miners self-protect via the K0 `Flee` reflex, so a
//! keeper that reappears costs a flee, not a death.

use super::data::*;
use super::haul::HaulMission;
use super::localsupply::source_mining::SourceMiningMission;
use super::missionsystem::*;
use super::utility::*;
use crate::military::composition::SquadComposition;
use crate::military::objective_queue::*;
use crate::remoteobjectid::*;
use crate::serialize::*;
use screeps::*;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;

/// A live Source Keeper within this range (tiles) of a source makes it unsafe to
/// mine — a miner would just flee (K0). Matches the keeper spawn radius / the K0
/// `THREAT_FLEE_RANGE`.
const KEEPER_DANGER_RANGE: u32 = 5;

#[derive(Clone, ConvertSaveload)]
pub struct SourceKeeperFarmMission {
    owner: EntityOption<Entity>,
    /// The SK room being farmed.
    sk_room_data: Entity,
    /// Home rooms supplying the suppression duo (+ K3 miners/haulers).
    home_room_datas: EntityVec<Entity>,
    /// K3: one `SourceMiningMission` per source in the SK room.
    source_mining_missions: EntityVec<Entity>,
    /// K3: the long-haul-home child for the SK room.
    haul_mission: EntityOption<Entity>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl SourceKeeperFarmMission {
    pub fn build<B>(builder: B, owner: Option<Entity>, sk_room_data: Entity, home_room_datas: &[Entity]) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = SourceKeeperFarmMission::new(owner, sk_room_data, home_room_datas);

        builder
            .with(MissionData::SourceKeeperFarm(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>, sk_room_data: Entity, home_room_datas: &[Entity]) -> SourceKeeperFarmMission {
        SourceKeeperFarmMission {
            owner: owner.into(),
            sk_room_data,
            home_room_datas: home_room_datas.to_owned().into(),
            source_mining_missions: EntityVec::new(),
            haul_mission: None.into(),
        }
    }

    /// Ensure a `SourceMiningMission` exists per source in the SK room plus one
    /// `HaulMission` (long-haul home). Mirrors `LocalSupplyMission::ensure_children`
    /// / `MiningOutpostMission`'s child creation, but per-source-gated below.
    fn ensure_mining_children(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<(), String> {
        let (room_name, sources): (screeps::RoomName, Vec<RemoteObjectId<Source>>) = {
            let room_data = system_data.room_data.get(self.sk_room_data).ok_or("Expected SK room data")?;
            match room_data.get_static_visibility_data() {
                // Not yet scouted; the mining child requests visibility when it exists.
                Some(svd) => (room_data.name, svd.sources().clone()),
                None => return Ok(()),
            }
        };

        for source_id in sources.iter() {
            let already_exists = self.source_mining_missions.iter().any(|&e| {
                system_data
                    .missions
                    .get(e)
                    .as_mission_type::<SourceMiningMission>()
                    .map(|m| *m.source() == *source_id)
                    .unwrap_or(false)
            });
            if already_exists {
                continue;
            }
            let child = SourceMiningMission::build(
                system_data.updater.create_entity(system_data.entities),
                Some(mission_entity),
                self.sk_room_data,
                &self.home_room_datas,
                *source_id,
                room_name,
            )
            .build();
            if let Some(rd) = system_data.room_data.get_mut(self.sk_room_data) {
                rd.add_mission(child);
            }
            self.source_mining_missions.push(child);
        }

        if self.haul_mission.is_none() {
            let child = HaulMission::build(
                system_data.updater.create_entity(system_data.entities),
                Some(mission_entity),
                self.sk_room_data,
                &self.home_room_datas,
            )
            .build();
            if let Some(rd) = system_data.room_data.get_mut(self.sk_room_data) {
                rd.add_mission(child);
            }
            self.haul_mission = Some(child).into();
        }

        Ok(())
    }

    /// Gate each mining child on its per-source suppression signal: a source is
    /// minable iff the room is visible and no live Source Keeper is within
    /// `KEEPER_DANGER_RANGE` of it (the duo creates these dead-keeper windows). The
    /// haul child runs whenever any source is minable. (The signal is *data* the
    /// child reads — the miner job still owns its movement; ADR 0008 §5 / principle 8.)
    fn gate_mining_children(&self, system_data: &mut MissionExecutionSystemData) {
        let (has_visibility, keepers): (bool, Vec<Position>) = match system_data.room_data.get(self.sk_room_data) {
            Some(rd) => {
                let visible = rd.get_dynamic_visibility_data().map(|v| v.visible()).unwrap_or(false);
                let keepers = rd
                    .get_creeps()
                    .map(|creeps| {
                        creeps
                            .hostile()
                            .iter()
                            .filter(|c| crate::military::is_source_keeper_owner(&c.owner().username()))
                            .map(|c| c.pos())
                            .collect()
                    })
                    .unwrap_or_default();
                (visible, keepers)
            }
            None => return,
        };

        let source_suppressed = |source: &RemoteObjectId<Source>| -> bool {
            let pos = source.pos();
            has_visibility && !keepers.iter().any(|kp| kp.get_range_to(pos) <= KEEPER_DANGER_RANGE)
        };

        let mut any_suppressed = false;
        for &child in self.source_mining_missions.iter() {
            let source = match system_data.missions.get(child).as_mission_type::<SourceMiningMission>().map(|m| *m.source()) {
                Some(s) => s,
                None => continue,
            };
            let suppressed = source_suppressed(&source);
            any_suppressed |= suppressed;
            if let Some(mut child_mission) = system_data.missions.get(child).as_mission_type_mut::<SourceMiningMission>() {
                child_mission.set_home_rooms(&self.home_room_datas);
                child_mission.allow_spawning(suppressed);
            }
        }

        if let Some(haul) = *self.haul_mission {
            if let Some(mut haul_mission) = system_data.missions.get(haul).as_mission_type_mut::<HaulMission>() {
                haul_mission.allow_spawning(any_suppressed);
            }
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for SourceKeeperFarmMission {
    fn get_owner(&self) -> &Option<Entity> {
        &self.owner
    }

    fn owner_complete(&mut self, owner: Entity) {
        assert!(Some(owner) == *self.owner);

        self.owner.take();
    }

    fn get_room(&self) -> Option<Entity> {
        Some(self.sk_room_data)
    }

    fn describe_state(&self, _system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> String {
        "Source Keeper Farm".to_string()
    }

    fn summarize(&self) -> crate::visualization::SummaryContent {
        crate::visualization::SummaryContent::Text("SK Farm".to_string())
    }

    fn pre_run_mission(&mut self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<(), String> {
        // Drop home rooms that no longer qualify; the operation re-creates the
        // mission with fresh homes if it still wants the farm.
        self.home_room_datas
            .retain(|entity| system_data.room_data.get(*entity).map(is_valid_home_room).unwrap_or(false));

        if self.home_room_datas.is_empty() {
            return Err("No home rooms for source keeper farm".to_owned());
        }

        Ok(())
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<MissionResult, String> {
        // Self-cancel when PERMANENTLY no longer viable (ADR 0018 §3.5). The
        // operation cannot retire us (its `mission_data` is read-only), so we
        // release the farm (→ cleanup) ourselves. `Success` here is "withdrawn",
        // not "objective achieved" — a keeper farm has no completion.
        //
        //   • feature kill-switch OFF — stops EXISTING farms, not just new ones;
        //   • the room is contested — a player owns or reserves it (no point
        //     farming a room another player took).
        //
        // TRANSIENT pressure (CPU-critical, war preempting the bodies) must PAUSE
        // the squad, never cancel the farm — so it is deliberately NOT checked
        // here. (Lost home rooms self-cancel via `pre_run_mission`. The ROI /
        // out-of-haul-range withdrawal + the squad spawn/renew + teardown land
        // with K2c-2.)
        if !system_data.features.source_keeper.farming {
            return Ok(MissionResult::Success);
        }

        let sk_room = match system_data.room_data.get(self.sk_room_data) {
            Some(room_data) => {
                if let Some(dynamic) = room_data.get_dynamic_visibility_data() {
                    if dynamic.owner().hostile() || dynamic.reservation().hostile() {
                        return Ok(MissionResult::Success);
                    }
                }
                room_data.name
            }
            None => return Ok(MissionResult::Success),
        };

        // Coordinator role (ADR 0018 §3.3, reconciliation §2.0 (ad)): the mission
        // does NOT own a squad. It REQUESTS a low-priority, preemptible `Farm{sk}`
        // objective each tick it is viable; the `SquadManager` claims it, fields the
        // `duo_sk_farmer`, and retires the squad when this upsert stops (feature off
        // / contested / lost homes → self-cancel above → the objective TTL-lapses).
        // The squad's `SquadCombatJob` self-drives to the SK room and suppresses the
        // keepers (job-owns-movement, ADR 0008 §5 ⚑). K3 source mining + the
        // per-source suppression signal remain the coordinator's to own.
        let request = ObjectiveRequest::new(
            ObjectiveKind::Farm {
                kind: FarmKind::SourceKeeper,
                room: sk_room,
            },
            OBJECTIVE_PRIORITY_LOW,
            ForceRequirement::single(SquadComposition::duo_sk_farmer()),
        )
        .owner(ObjectiveOwner::SourceKeeper);
        system_data.combat_objective_queue.request(request, game::time());

        // K3: own per-source mining + hauling, gated on per-source keeper liveness.
        self.ensure_mining_children(system_data, mission_entity)?;
        self.gate_mining_children(system_data);

        Ok(MissionResult::Running)
    }

    fn get_children(&self) -> Vec<Entity> {
        let mut children: Vec<Entity> = self.source_mining_missions.iter().copied().collect();
        if let Some(haul) = *self.haul_mission {
            children.push(haul);
        }
        children
    }

    fn child_complete(&mut self, child: Entity) {
        self.source_mining_missions.retain(|&e| e != child);
        if self.haul_mission.map(|e| e == child).unwrap_or(false) {
            self.haul_mission.take();
        }
    }
}
