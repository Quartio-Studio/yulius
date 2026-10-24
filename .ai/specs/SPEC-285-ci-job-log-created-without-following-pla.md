---
yulius-task: 35ad07bd-9076-4ebe-ac02-a28b81f109d4
status: in-progress
---

# SPEC-285: a CI job's log is created, read and removed without following a planted link

Status: in-progress
Date: 2026-10-24
Owner: Yulius task #271
Scope authorization: issue [#663](https://github.com/Quartio-Studio/yulius/issues/663) (found in the
independent review of task #205, SPEC-219): "The log is created from an anchor the agent cannot
replace: the folder opened with `O_NOFOLLOW|O_DIRECTORY`, the file with `create_new` and
`O_NOFOLLOW`." Acceptance: a link planted at the job log's path, and a linked `ci/logs` folder,
leave the link's target untouched and the job's output is still recorded; no host write in
`<sandbox>/ci/logs` follows a link.

## Goal

An agent of a sandbox cannot make the service user create, truncate or delete a host file through
the sandbox's `ci/logs` folder, and its CI jobs' output is still recorded.

## Context

- Symptom: `ci_job_process` (`crates/yulius-app/src/ci/exec.rs`) prepares `<sandbox>/ci/logs` with
  `std::fs::create_dir_all`, then runs `std::fs::remove_file(&log)` and
  `std::fs::File::create(&log)` (`O_CREAT|O_TRUNC`, follows links), and after the job
  `std::fs::remove_file(&log)` again, all by path. Contained agents can write the whole sandbox
  directory (ADR-0174), so:
  - a `ci/logs` folder replaced by a link to a host folder makes the server create the job's log
    in that host folder (and delete it there after the job);
  - a link planted at the log's path between the removal and the create makes the server truncate
    the link's target;
  - `ci/logs` relinked while the job runs makes the final removal delete `<target>/<job>.log`.
- Root cause: this code predates ADR-0174 and was not in #657's list. Task #205 hardened only the
  read side (`LogTail::open`: `O_NOFOLLOW`, regular files only, read through a handle opened before
  the job).
- Why the tests missed it: the CI tests never plant anything in `ci/`; the #656 test relinks a CD
  environment run's log, which lives outside the sandbox.
- CD and environment runs keep their logs in `<data>/runs/…`, out of the agent's reach; they are
  unchanged.

## Requirements

- LOG-001: `ci/logs` is opened from the sandbox directory one component at a time with
  `O_DIRECTORY|O_NOFOLLOW` (`files::open_dir_at`); a link or a file the agent put at `ci/logs` is
  removed with `unlinkat` (never followed) and a fresh folder made in its place.
- LOG-002: the old log is removed with `unlinkat` in that folder handle and the new one created
  with `openat(O_RDWR|O_CREAT|O_EXCL|O_NOFOLLOW)`; an entry planted between the two is removed and
  the create tried again, a bounded number of times, then the job ends in error.
- LOG-003: the server reads the job's output only through the handle it created (no reopen by
  path), and removes the log after the job with `unlinkat` in the same folder handle.
- LOG-004: the job itself still writes the log by its path, inside its containment: whatever the
  agent swaps in there can only lose that job's own output, never reach a host file.

## Plan

- [x] Spec SPEC-285 with root cause and plan
- [x] Regression tests: planted links in ci/logs leave host files untouched
- [x] Job log created, read and removed through anchored handles
- [x] Docs: changes entry, ADR-0174 note, spec evidence
- [ ] Independent review answered
- [ ] Gates and security scan green; delivered on green CI

## Acceptance criteria

- AC-1: with `ci/logs` replaced by a link to a host folder before the run, nothing is created in
  that folder during the job, and the job passes with its output recorded.
- AC-2: with a link to a host file planted at the job log's path exactly between the removal of
  the old log and the create (pause point `race::Point::CiLogCreate`), the host file keeps its
  content and the job passes with its output recorded.
- AC-3: a job that relinks `ci/logs` to a host folder holding a `<job>.log` while it runs leaves
  that file in place, and its output is recorded.

## Test plan

- `local_ci::a_linked_ci_logs_folder_gets_no_host_write` (AC-1), red on `dev`: the job lists the
  host folder while it runs and sees its own log there.
- `local_ci::a_link_planted_at_the_job_log_is_never_followed` (AC-2), red on `dev`: the host file
  is truncated.
- `local_ci::a_job_relinking_ci_logs_keeps_the_host_file` (AC-3), red on `dev`: the final removal
  deletes the host file.
- The rest of `local_ci` (job output, stages, failures, output limit) stays green: the log is still
  read through one handle.

## Decisions

- Anchored handles in the sandbox, not a log moved to `<data>/runs/…` (the issue's alternative):
  a contained job writes its log by path, and a log outside the sandbox would need a new writable
  bind into every CI job's containment. The anchored create closes the host write with no change
  to containment. This applies ADR-0174 and needs no new ADR; ADR-0174's consequences name it.
- A link at `ci/logs` is replaced, not refused: the folder is Yulius's, and refusing would let an
  agent's planted link fail every CI job of the sandbox.
- A planted `ci` link still fails the job (`ELOOP`): the checkout already ran there by path, out
  of this issue's scope (see Evidence).

## Evidence

- Red on `dev` (2025e0f0 with the test-only pause point, commit 4aa95f55), and again with the
  review-round-1 tests against the unfixed `ci/` code:
  `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^local_ci::a_(linked_ci_logs|link_planted|job_relinking)/)'`
  → 3 failed: the linked folder held `<job>.log` files while the jobs ran (AC-1); the planted
  link's target was truncated to `""` (AC-2); the host `<job>.log` was deleted through the
  relinked folder (AC-3, "No such file or directory").
- Green with the fix: the same command, 3 passed, 5 runs in a row; `local_ci::` 46 passed;
  `delivery::` and `cd` 72 passed (CD and environment runs keep `RunLog::At`).
- Gate: `scripts/dev ci` exit 0 (2575 tests passed, 17 skipped) on 56552d06; rerun on the final
  head below.
- Security: `scripts/dev security` exit 2, **unrun**: Vidoc answered HTTP 429 ("This API key has
  used its 50,000,000 tokens"); the 5 code files were not scanned. Not a pass. Vidoc's own issue
  #793 reports the same `File::create` and closes once its line is gone from `dev`.
- Independent review round 1 (fresh session, 56552d06), request changes, answered:
  - own, major: the relinking test read `/proc` (absent on macOS, a supported host): the test now
    writes the host files at the queued jobs' log names; still red on the old code.
  - own, minor: the planted-link test did not prove its pause point fired: it counts the plants
    and asserts one per job.
  - own, minor: this Evidence section was missing.
  - own, nit: the linked-folder test passed on an empty listing: it requires `keep.txt` in it.
  - pre-existing, major: CI checkout preparation still follows a link at `ci` or `ci/jobs`
    (`ci_checkout`'s `remove_dir_all`/`create_dir_all`/`git worktree add` by path in
    `crates/yulius-git/src/ci.rs`, and `create_dir_all(ci/home|ci/cache)`), on `dev` before this
    change (2025e0f0 `exec.rs:917,925`). Outside `ci/logs` and this issue; this task may not
    create issues, so it is reported in a comment on #663 and in the task summary.
