# High Priority

- Fix partial hauls due to damage causing return to harvesting source. (Stay stick with delivery once started.)
- Remote upgrade mission/role.
- Add stuck detection and response for creeps.
- Add lost creep recovery - i.e. memory is lost.
1. Gather haul requests/providers/state.
2. Gather visibility requests and missions in progress to gather visibility.
- Add remote mining container building.
- Add road system - gather nodes and generate connectivity.
- Computer number of hauler/harvester parts needed based on path distance.
- Factory usage.
- Boost usage.
- Add CPU analysis.
1. Prevent additional remote mining, reserving or claiming of new rooms without sufficient CPU.
- Post-process for room planner. (Remove roads not needed, fix RCL for links etc. based on distance, prioritize storage.)
1. Apply RCL as post-process with constraints. (i.e. do extensions by distance, don't spawn extractor container till RCL 6 etc.)
- Spawn body calculation using current available energy needs to use at least min body cost, otherwise never ends up in queue. (Will not block lower priority spawns!)
- Order system needs better analysis of price history and hard guards on price manipulation.
- Squad system for military. (Goals?)
- Military units.
- Shared, predicted storage capacity for the tick. (Allow for haulers to not wait for a tick at the end of their delivery tickets.)

# Medium Priority

# - Pathfinding solution. (Use built in path finder.)
- Add per-room stats (i.e. energy available over X minutes) to use for predicting needed roles.
- Allow scouts to find multiple rooms. (Use a goal system?)

# Low priority

- Add market statistics that can be used to drive buy/sell price.
- Use generator for spawn queue to compute only on-demand.

----

- Cleanup and game references in screeps-rover to use only external interface vs game calls.


- Foreman should likely depend on the same pathfinding code as Rover. Can this be factored in to common lib/crate?
- Need to correctly implement 3 range rampart requirement? I saw some comments previously this wasn't fully implemented.
- [DONE] Factor scoring in to separate functions/types so that they can be easily added/removed/modified - Scoring split into 6 individual layers with configurable weights.
- [DONE] The previous system used scoring for search space exploration - Search tree architecture with exhaustive exploration and score-based pruning.
- [DONE] The core step assumes that the set of stamps to use is constant - Generic StampLayer allows custom stamps via PlannerBuilder::add_layer().
- [DONE] Which of the phases can be provided as custom layers? - All phases are layers; PlannerBuilder provides append-only add_layer() API with default_layers().
- [DONE] Look for usages of hard coded constants (e.g. RCL level) - RclAssignmentLayer assigns RCL as post-process; stamps use rcl=0 for "assign later".


---

[DONE] The plan should likely assume final RCL so a complete plan is generated, RCL should then be attached to WHEN the placement should occur. -- Implemented as RclAssignmentLayer: assigns RCL values as post-process based on structure count limits and distance to hub.

[DONE] Ensure that if an item is required (e.g. link must be placable next to source but can't) it causes plan failure. -- source_infra, controller_infra, and extension layers now return Err(()) on mandatory placement failure.

[DONE] Stamps could be 'generic' in that a layout is defined and then tested for valid placement based on requirements. -- Generic StampLayer created; stamps support optional placements via `required` field; Stamp::validate() added.

[DONE] The anchor layer almost certainly needs to evaluate more than one placement as it will drive most of the rest of the scoring/placement. -- Already handled by search tree architecture. AnchorLayer now generates one candidate per valid position; hub stamp is placed by a separate StampLayer.

[DONE] Separate out scoring in to its own layers for each type so they can be parameterized. -- FinalScoreLayer split into 6 individual scoring layers (HubQualityScoreLayer, UpgradeAreaScoreLayer, ExtensionScoreLayer, TowerCoverageScoreLayer, UpkeepScoreLayer, TrafficScoreLayer) placed at earliest viable positions in the stack.

---

Container miners need to anchor to their container location, or need to look for a container for their source to use if it gets built after they have started mining.