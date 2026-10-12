---
yulius-task: 6444bcc1-8292-4016-984d-11f75636dfd9
status: done
---

# SPEC-006: Files dropped on the new-task prompt

Status: implemented
Date: 2026-10-11
Owner: Yulius task #2
Scope authorization: user request ("Add draging images in task view. At the moment it works only on claude code.")

## Goal

Images and other files can be dragged onto, or pasted into, the new-task prompt in the task view,
not only onto the Claude terminal. The task's agent then reads them by path.

## Context

- UI-048 already uploads files dropped on the task terminal or a user's terminal and types their
  paths there. Files go to attachments folders inside the sandbox directory, outside every worktree.
- The new-task composer has no task yet, so no task attachments folder exists to receive files.
- Interpretation: "works only on Claude Code" read as "only the terminal accepts drops". Recorded as an assumption.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| CMP-001 | Files dropped on the new-task prompt box, or an image pasted into it, are uploaded and their paths (quoted when needed, space-separated) are written into the prompt at the cursor. Text pastes are unchanged. | e2e `filedrop.e2e.ts` composer test |
| CMP-002 | `POST /api/sandboxes/{sid}/attachments?name` stores a file in `sandboxes/<sid>/attachments/drafts/<user>/` with a safe, never-clashing name, outside every worktree; it needs write access to the sandbox (403 otherwise, 404 for an unknown sandbox) and is bounded by the upload limit. | `composer_drafts_are_kept_per_user_outside_git_and_need_write_access` in uploads.rs |
| CMP-003 | Files in a user's drafts folder older than 7 days are removed when that user next uploads a draft. | the same test (a backdated draft is removed by the next upload) |
| CMP-004 | While the prompt is read-only (a task is being created), drops are refused with a short message. | code review |

## Acceptance criteria

- AC-CMP-1: dropping a file on the prompt shows "Drop to add to the prompt", then the stored path appears in the prompt (CMP-001).
- AC-CMP-2: a read-only user's upload is refused; a write user's two uploads of one name get distinct files (CMP-002).

## Plan

- [x] Read the product context and related work
- [x] Draft attachment endpoint for the new-task composer
- [x] Drop and paste on the new-task prompt
- [x] Spec note, tests and commit

## Test plan

Rust integration test for the endpoint; browser e2e for drop and paste on the prompt (mock server).

## Decisions

- Drafts are kept per user and sandbox and expire after a week, instead of being tied to a task that does not exist yet. Limit: a task running longer than a week may lose a file it was given.
- The terminal drop flow was generalised (`attachPathDrop`) rather than duplicated.

## Evidence

- `cargo test -p yulius --test uploads composer`: 1 passed (CMP-002).
- `npx e2e run tests/filedrop.e2e.ts`: the composer drop-and-paste test passes (CMP-001). The existing "a text file dropped on the editor opens in a tab" test failed in 2 of 3 runs and passed once; the editor drop path is untouched, and it was not checked on `dev`.
- Not run: the full gate. `uploads` also has `the_agent_reads_a_dropped_file_by_path_and_git_stays_clean`, which fails here because fake-agent is not built.
- CMP-004 has no automated test. A failed upload no longer inserts an empty path (review finding, fixed).
