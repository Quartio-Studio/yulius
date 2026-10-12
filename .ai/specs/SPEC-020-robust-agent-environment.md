---
yulius-task: none
status: done
---

# SPEC-020: agents meet no avoidable obstacles

Status: done
Date: 2026-10-12
Owner: finished by hand after the analysis of the `yulius` sandbox's task transcripts
Scope authorization: user request (F 2026-10-12): "(4)(a) it meets technical problems that should
have been fixed on onboarding, like rust things etc. (b) Things failing for no reason, things
should be robust ... Agent can modify CI behaviour, for example for ../studio we should run only
tests of specific project ... we want agents to meet zero obstacles"; "agents doesnt seem to work
very in parallel".

## Goal

What slowed or stopped agents without being their fault no longer does.

## Context

From the 2026-10-11/12 transcripts:
- Every task worktree compiled the repository's pinned tools from source (10–40 minutes).
- CI ran one run at a time per sandbox, and each merge into `dev` cost another full run.
- A failed `quality` job left `e2e` holding the CI slot for up to 30 minutes.
- A new head of a pull request lost its place in the queue.
- Infrastructure errors reached agents as "fix the cause test-first".
- A pull request could not test its own CI change.
- Proactive subscription switches killed background builds.
- `rust-lld` running out of the sandbox's memory killed the `claude` process.
- Every parallel task took the same spec number.
- `docs/changes.md` conflicted on every merge.
- The onboarding task ended at "Your turn", with its environment unverified.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| ENV-001 | `scripts/bootstrap.py` builds each pinned tool once per home (`~/.cache/yulius-tools/<name>-<version>`, locked against parallel builds) and links it into the worktree's `.tools/bin`. | run on macOS: linked from the cache, one missing tool built once |
| ENV-002 | `docs/changes.md` merges with Git's union driver. | `.gitattributes` |
| ENV-003 | Agent commands raise their own `oom_score_adj` to 500, so memory pressure kills a build before the agent; agent Rust builds use a third of the CPUs (`CARGO_BUILD_JOBS`). | `main.rs`, `supervisor.rs` |
| CI-101 | A branch commit whose tree a passed run tested is recorded as passed ("not run again") and gets `yulius/ci`. | `local_ci.rs::a_merge_with_a_tested_tree_reuses_its_result` |
| CI-102 | A failed job stops the run's other jobs. | `exec.rs` |
| CI-103 | A pull request's new head keeps the queue place of the run it supersedes. | `ci/mod.rs` |
| CI-104 | A run that errored is reported as "could not run" (not the code) and queued again once. | `delivery.rs` |
| CI-105 | A pull request that changes `[[ci.jobs]]` is tested with its own jobs (never with none). | `ci/mod.rs` |
| ROT-020 | A proactive switch waits while the agent's background shells run. | `hooks.rs` |
| SPC-001 | A task without a spec path gets the next free `SPEC-NNN-<slug>.md` at its first start, in repositories that number specs that way. | `delivery.rs` |
| ONB-001 | The onboarding task runs in goal mode, and its prompt asks to check the base image, install tools once in the home, install test dependencies and run every `[[ci.jobs]]` command until it passes. It describes Yulius CI as `ci/exec.rs` runs it: the jobs' `HOME` and `$YULIUS_CI_CACHE` are their own, separate from the agent's, start empty and persist across runs, each job's checkout keeps its ignored files, and the job's `PATH` lacks that home's tool directories; so a job installs what it needs itself and never depends on what an earlier run left behind, which the agent proves by running each job command normally and, committed, from the root of a clean checkout (`git worktree add --detach`) with a fresh home and only CI's variables (`env -i HOME="$(mktemp -d)" YULIUS_CI_CACHE="$(mktemp -d)" ...`) (task #34). | `onboarding.rs::prompts_describe_the_persistent_ci_home_and_the_fresh_home_proof` |

## Plan

- [x] Shared tools, union merge for the changes log, memory protection
- [x] CI: tree reuse, fail-fast, queue place, errored runs, own CI jobs
- [x] Switches wait for background shells; spec numbers allocated; onboarding goal mode and prompt

## Evidence

- `scripts/dev ci` on macOS: see the pull request.
- `scripts/bootstrap.py` on macOS: two tools linked from the cache, one built once into it.
