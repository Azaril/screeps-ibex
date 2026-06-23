//! Persistent Source Keeper farm mission (ADR 0018 §3.3, P2.K2c).
//!
//! Owns a `duo_sk_farmer` squad that suppresses the keepers of one SK room so
//! K3 mining can harvest around them. This mission requests a low-priority
//! `Farm{SourceKeeper}` objective (the `SquadManager` fields the duo); it runs
//! **indefinitely**: keepers respawn every 300t, so suppression is
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
use crate::room::data::RoomData;
use crate::room::visibilitysystem::*;
use crate::serialize::*;
use log::*;
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

/// A Source Keeper deals **168 melee DPS** if it catches the kiter (see
/// [`crate::military::bodies::sk_healer_body`]). The suppression duo kites at range 3
/// to avoid this, but R6 (ADR 0020 §12.6) force-sizes the healer to OUT-HEAL it
/// (×`HOLD_MARGIN`) so a kiting slip — cornered, swamp, or a re-pathing keeper —
/// costs HP it recovers, not a death, instead of relying on the body template
/// happening to reach `maximum_repeat` at a high-energy home.
const SK_KEEPER_MELEE_DPS: f32 = 168.0;

/// While a stronghold pauses the farm the room goes blind (no friendly creeps),
/// so the persisted `hostile_structures` flag sticks at its last-observed value
/// and the farm cannot learn the stronghold has cleared without an active probe.
/// An observer refreshes it for free, but SK farming starts at RCL 6 while
/// observers are RCL 8 — so once per ~creep-lifetime we dispatch one cheap scout
/// to peek the room (a single tick of visibility on entry is enough to re-read
/// the flag). Throttled so we never feed a scout to the towers every tick; the
/// stronghold itself lasts ~75k ticks, so a probe every 1500t resumes promptly
/// enough. (A *non-opportunistic* request can't be paired with an every-tick
/// opportunistic one — the queue upsert makes the non-opportunistic flag sticky.)
const STRONGHOLD_RESCOUT_INTERVAL: u32 = 1500;

/// True when the SK room holds — or last held, while out of view — a dangerous
/// **invader stronghold** (a level≥1 [`StructureInvaderCore`]). A deployed
/// stronghold rings its core with ramparts and 1–6 towers that reach the
/// *entire* room (≥150 dmg/tick each, up to 6×600 at L5) and spawns defender
/// creeps, so the whole room is lethal: the farm must stand down rather than
/// feed the suppression duo or any miner into tower fire (ADR 0018 §3.5).
///
/// Detection mirrors the keeper read, but for structures:
/// * **Visible** — read the live cores directly and ignore harmless level-0
///   highway placeholders (they never deploy or defend). A core is treated as
///   dangerous from the moment it appears: the deploy window itself has no fire,
///   but it flips to lethal the instant `ticks_to_deploy` expires, so we stand
///   down on sight rather than risk being caught mid-deploy.
/// * **Out of view** — `invader_cores()` is live-only (`#[serde(skip)]`), so
///   fall back to the persisted `hostile_structures` flag. In an unclaimable SK
///   room the only owned, non-keeper-lair structures are a stronghold, so this
///   is a reliable last-observed signal that keeps the farm paused until we
///   regain visibility and confirm the stronghold is gone (it auto-collapses
///   after ~75k ticks, or we / another player clear it).
pub fn sk_room_has_stronghold(room_data: &RoomData) -> bool {
    if let Some(structures) = room_data.get_structures() {
        return structures.invader_cores().iter().any(|core| core.level() >= 1);
    }
    room_data
        .get_dynamic_visibility_data()
        .map(|dynamic| dynamic.hostile_structures())
        .unwrap_or(false)
}

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
    fn gate_mining_children(&self, system_data: &mut MissionExecutionSystemData, force_off: bool) {
        // `force_off` (an invader stronghold makes the WHOLE room lethal) forces
        // every child off regardless of per-keeper state — skip the now-moot
        // keeper scan and leave `has_visibility` false so nothing is suppressed.
        let (has_visibility, keepers): (bool, Vec<Position>) = if force_off {
            (false, Vec::new())
        } else {
            match system_data.room_data.get(self.sk_room_data) {
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
            }
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

        let (sk_room, stronghold_present) = match system_data.room_data.get(self.sk_room_data) {
            Some(room_data) => {
                if let Some(dynamic) = room_data.get_dynamic_visibility_data() {
                    if dynamic.owner().hostile() || dynamic.reservation().hostile() {
                        return Ok(MissionResult::Success);
                    }
                }
                (room_data.name, sk_room_has_stronghold(room_data))
            }
            None => return Ok(MissionResult::Success),
        };

        // Keep the K3 mining children attached either way (idempotent) so the
        // farm resumes instantly once the room is clear again.
        self.ensure_mining_children(system_data, mission_entity)?;

        let farm_kind = ObjectiveKind::Farm {
            kind: FarmKind::SourceKeeper,
            room: sk_room,
        };

        // Stand down on an invader stronghold (ADR 0018 §3.5 — a stronghold is
        // TRANSIENT, so PAUSE, never cancel: it auto-collapses in ~75k ticks).
        // A level≥1 core rings the whole room with towers + defenders that shred
        // the suppression duo and any miner. We (1) WITHDRAW the `Farm{sk}`
        // objective immediately — not merely stop re-asserting it — so the
        // `SquadManager` retires the duo THIS tick instead of commanding it to
        // engage keepers for the 200t objective TTL (driving it deeper into
        // tower fire); (2) force ALL source mining + hauling off; and (3)
        // periodically re-probe visibility (`STRONGHOLD_RESCOUT_INTERVAL`) so we
        // notice when the stronghold clears and can resume. The mission stays
        // Running (a stronghold is transient).
        //
        // ⚠ Residual gap (ADR 0008 L1 — single-room flee): this stops the bot
        // FEEDING the room, but creeps already inside cannot flee *across* the
        // room boundary, and the duo's disband / the miner flee-reflex only
        // react to hostile creeps, not towers — so the last in-flight creeps may
        // still take tower fire on the way out. Prompt cross-room evacuation is
        // tracked as the L1 follow-up.
        if stronghold_present {
            if let Some(id) = system_data.combat_objective_queue.find_by_kind(&farm_kind) {
                system_data.combat_objective_queue.withdraw(id);
            }
            self.gate_mining_children(system_data, true);
            if game::time().is_multiple_of(STRONGHOLD_RESCOUT_INTERVAL) {
                system_data.visibility.request(VisibilityRequest::new(
                    sk_room,
                    VISIBILITY_PRIORITY_LOW,
                    VisibilityRequestFlags::ALL,
                ));
            }
            if system_data.features.source_keeper.diagnostics {
                info!("SK farm {sk_room}: invader stronghold present — standing down (duo recalled, mining paused)");
            }
            return Ok(MissionResult::Running);
        }

        // Coordinator role (ADR 0018 §3.3, reconciliation §2.0 (ad)): the mission
        // does NOT own a squad. It REQUESTS a low-priority, preemptible `Farm{sk}`
        // objective each tick it is viable; the `SquadManager` claims it, fields the
        // `duo_sk_farmer`, and retires the squad when this upsert stops (feature off
        // / contested / lost homes / stronghold → the objective is withdrawn or
        // TTL-lapses). The squad's `SquadCombatJob` self-drives to the SK room and
        // suppresses the keepers (job-owns-movement, ADR 0008 §5 ⚑). K3 source
        // mining + the per-source suppression signal remain the coordinator's to own.
        // R6 (ADR 0020 §12.6): force-size the suppression duo's HEALER to out-heal a
        // Source Keeper (168 melee DPS × the hold margin) at the strongest in-range
        // home's energy — the same energy the `SquadManager` spawn path sizes a `Sized`
        // body against — so a kiting slip costs HP it recovers, not a death. The ranged
        // kiter stays the proven template. `sized_for` returns `None` when no home can
        // afford the sized healer (low RCL) → fall back to the template duo (the spawn
        // path still builds the largest healer that home affords). The SK suppression
        // model is positional (mine when the keeper is away), so this sizes the duo to
        // SURVIVE a keeper engagement, not to tank keepers continuously.
        let required = crate::military::force_sizing::RequiredForce {
            heal_parts: crate::military::damage::defender_heal_parts_for_dps(
                SK_KEEPER_MELEE_DPS * crate::military::force_sizing::HOLD_MARGIN,
                false,
            ),
            ..Default::default()
        };
        let home_energy = self
            .home_room_datas
            .iter()
            .filter_map(|&e| system_data.room_data.get(e))
            .filter_map(|rd| game::rooms().get(rd.name))
            .map(|r| r.energy_capacity_available())
            .max()
            .unwrap_or(0);
        let duo = SquadComposition::duo_sk_farmer();
        let comp = duo.sized_for(required, home_energy).unwrap_or(duo);

        let request = ObjectiveRequest::new(farm_kind, OBJECTIVE_PRIORITY_LOW, ForceRequirement::single(comp))
            .owner(ObjectiveOwner::SourceKeeper);
        system_data.combat_objective_queue.request(request, game::time());

        // K3: own per-source mining + hauling, gated on per-source keeper liveness.
        self.gate_mining_children(system_data, false);

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
