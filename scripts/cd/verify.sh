#!/bin/sh
# Yulius CD's verify of the staging VM, after a deploy, a rollback and each step of a drill
# (ADR-0430): scripts/cd/verify.sh dev
# On the VM (remote.sh verify): the release recorded current is the one this run deployed
# ($YULIUS_CD_SHA) or rolled back to ($YULIUS_CD_RELEASE), and the installed binary is it;
# /internal/version reports it; the public URL answers /healthz over HTTPS; the contained doctor
# passes; the live store is sound; after a deploy, its schema is this tree's migrations count, the
# release's migration check (no rows lost, on a copy before the stop) is kept, and the sandbox
# Docker smoke test passes where the doctor offers sandboxes Docker.
set -eu
. "$(dirname "$0")/lib.sh"
cd_env "${1:-}"

expected=-
schema=-
smoke=0
if [ -n "${YULIUS_CD_RELEASE:-}" ]; then
  expected="$YULIUS_CD_RELEASE"
elif [ "${YULIUS_CD_KIND:-}" = deploy ]; then
  expected="${YULIUS_CD_SHA:-}"
  # The schema a release migrates to is the number of its migrations (crates/yulius-store's
  # MIGRATIONS lists each file once; scripts/test_cd.py checks that).
  schema="$(find "$repo/crates/yulius-store/src/migrations" -maxdepth 1 -name '*.sql' | wc -l | tr -d ' ')"
  smoke=1
fi
[ "$expected" = - ] || valid_release "$expected" || die "not a release: $expected"
ssh_start
remote verify "$expected" "$schema" "$smoke"
