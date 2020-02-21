# High Priority

- Containers are not always placed when planning preventing container mining.
- Remote build (likely) seems to be creating a massive number of builders when supporting a newly claimed room.
- Add state machine system. Use it to drive operations/missions/jobs. (Requires entity vector fix or workaround.)
- Add stuck detection and response for creeps.
- Use 'local supply' mission for both local supply and remote mine.
- Add lost creep recovery - i.e. memory is lost.
- Add time limiting to scout mission - don't keep running if creep can't complete objective. Don't keep spawning waves.
- Rampart priorization to prevent decay needs fixing.
- Add pre-pass to operations/missions/jobs to gather information.
1. Gather haul requests/providers/state.
2. Gather visibility requests and missions in progress to gather visibility.
- Add hauling requests and in-progress deliveries.
- Add remote mining capability. (Static + container mining needed. Switch from remote harvesting.)
- Add remote claiming ability.
- Add build priority bucketing.
- Computer number of hauler/harvester parts needed based on path distance.
- Attach missions to operations as needed. (Requires entity vector fix or workaround.)
- Add heuristics for which rooms to claim next. (Number of sources, source proximity, amount of swamp, etc.)

# Medium Priority

- Add chrome tracing format profiling.
- Add fast entity lookup based on room name, global object ID, etc.
- Add observer support to visibility requests. (Currently just used for triggering room data generation.)
- Pathfinding solution. (Use built in path finder.)