---
yulius-task: 5a254bf5-8315-4e31-862b-e339e88e8434
status: done
---

# SPEC-591: Yulius gets its own staging instance and merge-first delivery

## Goal

Give Yulius's own project a shared staging instance on the former production host and turn on
merge-first delivery for it ([SPEC-575](SPEC-575-make-task-local-testing-and-one-staging.md),
[ADR-0410](../../docs/decisions/0410-the-full-suite-runs-on-the-merged-revision-beside-a-merge-first-staging-target.md),
[ADR-0411](../../docs/decisions/0411-staging-changes-through-a-queue-of-tested-merged-revisions.md)):
a pull request merges on its quick checks, the full suite runs once on the merged revision, and
staging, and through it production, changes only to a revision that passed the full suite and
was verified there.

## Context

- [SPEC-579](SPEC-579-adopt-merge-first-staging-infra-bootstra.md) (task #563) kept the full suite
  on every pull request because `[cd.dev]` had no machine. The owner has now provided one.
- Verified on 2026-11-21 over SSH with the pinned key (not assumed):
  - The VM is `deciding-whale` (Ubuntu 26.04.1 LTS, x86_64, 64 CPUs, 755 GiB, 3.5 TB md mirror at
    `/var/lib/yulius`, glibc 2.43).
  - Its host key is SHA256:jyEISzZSlZiCwSSGcLiha5+M64caekYVrWwvO4LmRAQ.
  - `yulius.service` runs `/usr/local/bin/yulius` 3fc0e5976ae2ce2da68a81c400866e2669316f66 as
    `yulius`. `/internal/version` on 127.0.0.1:8081 answers that commit, and
    https://185-189-47-160.sslip.io/healthz answers 200.
  - The contained doctor passes. Its "sandbox Docker" line is ok (one engine per sandbox).
  - `yulius-sandboxes.service` is active and `yulius-update.path` is disabled. The data
    directory holds a fresh store (no users), no `/var/lib/yulius-releases`, `python3` with
    SQLite 3.46.1, no `sqlite3` CLI.
  - The agent key STAGING_VM_SSH_KEY has fingerprint SHA256:rTV57147ZCrszF9UXKCRUxO5yX+5P+4ilBFpsE6w9FI
    and may be copied to `.env` files, so dev CD runs get it. STAGING_VM_HOST is a project
    variable, and dev runs get every variable.
- The shape to follow is autoresearch's `scripts/cd` (Odyseuss, a fork of Yulius):
  - an ssh-agent-held key and a host key pinned in the repository;
  - a Docker build in CD's builder and the binary uploaded by SHA-256;
  - the last 10 releases kept on the VM;
  - a root script piped over SSH.
  Yulius differs: it has no `host update`/`host rollback` commands, so the install, the store
  snapshot and the restore are CD's own.
- Yulius's update helper (§8 of operations) builds only commits on `main`, never older than its
  last install, with a GitHub credential the staging instance does not have.
- The product already refuses every promotion of a merge-first project unless staging verified
  that revision through its queue (ADR-0411, MFS-009: `cd_prepare`, the Delivery page's Promote,
  automatic promotion). The queue verifies only revisions whose full gate passed.
- Two defects of the move were split into their own tasks, since they touch other code (the move
  helper, the browser) and need their own tests:
  - #576: Caddy keeps its pre-move configuration.
  - #577: stale Chrome Singleton locks.
  Staging's verify checks the public HTTPS answer, so a Caddy left on port 80 fails a deploy.

## Requirements

- **STG-001 Pinned SSH.** CD reaches the VM only as `root@$STAGING_VM_HOST` with
  `StrictHostKeyChecking=yes` against the key pinned in `scripts/cd/known_hosts`
  (alias `yulius-staging`), with STAGING_VM_SSH_KEY held by an ssh-agent the script starts and
  stops. The key is never written to disk. A missing secret or variable, a malformed host or a
  missing pin refuses with what to do. Nothing depends on the VM's address or size beyond the
  variable and the pin.
- **STG-002 One build per revision, in CD's builder.** `deploy` builds `$YULIUS_CD_SHA`:
  - with `deploy/build/Dockerfile` on Ubuntu 24.04, `cargo build --release --locked -p yulius` and
    `YULIUS_BUILD_COMMIT` set, the toolchain of `rust-toolchain.toml`;
  - checks that `yulius version` names that commit;
  - keeps the binary in `$YULIUS_CD_CACHE` (last 10).
  A rollback never builds. The VM's update helper is not used (ADR-0430).
- **STG-003 Deploy installs exactly that release and restarts only yulius.service.**
  - The binary is uploaded and checked by SHA-256 into `/var/lib/yulius-releases/<sha>/`.
  - The deploy stops `yulius.service` and keeps the replaced binary and a snapshot of its store
    (the database and its WAL, as the previous release left it) under that release.
  - It then installs, starts and waits until `/internal/version` reports the commit.
  - If that never happens, it puts the previous binary and store back and fails.
  - It refuses while the update helper runs and prints `::yulius-release <sha>`.
  - A second deploy of the running release changes nothing. The last 10 releases are kept.
- **STG-004 Verify proves the release, not only health.**
  - The recorded release is the expected one (the deploy's commit or the rollback's release), the
    installed binary is that release's, and `/internal/version` reports it.
  - The public URL of the unit answers `/healthz` 200 over HTTPS.
  - The contained doctor, run as the service user with the unit's environment, passes.
  - The live store's schema is the migrations count of the deployed tree (deploys).
  - Before anything stops, the release's migrations run on a consistent copy of the live store
    (`yulius store-migrate`). No table may vanish, and none may lose rows but those a migration
    deletes from by design. Verify finds that check kept and the live store sound, with the
    release's schema.
  - On a deploy, the sandbox Docker smoke test (`yulius doctor --docker-smoke`) passes when the
    doctor offers sandboxes Docker. It does not apply otherwise and says so.
- **STG-005 Rollback restores the release and its store.** `rollback` puts back
  `$YULIUS_CD_RELEASE`:
  - The binary comes from the VM's kept releases, else from the CD cache.
  - Its own store snapshot is restored when the live store's schema differs (a newer release
    migrated it). With the same schema the live store is kept.
  - The store being left is snapshotted first, so the drill's step forward restores it.
  - It refuses without a release, and its dry run (`YULIUS_CD_DRY_RUN=1`) changes nothing.
  - Yulius drills it after each dev deploy (back, verify, forward, verify).
- **STG-006 `[cd.dev]` filled.** `deploy`, `verify`, `rollback`, `migrations` and `secrets` are set,
  and `about` names the VM's role, not its address. The gate tests the scripts: syntax,
  refusals, dry run, and the VM steps against a fake root.
- **STG-007 Merge-first.**
  - `[cd.dev] merge_first = true`.
  - `after_merge = true` on the two `test` shards and on `e2e`.
  - `quality` and `security` stay on pull requests, justified by the CI numbers in
    [ADR-0436](../../docs/decisions/0436-yuliuss-full-suite-runs-after-the-merge-and-promotion-comes-from-verified-staging.md),
    which also says what a pull request still proves before it merges.
- **STG-008 Promotion rule explicit and enforced.**
  - Only a revision that passed the full suite on dev and that staging verified may be promoted
    to `main`.
  - A red dev is fixed forward, and promotion is refused until staging verified a fixed revision.
  - The product's refusal is tested against Yulius's own `yulius.toml`.
- **STG-009 Documentation.** Same change: the operations section on the staging VM (prepare, swap,
  who deploys, the update helper staying off), the architecture, AGENTS.md's delivery section,
  ADRs and `docs/changes.md`.

## Plan

- [x] Write and validate SPEC-591 with the plan of both slices
- [x] Pin the staging host key and add CD's SSH library with refusal tests
- [x] Build the exact revision in CD's builder and keep the last 10 releases
- [x] Install a kept release with its store snapshot, restarting only yulius.service
- [x] Verify the commit, public HTTPS, contained doctor, migration without loss and Docker smoke
- [x] Roll back to a kept release and its store, with dry run and refusals tested
- [x] Run the scripts against the staging VM by hand and restore 3fc0e5976
- [x] Fill [cd.dev], ADR-0430 and the staging VM's operations, architecture and AGENTS docs
- [x] Answer review round 1 of the CD slice test-first and rerun on the VM
- [x] Independent review of the CD slice approved
- [x] First staging deploy through CD recorded
- [x] Answer review round 3's minor and nits test-first
- [x] Move the test shards and e2e to the merged revision with merge_first and an ADR
- [x] Make the promotion rule explicit and test it against Yulius's own yulius.toml
- [x] Evidence, changes entry and independent review of the merge-first slice approved
- [x] Record the first merge-first staging request, its deploy and rollback drill in a follow-up pull request

## Acceptance criteria

- **AC-1 (STG-001):** `scripts/test_cd.py` shows that each script refuses in these cases:
  - without its secret or variable;
  - with a host that is not `[user@]host[:port]`;
  - without the pin.
  The SSH command carries `StrictHostKeyChecking=yes`, its own known-hosts file and the alias.
- **AC-2 (STG-003, STG-005):** On a fake root, the test shows:
  - A deploy keeps the replaced binary and store, installs the new one and records it.
  - A release that never reports its commit is undone: the previous binary and the original store
    bytes are back.
  - A redeploy of the current release changes nothing.
  - A rollback across a schema change restores the release's snapshot, and one without keeps
    the live rows.
  - The drill's forward step restores the store the newer release left.
- **AC-3 (STG-004):** On a fake root:
  - A deploy whose migration loses rows or drops a table is refused before anything stops, but
    rows of a table that a migration the deploy applies deletes from by design may go.
  - Verify fails in each of these cases:
    - a wrong recorded release;
    - a binary that is not the release;
    - another commit at `/internal/version`;
    - a failing doctor;
    - HTTPS down;
    - a failing smoke;
    - a schema other than the tree's after a deploy, or other than the one the release left
      after a rollback;
    - no kept migration check.
  - Verify passes otherwise, also after rows were deleted since the start.
  - The smoke runs on a deploy's verify only when the doctor offers Docker.
- **AC-4 (STG-002, real):** A CD run of `[cd.dev]` deploys dev's head to the staging VM. Its
  output has the build, `::yulius-release <sha>` and verify's lines, and
  `/internal/version` on the VM reports that commit. A later deploy's rollback drill passes (back
  to the previous release and its store, forward again). The spec's Evidence names both runs,
  their revisions and request ids.
- **AC-6 (review round 3):** On a fake root:
  - A put back whose store cannot be restored leaves the server stopped, the snapshot intact,
    with the words "restore by hand". This holds both in recover and in a deploy's undo.
  - The deletion parser ignores block comments and `CASE … END` inside a trigger.
  - Every pair of the deletion list is validated on the VM.
- **AC-5 (STG-007, STG-008):** `the_repositorys_own_cd_is_defined` asserts:
  - `[cd.dev]` is the merge-first target with deploy, verify, rollback and migrations;
  - `test` and `e2e` are `after_merge`, and `quality` and `security` are not.
  A test with Yulius's own `yulius.toml` shows a promotion refused while staging has not
  verified the integration head.

## Test plan

- `python3 -m unittest scripts/test_cd.py`, run by `quality` with the checker's tests. It covers:
  - every script's refusals and the rollback's dry run;
  - `sh -n` of each script;
  - `remote.sh` steps on a temporary root with stubbed `systemctl`, `runuser`, `curl` and `id`;
  - a fake `yulius` whose start migrates a real SQLite file;
  - a failing case for each verify check.
- Rust: `the_repositorys_own_cd_is_defined` (yulius-domain), the staging promotion test in
  `crates/yulius/tests/staging.rs` with Yulius's own `yulius.toml`.
- Real: the scripts by hand against the staging VM (deploy of this branch's build, verify,
  rollback to 3fc0e5976 with its store), then the first CD run from dev and the drill.

## Decisions

- **Build in CD's builder, not through the VM's update helper**
  ([ADR-0430](../../docs/decisions/0430-yuliuss-staging-builds-in-cds-builder-and-installs-a-kept-release-with-its-store.md)).
  The helper builds only `main` and never an older commit (staging serves dev, and a rollback
  goes back). It needs a GitHub credential on the VM, which the unset-up staging instance lacks.
  It would rebuild on every rollback and tie build time to a VM that will be replaced by a
  smaller one. The builder makes one artifact per revision, checked by SHA-256, and the drill
  redeploys the same bytes.
- **CD installs, not `host update`:** Yulius has no install command apart from the helper's
  request file, which only the server may write. The VM script repeats the helper's install
  contract (stop, back up the binary and store, install, start, wait for the commit, restore
  on failure) with release-indexed snapshots, so a rollback can find a release's own store.
- **A rollback restores a store only across a schema change:** "restores the previous release
  and its store" without losing staging data when nothing needs it. Across a schema change, what
  the newer release wrote since its deploy is lost on staging. That is staging's accepted cost
  (a person's production keeps the helper's own rollback).
- **No-loss check on a migrated copy before the stop (review round 1):**
  - The new binary migrates a copy of the live store (SQLite's backup) with
    `yulius store-migrate`, so nothing the server does after its start (retention, a person's
    delete) reads as loss, and a lossy migration refuses the deploy before staging changes.
  - Only tables that a migration the deploy applies deletes from by design may shrink (its own
    `DELETE FROM`, not a trigger's; `scripts/cd/migration_deletes.py`, review round 2).
  - It names the table that lost rows or vanished.
  - On a fresh instance it proves little until a person uses staging. That is reported, not
    hidden.
- **Docker smoke on a deploy's verify only:** a rollback or drill returns to a release whose
  deploy passed the smoke. The smoke pulls from Docker Hub, and three pulls per drill would add
  minutes and rate-limit risk.
- **The move's defects go to their own tasks (#576, #577):** different code and tests (the move
  helper's proxy step, the browser's profile preparation). Staging's verify now fails on a Caddy
  that does not serve HTTPS.
- **No `[local]` table in this change:** Yulius's tasks already run the server locally through
  the e2e harness on their ports. A `[local]` deployment is not part of this request.

## Evidence

Slice 1 (the CD scripts and `[cd.dev]`):

- Test-first: before the scripts existed, `python3 -m unittest scripts/test_cd.py` failed with
  24 errors. With them, all 27 pass (about 22 s). They cover:
  - refusals and the rollback's dry run;
  - the pin;
  - deploy, redeploy and undo on a fake root with real SQLite;
  - each failing verify check, named by its message;
  - rollback across a schema change and without one, and the drill's step forward;
  - upload from the cache, and the build's commit check.
  The test caught one real bug on the way: the `/internal/version` parser failed on JSON with a
  space after the colon.
- `the_repositorys_own_cd_is_defined` failed against the old `yulius.toml` (panicked at line
  1897) and passes with the new `[cd.dev]`. `scripts/dev quick` passed; `skills::tests`: 22
  passed. `scripts/test_decision_slots.py`, run with discover as the gate does: 8 passed.
- Real build: `scripts/cd/build.sh 2f4d57f1b142ef00a28c1ccc0d24783c6687bdd8` in the sandbox's
  Docker took 7 min 33 s cold (cargo 6 min 6 s). The binary is 86 MB and its `yulius version`
  names the commit.
- By hand against the staging VM, with the committed scripts of 2f4d57f1b, the key from
  `secret_copy_to_env` and the pinned host key:
  - **Deploy:** uploaded by SHA-256 4d6177e4…366c and kept 3fc0e5976 with its store (schema 82).
    It started 2f4d57f1b, which reported its commit.
  - **Verify (deploy):** `/internal/version` named the commit, `/healthz` answered 200 over HTTPS,
    and the contained doctor passed. The store showed schema 82 from 82, 83 tables, no lost
    rows. The Docker smoke test said "sandboxes get Docker on this host". It took 24 s.
  - **Drill:** rollback to 3fc0e5976, verify, forward to 2f4d57f1b, verify, rollback to
    3fc0e5976 again, verify: each exit 0. The live store was kept each time (same schema 82).
    Staging runs 3fc0e5976 again for the first CD deploy.
  - The schema-change branch of the rollback and the undo of a release that does not start were
    exercised on the fake root only: no migration lies between 3fc0e5976 and this branch.
- Vidoc: turned off for this project in Yulius (disabled, not passed).

Review round 1 (Fable 5.2, head c4848a999): request changes, four major findings in this change's
own code, answered at a7242b669:

- **The helper refusal.** It never fired: a running oneshot is `activating`, and `is-active`
  exits 3. It now reads the state word.
- **The no-loss check.** It failed healthy releases: retention at the start and a redeploy's
  stale baseline counted as loss. It now measures the migration itself, before anything stops.
  The new binary migrates a consistent copy of the live store with the new hidden
  `yulius store-migrate` (`yulius_store::migrate_file`). Only tables a migration deletes from by
  design may shrink.
- **The undo test** could not fail for the store restore. The new `halfway` fake migrates and
  writes, then fails to report its commit.
- **An interrupted switch** left the service stopped, and a redeploy could prune the operator's
  release. Now HUP and PIPE are ignored and an EXIT trap recovers. The replaced release is in the
  history before the stop. Checks run before the stop, and one CD step runs at a time (flock).

Evidence of the round:

- **The new test file** run against c4848a999's scripts: 11 failures and errors, one per finding.
  With the fixes, 33 pass. The reviewer's mutation (`restore "$prev"` made a no-op) is caught by 2
  tests.
- **`store_migrate::`** in `crates/yulius`: 2 passed. Before the command existed, `yulius
  store-migrate` was an unknown subcommand.
- **Gates:** `cargo clippy -p yulius-store -p yulius --all-targets --all-features -D warnings`
  clean; `scripts/dev quick` passed.
- **CI's red run** 96b74ed0 at c4848a999 failed on
  `task_done::a_switch_whose_old_run_cannot_be_ended_starts_no_new_run`, which this change does
  not touch. That test passed 40/40 with `--stress-count 40` alone. Filed as #1611, outside this
  change.
- **By hand against the staging VM** with a7242b669's scripts and a real build (6 min 5 s, the
  cache warm, host load about 166):
  - **Deploy:** "release migrates a copy of the live store from schema 82 to 82, 83 tables, no
    rows lost".
  - **Verify:** the commit, HTTPS 200, the contained doctor, schema 82 with the kept migration
    check, and the Docker smoke test.
  - **Drill:** back to 3fc0e5976 (live store kept, same schema), verify, forward, verify.
  - **Redeploy:** "installed already", nothing stopped.
  - **Rollback to 3fc0e5976, verify:** every step exit 0. Staging runs 3fc0e5976 again.

Slice 1 merged into `dev` as e19ea9f18 (PR #1609, from PR head 416a9a113). Its own run
3ae8fb64-0c06-4426-93c3-c7611b8fc2a2 copies the verdict of merge train run
8a5d7592-6a05-42fe-8182-a6ef8a3ca7a8, where the jobs ran once: quality, both test shards and e2e
passed; security skipped, Vidoc off. Review round 3 (Fable 5.2) approved 16e28a937.

**The first staging deploy through Yulius CD:** run `cd50fb89-1e49-406d-b6a2-eae3ded3f791` of
`[cd.dev]` at `e19ea9f18b0523474024fd9cfe11d42ef2f7e5ec`. It was asked by this task with
`cd_deploy`, before merge-first, so there is no staging request. It started at
2026-11-21T18:08:55Z and passed:

- **Build:** release build in CD's builder, 6 min 10 s.
- **Upload:** SHA-256 5bfd1bfb…5426.
- **Migration check:** "release migrates a copy of the live store from schema 82 to 82, 83 tables,
  no rows lost". It kept 3fc0e5976 with its store (schema 82).
- **Verify:** `/internal/version` named e19ea9f18, `/healthz` answered 200 over HTTPS, and the
  contained doctor passed every line. The schema was 82 with the kept migration check.
- **Docker smoke:** passed all its 17 lines ("sandboxes get Docker on this host").
- **Drill:** none followed. It was the environment's first release, and a drill needs two. The
  next deploy drills.

Slice 2 (merge-first and the promotion rule), test-first:

- `the_repositorys_own_cd_is_defined` failed at the `merge_first` assertion
  (config.rs:1913) before `yulius.toml` changed. With the change, it asserts the job split
  `quality`/`security` on pull requests and `test (1/2)`, `test (2/2)`, `e2e` after the merge,
  and passes.
- `staging::yuliuss_own_config_promotes_only_what_staging_verified` runs with
  `include_str!("../../../yulius.toml")`. Before the change, it was refused for another reason
  ("no verified revision in dev covers its required CI jobs"). After it, both the production
  deploy and the Delivery page's Promote answer 409, "[cd.dev] staging verified nothing yet".
  It passes.
- Review round 3's minor and nits, test-first: 9 new failures against 16e28a937's scripts, then
  42 pass. The tests caught one real bug: `restore` called inside a condition ran on past a
  failed copy, because sh suspends `-e` there. It now returns each failure.

**The second staging deploy through Yulius CD and Yulius's rollback drill:**

- **Deploy:** run `f3129b43-0e36-4d08-80eb-0acfdf3891ae` of `[cd.dev]` at
  `7383920f0d9286b99e4f72387f10065889430b9a`, which includes #577's Chrome-lock fix, asked by this
  task. It passed.
- **Drill:** Yulius drilled the rollback by itself, run `8946704a-03d6-411f-a4b0-88c70bb5d356`
  (kind drill, by yulius, 2026-11-21T18:38:30Z), and it passed.
  - **Back:** `scripts/cd/rollback.sh dev` to `e19ea9f18b0523474024fd9cfe11d42ef2f7e5ec`. It kept
    the store 7383920f0 left and kept the live store (same schema 82). `::yulius-rolled-back 0`.
  - **Verify:** the commit, HTTPS 200 and the contained doctor passed, with "the store has
    schema 82, the one release e19ea9f18 left".
  - **Forward:** to `7383920f0`, with the store e19ea9f18 left kept. `::yulius-rolled-back 1`.
  - **Verify again:** it passed.
- **After it:** `cd_status` shows releases 7383920 (current) and e19ea9f, and "drill: passed at
  7383920 (dev)".

Slice 2, review round 1 (Fable 5.2, head 744655f2c): request changes, one major in this change's
own code, answered:

- **The major: the "stays stopped" state did not last.** After a failed put back, the rollback
  Yulius starts by itself, or a redeploy, took the `already` path and started the server on an
  empty store.
  - Now `hold` writes `/var/lib/yulius-releases/HELD` and prints
    `::yulius-rollback-barrier staging is held…`, so Yulius restores nothing by itself. Activate,
    rollback and verify refuse while the file exists.
  - Both stays-stopped tests now also check, through `assert_held`: a rollback to the previous
    release, a redeploy and a verify are each refused, the server stays inactive, and the
    snapshot digest is unchanged.
- **Minors:**
  - Operations step 7 asks through `cd_deploy`, and the held marker is in the hand-restore steps.
  - `docs/engineering/git.md` and the `yulius-git` skill say the work ends once the staging
    request was answered. The skill was regenerated with `apm install` (0.37.0); the lockfile
    keeps its format, with only the content hash changed.
  - A test passes the real two-pair list to `activate`.
  - ADR-0436 names where its numbers come from: the owner's figures in task #575's request, not
    measured again here.
- **Nits:**
  - The deletion parser reads comments and quoted strings in one pass. A string in a trigger
    deletes nothing, and a quote in a comment no longer hides a statement: the first, two-regex
    try lost migration 69, and the test caught it.
  - The `CASE` fixture has a top-level `DELETE FROM d` after it.
  - The page assertion names "staging verified nothing yet".
  - ADR-0436 is precise on when the full suite runs, and says that a flake on `dev` holds
    promotion and must be fixed or quarantined.
  - The lock test's cleanup runs once. The test's doc comment is current.
- **The new checks before the fixes:** 3 failures, then 42 pass. `scripts/dev quick`,
  `skills::tests` (22), `the_repositorys_own_cd_is_defined` and the staging test pass.

Slice 2, review round 2 (Fable 5.2): approved 4da6b5321. Its one minor, a fixture that passed
without the string handling, is answered in tests only. Two new lines must not count as
deletions: a trigger with `RAISE(ABORT, 'at its end'); DELETE FROM e`, and a top-level `INSERT …
('DELETE FROM g')`. With the quoted-string branch of `code_only` disabled, the test fails with
`2:d,2:e,2:g,2:notes`; with it, the test passes. Its nit (double-quoted identifiers holding an
apostrophe) is left: such an identifier is contrived, and no migration has one.

Slice 2 merged into `dev` as 77f285e08 (PR #1618, from PR head 4e2927eaf). Its own run
4f7fcfce-f75f-4ed4-9201-44d0c14c0d3c copies the verdict of merge train run
01163ca6-3baf-4227-947a-e952b0710956, where the jobs ran once: quality, both test shards and e2e
passed; security skipped, Vidoc off. Merge-first is in force from that revision on.

**The first merge-first staging request, end to end:**

- **The request:** `421603a1-b80a-4306-9a26-5b49587c4798`, made by this task with `cd_deploy`, for
  revision `77f285e0891b0111c315360907115707fcda645c` (its landing). It started as "testing".
- **Its full gate:** candidate run `9d3d2655-6ac7-4db6-9fef-b42027df9903` (`Candidate 77f285e08`)
  passed in 13 min 58 s:
  - `quality`, 4 min 18 s;
  - `test (1/2)`, 4 min 45 s;
  - `test (2/2)`, 3 min 56 s;
  - `e2e`, 6 min 54 s;
  - `security` skipped: "not a pull request head".
  This is the full suite run on exactly the merged revision, as ADR-0410 and ADR-0436 say.
- **Its deploy:** CD run `ba7de908-5219-4252-aa73-938e70729f74` of `[cd.dev]` at 77f285e started
  2026-11-21T19:37:09Z and passed. The request reads `verified`, and the queue shows "verified revision
  77f285e0891b".
- **Its drill:** Yulius's run `a2c1fe2b-91ea-47f8-92fb-5867679ecb42` went back to `7383920f0`
  and then forward to `77f285e08`. It started 2026-11-21T19:41:43Z and passed.
- **After it:** `cd_status` shows releases 77f285e (current), 7383920 and e19ea9f, and "drill:
  passed at 77f285e (dev)". Staging serves the revision whose full suite passed, so it is the
  revision a person may promote to `main` while it is the newest one staging verified (ADR-0436;
  no promotion was asked or observed). Promotion is not this task's.

**What could not be tested here:**

- **Two paths on real data:** no migration lies between the staging releases, so the rollback
  across a schema change (restoring a release's own store) and a lossy migration's refusal ran
  only on the fake root.
- **On real systemd:** the undo of a release that does not start and the held state were also
  exercised only on the fake root.
- **The no-loss check on a used store:** the staging instance is still not set up by a person (no
  GitHub App, no users), so the check ran on a nearly empty store.
- **Paths shown only by the fake-root tests:** the recovery of an interrupted switch (HUP and PIPE
  ignored, the EXIT trap), the lock, a rollback's binary uploaded from the CD cache, pruning past
  10 releases (staging holds three), and a `gate_failed` staging request on this instance.
- **The oneshot `activating` state** of `yulius-update.service` is inferred from systemd and the
  repository's own code. The update helper is disabled on staging, and it was not run.
