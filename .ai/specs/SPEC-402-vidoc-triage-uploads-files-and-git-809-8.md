---
yulius-task: a744e168-9f13-4bf5-a042-95b633ed9f66
status: in-progress
---

# SPEC-402: Vidoc triage of the uploads, files and Git findings (#809 #808 #800 #811 #810)

Status: in-progress
Date: 2026-11-03
Owner: Yulius task #385
Scope authorization: task #385 (a person asked that each of the five issues end closed). Fixes
#810, #808 and #800; #809 and #811 are dismissed as false positives.

## Goal

Each of the five Vidoc findings about uploads, the files view and the server's Git is checked
against `dev` (ffbc2e91). Each one is either fixed test-first or dismissed with file:line
evidence and a narrow `[[ignore]]` entry for its wording, so that a repeat scan does not reopen
it.

## Context

- Every agent run and shell of a sandbox mounts the whole sandbox directory read-write
  (`crates/yulius-ptyhost/src/containment.rs:94`). A contained agent can therefore change any
  entry below it at any moment, the sandbox's clones' `.git` included (SPEC-220, ADR-0174).
- Contained runs have their own user and PID namespaces and write as sub-uids
  (`docs/architecture.md`, "Leftovers"); the server's Git runs as the service user, outside
  containment, with a cleared environment and fixed `-c` hardening: no hooks, no fsmonitor, no
  `ext::` (`crates/yulius-git/src/runner.rs:51-62`, `:242`).
- `scrub_transport_config` (`crates/yulius-git/src/repo.rs:238`) checks with `symlink_metadata`
  that `.git` is a directory and that `config` / `config.worktree` are not links, then runs
  `git config --list` and `git config --unset-all` by path. Git writes configuration through
  `<file>.lock` and a rename, and resolves a link at `<file>` first, so a link swapped in after
  the check sends the write to the file it points at. Callers: `playground/github.rs:295` (the
  sandbox's own clone) and `remote_files.rs:411`.
- `link_new_in` (`crates/yulius-app/src/uploads.rs:397`) places a file with `linkat`, and where
  that fails for another reason than `EEXIST` it checks with `fstatat` and then `renameat`s,
  which replaces a name created in between (documented "not atomic against a racing writer").
  Uploads (`Placement::Create`, `FreeName`) and the attachments trash use it.
- `open_dir_at` (`crates/yulius-app/src/files.rs:196`) and `file_create` (`files.rs:863`) create
  folders with `mkdirat(…, 0o777)`, so the mode is whatever the process umask leaves. The unit
  `host_setup` writes sets no `UMask=` (systemd's default 0022, giving 0755), but a server
  started under a permissive umask would create world-writable folders.
- `[[ignore]]` matches the path exactly and the title as a case-insensitive substring; there is
  one narrow entry per wording (SPEC-262, ADR-0202).

## Requirements

- VUG-001 (#810, real). The configuration scrub never writes through a link.
  - `.git`, `config` and `config.worktree` are opened through handles with `O_NOFOLLOW` (the
    repository folder itself too); a link or a non-regular file there is refused before
    anything is written, and opening never blocks (a FIFO is refused).
  - Each file is scrubbed as Git's own writers do: `<file>.lock` is created exclusively in the
    `.git` handle (a lock or link already there refuses), the file is read through its handle,
    Git lists and unsets the keys on a private copy (a 0700 folder of the service user, never in
    the repository), and the scrubbed bytes are renamed over the name through the `.git` handle
    with the original file's mode. A name swapped for a link meanwhile has only the link
    replaced; nothing it points at is read or written.
  - A file with nothing to remove is left as it is; the key list is unchanged.
- VUG-002 (#808, real). Placing a file under a new name never replaces an entry.
  - When `linkat` fails for another reason than `EEXIST`, the fallback is
    `renameat2(RENAME_NOREPLACE)`: `EEXIST` answers "not placed", as the link does.
  - Where `renameat2` or the flag is unavailable (another libc or OS, a file system without
    it), the placement fails with the error; there is no check-then-rename any more.
- VUG-003 (#800, real). Folders the server creates in a sandbox tree are 0755 at most, whatever
  the umask: `mkdirat` asks for 0o755 (umask can only take bits away). Under the default umask
  the result is unchanged.
- VUG-004 (#809, false positive). A draft whose mtime lies before 1970 is older than a week, so
  pruning it is the intended outcome, not a coerced one; the drafts folder is the sandbox's own,
  which its agents can already change. Dismissed with a narrow entry; a test pins it.
- VUG-005 (#811, false positive). The token in the one Git command's `GIT_CONFIG_VALUE_n` is
  readable only by the service user or root (`/proc/<pid>/environ` needs the same uid and ptrace
  access); contained agents are other uids in other PID namespaces. Git's children are its own
  helpers (hooks and fsmonitor are off). A configuration-driven command in an agent-writable
  repository is ADR-0114's recorded #54 part B, which a person set aside; a credential helper or
  askpass would hand such a child the token just the same (`git credential fill`). Dismissed with
  a narrow entry.

## Plan

- [x] Spec SPEC-402 with the triage of all five findings
- [ ] Configuration scrub through no-follow handles and Git's lock (#810)
- [ ] Atomic no-replace fallback for new names (#808)
- [ ] Folders created 0755 whatever the umask (#800)
- [ ] Old-draft pin and narrow [[ignore]] entries (#809 #811)
- [ ] ADR, architecture, changes entry and gates
- [ ] Independent review answered
- [ ] Dismissed issues commented and closed (#809 #811)

## Acceptance criteria

- A `git` that swaps `.git/config` for a link to an outside file at its first `config` call
  leaves that file untouched and the repository's `config` scrubbed (fails on `dev`: the outside
  file loses its `http.proxy`).
- A FIFO `config.worktree` is refused at once; existing scrub tests still pass.
- A name taken between the failed link and the fallback placement is kept, the call answers
  "not placed" (fails on `dev`: the entry is replaced).
- `open_dir_at` under umask 0 creates a 0755 folder (fails on `dev`: 0777).
- A pre-1970 draft is pruned by the owner's next upload; a fresh one stays.
- `scripts/dev ci` passes; `yulius security-scan` is reported as what it says.

## Test plan

- `yulius-git` `git_ops`: `a_clone_configuration_swapped_for_a_link_during_the_scrub_is_never_written_through`
  (a wrapper `git` swaps the file at its first `config` call),
  `a_clone_configuration_that_is_not_a_file_is_refused_without_blocking`.
- `yulius-app` unit tests: the fallback placement with a name taken at a test-only pause point
  (`uploads::tests`), and `open_dir_at` on a thread with its own umask 0 (`files::tests`, Linux).
- `yulius` `uploads`: `a_draft_dated_before_1970_is_old`.

## Decisions

- Scrub a private copy and rename it back through the `.git` handle (ADR-0257) rather than parse
  Git's configuration format in Rust: Git keeps deciding what a key is, and only the place it
  writes changes. `<file>.lock` as the temporary file is Git's own lock, so a concurrent Git
  writer and the scrub exclude each other.
- No non-atomic fallback for new names (ADR-0257): failing a placement on an exotic file system
  is better than replacing an entry the person did not ask to replace.
- 0o755 rather than 0o700 for created folders: it is what production creates today; 0o700 could
  hide folders from whoever reads the sandbox tree as another uid.
