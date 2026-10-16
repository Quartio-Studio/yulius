# 0155. A task removes its own files with `yulius remove`, judged by where they are

## Status

Accepted. Specified in [SPEC-192](../../.ai/specs/SPEC-192-claude-sessions-asks-me-to-delete-someth.md);
task #177.

## Context

A person found task sessions asking them to delete things. Claude Code's built-in removal check
runs even with `--dangerously-skip-permissions`: an `rm` of a root-level path (`/ci6.log`), of a
path held in a variable it cannot resolve, or inside a `sh -c` script asks a person, who sees the
prompt in the session terminal; it times out after two minutes and the agent is told to leave the
removal to them. Transcripts of tasks #5, #89, #114, #118, #121, #124 and #134 show it. The strays
at `/` came from `$TMPDIR/…`: agent runs had no `TMPDIR`.

The person asked for a Yulius tool that does the removal, "if only it deletes files created by
this specific task". In a contained run (ADR-22 of SPEC-002) `/`, `/tmp`, `/run` and `/dev/shm` are
the run's own fresh tmpfs, which only the run's processes see: neither the Yulius server, which
serves the MCP tools, nor a person can reach `/ci6.log`. The sandbox directory and the system
layer are shared by every task of the sandbox; the task's worktree is its own.

## Decision

- **A shell tool in the run.** `yulius remove <path>…` runs in the agent's run (the `yulius`
  executable is bound into every run, as for `yulius wait`), so it sees the run's own filesystem.
  The briefing names it next to the MCP tools.
- **Ownership by location.** A path is the task's when it is an untracked or ignored path of the
  task's worktree (`YULIUS_TASK_WORKTREE`, set by the launch; not the worktree, not `.git`, git
  tracks nothing at or under it), or a path on the run's own tmpfs mounted at `/`, `/tmp`, `/run`
  or `/dev/shm` when `/` is a tmpfs (a contained run). The root's links containment makes
  (`/bin`, `/sbin`, `/lib`, `/lib64`), mount points and directories holding one are refused, so a
  removal never empties another filesystem. Everything else is refused with the reason and
  where Yulius removes; the agent leaves it and names it in its summary.
- **No following.** The path's directory is resolved, its last component never: a link goes as a
  link.
- **`TMPDIR=/tmp`** for every task run, so `$TMPDIR/x` is no longer `/x`.

## Alternatives

- **An MCP tool.** It runs in the Yulius server, which cannot see a run's private tmpfs, so it
  could not remove `/ci6.log`, the case that prompted the request. Reaching the run through
  `/proc/<pid>/root` would tie the server to the run's process layout and its permissions.
  Rejected.
- **A record of the files the task created.** Write and Edit calls could be recorded by a hook,
  but files made by shell commands (`> /ci6.log`) cannot be told reliably from the command text.
  Location proves ownership without guessing. Rejected.
- **A `PreToolUse` hook that allows the `rm`.** Claude Code says this check "cannot be auto-allowed
  by permission rules", and it is not documented to yield to a hook; relying on it would break
  silently on an update. Rejected.
- **Removing anything the run can write.** The sandbox's home, system layer and other tasks'
  worktrees are shared; the request limits removal to the task's own files. Rejected.

## Consequences

- An agent removes its strays without a person, in its worktree and in its run's own tmpfs. A file
  it made in the sandbox's shared areas (home, `/usr`, `/etc`, `/var`) is refused; there `rm` is
  rarely stopped by Claude Code, and when it is the agent names it in its summary.
- A session that was running before this change has no `YULIUS_TASK_WORKTREE` until it starts
  again: `yulius remove` then removes only paths of its own tmpfs and says the worktree is
  unknown.
- Claude Code flagging a `sh -c` script that holds no removal is not changed: there is nothing to
  remove, and agents already move such scripts into files.
- The briefing's tools section drops its `yulius wait` sentence, which "Git and delivery" already
  gives, to stay within the briefing's budget.
