# Implementation Tracker — the living master status doc

> **★ This is the single forward-looking source of truth.** If you are resuming cold, read §1–§4
> and stop; the rest is reference. Supersedes the "single source of truth" claim in
> [`../plans/combat-overhaul-plan.md`](../plans/combat-overhaul-plan.md) and the forward-looking half
> of [`phase-2.md`](phase-2.md) (a historical log, frozen 2026-06-23).
>
> **Last reconciled against code:** 2026-08-22 (56 ADRs verified by an 11-agent code-grounded pass;
> 29 carried stale headers). History and method: [`project-reconciliation-2026-08-22.md`](project-reconciliation-2026-08-22.md).

## The document model

Three tiers, each answering exactly one question. Introduced 2026-08-22 — before that, ADR
`Status:` conflated the design's maturity with the code's progress, which is why 29 of 56 headers
had drifted.

| Tier | Answers | Lifetime |
|---|---|---|
| [`../design/`](../design/) — ADR | *What are we building, and why?* | Permanent. **Contains no status.** |
| [`../implementation/`](../implementation/) — impl doc | *Where am I, what is the next action?* | Ephemeral — created when work starts, **deleted when it closes**. |
| **this file** | *What is in flight across the project?* | Permanent index, one line per item. |

**An ADR's `Status:` is now only ever `Decided` / `Draft` / `Superseded by NNNN` / `Withdrawn`** —
a statement about the *design*, never the code. There is deliberately no "Implemented". Whether
something is built is answered here, in §5–§6. Conventions and the impl-doc template:
[`../implementation/README.md`](../implementation/README.md).

## How to use this document — the trim rules

This doc tracks **status and open work only**. It must stay small enough to read in full.

1. **Detail lives in the ADR, never here.** A line here is a pointer plus what remains. If you find
   yourself explaining a design, put it in the ADR and link it.
2. **Done collapses.** An ADR with zero open items drops out of §6/§7 and becomes an ID in §5's
   Closed list. Do not leave a paragraph behind describing what it used to be.
3. **One line per open item.** If an item needs a paragraph, it is a workstream — promote it to §1
   or §3, or give it its own doc and link it.
4. **§1 holds exactly one workstream.** That is a policy, not an observation. Finishing beats
   starting; if §1 is full, §3 waits.
5. **Closing anything appends one line to §9** and deletes the entry. §9 is the only place that
   grows without bound, and it is one line per close.
6. **Bulk documentation drift is a chore, not work.** See CHORE-1 — do not create one tracking entry
   per stale header.
7. **Target ≤ 300 lines.** If it is longer, something that should have collapsed did not.
8. **Every session closes with an update** (standing convention, operator 2026-08-22): before a
   session ends, refresh §1's state, tick the active impl doc's checkboxes + log, and record any
   FOUND WORK — new defects, discovered gaps, process lessons — either under its owning ADR in §6,
   in the active impl doc, or (if unowned) in §7. Progress that lives only in a chat transcript is
   lost progress.

---

## 1. NOW — the single active workstream

### NOW: Phase 4.5 — the WS-VAL defect program (items 1+2+4+5+6 + RULING-9 currency + boost flip ALL DONE 2026-08-24; next = item 7 parity remainder → item 8 clamp-then-doctrine)
See §3 Phase 4.5 for the ordered ledger + acceptance bars. Prior arc below (the corpus that built the instruments).

### Prior: WS-VAL — combat validation corpus (operator directive 2026-08-23) — corpus LANDED
→ [`../implementation/ws-val-combat-validation-corpus.md`](../implementation/ws-val-combat-validation-corpus.md) · parity report [`../reviews/live-sim-parity-audit-2026-08-23.md`](../reviews/live-sim-parity-audit-2026-08-23.md)

**Landed 2026-08-23**: the engine-exact stronghold corpus (bunker1–5, real boosted defender
populations, chokepoint + multi-room layouts, focusClosest/focusMax tower AI), the border gauntlet
(crossing under camper fire, grades 1–4), the boosted self-play lane (tier-mirror basket +
T3-vs-T0 twin pin), the **boost-blind seam fix on BOTH sides** (live DTOs/member views + sim
adapters now price boosts via one shared `effective_output`; live heal_power un-latched — parity
H4/H7/M15/M16/M19), and the ultracode live↔sim parity audit (43 confirmed findings; backlog ranked
in the report, H0 neutral-wall intent drop on top). **Honest baseline**: T0 defers ALL strongholds
(quantified heal ceiling — why live only ever killed towerless cores); L1@T3 fields but loses
tactically (members picked off strung-out — heal-adjacency collapse); border g1 (2 unboosted
campers!) stalls; boosted meta reshuffles (default tactics lose at T2/T3). **Follow-up queue (in
the ws doc)**: cohesion under focused fire → border crossing → lone-survivor policy → parity H0 →
boosted re-tune. Acceptance bar for the tactical work: `stronghold_gauntlet` L1@T3 → Killed.

### Prior arc: the military program (Phases 3+4) — ALL SHIPPED 2026-08-23; live-watch open
→ [`../implementation/ws-wvc1-military-completion.md`](../implementation/ws-wvc1-military-completion.md) · [`../implementation/ws-wvc2-defensive-features.md`](../implementation/ws-wvc2-defensive-features.md) · [`../implementation/ws-4-r19-retune.md`](../implementation/ws-4-r19-retune.md)

**The operator's "peak effectiveness" program (reorder 2026-08-23) completed in one arc**: WvC-1
(correctness + wiring, `e08162810921`), WvC-2 (defensive features, `1fb233b30416`), and the WS-4
R19 re-tune (`9913ef980109`) all code-complete, tuned, and live as hot swaps; fence green ×5.
Live validated post-ship (CPU 34/140, bucket full, 0 panics; 8th AND 9th rooms claimed mid-arc).
**Live-watch is what remains**: the next real fight exercises the give-up clock, urgent-defender
downsizing, S5-CAP surge, rampart cover-anchoring, predictive safe mode, exit discipline, and the
re-tuned open-combat profile. When it closes, the three ws docs delete. **Phase 5 (boost): the ENTIRE CONSUMER SIDE (P0–P3) landed DARK 2026-08-23 + shipped** (hot swap
`00cf29b552ff`) →
[`../implementation/ws-5-boost-pipeline.md`](../implementation/ws-5-boost-pipeline.md): the boost
EV axis (T-TOWER-3 proof green), supply clamp + populated `available_boosts`, persisted
`CombatBodySpec.boost` (reset-free — 0047 obviates the planned WFV bump), and the full apply wire
(BoostQueue → LabsMission fulfiller → `AwaitBoost` job, renew-skips-boosted). ALL inert behind
`features.military.boost_military` — **FLIPPED ON (live, watched) 2026-08-24**; the P4 re-tune
landed as Phase 4.5 item 6. Remaining: the watched first boosted engagement (shakedown), O4
market-fed valuation (constants suffice for first light), then ADR 0010 L1/L2 demand-driven supply.

WATCH: movement CPU at 9 rooms (transient post-swap spikes ~81-95, steady ~34; the H3/M13 threat
overlay + M4/M6 landed 2026-08-24 — un-memoized cross-room builds spiked ~120, fixed `2f38c1a`,
settled to ~85-93 post-swap; VERIFY it returns to ~34 once path caches warm, and attribute any
sustained elevation to the overlay first); segment chars;
wasm 49.0%; foreman `InvalidTarget` transients; post-hot-swap one-tick `INTEGRITY` squad-ref
scrubs (benign backstop, attribute if it recurs outside deploys). **WS-VAL swap (wasm
`039c587dc1c6`, 2026-08-23) tail-verified clean** (0 panics/deser, war pricing through the new
boost-aware path). NEW WATCH: live threat assessment now prices BOOSTED hostiles at real strength
(×2–×4) — defense sizing may correctly grow vs boosted invaders, and offense may correctly defer
fights it previously under-priced; attribute any "why did sizing change" observation here first.
NEW WATCH (2026-08-24): boost_military is ON (live) — first sized fight should file BoostQueue
requests, run labs, spawn AwaitBoost creeps; attribute lab/spawn/sizing observations here first.

---

## 2. BLOCKED

- ~~**B-1 · `com.docker.service` Stopped/Manual**~~ **RESOLVED 2026-08-24** (operator updated +
  relaunched Docker; `docker ps` responds). The private-server/harness lane is AVAILABLE again:
  H5 parity oracle, P2.M2-LIVE, M4 exit criteria, 0036 live-raze, 0028 closeout (all →HARNESS in
  §6) can be scheduled. RULING-8's deploy-to-live posture stands regardless.

---

## 3. NEXT — the completion roadmap (decided 2026-08-22)

Goal: **finish what is started before starting anything new.** Ordered so each phase closes the
maximum number of open ADR tails; Phase 2 is a *decision* pass, not a build pass. One workstream
active at a time (§1).

**Phase 0 — close WS-1** (above). Observe live, judge C1–C5, L2 last. Ride-alongs while in the
deploy loop: ~~UNOWNED-4~~ **retuned 1→2 + live-reconciled 2026-08-23** (UNOWNED-5/6 also closed
— features.rs doc fixed, `allow_replan` deleted). *Closes: 0046→Live, 0038, 0017's deploy residue, 0021 re-head, the
expansion program; collects the 0018 "has an SK farm ever run" evidence for free.*

**Phase 1 — WS-2 · Combat review Tier −1 Wave B** →
[`../implementation/ws-2-combat-wave-b.md`](../implementation/ws-2-combat-wave-b.md).
**CODE-COMPLETE 2026-08-23**: D2/D3 `8fa0c60`, D4/D5/D6 `be5ce24`, D28 `b26eba4`, D9/D10
`1a85a57` + rover `850a06b`, T1/T2 ruled retained-by-design. 15 RED-verified pins, no WFV;
ADR 0027 amended. **SHIPPED to live 2026-08-23** (hot swap `0d9524f2668f`, world persisted — missions carried through, 0 deser errors). Live-watch, then the WS-2 doc deletes.
*Closes: 0037's decision item and the 2026-07-09 review's Tier −1 as a live work list.*

**Phase 2 — the triage pass — DONE 2026-08-23** (verdicts in git: `f3c822b`, `29072c3`, the sweep). Original brief: **decide, don't build** (one session; create its impl doc — the
verdicts must land as ADR amendments via `Design deltas`). Every §6/§8 item gets one of three
verdicts: **schedule** (into Phases 3–6), **amend out** (shrink the ADR's end state — candidates:
0030 `EngagementTempo` → Withdraw/fold into 0031; 0020 S5/S6/S7 keep-or-cut; 0026a's six deferred
modes; 0039 P2–P4; 0025a residual → close as documented-mitigated), or **do now** (the §8
wire-or-delete one-liners, then **UNOWNED-3: remove `#![allow(dead_code)]`** so the compiler
enforces the register). *Exit criterion: every remaining §6 line is inside a scheduled phase.
Plausibly closes 6–10 ADRs by amendment.*

**Phase 2.5 — WS-6 · ADR 0047 — SHIPPED 2026-08-23** (msgpack world stream + Plan shrink, WFV 29 live; the LAST format-transition reset). WATCH: segment chars once plans rebuild (projected 12–14% of budget); wasm binary 48.5% of code limit.

**Phase 3 — the military completion waves (WvC-1/WvC-2, operator-prioritized 2026-08-23).** WvC-1 (correctness + wiring): T-HEAL-3a winnability inputs, the damage.rs readiness-tranche wiring, S5-CAP governor-dynamic squad cap, 0035 FU2 never-engages give-up, 0026 L8 observed-bodies coordination, 0034 rally-bias live-wire, 0028 K3/K4 wiring. WvC-2 (defensive features): T-DEF-1 rampart-anchored defenders, T-DEF-5 predictive safe-mode arm, T-POS-5 exit-tile cost, 0037 T3 candidate emission. Batched + shipped like Wave B (no WFV — and under 0047, even shape changes are cheap now).

**Phase 4 — WS-4 · R19 chokepoint re-tune — DONE 2026-08-23** (same-day as Phase 3): chokepoint basket + maximin tournament built; `open_combat` re-tuned a1-i6-tight-s2 → a0-i3-d14-K3-s2 (the only cross-regime-positive config; the old profile measured NEGATIVE vs untuned default — R19 quantified) + SHIPPED (`9913ef980109`); 0031a/b re-swept at w_energy=1.0 (defaults confirmed, 0031b §5); 0019 S4-TUNE + 0024 FU#4-presets + 0033 kite-retune closed defaults-confirmed via `s4_weights_retune` (flat surface — the EV kernel owns engaged positioning); 0033 corpus-wide fence promoted (spread==0 over 21, H 0.9625). Re-tagged →P6: 0026 L6c (consumer-gated per its own rule), 0032 value_e (its ADR says "later"; no discriminating bed).

**Phase 4.5 — WS-VAL defect program (ACTIVE — operator 2026-08-23: "we'll work through the
defects found from the broad review afterwards").** The instruments exist and are checked in; each
item has an acceptance bar. Ordered: **(1) cohesion under focused fire — ✅ ACHIEVED 2026-08-24** (decision `be725c9`, deployed): the EV kernel now prices REAL heal delivery —
deliverable-heal advance gating (no squad-total optimism, no catch-up slack), a risk-currency
floor (g_us collapsed to ~0 in structure sieges — members priced their HP at nothing) with a ×4
uncovered-net steepener, lockstep healer-tile advertising, and evidence-gated URGENT/BACKLOG heal
triage (full-HP healers were self-pre-healing against field-stamped threat while the actually-
focused member died in a part-loss spiral). **(2) border crossing under fire — ✅ ACHIEVED
2026-08-24** (decision `b0b7ea0` + agent `1fbff1b` + eval `8dab84d`, from the operator's live
replay observation "one creep enters and everything outside the room stalls"): bloc
border-crossing gate (gather at the exit band, cross together), full-roster member views (parity
H5 — the in-room-only scoping was BOTH a live divergence and the crossing livelock root),
fight-room kernel anchoring (`plan_squad_ev` room param — the centroid-room V-1 aliasing sent a
mid-crossing squad's goals into the staging room), room-gated mover anchor, exit-edge tile pricing
(edge tiles are transitional, never holdable — the doorway jam), room-local tower assessment (the
150-floor made cross-room towers phantom threats), rout-to-rally, Retreating state decay. Also
flushed out a LATENT fixture bug (twin_room_siege built its "target-room" core in the staging
room). **Result: EVERY L1 rung @T3 Killed** — open 172 / choke 273 / choke-multi 590 (the full
cross-border stronghold assault end-to-end) — **and border g1@T0 74 / g1@T3 49 / g2@T3 50, all
with the bloc crossing together** (pinned: `stronghold_floor_t0_defers_t3_kills_every_l1_rung`).
Remaining ladder: border g3/g4 (T2/T3 camper packs — sizing defers; capability item 8 territory);
**(3) lone-survivor policy** — largely superseded by rout-to-rally, keep the bar (no `Timeout`
with a lone surviving member) as a watch; **(4) parity H0 — ✅ FIXED 2026-08-24**
(`is_combat_targetable`, squad_combat.rs): the execution-side structure list now includes neutral
constructed walls with hits in both arms, matching the decision layer — no native pin possible
(JS types); verify on the next live neutral-wall breach. H5/M0 also closed by the item-2 batch
(sim full-roster views + shared-kernel room fixes). Original H0 text: live drops
Attack/Dismantle vs NEUTRAL constructed walls (`get_hostile_structures` filter; sim executes them);
**(5) threat/traversal unification — SIM SIDE ✅ COMPLETE 2026-08-24** (decision
`acf3600`+`71c6e0a`, agent `bed5f0e`+`4e68de4`, eval `1736ea6`): the sim traversal field now IS
the shared `build_room_threat_field` (cover-aware, hostile-energized-tower-gated,
unboosted-stamped — H1/H2/M3/M8) and own ramparts are walkable (M2). Unblocked by the DRAIN
REWORK the substitutions forced: drain-aware placement (the harness placed the squad at ~r11
INSIDE the falloff — the focused tank died in ~3 ticks every run, every variant, and the canary's
"Killed" rode whichever remnant survived; a corrected earlier note blamed the form phase — it was
placement), DELIVERABLE standoff sizing (total-heal optimism picked r15; the survivable band is
r19), distinct rear-ring support slots, URGENT_HEAL_MULT 8→3 (flat heal premium vs
residual-diluted attack value let survivors PERCH — asymmetry queued), and the approach-aware
PLATEAU TIE-BREAK (equal-cost bands drift objective-blind under lower-x/y; ties now prefer
smaller approach distance — the last blocker). COMPLETE 2026-08-24 (super `1abcc8f`, rover `fae493b`): the live mover now prices threat —
squad-manager-published per-tick `RoomThreatCosts` overlaid under the structure layer's hard
blockers via `ThreatOverlayCostSource` (H3/M13; executed paths — approach/rejoin/retreat/civilian
traffic through war rooms — route around kill-zones like every sim-validated trajectory), and the
decide room-callback honors the requested room (M4/M6). Other queued finds:
goal-convergence churn (exact-claim tried/reverted — needs the EXP register in the loop);
~~heal-vs-offense EV asymmetry~~ → became the RULING-9 redesign, below. Instruments:
`SQ_DEBUG`/`SQ_DEBUG2`/`SQ_DEBUG3`/`SQ_DEBUG4` env-gated traces;
**(RULING-9 currency redesign — ✅ ACHIEVED 2026-08-24** (decision `f8b97a6`, agent `34152cd`,
eval `04c7414`): ONE progress-diluted currency across all three EV legs. Heal value_per_hp =
`g_us × member_output / horizon_hits` (TRIAGE price, swells near death; wounded-evidence includes
hostiles inside weapon band); self-RISK = `net × my own value_per_hp` at the **MARGINAL** price
`g_us.max(unit) × my_out / max(tile_raw, hits/H)` — the triage form diverges as a risk price
(healthy ≈ risk-blind, six of eight died in the choke kill zone; the doomed remnant priced its hp
infinite and parked to timeout), marginal is bounded both ways. Flat `NET_RISK_MULT` deleted
(out-massed every diluted attack value → universal cowardice, T3 refused a 4:1 trade). Batch also
flushed two latent defects the currency exposed: the **kite dead-zone** (kite-plan-None suppressed
the EV kernel → stable non-fighting equilibrium at the melee-band edge; now falls through, except
Retreating) and the **focus stall attractor** (our_dps=0 comps focusing unkillable creeps over a
live structure objective). Heal incumbency dead-band (1×unit) stops value_per_hp breathing from
re-deciding positions (designed#2 29.5%→5%). Gates: floor pin all-rungs, t3_twin (repinned vs
HOLDING defender — the mirror fleer is honestly uncatchable), oscillation, drain, assembler, EXP;
1537 workspace + fence green. Generated-bed fairness bound 2000→3000 (mirror fights now TRADE —
order-bias compounds over real casualties; sign still seed-varying).**)
**(6) boosted kernel re-tune — ✅ ACHIEVED 2026-08-24** (decision `173cd5f`, eval `8997234`;
merges ADR 0041 P4): `open_combat` re-adopted as **`a2-i6-tight`** via the new
`joint_boosted_terrain_retune` instrument (3 boost tiers × 3 terrain classes = 9-cell maximin over
`chokepoint_comp_basket` boosted per tier). The R19 winner regressed to worst −852 under the
RULING-9 currency; no config is positive in all 9 cells (literal maximin = the untuned default),
adoption used maximin-with-a-noise-band (a2-i6-tight: −26/−18 noise cells vs mean +652, the
strongest boosted generalizer). Full battery + fence green. **Unblocks the watched
`boost_military` flip (the RULING-9 next step)**;
**(7) the rest of the parity backlog** (H5 roster scope, H6 mover config, H8 tower-path fork,
H9 lifecycle inputs, M4/M6 wrong-room matrix — ranked in
[`../reviews/live-sim-parity-audit-2026-08-23.md`](../reviews/live-sim-parity-audit-2026-08-23.md));
**(8) stronghold capability** — L2+ defer even at T3 (multi-squad assault doctrine or a
deliberate member-clamp lift for sieges; a DESIGN fork, stop-and-ask).
*Closes: the WS-VAL follow-up queue; makes the corpus verdicts green instead of honest-red.*

**Phase 5 — Boost pipeline (ADR 0010 L0 → 0041) — CONSUMER SIDE (P0–P3) SHIPPED DARK 2026-08-23**
(see §1 prior arc + [`../implementation/ws-5-boost-pipeline.md`](../implementation/ws-5-boost-pipeline.md)).
Remaining: O4 market-fed valuation (constants suffice for first light), the deliberate live
activation shakedown (`boost_military` flag flip, watching), the P4 rung sweep (now over the
WS-VAL boosted basket — same instrument as Phase 4.5 item 6), then ADR 0010 L1/L2 demand-driven
supply. *Closes: 0010, 0041; unblocks 0019 boosted-TOUGH, 0020-TOUGH, 0008a Tier 3, 0008 S2.*

**Phase 6 — economy completion.** The 0043 band→EV conversions (A2/A4/A7/A9/A10, A11, A12,
C1–C7), 0042 `opportunity_floor` + R1–R4, 0044/0044a P3 all-sinks activation, 0007 item 4,
0040 §D8 reserve retirement. Mechanical batch work against a shipped market.
*Closes: 0007, 0040, 0042, 0043, 0044, 0044a.*

**Phase 7 — remaining designs.** WS-5 (0045 power creeps), 0020 S5–S7 (after Phase 4, ratified), plus
whichever of 0011/0012/0013/0014/0015/0016 the triage keeps (0013's spending half is already
delegated to 0045; 0014 may reduce to the W4 `WarDecl` hook owned by 0008). New builds, so last
by policy.

*Convergence: Phases 0–2 ≈ a week of sessions, taking the corpus from 2 Closed to ~15–20 Closed;
Phases 3–5 are the military program (waves → re-tune → boost capstone); Phase 6 economy; Phase 7 is a choice, not a debt.*

---

## 4. Deployment ledger

| Where | Artifact | WFV | Date |
|---|---|---|---|
| Live MMO (shardX) | wasm `bd6eebcc0f56` (Wave B + retune hot swaps) | **28** | 2026-08-23 |
| Docker private | `ab692bd` (stale — refresh when B-1 clears) | 27 | 2026-07-28 |
| `master` | HEAD (WFV-anchored; do not pin a SHA here — it drifts every commit) | 28 — **live on MMO** | since 2026-08-22 |

**The deployed-artifact test point is now `77dc9cc`** (2026-08-22); anything after it is undeployed. Use this as the test when an ADR claims a
deploy — pre-split ADRs claimed deploy dates predating the only real one (fixed by the doc split).
`wfv27-deployable-e857c76` is the historical WFV-27 point. Live MMO baseline 2026-08-23: **8 rooms**,
GCL 12, bucket 10000, W7N47 under remote-build.

---

## 5. ADR state index

56 ADRs. States: **Live** (in `ab692bd`) · **Host-only** (offline tooling, never in the wasm
bundle) · **On master** (merged, undeployed) · **Partial** · **Design-only** · **Closed**.

**Closed — no open work. Detail in the ADR; do not re-track.** `0001`, `0005` (containment ratified as-shipped 2026-08-23), `0009c`, `0037` (T1-T3 landed; emission closed by ruling 2026-08-23), `0038`

**Live** — `0002 0004 0008 0047 0017 0019 0024 0025 0027 0029 0031 0031b 0032 0034 0035 0036 0040 0042 0044 0044a 0046`
**Host-only** — `0006 0023 0023a 0025a 0026 0026a 0033`

**Partial** — `0003 0007 0008a 0009 0009a 0009b 0011 0012 0018 0020 0021 0028 0031a 0039 0043`
**Withdrawn** — `0030` (2026-08-23; tempo axis preserved in 0031 — no open work)
**Superseded** — `0022` (by 0027; its P-AUCTION residue is owned by 0020/0031, and its P-OBJ asks were superseded by 0027's observed-success model — no open work of its own)
**Design-only, zero code** — `0010 0013 0014 0015 0016 0041 0045`

Open work for these is in §6 and §7. An ADR absent from both is Closed.

---

## 6. Open work by owning ADR

One line per item. **Phase tags** (sweep 2026-08-23): P3 boost · P4 R19-tuning · P5 economy · P6 remaining designs · **WvC** = the post-P4 combat wave (small live combat fixes batched like Wave B) · HARNESS = B-1 lane · WATCH = live observation.

**Combat**
- `0008` — S2 boost handoff **→P3**; S1 pre-spawn, W2 trim + W4 `WarDecl` **→P6**; O5 power-bank + heavy assault = deferred capabilities (activate by decision, not schedule).
- `0008a` — T-HEAL-3 (widens into R1) **→P3** (T-HEAL-3a SHIPPED, WvC-1); T-DEF-1/T-DEF-5/T-POS-5 SHIPPED (WvC-2); Tier 3 **→P6** (after P3).
- `0019` — boosted-TOUGH **→P3** (blocked on 0041). (S4-TUNE DONE, WS-4: presets confirmed flat — no adoption.)
- `0020` — S5/S6/S7 (blob auction + R7 currency, adaptivity, adversarial room-gen): **operator-ratified scheduled end-state, sequenced AFTER Phase 4** (they want the R19-retuned kernels). S5-CAP SHIPPED (WvC-1): empire-scaled cap + defense surge, shared `claim_pacing` kernel.
- `0026` — L6c **→P6** (re-tagged WS-4: its `DoctrineParams` consumers are the unbuilt rungs 2–3 — per L6c's own rule, a weight with no consumer has nothing to sweep). (L8 SHIPPED, WvC-1.)
- `0026a` — modes activate as their signals land **→P6** (catalog; no standalone schedule).
- `0027` — Farm producers (PowerBank needs its own ADR) + salvage-teardown migration **→P6**.
- `0028` — `run_defended_lifecycle` closeout **→HARNESS** (K3/K4 RESOLVED, WvC-1: claim_admission is the shared kernel, `claims_allowed` harness-only; K3 adapters separate by design); multi-squad lane contention folded into that closeout (WvC-2 ruling: it is scenario-coverage beds 1+3, not bot work).

- `0031` — Tier-2 archetype search + Tier-3 axes **→P4** (the 0031a sweep plan).
- `0031a` / `0031b` — re-sweep DONE (WS-4, 0031b §5): defaults CONFIRMED at w_energy=1.0; margin knobs inert under the binding cost term. Tier-2/3 archetype axes remain **→P6** (with 0031).
- `0034` — convergence gates **→P4** (D6c renewable-rally bias SHIPPED, WvC-1).
- `0035` — FU1 **→P6** (poll-until-fresh sufficiency undecided; FU2 CLOSED, WvC-1: terminator composition + stall-aware give-up clock).
- `0036` — live raze confirmation **→HARNESS** (private-server world mechanics, B-1).
- `0039` — P2–P4 **folded into the harness lane** (2026-08-23): re-activate with H5 **→HARNESS**.

**Economy**
- `0007` — item 4 (route-distance hauler sizing + shared predicted capacity) **→P5**.
- `0010` — L0 SHIPPED dark 2026-08-23 (`available_boosts` populated, `BoostQueue` wired with owner-staged clears, `AwaitBoost` calls `boost_creep` — ws-5 P1+P3); remaining: chain math + L1–L4 demand-driven planner/labs/factory **→P3** (after the 0041 activation shakedown).
- `0012` — M2/M3 **→P6**.
- `0040` — §D8 #2: the 20% military reserve (`economy.rs:87`) was never retired post-soak. Owns review R15 **→P5**.
- `0042` — `opportunity_floor` still hardcoded `0` (`squad_manager.rs:1868`, gated on 0043 A2); R1–R4 refinements **→P5**.
- `0043` — A2/A4/A7/A9/A10 band lerps still live in `spawn_policy.rs`; A11 importance margin; A12 exponential backoff; C1–C7 vetoes **→P5**.
- `0044` / `0044a` — P3 all-sinks only partially activated (build/repair bids are admission gates, not EV-priced haul registrations); per-lane road awareness; Phase-3 verification never recorded **→P5**.

**Rooms, expansion, infrastructure**
- `0003` — `MissionResult::Wait/Idle` park-don't-teardown **→P6**.
- `0009` / `0009a` / `0009b` — planner revamp (bench evaluator gates it) **→P6**.
- `0011` — D5 assist, G3 incubation, empire spawn-budget orchestrator **→P6**.
- `0017` — M5b escort (owned by 0008 **→P6**); abort-threshold tune **→WATCH** (needs live attacker evidence).
- `0018` — K4 mineral + K-RECONCILE (incl. `HoldModel::Suppress` unification) **→P6**; SK-farm live evidence **→WATCH**.
- `0021` — follow-ups #5/#6 **→P6** (#1/#2 absorbed by 0046, live).
- `0046` — staleness-bucket quantization tune rides live observation (low priority; C1–C5 all passed) **→WATCH**.

**Platform / tooling**
- `0004` — governor pressure-scenario calibration **→HARNESS**.
- `0006` — server-harness combat scenarios absent (`Fault` enum is only CpuBurn/GlobalReset/PanicOnce); **H5 sim-vs-server parity oracle** (golden vectors + nightly gate — reassigned here from 0008/0028, see UNOWNED-2; blocked on B-1) **→HARNESS**.
- `0013` / `0014` / `0015` / `0016` / `0045` — design-only. 0015 (testkit + seam registry) and 0016 (HUD) were marked "in scope" by the ultracode completion kickoff, a program that has driven nothing since 2026-07-02 (RULING-5) **→P6**.
- `0023` / `0023a` — S5 border scenarios, cross-room `Flee`, MultiRoom generator **→HARNESS**. Partially advanced by WS-VAL: the border gauntlet + multi-room stronghold scenarios ARE sim-side border-crossing beds (and they FAIL honestly — Phase 4.5 item 2 is the fix lane).
- `0025` — `action_oscillation_rate` metric **→P4** (rides the sweep instrumentation). (0025a residual closed 2026-08-23.) WS-VAL grew the harness: stronghold + border gauntlet corpus + boosted self-play lane (2026-08-23). Documented corpus approximations to revisit ON EVIDENCE: fortifier rampart-repair unresolved (no creep-repair intent in the sim), defender micro = `Hold` (not the engine's coordinated spot-walk), L5 anti-nuke fortify out of scope, roads/containers omitted.
- `0033` — BOTH P4 items DONE (WS-4): kite retune = defaults confirmed via `s4_weights_retune`; corpus-wide fence = `full_corpus_evaluation_is_deterministic` (spread==0 over 21, H 0.9625).
- `0041` — P0–P3 SHIPPED dark 2026-08-23 (flag `boost_military` OFF); remaining: O4 market valuation, the deliberate activation shakedown, P4 rung sweep (over the WS-VAL boosted basket) **→P3**. WS-VAL verified the T3 unlock in sim (L1 fields; T0 defers everything — the quantified boost case).

---

## 7. Cross-cutting work with no ADR owner

- **UNOWNED-1 · Ship WFV 28.** No ADR owns "soak and deploy". Owned here as **WS-1**.
- **UNOWNED-2 · H5 sim-vs-server parity oracle — assigned to ADR 0006** (2026-08-22; 0008 had
  mis-routed it to 0028). No `parity.rs`, no golden vectors, no nightly gate. Blocked on B-1.
  Listed under 0006 in §6.
- ~~**UNOWNED-3**~~ **CLOSED 2026-08-23** (ws-triage): the crate-wide allow is GONE; 115 warnings triaged to zero. The compiler now IS the dead-code register. New annotations carry an owner tag (KEEP/TEST-PINNED/FOLLOW-UP).
- **UNOWNED-4 · `remote_mine.search_radius` still defaults to `1`** (`features.rs:209`) — the
  expansion Wave-1 fix shipped the knob at the value that was the bug. "Wave 1 done" reads as if the
  remote ring widened; it did not.
- ~~**UNOWNED-5**~~ **CLOSED 2026-08-23**: the `SourceKeeperFeatures` container doc contradicted
  its own field default for two months; it now records the operator's real 2026-06-18 default-ON
  decision.
- ~~**UNOWNED-6**~~ **CLOSED 2026-08-23**: `construction.allow_replan` **deleted** (declared but
  read by no code — an operator flipping it silently got nothing). Re-add a replan flag together
  with its consumer when discretionary replan lands (0009).
- ~~**UNOWNED-7 · Stale `Memory._features` overrides**~~ **CLOSED 2026-08-22** by the
  `reset.features` one-shot (`77dc9cc`): setting `Memory._features.reset.features = true` rebuilds
  the persisted tree from compiled defaults next tick (self-clearing, like the other reset flags).
  Fired and live-verified on MMO the same day — `military.offense` reconciled `false→true`, new
  keys appeared, flag self-cleared. **Deliberate retunes now go through this pattern**, never a
  hand-edit that then shadows future defaults.
- ~~**CHORE-1 · 29 ADR headers are stale.**~~ **CLOSED 2026-08-22 by the design/implementation
  split.** All 56 ADRs were rewritten as pure end-state designs; status moved here and to
  [`../implementation/`](../implementation/). The drift class is now structurally impossible — an
  ADR header can no longer make a claim about code. Rollback tag: `pre-doc-split`.

---

## 8. Dead / unwired code register

Found 2026-08-22 by removing `#![allow(dead_code)]` and reading the compiler. Each is a decision —
wire it or delete it — not necessarily work.

| Item | Location | Note |
|---|---|---|
| `gameview.rs` | 104 lines, zero refs | The ADR 0006 seam Inc-6 record/replay and 0015's fakes both assume. Never migrated a single consumer. |
| `ui.rs` | 36 lines, `UISystem` never constructed | Doc comment claims consumers that do not exist. |
| ~~`BoostQueue`~~ | `military/boostqueue.rs` | **WIRED 2026-08-23** (ws-5 P3): manager files compounds, LabsMission fulfills, owner-staged clears. No longer dead — row kept for the register's history. |
| `issue_virtual_anchor_flee` | `military/formation.rs:398` | The **only** squad-level flee construct; nothing replaced it ⇒ squads have no coordinated retreat. Adjacent to review D10. |
| `Job::describe` layer | `jobs/jobsystem.rs:99,105` + ~15 jobs | Every job implements it; nothing dispatches it. A whole overlay with no renderer. |
| T1/T2 neighbour kernels | `war_decision.rs:182,327` | **Decided: retained by design** — sim/harness kernels (`run_v1_flow` proofs). WvC-2 ruling: NO offense-side feed — emission would contradict ADR 0037 T3 ("structurally incapable of opening a new attack path") + D27. Not dead code. |
| `HoldModel::Suppress` | `room_economics.rs:88,191` | Unreachable — SK farming runs a duplicate ROI kernel at `sourcekeeper.rs:99`. |
| `StructureIdentifier` | `structureidentifier.rs:7,32` | Superseded half of a live module. |

---

## 9. Rulings — decided 2026-08-22, do not relitigate

- **RULING-9 (operator 2026-08-24)** — Phase 4.5 tail order: **heal-EV principled redesign NOW**
  (reprice heal in the same progress-diluted currency as attack — an ADR 0025 semantics change,
  done BEFORE the re-tune so the tune grades the final currency) → **item 6 boosted re-tune** →
  **boost activation flip, watched** (gated on the re-tune) → item 7 parity remainder → **item 8:
  BOTH capability directions, clamp first** (siege member-clamp lift lands first for L2–L3 reach;
  multi-squad assault doctrine is the follow-on design for L4–L5). Phase 4.5 completes before the
  private-harness lane.

Recorded because the corpus contradicted itself and a future reader would otherwise reopen these.

- **RULING-1 · Minted `SquadId`/`SquadStore` (I1/I2) will NOT be built.** `EntityOption<Entity>` +
  `repair_entity_integrity` is the end state (ADR 0001, REC-009b). Three sources disagreed
  (0008 listed it open, 0020 said "dropped per 0022 D1", plan §3 and phase-2 CP-I list it blocking).
  ⇒ **CP-I is retired, not pending.** 0008 was retargeted to the marker-converted `squad_entity` in the 2026-08-22 doc split; plan §3 is historical.
- **RULING-2 · "Live" means "in the deployed wasm artifact."** Offline harnesses are **Host-only**,
  a separate state. Previously both were called Live, making "is it live?" unanswerable.
- **RULING-3 · D27 is closed AND created dead code.** Both facts stand; ADR 0037 owns the cleanup.
- **RULING-4 · ADR 0022's "no MMO deploy until all roadmap objectives are complete" is VOID.**
  `ab692bd` shipped with P-AUCTION and parts of P-OBJ unbuilt. Left unamended it reads as a standing
  block on every future deploy.
- **RULING-5 · The ultracode completion kickoff is dormant, not live.** It has driven nothing since
  2026-07-02. Do not treat its "in scope" list as commitments.
- **RULING-6 · R19 does not gate Wave B.** R19 gates *kernel-parameter tuning*; Wave B is safe-mode
  constants, roster/formation logic and adapter wiring. It **does** gate 0024 FU#4, 0031a Tier-2/3,
  0031b's re-sweep, 0032's `value_e` tuning and 0026 L6c — all of which currently list tuning as
  their next action without acknowledging it. Resolve in WS-4.
- **RULING-8 · Testing posture (operator 2026-08-23): deploy-to-live, batched.** The Docker soak
  lane is NOT a deploy gate until the operator is home. Validation = the offline sims/pins/fence
  where possible; otherwise test on live with batches large enough that a reset is acceptable.
  Rationale: empire impact is acceptable — the real cost is RECOVERY latency (MMO ~1 tick/sec +
  re-scout/re-plan convergence), which batching amortizes and no-WFV hot swaps avoid entirely.
  ADR 0047 (Draft) is the structural fix: reset-tolerant serialization so shape changes stop
  costing a recovery at all.
- **RULING-7 · Three distinct quantities are called `opportunity_floor`** — `market_adapter.rs:105`
  (computed, discarded), `transfersystem.rs:1669` (the one consumers see), and ADR 0042's forming
  give-up floor (hardcoded `0`). Name them separately; they are not one thing.

**Single owner for previously-duplicated items:** 20% military reserve → `0040` · `MAX_CONCURRENT_SQUADS`
→ `0020` · multi-squad lane contention → `0028` · boosted-TOUGH → `0041` · weapon archetype → `0031`
· BoostQueue → `0010` · `available_boosts` → `0010` · SK mineral K4 → `0018` · W2/W4 + S1/S2 →
`0008` · activator signals → `0026a`.

**Verified closed — do not reopen** (plan/phase-2 still list some as open): W3 escort producer
(`claim.rs:1269`, `81ed7f2`) · K2c-2 yield-to-defense predicate (`sourcekeeper.rs:337`) · U-TOWER
(`tower_fire.rs` → `missions/tower.rs:353`) · G workstream in full (legacy attack path deleted) ·
review D1/D11/D24/D25/D26/D27/R22 (Wave A).

---

## 10. Changelog

Append one line per closed item. Newest first.

- **2026-08-24 (latest)** — **boost_military ACTIVATED on live MMO (WATCHED)**: Memory._features.military.boost_military = true on shardX via the new rest-api console example (`eeabd42` — POST /api/user/console, shard auto-injected); verified by console echo of the military tree; 0 panics post-flip. WATCH: first boosted engagement (labs fill BoostQueue, AwaitBoost job, offense sized per tier); attribute lab/spawn/sizing changes here first.

- **2026-08-24 (latest)** — **Phase 4.5 item 6 (boosted re-tune, merges 0041 P4) ACHIEVED**: `open_combat` -> `a2-i6-tight` (decision `173cd5f`) via the new joint tier x terrain 9-cell maximin (eval `8997234`); the R19 profile regressed to worst -852 under the RULING-9 currency. Battery + fence green. Unblocks the watched boost_military flip.

- **2026-08-24 (latest)** — **RULING-9 one-currency EV redesign ACHIEVED + shipped** (decision `f8b97a6`, agent `34152cd`, eval `04c7414`): progress-diluted heal value_per_hp (triage form) + MARGINAL self-risk price (flat NET_RISK_MULT deleted) + kite dead-zone fall-through + our_dps=0 focus gate + heal-incumbency dead-band. All 6 gates + 1537 workspace + fence green. Full detail in §3 Phase 4.5.

- **2026-08-23 (late)** — **ADR 0041 P0–P3 consumer side complete, dark, shipped** (`00cf29b552ff`): EV axis + per-tier winnability (decision `04cc020`), supply table/clamp (`d217a3d`), persisted tier + required_boosts (`78e70b8`), apply wire (`2fd8253`+`82f2e83` — queue/labs/AwaitBoost/renew-skip). 7 RED-verified pins; fence ×2; byte-identical live at T0. `boost_military` = the activation switch.
- **2026-08-23 (late)** — **Live validation PASS** (CPU 34/140, bucket full, 0 panics, seg ~18%, INTEGRITY scrubs = benign REC-009b backstop) + **Phase 5 P0a landed dark** (decision `04cc020`): BoostTier EV axis, per-tier ceiling assessment, T-TOWER-3 proof green, 3 RED-verified pins; not separately deployed (byte-identical at T0). ADR 0041-P2 WFV bump obviated by 0047 (design delta).
- **2026-08-23 (late)** — **Phase 4 / WS-4 R19 re-tune DONE + shipped** (`9913ef980109`): `chokepoint_comp_basket` + maximin tournament (eval `940f739`); `open_combat` → a0-i3-d14-K3-s2 (decision `a7acb0b` — only cross-regime-positive config; old profile NEGATIVE vs untuned default, 0026a rejection reversed-with-reconciliation); 0031a/b re-swept at w_energy=1.0 → defaults confirmed, margin knobs inert (0031b §5); S4-TUNE/FU#4-presets/0033-kite-retune closed defaults-confirmed (`s4_weights_retune`, eval `bbc1184`); 0033 corpus-wide fence promoted (rover-eval `ab3e818`, spread==0/21, H 0.9625); L6c + value_e re-tagged →P6 (consumer/bed-gated). Process: run fences in RELEASE (18s vs 385s).
- **2026-08-23 (late)** — **WvC-2 code-complete + shipped** (hot swap `1fb233b30416`): T-POS-5 exit-tile surcharge (decision `3d451ac`), T-DEF-1 rampart cover via `ThreatField::build_covered` (`47a163a` — TAKEN/EV-risk/survival-veto/traversal all inherit the redirect from one point), T-DEF-5 predictive safe-mode arm (`8502af9`); 0037-T3 emission closed by ruling (contradicts the ADR's no-new-aggression seam + D27), 0028 lane-contention re-routed to the harness closeout. Fence green ×2 this session.
- **2026-08-23 (late)** — **WvC-1 code-complete, all 7 items**: T-HEAL-3a winnability inputs (`c5a06c8`), defender spawn-readiness wired + tower half deleted as U-TOWER-superseded (`81ee72f`), S5-CAP empire-scaled cap + defense surge (`7a87df5` → shared kernel `13112e6`), 0035 FU2 closed (veto attempt `4d044be` reverted `4d186d8` after 2 eval-bed regressions — the probe bounce is load-bearing; final = stall-aware give-up clock + engaged-gated stall streaks, agent `0c57c45`), 0026 L8 observed-owner coordination (`0455298`), 0034 D6c renewable-rally bias (`e6aa3ce`), 0028 K3/K4 resolved as-built (ADR rewritten). Ship + live-watch pending.
- **2026-08-23** — **Operator reorder: military first.** Boost pipeline (largest NEW build) demoted behind the military completion waves (WvC-1 correctness+wiring, WvC-2 defensive features) and the P4 re-tune — finish partial machinery + kill the bug farm before feeding it boosts. WvC promoted out of the old Phase-6 into Phase 3; NOW = WvC-1.

- **2026-08-23** — **WS-6 SHIPPED + CLOSED: ADR 0047 live at WFV 29** (msgpack struct-map stream + foreman Plan shrink `5c89f30` — road_network deleted, build_order on-demand; plans ~70% smaller). The LAST format-transition reset paid; additive changes are now reset-free. Live: 2.8% of segment budget mid-rebuild (proj. 12–14% full), named decode FASTER than old bincode. Costs recorded: wasm +71% (48.5% of code limit). Operator constraint recorded: plans are durable state, never recompute-after-reset. ws-6 doc deleted.

- **2026-08-23** — **ADR 0047 → DECIDED**: whole-stream msgpack struct-map, ONE encoding (operator simplicity steer, confirmed by round-2 data: 30.4% of the real 400KB segment budget; RoomPlanData=86% of bytes and shape-stable; real-world named round-trip works; sectioning rejected as unnecessary). WS-6 remaining: the game_loop swap (one WFV bump, batched).

- **2026-08-23** — **Phase 2 (triage) CLOSED**: final sweep phase-tagged every §6 line (P3/P4/P5/P6/WvC/HARNESS/WATCH); 0005 containment ratified as-shipped → Closed; ws-triage doc deleted. WS-6 (0047 benches) is NOW.

- **2026-08-23** — **WS-1 CLOSED: C1–C5 ALL PASS.** The pipeline claimed **W7N47** (dist 4, above-ring, score 0.835) — 8 rooms; RemoteBuildMission constructing. L2 ruled OBVIATED by 0046's bounded-retry machinery. 0046→Live, 0038→Closed. WS-1 doc deleted per lifecycle.

- **2026-08-23** — Triage decisions ratified (operator): 0030 Withdrawn (tempo→0031), 0025a residual documented-mitigated, 0039 P2–P4→harness lane, 0020 S5–S7 kept-scheduled (after Phase 4); **0047 pulled forward as Phase 2.5 (WS-6)**. `search_radius` 1→2 shipped + live-reconciled (wasm `bd6eebcc0f56`, hot swap, pattern proven twice). UNOWNED-4 closed.

- **2026-08-23** — **Wave B SHIPPED to live MMO** (hot swap `0d9524f2668f` per RULING-8; vm_starts 2749, missions persisted, 0 deser). RULING-8 recorded (deploy-to-live batched; B-1 demoted); ADR 0047 drafted (reset-tolerant serialization).

- **2026-08-24 (latest)** — **Phase 4.5 item 4 (parity H0): FIXED** — `is_combat_targetable` in
- **2026-08-24 (item 5 partial)** — drain delivery honesty + heal-premium rebalance shipped
- **2026-08-24 (item 5 COMPLETE)** — the threat/traversal unification cluster is CLOSED both
  sides: sim field = shared build_room_threat_field + M2 (agent `4e68de4`), plateau tie-break
  (decision `71c6e0a`), drain-aware placement (eval `1736ea6`), live mover threat overlay
  (H3/M13: RoomThreatCosts + ThreatOverlayCostSource, rover `fae493b` merge_from) and room-correct
  decide callback (M4/M6) — super `1abcc8f`. 1537 + fence + wasm green.
  (decision `acf3600`, agent `bed5f0e`); the sim-field delegation + M2 + live mover threat layer
  DEFERRED into a drain-rework sub-batch with four precisely-traced findings (form-into-the-nest
  tank bleed is pre-existing and invariant; the drain canary passes on remnant luck). SQ_DEBUG
  trace instrumentation landed. 1537 + fence + wasm green.
  squad_combat.rs: the execution-side structure list now includes neutral constructed walls with
  hits (both the cached arm and the live find fallback), so kernel-chosen wall breaches resolve
  instead of silently dropping. H5/M0 also closed (item-2 batch). Parity report triage updated;
  no native pin (JS types) — live-verify on the next neutral-wall breach. Master replay index
  (all six lenses) generated + delivered.

- **2026-08-24 (later)** — **Phase 4.5 item 2: border crossing — ACHIEVED** (decision `b0b7ea0`,
  agent `1fbff1b`, eval `8dab84d`; root-caused live from the operator's replay observation "one
  creep enters and everything outside the room stalls"): bloc crossing gate + full-roster views
  (parity H5) + fight-room kernel anchoring (the centroid-room V-1 aliasing) + room-gated mover
  anchor + exit-edge pricing + room-local tower assessment + rout-to-rally + state decay + a
  latent twin_room_siege fixture bug. **Every fielding gauntlet rung now KILLS** (L1
  open/choke/choke-multi + border g1×2/g2 — pinned). Replay viewer regenerated + sent; 1537
  workspace + fence + wasm green. Item 1's choke trickle-in tail closed by the same batch.
- **2026-08-24** — **Phase 4.5 item 1: cohesion under focused fire — open-layout bar ACHIEVED**
  (decision `be725c9` + agent `e3660d8` + eval `17d2d74`): four composing EV-kernel fixes
  (deliverable heal, siege risk-currency floor + ×4 uncovered steepener, lockstep healer
  advertising, evidence-gated urgent heal triage). L1-open@T3 → Killed/151 ticks/zero losses
  (was timeout-freeze → 611-tick kill with 5 deaths). Floor pin upgraded; permanent `probe_rung`
  trace instrument; healers-first-sort attempt reverted on the oscillation gate (documented in
  kernel.rs). 1537 workspace + fence + wasm green. Remaining item-1 tail: choke trickle-in.
- **2026-08-23 (later)** — **WS-VAL phase closed out**: MMO hot swap tail-verified clean (new
  `tail.rs --server` reads `.screeps.yaml` directly — no more env-token dance); found work swept
  into §3 **Phase 4.5** (the defect program, queued next per operator), §6 refreshed
  (0010/0041/0025/0023 were stale vs shipped code), §8 BoostQueue row closed.
- **2026-08-23 (late)** — **WS-VAL corpus landed**: engine-exact stronghold gauntlet + border
  gauntlet + boosted self-play lane; boost-blind seam fixed live+sim (shared `effective_output`,
  heal_power un-latched); ultracode parity audit (43 findings; H4/H7/M15/M16/M19 fixed, rest
  triaged in `docs/reviews/live-sim-parity-audit-2026-08-23.md`); pre-existing `screeps-prospector`
  breakage fixed (`Plan.build_order` → `compute_build_order`). 1536 workspace tests + fence + wasm
  green. Honest baseline tables in the ws doc; tactical follow-ups queued (cohesion under fire is
  the binding defect).
- **2026-08-23** — **Wave B CODE-COMPLETE**: D9/D10 landed (`1a85a57` + rover `850a06b` — shared engaged ladder now ONE implementation in rover, wired live; flee uses partial paths); T1/T2 ruled retained-by-design. 15 pins total. The 2026-07-09 review Tier −1 work list is closed; soak pending B-1. World checks 1–3 healthy.

- **2026-08-22 (late)** — Wave B 6/8: D2/D3 safe-mode (`8fa0c60`), D4/D5/D6 roster churn (`be5ce24`), D28 vacuous clear (`b26eba4` + decision/eval submodules). 13 RED-verified pins; fence green; ADR 0027 amended. D9/D10 + 0037 decision remain. WS-1 observation healthy through 3 checks (claim pipeline live, C2 signature absent).

- **2026-08-22** — **WFV 28 DEPLOYED TO LIVE MMO** (`77dc9cc`, wasm `d9b748497e4a`; operator inverted soak order, MMO-first). Loud reset clean: 0 panics, CPU 52→37/140, bucket 10000. `reset.features` one-shot built + fired + verified — live config at compiled-default parity (offense back ON; Wave A fixes in-artifact). Closes UNOWNED-7. Observation window open (C1–C5).
- **2026-08-22** — Completion roadmap (Phases 0–6) recorded in §3; §1 updated to MMO-first.

- **2026-08-22** — **Design/implementation split.** All 56 ADRs rewritten as pure end-state designs; status moved here and to `../implementation/`. Status vocabulary reduced to Decided/Draft/Superseded/Withdrawn (+ note types). Closes CHORE-1 structurally. Adversarial verify caught 4 design-loss regressions and 19 lesser ones, all remediated and re-verified. Rollback tag: `pre-doc-split`.
- **2026-08-22** — Full ADR-corpus reconciliation (56 verified, 29 drifted); this tracker created; rulings 1–7 recorded.
- **2026-08-22** — Repo tie-off: ADR 0046 merged (WFV 28), working tree emptied, all branches/worktrees removed, master + 49 submodule commits pushed, ADR 0044a renumbered, 0038/0042 headers fixed.
- **2026-07-28** — Combat Wave A shipped to MMO (`ab692bd`): D1/D11/D24/D25/D26/D27/R22. CPU 87→16.
- **2026-07-06** — ADR 0040 accepted; WFV 27 to MMO.
