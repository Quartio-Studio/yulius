---
yulius-task: 5a254bf5-8315-4e31-862b-e339e88e8434
status: in-progress
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
  - `quality` and `security` stay on pull requests, justified by the CI numbers in an ADR.
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
- [ ] Independent review of the CD slice approved
- [ ] First staging deploy through CD and the rollback drill of the next deploy recorded
- [ ] Move the test shards and e2e to the merged revision with merge_first and an ADR
- [ ] Make the promotion rule explicit and test it against Yulius's own yulius.toml
- [ ] Evidence, changes entry and independent review of the merge-first slice approved

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
- **AC-3 (STG-004):** On a fake root, verify fails in each of these cases:
  - a wrong recorded release;
  - a binary that is not the release;
  - another commit at `/internal/version`;
  - a failing doctor;
  - a live table with fewer rows than the snapshot;
  - a dropped table;
  - a schema other than the tree's.
  It passes otherwise. The smoke runs on a deploy's verify only when the doctor offers Docker.
- **AC-4 (STG-002, real):** A CD run of `[cd.dev]` deploys dev's head to the staging VM. Its
  output has the build, `::yulius-release <sha>` and verify's lines, and
  `/internal/version` on the VM reports that commit. A later deploy's rollback drill passes (back
  to the previous release and its store, forward again). The spec's Evidence names both runs,
  their revisions and request ids.
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
  - Tables a migration deletes from by design (`DELETE FROM` in the tree's migrations) may shrink.
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
