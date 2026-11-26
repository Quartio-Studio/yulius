---
yulius-task: 7341e60c-41be-421d-b9c0-a62785782ad0
status: in-progress
---

# SPEC-671: Cleanup of the wiring, the main.rs arguments and the self-update overlap (audit L6)

Status: in-progress
Date: 2026-11-26
Owner: Yulius task #655
Scope authorization: the repository owner's cleanup chore of 2026-11-25 (audit L6, findings F4,
F5 and F7, under the cleanup charter). The charter asks for no spec; this one exists only because
Yulius's delivery needs one, and records nothing beyond the task.

## Goal

The same behaviour in less code: the findings F4 (wiring), F5 (subcommand arguments declared once)
and F7 (the self-update overlap, only where update and rollback stay identical) of the L6 audit,
one pull request each.

## Context

The audit report L6 "composition and operations" of 2026-11-25 names the copies: a dead
`wiring/github.rs`, test-only adapters in `wiring/pending.rs`, a hand-rolled base64, seven copies
of the reqwest client setup and eight bounded body reads, two npm tarball installers, four
wall-clock readers, and `main.rs` rebuilding every option struct field by field.

## Requirements

- R1: Behaviour stays the same; every existing test passes unchanged.
- R2: A new helper only where it replaces at least two copies.
- R3: F7 changes nothing observable in an update or a rollback; otherwise it is reported, not done.

## Plan

- [x] F4: delete dead github.rs and move pending.rs to tests
- [x] F4: base64 crate and Timestamp::now()
- [x] F4: one HTTP client builder and one bounded reader
- [x] F4: one shared npm tarball module
- [x] F4: independent review answered
- [x] F5: clap Args on the option structs in main.rs
- [ ] F5: independent review answered
- [ ] F7: compare the update helper and staging CD install/rollback and the two freezes
- [ ] F7: simplify where behaviour stays identical, or report
- [ ] F7: independent review answered

## Acceptance criteria

- The `yulius` test binary and the touched crates' unit tests pass with no assertion changed.
- `git diff --shortstat origin/dev` shows fewer product lines for each pull request.

## Test plan

Run the whole `yulius` test binary and the touched crates' unit tests; clippy and `cargo fmt
--check`; Yulius CI on each head.

## Decisions

- The wall-clock reader is `wiring::clock::now()`, not `Timestamp::now()` in yulius-domain: RUST-005
  keeps clocks out of the domain crate.
