# Recon findings for ADR 0040 (stress-time energy prioritization + offline economy simulator)

## 1. ADR template conventions (exact)

**Template file:** `C:\code\screeps-ibex\docs\design\adr-template.md`

Title line: `# ADR NNNN — <Title>`

Frontmatter = a bullet list (not YAML):
- `- **Status:** Proposed | Accepted | Superseded by NNNN`
- `- **Date:** <YYYY-MM-DD>`
- `- **Deciders:** <…>`
- `- **Related:** <review finding IDs, Field Reports, other ADRs>`

Template section headings, in order:
1. `## Context` — "forces and constraints: CPU budget (execution + intents), single-threaded WASM, VM-reset resilience, incremental rewrite. The pain being solved and why now."
2. `## Decision`
3. `## Alternatives Considered` — a `| Option | Pros | Cons |` table
4. `## Consequences` — "Positive / negative / new risks; … CPU and tick-safety impact"
5. `## Incremental Migration Path` — "What to replace first; the stable seam to hide it behind; how to validate behavior before/after each step (tests / replay / parallel-run); breaking-change & state-drop notes."

**README conventions** (`docs/design/README.md`):
- New ADRs: copy `adr-template.md`, number sequentially `NNNN-title.md` (line 5).
- Status lifecycle: `Proposed → Accepted → Superseded by NNNN` (line 6).
- Register a row in the README table (`| ADR | Pillar | Drives | Status |`) with a one-line pillar description, what drives it, and status (lines 9–51). ADR 0040 must add its row.
- Standing constraints every ADR must respect (line 54): "single-threaded WASM (no parallelism), per-tick CPU budget **including intents**, VM-reset resilience, and **incremental** migration (a stable seam, verifiable per step)."
- Engine ground truth: `docs/references/engine-mechanics.md` — "check it before guessing mechanics" (line 52).

**Recent-ADR practice on top of the template:**
- The **Status bullet accretes rich history**: bold verdicts, milestone landing dates, gates met, "operator-veto-pending" / "pending operator review + commit + deploy" (0033:3, 0038:3–9). Per EP-10.7, ADRs stay Proposed until the operator accepts — completion is never self-declared.
- 0038 adds a `- **One line:**` bullet (0038:10–18) summarizing the defect+fix in one paragraph; it drops the Date/Deciders/Related bullets (a deviation from the template) and puts cross-ADR lineage in prose ("Extends ADR 0032 …", 0038:20–27).
- 0033 keeps all four template bullets and adds a bolded **"Division of ownership"** paragraph directly under the frontmatter (0033:8) stating what this ADR owns vs sibling ADRs — worth mirroring for 0040 since an economy sim overlaps ADR 0006/0028/0033 territory.
- Both recent ADRs end with a `## Cross-references` section carrying file:line pointers (0033:489–495, 0038:332–344).

## 2. ADR 0033's structural pattern (the sim+benchmark ADR to mirror)

Full heading skeleton (line numbers in 0033):
- Frontmatter + Division-of-ownership (1–8)
- `## Context` (12) → `### §0 The pain, and why now` (14 — operator asks quoted verbatim and made measurable); `### §1 The constraint litany` (24); `### §2 The one architectural fact that frames the whole design` (31); `### §3 What is in scope vs out of scope` (44 — explicit IN/OUT bullet lists, OUT items each name their owning ADR)
- `## Decision` (66) — one bold headline paragraph, then a **decision-provenance note** (70: an independent analysis recommended against; "The operator chose the extraction anyway" with rationale recorded), then §D-numbered subsections:
  - §D1 new crates + extraction (74), with **AS BUILT** reconciliation notes where implementation diverged from the draft (78–131)
  - §D2 **anti-duplication map** (218): every component gets exactly one disposition — REUSE-AS-IS / REUSE-AFTER-MOVE / SHARED-FIX / NET-NEW — in a table
  - §D3 reused server half + shared fixes (254); §D4 ground-truth oracle (291)
  - §D5 **the metric set (formulas + L-layer mapping)** (307): every metric gets (a) a formula, (b) a bold **Gate:** with a threshold, (c) an L0–L6 taxonomy tag, e.g. `R_fatigue = cost_fatigue(P)/cost_fatigue(O*)`, "Gate: ≤ 1.0 + ε, ε≈0.02. [L2/L0]" (315)
  - **§D5.4 the objective function** (347–359) — the section the prompt asks about, detailed below
  - §D6 scenario catalog (361): lettered families A–I, each "what it stresses → gate"
  - §D7 determinism fence as first-class ship-blocker (375): `det_repeat`/`det_reorder`/`det_hash_seed`, spread == 0
  - §D8 mapping every gate into ADR 0015's L0–L6 taxonomy table (385) so the ADR inherits policy instead of re-litigating it
- `## Alternatives Considered` (403): CHOSEN option is the first table row, marked **(CHOSEN)**; a rejected option records "Operator chose X over Y" as provenance (408)
- `## Consequences` (418) with three subheads: `### Positive`, `### Negative / costs`, `### CPU & tick-safety impact` (the last explicitly states WFV/reset impact: "no WORLD_FORMAT_VERSION bump, no reset", 438)
- `## Incremental Migration Path` (443): **named milestones M0..M5**, "Named, independently-testable slices, each with a gate (the 0023 S1–S5 / 0028 K0–K4 / 0006 Inc A–E idiom)"; each slice carries a bold **Gate:**; slices are updated in place to `✅ LANDED <date>` with gate-met evidence and as-built departures; closing line: "Each slice is shippable on master independently, gated by its own `#[test]`s in the host test pass, and adds zero live CPU" (485)
- `## End state (as shipped, <date>)` (461): shipped defaults by layer, final regression-tracked baselines (the journey "0.680 → … → 0.963"), and a **Remaining** list where every item is "chip-tracked or gated" (481)
- `## Cross-references` (489)

**§D5.4 objective-function design pattern (the part to mirror for an economy objective):**
- Physical-unit metrics (ticks/fatigue/ops) sit *underneath* one value-weighted objective. The scalar: `w(creep) = quantize(G · r_bid)` in **energy-equivalent per tick (e/t)** — "the marginal energy-equivalent value destroyed if this creep's one movement intent is denied this tick" (351).
- Key argument reusable verbatim for 0040: "e/t is the codebase's ONE existing cross-goal currency (stocks: `objective_value.rs::value_e` ADR 0032, `room_net_roi`/`claim_value` ADR 0038; rates: the ubiquitous `body_cost/CREEP_LIFE_TIME` amortization; CPU already priced at 0.02 e/tile) — no new unit invented" (351).
- Multiplicative gates `G = Δ_crit · A · S · U`, each ∈ [0,1] (progress-criticality, arrive-in-time ramp, survival veto, slack decay) (353).
- A **role table** mapping each role to an e/t bid, with the anchor role first (hauler `min(Q/T*_rtt, marginal_route_share)`), and honest policy constants declared as such ("scout `max(ε_intel, upkeep)` — honestly a policy constant … declared, not buried") (355).
- **Aggregate:** per-episode efficiency `η = T*/T_realized ∈ [0,1]`, sample weight `W = r_bid · T*`, headline `H = Σ Wᵢηᵢ / Σ Wᵢ` reported as weighted mean + seeded-bootstrap 95% CI + p05–p95 percentiles; "`1 − H` ≈ the fraction of energy-equivalent value destroyed by movement delay"; **H is regression-tracked, NEVER a ==1 gate** — hard physical gates + a zero-tolerance sentinel remain the pass/fail layer underneath (357).
- A **reduction proof**: the operator's original ask (hauler max-value-transport) must fall out of the general form by construction "with zero residue" (357).
- **Numbered open-decision list** (1)–(11), each later marked DECIDED-with-rationale in the code (`value.rs`'s `## §D5.4 decisions` block), "operator-veto-pending — each is one constant/match-arm to flip"; the original list is kept for the record (359). This is the decision-list-with-operator-sign-off convention.
- Design provenance recorded: "Designed via a grounded multi-agent synthesis (5 codebase-grounding readers … 4 independent formulations, adversarial judging); full transcript retained" (349).

**0038's complementary conventions** (leaner, for a redesign-of-existing-behavior ADR): `## 0. Problem statement` with a live log excerpt (0038:30–47); `## 1. Root-cause map` as a `| # | Mechanism | file:line |` table with S1..S5 rows, "verified against the tree <date>" (61–73); `## 2. Decisions` split Part A/Part B with D1–D8 bullets, defects caught in pressure-testing promoted to decisions (`C-DEFECT-1`, 138–147); `## 4. Migration ledger` with explicit **DELETE / KEEP / ADD** lists at file:line granularity (224–259); `## 5. Sim-first test plan` — numbered named kernel tests, "prove RED→GREEN offline" (263–295); `## 6. WORLD_FORMAT_VERSION` section stating bump-or-no-bump per artifact with reasons (299–308); `## 7. Interactions` per neighboring ADR (312–328).

## 3. `room_net_roi` — definition and reusability for economy metrics

**Kernel:** `screeps-ibex/src/room_economics.rs` — a "functionally PURE kernel — NO `game::*` / world reads", bit-deterministic scalar f64, no HashMap, callers quantize before any discrete decision (module docs, lines 10–26). Built by ADR 0032, consumed by war.rs and (per ADR 0038) claim selection.

**Signature:** `pub fn room_net_roi(facts: &RoomEconomyFacts) -> RoomEconomyValue` (room_economics.rs:151 per 0038:337).

**Inputs — `RoomEconomyFacts`** (room_economics.rs:88–107):
- `source_count: u32`
- `source_capacity: f64` (RESERVED 3000 / NEUTRAL 1500 per regen cycle)
- `haul_tiles: u32` — one-way path tiles to the hauling home ("the dominant distance term")
- `hold_model: HoldModel` — `Reserve` (reserver CLAIM+MOVE upkeep) | `Suppress` (reads `hold_body_cost`) | `None`
- `hold_body_cost: u32` (Suppress only)
- `horizon: f64` (≤0 ⇒ `DEFAULT_HOLD_HORIZON` = `CREEP_LIFETIME` = 1500)

**Formula** (doc comment above the fn, room_economics.rs ~166–178):
```
net e/t = gross − hold − mining − haul − cpu
net_roi = max(net, 0) × horizon
```
- gross = `source_count × source_capacity / SOURCE_REGEN_TICKS(300)`
- hold = `(CLAIM 600 + MOVE 50)/1500` for Reserve; `hold_body_cost/1500` for Suppress; 0 for None
- mining = WORK to saturate gross (`gross / HARVEST_POWER(2)`) + 2 MOVE/source, amortized over `CREEP_LIFETIME(1500)`
- haul = CARRY (+ matched MOVE) to move gross over a `2 × haul_tiles` round trip, amortized
- cpu = `haul_tiles × CPU_PENALTY_PER_TILE(0.02 e/t per tile)`

**Output — `RoomEconomyValue`**: `gross_per_tick`, `net_per_tick` (floored at 0), `net_roi = net_per_tick × horizon`.

**0038's composition on top** (0038:119–207): `claim_value(R,d) = intrinsic_roi(R) · unlock_fraction(d) · support_decay(d) · plan_quality(R)`, with the `owned_colony(source_count, internal_haul_tiles≈25)` ctor (`HoldModel::None`) and the **C-DEFECT-1 lesson**: distance must enter the composite exactly once — passing claim distance as `haul_tiles` drives net to exactly 0 at d≥4 (1-source)/d≥6 (2-source) and re-creates the stall (0038:138–147; ctor doc, room_economics.rs:120–133).

**Can bootstrap-recovery and RCL-rush metrics share this accounting? Yes, deliberately.** The corpus already treats e/t as the single cross-goal currency (0033:351), and the kernel's building blocks are exactly what an economy objective needs: gross-income rate, body-cost amortization over `CREEP_LIFE_TIME`, round-trip haul sizing (`2 × haul_tiles`), CPU priced at 0.02 e/tile, and horizon projection to an energy-equivalent stock. The precedent for valuing *upgrade/build* output in e/t also exists: 0033's role table prices worker/upgrader as `min(WORK·k, supply_rate)·v_sink` with `k = 5 build / 1 upgrade e/t/WORK` (0033:355) — i.e., RCL-rush value would enter through a `v_sink`-style declared policy constant (energy→controller-progress worth), not a new unit. Two caveats for 0040: (a) `room_net_roi` **floors net at 0** — a bootstrap/stress metric measuring deficits needs a signed variant or its own composition kernel (the sanctioned pattern is a *new sibling pure kernel* that calls `room_net_roi`, like `claim_economics.rs`, never editing `room_net_roi`'s body — 0038:314–319 says body changes would move war.rs's numbers); (b) `plan_quality`/`unlock`/`support_decay` are claim-specific — 0040 composes its own multipliers adapter-side.

## 4. Relevant EP-* rules (`docs/guides/engineering-practices.md`)

**New-systems design:**
- EP-2.1 (line 38) — "One change, one named frozen seam; callers never move (strangler-fig)." Each increment independently verifiable and reversible.
- EP-2.2 (39) — "New plumbing lands at behavior parity first; behavior changes turn on only after parity is verified."
- EP-2.3 (40) — "Seams grow consumer-by-consumer; no speculative scaffolding."
- EP-2.5 (42) — "Mechanism/policy split, genericity verified not asserted." (Policy lives in the eval/policy crate.)
- EP-2.6 (43) — "One implementation per concern, not N." Supplanted code is deleted, not flagged off.
- EP-2.7 (44) — "One writer per shared datum; every long-running activity has exactly one lifecycle owner and a definite terminator." Liveness conditions never relax themselves under failure.
- EP-6.14 (94) — "One artifact per gate, one owner": new ADRs "must reference an existing harness artifact before minting a new one." (Directly binding on an economy simulator — it must reuse the 0033 `sim-core`/eval substrate, which 0033:427 explicitly reserves "for future non-combat sims (economy, hauling, lifecycle)".)

**Priority/utility code:**
- EP-6.13 (93) — "Determinism is a prerequisite of decision code: no HashMap/HashSet iteration order may reach a decision or an emitted ordering — sort or use BTreeMap at the boundary."
- EP-6.2 (82) — "Build decision logic pure-by-design: DTO inputs, no JS-bound game-API types below a seam line."
- EP-6.1 (81) — "Kernel-vs-shell: extracted pure decision logic ships kernel/fixture tests in the same commit" (unit coverage plus ≥1 property/relation test per stable kernel); strategy tweaks ship no new tests.
- EP-4.6 (67) — "Tuning numbers are measured, not asserted"; thresholds are named constants so calibration lands as a reviewed diff.
- EP-7.2 (99) — "Do the arithmetic before deciding; alternatives get rejected by math, not taste."
- EP-3.8 (58) — "Transient faults park work (Wait), they don't tear it down." (Relevant to stress-time reprioritization semantics.)
- Also the memory rule (not an EP): prefer per-tick optimal + deterministic tie-break over hysteresis/latching unless oscillation is actually observed.

**Stress-time behavior / budgets (the (a) half of ADR 0040):**
- EP-4.1 (62) — "Every unbounded or expensive operation draws from a budget, and budgets charge a shared pool" (per-call-site caps alone are the aggregate leak class).
- EP-4.2 (63) — "Every budget defines its degradation where the grant is issued."
- EP-4.3 (64) — "The never-shed set is tiny and survival-gated (defense, spawn, haul, movement, serialize_world); admission bar is survival-criticality, not importance. Telemetry never sheds."
- EP-4.4 (65) — "Shed re-decision, never committed work"; tier decisions use trend, not just level.
- EP-4.7 (68) — "For emergent, non-reproducible failure classes, don't chase a repro — make the defensive property first-class and validate with synthetic pressure."

**Serialization changes:**
- EP-5.1 (73) — "Rewrite-period policy: reset-anytime … build NO migration paths … Loudness is the entire requirement."
- EP-5.2 (74) — "Never break the currently-running bot mid-increment; every change carries its breaking-change label (None / Behavioral / Memory-format)."
- EP-5.3 (75) — "When a resumable process's semantics change, force invalidation of its in-flight persisted state via a fingerprint/version bump." (Cf. the memory note: `WORLD_FORMAT_VERSION` in game_loop.rs MUST bump on any serialized-shape change; currently 23 pending deploy per ADR 0038.)
- EP-5.4 (76) — "Don't persist what is derivable or re-assertable … unless recomputing it on the post-reset tick is itself the failure mode (EP-4.8)."
- EP-5.5 (77) — "Telemetry/metric schemas are versioned from day one with additive-evolution rules pinned by tests."

**Sim/bench code:**
- EP-6.7 (87) — "Gates: hard zeros for single runs … single-run metric thresholds are flake generators. Statistical gates are N-seeded paired diffs vs a stored (scenario, seed, SHA) baseline, never absolute thresholds"; gate numbers live in one reviewed config; CI retries forbidden.
- EP-6.8 (88) — "Comparisons use plan-loss accounting: entities that drop out of the result set are regressions"; recompute quality from ground truth, never self-reported scores.
- EP-6.9 (89) — Run hygiene: never edit bot sources while a smoke runs; runs land under `runs/` keyed (scenario, git SHA).
- EP-6.11 (91) — "Optimizations are validated against the constrained/worst case, not the nominal case."
- EP-6.12 (92) — "The harness never lies: infra failures are never reported as bot failures; the harness has its own known-good self-test."
- EP-6.4 (84) — Behavior-preserving conversions prove it (parity test / intent digest / byte-compare); "Determinism fixes land before any parity recording."
- EP-9.2 (116) — "Prefer Rust over Node/JS; programmatic over shell, for anything durable."

**Feature flags / scaffolding:**
- EP-2.10 (47) — "Interim scaffolding is labelled and must not accrete callers."
- EP-10.5 (129) — "transitional flags get a removal point tied to validation"; unused code is removed, not kept.
- EP-2.6 (43) — dead-but-planned code sits on an explicit KEEP list with an ADR owner; supplanted code deleted, not flagged off.

**Process (bearing on the ADR itself):** EP-10.7 (131) — "Operator owns acceptance. ADRs stay Proposed until accepted"; EP-10.8 (132) — artifacts live under `docs/` by type; EP-10.3 (127) — no AI attribution in commits.

## 5. todo.md items relevant to economy / bootstrap / repair / refill / RCL rush (verbatim)

High priority:
- (line 3) "Fix partial hauls due to damage causing return to harvesting source. (Stay stick with delivery once started.)"
- (line 4) "Remote upgrade mission/role."
- (line 9) "Add remote mining container building."
- (line 11) "Computer number of hauler/harvester parts needed based on path distance."
- (lines 14–15) "Add CPU analysis." / "1. Prevent additional remote mining, reserving or claiming of new rooms without sufficient CPU."
- (lines 16–17) "Post-process for room planner. (Remove roads not needed, fix RCL for links etc. based on distance, prioritize storage.)" / "1. Apply RCL as post-process with constraints. (i.e. do extensions by distance, don't spawn extractor container till RCL 6 etc.)"
- (line 18) "Spawn body calculation using current available energy needs to use at least min body cost, otherwise never ends up in queue. (Will not block lower priority spawns!)" — the closest existing item to stress-time energy prioritization (a bootstrap/starvation spawn defect).
- (line 22) "Shared, predicted storage capacity for the tick. (Allow for haulers to not wait for a tick at the end of their delivery tickets.)"

Medium priority:
- (line 27) "Add per-room stats (i.e. energy available over X minutes) to use for predicting needed roles." — directly a proto-economy-metric item.

Low priority:
- (line 33) "Use generator for spawn queue to compute only on-demand."

Trailing notes:
- (line 63) "Container miners need to anchor to their container location, or need to look for a container for their source to use if it gets built after they have started mining."

No todo.md item mentions "repair", "refill" (by that word), "bootstrap", or "RCL rush" explicitly — the above are the nearest matches.

## Extra context worth carrying into ADR 0040
- 0033 explicitly positions `screeps-sim-core` as "a reusable mechanism foundation for future non-combat sims (economy / hauling / lifecycle — ADR 0028, `screeps-ibex-eval`)" (0033:70, 427) — an offline economy simulator has a pre-sanctioned substrate and, per EP-6.14, should build on it rather than mint a new world/physics.
- The ADR corpus already has an economy micro-ownership map: ADR 0006 owns the colony-health sim, ADR 0007 hauling/logistics, ADR 0011 spawn orchestration, ADR 0004 CPU governance — "economy / colony / the full game tick — owned by ADR 0006's colony-health sim" is 0033's out-of-scope line (0033:58). ADR 0040 should include a Division-of-ownership paragraph reconciling against these.
- The `[[sim-determinism-fence]]` (spread-0, quantize-before-discrete-decision, no HashMap on decision paths) is treated as non-negotiable doctrine in both 0033 (§D7) and 0038 (D8 + test #11).

## Citations
- C:\code\screeps-ibex\docs\design\adr-template.md:1 — ADR template: title form, Status/Date/Deciders/Related bullets, Context/Decision/Alternatives/Consequences/Incremental-Migration-Path headings
- C:\code\screeps-ibex\docs\design\README.md:5 — New-ADR convention: copy adr-template.md, number sequentially NNNN-title.md; status lifecycle line 6; register table lines 9-51; standing constraints line 54
- C:\code\screeps-ibex\docs\design\0033-rover-pathing-sim-and-benchmark.md:3 — Status-bullet-as-milestone-ledger convention (M0..M5 landing history in the Status line)
- C:\code\screeps-ibex\docs\design\0033-rover-pathing-sim-and-benchmark.md:8 — Division-of-ownership paragraph convention (what this ADR owns vs sibling ADRs)
- C:\code\screeps-ibex\docs\design\0033-rover-pathing-sim-and-benchmark.md:70 — Decision-provenance note pattern (operator overrode an independent analysis, rationale recorded)
- C:\code\screeps-ibex\docs\design\0033-rover-pathing-sim-and-benchmark.md:218 — §D2 anti-duplication map: REUSE-AS-IS / REUSE-AFTER-MOVE / SHARED-FIX / NET-NEW disposition table
- C:\code\screeps-ibex\docs\design\0033-rover-pathing-sim-and-benchmark.md:307 — §D5 metric-set pattern: formula + bold Gate + L-layer tag per metric
- C:\code\screeps-ibex\docs\design\0033-rover-pathing-sim-and-benchmark.md:347 — §D5.4 objective function: w = quantize(G·r_bid) in e/t, gates G=Δ_crit·A·S·U, role table, H=ΣWη/ΣW aggregate with bootstrap CI, numbered open-decision list operator-veto-pending
- C:\code\screeps-ibex\docs\design\0033-rover-pathing-sim-and-benchmark.md:351 — e/t declared the codebase's ONE cross-goal currency (value_e, room_net_roi/claim_value, body_cost/CREEP_LIFE_TIME, CPU at 0.02 e/tile)
- C:\code\screeps-ibex\docs\design\0033-rover-pathing-sim-and-benchmark.md:355 — Role table incl. worker/upgrader min(WORK·k, supply_rate)·v_sink with k = 5 build / 1 upgrade e/t/WORK — the RCL-progress-in-e/t precedent
- C:\code\screeps-ibex\docs\design\0033-rover-pathing-sim-and-benchmark.md:375 — §D7 determinism fence: det_repeat/det_reorder/det_hash_seed, spread==0 ship-blocker
- C:\code\screeps-ibex\docs\design\0033-rover-pathing-sim-and-benchmark.md:443 — Incremental Migration Path: named M0..M5 slices, each with a Gate, updated in place with LANDED dates; 'each slice shippable on master independently'
- C:\code\screeps-ibex\docs\design\0033-rover-pathing-sim-and-benchmark.md:461 — 'End state (as shipped)' section: shipped defaults by layer + final regression-tracked baselines + chip-tracked Remaining list
- C:\code\screeps-ibex\docs\design\0033-rover-pathing-sim-and-benchmark.md:427 — sim-core extraction explicitly reserved as the substrate for future economy/hauling/lifecycle sims
- C:\code\screeps-ibex\docs\design\0038-expansion-reach-gating-and-economic-claim-value.md:3 — 0038 frontmatter style: Status + 'One line' bullets (no Date/Deciders/Related — a template deviation)
- C:\code\screeps-ibex\docs\design\0038-expansion-reach-gating-and-economic-claim-value.md:61 — Root-cause map table convention: | # | Mechanism | file:line |, 'verified against the tree'
- C:\code\screeps-ibex\docs\design\0038-expansion-reach-gating-and-economic-claim-value.md:119 — claim_value(R,d) = intrinsic_roi·unlock_fraction·support_decay·plan_quality composition over room_net_roi
- C:\code\screeps-ibex\docs\design\0038-expansion-reach-gating-and-economic-claim-value.md:138 — C-DEFECT-1: distance must enter the composite exactly once; owned_colony facts use INTERNAL_HAUL_TILES, HoldModel::None
- C:\code\screeps-ibex\docs\design\0038-expansion-reach-gating-and-economic-claim-value.md:224 — Migration ledger convention: DELETE / KEEP / ADD lists at file:line granularity
- C:\code\screeps-ibex\docs\design\0038-expansion-reach-gating-and-economic-claim-value.md:263 — Sim-first test plan: numbered named kernel tests, RED→GREEN offline, kernel-unit-tests-only rationale
- C:\code\screeps-ibex\docs\design\0038-expansion-reach-gating-and-economic-claim-value.md:299 — WORLD_FORMAT_VERSION section pattern: bump-or-no-bump per artifact with reasons (22→23)
- C:\code\screeps-ibex\docs\design\0038-expansion-reach-gating-and-economic-claim-value.md:314 — Interactions rule: never fork/edit room_net_roi's body (would move war.rs numbers); new curves live in adapter-side pure kernels
- C:\code\screeps-ibex\screeps-ibex\src\room_economics.rs:88 — RoomEconomyFacts inputs: source_count, source_capacity, haul_tiles, hold_model, hold_body_cost, horizon
- C:\code\screeps-ibex\screeps-ibex\src\room_economics.rs:166 — room_net_roi formula: net e/t = gross − hold − mining − haul − cpu; net_roi = max(net,0)×horizon; term definitions
- C:\code\screeps-ibex\screeps-ibex\src\room_economics.rs:36 — Engine constants: RESERVED 3000 / NEUTRAL 1500 per cycle, regen 300, CREEP_LIFETIME 1500, CPU_PENALTY_PER_TILE 0.02, TILES_PER_ROOM 50
- C:\code\screeps-ibex\screeps-ibex\src\room_economics.rs:120 — owned_colony ctor doc: distance enters claim value only via unlock×support, never the kernel haul term
- C:\code\screeps-ibex\docs\guides\engineering-practices.md:38 — EP-2.1..2.10 seams/abstractions rules (new-systems design)
- C:\code\screeps-ibex\docs\guides\engineering-practices.md:62 — EP-4.1..4.8 CPU/budget discipline (stress-time behavior: shared pools, degradation at grant, never-shed set, shed re-decision, synthetic pressure)
- C:\code\screeps-ibex\docs\guides\engineering-practices.md:73 — EP-5.1..5.5 serialization rules (reset-anytime, breaking-change labels, fingerprint bumps, don't persist derivable, versioned telemetry schemas)
- C:\code\screeps-ibex\docs\guides\engineering-practices.md:81 — EP-6.1..6.14 testing/validation rules (kernel-vs-shell, pure-by-design, paired-diff gates, plan-loss accounting, harness-never-lies, determinism prerequisite, one-artifact-per-gate)
- C:\code\screeps-ibex\docs\guides\engineering-practices.md:131 — EP-10.7 operator owns acceptance — ADRs stay Proposed until accepted
- C:\code\screeps-ibex\todo.md:3 — Partial-haul stickiness item (refill/haul defect)
- C:\code\screeps-ibex\todo.md:18 — Min-body-cost spawn-queue starvation item — closest existing item to stress-time energy prioritization
- C:\code\screeps-ibex\todo.md:27 — Per-room energy-over-time stats item (proto economy metric)
- C:\code\screeps-ibex\todo.md:22 — Shared predicted storage capacity per tick (hauler wait elimination)

## Gaps
- ADR 0033 lines 224-306 (§D2 table body, §D3 reused-server-half detail, §D4 oracle detail) were not read line-by-line — only their headings and roles; structure and metric/milestone patterns were fully captured from the surrounding sections, so this does not affect the requested pattern summary.
- room_economics.rs was read only through line ~180 (the room_net_roi doc + start of body); the mining/haul term implementation bodies and the tests block (:181-243 per 0038) were not read — the formula terms are quoted from the authoritative doc comment.
- todo.md contains no items literally mentioning 'repair', 'bootstrap', or 'RCL rush'; the listed items are the nearest matches (spawn min-body starvation, per-room energy stats, RCL post-process planner items).
- ADR 0006 (eval-and-iteration-harness) and ADR 0028 (lifecycle harness) — named by 0033 as the sibling sim pattern and a future sim-core consumer — were not read; if ADR 0040's economy simulator claims the 'colony-health sim' territory 0033 assigns to ADR 0006, that division-of-ownership needs a direct read of 0006 before drafting.