---
name: ""
overview: ""
todos: []
isProject: false
---

# Transfer System Visualization and Stats Plan (revised)

## 1. Snapshot strategy: Option B (dedicated system)

Use a **dedicated `TransferStatsSnapshotSystem**` that runs at the right point: **after** transfers have been gathered (jobs/missions have run and populated the queue) and **before** the queue is cleared or mutated.

- **Order in dispatcher**: Run `TransferStatsSnapshotSystem` immediately **before** `TransferQueueUpdateSystem` (e.g. dependencies: `&["spawn_queue"]` so it runs after jobs/spawn; `TransferQueueUpdateSystem` depends on `&["transfer_stats_snapshot"]`).
- **Snapshot system**:
  - **SystemData**: `viz_gate: Option<Read<VisualizationData>>`, `transfer_queue: Write<TransferQueue>`, `transfer_stats_snapshot: Option<Write<TransferStatsSnapshot>>`.
  - When `viz_gate` is `Some` (visualization on), build the snapshot from the transfer queue: iterate all rooms via `get_all_rooms()` and for each room call `get_room_no_flush(room)` to get `&TransferQueueRoomData`, then aggregate from `room.stats()` into `TransferRoomSnapshot` (totals and by-priority; optionally by resource). Write the result into the `TransferStatsSnapshot` resource (create or overwrite).
  - The snapshot system **does not** call `transfer_queue.clear()`; it only reads. `TransferQueueUpdateSystem` continues to clear as it does today.
- **Why Write****: Building the snapshot requires iterating rooms and calling **`get_room_no_flush(room)`, which takes `&mut self` on `TransferQueue`. So the snapshot system must have `Write<TransferQueue>`. It uses that access only to read current state; clearing remains the responsibility of `TransferQueueUpdateSystem`.

**Room transfer overlay**: Defer until after the room-level aggregate visualization (panel under storage) is complete. No room overlay in this phase.

---

## 2. Snapshot types and capture

- **Types** (e.g. in `transfer/transfersystem.rs` or `transfer/stats.rs`):
  - `TransferRoomSnapshot`: per-room, one tick — e.g. `supply_total`, `supply_pending`, `supply_by_priority: [u32; 3]` (High/Medium/Low), `demand_total`, `demand_pending`, `demand_by_priority: [u32; 3]`. Optionally: `supply_by_resource: HashMap<ResourceType, u32>` and same for demand (capped to top N or energy + “other” to avoid clutter).
  - `TransferStatsSnapshot`: `HashMap<RoomName, TransferRoomSnapshot>`.
- **Building**: Implement `TransferQueue::snapshot_for_visualization(&mut self) -> TransferStatsSnapshot` that iterates `get_all_rooms()`, for each room uses `get_room_no_flush(room)` and aggregates `TransferQueueRoomStatsData` (withdrawl/deposit stats and pending) into the above shape. Call this from `TransferStatsSnapshotSystem` when viz is on.
- **Resource**: `TransferStatsSnapshot` world resource; when viz is on, the snapshot system overwrites it each tick. `AggregateSummarySystem` and (later) history system read via `Option<Read<TransferStatsSnapshot>>`.

---

## 3. Visualization design: placement and content

**Placement**: Draw the transfer visualization **underneath the storage history** (i.e. below the existing “Storage” sparkline in the center of the room view). Reuse the same horizontal band (e.g. `STATS_SPARKLINE_TOP_Y + STATS_SPARKLINE_H + GAP`) so the transfer block sits directly under the storage block.

**What to show** (current-tick panel / summary):

- **Supply vs demand** (primary): Show both so imbalance is obvious (e.g. “all storage requesting energy but small amounts available”).
  - **Scale imbalance**: Supply and demand can differ by orders of magnitude. Prefer one or more of:
  - **Two separate small sparklines** (e.g. “Supply” and “Demand”) with independent Y scales so each is readable.
  - **Text summary**: e.g. “Supply: 12k / Demand: 450k” so the numbers make the imbalance clear even if a single shared-scale chart would flatten supply.
  - **Normalized or dual-axis** only if you keep both series readable (e.g. “Supply” line and “Demand” line with different colors and clear labels; dual Y-axis is possible but can be confusing in a small widget).
- **By priority**: Breakdown by High / Medium / Low helps see where demand is urgent vs background. Show as:
  - Short text line: e.g. “S: 2k/5k/5k (H/M/L)  D: 100k/200k/150k” or “Supply H:2k M:5k L:5k | Demand H:100k M:200k L:150k”.
  - Or a tiny stacked/grouped representation (e.g. three small segments per of supply and demand) if space allows.
- **By resource / type**: Many resource types make a single “all resources” series noisy.
  - **Aggregated views**:
    - **Energy vs non-energy**: Two series (energy total, minerals/other total) keeps the chart readable and matches common gameplay (energy is the main flow).
    - **By priority** (above) already gives a high-level view.
    - **Top N resources**: If you show per-resource at all, limit to e.g. top 3–5 by amount (energy first, then next by volume) and label “Energy”, “Oxygen”, etc.; treat the rest as “Other” to avoid clutter.
  - Prefer **one or two clear metrics** in the main widget (e.g. “Supply vs demand (energy)” or “Total supply vs total demand” with a single sparkline pair) and use the **text line** for priority breakdown and maybe “Energy / Other” so the panel stays usable.

**Recommended default for “under storage”**:

1. **Text line** (one or two lines) directly under the storage sparkline:
  - Line 1: e.g. `Transfer  Supply: 12k (H:2k M:5k L:5k)  Demand: 450k (H:100k M:200k L:150k)`.
  - Line 2 (optional): `Pending  S: 1k  D: 8k` or by priority if useful.
2. **Optional small sparkline(s)** below that:
  - Either **two tiny sparklines** (Supply / Demand) with separate scales, or
  - **One dual-line sparkline** (supply + demand, different colors, same or dual Y-axis) for “total over time” if you add historical transfer stats later.

This keeps the main view readable, handles supply/demand imbalance via numbers and (if used) separate sparkline scales, and avoids drowning the widget in many resource types by focusing on totals and priority (and optionally energy vs other).

---

## 4. Feeding into VisualizationData and RenderSystem

- **RoomVisualizationData**: Add e.g. `transfer_stats: Option<TransferRoomSummary>` (or reuse `TransferRoomSnapshot`). Fill it in `AggregateSummarySystem` from `TransferStatsSnapshot` (per room).
- **RenderSystem**:
  - After drawing the storage sparkline for a room, compute the Y position for the transfer block (e.g. `transfer_block_y = STATS_SPARKLINE_TOP_Y + STATS_SPARKLINE_H + GAP`).
  - Draw the transfer text (and optional small sparkline) in the same panel style (rect, font, truncation) as the rest of the UI. Use the “text line(s) + optional sparkline” layout described above.
- **Aggregation for display**: When building the text/sparkline, use the room’s `TransferRoomSnapshot`: totals and `*_by_priority`; if you add resource breakdown, use energy vs other or top-N from the snapshot so the visualization stays aggregated.

---

## 5. Historical transfer stats (optional, later)

- Can mirror `stats_history`: dedicated segment, downsampled tiers, and a sparkline for “supply vs demand” or “unfulfilled” under the transfer text. Use the same placement rule: below the transfer current-tick block. Same backward-compatibility rules (`#[serde(default)]`, defaulted fields).

---

## 6. Implementation order

1. Add `TransferRoomSnapshot`, `TransferStatsSnapshot`, and `TransferQueue::snapshot_for_visualization(&mut self)` (no clear).
2. Add `TransferStatsSnapshotSystem` (viz-gate, `Write<TransferQueue>`, `Option<Write<TransferStatsSnapshot>>`); when viz on, build snapshot and write resource. Insert `TransferStatsSnapshot` resource when viz is on (e.g. in game_loop next to `VisualizationData`). Register system **before** `TransferQueueUpdateSystem` with dependency so snapshot runs first.
3. Add `transfer_stats` to `RoomVisualizationData` and fill it in `AggregateSummarySystem` from `TransferStatsSnapshot`.
4. In `RenderSystem`, add the transfer block **underneath the storage history**: text line(s) (supply/demand and optional pending, with priority breakdown); optionally two small sparklines or one dual-line for current tick only (no history yet).
5. (Later) Room transfer overlay after this is complete.
6. (Optional) Transfer stats history segment + sparkline below the transfer text.

---

## 7. Summary

- **Option B**: Dedicated `TransferStatsSnapshotSystem` runs after transfers are gathered, before clear; it has `Write<TransferQueue>` only to read room data and write `TransferStatsSnapshot`; `TransferQueueUpdateSystem` still clears.
- **Room overlay**: Deferred until room-level aggregate visualization is done.
- **Visualization**: Under storage; emphasize supply vs demand (with explicit numbers and/or separate scales); show priority breakdown; keep resource aggregation (energy vs other or top N) so individual types don’t dominate; use text for clarity and optional small sparkline(s) for trend.

