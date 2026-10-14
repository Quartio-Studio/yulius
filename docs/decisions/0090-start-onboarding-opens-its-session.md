# 0090. Start onboarding loads in place and opens the onboarding task

## Status

Accepted. Specified in [SPEC-111](../../.ai/specs/SPEC-111-when-i-click-onboarding-i-gota-short-lag.md).
It amends the gate's action of [ADR-0076](0076-onboarding-is-mandatory-in-the-ui.md).

## Context

Start onboarding waits for the server to create the onboarding task and start its run, which takes
seconds. Meanwhile the gate kept its live button (events rebuilt it with a link to the new task),
and when the call returned it showed "Onboarding <name>" with **Follow onboarding**: the person had
to click again to reach the session they had just started. The person then pointed at that screen
(as anyone opening the sandbox while onboarding runs sees it) and said it should not exist.

## Decision

- Pressing Start onboarding turns the whole gate into a loading state (`aria-busy`, no action) that
  task events do not rebuild into a live button while the approval runs.
- When the approval succeeds the page opens the onboarding task, as starting any task opens it.
- When it fails the gate comes back as it was, Start onboarding focused, with an error toast.
- While a sandbox is `preparing`, its new-task page opens the onboarding task in its place (a
  history `replace`), whatever the task's state; the task page says whether it runs, waits for
  capacity or stopped. There is no Follow onboarding or Open onboarding screen.
- The page speaks in words only when there is no task to open: the task list could not be read,
  or the onboarding task is missing (the administrator's recovery path of ADR-0076).

## Alternatives

- **A busy button only.** The press would no longer look repeatable, but the screen would still
  stop on Follow onboarding, which the person asked to remove.
- **Navigate on the first task event.** Faster by the length of the run's start, but it would race
  an approval that fails after the task exists, and leave the person on a task that never started.
- **Make the approval return before the run starts.** A server change that moves the start's
  failures out of the response the person waits for.
- **Keep the `preparing` screen for others.** It is the screen the person showed; the task page
  already says everything it said, and one more click.

## Consequences

- One click from the gate to the onboarding session, and opening a sandbox during onboarding lands
  on that session. New task (the shortcut, the sidebar) for such a sandbox opens it too.
- Back from the onboarding task returns to the page before the sandbox, not to a page that
  redirects again.
- The gate keeps a `starting` flag in its render key; a new rebuild trigger must respect it.
