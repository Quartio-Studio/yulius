---
yulius-task: 6444bcc1-8292-4016-984d-11f75636dfd9
status: done
---

# SPEC-005: Files dropped on the new-task prompt

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
| CMP-003 | Files in a user's drafts folder older than 7 days are removed when that user next uploads a draft. | code review (`prune_old_drafts`) |
| CMP-004 | While the prompt is read-only (a task is being created), drops are refused with a short message. | code review |

## Delivery and acceptance

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
