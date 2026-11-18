---
yulius-task: d127ec64-2008-464e-a0f8-a622a6af5dcd
status: done
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
- [x] Write the fix spec and the regression test that fails
- [x] Fix the cause until the regression test and every gate pass
- [x] Independent review in a fresh session, findings fixed test-first

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

## Evidence

- Red at 5d2736cd2 (regression test only):
  `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^accounts::/)'`:
  27 run, 6 passed, 21 failed; each create call answers 500 `internal` instead of 201.
- Green at 9476119e7 (fix): the same command, 27 passed; `cargo nextest run -p yulius-app -E
  'test(/accounts|login/)'`: 12 passed; `scripts/dev quick`: passed.
- Yulius CI run e597e2f7 on 9476119e7 (quality, test 1/2, test 2/2, e2e): passed. Security
  (Vidoc) skipped, exit 78, as on dev ac2b917a1.
- Review round 1 (yulius-reviewer, Fable 5.2) approved 9476119e7 and reproduced red and green
  on its own copies. Answers:
  - Minor, own code: five comments and docs still said 1024 columns (`yulius-harness`
    `login.rs`, web `README.md`, `admin.js`, `terminal-core.js`, `e2e/README.md`). Fixed: they
    say 1000.
  - Nit, own code: the edited `docs/api.md` paragraph was wrapped past 100 columns. Rewrapped.
  - Nit, own code: optional compile-time `const` assertion of 704 <= `LOGIN_COLS` <=
    `MAX_DIMENSION`. Not added: `LOGIN_COLS` is `MAX_DIMENSION` itself, the fake host refuses
    any size above it, and the exact `(1000, 40)` and `cols >= 704` test assertions catch either
    bound moving.
- Not testable here: a sign-in through the real process host and real provider CLIs on the
  instance; the bound is the host's own `LaunchSpec::validate`, which the fake host now mirrors.
