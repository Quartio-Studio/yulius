---
yulius-task: 1ddb4937-1438-4719-96e8-ec4439e7a956
status: done
---

# SPEC-116: onboarding leaves the project ready to work in the discipline

Status: done
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
| DMG-001 | `delivery_merge` delivers the head before it decides. When the task's branch has no pull request yet, or its open pull request is behind the local head, Yulius pushes and opens or updates it. It uses the same readiness rules and author as at the end of a turn (`try_auto_open_pr`), then merges or arms. A goal task no longer depends on a turn's end to get its pull request. When delivery is blocked (invalid spec, open step, no plan for a task that needs a spec, uncommitted work), the call is refused with that reason instead of arming a merge with no pull request. A push GitHub refuses answers with the remedy `yulius wait` gives (merge the pushed head back in, never force-push). | `delivery::a_merge_call_opens_the_pull_request_a_goal_task_still_lacks` |
| CHG-001 | The briefing finds which end of the changes log is newest from its ISO dates (`YYYY-MM-DD`). When the first date in the file is earlier than the last, the log is oldest-first. Yulius then reads the last 4,000 characters (from a line start) instead of the first, labels the quote "newest entries, at its end", and discipline rule 6 says "at the end of". Dates in entry headings (`##` and deeper, not the title) decide when there are two. Without two dates, the log reads as newest-first, as before. When the section's share of the briefing is smaller than the excerpt, the quote loses its oldest lines, not its newest. | `briefing::tests::an_oldest_first_changes_log_is_quoted_from_its_end` (`hooks.rs` reads the log through `changes_excerpt`) |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Onboarding installs the gate: lint, format, dead-code check and CI for every project
- [x] Onboarding delivers like every task and knows the sandbox's limits
- [x] delivery_merge opens the pull request a goal task still lacks
- [x] The briefing quotes the newest end of an oldest-first changes log
- [x] Decision record, architecture, product and SPEC-002 describe the new onboarding contract
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

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
- Accepted limit: a changes log is read up to the 256 KB bound of every briefing document. For an
  oldest-first log larger than that, "its end" is the end of what was read. No real log is that
  large, and reading from the end would need a second reader.
- Not now: setting up deploy environments during onboarding. They need credentials and target
  machines that only a person can provide. The prompt keeps asking the agent to file an issue for
  them (the briefing's "only a person can do it" rule).
- Review findings (two rounds, then a check of the last fixes):
  - Round 1 found that the section's share of a real briefing cut the quote from its top. It is
    now fitted from the end (`recent_changes_within`, tested at `DEFAULT_BUDGET_BYTES`).
  - Round 1 also asked to document the "no plan" refusal, to answer a refused push with its
    remedy, to clear a stale undelivered note, and to fix the prompt's step order and test
    carve-out.
  - Round 2 approved, with two Low findings that are now fixed: a stored push note is used only
    for the head being merged, and a title's date never decides the order.
  - Not covered by a test: a non-push failure while a note about an earlier head is stored. The
    fake GitHub cannot fail `dlv_observe`. The filter is the same commit comparison the review
    check uses.

## Evidence

- `cargo test -p yulius-app --lib onboarding::tests`: 6 passed.
  - It includes `prompts_install_the_gate_in_the_onboarding_pull_request` and
    `prompts_deliver_like_every_task_and_name_the_root_sandbox`.
  - Both failed before their slices, with "missing \"in this pull request, not as follow-ups\""
    and "missing \"run as root\"".
- `cargo test -p yulius --test main delivery::a_merge_call_opens_the_pull_request_a_goal_task_still_lacks`:
  passed.
  - Before the fix, the call answered `armed: true, checks: none` and no pull request was created.
  - The refused-push case answered with git's raw text before the review fix.
- `cargo test -p yulius-domain --lib briefing`: 14 passed.
  - `an_oldest_first_changes_log_is_quoted_from_its_end` failed with the generic cut in place, and
    again before the title-date fix.
- `cargo test -p yulius --test main` on `delivery::`, `playground::`, `merge_train` and `agent`:
  112 passed.
  - `merge_train::a_red_train_falls_back_to_one_by_one_merges` failed once under the load of 113
    concurrent tests. It passed three times alone, and its duration on `origin/dev` is 31–45 s
    against a 50 s bound: issue #443.
- `scripts/dev ci` at `8677066c` (after `python3 scripts/bootstrap.py`): exit 0.
  - fmt, clippy `-D warnings`, deny, shear and docs all passed.
  - nextest: "1608 tests run: 1608 passed (3 slow), 16 skipped".
  - The final head adds only the two Low fixes, their tests and this spec; its gate result is in
    the pull request's Yulius CI.
- `spec_check`: valid.
- Not testable here: a real onboarding run of another repository with Claude Code. The prompts
  and the merge path are covered by unit and integration tests with the fake agent and fake
  GitHub.
