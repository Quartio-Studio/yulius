---
yulius-task: 39de4bf4-ebc4-4330-929b-9a885da7f2ad
status: done
---

# SPEC-407: Triage of the Vidoc findings on the CLI token store, the harness and the Docker smoke

Status: done
Date: 2026-11-03
Owner: Yulius task #389
Scope authorization: task #389 (close #771, #770, #769, #802 and #684, each as a false positive
or as solved; no new issues).

## Goal

Each of the five Vidoc findings is checked against `dev` (d51b98ae). A real one is fixed
test-first. A false positive is dismissed with the guard that stops it, a test that pins that
guard, and a narrow `[[ignore]]` entry (one per wording) in `.config/vidoc.toml`, so Yulius does
not reopen the issue on the next scan.

## Context

The CLI token store (`crates/yulius-import/src/token_store.rs`) keeps `yulius login` and
`yulius remote login` bearer tokens in `~/.config/yulius/credentials.json` (and
`remote-credentials.json` beside it), owner-only. The crate is Unix-only (`states.rs:13` uses
`std::os::unix` unconditionally, `nix` is a plain dependency), so its `#[cfg(not(unix))]` branches
are never built.

- **#770 (size check, then read), real.** `read` (`token_store.rs:190-203`) checks
  `fs::metadata(path).len()` and then calls `fs::read(path)`, two lookups by path. The 1 MiB bound
  holds only for a regular file that does not change in between: a FIFO (length 0) or a file that
  grows after the check is read whole. The file is the person's own, so this is a robustness bound
  that does not hold, not a cross-user attack, but the fix is one handle.
- **#769 (token as a plain `String`), real.** `StoredCredential.token` is a `String`
  (`token_store.rs:46-48`). `save` copies the token into it (`:133`), `load` moves it out
  (`:120`), and every other server's token is a plain `String` that is freed unzeroed when the
  parsed file drops; so are the read bytes and the serialized JSON (`:195`, `:219`). `Secret`
  (`secret.rs:9-11`) promises the value is wiped when dropped, which these copies bypass.
- **#771 (predictable temporary name), partly real.** `write` (`token_store.rs:221-228`) names
  the temporary file `.credentials.json.tmp-<pid>`, removes any file of that name, then creates it
  with `create_new` (`O_CREAT|O_EXCL`, which also refuses a link) and mode `0600`. Inside the
  `0700` directory Yulius creates nobody else can squat that name, and `O_EXCL` with `0600` means
  no other user ever reads a copy. But `create_private_dir` does not tighten a directory that
  already exists, and a squatted name (a directory left there) makes every save fail, since
  `remove_file` cannot remove it. A random name created with `O_EXCL` and `0600` (the `tempfile`
  crate, already a dependency) removes the predictable name and the remove-then-create step. The
  non-Unix part of the finding concerns code that is never built (above). The directory is
  already fsynced after the rename (`:235-238`).
- **#802 (agent `.git` file derives trust paths), false positive.** `main_repository`
  (`crates/yulius-harness/src/claude_config.rs:282-298`) reads the worktree's `.git` through a
  folder opened from the sandbox directory with `O_NOFOLLOW` (`worktree_key`, `:305-336`, from
  #801, task #381) and as a regular file only (`read_small`). The `gitdir:` path is resolved
  lexically (`lexical`, `:341-356`) and nothing below it is opened, not even `commondir`
  (SPEC-253 CFG-005, #634): the host performs no filesystem operation on the agent's path. The
  only effect is one more trust key in the agent's own Claude config (`config_dir`, the agent's
  home, `:54-57`), which the agent can write itself (SPEC-106). A key outside the worktree is the
  point: a linked worktree's main repository lies outside it (`<sandbox>/repo` beside
  `<sandbox>/worktrees/<task>`). Pinned by `the_main_repository_comes_from_the_gitdir_path_alone`
  (`:1045`) and `host_reads_never_follow_a_link_or_wait_on_a_fifo` (`:815`).
- **#684 (smoke PID from a directory name), false positive.** `stale_smokes`
  (`crates/yulius/src/docker_smoke.rs:337-361`) considers only names `docker-smoke-<i32>` under
  `<data>/runtime/sandboxes`. Every real sandbox id is a UUID v4 (`crypto.new_id()`,
  `yulius-store/src/crypto.rs:165-167`, used at `projects.rs:612`, `playground/mod.rs:168` and the
  other creators), so no real sandbox ever has such a name, and `remove_sandbox`
  (`docker_smoke.rs:364-389`) removes only the trees of that smoke name. `runtime/sandboxes`
  itself is written only by the server's service user: a holder gets binds of subfolders of its
  own state dir (`yulius-sandbox/src/engine.rs:134-149`: its sysfs, control and Docker
  folders) and the sandbox directory, never the parent, so creating a `docker-smoke-<n>` entry there
  already needs the service user, who owns every tree the smoke could remove. A reused PID only
  keeps a stale smoke sandbox (fail-safe). `yulius doctor --docker-smoke` is run by the operator.
  Pinned by `only_sandboxes_of_finished_smoke_tests_are_stale` (`docker_smoke.rs:925`).

## Requirements

- TKS-001 (#770). `read` opens the credentials file once, refuses anything that is not a regular
  file, and reads at most `MAX_CREDENTIALS_BYTES + 1` bytes from that handle; more is `Corrupt`.
  It never blocks on a FIFO.
- TKS-002 (#769). The stored token is a `Secret` from parsing to drop: `load` moves it out without
  a copy, `save` clones the `Secret`, and every other server's token is wiped when the parsed file
  drops. The bytes read and the JSON written are zeroized when dropped.
- TKS-003 (#771). `write` creates its temporary file with a random name in the credentials
  directory (`O_EXCL`, `0600`), syncs it, and renames it over the file; a leftover at any fixed
  name cannot block a save, and no temporary file remains after a save or a failed one.
- TKS-004 (#802, #684). One narrow `[[ignore]]` entry per wording, with a reason that names the
  guard and its test.

## Plan

- [x] Spec SPEC-407 with the triage of all five findings
- [x] Credentials read bounded on one handle (#770)
- [x] Stored tokens held in Secret, buffers zeroized (#769)
- [x] Credentials written through a random-name temp file (#771)
- [x] Narrow [[ignore]] entries for #802 and #684
- [x] Gates, docs and changes entry green
- [x] Independent review approved on the final head
- [x] Issues commented and closed or linked to the pull request

## Acceptance criteria

- `token_store::tests::an_oversized_or_special_credentials_file_is_refused` fails on `dev`
  (a FIFO fed more than 1 MiB of valid JSON is accepted) and passes with the fix.
- `token_store::tests::reads_stop_at_the_bound` pins that what is read is bounded, whatever the
  length was when the file was opened: it fails when the length check after the read is removed
  (review round 1).
- `token_store::tests::stored_tokens_are_secrets` does not build on `dev` (the token is a
  `String`) and passes with the fix.
- `token_store::tests::a_squatted_temporary_name_does_not_block_a_save` fails on `dev` (a folder
  at `.credentials.json.tmp-<pid>` makes the save fail) and passes with the fix.
- The existing token store tests (owner-only modes, no leftovers, corrupt files) still pass.
- `.config/vidoc.toml` has two new entries; the scanner's `apply_ignores` marks the two wordings
  ignored and leaves an unrelated title in the same files reported.
- #770, #769 and #771 are linked to the pull request, whose `Fixes` lines close them on merge;
  #802 and #684 are closed as not planned with their evidence.

## Test plan

- Unit tests in `token_store.rs` (above), each seen failing on `dev`'s code first.
- The two new entries are checked with the scanner's own `apply_ignores`
  (`scripts/vidoc_scan.py`) against the two reported wordings and an unrelated title.
- `scripts/dev ci` (the full gate). The security scan is disabled for this project, and is
  reported as disabled, not as passed.

## Decisions

- **#771: `tempfile::NamedTempFile` in the credentials directory, then `persist`.** The
  alternatives were a random suffix from `getrandom` with the same `create_new` code (the same
  result, more code), or keeping the pid name and tightening the directory (still a fixed name a
  leftover can block). `tempfile` is already a dependency of the crate and creates with `O_EXCL`
  and `0600` on Unix, and removes the file on drop when the rename fails.
- **#769: `#[serde(with)]` around `Secret` instead of `Serialize` on `Secret`.** `Secret` has no
  `Serialize` on purpose, so it cannot reach output by accident (`secret.rs:9-11`); the token
  store is the one place that persists it, so the conversion lives there.
- **Residual, accepted.** A save killed between creating and renaming its temporary file (a
  `SIGKILL`, a power loss) leaves that owner-only file, now under a random name nothing reuses.
  `dev`'s code removed such a file only when a later save happened to get the same process id, so
  it was effectively never removed either. A string with JSON escapes passes through
  `serde_json`'s parsing scratch buffer, which is not zeroized; Yulius tokens (`yul_…`) have none.
- **Review round 1 (approve, one low finding, own code): the bound on what is read had no test.**
  The FIFO is refused by its type, and a regular file by its length, before anything is read.
  The bounded read moved into `read_bounded` (any reader), and `reads_stop_at_the_bound` tests it
  with a source over the bound and an endless one. The nits are kept on purpose: the 1 MiB buffer
  is sized once so it never grows and frees no unzeroed copy (zeroing it is one `memset` per CLI
  call), and a folder at the credentials path reads as corrupt, which asks to remove it.
- No ADR: these are local hardening choices inside one module with no lasting cross-cutting
  consequence.

## Evidence

- Each regression test was run on `dev`'s code first (`cargo nextest run -p yulius-import -E
  'test(/token_store::/)'`): `an_oversized_or_special_credentials_file_is_refused` failed with
  `a FIFO: expected Corrupt, got Ok(None)`; `stored_tokens_are_secrets` did not build (E0277, the
  token is a `String`); `a_squatted_temporary_name_does_not_block_a_save` failed with
  `IsADirectory`. With the fixes all 9 token store tests pass and the crate's 114 tests pass.
  `reads_stop_at_the_bound` fails with the final length check replaced by `Ok(Some(bytes))`
  (mutation check, reverted).
- `cargo clippy -p yulius-import --all-targets -- -D warnings`: clean.
- The scanner's own `apply_ignores` (`scripts/vidoc_scan.py`, run from a scratch script) marks
  both reported wordings ignored, and leaves an unrelated title in each file reported.
- `scripts/dev ci` on 082a0e0d (the fixes merged with `dev` ea39cb55): exit 0, 3018 tests passed,
  16 skipped.
- `yulius security-scan`: "Security scan disabled: Vidoc is turned off in Yulius"; nothing was
  scanned (disabled, not passed).
- Independent review (fresh session, read-only): round 1 approved 0315a004 with one low
  own-code finding (the bound on what is read had no test), fixed test-first in 48b88d4d; round 2
  approved 082a0e0d and agreed both dismissals are false positives.
- #802 and #684 commented with their evidence and closed as not planned; #770, #769 and #771
  commented with a link to #1041, whose `Fixes` lines close them on merge.
