#!/bin/sh
# The release build of one commit (ADR-0430): scripts/cd/build.sh <commit> <out-dir>
# Runs deploy/build/Dockerfile with this tree as its context (Ubuntu 24.04, the toolchain of
# rust-toolchain.toml, YULIUS_BUILD_COMMIT=<commit>) and writes <out-dir>/yulius, whose
# `yulius version` must name <commit>. The tree must be that commit's (CD runs it in the commit's
# own files; by hand: `git archive <commit> | tar -x -C <dir>` first).
set -eu
. "$(dirname "$0")/lib.sh"

[ $# -eq 2 ] || {
  echo "usage: $0 <commit> <out-dir>" >&2
  exit 2
}
release="$1"
out="$2"
valid_release "$release" && [ "${#release}" -eq 40 ] || die "not a full commit id: $release"
need docker
toolchain="$(sed -n 's/^channel *= *"\([^"]*\)".*/\1/p' "$repo/rust-toolchain.toml")"
[ -n "$toolchain" ] || die "no channel in rust-toolchain.toml"
mkdir -p "$out"
say "building release $release with Rust $toolchain (deploy/build/Dockerfile)"
DOCKER_BUILDKIT=1 docker build --progress=plain -f "$repo/deploy/build/Dockerfile" \
  --build-arg "RUST_TOOLCHAIN=$toolchain" --build-arg "YULIUS_BUILD_COMMIT=$release" \
  --output "type=local,dest=$out" "$repo"
version="$("$out/yulius" version)" || die "the build does not run here"
case "$version" in
  *"($release)") say "built $version" ;;
  *) die "the build says \"$version\", not commit $release" ;;
esac
