# 0430. Yulius's staging builds in CD's builder and installs a kept release with its store

## Status

Accepted. Specified in [SPEC-591](../../.ai/specs/SPEC-591-yulius-gets-its-own-staging-instance-and.md)
(STG-001 to STG-006); task #575. Gives Yulius's own `[cd.dev]` the staging target that
[0410](0410-the-full-suite-runs-on-the-merged-revision-beside-a-merge-first-staging-target.md)
and [0411](0411-staging-changes-through-a-queue-of-tested-merged-revisions.md) need. Keeps
[0264](0264-yulius-own-production-rolls-back-through-its-update-not-a-cd-command.md) for
production and [0263](0263-rollback-redeploys-a-kept-release-and-a-dev-drill-proves-it-before-production.md)
for the drill.

## Context

The owner gave Yulius a staging VM, the former production host. It runs a fresh Yulius instance
that no person relies on: `yulius.service`, the update helper's units with the helper's path unit
disabled, and Caddy in front. CD must put the exact build of a dev revision there. It must prove
that the build runs, and roll it back with the store it had.

Two places could build it:

- **CD's builder.** A dev CD run has the sandbox's Docker. Autoresearch, a fork of Yulius, builds
  there and uploads the binary.
- **The VM's own update helper** (`yulius update-helper`, ADR-0013). Root's Rust toolchain and a
  checkout are still on the VM from production.

## Decision

- **The build happens in CD's builder.**
  - `scripts/cd/build.sh` runs `deploy/build/Dockerfile`: Ubuntu 24.04, rustup pinned by its
    digest, the toolchain of `rust-toolchain.toml`, and `cargo build --release --locked -p yulius`
    with `YULIUS_BUILD_COMMIT` set.
  - It refuses a binary whose `yulius version` names another commit.
  - The build is kept in `$YULIUS_CD_CACHE` for the last 10 deploys, and a rollback never builds.
- **The upload is checked by SHA-256** into `/var/lib/yulius-releases/<commit>/` on the VM
  (root's, the last 10 releases used).
- **SSH as in autoresearch.**
  - CD connects to `root@$STAGING_VM_HOST`, a project variable.
  - The host key is pinned as `yulius-staging` in `scripts/cd/known_hosts`, checked with
    `StrictHostKeyChecking=yes` and `HostKeyAlias`.
  - The key is `STAGING_VM_SSH_KEY`, held by an ssh-agent that the script starts and stops.
  - Every step on the VM is `scripts/cd/remote.sh`, piped to `sh` and never installed there.
- **The migration is measured before anything stops.** The new binary migrates a consistent copy
  of the live store (SQLite's backup, with a hidden `yulius store-migrate --db <copy>` that applies
  the release's own migrations and never creates a store). No table may vanish, and none may lose
  rows but those a migration this deploy applies deletes from by design (its own `DELETE FROM`,
  not a trigger's, read by `scripts/cd/migration_deletes.py` with its schema). A
  failed check refuses the deploy with nothing changed. The result is kept beside the release for
  verify.
- **The install is CD's own and follows the helper's contract.**
  - Stop `yulius.service` only. Agent sessions live in `yulius-sandboxes.service` and keep
    running.
  - Keep the replaced binary and a snapshot of its store (`yulius.db` with its WAL, as that
    release left it) under that release.
  - Install, start, and wait up to 2 minutes until `/internal/version` reports the commit.
  - Otherwise put the previous binary and the snapshot back.
  - A second deploy of the running release changes nothing.
  - A lost connection does not stop a switch half way (HUP and PIPE are ignored), and any other
    exit after the stop starts the server again with a binary and store that belong together.
    When the previous store cannot be put back, the server stays stopped and the run says to
    restore it by hand: started, it would serve another store or create an empty one.
  - The replaced release is in the kept history before anything stops, so pruning never removes
    the release an interrupted switch goes back to. One CD step runs at a time (a lock on the VM).
- **Verify proves the release.**
  - The recorded release is the installed binary and the commit at `/internal/version`.
  - The unit's public URL answers `/healthz` over HTTPS.
  - The contained doctor passes as the service user with the unit's environment.
  - The live store passes SQLite's check. After a deploy its schema is the tree's migrations
    count and the release's migration check is kept. After a rollback its schema is the one the
    release left. Rows deleted after the start (retention, a person) are no migration loss.
  - After a deploy, `yulius doctor --docker-smoke` passes where the doctor offers sandboxes Docker.
- **Rollback restores the release with its store.** When the live store has another schema than
  the store the target release left, that store is restored. Otherwise the live store is kept,
  since the release reads it. The store being left is snapshotted first, so the drill's step
  forward gets it back.

## Alternatives

- **The VM's update helper.**
  - By design it builds only commits on `main`, never older than its last install. Staging serves
    `dev`, and a rollback goes back.
  - It reads the private repository with the GitHub App or a token file. The staging instance has
    neither, and adding one would put a repository credential on a machine people will try things
    on.
  - A rollback or drill would rebuild instead of redeploying the same bytes.
  - The build would take the VM's CPUs, on a VM that will be replaced by a smaller one.
  - Widening it (dev commits, older commits, an uploaded binary) would weaken the guard that keeps
    production instances on `main`.
- **Building on the VM from an uploaded source archive.** Same toolchain and size coupling, and
  no artifact to keep for rollbacks.
- **A new `yulius host install` command.** Cleaner than shell, but a product change and a
  privileged command to design. The script is small and tested against a fake root, and only
  Yulius's own CD needs it.
- **Comparing the live store with the snapshot after the start.** Retention at the server's start
  (CI runs, spans) and ordinary deletes would read as loss, and a lossy migration would have
  served staging before it was caught.
- **Always restoring the release's store on rollback.** That loses every staging write since
  the deploy, also when nothing needs it. Restoring only across a schema change keeps them when
  the older release can read the live store.

## Consequences

- Each dev deploy builds once in the sandbox's Docker and caches the crate registry and target
  as BuildKit mounts. The first build of a revision takes minutes within the CI job timeout. A
  rebuild after a small change takes less.
- The VM needs no toolchain, no GitHub credential and no checkout. The production leftovers
  (root's rustup, `/var/lib/yulius-update/src`) are unused, and `yulius-update.path` stays
  disabled. A deploy refuses while the helper runs.
- Swapping the VM changes the variable and the pinned key, in a pull request. Nothing depends on
  its size or address.
- A rollback across a schema change drops staging data the newer release wrote. The snapshot it
  replaced stays under the newer release until it is pruned.
- On a fresh staging instance the migration check proves little until people use it. It names
  the table that loses rows when it does, before staging changes.
- `yulius store-migrate` is a hidden command for this check; it changes only the file it is
  given.
