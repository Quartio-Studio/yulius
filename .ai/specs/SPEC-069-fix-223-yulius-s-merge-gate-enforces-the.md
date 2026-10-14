---
yulius-task: ae80c897-6658-4df6-82d1-81be0dc84b0b
status: done
---

# SPEC-069: Yulius's merge gate enforces the branch policy

Status: implemented
Date: 2026-10-13
Owner: Yulius task #59
Scope authorization: GitHub issue #223, through task #59 (part of task #47, "fix all open
actionable GitHub issues"). No new GitHub issue is created.

## Goal

Before Yulius merges a pull request it checks the pull request's title, head and base against the
project's branch policy. A violation blocks the merge and says the rule in words; a valid pull
request merges as before. The policy is configured per project in `yulius.toml`; this repository's
rules are the standard preset and the default.

## Context

- `scripts/check_git_policy.py` holds the rules (typed short-lived branches, conventional one-line
  titles of at most 100 characters, changes into `dev`, promotions `dev` → `main` titled
  `release: promote dev to main[ - label]`). It ran only as the GitHub Actions workflow
  `policy.yml`, which SPEC-010 (#184) removed. Since then nothing enforces it (#223).
- Every merge of a task's pull request goes through `App::dlv_merge`
  (`crates/yulius-app/src/delivery.rs`): `delivery_merge` (agents), the armed merge fired by
  `agent_wait` and the Git page / task page (`POST /api/tasks/{tid}/delivery/merge`).
  `tool_delivery_merge` arms a merge while checks run, before `dlv_merge` is reached.
- Promotions go through `App::dlv_promote`: with `promote = "merge_commit"` Yulius opens a
  promotion pull request titled `Promote dev to main` (which the policy would refuse) and merges
  it on a later request. Fast-forward promotions have no pull request.
- `yulius.toml` is parsed by the domain's strict TOML subset (`spec_format/config.rs`); unknown
  tables are ignored, so a new table does not break an older Yulius reading this repository.
  CI reads `yulius.toml` at a revision of the sandbox clone (`GitPort::show_file`).
- Yulius names task branches `feat/<number>-<slug>` (`supervisor::branch_for`) and titles task
  pull requests with `default_pr_title` (the task title, prefixed with the type of the oldest
  conventional commit), clipped at 200 characters. Recent titles exceed 100 characters.
- `GhPull` does not say whether the head is in another repository (a fork).
- 2026-10-14 (BPL-008): the squash commits on `dev` mislabel changes. #381 "docs(spec): fix #364:
  the repository check catches committed conflict markers" changed `scripts/check_repository.py`
  (commits `docs(spec)`, `fix(check)`, `docs`, `fix(check)`); #382 "docs(spec): yulius doctor
  warns about a GitHub CLI older than 2.82.1" is a `feat(doctor)` change. `default_pr_title` took
  the type of the branch's oldest conventional commit, in Yulius's flow always the spec commit.
  Of the 64 merged pull requests Yulius opened in the `yulius` sandbox (audit `delivery.pr.open`,
  69 opened), 30 carry that oldest type while a more significant commit is on the branch (22
  `docs(spec)`, 1 `docs(sessions)`, 6 `test`, 1 `fix`; 18 of them are features, 12 fixes). Yulius also never revisits the title: 53 of
  the 69 got commits after it opened them, and it squash-merges them under the title it gave at
  the start.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| BPL-001 | The standard policy: into the integration branch, the head is `<type>/<slug>` (type one of feat, fix, refactor, perf, docs, test, build, ci, chore, revert; slug lowercase letters and digits in hyphen-separated words with at least one letter; at most 80 characters) and the title is `<type>[(scope)][!]: <description>`, one line of at most 100 characters. Into the production branch, only the integration branch of this repository, titled `release: promote <integration> to <production>` with an optional ` - <label>`. Any other base is refused. | `branch_policy::tests` |
| BPL-002 | `yulius.toml` `[branch_policy]` configures it: `preset = "standard"` (default) or `"none"` (no checks), and `types = "feat fix …"` replacing the standard types (it must include `feat`, the type of Yulius's task branches). Unknown keys and invalid values are errors with their line, also for the CI's and the spec check's parse. Without the table the standard preset applies. | `spec_format::config::tests` |
| BPL-003 | The policy is read from `yulius.toml` at `origin/<integration branch>` of the sandbox clone, so a pull request cannot relax the policy it is checked against. When the file is missing the default applies; when it is unreadable or invalid the standard preset applies (fail closed), a warning is logged and a refusal says why. | `delivery::the_branch_policy_gates_every_merge` |
| BPL-004 | `dlv_merge` (agents' `delivery_merge`, armed merges, the Git page) refuses a pull request that violates the policy with a conflict naming the rule in words, the offending value and how to fix it (edit the title, or move the work to a typed branch); nothing is merged. `delivery_merge` refuses before arming, so no violating merge is armed. | `delivery::the_branch_policy_gates_every_merge`, `delivery::agents_cannot_arm_or_land_a_merge_the_policy_refuses` |
| BPL-005 | Promotion pull requests Yulius opens are titled `release: promote <integration> to <production>`. Merging an existing promotion pull request is refused when its title violates the policy or its head is from another repository; a fork's pull request is never taken for the promotion pull request. | `delivery::standard_flow_merges_on_green_checks_and_promotes_by_pr`, `delivery::the_branch_policy_gates_every_merge` |
| BPL-006 | Titles and branch names Yulius generates pass the policy: `default_pr_title` uses a type the policy allows (the most significant allowed conventional commit, BPL-008, else the branch's type) and clips the title to the policy's 100 characters; `branch_for` always yields a slug with a letter. | `delivery::tests::generated_titles_pass_the_policy`, `supervisor::tests::branch_names_are_safe_and_bounded` |
| BPL-007 | `scripts/check_git_policy.py` and its test are removed: the Rust rules are the single source; the docs and `yulius.toml` say the merge gate enforces the policy. | docs diff |
| BPL-008 | A task pull request's default title names the change: the task's title after the type and scope of the branch's most significant conventional commit (feat > fix > perf > refactor > revert > build, ci > test > docs > style > chore; any other type lowest; the oldest of equals), not of its oldest. Under an enforced policy only its types count and its length applies; without the policy any conventional-looking type counts; a repository without conventional commits keeps the task's title. Before Yulius merges a task pull request (`dlv_merge`: the armed merge, the merge train, `delivery_merge`, the Git page), it recomputes the default from the pull request's final commits and, when the pull request still carries a title Yulius gave it, sets the new one on GitHub (audit `delivery.pr.retitle`) before the policy check and the merge, so the squash commit carries it. A title Yulius gave is the task's title with the type of one of the branch's commits, or with the branch's type (by the rule then or now: commits are only added); any other title, written by a person or an agent, is kept. Nothing is merged under a title Yulius could not set. When GitHub cannot take it for now (a rate limit, a 5xx, a timeout, the network), the merge stays armed and the next pass tries again: the armed merge keeps waiting, the merge train waits (within its wait limit), and an agent's `delivery_merge` on green arms the merge. When GitHub refuses it (a 4xx: validation, permission, missing), the merge is refused with the reason and how to set a title (`gh pr edit <n> --title "…"`, which Yulius keeps); `yulius wait` gives the agent that text. | `delivery::tests::the_default_title_names_the_most_significant_change`, `delivery::tests::a_title_yulius_gave_is_renewed_before_the_merge_and_a_written_one_kept`, `delivery::a_merge_names_the_change_by_its_most_significant_commit`, `github_client::tests::a_refused_write_is_told_from_one_that_may_pass`, `client::pull_request_merge_refuses_stale_head` |

## Design

- `crates/yulius-domain/src/branch_policy.rs` (pure): `BranchPolicy { types: Option<Vec<String>> }`
  (`None` = preset none), `BranchPolicy::standard()`, `check(&PullFacts, integration, production)
  -> Result<(), PolicyViolation>`, `conventional_head(title)` and `branch_type(branch)` for the
  title generator. `PolicyViolation` is an enum whose `Display` states the rule in words. No regex
  dependency: the grammar is small and checked by hand.
- `spec_format/config.rs`: the `[branch_policy]` table in `Document`, `parse_yulius_toml_branch_policy`
  and validation inside `parse_yulius_toml` (result discarded, so `ProjectConfig` is unchanged).
- `GhPull.from_fork` (head repository differs from the base repository or is gone), mapped from
  the GitHub wire `head.repo.id` / `base.repo.id`.
- `App::dlv_branch_policy(sandbox, profile)` reads the policy; `App::dlv_check_policy` maps a
  violation to `AppError::Conflict` with the fix hint. Called in `dlv_merge` before the merge,
  in `tool_delivery_merge` before arming, and in `dlv_promote` before merging an existing
  promotion pull request.
- BPL-008: `type_rank` orders the conventional types; `default_pr_title` keeps the first (oldest)
  head of the highest rank. `merge_title(current, task title, subjects, branch, policy)` (pure)
  answers the title to set before a merge, or `None` to leave it. `App::dlv_merge_title` reads the
  pull request's commits (`review_commits` from `origin/<integration>` to its head, at most 50, in
  the task worktree), calls it, sets the title with `GitHubPort::update_pull_title` (`PATCH
  /repos/{owner}/{repo}/pulls/{number}`, `yulius-github` `update_pull_title`) and audits it. It
  runs in `dlv_merge` before `dlv_check_policy`, which checks the new title. Commits that cannot
  be read leave the title as it is.
- BPL-008 failures: `update_pull_title` answers `Upstream` only for what may pass (the adapter's
  `map_refusal` turns GitHub's 4xx answers into `Invalid`, `Forbidden`, `NotFound`,
  `Unauthenticated` or `Conflict`). `dlv_merge` and `merge_task_pr_for_task` return
  `MergeFailure::Later` for a title that may pass and `MergeFailure::Refused` for everything
  else; `merge_armed_task` keeps the armed merge on `Later`, `train_member` answers
  `Step::Wait`, and `tool_delivery_merge` arms the head. The Git page answers the error.

## Plan

- [x] Spec SPEC-069 and ADR 0051 for the branch policy gate
- [x] Branch policy rules in the domain with unit tests
- [x] [branch_policy] table in yulius.toml parsed and validated
- [x] Merge gate refuses violating task pull requests (delivery_merge, armed, Git page)
- [x] Promotion pull requests are titled and checked by the policy
- [x] Yulius-generated titles and branch names pass the policy
- [x] Python branch policy script removed; docs and yulius.toml updated
- [x] Gates green, independent review, evidence and changes entry

## Acceptance criteria

- A pull request into `dev` with a non-conventional title or an untyped branch is refused by the
  merge gate with the reason shown, and merges after the title is fixed.
- A pull request into `main` that is not `dev` → `main` with a release title is refused.
- Valid pull requests merge as before; with `preset = "none"` nothing is checked.
- Titles Yulius generates for task and promotion pull requests pass the standard policy.

## Test plan

- Unit: `branch_policy::tests` cover every accepted and refused case of the Python tests (same
  cases), custom types and preset none; config tests cover the table's keys and errors;
  `default_pr_title` and `branch_for` tests cover generated names.
- BPL-008: unit tests over the commits of #381 and #382 and the ranking; an integration test
  where pull requests opened after the spec commit alone merge as `feat(doctor)` (armed merge)
  and `fix(check)` (Git page) with the squash commit titled so; a 502 on the first retitle keeps
  the armed merge, which lands on the next pass; a 502 on `delivery_merge` arms the merge and a
  422 on the next pass stops it with the reason for `yulius wait`; a title a person wrote stays.
- Integration (`crates/yulius/tests/delivery.rs`, fake GitHub over a real bare repository): a
  non-conventional title is refused on the Git page route and via `delivery_merge` (before arming),
  an untyped branch is refused, a fixed title merges; `[branch_policy] preset = "none"` at
  `origin/dev` lets the same pull request merge; a promotion pull request with a bad title or from
  a fork is refused, and the one Yulius opens is titled `release: promote dev to main`.

## Decisions

- Recorded in [ADR 0051](../../docs/decisions/0051-the-merge-gate-enforces-the-branch-policy.md):
  the rules live in Rust and are checked at merge time from the integration branch's
  `yulius.toml`; the standard preset is on by default; the Python script is removed.
- A new `[branch_policy]` table, not keys in `[delivery]`: `[delivery]` keys are strict, so an
  older Yulius (the one running CI) would reject this repository's `yulius.toml`.
- The policy is read from the sandbox clone as last fetched, like the CI's configuration: no
  fetch per merge. A policy change on GitHub applies once the clone is fetched (each CI round,
  each merge Yulius makes).
- Titles are checked at merge, not when the pull request is opened: an agent or a person can
  retitle it until then, and the refusal says exactly what to change.
- BPL-008: a title Yulius gave is told from a written one by recomputing the titles Yulius could
  have given (the task's title with any of the branch commits' types, or the branch's type), not
  by storing the title it sent: no new persistence, and the pull requests already open when this
  ships (titled by the oldest-commit rule) are renewed too. A task renamed after its pull request
  opened keeps the old title (left alone, the safe side). Someone who writes exactly such a title
  gets the default instead, which is the same kind of title.
- BPL-008: the ranking follows how much a type says about the change (features, then fixes, then
  the rest); `revert` sits above build and docs because it undoes a change of any type. A breaking
  mark (`!`) stays with its commit's head and does not change the rank.
- BPL-008: Yulius never merges under a title it knows is wrong (history is permanent), and a
  GitHub outage only delays the merge: it stays armed, as for other failures that pass. Only
  GitHub's refusal stops it, with what to do.
- Only `preset` and `types` are configurable: the length limits and the release title follow the
  repository's rules; the parser has no numbers, and no project needs other limits yet.

## Evidence

- `scripts/dev ci` (bootstrap, repository contracts, Python script tests, fmt, clippy, the
  workspace tests with nextest, doctests) at `7093530`: exit 0, `1329 tests run: 1329 passed
  (10 slow), 16 skipped`.
- After the last review nits (a comment and the refusal's wording):
  `cargo test -p yulius --test main -- delivery::the_branch_policy delivery::agents_cannot`:
  2 passed; `cargo clippy -p yulius-app --all-targets`: clean.
- Unit: `branch_policy::tests` (7, the removed Python tests' cases included),
  `spec_format::config::tests::branch_policy_table_selects_a_preset_and_types`,
  `delivery::tests::generated_titles_pass_the_policy`,
  `supervisor::tests::branch_names_are_safe_and_bounded`,
  `wire::tests::a_head_from_another_repository_is_marked`.
- Integration: `delivery::the_branch_policy_gates_every_merge` (a non-conventional title, a title
  that is too long and an untyped branch are refused with the rule, a fixed title merges; the
  promotion pull request is titled `release: promote dev to main`, a retitled one is refused, a
  fork's is not taken; an invalid policy falls back to the standard rules and says why;
  `preset = "none"` lets the untyped branch merge) and
  `delivery::agents_cannot_arm_or_land_a_merge_the_policy_refuses` (`delivery_merge` refuses
  before arming; an armed merge retitled afterwards is refused, not merged).
- Independent review: round 1 requested changes (configured `types` without `feat`, the fallback
  refusal's reason, the untested fail-closed path); all fixed test-first. Round 2 approved
  `7093530`; its minor findings (two stale doc sentences) and nits are fixed in the last commit.
- Not tested here: merges against GitHub itself (the fake GitHub over a real bare repository
  stands in for it).
- BPL-008, red first: with the oldest-commit rule restored,
  `the_default_title_names_the_most_significant_change` (left `docs(spec): fix #364: …`),
  `a_title_yulius_gave_is_renewed_before_the_merge_and_a_written_one_kept` and
  `delivery::a_merge_names_the_change_by_its_most_significant_commit` failed; with the ranking but
  no retitle before the merge, the integration test failed (the pull request still read
  `docs(spec): yulius doctor warns …` when merged). With a title GitHub could not take treated
  as a refusal, the same test failed: the armed merge was dropped after one 502
  (`merge_armed: false`) instead of landing on the next pass. With all: the 16 `delivery::tests`,
  `github_client::tests::a_refused_write_is_told_from_one_that_may_pass`, the 33
  `delivery` integration tests, the 34 `yulius-github` tests (`--features fake`) and the
  neighbouring `merge_train`, `local_ci`, `supervision`, `task_ownership`, `skills_mcp`, `intake`,
  `automation_start`, `http_auth`, `ui_contract`, `github_checks`, `mcp_coordination` and
  `manage_tools` suites (108) pass.

## Changelog

- 2026-10-13: written with the fix (#223).
- 2026-10-14: BPL-008, the default title takes the most significant commit's type and is renewed
  from the final commits before Yulius merges (BPL-006 changed: it took the oldest commit's).

