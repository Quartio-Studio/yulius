---
yulius-task: f140a17c-e2b6-43e0-8e64-c6c4c1c99ebb
status: in-progress
---

# SPEC-391: Vidoc triage of the CI jobs and caches findings (#842 #739 #736 #829 #843)

Status: in-progress
Date: 2026-11-02
Owner: Yulius task #373
Scope authorization: task #373 (a person asked that each of the five issues end closed). Fixes
#829; #842, #739 and #843 are dismissed as false positives and #736 as a duplicate of #739.

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
- VCT-003 (#842, false positive, by design). A pull request's own jobs run only for heads of the
  sandbox's own repository, inside its containment, with no secret its own configuration grants;
  dismissed with a narrow entry.
- VCT-004 (#843, false positive). Declared caches cannot escape the worktree; dismissed with a
  narrow entry.

## Plan

- [x] Spec SPEC-391 with the triage of all five findings
- [ ] ci_setup reads the worktree's files through no-follow handles from the sandbox directory (#829)
- [ ] Narrow [[ignore]] entries for the dismissed wordings (#842 #739 #736 #843)
- [ ] Gates, docs and changes entry
- [ ] Independent review answered
- [ ] Pull request merged into dev on green and issues closed

## Acceptance criteria

- A `ci/tools.rs` unit test where `.github` is a link to a host folder holding a workflow fails on
  `dev` (the host workflow is summarized) and passes; the same for a worktree folder that is a
  link, and a `yulius.toml` or workflow that is a link or a FIFO yields nothing and returns.
- `.config/vidoc.toml` has one narrow entry per dismissed wording, each with its reason.
- Each issue ends closed: #829 by the merged pull request, the others as not planned with a
  comment giving the evidence.

## Test plan

- Unit tests in `crates/yulius-app/src/ci/tools.rs` on the blocking reader (tempdir sandbox).
- `scripts/dev ci` (the full gate) and `scripts/dev security`.

## Decisions

- #829 is fixed rather than dismissed: the worktree belongs to a contained agent and the server
  reads it outside containment, which is exactly the ADR-0174 boundary; the existing handle
  helpers make the fix small. The leaf-only `O_NOFOLLOW` the finding suggests is not enough: a
  folder on the way (`.github`) swapped for a link would still be followed, so the whole path is
  walked by handles.
- #842 is by design: CI exists to run the pull request's code. Who can reach it already runs code
  in that sandbox (push access to its repository, or its agents), the job runs in the same
  containment as those agents, and secrets come only from the integration branch's entry for that
  name and command. Whitelisting commands would make CI useless.
- #736 is a duplicate of #739 (same `checkout_name` path, same fix asked).
- No new ADR: the fix applies ADR-0174 to one more reader.
