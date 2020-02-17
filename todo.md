# High Priority

- Use correct screeps-game-api crate once owner fix is merged in to master.
- Add lost creep recovery - i.e. memory is lost.
- Add time limiting to scout mission - don't keep running if creep can't complete objective. Don't keep spawning waves.
- Rampart priorization to prevent decay needs fixing.
- Add pre-pass to operations/missions/jobs to gather information.
1. Gather haul requests/providers/state.
2. Gather visibility requests and missions in progress to gather visibility.
- Add hauling requests and in-progress deliveries.
- Add remote mining capability.
- Add remote claiming ability.
- Add scout mission to get visibility using a creep.
- Add build priority bucketing.

# Medium Priority

- Add chrome tracing format profiling.
- Add fast entity lookup based on room name, global object ID, etc.
- Add observer support to visibility requests. (Currently just used for triggering room data generation.)