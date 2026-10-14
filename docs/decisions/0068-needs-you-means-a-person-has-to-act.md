# 0068. "Needs you" means a person has to act; the agent's own work and waits come first

## Status

Accepted. Extends [0044](0044-task-status-follows-claude-codes-own-records.md),
[0061](0061-sandbox-counts-are-the-sidebar-groups.md) and
[0063](0063-background-work-and-goal-verdict-from-the-transcript.md); changes how SPEC-050
DSK-102's `yulius wait` shell reads in the sidebar. Specified in
[SPEC-085](../../.ai/specs/SPEC-085-status-truth.md). Point 2's tone is amended by
[0080](0080-a-wait-for-yulius-shows-a-still-muted-branch.md): the wait reads muted and still.

Amended by [SPEC-130](../../.ai/specs/SPEC-130-458-got-achieved-but-wasn-t-marked-as-do.md) (task
#117, #474): point 1 also holds when the plan is complete and the pull request is tested or merged.
A wait keeps the task Working until it returns, and only an achieved goal reads Done while a wait
runs (task #109 read Done while it waited for its armed merge).

## Context

A replay of two days of the sandbox's transcripts against the sidebar's rule showed that "Needs
you" was mostly false: 43 of its 50 task-hours were agents waiting in `yulius wait` for their
checks or armed merge, which DSK-102 keeps out of the background count so that disk upkeep and
DLV-203 treat the task as idle. Their plan's merge step was open, so the pull request's "In
review" did not apply either, and the task read "Your turn". Failing checks went before
background work, so an agent fixing its own CI failure, with a review agent still running, read
"Needs you" too. And an idle compaction's summary record, which Claude Code writes for itself,
was read as a prompt, so a Done task read Working until a person stopped it.

## Decision

1. **"Needs you" waits for the agent's own work.** An idle agent goes on by itself while its
   background work runs (shells, pending agents, live monitors, as before) or while it waits in
   `yulius wait`. Then the task is Working, whatever its pull request's checks or its plan say.
   Only an agent that stopped and cannot go on by itself, a question or permission prompt, gate
   exhaustion, a held prompt or a browser request need a person.
2. **`yulius wait` is counted apart, not as background work.** The process probe returns both
   counts from one walk. Disk upkeep, DLV-203, Done and the session limit keep reading the shells
   alone, as DSK-102 decided; only the status reads the wait, as "Waiting for checks or merge"
   with Working's tone, so it notifies nobody.
3. **Only prompts start turns.** A user record Claude Code writes for itself (a compaction's
   summary, a command's echo and output, the shell mode) is no prompt. Nor is `/compact` as typed:
   Claude Code compacts and runs no turn, so neither its line nor a prompt hook for it starts
   work (SPEC-085 STT-005). The model's own work shows a turn that follows any other command.

## Alternatives

- **Count `yulius wait` as a background shell.** DLV-203 merges dev into idle armed PRs, disk
  upkeep prunes idle caches and Done needs no shell left: each would stop working while an agent
  waits for its checks.
- **Track waits from the server's wait requests.** The CLI asks again every 50 s, so the server
  sees gaps between requests, and the status would need a grace period. The probe sees the
  process itself and already tells it apart.
- **Make an open goal alone keep a task Working.** A goal that Claude Code paused (nine blocked
  stops, a failing evaluator) is still "open" in the transcript; its task needs a person.
- **Skip every typed line that starts with `/`.** The raw `/compact` line is the only one seen,
  and the fake agent and older CLIs write prompts that way; the model's work already shows a
  command's turn. Only the compaction command is known to run no turn.
- **End a `/compact` "turn" when the compaction ends.** It needs a flag that the boundary, a
  failed compaction or the next prompt clears, in the right order, and the task reads Working
  for the minute the compaction takes although the agent does no work. Never starting one needs
  no state.

## Consequences

- A task waiting for its CI reads Working ("Waiting for checks or merge") until `yulius wait`
  returns; when the agent then stops, it reads Your turn or its pull request's state.
- Failing checks read "Working · PR #N checks failing" while the agent's background work runs;
  once it ended and the agent stopped, they need you as before.
- `/compact` leaves the task as it was, during and after the compaction.
- Yulius's own recovery of a full session reads Working until it types the prompt that carries
  on (SPEC-085 STT-006, CTX-002 changed): Yulius acts, nobody else has to. Left to a person (they
  typed into it), the task waits for them; a recovery that fails needs a person, as before.
