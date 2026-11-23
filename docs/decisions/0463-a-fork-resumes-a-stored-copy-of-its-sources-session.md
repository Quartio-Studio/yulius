# 0463. A fork resumes a stored copy of its source's session

## Status

Accepted on 2026-11-22 for task #612. The owner approved Fork task on 22 Nov 2026 after a review
of iTermX, whose Clone Tab runs `claude --resume <ID> --fork-session`. Specified in
[SPEC-628](../../.ai/specs/SPEC-628-fork-a-task-a-new-task-continues-from-th.md).

## Context

A person wants to try another approach from where a task stands without losing what it knows and
without disturbing it. Each task has its own worktree, so its Claude Code transcript is in a
project folder named after that worktree. Claude Code 2.1.330 continues a copy of a session under
a new id with `--resume <id> --fork-session`. It takes `--session-id <new>` only together with
`--fork-session`, and it also finds a session kept in another project folder. Codex 0.170 has
`codex fork <id>`, which names the new session itself. The Antigravity CLI 1.5.12 forks only with
`/fork` typed into a running session.

## Decision

- A fork is a new task of the same project and harness. Yulius records the source's id, number,
  title, branch, the branch's commit and its latest session in `task_forks` (migration 0092). The
  source's id becomes null when the source is deleted.
- For Claude Code, the complete lines of the source's transcript and its side files are copied at
  fork time to `<data>/transcripts/<fork id>/`. At the fork's first launch the copy is placed in
  the fork worktree's project folder with every `cwd` rewritten, and the run is
  `claude --resume <source> --fork-session --session-id <new>`. Yulius names the new session, so
  the fork's lineage never holds the source's id.
- For Codex, the first launch is `codex … fork <source> -- <opening>`. The new session id is
  adopted from the hook or the rollout, as for a new Codex session.
- Antigravity tasks are refused.
- The launch is `SessionMode::Resume { fork }`. The opening message is the first turn, a goal is
  set after it, and the transcript is read from the run's start.
- The worktree is a new branch at the recorded commit. Uncommitted work stays behind.

## Alternatives

- **Let Claude Code find the source's own file.** The fork would read the live file when it
  launches, which can be later than the recorded commit. The history would keep the source's
  `cwd`, and the two sessions' files would be open at the same time.
- **A new session with a written summary of the source.** This loses the context, which is the
  problem the fork exists to solve.
- **Rewrite a Codex rollout under a new id.** Codex has no documented format for that. Its own
  `fork` is the supported way, at the cost of reading the rollout when the fork starts.
- **Type `/fork` into a second Antigravity instance.** That instance would first open the
  source's live conversation, which the CLI itself warns against.
- **Fork columns on `tasks`.** Every task literal would change for an optional fact. A link table
  is what chains already use.

## Consequences

- The fork's own usage excludes the copied history: Claude Code keeps the copied records'
  timestamps, and a resume is read from the run's start.
- A Codex fork that waits for capacity sees turns its source took after the recorded commit.
- The stored copy stays with the fork and is removed with it.
