# 0090. Start onboarding loads in place and opens the onboarding task

## Status

Accepted. Specified in [SPEC-111](../../.ai/specs/SPEC-111-when-i-click-onboarding-i-gota-short-lag.md).
It amends the gate's action of [ADR-0076](0076-onboarding-is-mandatory-in-the-ui.md).

## Context

Start onboarding waits for the server to create the onboarding task and start its run, which takes
seconds. Meanwhile the gate kept its live button (events rebuilt it with a link to the new task),
and when the call returned it showed "Onboarding <name>" with **Follow onboarding**: the person had
to click again to reach the session they had just started.

## Decision

- Pressing Start onboarding turns the whole gate into a loading state (`aria-busy`, no action) that
  task and sandbox events do not rebuild while the approval runs.
- When the approval succeeds the page opens the onboarding task, as starting any task opens it.
- When it fails the gate comes back as it was, Start onboarding focused, with an error toast.
- The `preparing` gate with Follow onboarding stays for every other visit (another person, a reload,
  coming back) and as the fallback when the task cannot be found.

## Alternatives

- **A busy button only.** The press would no longer look repeatable, but the screen would still
  stop on Follow onboarding and the person asked for the screen to load.
- **Navigate on the first task event.** Faster by the length of the run's start, but it would race
  an approval that fails after the task exists, and leave the person on a task that never started.
- **Make the approval return before the run starts.** A server change that moves the start's
  failures out of the response the person waits for.

## Consequences

- One click from the gate to the onboarding session; the Follow onboarding message is seen only
  by people who did not start it on this page.
- The gate keeps a `starting` flag in its render key; a new rebuild trigger must respect it.
