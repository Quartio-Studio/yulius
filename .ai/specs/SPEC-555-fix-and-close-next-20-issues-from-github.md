---
yulius-task: 33a0a9dc-3729-4100-a6e4-a0fb1cac5b05
status: in-progress
---

# SPEC-555 Resolve the next twenty GitHub issues

## Goal

Deliver twenty actionable fixes on dev and close their linked reports.

Fixes #1200
Fixes #1202
Fixes #1206
Fixes #1210
Fixes #1217
Fixes #1218
Fixes #1223
Fixes #1237
Fixes #1246
Fixes #1249
Fixes #1256
Fixes #1258
Fixes #1380
Fixes #1302
Fixes #1303
Fixes #1334
Fixes #1351
Fixes #1361
Fixes #1363
Fixes #1369

## Context

The starting head c20e9807d contains task #535. Use its oldest-actionable-first selection,
excluding its documented prerequisites, issues owned by task #494, and #1239 whose
upgrade prerequisite for older live wrappers has not been established. Open PR #1266 owns #1260.

## Requirements

- **ISS-1200** [A second ask right after the agent's work is lost: the task reads Working while a permission prompt waits](https://github.com/Quartio-Studio/yulius/issues/1200).
- **ISS-1202** [Settings → Instance, Move to another host: a field for the longest freeze (`--max-downtime`)](https://github.com/Quartio-Studio/yulius/issues/1202).
- **ISS-1206** [Stop hook: an open prompt pause is taken for a hold the CLI gave up, but Claude Code runs a queued prompt's hook at its Enter](https://github.com/Quartio-Studio/yulius/issues/1206).
- **ISS-1210** [e2e/README.md: the table of test files lists neither instance-limits.e2e.ts nor production-protection.e2e.ts](https://github.com/Quartio-Studio/yulius/issues/1210).
- **ISS-1217** [Flaky e2e under host load: menus "the automation editor's start menu shows its tasks five at a time too" opens the menu before the sandbox's tasks are read](https://github.com/Quartio-Studio/yulius/issues/1217).
- **ISS-1218** [Flaky e2e under host load: loading-shapes "the sidebar and Tasks lists load as task rows of the real rows' height" measures a first real row 20 px taller than its skeleton](https://github.com/Quartio-Studio/yulius/issues/1218).
- **ISS-1223** [Flaky e2e under host load: cd "a scrolled needs_you list survives task updates and its to-do stays opaque" reads the list's tabindex once](https://github.com/Quartio-Studio/yulius/issues/1223).
- **ISS-1237** [A Refresh that finds the model running does not end a waiting task's left-out memory](https://github.com/Quartio-Studio/yulius/issues/1237).
- **ISS-1246** [Flaky e2e: notes "a failed reload after a teammate's edit backs off" counts 4 loads in its 2.5 s window](https://github.com/Quartio-Studio/yulius/issues/1246).
- **ISS-1249** [Sandbox port ranges are never given again: they drift into the system's free ports and run out](https://github.com/Quartio-Studio/yulius/issues/1249).
- **ISS-1256** [A limit during a dynamic workflow: small leftovers after the third review round (SPEC-486)](https://github.com/Quartio-Studio/yulius/issues/1256).
- **ISS-1258** [A playground task's delivery view offers actions a playground does not have (open_pr, promote)](https://github.com/Quartio-Studio/yulius/issues/1258).
- **ISS-1380** [Usage probe reads a whole provider answer before checking its size bound](https://github.com/Quartio-Studio/yulius/issues/1380).
- **ISS-1302** [A Superagent's request to message or re-plan another person's task in another sandbox is asked although its approval can only be refused](https://github.com/Quartio-Studio/yulius/issues/1302).
- **ISS-1303** [The decisions index lists ADR-0332 before ADR-0331](https://github.com/Quartio-Studio/yulius/issues/1303).
- **ISS-1334** [Flaky e2e: subscriptions "a card stops saying Limit reached the moment its window resets" fails between 23:50 and midnight](https://github.com/Quartio-Studio/yulius/issues/1334).
- **ISS-1351** [Flaky under load: http_auth::event_streams_follow_revoked_access_and_close_for_disabled_users takes a heartbeat for a leaked event](https://github.com/Quartio-Studio/yulius/issues/1351).
- **ISS-1361** [Flaky risk: cockpit stray tests time out when the first probe's answer comes after SHELL_PROBE_TIMEOUT](https://github.com/Quartio-Studio/yulius/issues/1361).
- **ISS-1363** [The task search stays empty when typed in before the tasks load](https://github.com/Quartio-Studio/yulius/issues/1363).
- **ISS-1369** [CI admission: a failed read of one of a sandbox's ci/engine cpu.stat files makes the next reading a huge CPU rate](https://github.com/Quartio-Studio/yulius/issues/1369).

## Plan

- [x] Identify the next twenty actionable open issues
- [x] Correct playground delivery and cross-sandbox request actions
- [x] Repair repository test and decision indexes
- [x] Measure complete CPU samples and synchronize access and cockpit tests
- [x] Preserve attention and held prompts across overlapping hooks
- [x] Resume eligible subscriptions and classify workflow limits consistently
- [x] Bound provider usage reads before buffering the response
- [x] Carry the freeze budget through the instance move form and helper
- [x] Reuse a removed sandbox port range after its listeners are retired
- [x] Synchronize browser checks with the state each assertion measures
- [x] Refresh open task search when the sandbox tasks arrive
- [ ] Verify the integrated changes, obtain independent review and close on green delivery

## Acceptance criteria

- **AC-1200** A second answerable notification remains open when earlier work arrives. A worker's unanswered request survives main-session work. Later work still answers a single main-session notice.
- **AC-1202** The Settings form, stored plan, confirmation, API and helper agree on the optional positive whole-minute freeze limit. The helper rejects an invalid value or an abort bearing a limit. A move must match its plan and explain when the estimate prevents freezing.
- **AC-1206** A Stop from an earlier turn preserves the queued prompt's capacity hold. A Stop from the held prompt's own turn ends an abandoned hold. The architecture contract describes the order of these events.
- **AC-1210** Every browser test file has an accurate row in the test suite index.
- **AC-1217** The automation task menu test finds its task choices when the sandbox task read is delayed.
- **AC-1218** The first real task row and its skeleton measure the same height when Chromium skips layout for content outside the viewport.
- **AC-1223** A scrollable request list becomes a keyboard tab stop even with delayed observers. Short lists stay outside the tab order. Run the delayed case twenty times.
- **AC-1237** A successful model refresh allows a waiting task to use the recovered subscription before another subscription resets. Refused or inconclusive refreshes retain the excluded pair.
- **AC-1246** Identify the extra note load and verify increasing intervals between retries for the affected note. Run the regression twenty times at approximately one fifth of one CPU.
- **AC-1249** Allocation reuses the lowest available block after its previous sandbox retires. Existing holders and listeners retain their ranges. Upgrades preserve existing reservations and child rows. The operations guide specifies the range.
- **AC-1256** A workflow limit is classified consistently in either order of its agent files. Polls without a workflow limit avoid reading the run's model family. SPEC-486 records the prompt-hook behavior and a decision for each remaining review point.
- **AC-1258** The delivery API, agent tool, Git view and delivery dialog offer the local squash in a ready playground. Local prerequisites explain blocked squashes. A merged playground task has no further action.
- **AC-1302** Unowned steering requests are refused and audited before a card appears. Eligible answers to open questions keep their existing permissions. Approval rechecks the current owner, including after restoration.
- **AC-1303** The decision index is in numeric order and the repository check reports a reversed pair.
- **AC-1334** A subscription window starting at 23:55 displays its next-day reset and clears its limit on time.
- **AC-1351** Event stream assertions ignore heartbeats under the original deadline and still detect a real sandbox event.
- **AC-1361** Clock advances and reap assertions follow a confirmed stray sighting. A deliberately dropped probe identifies the missing sighting. Run the cockpit suite twenty times under load.
- **AC-1363** An open task search updates when tasks load and keeps keyboard focus and the selected task across subsequent updates.
- **AC-1369** An unreadable CPU statistic in an existing group invalidates the sample. An absent group contributes nothing. Admission uses the prior complete sample when the next reading succeeds.
- **AC-1380** A chunked usage response exceeding 256 KiB yields a temporary failure before the stream ends. Normal provider responses still parse.

## Test plan

Reproduce reported failures on the starting behavior and run the adjacent Rust and browser
tests on the changed behavior. Verify documentation against actual files. Run quick checks,
security scan, mutation checks, the browser suite and independent review before delivery.
Verify the exact CI head and each GitHub issue's closure.

## Decisions

Keep all twenty issues in this task. Preserve existing authorization, compatibility and
process ownership rules while applying the individual fixes.

## Evidence

- #1210 and #1303. Verified all browser files have table rows. A reversed decision pair failed the new check before the fix. All 14 repository contract unit cases passed afterward.
- #1217, #1218, #1223, #1246 and #1363. Chromium reproduced the late task read, skipped row layout, delayed tab stop and stale search. The skipped real row used 72 px where its contents needed 52 px. A delayed Team note read accounted for the fourth load. All 166 adjacent browser checks passed. The notes retry and late observer tests each passed twenty consecutive runs. The constrained notes series measured 0.192 CPU share. Search retained focus and selection during a live reorder.
- #1334. PR #1481 already covered a clock starting at 23:55. The regression passed at the batch's starting head and the issue closed on GitHub.
- #1258, #1302, #1351 and #1369. Before the fixes, the delivery API offered open_pr and promote, a cross-project request reached the panel, a heartbeat failed the revoked-access assertion and the sandbox sampler returned 1,500 microseconds for an incomplete reading. Afterward the scoped nextest run passed 127 cases. The Chromium playground delivery dialog failed before the fix and passed afterward.
- #1351. A temporary fixture retaining read permission failed on the real task.removed event for t-hidden. The fixture change was removed.
- #1202 and #1249. Baseline failures included the missing Settings field and an unavailable historical port. After the changes, all 223 focused Rust cases and all 12 instance-move Chromium cases passed. The host transition was exercised using the migration fixtures.

- #1200 and #1206. Older work could clear a second notice and an earlier Stop could release a queued prompt. The corrected cases passed, including worker questions after restart, interrupted predecessor turns, upgraded open holds and continued goals.
- #1237, #1256 and #1380. The refresh test distinguishes a running model from repeated refusals and inconclusive reads. Workflow classification passed with both file orders. A held-open oversized response failed promptly and a valid response exactly at 256 KiB parsed.
- #1249. The extended five-crate run passed all 272 selected tests. The upgraded database preserved child rows and leases. IPv4 and IPv6 listeners held ranges, as did a browser connection after its proxy listener ended. Schema versions 73 and 74 apply the port and held-prompt migrations in that order.
- #1361. With a 3,500 ms delay, all three original Cockpit cases identified the dropped first probe within 4.19–6.71 seconds. The injected delay was removed.
- The full Chromium suite passed 1,157 tests with one opt-in agent test skipped. The stricter short-list assertion also passed twenty times after the full run began. The scoped access and delivery rerun passed 125 tests. Vidoc was disabled by the instance or project setting. Host-move verification used fixtures.
- The Cockpit suite passed all 76 cases in each of twenty consecutive runs with CPU contention (1,520 executions). All 153 adjacent runtime checks passed using synthetic provider responses.

- First independent review requested five changes to e32eba4b1. The notes test now observes the timers that actually issue each sandbox GET. A constant 2,000 ms retry passed the former test and failed the corrected one. Purging and reusing a range formerly re-created both holder directories. The new lifecycle test and 49 neighboring cases passed with the directories absent.
- A worker question replacing a main question during reattachment could be cleared by main-session work. The new overlap test failed before recovery took the same lock as replacement. A predecessor's continuing Stop could keep an abandoned submit heartbeat alive. The fake-clock case failed before Stop stopped refreshing that heartbeat. Both regressions exercise the runtime entry points.
- The playground delivery view offered Merge even when its branch and dev pointed at the same commit. A GET regression reproduced the offered action. Both the view and the squash now use the same ahead check against the refreshed local integration branch.

- After round one, all 455 integrated Rust cases passed, including both schema migrations, and scoped Clippy passed with warnings denied. An earlier run timed out while the fake-agent fixture tried to build behind a concurrent Cargo lock. Prebuilding the fixture cleared the build dependency. The reviewed notes suite passed all 36 cases and twenty consecutive retries at 0.200 CPU share. The whole browser suite again passed 1,157 tests with the live-model case skipped.
- Round two on a527634a8 found three further #1258 cases. GET offered a squash for an edit followed by its revert, for a known conflict and for an exact-head failed or queued CI run. The net-change test and crash-recovery test passed after sharing the squash preflight. The Git conflict preview preserves refs and returns the actual conflicting paths. Reading the CI verdict uses existing runs and permits the first merge action when there is no run for this head and integration base.

- Round three reproduced two #1258 conflict-reporting edge cases. Two Unix names ending in different invalid UTF-8 bytes collapsed into one name. Git now keeps valid filenames intact and quotes the original bytes for invalid names. Both escaped names were observed and all ten selected Git tests passed. The remaining review finding concerned merge attributes when the task and primary clone check out different rules.
- Integrated verification also reproduced a pre-existing host-move test race. The helper announced Running before its fixture installed a signal handler. A 350 ms startup delay reproduced the incorrect abort order. Tests now await the shell's readiness marker. All eighteen helper tests passed twenty consecutive rounds under CPU contention.
- The third-round attributes finding reproduced in both directions on real Git repositories, including a task whose formerly required CI jobs disappeared on dev. Unchecked delivery now previews in the primary clone, and checked delivery uses the head's attributes and the merged candidate's configuration. The POST uses the actual train or squash result.
- Coverage of a retired namespace now exercises the production isolation adapter with a torn and then removed holder record. Pinned cargo-mutants caught both constant-return replacements. The public usage probe now receives body-only refusals, successful responses and oversized chunked refusals over local HTTP. All nineteen usage-adapter cases passed.
