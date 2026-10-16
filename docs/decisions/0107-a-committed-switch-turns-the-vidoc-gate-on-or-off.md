# 0107. A committed switch turns the Vidoc gate on or off

## Status

Accepted. Specified in [SPEC-131](../../.ai/specs/SPEC-131-we-d-like-to-be-able-to-enable-and-disab.md).
Amends [ADR-0104](0104-agents-scan-their-branch-with-vidoc-before-delivery.md). Amended by [ADR-0157](0157-vidoc-is-an-optional-instance-key-and-the-scan-runs-on-vidoc-1.md): the switch is read from `HEAD`, and the file also holds `exclude` globs and `[[ignore]]` entries.

## Context

The person asked: "we'd like to be able to enable and disable vidoc in the pipeline but atm it
should be enabled". Since ADR-0104, every agent runs `scripts/dev security` (`scripts/vidoc_scan.py`)
before delivery, and no setting can turn it off: stopping the scan meant editing the rules in
AGENTS.md, the development guide and skill `yulius-verify`. The scan costs 10 to 15 minutes of
each task and the Vidoc plan's quota, and the key expires, so a project can have good reasons to
pause it.

## Decision

`.config/vidoc.toml` holds one key, `enabled`, committed as `true`. The gate reads it first. With
`enabled = false`, it prints `Security scan disabled: .config/vidoc.toml sets enabled = false;
nothing was scanned.` and exits 0, before it reads the key, checks Node or calls Vidoc. It only
asks Git whether the switch is committed (clean in `git status`, off in `HEAD`): an uncommitted
`enabled = false` exits 2, so the scan is turned off by a reviewed commit, not by an edit in an
agent's tree. Agents report a disabled run as disabled, never as passed. Without the file the
gate is on. A file that is not valid TOML, a missing `enabled`, an `enabled` that is not a
boolean, or any other key exits 2 (not run).

## Alternatives

- **A `[security]` table in `yulius.toml`.** That file is the Yulius product's contract, shared by
  every project Yulius manages and parsed by its Rust domain. The Vidoc gate is this repository's
  tooling, like the pins in `.config/tools.toml`, so its switch sits next to them.
- **An environment variable.** It would be per session, invisible in review and in history, and an
  agent could turn the scan off for itself. Rejected.
- **A setting in the Yulius UI.** Yulius does not run the gate; agents and `scripts/dev` do. A
  product setting would need a way to reach a repository script, for a single repository's tool.
- **Exit 2 when disabled.** Exit 2 means "wanted to run and could not", which agents chase (bind
  the host, replace the key). A disabled scan is the project's decision, not a problem to fix.

## Consequences

- Turning the scan off or on is a one-line pull request, reviewed and visible in history. It takes
  effect for every task once the task's branch has the commit (after it merges `dev`).
- The switch applies to the branch that carries it, so the pull request that turns the scan off is
  not scanned itself. Its reviewer checks that the change is only the switch and that the pull
  request says why.
- A future CI job for the gate (#466) obeys the same switch through `scripts/dev security`.
- Deleting the file keeps the scan on, and a typo stops the gate with exit 2 rather than silently
  turning the scan off.
