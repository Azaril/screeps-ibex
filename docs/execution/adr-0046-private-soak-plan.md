# ADR 0046 private-server soak plan (2026-08-22)

> **Why this exists.** ADR 0046 is merged to master at **WFV 28** with 320 green tests, clean wasm
> build/clippy, and a passing `sim_is_deterministic_over_rounds` fence — but it has **never run
> against a live world**. This is the run that closes that gap. The MMO deploy is deliberately
> **held** until this soak is clean (operator decision 2026-08-22; the live MMO empire is healthy at
> 7 rooms, so there is no urgency forcing the reset).
>
> Written to be mechanical: preconditions, exact commands, what PASS looks like per criterion, the
> failure signatures that matter, and the rollback.

---

## 0. Precondition — start the Docker service (blocked 2026-08-22)

The soak could not start because **`com.docker.service` (Docker Desktop Service) is `Stopped`, with
`StartType: Manual`**. Docker Desktop's GUI, its 25 named pipes, and the `docker-desktop` WSL2
distro can all be up while this service is down — and the symptom is that `docker ps` **hangs
indefinitely** rather than erroring, which is easy to misread as a slow cold start.

Starting it needs Administrator. From an **elevated** PowerShell:

```powershell
Start-Service com.docker.service
```

To stop it recurring, set it to start automatically (also elevated, one time):

```powershell
Set-Service com.docker.service -StartupType Automatic
```

Then confirm the engine actually attaches before going further — do not trust the GUI whale icon:

```bash
docker ps          # must RETURN, not hang
wsl -l -v          # docker-desktop must show Running
```

> Diagnostic note for next time: `docker ps` hanging + backend processes present + pipes present +
> `docker-desktop` distro `Stopped` ⇒ check `com.docker.service` first. Restarting Docker Desktop,
> `wsl --shutdown`, and booting the distro by hand all fail to fix it, because none of them can
> start a privileged service without UAC.

---

## 1. Bring up the stack and deploy

```bash
cd screeps-server-kit
cargo run -- server up
```

Then deploy master (from the repo root):

```bash
cargo run --manifest-path screeps-pack/Cargo.toml -- deploy --server private-server
```

**Expect a loud reset.** Master is WFV 28 and the private world was last written at WFV 27, so the
bot must discard its serialized world model and rebuild. That is the intended behavior and is
itself part of what is being tested — the post-reset re-scout is ADR 0046's highest-risk untested
path.

---

## 2. First-tick health gate (before any behavioral judgement)

Tail the bot console:

```bash
cd screeps-server-kit
cargo run -- console --user ibex --seconds 120
```

| Check | PASS | FAIL means |
|---|---|---|
| Version banner | a decode-mismatch/loud-reset line, then normal ticking | — |
| Panics | **zero** `panic`, `unwrap`, `RuntimeError` | stop; do not proceed |
| Deserialization | no repeated deser errors after the first reset tick | the WFV bump did not clear cleanly |
| Tick continuity | tick advances every second | VM wedged |
| Bucket | recovering, not pinned at 0 | CPU blowout in the new assignment pass |

A reset costs a rebuild of room plans, mission state and intel — transient noise in the first few
hundred ticks is expected. Sustained error spam is not.

---

## 3. Success criteria (ADR 0046 §5, made checkable)

Let the soak run; `system.setTickDuration(100)` (or lower) to fast-forward. Judge after the bot has
had at least one full discover cycle (~840–5000 ticks).

### C1 — the unreachable list does not re-poison
The reset wipes the list (no migration, by design — D2.4). The test is whether it **stays** clean.

- **PASS:** no rooms 1 hop from a colony appear in the `unreachable` list. The old list peaked at
  103 rooms; anything approaching that is a hard fail.
- **Check:** the offline world decoder —
  `IBEX_WORLD_PAYLOAD=<segs 50-52> IBEX_NOW=<tick> cargo test -p screeps-ibex decode_live_world -- --ignored --nocapture`
- **Why it matters:** this is the defect ADR 0046 exists to remove structurally (F5 — the per-room
  `ScoutMission` counting *spawns* while scouts picked targets globally). If it re-poisons, the
  redesign did not land its central claim.

### C2 — the stale-intel skip disappears from Select
- **PASS:** `ClaimOp [Select]` captures no longer show `failed commit-time safety re-check,
  skipping` for top candidates — scouts keep candidate intel fresh enough to pass
  `intel_freshness_ticks`.
- **Check:** `console --user ibex --grep ClaimOp --seconds 300`
- **Baseline to beat (live MMO, 2026-08-11):** 11 scored candidates, zero missions created; #1 and
  #2 both lost to the stale-intel skip.

### C3 — scouts tour, and the fleet tracks demand
- **PASS:** scouts visibly walk multi-room routes rather than pinning; fleet size responds to
  frontier size instead of sitting at the old `MAX_SCOUT_MISSIONS = 3`.
- **Check:** `console --user ibex --grep "ScoutAssignment|tour" --seconds 300` — the assignment pass
  logs fleet EV in e/t plus unserviced-entry and pending counts.

### C4 — the self-pin does not return (the regression that started all this)
- **PASS:** no scout remains in the same room across consecutive assignment passes while that room
  is its tour head.
- **Guarded in code by** `entry_needs_service` + its two RED-verified pins
  (`occupied_room_never_needs_service`, `imperative_entry_still_excludes_the_occupied_room`).
  This soak check is the live confirmation of what those tests assert offline.

### C5 — a claim actually fires
- **PASS:** at least one claim mission is created and a room is claimed during the soak.
- This is the end-to-end proof; C1–C4 can all look right while something downstream still blocks.

---

## 4. Failure signatures worth naming in advance

| Signature | Likely cause | Action |
|---|---|---|
| Bucket drains steadily after reset | the new assignment pass is too expensive at full frontier size (it is `SkipUnderCritical`, so it should shed — verify it does) | profile before any MMO deploy |
| Scouts idle with demand outstanding | tour build returning empty — check `build_tours` budget/lifetime gating | fix before MMO |
| `unreachable` list grows past ~20 | C1 failed; the room-centric evidence rule (resolution #2, ~100 ticks adjacent-not-inside) is mis-tuned | fix before MMO |
| Claims fire but at close rooms only | below-ring patience (ADR 0038 D9) interacting with L3's commit window — an expansion-policy issue, not a 0046 defect | note, do not necessarily block |

---

## 5. Rollback

Nothing on the private server is precious, but if the soak has to be abandoned mid-run:

```bash
git checkout wfv27-deployable-e857c76
cargo run --manifest-path screeps-pack/Cargo.toml -- deploy --server private-server
```

That tag is the last no-reset-from-27 point (expansion Wave 1 + L1 + L3, no ADR 0046). Returning to
master afterwards is another loud reset — expected, and free on private.

---

## 6. What to record before considering MMO

Capture these so the MMO decision is made on evidence rather than vibes:

1. Ticks soaked, and whether any panic/deser error appeared after the reset settled.
2. C1–C5 verdicts, each with the console/decoder line that supports it.
3. CPU and bucket trend at steady state, compared against the live MMO baseline
   (**CPU 18.5/140, bucket 10000 flat, 7 rooms, GCL 12, tick 4,871,333** — captured 2026-08-22).
4. Any tuning changed during the soak (and whether it needs a WFV bump — most 0046 constants do not).

**Then, and only then,** decide the MMO reset. Live is healthy and expanding on WFV-27 code, so a
good trigger is a *reason* — e.g. GCL 13 making the room cap bind — not merely "the soak passed."
