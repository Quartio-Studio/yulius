#!/bin/sh
# Yulius CD's deploy of Yulius to its staging VM (ADR-0430; docs/operations.md, "Yulius's own
# staging VM"): scripts/cd/deploy.sh dev
# Builds the commit $YULIUS_CD_SHA with scripts/cd/build.sh (once: kept in $YULIUS_CD_CACHE),
# copies it to the VM (checked by SHA-256, kept there for rollbacks), and has remote.sh install it:
# the release's migrations checked on a copy of the live store first (no rows lost), then
# yulius.service stopped, the replaced binary and its store kept, the release installed and
# started (it migrates the store), put back when it does not report its commit. Prints
# ::yulius-release <commit>. The same commit again changes nothing.
set -eu
. "$(dirname "$0")/lib.sh"
cd_env "${1:-}"

release="${YULIUS_CD_SHA:-}"
valid_release "$release" || die "YULIUS_CD_SHA is not a commit id: Yulius CD sets it to the commit it deploys"
[ -n "$cache" ] || die "no YULIUS_CD_CACHE: Yulius CD gives each environment a cache for its builds"
need sha256sum docker
ssh_start

vm_arch="$(remote arch)"
build_arch="$(docker info --format '{{.Architecture}}')"
[ "$vm_arch" = "$build_arch" ] ||
  die "the VM is $vm_arch, but this Docker builds for $build_arch"

# Builds a killed run left (one run per environment at a time: none is running).
rm -rf "$cache/releases"/.build-*
binary="$(cached "$release")"
if [ -x "$binary" ]; then
  say "release $release is built already ($binary)"
else
  build="$cache/releases/.build-$$"
  "$cd_dir/build.sh" "$release" "$build"
  mkdir -p "$cache/releases/$release"
  mv "$build/yulius" "$binary"
  rm -rf "$build"
fi
touch "$cache/releases/$release"

if kept_on_vm "$release"; then
  say "release $release is kept on the VM already"
else
  upload "$release" "$binary"
fi
# The tables each migration deletes rows from by design, as <schema>:<table> (its own DELETE FROM,
# triggers aside): the migration check lets only those lose rows, and only when the deploy applies
# that migration.
deletes="$(python3 -I "$cd_dir/migration_deletes.py" "$repo")" || die "cannot read the store's migrations"
remote activate "$release" "$deletes"
echo "::yulius-release $release"

# The last 10 builds used stay in the cache (the VM keeps its own copies); never fails the deploy.
ls -1t "$cache/releases" | grep -E '^[0-9a-f]{7,64}$' | tail -n +11 | while read -r old; do
  rm -rf "${cache:?}/releases/$old" || say "cannot remove the old build $old from the cache"
done || true
say "deployed release $release to $env_name"
