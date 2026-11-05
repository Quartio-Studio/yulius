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
  `<data>/claude-code/` and requires it to be a 64-bit little-endian ELF for the platform's
  machine that embeds `VERSION:"<version>"` (as Claude Code's builds do); it is never run on the
  host. It then moves to `versions/<version>/bin/claude.exe` (mode 0755 whatever the umask) and
  `current` is pointed at it by an atomic rename. Any failure leaves the version in use untouched,
  removes the temporary directory, is logged, and is tried again at the next check. Only a host
  package laid out as `bin/claude.exe` (the file the sandbox's `claude` link names) is updated or
  bound over; another layout is left to the host.
- CCU-003. **Use.** Every run that joins an isolated sandbox (task sessions and their resumes,
  Superagent sessions, people's shells, CI jobs and dev CD runs) gets, read-only at the host
  package's path, the newer of the host's Claude Code and Yulius's `current`, chosen when the run
  starts. A running process keeps its version: nothing is stopped, frozen or restarted for an
  update, and the next start of a session (Continue, a resume after a pause) uses the new version.
  Subscriptions and their tokens are untouched, so nobody signs in again.
- CCU-004. **Keep.** After a switch, `current` and the version before it stay; older versions are
  removed, except one a live process still executes (its binary is a process's executable), which
  a later check removes once it ended. Temporary directories left by an interrupted install are
  removed at the next check. The host's own install is never written, and an instance export
  leaves `claude-code/` out (it is built for this host's platform).
- CCU-005. **Off and fallbacks.** With `off`, on a host other than Linux, or when the host has no
  npm-installed Claude Code to bind over, nothing is checked or installed and runs use the host's
  Claude Code as before. A host version newer than Yulius's (an operator's manual update) is the
  one runs get.
- CCU-006. **Visible.** `yulius doctor` prints the version runs get and where it comes from
  (Yulius's auto-update or the host).

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Version policy: channels, versions and the platform package, unit-tested
- [x] A verified install of a released Claude Code into the data directory
- [x] Hourly update loop with the off switch and old versions removed
- [x] Runs bind the newest Claude Code at the host package's path
- [x] Doctor line, operations guide, ADR and architecture updated
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC1. With the registry offering a newer `stable` version than the host's, one check installs it
  under `<data>/claude-code/versions/<v>/bin/claude.exe` and `current` points at it; the next run's
  join binds that directory at the host package's path.
- AC2. A tarball whose sha512 differs from `integrity`, a binary that is not an ELF for the
  platform or does not embed the version, a tarball without a regular `package/claude`, a URL off
  `registry.npmjs.org`, or a tarball or binary over its bound installs nothing and leaves
  `current` as it was, with no temporary directory left.
- AC3. With `off`, no request is made. With the host's version equal to or newer than the channel's,
  nothing is downloaded.
- AC4. After installing a third version, the oldest one is removed and `current` plus the previous
  one remain; a version a live process executes stays until a check after it ended.
- AC6. A host package whose `claude` is not `bin/claude.exe` is neither updated nor bound over.
- AC5. A join made while the host's version is newer than `current` binds the host's package
  (same path, as before this change).

## Test plan

- Unit (`crates/yulius-domain/src/agent_cli.rs`): version parsing (plain `X.Y.Z` only, no path
  characters) and ordering, channel parsing, the platform package name, which versions to keep.
- Integration (`crates/yulius/tests/claude_updates.rs`, a local HTTP registry stub serving
  `dist-tags`, a version document and a gzip tarball whose `claude` is an ELF64 stand-in embedding
  its version across a 64 KiB read boundary): AC1 install, modes and switch; AC2 each refusal with
  its own reason leaves `current` and no temporary directory; AC3 nothing downloaded when not
  newer, no request with `off`; AC4 pruning to two versions and a version a live process runs
  (a copy of `/bin/sleep`) kept until it ended; AC5 and AC6; the agent-CLI bind
  (`agent_cli_binds`); the loop; the doctor's text per channel.
- Each consumer of `SandboxJoin.extra_binds` carries the bind into its containment: agent runs
  (`cd::an_agent_run_binds_the_agent_cli_of_its_join`), CI jobs
  (`local_ci::a_ci_job_binds_the_agent_cli_of_its_join`), people's shells
  (`shells::an_isolated_shell_binds_the_agent_cli_of_its_join`), dev CD runs and none in
  production (`cd::production_runs_join_the_production_holder_with_its_docker_engine`).
- The instance export's walk leaves `claude-code/` out (`instance::walk` tests).

## Decisions

- The download is never executed on the host (review round 1, finding 3): running it as the
  service user would give a compromised release the database and the vault key, where it is
  otherwise only ever run inside sandboxes. The registry's sha512, the ELF machine and the embedded
  version check what `--version` checked, without running anything.
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
- Review round 1 (request changes; all own code, all fixed test-first): H1 a host package of
  another layout would have been bound over: only `bin/claude.exe` hosts are updated
  (`a_host_package_of_another_layout_is_left_alone`). H2 a version a running session executes
  could be removed after two switches (its embedded ripgrep re-executes the binary): versions a
  live process runs are kept (`a_version_a_process_runs_is_not_removed`). M3 the download ran
  uncontained on the host: it is no longer run, the ELF machine and embedded version are checked.
  M4 refusals now each assert their own reason, with a missing `package/claude`, a non-ELF, a
  wrong machine and an oversized binary added (the `--version` timeout went with M3). M5 agent
  runs and CI jobs now have bind tests. L6 exports leave `claude-code/` out. L7 the doctor line
  honours `YULIUS_CLAUDE_UPDATES`. L8 this spec. Nits: leading zeros refused; the directories are
  synced after the switch; the join's few `std::fs` calls (a link, a small `package.json`, a
  `canonicalize`) stay synchronous, as `agent_cli_dir` and the other join preparation already are.
  The installed binary's mode was also found to follow the umask (0700 under 077): modes are now
  set explicitly and tested.
