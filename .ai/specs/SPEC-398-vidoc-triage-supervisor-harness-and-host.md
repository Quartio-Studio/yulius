---
yulius-task: ece3cd8e-a84c-4cb9-86dd-9f90c6435d17
status: in-progress
---

# SPEC-398: Triage of the Vidoc findings on the supervisor, harness and host setup (#813 #696 #801 #677 #728)

Status: in-progress
Date: 2026-11-03
Owner: Yulius task #381
Scope authorization: task #381 (a person asked for each of the five issues to end closed, as a
false positive or as solved, without creating new issues).

## Goal

Each of the five Vidoc findings filed as #813, #696, #801, #677 and #728 is checked against `dev`
(88a485ca) and either fixed test-first or dismissed with its reason, the guard that stops it and a
test that pins that guard. A dismissed wording gets one narrow `[[ignore]]` entry in
`.config/vidoc.toml`, so Yulius does not reopen the issue on the next scan.

## Context

- Vidoc files each finding once as an issue and reopens it when it reports the finding again after
  it was closed; a false positive needs an `[[ignore]]` entry (path exact, title a
  case-insensitive substring, SPEC-217, SPEC-273). Entries stay narrow, one per wording on record,
  with no `function` or `line` (SPEC-262 VTR-005, as SPEC-393 did).
- The server runs outside containment and a sandbox's whole directory is mounted read-write into
  its runs, so an agent can swap any folder below it for a link; the server reaches such folders
  from an anchor it owns, one component at a time without following links (ADR-0174). SPEC-253
  CFG-006 left one exception: the Claude config's trust key canonicalizes the worktree path, a
  residual noted on #634.
- `yulius host setup` and `yulius host check` run as root; their values are the operator's options
  (`--data-dir`/`YULIUS_DATA`, `--user`, `--service`/`YULIUS_UPDATE_SERVICE`), and
  `yulius instance import` runs the same items with its target directory (SPEC-329).

## Requirements

- VSH-001 (#813, dismissed). Attempt ids (`supervisor.rs:1359-1361`) and every other id come from
  `Crypto::new_id`, whose only production implementation (`MasterKeyCrypto`, wired at
  `serve.rs:438`) is `uuid::Uuid::new_v4()` (`crypto.rs:165-167`): 122 random bits from
  `getrandom`, the operating system's CSPRNG (`uuid` with feature `v4`, `Cargo.toml:58`;
  `getrandom 0.4` in `Cargo.lock`). `random_bytes` (`crypto.rs:169-173`), which the bearer tokens
  use, is `rand::rng()`, a ChaCha CSPRNG seeded from the operating system. A run directory's name
  grants nothing anyway: run access goes through the run's bearer token. Pinned by a new
  `crypto.rs` test that ids are distinct random (version 4) UUIDs.
- VSH-002 (#696, dismissed). The profile's branch names are validated where they are written and
  again where Git gets them. Every writer of a profile's branches validates them or writes a
  constant preset: the profile editor (`delivery.rs:282`), the import of a repository's
  `yulius.toml` (`import_profile`, `projects.rs:87-95`, through `parse_yulius_toml`, which runs
  the same validation, `spec_format/config.rs:890-892`), and the presets (`DeliveryProfile::
  for_preset`, `standard`, `projects.rs:632`, `projects.rs:897`); the Vidoc switch
  (`delivery.rs:2010-2020`) rewrites the stored object keeping its validated fields. The check is
  `DeliveryProfile::validate` (`yulius-domain` `delivery.rs:360-362`, `validate_branch`
  `delivery.rs:430-452`: no leading `-`, whitespace, control character or `~^:?*[\`). The
  sandbox's `default_branch` (GitHub's answer, not a profile field) reaches Git only as
  `origin/<default_branch>` through `rev_parse`. At use, `branch_exists` validates the name
  (`repo.rs:383`), `rev_parse` refuses a revision starting with `-` (`validate.rs:244-248`) and passes `--end-of-options`
  (`repo.rs:362`), `worktree add` passes `--` before its path (`worktree.rs:77`), and the push
  guard validates every protected branch (`hooks.rs:164-166`) and single-quotes it in the hook
  script (`hooks.rs:74`). Git runs with an argv, never a shell. Pinned by `profile_validation`
  (`yulius-domain` `delivery.rs:1623`), `revisions_cannot_be_options` (`validate.rs:329`),
  `protected_names_are_single_quoted` (`hooks.rs:198`) and
  `pre_push_guard_blocks_protected_branches_only` (`git_ops.rs:975`).
- VSH-003 (#801, fixed). The trust key of the worktree is the real path of the folder the agent
  cannot replace (for a task the sandbox directory above its home, for the Superagent the sandbox
  directory its repository lies in) and the rest of the worktree's path below it as named; that
  rest is opened folder by folder with `O_DIRECTORY | O_NOFOLLOW`, and the worktree's `.git` is
  read through the handle. A link the agent puts at the worktree, or at any folder between the
  anchor and it, is never resolved: the key names the link, no key names its target, and no git
  file is read through it (the worktree alone is trusted). A worktree outside its anchor, or one
  naming `..`, is keyed as given and not opened. Without links the keys are unchanged: Claude Code
  still finds its trust by the real path of its working directory, also behind a host link above
  the anchor (macOS `/var`). The Superagent's working directory is its shell's folder, which
  `new_shell_runtime` resolved in full when the shell was made (`shells.rs:819`): a shell's folder
  now resolves only the sandbox directory the same way (`named_folder`), when it is made and when
  it is adopted again after a restart (`App::shell_folder`), so a link at the sandbox repository or
  a task worktree is named, never resolved, for terminals and the Superagent alike (review rounds
  1 and 2). A worktree path stored under an older data-directory path (a moved data
  directory) is keyed as given and its main repository not trusted; an instance import keeps the
  data directory (`instance/import.rs:139`), so this needs a hand move.
- VSH-004 (#677, fixed for the data directory; dismissed for the service name). `host setup`,
  `host check` and the import's user step refuse a data directory that is not absolute before
  running anything, so it can never reach `install -d`, `useradd --home-dir`, `chmod` or the
  helper commands as an option. The service name is already checked before any `systemctl` call:
  `Run::all` refuses one that is not `[A-Za-z0-9@._-]+.service` without a leading `-`
  (`host_setup.rs:454-457`, `service_name_ok` `host_setup.rs:217-224`), and `user_and_ids` (the
  import) runs no `systemctl`. A new test pins both refusals: nothing runs.
- VSH-005 (#728, dismissed). The reading is not taken on the session's word about which account it
  is: the status line hook authenticates the run by its own bearer token (`agent_context`) and
  updates only the account the server recorded for that attempt (`hooks.rs:354-355`,
  `ctx.attempt.account_id`). Its values are bounded (`session_window`, `capacity.rs:114-133`: a
  finite percentage from 0 to 1000, a reset time in the future and at most eight days ahead, else
  unknown). The session spends that account's quota itself, so a reading it reports, true or not,
  can only say what its own use could make true; a `claude setup-token` sign-in has no other
  source (the usage endpoint refuses it, ADR-17), and the next reading of the account (a probe or
  another session) replaces it. Pinned by a new `capacity.rs` test of those bounds and the
  existing `subscription_pool::the_running_session_reports_its_subscriptions_usage`.

## Plan

- [x] Spec with the triage of all five findings
- [x] The Claude config trusts the worktree without following the agent's links (#801)
- [x] Host setup refuses a data directory that is not absolute (#677)
- [x] Pinning tests and narrow [[ignore]] entries for the dismissed findings (#813 #696 #728 #677)
- [ ] Docs, changes entry, gate and evidence
- [ ] Independent review answered
- [ ] Issues commented: dismissed ones closed, fixed ones linked to the pull request

## Acceptance criteria

- A terminal opened in a sandbox whose repository was swapped for a link to a host folder reports
  the link's path (the sandbox directory's real path and `repo`), not the target, also once it is
  adopted again after a server restart; the test fails on `dev`.
- With the worktree folder (or the `worktrees` folder above it) swapped for a link to a folder
  outside the sandbox that holds a linked worktree's `.git`, `.claude.json` holds no key naming the
  link's target or the main repository its `.git` names, and the link's own path is trusted; the
  test fails on `dev`.
- `host setup`, `host check` and `user_and_ids` with a relative data directory, or `all` with a
  service name starting with `-`, fail their first line and run no command; the data-directory
  test fails on `dev`.
- `.config/vidoc.toml` has one `[[ignore]]` entry per dismissed wording (the titles of #813, #696,
  #728 and #677), each with its reason; none uses `function` or `line`.
- The pull request's description has `Fixes #801` and `Fixes #677`; #813, #696 and #728 carry a
  comment with the reason and file:line evidence on `dev` and are closed as not planned; no issue
  is created.

## Test plan

- `cargo nextest run -p yulius-harness -E 'test(/claude_config::/)'`: the new link test and every
  existing one (trust keys, linked worktrees, config dir links).
- `cargo nextest run -p yulius -E 'test(/host_setup::/)'`: the new refusal test and the others.
- `cargo nextest run -p yulius -E 'binary_id(yulius::main) & (test(/^shells::/) |
  test(/^cockpit::/))'`: the new shell-folder test (new and adopted shells), terminals and the
  Superagent.
- The new `yulius-store` crypto test and `yulius-app` capacity test.
- `scripts/vidoc_scan.py`'s config parser accepts `.config/vidoc.toml`.
- `scripts/dev ci`: the full gate.

## Decisions

- **#801: anchor, then handles (ADR-0174), for the trust key too.** SPEC-253 kept canonicalizing
  the worktree because Claude Code keys trust by the real path and resolving only the host's
  prefix needed each caller to pass its anchor. Passing it is cheap (two callers), and only the
  anchor may sit behind a host link: the folders below it are Yulius's own (`worktrees/<n>`,
  `repo`). Alternatives: refusing a worktree whose real path leaves the sandbox (still resolves the
  link, an existence oracle on host paths, and still reads `.git` through it), or keeping the
  residual (the host disclosed where a planted link leads in a file the agent reads). No new ADR:
  this applies ADR-0174 and removes CFG-006's exception.
- **#677: an absolute data directory is a precondition, not a per-command check.** Every command
  of the setup that names the data directory then gets a path starting with `/`; checking at the
  entry covers the helper commands too (`systemctl` environment, the doctor, the base build),
  which a check in front of `install` alone would miss. A relative data directory was never a
  working setup (the unit file's `YULIUS_DATA` needs an absolute path).
- **Pinning tests for dismissed findings** where no test held the guard yet (#813, #728, the
  service name of #677); #696's guards already had tests.

## Evidence

Pending.
