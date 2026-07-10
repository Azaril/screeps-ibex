# ADR 0044 A3 — All-Sinks-EV: Scoping Document

**Status:** Decision-ready (scoping). Synthesized from three independent investigation briefs and one adversarial code review (all claims below re-verified against the code at the cited file:line).
**Date:** 2026-07-06
**Decision owner:** operator

---

## TL;DR

A3 has two candidate architectures, and the analysis converges decisively:

- **Architecture 1 (CONTAINER-AS-EV-DEPOSIT)** is SMALL, is *already the sim default*, and closes a live-vs-sim divergence the code itself flags as a stub (`room_transfer.rs:372` — *"Non-refill deposits keep their tier for now (A8)"*). It reaches exactly **one** consumer class: the **controller container** (upgraders). Build and repair have no container to reprice.
- **Architecture 2 (DELIVER-TO-CONSUMER / creep-as-sink)** is LARGE, multi-crate, multi-week, and fights three ratified subsystems (the body-EV kernel, the static-target mover, the determinism fence) to re-implement a buffer the adjacent container already provides for free.

The name "all-sinks-EV" is a **misnomer** — only the controller container is a container-backed consumer buffer.

There are **two distinct live/sim parity gaps** in scope, and the framing correction from review matters: the **larger correctness fix is Defect 2** (the Use-lane admission + downgrade veto are defined live but test-only, so live consumers *never* stop draining under a refill deficit — a behavior *inversion*), and the **narrower optimization is Defect 1** (the controller-container deposit is priced by a coarse tier ladder instead of the EV buffer curve). Defect 2 is the correctness fix; Defect 1 is the tie-winning optimization layered on top.

**Recommendation: GO on Architecture 1 (both defects, sequenced), gated behind a cheap sim validation arm that reproduces *both* defects in its control; NO-GO on Architecture 2.**

---

## 1. Problem statement — the concrete inefficiency of the current withdraw+floor-gate model

The intended model (as designed and as the sim runs it): consumer creeps (upgraders/builders/repairers) are *second-class* Use-lane withdrawers, floor-gated so that under a refill deficit they stop draining the container the refill hauler needs. High-EV consumer work that is genuinely worth more than a marginal refill should be able to **out-compete refill for a hauler** by pricing its container-buffer deposit at the downstream EV — *and*, on the withdraw side, its draw should be admitted or gated by whether its sink bid clears the room's opportunity floor.

The live bot does neither. Two independent, narrow defects, of which **Defect 2 is the larger correctness problem** despite being listed second here for pricing continuity.

### Defect 1 — the controller container deposit is priced by a coarse tier ladder, not the EV buffer curve (the A3 "repricing" core)

The live controller container deposit is set by a **coarse tier ladder**:

- `demand::controller_container_deposit_priority` (`demand.rs:150-157`): energy `<75%` → `Low`, else `None`.
- registered via `new_tier` (`room_transfer.rs:378`); `room_transfer.rs:372` says explicitly: *"Non-refill deposits (containers/storage) keep their tier for now (A8)."*
- `new_tier` → `tier_to_bid` → `BID_TIER_LOW=1250` / `BID_TIER_NONE=STORAGE_BID` (`sink_economics.rs:523-527`).

So live, the controller container bids a **flat 1250** (when <75% energy) or storage-par (when ≥75%). The **sim** prices the *same* container at `buffer_deposit_bid(up_bid, free, cap)` (`market.rs:313-320`).

**The magnitude is fill-dependent, not a flat percentage.** This is the load-bearing correction from review. `buffer_deposit_bid(base, free, cap) = base·(free/cap)²` (`sink_economics.rs:443-449`, verified — a *quadratic* in free capacity, not a flat bid). It reaches its ceiling (`V_UPGRADE`=2000, or 3000 near level-up with `+UPGRADE_STEP_PREMIUM`) **only when the container is empty**, and falls quadratically as it fills:

| Container fullness | sim EV deposit bid (normal, base 2000) | sim EV (near level-up, base 3000) | live flat bid |
|---|---|---|---|
| empty (free = cap) | 2000 | 3000 | 1250 |
| 25% full (free = 0.75 cap) | 1125 | 1688 | 1250 |
| ~21% full (crossover, normal) | ≈1250 | — | 1250 |
| ~13% full (crossover, near level-up) | — | ≈1250 | 1250 |
| 50% full (free = 0.5 cap) | 500 | 750 | 1250 |

So the live flat-1250 is *below* the sim EV only while the container is roughly **<21% full** (normal) or **<13% full** (near level-up); across the rest of the fill range the live flat bid is actually *higher* than the sim EV. The honest characterization:

- **Live** uses a *flat step* (1250 below 75% energy, par above).
- **Sim** uses a *quadratic* that crosses the live 1250 line at ≈79% free (normal) / ≈87% free (near level-up).

The mis-allocation is real, but it is **a bid-shape mismatch that matters most for a near-empty container the upgrader is about to starve on** — precisely the "idle upgrader beside an empty controller container" case — not a uniform 37–58% uplift. The earlier draft's "37–58% under-pricing" figure was the *empty-container* delta stated as if it were uniform; it holds only at `free ≈ cap`. Do not quote it as a blanket number.

**Concrete mis-allocation (near-empty case, where the defect actually bites):** an idle upgrader sits beside an empty controller container. The empty container's true EV is 2000 (normal) / 3000 (near level-up); live it bids 1250. Spawn/extension refill deposits bid the derived `refill_bid` (par…ROI-cap). Storage bids par. The empty container *should* win against par storage and against a nearly-satisfied refill lane, but at 1250 it **loses ties it should win** — the density-ranked `market_pass` kernel routes the hauler elsewhere, the container stays empty, and the upgrader idles/harvests while controller progress worth 2000–3000 is deferred.

### Defect 2 — Use-lane admission + downgrade veto are defined live but never called (the larger correctness fix)

`market_adapter::admit_use_withdraw` / `admit_repair` → `econ::admit_use_withdraw(sink_bid, floor)` = `sink_bid >= floor` (`sink_economics.rs:471-473`) exist, but **every live reference is `#[cfg(test)]`** — the two public wrappers (`market_adapter.rs:132/139`) are never called from a production path (verified by grep; all call-sites are test-gated). The consumer pickup selection (`haulbehavior.rs:46`) and repair selection (`build.rs:64-80`) **never consult the floor**.

The **sim** wires the gate live in the consumer loop: `runner.rs:1376, 1528` (`rt.veto || admit_use_withdraw(rt.upgrade_sink_bid(world), rt.floor)`) and `runner.rs:1598` (build/repair), with the `downgrade_veto` bypass OR'd in.

**Why this is the larger fix.** The withdraw side is *already EV-gated in design*: `admit_use_withdraw(sink_bid, floor)` **is** the mechanism by which a consumer competes on EV — a consumer whose sink bid clears the room's opportunity floor is admitted, one whose bid falls below it is shed. The floor is not merely defensive; it *is* the consumer's EV gate. Because it is unwired live, the live bot exhibits the **opposite** of the intended second-class behavior: under a refill deficit, upgraders/builders/repairers **keep pulling** from their container regardless of the floor. This is a behavior *inversion*, not a missing optimization — the more consequential of the two defects, and largely independent of Defect 1's pricing.

### Note on the two upgrade prices (do not merge them)

The two phases touch **different quantities**, and conflating them is a real trap:

- **Deposit competition (Defect 1)** uses the *fullness-scaled* `buffer_deposit_bid(up_bid, free, cap)` — a near-full container should *not* pull a hauler, because its marginal energy just sits.
- **Withdraw admission (Defect 2)** gates on the *raw, unscaled* `admit_use_withdraw(upgrade_sink_bid, floor)`, where `upgrade_sink_bid` returns the raw `upgrade_bid` (2000/3000) with **no** buffer scaling (`market.rs:283-285`, verified).

These are deliberately different: the raw bid gates the upgrader's own draw (a near-full container must still *admit* its upgrader's withdraw, or the upgrader starves beside a full container), while the scaled bid governs whether a *hauler* is worth routing to fill it. **Phase 1 reprices the scaled deposit bid; Phase 2 wires the raw admission bid. Do not use the buffer-scaled bid at the admission seam.**

### Skeptical check — is the current model actually fine?

**No — but the value is narrow and specific, not sweeping.** The room does not hard-starve today because the controller container *is* a registered Haul deposit (`demand.rs:310-322`; `room_transfer.rs:279-283`) and haulers *do* refill it at Low/None tier. That partial solution **masks** both defects: energy trickles to the controller when nothing else is Low-or-higher, but (Defect 1) a near-empty high-EV container cannot outbid par storage / a satisfied refill lane because its deposit is mispriced, and (Defect 2) consumers never shed their draws under deficit. The `buffer_deposit_bid` quadratic and the `admit_use_withdraw` gate that together implement the intended behavior are applied **sim-only**. So A3 is real, but it is a *parity + repricing* fix on one container plus a withdraw-gate wiring, not an architectural gap.

---

## 2. Architecture 1 — CONTAINER-AS-EV-DEPOSIT (small)

**Idea:** price the consumer *container* at its EV buffer bid so it competes with refill in the single market (Defect 1), and wire the already-written withdraw admission live (Defect 2), mirroring what the sim already does by default.

### Scope
**Phase 1 (Defect 1 — reprice deposit).** Extend the `is_refill` EV branch in `execute_demands` (`room_transfer.rs:367-380`) so the controller container deposit is priced by `buffer_deposit_bid(upgrade_bid(near_level_up), free, CONTAINER_CAPACITY)` instead of `new_tier`/`tier_to_bid`. The mission already has the controller level/progress data to compute `upgrade_bid` and `near_level_up`, and routes it through the existing numeric `TransferDepositRequest::new(target, res, bid, …)` path — the *same* path refill already uses (`room_transfer.rs:375-377`).

- **~1 file** (`room_transfer.rs`), reusing existing types and the existing numeric registration path.
- **No new `TransferTarget`/`SinkKey` variant**, no body change, no mover change.
- **No WFV bump** — the numeric `new()` path carries no new serialized field (the request shape is unchanged; only the bid *value* differs).

**Phase 2 (Defect 2 — wire admission).** Wire the already-written `admit_use_withdraw` / `admit_repair` + `downgrade_veto` into the live consumer pickup / repair selection (`haulbehavior.rs:46`, `build.rs:64-80` repair selection), mirroring `runner.rs:1376/1528/1598`, gating on the *raw* `upgrade_sink_bid` / `repair_bid` against the published floor. This completes the intended second-class-consumer behavior live and is the larger correctness fix.

- **Behavior-only**, provided it consults the already-published live floor (`transfersystem.rs:1637-1668`) and stores no new serialized state. **Confirm no WFV bump on Phase 2** — the gate is a pure admission check at selection time; if an implementation stores a floor snapshot inside a serialized `TransferWithdrawTicket`/state-machine node it *would* bump, so the implementation must keep it behavior-only.

The sim already computes exactly these shapes (`market.rs:313-320` for the deposit, `runner.rs:1376` for admission), so both phases are **live-parity patches**, not new design. Risk is parity verification, not modeling.

### What it covers
- **Upgraders (controller container): fully.** Phase 1: a near-empty container bids up to ~2000/3000 and competes in the same market against refill and par storage. Phase 2: under a deep refill deficit the upgrader's own draw is shed unless its sink clears the floor (or the downgrade veto fires). This is the ADR P3-vision EV behavior and is **the 80/20** — the controller container is the only real haul-deposit consumer buffer.

### What it misses
- **Builders — NO container.** Construction sites are not haul targets; `TransferTarget` (`transfersystem.rs:93-108`) has **no `ConstructionSite` variant**. Builders roam and deliver from their own CARRY (`build.rs:71/87-97`). Nothing to reprice.
- **Repairers — NO container.** Repair targets are not haul targets either. Repairers repair from their own CARRY; `admit_repair` is a floor gate on the *repair action*, not a deposit.
- **A residual Defect-2 gap even after Phase 2 (out of scope, flagged honestly):** Phase 2 as scoped names the *repair* selection (`build.rs:64-80`) and the hauler pickup (`haulbehavior.rs:46`). But the builder's own *self-fetch* pickup — `get_new_pickup_state_fill_resource` at `build.rs:87-97` and `:137-146` — uses `TransferPriorityFlags::ALL` with **no floor consult** (verified). So even after Phase 2, **builders remain floor-unaware and keep self-fetching under deficit** unless that pickup path is also gated. This is an acknowledged out-of-scope gap for build/repair consumers; do not claim Phase 2 fully closes the consumer inversion. Closing it for builders would require gating the build self-fetch pickup on the floor as well (a small additional patch, still Architecture-1-shaped since builders have no container — it is admission-side only), OR the full Architecture 2 (making them haul sinks). Recommend gating the build self-fetch pickup in Phase 2 for completeness; it is the same admission mechanism.

Build/repair EV is otherwise already represented: spawn-side via `build_bid`/`repair_bid`, and — once the pickups are floor-gated — on the withdraw side via the opportunity floor. Making them true *haul sinks* is Architecture 2.

---

## 3. Architecture 2 — DELIVER-TO-CONSUMER (large: creep-as-sink)

**Idea:** make the consumer creep itself a first-class haul deposit, so a hauler delivers *into the creep's store* and a high-EV build/upgrade/repair outbids refill for hauler attention directly. This is the only *literal* way to reach the container-less build/repair consumers.

### Scope (why it is architecture-breaking)

**New sink identity threaded through everything.** The live deposit set is built exclusively from `TransferTarget` structure nodes (`transfersystem.rs:1919-1961`). `TransferTarget` (`transfersystem.rs:93-152`) has no `Creep` variant. Adding `TransferTarget::Creep(RemoteObjectId<Creep>)` means threading it through the entire impl surface — `is_valid` (:142), `local_pos`/`pos` (:161/:182), `withdraw_resource_amount`/`creep_transfer_resource_amount` (:242/:292), the `ConvertSaveload` serialization inside every in-flight `TransferDepositTicket`, `source_floor_milli`, `target_sort_key`, the `same_structure` self-withdraw guard (`transfersystem.rs:2022-2024`) — plus every `match` on `TransferTarget` in the codebase (dozens) grows an arm. Sim-side, `SinkKey` (`baseline.rs:116-121`, all positional/index identities) needs a new `Consumer(u32)` variant threaded through `is_fungible_pool_member`, the K1 `deposits()` mapping, `select_delivery_masked`, and the booking `BTreeMap<SinkKey,_>`.

**The position problem (the killer).** `target.local_pos()` is read once per tick to price the pickup→sink leg. A creep's position **changes every tick** (upgraders shuffle within range 5; builders walk to the site). The mover does **not** support a moving sink: `Mover::travel_ticks(from, to: Position, …)` takes a *fixed tile*; `AnalyticMover::trace` asserts same-room and memoizes by fixed `(from,to)` `TraceKey` (`movement.rs:54-68, 127-135`). There is no primitive for "route to a creep that will have moved by arrival." Pricing a haul to a moving consumer is a pursuit problem the pathing layer explicitly does not model — and the memory note *No one-off pathfinding algorithms* forbids inventing one in the feature layer. In practice you price to the creep's current tile and accept staleness, reintroducing the creep-anchored-radius deadlock `upgrade.rs:44-48` already documents.

### Body / CPU / determinism cost

- **Body recomposition.** Today's upgrader is `[W,C,M,M]+N*[W]` (`upgrade.rs:28`) — one CARRY, a stationary drain that sips from the adjacent container. Making the creep the deposit means it must buffer a delivery: a 15-WORK upgrader burns 15 e/t but holds 50 → runs dry in ~3 ticks. Decoupling requires scaling CARRY toward `WORK·burn·interval/50` — several extra CARRY per upgrader at ~50e each.

  *Correction from review (category-error guard):* the "zero throughput gain" claim is true **only for the upgrader**, because it has an adjacent container that already provides the buffer for free — extra CARRY there buys nothing. For a **moving builder with no adjacent container** (the one case Architecture 2 uniquely serves), there *is* no buffer, so extra CARRY *does* buy throughput (fewer self-fetch round-trips). So the honest framing is: Arch-2's CARRY cost is pure waste for upgraders (which Arch-1 already serves), and buys real-but-modest throughput for builders — but the builder case still dies on the mover/determinism/serialization costs below, not on this efficiency argument. Do not use the upgrader's zero-gain to argue against the build case; they are different cases.

- **Fights the spawn-EV kernel.** `role_w_milli` prices the upgrader's `w` on WORK only (`market.rs:611-613`). Extra CARRY raises `body_cost` (the ROI denominator, `sink_economics.rs:239`) without raising `w`, so `deficit_priced_pick` ranks these bodies *worse*. You'd have to re-model the worker rate to credit "delivery decoupling" — not in the ratified §D5.4 arms.
- **CPU / matching quality.** The matcher assigns on a single priced distance; a moving creep sink is mispriced for the whole delivery, degrading the assignment quality `match_optimality_gap` (`market.rs:846`) is built to certify (its edge `service_ticks` assume static targets). Plus a new per-tick creep-liveness scrub and per-creep consumption modeling.
- **Determinism.** A creep sink keyed by a non-stable id, re-priced on a per-tick-changing position, is a new determinism surface — iteration over live creeps + position-dependent bids is exactly the result-affecting-HashMap-iteration bug class the determinism fence exists to catch (memory: *Sim determinism fence*).
- **Serialization.** `TransferTarget::Creep` embeds a creep id that may be dead next tick, inside a `ConvertSaveload`-serialized `TransferDepositTicket` that survives resets — the dangling-ref serialize-panic class (memory: *Entity marker serialization*, *ECS dangling-ref serialize panic*).
- **WFV bump** (fine per operator policy, but a real deploy reset).

**Net:** multi-crate, multi-week, re-implements per-creep at energy cost a buffer the adjacent container already provides for free (for upgraders), and fights three ratified subsystems. It reaches build/repair (which Architecture 1 misses), but at wildly disproportionate cost.

---

## 4. Comparison

| Dimension | Arch 1: Container-as-EV-deposit | Arch 2: Deliver-to-consumer |
|---|---|---|
| **Consumer classes reached** | Upgraders (controller container) + build/repair *withdraw-gating* (Defect 2) | Upgraders + builders + repairers as haul sinks |
| **What it fixes** | Defect 2 (consumer-shed inversion — the correctness fix) + Defect 1 (near-empty container mispricing — the tie-winning optimization) | Literal delivery-into-creep for the container-less consumers |
| **Throughput benefit** | Correctness: consumers finally shed under deficit. Optimization: near-empty controller container wins ties it should win. Magnitude fill-dependent (crossover ~21%/13% free), not a flat uplift. | Marginally more coverage (build/repair delivery), but mispriced-while-moving degrades the very assignment it adds |
| **Effort** | Phase 1 ~1 file (`room_transfer.rs`); Phase 2 admission wiring (behavior-only) | Multi-crate, multi-week; new `TransferTarget`/`SinkKey` variant threaded through dozens of matches; FSM inversion; body re-model |
| **Body change** | None | Extra CARRY per upgrader (waste); modest throughput for builders; worse spawn-EV rank |
| **CPU** | Negligible (one extra bid + one admission compare) | New per-tick creep-liveness scrub + per-creep consumption model + degraded matcher quality |
| **Mover** | Static container position — supported today | Moving sink — **not expressible**; forbidden one-off pursuit pathing |
| **Determinism** | No new surface (numeric bid on stable tile-keyed sink) | New surface: non-stable id + position-dependent bid = fence bug class |
| **Serialization** | No new surface | Creep id in serialized ticket = dangling-ref panic class |
| **WFV bump** | None (numeric `new()` path + behavior-only admission) | Yes (new serialized variant) |
| **Live/sim parity** | *Closes* two divergences the sim already validates | *Opens* new modeling the sim must grow to match |
| **Risk** | Low (parity patch; sim already runs these shapes) | High (fights 3 ratified subsystems) |

---

## 5. Smallest validation experiment — the sim A3 arm (must reproduce BOTH defects)

**Subtlety #1:** the sim baseline **already** prices the controller container at EV (`market.rs:313-320` is unconditional) *and* wires admission (`runner.rs:1376`). So an `a3` flag can't "turn A3 on" — it's already on. The clean experiment is the **inverse control**: add a flag that *reverts* the sim to today's LIVE behavior, then tournament the two arms.

**Subtlety #2 (the decisive correction):** reverting *only* the deposit bid does **not** reproduce live, because live also never runs the admission gate (Defect 2). The sim's admission at `runner.rs:1376` uses the raw `upgrade_sink_bid`, unaffected by any deposit-bid toggle. **Arm A must revert both defects**, or it fails to reproduce live and the experiment measures Defect 1 in isolation — which is *not* the change being shipped (Phases 1+2 ship together).

**Subtlety #3 (the seam name):** there is no `SinkKey::Controller` variant. `SinkKey::Container(x, y)` is positional (`baseline.rs:119`); the controller role comes from a *separate* `info.container_roles.get(&(x,y))` lookup returning `Some(ContainerRole::Controller)` (`market.rs:313-317`, verified). The control arm gates on that **role lookup**, not on a nonexistent enum variant.

### The arms
Add `a3_live_control: bool` to `MarketArmCfg` (`market.rs:60-69`, alongside `k4_bodies`).

- **Arm A (`a3_live_control = true`, the "live-today" control) — reverts BOTH defects:**
  1. For a container whose `container_roles` lookup is `Some(ContainerRole::Controller)`, price the deposit via `tier_to_bid(controller_container_deposit_priority(used, cap))` (flat 1250/par) instead of `buffer_deposit_bid`.
  2. Bypass the Use-lane admission gate (`runner.rs:1376/1528/1598`) so consumers draw regardless of the floor — reproducing the live inversion.
- **Arm B (`a3_live_control = false`, default = shipped sim = proposed live):** unchanged (EV buffer deposit + live admission).

### Tournament corpora
- **Family-C** (controller/upgrade-latency corpus) — the case A3 targets.
- **Family-S** (refill-latency corpus) — the regression guard.

### Predeclared GO threshold (predeclare, or the experiment is decorative)
Because the benefit is a fill-range crossover (§1) rather than a uniform uplift, the tournament could plausibly show a wash, and a wash must be an interpretable outcome. **Predeclare before running:**

- **GO iff** Arm B improves the Family-C metric (upgrade throughput and/or controller downgrade-safety — pick one primary before the run) by **at least the corpus noise floor** (the standard tournament seed-variance band; state the numeric band from the existing tournament harness, e.g. the ±σ already reported per family), **AND** Family-S refill-latency does not regress beyond that same noise band.
- **NO-GO / re-scope** if Family-C improvement is within noise (the defect, though real in pricing, does not move the corpus outcome — which would itself be a finding: ship Phase 2 for correctness only, and treat Phase 1 as cosmetic-parity).
- Report the Family-C benefit **and** the Family-S guard as effect sizes against the noise band, not as raw win/loss.

This isolates the *exact* levers live A3 flips (tier→EV deposit **and** admission wiring) with everything else held — no body changes, no new sink types, no new DTO fields. It measures the benefit **before any live edit**, cheaply, with machinery that exists today.

---

## 6. Recommendation

### GO on Architecture 1 (both defects, sequenced). NO-GO on Architecture 2.

**Rationale for GO on Arch 1:**
- The **correctness fix (Defect 2)** is the larger win: live consumers currently *never* stop draining under a refill deficit — a behavior inversion versus the intended second-class design. Wiring the already-written `admit_use_withdraw`/`admit_repair` + `downgrade_veto` (mirroring `runner.rs:1376/1528/1598`) closes it.
- The **optimization (Defect 1)** routes the controller container deposit through the EV buffer curve (`buffer_deposit_bid(upgrade_bid, …)`) on the live path, deleting the self-flagged A8 stub (`room_transfer.rs:372`). It lets a near-empty container win the ties it should win. Its magnitude is fill-dependent (crossover ≈21% free normal / ≈13% near level-up), not a flat 37–58%.
- Both are **live-parity patches**, not new design: the sim already runs both shapes (`market.rs:313-320`, `runner.rs:1376`). Risk is parity verification, not modeling.
- Effort is ~1 file for Phase 1, behavior-only admission wiring for Phase 2; no new types, no body/mover change, no WFV bump (numeric deposit path + behavior-only admission — confirm Phase 2 stores no serialized floor).

**Rationale for NO-GO on Arch 2:**
- It reaches build/repair, but those are *not container-backed* and their EV is already carried on the spawn side (`build_bid`/`repair_bid`) and — once the build/repair pickups are floor-gated in Phase 2 — on the Use lane. The incremental coverage is small.
- It re-implements per-creep, at ~50e/creep, a buffer the adjacent container gives for free (for the upgrader case Arch-1 already serves), while fighting the body-EV kernel, the static-target mover (moving-sink pricing is *not expressible* and pursuit pathing is forbidden), and the determinism/serialization fences. Cost is wildly disproportionate to benefit.

### Two open analysis items the sim adjudicates (do not assert them in prose)

- **Hauler round-trip / distance normalization (M1).** The whole point of a deposit bid is that a hauler *round-trips* to it. The controller container is typically **far** from storage/sources; the spawn/extension refill lane is central. `market_pass` ranks by *density* (value per distance), so a 2000 bid at 2× the leg distance can lose to a 1250 refill at 1× — meaning A3 may change *no* assignment in some rooms. **This is the exact lever that decides whether A3 moves anything**, and it cannot be settled by prose reasoning ("container outbids par → gets served" is unproven until distance-normalized). The sim prices legs, so the Family-C/Family-S tournament captures it directly — which is *why* the validation arm (§5) is a gate, not a formality. Do not claim a live win before the tournament shows one after distance normalization.
- **RCL-dependence of "how often it bites" (M3 — reconcile the tension).** The two "frequently bites" cases are *different rooms* and must not be blended:
  - **RCL8 maxed rooms** are frequent, but there `near_level_up` is *never* true (no level to gain), so the deposit ceiling is `V_UPGRADE`=2000; and a full refill lane "registers zero deposit amount" (`sink_economics.rs:321`, verified — a full lane's bid is moot), so the competitor is **storage par**, not refill. The live gap here is the *modest* "1250 flat vs 2000-when-near-empty," and only while the container is below the crossover fill.
  - **Near-level-up (the 3000 / large-delta case)** is by definition *mid-RCL leveling rooms*, which are *infrequent*.
  So "frequently-biting… worst exactly at near-level-up" conflates two disjoint populations: the frequent case (RCL8-maxed) carries the *small* delta; the large delta (near level-up) is *infrequent*. The honest claim is: **the defect is common but modest at RCL8-maxed, and large but rare at near-level-up.** The tournament's Family-C corpus should include both populations so the weighted benefit is measured, not assumed.

### Honesty check
A3 is **narrow, not sweeping**. "All-sinks-EV" is a misnomer — only the controller container qualifies as a container-backed consumer buffer; build/repair remain correctly withdraw-side (and, post-Phase-2, floor-gated on the withdraw). The room does not hard-starve today, so Defect 1 is an *optimization* (winning near-empty ties, protecting near-level-up steps) whose live impact is gated on the distance-normalization question above; Defect 2 is a genuine *correctness* fix (consumers finally shed under deficit). It is worth doing because it is cheap, closes two known parity divergences, and the sim can adjudicate the optimization's value before any live edit — but it should not be oversold as a large-value initiative, and the headline number is a fill-dependent crossover, not a flat percentage.

### Phased path (if GO)

1. **Phase 0 — Validate (sim only, no live edit).** Add the `a3_live_control: bool` control arm to `MarketArmCfg` (`market.rs:60-69`). Arm A reverts **both** defects (tier deposit bid via the `container_roles` role lookup **and** bypassed admission gate); Arm B is the current EV+admission default. Tournament on Family-C (benefit, both RCL populations) + Family-S (regression guard) in `tournament.rs`. **Decision gate:** proceed to live only if B beats A on the predeclared Family-C primary metric by at least the noise band, without Family-S regression beyond that band. A wash ⇒ ship Phase 2 (correctness) only, treat Phase 1 as cosmetic parity.
2. **Phase 1 — Live repricing (Defect 1).** In `execute_demands` (`room_transfer.rs:367-380`), extend the `is_refill` EV branch to EV-price the controller container deposit via `buffer_deposit_bid(upgrade_bid(near_level_up), free, CONTAINER_CAPACITY)` through the existing numeric `TransferDepositRequest::new` path; delete the A8 "keep their tier for now" stub. Confirm no WFV bump (numeric path).
3. **Phase 2 — Live admission parity (Defect 2, the correctness fix).** Wire `admit_use_withdraw`/`admit_repair` + `downgrade_veto` into the live consumer pickup / repair selection (`haulbehavior.rs:46`, `build.rs:64-80`), gating on the *raw* `upgrade_sink_bid`/`repair_bid` against the published floor (`transfersystem.rs:1637-1668`), mirroring `runner.rs:1376/1528/1598`. **Also gate the builder self-fetch pickup** (`build.rs:87-97, 137-146`, currently `TransferPriorityFlags::ALL` with no floor consult) so builders too shed under deficit. Keep it behavior-only — **confirm no serialized floor snapshot, hence no WFV bump.**
4. **Phase 3 — Verify.** Private-server soak on a maxed room: confirm the idle-upgrader-beside-empty-container scenario resolves (near-empty container out-bids par storage when refill is satisfied *and* the leg distance permits; refill still wins under deep deficit; consumers shed under deficit; downgrade veto fires when the clock is below `downgrade_veto_q`). Then MMO on go-ahead per standing policy.

**Making build/repair true haul sinks (Architecture 2) is explicitly out of scope** and is a separate, much larger initiative that is not recommended. Their EV is represented spawn-side and (post-Phase-2) floor-gated on the withdraw.

---

## Key file:line index

- Live container deposit tier (not EV): `screeps-econ-decision/src/demand.rs:150-157, 310-333`; `screeps-ibex/src/missions/localsupply/room_transfer.rs:367-380` (A8 stub at :372).
- tier→bid caps: `screeps-econ-decision/src/sink_economics.rs:523-539`; `screeps-ibex/src/transfer/transfersystem.rs:666-674`.
- EV pricing (sim-only today): `screeps-econ-decision/src/sink_economics.rs:408-414 (upgrade_bid, RAW/unscaled), 395-404 (build_bid), 443-449 (buffer_deposit_bid — quadratic base·(free/cap)²), 360-370 (repair_bid), 87-90 (V_UPGRADE + step premium)`; applied at `screeps-econ-eval/src/market.rs:299-322`; raw upgrade sink bid used at admission: `market.rs:283-285`.
- Floor + admission (defined; live-unwired = Defect 2): `screeps-econ-decision/src/sink_economics.rs:458-480`; `screeps-ibex/src/transfer/market_adapter.rs:132/139` (public wrappers, never called live); live floor publish `screeps-ibex/src/transfer/transfersystem.rs:1637-1668`.
- Sim admission wired (parity gap): `screeps-econ-eval/src/runner.rs:1373-1379, 1526-1534, 1591-1604`.
- SinkKey set (positional; role via separate lookup, no `Controller` variant): `screeps-econ-eval/src/baseline.rs:116-121`; role lookup `screeps-econ-eval/src/market.rs:313-317`.
- Consumer draw paths (Use lane, self-fetch; build self-fetch is floor-unaware): `screeps-ibex/src/jobs/upgrade.rs:28-73, 112-133`; `screeps-ibex/src/jobs/build.rs:56-106 (self-fetch at 87-97), 137-146, 157-161`.
- TransferTarget (no site/repair/creep variant): `screeps-ibex/src/transfer/transfersystem.rs:93-152, 1919-1961`.
- Static-Position mover: `screeps-econ-eval/src/movement.rs:54-68, 127-135`.
- Refill full-lane moot (RCL8-maxed competitor is storage par): `screeps-econ-decision/src/sink_economics.rs:316-334` (comment at :321).
- Sim MarketArmCfg (validation-arm home): `screeps-econ-eval/src/market.rs:60-69`.
- Container classification: `screeps-ibex/src/missions/localsupply/room_transfer.rs:279-283`.
