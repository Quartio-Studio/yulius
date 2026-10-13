# 0053. The CLI's own continuation ends a limit pause; a refused limit switch is remembered per run

## Status

Accepted. Extends [0027](0027-step-back-past-a-usage-limit.md); specified in
[SPEC-072](../../.ai/specs/SPEC-072-fix-276-277-278-usage-limit-resume-respe.md) (#276, #277,
#278).

## Context

Decision 0027 resumes every turn a usage limit cut by a native resume past the limit. Three gaps
remained:

- Claude Code 2.1.292 sometimes continues the cut turn itself at the reset ("continuing
  automatically at …"). Its continuation prompt is hidden (`isMeta`), so Yulius saw only agent
  work. The resume loop also woke at the reset and terminated the run, because an automatic switch
  assumes an idle agent. A tool call in flight could be cut.
- When the automatic switch after a limit was refused in preflight (ROT-014), the task stayed
  running at its limit with no pause. A person's switch then sent the continuation prompt instead
  of resuming past the limit.
- The step back ran inside the synchronous `HarnessPort::plan`, on an async runtime worker, and the
  session index was reset even when nothing changed.

## Decision

1. An open `limit` pause ends as `overtaken` when the transcript shows a turn after the pause
   started: a prompt, an interrupt, agent work, or the session's own turn end. The task runs
   again. Before the resume loop ends a paused run, it checks that the pause is still open, and its
   switch requires the task to still wait for capacity. After a restart, a limit read again from
   the transcript is acted on only if no prompt, agent work or session turn end follows it, and a
   `limit` pause older than the last such record ends as `overtaken`.
2. The run's runtime remembers that its last turn ended at a usage limit (`at_limit`). A prompt,
   agent work or a turn's end clears it, and so does a new run. A person's switch resumes past the
   limit when the task is at a `limit` pause or `at_limit`.
3. The step back is its own port method (`step_back_transcript`). The supervisor runs it in
   `spawn_blocking`, and it reports whether it rewrote the file. The session index is reset only
   after a rewrite.

## Alternatives

- **Make a limit resume interrupt instead of terminate while a turn runs**: the session that is
  working would still restart, and an interrupt can cancel a tool call too.
- **Record a `limit` pause when a limit switch is refused**: the task would read "waiting for
  capacity" although capacity exists. The resume loop would also retry, every round, a switch that
  preflight refuses.
- **Read the limit from the runtime's `limit_resets_at`**: it is never cleared, so it cannot tell a
  task still at its limit from one that worked since.
- **An async `plan`**: the step back is the only blocking I/O in `plan` worth moving. A sync
  method run in `spawn_blocking` keeps the port and its test doubles simple.

## Consequences

- A turn the CLI continued by itself is not ended by Yulius once its first record is read. The CLI
  and the resume loop can still race at the reset. Ending the run before Claude Code's first
  record of the continuation is harmless: no tool has started, and the continued resume redoes the
  request. The harmful window is a tool call already started but not yet read: one transcript poll
  (about a second).
- A resumed attempt reattached after a restart is read from the end of its transcript. A
  continuation Claude Code wrote while Yulius was down is not seen, and the resume loop may end
  that run, as before. A first-run attempt is replayed from its start, so this case is covered.
- `at_limit` lives in memory. A restart sets it again only for a first-run attempt, whose
  transcript is replayed from the start, and only if the replayed limit is still pending. After a
  restart of a resumed attempt, a person's switch of a task left at its limit sends the
  continuation prompt, as before.
- The step back still has no timeout. It is file I/O bounded by the transcript's size, and a
  timeout would not stop a blocking thread anyway.
