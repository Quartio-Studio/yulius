# 0367. An idle session is parked and resumed by a message

## Status

Accepted (2026-11-19). Specified in
[SPEC-512](../../.ai/specs/SPEC-512-marcin-s-five-issues-1412-1418-1413-1107.md) part 1; task #494,
issue #1412. Extends SPEC-050 SES-101 (Done tasks' sessions do not count toward the limit); keeps
[ADR-0050](0050-a-run-ends-only-when-its-host-is-proven-gone.md) and
[ADR-0111](0111-agents-ask-a-person-with-needs-you.md) as they are.

## Context

- A task whose turn ended keeps its Claude Code process until a person closes, archives or stops
  it. Only Done tasks' sessions were left out of `max_sessions` (SES-101); a task that waits for
  a person's answer, which may take days, held a slot. On 19 Nov 2026 13 of 21 live sessions on
  one instance belonged to finished tasks; on another, three started tasks stayed queued behind
  finished ones, with nothing on them saying why.
- Every idle process holds memory: 37 idle Claude Code processes used 9 GB of an 11 GB host.
- Closing is not the answer: a closed task leaves the list's Needs you and Done groups for
  Ended, its request is hidden (ADR-0111) and only Continue brings it back.
- Claude Code (and Codex, Antigravity) resume a recorded session with its history; Yulius already
  does so after a usage limit and on Continue.

## Decision

- A task whose turn ended and that waits for a person is **parked** after a grace period
  (`YULIUS_PARK_AFTER_MINUTES`, default 30, 0 for never): its session ends gracefully, as a close
  ends it, and the task takes a new status, `parked`, instead of `closed`. It keeps its activity,
  its `needs_you`, its worktree, its claims, its delivery and its place in the list (Needs you or
  Done, labelled "parked").
- Only an idle task with nothing that goes on by itself is parked: no background shell or
  subagent, no `yulius wait`, no armed merge or pull request news due, no held prompt, browser
  request, person's pause, possible draft or recent keystroke, and never a question in its
  terminal (`needs_attention`), nothing in Claude Code's queue, and never the onboarding task. The
  background work is looked at afresh first (no answer counts as work), and no park starts in the
  first three minutes after a start. The decision is taken under the
  launch lock against the stored task.
- `parked` is neither live (no process, no slot, no subscription) nor resting (it has not ended).
  It leaves by a resume (`starting`, or `queued` at the session limit), a close or archive
  (`closed`) or a stop (`stopped`).
- A message resumes it: `task_message` launches the same provider session with the message as its
  first prompt and ends the request; the web's Resume launches it with nothing typed and keeps the
  request until the person writes. Only who may steer the task resumes it.
- A queued task says what it waits for (`queue_reason`), and the Capacity card shows working,
  parked, idle and queued sessions apart.

Capture the attempt ID in the task read that sees Parked, then check it at launch. Another turn can start and park during account or permission checks. Its outstanding decision belongs to that later turn.

Continue on a closed, stopped or failed task carries the same attempt identity and the archive state the owner saw. A run can already appear ended while its reader is finishing a park. Continue waits for the terminal cleanup bound, then accepts the launch or queue entry in the same transaction that removes the archive and clears the old question. A later archive or replacement run wins if it was recorded first.

## Alternatives

- **Count only working sessions, keep the processes.** Frees slots, not memory: the small host of
  #1412's comment would still run out.
- **Close finished tasks automatically.** Moves them to Ended and hides their requests: the
  person loses the list that tells them what waits for them.
- **A `running` task without a process.** The restart reconciliation (ADR-0050) takes a running
  task with no live run for a lost one, and every reader of `running` would have to learn the
  difference.
- **A flag on `closed`.** `closed` means a person ended it; chains, the Needs you count and
  ADR-0111 read it so.
- **A grace period in Settings.** Settings' limits are sized from the host; this number is not.
  The unit's variable is enough until a person asks for more.

## Consequences

- A night of finished tasks frees its slots and memory without anyone closing them; the person
  still finds them where they were.
- A person who writes to a parked task waits for a session start (seconds) instead of typing into
  a live one; a goal-mode task gets its goal typed again, as after any resume.
- Every reader of task statuses learns `parked` (the web's `status.js`, `tasks_list`, the API).
- Parking needs nothing from a person: it is on by default, unlike what widens who can do what.
