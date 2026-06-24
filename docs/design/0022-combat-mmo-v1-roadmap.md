# 0022 — Combat → MMO v1 roadmap (end-state, foundation-first)

Status: **Accepted 2026-06-23, revised same day.** Operator-driven re-plan; supersedes the P1–P5 / R-ladder *sequencing* in ADR 0020 §10–§12 (its sizing *design* still stands). Revised after the operator reviewed v1 of this ADR as **under-committed**, plus a 7-agent revision+gap workflow and two interview rounds.

## Why this exists

We repeatedly deployed combat that did **zero attack damage**, because work shipped as incremental stepping-stones — every soak was missing "one more piece." Operator directive: **build each area to its end-state; prove movement/objectives/identity/sizing in isolation BEFORE the first combat soak; reconcile all outstanding work; don't soak incomplete code; deliver real MMO value.** Two governing principles from the revision:

- **Foundation-first, but the end-state is fully committed.** Build the dependency tree bottom-up: each layer is a *complete* end-state piece, not a stepping-stone, and the top-level deliverables (the auction, the N-room sim, unified defense) are all in scope — just sequenced after the foundation that makes them buildable and validatable.
- **Reset-freedom.** Private-server resets are free; ticks are cheap. We will **not** deploy to MMO until *all* roadmap objectives are complete. So: choose the correct serialized shape freely, bump WFV during dev as needed, and fold everything into **one** intentional deploy reset. Correctness > reset-count.

## Three live blockers (the real "combat does nothing" causes)

Verified in code; these matter more than anything in the original plan:

1. **`repair_per_tick` is hardcoded `0`** (`military/threatmap.rs:364` → `war.rs:801`). The `DefenseProfile.repair_per_tick` field exists and `assess()`'s "repair out-paces our DPS → don't commit" veto consumes it — but the producer always emits 0, so that veto is **dead code**. The oracle declares repair-locked stalemates winnable → the engage-stall-die failure on anything that repairs.
2. **`squad_entity` is a bare `u32` resolved without a generation check** (`squad_combat.rs:18` + ~8 hot paths via `entities.entity(id)`). specs returns whatever *live* entity now occupies that index, so a recycled slot silently resolves to a **different squad's** `SquadState`/orders — a live aliasing bug and a likely hidden cause of "creeps do weird things."
3. **No combat energy-ROI gate.** `war.rs` offense never consults `economy.can_rooms_afford_military`; cumulative-siege spans creep *generations* with effectively unbounded re-spawn → a correctly-sized squad the colony can't sustain = the economy death-spiral.

> **Foundation progress — current state (2026-06-24).**
>
> *Tier-1 DONE:*
> - **P-ENGINE** — N-room combat engine: room graph + exits + engine-faithful cross-room edge-exit relocation + per-room terrain + **objective beds with active repair** + multi-room `ScenarioBuilder` + win condition. Offline scenario gate green (cross-room travel / flee-across-border / attacker-vs-objective). Design + status: **ADR 0023**.
> - **P-MOVE / P-MOVE+** — anchor delegated to rover; rover `LocalPathfinder::search` rewritten as a **true multi-room A\*** + real `find_route` Dijkstra. The sim squads (`SimSquad` + `ManagedSimSquad`) now route through rover's live `MovementSystem` + resolver — **one traffic-managed mover, sim ≡ live**. Two root-caused fixes landed: rover resolver made **deterministic** (`Handle` tie-break in `resolve_conflicts` — std-HashMap per-process seed was choosing the contested-tile winner; rover `85a1d30`); combat creeps take **`High` movement priority** so the shooter wins the forward kite tile (was parking the healer forward + the shooter out of range). Previously seed-flaky kite test now **24/24** across fresh processes. (super `48fddbd`, agent `1834340`.)
> - **P-ID** — squad-id validate-on-access (`47b2b0a`, WFV 16→17). **Blocker #1** — `repair_per_tick` producer wired (`d07afc7`, `threatmap.rs:371` emits `estimated_repair`, un-deading the `assess()` veto).
>
> *Tier-1 REMAINING (dependency order → first soak):* **P0** (freeze remaining contract stubs + CPU contract + ship the **per-squad observability dump** — the hard soak-gate, *not yet built*), **P-OBJ** (objective lifecycle: `SuccessPredicate`/retask-with-oracle/`Recall`/recycle/partial-wipe/re-scout/`Escort` — only a partial `SuccessPredicate` exists in `objective_queue.rs`), **P-FORCE** (unified force/engagement core + **member-scaling sizing (D3)** + cumulative-siege + **blocker #3 energy-ROI gate** — `force_sizing.rs` has `assess`/`RequiredForce` but is still **solo-spec** (`as_solo_spec`), no member-scaling/cumulative/ROI), **P-SPAWN** (synchronized spawn + rally gate — *not built*), then **PROVE-1** (offline integration gate in the N-room engine + FIRST live combat soak).
>
> **▶ Recommended next build: P-FORCE.** It is the direct fix for the headline "trickle-in-and-bail" engage-retreat under-sizing bug the operator has repeatedly flagged (SK-farming root cause), and it is now **validatable offline** against the just-built objective beds (P-ENGINE). It also un-deads blocker #1's veto in anger and folds in blocker #3. P0's observability dump should land before PROVE-1 (it gates the soak, not the offline sizing work). Strict roadmap order lists P-OBJ before P-FORCE, but P-OBJ's retask-fitness *consumes* the oracle, so building the oracle/sizing (P-FORCE) first is the lower-risk order.
>
> *Blocker #3 (energy-ROI gate)* stays DEFERRED into P-FORCE (task #29): its meaningful form bounds cumulative-siege spend across generations, which lands in P-FORCE.

## Decisions

★ = interview answer; ⚑ = operator override of the analysis recommendation; ▲ = refined this revision.

| # | Decision | Resolution |
|---|---|---|
| D1 ▲ | SquadId | **ECS Entity identity, stored as raw `(index, generation)` plain-serde data + validate-on-access (`entities().is_alive`)** — NOT a `ConvertSaveload`-serialized bare `Entity` (that re-triggers the deleted-entity-unwrap → wasm halt). `repair_entity_integrity` + `EntityCleanupQueue` kept. Fixes blocker #2 across **all** hot paths, not just `JobData` typing. |
| D2 ★▲ | Composition | **Auction over a parameterized ARCHETYPE roster, sim-validated by composition tournaments** — the committed end-state. **Foundation-first:** closed-form member-scaling sizing (D3) + minimal archetype **selection over existing templates** (Lanchester-margin + affordability) ship first and fix the headline bug; the parameterized auction + EV-currency ranking + objective-bed tournament land as a committed second increment on the proven foundation + N-room engine. |
| D3 ⚑ | 50-part cap | **Lanchester-driven member-count scaling** (`ceil(parts/cap)` members, distribute evenly, MOVE-ratio per member), NOT "defer." Defer only when one max-energy member can't deliver a role's minimum. This *is* the direct fix for the P2b engage/retreat under-sizing bug — size to hold from one calc. |
| D4 ⚑ | Defense | **Unify defense + offense NOW** through a shared "force-to-win-an-engagement" core (offense: towers-as-threat + structure objective + travel budget; defense: hostile-creep DPS as threat, OUR towers as negative incoming, objective = kill attackers, field-now, no travel). Static `DefenseEscalation` stays the **bootstrap/fast-path until the shared core is proven** (defense sim bed), then cut over — so working defense is never regressed. |
| D5 ▲ | Invader/stronghold kill | **Cumulative-siege (oracle reads the core's live `hits` as residual — cores never heal) + a real stronghold FIGHT model**: `DefenseProfile` gains `stronghold_level`, `defender_dps/heal/count`, `rampart_repair_per_tick`, `tower_heal_of_defenders_per_tick` (level-gated by `towerRefillChance`). Winnability for repairing targets = net DPS (ours − rampart_repair − tower-heal-of-defenders) > 0, with **burst-kill feasibility** (focused DPS > tower-heal on one defender) + the ROI cap (blocker #3). Level-0 = trivial. |
| D6 ▲ | R-attack (ranged sizing) | Folds into D3: cores are dismantle-immune → sized via RANGED parts; `RequiredForce` gains `ranged_parts`; structure-DPS counts RANGED (already landed `543350c`). |
| D7 ⚑ | Multi-room sim | **Extend the combat engine to N ROOMS** (room graph + exits + creeps persisting across borders + cross-room tower fire + objective beds with active repair). Enables integrated multi-room flee/attack/group-up *and* the auction's composition tournament to be simulated end-to-end. |
| D8 ⚑ | Anchor pathfinding | **Fix now by EXTENDING ROVER** (operator: rover is good/fast). Root cause: `AnchorPath::repath` (rover `anchor.rs:143-147`) provides a cost matrix for only the current room + no `find_route` → cross-room routing fails while members route correctly → divergence = wandering. Fix = delegate the anchor to the rover `MovementSystem` (one pathfinding path; footprint via a cost-matrix option) so anchor + members **cannot diverge**; covers **advance AND retreat**. Ephemeral → no shape change. |
| D13 | INVULNERABILITY | war.rs pre-filters deploying cores + a re-assess timer (never permanently abandoned; mirrors ADR 0021 re-scout). |
| D14 ▲ | Importance margin | Fixed formula for v1; reset-freedom means a per-objective field can land later if tuning needs it. |
| Obs ★ | Observability | **Hard P0 gate:** a per-squad live dump (objective + oracle verdict + required force + FSM state + anchor pos/`stuck_ticks`/Blocked + member HP + last-retreat-reason), gating entry to any soak. Every prior soak failed *silently*; this is the operator's confidence instrument. |
| Boost ★ | Boost ceiling | **Unboosted v1, ceiling stated:** clears invader cores / SK rooms / L1-2 strongholds. **L3-5 strongholds require boosts → deferred to P5** (lab handoff S2). Stated so L3+ not-clearing is no surprise. |
| CPU | Live/offline split | **Live runs only the closed-form path** (assess + archetype pricing/selection, no sim), under a per-tick CPU bench at N=cap candidates. **All simulation (tournaments, objective beds, integration) is offline/CI.** Running a sim per live decision is a CPU death-spiral (documented). |
| Retask ★ | Retask-fitness + recycle | Retask only on completion: re-run the oracle/selector vs candidate B; reuse healthy survivors only if their comp wins B, else **retire → recycle at nearest spawn** (recover ~half body cost), never idle-to-TTL. Preemption guard: don't trim a squad with high sunk progress/energy unless the preemptor is strictly higher tier. |

## Roadmap (foundation-first; everything committed)

**Tier 1 — the foundation (a complete combat system: clears cores/SK/L1-2 + defends, with template selection + closed-form sizing).** Proven live before the auction is built.

### P0 — Decisions + contracts + observability + CPU contract
Lock D1–D14 (above); freeze contract stubs (SquadId access helper, `SuccessPredicate` trait + per-`ObjectiveKind` list, shared engagement-model signature, `RequiredForce` + `ranged_parts`, `DefenseProfile` final fields incl. defender/repair intel); the live/offline CPU contract; **ship the per-squad observability dump as a hard deliverable** (acceptance: dumps a live squad's full why-state). Exit: stubs `cargo check` green; observability dump works on a live squad.

### P-ENGINE — Extend the combat sim engine to N rooms
The sim substrate everything validates against. N-room `CombatWorld` (room graph + exits + per-room terrain), creeps persist across borders, cross-room tower fire, **objective beds with active repair** (core + towers + ramparts + defender-repair + tower-heal-of-defenders), a win condition. Multi-room `ScenarioBuilder`. Exit: a multi-room scenario (squad crosses, fights across a border, flees across a border) runs deterministically; an attacker-vs-objective bed resolves to win/lose. Runs in parallel with the bot-side foundation (sim-side, independent until validation).

### P-MOVE — Anchor multi-room routing fix (extend rover) + retreat
Delegate the anchor to the rover `MovementSystem` (or `find_route` + per-room matrices) so anchor and members share one pathfinding path; footprint via a cost-matrix option; **covers advance AND retreat** (reverse-anchor route, not per-creep flee). Scenarios in P-ENGINE: cross-room travel, regroup-at-edge, group-up-then-engage-across-border, **flee-across-rooms**, corridor/tight-terrain, **stuck-member-timeout**. Trace-capture + offline replay with **input-parity** (the bot's `CombatView` inputs, not just positions). Exit: scenarios deterministic; live movement soak replays with parity; squads no longer wander.

### P-ID — SquadId validate-on-access (blocker #2)
Replace every `entities.entity(id)` squad lookup with a stored `(index, generation)` + `is_alive` validate-on-access; recycled slot → None, never aliases. Route squad deletions via `EntityCleanupQueue`. Exit: unit test (recycled slot resolves None); serialize/deserialize across a VM reset preserves creep→squad binding; zero dangling refs on a soak.

### P-OBJ — Objective lifecycle (zero orphans) + scouting
Centralized `SuccessPredicate` (manager-polled; fold `sourcekeeperfarm` withdraw into `Farm`); **retask-on-complete with oracle fitness** (re-run vs B; reuse-if-wins else retire→**recycle**); `Recall` FSM terminal state; **partial-wipe → retreat+reform** (lose healers / a role hits zero → retreat, not fight-to-wipe); strategic re-scout scheduler (generalize ADR 0021); `Escort` producer; **core-priority HIGH + light preemption with a sunk-progress guard**; seg-57 orphan-at-tick + recycle + per-kind completion counters. Exit: predicate unit tests; complete→retask no idle + never retask to a losing objective; killed/partial squad → retreat/Recall/recycle, none idle-to-TTL; soak orphan-at-tick = 0.

### P-FORCE — Unified force/engagement model + sizing (blockers #1, #3)
**Wire `repair_per_tick`** from a real producer (defender structure-repair + tower-repair, level-gated) — un-deads the existing veto (blocker #1). **Stronghold FIGHT model** (D5: defender HP/heal/repair, burst-kill term). **Closed-form Lanchester member-scaling sizing** (D3 — fixes P2b engage/retreat + SK-trickle). **Cumulative-siege** via live core hits. **Energy-ROI gate** (blocker #3: `cumulative_spawn_energy` vs economy surplus + target value; abort over-budget sieges). **INVULNERABILITY skip + re-assess timer** (D13). **The shared "force-to-win-an-engagement" core** that both offense and defense call (D4 unify-now; static escalation stays bootstrap until the shared core passes a defense sim bed, then cut over). **Minimal archetype selection over existing templates** (Lanchester-margin + affordability) — the auction's precursor. SK sized to worst-case concurrent keeper DPS × hold-margin (respawn-burst). Exit: host tests — repair-aware winnability (repair-locked target defers); member-scaling sizes a 100k core / multi-keeper SK to hold within budget; ROI gate aborts an unsustainable siege; defense via the shared core matches/beats static escalation in the defense sim bed; `force_sized_squad_keeps_holding_while_damaged` green.

### P-SPAWN — Synchronized spawn + rally + economy gate
One-tick spawn from a shared `GroupId=SquadId` token, deterministic `slot_index` order; `Forming→Rallying` hard gate (rally-or-timeout before Moving). **Economy/spawn-contention gate** (consult `military_spawns_claimed` + a max military-spawn-share; stagger on 1-spawn colonies). Exit: a SIZED 4-member squad spawns within window at equal TTL, gathers, advances only after the rally gate; spawn doesn't starve economy creeps.

### PROVE-1 — Foundation soak (FIRST live combat) + offline integration gate
**Offline integration gate** (in P-ENGINE): assess→size→spawn→move→engage→kill→retask→recycle in the N-room engine, + an unwinnable-defers case. **Live soak:** winnable core / SK 0-deaths / L1-2 stronghold clear end-to-end + defense, with template selection + closed-form sizing (no auction yet). Exit: integration gate passes; live soak clears + defends with no wander/orphan/engage-retreat-cycle, observability dump confirms behavior. This proves the *complete foundation* before the auction.

**Tier 2 — the optimization layer (committed), on the proven foundation.**

### P-AUCTION — EV currency + archetype auction + sim-validation
**EV currency R7** (one unit: net future enemy hits prevented; `EV(focus/breach/drain)` defined). **Parameterized archetype roster** (`military/archetypes.rs`) + the **greedy Lanchester-stable auction** (`select_composition_via_auction`). **Sim-validation:** composition tournament over a cost ladder (RCL4→RCL8+) in the N-room objective beds; gate = the auction's pick agrees with the simulated tournament winner within the exploitability margin at every cost point; regression-locked golden vectors. Live = closed-form auction only (CPU bench). Exit: tournament gate green across cost points; live CPU within bench; **PROVE-2** live soak (auction-selected squads clear + defend).

### P-DEPLOY — MMO v1
WFV re-proof (enumerate all shape changes since the last MMO version → **one** folded reset); `attack_players` OFF; `MAX_CONCURRENT_SQUADS` 4. Deploy via screeps-pack **only after explicit operator go-ahead**. **Stated ceiling: clears cores / SK / L1-2; L3+ → P5.** Post-deploy watch: seg-57 canary + per-kind counters; rollback = scatter/orphan regression.

### P5 — Boosts + exotic + scale
Boost handoff S2 (lab-gated) → **L3-5 strongholds**; full G4-HEAVY multi-squad; full war-supervisor (economy-abort/posture); player offense (`attack_players` + adaptivity + exploiter gate); composition-space exploitability gate; SK mineral K4; live empirical tuning (HOLD_MARGIN, importance_margin, Lanchester n).

## Keep / drop / redo (reconciliation)
**KEEP:** force-sizing ladder R1–R6 + hold-margin; P2a intel; comp templates (now archetype seeds); Lanchester gate + safeMode veto + rampart-redirect fidelity; CombatObjectiveQueue + SquadManager; scouting architecture; SK K0–K3/K5; `repair_entity_integrity`/`EntityCleanupQueue`; `MAX_CONCURRENT_SQUADS=4`; the landed fixes (border ping-pong `24f5494`, cores→attack `01e6f62`/`543350c`, re-scout Step-1 `3a687ef`, structure-DPS ranged `543350c`).
**REDO/WIRE:** `repair_per_tick` producer (blocker #1); `squad_entity` validate-on-access across all hot paths (blocker #2); anchor → rover multi-room (D8); `SuccessPredicate` + retask-with-oracle + Recall + recycle + partial-wipe (P-OBJ); member-scaling sizing (D3); shared engagement core + defense cutover (D4); energy-ROI gate (blocker #3); observability dump.
**FOLD-IN (now v1, sequenced):** N-room combat engine (D7); archetype auction + EV currency R7 + composition tournament (D2, Tier 2); synchronized spawn (ADR 0011); Escort producer.
**DROP:** ADR 0001 minted SquadId (D1 → Entity); the "single-reset-as-a-gate" constraint (→ reset-freedom, one folded deploy reset); curated-library-as-the-mechanism (templates become archetype seeds).
**P5:** boosts/L3+; full war-supervisor; player offense; composition-space exploitability; SK mineral; tuning.

## Open risks
- **Tier-1 is large** (N-room engine + unified defense + all blockers) before the first soak — but that's the point (don't soak incomplete). The integration gate + observability de-risk the first soak.
- **Unboosted ceiling** (cores/SK/L1-2) — L3+ MMO value waits for P5 boosts; stated, not a surprise.
- **Live CPU** of closed-form auction at N rooms × N squads — the per-tick bench is the gate; sim stays offline.
- **Defense cutover** could regress working defense — mitigated by keeping static escalation as bootstrap until the shared core passes the defense sim bed.
- **CombatView input-parity** — "prove in sim then works live" rests on the bot feeding the decision code the same inputs the sim does; the trace tool asserts input-parity, and the live adapter prefers fresh reads when the room is visible.

## Cross-references
ADR 0001 (SquadId — D1 → Entity validate-on-access), 0008 (combat arch / objective queue / anchor movement), 0019 (position selection), 0020 (force-sizing design; §10–§12 sequencing superseded), 0021 (visibility — scheduler in P-OBJ), 0006 (eval harness — extended to N-room in P-ENGINE). Living tracker: `docs/execution/phase-2.md`.
