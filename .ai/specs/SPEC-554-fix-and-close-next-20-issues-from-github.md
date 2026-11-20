---
yulius-task: 05e6cf27-efb2-4b85-904c-e8d451c75529
status: in-progress
---

# SPEC-554 Fix twenty open GitHub issues

## Goal

Deliver the next twenty actionable repository fixes to dev and close each issue with evidence.

Fixes #1052
Fixes #1061
Fixes #1070
Fixes #1073
Fixes #1078
Fixes #1082
Fixes #1083
Fixes #1089
Fixes #1101
Fixes #1126
Fixes #1136
Fixes #1137
Fixes #1147
Fixes #1166
Fixes #1181
Fixes #1186
Fixes #1187
Fixes #1191
Fixes #1195
Fixes #1196

## Context

Task #535 follows #517, #522, #529 and #534. Their fixes are in the starting dev tree.
The batch uses creation order and the issues' published acceptance criteria. Older issues
#321, #466, #529, #638 and #1133 have outstanding external evidence or design prerequisites.
Task #494 owns #1106 and #1107.

## Requirements

- **ISS-1052** [Help texts disagree with behaviour: vidoc_scan --base default, attachments_trash "into" default](https://github.com/Quartio-Studio/yulius/issues/1052).
- **ISS-1061** [CD page: a newer kept release reads "Roll back to this" though it rolls forward](https://github.com/Quartio-Studio/yulius/issues/1061).
- **ISS-1070** [GitHub sign-in start/callback answer a raw 400 for a malformed query string](https://github.com/Quartio-Studio/yulius/issues/1070).
- **ISS-1073** [Mock: protect_patch_fail answers 503 before a playground's 422 for the production protection switch](https://github.com/Quartio-Studio/yulius/issues/1073).
- **ISS-1078** [Flaky: task_done::a_done_onboarding_task_is_archived_and_the_sandbox_is_ready times out waiting for the shell probe under full-suite load](https://github.com/Quartio-Studio/yulius/issues/1078).
- **ISS-1082** [Stop gate says a linked spec "does not exist"](https://github.com/Quartio-Studio/yulius/issues/1082).
- **ISS-1083** [Spec reading over 256 KiB: gates judge a partial read; edit-gate boundary nits](https://github.com/Quartio-Studio/yulius/issues/1083).
- **ISS-1089** [Playground guests are offered the Superagent, which the server refuses them](https://github.com/Quartio-Studio/yulius/issues/1089).
- **ISS-1101** [Global Superagent image list: point every copy at the others](https://github.com/Quartio-Studio/yulius/issues/1101).
- **ISS-1126** [Flaky: task_done::a_run_lost_while_the_server_runs_saves_its_live_screen times out waiting for Done in CI](https://github.com/Quartio-Studio/yulius/issues/1126).
- **ISS-1136** [PR notices: a red e2e job names no failing tests (the scanner reads only libtest/nextest headers)](https://github.com/Quartio-Studio/yulius/issues/1136).
- **ISS-1137** [PR notices: "passed and the merge is armed" quotes a queue note that still says the run is pending](https://github.com/Quartio-Studio/yulius/issues/1137).
- **ISS-1147** [Git task review page shows the task's state as it was when the page opened](https://github.com/Quartio-Studio/yulius/issues/1147).
- **ISS-1166** [Ubuntu 26.04: `instance import` refusal prints `chown user:` (uutils does not set the group), and `build-essential` swaps GNU coreutils for uutils](https://github.com/Quartio-Studio/yulius/issues/1166).
- **ISS-1181** [Instance move: architecture.md still describes one unit, and the unreadable-state message names no unit to thaw (leftovers of #1173's review)](https://github.com/Quartio-Studio/yulius/issues/1181).
- **ISS-1186** [Remote CLI and the move's checklist: three nits left by the review of #1184 (SPEC-466)](https://github.com/Quartio-Studio/yulius/issues/1186).
- **ISS-1187** [instance migrate: leftovers in the thaw paths and the "does it run" readings after #1173 and #450 (low priority)](https://github.com/Quartio-Studio/yulius/issues/1187).
- **ISS-1191** [instance migrate: the plan's disk-space check counts an unfinished copy on the new host that Prepare removes](https://github.com/Quartio-Studio/yulius/issues/1191).
- **ISS-1195** [Subscriptions page: a read the person asked for still waits when the field it was held by is then refused and set aside](https://github.com/Quartio-Studio/yulius/issues/1195).
- **ISS-1196** [instance migrate: the plan's "an update is running" check never sees a running update helper (it compares with `active`, a oneshot is `activating`)](https://github.com/Quartio-Studio/yulius/issues/1196).

## Plan

- [x] Identify twenty open repository fixes and their acceptance criteria
- [x] Correct task-tool guidance and GitHub sign-in errors
- [x] Report failed browser tests and current merge state
- [x] Show the available sandbox actions and release direction
- [ ] Read complete specs and explain linked or oversized files
- [ ] Make onboarding and lost-run completion tests reliable
- [x] Refresh Git task reviews and subscription edits
- [x] Correct host move and import instructions
- [x] Preserve accurate freeze, thaw and readiness results
- [x] Explain staged copy capacity and refuse a running update
- [ ] Review and verify the combined fixes
- [ ] Link all twenty fixes for closure when the reviewed delivery merges

## Acceptance criteria

- **AC-1** The Vidoc help lists both default refs in precedence order, and attachment restore help names the required destination for callers outside a task (#1052).
- **AC-2** Malformed start and callback requests return to sign-in. A callback clears the browser's one-use state cookie (#1070).
- **AC-3** An e2e failure survives the bounded job scanner and appears by file and test in the red PR notice. A green PR waits for a current queue reason before quoting it (#1136, #1137).
- **AC-4** A release newer than the deployed one offers roll-forward in its row and confirmation. A refused protection update changes neither switch and playground validation precedes the mock busy response (#1061, #1073).
- **AC-5** Playground guests can open neither Superagent from the rail nor either deep link. Each of the three image allowlists points maintainers to its two counterparts (#1089, #1101).
- **AC-6** Stop, edit and plan sync judge whole specs within the byte bound, accept the exact limit, handle oversized and disappeared files, and explain refused symbolic links (#1082, #1083).
- **AC-7** Onboarding and lost-run completion tests synchronize on the state they exercise. Reproduce each reported wait and exercise the corrected path under the same condition (#1078, #1126).
- **AC-8** A task review follows task updates. A held user-requested subscription read completes after a refused edit, and touch tests detect page errors (#1147, #1195).
- **AC-9** Import recovery gives a working ownership command on GNU and uutils. Move guidance names the units and the commands recovery requires (#1166, #1181).
- **AC-10** Remote login explains taking back a sign-in, transport errors give applicable recovery guidance, and move output sanitizes invisible characters (#1186).
- **AC-11** Move recovery uses what the freeze covered and distinguishes known and unknown unit states in its report. The disk-space check accounts for or explains a staged copy that Prepare will clear. A running oneshot update stops the plan immediately (#1187, #1191, #1196).
- **AC-12** Each closure cites the merged fix or directly verified behavior on dev.

## Test plan

Exercise GitHub sign-in through HTTP, CI notices through the watch, spec gates through hooks, and task completion through supervision.
Run the browser suite for release labels, production protection, playground navigation, review updates and subscription edits.
Use controlled command and filesystem fixtures for import and move failures. Run touched tests, quick checks and the configured security scan. Record each result and the delivery head's CI.

## Decisions

Use one task and segregate editing by code ownership. Preserve existing protocols and defaults unless the named issue changes them.

## Evidence

- #1052. Compared `vidoc_scan.py --help` against `base_ref` and the shared MCP description against the restore handler. The revised help lists both default refs. All 92 tests in `scripts/test_vidoc_scan.py` passed.
- #1061, #1073, #1089, #1147 and #1195. Five Chromium cases failed on the original UI. With the fixes, 80 cases passed across the CD, release, guest, protection, Git review, subscription and touch suites. Injecting a console error failed the touch test. These runs used the browser mock.
- #1073. The current server maps a store failure to HTTP 500 (SPEC-483 P15). The test checks 500 for a configured sandbox, 422 for a playground, and both switch values after refusal.
- #1101. The page and the mock now point to the other image type lists.
- #1070. The original start handler returned `github_unconfigured` for invalid UTF-8 on the unconfigured instance. Axum's form decoder replaces invalid bytes, and duplicate fields can reject before the handler. Both start and callback now validate the query before using it.
- #1136 and #1137. The original scanner returned no names for two failed browser tests. The watch reproduced a single notice saying both “passed” and “(pending)”. The issue fixtures match the pinned e2e runner's uncolored CI summary.

- #1166. On Ubuntu 26.04.1 with uutils, the old chown form kept group root. Using the account’s primary group produced the expected ownership. The documented package selection was checked with apt’s dry run.
- #1181, #1186, #1187, #1191 and #1196. The original scoped run had nine failures. With the fixes, 246 migration, freeze and import tests, 17 remote client/store tests and one real CLI sign-in integration test passed. Migration fixtures used fake systemd and SSH with temporary directories.
- The malformed-query HTTP regression and four neighboring GitHub sign-in tests passed. All 13 failure scanner and 15 PR watch unit cases passed.
- Full Chromium suite on the integrated web changes: 1,154 passed, one model-dependent test skipped, 326.77 seconds.
