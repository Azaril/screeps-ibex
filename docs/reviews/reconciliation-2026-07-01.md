# Reconciliation Review — 2026-07-01

**Scope:** all work landed 2026-06-08 → HEAD `d3fe2b9` (2026-07-01): ~540 superproject commits + ~240 submodule commits (combat-decision 87, combat-eval 72, rover 29, combat-agent 27, engine 12, sim-core 7, rover-eval 8), ADRs 0008a–0039, and the aggregator docs. Reviewed by six parallel deep-review lenses (ADR-vs-code reconciliation; objective/auction lifecycle; rally/travel/tactics; force-sizing/EV; rover-sim/movement/expansion; build/tests + cross-cutting invariants), findings verified against code at HEAD. **Code is ground truth** — doc claims were checked against it, not the reverse.

**How to use this doc:** §2 is the tracking checklist (work the milestones top-down; tick boxes as fixes land). §3 is the detailed findings ledger (REC-###) with evidence and fix specs. §4 is documentation debt (DOC-#). §5 is the reconciled ADR status table. §6 is the already-declared open work harvested from the docs (not new findings — the standing backlog). §7 is what NOT to re-flag. §8 is residual coverage gaps.

---

## 1. Verification status at HEAD `d3fe2b9`

**Build/test board — all green:**

| Lane | Result |
|---|---|
| `cargo test --workspace` (host) | **1143 passed, 0 failed**, 27 ignored (bot crate 238/238, combat-decision 311, combat-eval 107, combat-agent 52, rover 52+4, rover-eval 62, sim-core 33, engine 36, foreman 31, tool crates all green) |
| `cargo build-wasm -p screeps-ibex` | SUCCESS |
| `cargo clippy-wasm -p screeps-ibex` | 0 warnings |
| Working tree / submodule pointers | clean, zero drift (the session-start war.rs modification landed as `d3fe2b9`) |

**Cross-cutting invariants — verified good:** WFV bump history 13→23 exact and documented (`game_loop.rs:585-679`) *except REC-001 below*; entity-deletion discipline holds (all deaths via `EntityCleanupQueue`; direct `delete_entity` sites each carry a safety proof; `repair_entity_integrity` runs every tick before serialize); `get_creeps()` coverage complete; segment budget disciplined (8 always-active + 2 on-demand of 10, seg-58 always-active as designed) *except REC-046*; zero new statics since June 20; `features.rs` defaults all match documented intent, **no unsafe-for-MMO default found** (`attack_players=false`, debug/viz flags off, `reuse_path_length=20`); SquadTrace debug-gated *except REC-049*.

**Movement/sim invariants — all PASS:** determinism fence present (`sim_is_deterministic_over_rounds`, spread-0; note it is `#[ignore]`d — expensive-lane by design, runs only with `--ignored`); `DEFAULT_REUSE_PATH_LENGTH=20` on all three surfaces (rover, sim-core, `features.rs:271`); swap-tile double-booking fix + regression test present; `stationary_occupant` handling per M5 slice 5; ONE parked-registration default across domains, military-Immovable enum-vetoed before any numeric bid comparison (civilian w bids cannot outrank); budget `None`=unlimited with no regressed caller; ADR 0039 leaks nothing into the wasm build.

---

## 2. Fix tracking checklist

Work top-down. M-A gates **any** deploy (including Docker soak of current HEAD). M-B/M-C contain the P1s that explain "squads misbehave live" — fix and offline-prove (0028 lifecycle harness) before the planned P3 re-soak / P4 MMO deploy.

### M-A — Deploy blocker
- [ ] **REC-001** Bump `WORLD_FORMAT_VERSION` 23→24 (two unbumped serialized-shape changes in rover `StuckState`)

### M-B — Squad lifecycle liveness (immortal/zombie/trickle squads)
- [ ] **REC-002** Gate the Engaged-arm order overwrite on in-room presence (rally/quorum bypass)
- [ ] **REC-003** Bound `Retreating` (absorbing state + indefinitely refreshed lease)
- [ ] **REC-004** Stamp `lost_in_room` on first-contact LOSE; stop resetting the travel clock per room poke
- [ ] **REC-005** Producer re-assert must never wipe the commitment-lease deadline
- [ ] **REC-009** Reload-stable squad identity for member jobs (`SquadRef` → minted `SquadId` or per-tick re-stamp)
- [ ] **REC-016** Consume the kite/retreat goal in live retreat orders (stop centroid-huddling under fire)
- [ ] **REC-017** Renew-to-sufficiency actually reaches the D6a gate's required TTL; hold only at home rooms
- [ ] **REC-019** Border-tile "hold" must step inward (engine relocation bounces exit-tile holders)
- [ ] **REC-022** Gate the `has_focus` lease refresh on target-room presence
- [ ] **REC-036** Wire `enemy_stalled` (stalemate disengage can currently never fire)

### M-C — Defense valuation, auction & claim loop
- [ ] **REC-006** Value defense by the protected asset (remote income / owned-room value), not `energy_capacity_available` of the threat room
- [ ] **REC-007** Defense-ness derived from ownership, not the `Defend` enum variant (own-room unwinnable-backoff inversion)
- [ ] **REC-008** Defense claims must not queue behind the offense forming cap (and must not increment it)
- [ ] **REC-010** Gone-objective reassign rows: class/caps from the squad, not `CapClass::Offense` + defaults
- [ ] **REC-018** `project_defense`: real tower ranges, not hardcoded 25
- [ ] **REC-020** Reassign matrix: per-row travel from squad position; caps from surviving bodies
- [ ] **REC-021** Merge-donor cleanup (forming clocks, claim release, skip same-tick Phase-B spawns)
- [ ] **REC-023** Seed `claimed_by` from managed squads on the first post-reload tick (Duplicate-retire race)
- [ ] **REC-037** Reassign feasibility checks spawn range; log/count the no-home-in-range slot skip
- [ ] **REC-040** Whole-squad Reassign rebinds member jobs
- [ ] **REC-041** Producer re-assert sets (not only raises) priority

### M-D — Force sizing, structure threat & economics accuracy
- [ ] **REC-011** Make the EV optimizer's cost term bind (currency-consistent `w_energy`, or marginal-EV-per-energy selection)
- [ ] **REC-013** **[operator directive 2026-07-01]** Dismantle = first-class *structure*-threat channel in `EnemyForce`; size defense to kill-in-breach-window; never fold the danger proxy into creep-dps; heal never sized against WORK
- [ ] **REC-012** Fix the `hits==0` + heal-dominant "unwinnable" degenerate (veto on budget, not requirement)
- [ ] **REC-014** Score drain comps against standoff tower dps, not point-blank
- [ ] **REC-015** Unbuildable-slot stall logs loudly + sizing homes restricted to spawn-range homes
- [ ] **REC-031** Thread real enemy `hits` into `RaidCreeps` (kill-in-time term currently dead)
- [ ] **REC-032** Replace the three inline dps folds with the kernel estimators (inline folds ignore boosts)
- [ ] **REC-026** `BodyType::estimated_cost`/`part_count` cover CLAIM
- [ ] **REC-027** SK suppression cost constant → real sized-comp cost
- [ ] **REC-028** Reserver hold amortized over 600-tick CLAIM lifetime
- [ ] **REC-034** SK stronghold rescout fulfillment priority (LOW→MEDIUM or persisted-structure `has_known_core`)

### M-E — Expansion & movement
- [ ] **REC-024** Claim reach oracle prices hostile rooms like the claimer's mover does (or intersect homes with the BFS set)
- [ ] **REC-025** Claim commit gate checks plan *validity*, not just presence
- [ ] **REC-045** Sort-build the two remaining resolver collision maps (determinism defence-in-depth)
- [ ] **REC-050** Close the same-tick spawn-exit placement race

### M-F — Polish, hygiene & docs
- [ ] REC-029 R4 P(win) log wrong for drain/optimized comps
- [ ] REC-030 Stale `force_ceiling` rustdoc links
- [ ] REC-033 Safe-mode veto asymmetry between the two EV paths (drift trap)
- [ ] REC-035 Member travel trackers keyed by recyclable `Entity::id()`
- [ ] REC-038 Drain/siege anchor create-then-drop per-tick churn
- [ ] REC-039 Pin the three uncoordinated retreat HP bands
- [ ] REC-042 Stale `withdraw_removes_objective_and_runtime` doc comment
- [ ] REC-043 Declaim withdraw is a post-reload no-op
- [ ] REC-044 `objective_queue.rs:589` unwraps → `let-else`
- [ ] REC-046 Cost-matrix seg-55 writes route through the MemoryArbiter
- [ ] REC-047 `salvage.rs:816` expect → `let-else`
- [ ] REC-048 Pin the war.rs:2081 cross-crate "heal probe fieldable at 550e" invariant
- [ ] REC-049 Gate (or counter-ize) the ungated `[SquadTrace] MOVE-BLOCKED` line
- [ ] REC-051 Fix stale rover budget-contract comment line refs
- [ ] REC-052 Spawn-band soft spots (head-of-line blocking across homes; SK refill recurrence; CRITICAL-defense band tie)
- [ ] DOC-1..DOC-7 (§4)

---

## 3. Findings ledger

Severity: P1 = must fix before relying on the subsystem; P2 = wrong behavior with bounded blast radius; P3 = latent/polish. Confidence: CONFIRMED = failing path traced (several independently re-verified during synthesis); PLAUSIBLE = mechanism solid, live frequency unproven.

### P1

**REC-001 · P1 · CONFIRMED — Two serialized-shape changes landed after the WFV 23 bump without a bump**
`screeps-rover/src/movementsystem.rs:99-100` (`denied_by_idle`, rover `0535352` / super `5ef27bc`) and `:103-104` (`ticks_since_repath`, rover `5a9fe9e` / super `553a618`) — both **after** the 22→23 bump at `cf5e8be`. They sit inside `StuckState` → `CreepRoverData`, which is in the bincode world-save tuple (`game_loop.rs:498`). Bincode is positional; the project's own WFV doc (`game_loop.rs:627-635`) states trailing `#[serde(default)]` does **not** make old payloads decode safely — the in-code comment citing "the `last_distance` precedent" is wrong (that field shipped with the struct). A WFV-23 world saved pre-`553a618` and loaded at HEAD decodes as **silent misaligned garbage** — the exact failure class WFV exists to prevent. Blast radius today: private/Docker worlds (WFV 23 never MMO-deployed). **Fix:** bump 23→24 with a history entry; correct the rover comment. EP-5.2, EP-5.3, EP-3.1.

**REC-002 · P1 · CONFIRMED — Engaged-state order overwrite bypasses the entire rally/mass/quorum machinery for any scouted target**
`squad_manager.rs:2761-2803` (`apply_squad_decision`), `combat-decision/src/lib.rs:1507-1511`, `jobs/squad_combat.rs:203-239`. `compute_squad_orders` stamps rally-hold / solo-travel orders, then unconditionally calls `apply_squad_decision`, whose Engaged arm overwrites **every member's `tick_orders`**. `decide_squad` returns Engaged purely from `focus.is_some()` (computed over the target room's *cached* DTOs, no proximity gate) — FIX B1 gated only the `engaged_once` latch on `in_room_any` (:2769), not the orders. Members in transit fall through to individual `tick_move_to_room`; the rally, solo-travel-to-shared-rally, gather-quorum and ADR 0037 T2b anchor gates steer only the unused anchor. **Failure:** a forming 1/4 squad vs a scouted core departs solo member-by-member — the exact P-OBJ #23 trickle the ADR 0034 wave was built to kill; the RC-11/rally fixes are live only for no-intel or losing targets. **Fix:** gate the Engaged arm's order overwrite on `in_room_any` (stamp attack targets for in-room members only), or pass the manager phase in and let rally/travel orders take precedence. EP-2.7.

**REC-003 · P1 · CONFIRMED (gates) / PLAUSIBLE (frequency) — `Retreating` is an absorbing state with an indefinitely refreshed lease**
`lib.rs:1545-1563` (dead band: retreat at balance ≤ −200, re-engage needs ≥ +200 AND avg > threshold+0.3), `lifecycle.rs:278` (`has_focus ⇒ KeepRefreshLease`), `squad_manager.rs:2574-2579` (`lost_in_room` cleared on room exit), `:1440-1441`, `:1516-1531` (B-renew immortality), `objective_queue.rs:422-428`. A squad whose cached assessment sits in (−200, +200) retreats home and parks forever: focus still computed each tick → lease refreshed; abandon carrier requires `in_target_room`; Phase B refills and renews indefinitely. Immortal squad holds 1 of 4 squad slots, burns renew energy, zero telemetry. **Fix:** bound time-in-Retreating-without-re-engage (force-abort → `GaveUp` + `mark_unwinnable`), or lease-refresh requires `state != Retreating || in_target_room`. EP-2.7, EP-3.2. *(Interacts with REC-005/022 — fix as one lease-discipline slice.)*

**REC-004 · P1 · PLAUSIBLE — Abandon-on-unwinnable-contact never fires when first in-room verdict is already LOSE; travel budget resets per room poke**
`squad_manager.rs:2769-2771` (latch requires Engaged ∧ in-room; an instant-lose first contact goes Moving→Retreating without ever being Engaged), `:1213-1219` (`departed_at` deleted on `in_target_room`, re-stamped on resume → `MAX_TRAVEL_BUDGET` resets each cycle), `:1228-1239` (`cur == 0` counts as closing → lease refresh per re-entry). Border-tower geometry (towers ≤ ~20 from entry) produces an unbounded enter→instant-retreat→heal→re-enter loop ended only by old-age wipe — the E1 class ADR 0035 targeted, un-fixed for this geometry. **Fix:** (a) stamp `lost_in_room` on `in_room_any ∧ !present_wins_or_stalls` regardless of `engaged_once` once LiveVisible; (b) only clear `departed_at` on `engaged_once` or retire. EP-2.7, EP-4.5.

**REC-005 · P1 · CONFIRMED — Producer re-assert wipes the manager's commitment-lease deadline, disarming give-up**
`objective_queue.rs:287` (upsert: `existing.deadline = request.deadline`); all war.rs producers pass `deadline: None` (war.rs:618-627, 679-688, 775-785, 888-897, 1705-1710); the manager re-stamps only on `KeepRefreshLease`. Defense re-asserts every 1–2 ticks → `deadline_lapsed` is false forever → `gave_up` can never fire; claimed objectives never expire; forming/travel budgets only stop lease *refreshes*, but the lapse trigger itself is gone. Also breaks the ADR 0027 lease-bridges-VM-reset property, and the `set_deadline` doc claim ("re-stamps each tick") is false. **Fix:** `request()` never lowers/clears an existing deadline (`existing.deadline = existing.deadline.max(request.deadline)`); manager stays the sole lease writer for claimed objectives. EP-2.7.

**REC-006 · P1 · CONFIRMED — `value_e(Defend)` asset proxy starves ALL non-owned-room defense out of the EV gate**
`squad_manager.rs:632-639`, `1624-1630` (`asset_of` = live `energy_capacity_available`, `unwrap_or(1.0)`), `:515-517`, `objective_value.rs:93-104`, threshold at `:395`/`1644`. Non-visible room → 1.0; visible non-owned room → 0. So `value_e ≤ 0.91 < 1.0` before travel: every `Defend{remote}`, `Secure{neighbour}`, defend-flag and attack-flag objective is filtered from Phase C **and** loses to Recycle@0 in the reassign matrix. **Failure:** invaders hit a reserved remote → outpost economy gated off by `is_remote_room_safe` → the Defend objective is never claimed → no defender ever fielded → mining stays dead until invaders age out. The headline threat-centric defense of `4f41da8`+`2ee452c` is dead through the auction; offline proofs miss it because they inject synthetic intel (`asset_of` is live-only glue). The in-code claim "a fielded defense objective always clears the EV-positive floor" (:508-514) is false. **Fix:** value defense by the protected asset — remote: outpost income over downtime horizon (`room_net_roi` reusable); neighbour: adjacent owned-room value (the `emit_defense` kernel already computes it — thread it through). Split offense-`Secure` from defense-`Secure` for valuation/class. EP-7.2, EP-8.3, EP-6.11.

**REC-007 · P1 · CONFIRMED — `Secure`-as-defense unrecognized by the Defend special cases: a losing own-room defense marks OUR room unwinnable**
`squad_manager.rs:1122` (`is_defend` matches only `Defend{..}`), `lifecycle.rs:253`, war.rs:618-627 (owned-room defense now emitted as `Secure{owned}` since `4f41da8`). A lost own-room engagement → `GaveUp` + `mark_unwinnable(our room)` → 2,000–20,000-tick backoff during which Phase C and the reassign matrix skip every objective in our own base — the inverse of "never abandon an owned room". **Fix:** one ownership-derived defense predicate used by `is_defend`, the forming-cap exemption and `holding_station`. EP-2.9.

**REC-008 · P1 · CONFIRMED — Phase C blocks NEW defense claims behind the offense forming cap, and miscounts them**
`squad_manager.rs:1587-1603` (forming count exempts `Defend` only), `:1649` (`while … forming < MAX_FORMING_SQUADS` gates every claim), `:1680` (`forming += 1` unconditionally). Two routine offense squads forming + a fresh base assault → the CRITICAL defense objective cannot be claimed until an offense roster completes (up to 3,000t — effectively unbounded given REC-005). **Fix:** per-kind cap: defense-class claims bypass the forming gate and don't increment it; only `active < MAX_CONCURRENT_SQUADS` applies (consider defense preemption above the active cap — see REC-023 note). EP-4.3.

**REC-009 · P1 · CONFIRMED (chain) — Member jobs' `SquadRef` cannot survive a VM reload: every reload disbands all fielded squads**
`military/squad.rs:39-52` (raw index+generation, plain serde), `jobs/squad_combat.rs:14-19` (NOT marker-remapped), recall at `:1162-1179`/`:122-131`. After reload, marker-recreated entities get fresh indices/generations → every member job's ref fails validate-on-access → `recall_decision` walks the whole roster home to recycle mid-assault, then the manager respawns from scratch. The `SquadContext.members` side (ConvertSaveload) survives correctly; the job side defeats it. This is the planned identity I1/I2 work — upgraded here because the ADR 0027 recall path converts a stale ref from "idle" into active roster teardown. **Fix:** minted stable `SquadId` (persisted counter + per-tick id→Entity map), or manager re-stamps member jobs' binding each tick, or marker-remap the ref. EP-1.7.

**REC-010 · P1 · CONFIRMED — Gone-objective reassign rows default to `CapClass::Offense` with default caps**
`squad_manager.rs:648-650`, gate math `:764-769` (gone objective ⇒ `stay_ev = INFEASIBLE_EV` ⇒ any feasible column passes). `Reassign` fires precisely when the objective is gone — exactly when class/caps can't be read off it. A freed declaimer (CLAIM+MOVE only) prices `p_kill=1` against an intel-less core `Dismantle` and rebinds onto a target it cannot scratch, then stands forever (REC-005). **Fix:** class/caps from the squad itself (persisted target variant → class; live bodies → caps); unknown class ⇒ reassign-infeasible. EP-2.9.

**REC-011 · P1 · CONFIRMED (by arithmetic) — EV optimizer's cost term never binds: every defended target sized to the largest feasible force**
`composition.rs:458` (`OVER_POWER_LADDER [1.0,1.25,1.5,2.0]`), `:499` (`w_energy: 0.001`), `:600-615`; war.rs:56 (`target_value` ≈ 300k–600k), :46 (defense 1M). Δ(p_win·value) per rung is 10³–10⁵ energy-equivalents vs Δcost ≈ 3–15, so argmax is always the top rung that fits `MAX_SIZED_MEMBERS=8` — a systematic 50–100% over-invest on every defended commit, contradicting the operator's min-size directive and ADR 0031 D13/D17. The crate's own test had to set `w_energy: 0.01` (10×) to make the trade real (`composition.rs:992-994`). **Fix:** cost in the same currency as value (`w_energy ≈ 1.0` amortized over the objective horizon), or marginal-EV-per-marginal-energy selection, or cap the rung when `Δp_win·value < Δcost`. Re-run the 0031b sweep with a spawn-efficiency metric (check whether its beds were member-cap-bound). Related design tension: FIX-3 sizes any defended-no-repair structure target to the full 3-fighter ceiling (`force_sizing.rs:329-336`, deliberate scoping) — revisit together. EP-7.2, EP-4.6.

### P2

**REC-012 · P2 · CONFIRMED — `clear_force` degenerate: `hits==0` + heal-dominant enemy ⇒ "unwinnable" regardless of budget**
`force_sizing.rs:457-461`: with `hits==0`, `required_kill_dps = max(heal, dps·margin, 1.0)`; when heal dominates, `required == heal` exactly and the `required <= enemy_heal` gate fires. Live path always passes `hits: 0` for creep-clears (war.rs:1552-1558, doctrine.rs:238). One attacker + two healers permanently defers `GatedPlayerRaid`; `GarrisonDefense` collapses to the default floor. **Fix:** veto on budget (`budget.max_dps <= enemy_heal`), size `required = max(hits/window + heal·OVERCOME_MARGIN, dps·margin)` with margin > 1; pin the heal-dominant case. EP-7.2, EP-6.3.

**REC-013 · P2 · CONFIRMED — Dismantle threat mis-channeled: ranking proxy leaks into Lanchester sizing; no structure-threat channel exists** *(operator directive 2026-07-01 governs the fix)*
HEAD `d3fe2b9` folds `war_decision::dismantle_danger` (15/WORK, documented as an urgency proxy) into `need.estimated_dps` (war.rs:409) → `EnemyForce.dps` for `GarrisonDefense` sizing (war.rs:567). WORK can't damage creeps: one 25-WORK dismantler reads dps=375 → ~41 HEAL + ~57 RANGED parts ≈ 15k-energy phantom garrison; meanwhile the relevant quantity (its hits, and its ~1250/tick structure damage vs our ramparts/towers/spawns) is not threaded. Note: the rally-lens review initially judged this fold "deliberate and acceptable"; the sizing-lens review flagged it; **the operator has ruled**: dismantle must be a first-class *structure*-threat channel — `EnemyForce` gains dismantle-dps + attacker hits; the sizing kernel gets a structure-defense arm (kill the dismantler within the breach window = key-structure effective HP ÷ enemy dismantle rate, tower fire + repair counter-pressure included); heal is never sized against WORK; ranking/priority may keep the proxy, sizing must not. EP-2.4, EP-7.2. *(See memory: dismantle-threat-force-sizing.)*

**REC-014 · P2 · CONFIRMED (model) — Drain comps EV-scored against point-blank tower DPS the drain never faces**
`composition.rs:562-563, 600` (`p_survive` from `tower_dps_at_assault`) while the requirement was sized at `tower_dps_at_drain_standoff` (`force_sizing.rs:373-404`) and the oracle's `AssaultMode` verdict is discarded (`_assessment`, :546). Drain-winnable bases score `p_survive ≈ 0.02–0.08` → EV-deferred despite a winnable-Drain verdict; the ladder climbs buying heal that can't reach point-blank sustain (compounds REC-011). **Fix:** select the incoming model by the emitted assessment's mode. EP-7.2, EP-2.6.

**REC-015 · P2 · CONFIRMED — Roster stall on unbuildable slot is silent; sizing-home set ≠ spawn-home set**
`squad_manager.rs:1746-1760` (`build_body → None` warns only under `debug_log`; slot never queued; squad churns 3000-tick form/give-up cycles with zero telemetry). Trigger: `best_force_budget` (war.rs:2037-2064) sizes at ANY reachable home's capacity while Phase B filters to Chebyshev ≤ `MAX_SPAWN_DISTANCE=10` (:1729, 1790) — a comp sized at a strong 11-room home can be unbuildable at every in-range home. **Fix:** warn-once + seg-57 counter unconditionally; restrict `best_force_budget` to the same range filter. EP-3.2, EP-2.6.

**REC-016 · P2 · CONFIRMED — Live retreat = huddle at own centroid; the threat-priced kite goal is computed and discarded**
`squad_manager.rs:2774-2777` (`issue_retreat_orders(None, …)`), `squad.rs:782-829` (`compute_retreat_centroid` has NO away-from-hostiles bias despite its doc; cross-room member coords averaged into `living[0]`'s room = garbage), `lib.rs:2023-2042` (kite goal for Retreating computed in `decide_squad_with_pathing`, never read live — the sim honors it: sim/live parity broken for a behavior the sim "proves"). Retreating squads sit in tower range instead of withdrawing; genuine withdrawal only happens after retirement orphans members into `flee_from_hostiles`. **Fix:** consume `decision.movement` (Kite goal) in the Retreating arm, or retreat to the shared rally one room short; fix/delete the misleading doc. EP-6.4, EP-7.2.

**REC-017 · P2 · CONFIRMED — D6a lifetime gate creates permanent "zombie" slot members; renew can never reach the gate's required TTL**
`squad_manager.rs:2461-2496` (hold at *current position*, including mid-field where no spawn exists), `:1516-1524` (renew requested only when `ttl < 300` and at home), `rally.rs:118-127` (required TTL = dist·50 + 100; `RENEW_TARGET_TTL=1400` never drives any request). Targets ≥ ~4 rooms: held member saturates at ~300 TTL, never Commits, renewed forever; D8 quietly drops it from the quorum → squad fights at N−1 indefinitely, slot never replaced. Mid-journey re-evaluation can freeze a member in an open field until old-age death. **Fix:** renew condition = `ttl < required_for_deployment` (bounded by `RENEW_TARGET_TTL`) for held members at home; hold only in home rooms (else Commit or route to nearest home); terrain-aware or conservative per-room tick model. EP-7.2, EP-2.7.

**REC-018 · P2 · CONFIRMED — `project_defense` hardcodes tower `range_to_assault: 25` (up to 4× damage underestimate)**
`squad_manager.rs:444-449`. The EV pairing/claim/reassign path assumes minimum tower damage (150/tick) while war.rs oracle paths compute real ranges (war.rs:995, 1219, 1339) — the auction can reassign a squad onto a room the launch oracle would defer; only the T2b live-view veto backstops. **Fix:** compute `tpos.get_range_to(room_center)` from `hostile_tower_positions` as war.rs does. EP-7.2, EP-2.6.

**REC-019 · P2 · PLAUSIBLE — Transit-room "hold" on an exit tile still bounces (engine border relocation)**
`jobs/squad_combat.rs:1047-1065`: the `db4ad3c` hold branch returns `Some(creep_pos)`; a just-crossed member stands on the entry edge (x/y ∈ {0,49}), and the engine's unconditional border relocation (`move.js:32`, same mechanism as ADR 0033 M5) bounces it back. The fix stabilizes only members already ≥1 tile inside. **Fix:** when on the border ring, return a tile one step inward (clamp 1..=48). EP-7.1.

**REC-020 · P2 · CONFIRMED — v1.2 "global optimum" runs on a degenerate matrix (constant travel per row; full-requested-roster caps)**
`squad_manager.rs:703-708` (same anchor→room distance for every row — squad position never enters), `:649-650` (caps = claimed objective's requested comp at `homes.first()` energy, not surviving bodies). The Hungarian solver itself is a verified-correct JV O(n³) — but its inputs erase both squad-specific dimensions, so it can't prefer the adjacent squad, and a 1-of-4-survivors squad's StayPut is priced at full strength (blocks correct merges; overstates P(win) on reassign). Cousin: `sheddable_of` counts unspawned members while `apply_merges` transfers only present ones (merge lift over-priced). **Fix:** per-row travel from squad centroid room; caps from live member bodies (both already computed in Phase A). ADR 0032 deviation. EP-7.2.

**REC-021 · P2 · CONFIRMED — `apply_merges` deletes the forming-consolidate donor without retire-path cleanup**
`squad_manager.rs:811-911` (no `SquadFormingProgress` cleanup, no `release_entity`; cf. retire at 1349-1366), `:1455-1483` (Phase B still queues spawns for donor slots + the receiver's about-to-be-filled slot the same tick — surplus creep spawns then recalls: wasted energy + spawn occupancy). The next squad fielded on that objective inherits the donor's aged forming clock via `.entry().or_insert(now)` (:1205) — after repeated merges a fresh squad is budget-exhausted at birth. **Fix:** on a `MergeDecision`: clear donor per-objective trackers, skip Phase-B spawns for donors + pending-transfer slots, `release_entity` on full-shed. EP-2.7.

**REC-022 · P2 · PLAUSIBLE — `has_focus` lease refresh is not proximity-gated**
`lifecycle.rs:278`; focus computed from target-room DTOs regardless of squad position (FIX B1 gated only the latch). A squad permanently blocked en route to a visible/mapped hostile room refreshes its lease forever — independent of and compounding REC-005. **Fix:** gate the refresh on `in_target_room` (mirror FIX B1) or subject it to the travel budget. EP-2.7.

**REC-023 · P2 · PLAUSIBLE — First-post-reload-tick claim blindness can retire a live fighting squad as `Duplicate`**
`squad_manager.rs:701-713` (feasibility reads ephemeral `claimed_by`, empty right after reload), reassign apply :1375-1433, duplicate retire :1266. Post-reload: terminal squad A's row sees live squad B's objective as unclaimed → A rebinds + covers → B retires `Duplicate` mid-fight, members recalled. **Fix:** seed `claimed_by` from managed squads' serialized `objective_id` before the solve. Related deliberate-design note: defense has no preemption over `MAX_CONCURRENT_SQUADS` (4 live offense + base assault = no defender) — pre-existing; needs an explicit decision. EP-2.7.

**REC-024 · P2 · PLAUSIBLE — Claim reach oracle disagrees with the claimer's actual movement policy**
`pathfinderservice.rs:222-241` (`compute_route` room callback reads `game::rooms()` — remote rooms rarely visible → hostile corridors price at default 2.0, never denied) vs the live claimer's `HostileBehavior::Deny` (`pathing/movementsystem.rs:181-192`). ADR 0038 made `is_claim_feasible` the SOLE reach gate (claim.rs:794); compounding: commit-time home eligibility iterates ALL owned homes (:783-797), not the BFS's `candidate.home_rooms`, and `hops×50` has no terrain fidelity against a 50-tick margin ([CLAIM,MOVE] pays 5/swamp-tile). Claimers can be sent on routes they will refuse or can't survive → repeated deaths / `PathNotFound` loops — the soft-stall flavor ADR 0038 set out to kill. **Fix:** price the route callback from cached `RoomData` (deny hostile-owned, same predicate as the mover); intersect commit-time homes with `candidate.home_rooms`. EP-7.5, EP-7.2.

**REC-025 · P2 · PLAUSIBLE — Claim "defer-until-planned" gate checks plan *presence*, not *validity***
`claim.rs:756-763` (gate = `room_plan_data.is_none()`); the validity half lives only in `can_plan` at Discover (:146-150); `ClaimMission` has no plan check; a failed plan's `plan_score` → `None` → neutral 1.0 quality (scores ABOVE planned-but-mediocre rooms). A candidate whose requested plan fails during the scouting window → irreversible claim on an unplannable room; `should_abandon_claim` won't fire without hostiles → wedged GCL slot. **Fix:** at commit, `plan_data.valid() == false` ⇒ hard skip with a loud log. EP-2.9, EP-3.1.

### P3

**REC-026 · CONFIRMED** — `BodyType::estimated_cost`/`part_count` omit CLAIM (`composition.rs:87-118`): a Declaimer under-reports 2,400e at 4 CLAIM. Latent (Declaim bypasses optimizer/ROI); any future affordability read is wrong. EP-2.9.
**REC-027 · CONFIRMED** — `SK_DUO_BODY_COST = 5350` (`operations/sourcekeeper.rs:155`) vs actual doctrine-sized comp ≈ 9,000e / 3 members; SK net-ROI overstated ~2.4 e/t; stale figure repeated at `room_economics.rs:261`. The "refine in K2c" was never done. EP-4.6.
**REC-028 · CONFIRMED** — Reserver hold cost amortized over 1500-tick lifetime; CLAIM creeps live 600 (`room_economics.rs:180`; cf. `missions/utility.rs:60`) and net-positive reservation needs ≥2 CLAIM → ~1.7 e/t overstatement of reservable-remote ROI (over-values far remotes). EP-7.1.
**REC-029 · CONFIRMED** — war.rs:1666-1669 "P(win)" log divides by `HOLD_MARGIN` that drain-mode heal doesn't carry, and uses the pre-ladder requirement — logged confidence matches neither model nor fielded force. Log-only. EP-7.2.
**REC-030 · CONFIRMED** — Dead rustdoc links to deleted `force_ceiling` (doctrine.rs:11/97/448; composition.rs:30/295). No dead code (grep-verified). EP-10.5.
**REC-031 · CONFIRMED** — `RaidCreeps` kill-in-time term dead on the live path: doctrine.rs:238 threads `f.hits`, sole producer builds `EnemyForce { hits: 0 }` (war.rs:1552-1558); threatmap has per-creep hits to sum. The 0029-D7 "sized to clear in the window" behavior never engages. Fold into REC-013's EnemyForce work. EP-2.6.
**REC-032 · CONFIRMED** — Three hand-rolled dps folds (war.rs:400-409, :822-836) duplicate kernel estimators with literals `30/10/12` and **ignore enemy boosts** (threatmap models ×4) → owned-room boosted raider under-sized up to 4× on this path. Consider P2 if boosted raids are expected on MMO. EP-2.6, EP-7.1.
**REC-033 · CONFIRMED** — Safe-mode veto asymmetry: `pairing_p_win` short-circuits `safe_mode → 0.0` (composition.rs:714-716); `optimize_composition` relies on `emit_requirement` returning a zero vector. Drift trap if the coupling changes. EP-2.6.
**REC-034 · PLAUSIBLE** — SK stronghold rescout interval fix is correct + compile-fenced, but the fulfillment leg is soft: probe at `VISIBILITY_PRIORITY_LOW` (sourcekeeperfarm.rs:400-405) — the band FIX C had to escalate — and war.rs's HIGH backstop keys `has_known_core` on live-only `invader_cores()` (war.rs:1067-1072). Pre-RCL8 under visibility flood, the room can silently drop from `threat_rooms`. **Fix:** LOW→MEDIUM, or key on persisted `hostile_structures` (`sk_room_has_stronghold`). EP-4.3.
**REC-035 · CONFIRMED** — `member_rally_dist`/`member_solo_stall`/`member_target_dist` keyed `(ObjectiveId, Entity::id())` — index without generation (squad_manager.rs:104-120); a recycled index inherits the predecessor's stall streak. Small blast radius; the EP-1.7 smell (IBEX-002b class). **Fix:** key by creep name or prune against `ctx.members`.
**REC-036 · CONFIRMED** — `enemy_stalled: false` hardwired (squad_manager.rs:2084-2085, documented fast-follow): stalemate disengage can never fire; an out-healed turtle is ground until members age out — bounded but a full creep-lifetime of waste per generation, invisible (no counter). Wire it or count it. EP-2.7, EP-3.2.
**REC-037 · CONFIRMED** — `queue_slot_spawn` silently no-ops with no home in `MAX_SPAWN_DISTANCE` (squad_manager.rs:1727-1734; debug log covers only the build_body branch); `solve_global_reassignment` has no home-in-range feasibility (:617-726) unlike Phase C (:1666-1673) → a reassigned-far squad death-spirals N−1, N−2… silently. EP-3.3, EP-3.2.
**REC-038 · CONFIRMED** — Drain/structure-siege: formation branch re-creates the anchor (PathFinder call + possible `reassign_slots`) then drops it, every tick (squad_manager.rs:2506-2555). Functionally correct; wasted per-tick pathfinder work + layout churn. Check the drop predicates before the advance. EP-4.1.
**REC-039 · CONFIRMED (drift) / PLAUSIBLE (impact)** — Job-level HP bands (<40%/<50%, >80%/>60% at squad_combat.rs:258/440/821-824) vs squad bands (<25% critical + avg threshold) are three uncoordinated bands with no relation pin; a 25–50% member is job-Retreating while squad-Engaged stamps it Formation orders (interacts with REC-016). Lift bands into the decision crate + relation test. EP-2.9, EP-6.1.
**REC-040 · CONFIRMED** — Whole-squad Reassign rewrites `SquadContext` but never rebinds member jobs (`context.target_room` stays old; FSM not reset) unlike `apply_merges` (squad_manager.rs:1394-1407 vs squad_combat.rs:1136-1141); orders-missing fallback ticks walk members toward the old room. Rebind in the reassign arm.
**REC-041 · CONFIRMED** — Objective priority is max-merged and never decays while alive (objective_queue.rs:284-285); a de-escalated threat keeps historical CRITICAL for its whole life, inflating `priority_implied_danger` + spawn priority. Let the authoritative producer *set* priority.
**REC-042 · CONFIRMED** — `withdraw_removes_objective_and_runtime` doc comment describes the deleted v1.2-retired capability-aware selector (objective_queue.rs:836-847). EP-6.3.
**REC-043 · PLAUSIBLE** — Declaim withdraw is a no-op after a VM reset (ephemeral tracker, missions/salvage.rs:385-405); if the controller went neutral in the gap, the claim-immune `Declaim` squad holds a neutral controller indefinitely. Match by room when the tracker is empty, or re-verify controller state in the producer.
**REC-044 · CONFIRMED** — `objective_queue.rs:589-590` `.unwrap()`s in the per-tick cleanup system; provably Some today; `let-else` is the EP-3.4 shape.
**REC-045 · CONFIRMED** — Resolver defence-in-depth: two remaining last-write-wins collision maps (`resolver.rs:810-814` try_shove, `:666-671` resolve_swaps) — the exact HashMap-seed class hardened elsewhere (:326-341); border stacks are the documented real trigger. ~6 lines each (sorted-build/or_insert). EP-6.13.
**REC-046 · CONFIRMED** — Cost-matrix seg-55 written via raw `raw_memory::segments().set` (pathing/costmatrixsystem.rs:23), bypassing the arbiter's 10-key guard; safe only because the registry caps at exactly 10 — zero headroom. Route through the arbiter. EP-2.8, EP-9.7.
**REC-047 · CONFIRMED** — `salvage.rs:816` `survey_tuple.expect(...)` in the mission tick path; invariant real today, refactor-fragile. `let-else` + log. EP-3.4.
**REC-048 · CONFIRMED** — war.rs:2081 `.expect("a minimal heal probe is always fieldable at >=550")` — true today, spans a crate boundary, no pin. Add `assemble_force(heal4, 550).is_some()` pin. EP-6.3, EP-3.4.
**REC-049 · CONFIRMED** — `[SquadTrace] MOVE-BLOCKED` at squad_combat.rs:182 is `log::info!` outside the debug gate (deliberate ADR 0034 D4 surfacing, but repeats per blocked member per tick, no latch). Gate it or convert to a counted seg-57 metric + warn-once. EP-3.5.
**REC-050 · PLAUSIBLE** — Spawn-exit fix leaves a same-tick race: construction places an obstacle site on an approach tile of an idle spawn that *starts spawning later the same tick* with a direction set built from tick-start data → single-approach spawn can wedge at birth. Defer obstacle sites adjacent to any spawn whose room's spawn queue is non-empty. EP-6.11.
**REC-051 · CONFIRMED** — rover movementsystem.rs:589-590 cites stale ibex line numbers for the budget-contract call sites (now :331/:334). Contract doc for the None=unlimited semantics — keep accurate. EP-10.5.
**REC-052 · noted** — Spawn-band soft spots (no starvation found overall): (a) a banking 85-priority request head-of-line blocks the ≤75 lanes of EVERY in-range home on unaffordable ticks (shared token, squad_manager.rs:1789-1799 + spawnsystem.rs:434-435); (b) standing SK farm refill recurs in the 85 band each duo lifetime; (c) CRITICAL defense maps to the same 85 as MEDIUM offense — no intra-band edge for base defenders.

---

## 4. Documentation reconciliation (DOC-#)

The June-30 refresh (`61a31d3`) fixed the per-ADR headers; staleness now concentrates in the **aggregators**.

- [ ] **DOC-1 · P1** — `docs/plans/combat-overhaul-plan.md` self-declares "THE source of truth for combat/war STATUS and REMAINING WORK" but is frozen at the 2026-06-19 / WFV-14 era (code: WFV 23; "I identity UNSTARTED" vs `SquadRef` landed WFV 17; "S spawning UNSTARTED" is OBE; §4D pre-deploy framing vs deployed state; no mention of ADR 0022 or the 0025–0038 wave). **Fix:** update it to current state, or demote the SSOT claim with a pointer to ADR 0022 + per-ADR ledgers. Its still-accurate residue (worth keeping): `military_free` hardcoded (sourcekeeper.rs:345 TODO), `Escort` kind inert (game_loop.rs:670, no producer), `MAX_CONCURRENT_SQUADS=4` static, `decide_towers` genuinely unbuilt.
- [ ] **DOC-2 · P1** — ADR 0028 header "In progress / final offline gate" mislabels a complete harness; the real open item — K3 `slots_to_spawn` (fielding.rs:17) / K4 `claims_allowed` (claim_pacing.rs:15) have **zero bot call sites** — is tracked nowhere; the mid-stream "squads lose defended engagements" diagnosis has no recorded resolution measurement.
- [ ] **DOC-3 · P2** — `docs/design/README.md` index: zero rows for ADRs 0034–0039; 0033 row "Proposed" vs M0–M5 landed; 0019/0020/0032 rows contradict their own refreshed docs.
- [ ] **DOC-4 · P2** — ADR 0030 §6 pseudocode (`present_force_is_winnable`/`EngagementTempo`/`WAVE_DPS_MARGIN`) reads implementation-ready but none of it exists (only the interim quorum gate; 0.75-ratio reverted `9705b6a`). Add a NOT-YET-IMPLEMENTED banner.
- [ ] **DOC-5 · P2** — ADR 0025 "Accepted" hides the open action half: `action_oscillation_rate` metric absent; bot does not consume `member_intents` (kernel emits at lib.rs:2144, zero bot call sites) — tracked only in §11 #12.
- [ ] **DOC-6 · P2** — ADR 0026a header "Proposed / unvalidated" — validation happened 2026-06-26 (spacing SHIPPED strategy.rs:73-75; ranged_duel_kite REJECTED; rest deferred/superseded).
- [ ] **DOC-7 · P3** — Cell/fragment fixes: ADR 0020 §12.2 "OPEN — R-attack" vs §12.6+code DONE; ADR 0029 §7 table's PlayerRaid/GatedPlayerRaid conflation + §10 #2 "D7 held for review" (landed `efa3336`); ADR 0033 header line-2 vestigial "Remaining M4/M5" fragment; ADR 0038 "not yet committed" (committed at `cf5e8be`; only the MMO deploy/reset is pending); ADR 0025a residual object anomaly (~15–20%) has NO tracker/owner anywhere — assign one; ADR 0027 header date lag.

---

## 5. ADR status table (reconciled, code-verified)

| ADR | Claimed | Verdict | Notes |
|---|---|---|---|
| 0008a combat tactics | catalog v0 + readiness matrix | **VERIFIED** | DONE/PARTIAL markers match code exactly; Tier 0–3 items genuinely unbuilt (§6 backlog) |
| 0019 position selection | Complete 0–4, deployed | **VERIFIED** | |
| 0020 EV blob combat | Steps 1–4 complete, deployed; S5–S7 deferred | **VERIFIED** | one stale table cell (DOC-7) |
| 0025 EV position/action | Accepted, steps 1–3 | **PARTIAL** | step-4 action half open (DOC-5) |
| 0025a coordinate anomaly | Resolved, residual open | **VERIFIED** | residual untracked (DOC-7) |
| 0026 strategy selection | Implemented; §9.10 L1–L7 built | **VERIFIED** | L6c/L8 correctly deferred |
| 0026a candidate modes | Proposed/unvalidated | **STALE** | actually evaluated — mostly rejected/deferred (DOC-6) |
| 0027 objective lifecycle | Complete v1, deployed, WFV 18→22 | **VERIFIED** | WFV comments match ledger |
| 0028 lifecycle harness | In progress | **CONTRADICTORY** | harness complete; K3/K4 bot wiring is the real open item (DOC-2) |
| 0029 force composition | Core landed; D7 held | **VERIFIED core** | D7 actually landed+deployed (DOC-7) |
| 0030 size tuning | Proposed | **VERIFIED** | §6 pseudocode not built (DOC-4) |
| 0031(+a/b) capability composition | Accepted + implemented | **VERIFIED** | Tier-2 archetype dimension not built (top follow-up) |
| 0032 EV assignment | v1.1→v1.2→v2→v3 done | **VERIFIED** | README row lags (DOC-3) |
| 0033 rover sim | Implemented through M5 | **VERIFIED** | vestigial header fragment (DOC-7) |
| 0034 rally robustness | Phases 0–2 deployed; 3/4/1.5/F-C open | **VERIFIED** | but see REC-002/003/004 — the shipped gates are bypassed/absorbing in specific paths |
| 0035 scout-before-commit | E1+E2 deployed; FU1/FU2 deferred | **VERIFIED** | REC-004 shows an E1 residual for border-tower geometry |
| 0036 structure targeting | S1+S2 complete | **VERIFIED** | |
| 0037 tower-aware defense | T1/T2/T3 complete | **VERIFIED** | HEAD `d3fe2b9` extends it; REC-013 governs the sizing side |
| 0038 expansion reach/EV | "pending commit + deploy" | **STALE** | committed (`cf5e8be`, WFV=23); only MMO deploy pending; REC-024/025 |
| 0039 self-play sim | Proposed | **VERIFIED** | paused; nothing leaks into wasm |
| combat-overhaul-plan.md | source of truth | **CONTRADICTORY** | DOC-1 |
| design README index | per-row statuses | **STALE** | DOC-3 |

---

## 6. Declared-open work harvested from the docs (standing backlog — not new findings)

| Doc § | Item | Size |
|---|---|---|
| 0008a T0.1 | T-HEAL-3 reachable-heal filtering for winnability (estimated_heal sums ALL healers, threatmap.rs:315-322) | M |
| 0008a T0.2 | T-BREACH-3 net-repair WORK sizing (repair_per_tick=0.0 hardcoded, squad_manager.rs:458) | M |
| 0008a T1.3–1.6 | T-POS-3 MOVE back-load; T-DEF-4 CLAIM-attacker priority; T-BREACH-5 RMA shield-check; T-POS-5 exit-tile discipline in EV kernel | S–M each |
| 0008a T2.7–T3.10 | T-DEF-5 predictive safe-mode; T-DEF-1 rampart-anchored defenders; T-CTRL-1/T-TOWER-2/RMA-EV | M–L |
| 0008a boost layer | Boost availability + scaling (blocks T-COMP-1/5, T-TOWER-3, T-NPC-7, L3+ strongholds) | L |
| 0019 §8.1/St.4 | Boosted-TOUGH net-damage in ThreatField; weight-discriminating sweep bed; engage-stickiness fix | S–M |
| 0020 §11 | S5 blob auction (gated on R7 cross-goal EV currency); S5-CAP governor-dynamic MAX_CONCURRENT_SQUADS; S6 archetype classifier + meta-Nash; S7 adversarial room-gen; CAUTIOUS retune; per-archetype Lanchester n; P2.M2-LIVE anchor-mover live validation; P2.H5 Docker parity oracle; P2.L1 cross-room flee fallback | M–L each |
| 0025 §4/§11/§12 | action_oscillation_rate metric; #11 Declaim controller in decision view; #12 member_intents bot wiring; Stage-4 re-tune constant adoption | S–M |
| 0025a §2 | Root-cause residual object anomaly (~15–20% wall-aligned; snap_to_open masks) — NO owner | M |
| 0026/0026a | L6c DoctrineParams tuning; L8 coordination from observed bodies; new activation signals + scenario beds | M–L |
| 0028 | K3 `slots_to_spawn` + K4 `claims_allowed` bot wiring (zero call sites); defended-base engage measurements; multi-squad in forming driver | M each |
| 0029 §10 | run_defended_lifecycle harness proof; post-landing live re-soak (W9N8) | M |
| 0030 §9–10 | EngagementTempo phases 2–6; D12 delete silent-static fallbacks; D13 member_energy:0 root cause; D14 Sizing enum; D15 min_count floor; D16 delete dead bodies.rs template sizing; D17 defend-flag duo → GarrisonDefense; D18 SK cost constants (=REC-027); D19 lock-in test | S–L |
| 0031/0031a §5/§2B | Budget-free emit_requirement (retire optimizer_ceiling_budget); **Tier-2 weapon-archetype in EV search (the measured WORK-siege-vs-guard failure — highest-priority composition follow-up)**; tough_fraction tunable + tower bed; attack_to_heal_mix; engage_range as tuned dimension; escalate-vs-abandon on assemble_force=None (#38); FormationShape 5–8 members (2x2 overlay silently wrong); P6 re-sweep | M–L |
| 0034 §2.2/§3 | Phase-3 contested-oscillation on production churn path; phase-4 S1/S2/S3 as ParamScore gates; 1.5 objective-level abort for unreachable targets; F-C renewable-rally-bias live wiring; D4-F1 explicit recycle job; D4-F2 RALLY_TRAVEL_PER_ROOM terrain tuning (see REC-017) | S–M |
| 0035 §2 | FU1 explicit scout-first pipeline; FU2 committed-but-never-engages give-up — deferred pending soak evidence (operator 2026-06-30) | L |
| 0033 end-state | Military w-as-priority arm (unblock condition satisfied — war.rs merged); dense-crowd N≥40 ops-saturation chip; kite member-goal scoring excluding held tiles; MovementTickStats through kernel driver; **live Docker soak before MMO (operator go-ahead required)** | S–L |
| 0038 | MMO deploy of the WFV 22→23 (now →24, REC-001) loud reset | S |
| 0039 §3 | P1 real terrain into render driver; P2 formation-cohesion kernel extraction; P3 unified engine-backed self-play; P4 tournament render corpus (paused workstream) | S–L |
| plan §5 | U-TOWER `decide_towers` pure fn (genuinely unbuilt); K2c-2 real yield-to-defense predicate replacing `military_free:true` + farm retirement; K-RECONCILE shared ensure_source_mining + outpost defender → Defend objective; K4 SK mineral mining; W3 Escort producer; W2/W4 war supervisor trim/posture; power-bank farming (needs own ADR) | M–XL |

---

## 7. Do-not-reflag ledger (carried forward + new)

**Prior review (docs/reviews/ibex-review-report.md):** IBEX-009 (staticmine unwrap — guarded), IBEX-012 (SquadContext members ARE repaired pre-serialize), IBEX-022 (spawn body-sizing clamp present; **the spawn comparator is CORRECT — never reverse it**), IBEX-031 (lazy transfer generators — reservation accounting is sound), NaN-scouting-deadlock (guarded), wall/rampart peacetime maintenance (covered), attack.rs:615 (latent only). **IBEX-049: operator-DECLINED — do not re-propose.**

**Deliberate designs confirmed this review (not defects):** `PlayerRaid`/`ClearCreeps` always-field vs `GatedPlayerRaid` gated (operator-intended split); the dropped 20-tick DefendMission de-escalation latch (per-tick-optimal directive); determinism fence `#[ignore]`d (expensive-lane, EP-6.9); `Immovable`-hold `allow_shove(true)` subtlety (documented, correct); 0.2-CPU move reserve counting holds (acknowledged-conservative); FIX-3 ceiling sizing for defended structure targets (documented scoping — but revisit with REC-011); defense-no-preemption-over-active-cap (pre-existing design — decide explicitly, see REC-023 note).

**Verified-good highlights (reconciled — don't re-review):** Hungarian solver internals (JV, overflow-safe, permutation-invariant, brute-force cross-checked); same-tick double-fill guard (maintain-ordering verified); merge death-mid-transfer safety; objective-queue TTL/backoff/NaN mechanics; salvage P1/P2 producers + D6 live-core deferral; DefendMission deletion clean (WFV 21→22 carried); EnemyForce unification real (one channel; `estimated_attack_dps` + canonical harmlessness); economics kernel pure + P(win) gate genuinely wired (depart fast-path, count-quorum, balance_retreat, war.rs commit gates); §2(d) heal right-sizing sound; ADR 0035 D1/D2 tri-state TowerIntel coherent; tower damage curve single-sourced + energized-filtered everywhere; RC-11/T2b gates pure and pinned; Moving↔Retreating and rally-oscillation fixes are input-stabilizations (not latches) with parity pins; `shared_rally_point_for_members` deterministic + world-edge-safe; reconcile kernel precedence sound; give-up backoff exponential + clear-on-win; RAZE ranking + breach_redirect (shared Dijkstra — no one-off pathfinding); T1/T3 signals distinct from creep danger; fighter-first stable sort; spawn-exit fix core + controller-link top-up kernel + `room_net_roi` math independently recomputed; claim selection deterministic with input-order-independence pin; rover/sim invariants table §1.

---

## 8. Residual coverage gaps (verify before treating as cleared)

- V-1: kite EV kernel `dist_to_target` flood keyed `(u8,u8)` without room — does `plan_squad_ev` guard cross-room member positions? (flagged in passing, untraced)
- V-2: `scout.rs` visibility-fulfillment half of `d3352f2` — not deep-reviewed.
- V-3: screeps-combat-agent submodule FSM/mover internals (sim-side driver) — log-skimmed only.
- V-4: `jobs/utility/dismantle*.rs` (15eb4f0's job side) and `operations/salvage.rs` flow body — seam-reviewed only.
- V-5: field-level serialized-shape audit is complete for `cf5e8be..HEAD` and spot-checked for `296c973..cf5e8be`; earlier windows verified at bump-history level only.
- V-6: `war_decision.rs` kernel internals (emit_defense/neighbour_threats) — reviewed at call seams.
- V-7: 0025 §12 Stage-4 re-tuned constants — run recorded, adoption not; confirm current seeds are the intended ones.

## 9. Method note

Review ran at HEAD `d3fe2b9` (clean tree, zero submodule drift). Six independent lenses with cross-checks; disagreements resolved by direct code verification during synthesis (e.g. the REC-013 fold: one lens called it deliberate, one called it a defect; operator directive settled the end-state). Where a P(win)/EV number is cited, it was recomputed, not quoted (EP-8.3). Line numbers are as of `d3fe2b9` and will drift.
