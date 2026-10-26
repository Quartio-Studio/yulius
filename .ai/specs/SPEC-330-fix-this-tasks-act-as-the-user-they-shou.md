---
yulius-task: 7e50bc4f-5ce9-40b0-8e45-0b2e64cd0df6
status: in-progress
---

# SPEC-330: a task's reference copy carries every branch its owner can read

Status: in-progress
Date: 2026-10-26
Owner: Yulius task #314
Scope authorization: task #314: a screenshot and "Fix this. Tasks act as the user. They should have
all things that the user has". Fixes #915. The screenshot: a task of `pomerado-rust` shows "Needs
you: Quartio v2 `dev` is unreachable: reference_repo fetches only the default branch (main) and
gh/git get 404 … or set dev as default"; beside it the person's Superagent, with the person's own
`gh`, has Quartio v2 `dev` cloned and sends its files to the tasks by hand.

## Goal

A task that reads another repository of its owner through `reference_repo` (SPEC-267) gets every
branch and tag that person can read there, not only the default branch, so no person has to copy
files or change the repository's default branch for it.

## Context

- Symptom: `reference_repo {repo}` answered `main`; the task needed `dev`, could not ask for it, and
  its own `gh`/`git` answer 404 (its installation token reaches only its own repository, ADR-0114).
  The only way left was `needs_you`.
- Root cause: `App::tool_reference_repo` always passes GitHub's default branch to
  `GitPort::bundle_branch`, which fetches `+refs/heads/<branch>:refs/heads/<branch>` only and bundles
  that one ref (`crates/yulius/src/wiring/git.rs`). SPEC-267 REF-001 specified "the repository's
  default branch" and its tests (`the_git_adapter_bundles_one_branch` asserted "only the branch")
  pinned exactly that, so nothing caught that the owner's read access covers every branch.
- The person's read access is per repository on GitHub, not per branch: the default-branch-only
  copy gave the task less than its owner has, which is what the person asks to stop.
- What else the cause could break: an agent told "reference_repo fetches only main" may ask a person
  to change the repository's default branch, which would change it for everyone.

## Requirements

- **REF-101** The reference fetch takes every branch and every tag of the repository
  (`+refs/heads/*:refs/heads/*`, `+refs/tags/*:refs/tags/*`) with the owner's token into the
  server's fresh folder, and the bundle carries all of them plus `HEAD`, which names the default
  branch. Still no submodules; size cap, concurrency, cleanup and audit as SPEC-267.
- **REF-102** `reference_repo` takes an optional `branch`: the branch the answer's clone command
  checks out (default: GitHub's default branch). A `branch` Git could read as an option or a
  refspec is `Invalid` before anything is fetched; a well-formed one the repository lacks is
  `Invalid`, names the repository's branches, places no bundle and asks no person.
- **REF-103** The answer names the checked-out branch and its commit (as before), the default
  branch, the number of branches and tags it carries, and the branches with their commits (at most
  100 listed); its message says how to clone it, check out another branch of the copy, and update
  an earlier copy with every branch and tag.
- **REF-104** The tool's description, ADR-0223 (amending ADR-0197), SPEC-267's pointer,
  `docs/architecture.md` and `docs/product.md` say the copy carries every branch and tag.

## Plan

- [x] Spec, issue and ADR record that a reference copy carries every branch and tag
- [x] Regression tests show a task cannot get a non-default branch
- [x] reference_repo bundles every branch and tag and checks out the asked branch
- [ ] Docs, gates and security scan are green
- [ ] Independent review approves the head and the change is merged into dev

## Acceptance criteria

- **AC1** A repository with `main` (default) and `dev` (a commit of its own) and a tag `v1`:
  `reference_repo {repo, branch: "dev"}` answers `branch: dev` with `dev`'s commit; cloning the
  bundle as the answer says gives `dev`'s files.
- **AC2** Without `branch` the answer is `main` and its commit; the clone also has `origin/dev` and
  the tag `v1`, and `branches` lists both branches with their commits.
- **AC3** `branch: "nope"` is invalid, names `dev` and `main`, says nothing of `needs_you` and leaves
  no bundle and no fetch folder; `branch: "-x"` is invalid before any fetch.
- **AC4** The real Git adapter bundles `HEAD`, every branch and every tag of a local remote and
  answers every branch's commit; a default branch Git could read as an option is refused before Git
  runs.

## Test plan

- Integration (`crates/yulius/tests/reference_repo.rs`, the shared `World`): AC1-AC3 through the MCP
  endpoint; AC4 with `GitAdapter::with_github_base` against a local bare remote (replaces
  `the_git_adapter_bundles_one_branch`). Each fails on `dev` before the fix (the stub and the adapter
  bundle one branch; `branch` is ignored).
- Existing SPEC-267 tests stay green (refusals, sweep, full attachments folder).
- Not testable here: a real GitHub fetch with a real user token, and a real agent's containment
  cloning the bundle (the test clones on the host).

## Decisions

- [ADR-0223](../../docs/decisions/0223-a-reference-copy-carries-every-branch-and-tag-its-owner-can-read.md):
  the bundle carries every branch and tag, the tool picks the branch to check out; the agent still
  never holds the person's token (ADR-0114) and the same-owner boundary of ADR-0197 stays.

## Evidence

- Regression first (commit 3d1c58b2, before the fix): `cargo nextest run -p yulius -E
  'binary_id(yulius::main) & test(/^reference_repo::/)'`: 7 run, 5 passed, 2 failed:
  `a_task_gets_every_branch_and_tag_its_owner_can_read` (asked `dev`, answered `branch: main`) and
  `a_branch_the_repository_lacks_is_invalid_and_names_the_branches` (`nope` answered `main`).
- After the fix: the same filter, 9 tests run, 9 passed (with
  `the_git_adapter_bundles_every_branch_and_tag` against a local bare remote with `dev`, `other`
  and a tag).
- `scripts/dev ci` at efd6561f: exit 0; nextest 2630 tests passed, 16 skipped; lint, docs, deps,
  macOS compile green.
- `yulius security-scan` at efd6561f: exit 2, **not run** (Vidoc HTTP 429: the instance key used
  its token quota); 8 code files listed unscanned. Not a pass; Yulius CI's `security` job runs it
  again on the pull request head.
- Not tested here: a real GitHub fetch with a person's token and a real agent's containment cloning
  the bundle (the tests clone on the host).
