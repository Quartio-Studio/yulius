# Shared by scripts/cd/deploy.sh, verify.sh and rollback.sh (ADR-0430; docs/operations.md,
# "Yulius's own staging VM"). Sourced, POSIX sh. Every step on the VM is scripts/cd/remote.sh,
# piped to sh as root over SSH.
#
# The one environment, [cd.dev] (the staging VM), and what Yulius gives its runs:
#   STAGING_VM_HOST     [user@]host[:port] of the VM (user root unless named): a project variable
#   STAGING_VM_SSH_KEY  its private key (an agent secret; loaded into an ssh-agent, never written)
# The VM's host key is pinned in scripts/cd/known_hosts as "yulius-staging <type> <key>".

cd_dir="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$cd_dir/../.." && pwd)"

say() { echo "cd: $*"; }
die() {
  echo "cd: $*" >&2
  exit 1
}

# A release is the commit it is built from: 7 to 64 lowercase hex digits.
valid_release() {
  case "$1" in '' | *[!0-9a-f]*) return 1 ;; esac
  [ "${#1}" -ge 7 ] && [ "${#1}" -le 64 ]
}

# cd_env <env>: the environment's names, or a usage error. Production is a promotion of dev to
# main ([cd.prod]); it deploys nothing here.
cd_env() {
  case "${1:-}" in
    dev) ;;
    *)
      echo "usage: $0 dev" >&2
      exit 2
      ;;
  esac
  env_name="$1"
  host_alias="yulius-staging"
  known_hosts="$cd_dir/known_hosts"
  cache="${YULIUS_CD_CACHE:-}"
}

need() {
  for tool; do
    command -v "$tool" >/dev/null 2>&1 || die "$tool is not installed here"
  done
}

# ssh_start: the target parsed, the host key found pinned, the key loaded into an agent that is
# stopped when the script exits.
ssh_start() {
  # Never the caller's agent: ssh_stop stops only the one started here.
  unset SSH_AGENT_PID SSH_AUTH_SOCK
  need ssh ssh-agent ssh-add awk
  target="${STAGING_VM_HOST:-}"
  key="${STAGING_VM_SSH_KEY:-}"
  missing=""
  [ -n "$target" ] || missing="STAGING_VM_HOST"
  [ -n "$key" ] || missing="$missing STAGING_VM_SSH_KEY"
  [ -z "$missing" ] ||
    die "missing for $env_name: ${missing# }: a person sets the variable STAGING_VM_HOST and the agent secret STAGING_VM_SSH_KEY on the Secrets page and allows the secret outside the proxy"
  case "$target" in
    *@*) user="${target%%@*}" host="${target#*@}" ;;
    *) user=root host="$target" ;;
  esac
  port=22
  case "$host" in
    *:*:*) ;; # an IPv6 address alone: port 22
    *:*) port="${host##*:}" host="${host%:*}" ;;
  esac
  case "$port" in '' | *[!0-9]*) die "STAGING_VM_HOST: the port is not a number" ;; esac
  # Neither may start with "-" (ssh would read an option) nor hold anything a name does not.
  case "$user" in '' | -* | *[!A-Za-z0-9._-]*) die "STAGING_VM_HOST is not [user@]host[:port]" ;; esac
  case "$host" in '' | -* | *[!A-Za-z0-9.:_-]*) die "STAGING_VM_HOST is not [user@]host[:port]" ;; esac
  grep -q "^$host_alias " "$known_hosts" 2>/dev/null ||
    die "no host key for $host_alias in $known_hosts: pin the VM's key (\"$host_alias <type> <key>\", from /etc/ssh/ssh_host_ed25519_key.pub on the VM) in a pull request"
  if [ "$user" = root ]; then sudo=""; else sudo="sudo -n "; fi
  # A short path: a socket's path may not exceed about 100 bytes.
  agent_dir="$(mktemp -d /tmp/yulius-cd.XXXXXX)"
  trap ssh_stop EXIT
  trap 'exit 130' HUP INT TERM
  ssh-agent -a "$agent_dir/agent" -s >"$agent_dir/env" || die "ssh-agent does not start"
  . "$agent_dir/env" >/dev/null
  # A key stored on one line has its line breaks as the two characters \n: put them back. The
  # agent forgets it after 2 hours, should a killed run leave the agent behind.
  printf '%s\n' "$key" | awk '{ gsub(/\\n/, "\n"); print }' | ssh-add -q -t 7200 - 2>/dev/null ||
    die "STAGING_VM_SSH_KEY is not a private key ssh-add loads"
  say "$env_name is $host_alias ($user@<STAGING_VM_HOST>, port $port), its host key pinned"
}

ssh_stop() {
  if [ -n "${SSH_AGENT_PID:-}" ]; then ssh-agent -k >/dev/null 2>&1 || true; fi
  rm -rf "${agent_dir:-/nonexistent}"
}

ssh_vm() {
  ssh -F /dev/null -o BatchMode=yes -o StrictHostKeyChecking=yes -o UserKnownHostsFile="$known_hosts" \
    -o GlobalKnownHostsFile=/dev/null -o HostKeyAlias="$host_alias" -o ConnectTimeout=20 \
    -o ServerAliveInterval=15 -o ServerAliveCountMax=8 -o LogLevel=ERROR -p "$port" \
    "$user@$host" "$@"
}

# remote <command> [args]: scripts/cd/remote.sh run on the VM as root. Its arguments are
# release ids, hex digests, numbers or "-", checked before.
remote() {
  ssh_vm "${sudo}sh -s -- $*" <"$cd_dir/remote.sh"
}

# upload <release> <file>: the binary into the VM's kept releases, checked by its SHA-256.
upload() {
  digest="$(sha256sum "$2" | cut -d ' ' -f 1)"
  say "copying release $1 to the VM (SHA-256 $digest)"
  ssh_vm "${sudo}sh -c 'umask 077 && r=\"\${YULIUS_CD_ROOT:-}/var/lib/yulius-releases\" && mkdir -p \"\$r\" && cat > \"\$r/.incoming-$1\"'" <"$2"
  remote receive "$1" "$digest"
}

# kept_on_vm <release>: 0 kept, 1 not kept; dies when the VM cannot be asked (remote.sh's has
# answers 3 for "not kept": anything else is SSH or the VM failing).
kept_on_vm() {
  kept_rc=0
  remote has "$1" || kept_rc=$?
  case "$kept_rc" in
    0) return 0 ;;
    3) return 1 ;;
    *) die "cannot ask the VM whether it keeps release $1 (exit $kept_rc): see the lines above" ;;
  esac
}

# The release's binary in the CD cache (kept per environment by Yulius).
cached() {
  echo "$cache/releases/$1/yulius"
}
