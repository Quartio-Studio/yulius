---
yulius-task: 29139e35-6467-465f-b585-6f80a004286b
status: in-progress
---

# SPEC-678: Fix all open GitHub issues

## Goal

The repository owner asked, on 2026-11-27, for every GitHub issue open that day (41) to be closed:
by a merged pull request that fixes it, or by a comment that says why it is already fixed or
cannot be fixed from this repository. Issues go in priority order (high, medium, low), one pull
request per issue or per small group of closely related issues, each test-first, independently
reviewed and merged into `dev` by `delivery_merge`.

## Context

- The 41 issues were read in full on 2026-11-27 (`gh issue list --state open --limit 1000`).
- Out of this repository's reach, by the owner's request: #1385 (quartio-v2's part of SPEC-497)
  is left as it is; #321 needs a measurement on the production VM's host, which a sandbox cannot
  reach (no `/sys/fs/cgroup`), so it gets a comment with what remains.
- Owner's decisions, asked with `needs_you` and answered on 2026-11-27: #638 keeps
  `GH_TOKEN`/`GITHUB_TOKEN` allowed (SPEC-004 GH-003 stays), so it was closed with that
  decision; #1239 removes the old fingerprint format now (the two weeks after 2026-11-13 have
  passed, and a session from before it would only stop running commands until continued).
- #1647's code fix merged in #1657; what remains is switching the App's webhook on, a person's
  action on the instance.
- Each slice below cites its issue; its own requirements and evidence are added to this spec in
  the pull request that delivers it. Non-goals: no new issues are opened (things noticed go into
  the final report), nothing is promoted to `main`.

## Requirements

- **FIX-001** Each issue's acceptance criteria are met by the slice that names it, or the issue
  is closed with a comment that names the fixing commit, or the reason it is not done.
- **FIX-002** A flaky test is fixed at its cause, shown by a probe at the racing step that fails
  before the fix; no retries, sleeps, longer timeouts or weaker assertions as the fix. A
  quarantined test leaves quarantine only once fixed (closing its issue ends the quarantine).
- **FIX-003** No slice widens a security boundary beyond what its issue asks.
- **FIX-1547** `task_message::a_flag_of_a_held_message_does_not_stop_the_pull_request_watch`
  passes when the news's submit hook reaches Yulius after its turn's Stop (the race behind its
  "timed out waiting for the turn's end" failures).

## Plan

- [x] #1547: the held-message test's cause shown fixed on dev by #1732; its quarantine ended
- [ ] #1831 #1727: yulius wait follows the task's staging request
- [ ] #1824: a disabled person's automations create no task
- [ ] #1808: the parking candidate's later stall is reported deterministically
- [ ] #1809: the cut-off reviewer's Opus note test waits on the note's trigger
- [ ] #1807: the compile-cache Ctrl+Z test waits on the stopped job
- [ ] #1769 #1761: chore tasks and the documents agree on specs; skill wording on dev-stage secrets
- [ ] #1765: privileged containers on Ubuntu 26.04 start or are skipped with the reason
- [ ] #1133 #321 #1647: remaining queue analysis and host-only checks recorded on their issues
- [ ] #1826 #1760: sweep and identity shim lib tests are deterministic
- [ ] #1820: the Telegram reply test waits for the second run's notice
- [ ] #1817: a test holds prove-on-dev's refusal when dev cannot be deployed
- [ ] #1811: the security scan card reads vidoc.toml as the scanner does
- [ ] #1800: the briefing's mandatory parts fit the budget in every docs layout
- [ ] #1795: the watch file-size bound holds whether the writer gets EFBIG or SIGXFSZ
- [ ] #1775: the MCP probe test counts a zombie grandchild as gone
- [ ] #1767: the memory stopgap walk counts every entry and reads folders by descriptor
- [ ] #1758 #1714: dialogs give the keyboard back to the redrawn opener or the failed action
- [ ] #1753 #1751: non-owner creators in a personal account or a narrowed App are told who acts
- [ ] #1741 #1604: opt-in real-Chrome tests wait for input and accept the decided results
- [ ] #1702: Codex goal, intake and context-full input repaint an unreadable screen
- [ ] #1693: the Antigravity error-turn hook test is deterministic
- [ ] #1692: a replaced run's reattach leaves the task's writer alone
- [ ] #1691: a decided armed merge is consumed across a restart
- [ ] #1688: a cancelled display start keeps its Xvfb in the slot
- [ ] #1595: confined test fixtures share one temporary root
- [ ] #1591: parked-list and CI watcher fixtures hold under shared CPUs
- [ ] #1544: egress warning-capture tests observe their own warning
- [ ] #1532: a branch fast-forwarded to a head with skipped jobs gets its own run
- [ ] #1520: the Superagent deep-link panel test is deterministic
- [ ] #1239: the old fingerprint format is removed and a version 1 file is rewritten as version 2
- [ ] Final report of every issue's outcome given

## Acceptance criteria

- **AC-1** (FIX-001) Every issue open on 2026-11-27 is closed by a merged pull request or a
  justified comment, or is named in the final report under "Not done" with its reason.
- **AC-2** (FIX-002) Each flaky-test slice records the probe that reproduced the failure and the
  same probe passing after the fix.
- **AC-3** (FIX-1547) With a probe at the racing step the test fails without the fix and passes
  with it, and its other assertions are unchanged.

## Test plan

Each slice adds or changes the test its issue names, run in full with its test binary; flaky
fixes are reproduced with probe delays at the await point (not by load) and stressed with
`nextest --stress-count` beside busy loops. Yulius CI runs the whole gate on each head.

## Decisions

- One spec for the whole task, one pull request per issue or group, delivered in turn from this
  branch: the task has one branch, and `delivery_merge` delivers the finished part of the plan
  (ADR-0325). Each slice's details are added in its own pull request.
- Issues needing a person's action or decision are asked with `needs_you` and the work goes on
  with the others, as the owner's request says.

## Evidence

### #1547 (closed, fixed on dev by #1732)

- All three failures (PR #1536, #1543 on 2026-11-20; task #596 on 2026-11-22) were `Error:
  "timed out waiting for the turn's end"`. #1732 (16556d990, 2026-11-23) changed `hook_event` to
  read a hook's send time on Yulius's clock (`Clock::of_wall`).
- Probe at `race::Point::HookArrived` (`<session>/UserPromptSubmit`), holding the news's submit
  until the news turn's Stop is answered: on dev d6e17997d the hold fired and the test passed
  (6.5 s). With `hooks.rs:251` reverted to `wall.0.min(now)` it failed as in CI,
  `activity=Some(Working)`, "timed out waiting for the turn's end" (36.5 s).
- Unchanged test, 140 busy loops, `nextest --stress-count 40 --test-threads 40`: 40/40 passed.
- Closed on GitHub with that evidence; the quarantine ends with the issue.
