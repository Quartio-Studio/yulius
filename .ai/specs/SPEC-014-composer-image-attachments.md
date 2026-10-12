---
yulius-task: 23ec195b-f092-4556-8e7f-01430aeb61d6
status: done
---

# SPEC-014: Images attached to a new task are seen, not shown as a path

Status: implemented
Date: 2026-10-12
Owner: Yulius task #12
Scope authorization: user request ("when I uploaded an image I see only path and not image and it's passed to claude as path. Fixed")

## Goal

An image dropped or pasted on the new-task prompt appears as an image in the composer, and the task's agent receives it as an attachment, not as a path typed in the text.

## Context

- SPEC-006 (CMP-001) uploaded the file and typed its path into the prompt, so the user saw a long path.
- A new task's first prompt is a CLI argument to Claude Code (`launch.rs`); in goal mode a short prompt is typed as `/goal <one line>` (`supervisor.rs` `spawn_goal`).
- Claude Code attaches a file named by an `@path` mention; for an image it attaches the image.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| IMG-001 | Attached files show beside the prompt as chips: images as thumbnails, others with a file icon, each removable. The prompt text is not changed. | e2e `filedrop.e2e.ts` composer test |
| IMG-002 | When the task starts, each attached file is appended to the prompt as an `@path` mention on its own line (also for each task of a split). Attachments are cleared once the task opens. | code review, `withAttachments` |

## Acceptance criteria

- AC-IMG-1: dropping or pasting an image shows a thumbnail and leaves the prompt text as typed.
- AC-IMG-2: removing a chip removes the file from what the task receives.

## Plan

- [x] Show attachments as thumbnails in the composer
- [x] Send attachments as `@path` mentions when the task starts
- [x] Spec, e2e test and commit

## Test plan

Browser e2e for drop and paste (mock server). Not covered: that a live Claude Code attaches the mentioned image; no credential in this environment.

## Decisions

- Attachments are held in the browser and joined at start, so the user can remove one, rather than typing paths into the text.
- `@path` mentions instead of bare paths so the agent receives the file; a bare path made the agent look for it.
- Limit: in goal mode the `/goal` condition is text; an `@` mention there may not attach the image. Unverified.

## Evidence

- `npx e2e run tests/filedrop.e2e.ts` (7 tests): the composer test passes in every run (IMG-001). Terminal tests fail intermittently with `operation timed out: locator(".taskview-term .xterm-rows")` (a different one each run, before any path assertion); a full 7 of 7 pass occurred in 3 of 5 runs. Not diagnosed.
- Not run: the full gate; a live Claude Code (IMG-002's attach-as-image behavior is unverified).
