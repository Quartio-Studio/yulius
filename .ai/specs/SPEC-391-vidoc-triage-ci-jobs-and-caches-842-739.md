---
yulius-task: f140a17c-e2b6-43e0-8e64-c6c4c1c99ebb
status: done
---

# SPEC-391: Vidoc triage of the CI jobs and caches findings (#842 #739 #736 #829 #843)

Status: done
Date: 2026-11-02
Owner: Yulius task #373
Scope authorization: task #373 (a person asked that each of the five issues end closed). Fixes
#829 and #842 (its real part: fork pull requests reached CI, found by the independent review);
#739 and #843 are dismissed as false positives and #736 as a duplicate of #739.

## Goal

Each of the five Vidoc findings about Yulius CI and its host-side paths is checked against `dev`
(4e07ce4e) and either fixed test-first or dismissed with file:line evidence and a narrow
`[[ignore]]` entry for its wording, so a repeat scan does not reopen it.

## Context

- `ci_setup` (`crates/yulius-app/src/ci/tools.rs`) is an agent tool: it reads the calling task's
  worktree (`yulius.toml`, `.github/workflows/*.yml`) on the host, outside containment, and returns
  their summary to the agent. The worktree lies in the sandbox directory, where a contained shell
  (root in its sandbox) can swap any folder or file for a link at any moment.
- ADR-0174 / SPEC-220: such reads go through handles opened from the sandbox directory one folder
  at a time with `O_NOFOLLOW` (`files::open_dirs_below`, `files::open_dir_at`), never by path.
- CI job keys: `expand_jobs` accepts only `[a-z0-9][a-z0-9_-]*` of at most 32 bytes
  (`yulius-domain/src/ci/mod.rs` `valid_job_name`), the profile gate and the built-in scan use the
  constants `gate` and `security`.
- Declared caches: `parse_yulius_toml_caches` passes each path through `check_repo_path`, which
  refuses absolute paths and empty, `.` or `..` segments (`spec_format/config.rs`).
- CI of a pull request: only heads in the sandbox's clone are tested (fork heads are not fetched),
  jobs run in the sandbox's containment without secrets unless the integration branch grants them
  by name and command (SPEC-142, SPEC-264, ADR-0116).
- `[[ignore]]` matches the path exactly and the title as a case-insensitive substring; one narrow
  entry per wording (SPEC-262, ADR-0202).

## Requirements

- VCT-001 (#829, real). `ci_setup` reads `yulius.toml` and the workflow files of the task's
  worktree only through handles: the worktree folder is opened from the canonical sandbox
  directory folder by folder with `O_NOFOLLOW`, `.github` and `workflows` the same way, and each
  file with `O_NOFOLLOW | O_NONBLOCK` relative to its folder's handle, checked as a regular file
  of at most the limit on that handle and read to at most limit + 1 bytes. A link anywhere on the
  way (the worktree, `.github`, `workflows`, the file), placed before or after the check, yields
  no content from outside; a FIFO does not block the server.
- VCT-002 (#739, #736, false positive). The checkout path's key cannot hold `/`, `.` or be empty;
  dismissed with a narrow entry per wording.
- VCT-003 (#842, real in part). CI never tests a pull request GitHub reports as coming from a fork
  (`from_fork`), whether or not its head commit is in the sandbox's clone, which a contained agent
  can fill (`gh pr checkout`, a fetch of `pull/N/head`, a fetch refspec in the clone's config).
  Running a same-repository pull request's own jobs stays by design (its author pushes to the
  repository; the jobs run in the sandbox's containment, with no secret its own configuration
  grants); that wording gets a narrow entry.
- VCT-004 (#843, false positive). Declared caches cannot escape the worktree; dismissed with a
  narrow entry.

## Plan

- [x] Spec SPEC-391 with the triage of all five findings
- [x] ci_setup reads the worktree's files through no-follow handles from the sandbox directory (#829)
- [x] Narrow [[ignore]] entries for the dismissed wordings (#842 #739 #736 #843)
- [x] CI never tests a fork's pull request whose head is in the clone (#842)
- [x] Gates, docs and changes entry
- [x] Independent review answered
- [ ] Pull request merged into dev on green and issues closed

## Acceptance criteria

- A `ci/tools.rs` unit test where `.github` is a link to a host folder holding a workflow fails on
  `dev` (the host workflow is summarized) and passes; the same for a worktree folder that is a
  link, and a `yulius.toml` or workflow that is a link or a FIFO yields nothing and returns.
- `local_ci::a_fork_pull_request_is_never_tested_even_with_its_head_in_the_clone` fails on `dev`
  (`pr/1` queued for a fork whose head is in the clone) and passes.
- An MCP test (`ci_setup::`) shows `ci_setup` summarizes the worktree's own files and, once
  `.github` is a link to a host folder, nothing of it; it fails on `dev`.
- `.config/vidoc.toml` has one narrow entry per dismissed wording, each with its reason.
- Each issue ends closed: #829 and #842 by the merged pull request, the others as not planned
  with a comment giving the evidence.

## Test plan

- Unit tests in `crates/yulius-app/src/ci/tools.rs` on the blocking reader (tempdir sandbox).
- Integration tests in `crates/yulius/tests/ci_setup.rs` (the tool through MCP) and
  `local_ci.rs` (a fork pull request whose head is in the clone).
- `scripts/dev ci` (the full gate) and `scripts/dev security`.

## Decisions

- #829 is fixed rather than dismissed: the worktree belongs to a contained agent and the server
  reads it outside containment, which is exactly the ADR-0174 boundary; the existing handle
  helpers make the fix small. The leaf-only `O_NOFOLLOW` the finding suggests is not enough: a
  folder on the way (`.github`) swapped for a link would still be followed, so the whole path is
  walked by handles.
- #842 is fixed where it was real and by design elsewhere. The independent review found that the
  "fork heads are skipped" mitigation relied on the head missing from the clone, which an agent
  controls; filtering on GitHub's `from_fork` (already on `GhPull`) is the boundary ADR-0116
  states. Otherwise CI exists to run the pull request's code. Who can reach it already runs code
  in that sandbox (push access to its repository, or its agents), the job runs in the same
  containment as those agents, and secrets come only from the integration branch's entry for that
  name and command. Whitelisting commands would make CI useless.
- #736 is a duplicate of #739 (same `checkout_name` path, same fix asked).
- No new ADR: the fix applies ADR-0174 to one more reader.

## Evidence

- Triage on `dev` 4e07ce4e:
  - #739 / #736: every `CiJobRecord.key` comes from a `dom::Job` (`ci/mod.rs` `ci_queue` ~667,
    tree reuse ~952, `merge_train.rs` ~889 copies stored ones); `expand_jobs` accepts only
    `valid_job_name` keys (`yulius-domain/src/ci/mod.rs:182`, `:243`), else the constants `gate`
    and `security`; `checkout_name` (`ci/exec.rs:285`) only appends `.N` and `@N`. Pinned by
    `ci::tests::jobs_expand_matrices_and_reject_bad_definitions` (`../x`, `..`, `a/b`, `/tmp`).
  - #843: `parse_yulius_toml_caches` (`spec_format/config.rs:1170`) runs `check_repo_path`
    (`:636`, absolute, empty, `.` and `..` refused); `find_caches` and `remove_cache` then need
    `real_dir_from` and remove through handles. Pinned by
    `spec_format::config::tests::declared_caches_are_repository_relative_paths`.
  - #842: by design for same-repository pull requests (containment, no secret their own
    configuration grants, `exec.rs` `trusted_job`/`granted` ~211-230); real for forks, fixed.
  - #829: real, fixed.
- Regression tests, each seen failing before its fix:
  `ci::tools::tests::ci_setup_follows_no_folder_swapped_for_a_link` and
  `ci_setup_follows_no_worktree_swapped_for_a_link` (the host workflow and `yulius.toml` were
  summarized), `ci_setup::ci_setup_reads_the_worktree_and_follows_no_link_out_of_it` (with dev's
  `tools.rs`: fails at the leak assertion), `local_ci::a_fork_pull_request_is_never_tested_even_with_its_head_in_the_clone`
  (`pr/1` was queued). Guards passing on both: `ci_setup_reads_the_worktrees_own_files`,
  `ci_setup_reads_no_linked_file_and_waits_on_no_fifo`.
- `scripts/dev ci` at 19af9540: exit 0, nextest 2960 passed, 16 skipped (clippy, fmt, doctests,
  macOS compile, deps and docs included).
- `local_ci::removing_the_sandbox_cancels_its_ci_runs` failed once in a targeted `local_ci::`
  run under load (load average 15; its removal must end runs within 5 s), passed 3/3 alone, in a
  full-file rerun (58/58) and in the gate. Pre-existing and unrelated (no fork there); noted in
  the pull request, no issue filed (the task forbids creating issues).
- `yulius security-scan`: "Security scan disabled: Vidoc is turned off in Yulius" (exit 0):
  disabled, not passed; nothing was scanned.
- `[[ignore]]` entries (one per wording, the independent reviewer agreed to each): `ci/mod.rs`
  "CI runs attacker-supplied job commands from pull requests"; `ci/exec.rs` "Path traversal via CI
  job key in checkout directory" and "Path traversal via unvalidated CI job key in checkout path";
  `disk.rs` "Path traversal via user-controlled declared caches in yulius.toml".
- Independent review (fresh session): round 1 requested changes (fork pull requests reached CI,
  so the #842 reason was untrue; minors on errors, the wiring test, canonicalize on the async
  thread), all answered; round 2 approved 19af9540. Left as is: `ci_rerun` does not re-check
  `from_fork`, so a fork's run queued before this change could still be rerun by a person or agent
  of the sandbox (noted in the pull request).
