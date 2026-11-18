---
yulius-task: d127ec64-2008-464e-a0f8-a622a6af5dcd
status: in-progress
---

# SPEC-509: subscription sign-ins start again (login terminal within the host's 1000 columns)

## Goal

Adding a subscription by sign-in starts its login terminal again. Today every sign-in (Claude
Code, Codex and Antigravity) fails at "Continue to sign-in" with "The agent process could not be
started: invalid launch spec field `cols`: must be within 1..=1000" (issue #1414).

## Context

- Symptom: screenshot from the instance, Antigravity → Google sign-in, the error above.
- Root cause: #1406 (SPEC-505 D) raised `LOGIN_COLS` (`crates/yulius-app/src/accounts.rs`) from
  500 to 1024 so the Antigravity CLI's 704-character Google sign-in link stays on one row. The
  process host's `LaunchSpec::validate` (`crates/yulius-contracts/src/ptyhost.rs`) allows at most
  `MAX_DIMENSION` = 1000 columns, so the session host refuses every login spawn.
- Why the tests missed it: the `FakeHost` session host in `crates/yulius/tests/accounts.rs`
  records the requested size but never applies the real host's limit, and the Antigravity test only
  asserted `cols >= 704`.
- What else the cause could break: only login terminals use `LOGIN_COLS`; task terminals take the
  browser's size through `Resize`, which the wire already bounds to 1..=1000.

## Requirements

- R1: a login terminal is at most `MAX_DIMENSION` columns, so the process host accepts its launch.
- R2: a login terminal is still at least 704 columns, so the Antigravity Google sign-in link
  stays whole (SPEC-505).
- R3: the accounts tests' fake session host refuses a size the real process host refuses, so a
  login wider than the host allows fails the tests.

## Plan

- [x] Reproduce the defect and find its root cause
- [ ] Write the fix spec and the regression test that fails
- [ ] Fix the cause until the regression test and every gate pass
- [ ] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- `LOGIN_COLS` equals `yulius_contracts::ptyhost::MAX_DIMENSION` (1000), which is >= 704.
- The accounts integration tests pass with a fake host that refuses sizes above `MAX_DIMENSION`;
  with `LOGIN_COLS` = 1024 they fail (21 of 27).

## Test plan

- Regression: `FakeHost::spawn` in `crates/yulius/tests/accounts.rs` returns an error for a size
  above `MAX_DIMENSION`, as the real host does; `an_antigravity_sign_in_ends_once_its_token_file_is_written`
  also asserts `cols <= MAX_DIMENSION`. Without the fix 21 accounts tests fail (the create call
  answers 500 instead of 201).
- `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^accounts::/)'` green with the fix.
- `scripts/dev quick`; the whole gate is Yulius CI's.

## Decisions

- Take the protocol's own bound (`LOGIN_COLS = MAX_DIMENSION`) instead of raising
  `MAX_DIMENSION`: the 1000 bound is a wire contract checked by the process host, its client, the
  terminal frames and the fuzzing generator; 1000 columns already exceed the 704-character link
  with room to spare. Tying the constant to `MAX_DIMENSION` keeps the two from drifting apart
  again. Raising the wire bound would be a protocol change for no user-visible gain.
