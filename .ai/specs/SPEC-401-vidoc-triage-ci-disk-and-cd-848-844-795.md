---
yulius-task: 66ccb5b5-4307-4f29-ba26-738e1f7ea791
status: in-progress
---

# SPEC-401: Vidoc triage of the CI, disk and CD findings (#848 #844 #795 #780 #717)

Status: in-progress
Date: 2026-11-03
Owner: Yulius task #378
Scope authorization: task #378 (a person asked that each of the five issues end closed). Fixes
#844; #848, #795, #780 and #717 are dismissed as false positives.

## Goal

Each of the five Vidoc findings about CI run output, the disk upkeep and CD is checked against
`dev` (88a485ca). Each one is either fixed test-first or dismissed with file:line evidence, a
test that pins the guard, and a narrow `[[ignore]]` entry for its wording, so that a repeat scan
does not reopen it.

## Context

- Every agent run and shell of a sandbox mounts the whole sandbox directory read-write
  (`crates/yulius-ptyhost/src/containment.rs:94`, `:246`). There, a contained agent can swap any
  folder for a link at any moment. ADR-0174 and SPEC-220 decide that the server reaches such
  folders through handles opened with `O_NOFOLLOW`, never by path.
- SPEC-263 (DUL-003) re-checks an ended task's worktree for links just before Git removes it. It
  recorded the window left open: `worktree_remove_if_safe` resolves the path with `canonicalize`,
  and then `git worktree remove` resolves it again and follows a link swapped in after that.
  "Closing it means removing worktrees through handles."
- `leftovers::remove_below` already removes a tree below a trusted root through `O_NOFOLLOW`
  handles (the disk upkeep's caches use it, SPEC-263 DUL-002).
- `discard_worktree` (task deletion) removes a registered worktree with
  `git worktree remove --force --force` and an unregistered one with `remove_dir_all`, both after
  one `canonicalize`.
- CD: stored run output is redacted of every agent secret of the sandbox. Production-run values
  are agent secrets of that sandbox (`vault/env_files.rs:850`).
- `[[ignore]]` matches the path exactly and the title as a case-insensitive substring; there is
  one narrow entry per wording (SPEC-262, ADR-0202).

## Requirements

- VCD-001 (#844, real). A clean removal of a task worktree (`GitPort::remove_worktree`: the disk
  upkeep, the hygiene cleanup, snapshot imports) and a discard (`discard_worktree`: task
  deletion) never follow a link.
  - Git still checks the worktree: inside the owned root, registered, not the main worktree, not
    locked and clean, as before.
  - On Linux the server then removes the worktree folder and its Git record
    (`.git/worktrees/<id>`) itself, each through `O_NOFOLLOW` handles from the parent of its
    folder (the sandbox directory), with `leftovers::remove_below`.
  - A worktree swapped for a link after the checks has only the link removed. Nothing behind the
    link is touched.
  - A worktree whose Git record holds submodules (`modules/`) is refused, as Git refuses it.
  - Off Linux (development hosts, whose runs are not contained) Git removes it as before.
- VCD-002 (#848, false positive). The agent tools `ci_status {run_id}` and `ci_rerun` stay
  scoped to the task's sandbox, not to its task; this is dismissed with a narrow entry.
- VCD-003 (#795, false positive). Declared caches stay a sandbox's own say over its own
  worktree; this is dismissed with a narrow entry.
- VCD-004 (#780, false positive). A deploy confirmation shorter than 7 characters is refused.
  This is pinned by a test and gets a narrow entry.
- VCD-005 (#717, false positive). A production run's output, as an agent reads it with
  `cd_status {run_id}`, is redacted of the production secret's value. This is pinned by a test
  and gets a narrow entry.

## Plan

- [ ] Spec SPEC-401 with the triage of all five findings
- [ ] Worktrees are removed through no-follow handles after Git's checks (#844)
- [ ] Task deletion discards worktrees through no-follow handles (#844)
- [ ] CD guards pinned: short confirmations refused, production output redacted for agents (#780 #717)
- [ ] Narrow [[ignore]] entries for the dismissed wordings (#848 #795 #780 #717)
- [ ] ADR, architecture, changes entry and gates
- [ ] Independent review answered
- [ ] Dismissed issues commented and closed (#848 #795 #780 #717)

## Acceptance criteria

- `disk::an_ended_worktree_swapped_for_a_link_during_its_removal_touches_nothing_behind_it`
  fails on `dev`, where Git deletes the other sandbox's worktree behind the link, and passes with
  the fix.
- A task-deletion test, where the worktree is swapped for a link after the checks, keeps what is
  behind the link. It fails on `dev` and passes with the fix.
- The existing worktree removal tests (`git_ops`, `git_adapter`, `disk::`, `hygiene`) pass
  unchanged, and the Git record of a removed worktree is gone (`git worktree list`).
- `cd::production_is_deployed_by_an_administrator_who_types_the_slug` refuses a 6-character
  `expect` with 409. `cd::production_secrets_need_their_own_allowance` shows an agent's
  `cd_status {run_id}` of the production run without the value.
- `.config/vidoc.toml` has one narrow entry per dismissed wording, each with its reason.
- Each issue ends closed: #844 by the merged pull request, and the others as not planned, each
  with a comment giving the evidence.

## Test plan

- Integration tests in `crates/yulius/tests/disk.rs` (the real Git adapter, a race point between
  Git's checks and the removal) and in the task deletion tests.
- Additions to `crates/yulius/tests/cd.rs`.
- `scripts/dev ci` (the full gate) and `yulius security-scan`.

## Decisions

- #844 is fixed, not dismissed. Git's own removal resolves the path after the server's last
  check, so a contained agent that wins the race makes the host delete a folder of another
  sandbox, or of Yulius, through a link. The fix uses the existing `remove_below` rather than a
  second implementation. Git keeps the checks, which only read. The removal of the folder and of
  the Git record moves to the server, because `git worktree prune` would drop the records of
  worktrees agents keep elsewhere (SPEC-215). Task deletion has the same hole (with `--force`,
  and so without even the cleanliness check), so it gets the same fix. ADR-0255.
- #848: a task is not a confidentiality boundary inside its sandbox.
  - The job logs sit in `<sandbox>/ci/logs`, which every run of the sandbox mounts.
  - `session_read` gives any task the transcripts of every other task of the sandbox
    (`session_docs.rs:1305`).
  - CI jobs get placeholders, never secret values (SPEC-142).
  - `ci_rerun` only queues the same commit again, as the sandbox's people and agents may.
- #795: impact stays inside the sandbox's own directory (#843). The declaring agent can delete
  those folders itself, and the upkeep removes only folders Git ignores that hold no tracked
  file. An allowlist would add no boundary.
- #780: the guard is `!(len >= 7 && starts_with)`, so a short value is refused (409), not let
  through. Production also needs an administrator who types the slug.
- #717: stored output is redacted of every agent secret of the sandbox. That is a superset of the
  values a production run gets, in their raw, hex, base64 and percent forms. Production runs
  verified commits that a person deploys. An agent that could change what production executes
  could send a value anywhere, so hiding the output would add nothing.

## Evidence

(filled in at the end)
