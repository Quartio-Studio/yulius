---
yulius-task: c56a3715-1dcd-455d-8e70-cc45a6249e26
status: in-progress
---

# SPEC-192: A task removes the files it created itself, with `yulius remove`

Status: in-progress
Date: 2026-10-16
Owner: Yulius task #177
Scope authorization: task #177, requested by MrFishPL: "Claude sessions asks me to delete
something sometimes. However, it should have yulius tools to do that, if only it deletes files
created by this specific task."

## Goal

A task's agent never asks a person to delete a file the task created. It removes such files
itself with a Yulius tool, `yulius remove <path>…`, which removes a path only when it provably
belongs to this task: an untracked or ignored path in the task's own worktree, or a path in the
session's own temporary filesystem (`/`, `/tmp`, `/run`, `/dev/shm` of its sandbox run). Anything
else is refused with the reason, and nothing is removed. Agent runs also get `TMPDIR=/tmp`, so
`$TMPDIR/x` no longer lands at `/x`.

## Context

- Claude Code (2.1.x) has a built-in removal check that runs even with
  `--dangerously-skip-permissions`: an `rm` of a root-level path (`/ci6.log`), of a path held in a
  shell variable it cannot resolve (`rm "$TMPDIR/x"`), or inside a `sh -c` script, asks a person
  ("cannot be auto-allowed by permission rules"). In a Yulius task the prompt appears in the
  session terminal, waits two minutes, times out, and the agent is told to "leave the removal to
  them". Transcripts: tasks #114 (`rm -f /ci6.log /e2e6.log`), #124 (`rm /bg.rs`), #118 and #134
  (`sh -c` loops), #5, #89, #121 (`sessions_search "rm permission prompt approve"`).
- The strays at `/` came from `$TMPDIR/…`: agent runs have no `TMPDIR` (checked in a running
  session: `declare -p TMPDIR` → not found), so `$TMPDIR/ci6.log` is `/ci6.log`.
- A contained run (SPEC-002 ADR-22, `crates/yulius-ptyhost/src/containment.rs`) is a `bwrap`
  whose root is a fresh tmpfs (mountinfo: `/newroot / tmpfs`), with fresh tmpfs `/tmp`, `/run`
  and `/dev/shm`; the system layer (`/usr`, `/etc`, `/var`, `/home`, …, overlay), the sandbox
  directory (worktrees, home, repository) and a few files are bound in. Only the run's own
  processes see its tmpfs: the Yulius server cannot reach `/ci6.log`, and neither can a person.
  It disappears when the run ends.
- The Yulius MCP server runs in the Yulius server (`/mcp` over HTTP), so an MCP tool cannot see a
  run's private filesystem. The `yulius` executable is bound into every run and already serves
  agents in the shell: `yulius wait` (SPEC-011), listed in the briefing's "Yulius tools".
- The run's launch (`crates/yulius-harness/src/launch.rs`) knows the task's worktree
  (`LaunchRequest::worktree`) and sets the run's environment (`YULIUS_URL`, …).
- Other tasks of the sandbox share the sandbox directory and the system layer, so "created by this
  task" cannot be told from a path there, except in the task's own worktree, which no other task
  writes, where git tells the task's own (untracked, ignored) files from the repository's.

## Requirements

- RMV-001 (env): Every agent run of a task gets `YULIUS_TASK_WORKTREE` (its worktree) and
  `TMPDIR=/tmp` in its environment.
- RMV-002 (worktree): `yulius remove` removes a path inside `$YULIUS_TASK_WORKTREE` (resolved,
  symbolic links in its parent directories followed; taken only when it is a directory of a
  sandbox's `worktrees`) when it is not the worktree itself, not its
  `.git`, and git (asked without the caller's `GIT_DIR`, `GIT_INDEX_FILE` and the like) tracks
  nothing at or under it. A tracked path is refused ("tracked by git: use
  `git rm`").
- RMV-003 (run-private): In a contained run (the root `/` is `bwrap`'s tmpfs, mounted from its
  `/newroot`), `yulius remove` removes a
  path whose filesystem is the run's own tmpfs mounted at `/`, `/tmp`, `/run` or `/dev/shm`.
  The root's links that containment makes (`/bin`, `/sbin`, `/lib`, `/lib64`) are refused.
- RMV-004 (mounts): A path that is a mount point or holds one (`/`, `/tmp`, `/usr`, a directory
  holding a bind) is refused; removal never crosses into another filesystem. Without a readable
  mount table nothing is removed.
- RMV-005 (refusal): Any other path (the sandbox's shared home, system layer, repository, another
  task's worktree, or a run that is not contained) is refused with a reason that names where
  Yulius removes and tells the agent to leave the file and mention it in its summary, never to ask
  a person to remove it.
- RMV-006 (removal): The last component is never followed: a link is removed as a link, a
  directory with its contents, a file as a file. A path that does not exist is reported as
  already gone; a removal that fails is reported as such, without the refusal note. Each path gets
  one line (trailing slashes ignored); the exit code is 0 when every path is gone, 1 when any
  was refused or failed, 2 for a usage error (no path).
- RMV-007 (briefing): The briefing's "Yulius tools" names `yulius remove` for the task's files at
  `/`, in `/tmp` or through a variable, instead of `rm`, so Claude Code's prompt never comes first.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Agent runs know their worktree and have a temporary directory
- [x] `yulius remove` decides which paths belong to the task
- [x] `yulius remove` removes them and reports each path
- [x] The briefing and docs tell agents to remove their own files with it
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC-1: A run's launch environment holds `YULIUS_TASK_WORKTREE=<worktree>` and `TMPDIR=/tmp`.
- AC-2: In a git worktree, `yulius remove` removes an untracked file, an ignored directory (not
  what links inside it point to) and a link (not its target, also given with a trailing slash),
  refuses a path reached through a link or `..` out of the worktree, and refuses a tracked file, a directory holding one, `.git` and the
  worktree itself, leaving them in place.
- AC-3: With a contained mount table, a file at `/x`, `/tmp/a/b` or `/run/x` is the task's; `/`,
  `/tmp`, `/bin`, `/usr/x`, `/home/x`, a directory holding a mount point and a path in another
  task's worktree are refused.
- AC-4: Without a contained root (no tmpfs `/`, or a tmpfs `/` not mounted from `/newroot`),
  `/tmp/x` is refused; without a mount table, everything is.
- AC-5: A run of `yulius remove` with a refused and a removable path removes the removable one,
  prints a line for each and exits 1; with only removable or missing paths it exits 0; without
  paths it exits 2.
- AC-6: The briefing names `yulius remove`.

## Test plan

- Unit (`crates/yulius-harness/src/launch.rs`): the launch environment (AC-1).
- Unit (`crates/yulius/src/remove_cli.rs`): mountinfo parsing and the ownership decision over a
  synthetic mount table (AC-3, AC-4); removal in a temporary git worktree with a mount table that
  has no contained root (AC-2, AC-5).
- Unit (`crates/yulius-domain/src/briefing.rs`): the tools section (AC-6).
- Manual: in this task's own contained session, a file made at `/` is removed with the built
  `yulius remove` without a prompt; recorded in Evidence.

## Decisions

- ADR-0155: a task removes its own files with `yulius remove`, in its run, by location (its
  worktree's untracked files and its run's private tmpfs), not with an MCP tool or a record of
  created files.
- `TMPDIR=/tmp`: `/tmp` is the run's own tmpfs in a contained run and the host's temporary
  directory otherwise; programs that read `TMPDIR` already default to it.
- The Superagent is out of scope: it runs as its person, who is in front of it and can answer
  Claude Code's prompt; it has no worktree.
- Claude Code flagging `sh -c` scripts that hold no removal (#118, #134) is Claude Code's check;
  agents already work around it with a script file. Out of scope.
