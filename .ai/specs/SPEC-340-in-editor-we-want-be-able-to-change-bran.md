---
yulius-task: 965a22fe-d39c-4c33-9837-bead6d8dbdee
status: in-progress
---

# SPEC-340: The editor's folder picker lists every worktree, named by its branch

Status: in-progress
Date: 2026-10-27
Owner: Yulius task #324
Scope authorization: task #324 (`/goal`, with a screenshot of the Editor panel whose folder picker
reads "Sandbox"). The request: "in editor we want be able to change branches/worktrees". Extends
SPEC-001 EDT-001..004 (the editor's files API), SPEC-070 (a task's worktree is its owner's to
change) and SPEC-166 (switching folders).

## Goal

From the Editor, a person opens any branch this sandbox has checked out: the sandbox repository
or any task's worktree, each named by its branch, not only the task selected in the sidebar. A
worktree they may not change opens read-only and says why, instead of failing at the first save.

## Context

- The editor's folder `<select>` (`editorview.js`, `folderOptions`) offers "Sandbox" and the
  sidebar's selected task's worktree (plus the folder open now, if a terminal link opened another
  task's). With no task selected, as in the screenshot, it offers only "Sandbox": there is no way
  to reach another branch from the Editor.
- The files API already serves any task worktree of the sandbox (`root=task:<id>`,
  `App::file_target`, `sandbox_worktree`): reading needs `Work` access on the sandbox, writing also
  needs the task's ownership (SPEC-070); a non-owner's save answers 403.
- In Yulius every branch is in its own worktree (AGENTS.md: "never switch an existing worktree to
  another branch"): `repo/` holds the sandbox's integration branch, `worktrees/<task id>` each task
  branch. Nothing lists them for the browser: `TaskSummary` has `branch` but not whether its
  worktree exists on disk (finished tasks' worktrees are removed), and the sandbox repository's
  branch is not exposed.
- The Shell panel has its own "Folder for new terminals" picker (`panels.js`); it is not part of
  this request.

## Requirements

- **EWT-001 Worktree list.** `GET /api/sandboxes/{sid}/files/roots` (sandbox `Work` access, like
  the other file requests) answers `{"roots": [...]}`, each entry
  `{root, branch, task_number, task_title, writable}`:
  - first the sandbox repository (`root: "sandbox"`, its branch read from `HEAD`, `null` when
    detached or unreadable, `writable: true`), when it is cloned;
  - then every task of the sandbox, active or archived, whose worktree exists on disk inside the
    sandbox directory, newest task first (`root: "task:<id>"`, the task's branch, number and
    title), `writable` true only for the task's owner (SPEC-070).
  A task without a worktree, or whose worktree is gone or leads outside the sandbox, is not listed.
- **EWT-002 Picker.** The Editor's folder picker lists those entries, each named by its branch:
  "Sandbox · `<branch>`" and "#`<number>` `<branch>`" (the title when the task has no branch), with
  "(read-only)" after one the person may not change; its tooltip names the folder open now in full.
  It is read each time the Editor comes to the front, when the picker gets focus and when the
  selected task changes; while unread or after a failed read it offers what it offers today, and the folder open
  now always stays in the list. Choosing an entry switches folders as today (unsaved changes are
  asked about, SPEC-166).
- **EWT-003 Read-only folders.** In a folder whose entry is not `writable`, Monaco is read-only,
  the explorer offers no New file, New folder, Upload or Delete (a drop adds nothing and says
  why), and the status bar says
  "Read-only: Task #N is another person's" in place of the save state. A writable folder behaves
  as today.
- **EWT-004 Status bar.** With no file open the status bar names the folder with its branch
  ("Sandbox repository · dev", "Task #12 worktree · feat/12-…").
- **EWT-005 No checkout.** The Editor never changes which branch a worktree has checked out; a
  branch without a worktree is not offered (ADR-0231).

## Plan

- [ ] Read the product context and related work, then write and validate the spec
- [ ] The files API lists the sandbox's worktrees with their branches and who may change them
- [ ] The Editor's folder picker lists every worktree by branch and switches between them
- [ ] A worktree the person may not change opens read-only and says why
- [ ] ADR, architecture, product and API docs describe the worktree picker
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- `GET /files/roots` for a write user lists the sandbox with its `HEAD` branch, then each task
  worktree on disk newest first with branch, number, title and `writable` (true for the owner,
  false for a teammate); a task without a worktree on disk is absent; a read-only user gets 403.
- In the mock, the Editor with no task selected offers the sandbox and every task worktree by
  branch; choosing another task's worktree lists its files; one owned by someone else opens
  read-only, says so, and offers no New file.
- The picker keeps today's options when the list cannot be read.

## Test plan

- Rust integration test in `crates/yulius/tests/files.rs`: the roots listing (order, branches,
  `writable` per person, absent worktrees, a worktree outside the sandbox, 403 for read access).
- e2e `e2e/tests/editor.e2e.ts` on the mock: the picker without a selected task, switching to
  another task's worktree by branch, the read-only worktree (no WCAG violations), and a failed
  roots read keeping today's options.

## Decisions

- **The server lists the worktrees.** Only the server knows which worktrees exist on disk and the
  repository's `HEAD`; deriving them in the browser from `TaskSummary` would offer removed
  worktrees of finished tasks. The list reuses `sandbox_worktree`'s containment check.
- **Branches are reached through their worktrees, not by checkout** (ADR-0231): switching the
  shared `repo/` or a task's worktree to another branch would pull files from under agents, shells
  and other people's editors working there.
- **Read-only, not hidden, for others' worktrees.** Reading them is already allowed and useful
  (looking at a teammate's work); hiding them would hide branches the request wants reachable.

## Evidence

(Filled when done.)
