# ADR 0014 — Empire Strategy & Posture (the executive layer)

- **Status:** Proposed
- **Date:** 2026-06-09
- **Deciders:** William Archbell
- **Related:** The completeness critic's #1 Critical gap: **"dominance has no decision-maker"** — postures are invented piecemeal across the design pass with no owner; and critic gap-9 (**no CPU capacity model** — nothing answers "can we afford room N+1?"). Gap analysis: **G-7b** (upgrade-vs-expand energy routing), **G-12** (Recovery posture), **G-14** (multi-front limits), **G-17** (per-source franchise P&L — this ADR's CPU twin), **G-1** (intel — *consumed*, not designed, here), G-15 (pixels), G-18 (war decision loop — the capstone consumer). Findings: IBEX-026/IBEX-028 (economy-abort / excess-attack trim — mechanism in [0008](0008-combat-and-squad-architecture.md), *policy* here), IBEX-032 (route-blind expansion — mechanism in [0009](0009-room-planning-and-multiroom-layout.md)), IBEX-021 (war cadences — [0004](0004-cpu-governance-and-load-shedding.md)). Sibling ADRs this layer arbitrates between (cross-referenced, never re-decided): [0004](0004-cpu-governance-and-load-shedding.md) (the per-tick CPU authority — the §4 boundary), [0006](0006-eval-and-iteration-harness.md) (seg-57 signals + fixtures), [0008](0008-combat-and-squad-architecture.md) (war ops *execute* what this ADR declares), [0009](0009-room-planning-and-multiroom-layout.md) (expansion gating), [0010](0010-boost-lab-factory-pipeline.md) (stockpile tiers), [0011](0011-spawn-orchestration.md) (demand classes / incubation), [0012](0012-market-and-risk.md) (embargo + §8 war chest), **0013** (power processing & GPL/power-creep economy — companion ADR being written in parallel in this pass; cross-referenced by number). Engine ground truth: [`../references/engine-mechanics.md`](../references/engine-mechanics.md), spot-cites below.

> **Scope boundary (read first).** This ADR owns **strategic intent**: the empire posture state machine, the WAR/PEACE declaration, and the arbitration of *marginal* surplus (energy, credits, CPU) across competing long-horizon consumers. It does **not** own: per-tick CPU shedding ([0004](0004-cpu-governance-and-load-shedding.md)), spawn scheduling ([0011](0011-spawn-orchestration.md) — the allocator gates *which discretionary demands exist*, never reorders 0011's bands), combat mechanism ([0008](0008-combat-and-squad-architecture.md)), trading mechanics ([0012](0012-market-and-risk.md)), or intel collection (G-1 — this ADR defines only the **read interface** it needs). Posture is a small piece of *data* that other systems read; this ADR is the only writer.

## Context

### Today, "strategy" is whatever each operation's thresholds happen to do

There is no executive layer. Four singleton operations (created unconditionally by `OperationManagerSystem`, `operations/managersystem.rs:34-62`) each gate themselves on ad-hoc reads of the same `EconomySnapshot`:

- **War declares itself.** `WarOperation::run_offense_evaluation` (`operations/war.rs:546-930`) scores rooms per-tick and **auto-launches** `AttackOperation`s: invader cores by a stored-energy affordability ladder (30k/100k/200k → max core level 1/3/5, `war.rs:722-744`), power banks above 100k stored (`war.rs:762-796` — with the IBEX-043 bug that the cap counts ALL attacks, `:766-776`), and — the part that is actually *war* — **hostile player rooms whenever `total_stored_energy > 150_000 && min_distance ≤ 6`** (`war.rs:838-860`). The only empire-level throttle is `max_concurrent_attacks = (room_count − 1).max(1) + economy_multiplier` recomputed from stored energy (`war.rs:937-949`). Nothing decides *which player* to fight, *why*, or *when to stop*: `WarOperation` returns `Ok(OperationResult::Running)` forever (`war.rs:1442`). All three cadence constants are 1 (`war.rs:139-141`, IBEX-021).
- **Expansion declares itself.** `ClaimOperation` runs a Discover→Scout→Select pipeline (`operations/claim.rs:893-917`) capped by `maximum_rooms = (cpu_limit / ESTIMATED_ROOM_CPU_COST).min(gcl)` with `ESTIMATED_ROOM_CPU_COST` a **hard-coded 10** (`claim.rs:876-879`) — a guess where a measurement should be (critic gap-9). `ColonyOperation` independently re-claims rooms with orphaned spawns every 50 ticks (`operations/colony.rs:50-169,193`). Neither consults war state, threat trends, or actual CPU attribution.
- **Everything else self-gates on the same two numbers.** `EconomySnapshot.total_stored_energy` and a per-room 20%-clamped-5k–30k reserve (`military/economy.rs:82-101`) are the entire "treasury": war launch, core ladder, power-bank gating, and military affordability all read them with independent thresholds.

Meanwhile the design pass has been *assuming* an executive layer into existence, piecemeal: [0010](0010-boost-lab-factory-pipeline.md) Tier C is "sized from `WarOperation` posture"; [0012](0012-market-and-risk.md) §5 embargoes war materiel "while 0008 reports an active conflict" and §8 sizes a war chest "from 0008's declared objectives"; the gap analysis invents per-room posture vocabulary ("war front, incubating, temple", G-6), a `Recovery` posture (G-12), an upgrade-vs-expand routing function (G-7b), and player-level front limits (G-14); [0011](0011-spawn-orchestration.md) D5 needs to know which rooms are fronts and which are incubating. **No document owns who sets posture, who declares war or peace, or who gets the marginal unit of surplus** when the boost planner (0010 Tier B/C), GCL upgrading (G-7a), expansion/incubation, the treasury (0012 §8), power processing (0013), and pixels (G-15) all want it. That is this ADR.

### Hard constraints

Tick-snapshot concurrency (`engine/main.js:32-66`; engine-mechanics §1): strategy is **predictive** — a posture that reacts to a siege after it lands is a posture that pre-stocked nothing; this is why posture drives *stockpiles and pre-positioning*, while reaction stays with the always-on defense scan. Single-threaded WASM, CPU = execution + intents (~0.2/intent): the executive layer must be a coarse-cadence pure computation, near-zero intents. VM-reset resilience: posture must be recomputable from signals (and its small hysteresis state persisted per [0002](0002-serialization.md) discipline once it carries decisions). Finite lifetimes bound commitments: creeps 1500t; power creeps live 5000t but renew at any power spawn/bank for free (`power-creeps/renew.js:9-20` resets `ageTime` +5000; verified) and respawn after an 8-real-hour cooldown on death (`common/constants.js:810-813`) — a power-creep commitment (0013) is therefore *durable* in a way creep commitments are not. Incremental strangler-fig: a thin posture resource must be consumable by Inc 4–5 ADRs without waiting for the full allocator.

### The arithmetic that makes arbitration non-optional

The consumers of marginal surplus are not vaguely competing — the engine prices them precisely, and the prices are wildly different per posture:

- **GCL upgrading:** level n→n+1 costs `1,000,000 × n^2.4` (`engine/utils.js:661-663`); at RCL8 the cap is 15 e/t but GCL credits the *boosted* amount (`creeps/upgradeController.js:42-52,84-86`) — 15 e/t buys 30 GCL/t boosted (G-7a). Steady, compounding, never urgent.
- **Expansion:** a claimed room compounds ~20 e/t native forever (engine-mechanics §7.3) but costs a claim body (2.5× spawn-time per part, §3.1), incubation spawn-ticks (0011 D5.3), bootstrap energy — **and a permanent CPU mortgage** (prior-art band: 2–10 CPU/room incl. remotes, gap analysis §1). CPU is the only cost with no market: the denominator must be measured, not assumed (gap-9).
- **Military stockpiles:** one boosted quad member ≈ 3,150 full-cluster lab-ticks (0010 §"arithmetic") — war materiel can only be bought with *peacetime*, which is exactly what a posture machine knows and a per-tick threshold does not.
- **Power processing:** 50 energy/power (`processor/intents/power-spawns/process-power.js:16-37`), GPL n costs 1000·n² lifetime power (`engine/game/game.js:133-134`) — a pure surplus sink worth "the full output of 5 sources" sustained (G-2b), unlocking the ×5 spawn multiplier (`spawns/create-creep.js:51-54`, 0013/G-2c). High ROI *eventually*, zero ROI this kilotick.
- **Treasury:** credits buy reagents and crisis energy (0012 §6) and nothing else structural; surplus above 0012 §8's three earmarks should convert to assets.
- **Pixels:** 10,000 bucket CPU each (`common/constants.js:384`), MMO-only — by construction the *last* consumer of anything (G-15).

## Decision

Adopt a three-part executive layer, all pure functions over snapshots ([0006](0006-eval-and-iteration-harness.md) fixture-tested), evaluated at a **strategic cadence** (~100 ticks, governor-sheddable), emitting **data that other systems read**:

```
seg-57 signals + threatmap + EconomySnapshot + IntelView (G-1, read-only)
        │
        ▼
PostureEngine (1) — deterministic precedence rules + hysteresis → EmpirePosture
        │            + WAR/PEACE declaration (consumes IntelView; G-14 limits)
        ▼            operator override pins any of it
EmpireAllocator (2) — marginal-surplus arbitration per posture → EmpireBudgets
        │            (incl. the CPU capacity model: room/remote go/no-go)
        ▼
READERS (3): 0008 war ops · 0010 tiers · 0011 demand classes · 0012 embargo/war-chest
             0009 expansion gate · 0013 processing throttle · G-6 terminal targets
```

### 1. The `EmpirePosture` state machine

One empire-level posture (not per-room — per-room *roles* are §2.4), six states, flipped by a deterministic precedence-ordered rule list over measurable signals. All thresholds are config defaults, eval-tuned ([0006](0006-eval-and-iteration-harness.md)); signals come from the seg-57 block (0006 §(1): bucket/cpu, gcl/gpl, energy_throughput, threat_max, deaths, restart_counter, death-spiral signals) plus its gap-analysis §4 extensions (spawn_utilization, deterrence_events, intel_freshness), `ThreatLevel` (`military/threatmap.rs:38-54`: None < SourceKeeper < Invader < PlayerScout < PlayerRaid < PlayerSiege < NukeIncoming), and the IntelView (§1.2).

| Posture | Enter when (precedence order, first match wins) | Exit when |
|---|---|---|
| **Bootstrap** | owned_rooms == 0, or no room has storage (RCL < 4), or post-respawn/`restart_counter` reset | first room reaches RCL ≥ 4 with positive refill trend and stored energy > floor → Develop |
| **Recovery** (G-12) | room lost (owned_rooms decreased), or structure-loss value in window > threshold, or spawn destroyed in any room, or extinction-adjacent event fired (death-spiral alarm, deser reject-and-reset) | rebuilt: spawns restored, Tier A floor (0010) refilled, no room below its pre-event RCL energy floor, quiet window elapsed → previous non-War posture |
| **War** | **only by explicit declaration** (§1.2) — never by drift | peace conditions of the declaration met (§1.2), or Recovery preempts |
| **Fortify** | IntelView reports a *capable* hostile within striking range (profile threat score > threshold), or `ThreatLevel ≥ PlayerRaid` observed in owned/reserved rooms ≥ K times in window, or deterrence_events trending up | quiet window (no qualifying events for N ticks) → Develop; or a declaration → War |
| **Expand** | GCL headroom (`gcl.level > owned_rooms + claims_in_flight`) AND the §3 CPU capacity rule passes AND `threat_max < PlayerRaid` sustained AND a route-reachable candidate scores above threshold ([0009](0009-room-planning-and-multiroom-layout.md) Step 3 graph + `claim.rs` scoring) | claim completed and incubation self-sufficient (0011 D5.3), or any entry condition fails → Develop |
| **Develop** | default | — |

**Mechanics of flipping.** The PostureEngine is `fn next_posture(inputs: &PostureInputs, current: &PostureState) -> Posture` — pure, host-testable. Anti-flap: a **minimum dwell** per posture (default ~500 ticks, except entries *into* Recovery and War which are immediate) and entry/exit thresholds separated by hysteresis bands (e.g. Fortify enters at profile-score X, exits at X·0.6). `PostureState = { posture, entered_tick, override: Option<Posture>, war: Option<WarDecl> }` — a few bytes. **Operator override always**: a config/flag path (same shape as the G-11 whitelist, `features.rs`) pins posture or forces a declaration; manual `attack`/`defend` flags (`war.rs:599-617`) are reinterpreted as operator declarations, not a parallel channel. Every transition (and every override) is logged and emitted to seg-57 with its reason — posture changes must be auditable in the eval harness.

**Defense never waits on posture.** The reactive defense scan and 0008 `Defend` objectives run identically under every posture ([0004](0004-cpu-governance-and-load-shedding.md) pins them never-shed; 0010 §4 pins "boosts never gate defense"). Posture modulates what we **pre-stock, pre-position, and pre-spend** — the only levers tick-snapshot concurrency actually gives a strategist.

### 1.2 The WAR/PEACE decision

War is a **declaration with an exit condition**, not an emergent score:

```rust
WarDecl {
    enemy: PlayerName,                  // stable id (0001 discipline)
    goal: Deny { rooms } | Evict { rooms } | Deter,   // what "done" means
    fronts: Vec<RoomName>,              // bounded by §1.3 limits
    declared_tick: u32,
    budget: { energy: u32, boosts_by_tier: …, credits: u32, deadline: u32 },
    peace_when: …                       // goal met | budget exhausted | deadline | Recovery entered
}
```

- **Inputs — the minimal intel interface (G-1, consumed not designed).** This ADR needs exactly one read-only view and defines it so G-1 has a contract to fill: `IntelView: fn profile(player) -> Option<PlayerProfile>` where `PlayerProfile = { rooms_owned, rooms_reserved: Vec<RoomName>, observed_tower_max, observed_rampart_max, boost_tier_seen, attacks_against_us: u32, attacks_by_us: u32, est_spawn_capacity, last_seen_tick }` — pure aggregation over data Ibex already collects per-room (`DynamicVisibilityData`, threatmap), per gap-analysis G-1. Until G-1 lands (Inc 8), a degraded `IntelView` is synthesized from per-room threat data only (no per-player aggregation) — sufficient for v0 NPC/flag-driven behavior, insufficient for automated player wars, **which is fine: automated player-war declaration is gated on G-1 by design.**
- **Target selection** is expected-value, not opportunity-proximity: `score = expected_gain(goal) − expected_cost(force plan from 0008's plan_by_detected_threat sized against profile maxima, boosts at 0010 chain cost, spawn part-ticks at 0011 accounting) − risk(retaliation given profile)`. Declare only when score clears a margin AND the war chest (0012 §8 term 2) covers `budget.credits` AND Tier C stock (0010) covers the force plan ×1.5 waves.
- **G-14 front limits, now decidable:** at most **one concurrent war against a *capable* player** (profile: est_spawn_capacity or boost tier above threshold); NPC policing (§4) doesn't count; total simultaneous fronts ≤ `max_concurrent_attacks` (kept, recomputed as today at `war.rs:937-949` until 0008 Step 3 absorbs it); **finish-or-abandon before opening the next** — a new declaration is rejected while `war.is_some()` unless the operator overrides.
- **Peace is automatic:** when `peace_when` fires, the posture leaves War, the declaration is archived to seg-57 (with realized cost vs budget — the G-18 campaign planner's future training data), [0008](0008-combat-and-squad-architecture.md)'s supervisor withdraws the objectives (Step 3 mechanism: withdraw → manager force-retires squads), and [0012](0012-market-and-risk.md)'s embargo lifts. Entering Recovery *forces* peace evaluation — we do not fund offense from a sacked economy (the IBEX-026 economy-abort, given its policy home).

### 2. The `EmpireAllocator` — marginal-surplus arbitration

A pure function, run at the strategic cadence: `fn allocate(posture: &PostureState, ledgers: &Ledgers, asks: &Asks) -> EmpireBudgets`. Host-testable against fixtures (0006); deterministic (same snapshot ⇒ same budgets — replay-diffable per 0011's discipline).

**2.1 Floors come off the top — they are not consumers.** The allocator arbitrates only **marginal** surplus above the posture-independent floors: 0010 **Tier A** defense-boost floor; [0012](0012-market-and-risk.md) §8 **survival credit reserve**; the per-room energy reserve (today's `economy.rs:82-101` clamp, including IBEX-034's RCL-scaling fix where 0011 placed it); 0004's `MIN_PATHFIND_OPS`. No posture, including War, may dip below a floor — this is what bounds the blast radius of a wrong posture.

**2.2 Ledgers (the three currencies).** `energy`: per-room surplus above floors + refill trend (from `EconomySnapshot`, extended per 0011 D2); `credits`: balance above 0012 §8 earmarks (the allocator *sizes* earmark 2, the war chest, from `WarDecl.budget`; 0012 enforces them); `cpu`: the §3 capacity model (p95 used, bucket trend, per-room/per-remote attribution). Spawn part-ticks are deliberately **not** a ledger here — [0011](0011-spawn-orchestration.md) owns spawn scheduling; the allocator influences it only by enabling/disabling *discretionary demand classes* and setting their targets (boosted-upgrader counts, incubation on/off, claim concurrency), which 0011's bands then schedule.

**2.3 Priority order per posture (the table that did not exist).** Consumers: **GCL** = upgrading incl. G-7a boosted upgraders consuming 0010 Tier B; **EXP** = expansion claims + incubation (0009/0011 D5.3); **MIL** = 0010 Tier C offense stockpiles + war-chest credits (0012 §8.2); **PWR** = power processing throttle (0013; engine cost 50 e/power); **TRD** = 0012 growth-budget buy programs (L3 reagent imports); **PXL** = pixels (G-15, MMO-only). Ordered allocation: each consumer takes up to its cap, remainder flows down; ∅ = disabled this posture.

| Posture | Marginal energy | Marginal credits | Marginal CPU (bucket surplus) |
|---|---|---|---|
| **Bootstrap** | build/RCL growth only (not a listed consumer — missions take it) | crisis imports only (0012 §6.2) | none spare by definition |
| **Develop** | Tier B fill → GCL → PWR (above surplus threshold) → EXP prep (scout/plan) | TRD → war-chest trickle | planner/bench bursts → PXL |
| **Expand** | **EXP first claim** (incubation targets, G-6 push) → GCL throttled to donor-room downgrade-safe minimum → Tier B → PWR ∅ | TRD (reagents for incubant defense) → war chest | reserved for room N+1 (§3 rule is the gate) → ∅ PXL |
| **Fortify** | **MIL first** (Tier C to WarDecl-candidate depth, Tier A deepened per 0010's threat scaling) → GCL → PWR ∅ | **war chest to target ×1.5** → TRD defense imports | intel sweeps (G-5 ext) → ∅ PXL |
| **War** | **front provisioning** (G-6 per-room targets toward `fronts`, battery-compressed per 0010 L4) → MIL replacement → GCL minimal → EXP ∅, PWR ∅ | war-chest spend per `budget` (0012 war-urgency ceilings) → embargo active | war recompute headroom; everything else per 0004 shed order — **posture never overrides the governor** (§4) |
| **Recovery** | **rebuild** (spawn/extensions/towers first — construction + imports via G-6) → Tier A refill → all else ∅ | crisis imports (0012 §6.2 — what the reserve is for) | ∅ — minimum strategic spend |

Two structural rules: (a) **GCL never starves entirely** outside Recovery — controllers downgrade (`CONTROLLER_DOWNGRADE`, engine-mechanics §7.4), so every owned room keeps its downgrade-safe trickle as a floor, not an allocation; (b) **PWR is a pure-surplus consumer at every posture** (the 0013 throttle this table feeds: process only when the room's energy surplus and the empire posture both say so — closing G-2b's "runs whenever fed").

**2.4 Per-room roles (the G-6 vocabulary, owned here).** The allocator tags each owned room: `RoomRole = Core | Front (war-front) | Incubating | Temple | Donor`. Consumed by: G-6 terminal empire-balancing (per-room target levels — push energy/boosts toward Front/Incubating ahead of demand), 0011 D5.2/D5.3 (war-support spillover exports a Front room's economic demands; incubation demand affinity), 0010 (Tier A depth per role). `Temple` is reserved for G-7c (Inc 9) — defined now so the enum doesn't churn, unused until then.

### 3. The CPU capacity model (critic gap-9 — the denominator)

CPU is the only currency with neither market nor regen-schedule; today its only model is `ESTIMATED_ROOM_CPU_COST = 10` (`claim.rs:876-879`). Replace guess with measurement:

1. **Attribution (seg-57 extension, rides G-17's per-source P&L in Inc 7).** Attribute execution CPU at the mission/operation dispatch boundary (missions carry their room entity; jobs carry creep→mission) and intent CPU (~0.2 × count) at the single guarded intent sink ([0003](0003-behavior-modeling.md)) by emitting room. Pathfinding facade ops are charged to the requesting room by the facade ([0004](0004-cpu-governance-and-load-shedding.md)). Unattributable cost (serialization, global systems, WASM overhead) is tracked as `cpu_overhead` and amortized evenly. Output per cadence: `cpu_by_room` (rolling p50/p95), `cpu_by_remote` (paired with G-17's `net_e_t` — one ledger row per remote: net energy/t **and** CPU/t).
2. **Marginal room estimate:** `marginal_room_cpu = p75(cpu_by_room over mature rooms, incl. their remotes' share + amortized overhead delta)`. Sanity band: 2–10 CPU/room (prior-art benchmark, gap analysis §1); an estimate outside the band is itself a seg-57 alarm.
3. **Room N+1 go/no-go (the Expand gate):** claim iff `p95(cpu_used) + marginal_room_cpu ≤ cpu_limit × 0.85` AND bucket trend ≥ 0 over the window AND GCL headroom. This **replaces** the constant at `claim.rs:876` and is the route-aware claim pipeline's ([0009](0009-room-planning-and-multiroom-layout.md) Step 3) admission ticket.
4. **Remote go/no-go:** drop a remote when `net_e_t ≤ 0` (always — G-17), or when the governor has spent > X% of the window in Conserve/Critical *due to load* and the remote is in the bottom decile of `net_e_t / cpu_t` (shed lowest-margin franchises first; resume with hysteresis when headroom recovers). Consumed by `MiningOutpostOperation` (today it expands to radius 1 with no economics at all, `miningoutpost.rs:100`).

### 4. Layering — who reads posture, and the governor boundary

**Readers (each ADR's existing hook, now given one writer):**

| Reader | Reads | Hook already reserved there |
|---|---|---|
| [0008](0008-combat-and-squad-architecture.md) | `war: Option<WarDecl>` gates player-offense objective creation; peace withdraws them | Step 3 `WarOperation`-as-supervisor (withdraw → force-retire) |
| [0010](0010-boost-lab-factory-pipeline.md) | posture + WarDecl size Tier C; role deepens Tier A | "sized from `WarOperation` posture" (Tier C row) — the posture now exists |
| [0011](0011-spawn-orchestration.md) | demand-class enables/targets; `RoomRole` for D5.2 war-support + D5.3 incubation | D1 demand classes; D5 placement |
| [0012](0012-market-and-risk.md) | `war.is_some()` = embargo; `WarDecl.budget.credits` sizes §8.2 war chest | §5 embargo "while 0008 reports an active conflict"; §8 |
| [0009](0009-room-planning-and-multiroom-layout.md) | Expand posture + §3 capacity rule admit claims; multi-room planning bursts are Develop/Expand work | Step 3 expansion eligibility |
| **0013** | `EmpireBudgets.power_processing` throttle; GPL/ops spend priority per posture | processing-throttle policy (G-2b); ops economy (G-2c) |
| G-6 / G-18 | per-room target levels from `RoomRole`; archived WarDecls feed the campaign planner | gap-analysis rows |

**The governor boundary ([0004](0004-cpu-governance-and-load-shedding.md)) — strategic vs tactical, never fighting:**

1. **Different timescales, different objects.** The governor decides *what runs this tick* from bucket + trend (tactical, per-tick). Posture decides *what commitments exist at all* over hundreds of ticks (strategic). The governor sheds work; posture creates or retires it.
2. **Posture never overrides the governor.** War posture does not entitle war systems to run under Critical — 0004's shed order is absolute (defense/spawn/haul/serialize always-on; war recompute sheds first). A war during CPU starvation is fought with whatever the tiers allow; if starvation is *sustained*, that is a posture **input** (next rule), not a tier exception.
3. **The governor never writes posture, but feeds it.** Sustained Conserve/Critical residency and negative bucket trend enter `PostureInputs` — they fail the §3 capacity rule (stop expanding), bias toward Develop, and can trigger `peace_when: budget-exhausted` evaluation. One-tick spikes never flip posture (the cadence + dwell guarantee it).
4. **The PostureEngine/Allocator are themselves sheddable** (Conserve: skip re-evaluation, keep current `PostureState`; Critical: same). Posture is plain data; *reading* it costs nothing, so consumers never block on a shed evaluation. A stale-but-valid posture is the designed degraded mode.
5. **No shared knobs.** The governor's tier thresholds and the allocator's budgets are disjoint config; nothing is dually owned. (0012's `TradeGovernor` similarly stays independent — the embargo is an input to its planner, not a tier.)

### 5. `operations/war.rs` becomes the EXECUTOR, not the declarer

Reclassify its three current launch paths:

- **Policing (stays autonomous, all postures):** invader cores, invader creeps in our remotes, power banks, SK farming (G-8) — NPC "farming" objectives (0008's `Farm{powerbank|sk|core}`), launched under economy/loot scoring as today (`war.rs:719-835`, with IBEX-043 fixed per the Inc-1 backlog and G-3 loot economics added in Inc 7). These are economic activity, not war; they never touch the WarDecl or the embargo.
- **War (moves behind the declaration):** the hostile-player path (`war.rs:838-860`) stops auto-launching on `stored_energy > 150k && distance ≤ 6`. It becomes: *given* `WarDecl { enemy, fronts, goal }`, build campaigns/objectives against those fronts (force plans via the existing `plan_by_detected_threat`, `attack.rs:212` — unchanged), supervise them per 0008 Step 3 (age-abort, trim on cap shrink — IBEX-028), report realized spend against `budget` (IBEX-026's live producer), and tear down on peace.
- **Defense (unchanged, always-on):** the defense scan (`war.rs:170-542`) keeps its reactive autonomy at every posture (§1).

Manual `attack` flags remain honored — reinterpreted as an operator-declared `WarDecl { goal: Deny, fronts: [flag room] }`, so the override path and the automated path converge on one mechanism.

## Alternatives Considered

| Option | Pros | Cons |
|---|---|---|
| **Posture FSM + per-posture allocator + measured CPU capacity model (chosen)** | One decision-maker; deterministic + auditable + operator-overridable; pure/host-testable (0006); gives 0010/0011/0012/0013's reserved hooks their writer; war gets a declarer AND an exit; expansion gets a real denominator | New tunables (thresholds, dwell, priority tables); a wrong posture is a *global* error (bounded by §2.1 floors + always-on defense) |
| **Status quo: each operation self-gates on `total_stored_energy`** | No new system | The critic's finding verbatim: postures multiply piecemeal; war has no exit (`war.rs:1442`); thresholds drift apart; 0012 §8 / 0010 Tier C reference a posture that doesn't exist |
| **Utility-AI / GOAP planner over empire goals** | Expressive; handles novel situations | Non-deterministic emergent priorities are hostile to replay-diff validation (same verdict as 0011's auction rejection); un-auditable transitions; over-engineered for six states |
| **Internal auction (subsystems bid for surplus)** | Elegant decentralized arbitration | Rejected in 0011 for spawn-time for the same reasons: emergent, non-replayable, tuning-opaque; priority tables are sufficient and testable |
| **Per-room postures only (no empire posture)** | Matches G-6's room vocabulary directly | War, treasury, GCL routing, and front limits are empire-scoped by nature; per-room-only re-smears the arbitration. Resolution: both, layered — empire posture (§1) + room roles (§2.4) |
| **Posture decided by the CPU governor (extend 0004 upward)** | One authority | Conflates timescales: tiers are per-tick reactions, posture is multi-kilotick commitment; would make bucket noise flip strategy. The §4 boundary exists precisely to prevent this |
| **Full diplomacy/reputation engine deciding war** | Prior art exists (TooAngel) | G-11's NOT-WORTH-IT verdict stands; the whitelist + IntelView profile is the survival subset; revisit only in alliance metas |

## Consequences

**Positive**
- **Dominance has a decision-maker.** Every "who decides X" left dangling by 0008–0013 resolves to one place: posture (this ADR §1), war/peace (§1.2), marginal surplus (§2), room count (§3). The critic's #1 gap and gap-9 close together because expansion *is* an allocation decision.
- **War becomes bounded and auditable:** declared with a goal, budget, and exit; executed by 0008's supervisor; embargoed by 0012; provisioned by G-6 — and archived with realized-vs-budgeted cost, the dataset G-18's campaign planner will need.
- **The reserved hooks get their writer** — 0010 Tier C sizing, 0012 §5/§8, 0011 D5, 0013's throttle were all written against "posture" on faith; this ADR pays that debt without re-deciding any of them.
- **Expansion stops being faith-based:** measured `marginal_room_cpu` + bucket trend replaces a hard-coded 10; remotes become droppable franchises (with G-17) instead of permanent fixtures.
- **Pure-function core:** posture transitions, the allocator, and the capacity rule are kernel-testable on fixtures; posture flips are reproducible from recorded seg-57 streams — strategy regressions become diffable like any other (0006).

**Negative / new risks**
- **A wrong posture is a correlated, empire-wide error** (vs today's uncorrelated local errors). Bounded by: floors off the top (§2.1), defense always-on, the governor unaffected (§4.2), dwell/hysteresis against flapping, operator override, and seg-57 transition auditing. The harness scenarios below are the regression net.
- **Tunables surface** (thresholds, dwell, priority tables, capacity factors). All config, all eval-diffable; the priority *tables* are data, so tuning never touches code.
- **Attribution error in the CPU model** (mis-attributed overhead skews `marginal_room_cpu`). Mitigated by the sanity band (2–10 CPU/room) alarm and by gating only *growth* on it — a wrong estimate delays a claim, never kills a room.
- **Degraded IntelView before Inc 8** means automated player-war stays off until G-1 lands — accepted and explicit (today's auto-attack path is the thing being *removed*; flags cover the interim).

**CPU / tick-safety.** The executive layer is O(rooms + consumers) scalar arithmetic once per ~100 ticks, zero pathfinding (route reads via 0004's facade where needed), zero intents of its own. Reads of `PostureState`/`EmpireBudgets` are field accesses. Sheddable per §4.4; stale posture is the designed degraded mode. VM reset: v0 recomputes everything from signals (deterministic; dwell timers restart — acceptable for a strategic cadence); the Inc-8 persisted block (`{posture, entered_tick, override, war}` + per-remote ledger) is additive `serde(default)` per [0002](0002-serialization.md), failing toward `Develop` + no-war on decode failure (the safe direction — no surprise offense after a reset).

## Incremental Migration Path

Stable seams: the **`EmpirePosture`/`PostureState` resource** (read-only to everyone but the PostureEngine), the **`EmpireBudgets` output struct**, and **seg-57** for signals/audit. Placement per the rewrite plan (Increments 0–7; 8/9 proposed by the gap analysis). Never break the running bot mid-increment — every step leaves the legacy gates in place until its reader migrates.

1. **Step 0 — Kernels & fixtures (anytime after Inc 0; Breaking: None).** `PostureInputs`/`next_posture`, the allocator skeleton with the §2.3 tables as data, and the §3 go/no-go arithmetic as pure kernels with fixture tests (0006): sacked-room fixture → Recovery; sustained-Conserve fixture → Expand entry refused; declaration fixture respects G-14 limits; dwell prevents flap on an oscillating-threat fixture. Nothing wired.
2. **Step 1 — Thin read-only posture resource (Inc 4–5; Breaking: None).** Insert `PostureState` as an ephemeral ECS resource recomputed each strategic cadence from *existing* signals (`EconomySnapshot`, threatmap, claim/war operation state — no intel, no persistence, no consumers changed). Emit posture + transition reasons to seg-57; render in the summary viz. This is the consumable 0008 (Step 2–3), 0010 (L1 Tier A scaling), and 0011 (D10 class weights) need on their own Inc-4/5 schedules — they read it when they land; the resource existing changes nothing by itself. **Validate:** harness smoke-run shows sane postures across the economy-bringup and siege scenarios; zero behavior diffs (read-only).
3. **Step 2 — war.rs split: executor vs declarer (Inc 5, co-staged with 0008 Step 3; Breaking: Behavioral).** Player-offense launches (`war.rs:838-860`) move behind `war: Option<WarDecl>`; v0 declarations come only from operator flags + a conservative deterministic recommender (NPC policing and defense unchanged). `WarOperation` gains the supervisor duties (0008 Step 3) and reports spend against `budget` (IBEX-026 live). **Validate:** harness — no player-room attack launches without a declaration; flag-declared war launches, peace condition tears it down within the deadline; policing (cores/banks) unchanged vs baseline.
4. **Step 3 — CPU capacity model + remote ledger (Inc 7, rides G-17's seg-57 P&L work; Breaking: Behavioral + seg-57 additive (Memory/format, ver-bumped per 0006's header discipline)).** Per-room/per-remote attribution; replace `claim.rs:876`'s constant with the §3.3 rule; wire the remote drop/resume rule into `MiningOutpostOperation`; 0009 Step 3's route-aware eligibility consumes the same gate. **Validate:** induced CPU pressure → claims refused while policing/economy continue; a manufactured negative-margin remote is dropped and resumes on recovery; `marginal_room_cpu` lands inside the sanity band on the multi-room scenario.
5. **Step 4 — Full allocator + intel-consuming WAR/PEACE + persistence (Inc 8; Breaking: Memory/format — one small additive `serde(default)` block for `PostureState` + the WarDecl archive; no reset, doesn't consume either sanctioned reset window).** `EmpireBudgets` wired to its readers as each lands in Inc 8: 0010 L2 tier sizing, 0011 Step 5 placement/incubation targets, 0012 M2/M3 embargo + war-chest, 0013 processing throttle, G-6 per-room terminal targets via `RoomRole`; `IntelView` backed by G-1's PlayerProfile store; G-14 player-level limits enforced; G-7b's route-energy function is the Develop/Expand rows of §2.3 (subsumed, not separate). **Validate:** declared-war scenario — embargo flips, war chest fills before growth resumes (0012 §8 shape), fronts provision ahead of demand; Recovery scenario (G-12's harness gate) — rebuild outranks all, exports halt; peace archives the WarDecl with realized cost.
6. **Step 5 — Terminal sinks & capstone consumers (Inc 9; Breaking: Behavioral, flag-gated).** Pixels as the allocator's last-resort bucket sink (G-15: governor-Normal + bucket pegged + no pending bursts → `generate_pixel`; MMO-only, untestable-by-harness — flag-gated, mark it so); `Temple` role activates with G-7c; archived WarDecls feed the G-18 campaign planner (its own design, not this ADR's).

**Breaking-change summary:** Steps 0–1 — **None**. Step 2 — **Behavioral** (which attacks launch changes; that is the point). Step 3 — **Behavioral** + seg-57 additive **Memory/format** (versioned-segment field additions, labelled per 0006). Step 4 — **Memory/format** (one additive labelled block, `serde(default)`, fails toward Develop/no-war; no state drop). Step 5 — **Behavioral**, flag-gated. No step touches `serialize_world`'s frozen seam, and the legacy threshold gates each step replaces stay live until their replacement is harness-validated.
