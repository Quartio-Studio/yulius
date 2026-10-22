---
yulius-task: fd0f272e-059c-4a3a-bc52-c2b3894db380
status: in-progress
---

# SPEC-267: tasks read another repository of their owner without asking a person

Status: in-progress
Date: 2026-10-22
Owner: Yulius task #253
Scope authorization: task #253: two screenshots and "i got this, such things should usually happen
on onboarding … and it turned out i didn't need to do it at all, fix". Fixes #803. Screenshot 1: a
task of Quartio-Studio/quartio-v2 shows "Needs you: Give the Yulius GitHub app read access to
Quartio-Studio/product … The repository returns 404 with this token". Screenshot 2: the person's
Superagent answered that nothing was needed: with the person's own `gh` sign-in it cloned the
repository into the sandbox as a read-only reference copy and told the task to use it.

## Goal

When a task needs to read another GitHub repository of its project's owner, Yulius fetches it for
the task as the person who owns the task, with no person's action; onboarding does it up front for
the repositories the project refers to. A person is asked (`needs_you`) only when the task's owner
cannot read the repository either.

## Context

- Symptom: an agent's Git and `gh` hold an installation token restricted to the sandbox's own
  repository (ADR-0114, `GitHubPort::agent_token`), so another repository answers 404, and the agent's
  only path is `needs_you`. The fix it suggests (adding the repository to the App's access) is not
  needed and would also give the App, and so agents' tokens elsewhere, more than read access.
- What actually worked: the Superagent runs with the person's own `gh` (`identity.rs` shim), so it
  could clone the repository; tasks had no equivalent.
- Root cause: no Yulius path gives a task read access to another repository; the onboarding prompt
  never asks about other repositories, so the need surfaces mid-task. SPEC-001 PRJ-002 said
  "Checking out another repository inside a sandbox is not supported; cross-project knowledge
  comes through MCP", and nothing covered a private repository without an MCP server.
- Precedent: `production_protect` (SPEC-157, ADR-0129) acts on GitHub as the task's owner through a
  tool instead of handing the agent a person's token.
- Constraints: the server must never run Git inside a folder an agent can write (a planted
  `.git/config` or hook would run on the host), and writes into the sandbox only through handles
  opened without links (ADR-0174). Task attachments (`uploads.rs`) already place a file in
  `<sandbox>/attachments/tasks/<task>/` that way, and the agent opens it by its absolute path.

## Requirements

- **REF-001** A task tool `reference_repo` takes `repo` (`owner/name`). Yulius fetches the
  repository's default branch, with its history, as the task's owner (their GitHub user token),
  into a fresh folder of the server's own (`<data>/tmp`, never inside a sandbox), bundles it
  (`git bundle`, the branch only) and places the bundle among the task's attachments through the
  attachment upload path (handles; it replaces the task's earlier bundle of the same repository,
  never a link or a folder). The fetch folder and the server's bundle are removed afterwards, on
  failure too, and also when the agent's request goes away (the fetch runs detached to its end); a
  fetch folder older than 2 hours (left by a restart mid-fetch) is swept by the next fetch. At most
  2 fetches run at once on the instance (`Capacity` beyond).
- **REF-002** It answers the bundle's absolute path, the branch, its commit, the size and the
  commands to clone it (or update an earlier copy) into `<sandbox>/reference/<owner>/<name>`,
  outside every worktree, and to remove the bundle afterwards; the agent runs Git on it itself, in
  its containment.
- **REF-003** Refused, with what a person must do and that only then a person is asked with
  `needs_you`: a repository of another owner than the sandbox repository's, judged by the owner
  GitHub answers with (a repository moved to another owner is that owner's); an owner who no
  longer exists, is disabled or has no usable GitHub sign-in; a repository GitHub says the owner
  cannot read (or does not answer about); a failed fetch; a bundle above 1 GiB. Refused with why
  but no person to ask: the sandbox's own repository (the worktree has it) and a playground (no
  GitHub owner). A malformed `repo` is `Invalid`.
- **REF-004** Each fetch is audited (`task.reference_repo`, the repository, commit and person).
- **REF-005** The onboarding prompt (both the preparation and the environment-only one) asks the
  agent to fetch, with `reference_repo`, the other repositories the project's code, docs or
  instructions refer to, and to name them in AGENTS.md so later tasks fetch them the same way; it
  never asks a person for GitHub App access to read one.
- **REF-006** `needs_you`'s description and the briefing's tool list send an agent that needs
  another repository to `reference_repo` first.
- **REF-007** ADR-0197 records the decision; SPEC-001 PRJ-002 and AC-PRJ-2, `docs/architecture.md`,
  `docs/product.md` and `docs/api.md` (if it lists task tools) say the same.

## Plan

- [x] Spec and ADR-0197 record that tasks read other repositories of their owner through Yulius
- [x] Agents get a reference_repo tool that delivers a bundle fetched as the task owner
- [x] Onboarding and needs_you send agents to reference_repo instead of a person
- [x] Docs and gates are green and review round 1 findings are fixed test-first
- [ ] Independent review approves the head and the change is merged into dev

## Acceptance criteria

- **AC1** A task whose owner can read `OWNER/lib` calls `reference_repo {repo: "OWNER/lib"}`: the
  answer names a bundle under the task's attachments folder; `git clone --branch main <bundle>`
  gives the repository's files at the answered commit; GitHub was asked with the owner's token.
- **AC2** Calling it again after the repository moved gives a new bundle with the new commit, in
  place of the earlier one.
- **AC3** Refused, with `needs_you` in the text and nothing fetched: another owner's repository,
  a repository GitHub moved to another owner, a repository the owner cannot read, a disabled
  owner, an owner without a GitHub sign-in. The sandbox's own repository and a playground are
  refused saying why. A malformed `repo` is invalid.
- **AC4** No folder is left in `<data>/tmp` after a success or a failed fetch; a fetch folder older
  than 2 hours is swept by the next fetch, a recent one stays.
- **AC6** The real Git adapter bundles exactly the default branch so that it clones to its commit,
  and refuses a branch Git could read as an option or a refspec before Git runs.
- **AC5** The onboarding task's prompt names `reference_repo`; `needs_you`'s description names it.

## Test plan

- Integration (`crates/yulius/tests/reference_repo.rs`, the shared repos `World`: a local Git remote
  per GitHub URL and a GitHub stub with per-person grants and moved repositories): AC1-AC4 through
  the MCP endpoint with a live attempt's token; AC5 from the served tool list and
  `onboarding::tests::prompts_fetch_referenced_repositories_without_a_person`; AC6 with
  `GitAdapter::with_github_base` against a local bare remote.
- Unit: the `repo` argument's parsing.
- Not testable here: a real GitHub fetch with a real user token, a real agent's containment
  cloning from the bundle (the test clones it on the host), and a request dropped in the middle of
  a fetch (the detached run is `App::run_detached`, ADR-0053's tested mechanism).

## Decisions

- [ADR-0197](../../docs/decisions/0197-tasks-read-other-repositories-of-their-owner-through-a-bundle.md):
  a Yulius tool fetches as the task's owner and hands the agent a bundle; the agent never holds a
  person's token and the App's access is not widened.
- Same owner only: the case at hand (an organisation's other repository) is covered, and a prompt
  injected into a task cannot pull in the person's repositories of other organisations; a public
  repository of another owner the agent clones itself.
- Any task, not only onboarding: reading adds nothing a person must consent to beyond starting a
  task in a project of that owner, and the need appeared in a feature task. Onboarding does it
  first so later tasks find the repositories named in AGENTS.md.
- A bundle, not a checkout written by the server: Git runs on the host only in a fresh private
  folder, and the only write into the sandbox is one file through the attachment path's handles.
- A fresh fetch per call, not a cached mirror: nothing to lock, invalidate or clean up after a
  sandbox's removal; reference fetches are rare. Bounded by 2 fetches at once and a sweep of
  folders a restart left (review round 1).
- The owner GitHub answers with decides, not the name asked for (review round 1): GitHub's API
  and Git follow a transferred repository's redirect.
- One bundle per repository per task, replaced on each call (review round 1), so repeated calls do
  not pile up gigabytes in the attachments folder.
