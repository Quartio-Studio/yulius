#!/bin/sh
# Yulius CD's rollback of the staging VM, and each step of its drill (ADR-0430;
# docs/operations.md, "Yulius's own staging VM"): scripts/cd/rollback.sh dev
# Puts the release $YULIUS_CD_RELEASE back from what is kept (the VM's releases, else the CD
# cache; a rollback never builds), with its own store when the live one has another schema (a
# newer release migrated it), else with the live store. The store being left is kept first, so a
# drill's step forward gets it back. YULIUS_CD_DRY_RUN=1 says what it would do and changes nothing.
set -eu
. "$(dirname "$0")/lib.sh"
cd_env "${1:-}"

release="${YULIUS_CD_RELEASE:-}"
[ -n "$release" ] || die "no YULIUS_CD_RELEASE: Yulius CD names the release to roll back to"
valid_release "$release" || die "YULIUS_CD_RELEASE is not a release this CD deploys (a commit id): $release"
if [ "${YULIUS_CD_DRY_RUN:-}" = 1 ]; then
  say "dry run: would switch $env_name to release $release, from the VM's kept releases or ${cache:-the CD cache}, with its own store across a schema change; nothing changed"
  exit 0
fi
need sha256sum
ssh_start

# The cached build is marked as used, so the cache's pruning keeps what was recently current.
if [ -n "$cache" ] && [ -d "$cache/releases/$release" ]; then touch "$cache/releases/$release"; fi
if kept_on_vm "$release"; then
  say "release $release is kept on the VM"
elif [ -n "$cache" ] && [ -x "$(cached "$release")" ]; then
  upload "$release" "$(cached "$release")"
else
  die "release $release is kept neither on the VM nor in the CD cache: a rollback never builds; deploy it instead"
fi
remote rollback "$release"
say "rolled $env_name back to release $release"
