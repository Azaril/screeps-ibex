use crate::missions::data::*;
use crate::operations::data::*;
use crate::room::data::*;
use log::*;
use specs::prelude::*;
use std::collections::HashSet;

/// Context extracted at queue-time for a creep/job deletion.
pub struct CreepCleanup {
    pub entity: Entity,
}

/// Context extracted at queue-time for a mission deletion.
pub struct MissionCleanup {
    pub entity: Entity,
    pub owner: Option<Entity>,
    pub children: Vec<Entity>,
    pub room: Entity,
}

/// Context extracted at queue-time for an operation deletion.
pub struct OperationCleanup {
    pub entity: Entity,
    pub owner: Option<Entity>,
}

/// Typed cleanup entry -- each variant carries all the data needed
/// to perform notifications and deletion without further lookups.
pub enum CleanupEntry {
    Creep(CreepCleanup),
    Mission(MissionCleanup),
    Operation(OperationCleanup),
}

impl CleanupEntry {
    pub fn entity(&self) -> Entity {
        match self {
            CleanupEntry::Creep(c) => c.entity,
            CleanupEntry::Mission(m) => m.entity,
            CleanupEntry::Operation(o) => o.entity,
        }
    }
}

/// World resource: collects entities scheduled for deletion.
#[derive(Default)]
pub struct EntityCleanupQueue {
    pending: Vec<CleanupEntry>,
}

impl EntityCleanupQueue {
    pub fn delete_creep(&mut self, entity: Entity) {
        self.pending.push(CleanupEntry::Creep(CreepCleanup { entity }));
    }

    pub fn delete_mission(&mut self, cleanup: MissionCleanup) {
        self.pending.push(CleanupEntry::Mission(cleanup));
    }

    pub fn delete_operation(&mut self, cleanup: OperationCleanup) {
        self.pending.push(CleanupEntry::Operation(cleanup));
    }

    /// Drain all pending entries, leaving the queue empty.
    fn drain(&mut self) -> Vec<CleanupEntry> {
        std::mem::take(&mut self.pending)
    }

    fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

/// Helper: extract a `MissionCleanup` from a live mission entity's component data.
pub fn extract_mission_cleanup(entity: Entity, missions: &WriteStorage<'_, MissionData>) -> Option<MissionCleanup> {
    missions.get(entity).map(|md| {
        let mission = md.as_mission();
        MissionCleanup {
            entity,
            owner: *mission.get_owner(),
            children: mission.get_children(),
            room: mission.get_room(),
        }
    })
}

/// System that processes the `EntityCleanupQueue` in a well-ordered pass.
///
/// Runs after all main-pass systems (after `RunJobSystem`) and before
/// serialization systems. Drains the queue and performs all deletions
/// synchronously with full world access.
pub struct EntityCleanupSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for EntityCleanupSystem {
    type SystemData = (
        Entities<'a>,
        WriteStorage<'a, MissionData>,
        WriteStorage<'a, OperationData>,
        WriteStorage<'a, RoomData>,
        Write<'a, EntityCleanupQueue>,
    );

    fn run(&mut self, (entities, missions, mut operations, mut room_data, mut queue): Self::SystemData) {
        if queue.is_empty() {
            return;
        }

        // ── Phase 1: Process creep deaths ────────────────────────────
        //
        // For each CreepCleanup entry, notify every live mission via
        // remove_creep(), then delete the creep entity.

        let all_entries = queue.drain();
        let mut creep_entries = Vec::new();
        let mut mission_entries = Vec::new();
        let mut operation_entries = Vec::new();

        for entry in all_entries {
            match entry {
                CleanupEntry::Creep(c) => creep_entries.push(c),
                CleanupEntry::Mission(m) => mission_entries.push(m),
                CleanupEntry::Operation(o) => operation_entries.push(o),
            }
        }

        if !creep_entries.is_empty() {
            // Notify all missions of each dead creep.
            for creep in &creep_entries {
                for md in (&missions).join() {
                    md.as_mission_mut().remove_creep(creep.entity);
                }
            }

            // Delete creep entities.
            for creep in &creep_entries {
                if let Err(err) = entities.delete(creep.entity) {
                    warn!("EntityCleanupSystem: failed to delete creep {:?}: {}", creep.entity, err);
                }
            }
        }

        // ── Phase 2: Expand mission cascades ─────────────────────────
        //
        // For each MissionCleanup entry, call complete() on the mission
        // to cascade-abort children. complete() pushes more MissionCleanup
        // entries into the queue. Loop until stable.

        let mut iteration = 0;
        const MAX_CASCADE_ITERATIONS: usize = 20;

        loop {
            let mut new_entries: Vec<MissionCleanup> = Vec::new();

            for mc in &mission_entries {
                // Children are already captured in mc.children from the
                // pre-extracted context. We cascade by extracting cleanup
                // data for each child below.

                // For each child in the captured list, extract cleanup data
                // and add to new_entries (if not already scheduled).
                for &child in &mc.children {
                    if entities.is_alive(child) {
                        if let Some(child_cleanup) = extract_mission_cleanup(child, &missions) {
                            new_entries.push(child_cleanup);
                        }
                    }
                }
            }

            if new_entries.is_empty() {
                break;
            }

            iteration += 1;
            if iteration >= MAX_CASCADE_ITERATIONS {
                error!(
                    "EntityCleanupSystem: cascade iteration limit ({}) reached with {} pending entries",
                    MAX_CASCADE_ITERATIONS,
                    new_entries.len()
                );
                // Process what we have and break to avoid infinite loops.
                mission_entries.extend(new_entries);
                break;
            }

            mission_entries.extend(new_entries);
        }

        // ── Phase 3: Deduplicate ─────────────────────────────────────
        //
        // An entity may appear multiple times (e.g. a child scheduled by
        // its parent's cascade AND by its own failure). Deduplicate so we
        // only process each entity once.

        let mut seen_missions = HashSet::new();
        mission_entries.retain(|mc| seen_missions.insert(mc.entity));

        let mut seen_operations = HashSet::new();
        operation_entries.retain(|oc| seen_operations.insert(oc.entity));

        // ── Phase 4: Topological sort (children before parents) ──────
        //
        // Build a set of all entities being deleted. For each entry,
        // check if its owner is also being deleted. If so, the entry
        // must come before its owner. Simple approach: entries without
        // owners-in-set go last (they are parents).

        let mission_entity_set: HashSet<Entity> = mission_entries.iter().map(|mc| mc.entity).collect();

        // Partition: children (owner is in the set) first, parents last.
        let mut children_first: Vec<MissionCleanup> = Vec::new();
        let mut parents_last: Vec<MissionCleanup> = Vec::new();

        for mc in mission_entries {
            if mc.owner.map(|o| mission_entity_set.contains(&o)).unwrap_or(false) {
                children_first.push(mc);
            } else {
                parents_last.push(mc);
            }
        }

        // Reassemble: children first, then parents.
        let sorted_missions: Vec<MissionCleanup> = children_first.into_iter().chain(parents_last).collect();

        // ── Phase 5: Delete entities (children first) ────────────────

        for mc in &sorted_missions {
            if !entities.is_alive(mc.entity) {
                continue;
            }

            // Remove from RoomData.missions.
            if let Some(rd) = room_data.get_mut(mc.room) {
                rd.remove_mission(mc.entity);
            }

            // Notify children via owner_complete (if alive).
            for &child in &mc.children {
                if entities.is_alive(child) {
                    if let Some(od) = operations.get_mut(child) {
                        od.as_operation().owner_complete(mc.entity);
                    }
                    if let Some(md) = missions.get(child) {
                        md.as_mission_mut().owner_complete(mc.entity);
                    }
                }
            }

            // Notify owner via child_complete (if alive).
            if let Some(owner) = mc.owner {
                if entities.is_alive(owner) {
                    if let Some(od) = operations.get_mut(owner) {
                        od.as_operation().child_complete(mc.entity);
                    }
                    if let Some(md) = missions.get(owner) {
                        md.as_mission_mut().child_complete(mc.entity);
                    }
                }
            }

            // Delete the entity.
            if let Err(err) = entities.delete(mc.entity) {
                warn!("EntityCleanupSystem: failed to delete mission {:?}: {}", mc.entity, err);
            }
        }

        // Process operation deletions.
        for oc in &operation_entries {
            if !entities.is_alive(oc.entity) {
                continue;
            }

            // Notify owner via child_complete (if alive).
            if let Some(owner) = oc.owner {
                if entities.is_alive(owner) {
                    if let Some(od) = operations.get_mut(owner) {
                        od.as_operation().child_complete(oc.entity);
                    }
                    if let Some(md) = missions.get(owner) {
                        md.as_mission_mut().child_complete(oc.entity);
                    }
                }
            }

            // Delete the entity.
            if let Err(err) = entities.delete(oc.entity) {
                warn!("EntityCleanupSystem: failed to delete operation {:?}: {}", oc.entity, err);
            }
        }
    }
}
