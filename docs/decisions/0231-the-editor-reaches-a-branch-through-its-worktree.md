# 0231. The editor reaches a branch through its worktree

## Status

Accepted. Specified in [SPEC-340](../../.ai/specs/SPEC-340-in-editor-we-want-be-able-to-change-bran.md);
task #324. Extends SPEC-001 EDT-001..004 and SPEC-070.

## Context

People asked to change branches or worktrees from the Editor. Its folder picker offered only the
sandbox repository and the task selected in the sidebar, so with no task selected it offered
nothing else. In Yulius every branch already lives in its own worktree: `repo/` holds the
integration branch and `worktrees/<task id>` each task branch, and agents, shells and other
people's editors work in them at the same time. The files API already serves any task worktree of
the sandbox (`root=task:<id>`), readable by every writer of the sandbox and changeable by the
task's owner only (SPEC-070).

## Decision

- `GET /api/sandboxes/{sid}/files/roots` (sandbox `Work` access) lists the folders the editor can
  open: the sandbox repository with the branch its `HEAD` names, then every task, active or
  archived, whose worktree is a directory on disk inside the sandbox directory, newest first, with
  its branch, number, title and whether the caller may change files there (its owner).
- The branch of `repo/` is read from `.git/HEAD` on the host: `.git` must resolve inside the
  sandbox directory and `HEAD` is opened with `O_NOFOLLOW | O_NONBLOCK` and read up to 1 KiB, so a
  link or pipe planted by a contained shell names no branch and blocks nothing.
- The Editor's folder picker shows those folders named by branch ("Sandbox · dev",
  "#12 feat/12-…", "(read-only)" after another person's), reading the list each time the Editor
  comes to the front, when the picker gets focus and when the selected task changes; until it is
  read, or when it cannot be, it offers what it offered before.
- In another person's worktree Monaco is read-only and the explorer offers no New file, Upload or
  Delete; the status bar says why.
- The Editor never checks out a branch: a branch without a worktree is not offered.

## Alternatives

- **`git checkout` in the open folder.** Switching `repo/` or a task's worktree to another branch
  pulls files from under the agents, shells and editors working there, and breaks the rule that
  a worktree keeps its branch (AGENTS.md); rejected.
- **A worktree per branch on demand** (`git worktree add` from the Editor). It creates disk state
  nobody cleans up and duplicates a task's own worktree; branches people work on already have one.
- **Derive the list in the browser from the task list.** The browser does not know which
  worktrees are still on disk (finished tasks' are removed) nor the repository's branch.
- **Hide other people's worktrees.** Reading them is allowed and is how a teammate's work is
  looked at; read-only is the honest rendering of the server's rule.

## Consequences

- One more read per Editor show; the list is small (one entry per task with a worktree).
- A task whose worktree is removed while it is open stays in the picker as the folder open now;
  its reads fail as before until the person picks another.
- The Shell panel's "Folder for new terminals" picker still offers only the selected task; it can
  reuse the same list later.
