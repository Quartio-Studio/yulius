---
yulius-task: ac2fdd52-58d5-4db4-be32-7d9b89b34542
status: in-progress
---

# SPEC-421: Claude Code updates itself on an instance, and nobody signs in again

Status: in-progress
Date: 2026-11-05
Owner: Yulius task #403
Scope authorization: task #403, asked by the project's owner: "Claude should autoupdate when new
version is released without logging out. Fix." Decision record:
[ADR-0266](../../docs/decisions/0266-yulius-keeps-claude-code-current-in-its-data-directory.md).

## Goal

When Anthropic releases a new Claude Code, an instance's tasks use it on their own: no person
installs it, no subscription is signed in again, no running session is stopped.

## Context

- Every session, sign-in terminal and split proposal runs with `DISABLE_AUTOUPDATER=1` (#45,
  `crates/yulius-harness/src/launch.rs`): Claude Code's own updater can never write the system
  install from a contained run, so it only failed. Since then nothing updates Claude Code: the
  operator installs it system-wide with npm (`docs/operations.md` §1, "its version is yours to
  choose"). The instance has run 2.1.292 since 2026-10-07; on 2026-11-05 npm's `stable` is 2.1.311
  and `latest` 2.1.318.
- Runs in an isolated sandbox (SPEC-002 ADR-22) see the host's Claude Code package bound
  read-only over their system layer at its own path (`agent_cli_dir`, `SandboxJoin.extra_readonly`,
  `crates/yulius/src/wiring/sandbox.rs`); the base's `/usr/local/bin/claude` link points into it.
  The bind is made per run, so a run sees whatever the bound directory holds when it starts.
- Updating the host's package needs root (the install is root-owned under `/usr/local`); the root
  update helper (ADR-0013) knows only `build` and `install` of Yulius itself, and its unit cannot
  write `/usr/local/lib/node_modules`.
- Sign-ins do not depend on the CLI's version: a subscription's token is sealed in the database and
  handed to each run as `CLAUDE_CODE_OAUTH_TOKEN`; `CLAUDE_CONFIG_DIR` is the project's home.
- npm publishes the CLI as `@anthropic-ai/claude-code` (a wrapper whose `postinstall` copies the
  native binary) and one package per platform, `@anthropic-ai/claude-code-<os>-<cpu>[-musl]`, whose
  tarball holds the native binary `package/claude`; the registry's version document gives the
  tarball URL and its sha512 `integrity`. Its `dist-tags` are `latest` and `stable`.

## Requirements

- CCU-001. **Check.** The server reads the `dist-tags` of `@anthropic-ai/claude-code` from
  `https://registry.npmjs.org` about every hour (the first check 2 minutes after a start), for the
  channel `YULIUS_CLAUDE_UPDATES` (`--claude-updates`) names: `stable` (default), `latest` or `off`.
  Each request has a 15 s deadline, https only, no redirects, a bounded body; only a plain
  `X.Y.Z` version is accepted.
- CCU-002. **Install.** When the channel's version is newer than the version runs get (CCU-003),
  the server downloads the platform package's tarball of that version (the URL must be on
  `registry.npmjs.org`; at most 512 MiB), checks its sha512 against the registry's `integrity`,
  takes only the regular file `package/claude` (at most 1 GiB) into a temporary directory under
  `<data>/claude-code/`, runs it with `--version` (30 s, the child reaped on timeout) and requires
  the output to start with the version, then moves it to `versions/<version>/bin/claude.exe` and
  points `current` at it by an atomic rename. Any failure leaves the version in use untouched,
  removes the temporary directory, is logged, and is tried again at the next check.
- CCU-003. **Use.** Every run that joins an isolated sandbox (task sessions and their resumes,
  Superagent sessions, people's shells, CI jobs and dev CD runs) gets, read-only at the host
  package's path, the newer of the host's Claude Code and Yulius's `current`, chosen when the run
  starts. A running process keeps its version: nothing is stopped, frozen or restarted for an
  update, and the next start of a session (Continue, a resume after a pause) uses the new version.
  Subscriptions and their tokens are untouched, so nobody signs in again.
- CCU-004. **Keep.** After a switch, `current` and the version before it stay; older versions are
  removed. Temporary directories left by an interrupted install are removed at the next check. The
  host's own install is never written.
- CCU-005. **Off and fallbacks.** With `off`, on a host other than Linux, or when the host has no
  npm-installed Claude Code to bind over, nothing is checked or installed and runs use the host's
  Claude Code as before. A host version newer than Yulius's (an operator's manual update) is the
  one runs get.
- CCU-006. **Visible.** `yulius doctor` prints the version runs get and where it comes from
  (Yulius's auto-update or the host).

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [ ] Version policy: channels, versions and the platform package, unit-tested
- [ ] A verified install of a released Claude Code into the data directory
- [ ] Hourly update loop with the off switch and old versions removed
- [ ] Runs bind the newest Claude Code at the host package's path
- [ ] Doctor line, operations guide, ADR and architecture updated
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC1. With the registry offering a newer `stable` version than the host's, one check installs it
  under `<data>/claude-code/versions/<v>/bin/claude.exe` and `current` points at it; the next run's
  join binds that directory at the host package's path.
- AC2. A tarball whose sha512 differs from `integrity`, a binary whose `--version` names another
  version or fails, a URL off `registry.npmjs.org`, or a body over the bound installs nothing and
  leaves `current` as it was, with no temporary directory left.
- AC3. With `off`, no request is made. With the host's version equal to or newer than the channel's,
  nothing is downloaded.
- AC4. After installing a third version, the oldest one is removed and `current` plus the previous
  one remain.
- AC5. A join made while the host's version is newer than `current` binds the host's package
  (same path, as before this change).

## Test plan

- Unit (`crates/yulius-domain/src/agent_cli.rs`): version parsing (plain `X.Y.Z` only, no path
  characters) and ordering, channel parsing, the platform package name, which versions to keep.
- Integration (`crates/yulius/tests/claude_updates.rs`, a local HTTP registry stub serving
  `dist-tags`, a version document and a gzip tarball whose `claude` is a shell script printing a
  version): AC1 install and switch; AC2 each refusal (bad integrity, wrong `--version`, foreign
  URL, oversized body) leaves `current` and no temporary directory; AC3 nothing downloaded when not
  newer, no request with `off`; AC4 pruning to two versions.
- Unit of the join's agent-CLI bind (`crates/yulius/src/wiring/claude_cli.rs`): AC1's bind and
  AC5's host fallback.
- Existing containment tests keep passing with the bind carried as a read-only `Bind`.

## Decisions

- Yulius installs Claude Code into its own data directory as its service user instead of updating
  the host's install: no root step on any instance (a root timer would need `yulius host setup`
  rerun by a person on every instance and a writable `/usr/local` for the helper), versions are
  switched atomically and never half-installed, and a running session's binary is never replaced
  under it. See ADR-0266 for the alternatives.
- The native binary is taken from the platform package directly, checked against the registry's
  sha512, and never through `npm install`: no Node.js on the server's path, no `postinstall`
  script runs on the host.
- `stable` is the default channel: Anthropic's `stable` lags `latest` by about a week and skips
  releases with major regressions, which matters for unattended sessions; `latest` is one setting
  away.
- Only runs in isolated sandboxes get Yulius's version. The sign-in terminal (`claude
  setup-token`), the Jev split proposer and uncontained runs on development hosts keep resolving
  `claude` on the host's `PATH`: they run on the host, outside the bound system layer, and none of
  them depends on the CLI's version.
- Two versions are kept: a shell or session that bound the previous version can still start
  `claude` in it until the next switch; each version is about 300 MB.
