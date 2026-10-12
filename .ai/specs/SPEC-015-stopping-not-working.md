---
yulius-task: bd35c31c-5eeb-470b-b720-6ef91d8c2bc3
status: done
---

# SPEC-015: a task being stopped is not listed under Working

Status: implemented
Date: 2026-10-12
Owner: Yulius task #13
Scope authorization: user request with a screenshot: "stopped tasks are getting to working for a
moment for some reason, fix".

## Goal

When the user stops a task, the sidebar never lists it under Working. From the moment the stop is
asked for it is listed under Stopped.

## Context

`statusInfo` in `crates/yulius-server/web/assets/status.js` maps a status to a sidebar group. The
`stopping` status (shown with the stopped square) was in the `working` group, so a stopped task
flashed under Working until its process ended and it became `stopped_cancelled`.

## Requirements

- STP-010-1: a task with status `stopping` is in the `ended` group (label "Stopping").
- STP-010-2: stopping stays a live status: Stop, Delete and Archive stay unavailable meanwhile.

## Plan

- [x] Write the failing e2e test: stopping a task lists it under Stopped
- [x] Move `stopping` to the ended group

## Acceptance criteria

- After confirming Stop, the sidebar's Stopped group holds the task as "Stopping" and Working does not.

## Test plan

`e2e/tests/task-done.e2e.ts`: "a task being stopped moves straight to Stopped, never into
Working"; it fails with the old grouping.

## Decisions

- Only the grouping changes; `stopping` remains in `LIVE_STATUSES` so actions stay disabled.

## Evidence

- `cd e2e && npx e2e run task-done`: 4 of 4 pass with the fix; with `stopping` back in the
  `working` group the new test fails (3 pass, 1 fail).
- `scripts/dev ci` was not run for this change.
