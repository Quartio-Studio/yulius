---
yulius-task: 2de5309f-4a54-4c08-8cd1-3f617a0d266e
status: done
---

# SPEC-215: Leftovers are found and cleaned accurately

Status: done
Date: 2026-10-17
Owner: task #202
Scope authorization: task #202 (requested by MrFishPL): "Make Yulius find and clean every kind of
leftover accurately, never touching anything still in use. It cleans by itself after every update
(and on a schedule). A superadmin can list the leftovers and delete them by hand." Fixes #625 and
#582. Amends PRJ-009 (SPEC-001) and SPEC-008's upkeep of finished tasks. Decision: ADR-0170.

## Goal

Every path Yulius writes under its data directory has an owner record and a rule for when it
becomes garbage. A leftover is a path that no record owns, or whose owner's state says it is
disposable. Yulius finds them, deletes the ones that are safe to delete automatically after every
start (so after every update) and every hour, and lists them all for the superadmin in Settings →
Instance and in `yulius leftovers`, where they can be deleted by hand. Nothing in use is touched:
liveness is checked again right before each deletion. Each deletion is audited with its size and
reason, and each failure is reported.

## Context

The VM's disk is at 81 % (312 of 386 GiB). Measured on the Yulius sandbox on 2026-10-17: tasks
#191–#194 are "running (done)" and their worktrees hold `target/` of 11, 14, 22 and 23 GB (about
70 GB). The removed product sandbox (`6b76ccc7-…`) still has all its files.

Root causes, from the code on `dev` (4e4ce7c2):

- **#625**: `start_disk_upkeep` (`crates/yulius-app/src/disk.rs`) calls `purge_removed_sandbox`
  for each `store.sandboxes()` row with `removed_at`, but that query selects
  `WHERE removed_at IS NULL`. The purge never runs. No store query lists removed sandboxes.
- **#582**: nothing deletes `<data>/sysroots/<sid>/` (the overlay's upper layer and the Docker
  engine's storage). Its files belong to the sandbox's mapped ids, so the service user cannot
  remove them with `remove_dir_all`. Only `yulius_sandbox::base::remove_tree` can, through a
  mapped namespace. SPEC-206 now stops a removed sandbox's holder through the `retired` marker.
- **Runtime state**: `holder::stop` removes only `host.json` and the sockets. The rest of
  `<data>/runtime/sandboxes/<sid>/` stays for good: records, logs, engine directories and the
  `retired` marker. `adopt` reads every one of these directories at each start.
- **Playground purge** (SPEC-171) deletes `sandboxes/<sid>` and the origin, but not `sysroots`,
  `runtime/sandboxes` or `cd`.
- **Done tasks**: `owner_of` (`yulius-domain/src/disk.rs`) counts a running task that idles
  (waiting or done) as "ended" only after `IDLE_PRUNE_AFTER_MS` (4 h). Its caches stay meanwhile,
  and before that only quota pressure (90 %) prunes them. The person continues done tasks rarely.
  Once delivered, a done task's 11–23 GB `target/` is dead weight.
- **Git metadata**: a worktree directory deleted by hand (or by `remove_dir_all`) leaves
  `repo/.git/worktrees/<name>` behind. Git's records of worktrees inside the sandbox directory
  whose directories are gone are removed one by one. `git worktree prune` is not used, because
  it would also drop the records of worktrees agents keep in their own `/tmp`.

### Inventory: every path, its owner and when it becomes garbage

`<data>` is the data directory (default `/var/lib/yulius`). "Auto" means the item is deleted by
the automatic rounds. "Manual" means it is listed and deleted only by a person.

| Path | Owner record | Garbage when | Kind / rule |
| --- | --- | --- | --- |
| `sandboxes/<sid>/repo` (clone) | sandbox | sandbox removed: kept (PRJ-009) | part of `removed-sandbox` (manual) |
| `sandboxes/<sid>/home`, `sessions/`, `attachments/`, `imports/` | sandbox | kept with the sandbox (transcripts, PRJ-009) | part of `removed-sandbox` (manual) |
| `sandboxes/<sid>/worktrees` | sandbox | removed, nothing of it runs | `removed-worktrees` (auto) |
| `sandboxes/<sid>/ci` | sandbox (CI) | removed, no CI run | `removed-ci` (auto) |
| `sandboxes/<sid>/browser` | sandbox (browser) | removed, browser stopped | `removed-browser` (auto) |
| `sysroots/<sid>` | holder | removed, holder stopped | `removed-system` (auto, mapped removal) |
| `runtime/sandboxes/<sid>` | holder | removed, holder stopped | `removed-runtime` (auto, mapped removal) |
| `cd/<sid>` | sandbox (CD) | removed, no CD run live | `removed-cd` (auto) |
| `displays/<sid>` | browser | removed, browser stopped | `removed-display` (auto) |
| `runs/<attempt>` of a removed sandbox's tasks | attempt | removed | `ended-run` (auto) |
| `transcripts/<task>`, `playgrounds/<sid>.git`, `exports/sandbox-<slug>-*.json` | task / playground / removal | kept (PRJ-009) | part of `removed-sandbox` (manual) |
| `sandboxes/<sid>/worktrees/<task>/<cache>` | task | task ended (closed, stopped, failed) after 10 min; **done** after 30 min; waiting 4 h (ADR-0170) | `task-cache` (auto) |
| `sandboxes/<sid>/worktrees/<task>` | task | closed or cancelled and clean (unchanged: `disk.rs`) | existing upkeep |
| `sandboxes/<sid>/worktrees/<name>` with no task record | none | always (may hold uncommitted work) | `orphan-worktree` (manual) |
| `repo/.git/worktrees/<name>` whose worktree, inside the sandbox directory, is gone | git | not locked | `git-metadata` (auto, that record only) |
| `sandboxes/<sid>/ci/jobs/<name>` whose job no run used in 14 days | sandbox (CI) | no CI run in its sandbox | `stale-ci-checkout` (auto, under the CI claim) |
| `sandboxes/<sid>/attachments/drafts/<user>/<file>` | user's draft | older than 7 days (`DRAFT_MAX_AGE`) | `old-draft` (auto) |
| `runs/<attempt>` | attempt record | no attempt record owns it (an ended task's last run holds the final screen), untouched for a day | `ended-run` (auto) |
| `runs/ci-<job>`, `runs/cd-<run>` | CI job / CD run | no active CI run has the job, no running deploy is the run, untouched for a day | `ended-run` (auto) |
| `transcripts/<task>` with no task record | none | always (task deletion failed half-way) | `orphan-transcripts` (auto) |
| `sandboxes/docker-smoke-<pid>`, `sysroots/…`, `runtime/sandboxes/…` | smoke process | its process is gone | `stale-smoke` (auto, mapped removal) |
| `sandboxes/<x>`, `sysroots/<x>`, `runtime/sandboxes/<x>`, `cd/<x>` with no sandbox record | none | always | `unowned` (manual) |
| `system/base-<n>` | instance | not current and no live holder mounts it (ADR-0161) | `unused-base` (auto, mapped removal) |
| `/var/lib/docker`, `/var/lib/containerd` | the host (docker-ce) | Yulius does not use them | `host-docker` (report only) |
| `/var/lib/yulius-update/backups/<id>` | update helper | beyond the last 3 | already pruned by the helper (`KEEP_BACKUPS`); not listed |
| `shells/<id>`, `logins/<id>` | shell / login | ended | already removed at start (`adopt_shells`, `cleanup_stale_logins`) |
| `runs/ci-*`, `runs/cd-*` | job / run | ended | already removed at the job's end and on requeue |
| `yulius.db`, `master.key`, `skills/`, `users/`, `runtime/bin`, `*.json` state | instance | never | not leftovers |

Since SPEC-228 (ADR-0176), every path of this table below `sandboxes/` is removed through the
isolation's mapped removal, walking down from the data directory without links, whatever its kind.

## Requirements

- **LFT-001** A removed sandbox's `worktrees`, `ci`, `browser`, `cd`, `displays` and run
  directories are deleted automatically once it was removed `REMOVED_KEEP` (10 min) ago and
  nothing of it runs: no agent run or stopping task, no shell, no browser, no CI run, no live CD
  run, no process in its namespaces (#625).
- **LFT-002** A removed sandbox's `sysroots/<sid>` and `runtime/sandboxes/<sid>` are deleted
  automatically once its holder is stopped, including files owned by mapped ids. While the holder
  is live they are kept (#582).
- **LFT-003** Everything else of a removed sandbox (clone, home with sessions, attachments,
  transcripts, playground origin, export) stays. It is listed as one manual item, "everything of
  the removed sandbox", which the superadmin can delete after a confirmation naming the size. The
  database records stay (PRJ-009 amended).
- **LFT-004** A partly-cleaned removed sandbox (some directories already deleted by hand) is
  finished without errors. A missing path is not an error. A clone's `.git/worktrees/<name>` goes
  when its worktree lay inside the sandbox directory, is gone, and the record is not locked. Only
  that record goes, so the records of worktrees outside the sandbox directory (an agent's `/tmp`)
  stay.
- **LFT-005** Policy for finished tasks (ADR-0170): a running task whose activity is **done** has
  its rebuildable caches deleted once it has not moved for 30 minutes. Its worktree and
  uncommitted work stay. A waiting task keeps the 4 h rule, and ended tasks keep the 10 min grace.
- **LFT-006** Instance leftovers are found: ended run directories, transcripts of deleted tasks,
  old draft attachments, stale CI checkouts, stale smoke sandboxes, unused bases, directories no
  sandbox record owns, worktrees no task owns, and the host's Docker directories (report only).
- **LFT-007** Each item says its path, kind, size, owner (sandbox, task), why it is a leftover,
  and whether it is safe to delete automatically. Right before deleting an item, Yulius checks its
  rule again against fresh state. An item that is no longer garbage is skipped and reported as
  kept.
- **LFT-008** Each deletion is audited (`leftovers.delete`: path, size, reason, actor). A failed
  deletion is audited (`leftovers.failed`) and logged. Deleting is idempotent: a path already gone
  counts as done. A round can stop half-way and the next one resumes.
- **LFT-009** The automatic round runs at every server start (so after every update) and every
  hour, and deletes only auto items.
- **LFT-010** `GET /api/admin/leftovers` (superadmin) returns the last scan's items, the total
  size and reclaimable size, the disk's usage, and the scan and deletion progress.
  `POST /api/admin/leftovers/scan` rescans in the background. `POST /api/admin/leftovers/delete`
  with `{ids}` or `{all_safe: true}` deletes in the background. Anyone else is refused (403, as the
  other `/api/admin/*` routes); unknown ids are refused (422).
- **LFT-011** Settings → Instance has a **Leftovers** section with the disk usage, the
  reclaimable total, a table of items, Refresh, **Delete** for the selected items and **Delete all
  safe**. Each delete asks for confirmation naming the size. Progress shows while a deletion runs,
  and the list updates when it ends.
- **LFT-012** `yulius leftovers list [--json]` and `yulius leftovers delete (--all-safe | <id>…)
  [--yes]`, run as root or the service user, use the same rules through the running server's
  loopback listener. They are authenticated by a key file in the data directory that only those
  users can read.

## Plan

- [x] Spec with the inventory, and ADR-0170
- [x] A removed sandbox's worktrees, CI, browser and CD directories are deleted once nothing of it runs
- [x] A removed sandbox's system layer and runtime state are deleted once its holder has stopped
- [x] Partly-cleaned sandboxes and stale Git worktree metadata are finished
- [x] A done task's caches are deleted after 30 quiet minutes
- [x] Instance-wide orphans are found and deleted by their rules
- [x] Automatic rounds at every start and every hour
- [x] Superadmin API lists, rescans and deletes in the background with progress
- [x] Leftovers section in Settings → Instance
- [x] yulius leftovers list and delete from the command line
- [x] Operations, architecture, PRJ-009 and changes documented
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green on the reviewed head, spec evidence recorded

## Acceptance criteria

- A sandbox removed more than 10 minutes ago loses its `worktrees` and `ci` in one automatic
  round, with a `leftovers.delete` audit entry for each. A live sandbox's directories are never
  listed as removed-sandbox items.
- A removed sandbox's `sysroots/<sid>` is kept while the isolation reports its holder live. It is
  deleted (through the isolation's mapped removal) once the holder is stopped.
- A done task idle for 31 minutes loses `target/`. One idle for 20 minutes keeps it, and so does a
  waiting task idle for 1 hour. Its worktree stays.
- A removed sandbox whose `worktrees` and `sysroots` were already deleted by hand: the round
  deletes the rest without failures, and `.git/worktrees/<name>` goes. A record of a worktree in an
  agent's own `/tmp` stays.
- A link a sandbox puts in its directory (`attachments/drafts/<user>`, `ci/jobs`) never steers a
  deletion outside it: not when the scan lists items, and not when the link appears after the last
  check.
- A removed sandbox with a live shell, a running CI run or a live holder keeps the guarded paths.
  An item listed as safe but blocked by the time of the deletion (a shell started meanwhile) is
  kept and reported.
- The API lists items for the superadmin and refuses anyone else (403). Delete with ids removes
  exactly those ids. Delete all safe never removes a manual item.
- The Leftovers section shows the list. Refresh rescans. Delete all safe, after its confirmation,
  empties the safe items.
- `yulius leftovers list` prints the items. `yulius leftovers delete --all-safe --yes` deletes
  them. A wrong key or no server is reported with a non-zero exit.

## Test plan

- `yulius-domain` unit tests: `owner_of` for done, waiting and ended activities around their
  thresholds; the kind table (auto or manual, labels).
- `yulius` integration tests (`tests/leftovers.rs`, real store, fake isolation):
  - a removed sandbox's worktrees and ci are purged (#625; failed on dev: nothing was deleted);
  - its sysroot is kept while the fake holder is live and deleted once it is stopped (#582);
  - a partly-cleaned sandbox is finished, and `.git/worktrees/<name>` is pruned;
  - a live shell, a busy CI lane or a live holder blocks deletion;
  - the re-check at delete time: an item becomes live between the scan and the delete and is kept;
  - the API (superadmin lists and deletes; another user gets 404; all safe skips manual items);
  - a done task's cache follows the policy through `disk_upkeep`.
- CLI: `leftovers::the_cli_lists_and_deletes_with_the_server_s_key` runs `leftovers_cli::run_async`
  against the internal router on a loopback port through the key file, plus a refused
  confirmation, a wrong key and no server.
- Schedule: `leftovers::the_round_runs_at_start_and_every_period` (periods shortened).
- `yulius-sandbox` manager: holder liveness (an ended holder counts while processes use its
  namespaces), mapped removal only under the system trees, unused bases.
- e2e: `e2e/tests/leftovers.e2e.ts`: the card lists the mock items with the disk; Delete all safe
  and Delete of a selected item confirm with the size and follow the deletion; an empty scan;
  members never see it; no WCAG violations.
- Measurement on this sandbox: sizes of the done tasks' `target/` before the policy (Context).

## Decisions

See ADR-0170 for the lasting ones. Summary:

- **One leftovers engine, rules per kind.** One module scans and deletes by kind, and the disk
  upkeep keeps its quota-pressure role. Each kind's rule is evaluated at scan time and again at
  delete time.
- **A removed sandbox's disposable parts go after 10 minutes, not a day.** The removal stops
  everything within about a minute (SPEC-206 retires the holder after 60 s), and the liveness
  checks guard the rest. A day only delayed the space (the product sandbox would keep its 100+ GB
  until tomorrow). Nothing in these parts can be restored: re-adding the repository makes a new
  sandbox.
- **Runs at every start, not only on a new version.** A start is when an update lands, including
  a hand-copied binary that leaves no update record. Running at each restart costs one scan.
  **Hourly** afterwards: space comes back within the hour, while a scan walks hundreds of GB and
  should not run every 5 minutes like the quota upkeep.
- **Done tasks' caches after 30 minutes.** "Done" means Claude Code recorded the goal as achieved,
  and the person rarely continues such a task within half an hour. Continuing rebuilds the caches
  (minutes), while keeping them costs 11–23 GB per task. The worktree stays, so uncommitted work
  and the branch are safe.
- **CLI through the loopback listener, authenticated by a key file.** The rules need the server's
  live state (shells, CI lanes, holders), so the CLI cannot work on the files by itself. The key
  is written by the server at start to `<data>/leftovers-cli.key` (mode 0600). Root and the
  service user can read it; sandboxed runs cannot (their data directory is an empty tmpfs).
- **Delete everything of a removed sandbox: files only.** Its records (tasks, audit) stay, so
  history and the analytics keep working.
- **One pull request.** The pieces share one engine and one rule table. Splitting them would ship
  a list that cannot delete, or deletions nobody can see.

## Evidence

Reviewed head `a1ac1831` (task branch `feat/202-leftovers-are-found-and-cleaned-accurate`, on
`dev` 9fb8c539).

- **Measured on the Yulius sandbox** (2026-10-17, before the change): the worktrees of tasks
  #191–#194 ("running (done)") held `target/` of 11, 14, 22 and 23 GB. Here `/var/lib/yulius` is
  a sandbox's tmpfs view, so the VM's own disk and the removed product sandbox cannot be measured
  from the task.
- **Fails before, passes after** (each checked by reverting the fix locally):
  - `disk::a_done_task_loses_its_caches_after_half_an_hour_and_keeps_its_worktree`: with the old
    4 h rule, "31 quiet minutes: a done task's caches go (DiskReport { freed: 0, caches: 0, … })".
  - `leftovers::a_removed_sandbox_loses_its_disposable_parts_and_keeps_what_prj_009_keeps` (#625):
    with dev's `WHERE removed_at IS NULL` filter, "worktrees of the removed sandbox is gone"
    fails.
  - `leftovers::the_system_layer_waits_for_the_holder_to_stop` (#582): with the holder check
    disabled, "kept while its holder runs" fails.
  - Review round 1: `a_link_in_a_sandbox_never_steers_a_deletion_outside_it` (failed with 11
    deletions, `master.key` among them), `git_records_of_worktrees_outside_the_sandbox_are_kept`
    and `an_existing_task_keeps_its_run_directories` failed before their fixes.
  - Review round 2: `leftovers::tests::the_removal_never_goes_through_a_link_on_the_way`
    (yulius-app) fails with `O_NOFOLLOW` removed.
- **Leftovers suite:** 15 integration tests in `crates/yulius/tests/leftovers.rs` (#625, #582,
  partly cleaned, blocked by a CI run or a live holder, the re-check at deletion, instance
  leftovers, the API, the schedule, the CLI, the loopback key, links, Git records, run
  directories, CI checkouts, a cache whose task works again), 2 yulius-app unit tests, 4 new
  yulius-sandbox manager tests and 4 yulius-domain tests: all pass.
- **`scripts/dev ci` at `a1ac1831`:** exit 0. 2161 tests passed, 18 skipped; lint, format, docs,
  deny, shear, and the macOS cross-check (ADR-0169) all passed. An earlier run at `ee6e8cf6` had
  one failure in an untouched ptyhost test
  (`shutdown_escalates_past_ignored_signals_and_reaps_descendants`, `ParseIntError` under load).
  It passed 5 of 5 when rerun alone and is filed as #647.
- **Security** (`scripts/vidoc_scan.py --base origin/dev`, run directly beside the gate, as
  `scripts/dev security` does): exit 0, no open critical or high finding at `a1ac1831`.
  - The first scan (at `68e3dac7`) raised 3 medium findings about the CLI key: a staged copy left
    on a failed write, a length-leaking comparison, and an empty key. All three were fixed in
    `ee6e8cf6`: no staged copy is left, the comparison runs over digests, and an empty key never
    matches.
  - Its claim that the delete selection is not validated is a false positive: ids resolve against
    the server's own last scan, and every path is checked again.
  - The last scan's notes on `base.rs`, `manager.rs` and `purge.rs` concern existing code. Its note
    on the route's key comparison is a false positive: the comparison happens in the app, over
    SHA-256 digests in constant time.
- **Yulius CI on PR #651 at `277fdfd7`:** the security job failed with one high finding: "Path
  traversal via sandbox id in retire/state_dir" (`yulius-sandbox` manager). The code is SPEC-206's,
  in a file this change touches. The server's ids were already restricted, but the manager now
  defends itself: a name outside the ids' alphabet maps to the fixed component `.invalid` in the
  state, system, sandbox and cgroup paths, and `retire` and `holder_live` refuse it. Fixed in
  `b29820f9` with `a_name_outside_the_id_alphabet_never_leaves_its_tree` (failed before, passes;
  109 yulius-sandbox tests pass). The local Vidoc rerun at `b29820f9` finds no critical or high
  finding.
- **e2e:** the full suite at `a1ac1831` (`cd e2e && npx e2e run`): 61 files, 615 tests passed and 1
  skipped, exit 0. That includes `tests/leftovers.e2e.ts` (4) and `tests/sandbox-docker.e2e.ts`
  (6).
- **Independent review:** four rounds in a fresh read-only session. Approved at `a1ac1831` with no
  blocker or major left.
  - Fixed along the way: deletions that a sandbox's link could steer (at the scan, at the guard,
    at the removal itself, and in the permission fallback); `git worktree prune` dropping the
    records of `/tmp` worktrees; the run directories that hold an ended task's final screen; tests
    for the CI checkout and re-check rules; the job slot after a stop; the docs.
  - Filed: #649 (the disk upkeep's existing variant of the link problem).
  - Kept as noted: a directory its owner cannot even read (mode 0o000) is not made removable. Its
    removal fails and is audited as `leftovers.failed`, which fails safe.
- **Not testable here:** the real mapped removal of a holder's files (needs the contained service
  user and `bwrap`; the opt-in isolation suite). On the VM after promotion and update, the operator
  checks the steps in docs/operations.md, "Upgrading to leftovers cleanup": the Leftovers view
  before and after, `df -h /var/lib/yulius`, and that the product sandbox's leftovers are gone.
  Nothing was deleted on the live host from this task.
