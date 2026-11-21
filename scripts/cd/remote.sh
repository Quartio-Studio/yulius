#!/bin/sh
# CD's steps on Yulius's staging VM, as root (ADR-0430; docs/operations.md, "Yulius's own staging
# VM"). Piped to `sh -s -- <command> [args]` over SSH by scripts/cd/lib.sh; never installed there.
#
#   arch                     the VM's architecture (uname -m)
#   has <release>            exit 0 when the release's binary is kept here, 3 when it is not
#   receive <release> <sha256>
#                            the uploaded .incoming-<release> checked and kept as
#                            /var/lib/yulius-releases/<release>/yulius (root's)
#   activate <release>       yulius.service stopped; the replaced release's binary and the store it
#                            left kept under it; the release installed and started (it migrates
#                            the store at its start); put back with that store when it does not
#                            report its commit; recorded current; the last 10 releases kept
#   rollback <release>       the same switch to a kept release, with the store that release left
#                            when the live store has another schema, else with the live store
#   verify <release|-> <schema|-> <0|1>
#                            the release is current, installed and running (/internal/version);
#                            the public URL answers /healthz; the contained doctor passes; no
#                            table lost rows since the store the release started from; the
#                            store's schema (when given); the Docker smoke test (when 1, where
#                            the doctor offers sandboxes Docker)
#
# Only yulius.service is stopped and started: agent sessions live in yulius-sandboxes.service.
# Everything is in functions and runs from the last line, so sh has read the whole script from
# its standard input before any command could read from it.
set -eu

# YULIUS_CD_ROOT and YULIUS_CD_START_WAIT are for scripts/test_cd.py's fake root; SSH passes
# neither to the VM.
root="${YULIUS_CD_ROOT:-}"
releases="$root/var/lib/yulius-releases"
bin="$root/usr/local/bin/yulius"
service=yulius.service
helper=yulius-update.service
keep=10
start_wait="${YULIUS_CD_START_WAIT:-120}"

say() { echo "vm: $*"; }
die() {
  echo "vm: $*" >&2
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
  store="$root${data:-/var/lib/yulius}"
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

refuse_while_updating() {
  if systemctl is-active "$helper" </dev/null >/dev/null 2>&1; then
    die "the update helper ($helper) runs: nothing changed; deploy again once it ended (on staging it stays disabled: docs/operations.md)"
  fi
}

# counts <db> [live]: one JSON line, {"schema": n, "tables": {name: rows}}, of a copy of the store
# (a live one through SQLite's backup, read-only; a stopped one with its WAL).
counts() {
  python3 -I - "$1" "${2:-}" <<'EOF'
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
restore() {
  src="$releases/$1/store"
  [ -f "$src/yulius.db" ] || die "no store of release $1 is kept in $src"
  owner="$(stat -c %U:%G "$store")"
  rm -f "$store/yulius.db" "$store/yulius.db-wal" "$store/yulius.db-shm"
  cp "$src/yulius.db" "$store/yulius.db"
  if [ -f "$src/yulius.db-wal" ]; then cp "$src/yulius.db-wal" "$store/yulius.db-wal"; fi
  for f in "$store/yulius.db" "$store/yulius.db-wal"; do
    [ -f "$f" ] || continue
    chown "$owner" "$f"
    chmod 0600 "$f"
  done
}

install_bin() {
  cp "$releases/$1/yulius" "$bin.new"
  chown root:root "$bin.new"
  chmod 0755 "$bin.new"
  mv -f "$bin.new" "$bin"
}

# keep_binary <release>: the running binary kept as that release (one CD did not install, such as
# the operator's first, becomes a release a rollback can go back to).
keep_binary() {
  [ -f "$releases/$1/yulius" ] && return 0
  mkdir -p -m 0700 "$releases/$1"
  cp "$bin" "$releases/$1/yulius"
  chmod 0755 "$releases/$1/yulius"
  say "kept the running binary as release $1"
}

stop_server() {
  say "stopping $service (agent sessions keep running in yulius-sandboxes.service)"
  systemctl stop "$service" </dev/null
}

# switch <release> <previous>: install and start <release>; when it does not report its commit,
# put <previous> and the store it left back and fail.
switch() {
  install_bin "$1"
  say "starting $service with release $1"
  systemctl start "$service" </dev/null || true
  if wait_for "$1"; then
    say "release $1 reports its commit at /internal/version"
    return 0
  fi
  say "release $1 did not report its commit within ${start_wait}s: putting back release $2 and its store"
  systemctl stop "$service" </dev/null || true
  install_bin "$2"
  restore "$2"
  systemctl start "$service" </dev/null || true
  wait_for "$2" ||
    die "release $1 did not start, and release $2 with its store does not answer either: restore by hand from $releases/$2 (journalctl -u $service)"
  die "release $1 did not report its commit (journalctl -u $service): put back release $2 and its store, which run"
}

# The installed release again: running it is all there is to do.
already() {
  say "release $1 is installed already"
  if [ "$(running_commit)" != "$1" ]; then
    systemctl start "$service" </dev/null || true
    wait_for "$1" || die "release $1 is installed but does not answer (journalctl -u $service)"
  fi
}

# record <release> <replaced>: the release is current; the last $keep releases used (deployed or
# rolled back to), by their last use, are kept, and the current one.
record() {
  mkdir -p -m 0700 "$releases"
  last="$(tail -n 1 "$releases/history" 2>/dev/null || true)"
  if [ -n "$2" ] && [ "$last" != "$2" ]; then
    echo "$2" >>"$releases/history"
    last="$2"
  fi
  echo "$1" >"$releases/current.new"
  mv "$releases/current.new" "$releases/current"
  [ "$last" = "$1" ] || echo "$1" >>"$releases/history"
  # Newest first, each release once, at its last use.
  wanted="$(tail -n 200 "$releases/history" | awk '{ l[n++] = $0 } END { for (i = n - 1; i >= 0; i--) if (!seen[l[i]]++) print l[i] }' | head -n "$keep")"
  find "$releases" -mindepth 1 -maxdepth 1 -type d | while read -r dir; do
    name="$(basename "$dir")"
    valid_release "$name" || continue
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
  say "kept release $1 as $releases/$1/yulius"
}

activate() {
  kept_release "$1"
  refuse_while_updating
  unit
  current_release
  if [ "$cur" = "$1" ] && [ "$(sha_of "$bin")" = "$(sha_of "$releases/$1/yulius")" ]; then
    already "$1"
    record "$1" ""
    return 0
  fi
  keep_binary "$cur"
  stop_server
  snapshot "$cur"
  cp "$releases/$cur/store/counts.json" "$releases/base.json"
  switch "$1" "$cur"
  record "$1" "$cur"
}

rollback() {
  kept_release "$1"
  refuse_while_updating
  unit
  current_release
  if [ "$cur" = "$1" ] && [ "$(sha_of "$bin")" = "$(sha_of "$releases/$1/yulius")" ]; then
    already "$1"
    record "$1" ""
    return 0
  fi
  keep_binary "$cur"
  stop_server
  snapshot "$cur"
  live_schema="$(schema_of "$releases/$cur/store/counts.json")"
  own="$releases/$1/store/counts.json"
  if [ -f "$own" ] && [ "$(schema_of "$own")" != "$live_schema" ]; then
    restore "$1"
    cp "$own" "$releases/base.json"
    say "release $1 has schema $(schema_of "$own"), the live store $live_schema: the store release $1 left is restored (what $cur wrote is kept in $releases/$cur/store)"
  else
    cp "$releases/$cur/store/counts.json" "$releases/base.json"
    if [ -f "$own" ]; then
      say "release $1 has the live store's schema $live_schema: the live store is kept"
    else
      say "no store of release $1 is kept: the live store (schema $live_schema) is kept, which it must read"
    fi
  fi
  switch "$1" "$cur"
  record "$1" "$cur"
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
  doctor="$(cd "$store" && runuser -u "$unit_user" -- env -i $unit_env "$bin" doctor </dev/null 2>&1)" || rc=$?
  printf '%s\n' "$doctor"
  [ "$rc" = 0 ] || die "the contained doctor (as $unit_user, with the unit's environment) failed (exit $rc): see its lines above"
  say "the contained doctor passes"

  [ -f "$releases/base.json" ] || die "no counts of the store release $current started from ($releases/base.json)"
  live="$(counts "$store/yulius.db" live)"
  printf '%s\n' "$live" >"$releases/live.json"
  python3 -I - "$releases/base.json" "$releases/live.json" "$2" <<'EOF'
import json, sys
base, live = (json.load(open(p)) for p in sys.argv[1:3])
want = sys.argv[3]
problems = []
for name, rows in sorted(base["tables"].items()):
    now = live["tables"].get(name)
    if now is None:
        problems.append("%s: gone" % name)
    elif now < rows:
        problems.append("%s: %d rows before, %d now" % (name, rows, now))
if live["schema"] < base["schema"]:
    problems.append("schema %d, older than the %d it started from" % (live["schema"], base["schema"]))
if want != "-" and live["schema"] != int(want):
    problems.append("schema %d, not %s (the release's migrations)" % (live["schema"], want))
if problems:
    sys.exit("vm: the store lost data or did not migrate as the release says: " + "; ".join(problems))
print("vm: the store: schema %d (from %d), %d tables, none lost rows since the store the release started from"
      % (live["schema"], base["schema"], len(live["tables"])))
EOF

  if [ "$3" = 1 ]; then
    line="$(printf '%s\n' "$doctor" | grep -E '^[a-zA-Z]+ +sandbox Docker ' | head -n 1 || true)"
    case "$line" in
      ok\ *)
        say "the sandbox Docker smoke test (yulius doctor --docker-smoke, as root)"
        rc=0
        timeout 900 "$bin" doctor --docker-smoke </dev/null || rc=$?
        [ "$rc" = 0 ] || die "the sandbox Docker smoke test failed (exit $rc): see its lines above"
        ;;
      *) say "the sandbox Docker smoke test does not apply: the doctor offers sandboxes no Docker (${line:-no such line})" ;;
    esac
  fi
  say "release $current verified"
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
      receive "$release" "$3"
      ;;
    activate | rollback)
      valid_release "$release" || die "not a release: $release"
      "$command" "$release"
      ;;
    verify)
      [ "$release" = - ] || valid_release "$release" || die "not a release: $release"
      case "${3:-}" in - | [0-9] | [0-9][0-9] | [0-9][0-9][0-9] | [0-9][0-9][0-9][0-9]) ;; *) die "not a schema: ${3:-}" ;; esac
      case "${4:-}" in 0 | 1) ;; *) die "not 0 or 1: ${4:-}" ;; esac
      verify "$release" "$3" "$4"
      ;;
    *) die "unknown command: $command" ;;
  esac
}

main "$@"
