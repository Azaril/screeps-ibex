# High Priority

- Fix partial hauls due to damage causing return to harvesting source. (Stay stick with delivery once started.)
- Remote upgrade mission/role.
- Add stuck detection and response for creeps.
- Use 'local supply' mission for both local supply and remote mine.
- Add lost creep recovery - i.e. memory is lost.
1. Gather haul requests/providers/state.
2. Gather visibility requests and missions in progress to gather visibility.
- Add remote mining capability. (Static + container mining needed. Switch from remote harvesting.)
- Computer number of hauler/harvester parts needed based on path distance.
- Add heuristics for which rooms to claim next. (Number of sources, source proximity, amount of swamp, etc.)
- Factory usage.
- Lab usage.
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

- Pathfinding solution. (Use built in path finder.)
- Add per-room stats (i.e. energy available over X minutes) to use for predicting needed roles.
- Allow scouts to find multiple rooms. (Use a goal system?)

# Low priority

- Add market statistics that can be used to drive buy/sell price.
- Use generator for spawn queue to compute only on-demand.