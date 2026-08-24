# 0048 — Multi-squad assault doctrine (siege operations)

- **Status:** Draft
Date: 2026-08-24.
Origin: Phase 4.5 item 8 (RULING-9: "both directions — siege clamp lift first for L2–L3, the
multi-squad doctrine as the follow-on"). The clamp lift was built and **measured red**
(tracker §3 item 8a; the machinery is landed and wired off at `member_cap_for`), which moves the
doctrine boundary down: **the multi-squad operation is the L2+ path**, not just L4–L5.

## 1. The measured evidence this design rests on

All from the honest-verdict gauntlet (2026-08-24, `Killed` = core razed, not defender-wipe):

1. **Sizing is not the blocker — mass tactics are.** With a siege cap of 16 the optimizer
   correctly fields an L2@T3 assault (p_surv 0.82, p_kill 1.0; the whole 6-gate battery stays
   green with the lift active). The 16-blob then loses at every terrain: it **congeals** in the
   choke corridor (resolver denial loop, parked t330→timeout), and in the open it **strings out**
   in transit (rear fighters heal-gate-frozen at entry while the vanguard is picked off serially
   by tower focus) or **rigid-body parks** at the tower-threat edge once the out-of-contact
   handoff returns control to the kernel.
2. **Deliverable heal is adjacency-bound and does NOT scale with member count.** Full-rate heal
   lands from range ≤1 on ONE focused member; ~5 healers is what a formation actually delivers
   (the WS-VAL item-1 finding, now formalized in `optimizer_ceiling_budget_capped`'s
   deliverable-heal ceiling). Members past the classic 3+5 buy kill speed only. This is the
   structural reason 8 is the single-squad optimum — and the reason a SECOND squad is
   qualitatively different from 8 more members: **each squad brings its own 5 deliverable
   healers and its own focus**, so deliverable sustain scales with squad count.
3. **Every validated behavior is per-squad.** Cohesion, the border bloc gate, rout-to-rally,
   the dance damper, deliverable-heal advance gating, the EXP register beds — all tuned and
   pinned at ≤8 members. A 2×8 operation keeps that entire envelope; a 16-blob leaves it.

## 2. Decision (proposed)

A **siege operation** is N coordinated standard squads (N=2 first light), each an ordinary
≤8-member squad running the existing validated lifecycle, plus a thin operation layer that owns
exactly four coordination points. No new movement/positioning machinery per squad.

### D1 — Sizing: the joint requirement split across squads

Reuse the item-8a machinery as the *joint* sizer: when the standard cap-8 optimizer defers on a
siege, re-run `optimize_composition_capped` at `N × 8` **with the deliverable-heal ceiling per
squad** (healers = 5·N, fighters scale — the capped ceiling already models this shape at cap 16:
5 healers held + fighters scaled is *wrong* for one blob but *right* per squad when the force is
split 8+8 with healers balanced). The winning joint composition is split into N squads by a
deterministic round-robin over roles (each squad gets its healer share first, then fighters), so
every squad is independently viable (own sustain, own fighters). If even the N-squad joint sizing
defers, the operation defers — the honest L4/L5 red until N grows.

### D2 — Phased commit: the operation staging gate

The border bloc gate, generalized one level up: **no squad enters the threat radius until every
squad in the operation is assembled at its staging point** (staging = outside the stronghold's
tower falloff, per-squad points a few tiles apart). This is the same principle that fixed the
trickle-crossing serial pick-off at the border, applied to the operation: the defense never gets
to fight the squads one at a time. Implementation shape: an operation-level `ready` predicate
(all squads' members within GATHER_RADIUS of their staging point) gating the per-squad
engage release — one shared kernel function, mirroring `rally::members_gathered_at`.

### D3 — Lanes: distinct approach corridors and assault arcs

Each squad gets a distinct approach lane and a distinct assault arc around the core (e.g. the
breach-geometry helper already computes approach lines; assign arc sectors by squad index,
deterministic). This halves corridor congestion (the choke congeal) and spreads tower focus
across two heal pools. Squads do not path through each other's lane; the resolver sees the other
squad's members as ordinary same-owner traffic (which ALSO makes the sim finally exercise parity
M14's cross-squad contention — the sim must run ONE resolver pass per side for this, closing
that finding as a by-product).

### D4 — Focus and retreat at the operation level

- **Focus:** lane-local targeting by default (each squad fights what its arc reaches — tower
  focus can't be split-healed anyway); the operation only overrides to a SHARED focus when one
  squad's focus is a gate structure both need down (the breach rampart).
- **Retreat:** a squad's rout is an operation event: the operation reassesses jointly (remaining
  force vs the honest requirement). If the remainder can't win, ALL squads withdraw to staging
  (no lone squad left to be picked apart — the operation-level twin of win-or-stall). The
  existing per-squad give-up clocks and budgets stay untouched underneath.

## 3. What this deliberately does NOT do

- No new per-member positioning machinery — the kernel/mover/bloc machinery is used as-is.
- No formation-of-formations: squads coordinate through the four points above only.
- No N>2 first light: L4/L5 stay honest-red until 2×8 is validated at L2/L3.
- The single-squad siege cap stays 8 (`member_cap_for` unchanged); the item-8a lift machinery
  becomes the JOINT sizer's engine rather than a mono-blob enabler.

## 4. Validation bars (the gauntlet, honest verdicts)

1. `stronghold-L2-{open,choke,choke-multi}#s1 @T3` → **Killed** by a 2-squad operation.
2. `stronghold-L3-*#s1 @T3` → Killed (expected reachable at 2×8 with T3 output; if honestly red,
   record the sizing trace and stop — N=3 is a follow-on decision, not a silent escalation).
3. Every existing pin stays green (floor pin, border end-to-end kills, oscillation ≤ bars,
   drain soak, EXP register, 1537 workspace, determinism fence).
4. Live: no behavior change until the operation layer is feature-gated on
   (`features.military` flag, dark-first per the boost-pipeline pattern).

## 5. Open questions for the operator review

1. **Ownership:** does the operation live as a new entity owning N squad objectives (clean
   lifecycle, more plumbing), or as N sibling objectives tagged with an operation id inside the
   existing war-objective queue (less plumbing, coordination reads by tag)? Proposal: the tag
   route first — the four coordination points are all *reads* over sibling squads.
2. **Spawn economics:** a 2×8 T3 operation is ~2× the spawn+boost cost of anything fielded so
   far; should the R_net band/ledger treat the operation as ONE bid (the objective's completed
   value already covers it) or per-squad bids? Proposal: one bid, split settlement.
3. **Live rollout:** private-harness soak first (the Docker lane is unblocked), or straight to
   the watched MMO flag flip per RULING-8? Proposal: harness soak — this is the largest new
   coordination surface since the border gate.
