# docs/implementation/

**Implementation documents — the resume state for work that is currently in flight.**

## The three-tier document model

Each tier answers exactly one question. Keeping them separate is what stops status drift: on
2026-08-22 a code-grounded pass found **29 of 56 ADR headers stale**, because `Status:` conflated
the *design's* maturity with the *code's* progress. One field, two lifecycles, always wrong.

| Tier | Answers | Lifetime |
|---|---|---|
| [`../design/`](../design/) — ADR | *What are we building, and why?* | Permanent. Never mentions progress. |
| **`./` — implementation doc** | *Where am I, and what is the next action?* | **Ephemeral** — created when work starts, deleted when it closes. |
| [`../execution/implementation-tracker.md`](../execution/implementation-tracker.md) | *What is in flight across the whole project?* | Permanent index, one line per item. |

## Rules

1. **An implementation doc is keyed to a WORKSTREAM, not to an ADR.** Work does not decompose one
   ADR at a time — combat Wave B spans ADR 0008a and the 2026-07-09 review; shipping WFV 28 spans
   ADRs 0046, 0038 and 0017. A workstream usually *is* one ADR, but the doc must not break when it
   isn't. List every ADR it advances in the header.
2. **Do not pre-create them.** Most ADRs need no implementation doc — either the work is done or it
   has not started. A parked gap lives as a one-line entry in the tracker §6. It becomes a document
   only when someone picks it up.
3. **Delete on completion.** Append one line to the tracker changelog citing the final commit, then
   delete the file. Git holds the history; a graveyard of finished plans is exactly the bloat this
   model exists to prevent.
4. **The ADR never gains status, and this doc never gains design.** If you are explaining *why* a
   mechanism works, that belongs in the ADR. If you are recording *where you got to*, it belongs
   here.
5. **`Design deltas` is mandatory when the design changes.** Discoveries made while building must
   be written back into the ADR, and noted here as confirmation. This is what keeps the end-state
   document actually true — otherwise the real design ends up living in a chat log.
6. **Keep it under ~120 lines.** If the plan needs more, it is more than one workstream.

## Template

```markdown
# <Workstream name> — implementation

**Workstream:** WS-N · **Advances:** ADR 00NN, ADR 00MM · **Status:** active | parked | blocked

## Resume point
The one section that must always be current. Last action taken, current state, and the
LITERAL next command or edit. Write it as if for someone who has never seen this work.

## Target
One or two sentences. What "done" looks like. Link the ADRs for the actual design.

## Plan
- [ ] Step, independently verifiable
- [ ] Step

## Design deltas
Decisions taken while building that changed the end state, and confirmation the ADR was
amended. "None so far" is a valid entry.

## Verification
The gates this must pass before it is considered done.

## Log
- YYYY-MM-DD — one line per session.
```

## Currently live

See the tracker §1 and §3. As a matter of policy the tracker holds **exactly one active
workstream**, so this directory should normally contain one or two files.
