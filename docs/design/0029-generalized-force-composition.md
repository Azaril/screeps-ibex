# ADR 0029 — Generalized force composition (one oracle, custom only where it pays)

- **Status:** Accepted (core landed) — player-room raid generalization is a flagged follow-up (§7)
- **Date:** 2026-06-27
- **Extends:** [0026 §9](0026-*.md) (the doctrine registry), [0020 §12](0020-ev-adaptive-blob-combat.md) (the force-sizing oracle), [0028](0028-lifecycle-harness.md) (the lifecycle harness that proves it)
- **Supersedes:** the `GarrisonDefense` solo/duo/quad bucket selection (the former `DefenseEscalation::from_threat`)

## 1. The question (operator, 2026-06-27)

> Is the sizing solvable with the part auction or using the actual size oracle? Now that we support
> dynamic blob sizes, we should only need custom spawning for specific objective types where there's an
> optimal mix (or no attackers/defenders, etc.). The solo/duo distinction is maybe not as helpful now.
> Can this be generalized? Should it be driven by the strategy layer?

## 2. Answer (the decision)

**Yes — and the mechanism already exists.** The size **oracle** (`force_sizing`: `assess`/`clear_force` →
`RequiredForce` → `SquadComposition::sized_for`) is the generalization. The doctrine registry (ADR 0026 §9)
already runs it for nearly every objective; the **only** real anomaly was `GarrisonDefense`, which wrapped the
oracle in **solo/duo/quad buckets** keyed on a hard threshold. That straddle — not the oracle — was the W9N8
bug. We **delete the buckets** and let the continuous oracle size the defender off a single base, exactly like
offense. After this:

- **The oracle sizes the blob for every "size to the opposing force" objective** — offense raids AND defense.
  The member count *emerges* from the required force; there is no discrete shape to straddle.
- **Custom compositions survive only where a blob is genuinely wrong** — an *optimal fixed mix* or a
  *behavioral* requirement the oracle doesn't model (kiting, no-hold harass). See §5.
- **Composition is driven by the OBJECTIVE + the ORACLE, not the strategy layer.** The doctrine classifier
  (objective class) picks *which* template/role-mix; the oracle sizes it; the strategy layer is the orthogonal
  **tactics** twin (how the fielded blob fights). See §6.
- **The part-auction (task #28) is a future refinement, not a prerequisite.** The oracle already solves the
  continuous *blob* (N ranged + M heal, etc.). The auction earns its keep only when the *optimal mix is not a
  blob* — trading ranged↔dismantle↔tough↔heal parts by EV. Until a measured case needs it, the oracle is enough
  (YAGNI). See §8.

## 3. The doctrine landscape after this ADR

| Doctrine | Objective | Registry | Sizing | Why |
|---|---|---|---|---|
| `NpcCore` | KillImmuneStructure | offense | **oracle** (`assess`) | size ranged kill-parts to core HP + tower drain |
| `SiegeBreach` | DismantleStructure | offense | **oracle** (`assess`) | size dismantlers to the wall ring + heal to towers |
| `PlayerRaid` | ClearCreeps | offense | **oracle** (`clear_force`) | size ranged+heal to out-power the room's creeps |
| `GarrisonDefense` | ClearCreeps | **defense** | **oracle** (`clear_force`) ⟵ *was buckets* | size ranged+heal to out-power+out-heal the attacker |
| `SkSuppression` | Suppress | sk | **custom kiting duo** | a *behavioral* mix (kite + out-heal one keeper); not a blob |
| `HarassRemote` | Harass | offense | **fixed solo** | deny an *undefended* remote; one cheap creep, no fight to size |

Three of six now size through the same `clear_force` path; two more through `assess`. Only **two** are
non-oracle, and both for a defensible reason (§5).

## 4. The W9N8 root cause this fixes (the live bug)

W9N8 was a **defense** churn (the bot defending its own contested room), not offense. `GarrisonDefense::plan`
chose a shape by a hard step (`f.dps > 60 || f.heal > 20 || f.count >= 2 || f.boosted` → duo, else solo) on
**live per-tick threat** (`ThreatAssessmentSystem` re-sums hostile part-HP every tick). A defender stepping in/out
flipped `count` across 2; one ATTACK part swung `dps` across 60 — so the committed slot count flapped **1↔2 every
defense scan**. The objective's force is overwritten each scan and the manager re-reads `comp.slots` per tick, so
on a `requested = 1` tick the lone slot-0 creep satisfied the rally gate, departed into the defended room, and was
**wiped** → re-field → churn (live: `RALLY present=1/1 (holding)` cycling with `RETIRE reason=Wiped`).

Compounding it, the defense scan passed `member_energy: 0`, so `sized_for` returned `None` and `GarrisonDefense`
fell back to the **bare template** — the parts were never sized at all.

**Fix (landed):**
- `GarrisonDefense::plan` sizes a single `quad_ranged` base via `clear_force` → `sized_for` — **no buckets**. A
  continuous size cannot straddle; the defender floors at the 4-member base (over-spend a trivial threat, the
  safe side — you can never under-defend an owned room) and grows monotonically. (`doctrine.rs`)
- The defense scan passes `member_energy = ` the defended room's `energy_capacity_available()`, so the oracle
  actually sizes. (`war.rs`)
- Test `garrison_defense_sizes_continuously_no_straddle` asserts the floor + monotonicity (replaces the bucket
  test). The pre-existing rally-gate fix (count present ≥ requested, [[0028]]) tolerates any residual smooth
  drift.

## 5. When custom (non-oracle) composition is justified

A doctrine should size through the oracle **unless** one of these holds — the test for "is a custom template
warranted":

1. **Behavioral mix, not a power blob.** `SkSuppression` kites a single keeper and out-heals its fixed DPS — a
   2-role duo whose *positioning discipline* matters more than raw size. Sizing it like a blob would field a mob
   that can't kite. (The keeper is a *farmable hazard*, not a scaling enemy.)
2. **No fight to size.** `HarassRemote` denies an *undefended* remote; the binding question is "is it defended?"
   (a gate), not "how big?" (a size). One cheap creep, fielded or not.
3. **A measured optimal mix the blob model misses** — e.g. a tough-tanked dismantle wedge where the EV-optimal
   ratio of tough:work:heal isn't what `sized_for`'s per-role growth produces. **This is the part-auction's
   domain (§8); none is in flight today.**

Anything else — "this enemy is bigger/smaller", "this room has towers" — is a *sizing* question and belongs to
the oracle, not a new template.

## 6. Why composition is NOT strategy-layer-driven

The two registries are deliberate twins (ADR 0026): the **doctrine** registry answers *what force* (objective →
template + sizing), the **strategy** registry answers *how it fights* (weights for the position-utility kernel,
ADR 0019/0020). They compose, they don't merge:

- **Composition** is a function of the *objective* and the *measured opposing force* — deterministic, oracle-sized,
  identical in the bot and the sim (the parity ADR 0026 requires). Routing it through the tactical strategy would
  couple "how many ranged parts" to "how aggressively to position", which are independent.
- The **objective class** already selects the doctrine (so a player room → `PlayerRaid`, an owned room →
  `GarrisonDefense`). That selection *is* the only "strategy-like" composition decision, and it lives in the
  doctrine classifier where the sim can replay it.

So: keep composition on the doctrine+oracle axis; keep the strategy layer for tactics. If a future need arises to
pick a *role mix* by strategy (e.g. "siege vs. raze this base"), that is a new *doctrine activator input*, not a
move of sizing into the strategy layer.

## 7. The SK and player-room fixes (landed this ADR)

- **SK farm spawn contention (zero-SK-farm, live `W6N4 present=1/3`).** The SK objective was upserted at
  `OBJECTIVE_PRIORITY_LOW`, which `spawn_priority_for` maps to `SPAWN_MEDIUM` — *below* economy — so its forming
  slots lost every spawn lane to CRITICAL miners / HIGH haulers and never completed the duo. Raised to
  `OBJECTIVE_PRIORITY_MEDIUM` → forming slots map to `SPAWN_HIGH` and win lanes; CRITICAL miners still out-rank
  it, so income is protected. (`sourcekeeperfarm.rs`)
- **Player-room under-sizing (offense calibration, latent — not a reported live symptom).** `ResourceDenial`
  mapped to `Harass` → `HarassRemote` (fixed solo, no gate), so a defended hostile *player* room was fed a doomed
  lone harasser. **Landed (safe half):** only harass towerless rooms — a solo can deny an undefended remote but is
  just fed to a tower. **Follow-up (held for review):** route `ResourceDenial` through the sized+gated `PlayerRaid`
  (DoctrineObjective `ClearCreeps`), populating `candidate.defense` (towers ranged to a chosen assault tile) so the
  winnability + ROI gate sizes a real raid or defers — the same path AttackFlag already uses (`war.rs:659-672`).
  This is a *notable offense-behavior change* (player rooms get sized raids or get deferred), so it ships behind
  operator review rather than in this batch.

## 8. The part-auction's place (task #28, future)

The oracle answers "how big a blob". An **archetype/part auction** answers "what *mix* of parts, valued in a
common EV currency". It is strictly richer and strictly more expensive (an EV currency + per-archetype valuation +
sim validation). It is the right tool when §5.3 bites — a measured objective where the EV-optimal part mix is not a
ranged+heal blob. **Decision: do not build it speculatively.** The oracle covers every objective we field today;
revisit the auction when a concrete objective demonstrably loses value to blob-only sizing.

## 9. Decisions

- **D1.** Generalize `GarrisonDefense` onto the continuous oracle; delete the solo/duo/quad buckets. *(landed)*
- **D2.** Defense sizing budget from `quad_ranged` (sizes a strong threat) but FLOOR from `duo_attack_heal` (2) —
  the floor is decoupled from the budget so a trivial threat doesn't over-spawn to 4. *(landed; the original
  4-member floor over-loaded the spawn lanes — corrected in §11 FIX B)*
- **D3.** Defense scan sizes to the defended room's spawn capacity, not 0. *(landed)*
- **D4.** Composition stays on the doctrine+oracle axis; the strategy layer stays tactics. *(decision)*
- **D5.** Custom templates only for behavioral mixes / no-fight / measured optimal mixes (§5); SkSuppression and
  HarassRemote qualify, nothing else does today. *(decision)*
- **D6.** SK objective → `OBJECTIVE_PRIORITY_MEDIUM` so it forms (SPAWN_HIGH) without starving CRITICAL economy.
  *(landed)*
- **D7.** ResourceDenial: only solo-harass towerless rooms now; route defended rooms through sized+gated
  `PlayerRaid` as a reviewed follow-up. *(half landed, half held)*
- **D8.** Defer the part-auction until a measured objective needs a non-blob mix. *(decision)*
- **D9.** The rally-until-full gate is OFFENSE-only: defenders (`ObjectiveKind::Defend`) deploy immediately with
  whatever has spawned (§11 FIX A). *(landed)*
- **D10.** The `MAX_FORMING_SQUADS` pace counts only OFFENSE forming squads; defense is exempt (§11 FIX C). *(landed)*
- **D11.** Renew-while-forming is moot once defense deploys immediately + offense forming is paced (D9/D10); left in
  place (harmless) but not relied on (§11 FIX D). *(decision)*

## 10. Remaining work

1. **Harness proof (§4):** a `run_defended_lifecycle` test ([[0028]]) — an oracle-sized force, *forming + moving*,
   vs a *defended* core — to close the seam between `SizingWins` (oracle-sized, pre-placed, ~99%) and
   `run_lifecycle` (formed, undefended). Discriminates "form/travel degrades a sized force" from "the live
   under-sizing was the whole story".
2. **D7 follow-up:** the sized+gated `PlayerRaid` routing for `ResourceDenial` (operator review — offense behavior).
3. **Live re-soak (private server):** ✅ W9N8 oscillation fixed (stable 4-slot request) + renew fires — but the
   re-soak EXPOSED the forming-completion wall (§11); FIX A/B/C landed + re-deployed, re-verifying.

## 11. Forming-completion under contention (the generalization's second-order effect — landed)

The §4 generalization fixed the W9N8 size oscillation, but the live re-soak then showed a NEW wall: four squads
forming at once, every one stuck at N-1/4 forever, none departing (defenders never deploy, offense never attacks).
A parallel investigation ranked the causes:

- **#1 (dominant) — the rally-until-full gate was applied to DEFENSE.** `squad_ready_to_depart` holds a squad at
  home until `present >= requested` — correct for an OFFENSE bloc crossing into a contested room, but WRONG for a
  defender of an owned room under attack: it sits at home massing a 4th member that contention never delivers while
  the room burns. The *direct, sufficient* cause of "stuck at N-1, never departs".
- **#2 — the 4-member defense floor over-loaded the lanes.** §4's `quad_ranged` floor × N contested rooms ≈ 16
  concurrent HIGH spawns saturate throughput, so the missing member never spawns (D2 was the original mistake).
- **#3 — the forming cap gated new claims, not in-flight stock** (so concurrent forming was unbounded).
- **#4 — renew was a *symptom*** (it needs a free spawn; under contention spawns are busy, so it can't fire — the
  `ttl` declines despite the request).

**Fixes (landed), ordered by leverage:**
- **FIX A (D9) — rally gate objective-kind-aware:** defenders deploy immediately. *Subsumes the visible lockup + most
  of renew.* (`squad_manager.rs` `compute_squad_orders`)
- **FIX B (D2) — decouple the defense floor from the budget:** quad budget (sizes a strong threat) + duo floor (no
  trivial over-spawn). *Subsumes the lane saturation.* (`doctrine.rs` `GarrisonDefense::plan`)
- **FIX C (D10) — count only OFFENSE forming** toward `MAX_FORMING_SQUADS`: defense exempt; offense serializes at ≤2.
- **FIX D (D11) — renew demoted:** A+B make it moot; left in place, harmless.

**Recommended follow-up:** extend `run_forming` ([[0028]]) to model MULTI-squad lane contention (today it is
single-squad) and prove A+B+C reproduce-then-fix the N-1 stall offline — the operator's tune-offline-not-live
preference (the second-order analogue of §10 #1).
