---
yulius-task: 1ddb4937-1438-4719-96e8-ec4439e7a956
status: in-progress
---

# SPEC-116: onboarding leaves the project ready to work in the discipline

Status: in-progress
Date: 2026-10-14
Owner: task #101
Scope authorization: the person's request (task #101): "Linter/dead code should be installed on
onboarding. Also other things from screenshots. Onboarding should make the project ready to work
in the discipline", with three screenshots of the studio sandbox's onboarding (task #1, PR #59).

## Goal

After onboarding, the first ordinary task meets a project that already has everything the
discipline asks for:
- a test command, a lint and format check, and a dead-code and unused-dependency check in its
  gate;
- Yulius CI jobs covering every project of the repository;
- documents that say how it works.

Onboarding delivers through the same path as every other task, so its agent never has to work
around Yulius to get its pull request. The briefing shows the newest changes, whichever end of
the changes log they are at.

## Context

The screenshots show onboarding task #1 of `Quartio-Studio/studio` ending with:

1. **"Studio has no linter or dead-code check yet (#57)" and "Client and studio projects have no
   Yulius CI jobs yet (#58)" as follow-ups.** The onboarding prompt
   (`crates/yulius-app/src/onboarding.rs`, step 7) says: "When tests, lint, a dead-code and
   unused-dependency check … or environments are missing, lay them as the infra-bootstrap skill
   describes, as follow-up pull requests". Step 10 adds "Change documentation, configuration and
   CI only". The agent did exactly that: it filed issues, and the project was left without the
   gate the discipline relies on. The `infra-bootstrap` skill also asks for "a short series of
   pull requests".
2. **"I pushed the branch and opened PR #59 myself, although the task said not to. Yulius only
   opens the PR when a turn ends, and the goal's stop hook kept blocking that."** This is a real
   deadlock. `clean_stop` (`finish.rs`) waits for the goal verdict while the goal is open. On
   "not met" the turn goes on, and `settle_clean_turn`, the only caller of `auto_open_pr`, never
   runs. The goal of every non-playground task is "merged into dev on green Yulius CI", which
   needs the pull request. So a goal task gets its pull request only when the evaluator lets a
   turn end. That happens when the agent ends its turn with `yulius wait` in the background (the
   briefing's advice). The onboarding prompt never says so, and the agent in the screenshot kept
   working until it pushed by hand. `delivery_merge` arms a merge even when the task has no pull
   request, and the armed merge never opens one.
3. **The Windows-only installer workflow was kept "as provisional until a founder accepts it",
   with a todo for a person.** Onboarding is autonomous, and a job Yulius CI cannot run (it needs
   a Windows or macOS runner) cannot be translated. The prompt only covers the GitHub
   permission case.
4. **The `shell-tests` job failed only on Yulius CI: `chmod 000` does not make a file unreadable
   for root.** Agents and CI jobs run as uid 0 in the sandbox's user namespace (`id` in a task:
   `uid=0(root)`). Nothing tells the onboarding agent, so its proof run passed for reasons it
   could not see, or failed with a confusing message.
5. **"Yulius's 'Recent changes' section reads the top of `knowledge/log.md`, but that log adds
   new entries at the end, so the section shows the oldest entry."**
   - `RepoDocs::read` (`hooks.rs`) takes the first 4,000 characters of the changes log.
   - The briefing quotes them under "From …".
   - Rule 6 of the discipline says "Add a short entry at the top of …".
   For an oldest-first log, the briefing therefore shows stale entries and asks for an entry at
   the wrong end.

Not changed here:
- The new jobs also ran on `main` and on the older PR #55, and turned them red (screenshot 1).
  That is the intended contract: CI jobs are read from the integration branch and gate every
  ref. The fix reaches those refs by a merge, which the agent explained correctly.
- The red runs' root cause (item 4) was fixed in studio itself.

Related work:
- SPEC-054 / ADR-0037 added the dead-code tools and the `dead-code-cleanup` skill. This spec
  makes onboarding install them.
- SPEC-106 (task #91) fixed onboarding's trust prompt and lost task.
- SPEC-050 DLV-204 (`note_undelivered`) says why a head was not delivered.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| OBG-001 | Both onboarding prompts (the preparation and the environment-only one) tell the agent to install the project's gate in the onboarding pull request itself, not as follow-ups: a test command, a lint and format check, and a dead-code and unused-dependency check (skill `dead-code-cleanup`, pinned, allowlist committed), in `[commands]` and `[[ci.jobs]]`, with a CI job covering every project of a monorepo. Existing findings do not change application code. The agent commits the tool's baseline or allowlist, or scopes the check to what passes today, so that the gate passes now and fails on new findings. It then files one issue to burn the baseline down. | `onboarding::tests::prompts_install_the_gate_in_the_onboarding_pull_request` |
| OBG-002 | The preparation task's steps include wiring the lint, format and dead-code checks into the gate and giving every project its CI job. The environment-only task's steps include the gate too. | same test |
| OBG-003 | A workflow whose jobs need a runner Yulius CI lacks (Windows, macOS) is kept, and the decision is recorded as a decision record, with no person asked to accept it. Every other workflow is translated and deleted, as before. | same test |
| OBG-004 | The prompts say that the agent and every CI job run as root in the sandbox. A test that relies on file permissions skips when the current user can still read the file. | same test |
| OBG-005 | The preparation prompt's delivery step follows the standard pipeline: commit, mark the steps done, get the independent review of the exact head, then call `delivery_merge`. Start `yulius wait` in the background and end the turn. It no longer suggests the pull request appears on its own while the agent keeps working. The environment-only prompt says the same when something changed. | same test |
| DMG-001 | `delivery_merge` delivers the head before it decides. When the task's branch has no pull request yet, or its open pull request is behind the local head, Yulius pushes and opens or updates it. It uses the same readiness rules and author as at the end of a turn (`try_auto_open_pr`), then merges or arms. A goal task no longer depends on a turn's end to get its pull request. When delivery is blocked (invalid spec, open step, uncommitted work), the call is refused with that reason instead of arming a merge with no pull request. | `delivery::a_merge_call_opens_the_pull_request_a_goal_task_still_lacks` |
| CHG-001 | The briefing finds which end of the changes log is newest from its ISO dates (`YYYY-MM-DD`). When the first date in the file is earlier than the last, the log is oldest-first. Yulius then reads the last 4,000 characters (from a line start) instead of the first, labels the quote "newest entries, at its end", and discipline rule 6 says "at the end of". Without two dates, the log reads as newest-first, as before. | `briefing::tests::an_oldest_first_changes_log_is_quoted_from_its_end` (`hooks.rs` reads the log through `changes_excerpt`) |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [ ] Onboarding installs the gate: lint, format, dead-code check and CI for every project
- [ ] Onboarding delivers like every task and knows the sandbox's limits
- [ ] delivery_merge opens the pull request a goal task still lacks
- [ ] The briefing quotes the newest end of an oldest-first changes log
- [ ] Decision record, architecture, product and SPEC-002 describe the new onboarding contract
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- A new onboarding task's prompt asks for the lint, format and dead-code checks and CI jobs for
  every project in its own pull request, and never calls them follow-up pull requests.
- An agent that has committed its work, finished its steps and calls `delivery_merge` gets its
  pull request opened by that call and the merge armed. It no longer needs its turn to end.
- A changes log whose entries run oldest-first is quoted from its end in the briefing, and rule
  6 asks for a new entry at the end.

## Test plan

- Unit (`yulius-app` `onboarding::tests`): the prompts' new wording and steps (OBG-001…005); the
  old "as follow-up pull requests" wording is gone.
- Integration (`crates/yulius/tests/delivery.rs`, existing fixture with the fake GitHub): a ready
  task with no pull request calls `delivery_merge`. The pull request is created, the branch is
  pushed and the merge is armed. With a step still open, the call is refused and nothing opens.
  The test fails before the change: the merge arms and no pull request is created.
- Unit (`yulius-domain` `briefing::tests`, `yulius-app` `hooks`): an oldest-first log is quoted
  from its end with the "at the end" rule. A newest-first log and a log without dates keep the
  top.
- Full gate: `scripts/dev ci`.

## Decisions

- [ADR-0094](../../docs/decisions/0094-onboarding-installs-the-gate-in-its-own-pull-request.md):
  onboarding installs the gate in its own pull request and ratchets existing findings with a
  baseline instead of fixing them or deferring the checks.
- Fix the goal deadlock in `delivery_merge` rather than at the goal verdict. Opening the pull
  request on every "not met" verdict would also work. The merge call, however, is where the
  agent says "deliver this head". It already requires the review, and it answers synchronously,
  so the agent learns about a push failure at once. Ending turns stays the briefing's advice for
  waiting.
- Assumed (reversible): the order of a changes log is read from its ISO dates, with no new
  `yulius.toml` key. Logs carry dates in practice (Yulius's own, studio's `knowledge/log.md`),
  and a key would be one more thing onboarding must remember to set.
- Not now: setting up deploy environments during onboarding. They need credentials and target
  machines that only a person can provide. The prompt keeps asking the agent to file an issue for
  them (the briefing's "only a person can do it" rule).
