# Combat Overhaul — Master Status & Plan

- **Owner:** William Archbell
- **Last updated:** 2026-06-19
- **This is the source of truth for combat/war STATUS and REMAINING WORK.** Forward-looking status lives here; landed-task detail (commit SHAs, per-task validation cells) lives in [`../execution/phase-2.md`](../execution/phase-2.md); design rationale lives in ADR [0008](../design/0008-combat-and-squad-architecture.md) (+ tactics annex [0008a](../design/0008a-combat-tactics.md)).

## 0. How to use this doc

| Doc | Role |
|---|---|
| **THIS doc** | Current status by workstream + the single ordered remaining-work plan + gates. Supersedes the phase-2.md Status columns. |
| ADR [0008](../design/0008-combat-and-squad-architecture.md) (+ [0008a](../design/0008a-combat-tactics.md) annex) | Design end-state / why. 0008a = the ~55-tactic catalog + ~70-knob param table + EXP-* register. |
| [`phase-2.md`](../execution/phase-2.md) | Cold-resume per-task historical log: §2.0 newest-first status log w/ commit SHAs, §2.2–2.7a per-task validation cells, §2.8 sequencing graph, §2.9 checkpoints, §2.1 operator-settled constraints. |
| ~~g4-offense-plan.md / g3-tail-plan.md~~ (DELETED) | Old sub-plans, removed 2026-06-18 — their remaining work folded into §5–6 here, landed history into [`phase-2.md`](../execution/phase-2.md) §2.0. Full text in git history if ever needed. |

One-line rule: **forward-looking status lives here; landed-task detail lives in phase-2.md; design rationale lives in ADR 0008.**

## 1. The problem (one paragraph)

Squads were ineffective in three named ways: (1) **null tactics** — "just stand and ranged-mass-attack" (no kiting in the ordered path, best-effort focus-fire, oscillating retreat, flat-HP heal math); (2) **orphan → idle** — combat creeps weren't mission children, so on objective completion the `SquadContext` was deleted but creeps kept a dangling `squad_entity` and idled until TTL; (3) **scatter** — cohesion was N independent solo pathfinds against a virtual anchor, members trickle-spawned rooms/ticks apart. The fix copies the **scout** subsystem (request→claim→complete→release→retire): a global `CombatObjectiveQueue` + a `SquadManager` lifecycle/tactics owner + an anchor mover + synchronized spawning.

> **Status note (2026-06-18):** the tactics / cohesion / lifecycle layers are **LIVE on master**. This paragraph is the original motivation, not the current state. (Supersedes the stale "design+ADR+plan only / just stands RMA" framing in MEMORY.md and the "no production code yet" line that previously headed this doc.)

## 2. Architecture at a glance

```
CombatObjectiveQueue  (global persistent priority/TTL queue; ephemeral claims)
        │  request() upsert-merge by kind; best_unclaimed_near; mark_unwinnable backoff
        ▼
SquadManager  (single perpetual system: reconcile → field rosters → compute orders → claim, ≤4 squads)
        │  Phase A reconcile/re-claim · B spawn-demand · B2 orders · C claim-new
        ▼
pure decide_squad_with_pathing / decide_combat / decide_movement   (screeps-combat-decision, 41 host tests)
        │  focus-fire · coupled engage/retreat hysteresis · orientation · heal assignment · scored kiting
        ▼
SquadCombatJob  (executes orders + Recall)
```

- **Movement is anchor-primary** (footprint "moving-maximum" pathfind, lockstep block advance) — **NOT** lead-follower. Corridors handled by *relaxing the same mover*; `Follow`/`pull` reserved for no-MOVE bodies only (M4, conditional).
- **Crates:** `screeps-combat-engine` (JS-free mechanism) · `screeps-combat-decision` (pure tactics + seam) · `screeps-combat-agent` (sim glue + scripted opponents) · `screeps-combat-eval` (scenarios/metrics/parity/replay).
- Design: ADR 0008 §3–5. Tactics catalog + EXP register: ADR 0008a.

## 3. Current status — by workstream

This table **supersedes** the phase-2.md Status columns (including the stale §2.5-line-177 "G4 unstarted" cell).

| WS | Rollup | Detail |
|---|---|---|
| **H** harness/engine | **PARTIAL** | H1/H2/H3 + **H4 DONE** (`screeps-combat-agent`, 28 tests: rush/kite/turtle/drain roster, `run_engagement` + tower controller + `CombatRecording` capture, `world_from_units`, `replay::to_svg`). **H5 IN PROGRESS** — `screeps-combat-eval` crate landed: the **EXP-* register as a metric-producing suite** (`register`/`report`; 5 experiments FOUND/KITE/FOCUS/TOWER/COMP passing = the tactics-tuning loop). Remaining H5 = sim-vs-server **parity oracle** + byte-exact golden vectors + nightly seeded gate (need Docker-capture integration). **U-series harness expansion STARTED** — U1 (`ScenarioBuilder`: room variety w/ walls/ramparts/towers/chokes) + U2 (recording fidelity for metrics) DONE (`08d22ab`); U3–U10 = structure-target apply layer → richer metrics → self-play/stalemate scoring → scenario suite → behavior fixes (see §5(b) U-roadmap). |
| **I** identity | **UNSTARTED** | Manager interim-keys squads by `SquadContext` Entity. I1 SquadStore/SquadId + I2 re-key both UNSTARTED. |
| **M** movement | **PARTIAL** | M1 DONE (footprint primitive). M2 code-done + sim-validated + live-swapped (WFV 7→8); orientation gap closed via G4-O2; **behavioral live-combat validation pending (M2-LIVE)**. M3 DONE (sim). M4 conditional. |
| **G** goals/squads | **DONE** | G1/G2/G3/G3-tail + G4 (defense-half + offense O1/O2/O3/O4/O6 + SK-DEADCODE + **O7 legacy deletion**) all landed. O5 dropped, G4-HEAVY deferred (§5(g)). **All combat is now objective-driven via the SquadManager — no legacy AttackMission/AttackOperation code remains.** |
| **W** war supervisor | **PARTIAL** | W1 DONE (defense → Defend, default ON). **W2/W3/W4 UNSTARTED** (supervisor trim/economy-abort; Escort producer; WarDecl posture). |
| **S** spawning | **UNSTARTED** | S1 sole-demand-producer (GroupId=SquadId) + S2 boost handoff (gated ADR 0010). |
| **K** source-keeper | **PARTIAL** | K0/K1/K2/K3/K5 DONE. K2c-2 (military_free gate + farm retirement) + K-RECONCILE UNSTARTED; **K4 mineral DEFERRED/BLOCKED** (needs remote extractor/container + market-glut predicate). |

## 4. Resume point + do-next ordering

**RESUME POINT (2026-06-19):** after **P2.G4-O7** (offense legacy deleted; G workstream complete) + the **H4 first increment** (scripted opponents + adversarial harness). Combat slice on master at Docker **WFV 13**; host green (bot 157, rover 27, decision 41, agent 25), wasm + clippy clean. **LIVE COMBAT PATH VALIDATED on Docker (2026-06-19):** with natural invader cores + remote invaders present (and an injected W7N5 stronghold), the bot produces `Dismantle` objectives (O6.1: W5N7, W7N3 cores) + `Defend` objectives (W1: remote invaders in W8N4), and the `SquadManager` fields combat squads (census: 8 combat creeps across W8N4/SK rooms). So the objective→manager→squad pipeline fires end-to-end. **Remaining validation gaps:** (a) the W7N5 stronghold isn't scouted yet (non-reserved → opportunistic visibility is slow), so the O3 *breach* + tower fight specifically aren't exercised; (b) confirm a Dismantle siege-quad actually clears a core (offense MEDIUM competes with defense/SK under `MAX_CONCURRENT_SQUADS`).

**Do-next ordering:** **The entire G workstream (incl. G4 offense migration) is DONE** — O7 deleted the legacy (WFV 12→13, deployed); all combat is objective-driven via the `SquadManager`. O5 (power-banks) + G4-HEAVY (heavy assault) are deferred future capabilities (§5(g)). **Remaining are the independent tracks: H4/H5 (harness), I1/I2 (SquadStore/SquadId), W2–W4 (war supervisor/escort/posture), S1/S2 (synchronized spawn), K2c-2/K-RECONCILE/K4 (SK), plus the validation/future items in §5(g).** Recommended next: validate the objective/manager combat path on the Docker soak (offense ON), then pick up **I1** (foundational — unblocks S1 + a future G4-HEAVY) or **H4** (unblocks the EXP-* tuning loop). Full dependency-ordered queue in §5.

## 5. Remaining work (the execution plan)

Dependency order. Each item: id · what · why · deps · gated_on · effort.

### (a) G4 finish

- **P2.G4-O5 — DONE-by-dropping (2026-06-18).** Power-bank farming was found **non-functional** (the neutral bank is never targeted — `get_hostile_structures` excludes unowned structures, `select_focus_target` only picks hostile ones; and `CollectResources` has no executor, so the "haulers" were `SquadCombatJob` that just idled). There was no working power-farming to preserve, so O7 is **not** gated on a power-bank port. Resolution: the offense scan no longer produces a `PowerBank` candidate (it only wasted a duo + haulers); removed `power_bank_min_ticks_needed` / `count_power_bank_attacks` / `max_concurrent_power_banks` + their tests. **Real power-bank farming is deferred to its own ADR-gated workstream — see §5(g) Power-bank farming.**
- **P2.G4-SK-DEADCODE — DONE (2026-06-18).** Removed the dead `AttackReason::SourceKeeper` variant + its `build_force_plan` arm + status label in `operations/attack.rs` (SK is fully `SourceKeeperOperation→Farm{SourceKeeper}`; the `duo_sk_farmer` composition is kept — still used by the SK farm). Not deployed standalone (removing a serialized enum variant rides O7's WFV-bump reset, so it doesn't wipe the running soak). Bot host 157, wasm + clippy clean.
- **P2.G4-O7 — DONE + DEPLOYED (2026-06-18).** Deleted `missions/attack_mission.rs` + `operations/attack.rs` (AttackMission/AttackOperation/AttackReason/AttackPhase/PlannedSquad/DeployCondition/ManagedSquad + the heavy-assault planner `plan_by_detected_threat`/`build_force_plan`), the `MissionData::AttackMission` + `OperationData::Attack` variants + their dispatch arms + `mission_type!`, the mod exports, and all of war.rs's active-attack machinery (`ActiveAttack`, `active_attacks`, `reassign_home_rooms`, the heavy-recompute threat-propagation, `is_attacking_room`/`get_attack_for_room`/`add`/`remove`/`cleanup_dead_attacks`, the `TargetSource→AttackReason` From impl, the dead `_ => None` launch branch). `run_offense_evaluation` is now purely objective-driven; `run_heavy_recompute` shrank to the cap calc + border-visibility refresh. **WFV 12→13** (removed serialized enum variants → loud reset). Pure dead-code deletion (the legacy was already runtime-unreachable). **`SquadCombatJob` is now the only combat-driving job — "no legacy combat-driving mission code remains."** Bot host 157, wasm + clippy clean. *(The heavy multi-squad player assault is the deferred future capability in §5(g); its design survives in ADR 0008a.)*

### (b) Harness

- **P2.H4 — DONE** (`screeps-combat-agent`, commits `a1e517d` + `d5a8747`; 28 tests, clippy clean). Scripted opponent roster `RushAgent`/`KiteAgent`/`TurtleAgent`/`DrainAgent` (each a `TacticalAgent` emitting `MoveTo`/`Flee` goals routed through the same rover pathfinder the bot uses — pathfound, not raw directional; a wall-routing test proves it). `run_engagement` head-to-head runner (IbexAgent vs opponent → outcome + side-A cohesion + side-B tower energy) with a scripted tower controller (tower scenarios) and per-tick `CombatRecording` capture. `world_from_units`/`Unit` composition builder (adversarial tests field our AI in arbitrary compositions). `replay::to_svg` SVG filmstrip scrubber + `examples/replay_demo` (emits a replay SVG per scenario). Validated scenarios: kiter-beats-rusher, focus-fire-beats-turtle, quad-vs-strong-turtle, drain-bleeds-tower. *Remaining (→ H5):* aggregated EXP-* scoring + parity oracle. *Why:* enables the 0008a EXP-* register; closes the CP-H self-play arm.
- **P2.H5** — Dockerized-server parity oracle + nightly N=9 seeded combat gate + byte-exact golden-vector capture. *Why:* acceptance gate / fidelity oracle; tightens the sim parity budget; the live seg-57 cohesion canary is the final arbiter. *Deps:* H4. *Gated_on:* depends on H4. *Effort:* L.

#### Harness-expansion roadmap (U-series — "dramatically improve combat logic & behavior")

The ultracode design pass (wf_c4ad0572, 11 agents, ground-truthed the engine/harness) decomposed the operator goal — *"expand scenarios; variety of rooms (walls/ramparts/towers); stronger metrics (healing/DPS/positioning/survivability/efficiency); bots play each other incl. stalemate scoring; dramatically improve single + squad combat; maybe multi-room for group-up/renew/traversal"* — into U1–U12. **Sequence:** scenario+metrics foundation (U1–U7) before behavior fixes (U8–U10); multi-room (U11/U12) is an XL engine rewrite, deferred (observed symptoms are all within-room — see §5(g) note).

- **P2.U1 — DONE (`08d22ab`).** `ScenarioBuilder` (`screeps-combat-agent/src/scenario.rs`): fluent composition of `CombatWorld` terrain (`wall`/`wall_column`/`wall_row` with choke gaps / `swamp_rect`), passive structures (`cwall`/`rampart`/`spawn`/`perimeter`), firing `tower`/`tower_nest`, `safe_mode`, `from_units`/`empty`/`world_mut`. All synthesized coords clamped/bounds-checked to 0..=49 (edge perimeter/nest can't panic). 4 tests.
- **P2.U2 — DONE (`08d22ab`).** Recording fidelity for the new metrics: `CreepResult.raw_damage`, `TowerFrame` + `TickFrame.towers` snapshot, additive `TickFrame.destroyed_kinds` (structure-kind on death). Additive only — `destroyed_structures` kept. Engine 40 / agent 32 / eval 2 green, clippy clean.
- **P2.U3** — Structure-targeted apply layer: `to_engine_action` (combat-decision→engine seam) must carry an attack/dismantle **target StructureId** so a creep can hit a specific wall/rampart/tower/spawn, and `SimView` must expose structure ids to the decision layer. *Why:* breach/dismantle scenarios are unscorable until a creep can target a structure (today only creep-vs-creep resolves). *Deps:* U1, U2. *Effort:* M.
- **P2.U4** — Tower re-key by `StructureId` (record + scripted controller key towers by stable id, not list index) so multi-tower nests survive partial destruction in replay/metrics. *Why:* `tower_nest` + tower-as-target (U3) make index-keying ambiguous once one dies. *Deps:* U2, U3. *Effort:* S.
- **P2.U5** — Richer metrics module `screeps-combat-eval::metrics`: healing (HP restored / overheal waste), DPS (raw vs effective post-rampart/TOUGH — uses U2 `raw_damage`), positioning (focus-range occupancy, kite distance held), survivability (TTL, parts lost front-to-back), efficiency (energy/boost spent per enemy part killed). **Review must-fixes baked in:** (a) attribute tower damage separately from creep DPS (don't contaminate creep DPS with tower hits); (b) cohesion via `combat-decision::cohesion::measure`, not a re-implementation. *Deps:* U2, U3. *Effort:* M.
- **P2.U6** — Symmetric `EngagementOutcome` + self-play / stalemate scoring: today the outcome is side-A-centric. Make it symmetric (both sides' metrics), add an IbexAgent-vs-IbexAgent runner, and a **stalemate score** — the review flagged the original `residual_EV` as an *inverted* incentive; redesign so a stalemate is scored on residual military advantage (surviving effective-DPS + heal throughput + board position), not on who merely has more HP. *Why:* "bots play each other and how to score a stalemate" is an explicit operator goal. *Deps:* U5. *Effort:* M.
- **P2.U7** — Scenario suite + EXP-register expansion: open skirmish / walled-with-gap / rampart bunker / tower nest / corridor / mixed base, each as a registered experiment with U5 metrics. Lights up BREACH (ramparts+repair), the full COMP-1 uniform-brick-vs-2+2 + TOUGH sweep, DEF-2. *Deps:* U1, U5, U6. *Effort:* M.
- **P2.U8** — Single-creep behavior fixes (the SINGLE-* P1 set surfaced by the design pass): kiting/focus/heal-self decisions for the solo path. **High-conflict file** (`combat-decision/lib.rs`) — serialize against U9/U10. *Deps:* U5, U7. *Effort:* M.
- **P2.U9** — Squad behavior fixes (SQUAD-* P1). Highest-leverage = **SQUAD-1: collapse the triple/inconsistent retreat logic** — make the `squad_combat.rs` FSM consume `decide_squad`'s `SquadOrderState` as the single source of truth and delete the per-state hardcoded 50%/40%/80% thresholds (`squad_combat.rs:184,366,676`). Plus SQUAD-2/3/4 (group-up, target reconciliation, heal assignment). **High-conflict** (`squad_combat.rs` + `combat-decision/lib.rs`) — serialize. *Deps:* U5, U7, U8. *Effort:* L.
- **P2.U10** — P2/P3 behavior backlog (the long tail of the design-pass findings once P1 lands + scenarios quantify them). *Deps:* U8, U9. *Effort:* L (ongoing).
- **P2.U-TOWER** — Tower↔creep fire cohesion (operator 2026-06-19: *"run our tower logic in the same system as our creep combat… only when no combat intents / no active combat in a room, the towers run repair or other behaviors"*). Today `TowerMission` (`missions/tower.rs`) decides tower targets in a **separate system** from `decide_squad`: it already coordinates all towers onto one target + gates on net-positive damage + has drain detection + repairs only when `hostile_creeps.is_empty()` — but it picks its *own* target, so a defense squad and the towers can split fire onto different enemies and neither overcomes the aggregate heal as fast as combined fire would. **Fix:** a pure **`decide_towers(room_view, squad_focus: Option<FocusTarget>)`** in `screeps-combat-decision` that ports the net-damage/drain/coordinated-fire logic and **prefers the squad focus when one is supplied and firing on it is net-positive**. **API constraint (operator):** the signature must serve **both** worlds — squad present *or* absent (passive base defense is the common live case → `squad_focus: None` → towers fall back to their own best-target selection), and the **multi-room live world** (towers are room-scoped; the adapter looks up the active defense squad's focus for *that* room, if any) **and** the **single-room sim** (the scenario supplies the focus directly). Live `TowerMission` + the harness both become thin callers; the persisted drain *tracking* (sawtooth state) stays live-side and feeds the view as a "conserve-against-these-ids" input. *Deps:* U5, U6 (build + a defense-squad+towers scenario in U7 so the combined-fire win is **measured** before the live tower system is touched). *Gated_on:* operator chose **after U5/U6 metrics**. *Effort:* M.
- **P2.U11 / U12 — DEFERRED (multi-room, XL).** Engine is hard single-room (`CombatTerrain`/`CombatWorld` model one 50×50 room; `StructureId`/`CreepId` aren't room-qualified). Group-up / renew / room-traversal evaluation needs a multi-room world model + inter-room pathing in the engine — an XL rewrite. The design pass recommends **deferring**: every observed live symptom (null tactics, scatter, orphan-idle, retreat thrash) is within-room and U1–U10 address them. Revisit when within-room behavior is solid + a traversal-specific symptom appears. *Gated_on:* operator decision (see §5(g))."

### (c) Identity

- **P2.I1** — SquadStore + minted, generation-carrying `SquadId`; `resolve(id)→same-squad-or-None`. *Why:* hard gate for CP-I; eliminates dangling-ref risk (IBEX-049 family); G2 currently keys on Entity (interim). *Deps:* G2. *Gated_on:* nothing blocking; ADR 0001 A1→A2. *Effort:* M.
- **P2.I2** — Re-key `SquadContext`/`SquadCombatJob` by `SquadId`; dangling-ref counter to seg-57; `#[serde(skip)]` on `CreepPathData.path` (IBEX-049); bump WFV. *Why:* completes the identity workstream, closes CP-I, removes the Entity-keying interim. *Deps:* I1. *Gated_on:* depends on I1. *Effort:* M.

### (d) War supervisor

- **P2.W2** — `WarOperation` as supervisor: withdraw/trim low-value objectives when `max_concurrent_attacks` shrinks (IBEX-028); feed real per-squad spend so economy-abort fires (IBEX-026); UnwinnableTarget backoff on the supervisor side. *Why:* only the defense half of migration step 5 is done; the supervisor still doesn't throttle/withdraw offense by real spend or cap shrink. *Deps:* O6. *Gated_on:* benefits from per-squad spend instrumentation. *Effort:* M.
- **P2.W3** — `Escort{room}` pre-clear producer in `claim.rs` for marginal claim targets (`DefenseEscalation::from_threat` sizing). *Why:* the `Escort` kind is defined + handled by `squad_manager` but has NO producer — inert; ADR 0017 expansion pre-clear. Operator decided "build it." *Deps:* O6. *Gated_on:* nothing blocking; unstarted. *Effort:* M.
- **P2.W4** — Thin `WarDecl` posture hook (player-offense only under WarDecl); feature-flag proactive de-reservation OFF (T-CTRL-3); register S11. *Why:* posture/ADR-0014 governance; reactive reserve denial always-on, proactive flagged OFF. *Deps:* W2. *Gated_on:* nothing blocking; unstarted. *Effort:* M.

### (e) Spawn

- **P2.S1** — `SquadManager` as sole combat spawn-demand producer (GroupId=SquadId); align-finish group admission + pre-spawn replacement. *Why:* synchronized spawning (ADR 0011); closes CP-S; pre-spawn deferred from G2. *Deps:* I1, I2. *Gated_on:* depends on SquadId. *Effort:* M.
- **P2.S2** — boost-on-spawn handoff (IBEX-027) behind kill-switch. *Why:* boost-commit policy (conservative floor, downgrade to unboosted when short). *Deps:* S1. *Gated_on:* gated on the ADR 0010 lab pipeline. *Effort:* M.

### (f) Source-keeper

- **P2.K2c-2** — Replace hardcoded `military_free: true` (`sourcekeeper.rs` L324) with a real yield-to-defense/war predicate; add Withhold/Veto retirement of an EXISTING farm when ROI drops or it falls out of haul range. *Why:* SK farm never stands down for active defense/war and never proactively retires an over-the-hill farm. *Deps:* K2. *Gated_on:* needs the war/defense posture signal (overlaps W4). *Effort:* M.
- **P2.K-RECONCILE** — Extract shared `ensure_source_mining(gate)` helper (SK duplicates `LocalSupplyMission`'s source loop); convert the outpost `DefendMission` child into a `Defend` objective. *Why:* de-dupes per-source vs room-level mining child logic; folds the last defender path onto the queue. *Deps:* K3, G4-defense. *Gated_on:* cleanup; nothing blocking. *Effort:* M.
- **P2.K4** — SK mineral mining. *Why:* completes the SK economic surface. *Deps:* K3. *Gated_on:* **DEFERRED/BLOCKED** — needs remote extractor/container construction + a market-glut predicate (ADR 0012 is trading-risk, not mining-glut). *Effort:* L.

### (g) Cross-cutting / future

- **HEAVY MULTI-SQUAD PLAYER ASSAULT (deferred capability — was G4-HEAVY, NOT an O7 blocker).** The towers≥4 drain-duo + sequenced quad (`plan_by_detected_threat`: `duo_drain` then `quad_ranged` with `DeployCondition::AfterSquad{Engaged}`) is the only thing the legacy `AttackOperation` ever planned that the one-squad-per-objective model can't express — but it is **already runtime-dead**: O6 maps every offense candidate (incl. AttackFlag, which used to trigger it) to a single-squad objective, so no `AttackOperation` is created and the heavy path never runs. O7 deletes its planner code (the design survives in ADR 0008a T-TOWER drain tactics). Rebuilding it later needs: a **multi-squad / sequenced-objective mechanism** in the queue+manager (`ForceRequirement.squads` is a Vec but the manager reads only `.first()`; add `DeployCondition`/`AfterSquad` sequencing across squads), plus `SquadId` (I1/I2) for stable multi-squad coordination, and is only worth it under `attack_players` (default OFF) — i.e. when the operator wants aggressive player offense. *Deps:* I1, I2, the multi-squad mechanism. *Gated_on:* operator demand for player assault + design (not started). *Effort:* L.
- **POWER-BANK FARMING (deferred workstream — needs an ADR first).** Replaces the dropped O5. A proper greenfield feature (~K2/K3-scale), gated on **a new ADR (`docs/design/00XX-power-bank-farming.md`) — TODO, author before implementing.** Verified requirements:
  - **Dedicated *healed* assault squad — NOT any squad.** Attacking a power bank applies `damage × POWER_BANK_HIT_BACK (0.5)` back to the attacker each tick (engine `_damage.js:35-36`), so an attacker dealing 600/tick takes 300/tick back and dies without ~300 HEAL/tick of support. Targeting must be **objective-scoped** (only a power-bank-farm squad attacks the neutral bank) — do **not** make the generic combat decision target neutral structures (every squad would wander onto banks unhealed). Likely shape: a `Farm{PowerBank, room}` objective whose composition is a boosted/large attacker+healer set, with the bank tile supplied as an explicit focus (the bank isn't a hostile structure, so `select_focus_target` won't pick it).
  - **Predictive, coordinated dropped-power collection.** The power drops on the ground in a HIGHWAY room (not a transfer-queue source — `HaulMission` can't read it), so a **bespoke collector job** is needed. Dispatch must be **predicted/timed** so CARRY haulers arrive *as the bank cracks* (travel time vs. remaining bank HP / DPS), and pick up **only when ready** — not camping the room for the full kill, not arriving after the power decays (`ceil(amount/1000)`/tick ≈ 5/tick for 5000 → forgiving, but still bounded). Coordinated with the assault squad's crack ETA.
  - **ROI/economy gate + niche cadence:** power banks are intermittent + highway-only; gate like the SK ROI scorer (worth the boosted squad + haul round-trip). A `PowerBankOperation` + coordinator mission mirroring `SourceKeeperOperation`/`SourceKeeperFarmMission` is the natural home.
  - *Deps:* O7 (or parallel), the ADR. *Gated_on:* author the ADR; then build. *Effort:* L.
- **MULTI-ROOM COMBAT EVALUATION (deferred — U11/U12, XL engine rewrite).** Evaluating initial group-up / renew / room-traversal needs the combat engine to model >1 room (today `CombatWorld`/`CombatTerrain` are hard single-room; ids aren't room-qualified) + inter-room pathing in the sim. The ultracode design pass recommends **deferring**: every observed live symptom is within-room (null tactics, scatter, orphan-idle, retreat thrash), and U1–U10 cover them. **Operator decision pending:** build multi-room now vs. after within-room behavior (U8–U10) is solid. *Gated_on:* operator call. *Effort:* XL.
- **P2.M2-LIVE** — M2 anchor-mover live behavioral validation (private server). *Why:* CP-M/M3 is code-done but not behaviorally live-validated. *Deps:* O6. *Gated_on:* needs a live Docker soak with an actual threat. *Effort:* S.
- **T-POS** — Attack-positioning experiment (`plan_engage_anchor`): reuse `search_scored` with combat attack-pricing (maximize EV — damage to focus incl RMA stacking, heal coverage, optimal weapon range; minimize damage taken). *Why:* generalizes scored search from kiting to offensive positioning. *Deps:* G3-tail, H4. *Gated_on:* operator — explore strictly AFTER kiting (now done); benefits from H4 self-play. *Effort:* M.
- **L1** — Cross-room flee/kite evacuation: when `LocalPathfinder::search_scored` finds no safe in-room tile, fall back to a server PathFinder multi-room flee (`search_many(flee)`); keep single-room scored search primary. *Why:* a creep cornered at a room edge with the threat between it and the interior can't escape; concrete K5 trigger (stronghold appears in farmed SK room, last duo/miners take tower fire exiting). *Deps:* G3-tail, K5. *Gated_on:* document + watch on Docker soak first; live-only, not self-play-validatable. *Effort:* M.
- **L2** — Trait-based combat view (avoid per-tick DTO copy on live path): `CombatCreep` live-impl over `screeps::Creep`, sim-impl over `SimCreep`, decisions generic. *Why:* drops per-tick copy/alloc (CPU+GC) on the live combat path. *Deps:* H2. *Gated_on:* **MEASURE FIRST** — DTO build reads cached RoomData (cheap-ish); gate on a measured live CPU win. *Effort:* M.
- **EXP-REGISTER** — Run the ADR 0008a ordered EXP-* register. **STARTED (`screeps-combat-eval`, H5):** FOUND-1 / KITE-1 / FOCUS-1 / TOWER-1 / COMP-1 are live as metric-producing experiments (`register()`/`report()`, all passing). *Remaining:* the harder items as the sim grows the needed surface — BREACH (ramparts+repair), DEF-2/CTRL (controllers), the full COMP-1 uniform-brick-vs-2+2 + TOUGH sweep, NPC, and PARITY (server). *Why:* turns the ~55-tactic catalog + ~70-knob table from hypotheses into shipped defaults; G3 tactics are live but largely untuned. *Deps:* H4 (done), H5. *Effort:* L (ongoing).
- **P2.M4** — pull for under-MOVE compositions (Follow/pull for no-MOVE/under-MOVE bodies). *Why:* only sanctioned use of lead-follower. *Deps:* M2-LIVE. *Gated_on:* conditional — only if under-MOVE compositions get fielded; otherwise skip. *Effort:* S.
- **CP-CHECKPOINTS** — Reach CP-I/CP-G/CP-W/CP-S/CP-K + close CP-H/CP-M live + audit the 11 M4 exit criteria. *Why:* checkpoints are the milestone gates for the whole Phase 2 overhaul; all 11 exit criteria pending. *Deps:* O7, I2, W4, S2, K2c-2, M2-LIVE. *Gated_on:* aggregates all workstreams + live validation. *Effort:* M.

## 6. Legacy / inert code (tracking)

**The offense legacy is GONE (O7, 2026-06-18):** `AttackMission`, `AttackOperation`, `AttackReason`, the heavy-assault planner, and war.rs's active-attack machinery are all deleted. All combat is objective-driven via the `SquadManager`. Remaining inert/cleanup items (not legacy combat drivers, just loose ends):

- `ObjectiveKind::Escort` — defined + handled by the manager, **no producer** (inert; W3 adds one).
- `sourcekeeper.rs`: `military_free: true` hardcoded (TODO K2c-2/W); no Withhold/Veto retirement of an existing farm (K2c-2).
- `TargetSource` (war.rs) retains a few never-constructed variants (DefendFlag/ThreatResponse/Expansion/InvaderCreeps/PowerBank/ProactiveDefense) — harmless pub enum variants; prune opportunistically.

## 7. WORLD_FORMAT_VERSION ledger

| Change | WFV |
|---|---|
| M2 anchor mover | 7→8 |
| G1 CombatObjectiveQueue | 8→9 |
| G2 SquadManager | 9→10 |
| K3 per-source mining | 10→11 |
| G4 defense-half | 11→12 |
| G4-O7 offense legacy removal | 12→13 (**current**) |
| I2 (SquadId field) | pending |
| O7 (removed enum variants) | pending |

**Standing rule:** `WORLD_FORMAT_VERSION` in `game_loop.rs` MUST bump on any serialized-shape change = one loud reset (reset-anytime policy).

## 8. Operator-settled constraints + decisions

Standing rules (single home):
- **Harness** = hybrid combat micro-sim running the bot's OWN code (focused deterministic sim + complementary Docker server).
- **Movement** anchor-primary, NOT lead-follower; corridors via relaxing the mover; `Follow`/`pull` for no-MOVE bodies only.
- **Squad on objective-complete** = retask-if-viable-else-recycle (Recall).
- **Boosts** wired behind a kill-switch (conservative floor; downgrade to unboosted when short).
- **Missions provide context / jobs own creep intent** including movement. The `AttackMission` §5 inversion is being unwound — new work must NOT extend it.
- **Anti-overfitting:** no opponent-specific constants; threat read at runtime; seed + opponent diversity; the live **seg-57 cohesion canary is the final arbiter**.

Old Q1–Q7 decisions, now SETTLED: Q1 retask-on-complete (realized); Q3 wire boosts behind kill-switch; Q4 orphan recovery realized via Recall + retask; Q5/Q2 subsumed by O4 wave-retry + unwinnable backoff + `MAX_CONCURRENT_SQUADS` cap; Q6/Q7 as recorded. (Stale crate name `screeps-ibex-metrics` → the landed crate is `screeps-combat-decision`.)

## 9. Checkpoints + exit criteria

(Milestone view; entry/exit gates in phase-2.md §2.9.)

- **CP-H** — code-done; **PARTIAL** (military-score-moves on combat is kernel-tested only; the LIVE combat-move carries to H4). M2 milestone not fully closed.
- **CP-M** — code-done; behavioral live-combat validation PENDING (M2-LIVE).
- **CP-I / CP-G / CP-W / CP-S / CP-K** — NOT reached.
- All **11 M4 exit criteria** PENDING.

Unblock map: CP-H/CP-M ← live soak + H4; CP-I ← I1/I2; CP-G ← O7; CP-W ← W2–W4; CP-S ← S1/S2; CP-K ← K2c-2 (+ K4 deferred).

## 10. Doc map + history pointers

- **Design end-state:** ADR [0008](../design/0008-combat-and-squad-architecture.md) (+ [0008a](../design/0008a-combat-tactics.md) tactics annex + EXP register).
- **Landed-task history with SHAs:** [`phase-2.md`](../execution/phase-2.md) §2.0 (newest-first); §2.9 checkpoints.
- **Old sub-plans (DELETED 2026-06-18):** `g3-tail-plan.md` (8-step kite) and `g4-offense-plan.md` (O-series) were removed once complete — their remaining items live in §5, the legacy delete-tracking in §6, and the `AttackReason→ObjectiveKind` mapping in ADR 0008 §2. Landed history with SHAs: [`phase-2.md`](../execution/phase-2.md) §2.0. Full original text recoverable from git history.
- **This doc is the source of truth for status and supersedes the phase-2.md Status columns.**