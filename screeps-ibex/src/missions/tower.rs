use super::data::*;
use super::missionsystem::*;
use crate::jobs::utility::repair::*;
use crate::remoteobjectid::*;
use crate::serialize::*;
use crate::transfer::transfersystem::*;
use log::*;
use screeps::*;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
/// Observed heal-while-away cycles before a hostile is treated as a confirmed
/// tower drainer. One is enough: a creep that re-enters with MORE hits than it
/// left can only have been healed outside our fire, so further fire is wasted.
/// Wasting tower energy is the costly error (worst for slow drainers); briefly
/// sparing a real attacker at long range is cheap — the bounded probe below
/// still finishes it the moment its heal support is gone.
const DRAIN_CONFIRM_CYCLES: u32 = 1;

/// Failed probe volleys spent on one drainer before we stop testing it for good
/// (permanent conserve). This is the bait ceiling: worst-case energy a single
/// drainer can pull out of us is bounded by MAX_PROBE_STRIKES volleys.
const MAX_PROBE_STRIKES: u32 = 3;

/// Ticks to wait after a failed probe before testing the same drainer again, so
/// repeated probing is a slow trickle rather than a steady drain.
const PROBE_COOLDOWN: u32 = 20;

/// Minimum hit drop between probe observations to count as "winning" (we are
/// out-damaging the heal). At or above this we keep firing to the kill; below
/// it the volley is treated as out-healed and counts as a failed probe. This is
/// what defeats a slow-bleed bait that leaks a trickle of damage to keep us
/// shooting: to pass, the creep must actually be dying fast enough to finish.
const MIN_PROBE_PROGRESS: u32 = 200;

/// Tracks a hostile creep suspected of tower draining.
///
/// Detection keys on the hitpoint *sawtooth* a drainer produces, NOT on the
/// creep's body: it takes tower fire inside the room, retreats, is healed back
/// up outside (commonly by a healer staged beyond tower range, so the drainer
/// itself carries no HEAL parts), then re-enters with more hits than it left
/// with. A roaming scout transits untouched and never returns healthier than
/// it left, so it is never confirmed.
#[derive(Clone, Debug, Default, ConvertSaveload)]
pub struct DrainTracker {
    /// How many times this creep has been seen entering the room.
    enter_count: u32,
    /// Last tick the creep was seen in the room.
    last_seen_tick: u32,
    /// Whether the creep was in the room last tick.
    was_present: bool,
    /// Hits observed the most recent tick the creep was present. Updated every
    /// tick it is in the room; snapshotted into `hits_on_exit` when it leaves.
    last_hits: u32,
    /// Hits the creep had the tick it most recently left the room.
    hits_on_exit: u32,
    /// Completed drain cycles: re-entries where the creep returned with more
    /// hits than it left with (i.e. healed while outside our fire).
    drain_cycles: u32,
    /// Whether confirmation has already been announced, so the log fires once
    /// per drainer rather than every tick.
    confirmation_logged: bool,
    /// Whether a bounded probe is currently active against this drainer — i.e.
    /// we are firing at it to test whether its heal support is gone.
    engaging: bool,
    /// Hits at the last probe observation, used to measure whether the current
    /// volley is out-damaging the heal.
    engage_baseline_hits: u32,
    /// Whether a tower volley was actually spent on this drainer last tick, so
    /// the next observation knows there is a probe result to judge.
    probe_fired: bool,
    /// Failed probe volleys so far; at [`MAX_PROBE_STRIKES`] we stop testing.
    probe_strikes: u32,
    /// Earliest tick a new probe may begin after the last failed one.
    probe_cooldown_until: u32,
}

#[derive(ConvertSaveload)]
pub struct TowerMission {
    owner: EntityOption<Entity>,
    room_data: Entity,
    /// Per-creep drain tracking data, keyed by stable hostile object id (not
    /// name: names can collide across players and be reused). Persisted across
    /// ticks so drain detection survives VM reloads.
    drain_trackers: EntityHashMap<ObjectId<Creep>, DrainTracker>,
    /// Last tick when stale drain trackers were cleaned up.
    last_drain_cleanup: u32,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl TowerMission {
    pub fn build<B>(builder: B, owner: Option<Entity>, room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = TowerMission::new(owner, room_data);

        builder
            .with(MissionData::Tower(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>, room_data: Entity) -> TowerMission {
        TowerMission {
            owner: owner.into(),
            room_data,
            drain_trackers: EntityHashMap::new(),
            last_drain_cleanup: 0,
        }
    }

    /// Get the set of creep ids confirmed as tower drainers — those that have
    /// completed at least [`DRAIN_CONFIRM_CYCLES`] observed damage->heal->return
    /// cycles (see [`DrainTracker`]).
    fn get_confirmed_drainers(&self) -> std::collections::HashSet<ObjectId<Creep>> {
        self.drain_trackers
            .iter()
            .filter(|(_, tracker)| tracker.drain_cycles >= DRAIN_CONFIRM_CYCLES)
            .map(|(id, _)| *id)
            .collect()
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for TowerMission {
    fn get_owner(&self) -> &Option<Entity> {
        &self.owner
    }

    fn owner_complete(&mut self, owner: Entity) {
        assert!(Some(owner) == *self.owner);

        self.owner.take();
    }

    fn get_room(&self) -> Option<Entity> {
        Some(self.room_data)
    }

    fn describe_state(&self, _system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> String {
        "Tower".to_string()
    }

    fn summarize(&self) -> crate::visualization::SummaryContent {
        crate::visualization::SummaryContent::Text("Tower".to_string())
    }

    fn pre_run_mission(&mut self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<(), String> {
        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;

        let room_data_entity = self.room_data;

        system_data.transfer_queue.register_generator(
            room_data.name,
            TransferTypeFlags::HAUL,
            Box::new(move |system, transfer, _room_name| {
                let room_data = system.get_room_data(room_data_entity).ok_or("Expected room data")?;
                let structures = room_data.get_structures().ok_or_else(|| {
                    let msg = format!("Expected structures - Room: {}", room_data.name);
                    log::warn!("{} at {}:{}", msg, file!(), line!());
                    msg
                })?;
                let creeps = room_data.get_creeps().ok_or_else(|| {
                    let msg = format!("Expected creeps - Room: {}", room_data.name);
                    log::warn!("{} at {}:{}", msg, file!(), line!());
                    msg
                })?;

                let towers = structures.towers();

                let hostile_creeps = creeps.hostile();
                let are_hostile_creeps = !hostile_creeps.is_empty();

                let priority = if are_hostile_creeps {
                    TransferPriority::High
                } else {
                    TransferPriority::Low
                };

                for tower in towers {
                    let tower_free_capacity = tower.store().get_free_capacity(Some(ResourceType::Energy));
                    if tower_free_capacity > 0 {
                        let transfer_request = TransferDepositRequest::new(
                            TransferTarget::Tower(tower.remote_id()),
                            Some(ResourceType::Energy),
                            priority,
                            tower_free_capacity as u32,
                            TransferType::Haul,
                        );

                        transfer.request_deposit(transfer_request);
                    }
                }

                Ok(())
            }),
        );

        Ok(())
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<MissionResult, String> {
        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;
        let structures = room_data.get_structures().ok_or_else(|| {
            let msg = format!("Expected structures - Room: {}", room_data.name);
            log::warn!("{} at {}:{}", msg, file!(), line!());
            msg
        })?;
        let dynamic_visibility_data = room_data.get_dynamic_visibility_data().ok_or("Expected dynamic visibility data")?;
        let creeps = room_data.get_creeps().ok_or_else(|| {
            let msg = format!("Expected creeps - Room: {}", room_data.name);
            log::warn!("{} at {}:{}", msg, file!(), line!());
            msg
        })?;

        let towers = structures.towers();
        let my_towers: Vec<_> = towers.iter().filter(|t| t.my()).collect();

        if my_towers.is_empty() {
            return Ok(MissionResult::Running);
        }

        // Collect our tower positions for damage calculations.
        let tower_positions: Vec<Position> = my_towers.iter().map(|t| t.pos()).collect();

        // Analyze hostile creeps for coordinated targeting.
        let hostile_creeps = creeps.hostile();

        // ─── Drain detection ────────────────────────────────────────────────
        // Robust tower-drain detection keys on the hitpoint sawtooth, NOT the
        // creep's body: a drainer takes tower fire in-room, retreats, is healed
        // back up outside (often by a creep staged out of tower range), and
        // returns with more hits than it left with. A transiting scout never
        // exhibits damage-then-heal-then-return, so it is never confirmed.
        let current_tick = game::time();
        let room_name = room_data.name;

        // Hostiles present this tick, keyed by stable object id.
        let mut present_ids: std::collections::HashSet<ObjectId<Creep>> = std::collections::HashSet::new();
        // Confirmed drainers we will test (fire at) this tick under the probe budget.
        let mut engaged_ids: std::collections::HashSet<ObjectId<Creep>> = std::collections::HashSet::new();

        for hostile in hostile_creeps.iter() {
            let Some(id) = hostile.try_id() else { continue };
            present_ids.insert(id);

            let cur_hits = hostile.hits();
            let tracker = self.drain_trackers.entry(id).or_default();

            // A re-entry with MORE hits than the creep left with means it was
            // healed while outside our fire (creeps gain hits only via HEAL) --
            // a healer is sustaining it through our towers. This holds even when
            // the creep oscillates across the border every single tick, which is
            // exactly how tower draining is done.
            if !tracker.was_present {
                tracker.enter_count += 1;
                if tracker.enter_count >= 2 && cur_hits > tracker.hits_on_exit {
                    tracker.drain_cycles += 1;

                    if tracker.drain_cycles >= DRAIN_CONFIRM_CYCLES && !tracker.confirmation_logged {
                        info!("[Tower] Confirmed tower drain in {}: {} ({})", room_name, hostile.name(), id);
                        tracker.confirmation_logged = true;
                    }
                }
            }

            // Bounded probe: a confirmed drainer is conserved against by default,
            // but periodically tested with a capped number of volleys to see if
            // its heal support is gone. A volley that out-damages the heal (hits
            // fell by >= MIN_PROBE_PROGRESS) means we press to the kill; otherwise
            // it is a strike, and after MAX_PROBE_STRIKES we stop testing it for
            // good. Worst-case wasted energy is bounded per creep -- it cannot
            // bait us into draining ourselves.
            if tracker.drain_cycles >= DRAIN_CONFIRM_CYCLES {
                if tracker.engaging {
                    if tracker.probe_fired {
                        if tracker.engage_baseline_hits.saturating_sub(cur_hits) >= MIN_PROBE_PROGRESS {
                            tracker.engage_baseline_hits = cur_hits;
                        } else {
                            tracker.engaging = false;
                            tracker.probe_strikes += 1;
                            tracker.probe_cooldown_until = current_tick + PROBE_COOLDOWN;
                        }
                        tracker.probe_fired = false;
                    }
                } else if tracker.probe_strikes < MAX_PROBE_STRIKES && current_tick >= tracker.probe_cooldown_until {
                    tracker.engaging = true;
                    tracker.engage_baseline_hits = cur_hits;
                }

                if tracker.engaging {
                    engaged_ids.insert(id);
                }
            }

            tracker.last_hits = cur_hits;
            tracker.last_seen_tick = current_tick;
            tracker.was_present = true;
        }

        // Mark creeps that left the room, snapshotting their exit hits.
        for (id, tracker) in self.drain_trackers.iter_mut() {
            if !present_ids.contains(id) && tracker.was_present {
                tracker.was_present = false;
                tracker.hits_on_exit = tracker.last_hits;
            }
        }

        // Periodic cleanup of stale trackers (every 100 ticks).
        if current_tick - self.last_drain_cleanup > 100 {
            self.drain_trackers.retain(|_, tracker| current_tick - tracker.last_seen_tick < 500);
            self.last_drain_cleanup = current_tick;
        }

        // Confirmed drainers drive tower fire-conservation below.
        let confirmed_drainers = self.get_confirmed_drainers();

        if !hostile_creeps.is_empty() {
            // Calculate per-hostile heal rate for net damage assessment.
            let hostile_infos: Vec<_> = hostile_creeps
                .iter()
                .map(|c| {
                    let heal_per_tick: f32 = c
                        .body()
                        .iter()
                        .filter(|p| p.hits() > 0 && p.part() == Part::Heal)
                        .map(|p| if p.boost().is_some() { 48.0 } else { 12.0 })
                        .sum();
                    let is_confirmed_drainer = c.try_id().map(|id| confirmed_drainers.contains(&id)).unwrap_or(false);
                    (c, heal_per_tick, is_confirmed_drainer)
                })
                .collect();

            // Find the best target: prefer targets where we can do net positive damage.
            // Skip confirmed drainers -- they're wasting our energy on purpose.
            let best_target = hostile_infos
                .iter()
                .filter(|(_, _, is_drainer)| !is_drainer)
                .filter(|(c, heal, _)| {
                    // Only fire if we can do net damage (overcome healing).
                    let total_damage = crate::military::damage::total_tower_damage(&tower_positions, c.pos());
                    total_damage > *heal
                })
                .min_by(|(a, _, _), (b, _, _)| {
                    // Prefer dangerous creeps first.
                    let a_dangerous = a
                        .body()
                        .iter()
                        .any(|p| matches!(p.part(), Part::Attack | Part::RangedAttack | Part::Work));
                    let b_dangerous = b
                        .body()
                        .iter()
                        .any(|p| matches!(p.part(), Part::Attack | Part::RangedAttack | Part::Work));

                    match (a_dangerous, b_dangerous) {
                        (true, false) => std::cmp::Ordering::Less,
                        (false, true) => std::cmp::Ordering::Greater,
                        _ => a.hits().cmp(&b.hits()),
                    }
                })
                .map(|(c, _, _)| *c);

            // Detect tower drain: hostile at room edge that can heal through all tower damage,
            // OR confirmed drainer based on enter/exit tracking.
            let is_drain = best_target.is_none()
                && hostile_infos.iter().any(|(c, heal, is_drainer)| {
                    *is_drainer || crate::military::damage::is_likely_tower_drain(c.pos(), *heal, &tower_positions)
                });

            if is_drain {
                // Tower drain detected: conserve energy. Fire at non-drainers
                // normally; fire at a confirmed drainer only while it is under an
                // active bounded probe (the budgeted test for a dead healer).
                let non_drainer_target = hostile_infos
                    .iter()
                    .filter(|(_, _, is_drainer)| !is_drainer)
                    .min_by_key(|(c, _, _)| c.hits())
                    .map(|(c, _, _)| *c);

                let probe_drainer = hostile_infos
                    .iter()
                    .filter(|(c, _, is_drainer)| *is_drainer && c.try_id().map(|id| engaged_ids.contains(&id)).unwrap_or(false))
                    .min_by_key(|(c, _, _)| c.hits())
                    .map(|(c, _, _)| *c);

                let target = non_drainer_target.or(probe_drainer);

                if let Some(target) = target {
                    for tower in &my_towers {
                        let _ = tower.attack(target);
                    }
                    // Record a probe volley so next tick can judge the result.
                    if let Some(tid) = target.try_id() {
                        if engaged_ids.contains(&tid) {
                            if let Some(tracker) = self.drain_trackers.get_mut(&tid) {
                                tracker.probe_fired = true;
                            }
                        }
                    }
                }
                // Otherwise, don't fire -- save energy against drainers.
            } else if let Some(target) = best_target {
                // Coordinated fire: all towers focus the same target.
                for tower in &my_towers {
                    let _ = tower.attack(target);
                }
            } else {
                // No target where we can do net damage. Check for any hostile we should still shoot.
                // Fall back to weakest non-drainer hostile.
                let weakest = hostile_creeps
                    .iter()
                    .filter(|c| !c.try_id().map(|id| confirmed_drainers.contains(&id)).unwrap_or(false))
                    .min_by_key(|c| c.hits());
                if let Some(target) = weakest {
                    for tower in &my_towers {
                        let _ = tower.attack(target);
                    }
                }
            }

            return Ok(MissionResult::Running);
        }

        // No hostiles -- heal friendly creeps or repair structures.
        let weakest_friendly_creep = creeps
            .friendly()
            .iter()
            .filter(|creep| creep.hits() < creep.hits_max())
            .min_by_key(|creep| creep.hits());

        let minimum_repair_priority = if dynamic_visibility_data.hostile_creeps() {
            Some(RepairPriority::Medium)
        } else {
            Some(RepairPriority::Low)
        };

        let repair_structure =
            select_repair_structure(room_data, system_data.repair_queue, minimum_repair_priority, false).and_then(|id| id.resolve());

        for tower in &my_towers {
            if let Some(creep) = weakest_friendly_creep {
                let _ = tower.heal(creep);
                continue;
            }

            if let Some(structure) = repair_structure.as_ref() {
                if let Some(repairable) = structure.as_repairable() {
                    let _ = tower.repair(repairable);
                }
                continue;
            }
        }

        Ok(MissionResult::Running)
    }
}
