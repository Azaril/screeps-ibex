# Proposed Fixes — Small Bugs & Localized Issues — 2026-06-09

> **Status:** Proposed (for review). These are concrete, reviewable fix sketches — **NOT applied changes**. They are sourced from the review report's §4 Quick-Wins and the Low/Medium localized findings in [`../reviews/ibex-review-report.md`](../reviews/ibex-review-report.md). Each retains its `IBEX-NN` id so it cross-references the report, the Bug & Issue Register (§7), and the ADRs ([`../design/`](../design/)).
>
> **Scope guardrail.** This file covers only **small/localized** fixes. The structural deep-refactors (global `CpuGovernor` → [0004](../design/0004-cpu-governance-and-load-shedding.md); stable-id store → [0001](../design/0001-entity-model.md); versioned/tagged serialization → [0002](../design/0002-serialization.md); lead-follower cohesion + supervised FSM → [0003](../design/0003-behavior-modeling.md); runtime/containment → [0005](../design/0005-runtime-and-scheduling-model.md); eval harness → [0006](../design/0006-eval-and-iteration-harness.md)) are NOT duplicated here — they live in the ADRs and the [rewrite plan](rewrite-plan.md).
>
> **REFUTED seeds are NOT bugs and are NOT listed as fixes** (spawn-ordering correct; staticmine *panic* guarded; SquadContext members repaired; body-sizing clamp present; transfer generator ordering reservation-safe; walls maintained in peacetime; NaN deadlock refuted / RepairQueue guarded). IBEX-009/IBEX-019/IBEX-020/IBEX-046 appear below only as **latent-cleanup / hardening** items (the report explicitly keeps them as cleanups), never as live panics.
>
> **Conventions.** Line numbers are from the working tree at this date (each construct was opened and quoted to verify). Breaking-change labels: **None** | **Memory/format** | **Behavioral**. "Rides in" = which increment of the [rewrite plan](rewrite-plan.md) the fix slots into; most are None-breaking quick-wins eligible for **Increment 1** (landing alongside the survival-critical governor/containment work, since they touch the same hot paths).

---

## A. Safe quick-wins (None-breaking, can land in Increment 1)

These are localized, reversible, and do not change the serialized format. They harden reachable/latent panics, route intents through the existing guard, and remove no-op/foot-gun constructs. None require a state drop.

---

### IBEX-010 — Nuker-withdraw `panic!` → `Err` (reachable tick-abort)

- **Problem (one line):** A raid that registers an enemy nuker as a withdraw target reaches a `panic!`, which under `panic="abort"` aborts the whole tick and skips `serialize_world`.
- **File:line:** `transfer/transfersystem.rs:208` (the only reachable panic arm in `withdraw_resource_amount`).
- **Current construct (verified):**
  ```rust
  pub fn withdraw_resource_amount(&self, creep: &Creep, resource: ResourceType, amount: u32) -> Result<(), ErrorCode> {
      match self {
          // ... all other arms return Result ...
          //TODO: Split pickup and deposit targets.
          TransferTarget::Nuker(_id) => panic!("Attempting to withdraw resources from a nuker."),
          TransferTarget::PowerSpawn(id) => Self::withdraw_resource_amount_from_id(id, creep, resource, amount),
      }
  }
  ```
- **Proposed change:** The function already returns `Result<(), ErrorCode>`. Replace the panic with a logged `Err`:
  ```rust
  TransferTarget::Nuker(_id) => {
      // A nuker cannot be a withdraw source (see raid.rs structure registration).
      // Return an error instead of aborting the tick (panic="abort").
      log_once!("Attempted nuker withdraw — invalid TransferTarget pairing");
      Err(ErrorCode::InvalidArgs)
  }
  ```
  (`log_once!` = whatever one-shot/throttled log helper exists; a plain `warn!` is acceptable interim — the point is no panic.)
- **Validation:** Unit test over `(TransferTarget::Nuker, withdraw)` asserting `Err(InvalidArgs)` rather than panic; harness smoke-run with an active raid against a resource-holding enemy nuker confirms the tick completes and `serialize_world` runs.
- **Breaking-change:** **None.**
- **Rides in:** **Increment 1** (explicitly listed in the plan's Inc 1 set; the tick-level `catch_unwind` from the same increment is the backstop). The longer-term type-split of `TransferTarget` into withdraw-capable vs deposit-capable enums is deferred to [ADR 0005](../design/0005-runtime-and-scheduling-model.md) and is **not** part of this quick-win.
- **Refinement flag:** The report notes the **deposit-side** panics in the sibling `creep_transfer_resource_amount` (`:249`/`:250`/`:251` Ruin/Tombstone/Resource) are **NOT reachable today** (generators register those only as withdraws; `raid.rs` comments them out). Converting them too is harmless and consistent, but do **not** present them as live bugs — only `:208` is reachable.

---

### IBEX-029 — Route `squad_combat` combat intents through `action_flags`

- **Problem (one line):** Every combat action in `squad_combat.rs` is a bare `let _ = creep.attack/heal(...)` that bypasses the `SimultaneousActionFlags` guard the rest of the job layer relies on — safe today only "by luck of return value," a foot-gun the moment a combat state is refactored to multi-transition.
- **File:line:** `jobs/squad_combat.rs:994` (an `UNSET` flag-set is created but never consumed for combat) + the ~23 unguarded action sites verified at `:200, :216, :234, :248, :250, :401, :419, :438, :455, :478, :480, :511, :528, :542, :551, :559, :673, :680, :693, :695, :745, :751, :765`.
- **Current construct (verified):**
  ```rust
  let _ = creep.attack(target);
  let _ = creep.ranged_mass_attack();
  let _ = creep.ranged_attack(target);
  let _ = creep.heal(&target);
  let _ = creep.ranged_heal(&target);
  // ...no action_flags.consume(...) anywhere in the file.
  ```
- **Proposed change:** Gate each combat intent through the per-creep `action_flags` exactly like `haul`/`staticmine` gate `MOVE`/`TRANSFER`/`HARVEST`:
  ```rust
  if tick_context.action_flags.consume(SimultaneousActionFlags::ATTACK) {
      let _ = creep.attack(target);
  }
  // RANGED_ATTACK / RANGED_MASS_ATTACK share one flag (one ranged-attack intent/tick);
  // HEAL / RANGED_HEAL share one flag (one heal intent/tick).
  ```
  Note: `ranged_attack` and `ranged_mass_attack` are the same engine intent slot (gate on one `RANGED_ATTACK` flag); `heal` and `ranged_heal` likewise (one `HEAL` flag). MOVE is already issued via the rover.
- **Validation:** Add a `debug_assert!` (or a counted check behind a debug feature) that no creep fires the same intent category twice in one tick; replay-parity on an engagement scenario (old vs flagged) should produce identical intents today since each combat state currently returns `None`.
- **Breaking-change:** **None** (no behavior change today — it only closes the double-fire hole for future refactors).
- **Rides in:** **Increment 1** (hardening the most-churned subsystem before the FSM work; the "all intents through one guarded sink" principle is the [ADR 0003](../design/0003-behavior-modeling.md) end-state, this is the interim hand-wire).
- **Refinement flag:** Confirm the exact flag enum variants for ranged/heal before wiring — the report says "~16 bare calls"; the verified count is **23 sites**. Audit all 23, not a subset, or the guarantee is partial.

---

### IBEX-044 — `saturating_sub` on `game::time() - t` (cadence/timeout subtractions)

- **Problem (one line):** Several cadence/timeout checks compute `game::time() - persisted_tick` with unchecked `u32` subtraction; if `game::time()` ever decreases relative to a persisted tick (private-server time reset, restored old snapshot) this underflows — a debug-overflow that would abort the tick under `panic="abort"`.
- **File:line:** `operations/war.rs:1315` (`should_run_tier`), `jobs/squad_combat.rs:167` (combat-response timeout), plus `operations/attack.rs:579/:637` and `operations/colony.rs:193` (same pattern per the register).
- **Current construct (verified):**
  ```rust
  // war.rs:1314–1316
  fn should_run_tier(&self, last_tick: Option<u32>, cadence: u32) -> bool {
      last_tick.map(|t| game::time() - t >= cadence).unwrap_or(true)
  }
  // squad_combat.rs:165–168
  let timed_out = state_context
      .combat_response_start
      .map(|start| game::time() - start > COMBAT_RESPONSE_TIMEOUT)
      .unwrap_or(false);
  ```
- **Proposed change:** Use the safe form the codebase already uses elsewhere (`scout.rs:175` is `game::time().saturating_sub(idle_since)`):
  ```rust
  last_tick.map(|t| game::time().saturating_sub(t) >= cadence).unwrap_or(true)
  // ...
  .map(|start| game::time().saturating_sub(start) > COMBAT_RESPONSE_TIMEOUT)
  ```
- **Validation:** Unit test with `stored_tick > game::time()` asserting no panic and a benign result (cadence not run yet / not timed out); grep to confirm every `game::time() - <tick>` site is converted.
- **Breaking-change:** **None** (no behavior change in normal play — `saturating_sub` equals subtraction whenever `game::time() >= t`).
- **Rides in:** **Increment 1** (trivial; pairs naturally with the war-cadence raise IBEX-021 which touches `should_run_tier`'s callers).
- **Refinement flag:** Confidence is **L** (the wrap is self-correcting in release). Worth doing because it removes a `panic=abort` vector for free; do not over-scope into a "tick-clock abstraction."

---

### IBEX-045 — `saturating_sub` in `store.rs` free-capacity helper

- **Problem (one line):** The free-capacity helper computes `capacity - used_capacity` (summed per-resource) and exists *because of* a `get_used_capacity` double-count workaround — so summed `used` could in principle exceed `get_capacity(None)` and panic under `panic="abort"`. Used in harvest/transfer hot paths.
- **File:line:** `store.rs:11–16` (`expensive_store_free_capacity`).
- **Current construct (verified):**
  ```rust
  fn expensive_store_free_capacity(&self) -> u32 {
      let capacity = self.store().get_capacity(None);
      let store_types = self.store().store_types();
      let used_capacity = store_types.iter().map(|r| self.store().get_used_capacity(Some(*r))).sum::<u32>();
      capacity - used_capacity
  }
  ```
- **Proposed change:**
  ```rust
  let used_capacity = store_types.iter().map(|r| self.store().get_used_capacity(Some(*r))).sum::<u32>();
  debug_assert!(used_capacity <= capacity, "store used ({used_capacity}) exceeded capacity ({capacity})");
  capacity.saturating_sub(used_capacity)
  ```
  The `debug_assert!` surfaces the API anomaly in tests/sim without aborting production.
- **Validation:** Unit test feeding a fixture store where summed per-resource used > capacity, asserting `0` (not panic). Reachable only if the store API mis-reports; currently unobserved.
- **Breaking-change:** **None.**
- **Rides in:** **Increment 1.**
- **Refinement flag:** This is the **same root** as IBEX-050 (the `capacity - used_capacity` workaround is duplicated). When IBEX-050 extracts one helper, this fix should live *in that single helper* rather than being applied twice — sequence IBEX-050 first, or fold this `saturating_sub`+`debug_assert` into the extracted helper.

---

### IBEX-050 — Extract one capacity helper (de-duplicate the `get_used_capacity` workaround)

- **Problem (one line):** The per-resource summed `used_capacity = Σ get_used_capacity(Some(r))` workaround (a guard against an upstream `get_used_capacity(None)` double-count) is copy-pasted 5+ times; all sites must change together if the upstream API bug is fixed.
- **File:line:** `jobs/utility/haulbehavior.rs:28/31, :80/83, :388/390, :479/482` (four copies, each with the commented-out `get_used_capacity(None)` alternative) and the structurally-identical body in `store.rs:11–16` (IBEX-045). Register also cites `haul.rs:168`.
- **Current construct (verified, representative):**
  ```rust
  // haulbehavior.rs:28–31 (repeated at 80/83, 388/390, 479/482)
  let used_capacity = store_types.iter().map(|r| creep.store().get_used_capacity(Some(*r))).sum::<u32>();
  //let used_capacity = creep.store().get_used_capacity(None);
  let free_capacity = capacity - used_capacity;
  ```
- **Proposed change:** Promote `store.rs`'s `HasExpensiveStore::expensive_store_free_capacity` (already a trait on `T: HasStore`) to the single source of truth and call it at every site; add a sibling `expensive_store_used_capacity` for the sites that need the used value directly. Fold the IBEX-045 `saturating_sub` + `debug_assert!` in here so the workaround — and any future upstream-fix revert — lives in **one** place.
  ```rust
  // one helper, called everywhere:
  let free_capacity = creep.expensive_store_free_capacity();
  ```
- **Validation:** `grep` confirms exactly one definition of the summed-used computation after the change; existing haul behavior unchanged (replay-parity on an economy-bringup scenario).
- **Breaking-change:** **None** (pure refactor / tech-debt).
- **Rides in:** **Increment 1** (low-risk consolidation; do this *before/with* IBEX-045 so the `saturating_sub` lands once).
- **Refinement flag:** Confirm the `store_types()`-based loop is semantically identical at all five sites (it is, per inspection) before collapsing — one site (`haulbehavior.rs:148`/`:604`) reads per-resource amounts for a *different* purpose and must NOT be folded into the free-capacity helper.

---

### IBEX-043 — Power-bank concurrency no-op filter counts ALL attacks

- **Problem (one line):** The power-bank concurrency gate counts *all* active attacks (the inline comment promises a filter that never follows), so `max_concurrent_power_banks` is throttled by unrelated attacks and is effectively meaningless.
- **File:line:** `operations/war.rs:766–776`.
- **Current construct (verified):**
  ```rust
  let power_bank_count = self
      .active_attack_rooms
      .iter()
      .zip(self.active_attack_entities.iter())
      .filter(|(_, _)| true) // Count all active -- we'll filter below
      .count() as u32;
  // ...compared against self.max_concurrent_power_banks
  ```
- **Proposed change:** Count only attacks whose reason is `PowerBank`. The cleanest form resolves each `active_attack_entities` to its `AttackOperation` and filters on its reason/kind, e.g.:
  ```rust
  let power_bank_count = self
      .active_attack_entities
      .iter()
      .filter_map(|e| system_data.operations.get(*e).as_operation_type::<AttackOperation>())
      .filter(|op| op.is_power_bank()) // or: op.reason == AttackReason::PowerBank
      .count() as u32;
  ```
  If a per-reason resolution is awkward at this call site, the simpler alternative is a dedicated `active_power_bank_count` field bumped on power-bank launch and decremented on cleanup. Either way, **delete the `.filter(|_| true)` no-op**.
- **Validation:** Scenario with N non-power-bank attacks active and the power-bank cap = 1; assert a power-bank launch is still permitted (today it is wrongly blocked). Assert independent slot filling.
- **Breaking-change:** **None** (no serialized-field change if using the resolve-and-filter form; the counter-field alternative would be Memory/format — prefer the resolve form to stay None-breaking).
- **Rides in:** **Increment 1.**
- **Refinement flag:** Verify the exact way `AttackOperation` records "this is a power-bank farm" (a `reason`/`kind` enum vs a flag) before writing the predicate — the report says "reason is PowerBank" but the field name must be confirmed at the operation type. Prefer the resolve-and-filter form over a new field to avoid a format break.

---

### IBEX-009 — `staticmine` `resolve().unwrap()` → `if let` (latent cleanup only)

- **Problem (one line):** A code-smell `unwrap()` on a re-resolve, **guarded and not a live panic** (downgraded Critical→Low in the report) — hardening it protects future refactors.
- **File:line:** `jobs/staticmine.rs:201` (reached only inside `if container_exists` at `:182`, where `container_exists = ...resolve().is_some()` at `:180`).
- **Current construct (verified):**
  ```rust
  // :180  let container_exists = state_context.container_target.resolve().is_some();
  // :182  if container_exists {
  // ...
  // :201      let container = state_context.container_target.resolve().unwrap();
  ```
- **Proposed change:** Replace the re-resolve+unwrap with a single bound resolve:
  ```rust
  if let Some(container) = state_context.container_target.resolve() {
      // ... use container ...
  } else {
      return None; // container vanished mid-tick — fall back to rediscovery
  }
  ```
- **Validation:** No live-bug test needed (the report confirms it is not reachable this tick). A `cargo build`/`clippy` pass is sufficient; optionally a unit asserting the `None` branch is a clean no-op.
- **Breaking-change:** **None.**
- **Rides in:** **Increment 1** (pure cleanup). **Do NOT prioritize as a survival bug** — the report is explicit it is a smell, not a live panic.

---

### IBEX-019 — `attack.rs:615` guarded double-unwrap → explicit `if let` (latent cleanup)

- **Problem (one line):** A `room_entity.unwrap()).unwrap()` that is **provably safe this tick** via the implicit `have_live_intel` guard; making the safety explicit prevents a future edit from regressing it.
- **File:line:** `operations/attack.rs:615`, guarded by `have_live_intel` at `:608–612`.
- **Current construct (verified):**
  ```rust
  let have_live_intel = room_entity
      .and_then(|e| system_data.room_data.get(e))
      .and_then(|rd| rd.get_dynamic_visibility_data())
      .map(|d| d.visible())
      .unwrap_or(false);
  if have_live_intel {
      let room_data = system_data.room_data.get(room_entity.unwrap()).unwrap();
      self.analyze_target(room_data);
      // ...
  }
  ```
- **Proposed change:** Bind the room data directly in the condition so no second unwrap exists:
  ```rust
  let live_room_data = room_entity
      .and_then(|e| system_data.room_data.get(e))
      .filter(|rd| rd.get_dynamic_visibility_data().map(|d| d.visible()).unwrap_or(false));
  if let Some(room_data) = live_room_data {
      self.analyze_target(room_data);
      // ...
  } else { /* existing persisted-threat fallback */ }
  ```
- **Validation:** Build/clippy; the branch is exercised by any attack-with-visibility scenario.
- **Breaking-change:** **None.**
- **Rides in:** **Increment 1** (pure cleanup; not a live panic).

---

### IBEX-020 — `attack_mission` `get_room()` last-resort `.expect` → sentinel/Option

- **Problem (one line):** `get_room()`'s last-resort `.expect(...)` would panic only in a fully-degraded mission (no home rooms, no owner, no squad entities) — but `get_room` is called by cleanup **and** by `repair_entity_integrity` in the serialize-critical post-pass, so a panic there aborts the tick (no isolation; IBEX-025).
- **File:line:** `missions/attack_mission.rs:1917`.
- **Current construct (verified):**
  ```rust
  } else {
      error!("[AttackMission] get_room: no home rooms and no owner for {}", self.context.target_room);
      // Return the first squad entity as a last resort.
      self.context
          .squad_entities
          .first()
          .copied()
          .expect("AttackMission must have at least one entity reference")
  }
  ```
- **Proposed change:** Callers already treat a non-room entity as a no-op, so return a sentinel instead of panicking. Cleanest is to widen `get_room` to return `Option<Entity>` (and adjust the two callers to `if let Some(e) = ...`); the minimal in-place version is to fall back to the owner-or-first-squad without `expect`:
  ```rust
  self.context.squad_entities.first().copied()
      .or(*self.owner)
      .unwrap_or_else(|| {
          error!("[AttackMission] get_room: fully degraded, returning sentinel");
          Entity::from_raw_parts(0, 0) // or change signature to Option<Entity> (preferred)
      })
  ```
  **Preferred:** change the signature to `Option<Entity>` — it removes the sentinel entirely and is honest about the degraded case. (Signature change touches two callers only; still None-breaking.)
- **Validation:** Construct a degraded mission (empty home rooms + `owner=None` + empty `squad_entities`), call `get_room()`, assert it returns `None`/sentinel and does **not** panic; assert the cleanup/repair callers treat it as a no-op.
- **Breaking-change:** **None.**
- **Rides in:** **Increment 1** (cheap; the tick-level `catch_unwind` from Inc 1 is the backstop, but removing the reachable-in-degraded-state panic is the targeted fix).

---

### IBEX-046 — `debug_assert!(is_finite)` at priority sites + guard divisors (latent NaN hardening)

- **Problem (one line):** Float comparators coalesce `NaN → Equal` via `partial_cmp(...).unwrap_or(Equal)`; there is **no NaN source today** (the deadlock framing is REFUTED and RepairQueue is fully guarded), but a future computed priority/value could introduce one silently.
- **File:line:** `room/visibilitysystem.rs:248–250` & `roomplansystem.rs:362` (priority); `transfer/transfersystem.rs:1842`/`:2061`/`:2108` & `ordersystem.rs:280` (value); `spawnsystem.rs:88–90` (priority).
- **Current construct (verified):**
  ```rust
  // visibilitysystem.rs:248–250
  a.priority.partial_cmp(&b.priority).unwrap_or(std::cmp::Ordering::Equal)
  // transfersystem.rs:1842
  a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
  ```
- **Proposed change:** **Keep** `unwrap_or(Equal)` as the runtime backstop (do not remove it). Add a `debug_assert!(x.is_finite())` at the *request/compute* sites where the priority/value is produced (not at the comparator), and guard the transfer value divisors at the source so a degenerate input can't produce inf/NaN:
  ```rust
  // at the value-producing site in transfersystem:
  let value = numerator / (length.max(1) as f32);   // never divide by 0
  // or cost.max(1.0)
  debug_assert!(value.is_finite(), "transfer value not finite: {value}");
  ```
- **Validation:** `debug_assert` fires in tests/sim if a future change introduces NaN; no production behavior change. Add a unit test feeding `length = 0` / `cost = 0` asserting a finite result.
- **Breaking-change:** **None.**
- **Rides in:** **Increment 1** (hardening). **Do NOT** "fix the NaN deadlock" — there is none; this is purely a tripwire for future regressions.
- **Refinement flag:** The report explicitly marks the NaN-deadlock and RepairQueue-NaN concerns **REFUTED** — frame any change as *defensive tripwire only*, never as a bug fix, to avoid re-introducing the false positive into the record.

---

### IBEX-013 (interim) — Compile-time disjoint-segment assert + non-breaking `COMPONENT_SEGMENTS` shrink

- **Problem (one line):** `serialize_world`'s trailing clear blanks every *unconsumed* segment, and `COMPONENT_SEGMENTS` includes seg-55 which is **also** `COST_MATRIX_SEGMENT` — so the cost matrix is wiped to empty end-of-tick in the normal small-ECS case, forcing a full per-room rebuild on the worst (post-reset) tick. **This is the Critical finding; the dedicated-segment move is the real fix (ADR 0002 / Increment 2). The two items here are the safe interim and the guardrail.**
- **File:line:** `game_loop.rs:554` (`COMPONENT_SEGMENTS = &[50, 51, 52, 53, 54, 55]`), `:453–455` (the trailing clear loop), `pathing/costmatrixsystem.rs:6` (`COST_MATRIX_SEGMENT = 55`).
- **Current construct (verified):**
  ```rust
  // game_loop.rs:554
  const COMPONENT_SEGMENTS: &[u32] = &[50, 51, 52, 53, 54, 55];
  // game_loop.rs:453–455 — blanks every segment not consumed by a chunk
  for segment in segments {
      data.memory_arbiter.set(*segment, "");
  }
  // pathing/costmatrixsystem.rs:6
  pub const COST_MATRIX_SEGMENT: u32 = 55;
  ```
- **Proposed change (two parts, both None-breaking):**
  1. **Compile-time disjointness assert** (the guardrail — prevents this class forever):
     ```rust
     const _: () = {
         let mut i = 0;
         while i < COMPONENT_SEGMENTS.len() {
             assert!(COMPONENT_SEGMENTS[i] != COST_MATRIX_SEGMENT,
                 "COMPONENT_SEGMENTS must not overlap COST_MATRIX_SEGMENT");
             i += 1;
         }
     };
     ```
     (A `const fn`/`const` block works on a `&[u32]` with a `while` loop; iterator combinators are not const-stable. Place it where both constants are in scope — likely `game_loop.rs` with a `use` of `COST_MATRIX_SEGMENT`.)
  2. **Interim shrink** to make the assert pass *without* a format change: `const COMPONENT_SEGMENTS: &[u32] = &[50, 51, 52, 53, 54];` — **only after** confirming the compressed payload fits in ≤5 chunks (5 × 50 KiB after gzip+base64 is a large empire; verify with a fullness watermark log first). This stops seg-55 from ever being in the clear set.
- **Validation:** The compile-time assert *is* the regression test (build fails if they overlap). Plus: log seg-55 length after `serialize_world`, force a reset, assert `load_cost_matrix_cache` returns non-empty. Before shrinking, log encoded size + chunk count (IBEX-014's watermark) to confirm ≤5 chunks.
- **Breaking-change:** **None** for the assert and the shrink (the shrink only *narrows* the segment set; existing payloads still deserialize from 50–54, and seg-55 was being wiped anyway). The **dedicated-cost-matrix-segment move** (the real ADR 0002 fix) IS **Memory/format** and is **deferred to Increment 2** with the version header + reject-and-reset.
- **Rides in:** **assert + shrink in Increment 1** (None-breaking guardrail; closes the *symptom* immediately and is the documented rollback for the Inc 2 work). The **dedicated-segment move** rides in **Increment 2** ([ADR 0002](../design/0002-serialization.md)).
- **Refinement flag:** The shrink is only safe if the payload genuinely fits in ≤5 chunks — **gate it on the watermark** (IBEX-014), do not assume. If a large empire needs the 6th chunk, the shrink is invalid and the dedicated-segment move (Inc 2) must come first; the assert alone (which would then fail to compile against the current `[50..55]`) signals exactly that.

---

## B. Needs-design / deferred (do NOT land as a quick-win)

These appear in the source list but the report defers them, or they require a design decision / a wait-vs-abort semantics change / a format-touching change. They are tracked here so the quick-win list stays clean, with a pointer to where the real work lives.

---

### IBEX-018 — Market trust-guard hardening (interim, ahead of the full ADR 0012 planner)

- **Problem (one line):** `can_trust_history` gates trades on transaction count >100, volume >1000, and `stddev ≤ avg×0.5` over the **latest day only** — a gate a wash-trading rival clears cheaply (the 0012 threat model costs history-painting at ~36× ROI), after which Ibex buys/sells at `avg ± 0.1σ` of the manipulated day.
- **File:line:** `transfer/ordersystem.rs:349–351` (`can_trust_history`), `:370–420` (pricing off the latest day), `:367`/`:446` (TODOs admitting the unimplemented sanity check).
- **Why deferred (not a pure quick-win):** The real fix is [ADR 0012](../design/0012-market-and-risk.md)'s `MarketSnapshot → TradePlanner` (volume-filtered 14-day median + chain-value anchors + exposure caps + kill-switch), which is Increment-7 work. An **interim hardening** (M1) is small but still a pricing-behavior change that needs the harness fixtures to validate: trailing multi-day median instead of latest-day, a hard per-day credit/resource exposure cap, and a max-transaction-cost / prefer-local floor on cross-map deals (the energy-cost griefing surface).
- **Proposed direction:** Land 0012 M1 (trailing-window median + exposure caps + cost floor) as the Increment 1–2 interim; the full planner + risk ledger (seg 58) lands with 0012 M2/M3 in Increment 7. Buy-side flags stay off until 0012 M4 (adversary scenario) passes — per the post-critic amendment.
- **Breaking-change:** **Behavioral** (pricing/eligibility changes; no format change in M1).
- **Rides in:** **Increment 1–2** (M1 interim) → **Increment 7** (full ADR 0012). Tracked under [ADR 0012](../design/0012-market-and-risk.md).
- **Refinement flag:** A trailing median still reads `getHistory`, which is backend-side and not organically populated on a private server (engine-mechanics verify-list) — fixture-test the pricing kernel; do not trust private-server market data as validation.

---

### IBEX-021 — Raise war cadences off `1` (CPU)

- **Problem (one line):** `DEFENSE/OFFENSE/RECOMPUTE_CADENCE` are all hardcoded to `1`, so per-tick threat scans + `RoomThreatData` clones + O(attacks×homes) cold-cache route rebalance run every tick — a primary contributor to the death-spiral.
- **File:line:** `operations/war.rs:139–141` (the constants), `:1314–1316` (`should_run_tier` returns true every tick), `:626–630` (clones every `RoomThreatData`), `:1097–1241` (`reassign_home_rooms`).
- **Why deferred (not a pure quick-win):** Raising the constants is a *one-line* change, but it is labelled **Behavioral** and the report wants it **either** restored to the documented cadences (defense ~2, offense ~10–20, recompute ~50) **or** gated by `can_execute_cpu`/the future `CpuGovernor`. Picking the right cadences and the gate is a design call that belongs with the governor work, and the change must be measured (`features.system_timing` per-tier CPU) — it is not a fire-and-forget cleanup.
- **Proposed direction:** Restore intended cadences as the interim; in [ADR 0004](../design/0004-cpu-governance-and-load-shedding.md) replace the fixed cadence with a bucket-aware gate. Also restructure the offense eval to borrow `RoomThreatData` immutably / collect only the small fields used instead of full clones.
- **Breaking-change:** **Behavioral.**
- **Rides in:** **Increment 1** (the plan explicitly lists "raise war cadences off 1" in Inc 1 alongside the governor), but as a **measured, governor-coordinated** change — NOT a blind constant bump. Tracked under [ADR 0004](../design/0004-cpu-governance-and-load-shedding.md). The IBEX-044 `saturating_sub` fix touches the same `should_run_tier` and should be applied together.
- **Refinement flag:** A blind bump to the doc-comment cadences without the immutable-borrow restructure still leaves the per-recompute clone cost; sequence the borrow fix with the cadence raise.

---

### IBEX-040 — Shove/local-avoidance walkability ignores blocking structures

- **Problem (one line):** `is_tile_walkable` checks only room edges and `Terrain::Wall`; it ignores structures and the cost matrix, so the resolver can shove a creep onto a tile occupied by a blocking structure (wall structure, hostile rampart, spawn) → the subsequent move errors and the creep wastes the tick / oscillates.
- **File:line:** `screeps-rover/src/screeps_impl.rs:142–155` (`is_tile_walkable`); used by the resolver via `movementsystem.rs:608`.
- **Current construct (verified):**
  ```rust
  fn is_tile_walkable(&self, pos: Position) -> bool {
      let x = pos.x().u8();
      let y = pos.y().u8();
      if x == 0 || x == 49 || y == 0 || y == 49 { return true; }
      if let Some(terrain) = game::map::get_room_terrain(pos.room_name()) {
          if terrain.get(x, y) == Terrain::Wall { return false; }
      }
      true   // <-- no structure / cost-matrix check
  }
  ```
- **Why deferred (not a pure quick-win):** **It lives in the `screeps-rover` submodule.** Per AGENTS.md §9, submodule changes must be made in the leaf repo first and the superproject pointer updated in a separate, ordered commit — it is not a same-repo one-liner. It also wants the *cached structure cost layer* (the rover's own cost matrix), which couples to the cost-matrix caching work (IBEX-017/038) and must avoid re-introducing an uncached per-tile `room.find` lookup (a CPU regression). The fix is correct but needs the cost-layer plumbing decided first.
- **Proposed direction:** Make `is_tile_walkable` (or a resolver-specific predicate) consult the cached structure cost layer already maintained by `costmatrixsystem`, so shove/avoidance respects blocking structures without a fresh `find`. Land it in the rover crate, then bump the submodule.
- **Breaking-change:** **None** (behavior-correcting, no format change) — but a **submodule change**, sequenced per AGENTS.md.
- **Rides in:** Slot with the pathing/cost-matrix caching work ([ADR 0004](../design/0004-cpu-governance-and-load-shedding.md)) so it reuses the cached structure layer rather than adding an uncached lookup. Not Increment 1.
- **Refinement flag:** Naively calling `room.find(STRUCTURES)` per `is_tile_walkable` call would be a CPU regression in the resolver hot loop — the fix MUST read the cached cost layer, which is why it is gated on the cost-matrix caching decision.

---

### IBEX-048 — Depleted-source / exhausted-mineral missions never torn down

- **Problem (one line):** `LocalSupplyMission::ensure_children` only ever **adds** child missions; a `MineralMiningMission` whose mineral is exhausted returns `Ok(())` as a no-op forever and is never pruned, so an idle mission lingers indefinitely after depletion.
- **File:line:** `missions/localsupply/mod.rs:130–252` (`ensure_children` — add-only, no removal), `missions/localsupply/mineral_mining.rs:183–187` (the `mineral_amount() == 0 → return Ok(())` no-op).
- **Current construct (verified):**
  ```rust
  // mineral_mining.rs:183–187 — depleted mineral becomes a silent no-op, mission stays alive
  if let Some(mineral) = self.mineral.resolve() {
      if mineral.mineral_amount() == 0 {
          return Ok(());
      }
  }
  // mod.rs ensure_children: only `self.mineral_mining_missions.push(child_entity)` — never removes.
  ```
- **Why deferred (not a pure quick-win):** Removing a child mission cleanly needs a teardown decision: when (a regenerating source vs a depleted mineral that won't return until the next regen window — minerals **do** regenerate after a cooldown, so "torn down forever" would be wrong), how to coordinate with the cleanup/child-cascade so the entity and its room-mission-list entry are removed without dangling refs (Field Report E territory), and whether to use an idle-timeout vs an explicit depletion signal. This is mission-lifecycle semantics, adjacent to the [ADR 0003](../design/0003-behavior-modeling.md) supervised-FSM / IBEX-002 watchdog work.
- **Proposed direction:** Add an idle/depletion teardown to `ensure_children` (or a sibling prune step): when a mineral is exhausted *and* its regen is far off, request mission removal through the normal child-cleanup path; re-create it via `ensure_children` when the mineral regenerates (the add-path already handles re-creation). Reuse the IBEX-002 per-state deadline machinery rather than a bespoke timer.
- **Breaking-change:** **Behavioral** (missions now tear down/recreate).
- **Rides in:** **Increment 4** (lifecycle/teardown, with IBEX-002's watchdog) — tracked under [ADR 0003](../design/0003-behavior-modeling.md). Not Increment 1.
- **Refinement flag:** Minerals regenerate after `MINERAL_REGEN_TIME`; a naive "depleted → delete forever" would permanently lose the mineral. The teardown must be re-creatable (which the add-only `ensure_children` already supports on the next visible tick) — design it as *idle-suspend/recreate*, not *delete-permanently*.

---

### IBEX-049 — `CreepRoverData.path` should be `#[serde(skip)]`

- **Problem (one line):** Each creep's full remaining `path: Vec<Position>` is serialized into the component segments **every tick**; the path is recomputable from destination+range, so persisting it is pure segment-size pressure that feeds the seg-overflow / chunk-exhaustion risks (IBEX-013/014).
- **File:line:** `pathing/movementsystem.rs:14–17` (`CreepRoverData(pub CreepMovementData)`, `#[serde(transparent)]`), serialized via `game_loop.rs` component list. The actual `path` field is **in the `screeps-rover` submodule**: `screeps-rover/src/movementsystem.rs:159` (`CreepPathData.path: Vec<Position>`), reached through `CreepMovementData.path_data: Option<CreepPathData>` (`:166–168`).
- **Current construct (verified):**
  ```rust
  // screeps-ibex/src/pathing/movementsystem.rs:14–17
  #[derive(Shrinkwrap, Component, Serialize, Deserialize, Clone, Default)]
  #[serde(transparent)]
  pub struct CreepRoverData(pub CreepMovementData);
  // screeps-rover/src/movementsystem.rs:157–168
  struct CreepPathData { destination: Position, range: u32, path: Vec<Position>, time: u32, /* ... */ }
  pub struct CreepMovementData { path_data: Option<CreepPathData> }
  ```
- **Why deferred (not a pure quick-win):** Two reasons. (1) **Submodule:** the `path` field lives in `screeps-rover`; per AGENTS.md the change is leaf-repo-first + a separate superproject pointer bump. (2) **It touches the serialized format** — dropping a serialized field is a **Memory/format** change (old snapshots carry the bytes; under positional bincode, removing a field mid-struct misaligns everything after it — exactly the IBEX-004 hazard). So it MUST land *with* the version header + reject-and-reset (Increment 2), not as a standalone edit, and the rover struct must repath-on-load when `path` is absent (it already recomputes paths, so a skipped/empty path just triggers a repath — acceptable).
- **Proposed direction:** In the rover crate, mark `CreepPathData.path` (and any other recomputable transient like remaining waypoints) `#[serde(skip)]`, ensure the deserialize path treats an empty `path` as "needs repath." Bundle the format change with the Increment 2 version header so old snapshots reject-and-reset cleanly rather than mis-decode.
- **Breaking-change:** **Memory/format** (drops a serialized field) — and a **submodule change**.
- **Rides in:** **Increment 2** (with the serialization version header + disjoint-segment work) — [ADR 0002](../design/0002-serialization.md). Not Increment 1.
- **Refinement flag:** Confirm the rover repaths correctly when `path_data` is present but `path` is empty after a skip-load (it should, since paths are recomputed on miss) — otherwise creeps stall for one tick post-reset. Measure serialized segment bytes before/after at creep scale to confirm the win.

---

## C. Cross-references & sequencing summary

| ID | One-line fix | Breaking | Group | Rides in | ADR |
|---|---|---|---|---|---|
| IBEX-010 | nuker withdraw `panic!`→`Err`+log | None | A | Inc 1 | 0005 (type-split later) |
| IBEX-029 | route combat intents through `action_flags` | None | A | Inc 1 | 0003 |
| IBEX-044 | `saturating_sub` on `game::time()-t` | None | A | Inc 1 | — |
| IBEX-045 | `saturating_sub`+`debug_assert` in store helper | None | A | Inc 1 | — |
| IBEX-050 | extract one capacity helper | None | A | Inc 1 | — |
| IBEX-043 | power-bank filter counts only PowerBank attacks | None | A | Inc 1 | — |
| IBEX-009 | staticmine `unwrap`→`if let` (cleanup) | None | A | Inc 1 | — |
| IBEX-019 | attack.rs:615 explicit `if let` (cleanup) | None | A | Inc 1 | — |
| IBEX-020 | attack_mission `get_room` sentinel/Option | None | A | Inc 1 | — |
| IBEX-046 | `debug_assert(is_finite)` + guard divisors | None | A | Inc 1 | — |
| IBEX-013 | disjoint-segment assert + COMPONENT_SEGMENTS shrink | None (interim) | A | Inc 1 (assert+shrink) | 0002 (dedicated segment, Inc 2) |
| IBEX-018 | market trust-guard interim (trailing median + exposure caps + cost floor) | Behavioral | B | Inc 1–2 (M1) → Inc 7 (full) | 0012 |
| IBEX-021 | raise war cadences (measured, governor-gated) | Behavioral | B | Inc 1 | 0004 |
| IBEX-040 | shove walkability consults structure cost layer | None (submodule) | B | with 0004 | 0004 |
| IBEX-048 | depleted source/mineral mission teardown | Behavioral | B | Inc 4 | 0003 |
| IBEX-049 | `CreepRoverData.path` `#[serde(skip)]` | Memory/format (submodule) | B | Inc 2 | 0002 |

**Ground rules carried from the review/plan:**
- These are **proposals to review** — none are applied. Refine the predicate/signature details flagged above before implementing.
- Group A is the None-breaking quick-win set that can land in **Increment 1** behind the same tick-level `catch_unwind` containment; Group B needs a design decision, a wait-vs-abort change, a submodule sequencing step, or a format-touching change and is deferred to its noted increment/ADR.
- Submodule items (IBEX-040, IBEX-049) follow AGENTS.md §9: leaf-repo commit first, then the superproject pointer bump.
- Format-touching items (IBEX-049, and IBEX-013's *real* dedicated-segment fix) ride the **Increment 2** version-header + reject-and-reset so old snapshots fail loud, never silently mis-decode.
- Do **not** re-open the REFUTED seeds; IBEX-009/019/020/046 are explicitly *latent hardening*, not live bugs.
