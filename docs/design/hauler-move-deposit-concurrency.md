# Design: Reclaim the wasted move after hauler deposit (move + deposit concurrency)

**Status:** Proposed
**Scope:** `screeps-ibex/screeps-ibex/src/jobs/utility/haulbehavior.rs`, `screeps-ibex/screeps-ibex/src/jobs/haul.rs`, `screeps-ibex/screeps-ibex/src/transfer/transfersystem.rs` (signature only), `screeps-econ-eval/src/runner.rs`
**Related:** ADR 0040 (unified e/t market), ADR 0007 Q5 (confirm-then-consume, re-match cadence), ADR 0044 (routed haul distance)

> Path note: the working tree is `screeps-ibex/screeps-ibex/src/...` (nested crate dir).
> All references below use that prefix; earlier drafts wrote `screeps-ibex/src/...` — the
> line numbers land correctly, only the directory prefix was off.

---

## 1. Root cause — tick-by-tick timeline of the wasted move

A hauler that finishes a delivery does not move toward its next target until the following
tick. The lost tick is a deliberate `return None` in `tick_delivery`, not an engine or
intent-flag limitation.

Timeline of a completing single-sink delivery (state = `Delivery`, `run_state_machine` loop at
`machine_tick.rs:5-21`):

| Tick | State | What `tick_delivery` does | Move issued? |
|---|---|---|---|
| **N** (arrival + deposit) | `Delivery` | Creep adjacent to the last sink. `creep_transfer_resource_amount` succeeds (`haulbehavior.rs:576`), `consume_deposit` drains the entry (`:577`), `TRANSFER` bit set (`:578`), `transfered = true` (`:580`). `while` exits (tickets drained). **`if transfered { None }` (`:593-598`)** returns `None`; the machine loop terminates for tick N. `Idle` is never entered. | **No** — MOVE pipeline was free (bit 0) but nothing consumed it. |
| **N+1** | `Delivery` → `Idle` → `Pickup`/`Delivery` | `tick_delivery` finds no tickets → `transfered = false` → `Some(HaulState::idle())` (`:600`, from `haul.rs:293`). Same-tick loop chains into `Idle::tick` (`haul.rs:56-178`), runs the full market cascade, chains into the selected state's `tick`, which issues the first `move_to`. | **Yes** — one tick late. |

Net: `Deliver(N, transfer only) → reselect+move(N+1)`. One move lost per completed delivery.

The guard's stated reason (`haulbehavior.rs:595`): *"Delay further execution by a tick as
inventory cannot be trusted."* This is real: `creep.transfer(...)` is a deferred game intent;
the creep's `store()` is not updated until end of tick, so any same-tick re-selection that
reads `store()` would size against phantom cargo.

**Multi-sink deliveries.** The `while` loop drains emptied tickets same-tick
(`else { tickets.remove(0) }`, `:588-589`), and the move to the *next* non-empty sink in
`self.deposits` is **already issued today** at `:551-563` (it queues a `move_to`, then returns
`None`). The wasted-move pathology is **only** the final-empty case: after the last sink empties,
`transfered == true` with an empty `tickets` vec is the sole path to line 593. So the reselect
closure (§3) is invoked at most once per whole delivery, and never while a reachable non-empty
ticket remains.

## 2. The underused engine capability — transfer + move in the same tick

The engine explicitly allows `move` and `transfer` in the same tick:

- `docs/references/engine-mechanics.md:76` / `:415` — *"`move`, `drop`, `transfer`, `withdraw`,
  `pickup`, `pull`, `say`, `suicide` have no conflicts."*
- The bot's own `SimultaneousActionFlags` encodes this: `MOVE = 1` (bit 0, `actions.rs:25`) and
  the logistics `TRANSFER = 1 << 4` (bit 4, `actions.rs:46`) are **distinct bits**; `consume()`
  (`actions.rs:60-70`) inserts iff not already present, and only blocks re-use of the *same*
  pipeline.
- Crucially, **today's deposit path never touches MOVE**: once adjacent, the move branch
  (`haulbehavior.rs:551`, `if !creep_pos.is_near_to(pos)`) is skipped, so only `TRANSFER` is
  inserted (`:578`). MOVE is genuinely free for a same-tick reselect.
- The `IntentRecorder` (`intents.rs:38-46`) guards only combat pipelines — no MOVE/TRANSFER
  category, no part in this path, no recorder change.

So the concurrent move+deposit is representable today with **zero new flag**. The only thing
forcing the delay is the `None` at `haulbehavior.rs:598` and its genuine stale-store concern.

## 3. Proposed FSM change

**Core idea:** on the deposit tick, instead of returning `None`, run the next-target selection
immediately against a *projected* store (carried cargo and free capacity adjusted for what was
just transferred), reserve the next tickets in this tick's `TransferQueue`, transition straight
into the next `Pickup`/`Delivery` state, and let the same-tick `run_state_machine` loop emit that
state's `move_to` while the MOVE pipeline is still free.

The `HaulState` enum is **unchanged** — no new variant, no new field (`haul.rs:28-39`,
serialized shape frozen → no WFV bump; see §5). States touched:

- **`Delivery`** (`haul.rs:271-295`) — its `tick` stops routing the completed-delivery case
  through `Idle`; instead it passes a reselect closure to `tick_delivery`.
- **`Idle`** (`haul.rs:56-178`) — the market cascade it runs is extracted into a reusable helper
  so `Delivery` can call the **exact same** selection (identical determinism, identical booking).

### 3.1 Thread a projected `(free, carried)` pair through `tick_delivery`

**Ship-blocker #1 (corrected from an earlier draft).** `select_market_pickup_and_delivery`
takes **two** independent capacity scalars — `free_capacity` **and** `carried_energy`
(`transfersystem.rs:1888-1889`), passed from `get_new_market_pickup_and_delivery_state` at
`haulbehavior.rs:179-180`. A partial deposit changes **both**: carried drops by
`deposited_total`, free rises by `deposited_total`. Projecting only free capacity would leave
`carried_energy` stale, and the **loaded-hauler branch sizes its next delivery against
`carried_energy`** (`:180`, `:189`) — reintroducing a phantom-cargo variant on the delivery
side. The projection must therefore carry **both** quantities, not a lone free-capacity override.

`tick_delivery` already tracks the drained amount per entry (`consume_deposit(resource, amount)`,
`:577`). Accumulate the accepted total:

```rust
// haulbehavior.rs, inside tick_delivery
let mut transfered = false;
let mut deposited_total: u32 = 0;               // NEW: sum of ACCEPTED transfers this tick
...
if let Some((resource, amount)) = ticket.get_next_deposit() {
    if !tick_context.action_flags.intersects(SimultaneousActionFlags::TRANSFER) {
        if ticket.target().creep_transfer_resource_amount(creep, resource, amount).is_ok() {
            ticket.consume_deposit(resource, amount);
            tick_context.action_flags.insert(SimultaneousActionFlags::TRANSFER);
            transfered = true;
            deposited_total = deposited_total.saturating_add(amount);   // NEW: accepted only
        } else {
            ticket.consume_deposit(resource, amount);                   // rejected: own slot only
        }
    } else {
        return None;
    }
}
```

Change the terminal branch so it hands control to a caller-supplied closure that receives the
accepted total (from which the caller derives the full projected `(free, carried)` pair):

```rust
pub fn tick_delivery<F, G, R>(
    tick_context: &mut JobTickContext,
    tickets: &mut Vec<TransferDepositTicket>,
    bid_cargo_value: bool,
    next_state: F,                       // Fn() -> R   (empty-vec / nothing-transferred path)
    on_deposit_complete: G,              // NEW: Fn(u32 /*deposited_total*/) -> Option<R>
) -> Option<R>
where
    F: Fn() -> R,
    G: Fn(u32) -> Option<R>,
{
    ...
    if transfered {
        // Was: None (defer a tick). Now: caller may reselect same-tick using a PROJECTED store,
        // since creep.store() is stale until end of tick.
        on_deposit_complete(deposited_total)
    } else {
        Some(next_state())
    }
}
```

For the **military/dismantle caller** (`bid_cargo_value = false`) and any other caller wanting
today's semantics, `on_deposit_complete` is `|_| None` — a pure preservation of current behavior,
keeping the military salvage lane frozen as its comment requires (`haulbehavior.rs:526-530`).

### 3.2 Add a projected-capacity override to the market selection

Add an override to `get_new_market_pickup_and_delivery_state` (`haulbehavior.rs:146-195`). It
currently reads **both** capacities from the live store (`:163-164`):

```rust
let free_capacity   = creep.store().get_free_capacity(None).max(0) as u32;   // :163
let carried_energy  = creep.store().get_used_capacity(Some(ResourceType::Energy));   // :164
```

Introduce a `ProjectedStore` override that supplies **both** projected values; when `None`
(the `Idle` path) behavior is byte-identical to today:

```rust
/// Some(..) on a deposit-tick reselect: both quantities projected for the transfer that has
/// been issued this tick but is not yet reflected in creep.store(). None from Idle (live store).
pub struct ProjectedStore { pub free_capacity: u32, pub carried_energy: u32 }

pub fn get_new_market_pickup_and_delivery_state<...>(
    ...,
    projected: Option<ProjectedStore>,   // NEW
) -> Option<R> {
    let (free_capacity, carried_energy) = match projected {
        Some(p) => (p.free_capacity, p.carried_energy),
        None => (
            creep.store().get_free_capacity(None).max(0) as u32,
            creep.store().get_used_capacity(Some(ResourceType::Energy)),
        ),
    };
    // ... unchanged: passes free_capacity, carried_energy into
    //     select_market_pickup_and_delivery (transfersystem.rs:1888-1889)
}
```

The caller (§3.4) derives the pair from the pre-deposit store snapshot minus `deposited_total`:

```rust
// Delivery::tick, captured BEFORE tick_delivery runs (store still trustworthy at entry).
let free_before    = creep.store().get_free_capacity(None).max(0) as u32;
let carried_before = creep.store().get_used_capacity(Some(ResourceType::Energy));
// ... after deposit, inside on_deposit_complete(deposited_total):
ProjectedStore {
    free_capacity:  free_before.saturating_add(deposited_total),
    carried_energy: carried_before.saturating_sub(deposited_total),
}
```

> Only the **energy** channel is threaded as `carried_energy` because
> `select_market_pickup_and_delivery` already models carry as energy-only (`:1889`). Haul cargo
> in this crate is energy on the delivery lane; a non-energy delivery leg leaves `carried_energy`
> untouched by construction (`deposited_total` for a non-energy resource does not reduce the
> energy carry) — matched by the projection, which subtracts from the same energy scalar the
> selection reads. If a future non-energy haul lane needs projection, extend `ProjectedStore`
> then; today's contract is energy-carry parity.

### 3.3 Extract the selection cascade into `select_next_haul_state`

Pull the body of `Idle::tick` (`haul.rs:74-177` — the
`get_new_market_pickup_and_delivery_state(...).or_else(...)...` chain, tail
`.or_else(|| Some(HaulState::wait(cadence.backoff_ticks)))`) into a shared helper:

```rust
// haul.rs
fn select_next_haul_state(
    state_context: &HaulJobContext,
    tick_context: &mut JobTickContext,
    projected: Option<ProjectedStore>,   // Some(..) on deposit-tick reselect; None from Idle
) -> Option<HaulState>
```

- `Idle::tick` calls it with `projected = None` → **byte-identical** to today (assert in test 3.5).
- The market head (`get_new_market_pickup_and_delivery_state`) receives `projected`; the
  `or_else` fallbacks (`get_new_delivery_current_resources_state`,
  `get_new_pickup_and_delivery_full_capacity_state`, the move-to-room scan, the `wait` tail) are
  reached only when the market assigns nothing. On the deposit-tick reselect these fallbacks read
  the live `store()` — which is stale — so they **must not size against store on the projected
  path**. Two sub-cases:
  - The common case: the market head returns `Some` and the fallbacks never run — safe.
  - Drained lane: the market returns `None`. Rather than let a store-reading fallback book
    against phantom cargo, the projected-path reselect **stops at the market head**: if the head
    returns `None` with `projected.is_some()`, return `Some(HaulState::wait(cadence.backoff_ticks))`
    directly (same tail the cascade reaches anyway), skipping the store-reading fallbacks. From
    `Idle` (`projected == None`) the full fallback chain runs exactly as today. This keeps the
    stale-store invariant total: **no `creep.store()` read on the deposit tick, in any branch.**

  Concretely, `select_next_haul_state` shapes as:

  ```rust
  let head = get_new_market_pickup_and_delivery_state(..., projected);
  match (head, projected) {
      (Some(s), _) => Some(s),
      (None, Some(_)) => Some(HaulState::wait(cadence.backoff_ticks)), // deposit-tick: no store reads
      (None, None) => head.or_else(/* full Idle fallback chain, live store */),
  }
  ```

### 3.4 Wire `Delivery::tick`

Replace the single call at `haul.rs:293`:

```rust
// haul.rs Delivery::tick  (repair drive-by above, :285-290, unchanged)
let creep = tick_context.runtime_data.owner;
let free_before    = creep.store().get_free_capacity(None).max(0) as u32;
let carried_before = creep.store().get_used_capacity(Some(ResourceType::Energy));

tick_delivery(
    tick_context,
    &mut self.deposits,
    /* bid_cargo_value */ true,
    /* next_state (empty-vec / nothing transferred) */ HaulState::idle,
    /* on_deposit_complete */ |deposited_total| {
        let projected = ProjectedStore {
            free_capacity:  free_before.saturating_add(deposited_total),
            carried_energy: carried_before.saturating_sub(deposited_total),
        };
        select_next_haul_state(state_context, tick_context, Some(projected))
    },
)
```

On the deposit tick this now returns `Some(Pickup{..})` / `Some(Delivery{..})` / `Some(Wait{..})`;
the machine loop re-enters that state the same tick and — for `Pickup`/`Delivery` toward a
non-adjacent target — issues `move_to` on the still-free MOVE pipeline. The `Idle` round-trip is
eliminated for the common completion path. `Wait` re-enters `Wait::tick` (`haul.rs`) → `mark_idle`
+ `tick_wait`, no move, no regression.

## 4. Booking / determinism / parity — the load-bearing corrections

### 4.1 This-tick booking comes from selection self-`register`, NOT `gather_data`

**Corrected mechanism.** `pre_run_job`→`gather_data` registers deliveries/pickups into the
`TransferQueue` **once, before any `tick` runs**, for the job's **current** state
(`Pickup::gather_data` `haul.rs:184-190`; `Delivery::gather_data` `:274-278`). When
`Delivery::tick` reselects mid-tick and produces a **new** `Pickup{deposits}` / `Delivery{deposits}`,
that new state's `gather_data` **does not run again this tick** — it already ran for the *old*
state. So the earlier draft's claim that "the resulting state's `gather_data` re-books this tick"
is wrong for *this* tick (it is true for *future* ticks only).

The only thing that books the reselected target **this** tick is the
`register_delivery`/`register_pickup` **inside** `get_new_market_pickup_and_delivery_state`
(`haulbehavior.rs:185`, `:192`). That self-registration is present and runs before control
returns and before the same-tick `move_to`. So **reserve-before-move holds — but because
selection self-registers, not because of `gather_data`.** An implementer must not "simplify" by
relying on `gather_data` to book the reselected target this tick.

### 4.2 Stale store — the invariant the whole change rests on

We never re-read `creep.store()` on the deposit tick. Capacities are captured from the store at
`Delivery::tick` **entry** (before `tick_delivery`), then projected by `deposited_total`. The
projected-path reselect stops at the market head and never falls into a store-reading fallback
(§3.3). This closes exactly the "inventory cannot be trusted" window named at
`haulbehavior.rs:595`. **Selection on tick N must consume the projected `(free, carried)` pair,
never the game store — in every branch.**

### 4.3 Double-booking / stolen target

No new stolen-target window is opened that `Idle` did not already have: the reselect runs the
**identical** helper against the same frozen-then-mutated per-tick `TransferQueue` that `Idle`
mutates, and all peer haul jobs' `gather_data` already hydrated the queue in `pre_run_job` before
any `tick`. Booking-before-move is atomic within the selection call. Verified in private soak
(§5).

### 4.4 Determinism + LIVE↔SIM parity — the biggest correction

**Ship-blocker #2 (corrected from an earlier draft).** The sim does **not** bake a symmetric
1-tick lag that this change merely mirrors. The sim has a different, **deeper** tick granularity:

- `step_worker` processes each worker **exactly once per tick** via
  `std::mem::replace(&mut worker.activity, Activity::Idle)` (`runner.rs:868`). There is **no
  same-tick state-chaining loop** — nothing analogous to `run_state_machine`.
- The hauler `Deliver` arm emits `Transfer` then sets `worker.activity = Activity::Idle`
  (`runner.rs:1202-1203`) — this is the **entry to a multi-tick chain**, not a symmetric 1-tick
  lag.
- `step_idle_market` selects a task but `return false; // Idle steps never move` (`runner.rs:877`)
  — the physical move lands the *next* tick.
- `travel_then` builds `Activity::Travel { trace, idx: 0, then }` (`runner.rs:1717-1719`); the
  **first move-step is consumed when the `Travel` arm next runs** (the following tick), or returns
  `Wait { until: tick + 5 }` on an unreachable target (`:1720`).

So live and sim are **already** out of lockstep on this exact timing, and the fence has tolerated
it because `fold_report` (`runner.rs:1726+`) hashes the **economic ledger** (`harvested`, spawn
charge, ...), **not** per-creep move counts. Consequences:

1. **`sim_is_deterministic_over_rounds` is NOT the parity gate.** It proves the sim is
   deterministic *against itself across rounds*; it is **blind** to sim-vs-live move-timing.
   Passing it does not validate the parity mirror. Keep running it (the reselect must still read
   only the id-ordered per-tick booking pool, so determinism-against-self must hold), but **do not
   claim it gates parity.**
2. **Mirroring the fix in the sim is a *reduction of the sim's own deeper lag*, not a symmetric
   mirror.** Reusing `step_idle_market` in the `Deliver` arm is insufficient by itself: it sets
   `Travel{idx:0}` and returns "no move", so the physical move still defers unless the `Deliver`
   arm **also advances the first trace step the same tick**. Otherwise the sim changes bookkeeping
   without changing move count, and the throughput assertion passes on one side but not the other —
   the very divergence we are trying to avoid.

The sim change (§5) therefore must: (a) run selection against **projected carry** in the
`Deliver` arm, (b) transition straight to `Travel`, **and** (c) advance the first trace step the
same tick to match the live loop's single reclaimed move. Parity is validated by a **direct
move-count / throughput diff** between pre-fix and post-fix runner, not by the determinism fence.

### 4.5 Partial deposit / mixed cargo

- **Partial deposit:** the engine caps a transfer to the sink's free capacity
  (`transfersystem.rs`), so `deposited_total` reflects only what landed. `projected.carried`
  stays > 0 and `projected.free` stays < full; the loaded-hauler branch re-targets a second sink
  same-tick with correctly-projected carry (this is exactly what ship-blocker #1 fixes). Fully
  drained → `carried == 0`, `free == full` → pickup path.
- **Rejected / mixed-resource entries:** `transfered` is set only on success (`:580`); a rejected
  entry consumes only its own slot (`:582`) and does **not** inflate `deposited_total`. Projection
  stays accurate under mixed accept/reject cargo.

### 4.6 Not always exactly one move reclaimed (co-located next target)

**Ship-blocker #4 (corrected).** If the reselected next target is a **pickup the creep is already
adjacent to** (loaded→empty→co-located pickup, or a sink and pickup on the same/adjacent tile),
`tick_pickup` (`haulbehavior.rs:452-467`) finds MOVE not needed (`:434` skipped when adjacent) and
hits the TRANSFER-already-set guard (`:454`, TRANSFER set by the deposit at `:578`) → `break None`
**without moving** — correctly, since pipeline D is spent this tick. So "one reclaimed move per
delivery" is **not universal**: the co-located-next-target case legitimately yields no move this
tick. The move-count assertion must tolerate this (§5, test 2).

### 4.7 Transition-count budget

`run_state_machine` caps at `MAX_STATE_TRANSITIONS = 20` (`machine_tick.rs:3`). Today a completed
delivery burns **0** transitions (returns `None`). After the fix the worst case is
`Delivery → (reselect) → Pickup/Delivery/Wait`, i.e. **~1–2** transitions per completion — far
under 20. The design changes the per-tick transition count from ~1 to a few; verify no scenario
chains a reselect into *another same-tick completion* (it cannot: the reselected `Pickup`/`Delivery`
target is non-co-located in the move case, and co-located pickup returns `None` immediately per
§4.6; a reselected `Delivery` toward a new sink is non-adjacent by selection or it would not have
been chosen as a move leg). Add a guard assertion in test 2 that transitions per tick stay bounded.

## 5. Test / validation plan

**Sim runner (deterministic, primary).**

1. **Deliver-arm reselect (`runner.rs:1165-1211`).** On the hauler deposit step, after emitting
   `Transfer`, run `step_idle_market`'s selection against this tick's `deposits`/`market_pass`
   using **projected carry** (`carried − transfer_amount`), then transition to `Travel` **and
   advance the first trace step this same tick** (per §4.4(c)). Update the `PostDelivery` market
   arm (`runner.rs:1213-1221`) consistently. Must preserve id-order processing (`runner.rs:384-401`,
   `:868`) — no new nondeterministic iteration.
2. **Move-count / throughput assertion (the parity gate).** A targeted runner test — one hauler,
   one source, one non-adjacent sink, N complete round-trips — asserts total moves for the
   post-fix runner drop versus the pre-fix baseline by **≤ N** (one reclaimed move per delivery,
   *tolerating* co-located legs that legitimately reclaim none, per §4.6), delivered throughput per
   1000 ticks rises, and per-tick transitions stay bounded (§4.7). This diff — not the determinism
   fence — is what validates live↔sim parity of the reclaimed move. Include one scenario with a
   **co-located next pickup** and assert **no** move is reclaimed there.
3. **Determinism fence (necessary, not sufficient).** Run `sim_is_deterministic_over_rounds` (the
   `#[ignore]`d eval lane, per MEMORY sim-determinism-fence). Bit-identical across rounds must
   hold — the reselect must consume only from the id-ordered per-tick booking pool. **Do not treat
   a green fence as parity validation** (§4.4(1)); it is blind to move timing.
4. **Throughput regression.** The standard econ-eval scenario H-metric must be ≥ baseline (no
   worse routing from same-tick reselection).
5. **Unreachable next-target (sim divergence watch).** `travel_then` backs off `Wait{tick+5}` on
   `trace == None` (`runner.rs:1720`), whereas live queues a move and lets the mover flail.
   Pre-existing for `PostDelivery`, but the fix now exercises it on the common completion path. Add
   a test asserting the throughput diff accounts for this back-off (it is ledger-invisible, so the
   fence tolerates it, but the move-count diff must not falsely fail on it).

**Live-parity check.**

6. Unit/integration test on `tick_delivery` + `select_next_haul_state`: on a completing-deposit
   tick, assert (a) `TRANSFER` is set; (b) MOVE is still consumable and a `move_to` toward the
   freshly-selected **non-adjacent** target is queued in `MovementData`; (c) selection used the
   projected `(free, carried)` pair, **never** `creep.store()` (assert no store read on the deposit
   tick, including in the drained-lane branch); (d) on a **co-located next pickup** target, MOVE is
   correctly **not** consumed (pipeline-D exhaustion, §4.6) — the reclaim must not assert a move
   that cannot happen.
7. **Military lane frozen.** Assert the military caller (`bid_cargo_value = false`,
   `on_deposit_complete = |_| None`) still returns `None` — old behavior preserved
   (`haulbehavior.rs:526-530`).
8. **Idle byte-identity.** Assert `select_next_haul_state(.., None)` from `Idle` produces the
   identical state/booking sequence as the pre-extraction `Idle::tick` (pure-extraction guard).
9. Full `cargo test` suite + the determinism fence green before any deploy.
10. **Deploy to private first**, soak, confirm hauler round-trip cadence tightens (no post-deposit
    idle tick in the movement trace) and no double-booking / capacity panics; MMO only on explicit
    operator go-ahead (per MEMORY deploy policy).
11. **WFV:** `HaulState` serialized shape is unchanged — no new variant/field (`haul.rs:28-39`,
    verified) → **no WFV bump on the live serialized shape**. Confirm before committing. The sim
    `Activity` enum is an **eval-crate** concern only (not WFV); confirm the runner change does not
    alter any serialized econ-scenario snapshot the fence baselines against.

## 6. Implementation steps + risk

1. Add `deposited_total` accumulation and the `on_deposit_complete: Fn(u32) -> Option<R>` closure
   param to `tick_delivery` (`haulbehavior.rs:532-602`); default all existing non-`Delivery`
   callers (notably the military dismantle caller) to `|_| None`. *(Low — additive; preserves
   current behavior everywhere until `Delivery` opts in.)*
2. Add `ProjectedStore` and the `projected: Option<ProjectedStore>` override to
   `get_new_market_pickup_and_delivery_state` so it sizes **both** `free_capacity` **and**
   `carried_energy` (`transfersystem.rs:1888-1889`) from the projection when `Some`
   (`haulbehavior.rs:146-195`). *(Medium — must audit both capacity reads at `:163-164`; missing
   `carried_energy` re-opens the phantom-cargo bug on the delivery side — ship-blocker #1.)*
3. Extract `Idle::tick`'s cascade into `select_next_haul_state(.., projected)`; `Idle` calls it
   with `None`; the projected path stops at the market head and returns `Wait` on a drained lane
   rather than entering a store-reading fallback (`haul.rs:56-178`). *(Medium — pure extraction
   plus the projected-path short-circuit; assert byte-identical Idle behavior, test 8.)*
4. Capture `(free_before, carried_before)` at `Delivery::tick` entry and wire the reselect closure
   (`haul.rs:280-295`). *(Medium — the behavioral change lives here.)*
5. Mirror in the sim: `Activity::Deliver` (`runner.rs:1165-1211`) + `PostDelivery`
   (`:1213-1221`) arms — reuse `step_idle_market` with projected carry, transition to `Travel`,
   **and advance the first trace step same-tick** (§4.4). *(Medium/high — parity-critical; must
   keep id-order determinism and actually reduce move count, not just bookkeeping.)*
6. Tests from §5; determinism fence; private soak.

**Risks.**

- **Phantom cargo (highest):** any residual `creep.store()` read in the same-tick selection path
  — including the fallback cascade — re-opens the guard's bug. Mitigation: project **both**
  capacities, short-circuit the projected path at the market head, and test 6(c) asserting no
  `store()` read on the deposit tick.
- **Parity/move-count drift (high):** the sim must *reduce its own deeper lag* and physically
  reclaim the move (advance the trace step same-tick), validated by a **direct move-count /
  throughput diff** (test 2) — **not** by the determinism fence, which is blind to this timing.
- **Booking mechanism misunderstanding:** this-tick reservation comes from selection self-
  `register` (`:185`/`:192`), not `gather_data` (which already ran in `pre_run_job`). Documented
  so an implementer does not rely on `gather_data`.
- **Double-booking:** mitigated by running the full booking selection helper before the move;
  verified in private soak.
- **Military lane regression:** avoided by `on_deposit_complete = |_| None` for
  `bid_cargo_value = false`, keeping the dismantle salvage lane frozen (`haulbehavior.rs:526-530`).
- **Co-located reclaim over-assertion (low):** the move-count assertion tolerates legs that
  legitimately reclaim no move (pipeline-D exhaustion, §4.6).

**Key references:** wasted tick `haulbehavior.rs:593-598`; deposit+TRANSFER `:574-587`; move
branch `:551-563`; both capacity reads `:163-164`; market selection `:146-195` /
`transfersystem.rs:1881-1892` (two scalars `:1888-1889`); pickup TRANSFER-exhaustion `:452-467`
(`:434`, `:454`); selection cascade `haul.rs:56-178`; `Delivery::tick` + call site `haul.rs:280-295`
(`:293`); `Pickup`/`Delivery` `gather_data` `:184-190` / `:274-278`; enum shape `haul.rs:28-39`;
flag independence `actions.rs:25` / `:46` / `:60-70`; same-tick loop + budget `machine_tick.rs:3`
/ `:5-21`; engine no-conflict `engine-mechanics.md:76` / `:415`; sim once-per-tick `runner.rs:868`
/ `:384-401`; sim Deliver→Idle `:1202-1203`; `step_idle_market` no-move `:877`; `PostDelivery`
`:1213-1221`; `travel_then` `:1717-1720`; fence hashes ledger not moves `fold_report` `:1726+`.
