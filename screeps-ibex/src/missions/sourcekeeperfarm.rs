//! Persistent Source Keeper farm mission (ADR 0018 §3.3, P2.K2c).
//!
//! Owns a `duo_sk_farmer` squad that suppresses the keepers of one SK room so
//! K3 mining can harvest around them. UNLIKE [`AttackMission`](super::attack_mission)
//! (one-shot — it completes and deletes itself once the target is clear), this
//! mission runs **indefinitely**: keepers respawn every 300t, so suppression is
//! a standing commitment with per-creep TTL renewal and no completion-on-clear.
//! It is created/retired by `SourceKeeperOperation` per the ROI decision.
//!
//! **P2.K2c-1 (this increment)** is the mission + lifecycle skeleton: the type,
//! the `MissionData` wiring, and the operation's create hook. The squad
//! spawn/renew (reusing the `AttackMission` squad machinery) + the T-NPC-2 kite
//! + the per-source suppression signal land in the next increment.

use super::data::*;
use super::missionsystem::*;
use super::utility::*;
use crate::serialize::*;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;

#[derive(Clone, ConvertSaveload)]
pub struct SourceKeeperFarmMission {
    owner: EntityOption<Entity>,
    /// The SK room being farmed.
    sk_room_data: Entity,
    /// Home rooms supplying the suppression duo (+ K3 miners).
    home_room_datas: EntityVec<Entity>,
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

    fn run_mission(&mut self, _system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<MissionResult, String> {
        // P2.K2c-2 TODO: spawn + renew the `duo_sk_farmer` squad (a `SquadContext`
        // + per-member `SquadCombatJob` targeting the SK room — reusing the
        // AttackMission spawn/renew machinery), run the T-NPC-2 kite, and publish
        // the per-source suppression signal K3 mining reads. Persistent: never
        // returns Success on its own — withdrawn by the operation (Withhold/Veto)
        // or when it loses all home rooms (above).
        Ok(MissionResult::Running)
    }
}
