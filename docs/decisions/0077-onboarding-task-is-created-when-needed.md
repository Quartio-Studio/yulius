# 0077. A sandbox's onboarding task is created when onboarding starts, not at import

## Status

Accepted. Specified in [SPEC-097](../../.ai/specs/SPEC-097-i-tried-to-start-onboarding-via-cli-and.md).
Amends [ADR-0038](0038-onboarding-starts-in-the-browser.md): a `pending` sandbox no longer keeps a
resting onboarding task.

## Context

Every imported repository got its onboarding task at import. Adding it in the browser started the
task at once; `yulius onboard` (a bearer token) left the sandbox `pending` until someone clicked
Start onboarding (ADR-0038), with the task resting. A task created without a run is `closed`, so
right after `yulius onboard` the task list showed "Prepare this repository for Yulius · Closed ·
0/6 steps": a task that looked as if it had run and ended, though nothing had started (task #82).

## Decision

- Linking a repository creates no task. The sandbox is `pending` with no onboarding task.
- The task is created when it is needed: when Start onboarding starts it (a browser import does
  that at once), or when a new task must wait after it (IMP-010: new tasks in a `pending` sandbox
  wait after the onboarding task; it is created then, resting, and they chain after it).
- One lock serializes that creation, so Start onboarding and a waiting task never create two.
- What onboarding will do stays readable without the task: `GET /api/sandboxes/{sid}/onboarding/plan`
  returns the task's plan, or the one it would be created with, and creates nothing. The
  mandatory-onboarding page (ADR-0076) reads it.

## Alternatives

- Keep the resting task but archive it until onboarding starts. It would still be listed under
  archived tasks as Closed, and continuing it would un-archive it.
- Keep it and hide it from the lists and counts while the sandbox is `pending`. A special case in
  every list, count and search for one task, and the task still exists to be found elsewhere.
- Refuse new tasks in a `pending` sandbox without an onboarding task. Simpler, but it breaks the
  IMP-010 contract that new tasks wait instead of failing (scripts, automations, the MCP tools).

## Consequences

- After `yulius onboard` the task list is empty until someone starts onboarding or creates a task.
- A task created in a `pending` sandbox still shows the onboarding task it waits after, resting:
  the person can see what it waits for.
- Sandboxes already `pending` keep their resting task; Start onboarding starts it as before. A
  person may delete it; the next start or waiting task creates a new one.
