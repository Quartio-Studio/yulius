---
yulius-task: a7231648-2f66-4803-bcab-6f62896501ad
status: done
---

# SPEC-271: Laptop onboarding moves a project's operator state into its sandbox

Status: done
Date: 2026-10-22
Owner: task #257
Scope authorization: task #257. Found while onboarding Quartio-Studio/product from a laptop
(SPEC-256's first user): "a person's agent can send a directory (or an archive of it) of operator
state from the laptop into the project's sandbox, and the project's agents can restore it where
their tasks run, with the same custody as agent secrets."

## Goal

A project whose deploys keep live state on the laptop (a folder with a SQLite inventory, rollout
folders, configs) moves that state into Yulius with the rest of laptop onboarding: the person's
agent sends the folder with `yulius secrets import --state NAME=DIR`, a task restores it into the
sandbox home with an agent tool, and from then on the sandbox's copy is the one written to, kept
across tasks, backed up into the vault and downloadable, and never forked by a second import.

## Context

Confirmed against `dev` at a85eb122.

- SPEC-256 (ADR-0194) made unattended laptop onboarding: `yulius secrets import --yes` with the
  person's operate token, `--from-file NAME=PATH` for key files (64 KiB, one value), `--env-files`.
- An agent secret is one value of at most `MAX_FIELD_BYTES` (16 KiB) stored as an envelope
  (`Crypto::envelope_seal`, a data key per row wrapped by the vault key), audited `vault.agent.*`;
  values never appear in tool answers (`tool_result` redacts) or transcripts (the run's
  fingerprint matcher and `yulius_domain::redact`).
- Nothing writes files into a sandbox's home for an agent. `secret_copy_to_env` and
  `secret_create_from_file` reach the sandbox through folder handles (ADR-0174, `env_files::open_folder`,
  `files::open_dir_at`, `files::remove_tree`); the sandbox home (`<sandbox>/home`, `~/` for a
  task) is shared by all tasks of a sandbox and outlives them.
- The real case: `~/.quartio-operator-20260901` (a 536 KB SQLite inventory with pinned host keys,
  an upgrade journal and rollout locks; rollout folders with `bundle.json`, receipts and
  `operator.json`; configs naming key files by path), `~/.local/share/quartio/admin-mcp` and
  `~/.local/share/quartio/tools/ops`. The keys themselves are agent secrets already.
- Gzip tar is already read (`imports::safe_relative`, playground folders) and written
  (`yulius-import` snapshots) with the `tar` and `flate2` crates.

## Requirements

- **OPS-001** An *agent state* is a project's (sandbox's) named folder of files, stored in the
  vault as one gzip tar sealed as an envelope bound to its row and version (encrypted at rest).
  Names follow agent secret names (`SecretName`) and are unique per sandbox. Its record holds
  the size, the file count, a version (1 at import, +1 on each save or replace), who changed it
  last and when, and once restored: where (`restored_path`, `~/…`), which version, by which
  task, when.
- **OPS-002** `PUT /api/sandboxes/{sid}/agent-states/{name}` (raw `application/gzip` body, at
  most 64 MiB; `?description=`, `?replace=true`) stores the archive. The caller must manage the
  sandbox's agent secrets (`Action::Configure`). The archive is checked before it is stored:
  readable gzip tar; only regular files and folders; relative paths without `..`; at most 20 000
  entries and 512 MiB expanded. Audited `vault.state.import` (or `vault.state.replace`) as the
  person (their login, as agent secret imports are), with name, files and bytes, never content.
- **OPS-003** One writer. A state never restored is replaced by a new import (the laptop is still
  its writer). Once restored, an import is refused with 409, naming where the sandbox's copy is
  and since when, unless `replace=true`; a replace bumps the version, so the sandbox's copy is
  stale until restored again over it.
- **OPS-004** `yulius secrets import --to agent --state NAME=DIR` (repeatable, combinable with a
  dotenv file and `--from-file`; `--replace-state`) packs DIR (a leading `~/` is the home), or
  sends an existing `.tar.gz`/`.tgz` as it is. It refuses before sending a folder holding a link
  or a special file, a pack above 64 MiB, a name given twice, `--instance` and `--to team`. It
  shows names, file counts and sizes only, asks for the typed `yes` like secrets, and sends
  unattended with `--yes` only under the operate token (SPEC-256 LOB-002). After a send it tells
  the laptop that once a task restores the state, the sandbox's copy is the one written to and
  DIR must no longer be used for rollouts. A 409 is reported as such, the rest still sent.
- **OPS-005** Agent tool `secret_state_restore {name, path}` (tasks): writes the state into
  `path`, a `~/…` folder of the sandbox home, through folder handles (no link followed, every
  folder on the way opened with `O_NOFOLLOW`, created when missing), keeping each file's
  permission bits (`0o777`; setuid, setgid and sticky dropped). It unpacks into a fresh sibling
  folder and renames it into place. Allowed when: never restored and `path` is missing or an
  empty folder; or restored at `path` and the vault holds a newer version (a replace from the
  laptop: the old folder is removed after the new one is in place); or the recorded copy is gone
  (restored again from the newest version, at any `path`). Refused when the recorded copy exists
  and is current ("use it"), or another `path` is asked while it exists. The answer gives the
  path, files, bytes and version, never content. Audited `vault.state.restore`.
- **OPS-006** Agent tool `secret_state_save {name}` (tasks): reads the restored folder through
  handles (links and special files are left out and named in the answer), packs it within the
  OPS-002 bounds, and stores it as the next version with a compare-and-set on the version: a
  replace from the laptop not yet restored, or a concurrent save, is refused. Audited
  `vault.state.save`. It is the backup after each rollout, and what goes back to the laptop.
- **OPS-007** `GET /api/sandboxes/{sid}/agent-states` lists the states (no content);
  `GET …/agent-states/{name}/archive` answers the newest archive to a person who manages the
  sandbox's agent secrets, audited `vault.state.export`; `yulius secrets state-get NAME --out
  FILE` writes it (mode 0600, never over an existing file) and says the sandbox's copy stays the
  writer. `secrets_list` (task, Superagent and remote MCP) names each state with its restore path
  and version, and how to use the two tools.
- **OPS-008** Custody: no tool answer, CLI line or audit detail holds a state's content; the
  briefing and tool descriptions tell agents never to print the restored files; key material
  stays agent secrets (the transcript redaction covers it); the state archive is held in memory
  at most twice at once on the server (two operations at a time, `Capacity` beyond).
- **OPS-009** Documented: `docs/remote-control.md` section 3 (a "Move the operator state" step),
  the remote MCP instructions, the onboarding prompt (restore states the hand-over names),
  `docs/architecture.md`, ADR-0200.

## Plan

- [x] Spec and ADR-0200: operator state is a sealed, versioned agent state with one writer
- [x] The vault stores, lists and checks agent states (migration 0042, store, archive checks)
- [x] People send and download states over the API with the single-writer rule
- [x] The CLI sends folders with --state and gets them back with state-get
- [x] Tasks restore a state into the sandbox home through folder handles
- [x] Tasks save the restored copy back as a new version
- [x] Agents and the laptop recipe know about states (secrets_list, instructions, onboarding prompt, docs)
- [x] Review findings fixed test-first: archive bounds, single-writer races, memory, read tokens
- [x] Gates and security scan are green and the spec has its evidence
- [x] The pull request is merged into dev on green

## Acceptance criteria

- **AC1** With only an operate token stored, `yulius secrets import --to agent --sandbox S --yes
  --state OPS=~/ops` sends a folder of nested files with modes 0600/0755 and a 50 MiB file; the
  output holds no content; the audit holds `vault.state.import` by the person's login.
- **AC2** A task's `secret_state_restore {name: OPS, path: "~/.ops"}` creates `<sandbox>/home/.ops`
  with the same files, bytes and modes; a second call is refused (current copy exists); a call
  for another path is refused.
- **AC3** After the restore, the CLI import of OPS exits 1 with the 409 message and changes
  nothing; with `--replace-state` it is stored as version 2, and a restore replaces the folder.
- **AC4** A task changes a file and calls `secret_state_save`: version +1; `state-get` downloads
  an archive holding the changed file.
- **AC5** Refused before anything is stored: an archive with a link, `..`, an absolute path, a
  device; a body over 64 MiB; a name that is not a secret name; a caller without Configure.
- **AC6** A link planted on the way to the restore path, or in place of the target, is not
  followed; nothing is written outside the home.

## Test plan

- Unit (`yulius-app` `vault/states.rs`): archive checks (links, `..`, specials, bounds), mode
  masking, restore placement and refusals over a temp home, save packing skipping links.
- Unit (`yulius-import`): packing a folder (modes, refusal of links, size bound), `--state` parsing.
- Store: insert/replace/restore/save compare-and-set, cascade with the sandbox.
- Integration `crates/yulius/tests/remote_mcp.rs` (the remote surface): AC1, AC3, AC4 through the
  real `yulius` binary against a served instance, AC2 and AC4 through the task MCP endpoint.
- Integration (`crates/yulius/tests/agent_states.rs`): AC5, AC6 over REST and MCP.
- Gates: `scripts/dev ci`, `scripts/dev security`.

## Decisions

- **ADR-0200**: operator state is an agent state: a sealed gzip tar per name and version in the
  vault database, restored by a task into the sandbox home, with the sandbox's copy as the only
  writer once restored. Alternatives: splitting into many secrets (64 KiB, one-line values,
  no modes), routing through another repository (state in Git history, outside the vault),
  sealed files beside the database (two stores to keep consistent; 64 MiB rows are within
  SQLite's limits), a sync service (two writers by design).
- **Refuse over notify**: the laptop cannot be reached once the CLI exits, so the single-writer
  rule is the server's 409 over a restored copy; the CLI also says it after each send.
- **Restores go to the home only** (`~/…`): worktrees end with their task; the home persists.
- **Debug and test builds optimize the cipher chain, gzip and yulius-store** (`[profile.dev.package]`
  in `Cargo.toml`): sealing 50 MiB took 10 s unoptimized (the AEAD's generic code is compiled
  into yulius-store, so optimizing only the RustCrypto crates gave 5.6 s), 0.14 s optimized; the
  store's rebuild grows from 3.4 s to 5.9 s. Alternatives: a smaller test state (would not show
  the 50 MB the person needs), a slow 50 MiB test (minutes under CI load).
- **One pull request**: send, restore, save and the rule only work together; the line budget is
  exceeded on purpose, as SPEC-256 did.

## Evidence

All on `feat/257-…` after merging `origin/dev` at 06cf70ea (a clean merge, no conflicts).

- `scripts/dev ci` passed (exit 0) at `f670fcf9`: 2461 tests passed, 17 skipped. It covers the
  repository checks, script tests, `cargo check`, `fmt --check`, Clippy `-D warnings` for Linux
  and `aarch64-apple-darwin`, nextest, doctests, `cargo deny`, `cargo shear` and `cargo doc`.
  Earlier runs failed twice and were fixed: once on a Clippy lint in a test helper, once on
  macOS, where `mode_t` is `u16` (modes are now mapped bit by bit, `PERMISSIONS`). After the
  Vidoc fix, `scripts/dev ci` passed again at `ac8d4802` (exit 0): 2462 tests passed, 17 skipped.
- `scripts/dev security` (Vidoc vidoc-1, change scan, 34 files):
  - At `f670fcf9` it failed on one medium: the CLI put an unchecked state name into a request
    path. `ApiClient::endpoint` already percent-encodes each segment, but the name is now
    checked as a `SecretName` first.
  - The one low finding is fixed too: the suggested `tar` command now quotes `--out`.
  - At `ac8d4802` the scan passed (exit 0). Four medium or low findings are in code this
    branch did not touch: `runtime.rs:859`, `secrets.rs:220`, `secrets.rs:251` and
    `uploads.rs:387`. Yulius CI files each one as an issue.
- Tests. Each new behavior was seen failing first, or checked by a mutation:
  - `remote_mcp::a_laptops_operator_state_moves_into_the_sandbox_with_one_writer` is the
    remote surface, through the real `yulius` binary with only an operate token:
    - `--state QUARTIO_OPERATOR=~/.quartio-operator-20260901` sends a folder with a 50 MiB
      file, nested folders and modes. No terminal is used and no content is printed.
    - A task restores it with `secret_state_restore`, keeping modes 0600 and 0755.
    - A second send is refused with the server's 409 text.
    - A task's `secret_state_save` comes back through `state-get` (mode 0600, never over an
      existing file).
    - A read-only token gets 403 on the download.
    - `--replace-state` overwrites on purpose.
    - Refusals tested: `--to team`, `--replace-state` alone, and a name given twice.
    - Audit actions by the person: `vault.state.import`, `export` and `replace`.
  - `agent_states::a_state_is_restored_once_saved_back_and_never_forked`:
    - The 50 MiB state goes over REST and MCP.
    - A reader is refused.
    - A send before any restore replaces the stored copy.
    - A restore, then a second restore or a restore to another path, is refused (409).
    - A save skips a planted link and names it.
    - The stored row is sealed, not gzip in the clear.
    - The download is `no-store`.
    - After a replace, save is refused until a restore replaces the folder, with no
      leftovers in the home.
    - `secrets_list` names the state.
    - Audit actors: `alice` and `task:1`.
    - Mutation check: with the 409 rule disabled, the test fails (200 instead of 409).
  - `agent_states::unusable_states_and_paths_are_refused` checks exact 422s and messages for:
    - a link, a non-gzip body, a bad name and a description over 500 characters;
    - over 64 MiB, both declared and streamed;
    - restore paths outside `~/`, and a link in the home (nothing written outside);
    - save before any restore, and an unknown state.
  - `agent_states::a_third_state_at_once_is_refused`: 429 with both slots held.
  - Unit tests:
    - `vault::states`: archive checks; the bounds by what is read (pax sizes, data under a
      folder, long names, accumulated slack, 20 001 entries, `./`); modes kept and
      setuid/sticky dropped; packing that leaves links out; placement rules; restores that
      follow no links; a save capped at 64 MiB.
    - Store compare-and-set on the version and the restored copy, and cascade with the
      sandbox.
    - CLI packing, refusals and state names; shell quoting.
    - The remote instructions and onboarding prompt name `--state` and `secret_state_restore`.
- Independent review in a fresh session, three rounds plus confirmations, all findings in this
  change's own code:
  - Round 1 found 1 blocker (archive bounds bypassable through pax sizes and extension
    headers) and 4 majors:
    - writer-rule races: the compare-and-set now covers the restored copy;
    - unbounded memory: access and slot are checked before the body, export and restore take
      slots, records are read without blobs;
    - the save packer: now capped while writing;
    - read tokens could download: now refused.
    All fixed test-first, with 7 minors.
  - Round 2: the per-header allowance accumulated across entries. It is now set, never added
    to, and every entry counts. Also added: an overall 5-minute upload deadline and a 429
    test.
  - Round 3: APPROVE at `f1176ccd`. It was confirmed again at `2c50e341` (Clippy fix),
    `f670fcf9` (macOS modes) and `ac8d4802` (Vidoc fixes).
  - Accepted nits:
    - n3: a restore losing its compare-and-set can remove, in a narrow race, a copy another
      restore just placed at the same path. Nothing unsaved is lost; the next restore puts it
      back.
    - n5: an upload cut by the 5-minute deadline while waiting for a chunk says "stalled".
- Not tested:
  - The real Quartio-Studio/product folders on a Mac. They are private and on the person's
    laptop; the remote test uses the same names, layout and a 50 MiB file.
  - A real Claude Code client following the remote instructions end to end. The steps are
    covered command by command.
  - Server memory under two concurrent 64 MiB states. It is bounded by design (2 slots) and
    the 429 path is tested; it was not measured.
  - Redaction of state content an agent prints on purpose. State files are not secret values.
    The tools never return content and agents are told never to print it. Keys stay agent
    secrets and are redacted as before (ADR-0200 consequences).
