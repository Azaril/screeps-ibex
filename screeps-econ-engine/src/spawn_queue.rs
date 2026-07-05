//! Pure spawn-throughput model — a deterministic mirror of the live `spawnsystem`'s per-room, per-tick
//! spawn loop (the head-of-line break at `spawnsystem.rs:379-418`), so the offline lifecycle harness can
//! reproduce the spawn-lane CONTENTION that throttles squad forming (the live "roster stuck at 3/5"
//! failure) and let us TUNE the combat-vs-economy spawn priority offline instead of guessing on Docker.
//!
//! No `game::*`, no ECS, no `HashMap` iteration — value-type math over a descending-priority `Vec`
//! exactly like `SpawnQueue` (the determinism fence: identical inputs → identical, reproducible output).
//!
//! **Shared home (ADR 0040 §D8 #3, M0):** moved verbatim from
//! `screeps-combat-decision::spawn_throughput` so the economy sim's spawn QUEUE policy and the combat
//! lifecycle harness consume ONE implementation (EP-2.6) — this is the sim spawn system's queue-policy
//! kernel (descending priority + head-of-line energy banking), the mechanism half being
//! [`crate::tick::resolve_econ_tick`]'s `SpawnCreep`. Superseded by the shared K4/queue kernel at M5b.
//! The only non-verbatim change: [`CREEP_SPAWN_TIME`] is re-exported from [`crate::constants`]
//! (one citation-pinned definition per engine constant).

/// One home's spawn capacity for one tick (mirrors the spawnsystem per-room facts: free spawns, current
/// energy, and the RCL energy ceiling).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HomeLanes {
    /// Spawns with no creep currently spawning (each can start one creep this tick).
    pub idle_spawns: u32,
    /// `room.energy_available()` — energy on hand right now.
    pub available_energy: u32,
    /// `room.energy_capacity_available()` — the RCL ceiling (max a body may cost here).
    pub energy_capacity: u32,
}

/// A queued spawn request (mirrors `SpawnRequest`'s load-bearing fields for the lane decision).
#[derive(Clone, Copy, Debug)]
pub struct QueuedSpawn {
    /// Higher wins. MILLI-e/t bid (ADR 0040 §D2, M5b — the live `SpawnRequest.priority` currency).
    /// Combat uses `spawn_priority_for`; economy is CRITICAL (miners) / HIGH (haulers) / …
    pub priority: u32,
    /// Total body energy cost.
    pub body_cost: u32,
    /// Body part count (drives the spawn duration).
    pub part_count: u32,
    /// Stable id for the request/slot — used to report which spawned and to de-dup across homes.
    pub id: u64,
}

/// A spawn that STARTED this tick at a home.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spawned {
    pub id: u64,
    /// Ticks until the creep finishes spawning (`part_count * CREEP_SPAWN_TIME`).
    pub completes_in: u32,
}

/// Engine constant: a spawn takes 3 ticks per body part (defined once, citation-pinned, in
/// [`crate::constants`]; re-exported here so `spawn_queue::CREEP_SPAWN_TIME` keeps resolving).
pub use crate::constants::CREEP_SPAWN_TIME;

/// Run ONE home's spawn step for ONE tick — a faithful mirror of `spawnsystem.rs:379-418`.
///
/// Requests are processed in DESCENDING priority order. For each idle spawn we take the next request:
/// - `body_cost > energy_capacity` → **skip** it (`continue`): this home can never afford it; try the next.
/// - `body_cost > available_energy` → **break**: head-of-line — the home reserves its energy for this
///   higher-priority request and spawns NOTHING below it this tick (the mechanism that strands a
///   lower-priority combat slot behind a not-yet-affordable economy creep).
/// - otherwise → **spawn**: debit `available_energy`, consume one idle spawn, emit a [`Spawned`].
///
/// Mutates `home` (energy + idle spawns consumed). Returns the spawns that started, in priority order.
pub fn spawn_step(home: &mut HomeLanes, queue: &[QueuedSpawn]) -> Vec<Spawned> {
    // Descending by bid — stable, mirroring `SpawnQueue`'s sorted `Vec` (never a `HashMap`). With
    // `u32` milli-e/t bids (M5b) the key sort is total — the old `partial_cmp`/NaN coalescing is gone.
    let mut sorted: Vec<QueuedSpawn> = queue.to_vec();
    sorted.sort_by_key(|q| std::cmp::Reverse(q.priority));

    let mut spawned = Vec::new();
    let mut idx = 0usize;
    while home.idle_spawns > 0 && idx < sorted.len() {
        let req = sorted[idx];
        if req.body_cost > home.energy_capacity {
            idx += 1; // unaffordable at this RCL even when full — skip, do not block the queue
            continue;
        }
        if req.body_cost > home.available_energy {
            break; // head-of-line: reserve for this higher-priority request; spawn nothing below it
        }
        home.available_energy -= req.body_cost;
        home.idle_spawns -= 1;
        spawned.push(Spawned { id: req.id, completes_in: req.part_count * CREEP_SPAWN_TIME });
        idx += 1;
    }
    spawned
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(priority: u32, body_cost: u32, id: u64) -> QueuedSpawn {
        QueuedSpawn { priority, body_cost, part_count: body_cost / 100, id }
    }

    #[test]
    fn affordable_requests_spawn_in_priority_order() {
        let mut home = HomeLanes { idle_spawns: 2, available_energy: 5000, energy_capacity: 5300 };
        // Lower-bid listed first to prove sorting, not input order, decides (milli-e/t bids).
        let out = spawn_step(&mut home, &[q(50_000, 1000, 1), q(75_000, 1000, 2), q(100_000, 1000, 3)]);
        assert_eq!(out.iter().map(|s| s.id).collect::<Vec<_>>(), vec![3, 2], "top-2 by bid spawn");
        assert_eq!(home.idle_spawns, 0, "both lanes consumed");
        assert_eq!(home.available_energy, 3000, "debited 2 x 1000");
    }

    #[test]
    fn head_of_line_break_strands_lower_priority_below_an_unaffordable_one() {
        // The crux of the live 3/5 stall: a higher-priority request the home can't YET afford (but is
        // within capacity) BREAKS the loop, so the affordable lower-priority combat slot never spawns —
        // even though a lane is free and the home could afford IT.
        let mut home = HomeLanes { idle_spawns: 1, available_energy: 1800, energy_capacity: 5300 };
        let economy_critical = q(100_000, 5000, 1); // miner, not yet affordable (banking up)
        let combat = q(87_500, 1800, 2); // affordable right now, but below the miner
        let out = spawn_step(&mut home, &[combat, economy_critical]);
        assert!(out.is_empty(), "the home reserves for the CRITICAL miner; combat is stranded");
        assert_eq!(home.available_energy, 1800, "nothing spent — energy held for the miner");
    }

    #[test]
    fn over_capacity_request_is_skipped_not_blocking() {
        // A request costing more than the RCL ceiling is skipped (continue), not a break — the next
        // affordable request still spawns. (Distinguishes the `continue` arm from the `break` arm.)
        let mut home = HomeLanes { idle_spawns: 1, available_energy: 3000, energy_capacity: 3000 };
        let too_big = q(100_000, 5000, 1); // > capacity: can never spawn here
        let combat = q(87_500, 1800, 2);
        let out = spawn_step(&mut home, &[combat, too_big]);
        assert_eq!(out.iter().map(|s| s.id).collect::<Vec<_>>(), vec![2], "combat spawns past the too-big one");
    }

    /// Drive the kernel over a small colony × window to REPRODUCE the live "roster stuck at 3/5" and prove
    /// the priority lever fixes it — the offline replacement for the Docker whack-a-mole. Two homes, one
    /// idle spawn each per tick, modest income; every tick the economy queues a HIGH hauler per home
    /// (logistics never sleeps — the constant pressure that holds the single lane). A 5-member combat
    /// roster (1800e each) competes for those lanes. At MEDIUM (below the hauler) the roster starves; above
    /// the hauler it completes. (A CRITICAL miner is intentionally NOT modeled here — when one is pending
    /// it head-of-line-blocks ALL sub-CRITICAL spawns regardless of the combat/hauler order, which is
    /// covered by `head_of_line_break_strands_lower_priority_below_an_unaffordable_one`; here we isolate the
    /// combat-vs-economy lane contest.)
    fn run_window(combat_priority: u32, ticks: u32) -> usize {
        let income = 250u32;
        let mut spawned_ids: std::collections::BTreeSet<u64> = Default::default();
        // 5 combat slots, ids 100..105.
        let mut homes = [
            HomeLanes { idle_spawns: 0, available_energy: 1500, energy_capacity: 5300 },
            HomeLanes { idle_spawns: 0, available_energy: 1500, energy_capacity: 5300 },
        ];
        for t in 0..ticks {
            for (h, home) in homes.iter_mut().enumerate() {
                home.idle_spawns = 1;
                home.available_energy = (home.available_energy + income).min(home.energy_capacity);
                // Constant HIGH economy demand: a hauler wants the lane every tick. Distinct id per
                // home/tick so it never dedups with the combat slots.
                let hauler = QueuedSpawn { priority: 75_000, body_cost: 1000, part_count: 10, id: 2_000_000 + (t * 10 + h as u32) as u64 };
                // The still-unfilled combat slots (de-duped across homes by id).
                let combat: Vec<QueuedSpawn> = (100u64..105)
                    .filter(|id| !spawned_ids.contains(id))
                    .map(|id| QueuedSpawn { priority: combat_priority, body_cost: 1800, part_count: 18, id })
                    .collect();
                let mut queue = vec![hauler];
                queue.extend(combat);
                for s in spawn_step(home, &queue) {
                    if (100..105).contains(&s.id) {
                        spawned_ids.insert(s.id);
                    }
                }
            }
        }
        spawned_ids.len()
    }

    #[test]
    fn roster_stalls_below_economy_and_completes_above_it() {
        // MEDIUM (50): the CRITICAL miner's head-of-line break + the HIGH hauler keep combat from winning
        // lanes — the roster stalls well short of 5 (the live "stuck at 3/5" shape).
        let med = run_window(50_000, 40);
        assert!(med < 5, "MEDIUM combat is starved by economy — roster does not complete (got {med}/5)");
        // Above economy (87_500, below the CRITICAL miner): combat wins lanes once the miner is sated and the
        // hauler is out-ranked — the roster completes within the window.
        let above = run_window(87_500, 40);
        assert!(above > med, "raising combat above economy spawns strictly more of the roster ({above} > {med})");
        assert_eq!(above, 5, "above-economy combat completes the 5-member roster (got {above}/5)");
    }
}
