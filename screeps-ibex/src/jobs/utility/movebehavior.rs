use crate::jobs::actions::*;
use crate::jobs::context::*;
use screeps::*;
use screeps_foreman::constants::*;
use screeps_rover::*;

/// Threshold (in ticks) beyond which stuck detection is reported to the caller
/// as a movement failure. Below this threshold, the movement system handles
/// recovery internally (repathing, avoiding creeps, etc.).
pub const STUCK_REPORT_THRESHOLD: u16 = 10;

/// Check the movement results from the previous tick for the current creep.
/// Returns `Some(())` if movement failed in a way that the job should handle
/// (e.g. path not found, stuck timeout). Returns `None` if movement is
/// proceeding normally or stuck recovery is still in progress.
#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn check_movement_failure(tick_context: &JobTickContext) -> Option<MovementFailure> {
    let entity = tick_context.runtime_data.creep_entity;
    let results = tick_context.runtime_data.movement_results;

    match results.get(&entity) {
        Some(MovementResult::Failed(failure)) => Some(failure.clone()),
        Some(MovementResult::Stuck { ticks }) if *ticks >= STUCK_REPORT_THRESHOLD => Some(MovementFailure::StuckTimeout { ticks: *ticks }),
        _ => None,
    }
}

/// Register the creep as idle and freely shovable. Call this when a creep has
/// nothing to do (Wait/Idle states) and is just occupying a tile. The resolver
/// can push it anywhere to clear the way for other creeps.
#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn mark_idle(tick_context: &mut JobTickContext) {
    let creep = tick_context.runtime_data.owner;
    let creep_pos = creep.pos();

    let mut builder = tick_context
        .runtime_data
        .movement
        .move_to(tick_context.runtime_data.creep_entity, creep_pos);

    builder.range(0).priority(MovementPriority::Low).allow_shove(true).allow_swap(true);
}

/// Register the creep as immovable at its current position. Call this when a
/// creep must stay on its exact tile (e.g. static miners on a container).
/// The movement resolver will never shove or swap this creep.
#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn mark_immovable(tick_context: &mut JobTickContext) {
    let creep = tick_context.runtime_data.owner;
    let creep_pos = creep.pos();

    let mut builder = tick_context
        .runtime_data
        .movement
        .move_to(tick_context.runtime_data.creep_entity, creep_pos);

    builder
        .range(0)
        .priority(MovementPriority::Immovable)
        .allow_shove(false)
        .allow_swap(false);
}

/// Register the creep as stationed at its current position. The creep uses
/// High priority so it wins tile conflicts against most other creeps, causing
/// them to repath around after their stuck timer fires. Shoving is still
/// permitted as a last resort so creeps with no alternative path can push
/// through.
///
/// Use this for creeps that have a specific assigned tile (e.g. link miners)
/// but where blocking all traffic is undesirable. Compared to
/// `mark_immovable`, stationed creeps can be displaced when truly necessary;
/// compared to `mark_working`, they will not be casually shoved aside by
/// normal-priority creeps.
#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn mark_stationed(tick_context: &mut JobTickContext) {
    let creep = tick_context.runtime_data.owner;
    let creep_pos = creep.pos();

    let mut builder = tick_context
        .runtime_data
        .movement
        .move_to(tick_context.runtime_data.creep_entity, creep_pos);

    builder
        .range(0)
        .priority(MovementPriority::High)
        .allow_shove(true)
        .allow_swap(false);
}

/// Register the creep as a stationary worker that prefers to stay put but may
/// be shoved or swapped to an adjacent tile as long as it remains within
/// `range` of `target_pos`. Use this for range-based workers (upgraders,
/// builders, repairers, etc.) so clustered creeps can rearrange without
/// deadlocking. The creep's work action (harvest, upgrade, build, etc.) is
/// not interrupted because MOVE and work intents use separate action slots.
#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn mark_working(tick_context: &mut JobTickContext, target_pos: Position, range: u32) {
    let creep = tick_context.runtime_data.owner;
    let creep_pos = creep.pos();

    let mut builder = tick_context
        .runtime_data
        .movement
        .move_to(tick_context.runtime_data.creep_entity, creep_pos);

    builder
        .range(0)
        .priority(MovementPriority::Low)
        .allow_shove(true)
        .allow_swap(true)
        .anchor(AnchorConstraint {
            position: target_pos,
            range,
        });
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn get_new_move_to_room_state<F, R>(creep: &Creep, room_name: RoomName, state_map: F) -> Option<R>
where
    F: Fn(RoomName) -> R,
{
    if creep.pos().room_name() != room_name {
        return Some(state_map(room_name));
    }

    None
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn tick_move_to_room<F, R>(
    tick_context: &mut JobTickContext,
    room_name: RoomName,
    room_options: Option<RoomOptions>,
    next_state: F,
) -> Option<R>
where
    F: Fn() -> R,
{
    let room_half_width = ROOM_WIDTH as u32 / 2;
    let room_half_height = ROOM_HEIGHT as u32 / 2;
    let range = room_half_width.max(room_half_height) - 2;

    let target_pos = RoomPosition::new(room_half_width as u8, room_half_height as u8, room_name);

    tick_move_to_position(tick_context, target_pos, range, room_options, next_state)
}

/// Stay at least this Chebyshev distance from a dangerous remote hostile.
/// Covers a Source Keeper's melee (range 1) + ranged (range 3) with margin;
/// tunable.
pub const THREAT_FLEE_RANGE: u32 = 5;

/// Whether a hostile creep can actually hurt a non-combatant — it has a live
/// `ATTACK` or `RANGED_ATTACK` part. Source Keepers and NPC invaders qualify;
/// a harmless enemy scout/claimer/hauler does not, so we never abandon work
/// for one.
fn hostile_is_dangerous(creep: &Creep) -> bool {
    creep
        .body()
        .iter()
        .any(|p| p.hits() > 0 && matches!(p.part(), Part::Attack | Part::RangedAttack))
}

/// Pure core of the flee reflex: the flee targets for the dangerous hostiles
/// within `range` of the creep. Split out so the range/danger filter is
/// unit-testable without the game runtime. `hostiles` is `(position, dangerous)`.
fn nearby_threat_flee_targets(creep_pos: Position, hostiles: &[(Position, bool)], range: u32) -> Vec<FleeTarget> {
    hostiles
        .iter()
        .filter(|(pos, dangerous)| *dangerous && creep_pos.get_range_to(*pos) <= range)
        .map(|(pos, _)| FleeTarget { pos: *pos, range })
        .collect()
}

/// Hostile creeps in `room_name` for the flee reflex — or `None` if the room
/// is **ours** (skip: towers + defenders handle threats there and we don't
/// interrupt mining for a covered incursion). Reads cached visibility, falling
/// back to the live room (treated as remote) when we have no `RoomData`.
fn remote_room_hostiles(room_name: RoomName, tick_context: &JobTickContext) -> Option<Vec<Creep>> {
    if let Some(room_entity) = tick_context.runtime_data.mapping.get_room(&room_name) {
        if let Some(room_data) = tick_context.system_data.room_data.get(room_entity) {
            if room_data.get_dynamic_visibility_data().map(|v| v.owner().mine()).unwrap_or(false) {
                return None;
            }
            if let Some(creeps) = room_data.get_creeps() {
                return Some(creeps.hostile().to_vec());
            }
        }
    }
    Some(
        game::rooms()
            .get(room_name)
            .map(|room| room.find(find::HOSTILE_CREEPS, None))
            .unwrap_or_default(),
    )
}

/// The flee targets for the dangerous hostiles within [`THREAT_FLEE_RANGE`] of
/// the creep — empty when there is nothing to flee. Reads cached `RoomData`
/// hostiles (skips our own rooms: towers + defenders handle those). Shared by
/// [`is_threatened`] and [`issue_flee`].
fn compute_flee_targets(tick_context: &JobTickContext) -> Vec<FleeTarget> {
    let creep_pos = tick_context.runtime_data.owner.pos();
    let hostiles = match remote_room_hostiles(creep_pos.room_name(), tick_context) {
        Some(hostiles) => hostiles,
        None => return Vec::new(), // our own room — defense handles it
    };
    let tagged: Vec<(Position, bool)> = hostiles.iter().map(|c| (c.pos(), hostile_is_dangerous(c))).collect();
    nearby_threat_flee_targets(creep_pos, &tagged, THREAT_FLEE_RANGE)
}

/// Whether a dangerous remote hostile (invader or Source Keeper) is close
/// enough to flee — the **transition guard into a job's `Flee` state** (P2.K0,
/// ADR 0018 §3.4). Detection only; issues no intent.
///
/// Remote miners and haulers need this because room-safety only gates *new
/// spawns*: a creep already in the room has no threat awareness and stands and
/// dies, repeatedly feeding kills — a net energy sink. Making flee a state
/// (entered via this guard) means the creep's **job** owns the move and it does
/// not compete with work intents (ADR 0008 §5 flag / ADR 0018 §2 principle 8).
#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn is_threatened(tick_context: &JobTickContext) -> bool {
    !compute_flee_targets(tick_context).is_empty()
}

/// Issue a flee from every nearby dangerous hostile and return `true`; return
/// `false` when none remain (safe to resume work). Call this from a job's
/// `Flee` state — it owns the move and competes with no other action, so it
/// needs no `SimultaneousActionFlags` guard. Reuses the rover flee primitive,
/// the same mechanism `squad_combat::flee_from_hostiles` uses.
#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn issue_flee(tick_context: &mut JobTickContext) -> bool {
    let flee_targets = compute_flee_targets(tick_context);
    if flee_targets.is_empty() {
        return false;
    }
    let creep_entity = tick_context.runtime_data.creep_entity;
    tick_context
        .runtime_data
        .movement
        .flee(creep_entity, flee_targets)
        .range(THREAT_FLEE_RANGE);
    true
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn tick_move_to_position<F, R>(
    tick_context: &mut JobTickContext,
    position: RoomPosition,
    range: u32,
    room_options: Option<RoomOptions>,
    next_state: F,
) -> Option<R>
where
    F: Fn() -> R,
{
    let creep = tick_context.runtime_data.owner;

    if creep.pos().in_range_to(position.clone().into(), range) {
        return Some(next_state());
    }

    if tick_context.action_flags.consume(SimultaneousActionFlags::MOVE) {
        let mut builder = tick_context
            .runtime_data
            .movement
            .move_to(tick_context.runtime_data.creep_entity, position.into());

        builder.range(range);

        if let Some(room_options) = room_options {
            builder.room_options(room_options);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(x: u8, y: u8) -> Position {
        Position::new(
            RoomCoordinate::new(x).unwrap(),
            RoomCoordinate::new(y).unwrap(),
            "W5N5".parse::<RoomName>().unwrap(),
        )
    }

    /// The reflex flees only hostiles that are BOTH dangerous (attack-capable)
    /// AND within range — not a harmless scout, and not a distant keeper.
    #[test]
    fn flees_only_nearby_dangerous_hostiles() {
        let me = pos(25, 25);
        let hostiles = [
            (pos(27, 25), true),  // dangerous, range 2 -> flee
            (pos(25, 28), false), // harmless scout, range 3 -> ignore
            (pos(40, 40), true),  // dangerous but range 15 -> ignore
        ];
        let targets = nearby_threat_flee_targets(me, &hostiles, THREAT_FLEE_RANGE);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].pos, pos(27, 25));
        assert_eq!(targets[0].range, THREAT_FLEE_RANGE);
    }

    /// No dangerous hostile in range -> the reflex is a no-op (work continues).
    #[test]
    fn no_nearby_danger_means_no_flee() {
        let me = pos(25, 25);
        let hostiles = [(pos(26, 25), false), (pos(45, 45), true)];
        assert!(nearby_threat_flee_targets(me, &hostiles, THREAT_FLEE_RANGE).is_empty());
        assert!(nearby_threat_flee_targets(me, &[], THREAT_FLEE_RANGE).is_empty());
    }

    /// A keeper at melee range (the respawn-next-to-miner case) is fled.
    #[test]
    fn keeper_at_melee_range_is_fled() {
        let me = pos(10, 10);
        let hostiles = [(pos(11, 10), true)];
        let targets = nearby_threat_flee_targets(me, &hostiles, THREAT_FLEE_RANGE);
        assert_eq!(targets.len(), 1);
    }
}
