# High Priority

- Remote upgrade mission/role.
- Add stuck detection and response for creeps.
- Use 'local supply' mission for both local supply and remote mine.
- Add lost creep recovery - i.e. memory is lost.
- Add time limiting to scout mission - don't keep running if creep can't complete objective. Don't keep spawning waves.
- Rampart priorization to prevent decay needs fixing.
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

# Medium Priority

- Add observer support to visibility requests. (Currently just used for triggering room data generation.)
- Pathfinding solution. (Use built in path finder.)
- Add per-room stats (i.e. energy available over X minutes) to use for predicting needed roles.

# Low priority

- Add market statistics that can be used to drive buy/sell price.