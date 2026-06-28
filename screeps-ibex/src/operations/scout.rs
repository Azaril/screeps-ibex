use super::data::*;
use super::operationsystem::*;
use crate::missions::scout::*;
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

/// Always-running operation that manages scout missions for the visibility queue.
///
/// Reads the visibility queue for entries that need scouting (rooms not in
/// observer range or not observed for a long time) and spawns `ScoutMission`s
/// to service them.
#[derive(Clone, ConvertSaveload)]
pub struct ScoutOperation {
    owner: EntityOption<Entity>,
    scout_missions: EntityVec<Entity>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl ScoutOperation {
    pub fn build<B>(builder: B, owner: Option<Entity>) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let operation = ScoutOperation::new(owner);

        builder.with(OperationData::Scout(operation)).marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>) -> ScoutOperation {
        ScoutOperation {
            owner: owner.into(),
            scout_missions: EntityVec::new(),
        }
    }

    /// Gather home rooms that can spawn scouts (have spawns, level >= 2).
    fn gather_home_rooms(system_data: &OperationExecutionSystemData) -> Vec<(Entity, RoomName)> {
        let mut result = Vec::new();
        for (entity, room_data) in (system_data.entities, &*system_data.room_data).join() {
            let dvd = match room_data.get_dynamic_visibility_data() {
                Some(d) => d,
                None => continue,
            };
            if !dvd.owner().mine() {
                continue;
            }
            let structures = match room_data.get_structures() {
                Some(s) => s,
                None => continue,
            };
            if structures.spawns().is_empty() {
                continue;
            }
            let max_level = match structures.controllers().iter().map(|c: &StructureController| c.level()).max() {
                Some(l) => l,
                None => continue,
            };
            if max_level < 2 {
                continue;
            }
            result.push((entity, room_data.name));
        }
        result
    }

    /// Scout reach (in rooms, Chebyshev) for an entry of the given visibility priority.
    ///
    /// Must cover the offense/SK consumer's reach: `war.rs` qualifies offense targets by BFS route
    /// hops <= 10 (`min_distance_to_homes`), and re-scout requests it registers are
    /// `VISIBILITY_PRIORITY_MEDIUM`+ (escalated to HIGH for active offense targets — see FIX C). The
    /// old flat Manhattan-5 bound silently excluded every strategic core a few rooms past the colony
    /// (W3N5 @ Manhattan 6, W3N3 @ 8, ...) so they were never scouted, never attacked — the live
    /// "stale data; requested re-scout" loop that never resets.
    ///
    /// Strategic entries (MEDIUM/HIGH/CRITICAL) get the full war reach; lazy economy entries (LOW)
    /// keep a short bound so opportunistic exploration stays cheap. Chebyshev (max-axis) is the
    /// natural over-approximation of BFS hops — it never under-reaches a room that war can attack.
    fn scout_reach_for_priority(priority: f32) -> i32 {
        // War qualifies offense targets at <= 10 BFS hops; cover it. Chebyshev <= 10 is a superset of
        // BFS <= 10, so the existing `mark_unreachable` backoff (and war's own distance gate) prunes
        // the genuinely-unreachable ones rather than letting a too-tight static bound do it blindly.
        const STRATEGIC_REACH: i32 = 10;
        // Economy/opportunistic scouting stays local so we don't waste creeps wandering far.
        const ECONOMY_REACH: i32 = 5;

        if priority >= VISIBILITY_PRIORITY_MEDIUM {
            STRATEGIC_REACH
        } else {
            ECONOMY_REACH
        }
    }

    /// Homes WITHIN the entry's priority reach (Chebyshev), if any. Empty = the entry is out of reach
    /// for its priority tier and should be served (if at all) only by the fallback pass.
    ///
    /// Pure (no `game::` calls) so it is host-testable; `RoomName - RoomName` yields a signed
    /// `(i32, i32)` delta.
    fn in_range_homes_for_entry<E: Copy>(target: RoomName, homes: &[(E, RoomName)], priority: f32) -> Vec<E> {
        let reach = Self::scout_reach_for_priority(priority);
        homes
            .iter()
            .filter(|(_, home_name)| {
                let delta = target - *home_name;
                let cheby = delta.0.abs().max(delta.1.abs());
                cheby <= reach
            })
            .map(|(entity, _)| *entity)
            .collect()
    }

    /// The single nearest home to `target` (Chebyshev), or `None` if there are no homes. Used as the
    /// fallback assignment so a strategic room just past the reach is never silently abandoned (the
    /// `mark_unreachable` backoff protects truly-unreachable rooms).
    fn nearest_home<E: Copy>(target: RoomName, homes: &[(E, RoomName)]) -> Option<E> {
        homes
            .iter()
            .min_by_key(|(_, home_name)| {
                let delta = target - *home_name;
                delta.0.abs().max(delta.1.abs())
            })
            .map(|(entity, _)| *entity)
    }

    /// Select the home rooms for an entry: in-range homes preferred, else the nearest-home fallback.
    /// Kept for callers/tests that just want "give me a non-empty home set for this entry".
    fn select_home_rooms_for_entry<E: Copy>(target: RoomName, homes: &[(E, RoomName)], priority: f32) -> Vec<E> {
        let in_range = Self::in_range_homes_for_entry(target, homes, priority);
        if !in_range.is_empty() {
            return in_range;
        }
        Self::nearest_home(target, homes).map(|e| vec![e]).unwrap_or_default()
    }

    /// FIX B (pure, testable core of the spawn loop): given priority-sorted eligible entries
    /// (`(room, priority)`) and the home set, return the rooms that should get a scout mission this
    /// tick — iterating the FULL list and stopping at `slots` *created* missions, NOT `take(slots)`.
    /// `has_room_entity` mirrors `mapping.get_room` (an entry with no mapped room is skipped without
    /// consuming budget).
    ///
    /// TWO PASSES so the two fixes compose without fighting each other:
    ///   - Pass 1 fields entries that have an IN-RANGE home (priority order). This is where the live
    ///     fix lands: an in-range strategic MEDIUM core below the claim flood now gets a slot because
    ///     OUT-OF-RANGE claim entries do NOT consume budget in pass 1 (they have no in-range home).
    ///   - Pass 2 fills any leftover budget with the nearest-home FALLBACK for still-unplanned
    ///     entries, so a strategic room just past the reach is not abandoned — but only AFTER every
    ///     in-range room has had its chance. This is the precise reconciliation of "don't let
    ///     out-of-range entries starve in-range ones" (FIX B) with "never silently drop a room" (FIX A).
    fn plan_scout_spawns<E: Copy>(
        sorted_entries: &[(RoomName, f32)],
        homes: &[(E, RoomName)],
        slots: usize,
        has_room_entity: &dyn Fn(RoomName) -> bool,
    ) -> Vec<(RoomName, Vec<E>)> {
        let mut plan: Vec<(RoomName, Vec<E>)> = Vec::new();
        let mut planned: std::collections::HashSet<RoomName> = std::collections::HashSet::new();

        // Pass 1: in-range homes only.
        for (room, priority) in sorted_entries {
            if plan.len() >= slots {
                return plan;
            }
            if !has_room_entity(*room) {
                continue;
            }
            let in_range = Self::in_range_homes_for_entry(*room, homes, *priority);
            if in_range.is_empty() {
                continue;
            }
            plan.push((*room, in_range));
            planned.insert(*room);
        }

        // Pass 2: nearest-home fallback for the rest (preserves priority order).
        for (room, _priority) in sorted_entries {
            if plan.len() >= slots {
                break;
            }
            if planned.contains(room) || !has_room_entity(*room) {
                continue;
            }
            if let Some(home) = Self::nearest_home(*room, homes) {
                plan.push((*room, vec![home]));
                planned.insert(*room);
            }
        }

        plan
    }

    /// Inject visibility requests for rooms that have a "scout" flag placed in them.
    /// This forces scouting regardless of whether the room would normally be queued.
    fn inject_flag_scout_requests(visibility: &mut VisibilityQueue) {
        for flag in game::flags().values() {
            if flag.name().to_lowercase().starts_with("scout") {
                let room_name = flag.pos().room_name();
                visibility.request(VisibilityRequest::new(
                    room_name,
                    VISIBILITY_PRIORITY_HIGH,
                    VisibilityRequestFlags::ALL,
                ));
            }
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Operation for ScoutOperation {
    fn get_owner(&self) -> &Option<Entity> {
        &self.owner
    }

    fn owner_complete(&mut self, owner: Entity) {
        assert!(Some(owner) == *self.owner);
        self.owner.take();
    }

    fn child_complete(&mut self, child: Entity) {
        self.scout_missions.retain(|e| *e != child);
    }

    fn repair_entity_refs(&mut self, is_valid: &dyn Fn(Entity) -> bool) {
        self.scout_missions.retain(|e| {
            let ok = is_valid(*e);
            if !ok {
                error!("INTEGRITY: dead scout mission entity {:?} removed from ScoutOperation", e);
            }
            ok
        });
    }

    fn describe_operation(&self, _ctx: &OperationDescribeContext) -> SummaryContent {
        SummaryContent::Text(format!("Scout - Missions: {}", self.scout_missions.len()))
    }

    fn pre_run_operation(&mut self, _system_data: &mut OperationExecutionSystemData, _runtime_data: &mut OperationExecutionRuntimeData) {}

    fn run_operation(
        &mut self,
        system_data: &mut OperationExecutionSystemData,
        runtime_data: &mut OperationExecutionRuntimeData,
    ) -> Result<OperationResult, ()> {
        // Inject visibility requests from "scout" flags so flagged rooms are
        // always queued for scouting.
        Self::inject_flag_scout_requests(system_data.visibility);

        // Check if there are scout-eligible entries that need servicing.
        if !system_data.visibility.has_unclaimed_scout_eligible() {
            return Ok(OperationResult::Running);
        }

        // Don't spawn too many scout missions.
        const MAX_SCOUT_MISSIONS: usize = 3;
        if self.scout_missions.len() >= MAX_SCOUT_MISSIONS {
            return Ok(OperationResult::Running);
        }

        let home_rooms = Self::gather_home_rooms(system_data);
        if home_rooms.is_empty() {
            return Ok(OperationResult::Running);
        }

        // Collect rooms that already have an active scout mission to avoid duplicates.
        let mut rooms_with_missions: std::collections::HashSet<RoomName> = std::collections::HashSet::new();
        for mission_entity in self.scout_missions.iter() {
            if let Some(mission) = system_data.mission_data.get(*mission_entity) {
                let room_entity = mission.as_mission().get_room();
                if let Some(room_data) = room_entity.and_then(|e| system_data.room_data.get(e)) {
                    rooms_with_missions.insert(room_data.name);
                }
            }
        }

        // Spawn missions for unclaimed entries until we hit the cap.
        let slots = MAX_SCOUT_MISSIONS.saturating_sub(self.scout_missions.len());

        // Gather eligible entries sorted by priority descending.
        // Opportunistic entries (created by idle scouts for proactive
        // exploration) are excluded — they should not trigger new missions.
        let now = game::time();
        let mut eligible_entries: Vec<_> = system_data
            .visibility
            .entries
            .iter()
            .filter(|e| e.allowed_types.contains(VisibilityRequestFlags::SCOUT) && !e.opportunistic)
            .filter(|e| !system_data.visibility.is_unreachable_now(e.room_name, now))
            .filter(|e| {
                let rt = system_data.visibility.runtime.get(&e.room_name);
                let claimed = rt.map(|r| r.claimed_by.is_some()).unwrap_or(false);
                !claimed && !rooms_with_missions.contains(&e.room_name)
            })
            .cloned()
            .collect();

        eligible_entries.sort_by(|a, b| a.priority.partial_cmp(&b.priority).unwrap_or(std::cmp::Ordering::Equal).reverse());

        // FIX B: iterate the FULL priority-sorted list and spawn until we've created `slots`
        // missions — do NOT `take(slots)` up front. claim.rs floods CRITICAL/HIGH visibility
        // requests; war/SK re-scouts are MEDIUM (HIGH for active offense targets, FIX C). With a
        // premature `take(slots)`, the top-`slots` window was consumed by claim entries (out-of-range
        // ones hit `continue` and still burned a slot), so the in-range strategic MEDIUM entries
        // (cores, W8N8) below the window were never reached. `plan_scout_spawns` iterates the whole
        // list and only counts an entry that actually yields a mission.
        let sorted_entries: Vec<(RoomName, f32)> = eligible_entries.iter().map(|e| (e.room_name, e.priority)).collect();
        let mapping = &system_data.mapping;
        let plan = Self::plan_scout_spawns(&sorted_entries, &home_rooms, slots, &|room| mapping.get_room(&room).is_some());

        let mut created = 0;
        for (room_name, home_room_entities) in plan {
            // Re-resolve the room entity (cheap; `plan_scout_spawns` already confirmed it exists).
            let room_entity = match system_data.mapping.get_room(&room_name) {
                Some(e) => e,
                None => continue,
            };
            let priority = sorted_entries.iter().find(|(r, _)| *r == room_name).map(|(_, p)| *p).unwrap_or(0.0);

            debug!("ScoutOperation: spawning scout mission for room {}", room_name);

            let mission_entity = ScoutMission::build(
                system_data.updater.create_entity(system_data.entities),
                Some(runtime_data.entity),
                room_entity,
                &home_room_entities,
                priority,
            )
            .build();

            self.scout_missions.push(mission_entity);
            rooms_with_missions.insert(room_name);

            if let Some(room_data) = system_data.room_data.get_mut(room_entity) {
                room_data.add_mission(mission_entity);
            }

            created += 1;
        }

        if created > 0 {
            debug!("ScoutOperation: created {} scout missions this tick", created);
        }

        Ok(OperationResult::Running)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rn(s: &str) -> RoomName {
        RoomName::new(s).unwrap()
    }

    // FIX A: a strategic room at BFS <= 10 but Manhattan > 5 (the old gate) must yield a non-empty
    // home set, so a scout mission can be spawned. W3N5 from a W9N5 home is Chebyshev 6 / Manhattan
    // 6 — excluded by the old `<= 5` Manhattan bound, the live "never scouted, never attacked" core.
    #[test]
    fn strategic_room_past_manhattan_5_still_gets_a_home() {
        // Entity stand-in = u32 (helpers are generic over a Copy entity type).
        let homes = vec![(7u32, rn("W9N5"))];
        // Old Manhattan distance from W9N5 to W3N5 is 6 ( > 5 ⇒ old gate excluded it forever).
        let delta = rn("W3N5") - rn("W9N5");
        assert_eq!(delta.0.unsigned_abs() + delta.1.unsigned_abs(), 6, "precondition: Manhattan > 5");

        // MEDIUM (war re-scout) gets the full strategic reach (10) ⇒ in range ⇒ home assigned.
        let medium = ScoutOperation::select_home_rooms_for_entry(rn("W3N5"), &homes, VISIBILITY_PRIORITY_MEDIUM);
        assert_eq!(medium, vec![7u32], "MEDIUM strategic entry must reach W3N5");

        // Even further cores in the soak (W3N3 = Chebyshev 6, W4N3 = 5) are covered too.
        assert!(!ScoutOperation::select_home_rooms_for_entry(rn("W3N3"), &homes, VISIBILITY_PRIORITY_MEDIUM).is_empty());
        assert!(!ScoutOperation::select_home_rooms_for_entry(rn("W4N3"), &homes, VISIBILITY_PRIORITY_MEDIUM).is_empty());
    }

    // FIX A: economy (LOW) scouting stays local — a far room past the economy reach (5) still gets a
    // home only via the nearest-home fallback (never silently abandoned), but is NOT treated as
    // in-range. We assert the fallback yields exactly one (nearest) home rather than the empty set
    // that caused the original starvation.
    #[test]
    fn out_of_reach_entry_falls_back_to_nearest_home_never_empty() {
        let homes = vec![(1u32, rn("W9N9")), (2u32, rn("W9N60"))];
        // Far north for everyone (Chebyshev > strategic reach 10 from both homes, on the N axis so
        // the nearest is unambiguous — no x-axis tie).
        let far = rn("W9N40");
        // Precondition: out of strategic reach (10) from both homes (31 and 20 respectively).
        assert!(ScoutOperation::in_range_homes_for_entry(far, &homes, VISIBILITY_PRIORITY_MEDIUM).is_empty());
        let got = ScoutOperation::select_home_rooms_for_entry(far, &homes, VISIBILITY_PRIORITY_MEDIUM);
        assert_eq!(got.len(), 1, "fallback must pick the single nearest home, not abandon the room");
        // W9N40 is 31 from W9N9 and 20 from W9N60 ⇒ W9N60 (entity 2) is nearest.
        assert_eq!(got, vec![2u32], "W9N60 is nearer to W9N40 than W9N9");
    }

    // FIX A: priority drives the reach — strategic >= MEDIUM reaches 10, lazy LOW stays at 5.
    #[test]
    fn reach_scales_with_priority() {
        assert_eq!(ScoutOperation::scout_reach_for_priority(VISIBILITY_PRIORITY_CRITICAL), 10);
        assert_eq!(ScoutOperation::scout_reach_for_priority(VISIBILITY_PRIORITY_HIGH), 10);
        assert_eq!(ScoutOperation::scout_reach_for_priority(VISIBILITY_PRIORITY_MEDIUM), 10);
        assert_eq!(ScoutOperation::scout_reach_for_priority(VISIBILITY_PRIORITY_LOW), 5);
    }

    // FIX B: an in-range MEDIUM strategic entry sitting BELOW out-of-range CRITICAL/HIGH claim
    // entries in the priority order must still be spawned. With the old `take(slots)`, the top
    // `slots` window was eaten by the claim entries (which, if out of range, used to skip and waste
    // the slot); now the full list is walked and only entries that actually yield a mission count.
    #[test]
    fn in_range_medium_behind_out_of_range_claims_still_spawns() {
        // One spawnable home near the strategic core, far from the claim targets.
        let homes = vec![(42u32, rn("W9N5"))];

        // Priority-sorted desc, as the real loop sorts: two CRITICAL + one HIGH claim rooms way out
        // of reach (the claim flood), then the in-range MEDIUM strategic core W3N5.
        let sorted = vec![
            (rn("E20N20"), VISIBILITY_PRIORITY_CRITICAL),
            (rn("E20N18"), VISIBILITY_PRIORITY_CRITICAL),
            (rn("E18N20"), VISIBILITY_PRIORITY_HIGH),
            (rn("W3N5"), VISIBILITY_PRIORITY_MEDIUM),
        ];

        // slots = 3 (the live MAX_SCOUT_MISSIONS). All rooms map to an entity.
        let plan = ScoutOperation::plan_scout_spawns(&sorted, &homes, 3, &|_| true);

        // The in-range strategic core MUST appear in the plan and reach its real home (42), not the
        // nearest-fallback degenerate case (here W9N5 IS in strategic range, so it's a real hit).
        let core = plan.iter().find(|(r, _)| *r == rn("W3N5"));
        assert!(core.is_some(), "in-range MEDIUM core starved by the claim flood: {plan:?}");
        assert_eq!(core.unwrap().1, vec![42u32]);
    }

    // FIX B: the spawn budget is honored on entries that actually produce a mission — the loop stops
    // at `slots` created, not after scanning `slots` entries.
    #[test]
    fn plan_respects_slot_budget_by_created_not_scanned() {
        let homes = vec![(1u32, rn("W9N5"))];
        let sorted = vec![
            (rn("W8N5"), VISIBILITY_PRIORITY_HIGH),
            (rn("W7N5"), VISIBILITY_PRIORITY_HIGH),
            (rn("W6N5"), VISIBILITY_PRIORITY_HIGH),
            (rn("W5N5"), VISIBILITY_PRIORITY_HIGH),
        ];
        // Budget 2 ⇒ exactly 2 spawned even though 4 are eligible/in-range.
        let plan = ScoutOperation::plan_scout_spawns(&sorted, &homes, 2, &|_| true);
        assert_eq!(plan.len(), 2);

        // An entry with no mapped room entity is skipped WITHOUT consuming budget: the first room has
        // no entity, so the next two in-range rooms fill the budget of 2.
        let plan2 = ScoutOperation::plan_scout_spawns(&sorted, &homes, 2, &|r| r != rn("W8N5"));
        assert_eq!(plan2.len(), 2);
        assert!(!plan2.iter().any(|(r, _)| *r == rn("W8N5")), "missing-entity room must not be planned");
        assert!(plan2.iter().any(|(r, _)| *r == rn("W7N5")));
        assert!(plan2.iter().any(|(r, _)| *r == rn("W6N5")));
    }
}
