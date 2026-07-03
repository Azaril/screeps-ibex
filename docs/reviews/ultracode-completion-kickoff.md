# Ultracode completion-drive kickoff prompt

> Paste the block below into a fresh **ultracode** session (include the word "ultracode"). It drives **all** remaining ADR-corpus work to completion with **no deferrals**. It is grounded in the 2026-07-02 ADR closeout: every open design question is already decided (see [adr-closeout-2026-07-02.md](adr-closeout-2026-07-02.md) §2), so this session implements — it only stops to ask about the handful of genuinely-undesigned forks flagged at the end.

---

```
ultracode

MISSION
Drive ALL remaining work in the screeps-ibex ADR corpus to completion — no deferrals. This repo just
completed a full reconciliation + a two-phase deploy (WFV 26 is LIVE on MMO; 1227 workspace tests green).
The operator has decided every open design question. Your job is to IMPLEMENT the remaining work in
dependency order, to a clean end-state, keeping everything resumable and verified.

SOURCE OF TRUTH (read these first, in order)
1. docs/reviews/adr-closeout-2026-07-02.md   — the operator's 13 closeout DECISIONS (§2), resumability
   status (§3), and the dependency-ordered remaining-work rollup (§4). THIS IS THE PLAN.
2. docs/reviews/reconciliation-2026-07-01.md  — the living ledger of what already landed (§5 ADR status
   table, §6 backlog). Update it as you complete work.
3. The per-ADR docs in docs/design/ — each stale ADR was patched 2026-07-02 with a Status line, a
   "Closeout decision log", and a Resume-point. TRUST the code + these two review docs over any older
   ADR prose (headers drift — this was the whole finding).
4. docs/guides/engineering-practices.md — the EP-* rules (cite them).

STANDING OPERATOR DIRECTIVES (non-negotiable)
- WFV / serialized-shape bumps are FINE (deploy resets anyway). NEVER defer or half-build to dodge a
  WORLD_FORMAT_VERSION bump. Batch all WFV-bumping work before a deploy so it's one reset.
- CLEAN design, NO tech debt: right crate (shared decision logic in the combat-decision/foreman/etc.
  crates, not the bot binary — so harnesses can test it), root causes not symptoms, delete dead code,
  clear EVERY review MINOR before commit, no docstring/behavior drift.
- VERIFY CODE OVER DOCS. ADR headers were systematically stale; ground every status in a grep/read.
- NO DEFERRALS. For each item: implement it, or STOP and ask the operator a specific design/impl
  question — never silently skip, never write "deferred" without an explicit operator decision.
- No AI attribution in commits. Work on master (rewrite-period override). Deploy only via screeps-pack
  (`cargo run --release --manifest-path screeps-pack/Cargo.toml -- deploy --server mmo`) and NEVER to MMO
  without an explicit operator go-ahead.

STATE SNAPSHOT
- WORLD_FORMAT_VERSION = 26 (game_loop.rs), LIVE on MMO. WFV history this era: 23→24 (REC-001), 24→25
  (REC-009b marker squad-ref), 25→26 (U-TOWER focus-id). Bump from 26 for the next serialized change.
- Submodules are in-tree; commit LEAF-FIRST (combat-decision/combat-eval/combat-agent/rover/foreman)
  then bump the superproject pointer. Full battery each sub-batch: `cargo test --workspace`,
  `cargo build-wasm -p screeps-ibex`, `cargo clippy-wasm -p screeps-ibex` (all must be green/clean).
- Two concurrent workstreams leave uncommitted in-tree files you MUST NOT touch: screeps-ibex/src/
  features.rs (expansion ADR 0038 D9 tuning) and docs/design/0040-* — stage explicit paths, never `-a`.

WORKING METHOD
- Work the closeout §4 rollup TIER BY TIER, top→bottom (free/high-value first). Within a tier, run
  reviewed sub-batches grouped by FILE-LOCALITY (items that share files run as one coherent change, not
  parallel edits that fight). For each sub-batch: implement → full battery → adversarial self-review
  against correctness/design/generalization/OSS-quality → clear findings → leaf-first commit.
- Keep it resumable AS YOU GO: after each item, update the ADR's Status/Resume-point and tick the
  reconciliation artifact. A fresh session must be able to pick up from the docs alone.
- Verify-first discipline: the harvested backlog OVERSTATES open work (the reconciliation + closeout both
  found many "open" items already shipped). Before building an item, grep to confirm it's genuinely
  unbuilt; if already done, record NOT-A-DEFECT with file:line and move on — do not fabricate a change.

THE PLAN (from closeout §4 — dependency ordered; decisions already made, see closeout §2)

TIER 0 — free, do first:
  • 0011 Step-0 spawn-executor quick-wins (S): engine-true renew energy decrement; move renew behind the
    priority check (fix the P4 renew-before-CRITICAL inversion); debug_assert!(priority.is_finite());
    comparator unit test. DO NOT reverse the spawnsystem comparator (verified-correct, reconciliation §7).
  • 0009 D1 residual (S, Q7): warn-once + seg-57 counter on fingerprint-mismatch restart (accept the
    shipped Failed{attempts}+escalation design; completed plans persist → CPU-waste only).
  • 0002 (S): verify the segment-fullness fail-loud-on-overflow half is complete.

TIER 1 — planner + economy robustness (host, low live risk):
  • 0009a provability fallback (S, Q8): adaptive beam-widen/cap-lift on 0009c ESCALATION_BEAMS.
  • 0003 MissionResult::Wait/Idle for ECONOMY missions (M, Q3): one-tick room/visibility loss parks
    miningoutpost/defend instead of tearing down a campaign. (NOT the FSM rewrite — that's abandoned.)
  • 0009b planner (Q8/Q9): §7 ground-truth bench → §4.3/§4.6 cost terms → §3/§5/§6 scoring re-weight →
    §8-step-6 (WFV bump from LIVE 26 + recalibrate claim.rs plan_score_weight/max_score_delta AFTER
    verifying claim.rs is still the sole plan.score.total consumer post-ADR-0038) → §8-step-7 sweep.
  • 0009a/0009b scorer-quality remainder (L): UpkeepScoreLayer weights, compactness, placement-driven
    hub-approach-tile reservation — CROSS-CHECK 0009c ownership first to avoid duplicating shipped work.

TIER 2 — operator-selected build-outs:
  • 0007 TransferSnapshot two-phase hauler matcher (M, Q5 — BUILD; pure/replay-diffable; item-4
    route-sizing reuses ADR 0038's committed route machinery per Q6).
  • 0009 D3 (XL): RoomGraph → exit-affinity → InterRoomRoadLayer. D3.5 DROPPED (ADR 0038 owns
    route-distance per Q6). 0009c already covers hub roads — build only inter-room/remote.
  • 0011 economic orchestrator (XL, Q10): D2 throughput/energy budget → D5 cross-room assist + G3
    incubation → D7 starvation cure. DROP the combat-cohesion pieces (auction lifecycle owns them).
  • 0015 (XL, Q11): build the full generic screeps-testkit (host crate) + Seam Contract Registry + shared
    fakes + ibex_invariant! macro (double-intent/capacity/is_finite; dangling-ref already covered) +
    proptest/insta/cargo-fuzz, AND migrate the combat-eval/IntentRecorder/determinism-fence stack onto it.
  • 0016 (L, Q13): full Glance HUD redesign (decompose visualization.rs, L1/L2/L3) + FIX the render-
    corruption bug — operator hint: it's world/RoomVisual visuals, look there first.

TIER 3 — the empire executive layer (Q12, FULL — build in this order):
  • 0010 boost-lab-factory pipeline (L0/L1 supply also unblocks ADR 0041's dark consumer) →
    0012 market/risk → 0013 power economy/power-creeps → 0014 empire strategy/posture (arbitration
    CAPSTONE — this is where you'll most likely need operator design input) → 0017 threat-aware
    expansion lifecycle → 0018 SK exploitation.

TIER 4 — flagship combat + tuning (interleave as unblocked):
  • 0041 combat boost layer (Accepted, §8 decided): P0 dark kernel (BoostTier into the pricing seams +
    BOOST_LADDER in optimize_composition; BoostSupply empty → T0 wins → byte-identical, WFV-neutral) →
    P1 populate EconomySnapshot.available_boosts + supply clamp → P2 persisted CombatBodySpec.boost
    (WFV 26→27) → P3 AwaitBoost + boostCreep lifecycle (gated on 0010 L0/L1 stocking compounds) → P4
    re-sweep. Full T0→T3 uniform-per-body ladder; cost via a pure trust-gated mineral_value_e kernel
    (market price if trustworthy else a cost-of-production floor else a constant); offense-first.
  • [TUNE] eval sweeps (needs-compute, the #[ignore]d beds): 0031a Tier-2 weapon-archetype in the EV
    search (highest-priority composition follow-up) + tunables; 0026 §9.10 L6c; 0019 St.4 bed.
  • [OP/soak] 0035 FU1/FU2, 0029 §10 W9N8 re-soak, 0033 Docker soak — needs live-behavior evidence;
    coordinate a soak with the operator, then land the give-up/scout-first pipeline.

STOP-AND-ASK ONLY FOR THESE GENUINE DESIGN FORKS (do not defer them — surface a specific question):
  • 0014 empire posture/arbitration policy — how competing goals (expand vs militarize vs bank vs boost)
    are weighed (the capstone; needs an explicit objective/priority model).
  • 0012 market risk model — trade thresholds, the compound valuation feeding ADR 0041's cost kernel,
    and what "trustworthy market price" means concretely (order-depth/volume/band checks).
  • 0013 power strategy — whether/when to run power creeps + PWR ability priorities.
  • 0020 §11 S5 blob auction — needs the R7 cross-goal EV currency defined first.
  • 0030 §9 EngagementTempo, 0031 Tier-2 archetype design, 0025a §2 object-anomaly root-cause,
    budget-free emit_requirement assess redesign (optimizer_ceiling_budget is the winnability seam —
    a calibration change, design it before touching it).
When you reach one of these, STOP, present the fork with options + a recommendation (like the ADR 0041
interview), get the decision, record it in the ADR's decision log, then implement — never skip it.

DELIVERABLE CADENCE
Report after each tier (or each XL item): what landed (commit SHAs), what the battery showed, which
ADRs you patched, and the next item. Keep docs/reviews/reconciliation-2026-07-01.md and each ADR's
Resume-point current so any interruption is resumable.

Begin with Tier 0. Verify each item is genuinely unbuilt before building it.
```

---

## For the operator: what this prompt will do

- **Implements Tiers 0–4** of the closeout rollup in dependency order, reviewed and committed leaf-first, keeping the ADRs + reconciliation ledger resumable as it goes.
- **Won't re-ask** the 13 questions you just decided (they're baked in) — but **will stop and interview you** on the ~7 genuinely-undesigned forks (chiefly the empire capstone 0014 arbitration policy, the 0012 market/valuation model that also feeds 0041's cost kernel, 0013 power strategy, and the 0020 S5 EV-currency + a few combat design items).
- **Respects the two live workstreams' uncommitted files** (features.rs, 0040) and never deploys to MMO without your explicit go-ahead.

This is a large multi-session drive (the empire tier + full testkit + HUD + boost layer are each XL). The prompt is built so you can paste it once and let it run tier by tier, stepping in only at the design forks.
