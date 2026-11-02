---
yulius-task: 5c1c3de0-e489-454c-af19-e793384e4f3f
status: done
---

# SPEC-383: A test asserts "your home does not exist on this machine" (#643)

Status: done
Date: 2026-11-02
Owner: Yulius task #365
Scope authorization: the open-issues automation (task #365): solve GitHub issue #643 in one pull
request into `dev` saying `Fixes #643`. Create no new GitHub issue; comment on #643 instead.

## Goal

An integration test covers `secret_create_from_file` naming `~/key` in a sandbox whose `home` was
removed: the call is refused with "your home does not exist on this machine" and no agent secret
is created. Test only; the behavior is unchanged.

## Context

- `open_folder` (`crates/yulius-app/src/vault/env_files.rs`, SPEC-211, task #198) walks from the
  anchor down the root's folders without links. Opening the root itself (no visible folder) maps
  `ENOENT` to `AppError::Invalid("{within} does not exist on this machine")` and `ELOOP`/`ENOTDIR`
  to "{within} is a link or not a folder: Yulius follows no links".
- A task's `~/…` anchors on the canonical sandbox folder with the folders `["home"]`
  (`secret_source_root` in `crates/yulius-app/src/vault/create.rs`); `within` is "your home". A
  missing home therefore reaches the `ENOENT` root branch of `open_folder`, not the
  `canonicalize` refusal of `secret_source_root` (that one is for a missing sandbox folder).
- Only the link branch is asserted
  (`secrets_create::a_root_swapped_for_a_link_and_reserved_names_are_refused`); the final review
  of #198 left the missing-root branch untested at the round limit (#643).

## Requirements

- **RDE-001** A test in `crates/yulius/tests/secrets_create.rs` removes the sandbox home of task
  t1 and calls `secret_create_from_file {"name": …, "file": "~/key"}` with the task's token.
- **RDE-002** It asserts the call is an error whose text contains "your home does not exist on
  this machine", and that the sandbox's agent secrets are unchanged.

## Plan

- [x] Spec the missing-home test
- [x] Missing-home refusal asserted by an integration test
- [x] Mutations of the missing-root branch fail the test
- [x] Independent review approved and findings answered
- [x] Gates green with evidence in the spec

## Acceptance criteria

- The new test passes on dev's code.
- It fails when the `ENOENT` root branch's message is changed, and when that branch stops
  refusing (shown in Evidence).
- `scripts/dev ci` is green.

## Test plan

- `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^secrets_create::/)'`.
- Three temporary mutations of `open_folder`, each committed nowhere: the message reworded, the
  `ENOENT` root arm removed (the generic `ENOENT` arm answers), and a missing root created before
  the walk instead of refused.

## Decisions

- Remove the home outright (`remove_dir_all`) rather than moving it aside: the issue's case is a
  home that is gone, and nothing in the test needs it afterwards.
- No ADR: a test-only change takes no lasting decision.

## Evidence

- `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^secrets_create::/)'` on
  dev's code (a0d156d0, no production change): 6 passed, including
  `a_missing_home_is_refused_as_not_existing`.
- Mutations of `crates/yulius-app/src/vault/env_files.rs` (reverted with `git checkout`), each
  run with `test(=secrets_create::a_missing_home_is_refused_as_not_existing)`, exit 100, failing at
  the message assertion:
  - M1, the root `ENOENT` message reworded to "{within} is missing on this machine".
  - M2, the `Errno::ENOENT if visible.is_empty()` arm removed: the generic arm answers "the folder
    ~/ does not exist in your home".
  - M3, `std::fs::create_dir_all(&root)` before the walk (no refusal of the missing root): the
    call answers `{"text":"~/key does not exist","isError":true}`.
  Only `env_files.rs` changed in each, so the test reaches `open_folder`'s root branch, not the
  identical message of `secret_source_root`'s `canonicalize` refusal (the sandbox folder still
  exists; the test asserts it).
- `scripts/dev ci` at 504916a1: exit 0; nextest 2952 passed, 16 skipped; doctests, `cargo deny`
  (advisories, bans, licenses, sources ok), docs and macOS clippy passed.
- After the review's nit (the sandbox-folder assertion): the file's 6 tests pass, `scripts/dev
  lint` exit 0.
- `yulius security-scan`: "Security scan disabled: Vidoc is turned off in Yulius" — disabled, not
  passed.
- Independent review (fresh session, `om-code-review`) of 504916a1: APPROVE, no blocker or major.
  Answered: minor (Evidence missing, Plan unchecked, status) — this section, Plan and status
  updated; nit (Test plan said "Two … plus one") — reworded to three; nit (state in the test that
  the sandbox folder remains) — assertion added.
