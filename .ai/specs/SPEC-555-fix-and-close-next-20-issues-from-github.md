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
- [ ] Correct playground delivery and cross-sandbox request actions
- [x] Repair repository test and decision indexes
- [ ] Measure complete CPU samples and synchronize access and cockpit tests
- [ ] Preserve attention and held prompts across overlapping hooks
- [ ] Resume eligible subscriptions and classify workflow limits consistently
- [ ] Bound provider usage reads before buffering the response
- [ ] Carry the freeze budget through the instance move form and helper
- [ ] Reuse a removed sandbox port range after its listeners are retired
- [ ] Synchronize browser checks with the state each assertion measures
- [ ] Refresh open task search when the sandbox tasks arrive
- [ ] Verify the integrated changes, obtain independent review and close on green delivery

## Acceptance criteria

- **AC-1200** A test that flags a task with an answerable notice, lets the agent write one work line and ask again at once, and finds the task in Needs you two transcript rounds later (it fails on `dev`: Working). A test that an ask work does not answer, arriving on an answerable notice's flag, is not lifted by the main session's next work. A single answerable notice is still answered by the agent's next work (`task_done::a_notice_is_answered_by_the_agents_next_work`). `scripts/dev ci` passes.
- **AC-1202** `MoveRequest` and the API (`AskMigration`, `MigrationRun`) carry an optional `max_downtime`; the rules refuse 0 and anything that is not a number, in the server and in the helper; an abort names none. The helper runs the command with `--max-downtime=N` for a plan and a move (argument test against the command's own parser), and a plan for other values still does not allow the move. The card: the field with its hint, the bound in the confirmation, and the outcome of a move that stopped above the bound; mock and e2e. Operations guide §7 ("Move from Settings → Instance") and the API reference.
- **AC-1206** A test sends the hooks in the real order (a prompt's hook while the turn before it runs, held for capacity; then that turn's Stop) and shows the pause stays open and the task reads as waiting for capacity; it fails on today's code. The case the code was written for (the CLI gave a held prompt up and its turn ran) still ends the pause, with its test. The comment in `hook_stop` and SPEC-002's paragraph agree with what Claude Code does.
- **AC-1210** `e2e/README.md` has a row for `instance-limits.e2e.ts` and one for `production-protection.e2e.ts`, written from what the tests assert. Every `*.e2e.ts` under `e2e/tests` is named in the table (a one-line check in the pull request's description is enough).
- **AC-1217** The failure is reproduced on demand (for example with a delay on the mock's task read) and the test, or the menu, is fixed so that it passes with that delay. If the menu itself is changed (rebuilt when the tasks arrive), a test covers a menu opened before the tasks were read.
- **AC-1218** The taller first row is reproduced on demand (for example with a delay at the step that changes its height) and explained. The test waits for that condition, or the layout is fixed, and the test passes with the delay in place.
- **AC-1223** The test passes 20 runs in a row with `lateResizeObservers(browser)` before `app.open`. A list that scrolls is a tab stop by the time the test reads it, however late the observers run; a short list is still no tab stop. The whole e2e suite passes.
- **AC-1237** A task that waits with subscription A left out for a model runs on A again after a Refresh of A in which Claude answers with that model, before any other subscription's reset. A Refresh that Claude refuses, or does not answer, leaves the task waiting as before (SPEC-464 UCL-010 still holds: no restart at every reading). A regression test beside `usage_credits::a_task_refused_again_waits_for_a_known_reset_off_that_subscription` fails without the fix.
- **AC-1246** The cause of the fourth load in the window is found (reproduced with a probe delay, not by repeating the test). The test asserts the back-off on a condition (gaps between retries or a controlled clock), not on a count after a fixed wait. The test passes 20 times in a row under a CPU share of 0.2.
- **AC-1249** A new sandbox gets the lowest range no live sandbox holds (a test creates, removes and creates again and gets the same range) A range is not given again while its former sandbox's relay or holder can still be listening on it `docs/operations.md` says which ports Yulius gives to sandboxes
- **AC-1256** A unit test reads an answered default-type agent and a first-request refusal of another agent of the same run in one poll, in both file orders, and gets the same scope. `run_family_of` is not called for a poll without a `WorkflowLimitReached`. SPEC-486 WFL-006 says a workflow paused at another model's limit gets no note. Items 4 to 7 are each fixed with a test or kept with a sentence in SPEC-486. `scripts/dev ci` passes.
- **AC-1258** A playground task that is not merged lists `merge` and not `open_pr` in `next_actions` (API and `delivery_status`). A merged playground task lists no `promote`. A test in `crates/yulius/tests/playground.rs` fails without the change. The Git page and the Deliver dialog show no action for a playground that the server would refuse.
- **AC-1380** `get_json` stops reading at `MAX_BODY` + 1 bytes and answers the temporary failure A unit test with a local stand-in that streams more than `MAX_BODY` bytes (without `Content-Length`) proves the read stops early and the failure is temporary Normal answers (Claude usage, Codex `wham/usage`) still parse
- **AC-1302** `sandbox_task_message` for a task the person does not own is refused before a card is shown, with the owner named, and audited as the other early refusals are `sandbox_task_update` for a task the person does not own is refused the same way The check at approval stays (a task handed over or taken back while the request waits is decided by who owns it then), with a test for a change of owner between asking and approving A waiting request restored after a restart for a task that is no longer the person's still ends in the refusal on approval The doc comment of `a_message_to_a_task_holding_its_owners_personal_secrets_is_refused` gives today's reason `docs/api.md` says when these two tools refuse without asking
- **AC-1303** 0331 is listed before 0332 in `docs/decisions/README.md` `scripts/check_repository.py` fails on an index whose entries are out of number order (with a test), or the issue records why the order is not checked
- **AC-1334** The test sets the page's clock to a fixed time of day (or asserts both forms), so it passes when run at 23:55. A run of the test with the page's clock at 23:55 passes.
- **AC-1351** The negative look in `event_streams_follow_revoked_access_and_close_for_disabled_users` ignores `heartbeat` messages, and any other negative look in `http_auth.rs` that reads the raw stream does the same. A probe that delays the test past one heartbeat interval, or a shortened heartbeat in the fixture, reproduces the failure before the fix and passes after it. The test still fails when the revoked sandbox's event is delivered (a mutation that keeps the access).
- **AC-1361** With the throwaway delay above, the three stray tests pass, or fail only with a message naming the dropped probe rather than timing out. No sleeps, retries or longer waits are added to the tests. `cockpit::` passes 20 consecutive runs under load.
- **AC-1363** With the tasks read held, an open search with a query shows the matching tasks once the read is released, with no further input. An e2e test covers it with the mock's hold, and it fails before the fix. The search keeps its selection and focus when it re-renders.
- **AC-1369** `ci_use` returns `None` when either the `ci` or the `engine` group's `cpu.stat` exists but cannot be read, and still returns a sum when a group does not exist at all. A unit test in `cgroup.rs` covers an unreadable `engine/cpu.stat` beside a readable `ci/cpu.stat`. A unit test in `yulius-app` `resources.rs` shows that a skipped reading followed by a full one does not produce a rate above what was used.

## Test plan

Reproduce reported failures on the starting behavior and run the adjacent Rust and browser
tests on the changed behavior. Verify documentation against actual files. Run quick checks,
security scan, mutation checks, the browser suite and independent review before delivery.
Verify the exact CI head and each GitHub issue's closure.

## Decisions

Keep all twenty issues in this task. Preserve existing authorization, compatibility and
process ownership rules while applying the individual fixes.

## Evidence

Pending behavioral verification.
