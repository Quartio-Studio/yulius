---
yulius-task: e9bc9491-0759-4a95-920c-9b9d8cc67501
status: done
---

# SPEC-131: a committed switch turns the Vidoc gate on or off

Status: done
Date: 2026-10-14
Owner: Yulius task #119
Scope authorization: the person's request (task #119): "we'd like to be able to enable and
disable vidoc in the pipeline but atm it should be enabled".

## Goal

The Vidoc security scan in the delivery pipeline (`scripts/dev security`, SPEC-128, ADR-0104) can
be turned off and on again with one committed line, `enabled = false|true` in `.config/vidoc.toml`.
It ships enabled. When it is off, the gate says so and exits 0 at once, without a key, Node, the
network or a scan, and agents report the scan as disabled, not as passed.

## Context

- `scripts/vidoc_scan.py` is the gate: exit 0 passed (or nothing to scan), 1 failed, 2 not run.
  `main()` first reads the key (`settings`), then Node, installs the pinned CLI and runs `gate`.
  Agents run it for every change before the review (AGENTS.md, skill `yulius-verify`, the
  development guide). `scripts/dev ci` and Yulius CI do not run it (no secret in CI, #466).
- The pipeline that runs Vidoc is the agents' delivery pipeline: the same rules and skill reach
  every task, so a switch read by the gate itself turns the scan off everywhere at once, and a
  future CI job (#466) that calls `scripts/dev security` obeys it too.
- Repository tool settings live in `.config/` (`tools.toml`, `nextest.toml`) and are read with
  Python's `tomllib` by the gate scripts. `yulius.toml` is the Yulius product's project contract,
  parsed by the Rust domain (`spec_format/config.rs`); it ignores unknown tables, but a knob of
  this repository's own tooling does not belong in the contract every Yulius project shares.

## Requirements

- VSW-001: `.config/vidoc.toml` holds one key, `enabled`, a boolean. The committed value is `true`.
- VSW-002: With `enabled = false`, `scripts/dev security` prints
  `Security scan disabled: .config/vidoc.toml sets enabled = false; nothing was scanned.` and exits
  0 before it reads the key, checks Node, installs the CLI or calls the API. It calls Git only
  to check that the switch is committed (clean in `git status` and off in `HEAD`): an uncommitted
  `enabled = false` exits 2.
- VSW-003: With `enabled = true`, or without the file, the gate behaves exactly as in SPEC-128.
- VSW-004: A file that is not valid TOML, an `enabled` that is not a boolean, a missing `enabled`
  or any other key exits 2 ("not run") naming the file and the problem; the gate never guesses.
- VSW-005: AGENTS.md, the development guide and skill `yulius-verify` say where the switch is, that
  it ships on, and that a disabled scan is reported as disabled, never as passed.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] A committed switch in .config/vidoc.toml turns the Vidoc gate on or off, on by default
- [x] Agents' docs, the verify skill and ADR-0107 explain the switch
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- The repository's `.config/vidoc.toml` sets `enabled = true`, and a unit test pins it.
- `main()` with `enabled = false` and no key returns 0 and prints the disabled message; with
  `enabled = true` and no key it returns 2 as before.
- Each malformed file of VSW-004 returns 2 with the file named.
- `scripts/dev security` keeps passing the existing xtask test (missing key exits 2), proving the
  committed switch is on.

## Test plan

- `scripts/test_vidoc_scan.py` (run by `scripts/dev check`): `switch()` for a missing file, `true`,
  `false`, invalid TOML, a string value, a missing key and an unknown key; `main()` against a
  temporary Git repository with the switch committed off (exit 0, message, no key needed) and on
  (exit 2 without a key); `committed_switch()` with a fake Git (dirty, untracked, unreadable,
  `HEAD` on or malformed); a real repository with an uncommitted off switch and with one hidden by
  `--skip-worktree` (both exit 2); the committed file is on.
- `tools/xtask/tests/cli.rs`: the existing "missing key exits 2" test runs through the committed
  switch.

## Decisions

- ADR-0107: the switch is a committed file the gate reads, `.config/vidoc.toml`, not a key in
  `yulius.toml`, not an environment variable and not a Yulius UI setting. Turning the scan off is a
  reviewed change in a pull request with a reason, visible in history, and it reaches every task
  that merges `dev`.
- A disabled gate exits 0, not 2: "disabled" is a decision of the project, while 2 means the gate
  wanted to run and could not, which agents must chase. The message, not the code, tells agents
  to report "disabled".
- A missing file means enabled: deleting the file cannot silently turn security off.
- A malformed file exits 2 rather than falling back to either value: a typo such as
  `enabled = "false"` must be noticed, not read as on or off.
- Review round 1 (fresh session, approved 2d09aae7 with four minors) found that an uncommitted
  `enabled = false` turned the gate off with nothing in history. The off path now asks Git that the
  switch is committed and exits 2 otherwise. The PR that turns the scan off is not scanned itself;
  ADR-0107 asks its reviewer to check it changes only the switch and says why. The other minors
  (ADR-0104 cross-reference, the missing-`enabled` case in the ADR, plan ticks) and the nits
  (binary read for `tomllib`, separate messages for extra keys and a non-boolean) were fixed.
- Review round 2 (fresh session, approved 63d74cc2) showed that `--skip-worktree` or an ignored
  file hides an off switch from `git status`. The off path now also reads the switch from `HEAD`
  (`git cat-file blob HEAD:.config/vidoc.toml`) and exits 2 unless it says off, and the tests'
  temporary repositories ignore the caller's `GIT_*` variables.
- Review round 3 (fresh session) approved 04836e61 with nits only: the spec's and ADR's wording
  were fixed. A `HEAD` blob that is not UTF-8 behind `--skip-worktree` ends the gate with a
  traceback (exit 1): it still fails closed, so it stays as is. That was the third and last round.

## Evidence

- `python3 -m unittest discover -s scripts -p 'test_*.py'`: 91 tests OK, 52 of them in
  `scripts/test_vidoc_scan.py` (9 in `Switch`: the committed file is on; on, off and missing;
  six malformed files; `committed_switch()` with a fake Git; a real repository with an uncommitted
  and a `--skip-worktree` off switch, both exit 2; `main()` disabled exits 0 without a key, enabled
  exits 2 without one, malformed exits 2).
- `python3 scripts/check_repository.py`: passed (unique decision numbers, skills in sync).
- `scripts/dev ci` (doctor, check, lint, test, deps, rustdoc) after `scripts/bootstrap.py`: exit 0,
  nextest 1639 passed, 16 skipped. No Rust changed after it; Yulius CI runs on the delivered head.
- `scripts/dev security` with the switch temporarily off (not committed, before the committed-switch
  rule): `Security scan disabled: .config/vidoc.toml sets enabled = false; nothing was scanned.`,
  exit 0, through xtask.
- `scripts/dev security` with the committed switch on: exit 2, "Vidoc rejected the key (HTTP 401:
  Invalid or expired token)": the switch let the gate run, and the scan itself is unrun because
  `api.vidoc.dev` is not yet allowed for `VIDOC_API_KEY` (#470).
- Reviews: three fresh read-only sessions, each approving; every minor fixed test-first.
