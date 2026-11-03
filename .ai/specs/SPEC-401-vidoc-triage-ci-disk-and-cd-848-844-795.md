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
  - A worktree holding submodules (`modules/` in its record, or a gitlink in its index whose
    folder holds a `.git`) is refused, as Git refuses it. So is a worktree whose `.git` names a
    record that does not name it back (its `gitdir`).
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

- `git_adapter::a_worktree_swapped_for_a_link_during_its_removal_touches_nothing_behind_it`
  (clean removal) and `git_adapter::a_discarded_worktree_swapped_for_a_link_touches_nothing_behind_it`
  (task deletion) fail with dev's removal (Git deletes the folder behind the link) and pass with
  the fix. Each asserts that its swap really ran.
- `git_ops::removal_below_hands_over_only_verified_worktrees` shows that each refusal (submodules
  either way, a record that does not name the worktree back, an owned root resolving outside its
  parent) hands nothing to the removal, and a clean worktree hands over its folder, then its
  record.
- The existing worktree removal tests (`git_ops`, `git_adapter`, `disk::`, `hygiene`) pass
  unchanged, and the Git record of a removed worktree is gone (`git worktree list`).
- `cd::production_is_deployed_by_an_administrator_who_types_the_slug` refuses a 6-character
  `expect` with 409. `cd::production_secrets_need_their_own_allowance` shows an agent's
  `cd_status {run_id}` of the production run without the value.
- `.config/vidoc.toml` has one narrow entry per dismissed wording, each with its reason.
- Each issue ends closed: #844 by the merged pull request, and the others as not planned, each
  with a comment giving the evidence.

## Test plan

- Integration tests in `crates/yulius/tests/git_adapter.rs`: the real Git adapter, with the race
  point `WorktreeRemove` between Git's checks and the removal; `disk::`, hygiene and imports
  tests keep passing unchanged.
- `crates/yulius-git/tests/git_ops.rs` for the refusals of `worktree_remove_if_safe_below`.
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

- Triage on `dev` 88a485ca (the file:line evidence is in each issue's closing comment):
  - #848: `tool_ci_status`/`ci_rerun` are scoped to the sandbox (`ci/tools.rs:65-75`,
    `ci/mod.rs:1335-1342`). Job logs are in `<sandbox>/ci/logs` (`ci/exec.rs:1286-1296`), which
    every run mounts (`containment.rs:94`, `:246`). `session_read` serves any task of the sandbox
    (`session_docs.rs:1294-1320`). CI jobs get placeholders (`ci/exec.rs:862`, `:925`).
  - #795: `check_repo_path` keeps declarations inside the worktree (#843); `find_caches`
    (`disk.rs:352-395`) and `remove_cache` (`disk.rs:138-152`) act only on ignored, untracked
    real folders, through handles.
  - #780: `ci/cd.rs:991-992` refuses any non-empty confirmation under 7 characters (409).
  - #717: `ci/cd.rs:1288-1289` (and the live tail, `ci/exec.rs:1214-1216`) redact every agent
    secret of the sandbox (`vault/mod.rs:402-415`, `:458-495`; forms in `vault/encode.rs:94`).
    Production's values are such secrets (`vault/env_files.rs:850-855`).
  - #844: real; SPEC-263 had recorded the window as out of scope.
- Regression tests, seen failing on dev-equivalent code (Git's removal, with the pause point
  after the checks): both `git_adapter::` swap tests failed with the victim's folder deleted
  ("the folder behind the link was deleted (... failed to delete '.../worktrees/t1': Not a
  directory)"). They pass with the fix. Mutation checks after review round 1: both fail when the
  adapter is put back to Git's removal (the swap never runs), and
  `removal_below_hands_over_only_verified_worktrees` fails without the gitlink check or the
  `gitdir` back-reference.
- The #780 pin fails when the length check is removed
  (`cd::production_is_deployed_by_an_administrator_who_types_the_slug`).
- `[[ignore]]` entries are title-only with the exact path, one per wording, like most entries in
  `.config/vidoc.toml`. The `function` field is optional and left out, as in all but one of the
  existing entries. The independent reviewer agreed to all four: `ci/tools.rs` "Task-scoped agent can read
  any run's output in the sandbox"; `disk.rs` "Attacker-controlled cache declarations steer host
  deletions"; `ci/cd.rs` "Weak deploy confirmation check" and "Agent can read production run
  output (potential secret disclosure)".
- `scripts/dev ci` at bd7390fc: exit 0, nextest 2990 passed, 16 skipped. The first run failed
  macOS clippy (Linux-only test helpers unused there), which was fixed by `cfg(target_os = "linux")`.
- `yulius security-scan`: "Security scan disabled: Vidoc is turned off in Yulius" (exit 0). This
  is disabled, not passed; nothing was scanned.
- Independent review, round 1 (fresh session, bd7390fc): request changes. It found no remaining
  delete through a link and agreed to the four dismissals. Findings answered:
  - The swap tests did not fail without the fix; they now assert that the race ran.
  - The refusal branches had no tests; `git_ops` now covers them.
  - Git also refuses populated gitlinks; now refused too.
  - The record is checked to name the worktree back.
  - Stale comments and the hygiene refusal message were updated.
  - ADR-0255's context was inaccurate: a discard needs the race too.
  - The docs wrongly said "sandbox directory" for imports.
  - Spec test names were wrong.
  - The #717 title was widened to Vidoc's full wording.
