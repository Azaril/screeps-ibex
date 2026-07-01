# ADR 0037 — Tower-aware neighbour defense (suppress bare Secure + route towered rooms to offense)

- **Status:** Proposed (2026-07-01)
- **One line:** A `Secure` objective fires for a towered adjacent room with `dps=0`, fields a bare
  floor-sized defender, and sends it toward the towered room — pointless (a floor squad can never beat
  towers) with a thin dangerous tail (the count-quorum anchor advance can step a bare body across the border
  into tower range for ~1 tick before the winnability veto turns it back). Fix in three stages: make the
  neighbour threat tower-aware, suppress the bare defense reflex + harden the advance gate, and route a
  towered neighbour to the winnability-gated offense path.

## 0. Problem (live, MMO shardX)

`[War] Secure objective for NEIGHBOUR threat room W13N56 prio=75 (dps=0, adjacent=true)` for a room that has
**powered towers**. Root causes (traced):

1. **Armed/danger divergence** — a room is flagged only via a hostile *creep*: `hostile_warrants_defender`
   counts **Work | Claim | Heal | Attack | RangedAttack** as armed (war_decision.rs:221-225), but
   `estimate_danger` scores only **Attack (30) + RangedAttack (10)** (war_decision.rs:232-242). So a lone
   dismantler/claimer/healer → `armed=true, danger=0`. Priority is pure distance (dist=1 → HIGH 75); danger
   is only the within-band tie-break (war_decision.rs:92,120-124). Towers never enter it — they are
   structures, never in `hostile_bodies` (war.rs:449-459).
2. **Tower-blind neighbour danger** — the neighbour threat estimate has no tower term at all, so a towered
   room is sized as if empty (a `danger=0` floor composition, war.rs:577-601).
3. **The defender is aimed *into* the towered room** — `Secure{room: nbr}` → `SquadTarget::AttackRoom{nbr}`,
   target_room = the neighbour itself (squad_manager.rs:898-910,1789-1800).

**Behavior / verdict (traced):** the engage cascade (ADR 0034/0035) *mostly* protects it — the room is
scouted (that is why it emitted) so towers are in the DTOs → E1 classifies it contested → rally stages one
room short → `present_force_wins_or_stalls` reads `unwinnable=true` (tower DPS never decays to 0: the
`tower_amount_at_range` clamp is [5,20], ~150/tick even a room away) → Retreating + `lost_in_room` abandon. So
it does **not** stand-and-die. But: (a) **pointless** — a `danger=0` floor squad can never beat towers, burns
a spawn, ties up 1 of 4 concurrent squad slots, and re-emits every scan; (b) **thin dangerous tail** — the
winnability veto gates only the *fast-path*, **not the count-quorum anchor advance** (squad_manager.rs:2242-2243,
2392-2424), and a bare 1-slot defender meets its own count quorum, so it can step the lead member across the
border into tower range for ~1 tick before the retreat gate flips it — an unhealed body can occasionally die.
**LOW–MEDIUM severity: a resource/logic waste with a thin safety tail.**

## 1. Fix — three stages (each sim-first, committed separately)

### Stage T1 — Tower-aware neighbour threat signal (the enabling data)
Expose the neighbour's hostile-tower DPS as a signal on the observation. The raw read (war.rs:449-459) already
has the scouted `RoomData` for each neighbour; add its energized-tower count/DPS (reuse
`tower_attack_damage_at_range` / `RoomThreatData.hostile_tower_positions`+`tower_energy`, the same signal
offense uses) to `RawObservation`/`ObservedRoom` as a `tower_danger` (kept **distinct** from the creep
`danger`, because a *defender* must not be sized to beat towers). Pure-kernel: a towered neighbour's
observation carries the tower threat; a non-towered one carries 0. **No WFV bump** (additive pure input).

### Stage T2 — Suppress the bare defense reflex + harden the advance gate
- **Suppress:** in `observe_neighbours`/`emit_defense`, do **not** emit a `Secure` for a neighbour whose only
  threat is a `danger=0` creep under hostile towers (a non-attacking creep sitting in a towered/hostile-owned
  room is not attacking *us*; it becomes a real threat only if it enters our room, which fires its own
  owned-room Secure). Use the T1 tower signal to gate it.
- **Harden (general, closes the dangerous tail):** gate the **count-quorum anchor advance** on
  `present_force_wins_or_stalls` the same way the fast-path is gated (squad_manager.rs:2392-2424), so **no**
  unwinnable-sized force ever advances the anchor across a border into towers — not just this case. Scope so a
  genuinely-winnable contested assault is unaffected. **No WFV bump** (ephemeral gates).

### Stage T3 — Route towered neighbours to the winnability-gated offense path
A towered adjacent room that we might want to clear is a static **offense** problem, not a defense reflex.
When the T1 signal shows towers, hand the room to the offense/winnability oracle (`military::force_sizing` /
`RequiredForce`, which already computes energized-tower DPS + breach corridors, per ADR 0031/0035): **attack
it sized-to-the-towers if winnable, ignore it if unwinnable** — instead of a bare defender. Pure/objective-queue
plumbing, ephemeral. **No WFV bump.**

## 2. Interactions & consequences
- ADR 0034/0035 (engage cascade): T2's advance-gate hardening extends the winnability veto from the fast-path
  to the count-quorum advance — a general robustness win beyond this case. The abandon (ADR 0035 D4) already
  fires on contact; T2 stops the border crossing that precedes it.
- ADR 0031 (force-sizing oracle) / 0036 (structure targeting): T3 reuses the oracle's tower-DPS + the raze
  path — a winnable towered neighbour is cleared by an appropriately-sized offense that razes its towers/core.
- ADR 0027 (defense/objective lifecycle): T2 refines the neighbour-Secure emission gate; owned-room Secure is
  unchanged (a threat *in our room* still defends).
- **WFV:** none expected across all three (tower signal is an additive pure input; suppression + advance-gate
  are ephemeral; offense routing is objective-queue plumbing) — confirm at implementation.

## 3. Cross-references
ADR 0027 (objective/defense lifecycle), 0031 (force-sizing/winnability oracle), 0034 (rally/convergence),
0035 (engage cascade — uncontested/winnability/abandon), 0036 (structure targeting / raze).
