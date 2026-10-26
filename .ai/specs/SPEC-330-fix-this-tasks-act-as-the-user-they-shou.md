---
yulius-task: 7e50bc4f-5ce9-40b0-8e45-0b2e64cd0df6
status: done
---

# SPEC-330: a task's reference copy carries every branch its owner can read

Status: done
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
  100 listed); `clone_command` and `update_command` are the shell commands, every word quoted, to
  clone it and to update an earlier copy with every branch and tag (forced, so a re-pointed tag
  updates); its message gives them and how to check out another branch of the copy.
- **REF-104** The tool's description, ADR-0223 (amending ADR-0197), SPEC-267's pointer,
  `docs/architecture.md` and `docs/product.md` say the copy carries every branch and tag.

## Plan

- [x] Spec, issue and ADR record that a reference copy carries every branch and tag
- [x] Regression tests show a task cannot get a non-default branch
- [x] reference_repo bundles every branch and tag and checks out the asked branch
- [x] Docs, gates and security scan are green
- [x] Independent review approves the head and the change is merged into dev

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
- After review round 1 (head 3e8dfb5c, with dev's #917 merged in): reference_repo filter 10 run,
  10 passed; `yulius-app` `reference_repo::` unit tests 2 passed. `scripts/dev ci`: exit 1, nextest
  2632 run, 2631 passed, 1 failed: `task_done::a_goal_the_tui_dropped_is_submitted_again` ("timed
  out waiting for Done", 65 s, under host load average ~130). Not caused by this change (it touches
  no task, goal or TUI code); alone it passed 3 of 3 (about 18 s each): filed as #921. Every other
  gate step passed. Yulius CI on the pull request head is the merge gate.
- Review round 2 (head a8a75a97: approve, no critical/high/medium). Low, own: the 100-branch
  listing cap had no test; added `an_answer_lists_at_most_a_hundred_branches_and_counts_them_all`
  (102 branches: 100 listed, `branch_count` 102, the missing-branch error says "and 2 more"); with
  `take(MAX_LISTED_BRANCHES)` mutated away it fails, restored it passes (11 of 11). Nit, own: the
  `pwned` check now also follows the update. Nits left as they are: `origin/HEAD` of an updated copy
  is not refreshed after the default branch is renamed (harmless to reading; the answer names the
  default branch); a listed branch over 255 bytes cannot be the `branch` argument (Yulius's own
  branch-name rule; the agent checks it out from the copy); a non-UTF-8 branch name is shown lossily
  (GitHub names are UTF-8).
- Delivery: merged into `dev` by `delivery_merge` only on green Yulius CI at the reviewed head.
- Not tested here: a real GitHub fetch with a person's token and a real agent's containment cloning
  the bundle (the tests clone on the host).

### Review round 1 (head 76458d82: request changes)

- Medium, own: the update command's `--tags` refuses a re-pointed tag (exit 1, silent under
  `--quiet`). Fixed test-first (f78ec0b4 failing, then the fix): forced
  `'+refs/tags/*:refs/tags/*'`; `the_answered_commands_clone_and_update_a_copy` runs the answered
  `clone_command` and `update_command` through `sh` after a branch moved, `v1` was re-pointed and
  `v2` added.
- Low, own: the message assertion tested only the word "checkout". Replaced by running the
  commands (above) and checking the message carries them.
- Low, own: `local_branches` skipped names Yulius would not create, so counts could miss bundled
  branches. Fixed test-first: `Git::every_branch`/`every_tag` list every ref; the adapter test has a
  301-byte branch name.
- Low, own (accepted): a misspelled `branch` costs one fetch; bounded by the two concurrent fetches
  and recorded in ADR-0223's consequences.
- Low, pre-existing, now also `branch`: names unquoted in the answered commands. Fixed here
  (`sh_quote`, unit test; the integration test clones a branch named `odd$(touch${IFS}pwned)'q`
  and checks nothing ran).
- Low, pre-existing: the 1 GiB cap is checked after the fetch lands on the server's disk: #920
  (present since SPEC-267; ADR-0223's consequences now say so).
- Nit: the ADR index entry for 0197 now says ADR-0223 widened it.
