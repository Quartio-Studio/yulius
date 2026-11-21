---
yulius-task: a7e07e4a-e6e5-4986-bcd4-becfa2aa104b
status: done
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
- [x] Review and deliver the verified fix to dev
- [x] Verify task attention during a blocked browser lookup and overlapping Git timeouts
- [x] Verify the other failures from the merged revision against their owning fixes
- [x] Deliver the reviewed fix forward and request staging for the new revision

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
- Review of `ee0e96fa4` found three remaining projection cases. The new tests failed before the corrections. A live row and overview returned Done and success for a locally errored run. The overview reopened a directly observed unmerged closure. An overlapping failed read erased a red result that was still in flight. The task projection now reads matching local CI, the PR display uses its observed open flag and the cache resolves missing sources in request order. The sandbox’s raw PR list still describes the current GitHub response used by delivery.
- The third review found two partial-read cases. Both regressions failed against the previous implementation. A new head with one successful check source and one unavailable source displayed `success` instead of `unknown`. After GitHub confirmed a conflict, the next unavailable overview lost it. The task display now keeps positive conflict evidence independently of checks, consults matching local CI on a new head and leaves the sandbox’s raw GitHub response intact.
- Independent review approved commit `09f98caa9` after all findings were addressed. The scoped nextest run passed 73 of 73 on that commit. A separate run of the GitHub client, CI status publication and production-branch tests passed 49 of 49. The delivery record for task #592 supplies the exact Yulius CI result and merged revision. Live production quota exhaustion was simulated with a refusing GitHub fixture.
- When integrating PR #1657, the Rust test build caught the new required `webhook_secret` field in the rate-limit fixture. The fixture uses `None` because it exercises outbound requests. Both projects’ product notes survived the documentation conflict.
- Staging gate `6f7921e2-c888-4098-a3e0-90b5008634e7` reported two deadline tests that still expected verification timeouts to create attention. Both failed locally with that expectation. The browser deadline now checks the recorded open PR and the absence of a new notice. The worker test seeds a real PR failure and checks its notice across two batches of twelve overlapping reads and while the workers drain. After verification resumes, the notice clears on green. All 12 scoped parked-detail and outage tests passed.
- Independent review approved `7e64e3f1e` and independently passed both revised cases. In a detached checkout with task #589’s owning fix at `7c49b5015`, every one of the 17 failures from both shards passed. The complete `agent_hooks` and `delivery` modules passed 258 of 258. The shared fixture attaches its recorded idle attempts, and delivered review rounds are visible through the task’s delivery view.
- A manual one-line check in that temporary checkout restored the old `PullSignal::Errored` timeout mapping. Both revised tests failed. The browser timeout created a false Needs-you notice, and the concurrent reads replaced the known failing-check episode with a CI-error episode. Restoring the real timeout handling passed both tests again.
- PR #1669 landed the shared fixes on dev at `2c386072c`. The clean integration at `d9eee5acf` passed all 17 previously failing tests again. Task #592’s delivery evidence records the fix-forward merge and its staging request.
