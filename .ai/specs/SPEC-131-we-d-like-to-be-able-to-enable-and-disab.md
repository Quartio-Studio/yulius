---
yulius-task: e9bc9491-0759-4a95-920c-9b9d8cc67501
status: in-progress
---

# SPEC-131: a committed switch turns the Vidoc gate on or off

Status: in-progress
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
  0 before it reads the key, checks Node, installs the CLI, touches Git or calls the API.
- VSW-003: With `enabled = true`, or without the file, the gate behaves exactly as in SPEC-128.
- VSW-004: A file that is not valid TOML, an `enabled` that is not a boolean, a missing `enabled`
  or any other key exits 2 ("not run") naming the file and the problem; the gate never guesses.
- VSW-005: AGENTS.md, the development guide and skill `yulius-verify` say where the switch is, that
  it ships on, and that a disabled scan is reported as disabled, never as passed.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [ ] A committed switch in .config/vidoc.toml turns the Vidoc gate on or off, on by default
- [ ] Agents' docs, the verify skill and ADR-0107 explain the switch
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

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
  temporary root with the switch off (exit 0, message, no key needed) and on (exit 2 without a
  key); the committed file is on.
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
