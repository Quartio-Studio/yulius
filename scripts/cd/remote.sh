#!/bin/sh
# CD's steps on Yulius's staging VM, as root (ADR-0430; docs/operations.md, "Yulius's own staging
# VM"). Piped to `sh -s -- <command> [args]` over SSH by scripts/cd/lib.sh; never installed there.
#
#   arch                     the VM's architecture (uname -m)
#   has <release>            exit 0 when the release's binary is kept here, 3 when it is not
#   receive <release> <sha256>
#                            the uploaded .incoming-<release> checked and kept as
#                            /var/lib/yulius-releases/<release>/yulius (root's)
#   activate <release> <deletes|->
#                            before anything stops: the release's migrations run on a consistent
#                            copy of the live store (`yulius store-migrate`), and no table may lose
#                            rows but those a migration it applies deletes from by design
#                            (<deletes>: <schema>:<table>, comma separated); then yulius.service stopped, the replaced release's
#                            binary and the store it left kept under it, the release installed and
#                            started (it migrates the store), put back with that store when it does
#                            not report its commit; recorded current; the last 10 releases kept
#   rollback <release>       the same switch to a kept release, with the store that release left
#                            when the live store has another schema, else with the live store
#   verify <release|-> <schema|-> <0|1>
#                            the release is current, installed and running (/internal/version);
#                            the public URL answers /healthz; the contained doctor passes; the
#                            live store is sound and has the release's schema; after a deploy
#                            (schema given) its migration check passed; the Docker smoke test
#                            (when 1, where the doctor offers sandboxes Docker)
#
# Only yulius.service is stopped and started: agent sessions live in yulius-sandboxes.service. A
# switch ignores a lost connection (HUP, PIPE) and runs to its end; any other exit between the
# stop and the end of the switch starts the server again with a binary and store that belong
# together, or, when the previous store cannot be put back, leaves it stopped (restore by hand).
# Everything is in functions and runs from the last line, so sh has read the whole
# script from its standard input before any command could read from it.
set -eu

# YULIUS_CD_ROOT, YULIUS_CD_START_WAIT and YULIUS_CD_LOCK_WAIT are for scripts/test_cd.py's fake
# root; SSH passes none of them to the VM.
root="${YULIUS_CD_ROOT:-}"
releases="$root/var/lib/yulius-releases"
bin="$root/usr/local/bin/yulius"
service=yulius.service
helper=yulius-update.service
keep=10
start_wait="${YULIUS_CD_START_WAIT:-120}"
lock_wait="${YULIUS_CD_LOCK_WAIT:-600}"
# Scratch space for store copies: beside the releases (a disk), never /tmp (often RAM).
scratch="$releases/.scratch"
# Left when a put back failed: the server stays stopped until a person restored the store by hand
# and removed it. Every switch and verify refuses meanwhile, also the rollback Yulius would start.
held="$releases/HELD"

say() { echo "vm: $*" || true; }
die() {
  echo "vm: $*" >&2 || true
  exit 1
}

valid_release() {
  case "$1" in '' | *[!0-9a-f]*) return 1 ;; esac
  [ "${#1}" -ge 7 ] && [ "${#1}" -le 64 ]
}

sha_of() { sha256sum "$1" | cut -d ' ' -f 1; }

# commit_of <binary>: the commit its `yulius version` names ("yulius 0.1.0 (<commit>)"), or nothing.
commit_of() {
  "$1" version </dev/null 2>/dev/null | sed -n 's/.*(\([0-9a-f]\{7,64\}\))$/\1/p' | head -n 1
}

# unit: the server's environment and user as systemd runs it.
unit() {
  unit_env="$(systemctl show "$service" -p Environment --value </dev/null)"
  unit_user="$(systemctl show "$service" -p User --value </dev/null)"
  unit_user="${unit_user:-yulius}"
  data="$(env_of YULIUS_DATA)"
  data="${data:-/var/lib/yulius}"
  store="$root$data"
  internal="$(env_of YULIUS_INTERNAL_BIND)"
  internal="${internal:-127.0.0.1:8081}"
  public="$(env_of YULIUS_PUBLIC_URL)"
}

env_of() {
  set -f
  for kv in $unit_env; do
    case "$kv" in "$1="*)
      set +f
      echo "${kv#*=}"
      return 0
      ;;
    esac
  done
  set +f
}

# The commit the running server reports on its internal listener, or nothing.
running_commit() {
  curl -fsS --max-time 5 "http://$internal/internal/version" </dev/null 2>/dev/null |
    sed -n 's/.*"commit" *: *"\([0-9a-f]*\)".*/\1/p' || true
}

# wait_for <commit>: the server reports it within $start_wait seconds.
wait_for() {
  deadline=$(($(date +%s) + start_wait))
  while :; do
    [ "$(running_commit)" = "$1" ] && return 0
    [ "$(date +%s)" -lt "$deadline" ] || return 1
    sleep 1
  done
}

# The update helper is a oneshot: while it runs, systemd says "activating" (and is-active exits
# 3), so the state's word decides, never the exit code.
refuse_while_updating() {
  state="$(systemctl is-active "$helper" </dev/null 2>/dev/null || true)"
  case "$state" in
    active | activating | deactivating | reloading | refreshing)
      die "the update helper ($helper) is $state: nothing changed; deploy again once it ended (on staging it stays disabled: docs/operations.md)"
      ;;
  esac
}

# py <args>: python3 with its temporary files in the scratch space.
py() {
  mkdir -p -m 0700 "$scratch"
  TMPDIR="$scratch" python3 -I - "$@"
}

# counts <db> [live]: one JSON line, {"schema": n, "tables": {name: rows}}, of a copy of the store
# (a live one through SQLite's backup, read-only; a stopped one with its WAL).
counts() {
  py "$1" "${2:-}" <<'EOF'
import json, os, shutil, sqlite3, sys, tempfile, urllib.parse
path, live = sys.argv[1], sys.argv[2] == "live"
with tempfile.TemporaryDirectory() as tmp:
    copy = os.path.join(tmp, "yulius.db")
    if live:
        src = sqlite3.connect("file:" + urllib.parse.quote(path) + "?mode=ro", uri=True)
        db = sqlite3.connect(copy)
        src.backup(db)
        src.close()
    else:
        shutil.copy2(path, copy)
        if os.path.exists(path + "-wal"):
            shutil.copy2(path + "-wal", copy + "-wal")
        db = sqlite3.connect(copy)
    check = db.execute("PRAGMA quick_check").fetchone()[0]
    if check != "ok":
        sys.exit("vm: the store at %s is damaged: %s" % (path, check))
    names = [r[0] for r in db.execute(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")]
    tables = {n: db.execute('SELECT count(*) FROM "%s"' % n.replace('"', '""')).fetchone()[0] for n in names}
    schema = db.execute("PRAGMA user_version").fetchone()[0]
    db.close()
print(json.dumps({"schema": schema, "tables": tables}, sort_keys=True))
EOF
}

schema_of() { sed -n 's/.*"schema": *\([0-9]*\).*/\1/p' "$1"; }

# migration_check <release> <deletes|->: the release migrates a consistent copy of the live store
# (taken while it runs, so nothing has stopped yet); no table may vanish, and none may lose rows
# but those a migration it applies deletes from by design. The result is kept as
# <release>/migration.json.
migration_check() {
  [ -f "$store/yulius.db" ] || die "no store at $store/yulius.db"
  rm -rf "$scratch/check"
  mkdir -p -m 0700 "$scratch/check"
  py "$store/yulius.db" "$scratch/check/yulius.db" <<'EOF'
import sqlite3, sys, urllib.parse
src = sqlite3.connect("file:" + urllib.parse.quote(sys.argv[1]) + "?mode=ro", uri=True)
dst = sqlite3.connect(sys.argv[2])
src.backup(dst)
dst.close()
src.close()
EOF
  counts "$scratch/check/yulius.db" >"$scratch/check/before.json"
  rc=0
  out="$("$releases/$1/yulius" store-migrate --db "$scratch/check/yulius.db" </dev/null 2>&1)" || rc=$?
  [ "$rc" = 0 ] || die "release $1 cannot migrate a copy of the live store (exit $rc): $out; nothing changed"
  counts "$scratch/check/yulius.db" >"$scratch/check/after.json"
  py "$scratch/check/before.json" "$scratch/check/after.json" "$2" "$releases/$1/migration.json" <<'EOF'
import json, sys
before, after = (json.load(open(p)) for p in sys.argv[1:3])
# Only the migrations this deploy applies (above the copy's schema, up to the release's).
deletes = set()
for pair in [] if sys.argv[3] == "-" else sys.argv[3].split(","):
    at, table = pair.split(":", 1)
    if before["schema"] < int(at) <= after["schema"]:
        deletes.add(table)
problems = []
for name, rows in sorted(before["tables"].items()):
    now = after["tables"].get(name)
    if now is None:
        problems.append("%s: gone" % name)
    elif now < rows and name not in deletes:
        problems.append("%s: %d rows before, %d now" % (name, rows, now))
result = {"from": before["schema"], "to": after["schema"], "tables": len(after["tables"]),
          "problems": problems}
if problems:
    sys.exit("vm: its migration loses data, so nothing changed: " + "; ".join(problems)
             + " (a migration that deletes rows by design says DELETE FROM <table> itself)")
json.dump(result, open(sys.argv[4], "w"), sort_keys=True)
print("vm: release migrates a copy of the live store from schema %d to %d, %d tables, no rows lost"
      % (result["from"], result["to"], result["tables"]))
EOF
  rm -rf "$scratch/check"
}

# snapshot <release>: the stopped store, as <release> left it, kept under it with its counts.
snapshot() {
  dir="$releases/$1/store"
  [ -f "$store/yulius.db" ] || die "no store at $store/yulius.db"
  rm -rf "$dir.new"
  mkdir -p -m 0700 "$dir.new"
  cp -p "$store/yulius.db" "$dir.new/yulius.db"
  if [ -f "$store/yulius.db-wal" ]; then cp -p "$store/yulius.db-wal" "$dir.new/yulius.db-wal"; fi
  counts "$dir.new/yulius.db" >"$dir.new/counts.json"
  rm -rf "$dir"
  mv "$dir.new" "$dir"
  say "kept the store release $1 left (schema $(schema_of "$dir/counts.json")) in $dir"
}

# restore <release>: the store <release> left, back in the data directory (the server stopped).
# Each step's failure is returned, never left to errexit: callers test it, where sh ignores -e.
restore() {
  src="$releases/$1/store"
  [ -f "$src/yulius.db" ] || {
    echo "vm: no store of release $1 is kept in $src" >&2 || true
    return 1
  }
  owner="$(stat -c %U:%G "$store")" || return 1
  rm -f "$store/yulius.db" "$store/yulius.db-wal" "$store/yulius.db-shm" || return 1
  cp "$src/yulius.db" "$store/yulius.db" || return 1
  if [ -f "$src/yulius.db-wal" ]; then cp "$src/yulius.db-wal" "$store/yulius.db-wal" || return 1; fi
  for f in "$store/yulius.db" "$store/yulius.db-wal"; do
    [ -f "$f" ] || continue
    chown "$owner" "$f" && chmod 0600 "$f" || return 1
  done
}

install_bin() {
  cp "$releases/$1/yulius" "$bin.new" && chown root:root "$bin.new" && chmod 0755 "$bin.new" &&
    mv -f "$bin.new" "$bin"
}

# note <release>: in the history, unless it is its last line. The replaced release is noted before
# anything stops, so pruning never takes the release an interrupted switch would go back to.
note() {
  if [ "$(tail -n 1 "$releases/history" 2>/dev/null || true)" != "$1" ]; then
    echo "$1" >>"$releases/history"
  fi
}

# keep_binary <release>: the running binary kept as that release (one CD did not install, such as
# the operator's first, becomes a release a rollback can go back to).
keep_binary() {
  if [ ! -f "$releases/$1/yulius" ]; then
    mkdir -p -m 0700 "$releases/$1"
    cp "$bin" "$releases/$1/yulius"
    chmod 0755 "$releases/$1/yulius"
    say "kept the running binary as release $1"
  fi
  note "$1"
}

# What an exit between the stop and the end of the switch leaves: the server started again, with
# the binary and store of $prev when the new binary was installed already.
phase=none
recover() {
  rc=$?
  case "$phase" in
    stopped)
      echo "vm: interrupted before anything changed: starting $service again" >&2 || true
      systemctl start "$service" </dev/null || true
      ;;
    installed)
      echo "vm: interrupted: putting back release $prev and its store" >&2 || true
      systemctl stop "$service" </dev/null || true
      if put_back "$prev"; then
        systemctl start "$service" </dev/null || true
      else
        hold "$prev"
        echo "vm: putting back release $prev failed: $service stays stopped; restore by hand from $releases/$prev (docs/operations.md, \"Yulius's own staging VM\")" >&2 || true
      fi
      ;;
  esac
  exit "$rc"
}

# hold <release>: a put back of <release> failed. Recorded for every later step, and announced as
# a rollback barrier, so Yulius does not start a rollback of staging by itself.
hold() {
  printf '%s release %s or its store could not be put back; %s is stopped\n' \
    "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" "$service" >"$held" || true
  echo "::yulius-rollback-barrier staging is held: the store of release $1 could not be put back; a person restores it by hand" || true
}

refuse_while_held() {
  [ ! -f "$held" ] ||
    die "staging is held ($(head -c 300 "$held" 2>/dev/null || true)): restore the store by hand from $releases (docs/operations.md, \"Yulius's own staging VM\"), then remove $held"
}

# put_back <release>: its binary and the store it left back in place; fails, changing nothing
# more, when either cannot be (the server must then stay stopped: it would start on another
# store, or create an empty one).
put_back() { (install_bin "$1" && restore "$1"); }

stop_server() {
  say "stopping $service (agent sessions keep running in yulius-sandboxes.service)"
  phase=stopped
  systemctl stop "$service" </dev/null
}

# switch <release>: install and start <release> over $prev; when it does not report its commit,
# put $prev and the store it left back and fail.
switch() {
  phase=installed
  install_bin "$1"
  say "starting $service with release $1"
  systemctl start "$service" </dev/null || true
  if wait_for "$1"; then
    phase=done
    say "release $1 reports its commit at /internal/version"
    return 0
  fi
  say "release $1 did not report its commit within ${start_wait}s: putting back release $prev and its store"
  systemctl stop "$service" </dev/null || true
  if ! put_back "$prev"; then
    phase=done
    hold "$prev"
    die "release $1 did not report its commit, and putting back release $prev failed: $service stays stopped; restore by hand from $releases/$prev (docs/operations.md, \"Yulius's own staging VM\")"
  fi
  phase=done
  systemctl start "$service" </dev/null || true
  wait_for "$prev" ||
    die "release $1 did not start, and release $prev with its store does not answer either: restore by hand from $releases/$prev (journalctl -u $service)"
  die "release $1 did not report its commit (journalctl -u $service): put back release $prev and its store, which run"
}

# The installed release again: running it is all there is to do.
already() {
  say "release $1 is installed already"
  if [ "$(running_commit)" != "$1" ]; then
    systemctl start "$service" </dev/null || true
    wait_for "$1" || die "release $1 is installed but does not answer (journalctl -u $service)"
  fi
}

# record <release>: the release is current; the last $keep releases used (deployed, rolled back to
# or replaced), by their last use, are kept, and the current one.
record() {
  note "$1"
  echo "$1" >"$releases/current.new"
  mv "$releases/current.new" "$releases/current"
  # Newest first, each release once, at its last use.
  wanted="$(tail -n 200 "$releases/history" | awk '{ l[n++] = $0 } END { for (i = n - 1; i >= 0; i--) if (!seen[l[i]]++) print l[i] }' | head -n "$keep")"
  find "$releases" -mindepth 1 -maxdepth 1 -type d | while read -r dir; do
    name="$(basename "$dir")"
    valid_release "$name" || continue
    # Only a release the history names is pruned (uploads are named when they arrive).
    grep -q -x -- "$name" "$releases/history" || continue
    if ! printf '%s\n' "$wanted" "$1" | grep -q -x -- "$name"; then
      rm -rf "${releases:?}/$name"
      say "removed kept release $name (more than $keep ago)"
    fi
  done
  rm -f "$releases"/.incoming-*
  say "release $1 is current"
}

kept_release() {
  [ -f "$releases/$1/yulius" ] || die "release $1 is not kept here"
  [ "$(commit_of "$releases/$1/yulius")" = "$1" ] || die "the kept binary of release $1 names another commit"
}

current_release() {
  cur="$(commit_of "$bin")"
  [ -n "$cur" ] || die "no Yulius that names its commit at $bin: prepare the VM first (docs/operations.md, \"Yulius's own staging VM\")"
}

receive() {
  incoming="$releases/.incoming-$1"
  [ -f "$incoming" ] || die "nothing was uploaded for $1"
  if [ "$(sha_of "$incoming")" != "$2" ]; then
    rm -f "$incoming"
    die "the upload of $1 does not have SHA-256 $2: removed"
  fi
  chmod 0755 "$incoming"
  if [ "$(commit_of "$incoming")" != "$1" ]; then
    rm -f "$incoming"
    die "release $1 does not run here or names another commit: built for another system?"
  fi
  mkdir -p -m 0700 "$releases/$1"
  chmod 0700 "$releases"
  mv "$incoming" "$releases/$1/yulius"
  # In the history from its upload, so an upload whose deploy is refused is pruned in its turn.
  note "$1"
  say "kept release $1 as $releases/$1/yulius"
}

# The same release installed: the binary is its bytes.
installed_is() { [ "$cur" = "$1" ] && [ "$(sha_of "$bin")" = "$(sha_of "$releases/$1/yulius")" ]; }

activate() {
  kept_release "$1"
  refuse_while_updating
  unit
  current_release
  if installed_is "$1"; then
    already "$1"
    record "$1"
    return 0
  fi
  migration_check "$1" "$2"
  keep_binary "$cur"
  prev="$cur"
  stop_server
  snapshot "$cur"
  switch "$1"
  record "$1"
}

rollback() {
  kept_release "$1"
  refuse_while_updating
  unit
  current_release
  if installed_is "$1"; then
    already "$1"
    record "$1"
    return 0
  fi
  [ -f "$store/yulius.db" ] || die "no store at $store/yulius.db"
  keep_binary "$cur"
  prev="$cur"
  stop_server
  snapshot "$cur"
  # From here the live store may change: an exit puts $prev's binary and store back.
  phase=installed
  live_schema="$(schema_of "$releases/$cur/store/counts.json")"
  own="$releases/$1/store/counts.json"
  if [ -f "$own" ] && [ "$(schema_of "$own")" != "$live_schema" ]; then
    restore "$1"
    say "release $1 has schema $(schema_of "$own"), the live store $live_schema: the store release $1 left is restored (what $cur wrote is kept in $releases/$cur/store)"
  elif [ -f "$own" ]; then
    say "release $1 has the live store's schema $live_schema: the live store is kept"
  else
    say "no store of release $1 is kept: the live store (schema $live_schema) is kept, which it must read"
  fi
  switch "$1"
  record "$1"
}

verify() {
  unit
  current="$(cat "$releases/current" 2>/dev/null)" || die "no release is recorded current: CD has not deployed here"
  [ "$1" = - ] || [ "$1" = "$current" ] || die "release $current is current, not $1"
  [ -f "$releases/$current/yulius" ] || die "release $current is not kept here"
  [ "$(sha_of "$bin")" = "$(sha_of "$releases/$current/yulius")" ] || die "$bin is not release $current"
  systemctl is-active "$service" </dev/null >/dev/null 2>&1 || die "$service is not active (journalctl -u $service)"
  running="$(running_commit)"
  [ "$running" = "$current" ] || die "/internal/version reports ${running:-nothing}, not release $current"
  say "release $current runs: /internal/version reports it"

  case "$public" in https://*) ;; *) die "the unit's YULIUS_PUBLIC_URL is not an https:// URL: ${public:-unset}" ;; esac
  code="$(curl -sS -o /dev/null -w '%{http_code}' --max-time 15 "${public%/}/healthz" </dev/null 2>/dev/null || true)"
  [ "$code" = 200 ] ||
    die "${public%/}/healthz answers ${code:-nothing}, not 200, over HTTPS: does the reverse proxy serve its certificate? (systemctl reload caddy; docs/operations.md §4)"
  say "${public%/}/healthz answers 200 over HTTPS"

  rc=0
  set -f
  doctor="$(cd "$store" && runuser -u "$unit_user" -- env -i $unit_env "$bin" doctor </dev/null 2>&1)" || rc=$?
  set +f
  printf '%s\n' "$doctor" || true
  [ "$rc" = 0 ] || die "the contained doctor (as $unit_user, with the unit's environment) failed (exit $rc): see its lines above"
  say "the contained doctor passes"

  live="$(counts "$store/yulius.db" live)"
  printf '%s\n' "$live" >"$scratch/live.json"
  live_schema="$(schema_of "$scratch/live.json")"
  if [ "$2" != - ]; then
    [ "$live_schema" = "$2" ] || die "the store has schema $live_schema, not $2 (the release's migrations)"
    check="$releases/$current/migration.json"
    [ -f "$check" ] || die "no migration check of release $current is kept ($check): it was not installed by this CD's deploy; deploy another release, then this one"
    say "the store has schema $live_schema; the release's migration check: $(cat "$check")"
  elif [ -f "$releases/$current/store/counts.json" ]; then
    own="$(schema_of "$releases/$current/store/counts.json")"
    [ "$live_schema" = "$own" ] || die "the store has schema $live_schema, not the $own release $current left"
    say "the store has schema $live_schema, the one release $current left"
  else
    say "the store has schema $live_schema (no store of release $current is kept to compare)"
  fi

  if [ "$3" = 1 ]; then
    line="$(printf '%s\n' "$doctor" | grep -E '^[a-zA-Z]+ +sandbox Docker ' | head -n 1 || true)"
    case "$line" in
      ok\ *)
        say "the sandbox Docker smoke test (yulius doctor --docker-smoke, as root)"
        rc=0
        timeout 900 "$bin" doctor --docker-smoke --data-dir "$store" </dev/null || rc=$?
        [ "$rc" = 0 ] || die "the sandbox Docker smoke test failed (exit $rc): see its lines above"
        ;;
      *) say "the sandbox Docker smoke test does not apply: the doctor offers sandboxes no Docker (${line:-no such line})" ;;
    esac
  fi
  say "release $current verified"
}

# valid_deletes <list>: every comma-separated item is <schema>:<table>.
valid_deletes() {
  case "$1" in '' | ,* | *, | *,,*) return 1 ;; esac
  rest="$1,"
  while [ -n "$rest" ]; do
    pair="${rest%%,*}"
    rest="${rest#*,}"
    at="${pair%%:*}"
    table="${pair#*:}"
    case "$at" in '' | *[!0-9]*) return 1 ;; esac
    case "$table" in '' | [!a-z_]* | *[!a-z0-9_]*) return 1 ;; esac
  done
}

# lock: one CD step at a time on this VM (a run by hand beside a CD run waits up to 10 minutes).
lock() {
  mkdir -p -m 0700 "$releases"
  exec 9>"$releases/.lock"
  flock -w "$lock_wait" 9 || die "another CD step holds $releases/.lock (waited ${lock_wait}s)"
}

main() {
  [ "$(id -u)" = 0 ] || die "CD runs as root on the VM (root, or a user with passwordless sudo)"
  command="${1:-}"
  release="${2:-}"
  case "$command" in
    arch) uname -m ;;
    has)
      valid_release "$release" || die "not a release: $release"
      [ -f "$releases/$release/yulius" ] || exit 3
      ;;
    receive)
      valid_release "$release" || die "not a release: $release"
      case "${3:-}" in '' | *[!0-9a-f]*) die "not a SHA-256: ${3:-}" ;; esac
      lock
      receive "$release" "$3"
      ;;
    activate | rollback)
      valid_release "$release" || die "not a release: $release"
      tables=-
      if [ "$command" = activate ]; then
        tables="${3:-}"
        [ "$tables" = - ] || valid_deletes "$tables" || die "not a list of <schema>:<table>: $tables"
      fi
      # A lost connection does not stop a switch half way.
      trap '' HUP PIPE
      trap recover EXIT
      trap 'exit 130' INT TERM
      lock
      refuse_while_held
      if [ "$command" = activate ]; then activate "$release" "$tables"; else rollback "$release"; fi
      ;;
    verify)
      [ "$release" = - ] || valid_release "$release" || die "not a release: $release"
      case "${3:-}" in - | [0-9] | [0-9][0-9] | [0-9][0-9][0-9] | [0-9][0-9][0-9][0-9]) ;; *) die "not a schema: ${3:-}" ;; esac
      case "${4:-}" in 0 | 1) ;; *) die "not 0 or 1: ${4:-}" ;; esac
      lock
      refuse_while_held
      verify "$release" "$3" "$4"
      ;;
    *) die "unknown command: $command" ;;
  esac
}

main "$@"
