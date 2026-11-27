# 0466. A finished task whose background work waits hears its staging outcome

## Status

Accepted for SPEC-631. Narrows ADR-0450's rule that a finished task gets no machine prompt.

## Context

On 23 November quartio-v2 task #377 merged with its plan complete, asked for staging with
`cd_deploy`, and ended its turn with a background shell waiting for the verdict. Staging verified
the revision. Yulius recorded the outcome on the task's delivery without typing it, because a
running task with a complete plan and a merged delivery gets no machine prompt (ADR-0450). The
task was not Done either: an idle agent's running background shell keeps it Working, since that
shell wakes the agent when it ends. The shell waited for news Yulius had decided not to type, so
the task read "running (waiting)" until its owner closed it by hand.

## Decision

When a task's staging request ends, Yulius still records the outcome silently on a finished
task, except while that task is idle and its agent runs background work (a background shell,
agent or Monitor, or a `yulius wait`): then it types the outcome into the session once, adding that the
task's work is finished, so the agent stops a command that waits for it, leaves anything else
running and answers in one line. A finished task that is not idle (mid-turn, or flagged by a
hold) is decided once it is idle again. A fresh look at the agent's processes decides, falling
back to the last probe. The usual holds apply (a person's possible draft, a needs-you request, a
pause, an update) and defer the outcome to a later tick, and the admission check right before
the paste confirms the task is still finished and the work still runs. Claude Code and Codex
share this path (only Claude Code is tested end to end).

A staging request's deploy is recorded as its task's CD run (`task:<n>`), deployed only while the
task's owner has write access, as before.

## Alternatives

- Always type staging outcomes into finished tasks: wakes sessions that are Done, which ADR-0450
  removed after finished tasks were woken by every verdict.
- Show the task Done while its shell runs: the shell would still wake the agent later, and the
  sidebar rule that background work keeps a task Working would hold for every other shell but
  this one.
- End the finished task's background shells: Yulius cannot tell a wait from a dev server it
  should not kill, and a killed command still wakes the agent with its failure.
- Make `yulius wait` follow staging requests and tell agents to use it, instead of typing the
  outcome: helps agents that use it, not the loops they write themselves, which this incident
  had. Since SPEC-678 (#1727) the wait follows the request as well, beside this decision, and the
  `cd_deploy` receipt points to it; the typed outcome stays for the other loops.

## Consequences

A finished task with no background work stays untouched; one whose own wait outlives the verdict
gets one prompt and then reads Done once its turn ends with nothing running. An agent that does
not stop its wait still keeps the task Working, as any background shell does. CD runs, `cd_status`
and the audit log name the task for staging deploys, and the task's `yulius wait` sees that run.
