# Ibex Review Report — 2026-06-09

> Output of the review driven by [`ibex-review-prompt.md`](ibex-review-prompt.md). Findings use the uniform block format. The Bug & Issue Register (§7) tracks suspected bugs for later individual deep-dives.
>
> **Verification applied:** seed rows IBEX-001..024 have been confirmed, located precisely, adjusted, or refuted against current code; reviewer findings are mapped onto matching seed ids and new findings assigned IBEX-025+. Verification verdicts (confirmed / adjusted / refuted, with confidence) have been applied throughout — refuted findings are never presented as confirmed.

## Executive Summary

- **Operator Field Report verdicts (A–H)** — root-mechanism hypothesis each:
  - **A — war/squad cohesion (quads scatter):** Cohesion breaks at TWO independent levels (confidence M). LEVEL 1 (dominant): the LIVE defense path (`SquadDefenseMission`) wires NO formation logic at all — `squad_entity=None` (`squad_defense.rs:455`), creeps built with the squad-LESS `SquadCombatJob::new`, so every defense creep runs `fallback_movement` and targets its own nearest hostile independently (`squad_combat.rs:591/606/627`). That IS scatter, by construction. `SquadAssault`/`SquadHarass` are orphaned dead code. LEVEL 2 (even on the offense `AttackMission` path that does coordinate): N independent per-member `move_to(slot).range(0)` toward a separately-advanced shared virtual anchor (`formation.rs:163` "No Follow intents are used"), gated SOFTLY — Strict advances on a 75%/3-tick quorum then drops to Loose after 15 ticks and never reliably re-tightens (`formation.rs:313-350`); `squad_is_cohesive` drops the offset requirement entirely once `strict_hold_ticks>=15` (`attack_mission.rs:752-756`). Friendly creeps are NOT impassable by default, so this is NOT a single-creep-routing bug — it is the formation/coordination architecture.
  - **B — operation/mission lifecycle hangs:** No wall-clock watchdog at operation or mission level; teardown is event-driven on "all members dead". `AttackMission` Rallying requires `squad_is_cohesive` (a scattered squad never satisfies it) while Rallying RENEWS members below TTL — so the only natural terminator (all-dead) is actively prevented, and the campaign neither engages nor tears down. Engaging has no timeout; `SquadDefenseMission` Defending exits only on `!has_hostiles && all_dead`, so a surviving squad lingers forever after the threat clears. The economy-collapse abort is DEAD CODE (`total_energy_invested` never incremented, `attack.rs:402`). `WarOperation` returns `Running` forever with no child-abort. Confidence H.
  - **C — CPU pathfinding death-spiral / load-shedding:** Three composing gaps (confidence M). (1) NO global CPU/bucket governor in `tick()` — only a segment-readiness early-return; the heaviest consumers (transfer matching, war recompute at cadence=1, gather BFS, per-candidate `pathfinder::search`) bypass the piecemeal local budgets. (2) Pathfinding leaks outside the movement budget: `find_route` unbounded with an unguarded direct caller (`RoomRouteCache::compute_route`), un-capped `pathfinder::search` per-candidate. (3) Ephemeral route/cost-matrix caches are wiped on a VM reset, so the first post-reset tick (already WASM-reinstantiation-heavy) re-runs the full route storm; `env.tick` is set BEFORE `run_systems` so a mid-tick abort does not force a clean reset. The movement subsystem itself is well-defended; the spiral originates outside it. **Extinction-class.**
  - **D — serialization brittleness:** Two compounding roots (confidence H). ROOT 1 — entity identity is unstable: the marker path reallocates fresh Entity indices on deserialize, while a marker-BYPASSING path stores a bare `entity.id()` u32 in JobData (generation erased) that silently aliases a different entity after reload/recycle. ROOT 2 — bincode is non-self-describing/positional, so reordering fields or appending a serialized field misaligns bytes; the documented `#[serde(default)]` back-compat is largely illusory under bincode (a truncated old payload has no "absent field" representation). Failure is SILENT (`game_loop.rs:508/533` → empty world), presenting as a spontaneous colony reset.
  - **E — ECS dangling entity refs:** `repair_entity_integrity` is structurally required by using recyclable specs Entity indices as the durable cross-subsystem key. It is a hand-maintained enumeration with a no-op default (`missionsystem.rs:140`), so a newly-added Entity-bearing field is silently uncovered (e.g. JobData's u32 squad ref, which the pass cannot help). Confidence H. The fix that deletes the entire failure class: key durable refs by stable game IDs (RoomName, ObjectId, explicit SquadId) — `EntityMappingData` and `CreepOwner` already prove the pattern in-tree.
  - **F — job FSM friction:** REFUTED that multi-transition is the double-fire source — each `run_job` threads ONE `SimultaneousActionFlags`, and `consume()` is check-and-set so guarded intents fire at most once. The real friction is (a) split-pass side effects (reservation in `gather_data` vs action in `tick()`), (b) UNGUARDED combat intents in `squad_combat.rs` (~16 bare `creep.attack/heal` calls bypassing `action_flags`, safe today only by luck of return value), and (c) `Option`-as-control-flow opacity with a silent 20-transition cap. Confidence M.
  - **G — single-creep routing (acceptable?):** CONFIRMED acceptable (confidence H). The router is CPU-bounded by five layered governors (50k-op ceiling, hard 80-CPU movement cap, find_route headroom gate, MAX_PATHFIND_ROOMS=16, tick-limit short-circuits) and degrades gracefully (keeps stale paths) rather than blowing the tick. It is NOT a hidden cohesion cause — friendly creeps are not impassable by default, so members do not route around each other as walls. Cohesion effort belongs in the formation/squad layer.
  - **H — world renderer corrupts all rendering:** The visual path has two unguarded failure modes that, under `panic="abort"`, abort the whole tick (confidence H for the chain). (1) The engine's `console.addVisual` THROWS when a per-target buffer exceeds 500 KB (room/all-rooms) or 1000 KB (map) — the prompt's "~16 KiB" is WRONG; (2) NaN/Inf raw-f32 room-visual coordinates serialize to JSON `null` and desync the buffer. The room-plan renderer adds the volume that breaches the limit; the global overlay is duplicated into every room's buffer; a throw skips end-of-tick `serialize_world`, so enabling the renderer can also silently halt persistence.

- **Top 5 must-fix** (severity + one-liner):
  1. **Critical — Cost-matrix segment 55 wiped every tick** (IBEX-013): `serialize_world`'s trailing-clear blanks seg-55 (the cost-matrix cache) on every normal-size-ECS tick, so the persisted cost matrix never survives a reset — full per-room rebuild lands precisely on the most CPU-starved post-reset tick, feeding Field Report C.
  2. **Critical — No global CPU governor / death-spiral has no circuit-breaker** (IBEX-003/IBEX-016): the only failure mode that can permanently kill the colony has no tick-level shed/early-return; the heaviest systems bypass all local budgets.
  3. **High — Squad cohesion is not enforced** (IBEX-001): defense squads have zero formation logic; offense squads string out under a soft quorum gate — the war system is untrustworthy.
  4. **High — Reachable tick-aborting panic on a Nuker withdraw TransferTarget** (IBEX-010): a raid-registered nuker withdraw hits `panic!` (`transfersystem.rs:208`); under `panic=abort` the whole tick aborts and `serialize_world` never runs.
  5. **High — No per-system panic isolation** (IBEX-025): any reachable panic aborts the tick, skips serialize_world, and persists partial heap mutations — the structural amplifier turning any per-subsystem panic into a whole-tick failure.

- **3 most fragile subsystems:**
  1. **Memory & serialization** — seg-55 wipe (Critical), positional-bincode format brittleness, silent deser-to-empty-world, raw-u32 entity aliasing. Zero tests guard any of it.
  2. **Combat & expansion missions / Military core** — two incompatible squad models, broken cohesion, dead economy-abort, no lifecycle watchdog, inert BoostQueue.
  3. **Operations (war.rs)** — all cadences hardcoded to 1, cold-cache route storm on reset, no force-abort of excess/stuck attacks.

- **Single biggest rewrite recommendation:** Introduce a **bucket-aware global CPU governor + a single budgeted pathfinding facade** that every caller shares, generalizing the already-mature movement budget rather than discarding it. It is the only change that closes the extinction-class failure mode (Field Report C), and it is the scheduler seam the runtime model rides on. It is non-format-breaking and should land early.

- **Competitive verdict:** Ibex is a competently-designed, feature-complete economic bot with a sophisticated tower-targeting and economically-gated-offense brain, but it is NOT yet top-tier: the war system cannot reliably convert an economic lead into territory (squads scatter, campaigns hang), expansion is route-blind, and there is no empire-level resource allocation. **The CPU death-spiral DOES make the current bot strategically non-viable at competitive scale** — there is no global governor, the heaviest consumers are un-budgeted, and a VM reset re-arms the storm on the most CPU-starved tick. A bot that can permanently die to its own CPU use is unviable however well it plays when healthy. Treat the governor as table-stakes; the bot is viable today only on a single colony with no sustained war.

## 1. Prioritized Findings

> **Two findings already verified during prompt prep** (format exemplars):

```
[INFO]               Spawn-queue priority ordering is CORRECT — do NOT "fix"
ID:                  (n/a — verified correct; guards against re-flagging. See IBEX-022 for the real, separate bug)
Subsystem:           Spawning, stats & helpers
Location:            spawnsystem.rs:85–94 (insert), :236–248 (consume)
Type:                Maintainability (false-positive guard)
Status:              Observed-fact (traced against code)
Evidence:            binary_search_by(|probe| new.priority.partial_cmp(&probe.priority)) keeps the vec DESCENDING (SPAWN_PRIORITY_CRITICAL=100.0 … NONE=0.0); the forward consume loop at :236 spawns highest-first. Insert 0,100,50 → [100,50,0]. Re-confirmed.
Impact:              None — behavior is correct. The real risk is "fixing" the non-idiomatic comparator and INTRODUCING an inversion.
Recommendation:      Do not change ordering. Optional: clarifying comment + unit test (CRITICAL spawns before NONE). Pursue the SEPARATE body-sizing question (IBEX-022) and NaN-coalescing instead.
Breaking-change?:    None
Rewrite-implication: keep-as-is
```

```
[HIGH]               staticmine container resolve().unwrap() — DOWNGRADED: guarded, latent only
ID:                  IBEX-009
Subsystem:           Jobs layer
Location:            jobs/staticmine.rs:180–182 (container_exists), :201 (.unwrap())
Type:                Tick-safety
Status:              Observed-fact (construct verified; REFUTED as reachable panic)
Evidence:            The unwrap at :201 is reached ONLY inside `if container_exists {` (:182), where container_exists = container_target.resolve().is_some() (:180). Within one tick resolve() is deterministic, so the re-resolve at :201 cannot return None when :180 returned Some. The container-gone path is handled by MoveToContainer/Harvest/Wait/FindContainer rediscovery.
Impact:              No reachable panic in current code (IBEX-009 DOWNGRADED Critical→Low cleanup). The unwrap is a code smell, not a live bug.
Recommendation:      Replace .unwrap() with `if let Some(c)=…resolve() { … } else { return None }` to remove the smell and protect future refactors. Do NOT prioritize as a survival bug.
Breaking-change?:    None
Rewrite-implication: keep-as-is
```

### Critical

```
ID:                  IBEX-013
[CRITICAL]           Cost-matrix segment 55 is wiped by serialize_world every tick
Subsystem:           Memory & serialization
Location:            game_loop.rs:434–455 (clear loop), :117 & :812 (tick ordering), :554 (COMPONENT_SEGMENTS), :594–596 (heap cache load); pathing/costmatrixsystem.rs:6,25,57
Type:                Persistence/Migration
Status:              Observed-fact (verdict: confirmed, confidence H)
Evidence:            COMPONENT_SEGMENTS=[50..55] ends at 55; COST_MATRIX_SEGMENT=55. serialize_world consumes one segment per 50KiB chunk then a trailing `for segment {set(segment,"")}` (453–455) blanks every UNCONSUMED segment. Typical ECS payload is 1–2 chunks, so the clear reaches seg-55 in the NORMAL case. CostMatrixStoreSystem (macro line 117) writes seg-55 earlier the same tick via raw_memory().segments().set; serialize_world runs at :812 strictly after. The in-heap CostMatrixCache hides loss until create_environment(:594) reloads seg-55 → empty → default.
Impact:              The persisted cost matrix is destroyed end-of-tick and never benefits a reset. On every VM reset the bot rebuilds each room cost matrix during the highest-CPU window (re-deserialize/WASM re-instantiation), feeding the Field Report C death-spiral. The 6th-chunk framing understates it: the clear is unconditional for normal small-ECS.
Recommendation:      Move COST_MATRIX off COMPONENT_SEGMENTS to a dedicated segment, OR skip clearing segments owned elsewhere. Add a compile-time assertion that COMPONENT_SEGMENTS and COST_MATRIX_SEGMENT are disjoint. Interim non-breaking: shrink COMPONENT_SEGMENTS to 50–54 after confirming payload under 5 chunks.
Breaking-change?:    Memory/format
Rewrite-implication: refactor (dedicated cost-matrix segment; per-owner reservation) → ../design/0002-serialization.md
```

```
ID:                  IBEX-010
[CRITICAL]           Reachable panic! on a Nuker-withdraw TransferTarget aborts the whole tick
Subsystem:           Transfer & market logistics
Location:            transfer/transfersystem.rs:208 (Nuker withdraw, the only reachable arm); raid.rs:101–118 (registers enemy structures incl. nuker as withdraw); haulbehavior.rs:439 (hot-path consume); Cargo.toml:16 (panic="abort")
Type:                Tick-safety
Status:              Observed-fact (verdict: adjusted Critical→High in reachability scope; the panic is real, the broad set is not all reachable, confidence M)
Evidence:            withdraw_resource_amount has `Nuker(_id) => panic!("Attempting to withdraw resources from a nuker.")` (:208). raid.rs request_transfer_for_structures iterates structures.all() and registers any has_store structure as a WITHDRAW (request_withdraw, :116); TryFrom<&StructureObject> maps StructureNuker → TransferTarget::Nuker (:331→410). An enemy nuker holding energy/ghodium in a raided room therefore reaches withdraw_resource_amount → panic at :208. NOT reachable today: the deposit-side Ruin/Tombstone/Resource panics (249–251) and the link panics (276–289) — every generator registers those only as WITHDRAW (handled) and raid.rs comments them out. With panic=abort the hook only logs, then the WASM module traps for the rest of the tick.
Impact:              An active raid against an enemy room containing a resource-holding nuker halts ALL remaining systems that tick and prevents serialize_world from running (state freezes at the prior good tick; partial heap mutations persist). Narrow in normal play but a colony-wide outage when it fires.
Recommendation:      Replace the panic! with `Err(ErrorCode::InvalidArgs)` + a one-shot log (the fn already returns Result). Longer term, type-split TransferTarget into withdraw-capable vs deposit-capable enums so the invalid pairing is unrepresentable (the //TODO at :207/:248 acknowledges this).
Breaking-change?:    None
Rewrite-implication: replace (type-split deposit/withdraw target enums) → ../design/0005-runtime-and-scheduling-model.md
```

### High

```
ID:                  IBEX-003 / IBEX-016
[HIGH]               No global CPU/bucket governor; pathfinding leaks bypass the local budgets (death-spiral)
Subsystem:           cross-cutting (Pathing)
Location:            game_loop.rs:703–709 (only early-return) & 633–814 (tick); screeps-rover/src/movementsystem.rs:1183–1199 (unbounded find_route) & :935 (repath_count written, never read); military/economy.rs:237 (un-CPU-guarded find_route caller); findnearest.rs:61–133; missions/localsupply/structure_data.rs:174–206
Type:                CPU
Status:              Observed-fact (verdicts: IBEX-003 confirmed; IBEX-016 find_route bypass confirmed, confidence H)
Evidence:            tick() never reads bucket()/get_used() to gate the main pass; the sole early-return is segment-readiness. Per-tick budgets exist ONLY in rover movement (movementsystem.rs:228–301), room planning, and can_execute_cpu (constants.rs:19–21, ~6–8 sites). find_route (screeps_impl.rs:119–136) passes no max_ops; in the rover it is headroom-gated but RoomRouteCache::compute_route (economy.rs:237) has no CPU guard and its callback does per-room game::rooms().get(). findnearest.rs runs the caller's full path per candidate with no cap; structure_data.rs runs pathfinder::search per spawn×target with no max_ops. repath_count is written (saturating_add) but never read as a cap.
Impact:              Un-budgeted searches spike CPU with no circuit-breaker; the sum of many small searches drains the 10k bucket; a VM reset re-arms the storm on the WASM-reinstantiation tick. The bot can permanently die to its own CPU use (Field Report C, extinction).
Recommendation:      Introduce a CpuGovernor resource (Normal/Conserve/Critical from bucket+trend) read at the top of every expensive system, and route ALL pathfinding through ONE budgeted facade sharing a per-tick ops pool (generalize the rover budget as the inner layer). Cap repath_count and surface Failed. Interim quick-win (None-breaking): add .max_ops to findnearest.rs and structure_data.rs; bucket-guard economy.rs:237.
Breaking-change?:    Behavioral
Rewrite-implication: replace (global governor + single budgeted pathfinding facade) → ../design/0004-cpu-governance-and-load-shedding.md
```

```
ID:                  IBEX-001
[HIGH]               Squad cohesion is not enforced; defense squads have no formation logic at all
Subsystem:           Combat & expansion missions / Military core
Location:            missions/squad_defense.rs:455 (squad_entity None), :207/:224 (squad-less SquadCombatJob::new); military/formation.rs:201–359 (soft quorum gate), :161–190 (N independent move_to); jobs/squad_combat.rs:576–646 (fallback_movement); missions/attack_mission.rs:732–776 (squad_is_cohesive self-relax)
Type:                Correctness
Status:              Observed-fact (Level 1 wiring) + Hypothesis (Level 2 desync, confidence H)
Evidence:            SquadDefenseMission (built from war.rs for ALL defense) sets squad_entity=None and uses SquadCombatJob::new, so get_squad_state/get_tick_orders/get_formation_target short-circuit to None and creeps independently target nearest hostile (squad_combat.rs:591/606/627). On the offense path, formation.rs:163 comment: "Every creep independently issues MoveTo … No Follow intents are used"; the anchor advances on a 75%/3-tick quorum (formation.rs:318) then permanently drops to Loose after STRICT_HOLD_MAX_TICKS=15 (313–315), and squad_is_cohesive drops the offset check once strict_hold_ticks>=15 (attack_mission.rs:752–756).
Impact:              Defense quads scatter by construction; offense quads string out under any contention (shove/fatigue/terrain) and enter combat dispersed. Because heal/focus require in-range, this is a correctness failure that makes the war system untrustworthy — exactly Field Report A.
Recommendation:      FIRST wire SquadDefenseMission onto SquadContext/new_with_squad (fixes the dominant break, quick-win). Then replace the virtual-anchor N-independent-MoveTo model with lead-follower + hard in-range wait-gates (the rover already exposes Follow/desired_offset), advancing only when every live member can step. Add a per-squad cohesion metric (max member spread, ticks-in-Loose).
Breaking-change?:    Behavioral
Rewrite-implication: replace (single-unit formation mover / hardened Follow with cohesion gate) → ../design/0003-behavior-modeling.md
```

```
ID:                  IBEX-002
[HIGH]               War/attack lifecycle has no wall-clock watchdog; living-but-stuck squads have no exit
Subsystem:           Operations / Combat missions
Location:            operations/war.rs:1441–1442 (WarOperation always Running); missions/attack_mission.rs:662–716 (Rallying needs cohesion), :644–647 (renews stuck members), :1023–1046 (Engaging needs living==0); missions/squad_defense.rs:289–384 (Defending no-exit); operations/attack.rs:391–422 (should_abort), :759 (total_waves only set in Execute)
Type:                Correctness
Status:              Observed-fact (verdict: confirmed for the lifecycle trace, confidence H)
Evidence:            Rallying→Engaging requires squad_is_cohesive (a scattered squad never satisfies it) while Rallying renews members below TTL 1200, so the all-dead terminator (all_squads_wiped) never fires. Engaging exits only when all squads are Complete (living==0) or wiped — no age timeout. SquadDefense Defending exits only on `!has_hostiles && all_dead`, so a surviving squad idles forever after the threat leaves. WarOperation returns Ok(Running) forever and only removes already-dead attacks; AttackOperation has no total-age abort and its wave watchdog is dead pre-Execute.
Impact:              A squad that cannot group (Field Report A) or cannot finish holds an AttackMission/AttackOperation open indefinitely, consuming spawn energy on renewals, never engaging and never tearing down — Field Report B. Defense squads accumulate as idle never-released missions after each threat clears.
Recommendation:      Add a per-state wall-clock budget (e.g. Rallying ~150t, Engaging ~400t, Defending-no-hostiles ~50t) forcing handle_wave_wipe/Cleanup; stop renewing members of a squad non-cohesive for >N ticks so all-dead can fire; make WarOperation a supervisor that age-aborts children; return Defending→Cleanup when has_hostiles is false. Quick interim: per-attack created-tick age abort in should_abort.
Breaking-change?:    Behavioral / Memory-format (adds per-state entry-tick fields)
Rewrite-implication: replace (supervised FSM with mandatory per-state deadlines) → ../design/0003-behavior-modeling.md
```

```
ID:                  IBEX-004
[HIGH]               Positional bincode + no version header: any serialized-field change silently breaks old snapshots
Subsystem:           Memory & serialization
Location:            serialize.rs:310–344 (encode/decode, no version); game_loop.rs:402/510 (bincode DefaultOptions), :508/:533 (silent failure); room/data.rs:84/:199 (serde(default) fields that don't work under bincode), :263–278 (RoomDataSaveloadData)
Type:                Persistence/Migration
Status:              Observed-fact / Hypothesis (verdict: confirmed, confidence H)
Evidence:            bincode 1.x DefaultOptions is positional/ordinal with no field framing; reordering a struct field, inserting an enum variant mid-list (JobData 11 / MissionData 24 / OperationData 6), or appending a trailing field misaligns every subsequent byte. serde(default) is actually RELIED UPON (room/data.rs:84 `exits`, :199 `tower_dps_at_edge` — the latter added in the most recent feature commit) but CANNOT fire on a truncated bincode stream: old buffers lacking those bytes read the Option tag from the next component and cascade into garbage/error. decode failure → empty Vec (508); deserialize error → log+continue (533).
Impact:              Routine schema edits silently invalidate stored segments, presenting as a spontaneous full colony reset with only a log — the operator's "repeated breakage" (Field Report D). The documented #[serde(default)] migration story is illusory for the actual format.
Recommendation:      Stage 1 (any format, non-breaking to ship): add a version header, reject-and-reset deterministically on mismatch instead of silently consuming garbage; add round-trip + old-snapshot-corpus + fuzz tests (the encode/decode helpers are pure — a MemoryArbiter double makes the pipeline testable today). Stage 2 (rewrite): migrate the body to a tagged/schema-evolving format.
Breaking-change?:    Memory/format
Rewrite-implication: replace (versioned explicit/tagged serialization) → ../design/0002-serialization.md
```

```
ID:                  IBEX-025
[HIGH]               No per-system panic isolation: one panic aborts the tick and skips serialize_world
Subsystem:           Tick orchestration & ECS core (cross-cutting)
Location:            game_loop.rs:135–151 (run_systems), :791–812 (post-pass), :746 (env.tick set before run_systems); Cargo.toml:16 (panic="abort"); panic.rs:23–56
Type:                Tick-safety
Status:              Observed-fact (verdict: confirmed, confidence H)
Evidence:            run_systems calls each system's .run_now(world) sequentially with NO catch_unwind (zero in crate). panic="abort"; the panic hook only formats a backtrace and log::error!s, then the WASM module aborts. cleanup_memory (797) / repair_entity_integrity (806) / serialize_world (812) are plain calls AFTER run_systems returns, so an abort mid-pass skips all three. env.tick=Some(current_time) is set at :746 BEFORE run_systems, so next tick's discontinuity check does NOT force an environment rebuild — partial heap mutations persist.
Impact:              Any reachable panic (e.g. IBEX-010 nuker withdraw, attack_mission get_room .expect, a visual addVisual throw) aborts the entire tick: subsequent systems skipped, world NOT serialized that tick, partial in-heap mutations persist. A recurring panic is a silent total stall with no telemetry beyond a log line. This is the structural amplifier behind several survival-critical failure modes.
Recommendation:      Runtime-model decision: prefer narrowing risky boundaries to Result/log-and-continue (transfer target enums, visual flush) PLUS a single tick-level catch_unwind/JS try-catch boundary so serialize_world always runs even if a system aborts. Add a panic counter to a metrics segment for early warning.
Breaking-change?:    Behavioral (a panic=unwind profile change, if chosen, is a build-config change not a Memory/format break)
Rewrite-implication: refactor (per-system isolation / tick-level abort containment) → ../design/0005-runtime-and-scheduling-model.md
```

```
ID:                  IBEX-021
[HIGH]               War cadences all hardcoded to 1: per-tick threat scan + O(A·H) cold-cache route rebalance
Subsystem:           Operations (campaigns)
Location:            operations/war.rs:139–141 (DEFENSE/OFFENSE/RECOMPUTE_CADENCE=1), :546–930 (run_offense_evaluation), :1097–1241 (reassign_home_rooms), :626–630 (clones every RoomThreatData), :1314–1316 (should_run_tier)
Type:                CPU
Status:              Observed-fact (verdict: confirmed, confidence H)
Evidence:            const DEFENSE/OFFENSE/RECOMPUTE_CADENCE=1; should_run_tier returns true every tick. Doc comment (109–112) intends 1–2 / 10–20 / 50+. So run_heavy_recompute (incl reassign_home_rooms's O(attacks×homes) get_route_distance matrix at :1132–1147) and run_offense_evaluation (clones every RoomThreatData at :626–630) run every tick. RoomRouteCache is ephemeral; on a cold cache (post-reset) each get_route_distance is an uncapped find_route. The project's own can_execute_cpu governor exists but War never calls it.
Impact:              Heavy per-tick CPU scaling with owned-rooms × threat-rooms × active-attacks; cold find_route storms after reset land on the most CPU-stressed tick — a primary contributor to the Field Report C bucket-exhaustion spiral, uncoordinated with any governor.
Recommendation:      Restore intended cadences (defense ~2, offense ~10–20, recompute ~50) or gate by bucket via can_execute_cpu; restructure the offense eval to borrow RoomThreatData immutably or collect only the small fields used, instead of full clones. Verify with features.system_timing per-tier CPU.
Breaking-change?:    Behavioral
Rewrite-implication: refactor → ../design/0004-cpu-governance-and-load-shedding.md
```

```
ID:                  IBEX-026
[HIGH]               AttackOperation economy-collapse abort is dead code (total_energy_invested never set)
Subsystem:           Operations (campaigns)
Location:            operations/attack.rs:402 (guard `total_energy_invested > 0`), :87 (decl), :137 (init to 0); :539/:542 (describe reads)
Type:                Correctness
Status:              Observed-fact (verdict: adjusted High→Medium; dead-code claim confirmed H, severity bounded by max_waves + spawn gate)
Evidence:            total_energy_invested is declared at :87, initialized to 0 at :137, read in should_abort at :402 and describe at :539/:542 — but NEVER written to a non-zero value anywhere in the crate. The economy-deterioration branch `if self.total_energy_invested > 0 && self.estimated_total_cost > 0` is therefore unreachable. The sibling estimated_total_cost IS assigned (:687), confirming the asymmetry. should_abort's only live exit is total_waves >= max_waves (default 3).
Impact:              An attack that has begun spending but whose home economy then collapses will NOT abort on economy grounds; only max_waves stops it. Contributes to Field Report B (campaigns that neither progress nor tear down) and wastes energy on unwinnable targets under stress. Blast radius bounded by max_waves=3 and a mission-layer stored-energy spawn gate.
Recommendation:      Either accumulate real per-wave spend into total_energy_invested, or re-gate the branch on `total_waves > 0` / `estimated_total_cost > 0`. Add a log when the economy-abort branch fires to confirm reachability.
Breaking-change?:    Behavioral
Rewrite-implication: refactor
```

```
ID:                  IBEX-015
[HIGH]               Job-layer stuck recovery is entirely absent: check_movement_failure is dead code
Subsystem:           Jobs layer / Pathing
Location:            jobs/utility/movebehavior.rs:17–26 (definition); zero call sites crate-wide; jobsystem.rs:90/123 (movement_results plumbed into JobTickContext but never read by a job)
Type:                Correctness
Status:              Observed-fact (verdict: confirmed, confidence H)
Evidence:            check_movement_failure() returns Some(MovementFailure) on MovementResult::Failed or Stuck>=STUCK_REPORT_THRESHOLD; a crate-wide grep finds ONLY the definition. Every working state (tick_repair/tick_build/tick_harvest/tick_move_to_position) unconditionally move_to + return None with no escape, no stuck_count, no abandon. The rover PRODUCES the results but the sole reader is the uncalled function.
Impact:              A creep the rover gives up on (path-not-found or stuck) keeps re-issuing the same move forever; it never self-deassigns, repaths at the job level, or yields its slot. High/Immovable-priority creeps can cascade-freeze a corridor with no recovery — feeds Field Report B (lifecycle hangs) and the repath-storm side of Field Report C.
Recommendation:      Wire check_movement_failure into every working/move state: on Some(MovementFailure), transition to Wait/Idle/abandon-target and (for assigned-target jobs) drop the target. Add a per-job stuck_count escalating to self-deassign after N failures. Validate: spawn a creep with an unreachable target; assert it leaves the move state within N ticks.
Breaking-change?:    None
Rewrite-implication: refactor → ../design/0003-behavior-modeling.md
```

```
ID:                  IBEX-027
[HIGH]               BoostQueue boost-economy subsystem is entirely inert (dead path)
Subsystem:           Military core
Location:            military/boostqueue.rs (request/mark_ready/is_ready/pending_requests/clear, zero external callers); inserted game_loop.rs:606; plumbed missions/missionsystem.rs:36/61/187/242; dead aggregator composition.rs:552
Type:                Strategy
Status:              Observed-fact (verdict: adjusted High→Medium; inert end-to-end confirmed H, but dead scaffolding not an active fault)
Evidence:            BoostQueue is inserted as a resource and never read/processed/cleared in the loop. Its public API has ZERO call sites outside boostqueue.rs; no mission populates a BoostRequest and nothing produces a BoostAllocation. composition.rs::required_boosts() returns boost lists nothing consumes. The required_boosts aggregator (composition.rs:552) has zero external callers.
Impact:              Boosted bodies (BoostedQuadMember/Tank/etc.) are never boosted, so military spawns unboosted even when compositions demand T3 boosts — under-powering siege/defense vs boosted opponents. A strategic capability gap, not a live regression.
Recommendation:      Decide wire-or-delete: if wiring, add a lab/terminal producer fulfilling requests + a spawn/deploy gate on is_ready; otherwise delete the dead compositions and queue.
Breaking-change?:    None
Rewrite-implication: replace (real boost-allocation pipeline tied to labs) or drop boosted compositions
```

```
ID:                  IBEX-002b (combat-1 / mem-2 / jobs-2 / mil-5 — same root)
[HIGH]               Creep→squad link persisted as a non-remapped raw entity index (silent aliasing)
Subsystem:           Combat & expansion missions / Jobs / Memory
Location:            jobs/squad_combat.rs:18 (squad_entity: Option<u32>), :904 (id()), :835/:915/:974 (entities.entity(id)); jobs/data.rs:44–49; cleanup.rs:145–149; game_loop.rs:168–369 (repair omits JobData)
Type:                Persistence/Migration
Status:              Hypothesis → Observed mechanism (verdicts: mem-2 confirmed H, tick-2/combat-2 adjusted to Medium blast radius)
Evidence:            squad_entity stores Entity::id() (bare index, generation stripped) and is resolved via entities.entity(id), which attaches the CURRENT live generation at that slot. JobData is plain serde for this u32, so the marker remapper never touches it and the repair pass cannot validate it. After deserialize the squad entity gets a fresh index; within a session a recycled index (squad entity deleted as auxiliary child at cleanup.rs:281) can host a different SquadContext.
Impact:              After a VM reset (frequent per the reset model) every squad creep loses or mis-acquires its SquadContext → None → solo fallback (squads scatter, Field Report A); worst case a recycled index resolves a DIFFERENT live squad's orders. Dominant outcome is graceful degradation, so blast radius is bounded (Medium), but it is a real, silent cohesion-breaker.
Recommendation:      Persist a generation-carrying handle {index, generation} resolved through one validate-on-access helper (Behavioral interim, closes the aliasing), then key squads by a stable minted SquadId. Add a round-trip test asserting a creep resolves the SAME logical squad after reload.
Breaking-change?:    Memory/format
Rewrite-implication: replace (typed generational handle / stable-id squad store) → ../design/0001-entity-model.md
```

### Medium

```
ID:                  IBEX-014
[MEDIUM]             Deser failure unrecoverable + silent chunk-drop on segment exhaustion
Subsystem:           Memory & serialization
Location:            game_loop.rs:436–451 (chunk loop, "Not enough segments" log-and-continue), :508 (decode→empty Vec), :533 (deser error→log+continue), :495–497 (stale "will panic" NOTE); memorysystem.rs:94–97
Type:                Persistence/Migration
Status:              Observed-fact (verdict: adjusted High→Medium; reachability behind a very high payload threshold, confidence M)
Evidence:            serialize_world chunks encoded data via .chunks(50KiB); when segments run out it only error!-logs and drops remaining chunks. Deserialize: decode failure → Vec::new() (508), bincode error → log+continue (533); missing segments filter_map'd (501–505). The payload is gzip+base64 BEFORE chunking, so 300KiB of compressed output ≈ MiB of raw bincode — a very large empire, not normal play. The memorysystem.rs:95–97 oversize guard cannot fire from serialize (chunks are pre-split ≤50KiB). The inline "will panic" NOTE at :495–497 is STALE — the read path cannot panic on a missing segment.
Impact:              Silent partial state loss past ~6×50KiB compressed, then a truncated bincode that fails next deserialize → empty world; no watermark or telemetry. Real but atypical-scale.
Recommendation:      Emit encoded size + chunk count to a metrics segment; treat overflow as a hard, loud error; rewrite the stale NOTE to the actual log-and-continue policy. Add a chunk-count check before decode.
Breaking-change?:    None
Rewrite-implication: refactor (fail-loud overflow + telemetry) → ../design/0002-serialization.md
```

```
ID:                  IBEX-028
[MEDIUM]             No force-abort of excess AttackOperations when max_concurrent_attacks shrinks
Subsystem:           Operations (campaigns)
Location:            operations/war.rs:559–569 (capacity gate blocks NEW launches only), :937–949 (recomputes cap), :1297–1312 (cleanup_dead_attacks removes dead only)
Type:                Strategy
Status:              Observed-fact
Evidence:            max_concurrent_attacks is recomputed from economy each recompute; its only consumers are the launch guards (len() >= cap -> break). Nothing iterates active attacks to cancel the lowest-value ones when the cap drops (e.g. room abandonment reduces room_count). Each AttackOperation self-aborts only via its own should_abort.
Impact:              After losing a room (cap drops), already-running attacks continue indefinitely beyond the new budget, over-committing spawns/energy (Field Report B).
Recommendation:      When active count exceeds the freshly computed cap, force-Success the lowest-scored/least-progressed AttackOperations. Validation: drop room_count in a scenario and assert active attacks are trimmed.
Breaking-change?:    Behavioral
Rewrite-implication: refactor
```

```
ID:                  IBEX-005
[MEDIUM]             repair_entity_integrity is a hand-maintained enumeration with a no-op default
Subsystem:           Tick orchestration & ECS core
Location:            game_loop.rs:168–369 (7 blocks, multi-borrow-scope); missions/missionsystem.rs:136–140 (repair_entity_refs default no-op)
Type:                Architecture
Status:              Observed-fact (verdict: confirmed; the pass is required by the index-as-key choice)
Evidence:            The pass walks RoomData.missions, MissionData owner/room/children + internal, OperationData internal, and SquadContext.members/heal_priority every tick solely so ConvertSaveload does not panic on a dangling Entity (doc 153–167). Mission/Operation::repair_entity_refs defaults to no-op (:140); nothing guarantees a new Entity field is covered. JobData's raw-u32 squad ref is uncovered by design (IBEX-002b). NOTE: the prompt's "5-phase" is loose — it is 7 blocks; the 168–369 range is exact.
Impact:              Recurring CPU + complexity + a maintenance hazard: every new entity-ref component must be added to the scan or it dangles. Ties Field Report E to D.
Recommendation:      In the rewrite, key intra-ECS refs by stable game/persistent IDs with lookup-miss handling, then DELETE this pass. Interim: derive the repair list from a registry, and add a test asserting every serialized component with an Entity field is covered.
Breaking-change?:    None (interim) / Behavioral (store cutover deletes the pass)
Rewrite-implication: replace (stable-id store removes the need for the pass) → ../design/0001-entity-model.md
```

```
ID:                  IBEX-029
[MEDIUM]             squad_combat fires combat intents UNGUARDED by SimultaneousActionFlags
Subsystem:           Jobs layer
Location:            jobs/squad_combat.rs:994 (UNSET created, never consumed for combat) + action sites :200,:216,:234,:401,:419,:438,:478,:542,:551,:559,:673,:745
Type:                Tick-safety
Status:              Observed-fact
Evidence:            Every combat action is a bare `let _ = creep.attack/ranged_attack/heal(...)` with no action_flags.consume(...), unlike haul/staticmine which guard MOVE/TRANSFER/HARVEST. Safe today ONLY because every combat state returns None and never multi-transitions while acting — protection "by luck of return value," not by the guard the rest of the codebase relies on.
Impact:              No active bug, but the central double-fire guard is bypassed in the most-churned subsystem; the moment a combat state is refactored to transition mid-action, intents can double-fire or contend with other writers, with no warning. Makes squad-combat the one job you cannot reason about with the action_flags model.
Recommendation:      Route combat intents through action_flags.consume(ATTACK/RANGED_ATTACK/HEAL/MOVE) like every other job; add a debug-assert that no intent fires twice per creep per tick.
Breaking-change?:    None
Rewrite-implication: refactor → ../design/0003-behavior-modeling.md
```

```
ID:                  IBEX-030
[MEDIUM]             Transfer matching is O(haulers·priorities·rooms·targets·resources) with no bucket gate
Subsystem:           Transfer & market logistics
Location:            transfer/transfersystem.rs:2113→1763→1641/1552 (select_pickup_and_delivery tree); jobs/utility/haulbehavior.rs:206 (per idle hauler); missions/haul.rs:289 (pickup_rooms small)
Type:                CPU
Status:              Hypothesis (verdict: adjusted High→Medium, confidence M)
Evidence:            select_pickup_and_delivery loops generate_active_priorities (~6 mask pairs) → select_best_delivery → re-runs select_pickups per returned delivery; invoked per idle hauler with no can_execute_cpu gate. BUT per-hauler room scope is SMALL (pickup_rooms = &[self.room_data] at haul.rs:289; delivery_rooms typically 1 home), and per-room node data is built once per tick and cached (swap_remove generator). So worst-case is bounded by small per-hauler room sets, not the "20 rooms × 100 targets" cross-product.
Impact:              Real ungated, per-creep greedy re-matching with no cross-tick amortization; a genuine CPU/scalability concern that adds to the Field Report C floor, but bounded by per-hauler scope and once-per-tick caching.
Recommendation:      Add a bucket-aware gate; re-decide assignment only every N ticks or on completion; cache per-room available-resource totals once per tick. Two-phase: snapshot supply/demand once, then assign creeps against the cache.
Breaking-change?:    None
Rewrite-implication: refactor → ../design/0004-cpu-governance-and-load-shedding.md
```

```
ID:                  IBEX-031
[MEDIUM]             Lazy transfer generators run on first room query (ordering-dependent, but reservation-safe)
Subsystem:           Transfer & market logistics
Location:            transfer/transfersystem.rs:1339–1356 (flush/swap_remove), :484/:496 (pending reservation), :602/:639/:723/:787 (node type filtering); room_transfer.rs:614–624 (registration)
Type:                Architecture
Status:              Observed-fact (verdict: adjusted High→Low; nondeterminism refuted by reservation accounting)
Evidence:            Generators are consumed lazily (swap_remove on first matching query). The double-serve/nondeterminism concern is REFUTED: flush_generators drains EVERY generator whose types intersect the query before any read, node-level type filtering means a HAUL-only consumer never sees extra-typed requests, and register_pickup/register_delivery bump pending_* which get_available_* subtract — so a second hauler sees reduced availability regardless of order. Greedy sequential-order-sensitivity exists in any sequential matcher and is not caused by lazy flushing.
Impact:              No correctness corruption; residual issue is testability/clarity (the TODO at :1330 acknowledges this) and the viz-on force-flush diverging from the matcher view.
Recommendation:      No urgent change. Document the lazy-flush contract; optionally move to an explicit two-phase collect (flush_all_generators exists) before matching for testability.
Breaking-change?:    None
Rewrite-implication: refactor (two-phase collect-then-match) — low priority
```

```
ID:                  IBEX-018
[MEDIUM]             Market trust guard is statistically weak; no time-series/spike detection
Subsystem:           Transfer & market logistics
Location:            transfer/ordersystem.rs:349–351 (can_trust_history), :370/:379/:415–420 (pricing), :367/:446 (TODO admits unimplemented sanity check)
Type:                Strategy
Status:              Observed-fact (verdict: confirmed)
Evidence:            can_trust_history = transactions()>100 && volume()>1000 && stddev_price() <= avg_price()*0.5, inspecting only .last(); pricing is avg ± a stddev fraction. The code's own TODOs flag "validate that the current average price is sane (compare to prior day?)" as unimplemented.
Impact:              A rival can spike a resource with a fake high-volume day (clearing the gate) and then trade into Ibex at the inflated avg+stddev price — out-tradeable. Cross-map sells DO subtract energy cost on the active-sell path; the intra-empire terminal-delivery path ranks by ratio without a hard prefer-local/value floor.
Recommendation:      Compare latest day to a trailing-window median (reject if avg deviates >X%); add volume-weighted bounds and per-order exposure caps; add a hard prefer-local gate + min value/cost threshold on cross-map deliveries.
Breaking-change?:    None
Rewrite-implication: refactor (trailing-window trust model + exposure caps)
```

```
ID:                  IBEX-032
[MEDIUM]             Claim/colony/scout score and gate home rooms by LINEAR distance, not route distance
Subsystem:           Operations / Expansion
Location:            operations/claim.rs:563–564 & :740–742 (Manhattan delta, range<=5 filter), :167–178 (distance_score), :737 (TODO "use path distance"); colony.rs:139–143; scout.rs:190–193
Type:                Strategy
Status:              Observed-fact (verdict: confirmed)
Evidence:            claim.rs:563 `let delta = room_data.name - *home_name; let range = delta.0.unsigned_abs() + delta.1.unsigned_abs()` then filter range<=5; same Manhattan pattern at colony.rs:139 and scout.rs:191. The war reassign path uses route distance (RoomRouteCache) but expansion eligibility uses raw linear delta. A TODO at claim.rs:737 explicitly notes the gap.
Impact:              Can select/assign home rooms or claim candidates that are linearly close but route-unreachable (behind hostiles, across closed/SK rooms), wasting claimers/escorts that can never arrive.
Recommendation:      Reuse RoomRouteCache.get_route_distance (already used by war) for home-room eligibility and candidate gating, with a reachability check; budget it (the cache is ephemeral and find_route uncapped). Validation: place a hostile wall between home and candidate and assert it is rejected.
Breaking-change?:    Behavioral
Rewrite-implication: refactor
```

```
ID:                  IBEX-033
[MEDIUM]             classify_threat over-triggers PlayerSiege on weak boosts or trivial creep count
Subsystem:           Military core
Location:            military/threatmap.rs:207 (Siege trigger), :109–117 (hardcoded 4.0× T3 boost multiplier)
Type:                Strategy
Status:              Observed-fact (verdict: confirmed)
Evidence:            `if any_boosted || (total_dps>200.0 && total_heal>100.0) || hostile_creeps.len()>=4` returns PlayerSiege. A single creep with one cheap boosted part is Siege; four unboosted 30-DPS creeps (no heal) are Siege by count; two unboosted 150-DPS creeps (300 DPS) fall through to Raid. The boost multiplier always assumes T3 (4.0×), inflating T1/T2 DPS 3×.
Impact:              Over-invests against harmless boosted scouts/small groups and under-rates genuine high-DPS unboosted raids, mis-sizing response and wasting energy/CPU.
Recommendation:      Replace the boolean any_boosted/count triggers with a boost-tier-aware effective-DPS + EHP score; gate Siege on sustained net DPS vs our defenses (damage.rs::net_tower_damage exists). Detect actual boost tier instead of assuming T3.
Breaking-change?:    Behavioral
Rewrite-implication: refactor
```

```
ID:                  IBEX-034
[MEDIUM]             EconomySnapshot military reserve uses stored_energy/5 with a flat 5k floor (not RCL-scaled)
Subsystem:           Economy & infra missions
Location:            military/economy.rs:82–114 (can_afford_military :83, can_rooms_afford_military :91–101)
Type:                Strategy
Status:              Observed-fact (verdict: confirmed)
Evidence:            `let reserve = (r.stored_energy / 5).clamp(5_000, 30_000);` — the 5k floor dominates whenever stored_energy < 25k. No scaling by RCL, controller level, or income; the same floor applies to a fresh RCL4 room and a mature RCL8 room.
Impact:              Over-reserves low-RCL rooms (caps early-game military) and under-protects mature high-throughput rooms relative to income; a coarse proxy for true surplus.
Recommendation:      Scale the reserve by RCL/income (a multiple of income-per-tick or a function of controller level), or derive from desired-storage thresholds in constants.rs. Validation: log reserve vs stored_energy across RCL2→8.
Breaking-change?:    Behavioral
Rewrite-implication: refactor
```

```
ID:                  IBEX-035
[MEDIUM]             compute_nearest_spawn_distances runs uncapped-by-default pathfinder::search per spawn×target
Subsystem:           Economy & infra missions
Location:            missions/localsupply/structure_data.rs:174–206 (call site :152–153; refresh ~every 10 ticks per visible room)
Type:                CPU
Status:              Hypothesis (verdict: adjusted High→Medium, confidence H)
Evidence:            `pathfinder::search(spawn.pos(), target_pos, 1, Some(opts))` in a nested loop over spawns × (sources∪minerals), with SearchOptions setting only plain_cost/swamp_cost. The engine applies its default 2000-op cap per call (screeps-game-api max_ops=None → engine default), so each search is bounded ~2000 ops, NOT runaway. The cache is per-room (Rc<RefCell>) so the refresh runs once per ~10 ticks per room, not per mission. No can_execute_cpu gate.
Impact:              Bounded, throttled spike: ≤3 spawns × (~2–3 sources+mineral) ≈ 6–9 searches × ≤2000 ops once per 10 ticks per room — amortizes to well under ~1 CPU/tick/room. A real inefficiency feeding the floor, not the unbounded multiplicative driver originally framed.
Recommendation:      Pass a tighter .max_ops; gate behind can_execute_cpu(LowPriority); cache spawn-distance with a long/structural TTL (spawn/source positions are static — invalidate on build/destroy, not every 10 ticks).
Breaking-change?:    None
Rewrite-implication: refactor
```

```
ID:                  IBEX-036
[MEDIUM]             Unbudgeted gather BFS calls uncached describe_exits per room (periodic spike)
Subsystem:           Room data, visibility & planning
Location:            room/gather.rs:118–229 (BFS, describe_exits at :143/:181); operations/claim.rs:244 (max_distance=4)
Type:                CPU
Status:              Observed-fact (verdict: adjusted High→Medium, confidence H)
Evidence:            The while loop (164–204) expands every reachable room up to max_distance with no game::cpu reads, no early-out, calling game::map::describe_exits live per room and NOT using the already-cached RoomStaticVisibilityData.exits. BUT run_discover is gated behind discover_interval (~500 ticks) and the frontier only expands through ALREADY-SCOUTED non-hostile rooms (unscouted candidates terminate without expanding), so it is a periodic burst over dozens of rooms, not a per-tick hundreds-of-rooms blowout.
Impact:              Dozens of redundant JS describe_exits calls + HashMap work in one unbudgeted burst every ~500 ticks — a genuine avoidable spike worth fixing, not the per-tick death-spiral driver originally framed.
Recommendation:      Add a CpuBudget closure that aborts the BFS at a fraction of remaining tick CPU (persist partial frontier); source exits from the cached RoomData.exits; cap visited-room count.
Breaking-change?:    None
Rewrite-implication: refactor → ../design/0004-cpu-governance-and-load-shedding.md
```

```
ID:                  IBEX-037
[MEDIUM]             seg-60 planner can restart-thrash on a persistent fingerprint mismatch (no attempt counter)
Subsystem:           Room data, visibility & planning
Location:            room/roomplansystem.rs:197–204 (resume→default restart), :443–445 (encode-or-drop)
Type:                CPU
Status:              Hypothesis (verdict: confirmed mechanism, confidence M)
Evidence:            builder.resume(old_state) Err → PlannerBuilder::default().build() restarts from scratch with an info log, every tick the room is planning. If the layer-config fingerprint is permanently changed, resume fails every tick, discarding accumulated PlanningState and never converging. No restart counter or backoff. On encode failure (:443) the segment is left un-updated (silent skip).
Impact:              A room can burn its full planning budget every tick forever without producing a plan (no base layout → no construction → stunted growth), while emitting an info log each tick.
Recommendation:      Add a bounded restart counter on RoomPlannerRunningData; after N failed resumes mark Failed{time} and back off (reuse the 2000-tick replan gate at :348). Log encode failures at warn.
Breaking-change?:    Memory/format (adds a serde(default) counter field)
Rewrite-implication: refactor
```

```
ID:                  IBEX-038
[MEDIUM]             Cost-matrix ephemeral data cleared and rebuilt every tick (steady CPU floor)
Subsystem:           Pathing & movement
Location:            screeps-rover/src/costmatrixsystem.rs:67–74 (clear_ephemeral, explicit TODO); pathing/costmatrixsystem.rs:43–45 (CostMatrixClearSystem)
Type:                CPU
Status:              Observed-fact (verdict: confirmed; IBEX-017)
Evidence:            clear_ephemeral() nulls structures/construction_sites/creeps for every cached room each tick (TODO: "Need to add cache eviction policy"). refresh_room re-fetches via room.find(...) on the next miss — each an O(objects) engine call. Structures persist in the seg-55 cache but the in-tick clear drops the in-memory entry, forcing a rebuild.
Impact:              For N creeps across M rooms, every room touched by pathfinding triggers a full multi-find rebuild every tick. Construction sites and structures change rarely; clearing them every tick is wasted CPU that compounds the spiral.
Recommendation:      Cache structures + construction sites across ticks with an invalidation signal (build/destroy event or N-tick TTL); only rebuild the creep layer per tick (and even that can be a few-tick snapshot).
Breaking-change?:    Memory/format
Rewrite-implication: refactor (cache-eviction policy keyed by change events)
```

```
ID:                  IBEX-039
[MEDIUM]             find_route is CPU-headroom-gated but not ops-budgeted; runs before the per-tick ops cap
Subsystem:           Pathing & movement
Location:            screeps-rover/src/movementsystem.rs:1183–1199 (find_route in generate_path), :1169–1177 (headroom gate), :1239–1247 (ops deducted only for the subsequent tile search)
Type:                CPU
Status:              Observed-fact (verdict: confirmed; part of IBEX-016)
Evidence:            generate_path calls find_route BEFORE the pathfinding_ops_budget deduction; find_route's cost is bounded only by the headroom guard and post-hoc truncation to MAX_PATHFIND_ROOMS=16. The ops budget never sees find_route. In burst mode (bucket>=9500) it permits an 80-CPU-headroom search per eligible creep.
Impact:              On roadless/contested maps find_route is the unbounded leg; many creeps repathing in burst mode is the find_route contribution to the spiral.
Recommendation:      Add an explicit per-tick find_route call budget (count or CPU) shared with the ops budget so route searches are shed under pressure like tile searches.
Breaking-change?:    None
Rewrite-implication: refactor → ../design/0004-cpu-governance-and-load-shedding.md
```

```
ID:                  IBEX-040
[MEDIUM]             Shove/local-avoidance walkability check is terrain-only (ignores blocking structures)
Subsystem:           Pathing & movement
Location:            screeps-rover/src/screeps_impl.rs:142–155 (is_tile_walkable); used by resolver via movementsystem.rs:608
Type:                Correctness
Status:              Observed-fact
Evidence:            is_tile_walkable checks only room edges and Terrain::Wall via get_room_terrain; it does NOT consult structures or the cost matrix. resolve_conflicts/try_shove use this predicate, so a creep can be shoved onto a tile that is terrain-plains but occupied by a blocking structure (wall structure, hostile rampart, spawn).
Impact:              A shoved creep can be pushed onto an impassable structure tile; the subsequent move errors and the creep wastes the tick / oscillates — degrading resolver quality and manufacturing stuck states in a structure-dense base.
Recommendation:      Make is_tile_walkable (or a resolver-specific predicate) consult the cached structure cost layer so shove/avoidance respects blocking structures, not just terrain.
Breaking-change?:    None
Rewrite-implication: refactor
```

```
ID:                  IBEX-041
[MEDIUM]             Combat job re-clones the full hostile/friendly creep list 3–4× per creep per tick
Subsystem:           Jobs layer
Location:            jobs/squad_combat.rs:922–969 (get_hostile_creeps → .hostile().to_vec()) + call sites :375,:443,:494,:587,:774
Type:                CPU
Status:              Observed-fact
Evidence:            get_hostile_creeps returns creeps.hostile().to_vec() (full clone of Vec<Creep>) and is called repeatedly within a single Engaged/CombatResponse tick. No per-tick cache of the room's hostile set is passed down.
Impact:              For a quad in a busy enemy room this multiplies allocations and range scans per creep per tick; across multiple war squads it adds avoidable CPU during exactly the moments (active war) when bucket headroom is lowest.
Recommendation:      Compute the room hostile/friendly/structure sets once per tick (cache on JobTickContext or read cached RoomData creeps via slices without to_vec) and pass references into the attack/heal/move helpers.
Breaking-change?:    None
Rewrite-implication: refactor
```

```
ID:                  IBEX-042
[MEDIUM]             A single Err from any mission state tick tears down the entire mission
Subsystem:           Combat & expansion missions
Location:            missions/missionsystem.rs:254–266 (RunMissionSystem) + :199–211 (PreRunMissionSystem); machine_tick.rs:28 (`?`); miningoutpost.rs:119/129; defend.rs:215
Type:                Correctness
Status:              Observed-fact
Evidence:            run_mission/pre_run_mission returning Err → queue_mission_abort (full teardown + child cascade). machine_tick propagates a state-tick Err via `?`. miningoutpost/defend ticks return Err on transient conditions (can_run_mission false, home-rooms empty).
Impact:              A momentarily-unavailable room/visibility/home-room (common during contested expansion) destroys a long-running mining/defend campaign and all its children rather than waiting — premature teardown / lifecycle churn.
Recommendation:      Distinguish "abort" from "skip this tick": add a MissionResult::Idle/Wait or a typed error so transient conditions return Running. Reserve Err for genuinely unrecoverable states.
Breaking-change?:    Behavioral
Rewrite-implication: refactor → ../design/0003-behavior-modeling.md
```

```
ID:                  IBEX-008
[MEDIUM]             World renderer breaches the per-target visual limit / poisons the shared buffer (Field Report H)
Subsystem:           Visualization & support/API-fork
Location:            visualize.rs:39–41,129–141 (unbounded per-primitive flush, no size check); screeps-game-api room_visual.rs:377–392 (serde to_value().expect + add_visual); visualization.rs:1294–1410 (global overlay duplicated per room); constants/extra.rs:137,266 (500KB/1000KB limits)
Type:                Tick-safety / Robustness
Status:              Hypothesis (verdicts: viz-1 adjusted to Low — to_value path not reachably fallible; viz-2 adjusted to Low — off-by-default + generous limit; confidence M)
Evidence:            Engine console.addVisual THROWS when a per-target buffer exceeds 500KB (room/all-rooms) or 1000KB (map) — the prompt's "~16 KiB" is REFUTED (real limits in extra.rs:137/266). Ibex emits one add_visual per primitive with no get_visual_size guard (never called anywhere). Room visuals carry raw f32 x/y; NaN/Inf → JSON null → desync. The global overlay is drawn into target None AND duplicated into every room (visualization.rs:1401–1409). The serde_wasm_bindgen to_value().expect cannot actually fail for any constructible Visual (f32/String/u32 serializers are infallible), so the panic vector is latent, not reachable.
Impact:              Under heavy debug-flag-on load a per-room buffer can approach 500KB and a throw would abort the tick (compounding IBEX-025). Off-by-default emitters + the generous limit make this debug-only, not a gameplay corruption — but enabling the renderer can blank a target's visuals (and, via IBEX-025, skip serialize_world).
Recommendation:      Pre-check console::get_visual_size(target) and stop appending near the cap (drop-with-telemetry); make add_visual fallible at the API boundary; clamp room-visual coords to finite before emit; draw the global overlay once to None, not per room; add tick-level abort containment (IBEX-025).
Breaking-change?:    None
Rewrite-implication: refactor (bounded, fallible visual flush + coordinate validation)
```

```
ID:                  IBEX-024
[MEDIUM]             Oversized files mix many concerns; decomposition candidates
Subsystem:           cross-cutting
Location:            transfer/transfersystem.rs (2439), missions/attack_mission.rs (~2040), visualization.rs (1485), operations/war.rs (1444), room/data.rs (899); military/squad.rs (now ~900, shrunk from prompt's 1021)
Type:                Maintainability
Status:              Observed-fact (verdict: confirmed; squad.rs line count corrected)
Evidence:            transfersystem.rs holds target dispatch + queue model + matching + stats + 2 ECS systems; visualization.rs mixes summary types, 6 systems, layout, and 4 render impls; RoomData holds persistent state + 5 RefCell caches + a 22-field structure bucket. The pure matching/geometry/layout kernels are entangled with game-API side effects.
Impact:              High cognitive load; the testable cores (matching, geometry, layout, scoring) are buried; the target/mode mispairing that triggers IBEX-010 is easy to introduce.
Recommendation:      Decompose during the rewrite (e.g. transfer → target/queue/matching/stats/systems; visualization → summary/layout/render/map; RoomData → persistent component + ephemeral RoomView resource); extract pure kernels as test seams.
Breaking-change?:    None
Rewrite-implication: refactor → ../design/0001-entity-model.md (RoomData), 0002 (transfer)
```

### Notable Low

```
ID:                  IBEX-019
[LOW]                attack.rs:615 guarded double-unwrap — REFUTED as active panic, latent only
Subsystem:           Combat & expansion missions
Location:            operations/attack.rs:615 (room_data.get(room_entity.unwrap()).unwrap()), guarded by have_live_intel at :608–612
Type:                Tick-safety
Status:              Observed-fact (verdict: confirmed latent)
Evidence:            Line 615 runs only inside `if have_live_intel`, which requires room_entity.is_some() AND room_data.get(e).is_some(); both unwraps are provably safe THIS tick (same system_data, no intervening mutation). Not an active panic.
Impact:              None active. The guard is implicit and load-bearing; a future edit reading room_entity before recomputing the guard could regress it.
Recommendation:      Replace with `if let Some(room_data) = room_entity.and_then(|e| system_data.room_data.get(e)) { … }` so the safety is explicit. Pure cleanup.
Breaking-change?:    None
Rewrite-implication: keep-as-is (harden)
```

```
ID:                  IBEX-020
[LOW]                attack_mission get_room() last-resort .expect — reachable only in a fully-degraded mission
Subsystem:           Combat & expansion missions
Location:            missions/attack_mission.rs:1917 (.expect in get_room fallback)
Type:                Tick-safety
Status:              Observed-fact (verdict: confirmed latent)
Evidence:            `squad_entities.first().copied().expect("AttackMission must have at least one entity reference")` is reached only when home_room_datas is empty AND owner is None AND squad_entities is empty (two error!() logs precede it at :1901/:1907). get_room() is called by cleanup and by repair_entity_integrity (game_loop.rs:211) — a panic there runs in the serialize-critical post-pass.
Impact:              No active panic, but a panic in this fully-degraded state would run during the end-of-tick serialize path, aborting the tick (panic=abort, no isolation — IBEX-025).
Recommendation:      Return a sentinel/Option instead of panicking; callers already treat a non-room entity as a no-op. Cheap quick-win.
Breaking-change?:    None
Rewrite-implication: refactor
```

```
ID:                  IBEX-012
[INFO/LOW]           SquadContext.members/heal_priority ARE repaired pre-serialize — REFUTED as written
Subsystem:           Military core / Combat missions
Location:            game_loop.rs:264–302 (SquadContext block)
Type:                Correctness (false-positive guard)
Status:              Observed-fact (verdict: REFUTED — downgrade seed)
Evidence:            Block 5 iterates (&entities, &mut squads), runs `sc.members.retain(|m| is_valid(m.entity))` with per-removal error log + count, and clears heal_priority when its entity is invalid (292–299). PreRunSquadUpdateSystem also prunes dead members each tick (squad.rs:959–971). This directly contradicts prompt §6.5/§7's claim that SquadMember refs are not repaired.
Impact:              The serialization-side dangling-member hazard for SquadContext is ALREADY mitigated. The residual open hazard is the raw-u32 squad link in JobData (IBEX-002b), which this block cannot cover.
Recommendation:      Mark IBEX-012 closed/refuted; redirect the concern to IBEX-002b. Add a round-trip test that kills a squad member and asserts no dangling member survives repair.
Breaking-change?:    None
Rewrite-implication: keep-as-is
```

```
ID:                  IBEX-011
[LOW]                Partial-haul does NOT strand resources; it re-plans via Idle (commitment churn, not data loss)
Subsystem:           Jobs / economy
Location:            jobs/utility/haulbehavior.rs:513–567 (tick_delivery, consume :541, drop-on-invalid :554, delay-return :558–563); haul.rs:217–224
Type:                Correctness
Status:              Observed-fact (verdict: adjusted — reclassified, confidence M)
Evidence:            tickets.remove(0) (:554) fires only for invalid/exhausted targets; carried resources stay in the creep. On a failed transfer (transfered==false) tick_delivery returns Some(idle), and Idle/FinishedDelivery re-run delivery selection against the creep's CURRENT store — re-planning, not stranding. consume_deposit precedes the transfer intent, so a transfer the engine rejects still mutates the ticket, but resources are recovered via re-plan.
Impact:              The seed's "resources stranded/lost" reading is REFUTED. The real defect is churn: a hauler can abandon an in-progress delivery plan and re-enter Idle re-selection (wasted ticks, target switching) plus transient transfer-queue mis-accounting.
Recommendation:      Keep IBEX-011 OPEN as a behavioral commitment bug, not data loss. Add a "committed delivery" guard so a hauler that started depositing finishes its ticket set before Idle can re-plan; prefer confirm-then-consume.
Breaking-change?:    None
Rewrite-implication: refactor
```

```
ID:                  IBEX-043
[LOW]                Power-bank concurrency uses a no-op filter and counts ALL attacks
Subsystem:           Operations (campaigns)
Location:            operations/war.rs:766–776
Type:                Maintainability / Strategy
Status:              Observed-fact
Evidence:            power_bank_count = active_attack_rooms.iter().zip(...).filter(|_| true).count() — the .filter(|_| true) is a no-op, so it counts ALL active attacks, not power-bank ones, despite the inline comment "we'll filter below" (no filter follows). Compared against max_concurrent_power_banks.
Impact:              Power-bank launch gating uses total-active-attack count as if it were power-bank count; power-bank farming is throttled by unrelated attacks and the separate cap is effectively meaningless.
Recommendation:      Count only AttackOperations whose reason is PowerBank (or a dedicated counter); remove the no-op filter.
Breaking-change?:    None
Rewrite-implication: refactor
```

```
ID:                  IBEX-044
[LOW]                Tier-gating / timeouts use unchecked u32 subtraction on persisted tick values
Subsystem:           Operations / Jobs
Location:            operations/war.rs:1314–1316 (should_run_tier), attack.rs:579/:637, colony.rs:193; jobs/squad_combat.rs:165–168 (combat_response timeout)
Type:                Tick-safety
Status:              Hypothesis (confidence L)
Evidence:            should_run_tier and several timeouts compute `game::time() - t` with raw subtraction on serialized last_*_tick values. If game::time() ever decreases relative to a persisted tick (private-server time reset / restored old snapshot) the subtraction underflows. Scout already uses game::time().saturating_sub(idle_since) (scout.rs:175), showing the safe form is known.
Impact:              In release the wrap is self-correcting (result >= cadence, benign); under panic=abort a debug overflow would abort the tick. Latent inconsistency in serialized-tick fields.
Recommendation:      Use game::time().saturating_sub(t) everywhere. Trivial quick-win, no behavior change in normal play.
Breaking-change?:    None
Rewrite-implication: keep-as-is
```

```
ID:                  IBEX-045
[LOW]                store.rs free-capacity helper uses unchecked u32 subtraction
Subsystem:           Spawning, stats & helpers (used in jobs)
Location:            store.rs:11–16 (capacity - used_capacity)
Type:                Tick-safety
Status:              Hypothesis (confidence L)
Evidence:            `capacity - used_capacity` summed over per-resource get_used_capacity. The helper exists BECAUSE of a get_used_capacity double-count workaround, so summed used could in principle exceed get_capacity(None) and panic under panic=abort. Used in harvest/transfer hot paths.
Impact:              If it fires it aborts the whole tick (IBEX-025); reachable only if the store API mis-reports; currently unobserved.
Recommendation:      Use saturating_sub; add a debug assert logging when used>capacity to detect the API anomaly.
Breaking-change?:    None
Rewrite-implication: keep-as-is (saturating_sub)
```

```
ID:                  IBEX-022
[LOW]                Spawn body-sizing min-cost concern — REFUTED for current callers (clamp already present)
Subsystem:           Spawning, stats & helpers
Location:            creep.rs:123–156 (create_body); missions/haul.rs:207, localbuild.rs:252, source_mining.rs:394 (`room.energy_available().max(SPAWN_ENERGY_CAPACITY)`)
Type:                Strategy
Status:              Observed-fact (verdict: REFUTED, confidence H)
Evidence:            The empty-fleet path clamps maximum_energy to .max(SPAWN_ENERGY_CAPACITY)=300 (haul.rs:207, localbuild.rs:252, source_mining.rs:394). create_body with maximum_energy=300 passes the min-repeat guard for every cited body (max needed 250), so the request IS built and queued. The clamp IS the "build at min body" remedy the todo.md note recommends; it landed in commit 73fa0f3. The todo.md:18 line describes the PRE-FIX state — a stale, unchecked backlog item.
Impact:              No economy-bootstrap stall for the cited callers in normal play. (Ordering remains correct — see the INFO exemplar.)
Recommendation:      Check the todo.md item off. Optionally add a create_body unit test at maximum_energy just below the min-repeat threshold to lock in the clamp.
Breaking-change?:    None
Rewrite-implication: keep-as-is
```

```
ID:                  IBEX-046
[LOW]                NaN partial_cmp coalescing in priority/value comparators — unreachable today, undefended at source
Subsystem:           Room visibility / Transfer / Spawning
Location:            room/visibilitysystem.rs:248–250 & roomplansystem.rs:362 (priority); transfersystem.rs:1842/:2061/:2108 & ordersystem.rs:280 (value); spawnsystem.rs:88–90 (priority); repairqueue.rs:64–75/97–108 (max_hits guard)
Type:                Correctness
Status:              Observed-fact (verdict: NaN-deadlock REFUTED; RepairQueue NaN fully guarded)
Evidence:            partial_cmp(...).unwrap_or(Equal) coalesces NaN to Equal. All visibility/spawn priorities are finite named constants (no NaN source), so a NaN cannot enter today; even if it did, max_by with Equal-fallback would skip it (no deadlock). RepairQueue is FULLY guarded by `if max_hits>0 { frac } else { 1.0 }` (repairqueue.rs:64–75) — the §6.11/§7 NaN concern is already mitigated. Transfer value rankings can produce inf/NaN only on degenerate length=0/cost=0 cases (arbitrary pick, no panic).
Impact:              No active bug; latent if a future computed priority introduces NaN. The "NaN deadlocks scouting" framing is REFUTED.
Recommendation:      Leave unwrap_or(Equal) as a backstop; add debug_assert!(priority.is_finite()) at request sites and guard transfer divisors (length.max(1), cost.max(1.0)) to catch a future NaN at the source.
Breaking-change?:    None
Rewrite-implication: keep-as-is
```

```
ID:                  IBEX-005b
[LOW]                Wall/rampart peacetime maintenance concern — REFUTED (room-scan repair path covers it)
Subsystem:           Economy & infra missions / Defense
Location:            missions/wall_repair.rs (war-only constructor at war.rs:385); repairbehavior.rs:28/:81–99; repair.rs:76–88/:116–117/:186; localbuild.rs:113
Type:                Strategy (false-positive guard)
Status:              Observed-fact (verdict: REFUTED, confidence H)
Evidence:            WallRepairMission is indeed only built by the war op when has_hostiles, and LocalBuild's repair QUEUE excludes walls. BUT walls/ramparts are maintained in peacetime by two INDEPENDENT room-scan paths with allow_walls=true: BuildJob/HarvestJob get_new_repair_state → select_repair_structure(allow_walls=true) → room scan get_prioritized_repair_targets(allow_walls=true) (repair.rs:186), and LocalBuild's repairer-spawn trigger get_repairer_priority(..., true) (localbuild.rs:113). map_defense_priority's not-under-attack branch (repair.rs:76–88) is explicitly designed for peacetime rampart upkeep.
Impact:              Walls do NOT decay unrepaired in an idle owned room; WallRepairMission is a siege-time accelerator, not the sole mechanism. The only residual issue is the up-to-20-tick siege scan latency (minor tuning).
Recommendation:      No structural change needed; optionally reduce the siege-scan interval or make it hostiles-adaptive. Do not treat as a High strategic gap.
Breaking-change?:    None
Rewrite-implication: keep-as-is
```

## 2. Per-Subsystem Health (all 12)

| # | Subsystem | Assessment (2–4 sentences) | Biggest single risk |
|---|---|---|---|
| 1 | Tick orchestration & ECS core | The dispatch core is solid: one `for_each_system!` macro drives setup+run so order cannot drift, `world.maintain()` runs after every system, and reset/discontinuity handling is coherent. The fragility lives at the ECS↔persistence seam: `repair_entity_integrity` (168–369) is a 7-block multi-borrow-scope safety net that exists only because Entity refs dangle, and it does NOT cover raw-u32 refs in JobData. There is zero per-system panic isolation under `panic="abort"`. | No per-system panic containment (IBEX-025): a single reachable unwrap aborts the tick AND skips `serialize_world`, losing the tick's work while partial heap mutations persist (env.tick already advanced). |
| 2 | Memory & serialization | Functional in steady state but built on brittle, unversioned foundations that fail across resets. seg-55 is wiped to empty every normal tick (IBEX-013), so the cost matrix never survives a reset. Positional bincode + a no-op `serde(default)` convention + no version header means any serialized-field change silently breaks old snapshots (IBEX-004). Repair covers SquadContext but not the JobData u32 ref. | Segment-55 collision: `serialize_world`'s trailing-clear destroys the cost-matrix cache every tick, forcing a full per-room rebuild on the most CPU-fragile post-reset tick. |
| 3 | Operations (campaigns) | The operation layer is structurally simple; the manager respawns missing singletons and entity-ref repair is wired for ops that hold child refs. `war.rs` is functionally complete but all cadences are hardcoded to 1, making per-tick threat scanning, full RoomThreatData cloning, and O(A·H) cold-cache route rebalance a direct CPU drain. The attack lifecycle has a dead economy-abort branch and no force-abort on capacity shrink. | War per-tick CPU (IBEX-021): cadence=1 + cold RoomRouteCache + RoomThreatData clones run a full owned/remote defense scan and route rebalance every tick, amplifying the death-spiral on reset. |
| 4 | Economy & infra missions | The most mature, disciplined area: consistent log-and-continue error handling, essentially no reachable hot-path panics, and a coherent Colony→LocalSupply→{mining,transfer} tree with per-mission spawn sizing. Weaknesses are CPU shape (compute_nearest_spawn_distances, throttled), a coarse RCL-flat military reserve, and stale-cache amplification of the staticmine smell. Reset-resilience is good (snapshots ephemeral, rebuilt each tick). | Stale SupplyStructureCache (IBEX-003-adjacent) lets the spawn path request miners for destroyed containers/links (10-tick TTL, no resolve() re-check on the spawn path), upstream of the staticmine smell. |
| 5 | Combat & expansion missions | Two incompatible squad models coexist: a mature SquadContext/virtual-anchor design (attack_mission) and an older flat-Vec model (squad_assault/defense/harass) with no formation and a broken rally check. The persistent creep→squad binding is a bare u32 index that aliases on recycle. Lifecycle is mostly bounded in attack_mission but legacy/defend missions lean on parents for teardown, and one Err tears down the whole mission. Expansion missions (claim/scout/raid/dismantle) are comparatively settled. | Creep→squad link is a non-remapped raw entity index (IBEX-002b): after reset/recycle creeps bind to the wrong squad or none, silently breaking formation/heal/retreat — the strongest single root for Field Report A. |
| 6 | Jobs layer | The most mature, well-commented area: economy jobs are hardened with resolve-and-fallback and `if let` guards, so reachable hot-path panics are now rare (staticmine unwrap is guarded; partial-haul re-plans rather than stranding). The combat job is the outlier — unguarded intents, repeated hostile-list clones, raw-u32 squad reconstruction, aggregate-state engage gate. The single biggest gap: job-layer stuck recovery is entirely absent (check_movement_failure is dead code). | No job-layer stuck recovery (IBEX-015): a creep the rover abandons loops the same move forever; one wedged blocker can cascade-freeze a corridor with no recovery. |
| 7 | Military core | A thoughtfully-structured, mostly-pure toolkit (data-driven compositions, virtual-anchor cohesion with anti-deadlock timers, greedy heal assignment, threat classification, tower math) with no reachable hot-path panics. Dominant risks are strategic: advisory (not enforced) cohesion, an entirely inert BoostQueue, and over-triggering PlayerSiege. Serialization safety is solid — SquadContext.members ARE repaired (IBEX-012 refuted). | Cohesion cannot guarantee all members are in range before engaging (IBEX-001): the virtual anchor advances on a 3-tick 75% quorum and degrades permanently to Loose, so squads enter combat dispersed. |
| 8 | Room data, visibility & planning | Among the more carefully-structured areas: clean persistent/ephemeral visibility split, a correct per-tick RoomStatusCache, and one of the few real load-shedding mechanisms (room-plan CPU budget). Weaknesses are CPU shape (periodic unbudgeted gather BFS with uncached describe_exits), a monolithic 899-line RoomData, and seg-60 planner restart-thrash potential. Double-unwrap and NaN concerns are latent only. | Unbudgeted gather BFS (IBEX-036): a periodic Discover burst calls dozens of uncached describe_exits with no CPU budget; the planner can restart-thrash on a persistent fingerprint mismatch (IBEX-037). |
| 9 | Pathing & movement | The most CPU-disciplined subsystem: layered ops/CPU caps, stuck/expiry budgets, a find_route headroom guard, and a min-ops floor. But the recovery loop is half-wired (check_movement_failure unused, repath_count never read as a cap), the rover's Follow/quad-formation machinery is unused by the military layer, and the cost matrix discards ephemeral data every tick. | No repath cap + no cross-tick governor (IBEX-016): the per-tick caps bound one tick but not the cumulative storm; doomed/contested targets repath every tick indefinitely, draining the bucket. |
| 10 | Transfer & market logistics | A single 2439-line file: a lazy generator-backed supply/demand queue + a greedy per-creep matcher, internally consistent and ephemeral (reset-safe). Dominant problems are a reachable Nuker-withdraw panic, per-creep greedy matching cost, weak market trust guards, and a 14-arm panic table. The generator-ordering hazard is refuted (reservation accounting prevents double-serve). | Reachable Nuker-withdraw panic (IBEX-010): a raid registers an enemy nuker as a withdraw target, hitting `panic!`; under panic=abort the whole tick aborts and serialize_world never runs. |
| 11 | Spawning, stats & helpers | One of the more settled, defensively-written areas: the spawn comparator is correct (descending), RepairQueue NaN is fully guarded, and stats/feature structs use serde defaults for forward-compat. Open items are minor: break-on-unaffordable starvation, unbudgeted per-candidate find_nearest searches, unversioned stats segments, and a transient stats_history rollback miscascade. The body-sizing min-cost bug is refuted (clamp present). | Unbudgeted per-candidate pathfinder::search in find_nearest_* (IBEX-016-adjacent): N full searches per call with no CPU/ops budget — a cross-cutting death-spiral vector used across missions. |
| 12 | Visualization & support/API-fork | Well-organized for a debug tool: a clean Summarize→Aggregate→Render→Apply pipeline gated behind feature flags. Real survival-adjacent hazard is the engine size-limit throw + zero panic isolation, with no buffer-size accounting (get_visual_size/clear_visual never called). The fork divergence is genuinely minimal and upstream-shaped (visual structs only); the `.expect` convenience-panic is the one upstreaming concern. | Engine per-target visual-limit throw (IBEX-008) under no panic isolation: an oversized/duplicated-per-room draw can abort the tick and skip serialize_world; CPU/intent accounting is coupled to the debug flag. |

## 3. Cross-Cutting Architectural Findings

- **Persistence/entity model is the single highest-leverage architectural seam.** Two coexisting identity schemes — marker-remapped Entity refs (correct) and bare-u32 `entity.id()` indices (JobData squad ref, generation-erased, silently aliasing) — produce BOTH the per-tick `repair_entity_integrity` maintenance tax (Field Report E) and silent cross-reference corruption (Field Report D/A). The in-tree `EntityMappingData` (RoomName→Entity, rebuilt each tick) and `CreepOwner` (stable ObjectId) already prove the stable-ID pattern; adopting it for the durable op/mission/squad/creep graph deletes the repair pass and the entire dangling/aliasing class.
- **No global CPU governance.** `tick()` runs ~60+ systems unconditionally with no bucket-aware degradation; load-shedding is opt-in (`can_execute_cpu` in ~6–8 sites) and the heaviest consumers (transfer matching, war recompute at cadence=1, gather BFS, per-candidate `pathfinder::search`, `find_route`) bypass it. The mature rover movement budget is the reference implementation to generalize, not replace. This is the extinction-class gap.
- **Zero panic containment under `panic="abort"`.** One reachable panic anywhere in the pass aborts the tick, skips `serialize_world`, and persists partial heap mutations (env.tick advanced before run_systems). This converts any per-subsystem panic (IBEX-010 nuker withdraw, attack_mission get_room, a visual addVisual throw) into a whole-tick failure with no telemetry. A single tick-level containment boundary plus narrowing risky boundaries to Result is the structural fix.
- **Positional, unversioned wire format.** bincode DefaultOptions + no version header means schema drift silently corrupts; the documented `serde(default)` back-compat does not work for a truncated bincode stream and is actively relied upon by recently-added fields. A version header (reject-and-reset on mismatch) is the cheapest robustness win.
- **Two parallel formation movement APIs.** The military layer drives each quad member with an independent `move_to(range 0)` toward a virtual anchor while the rover's purpose-built `Follow`/`desired_offset` quad machinery sits entirely unused — a design-debt fork that must resolve to one cohesion model.
- **Observability is coupled to the debug flag.** CPU/intent accounting lives only inside the visualization overlay (CpuTrackingSystem produces CpuHistory only when viz is on); with viz off there is no always-on CPU-trend signal — exactly the death-spiral early-warning the rewrite needs.
- **Zero automated tests** across the crate and support crates, so format drift and reachable-panic regressions are caught only in production. The pure kernels (serializer, formation geometry, threat classification, transfer matching, spawn ordering, body calc) are testable today once a thin world-model/MemoryArbiter seam decouples game-API side effects.

## 4. Quick-Wins vs. Deep-Refactors

**Quick-wins** (localized, low-risk, don't break the running bot):
- Replace the Nuker-withdraw `panic!` (transfersystem.rs:208) with `Err(InvalidArgs)` + a one-shot log (IBEX-010) — closes a reachable tick-abort.
- Add a single tick-level `catch_unwind`/JS try-catch boundary so a system panic can never skip `serialize_world` (IBEX-025).
- Add a compile-time assert that COMPONENT_SEGMENTS excludes COST_MATRIX_SEGMENT, and a segment-fullness watermark log (IBEX-013/IBEX-014).
- Add `.max_ops` to `findnearest.rs` and `compute_nearest_spawn_distances`; bucket-guard `RoomRouteCache::compute_route` (IBEX-016/IBEX-035).
- Raise war cadences off 1 (defense ~2, offense ~10–20, recompute ~50) or gate them by bucket (IBEX-021).
- Wire `check_movement_failure` into job move states so a stuck creep self-deassigns (IBEX-015).
- Route `squad_combat` combat intents through `action_flags.consume(...)` (IBEX-029).
- Switch raw `game::time() - t` cadence/timeout subtractions to `saturating_sub` (IBEX-044); `saturating_sub` in store.rs free-capacity (IBEX-045).
- Replace the `staticmine.rs:201` unwrap with `if let` (IBEX-009 cleanup); explicit if-let at attack.rs:615 (IBEX-019) and a sentinel-return at attack_mission get_room (IBEX-020).
- Check off the todo.md body-sizing item (IBEX-022 refuted) and add a clarifying comment + unit test to spawn ordering so it is never re-flagged as inverted. (Do NOT reverse the spawn comparator — it is correct.)
- Cap per-target visual size via `get_visual_size`, clamp room-visual coords to finite, draw the global overlay once to None not per-room (IBEX-008).
- Add a per-squad cohesion metric (max member spread, ticks-in-Loose) to make Field Report A visible as data before the rewrite.

**Deep-refactors** (structural; feed the rewrite):
- Global CpuGovernor + single budgeted pathfinding facade; persist/warm the route cache (IBEX-003/IBEX-016) → ADR 0004.
- Key durable refs by stable game IDs; delete `repair_entity_integrity` (IBEX-005/IBEX-002b/Field Report E) → ADR 0001.
- Versioned/tagged schema-evolving serialization; dedicated cost-matrix segment (IBEX-004/IBEX-013) → ADR 0002.
- Replace the virtual-anchor N-independent-MoveTo cohesion model with lead-follower + hard in-range wait-gates; wire SquadDefenseMission onto SquadContext; retire orphaned squad missions (IBEX-001) → ADR 0003.
- Supervised mission/operation FSM with mandatory per-state deadlines and top-down abort; transient-error tolerance (IBEX-002/IBEX-042) → ADR 0003.
- Decompose transfersystem.rs / attack_mission.rs / visualization.rs / RoomData; extract pure matching/geometry/layout kernels as test seams (IBEX-024).
- Stand up the eval harness + always-on metrics segment + first pure-logic tests (IBEX-023) → ADR 0006.

## 5. Risk Register

| Risk | Subsystem | Likelihood | Impact | Trigger condition | Mitigation |
|---|---|---|---|---|---|
| CPU pathfinding death-spiral → colony collapse (IBEX-003/016) | Pathing / cross-cutting | Med–High | **Extinction** | Sustained pathfinding load / low bucket; multi-room war; post-reset cold caches | Global CPU governor + single budgeted pathfinding facade + graceful degradation (ADR 0004); persist/warm route cache; raise war cadences |
| Cost-matrix seg-55 wiped every tick → full rebuild on the worst tick (IBEX-013) | Memory & serialization | High | Critical | Any VM reset with normal-size ECS (i.e. always) | Move cost matrix off COMPONENT_SEGMENTS; compile-time disjointness assert |
| Deserialization failure → silent full state loss (IBEX-004/014) | Memory & serialization | Med | Critical | Any serialized-field add/reorder; payload truncation | Version header + reject-and-reset; round-trip/old-snapshot/fuzz tests; segment watermark telemetry |
| Reachable hot-path panic aborts tick + skips serialize (IBEX-010/025) | Transfer / cross-cutting | Med | High | Raid vs enemy nuker; any reachable unwrap in any system | Replace panic! with Result; tick-level catch_unwind boundary; panic counter telemetry |
| War/squad cohesion broken → war system unusable (IBEX-001) | Combat / Military | High (in war) | High | Any engagement; defense always (no wiring); offense under contention | Wire SquadDefense onto SquadContext; lead-follower hard wait-gates (ADR 0003); cohesion telemetry |
| Operation/mission lifecycle hang (IBEX-002) | Operations / Combat | Med–High | High | Squad cannot group; target unreachable; threat clears with squad alive | Per-state wall-clock deadlines; stop renewing non-cohesive squads; WarOperation supervisor (ADR 0003) |
| Raw-u32 squad ref aliasing → wrong/None squad state (IBEX-002b) | Combat / Jobs / Memory | Med | High | VM reset or ECS index recycle | Generation-carrying handle interim; stable SquadId store (ADR 0001) |
| Renderer per-target limit throw → no visual debugging + skipped serialize (IBEX-008) | Visualization | Med (debug-on) | Med (debug) / High (via IBEX-025) | World renderer enabled under heavy load | Size watermark + drop-with-telemetry; finite-coord clamp; tick-level containment |
| seg-60 planner restart-thrash → stunted base growth (IBEX-037) | Room planning | Low–Med | Med | Permanent layer-config fingerprint mismatch | Bounded restart counter + Failed{time} backoff |

## 6. Maturity / Score Rubric (1–5)

| Subsystem | Correctness | Robustness (tick/reset) | Performance/CPU | Maintainability | Strategic fitness |
|---|---|---|---|---|---|
| 1 Tick orchestration & ECS core | 3 | 2 | 3 | 4 | 3 |
| 2 Memory & serialization | 2 | 2 | 3 | 3 | 2 |
| 3 Operations (campaigns) | 3 | 3 | 2 | 3 | 2 |
| 4 Economy & infra missions | 4 | 3 | 2 | 4 | 3 |
| 5 Combat & expansion missions | 2 | 2 | 3 | 2 | 2 |
| 6 Jobs layer | 3 | 2 | 3 | 3 | 2 |
| 7 Military core | 3 | 3 | 3 | 4 | 2 |
| 8 Room data, visibility & planning | 4 | 3 | 2 | 3 | 3 |
| 9 Pathing & movement | 3 | 3 | 3 | 4 | 2 |
| 10 Transfer & market logistics | 3 | 2 | 2 | 2 | 2 |
| 11 Spawning, stats & helpers | 3 | 3 | 3 | 4 | 3 |
| 12 Visualization & support/API-fork | 3 | 2 | 3 | 3 | 3 |

## 7. Bug & Issue Register (for later individual deep-dives)

| ID | Title | Subsystem | Location | Symptom / observed impact | Status | Suggested validation (repro / test / log) |
|---|---|---|---|---|---|---|
| IBEX-001 | War/squad cohesion: quads scatter (Field Report A) | Combat / Military | squad_defense.rs:455/207, formation.rs:201–359/161–190, squad_combat.rs:576–646, attack_mission.rs:732–776 | Defense squads have NO formation wiring; offense squads string out under soft quorum gate | Confirmed (located) | Wire SquadDefense onto SquadContext; log per-tick member spread + ticks-in-Loose; lead-follower wait-gate |
| IBEX-002 | Operation/mission lifecycle hangs (Field Report B) | Operations / Combat | war.rs:1441, attack_mission.rs:662–716/644–647/1023–1046, squad_defense.rs:289–384, attack.rs:391–422/759 | Rallying renews stuck members so all-dead never fires; Engaging/Defending no timeout; no watchdog | Confirmed (located) | Per-state deadline forcing wave-wipe/Cleanup; launch attack on unreachable room, assert teardown |
| IBEX-002b | Creep→squad link is a non-remapped raw u32 index | Combat / Jobs / Memory | squad_combat.rs:18/904/835/915/974, cleanup.rs:146, game_loop.rs:168–369 | After reset/recycle, creep resolves wrong/None SquadContext → silent cohesion loss | Suspected (H) | Serialize squad+members, deserialize, assert same logical squad; recycle index, assert no foreign attach |
| IBEX-003 | CPU pathfinding death-spiral (Field Report C) | Pathing / cross-cutting | game_loop.rs:633–814, pathing/*, screeps-rover | Bucket exhaustion → tick-restart loop; no tick-level governor | Confirmed gap | Induce CPU pressure + reset in sim with active war; assert progress + no restart loop; CPU incl intents |
| IBEX-004 | Serialization brittleness: positional bincode, no version header (Field Report D) | Memory & serialization | serialize.rs:310–344, game_loop.rs:402/508/533, room/data.rs:84/199 | Field add/reorder silently breaks/mis-decodes old snapshots → spontaneous reset | Confirmed | Round-trip + old-snapshot corpus + fuzz; version byte + reject-on-mismatch |
| IBEX-005 | repair_entity_integrity hand-maintained, no-op default (Field Report E) | Tick / ECS core | game_loop.rs:168–369, missionsystem.rs:140 | New Entity field silently uncovered; per-tick repair tax | Confirmed | Derive repair list from a registry; test every serialized Entity-bearing component is covered |
| IBEX-006 | Job FSM friction (Field Report F) | Jobs | machine_tick.rs:3/13, jobs/*, screeps-machine | Split-pass side effects + unguarded combat intents + Option-as-control-flow opacity (multi-transition double-fire REFUTED) | Confirmed (reframed) | Pilot data-driven FSM on HaulJob; replay intent-diff; route combat through action_flags |
| IBEX-007 | Single-creep routing acceptable (Field Report G) | Pathing | movementsystem.rs, screeps-rover | CPU-bounded by 5 layered governors; NOT a cohesion cause | Confirmed-OK | Add routing unit tests; the acute pain is cohesion (IBEX-001), not routing |
| IBEX-008 | World renderer corrupts all rendering (Field Report H) | Visualization | visualize.rs:39–41/129–141, room_visual.rs:377–392, visualization.rs:1294–1410, extra.rs:137/266 | Per-target 500KB/1000KB limit throw + NaN coords desync; global overlay duplicated per room; throw skips serialize | Suspected (M; limits + to_value-panic claims corrected) | Log get_visual_size per layer; force >500KB room buffer; toggle construction.visualize.plan |
| IBEX-009 | staticmine resolve().unwrap() — REFUTED as panic (guarded) | Jobs | jobs/staticmine.rs:180–182/201 | Not reachable; container-gone handled by rediscovery | Refuted as panic; Low cleanup | Replace unwrap with if-let for future-proofing; no live-bug test needed |
| IBEX-010 | Transfer panic! on Nuker-withdraw TransferTarget | Transfer | transfersystem.rs:208, raid.rs:101–118, haulbehavior.rs:439 | Raid vs enemy nuker → panic halts tick + skips serialize | Confirmed (Nuker arm reachable; deposit-side arms NOT reachable) | Add log+Err at :208; test all (variant, mode) pairs; type-split enums |
| IBEX-011 | Partial-haul re-plans via Idle (commitment churn, not data loss) | Jobs / economy | haulbehavior.rs:513–567, haul.rs:217–224 | Hauler abandons in-progress delivery plan → Idle re-select churn (NOT stranded resources) | Open (reclassified) | Damage hauler mid-delivery; assert it finishes same target; add committed-delivery guard |
| IBEX-012 | SquadContext.members/heal_priority ARE repaired — REFUTED | Combat / Military | game_loop.rs:264–302 | Seed claim false; members + heal_priority retained on is_valid | Refuted (downgrade) | Round-trip killing a squad member; assert no dangling member; residual risk is IBEX-002b |
| IBEX-013 | Cost-matrix segment 55 wiped by serialize_world every tick | Memory & serialization | game_loop.rs:453–455/554, costmatrixsystem.rs:6/57, macro line 117 | Persisted cost matrix lost on every reset → full rebuild post-reset (feeds C) | Confirmed | Log seg-55 length after serialize; force reset, assert load_cost_matrix_cache non-empty; split segment |
| IBEX-014 | Deser failure unrecoverable + silent chunk-drop | Memory & serialization | game_loop.rs:444–450/508/533, memorysystem.rs:94–97 | Overflow chunks dropped with only a log; mid-decode failure swallowed → partial/empty world | Confirmed by-design (Med reachability) | Inflate ECS past 5 chunks; emit size + chunk count to metrics; fail loud |
| IBEX-015 | No job-layer stuck recovery (check_movement_failure dead code) | Pathing / Jobs | movebehavior.rs:17–26 (zero call sites) | Rover-abandoned creep loops same move forever; corridor cascade-freeze | Confirmed | Assign unreachable target; assert creep leaves move state within N ticks; wire recovery |
| IBEX-016 | Unbounded find_route + no global CPU governor | Pathing / cross-cutting | screeps-rover movementsystem.rs:1183–1199/935, economy.rs:237, findnearest.rs:61–133, structure_data.rs:174–206 | Pathfinding leaks bypass local budgets; repath_count never read; no governor | Confirmed gap | Add governor + budgeted facade; cap repath_count; .max_ops on uncapped callers; profile under pressure |
| IBEX-017 | Cost-matrix rebuilt every tick | Pathing | screeps-rover costmatrixsystem.rs:67–74, pathing/costmatrixsystem.rs:43–45 | Ephemeral creep/construction/structure costs cleared each tick → CPU sink | Confirmed TODO | Cache structures/construction across ticks with change-event/TTL; rebuild only creep layer |
| IBEX-018 | Market manipulation guards weak | Transfer / market | ordersystem.rs:349–351/367/446 | count/volume/stddev gate clearable by a fake high-volume day; no trend/spike check | Confirmed | Trailing-window median vs latest day; replay synthetic spike; exposure caps; prefer-local gate |
| IBEX-019 | attack.rs:615 guarded double-unwrap (latent) | Combat | operations/attack.rs:615/608–612 | Safe today via have_live_intel guard; would regress if guard moves | Confirmed latent | Convert to if-let bind over the join |
| IBEX-020 | attack_mission get_room .expect last-resort (latent) | Combat | missions/attack_mission.rs:1917 | Panic only in fully-degraded mission (no home/owner/squads), in serialize-critical path | Confirmed exists (latent) | Construct degraded mission, call get_room; return Option/sentinel |
| IBEX-021 | War cadences=1 → per-tick scan + O(A·H) cold-cache route rebalance | Operations | war.rs:139–141/546–930/1097–1241/626–630 | Heavy uncoordinated per-tick CPU; cold find_route storms after reset (feeds C) | Confirmed | Enable system_timing; confirm recompute runs every tick; raise cadences; re-measure bucket |
| IBEX-022 | Spawn body-sizing min-cost — REFUTED (clamp present) | Spawning | creep.rs:123–156, haul.rs:207, localbuild.rs:252, source_mining.rs:394 | Empty-fleet path clamps to SPAWN_ENERGY_CAPACITY=300; request IS queued | Refuted | Check off todo.md item; optional create_body edge unit test |
| IBEX-023 | Zero automated tests | cross-cutting | whole crate + support crates | No offline validation before deploy | Confirmed | Stand up eval harness + pure-logic kernel tests (Increment 0) |
| IBEX-024 | Oversized files | cross-cutting | transfersystem.rs 2439, attack_mission.rs ~2040, visualization.rs 1485, war.rs 1444, room/data.rs 899, squad.rs ~900 | Maintainability / complexity; testable kernels buried | Confirmed | Decompose during rewrite; extract pure kernels as test seams |
| IBEX-025 | No per-system panic isolation | Tick / cross-cutting | game_loop.rs:135–151/791–812/746, Cargo.toml:16, panic.rs | One panic aborts whole tick, skips serialize_world, persists partial heap mutations | Confirmed | Force a mid-pass panic; confirm no serialize + env.tick advanced; add tick-level catch_unwind + panic counter |
| IBEX-026 | AttackOperation economy-abort is dead code | Operations | attack.rs:402/87/137 | total_energy_invested never set → economy abort branch unreachable | Confirmed (Med) | Log at :402 branch; wire real spend or re-gate on total_waves>0 |
| IBEX-027 | BoostQueue subsystem inert | Military | boostqueue.rs (zero callers), game_loop.rs:606, composition.rs:552 | Boosted compositions never boosted; military under-powered | Confirmed (Med) | Decide wire-vs-delete; if wiring, add lab fulfillment + spawn gate on is_ready |
| IBEX-028 | No force-abort of excess AttackOperations on cap shrink | Operations | war.rs:559–569/937–949/1297–1312 | After room loss, active attacks exceed new cap and keep running | Confirmed | Drop room_count in scenario; assert active attacks trimmed |
| IBEX-029 | squad_combat fires combat intents UNGUARDED | Jobs | squad_combat.rs:994 + action sites | action_flags bypassed in the most-churned subsystem; latent double-fire | Confirmed (design) | Route combat through action_flags.consume; debug-assert no intent twice per creep/tick |
| IBEX-030 | Transfer matching O(haulers·priorities·rooms·targets·resources), no bucket gate | Transfer | transfersystem.rs:2113→1763→1641, haulbehavior.rs:206 | Per-creep greedy re-matching ungated; bounded by small per-hauler scope | Suspected (M) | Profile at many idle haulers; add can_execute_cpu gate + per-tick supply snapshot cache |
| IBEX-031 | Lazy transfer generators ordering — REFUTED as nondeterministic | Transfer | transfersystem.rs:1339–1356/484/496/602 | Reservation accounting prevents double-serve; node type filtering prevents cross-type leak | Refuted (Low) | Document lazy-flush contract; optional two-phase collect for testability |
| IBEX-032 | Linear (not route) distance for claim/colony/scout home gating | Operations / Expansion | claim.rs:563–564/740–742/167–178/737, colony.rs:139–143, scout.rs:190–193 | Can target route-unreachable / behind-hostile rooms | Confirmed (TODO at :737) | Insert hostile wall between home and candidate; assert rejected with route-distance gating |
| IBEX-033 | classify_threat over-triggers PlayerSiege | Military | threatmap.rs:207/109–117 | any_boosted or ≥4 creeps → Siege; T3-assumed 4.0× multiplier | Confirmed | Unit-test across boost tiers + DPS/heal/count; effective-DPS-vs-defense gate |
| IBEX-034 | EconomySnapshot military reserve flat 5k floor, not RCL-scaled | Economy | military/economy.rs:82–114 | Over-reserves low-RCL rooms; under-protects mature rooms | Confirmed | Log reserve vs stored_energy across RCL2–8; income-based reserve |
| IBEX-035 | compute_nearest_spawn_distances uncapped-by-default pathfinder::search | Economy | structure_data.rs:174–206 | Per spawn×target searches on 10-tick refresh; engine 2000-op default bounds each | Suspected (Med) | Add .max_ops; gate behind bucket; long/structural TTL |
| IBEX-036 | Unbudgeted gather BFS + uncached describe_exits | Room data | gather.rs:118–229, claim.rs:244 | Periodic Discover burst of dozens of describe_exits calls, no CPU budget | Confirmed (Med) | Log used-CPU delta + describe_exits count on a dense map; CpuBudget closure; use cached exits |
| IBEX-037 | seg-60 planner restart-thrash on fingerprint mismatch | Room planning | roomplansystem.rs:197–204/443–445 | Per-tick restart with no convergence; no attempt counter | Suspected (M) | Force a layer-config change after partial plan; observe re-restart; add restart counter + Failed backoff |
| IBEX-038 | Cost-matrix ephemeral data cleared & rebuilt every tick | Pathing | screeps-rover costmatrixsystem.rs:67–74 | Structures/construction re-found per tick; CPU sink (=IBEX-017) | Confirmed TODO | Change-event/TTL caching for structures+construction; measure CPU at 100+ creeps |
| IBEX-039 | find_route not ops-budgeted; runs before per-tick ops cap | Pathing | screeps-rover movementsystem.rs:1183–1199/1169–1177 | Room-graph search only headroom-gated; not charged to ops budget | Confirmed | Add a find_route call budget; test pathing to a far/unreachable room under low bucket |
| IBEX-040 | Shove/avoidance walkability is terrain-only | Pathing | screeps-rover screeps_impl.rs:142–155 | Creep shoved onto blocking-structure/hostile-rampart tile → move error, wasted tick | Confirmed | Place a creep behind a structure-blocked tile, force a shove, assert not pushed onto structure |
| IBEX-041 | Combat hostile/friendly lists re-cloned 3–4× per creep/tick | Jobs | squad_combat.rs:922–969 + call sites | Avoidable allocations/scans during war when bucket headroom is lowest | Confirmed | Profile contested-room tick before/after caching the hostile set once |
| IBEX-042 | One mission-state Err tears down the entire mission | Combat / Economy | missionsystem.rs:254–266, miningoutpost.rs:119/129, defend.rs:215 | Momentary room/visibility loss destroys long-running campaign + children | Confirmed | Inject one-tick room_data None; confirm mission deleted vs waiting; add MissionResult::Wait |
| IBEX-043 | Power-bank concurrency no-op filter counts all attacks | Operations | war.rs:766–776 | max_concurrent_power_banks meaningless; throttled by unrelated attacks | Confirmed | Count only PowerBank-reason attacks; assert independent slot filling |
| IBEX-044 | Raw game::time()-t subtraction in cadence/timeout | Operations / Jobs | war.rs:1314–1316, attack.rs:579/637, colony.rs:193, squad_combat.rs:165–168 | Underflow on time discontinuity (panic in debug / benign wrap in release) | Suspected (L) | Switch to saturating_sub; test with stored tick > game::time() |
| IBEX-045 | store.rs free-capacity unchecked u32 subtraction | Spawning / Jobs | store.rs:11–16 | Potential underflow panic if store API double-reports used>capacity | Suspected (L) | saturating_sub; debug assert logging when used>capacity |
| IBEX-046 | NaN partial_cmp coalescing (unreachable today; RepairQueue fully guarded) | Room / Transfer / Spawning | visibilitysystem.rs:248–250, transfersystem.rs:1842/2061, spawnsystem.rs:88–90, repairqueue.rs:64–75 | Coalesces NaN→Equal; no NaN source today; RepairQueue NaN already guarded | Suspected (latent) | debug_assert!(priority.is_finite()) at request sites; guard transfer divisors |
| IBEX-047 | Most economy missions never override repair_entity_refs | Economy / Combat | missionsystem.rs:140, source_mining/mineral_mining/haul/upgrade/localbuild/reserve | EntityVec creep lists rely solely on reactive remove_creep; dangling ref could break serialize | Suspected (M) | Deserialize a snapshot with a stale creep ref; confirm round-trip; consider EntityVec skip-invalid |
| IBEX-048 | Depleted-source/exhausted-mineral missions never torn down | Economy | localsupply/mod.rs:130–252, mineral_mining.rs:183–187 | ensure_children only ADDS; idle MineralMiningMission persists after depletion | Confirmed | Deplete a mineral; confirm mission runs forever as a no-op; add idle-timeout / prune |
| IBEX-049 | Per-creep path vectors serialized every tick | Pathing / Memory | pathing/movementsystem.rs:14–17, game_loop.rs:408 | CreepRoverData.path persisted each tick → segment size pressure (feeds IBEX-013/014) | Confirmed | Mark path #[serde(skip)]; compare serialized segment bytes before/after at scale |
| IBEX-050 | get_used_capacity double-count workaround duplicated 6+ times | Jobs | haulbehavior.rs:28/80/388/479, haul.rs:168 | All sites must change together if the upstream API bug is fixed | Confirmed (tech-debt) | Extract a single creep_used/free_capacity helper; grep for one definition |

## 8. Rewrite Direction & Architectural Alternatives

| Pillar | Current → Pain | Recommended direction | ADR |
|---|---|---|---|
| Entity model | specs/ECS recyclable Entity indices used as durable cross-refs → dangling-ref repair tax (E) + raw-u32 squad aliasing (D/A) | Stable-game-ID arena/store (RoomName / ObjectId / minted SquadId), id→Entity rebuilt per tick; delete repair_entity_integrity. Interim: generation-carrying handle | ../design/0001-entity-model.md |
| Serialization | bincode positional + no version header → silent breakage on schema drift (D); seg-55 collision | Stage 1: version header + reject-and-reset + round-trip/old-snapshot/fuzz tests (any format). Stage 2: tagged/schema-evolving format; dedicated cost-matrix segment | ../design/0002-serialization.md |
| Behavior modeling | screeps-machine FSM → split-pass side effects, unguarded combat intents, opaque Option control flow (F); one Err tears down a mission | Data-driven FSM behind the Job/Mission trait seam; ALL intents through one guarded sink; transient-error tolerance; pilot on HaulJob with replay parity | ../design/0003-behavior-modeling.md |
| Squad cohesion | N independent move_to(range 0) toward a soft-gated virtual anchor; defense unwired; rover Follow unused (A) | Lead-follower + hard in-range wait-gates (use rover Follow/desired_offset); cohesion as an invariant; wire SquadDefense; retire orphaned missions | ../design/0003-behavior-modeling.md |
| CPU governance | none → death-spiral (C); leaks bypass local budgets; ephemeral caches wiped on reset | Global CpuGovernor (Normal/Conserve/Critical) + single budgeted pathfinding facade; generalize the rover budget; persist/warm route cache; tiered shedding | ../design/0004-cpu-governance-and-load-shedding.md |
| Runtime / scheduling | specs dispatch, zero panic isolation under panic=abort | Explicit ordered/priority scheduler (NOT async-for-its-own-sake) + tick-level catch_unwind containment + narrow risky boundaries to Result; resumable work only where needed | ../design/0005-runtime-and-scheduling-model.md |
| Eval & iteration | manual iteration, zero tests, observability coupled to debug flag | Rust harness around screeps-launcher (deploy via js_tools/deploy.js interim) + always-on metrics segment + pure-logic kernel tests; colony-health score + pre-deploy gates | ../design/0006-eval-and-iteration-harness.md |

**Per-pillar detail:**

- **Entity model (ADR 0001).** Two identity schemes coexist; both fail from the same root (recyclable indices as durable keys). Adopt stable-game-ID keying — `EntityMappingData` and `CreepOwner` already prove it in-tree. A missing ref becomes a handled lookup-miss, not a serialize panic, deleting `repair_entity_integrity` and closing both Field Report E (repair tax) and the IBEX-002b aliasing. Whether specs stays as the *dispatch* substrate is independent of the *identity* decision. Migrate squads first (smallest, most broken): A1 generation-carrying handle (Behavioral interim) → A2 SquadStore by minted SquadId → A3 mission/operation ownership by id, then delete the pass.

- **Serialization (ADR 0002).** Stage 1 is the cheapest robustness win and ships non-breaking (a version header + reject-and-reset turns silent garbage into a clean reset; the encode/decode helpers are pure so a MemoryArbiter double makes the pipeline testable today). Stage 2 swaps the body to a tagged/schema-evolving format behind the frozen `serialize_world`/`deserialize_world` seam, after ADR 0001 removes Entity wrappers from the payload. Independently, make COMPONENT_SEGMENTS and COST_MATRIX_SEGMENT provably disjoint (closes the Critical IBEX-013) and add a fullness watermark.

- **Behavior modeling (ADR 0003).** Multi-transition is NOT the double-fire source (the action_flags guard neutralizes it) — refute that in the register. The real friction is split-pass side effects, unguarded combat intents, and Option-as-control-flow. Replace the per-job representation behind the unchanged Job trait with a data-driven FSM that funnels all intents (incl. combat) through one guarded sink and computes reservations once per tick; add transient-error tolerance. Use utility AI for target/role SELECTION only, not sequential execution. Pilot on HaulJob with replay intent-diff parity before rollout.

- **Squad cohesion (ADR 0003).** Replace N-independent-MoveTo-toward-a-soft-anchor with lead-follower + hard in-range wait-gates (the rover already provides Follow/desired_offset), escalating to single-fat-position only if needed. FIRST wire SquadDefenseMission onto SquadContext — without it, defense scatters regardless of the Level 2 model. Make cohesion an invariant: a squad non-cohesive for N ticks force-aborts (also closes the Field Report B hang). Behavioral-only (SquadContext already serializes virtual_pos/formation_mode).

- **CPU governance (ADR 0004).** Land EARLY — it is None-breaking, survival-critical, and the scheduler hook the runtime model rides on. A single CpuGovernor read at the top of every expensive system tiers the flat ~60-system pass into a sheddable pass; a single budgeted pathfinding facade owns a shared per-tick ops pool so findnearest/structure_data/RoomRouteCache/find_route draw from the same budget as movement. Under Critical, shed war recompute and expansion/visuals first; keep the MIN_PATHFIND_OPS floor so creeps never fully freeze. Persist/warm RoomRouteCache so the post-reset tick degrades gracefully. Always-on death-spiral telemetry (bucket trend, ticks-since-progress, repath storms, restart counter) feeds both the shed trigger and diagnostics.

- **Runtime / scheduling (ADR 0005).** Pick the SIMPLEST model that supports load-shedding and resumable work: an explicit ordered/priority scheduler, NOT a cooperative async executor. For panic containment, prefer narrowing risky boundaries to Result/log-and-continue (transfer target enums, visual flush) PLUS a single tick-level catch_unwind boundary so serialize_world always runs. Add resumable work only where a concrete need exists (planning already resumes via seg-60). A runtime-MODEL change off specs rides on the ADR 0001 decision and is deferred.

- **Eval & iteration (ADR 0006).** Build the Rust harness around screeps-launcher (shelling out to js_tools/deploy.js until a reqwest code-upload client lands); land it FIRST as the verification substrate for every other increment. In parallel, add a dedicated always-on metrics segment (CPU + intent count, bucket, GCL/RCL, throughput, creep counts, active ops/missions, death-spiral signals) decoupled from the visualization flag, plus pure-logic unit tests for the highest-ROI stable kernels.

**Sequencing.** Increment 0 (gate: none): eval harness + metrics segment + first pure-logic tests. Increment 1 (gate: harness can induce CPU pressure): global CpuGovernor + budgeted pathfinding facade + tick-level panic containment + priority-scheduler seam at parity (None-breaking, survival-critical → first). Increment 2 (gate: round-trip/old-snapshot/fuzz green): serialization Stage 1 version header + disjoint-segment assert + overflow watermark (one-time intentional reset on a low-stakes tick). Increment 3 (gate: dangling-ref counter emitting): generational-handle interim → stable-ID SquadStore. Increment 4 (gate: store stable + cohesion metric emitting): wire SquadDefense → lead-follower wait-gates → retire orphaned squad missions. Increment 5 (gate: ADR 0001 store landed): serialization Stage 2 format swap + migrate mission/op ownership to ids + delete repair_entity_integrity. Increment 6 (gate: replay parity infra): data-driven FSM piloted on HaulJob, rolled out job-by-job + transient-error tolerance. Each increment is validated by the harness before the next; never break the running bot mid-increment; confine the two intentional one-time resets (Increment 2, Increment 5) to low-stakes ticks.

## 9. Observability & Self-Improvement Plan

- **CPU + intent accounting (today incomplete).** CPU is measured execution-only and only behind the debug timing flag (game_loop.rs:139–143); statssystem records bucket/used but no per-system breakdown and ZERO intent accounting — systematically under-measuring because action intents charge CPU when logged. Make the per-system `get_used()` delta an always-on accumulator at the `for_each_system!` instrumentation point, and add an intent counter at the world-model/GameView side-effect choke point (per-category: move/transfer/attack/build/repair/spawn). The move-intent cost is already known in-code (`MOVE_ACTION_CPU=0.2`, movementsystem.rs:255) but never aggregated. Validate intent costs against the open-source engine.

- **Death-spiral early-warning signals** (feed both the runtime shed trigger and post-hoc diagnostics): bucket trend (negative delta over a sliding window — the leading indicator), ticks-since-progress (GCL/RCL/stored-energy stall), repath storms (sum of `repath_count` increments — currently written but never read, movementsystem.rs:935), simultaneous-repath count, env-reset/restart counter (extend the env.tick discontinuity check), long-tick rate (used >= tick_limit), pathfinding-ops saturation, and serialize-skipped count (a rising count is a direct death-spiral signal since serialize_world is skipped on a panic/abort).

- **Console telemetry (events/errors, budgeted).** One structured JSON line per significant EVENT (not per tick): panic caught, deserialize failure, segment overflow, operation/mission force-aborted, stuck-operation watchdog fire (Field Report B), squad-out-of-range-entered-combat (Field Report A). Severity-tagged so panics/deser-failures/stuck-ops surface as DATA, not silent degradation. Batch counters once every N ticks.

- **Segment telemetry (periodic metric snapshot).** A DEDICATED, versioned metrics segment (propose seg 57 — 50–55 ECS+costmatrix, 56 stats_history, 60 planner, 99 live stats are taken): `{ver, tick, cpu_used, cpu_limit, tick_limit, bucket, intents_by_category, gcl, gpl, rcl_by_room, energy_throughput, creeps_by_role, active_ops, active_missions, threat_max, deaths, restart_counter, deser_failures, panics_caught}` plus the death-spiral signal block and a per-segment fullness watermark. Add a version header to ALL metric/state segments (seg 99/56 are currently unversioned JSON and mis-decode silently on schema change).

- **World-model abstraction = the biggest testing+rewrite enabler.** A thin GameView trait that abstracts every game-API read and every intent so decision logic takes `&impl GameView` → `Vec<Intent>`. The pattern already works in-tree (transfer's `&dyn TransferRequestSystemData`, threatmap's HostileCreepInfo DTO, rover's cost-injection closures). It makes pure decision functions host-target-testable against in-memory fixtures and enables record/replay of real GameView reads to reproduce Field Reports B/C deterministically offline.

- **Test strategy.** Most dangerous to leave untested (survival-ranked): serialization round-trip (deser failure is unrecoverable), entity-ref repair invariant (a dangling ref panics ConvertSaveload), CPU governor decision logic, reachable panics (transfer arms, attack_mission). Testable TODAY (pure / fixture-drivable): create_body, classify_threat, formation geometry (virtual_anchor_target / advance_squad_virtual_position), spawn ordering comparator (lock in the KNOWN-CORRECT behavior), serialize encode/decode round-trip, stats_history downsample cascade, transfer value formula, foreman plan scoring. Balance: the strategy shell churns fast — test the STABLE KERNEL and pin the experimental shell with integration/eval-harness assertions, not brittle unit tests.

- **Offline feedback loop (recursive self-improvement).** Pull console events + the metrics segment after a run → reduce into a colony-health time-series → diff vs a stored baseline keyed by (scenario, git SHA) under `runs/` → flag regressions (CPU headroom down, deaths up, GCL slope down, any nonzero deser-failure/panic/spiral-alarm) into the Bug & Issue Register → redeploy a candidate → re-measure. Record/replay reproduces a flagged regression deterministically offline. Prefer a single-language Rust crate (bollard + reqwest + the screeps-api crate) per repo convention.

- **Eval harness (ADR 0006).** Dockerized screeps-launcher private server (much faster than MMO): server lifecycle via bollard → bootstrap/scenario via the server CLI (reset, password, low tick-duration, place spawn, optional opponent bots) → deploy via js_tools/deploy.js (interim) → run-control (advance N ticks / until condition) → data via the screeps-api crate (console + cpu) + reqwest for segment reads → colony-health scoring → comparison persisted under `runs/`. Step 1 smoke loop → Step 2 scenarios + score + gates → Step 3 regression detection + CI. Private-server CPU != MMO, so it validates correctness/behavior/regressions, not absolute MMO CPU numbers.

- **Pre-deploy gates.** wasm32 build + host-target test build pass; serialization round-trip + old-snapshot-corpus green; reachable-panic guards covered (create_body, spawn-ordering descending invariant, classify_threat, formation off-grid→None); clippy/fmt clean; sim smoke-run with ZERO deser failures, ZERO caught panics, ZERO segment-overflow drops, CPU under tick_limit, no death-spiral alarm; colony-health score on the economy-bringup scenario not regressed below baseline beyond threshold.

- **Colony-health score (the objective function).** Four weighted terms, survival-dominating: (1) SURVIVAL gate+score — avoided extinction (no death-spiral, no unrecoverable deser failure, spawn alive), a spiral run scores ~0 here regardless of other terms; (2) CPU HEADROOM — mean/p95 of (tick_limit − cpu_used)/tick_limit including intent cost, plus long-tick rate; (3) ECONOMIC GROWTH — slope of GCL + stored energy + RCL-progress, per-scenario normalized; (4) MILITARY WIN-RATE — rooms held vs lost, squad cohesion rate (fraction of combat ticks all members in-range — directly measures Field Report A), targets killed vs waves spent. Weights and per-scenario normalization fixed in config so the score is reproducible and diffable; record the term breakdown so regressions are attributable.
