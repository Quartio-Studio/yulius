---
yulius-task: a7e07e4a-e6e5-4986-bcd4-becfa2aa104b
status: in-progress
---

# SPEC-608. Preserve task state during a GitHub outage

## Goal

Keep finished tasks in Done when GitHub cannot answer. Tell the owner when the instance can use GitHub again.

## Context

Task #592 reports two GitHub App quota resets on 21 November. During the second incident fifteen parked tasks acquired a Resume suggestion and a finished live task appeared to be working. ADR-0424 already preserves a recorded merge during follow-up discovery. The remaining display paths can substitute a failed read for CI evidence.

## Requirements

- GHR-001. Preserve the last known PR state across refused reads, service errors, network timeouts and the display deadline. Genuine CI outcomes and owner requests still determine task attention.
- GHR-002. Surface one active GitHub App reset time across sandboxes. Name the merge, CI status and staging operations waiting on GitHub. Expire the notice at reset.
- GHR-003. Preserve existing fresh-evidence requirements for parking, dependency release and delivery. Task #593 owns request-budget optimization.

## Plan

- [x] Keep the last observed pull request state on unreadable task rows
- [x] Expose the GitHub App reset time in instance status
- [x] Show the shared wait in Status and task lists
- [ ] Review and deliver the verified fix to dev

## Acceptance criteria

An always-rate-limited GitHub leaves a merged parked task and a finished live task in Done, without an attention notice. Status and task lists show the reset and the affected operations. A genuine CI error remains visible. Delivery merges on green CI and requests staging once.

## Test plan

Exercise the task projection with unavailable GitHub and stored observations. Verify reset-header handling and expiry with a controlled clock. Exercise both browser surfaces and inspect the independent review and Yulius delivery evidence.

## Decisions

Use the existing delivery and pull observation paths for display and keep the quota wait at instance scope.

## Evidence

- With the original display code, the parked-task regression changed `parked_needs_you` from false to true after a verified green PR became unreadable. The always-rate-limited HTTP fixture failed to find the reset timestamp in instance status. The new behavior also checks the finished live task, the absence of a task notice and the pending CI status publication.
- A direct-read regression exposed a second failure mode. When the delivery record still said open, a failed read changed a previously observed merged PR back to open. The test checks the PR identity and state as well as the task group.
- The browser test failed when the shared notice was absent. It also failed when opening Status left two notices visible. Chromium verifies the same UTC reset in each view and only one visible notice.
- The scoped nextest run passed 65 tests across `yulius`, `yulius-app` and `yulius-github`. Coverage includes genuine CI errors, a changed head during HTTP 503s, the parked read deadline, fresh dependency checks, App versus personal credentials, and reset expiry under a controlled clock.
- Manual check of the browser assertion temporarily suppressed the reset message. The task-list text assertion failed. Restoring the message passed the same test.
- Independent review of `37476da02` identified five cases. Four new Rust assertions failed on that head. A second outage restored an older green head, local CI error had no notice, a partial green check response hid the same head’s failure, and an evicted installation token’s 429 left the instance reset empty. The open-panel expiry assertion also failed. The fixes retain an observed head independently of checks, consult matching local CI within the display deadline, preserve prior evidence for incomplete sources, mark token origin at issuance, and expire an open Status panel on its own timer.
- The Chromium task-done, parked and GitHub wait suites passed together on the integrated worktree (143 tests). The additional open-panel expiry regression passed with the local timer in place. Vidoc was disabled on this project.
