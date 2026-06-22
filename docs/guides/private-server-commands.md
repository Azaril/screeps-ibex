# Private-Server Console Commands (reference)

Quick reference for driving the local Dockerized Screeps private server — the
**fidelity oracle + soak gate** for combat (ADR 0006 / phase-2 §2.0). Sourced
from the Screeps wiki "Private Server Common Tasks" + screepsmod-auth README
(links at the bottom); cross-check live with `help(<object>)`.

## How to reach the server console

The world-server CLI (port **21026**) exposes the admin objects below. The bot
runtime console (`/api/user/console`) is separate — use it for `Game.*` /
`Memory.*` from the bot's perspective.

```
cd screeps-server-kit
cargo run -- cli "<js>"            # one admin command via the world CLI (port 21026)
cargo run -- cli                   # interactive admin REPL
cargo run -- exec --user ibex "<js>"   # one expr in the BOT runtime (Game/Memory)
cargo run -- console --user ibex --grep Squad --seconds 60   # live bot console tail
```

The CLI evaluates JS and **prints resolved promises**, so `storage.db[...].find(...)`
prints its result directly (do NOT wrap in `.then(console.log)` — that resolves
after the connection closes and prints `undefined`). `help(system)` /
`help(map)` / `help(strongholds)` / `help(bots)` list each section's commands live.

## system

| Command | Purpose |
|---|---|
| `system.pauseSimulation()` / `system.resumeSimulation()` | stop / start ticks |
| `system.getTickDuration()` | current tick duration (ms; default 1000) |
| `system.setTickDuration(ms)` | set tick rate — e.g. `100` to fast-forward a soak |
| `system.runCronjob("genInvaders")` | force NPC invader generation now |
| `system.resetAllData()` | **WIPE** the map + all user data (full reset) |

## map

| Command | Purpose |
|---|---|
| `map.generateRoom('W11N11', { sources: 2 })` | create a room |
| `map.openRoom('W5N1')` | make a room available |
| `map.openRoom('W5N1', Date.now() + 300*1000)` | schedule a room opening |
| `map.closeRoom('W5N1')` | disable room access |
| `map.updateTerrainData()` | refresh the terrain cache |
| `map.updateRoomImageAssets(roomId)` | refresh room images |

## strongholds (invader cores) — the combat-soak driver

Invader cores and strongholds are unified (since the 2019 NPC-strongholds
update). Spawn one to create a real offense target with ramparts + towers:

```
strongholds.spawn('W4N5', { templateName: 'bunker3' })   // level = the number in templateName (bunker1..bunker5)
help(strongholds)                                        // list all stronghold commands
```

`bunker1` = a bare low core; higher `bunkerN` = more ramparts + towers + a
tougher tower fight (the breach + tower-drain path). Pick a room **adjacent to
or reserved by our colony** so the bot scouts it and the offense scan produces a
`Dismantle` objective.

## bots (NPC AI players)

```
bots.spawn(botAiName, roomName, { name, cpu, gcl, x, y })   // all opts optional
bots.spawn('screeps-bot-tooangel', 'W7N4')
help(bots)                                                  // removeUser / reload / etc.
```

`opts`: `name` (default random), `cpu` (default 100), `gcl` (default 1), `x`/`y`
(default random spawn position).

## storage.db (direct world queries)

```
storage.db['rooms.objects'].find({ type:'invaderCore' }, { room:1, level:1 })
storage.db['rooms.objects'].find({ $or:[{type:'invaderCore'},{type:'keeperLair'}] }, { room:1, type:1 })
storage.db['rooms.objects'].count({ type:'invaderCore' })
storage.db['users'].find({ username:'ibex' })
storage.db['users.code'].find({}, { user:1, branch:1 })   // uploaded bot branches
```

## Users / auth (screepsmod-auth)

Username/password auth + the deploy endpoint (`POST /api/auth/signin` →
`/api/user/code`) come from **screepsmod-auth**. Password reset is **CLI-only**:

```
setPassword('ibex', '<password-from-.screeps.yaml>')   // Promise -> user object | false
```

- `setPassword` is a screepsmod-auth global. If it is **defined**, the auth mod
  loaded → a deploy `Cannot POST /api/auth/signin` (404) is a **password/state**
  issue, fixable here without a wipe. If `setPassword` is **undefined**, the auth
  mod did not load → it is a mods problem (a clean rebuild does NOT fix it — the
  launcher npm-installs the same broken version every boot).
- The world CLI (21026) needs no auth; only the REST deploy path does.
- The **authoritative diagnostic** is the backend's own log:
  `docker exec screeps-eval-launcher cat /screeps/logs/backend.log` — a mod that
  throws on load shows `Error loading ".../screepsmod-auth/index.js": ...`.

### Known break: screepsmod-auth 2.9.0 (pin 2.8.3)

`screepsmod-auth@2.9.0` (published 2026-06-16) **crashes on load** —
`TypeError: Cannot read properties of undefined (reading 'db')` at
`lib/index.js:123` — so neither `setPassword` (CLI) nor `POST /api/auth/signin`
(REST) registers, and `deploy` fails with a 404. Fixed by pinning the **installed**
version to `2.8.3` via the launcher's `extraPackages` (NOT `mods:` inline `@ver`,
which the launcher mangles to `@*`, and NOT `pinnedPackages`, which is yarn
*resolutions* and only overrides nested deps — `extraPackages` overrides the
top-level dep key in `package.json`). In `screeps-server-kit/config/server.yml`:

```yaml
mods:
  - screepsmod-auth        # stays here so it's loaded (mods.json)
extraPackages:
  screepsmod-auth: 2.8.3   # forces the installed version
```

Verify after a rebuild: `docker exec screeps-eval-launcher sh -c "grep version /screeps/node_modules/screepsmod-auth/package.json"` → `2.8.3`, and `POST /api/auth/signin` → 401 (route present), not 404. Re-evaluate when a fixed 2.9.x ships.

## Fast-forward shortcuts (testing / soak bootstrap)

Skip the economic climb when you need a combat-capable colony fast. RCL/GCL are
plain DB fields (wiki "Private Server Common Tasks"):

```
# Set a room's controller level (RCL) directly — the bot then builds the
# newly-unlocked structures over the next ticks (fast with an existing creep base):
storage.db['rooms.objects'].update({ room:'W7N4', type:'controller' }, { $set:{ level:4 } })
# (optional) set controller progress:
storage.db['rooms.objects'].update({ room:'W7N4', type:'controller' }, { $set:{ progress:10899000 } })

# Raise a user's GCL / GPL (the kit's `bootstrap` already raises GCL via config gcl:):
storage.db.users.update({ username:'ibex' }, { $set:{ gcl:38000000 } })
storage.db.users.update({ username:'ibex' }, { $set:{ power:540000 } })

# Run ticks faster while building up:
system.setTickDuration(25)   # ms; or utils.setTickRate(25)
```

Note: setting `level` only **unlocks** the higher structure caps; energy capacity
rises as the bot builds the extra extensions, so allow a few ticks before
expecting full-size combat bodies. Worth wiring an optional RCL bump into the
kit's `bootstrap` for combat testing.

**CAUTION — bump RCL once, EARLY, on a small colony.** The `controller.level`
record itself is not corrupted by a direct set, but **abruptly jumping multiple
levels (e.g. 2→7→8) on a built-up colony** triggers a chaotic construction burst:
the bot floods construction sites for all the newly-unlocked structures, and one
can land on a spawn's output tile — the spawn then stalls/cycles, unable to
release creeps (observed 2026-06-22: a constructionSite on Spawn1's output
direction froze spawning). Combined with any halt period (e.g. a panic loop) that
piles idle creeps around the spawn, the room ends up wedged. Prefer bumping a
single level on a fresh/small colony and letting it stabilize.

## Offense soak recipe (validate attack tactics)

1. Stack + bot: `server up` → `deploy --user ibex` (hot swap, same WFV = no reset).
2. Confirm a scoutable target exists near our rooms:
   `storage.db['rooms.objects'].find({type:'invaderCore'},{room:1,level:1})` —
   else inject one: `strongholds.spawn('<adjacent room>', {templateName:'bunker3'})`.
3. Fast-forward to combat-capable: bump the home controller to RCL4+ (see
   Fast-forward shortcuts above) and `system.setTickDuration(25)`; allow a few
   ticks for extensions to build before expecting full-size combat bodies.
4. Watch: `console --user ibex --grep "Dismantle|Secure|Squad|breach|War" --seconds 120`
   and the seg-57 cohesion canary. **Pass = a `Dismantle` objective is produced, a
   cohesive squad travels in, breaches the rampart, and the core is CLEARED**, with
   defense not starved under `MAX_CONCURRENT_SQUADS` (4).
5. Off-ramp / reset to peace: remove the target or `Memory._features.military.offense=false`.

## Sources

- [Private Server Common Tasks — Screeps Wiki](https://wiki.screepspl.us/Private_Server_Common_Tasks/)
- [Private Server Bot Development — Screeps Wiki](https://wiki.screepspl.us/Private_Server_Bot_Development/)
- [screepsmod-auth (README — setPassword)](https://github.com/ScreepsMods/screepsmod-auth)
- [screeps/screeps server README](https://github.com/screeps/screeps/blob/master/README.md)
