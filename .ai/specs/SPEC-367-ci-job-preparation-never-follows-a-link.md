---
yulius-task: 2d9500da-bae1-4253-9576-63ee2c3a5066
status: in-progress
---

# SPEC-367: CI job preparation never follows a link at ci, ci/jobs or ci/home

Status: in progress
Date: 2026-11-01
Owner: Yulius task #350
Scope authorization: task #350, "Solve GitHub issue #969 in one pull request" (Fixes #969).

## Goal

Before a CI job runs, Yulius makes the job's `HOME` (`ci/home`) and its checkout
(`ci/jobs/<checkout>`) without ever following a link an agent put at `ci`, `ci/jobs`, `ci/home`
or the checkout's name. Nothing outside the sandbox is created, removed, checked out or cleaned.

## Context

- Symptom (#969, verified by task #348 on dev 50f1faf5): with `<sandbox>/ci` a link to a host
  folder, the next job creates `<target>/home` before it fails; a link at `ci/home` becomes the
  job's `HOME`; a link at `ci` or `ci/jobs` makes `git worktree add` create the checkout at the
  target, and every later run removes `<target>/<checkout>` through the link (`remove_dir_all`)
  and adds it again, because Git records the resolved path and `w.path == path` never matches. A
  checkout registered while `ci/jobs` was a folder, with `ci/jobs` later swapped for a link to a
  folder holding `<checkout>/.git`, gets `git checkout --force` and `git clean` at the target.
- Root cause: `ci_job_process` (`crates/yulius-app/src/ci/exec.rs`) made `ci/home` with
  `std::fs::create_dir_all` by path, and `Git::ci_checkout` (`crates/yulius-git/src/ci.rs`)
  removed a broken checkout, made its parent and added it by path; its only guard is a lexical
  prefix check against `owned_root`, itself `<sandbox>/ci`, which the agent can replace. ADR-0174
  (host access to agent-owned folders goes through handles) was applied to `ci/logs` (SPEC-285)
  and `ci/cache` (SPEC-351), not to these.
- Why tests missed it: `local_ci.rs` links only `ci/logs` and `ci/cache`; no test linked `ci`,
  `ci/home` or `ci/jobs`.
- What else the cause breaks: the merge train's scratch checkouts (`ci/train`, `ci/candidate`)
  go through the same `ci_checkout`, so a link at `ci` steered them the same way.

## Requirements

| ID | Requirement |
| --- | --- |
| CJP-001 | `ci` is opened from the sandbox directory with `O_DIRECTORY \| O_NOFOLLOW` (made when missing) before anything else of the job is made; a link or file there fails the job and nothing is made at its target. |
| CJP-002 | `ci/home` and `ci/jobs` are opened through that handle; a link or a file at either is removed and a folder made in its place, never followed, as `ci/logs` and `ci/cache` already are. |
| CJP-003 | At the checkout's name in `ci/jobs`, a link or file is removed; a folder that is not a checkout Git registers at its real path (the handle's, `/proc/self/fd` or `F_GETPATH`) with a regular `.git` file is removed through handles (`files::remove_tree`). Nothing is removed by path. |
| CJP-004 | `Git::ci_checkout` no longer removes or makes folders: it matches the registration against the checkout's resolved path, checks out and cleans that resolved path, and refuses (an error, nothing changed) when something that is not a registered checkout is in the way. |
| CJP-005 | The merge train's scratch checkouts are readied the same way before the train is built. |

## Plan

- [ ] Spec SPEC-367 written and validated
- [ ] Regression tests show links at ci, ci/home and ci/jobs followed today
- [ ] The job's HOME and checkout place are made through handles from the sandbox directory
- [ ] The CI checkout in yulius-git removes and makes nothing by path
- [ ] Merge-train scratch checkouts are readied through handles too
- [ ] Docs, changes and evidence updated, gates green and review answered
- [ ] Pull request merged into dev on green CI

## Acceptance criteria

- A link at `ci` makes the job fail without creating anything at its target (test, fails before
  the fix).
- A link at `ci/home` or `ci/jobs` is replaced by a folder, never followed (tests).
- `ci_checkout`'s removal of a broken checkout never follows a link on the way (test: a broken
  checkout and a link at `ci/jobs` to a folder holding the checkout's name leave that folder
  intact).
- The existing CI, merge-train and disk upkeep tests still pass.

## Test plan

- `crates/yulius/tests/local_ci.rs`, modelled on `a_linked_ci_cache_folder_never_reaches_outside`:
  a link at `ci` (job not passed, the target stays empty); a link at `ci/home` (replaced, the job's
  `$HOME` is in the sandbox, the target untouched); a link at `ci/jobs` to a folder holding
  `<checkout>/keep` (replaced, the jobs pass, `keep` stays and Git registers nothing outside).
- `crates/yulius-git/tests/git_ops.rs`: `ci_checkout` refuses a folder that is not a registered
  checkout and leaves it as it is.
- The full gate (`scripts/dev ci`).

## Decisions

- **The app readies the place, Git only checks out.** The handle helpers (`open_dirs_below`,
  `kind_at`, `remove_tree`, `real_path`) live in `yulius-app` (ADR-0174), so the removal of a
  broken checkout moves there instead of a second handle walker in `yulius-git`. `ci_checkout`
  keeps its registration check and refuses what is in the way rather than removing it, so a caller
  that skipped the preparation fails safe.
- **Link at `ci` fails, links below it are replaced.** The same rule as `create_job_cache`: the
  sandbox directory is the anchor, `ci` is the first folder opened from it; replacing it would
  mean removing a link in the sandbox's root each run, while a link below `ci` is replaced so a
  stray link never blocks CI.
- **Registration by resolved path.** Git records the real path of a worktree, so the registration
  is compared with the resolved checkout path (the handle's real path in the app,
  `canonicalize` in `yulius-git`); a registration made through an old link, outside, never matches
  a checkout inside again, and a data directory below a link no longer re-adds checkouts each run.
- **Registrations outside the CI directory never count** (review round 1). The registrations
  (`repo/.git/worktrees/*/gitdir`) are in the sandbox, so an agent can forge one for any host
  folder holding a regular `.git` file, and checkouts added through a link before this fix are
  registered outside. `ci_checkout` treats a registration as its checkout only when the resolved
  path is inside `owned_root` with the root's parent resolved and its own name not followed (like
  `worktree_remove_if_safe`, which resolves the whole root; here a link at `ci` must not count);
  both sides are compared resolved (`canonicalize`), so
  a registration spelled through a link to the real checkout still matches.
- **Two lanes replacing one link** (review round 1): the second finds it gone (`ENOENT`) or the
  other's folder made (`EISDIR`; `EPERM` on macOS only, review round 2) and opens that folder
  instead of failing. Preparation errors are conflicts with the OS's reason, and the merge
  checkout's name its folder (review round 2).
- **Bounded removal.** A broken checkout is removed with `remove_tree`, which stops at
  `MAX_REMOVED_ENTRIES` (1,000,000) entries: a broken checkout with a larger cache fails the job
  with that reason, and each later run removes the next million until it is gone. The old
  `remove_dir_all` was unbounded and followed links on the way.
- **Remaining window, recorded as SPEC-263 did.** Git takes paths, so between the preparation
  through handles and `git worktree add`, `checkout` or `clean` (and, for the merge train, the
  merge and commit commands `build_train` runs in its scratch checkout after each `ci_checkout`),
  an agent that swaps `ci/jobs` or `ci` for a link again at exactly that moment can still make Git
  act at the link's target. Closing it would need Git to run on a directory handle (not possible
  on macOS, ADR-0169) or the checkout to run inside the sandbox's containment, a change of its
  own. The window is now the Git commands of one preparation instead of every run, a removal
  never goes through a link, and Git never acts on a registration outside `ci`.
- **Out of scope, reported on #969** (review round 1, pre-existing): a registered checkout's
  `.git` file names its admin folder (`gitdir: …`), which an agent can point at another
  repository's; `checkout --force` would then rewrite that repository's `HEAD` and index. Hooks and
  `fsmonitor` are already off for Yulius's Git (`runner.rs`).
- **No new ADR**: this applies ADR-0174 to two more folders; the spec records the specifics.
